//! The engine: one task owning all mutable state.
//!
//! Local input, peer messages, commands and timers all arrive as [`Event`]s on one channel
//! and are handled one at a time, so the router, the injector and the peer table need no
//! locks. Network and device I/O live in their own tasks and only talk to the actor through
//! events.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use aujar_core::{Position, Size};
use tokio::{
    net::TcpListener,
    sync::{Notify, mpsc, oneshot, watch},
    task::JoinHandle,
    time::timeout,
};

use crate::{
    audit::{AuditEvent, AuditSink, FileAudit, NullAudit},
    backend::{
        Backends, CapturedInput, Clipboard, ClipboardContent, Desktop, EdgeControl, EdgeEvent,
        EdgeHit, Injector, InputSource, ScreenLocker,
    },
    clipboard::{self, Assembler},
    config::EngineConfig,
    error::{Error, Result},
    input::Button,
    keys::{HotkeyAction, KeyboardTracker},
    layout::{Edge, MachineName},
    link::{self, PeerLink},
    protocol::{
        ClipboardPacket, Features, FileMessage, Hello, InputEvent, Message, PROTOCOL_VERSION,
    },
    router::{Focus, RemoteStep, Router, Transfer},
    secure::SecurityKey,
    status::{LayoutLink, PeerStatus, Status},
    transfer::{Outgoing, Receiver},
};

/// Input from the user in the last minute counts as "working" for keep-awake purposes.
const ACTIVE_WINDOW: Duration = Duration::from_secs(60);

/// At most one idle-timer nudge from peers per this interval.
const NUDGE_INTERVAL: Duration = Duration::from_secs(10);

const GRAB_TIMEOUT: Duration = Duration::from_secs(2);

/// While another machine controls this one, screen-edge capture (Wayland) is only on for this
/// long after the user touched this machine's own mouse or keyboard.
const OWN_INPUT_WINDOW: Duration = Duration::from_secs(2);

/// How often the request to hide the pointer is repeated while away (see `hide_pointer`).
const POINTER_HIDE_RENEWAL: Duration = Duration::from_secs(20);

/// How long a switch waits for held keys and mouse buttons to be let go before it grabs the
/// input anyway (see `Engine::acquire_grab`).
const GRAB_RELEASE_WAIT: Duration = Duration::from_secs(2);

const MAX_OUTGOING_TRANSFERS: usize = 32;

const FALLBACK_SCREEN: Size = Size {
    width: 1920,
    height: 1080,
};

/// Something the user (or a script, through the daemon) asks the engine to do.
#[derive(Debug, Clone)]
pub enum Command {
    /// Give the keyboard and mouse to a machine; `None` means this machine.
    SwitchTo(Option<MachineName>),
    SetEasyMouse(bool),
    LockAll,
    /// Drop every connection and dial again immediately.
    Reconnect,
    /// Send the same input to every machine at once.
    MultiMachineInput(bool),
    SendFiles {
        machine: MachineName,
        paths: Vec<PathBuf>,
    },
}

pub(crate) struct PeerUp {
    pub name: MachineName,
    pub conn_id: u64,
    /// The machine that opened this connection.
    pub initiator: MachineName,
    pub hello: Hello,
    pub link: PeerLink,
    pub remote: Option<SocketAddr>,
}

pub(crate) enum Event {
    Local(CapturedInput),
    SourceFailed(String),
    PeerUp(PeerUp),
    Peer {
        name: MachineName,
        conn_id: u64,
        message: Message,
    },
    PeerDown {
        name: MachineName,
        conn_id: u64,
        error: Option<String>,
    },
    DialFailed {
        name: MachineName,
        error: String,
    },
    ClipboardChanged(ClipboardContent),
    /// The desktop reports the pointer running into a screen edge (Wayland).
    Edge(EdgeEvent),
    Command {
        command: Command,
        reply: oneshot::Sender<Result<String>>,
    },
    Note(String),
    Tick,
}

/// State shared with the connection tasks (kept deliberately small).
pub(crate) struct Shared {
    pub config: Arc<EngineConfig>,
    pub key: SecurityKey,
    pub audit: Arc<dyn AuditSink>,
    pub reconnect: Notify,
    connected: Mutex<std::collections::BTreeSet<MachineName>>,
    hello: Mutex<Hello>,
    next_connection: AtomicU64,
}

impl Shared {
    pub fn is_connected(&self, name: &MachineName) -> bool {
        self.connected.lock().unwrap().contains(name)
    }

    fn set_connected(&self, name: &MachineName, connected: bool) {
        let mut set = self.connected.lock().unwrap();

        if connected {
            set.insert(name.clone());
        } else {
            set.remove(name);
        }
    }

    fn any_connected(&self) -> bool {
        !self.connected.lock().unwrap().is_empty()
    }

    pub fn hello(&self) -> Hello {
        self.hello.lock().unwrap().clone()
    }

    fn set_screen(&self, screen: Size) {
        self.hello.lock().unwrap().screen = screen;
    }

    pub fn next_connection_id(&self) -> u64 {
        self.next_connection.fetch_add(1, Ordering::Relaxed)
    }
}

struct GrabRequest {
    grabbed: bool,
    reply: oneshot::Sender<Result<()>>,
}

/// Control of a running engine.
pub struct EngineHandle {
    events: mpsc::Sender<Event>,
    status: watch::Receiver<Status>,
    shutdown: watch::Sender<bool>,
    local_addr: SocketAddr,
    tasks: Vec<JoinHandle<()>>,
    actor: JoinHandle<()>,
}

impl EngineHandle {
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }

    /// Address the engine is listening on (useful when configured with port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Run a command and wait for its outcome (a message for the user, or an error).
    pub async fn request(&self, command: Command) -> Result<String> {
        let (reply, answer) = oneshot::channel();

        self.events
            .send(Event::Command { command, reply })
            .await
            .map_err(|_| Error::Other("the engine has stopped".into()))?;

        answer
            .await
            .map_err(|_| Error::Other("the engine stopped before answering".into()))?
    }

    /// Stop everything: release input, tell peers goodbye, close the listener.
    pub async fn stop(self) {
        let _ = self.shutdown.send(true);
        let _ = timeout(Duration::from_secs(3), self.actor).await;

        for task in self.tasks {
            task.abort();
        }
    }
}

pub struct Engine;

