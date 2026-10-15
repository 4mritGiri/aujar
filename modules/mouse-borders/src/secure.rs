//! The security key and the encrypted, authenticated channel between machines.
//!
//! Machines pair by sharing a generated security key. Every connection runs the Noise
//! handshake `XXpsk3` with a pre-shared key derived from it: both sides prove they know the
//! key, fresh ephemeral keys give forward secrecy, and all traffic is ChaCha20-Poly1305
//! encrypted. Nonces are a strict counter, so replayed, dropped or reordered frames fail
//! authentication.
//!
//! The key is generated (160 bits), never chosen: a short typed password would let an
//! attacker who can observe a handshake guess it offline.

use std::{sync::Arc, time::Duration};

use sha2::{Digest, Sha256};
use snow::{Builder, StatelessTransportState};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf, split};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::{
    error::{Error, Result},
    protocol::{self, MAX_MESSAGE_BYTES, Message},
};

const NOISE_PARAMS: &str = "Noise_XXpsk3_25519_ChaChaPoly_SHA256";
const PSK_LABEL: &[u8] = b"aujar-mouse-borders/psk/v1";

/// The whole handshake (three messages) must finish within this time.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// Handshake messages are under 100 bytes; anything larger before authentication is hostile.
const MAX_HANDSHAKE_FRAME: usize = 512;

/// Poly1305 tag length.
const TAG_LEN: usize = 16;

const KEY_BYTES: usize = 20;
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Shared secret that pairs machines. Zeroed on drop; never printed by `Debug`.
#[derive(Clone, PartialEq, Eq, ZeroizeOnDrop)]
pub struct SecurityKey([u8; KEY_BYTES]);

impl std::fmt::Debug for SecurityKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecurityKey(<redacted>)")
    }
}

impl SecurityKey {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; KEY_BYTES];

        getrandom::fill(&mut bytes)
            .map_err(|error| Error::Key(format!("no secure random source: {error}")))?;

        Ok(Self(bytes))
    }

    /// Parse the grouped form (`ABCD-EFGH-...`). Case, dashes and spaces are ignored.
    pub fn parse(text: &str) -> Result<Self> {
        let mut bits: u32 = 0;
        let mut bit_count = 0u32;
        let mut bytes = Vec::with_capacity(KEY_BYTES);
        let mut symbols = 0usize;

        for c in text.chars().filter(|c| !matches!(c, '-' | ' ' | '\t')) {
            let value = BASE32
                .iter()
                .position(|&b| b == c.to_ascii_uppercase() as u8 && c.is_ascii())
                .ok_or_else(|| {
                    Error::Key(format!(
                        "`{}` is not valid in a security key (letters A-Z and digits 2-7)",
                        c.escape_default()
                    ))
                })? as u32;

            symbols += 1;
            bits = (bits << 5) | value;
            bit_count += 5;

            if bit_count >= 8 {
                bit_count -= 8;
                bytes.push((bits >> bit_count) as u8);
                bits &= (1 << bit_count) - 1;
            }
        }

        let expected = KEY_BYTES * 8 / 5;

        if symbols != expected {
            bytes.zeroize();

            return Err(Error::Key(format!(
                "a security key has {expected} characters, not {symbols}; \
                 copy the whole key from the other machine"
            )));
        }

        let mut key = [0u8; KEY_BYTES];
        key.copy_from_slice(&bytes);
        bytes.zeroize();

        Ok(Self(key))
    }

    /// Display form: eight groups of four, e.g. `K7QD-2XJP-...`.
    pub fn to_display(&self) -> String {
        let mut text = String::with_capacity(KEY_BYTES * 8 / 5 + 7);
        let mut bits: u32 = 0;
        let mut bit_count = 0u32;
        let mut symbols = 0usize;

        let mut push = |value: u32, text: &mut String| {
            if symbols > 0 && symbols % 4 == 0 {
                text.push('-');
            }

            text.push(char::from(BASE32[value as usize]));
            symbols += 1;
        };

        for &byte in &self.0 {
            bits = (bits << 8) | u32::from(byte);
            bit_count += 8;

            while bit_count >= 5 {
                bit_count -= 5;
                push((bits >> bit_count) & 0x1F, &mut text);
            }

            bits &= (1 << bit_count) - 1;
        }

        text
    }

    fn psk(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(PSK_LABEL);
        hasher.update(self.0);

        hasher.finalize().into()
    }
}

