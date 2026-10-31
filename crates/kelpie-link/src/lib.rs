//! kelpie-link: the end-to-end encrypted link between Kelpie (the daemon, the responder) and a phone (the initiator).
//!
//! Sans I/O: this crate turns bytes into bytes; the caller owns sockets and clocks (`now_ms`).
//!
//! - **Pairing** is `Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s`. The phone knows the desktop's static key and a single-use 32-byte
//!   secret from the QR code (`QrPayload`); the secret is mixed in at the end of the first message, so the desktop can tell a
//!   first message made with a live code from anything else *before* it answers or asks anybody. A wrong code changes nothing.
//! - **Reconnecting** is `Noise_IK_25519_ChaChaPoly_BLAKE2s`. The desktop learns the phone's static key from the first message
//!   and answers only a key it knows (the caller decides between [`read_first`] and [`Opened::accept`]); no application data
//!   travels before the second message.
//! - **Approval**: both sides derive a 6-digit code from the handshake hash ([`Session::pair_code`]); the phone shows it and the
//!   person types it on the desktop.
//! - **Transport**: one carrier message is one Noise transport message (at most 65 535 bytes). An application message of up to
//!   1 MiB is cut into chunks, each `[flags] + bytes` inside one Noise message; `flags` says whether more chunks follow. Each
//!   direction moves to a new key (Noise `Rekey`) every 2^16 messages by counting them, so both ends agree without a word. Noise
//!   nonces are implicit counters over an ordered stream: a dropped, repeated, reordered or altered message fails to decrypt, and
//!   every error means the connection is finished (the caller drops it and both ends handshake again). There is no reorder window
//!   and no resumption: every reconnect is a fresh handshake.
//!
//! Wire framing of the first message: `[version, mode, mac1 (16 bytes)] + noise message 1`. `mac1` is a keyed BLAKE2s over the
//! version, the mode and the Noise message, keyed from the desktop's public key (reconnect) or the pairing secret (pairing): it
//! is checked before any Diffie-Hellman ([`prefilter`]), so garbage costs a hash, and anything that fails it is dropped
//! unanswered. The version and the mode are also in the Noise prologue, so neither can be changed in flight. The version is
//! fixed: there is no negotiation, and nothing below `MIN_VERSION` is accepted or ever retried. The reply is the bare second
//! Noise message.
//!
//! The crate knows nothing of what carries these messages (a WebSocket on the LAN today): anything ordered and message-framed will do.

use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine as _;
use blake2::digest::consts::U16;
use blake2::{Blake2s256, Blake2sMac, Digest};
use hmac::{Mac, SimpleHmac};
use serde::{Deserialize, Serialize};
use snow::{HandshakeState, TransportState};

pub const PAIR_PATTERN: &str = "Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s";
pub const RECONNECT_PATTERN: &str = "Noise_IK_25519_ChaChaPoly_BLAKE2s";
/// The protocol's version, the first byte of a first message and part of the Noise prologue.
pub const VERSION: u8 = 1;
/// Nothing older is accepted: a downgrade is refused, and a client never retries at a lower version.
pub const MIN_VERSION: u8 = 1;
/// Second byte of the phone's first message.
pub const MODE_PAIR: u8 = 1;
pub const MODE_RECONNECT: u8 = 2;
/// What precedes the Noise message in a first message: version, mode, mac1.
pub const FIRST_HEADER: usize = 2 + MAC1_LEN;
const MAC1_LEN: usize = 16;
const MAC1_LABEL_DESKTOP: &[u8] = b"kelpie-link mac1 desktop";
const MAC1_LABEL_PSK: &[u8] = b"kelpie-link mac1 psk";
pub const KEY_LEN: usize = 32;
/// The largest Noise message (and so socket message) either side sends or accepts.
pub const MAX_NOISE_MESSAGE: usize = 65_535;
const TAG_LEN: usize = 16;
/// Application bytes in one chunk: a Noise message less its tag and the flags byte.
pub const MAX_CHUNK: usize = MAX_NOISE_MESSAGE - TAG_LEN - 1;
/// The largest application message, in chunks.
pub const MAX_APP_MESSAGE: usize = 1 << 20;
/// Each direction moves to a new key after this many messages.
pub const REKEY_EVERY_MESSAGES: u64 = 1 << 16;
/// A message whose chunks stop arriving for this long ends the connection.
pub const REASSEMBLY_TIMEOUT_MS: u64 = 10_000;
/// How long either side waits for the other half of a handshake (for callers).
pub const HANDSHAKE_TIMEOUT_MS: u64 = 5_000;

