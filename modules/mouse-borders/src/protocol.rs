//! Messages exchanged between machines, after the encrypted channel is up.
//!
//! This is Aujar's own protocol (postcard-encoded). It does not interoperate with the
//! Windows Mouse Without Borders wire format.
//!
//! Keys are USB HID usages and buttons a neutral enum, so every operating system speaks the
//! same input. Large payloads (clipboard, files) are split into chunks so input events never
//! wait long behind them.

use aujar_core::{Position, Size};
use serde::{Deserialize, Serialize};

use crate::{
    error::{Error, Result},
    input::Button,
    layout::MachineName,
};

/// Bumped on incompatible changes. Peers with different versions refuse each other.
pub const PROTOCOL_VERSION: u16 = 2;

/// Largest encoded message. Must fit one Noise transport frame (64 KiB minus the tag).
pub const MAX_MESSAGE_BYTES: usize = 60 * 1024;

/// Payload bytes per file or clipboard chunk.
pub const CHUNK_BYTES: usize = 32 * 1024;

/// Hard upper limit for one transferred file, matching Mouse Without Borders.
pub const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;

/// Largest clipboard payload shared.
pub const MAX_CLIPBOARD_BYTES: u64 = 16 * 1024 * 1024;

/// Longest text accepted for names and reasons carried in messages.
pub const MAX_TEXT_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u16,
    pub machine: MachineName,
    pub screen: Size,
    pub features: Features,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Features {
    pub clipboard: bool,
    pub files: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEvent {
    PointerAbs {
        x: i32,
        y: i32,
    },
    PointerRel {
        dx: i32,
        dy: i32,
    },
    Button {
        button: Button,
        pressed: bool,
    },
    /// 120 units per wheel notch (the kernel's high-resolution unit).
    Wheel {
        horizontal: i32,
        vertical: i32,
    },
    /// USB HID usage of the physical key.
    Key {
        key: u16,
        pressed: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardKind {
    Text,
    Png,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipboardPacket {
    Begin {
        id: u64,
        kind: ClipboardKind,
        total: u64,
    },
    Chunk {
        id: u64,
        data: Vec<u8>,
    },
    End {
        id: u64,
        sha256: [u8; 32],
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileMessage {
    Offer { id: u64, name: String, size: u64 },
    Accept { id: u64 },
    Reject { id: u64, reason: String },
    Chunk { id: u64, offset: u64, data: Vec<u8> },
    Done { id: u64, sha256: [u8; 32] },
    Abort { id: u64, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    Hello(Hello),
    Heartbeat,
    /// The sender's screen size changed.
    Screen(Size),
    /// Controller to target: you now have the keyboard and mouse; the pointer is at
    /// `position` (ignored when `relative`).
    Enter {
        position: Position,
        relative: bool,
    },
    /// Controller to target: control has moved away. Release everything held.
    Leave,
    Input(InputEvent),
    /// Target to controller: hand control to `target` (`None` means the sender itself).
    RequestControl {
        target: Option<MachineName>,
    },
    /// Multi-machine input mode switched on or off by the sender.
    Broadcast {
        active: bool,
    },
    Clipboard(ClipboardPacket),
    File(FileMessage),
    /// Reset idle timers: someone is working.
    KeepAwake,
    LockScreen,
    Bye {
        reason: String,
    },
}

impl Message {
    /// Whether the message is time-sensitive input traffic rather than bulk data.
    pub fn is_realtime(&self) -> bool {
        !matches!(self, Self::Clipboard(_) | Self::File(_))
    }

    /// Reject messages whose fields exceed protocol limits, before acting on them.
    pub fn validate(&self) -> Result<()> {
        let too_long = |text: &str| text.len() > MAX_TEXT_LEN;

        let ok = match self {
            Self::Bye { reason } => !too_long(reason),
            Self::File(FileMessage::Offer { name, size, .. }) => {
                !too_long(name) && *size <= MAX_FILE_BYTES
            }
            Self::File(FileMessage::Reject { reason, .. } | FileMessage::Abort { reason, .. }) => {
                !too_long(reason)
            }
            Self::File(FileMessage::Chunk { data, .. })
            | Self::Clipboard(ClipboardPacket::Chunk { data, .. }) => data.len() <= CHUNK_BYTES,
            Self::Clipboard(ClipboardPacket::Begin { total, .. }) => *total <= MAX_CLIPBOARD_BYTES,
            _ => true,
        };

        if ok {
            Ok(())
        } else {
            Err(Error::Protocol("message exceeds protocol limits".into()))
        }
    }
}

pub fn encode(message: &Message) -> Result<Vec<u8>> {
    let bytes = postcard::to_stdvec(message)
        .map_err(|error| Error::Protocol(format!("cannot encode message: {error}")))?;

    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Error::Protocol(format!(
            "message of {} bytes exceeds the {MAX_MESSAGE_BYTES} byte limit",
            bytes.len()
        )));
    }

    Ok(bytes)
}

pub fn decode(bytes: &[u8]) -> Result<Message> {
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err(Error::Protocol("message too large".into()));
    }

    let message: Message = postcard::from_bytes(bytes)
        .map_err(|error| Error::Protocol(format!("malformed message: {error}")))?;

    message.validate()?;

    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(message: Message) {
        let bytes = encode(&message).unwrap();

        assert_eq!(decode(&bytes).unwrap(), message);
    }

    #[test]
    fn every_kind_of_message_round_trips() {
        let machine = MachineName::new("desk").unwrap();

        round_trip(Message::Hello(Hello {
            version: PROTOCOL_VERSION,
            machine: machine.clone(),
            screen: Size {
                width: 1920,
                height: 1080,
            },
            features: Features {
                clipboard: true,
                files: false,
            },
        }));
        round_trip(Message::Heartbeat);
        round_trip(Message::Enter {
            position: Position { x: -3, y: 40 },
            relative: false,
        });
        round_trip(Message::Input(InputEvent::PointerRel { dx: -1, dy: 7 }));
        round_trip(Message::Input(InputEvent::Wheel {
            horizontal: 0,
            vertical: -120,
        }));
        round_trip(Message::RequestControl {
            target: Some(machine),
        });
        round_trip(Message::Clipboard(ClipboardPacket::Begin {
            id: 9,
            kind: ClipboardKind::Png,
            total: 1234,
        }));
        round_trip(Message::File(FileMessage::Offer {
            id: 1,
            name: "a.txt".into(),
            size: 5,
        }));
        round_trip(Message::Bye {
            reason: "bye".into(),
        });
    }

    #[test]
    fn input_events_are_small() {
        let bytes = encode(&Message::Input(InputEvent::PointerRel { dx: 5, dy: -5 })).unwrap();

        assert!(bytes.len() <= 8, "{} bytes", bytes.len());
    }

    #[test]
    fn full_chunks_fit_in_a_frame() {
        let message = Message::File(FileMessage::Chunk {
            id: u64::MAX,
            offset: u64::MAX,
            data: vec![0xAB; CHUNK_BYTES],
        });

        assert!(encode(&message).unwrap().len() <= MAX_MESSAGE_BYTES);
    }

    #[test]
    fn oversized_chunks_are_rejected_both_ways() {
        let big = Message::File(FileMessage::Chunk {
            id: 1,
            offset: 0,
            data: vec![0; CHUNK_BYTES + 1],
        });

        // Encoding works (it is below the frame limit) but the receiver refuses it.
        let bytes = encode(&big).unwrap();
        assert!(decode(&bytes).is_err());

        let huge = Message::File(FileMessage::Chunk {
            id: 1,
            offset: 0,
            data: vec![0; MAX_MESSAGE_BYTES],
        });
        assert!(encode(&huge).is_err());
    }

    #[test]
    fn limits_on_offers_and_clipboard_are_enforced() {
        let offer = |size| {
            Message::File(FileMessage::Offer {
                id: 1,
                name: "x".into(),
                size,
            })
        };

        assert!(offer(MAX_FILE_BYTES).validate().is_ok());
        assert!(offer(MAX_FILE_BYTES + 1).validate().is_err());
        assert!(
            Message::Clipboard(ClipboardPacket::Begin {
                id: 1,
                kind: ClipboardKind::Text,
                total: MAX_CLIPBOARD_BYTES + 1,
            })
            .validate()
            .is_err()
        );
        assert!(
            Message::Bye {
                reason: "r".repeat(MAX_TEXT_LEN + 1)
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn garbage_and_invalid_names_do_not_decode() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[0xFF; 32]).is_err());

        // A Hello whose machine name fails validation must not decode.
        let mut bytes = postcard::to_stdvec(&Message::Hello(Hello {
            version: 1,
            machine: MachineName::new("ok").unwrap(),
            screen: Size {
                width: 1,
                height: 1,
            },
            features: Features {
                clipboard: false,
                files: false,
            },
        }))
        .unwrap();
        // Replace the 2-byte name "ok" by two spaces.
        let at = bytes.windows(2).position(|w| w == b"ok").unwrap();
        bytes[at] = b' ';
        bytes[at + 1] = b' ';

        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn realtime_classification() {
        assert!(Message::Heartbeat.is_realtime());
        assert!(
            Message::Input(InputEvent::Key {
                key: 0x29,
                pressed: true
            })
            .is_realtime()
        );
        assert!(!Message::File(FileMessage::Accept { id: 1 }).is_realtime());
    }
}
