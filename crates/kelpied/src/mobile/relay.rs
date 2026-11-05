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
//! - **Slots**: each phone socket feeds `link::accept` like a socket on `/link`, at an address of its own for the limits (`slot_ip`):
//!   the same handshake budget, the same `mac1` check before any Diffie-Hellman. A slot serves only a device paired on a secure
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
/// A phone keeps its newest two: the one it holds, and a new one it may not have received.
const TOKENS_PER_DEVICE: usize = 2;
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

/// A slot's address, for the limits and for what the person reads: in 100::/64, the block set aside to go nowhere (RFC 6666), so
/// no phone on any network can have one.
pub fn slot_ip(slot: u16) -> IpAddr {
    IpAddr::V6(Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0x6b6c, slot))
}

pub fn is_slot_ip(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V6(v6) if v6.segments()[..4] == [0x0100, 0, 0, 0])
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

/// What goes to the relay.
pub enum Out {
    Frame(u16, Vec<u8>),
    /// Close that phone's socket, with this code.
    Kick(u16, u16, String),
    Control(String),
}

/// One phone's socket at the relay, as `link` sees it.
pub struct Slot {
    slot: u16,
    frames: mpsc::Receiver<Vec<u8>>,
    out: mpsc::Sender<Out>,
}

impl Slot {
    /// The phone's next frame; None once it left or the relay's connection ended.
    pub async fn recv(&mut self) -> Option<Vec<u8>> {
        self.frames.recv().await
    }

    pub async fn send(&self, frame: Vec<u8>) -> bool {
        self.out.send(Out::Frame(self.slot, frame)).await.is_ok()
    }

    pub async fn close(&self, code: u16, reason: String) {
        let _ = self.out.send(Out::Kick(self.slot, code, reason)).await;
    }
}

// ---------- state ----------

