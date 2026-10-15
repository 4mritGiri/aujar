//! Connections between machines: accepting, dialing, the Hello exchange and the per-peer
//! reader/writer tasks.
//!
//! A [`PeerLink`] has two outgoing lanes. Input and control traffic use the realtime lane;
//! clipboard and file chunks use the bulk lane. The writer always drains realtime first, so a
//! 100 MB transfer never makes the pointer lag.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, TcpStream},
    sync::{Semaphore, mpsc, watch},
    time::{sleep, timeout},
};

use crate::{
    engine::{Event, PeerUp, Shared},
    error::{Error, Result},
    layout::MachineName,
    net,
    protocol::{Message, PROTOCOL_VERSION},
    secure::{self, SecureReader, SecureWriter},
};

const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Handshakes running at once; further connections are dropped (protects against floods
/// from unauthenticated peers).
const MAX_PENDING_HANDSHAKES: usize = 8;

const REALTIME_LANE: usize = 512;
const BULK_LANE: usize = 32;

/// Outgoing half of a live connection. Dropping every clone of the realtime sender closes it.
#[derive(Clone)]
pub struct PeerLink {
    pub realtime: mpsc::Sender<Message>,
    pub bulk: mpsc::Sender<Message>,
}

fn subnet_allows(shared: &Shared, ip: IpAddr) -> bool {
    !shared.config.same_subnet_only || net::is_same_subnet(ip, &net::local_networks())
}

pub(crate) async fn run_listener(
    listener: TcpListener,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
    mut shutdown: watch::Receiver<bool>,
) {
    let pending = Arc::new(Semaphore::new(MAX_PENDING_HANDSHAKES));

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => accepted,
            _ = shutdown.changed() => return,
        };

        let (stream, remote) = match accepted {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(%error, "accept failed");
                sleep(Duration::from_millis(100)).await;
                continue;
            }
        };

        if !subnet_allows(&shared, remote.ip()) {
            tracing::warn!(%remote, "refused a connection from outside the local subnets");
            continue;
        }

        let Ok(permit) = Arc::clone(&pending).try_acquire_owned() else {
            tracing::warn!(%remote, "too many pending handshakes; dropping connection");
            continue;
        };

        let shared = Arc::clone(&shared);
        let events = events.clone();

        tokio::spawn(async move {
            let _ = stream.set_nodelay(true);

            match secure::accept(stream, &shared.key).await {
                Ok((reader, writer)) => {
                    drop(permit);

                    if let Err(error) =
                        run_link(reader, writer, shared, events, Some(remote), None).await
                    {
                        tracing::debug!(%remote, %error, "inbound link ended");
                    }
                }
                Err(error) => tracing::warn!(%remote, %error, "inbound handshake failed"),
            }
        });
    }
}

/// Keep a connection to `name` alive: dial, run, back off, repeat.
pub(crate) async fn run_dialer(
    name: MachineName,
    address: Option<String>,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
    mut shutdown: watch::Receiver<bool>,
) {
    let timing = shared.config.timing;
    let target = address.unwrap_or_else(|| name.to_string());
    let mut delay = timing.reconnect_min;

    loop {
        if *shutdown.borrow() {
            return;
        }

        // The peer may have connected to us; no need to dial it then.
        if shared.is_connected(&name) {
            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {}
                _ = shared.reconnect.notified() => {}
                _ = shutdown.changed() => return,
            }

            continue;
        }

        // Of two machines, the alphabetically first one dials. The other gives it a moment
        // first, so the two do not open duplicate connections at the same time.
        if name < shared.config.local {
            let deadline = Instant::now() + timing.dial_grace;

            while Instant::now() < deadline && !shared.is_connected(&name) {
                tokio::select! {
                    _ = sleep(Duration::from_millis(25)) => {}
                    _ = shutdown.changed() => return,
                }
            }

            if shared.is_connected(&name) {
                continue;
            }
        }

        let started = Instant::now();

        if let Err(error) = dial_once(&name, &target, &shared, &events).await {
            let _ = events
                .send(Event::DialFailed {
                    name: name.clone(),
                    error: error.to_string(),
                })
                .await;
        }

        delay = if started.elapsed() > Duration::from_secs(10) {
            timing.reconnect_min
        } else {
            (delay * 2).min(timing.reconnect_max)
        };

        tokio::select! {
            _ = sleep(delay) => {}
            _ = shared.reconnect.notified() => delay = timing.reconnect_min,
            _ = shutdown.changed() => return,
        }
    }
}