impl Engine {
    pub async fn start(
        config: EngineConfig,
        key: SecurityKey,
        backends: Backends,
    ) -> Result<EngineHandle> {
        let config = Arc::new(config);
        let Backends {
            edges,
            source,
            sink,
            desktop,
            clipboard,
            locker,
            notes: backend_notes,
        } = backends;
        let desktop: Arc<dyn Desktop> = Arc::from(desktop);
        let clipboard: Option<Arc<dyn Clipboard>> = clipboard.map(Arc::from);
        let locker: Arc<dyn ScreenLocker> = Arc::from(locker);

        let mut notes = backend_notes;
        let screen = match desktop.screen_size() {
            Ok(size) => size,
            Err(error) => {
                let size = config.screen_override.unwrap_or(FALLBACK_SCREEN);
                notes.push(format!(
                    "cannot detect the screen size ({error}); using {}x{} \
                     (set `mouse_borders.screen` to override)",
                    size.width, size.height
                ));

                size
            }
        };

        let edge_unavailable = desktop.pointer().err().map(|error| error.reason());

        let listener = TcpListener::bind(config.listen).await.map_err(|error| {
            Error::Other(format!(
                "cannot listen on {}: {error} (is another Aujar instance running?)",
                config.listen
            ))
        })?;
        let local_addr = listener.local_addr()?;

        let audit: Arc<dyn AuditSink> = match &config.audit_log {
            Some(path) => Arc::new(FileAudit::new(config.local.to_string(), path)),
            None => Arc::new(NullAudit),
        };
        audit.record(AuditEvent::ServiceStarted {
            listen: local_addr.to_string(),
        });

        let shared = Arc::new(Shared {
            config: Arc::clone(&config),
            key,
            audit,
            reconnect: Notify::new(),
            connected: Mutex::default(),
            hello: Mutex::new(Hello {
                version: PROTOCOL_VERSION,
                machine: config.local.clone(),
                screen,
                features: Features {
                    clipboard: config.share_clipboard && clipboard.is_some(),
                    files: config.transfer_files,
                },
            }),
            next_connection: AtomicU64::new(1),
        });

        let (events_tx, events_rx) = mpsc::channel::<Event>(1024);
        let (grab_tx, grab_rx) = mpsc::channel::<GrabRequest>(8);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let mut tasks = Vec::new();

        tasks.push(tokio::spawn(input_task(
            source,
            events_tx.clone(),
            grab_rx,
            shutdown_rx.clone(),
        )));
        tasks.push(tokio::spawn(link::run_listener(
            listener,
            Arc::clone(&shared),
            events_tx.clone(),
            shutdown_rx.clone(),
        )));

        for (name, address) in &config.peers {
            tasks.push(tokio::spawn(link::run_dialer(
                name.clone(),
                address.clone(),
                Arc::clone(&shared),
                events_tx.clone(),
                shutdown_rx.clone(),
            )));
        }

        // Some sessions (GNOME on Wayland) show every clipboard read on screen, so there nothing
        // watches: the clipboard is read when control moves instead (`refresh_local_clip`).
        if let (true, Some(clipboard)) = (config.share_clipboard, clipboard.as_ref())
            && clipboard.polls_in_background()
        {
            tasks.push(tokio::spawn(clipboard_watcher(
                Arc::clone(clipboard),
                Arc::clone(&shared),
                events_tx.clone(),
                shutdown_rx.clone(),
            )));
        }

        // Where the pointer position cannot be read (Wayland), the desktop tells us when the
        // pointer runs into an edge instead.
        let edge_control = edges.map(|watcher| {
            let (edge_tx, mut edge_rx) = mpsc::channel::<EdgeEvent>(32);
            let (control_tx, control_rx) = mpsc::unbounded_channel::<EdgeControl>();
            let relay = events_tx.clone();

            tasks.push(tokio::spawn(watcher.run(edge_tx, control_rx)));
            tasks.push(tokio::spawn(async move {
                while let Some(event) = edge_rx.recv().await {
                    if relay.send(Event::Edge(event)).await.is_err() {
                        break;
                    }
                }
            }));

            control_tx
        });

        tasks.push(tokio::spawn(ticker(events_tx.clone(), shutdown_rx.clone())));

        let mut status = Status::stopped();
        status.running = true;
        status.local = config.local.to_string();
        status.focus = config.local.to_string();
        status.listening = Some(local_addr);
        status.edge_switching_unavailable = edge_unavailable;
        status.easy_mouse = config.policy.easy_mouse;

        for note in notes {
            status.push_log(note);
        }

        let (status_tx, status_rx) = watch::channel(status.clone());

        let now = Instant::now();
        let mut actor = Actor {
            router: Router::new(
                config.layout.clone(),
                config.local.clone(),
                screen,
                config.policy,
            ),
            injector: Injector::new(sink),
            files: Receiver::new(config.receive_dir.clone(), config.max_file_bytes),
            saved_easy_mouse: config.policy.easy_mouse,
            cfg: config,
            shared,
            desktop,
            clipboard,
            locker,
            grab: grab_tx,
            peers: BTreeMap::new(),
            keyboard: KeyboardTracker::default(),
            held_buttons: BTreeSet::new(),
            grab_wait: None,
            pointer_hidden_at: None,
            edge_control,
            edge_watch_ready: false,
            watched_edges: Vec::new(),
            controlled_by: None,
            broadcast_from: None,
            broadcasting: false,
            pending_request: None,
            grabbed: false,
            status,
            status_tx,
            outgoing: BTreeMap::new(),
            assemblers: BTreeMap::new(),
            local_clip: None,
            applied_clip: None,
            sent_clip: BTreeMap::new(),
            last_error: BTreeMap::new(),
            last_local_input: None,
            next_keep_awake: now,
            next_screen_check: now,
            last_nudge: None,
            events: events_tx.clone(),
            dead: Vec::new(),
        };

        // Complete from the first moment: callers read the status right after `start`.
        actor.publish();

        let actor = tokio::spawn(actor.run(events_rx, shutdown_rx));

        Ok(EngineHandle {
            events: events_tx,
            status: status_rx,
            shutdown: shutdown_tx,
            local_addr,
            tasks,
            actor,
        })
    }
}

async fn ticker(events: mpsc::Sender<Event>, mut shutdown: watch::Receiver<bool>) {
    let mut interval = tokio::time::interval(Duration::from_secs(1));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                if events.send(Event::Tick).await.is_err() {
                    return;
                }
            }
            _ = shutdown.changed() => return,
        }
    }
}

/// Reads local input and applies grab requests. Runs in its own task so a blocked device
/// read never delays the actor.
async fn input_task(
    mut source: Box<dyn InputSource>,
    events: mpsc::Sender<Event>,
    mut grabs: mpsc::Receiver<GrabRequest>,
    mut shutdown: watch::Receiver<bool>,
) {
    let failure = loop {
        tokio::select! {
            biased;

            _ = shutdown.changed() => {
                let _ = source.set_grab(false);

                return;
            }

            Some(request) = grabs.recv() => {
                let _ = request.reply.send(source.set_grab(request.grabbed));
            }

            next = source.next() => match next {
                Ok(Some(input)) => {
                    if events.send(Event::Local(input)).await.is_err() {
                        return;
                    }
                }
                Ok(None) => break "the input source closed".to_owned(),
                Err(error) => break error.reason(),
            },
        }
    };

    let _ = events.send(Event::SourceFailed(failure.clone())).await;

    // Capture is gone, but keep answering grab requests so the actor never waits on us.
    loop {
        tokio::select! {
            _ = shutdown.changed() => return,
            request = grabs.recv() => match request {
                Some(request) => {
                    let _ = request.reply.send(Err(Error::unsupported(
                        "input capture",
                        &failure,
                    )));
                }
                None => return,
            },
        }
    }
}

async fn clipboard_watcher(
    clipboard: Arc<dyn Clipboard>,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
    mut shutdown: watch::Receiver<bool>,
) {
    let poll = shared.config.timing.clipboard_poll;
    let mut last: Option<[u8; 32]> = None;
    let mut last_token: Option<u64> = None;
    let mut warned = false;

    loop {
        tokio::select! {
            _ = tokio::time::sleep(poll) => {}
            _ = shutdown.changed() => return,
        }

        // Nobody to share with: do not even look at the clipboard.
        if !shared.any_connected() {
            continue;
        }

        // The OS can tell us nothing changed: skip the (possibly expensive) read.
        if let Some(token) = clipboard.change_token() {
            if last_token == Some(token) {
                continue;
            }

            last_token = Some(token);
        }

        match clipboard.read().await {
            Ok(Some(content)) => {
                let fingerprint = clipboard::fingerprint(&content);

                if last != Some(fingerprint) {
                    last = Some(fingerprint);

                    if events.send(Event::ClipboardChanged(content)).await.is_err() {
                        return;
                    }
                }
            }
            Ok(None) => {}
            Err(error) if !warned => {
                warned = true;
                let _ = events
                    .send(Event::Note(format!(
                        "clipboard sharing is not working: {error}"
                    )))
                    .await;
            }
            Err(_) => {}
        }
    }
}

