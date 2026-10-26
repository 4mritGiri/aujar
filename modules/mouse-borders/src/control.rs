//! The local control channel of the standalone Mouse Borders agent.
//!
//! The `aujar-mwb` command talks to the running agent over this channel: a Unix domain socket
//! on Linux and macOS, a named pipe on Windows. Requests and replies are single lines of JSON
//! (the same [`MouseBordersRequest`] / [`MouseBordersResponse`] the settings window and the
//! daemon use). The channel is local-only and private to the user:
//!
//! * the socket lives in the user's runtime directory with mode `0600`, and connections from
//!   other users are refused (Unix);
//! * the pipe is created with `reject_remote_clients` and a per-user name (Windows).

#[cfg(unix)]
use std::path::PathBuf;
use std::{future::Future, time::Duration};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    sync::watch,
    time::timeout,
};

use crate::{
    api::{self, MouseBordersRequest, MouseBordersResponse},
    error::{Error, Result},
    service::ServiceHandle,
};

/// Longest request or reply line accepted.
const MAX_LINE: usize = 256 * 1024;

/// How long a client waits for an answer.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

/// What the agent answers with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    Ok(MouseBordersResponse),
    Error(String),
}

/// Something that answers control requests.
#[async_trait]
pub trait Handler: Send + Sync + 'static {
    async fn handle(
        &self,
        request: MouseBordersRequest,
    ) -> std::result::Result<MouseBordersResponse, String>;
}

#[async_trait]
impl Handler for ServiceHandle {
    async fn handle(
        &self,
        request: MouseBordersRequest,
    ) -> std::result::Result<MouseBordersResponse, String> {
        api::handle(request, self).await
    }
}

/// Where the control channel lives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A Unix domain socket path.
    #[cfg(unix)]
    Socket(PathBuf),
    /// A Windows named pipe, e.g. `\\.\pipe\aujar-mwb-alice`.
    #[cfg(windows)]
    Pipe(String),
}

impl Endpoint {
    /// The channel of the current user's agent.
    pub fn user_default() -> Self {
        #[cfg(unix)]
        {
            Self::Socket(aujar_config::platform::runtime_dir().join("aujar-mwb.sock"))
        }

        #[cfg(windows)]
        {
            let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_owned());
            let user: String = user
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();

            Self::Pipe(format!(r"\\.\pipe\aujar-mwb-{user}"))
        }
    }

    pub fn describe(&self) -> String {
        match self {
            #[cfg(unix)]
            Self::Socket(path) => path.display().to_string(),
            #[cfg(windows)]
            Self::Pipe(name) => name.clone(),
        }
    }
}

/// Read one line of at most [`MAX_LINE`] bytes. `None` at end of stream.
async fn read_line<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> Result<Option<String>> {
    let mut line = String::new();
    let read = (&mut *reader)
        .take(MAX_LINE as u64 + 1)
        .read_line(&mut line)
        .await?;

    if read == 0 {
        return Ok(None);
    }

    if line.len() > MAX_LINE {
        return Err(Error::Protocol("control message too long".into()));
    }

    Ok(Some(line))
}

/// Answer requests on one connection until it closes.
pub async fn serve_connection<S>(stream: S, handler: &dyn Handler) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);

    while let Some(line) = read_line(&mut reader).await? {
        let reply = match serde_json::from_str::<MouseBordersRequest>(line.trim()) {
            Ok(request) => match handler.handle(request).await {
                Ok(response) => Reply::Ok(response),
                Err(message) => Reply::Error(message),
            },
            Err(error) => Reply::Error(format!("not a valid request: {error}")),
        };

        let mut out =
            serde_json::to_vec(&reply).map_err(|error| Error::Protocol(error.to_string()))?;

        out.push(b'\n');
        write.write_all(&out).await?;
        write.flush().await?;
    }

    Ok(())
}

/// Send one request on an open connection and read the answer.
pub async fn exchange<S>(stream: S, request: &MouseBordersRequest) -> Result<MouseBordersResponse>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut line =
        serde_json::to_vec(request).map_err(|error| Error::Protocol(error.to_string()))?;

    line.push(b'\n');
    write.write_all(&line).await?;
    write.flush().await?;

    let mut reader = BufReader::new(read);
    let answer = read_line(&mut reader).await?.ok_or_else(|| {
        Error::Protocol("the agent closed the connection without answering".into())
    })?;

    match serde_json::from_str::<Reply>(answer.trim())
        .map_err(|error| Error::Protocol(format!("unreadable answer from the agent: {error}")))?
    {
        Reply::Ok(response) => Ok(response),
        Reply::Error(message) => Err(Error::Other(message)),
    }
}

