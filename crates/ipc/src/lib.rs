use anyhow::Result;
use powertoys_core::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

pub async fn send(socket: &std::path::Path, req: &Request) -> Result<Response> {
    let stream=UnixStream::connect(socket).await?;
    let (r, mut w)=stream.into_split();
    w.write_all(serde_json::to_string(req)?.as_bytes()).await?; w.write_all(b"\n").await?;
    let mut line=String::new(); BufReader::new(r).read_line(&mut line).await?;
    Ok(serde_json::from_str(line.trim())?)
}
