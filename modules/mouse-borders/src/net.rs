//! Network helpers: the same-subnet rule and resolving configured peer addresses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::error::{Error, Result};

/// One address configured on a local network interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalNet {
    pub ip: IpAddr,
    pub prefix: u8,
}

impl LocalNet {
    pub fn contains(&self, other: IpAddr) -> bool {
        match (self.ip, normalize(other)) {
            (IpAddr::V4(net), IpAddr::V4(other)) => {
                masked_v4(net, self.prefix) == masked_v4(other, self.prefix)
            }
            (IpAddr::V6(net), IpAddr::V6(other)) => {
                masked_v6(net, self.prefix) == masked_v6(other, self.prefix)
            }
            _ => false,
        }
    }
}

/// Networks of this machine's non-loopback interfaces.
pub fn local_networks() -> Vec<LocalNet> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback())
        .map(|interface| match interface.addr {
            if_addrs::IfAddr::V4(v4) => LocalNet {
                ip: IpAddr::V4(v4.ip),
                prefix: u32::from(v4.netmask).count_ones() as u8,
            },
            if_addrs::IfAddr::V6(v6) => LocalNet {
                ip: IpAddr::V6(v6.ip),
                prefix: u128::from(v6.netmask).count_ones() as u8,
            },
        })
        .collect()
}

/// Whether `peer` is on one of `networks`. Loopback always counts: it never leaves the
/// machine. IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are treated as IPv4.
pub fn is_same_subnet(peer: IpAddr, networks: &[LocalNet]) -> bool {
    let peer = normalize(peer);

    peer.is_loopback() || networks.iter().any(|network| network.contains(peer))
}

fn normalize(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        v4 => v4,
    }
}

fn masked_v4(ip: Ipv4Addr, prefix: u8) -> u32 {
    let mask = match prefix.min(32) {
        0 => 0,
        bits => u32::MAX << (32 - u32::from(bits)),
    };

    u32::from(ip) & mask
}

fn masked_v6(ip: Ipv6Addr, prefix: u8) -> u128 {
    let mask = match prefix.min(128) {
        0 => 0,
        bits => u128::MAX << (128 - u32::from(bits)),
    };

    u128::from(ip) & mask
}

/// Split `host`, `host:port`, `1.2.3.4`, `[::1]:15101` into a host and port. Bare IPv6
/// addresses (several colons, no brackets) are taken as a host with the default port.
pub fn split_host_port(address: &str, default_port: u16) -> Result<(String, u16)> {
    let address = address.trim();

    if address.is_empty() {
        return Err(Error::Config("empty address".into()));
    }

    if let Ok(socket) = address.parse::<SocketAddr>() {
        return Ok((socket.ip().to_string(), socket.port()));
    }

    if address.parse::<IpAddr>().is_ok() {
        return Ok((address.to_owned(), default_port));
    }

    match address.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            let port = port.parse::<u16>().map_err(|_| {
                Error::Config(format!("`{port}` is not a valid port in `{address}`"))
            })?;

            Ok((host.to_owned(), port))
        }
        _ => Ok((address.to_owned(), default_port)),
    }
}

/// Resolve a configured address (or machine name) to socket addresses.
pub async fn resolve(address: &str, default_port: u16) -> Result<Vec<SocketAddr>> {
    let (host, port) = split_host_port(address, default_port)?;

    let addresses: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), port))
        .await
        .map_err(|error| Error::Config(format!("cannot resolve `{host}`: {error}")))?
        .collect();

    if addresses.is_empty() {
        return Err(Error::Config(format!("`{host}` has no addresses")));
    }

    Ok(addresses)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ip: &str, prefix: u8) -> LocalNet {
        LocalNet {
            ip: ip.parse().unwrap(),
            prefix,
        }
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn ipv4_subnet_membership() {
        let nets = [net("192.168.1.10", 24)];

        assert!(is_same_subnet(ip("192.168.1.200"), &nets));
        assert!(!is_same_subnet(ip("192.168.2.1"), &nets));
        assert!(!is_same_subnet(ip("10.0.0.1"), &nets));
    }

    #[test]
    fn narrow_and_wide_prefixes() {
        assert!(net("10.1.2.3", 8).contains(ip("10.200.0.1")));
        assert!(!net("10.1.2.3", 32).contains(ip("10.1.2.4")));
        assert!(net("10.1.2.3", 32).contains(ip("10.1.2.3")));
        // A /0 matches everything of the same family.
        assert!(net("10.1.2.3", 0).contains(ip("8.8.8.8")));
        assert!(!net("10.1.2.3", 0).contains(ip("::1")));
    }

    #[test]
    fn ipv6_and_mapped_addresses() {
        let nets = [net("fd00::5", 64), net("192.168.1.10", 24)];

        assert!(is_same_subnet(ip("fd00::abcd"), &nets));
        assert!(!is_same_subnet(ip("fd01::1"), &nets));
        assert!(is_same_subnet(ip("::ffff:192.168.1.9"), &nets));
        assert!(!is_same_subnet(ip("::ffff:192.168.7.9"), &nets));
    }

    #[test]
    fn loopback_is_always_allowed_and_no_networks_blocks_the_rest() {
        assert!(is_same_subnet(ip("127.0.0.1"), &[]));
        assert!(is_same_subnet(ip("::1"), &[]));
        assert!(is_same_subnet(ip("::ffff:127.0.0.1"), &[]));
        assert!(!is_same_subnet(ip("192.168.1.1"), &[]));
    }

    #[test]
    fn address_parsing() {
        let split = |text| split_host_port(text, 15101).unwrap();

        assert_eq!(split("desk"), ("desk".into(), 15101));
        assert_eq!(split("desk:2000"), ("desk".into(), 2000));
        assert_eq!(split("192.168.1.5"), ("192.168.1.5".into(), 15101));
        assert_eq!(split("192.168.1.5:99"), ("192.168.1.5".into(), 99));
        assert_eq!(split("[::1]:7"), ("::1".into(), 7));
        assert_eq!(split("fd00::1"), ("fd00::1".into(), 15101));
        assert!(split_host_port("desk:notaport", 1).is_err());
        assert!(split_host_port("  ", 1).is_err());
    }

    #[tokio::test]
    async fn resolves_ip_literals_and_localhost() {
        let literal = resolve("127.0.0.1:4000", 1).await.unwrap();

        assert_eq!(literal, ["127.0.0.1:4000".parse::<SocketAddr>().unwrap()]);
        assert!(!resolve("localhost", 15101).await.unwrap().is_empty());
        assert!(resolve("no-such-host.invalid", 1).await.is_err());
    }
}
