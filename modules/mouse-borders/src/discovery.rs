//! Finding other machines that are ready to pair, on the local network.
//!
//! A machine that opens a pairing window answers discovery queries (see [`Responder`]); a
//! machine that wants to pair broadcasts one and collects the answers ([`scan`]). Nothing
//! here ever blocks for longer than the timeout it is given, and nothing here carries a
//! secret: an answer holds only the machine's name and the port to pair on.
//!
//! The scan is synchronous on purpose. Call it from a worker thread (the settings window
//! runs it on its background executor) or through [`scan_local_network`], which moves it
//! off the async runtime for you. It must never run on a UI thread.

use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::{Duration, Instant},
};

use aujar_config::MouseBorders;
use tokio::sync::mpsc;

use crate::{
    error::{Error, Result},
    net::{LocalNet, is_same_subnet, local_networks, retry_in_use},
};

/// UDP port for discovery. UDP and TCP port numbers are separate, so this is the same number
/// as the sharing port by default without clashing with it.
pub const DISCOVERY_PORT: u16 = 15101;

/// Magic bytes to identify Mouse Borders discovery packets.
const MAGIC: [u8; 5] = *b"Aujar";

/// Layout of an answer: `MAGIC`, type, flags, TCP port (big endian), name length, name.
const HEADER_LEN: usize = MAGIC.len() + 1;
const MAX_PACKET: usize = 512;

/// The answer says this machine accepts pairing right now.
const FLAG_PAIRING_OPEN: u8 = 0b0000_0001;

/// How long one blocking read waits before the loops check the clock again.
const TICK: Duration = Duration::from_millis(150);

/// Discovery packet types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    /// Broadcast: "I am here, this is my info".
    Announce = 1,
    /// Request: "Who is there?"
    Query = 2,
    /// Response: "I am here too".
    Response = 3,
}

impl PacketType {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::Announce),
            2 => Some(Self::Query),
            3 => Some(Self::Response),
            _ => None,
        }
    }
}

/// Information about a discovered Mouse Borders machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredDevice {
    /// Machine name.
    pub name: String,
    /// IP address it answered from.
    pub address: IpAddr,
    /// TCP port to pair on (when `pairing_open`) or to share on.
    pub port: u16,
    /// Whether the machine is waiting for someone to pair with it.
    pub pairing_open: bool,
}

/// What a machine says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    pub name: String,
    pub port: u16,
    pub pairing_open: bool,
}

fn encode(kind: PacketType, presence: Option<&Presence>) -> Vec<u8> {
    let mut packet = Vec::with_capacity(MAX_PACKET.min(HEADER_LEN + 4 + 64));

    packet.extend_from_slice(&MAGIC);
    packet.push(kind as u8);

    if let Some(presence) = presence {
        let name = presence.name.as_bytes();
        let name = &name[..name.len().min(63)];

        packet.push(if presence.pairing_open {
            FLAG_PAIRING_OPEN
        } else {
            0
        });
        packet.extend_from_slice(&presence.port.to_be_bytes());
        packet.push(name.len() as u8);
        packet.extend_from_slice(name);
    }

    packet
}

/// Parse a packet. Anything malformed is an error, never a panic: this reads bytes from
/// anyone on the network.
fn decode(data: &[u8]) -> Result<(PacketType, Option<Presence>)> {
    let bad = |why: &str| Error::Config(format!("discovery packet: {why}"));

    if data.len() < HEADER_LEN || data[..MAGIC.len()] != MAGIC {
        return Err(bad("wrong magic"));
    }

    let kind = PacketType::from_byte(data[MAGIC.len()]).ok_or_else(|| bad("unknown type"))?;
    let body = &data[HEADER_LEN..];

    if body.is_empty() {
        return Ok((kind, None));
    }

    if body.len() < 4 {
        return Err(bad("truncated"));
    }

    let flags = body[0];
    let port = u16::from_be_bytes([body[1], body[2]]);
    let length = usize::from(body[3]);
    let name = body.get(4..4 + length).ok_or_else(|| bad("truncated name"))?;
    let name = std::str::from_utf8(name).map_err(|_| bad("name is not text"))?;

    // The same rule as for every machine name, so a stranger cannot put odd characters into
    // the settings window.
    let name = crate::layout::MachineName::new(name)
        .map_err(|_| bad("unusable name"))?
        .to_string();

    if port == 0 {
        return Err(bad("port 0"));
    }

    Ok((
        kind,
        Some(Presence {
            name,
            port,
            pairing_open: flags & FLAG_PAIRING_OPEN != 0,
        }),
    ))
}