/// Run the handshake as the side that opened the connection.
pub async fn connect<S>(
    stream: S,
    key: &SecurityKey,
) -> Result<(SecureReader<ReadHalf<S>>, SecureWriter<WriteHalf<S>>)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(stream, key, true))
        .await
        .map_err(|_| Error::Handshake("timed out".into()))?
}

/// Run the handshake as the side that accepted the connection.
pub async fn accept<S>(
    stream: S,
    key: &SecurityKey,
) -> Result<(SecureReader<ReadHalf<S>>, SecureWriter<WriteHalf<S>>)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    tokio::time::timeout(HANDSHAKE_TIMEOUT, handshake(stream, key, false))
        .await
        .map_err(|_| Error::Handshake("timed out".into()))?
}

async fn handshake<S>(
    stream: S,
    key: &SecurityKey,
    initiator: bool,
) -> Result<(SecureReader<ReadHalf<S>>, SecureWriter<WriteHalf<S>>)>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let params = NOISE_PARAMS
        .parse()
        .map_err(|error| Error::Handshake(format!("noise parameters: {error}")))?;
    let builder = Builder::new(params);
    let static_key = builder
        .generate_keypair()
        .map_err(|error| Error::Handshake(format!("key generation: {error}")))?;
    let mut psk = key.psk();
    let state = Builder::new(
        NOISE_PARAMS
            .parse()
            .map_err(|error| Error::Handshake(format!("noise parameters: {error}")))?,
    )
    .local_private_key(&static_key.private)
    .and_then(|builder| builder.psk(3, &psk))
    .and_then(|builder| {
        if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
    });
    // The handshake state keeps its own copy; do not leave ours lying around.
    psk.zeroize();

    let mut state = state.map_err(|error| Error::Handshake(format!("setup: {error}")))?;

    let (mut reader, mut writer) = split(stream);
    let mut buffer = vec![0u8; MAX_HANDSHAKE_FRAME];
    let mut incoming = vec![0u8; MAX_HANDSHAKE_FRAME];

    // XX: initiator writes 1 and 3, responder writes 2.
    for step in 0..3 {
        let our_turn = (step % 2 == 0) == initiator;

        if our_turn {
            let length = state
                .write_message(&[], &mut buffer)
                .map_err(|error| Error::Handshake(format!("writing handshake message: {error}")))?;

            write_frame(&mut writer, &buffer[..length]).await?;
        } else {
            let length = read_frame(&mut reader, &mut incoming, MAX_HANDSHAKE_FRAME)
                .await
                .map_err(|error| Error::Handshake(format!("reading handshake: {error}")))?;

            state
                .read_message(&incoming[..length], &mut buffer)
                .map_err(|_| {
                    Error::Handshake("the other machine does not use the same security key".into())
                })?;
        }
    }

    let transport = Arc::new(
        state
            .into_stateless_transport_mode()
            .map_err(|error| Error::Handshake(format!("finishing handshake: {error}")))?,
    );

    Ok((
        SecureReader {
            io: reader,
            state: Arc::clone(&transport),
            nonce: 0,
            buffer: vec![0u8; MAX_MESSAGE_BYTES + TAG_LEN],
            plain: vec![0u8; MAX_MESSAGE_BYTES + TAG_LEN],
        },
        SecureWriter {
            io: writer,
            state: transport,
            nonce: 0,
            buffer: vec![0u8; MAX_MESSAGE_BYTES + TAG_LEN],
        },
    ))
}

async fn write_frame<W: AsyncWrite + Unpin>(io: &mut W, payload: &[u8]) -> Result<()> {
    let length =
        u16::try_from(payload.len()).map_err(|_| Error::Protocol("frame too large".into()))?;

    // One write so a frame is never interleaved or split across syscalls needlessly.
    let mut frame = Vec::with_capacity(payload.len() + 2);
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);

    io.write_all(&frame).await?;
    io.flush().await?;

    Ok(())
}

/// Read one frame into `into`, refusing frames longer than `limit`.
async fn read_frame<R: AsyncRead + Unpin>(
    io: &mut R,
    into: &mut [u8],
    limit: usize,
) -> Result<usize> {
    let mut header = [0u8; 2];

    io.read_exact(&mut header).await?;

    let length = usize::from(u16::from_be_bytes(header));

    if length > limit || length > into.len() {
        return Err(Error::Protocol(format!(
            "frame of {length} bytes exceeds the {limit} byte limit"
        )));
    }

    io.read_exact(&mut into[..length]).await?;

    Ok(length)
}

/// Receiving half of an encrypted channel.
pub struct SecureReader<R> {
    io: R,
    state: Arc<StatelessTransportState>,
    nonce: u64,
    buffer: Vec<u8>,
    plain: Vec<u8>,
}

