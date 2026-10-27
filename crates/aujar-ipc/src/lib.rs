use anyhow::{Context, Result};
use aujar_core::{Capability, Health, WindowId};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::path::Path;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
};

pub const PROTOCOL_VERSION: u16 = 1;

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
}

/// Mouse Borders request and response types, shared with the standalone agent.
pub use aujar_mouse_borders::api::{
    MouseBordersPeer, MouseBordersRequest, MouseBordersResponse, MouseBordersStatus,
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
            Self::Ping
            | Self::Health
            | Self::Modules
            | Self::Search(_)
            | Self::Subscribe
            | Self::Status
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
    /// A launcher command was broadcast to this many subscribers.
    Delivered {
        subscribers: usize,
    },
    Error(String),
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

/// Serialize `payload` as one protocol line (JSON followed by a newline).
pub fn encode_line<T: Serialize>(payload: T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        payload,
    })?;

    bytes.push(b'\n');

    Ok(bytes)
}

/// Parse one protocol line and verify the protocol version.
pub fn decode_line<T: DeserializeOwned>(line: &str) -> Result<T> {
    let envelope: Envelope<T> = serde_json::from_str(line)?;

    if envelope.version != PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported Aujar IPC protocol version {}",
            envelope.version
        );
    }

    Ok(envelope.payload)
}

async fn open(socket: impl AsRef<Path>, request: Request) -> Result<UnixStream> {
    let mut stream = UnixStream::connect(socket)
        .await
        .context("failed to connect to Aujar daemon")?;

    stream.write_all(&encode_line(request)?).await?;

    Ok(stream)
}

async fn read_reply(reader: &mut BufReader<UnixStream>) -> Result<String> {
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
    let stream = open(socket, request).await?;

    let mut reader = BufReader::new(stream);
    let line = read_reply(&mut reader).await?;

    decode_line(&line)
}

/// A live subscription to daemon events.
pub struct EventStream {
    reader: BufReader<UnixStream>,
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
        assert!(decode_line::<Event>(wrong).is_err());
    }

    #[test]
    fn picking_a_color_needs_screen_capture() {
        assert_eq!(
            Request::PickColor.required_capability(),
            Some(Capability::ScreenCapture)
        );
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
}
