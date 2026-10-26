//! The Mouse Borders control API: what `aujar mwb ...`, the settings window and the agent's
//! control channel ask of the running service, and what it answers.
//!
//! These types and the handler live here (not in the daemon's IPC crate) so every front end
//! on every platform shares one implementation.

use std::path::PathBuf;

use aujar_core::Capability;
use serde::{Deserialize, Serialize};

use crate::{
    engine::Command,
    layout::MachineName,
    service::{ServiceHandle, ServiceReport, ServiceState},
    status::Status,
};

/// Operations on the Mouse Borders service (`aujar mwb ...`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseBordersRequest {
    Status,
    /// Create the security key; `force` replaces an existing one.
    GenerateKey {
        force: bool,
    },
    ShowKey,
    /// Store a key copied from another machine.
    SetKey {
        key: String,
        force: bool,
    },
    /// Re-read settings and the key now.
    Reload,
    /// Give the keyboard and mouse to a machine (`None`: this one).
    Switch {
        machine: Option<String>,
    },
    SetEasyMouse {
        enabled: bool,
    },
    LockAll,
    Reconnect,
    MultiMachineInput {
        enabled: bool,
    },
    /// Send files (absolute paths) to a connected machine.
    SendFiles {
        machine: String,
        paths: Vec<String>,
    },
}

