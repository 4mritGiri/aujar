use anyhow::{Context, Result};
use aujar_core::{Capability, Health, WindowId};
use serde::{Deserialize, Serialize};
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
}

impl Request {
    /// Capability the daemon must grant before serving this request.
    pub fn required_capability(&self) -> Option<Capability> {
        match self {
            Self::Windows => Some(Capability::ReadWindows),
            Self::Rename(_) => Some(Capability::FileSystem),
            Self::Execute(_) => Some(Capability::ExecuteCommand),
            Self::Ping | Self::Health | Self::Modules | Self::Search(_) => None,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Envelope<T> {
    version: u16,
    payload: T,
}

pub async fn send(socket: impl AsRef<Path>, request: Request) -> Result<Response> {
    let mut stream = UnixStream::connect(socket)
        .await
        .context("failed to connect to Aujar daemon")?;

    let payload = serde_json::to_vec(&Envelope {
        version: PROTOCOL_VERSION,
        payload: request,
    })?;

    stream.write_all(&payload).await?;
    stream.write_all(b"\n").await?;

    let mut reader = BufReader::new(stream);
    let mut line = String::new();

    reader.read_line(&mut line).await?;

    let response: Envelope<Response> = serde_json::from_str(&line)?;

    if response.version != PROTOCOL_VERSION {
        anyhow::bail!(
            "unsupported Aujar IPC protocol version {}",
            response.version
        );
    }

    Ok(response.payload)
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
}
