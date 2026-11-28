//! Letting the user select part of the screen, and getting a picture of it.
//!
//! Wayland does not let an app draw over the whole desktop on every compositor (GNOME has no
//! layer-shell), so the desktop's own area selection is used:
//!
//! | Backend | Where | How |
//! |---|---|---|
//! | `portal` | GNOME, KDE (Wayland and X11) | `org.freedesktop.portal.Screenshot`, interactive: the desktop's screenshot UI, where you drag an area |
//! | `grim` | Sway, Hyprland and other wlroots compositors | `slurp` to drag an area, `grim` to capture it |
//! | `maim` / `scrot` | X11 | `maim -s` / `scrot -s` |
//!
//! Every picture is deleted as soon as it is loaded: a screenshot can show anything.

use anyhow::{Context, Result, bail};
use image::DynamicImage;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Time allowed to drag an area (or answer the desktop's permission dialog).
const SELECT_TIMEOUT: Duration = Duration::from_secs(120);

/// The user closed the selection without choosing an area (Esc, right-click, Cancel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the selection was cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backend {
    /// Pick from the session: `grim` on wlroots, `maim`/`scrot` on X11, else the portal.
    #[default]
    Auto,
    Portal,
    Grim,
    Maim,
    Scrot,
}

impl Backend {
    pub const ALL: [Self; 5] = [
        Self::Auto,
        Self::Portal,
        Self::Grim,
        Self::Maim,
        Self::Scrot,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Portal => "portal",
            Self::Grim => "grim",
            Self::Maim => "maim",
            Self::Scrot => "scrot",
        }
    }
}

impl std::str::FromStr for Backend {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|backend| backend.as_str().eq_ignore_ascii_case(value.trim()))
            .ok_or_else(|| {
                format!("unknown capture backend `{value}` (auto, portal, grim, maim or scrot)")
            })
    }
}

/// The facts [`resolve`] decides from, gathered from the environment.
#[derive(Debug, Clone, Default)]
pub struct Session {
    pub wayland: bool,
    pub x11: bool,
    /// A wlroots compositor (Sway, Hyprland, river, Wayfire, labwc).
    pub wlroots: bool,
    /// Programs found on `PATH`.
    pub programs: Vec<&'static str>,
}

const TOOLS: [&str; 4] = ["grim", "slurp", "maim", "scrot"];

impl Session {
    pub fn detect() -> Self {
        let set = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_lowercase();
        let wlroots_desktop = ["sway", "hyprland", "river", "wayfire", "labwc"]
            .iter()
            .any(|name| desktop.split(':').any(|part| part == *name));

        Self {
            wayland: set("WAYLAND_DISPLAY"),
            x11: set("DISPLAY") && !set("WAYLAND_DISPLAY"),
            wlroots: set("SWAYSOCK") || set("HYPRLAND_INSTANCE_SIGNATURE") || wlroots_desktop,
            programs: TOOLS.into_iter().filter(|tool| on_path(tool)).collect(),
        }
    }

    fn has(&self, program: &str) -> bool {
        self.programs.contains(&program)
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
        .unwrap_or(false)
}

/// The backend to use for `requested` in `session`, or why none can work.
pub fn resolve(requested: Backend, session: &Session) -> Result<Backend> {
    let need = |programs: &[&str], hint: &str| -> Result<()> {
        match programs.iter().find(|program| !session.has(program)) {
            Some(missing) => bail!("`{missing}` is not installed ({hint})"),
            None => Ok(()),
        }
    };

    match requested {
        Backend::Auto => {
            if session.wayland && session.wlroots && session.has("grim") && session.has("slurp") {
                Ok(Backend::Grim)
            } else if session.wayland && session.wlroots {
                bail!(
                    "selecting an area on this compositor needs `grim` and `slurp` (install both, \
                     e.g. `sudo apt install grim slurp`)"
                )
            } else if session.x11 && session.has("maim") {
                Ok(Backend::Maim)
            } else if session.x11 && session.has("scrot") {
                Ok(Backend::Scrot)
            } else if session.wayland || session.x11 {
                Ok(Backend::Portal)
            } else {
                bail!("no graphical session found (neither WAYLAND_DISPLAY nor DISPLAY is set)")
            }
        }
        Backend::Grim => {
            need(&["grim", "slurp"], "install grim and slurp")?;
            Ok(Backend::Grim)
        }
        Backend::Maim => {
            need(&["maim"], "install maim")?;
            Ok(Backend::Maim)
        }
        Backend::Scrot => {
            need(&["scrot"], "install scrot")?;
            Ok(Backend::Scrot)
        }
        Backend::Portal => Ok(Backend::Portal),
    }
}

/// A picture file that is deleted when this goes out of scope, however the extraction ends.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A new, unused path in a private directory (`$XDG_RUNTIME_DIR` is owner-only by spec).
fn private_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());

    dir.join(format!("aujar-text-{}-{nanos}.png", std::process::id()))
}

