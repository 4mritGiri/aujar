//! A report to paste when something does not work.
//!
//! One command gathers what is otherwise guessed from symptoms: the desktop session, whether
//! the desktop's input-capture portal is there, whether the GNOME extension that hides the
//! pointer answers, the checks of `doctor`, the running status and the latest log lines.
//!
//! Everything that is *decided* here (reading version strings, judging what was found,
//! laying out the report) is plain functions of the collected [`Facts`], so it is tested without
//! a desktop. Only [`collect`] touches the system, and it never fails: what cannot be found is
//! reported as not found.

use std::{fmt::Write as _, process::Command};

use aujar_config::MouseBorders;

use crate::doctor::{Check, Level};

/// The GNOME extension that can hide the pointer (shipped with Aujar).
const EXTENSION_UUID: &str = "aujar-windows@aujar.dev";

/// The first GNOME version with the input-capture portal.
const GNOME_WITH_INPUT_CAPTURE: u32 = 45;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PointerExtension {
    /// It answered on the session bus.
    Responding,
    /// The files are there, but the shell has not loaded it (its state, if known).
    InstalledNotLoaded(Option<String>),
    NotInstalled,
}

/// Everything found out about this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facts {
    pub version: String,
    /// `XDG_SESSION_TYPE`.
    pub session_type: Option<String>,
    /// `WAYLAND_DISPLAY` is set.
    pub wayland_display: bool,
    /// `XDG_CURRENT_DESKTOP`.
    pub desktop: Option<String>,
    /// The output of `gnome-shell --version`.
    pub gnome_shell: Option<String>,
    /// The portal's version, or why it could not be asked. `None`: not asked (not Wayland).
    pub input_capture_portal: Option<Result<u32, String>>,
    /// `None`: not asked (pointer hiding is off).
    pub pointer_extension: Option<PointerExtension>,
    pub edge_portal_enabled: bool,
    pub hide_pointer_enabled: bool,
    pub kernel: Option<String>,
    /// The latest lines of the daemon's log, or why they could not be read.
    pub log_tail: Option<Result<String, String>>,
}

impl Facts {
    fn is_wayland(&self) -> bool {
        self.session_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("wayland"))
            || (self.session_type.is_none() && self.wayland_display)
    }

    fn is_gnome(&self) -> bool {
        self.desktop
            .as_deref()
            .is_some_and(|desktop| desktop.to_ascii_lowercase().contains("gnome"))
    }
}

// ---- Reading what commands print --------------------------------------------------------

/// `GNOME Shell 46.0` is `(46, 0)`; `GNOME Shell 47.rc` is `(47, 0)`.
pub fn parse_gnome_shell_version(text: &str) -> Option<(u32, u32)> {
    let version = text
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|c| c.is_ascii_digit()))?;
    let mut parts = version.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts
        .next()
        .and_then(|part| {
            part.chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .ok()
        })
        .unwrap_or(0);

    Some((major, minor))
}

/// `gdbus` prints a property as `(<uint32 1>,)`.
pub fn parse_portal_version(text: &str) -> Option<u32> {
    let after = text.split("uint32").nth(1)?;
    let digits: String = after
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();

    digits.parse().ok()
}

/// `gdbus call ... SetHidden false` answers `(true,)` when the extension did it.
pub fn parse_extension_reply(text: &str) -> bool {
    text.trim() == "(true,)"
}

/// The `State:` line of `gnome-extensions info`, e.g. `ENABLED` or `ERROR`.
pub fn parse_extension_state(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("State:")
            .map(|state| state.trim().to_owned())
            .filter(|state| !state.is_empty())
    })
}

// ---- Judging what was found --------------------------------------------------------------

/// The extra checks for screen edges and the pointer (the ones `doctor` does not make).
pub fn checks(facts: &Facts) -> Vec<Check> {
    let mut checks = Vec::new();

    checks.push(match (&facts.session_type, facts.is_wayland()) {
        (_, true) => Check {
            name: "Session".into(),
            level: Level::Ok,
            detail: "Wayland: the pointer position cannot be read, so screen edges need the \
                     desktop's input-capture portal"
                .into(),
            fix: None,
        },
        (Some(kind), false) => Check {
            name: "Session".into(),
            level: Level::Ok,
            detail: format!("{kind}: screen edges are read directly"),
            fix: None,
        },
        (None, false) => Check {
            name: "Session".into(),
            level: Level::Warn,
            detail: "the session type is unknown (XDG_SESSION_TYPE is not set)".into(),
            fix: Some("run this from a terminal inside your desktop session, not over ssh".into()),
        },
    });

    if facts.is_wayland() {
        checks.push(portal_check(facts));
    }

    checks.push(pointer_check(facts));

    checks
}