fn random_id() -> u64 {
    let mut bytes = [0u8; 8];
    let _ = getrandom::fill(&mut bytes);

    u64::from_le_bytes(bytes)
}

struct PeerState {
    conn_id: u64,
    initiator: MachineName,
    link: PeerLink,
    features: Features,
    remote: Option<SocketAddr>,
}

struct Actor {
    cfg: Arc<EngineConfig>,
    shared: Arc<Shared>,
    router: Router,
    injector: Injector,
    desktop: Arc<dyn Desktop>,
    clipboard: Option<Arc<dyn Clipboard>>,
    locker: Arc<dyn ScreenLocker>,
    grab: mpsc::Sender<GrabRequest>,
    peers: BTreeMap<MachineName, PeerState>,
    keyboard: KeyboardTracker,
    /// Mouse buttons that are physically down.
    held_buttons: BTreeSet<Button>,
    /// Set while a switch waits for held keys and buttons to be let go before the input is
    /// grabbed: the time after which it stops waiting.
    grab_wait: Option<Instant>,
    /// When the request to hide the pointer was last made, while it is hidden.
    pointer_hidden_at: Option<Instant>,
    /// Where the desktop reports the pointer running into an edge instead of the engine reading
    /// the pointer position (Wayland): the way to talk to it.
    edge_control: Option<mpsc::UnboundedSender<EdgeControl>>,
    /// The desktop's edge reports work, so the pointer position is not needed.
    edge_watch_ready: bool,
    /// The edges the desktop was last asked to report.
    watched_edges: Vec<Edge>,
    /// The machine whose keyboard and mouse currently drive this one.
    controlled_by: Option<MachineName>,
    /// A peer in multi-machine input mode whose input we apply.
    broadcast_from: Option<MachineName>,
    /// We are sending our input to every machine.
    broadcasting: bool,
    /// What to do when the controller lets go (see `RequestControl`).
    pending_request: Option<Option<MachineName>>,
    grabbed: bool,
    status: Status,
    status_tx: watch::Sender<Status>,
    outgoing: BTreeMap<(MachineName, u64), Outgoing>,
    assemblers: BTreeMap<MachineName, Assembler>,
    files: Receiver,
    local_clip: Option<ClipboardContent>,
    /// Fingerprint of the last clipboard value we wrote on a peer's behalf.
    applied_clip: Option<[u8; 32]>,
    sent_clip: BTreeMap<MachineName, [u8; 32]>,
    last_error: BTreeMap<MachineName, String>,
    last_local_input: Option<Instant>,
    next_keep_awake: Instant,
    next_screen_check: Instant,
    last_nudge: Option<Instant>,
    saved_easy_mouse: aujar_config::EasyMouse,
    events: mpsc::Sender<Event>,
    /// Peers whose connection stalled while sending; dropped after the current event.
    dead: Vec<MachineName>,
}

impl Actor {
    async fn run(mut self, mut events: mpsc::Receiver<Event>, mut shutdown: watch::Receiver<bool>) {
        self.publish();

        loop {
            tokio::select! {
                biased;
                _ = shutdown.changed() => break,
                event = events.recv() => match event {
                    Some(event) => self.handle(event).await,
                    None => break,
                },
            }

            while let Some(name) = self.dead.pop() {
                self.drop_peer(&name, "connection stalled").await;
            }
        }

        self.shut_down().await;
    }

    async fn shut_down(&mut self) {
        let _ = self.release_grab().await;
        let _ = self.injector.release_all();

        let peers: Vec<_> = self.peers.keys().cloned().collect();

        for peer in &peers {
            self.send(
                peer,
                Message::Bye {
                    reason: "shutting down".into(),
                },
            );
        }

        self.peers.clear();
        self.audit(AuditEvent::ServiceStopped);
        self.status.running = false;
        self.publish();
    }

    fn audit(&self, event: AuditEvent) {
        self.shared.audit.record(event);
    }

    async fn handle(&mut self, event: Event) {
        match event {
            Event::Local(input) => self.on_local(input).await,
            Event::SourceFailed(reason) => {
                self.status.capture_unavailable = Some(reason.clone());
                self.log(format!(
                    "cannot capture input: {reason}. This machine can still be controlled."
                ));
            }
            Event::Edge(event) => self.on_edge_event(event).await,
            Event::PeerUp(up) => self.on_peer_up(up).await,
            Event::Peer {
                name,
                conn_id,
                message,
            } => {
                if self
                    .peers
                    .get(&name)
                    .is_some_and(|peer| peer.conn_id == conn_id)
                {
                    self.on_peer_message(name, message).await;
                }
            }
            Event::PeerDown {
                name,
                conn_id,
                error,
            } => {
                if self
                    .peers
                    .get(&name)
                    .is_some_and(|peer| peer.conn_id == conn_id)
                {
                    let reason = error.unwrap_or_else(|| "connection closed".into());

                    self.drop_peer(&name, &reason).await;
                }
            }
            Event::DialFailed { name, error } => {
                self.last_error.insert(name, error);
                self.publish();
            }
            Event::ClipboardChanged(content) => self.on_clipboard_changed(content),
            Event::Command { command, reply } => {
                let result = self.on_command(command).await;
                let _ = reply.send(result);
                self.publish();
            }
            Event::Note(note) => self.log(note),
            Event::Tick => self.on_tick().await,
        }

        self.update_edge_watch();
    }

    // ---- publishing ------------------------------------------------------------------

    fn log(&mut self, entry: impl Into<String>) {
        let entry = entry.into();

        tracing::info!(target: "aujar::mouse_borders", "{entry}");
        self.status.push_log(entry);
        self.publish();
    }

    fn publish(&mut self) {
        self.status.focus = self.router.focused_machine().to_string();
        self.status.controlled_by = self.controlled_by.as_ref().map(ToString::to_string);
        self.status.multi_machine_input = self.broadcasting;
        self.status.easy_mouse = self.router.policy().easy_mouse;
        self.status.peers = self
            .cfg
            .peers
            .iter()
            .map(|(name, configured)| {
                let peer = self.peers.get(name);

                PeerStatus {
                    name: name.to_string(),
                    connected: peer.is_some(),
                    address: peer
                        .and_then(|peer| peer.remote.map(|addr| addr.to_string()))
                        .or_else(|| configured.clone()),
                    screen: peer.and_then(|_| self.router.screen(name)),
                    slot: self.cfg.layout.shortcut_number_of(name),
                    last_error: if peer.is_some() {
                        None
                    } else {
                        self.last_error.get(name).cloned()
                    },
                }
            })
            .collect();

        self.status.local_slot = self.cfg.layout.shortcut_number_of(&self.cfg.local);
        self.status.layout = self
            .cfg
            .layout
            .machines()
            .into_iter()
            .flat_map(|machine| {
                [
                    (Edge::Left, "left"),
                    (Edge::Right, "right"),
                    (Edge::Top, "top"),
                    (Edge::Bottom, "bottom"),
                ]
                .into_iter()
                .filter_map(|(edge, label)| {
                    self.cfg
                        .layout
                        .neighbor(machine, edge)
                        .map(|to| LayoutLink {
                            from: machine.to_string(),
                            edge: label.to_owned(),
                            to: to.to_string(),
                        })
                })
                .collect::<Vec<_>>()
            })
            .collect();

        self.status_tx.send_replace(self.status.clone());
    }

    // ---- sending ---------------------------------------------------------------------

