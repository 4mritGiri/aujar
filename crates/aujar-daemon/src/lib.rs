use anyhow::{Context, Result};
use aujar_core::{Capability, Health, WindowId};
use aujar_hotkeys::{HotkeyEvent, ShortcutSpec};
use aujar_ipc::{
    ColorResponse, Envelope, Event, ExecuteResponse, LauncherCommand, ModuleInfo, ModulesResponse,
    PROTOCOL_VERSION, RenameResponse, Request, Response, SearchHit, SearchResponse, StatusResponse,
    encode_line,
};
use aujar_launcher::{LaunchSpec, Launcher};
use aujar_platform::detect_session;
use aujar_rename::{RenameOptions, RenamePlanner};
use aujar_runtime::{ModuleContext, ModuleRegistry};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::HashSet,
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        process::CommandExt,
    },
    path::Path,
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::Take,
    net::unix::{OwnedReadHalf, OwnedWriteHalf},
    sync::broadcast,
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

/// System policy file; a missing file means no restrictions.
const POLICY_PATH: &str = "/etc/aujar/policy.toml";

/// Maximum simultaneous event subscribers (UIs, scripts).
const MAX_SUBSCRIBERS: usize = 16;

/// Events buffered per slow subscriber before it starts missing events.
const EVENT_BUFFER: usize = 64;

use modules::CoreModule;

/// The launcher plus the settings file it was built from. The launcher is rebuilt when the
/// file changes, so toggling a plugin in the settings window needs no daemon restart.
struct LauncherState {
    launcher: Launcher,
    stamp: Option<std::time::SystemTime>,
}

fn settings_stamp() -> Option<std::time::SystemTime> {
    std::fs::metadata(aujar_config::Settings::path())
        .and_then(|meta| meta.modified())
        .ok()
}

fn launcher_options(settings: &aujar_config::Settings) -> aujar_launcher::LauncherOptions {
    aujar_launcher::LauncherOptions {
        applications: settings.plugins.applications,
        calculator: settings.plugins.calculator,
        units: settings.plugins.units,
        files: settings.plugins.files,
        system: settings.plugins.system,
        shell: settings.plugins.shell,
        file_roots: settings.files.roots.clone(),
        file_depth: settings.files.max_depth as usize,
    }
}

fn build_launcher() -> Launcher {
    let settings = aujar_config::Settings::load_or_default();

    Launcher::with_options(&launcher_options(&settings))
}

impl LauncherState {
    fn new() -> Self {
        Self {
            launcher: build_launcher(),
            stamp: settings_stamp(),
        }
    }
}

/// Run `f` with an up-to-date launcher.
fn with_launcher<R>(shared: &Shared, f: impl FnOnce(&Launcher) -> R) -> R {
    let mut state = shared
        .launcher
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let stamp = settings_stamp();

    if stamp != state.stamp {
        tracing::info!("settings changed; reloading launcher providers");

        state.launcher = build_launcher();
        state.stamp = stamp;
    }

    f(&state.launcher)
}

/// State shared by every client connection.
struct Shared {
    health: Health,
    registry: Arc<ModuleRegistry>,
    launcher: std::sync::Mutex<LauncherState>,
    owner_uid: u32,
    denied: HashSet<Capability>,
    events: broadcast::Sender<Event>,
    subscribers: AtomicUsize,
    hotkey_status: std::sync::Mutex<String>,
}

fn set_hotkey_status(shared: &Shared, status: String) {
    *shared
        .hotkey_status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = status;
}

/// Counts a subscriber for as long as it is alive.
struct SubscriberGuard<'a>(&'a AtomicUsize);

impl<'a> SubscriberGuard<'a> {
    fn acquire(counter: &'a AtomicUsize) -> Option<Self> {
        if counter.fetch_add(1, Ordering::SeqCst) >= MAX_SUBSCRIBERS {
            counter.fetch_sub(1, Ordering::SeqCst);
            None
        } else {
            Some(Self(counter))
        }
    }
}

impl Drop for SubscriberGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