const FLAG_MORE: u8 = 1;
/// HKDF `info` of the six-digit code.
const SAS_INFO: &[u8] = b"kelpie-sas";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    /// Not a message of this protocol (wrong size, unknown mode, bad QR payload).
    Malformed,
    /// A protocol version this end does not speak (there is no negotiation).
    Version,
    /// The first message's `mac1` is wrong: garbage, or made with a code that is not live. Drop it unanswered.
    BadMac,
    /// It did not decrypt: a wrong pairing code, a key the phone does not hold, or a message altered, repeated or out of order.
    Decrypt,
    /// An application message over `MAX_APP_MESSAGE`.
    TooLarge,
    /// The rest of a message did not come in time.
    Reassembly,
    /// The Noise library refused (a bug or a misuse; the text has no secrets).
    Noise(String),
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LinkError::Malformed => f.write_str("not a kelpie-link message"),
            LinkError::Version => f.write_str("another version of kelpie-link"),
            LinkError::BadMac => f.write_str("the first message's mac1 is wrong"),
            LinkError::Decrypt => f.write_str("the message did not decrypt"),
            LinkError::TooLarge => f.write_str("the message is too large"),
            LinkError::Reassembly => f.write_str("a message was cut off"),
            LinkError::Noise(e) => write!(f, "noise: {e}"),
        }
    }
}

impl std::error::Error for LinkError {}

fn noise(e: snow::Error) -> LinkError {
    match e {
        snow::Error::Decrypt => LinkError::Decrypt,
        other => LinkError::Noise(other.to_string()),
    }
}

/// A static X25519 key pair. Its `Debug` never shows the private half.
#[derive(Clone, PartialEq, Eq)]
pub struct Keypair {
    pub private: [u8; KEY_LEN],
    pub public: [u8; KEY_LEN],
}

impl fmt::Debug for Keypair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Keypair {{ public: {}, private: [redacted] }}", B64.encode(self.public))
    }
}

impl Keypair {
    /// A new key pair from the OS random generator.
    pub fn generate() -> Result<Keypair, LinkError> {
        let kp = snow::Builder::new(params(RECONNECT_PATTERN)).generate_keypair().map_err(noise)?;
        Ok(Keypair { private: to_key(&kp.private)?, public: to_key(&kp.public)? })
    }
}

/// 32 bytes, or `Malformed`.
pub fn to_key(bytes: &[u8]) -> Result<[u8; KEY_LEN], LinkError> {
    bytes.try_into().map_err(|_| LinkError::Malformed)
}

/// 32 random bytes from the OS (a pairing secret).
pub fn random_key() -> Result<[u8; KEY_LEN], LinkError> {
    let mut key = [0u8; KEY_LEN];
    getrandom::fill(&mut key).map_err(|e| LinkError::Noise(e.to_string()))?;
    Ok(key)
}

/// Base64url without padding, as keys travel in QR codes and JSON.
pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn unb64(text: &str) -> Result<Vec<u8>, LinkError> {
    B64.decode(text.trim()).map_err(|_| LinkError::Malformed)
}

fn params(pattern: &str) -> snow::params::NoiseParams {
    pattern.parse().expect("a valid Noise pattern")
}

fn prologue(version: u8, mode: u8) -> Vec<u8> {
    let mut p = format!("kelpie-link/{version}").into_bytes();
    p.push(mode);
    p
}

fn mac1_key(label: &[u8], secret: &[u8; KEY_LEN]) -> [u8; 32] {
    Blake2s256::new().chain_update(label).chain_update(secret).finalize().into()
}

fn mac1_state(key: &[u8; 32], version: u8, mode: u8, msg: &[u8]) -> Blake2sMac<U16> {
    let mut m = <Blake2sMac<U16> as blake2::digest::KeyInit>::new_from_slice(key).expect("a 32-byte key fits");
    blake2::digest::Update::update(&mut m, &[version, mode]);
    blake2::digest::Update::update(&mut m, msg);
    m
}

/// The cheap look at a first message: what a caller learns before spending a Diffie-Hellman on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefiltered {
    pub mode: u8,
    /// Pairing: which of the live codes its `mac1` was made with.
    pub psk_index: Option<usize>,
}

