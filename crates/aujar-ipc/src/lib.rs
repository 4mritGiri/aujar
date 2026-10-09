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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LauncherCommand {
    Show,
    Hide,
    Toggle,
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
            Self::Ping
            | Self::Health
            | Self::Modules
            | Self::Search(_)
            | Self::Subscribe
            | Self::Launcher(_) => None,
        }
    }
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
    /// Maximum results (daemon clamps to 1..=50, default 10).
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

/// Send one request and wait for its single response.
pub async fn send(socket: impl AsRef<Path>, request: Request) -> Result<Response> {
    let stream = open(socket, request).await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    reader.read_line(&mut line).await?;

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
    let mut line = String::new();

    reader.read_line(&mut line).await?;

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
    fn subscribe_and_launcher_need_no_capability() {
        assert_eq!(Request::Subscribe.required_capability(), None);
        assert_eq!(
            Request::Launcher(LauncherCommand::Toggle).required_capability(),
            None
        );
    }
}
