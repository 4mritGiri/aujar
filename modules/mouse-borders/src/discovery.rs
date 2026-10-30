//! Auto-discovery of Mouse Borders instances on the local network.
//!
//! Scans for other machines running Mouse Borders on the same subnet using UDP multicast
//! or broadcast. When a machine starts, it broadcasts its presence, and others can detect it.

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    time::Duration,
};

use aujar_config::MouseBorders;
use tokio::sync::mpsc;

use crate::{
    error::{Error, Result},
    net::local_networks,
};

/// UDP port for Mouse Borders discovery (same as the main service port by default)
const DISCOVERY_PORT: u16 = 15101;

/// Magic bytes to identify Mouse Borders discovery packets
const MAGIC: [u8; 5] = *b"Aujar";

/// Discovery packet types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PacketType {
    /// Broadcast: "I am here, this is my info"
    Announce = 1,
    /// Request: "Who is there?"
    Query = 2,
    /// Response: "I am here too"
    Response = 3,
}

/// Information about a discovered Mouse Borders instance
#[derive(Debug, Clone)]
pub struct DiscoveredDevice {
    /// Machine name
    pub name: String,
    /// IP address
    pub address: IpAddr,
    /// Port
    pub port: u16,
    /// Machine slot in the layout
    pub slot: u8,
}

/// Start a background task that periodically announces this machine's presence
/// and listens for announcements from other machines.
pub fn start_discovery(
    _settings: MouseBorders,
    _local_name: String,
    local_host: IpAddr,
) -> Result<mpsc::Receiver<DiscoveredDevice>> {
    let (tx, rx) = mpsc::channel(32);

    tokio::spawn(async move {
        if let Err(e) = run_discovery(local_host, tx).await {
            tracing::error!("Discovery task failed: {}", e);
        }
    });

    Ok(rx)
}

/// Scan the local network for Mouse Borders instances
pub async fn scan_local_network(
    local_host: IpAddr,
    timeout: Duration,
) -> Result<Vec<DiscoveredDevice>> {
    let networks = local_networks();
    let mut discovered = Vec::new();

    // Create UDP socket for broadcast
    let socket = UdpSocket::bind("0.0.0.0:0")
        .map_err(|e| Error::Config(format!("Failed to create discovery socket: {}", e)))?;

    socket
        .set_broadcast(true)
        .map_err(|e| Error::Config(format!("Failed to enable broadcast: {}", e)))?;

    // Send query to broadcast address on each local network
    let query_packet = build_packet(PacketType::Query, local_host);

    for network in &networks {
        if let IpAddr::V4(ip) = network.ip {
            let broadcast_ip = broadcast_ip(ip, network.prefix);
            let broadcast_addr = SocketAddr::new(IpAddr::V4(broadcast_ip), DISCOVERY_PORT);

            if let Err(e) = socket.send_to(&query_packet, broadcast_addr) {
                tracing::debug!(
                    "Failed to send discovery query to {}: {}",
                    broadcast_addr,
                    e
                );
            } else {
                tracing::debug!("Sent discovery query to {}", broadcast_addr);
            }
        }
    }

    // Listen for responses
    let mut buf = [0u8; 1024];
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        match socket.recv_from(&mut buf) {
            Ok((size, addr)) => {
                if let Ok(device) = parse_packet(&buf[..size], addr.ip()) {
                    // Only add if not already discovered and not ourselves
                    if !discovered.iter().any(|d: &DiscoveredDevice| d.address == device.address)
                        && device.address != local_host
                    {
                        discovered.push(device);
                    }
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                // No data available, wait a bit
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) => {
                tracing::debug!("Discovery recv error: {}", e);
                break;
            }
        }
    }

    Ok(discovered)
}

/// Calculate broadcast IP for a given IP and prefix
fn broadcast_ip(ip: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let ip_bits = u32::from(ip);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    };
    let network = ip_bits & mask;
    Ipv4Addr::from(network | (!mask))
}