/// Checks a first message's version and `mac1` (keyed from `desktop`, this end's public key, or from each live code), in
/// constant time per key and before any Diffie-Hellman. Anything but `Ok` is to be dropped unanswered.
pub fn prefilter(desktop: &[u8; KEY_LEN], first: &[u8], psks: &[[u8; KEY_LEN]]) -> Result<Prefiltered, LinkError> {
    if first.len() < FIRST_HEADER || first.len() > FIRST_HEADER + MAX_NOISE_MESSAGE {
        return Err(LinkError::Malformed);
    }
    let (version, mode) = (first[0], first[1]);
    if !(MIN_VERSION..=VERSION).contains(&version) {
        return Err(LinkError::Version);
    }
    let (tag, msg) = first[2..].split_at(MAC1_LEN);
    let checks = |key: &[u8; 32]| mac1_state(key, version, mode, msg).verify_slice(tag).is_ok();
    match mode {
        MODE_RECONNECT if checks(&mac1_key(MAC1_LABEL_DESKTOP, desktop)) => Ok(Prefiltered { mode, psk_index: None }),
        MODE_PAIR => psks.iter().position(|psk| checks(&mac1_key(MAC1_LABEL_PSK, psk))).map(|i| Prefiltered { mode, psk_index: Some(i) }).ok_or(LinkError::BadMac),
        MODE_RECONNECT => Err(LinkError::BadMac),
        _ => Err(LinkError::Malformed),
    }
}

// ---------- the QR code ----------

/// A way to reach the desktop, in a pairing record. A list of these, not one address: direct candidates and, later, relays.
/// `t` says what it is (`lan` today, `a` being `ip:port`); a client skips kinds it does not know.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct Endpoint {
    #[serde(rename = "t")]
    pub kind: String,
    #[serde(rename = "a")]
    pub addr: String,
    /// What the carrier needs besides the address (a relay's room, later).
    #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
    pub room: Option<String>,
}

impl Endpoint {
    /// A direct address (`ip:port`).
    pub fn lan(addr: &str) -> Endpoint {
        Endpoint { kind: "lan".into(), addr: addr.into(), room: None }
    }
}

/// What the desktop's pairing QR code carries (`kelpie://pair#p=<encoded>` for the app, `http(s)://<address>/#p=<encoded>` for a
/// browser): the desktop's static key, the single-use pairing secret, where to reach it, and what to call it.
#[derive(Clone, PartialEq, Eq)]
pub struct QrPayload {
    pub dpk: [u8; KEY_LEN],
    pub psk: [u8; KEY_LEN],
    /// Where the desktop can be reached.
    pub endpoints: Vec<Endpoint>,
    /// The computer's name, shown on the phone.
    pub name: String,
}

impl fmt::Debug for QrPayload {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "QrPayload {{ dpk: {}, psk: [redacted], endpoints: {:?}, name: {:?} }}", B64.encode(self.dpk), self.endpoints, self.name)
    }
}

#[derive(Serialize, Deserialize)]
struct QrWire {
    v: u8,
    dpk: String,
    psk: String,
    #[serde(default)]
    ep: Vec<Endpoint>,
    #[serde(default)]
    name: String,
}

/// The QR payload's format.
pub const QR_VERSION: u8 = 1;

impl QrPayload {
    /// Base64url of its JSON.
    pub fn encode(&self) -> String {
        let wire = QrWire { v: QR_VERSION, dpk: b64(&self.dpk), psk: b64(&self.psk), ep: self.endpoints.clone(), name: self.name.clone() };
        b64(&serde_json::to_vec(&wire).expect("serializable"))
    }

    pub fn decode(text: &str) -> Result<QrPayload, LinkError> {
        let bytes = unb64(text)?;
        if bytes.len() > 4096 {
            return Err(LinkError::Malformed);
        }
        let wire: QrWire = serde_json::from_slice(&bytes).map_err(|_| LinkError::Malformed)?;
        if wire.v != QR_VERSION || wire.ep.len() > 32 || wire.name.chars().count() > 64 {
            return Err(LinkError::Malformed);
        }
        Ok(QrPayload { dpk: to_key(&unb64(&wire.dpk)?)?, psk: to_key(&unb64(&wire.psk)?)?, endpoints: wire.ep, name: wire.name })
    }
}

// ---------- the phone: initiator ----------

