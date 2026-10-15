//! `aujar mwb ...`: control sharing of keyboard, mouse, clipboard and files between machines.

use std::{collections::HashSet, fmt::Write as _, path::PathBuf};

use anyhow::{Context, Result, bail};
use aujar_core::Capability;
use aujar_ipc::{
    MouseBordersRequest, MouseBordersResponse, MouseBordersStatus, Request, Response, send,
};
use clap::{Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Toggle {
    On,
    Off,
}

impl Toggle {
    fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

#[derive(Debug, Subcommand)]
pub enum KeyAction {
    /// Create the security key that pairs your machines.
    Generate {
        /// Replace an existing key (disconnects machines paired with the old one).
        #[arg(long)]
        force: bool,
    },

    /// Print the security key to copy to your other machines.
    Show,

    /// Store the key created on another machine.
    Set {
        /// The key, e.g. `K7QD-2XJP-...` (all 32 characters).
        key: String,

        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum MwbAction {
    /// Show whether sharing works, which machines are connected and what happened recently.
    Status,

    /// Manage the security key.
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },

    /// Give the keyboard and mouse to a machine by name, or `local` for this one.
    Switch { machine: String },

    /// Switch machines by moving the pointer across screen edges.
    EasyMouse {
        #[arg(value_enum)]
        state: Toggle,
    },

    /// Send your keyboard and mouse input to every connected machine at once.
    MultiInput {
        #[arg(value_enum)]
        state: Toggle,
    },

    /// Lock every connected machine (and this one).
    LockAll,

    /// Drop all connections and connect again.
    Reconnect,

    /// Re-read settings and the key now.
    Reload,

    /// Send files (up to 100 MB each) to a connected machine.
    Send {
        machine: String,

        #[arg(required = true)]
        files: Vec<PathBuf>,
    },

    /// Run as a system service (for the login screen and elevated windows). See
    /// docs/MOUSE_BORDERS.md.
    Service,
}

pub async fn run(action: MwbAction, socket: PathBuf) -> Result<()> {
    match action {
        MwbAction::Service => service().await,

        MwbAction::Status => match ask(&socket, MouseBordersRequest::Status).await? {
            MouseBordersResponse::Status(status) => {
                print!("{}", format_status(&status));

                Ok(())
            }
            other => bail!("unexpected answer: {other:?}"),
        },

        MwbAction::Key { action } => key(&socket, action).await,

        MwbAction::Switch { machine } => {
            let machine = (machine != "local").then_some(machine);

            done(&socket, MouseBordersRequest::Switch { machine }).await
        }

        MwbAction::EasyMouse { state } => {
            done(
                &socket,
                MouseBordersRequest::SetEasyMouse {
                    enabled: state.enabled(),
                },
            )
            .await
        }

        MwbAction::MultiInput { state } => {
            done(
                &socket,
                MouseBordersRequest::MultiMachineInput {
                    enabled: state.enabled(),
                },
            )
            .await
        }

        MwbAction::LockAll => done(&socket, MouseBordersRequest::LockAll).await,
        MwbAction::Reconnect => done(&socket, MouseBordersRequest::Reconnect).await,
        MwbAction::Reload => done(&socket, MouseBordersRequest::Reload).await,

        MwbAction::Send { machine, files } => {
            // The daemon has its own working directory: send absolute paths.
            let paths = files
                .iter()
                .map(|file| {
                    std::fs::canonicalize(file)
                        .with_context(|| format!("cannot find {}", file.display()))
                        .map(|path| path.display().to_string())
                })
                .collect::<Result<Vec<_>>>()?;

            done(&socket, MouseBordersRequest::SendFiles { machine, paths }).await
        }
    }
}

async fn ask(
    socket: &std::path::Path,
    request: MouseBordersRequest,
) -> Result<MouseBordersResponse> {
    match send(socket, Request::MouseBorders(request)).await? {
        Response::MouseBorders(response) => Ok(response),
        Response::Error(message) => bail!(message),
        other => bail!("unexpected answer: {other:?}"),
    }
}

async fn done(socket: &std::path::Path, request: MouseBordersRequest) -> Result<()> {
    match ask(socket, request).await? {
        MouseBordersResponse::Done(message) => {
            println!("{message}");

            Ok(())
        }
        other => bail!("unexpected answer: {other:?}"),
    }
}

async fn key(socket: &std::path::Path, action: KeyAction) -> Result<()> {
    match action {
        KeyAction::Generate { force } => {
            let MouseBordersResponse::Key(key) =
                ask(socket, MouseBordersRequest::GenerateKey { force }).await?
            else {
                bail!("the daemon did not return a key");
            };

            println!("Security key created:\n\n    {key}\n");
            println!("On every other machine run:\n\n    aujar mwb key set {key}\n");
            println!("Keep it private: anyone with the key can connect to your machines.");

            Ok(())
        }

        KeyAction::Show => match ask(socket, MouseBordersRequest::ShowKey).await? {
            MouseBordersResponse::Key(key) => {
                println!("{key}");

                Ok(())
            }
            MouseBordersResponse::NoKey => {
                bail!("there is no security key yet; run `aujar mwb key generate`")
            }
            other => bail!("unexpected answer: {other:?}"),
        },

        KeyAction::Set { key, force } => {
            done(socket, MouseBordersRequest::SetKey { key, force }).await
        }
    }
}

/// Human-readable status.
pub fn format_status(status: &MouseBordersStatus) -> String {
    let mut out = String::new();

    match status.state.as_str() {
        "running" => {
            let _ = writeln!(out, "Mouse Borders: running as `{}`", status.local);

            if let Some(address) = &status.listening {
                let _ = writeln!(out, "Listening on:  {address}");
            }

            let holder = if status.focus == status.local {
                "this machine".to_owned()
            } else {
                status.focus.clone()
            };
            let _ = writeln!(out, "Keyboard and mouse are on: {holder}");

            if let Some(controller) = &status.controlled_by {
                let _ = writeln!(out, "Controlled by: {controller}");
            }

            let _ = writeln!(out, "Switching with the mouse: {}", status.easy_mouse);

            if status.multi_machine_input {
                let _ = writeln!(out, "Multi-machine input: ON (input goes to every machine)");
            }
        }
        state => {
            let label = match state {
                "disabled" => "off",
                "waiting_for_key" => "waiting for a security key",
                "blocked" => "blocked by policy",
                _ => "not working",
            };
            let _ = writeln!(out, "Mouse Borders: {label}");

            if !status.detail.is_empty() {
                let _ = writeln!(out, "  {}", status.detail);
            }
        }
    }

    if !status.peers.is_empty() {
        let _ = writeln!(out, "\nOther machines:");

        let width = status
            .peers
            .iter()
            .map(|peer| peer.name.len())
            .max()
            .unwrap_or(0);

        for peer in &status.peers {
            let slot = peer
                .slot
                .map_or_else(String::new, |slot| format!("#{}", slot + 1));
            let state = if peer.connected {
                let screen = peer
                    .screen
                    .map_or_else(String::new, |(w, h)| format!("  {w}x{h}"));
                let address = peer.address.as_deref().unwrap_or("");

                format!("connected  {address}{screen}")
            } else {
                match &peer.last_error {
                    Some(error) => format!("not connected ({error})"),
                    None => "not connected".to_owned(),
                }
            };

            let _ = writeln!(out, "  {slot:<3} {:<width$}  {state}", peer.name);
        }
    }

    for (label, problem) in [
        (
            "Switching at screen edges",
            &status.edge_switching_unavailable,
        ),
        (
            "Capturing this machine's input",
            &status.capture_unavailable,
        ),
    ] {
        if let Some(problem) = problem {
            let _ = writeln!(out, "\n{label} is unavailable: {problem}");
        }
    }

    if !status.log.is_empty() {
        let _ = writeln!(out, "\nRecent:");

        for line in status.log.iter().rev().take(8).rev() {
            let _ = writeln!(out, "  {line}");
        }
    }

    out
}

/// Run the system service: the same supervisor as the daemon, configured from `/etc/aujar`,
/// without a clipboard (there is no desktop session to share one with).
async fn service() -> Result<()> {
    use aujar_mouse_borders::service::{Profile, ServiceState, Supervisor};

    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .try_init();

    let denied: HashSet<Capability> = aujar_config::Policy::load_optional("/etc/aujar/policy.toml")
        .context("cannot load /etc/aujar/policy.toml")?
        .denied_capabilities
        .iter()
        .filter_map(|name| Capability::parse(name))
        .collect();

    let supervisor = Supervisor::spawn(Profile::system(denied));
    let mut report = supervisor.handle().subscribe();
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut last = None;

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = sigterm.recv() => break,
            changed = report.changed() => {
                if changed.is_err() {
                    break;
                }

                let state = report.borrow().state.clone();

                if last.as_ref() != Some(&state) {
                    match &state {
                        ServiceState::Running => tracing::info!("Mouse Borders service running"),
                        other => tracing::warn!(?other, "Mouse Borders service is not running"),
                    }

                    last = Some(state);
                }
            }
        }
    }

    supervisor.shutdown().await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aujar_ipc::MouseBordersPeer;

    fn running() -> MouseBordersStatus {
        MouseBordersStatus {
            state: "running".into(),
            detail: String::new(),
            local: "desk".into(),
            focus: "desk".into(),
            controlled_by: None,
            multi_machine_input: false,
            easy_mouse: "Enabled".into(),
            listening: Some("0.0.0.0:15101".into()),
            edge_switching_unavailable: None,
            capture_unavailable: None,
            peers: vec![
                MouseBordersPeer {
                    name: "laptop".into(),
                    connected: true,
                    address: Some("192.168.1.20:15101".into()),
                    screen: Some((1920, 1080)),
                    slot: Some(1),
                    last_error: None,
                },
                MouseBordersPeer {
                    name: "tv".into(),
                    connected: false,
                    address: None,
                    screen: None,
                    slot: Some(2),
                    last_error: Some("connection refused".into()),
                },
            ],
            log: vec!["connected to laptop".into()],
        }
    }

    #[test]
    fn running_status_lists_machines_and_what_happened() {
        let text = format_status(&running());

        assert!(text.contains("running as `desk`"), "{text}");
        assert!(text.contains("are on: this machine"));
        assert!(text.contains("#2  laptop  connected  192.168.1.20:15101  1920x1080"));
        assert!(text.contains("#3  tv"));
        assert!(text.contains("not connected (connection refused)"));
        assert!(text.contains("Recent:\n  connected to laptop"));
    }

    #[test]
    fn status_says_who_has_the_keyboard_and_who_controls_this_machine() {
        let mut status = running();
        status.focus = "laptop".into();
        status.controlled_by = Some("laptop".into());
        status.multi_machine_input = true;

        let text = format_status(&status);

        assert!(text.contains("are on: laptop"));
        assert!(text.contains("Controlled by: laptop"));
        assert!(text.contains("Multi-machine input: ON"));
    }

    #[test]
    fn problems_are_explained_not_hidden() {
        let mut status = running();
        status.edge_switching_unavailable = Some("Wayland does not allow it".into());
        status.capture_unavailable = Some("no permission".into());

        let text = format_status(&status);

        assert!(text.contains("Switching at screen edges is unavailable: Wayland"));
        assert!(text.contains("Capturing this machine's input is unavailable: no permission"));
    }

    #[test]
    fn stopped_states_give_the_next_step() {
        let mut status = running();
        status.state = "waiting_for_key".into();
        status.detail = "run `aujar mwb key generate`".into();
        status.peers.clear();

        let text = format_status(&status);

        assert!(text.starts_with("Mouse Borders: waiting for a security key"));
        assert!(text.contains("aujar mwb key generate"));
        assert!(!text.contains("Other machines"));
    }

    #[test]
    fn only_the_latest_log_lines_are_shown() {
        let mut status = running();
        status.log = (0..20).map(|i| format!("event {i}")).collect();

        let text = format_status(&status);

        assert!(text.contains("event 19") && text.contains("event 12"));
        assert!(!text.contains("event 11"));
    }
}
