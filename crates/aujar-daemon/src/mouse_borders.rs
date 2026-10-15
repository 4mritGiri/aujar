//! Serves `Request::MouseBorders` by talking to the Mouse Borders service.

use std::{collections::HashSet, path::PathBuf};

use aujar_core::Capability;
use aujar_ipc::{
    MouseBordersPeer, MouseBordersRequest, MouseBordersResponse, MouseBordersStatus, Response,
};
use aujar_mouse_borders::{
    engine::Command,
    layout::MachineName,
    service::{MouseBordersModule, Profile, ServiceHandle, ServiceReport, ServiceState},
    status::Status,
};

/// The module for this user's session, ready to register with the runtime.
pub fn module(denied: &HashSet<Capability>) -> MouseBordersModule {
    MouseBordersModule::new(Profile::user(denied.clone()))
}

pub async fn handle(request: MouseBordersRequest, module: &MouseBordersModule) -> Response {
    let Some(service) = module.handle() else {
        return Response::Error("Mouse Borders has not started yet".into());
    };

    match run(request, &service).await {
        Ok(response) => Response::MouseBorders(response),
        Err(message) => Response::Error(message),
    }
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

fn status(report: ServiceReport) -> MouseBordersStatus {
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