/// A handshake the phone started; [`Initiator::finish`] takes the desktop's reply.
pub struct Initiator {
    hs: HandshakeState,
}

impl Initiator {
    /// The first message of a pairing. It carries nothing but the handshake: Noise IK's first message is replayable and
    /// only weakly forward secret, so anything the phone has to say goes after the second.
    pub fn pair(local: &Keypair, desktop: &[u8; KEY_LEN], psk: &[u8; KEY_LEN]) -> Result<(Initiator, Vec<u8>), LinkError> {
        Self::start(MODE_PAIR, local, desktop, Some(psk), None)
    }

    /// The first message of a reconnect by a paired phone.
    pub fn reconnect(local: &Keypair, desktop: &[u8; KEY_LEN]) -> Result<(Initiator, Vec<u8>), LinkError> {
        Self::start(MODE_RECONNECT, local, desktop, None, None)
    }

    fn start(mode: u8, local: &Keypair, desktop: &[u8; KEY_LEN], psk: Option<&[u8; KEY_LEN]>, ephemeral: Option<&[u8; KEY_LEN]>) -> Result<(Initiator, Vec<u8>), LinkError> {
        let prologue = prologue(VERSION, mode);
        let mut b = snow::Builder::new(params(if psk.is_some() { PAIR_PATTERN } else { RECONNECT_PATTERN }))
            .local_private_key(&local.private)
            .map_err(noise)?
            .remote_public_key(desktop)
            .map_err(noise)?
            .prologue(&prologue)
            .map_err(noise)?;
        if let Some(psk) = psk {
            b = b.psk(1, psk).map_err(noise)?;
        }
        if let Some(e) = ephemeral {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        let mut hs = b.build_initiator().map_err(noise)?;
        let mut msg = vec![0u8; MAX_NOISE_MESSAGE];
        let n = hs.write_message(&[], &mut msg).map_err(noise)?;
        let key = match psk {
            Some(psk) => mac1_key(MAC1_LABEL_PSK, psk),
            None => mac1_key(MAC1_LABEL_DESKTOP, desktop),
        };
        let tag = mac1_state(&key, VERSION, mode, &msg[..n]).finalize().into_bytes();
        let mut first = Vec::with_capacity(FIRST_HEADER + n);
        first.extend_from_slice(&[VERSION, mode]);
        first.extend_from_slice(&tag);
        first.extend_from_slice(&msg[..n]);
        Ok((Initiator { hs }, first))
    }

    /// The desktop's reply: the session. A reply that carries anything is not one of ours.
    pub fn finish(mut self, reply: &[u8]) -> Result<Session, LinkError> {
        if reply.len() > MAX_NOISE_MESSAGE {
            return Err(LinkError::Malformed);
        }
        let mut payload = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self.hs.read_message(reply, &mut payload).map_err(noise)?;
        if n != 0 {
            return Err(LinkError::Malformed);
        }
        Session::new(self.hs)
    }
}

// ---------- the desktop: responder ----------

/// A first message that decrypted: who sent it. Nothing has been answered yet: [`Opened::accept`] writes the reply; dropping it
/// answers nothing. A first message can be a replay, so nothing may be done on its account but answering it.
pub struct Opened {
    pub mode: u8,
    /// Pairing: which of the live codes it was made with.
    pub psk_index: Option<usize>,
    /// The phone's static key.
    pub remote: [u8; KEY_LEN],
    hs: HandshakeState,
}

impl fmt::Debug for Opened {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Opened {{ mode: {}, psk_index: {:?}, remote: {} }}", self.mode, self.psk_index, B64.encode(self.remote))
    }
}

/// Reads a phone's first message ([`prefilter`] first: at most one Diffie-Hellman follows, with the code its `mac1` names). A
/// wrong code never gets as far as decrypting (`BadMac`), and the caller has nothing to spend or show.
pub fn read_first(local: &Keypair, first: &[u8], psks: &[[u8; KEY_LEN]]) -> Result<Opened, LinkError> {
    read_first_with(local, first, psks, None)
}