    /// Queue `message` for `to`. Never waits: a stuck peer must not stall the engine.
    fn send(&mut self, to: &MachineName, message: Message) -> bool {
        use tokio::sync::mpsc::error::TrySendError;

        let Some(peer) = self.peers.get(to) else {
            return false;
        };
        let realtime = message.is_realtime();
        let lane = if realtime {
            &peer.link.realtime
        } else {
            &peer.link.bulk
        };

        match lane.try_send(message) {
            Ok(()) => true,
            Err(TrySendError::Full(message)) if !realtime => {
                // Bulk lane busy (a file is streaming): wait for room off the actor.
                let lane = lane.clone();
                tokio::spawn(async move {
                    let _ = lane.send(message).await;
                });

                true
            }
            Err(TrySendError::Full(Message::Input(
                InputEvent::PointerRel { .. } | InputEvent::PointerAbs { .. },
            ))) => true, // Pointer motion is lossy by nature.
            Err(TrySendError::Full(_) | TrySendError::Closed(_)) => {
                self.dead.push(to.clone());

                false
            }
        }
    }

    fn send_all(&mut self, message: &Message) {
        let names: Vec<_> = self.peers.keys().cloned().collect();

        for name in names {
            self.send(&name, message.clone());
        }
    }

    // ---- grabbing and transfers --------------------------------------------------------

    async fn set_grab(&mut self, grabbed: bool) -> Result<()> {
        let (reply, answer) = oneshot::channel();

        self.grab
            .send(GrabRequest { grabbed, reply })
            .await
            .map_err(|_| Error::unsupported("input capture", "the input task has stopped"))?;

        timeout(GRAB_TIMEOUT, answer)
            .await
            .map_err(|_| Error::Other("grabbing input timed out".into()))?
            .map_err(|_| Error::Other("the input task stopped".into()))?
    }

    /// Whether any key or mouse button is physically down.
    fn input_is_held(&self) -> bool {
        !self.keyboard.held().is_empty() || !self.held_buttons.is_empty()
    }

    /// Start capturing the local input for another machine.
    ///
    /// Grabbing a device hides everything that happens on it from the desktop. A key or button
    /// that is down at that moment would go up unseen, and the desktop (libinput) would think it
    /// is held for good: a stuck Ctrl+Alt, a key that repeats on its own, a click that never
    /// registers. Letting the virtual keyboard "release" them does not help, because the
    /// desktop tracks keys per device. So with something held the grab waits until it is let
    /// go (`wait_for_release`), which is what happens with every Ctrl+Alt+number shortcut.
    async fn acquire_grab(&mut self) -> Result<()> {
        if self.grabbed || self.grab_wait.is_some() {
            return Ok(());
        }

        if self.input_is_held() {
            self.grab_wait = Some(Instant::now() + GRAB_RELEASE_WAIT);

            return Ok(());
        }

        self.grab_now().await
    }

    async fn grab_now(&mut self) -> Result<()> {
        self.set_grab(true).await?;
        self.grabbed = true;

        // Only after a wait that ran out, with a key still down: tell the desktop at least what
        // a virtual keyboard can.
        let held = self.keyboard.held();
        let _ = self.injector.release_keys(&held);

        self.hide_pointer();

        Ok(())
    }

    /// Hide the local pointer while another machine is controlled (when that is wanted), and
    /// note when, so it can be renewed: a desktop that hides it on request (GNOME) shows it
    /// again by itself if nobody renews the request, so it cannot stay hidden for good.
    fn hide_pointer(&mut self) {
        if !self.cfg.hide_mouse_at_edge {
            return;
        }

        match self.desktop.set_cursor_visible(false) {
            Ok(()) => self.pointer_hidden_at = Some(Instant::now()),
            Err(error) => self.log(format!("cannot hide the pointer: {error}")),
        }
    }

    /// A switch is waiting for held keys and buttons to be let go. Returns `true` when the
    /// event is not to be passed on: the desktop is still handling the input itself, or this
    /// was the release that ended the wait, which it has already seen.
    async fn wait_for_release(&mut self, now: Instant) -> bool {
        let Some(deadline) = self.grab_wait else {
            return false;
        };

        if self.router.focus() == &Focus::Local {
            // Control came back before the grab: nothing to wait for.
            self.grab_wait = None;

            return false;
        }

        let held = self.input_is_held();

        if held && now < deadline {
            return true;
        }

        self.grab_wait = None;

        if let Err(error) = self.grab_now().await {
            // Like a switch that cannot capture: stay on this machine.
            self.log(format!("cannot capture input: {}", error.reason()));

            let local = self.cfg.local.clone();

            if let Focus::Remote(machine) = self.router.focus().clone() {
                self.send(&machine, Message::Leave);
            }

            self.router.jump(&local, now);
            self.publish();

            return true;
        }

        !held
    }

    async fn release_grab(&mut self) -> Result<()> {
        self.grab_wait = None;

        if !self.grabbed {
            return Ok(());
        }

        if self.cfg.hide_mouse_at_edge {
            let _ = self.desktop.set_cursor_visible(true);
        }

        self.pointer_hidden_at = None;

        // Only forget the grab once the devices really let go. Clearing the flag first meant a
        // failed release (timeout, device change) was never retried, and the local mouse stayed
        // dead until the program was restarted.
        self.set_grab(false).await?;
        self.grabbed = false;

        Ok(())
    }

    /// Carry out a change of focus decided by the router.
    async fn perform_transfer(&mut self, transfer: Transfer) -> Result<()> {
        let local = self.cfg.local.clone();

        if transfer.from != local {
            self.send(&transfer.from, Message::Leave);
        }

        if transfer.to == local {
            if let Err(error) = self.release_grab().await {
                self.log(format!("cannot release input: {error}"));
            }

            if let Err(error) = self.injector.warp(transfer.entry) {
                self.log(format!("cannot move the pointer: {error}"));
            }
        } else {
            if transfer.from == local
                && let Err(error) = self.acquire_grab().await
            {
                // Without a grab local input would keep moving this desktop too.
                self.log(format!("cannot capture input: {}", error.reason()));
                self.router.jump(&local, Instant::now());
                self.publish();

                return Err(Error::Other(format!(
                    "cannot capture input: {}",
                    error.reason()
                )));
            }

            self.send(
                &transfer.to,
                Message::Enter {
                    position: transfer.entry,
                    relative: self.cfg.move_relatively,
                },
            );

            if transfer.from == local {
                self.refresh_local_clip().await;
            }

            self.push_clipboard(&transfer.to);
        }

        self.audit(AuditEvent::ControlMoved {
            from: transfer.from.to_string(),
            to: transfer.to.to_string(),
        });
        self.publish();

        Ok(())
    }

    // ---- local input -------------------------------------------------------------------

    async fn on_local(&mut self, input: CapturedInput) {
        let now = Instant::now();

        self.last_local_input = Some(now);

        if let CapturedInput::Button { button, pressed } = input {
            if pressed {
                self.held_buttons.insert(button);
            } else {
                self.held_buttons.remove(&button);
            }
        }

        if let CapturedInput::Key { key, pressed } = input {
            self.keyboard.update(key, pressed);

            if pressed && let Some(action) = self.cfg.hotkeys.lookup(key, self.keyboard.modifiers())
            {
                self.on_hotkey(action).await;

                return;
            }
        }

        if self.grab_wait.is_some() && self.wait_for_release(now).await {
            // The desktop still has the keyboard and the buttons, but the pointer and the wheel
            // can already move the other machine (they hold no state that could get stuck).
            if matches!(
                input,
                CapturedInput::PointerMotion { .. } | CapturedInput::Wheel { .. }
            ) && let Focus::Remote(machine) = self.router.focus().clone()
            {
                self.forward(&machine, input, now).await;
            }

            return;
        }

        match self.router.focus().clone() {
            Focus::Remote(machine) => self.forward(&machine, input, now).await,
            Focus::Local if self.broadcasting => self.broadcast(input),
            Focus::Local if self.controlled_by.is_none() => {
                if let CapturedInput::PointerMotion { dx, dy } = input {
                    self.check_local_edge((dx, dy), now).await;
                }
            }
            // Another machine controls this one, and this machine's own mouse still works.
            Focus::Local => {
                if let CapturedInput::PointerMotion { dx, dy } = input {
                    self.take_over_at_local_edge((dx, dy), now).await;
                }
            }
        }
    }

