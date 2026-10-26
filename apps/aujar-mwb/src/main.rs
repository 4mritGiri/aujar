//! `aujar-mwb`: the Mouse Borders agent and its command line, for Linux, macOS and Windows.
//!
//! `aujar-mwb run` is the agent: it connects to your other machines and shares the keyboard,
//! mouse, clipboard and files. Every other command (`status`, `pair`, `join`, `doctor`, ...)
//! talks to the running agent over a local control channel, or works on files directly.

use std::{collections::HashSet, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, bail};
use aujar_core::Capability;
use aujar_mouse_borders::{
    api::MouseBordersRequest,
    autostart,
    control::{self, Endpoint},
    service::{Profile, Supervisor},
};
use aujar_mwb_cli::{Cli as Context_, ControlTransport, MwbCommand};
use clap::{Parser, Subcommand};
use tokio::sync::watch;

#[derive(Parser)]
#[command(
    name = "aujar-mwb",
    version,
    about = "Share one keyboard and mouse between your computers"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the agent in the foreground.
    Run {
        /// Detach from the console window (Windows) and log to a file. Used by autostart.
        #[arg(long)]
        background: bool,
    },

    /// Start the agent automatically when you log in.
    Autostart {
        #[command(subcommand)]
        action: AutostartAction,
    },

    #[command(flatten)]
    Mwb(MwbCommand),
}

#[derive(Subcommand)]
enum AutostartAction {
    /// Start `aujar-mwb` at login.
    Enable,
    /// Stop starting it at login.
    Disable,
}

/// The machine-wide policy file an administrator can use to forbid capabilities.
fn policy_path() -> PathBuf {
    #[cfg(windows)]
    {
        let base = std::env::var_os("ProgramData")
            .map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);

        base.join("Aujar").join("policy.toml")
    }

    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/Aujar/policy.toml")
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    {
        PathBuf::from("/etc/aujar/policy.toml")
    }
}

fn denied_capabilities() -> Result<HashSet<Capability>> {
    let path = policy_path();

    Ok(aujar_config::Policy::load_optional(&path)
        .with_context(|| format!("cannot load {}", path.display()))?
        .denied_capabilities
        .iter()
        .filter_map(|name| Capability::parse(name))
        .collect())
}

fn init_logging(background: bool) {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());

    if background {
        // No console: keep a log next to the other state files (replaced at each start).
        let path = aujar_config::platform::state_dir().join("aujar-mwb.log");

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        if let Ok(file) = std::fs::File::create(path) {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_ansi(false)
                .with_writer(Arc::new(file))
                .try_init();

            return;
        }
    }

    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(windows)]
fn detach_console() {
    // SAFETY: a plain Win32 call; failure (no console) is fine.
    unsafe {
        windows_sys::Win32::System::Console::FreeConsole();
    }
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        if let Ok(mut terminate) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }

            return;
        }
    }

    let _ = tokio::signal::ctrl_c().await;
}

async fn run_agent(background: bool) -> Result<()> {
    #[cfg(windows)]
    {
        aujar_mouse_borders::backend::windows::init_process();

        if background {
            detach_console();
        }
    }

    init_logging(background);

    let endpoint = Endpoint::user_default();

    // One agent per user: a running one answers on the control channel.
    if control::request(&endpoint, &MouseBordersRequest::Status)
        .await
        .is_ok()
    {
        bail!(
            "the Mouse Borders agent is already running ({})",
            endpoint.describe()
        );
    }

    let supervisor = Supervisor::spawn(Profile::user(denied_capabilities()?));
    let (stop, shutdown) = watch::channel(false);

    let server = tokio::spawn({
        let endpoint = endpoint.clone();
        let handler = Arc::new(supervisor.handle());

        async move { control::serve(&endpoint, handler, shutdown).await }
    });

    tracing::info!(control = %endpoint.describe(), "Mouse Borders agent running; try `aujar-mwb status`");

    wait_for_shutdown().await;

    let _ = stop.send(true);

    if let Ok(Err(error)) = server.await {
        tracing::warn!(%error, "the control channel stopped with an error");
    }

    supervisor.shutdown().await;

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Run { background } => run_agent(background).await,

        Command::Autostart { action } => {
            let message = match action {
                AutostartAction::Enable => {
                    autostart::enable(&std::env::current_exe().context("cannot find this program")?)
                }
                AutostartAction::Disable => autostart::disable(),
            }?;

            println!("{message}");

            Ok(())
        }

        Command::Mwb(command) => {
            let transport = ControlTransport::user_default();

            aujar_mwb_cli::run(
                command,
                &Context_ {
                    transport: &transport,
                    program: "aujar-mwb",
                },
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_is_well_formed() {
        use clap::CommandFactory;

        Cli::command().debug_assert();
    }

    #[test]
    fn every_command_parses() {
        for args in [
            vec!["aujar-mwb", "run"],
            vec!["aujar-mwb", "run", "--background"],
            vec!["aujar-mwb", "status"],
            vec!["aujar-mwb", "pair"],
            vec!["aujar-mwb", "join", "aujar-pair1;row;k"],
            vec!["aujar-mwb", "doctor"],
            vec!["aujar-mwb", "audit", "-n", "5"],
            vec!["aujar-mwb", "autostart", "enable"],
            vec!["aujar-mwb", "switch", "laptop"],
            vec!["aujar-mwb", "send", "laptop", "a.txt"],
        ] {
            assert!(Cli::try_parse_from(&args).is_ok(), "{args:?}");
        }

        assert!(Cli::try_parse_from(["aujar-mwb"]).is_err());
        assert!(Cli::try_parse_from(["aujar-mwb", "nonsense"]).is_err());
    }

    #[test]
    fn the_policy_file_lives_in_a_machine_wide_place() {
        let path = policy_path();

        assert!(path.ends_with("policy.toml"));
        assert!(path.is_absolute());
    }

    #[test]
    fn a_missing_policy_file_denies_nothing() {
        // Nothing is installed at the machine-wide path on a developer machine.
        if !policy_path().exists() {
            assert!(denied_capabilities().unwrap().is_empty());
        }
    }
}