fn load_denied_capabilities() -> Result<HashSet<Capability>> {
    let policy = aujar_config::Policy::load_optional(POLICY_PATH)
        .with_context(|| format!("failed to load policy {POLICY_PATH}"))?;

    policy
        .denied_capabilities
        .iter()
        .map(|name| {
            Capability::parse(name)
                .ok_or_else(|| anyhow::anyhow!("unknown capability `{name}` in {POLICY_PATH}"))
        })
        .collect()
}

const TOGGLE_LAUNCHER_SHORTCUT: &str = "toggle-launcher";

/// Ask the desktop portal for global shortcuts and turn presses into launcher events.
fn start_hotkeys(shared: Arc<Shared>) {
    if shared.denied.contains(&Capability::GlobalHotkeys) {
        tracing::info!("global hotkeys are disabled by policy");
        set_hotkey_status(&shared, "disabled by policy (global_hotkeys)".to_owned());

        return;
    }

    let mut events = aujar_hotkeys::start(vec![ShortcutSpec {
        id: TOGGLE_LAUNCHER_SHORTCUT,
        description: "Show or hide the Aujar launcher",
        preferred_trigger: "CTRL+ALT+space",
    }]);

    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                HotkeyEvent::Bound { id, trigger } => {
                    tracing::info!(%id, %trigger, "global shortcut bound");
                    set_hotkey_status(&shared, format!("bound: {id} = {trigger}"));
                }
                HotkeyEvent::Activated(id) if id == TOGGLE_LAUNCHER_SHORTCUT => {
                    let subscribers = shared
                        .events
                        .send(Event::Launcher(LauncherCommand::Toggle))
                        .unwrap_or(0);

                    tracing::info!(subscribers, "global shortcut pressed");
                }
                HotkeyEvent::Activated(id) => {
                    tracing::debug!(%id, "unknown global shortcut activated");
                }
                HotkeyEvent::Unavailable(reason) => {
                    set_hotkey_status(&shared, format!("unavailable: {reason}"));
                    tracing::warn!(
                        %reason,
                        "global shortcuts unavailable; bind `aujar launcher toggle` in your desktop settings instead"
                    );
                }
            }
        }
    });
}

fn spawn_detached(spec: &LaunchSpec) -> std::io::Result<Option<u32>> {
    let mut command = std::process::Command::new(&spec.program);

    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Own process group: stopping the daemon must not kill launched apps.
        .process_group(0);

    let mut child = tokio::process::Command::from(command).spawn()?;
    let pid = child.id();

    // Reap the child when it exits so it does not become a zombie.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    Ok(pid)
}

/// Daemon behaviour switches.
#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Register native global shortcuts through the desktop portal.
    pub hotkeys: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { hotkeys: true }
    }
}

pub async fn run(socket_path: impl AsRef<Path>) -> Result<()> {
    run_with(socket_path, Options::default()).await
}

/// Send `tracing` output to stderr (journald when run by systemd). `RUST_LOG` overrides the
/// default `info` level. Harmless if a subscriber is already installed (tests).
fn init_logging() {
    use std::io::IsTerminal;

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(std::io::stderr().is_terminal())
        .try_init();
}

pub async fn run_with(socket_path: impl AsRef<Path>, options: Options) -> Result<()> {
    init_logging();

    let socket_path = socket_path.as_ref();

    // Fail before touching the socket if the policy is invalid.
    let denied = load_denied_capabilities()?;

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
        launcher: std::sync::Mutex::new(LauncherState::new()),
        owner_uid,
        denied,
        events: broadcast::channel(EVENT_BUFFER).0,
        subscribers: AtomicUsize::new(0),
        hotkey_status: std::sync::Mutex::new(if options.hotkeys {
            "waiting for the desktop portal".to_owned()
        } else {
            "disabled by daemon option".to_owned()
        }),
    });

    if options.hotkeys {
        start_hotkeys(Arc::clone(&shared));
    }

    run_server(listener, Arc::clone(&shared)).await?;

    // Tell subscribers we are going away, and give their tasks a moment to write it.
    let _ = shared.events.send(Event::Shutdown);
    tokio::time::sleep(Duration::from_millis(100)).await;

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

    let envelope: Envelope<Request> = match serde_json::from_str(&line) {
        Ok(envelope) => envelope,
        Err(error) => {
            // Most likely a newer client talking to this daemon: say so instead of hanging up.
            let reply = Response::Error(format!(
                "unrecognised request (is the daemon older than the client?): {error}"
            ));
            write_half.write_all(&encode_line(reply)?).await?;

            return Ok(());
        }
    };

    if envelope.version == PROTOCOL_VERSION && matches!(envelope.payload, Request::Subscribe) {
        return stream_events(reader, write_half, &shared).await;
    }

    let payload = if envelope.version != PROTOCOL_VERSION {
        Response::Error(format!("unsupported protocol version {}", envelope.version))
    } else {
        dispatch(envelope.payload, &shared).await
    };

    write_half.write_all(&encode_line(payload)?).await?;

    Ok(())
}

