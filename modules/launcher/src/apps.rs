use crate::{
    model::{LaunchSpec, Provider, ResultKind, SearchResult},
    ranking::score_lowered,
};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

/// Maximum directory nesting scanned below each applications directory.
const MAX_DEPTH: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopEntry {
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub exec: String,
    pub terminal: bool,
    /// The raw `Icon=` value: a theme icon name or an absolute path.
    pub icon: Option<String>,
    /// The `Categories=` list, in file order.
    pub categories: Vec<String>,
}

impl DesktopEntry {
    /// The group the Applications grid files this application under: the first recognised
    /// freedesktop category wins, so `Settings;System;` is "System" and `Utility;Science;` is
    /// "Utilities".
    pub fn group(&self) -> &'static str {
        for category in &self.categories {
            let group = match category.as_str() {
                "Network" | "WebBrowser" | "Email" | "Chat" | "InstantMessaging"
                | "FileTransfer" | "P2P" | "RemoteAccess" | "Telephony" | "VideoConference" => {
                    "Internet"
                }
                "Office" | "Finance" | "Calendar" | "ContactManagement" | "Spreadsheet"
                | "WordProcessor" | "Presentation" | "ProjectManagement" => "Productivity",
                "AudioVideo" | "Audio" | "Video" | "Player" | "Music" | "Recorder" | "Midi"
                | "Mixer" | "Sequencer" | "TV" => "Media",
                "Graphics" | "Photography" | "2DGraphics" | "3DGraphics" | "RasterGraphics"
                | "VectorGraphics" | "Scanning" | "Viewer" => "Graphics",
                "Development" | "IDE" | "Debugger" | "RevisionControl" | "GUIDesigner"
                | "WebDevelopment" | "Building" | "Profiling" | "Translation" => "Development",
                "Game" | "ArcadeGame" | "BoardGame" | "CardGame" | "LogicGame" | "RolePlaying"
                | "Simulation" | "SportsGame" | "StrategyGame" | "ActionGame" | "AdventureGame"
                | "BlocksGame" => "Games",
                "Education" | "Science" | "Math" | "Astronomy" | "Chemistry" | "Geography"
                | "History" | "Languages" | "Biology" | "Physics" | "Literature" | "Art"
                | "Electronics" => "Education & Science",
                "Settings" | "System" | "Monitor" | "PackageManager" | "Security"
                | "HardwareSettings" | "DesktopSettings" => "System",
                "Utility" | "Accessories" | "Calculator" | "TextEditor" | "Archiving"
                | "Compression" | "FileManager" | "TerminalEmulator" | "Clock"
                | "Documentation" | "Accessibility" | "TextTools" => "Utilities",
                _ => continue,
            };

            return group;
        }

        "Other"
    }
}

/// Parse a freedesktop `.desktop` file. Returns `None` for entries that
/// should not be shown (not an Application, `NoDisplay`, `Hidden`, or
/// missing `Name`/`Exec`).
pub fn parse_desktop_entry(id: &str, content: &str) -> Option<DesktopEntry> {
    let mut in_entry = false;
    let mut name = None;
    let mut comment = None;
    let mut exec = None;
    let mut kind = None;
    let mut hidden = false;
    let mut terminal = false;
    let mut icon = None;
    let mut categories = Vec::new();

    for raw in content.lines() {
        let line = raw.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_entry {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let (key, value) = (key.trim(), value.trim());

        match key {
            "Name" => name = Some(value.to_owned()),
            "Comment" => comment = Some(value.to_owned()),
            "Exec" => exec = Some(value.to_owned()),
            "Icon" => icon = Some(value.to_owned()).filter(|value| !value.is_empty()),
            "Type" => kind = Some(value.to_owned()),
            "Terminal" => terminal = value.eq_ignore_ascii_case("true"),
            "Categories" => {
                categories = value
                    .split(';')
                    .map(str::trim)
                    .filter(|category| !category.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            "NoDisplay" | "Hidden" if value.eq_ignore_ascii_case("true") => hidden = true,
            _ => {}
        }
    }

    if hidden || kind.as_deref() != Some("Application") {
        return None;
    }

    Some(DesktopEntry {
        id: id.to_owned(),
        name: name?,
        comment: comment.filter(|value: &String| !value.is_empty()),
        exec: exec?,
        terminal,
        icon,
        categories,
    })
}

/// Split a desktop-entry `Exec` value into arguments **without a shell**.
///
/// Handles double quotes and backslash escapes inside quotes, `%%`, and drops
/// field codes such as `%u`/`%F` (no files or URLs are passed).
pub fn parse_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut in_quotes = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            '\\' if in_quotes => {
                match chars.peek().copied() {
                    Some(next) if matches!(next, '"' | '`' | '$' | '\\') => {
                        current.push(next);
                        chars.next();
                    }
                    _ => current.push('\\'),
                }

                has_token = true;
            }
            '%' => match chars.peek().copied() {
                Some('%') => {
                    chars.next();
                    current.push('%');
                    has_token = true;
                }
                Some(code) if code.is_ascii_alphabetic() => {
                    chars.next();
                }
                _ => {
                    current.push('%');
                    has_token = true;
                }
            },
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            _ => {
                current.push(c);
                has_token = true;
            }
        }
    }

    if has_token {
        args.push(current);
    }

    args
}

