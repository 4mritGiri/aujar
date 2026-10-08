use anyhow::{Context, Result};
use nexora_core::{Health, WindowId};
use nexora_ipc::{PROTOCOL_VERSION, RenameResponse, Request, Response};
use nexora_platform::detect_session;
use nexora_rename::{RenameOptions, RenamePlanner};
use nexora_runtime::{ModuleContext, ModuleRegistry};
use serde::{Deserialize, Serialize};
use std::{path::Path, sync::Arc};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

mod modules;

use modules::CoreModule;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope<T> {
    version: u16,
    payload: T,
}

pub async fn run(socket_path: impl AsRef<Path>) -> Result<()> {
    let socket_path = socket_path.as_ref();

    prepare_socket(socket_path).await?;

    let listener = UnixListener::bind(socket_path).context("failed to bind Nexora IPC socket")?;

    let session = detect_session();

    let health = Arc::new(Health {
        name: "Nexora".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        session,
    });

    let registry = initialize_runtime().await?;

    tracing::info!(
        socket = %socket_path.display(),
        modules = registry.len(),
        "Nexora daemon listening"
    );

    run_server(listener, health, &registry).await?;

    registry
        .stop_all()
        .await
        .context("failed to stop Nexora modules")?;

    remove_socket(socket_path).await?;

    tracing::info!("Nexora daemon stopped");

    Ok(())
}

async fn prepare_socket(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("failed to remove stale Nexora socket")?;
    }

    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create Nexora socket directory")?;
    }

    Ok(())
}

async fn remove_socket(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("failed to remove Nexora socket")?;
    }

    Ok(())
}

async fn initialize_runtime() -> Result<ModuleRegistry> {
    let mut registry = ModuleRegistry::new();

    registry
        .register(CoreModule)
        .context("failed to register Nexora core module")?;

    let context = ModuleContext::new(env!("CARGO_PKG_VERSION"));

    registry
        .start_all(context)
        .await
        .context("failed to start Nexora modules")?;

    Ok(registry)
}

async fn run_server(
    listener: UnixListener,
    health: Arc<Health>,
    registry: &ModuleRegistry,
) -> Result<()> {
    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let health = Arc::clone(&health);

                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, health).await {
                        tracing::warn!(%error, "IPC client failed");
                    }
                });
            }

            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for shutdown signal")?;

                tracing::info!("shutdown signal received");

                break;
            }
        }
    }

    tracing::debug!(
        modules = registry.len(),
        "Nexora runtime shutdown requested"
    );

    Ok(())
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
                            Err(error) => {
                                return Response::Error(error.to_string());
                            }
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