fn portal_check(facts: &Facts) -> Check {
    let name = "Screen edges (Wayland)";
    let gnome = facts
        .gnome_shell
        .as_deref()
        .and_then(parse_gnome_shell_version);

    if !facts.edge_portal_enabled {
        return Check {
            name: name.into(),
            level: Level::Warn,
            detail: "turned off: `edge_portal = false` in the [mouse_borders] settings".into(),
            fix: Some(
                "set it to true (or remove it) to switch machines by moving the mouse; the \
                 Ctrl+Alt+number shortcuts work either way"
                    .into(),
            ),
        };
    }

    match &facts.input_capture_portal {
        Some(Ok(version)) => Check {
            name: name.into(),
            level: Level::Ok,
            detail: format!("the input-capture portal is there (version {version})"),
            fix: None,
        },
        Some(Err(reason)) => {
            let too_old = gnome.is_some_and(|(major, _)| major < GNOME_WITH_INPUT_CAPTURE);

            Check {
                name: name.into(),
                level: Level::Fail,
                detail: format!("no input-capture portal: {reason}"),
                fix: Some(if too_old {
                    format!(
                        "GNOME {} is too old (it needs {GNOME_WITH_INPUT_CAPTURE} or later): \
                         use the Ctrl+Alt+number shortcuts",
                        gnome.map_or(0, |(major, _)| major)
                    )
                } else {
                    "install xdg-desktop-portal and your desktop's portal (xdg-desktop-portal-gnome \
                     or -kde), then log out and back in; the Ctrl+Alt+number shortcuts work \
                     meanwhile"
                        .into()
                }),
            }
        }
        None => Check {
            name: name.into(),
            level: Level::Warn,
            detail: "the portal was not asked".into(),
            fix: None,
        },
    }
}

fn pointer_check(facts: &Facts) -> Check {
    let name = "Hiding the pointer";

    if !facts.hide_pointer_enabled {
        return Check {
            name: name.into(),
            level: Level::Ok,
            detail: "off (Settings, Mouse Borders, \"Hide the pointer while away\")".into(),
            fix: None,
        };
    }

    if !facts.is_wayland() {
        return Check {
            name: name.into(),
            level: Level::Ok,
            detail: "on, done directly by the desktop session".into(),
            fix: None,
        };
    }

    if !facts.is_gnome() {
        return Check {
            name: name.into(),
            level: Level::Warn,
            detail: "on, but only GNOME can hide the pointer on Wayland".into(),
            fix: Some("turn it off, or use an X11 session".into()),
        };
    }

    match &facts.pointer_extension {
        Some(PointerExtension::Responding) => Check {
            name: name.into(),
            level: Level::Ok,
            detail: "on, and the Aujar Windows extension answers".into(),
            fix: None,
        },
        Some(PointerExtension::InstalledNotLoaded(state)) => Check {
            name: name.into(),
            level: Level::Warn,
            detail: format!(
                "on, but the extension is not running (state: {})",
                state.as_deref().unwrap_or("unknown")
            ),
            fix: Some(
                "run `aujar gnome-extension install` again, then log out and back in (Wayland \
                 loads new extension code only at login)"
                    .into(),
            ),
        },
        Some(PointerExtension::NotInstalled) | None => Check {
            name: name.into(),
            level: Level::Warn,
            detail: "on, but the Aujar Windows extension is not installed".into(),
            fix: Some("run `aujar gnome-extension install`, then log out and back in".into()),
        },
    }
}

// ---- Laying out the report ---------------------------------------------------------------

