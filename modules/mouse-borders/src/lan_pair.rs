//! Pairing two machines over the network: no code to copy, send and paste.
//!
//! One machine opens a pairing window ([`host`]); the other finds it with a scan and
//! connects ([`join`]). The two run an unauthenticated Noise `NN` handshake, which gives an
//! encrypted channel but proves nothing about who is on the other end. So both screens then
//! show the same 8-digit code, derived from the handshake, and the people compare them:
//!
//! ```text
//!   desk                                    laptop
//!   "Pair a new device"                     "Find devices" -> Pair
//!        |<------ TCP, Noise NN handshake ------>|
//!   shows 4821 0937                         shows 4821 0937
//!   "Codes match"                           "Codes match"
//!        |------ pairing code (encrypted) ------>|
//! ```
//!
//! A machine in the middle ends up with a different handshake on each side, so the two codes
//! differ and the people see it. Finding a handshake with a chosen code means trying about
//! 10^8 key pairs inside the ten seconds a handshake may take, which is out of reach.
//!
//! The pairing code that travels (see `pairing`) holds the security key, so it is only sent
//! after *both* people have confirmed, and only over the encrypted channel.
//!
//! Both ends run on their own thread and are driven through a [`Handle`]: poll
//! [`Handle::state`], answer the code check with [`Handle::confirm`], stop with
//! [`Handle::cancel`].

use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};
use snow::{Builder, TransportState};

use crate::{
    discovery::{Presence, Responder},
    error::{Error, Result},
    layout::MachineName,
    net::{is_same_subnet, local_networks, retry_in_use},
};

const NOISE_PARAMS: &str = "Noise_NN_25519_ChaChaPoly_SHA256";

/// A handshake must finish within this, which is what makes grinding for a chosen code
/// pointless.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// How long the people get to compare the codes and click.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);

/// How long a pairing window stays open by default.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(120);

/// Largest frame either side accepts.
const MAX_FRAME: usize = 2048;

const YES: &[u8] = b"OK";
const NO: &[u8] = b"NO";
const DONE: &[u8] = b"DONE";

/// The TCP port pairing uses: the sharing port plus one, so it never clashes with the
/// engine's listener.
pub fn pair_port(sharing_port: u16) -> u16 {
    sharing_port
        .checked_add(1)
        .unwrap_or_else(|| sharing_port.saturating_sub(1))
}

/// Where a pairing stands, for the settings window to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairState {
    /// Host: listening for a device. Guest: connecting.
    Waiting,
    /// Both screens show `sas`; the people compare it and confirm.
    Verify { sas: String, peer: String },
    /// This side confirmed; the other has not yet.
    Confirmed,
    /// Guest: the pairing code arrived.
    Received(String),
    /// Host: the code was handed over.
    Sent,
    /// Ended without pairing; the text says why.
    Failed(String),
}

impl PairState {
    /// Nothing more will happen.
    pub fn is_over(&self) -> bool {
        matches!(self, Self::Received(_) | Self::Sent | Self::Failed(_))
    }
}

struct Shared {
    state: PairState,
    decision: Option<bool>,
    cancelled: bool,
}

/// The settings window's grip on a running pairing.
#[derive(Clone)]
pub struct Handle(Arc<Mutex<Shared>>);