/// XDG application directories, highest priority first.
pub fn default_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let home = std::env::var_os("HOME").map(PathBuf::from);

    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".local/share")));

    if let Some(data_home) = data_home {
        dirs.push(data_home.join("applications"));
    }

    let data_dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());

    for dir in data_dirs.split(':').filter(|dir| !dir.is_empty()) {
        dirs.push(Path::new(dir).join("applications"));
    }

    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    if let Some(home) = home {
        dirs.push(home.join(".local/share/flatpak/exports/share/applications"));
    }

    dirs
}

fn collect(
    dir: &Path,
    base: &Path,
    depth: usize,
    entries: &mut Vec<DesktopEntry>,
    seen: &mut HashSet<String>,
) {
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };

    for item in read_dir.flatten() {
        let path = item.path();

        if path.is_dir() {
            if depth < MAX_DEPTH {
                collect(&path, base, depth + 1, entries, seen);
            }

            continue;
        }

        if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
            continue;
        }

        let Ok(relative) = path.strip_prefix(base) else {
            continue;
        };

        // Per the desktop-entry spec, subdirectories become `-` in the id.
        let id = relative.to_string_lossy().replace('/', "-");

        // First directory wins, even if it is hidden (user overrides).
        if !seen.insert(id.clone()) {
            continue;
        }

        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };

        if let Some(entry) = parse_desktop_entry(&id, &content) {
            entries.push(entry);
        }
    }
}

/// Applications shown per search at most (the launcher shows far fewer; this bounds how many
/// icons are looked up on disk).
const MAX_RESULTS: usize = 16;

/// `kind:app` lists the installed applications A to Z (the launcher's Applications button).
/// The daemon never returns more than 200 results to one search.
const LIST_TRIGGER: &str = "kind:app";
const LIST_MAX: usize = 150;
/// How often the application directories are checked for changes.
const RECHECK: std::time::Duration = std::time::Duration::from_secs(10);

/// Lower-cased copies made once at scan time, so a search allocates nothing per application.
struct Lowered {
    name: String,
    comment: Option<String>,
}

struct Snapshot {
    entries: Vec<DesktopEntry>,
    lowered: Vec<Lowered>,
    /// Newest modification time over the application directories.
    stamp: Option<std::time::SystemTime>,
    checked: std::time::Instant,
}

fn scan_dirs(dirs: &[PathBuf]) -> Snapshot {
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    for dir in dirs {
        collect(dir, dir, 0, &mut entries, &mut seen);
    }

    let lowered = entries
        .iter()
        .map(|entry| Lowered {
            name: entry.name.to_lowercase(),
            comment: entry.comment.as_deref().map(str::to_lowercase),
        })
        .collect();

    Snapshot {
        entries,
        lowered,
        stamp: dirs_stamp(dirs),
        checked: std::time::Instant::now(),
    }
}

/// Installing or removing an application changes the mtime of its applications directory.
fn dirs_stamp(dirs: &[PathBuf]) -> Option<std::time::SystemTime> {
    dirs.iter()
        .filter_map(|dir| fs::metadata(dir).and_then(|meta| meta.modified()).ok())
        .max()
}

