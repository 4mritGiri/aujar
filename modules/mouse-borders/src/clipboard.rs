//! Splitting clipboard content into protocol packets and putting it back together.

use sha2::{Digest, Sha256};

use crate::{
    backend::ClipboardContent,
    error::{Error, Result},
    protocol::{CHUNK_BYTES, ClipboardKind, ClipboardPacket, MAX_CLIPBOARD_BYTES},
};

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

fn payload(content: &ClipboardContent) -> (ClipboardKind, &[u8]) {
    match content {
        ClipboardContent::Text(text) => (ClipboardKind::Text, text.as_bytes()),
        ClipboardContent::Png(bytes) => (ClipboardKind::Png, bytes),
    }
}

/// What kind of content this is and how big, for logs. Never the content itself.
pub fn describe(content: &ClipboardContent) -> (&'static str, u64) {
    match content {
        ClipboardContent::Text(text) => ("text", text.len() as u64),
        ClipboardContent::Png(bytes) => ("image", bytes.len() as u64),
    }
}

/// Hash that identifies a clipboard value, used to avoid echoing it back and forth.
pub fn fingerprint(content: &ClipboardContent) -> [u8; 32] {
    let (kind, bytes) = payload(content);
    let mut hasher = Sha256::new();

    hasher.update([kind as u8]);
    hasher.update(bytes);

    hasher.finalize().into()
}

/// `Begin`, the chunks, then `End` with the checksum.
pub fn packets(id: u64, content: &ClipboardContent) -> Result<Vec<ClipboardPacket>> {
    let (kind, bytes) = payload(content);

    if bytes.len() as u64 > MAX_CLIPBOARD_BYTES {
        return Err(Error::Other(format!(
            "clipboard content of {} bytes is over the {MAX_CLIPBOARD_BYTES} byte limit",
            bytes.len()
        )));
    }

    let mut packets = Vec::with_capacity(bytes.len() / CHUNK_BYTES + 3);

    packets.push(ClipboardPacket::Begin {
        id,
        kind,
        total: bytes.len() as u64,
    });
    packets.extend(
        bytes
            .chunks(CHUNK_BYTES)
            .map(|chunk| ClipboardPacket::Chunk {
                id,
                data: chunk.to_vec(),
            }),
    );
    packets.push(ClipboardPacket::End {
        id,
        sha256: Sha256::digest(bytes).into(),
    });

    Ok(packets)
}

struct Partial {
    id: u64,
    kind: ClipboardKind,
    total: u64,
    data: Vec<u8>,
}

/// Reassembles one peer's clipboard packets. Each peer needs its own assembler.
#[derive(Default)]
pub struct Assembler {
    partial: Option<Partial>,
}

impl Assembler {
    /// Feed one packet. Returns the content when `End` completes a valid value. Any error
    /// discards the partial data.
    pub fn push(&mut self, packet: ClipboardPacket) -> Result<Option<ClipboardContent>> {
        let result = self.step(packet);

        if result.is_err() {
            self.partial = None;
        }

        result
    }

    fn step(&mut self, packet: ClipboardPacket) -> Result<Option<ClipboardContent>> {
        match packet {
            ClipboardPacket::Begin { id, kind, total } => {
                if total > MAX_CLIPBOARD_BYTES {
                    return Err(Error::Protocol("clipboard content too large".into()));
                }

                self.partial = Some(Partial {
                    id,
                    kind,
                    total,
                    data: Vec::with_capacity(total.min(1 << 20) as usize),
                });

                Ok(None)
            }

            ClipboardPacket::Chunk { id, data } => {
                let partial = self.current(id)?;

                if partial.data.len() as u64 + data.len() as u64 > partial.total {
                    return Err(Error::Protocol(
                        "clipboard data exceeds its announced size".into(),
                    ));
                }

                partial.data.extend_from_slice(&data);

                Ok(None)
            }

            ClipboardPacket::End { id, sha256 } => {
                self.current(id)?;
                let Partial {
                    kind, total, data, ..
                } = self.partial.take().expect("checked above");

                if data.len() as u64 != total {
                    return Err(Error::Protocol("clipboard data is incomplete".into()));
                }

                if <[u8; 32]>::from(Sha256::digest(&data)) != sha256 {
                    return Err(Error::Protocol("clipboard checksum mismatch".into()));
                }

                match kind {
                    ClipboardKind::Text => String::from_utf8(data)
                        .map(|text| Some(ClipboardContent::Text(text)))
                        .map_err(|_| Error::Protocol("clipboard text is not UTF-8".into())),
                    ClipboardKind::Png if data.starts_with(PNG_MAGIC) => {
                        Ok(Some(ClipboardContent::Png(data)))
                    }
                    ClipboardKind::Png => {
                        Err(Error::Protocol("clipboard image is not a PNG".into()))
                    }
                }
            }
        }
    }