impl<R: AsyncRead + Unpin> SecureReader<R> {
    /// Wait for the next message. Any authentication failure is fatal for the connection.
    pub async fn recv(&mut self) -> Result<Message> {
        let limit = MAX_MESSAGE_BYTES + TAG_LEN;
        let length = read_frame(&mut self.io, &mut self.buffer, limit).await?;

        if length < TAG_LEN {
            return Err(Error::Protocol("frame shorter than its tag".into()));
        }

        let plain_len = self
            .state
            .read_message(self.nonce, &self.buffer[..length], &mut self.plain)
            .map_err(|_| Error::Protocol("message failed authentication".into()))?;

        self.nonce = self
            .nonce
            .checked_add(1)
            .ok_or_else(|| Error::Protocol("nonce space exhausted".into()))?;

        protocol::decode(&self.plain[..plain_len])
    }
}

/// Sending half of an encrypted channel.
pub struct SecureWriter<W> {
    io: W,
    state: Arc<StatelessTransportState>,
    nonce: u64,
    buffer: Vec<u8>,
}

impl<W: AsyncWrite + Unpin> SecureWriter<W> {
    pub async fn send(&mut self, message: &Message) -> Result<()> {
        let plain = protocol::encode(message)?;
        let length = self
            .state
            .write_message(self.nonce, &plain, &mut self.buffer)
            .map_err(|error| Error::Protocol(format!("encryption failed: {error}")))?;

        self.nonce = self
            .nonce
            .checked_add(1)
            .ok_or_else(|| Error::Protocol("nonce space exhausted".into()))?;

        write_frame(&mut self.io, &self.buffer[..length]).await
    }

