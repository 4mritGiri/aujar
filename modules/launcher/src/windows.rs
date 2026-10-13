//! Window switching: `< firefox`, `win terminal`, `switch to mail`, or just `<` for all windows.
//!
//! Linux has no single API for other applications' windows, so this talks to whatever the
//! session offers:
//!
//! | Session | Backend |
//! |---|---|
//! | GNOME (Wayland or X11) | the Aujar Shell extension over D-Bus (`aujar gnome-extension install`) |
//! | Sway | `swaymsg` |
//! | Hyprland | `hyprctl` |
//! | Any X11 window manager | `wmctrl` |
//!
//! Listing windows runs a helper process, so it only happens for the trigger forms above, never
//! for ordinary queries, and the list is cached for a moment while you type.

use crate::{
    icons::resolve_icon,
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    ranking::score,
};
use std::{
    collections::HashMap,
    io::Read,
    process::{Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Maximum windows shown.
const MAX_RESULTS: usize = 8;
/// A listing is reused for this long (typing a filter must not spawn a process per key).
const LIST_TTL: Duration = Duration::from_millis(1500);
/// Helper processes that take longer than this are killed.
const HELPER_TIMEOUT: Duration = Duration::from_secs(2);

pub const GNOME_BUS_NAME: &str = "dev.aujar.Windows";
pub const GNOME_OBJECT_PATH: &str = "/dev/aujar/Windows";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Gnome,
    Sway,
    Hyprland,
    X11,
}

impl Backend {
    const fn key(self) -> &'static str {
        match self {
            Self::Gnome => "gnome",
            Self::Sway => "sway",
            Self::Hyprland => "hyprland",
            Self::X11 => "x11",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        [Self::Gnome, Self::Sway, Self::Hyprland, Self::X11]
            .into_iter()
            .find(|backend| backend.key() == key)
    }

    /// Whether `token` has the shape this backend's window ids have. Ids come from clients, so
    /// they are validated before they become command-line arguments.
    fn valid_token(self, token: &str) -> bool {
        match self {
            Self::Gnome => token.parse::<u32>().is_ok(),
            Self::Sway => token.parse::<u64>().is_ok(),
            Self::Hyprland | Self::X11 => {
                token
                    .strip_prefix("0x")
                    .is_some_and(|hex| !hex.is_empty() && hex.len() <= 16 && hex.chars().all(|c| c.is_ascii_hexdigit()))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    /// Backend-specific id (validated by [`Backend::valid_token`]).
    id: String,
    title: String,
    app: String,
    /// Desktop-file id or class, used to find an icon.
    icon_hint: Option<String>,
    workspace: Option<String>,
    focused: bool,
}

// ---- Running helpers --------------------------------------------------------------------

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Run `program`, returning its stdout, or an error after [`HELPER_TIMEOUT`].
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run `{program}`: {error}"))?;

    let mut stdout = child.stdout.take().ok_or("no output from helper")?;
    let (sender, receiver) = std::sync::mpsc::channel();

    // Read on a thread: a large listing would fill the pipe and stall the child otherwise.
    std::thread::spawn(move || {
        let mut text = String::new();
        let _ = stdout.read_to_string(&mut text);
        let _ = sender.send(text);
    });

    match receiver.recv_timeout(HELPER_TIMEOUT) {
        Ok(text) => {
            let status = child.wait().map_err(|error| error.to_string())?;

            if status.success() {
                Ok(text)
            } else {
                Err(format!("`{program}` failed"))
            }
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();

            Err(format!("`{program}` did not answer"))
        }
    }
}

// ---- Parsers ----------------------------------------------------------------------------

/// `busctl --json=short call ... List` prints `{"type":"s","data":["[...]"]}`.
fn parse_gnome(output: &str) -> Option<Vec<WindowInfo>> {
    let reply: serde_json::Value = serde_json::from_str(output).ok()?;
    let inner = reply.get("data")?.as_array()?.first()?.as_str()?;
    let list: serde_json::Value = serde_json::from_str(inner).ok()?;

    let windows = list
        .as_array()?
        .iter()
        .filter_map(|window| {
            let text = |key: &str| window.get(key).and_then(|v| v.as_str()).map(str::to_owned);

            Some(WindowInfo {
                id: window.get("id")?.as_u64()?.to_string(),
                title: text("title").unwrap_or_default(),
                app: text("app").unwrap_or_default(),
                icon_hint: text("app_id").or_else(|| text("wm_class")),
                workspace: window
                    .get("workspace")
                    .and_then(|v| v.as_i64())
                    .filter(|workspace| *workspace >= 0)
                    .map(|workspace| (workspace + 1).to_string()),
                focused: window.get("focused").and_then(|v| v.as_bool()).unwrap_or(false),
            })
        })
        .collect();

    Some(windows)
}

fn collect_sway(node: &serde_json::Value, workspace: Option<&str>, out: &mut Vec<WindowInfo>) {
    let kind = node.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let name = node.get("name").and_then(|v| v.as_str());

    let workspace = if kind == "workspace" { name } else { workspace };

    let is_window = matches!(kind, "con" | "floating_con") && node.get("pid").is_some();
    let has_children = ["nodes", "floating_nodes"]
        .iter()
        .any(|key| node.get(key).and_then(|v| v.as_array()).is_some_and(|c| !c.is_empty()));

    if is_window && !has_children {
        let app = node
            .get("app_id")
            .and_then(|v| v.as_str())
            .or_else(|| {
                node.get("window_properties")
                    .and_then(|props| props.get("class"))
                    .and_then(|v| v.as_str())
            })
            .unwrap_or("")
            .to_owned();

        if let Some(id) = node.get("id").and_then(|v| v.as_u64()) {
            out.push(WindowInfo {
                id: id.to_string(),
                title: name.unwrap_or("").to_owned(),
                icon_hint: (!app.is_empty()).then(|| app.clone()),
                app,
                workspace: workspace.map(str::to_owned),
                focused: node.get("focused").and_then(|v| v.as_bool()).unwrap_or(false),
            });
        }
    }

    for key in ["nodes", "floating_nodes"] {
        if let Some(children) = node.get(key).and_then(|v| v.as_array()) {
            for child in children {
                collect_sway(child, workspace, out);
            }
        }
    }
}

fn parse_sway(output: &str) -> Option<Vec<WindowInfo>> {
    let tree: serde_json::Value = serde_json::from_str(output).ok()?;
    let mut windows = Vec::new();

    collect_sway(&tree, None, &mut windows);

    Some(windows)
}

fn parse_hyprland(output: &str) -> Option<Vec<WindowInfo>> {
    let clients: serde_json::Value = serde_json::from_str(output).ok()?;

    let windows = clients
        .as_array()?
        .iter()
        .filter(|client| client.get("mapped").and_then(|v| v.as_bool()).unwrap_or(true))
        .filter_map(|client| {
            let class = client.get("class")?.as_str()?.to_owned();

            Some(WindowInfo {
                id: client.get("address")?.as_str()?.to_owned(),
                title: client.get("title").and_then(|v| v.as_str()).unwrap_or("").to_owned(),
                icon_hint: Some(class.clone()),
                app: class,
                workspace: client
                    .get("workspace")
                    .and_then(|workspace| workspace.get("name"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                // `focusHistoryID` 0 is the focused window.
                focused: client.get("focusHistoryID").and_then(|v| v.as_i64()) == Some(0),
            })
        })
        .collect();

    Some(windows)
}

/// `wmctrl -lx`: `0x04200003  0 firefox.Firefox  host Page title`.
fn parse_wmctrl(output: &str) -> Vec<WindowInfo> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();

            let id = fields.next()?.to_owned();
            let desktop: i64 = fields.next()?.parse().ok()?;
            let class = fields.next()?;
            let _host = fields.next()?;
            let title = fields.collect::<Vec<_>>().join(" ");

            // Desktop -1 is "all desktops": panels, docks and the like.
            if desktop < 0 {
                return None;
            }

            let app = class.rsplit_once('.').map_or(class, |(_, name)| name).to_owned();

            Some(WindowInfo {
                id,
                title,
                icon_hint: Some(class.split('.').next().unwrap_or(class).to_lowercase()),
                app,
                workspace: Some((desktop + 1).to_string()),
                focused: false,
            })
        })
        .collect()
}

// ---- Backends ---------------------------------------------------------------------------

const GNOME_HELP: &str = "Window switching on GNOME needs the Aujar Shell extension. \
Run `aujar gnome-extension install`, then log out and back in.";

fn gnome_args<'a>(method: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "--user",
        "--json=short",
        "--timeout=2",
        "call",
        GNOME_BUS_NAME,
        GNOME_OBJECT_PATH,
        GNOME_BUS_NAME,
        method,
    ];

    args.extend_from_slice(extra);
    args
}

