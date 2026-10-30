//! One-step setup of the device access Mouse Borders needs on Linux.
//!
//! Reading this machine's keyboard and mouse needs the `input` group; typing and clicking for
//! other machines needs `/dev/uinput`. Doing that by hand (kernel module, udev rule, group,
//! log out and in) is too much to ask, so [`setup`] does it in one go behind a single password
//! prompt (`pkexec`, or `sudo` in a terminal) and, where it can, makes it work without logging
//! out. The privileged part is `data/linux-setup.sh`.
//!
//! On macOS and Windows the operating system asks for permissions itself, so nothing here
//! applies and [`needs_setup`] is always false.

use std::{fmt, io, process::Command};

/// The script that runs as root. It validates its one argument, the user name.
pub const SETUP_SCRIPT: &str = include_str!("../data/linux-setup.sh");

/// Whether this machine's input devices can be used by the current user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Access {
    /// `/dev/input/event*` exist but none can be read.
    pub capture_blocked: bool,
    /// `/dev/uinput` is missing or cannot be written.
    pub inject_blocked: bool,
}

impl Access {
    /// True when [`setup`] has something to fix.
    pub const fn needs_setup(self) -> bool {
        self.capture_blocked || self.inject_blocked
    }
}

/// How [`setup`] talks to the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interaction {
    /// A terminal: the password prompt and the script's messages use it.
    Terminal,
    /// No terminal (the settings window): a graphical prompt asks, output is collected.
    Background,
}

/// What state [`setup`] left things in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing to do, or access works now.
    Ready,
    /// Access is granted but only takes effect after logging out and in again.
    NeedsRelogin,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupError(pub String);

impl fmt::Display for SetupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SetupError {}

/// True when an attempt to open a device failed in a way setting up can fix.
fn fixable(result: &io::Result<()>) -> bool {
    matches!(
        result,
        Err(error) if matches!(
            error.kind(),
            io::ErrorKind::PermissionDenied | io::ErrorKind::NotFound
        )
    )
}

/// Whether `name` is a plain account name, as the setup script requires.
pub fn valid_user(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Look at the devices now.
#[cfg(target_os = "linux")]
pub fn probe() -> Access {
    let mut events = 0usize;
    let mut readable = 0usize;

    if let Ok(entries) = std::fs::read_dir("/dev/input") {
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("event") {
                events += 1;

                if std::fs::File::open(entry.path()).is_ok() {
                    readable += 1;
                }
            }
        }
    }

    let uinput = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/uinput")
        .map(drop);

    Access {
        // No input devices at all (a container, say) is not something to set up.
        capture_blocked: events > 0 && readable == 0,
        inject_blocked: fixable(&uinput),
    }
}

#[cfg(not(target_os = "linux"))]
pub fn probe() -> Access {
    Access {
        capture_blocked: false,
        inject_blocked: false,
    }
}

