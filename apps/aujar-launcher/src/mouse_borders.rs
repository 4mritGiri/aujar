//! `aujar mwb ...`: the shared `mwb` commands talking to the Aujar daemon, plus the Linux
//! system service.
//!
//! The commands themselves live in `aujar-mwb-cli`, shared with the standalone `aujar-mwb`
//! agent that also runs on Windows and macOS.

#[cfg(target_os = "linux")]
use std::collections::HashSet;
use std::path::PathBuf;

#[cfg(target_os = "linux")]
use anyhow::Context;
use anyhow::{Result, bail};
use async_trait::async_trait;
#[cfg(target_os = "linux")]
use aujar_core::Capability;
use aujar_ipc::{MouseBordersRequest, MouseBordersResponse, Request, Response, send};
use aujar_mwb_cli::{Cli, MwbCommand, Transport};
use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum MwbAction {
    #[command(flatten)]
    Common(MwbCommand),

    /// Run as a system service (for the login screen and elevated windows). See
    /// docs/MOUSE_BORDERS.md.
    #[cfg(target_os = "linux")]
    Service,
}

/// Sends requests to the per-user Aujar daemon.
struct DaemonTransport(PathBuf);

#[async_trait]
impl Transport for DaemonTransport {
    async fn call(&self, request: MouseBordersRequest) -> Result<MouseBordersResponse> {
        match send(&self.0, Request::MouseBorders(request)).await? {
            Response::MouseBorders(response) => Ok(response),
            Response::Error(message) => bail!(message),
            other => bail!("unexpected answer: {other:?}"),
        }
    }
}

pub async fn run(action: MwbAction, socket: PathBuf) -> Result<()> {
    match action {
        #[cfg(target_os = "linux")]
        MwbAction::Service => service().await,

        MwbAction::Common(command) => {
            let transport = DaemonTransport(socket);

            aujar_mwb_cli::run(
                command,
                &Cli {
                    transport: &transport,
                    program: "aujar mwb",
                },
            )
            .await
        }
    }
}

/// Run the system service: the same supervisor as the daemon, configured from `/etc/aujar`,
/// without a clipboard (there is no desktop session to share one with).
#[cfg(target_os = "linux")]
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
    use clap::Parser;

    #[derive(Parser)]
    struct Parsed {
        #[command(subcommand)]
        action: MwbAction,
    }

    #[test]
    fn shared_and_service_commands_both_parse() {
        assert!(matches!(
            Parsed::try_parse_from(["x", "pair"]).unwrap().action,
            MwbAction::Common(MwbCommand::Pair)
        ));
        assert!(matches!(
            Parsed::try_parse_from(["x", "doctor"]).unwrap().action,
            MwbAction::Common(MwbCommand::Doctor)
        ));
        assert!(Parsed::try_parse_from(["x", "audit", "-n", "5"]).is_ok());

        #[cfg(target_os = "linux")]
        assert!(matches!(
            Parsed::try_parse_from(["x", "service"]).unwrap().action,
            MwbAction::Service
        ));
    }
}