fn load(path: &Path) -> Result<DynamicImage> {
    image::open(path).with_context(|| format!("could not read the screenshot {}", path.display()))
}

/// Run a selection tool. A non-zero exit is how all of them report Esc, so it is a cancel.
fn run_tool(program: &str, args: &[&str]) -> Result<Vec<u8>> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("could not run `{program}`"))?;
    let started = Instant::now();

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }

        if started.elapsed() > SELECT_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "no area was selected within {} seconds",
                SELECT_TIMEOUT.as_secs()
            );
        }

        std::thread::sleep(Duration::from_millis(30));
    };

    if !status.success() {
        return Err(Cancelled.into());
    }

    let mut out = Vec::new();

    if let Some(mut stdout) = child.stdout.take() {
        use std::io::Read;

        let _ = stdout.read_to_end(&mut out);
    }

    Ok(out)
}

/// `slurp` prints `x,y wxh`; accept exactly that, so nothing else reaches `grim`.
pub fn parse_geometry(output: &str) -> Option<String> {
    let geometry = output.trim();
    let (position, size) = geometry.split_once(' ')?;
    let (x, y) = position.split_once(',')?;
    let (w, h) = size.split_once('x')?;
    let number = |text: &str| {
        !text.is_empty()
            && text
                .trim_start_matches('-')
                .chars()
                .all(|c| c.is_ascii_digit())
    };

    (number(x) && number(y) && number(w) && number(h) && w != "0" && h != "0")
        .then(|| geometry.to_owned())
}

/// Let the user select an area and return a picture of it.
///
/// Blocking: call from a blocking thread. `requested` is resolved against the session first.
pub fn select_area(requested: Backend) -> Result<DynamicImage> {
    let backend = resolve(requested, &Session::detect())?;

    tracing::debug!(
        backend = backend.as_str(),
        "selecting a screen area for text extraction"
    );

    match backend {
        Backend::Grim => {
            let geometry = String::from_utf8_lossy(&run_tool("slurp", &[])?).into_owned();
            let geometry = parse_geometry(&geometry)
                .with_context(|| format!("unexpected answer from slurp: {}", geometry.trim()))?;
            let file = Removed(private_path());

            run_tool("grim", &["-g", &geometry, &file.0.to_string_lossy()])
                .context("grim could not capture the area")?;
            load(&file.0)
        }
        Backend::Maim => {
            let file = Removed(private_path());

            run_tool(
                "maim",
                &["--select", "--hidecursor", &file.0.to_string_lossy()],
            )?;
            load(&file.0)
        }
        Backend::Scrot => {
            let file = Removed(private_path());

            run_tool("scrot", &["--select", &file.0.to_string_lossy()])?;
            load(&file.0)
        }
        Backend::Portal => {
            let file = Removed(portal(true)?);

            load(&file.0)
        }
        Backend::Auto => unreachable!("resolve never returns Auto"),
    }
}