fn list_backend(backend: Backend) -> Result<Vec<WindowInfo>, String> {
    let unexpected = || "the window list was in an unexpected format".to_owned();

    match backend {
        Backend::Gnome => run("busctl", &gnome_args("List", &[]))
            .map_err(|_| GNOME_HELP.to_owned())
            .and_then(|out| parse_gnome(&out).ok_or_else(unexpected)),
        Backend::Sway => run("swaymsg", &["-t", "get_tree"])
            .and_then(|out| parse_sway(&out).ok_or_else(unexpected)),
        Backend::Hyprland => run("hyprctl", &["clients", "-j"])
            .and_then(|out| parse_hyprland(&out).ok_or_else(unexpected)),
        Backend::X11 => run("wmctrl", &["-lx"]).map(|out| parse_wmctrl(&out)),
    }
}

/// Backends worth trying in this session, best first.
fn candidates() -> Vec<Backend> {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let mut found = Vec::new();

    if var("SWAYSOCK").is_some() && on_path("swaymsg") {
        found.push(Backend::Sway);
    }

    if var("HYPRLAND_INSTANCE_SIGNATURE").is_some() && on_path("hyprctl") {
        found.push(Backend::Hyprland);
    }

    let gnome = var("XDG_CURRENT_DESKTOP").is_some_and(|desktop| desktop.to_uppercase().contains("GNOME"));

    if gnome && on_path("busctl") {
        found.push(Backend::Gnome);
    }

    // wmctrl only sees X11 windows, so on a Wayland session it would list a misleading subset.
    let wayland = var("XDG_SESSION_TYPE").is_some_and(|kind| kind == "wayland");

    if var("DISPLAY").is_some() && !wayland && on_path("wmctrl") {
        found.push(Backend::X11);
    }

    found
}