impl Handle {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(Shared {
            state: PairState::Waiting,
            decision: None,
            cancelled: false,
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        // A panic on the pairing thread must not take the window down with it.
        self.0.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn state(&self) -> PairState {
        self.lock().state.clone()
    }

    /// Answer "do the codes match?".
    pub fn confirm(&self, matches: bool) {
        self.lock().decision = Some(matches);
    }

    /// Stop pairing. Safe to call at any time, any number of times.
    pub fn cancel(&self) {
        self.lock().cancelled = true;
    }

    fn set(&self, state: PairState) {
        self.lock().state = state;
    }

    fn fail(&self, why: impl Into<String>) {
        self.set(PairState::Failed(why.into()));
    }

    fn cancelled(&self) -> bool {
        self.lock().cancelled
    }

    fn decision(&self) -> Option<bool> {
        self.lock().decision
    }
}

// ---- Wire helpers -------------------------------------------------------------------

fn write_frame(stream: &mut impl Write, data: &[u8]) -> io::Result<()> {
    let length = u16::try_from(data.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too long"))?;

    stream.write_all(&length.to_be_bytes())?;
    stream.write_all(data)?;
    stream.flush()
}

fn read_frame(stream: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut length = [0u8; 2];

    stream.read_exact(&mut length)?;

    let length = usize::from(u16::from_be_bytes(length));

    if length > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too long"));
    }

    let mut data = vec![0u8; length];

    stream.read_exact(&mut data)?;

    Ok(data)
}

fn send(stream: &mut TcpStream, transport: &mut TransportState, plain: &[u8]) -> Result<()> {
    let mut buffer = vec![0u8; plain.len() + 16];
    let length = transport
        .write_message(plain, &mut buffer)
        .map_err(|error| Error::Handshake(format!("encrypting: {error}")))?;

    write_frame(stream, &buffer[..length]).map_err(io_error)
}

fn receive(stream: &mut TcpStream, transport: &mut TransportState) -> Result<Vec<u8>> {
    let frame = read_frame(stream).map_err(io_error)?;
    let mut plain = vec![0u8; frame.len()];
    let length = transport
        .read_message(&frame, &mut plain)
        .map_err(|_| Error::Handshake("the other device sent something unreadable".into()))?;

    plain.truncate(length);

    Ok(plain)
}

fn io_error(error: io::Error) -> Error {
    match error.kind() {
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => {
            Error::Handshake("the other device stopped answering".into())
        }
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset => {
            Error::Handshake("the other device closed the connection".into())
        }
        _ => Error::Handshake(format!("network error: {error}")),
    }
}

fn params() -> Result<snow::params::NoiseParams> {
    NOISE_PARAMS
        .parse()
        .map_err(|error| Error::Handshake(format!("noise parameters: {error}")))
}

/// Eight digits, `1234 5678`, from the handshake hash.
fn code_from_hash(hash: &[u8]) -> String {
    let digest = Sha256::digest(hash);
    let number = u32::from_be_bytes([digest[0], digest[1], digest[2], digest[3]]) % 100_000_000;

    format!("{:04} {:04}", number / 10_000, number % 10_000)
}

/// Wait for the local person's answer, or for cancel or the deadline.
fn wait_for_decision(handle: &Handle, deadline: Instant) -> Result<bool> {
    loop {
        if handle.cancelled() {
            return Err(Error::Handshake("pairing was cancelled".into()));
        }

        if let Some(decision) = handle.decision() {
            return Ok(decision);
        }

        if Instant::now() >= deadline {
            return Err(Error::Handshake("nobody confirmed the code in time".into()));
        }

        thread::sleep(Duration::from_millis(100));
    }
}

fn time_left(deadline: Instant) -> Duration {
    deadline
        .saturating_duration_since(Instant::now())
        .max(Duration::from_secs(1))
}

fn machine_name(bytes: &[u8]) -> Result<String> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| Error::Handshake("the other device sent an unusable name".into()))?;

    Ok(MachineName::new(text)
        .map_err(|_| Error::Handshake("the other device sent an unusable name".into()))?
        .to_string())
}

// ---- Host ---------------------------------------------------------------------------

pub struct HostConfig {
    /// This machine's name, shown on the other device.
    pub name: String,
    /// The sharing port; pairing listens on [`pair_port`] of it.
    pub sharing_port: u16,
    /// The pairing code to hand over once both people confirmed.
    pub code: String,
    /// How long the window stays open.
    pub window: Duration,
}