/// The backend for a picture of the whole screen: like [`resolve`], but no area is selected,
/// so `grim` needs no `slurp`.
pub fn resolve_full(requested: Backend, session: &Session) -> Result<Backend> {
    match requested {
        Backend::Auto if session.wayland && session.wlroots => {
            if session.has("grim") {
                Ok(Backend::Grim)
            } else {
                bail!(
                    "capturing the screen on this compositor needs `grim` (e.g. `sudo apt install grim`)"
                )
            }
        }
        Backend::Grim if !session.has("grim") => bail!("`grim` is not installed"),
        Backend::Grim => Ok(Backend::Grim),
        other => resolve(other, session),
    }
}

/// Capture the whole screen, without asking the user to select anything, into a new
/// owner-only file in the private runtime directory. The caller owns the file and must delete
/// it. Blocking.
///
/// This is what Aujar's own snipping overlay starts from: the frozen picture of the screen.
pub fn capture_screen(requested: Backend) -> Result<PathBuf> {
    let backend = resolve_full(requested, &Session::detect())?;
    let target = private_path();
    let path = target.to_string_lossy().into_owned();
    fn failed(tool: &'static str) -> impl Fn(anyhow::Error) -> anyhow::Error {
        move |error| {
            if error.downcast_ref::<Cancelled>().is_some() {
                anyhow::anyhow!("`{tool}` could not capture the screen")
            } else {
                error
            }
        }
    }

    tracing::debug!(backend = backend.as_str(), "capturing the whole screen");

    match backend {
        Backend::Grim => run_tool("grim", &[&path])
            .map(drop)
            .map_err(failed("grim"))?,
        Backend::Maim => run_tool("maim", &["--hidecursor", &path])
            .map(drop)
            .map_err(failed("maim"))?,
        Backend::Scrot => run_tool("scrot", &[&path])
            .map(drop)
            .map_err(failed("scrot"))?,
        Backend::Portal => {
            let saved = Removed(portal(false).map_err(|error| {
                if error.downcast_ref::<Cancelled>().is_some() {
                    anyhow::anyhow!(
                        "the desktop refused to take a screenshot. GNOME asks once for programs \
                         it does not know: allow Aujar, or run the UI as its service \
                         (`systemctl --user start aujar-ui`)"
                    )
                } else {
                    error
                }
            })?);

            move_file(&saved.0, &target)?;
        }
        Backend::Auto => unreachable!("resolve_full never returns Auto"),
    }

    let _ = restrict(&target);

    Ok(target)
}

/// Move a file, also across file systems (`~/Pictures` to the runtime tmpfs).
fn move_file(from: &Path, to: &Path) -> Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }

    std::fs::copy(from, to)
        .with_context(|| format!("could not move the screenshot from {}", from.display()))?;

    Ok(())
}

