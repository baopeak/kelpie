//! The relay (`relay/` in this repository): an untrusted forwarder on Cloudflare Workers through which a phone reaches this
//! computer when Tailscale is off on the phone or blocked. This end dials out to `/v1/desktop`; the relay gives each phone socket a
//! slot and forwards opaque kelpie-link frames between the two. It sees ciphertext, addresses, timing, sizes and the room id, and
//! nothing a session says. `docs/link-protocol.md` has the wire.
//!
//! - **Off** (the default): no socket and no task, nothing in a QR code, an `addrs` message or the page's CSP.
//! - **The room** is made once with the operator's registration key (`POST /v1/rooms`), which is kept in the credential store to make
//!   a new room later. Its key is a P-256 key in the credential store; the relay keeps the public half and checks a fresh signature
//!   on every connection, so nothing that crosses the wire lets anyone else in as this computer.
//! - **Phones**: one that paired on a secure origin gets a token inside kelpie-link (good for 30 days, a new one after 7) and the
//!   relay gets the tokens' hashes. The whole list goes first on every connection, before the relay says which phones are there: a
//!   phone removed while the relay was away is closed before it reaches anything. A pairing QR code adds a single-use token derived
//!   from its secret, for two minutes.
//! - **Slots**: each phone socket feeds `link::accept` like a socket on `/link`, at an address for the limits that goes with the token it
//!   came in with (`phone_ip`): the same handshake budget, the same `mac1` check before any Diffie-Hellman. A slot serves only a device paired on a secure
//!   origin, and only one that says `caps` with `ack` and `hb` (`link`).
//! - **Liveness**: a keepalive every 20 s, which the relay answers without waking its room; nothing heard for 45 s and the connection
//!   is made again, after a full-jitter backoff from 1 s to 60 s that starts over after 30 s up and when this machine's addresses change.
//!   A round trip that stays above 200 ms for a day is reported, and Settings offers a new room (where it lands is where the desktop is).

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use futures_util::{SinkExt, StreamExt};
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, Notify};
use tokio_tungstenite::tungstenite::{self, Message as WsMessage};

use super::link::{self, Carrier, Peer};
use super::store::{self, Device, RelaySettings, RelayToken};
use super::{listen, Generation, Mobile};
use crate::daemon::Daemon;
use crate::session::now_millis;

/// The relay this Kelpie is built to use: personal use first, so a room there needs the owner's registration key.
pub const DEFAULT_URL: &str = "wss://kelpie-link-relay.baopeak.workers.dev";
/// Credential-store keys: the room's signing key, and the registration key that makes rooms.
const K_ROOM: &str = "mobile-relay-room-key";
const K_OPERATOR: &str = "mobile-relay-operator-key";
pub const KEYS: [&str; 2] = [K_ROOM, K_OPERATOR];
const DAY_MS: u64 = 24 * 3600 * 1000;
const TOKEN_LIFE_MS: u64 = 30 * DAY_MS;
const TOKEN_RENEW_MS: u64 = 7 * DAY_MS;
const MAX_ALLOWED: usize = 64;
const PAIR_TOKENS: usize = 4;
const KEEPALIVE: Duration = Duration::from_secs(20);
const DEAD_AFTER: Duration = Duration::from_secs(45);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const BACKOFF_FIRST: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const STABLE_AFTER: Duration = Duration::from_secs(30);
/// Tokens expire, so the list is told again now and then even when nothing else changed.
const ALLOW_REFRESH: Duration = Duration::from_secs(3600);
pub const SLOW_RTT: Duration = Duration::from_millis(200);
pub const SLOW_FOR: Duration = Duration::from_secs(24 * 3600);
/// Frames from one phone waiting for its connection's task; a phone that outruns it is closed.
const SLOT_QUEUE: usize = 64;
const OUT_QUEUE: usize = 256;
/// What the relay takes in one frame, our slot number included.
const MAX_FRAME: usize = 64 * 1024 + 2;
/// How long phones have to hear of a new room before the old one is dropped.
const MOVE_GRACE: Duration = Duration::from_secs(2);
/// A device change reaches the relay this long after it is made, so that a removed phone hears it in its session before the relay closes it.
const REVOKE_GRACE: Duration = Duration::from_secs(1);
/// How long a gentle end waits for what the sessions still send.
const FLUSH: Duration = Duration::from_secs(1);

/// A slot's address, for the limits and for what the person reads: in 100::/64, the block set aside to go nowhere (RFC 6666), so
/// no phone on any network can have one.
pub fn slot_ip(slot: u16) -> IpAddr {
    IpAddr::V6(Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0x6b6c, slot))
}

pub fn is_slot_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.segments()[..4] == [0x0100, 0, 0, 0])
}

/// The address a phone on the relay goes by for the limits: the one of the credential it came in with (`k` in `open`, the hash of its token or
/// of its pairing token), also in 100::/64. A slot is new for every socket, so by slot nothing would ever add up; by credential what one phone
/// does adds up wherever its sockets land, and costs no other phone. A relay that names none gives the slot's address.
pub fn phone_ip(slot: u16, cred: Option<&str>) -> IpAddr {
    match cred.filter(|k| k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit())) {
        Some(k) => {
            let group = |i: usize| u16::from_str_radix(&k[i * 4..i * 4 + 4], 16).unwrap_or(0);
            IpAddr::V6(Ipv6Addr::new(0x0100, 0, 0, 0, group(0), group(1), group(2), group(3)))
        }
        None => slot_ip(slot),
    }
}

/// The single-use token a pairing QR code lets into the room: `HKDF-SHA256(ikm = the code's secret, no salt, info = "kelpie-relay-pair")`.
/// The phone derives the same from the QR code, and the relay sees only its hash.
pub fn pair_token(psk: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hkdf::<Sha256>::new(None, psk).expand(b"kelpie-relay-pair", &mut out).expect("32 bytes is a valid length");
    out
}

/// SHA-256, hex: how the relay knows a token.
pub fn token_hash(token: &[u8]) -> String {
    super::hex(&Sha256::digest(token))
}

/// What this computer signs to enter its room.
pub fn statement(room: &str, ts: u64) -> String {
    format!("kelpie-relay/1 desktop {room} {ts}")
}

fn signature(key: &SigningKey, room: &str, ts: u64) -> String {
    let sig: Signature = key.sign(statement(room, ts).as_bytes());
    kelpie_link::b64(&sig.to_bytes())
}

fn public_key(key: &SigningKey) -> String {
    kelpie_link::b64(key.verifying_key().to_encoded_point(false).as_bytes())
}

fn new_room_key() -> Result<SigningKey> {
    let bytes = kelpie_link::random_key().map_err(|e| anyhow!("{e}"))?;
    SigningKey::from_slice(&bytes).map_err(|_| anyhow!("could not make a room key"))
}

/// The relay's address as kept: `wss://<host>[:port]`, or `ws://` to this machine (a local `workerd`, in tests).
pub fn normalized_url(text: &str) -> Result<String> {
    let text = text.trim().trim_end_matches('/');
    let wrong = || anyhow!("the relay's address is wss://<name>, and nothing after it");
    let uri: axum::http::Uri = text.parse().map_err(|_| wrong())?;
    let host = uri.host().filter(|h| !h.is_empty()).ok_or_else(wrong)?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    // A name of letters, digits, dots and dashes, or an address: nothing that could mean more where the address is written into the page's CSP.
    let plain_name = !bare.is_empty() && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !plain_name && bare.parse::<IpAddr>().is_err() {
        return Err(wrong());
    }
    let local = bare == "localhost" || bare.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback());
    match uri.scheme_str() {
        Some("wss") => {}
        Some("ws") if local => {}
        _ => return Err(wrong()),
    }
    let bare_address = uri.path_and_query().is_none_or(|p| p.as_str().is_empty() || p.as_str() == "/");
    if !bare_address || text.contains('@') || text.len() > 200 {
        return Err(wrong());
    }
    Ok(text.to_string())
}