/// A launchable command that raises the window.
fn activation(backend: Backend, id: &str) -> Option<LaunchSpec> {
    if !backend.valid_token(id) {
        return None;
    }

    let (program, args): (&str, Vec<String>) = match backend {
        Backend::Gnome => (
            "busctl",
            gnome_args("Activate", &["u", id]).into_iter().map(str::to_owned).collect(),
        ),
        Backend::Sway => ("swaymsg", vec![format!("[con_id={id}]"), "focus".to_owned()]),
        Backend::Hyprland => (
            "hyprctl",
            vec!["dispatch".to_owned(), "focuswindow".to_owned(), format!("address:{id}")],
        ),
        Backend::X11 => ("wmctrl", vec!["-ia".to_owned(), id.to_owned()]),
    };

    Some(LaunchSpec {
        title: "window".to_owned(),
        program: program.to_owned(),
        args,
    })
}

// ---- Provider ---------------------------------------------------------------------------

/// What the trigger forms ask for: the filter text, which may be empty.
fn parse_query(query: &str) -> Option<String> {
    let trimmed = query.trim();
    let lower = trimmed.to_lowercase();

    if let Some(rest) = trimmed.strip_prefix('<') {
        return Some(rest.trim().to_owned());
    }

    if lower == "windows" {
        return Some(String::new());
    }

    for prefix in ["switch to ", "switch ", "windows ", "window ", "win "] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let rest = rest.trim();

            if !rest.is_empty() {
                return Some(rest.to_owned());
            }
        }
    }

    None
}

pub struct WindowsProvider {
    cache: Mutex<Option<(Instant, Backend, Vec<WindowInfo>)>>,
    icons: Mutex<HashMap<String, Option<String>>>,
}

impl WindowsProvider {
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(None),
            icons: Mutex::new(HashMap::new()),
        }
    }

    fn windows(&self) -> Result<(Backend, Vec<WindowInfo>), String> {
        let mut cache = self.cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some((at, backend, windows)) = cache.as_ref() {
            if at.elapsed() < LIST_TTL {
                return Ok((*backend, windows.clone()));
            }
        }

        let candidates = candidates();

        if candidates.is_empty() {
            return Err(
                "Window switching works on GNOME (with the Aujar extension), Sway, Hyprland and X11 (wmctrl)."
                    .to_owned(),
            );
        }

        let mut last_error = String::new();

        for backend in candidates {
            match list_backend(backend) {
                Ok(windows) => {
                    *cache = Some((Instant::now(), backend, windows.clone()));

                    return Ok((backend, windows));
                }
                Err(error) => last_error = error,
            }
        }

        Err(last_error)
    }

    fn icon_for(&self, window: &WindowInfo) -> Option<String> {
        let hint = window.icon_hint.as_deref()?;
        let name = hint.strip_suffix(".desktop").unwrap_or(hint);
        let mut icons = self.icons.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        icons
            .entry(name.to_owned())
            .or_insert_with(|| resolve_icon(name).or_else(|| resolve_icon(&name.to_lowercase())))
            .clone()
    }
}