/// Terminal emulators and the argument that precedes the command to run, in preference order.
const TERMINALS: [(&str, &[&str]); 11] = [
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--"]),
    ("kgx", &["--"]),
    ("konsole", &["-e"]),
    ("xfce4-terminal", &["-x"]),
    ("alacritty", &["-e"]),
    ("kitty", &[]),
    ("foot", &[]),
    ("wezterm", &["start", "--"]),
    ("tilix", &["-e"]),
    ("xterm", &["-e"]),
];

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// Wrap `program args` so it runs inside a terminal emulator.
fn in_terminal(program: String, args: Vec<String>) -> Option<(String, Vec<String>)> {
    let preferred = std::env::var("TERMINAL").ok().filter(|name| {
        // Only a bare program name: this value ends up as the command that is executed.
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    });

    let (terminal, flags): (String, Vec<&str>) = match preferred.filter(|name| on_path(name)) {
        Some(name) => {
            let flags = TERMINALS
                .iter()
                .find(|(known, _)| *known == name)
                .map_or(vec!["-e"], |(_, flags)| flags.to_vec());

            (name, flags)
        }
        None => {
            let (name, flags) = TERMINALS.iter().find(|(name, _)| on_path(name))?;

            ((*name).to_owned(), flags.to_vec())
        }
    };

    let mut wrapped: Vec<String> = flags.into_iter().map(str::to_owned).collect();

    wrapped.push(program);
    wrapped.extend(args);

    Some((terminal, wrapped))
}

/// Searches installed desktop applications.
pub struct AppsProvider {
    dirs: Vec<PathBuf>,
    snapshot: std::sync::RwLock<Snapshot>,
    /// Icon name -> resolved file, filled lazily (looking up icons touches the disk).
    icons: std::sync::Mutex<std::collections::HashMap<String, Option<String>>>,
}

impl AppsProvider {
    pub fn from_dirs(dirs: &[PathBuf]) -> Self {
        Self {
            dirs: dirs.to_vec(),
            snapshot: std::sync::RwLock::new(scan_dirs(dirs)),
            icons: Default::default(),
        }
    }

    fn icon_for(&self, entry: &DesktopEntry) -> Option<String> {
        let name = entry.icon.as_deref()?;
        let mut cache = self
            .icons
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        cache
            .entry(name.to_owned())
            .or_insert_with(|| crate::icons::resolve_icon(name))
            .clone()
    }

    pub fn from_env() -> Self {
        Self::from_dirs(&default_dirs())
    }