/// `https://` for `wss://`, `http://` for `ws://`.
fn http_url(url: &str) -> String {
    match url.strip_prefix("wss://") {
        Some(rest) => format!("https://{rest}"),
        None => format!("http://{}", url.strip_prefix("ws://").unwrap_or(url)),
    }
}

/// What the phone page may connect to besides itself while the relay is on: the relay, for its socket and its health.
fn connect_src(url: &str) -> String {
    format!("{} {url}", http_url(url))
}

/// Full jitter: anywhere up to 1 s, then 2, 4, ... up to 60 s.
fn backoff(attempt: u32) -> Duration {
    let cap = BACKOFF_FIRST.saturating_mul(1 << attempt.saturating_sub(1).min(6)).min(BACKOFF_MAX);
    let r = kelpie_link::random_key().map_or(0.5, |k| f64::from(u32::from_le_bytes([k[0], k[1], k[2], k[3]])) / f64::from(u32::MAX));
    cap.mul_f64(r)
}

// ---------- what the relay's connection and its slots say to each other ----------

/// What a phone's session sends the relay: a frame for its slot, or closing it with a code. Each names the generation the slot had
/// when the session began, so what an old session still sends never reaches a phone that holds the slot number now.
pub enum Out {
    Frame(u16, u64, Vec<u8>),
    Kick(u16, u64, u16, String),
}

/// What comes for a slot from the relay: a phone's frame, or the code the phone closed its socket with.
pub type Inbound = std::result::Result<Vec<u8>, u16>;

/// One phone's socket at the relay, as `link` sees it.
pub struct Slot {
    slot: u16,
    generation: u64,
    frames: mpsc::Receiver<Inbound>,
    out: mpsc::Sender<Out>,
}

impl Slot {
    /// The phone's next frame, or the code it closed with; None once it left or the relay's connection ended.
    pub async fn recv(&mut self) -> Option<Inbound> {
        self.frames.recv().await
    }

    pub async fn send(&self, frame: Vec<u8>) -> bool {
        self.out.send(Out::Frame(self.slot, self.generation, frame)).await.is_ok()
    }

    pub async fn close(&self, code: u16, reason: String) {
        let _ = self.out.send(Out::Kick(self.slot, self.generation, code, reason)).await;
    }
}

/// The phone sockets of one relay connection, by slot number, each with the generation `open` gave it.
#[derive(Default)]
struct Slots {
    next: u64,
    open: HashMap<u16, (u64, mpsc::Sender<Inbound>)>,
}

impl Slots {
    /// A slot opened (again): an older holder's frames stop, and that session ends.
    fn open(&mut self, slot: u16) -> (u64, mpsc::Receiver<Inbound>) {
        self.next += 1;
        let (tx, rx) = mpsc::channel(SLOT_QUEUE);
        self.open.insert(slot, (self.next, tx));
        (self.next, rx)
    }

    /// Whether what a session sends is from the slot's present holder.
    fn current(&self, slot: u16, generation: u64) -> bool {
        self.open.get(&slot).is_some_and(|(g, _)| *g == generation)
    }

    fn close(&mut self, slot: u16) {
        self.open.remove(&slot);
    }

    /// The phone closed its socket: its session hears the code, then nothing more.
    fn closed(&mut self, slot: u16, code: u16) {
        if let Some((_, tx)) = self.open.remove(&slot) {
            let _ = tx.try_send(Err(code));
        }
    }

    fn sender(&self, slot: u16) -> Option<&mpsc::Sender<Inbound>> {
        self.open.get(&slot).map(|(_, tx)| tx)
    }
}

// ---------- state ----------

/// The relay's side of phone access, in memory.
#[derive(Default)]
pub struct Runtime {
    task: Mutex<Option<Task>>,
    facts: Mutex<Facts>,
    /// What this end tells the relay itself (a pairing token, an origin), while the connection is up: a queue of its own, so that no
    /// amount of phone traffic drops it.
    control: Mutex<Option<mpsc::UnboundedSender<String>>>,
    /// Pairing tokens' hashes and until when they count, to tell a relay that connects meanwhile.
    pairs: Mutex<Vec<(String, Instant)>>,
    /// The time of the last signature: each one is later.
    last_ts: Mutex<u64>,
    /// Whether the credential store holds the registration key (None: not looked yet).
    operator: Mutex<Option<bool>>,
    room_key: Mutex<Option<SigningKey>>,
}

struct Task {
    stop: Arc<Generation>,
    nudge: Arc<Notify>,
    ending: Arc<Mutex<Ending>>,
    handle: tokio::task::JoinHandle<()>,
}

/// How a connection ends when it is stopped.
#[derive(Clone)]
enum Ending {
    /// At once (the relay turned off, or a word owed was said).
    Now,
    /// After what the sessions still send (a removed phone's sealed `revoked`), then the room is told the last word: on this connection
    /// when it is in that room, else on one of its own once the relay answers.
    Flush(Option<Arc<LastWord>>),
}

/// What a room is told when this computer stops using it: that nobody comes in any more (phone access off, the kill switch), or that it goes
/// (a reset, a new room). Kept with what enters that room, as it was when the word was decided: by then the settings may name another.
struct LastWord {
    url: String,
    room: String,
    ticket: String,
    key: SigningKey,
    drop: bool,
}

impl LastWord {
    fn message(&self) -> Value {
        if self.drop {
            json!({"t": "drop"})
        } else {
            json!({"t": "allow", "h": []})
        }
    }
}

/// What a connection in `room` says as it ends gently: the last word when it is for this room, and nothing for another (a connection that came
/// back in a new room meanwhile must not empty it).
fn said_on_ending(last: Option<&LastWord>, room: &str) -> Option<Value> {
    last.filter(|w| w.room == room).map(LastWord::message)
}

/// How long a word owed to a room is tried for when the relay does not answer. A room that never hears it expires unused anyway.
const LAST_WORD_FOR: Duration = Duration::from_secs(24 * 3600);

#[derive(Default, Clone)]
struct Facts {
    connected: bool,
    error: Option<String>,
    rtt_ms: Option<f64>,
    slow_since: Option<Instant>,
}

impl Runtime {
    fn next_ts(&self) -> u64 {
        let mut last = self.last_ts.lock();
        *last = now_millis().max(*last + 1);
        *last
    }

    fn up(&self, control: mpsc::UnboundedSender<String>) {
        *self.control.lock() = Some(control);
        let mut f = self.facts.lock();
        f.connected = true;
        f.error = None;
    }

    fn down(&self, error: Option<String>) {
        *self.control.lock() = None;
        let mut f = self.facts.lock();
        f.connected = false;
        f.error = error;
    }

    /// A keepalive's round trip, smoothed. Slow for a day and the person is offered a new room.
    fn rtt(&self, took: Duration) {
        let ms = took.as_secs_f64() * 1000.0;
        let mut f = self.facts.lock();
        let smooth = f.rtt_ms.map_or(ms, |old| old * 0.8 + ms * 0.2);
        f.rtt_ms = Some(smooth);
        if smooth > SLOW_RTT.as_secs_f64() * 1000.0 {
            f.slow_since.get_or_insert_with(Instant::now);
        } else {
            f.slow_since = None;
        }
    }

    fn control(&self, msg: Value) {
        if let Some(control) = self.control.lock().as_ref() {
            let _ = control.send(msg.to_string());
        }
    }

    /// The tokens changed: the list goes to the relay again.
    pub(super) fn nudge(&self) {
        if let Some(t) = self.task.lock().as_ref() {
            t.nudge.notify_one();
        }
    }

    fn live_pairs(&self) -> Vec<(String, u64)> {
        let now = Instant::now();
        let mut pairs = self.pairs.lock();
        pairs.retain(|(_, until)| *until > now);
        pairs.iter().map(|(h, until)| (h.clone(), until.duration_since(now).as_secs().max(1))).collect()
    }
}

impl Mobile {
    /// The relay's settings while it is on and they hold, and phone access is on.
    pub(super) fn relay_live(&self) -> Option<RelaySettings> {
        if self.broken.lock().is_some() {
            return None;
        }
        let key = self.mac_key(false).ok().flatten()?;
        let c = self.config.lock();
        if !c.enabled {
            return None;
        }
        c.relay.clone().filter(|r| r.on && r.room.is_some() && r.ticket.is_some() && store::relay_holds(r, &key))
    }

