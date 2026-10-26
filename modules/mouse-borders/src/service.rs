//! Keeps the engine in step with the user's settings and security key.
//!
//! The supervisor watches `settings.toml` and the key file. When Mouse Borders is enabled and
//! everything needed is in place it runs an [`Engine`]; when settings change it restarts it;
//! when something is missing it says what (and keeps checking). It never fails to start for
//! configuration reasons: a daemon must not die because of a typo in a layout.
//!
//! The same supervisor runs inside the per-user daemon and, with a different [`Profile`], as
//! the system service.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use aujar_config::Settings;
use aujar_core::Capability;
use aujar_runtime::{Module, ModuleContext, ModuleMetadata, RuntimeError};
use tokio::{
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    audit::AuditSink,
    backend::Backends,
    config::EngineConfig,
    engine::{Command, Engine, EngineHandle},
    error::{Error, Result},
    keystore::KeyStore,
    secure::SecurityKey,
    status::Status,
};

/// How often settings and the key file are checked for changes.
const POLL: Duration = Duration::from_secs(2);

/// How often a failed start (port in use, no devices yet) is retried.
const RETRY: Duration = Duration::from_secs(10);

pub type BackendFactory = Arc<dyn Fn(&EngineConfig) -> Backends + Send + Sync>;

/// Where a supervisor reads its configuration and what it may do.
#[derive(Clone)]
pub struct Profile {
    pub settings_path: PathBuf,
    pub key_store: KeyStore,
    /// Where the audit log is written (when the settings switch auditing on).
    pub audit_log: PathBuf,
    /// Capabilities the system policy forbids.
    pub denied: HashSet<Capability>,
    /// Off in the system service: it has no desktop session to share a clipboard with.
    pub clipboard: bool,
    pub backends: BackendFactory,
}

impl Profile {
    /// The per-user profile: the user's settings file and its key file, on any platform.
    pub fn user(denied: HashSet<Capability>) -> Self {
        Self {
            settings_path: Settings::path(),
            key_store: KeyStore::user_default(),
            audit_log: crate::audit::user_log_path(),
            denied,
            clipboard: true,
            backends: Arc::new(crate::backend::platform_backends),
        }
    }