/// The whole report, as text to paste. `doctor` is the output of the preflight checks;
/// `status` is the running service's status (`Err` with why not, when it could not be asked).
pub fn format_report(
    facts: &Facts,
    doctor_checks: &[Check],
    status: &Result<String, String>,
) -> String {
    let mut out = String::new();

    let _ = writeln!(
        out,
        "Aujar Mouse Borders report (version {})",
        facts.version
    );
    let _ = writeln!(
        out,
        "Paste this when asking for help. It shows machine names and network addresses from \
         your own setup; it does not contain your security key.\n"
    );

    let _ = writeln!(out, "== This machine ==");
    let _ = writeln!(
        out,
        "Session: {}{}",
        facts.session_type.as_deref().unwrap_or("unknown"),
        if facts.wayland_display {
            " (WAYLAND_DISPLAY is set)"
        } else {
            ""
        }
    );
    let _ = writeln!(
        out,
        "Desktop: {}",
        facts.desktop.as_deref().unwrap_or("unknown")
    );

    if let Some(shell) = &facts.gnome_shell {
        let _ = writeln!(out, "GNOME Shell: {}", shell.trim());
    }

    if let Some(kernel) = &facts.kernel {
        let _ = writeln!(out, "Kernel: {}", kernel.trim());
    }

    let _ = writeln!(
        out,
        "Settings: edge_portal = {}, hide_mouse_at_edge = {}",
        facts.edge_portal_enabled, facts.hide_pointer_enabled
    );

    let _ = writeln!(out, "\n== Checks ==");
    out.push_str(&crate::doctor::format(doctor_checks));
    out.push_str(&crate::doctor::format(&checks(facts)));

    let _ = writeln!(out, "\n== Status ==");

    match status {
        Ok(text) => out.push_str(text),
        Err(reason) => {
            let _ = writeln!(out, "not available: {reason}");
        }
    }

    let _ = writeln!(out, "\n== Recent log ==");

    match &facts.log_tail {
        Some(Ok(text)) if !text.trim().is_empty() => {
            out.push_str(text.trim_end());
            out.push('\n');
        }
        Some(Ok(_)) => {
            let _ = writeln!(out, "(the log is empty)");
        }
        Some(Err(reason)) => {
            let _ = writeln!(out, "not available: {reason}");
        }
        None => {
            let _ = writeln!(out, "not collected");
        }
    }

    out
}

// ---- Looking at the system ---------------------------------------------------------------

/// Run a program with a time limit (the `timeout` tool, so a stuck desktop cannot hang the
/// report). `Ok` is what it printed; `Err` says why there is nothing.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new("timeout")
        .arg("4")
        .arg(program)
        .args(args)
        .output()
        .map_err(|error| format!("cannot run `{program}`: {error}"))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else if output.status.code() == Some(124) {
        Err(format!("`{program}` did not answer in time"))
    } else if output.status.code() == Some(127) {
        Err(format!("`{program}` is not installed"))
    } else {
        let message = String::from_utf8_lossy(&output.stderr);
        let message = message.trim();

        Err(if message.is_empty() {
            format!("`{program}` failed")
        } else {
            message.lines().next().unwrap_or(message).to_owned()
        })
    }
}

fn probe_portal() -> Result<u32, String> {
    let text = run(
        "gdbus",
        &[
            "call",
            "--session",
            "--timeout",
            "2",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.DBus.Properties.Get",
            "org.freedesktop.portal.InputCapture",
            "version",
        ],
    )
    .map_err(|reason| {
        if reason.contains("No such interface") || reason.contains("UnknownInterface") {
            "this desktop does not offer it".to_owned()
        } else {
            reason
        }
    })?;

    parse_portal_version(&text).ok_or_else(|| format!("unexpected answer: {}", text.trim()))
}

fn probe_extension() -> PointerExtension {
    let answered = run(
        "gdbus",
        &[
            "call",
            "--session",
            "--timeout",
            "2",
            "--dest",
            "dev.aujar.Windows",
            "--object-path",
            "/dev/aujar/Pointer",
            "--method",
            "dev.aujar.Pointer.SetHidden",
            "false",
        ],
    )
    .is_ok_and(|text| parse_extension_reply(&text));

    if answered {
        return PointerExtension::Responding;
    }

    let info = run("gnome-extensions", &["info", EXTENSION_UUID]);

    match info {
        Ok(text) => PointerExtension::InstalledNotLoaded(parse_extension_state(&text)),
        Err(_) => PointerExtension::NotInstalled,
    }
}

