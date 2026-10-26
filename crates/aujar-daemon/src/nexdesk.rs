//! Serves `Request::NexDesk` by asking the NexDesk module.

use aujar_ipc::{NexDeskRequest, Response};
use aujar_nexdesk::{NexDeskModule, api};

pub fn handle(request: NexDeskRequest, module: &NexDeskModule) -> Response {
    match api::handle(request, module) {
        Ok(response) => Response::NexDesk(response),
        Err(message) => Response::Error(message),
    }
}