    /// The system-service profile: configuration under `/etc/aujar`, no clipboard.
    #[cfg(all(target_os = "linux", feature = "linux-backend"))]
    pub fn system(denied: HashSet<Capability>) -> Self {
        Self {
            settings_path: PathBuf::from("/etc/aujar/settings.toml"),
            key_store: KeyStore::new("/etc/aujar/mouse-borders.key"),
            audit_log: crate::audit::user_log_path(),
            denied,
            clipboard: false,
            backends: Arc::new(crate::backend::linux::backends),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceState {
    /// `mouse_borders.enabled` is false.
    Disabled,
    /// The system policy forbids it.
    BlockedByPolicy(String),
    /// Enabled, but no security key exists yet.
    WaitingForKey,
    /// Settings or key could not be used, or the engine could not start.
    Error(String),
    Running,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceReport {
    pub state: ServiceState,
    /// Present while the engine runs.
    pub engine: Option<Status>,
}

impl ServiceReport {
    fn new(state: ServiceState) -> Self {
        Self {
            state,
            engine: None,
        }
    }
}

enum Request {
    Engine {
        command: Command,
        reply: oneshot::Sender<Result<String>>,
    },
    Reload,
}

/// What the running engine was started from, to notice when it needs restarting.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    settings: Option<(SystemTime, u64)>,
    key: Option<(SystemTime, u64)>,
}

fn stamp(path: &std::path::Path) -> Option<(SystemTime, u64)> {
    std::fs::metadata(path)
        .ok()
        .and_then(|meta| Some((meta.modified().ok()?, meta.len())))
}

enum Desired {
    Off(ServiceState),
    On(Box<EngineConfig>, SecurityKey),
}

fn evaluate(profile: &Profile) -> Desired {
    let settings = match Settings::load_from(&profile.settings_path) {
        Ok(settings) => settings.mouse_borders,
        Err(error) => {
            return Desired::Off(ServiceState::Error(format!(
                "cannot read {}: {error}",
                profile.settings_path.display()
            )));
        }
    };

    if !settings.enabled {
        return Desired::Off(ServiceState::Disabled);
    }

    for capability in [Capability::ControlInput, Capability::Network] {
        if profile.denied.contains(&capability) {
            return Desired::Off(ServiceState::BlockedByPolicy(format!(
                "capability `{capability}` is denied by policy"
            )));
        }
    }

    let mut config = match EngineConfig::from_settings(&settings) {
        Ok(config) => config,
        Err(error) => return Desired::Off(ServiceState::Error(error.to_string())),
    };

    config.audit_log = settings.audit_log.then(|| profile.audit_log.clone());
    config.share_clipboard &= profile.clipboard
        && !profile.denied.contains(&Capability::ReadClipboard)
        && !profile.denied.contains(&Capability::WriteClipboard);
    config.transfer_files &= !profile.denied.contains(&Capability::FileSystem);

    match profile.key_store.load() {
        Ok(Some(key)) => Desired::On(Box::new(config), key),
        Ok(None) => Desired::Off(ServiceState::WaitingForKey),
        Err(error) => Desired::Off(ServiceState::Error(error.to_string())),
    }
}

/// Cloneable access to a running supervisor.
#[derive(Clone)]
pub struct ServiceHandle {
    report: watch::Receiver<ServiceReport>,
    requests: mpsc::Sender<Request>,
    key_store: KeyStore,
    settings_path: PathBuf,
    audit_log: PathBuf,
}

impl ServiceHandle {
    pub fn report(&self) -> ServiceReport {
        self.report.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<ServiceReport> {
        self.report.clone()
    }

    pub fn key_store(&self) -> &KeyStore {
        &self.key_store
    }

    /// Run a command on the engine, if it is running.
    pub async fn request(&self, command: Command) -> Result<String> {
        if !matches!(self.report().state, ServiceState::Running) {
            return Err(Error::Other(
                "Mouse Borders is not running (see `aujar mwb status`)".into(),
            ));
        }

        let (reply, answer) = oneshot::channel();

        self.requests
            .send(Request::Engine { command, reply })
            .await
            .map_err(|_| Error::Other("Mouse Borders has stopped".into()))?;

        answer
            .await
            .map_err(|_| Error::Other("Mouse Borders stopped before answering".into()))?
    }

    /// Re-read settings and the key now instead of at the next poll.
    pub async fn reload(&self) {
        let _ = self.requests.send(Request::Reload).await;
    }

    /// Create a new key. Pass `force` to replace an existing one (this disconnects every
    /// machine paired with the old key).
    pub async fn generate_key(&self, force: bool) -> Result<SecurityKey> {
        let replacing = matches!(self.key_store.load(), Ok(Some(_)));
        let key = self.key_store.generate(force)?;

        self.audit_key_change(if replacing { "replaced" } else { "generated" });
        self.reload().await;

        Ok(key)
    }

    /// Store a key copied from another machine.
    pub async fn set_key(&self, text: &str, force: bool) -> Result<()> {
        let key = SecurityKey::parse(text)?;

        self.key_store.save(&key, force)?;
        self.audit_key_change("set from another machine");
        self.reload().await;

        Ok(())
    }

    fn audit_key_change(&self, how: &str) {
        // Key changes can happen while the engine is off, so write to the log directly.
        if let Ok(settings) = Settings::load_from(&self.settings_path)
            && settings.mouse_borders.audit_log
        {
            crate::audit::FileAudit::new(
                crate::settings_edit::local_name(&settings.mouse_borders).to_string(),
                &self.audit_log,
            )
            .record(crate::audit::AuditEvent::KeyChanged {
                how: how.to_owned(),
            });
        }
    }

    pub fn show_key(&self) -> Result<Option<SecurityKey>> {
        self.key_store.load()
    }
}

pub struct Supervisor {
    handle: ServiceHandle,
    stop: watch::Sender<bool>,
    task: tokio::sync::Mutex<Option<JoinHandle<()>>>,
}

impl Supervisor {
    pub fn spawn(profile: Profile) -> Self {
        let (report_tx, report_rx) = watch::channel(ServiceReport::new(ServiceState::Disabled));
        let (requests_tx, requests_rx) = mpsc::channel(16);
        let (stop_tx, stop_rx) = watch::channel(false);

        let handle = ServiceHandle {
            report: report_rx,
            requests: requests_tx,
            key_store: profile.key_store.clone(),
            settings_path: profile.settings_path.clone(),
            audit_log: profile.audit_log.clone(),
        };

        let task = tokio::spawn(supervise(profile, report_tx, requests_rx, stop_rx));

        Self {
            handle,
            stop: stop_tx,
            task: tokio::sync::Mutex::new(Some(task)),
        }
    }

    pub fn handle(&self) -> ServiceHandle {
        self.handle.clone()
    }

    /// Stop the engine and the supervisor.
    pub async fn shutdown(&self) {
        let _ = self.stop.send(true);

        if let Some(task) = self.task.lock().await.take() {
            let _ = timeout(Duration::from_secs(5), task).await;
        }
    }
}

async fn supervise(
    profile: Profile,
    report: watch::Sender<ServiceReport>,
    mut requests: mpsc::Receiver<Request>,
    mut stop: watch::Receiver<bool>,
) {
    let mut running: Option<(EngineHandle, Fingerprint)> = None;
    let mut last_checked: Option<Fingerprint> = None;
    let mut failed_at: Option<tokio::time::Instant> = None;
    let mut force = true;
    let mut engine_status: Option<watch::Receiver<Status>> = None;

    loop {
        let now_fingerprint = Fingerprint {
            settings: stamp(&profile.settings_path),
            key: stamp(profile.key_store.path()),
        };
        let retry_due = failed_at.is_some_and(|at| at.elapsed() >= RETRY);
        let changed = last_checked.as_ref() != Some(&now_fingerprint);

        if force || changed || retry_due {
            force = false;
            failed_at = None;
            last_checked = Some(now_fingerprint.clone());

            // Anything that changed means a fresh start; an unchanged, healthy engine stays.
            if let Some((engine, _)) = running.take() {
                engine.stop().await;
                engine_status = None;
            }

            match evaluate(&profile) {
                Desired::Off(state) => {
                    report.send_replace(ServiceReport::new(state));
                }
                Desired::On(config, key) => {
                    let backends = (profile.backends)(&config);

                    match Engine::start(*config, key, backends).await {
                        Ok(engine) => {
                            engine_status = Some(engine.subscribe());
                            running = Some((engine, now_fingerprint));
                        }
                        Err(error) => {
                            failed_at = Some(tokio::time::Instant::now());
                            report.send_replace(ServiceReport::new(ServiceState::Error(
                                error.to_string(),
                            )));
                        }
                    }
                }
            }
        }

        if let Some(status) = &engine_status {
            report.send_replace(ServiceReport {
                state: ServiceState::Running,
                engine: Some(status.borrow().clone()),
            });
        }

        tokio::select! {
            _ = stop.changed() => break,

            _ = tokio::time::sleep(POLL) => {}

            // Keep the published report fresh while the engine runs.
            _ = async {
                match engine_status.as_mut() {
                    Some(status) => { let _ = status.changed().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {}

            request = requests.recv() => match request {
                Some(Request::Reload) => force = true,
                Some(Request::Engine { command, reply }) => {
                    let result = match &running {
                        Some((engine, _)) => engine.request(command).await,
                        None => Err(Error::Other("Mouse Borders is not running".into())),
                    };

                    let _ = reply.send(result);
                }
                None => break,
            },
        }
    }

    if let Some((engine, _)) = running {
        engine.stop().await;
    }

    report.send_replace(ServiceReport::new(ServiceState::Disabled));
}

static METADATA: ModuleMetadata = ModuleMetadata::new(
    "mouse-borders",
    "Mouse Borders",
    env!("CARGO_PKG_VERSION"),
    "Share one keyboard, mouse, clipboard and files between up to four Linux machines",
    &[
        Capability::ControlInput,
        Capability::Network,
        Capability::ReadClipboard,
        Capability::WriteClipboard,
        Capability::FileSystem,
    ],
);

struct ModuleInner {
    profile: Profile,
    supervisor: tokio::sync::Mutex<Option<Arc<Supervisor>>>,
    handle: std::sync::Mutex<Option<ServiceHandle>>,
}

/// The runtime [`Module`] wrapping a supervisor. Clones share state, so the daemon can
/// register one and keep another to reach the running service.
#[derive(Clone)]
pub struct MouseBordersModule {
    inner: Arc<ModuleInner>,
}

impl MouseBordersModule {
    pub fn new(profile: Profile) -> Self {
        Self {
            inner: Arc::new(ModuleInner {
                profile,
                supervisor: tokio::sync::Mutex::new(None),
                handle: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Access for IPC handlers; `None` until the module has started.
    pub fn handle(&self) -> Option<ServiceHandle> {
        self.inner.handle.lock().unwrap().clone()
    }
}

#[async_trait]
impl Module for MouseBordersModule {
    fn metadata(&self) -> &'static ModuleMetadata {
        &METADATA
    }

    async fn start(&self, _context: ModuleContext) -> std::result::Result<(), RuntimeError> {
        let mut slot = self.inner.supervisor.lock().await;

        if slot.is_none() {
            let supervisor = Arc::new(Supervisor::spawn(self.inner.profile.clone()));

            *self.inner.handle.lock().unwrap() = Some(supervisor.handle());
            *slot = Some(supervisor);
        }

        Ok(())
    }

    async fn stop(&self) -> std::result::Result<(), RuntimeError> {
        if let Some(supervisor) = self.inner.supervisor.lock().await.take() {
            supervisor.shutdown().await;
        }

        Ok(())
    }
}