/// Look at this machine. Never fails; what is missing is `None` or an `Err` inside.
pub fn collect(settings: &MouseBorders) -> Facts {
    let mut facts = Facts {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        session_type: std::env::var("XDG_SESSION_TYPE")
            .ok()
            .filter(|s| !s.is_empty()),
        wayland_display: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        desktop: std::env::var("XDG_CURRENT_DESKTOP")
            .ok()
            .filter(|s| !s.is_empty()),
        edge_portal_enabled: settings.edge_portal,
        hide_pointer_enabled: settings.hide_mouse_at_edge,
        ..Facts::default()
    };

    if cfg!(target_os = "linux") {
        facts.kernel = run("uname", &["-sr"]).ok();

        if facts.is_gnome() {
            facts.gnome_shell = run("gnome-shell", &["--version"]).ok();
        }

        if facts.is_wayland() {
            facts.input_capture_portal = Some(probe_portal());
        }

        if facts.hide_pointer_enabled && facts.is_wayland() && facts.is_gnome() {
            facts.pointer_extension = Some(probe_extension());
        }

        facts.log_tail = Some(run(
            "journalctl",
            &[
                "--user",
                "-u",
                "aujar",
                "-n",
                "60",
                "--no-pager",
                "-o",
                "short-iso",
            ],
        ));
    }

    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wayland_gnome() -> Facts {
        Facts {
            version: "0.1.1".into(),
            session_type: Some("wayland".into()),
            wayland_display: true,
            desktop: Some("ubuntu:GNOME".into()),
            gnome_shell: Some("GNOME Shell 46.0".into()),
            input_capture_portal: Some(Ok(1)),
            pointer_extension: None,
            edge_portal_enabled: true,
            hide_pointer_enabled: false,
            kernel: Some("Linux 6.8.0".into()),
            log_tail: Some(Ok("Nov 05 aujar[1]: ready\n".into())),
        }
    }

    fn find<'a>(checks: &'a [Check], name: &str) -> &'a Check {
        checks
            .iter()
            .find(|check| check.name.starts_with(name))
            .unwrap_or_else(|| panic!("no check named {name}: {checks:?}"))
    }

    #[test]
    fn versions_are_read_from_what_the_tools_print() {
        assert_eq!(
            parse_gnome_shell_version("GNOME Shell 46.0\n"),
            Some((46, 0))
        );
        assert_eq!(
            parse_gnome_shell_version("GNOME Shell 47.rc"),
            Some((47, 0))
        );
        assert_eq!(
            parse_gnome_shell_version("GNOME Shell 44.9.1"),
            Some((44, 9))
        );
        assert_eq!(parse_gnome_shell_version("GNOME Shell"), None);
        assert_eq!(parse_gnome_shell_version(""), None);

        assert_eq!(parse_portal_version("(<uint32 1>,)\n"), Some(1));
        assert_eq!(parse_portal_version("(<uint32 12>,)"), Some(12));
        assert_eq!(parse_portal_version("(<'x'>,)"), None);
        assert_eq!(parse_portal_version(""), None);

        assert!(parse_extension_reply("(true,)\n"));
        assert!(!parse_extension_reply("(false,)"));
        assert!(!parse_extension_reply(""));

        assert_eq!(
            parse_extension_state("aujar-windows@aujar.dev\n  Name: x\n  State: ERROR\n"),
            Some("ERROR".into())
        );
        assert_eq!(parse_extension_state("no state here"), None);
        assert_eq!(parse_extension_state("State:\n"), None);
    }

    #[test]
    fn a_healthy_wayland_gnome_machine_has_nothing_to_fix() {
        let checks = checks(&wayland_gnome());

        assert!(
            checks.iter().all(|check| check.level == Level::Ok),
            "{checks:?}"
        );
        assert!(find(&checks, "Screen edges").detail.contains("version 1"));
    }

    #[test]
    fn x11_reads_the_edges_directly_and_does_not_ask_the_portal() {
        let mut facts = wayland_gnome();

        facts.session_type = Some("x11".into());
        facts.wayland_display = false;
        facts.input_capture_portal = None;

        let checks = checks(&facts);

        assert!(find(&checks, "Session").detail.contains("read directly"));
        assert!(
            checks
                .iter()
                .all(|check| !check.name.starts_with("Screen edges"))
        );
    }

    #[test]
    fn a_missing_portal_says_what_to_install_or_that_gnome_is_too_old() {
        let mut facts = wayland_gnome();

        facts.input_capture_portal = Some(Err("this desktop does not offer it".into()));

        let missing = checks(&facts);
        let check = find(&missing, "Screen edges");

        assert_eq!(check.level, Level::Fail);
        assert!(check.fix.as_deref().unwrap().contains("xdg-desktop-portal"));

        facts.gnome_shell = Some("GNOME Shell 44.2".into());

        let old = checks_for(&facts);

        assert!(old.fix.as_deref().unwrap().contains("GNOME 44 is too old"));
        assert!(old.fix.as_deref().unwrap().contains("Ctrl+Alt+number"));
    }

    fn checks_for(facts: &Facts) -> Check {
        find(&checks(facts), "Screen edges").clone()
    }

    #[test]
    fn turning_the_portal_off_is_noticed_before_blaming_the_desktop() {
        let mut facts = wayland_gnome();

        facts.edge_portal_enabled = false;
        facts.input_capture_portal = Some(Err("whatever".into()));

        let check = checks_for(&facts);

        assert_eq!(check.level, Level::Warn);
        assert!(check.detail.contains("edge_portal = false"));
    }

    #[test]
    fn hiding_the_pointer_checks_the_extension_only_where_it_matters() {
        let mut facts = wayland_gnome();

        facts.hide_pointer_enabled = true;

        for (found, level, needle) in [
            (Some(PointerExtension::Responding), Level::Ok, "answers"),
            (
                Some(PointerExtension::InstalledNotLoaded(Some(
                    "INITIALIZED".into(),
                ))),
                Level::Warn,
                "INITIALIZED",
            ),
            (
                Some(PointerExtension::NotInstalled),
                Level::Warn,
                "not installed",
            ),
            (None, Level::Warn, "not installed"),
        ] {
            facts.pointer_extension = found;

            let all = checks(&facts);
            let check = find(&all, "Hiding the pointer");

            assert_eq!(check.level, level, "{check:?}");
            assert!(check.detail.contains(needle), "{check:?}");

            if level == Level::Warn {
                assert!(
                    check
                        .fix
                        .as_deref()
                        .unwrap()
                        .contains("gnome-extension install")
                );
            }
        }

        // Not GNOME: nothing to install, and it says so.
        facts.desktop = Some("KDE".into());

        let all = checks(&facts);

        assert!(
            find(&all, "Hiding the pointer")
                .detail
                .contains("only GNOME")
        );

        // X11 hides it directly.
        facts.session_type = Some("x11".into());
        facts.wayland_display = false;

        let all = checks(&facts);

        assert!(find(&all, "Hiding the pointer").detail.contains("directly"));
    }

    #[test]
    fn an_unknown_session_asks_for_a_terminal_in_the_desktop() {
        let facts = Facts {
            edge_portal_enabled: true,
            ..Facts::default()
        };
        let all = checks(&facts);
        let check = find(&all, "Session");

        assert_eq!(check.level, Level::Warn);
        assert!(check.fix.as_deref().unwrap().contains("ssh"));
    }

    #[test]
    fn the_report_has_every_section_and_the_status() {
        let doctor = vec![Check {
            name: "Port".into(),
            level: Level::Ok,
            detail: "15101 is free".into(),
            fix: None,
        }];
        let report = format_report(&wayland_gnome(), &doctor, &Ok("State: running\n".into()));

        for needle in [
            "Aujar Mouse Borders report (version 0.1.1)",
            "== This machine ==",
            "Session: wayland (WAYLAND_DISPLAY is set)",
            "Desktop: ubuntu:GNOME",
            "GNOME Shell: GNOME Shell 46.0",
            "Kernel: Linux 6.8.0",
            "edge_portal = true",
            "== Checks ==",
            "[ ok ] Port: 15101 is free",
            "[ ok ] Screen edges (Wayland)",
            "== Status ==",
            "State: running",
            "== Recent log ==",
            "aujar[1]: ready",
        ] {
            assert!(report.contains(needle), "missing {needle:?} in:\n{report}");
        }

        assert!(report.contains("does not contain your security key"));
    }

    #[test]
    fn missing_pieces_are_said_not_skipped() {
        let mut facts = wayland_gnome();

        facts.log_tail = Some(Err("`journalctl` is not installed".into()));

        let report = format_report(&facts, &[], &Err("the daemon is not running".into()));

        assert!(report.contains("not available: the daemon is not running"));
        assert!(report.contains("not available: `journalctl` is not installed"));

        facts.log_tail = Some(Ok("  \n".into()));
        assert!(format_report(&facts, &[], &Ok(String::new())).contains("(the log is empty)"));

        facts.log_tail = None;
        assert!(format_report(&facts, &[], &Ok(String::new())).contains("not collected"));
    }

    #[test]
    fn failing_checks_show_their_fix_in_the_report() {
        let mut facts = wayland_gnome();

        facts.input_capture_portal = Some(Err("no".into()));

        let report = format_report(&facts, &[], &Ok(String::new()));

        assert!(report.contains("[FAIL] Screen edges (Wayland)"));
        assert!(report.contains("-> install xdg-desktop-portal"));
    }

    #[test]
    fn collecting_never_fails_and_reads_the_settings() {
        let settings = MouseBorders {
            edge_portal: false,
            hide_mouse_at_edge: true,
            ..MouseBorders::default()
        };
        let facts = collect(&settings);

        assert!(!facts.edge_portal_enabled);
        assert!(facts.hide_pointer_enabled);
        assert_eq!(facts.version, env!("CARGO_PKG_VERSION"));
    }
}
