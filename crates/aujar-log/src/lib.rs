//! Logging for every Aujar binary (daemon, CLI, UI, Mouse Borders agent), set up the same way.
//!
//! What is logged is controlled by the environment, so it works for programs started by systemd
//! or from the desktop as well as from a terminal:
//!
//! | Variable | Effect |
//! |---|---|
//! | `RUST_LOG` | standard `tracing` filter syntax, e.g. `debug` or `info,aujar_mouse_borders=trace`; wins over everything below |
//! | `AUJAR_DEBUG=1` | debug messages from Aujar's own crates (same as `-v` on the CLI) |
//! | `AUJAR_IPC_TRACE=1` | one line per request between a client (CLI, UI) and the daemon: its kind, how long it took, the outcome. Never its contents |
//! | `AUJAR_LOG_FILE=1` | also write the log to `<state dir>/logs/<program>.log` (0600; at most 5 MiB, then one `.1` backup). Give a path instead of `1` to choose the file |
//!
//! The file is what makes the UI debuggable: it is normally started from the desktop and has no
//! terminal. Panics are logged too, so they reach the file.
//!
//! `log` records (GPUI, and most other libraries) are forwarded into `tracing`, so they show up
//! in the same place.

use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::filter::Directive;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

/// A log file larger than this is moved to `<name>.log.1` before a new one is started.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// The `tracing` target of IPC trace lines. Enable with `RUST_LOG=aujar::ipc=debug` or
/// `AUJAR_IPC_TRACE=1`.
pub const IPC_TARGET: &str = "aujar::ipc";

/// For long-running programs (daemon, UI, agent): `info` by default.
///
/// `RUST_LOG` and the variables in the crate documentation change that. Safe to call more than
/// once (tests, a CLI that starts the daemon in-process): only the first call has an effect.
pub fn init(app: &str) {
    setup(app, 0, "info");
}

/// Like [`init`], with a verbosity from the command line: 0 = default, 1 = Aujar's debug
/// messages, 2 or more = Aujar's trace messages.
pub fn init_with(app: &str, verbosity: u8) {
    setup(app, verbosity, "info");
}

/// For short-lived command-line programs: stays silent (exactly as if this were not called)
/// unless the user asked for logging with `-v`, `RUST_LOG` or one of the variables in the crate
/// documentation, so ordinary command output is never mixed with log lines.
pub fn init_cli(app: &str, verbosity: u8) {
    if verbosity == 0 && !logging_requested() {
        return;
    }

    setup(app, verbosity, "warn");
}

/// Where `AUJAR_LOG_FILE` makes `app` write its log, if it is set.
pub fn log_file_path(app: &str) -> Option<PathBuf> {
    resolve_log_path(
        std::env::var_os("AUJAR_LOG_FILE").as_deref(),
        app,
        &aujar_config::platform::state_dir(),
    )
}

fn setup(app: &str, verbosity: u8, base: &str) {
    let filter = env_filter(verbosity, base);

    let stderr = fmt::layer()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal());

    let file =
        open_log_file(app).map(|file| fmt::layer().with_writer(Mutex::new(file)).with_ansi(false));

    let installed = tracing_subscriber::registry()
        .with(filter)
        .with(stderr)
        .with(file)
        .try_init()
        .is_ok();

    if installed {
        log_panics();
    }
}

fn logging_requested() -> bool {
    std::env::var_os("RUST_LOG").is_some()
        || std::env::var_os("AUJAR_LOG_FILE").is_some()
        || env_flag("AUJAR_DEBUG")
        || env_flag("AUJAR_IPC_TRACE")
}

fn env_flag(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty() && value != "0")
}

/// The filter directives used when `RUST_LOG` is not set.
///
/// A target directive matches by prefix, so `aujar=debug` covers `aujar_daemon`,
/// `aujar_mouse_borders` and the rest.
fn default_directives(verbosity: u8, aujar_debug: bool, base: &str) -> String {
    let verbosity = if verbosity == 0 && aujar_debug {
        1
    } else {
        verbosity
    };

    match verbosity {
        0 => base.to_owned(),
        1 => format!("{base},aujar=debug"),
        _ => format!("{base},aujar=trace"),
    }
}

fn env_filter(verbosity: u8, base: &str) -> EnvFilter {
    let mut filter = match EnvFilter::try_from_default_env() {
        Ok(filter) => filter,
        Err(error) => {
            if std::env::var_os("RUST_LOG").is_some() {
                eprintln!("aujar: ignoring invalid RUST_LOG: {error}");
            }

            EnvFilter::new(default_directives(verbosity, env_flag("AUJAR_DEBUG"), base))
        }
    };

    if env_flag("AUJAR_IPC_TRACE") {
        if let Ok(directive) = format!("{IPC_TARGET}=debug").parse::<Directive>() {
            filter = filter.add_directive(directive);
        }
    }

    filter
}