#[cfg(unix)]
fn restrict(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Ask the screenshot portal for a picture (with the desktop's own area selection when
/// `interactive`) and return the file the desktop saved. The caller deletes it.
#[cfg(target_os = "linux")]
fn portal(interactive: bool) -> Result<PathBuf> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to start a runtime for the screenshot portal")?;

    runtime.block_on(async {
        tokio::time::timeout(SELECT_TIMEOUT, portal_inner(interactive))
            .await
            .context("timed out waiting for the desktop's screenshot dialog")?
    })
}

#[cfg(target_os = "linux")]
async fn portal_inner(interactive: bool) -> Result<PathBuf> {
    use ashpd::desktop::screenshot::Screenshot;

    let request = Screenshot::request()
        .interactive(interactive)
        .modal(interactive)
        .send()
        .await
        .context("the screenshot portal is not available on this desktop")?;

    // The portal answers "cancelled" and "denied" the same way to the app.
    let Ok(shot) = request.response() else {
        return Err(Cancelled.into());
    };

    let uri = shot.uri().as_str().to_owned();

    // The desktop saved it (often in ~/Pictures); the caller removes it.
    file_uri_to_path(&uri).with_context(|| format!("unexpected screenshot location: {uri}"))
}

#[cfg(not(target_os = "linux"))]
fn portal(_interactive: bool) -> Result<PathBuf> {
    bail!("selecting a screen area is not implemented for this platform yet")
}

/// `file:///home/me/Pictures/Screenshot%20from%20now.png` to a path.
pub fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // Skip an optional host (`file://localhost/...`).
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;

            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(wayland: bool, x11: bool, wlroots: bool, programs: &[&'static str]) -> Session {
        Session {
            wayland,
            x11,
            wlroots,
            programs: programs.to_vec(),
        }
    }

    #[test]
    fn auto_picks_the_native_tool_for_each_session() {
        let resolved = |s: Session| resolve(Backend::Auto, &s).unwrap();

        assert_eq!(
            resolved(session(true, false, true, &["grim", "slurp"])),
            Backend::Grim
        );
        assert_eq!(
            resolved(session(true, false, false, &["grim", "slurp"])),
            Backend::Portal
        );
        assert_eq!(
            resolved(session(false, true, false, &["maim", "scrot"])),
            Backend::Maim
        );
        assert_eq!(
            resolved(session(false, true, false, &["scrot"])),
            Backend::Scrot
        );
        assert_eq!(resolved(session(false, true, false, &[])), Backend::Portal);
    }

    #[test]
    fn auto_explains_what_is_missing() {
        let wlroots = resolve(Backend::Auto, &session(true, false, true, &["grim"]))
            .unwrap_err()
            .to_string();
        assert!(wlroots.contains("slurp"), "{wlroots}");

        assert!(resolve(Backend::Auto, &session(false, false, false, &[])).is_err());
    }

    #[test]
    fn full_screen_capture_needs_no_area_selector() {
        assert_eq!(
            resolve_full(Backend::Auto, &session(true, false, true, &["grim"])).unwrap(),
            Backend::Grim
        );
        assert!(resolve_full(Backend::Auto, &session(true, false, true, &[])).is_err());
        assert_eq!(
            resolve_full(Backend::Auto, &session(true, false, false, &[])).unwrap(),
            Backend::Portal
        );
        assert_eq!(
            resolve_full(Backend::Auto, &session(false, true, false, &["maim"])).unwrap(),
            Backend::Maim
        );
    }

    #[test]
    fn explicit_backends_need_their_programs() {
        let none = session(true, false, false, &[]);

        assert!(resolve(Backend::Grim, &none).is_err());
        assert!(resolve(Backend::Maim, &none).is_err());
        assert_eq!(resolve(Backend::Portal, &none).unwrap(), Backend::Portal);
    }

    #[test]
    fn backends_parse_from_settings() {
        assert_eq!("Grim".parse::<Backend>().unwrap(), Backend::Grim);
        assert_eq!(" auto ".parse::<Backend>().unwrap(), Backend::Auto);
        assert!("flameshot".parse::<Backend>().is_err());
    }

    #[test]
    fn only_slurp_geometry_reaches_grim() {
        assert_eq!(
            parse_geometry("10,20 300x40\n").as_deref(),
            Some("10,20 300x40")
        );
        assert_eq!(
            parse_geometry("-1920,0 50x50").as_deref(),
            Some("-1920,0 50x50")
        );
        assert_eq!(parse_geometry("10,20 0x40"), None);
        assert_eq!(parse_geometry("10,20 300x40; rm -rf ~"), None);
        assert_eq!(parse_geometry("selection cancelled"), None);
    }

    #[test]
    fn decodes_file_uris() {
        assert_eq!(
            file_uri_to_path("file:///home/me/Pictures/Screenshot%20from%20now.png"),
            Some("/home/me/Pictures/Screenshot from now.png".into())
        );
        assert_eq!(
            file_uri_to_path("file://localhost/tmp/a.png"),
            Some("/tmp/a.png".into())
        );
        assert_eq!(file_uri_to_path("https://example.com/a.png"), None);
        assert_eq!(file_uri_to_path("file:///bad%zz"), None);
        assert_eq!(file_uri_to_path("file:///truncated%2"), None);
    }

    #[test]
    fn pictures_are_deleted_when_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        std::fs::write(&path, b"x").unwrap();

        drop(Removed(path.clone()));

        assert!(!path.exists());
    }
}
