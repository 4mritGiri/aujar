//! Windows named-pipe transport for the Aujar daemon.
//!
//! The same versioned JSON-line protocol as the Unix socket: one request per connection, or
//! `Subscribe` to receive events (launcher, Aujar Snip). Served here: `Ping`, `Version`,
//! `Health`, `Status`, `Launcher`, `Subscribe` and `CaptureScreen`; other requests answer that
//! Windows does not have them yet.

use anyhow::{Context, Result};
use aujar_core::{Health, Platform, Session};
use aujar_ipc::{
    Event, Incoming, Request, Response, StatusResponse, VersionResponse, encode_line_as,
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
    sync::broadcast,
};

/// At most this many UIs subscribed at once (as on Unix).
const MAX_SUBSCRIBERS: usize = 16;
/// Events a slow subscriber may fall behind before it loses the oldest.
const EVENT_BUFFER: usize = 64;
/// Longest request line.
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

/// Another daemon already owns the pipe. Not a crash: nothing should restart us.
#[derive(Debug)]
pub struct AlreadyRunning {
    pub socket: PathBuf,
}

impl std::fmt::Display for AlreadyRunning {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "an Aujar daemon is already running on {}",
            self.socket.display()
        )
    }
}

impl std::error::Error for AlreadyRunning {}

/// Exit status for "another instance is already running" (`EX_TEMPFAIL`, as on Unix).
pub const EXIT_ALREADY_RUNNING: i32 = 75;

struct Shared {
    events: broadcast::Sender<Event>,
    subscribers: AtomicUsize,
}

fn create(pipe: &Path, first: bool) -> std::io::Result<NamedPipeServer> {
    let mut options = ServerOptions::new();

    options.reject_remote_clients(true);
    options.first_pipe_instance(first);
    options.create(pipe)
}

/// Run the Windows daemon on `pipe` (`\\.\pipe\aujar`) until Ctrl-C.
pub async fn run(pipe: impl AsRef<Path>) -> Result<()> {
    let pipe = pipe.as_ref().to_path_buf();

    // The first instance proves nobody else owns the name.
    let mut server = match create(&pipe, true) {
        Ok(server) => server,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(AlreadyRunning { socket: pipe }.into());
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to create the Aujar pipe {}", pipe.display()));
        }
    };

    let (events, _) = broadcast::channel(EVENT_BUFFER);
    let shared = Arc::new(Shared {
        events,
        subscribers: AtomicUsize::new(0),
    });

    tracing::info!(pipe = %pipe.display(), "Aujar daemon listening");

    loop {
        tokio::select! {
            connected = server.connect() => {
                connected.context("failed to accept a pipe client")?;

                // Make the next instance before handing this one off, so a client never finds
                // the pipe missing.
                let client = std::mem::replace(
                    &mut server,
                    create(&pipe, false).context("failed to create the next pipe instance")?,
                );
                let shared = shared.clone();

                tokio::spawn(async move {
                    if let Err(error) = serve(client, &shared).await {
                        tracing::debug!(error = %format!("{error:#}"), "pipe client ended");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => {
                let _ = shared.events.send(Event::Shutdown);
                tracing::info!("Aujar daemon stopped");
                return Ok(());
            }
        }
    }
}

async fn serve(server: NamedPipeServer, shared: &Shared) -> Result<()> {
    let (reader, mut writer) = tokio::io::split(server);
    let mut reader = BufReader::new(tokio::io::AsyncReadExt::take(reader, MAX_REQUEST_BYTES));
    let mut line = String::new();

    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        reader.read_line(&mut line),
    )
    .await
    .context("timed out reading the IPC request")??;

    let (version, request) = match Incoming::parse(&line) {
        Incoming::Request { version, request } => (version, request),
        Incoming::Refused { version, response } => {
            writer
                .write_all(&encode_line_as(version, response)?)
                .await?;
            return Ok(());
        }
    };

    if matches!(request, Request::Subscribe) {
        return stream_events(reader, writer, shared, version).await;
    }

    let response = dispatch(request, shared).await;

    writer
        .write_all(&encode_line_as(version, response)?)
        .await?;
    writer.flush().await?;

    Ok(())
}

async fn stream_events<R, W>(
    mut reader: R,
    mut writer: W,
    shared: &Shared,
    version: u16,
) -> Result<()>
where
    R: AsyncBufReadExt + Unpin,
    W: AsyncWriteExt + Unpin,
{
    if shared.subscribers.fetch_add(1, Ordering::SeqCst) >= MAX_SUBSCRIBERS {
        shared.subscribers.fetch_sub(1, Ordering::SeqCst);
        let error = Response::Error("too many event subscribers".into());
        writer.write_all(&encode_line_as(version, error)?).await?;
        return Ok(());
    }

    // Subscribe before acknowledging so no event slips in between.
    let mut events = shared.events.subscribe();
    let result: Result<()> = async {
        writer
            .write_all(&encode_line_as(version, Response::Subscribed)?)
            .await?;

        let mut ignored = String::new();

        loop {
            tokio::select! {
                received = events.recv() => match received {
                    Ok(event) => {
                        writer.write_all(&encode_line_as(version, &event)?).await?;

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
    .await;

    shared.subscribers.fetch_sub(1, Ordering::SeqCst);

    result
}

async fn dispatch(request: Request, shared: &Shared) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Version => Response::Version(VersionResponse::current()),
        Request::Health => Response::Health(Health {
            name: "Aujar".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: Platform::Windows,
            session: Session::Windows,
        }),
        Request::Status => Response::Status(StatusResponse {
            subscribers: shared.subscribers.load(Ordering::SeqCst),
            hotkeys:
                "Windows: bind a shortcut to `aujar snip` (global hotkeys are not built in yet)"
                    .into(),
        }),
        Request::Launcher(command) => Response::Delivered {
            subscribers: shared.events.send(Event::Launcher(command)).unwrap_or(0),
        },
        Request::CaptureScreen => crate::screen::capture().await,
        Request::ExtractText(_) => Response::Error(
            "Windows has no desktop area selection to borrow: use Aujar Snip's overlay \
             (`aujar text` with the Aujar UI running, or `aujar snip` then T)"
                .into(),
        ),
        request => Response::Error(format!(
            "{} is not available on Windows yet",
            request.kind()
        )),
    }
}