/// Serve a `Subscribe` connection: acknowledge, then push events until the
/// client disconnects, the daemon shuts down, or the client misbehaves.
async fn stream_events(
    mut reader: BufReader<Take<OwnedReadHalf>>,
    mut writer: OwnedWriteHalf,
    shared: &Shared,
) -> Result<()> {
    let Some(_guard) = SubscriberGuard::acquire(&shared.subscribers) else {
        let error = Response::Error("too many event subscribers".into());
        writer.write_all(&encode_line(error)?).await?;

        return Ok(());
    };

    // Subscribe before acknowledging so no event can slip in between.
    let mut events = shared.events.subscribe();

    writer
        .write_all(&encode_line(Response::Subscribed)?)
        .await?;

    let mut ignored = String::new();

    loop {
        tokio::select! {
            received = events.recv() => match received {
                Ok(event) => {
                    writer.write_all(&encode_line(&event)?).await?;

                    if event == Event::Shutdown {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(skipped, "event subscriber lagged; events were dropped");
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },

            // Clients send nothing after Subscribe: EOF (or any data) ends the stream.
            _ = reader.read_line(&mut ignored) => break,
        }
    }

    Ok(())
}

async fn dispatch(request: Request, shared: &Shared) -> Response {
    if let Some(capability) = request
        .required_capability()
        .filter(|capability| shared.denied.contains(capability))
    {
        return Response::Error(format!("capability `{capability}` is denied by policy"));
    }

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

        Request::Status => Response::Status(StatusResponse {
            subscribers: shared.subscribers.load(Ordering::SeqCst),
            hotkeys: shared
                .hotkey_status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone(),
        }),

        Request::PickColor => match aujar_color::pick().await {
            Ok(color) => Response::Color(ColorResponse {
                hex: color.to_hex(),
                rgb: color.to_rgb_string(),
                hsl: color.to_hsl_string(),
            }),
            Err(error) => Response::Error(format!("{error:#}")),
        },

        Request::Subscribe => {
            Response::Error("Subscribe is handled by the connection layer".into())
        }

        Request::Launcher(command) => {
            let subscribers = shared.events.send(Event::Launcher(command)).unwrap_or(0);

            Response::Delivered { subscribers }
        }

        Request::Execute(request) => match with_launcher(shared, |launcher| launcher.resolve(&request.id)) {
            Ok(spec) => match spawn_detached(&spec) {
                Ok(pid) => {
                    tracing::info!(id = %request.id, program = %spec.program, ?pid, "launched application");

                    Response::Executed(ExecuteResponse {
                        title: spec.title,
                        pid,
                    })
                }
                Err(error) => {
                    Response::Error(format!("failed to launch `{}`: {error}", spec.title))
                }
            },
            Err(message) => Response::Error(message),
        },

        Request::Search(request) => {
            let limit = request.limit.unwrap_or(10).clamp(1, 50);

            let results = with_launcher(shared, |launcher| launcher.search(&request.query, limit))
                .into_iter()
                .map(|result| SearchHit {
                    id: result.id,
                    title: result.title,
                    subtitle: result.subtitle,
                    kind: result.kind.as_str().to_owned(),
                    score: result.score,
                    icon: result.icon,
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