/// The relay's side of phone access, in memory.
#[derive(Default)]
pub struct Runtime {
    task: Mutex<Option<Task>>,
    facts: Mutex<Facts>,
    /// The connection's queue, while it is up.
    out: Mutex<Option<mpsc::Sender<Out>>>,
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
}

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

    fn up(&self, out: mpsc::Sender<Out>) {
        *self.out.lock() = Some(out);
        let mut f = self.facts.lock();
        f.connected = true;
        f.error = None;
    }

    fn down(&self, error: Option<String>) {
        *self.out.lock() = None;
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
        if let Some(out) = self.out.lock().clone() {
            let _ = out.try_send(Out::Control(msg.to_string()));
        }
    }

    /// The tokens changed: the list goes to the relay again.
    fn nudge(&self) {
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
            let mut mine: Vec<u64> = relay.tokens.iter().filter(|x| x.device == device.id).map(|x| x.issued).collect();
            mine.sort_unstable();
            let keep_from = mine.len().saturating_sub(TOKENS_PER_DEVICE - 1);
            let oldest_kept = mine.get(keep_from).copied();
            relay.tokens.retain(|x| x.device != device.id || oldest_kept.is_some_and(|o| x.issued >= o));
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
        *task = Some(Task { stop: stop.clone(), nudge: nudge.clone() });
        tokio::spawn(run(Arc::downgrade(daemon), stop, nudge));
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
    // Kept once the relay took it (or it was not needed now): a wrong key typed never replaces a good one.
    if let Some(k) = &asked.operator_key {
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
    let old = m.relay.out.lock().clone();
    m.keep_room_key(&room_key)?;
    let key = m.mac_key(false)?.ok_or_else(|| anyhow!("the device key cannot be read now"))?;
    {
        let mut c = m.config.lock();
        let r = c.relay.as_mut().ok_or_else(|| anyhow!("the relay was turned off meanwhile"))?;
        r.room = Some(room);
        r.ticket = Some(ticket);
        store::seal_relay(r, &key);
    }
    m.keep();
    m.audit("-", "relay_room", "a new room");
    m.relay.facts.lock().slow_since = None;
    readvertise(m);
    tokio::time::sleep(MOVE_GRACE).await;
    if let Some(out) = old {
        let _ = out.send(Out::Control(json!({"t": "drop"}).to_string())).await;
    }
    restart(&daemon);
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

async fn run(daemon: Weak<Daemon>, stop: Arc<Generation>, nudge: Arc<Notify>) {
    let mut attempt: u32 = 0;
    loop {
        let Some(d) = daemon.upgrade() else { return };
        let began = Instant::now();
        let ended = tokio::select! {
            biased;
            _ = stop.cancelled() => return,
            ended = connection(&d, &stop, &nudge) => ended,
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
        Err(tungstenite::Error::Http(response)) => Err(match response.status().as_u16() {
            403 => anyhow!("the relay refused this computer's signature"),
            404 => anyhow!("the relay no longer has this computer's room: make a new room in Settings → Mobile"),
            429 => anyhow!("the room reached its monthly limit at the relay"),
            s => anyhow!("the relay answered HTTP {s}"),
        }),
        Err(e) => Err(e).context("the relay's WebSocket"),
    }
}

async fn send_text(ws: &mut Socket, msg: Value) -> Result<()> {
    ws.send(WsMessage::text(msg.to_string())).await.context("writing to the relay")
}

fn allow_message(m: &Mobile) -> Value {
    json!({"t": "allow", "h": m.relay_allowed()})
}

/// One connection to the relay, until it ends.
async fn connection(d: &Arc<Daemon>, stop: &Generation, nudge: &Notify) -> Result<()> {
    let m = &d.mobile;
    let r = m.relay_live().ok_or_else(|| anyhow!("the relay is off"))?;
    let (Some(room), Some(ticket)) = (r.room.clone(), r.ticket.clone()) else { bail!("this computer has no room at the relay") };
    let key = m.room_key()?.ok_or_else(|| anyhow!("this computer's room key is gone: make a new room in Settings → Mobile"))?;
    let ts = m.relay.next_ts();
    let url = format!("{}/v1/desktop?t={ticket}&ts={ts}&sig={}", r.url, signature(&key, &room, ts));
    let mut ws = tokio::time::timeout(CONNECT_TIMEOUT, dial(&url)).await.map_err(|_| anyhow!("the relay did not answer in time"))??;
    // What the relay must know before it says which phones are there: who may be let in, and from what page.
    send_text(&mut ws, allow_message(m)).await?;
    if let Some(origin) = &r.origin {
        send_text(&mut ws, json!({"t": "origin", "o": origin})).await?;
    }
    for (hash, ttl) in m.relay.live_pairs() {
        send_text(&mut ws, json!({"t": "pair", "h": hash, "ttl": ttl})).await?;
    }
    let (out_tx, mut out_rx) = mpsc::channel::<Out>(OUT_QUEUE);
    m.relay.up(out_tx.clone());
    d.broadcast("mobile.updated", json!({}));
    tracing::info!("relay: connected");
    let mut slots: HashMap<u16, mpsc::Sender<Vec<u8>>> = HashMap::new();
    let mut devices = m.revoked.subscribe();
    let mut keepalive = tokio::time::interval_at(tokio::time::Instant::now() + KEEPALIVE, KEEPALIVE);
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut refresh = tokio::time::interval_at(tokio::time::Instant::now() + ALLOW_REFRESH, ALLOW_REFRESH);
    let mut heard = Instant::now();
    let mut pinged: Option<Instant> = None;
    loop {
        tokio::select! {
            biased;
            _ = stop.cancelled() => {
                let _ = ws.close(None).await;
                return Ok(());
            }
            // A device added, changed or removed: the relay's list follows the store's (a revoked phone's socket also closes from its own connection).
            event = devices.recv() => {
                if matches!(event, Err(broadcast::error::RecvError::Closed)) {
                    return Ok(());
                }
                send_text(&mut ws, allow_message(m)).await?;
            }
            _ = nudge.notified() => send_text(&mut ws, allow_message(m)).await?,
            _ = refresh.tick() => send_text(&mut ws, allow_message(m)).await?,
            _ = keepalive.tick() => {
                if heard.elapsed() >= DEAD_AFTER {
                    bail!("the relay stopped answering");
                }
                pinged = Some(Instant::now());
                ws.send(WsMessage::text("ping")).await.context("writing to the relay")?;
            }
            Some(out) = out_rx.recv() => match out {
                Out::Frame(slot, frame) => {
                    let mut framed = Vec::with_capacity(frame.len() + 2);
                    framed.extend_from_slice(&slot.to_be_bytes());
                    framed.extend_from_slice(&frame);
                    ws.send(WsMessage::binary(framed)).await.context("writing to the relay")?;
                }
                Out::Kick(slot, code, reason) => {
                    slots.remove(&slot);
                    send_text(&mut ws, json!({"t": "kick", "s": slot, "c": code, "r": reason})).await?;
                }
                Out::Control(text) => ws.send(WsMessage::text(text)).await.context("writing to the relay")?,
            },
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
                        let full = slots.get(&slot).is_some_and(|tx| tx.try_send(frame[2..].to_vec()).is_err());
                        if full {
                            slots.remove(&slot);
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

/// What the relay says of a phone slot: `open` starts a connection on it, as an upgrade on `/link` does; `close` ends it.
fn on_control(d: &Arc<Daemon>, text: &str, slots: &mut HashMap<u16, mpsc::Sender<Vec<u8>>>, out: &mpsc::Sender<Out>) {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return };
    let Some(slot) = v["s"].as_u64().and_then(|s| u16::try_from(s).ok()) else { return };
    match v["t"].as_str() {
        Some("open") => {
            let generation = d.mobile.running.lock().as_ref().map(|r| r.stop.clone());
            let Some(generation) = generation else {
                let _ = out.try_send(Out::Kick(slot, super::RECONNECT, "off".into()));
                return;
            };
            let (tx, frames) = mpsc::channel(SLOT_QUEUE);
            // A slot opened again replaces what was there: its frames stop, and that connection ends.
            slots.insert(slot, tx);
            let origin = d.mobile.relay_live().and_then(|r| r.origin);
            let carrier = Carrier::Relay(Slot { slot, frames, out: out.clone() });
            tokio::spawn(link::accept(d.clone(), carrier, generation, Peer::relayed(slot, origin), true));
        }
        Some("close") => {
            slots.remove(&slot);
        }
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
        let relayed = Peer::relayed(3, Some("https://desk.tail1234.ts.net:8443".into()));
        assert!(relayed.via_relay() && relayed.secure_origin);
        assert_eq!(relayed.ip(), slot_ip(3));
        assert!(!Peer::relayed(3, None).secure_origin, "no origin registered: nothing to call secure");
        assert!(!Peer::direct("192.168.1.9:5000".parse().unwrap(), true).via_relay());
        assert_eq!(super::super::transport_cap(Kind::Pwa, true, &relayed), Permission::Write);
    }

    #[test]
    fn only_a_relay_address_is_kept() {
        assert_eq!(normalized_url("wss://relay.example/").unwrap(), "wss://relay.example");
        assert_eq!(normalized_url(" wss://relay.example:8443 ").unwrap(), "wss://relay.example:8443");
        assert_eq!(normalized_url("ws://127.0.0.1:8787").unwrap(), "ws://127.0.0.1:8787");
        for bad in ["https://relay.example", "ws://relay.example", "wss://relay.example/v1/desktop", "wss://me@relay.example", "wss://relay.example?x=1", "relay.example", ""] {
            assert!(normalized_url(bad).is_err(), "{bad}");
        }
        assert_eq!(http_url("wss://relay.example"), "https://relay.example");
        assert_eq!(connect_src("wss://relay.example"), "https://relay.example wss://relay.example");
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
        assert_eq!(m.config.lock().relay.as_ref().unwrap().tokens.len(), TOKENS_PER_DEVICE);
        assert_eq!(m.relay_allowed().len(), TOKENS_PER_DEVICE);
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
