use anyhow::{Context, Result};
use aujar_core::{Capability, Health, WindowId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::Path;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

/// The local byte stream used by Aujar IPC. Linux and macOS use a user-owned Unix socket;
/// Windows uses a named pipe. Both carry the same versioned JSON-line protocol.
#[cfg(unix)]
type LocalStream = UnixStream;
#[cfg(windows)]
type LocalStream = NamedPipeClient;

/// The IPC protocol version this build speaks (see `docs/IPC.md`, "Versioning").
///
/// Bump it only for an incompatible change: a renamed or removed variant or field, or a changed
/// meaning. New variants and new `#[serde(default)]` fields are additive and keep the version.
pub const PROTOCOL_VERSION: u16 = 1;

/// The oldest protocol version the daemon still answers. When the protocol is bumped, keep the
/// old version here for at least one release so older clients get answers (in their own
/// version) instead of errors.
pub const MIN_PROTOCOL_VERSION: u16 = 1;

/// Whether this build can talk protocol `version`.
pub fn supports_version(version: u16) -> bool {
    (MIN_PROTOCOL_VERSION..=PROTOCOL_VERSION).contains(&version)
}

/// Explains a version mismatch to a person, with the fix.
fn version_mismatch(theirs: u16, them: &str) -> String {
    let ours = if MIN_PROTOCOL_VERSION == PROTOCOL_VERSION {
        format!("v{PROTOCOL_VERSION}")
    } else {
        format!("v{MIN_PROTOCOL_VERSION} to v{PROTOCOL_VERSION}")
    };

    format!(
        "Aujar IPC protocol mismatch: the {them} uses v{theirs}, this program supports {ours}. \
         Install the same Aujar release for both and restart the daemon \
         (`systemctl --user restart aujar`)"
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Ping,
    Health,
    Windows,
    Modules,
    Rename(RenameRequest),
    Search(SearchRequest),
    Execute(ExecuteRequest),
    /// Keep the connection open and receive [`Event`]s pushed by the daemon.
    Subscribe,
    /// Ask every subscribed UI to show, hide or toggle the launcher.
    Launcher(LauncherCommand),
    /// Let the user pick a color on screen; the response arrives once they choose or cancel.
    PickColor,
    /// Daemon diagnostics: connected UIs and global-shortcut state (see `aujar doctor`).
    Status,
    /// Share keyboard, mouse, clipboard and files with other machines.
    MouseBorders(MouseBordersRequest),
    /// Open the NexDesk remote-access manager, or ask whether it is installed.
    NexDesk(NexDeskRequest),
    /// Keep the computer from sleeping, or ask whether that is on.
    Awake(AwakeRequest),
    /// Which protocol versions the daemon speaks (older daemons do not know it
    /// and answer `Error`, which means "v1 only").
    Version,
}

/// Mouse Borders request and response types, shared with the standalone agent.
pub use aujar_mouse_borders::api::{
    MouseBordersPeer, MouseBordersRequest, MouseBordersResponse, MouseBordersStatus,
};

/// Awake request and response types.
pub use aujar_awake::{
    AwakeMode, AwakeStatus,
    api::{AwakeRequest, AwakeResponse},
};
/// NexDesk request and response types.
pub use aujar_nexdesk::api::{NexDeskRequest, NexDeskResponse, NexDeskStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LauncherCommand {
    Show,
    Hide,
    Toggle,
    /// Open the settings window.
    Settings,
    /// The color picker shortcut: pick from the screen, or open the editor first, as set up in
    /// the color picker settings.
    ColorPicker,
    /// Open the color editor with the color history.
    ColorEditor,
}

/// Messages pushed by the daemon on a subscribed connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Launcher(LauncherCommand),
    /// The daemon is exiting; the stream ends after this event.
    Shutdown,
}

impl Request {
    /// Capability the daemon must grant before serving this request.
    pub fn required_capability(&self) -> Option<Capability> {
        match self {
            Self::Windows => Some(Capability::ReadWindows),
            Self::Rename(_) => Some(Capability::FileSystem),
            Self::Execute(_) => Some(Capability::ExecuteCommand),
            Self::PickColor => Some(Capability::ScreenCapture),
            Self::MouseBorders(request) => request.required_capability(),
            Self::NexDesk(request) => request.required_capability(),
            Self::Awake(request) => request.required_capability(),
            Self::Ping
            | Self::Health
            | Self::Modules
            | Self::Search(_)
            | Self::Subscribe
            | Self::Status
            | Self::Version
            | Self::Launcher(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    /// UIs currently subscribed to events.
    pub subscribers: usize,
    /// Human-readable state of the native global shortcut.
    pub hotkeys: String,
}

/// A picked color in the formats Aujar supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorResponse {
    pub hex: String,
    pub rgb: String,
    pub hsl: String,
}

/// Launch a previously returned search result by its id.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteRequest {
    pub id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteResponse {
    pub title: String,
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    /// Maximum results (daemon clamps to 1..=200, default 10).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameRequest {
    pub paths: Vec<String>,
    pub pattern: String,
    pub replacement: String,
    pub regex: bool,
    pub apply: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response {
    Pong,
    Health(Health),
    Windows(Vec<WindowId>),
    Modules(ModulesResponse),
    Rename(RenameResponse),
    Search(SearchResponse),
    Executed(ExecuteResponse),
    /// Acknowledges [`Request::Subscribe`]; events follow on the same connection.
    Subscribed,
    Color(ColorResponse),
    Status(StatusResponse),
    MouseBorders(MouseBordersResponse),
    NexDesk(NexDeskResponse),
    Awake(AwakeResponse),
    /// A launcher command was broadcast to this many subscribers.
    Delivered {
        subscribers: usize,
    },
    Version(VersionResponse),
    /// The request failed. This variant's shape is frozen across protocol versions, so a client
    /// of any version can read why the daemon refused it.
    Error(String),
}

/// Answer to [`Request::Version`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionResponse {
    /// The newest protocol version the daemon speaks.
    pub protocol: u16,
    /// The oldest protocol version the daemon still answers.
    pub min_protocol: u16,
    /// The daemon's release (`0.1.1`).
    pub daemon: String,
}

impl VersionResponse {
    /// What this build of the daemon answers.
    pub fn current() -> Self {
        Self {
            protocol: PROTOCOL_VERSION,
            min_protocol: MIN_PROTOCOL_VERSION,
            daemon: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Whether a client speaking `version` can talk to this daemon.
    pub fn accepts(&self, version: u16) -> bool {
        (self.min_protocol..=self.protocol).contains(&version)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub results: Vec<SearchHit>,
}

/// Wire format of a launcher result (kept independent of the launcher crate).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub kind: String,
    pub score: u32,
    /// Absolute path of an icon image (PNG/SVG), when one was found.
    #[serde(default)]
    pub icon: Option<String>,
    /// Longer text for a preview pane (definitions, forecast, quote details), one item per
    /// line. Additive: older clients ignore it, older daemons omit it.
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModulesResponse {
    pub modules: Vec<ModuleInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub api_version: u16,
    pub description: String,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameResponse {
    pub plan: aujar_rename::RenamePlan,
    pub applied: bool,
}

/// Wire envelope: every line on the socket is one JSON-encoded `Envelope`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub version: u16,
    pub payload: T,
}

/// The name of an enum variant (`Search`, `Pong`, ...) without its contents.
///
/// For logging only: requests and responses can hold search text, file names, clipboard data or
/// key material, which must never reach a log.
fn variant_name<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        Ok(serde_json::Value::Object(map)) => {
            map.keys().next().cloned().unwrap_or_else(|| "?".to_owned())
        }
        _ => "?".to_owned(),
    }
}

impl Request {
    /// The kind of request, without its contents (safe to log).
    pub fn kind(&self) -> String {
        variant_name(self)
    }
}

impl Response {
    /// The kind of response, without its contents (safe to log).
    pub fn kind(&self) -> String {
        variant_name(self)
    }
}

/// Whether IPC trace lines are wanted (`AUJAR_IPC_TRACE=1` or `RUST_LOG=aujar::ipc=debug`).
/// Check this before doing any work that only the trace needs.
pub fn trace_enabled() -> bool {
    tracing::enabled!(target: "aujar::ipc", tracing::Level::DEBUG)
}

/// Trace one request the daemon has answered. Call only when [`trace_enabled`].
pub fn trace_handled(kind: &str, started: std::time::Instant, response: &Response) {
    let elapsed_ms = started.elapsed().as_millis() as u64;

    match response {
        Response::Error(message) => tracing::debug!(
            target: "aujar::ipc",
            request = %kind,
            elapsed_ms,
            error = %message,
            "handled, answered with an error"
        ),
        other => tracing::debug!(
            target: "aujar::ipc",
            request = %kind,
            response = %other.kind(),
            elapsed_ms,
            "handled"
        ),
    }
}

fn trace_sent(socket: &Path, kind: &str, started: std::time::Instant, result: &Result<Response>) {
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let socket = socket.display();

    match result {
        Ok(Response::Error(message)) => tracing::debug!(
            target: "aujar::ipc",
            %socket,
            request = %kind,
            elapsed_ms,
            error = %message,
            "sent, the daemon answered with an error"
        ),
        Ok(response) => tracing::debug!(
            target: "aujar::ipc",
            %socket,
            request = %kind,
            response = %response.kind(),
            elapsed_ms,
            "sent"
        ),
        Err(error) => tracing::debug!(
            target: "aujar::ipc",
            %socket,
            request = %kind,
            elapsed_ms,
            error = %format!("{error:#}"),
            "request failed"
        ),
    }
}

/// Serialize `payload` as one protocol line (JSON followed by a newline).
pub fn encode_line<T: Serialize>(payload: T) -> Result<Vec<u8>> {
    encode_line_as(PROTOCOL_VERSION, payload)
}

/// Serialize `payload` as one protocol line in protocol `version` (the daemon answers each
/// client in the version it spoke).
pub fn encode_line_as<T: Serialize>(version: u16, payload: T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&Envelope { version, payload })?;

    bytes.push(b'\n');

    Ok(bytes)
}

/// Only the version of an envelope, read before the payload so a payload from another protocol
/// version is reported as a version mismatch rather than as malformed JSON.
#[derive(Deserialize)]
struct Header {
    version: u16,
}

/// Parse one line from the daemon (a response or an event) and verify the protocol version.
pub fn decode_line<T: DeserializeOwned>(line: &str) -> Result<T> {
    let header: Header = serde_json::from_str(line).context("malformed Aujar IPC message")?;

    if !supports_version(header.version) {
        anyhow::bail!(version_mismatch(header.version, "daemon"));
    }

    let envelope: Envelope<T> = serde_json::from_str(line).with_context(|| {
        "unrecognised Aujar IPC message (is the daemon newer than this program? \
         restart both after upgrading)"
    })?;

    Ok(envelope.payload)
}

/// A request line the daemon read, or why it cannot be served.
#[derive(Debug)]
pub enum Incoming {
    /// Serve `request` and answer in protocol `version`.
    Request { version: u16, request: Request },
    /// Answer `response` (always [`Response::Error`]) in protocol `version`, then close.
    Refused { version: u16, response: Response },
}

impl Incoming {
    /// Decode one request line on the daemon side.
    ///
    /// The refusal is written in the client's own version when the envelope says which one it
    /// is, because [`Response::Error`] reads the same in every version.
    pub fn parse(line: &str) -> Self {
        let version = match serde_json::from_str::<Header>(line) {
            Ok(header) => header.version,
            Err(error) => {
                return Self::Refused {
                    version: PROTOCOL_VERSION,
                    response: Response::Error(format!("malformed request: {error}")),
                };
            }
        };

        if !supports_version(version) {
            return Self::Refused {
                version,
                response: Response::Error(version_mismatch(version, "client")),
            };
        }

        match serde_json::from_str::<Envelope<Request>>(line) {
            Ok(envelope) => Self::Request {
                version,
                request: envelope.payload,
            },
            // Most likely a newer client talking to this daemon: say so instead of hanging up.
            Err(error) => Self::Refused {
                version,
                response: Response::Error(format!(
                    "unrecognised request (is the daemon older than the client? restart it \
                     after upgrading): {error}"
                )),
            },
        }
    }
}

#[cfg(unix)]
async fn connect(socket: impl AsRef<Path>) -> Result<LocalStream> {
    UnixStream::connect(socket)
        .await
        .context("failed to connect to Aujar daemon. Start it with: systemctl --user start aujar")
}

#[cfg(windows)]
async fn connect(socket: impl AsRef<Path>) -> Result<LocalStream> {
    ClientOptions::new()
        .open(socket.as_ref())
        .context("failed to connect to Aujar daemon. Start it with: aujar daemon")
}

async fn open(socket: impl AsRef<Path>, request: Request) -> Result<LocalStream> {
    let mut stream = connect(socket).await?;

    stream.write_all(&encode_line(request)?).await?;

    Ok(stream)
}

async fn read_reply(reader: &mut BufReader<LocalStream>) -> Result<String> {
    let mut line = String::new();

    if reader.read_line(&mut line).await? == 0 {
        anyhow::bail!(
            "the daemon closed the connection without answering; it may be an older version \
             (try `systemctl --user restart aujar`)"
        );
    }

    Ok(line)
}

/// Send one request and wait for its single response.
pub async fn send(socket: impl AsRef<Path>, request: Request) -> Result<Response> {
    let socket = socket.as_ref();
    let traced = trace_enabled().then(|| (request.kind(), std::time::Instant::now()));

    let result = send_untraced(socket, request).await;

    if let Some((kind, started)) = traced {
        trace_sent(socket, &kind, started, &result);
    }

    result
}

async fn send_untraced(socket: &Path, request: Request) -> Result<Response> {
    let stream = open(socket, request).await?;

    let mut reader = BufReader::new(stream);
    let line = read_reply(&mut reader).await?;

    decode_line(&line)
}

/// A live subscription to daemon events.
pub struct EventStream {
    reader: BufReader<LocalStream>,
}

impl EventStream {
    /// Wait for the next event. `Ok(None)` means the daemon closed the stream.
    pub async fn recv(&mut self) -> Result<Option<Event>> {
        let mut line = String::new();

        if self.reader.read_line(&mut line).await? == 0 {
            return Ok(None);
        }

        decode_line(&line).map(Some)
    }
}

/// Subscribe to daemon events (launcher show/hide/toggle, shutdown).
pub async fn subscribe(socket: impl AsRef<Path>) -> Result<EventStream> {
    let stream = open(socket, Request::Subscribe).await?;

    let mut reader = BufReader::new(stream);
    let line = read_reply(&mut reader).await?;

    match decode_line::<Response>(&line)? {
        Response::Subscribed => Ok(EventStream { reader }),
        Response::Error(message) => anyhow::bail!(message),
        other => anyhow::bail!("unexpected response to Subscribe: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execute_requires_capability_and_search_does_not() {
        let execute = Request::Execute(ExecuteRequest {
            id: "apps:x".into(),
        });
        let search = Request::Search(SearchRequest {
            query: "x".into(),
            limit: None,
        });

        assert_eq!(
            execute.required_capability(),
            Some(Capability::ExecuteCommand)
        );
        assert_eq!(search.required_capability(), None);
    }

    #[test]
    fn lines_round_trip_and_reject_other_versions() {
        let line = String::from_utf8(encode_line(Event::Shutdown).unwrap()).unwrap();

        assert!(line.ends_with('\n'));
        assert_eq!(decode_line::<Event>(&line).unwrap(), Event::Shutdown);

        let wrong = r#"{"version":999,"payload":"Shutdown"}"#;
        let error = decode_line::<Event>(wrong).unwrap_err().to_string();
        assert!(error.contains("daemon uses v999"), "{error}");
        assert!(error.contains("restart the daemon"), "{error}");
    }

    #[test]
    fn a_newer_payload_in_a_known_version_is_unrecognised_not_a_mismatch() {
        let line = r#"{"version":1,"payload":"SomethingFromTheFuture"}"#;
        let error = format!("{:#}", decode_line::<Event>(line).unwrap_err());

        assert!(error.contains("unrecognised"), "{error}");
    }

    #[test]
    fn daemon_parses_supported_requests() {
        let line = String::from_utf8(encode_line(Request::Version).unwrap()).unwrap();

        match Incoming::parse(&line) {
            Incoming::Request { version, request } => {
                assert_eq!(version, PROTOCOL_VERSION);
                assert!(matches!(request, Request::Version));
            }
            other => panic!("refused: {other:?}"),
        }
    }

    #[test]
    fn daemon_refuses_other_versions_in_the_clients_version() {
        let line = r#"{"version":7,"payload":"Ping"}"#;

        let Incoming::Refused { version, response } = Incoming::parse(line) else {
            panic!("accepted an unsupported version");
        };

        // A v7 client can read the refusal: same version, frozen `Error` shape.
        assert_eq!(version, 7);
        let encoded = String::from_utf8(encode_line_as(version, &response).unwrap()).unwrap();
        assert!(
            encoded.starts_with(r#"{"version":7,"payload":{"Error":"#),
            "{encoded}"
        );

        let Response::Error(message) = response else {
            panic!("not an error")
        };
        assert!(message.contains("client uses v7"), "{message}");
    }

    #[test]
    fn daemon_refuses_garbage_and_unknown_requests() {
        assert!(matches!(
            Incoming::parse("not json"),
            Incoming::Refused { .. }
        ));
        assert!(matches!(
            Incoming::parse(r#"{"version":1,"payload":"Teleport"}"#),
            Incoming::Refused { version: 1, .. }
        ));
    }

    #[test]
    fn version_response_describes_the_supported_range() {
        let current = VersionResponse::current();

        assert!(current.accepts(PROTOCOL_VERSION));
        assert!(current.accepts(MIN_PROTOCOL_VERSION));
        assert!(!current.accepts(PROTOCOL_VERSION + 1));
        assert!(!current.accepts(0));
    }

    #[test]
    fn picking_a_color_needs_screen_capture() {
        assert_eq!(
            Request::PickColor.required_capability(),
            Some(Capability::ScreenCapture)
        );
    }

    #[test]
    fn the_color_shortcut_and_editor_read_nothing_from_the_screen() {
        for command in [LauncherCommand::ColorPicker, LauncherCommand::ColorEditor] {
            assert_eq!(Request::Launcher(command).required_capability(), None);
        }
    }

    #[test]
    fn subscribe_and_launcher_need_no_capability() {
        assert_eq!(Request::Subscribe.required_capability(), None);
        assert_eq!(
            Request::Launcher(LauncherCommand::Toggle).required_capability(),
            None
        );
    }

    #[test]
    fn awake_status_needs_nothing_and_changes_need_inhibit_sleep() {
        assert_eq!(
            Request::Awake(AwakeRequest::Status).required_capability(),
            None
        );

        for request in [
            AwakeRequest::Off,
            AwakeRequest::Indefinite,
            AwakeRequest::For { seconds: 600 },
        ] {
            assert_eq!(
                Request::Awake(request).required_capability(),
                Some(Capability::InhibitSleep)
            );
        }
    }

    #[test]
    fn awake_requests_and_responses_cross_the_socket_as_lines() {
        let request = Request::Awake(AwakeRequest::For { seconds: 1800 });
        let line = String::from_utf8(encode_line(request.clone()).unwrap()).unwrap();

        assert_eq!(
            format!("{:?}", decode_line::<Request>(line.trim_end()).unwrap()),
            format!("{request:?}")
        );

        let response = Response::Awake(AwakeResponse::Status(AwakeStatus {
            mode: AwakeMode::Timed,
            active: true,
            remaining_seconds: Some(1700),
            keep_screen_on: true,
            error: None,
        }));
        let line = String::from_utf8(encode_line(response.clone()).unwrap()).unwrap();

        assert_eq!(
            format!("{:?}", decode_line::<Response>(line.trim_end()).unwrap()),
            format!("{response:?}")
        );
    }

    #[test]
    fn nexdesk_open_needs_execute_command_and_status_needs_nothing() {
        assert_eq!(
            Request::NexDesk(NexDeskRequest::Status).required_capability(),
            None
        );
        assert_eq!(
            Request::NexDesk(NexDeskRequest::Open).required_capability(),
            Some(Capability::ExecuteCommand)
        );
    }

    #[test]
    fn mouse_borders_capabilities_match_what_the_request_does() {
        let capability = |request| Request::MouseBorders(request).required_capability();

        // Looking, and managing the key, never needs more than the daemon itself.
        assert_eq!(capability(MouseBordersRequest::Status), None);
        assert_eq!(capability(MouseBordersRequest::ShowKey), None);
        assert_eq!(
            capability(MouseBordersRequest::GenerateKey { force: true }),
            None
        );

        assert_eq!(
            capability(MouseBordersRequest::Switch { machine: None }),
            Some(Capability::ControlInput)
        );
        assert_eq!(
            capability(MouseBordersRequest::LockAll),
            Some(Capability::ControlInput)
        );
        assert_eq!(
            capability(MouseBordersRequest::SendFiles {
                machine: "laptop".into(),
                paths: vec!["/tmp/a".into()],
            }),
            Some(Capability::FileSystem)
        );
    }

    #[test]
    fn mouse_borders_messages_round_trip() {
        let request = Request::MouseBorders(MouseBordersRequest::SendFiles {
            machine: "laptop".into(),
            paths: vec!["/tmp/a b".into()],
        });
        let line = String::from_utf8(encode_line(&request).unwrap()).unwrap();

        match decode_line::<Request>(&line).unwrap() {
            Request::MouseBorders(MouseBordersRequest::SendFiles { machine, paths }) => {
                assert_eq!(machine, "laptop");
                assert_eq!(paths, ["/tmp/a b"]);
            }
            other => panic!("unexpected {other:?}"),
        }

        let response = Response::MouseBorders(MouseBordersResponse::Key("AAAA".into()));
        let line = String::from_utf8(encode_line(&response).unwrap()).unwrap();

        assert!(matches!(
            decode_line::<Response>(&line).unwrap(),
            Response::MouseBorders(MouseBordersResponse::Key(key)) if key == "AAAA"
        ));
    }

    #[test]
    fn local_transport_is_selected_at_compile_time() {
        #[cfg(unix)]
        const {
            assert!(cfg!(unix))
        };
        #[cfg(windows)]
        assert!(cfg!(windows));
    }
}

#[cfg(test)]
mod kind_tests {
    use super::*;

    #[test]
    fn kinds_name_the_variant_and_nothing_else() {
        assert_eq!(Request::Ping.kind(), "Ping");
        assert_eq!(
            Request::Launcher(LauncherCommand::Toggle).kind(),
            "Launcher"
        );
        assert_eq!(
            Request::Search(SearchRequest {
                query: "my secret query".into(),
                limit: None,
            })
            .kind(),
            "Search"
        );
        assert_eq!(Response::Pong.kind(), "Pong");
        assert_eq!(Response::Error("boom".into()).kind(), "Error");
    }

    #[test]
    fn a_kind_never_contains_the_request_contents() {
        let kind = Request::Execute(ExecuteRequest {
            id: "apps:secret-file-name".into(),
        })
        .kind();

        assert_eq!(kind, "Execute");
        assert!(!kind.contains("secret"));
    }
}