impl MouseBordersRequest {
    /// Capability needed on top of the request itself. Reading status (and explaining why
    /// the service is off) always works.
    pub fn required_capability(&self) -> Option<Capability> {
        match self {
            Self::Status
            | Self::ShowKey
            | Self::Reload
            | Self::GenerateKey { .. }
            | Self::SetKey { .. } => None,
            Self::SendFiles { .. } => Some(Capability::FileSystem),
            Self::Switch { .. }
            | Self::SetEasyMouse { .. }
            | Self::LockAll
            | Self::Reconnect
            | Self::MultiMachineInput { .. } => Some(Capability::ControlInput),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseBordersResponse {
    Status(Box<MouseBordersStatus>),
    /// The security key, in its copyable form.
    Key(String),
    /// A command ran; this is what to tell the user.
    Done(String),
    /// There is no security key yet.
    NoKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseBordersPeer {
    pub name: String,
    pub connected: bool,
    pub address: Option<String>,
    pub screen: Option<(u32, u32)>,
    pub slot: Option<u8>,
    pub last_error: Option<String>,
}

/// Service state in a form independent of the module crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MouseBordersStatus {
    /// `disabled`, `blocked`, `waiting_for_key`, `error` or `running`.
    pub state: String,
    /// Why, for every state except `running`.
    pub detail: String,
    pub local: String,
    /// Machine holding the keyboard and mouse.
    pub focus: String,
    pub controlled_by: Option<String>,
    pub multi_machine_input: bool,
    pub easy_mouse: String,
    pub listening: Option<String>,
    pub edge_switching_unavailable: Option<String>,
    pub capture_unavailable: Option<String>,
    pub peers: Vec<MouseBordersPeer>,
    pub log: Vec<String>,
}

/// Carry out a request against the running service. The error is a message for the user.
pub async fn handle(
    request: MouseBordersRequest,
    service: &ServiceHandle,
) -> Result<MouseBordersResponse, String> {
    run(request, service).await
}

async fn run(
    request: MouseBordersRequest,
    service: &ServiceHandle,
) -> Result<MouseBordersResponse, String> {
    let engine = |command| async move {
        service
            .request(command)
            .await
            .map(MouseBordersResponse::Done)
            .map_err(|error| error.to_string())
    };

    match request {
        MouseBordersRequest::Status => Ok(MouseBordersResponse::Status(Box::new(status(
            service.report(),
        )))),

        MouseBordersRequest::GenerateKey { force } => service
            .generate_key(force)
            .await
            .map(|key| MouseBordersResponse::Key(key.to_display()))
            .map_err(|error| error.to_string()),

        MouseBordersRequest::ShowKey => service
            .show_key()
            .map(|key| {
                key.map_or(MouseBordersResponse::NoKey, |key| {
                    MouseBordersResponse::Key(key.to_display())
                })
            })
            .map_err(|error| error.to_string()),

        MouseBordersRequest::SetKey { key, force } => service
            .set_key(&key, force)
            .await
            .map(|()| MouseBordersResponse::Done("security key saved".into()))
            .map_err(|error| error.to_string()),

        MouseBordersRequest::Reload => {
            service.reload().await;

            Ok(MouseBordersResponse::Done(
                "reloading settings and key".into(),
            ))
        }

        MouseBordersRequest::Switch { machine } => {
            let target = machine
                .as_deref()
                .map(MachineName::new)
                .transpose()
                .map_err(|error| error.to_string())?;

            engine(Command::SwitchTo(target)).await
        }

        MouseBordersRequest::SetEasyMouse { enabled } => {
            engine(Command::SetEasyMouse(enabled)).await
        }
        MouseBordersRequest::LockAll => engine(Command::LockAll).await,
        MouseBordersRequest::Reconnect => engine(Command::Reconnect).await,
        MouseBordersRequest::MultiMachineInput { enabled } => {
            engine(Command::MultiMachineInput(enabled)).await
        }

        MouseBordersRequest::SendFiles { machine, paths } => {
            let machine = MachineName::new(machine).map_err(|error| error.to_string())?;

            if let Some(relative) = paths.iter().find(|path| !path.starts_with('/')) {
                return Err(format!("`{relative}` must be an absolute path"));
            }

            engine(Command::SendFiles {
                machine,
                paths: paths.into_iter().map(PathBuf::from).collect(),
            })
            .await
        }
    }
}

pub fn status(report: ServiceReport) -> MouseBordersStatus {
    let (state, detail) = match &report.state {
        ServiceState::Disabled => (
            "disabled",
            "set `enabled = true` under [mouse_borders] in settings.toml".to_owned(),
        ),
        ServiceState::BlockedByPolicy(reason) => ("blocked", reason.clone()),
        ServiceState::WaitingForKey => (
            "waiting_for_key",
            "run `aujar mwb key generate` on one machine and `aujar mwb key set` on the others"
                .to_owned(),
        ),
        ServiceState::Error(message) => ("error", message.clone()),
        ServiceState::Running => ("running", String::new()),
    };

    let engine = report.engine.unwrap_or_else(Status::stopped);

    MouseBordersStatus {
        state: state.to_owned(),
        detail,
        local: engine.local,
        focus: engine.focus,
        controlled_by: engine.controlled_by,
        multi_machine_input: engine.multi_machine_input,
        easy_mouse: format!("{:?}", engine.easy_mouse),
        listening: engine.listening.map(|address| address.to_string()),
        edge_switching_unavailable: engine.edge_switching_unavailable,
        capture_unavailable: engine.capture_unavailable,
        peers: engine
            .peers
            .into_iter()
            .map(|peer| MouseBordersPeer {
                name: peer.name,
                connected: peer.connected,
                address: peer.address,
                screen: peer.screen.map(|size| (size.width, size.height)),
                slot: peer.slot,
                last_error: peer.last_error,
            })
            .collect(),
        log: engine.log.into_iter().collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_service_state_has_a_stable_name_and_helpful_detail() {
        let name = |state| {
            status(ServiceReport {
                state,
                engine: None,
            })
            .state
        };
        let detail = |state| {
            status(ServiceReport {
                state,
                engine: None,
            })
            .detail
        };

        assert_eq!(name(ServiceState::Disabled), "disabled");
        assert_eq!(name(ServiceState::WaitingForKey), "waiting_for_key");
        assert_eq!(name(ServiceState::Error("x".into())), "error");
        assert_eq!(name(ServiceState::BlockedByPolicy("y".into())), "blocked");
        assert_eq!(name(ServiceState::Running), "running");

        assert!(detail(ServiceState::WaitingForKey).contains("aujar mwb key generate"));
        assert_eq!(detail(ServiceState::Error("boom".into())), "boom");
        assert_eq!(detail(ServiceState::Running), "");
    }

    #[test]
    fn engine_status_is_flattened() {
        let mut engine = Status::stopped();
        engine.running = true;
        engine.local = "desk".into();
        engine.focus = "laptop".into();
        engine.push_log("connected to laptop");

        let dto = status(ServiceReport {
            state: ServiceState::Running,
            engine: Some(engine),
        });

        assert_eq!((dto.local.as_str(), dto.focus.as_str()), ("desk", "laptop"));
        assert_eq!(dto.log, ["connected to laptop"]);
    }
}
