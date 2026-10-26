//! Serves `Request::MouseBorders` by talking to the Mouse Borders service.

use std::collections::HashSet;

use aujar_core::Capability;
use aujar_ipc::{MouseBordersRequest, Response};
use aujar_mouse_borders::{
    api,
    service::{MouseBordersModule, Profile},
};

/// The module for this user's session, ready to register with the runtime.
pub fn module(denied: &HashSet<Capability>) -> MouseBordersModule {
    MouseBordersModule::new(Profile::user(denied.clone()))
}

pub async fn handle(request: MouseBordersRequest, module: &MouseBordersModule) -> Response {
    let Some(service) = module.handle() else {
        return Response::Error("Mouse Borders has not started yet".into());
    };

    match api::handle(request, &service).await {
        Ok(response) => Response::MouseBorders(response),
        Err(message) => Response::Error(message),
    }
}
