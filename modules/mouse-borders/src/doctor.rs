//! A preflight check: is this machine ready to share its keyboard and mouse, and if not,
//! what exactly should the user do?
//!
//! Every check has a plain-language result and, when it fails, a concrete fix. Common checks
//! (configuration, key, network, port, folders) run everywhere; permission checks are
//! specific to the operating system.

use std::fmt::Write as _;

use aujar_config::MouseBorders;

use crate::{config::EngineConfig, fsutil, keystore::KeyStore, net};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    /// Works, but something is worth knowing or fixing.
    Warn,
    /// Mouse Borders cannot work until this is fixed.
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub level: Level,
    pub detail: String,
    pub fix: Option<String>,
}

impl Check {
    fn new(name: &str, level: Level, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_owned(),
            level,
            detail: detail.into(),
            fix: None,
        }
    }

    fn ok(name: &str, detail: impl Into<String>) -> Self {
        Self::new(name, Level::Ok, detail)
    }

    fn warn(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::new(name, Level::Warn, detail).with_fix(fix)
    }

    fn fail(name: &str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self::new(name, Level::Fail, detail).with_fix(fix)
    }

    fn with_fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = Some(fix.into());

        self
    }
}

/// Run every check that applies here.
pub fn run(settings: &MouseBorders, key_store: &KeyStore) -> Vec<Check> {
    let mut checks = common(settings, key_store);

    checks.extend(platform(settings));

    checks
}

/// The checks that do not depend on the operating system.
pub fn common(settings: &MouseBorders, key_store: &KeyStore) -> Vec<Check> {
    let mut checks = Vec::new();

    let config = EngineConfig::from_settings(settings);

    checks.push(match &config {
        Ok(config) => Check::ok(
            "Configuration",
            format!(
                "{} machines; this machine is `{}`",
                config.layout.len(),
                config.local
            ),
        ),
        Err(error) => Check::fail(
            "Configuration",
            error.to_string(),
            "fix [mouse_borders] in the settings file, or join your machines with a pairing code",
        ),
    });

    checks.push(if settings.enabled {
        Check::ok("Switched on", "Mouse Borders is enabled")
    } else {
        Check::warn(
            "Switched on",
            "Mouse Borders is disabled in the settings",
            "set `enabled = true` under [mouse_borders], or create a pairing code",
        )
    });

    checks.push(match key_store.load() {
        Ok(Some(_)) => Check::ok("Security key", "present and private to your user"),
        Ok(None) => Check::warn(
            "Security key",
            "there is no security key yet",
            "run `aujar-mwb pair` on one machine and `aujar-mwb join` on the others",
        ),
        Err(error) => Check::fail(
            "Security key",
            error.to_string(),
            "create a new key, or fix the key file's permissions",
        ),
    });

    let networks = net::local_networks();

    checks.push(if networks.is_empty() {
        Check::fail(
            "Network",
            "no network connection was found",
            "connect to the network the other machines are on",
        )
    } else {
        let list: Vec<String> = networks
            .iter()
            .map(|network| format!("{}/{}", network.ip, network.prefix))
            .collect();

        Check::ok("Network", list.join(", "))
    });

    checks.push(match std::net::TcpListener::bind(("0.0.0.0", settings.port)) {
        Ok(_) => Check::ok("Port", format!("{} is free", settings.port)),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => Check::warn(
            "Port",
            format!("{} is already in use", settings.port),
            "that is normal if the agent is running; otherwise another program uses it: change `port`",
        ),
        Err(error) => Check::fail(
            "Port",
            format!("cannot listen on {}: {error}", settings.port),
            "allow the program to listen on the network (firewall or permissions)",
        ),
    });

    if let Ok(config) = &config {
        checks.push(writable_folder("Receive folder", &config.receive_dir));

        if let Some(path) = &config.audit_log
            && let Some(parent) = path.parent()
        {
            checks.push(writable_folder("Audit log", parent));
        }
    }

    checks
}

fn writable_folder(name: &str, path: &std::path::Path) -> Check {
    let probe = path.join(".aujar-write-test");
    let result = fsutil::create_private_dir_all(path).and_then(|()| {
        std::fs::write(&probe, b"x")?;
        std::fs::remove_file(&probe)
    });

    match result {
        Ok(()) => Check::ok(name, path.display().to_string()),
        Err(error) => Check::warn(
            name,
            format!("{} is not writable ({error})", path.display()),
            "choose another folder in the settings, or fix its permissions",
        ),
    }
}