    /// The real pointer position, when switching at screen edges is on and works on this
    /// machine.
    fn edge_pointer(&mut self) -> Option<Position> {
        // The desktop reports the edges itself (`on_edge_hit`): there is no pointer to read.
        if self.edge_watch_ready {
            return None;
        }

        if !self.cfg.easy_mouse_enabled()
            || self.router.policy().easy_mouse == aujar_config::EasyMouse::Disabled
            || self.status.edge_switching_unavailable.is_some()
        {
            return None;
        }

        match self.desktop.pointer() {
            Ok(position) => Some(position),
            Err(error) => {
                self.status.edge_switching_unavailable = Some(error.reason());
                self.log(format!(
                    "switching at screen edges is unavailable: {}. \
                     Keyboard shortcuts still work.",
                    error.reason()
                ));

                None
            }
        }
    }

    async fn check_local_edge(&mut self, delta: (i32, i32), now: Instant) {
        let Some(position) = self.edge_pointer() else {
            return;
        };

        self.switch_at_local_edge(position, delta, now).await;
    }

    /// The local pointer is at `position`, pushing by `delta`: switch machines if that leads to
    /// one.
    async fn switch_at_local_edge(&mut self, position: Position, delta: (i32, i32), now: Instant) {
        let modifiers = self.keyboard.modifiers();

        if let Some(transfer) = self.router.on_local_motion(position, delta, modifiers, now) {
            let _ = self.perform_transfer(transfer).await;
        }
    }

    /// Another machine controls this one. Pushing this machine's own mouse against an edge that
    /// leads to another machine asks the controller to let go, and then this machine takes
    /// control of the one beyond the edge (the controller, in a row of two). So either laptop's
    /// mouse and keyboard can take over the other.
    async fn take_over_at_local_edge(&mut self, delta: (i32, i32), now: Instant) {
        // One request at a time: the controller has not answered the last one yet.
        if self.pending_request.is_some() {
            return;
        }

        let Some(position) = self.edge_pointer() else {
            return;
        };

        self.ask_for_control_at(position, delta, now).await;
    }

    async fn ask_for_control_at(&mut self, position: Position, delta: (i32, i32), now: Instant) {
        if self.pending_request.is_some() {
            return;
        }

        let modifiers = self.keyboard.modifiers();
        let Some(target) = self
            .router
            .local_edge_target(position, delta, modifiers, now)
        else {
            return;
        };

        match self.switch_to(Some(target)).await {
            Ok(message) => self.log(message),
            Err(error) => self.log(format!("{error}")),
        }
    }

    // ---- screen edges reported by the desktop (Wayland) ----------------------------------

    async fn on_edge_event(&mut self, event: EdgeEvent) {
        match event {
            EdgeEvent::Ready => {
                self.edge_watch_ready = true;
                self.status.edge_switching_unavailable = None;
                self.watched_edges.clear();
                self.log(
                    "switching at screen edges works through the desktop's input-capture portal",
                );
                self.publish();
            }
            EdgeEvent::Unavailable(reason) => {
                self.edge_watch_ready = false;
                self.status.edge_switching_unavailable = Some(
                    match self.status.edge_switching_unavailable.take() {
                        Some(before) => format!(
                            "{before}. The desktop's input-capture portal does not help either: {reason}"
                        ),
                        None => reason.clone(),
                    },
                );
                self.log(format!(
                    "screen edges are not reported by the desktop: {reason}. Keyboard shortcuts still work."
                ));
                self.publish();
            }
            EdgeEvent::Note(note) => self.log(note),
            EdgeEvent::Hit(hit) => self.on_edge_hit(hit, Instant::now()).await,
        }
    }

    /// The pointer ran into a screen edge that leads to a connected machine.
    async fn on_edge_hit(&mut self, hit: EdgeHit, now: Instant) {
        // The desktop already has the pointer back (the watcher gives it back first), so a hit
        // that does not lead to a switch (a modifier is missing, a pause after a switch) costs
        // nothing; one that does is picked up by our own grab of the devices.
        if self.router.focus() != &Focus::Local {
            return;
        }

        // Another machine is driving this one and nobody touched this machine's own mouse: the
        // pointer only ran into the edge because of that machine's motion. Its own router
        // decides when control goes back.
        if self.controlled_by.is_some()
            && !self
                .last_local_input
                .is_some_and(|at| now.saturating_duration_since(at) < OWN_INPUT_WINDOW)
        {
            return;
        }

        let (position, delta) = self.router.edge_push(hit.edge, hit.along);

        if self.controlled_by.is_none() {
            self.switch_at_local_edge(position, delta, now).await;
        } else {
            self.ask_for_control_at(position, delta, now).await;
        }
    }

    /// Tell the desktop which edges lead to a connected machine, so only those are reported:
    /// the others must behave like any screen edge.
    fn update_edge_watch(&mut self) {
        if self.edge_control.is_none() || !self.edge_watch_ready {
            return;
        }

        let local = self.cfg.local.clone();
        // No barriers while this machine is not the one in use. When another machine drives
        // the pointer here, its motion runs into our barriers; the desktop then captures and
        // releases the pointer over and over, which shows as flicker at the border (and made
        // this machine "ask for control" on behalf of motion that was not its user's).
        let in_use = match self.controlled_by {
            Some(_) => self
                .last_local_input
                .is_some_and(|at| at.elapsed() < OWN_INPUT_WINDOW),
            None => true,
        };
        let edges: Vec<Edge> = if self.router.focus() != &Focus::Local || !in_use {
            Vec::new()
        } else {
            [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom]
                .into_iter()
                .filter(|edge| {
                    self.router
                        .layout()
                        .neighbor(&local, *edge)
                        .is_some_and(|to| self.router.is_online(to))
                })
                .collect()
        };

        if edges != self.watched_edges {
            self.watched_edges = edges.clone();

            if let Some(control) = &self.edge_control {
                let _ = control.send(EdgeControl::Watch(edges));
            }
        }
    }

    async fn forward(&mut self, machine: &MachineName, input: CapturedInput, now: Instant) {
        let event = match input {
            CapturedInput::PointerMotion { dx, dy } => {
                match self
                    .router
                    .on_remote_motion((dx, dy), self.keyboard.modifiers(), now)
                {
                    Some(RemoteStep::Moved { position }) => {
                        if self.cfg.move_relatively {
                            InputEvent::PointerRel { dx, dy }
                        } else {
                            InputEvent::PointerAbs {
                                x: position.x,
                                y: position.y,
                            }
                        }
                    }
                    Some(RemoteStep::Switched(transfer)) => {
                        let _ = self.perform_transfer(transfer).await;

                        return;
                    }
                    None => return,
                }
            }
            other => to_event(other),
        };

        self.send(machine, Message::Input(event));
    }