/// Whether [`setup`] would change anything.
pub fn needs_setup() -> bool {
    probe().needs_setup()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Elevation {
    Root,
    Pkexec,
    Sudo,
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

#[cfg(unix)]
fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;

    std::fs::metadata("/proc/self").is_ok_and(|meta| meta.uid() == 0)
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

/// How to get root. A terminal prefers `sudo`, which people expect there; the settings window
/// needs `pkexec`, which shows the desktop's own password dialog.
fn elevation(interaction: Interaction) -> Option<Elevation> {
    if is_root() {
        return Some(Elevation::Root);
    }

    let order: [(&str, Elevation); 2] = match interaction {
        Interaction::Terminal => [("sudo", Elevation::Sudo), ("pkexec", Elevation::Pkexec)],
        Interaction::Background => [("pkexec", Elevation::Pkexec), ("sudo", Elevation::Sudo)],
    };

    order
        .into_iter()
        // `sudo` cannot ask for a password without a terminal.
        .filter(|(_, how)| interaction == Interaction::Terminal || *how != Elevation::Sudo)
        .find(|(program, _)| on_path(program))
        .map(|(_, how)| how)
}

fn setup_command(how: Elevation, user: &str) -> Command {
    let mut command = match how {
        Elevation::Root => Command::new("/bin/sh"),
        Elevation::Pkexec => {
            let mut command = Command::new("pkexec");
            command.arg("/bin/sh");
            command
        }
        Elevation::Sudo => {
            let mut command = Command::new("sudo");
            command.arg("/bin/sh");
            command
        }
    };

    command
        .arg("-c")
        .arg(SETUP_SCRIPT)
        .arg("aujar-mwb-setup")
        .arg(user);

    command
}

fn current_user() -> Result<String, SetupError> {
    let output = Command::new("id")
        .arg("-un")
        .output()
        .map_err(|error| SetupError(format!("cannot tell who you are (`id -un`): {error}")))?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();

    if valid_user(&name) {
        Ok(name)
    } else {
        Err(SetupError(format!(
            "this account name (`{name}`) cannot be set up automatically"
        )))
    }
}

/// Give the current user the device access Mouse Borders needs. Blocks while the person types
/// their password, so call it off the UI thread.
///
/// Does nothing when everything already works. On Windows and macOS it does nothing at all.
pub fn setup(interaction: Interaction) -> Result<Outcome, SetupError> {
    if cfg!(not(target_os = "linux")) || !needs_setup() {
        return Ok(Outcome::Ready);
    }

    let user = current_user()?;
    let how = elevation(interaction).ok_or_else(|| {
        SetupError(
            "neither `pkexec` nor `sudo` is available; run the steps in \
             docs/MOUSE_BORDERS.md#permissions as root"
                .into(),
        )
    })?;

    let mut command = setup_command(how, &user);
    let output = match interaction {
        Interaction::Terminal => command.status().map(|status| (status, String::new())),
        Interaction::Background => command.output().map(|output| {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&output.stdout)
            );

            (output.status, text)
        }),
    }
    .map_err(|error| SetupError(format!("cannot start the setup: {error}")))?;

    let (status, text) = output;

    if !status.success() {
        // 126 and 127 are what pkexec returns when the password dialog is dismissed or the
        // password is wrong: say that rather than a code.
        return Err(SetupError(match status.code() {
            Some(126 | 127) => "the password prompt was cancelled".into(),
            _ => {
                let detail = text.trim();

                if detail.is_empty() {
                    format!("setup failed ({status})")
                } else {
                    format!("setup failed: {detail}")
                }
            }
        }));
    }

    Ok(if needs_setup() {
        Outcome::NeedsRelogin
    } else {
        Outcome::Ready
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_account_names_only() {
        for good in ["amrit", "amrit.giri", "a_b-c", "user1"] {
            assert!(valid_user(good), "{good}");
        }

        for bad in ["", "-rf", "a b", "a;b", "$(id)", "a/b", "né", "a\nb"] {
            assert!(!valid_user(bad), "{bad:?}");
        }
    }

    #[test]
    fn only_blocked_devices_need_setup() {
        let access = |capture_blocked, inject_blocked| Access {
            capture_blocked,
            inject_blocked,
        };

        assert!(!access(false, false).needs_setup());
        assert!(access(true, false).needs_setup());
        assert!(access(false, true).needs_setup());
        assert!(access(true, true).needs_setup());
    }

    #[test]
    fn missing_or_denied_devices_are_fixable_other_errors_are_not() {
        let err = |kind| Err(io::Error::from(kind));

        assert!(fixable(&err(io::ErrorKind::PermissionDenied)));
        assert!(fixable(&err(io::ErrorKind::NotFound)));
        assert!(!fixable(&err(io::ErrorKind::TimedOut)));
        assert!(!fixable(&Ok(())));
    }

    #[test]
    fn command_passes_the_script_and_the_user_as_separate_arguments() {
        let command = setup_command(Elevation::Pkexec, "amrit");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(command.get_program(), "pkexec");
        assert_eq!(args[0], "/bin/sh");
        assert_eq!(args[1], "-c");
        assert_eq!(args[2], SETUP_SCRIPT);
        assert_eq!(args[4], "amrit");
    }

    #[test]
    fn a_background_setup_never_picks_sudo() {
        // Without a terminal `sudo` cannot ask for the password.
        assert_ne!(elevation(Interaction::Background), Some(Elevation::Sudo));
    }

    #[cfg(unix)]
    fn run_script(user: &str) -> std::process::Output {
        Command::new("/bin/sh")
            .arg("-c")
            .arg(SETUP_SCRIPT)
            .arg("aujar-mwb-setup")
            .arg(user)
            .output()
            .unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_script_is_valid_shell() {
        let path = std::env::temp_dir().join(format!("aujar-setup-{}.sh", std::process::id()));

        std::fs::write(&path, SETUP_SCRIPT).unwrap();

        let mut check = Command::new("/bin/sh");

        check.arg("-n").arg(&path);

        let status = check.status().unwrap();

        let _ = std::fs::remove_file(&path);

        assert!(status.success());
    }

    #[cfg(unix)]
    #[test]
    fn the_script_refuses_names_that_could_inject_commands() {
        // Validation comes before anything touches the system, root or not.
        for bad in ["", "a;reboot", "$(id)", "-rf", "a b"] {
            let output = run_script(bad);

            assert_eq!(output.status.code(), Some(2), "{bad:?}");
        }
    }

    /// `.exit 0` (a typo) is valid shell syntax but a missing command, so with `set -e` it
    /// fails the whole package installation: check the last line, not just the syntax.
    #[cfg(unix)]
    #[test]
    fn the_package_scripts_end_with_a_real_exit() {
        let debian = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../apps/aujar-launcher/debian");

        for name in ["postinst", "prerm"] {
            let Ok(text) = std::fs::read_to_string(debian.join(name)) else {
                continue;
            };

            let last = text.lines().rev().find(|line| !line.trim().is_empty());

            assert_eq!(last, Some("exit 0"), "{name}");
        }
    }

    #[test]
    fn the_script_installs_the_same_rule_as_the_package() {
        let packaged = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../packaging/udev/70-aujar-mouse-borders.rules");

        // A crate downloaded on its own has no `packaging` folder next to it.
        let Ok(packaged) = std::fs::read_to_string(packaged) else {
            return;
        };

        let embedded: String = SETUP_SCRIPT
            .lines()
            .skip_while(|line| !line.contains("<<'RULE'"))
            .skip(1)
            .take_while(|line| *line != "RULE")
            .map(|line| format!("{line}\n"))
            .collect();

        assert_eq!(embedded, packaged);
    }
}