/// Plain-text report, one line per check with its fix underneath.
pub fn format(checks: &[Check]) -> String {
    let mut out = String::new();

    for check in checks {
        let mark = match check.level {
            Level::Ok => "[ ok ]",
            Level::Warn => "[warn]",
            Level::Fail => "[FAIL]",
        };

        let _ = writeln!(out, "{mark} {}: {}", check.name, check.detail);

        if let Some(fix) = &check.fix
            && check.level != Level::Ok
        {
            let _ = writeln!(out, "       -> {fix}");
        }
    }

    out
}

/// True when nothing prevents Mouse Borders from working.
pub fn healthy(checks: &[Check]) -> bool {
    checks.iter().all(|check| check.level != Level::Fail)
}

#[cfg(target_os = "linux")]
fn platform(settings: &MouseBorders) -> Vec<Check> {
    let mut checks = Vec::new();

    let readable = std::fs::read_dir("/dev/input")
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("event"))
                .filter(|entry| std::fs::File::open(entry.path()).is_ok())
                .count()
        })
        .ok();

    checks.push(match readable {
        Some(count) if count > 0 => Check::ok(
            "Capturing input",
            format!("{count} input devices can be read"),
        ),
        Some(_) => Check::fail(
            "Capturing input",
            "no permission to read /dev/input/event*",
            "add your user to the `input` group (sudo usermod -aG input $USER) and log in again",
        ),
        None => Check::fail(
            "Capturing input",
            "/dev/input is not available",
            "this needs a real Linux machine with input devices (not a container)",
        ),
    });

    checks.push(
        match std::fs::OpenOptions::new().write(true).open("/dev/uinput") {
            Ok(_) => Check::ok("Injecting input", "/dev/uinput is writable"),
            Err(error) => Check::fail(
                "Injecting input",
                format!("cannot open /dev/uinput ({error})"),
                "install the udev rule from packaging/udev, load the `uinput` module, add your user to `input`",
            ),
        },
    );

    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();

    checks.push(if session.eq_ignore_ascii_case("wayland") {
        Check::warn(
            "Switching at screen edges",
            "this is a Wayland session, which does not let programs read the pointer position",
            "use the keyboard shortcuts (Ctrl+Alt+1..4) to switch, or log in to an X11 session",
        )
    } else {
        Check::ok(
            "Switching at screen edges",
            format!("session type `{session}`"),
        )
    });

    if settings.share_clipboard {
        let wayland = session.eq_ignore_ascii_case("wayland");
        let tool = if wayland { "wl-paste" } else { "xclip" };
        let found = std::env::var_os("PATH")
            .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(tool).is_file()));

        checks.push(if found {
            Check::ok("Clipboard sharing", format!("`{tool}` is installed"))
        } else {
            Check::warn(
                "Clipboard sharing",
                format!("`{tool}` is not installed"),
                if wayland {
                    "install wl-clipboard"
                } else {
                    "install xclip"
                },
            )
        });
    }

    checks
}

#[cfg(target_os = "macos")]
fn platform(_settings: &MouseBorders) -> Vec<Check> {
    use crate::backend::macos::permissions;

    let fix = "System Settings → Privacy & Security → Accessibility and Input Monitoring: allow this program, then restart it";

    vec![
        if permissions::accessibility_trusted() {
            Check::ok("Accessibility permission", "granted")
        } else {
            Check::fail(
                "Accessibility permission",
                "not granted: input cannot be captured or controlled",
                fix,
            )
        },
        if permissions::input_monitoring_allowed() {
            Check::ok("Input Monitoring permission", "granted")
        } else {
            Check::fail(
                "Input Monitoring permission",
                "not granted: this Mac's keyboard and mouse cannot be shared",
                fix,
            )
        },
        if permissions::post_events_allowed() {
            Check::ok(
                "Controlling this Mac",
                "other machines may type and click here",
            )
        } else {
            Check::fail(
                "Controlling this Mac",
                "not allowed: other machines cannot control this one",
                fix,
            )
        },
    ]
}