    fn broadcast(&mut self, input: CapturedInput) {
        let event = match input {
            CapturedInput::PointerMotion { dx, dy } => InputEvent::PointerRel { dx, dy },
            other => to_event(other),
        };

        self.send_all(&Message::Input(event));
    }

    // ---- hotkeys and commands ----------------------------------------------------------

    async fn on_hotkey(&mut self, action: HotkeyAction) {
        let result = match action {
            HotkeyAction::ToggleEasyMouse => {
                let enabled = self.router.policy().easy_mouse == aujar_config::EasyMouse::Disabled;

                self.on_command(Command::SetEasyMouse(enabled)).await
            }
            HotkeyAction::LockAll => self.on_command(Command::LockAll).await,
            HotkeyAction::Reconnect => self.on_command(Command::Reconnect).await,
            HotkeyAction::ToggleMultiMachineInput => {
                let on = !self.broadcasting;

                self.on_command(Command::MultiMachineInput(on)).await
            }
            HotkeyAction::Switch(number) => {
                match self.cfg.layout.machine_for_shortcut(number).cloned() {
                    Some(machine) => self.switch_to(Some(machine)).await,
                    None => Err(Error::Layout(format!(
                        "no machine for shortcut {}",
                        number + 1
                    ))),
                }
            }
        };

        match result {
            Ok(message) => self.log(message),
            Err(error) => self.log(format!("{error}")),
        }
    }

    async fn on_command(&mut self, command: Command) -> Result<String> {
        match command {
            Command::SwitchTo(target) => self.switch_to(target).await,
            Command::SetEasyMouse(enabled) => {
                let mut policy = *self.router.policy();

                if enabled {
                    policy.easy_mouse = self.saved_easy_mouse;
                } else {
                    if policy.easy_mouse != aujar_config::EasyMouse::Disabled {
                        self.saved_easy_mouse = policy.easy_mouse;
                    }

                    policy.easy_mouse = aujar_config::EasyMouse::Disabled;
                }

                self.router.set_policy(policy);

                Ok(format!(
                    "switching with the mouse is {}",
                    if enabled { "on" } else { "off" }
                ))
            }
            Command::LockAll => {
                self.send_all(&Message::LockScreen);
                self.audit(AuditEvent::LockRequested {
                    peer: None,
                    allowed: true,
                });
                self.locker.lock().await?;

                Ok("locked all machines".into())
            }
            Command::Reconnect => {
                let names: Vec<_> = self.peers.keys().cloned().collect();

                for name in &names {
                    self.send(
                        name,
                        Message::Bye {
                            reason: "reconnecting".into(),
                        },
                    );
                    self.drop_peer(name, "reconnecting").await;
                }

                self.shared.reconnect.notify_waiters();

                Ok("reconnecting".into())
            }
            Command::MultiMachineInput(on) => self.set_broadcast(on),
            Command::SendFiles { machine, paths } => self.send_files(machine, paths).await,
        }
    }

    async fn switch_to(&mut self, target: Option<MachineName>) -> Result<String> {
        let local = self.cfg.local.clone();
        let target = target.unwrap_or_else(|| local.clone());

        if !self.cfg.layout.contains(&target) {
            return Err(Error::Layout(format!("`{target}` is not in the layout")));
        }

        // Another machine controls us: ask it to hand over, then continue from there.
        if let Some(controller) = self.controlled_by.clone() {
            let wanted = (target != local).then(|| target.clone());

            self.pending_request = Some(wanted.clone());
            self.send(&controller, Message::RequestControl { target: wanted });

            return Ok(format!("asked {controller} to hand over control"));
        }

        if self.broadcasting {
            self.set_broadcast(false)?;
        }

        match self.router.jump(&target, Instant::now()) {
            Some(transfer) => {
                self.perform_transfer(transfer).await?;

                if self.router.focused_machine() == &target {
                    Ok(format!("keyboard and mouse are now on {target}"))
                } else {
                    Err(Error::Other(format!("could not switch to {target}")))
                }
            }
            None if self.router.focused_machine() == &target => Ok(format!("already on {target}")),
            None => Err(Error::NotConnected(target.to_string())),
        }
    }

    fn set_broadcast(&mut self, on: bool) -> Result<String> {
        if on == self.broadcasting {
            return Ok(format!(
                "multi-machine input is already {}",
                if on { "on" } else { "off" }
            ));
        }

        if on {
            if self.router.focus() != &Focus::Local || self.controlled_by.is_some() {
                return Err(Error::Other(
                    "multi-machine input starts from this machine's own desktop".into(),
                ));
            }

            if self.peers.is_empty() {
                return Err(Error::Other("no machines are connected".into()));
            }
        }

        self.broadcasting = on;
        self.send_all(&Message::Broadcast { active: on });
        self.audit(AuditEvent::BroadcastChanged { on });

        Ok(if on {
            "multi-machine input on: your input goes to every connected machine".into()
        } else {
            "multi-machine input off".into()
        })
    }

    async fn send_files(&mut self, machine: MachineName, paths: Vec<PathBuf>) -> Result<String> {
        if !self.cfg.transfer_files {
            return Err(Error::Transfer(
                "file transfer is disabled in settings".into(),
            ));
        }

        let peer = self
            .peers
            .get(&machine)
            .ok_or_else(|| Error::NotConnected(machine.to_string()))?;

        if !peer.features.files {
            return Err(Error::Transfer(format!(
                "`{machine}` has file transfer disabled"
            )));
        }

        let mut offered = 0usize;
        let mut problems = Vec::new();

        for path in paths {
            if self.outgoing.len() >= MAX_OUTGOING_TRANSFERS {
                problems.push("too many transfers waiting".to_owned());
                break;
            }

            match Outgoing::prepare(&path, self.cfg.max_file_bytes).await {
                Ok(file) => {
                    let id = random_id();

                    self.send(
                        &machine,
                        Message::File(FileMessage::Offer {
                            id,
                            name: file.name.clone(),
                            size: file.size,
                        }),
                    );
                    self.audit(AuditEvent::FileOffered {
                        peer: machine.to_string(),
                        name: file.name.clone(),
                        bytes: file.size,
                    });
                    self.outgoing.insert((machine.clone(), id), file);
                    offered += 1;
                }
                Err(error) => problems.push(error.to_string()),
            }
        }

        if offered == 0 {
            return Err(Error::Transfer(problems.join("; ")));
        }

        let mut message = format!("offered {offered} file(s) to {machine}");

        if !problems.is_empty() {
            message.push_str(&format!(" (skipped: {})", problems.join("; ")));
        }

        Ok(message)
    }

    // ---- peers -------------------------------------------------------------------------

    async fn on_peer_up(&mut self, up: PeerUp) {
        let local = self.cfg.local.clone();
        let name = up.name.clone();

        // Two machines may dial each other at once. The connection opened by the
        // alphabetically first name wins, so both ends keep the same one.
        let winner = std::cmp::min(&local, &name).clone();

        if let Some(existing) = self.peers.get(&name) {
            if existing.initiator == winner && up.initiator != winner {
                let _ = up.link.realtime.try_send(Message::Bye {
                    reason: "duplicate connection".into(),
                });

                return;
            }

            let _ = existing.link.realtime.try_send(Message::Bye {
                reason: "replaced by a newer connection".into(),
            });
        }

        self.shared.set_connected(&name, true);
        self.router.peer_up(name.clone(), up.hello.screen);
        self.last_error.remove(&name);
        self.peers.insert(
            name.clone(),
            PeerState {
                conn_id: up.conn_id,
                initiator: up.initiator,
                link: up.link,
                features: up.hello.features,
                remote: up.remote,
            },
        );

        if self.broadcasting {
            self.send(&name, Message::Broadcast { active: true });
        }

        self.audit(AuditEvent::PeerConnected {
            peer: name.to_string(),
            address: self
                .peers
                .get(&name)
                .and_then(|peer| peer.remote)
                .map(|a| a.to_string()),
        });
        self.log(format!("connected to {name}"));
    }