    /// How phones are told to reach the relay (`addrs`, the QR code).
    pub(super) fn relay_endpoint(&self) -> Option<kelpie_link::Endpoint> {
        self.relay_live().and_then(|r| Some(kelpie_link::Endpoint::relay(&r.url, &r.ticket?)))
    }

    /// What the page's CSP lets it connect to besides itself, while the relay is on.
    pub(super) fn relay_connect_src(&self) -> Option<String> {
        self.relay_live().map(|r| connect_src(&r.url))
    }

    /// The hashes of the tokens that let phones in: a token of a device that is still paired, on a secure origin, whose record and token both verify, and that has not expired.
    fn relay_allowed(&self) -> Vec<String> {
        let Some(key) = self.mac_key(false).ok().flatten() else { return Vec::new() };
        let now = now_millis();
        let c = self.config.lock();
        let Some(r) = c.relay.as_ref() else { return Vec::new() };
        let mut tokens: Vec<&RelayToken> = r.tokens.iter().filter(|t| t.expires > now && c.devices.iter().any(|d| d.verified && d.secure_origin && store::token_holds(t, d, &key))).collect();
        tokens.sort_by_key(|t| std::cmp::Reverse(t.issued));
        tokens.into_iter().take(MAX_ALLOWED).map(|t| t.hash.clone()).collect()
    }

    /// A token for this phone, when it may use the relay and does not hold a good one: the message that gives it, sent inside the session.
    /// `first` is what the phone said first: `caps` with `relay` from a client that can use one, and `relay_token`, the hash of the token it holds.
    pub(super) fn relay_grant(&self, device: &Device, first: &Value) -> Option<String> {
        let wants = first["caps"].as_array().is_some_and(|c| c.iter().any(|x| x == "relay"));
        if !wants || !device.secure_origin || !device.verified {
            return None;
        }
        let r = self.relay_live()?;
        let key = self.mac_key(false).ok().flatten()?;
        let now = now_millis();
        let held = first["relay_token"].as_str().unwrap_or_default();
        let fresh = |t: &RelayToken| t.expires > now && now.saturating_sub(t.issued) < TOKEN_RENEW_MS && super::same(&t.hash, held) && store::token_holds(t, device, &key);
        if r.tokens.iter().any(fresh) {
            return None;
        }
        let token = kelpie_link::random_key().ok()?;
        let mut t = RelayToken { device: device.id.clone(), hash: token_hash(&token), issued: now, expires: now + TOKEN_LIFE_MS, mac: String::new() };
        store::seal_token(&mut t, device, &key);
        {
            let mut c = self.config.lock();
            let store::Store { relay, devices, .. } = &mut *c;
            let relay = relay.as_mut()?;
            let paired: HashSet<&str> = devices.iter().map(|d| d.id.as_str()).collect();
            relay.tokens.retain(|x| x.expires > now && paired.contains(x.device.as_str()));
            // The one the phone says it holds stays, whatever else it was given: it may have missed a token given on a connection that
            // dropped, and must not be shut out of the relay for that. Without one it holds, the newest stays.
            let holds = |x: &RelayToken| x.device == device.id && super::same(&x.hash, held) && store::token_holds(x, device, &key);
            let kept = relay.tokens.iter().filter(|x| holds(x)).map(|x| x.issued).max().or_else(|| relay.tokens.iter().filter(|x| x.device == device.id).map(|x| x.issued).max());
            relay.tokens.retain(|x| x.device != device.id || Some(x.issued) == kept);
            relay.tokens.push(t.clone());
        }
        self.keep();
        self.relay.nudge();
        Some(json!({"type": "relay", "url": r.url, "ticket": r.ticket, "token": kelpie_link::b64(&token), "expires": t.expires}).to_string())
    }

    /// A pairing QR code was shown: its token may enter the room, once, for as long as the code lives.
    pub(super) fn relay_pair(&self, psk: &[u8; 32]) {
        if self.relay_live().is_none() {
            return;
        }
        let hash = token_hash(&pair_token(psk));
        {
            let mut pairs = self.relay.pairs.lock();
            let now = Instant::now();
            pairs.retain(|(_, until)| *until > now);
            pairs.push((hash.clone(), now + super::CODE_TTL));
            let extra = pairs.len().saturating_sub(PAIR_TOKENS);
            pairs.drain(..extra);
        }
        self.relay.control(json!({"t": "pair", "h": hash, "ttl": super::CODE_TTL.as_secs()}));
    }

    /// A pairing code was spent: its token is not told to the relay again, on this connection or the next.
    pub(super) fn relay_pair_spent(&self, psk: &[u8; 32]) {
        let hash = token_hash(&pair_token(psk));
        self.relay.pairs.lock().retain(|(h, _)| *h != hash);
    }

