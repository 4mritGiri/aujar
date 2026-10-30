//! Windows named-pipe transport backend for the Aujar daemon.

use anyhow::{Context, Result};
use aujar_core::{Health, Platform, Session};
use aujar_ipc::{Envelope, PROTOCOL_VERSION, Request, Response, StatusResponse, encode_line};
use std::path::Path;
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::windows::named_pipe::{NamedPipeServer, ServerOptions},
};

/// Run the Windows transport host. It uses the same versioned protocol as the Unix backend.
pub async fn run(pipe: impl AsRef<Path>) -> Result<()> {
    let pipe = pipe.as_ref().to_path_buf();
    let mut first_instance = true;

    loop {
        let mut options = ServerOptions::new();
        options.reject_remote_clients(true);
        options.first_pipe_instance(first_instance);
        let server = options
            .create(&pipe)
            .with_context(|| format!("failed to create Aujar named pipe {}", pipe.display()))?;
        first_instance = false;

        tokio::spawn(async move {
            if let Err(error) = serve(server).await {
                tracing::warn!(%error, "Windows IPC client failed");
            }
        });
    }
}

async fn serve(mut server: NamedPipeServer) -> Result<()> {
    server
        .connect()
        .await
        .context("failed to accept pipe client")?;

    let mut line = String::new();
    let mut reader = BufReader::new(&mut server);
    reader
        .read_line(&mut line)
        .await
        .context("failed to read IPC request")?;
    drop(reader);

    let response = match serde_json::from_str::<Envelope<Request>>(&line) {
        Ok(envelope) if envelope.version == PROTOCOL_VERSION => dispatch(envelope.payload),
        Ok(envelope) => {
            Response::Error(format!("unsupported protocol version {}", envelope.version))
        }
        Err(error) => Response::Error(format!("unrecognised request: {error}")),
    };
    server.write_all(&encode_line(response)?).await?;
    server.flush().await?;
    Ok(())
}

fn dispatch(request: Request) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Health => Response::Health(Health {
            name: "Aujar".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            platform: Platform::Windows,
            session: Session::Windows,
        }),
        Request::Status => Response::Status(StatusResponse {
            subscribers: 0,
            hotkeys: "Windows hotkey backend is not implemented yet".into(),
        }),
        request => Response::Error(format!(
            "{:?} is not implemented by the Windows daemon backend yet",
            request
        )),
    }
}