#[cfg(windows)]
fn platform(_settings: &MouseBorders) -> Vec<Check> {
    use windows_sys::Win32::UI::Shell::IsUserAnAdmin;

    // SAFETY: a plain Win32 query with no arguments.
    let elevated = unsafe { IsUserAnAdmin() } != 0;

    vec![if elevated {
        Check::ok(
            "Privileges",
            "running as administrator: elevated windows can be shared too",
        )
    } else {
        Check::ok(
            "Privileges",
            "running as a standard user: elevated windows (Task Manager, installers) cannot be captured or controlled; run as administrator to include them",
        )
    }]
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn platform(_settings: &MouseBorders) -> Vec<Check> {
    vec![Check::fail(
        "Operating system",
        "this operating system has no Mouse Borders backend",
        "use Linux, macOS or Windows",
    )]
}

#[cfg(test)]
mod tests {
    use super::*;
    use aujar_config::Machine;

    fn settings(dir: &std::path::Path) -> MouseBorders {
        MouseBorders {
            enabled: true,
            machine_name: "desk".into(),
            port: 0,
            machines: vec![
                Machine {
                    name: "desk".into(),
                    slot: 0,
                    address: String::new(),
                },
                Machine {
                    name: "laptop".into(),
                    slot: 1,
                    address: String::new(),
                },
            ],
            receive_dir: dir.join("received").display().to_string(),
            ..MouseBorders::default()
        }
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("no `{name}` in {checks:?}"))
    }

    fn store(dir: &std::path::Path) -> KeyStore {
        KeyStore::new(dir.join("mouse-borders.key"))
    }

    #[test]
    fn a_fully_configured_machine_passes_the_common_checks() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());

        store.generate(false).unwrap();

        let checks = common(&settings(dir.path()), &store);

        assert_eq!(find(&checks, "Configuration").level, Level::Ok);
        assert_eq!(find(&checks, "Switched on").level, Level::Ok);
        assert_eq!(find(&checks, "Security key").level, Level::Ok);
        assert_eq!(find(&checks, "Receive folder").level, Level::Ok);
        assert!(find(&checks, "Configuration").detail.contains("2 machines"));
    }

    #[test]
    fn problems_come_with_a_fix() {
        let dir = tempfile::tempdir().unwrap();
        let mut broken = settings(dir.path());

        broken.enabled = false;
        broken.machine_name = "stranger".into();

        let checks = common(&broken, &store(dir.path()));

        let config = find(&checks, "Configuration");
        assert_eq!(config.level, Level::Fail);
        assert!(
            config.detail.contains("not in `mouse_borders.machines`"),
            "{}",
            config.detail
        );
        assert!(config.fix.is_some());

        assert_eq!(find(&checks, "Switched on").level, Level::Warn);

        let key = find(&checks, "Security key");
        assert_eq!(key.level, Level::Warn);
        assert!(key.fix.as_deref().unwrap().contains("aujar-mwb pair"));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_others_can_read_is_a_failure() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());

        store.generate(false).unwrap();
        std::fs::set_permissions(store.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

        let checks = common(&settings(dir.path()), &store);

        assert_eq!(find(&checks, "Security key").level, Level::Fail);
    }

    #[test]
    fn a_taken_port_is_noticed() {
        let dir = tempfile::tempdir().unwrap();
        let taken = std::net::TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let mut busy = settings(dir.path());

        busy.port = taken.local_addr().unwrap().port();

        let checks = common(&busy, &store(dir.path()));

        assert_eq!(find(&checks, "Port").level, Level::Warn);
        assert!(
            find(&checks, "Port")
                .fix
                .as_deref()
                .unwrap()
                .contains("agent")
        );
    }

    #[test]
    fn an_unwritable_receive_folder_is_flagged() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("a-file");

        std::fs::write(&blocker, "x").unwrap();

        let mut bad = settings(dir.path());

        bad.receive_dir = blocker.join("inside").display().to_string();

        let checks = common(&bad, &store(dir.path()));

        assert_eq!(find(&checks, "Receive folder").level, Level::Warn);
    }

    #[test]
    fn the_report_shows_fixes_only_for_problems_and_health_ignores_warnings() {
        let checks = vec![
            Check::ok("A", "fine"),
            Check::warn("B", "meh", "do this"),
            Check::fail("C", "broken", "do that"),
        ];
        let text = format(&checks);

        assert!(text.contains("[ ok ] A: fine"));
        assert!(text.contains("[warn] B: meh\n       -> do this"));
        assert!(text.contains("[FAIL] C: broken\n       -> do that"));
        assert!(!healthy(&checks));
        assert!(healthy(&checks[..2]));
    }
}