    /// Tailscale publishes this computer at a new origin: the room lets phones in from there.
    pub(super) fn relay_note_origin(&self) {
        let Some(ts) = self.tailscale_origin() else { return };
        let origin = format!("https://{ts}");
        let Some(key) = self.mac_key(false).ok().flatten() else { return };
        let changed = {
            let mut c = self.config.lock();
            match c.relay.as_mut() {
                Some(r) if store::relay_holds(r, &key) && r.origin.as_deref() != Some(origin.as_str()) => {
                    r.origin = Some(origin.clone());
                    store::seal_relay(r, &key);
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.keep();
            self.relay.control(json!({"t": "origin", "o": origin}));
        }
    }

    fn relay_has_operator_key(&self) -> bool {
        let mut known = self.relay.operator.lock();
        if known.is_none() {
            *known = self.secrets.read(K_OPERATOR).ok().map(|k| k.is_some());
        }
        known.unwrap_or(false)
    }

    fn room_key(&self) -> Result<Option<SigningKey>> {
        let mut cached = self.relay.room_key.lock();
        if cached.is_none() {
            *cached = match self.secrets.read(K_ROOM).context("reading the relay room key")? {
                Some(text) => Some(super::unhex32(&text).and_then(|b| SigningKey::from_slice(&b).ok()).ok_or_else(|| anyhow!("the relay room key in the credential store is damaged: make a new room"))?),
                None => None,
            };
        }
        Ok(cached.clone())
    }

    fn keep_room_key(&self, key: &SigningKey) -> Result<()> {
        self.secrets.set(K_ROOM, &super::hex(&key.to_bytes())).context("keeping the relay room key")?;
        *self.relay.room_key.lock() = Some(key.clone());
        Ok(())
    }

    /// The relay's connection ends, and nothing starts it until it is asked again (`follow`).
    pub(super) fn relay_stop(&self) {
        if let Some(t) = self.relay.task.lock().take() {
            t.stop.cancel();
        }
        self.relay.down(None);
    }

    /// Ends the relay's connection gently (phone access off, the kill switch, a reset): what the sessions still send goes first, then the
    /// room this computer is in lets nobody in, or goes (`drop`), even when the relay is out of reach now. Returns the task, to wait for it.
    pub(super) fn relay_wind_down(&self, drop: bool) -> Option<tokio::task::JoinHandle<()>> {
        if self.relay.task.lock().is_none() {
            return None;
        }
        self.relay_wind_down_with(self.relay_last_word(drop))
    }

    /// As `relay_wind_down`, with the word decided before (a new room: the old one goes).
    fn relay_wind_down_with(&self, last: Option<LastWord>) -> Option<tokio::task::JoinHandle<()>> {
        let t = self.relay.task.lock().take()?;
        *t.ending.lock() = Ending::Flush(last.map(Arc::new));
        t.stop.cancel();
        self.relay.down(None);
        Some(t.handle)
    }

    /// The last word for the room the settings name now: `drop` to remove it, else to let nobody in. None with no room, or no key to enter it.
    fn relay_last_word(&self, drop: bool) -> Option<LastWord> {
        let key = self.mac_key(false).ok().flatten()?;
        let r = self.config.lock().relay.clone().filter(|r| store::relay_holds(r, &key))?;
        Some(LastWord { url: r.url, room: r.room?, ticket: r.ticket?, key: self.room_key().ok().flatten()?, drop })
    }

    /// The room this computer has at the relay, as kept.
    pub(super) fn relay_room(&self) -> Option<String> {
        self.config.lock().relay.as_ref().and_then(|r| r.room.clone())
    }

    /// Everything of the relay is forgotten (a reset of phone access).
    pub(super) fn relay_forget(&self) {
        self.relay_stop();
        self.relay.pairs.lock().clear();
        *self.relay.operator.lock() = None;
        *self.relay.room_key.lock() = None;
    }

    /// What Settings shows.
    pub(super) fn relay_status(&self) -> Value {
        let live = self.relay_live();
        let kept = self.config.lock().relay.clone();
        let facts = self.relay.facts.lock().clone();
        let phones: HashSet<String> = {
            let allowed: HashSet<String> = self.relay_allowed().into_iter().collect();
            kept.iter().flat_map(|r| r.tokens.iter()).filter(|t| allowed.contains(&t.hash)).map(|t| t.device.clone()).collect()
        };
        json!({
            "on": live.is_some(),
            "url": kept.as_ref().map(|r| r.url.clone()).filter(|u| !u.is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string()),
            "default_url": DEFAULT_URL,
            "room": kept.as_ref().is_some_and(|r| r.room.is_some()),
            "origin": kept.as_ref().and_then(|r| r.origin.clone()),
            "operator_key": self.relay_has_operator_key(),
            "connected": live.is_some() && facts.connected,
            "error": facts.error.filter(|_| live.is_some()),
            "rtt_ms": facts.rtt_ms.map(|ms| ms.round() as u64),
            "slow": facts.slow_since.is_some_and(|s| s.elapsed() >= SLOW_FOR),
            "phones": phones.len(),
        })
    }
}

/// Tells phones, now and next time, whether and where the relay is.
fn readvertise(m: &Mobile) {
    let relay = m.relay_endpoint();
    m.addrs.send_modify(|a| {
        if a.relay != relay {
            a.seq += 1;
            a.relay = relay;
        }
    });
}

/// Starts the relay's connection when it should run (the relay on, phone access running) and stops it when not.
pub fn follow(daemon: &Arc<Daemon>) {
    let m = &daemon.mobile;
    let should = m.relay_live().is_some() && m.running.lock().is_some();
    let mut task = m.relay.task.lock();
    if should && task.is_none() {
        let stop = Arc::new(Generation::new());
        let nudge = Arc::new(Notify::new());
        let ending = Arc::new(Mutex::new(Ending::Now));
        let handle = tokio::spawn(run(Arc::downgrade(daemon), stop.clone(), nudge.clone(), ending.clone()));
        *task = Some(Task { stop, nudge, ending, handle });
    } else if !should {
        if let Some(t) = task.take() {
            t.stop.cancel();
        }
        drop(task);
        m.relay.down(None);
    }
}

fn restart(daemon: &Arc<Daemon>) {
    daemon.mobile.relay_stop();
    follow(daemon);
}

// ---------- turning it on and off ----------

/// What turning the relay on will use, checked before the person is asked.
pub struct TurnOn {
    url: String,
    operator_key: Option<String>,
    /// There is no room at this relay yet.
    register: bool,
}

/// `mobile.set_relay {on: true, url?, operator_key?}`: everything that needs no prompt. A room needs the registration key, given now or kept from before.
pub fn check_turn_on(m: &Mobile, params: &Value) -> Result<TurnOn> {
    ensure!(m.is_on(), "turn phone access on first");
    let url = normalized_url(params.get("url").and_then(Value::as_str).filter(|u| !u.trim().is_empty()).unwrap_or(DEFAULT_URL))?;
    let operator_key = params.get("operator_key").and_then(Value::as_str).map(str::trim).filter(|k| !k.is_empty()).map(str::to_string);
    if let Some(k) = &operator_key {
        ensure!(k.len() <= 256 && !k.chars().any(|c| c.is_whitespace() || c.is_control()), "that is not a registration key");
    }
    let register = m.config.lock().relay.as_ref().is_none_or(|r| r.url != url || r.room.is_none() || r.ticket.is_none());
    if register && operator_key.is_none() && !m.relay_has_operator_key() {
        bail!("paste the relay's registration key: it is needed once, to make this computer's room");
    }
    Ok(TurnOn { url, operator_key, register })
}

/// After the person verified: the room is made if there is none, and the relay is on. Runs on the prompt's thread, which may block, inside the runtime.
pub fn turn_on(daemon: &Arc<Daemon>, asked: TurnOn) -> Result<Value> {
    let m = &daemon.mobile;
    let key = m.mac_key(true)?.ok_or_else(|| anyhow!("the credential store did not keep the device key"))?;
    let mut settings = m.config.lock().relay.clone().filter(|r| store::relay_holds(r, &key)).unwrap_or_default();
    if asked.register {
        let operator = match asked.operator_key.clone() {
            Some(k) => k,
            None => m.secrets.read(K_OPERATOR)?.ok_or_else(|| anyhow!("paste the relay's registration key"))?,
        };
        let room_key = new_room_key()?;
        let (room, ticket) = register(&asked.url, &operator, &public_key(&room_key))?;
        m.keep_room_key(&room_key)?;
        settings.room = Some(room);
        settings.ticket = Some(ticket);
    }
    // Kept only once the relay took it: a key typed when no room was made was never checked, and must not replace a good one.
    if let Some(k) = asked.operator_key.as_ref().filter(|_| asked.register) {
        m.secrets.set(K_OPERATOR, k).context("keeping the registration key")?;
        *m.relay.operator.lock() = Some(true);
    }
    settings.on = true;
    settings.url = asked.url.clone();
    if let Some(ts) = m.tailscale_origin() {
        settings.origin = Some(format!("https://{ts}"));
    }
    store::seal_relay(&mut settings, &key);
    m.config.lock().relay = Some(settings);
    m.keep();
    m.audit("-", "relay_on", &asked.url);
    readvertise(m);
    restart(daemon);
    Ok(m.status(true))
}

/// `mobile.set_relay {on: false}`: no prompt, it only takes a way in away. The room and the tokens are kept for turning it on again.
pub fn turn_off(daemon: &Arc<Daemon>) -> Result<Value> {
    let m = &daemon.mobile;
    let key = m.mac_key(false)?;
    let changed = {
        let mut c = m.config.lock();
        match c.relay.as_mut() {
            Some(r) if r.on => {
                r.on = false;
                // With no key to seal it, what is left does not verify: off all the same.
                if let Some(k) = &key {
                    store::seal_relay(r, k);
                }
                true
            }
            _ => false,
        }
    };
    m.relay_stop();
    if changed {
        m.keep();
        m.audit("-", "relay_off", "turned off");
    }
    readvertise(m);
    Ok(m.status(true))
}

/// `mobile.relay_recreate`: a new room, where this computer's first request places it, for a room that stays slow. Phones hear the new room before the
/// old one goes; the tokens carry over.
pub async fn recreate(daemon: Arc<Daemon>) -> Result<Value> {
    let m = &daemon.mobile;
    let live = m.relay_live().ok_or_else(|| anyhow!("turn the relay on first"))?;
    let asker = daemon.clone();
    let url = live.url.clone();
    let (room, ticket, room_key) = tokio::task::spawn_blocking(move || -> Result<(String, String, SigningKey)> {
        let operator = asker.mobile.secrets.read(K_OPERATOR)?.ok_or_else(|| anyhow!("paste the relay's registration key again: this computer no longer has it"))?;
        let key = new_room_key()?;
        let (room, ticket) = register(&url, &operator, &public_key(&key))?;
        Ok((room, ticket, key))
    })
    .await
    .map_err(|_| anyhow!("that did not finish"))??;
    let old_key = m.room_key()?;
    // Decided now, while the settings still name the old room: it is removed once phones had time to hear of the new one.
    let old_room = m.relay_last_word(true);
    m.keep_room_key(&room_key)?;
    let committed = (|| -> Result<()> {
        let key = m.mac_key(false)?.ok_or_else(|| anyhow!("the device key cannot be read now"))?;
        let mut c = m.config.lock();
        let r = c.relay.as_mut().filter(|r| r.on).ok_or_else(|| anyhow!("the relay was turned off meanwhile"))?;
        r.room = Some(room);
        r.ticket = Some(ticket);
        store::seal_relay(r, &key);
        Ok(())
    })();
    if let Err(e) = committed {
        // The settings still name the old room: so does the key again.
        if let Some(k) = &old_key {
            m.keep_room_key(k)?;
        }
        return Err(e);
    }
    m.keep();
    m.audit("-", "relay_room", "a new room");
    m.relay.facts.lock().slow_since = None;
    readvertise(m);
    tokio::time::sleep(MOVE_GRACE).await;
    // The old room is removed on the old connection while it is in that room (else on one of its own), and that connection is gone before the
    // next one opens the new room.
    if let Some(old) = m.relay_wind_down_with(old_room) {
        let _ = tokio::time::timeout(Duration::from_secs(5), old).await;
    }
    follow(&daemon);
    Ok(m.status(true))
}

/// Makes a room: the room id and its ticket.
fn register(url: &str, operator: &str, pk: &str) -> Result<(String, String)> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(false).max_redirects(0).build().into();
    let bearer = format!("Bearer {operator}");
    let body = json!({"pk": pk}).to_string();
    let mut resp = agent.post(&format!("{}/v1/rooms", http_url(url))).header("authorization", bearer.as_str()).content_type("application/json").send(body.as_bytes()).map_err(|e| anyhow!("the relay could not be reached: {e}"))?;
    match resp.status().as_u16() {
        201 => {}
        401 => bail!("the relay did not take that registration key"),
        429 => bail!("the relay stopped taking registration keys from this network for today: too many wrong ones"),
        503 => bail!("the relay is not set up yet"),
        s => bail!("the relay answered HTTP {s}"),
    }
    let text = resp.body_mut().read_to_string().context("the relay's answer")?;
    let v: Value = serde_json::from_str(&text).context("the relay's answer")?;
    let room = v["room"].as_str().filter(|r| r.len() == 32 && r.bytes().all(|b| b.is_ascii_hexdigit())).ok_or_else(|| anyhow!("the relay's answer has no room"))?;
    let ticket = v["ticket"].as_str().filter(|t| t.len() <= 100 && t.strip_prefix(room).is_some_and(|rest| rest.starts_with('.'))).ok_or_else(|| anyhow!("the relay's answer has no ticket"))?;
    Ok((room.to_string(), ticket.to_string()))
}

// ---------- the connection ----------

trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}
type Socket = tokio_tungstenite::WebSocketStream<Box<dyn Io>>;

