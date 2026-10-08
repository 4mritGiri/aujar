use anyhow::{Context, Result};
use nexora_core::{Health, WindowId};
use nexora_ipc::{PROTOCOL_VERSION, RenameResponse, Request, Response};
use nexora_platform::detect_session;
use nexora_rename::{RenameOptions, RenamePlanner};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope<T> {
    version: u16,
    payload: T,
}

pub async fn run(socket_path: impl AsRef<Path>) -> Result<()> {
    let socket_path = socket_path.as_ref();

    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("failed to remove stale Nexora socket")?;
    }

    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let listener = UnixListener::bind(socket_path)?;
    let session = detect_session();
    let health = Arc::new(Health {
        name: "Nexora".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        session,
    });

    tracing::info!(socket = %socket_path.display(), "Nexora daemon listening");

    loop {
        let (stream, _) = listener.accept().await?;
        let health = Arc::clone(&health);
        tokio::spawn(async move {
            if let Err(error) = handle_client(stream, health).await {
                tracing::warn!(%error, "IPC client failed");
            }
        });
    }
}

async fn handle_client(stream: UnixStream, health: Arc<Health>) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    reader.read_line(&mut line).await?;
    let envelope: Envelope<Request> = serde_json::from_str(&line)?;

    let payload = if envelope.version != PROTOCOL_VERSION {
        Response::Error(format!("unsupported protocol version {}", envelope.version))
    } else {
        dispatch(envelope.payload, &health).await
    };

    let response = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        payload,
    })?;

    write_half.write_all(&response).await?;
    write_half.write_all(b"\n").await?;
    Ok(())
}

async fn dispatch(request: Request, health: &Health) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Health => Response::Health(health.clone()),
        Request::Windows => Response::Windows(Vec::<WindowId>::new()),
        Request::Rename(request) => {
            let options = RenameOptions {
                pattern: request.pattern,
                replacement: request.replacement,
                regex: request.regex,
                apply: request.apply,
            };

            match RenamePlanner::new(options).plan(&request.paths) {
                Ok(plan) => {
                    let applied = if request.apply {
                        match plan.execute() {
                            Ok(()) => true,
                            Err(error) => return Response::Error(error.to_string()),
                        }
                    } else {
                        false
                    };

                    Response::Rename(RenameResponse { plan, applied })
                }
                Err(error) => Response::Error(error.to_string()),
            }
        }
    }
}