fn read_first_with(local: &Keypair, first: &[u8], psks: &[[u8; KEY_LEN]], ephemeral: Option<&[u8; KEY_LEN]>) -> Result<Opened, LinkError> {
    let filtered = prefilter(&local.public, first, psks)?;
    let (version, mode) = (first[0], filtered.mode);
    let msg = &first[FIRST_HEADER..];
    let prologue = prologue(version, mode);
    let build = |psk: Option<&[u8; KEY_LEN]>| -> Result<HandshakeState, LinkError> {
        let mut b = snow::Builder::new(params(if psk.is_some() { PAIR_PATTERN } else { RECONNECT_PATTERN }))
            .local_private_key(&local.private)
            .map_err(noise)?
            .prologue(&prologue)
            .map_err(noise)?;
        if let Some(psk) = psk {
            b = b.psk(1, psk).map_err(noise)?;
        }
        if let Some(e) = ephemeral {
            b = b.fixed_ephemeral_key_for_testing_only(e);
        }
        b.build_responder().map_err(noise)
    };
    let mut payload = vec![0u8; MAX_NOISE_MESSAGE];
    // A first message carries nothing but the handshake.
    let opened = |hs: HandshakeState, n: usize, psk_index: Option<usize>| -> Result<Opened, LinkError> {
        let remote = to_key(hs.get_remote_static().ok_or(LinkError::Malformed)?)?;
        if n != 0 {
            return Err(LinkError::Malformed);
        }
        Ok(Opened { mode, psk_index, remote, hs })
    };
    let mut hs = build(filtered.psk_index.map(|i| &psks[i]))?;
    let n = hs.read_message(msg, &mut payload).map_err(noise)?;
    opened(hs, n, filtered.psk_index)
}

impl Opened {
    /// Answers: the reply to send (it carries nothing but the handshake) and the session.
    pub fn accept(mut self) -> Result<(Session, Vec<u8>), LinkError> {
        let mut msg = vec![0u8; MAX_NOISE_MESSAGE];
        let n = self.hs.write_message(&[], &mut msg).map_err(noise)?;
        msg.truncate(n);
        Ok((Session::new(self.hs)?, msg))
    }
}

// ---------- the session ----------

/// An established link: seals application messages into socket messages and opens them again. Any error ends it.
pub struct Session {
    transport: TransportState,
    hash: [u8; 32],
    sent_since_rekey: u64,
    received_since_rekey: u64,
    rekey_every: u64,
    partial: Vec<u8>,
    partial_since: Option<u64>,
    buf: Vec<u8>,
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Session { .. }")
    }
}

impl Session {
    fn new(hs: HandshakeState) -> Result<Session, LinkError> {
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&hs.get_handshake_hash()[..32]);
        Ok(Session {
            transport: hs.into_transport_mode().map_err(noise)?,
            hash,
            sent_since_rekey: 0,
            received_since_rekey: 0,
            rekey_every: REKEY_EVERY_MESSAGES,
            partial: Vec::new(),
            partial_since: None,
            buf: vec![0u8; MAX_NOISE_MESSAGE],
        })
    }

    /// The handshake hash: the same on both ends, unique to this handshake.
    pub fn handshake_hash(&self) -> &[u8; 32] {
        &self.hash
    }

    /// The six digits the phone shows and the person types on the desktop.
    pub fn pair_code(&self) -> String {
        pair_code(&self.hash)
    }

    /// Rekeys after `n` messages instead of 2^16 (both ends must agree).
    #[cfg(test)]
    pub fn set_rekey_every(&mut self, n: u64) {
        self.rekey_every = n.max(1);
    }

    /// One application message as the carrier messages to send, in order.
    pub fn seal(&mut self, msg: &[u8]) -> Result<Vec<Vec<u8>>, LinkError> {
        if msg.len() > MAX_APP_MESSAGE {
            return Err(LinkError::TooLarge);
        }
        let chunks: Vec<&[u8]> = if msg.is_empty() { vec![&[][..]] } else { msg.chunks(MAX_CHUNK).collect() };
        let last = chunks.len() - 1;
        let mut out = Vec::with_capacity(chunks.len());
        let mut plain = Vec::with_capacity(MAX_CHUNK + 1);
        for (i, chunk) in chunks.into_iter().enumerate() {
            plain.clear();
            plain.push(if i < last { FLAG_MORE } else { 0 });
            plain.extend_from_slice(chunk);
            let n = self.transport.write_message(&plain, &mut self.buf).map_err(noise)?;
            out.push(self.buf[..n].to_vec());
            self.sent_since_rekey += 1;
            if self.sent_since_rekey >= self.rekey_every {
                self.transport.rekey_outgoing();
                self.sent_since_rekey = 0;
            }
        }
        Ok(out)
    }

    /// One carrier message: a whole application message once its last chunk is in, `None` while more are to come.
    pub fn open(&mut self, frame: &[u8], now_ms: u64) -> Result<Option<Vec<u8>>, LinkError> {
        if frame.len() > MAX_NOISE_MESSAGE || frame.len() < TAG_LEN + 1 {
            return Err(LinkError::Malformed);
        }
        if self.partial_since.is_some_and(|t| now_ms.saturating_sub(t) > REASSEMBLY_TIMEOUT_MS) {
            return Err(LinkError::Reassembly);
        }
        let n = self.transport.read_message(frame, &mut self.buf).map_err(noise)?;
        self.received_since_rekey += 1;
        if self.received_since_rekey >= self.rekey_every {
            self.transport.rekey_incoming();
            self.received_since_rekey = 0;
        }
        let (&flags, chunk) = self.buf[..n].split_first().ok_or(LinkError::Malformed)?;
        if flags & !FLAG_MORE != 0 {
            return Err(LinkError::Malformed);
        }
        if self.partial.len() + chunk.len() > MAX_APP_MESSAGE {
            return Err(LinkError::TooLarge);
        }
        self.partial.extend_from_slice(chunk);
        if flags & FLAG_MORE != 0 {
            self.partial_since.get_or_insert(now_ms);
            return Ok(None);
        }
        self.partial_since = None;
        Ok(Some(std::mem::take(&mut self.partial)))
    }

    /// A message has been waiting for its other chunks longer than allowed (call on a timer; true ⇒ drop the connection).
    pub fn reassembly_expired(&self, now_ms: u64) -> bool {
        self.partial_since.is_some_and(|t| now_ms.saturating_sub(t) > REASSEMBLY_TIMEOUT_MS)
    }
}