impl Default for WindowsProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }

    let mut out: String = text.chars().take(limit).collect();

    out.push('\u{2026}');
    out
}

impl Provider for WindowsProvider {
    fn id(&self) -> &'static str {
        "windows"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let Some(filter) = parse_query(query) else {
            return Vec::new();
        };

        let (backend, windows) = match self.windows() {
            Ok(listing) => listing,
            Err(message) => {
                return vec![
                    SearchResult::new("notice:windows", "Window switching isn\u{2019}t set up", ResultKind::Notice, 1850)
                        .with_subtitle(message),
                ];
            }
        };

        let mut ranked: Vec<(u32, &WindowInfo)> = windows
            .iter()
            // The launcher's own window is open while you search.
            .filter(|window| !window.app.to_lowercase().contains("aujar-launcher"))
            .filter(|window| !window.title.is_empty() || !window.app.is_empty())
            .filter_map(|window| {
                if filter.is_empty() {
                    return Some((0, window));
                }

                let best = [score(&filter, &window.title), score(&filter, &window.app)]
                    .into_iter()
                    .flatten()
                    .max()?;

                Some((best, window))
            })
            .collect();

        // Best match first; with no filter, the most recently used would be ideal but no
        // backend reports that uniformly, so keep the backend's order and put the focused
        // window last (you are already there).
        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.focused.cmp(&b.1.focused)));
        ranked.truncate(MAX_RESULTS);

        let count = ranked.len() as u32;

        ranked
            .into_iter()
            .enumerate()
            .map(|(order, (_, window))| {
                let place = match &window.workspace {
                    Some(workspace) => format!(" \u{b7} Workspace {workspace}"),
                    None => String::new(),
                };

                let app = if window.app.is_empty() { "Window" } else { window.app.as_str() };
                let title = if window.title.is_empty() { app } else { window.title.as_str() };

                SearchResult::new(
                    format!("windows:{}:{}", backend.key(), window.id),
                    truncate(title, 90),
                    ResultKind::Window,
                    1700 + (count - order as u32),
                )
                .with_subtitle(format!(
                    "{app}{place}{}",
                    if window.focused { " \u{b7} Current" } else { "" }
                ))
                .with_icon(self.icon_for(window))
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let rest = id.strip_prefix("windows:")?;
        let (key, token) = rest.split_once(':')?;

        let Some(backend) = Backend::from_key(key) else {
            return Some(Err("unknown window backend".to_owned()));
        };

        Some(activation(backend, token).ok_or_else(|| "not a window id".to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_trigger_forms() {
        assert_eq!(parse_query("<"), Some(String::new()));
        assert_eq!(parse_query("< firefox").as_deref(), Some("firefox"));
        assert_eq!(parse_query("<term").as_deref(), Some("term"));
        assert_eq!(parse_query("windows"), Some(String::new()));
        assert_eq!(parse_query("win mail").as_deref(), Some("mail"));
        assert_eq!(parse_query("Switch to Slack").as_deref(), Some("slack"));
        assert_eq!(parse_query("firefox"), None);
        assert_eq!(parse_query("win"), None);
        assert_eq!(parse_query("window "), None);
    }

    #[test]
    fn parses_the_gnome_extension_reply() {
        let inner = r#"[{"id":4242,"title":"Inbox - Mail","app":"Thunderbird","app_id":"thunderbird.desktop","workspace":1,"focused":false},
                        {"id":7,"title":"~","app":"Terminal","wm_class":"gnome-terminal-server","workspace":0,"focused":true}]"#;
        let reply = serde_json::json!({"type": "s", "data": [inner]}).to_string();

        let windows = parse_gnome(&reply).unwrap();

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, "4242");
        assert_eq!(windows[0].workspace.as_deref(), Some("2"));
        assert_eq!(windows[0].icon_hint.as_deref(), Some("thunderbird.desktop"));
        assert!(windows[1].focused);
        assert_eq!(windows[1].icon_hint.as_deref(), Some("gnome-terminal-server"));
        assert!(parse_gnome("garbage").is_none());
    }

    #[test]
    fn parses_the_sway_tree_and_skips_containers() {
        let tree = r#"{"type":"root","nodes":[{"type":"output","name":"eDP-1","nodes":[
            {"type":"workspace","name":"2","nodes":[
              {"type":"con","id":15,"name":"Docs","app_id":"org.gnome.Evince","pid":10,"focused":true,"nodes":[],"floating_nodes":[]},
              {"type":"con","id":16,"name":"split","nodes":[
                 {"type":"con","id":17,"name":"vim","pid":11,"window_properties":{"class":"XTerm"},"nodes":[],"floating_nodes":[]}
              ],"floating_nodes":[]}
            ],"floating_nodes":[]}
        ],"floating_nodes":[]}],"floating_nodes":[]}"#;

        let windows = parse_sway(tree).unwrap();

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].app, "org.gnome.Evince");
        assert_eq!(windows[0].workspace.as_deref(), Some("2"));
        assert!(windows[0].focused);
        assert_eq!(windows[1].app, "XTerm");
    }

    #[test]
    fn parses_hyprland_clients() {
        let clients = r#"[
            {"address":"0x55d1a2b3c4d0","mapped":true,"class":"firefox","title":"Docs","workspace":{"id":1,"name":"1"},"focusHistoryID":0},
            {"address":"0x55d1a2b3c500","mapped":false,"class":"ghost","title":"x","workspace":{"id":1,"name":"1"},"focusHistoryID":3}
        ]"#;

        let windows = parse_hyprland(clients).unwrap();

        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "0x55d1a2b3c4d0");
        assert!(windows[0].focused);
    }

    #[test]
    fn parses_wmctrl_and_skips_sticky_windows() {
        let output = "0x04200003  0 firefox.Firefox  host Mozilla Firefox\n\
                      0x04400001 -1 xfce4-panel.Xfce4-panel  host Panel\n\
                      0x05000007  1 Navigator.Chromium  host A title  with spaces\n";

        let windows = parse_wmctrl(output);

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].app, "Firefox");
        assert_eq!(windows[0].icon_hint.as_deref(), Some("firefox"));
        assert_eq!(windows[1].title, "A title with spaces");
        assert_eq!(windows[1].workspace.as_deref(), Some("2"));
    }

    #[test]
    fn activation_validates_ids_per_backend() {
        assert!(activation(Backend::Gnome, "4242").is_some());
        assert!(activation(Backend::Gnome, "4242; rm").is_none());
        assert!(activation(Backend::Gnome, "-1").is_none());
        assert!(activation(Backend::Sway, "17").is_some());
        assert!(activation(Backend::Sway, "x").is_none());
        assert!(activation(Backend::Hyprland, "0xdeadbeef").is_some());
        assert!(activation(Backend::Hyprland, "deadbeef").is_none());
        assert!(activation(Backend::X11, "0x4200003").is_some());
        assert!(activation(Backend::X11, "0x").is_none());
    }

    #[test]
    fn activation_builds_argument_vectors_not_shell_strings() {
        let gnome = activation(Backend::Gnome, "42").unwrap();

        assert_eq!(gnome.program, "busctl");
        assert!(gnome.args.windows(2).any(|pair| pair == ["u", "42"]));
        assert!(gnome.args.contains(&"Activate".to_owned()));

        let sway = activation(Backend::Sway, "17").unwrap();

        assert_eq!(sway.args, ["[con_id=17]", "focus"]);

        let hypr = activation(Backend::Hyprland, "0xab").unwrap();

        assert_eq!(hypr.args, ["dispatch", "focuswindow", "address:0xab"]);
    }

    #[test]
    fn resolve_rejects_unknown_backends_and_foreign_ids() {
        let provider = WindowsProvider::new();

        assert!(provider.resolve("windows:gnome:42").unwrap().is_ok());
        assert!(provider.resolve("windows:nope:42").unwrap().is_err());
        assert!(provider.resolve("windows:sway:$(x)").unwrap().is_err());
        assert!(provider.resolve("apps:firefox").is_none());
    }

    #[test]
    fn truncates_on_character_boundaries() {
        assert_eq!(truncate("héllo wörld", 5), "héllo\u{2026}");
        assert_eq!(truncate("short", 10), "short");
    }
}