/// Calculate the broadcast address of an IPv4 network.
fn broadcast_ip(ip: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask = match prefix.min(32) {
        0 => 0,
        bits => u32::MAX << (32 - u32::from(bits)),
    };

    Ipv4Addr::from(u32::from(ip) & mask | !mask)
}

fn is_own_address(address: IpAddr, networks: &[LocalNet]) -> bool {
    networks.iter().any(|network| network.ip == address)
}

/// Broadcast a query and collect answers for `timeout`. Blocks the calling thread for about
/// that long; never longer, however the network behaves.
pub fn scan(timeout: Duration) -> Result<Vec<DiscoveredDevice>> {
    let networks = local_networks();

    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|error| Error::Config(format!("cannot open the discovery socket: {error}")))?;

    socket
        .set_broadcast(true)
        .map_err(|error| Error::Config(format!("cannot enable broadcast: {error}")))?;
    // Without a read timeout `recv_from` waits forever when nobody answers.
    socket
        .set_read_timeout(Some(TICK))
        .map_err(|error| Error::Config(format!("cannot set a read timeout: {error}")))?;

    let query = encode(PacketType::Query, None);
    let mut targets: Vec<Ipv4Addr> = vec![Ipv4Addr::BROADCAST];

    for network in &networks {
        if let IpAddr::V4(ip) = network.ip
            && !ip.is_loopback()
            && !ip.is_link_local()
        {
            let broadcast = broadcast_ip(ip, network.prefix);

            if !targets.contains(&broadcast) {
                targets.push(broadcast);
            }
        }
    }

    let mut sent = 0;

    for target in targets {
        match socket.send_to(&query, SocketAddr::new(IpAddr::V4(target), DISCOVERY_PORT)) {
            Ok(_) => sent += 1,
            Err(error) => tracing::debug!("discovery query to {target} failed: {error}"),
        }
    }

    if sent == 0 {
        return Err(Error::Config(
            "could not send on any network; is this machine connected to one?".into(),
        ));
    }

    let mut found: Vec<DiscoveredDevice> = Vec::new();
    let mut buffer = [0u8; MAX_PACKET];
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        match socket.recv_from(&mut buffer) {
            Ok((size, from)) => {
                let Ok((PacketType::Response | PacketType::Announce, Some(presence))) =
                    decode(&buffer[..size])
                else {
                    continue;
                };

                let address = from.ip();

                if is_own_address(address, &networks)
                    || found.iter().any(|device| device.address == address)
                {
                    continue;
                }

                found.push(DiscoveredDevice {
                    name: presence.name,
                    address,
                    port: presence.port,
                    pairing_open: presence.pairing_open,
                });
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                tracing::debug!("discovery receive failed: {error}");
                break;
            }
        }
    }

    Ok(found)
}

/// [`scan`] for async callers: runs it on a blocking thread so the runtime keeps going.
/// `local_host` is kept for the callers that already pass it.
pub async fn scan_local_network(
    _local_host: IpAddr,
    timeout: Duration,
) -> Result<Vec<DiscoveredDevice>> {
    tokio::task::spawn_blocking(move || scan(timeout))
        .await
        .map_err(|error| Error::Config(format!("the scan stopped: {error}")))?
}

/// Answers discovery queries while it exists. Bind it when a pairing window opens and drop
/// it (or call [`Responder::poll`] no more) when the window closes.
///
/// It owns no thread: whoever runs the pairing loop calls [`Responder::poll`] often, which
/// keeps the loop in one place and stops the answering the moment the loop does.
pub struct Responder {
    socket: UdpSocket,
    presence: Presence,
    networks: Vec<LocalNet>,
    last_refresh: Instant,
}

impl Responder {
    pub fn bind(presence: Presence) -> Result<Self> {
        let socket = retry_in_use(|| UdpSocket::bind(("0.0.0.0", DISCOVERY_PORT))).map_err(|error| {
            Error::Config(format!(
                "cannot listen for discovery on UDP port {DISCOVERY_PORT}: {error}"
            ))
        })?;

        socket
            .set_read_timeout(Some(TICK))
            .map_err(|error| Error::Config(format!("cannot set a read timeout: {error}")))?;

        Ok(Self {
            socket,
            presence,
            networks: local_networks(),
            last_refresh: Instant::now(),
        })
    }