/// Resolves when the connection is to end at once; never for a gentle end, which the connection carries out itself.
async fn cut_short(stop: &Generation, ending: &Mutex<Ending>) {
    stop.cancelled().await;
    if !matches!(*ending.lock(), Ending::Now) {
        std::future::pending::<()>().await;
    }
}

async fn run(daemon: Weak<Daemon>, stop: Arc<Generation>, nudge: Arc<Notify>, ending: Arc<Mutex<Ending>>) {
    connect_until_stopped(&daemon, &stop, &nudge, &ending).await;
    // Stopped gently with the word still owed: the connection was down, or in another room.
    let owed = match &*ending.lock() {
        Ending::Flush(Some(word)) => word.clone(),
        _ => return,
    };
    say_last_word(&daemon, &owed).await;
}

async fn connect_until_stopped(daemon: &Weak<Daemon>, stop: &Generation, nudge: &Notify, ending: &Mutex<Ending>) {
    let mut attempt: u32 = 0;
    loop {
        let Some(d) = daemon.upgrade() else { return };
        let began = Instant::now();
        let ended = tokio::select! {
            biased;
            _ = cut_short(stop, ending) => return,
            ended = connection(&d, stop, nudge, ending) => ended,
        };
        if stop.is_cancelled() {
            return;
        }
        let why = ended.err().map(|e| format!("{e:#}"));
        if let Some(why) = &why {
            tracing::info!("relay: {why}");
        }
        d.mobile.relay.down(why);
        d.broadcast("mobile.updated", json!({}));
        drop(d);
        attempt = if began.elapsed() >= STABLE_AFTER { 1 } else { attempt.saturating_add(1) };
        let until = tokio::time::Instant::now() + backoff(attempt);
        let addresses = listen::addresses();
        loop {
            tokio::select! {
                biased;
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep_until(until) => break,
                // A new network: what failed on the old one says nothing about it.
                _ = listen::changed() => {
                    if listen::addresses() != addresses {
                        listen::settle().await;
                        attempt = 0;
                        break;
                    }
                }
            }
        }
    }
}

/// Tells a room its last word on a connection of its own, as soon as the relay answers: tried for a day at most, and no more once this computer is
/// back in that room (its own connection then tells the room what holds) or the room is gone.
async fn say_last_word(daemon: &Weak<Daemon>, word: &LastWord) {
    let until = Instant::now() + LAST_WORD_FOR;
    let mut attempt: u32 = 0;
    loop {
        let ts = {
            let Some(d) = daemon.upgrade() else { return };
            let m = &d.mobile;
            let running = m.relay.task.lock().is_some();
            if running && m.relay_room().as_deref() == Some(word.room.as_str()) {
                return;
            }
            m.relay.next_ts()
        };
        let url = format!("{}/v1/desktop?t={}&ts={ts}&sig={}", word.url, word.ticket, signature(&word.key, &word.room, ts));
        let said = async {
            let mut ws = tokio::time::timeout(CONNECT_TIMEOUT, dial(&url)).await.map_err(|_| anyhow!("the relay did not answer in time"))??;
            // The room reads a socket's messages in order, the close after the word.
            send_text(&mut ws, word.message()).await?;
            let _ = ws.close(None).await;
            Ok::<(), anyhow::Error>(())
        };
        match said.await {
            Ok(()) => return,
            // No such room, or not this computer's to enter any more: nothing left to tell.
            Err(e) if e.downcast_ref::<Refused>().is_some_and(|r| r.status == 403 || r.status == 404) => return,
            Err(_) if Instant::now() >= until => return,
            Err(_) => {}
        }
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = tokio::time::sleep(backoff(attempt)) => {}
            // A new network may reach the relay at once.
            _ = listen::changed() => listen::settle().await,
        }
    }
}

/// The relay turned the desktop away: the HTTP status it gave, and whether the router said too many refusals came from this network.
#[derive(Debug)]
struct Refused {
    status: u16,
    attempts: bool,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.status {
            403 => write!(f, "the relay refused this computer's signature"),
            404 => write!(f, "the relay no longer has this computer's room: make a new room in Settings → Mobile"),
            429 if self.attempts => write!(f, "the relay turns this network away for a few minutes: too many refused attempts came from it"),
            429 => write!(f, "the room reached its monthly limit at the relay"),
            s => write!(f, "the relay answered HTTP {s}"),
        }
    }
}

impl std::error::Error for Refused {}

/// Dials the relay: TCP with no Nagle delay, TLS from the system's usual roots (`webpki-roots`, ring), then the WebSocket.
async fn dial(url: &str) -> Result<Socket> {
    let uri: axum::http::Uri = url.parse().context("the relay's address")?;
    let host = uri.host().ok_or_else(|| anyhow!("the relay's address has no name"))?.trim_start_matches('[').trim_end_matches(']').to_string();
    let tls = uri.scheme_str() == Some("wss");
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let tcp = tokio::net::TcpStream::connect((host.as_str(), port)).await.context("connecting to the relay")?;
    tcp.set_nodelay(true).context("nodelay")?;
    super::keep_alive(&tcp);
    let io: Box<dyn Io> = if tls {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|_| anyhow!("the relay's name"))?;
        Box::new(tokio_rustls::TlsConnector::from(Arc::new(config)).connect(name, tcp).await.context("TLS with the relay")?)
    } else {
        Box::new(tcp)
    };
    let config = tungstenite::protocol::WebSocketConfig::default().max_message_size(Some(MAX_FRAME)).max_frame_size(Some(MAX_FRAME));
    match tokio_tungstenite::client_async_with_config(url, io, Some(config)).await {
        Ok((ws, _)) => Ok(ws),
        Err(tungstenite::Error::Http(response)) => {
            let attempts = response.headers().get("x-kelpie-reason").is_some_and(|v| v.as_bytes() == b"attempts");
            Err(Refused { status: response.status().as_u16(), attempts }.into())
        }
        Err(e) => Err(e).context("the relay's WebSocket"),
    }
}

