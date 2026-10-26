//! NexDesk request and response types, shared by the daemon, the CLI and the UI.
//!
//! Deliberately small: Aujar starts and watches the NexDesk manager, nothing more. Connection
//! profiles, passwords and the vault stay inside NexDesk and never cross Aujar's IPC socket.

use aujar_runtime::Capability;
use serde::{Deserialize, Serialize};

use crate::NexDeskModule;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NexDeskRequest {
    /// Is NexDesk installed, where, and is the manager Aujar started still running?
    Status,
    /// Open the NexDesk manager window (does nothing new if Aujar already opened one).
    Open,
}

impl NexDeskRequest {
    /// Capability the daemon must grant before serving this request.
    pub fn required_capability(&self) -> Option<Capability> {
        match self {
            Self::Status => None,
            Self::Open => Some(Capability::ExecuteCommand),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NexDeskStatus {
    pub installed: bool,
    /// Full path of the executable that would be started.
    pub path: Option<String>,
    /// Process id of the manager Aujar started, while it is still running.
    pub running_pid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NexDeskResponse {
    Status(NexDeskStatus),
    Opened {
        pid: u32,
        /// `true` when a manager started earlier by Aujar was still running, so none was added.
        already_running: bool,
    },
}

/// Serve one request. Errors are user-facing messages.
pub fn handle(request: NexDeskRequest, module: &NexDeskModule) -> Result<NexDeskResponse, String> {
    match request {
        NexDeskRequest::Status => Ok(NexDeskResponse::Status(module.status())),
        NexDeskRequest::Open => module
            .open()
            .map(|(pid, already_running)| NexDeskResponse::Opened {
                pid,
                already_running,
            })
            .map_err(|error| error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_open_needs_a_capability() {
        assert_eq!(NexDeskRequest::Status.required_capability(), None);
        assert_eq!(
            NexDeskRequest::Open.required_capability(),
            Some(Capability::ExecuteCommand)
        );
    }

    #[test]
    fn requests_and_responses_round_trip_as_json() {
        let request = serde_json::to_string(&NexDeskRequest::Open).unwrap();
        assert_eq!(
            serde_json::from_str::<NexDeskRequest>(&request).unwrap(),
            NexDeskRequest::Open
        );

        let response = NexDeskResponse::Opened {
            pid: 42,
            already_running: false,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert_eq!(
            serde_json::from_str::<NexDeskResponse>(&json).unwrap(),
            response
        );
    }

    #[test]
    fn open_reports_a_missing_install() {
        let module = NexDeskModule::with_binary(None);

        let error = handle(NexDeskRequest::Open, &module).unwrap_err();

        assert!(error.contains("not installed"), "{error}");
    }

    #[test]
    fn status_reports_a_missing_install() {
        let module = NexDeskModule::with_binary(None);

        let Ok(NexDeskResponse::Status(status)) = handle(NexDeskRequest::Status, &module) else {
            panic!("expected a status response");
        };

        assert!(!status.installed);
        assert_eq!(status.path, None);
        assert_eq!(status.running_pid, None);
    }
}