    async fn drop_peer(&mut self, name: &MachineName, reason: &str) {
        if self.peers.remove(name).is_none() {
            return;
        }

        self.shared.set_connected(name, false);
        self.files.drop_peer(name).await;
        self.assemblers.remove(name);
        self.sent_clip.remove(name);
        self.outgoing.retain(|(peer, _), _| peer != name);

        if let Some(transfer) = self.router.peer_down(name, Instant::now()) {
            let _ = self.perform_transfer(transfer).await;
        }

        if self.controlled_by.as_ref() == Some(name) {
            self.controlled_by = None;
            self.pending_request = None;
            self.audit(AuditEvent::ControlledBy { peer: None });
            let _ = self.injector.release_all();
        }

        if self.broadcast_from.as_ref() == Some(name) {
            self.broadcast_from = None;
            let _ = self.injector.release_all();
        }

        self.last_error.insert(name.clone(), reason.to_owned());
        self.audit(AuditEvent::PeerDisconnected {
            peer: name.to_string(),
            reason: reason.to_owned(),
        });
        self.log(format!("{name} disconnected ({reason})"));
    }

    async fn on_peer_message(&mut self, from: MachineName, message: Message) {
        match message {
            Message::Enter { position, relative } => self.on_enter(from, position, relative).await,
            Message::Leave => self.on_leave(&from).await,
            Message::Input(event) => {
                let allowed = self.controlled_by.as_ref() == Some(&from)
                    || self.broadcast_from.as_ref() == Some(&from);

                if allowed && let Err(error) = self.injector.apply(&event) {
                    tracing::debug!(%error, "cannot inject input");
                }
            }
            Message::RequestControl { .. } => {
                if self.router.focus() == &Focus::Remote(from.clone())
                    && let Some(transfer) =
                        self.router.jump(&self.cfg.local.clone(), Instant::now())
                {
                    let _ = self.perform_transfer(transfer).await;
                }
            }
            Message::Broadcast { active } => self.on_broadcast(from, active),
            Message::Screen(size) => {
                self.router.peer_up(from, size);
                self.publish();
            }
            Message::Clipboard(packet) => self.on_clipboard_packet(from, packet).await,
            Message::File(message) => self.on_file(from, message).await,
            Message::KeepAwake => {
                let now = Instant::now();
                let due = self
                    .last_nudge
                    .is_none_or(|at| now.duration_since(at) >= NUDGE_INTERVAL);

                if self.cfg.block_screensaver && due {
                    self.last_nudge = Some(now);
                    let _ = self.injector.nudge();
                }
            }
            Message::LockScreen => {
                self.audit(AuditEvent::LockRequested {
                    peer: Some(from.to_string()),
                    allowed: self.cfg.allow_remote_lock,
                });

                if self.cfg.allow_remote_lock {
                    match self.locker.lock().await {
                        Ok(()) => self.log(format!("{from} locked this machine")),
                        Err(error) => self.log(format!("cannot lock the screen: {error}")),
                    }
                } else {
                    self.log(format!(
                        "{from} asked to lock this machine; refused by settings"
                    ));
                }
            }
            Message::Bye { reason } => self.log(format!("{from} said goodbye: {reason}")),
            Message::Hello(_) | Message::Heartbeat => {}
        }
    }

    async fn on_enter(&mut self, from: MachineName, position: Position, relative: bool) {
        let local = self.cfg.local.clone();

        // We were controlling someone else; the newcomer wins.
        if matches!(self.router.focus(), Focus::Remote(_))
            && let Some(transfer) = self.router.jump(&local, Instant::now())
        {
            let _ = self.perform_transfer(transfer).await;
        }

        if self.broadcasting {
            let _ = self.set_broadcast(false);
        }

        if let Some(previous) = self.controlled_by.replace(from.clone())
            && previous != from
        {
            // Tell the old controller it lost us.
            self.send(&previous, Message::RequestControl { target: None });
        }

        self.broadcast_from = None;

        if !relative {
            let screen = self.router.screen(&local).unwrap_or(FALLBACK_SCREEN);
            let bounds = aujar_core::Rect::new(0, 0, screen.width, screen.height);

            let _ = self.injector.warp(bounds.clamp(position));
        }

        self.audit(AuditEvent::ControlledBy {
            peer: Some(from.to_string()),
        });
        self.log(format!("{from} is controlling this machine"));
    }

    async fn on_leave(&mut self, from: &MachineName) {
        if self.controlled_by.as_ref() != Some(from) {
            return;
        }

        self.controlled_by = None;
        let _ = self.injector.release_all();
        self.audit(AuditEvent::ControlledBy { peer: None });
        self.log(format!("{from} released this machine"));

        // Control goes back to `from`: where nothing watches the clipboard, this is the moment
        // to look at what was copied here and hand it over.
        if self.refresh_local_clip().await {
            self.push_clipboard(from);
        }

        if let Some(Some(target)) = self.pending_request.take() {
            match self.switch_to(Some(target)).await {
                Ok(message) => self.log(message),
                Err(error) => self.log(format!("{error}")),
            }
        }
    }

    fn on_broadcast(&mut self, from: MachineName, active: bool) {
        if active {
            if self.controlled_by.is_none() && !self.grabbed && !self.broadcasting {
                self.broadcast_from = Some(from.clone());
                self.log(format!("{from} is sending input to every machine"));
            }
        } else if self.broadcast_from.as_ref() == Some(&from) {
            self.broadcast_from = None;
            let _ = self.injector.release_all();
            self.log(format!("{from} stopped multi-machine input"));
        }
    }

    // ---- clipboard ---------------------------------------------------------------------

    fn on_clipboard_changed(&mut self, content: ClipboardContent) {
        let fingerprint = clipboard::fingerprint(&content);

        // Our own write of a peer's clipboard coming back through the watcher.
        if self.applied_clip == Some(fingerprint) {
            return;
        }

        self.local_clip = Some(content);

        // Whoever is working with this machine right now gets the new value.
        let mut targets = Vec::new();

        if let Some(controller) = &self.controlled_by {
            targets.push(controller.clone());
        }

        if let Focus::Remote(machine) = self.router.focus() {
            targets.push(machine.clone());
        }

        for target in targets {
            self.push_clipboard(&target);
        }
    }

    /// Where the clipboard is not watched in the background, read it now (control is moving
    /// between machines). Returns whether it did; with a watcher `local_clip` is already current.
    async fn refresh_local_clip(&mut self) -> bool {
        if !self.cfg.share_clipboard {
            return false;
        }

        let Some(clipboard) = self.clipboard.clone() else {
            return false;
        };

        if clipboard.polls_in_background() {
            return false;
        }

        if let Ok(Some(content)) = clipboard.read().await {
            // A value a peer just sent us is already `local_clip`; do not treat it as news.
            if self.applied_clip != Some(clipboard::fingerprint(&content)) {
                self.local_clip = Some(content);
            }
        }

        true
    }