async fn send_text(ws: &mut Socket, msg: Value) -> Result<()> {
    ws.send(WsMessage::text(msg.to_string())).await.context("writing to the relay")
}

/// The allow-list to send now: what may come in, and while a device change waits its grace (`holding`) also what was sent last, so that a
/// token given meanwhile is let in at once and a removed phone's stays until its session told it. Never more than the relay takes.
fn list_now(allowed: Vec<String>, sent: &[String], holding: bool) -> Vec<String> {
    let mut list = allowed;
    if holding {
        for h in sent {
            if list.len() >= MAX_ALLOWED {
                break;
            }
            if !list.contains(h) {
                list.push(h.clone());
            }
        }
    }
    list
}

/// Sends `list` as the room's allow-list, and keeps it as the one sent.
async fn send_list(ws: &mut Socket, list: Vec<String>, sent: &mut Vec<String>) -> Result<()> {
    send_text(ws, json!({"t": "allow", "h": list})).await?;
    *sent = list;
    Ok(())
}

/// What a session sent, on its way to the relay: dropped when its slot has another holder by now.
async fn forward(ws: &mut Socket, slots: &mut Slots, out: Out) -> Result<()> {
    match out {
        Out::Frame(slot, generation, frame) if slots.current(slot, generation) => {
            let mut framed = Vec::with_capacity(frame.len() + 2);
            framed.extend_from_slice(&slot.to_be_bytes());
            framed.extend_from_slice(&frame);
            ws.send(WsMessage::binary(framed)).await.context("writing to the relay")
        }
        Out::Kick(slot, generation, code, reason) if slots.current(slot, generation) => {
            slots.close(slot);
            send_text(ws, json!({"t": "kick", "s": slot, "c": code, "r": reason})).await
        }
        _ => Ok(()),
    }
}

/// One connection to the relay, until it ends.
async fn connection(d: &Arc<Daemon>, stop: &Generation, nudge: &Notify, ending: &Mutex<Ending>) -> Result<()> {
    // Stopped before it began: nothing to say, and no room to open (a new room's would be the wrong one to end).
    if stop.is_cancelled() {
        return Ok(());
    }
    let m = &d.mobile;
    let r = m.relay_live().ok_or_else(|| anyhow!("the relay is off"))?;
    let (Some(room), Some(ticket)) = (r.room.clone(), r.ticket.clone()) else { bail!("this computer has no room at the relay") };
    let key = m.room_key()?.ok_or_else(|| anyhow!("this computer's room key is gone: make a new room in Settings → Mobile"))?;
    let ts = m.relay.next_ts();
    let url = format!("{}/v1/desktop?t={ticket}&ts={ts}&sig={}", r.url, signature(&key, &room, ts));
    let mut ws = tokio::time::timeout(CONNECT_TIMEOUT, dial(&url)).await.map_err(|_| anyhow!("the relay did not answer in time"))??;
    // What the relay must know before it says which phones are there: who may be let in, and from what page.
    let mut sent = Vec::new();
    send_list(&mut ws, m.relay_allowed(), &mut sent).await?;
    if let Some(origin) = &r.origin {
        send_text(&mut ws, json!({"t": "origin", "o": origin})).await?;
    }
    for (hash, ttl) in m.relay.live_pairs() {
        send_text(&mut ws, json!({"t": "pair", "h": hash, "ttl": ttl})).await?;
    }
    let (out_tx, mut out_rx) = mpsc::channel::<Out>(OUT_QUEUE);
    let (control_tx, mut control_rx) = mpsc::unbounded_channel::<String>();
    m.relay.up(control_tx);
    d.broadcast("mobile.updated", json!({}));
    tracing::info!("relay: connected");
    let mut slots = Slots::default();
    let mut devices = m.revoked.subscribe();
    // A device change reaches the relay this long after it is made: a removed phone's session first tells it, inside the session, and closes its slot.
    let mut list_due: Option<tokio::time::Instant> = None;
    let mut keepalive = tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut refresh = tokio::time::interval_at(tokio::time::Instant::now() + ALLOW_REFRESH, ALLOW_REFRESH);
    let mut heard = Instant::now();
    let mut pinged: Option<Instant> = None;
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => {
                let wanted = ending.lock().clone();
                if let Ending::Flush(last) = wanted {
                    let until = tokio::time::Instant::now() + FLUSH;
                    while let Ok(Some(out)) = tokio::time::timeout_at(until, out_rx.recv()).await {
                        forward(&mut ws, &mut slots, out).await?;
                    }
                    // A room that goes takes everything with it, and its phones hear that it is gone (4004); one that stays lets nobody in. A word
                    // for another room is said on a connection of its own (`run`).
                    if let Some(word) = said_on_ending(last.as_deref(), &room) {
                        send_text(&mut ws, word).await?;
                        *ending.lock() = Ending::Now;
                    }
                }
                let _ = ws.close(None).await;
                return Ok(());
            }
            // A device added, changed or removed: the relay's list follows the store's, a moment later (`list_due`).
            event = devices.recv() => {
                if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                    return Ok(());
                }
                list_due.get_or_insert(tokio::time::Instant::now() + REVOKE_GRACE);
            }
            _ = sleep_until_due(list_due) => {
                list_due = None;
                send_list(&mut ws, m.relay_allowed(), &mut sent).await?;
            }
            Some(text) = control_rx.recv() => ws.send(WsMessage::text(text)).await.context("writing to the relay")?,
            // A new token goes at once; while a device change waits its grace, nothing sent was taken off yet (`list_now`), so no removed phone
            // is closed before its session told it.
            _ = nudge.notified() => {
                let list = list_now(m.relay_allowed(), &sent, list_due.is_some());
                send_list(&mut ws, list, &mut sent).await?;
            }
            _ = refresh.tick() => {
                let list = list_now(m.relay_allowed(), &sent, list_due.is_some());
                send_list(&mut ws, list, &mut sent).await?;
            }
            _ = keepalive.tick() => {
                if heard.elapsed() >= DEAD_AFTER {
                    bail!("the relay stopped answering");
                }
                pinged = Some(Instant::now());
                ws.send(WsMessage::text("ping")).await.context("writing to the relay")?;
            }
            Some(out) = out_rx.recv() => forward(&mut ws, &mut slots, out).await?,
            msg = ws.next() => {
                let msg = match msg {
                    Some(Ok(msg)) => msg,
                    Some(Err(e)) => return Err(e).context("reading from the relay"),
                    None => bail!("the relay closed the connection"),
                };
                heard = Instant::now();
                match msg {
                    WsMessage::Text(text) if text.as_str() == "pong" => {
                        if let Some(at) = pinged.take() {
                            m.relay.rtt(at.elapsed());
                        }
                    }
                    WsMessage::Text(text) => on_control(d, text.as_str(), &mut slots, &out_tx),
                    WsMessage::Binary(frame) if frame.len() >= 2 => {
                        let slot = u16::from_be_bytes([frame[0], frame[1]]);
                        // A phone that sends faster than its connection reads goes, rather than holding up the others.
                        let full = slots.sender(slot).is_some_and(|tx| tx.try_send(Ok(frame[2..].to_vec())).is_err());
                        if full {
                            slots.close(slot);
                            send_text(&mut ws, json!({"t": "kick", "s": slot, "c": super::SLOW_CONSUMER, "r": "slow"})).await?;
                        }
                    }
                    WsMessage::Close(frame) => bail!("the relay closed the connection ({})", frame.map_or(1005, |f| u16::from(f.code))),
                    _ => {}
                }
            }
        }
    }
}

