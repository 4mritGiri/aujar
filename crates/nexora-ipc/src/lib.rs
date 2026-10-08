use anyhow::{Context, Result};
use nexora_core::{Health, WindowId};
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
    Rename(RenameRequest),
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
    Rename(RenameResponse),
    Error(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameResponse {
    pub plan: nexora_rename::RenamePlan,
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
        .context("failed to connect to Nexora daemon")?;

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
            "unsupported Nexora IPC protocol version {}",
            response.version
        );
    }
    Ok(response.payload)
}