/// Build a discovery packet
fn build_packet(packet_type: PacketType, ip: IpAddr) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.extend_from_slice(&MAGIC);
    packet.push(packet_type as u8);

    // For now, just include the IP address
    // Future: include machine name, slot, port, etc.
    match ip {
        IpAddr::V4(v4) => {
            packet.push(4); // IPv4
            packet.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            packet.push(6); // IPv6
            packet.extend_from_slice(&v6.octets());
        }
    }

    packet
}

/// Parse a discovery packet
fn parse_packet(data: &[u8], from_ip: IpAddr) -> Result<DiscoveredDevice> {
    if data.len() < 6 || &data[0..5] != MAGIC {
        return Err(Error::Config("Invalid discovery packet magic".into()));
    }

    let _packet_type = match data[5] {
        1 => PacketType::Announce,
        2 => PacketType::Query,
        3 => PacketType::Response,
        _ => return Err(Error::Config("Invalid discovery packet type".into())),
    };

    // For now, just extract the IP from the sender
    // Future: parse machine name, slot, etc. from the packet
    Ok(DiscoveredDevice {
        name: "Unknown".to_string(),
        address: from_ip,
        port: DISCOVERY_PORT,
        slot: 0,
    })
}

/// Run the discovery service in the background
async fn run_discovery(
    local_host: IpAddr,
    tx: mpsc::Sender<DiscoveredDevice>,
) -> Result<()> {
    // For now, just do periodic scans
    // Future: also listen for incoming announcements
    loop {
        if let Ok(devices) = scan_local_network(local_host, Duration::from_secs(2)).await {
            for device in devices {
                if tx.send(device).await.is_err() {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
}

/// Auto-configure Mouse Borders by discovering devices on the local network
/// Returns the updated settings with discovered devices added
pub async fn auto_configure_from_discovery(
    settings: MouseBorders,
    local_name: &str,
    local_host: IpAddr,
) -> Result<MouseBorders> {
    use crate::layout::MachineName;
    use crate::settings_edit;

    let discovered = scan_local_network(local_host, Duration::from_secs(3)).await?;

    if discovered.is_empty() {
        return Ok(settings);
    }

    let mut new_settings = settings.clone();
    let local = MachineName::new(local_name)?;

    // Add this machine if not already present
    settings_edit::add_machine(&mut new_settings, &local, None)?;

    // Add discovered devices
    for device in discovered {
        let name = MachineName::new(&device.name)?;
        if name != local {
            // Find a free slot
            let used_slots: Vec<u8> = new_settings.machines.iter().map(|m| m.slot).collect();

            let free_slot = (0..4)
                .find(|s| !used_slots.contains(s))
                .ok_or_else(|| Error::Layout("No free slots for auto-discovered devices".into()))?;

            // Check if this device is already in the layout
            if !new_settings.machines.iter().any(|m| m.name == device.name) {
                new_settings.machines.push(aujar_config::Machine {
                    name: device.name.clone(),
                    slot: free_slot,
                    address: format!("{}:{}", device.address, device.port),
                });
            }
        }
    }

    // Enable if we found devices
    if new_settings.machines.len() > 1 {
        new_settings.enabled = true;
    }

    Ok(new_settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_broadcast_ip() {
        // 192.168.1.10/24 -> 192.168.1.255
        assert_eq!(
            broadcast_ip(Ipv4Addr::new(192, 168, 1, 10), 24),
            Ipv4Addr::new(192, 168, 1, 255)
        );

        // 10.0.0.1/8 -> 10.255.255.255
        assert_eq!(
            broadcast_ip(Ipv4Addr::new(10, 0, 0, 1), 8),
            Ipv4Addr::new(10, 255, 255, 255)
        );
    }

    #[test]
    fn test_build_and_parse_packet() {
        let ip = Ipv4Addr::new(192, 168, 1, 10);
        let packet = build_packet(PacketType::Announce, IpAddr::V4(ip));

        // Verify magic
        assert_eq!(&packet[0..5], &MAGIC);
        assert_eq!(packet[5], PacketType::Announce as u8);
        assert_eq!(packet[6], 4); // IPv4
        assert_eq!(&packet[7..11], &ip.octets());
    }
}