async fn dial_once(
    name: &MachineName,
    target: &str,
    shared: &Arc<Shared>,
    events: &mpsc::Sender<Event>,
) -> Result<()> {
    let timing = shared.config.timing;
    let addresses = net::resolve(target, shared.config.default_port).await?;

    let mut last_error = Error::Other("no usable address".into());
    let mut connected = None;

    for address in addresses {
        if !subnet_allows(shared, address.ip()) {
            last_error = Error::Config(format!(
                "{address} is outside this machine's subnets (`same_subnet_only` is on)"
            ));
            continue;
        }

        match timeout(timing.connect_timeout, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => {
                connected = Some((stream, address));
                break;
            }
            Ok(Err(error)) => last_error = error.into(),
            Err(_) => last_error = Error::Other(format!("connecting to {address} timed out")),
        }
    }

    let (stream, remote) = connected.ok_or(last_error)?;
    let _ = stream.set_nodelay(true);

    let (reader, writer) = secure::connect(stream, &shared.key).await?;

    run_link(
        reader,
        writer,
        Arc::clone(shared),
        events.clone(),
        Some(remote),
        Some(name.clone()),
    )
    .await
}

/// Exchange Hellos, register the peer with the engine, then pump messages until the
/// connection ends. `expected` is set when we dialed a specific machine.
async fn run_link<R, W>(
    mut reader: SecureReader<R>,
    mut writer: SecureWriter<W>,
    shared: Arc<Shared>,
    events: mpsc::Sender<Event>,
    remote: Option<SocketAddr>,
    expected: Option<MachineName>,
) -> Result<()>
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let timing = shared.config.timing;
    let local = shared.config.local.clone();

    let ours = shared.hello();
    writer.send(&Message::Hello(ours)).await?;

    let hello = match timeout(HELLO_TIMEOUT, reader.recv()).await {
        Ok(Ok(Message::Hello(hello))) => hello,
        Ok(Ok(_)) => return Err(Error::Protocol("expected Hello first".into())),
        Ok(Err(error)) => {
            return Err(Error::Handshake(format!(
                "the other machine did not accept the security key ({error})"
            )));
        }
        Err(_) => return Err(Error::Protocol("no Hello received".into())),
    };

    let peer = hello.machine.clone();
    let refusal = if hello.version != PROTOCOL_VERSION {
        Some(format!(
            "protocol version {} is not supported (this is {PROTOCOL_VERSION})",
            hello.version
        ))
    } else if peer == local {
        Some("a machine cannot connect to itself; names must be unique".to_owned())
    } else if !shared.config.layout.contains(&peer) {
        Some(format!("`{peer}` is not in this machine's layout"))
    } else if expected.as_ref().is_some_and(|expected| *expected != peer) {
        Some(format!(
            "expected `{}` but reached `{peer}`",
            expected.as_ref().unwrap()
        ))
    } else {
        None
    };

    if let Some(reason) = refusal {
        let _ = writer
            .send(&Message::Bye {
                reason: reason.clone(),
            })
            .await;
        writer.shutdown().await;

        return Err(Error::Handshake(reason));
    }

    let (realtime_tx, mut realtime_rx) = mpsc::channel::<Message>(REALTIME_LANE);
    let (bulk_tx, mut bulk_rx) = mpsc::channel::<Message>(BULK_LANE);
    let conn_id = shared.next_connection_id();

    let writer_task = tokio::spawn(async move {
        let mut heartbeat = tokio::time::interval(timing.heartbeat);
        let mut bulk_open = true;

        loop {
            let message = tokio::select! {
                biased;
                message = realtime_rx.recv() => match message {
                    Some(message) => message,
                    None => break,
                },
                message = bulk_rx.recv(), if bulk_open => match message {
                    Some(message) => message,
                    None => {
                        bulk_open = false;
                        continue;
                    }
                },
                _ = heartbeat.tick() => Message::Heartbeat,
            };

            if writer.send(&message).await.is_err() {
                break;
            }
        }

        writer.shutdown().await;
    });

    let up = PeerUp {
        name: peer.clone(),
        conn_id,
        initiator: if expected.is_some() {
            local
        } else {
            peer.clone()
        },
        hello,
        link: PeerLink {
            realtime: realtime_tx,
            bulk: bulk_tx,
        },
        remote,
    };

    if events.send(Event::PeerUp(up)).await.is_err() {
        writer_task.abort();

        return Ok(());
    }

    let outcome = loop {
        match timeout(timing.peer_timeout, reader.recv()).await {
            Err(_) => break Err(Error::Protocol("peer stopped responding".into())),
            Ok(Err(error)) => break Err(error),
            Ok(Ok(Message::Heartbeat)) => {}
            Ok(Ok(message)) => {
                let event = Event::Peer {
                    name: peer.clone(),
                    conn_id,
                    message,
                };

                if events.send(event).await.is_err() {
                    break Ok(());
                }
            }
        }
    };

    writer_task.abort();

    let _ = events
        .send(Event::PeerDown {
            name: peer,
            conn_id,
            error: outcome.as_ref().err().map(ToString::to_string),
        })
        .await;

    outcome
}