/// Ask the running agent. Fails with a clear message when none is running.
pub async fn request(
    endpoint: &Endpoint,
    request: &MouseBordersRequest,
) -> Result<MouseBordersResponse> {
    timeout(CLIENT_TIMEOUT, async {
        let stream = connect(endpoint).await.map_err(|error| {
            Error::Other(format!(
                "the Mouse Borders agent is not running ({}: {error}). Start it with `aujar-mwb run`",
                endpoint.describe()
            ))
        })?;

        exchange(stream, request).await
    })
    .await
    .map_err(|_| Error::Other("the Mouse Borders agent did not answer".into()))?
}

#[cfg(unix)]
async fn connect(endpoint: &Endpoint) -> std::io::Result<tokio::net::UnixStream> {
    let Endpoint::Socket(path) = endpoint;

    tokio::net::UnixStream::connect(path).await
}

#[cfg(windows)]
async fn connect(
    endpoint: &Endpoint,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    use tokio::net::windows::named_pipe::ClientOptions;

    /// `ERROR_PIPE_BUSY`: every instance is serving someone; try again in a moment.
    const PIPE_BUSY: i32 = 231;

    let Endpoint::Pipe(name) = endpoint;

    for _ in 0..20 {
        match ClientOptions::new().open(name) {
            Err(error) if error.raw_os_error() == Some(PIPE_BUSY) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            other => return other,
        }
    }

    ClientOptions::new().open(name)
}

/// Run the control channel until `shutdown` turns true.
#[cfg(unix)]
pub async fn serve(
    endpoint: &Endpoint,
    handler: std::sync::Arc<dyn Handler>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let Endpoint::Socket(path) = endpoint;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // A socket file may be left over from a crash; a live agent answers on it.
    if path.exists() {
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            return Err(Error::Other(format!(
                "another Mouse Borders agent is already running ({})",
                path.display()
            )));
        }

        std::fs::remove_file(path)?;
    }

    let listener = tokio::net::UnixListener::bind(path)?;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;

    let owner = std::fs::metadata(path)?.uid();

    let result = loop {
        let (stream, _) = tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok(connection) => connection,
                Err(error) => break Err(Error::from(error)),
            },
            _ = shutdown.changed() => break Ok(()),
        };

        // Belt and braces next to the file mode: only our own user may talk to us.
        if stream.peer_cred().map(|cred| cred.uid()).ok() != Some(owner) {
            tracing::warn!("refused a control connection from another user");
            continue;
        }

        let handler = std::sync::Arc::clone(&handler);

        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, handler.as_ref()).await {
                tracing::debug!(%error, "control connection ended");
            }
        });
    };

    let _ = std::fs::remove_file(path);

    result
}

/// Run the control channel until `shutdown` turns true.
#[cfg(windows)]
pub async fn serve(
    endpoint: &Endpoint,
    handler: std::sync::Arc<dyn Handler>,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    use tokio::net::windows::named_pipe::ServerOptions;

    let Endpoint::Pipe(name) = endpoint;

    let mut server = ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(name)
        .map_err(|error| {
            Error::Other(format!(
                "cannot create the control pipe {name} (is an agent already running?): {error}"
            ))
        })?;

    loop {
        tokio::select! {
            connected = server.connect() => connected?,
            _ = shutdown.changed() => return Ok(()),
        }

        // Open the next instance before handing this one off, so clients never find no pipe.
        let next = ServerOptions::new()
            .reject_remote_clients(true)
            .create(name)?;
        let connection = std::mem::replace(&mut server, next);
        let handler = std::sync::Arc::clone(&handler);

        tokio::spawn(async move {
            if let Err(error) = serve_connection(connection, handler.as_ref()).await {
                tracing::debug!(%error, "control connection ended");
            }
        });
    }
}