/// The six-digit approval code (short authentication string) of a handshake hash: `HKDF-BLAKE2s(ikm = h, salt = none,
/// info = "kelpie-sas")`, its first four bytes big-endian, modulo 10^6. Both ends compute it from the handshake they made, so
/// a code typed on the desktop that matches the phone's binds the approval to that very handshake.
pub fn pair_code(hash: &[u8]) -> String {
    type H = SimpleHmac<Blake2s256>;
    // Extract (RFC 5869): no salt is a key of zeros, the same as an empty HMAC key.
    let prk = H::new_from_slice(&[0u8; 32]).expect("any key").chain_update(hash).finalize().into_bytes();
    // Expand, first block.
    let t1 = H::new_from_slice(&prk).expect("any key").chain_update(SAS_INFO).chain_update([1u8]).finalize().into_bytes();
    let n = u32::from_be_bytes([t1[0], t1[1], t1[2], t1[3]]) % 1_000_000;
    format!("{n:06}")
}

/// Deterministic handshakes for test vectors shared with the phone's and the test scripts' implementations.
#[cfg(test)]
pub mod testing {
    use super::*;

    pub fn initiator_with_ephemeral(mode: u8, local: &Keypair, desktop: &[u8; KEY_LEN], psk: Option<&[u8; KEY_LEN]>, ephemeral: &[u8; KEY_LEN]) -> Result<(Initiator, Vec<u8>), LinkError> {
        Initiator::start(mode, local, desktop, psk, Some(ephemeral))
    }

    pub fn read_first_with_ephemeral(local: &Keypair, first: &[u8], psks: &[[u8; KEY_LEN]], ephemeral: &[u8; KEY_LEN]) -> Result<Opened, LinkError> {
        read_first_with(local, first, psks, Some(ephemeral))
    }

    /// A key pair from fixed private bytes.
    pub fn keypair_from(private: [u8; KEY_LEN]) -> Keypair {
        // The public half: X25519 of the base point, through a handshake-free route (snow has no direct call).
        let b = snow::Builder::new(params(RECONNECT_PATTERN)).fixed_ephemeral_key_for_testing_only(&private);
        let mut hs = b.local_private_key(&private).unwrap().remote_public_key(&[9u8; 32]).unwrap().build_initiator().unwrap();
        let mut msg = vec![0u8; MAX_NOISE_MESSAGE];
        let _ = hs.write_message(&[], &mut msg).unwrap();
        // IK message 1 starts with the initiator's ephemeral public key in the clear: the fixed private's public half.
        Keypair { private, public: to_key(&msg[..KEY_LEN]).unwrap() }
    }
}

#[cfg(test)]
mod tests;
