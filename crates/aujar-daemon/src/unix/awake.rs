//! Serves `Request::Awake` by asking the Awake module, and keeps its clock ticking.

use std::{sync::Arc, time::Duration};

use aujar_awake::{AwakeModule, api};
use aujar_ipc::{AwakeRequest, Response};

pub async fn handle(request: AwakeRequest, module: &AwakeModule) -> Response {
    match api::handle(request, module).await {
        Ok(response) => Response::Awake(response),
        Err(message) => Response::Error(message),
    }
}

/// Pick up the mode saved by the last run, then end timed modes, follow the settings and retry
/// failures once a second for as long as the daemon runs. Nothing is started when the policy
/// forbids keeping the computer awake.
pub async fn start(module: &Arc<AwakeModule>, allowed: bool) {
    if !allowed {
        tracing::info!("Awake is not started: the policy denies inhibit_sleep");

        return;
    }

    module.start().await;

    let module = Arc::clone(module);

    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));

        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tick.tick().await;
            module.tick().await;
        }
    });
}