async fn sleep_until_due(due: Option<tokio::time::Instant>) {
    match due {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// What the relay says of a phone slot: `open` starts a connection on it, as an upgrade on `/link` does; `close` ends it.
fn on_control(d: &Arc<Daemon>, text: &str, slots: &mut Slots, out: &mpsc::Sender<Out>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    let Some(slot) = v["s"].as_u64().and_then(|s| u16::try_from(s).ok()) else { return };
    match v["t"].as_str() {
        Some("open") => {
            let running = d.mobile.running.lock().as_ref().map(|r| r.stop.clone());
            // A slot opened again replaces what was there: its frames stop, and that connection ends.
            let (generation, frames) = slots.open(slot);
            let Some(running) = running else {
                let _ = out.try_send(Out::Kick(slot, generation, super::RECONNECT, "off".into()));
                return;
            };
            let origin = d.mobile.relay_live().and_then(|r| r.origin);
            let carrier = Carrier::Relay(Slot { slot, generation, frames, out: out.clone() });
            let ip = phone_ip(slot, v["k"].as_str());
            tokio::spawn(link::accept(d.clone(), carrier, running, Peer::relayed(ip, origin), true));
        }
        // A relay from before codes were passed on says none: as if the socket dropped.
        Some("close") => slots.closed(slot, v["c"].as_u64().and_then(|c| u16::try_from(c).ok()).unwrap_or(1005)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::store::Kind;
    use crate::mobile::Permission;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../../relay/test/vectors.json")).unwrap()
    }

    #[test]
    fn the_shared_vectors_hold_here_as_at_the_relay() {
        use p256::ecdsa::signature::Verifier;
        let v = vectors();
        let private = super::super::unhex32(v["room_key_private_hex"].as_str().unwrap()).unwrap();
        let key = SigningKey::from_slice(&private).unwrap();
        assert_eq!(public_key(&key), v["room_key_public"], "the public key the relay keeps");
        let (room, ts) = (v["room"].as_str().unwrap(), v["ts"].as_u64().unwrap());
        assert_eq!(statement(room, ts), v["statement"]);
        // A signature WebCrypto made, which is what the relay checks with.
        let theirs = Signature::from_slice(&kelpie_link::unb64(v["signature"].as_str().unwrap()).unwrap()).unwrap();
        assert!(key.verifying_key().verify(statement(room, ts).as_bytes(), &theirs).is_ok());
        // And one of ours, in the form the relay takes: r || s, base64url.
        let ours = signature(&key, room, ts);
        assert_eq!(ours.len(), 86);
        let ours = Signature::from_slice(&kelpie_link::unb64(&ours).unwrap()).unwrap();
        assert!(key.verifying_key().verify(statement(room, ts).as_bytes(), &ours).is_ok());
        assert!(key.verifying_key().verify(statement(room, ts + 1).as_bytes(), &ours).is_err());

        let psk = super::super::unhex32(v["psk_hex"].as_str().unwrap()).unwrap();
        assert_eq!(kelpie_link::b64(&pair_token(&psk)), v["pair_token"]);
        assert_eq!(token_hash(&pair_token(&psk)), v["pair_token_sha256"]);
        assert_eq!(token_hash(&kelpie_link::unb64(v["device_token"].as_str().unwrap()).unwrap()), v["device_token_sha256"]);
    }

    #[test]
    fn a_slot_has_an_address_no_network_gives() {
        for slot in [0, 7, u16::MAX] {
            assert!(is_slot_ip(slot_ip(slot)));
            assert_eq!(listen::shown(slot_ip(slot)), "the relay");
        }
        for ip in ["127.0.0.1", "::1", "100.100.1.2", "fd7a:115c:a1e0::5", "192.168.1.9", "::"] {
            assert!(!is_slot_ip(ip.parse().unwrap()), "{ip}");
        }
        let relayed = Peer::relayed(slot_ip(3), Some("https://desk.tail1234.ts.net:8443".into()));
        assert!(relayed.via_relay() && relayed.secure_origin);
        assert_eq!(relayed.ip(), slot_ip(3));
        assert!(!Peer::relayed(slot_ip(3), None).secure_origin, "no origin registered: nothing to call secure");
        assert!(!Peer::direct("192.168.1.9:5000".parse().unwrap(), true).via_relay());
        assert_eq!(super::super::transport_cap(Kind::Pwa, true, &relayed), Permission::Write);
    }

    #[test]
    fn only_a_relay_address_is_kept() {
        assert_eq!(normalized_url("wss://relay.example/").unwrap(), "wss://relay.example");
        assert_eq!(normalized_url(" wss://relay.example:8443 ").unwrap(), "wss://relay.example:8443");
        assert_eq!(normalized_url("ws://127.0.0.1:8787").unwrap(), "ws://127.0.0.1:8787");
        for bad in ["https://relay.example", "ws://relay.example", "wss://relay.example/v1/desktop", "wss://me@relay.example", "wss://relay.example?x=1", "relay.example", "", "wss://a;script-src.example", "wss://a,b.example", "wss://a%20b.example"] {
            assert!(normalized_url(bad).is_err(), "{bad}");
        }
        assert_eq!(http_url("wss://relay.example"), "https://relay.example");
        assert_eq!(connect_src("wss://relay.example"), "https://relay.example wss://relay.example");
    }

    #[test]
    fn what_an_old_session_of_a_slot_sends_never_reaches_the_slots_new_holder() {
        let mut slots = Slots::default();
        let (first, _a) = slots.open(7);
        let (second, _b) = slots.open(7);
        assert_ne!(first, second);
        assert!(!slots.current(7, first), "the old session's frames and kicks are dropped");
        assert!(slots.current(7, second));
        slots.close(7);
        assert!(!slots.current(7, second), "a closed slot takes nothing");
        let (third, _c) = slots.open(7);
        assert!(third > second, "generations are never reused");
    }

    #[test]
    fn a_phone_on_the_relay_goes_by_its_credential_wherever_its_socket_lands() {
        let (a, b) = ("ab".repeat(32), "cd".repeat(32));
        assert_eq!(phone_ip(1, Some(&a)), phone_ip(9, Some(&a)), "one phone's sockets add up");
        assert_ne!(phone_ip(1, Some(&a)), phone_ip(1, Some(&b)), "and cost no other phone");
        assert!(is_slot_ip(phone_ip(1, Some(&a))) && listen::shown(phone_ip(1, Some(&a))) == "the relay");
        for odd in [None, Some(""), Some("not hex"), Some(&a[..60])] {
            assert_eq!(phone_ip(4, odd), slot_ip(4), "{odd:?}");
        }
    }

    #[test]
    fn a_closed_slot_tells_its_session_the_phones_code_then_nothing() {
        let mut slots = Slots::default();
        let (_, mut frames) = slots.open(2);
        slots.closed(2, link::ROUTE_LOST);
        assert_eq!(frames.try_recv().unwrap(), Err(link::ROUTE_LOST));
        assert!(frames.try_recv().is_err(), "the slot is gone");
        assert!(slots.sender(2).is_none());
    }

    #[test]
    fn a_connection_says_its_last_word_only_to_the_room_it_is_in() {
        let word = |room: &str, drop: bool| LastWord { url: "wss://relay.example".into(), room: room.into(), ticket: format!("{room}.t"), key: SigningKey::from_slice(&[7u8; 32]).unwrap(), drop };
        assert_eq!(said_on_ending(Some(&word("old", false)), "old"), Some(json!({"t": "allow", "h": []})));
        assert_eq!(said_on_ending(Some(&word("old", true)), "old"), Some(json!({"t": "drop"})));
        // Back in the new room meanwhile: the new room is neither emptied nor dropped, and the old one is told on a connection of its own.
        assert_eq!(said_on_ending(Some(&word("old", true)), "new"), None);
        assert_eq!(said_on_ending(Some(&word("old", false)), "new"), None);
        assert_eq!(said_on_ending(None, "old"), None);
    }

    #[test]
    fn a_list_sent_during_a_removals_grace_adds_at_once_and_takes_nothing_off() {
        let h = |c: char| c.to_string().repeat(64);
        let sent = vec![h('a'), h('b')];
        // b was removed, c was just given: c goes at once, b stays until the grace is over.
        assert_eq!(list_now(vec![h('a'), h('c')], &sent, true), vec![h('a'), h('c'), h('b')]);
        assert_eq!(list_now(vec![h('a'), h('c')], &sent, false), vec![h('a'), h('c')]);
        let full: Vec<String> = (0..MAX_ALLOWED).map(|i| format!("{i:064x}")).collect();
        assert_eq!(list_now(full.clone(), &sent, true).len(), MAX_ALLOWED, "never more than the relay takes");
    }

    #[test]
    fn a_relay_refusal_says_whose_it_is() {
        let text = |status, attempts| Refused { status, attempts }.to_string();
        assert!(text(429, true).contains("too many refused attempts"), "the router's, not the room's limit");
        assert!(text(429, false).contains("monthly limit"));
        assert!(text(404, false).contains("make a new room"));
    }

    #[test]
    fn backoff_starts_at_a_second_and_stops_at_a_minute() {
        for _ in 0..50 {
            assert!(backoff(1) <= BACKOFF_FIRST);
            assert!(backoff(3) <= Duration::from_secs(4));
            assert!(backoff(40) <= BACKOFF_MAX);
        }
    }

    fn mobile() -> Mobile {
        let dir = std::env::temp_dir().join(format!("kelpie-relay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = Mobile::load(&dir, crate::notify::Memory::new());
        m.config.lock().enabled = true;
        m
    }

    /// The relay on, as `turn_on` leaves it, with no network.
    fn relay_on(m: &Mobile) {
        let key = m.mac_key(true).unwrap().unwrap();
        let mut r = RelaySettings { on: true, url: "wss://relay.example".into(), room: Some("ab".repeat(16)), ticket: Some(format!("{}.x", "ab".repeat(16))), origin: Some("https://desk.tail1234.ts.net:8443".into()), ..Default::default() };
        store::seal_relay(&mut r, &key);
        m.config.lock().relay = Some(r);
    }

    fn secure_device(m: &Mobile, id: &str) -> Device {
        m.add_test_device(id, id, kelpie_link::random_key().unwrap(), Permission::Write, Kind::Pwa);
        let key = m.mac_key(false).unwrap().unwrap();
        let mut c = m.config.lock();
        let d = c.devices.iter_mut().find(|d| d.id == id).unwrap();
        d.secure_origin = true;
        store::seal(d, &key);
        d.clone()
    }

    fn caps(held: Option<&str>) -> Value {
        json!({"caps": ["ack", "hb", "relay"], "relay_token": held})
    }

    #[test]
    fn off_or_edited_in_the_file_means_no_relay() {
        let m = mobile();
        assert!(m.relay_live().is_none() && m.relay_endpoint().is_none() && m.relay_connect_src().is_none());
        relay_on(&m);
        assert!(m.relay_live().is_some());
        assert_eq!(m.relay_connect_src().as_deref(), Some("https://relay.example wss://relay.example"));
        m.config.lock().relay.as_mut().unwrap().url = "wss://elsewhere.example".into();
        assert!(m.relay_live().is_none(), "sent elsewhere in the file");
        relay_on(&m);
        m.config.lock().enabled = false;
        assert!(m.relay_live().is_none(), "phone access off");
    }

    #[test]
    fn a_secure_phone_gets_a_token_once_and_a_new_one_after_a_week() {
        let m = mobile();
        relay_on(&m);
        let d = secure_device(&m, "pixel");
        assert!(m.relay_grant(&d, &json!({"caps": ["ack", "hb"]})).is_none(), "a client that cannot use one");
        let given: Value = serde_json::from_str(&m.relay_grant(&d, &caps(None)).unwrap()).unwrap();
        assert_eq!(given["type"], "relay");
        assert_eq!(given["url"], "wss://relay.example");
        let token = kelpie_link::unb64(given["token"].as_str().unwrap()).unwrap();
        let hash = token_hash(&token);
        assert_eq!(m.relay_allowed(), vec![hash.clone()]);
        assert!(!m.config.lock().relay.as_ref().unwrap().tokens.iter().any(|t| t.hash.len() != 64), "only hashes are kept");
        assert!(m.relay_grant(&d, &caps(Some(&hash))).is_none(), "it holds a good one");
        // A week on, it gets a new one; it keeps the newest two.
        for t in &mut m.config.lock().relay.as_mut().unwrap().tokens {
            t.issued -= TOKEN_RENEW_MS;
        }
        let key = m.mac_key(false).unwrap().unwrap();
        for t in &mut m.config.lock().relay.as_mut().unwrap().tokens {
            store::seal_token(t, &d, &key);
        }
        assert!(m.relay_grant(&d, &caps(Some(&hash))).is_some());
        assert!(m.relay_grant(&d, &caps(None)).is_some());
        assert_eq!(m.config.lock().relay.as_ref().unwrap().tokens.len(), 2, "the one it holds and the new one");
        assert_eq!(m.relay_allowed().len(), 2);
    }

    #[test]
    fn a_phone_that_missed_a_token_keeps_the_one_it_holds() {
        let m = mobile();
        relay_on(&m);
        let d = secure_device(&m, "pixel");
        let key = m.mac_key(false).unwrap().unwrap();
        let held = |given: &str| token_hash(&kelpie_link::unb64(serde_json::from_str::<Value>(given).unwrap()["token"].as_str().unwrap()).unwrap());
        let a = held(&m.relay_grant(&d, &caps(None)).unwrap());
        for t in &mut m.config.lock().relay.as_mut().unwrap().tokens {
            t.issued -= TOKEN_RENEW_MS;
            store::seal_token(t, &d, &key);
        }
        // Given on a connection that dropped before the phone heard it.
        let b = held(&m.relay_grant(&d, &caps(None)).unwrap());
        // The phone comes back with the token it has, which is due for a new one.
        let c = held(&m.relay_grant(&d, &caps(Some(&a))).unwrap());
        let allowed = m.relay_allowed();
        assert!(allowed.contains(&a), "what the phone holds still lets it in");
        assert!(allowed.contains(&c));
        assert!(!allowed.contains(&b), "the one it never got goes");
    }

    #[test]
    fn the_allow_list_leaves_out_what_may_not_come_in() {
        let m = mobile();
        relay_on(&m);
        let d = secure_device(&m, "pixel");
        m.relay_grant(&d, &caps(None)).unwrap();
        // A page paired over plain HTTP gets nothing.
        m.add_test_device("plain", "plain", kelpie_link::random_key().unwrap(), Permission::Read, Kind::Pwa);
        let plain = m.config.lock().devices.iter().find(|d| d.id == "plain").unwrap().clone();
        assert!(m.relay_grant(&plain, &caps(None)).is_none());
        assert_eq!(m.relay_allowed().len(), 1);
        // A token edited in the file.
        m.config.lock().relay.as_mut().unwrap().tokens[0].expires += 1;
        assert!(m.relay_allowed().is_empty());
        let key = m.mac_key(false).unwrap().unwrap();
        store::seal_token(&mut m.config.lock().relay.as_mut().unwrap().tokens[0], &d, &key);
        assert_eq!(m.relay_allowed().len(), 1);
        // Removed, even while the relay is away: the next list it gets leaves the phone out.
        m.revoke("pixel").unwrap();
        assert!(m.relay_allowed().is_empty());
        // An expired one.
        let d = secure_device(&m, "pixel2");
        m.relay_grant(&d, &caps(None)).unwrap();
        for t in &mut m.config.lock().relay.as_mut().unwrap().tokens {
            t.expires = now_millis() - 1;
            store::seal_token(t, &d, &key);
        }
        assert!(m.relay_allowed().is_empty());
    }
}
