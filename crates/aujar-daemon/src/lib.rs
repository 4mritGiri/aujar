use anyhow::{Context, Result};
use aujar_core::{Health, WindowId};
use aujar_ipc::{ModuleInfo, ModulesResponse, PROTOCOL_VERSION, RenameResponse, Request, Response};
use aujar_platform::detect_session;
use aujar_rename::{RenameOptions, RenamePlanner};
use aujar_runtime::{ModuleContext, ModuleRegistry};
use serde::{Deserialize, Serialize};
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

mod modules;

/// Maximum size of a single IPC request line.
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Maximum time a client may take to send its request line.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

use modules::CoreModule;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope<T> {
    version: u16,
    payload: T,
}

pub async fn run(socket_path: impl AsRef<Path>) -> Result<()> {
    let socket_path = socket_path.as_ref();

    prepare_socket(socket_path).await?;

    let listener = UnixListener::bind(socket_path).context("failed to bind Aujar IPC socket")?;

    // Only the owning user may talk to the daemon.
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
        .context("failed to restrict Aujar socket permissions")?;

    // Only this user (and root) may use the daemon.
    let owner_uid = std::fs::metadata("/proc/self")
        .context("failed to determine daemon uid")?
        .uid();

    let session = detect_session();

    let health = Arc::new(Health {
        name: "Aujar".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        session,
    });

    let registry = initialize_runtime().await?;

    tracing::info!(
        socket = %socket_path.display(),
        modules = registry.len(),
        "Aujar daemon listening"
    );

    run_server(listener, health, Arc::clone(&registry), owner_uid).await?;

    registry
        .stop_all()
        .await
        .context("failed to stop Aujar modules")?;

    remove_socket(socket_path).await?;

    tracing::info!("Aujar daemon stopped");

    Ok(())
}

async fn prepare_socket(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("failed to remove stale Aujar socket")?;
    }

    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create Aujar socket directory")?;
    }

    Ok(())
}

async fn remove_socket(socket_path: &Path) -> Result<()> {
    if socket_path.exists() {
        tokio::fs::remove_file(socket_path)
            .await
            .context("failed to remove Aujar socket")?;
    }

    Ok(())
}

async fn initialize_runtime() -> Result<Arc<ModuleRegistry>> {
    let mut registry = ModuleRegistry::new();

    registry
        .register(CoreModule)
        .context("failed to register Aujar core module")?;

    let context = ModuleContext::new(env!("CARGO_PKG_VERSION"));

    registry
        .start_all(context)
        .await
        .context("failed to start Aujar modules")?;

    Ok(Arc::new(registry))
}

async fn run_server(
    listener: UnixListener,
    health: Arc<Health>,
    registry: Arc<ModuleRegistry>,
    owner_uid: u32,
) -> Result<()> {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("failed to install SIGTERM handler")?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let health = Arc::clone(&health);
                let registry = Arc::clone(&registry);

                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, health, registry, owner_uid).await {
                        tracing::warn!(%error, "IPC client failed");
                    }
                });
            }

            result = tokio::signal::ctrl_c() => {
                result.context("failed to listen for shutdown signal")?;

                tracing::info!("shutdown signal received");

                break;
            }

            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received");

                break;
            }
        }
    }

    tracing::debug!(modules = registry.len(), "Aujar runtime shutdown requested");

    Ok(())
}

async fn handle_client(
    stream: UnixStream,
    health: Arc<Health>,
    registry: Arc<ModuleRegistry>,
    owner_uid: u32,
) -> Result<()> {
    let peer = stream
        .peer_cred()
        .context("failed to read IPC peer credentials")?;

    if peer.uid() != owner_uid && peer.uid() != 0 {
        anyhow::bail!("rejected IPC client with uid {}", peer.uid());
    }

    let (read_half, mut write_half) = stream.into_split();

    let mut reader = BufReader::new(read_half.take(MAX_REQUEST_BYTES));
    let mut line = String::new();

    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line))
        .await
        .context("timed out reading IPC request")??;

    let envelope: Envelope<Request> = serde_json::from_str(&line)?;

    let payload = if envelope.version != PROTOCOL_VERSION {
        Response::Error(format!("unsupported protocol version {}", envelope.version))
    } else {
        dispatch(envelope.payload, &health, &registry).await
    };

    let response = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        payload,
    })?;

    write_half.write_all(&response).await?;
    write_half.write_all(b"\n").await?;

    Ok(())
}

async fn dispatch(request: Request, health: &Health, registry: &ModuleRegistry) -> Response {
    match request {
        Request::Ping => Response::Pong,

        Request::Health => Response::Health(health.clone()),

        Request::Windows => Response::Windows(Vec::<WindowId>::new()),

        Request::Modules => Response::Modules(ModulesResponse {
            modules: registry
                .metadata()
                .into_iter()
                .map(|module| ModuleInfo {
                    id: module.id.to_owned(),
                    name: module.name.to_owned(),
                    version: module.version.to_owned(),
                    api_version: module.api_version,
                    description: module.description.to_owned(),
                    capabilities: module
                        .capabilities
                        .iter()
                        .map(|capability| capability.as_str().to_owned())
                        .collect(),
                })
                .collect(),
        }),

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