    /// Answer one waiting query, if any (waits up to a tick). Only machines on one of our own
    /// networks get an answer.
    pub fn poll(&mut self) {
        if self.last_refresh.elapsed() > Duration::from_secs(30) {
            self.networks = local_networks();
            self.last_refresh = Instant::now();
        }

        let mut buffer = [0u8; MAX_PACKET];

        let Ok((size, from)) = self.socket.recv_from(&mut buffer) else {
            return;
        };

        if !matches!(decode(&buffer[..size]), Ok((PacketType::Query, _))) {
            return;
        }

        if is_own_address(from.ip(), &self.networks)
            || !is_same_subnet(from.ip(), &self.networks)
        {
            return;
        }

        let answer = encode(PacketType::Response, Some(&self.presence));

        if let Err(error) = self.socket.send_to(&answer, from) {
            tracing::debug!("discovery answer to {from} failed: {error}");
        }
    }
}

/// Start a background task that scans now and then and reports new machines.
pub fn start_discovery(
    _settings: MouseBorders,
    _local_name: String,
    local_host: IpAddr,
) -> Result<mpsc::Receiver<DiscoveredDevice>> {
    let (tx, rx) = mpsc::channel(32);

    tokio::spawn(async move {
        loop {
            if let Ok(devices) = scan_local_network(local_host, Duration::from_secs(2)).await {
                for device in devices {
                    if tx.send(device).await.is_err() {
                        return;
                    }
                }
            }

            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });

    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcast_ip() {
        assert_eq!(
            broadcast_ip(Ipv4Addr::new(192, 168, 1, 10), 24),
            Ipv4Addr::new(192, 168, 1, 255)
        );
        assert_eq!(
            broadcast_ip(Ipv4Addr::new(10, 0, 0, 1), 8),
            Ipv4Addr::new(10, 255, 255, 255)
        );
        assert_eq!(
            broadcast_ip(Ipv4Addr::new(10, 0, 0, 1), 0),
            Ipv4Addr::new(255, 255, 255, 255)
        );
        assert_eq!(
            broadcast_ip(Ipv4Addr::new(10, 0, 0, 1), 32),
            Ipv4Addr::new(10, 0, 0, 1)
        );
    }

    #[test]
    fn a_query_is_just_the_header() {
        let packet = encode(PacketType::Query, None);

        assert_eq!(&packet[..5], b"Aujar");
        assert_eq!(decode(&packet).unwrap(), (PacketType::Query, None));
    }

    #[test]
    fn an_answer_round_trips() {
        let presence = Presence {
            name: "yaman-Latitude-5480".into(),
            port: 15102,
            pairing_open: true,
        };
        let packet = encode(PacketType::Response, Some(&presence));

        assert_eq!(
            decode(&packet).unwrap(),
            (PacketType::Response, Some(presence))
        );
    }

    #[test]
    fn hostile_packets_are_errors_not_panics() {
        assert!(decode(b"").is_err());
        assert!(decode(b"Aujar").is_err());
        assert!(decode(b"Nope!\x03").is_err());
        assert!(decode(b"Aujar\x09").is_err());
        // Truncated body, and a name longer than the packet.
        assert!(decode(b"Aujar\x03\x01\x3b").is_err());
        assert!(decode(b"Aujar\x03\x01\x3b\x06\x09\x40bob").is_err());
        // A name with characters a machine name may not have.
        assert!(decode(b"Aujar\x03\x01\x3b\x06\x03a b").is_err());
        // Port zero.
        assert!(decode(b"Aujar\x03\x01\x00\x00\x01a").is_err());
    }

    #[test]
    fn long_names_are_cut_to_the_limit_and_still_parse() {
        let presence = Presence {
            name: "x".repeat(200),
            port: 7,
            pairing_open: false,
        };
        let (_, parsed) = decode(&encode(PacketType::Response, Some(&presence))).unwrap();

        assert_eq!(parsed.unwrap().name.len(), 63);
    }

    #[test]
    fn a_scan_with_nobody_there_returns_in_time() {
        // The bug this module was rewritten for: a scan nobody answers must still end.
        let started = Instant::now();
        let result = scan(Duration::from_millis(400));

        // Sandboxes without a network may refuse to send; either way it must be quick.
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(result.map_or(true, |found| found.iter().all(|d| d.port != 0)));
    }
}