/// Open a pairing window: answer scans and wait for one device to connect. Errors here are
/// problems opening the sockets (port in use); everything later comes through the handle.
pub fn host(config: HostConfig) -> Result<Handle> {
    let port = pair_port(config.sharing_port);
    let listener = retry_in_use(|| TcpListener::bind(("0.0.0.0", port))).map_err(|error| {
        Error::Config(format!("cannot listen for pairing on TCP port {port}: {error}"))
    })?;

    listener
        .set_nonblocking(true)
        .map_err(|error| Error::Config(format!("cannot listen for pairing: {error}")))?;

    let responder = Responder::bind(Presence {
        name: config.name.clone(),
        port,
        pairing_open: true,
    })?;

    let handle = Handle::new();
    let thread_handle = handle.clone();

    thread::Builder::new()
        .name("aujar-pair-host".into())
        .spawn(move || host_loop(listener, responder, config, thread_handle))
        .map_err(|error| Error::Config(format!("cannot start pairing: {error}")))?;

    Ok(handle)
}

fn host_loop(
    listener: TcpListener,
    mut responder: Responder,
    config: HostConfig,
    handle: Handle,
) {
    let deadline = Instant::now() + config.window;

    loop {
        if handle.cancelled() {
            handle.fail("Pairing was cancelled.");
            return;
        }

        if Instant::now() >= deadline {
            handle.fail("Nobody paired in time. Start pairing again to try again.");
            return;
        }

        // Answers a scan if one is waiting; takes up to a tick, which paces this loop.
        responder.poll();

        match listener.accept() {
            Ok((stream, from)) => {
                if !is_same_subnet(from.ip(), &local_networks()) {
                    continue;
                }

                let mut verified = false;

                match serve(stream, &config, &handle, &mut verified) {
                    Ok(()) => return,
                    // A stray connection before the codes were shown does not end the window.
                    Err(error) if !verified => {
                        tracing::debug!("pairing attempt from {from} failed: {error}");
                    }
                    Err(error) => {
                        handle.fail(describe(&error));
                        return;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                handle.fail(format!("Pairing stopped: {error}"));
                return;
            }
        }
    }
}

fn describe(error: &Error) -> String {
    match error {
        Error::Handshake(message) | Error::Config(message) => {
            let mut text = message.clone();

            if let Some(first) = text.get_mut(0..1) {
                first.make_ascii_uppercase();
            }

            format!("{}.", text.trim_end_matches('.'))
        }
        other => format!("{other}."),
    }
}

fn serve(
    mut stream: TcpStream,
    config: &HostConfig,
    handle: &Handle,
    verified: &mut bool,
) -> Result<()> {
    // Accepted sockets may inherit the listener's non-blocking mode.
    stream.set_nonblocking(false).map_err(io_error)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(io_error)?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(io_error)?;

    let mut state = Builder::new(params()?)
        .build_responder()
        .map_err(|error| Error::Handshake(format!("setup: {error}")))?;
    let mut buffer = vec![0u8; MAX_FRAME];

    let first = read_frame(&mut stream).map_err(io_error)?;

    state
        .read_message(&first, &mut buffer)
        .map_err(|_| Error::Handshake("the other device is not Aujar".into()))?;

    let length = state
        .write_message(&[], &mut buffer)
        .map_err(|error| Error::Handshake(format!("handshake: {error}")))?;

    write_frame(&mut stream, &buffer[..length]).map_err(io_error)?;

    let code = code_from_hash(state.get_handshake_hash());
    let mut transport = state
        .into_transport_mode()
        .map_err(|error| Error::Handshake(format!("handshake: {error}")))?;

    // The guest names itself over the encrypted channel.
    let peer = machine_name(&receive(&mut stream, &mut transport)?)?;

    *verified = true;
    handle.set(PairState::Verify { sas: code, peer });

    // From here the people take their time, whatever is left of the window.
    let confirm_deadline = Instant::now() + CONFIRM_TIMEOUT;

    if !wait_for_decision(handle, confirm_deadline)? {
        let _ = send(&mut stream, &mut transport, NO);

        return Err(Error::Handshake("you said the codes do not match".into()));
    }

    handle.set(PairState::Confirmed);
    stream
        .set_read_timeout(Some(time_left(confirm_deadline)))
        .map_err(io_error)?;

    if receive(&mut stream, &mut transport)? != YES {
        return Err(Error::Handshake(
            "the other device did not confirm the code".into(),
        ));
    }

    send(&mut stream, &mut transport, config.code.as_bytes())?;

    // Give the guest a moment to say it has the code; its answer changes nothing here.
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = receive(&mut stream, &mut transport);

    handle.set(PairState::Sent);

    Ok(())
}

// ---- Guest --------------------------------------------------------------------------

/// Connect to a device that has a pairing window open (found with
/// [`crate::discovery::scan`]) and run the pairing. Returns at once; follow it through the
/// handle.
pub fn join(address: SocketAddr, local_name: String, peer_name: String) -> Handle {
    let handle = Handle::new();
    let thread_handle = handle.clone();

    let started = thread::Builder::new()
        .name("aujar-pair-join".into())
        .spawn(move || {
            if let Err(error) = guest(address, &local_name, &peer_name, &thread_handle) {
                thread_handle.fail(describe(&error));
            }
        });

    if let Err(error) = started {
        handle.fail(format!("Cannot start pairing: {error}."));
    }

    handle
}

fn guest(address: SocketAddr, local_name: &str, peer_name: &str, handle: &Handle) -> Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(5))
        .map_err(|error| Error::Handshake(format!("cannot reach {address}: {error}")))?;

    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(io_error)?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT)).map_err(io_error)?;

    let mut state = Builder::new(params()?)
        .build_initiator()
        .map_err(|error| Error::Handshake(format!("setup: {error}")))?;
    let mut buffer = vec![0u8; MAX_FRAME];

    let length = state
        .write_message(&[], &mut buffer)
        .map_err(|error| Error::Handshake(format!("handshake: {error}")))?;

    write_frame(&mut stream, &buffer[..length]).map_err(io_error)?;

    let reply = read_frame(&mut stream).map_err(io_error)?;

    state
        .read_message(&reply, &mut buffer)
        .map_err(|_| Error::Handshake("the other device is not Aujar".into()))?;

    let code = code_from_hash(state.get_handshake_hash());
    let mut transport = state
        .into_transport_mode()
        .map_err(|error| Error::Handshake(format!("handshake: {error}")))?;

    send(&mut stream, &mut transport, local_name.as_bytes())?;

    handle.set(PairState::Verify {
        sas: code,
        peer: peer_name.to_owned(),
    });

    let deadline = Instant::now() + CONFIRM_TIMEOUT;

    if !wait_for_decision(handle, deadline)? {
        let _ = send(&mut stream, &mut transport, NO);

        return Err(Error::Handshake("you said the codes do not match".into()));
    }

    send(&mut stream, &mut transport, YES)?;
    handle.set(PairState::Confirmed);
    stream
        .set_read_timeout(Some(time_left(deadline)))
        .map_err(io_error)?;

    let code = receive(&mut stream, &mut transport)?;

    if code == NO {
        return Err(Error::Handshake(
            "the other device said the codes do not match".into(),
        ));
    }

    let code = String::from_utf8(code)
        .map_err(|_| Error::Handshake("the other device sent an unusable code".into()))?;

    if !code.starts_with("aujar-pair") {
        return Err(Error::Handshake(
            "the other device sent something that is not a pairing code".into(),
        ));
    }

    let _ = send(&mut stream, &mut transport, DONE);
    handle.set(PairState::Received(code));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(handle: &Handle, mut done: impl FnMut(&PairState) -> bool) -> PairState {
        let deadline = Instant::now() + Duration::from_secs(15);

        loop {
            let state = handle.state();

            if done(&state) {
                return state;
            }

            assert!(Instant::now() < deadline, "stuck in {state:?}");
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn verify(handle: &Handle) -> (String, String) {
        match wait_for(handle, |s| matches!(s, PairState::Verify { .. })) {
            PairState::Verify { sas, peer } => (sas, peer),
            _ => unreachable!(),
        }
    }

    /// A host on an ephemeral port without discovery: the protocol alone, over loopback.
    fn loopback_host(code: &str) -> (SocketAddr, Handle) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = Handle::new();
        let thread_handle = handle.clone();
        let config = HostConfig {
            name: "desk".into(),
            sharing_port: 1,
            code: code.into(),
            window: Duration::from_secs(30),
        };

        thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut verified = false;

            match serve(stream, &config, &thread_handle, &mut verified) {
                Ok(()) => {}
                Err(error) => thread_handle.fail(describe(&error)),
            }
        });

        (address, handle)
    }

    #[test]
    fn both_screens_show_the_same_code_and_the_pairing_code_arrives() {
        let (address, host) = loopback_host("aujar-pair1;row;SECRET");
        let guest = join(address, "laptop".into(), "desk".into());

        let (host_code, host_peer) = verify(&host);
        let (guest_code, guest_peer) = verify(&guest);

        assert_eq!(host_code, guest_code);
        assert_eq!(host_code.len(), 9);
        assert_eq!(host_peer, "laptop");
        assert_eq!(guest_peer, "desk");

        host.confirm(true);
        guest.confirm(true);

        let received = wait_for(&guest, PairState::is_over);

        assert_eq!(received, PairState::Received("aujar-pair1;row;SECRET".into()));
        assert_eq!(wait_for(&host, PairState::is_over), PairState::Sent);
    }

    #[test]
    fn the_code_is_not_sent_before_both_people_confirm() {
        let (address, host) = loopback_host("aujar-pair1;row;SECRET");
        let guest = join(address, "laptop".into(), "desk".into());

        verify(&host);
        verify(&guest);
        host.confirm(true);

        thread::sleep(Duration::from_millis(400));

        assert!(!guest.state().is_over(), "{:?}", guest.state());

        guest.confirm(true);

        assert!(matches!(
            wait_for(&guest, PairState::is_over),
            PairState::Received(_)
        ));
    }

    #[test]
    fn saying_the_codes_differ_ends_it_on_both_sides_without_a_code() {
        let (address, host) = loopback_host("aujar-pair1;row;SECRET");
        let guest = join(address, "laptop".into(), "desk".into());

        verify(&host);
        verify(&guest);
        host.confirm(false);

        assert!(matches!(
            wait_for(&host, PairState::is_over),
            PairState::Failed(_)
        ));

        guest.confirm(true);

        match wait_for(&guest, PairState::is_over) {
            PairState::Failed(_) => {}
            other => panic!("the guest must not receive anything: {other:?}"),
        }
    }

    #[test]
    fn cancelling_stops_a_waiting_guest() {
        let (address, host) = loopback_host("x");
        let guest = join(address, "laptop".into(), "desk".into());

        verify(&guest);
        guest.cancel();

        assert!(matches!(
            wait_for(&guest, PairState::is_over),
            PairState::Failed(_)
        ));

        host.cancel();
    }

    #[test]
    fn nobody_listening_is_a_clear_failure_not_a_hang() {
        // A port with no listener: bind to get a free one, then close it.
        let address = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();

            listener.local_addr().unwrap()
        };
        let guest = join(address, "laptop".into(), "desk".into());

        match wait_for(&guest, PairState::is_over) {
            PairState::Failed(message) => assert!(message.contains("Cannot reach"), "{message}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn codes_are_eight_digits_in_two_groups() {
        let code = code_from_hash(b"anything");

        assert_eq!(code.len(), 9);
        assert_eq!(code.as_bytes()[4], b' ');
        assert!(code.chars().filter(|c| *c != ' ').all(|c| c.is_ascii_digit()));
        assert_ne!(code_from_hash(b"one"), code_from_hash(b"two"));
    }

    #[test]
    fn the_pairing_port_sits_next_to_the_sharing_port() {
        assert_eq!(pair_port(15101), 15102);
        assert_eq!(pair_port(u16::MAX), u16::MAX - 1);
    }
}