    pub async fn shutdown(&mut self) {
        let _ = self.io.shutdown().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::InputEvent;
    use tokio::io::duplex;

    fn key() -> SecurityKey {
        SecurityKey::generate().unwrap()
    }

    async fn pair(
        client_key: &SecurityKey,
        server_key: &SecurityKey,
    ) -> (
        Result<(
            SecureReader<ReadHalf<tokio::io::DuplexStream>>,
            SecureWriter<WriteHalf<tokio::io::DuplexStream>>,
        )>,
        Result<(
            SecureReader<ReadHalf<tokio::io::DuplexStream>>,
            SecureWriter<WriteHalf<tokio::io::DuplexStream>>,
        )>,
    ) {
        let (a, b) = duplex(64 * 1024);

        tokio::join!(connect(a, client_key), accept(b, server_key))
    }

    #[test]
    fn keys_round_trip_through_their_display_form() {
        for _ in 0..50 {
            let key = key();
            let text = key.to_display();

            assert_eq!(text.len(), 32 + 7);
            assert_eq!(text.matches('-').count(), 7);
            assert_eq!(SecurityKey::parse(&text).unwrap(), key);
            assert_eq!(SecurityKey::parse(&text.to_lowercase()).unwrap(), key);
            assert_eq!(SecurityKey::parse(&text.replace('-', " ")).unwrap(), key);
        }
    }

    #[test]
    fn keys_are_random_and_redacted() {
        let (a, b) = (key(), key());

        assert_ne!(a, b);
        assert!(!format!("{a:?}").contains(&a.to_display()));
    }

    #[test]
    fn malformed_keys_are_rejected_with_a_useful_message() {
        let good = key().to_display();

        assert!(SecurityKey::parse("").is_err());
        assert!(SecurityKey::parse("ABCD-EFGH").is_err());
        assert!(SecurityKey::parse(&format!("{good}A")).is_err());
        assert!(SecurityKey::parse(&good.replace(&good[..1], "1")).is_err());
        assert!(SecurityKey::parse(&"é".repeat(32)).is_err());
        assert!(
            SecurityKey::parse("ABCD")
                .unwrap_err()
                .to_string()
                .contains("32 characters")
        );
    }

    #[tokio::test]
    async fn matching_keys_exchange_messages_in_both_directions() {
        let key = key();
        let (client, server) = pair(&key, &key).await;
        let ((mut client_rx, mut client_tx), (mut server_rx, mut server_tx)) =
            (client.unwrap(), server.unwrap());

        let event = Message::Input(InputEvent::PointerRel { dx: 3, dy: -4 });

        client_tx.send(&event).await.unwrap();
        server_tx.send(&Message::Heartbeat).await.unwrap();

        assert_eq!(server_rx.recv().await.unwrap(), event);
        assert_eq!(client_rx.recv().await.unwrap(), Message::Heartbeat);

        // Many messages keep the nonces in step.
        for i in 0..200 {
            client_tx
                .send(&Message::Input(InputEvent::PointerRel { dx: i, dy: 0 }))
                .await
                .unwrap();
        }
        for i in 0..200 {
            assert_eq!(
                server_rx.recv().await.unwrap(),
                Message::Input(InputEvent::PointerRel { dx: i, dy: 0 })
            );
        }
    }

    #[tokio::test]
    async fn different_keys_never_complete_a_session() {
        let (client, server) = pair(&key(), &key()).await;

        // The responder notices on the third message.
        let server_error = server.err().expect("responder must reject a wrong key");
        assert!(server_error.to_string().contains("security key"));

        // The initiator finishes its own writes, but nothing it receives will decrypt.
        if let Ok((mut rx, _tx)) = client {
            assert!(rx.recv().await.is_err());
        }
    }

    #[tokio::test]
    async fn garbage_instead_of_a_handshake_is_rejected() {
        let (mut attacker, victim) = duplex(4096);
        let key = key();

        let accept = tokio::spawn(async move { accept(victim, &key).await });

        // An oversized "frame" from an unauthenticated peer.
        attacker.write_all(&60_000u16.to_be_bytes()).await.unwrap();
        attacker.write_all(&[0u8; 100]).await.unwrap();

        assert!(accept.await.unwrap().is_err());
    }

    #[tokio::test]
    async fn silent_peers_time_out() {
        tokio::time::pause();

        let (_silent, victim) = duplex(4096);
        let key = key();
        let accept = tokio::spawn(async move { accept(victim, &key).await });

        tokio::time::sleep(HANDSHAKE_TIMEOUT + Duration::from_secs(1)).await;

        let error = accept.await.unwrap().err().expect("must time out");
        assert!(error.to_string().contains("timed out"));
    }

    type Duplex = tokio::io::DuplexStream;

    /// Open a session whose client-to-server frames pass through `mutate`, which receives
    /// each raw frame (header included) and its index and returns the frames to deliver.
    /// In this direction frames 0 and 1 are the handshake (Noise messages 1 and 3); frame 2
    /// is the first application message.
    async fn tampered_session(
        mutate: impl Fn(usize, Vec<u8>) -> Vec<Vec<u8>> + Send + 'static,
    ) -> (
        SecureWriter<WriteHalf<Duplex>>,
        SecureReader<ReadHalf<Duplex>>,
    ) {
        let key = key();
        let (client_end, proxy_client) = duplex(64 * 1024);
        let (proxy_server, server_end) = duplex(64 * 1024);
        let (mut from_client, mut to_client) = split(proxy_client);
        let (mut from_server, mut to_server) = split(proxy_server);

        tokio::spawn(async move {
            for index in 0.. {
                let mut header = [0u8; 2];
                if from_client.read_exact(&mut header).await.is_err() {
                    return;
                }

                let mut frame = header.to_vec();
                frame.resize(2 + usize::from(u16::from_be_bytes(header)), 0);
                if from_client.read_exact(&mut frame[2..]).await.is_err() {
                    return;
                }

                for out in mutate(index, frame) {
                    if to_server.write_all(&out).await.is_err() {
                        return;
                    }
                }
            }
        });
        tokio::spawn(async move {
            let _ = tokio::io::copy(&mut from_server, &mut to_client).await;
        });

        let server_key = key.clone();
        let server = tokio::spawn(async move { accept(server_end, &server_key).await });
        let (_client_rx, client_tx) = connect(client_end, &key).await.unwrap();
        let (server_rx, _server_tx) = server.await.unwrap().unwrap();

        (client_tx, server_rx)
    }

    #[tokio::test]
    async fn a_modified_frame_is_detected() {
        let (mut tx, mut rx) = tampered_session(|index, mut frame| {
            if index == 2 {
                frame[2] ^= 1;
            }

            vec![frame]
        })
        .await;

        tx.send(&Message::Heartbeat).await.unwrap();

        let error = rx.recv().await.unwrap_err();
        assert!(error.to_string().contains("authentication"), "{error}");
    }

    #[tokio::test]
    async fn a_replayed_frame_is_detected() {
        let (mut tx, mut rx) = tampered_session(|index, frame| {
            if index == 2 {
                vec![frame.clone(), frame]
            } else {
                vec![frame]
            }
        })
        .await;

        tx.send(&Message::Heartbeat).await.unwrap();

        assert_eq!(rx.recv().await.unwrap(), Message::Heartbeat);
        assert!(rx.recv().await.is_err(), "replayed frame must not decrypt");
    }
}