/// Convenience for the agent: serve until shutdown and report errors through `on_error`.
pub async fn run<F, Fut>(
    endpoint: Endpoint,
    handler: std::sync::Arc<dyn Handler>,
    shutdown: watch::Receiver<bool>,
    on_error: F,
) where
    F: FnOnce(Error) -> Fut,
    Fut: Future<Output = ()>,
{
    if let Err(error) = serve(&endpoint, handler, shutdown).await {
        on_error(error).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::duplex;

    /// Remembers what it was asked and answers from a script.
    #[derive(Default)]
    struct Script {
        seen: Mutex<Vec<MouseBordersRequest>>,
    }

    #[async_trait]
    impl Handler for Script {
        async fn handle(
            &self,
            request: MouseBordersRequest,
        ) -> std::result::Result<MouseBordersResponse, String> {
            self.seen.lock().unwrap().push(request.clone());

            match request {
                MouseBordersRequest::LockAll => Ok(MouseBordersResponse::Done("locked".into())),
                MouseBordersRequest::ShowKey => Ok(MouseBordersResponse::NoKey),
                other => Err(format!("cannot {other:?}")),
            }
        }
    }

    async fn ask(
        handler: Arc<Script>,
        request: MouseBordersRequest,
    ) -> Result<MouseBordersResponse> {
        let (client, server) = duplex(64 * 1024);
        let task = tokio::spawn(async move { serve_connection(server, handler.as_ref()).await });
        let answer = exchange(client, &request).await;

        let _ = task.await;

        answer
    }

    #[tokio::test]
    async fn answers_travel_both_ways_and_errors_stay_errors() {
        let script = Arc::new(Script::default());

        assert_eq!(
            ask(script.clone(), MouseBordersRequest::LockAll)
                .await
                .unwrap(),
            MouseBordersResponse::Done("locked".into())
        );
        assert_eq!(
            ask(script.clone(), MouseBordersRequest::ShowKey)
                .await
                .unwrap(),
            MouseBordersResponse::NoKey
        );

        let error = ask(script.clone(), MouseBordersRequest::Reload)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("cannot Reload"), "{error}");
        assert_eq!(script.seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn one_connection_can_carry_several_requests() {
        let script = Arc::new(Script::default());
        let (client, server) = duplex(64 * 1024);
        let handler = Arc::clone(&script);
        let task = tokio::spawn(async move { serve_connection(server, handler.as_ref()).await });

        let (read, mut write) = tokio::io::split(client);
        let mut reader = BufReader::new(read);

        for _ in 0..3 {
            write.write_all(b"\"LockAll\"\n").await.unwrap();

            let line = read_line(&mut reader).await.unwrap().unwrap();

            assert!(line.contains("locked"), "{line}");
        }

        drop(write);
        drop(reader);
        task.await.unwrap().unwrap();
        assert_eq!(script.seen.lock().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn garbage_gets_an_error_reply_not_a_crash() {
        let script = Arc::new(Script::default());
        let (client, server) = duplex(64 * 1024);
        let handler = Arc::clone(&script);
        let task = tokio::spawn(async move { serve_connection(server, handler.as_ref()).await });

        let (read, mut write) = tokio::io::split(client);
        let mut reader = BufReader::new(read);

        write.write_all(b"this is not json\n").await.unwrap();
        let line = read_line(&mut reader).await.unwrap().unwrap();

        assert!(line.contains("not a valid request"), "{line}");
        assert!(script.seen.lock().unwrap().is_empty());

        drop(write);
        drop(reader);
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn oversized_messages_are_refused() {
        let script = Arc::new(Script::default());
        let (client, server) = duplex(1 << 20);
        let handler = Arc::clone(&script);
        let task = tokio::spawn(async move { serve_connection(server, handler.as_ref()).await });

        let (_read, mut write) = tokio::io::split(client);
        let huge = vec![b'a'; MAX_LINE + 10];

        let _ = write.write_all(&huge).await;
        let _ = write.write_all(b"\n").await;
        drop(write);

        let outcome = task.await.unwrap();

        assert!(outcome.is_err(), "an endless line must not be buffered");
    }

    #[tokio::test]
    async fn a_missing_agent_is_explained() {
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let endpoint = Endpoint::Socket(dir.path().join("nobody.sock"));

            let error = request(&endpoint, &MouseBordersRequest::Status)
                .await
                .unwrap_err()
                .to_string();

            assert!(error.contains("not running"), "{error}");
            assert!(error.contains("aujar-mwb run"), "{error}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_real_socket_serves_clients_and_cleans_up() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run").join("agent.sock");
        let endpoint = Endpoint::Socket(path.clone());
        let (stop, shutdown) = watch::channel(false);
        let handler: Arc<dyn Handler> = Arc::new(Script::default());

        let server = tokio::spawn({
            let endpoint = endpoint.clone();
            async move { serve(&endpoint, handler, shutdown).await }
        });

        for _ in 0..100 {
            if path.exists() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            request(&endpoint, &MouseBordersRequest::LockAll)
                .await
                .unwrap(),
            MouseBordersResponse::Done("locked".into())
        );

        // A second agent must refuse to start while the first one answers.
        let (_stop2, shutdown2) = watch::channel(false);
        let second = serve(&endpoint, Arc::new(Script::default()), shutdown2)
            .await
            .unwrap_err();

        assert!(second.to_string().contains("already running"), "{second}");

        stop.send(true).unwrap();
        server.await.unwrap().unwrap();
        assert!(
            !path.exists(),
            "the socket file must be removed on shutdown"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stale_socket_file_from_a_crash_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("agent.sock");
        // A socket file nobody listens on.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        let (stop, shutdown) = watch::channel(false);
        let endpoint = Endpoint::Socket(path.clone());
        let server = tokio::spawn({
            let endpoint = endpoint.clone();
            async move { serve(&endpoint, Arc::new(Script::default()), shutdown).await }
        });

        for _ in 0..100 {
            if request(&endpoint, &MouseBordersRequest::LockAll)
                .await
                .is_ok()
            {
                break;
            }

            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert!(
            request(&endpoint, &MouseBordersRequest::LockAll)
                .await
                .is_ok()
        );

        stop.send(true).unwrap();
        server.await.unwrap().unwrap();
    }
}