    pub fn len(&self) -> usize {
        self.read().entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Snapshot> {
        self.snapshot
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Pick up applications installed or removed since the last scan (at most every
    /// [`RECHECK`], and only a handful of `stat` calls unless something changed).
    fn refresh_if_changed(&self) {
        {
            let snapshot = self.read();

            if snapshot.checked.elapsed() < RECHECK {
                return;
            }
        }

        let stamp = dirs_stamp(&self.dirs);
        let mut snapshot = self
            .snapshot
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        // Another search may have refreshed while we waited for the write lock.
        if snapshot.checked.elapsed() < RECHECK {
            return;
        }

        if stamp == snapshot.stamp {
            snapshot.checked = std::time::Instant::now();
        } else {
            *snapshot = scan_dirs(&self.dirs);
        }
    }
}

impl Provider for AppsProvider {
    fn id(&self) -> &'static str {
        "apps"
    }

    fn search(&self, query: &str) -> Vec<SearchResult> {
        let query = query.trim().to_lowercase();

        if query.is_empty() {
            return Vec::new();
        }

        self.refresh_if_changed();

        let snapshot = self.read();

        // `kind:app` lists everything; `kind:app fire` searches only applications (what the
        // Applications button does once you type). `kind:apple` is an ordinary query.
        let (scoped, query) = match query.strip_prefix(LIST_TRIGGER) {
            Some("") => (true, String::new()),
            Some(rest) if rest.starts_with(' ') => (true, rest.trim().to_owned()),
            _ => (false, query.clone()),
        };

        if scoped && query.is_empty() {
            let mut all: Vec<&DesktopEntry> = snapshot.entries.iter().collect();

            all.sort_by_key(|entry| entry.name.to_lowercase());
            all.truncate(LIST_MAX);

            // Scores count down so the merged, score-sorted result list stays A to Z, and all
            // stay below the "top hit" threshold. The group tells the UI which chip to file the
            // application under.
            return all
                .into_iter()
                .enumerate()
                .map(|(position, entry)| {
                    SearchResult::new(
                        format!("apps:{}", entry.id),
                        &entry.name,
                        ResultKind::Application,
                        500 - u32::try_from(position).unwrap_or(0).min(400),
                    )
                    .with_icon(self.icon_for(entry))
                    .with_optional_subtitle(entry.comment.clone())
                    .with_detail(format!("group:{}", entry.group()))
                })
                .collect();
        }

        // Rank everything first; only the winners get an icon lookup (a disk probe per icon).
        let mut ranked: Vec<(u32, usize)> = snapshot
            .lowered
            .iter()
            .enumerate()
            .filter_map(|(index, lowered)| {
                let name_score = score_lowered(&query, &lowered.name, true);
                let comment_score = lowered
                    .comment
                    .as_deref()
                    .and_then(|comment| score_lowered(&query, comment, false))
                    .map(|value| value / 2);

                name_score
                    .into_iter()
                    .chain(comment_score)
                    .max()
                    .map(|best| (best, index))
            })
            .collect();

        ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        ranked.truncate(if scoped { LIST_MAX } else { MAX_RESULTS });

        ranked
            .into_iter()
            .map(|(best, index)| {
                let entry = &snapshot.entries[index];

                SearchResult::new(
                    format!("apps:{}", entry.id),
                    &entry.name,
                    ResultKind::Application,
                    best,
                )
                .with_icon(self.icon_for(entry))
                .with_optional_subtitle(entry.comment.clone())
                .with_detail(format!("group:{}", entry.group()))
            })
            .collect()
    }

    fn resolve(&self, id: &str) -> Option<Result<LaunchSpec, String>> {
        let key = id.strip_prefix("apps:")?;
        let snapshot = self.read();
        let entry = snapshot.entries.iter().find(|entry| entry.id == key)?;

        let mut parts = parse_exec(&entry.exec).into_iter();

        let Some(program) = parts.next() else {
            return Some(Err(format!("`{}` has an empty Exec line", entry.name)));
        };

        let args: Vec<String> = parts.collect();

        if entry.terminal {
            return Some(match in_terminal(program, args) {
                Some((program, args)) => Ok(LaunchSpec {
                    title: entry.name.clone(),
                    program,
                    args,
                }),
                None => Err(format!(
                    "`{}` needs a terminal, and no terminal emulator was found (set $TERMINAL)",
                    entry.name
                )),
            });
        }

        Some(Ok(LaunchSpec {
            title: entry.name.clone(),
            program,
            args,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Type=Application
Name=Firefox
Name[ne]=फायरफक्स
Comment=Browse the web
Exec=firefox %u

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window
";

    #[test]
    fn parses_visible_application() {
        let entry = parse_desktop_entry("firefox.desktop", FIREFOX).unwrap();

        assert_eq!(entry.name, "Firefox");
        assert_eq!(entry.comment.as_deref(), Some("Browse the web"));
        assert_eq!(entry.exec, "firefox %u");
    }

    #[test]
    fn skips_hidden_and_non_applications() {
        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        let link = "[Desktop Entry]\nType=Link\nName=X\nExec=x\n";
        let no_exec = "[Desktop Entry]\nType=Application\nName=X\n";

        assert!(parse_desktop_entry("a", hidden).is_none());
        assert!(parse_desktop_entry("b", link).is_none());
        assert!(parse_desktop_entry("c", no_exec).is_none());
    }

    #[test]
    fn scans_directory_and_ranks() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-apps-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("firefox.desktop"), FIREFOX).unwrap();
        fs::write(
            dir.join("files.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=files\n",
        )
        .unwrap();

        let provider = AppsProvider::from_dirs(std::slice::from_ref(&dir));
        assert_eq!(provider.len(), 2);

        let results = provider.search("fire");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "apps:firefox.desktop");

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn categories_are_parsed_and_grouped_by_the_first_known_one() {
        let entry = |categories: &str| {
            parse_desktop_entry(
                "x.desktop",
                &format!(
                    "[Desktop Entry]\nType=Application\nName=X\nExec=x\nCategories={categories}\n"
                ),
            )
            .unwrap()
        };

        assert_eq!(
            entry("Network;WebBrowser;").categories,
            ["Network", "WebBrowser"]
        );
        assert_eq!(entry("Network;WebBrowser;").group(), "Internet");
        assert_eq!(
            entry("GTK;Settings;X-GNOME-NetworkSettings;").group(),
            "System"
        );
        assert_eq!(entry("Utility;Calculator;Science;").group(), "Utilities");
        assert_eq!(entry("Development;IDE;").group(), "Development");
        assert_eq!(entry("AudioVideo;Player;").group(), "Media");
        assert_eq!(entry("Game;ArcadeGame;").group(), "Games");
        assert_eq!(entry("X-Unknown;").group(), "Other");
        assert_eq!(entry("").group(), "Other");
    }

    #[test]
    fn kind_app_lists_every_application_a_to_z() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-apps-list-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("firefox.desktop"), FIREFOX).unwrap();
        fs::write(
            dir.join("files.desktop"),
            "[Desktop Entry]\nType=Application\nName=Files\nExec=files\n",
        )
        .unwrap();

        let provider = AppsProvider::from_dirs(std::slice::from_ref(&dir));
        let titles: Vec<String> = provider
            .search("  KIND:APP ")
            .into_iter()
            .map(|result| result.title)
            .collect();

        assert_eq!(titles.len(), 2);
        assert!(titles[0].to_lowercase() <= titles[1].to_lowercase());

        let listed = provider.search("kind:app");

        // With text, only applications that match it, still carrying their group.
        let matching = provider.search("kind:app fire");

        assert_eq!(matching.len(), 1);
        assert!(matching[0].title.to_lowercase().contains("fire"));
        assert!(
            matching[0]
                .detail
                .as_deref()
                .is_some_and(|d| d.starts_with("group:"))
        );
        assert!(provider.search("kind:apple").is_empty());

        assert!(listed[0].score > listed[1].score);
        assert!(listed.iter().all(|result| result.score < 600));
        assert!(listed.iter().all(|result| {
            result
                .detail
                .as_deref()
                .is_some_and(|d| d.starts_with("group:"))
        }));

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn exec_parsing_has_no_shell_semantics() {
        assert_eq!(parse_exec("firefox %u"), ["firefox"]);
        assert_eq!(
            parse_exec(r#"env FOO=1 "my app" --file=%f"#),
            ["env", "FOO=1", "my app", "--file="]
        );
        assert_eq!(
            parse_exec(r#"sh -c "echo \"hi\"""#),
            ["sh", "-c", r#"echo "hi""#]
        );
        assert_eq!(parse_exec("app 100%%"), ["app", "100%"]);
        // Shell metacharacters are plain text, never interpreted.
        assert_eq!(parse_exec("app ; rm -rf ~"), ["app", ";", "rm", "-rf", "~"]);
    }

    #[test]
    fn resolves_only_indexed_applications() {
        let dir = std::env::temp_dir().join(format!(
            "aujar-resolve-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("firefox.desktop"), FIREFOX).unwrap();
        fs::write(
            dir.join("htop.desktop"),
            "[Desktop Entry]\nType=Application\nName=Htop\nExec=htop\nTerminal=true\n",
        )
        .unwrap();

        let provider = AppsProvider::from_dirs(std::slice::from_ref(&dir));

        let spec = provider.resolve("apps:firefox.desktop").unwrap().unwrap();
        assert_eq!(spec.program, "firefox");
        assert!(spec.args.is_empty());

        // A terminal app is wrapped in a terminal emulator, or refused when none is installed.
        match provider.resolve("apps:htop.desktop").unwrap() {
            Ok(spec) => assert!(spec.args.contains(&"htop".to_owned())),
            Err(message) => assert!(message.contains("terminal")),
        }
        assert!(provider.resolve("apps:missing.desktop").is_none());
        assert!(provider.resolve("calc:4").is_none());

        fs::remove_dir_all(dir).unwrap();
    }
}
