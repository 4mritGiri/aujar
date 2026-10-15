//! A snapshot of the engine for `aujar mwb status`, the daemon and the UI.

use std::{collections::VecDeque, net::SocketAddr};

use aujar_config::EasyMouse;
use aujar_core::Size;

/// How many recent events (transfers, notices) a status keeps.
pub const LOG_CAPACITY: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerStatus {
    pub name: String,
    pub connected: bool,
    pub address: Option<String>,
    pub screen: Option<Size>,
    pub slot: Option<u8>,
    /// Why the last connection attempt failed.
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub running: bool,
    pub local: String,
    /// Machine currently holding the keyboard and mouse.
    pub focus: String,
    /// Machine controlling this one, if any.
    pub controlled_by: Option<String>,
    pub multi_machine_input: bool,
    pub easy_mouse: EasyMouse,
    pub listening: Option<SocketAddr>,
    /// `None` when switching at screen edges works; otherwise why it does not.
    pub edge_switching_unavailable: Option<String>,
    /// `None` when local input can be captured; otherwise why not (this machine can still be
    /// controlled).
    pub capture_unavailable: Option<String>,
    pub peers: Vec<PeerStatus>,
    pub log: VecDeque<String>,
}

impl Status {
    pub fn stopped() -> Self {
        Self {
            running: false,
            local: String::new(),
            focus: String::new(),
            controlled_by: None,
            multi_machine_input: false,
            easy_mouse: EasyMouse::Enabled,
            listening: None,
            edge_switching_unavailable: None,
            capture_unavailable: None,
            peers: Vec::new(),
            log: VecDeque::new(),
        }
    }

    pub fn connected_peers(&self) -> usize {
        self.peers.iter().filter(|peer| peer.connected).count()
    }

    pub fn push_log(&mut self, entry: impl Into<String>) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }

        self.log.push_back(entry.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_is_bounded() {
        let mut status = Status::stopped();

        for i in 0..LOG_CAPACITY + 5 {
            status.push_log(format!("event {i}"));
        }

        assert_eq!(status.log.len(), LOG_CAPACITY);
        assert_eq!(status.log.front().unwrap(), "event 5");
    }

    #[test]
    fn counts_connected_peers() {
        let mut status = Status::stopped();
        let peer = |connected| PeerStatus {
            name: "p".into(),
            connected,
            address: None,
            screen: None,
            slot: None,
            last_error: None,
        };

        status.peers = vec![peer(true), peer(false), peer(true)];

        assert_eq!(status.connected_peers(), 2);
    }
}