    fn current(&mut self, id: u64) -> Result<&mut Partial> {
        match self.partial.as_mut() {
            Some(partial) if partial.id == id => Ok(partial),
            _ => Err(Error::Protocol(
                "clipboard packet without a matching start".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> ClipboardContent {
        ClipboardContent::Text(value.to_owned())
    }

    fn png(extra: usize) -> ClipboardContent {
        let mut bytes = PNG_MAGIC.to_vec();
        bytes.extend((0..extra).map(|i| (i % 256) as u8));

        ClipboardContent::Png(bytes)
    }

    fn transfer(content: &ClipboardContent) -> Result<Option<ClipboardContent>> {
        let mut assembler = Assembler::default();
        let mut done = None;

        for packet in packets(1, content)? {
            done = assembler.push(packet)?;
        }

        Ok(done)
    }

    #[test]
    fn text_and_images_survive_chunking() {
        for content in [
            text(""),
            text("hello wörld ✓"),
            text(&"x".repeat(CHUNK_BYTES * 3 + 5)),
            png(10),
            png(CHUNK_BYTES * 2),
        ] {
            assert_eq!(transfer(&content).unwrap(), Some(content));
        }
    }

    #[test]
    fn descriptions_never_include_the_content() {
        assert_eq!(describe(&text("secret")), ("text", 6));
        assert_eq!(describe(&png(10)), ("image", PNG_MAGIC.len() as u64 + 10));
    }

    #[test]
    fn fingerprints_distinguish_kind_and_content() {
        assert_eq!(fingerprint(&text("a")), fingerprint(&text("a")));
        assert_ne!(fingerprint(&text("a")), fingerprint(&text("b")));
        assert_ne!(
            fingerprint(&text("\u{89}PNG")),
            fingerprint(&ClipboardContent::Png(b"\xc2\x89PNG".to_vec()))
        );
    }

    #[test]
    fn oversized_content_is_refused_when_sending() {
        let huge = ClipboardContent::Text("x".repeat(MAX_CLIPBOARD_BYTES as usize + 1));

        assert!(packets(1, &huge).is_err());
    }

    #[test]
    fn corrupted_or_misordered_packets_are_rejected_and_reset() {
        let mut assembler = Assembler::default();
        let mut all = packets(1, &text("secret")).unwrap();

        // Chunk before Begin.
        assert!(assembler.push(all[1].clone()).is_err());

        // Corrupt data: checksum mismatch.
        assembler.push(all[0].clone()).unwrap();
        if let ClipboardPacket::Chunk { data, .. } = &mut all[1] {
            data[0] ^= 1;
        }
        assembler.push(all[1].clone()).unwrap();
        assert!(assembler.push(all[2].clone()).is_err());

        // The assembler recovers for the next transfer.
        let mut done = None;
        for packet in packets(2, &text("fine")).unwrap() {
            done = assembler.push(packet).unwrap();
        }
        assert_eq!(done, Some(text("fine")));
    }

    #[test]
    fn more_data_than_announced_and_wrong_ids_are_rejected() {
        let mut assembler = Assembler::default();

        assembler
            .push(ClipboardPacket::Begin {
                id: 1,
                kind: ClipboardKind::Text,
                total: 2,
            })
            .unwrap();
        assert!(
            assembler
                .push(ClipboardPacket::Chunk {
                    id: 1,
                    data: vec![b'a'; 3]
                })
                .is_err()
        );

        assembler
            .push(ClipboardPacket::Begin {
                id: 5,
                kind: ClipboardKind::Text,
                total: 1,
            })
            .unwrap();
        assert!(
            assembler
                .push(ClipboardPacket::Chunk {
                    id: 6,
                    data: vec![b'a']
                })
                .is_err()
        );
    }

    #[test]
    fn invalid_text_and_fake_images_are_rejected() {
        let mut assembler = Assembler::default();
        let mut feed = |kind, data: Vec<u8>| {
            let sha256: [u8; 32] = Sha256::digest(&data).into();
            let mut result = Ok(None);

            for packet in [
                ClipboardPacket::Begin {
                    id: 1,
                    kind,
                    total: data.len() as u64,
                },
                ClipboardPacket::Chunk { id: 1, data },
                ClipboardPacket::End { id: 1, sha256 },
            ] {
                result = assembler.push(packet);
            }

            result
        };

        assert!(feed(ClipboardKind::Text, vec![0xFF, 0xFE]).is_err());
        assert!(feed(ClipboardKind::Png, b"GIF89a....".to_vec()).is_err());
        assert!(feed(ClipboardKind::Text, b"ok".to_vec()).is_ok());
    }

    #[test]
    fn announcing_too_much_is_rejected_immediately() {
        let mut assembler = Assembler::default();

        assert!(
            assembler
                .push(ClipboardPacket::Begin {
                    id: 1,
                    kind: ClipboardKind::Png,
                    total: MAX_CLIPBOARD_BYTES + 1,
                })
                .is_err()
        );
    }
}