    fn push_clipboard(&mut self, to: &MachineName) {
        if !self.cfg.share_clipboard {
            return;
        }

        let Some(content) = self.local_clip.clone() else {
            return;
        };
        let Some(peer) = self.peers.get(to) else {
            return;
        };

        if !peer.features.clipboard {
            return;
        }

        let fingerprint = clipboard::fingerprint(&content);

        if self.sent_clip.get(to) == Some(&fingerprint) {
            return;
        }

        match clipboard::packets(random_id(), &content) {
            Ok(packets) => {
                let lane = peer.link.bulk.clone();

                self.sent_clip.insert(to.clone(), fingerprint);

                let (kind, bytes) = clipboard::describe(&content);

                self.shared.audit.record(AuditEvent::ClipboardSent {
                    peer: to.to_string(),
                    kind: kind.to_owned(),
                    bytes,
                });
                tokio::spawn(async move {
                    for packet in packets {
                        if lane.send(Message::Clipboard(packet)).await.is_err() {
                            break;
                        }
                    }
                });
            }
            Err(error) => self.log(format!("not sharing the clipboard: {error}")),
        }
    }

    async fn on_clipboard_packet(&mut self, from: MachineName, packet: ClipboardPacket) {
        if !self.cfg.share_clipboard {
            return;
        }

        let Some(clipboard) = self.clipboard.clone() else {
            return;
        };

        let result = self
            .assemblers
            .entry(from.clone())
            .or_default()
            .push(packet);

        match result {
            Ok(Some(content)) => {
                let fingerprint = clipboard::fingerprint(&content);

                // Remember it first so the watcher does not send it straight back.
                self.applied_clip = Some(fingerprint);
                self.sent_clip.insert(from.clone(), fingerprint);
                self.local_clip = Some(content.clone());

                let (kind, bytes) = clipboard::describe(&content);

                self.audit(AuditEvent::ClipboardReceived {
                    peer: from.to_string(),
                    kind: kind.to_owned(),
                    bytes,
                });

                if let Err(error) = clipboard.write(&content).await {
                    self.log(format!("cannot set the clipboard: {error}"));
                }
            }
            Ok(None) => {}
            Err(error) => self.log(format!("ignored clipboard data from {from}: {error}")),
        }
    }

    // ---- files -------------------------------------------------------------------------

    async fn on_file(&mut self, from: MachineName, message: FileMessage) {
        let now = Instant::now();

        match message {
            FileMessage::Offer { id, name, size } => {
                let verdict = if self.cfg.transfer_files {
                    self.files.offer(&from, id, &name, size, now).await
                } else {
                    Err(Error::Transfer("file transfer is disabled".into()))
                };

                match verdict {
                    Ok(()) => {
                        self.send(&from, Message::File(FileMessage::Accept { id }));
                        self.log(format!("receiving `{name}` from {from}"));
                    }
                    Err(error) => {
                        let reason = error.to_string();

                        self.send(
                            &from,
                            Message::File(FileMessage::Reject {
                                id,
                                reason: reason.clone(),
                            }),
                        );
                        self.audit(AuditEvent::FileRefused {
                            peer: from.to_string(),
                            name: name.clone(),
                            reason: reason.clone(),
                        });
                        self.log(format!("refused `{name}` from {from}: {reason}"));
                    }
                }
            }

            FileMessage::Accept { id } => {
                let Some(file) = self.outgoing.get(&(from.clone(), id)).cloned() else {
                    return;
                };
                let Some(peer) = self.peers.get(&from) else {
                    return;
                };
                let lane = peer.link.bulk.clone();
                let events = self.events.clone();

                tokio::spawn(async move {
                    let note = match file.stream(id, &lane).await {
                        Ok(()) => format!("sent `{}` to {from}", file.name),
                        Err(error) => {
                            let _ = lane
                                .send(Message::File(FileMessage::Abort {
                                    id,
                                    reason: error.to_string(),
                                }))
                                .await;

                            format!("could not send `{}` to {from}: {error}", file.name)
                        }
                    };

                    let _ = events.send(Event::Note(note)).await;
                });
            }

            FileMessage::Reject { id, reason } => {
                if let Some(file) = self.outgoing.remove(&(from.clone(), id)) {
                    self.log(format!("{from} refused `{}`: {reason}", file.name));
                }
            }

            FileMessage::Chunk { id, offset, data } => {
                if let Err(error) = self.files.chunk(&from, id, offset, &data, now).await {
                    self.send(
                        &from,
                        Message::File(FileMessage::Abort {
                            id,
                            reason: error.to_string(),
                        }),
                    );
                    self.log(format!("transfer from {from} failed: {error}"));
                }
            }

            FileMessage::Done { id, sha256 } => match self.files.done(&from, id, sha256).await {
                Ok(path) => {
                    let bytes = std::fs::metadata(&path).map_or(0, |meta| meta.len());

                    self.audit(AuditEvent::FileReceived {
                        peer: from.to_string(),
                        name: path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        bytes,
                    });
                    self.log(format!("received {} from {from}", path.display()));
                }
                Err(error) => {
                    self.send(
                        &from,
                        Message::File(FileMessage::Abort {
                            id,
                            reason: error.to_string(),
                        }),
                    );
                    self.log(format!("transfer from {from} failed: {error}"));
                }
            },

            FileMessage::Abort { id, reason } => {
                self.files.abort(&from, id).await;

                if self.outgoing.remove(&(from.clone(), id)).is_some() {
                    self.log(format!("{from} cancelled a transfer: {reason}"));
                }
            }
        }
    }

    // ---- timers ------------------------------------------------------------------------

    async fn on_tick(&mut self) {
        let now = Instant::now();

        // Away from this machine, with the pointer hidden: renew that now and then.
        if self
            .pointer_hidden_at
            .is_some_and(|at| now.duration_since(at) >= POINTER_HIDE_RENEWAL)
        {
            self.hide_pointer();
        }

        // Control is back on this machine but the devices are still captured (a release that
        // failed): try again every second instead of leaving the mouse dead.
        if self.grabbed && self.router.focus() == &Focus::Local {
            match self.release_grab().await {
                Ok(()) => self.log("input was still captured; released it"),
                Err(error) => tracing::warn!(%error, "cannot release captured input yet"),
            }
        }

        for (peer, id) in self.files.expire(now).await {
            self.send(
                &peer,
                Message::File(FileMessage::Abort {
                    id,
                    reason: "transfer timed out".into(),
                }),
            );
            self.log(format!("transfer from {peer} timed out"));
        }

        if now >= self.next_keep_awake {
            self.next_keep_awake = now + self.cfg.timing.keep_awake;

            let working = self
                .last_local_input
                .is_some_and(|at| now.duration_since(at) < ACTIVE_WINDOW);

            if self.cfg.block_screensaver && working {
                self.send_all(&Message::KeepAwake);

                // Grabbed input never reaches this desktop, so it would idle out.
                if self.grabbed {
                    let _ = self.injector.nudge();
                }
            }
        }

        if now >= self.next_screen_check {
            self.next_screen_check = now + self.cfg.timing.screen_check;

            if let Ok(size) = self.desktop.screen_size()
                && self.router.screen(&self.cfg.local) != Some(size)
            {
                self.router.set_local_screen(size);
                self.injector.set_screen(size);
                self.shared.set_screen(size);
                self.send_all(&Message::Screen(size));
                self.log(format!("screen size is now {}x{}", size.width, size.height));
            }
        }
    }
}

fn to_event(input: CapturedInput) -> InputEvent {
    match input {
        CapturedInput::PointerMotion { dx, dy } => InputEvent::PointerRel { dx, dy },
        CapturedInput::Button { button, pressed } => InputEvent::Button { button, pressed },
        CapturedInput::Wheel {
            horizontal,
            vertical,
        } => InputEvent::Wheel {
            horizontal,
            vertical,
        },
        CapturedInput::Key { key, pressed } => InputEvent::Key { key, pressed },
    }
}
