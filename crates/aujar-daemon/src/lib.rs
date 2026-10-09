use anyhow::{Context, Result};
use aujar_core::{Health, WindowId};
use aujar_ipc::{
    ModuleInfo, ModulesResponse, PROTOCOL_VERSION, RenameResponse, Request, Response, SearchHit,
    SearchResponse,
};
use aujar_launcher::Launcher;
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

/// State shared by every client connection.
struct Shared {
    health: Health,
    registry: Arc<ModuleRegistry>,
    launcher: Launcher,
    owner_uid: u32,
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

    let health = Health {
        name: "Aujar".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        session,
    };

    let registry = initialize_runtime().await?;

    tracing::info!(
        socket = %socket_path.display(),
        modules = registry.len(),
        "Aujar daemon listening"
    );

    let shared = Arc::new(Shared {
        health,
        registry: Arc::clone(&registry),
        launcher: Launcher::with_default_providers(),
        owner_uid,
    });

    run_server(listener, shared).await?;

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

async fn run_server(listener: UnixListener, shared: Arc<Shared>) -> Result<()> {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("failed to install SIGTERM handler")?;

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let shared = Arc::clone(&shared);

                tokio::spawn(async move {
                    if let Err(error) = handle_client(stream, shared).await {
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

    tracing::debug!(
        modules = shared.registry.len(),
        "Aujar runtime shutdown requested"
    );

    Ok(())
}

async fn handle_client(stream: UnixStream, shared: Arc<Shared>) -> Result<()> {
    let peer = stream
        .peer_cred()
        .context("failed to read IPC peer credentials")?;

    if peer.uid() != shared.owner_uid && peer.uid() != 0 {
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
        dispatch(envelope.payload, &shared).await
    };

    let response = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        payload,
    })?;

    write_half.write_all(&response).await?;
    write_half.write_all(b"\n").await?;

    Ok(())
}

async fn dispatch(request: Request, shared: &Shared) -> Response {
    match request {
        Request::Ping => Response::Pong,

        Request::Health => Response::Health(shared.health.clone()),

        Request::Windows => Response::Windows(Vec::<WindowId>::new()),

        Request::Modules => Response::Modules(ModulesResponse {
            modules: shared
                .registry
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

        Request::Search(request) => {
            let limit = request.limit.unwrap_or(10).clamp(1, 50);

            let results = shared
                .launcher
                .search(&request.query, limit)
                .into_iter()
                .map(|result| SearchHit {
                    id: result.id,
                    title: result.title,
                    subtitle: result.subtitle,
                    kind: result.kind.as_str().to_owned(),
                    score: result.score,
                })
                .collect();

            Response::Search(SearchResponse { results })
        }

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