/// Interpret the value of `AUJAR_LOG_FILE`: unset, empty or `0` means no file; `1` or `default`
/// the standard place under the state directory; anything else is the path itself.
fn resolve_log_path(value: Option<&OsStr>, app: &str, state_dir: &Path) -> Option<PathBuf> {
    let value = value?;

    if value.is_empty() || value == "0" {
        return None;
    }

    if value == "1" || value == "default" {
        return Some(state_dir.join("logs").join(format!("{app}.log")));
    }

    Some(PathBuf::from(value))
}

fn open_log_file(app: &str) -> Option<File> {
    let path = log_file_path(app)?;

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    rotate_if_large(&path);

    let mut options = OpenOptions::new();
    options.create(true).append(true);

    // Logs can contain file names and desktop details: readable by the owner only.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    match options.open(&path) {
        Ok(file) => Some(file),
        Err(error) => {
            eprintln!(
                "aujar: cannot write the log file {}: {error}",
                path.display()
            );

            None
        }
    }
}

/// Move a log that has grown past [`MAX_LOG_BYTES`] to `<name>.1`, replacing an older backup.
fn rotate_if_large(path: &Path) {
    let too_big = fs::metadata(path).is_ok_and(|meta| meta.len() > MAX_LOG_BYTES);

    if too_big {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".1");

        let _ = fs::rename(path, backup);
    }
}

/// Send panic messages through `tracing` (and so into the log file), then to the previous hook.
fn log_panics() {
    let previous = std::panic::take_hook();

    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");

        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_level_is_the_base() {
        assert_eq!(default_directives(0, false, "info"), "info");
        assert_eq!(default_directives(0, false, "warn"), "warn");
    }

    #[test]
    fn verbosity_raises_only_aujars_own_crates() {
        assert_eq!(default_directives(1, false, "warn"), "warn,aujar=debug");
        assert_eq!(default_directives(2, false, "warn"), "warn,aujar=trace");
        assert_eq!(default_directives(5, false, "info"), "info,aujar=trace");
    }

    #[test]
    fn aujar_debug_means_one_v_but_never_lowers_an_explicit_one() {
        assert_eq!(default_directives(0, true, "info"), "info,aujar=debug");
        assert_eq!(default_directives(2, true, "info"), "info,aujar=trace");
    }

    #[test]
    fn the_default_directives_are_valid_filters() {
        for verbosity in 0..=3 {
            for debug in [false, true] {
                let directives = default_directives(verbosity, debug, "info");

                assert!(
                    EnvFilter::try_new(&directives).is_ok(),
                    "{directives} must parse"
                );
            }
        }

        assert!(format!("{IPC_TARGET}=debug").parse::<Directive>().is_ok());
    }

    #[test]
    fn log_file_value_is_interpreted() {
        let state = Path::new("/state");

        assert_eq!(resolve_log_path(None, "aujar-ui", state), None);
        assert_eq!(
            resolve_log_path(Some(OsStr::new("")), "aujar-ui", state),
            None
        );
        assert_eq!(
            resolve_log_path(Some(OsStr::new("0")), "aujar-ui", state),
            None
        );
        assert_eq!(
            resolve_log_path(Some(OsStr::new("1")), "aujar-ui", state),
            Some(PathBuf::from("/state/logs/aujar-ui.log"))
        );
        assert_eq!(
            resolve_log_path(Some(OsStr::new("default")), "aujar-daemon", state),
            Some(PathBuf::from("/state/logs/aujar-daemon.log"))
        );
        assert_eq!(
            resolve_log_path(Some(OsStr::new("/tmp/x.log")), "aujar-ui", state),
            Some(PathBuf::from("/tmp/x.log"))
        );
    }

    #[test]
    fn an_oversized_log_is_rotated_and_a_small_one_is_kept() {
        let dir = std::env::temp_dir().join(format!("aujar-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let small = dir.join("small.log");
        fs::write(&small, b"short").unwrap();
        rotate_if_large(&small);
        assert!(small.exists());
        assert!(!dir.join("small.log.1").exists());

        let big = dir.join("big.log");
        fs::write(&big, vec![b'x'; (MAX_LOG_BYTES + 1) as usize]).unwrap();
        rotate_if_large(&big);
        assert!(!big.exists());
        assert!(dir.join("big.log.1").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
