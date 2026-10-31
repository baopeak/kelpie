//! Mobile companion: a small web app the daemon serves on the local network, so a phone can see
//! every session, what needs the user, read a session's screen and (when allowed) answer it.
//!
//! Protocol v2 (JSON messages, inside kelpie-link on `/link`): the session list is pushed when it changes; a
//! terminal session's screen is followed with `sub` and arrives as row diffs (`sync`) that the phone
//! acknowledges (`ack`); a chat session is watched with `chat_sub` and its permission requests
//! answered with `chat_answer`. Sessions are addressed by id (handles are not stable across daemon
//! restarts) and what a phone may ask for is the closed table in `verbs`. A connection is built for
//! a bad network and a busy computer: heartbeats in both directions, two bounded lanes to the socket
//! (`lanes`), and rates that follow the machine's load (`load`).
//!
//! The page is a remote shell on this machine, so: off by default; bound to the machine's private and
//! Tailscale addresses, never 0.0.0.0 (`listen`); every message end-to-end encrypted with Noise (`link`,
//! the `kelpie-link` crate), whatever carries it; a device pairs with a single-use secret in a QR code (2
//! minutes) and only after the person types, in the Kelpie app, the code the phone shows and passes the OS
//! verification the daemon runs (`user_presence`); each device is a public key in a MAC'd record (`store`), `read` until raised, revocable; a
//! page in a browser may type only over HTTPS or Tailscale; and HTTPS is used when a certificate is
//! configured (e.g. from `tailscale cert`).

mod audit;
mod lanes;
mod link;
mod listen;
mod store;
mod sync;
mod verbs;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use parking_lot::Mutex;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, oneshot, watch, Notify};

use crate::daemon::{Daemon, Entry};
use crate::load::{Level, Load};
use crate::notify::SecretStore;
use crate::persist::write_atomic;
use crate::session::{bounded_client_channel, next_client_id, now_millis, Answer, AnswerError, By, ClientTx, Outgoing, Resolution};
use lanes::{Beat, Liveness, SLOW_AFTER};
use link::{Opened, Peer, Wire};
use store::{Device, Kind, Store};
pub use verbs::Permission;
use verbs::Verb;

pub const DEFAULT_PORT: u16 = 4817;
const CODE_TTL: Duration = Duration::from_secs(120);
/// Pairing codes alive at once (each QR shown makes one).
const LIVE_CODES: usize = 4;
/// Failed pairing attempts one address may make in a minute.
const PAIR_FAILURES: usize = 10;
/// Addresses the limiter remembers at once.
const LIMITER_ADDRESSES: usize = 1024;
/// A paired device may handshake this often (a flaky network reconnects in bursts).
const RECONNECT_BURST: usize = 5;
const RECONNECT_WINDOW: Duration = Duration::from_secs(10);
/// Wrong codes typed for one pairing request before it is declined.
const CODE_TRIES: u8 = 3;
/// First messages one pairing code answers per source address (a first message can be replayed); the rest are dropped.
const CODE_ATTEMPTS: u8 = 5;
/// Handshakes (a Diffie-Hellman each) the daemon does in a second, and handshakes under way at once: over either, a first
/// message is dropped unanswered. A WebSocket carries exactly one handshake, so one connection can do no more than that.
const HANDSHAKES_PER_SECOND: f64 = 20.0;
const HANDSHAKES_PENDING: usize = 16;
/// Of those, what one address may hold. When all of them are taken the oldest that has said nothing gives way to the newcomer, and a
/// first message seen before is dropped, so a place is held only by a phone that is really shaking hands. This machine itself is not
/// held to the address limit (no network sits between, and it could as well read the keys).
const HANDSHAKES_PENDING_PER_ADDRESS: usize = 4;
/// What one address may take of the machine's handshakes: a replayed first message costs a Diffie-Hellman, and can be sent by anyone
/// who saw one on plain HTTP.
const HANDSHAKES_PER_SECOND_PER_ADDRESS: f64 = 5.0;
/// WebSocket upgrades one address may make: a second's worth, then this many a second.
const UPGRADES_PER_SECOND_PER_ADDRESS: f64 = 5.0;
const UPGRADE_BURST: f64 = 20.0;
/// So many connections given way or refused in a minute are a flood, which goes in the audit log once in `FLOOD_NOTE_GAP`.
const FLOOD_EVENTS: u32 = 20;
const FLOOD_NOTE_GAP: Duration = Duration::from_secs(300);
/// First messages remembered, and for how long: a phone's is new every time, since it carries a fresh ephemeral key, so seeing one again
/// means someone replays what they saw on the network.
const FIRST_MESSAGES_KEPT: usize = 256;
const FIRST_MESSAGE_MEMORY: Duration = Duration::from_secs(120);
/// What the person is told when a change that takes access away could not be written.
pub const NOT_SAVED_REMOVED: &str = "Removed for now, but this could not be saved: it will come back after a restart unless this is fixed.";
pub const NOT_SAVED_OFF: &str = "Turned off for now, but this could not be saved: it will come back on after a restart unless this is fixed.";
pub const NOT_SAVED_LOWERED: &str = "Changed for now, but this could not be saved: the phone will have its old permission after a restart unless this is fixed.";
/// A session this old starts over with a new handshake (each direction also rekeys every 2^16 messages).
const REHANDSHAKE_AFTER: Duration = Duration::from_secs(24 * 3600);
/// How often dormant devices are looked for, and connected ones noted as seen.
const SWEEP: Duration = Duration::from_secs(3600);
/// Followed and watched sessions per connection.
const MAX_FOLLOWED: usize = 16;
/// Text a phone may type in one message; a chat answer line may be longer (a tool's input).
const MAX_TYPED: usize = 16 * 1024;
const MAX_ANSWER_LINE: usize = 64 * 1024;
/// A chat message to the phone carries lines up to about this much (a longer line goes alone).
const CHAT_MESSAGE_BYTES: usize = 64 * 1024;
/// Least time between two `chat_sub` for one chat on one connection: each can mean a log of 2 MiB.
const CHAT_SUB_GAP: Duration = Duration::from_secs(1);
/// What the daemon has to say to one phone and has not yet been turned into messages: a phone
/// that lets more than this pile up is told to ask for its state again.
const EVENT_QUEUE: usize = 256;
/// A request repeated under the same `cid` (a retry after a lost reply, from any connection of the device) gets the first
/// reply again instead of being carried out twice: answers for 10 minutes, typing and keys for a minute.
const ANSWER_DEDUPE: Duration = Duration::from_secs(600);
const INPUT_DEDUPE: Duration = Duration::from_secs(60);
const DEDUPE_PER_DEVICE: usize = 512;

/// Credential-store keys: the desktop's Noise static key, and the key device records are MAC'd with.
const K_LINK: &str = "mobile-link-key";
const K_MAC: &str = "mobile-device-key";

struct Running {
    tls: bool,
    port: u16,
    listeners: Arc<listen::Listeners>,
    stop: Arc<Generation>,
}

/// Sticky shutdown: late subscribers (including HTTP upgrades) also see cancellation.
pub struct Generation {
    stopped: tokio::sync::watch::Sender<bool>,
}

impl Generation {
    fn new() -> Self {
        Self { stopped: tokio::sync::watch::channel(false).0 }
    }

    fn cancel(&self) {
        self.stopped.send_replace(true);
    }

    fn is_cancelled(&self) -> bool {
        *self.stopped.borrow()
    }

    async fn cancelled(&self) {
        let mut stopped = self.stopped.subscribe();
        let _ = stopped.wait_for(|stopped| *stopped).await;
    }

    fn watch(&self) -> watch::Receiver<bool> {
        self.stopped.subscribe()
    }
}

/// A phone that used a live code and waits for the person to type its six digits on the desktop.
struct Pending {
    name: String,
    kind: Kind,
    pk: [u8; 32],
    code: String,
    tries: u8,
    /// What the phone asks for (`read` unless it says otherwise).
    want: Permission,
    /// Where the request came from.
    address: IpAddr,
    reply: oneshot::Sender<Option<Device>>,
}

/// A pairing secret shown in a QR code.
struct Code {
    psk: [u8; 32],
    kind: Kind,
    at: Instant,
    /// First messages it has answered, by source address.
    attempts: HashMap<IpAddr, u8>,
}

/// One handshake under way.
struct PendingHandshake {
    number: u64,
    /// The address it counts against (none for this machine).
    address: Option<IpAddr>,
    give_way: Arc<Notify>,
    /// Its first message passed the check, so the phone is working on the handshake: it gives way to nobody.
    answered: bool,
}

/// The handshakes under way, oldest first.
#[derive(Default)]
struct Handshakes {
    next: u64,
    all: VecDeque<PendingHandshake>,
    by_address: HashMap<IpAddr, usize>,
}

impl Handshakes {
    fn forget_address(&mut self, address: Option<IpAddr>) {
        if let Some(ip) = address {
            if let Some(held) = self.by_address.get_mut(&ip) {
                *held = held.saturating_sub(1);
                if *held == 0 {
                    self.by_address.remove(&ip);
                }
            }
        }
    }
}

/// One handshake under way, from a connection's upgrade until its first valid sealed frame.
pub struct HandshakeSlot {
    number: u64,
    table: Arc<Mutex<Handshakes>>,
    give_way: Arc<Notify>,
}

impl HandshakeSlot {
    /// Resolves when a newer connection needs this place: the connection ends.
    pub async fn given_way(&self) {
        self.give_way.notified().await;
    }

    /// The first message passed the check: from here on this place is not given up, whoever comes.
    pub fn answered(&self) {
        if let Some(pending) = self.table.lock().all.iter_mut().find(|p| p.number == self.number) {
            pending.answered = true;
        }
    }
}

impl Drop for HandshakeSlot {
    fn drop(&mut self) {
        let mut table = self.table.lock();
        if let Some(i) = table.all.iter().position(|p| p.number == self.number) {
            if let Some(gone) = table.all.remove(i) {
                table.forget_address(gone.address);
            }
        }
    }
}

/// What the store knows about a key that opened a handshake.
pub enum KeyLookup {
    Paired(Box<Device>),
    /// A device's record that cannot be trusted now (its MAC fails, or the key that checks it cannot be read), or phone access
    /// is off or its settings unreadable: no access, and no reason for the phone to give up on its pairing.
    Unconfirmed,
    Unknown,
}

/// A pairing request that passed every check but the person's (`Mobile::check_approval`).
pub struct CheckedApproval {
    id: String,
    name: String,
    fingerprint: String,
    permission: Permission,
}

impl CheckedApproval {
    /// What the OS prompt asks.
    pub fn prompt(&self) -> crate::user_presence::Prompt {
        crate::user_presence::Prompt::Pair { name: self.name.clone(), key: self.fingerprint.clone(), permission: self.permission }
    }
}

/// A device added by `commit_approval`.
pub struct Approved {
    pub reply: Value,
    pub event: DeviceEvent,
    /// Old records of the same key it replaced.
    pub replaced: Vec<String>,
}

/// The permission a `mobile.set_permission` or `mobile.set_write` asks for: the one reading of its parameters, so that what is
/// judged is what is applied.
pub fn requested_permission(params: &Value) -> Result<Permission> {
    match params.get("permission").and_then(Value::as_str) {
        Some(p) => Permission::parse(p).ok_or_else(|| anyhow!("unknown permission {p:?}")),
        None if params.get("can_write").and_then(Value::as_bool) == Some(true) => Ok(Permission::Write),
        None => Ok(Permission::Read),
    }
}

/// The setting a `mobile.set_options` asks for, when it asks for one.
pub fn requested_dormant_days(params: &Value) -> Result<Option<u32>> {
    params.get("dormant_days").map(|days| days.as_u64().and_then(|d| u32::try_from(d).ok()).ok_or_else(|| anyhow!("dormant_days is a number of days (0: never)"))).transpose()
}

/// Something happened to a device: its own connection closes or reconnects, the others are told.
#[derive(Clone, Debug)]
pub struct DeviceEvent {
    pub id: String,
    pub name: String,
    /// `added`, `changed` or `removed`.
    pub change: &'static str,
    pub permission: Option<Permission>,
}

impl DeviceEvent {
    pub fn json(&self) -> Value {
        json!({"id": self.id, "name": self.name, "change": self.change, "permission": self.permission.map(Permission::name)})
    }
}

/// The addresses phones are told to dial, numbered so a phone keeps the newest.
#[derive(Clone, Default)]
struct Addrs {
    seq: u64,
    list: Vec<String>,
}

/// The replies kept for one device: (`cid`, when, how long it counts, the reply).
type KeptReplies = VecDeque<(String, Instant, Duration, Value)>;

pub struct Mobile {
    dir: PathBuf,
    secrets: Arc<dyn SecretStore>,
    config: Mutex<Store>,
    /// Held while the store is saved (see `save`).
    saving: Mutex<()>,
    /// The store file could not be read: phone access stays off and nothing is written over it (until a reset).
    broken: Mutex<Option<String>>,
    running: Mutex<Option<Running>>,
    last_error: Mutex<Option<String>>,
    codes: Mutex<Vec<Code>>,
    pending: Mutex<HashMap<String, Pending>>,
    /// Failed pairing and handshake attempts by source address.
    failures: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    /// Handshakes of each paired device (see `RECONNECT_BURST`).
    reconnects: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Random per daemon start: a phone that sees it change knows ids, positions and anything
    /// it had in flight belong to the daemon before.
    epoch: String,
    /// The open connection of each device: a newer one for the same device ends the older.
    connections: Mutex<HashMap<String, (u64, Arc<Notify>)>>,
    /// What each phone was last sent of each session, and the renders they share (see `sync`).
    views: Arc<sync::Views>,
    /// How busy the computer is; measured only while phone access is on.
    pub load: Arc<Load>,
    /// Chat output each open connection has been sent and has not yet taken (the logs in flight).
    waiting: Mutex<HashMap<u64, Arc<AtomicUsize>>>,
    /// When a pairing handshake was last let through (they are paced when the machine is loaded).
    paired_at: Mutex<Option<Instant>>,
    /// Project names from the app (the daemon only knows ids).
    projects: Mutex<HashMap<String, String>>,
    /// Devices added, changed, removed: their own sockets close or reconnect, the other phones are told.
    revoked: broadcast::Sender<DeviceEvent>,
    /// How fast each device has been sending chat messages (see `allow_send`).
    send_rate: Mutex<HashMap<String, SendBucket>>,
    /// The desktop's Noise static key, once read from (or made in) the credential store.
    link_key: Mutex<Option<kelpie_link::Keypair>>,
    mac_key: Mutex<Option<[u8; 32]>>,
    /// Replies by (device, cid) for repeated requests (see `ANSWER_DEDUPE`).
    replies: Mutex<HashMap<String, KeptReplies>>,
    /// Control a device holds (screen and browser takeover, later phases): each flag is checked by the control path on
    /// every input event, and cleared before a revoke or downgrade closes any socket.
    grants: Mutex<HashMap<String, Vec<Arc<AtomicBool>>>>,
    addrs: watch::Sender<Addrs>,
    /// What phones did and what was decided about them.
    audit: audit::AuditLog,
    /// How many handshakes are under way (`HANDSHAKES_PENDING`), and the machine's budget for them.
    handshakes: Arc<Mutex<Handshakes>>,
    handshake_budget: Mutex<SendBucket>,
    handshakes_by_address: Mutex<HashMap<IpAddr, SendBucket>>,
    upgrades_by_address: Mutex<HashMap<IpAddr, SendBucket>>,
    /// Connections given way or refused lately, and when that last went in the audit log.
    flood: Mutex<Flood>,
    /// The last save failed; what is in memory is ahead of the file (see `retry_save`).
    save_failed: AtomicBool,
    /// What the person was told about that, until it could be written.
    unsaved_text: Mutex<Option<String>>,
    /// A retry is running: a disk that hangs holds up one task, not one more every 10 s.
    retrying: AtomicBool,
    /// Hashes of the first messages seen lately, oldest first.
    first_messages: Mutex<VecDeque<([u8; 32], Instant)>>,
    /// The person typed a wrong code three times: the app is told (see `take_lockout`).
    locked_out: AtomicBool,
    /// The credential store would not give the key that checks device records the last time it was asked.
    key_unreadable: Mutex<bool>,
    /// A test on the only address it can bind holds this machine to the limits for an address.
    #[cfg(test)]
    loopback_is_remote: AtomicBool,
}

/// A device's key as people compare it: the start of its SHA-256 in groups of four (the phone shows the same).
pub fn fingerprint(pk: &[u8; 32]) -> String {
    let hex: String = Sha256::digest(pk).iter().take(8).map(|b| format!("{b:02X}")).collect();
    hex.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join(" ")
}

/// See `Mobile::note_flood`.
struct Flood {
    events: u32,
    since: Instant,
    noted: Option<Instant>,
}

/// Takes one from the bucket of `ip`, which fills at `rate` a second up to `burst`. False when it is empty. The table of addresses is
/// bounded: a bucket that has filled up says nothing, and goes first.
fn take_token(buckets: &Mutex<HashMap<IpAddr, SendBucket>>, ip: IpAddr, rate: f64, burst: f64) -> bool {
    let now = Instant::now();
    let mut buckets = buckets.lock();
    if buckets.len() >= LIMITER_ADDRESSES && !buckets.contains_key(&ip) {
        buckets.retain(|_, b| b.tokens + now.duration_since(b.at).as_secs_f64() * rate < burst);
        if buckets.len() >= LIMITER_ADDRESSES {
            buckets.clear();
        }
    }
    let b = buckets.entry(ip).or_insert(SendBucket { tokens: burst, at: now });
    b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * rate).min(burst);
    b.at = now;
    if b.tokens < 1.0 {
        return false;
    }
    b.tokens -= 1.0;
    true
}

/// Chat messages one device may send: this many at once, then one a second.
const SEND_BURST: f64 = 3.0;

struct SendBucket {
    tokens: f64,
    at: Instant,
}

/// Compares two strings in time independent of where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let v: Option<Vec<u8>> = (0..64).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok()).collect();
    v?.try_into().ok()
}

impl Mobile {
    pub fn load(dir: &std::path::Path, secrets: Arc<dyn SecretStore>) -> Mobile {
        let loaded = store::load(dir);
        let load = Load::new();
        let m = Mobile {
            dir: dir.to_path_buf(),
            secrets,
            config: Mutex::new(loaded.store),
            saving: Mutex::new(()),
            broken: Mutex::new(loaded.broken),
            running: Mutex::new(None),
            last_error: Mutex::new(None),
            codes: Mutex::new(Vec::new()),
            pending: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            reconnects: Mutex::new(HashMap::new()),
            epoch: uuid::Uuid::new_v4().simple().to_string(),
            connections: Mutex::new(HashMap::new()),
            views: Arc::new(sync::Views::new(load.clone())),
            load,
            waiting: Mutex::new(HashMap::new()),
            paired_at: Mutex::new(None),
            projects: Mutex::new(HashMap::new()),
            revoked: broadcast::channel(16).0,
            send_rate: Mutex::new(HashMap::new()),
            link_key: Mutex::new(None),
            mac_key: Mutex::new(None),
            replies: Mutex::new(HashMap::new()),
            grants: Mutex::new(HashMap::new()),
            addrs: watch::channel(Addrs::default()).0,
            audit: audit::AuditLog::new(dir),
            handshakes: Arc::default(),
            handshakes_by_address: Mutex::new(HashMap::new()),
            upgrades_by_address: Mutex::new(HashMap::new()),
            flood: Mutex::new(Flood { events: 0, since: Instant::now(), noted: None }),
            save_failed: AtomicBool::new(false),
            unsaved_text: Mutex::new(None),
            retrying: AtomicBool::new(false),
            first_messages: Mutex::new(VecDeque::new()),
            handshake_budget: Mutex::new(SendBucket { tokens: HANDSHAKES_PER_SECOND, at: Instant::now() }),
            locked_out: AtomicBool::new(false),
            key_unreadable: Mutex::new(false),
            #[cfg(test)]
            loopback_is_remote: AtomicBool::new(false),
        };
        m.verify_devices();
        if loaded.fresh {
            m.keep();
        }
        m
    }

    /// Marks each device whose record carries the MAC it was given; the rest are refused until paired again. When the key that
    /// checks them cannot be read now nothing is concluded: they stay unchecked, and are looked at again when it can be
    /// (called when the app looks and when such a phone connects).
    fn verify_devices(&self) {
        if self.config.lock().devices.iter().all(|d| d.verified) {
            *self.key_unreadable.lock() = false;
            return;
        }
        let key = match self.mac_key(false) {
            Ok(key) => key,
            Err(e) => {
                tracing::warn!("phone access: device records cannot be checked now: {e:#}");
                *self.key_unreadable.lock() = true;
                return;
            }
        };
        *self.key_unreadable.lock() = false;
        for d in self.config.lock().devices.iter_mut().filter(|d| !d.verified) {
            d.verified = key.as_ref().is_some_and(|k| store::verify(d, k));
            if !d.verified {
                tracing::warn!("phone access: the record of device {:?} does not verify; it must be paired again", d.name);
            }
        }
    }

    /// The key device records are MAC'd with. Made when `create` and only when the store says it has none: a read that failed,
    /// or a value that is no key, is an error and nothing is written over it (every record depends on this key).
    fn mac_key(&self, create: bool) -> Result<Option<[u8; 32]>> {
        let mut cached = self.mac_key.lock();
        if cached.is_some() {
            return Ok(*cached);
        }
        let k = match self.secrets.read(K_MAC).context("reading the device key")? {
            Some(s) => unhex32(&s).ok_or_else(|| anyhow!("the device key in the credential store is damaged: reset phone access"))?,
            None if create => {
                let k = kelpie_link::random_key().map_err(|e| anyhow!("{e}"))?;
                self.secrets.set(K_MAC, &hex(&k)).context("keeping the device key")?;
                k
            }
            None => return Ok(None),
        };
        *cached = Some(k);
        Ok(Some(k))
    }

    /// The desktop's Noise static key (made and kept in the credential store on first need).
    fn link_key(&self) -> Result<kelpie_link::Keypair> {
        let mut cached = self.link_key.lock();
        if let Some(k) = cached.as_ref() {
            return Ok(k.clone());
        }
        // Made only when the store says it has none: every phone's pairing depends on this key, so a read that failed or a value
        // that is no key is an error, and nothing is written over it.
        let k = match self.secrets.read(K_LINK).context("reading the phone link key")? {
            Some(s) => {
                let parts = (|| Some(kelpie_link::Keypair { private: unhex32(s.get(..64)?)?, public: unhex32(s.get(64..)?)? }))();
                parts.ok_or_else(|| anyhow!("the phone link key in the credential store is damaged: reset phone access"))?
            }
            None => {
                let k = kelpie_link::Keypair::generate().map_err(|e| anyhow!("{e}"))?;
                self.secrets.set(K_LINK, &format!("{}{}", hex(&k.private), hex(&k.public))).context("keep the phone link key in the credential store")?;
                k
            }
        };
        *cached = Some(k.clone());
        Ok(k)
    }

    /// Gives back the token `allow_send` took.
    fn refund_send(&self, device: &str) {
        if let Some(b) = self.send_rate.lock().get_mut(device) {
            b.tokens = (b.tokens + 1.0).min(SEND_BURST);
        }
    }

    /// Whether `device` may send a chat message now: at most one a second, in bursts of three. A message the chat then refuses
    /// (`refund_send`: too long, a command for the computer, a full queue) costs nothing: only a message that gets in spends one.
    /// The limiter is as strong as it was for what gets in.
    fn allow_send(&self, device: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.send_rate.lock();
        if buckets.len() > 256 {
            buckets.retain(|_, b| now.duration_since(b.at) < Duration::from_secs(5));
        }
        let b = buckets.entry(device.to_string()).or_insert(SendBucket { tokens: SEND_BURST, at: now });
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64()).min(SEND_BURST);
        b.at = now;
        if b.tokens < 1.0 {
            return false;
        }
        b.tokens -= 1.0;
        true
    }

    /// Writes the store, unless the file on disk could not be read (it is never written over then). A slow disk holds up only other
    /// saves: every message of every phone reads the config, so its lock is held just to copy it. Saves stay in order, so
    /// an older copy is never written over a newer one.
    fn save(&self) -> Result<()> {
        let _one_at_a_time = self.saving.lock();
        if self.broken.lock().is_some() {
            return Ok(());
        }
        let bytes = serde_json::to_vec_pretty(&*self.config.lock());
        let written = bytes.map_err(anyhow::Error::from).and_then(|bytes| write_atomic(&store::path(&self.dir), &bytes));
        self.save_failed.store(written.is_err(), Ordering::SeqCst);
        if written.is_ok() {
            self.unsaved_text.lock().take();
        }
        written.with_context(|| format!("saving {}", store::FILE))
    }

    /// `save` where a failure only needs trying again: said in the log, and picked up by `retry_save`.
    fn keep(&self) {
        if let Err(e) = self.save() {
            tracing::warn!("phone access: {e:#}");
        }
    }

    /// Whether what is in memory is ahead of the file.
    pub fn save_pending(&self) -> bool {
        self.save_failed.load(Ordering::SeqCst)
    }

    /// Reports a change that takes access away and could not be written. It holds for now and is lost at a restart, which is the one
    /// thing the person must not be left unaware of.
    pub fn unsaved(&self, what: &str) -> Result<()> {
        if !self.save_pending() {
            return Ok(());
        }
        // A phone that keeps its old permission does not hide from the person that another comes back, or that access comes back on.
        let mut text = self.unsaved_text.lock();
        if what != NOT_SAVED_LOWERED || text.is_none() {
            *text = Some(what.to_string());
        }
        drop(text);
        self.audit.add("-", "save_failed", what);
        bail!("{what}")
    }

    /// Whether a new retry may start: the one before it has ended. Followed by `retry_save` and `retry_ended`.
    pub fn claim_retry(&self) -> bool {
        !self.retrying.swap(true, Ordering::SeqCst)
    }

    pub fn retry_ended(&self) {
        self.retrying.store(false, Ordering::SeqCst);
    }

    /// Whether this first message was seen lately. A repeat is a replay: a phone's own retry is a new message.
    fn first_message_seen(&self, first: &[u8]) -> bool {
        let hash: [u8; 32] = Sha256::digest(first).into();
        let mut seen = self.first_messages.lock();
        while seen.front().is_some_and(|(_, at)| at.elapsed() >= FIRST_MESSAGE_MEMORY) {
            seen.pop_front();
        }
        if seen.iter().any(|(h, _)| *h == hash) {
            return true;
        }
        if seen.len() >= FIRST_MESSAGES_KEPT {
            seen.pop_front();
        }
        seen.push_back((hash, Instant::now()));
        false
    }

    /// Tries again to write what could not be. True when it now could, and the person's warning is gone.
    pub fn retry_save(&self) -> bool {
        if let Err(e) = self.save() {
            tracing::debug!("phone access: still: {e:#}");
            return false;
        }
        self.audit.add("-", "saved", "the change that could not be saved has been now");
        true
    }

    /// Connections given way or refused: a flood that goes on goes in the audit log, at most once in `FLOOD_NOTE_GAP`.
    fn note_flood(&self, what: &str, ip: IpAddr) {
        let mut flood = self.flood.lock();
        if flood.since.elapsed() > Duration::from_secs(60) {
            flood.events = 0;
            flood.since = Instant::now();
        }
        flood.events += 1;
        if flood.events >= FLOOD_EVENTS && flood.noted.is_none_or(|at| at.elapsed() >= FLOOD_NOTE_GAP) {
            flood.noted = Some(Instant::now());
            let events = flood.events;
            drop(flood);
            self.audit.add("-", "flood", &format!("{events} connections given way or refused in a minute, the last: {what} from {ip}"));
        }
    }

    /// Whether `ip` may start another WebSocket now. Asked before the upgrade: a flood costs no task.
    fn upgrade_allowed(&self, ip: IpAddr) -> bool {
        if self.this_machine(ip) || take_token(&self.upgrades_by_address, ip, UPGRADES_PER_SECOND_PER_ADDRESS, UPGRADE_BURST) {
            return true;
        }
        self.note_flood("an upgrade refused", ip);
        false
    }

    /// Who is asking, for `status`: the verified app sees that it may approve.
    pub fn status(&self, app: bool) -> Value {
        self.verify_devices();
        let key_unreadable = *self.key_unreadable.lock();
        let c = self.config.lock();
        let running = self.running.lock();
        let now = Instant::now();
        let code = self.codes.lock().iter().filter(|k| now.duration_since(k.at) < CODE_TTL).map(|k| (CODE_TTL - now.duration_since(k.at)).as_secs()).max();
        let addrs: Vec<String> = running.as_ref().map(|r| r.listeners.addrs().iter().map(listen::host).collect()).unwrap_or_default();
        let scheme = if running.as_ref().is_some_and(|r| r.tls) { "https" } else { "http" };
        json!({
            "enabled": c.enabled,
            "running": running.is_some(),
            "url": addrs.first().map(|a| format!("{scheme}://{a}/")),
            "urls": addrs.iter().map(|a| format!("{scheme}://{a}/")).collect::<Vec<_>>(),
            "addresses": addrs,
            "error": self.unsaved_text.lock().clone().filter(|_| self.save_pending()).or_else(|| self.last_error.lock().clone()),
            "store_error": self.broken.lock().clone(),
            "ip": c.ip,
            "port": c.port.unwrap_or(DEFAULT_PORT),
            "tls": c.cert.is_some() && c.key.is_some(),
            "cert": c.cert,
            "key": c.key,
            "ips": listen::addresses().into_iter().map(|(ip, name)| json!({"ip": ip.to_string(), "name": name})).collect::<Vec<_>>(),
            "devices": c.devices.iter().map(|d| json!({
                "id": d.id, "name": d.name, "permission": d.permission.name(), "kind": d.kind.name(), "can_write": d.permission >= Permission::Write,
                "created_at": d.created_at, "last_seen": d.last_seen, "state": if d.verified { "ok" } else if key_unreadable { "unchecked" } else { "unverified" },
            })).collect::<Vec<_>>(),
            "legacy": c.legacy.iter().map(|d| json!({"id": d.id, "name": d.name, "created_at": d.created_at})).collect::<Vec<_>>(),
            "pairing_expires_in": code,
            "pending": self.pending.lock().iter().map(|(id, p)| json!({"id": id, "name": p.name, "kind": p.kind.name(), "fingerprint": fingerprint(&p.pk), "want": p.want.name(), "address": p.address.to_string()})).collect::<Vec<_>>(),
            "dormant_days": c.dormant_days.unwrap_or(store::DEFAULT_DORMANT_DAYS),
            "app": app,
        })
    }

    /// What the device's record allows now (None while disabled, once revoked, or when its record does not verify).
    fn permission(&self, id: &str) -> Option<Permission> {
        if self.broken.lock().is_some() {
            return None;
        }
        let c = self.config.lock();
        if !c.enabled {
            return None;
        }
        c.devices.iter().find(|d| d.id == id && d.verified).map(|d| d.permission)
    }

    /// Whether phone access is on.
    pub fn is_on(&self) -> bool {
        self.config.lock().enabled
    }

    /// Whether the device's record is gone (revoked, expired, the kill switch, a reset). Phone access being off, or a record
    /// that does not verify, is not that: the pairing still exists.
    fn removed(&self, id: &str) -> bool {
        self.broken.lock().is_none() && !self.config.lock().devices.iter().any(|d| d.id == id)
    }

    /// Whether a record holds this key: one that lost its record to a new pairing of the same key is still paired.
    fn key_paired(&self, pk: &str) -> bool {
        self.config.lock().devices.iter().any(|d| d.pk == pk)
    }

    /// What is known of Noise static key `pk`. Looking changes nothing: a first message can be a replay. [`Self::seen`] follows
    /// once the phone proved it finished the handshake.
    fn lookup_key(&self, pk: &[u8; 32]) -> KeyLookup {
        let key = kelpie_link::b64(pk);
        if self.config.lock().devices.iter().any(|d| !d.verified && same(&d.pk, &key)) {
            self.verify_devices();
        }
        if self.broken.lock().is_some() {
            return KeyLookup::Unconfirmed;
        }
        let c = self.config.lock();
        if !c.enabled {
            return KeyLookup::Unconfirmed;
        }
        match c.devices.iter().find(|d| same(&d.pk, &key)) {
            Some(d) if d.verified => KeyLookup::Paired(Box::new(d.clone())),
            Some(_) => KeyLookup::Unconfirmed,
            None => KeyLookup::Unknown,
        }
    }

    /// The device is connected now. Written to disk at most once an hour for each: dormant devices are judged by this.
    fn seen(&self, id: &str) {
        self.seen_at(id, now_millis());
    }

    fn seen_at(&self, id: &str, now: u64) {
        let write = {
            let mut c = self.config.lock();
            let Some(d) = c.devices.iter_mut().find(|d| d.id == id) else { return };
            let stale = now.saturating_sub(d.last_seen) > SWEEP.as_millis() as u64;
            d.last_seen = now;
            stale
        };
        if write {
            self.keep();
        }
    }

    /// Removes devices unused for longer than the setting allows (they must be paired again). Connected ones are seen.
    fn expire_dormant(&self, now: u64) -> Vec<DeviceEvent> {
        let open: Vec<String> = self.connections.lock().keys().cloned().collect();
        for id in &open {
            self.seen_at(id, now);
        }
        let expired: Vec<(String, String)> = {
            let mut c = self.config.lock();
            let days = u64::from(c.dormant_days.unwrap_or(store::DEFAULT_DORMANT_DAYS));
            if days == 0 {
                return Vec::new();
            }
            let limit = days * 24 * 3600 * 1000;
            let old: Vec<(String, String)> = c.devices.iter().filter(|d| now.saturating_sub(d.last_seen.max(d.created_at)) > limit).map(|d| (d.id.clone(), d.name.clone())).collect();
            c.devices.retain(|d| !old.iter().any(|(id, _)| *id == d.id));
            old
        };
        if expired.is_empty() {
            return Vec::new();
        }
        self.keep();
        expired
            .into_iter()
            .map(|(id, name)| {
                self.stop_grants(&id);
                self.audit.add(&id, "expired", &format!("{name}: unused for too long"));
                let event = DeviceEvent { id, name, change: "removed", permission: None };
                let _ = self.revoked.send(event.clone());
                event
            })
            .collect()
    }

    /// Whether a wrong code just declined a pairing request three times (once per lockout).
    pub fn take_lockout(&self) -> bool {
        self.locked_out.swap(false, Ordering::SeqCst)
    }

    /// One more thing for the audit log.
    pub fn audit(&self, device: &str, action: &str, summary: &str) {
        self.audit.add(device, action, summary);
    }

    /// What the audit log holds (reads a file: off the event loop).
    pub fn audit_entries(&self, limit: usize) -> Value {
        self.audit.read(limit)
    }

    /// What the prompt asks when `days` keeps unused phones longer than now (never counts as longest); None when it does not.
    pub fn dormant_prompt(&self, days: u32) -> Option<crate::user_presence::Prompt> {
        let now = self.config.lock().dormant_days.unwrap_or(store::DEFAULT_DORMANT_DAYS);
        let longer = now != 0 && (days == 0 || days > now);
        longer.then_some(crate::user_presence::Prompt::KeepUnused { days })
    }

    pub fn set_options(&self, days: Option<u32>) -> Value {
        if let Some(days) = days {
            self.config.lock().dormant_days = Some(days);
            self.keep();
            self.audit.add("-", "setting", &format!("devices expire after {days} days unused (0: never)"));
        }
        json!({"dormant_days": self.config.lock().dormant_days.unwrap_or(store::DEFAULT_DORMANT_DAYS)})
    }

    /// Whether `ip` has failed to pair or handshake too often lately.
    fn pairing_blocked(&self, ip: IpAddr) -> bool {
        let mut by_ip = self.failures.lock();
        let recent = by_ip.get_mut(&ip).map(|f| {
            while f.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
                f.pop_front();
            }
            f.len()
        });
        if recent == Some(0) {
            by_ip.remove(&ip);
        }
        recent.is_some_and(|n| n >= PAIR_FAILURES)
    }

    /// Whether a pairing handshake is too soon after the last, for the load the machine is under.
    fn pairing_paced(&self) -> bool {
        let gap = self.load.level().pairing_gap();
        if gap.is_zero() {
            return false;
        }
        let mut last = self.paired_at.lock();
        if last.is_some_and(|at| at.elapsed() < gap) {
            return true;
        }
        *last = Some(Instant::now());
        false
    }

    /// Counts a failed attempt; true when this one locks the address out.
    fn pairing_failed(&self, ip: IpAddr) -> bool {
        let mut by_ip = self.failures.lock();
        if by_ip.len() >= LIMITER_ADDRESSES && !by_ip.contains_key(&ip) {
            // Addresses whose failures all aged out go first; if the table is still full, it starts over.
            by_ip.retain(|_, f| f.back().is_some_and(|t| t.elapsed() <= Duration::from_secs(60)));
            if by_ip.len() >= LIMITER_ADDRESSES {
                by_ip.clear();
            }
        }
        let attempts = by_ip.entry(ip).or_default();
        attempts.push_back(Instant::now());
        attempts.len() == PAIR_FAILURES
    }

    /// Whether `ip` is this machine itself, which is not held to the limits for an address.
    fn this_machine(&self, ip: IpAddr) -> bool {
        #[cfg(test)]
        if self.loopback_is_remote.load(Ordering::SeqCst) {
            return false;
        }
        ip.is_loopback()
    }

    /// A place among the handshakes under way, or None when this address holds its share. When every place is taken the oldest
    /// connection gives way to this one: a place held for long is worth nothing.
    fn handshake_slot(&self, ip: IpAddr) -> Option<HandshakeSlot> {
        let address = (!self.this_machine(ip)).then_some(ip);
        let mut table = self.handshakes.lock();
        if let Some(ip) = address {
            if table.by_address.get(&ip).copied().unwrap_or(0) >= HANDSHAKES_PENDING_PER_ADDRESS {
                self.note_flood("a connection over its address's share", ip);
                return None;
            }
        }
        if table.all.len() >= HANDSHAKES_PENDING {
            // The oldest that has said nothing yet; if every one has, nobody is let go and there is no room.
            let Some(oldest) = table.all.iter().position(|p| !p.answered) else {
                self.note_flood("a connection with no room", ip);
                return None;
            };
            if let Some(gone) = table.all.remove(oldest) {
                table.forget_address(gone.address);
                gone.give_way.notify_one();
            }
            self.note_flood("a connection given way", ip);
        }
        if let Some(ip) = address {
            *table.by_address.entry(ip).or_insert(0) += 1;
        }
        table.next += 1;
        let number = table.next;
        let give_way = Arc::new(Notify::new());
        table.all.push_back(PendingHandshake { number, address, give_way: give_way.clone(), answered: false });
        Some(HandshakeSlot { number, table: self.handshakes.clone(), give_way })
    }

    /// Whether the machine's budget has room for one more handshake's Diffie-Hellman now, and `ip` has not used its share of it.
    fn handshake_allowed(&self, ip: IpAddr) -> bool {
        if !self.this_machine(ip) && !take_token(&self.handshakes_by_address, ip, HANDSHAKES_PER_SECOND_PER_ADDRESS, HANDSHAKES_PER_SECOND_PER_ADDRESS) {
            self.note_flood("a handshake over its share", ip);
            return false;
        }
        let mut b = self.handshake_budget.lock();
        let now = Instant::now();
        b.tokens = (b.tokens + now.duration_since(b.at).as_secs_f64() * HANDSHAKES_PER_SECOND).min(HANDSHAKES_PER_SECOND);
        b.at = now;
        if b.tokens < 1.0 {
            drop(b);
            self.note_flood("a handshake over the machine's budget", ip);
            return false;
        }
        b.tokens -= 1.0;
        true
    }

    /// Whether a paired device may start again now (`RECONNECT_BURST` in `RECONNECT_WINDOW`). Asked only once it proved it finished
    /// the handshake: a replayed first message cannot lock the phone out.
    fn reconnect_allowed(&self, device: &str) -> bool {
        let mut all = self.reconnects.lock();
        if all.len() > 256 {
            all.retain(|_, q| q.back().is_some_and(|t| t.elapsed() < RECONNECT_WINDOW));
        }
        let q = all.entry(device.to_string()).or_default();
        while q.front().is_some_and(|t| t.elapsed() >= RECONNECT_WINDOW) {
            q.pop_front();
        }
        if q.len() >= RECONNECT_BURST {
            return false;
        }
        q.push_back(Instant::now());
        true
    }

    /// The live pairing codes (expired ones are dropped).
    fn live_codes(&self) -> Vec<([u8; 32], Kind)> {
        let mut codes = self.codes.lock();
        codes.retain(|c| c.at.elapsed() < CODE_TTL);
        codes.iter().map(|c| (c.psk, c.kind)).collect()
    }

    /// One more first message made with this code from `ip`. False when that is one too many from there. The code itself is
    /// not touched: first messages can be replayed, and only a finished handshake spends it.
    fn code_attempt(&self, psk: &[u8; 32], ip: IpAddr) -> bool {
        let mut codes = self.codes.lock();
        let Some(code) = codes.iter_mut().find(|c| c.psk == *psk) else { return false };
        if code.attempts.len() >= LIMITER_ADDRESSES && !code.attempts.contains_key(&ip) {
            return false;
        }
        let n = code.attempts.entry(ip).or_insert(0);
        *n = n.saturating_add(1);
        *n <= CODE_ATTEMPTS
    }

    /// Spends a code that was just used: its kind, or None when it was spent or expired meanwhile.
    fn spend_code(&self, psk: &[u8; 32]) -> Option<Kind> {
        let mut codes = self.codes.lock();
        let i = codes.iter().position(|c| c.psk == *psk && c.at.elapsed() < CODE_TTL)?;
        Some(codes.remove(i).kind)
    }

    #[allow(clippy::too_many_arguments)]
    fn add_pending(&self, id: &str, name: &str, kind: Kind, pk: [u8; 32], code: String, want: Permission, address: IpAddr, reply: oneshot::Sender<Option<Device>>) {
        self.pending.lock().insert(id.to_string(), Pending { name: name.to_string(), kind, pk, code, tries: 0, want, address, reply });
    }

    fn remove_pending(&self, id: &str) {
        self.pending.lock().remove(id);
    }

    /// A flag for control the device is given (screen or browser takeover): cleared when it is revoked or downgraded.
    #[allow(dead_code)] // the control paths of the later phases hold these
    pub fn grant(&self, device: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(true));
        self.grants.lock().entry(device.to_string()).or_default().push(flag.clone());
        flag
    }

    /// Stops everything the device controls, at once (before its sockets are closed).
    fn stop_grants(&self, device: &str) {
        for flag in self.grants.lock().remove(device).unwrap_or_default() {
            flag.store(false, Ordering::SeqCst);
        }
    }

    fn stop_all_grants(&self) {
        for (_, flags) in self.grants.lock().drain() {
            for flag in flags {
                flag.store(false, Ordering::SeqCst);
            }
        }
    }

    /// The reply a request with this `cid` got already, if it is a repeat within its window.
    fn repeated(&self, device: &str, cid: &str) -> Option<Value> {
        let mut all = self.replies.lock();
        let seen = all.get_mut(device)?;
        seen.retain(|(_, at, window, _)| at.elapsed() < *window);
        seen.iter().find(|(c, ..)| c == cid).map(|(.., reply)| reply.clone())
    }

    fn remember_reply(&self, device: &str, cid: &str, window: Duration, reply: &Value) {
        let mut all = self.replies.lock();
        if all.len() > 64 && !all.contains_key(device) {
            all.retain(|_, q| q.iter().any(|(_, at, window, _)| at.elapsed() < *window));
        }
        let seen = all.entry(device.to_string()).or_default();
        if seen.len() >= DEDUPE_PER_DEVICE {
            seen.pop_front();
        }
        seen.push_back((cid.to_string(), Instant::now(), window, reply.clone()));
    }
}

#[cfg(test)]
impl Mobile {
    /// A paired device as `approve` makes one (its record sealed with this store's key).
    fn add_test_device(&self, id: &str, name: &str, pk: [u8; 32], permission: Permission, kind: Kind) {
        let key = self.mac_key(true).expect("a readable store").expect("a key");
        let mut d = Device { id: id.into(), name: name.into(), pk: kelpie_link::b64(&pk), permission, kind, created_at: 1, last_seen: now_millis(), mac: String::new(), verified: false };
        store::seal(&mut d, &key);
        self.config.lock().devices.push(d);
    }

    fn set_write(&self, id: &str, write: bool) -> Result<DeviceEvent> {
        self.set_permission(id, if write { Permission::Write } else { Permission::Read })
    }

    /// A person's whole decision on a pairing request, past the OS verification.
    fn approve(&self, params: &Value) -> Result<Approved> {
        let checked = self.check_approval(params)?;
        self.commit_approval(&checked)
    }
}

// ---------- RPCs (from the desktop app) ----------

impl Mobile {
    /// A project's name as the app last sent it.
    pub fn project_name(&self, id: &str) -> Option<String> {
        self.projects.lock().get(id).cloned()
    }

    /// The phone web app's address while phone access is on.
    pub fn running_url(&self) -> Option<String> {
        let running = self.running.lock();
        let r = running.as_ref()?;
        let addr = r.listeners.addrs().into_iter().next()?;
        Some(format!("{}://{}/", if r.tls { "https" } else { "http" }, listen::host(&addr)))
    }

    pub fn set_projects(&self, params: &Value) {
        let map = params.get("projects").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        *self.projects.lock() = map;
    }

    /// A pairing QR code: the explicit browser pairing, whose link opens this daemon's page. Until a native app exists nothing
    /// else can be asked for: a native client's record has no ceiling below `control` and no cap on its transport.
    pub fn pair_start(&self) -> Result<Value> {
        let (tls, addrs, port) = {
            let running = self.running.lock();
            let r = running.as_ref().ok_or_else(|| anyhow!("turn mobile access on first"))?;
            (r.tls, r.listeners.addrs(), r.port)
        };
        let kind = Kind::Pwa;
        let key = self.link_key()?;
        let psk = kelpie_link::random_key().map_err(|e| anyhow!("{e}"))?;
        {
            let mut codes = self.codes.lock();
            codes.retain(|c| c.at.elapsed() < CODE_TTL);
            if codes.len() >= LIVE_CODES {
                codes.remove(0);
            }
            codes.push(Code { psk, kind, at: Instant::now(), attempts: HashMap::new() });
        }
        let lan: Vec<String> = addrs.iter().map(listen::host).collect();
        let payload = kelpie_link::QrPayload { dpk: key.public, psk, endpoints: lan.iter().map(|a| kelpie_link::Endpoint::lan(a)).collect(), name: listen::hostname() };
        // The secret travels in the fragment: browsers never send it in a request or a Referer.
        let preferred = self.config.lock().ip.clone();
        let host = preferred.and_then(|ip| ip.parse::<IpAddr>().ok()).map(|ip| listen::host(&SocketAddr::new(ip, port))).or_else(|| lan.first().cloned()).ok_or_else(|| anyhow!("no address to pair on"))?;
        let link = format!("{}://{host}/#p={}", if tls { "https" } else { "http" }, payload.encode());
        let svg = qrcode::QrCode::new(link.as_bytes())?
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .dark_color(qrcode::render::svg::Color("#141823"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build();
        Ok(json!({"url": link, "qr": svg, "expires_in": CODE_TTL.as_secs(), "kind": kind.name(), "tls": tls}))
    }

    /// The person says no to a pairing request (no verification: it adds nothing).
    pub fn decline(&self, params: &Value) -> Result<()> {
        let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
        let p = self.pending.lock().remove(id).ok_or_else(|| anyhow!("no such request (it may have timed out)"))?;
        self.audit.add("-", "pair_declined", &format!("{}, key {}", p.name, fingerprint(&p.pk)));
        let _ = p.reply.send(None);
        Ok(())
    }

    /// Everything about allowing a pairing request that needs no prompt (the verified app only): the request exists, `code` is
    /// the six digits the phone shows (three wrong ones decline it), and the permission (`read` unless said) is within what
    /// its kind may have. The request keeps waiting: [`Self::commit_approval`] takes it once the person verified.
    pub fn check_approval(&self, params: &Value) -> Result<CheckedApproval> {
        let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
        let typed: String = params.get("code").and_then(Value::as_str).unwrap_or_default().chars().filter(char::is_ascii_digit).collect();
        let wanted = params.get("permission").and_then(Value::as_str).map(|p| Permission::parse(p).ok_or_else(|| anyhow!("unknown permission {p:?}"))).transpose()?.unwrap_or(Permission::Read);
        let mut pending = self.pending.lock();
        let p = pending.get_mut(id).ok_or_else(|| anyhow!("no such request (it may have timed out)"))?;
        if !same(&typed, &p.code) {
            p.tries += 1;
            if p.tries >= CODE_TRIES {
                let p = pending.remove(id).expect("present");
                self.audit.add("-", "lockout", &format!("{}, key {}: wrong code three times", p.name, fingerprint(&p.pk)));
                self.locked_out.store(true, Ordering::SeqCst);
                let _ = p.reply.send(None);
                bail!("wrong code three times: the request was declined; pair the phone again");
            }
            bail!("that is not the code the phone shows");
        }
        if wanted > p.kind.ceiling() {
            let p = pending.remove(id).expect("present");
            let _ = p.reply.send(None);
            bail!("a page in a browser cannot be given {}", wanted.name());
        }
        Ok(CheckedApproval { id: id.to_string(), name: p.name.clone(), fingerprint: fingerprint(&p.pk), permission: wanted })
    }

    /// Adds the device of a checked request, after the person verified. The request must still be there: it may have timed out,
    /// been declined or the phone may have gone during the prompt, and then nothing is added.
    pub fn commit_approval(&self, checked: &CheckedApproval) -> Result<Approved> {
        let wanted = checked.permission;
        let p = self.pending.lock().remove(&checked.id).ok_or_else(|| anyhow!("no such request (it timed out while you were verifying)"))?;
        let key = match self.mac_key(true) {
            Ok(Some(key)) => key,
            failed => {
                let _ = p.reply.send(None);
                let why = failed.err().map_or_else(|| "no key".to_string(), |e| format!("{e:#}"));
                bail!("the credential store did not give the device key ({why}): phone access cannot pair devices on this computer");
            }
        };
        let mut device = Device { id: uuid::Uuid::new_v4().to_string(), name: p.name.clone(), pk: kelpie_link::b64(&p.pk), permission: wanted, kind: p.kind, created_at: now_millis(), last_seen: now_millis(), mac: String::new(), verified: false };
        store::seal(&mut device, &key);
        let replaced: Vec<(String, String)> = {
            let mut c = self.config.lock();
            // One record per key: pairing a phone again replaces its old record (whose connection then ends).
            let old = c.devices.iter().filter(|d| d.pk == device.pk).map(|d| (d.id.clone(), d.name.clone())).collect();
            c.devices.retain(|d| d.pk != device.pk);
            c.devices.push(device.clone());
            old
        };
        self.keep();
        let replaced_ids = replaced.iter().map(|(id, _)| id.clone()).collect();
        for (id, name) in replaced {
            self.stop_grants(&id);
            let _ = self.revoked.send(DeviceEvent { id, name, change: "replaced", permission: None });
        }
        self.audit.add(&device.id, "pair", &format!("{}, key {}, {}, {}", device.name, fingerprint(&p.pk), device.kind.name(), device.permission.name()));
        let event = DeviceEvent { id: device.id.clone(), name: device.name.clone(), change: "added", permission: Some(device.permission) };
        let _ = self.revoked.send(event.clone());
        let _ = p.reply.send(Some(device.clone()));
        Ok(Approved { reply: json!({"id": device.id, "permission": device.permission.name()}), event, replaced: replaced_ids })
    }

    /// What the OS prompt asks when `permission` raises what the device may do now; None when it does not (lowering, or no change,
    /// needs nobody). What cannot be applied at all is refused here, with no prompt.
    pub fn raise_prompt(&self, id: &str, permission: Permission) -> Result<Option<crate::user_presence::Prompt>> {
        let c = self.config.lock();
        let d = c.devices.iter().find(|d| d.id == id).ok_or_else(|| anyhow!("no such device"))?;
        anyhow::ensure!(d.verified, "this device must be paired again");
        anyhow::ensure!(permission <= d.kind.ceiling(), "a page in a browser cannot be given {}", permission.name());
        Ok((permission > d.permission).then(|| crate::user_presence::Prompt::Raise { name: d.name.clone(), permission }))
    }

    /// Removes a device (or the name of one paired before kelpie-link). Whatever it controls stops first.
    pub fn revoke(&self, id: &str) -> Result<DeviceEvent> {
        self.stop_grants(id);
        let removed = {
            let mut c = self.config.lock();
            let device = c.devices.iter().position(|d| d.id == id).map(|i| c.devices.remove(i).name);
            device.or_else(|| c.legacy.iter().position(|d| d.id == id).map(|i| c.legacy.remove(i).name))
        };
        let name = removed.ok_or_else(|| anyhow!("no such device"))?;
        self.keep();
        self.audit.add(id, "revoke", &name);
        let event = DeviceEvent { id: id.to_string(), name, change: "removed", permission: None };
        let _ = self.revoked.send(event.clone());
        Ok(event)
    }

    /// Changes what a device may do (a raise needs the verified app; the dispatcher checks). A lower permission stops its
    /// control at once and makes its connection start again.
    pub fn set_permission(&self, id: &str, permission: Permission) -> Result<DeviceEvent> {
        let key = self.mac_key(true)?.ok_or_else(|| anyhow!("the credential store did not keep the device key"))?;
        let lower = {
            let c = self.config.lock();
            let d = c.devices.iter().find(|d| d.id == id).ok_or_else(|| anyhow!("no such device"))?;
            anyhow::ensure!(d.verified, "this device must be paired again");
            anyhow::ensure!(permission <= d.kind.ceiling(), "a page in a browser cannot be given {}", permission.name());
            permission < d.permission
        };
        // What it controls stops before anything else changes.
        if lower {
            self.stop_grants(id);
        }
        let event = {
            let mut c = self.config.lock();
            let d = c.devices.iter_mut().find(|d| d.id == id).ok_or_else(|| anyhow!("no such device"))?;
            d.permission = permission;
            store::seal(d, &key);
            DeviceEvent { id: id.to_string(), name: d.name.clone(), change: "changed", permission: Some(permission) }
        };
        self.keep();
        self.audit.add(id, "permission", &format!("{} now {}", event.name, permission.name()));
        // Reconnecting picks the new permission up (the device stays paired).
        let _ = self.revoked.send(event.clone());
        Ok(event)
    }

    pub fn disable(&self) {
        self.stop_all_grants();
        self.config.lock().enabled = false;
        if let Some(r) = self.running.lock().take() {
            r.listeners.stop_all();
            r.stop.cancel();
        }
        self.codes.lock().clear();
        self.keep();
    }

    /// The kill switch: phone access off (listeners stopped, sockets closed, control stopped) and every device removed, whether
    /// or not it is online. Keeps the keys and needs no verification: it only ever takes access away.
    pub fn revoke_all(&self) -> Vec<DeviceEvent> {
        // Removed before the sockets close, so each phone is told it was removed (`client`).
        let devices: Vec<(String, String)> = {
            let mut c = self.config.lock();
            let all = c.devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
            c.devices.clear();
            all
        };
        self.disable();
        for (_, p) in self.pending.lock().drain() {
            let _ = p.reply.send(None);
        }
        self.keep();
        self.audit.add("-", "revoke_all", &format!("phone access off, {} devices removed", devices.len()));
        let events: Vec<DeviceEvent> = devices.into_iter().map(|(id, name)| DeviceEvent { id, name, change: "removed", permission: None }).collect();
        for e in &events {
            let _ = self.revoked.send(e.clone());
        }
        events
    }

    /// Forgets every device and pairing, makes new keys and turns phone access off. Also the way out of a store file
    /// that could not be read (it was copied aside when it was found).
    pub fn reset(&self) -> Vec<DeviceEvent> {
        // Removed before the sockets close, so each phone is told it was removed (`client`).
        let devices: Vec<(String, String)> = {
            let mut c = self.config.lock();
            let all = c.devices.iter().map(|d| (d.id.clone(), d.name.clone())).collect();
            c.devices.clear();
            all
        };
        self.disable();
        for (_, p) in self.pending.lock().drain() {
            let _ = p.reply.send(None);
        }
        for key in [K_LINK, K_MAC] {
            if let Err(e) = self.secrets.delete(key) {
                tracing::warn!("phone access reset: {e:#}");
            }
        }
        *self.link_key.lock() = None;
        *self.mac_key.lock() = None;
        {
            let mut c = self.config.lock();
            c.legacy.clear();
            c.version = store::VERSION;
        }
        *self.broken.lock() = None;
        self.keep();
        self.audit.add("-", "reset", "phone access reset: devices forgotten, new keys");
        let events: Vec<DeviceEvent> = devices.into_iter().map(|(id, name)| DeviceEvent { id, name, change: "removed", permission: None }).collect();
        for e in &events {
            let _ = self.revoked.send(e.clone());
        }
        events
    }
}

/// Starts (or restarts) the server with `params` (`ip`, `port`, `cert`, `key`) merged into the
/// saved settings. Returns the status.
pub async fn enable(daemon: Arc<Daemon>, params: Value) -> Result<Value> {
    let m = &daemon.mobile;
    if let Some(why) = m.broken.lock().clone() {
        bail!("{why}. Reset phone access in Settings → Mobile to start again (a copy of the file was kept).");
    }
    let turned_on;
    {
        let mut c = m.config.lock();
        if let Some(ip) = params.get("ip").and_then(Value::as_str) {
            c.ip = Some(ip.to_string()).filter(|s| !s.is_empty());
        }
        if let Some(port) = params.get("port").and_then(Value::as_u64) {
            c.port = Some(u16::try_from(port).ok().filter(|p| *p >= 1024).ok_or_else(|| anyhow!("port must be 1024–65535"))?);
        }
        turned_on = !c.enabled;
        let path = |k: &str| params.get(k).map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
        if let Some(v) = path("cert") {
            c.cert = v;
        }
        if let Some(v) = path("key") {
            c.key = v;
        }
        if let Some(r) = m.running.lock().take() {
            r.listeners.stop_all();
            r.stop.cancel();
        }
        c.enabled = true;
    }
    m.keep();
    if turned_on {
        m.audit("-", "phone_access_on", "turned on");
    }
    match start(daemon.clone()).await {
        Ok(_) => *m.last_error.lock() = None,
        Err(e) => {
            *m.last_error.lock() = Some(format!("{e:#}"));
            return Err(e);
        }
    }
    Ok(m.status(false))
}

/// At daemon start: serve again if it was on.
pub async fn autostart(daemon: Arc<Daemon>) {
    if daemon.mobile.config.lock().enabled && daemon.mobile.broken.lock().is_none() {
        if let Err(e) = start(daemon.clone()).await {
            tracing::warn!("mobile access: {e:#}");
            *daemon.mobile.last_error.lock() = Some(format!("{e:#}"));
        }
    }
}

async fn start(daemon: Arc<Daemon>) -> Result<()> {
    let (ip, port, cert, key) = {
        let c = daemon.mobile.config.lock();
        (c.ip.clone(), c.port.unwrap_or(DEFAULT_PORT), c.cert.clone(), c.key.clone())
    };
    let ips = listen::wanted(ip.as_deref())?;
    // The key is made before anything listens: a credential store that will not keep it is reported now.
    daemon.mobile.link_key()?;
    let tls = match (cert, key) {
        (Some(c), Some(k)) => Some(tls_acceptor(&c, &k)?),
        _ => None,
    };
    let listeners = Arc::new(listen::Listeners::default());
    let (fresh, _, error) = listeners.rebind(&ips, port).await;
    if fresh.is_empty() {
        return Err(error.unwrap_or_else(|| anyhow!("nothing to listen on")));
    }
    let stop = Arc::new(Generation::new());
    let app = Router::new().route("/link", get(link_socket)).fallback(page).with_state(daemon.clone()).layer(axum::Extension(stop.clone()));
    {
        let c = daemon.mobile.config.lock();
        if !c.enabled {
            listeners.stop_all();
            bail!("mobile access was disabled while starting");
        }
        if let Some(old) = daemon.mobile.running.lock().replace(Running { tls: tls.is_some(), port, listeners: listeners.clone(), stop: stop.clone() }) {
            old.listeners.stop_all();
            old.stop.cancel();
        }
    }
    for (listener, own) in fresh {
        tokio::spawn(serve_listener(listener, app.clone(), tls.clone(), stop.clone(), own));
    }
    advertise(&daemon.mobile, &listeners);
    // Measuring the machine's load runs exactly as long as this server does.
    let told = Arc::downgrade(&daemon);
    daemon.mobile.load.start(
        stop.watch(),
        Arc::new(move |seconds| {
            if let Some(daemon) = told.upgrade() {
                daemon.broadcast("daemon.unresponsive", json!({"seconds": seconds}));
            }
        }),
    );
    // Devices unused for too long must be paired again; looked for now and every hour while phone access is on.
    let (sweeper, sweep_stop) = (Arc::downgrade(&daemon), stop.clone());
    tokio::spawn(async move {
        loop {
            let Some(d) = sweeper.upgrade() else { return };
            for event in d.mobile.expire_dormant(now_millis()) {
                d.drop_device_queues(&event.id, "revoked");
                d.broadcast("mobile.device_changed", event.json());
                d.broadcast("mobile.updated", json!({}));
            }
            drop(d);
            tokio::select! {
                _ = sweep_stop.cancelled() => return,
                _ = tokio::time::sleep(SWEEP) => {}
            }
        }
    });
    // Without a chosen address the listeners follow the machine's addresses.
    if ip.is_none() {
        let (daemon, listeners, stop) = (Arc::downgrade(&daemon), listeners.clone(), stop.clone());
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = stop.cancelled() => return,
                    _ = listen::changed() => {}
                }
                listen::settle().await;
                if stop.is_cancelled() {
                    return;
                }
                let Some(d) = daemon.upgrade() else { return };
                let ips = listen::wanted(None).unwrap_or_default();
                let (fresh, changed, _) = listeners.rebind(&ips, port).await;
                for (listener, own) in fresh {
                    tokio::spawn(serve_listener(listener, app.clone(), tls.clone(), stop.clone(), own));
                }
                if changed {
                    tracing::info!("phone access now on {:?}", listeners.addrs());
                    advertise(&d.mobile, &listeners);
                    d.broadcast("mobile.updated", json!({}));
                }
            }
        });
    }
    tracing::info!("mobile access on {:?}", listeners.addrs());
    Ok(())
}

/// Tells every connected phone (and the next to connect) where this computer can be reached.
fn advertise(m: &Mobile, listeners: &listen::Listeners) {
    let list: Vec<String> = listeners.addrs().iter().map(listen::host).collect();
    m.addrs.send_modify(|a| {
        if a.list != list {
            a.seq += 1;
            a.list = list;
        }
    });
}

/// Accepts connections on one address until that address or the whole server stops.
async fn serve_listener(listener: tokio::net::TcpListener, app: Router, tls: Option<tokio_rustls::TlsAcceptor>, all: Arc<Generation>, own: Arc<Generation>) {
    loop {
        let accepted = tokio::select! {
            biased;
            _ = all.cancelled() => break,
            _ = own.cancelled() => break,
            a = listener.accept() => a,
        };
        let Ok((tcp, addr)) = accepted else { continue };
        keep_alive(&tcp);
        // A keystroke's frame goes out at once, not after Nagle's wait for more.
        if let Err(e) = tcp.set_nodelay(true) {
            tracing::debug!("nodelay: {e}");
        }
        // Who is calling, for the pairing limiter and the transport's cap.
        let app = app.clone().layer(axum::Extension(Peer { addr, tls: tls.is_some() }));
        let tls = tls.clone();
        let connection_stop = all.clone();
        tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = connection_stop.cancelled() => {}
                _ = async move {
                    let svc = hyper_util::service::TowerToHyperService::new(app);
                    let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
                    let _ = match tls {
                        Some(acceptor) => match acceptor.accept(tcp).await {
                            Ok(s) => builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(s), svc).await,
                            Err(_) => return,
                        },
                        None => builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(tcp), svc).await,
                    };
                } => {}
            }
        });
    }
}

/// Asks the OS to probe an idle connection (after 30 s, every 10 s, three times): a backstop for
/// the heartbeat, which notices a path that died first.
fn keep_alive(tcp: &tokio::net::TcpStream) {
    let probes = socket2::TcpKeepalive::new().with_time(Duration::from_secs(30)).with_interval(Duration::from_secs(10)).with_retries(3);
    if let Err(e) = socket2::SockRef::from(tcp).set_tcp_keepalive(&probes) {
        tracing::debug!("keep-alive: {e}");
    }
}

fn tls_acceptor(cert: &str, key: &str) -> Result<tokio_rustls::TlsAcceptor> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert).context("certificate file")?.collect::<Result<_, _>>().context("certificate file")?;
    let key = PrivateKeyDer::from_pem_file(key).context("key file")?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

/// The most a connection may do whatever its device's record says: a page in a browser (a device paired as one, or any
/// connection whose upgrade carried a browser's `Origin`) may type only over HTTPS or from a Tailscale address (or from this
/// machine itself, which no network sits between); over plain HTTP elsewhere the page could have been swapped on the way, so it
/// may only read. The native app is not capped.
fn transport_cap(kind: Kind, browser: bool, peer: &Peer) -> Permission {
    if kind == Kind::App && !browser {
        return Permission::Control;
    }
    if peer.tls || peer.ip().is_loopback() || listen::tailscale(peer.ip()) {
        Permission::Write
    } else {
        Permission::Read
    }
}

// ---------- HTTP ----------

// The phone client (app/phone, built by `pnpm build:phone`), embedded by build.rs as `PHONE_ASSETS`.
include!(concat!(env!("OUT_DIR"), "/phone_assets.rs"));

/// The embedded file for a request path: `/` is the page; anything else must be a file of the bundle.
fn phone_asset(path: &str) -> Option<(&'static str, &'static [u8])> {
    let path = if path == "/" { "/index.html" } else { path };
    PHONE_ASSETS.iter().find(|(p, _, _)| *p == path).map(|(_, ty, body)| (*ty, *body))
}

fn secure_headers(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    r
}

async fn page(method: axum::http::Method, uri: axum::http::Uri) -> Response {
    if method != axum::http::Method::GET && method != axum::http::Method::HEAD {
        return secure_headers(StatusCode::METHOD_NOT_ALLOWED.into_response());
    }
    let Some((ty, body)) = phone_asset(uri.path()) else {
        return secure_headers(StatusCode::NOT_FOUND.into_response());
    };
    let mut r = secure_headers(([(header::CONTENT_TYPE, ty)], body).into_response());
    // Files under /assets/ carry a hash of their content in their name: the browser may keep them. The page,
    // the service worker and the manifest are asked for again every time.
    if uri.path().starts_with("/assets/") {
        r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=31536000, immutable"));
    }
    r
}

/// WebSockets only from the page itself (no cross-site socket hijacking).
fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        Some(origin) => origin.split_once("://").is_some_and(|(_, rest)| rest == host),
        None => true, // non-browser clients send no Origin
    }
}

/// `/link`: kelpie-link over a WebSocket (see `link`). A connection whose upgrade carries an `Origin` is a page in a browser.
async fn link_socket(State(daemon): State<Arc<Daemon>>, axum::Extension(generation): axum::Extension<Arc<Generation>>, axum::Extension(peer): axum::Extension<Peer>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if generation.is_cancelled() || !daemon.mobile.config.lock().enabled {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !daemon.mobile.upgrade_allowed(peer.ip()) {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    }
    let browser = headers.contains_key(header::ORIGIN);
    // One Noise message per WebSocket message.
    ws.max_message_size(kelpie_link::MAX_NOISE_MESSAGE + 16).max_frame_size(kelpie_link::MAX_NOISE_MESSAGE + 16).on_upgrade(move |s| link::accept(daemon, s, generation, peer, browser))
}

fn sessions(daemon: &Daemon) -> Value {
    json!(daemon
        .list()
        .into_iter()
        .map(|s| json!({
            "id": s.spec.id, "name": s.spec.name, "kind": s.spec.kind, "title": s.title,
            "status": s.status, "unread": s.unread, "alive": s.alive, "chat": s.chat,
            "project_id": s.spec.project_id, "cwd": s.spec.cwd, "last_activity": s.last_activity,
            "waiting_since": s.waiting_since,
        }))
        .collect::<Vec<_>>())
}

/// Close codes: 4401 the device is not paired (in a session it follows the sealed `{type: "revoked"}`, which is what makes a
/// phone forget its pairing: a close frame proves nothing, anyone on the path can send one); 4408 the connection must
/// be made again, the device stays paired (its permission changed, it fell too far behind, or phone access was turned off);
/// 4409 another connection of this device took over (the page waits until it is looked at again
/// instead of fighting for the slot); 4410 the phone stopped answering heartbeats; 4411 it cannot
/// take what is sent (`slow_consumer`). The last two are for reconnecting. Before a session exists:
/// 4401 this computer does not know the key (the phone keeps it until the person forgets it), 4402 it knows the key and cannot
/// confirm the record now, 4403 the pairing code is not valid, 4429 too many attempts, 4503 the computer is busy (`link`).
const NOT_PAIRED: u16 = 4401;
const UNCONFIRMED: u16 = 4402;
const RECONNECT: u16 = 4408;
const REPLACED: u16 = 4409;
const SILENT: u16 = 4410;
const SLOW_CONSUMER: u16 = 4411;

/// How long a close frame may take to leave: a peer that stopped reading must not keep its
/// connection (and what hangs on it) alive.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether `fut` finished within `limit`.
#[cfg(test)]
async fn within<F: std::future::Future>(limit: Duration, fut: F) -> bool {
    tokio::time::timeout(limit, fut).await.is_ok()
}

/// What a message on the device-event channel means for the connection of `device`.
#[derive(Debug, PartialEq)]
enum OnEvent {
    /// Close with this code: the device was removed (unpaired), or changed (reconnect and find out).
    Close(u16, &'static str),
    /// Another device was added, changed or removed: the phone is told.
    Tell(Value),
    /// The receiver fell behind and may have missed something about this device: look at its record again.
    Recheck,
}

fn on_event(event: Result<DeviceEvent, broadcast::error::RecvError>, device: &str) -> OnEvent {
    match event {
        Ok(e) if e.id == device => {
            if e.change == "removed" {
                OnEvent::Close(NOT_PAIRED, "revoked")
            } else if e.change == "replaced" {
                // The same key was paired again. Whatever still uses the old record (another tab of the browser) holds that key
                // and the pairing just saved, so it must not be told to forget them.
                OnEvent::Close(REPLACED, "replaced")
            } else {
                OnEvent::Close(RECONNECT, "changed")
            }
        }
        Ok(e) => {
            let mut told = e.json();
            told["type"] = json!("device_changed");
            OnEvent::Tell(told)
        }
        Err(broadcast::error::RecvError::Lagged(_)) => OnEvent::Recheck,
        Err(broadcast::error::RecvError::Closed) => OnEvent::Close(RECONNECT, "closed"),
    }
}

/// A request that was not carried out: `code` for programs, `message` for people; `reason` says
/// why an answer is stale. An answer's result is `stale` (somebody else answered it, or it
/// moved on: `answered_by` and `at` say who and when), `dup` (this device answered it already)
/// or `refused`.
struct Refusal {
    code: &'static str,
    reason: Option<&'static str>,
    message: String,
    result: &'static str,
    answered: Option<Box<Resolution>>,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, reason: None, message: message.into(), result: "refused", answered: None }
}

/// What a device is told about an answer that was not written.
fn already_answered(device: &str, answer: Resolution, message: &str) -> Refusal {
    let mine = answer.by.id == device;
    let result = if mine { "dup" } else { "stale" };
    Refusal { code: result, reason: Some("answered"), message: message.into(), result, answered: Some(Box::new(answer)) }
}

fn answer_refusal(device: &str, e: AnswerError) -> Refusal {
    match e {
        AnswerError::Prompt => refuse("prompt", "A prompt is waiting on this session: answer it, or send the text anyway."),
        AnswerError::Stale(reason) => Refusal { code: "stale", reason: Some(reason), message: "That prompt was answered already, or has changed.".into(), result: "stale", answered: None },
        AnswerError::Answered(answer) => already_answered(device, answer, "That prompt was answered already."),
        AnswerError::UnknownKey => refuse("bad_request", "unknown key"),
    }
}

fn chat_answer_refusal(device: &str, e: crate::chat::NotAnswered) -> Refusal {
    match e {
        crate::chat::NotAnswered::NotAnAnswer => refuse("bad_answer", "That is not the answer to that request."),
        crate::chat::NotAnswered::Lasting => refuse("one_shot", ONE_SHOT),
        crate::chat::NotAnswered::AlreadyAnswered(answer) => already_answered(device, answer, "That request was answered already."),
    }
}

/// What a chat answer decided, for the audit log: the decision word, not the tool's input.
fn chat_decision(line: &str) -> String {
    let v: Value = serde_json::from_str(line).unwrap_or_default();
    let word = v.pointer("/response/response/behavior").or_else(|| v.pointer("/result/decision")).or_else(|| v.pointer("/result/outcome/optionId")).or_else(|| v.pointer("/result/action"));
    word.and_then(Value::as_str).unwrap_or("answer").chars().take(40).collect()
}

/// A rule that outlasts this one answer is the computer's to make: a phone's approvals are one-shot.
const ONE_SHOT: &str = "A choice that lasts (\"always\", \"don't ask again\", \"for this session\") can only be made on the computer. Answer this one time here.";

/// A terminal session a phone follows.
struct Followed {
    task: tokio::task::JoinHandle<()>,
    /// The one the phone is looking at; the others are paced harder when the machine is busy.
    focused: Arc<AtomicBool>,
}

/// One phone's connection.
struct Conn {
    daemon: Arc<Daemon>,
    device: String,
    /// The device's name, for the other side of an answer to say who answered.
    name: String,
    /// This connection as a client of the daemon (it hears the daemon's events, and attaches to chats).
    id: u64,
    tx: ClientTx,
    /// Where screen frames and chat messages (JSON text) wait for the socket, and where what must not
    /// wait behind them does.
    out: mpsc::Sender<String>,
    /// Terminal sessions followed, by session id.
    followed: HashMap<String, Followed>,
    /// Chat sessions watched.
    watched: HashSet<String>,
    /// Chats a phone asked for and was told `busy`: it is told when to ask again (`chat_resync`).
    owed: HashSet<String>,
    /// When each chat was last asked for (`chat_sub`).
    chat_at: HashMap<String, Instant>,
    /// Chat output sent to this connection and not yet taken by its socket.
    waiting: Arc<AtomicUsize>,
    /// The daemon had more to say than the queue holds: the next look at the list is a full one.
    overflowed: Arc<AtomicBool>,
    /// The most this connection may do whatever the device's record says (`transport_cap`).
    cap: Permission,
    list: Option<(Value, Value)>,
    rev: u64,
    /// The phone does not acknowledge frames or answer heartbeats (see `acknowledges`).
    legacy: bool,
    _phone: Option<crate::load::PhoneGuard>,
}

impl Conn {
    /// The session list and project names as they are now, with the next `rev`, when they differ
    /// from what this phone has (or `force`).
    fn publish(&mut self, force: bool) -> Option<Value> {
        let now = (sessions(&self.daemon), json!(self.daemon.mobile.projects.lock().clone()));
        if !force && self.list.as_ref() == Some(&now) {
            return None;
        }
        self.rev += 1;
        let msg = json!({"type": "sessions", "rev": self.rev, "sessions": now.0, "projects": now.1});
        self.list = Some(now);
        Some(msg)
    }

    async fn handle(&mut self, granted: Permission, req: &Value) -> Option<Value> {
        let name = req["type"].as_str()?;
        let tag = req.get("req").filter(|r| r.is_string() || r.is_number()).cloned();
        // Typing and answers may carry a `cid`: a repeat of one (a retry after a lost reply, over this or another connection
        // of the device) gets the first reply again, marked `dup`, and is not carried out twice. The daemon run's memory
        // only: a phone that sees `daemon_epoch` change starts over anyway.
        let window = match Verb::parse(name) {
            Some(v @ (Verb::Input | Verb::Key)) if v.needs() <= granted => Some(INPUT_DEDUPE),
            Some(v @ (Verb::Answer | Verb::ChatAnswer)) if v.needs() <= granted => Some(ANSWER_DEDUPE),
            _ => None,
        };
        let cid = window.and(req.get("cid").and_then(Value::as_str).filter(|c| !c.is_empty() && c.len() <= 64));
        if let Some(cid) = cid {
            if let Some(mut reply) = self.daemon.mobile.repeated(&self.device, cid) {
                reply["dup"] = json!(true);
                if let Some(t) = tag {
                    reply["req"] = t;
                }
                return Some(reply);
            }
        }
        let reply = self.reply(granted, name, req, tag).await;
        self.note(name, req, reply.as_ref());
        if let (Some(cid), Some(window)) = (cid, window) {
            let mut kept = reply.clone().unwrap_or_else(|| json!({"type": "ack"}));
            if let Some(o) = kept.as_object_mut() {
                o.remove("req");
            }
            self.daemon.mobile.remember_reply(&self.device, cid, window, &kept);
        }
        reply
    }

    /// The audit log's line for what this phone just did (only what was carried out; never the text itself).
    fn note(&self, name: &str, req: &Value, reply: Option<&Value>) {
        if reply.is_some_and(|r| r["type"] == "error" || r["result"] == "dup") || !matches!(Verb::parse(name), Some(v) if v.needs() >= Permission::Write) {
            return;
        }
        let id = req["id"].as_str().unwrap_or_default();
        let text = |k: &str| req[k].as_str().map(audit::text_summary).unwrap_or_default();
        let summary = match name {
            "key" => format!("{id}: key {}", req["key"].as_str().unwrap_or_default()),
            "answer" => format!("{id}: {}", req["key"].as_str().map_or_else(|| text("text"), |k| format!("key {k}"))),
            "chat_answer" => format!("{id}: request {}, {}", req["request_id"], chat_decision(req["line"].as_str().unwrap_or_default())),
            "chat_cancel" => format!("{id}: {}", req["qid"].as_str().unwrap_or_default()),
            _ => format!("{id}: {}", text("text")),
        };
        self.daemon.mobile.audit(&self.device, name, &summary);
    }

    async fn reply(&mut self, granted: Permission, name: &str, req: &Value, tag: Option<Value>) -> Option<Value> {
        match self.run(granted, name, req).await {
            Ok(Some(mut reply)) => {
                if let Some(t) = tag {
                    reply["req"] = t;
                }
                Some(reply)
            }
            Ok(None) => tag.map(|t| json!({"type": "ack", "req": t})),
            Err(r) => {
                let mut reply = json!({"type": "error", "req": tag, "code": r.code, "reason": r.reason, "message": r.message});
                if matches!(name, "answer" | "chat_answer") {
                    reply["result"] = json!(r.result);
                    if let Some(answer) = r.answered {
                        reply["answered_by"] = json!(answer.by.name);
                        reply["at"] = json!(answer.at);
                    }
                }
                Some(reply)
            }
        }
    }

    /// What the phone is told now and then: that it should ask again for a chat it was refused, and
    /// the whole picture after the daemon had more to say than the queue held.
    fn housekeeping(&mut self) -> Vec<Value> {
        let mut told = Vec::new();
        if self.overflowed.swap(false, Ordering::Relaxed) {
            told.extend(self.publish(true));
            told.extend(self.watched.iter().map(|id| json!({"type": "chat_resync", "id": id})));
            self.owed.clear();
        }
        // The log that made it busy has been taken, and no other phone's start-over is in the way.
        if !self.owed.is_empty() && self.chat_room().is_ok() {
            told.extend(self.owed.drain().map(|id| json!({"type": "chat_resync", "id": id})));
        }
        told
    }

    /// Whether another chat log may start towards this phone: it has taken the last, and not too
    /// many are on their way to phones at the machine's load.
    fn chat_room(&self) -> Result<(), Refusal> {
        if self.waiting.load(Ordering::Relaxed) >= crate::chat::RESET_WAITING_MAX {
            return Err(refuse("busy", "The last chat log is still being sent."));
        }
        let limit = self.daemon.mobile.load.level().chat_resets();
        if limit != usize::MAX {
            let others = self.daemon.mobile.waiting.lock().iter().filter(|(id, waiting)| **id != self.id && waiting.load(Ordering::Relaxed) > 0).count();
            if others >= limit {
                return Err(refuse("busy", "Kelpie is busy sending chat logs to other phones."));
            }
        }
        Ok(())
    }

    fn entry(&self, req: &Value) -> Result<(String, Entry), Refusal> {
        let id = req["id"].as_str().filter(|i| !i.is_empty() && i.len() <= 128).ok_or_else(|| refuse("bad_request", "missing session id"))?;
        self.daemon.by_id(id).map(|e| (id.to_string(), e)).ok_or_else(|| refuse("no_session", "That session is gone."))
    }

    fn terminal(&self, req: &Value) -> Result<(String, Arc<crate::session::Session>), Refusal> {
        match self.entry(req)? {
            (id, Entry::Term(s)) => Ok((id, s)),
            _ => Err(refuse("chat", "This is a chat session: watch it with chat_sub.")),
        }
    }

    fn chat(&self, req: &Value) -> Result<(String, Arc<crate::chat::ChatSession>), Refusal> {
        match self.entry(req)? {
            (id, Entry::Chat(c)) => Ok((id, c)),
            _ => Err(refuse("not_chat", "This is a terminal session.")),
        }
    }

    /// Room for one more followed or watched session (finished ones do not count).
    fn make_room(&mut self, id: &str) -> Result<(), Refusal> {
        self.followed.retain(|_, f| !f.task.is_finished());
        // A chat that was closed while watched is no longer open.
        let daemon = self.daemon.clone();
        self.watched.retain(|w| daemon.by_id(w).is_some());
        self.chat_at.retain(|c, _| daemon.by_id(c).is_some());
        self.owed.retain(|c| daemon.by_id(c).is_some());
        if !self.followed.contains_key(id) && !self.watched.contains(id) && self.followed.len() + self.watched.len() >= MAX_FOLLOWED {
            return Err(refuse("too_many", "Too many sessions open at once."));
        }
        Ok(())
    }

    /// Carries out one request of the verb table; `Some` is a reply with content, `None` a plain ack.
    async fn run(&mut self, granted: Permission, name: &str, req: &Value) -> Result<Option<Value>, Refusal> {
        let verb = Verb::parse(name).ok_or_else(|| refuse("unknown_verb", format!("unknown request {name:?}")))?;
        if verb.needs() > granted {
            return Err(refuse("read_only", "This device may read but not type. Allow it in Kelpie → Settings → Mobile."));
        }
        // A phone that names the daemon run it talked to: after a restart its ids and positions mean nothing.
        if verb.needs() >= Permission::Write && req.get("epoch").is_some_and(|e| e.as_str() != Some(&self.daemon.mobile.epoch)) {
            return Err(refuse("epoch", "Kelpie restarted since this was sent."));
        }
        let enter = req["enter"].as_bool() == Some(true);
        match verb {
            Verb::List => return Ok(self.publish(true)),
            Verb::Hb => {
                // Answered unless it is an answer itself.
                if req["echo"].as_bool() == Some(true) {
                    return Ok(None);
                }
                let load = self.daemon.mobile.load.level();
                return Ok(Some(json!({"type": "hb", "echo": true, "n": req["n"], "server_ts": now_millis(), "level": load.number()})));
            }
            Verb::Ack => {
                let id = req["id"].as_str().filter(|i| !i.is_empty() && i.len() <= 128).ok_or_else(|| refuse("bad_request", "missing session id"))?;
                let rev = req["rev"].as_u64().ok_or_else(|| refuse("bad_request", "missing rev"))?;
                self.daemon.mobile.views.ack(&self.device, id, self.id, rev);
            }
            Verb::Sub => {
                let (id, session) = self.terminal(req)?;
                self.make_room(&id)?;
                let height = req["height"].as_u64().map_or(sync::MAX_HEIGHT, |h| h as usize).clamp(sync::MIN_HEIGHT, sync::MAX_HEIGHT);
                if let Some(previous) = self.followed.remove(&id) {
                    previous.task.abort();
                }
                // The session being looked at is the last one asked for, unless the phone says it is a background one.
                let focused = req["focus"].as_bool() != Some(false);
                if focused {
                    for other in self.followed.values() {
                        other.focused.store(false, Ordering::Relaxed);
                    }
                }
                let focused = Arc::new(AtomicBool::new(focused));
                let m = &self.daemon.mobile;
                let have = req["have_rev"].as_u64().or_else(|| req["rev"].as_u64());
                let acked = m.views.open_as(&self.device, &id, self.id, have, self.legacy);
                let follow = sync::Follow { views: m.views.clone(), device: self.device.clone(), conn: self.id, id: id.clone(), height, session: Arc::downgrade(&session), out: self.out.clone(), focused: focused.clone(), acked };
                self.followed.insert(id, Followed { task: tokio::spawn(sync::run(follow)), focused });
            }
            Verb::Unsub => {
                // Let go first: a session that is gone still counts against the limit until then.
                let id = req["id"].as_str().filter(|i| !i.is_empty() && i.len() <= 128).ok_or_else(|| refuse("bad_request", "missing session id"))?;
                if let Some(followed) = self.followed.remove(id) {
                    followed.task.abort();
                    // Aborted before it could say so: nothing follows it for this device any more.
                    self.daemon.mobile.views.close(&self.device, id, self.id);
                }
                self.owed.remove(id);
                self.chat_at.remove(id);
                if self.watched.remove(id) {
                    if let Some(Entry::Chat(c)) = self.daemon.by_id(id) {
                        c.detach(self.id);
                    }
                }
            }
            Verb::ChatSub => {
                let (id, chat) = self.chat(req)?;
                self.make_room(&id)?;
                if self.chat_at.get(&id).is_some_and(|at| at.elapsed() < CHAT_SUB_GAP) {
                    return Err(refuse("too_fast", "Asked for that chat again too soon."));
                }
                // Refused while a log is still on its way, here or (at a busy load) to other phones: the
                // phone is told to ask again when there is room.
                if let Err(busy) = self.chat_room() {
                    self.owed.insert(id);
                    return Err(busy);
                }
                chat.observe(self.id, &self.tx, &json!(format!("chat:{id}")), req["since_seq"].as_u64(), &self.waiting)
                    .map_err(|_| refuse("busy", "The last chat log is still being sent."))?;
                self.owed.remove(&id);
                self.chat_at.insert(id.clone(), Instant::now());
                self.watched.insert(id);
            }
            Verb::Input => {
                let (id, session) = self.terminal(req)?;
                self.daemon.mobile.views.boost(&id);
                let text = req["text"].as_str().ok_or_else(|| refuse("bad_request", "missing text"))?;
                if text.len() > MAX_TYPED {
                    return Err(refuse("too_long", "too long"));
                }
                if session.choice_lasts(text.trim()) {
                    return Err(refuse("one_shot", ONE_SHOT));
                }
                if !session.is_alive() {
                    return Err(refuse("exited", "This session has stopped."));
                }
                let by = By { id: self.device.clone(), name: self.name.clone() };
                let typed = session.type_message_by(text, req["force"].as_bool() == Some(true), by).map_err(|e| answer_refusal(&self.device, e))?;
                if enter {
                    // Waited for here: the next message of this phone then starts after this Enter.
                    tokio::time::sleep(crate::session::ENTER_DELAY).await;
                    // Skipped when someone else typed meanwhile or a prompt came up: the text is on the
                    // prompt line, not sent.
                    let entered = session.enter_if(typed);
                    return Ok((!entered || req.get("req").is_some()).then(|| json!({"type": "ack", "entered": entered})));
                }
            }
            Verb::Key => {
                let (id, session) = self.terminal(req)?;
                self.daemon.mobile.views.boost(&id);
                let bytes = req["key"].as_str().and_then(verbs::key_bytes).ok_or_else(|| refuse("bad_request", "unknown key"))?;
                if req["key"].as_str().is_some_and(|k| session.choice_lasts(k)) {
                    return Err(refuse("one_shot", ONE_SHOT));
                }
                if !session.is_alive() {
                    return Err(refuse("exited", "This session has stopped."));
                }
                session.input_by(bytes, By { id: self.device.clone(), name: self.name.clone() });
            }
            Verb::Answer => {
                let (_, session) = self.terminal(req)?;
                let answer = match (req["key"].as_str(), req["text"].as_str()) {
                    (Some(key), _) => Answer::Key(key),
                    (None, Some(text)) if text.len() <= MAX_TYPED => Answer::Text(text),
                    _ => return Err(refuse("bad_request", "an answer is a key or text")),
                };
                // What is typed goes in as it is, so on a prompt with a lasting choice only its plain choices are let through.
                if session.screen_prompt().is_some_and(|p| !p.plain_answer(req["key"].as_str(), req["text"].as_str())) {
                    return Err(refuse("one_shot", ONE_SHOT));
                }
                let by = By { id: self.device.clone(), name: self.name.clone() };
                session.answer_by(req["waiting_since"].as_u64(), answer, enter, by).map_err(|e| answer_refusal(&self.device, e))?;
                return Ok(Some(json!({"type": "ack", "result": "ok"})));
            }
            Verb::ChatSend => {
                let (_, chat) = self.chat(req)?;
                let cid = req["cid"].as_str().filter(|c| !c.is_empty() && c.len() <= 64).ok_or_else(|| refuse("bad_request", "missing cid"))?;
                let text = req["text"].as_str().ok_or_else(|| refuse("bad_request", "missing text"))?;
                let by = By { id: self.device.clone(), name: self.name.clone() };
                // A retry of a message the queue remembers costs nothing and makes no second one.
                if let Some((qid, state)) = chat.sent_state(&by.id, cid) {
                    return Ok(Some(json!({"type": "ack", "result": "dup", "qid": qid, "state": state.name()})));
                }
                if text.len() > MAX_TYPED {
                    return Err(refuse("too_long", crate::chat::SendRefused::TooLong.message()));
                }
                if !self.daemon.mobile.allow_send(&self.device) {
                    return Err(refuse("rate_limited", "Sending too fast: wait a moment."));
                }
                let new = crate::chat::NewMessage { cid: cid.to_string(), by, text: text.to_string(), images: Vec::new(), origin: crate::chat::MessageOrigin::Phone };
                // The permission is read again once the chat's queue is locked: a revoke that came in since the check at the top
                // leaves nothing behind. A message the queue refuses spent nothing.
                let daemon = self.daemon.clone();
                let device = self.device.clone();
                let cap = self.cap;
                let q = chat
                    .enqueue_if(new, &|| daemon.mobile.permission(&device).map(|p| p.min(cap)) >= Some(Permission::Write))
                    .map_err(|r| {
                        self.daemon.mobile.refund_send(&self.device);
                        refuse(r.code(), r.message())
                    })?;
                if q.dup {
                    self.daemon.mobile.refund_send(&self.device);
                }
                return Ok(Some(json!({"type": "ack", "result": if q.dup { "dup" } else { "ok" }, "qid": q.qid, "state": q.state.name(), "ahead": q.ahead})));
            }
            Verb::ChatCancel => {
                let (_, chat) = self.chat(req)?;
                let qid = req["qid"].as_str().filter(|q| !q.is_empty() && q.len() <= 64).ok_or_else(|| refuse("bad_request", "missing qid"))?;
                chat.cancel_by(qid, &By { id: self.device.clone(), name: self.name.clone() }).map_err(|_| refuse("too_late", "That message already went out, or was cancelled."))?;
                return Ok(Some(json!({"type": "ack", "result": "ok"})));
            }
            Verb::ChatStop => {
                let (_, chat) = self.chat(req)?;
                let stopped = chat.stop(By { id: self.device.clone(), name: self.name.clone() });
                return Ok(Some(json!({"type": "ack", "result": if stopped == crate::chat::Stopped::Done { "ok" } else { "noop" }})));
            }
            Verb::ChatResume => {
                let (_, chat) = self.chat(req)?;
                chat.resume();
                return Ok(Some(json!({"type": "ack", "result": "ok"})));
            }
            Verb::ChatAnswer => {
                let (_, chat) = self.chat(req)?;
                let request_id = req.get("request_id").filter(|r| r.is_string() || r.is_number()).ok_or_else(|| refuse("bad_request", "missing request_id"))?;
                let line = req["line"].as_str().filter(|l| l.len() <= MAX_ANSWER_LINE).ok_or_else(|| refuse("bad_request", "missing or too long line"))?;
                let by = By { id: self.device.clone(), name: self.name.clone() };
                chat.answer(request_id, line, by).map_err(|e| chat_answer_refusal(&self.device, e))?;
                return Ok(Some(json!({"type": "ack", "result": "ok"})));
            }
        }
        Ok(None)
    }

    fn end(&mut self) {
        for (_, followed) in self.followed.drain() {
            followed.task.abort();
        }
        self.daemon.mobile.views.leave(&self.device, self.id);
        self.daemon.mobile.waiting.lock().remove(&self.id);
        self.daemon.unregister_client(self.id);
    }
}

/// A paired device's connection, once it proved it finished its handshake (`link`); `first` is what it said then.
async fn client(daemon: Arc<Daemon>, mut wire: Wire, generation: Arc<Generation>, device: Device, peer: Peer, browser: bool, first: Value) {
    daemon.mobile.seen(&device.id);
    let legacy = !acknowledges(Some(&first));
    let cap = transport_cap(device.kind, browser, &peer);
    // The daemon's events reach the phone through an adapter: a connection is a client like the app,
    // with a queue that has a limit.
    let id = next_client_id();
    let (tx, rx, overflowed) = bounded_client_channel(EVENT_QUEUE);
    daemon.register_client(id, tx.clone());
    daemon.set_client_role(id, &crate::auth::Role::User);
    let lanes = lanes::lanes();
    let dirty = Arc::new(Notify::new());
    let mut adapter = tokio::spawn(adapt(daemon.clone(), rx, lanes.bulk.clone(), lanes.slow.clone(), dirty.clone(), device.id.clone()));
    let waiting = Arc::new(AtomicUsize::new(0));
    daemon.mobile.waiting.lock().insert(id, waiting.clone());
    let mut conn = Conn {
        daemon: daemon.clone(), device: device.id.clone(), name: device.name.clone(), id, tx, out: lanes.bulk.clone(),
        followed: HashMap::new(), watched: HashSet::new(), owed: HashSet::new(), chat_at: HashMap::new(), waiting, overflowed,
        cap, list: None, rev: 0, legacy, _phone: Some(daemon.mobile.load.phone()),
    };
    // The device's older connection, if any, is told it was replaced.
    let replaced = Arc::new(Notify::new());
    if let Some((_, older)) = daemon.mobile.connections.lock().insert(device.id.clone(), (id, replaced.clone())) {
        older.notify_one();
    }
    let mut wires = Wires { bulk_rx: lanes.bulk_rx, slow: lanes.slow, dirty, replaced };
    // Dropping serve cancels an in-flight request/send, then the common cleanup still runs.
    tokio::select! {
        biased;
        _ = generation.cancelled() => {}
        _ = serve(&mut conn, &mut wire, &device, &mut wires, &mut adapter, &generation) => {}
    }
    // Turned off because every device was removed (kill switch, reset): the phone forgets a pairing that no longer exists,
    // instead of trying a computer that stays silent. Plain "phone access off" keeps the record, and with it the pairing.
    if generation.is_cancelled() && daemon.mobile.removed(&device.id) {
        wire.close(NOT_PAIRED, "revoked").await;
    }
    adapter.abort();
    {
        let mut open = daemon.mobile.connections.lock();
        if open.get(&device.id).is_some_and(|(c, _)| *c == id) {
            open.remove(&device.id);
        }
    }
    conn.end();
}

/// Whether the first message (`{caps}`) says the phone acknowledges frames and answers heartbeats. A client that says
/// nothing is served without either: nothing is withheld until it acknowledges and no silence is held against it (the OS
/// keep-alive and the write timeout still end a dead one).
fn acknowledges(first: Option<&Value>) -> bool {
    let caps = first.and_then(|v| v["caps"].as_array());
    ["ack", "hb"].iter().all(|c| caps.is_some_and(|caps| caps.iter().any(|x| x == c)))
}

/// What a connection waits on besides its socket.
struct Wires {
    bulk_rx: mpsc::Receiver<String>,
    /// Something decided this phone cannot take what it is sent.
    slow: Arc<Notify>,
    /// The session list may have changed.
    dirty: Arc<Notify>,
    /// Another connection of this device took over.
    replaced: Arc<Notify>,
}

/// A heartbeat: `n` counts them, `server_ts` lets the phone see the clock and data age.
fn heartbeat(n: u64, level: Level) -> String {
    json!({"type": "hb", "n": n, "server_ts": now_millis(), "level": level.number()}).to_string()
}

/// What a phone is told about the machine's load: the level, why, since when.
fn load_status(load: &Load) -> String {
    let mut msg = load.status().json();
    msg["type"] = json!("load.status");
    msg["server_ts"] = json!(now_millis());
    msg.to_string()
}

/// What the phone is told about itself: what it may do on this connection, and whether the transport holds it below its record.
fn device_hello(device: &Device, record: Permission, cap: Permission) -> Value {
    let effective = record.min(cap);
    json!({"id": device.id, "name": device.name, "can_write": effective >= Permission::Write, "permission": effective.name(), "kind": device.kind.name(), "capped": effective < record})
}

/// Where this computer can be reached, for a phone to dial next time (after an address change above all).
fn addrs_message(a: &Addrs) -> String {
    json!({"type": "addrs", "seq": a.seq, "list": a.list, "hostname": listen::hostname()}).to_string()
}

/// The record allows nothing now: a phone whose record is gone is told so, one that is merely not let in now (phone access off)
/// connects again.
async fn no_access(wire: &mut Wire, m: &Mobile, device: &Device) {
    if m.removed(&device.id) && m.key_paired(&device.pk) {
        wire.close(REPLACED, "replaced").await;
    } else if m.removed(&device.id) {
        wire.close(NOT_PAIRED, "revoked").await;
    } else {
        wire.close(RECONNECT, "off").await;
    }
}

async fn serve(conn: &mut Conn, wire: &mut Wire, device: &Device, wires: &mut Wires, adapter: &mut tokio::task::JoinHandle<()>, generation: &Generation) {
    if generation.is_cancelled() {
        return;
    }
    let daemon = conn.daemon.clone();
    let m = &daemon.mobile;
    let load = m.load.clone();
    let mut revoked = m.revoked.subscribe();
    let mut load_changes = load.subscribe();
    let mut addrs = m.addrs.subscribe();
    // What the record allowed when the connection began: a change means connect again.
    let Some(record) = m.permission(&device.id) else {
        no_access(wire, m, device).await;
        return;
    };
    let first = conn.publish(true).expect("forced");
    let hello = json!({
        "type": "hello", "v": 2, "api": kelpie_proto::API_LEVEL, "daemon_epoch": m.epoch, "rev": first["rev"],
        "device": device_hello(device, record, conn.cap),
        "sessions": first["sessions"], "projects": first["projects"],
        "load": load.status().json(), "server_ts": now_millis(),
        "hb": {"idle_s": 20, "pong_s": 8, "dead_s": 45, "legacy": conn.legacy, "ack_frames": sync::WINDOW_FRAMES, "ack_bytes": sync::WINDOW_BYTES, "ack_every_frames": 2, "ack_every_ms": 250},
    });
    if !wire.send(&hello.to_string()).await {
        return;
    }
    let told = addrs.borrow_and_update().clone();
    if !wire.send(&addrs_message(&told)).await {
        return;
    }
    let (mut live, mut beats) = (Liveness::new(Instant::now()), 0);
    let began = Instant::now();
    let mut tick = tokio::time::interval(lanes::TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        // Also check synchronously: a busy socket can keep this future ready without yielding.
        if generation.is_cancelled() {
            return;
        }
        // In this order: what ends the connection, the heartbeat, then the
        // phone's own requests and the list, and the bulk lane last.
        tokio::select! {
            biased;
            r = revoked.recv() => {
                match on_event(r, &device.id) {
                    OnEvent::Close(code, reason) => {
                        wire.close(code, reason).await;
                        return;
                    }
                    OnEvent::Tell(msg) => {
                        if !wire.send(&msg.to_string()).await {
                            return;
                        }
                    }
                    // Something may have been missed: the record decides.
                    OnEvent::Recheck => match conn.daemon.mobile.permission(&device.id) {
                        None => {
                            no_access(wire, m, device).await;
                            return;
                        }
                        Some(now) if now != record => {
                            wire.close(RECONNECT, "changed").await;
                            return;
                        }
                        Some(_) => {}
                    },
                }
            }
            _ = wires.slow.notified() => {
                wire.close(SLOW_CONSUMER, "slow_consumer").await;
                return;
            }
            _ = &mut *adapter => return,
            _ = wires.replaced.notified() => {
                wire.close(REPLACED, "replaced").await;
                return;
            }
            _ = tick.tick() => {
                // A message whose chunks stopped coming: this session is broken, the phone handshakes again.
                if wire.session.reassembly_expired(now_millis()) {
                    return;
                }
                if began.elapsed() >= REHANDSHAKE_AFTER {
                    wire.close(RECONNECT, "rehandshake").await;
                    return;
                }
                let level = load.level();
                let beat = live.tick(Instant::now(), level.patience(), |since| load.healthy_since(since));
                match if conn.legacy { Beat::Quiet } else { beat } {
                    Beat::Dead => {
                        wire.close(SILENT, "silent").await;
                        return;
                    }
                    Beat::Ping => {
                        beats += 1;
                        if !wire.send(&heartbeat(beats, level)).await {
                            return;
                        }
                    }
                    Beat::Quiet => {}
                }
                // While the machine is loaded the phone hears it every second (a missed one is soon replaced).
                if level > Level::L0 && !wire.send(&load_status(&load)).await {
                    return;
                }
                conn.daemon.mobile.views.sweep();
                for msg in conn.housekeeping() {
                    if !wire.send(&msg.to_string()).await {
                        return;
                    }
                }
            }
            changed = load_changes.changed() => {
                if changed.is_err() {
                    return;
                }
                if !wire.send(&load_status(&load)).await {
                    return;
                }
            }
            changed = addrs.changed() => {
                if changed.is_err() {
                    return;
                }
                let now = addrs.borrow_and_update().clone();
                if !wire.send(&addrs_message(&now)).await {
                    return;
                }
            }
            _ = wires.dirty.notified() => {
                if let Some(list) = conn.publish(false) {
                    if !wire.send(&list.to_string()).await {
                        return;
                    }
                }
            }
            msg = wire.ws.recv() => {
                let Some(Ok(msg)) = msg else { return };
                // Anything at all, a ping frame included, says the phone is there.
                live.heard(Instant::now());
                let text = match wire.open(msg) {
                    Opened::Text(t) => t,
                    Opened::Nothing => continue,
                    Opened::End => return,
                };
                let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
                if generation.is_cancelled() {
                    return;
                }
                // Every request is checked against the device's record as it is now, and held to what this transport allows.
                let Some(granted) = conn.daemon.mobile.permission(&device.id) else {
                    no_access(wire, m, device).await;
                    return;
                };
                if let Some(reply) = conn.handle(granted.min(conn.cap), &req).await {
                    if !wire.send(&reply.to_string()).await {
                        return;
                    }
                }
            }
            Some(msg) = wires.bulk_rx.recv() => {
                if !wire.send(&msg).await {
                    return;
                }
            }
        }
    }
}


/// Turns what the daemon tells this connection (a client like any other) into messages for the
/// phone: a changed session list wakes the connection to publish it, a chat's log arrives as
/// `chat` messages, and a request answered elsewhere as `chat_resolved`.
async fn adapt(daemon: Arc<Daemon>, mut rx: mpsc::Receiver<Outgoing>, bulk: mpsc::Sender<String>, slow: Arc<Notify>, dirty: Arc<Notify>, device: String) {
    let mut translator = Translator { device, ..Default::default() };
    while let Some(first) = rx.recv().await {
        // What else is waiting is taken with it: of several states of one chat's queue only the
        // latest is worth sending.
        let mut batch = vec![first];
        while batch.len() < 16 {
            match rx.try_recv() {
                Ok(o) => batch.push(o),
                Err(_) => break,
            }
        }
        let mut messages: Vec<Value> = batch.iter().flat_map(|o| translator.translate(&daemon, &o.data, &dirty)).collect();
        latest_queue_only(&mut messages);
        for msg in messages {
            // Everything about a chat goes the same way, in order: a resolution or a resync is queued after
            // the lines it is about, and the lane is first in, first out. A message waits for room in the
            // lane; no room for `SLOW_AFTER` means a phone that cannot keep up.
            match tokio::time::timeout(SLOW_AFTER, bulk.send(msg.to_string())).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return,
                Err(_) => {
                    slow.notify_one();
                    return;
                }
            }
        }
        // Written off only once the phone's queue took it: a phone that cannot keep up makes the
        // chat stop feeding it (and tell it to resync) instead of a backlog growing here.
        for o in batch {
            if let Some(credit) = o.credit {
                credit.fetch_sub(o.data.len(), Ordering::Relaxed);
            }
        }
    }
}

/// Of several `chat_queue` messages for one chat, keeps the last: each says the whole state of the
/// queue, and the ones in between are out of date. What they list as dropped is kept (it is news,
/// not state).
fn latest_queue_only(messages: &mut Vec<Value>) {
    let mut later: HashSet<String> = HashSet::new();
    let mut carried: HashMap<String, Vec<Value>> = HashMap::new();
    let mut keep = vec![true; messages.len()];
    for (i, m) in messages.iter().enumerate().rev() {
        if m["type"] != "chat_queue" {
            continue;
        }
        let id = m["id"].as_str().unwrap_or_default().to_string();
        if !later.insert(id.clone()) {
            keep[i] = false;
            if let Some(d) = m["dropped"].as_array() {
                carried.entry(id).or_default().extend(d.iter().cloned());
            }
        }
    }
    let mut at = 0;
    messages.retain(|_| {
        at += 1;
        keep[at - 1]
    });
    for m in messages.iter_mut().filter(|m| m["type"] == "chat_queue") {
        if let Some(extra) = carried.remove(m["id"].as_str().unwrap_or_default()) {
            let mut all: Vec<Value> = extra;
            all.extend(m["dropped"].as_array().cloned().unwrap_or_default());
            m["dropped"] = json!(all);
        }
    }
}

/// What `adapt` remembers between frames.
#[derive(Default)]
struct Translator {
    /// The device this connection is for: its own queued messages are marked `mine`.
    device: String,
    /// Session id of each handle seen.
    ids: HashMap<u32, String>,
    /// Chats whose attach reply came: the next output frame of that handle is the log it promised,
    /// in this mode.
    awaiting: HashMap<u32, String>,
}

impl Translator {
    fn session_of(&mut self, daemon: &Daemon, handle: u32) -> Option<String> {
        if let Some(id) = self.ids.get(&handle) {
            return Some(id.clone());
        }
        let id = daemon.session_id_of(handle)?;
        self.ids.insert(handle, id.clone());
        Some(id)
    }

    fn translate(&mut self, daemon: &Daemon, frame: &[u8], dirty: &Notify) -> Vec<Value> {
        // A frame is `u32 length | u8 kind | payload`.
        match frame.get(4).copied() {
            Some(kelpie_proto::KIND_OUTPUT) if frame.len() >= 17 => {
                let handle = u32::from_le_bytes(frame[5..9].try_into().unwrap());
                let seq = u64::from_le_bytes(frame[9..17].try_into().unwrap());
                let mode = self.awaiting.remove(&handle).unwrap_or_else(|| "live".into());
                self.session_of(daemon, handle).map(|id| chat_messages(&id, &mode, seq, &frame[17..])).unwrap_or_default()
            }
            Some(kelpie_proto::KIND_JSON) => {
                let body = &frame[5..];
                let is = |event: &str| body.starts_with(format!(r#"{{"event":"{event}""#).as_bytes());
                if is("session.updated") || is("session.closed") {
                    dirty.notify_one();
                    return Vec::new();
                }
                let parse = || serde_json::from_slice::<Value>(body).ok();
                if is("chat.resolved") {
                    return parse().map(|v| vec![json!({"type": "chat_resolved", "id": v["params"]["id"], "request_id": v["params"]["request_id"], "by": v["params"]["by"], "dropped": v["params"]["dropped"], "answered_by": v["params"]["answered_by"], "at": v["params"]["at"]})]).unwrap_or_default();
                }
                if is("chat.queue") {
                    return parse().map(|v| vec![queue_message(&v["params"], &self.device)]).unwrap_or_default();
                }
                if is("session.resync") {
                    let handle = parse().and_then(|v| v["params"]["handle"].as_u64()).and_then(|h| u32::try_from(h).ok());
                    return handle.and_then(|h| self.session_of(daemon, h)).map(|id| vec![json!({"type": "chat_resync", "id": id})]).unwrap_or_default();
                }
                // The reply to a `chat_sub` attach (its request id names the session); the log follows.
                if body.starts_with(br#"{"id":"chat:"#) {
                    let v = parse().unwrap_or_default();
                    if let (Some(handle), Some(result)) = (v["result"]["handle"].as_u64().and_then(|h| u32::try_from(h).ok()), v.get("result")) {
                        self.awaiting.insert(handle, result["mode"].as_str().unwrap_or("reset").to_string());
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// The queue of a chat as the phone is told it: who queued each message (`mine`: this device did).
fn queue_message(p: &Value, device: &str) -> Value {
    let item = |e: &Value| {
        let text: String = e["text"].as_str().unwrap_or_default().chars().take(300).collect();
        let mut v = json!({"qid": e["qid"], "cid": e["cid"], "by": e["by"]["name"], "mine": e["by"]["id"].as_str() == Some(device), "kind": e["kind"], "text": text, "reason": e["reason"], "needs_window": e["needs_window"]});
        // Who cancelled it, for the phone whose message it was: the computer, or another phone.
        if e["cancelled_by"].is_object() {
            v["cancelled_by"] = json!(if e["cancelled_by"]["id"] == "desktop" { "desktop" } else { "phone" });
            v["cancelled_by_name"] = e["cancelled_by"]["name"].clone();
        }
        v
    };
    let list = |key: &str| p[key].as_array().map(|a| a.iter().map(item).collect::<Vec<_>>());
    let mut msg = json!({"type": "chat_queue", "id": p["id"], "items": list("items").unwrap_or_default(), "paused": p["paused"], "held": p["held"], "in_flight": p["in_flight"]});
    if let Some(dropped) = list("dropped") {
        msg["dropped"] = json!(dropped);
    }
    msg
}

/// A log line as a phone is sent it. A message that goes to a phone is at most `MAX_APP_MESSAGE`: a line that would not fit
/// once it is JSON-escaped (6 bytes for each control character at worst) becomes a notice, as a line too long for the log does,
/// so the position in the log stays right and the connection is not lost to it again and again.
fn phone_line(line: &[u8]) -> String {
    let text = String::from_utf8_lossy(line);
    // Room for the rest of the message around the one line.
    let room = kelpie_link::MAX_APP_MESSAGE - 1024;
    if line.len() > room / 6 && serde_json::to_string(text.as_ref()).map_or(usize::MAX, |escaped| escaped.len()) > room {
        return json!({"type": "kelpie_stderr", "text": format!("[a {} byte line is not shown on the phone]", line.len())}).to_string();
    }
    text.into_owned()
}

/// A chat's log lines as messages of about `CHAT_MESSAGE_BYTES`. `seq` is the log position after
/// the last line; each message says the position after its own, so a phone that loses the
/// connection halfway resumes from the last whole message. The first message of a reset or delta
/// carries `mode`, the rest say `more`.
fn chat_messages(id: &str, mode: &str, seq: u64, data: &[u8]) -> Vec<Value> {
    let mut groups: Vec<(Vec<String>, usize)> = Vec::new();
    for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let size = line.len() + 1;
        match groups.last_mut() {
            Some((lines, bytes)) if *bytes + size <= CHAT_MESSAGE_BYTES => {
                lines.push(phone_line(line));
                *bytes += size;
            }
            _ => groups.push((vec![phone_line(line)], size)),
        }
    }
    if groups.is_empty() {
        groups.push((Vec::new(), 0));
    }
    let mut later: usize = groups.iter().map(|g| g.1).sum();
    let rest = if mode == "live" { "live" } else { "more" };
    groups
        .into_iter()
        .enumerate()
        .map(|(i, (lines, bytes))| {
            later -= bytes;
            json!({"type": "chat", "id": id, "mode": if i == 0 { mode } else { rest }, "seq": seq.saturating_sub(later as u64), "lines": lines})
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_and_hex() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        let k = [0xabu8; 32];
        assert_eq!(unhex32(&hex(&k)), Some(k));
        assert_eq!(unhex32("zz"), None);
        assert_eq!(unhex32(&"g".repeat(64)), None);
    }

    #[test]
    fn a_browser_page_types_only_over_https_or_tailscale_and_never_controls() {
        let at = |addr: &str, tls: bool| Peer { addr: addr.parse().unwrap(), tls };
        assert_eq!(transport_cap(Kind::Pwa, true, &at("192.168.1.9:5000", false)), Permission::Read, "plain http on the LAN: the page could be swapped");
        assert_eq!(transport_cap(Kind::Pwa, true, &at("192.168.1.9:5000", true)), Permission::Write);
        assert_eq!(transport_cap(Kind::Pwa, true, &at("100.100.1.2:5000", false)), Permission::Write, "tailscale");
        assert_eq!(transport_cap(Kind::Pwa, true, &at("[fd7a:115c:a1e0::5]:5000", false)), Permission::Write);
        assert_eq!(transport_cap(Kind::App, false, &at("192.168.1.9:5000", false)), Permission::Control, "the native app is not capped");
        assert_eq!(transport_cap(Kind::App, true, &at("192.168.1.9:5000", false)), Permission::Read, "an app record used from a browser is a page");
        assert_eq!(transport_cap(Kind::Pwa, false, &at("192.168.1.9:5000", true)), Permission::Write, "a page paired as one stays one");
        assert_eq!(transport_cap(Kind::Pwa, true, &at("127.0.0.1:5000", false)), Permission::Write, "no network between: nothing to swap the page");
        // What the phone hears.
        let mut d = Device { id: "d".into(), name: "n".into(), pk: String::new(), permission: Permission::Write, kind: Kind::Pwa, created_at: 0, last_seen: 0, mac: String::new(), verified: true };
        let capped = device_hello(&d, d.permission, Permission::Read);
        assert_eq!((capped["permission"].as_str(), capped["can_write"].as_bool(), capped["capped"].as_bool()), (Some("read"), Some(false), Some(true)));
        d.permission = Permission::Read;
        assert_eq!(device_hello(&d, d.permission, Permission::Read)["capped"], false, "it may only read anyway");
        assert_eq!(fingerprint(&[0u8; 32]), "6668 7AAD F862 BD77", "the phone computes the same (noise.test.ts)");
    }

    #[test]
    fn origin_must_match_host() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "192.168.1.2:4817".parse().unwrap());
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, "http://192.168.1.2:4817".parse().unwrap());
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        assert!(!same_origin(&h));
    }

    #[test]
    fn pairing_failures_are_counted_per_address() {
        let m = Mobile::load(&std::env::temp_dir().join(format!("kelpie-limit-{}", uuid::Uuid::new_v4())), crate::notify::Memory::new());
        let (a, b): (IpAddr, IpAddr) = ("192.168.1.20".parse().unwrap(), "192.168.1.21".parse().unwrap());
        for _ in 0..PAIR_FAILURES {
            assert!(!m.pairing_blocked(a));
            m.pairing_failed(a);
        }
        assert!(m.pairing_blocked(a));
        assert!(!m.pairing_blocked(b), "another address is not held up by it");
        // Failures older than a minute are forgotten.
        for t in m.failures.lock().get_mut(&a).unwrap().iter_mut() {
            *t = Instant::now() - Duration::from_secs(61);
        }
        assert!(!m.pairing_blocked(a));
        assert!(m.failures.lock().get(&a).is_none(), "and so is the address");
        // The table is bounded.
        for i in 0..(LIMITER_ADDRESSES + 5) {
            m.pairing_failed(IpAddr::from([10, 0, (i / 256) as u8, (i % 256) as u8]));
        }
        assert!(m.failures.lock().len() <= LIMITER_ADDRESSES);
    }

    /// A daemon with a terminal session `t1` (a Claude Code that is not asking anything) and a
    /// chat `c1`, and a connection of the device `phone`.
    struct Rig {
        daemon: Arc<Daemon>,
        conn: Conn,
        out: mpsc::Receiver<String>,
        term: Arc<crate::session::Session>,
        typed: crate::session::fake::Typed,
        chat: Arc<crate::chat::ChatSession>,
        /// The paired phone's Noise static key.
        phone: kelpie_link::Keypair,
    }

    fn rig() -> Rig {
        let dir = std::env::temp_dir().join(format!("kelpie-phone-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let daemon = Daemon::with_secrets(dir, std::path::PathBuf::from("kelpied"), crate::notify::Memory::new());
        let (term, typed) = crate::session::fake::running(1, "t1", "claude");
        daemon.insert_for_test(Entry::Term(term.clone()));
        let chat = crate::chat::ChatSession::new(2, crate::session::SessionSpec { id: "c1".into(), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
        daemon.insert_for_test(Entry::Chat(chat.clone()));
        let phone = kelpie_link::Keypair::generate().unwrap();
        daemon.mobile.add_test_device("phone", "Phone", phone.public, Permission::Write, Kind::App);
        daemon.mobile.config.lock().enabled = true;
        let (tx, _rx, overflowed) = bounded_client_channel(8);
        let (out_tx, out) = mpsc::channel(16);
        let conn = Conn {
            daemon: daemon.clone(), device: "phone".into(), name: "Phone".into(), id: 1, tx, out: out_tx, followed: HashMap::new(), watched: HashSet::new(), owed: HashSet::new(),
            chat_at: HashMap::new(), waiting: Arc::new(AtomicUsize::new(0)), overflowed, cap: Permission::Control, list: None, rev: 0, legacy: false, _phone: None,
        };
        Rig { daemon, conn, out, term, typed, chat, phone }
    }

    impl Rig {
        /// What the phone is told about a request (every request carries `req`, so there is always an answer).
        async fn ask(&mut self, granted: Permission, mut req: Value) -> Value {
            req["req"] = json!(1);
            self.conn.handle(granted, &req).await.expect("a reply")
        }

        async fn ask_write(&mut self, req: Value) -> Value {
            self.ask(Permission::Write, req).await
        }
    }

    /// Real loopback transports, isolated from the user's daemon and mobile settings.
    struct NetworkRig {
        rig: Rig,
        dir: PathBuf,
        addr: SocketAddr,
    }

    impl NetworkRig {
        async fn new() -> Self {
            let rig = rig();
            let dir = rig.daemon.mobile.dir.clone();
            let mut network = Self { rig, dir, addr: "127.0.0.1:0".parse().unwrap() };
            network.restart().await;
            network
        }

        async fn restart(&mut self) {
            // Reserve an OS-selected port, never the daemon's default port.
            let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            self.addr = reservation.local_addr().unwrap();
            drop(reservation);
            enable(self.rig.daemon.clone(), json!({"ip": "127.0.0.1", "port": self.addr.port()})).await.unwrap();
        }

        async fn http(&self) -> tokio::net::TcpStream {
            tokio::net::TcpStream::connect(self.addr).await.unwrap()
        }

        /// A WebSocket to `/link` with the rig's paired phone's key (`browser`: as a page, with an `Origin`).
        async fn websocket(&self) -> TestPhone {
            self.socket_as(self.rig.phone.clone(), false).await
        }

        async fn socket_as(&self, keys: kelpie_link::Keypair, browser: bool) -> TestPhone {
            use tokio::io::AsyncWriteExt;
            let mut tcp = self.http().await;
            let origin = if browser { format!("Origin: http://{}\r\n", self.addr) } else { String::new() };
            tcp.write_all(format!(
                "GET /link HTTP/1.1\r\nHost: {}\r\n{origin}Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
                self.addr
            ).as_bytes()).await.unwrap();
            assert!(http_head(&mut tcp).await.starts_with("HTTP/1.1 101"));
            let desktop = self.rig.daemon.mobile.link_key().unwrap().public;
            TestPhone { tcp, session: None, keys, desktop, closed: None }
        }
    }

    /// A phone speaking kelpie-link over a raw WebSocket.
    struct TestPhone {
        tcp: tokio::net::TcpStream,
        session: Option<kelpie_link::Session>,
        keys: kelpie_link::Keypair,
        desktop: [u8; 32],
        /// The close code the daemon sent.
        closed: Option<u16>,
    }

    /// One masked binary frame (client to server).
    async fn ws_send(tcp: &mut tokio::net::TcpStream, opcode: u8, body: &[u8]) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let mask = [17, 29, 43, 59];
        let mut frame = vec![0x80 | opcode];
        match body.len() {
            n if n < 126 => frame.push(0x80 | n as u8),
            n if n <= 0xffff => {
                frame.push(0x80 | 126);
                frame.extend((n as u16).to_be_bytes());
            }
            n => {
                frame.push(0x80 | 127);
                frame.extend((n as u64).to_be_bytes());
            }
        }
        frame.extend(mask);
        frame.extend(body.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        tcp.write_all(&frame).await
    }

    /// One frame from the server: (opcode, body); None when the transport ended.
    async fn ws_recv(tcp: &mut tokio::net::TcpStream) -> Option<(u8, Vec<u8>)> {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(Duration::from_secs(2), async {
            let opcode = tcp.read_u8().await.ok()? & 0x0f;
            let size = tcp.read_u8().await.ok()? & 0x7f;
            let size = match size {
                126 => tcp.read_u16().await.ok()? as usize,
                127 => usize::try_from(tcp.read_u64().await.ok()?).unwrap(),
                n => n as usize,
            };
            assert!(size <= kelpie_link::MAX_NOISE_MESSAGE);
            let mut body = vec![0; size];
            tcp.read_exact(&mut body).await.ok()?;
            Some((opcode, body))
        })
        .await
        .expect("mobile transport must respond or close promptly")
    }

    impl TestPhone {
        /// Sends a handshake's first message and reads the reply: the session, or None when the daemon closed instead.
        async fn handshake(&mut self, first: Vec<u8>, init: kelpie_link::Initiator) -> Option<()> {
            ws_send(&mut self.tcp, 2, &first).await.ok()?;
            match ws_recv(&mut self.tcp).await? {
                (2, reply) => {
                    self.session = Some(init.finish(&reply).ok()?);
                    Some(())
                }
                (8, body) => {
                    self.closed = body.get(..2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                    None
                }
                other => panic!("unexpected frame {other:?}"),
            }
        }

        async fn reconnect(&mut self) -> Option<()> {
            let (init, first) = kelpie_link::Initiator::reconnect(&self.keys, &self.desktop).unwrap();
            self.handshake(first, init).await
        }
    }

    async fn phone_send(p: &mut TestPhone, value: Value) -> std::io::Result<()> {
        let frames = p.session.as_mut().expect("a session").seal(value.to_string().as_bytes()).unwrap();
        for f in frames {
            ws_send(&mut p.tcp, 2, &f).await?;
        }
        Ok(())
    }

    /// None means the transport ended or the server sent a WebSocket close, not a timeout.
    async fn phone_recv(p: &mut TestPhone) -> Option<Value> {
        loop {
            match ws_recv(&mut p.tcp).await? {
                (8, body) => {
                    p.closed = body.get(..2).map(|c| u16::from_be_bytes([c[0], c[1]]));
                    return None;
                }
                (2, body) => {
                    if let Some(bytes) = p.session.as_mut().expect("a session").open(&body, now_millis()).expect("it decrypts") {
                        return Some(serde_json::from_slice(&bytes).unwrap());
                    }
                }
                (op, _) => panic!("unexpected opcode {op}: everything but control frames is binary"),
            }
        }
    }

    impl Drop for NetworkRig {
        fn drop(&mut self) {
            self.rig.daemon.mobile.disable();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn http_head(tcp: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(tcp.read_u8().await.unwrap());
                assert!(bytes.len() < 16 * 1024);
            }
            String::from_utf8(bytes).unwrap()
        }).await.expect("HTTP response")
    }

    /// Reconnects as the paired phone and says nothing of `caps` (served as a client that does not acknowledge), then
    /// reads the hello and the address list.
    async fn authenticate(p: &mut TestPhone) {
        p.reconnect().await.expect("a paired key is answered");
        phone_send(p, json!({})).await.unwrap();
        assert_eq!(phone_recv(p).await.unwrap()["type"], "hello");
        assert_eq!(phone_recv(p).await.unwrap()["type"], "addrs");
    }

    #[tokio::test]
    async fn security_regression_mobile_disable_ends_authenticated_socket_and_output() {
        let mut network = NetworkRig::new().await;
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        phone_send(&mut phone, json!({"type": "input", "id": "t1", "text": "before", "req": 1})).await.unwrap();
        assert_eq!(phone_recv(&mut phone).await.unwrap()["type"], "ack");
        assert_eq!(network.rig.typed.writes(), vec![b"before".to_vec()]);

        network.rig.daemon.mobile.disable();
        // A queued event must not leak through an already authenticated connection.
        network.rig.daemon.broadcast("session.updated", json!({"id": "t1"}));
        let _ = phone_send(&mut phone, json!({"type": "input", "id": "t1", "text": "after", "req": 2})).await;
        assert!(phone_recv(&mut phone).await.is_none(), "disabled sockets must not reply or publish events");
        assert!(network.rig.typed.writes().is_empty(), "disable removes write authority");
        assert!(matches!(network.rig.daemon.mobile.lookup_key(&network.rig.phone.public), KeyLookup::Unconfirmed), "off: not a stranger, and not let in");
        assert_eq!(network.rig.daemon.mobile.permission("phone"), None);

        network.restart().await;
        let mut fresh = network.websocket().await;
        authenticate(&mut fresh).await;
        phone_send(&mut fresh, json!({"type": "list", "req": 3})).await.unwrap();
        assert!(phone_recv(&mut fresh).await.is_some(), "paired device reconnects on the new server");
        assert!(phone_recv(&mut phone).await.is_none(), "old socket cannot revive");
    }

    #[tokio::test]
    async fn security_regression_mobile_shutdown_is_sticky_before_websocket_authentication() {
        let mut network = NetworkRig::new().await;
        // The upgrade has finished, but its token has not arrived when shutdown wins.
        let mut old = network.websocket().await;
        network.rig.daemon.mobile.disable();
        network.restart().await;
        assert!(old.reconnect().await.is_none(), "old generation cannot authenticate after re-enable");
        let mut fresh = network.websocket().await;
        authenticate(&mut fresh).await;
    }

    #[tokio::test]
    async fn security_regression_mobile_disable_closes_accepted_http_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let network = NetworkRig::new().await;
        let mut http = network.http().await;
        // HEAD has no body: consuming the headers leaves an idle, established keep-alive connection.
        let request = format!("HEAD / HTTP/1.1\r\nHost: {}\r\n\r\n", network.addr);
        http.write_all(request.as_bytes()).await.unwrap();
        assert!(http_head(&mut http).await.starts_with("HTTP/1.1 200"));
        network.rig.daemon.mobile.disable();
        let _ = http.write_all(request.as_bytes()).await;
        let mut byte = [0];
        let ended = tokio::time::timeout(Duration::from_secs(2), http.read(&mut byte)).await
            .expect("disabled accepted HTTP connection must close");
        assert!(matches!(ended, Ok(0) | Err(_)), "old HTTP connection still serves requests");
    }

    /// Whether this daemon carries the phone client (`pnpm build:phone` ran before it was built) or the placeholder page.
    fn has_phone_client() -> bool {
        !phone_asset("/").is_some_and(|(_, body)| String::from_utf8_lossy(body).contains("KELPIE_PHONE_BUNDLE_MISSING"))
    }

    #[test]
    fn the_phone_client_is_found_by_path_and_nothing_else_is() {
        let (ty, body) = phone_asset("/").expect("the page");
        assert!(ty.starts_with("text/html") && body.starts_with(b"<!doctype html"), "{ty}");
        assert_eq!(phone_asset("/index.html").map(|(_, b)| b.len()), Some(body.len()));
        for path in ["/nope.js", "/../Cargo.toml", "/assets/../index.html", "/app.js", "//", ""] {
            assert!(phone_asset(path).is_none(), "{path}");
        }
        if has_phone_client() {
            for path in ["/sw.js", "/manifest.webmanifest", "/icon.svg"] {
                assert!(phone_asset(path).is_some(), "{path}");
            }
            assert!(PHONE_ASSETS.iter().all(|(p, _, _)| !p.ends_with(".map")), "source maps are not served");
            assert!(PHONE_ASSETS.iter().any(|(p, ty, _)| p.starts_with("/assets/") && ty.starts_with("text/javascript")));
        } else {
            assert_eq!(PHONE_ASSETS.len(), 1, "the placeholder is one page");
        }
    }

    #[tokio::test]
    async fn the_phone_client_is_served_with_its_headers() {
        use tokio::io::AsyncWriteExt;
        let network = NetworkRig::new().await;
        let head = |path: String| {
            let network = &network;
            async move {
                let mut http = network.http().await;
                http.write_all(format!("HEAD {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", network.addr).as_bytes()).await.unwrap();
                http_head(&mut http).await.to_ascii_lowercase()
            }
        };
        let page = head("/".into()).await;
        assert!(page.starts_with("http/1.1 200") && page.contains("text/html") && page.contains("script-src 'self'") && page.contains("no-store"), "{page}");
        assert!(head("/nothing-here".into()).await.starts_with("http/1.1 404"));
        assert!(head("/app.js".into()).await.starts_with("http/1.1 404"), "the old page's script is gone");
        if has_phone_client() {
            let worker = head("/sw.js".into()).await;
            assert!(worker.starts_with("http/1.1 200") && worker.contains("text/javascript") && worker.contains("no-store"), "{worker}");
            let (path, _, _) = PHONE_ASSETS.iter().find(|(p, ty, _)| p.starts_with("/assets/") && ty.starts_with("text/javascript")).unwrap();
            let asset = head(path.to_string()).await;
            assert!(asset.starts_with("http/1.1 200") && asset.contains("immutable"), "{asset}");
        }
        // Only reading: a POST to the page, and a GET of the pairing endpoint, are refused.
        let mut http = network.http().await;
        http.write_all(format!("POST / HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", network.addr).as_bytes()).await.unwrap();
        assert!(http_head(&mut http).await.starts_with("HTTP/1.1 405"));
        // The token-based endpoints are gone: pairing and sessions go through kelpie-link on /link only.
        assert!(head("/pair".into()).await.starts_with("http/1.1 404"));
        assert!(head("/ws".into()).await.starts_with("http/1.1 404"));
    }

    fn code(reply: &Value) -> Option<&str> {
        reply["code"].as_str()
    }

    #[tokio::test]
    async fn every_verb_is_refused_below_its_permission_and_unknown_ones_always() {
        let mut r = rig();
        let requests = [
            json!({"type": "list"}),
            json!({"type": "hb"}),
            json!({"type": "ack", "id": "t1", "rev": 1}),
            json!({"type": "sub", "id": "t1"}),
            json!({"type": "unsub", "id": "t1"}),
            json!({"type": "chat_sub", "id": "c1"}),
            json!({"type": "input", "id": "t1", "text": "hi"}),
            json!({"type": "key", "id": "t1", "key": "enter"}),
            json!({"type": "answer", "id": "t1", "waiting_since": 1, "text": "1"}),
            json!({"type": "chat_answer", "id": "c1", "request_id": "r", "line": "{}"}),
            json!({"type": "chat_send", "id": "c1", "cid": "x", "text": "hi"}),
            json!({"type": "chat_cancel", "id": "c1", "qid": "q1"}),
            json!({"type": "chat_stop", "id": "c1"}),
            json!({"type": "chat_resume", "id": "c1"}),
        ];
        for req in &requests {
            let name = req["type"].as_str().unwrap();
            let reply = r.ask(Permission::Read, req.clone()).await;
            let writes = Verb::parse(name).unwrap().needs() == Permission::Write;
            assert_eq!(code(&reply) == Some("read_only"), writes, "{name} as a reader: {reply}");
            assert!(r.typed.writes().is_empty(), "{name}: a reader typed something");
            assert_ne!(code(&reply), Some("unknown_verb"), "{name}");
        }
        for name in ["session.input", "session.update", "session.attach", "session.close", "mobile.revoke", "read", "daemon.shutdown", ""] {
            for granted in [Permission::Read, Permission::Write] {
                let reply = r.ask(granted, json!({"type": name, "id": "t1", "handle": 1, "data": "x"})).await;
                assert_eq!((reply["type"].as_str(), code(&reply)), (Some("error"), Some("unknown_verb")), "{name}");
            }
        }
        assert!(r.typed.writes().is_empty());
        // A request without a type is not answered at all.
        assert!(r.conn.handle(Permission::Write, &json!({"id": "t1"})).await.is_none());
    }

    #[tokio::test]
    async fn the_device_record_decides_not_the_connection() {
        let r = rig();
        assert_eq!(r.daemon.mobile.permission("phone"), Some(Permission::Write));
        r.daemon.mobile.set_write("phone", false).unwrap();
        assert_eq!(r.daemon.mobile.permission("phone"), Some(Permission::Read));
        r.daemon.mobile.revoke("phone").unwrap();
        assert_eq!(r.daemon.mobile.permission("phone"), None);
    }

    #[tokio::test]
    async fn typing_from_the_phone_is_cleaned_and_goes_through_the_agents_enter_delay() {
        let mut r = rig();
        let began = Instant::now();
        let reply = r.ask_write(json!({"type": "input", "id": "t1", "text": "a\u{1b}[201~b", "enter": true})).await;
        assert_eq!(reply["type"], "ack");
        assert!(began.elapsed() >= crate::session::ENTER_DELAY, "the reply waits for the Enter");
        assert_eq!(r.typed.writes(), vec![b"a[201~b".to_vec(), b"\r".to_vec()], "the paste break-out is gone, and Enter is a write of its own");
        // Limits and the session's state.
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "t1", "text": "x".repeat(MAX_TYPED + 1)})).await), Some("too_long"));
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "nope", "text": "x"})).await), Some("no_session"));
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "c1", "text": "x"})).await), Some("chat"));
        assert_eq!(code(&r.ask_write(json!({"type": "key", "id": "t1", "key": "rm -rf"})).await), Some("bad_request"));
        assert_eq!(r.ask_write(json!({"type": "key", "id": "t1", "key": "ctrl+d"})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![vec![4u8]]);
        crate::session::fake::stop(&r.term);
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "t1", "text": "x"})).await), Some("exited"));
    }

    #[tokio::test]
    async fn text_is_refused_while_a_prompt_is_shown_unless_forced() {
        let mut r = rig();
        crate::session::fake::show_prompt(&r.term);
        let reply = r.ask_write(json!({"type": "input", "id": "t1", "text": "hello", "enter": false})).await;
        assert_eq!(code(&reply), Some("prompt"), "{reply}");
        assert!(r.typed.writes().is_empty());
        assert_eq!(r.ask_write(json!({"type": "input", "id": "t1", "text": "hello", "force": true})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![b"hello".to_vec()]);
    }

    #[tokio::test]
    async fn an_answer_lands_once_and_never_after_the_desktop_answered() {
        let mut r = rig();
        let since = crate::session::fake::wait_on_user(&r.term);
        let answer = json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "1"});
        assert_eq!(r.ask_write(answer.clone()).await["type"], "ack");
        // The second tap of this device: a duplicate, not somebody else's answer.
        let second = r.ask_write(answer.clone()).await;
        assert_eq!((code(&second), second["result"].as_str(), second["reason"].as_str()), (Some("dup"), Some("dup"), Some("answered")));
        assert_eq!(r.typed.writes(), vec![b"1".to_vec()]);
        let ok = r.ask_write(json!({"type": "answer", "id": "t9", "waiting_since": 1, "text": "1"})).await;
        assert_eq!((ok["type"].as_str(), ok["result"].as_str()), (Some("error"), Some("refused")), "{ok}");

        // A hook wait the user answered at the keyboard stays `waiting` until the next hook.
        let (term2, typed2) = crate::session::fake::running(3, "t2", "claude");
        r.daemon.insert_for_test(Entry::Term(term2.clone()));
        let since = crate::session::fake::wait_on_user(&term2);
        term2.input(b"2".to_vec());
        let reply = r.ask_write(json!({"type": "answer", "id": "t2", "waiting_since": since, "text": "1"})).await;
        assert_eq!((code(&reply), reply["result"].as_str(), reply["answered_by"].as_str()), (Some("stale"), Some("stale"), Some("desktop")));
        assert!(reply["at"].as_u64().is_some_and(|at| at <= now_millis() && at > now_millis() - 10_000), "{reply}");
        assert_eq!(typed2.writes(), vec![b"2".to_vec()], "only the desktop's key reached the agent");

        // Another phone answered first: the tap is told whose answer stood.
        let (term4, typed4) = crate::session::fake::running(5, "t4", "claude");
        r.daemon.insert_for_test(Entry::Term(term4.clone()));
        let since = crate::session::fake::wait_on_user(&term4);
        term4.answer_by(Some(since), Answer::Text("1"), false, By { id: "tablet".into(), name: "Tablet".into() }).unwrap();
        let reply = r.ask_write(json!({"type": "answer", "id": "t4", "waiting_since": since, "text": "2"})).await;
        assert_eq!((code(&reply), reply["result"].as_str(), reply["answered_by"].as_str()), (Some("stale"), Some("stale"), Some("Tablet")));
        assert_eq!(typed4.writes(), vec![b"1".to_vec()]);

        // A prompt that is not the one the phone showed.
        let (term3, typed3) = crate::session::fake::running(4, "t3", "claude");
        r.daemon.insert_for_test(Entry::Term(term3.clone()));
        let since = crate::session::fake::wait_on_user(&term3);
        let reply = r.ask_write(json!({"type": "answer", "id": "t3", "waiting_since": since - 1, "text": "1"})).await;
        assert_eq!(reply["reason"].as_str(), Some("moved"));
        assert_eq!(code(&r.ask_write(json!({"type": "answer", "id": "t3", "waiting_since": since, "key": "tab"})).await), Some("bad_request"));
        assert_eq!(code(&r.ask_write(json!({"type": "answer", "id": "t3", "waiting_since": since})).await), Some("bad_request"));
        assert!(typed3.writes().is_empty());
    }

    #[tokio::test]
    async fn requests_from_before_a_restart_are_dropped() {
        let mut r = rig();
        let epoch = r.daemon.mobile.epoch.clone();
        let stale = r.ask_write(json!({"type": "input", "id": "t1", "text": "x", "epoch": "an older daemon"})).await;
        assert_eq!(code(&stale), Some("epoch"));
        assert!(r.typed.writes().is_empty());
        assert_eq!(r.ask_write(json!({"type": "input", "id": "t1", "text": "x", "epoch": epoch})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![b"x".to_vec()]);
    }

    #[tokio::test]
    async fn a_chat_permission_is_answered_once_across_phone_and_desktop() {
        let mut r = rig();
        r.chat.ingest_for_test(json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool"}}).to_string());
        let line = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1", "response": {"behavior": "allow"}}}).to_string();
        let answer = json!({"type": "chat_answer", "id": "c1", "request_id": "r1", "line": line});
        // A request the agent never made cannot be answered, whatever the line.
        let unasked = json!({"type": "chat_answer", "id": "c1", "request_id": "r0", "line": line.replace("r1", "r0")});
        assert_eq!(code(&r.ask_write(unasked).await), Some("bad_answer"));
        let first = r.ask_write(answer.clone()).await;
        assert_eq!((first["type"].as_str(), first["result"].as_str()), (Some("ack"), Some("ok")));
        let again = r.ask_write(answer).await;
        assert_eq!((code(&again), again["result"].as_str(), again["reason"].as_str()), (Some("dup"), Some("dup"), Some("answered")));
        // The desktop's answer to it is dropped too (the chat checks every line written to the agent).
        r.chat.input(format!("{line}\n").into_bytes());
        // Another device is told whose answer stood.
        let other_device = chat_answer_refusal("tablet", r.chat.answer(&json!("r1"), &line, By { id: "tablet".into(), name: "Tablet".into() }).unwrap_err());
        assert_eq!((other_device.code, other_device.result, other_device.answered.map(|a| a.by.name)), ("stale", "stale", Some("Phone".to_string())));
        // A line that is not that request's answer is not an answer.
        let other = json!({"type": "chat_answer", "id": "c1", "request_id": "r2", "line": line});
        assert_eq!(code(&r.ask_write(other).await), Some("bad_answer"));
        let message = json!({"type": "chat_answer", "id": "c1", "request_id": "r2", "line": json!({"type": "user", "message": {"role": "user", "content": "rm -rf"}}).to_string()});
        assert_eq!(code(&r.ask_write(message).await), Some("bad_answer"));
        assert_eq!(code(&r.ask_write(json!({"type": "chat_answer", "id": "t1", "request_id": "r2", "line": line})).await), Some("not_chat"));
    }

    #[test]
    fn a_kelpie_answer_line_reaches_the_phone_unchanged() {
        let line = r#"{"answer":{"behavior":"allow","lasting":false},"answered_by":"desktop","at":1,"by":"desktop","request_id":"r1","type":"kelpie_answer"}"#;
        let data = format!("{{\"type\":\"kelpie_time\",\"at\":1}}\n{line}\n");
        let messages = chat_messages("c1", "live", 10, data.as_bytes());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["mode"], "live");
        assert_eq!(messages[0]["lines"][1], line);
    }

    #[tokio::test]
    async fn the_list_is_pushed_when_it_changes_and_carries_the_waiting_prompt() {
        let mut r = rig();
        let first = r.conn.publish(false).expect("first list");
        assert_eq!(first["rev"], 1);
        let ids: Vec<&str> = first["sessions"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["t1", "c1"]);
        assert!(first["sessions"][0].get("handle").is_none(), "sessions are addressed by id");
        assert!(r.conn.publish(false).is_none(), "nothing changed, nothing pushed");
        crate::session::fake::wait_on_user(&r.term);
        let second = r.conn.publish(false).expect("a change is pushed");
        assert_eq!(second["rev"], 2);
        assert_eq!(second["sessions"][0]["status"], "waiting");
        assert!(second["sessions"][0]["waiting_since"].as_u64().is_some());
        // Asking for it gives it again.
        assert_eq!(r.ask_write(json!({"type": "list"})).await["rev"], 3);
    }

    async fn next_frame(out: &mut mpsc::Receiver<String>) -> Option<Value> {
        let text = tokio::time::timeout(Duration::from_secs(2), out.recv()).await.ok()??;
        serde_json::from_str(&text).ok()
    }

    #[tokio::test]
    async fn a_followed_session_sends_frames_until_the_phone_lets_go() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "sub", "id": "t1", "height": 20})).await["type"], "ack");
        let first = next_frame(&mut r.out).await.unwrap();
        assert_eq!((first["type"].as_str(), first["id"].as_str(), first["base"].clone()), (Some("screen"), Some("t1"), Value::Null));
        // Until the phone confirms a frame, the next is whole again; after it, only what changed.
        assert_eq!(r.ask_write(json!({"type": "ack", "id": "t1", "rev": first["rev"]})).await["type"], "ack");
        r.term.ingest(b"hello\r\n");
        let next = next_frame(&mut r.out).await.unwrap();
        assert_eq!((next["base"].clone(), next["rows"].clone()), (first["rev"].clone(), json!([[0, "hello"]])));
        assert_eq!(code(&r.ask_write(json!({"type": "sub", "id": "c1"})).await), Some("chat"));
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "t1"})).await["type"], "ack");
        tokio::time::sleep(Duration::from_millis(150)).await;
        while r.out.try_recv().is_ok() {}
        r.term.ingest(b"more\r\n");
        assert!(tokio::time::timeout(Duration::from_millis(400), r.out.recv()).await.is_err(), "nothing for a session nobody follows");
        // A phone cannot follow more than it can show.
        for i in 0..MAX_FOLLOWED {
            let (s, _typed) = crate::session::fake::running(10 + i as u32, &format!("many-{i}"), "shell");
            r.daemon.insert_for_test(Entry::Term(s));
            let reply = r.ask_write(json!({"type": "sub", "id": format!("many-{i}")})).await;
            assert_eq!(reply["type"], "ack", "{i}");
        }
        assert_eq!(code(&r.ask_write(json!({"type": "sub", "id": "t1"})).await), Some("too_many"));
        r.conn.end();
    }

    #[tokio::test]
    async fn the_daemons_events_reach_the_phone_as_messages() {
        let r = rig();
        let dirty = Notify::new();
        let mut t = Translator::default();
        let event = |name: &str, params: Value| crate::session::json_out(&json!({"event": name, "params": params})).data;
        for name in ["session.updated", "session.closed"] {
            assert!(t.translate(&r.daemon, &event(name, json!({})), &dirty).is_empty());
            tokio::time::timeout(Duration::from_millis(50), dirty.notified()).await.unwrap_or_else(|_| panic!("{name} did not wake the list"));
        }
        assert!(t.translate(&r.daemon, &event("notify.event", json!({"kind": "waiting"})), &dirty).is_empty());
        assert!(tokio::time::timeout(Duration::from_millis(50), dirty.notified()).await.is_err(), "other events are not about the list");

        let resolved = t.translate(&r.daemon, &event("chat.resolved", json!({"handle": 2, "id": "c1", "request_id": "r1", "by": "phone", "dropped": false, "answered_by": "Pixel", "at": 5})), &dirty);
        assert_eq!(resolved, vec![json!({"type": "chat_resolved", "id": "c1", "request_id": "r1", "by": "phone", "dropped": false, "answered_by": "Pixel", "at": 5})]);
        let resync = t.translate(&r.daemon, &event("session.resync", json!({"handle": 2})), &dirty);
        assert_eq!(resync, vec![json!({"type": "chat_resync", "id": "c1"})]);

        // Log lines of a watched chat, live: addressed by id, not handle.
        let frame = kelpie_proto::Frame::Output { handle: 2, seq: 40, data: b"{\"a\":1}\n{\"b\":2}\n".to_vec() }.encode();
        assert_eq!(t.translate(&r.daemon, &frame, &dirty), vec![json!({"type": "chat", "id": "c1", "mode": "live", "seq": 40, "lines": ["{\"a\":1}", "{\"b\":2}"]})]);
        // The reply to an attach announces the log, which follows as the next output frame of that chat.
        let reply = crate::session::json_out(&json!({"id": "chat:c1", "result": {"mode": "delta", "raw": true, "seq": 9, "handle": 2, "observer": true}})).data;
        assert!(t.translate(&r.daemon, &reply, &dirty).is_empty());
        let log = kelpie_proto::Frame::Output { handle: 2, seq: 9, data: b"{\"a\":1}\n".to_vec() }.encode();
        assert_eq!(t.translate(&r.daemon, &log, &dirty), vec![json!({"type": "chat", "id": "c1", "mode": "delta", "seq": 9, "lines": ["{\"a\":1}"]})]);
        // After that it is live again.
        assert_eq!(t.translate(&r.daemon, &frame, &dirty)[0]["mode"], "live");
        // Replies to anything else are none of the phone's business.
        let other = crate::session::json_out(&json!({"id": 5, "result": true})).data;
        assert!(t.translate(&r.daemon, &other, &dirty).is_empty());
    }

    /// The phone is told it was removed in a message only the computer can make, and the close follows.
    async fn told_revoked(p: &mut TestPhone) {
        loop {
            match phone_recv(p).await {
                Some(m) if m["type"] == "revoked" => break,
                Some(_) => continue,
                None => panic!("the connection ended with no sealed word (close code {:?})", p.closed),
            }
        }
        assert!(phone_recv(p).await.is_none());
        assert_eq!(p.closed, Some(NOT_PAIRED));
    }

    /// Everything until the connection ends, and whether it held the sealed word.
    async fn until_closed(p: &mut TestPhone) -> bool {
        let mut revoked = false;
        while let Some(m) = phone_recv(p).await {
            revoked |= m["type"] == "revoked";
        }
        revoked
    }

    #[tokio::test]
    async fn turning_phone_access_off_keeps_the_pairing_and_the_kill_switch_and_a_reset_end_it() {
        let mut network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        // Off: the connection ends, and nothing says the device is gone.
        d.mobile.disable();
        assert!(!until_closed(&mut phone).await, "no sealed word");
        assert_ne!(phone.closed, Some(NOT_PAIRED));
        assert!(!d.mobile.removed("phone"), "the record is there");
        assert_eq!(d.mobile.permission("phone"), None, "and it may do nothing meanwhile");
        // A request that races the switch finds the record there: connect again, not "you are gone".
        network.restart().await;
        let mut back = network.websocket().await;
        authenticate(&mut back).await;
        // The kill switch: the record is gone, and the phone hears it inside its session.
        let (tx, _rx) = crate::session::client_channel();
        d.dispatch(1, &tx, &json!(1), "mobile.revoke_all", json!({})).unwrap();
        told_revoked(&mut back).await;
        assert!(d.mobile.removed("phone"));
        // A reset too.
        let keys = kelpie_link::Keypair::generate().unwrap();
        d.mobile.add_test_device("again", "Again", keys.public, Permission::Read, Kind::Pwa);
        network.restart().await;
        let mut third = network.socket_as(keys, false).await;
        authenticate(&mut third).await;
        d.mobile.reset();
        told_revoked(&mut third).await;
    }

    #[tokio::test]
    async fn a_stranger_hears_not_paired_and_a_record_that_cannot_be_trusted_is_told_apart() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let mut stranger = network.socket_as(kelpie_link::Keypair::generate().unwrap(), false).await;
        assert!(stranger.reconnect().await.is_none());
        assert_eq!(stranger.closed, Some(NOT_PAIRED));
        assert_eq!(d.mobile.failures.lock().values().map(VecDeque::len).sum::<usize>(), 1, "a failed attempt");
        // The record's MAC fails: known, not let in, and no failed attempt (such a phone tries again).
        d.mobile.config.lock().devices[0].permission = Permission::Control;
        d.mobile.config.lock().devices[0].verified = false;
        let mut edited = network.websocket().await;
        assert!(edited.reconnect().await.is_none());
        assert_eq!(edited.closed, Some(UNCONFIRMED));
        assert_eq!(d.mobile.failures.lock().values().map(VecDeque::len).sum::<usize>(), 1, "not counted");
        assert_eq!(d.mobile.status(false)["devices"][0]["state"], "unverified");
    }

    /// A credential store that can be made to fail. It never touches the OS.
    #[derive(Default)]
    struct FlakySecrets {
        values: Mutex<HashMap<String, String>>,
        failing: AtomicBool,
        writes: AtomicUsize,
    }

    impl crate::notify::SecretStore for FlakySecrets {
        fn get(&self, key: &str) -> Option<String> {
            self.read(key).ok().flatten()
        }
        fn read(&self, key: &str) -> Result<Option<String>> {
            if self.failing.load(Ordering::SeqCst) {
                bail!("the store is not answering");
            }
            Ok(self.values.lock().get(key).cloned())
        }
        fn set(&self, key: &str, value: &str) -> Result<()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            self.values.lock().insert(key.into(), value.into());
            Ok(())
        }
        fn delete(&self, key: &str) -> Result<()> {
            self.values.lock().remove(key);
            Ok(())
        }
    }

    #[test]
    fn a_failed_or_garbled_key_read_never_makes_a_new_key() {
        let dir = std::env::temp_dir().join(format!("kelpie-keys-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let secrets = Arc::new(FlakySecrets::default());
        let m = Mobile::load(&dir, secrets.clone());
        // The store says it has none: made and kept.
        let link = m.link_key().unwrap();
        let mac = m.mac_key(true).unwrap().unwrap();
        m.add_test_device("phone", "Phone", [4u8; 32], Permission::Write, Kind::Pwa);
        m.config.lock().enabled = true;
        m.keep();
        let writes = secrets.writes.load(Ordering::SeqCst);
        assert_eq!(writes, 2);

        // The daemon starts and the store does not answer: an error, nothing written, every record unchecked and none lost.
        secrets.failing.store(true, Ordering::SeqCst);
        let m = Mobile::load(&dir, secrets.clone());
        assert!(m.link_key().is_err());
        assert!(m.mac_key(true).is_err());
        assert!(m.set_permission("phone", Permission::Read).is_err());
        assert_eq!(secrets.writes.load(Ordering::SeqCst), writes, "nothing is written over the keys");
        assert_eq!(m.status(false)["devices"][0]["state"], "unchecked");
        assert_eq!(m.permission("phone"), None, "no access meanwhile");
        assert!(matches!(m.lookup_key(&[4u8; 32]), KeyLookup::Unconfirmed));
        assert!(!m.removed("phone"), "and the pairing is still there");

        // It answers again: the same keys, and the records are verified without a restart.
        secrets.failing.store(false, Ordering::SeqCst);
        assert!(matches!(m.lookup_key(&[4u8; 32]), KeyLookup::Paired(_)));
        assert_eq!(m.status(false)["devices"][0]["state"], "ok");
        assert_eq!(m.permission("phone"), Some(Permission::Write));
        assert_eq!(m.link_key().unwrap().public, link.public);
        assert_eq!(m.mac_key(false).unwrap(), Some(mac));
        assert_eq!(secrets.writes.load(Ordering::SeqCst), writes);

        // A value that is no key is an error too, and stays for the person to reset.
        for key in [K_LINK, K_MAC] {
            secrets.values.lock().insert(key.into(), "garbled".into());
        }
        let m = Mobile::load(&dir, secrets.clone());
        assert!(m.link_key().unwrap_err().to_string().contains("damaged"));
        assert!(m.mac_key(true).unwrap_err().to_string().contains("damaged"));
        assert_eq!(secrets.values.lock().get(K_MAC).map(String::as_str), Some("garbled"));
        assert_eq!(secrets.writes.load(Ordering::SeqCst), writes);
        // A reset is the way out.
        m.reset();
        assert!(m.link_key().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The connections' places as `accept` holds them: once they are all there, or within a few seconds.
    async fn places_taken(d: &Daemon, wanted: usize) {
        for _ in 0..500 {
            if d.mobile.handshakes.lock().all.len() == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{} places taken, not {wanted}", d.mobile.handshakes.lock().all.len());
    }

    /// A phone gets through at once, whatever else is connected.
    async fn phone_gets_through(network: &NetworkRig) {
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
    }

    /// Every place taken by connections that say nothing, then a byte of garbage from each just before their time is up, then all
    /// over again: nobody else is held up at any moment.
    #[tokio::test]
    async fn holding_every_place_gains_nothing_however_late_the_garbage_comes() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let began = Instant::now();
        let mut idle = Vec::new();
        for _ in 0..HANDSHAKES_PENDING {
            idle.push(network.websocket().await);
        }
        places_taken(&d, HANDSHAKES_PENDING).await;
        // A newcomer gets in, and the one that held its place longest is out.
        let mut newcomer = network.websocket().await;
        places_taken(&d, HANDSHAKES_PENDING).await;
        assert!(ws_recv(&mut idle[0].tcp).await.is_none(), "the oldest was let go");
        newcomer.reconnect().await.expect("answered with every place taken");
        phone_send(&mut newcomer, json!({})).await.unwrap();
        assert_eq!(phone_recv(&mut newcomer).await.unwrap()["type"], "hello");
        // One byte at the last moment: it fails the check, and that is all it gains.
        tokio::time::sleep(Duration::from_millis(4900).saturating_sub(began.elapsed())).await;
        for socket in &mut idle[1..] {
            let _ = ws_send(&mut socket.tcp, 2, &[7u8]).await;
        }
        for socket in &mut idle[1..] {
            assert!(ws_recv(&mut socket.tcp).await.is_none(), "over");
        }
        places_taken(&d, 0).await;
        phone_gets_through(&network).await;
        // And at once again.
        let mut again = Vec::new();
        for _ in 0..HANDSHAKES_PENDING {
            again.push(network.websocket().await);
        }
        places_taken(&d, HANDSHAKES_PENDING).await;
        phone_gets_through(&network).await;
    }

    /// The limit for an address, through the real connection path: a further connection is not served; garbage at the last moment
    /// frees its places and buys nothing more; silence until the time is up counts against the address.
    #[tokio::test]
    async fn one_address_holds_its_share_and_silence_counts_against_it() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        d.mobile.loopback_is_remote.store(true, Ordering::SeqCst);
        let began = Instant::now();
        let mut held = Vec::new();
        for _ in 0..HANDSHAKES_PENDING_PER_ADDRESS {
            held.push(network.websocket().await);
        }
        places_taken(&d, HANDSHAKES_PENDING_PER_ADDRESS).await;
        let mut one_too_many = network.websocket().await;
        assert!(ws_recv(&mut one_too_many.tcp).await.is_none(), "not served");
        assert_eq!(d.mobile.handshakes.lock().all.len(), HANDSHAKES_PENDING_PER_ADDRESS);
        tokio::time::sleep(Duration::from_millis(4900).saturating_sub(began.elapsed())).await;
        let on_time = began.elapsed() < Duration::from_millis(4950);
        for socket in &mut held {
            let _ = ws_send(&mut socket.tcp, 2, &[7u8]).await;
        }
        for socket in &mut held {
            assert!(ws_recv(&mut socket.tcp).await.is_none(), "over");
        }
        places_taken(&d, 0).await;
        // A byte of garbage is no failed attempt (unless a busy machine sent it after the time was up).
        if on_time {
            assert!(d.mobile.failures.lock().is_empty());
        }
        phone_gets_through(&network).await;
        let before = d.mobile.failures.lock().values().map(VecDeque::len).sum::<usize>();
        let _silent = network.websocket().await;
        places_taken(&d, 1).await;
        tokio::time::sleep(link::HANDSHAKE + Duration::from_millis(300)).await;
        places_taken(&d, 0).await;
        assert_eq!(d.mobile.failures.lock().values().map(VecDeque::len).sum::<usize>(), before + 1, "silence counts");
    }

    #[test]
    fn a_slow_disk_does_not_hold_up_what_reads_the_settings() {
        let r = rig();
        let writing = r.daemon.mobile.saving.lock();
        let d = r.daemon.clone();
        let save = std::thread::spawn(move || d.mobile.keep());
        std::thread::sleep(Duration::from_millis(50));
        assert!(r.daemon.mobile.config.try_lock().is_some(), "another save waiting holds nothing that readers need");
        assert_eq!(r.daemon.mobile.permission("phone"), Some(Permission::Write));
        r.daemon.mobile.config.lock().dormant_days = Some(11);
        drop(writing);
        save.join().unwrap();
        let on_disk: Value = serde_json::from_slice(&std::fs::read(store::path(&r.daemon.mobile.dir)).unwrap()).unwrap();
        assert_eq!(on_disk["dormant_days"], 11, "what is written is copied when its turn comes");
    }

    #[tokio::test]
    async fn turning_phone_access_on_and_off_is_in_the_audit_log() {
        let mut network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let (tx, _rx) = crate::session::client_channel();
        let actions = |d: &Daemon| d.mobile.audit_entries(50)["entries"].as_array().unwrap().iter().map(|e| e["action"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        d.dispatch(1, &tx, &json!(1), "mobile.disable", json!({})).unwrap();
        assert!(actions(&d).contains(&"phone_access_off".to_string()));
        assert!(!actions(&d).contains(&"phone_access_on".to_string()));
        d.dispatch(1, &tx, &json!(2), "mobile.disable", json!({})).unwrap();
        assert_eq!(actions(&d).iter().filter(|a| *a == "phone_access_off").count(), 1, "off once is off once");
        network.restart().await;
        assert_eq!(actions(&d).iter().filter(|a| *a == "phone_access_on").count(), 1);
        // A change of address while it is on turns nothing on.
        network.restart().await;
        assert_eq!(actions(&d).iter().filter(|a| *a == "phone_access_on").count(), 1);
    }

    #[tokio::test]
    async fn an_address_cannot_take_every_place_among_the_handshakes() {
        let r = rig();
        let m = &r.daemon.mobile;
        let (a, b): (IpAddr, IpAddr) = ("192.168.1.20".parse().unwrap(), "192.168.1.21".parse().unwrap());
        let held: Vec<_> = (0..HANDSHAKES_PENDING_PER_ADDRESS).map(|_| m.handshake_slot(a).unwrap()).collect();
        assert!(m.handshake_slot(a).is_none(), "its share is taken");
        assert_eq!(m.handshakes.lock().all.len(), HANDSHAKES_PENDING_PER_ADDRESS, "and the refusal holds no place");
        let other = m.handshake_slot(b).expect("another address is not held up by it");
        drop(held);
        assert!(m.handshake_slot(a).is_some(), "it gets them back");
        drop(other);
        assert_eq!(m.handshakes.lock().all.len(), 0);
        assert!(m.handshakes.lock().by_address.is_empty(), "and nothing is remembered of them");
        // This machine has no share. When every place is taken the oldest gives way to whoever comes next, from anywhere.
        let loopback: IpAddr = "127.0.0.1".parse().unwrap();
        let held: Vec<_> = (0..HANDSHAKES_PENDING).map(|_| m.handshake_slot(loopback).unwrap()).collect();
        let newcomer = m.handshake_slot(b).expect("a newcomer always gets in");
        assert_eq!(m.handshakes.lock().all.len(), HANDSHAKES_PENDING);
        assert!(within(Duration::from_secs(2), held[0].given_way()).await, "the oldest is told");
        // One whose first message passed gives way to nobody; with every place like that, there is no room.
        for slot in &held[1..] {
            slot.answered();
        }
        newcomer.answered();
        assert!(m.handshake_slot(b).is_none(), "no room, and nobody let go");
        assert!(!within(Duration::from_millis(50), held[2].given_way()).await);
        assert!(!within(Duration::from_millis(50), held[1].given_way()).await, "and only the oldest");
        drop(held);
        drop(newcomer);
        assert_eq!(m.handshakes.lock().all.len(), 0, "no place is lost, whoever gave way");
        assert!(m.handshakes.lock().by_address.is_empty());
        // Silence costs the address: it locks itself out, and the desktop hears of it.
        let (tx, mut events) = crate::session::client_channel();
        r.daemon.register_client(9, tx);
        r.daemon.set_client_role(9, &crate::auth::Role::App);
        for _ in 0..PAIR_FAILURES {
            link::held_silently(&r.daemon, a);
            link::held_silently(&r.daemon, loopback);
        }
        assert!(m.pairing_blocked(a));
        assert!(!m.pairing_blocked(loopback));
        let mut told = false;
        while let Ok(o) = events.try_recv() {
            told |= String::from_utf8_lossy(&o.data).contains("mobile.lockout");
        }
        assert!(told);
    }

    #[test]
    fn a_line_too_long_for_a_phone_becomes_a_notice_measured_as_it_goes_out() {
        let room = kelpie_link::MAX_APP_MESSAGE - 1024;
        let wrap = |line: &[u8]| {
            let mut data = line.to_vec();
            data.push(b'\n');
            chat_messages("c1", "live", 5_000_000, &data)
        };
        // Long, and fits: unchanged.
        let fits = "x".repeat(room - 2).into_bytes();
        assert_eq!(wrap(&fits)[0]["lines"][0].as_str().unwrap().len(), room - 2);
        // Longer than a message: a notice saying how long, at the same place in the log.
        let long = wrap(&"x".repeat(kelpie_link::MAX_APP_MESSAGE).into_bytes());
        assert!(long[0]["lines"][0].as_str().unwrap().contains(&kelpie_link::MAX_APP_MESSAGE.to_string()), "{}", long[0]["lines"][0]);
        assert_eq!(long[0]["seq"], 5_000_000);
        // Well under a message in the log, and well over once escaped: what counts is what goes out.
        let escapes = vec![1u8; room / 3];
        let wrapped = wrap(&escapes);
        assert!(wrapped[0]["lines"][0].as_str().unwrap().contains("kelpie_stderr"), "a line of {} bytes", escapes.len());
        for messages in [wrap(&fits), long, wrapped, wrap(&vec![b'"'; room / 2 - 10])] {
            for m in messages {
                assert!(m.to_string().len() <= kelpie_link::MAX_APP_MESSAGE, "{}", m.to_string().len());
            }
        }
    }

    #[tokio::test]
    async fn a_message_too_long_to_seal_is_dropped_and_the_session_carries_on() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        // The session list grows past what one message may carry.
        d.mobile.set_projects(&json!({"projects": {"big": "x".repeat(kelpie_link::MAX_APP_MESSAGE)}}));
        d.broadcast("session.updated", json!({"id": "t1"}));
        tokio::time::sleep(Duration::from_millis(100)).await;
        d.mobile.set_projects(&json!({"projects": {}}));
        phone_send(&mut phone, json!({"type": "hb", "n": 1})).await.unwrap();
        loop {
            let m = phone_recv(&mut phone).await.expect("the connection is still there");
            if m["type"] == "hb" {
                break;
            }
        }
        assert_eq!(phone.closed, None);
    }

    /// A phone that has sent a good first message keeps its place through any number of newcomers.
    #[tokio::test]
    async fn a_phone_in_the_middle_of_its_handshake_outlasts_any_number_of_newcomers() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let mut phone = network.websocket().await;
        phone.reconnect().await.expect("answered");
        let mut newcomers = Vec::new();
        for _ in 0..HANDSHAKES_PENDING {
            newcomers.push(network.websocket().await);
        }
        places_taken(&d, HANDSHAKES_PENDING).await;
        assert!(ws_recv(&mut newcomers[0].tcp).await.is_none(), "the first of them gave way to the last");
        phone_send(&mut phone, json!({})).await.unwrap();
        assert_eq!(phone_recv(&mut phone).await.unwrap()["type"], "hello", "the phone's own place was never at stake");
    }

    /// A connection let go before it said anything counts against its address, like one that stayed silent to the end.
    #[tokio::test]
    async fn a_connection_let_go_before_its_first_message_counts_against_its_address() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        d.mobile.loopback_is_remote.store(true, Ordering::SeqCst);
        let mut silent = network.websocket().await;
        places_taken(&d, 1).await;
        let others: Vec<_> = (0..HANDSHAKES_PENDING).map(|i| d.mobile.handshake_slot(IpAddr::from([10, 0, 0, (i / 4) as u8])).unwrap()).collect();
        assert!(ws_recv(&mut silent.tcp).await.is_none(), "let go");
        let at_it = d.mobile.failures.lock().get(&network.addr.ip()).map_or(0, VecDeque::len);
        assert_eq!(at_it, 1);
        drop(others);
    }

    /// One address's replays use up its own share of the handshakes, not everyone's.
    #[tokio::test]
    async fn a_replayed_first_message_from_one_address_leaves_the_budget_to_the_others() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        d.mobile.loopback_is_remote.store(true, Ordering::SeqCst);
        let (_, first) = kelpie_link::Initiator::reconnect(&network.rig.phone, &d.mobile.link_key().unwrap().public).unwrap();
        let mut replies = 0;
        for _ in 0..12 {
            let mut replay = network.websocket().await;
            ws_send(&mut replay.tcp, 2, &first).await.unwrap();
            replies += usize::from(ws_recv(&mut replay.tcp).await.is_some());
        }
        assert_eq!(replies, 1, "only the first time");
        let other: IpAddr = "192.168.1.30".parse().unwrap();
        assert!(d.mobile.handshake_allowed(other), "another address is not held up");
        // At once, by the clock: one address gets its share and no more, and what it leaves is the others'.
        let a: IpAddr = "192.168.1.31".parse().unwrap();
        d.mobile.handshake_budget.lock().tokens = HANDSHAKES_PER_SECOND;
        let granted = (0..50).filter(|_| d.mobile.handshake_allowed(a)).count();
        assert!((5..=6).contains(&granted), "{granted}");
        assert!(d.mobile.handshake_budget.lock().tokens >= HANDSHAKES_PER_SECOND - 6.0 - 1.0);
    }

    /// Sniffing a first message buys no place: its repeats are dropped before they could hold one, and count against the address.
    #[tokio::test]
    async fn a_sniffed_first_message_holds_no_place_and_counts_against_its_address() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        d.mobile.loopback_is_remote.store(true, Ordering::SeqCst);
        let (_, first) = kelpie_link::Initiator::reconnect(&network.rig.phone, &d.mobile.link_key().unwrap().public).unwrap();
        let mut honest = network.websocket().await;
        ws_send(&mut honest.tcp, 2, &first).await.unwrap();
        assert_eq!(ws_recv(&mut honest.tcp).await.unwrap().0, 2);
        places_taken(&d, 1).await;
        let at_it = |d: &Daemon| d.mobile.failures.lock().get(&network.addr.ip()).map_or(0, VecDeque::len);
        assert_eq!(at_it(&d), 0);
        for attempt in 1..=3 {
            let mut replay = network.websocket().await;
            ws_send(&mut replay.tcp, 2, &first).await.unwrap();
            assert!(ws_recv(&mut replay.tcp).await.is_none());
            assert_eq!(at_it(&d), attempt, "each counts");
            places_taken(&d, 1).await;
        }
        // One that passed the check and never finishes its handshake counts when it is over.
        drop(honest);
        places_taken(&d, 0).await;
        for _ in 0..200 {
            if at_it(&d) == 4 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(at_it(&d), 4, "it passed the check and said nothing more");
        // A phone's own retry is a new message.
        d.mobile.failures.lock().clear();
        phone_gets_through(&network).await;
        phone_gets_through(&network).await;
        assert_eq!(at_it(&d), 0);
    }

    /// With every place held by a connection that has passed the check, a newcomer is turned away with nobody let go, and that is recorded.
    #[tokio::test]
    async fn a_table_full_of_answered_places_turns_newcomers_away_and_says_so() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let held: Vec<_> = (0..HANDSHAKES_PENDING).map(|i| d.mobile.handshake_slot(IpAddr::from([10, 0, (i / 4) as u8, 1])).unwrap()).collect();
        for slot in &held {
            slot.answered();
        }
        for _ in 0..FLOOD_EVENTS {
            let mut newcomer = network.websocket().await;
            assert!(ws_recv(&mut newcomer.tcp).await.is_none(), "turned away");
        }
        assert_eq!(d.mobile.handshakes.lock().all.len(), HANDSHAKES_PENDING, "nobody was let go");
        let log = d.mobile.audit_entries(50);
        assert!(log["entries"].as_array().unwrap().iter().any(|e| e["action"] == "flood" && e["summary"].as_str().unwrap().contains("no room")), "{log}");
        drop(held);
        phone_gets_through(&network).await;
    }

    #[test]
    fn running_out_of_the_budget_is_recorded() {
        let r = rig();
        let m = &r.daemon.mobile;
        m.handshake_budget.lock().tokens = 0.0;
        for _ in 0..FLOOD_EVENTS {
            m.handshake_budget.lock().at = Instant::now() + Duration::from_secs(60);
            assert!(!m.handshake_allowed(IpAddr::from([127, 0, 0, 1])));
        }
        let log = m.audit_entries(50);
        assert!(log["entries"].as_array().unwrap().iter().any(|e| e["action"] == "flood" && e["summary"].as_str().unwrap().contains("budget")), "{log}");
    }

    #[test]
    fn a_retry_that_hangs_holds_up_no_other() {
        let r = rig();
        let m = &r.daemon.mobile;
        assert!(m.claim_retry());
        assert!(!m.claim_retry(), "one at a time");
        m.retry_ended();
        assert!(m.claim_retry());
    }

    #[tokio::test]
    async fn upgrades_are_limited_per_address_before_any_task_exists() {
        use tokio::io::AsyncWriteExt;
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        d.mobile.loopback_is_remote.store(true, Ordering::SeqCst);
        let upgrade = |network: &NetworkRig| format!("GET /link HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n", network.addr);
        let mut refused = 0;
        for _ in 0..(UPGRADE_BURST as usize + 15) {
            let mut tcp = network.http().await;
            tcp.write_all(upgrade(&network).as_bytes()).await.unwrap();
            if http_head(&mut tcp).await.starts_with("HTTP/1.1 429") {
                refused += 1;
            }
        }
        assert!(refused >= 1, "over the burst");
        // Another address has its own, and this machine is not held to it.
        assert!(d.mobile.upgrade_allowed("192.168.1.40".parse().unwrap()));
        d.mobile.loopback_is_remote.store(false, Ordering::SeqCst);
        assert!((0..100).all(|_| d.mobile.upgrade_allowed(network.addr.ip())));
    }

    #[test]
    fn a_flood_that_goes_on_is_in_the_audit_log_once_in_a_while() {
        let r = rig();
        let m = &r.daemon.mobile;
        let ip: IpAddr = "192.168.1.50".parse().unwrap();
        let held: Vec<_> = (0..HANDSHAKES_PENDING).map(|i| m.handshake_slot(IpAddr::from([10, 0, 1, i as u8])).unwrap()).collect();
        let noted = |m: &Mobile| m.audit_entries(50)["entries"].as_array().unwrap().iter().filter(|e| e["action"] == "flood").count();
        assert_eq!(noted(m), 0);
        let mut slots = Vec::new();
        for i in 0..(FLOOD_EVENTS as usize - 1) {
            slots.push(m.handshake_slot(IpAddr::from([10, 0, 2, i as u8])).unwrap());
        }
        assert_eq!(noted(m), 0, "a few are no flood");
        let flood: Vec<_> = (0..200).map(|i| m.handshake_slot(IpAddr::from([10, 0, 3, i as u8])).unwrap()).collect();
        assert_eq!(noted(m), 1, "and it is said once, not two hundred times");
        assert!(m.audit_entries(50)["entries"].as_array().unwrap().iter().any(|e| e["action"] == "flood" && e["summary"].as_str().unwrap().contains("10.0.3.")));
        drop((held, slots, flood, ip));
    }

    /// The store cannot be written (a folder is where the file goes): the kill switch says so, the warning stays until it can be, and then
    /// the file agrees, so a restart brings nothing back.
    #[tokio::test]
    async fn a_removal_that_cannot_be_saved_says_so_and_is_saved_when_it_can_be() {
        let r = rig();
        let d = r.daemon.clone();
        let file = store::path(&d.mobile.dir);
        let (tx, _rx) = crate::session::client_channel();
        d.mobile.keep();
        assert!(!d.mobile.save_pending());
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        let said = d.dispatch(1, &tx, &json!(1), "mobile.revoke_all", json!({})).unwrap_err().to_string();
        assert!(said.contains("could not be saved") && said.contains("come back after a restart"), "{said}");
        assert!(d.mobile.removed("phone"), "removed for now");
        assert!(d.mobile.save_pending());
        assert_eq!(d.mobile.status(false)["error"].as_str(), Some(NOT_SAVED_REMOVED));
        assert!(!d.mobile.retry_save(), "still");
        assert_eq!(d.mobile.status(false)["error"].as_str(), Some(NOT_SAVED_REMOVED));
        // What else goes right or wrong with phone access does not take the warning away.
        *d.mobile.last_error.lock() = None;
        assert_eq!(d.mobile.status(false)["error"].as_str(), Some(NOT_SAVED_REMOVED));
        *d.mobile.last_error.lock() = Some("port in use".into());
        assert_eq!(d.mobile.status(false)["error"].as_str(), Some(NOT_SAVED_REMOVED));
        *d.mobile.last_error.lock() = None;
        // The same for each way of taking access away.
        d.mobile.add_test_device("again", "Again", [9u8; 32], Permission::Read, Kind::Pwa);
        assert!(d.dispatch(1, &tx, &json!(2), "mobile.revoke", json!({"id": "again"})).unwrap_err().to_string().contains("could not be saved"));
        d.mobile.config.lock().enabled = true;
        assert!(d.dispatch(1, &tx, &json!(3), "mobile.disable", json!({})).unwrap_err().to_string().contains("Turned off for now"));
        // Lowering what a phone may do comes back at a restart just the same.
        d.mobile.add_test_device("lowered", "Lowered", [10u8; 32], Permission::Write, Kind::Pwa);
        d.set_client_role(1, &crate::auth::Role::App);
        let said = d.dispatch(1, &tx, &json!(4), "mobile.set_permission", json!({"id": "lowered", "permission": "read"})).unwrap_err().to_string();
        assert!(said.contains("old permission"), "{said}");
        assert_eq!(d.mobile.status(false)["error"].as_str(), Some(NOT_SAVED_OFF), "what comes back matters more, and stays");
        assert_eq!(d.mobile.config.lock().devices.iter().find(|x| x.id == "lowered").map(|x| x.permission), Some(Permission::Read), "lowered for now");
        d.mobile.revoke("lowered").unwrap();
        // It can be written again.
        std::fs::remove_dir(&file).unwrap();
        assert!(d.mobile.retry_save());
        assert!(!d.mobile.save_pending());
        assert_eq!(d.mobile.status(false)["error"], Value::Null);
        let on_disk: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!((on_disk["enabled"].as_bool(), on_disk["devices"].as_array().map(Vec::len)), (Some(false), Some(0)), "what a restart would load");
        let log = d.mobile.audit_entries(50);
        assert!(log["entries"].as_array().unwrap().iter().any(|e| e["action"] == "save_failed"), "{log}");
        assert_eq!(d.mobile.unsaved_text.lock().as_deref(), None, "said only until it is written");
    }

    /// Any save that gets through has written what was waiting too, so a later failure is not told as that old one. With nothing else
    /// waiting, the permission's own message is what the person reads.
    #[test]
    fn a_save_that_gets_through_ends_the_warning_of_the_one_before() {
        let r = rig();
        let m = &r.daemon.mobile;
        let file = store::path(&m.dir);
        m.keep();
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        m.keep();
        assert!(m.unsaved(NOT_SAVED_LOWERED).is_err());
        assert_eq!(m.status(false)["error"].as_str(), Some(NOT_SAVED_LOWERED));
        assert!(m.unsaved(NOT_SAVED_REMOVED).is_err());
        assert_eq!(m.status(false)["error"].as_str(), Some(NOT_SAVED_REMOVED), "the one that matters more takes over");
        std::fs::remove_dir(&file).unwrap();
        m.keep();
        assert!(!m.save_pending());
        assert_eq!(m.status(false)["error"], Value::Null);
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        m.keep();
        assert!(m.save_pending());
        assert_eq!(m.status(false)["error"], Value::Null, "a routine failure says nothing of a removal already on disk");
        std::fs::remove_dir(&file).unwrap();
    }

    /// A connection of the verified app, and what it is answered.
    fn app_client(d: &Arc<Daemon>, id: u64) -> (ClientTx, mpsc::UnboundedReceiver<Outgoing>) {
        d.set_client_role(id, &crate::auth::Role::App);
        crate::session::client_channel()
    }

    /// The reply to a request answered later.
    async fn later_reply(rx: &mut mpsc::UnboundedReceiver<Outgoing>) -> Value {
        let o = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.expect("a reply").expect("a reply");
        serde_json::from_slice(&o.data[5..]).unwrap()
    }

    /// Verifications of this daemon fail the test if they happen at all.
    fn no_prompt_expected(d: &Daemon) {
        d.presence.inject(|reason| panic!("no prompt was expected, and this one says: {reason}"));
    }

    /// Verifications that wait until the test lets them go; they count, and remember what they said.
    fn held_prompt(d: &Daemon, answer: Result<(), String>) -> (std::sync::mpsc::SyncSender<()>, Arc<Mutex<Vec<String>>>) {
        let (go, wait) = std::sync::mpsc::sync_channel::<()>(4);
        let wait = std::sync::Mutex::new(wait);
        let said = Arc::new(Mutex::new(Vec::new()));
        let told = said.clone();
        d.presence.inject(move |reason| {
            told.lock().push(reason.to_string());
            let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(10));
            answer.clone()
        });
        (go, said)
    }

    #[tokio::test]
    async fn the_person_verifies_only_after_every_other_check_and_nothing_is_added_before_or_without_it() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let (tx, mut replies) = app_client(&d, 50);
        let psk = pairing_code(&d);
        let keys = kelpie_link::Keypair::generate().unwrap();
        let mut phone = network.socket_as(keys, true).await;
        start_pairing(&mut phone, &psk, "Pixel\"\u{202E}").await.unwrap();
        let id = pending_request(&d).await;
        let shown = phone.session.as_ref().unwrap().pair_code();
        let wrong = if shown == "000000" { "111111" } else { "000000" };
        // Nothing wrong gets as far as a prompt: a wrong code counts and is answered at once, too much is refused at once.
        no_prompt_expected(&d);
        assert!(d.dispatch(50, &tx, &json!(1), "mobile.approve", json!({"id": "nobody", "allow": true, "code": shown})).is_err());
        assert!(d.dispatch(50, &tx, &json!(2), "mobile.approve", json!({"id": id, "allow": true, "code": wrong})).is_err());
        assert_eq!(d.mobile.pending.lock()[&id].tries, 1);

        // The right code: the call returns at once with the prompt up, so the app's other requests are not held up by it.
        let (go, said) = held_prompt(&d, Ok(()));
        let answer = json!({"id": id, "allow": true, "code": shown, "permission": "write", "window": 12345});
        assert_eq!(d.dispatch(50, &tx, &json!(3), "mobile.approve", answer.clone()).unwrap(), None, "answered later");
        assert_eq!(d.dispatch(50, &tx, &json!(4), "daemon.info", json!({})).unwrap().unwrap()["pid"], std::process::id());
        for _ in 0..500 {
            if !said.lock().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // What it says is the daemon's: what is asked, the phone's name in quotes it cannot end, its key.
        let text = said.lock()[0].clone();
        assert!(text.contains("\u{201C}Pixel\u{201D}") && text.contains(&fingerprint(&d.mobile.pending.lock()[&id].pk)) && text.contains("type"), "{text}");
        // One prompt at a time; nothing is added while it is up.
        let second = d.dispatch(50, &tx, &json!(5), "mobile.approve", answer.clone()).unwrap_err().to_string();
        assert!(second.contains("already"), "{second}");
        assert!(d.mobile.status(true)["devices"].as_array().unwrap().iter().all(|x| x["name"] == "Phone"), "only the rig's");
        assert_eq!(d.mobile.pending.lock().len(), 1);
        // The person passed: now the device is added, and the phone hears it.
        go.send(()).unwrap();
        let reply = later_reply(&mut replies).await;
        assert_eq!((reply["id"].clone(), reply["result"]["permission"].as_str()), (json!(3), Some("write")), "{reply}");
        assert!(d.mobile.status(true)["devices"].as_array().unwrap().iter().any(|x| x["name"].as_str().is_some_and(|n| n.starts_with("Pixel"))));
        assert_eq!(phone_recv(&mut phone).await.unwrap()["type"], "paired");
    }

    #[tokio::test]
    async fn a_failed_verification_adds_nothing_and_a_request_gone_meanwhile_is_not_brought_back() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let (tx, mut replies) = app_client(&d, 51);
        let psk = pairing_code(&d);
        let mut phone = network.socket_as(kelpie_link::Keypair::generate().unwrap(), true).await;
        start_pairing(&mut phone, &psk, "Declined").await.unwrap();
        let id = pending_request(&d).await;
        let shown = phone.session.as_ref().unwrap().pair_code();
        let answer = json!({"id": id, "allow": true, "code": shown});
        let devices = |d: &Daemon| d.mobile.status(true)["devices"].as_array().unwrap().len();
        let before = devices(&d);
        // The person cancels: an error for the app, and the request still waits for another try.
        d.presence.inject(|_| Err("Verification was cancelled.".into()));
        assert_eq!(d.dispatch(51, &tx, &json!(1), "mobile.approve", answer.clone()).unwrap(), None);
        let reply = later_reply(&mut replies).await;
        assert_eq!(reply["error"]["message"], "Verification was cancelled.", "{reply}");
        assert_eq!(devices(&d), before);
        assert!(d.mobile.pending.lock().contains_key(&id));
        // The person passes, but the request was declined while the prompt was up: nothing is added.
        let (go, _) = held_prompt(&d, Ok(()));
        assert_eq!(d.dispatch(51, &tx, &json!(2), "mobile.approve", answer.clone()).unwrap(), None);
        assert_eq!(d.dispatch(51, &tx, &json!(3), "mobile.approve", json!({"id": id, "allow": false})).unwrap(), Some(json!(false)), "declining needs nobody");
        go.send(()).unwrap();
        let reply = later_reply(&mut replies).await;
        assert!(reply["error"]["message"].as_str().unwrap().contains("no such request"), "{reply}");
        assert_eq!(devices(&d), before);
        assert_eq!(phone_recv(&mut phone).await.unwrap()["type"], "declined");
    }

    #[test]
    fn who_is_not_the_verified_app_never_raises_a_prompt() {
        let r = rig();
        let d = &r.daemon;
        no_prompt_expected(d);
        let (tx, _rx) = crate::session::client_channel();
        for role in [None, Some(crate::auth::Role::User)] {
            if let Some(role) = &role {
                d.set_client_role(60, role);
            }
            for (method, params) in [("mobile.reset", json!({})), ("mobile.set_permission", json!({"id": "phone", "permission": "control"})), ("mobile.set_options", json!({"dormant_days": 0}))] {
                let refused = d.dispatch(60, &tx, &json!(1), method, params.clone());
                assert!(refused.is_err(), "{method} {role:?}");
                assert!(refused.unwrap_err().to_string().contains("Kelpie app"), "{method}");
            }
        }
        assert_eq!(d.mobile.permission("phone"), Some(Permission::Write));
    }

    #[tokio::test]
    async fn one_reading_of_the_parameters_says_what_raises_access_and_what_is_applied() {
        let r = rig();
        let d = r.daemon.clone();
        let (tx, mut replies) = app_client(&d, 52);
        d.mobile.set_permission("phone", Permission::Read).unwrap();
        // Every way of asking for more goes to the person, and when they refuse nothing changes.
        d.presence.inject(|_| Err("Verification was cancelled.".into()));
        for params in [json!({"id": "phone", "permission": "write"}), json!({"id": "phone", "can_write": true}), json!({"id": "phone", "permission": "write", "can_write": false}), json!({"id": "phone", "permission": "control"})] {
            for method in ["mobile.set_write", "mobile.set_permission"] {
                d.presence.forget_refusals();
                assert_eq!(d.dispatch(52, &tx, &json!(1), method, params.clone()).unwrap(), None, "{method} {params} goes to the person");
                assert!(later_reply(&mut replies).await["error"].is_object(), "{method} {params}");
                assert_eq!(d.mobile.permission("phone"), Some(Permission::Read), "{method} {params}");
            }
        }
        // A permission that means nothing is refused, whichever way it is asked.
        assert!(d.dispatch(52, &tx, &json!(1), "mobile.set_write", json!({"id": "phone", "permission": "root"})).is_err());
        // They pass: it is what was asked.
        d.presence.inject(|reason| if reason.contains("type into them") && reason.contains("\u{201C}Phone\u{201D}") { Ok(()) } else { Err(reason.to_string()) });
        assert_eq!(d.dispatch(52, &tx, &json!(2), "mobile.set_write", json!({"id": "phone", "permission": "write"})).unwrap(), None);
        assert_eq!(later_reply(&mut replies).await["result"], true);
        assert_eq!(d.mobile.permission("phone"), Some(Permission::Write));
        // Lowering, or no change at all, needs nobody and is done before the call returns.
        no_prompt_expected(&d);
        assert_eq!(d.dispatch(52, &tx, &json!(3), "mobile.set_permission", json!({"id": "phone", "permission": "write"})).unwrap(), Some(json!(true)));
        assert_eq!(d.dispatch(52, &tx, &json!(4), "mobile.set_write", json!({"id": "phone", "can_write": false})).unwrap(), Some(json!(true)));
        assert_eq!(d.mobile.permission("phone"), Some(Permission::Read));
        // What cannot be applied is refused with no prompt.
        d.mobile.add_test_device("page", "Page", [8u8; 32], Permission::Read, Kind::Pwa);
        assert!(d.dispatch(52, &tx, &json!(5), "mobile.set_permission", json!({"id": "page", "permission": "control"})).is_err());
        assert!(d.dispatch(52, &tx, &json!(6), "mobile.set_permission", json!({"id": "nobody", "permission": "write"})).is_err());
    }

    #[tokio::test]
    async fn a_prompt_past_its_time_answers_an_error_frees_the_screen_and_its_late_pass_adds_nothing() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let (tx, mut replies) = app_client(&d, 56);
        let psk = pairing_code(&d);
        let mut phone = network.socket_as(kelpie_link::Keypair::generate().unwrap(), true).await;
        start_pairing(&mut phone, &psk, "Slow").await.unwrap();
        let id = pending_request(&d).await;
        let shown = phone.session.as_ref().unwrap().pair_code();
        let devices = |d: &Daemon| d.mobile.status(true)["devices"].as_array().unwrap().len();
        let before = devices(&d);
        // A dialog nobody answers: it ends, and passes, only long after its time.
        let (go, _) = held_prompt(&d, Ok(()));
        d.presence.set_limit(Duration::from_millis(100));
        assert_eq!(d.dispatch(56, &tx, &json!(1), "mobile.approve", json!({"id": id, "allow": true, "code": shown})).unwrap(), None);
        let reply = later_reply(&mut replies).await;
        assert!(reply["error"]["message"].as_str().unwrap().contains("in time"), "{reply}");
        assert!(d.presence.begin().unwrap_err().contains("close the earlier"), "its dialog may still be up");
        go.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(d.presence.begin().is_ok(), "once it has left the screen the next may ask");
        assert_eq!(devices(&d), before, "the late pass adds nothing");
        assert!(d.mobile.pending.lock().contains_key(&id), "and the request still waits");
        assert!(replies.try_recv().is_err(), "nor is it answered twice");
        let log = d.mobile.audit_entries(10);
        assert!(log["entries"].as_array().unwrap().iter().any(|e| e["action"] == "verify_failed" && e["summary"].as_str().unwrap().contains("Slow")), "{log}");
    }

    #[tokio::test]
    async fn a_refused_verification_leaves_a_line_and_three_stop_the_prompts_for_a_while() {
        let r = rig();
        let d = r.daemon.clone();
        let (tx, mut replies) = app_client(&d, 57);
        d.mobile.set_permission("phone", Permission::Read).unwrap();
        d.presence.inject(|_| Err("Verification was cancelled.".into()));
        for i in 0..3 {
            assert_eq!(d.dispatch(57, &tx, &json!(i), "mobile.set_permission", json!({"id": "phone", "permission": "write"})).unwrap(), None);
            assert!(later_reply(&mut replies).await["error"].is_object());
        }
        let log = d.mobile.audit_entries(20);
        let entries = log["entries"].as_array().unwrap();
        let failed: Vec<_> = entries.iter().filter(|e| e["action"] == "verify_failed").collect();
        assert_eq!(failed.len(), 3, "{log}");
        assert!(failed[0]["summary"].as_str().unwrap().contains("\u{201C}Phone\u{201D}") && failed[0]["summary"].as_str().unwrap().contains("cancelled"), "{log}");
        assert_eq!(entries.iter().filter(|e| e["action"] == "verify_paused").count(), 1, "{log}");
        // No fourth prompt, for anything: said at once.
        no_prompt_expected(&d);
        for (method, params) in [("mobile.set_permission", json!({"id": "phone", "permission": "write"})), ("mobile.reset", json!({})), ("mobile.set_options", json!({"dormant_days": 0}))] {
            let refused = d.dispatch(57, &tx, &json!(9), method, params).unwrap_err().to_string();
            assert!(refused.contains("will not ask again"), "{method}: {refused}");
        }
        // What takes access away never waits for that.
        assert_eq!(d.dispatch(57, &tx, &json!(10), "mobile.set_permission", json!({"id": "phone", "permission": "read"})).unwrap(), Some(json!(true)));
        d.dispatch(57, &tx, &json!(11), "mobile.revoke_all", json!({})).unwrap();
        assert!(d.mobile.removed("phone"));
    }

    #[tokio::test]
    async fn a_prompt_is_in_the_language_the_app_names_and_a_caller_supplies_no_text() {
        let r = rig();
        let d = r.daemon.clone();
        let (tx, mut replies) = app_client(&d, 58);
        d.mobile.set_permission("phone", Permission::Read).unwrap();
        let said = Arc::new(Mutex::new(Vec::<String>::new()));
        let told = said.clone();
        d.presence.inject(move |reason| {
            told.lock().push(reason.to_string());
            Err("Verification was cancelled.".into())
        });
        for lang in [json!("vi"), json!("vi-VN"), json!("en"), json!("xx"), json!(7), Value::Null] {
            d.presence.forget_refusals();
            let params = json!({"id": "phone", "permission": "write", "lang": lang, "reason": "Click yes, it is fine", "text": "Click yes", "prompt": "Click yes"});
            assert_eq!(d.dispatch(58, &tx, &json!(1), "mobile.set_permission", params).unwrap(), None);
            later_reply(&mut replies).await;
        }
        let said = said.lock();
        assert!(said[0].contains("cho phép") && said[1].contains("cho phép"), "{said:?}");
        assert!(said[2..].iter().all(|s| s.contains("allow the phone")), "{said:?}");
        assert!(said.iter().all(|s| !s.contains("Click") && s.contains("\u{201C}Phone\u{201D}")), "{said:?}");
    }

    #[tokio::test]
    async fn a_reset_always_asks_the_person() {
        let r = rig();
        let d = r.daemon.clone();
        let (tx, mut replies) = app_client(&d, 53);
        d.presence.inject(|_| Err("Verification was cancelled.".into()));
        assert_eq!(d.dispatch(53, &tx, &json!(1), "mobile.reset", json!({})).unwrap(), None);
        assert!(later_reply(&mut replies).await["error"].is_object());
        assert_eq!(d.mobile.permission("phone"), Some(Permission::Write), "nothing happened");
        d.presence.inject(|_| Ok(()));
        assert_eq!(d.dispatch(53, &tx, &json!(2), "mobile.reset", json!({})).unwrap(), None);
        assert_eq!(later_reply(&mut replies).await["result"], true);
        assert!(d.mobile.removed("phone"));
    }

    #[tokio::test]
    async fn keeping_unused_phones_longer_is_for_the_person_and_shortening_is_for_anyone() {
        let r = rig();
        let d = r.daemon.clone();
        let (tx, mut replies) = app_client(&d, 54);
        let (user_tx, _user_rx) = crate::session::client_channel();
        d.set_client_role(55, &crate::auth::Role::User);
        let days = |d: &Daemon| d.mobile.status(false)["dormant_days"].as_u64().unwrap();
        assert_eq!(days(&d), 90);
        // The user token: shorter is fine, and needs nobody; longer or never is refused with no prompt.
        no_prompt_expected(&d);
        assert_eq!(d.dispatch(55, &user_tx, &json!(1), "mobile.set_options", json!({"dormant_days": 30})).unwrap().unwrap()["dormant_days"], 30);
        assert_eq!(days(&d), 30);
        for longer in [31, 0] {
            assert!(d.dispatch(55, &user_tx, &json!(2), "mobile.set_options", json!({"dormant_days": longer})).unwrap_err().to_string().contains("Kelpie app"));
            assert_eq!(days(&d), 30);
        }
        assert!(d.dispatch(55, &user_tx, &json!(3), "mobile.set_options", json!({"dormant_days": "soon"})).is_err());
        // The app: the person decides, and refusing leaves it.
        d.presence.inject(|_| Err("Verification was cancelled.".into()));
        assert_eq!(d.dispatch(54, &tx, &json!(4), "mobile.set_options", json!({"dormant_days": 0})).unwrap(), None);
        assert!(later_reply(&mut replies).await["error"].is_object());
        assert_eq!(days(&d), 30);
        d.presence.inject(|reason| if reason.contains("for ever") { Ok(()) } else { Err(reason.to_string()) });
        assert_eq!(d.dispatch(54, &tx, &json!(5), "mobile.set_options", json!({"dormant_days": 0})).unwrap(), None);
        assert_eq!(later_reply(&mut replies).await["result"]["dormant_days"], 0);
        assert_eq!(days(&d), 0);
        // From never, any number of days is shorter.
        no_prompt_expected(&d);
        assert_eq!(d.dispatch(55, &user_tx, &json!(6), "mobile.set_options", json!({"dormant_days": 400})).unwrap().unwrap()["dormant_days"], 400);
    }

    #[tokio::test]
    async fn pairing_again_with_the_same_key_says_which_record_it_replaced() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let keys = kelpie_link::Keypair::generate().unwrap();
        let mut ids = Vec::new();
        for _ in 0..2 {
            let psk = pairing_code(&d);
            let mut phone = network.socket_as(keys.clone(), true).await;
            start_pairing(&mut phone, &psk, "Pixel").await.unwrap();
            let id = pending_request(&d).await;
            let shown = phone.session.as_ref().unwrap().pair_code();
            ids.push(d.mobile.approve(&json!({"id": id, "allow": true, "code": shown})).unwrap());
        }
        assert!(ids[0].replaced.is_empty());
        assert_eq!(ids[1].replaced, [ids[0].event.id.clone()], "its waiting messages go with it");
        // One record, under the new id; the key is still paired, so what used the old record is not told to forget it.
        let status = d.mobile.status(true);
        let records: Vec<&str> = status["devices"].as_array().unwrap().iter().filter(|x| x["name"] == "Pixel").filter_map(|x| x["id"].as_str()).collect();
        assert_eq!(records, [ids[1].event.id.as_str()]);
        assert!(d.mobile.removed(&ids[0].event.id) && d.mobile.key_paired(&kelpie_link::b64(&keys.public)));
    }

    #[test]
    fn a_connection_on_a_replaced_record_is_not_told_to_forget_the_pairing() {
        let event = |change| Ok(DeviceEvent { id: "phone".into(), name: "Pixel".into(), change, permission: None });
        assert_eq!(on_event(event("replaced"), "phone"), OnEvent::Close(REPLACED, "replaced"));
        assert_eq!(on_event(event("removed"), "phone"), OnEvent::Close(NOT_PAIRED, "revoked"));
        assert!(matches!(on_event(event("replaced"), "another"), OnEvent::Tell(_)), "other phones hear of it");
    }

    #[test]
    fn a_long_log_goes_out_in_messages_that_each_say_where_they_end() {
        let line = format!("{{\"text\":\"{}\"}}", "x".repeat(30 * 1024));
        let data = format!("{line}\n{line}\n{line}\n").into_bytes();
        let size = line.len() as u64 + 1;
        let messages = chat_messages("c1", "reset", 100_000, &data);
        // Two lines fit in a message, the third starts the next.
        assert_eq!(messages.iter().map(|m| m["lines"].as_array().unwrap().len()).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(messages.iter().map(|m| m["mode"].as_str().unwrap()).collect::<Vec<_>>(), ["reset", "more"]);
        assert_eq!(messages[0]["seq"].as_u64(), Some(100_000 - size));
        assert_eq!(messages[1]["seq"].as_u64(), Some(100_000));
        // A line larger than a message goes alone; nothing is lost or cut.
        let big = format!("{{\"text\":\"{}\"}}", "y".repeat(CHAT_MESSAGE_BYTES * 2));
        let messages = chat_messages("c1", "live", 5, format!("{big}\n").as_bytes());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["lines"][0].as_str().unwrap().len(), big.len());
        // No lines is still an answer: the phone knows it is up to date.
        let none = chat_messages("c1", "delta", 77, b"");
        assert_eq!((none.len(), none[0]["seq"].as_u64(), none[0]["lines"].as_array().unwrap().len()), (1, Some(77), 0));
    }

    #[tokio::test]
    async fn a_watched_chat_that_closed_is_let_go_and_does_not_fill_the_connection() {
        let mut r = rig();
        for i in 0..MAX_FOLLOWED {
            let chat = crate::chat::ChatSession::new(100 + i as u32, crate::session::SessionSpec { id: format!("gone-{i}"), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
            r.daemon.insert_for_test(Entry::Chat(chat));
            assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": format!("gone-{i}")})).await["type"], "ack", "{i}");
        }
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("too_many"));
        // They are closed on the computer; the phone lets go of one, and the others no longer count.
        for i in 0..MAX_FOLLOWED {
            r.daemon.remove_for_test(100 + i as u32);
        }
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "gone-0"})).await["type"], "ack");
        assert!(!r.conn.watched.contains("gone-0"));
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        assert_eq!(r.conn.watched.len(), 1, "{:?}", r.conn.watched);
        // Letting go of something that is not there is not an error either.
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "never-was"})).await["type"], "ack");
    }

    #[tokio::test]
    async fn asking_for_a_chat_again_at_once_is_refused() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("too_fast"));
        r.conn.chat_at.insert("c1".into(), Instant::now() - CHAT_SUB_GAP);
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
    }

    #[tokio::test]
    async fn two_messages_sent_at_once_are_two_submits() {
        let mut r = rig();
        r.ask_write(json!({"type": "input", "id": "t1", "text": "A", "enter": true})).await;
        r.ask_write(json!({"type": "input", "id": "t1", "text": "B", "enter": true})).await;
        // Each is followed by its own Enter before the next is typed.
        assert_eq!(r.typed.writes(), vec![b"A".to_vec(), b"\r".to_vec(), b"B".to_vec(), b"\r".to_vec()]);
    }

    #[tokio::test]
    async fn a_permission_change_or_a_missed_event_reconnects_and_only_a_removal_unpairs() {
        let r = rig();
        let mut events = r.daemon.mobile.revoked.subscribe();
        r.daemon.mobile.set_write("phone", false).unwrap();
        assert_eq!(on_event(events.recv().await, "phone"), OnEvent::Close(RECONNECT, "changed"), "still paired: the phone keeps its key");
        // Another device's change is told to this phone, not acted on.
        r.daemon.mobile.set_write("phone", true).unwrap();
        match on_event(events.recv().await, "other") {
            OnEvent::Tell(m) => assert_eq!((m["type"].as_str(), m["change"].as_str(), m["permission"].as_str(), m["name"].as_str()), (Some("device_changed"), Some("changed"), Some("write"), Some("Phone"))),
            other => panic!("{other:?}"),
        }
        // Falling behind may have missed a removal or a change: the record is looked at again.
        let (tx, mut slow) = broadcast::channel::<DeviceEvent>(2);
        for _ in 0..5 {
            tx.send(DeviceEvent { id: "x".into(), name: "x".into(), change: "changed", permission: None }).unwrap();
        }
        assert_eq!(on_event(slow.recv().await, "phone"), OnEvent::Recheck);
        // Only a removed device is unpaired.
        r.daemon.mobile.revoke("phone").unwrap();
        assert_eq!(on_event(events.recv().await, "phone"), OnEvent::Close(NOT_PAIRED, "revoked"));
        assert_ne!(NOT_PAIRED, RECONNECT);
        assert_ne!(RECONNECT, REPLACED);
        assert_eq!(([SILENT, SLOW_CONSUMER].iter().collect::<HashSet<_>>().len(), [NOT_PAIRED, RECONNECT, REPLACED].contains(&SILENT)), (2, false));
    }

    #[tokio::test]
    async fn a_close_that_cannot_leave_is_given_up_on() {
        assert!(!within(Duration::from_millis(50), std::future::pending::<()>()).await);
        assert!(within(Duration::from_millis(50), async {}).await);
        assert_eq!(CLOSE_TIMEOUT, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_message_whose_enter_was_skipped_says_so() {
        let mut r = rig();
        // Someone types at the keyboard during the delay: the Enter would submit their text too.
        let term = r.term.clone();
        let typist = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            term.input(b"x".to_vec());
        });
        let reply = r.ask_write(json!({"type": "input", "id": "t1", "text": "hello", "enter": true})).await;
        typist.await.unwrap();
        assert_eq!((reply["type"].as_str(), reply["entered"].clone(), reply["req"].clone()), (Some("ack"), json!(false), json!(1)));
        assert_eq!(r.typed.writes(), vec![b"hello".to_vec(), b"x".to_vec()], "no Enter followed");
    }

    #[tokio::test]
    async fn repeating_chat_sub_without_reading_is_refused_as_busy() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        r.conn.chat_at.clear();
        // The first log has not been taken by the socket (nothing drains the connection here).
        r.conn.waiting.store(crate::chat::RESET_WAITING_MAX, std::sync::atomic::Ordering::Relaxed);
        let reply = r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await;
        assert_eq!(code(&reply), Some("busy"), "{reply}");
        r.conn.waiting.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
    }


    /// A second device's connection to the same daemon.
    fn other_conn(r: &Rig, device: &str, name: &str, id: u64) -> Conn {
        let (tx, _rx, overflowed) = bounded_client_channel(8);
        let (out, _out_rx) = mpsc::channel(16);
        Conn {
            daemon: r.daemon.clone(), device: device.into(), name: name.into(), id, tx, out, followed: HashMap::new(), watched: HashSet::new(), owed: HashSet::new(),
            chat_at: HashMap::new(), waiting: Arc::new(AtomicUsize::new(0)), overflowed, cap: Permission::Control, list: None, rev: 0, legacy: false, _phone: None,
        }
    }

    async fn ask_as(conn: &mut Conn, mut req: Value) -> Value {
        req["req"] = json!(1);
        conn.handle(Permission::Write, &req).await.expect("a reply")
    }

    #[tokio::test]
    async fn an_answer_typed_through_input_or_key_belongs_to_the_device_that_sent_it() {
        let mut r = rig();
        let mut tablet = other_conn(&r, "tablet", "Tablet", 2);
        // The page answers with a key.
        let since = crate::session::fake::wait_on_user(&r.term);
        assert_eq!(r.ask_write(json!({"type": "key", "id": "t1", "key": "1"})).await["type"], "ack");
        let answer = json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "2"});
        let other = ask_as(&mut tablet, answer.clone()).await;
        assert_eq!((code(&other), other["result"].as_str(), other["answered_by"].as_str()), (Some("stale"), Some("stale"), Some("Phone")), "{other}");
        let same = r.ask_write(answer).await;
        assert_eq!((code(&same), same["result"].as_str()), (Some("dup"), Some("dup")), "{same}");
        assert_eq!(r.typed.writes(), vec![b"1".to_vec()], "nothing else reached the agent");

        // The page answers by typing text.
        let (term2, typed2) = crate::session::fake::running(3, "t2", "claude");
        r.daemon.insert_for_test(Entry::Term(term2.clone()));
        let since = crate::session::fake::wait_on_user(&term2);
        let typed = r.ask_write(json!({"type": "input", "id": "t2", "text": "1", "enter": false, "force": true})).await;
        assert_eq!(typed["type"], "ack");
        let answer = json!({"type": "answer", "id": "t2", "waiting_since": since, "text": "2"});
        let other = ask_as(&mut tablet, answer.clone()).await;
        assert_eq!((code(&other), other["answered_by"].as_str()), (Some("stale"), Some("Phone")), "{other}");
        let same = r.ask_write(answer).await;
        assert_eq!(code(&same), Some("dup"), "{same}");
        assert_eq!(typed2.writes(), vec![b"1".to_vec()]);
        // The desktop's own key is still the desktop's.
        let (term3, _typed3) = crate::session::fake::running(4, "t3", "claude");
        r.daemon.insert_for_test(Entry::Term(term3.clone()));
        let since = crate::session::fake::wait_on_user(&term3);
        term3.input(b"1".to_vec());
        let late = ask_as(&mut tablet, json!({"type": "answer", "id": "t3", "waiting_since": since, "text": "2"})).await;
        assert_eq!(late["answered_by"].as_str(), Some("desktop"), "{late}");
    }

    #[tokio::test]
    async fn letting_go_of_a_session_frees_its_place_in_the_resume_cache() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "sub", "id": "t1"})).await["type"], "ack");
        assert_eq!(r.daemon.mobile.views.slot_count(), 1);
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "t1"})).await["type"], "ack");
        assert_eq!(r.daemon.mobile.views.slot_count(), 0);
    }

    #[test]
    fn only_a_first_message_that_names_both_capabilities_is_not_a_page_from_before() {
        assert!(acknowledges(Some(&json!({"token": "t", "caps": ["ack", "hb"]}))));
        assert!(acknowledges(Some(&json!({"caps": ["hb", "x", "ack"]}))));
        for first in [json!({"token": "t"}), json!({"token": "t", "caps": ["ack"]}), json!({"caps": ["hb"]}), json!({"caps": "ack hb"})] {
            assert!(!acknowledges(Some(&first)), "{first}");
        }
        assert!(!acknowledges(None));
    }

    #[tokio::test]
    async fn a_page_that_says_nothing_of_acknowledgements_keeps_getting_frames() {
        let network = NetworkRig::new().await;
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await; // no `caps`: a page from before
        phone_send(&mut phone, json!({"type": "sub", "id": "t1", "height": 20, "req": 1})).await.unwrap();
        let mut previous = loop {
            let m = phone_recv(&mut phone).await.unwrap();
            if m["type"] == "screen" {
                break m["rev"].as_u64().unwrap();
            }
        };
        // Twice the window of frames, and not one acknowledged: each follows the one before.
        for i in 0..2 * sync::WINDOW_FRAMES {
            network.rig.term.ingest(format!("line {i}\r\n").as_bytes());
            let frame = loop {
                let m = phone_recv(&mut phone).await.unwrap();
                if m["type"] == "screen" {
                    break m;
                }
            };
            assert_eq!(frame["base"].as_u64(), Some(previous), "frame {i}");
            previous = frame["rev"].as_u64().unwrap();
        }
    }

    #[tokio::test]
    async fn a_request_resolved_is_never_told_ahead_of_the_chat_lines_it_is_about() {
        let network = NetworkRig::new().await;
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        phone_send(&mut phone, json!({"type": "chat_sub", "id": "c1", "req": 1})).await.unwrap();
        let (mut acked, mut log) = (false, false);
        while !(acked && log) {
            match phone_recv(&mut phone).await.unwrap()["type"].as_str() {
                Some("ack") => acked = true,
                Some("chat") => log = true,
                _ => {}
            }
        }
        // Nothing yields between the two, so the connection finds the line in the bulk lane and the
        // resolution in the bulk lane behind it at the same moment.
        let request = json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool"}}).to_string();
        network.rig.chat.ingest_for_test(request);
        let answer = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1", "response": {"behavior": "allow"}}});
        network.rig.chat.input(format!("{answer}\n").into_bytes());
        let mut seen = Vec::new();
        for _ in 0..6 {
            let m = phone_recv(&mut phone).await.unwrap();
            let kind = m["type"].as_str().unwrap_or_default().to_string();
            if kind == "chat" && m["lines"].to_string().contains("r1") {
                seen.push("line");
            } else if kind == "chat_resolved" {
                seen.push("resolved");
                break;
            }
        }
        assert_eq!(seen, ["line", "resolved"], "the request line comes first");
    }

    #[tokio::test]
    async fn a_revoked_device_is_cut_off_within_a_send_however_full_the_lanes_are_kept() {
        let network = NetworkRig::new().await;
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        phone_send(&mut phone, json!({"type": "sub", "id": "t1", "height": 20, "req": 1})).await.unwrap();
        phone_send(&mut phone, json!({"type": "chat_sub", "id": "c1", "req": 2})).await.unwrap();
        // A chat that never stops (asking and being answered every few lines, so `chat_resolved` keeps
        // coming) and a screen that never stops changing, towards a phone that reads slowly: the socket's
        // buffers fill, and the bulk lane is full whenever the writer looks at it.
        let (chat, term) = (network.rig.chat.clone(), network.rig.term.clone());
        let feeder = tokio::spawn(async move {
            let pad = "y".repeat(20_000);
            for i in 0..12_000 {
                chat.ingest_for_test(json!({"type": "assistant", "n": i, "pad": pad}).to_string());
                term.ingest(format!("line {i}\r\n").as_bytes());
                if i % 4 == 0 {
                    chat.ingest_for_test(json!({"type": "control_request", "request_id": format!("q{i}"), "request": {"subtype": "can_use_tool"}}).to_string());
                    let answer = json!({"type": "control_response", "response": {"subtype": "success", "request_id": format!("q{i}"), "response": {"behavior": "allow"}}});
                    chat.input(format!("{answer}\n").into_bytes());
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        });
        let began = Instant::now();
        let mut revoked_at = None;
        let mut read = 0;
        // The daemon's side of the connection is what is timed (the phone only sees the close after the
        // data in the socket's buffers): it must be gone within a close timeout of the revoke, though
        // the feeder runs for twenty-five seconds.
        while network.rig.daemon.mobile.connections.lock().len() == 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            if revoked_at.is_none() && began.elapsed() > Duration::from_millis(2500) {
                assert!(read > 10, "the phone was being sent a stream ({read} messages)");
                network.rig.daemon.mobile.revoke("phone").unwrap();
                revoked_at = Some(Instant::now());
            }
            match phone_recv(&mut phone).await {
                Some(_) => read += 1,
                None => break,
            }
            assert!(revoked_at.is_none_or(|at| at.elapsed() < CLOSE_TIMEOUT + Duration::from_secs(4)), "still connected {:?} after the revoke, with the lanes full", revoked_at.map(|at| at.elapsed()));
        }
        feeder.abort();
        assert!(revoked_at.is_some(), "the connection ended by itself");
    }

    #[tokio::test]
    async fn a_heartbeat_is_answered_with_the_time_and_the_load_and_an_answer_to_one_is_not() {
        let mut r = rig();
        let reply = r.ask(Permission::Read, json!({"type": "hb", "n": 7})).await;
        assert_eq!((reply["type"].as_str(), reply["echo"].clone(), reply["n"].clone(), reply["level"].clone()), (Some("hb"), json!(true), json!(7), json!(0)), "{reply}");
        assert!(reply["server_ts"].as_u64().is_some());
        r.daemon.mobile.load.set_for_test(Level::L2);
        assert_eq!(r.ask(Permission::Read, json!({"type": "hb"})).await["level"], 2);
        // The phone's answer to the daemon's own heartbeat needs no answer (it would be a loop).
        assert!(r.conn.handle(Permission::Read, &json!({"type": "hb", "echo": true})).await.is_none());
        let line = heartbeat(3, Level::L2);
        let beat: Value = serde_json::from_str(&line).unwrap();
        assert_eq!((beat["type"].as_str(), beat["n"].clone(), beat["level"].clone()), (Some("hb"), json!(3), json!(2)));
        assert!(beat.get("echo").is_none(), "only the answer says echo");
    }

    #[tokio::test]
    async fn a_chat_it_was_refused_is_asked_for_again_once_the_log_in_flight_has_been_taken() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        r.conn.chat_at.clear();
        r.conn.waiting.store(crate::chat::RESET_WAITING_MAX, Ordering::Relaxed);
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("busy"));
        assert!(r.conn.housekeeping().is_empty(), "nothing to say while the log is still on its way");
        r.conn.waiting.store(0, Ordering::Relaxed);
        assert_eq!(r.conn.housekeeping(), vec![json!({"type": "chat_resync", "id": "c1"})], "told to ask again, once");
        assert!(r.conn.housekeeping().is_empty());
    }

    #[tokio::test]
    async fn at_a_busy_load_only_a_few_chat_logs_are_on_their_way_at_once() {
        let mut r = rig();
        let others: Vec<Arc<AtomicUsize>> = (10..13).map(|id| {
            let waiting = Arc::new(AtomicUsize::new(1000));
            r.daemon.mobile.waiting.lock().insert(id, waiting.clone());
            waiting
        }).collect();
        // Calm: any number.
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        r.conn.chat_at.clear();
        // L2: two at most; three are on their way already.
        r.daemon.mobile.load.set_for_test(Level::L2);
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("busy"));
        others[0].store(0, Ordering::Relaxed);
        others[1].store(0, Ordering::Relaxed);
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack", "one other log is on its way: room for this one");
        // L3: one at most.
        r.conn.chat_at.clear();
        r.daemon.mobile.load.set_for_test(Level::L3);
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("busy"));
        // …and the phone is told when to ask again, as soon as the others have been taken.
        others[2].store(0, Ordering::Relaxed);
        assert_eq!(r.conn.housekeeping(), vec![json!({"type": "chat_resync", "id": "c1"})]);
    }

    #[tokio::test]
    async fn pairing_handshakes_are_spaced_out_when_the_machine_is_loaded() {
        let r = rig();
        let m = &r.daemon.mobile;
        assert!(!m.pairing_paced() && !m.pairing_paced(), "calm: no spacing");
        m.load.set_for_test(Level::L2);
        assert!(!m.pairing_paced(), "the first goes");
        assert!(m.pairing_paced(), "the next within a second waits");
        *m.paired_at.lock() = Some(Instant::now() - Duration::from_millis(1100));
        assert!(!m.pairing_paced());
        m.load.set_for_test(Level::L3);
        *m.paired_at.lock() = Some(Instant::now() - Duration::from_millis(1100));
        assert!(m.pairing_paced(), "five seconds at L3");
    }

    /// An attempt turned away because the machine is busy is turned away before the code is looked at: it
    /// cannot be used to test a guess, spends nothing and counts as no failure.
    #[tokio::test]
    async fn a_pairing_attempt_turned_away_for_load_never_looks_at_the_code() {
        let network = NetworkRig::new().await;
        let m = &network.rig.daemon.mobile;
        let psk = pairing_code(&network.rig.daemon);
        m.load.set_for_test(Level::L3);
        *m.paired_at.lock() = Some(Instant::now());
        for right in [true, false, false] {
            let mut p = network.socket_as(kelpie_link::Keypair::generate().unwrap(), true).await;
            let used = if right { psk } else { [9u8; 32] };
            assert!(start_pairing(&mut p, &used, "x").await.is_none());
            // Only who holds the code hears why; a wrong one is dropped before anything is looked at.
            assert_eq!(p.closed, right.then_some(link::BUSY), "right code: {right}");
        }
        assert_eq!(m.live_codes().len(), 1, "the code is not spent");
        assert!(m.failures.lock().is_empty(), "nothing is counted as a failed guess");
    }

    /// A pairing secret as `mobile.pair_start` makes one (for a browser page).
    fn pairing_code(daemon: &Daemon) -> [u8; 32] {
        let made = daemon.mobile.pair_start().unwrap();
        let url = made["url"].as_str().unwrap();
        assert!(url.starts_with("http://127.0.0.1:") && url.contains("/#p="), "{url}");
        let payload = kelpie_link::QrPayload::decode(url.split("#p=").nth(1).unwrap()).unwrap();
        assert_eq!(payload.dpk, daemon.mobile.link_key().unwrap().public);
        assert_eq!(payload.endpoints.len(), 1);
        assert_eq!(payload.endpoints[0].kind, "lan");
        payload.psk
    }

    /// A phone's pairing with `psk` up to the desktop being asked: its first message (carrying nothing), the reply, then its
    /// name in the first sealed message. None when the desktop did not reply (the code was not right).
    async fn start_pairing(p: &mut TestPhone, psk: &[u8; 32], name: &str) -> Option<()> {
        let (init, first) = kelpie_link::Initiator::pair(&p.keys, &p.desktop, psk).unwrap();
        p.handshake(first, init).await?;
        phone_send(p, json!({"type": "pair", "name": name})).await.ok()?;
        Some(())
    }

    /// The pairing request that is waiting (it comes up once the phone's sealed message has arrived).
    async fn pending_request(d: &Daemon) -> String {
        for _ in 0..500 {
            if let Some(id) = d.mobile.pending.lock().keys().next().cloned() {
                return id;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no pairing request came up");
    }

    /// The whole pairing: a wrong code asks nobody and spends nothing; the right one is spent, the desktop is asked, the
    /// typed code must be the one the phone shows; then the phone reconnects as a paired device, `read` and capped as a page.
    #[tokio::test]
    async fn a_phone_pairs_with_the_qr_secret_and_the_code_the_person_types() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let (tx, mut events) = crate::session::client_channel();
        d.register_client(77, tx);
        d.set_client_role(77, &crate::auth::Role::App);
        let psk = pairing_code(&d);
        // A wrong secret: dropped on its mac1, with no reply, no close code and no event; nobody is asked, nothing is counted, the
        // live code is still there.
        let mut wrong = network.socket_as(kelpie_link::Keypair::generate().unwrap(), true).await;
        assert!(start_pairing(&mut wrong, &[1u8; 32], "Mallory").await.is_none());
        assert_eq!(wrong.closed, None);
        assert!(d.mobile.pending.lock().is_empty() && d.mobile.live_codes().len() == 1);
        assert!(d.mobile.failures.lock().is_empty());
        while let Ok(o) = events.try_recv() {
            assert!(!String::from_utf8_lossy(&o.data).contains("mobile.lockout"));
        }
        // The right one.
        let keys = kelpie_link::Keypair::generate().unwrap();
        let mut phone = network.socket_as(keys.clone(), true).await;
        start_pairing(&mut phone, &psk, "Pixel").await.expect("the desktop replies");
        let shown = phone.session.as_ref().unwrap().pair_code();
        let id = pending_request(&d).await;
        assert!(d.mobile.live_codes().is_empty(), "single use");
        let mut asked = None;
        while let Ok(o) = events.try_recv() {
            let text = String::from_utf8_lossy(&o.data).into_owned();
            if text.contains("mobile.pair_request") {
                asked = Some(text);
            }
        }
        let asked = asked.expect("the app is asked once the phone finished the handshake");
        assert!(asked.contains(&fingerprint(&keys.public)) && asked.contains("\"want\":\"read\""), "{asked}");
        // A second phone with the spent code gets nowhere.
        let mut late = network.socket_as(kelpie_link::Keypair::generate().unwrap(), true).await;
        assert!(start_pairing(&mut late, &psk, "Late").await.is_none());
        // A wrong typed code is refused; the right one approves (a page gets `read` unless raised, never control).
        let other = if shown == "000000" { "111111" } else { "000000" };
        assert!(d.mobile.approve(&json!({"id": id, "allow": true, "code": other})).is_err());
        assert!(d.mobile.approve(&json!({"id": id, "allow": true, "code": shown, "permission": "control"})).is_err(), "a page never gets control");
        let paired = loop {
            let m = phone_recv(&mut phone).await;
            if m.is_some() || phone.closed.is_some() {
                break m;
            }
        };
        assert_eq!(paired.unwrap()["type"], "declined", "asking for too much declines");
        // Again, properly.
        let psk = pairing_code(&d);
        let mut phone = network.socket_as(keys.clone(), true).await;
        start_pairing(&mut phone, &psk, "Pixel").await.unwrap();
        let shown = phone.session.as_ref().unwrap().pair_code();
        let id = pending_request(&d).await;
        let approved = d.mobile.approve(&json!({"id": id, "allow": true, "code": format!(" {} ", shown)})).unwrap();
        assert_eq!(approved.event.change, "added");
        let paired = phone_recv(&mut phone).await.unwrap();
        assert_eq!((paired["type"].as_str(), paired["device"]["permission"].as_str(), paired["device"]["kind"].as_str()), (Some("paired"), Some("read"), Some("pwa")), "{paired}");
        let device = paired["device"]["id"].as_str().unwrap().to_string();
        assert_eq!(d.mobile.permission(&device), Some(Permission::Read));
        // It reconnects with its key, and may only read until raised.
        let mut again = network.socket_as(keys.clone(), true).await;
        again.reconnect().await.expect("a paired key");
        phone_send(&mut again, json!({"caps": ["ack", "hb"]})).await.unwrap();
        let hello = phone_recv(&mut again).await.unwrap();
        assert_eq!((hello["device"]["permission"].as_str(), hello["device"]["capped"].as_bool(), hello["device"]["can_write"].as_bool()), (Some("read"), Some(false), Some(false)), "{hello}");
        let addrs = phone_recv(&mut again).await.unwrap();
        assert_eq!((addrs["type"].as_str(), addrs["list"][0].as_str()), (Some("addrs"), Some(network.addr.to_string().as_str())));
        let typed = loop {
            phone_send(&mut again, json!({"type": "input", "id": "t1", "text": "x", "req": "w"})).await.unwrap();
            let m = phone_recv(&mut again).await.unwrap();
            if m["req"] == "w" {
                break m;
            }
        };
        assert_eq!(typed["code"], "read_only", "{typed}");
        assert!(network.rig.typed.writes().is_empty());
        // An unknown key is not answered.
        let mut stranger = network.socket_as(kelpie_link::Keypair::generate().unwrap(), false).await;
        assert!(stranger.reconnect().await.is_none());
        assert_eq!(stranger.closed, Some(NOT_PAIRED));
    }

    /// A captured first message can be sent again: it gets a reply nobody can read, and nothing else happens. A code answers one
    /// address a few of those at most. A phone that never finishes the handshake spends nothing and asks nobody, and until its
    /// first sealed frame a paired device is not seen, does not replace its connection and uses up none of its reconnects.
    #[tokio::test]
    async fn a_replayed_first_message_changes_nothing() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let psk = pairing_code(&d);
        let keys = kelpie_link::Keypair::generate().unwrap();
        let (_, first) = kelpie_link::Initiator::pair(&keys, &d.mobile.link_key().unwrap().public, &psk).unwrap();
        let mut p = network.socket_as(keys.clone(), true).await;
        ws_send(&mut p.tcp, 2, &first).await.unwrap();
        assert_eq!(ws_recv(&mut p.tcp).await.expect("a reply").0, 2);
        assert!(d.mobile.pending.lock().is_empty(), "nobody is asked");
        assert_eq!(d.mobile.live_codes().len(), 1, "nothing is spent");
        // The same message again is a replay: nothing answers it.
        for _ in 0..3 {
            let mut again = network.socket_as(keys.clone(), true).await;
            ws_send(&mut again.tcp, 2, &first).await.unwrap();
            assert!(ws_recv(&mut again.tcp).await.is_none(), "seen before: dropped");
        }
        // New ones from one address, as a phone's are: a code answers a few of them at most.
        let mut answered = 1;
        for _ in 1..=CODE_ATTEMPTS {
            let (_, fresh) = kelpie_link::Initiator::pair(&keys, &d.mobile.link_key().unwrap().public, &psk).unwrap();
            let mut p = network.socket_as(keys.clone(), true).await;
            ws_send(&mut p.tcp, 2, &fresh).await.unwrap();
            answered += usize::from(ws_recv(&mut p.tcp).await.is_some());
        }
        assert_eq!(answered, CODE_ATTEMPTS as usize, "enough from this address: dropped");
        assert_eq!(d.mobile.live_codes().len(), 1, "the code itself is untouched");
        let elsewhere: IpAddr = "192.168.1.77".parse().unwrap();
        assert!(d.mobile.code_attempt(&psk, elsewhere), "another address is not held up");
        // A replayed reconnect of a paired phone: answered once, then dropped.
        d.mobile.config.lock().devices[0].last_seen = 5;
        let (_, first) = kelpie_link::Initiator::reconnect(&network.rig.phone, &d.mobile.link_key().unwrap().public).unwrap();
        for attempt in 0..(RECONNECT_BURST + 1) {
            let mut p = network.websocket().await;
            ws_send(&mut p.tcp, 2, &first).await.unwrap();
            assert_eq!(ws_recv(&mut p.tcp).await.map(|f| f.0), (attempt == 0).then_some(2), "{attempt}");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(d.mobile.config.lock().devices[0].last_seen, 5);
        assert!(d.mobile.connections.lock().is_empty());
        let mut real = network.websocket().await;
        authenticate(&mut real).await;
        assert!(d.mobile.config.lock().devices[0].last_seen > 5, "the phone that finished is seen");
    }

    /// What a first message may cost: garbage costs a hash and uses up nothing; past the budget or the handshakes under way,
    /// first messages are dropped unanswered; a place is held until the first valid sealed frame.
    #[tokio::test]
    async fn the_handshake_budget_drops_silently_and_garbage_does_not_touch_it() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        // Garbage and another version: no reply, no close frame, the budget is as it was.
        let before = d.mobile.handshake_budget.lock().tokens;
        let mut noise = vec![kelpie_link::VERSION, kelpie_link::MODE_RECONNECT];
        noise.extend([7u8; 112]);
        let mut old = kelpie_link::Initiator::reconnect(&network.rig.phone, &d.mobile.link_key().unwrap().public).unwrap().1;
        old[0] = 0;
        for bytes in [noise, old] {
            let mut p = network.websocket().await;
            ws_send(&mut p.tcp, 2, &bytes).await.unwrap();
            assert!(ws_recv(&mut p.tcp).await.is_none(), "dropped");
        }
        assert!(d.mobile.handshake_budget.lock().tokens >= before, "not one Diffie-Hellman");
        assert!(d.mobile.failures.lock().is_empty(), "no event, nothing counted");
        // The budget is spent: a good first message is dropped too, and is answered once there is room again. Its clock is set
        // ahead, as a token comes back every 50 ms and a slow machine takes longer than that to get here.
        d.mobile.handshake_budget.lock().tokens = 0.0;
        d.mobile.handshake_budget.lock().at = Instant::now() + Duration::from_secs(60);
        let mut p = network.websocket().await;
        assert!(p.reconnect().await.is_none());
        assert_eq!(p.closed, None, "no close code");
        d.mobile.handshake_budget.lock().tokens = HANDSHAKES_PER_SECOND;
        let mut p = network.websocket().await;
        p.reconnect().await.expect("room again");
        // A place is held by a handshake that has not finished, and let go by its first sealed frame.
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(d.mobile.handshakes.lock().all.len(), 1);
        phone_send(&mut p, json!({})).await.unwrap();
        assert_eq!(phone_recv(&mut p).await.unwrap()["type"], "hello");
        assert_eq!(d.mobile.handshakes.lock().all.len(), 0);
        // The budget refills: 20 a second, no more than that held up.
        d.mobile.handshake_budget.lock().at = Instant::now() - Duration::from_secs(60);
        d.mobile.handshake_budget.lock().tokens = 0.0;
        let granted = (0..100).filter(|_| d.mobile.handshake_allowed(network.addr.ip())).count();
        assert!((20..=21).contains(&granted), "{granted}");
        assert_eq!(link::HANDSHAKE, Duration::from_secs(5));
    }

    /// Nothing on the phone's traffic is compressed: an offer of permessage-deflate is declined.
    #[tokio::test]
    async fn the_link_is_never_compressed() {
        use tokio::io::AsyncWriteExt;
        let network = NetworkRig::new().await;
        let mut tcp = network.http().await;
        tcp.write_all(format!(
            "GET /link HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\n\r\n",
            network.addr
        ).as_bytes()).await.unwrap();
        let head = http_head(&mut tcp).await.to_ascii_lowercase();
        assert!(head.starts_with("http/1.1 101") && !head.contains("sec-websocket-extensions"), "{head}");
    }

    /// Revoking cuts the device's live connection, whichever listener it came through, and the record decides from then on:
    /// a handshake it had already started finds nobody.
    #[tokio::test]
    async fn revoking_ends_a_live_session_and_one_being_made() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let mut live = network.websocket().await;
        authenticate(&mut live).await;
        // A second connection is half way: it has its reply, and has not said anything sealed yet.
        let mut halfway = network.websocket().await;
        halfway.reconnect().await.unwrap();
        d.mobile.revoke("phone").unwrap();
        told_revoked(&mut live).await;
        phone_send(&mut halfway, json!({})).await.unwrap();
        told_revoked(&mut halfway).await;
        assert!(d.mobile.connections.lock().is_empty() || d.mobile.permission("phone").is_none());
    }

    #[tokio::test]
    async fn three_wrong_codes_decline_the_request_and_the_app_hears_of_it() {
        let network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        let (tx, mut events) = crate::session::client_channel();
        d.register_client(78, tx);
        d.set_client_role(78, &crate::auth::Role::App);
        let psk = pairing_code(&d);
        let mut phone = network.socket_as(kelpie_link::Keypair::generate().unwrap(), true).await;
        start_pairing(&mut phone, &psk, "Pixel").await.unwrap();
        let id = pending_request(&d).await;
        let shown = phone.session.as_ref().unwrap().pair_code();
        let wrong = if shown == "000000" { "111111" } else { "000000" };
        let (reply_tx, _reply_rx) = crate::session::client_channel();
        for attempt in 1..=3 {
            let r = d.dispatch(78, &reply_tx, &json!(attempt), "mobile.approve", json!({"id": id, "allow": true, "code": wrong}));
            assert!(r.is_err(), "{attempt}");
            assert_eq!(d.mobile.pending.lock().is_empty(), attempt == 3, "{attempt}");
        }
        assert_eq!(phone_recv(&mut phone).await.unwrap()["type"], "declined");
        let mut told = false;
        while let Ok(o) = events.try_recv() {
            told |= String::from_utf8_lossy(&o.data).contains("mobile.lockout");
        }
        assert!(told, "the desktop hears of the lockout");
        // Even the right code does not bring it back.
        assert!(d.mobile.approve(&json!({"id": id, "allow": true, "code": shown})).is_err());
    }

    #[tokio::test]
    async fn the_kill_switch_stops_listening_and_removes_every_device_online_or_not() {
        let mut network = NetworkRig::new().await;
        let d = network.rig.daemon.clone();
        d.mobile.add_test_device("offline", "Tablet", [6u8; 32], Permission::Read, Kind::Pwa);
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        let screen = d.mobile.grant("phone");
        network.rig.ask_write(json!({"type": "list"})).await;
        let (tx, _rx) = crate::session::client_channel();
        d.dispatch(1, &tx, &json!(1), "mobile.revoke_all", json!({})).unwrap();
        assert!(!screen.load(Ordering::SeqCst), "control stops");
        told_revoked(&mut phone).await;
        let status = d.mobile.status(false);
        assert_eq!((status["enabled"].as_bool(), status["running"].as_bool(), status["devices"].as_array().unwrap().len()), (Some(false), Some(false), 0));
        assert!(tokio::net::TcpStream::connect(network.addr).await.is_err(), "nothing listens");
        // Turning it on again does not bring a device back.
        network.restart().await;
        let mut again = network.websocket().await;
        assert!(again.reconnect().await.is_none());
        assert_eq!(again.closed, Some(NOT_PAIRED));
        let log = d.mobile.audit_entries(10);
        assert!(log["entries"].as_array().unwrap().iter().any(|e| e["action"] == "revoke_all"), "{log}");
    }

    #[test]
    fn a_device_unused_for_too_long_must_be_paired_again() {
        let r = rig();
        let m = &r.daemon.mobile;
        let day = 24 * 3600 * 1000u64;
        let now = 400 * day;
        m.config.lock().devices[0].last_seen = now - 89 * day;
        m.add_test_device("old", "Old phone", [7u8; 32], Permission::Write, Kind::App);
        m.config.lock().devices[1].last_seen = now - 91 * day;
        let mut events = m.revoked.subscribe();
        let screen = m.grant("old");
        let gone = m.expire_dormant(now);
        assert_eq!(gone.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), ["old"]);
        assert!(!screen.load(Ordering::SeqCst));
        assert_eq!(events.try_recv().unwrap().id, "old");
        assert_eq!(m.permission("old"), None);
        assert_eq!(m.permission("phone"), Some(Permission::Write), "89 days is not 90");
        // A connected device is in use, whatever its last handshake.
        m.connections.lock().insert("phone".into(), (1, Arc::new(Notify::new())));
        assert!(m.expire_dormant(now_millis() + 200 * day).is_empty());
        m.connections.lock().clear();
        // Never, by setting; and the setting is kept.
        m.set_options(Some(0));
        assert_eq!(m.status(false)["dormant_days"], 0);
        assert!(m.expire_dormant(now_millis() + 9000 * day).is_empty());
        assert!(requested_dormant_days(&json!({"dormant_days": "soon"})).is_err());
        m.set_options(Some(30));
        assert_eq!(m.expire_dormant(now_millis() + 240 * day).len(), 1, "last seen 200 days on, 30 days allowed");
    }

    #[tokio::test]
    async fn what_a_phone_does_is_in_the_audit_log_without_what_it_typed() {
        let mut r = rig();
        let _stdin = r.chat.live_for_test();
        r.ask_write(json!({"type": "input", "id": "t1", "text": "a secret"})).await;
        r.ask_write(json!({"type": "key", "id": "t1", "key": "enter"})).await;
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m1", "text": "another secret"})).await;
        r.ask_write(json!({"type": "list"})).await;
        r.ask(Permission::Read, json!({"type": "input", "id": "t1", "text": "refused"})).await;
        r.daemon.mobile.set_write("phone", false).unwrap();
        r.daemon.mobile.revoke("phone").unwrap();
        let log = r.daemon.mobile.audit_entries(50);
        assert_eq!(log["intact"], true);
        let actions: Vec<&str> = log["entries"].as_array().unwrap().iter().rev().map(|e| e["action"].as_str().unwrap()).collect();
        assert_eq!(actions, ["input", "key", "chat_send", "permission", "revoke"], "{log}");
        let file = std::fs::read_to_string(r.daemon.mobile.dir.join(audit::FILE)).unwrap();
        assert!(!file.contains("secret"), "{file}");
    }

    /// A phone's approvals are one-shot: an answer or a key that makes a rule (for the session, always, don't ask again) is
    /// refused by the daemon, whatever its permission, and nothing reaches the agent. The same answer without it goes through.
    #[tokio::test]
    async fn a_phone_cannot_make_a_rule_that_outlasts_one_answer() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        let ask = |id: &str| json!({"type": "control_request", "request_id": id, "request": {"subtype": "can_use_tool", "tool_name": "Bash"}}).to_string();
        let answer = |id: &str, extra: Value| {
            let mut response = json!({"behavior": "allow", "updatedInput": {}});
            if let (Some(o), Some(e)) = (response.as_object_mut(), extra.as_object()) {
                o.extend(e.clone());
            }
            json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": response}}).to_string()
        };
        r.chat.ingest_for_test(ask("r1"));
        let lasting = r.ask_write(json!({"type": "chat_answer", "id": "c1", "request_id": "r1", "line": answer("r1", json!({"updatedPermissions": [{"type": "setMode", "mode": "acceptEdits", "destination": "session"}]}))})).await;
        assert_eq!(code(&lasting), Some("one_shot"), "{lasting}");
        assert!(stdin.try_recv().is_err(), "nothing reached the agent");
        let once = r.ask_write(json!({"type": "chat_answer", "id": "c1", "request_id": "r1", "line": answer("r1", json!({}))})).await;
        assert_eq!(once["result"], "ok", "{once}");
        assert!(stdin.try_recv().is_ok());
        // Codex's and ACP's lasting answers.
        let rpc = |id: u64, method: &str, params: Value| json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        let result = |id: u64, result: Value| json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
        let chat = crate::chat::ChatSession::new(5, crate::session::SessionSpec { id: "cx".into(), kind: "codex".into(), cwd: ".".into(), ..Default::default() });
        let mut cx_in = chat.live_for_test();
        r.daemon.insert_for_test(Entry::Chat(chat.clone()));
        chat.ingest_for_test(rpc(7, "item/commandExecution/requestApproval", json!({})));
        for decision in [json!("acceptForSession"), json!("approved_for_session"), json!({"acceptWithExecpolicyAmendment": {"execpolicy_amendment": ["git"]}})] {
            let reply = r.ask_write(json!({"type": "chat_answer", "id": "cx", "request_id": 7, "line": result(7, json!({"decision": decision}))})).await;
            assert_eq!(code(&reply), Some("one_shot"), "{decision}: {reply}");
        }
        assert!(cx_in.try_recv().is_err());
        assert_eq!(r.ask_write(json!({"type": "chat_answer", "id": "cx", "request_id": 7, "line": result(7, json!({"decision": "accept"}))})).await["result"], "ok");
        let acp = crate::chat::ChatSession::new(6, crate::session::SessionSpec { id: "ac".into(), kind: "gemini".into(), cwd: ".".into(), ..Default::default() });
        let _acp_in = acp.live_for_test();
        r.daemon.insert_for_test(Entry::Chat(acp.clone()));
        acp.ingest_for_test(rpc(9, "session/request_permission", json!({"options": [{"optionId": "yes", "kind": "allow_once"}, {"optionId": "yes-all", "kind": "allow_always"}]})));
        let selected = |o: &str| result(9, json!({"outcome": {"outcome": "selected", "optionId": o}}));
        assert_eq!(code(&r.ask_write(json!({"type": "chat_answer", "id": "ac", "request_id": 9, "line": selected("yes-all")})).await), Some("one_shot"));
        assert_eq!(r.ask_write(json!({"type": "chat_answer", "id": "ac", "request_id": 9, "line": selected("yes")})).await["result"], "ok");
        // A terminal's menu: the key that says "don't ask again" is refused as a key, as typed text and as an answer of any shape. Arrow
        // keys and Enter can still reach it: the documented residual.
        crate::session::fake::show_lasting_prompt(&r.term, "2");
        let since = crate::session::fake::wait_on_user(&r.term);
        let answer = |extra: Value| {
            let mut req = json!({"type": "answer", "id": "t1", "waiting_since": since});
            req.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            req
        };
        for req in [json!({"type": "key", "id": "t1", "key": "2"}), json!({"type": "input", "id": "t1", "text": "2", "force": true}), answer(json!({"text": "2"})), answer(json!({"text": "2x"})), answer(json!({"text": "2\u{200B}"})), answer(json!({"text": "yes"})), answer(json!({"key": "enter"})), answer(json!({"text": "", "enter": true}))] {
            let reply = r.ask_write(req.clone()).await;
            assert_eq!(code(&reply), Some("one_shot"), "{req}: {reply}");
        }
        assert!(r.typed.writes().is_empty(), "nothing was typed");
        assert_eq!(r.ask_write(json!({"type": "key", "id": "t1", "key": "1"})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![b"1".to_vec()]);
    }

    /// What a phone can name is the closed verb table: device management, settings, agent configuration, pairing, automations
    /// and rules are not in it, for any permission. The connection code has no path to the daemon's own methods.
    #[tokio::test]
    async fn no_phone_permission_reaches_anything_the_computer_keeps_for_itself() {
        let mut r = rig();
        let forbidden = [
            "mobile.approve", "mobile.revoke", "mobile.revoke_all", "mobile.reset", "mobile.set_permission", "mobile.set_write", "mobile.pair_start", "mobile.enable", "mobile.disable", "mobile.set_options", "mobile.audit",
            "pair", "approve", "revoke", "upgrade", "set_permission", "settings", "set_settings", "daemon.set_policy", "daemon.set_theme", "daemon.shutdown",
            "agent.set_custom", "agent.run", "hook.report", "mcp", "plugin", "automation.save", "automation.approve", "automation.delete", "automation.set_paused", "always_allow", "notify.set",
            "session.create", "session.close", "session.update", "session.attach", "run.approve", "meta.set",
        ];
        for name in forbidden {
            assert_eq!(Verb::parse(name), None, "{name}");
            for granted in [Permission::Read, Permission::Write, Permission::Control] {
                let reply = r.ask(granted, json!({"type": name, "id": "t1"})).await;
                assert_eq!(code(&reply), Some("unknown_verb"), "{name} as {granted:?}");
            }
        }
        let shipped = include_str!("mobile.rs").split("#[cfg(test)]\nmod tests").next().unwrap().to_string() + include_str!("mobile/link.rs");
        assert!(!shipped.contains(concat!(".dis", "patch(")), "the phone's code never calls the daemon's dispatcher");
    }

    #[tokio::test]
    async fn a_repeated_typing_or_answer_gets_the_first_reply_and_is_done_once() {
        let mut r = rig();
        let first = r.ask_write(json!({"type": "input", "id": "t1", "text": "once", "cid": "k1"})).await;
        assert_eq!(first["type"], "ack");
        let again = r.ask_write(json!({"type": "input", "id": "t1", "text": "once", "cid": "k1"})).await;
        assert_eq!((again["type"].as_str(), again["dup"].as_bool()), (Some("ack"), Some(true)));
        // The same device over another connection: still once.
        let mut other = other_conn(&r, "phone", "Phone", 5);
        let third = ask_as(&mut other, json!({"type": "key", "id": "t1", "key": "enter", "cid": "k1"})).await;
        assert_eq!(third["dup"], true);
        assert_eq!(r.typed.writes(), vec![b"once".to_vec()]);
        // Another cid is another message; another device's cid is its own.
        r.ask_write(json!({"type": "input", "id": "t1", "text": "twice", "cid": "k2"})).await;
        let mut tablet = other_conn(&r, "tablet", "Tablet", 6);
        ask_as(&mut tablet, json!({"type": "input", "id": "t1", "text": "tablet", "cid": "k1"})).await;
        assert_eq!(r.typed.writes(), vec![b"twice".to_vec(), b"tablet".to_vec()]);
        // A reader's request is not remembered, so it cannot later pass for a writer's.
        let read = r.ask(Permission::Read, json!({"type": "input", "id": "t1", "text": "no", "cid": "k9"})).await;
        assert_eq!(read["code"], "read_only");
        assert!(r.daemon.mobile.repeated("phone", "k9").is_none());
        // Typing is remembered for a minute, answers for ten.
        assert_eq!((INPUT_DEDUPE, ANSWER_DEDUPE), (Duration::from_secs(60), Duration::from_secs(600)));
    }

    #[tokio::test]
    async fn revoking_or_downgrading_stops_what_the_device_controls_first() {
        let r = rig();
        let m = &r.daemon.mobile;
        let screen = m.grant("phone");
        let other = m.grant("tablet");
        m.set_write("phone", true).unwrap();
        assert!(screen.load(Ordering::SeqCst), "a raise or the same permission keeps it");
        let mut events = m.revoked.subscribe();
        m.set_write("phone", false).unwrap();
        assert!(!screen.load(Ordering::SeqCst), "a downgrade stops it");
        assert!(events.try_recv().is_ok(), "and only then is the connection told");
        let again = m.grant("phone");
        m.revoke("phone").unwrap();
        assert!(!again.load(Ordering::SeqCst));
        assert!(other.load(Ordering::SeqCst), "another device's grant is its own");
        m.disable();
        assert!(!other.load(Ordering::SeqCst), "turning phone access off stops everything");
    }

    #[test]
    fn an_edited_record_is_refused_and_the_old_file_is_never_rewritten() {
        let dir = std::env::temp_dir().join(format!("kelpie-store-mobile-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = br#"{"enabled":false,"devices":[{"id":"o","name":"Old","token_sha256":"x","can_write":true,"created_at":1}]}"#;
        std::fs::write(dir.join(store::LEGACY_FILE), old).unwrap();
        let secrets: Arc<dyn SecretStore> = crate::notify::Memory::new();
        let m = Mobile::load(&dir, secrets.clone());
        assert_eq!(m.status(false)["legacy"][0]["name"], "Old", "listed to pair again");
        m.config.lock().enabled = true;
        m.add_test_device("d", "Pixel", [4u8; 32], Permission::Read, Kind::Pwa);
        m.keep();
        assert_eq!(std::fs::read(dir.join(store::LEGACY_FILE)).unwrap(), old, "the old file is left for an older Kelpie");
        assert_eq!(Mobile::load(&dir, secrets.clone()).permission("d"), Some(Permission::Read), "a record as written verifies");
        // Someone raises the device's permission in the file.
        let text = std::fs::read_to_string(dir.join(store::FILE)).unwrap().replace("\"read\"", "\"control\"");
        std::fs::write(dir.join(store::FILE), &text).unwrap();
        let edited = Mobile::load(&dir, secrets.clone());
        assert_eq!(edited.permission("d"), None, "refused");
        assert!(matches!(edited.lookup_key(&[4u8; 32]), KeyLookup::Unconfirmed), "known, and not to be trusted");
        assert_eq!(edited.status(false)["devices"][0]["state"], "unverified");
        // The same file with a credential store that lost its key: refused too.
        let _ = secrets.delete(K_MAC);
        std::fs::write(dir.join(store::FILE), text.replace("\"control\"", "\"read\"")).unwrap();
        assert_eq!(Mobile::load(&dir, secrets).permission("d"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_unreadable_store_keeps_phone_access_off_and_is_never_written_over_until_reset() {
        let dir = std::env::temp_dir().join(format!("kelpie-store-broken-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(store::FILE), b"{\"version\":2,\"enabled\":tru").unwrap();
        let daemon = Daemon::with_secrets(dir.clone(), std::path::PathBuf::from("kelpied"), crate::notify::Memory::new());
        assert!(daemon.mobile.status(false)["store_error"].is_string());
        let err = enable(daemon.clone(), json!({"ip": "127.0.0.1", "port": 0})).await.unwrap_err();
        assert!(err.to_string().contains("Reset phone access"), "{err}");
        daemon.mobile.keep();
        assert_eq!(std::fs::read(dir.join(store::FILE)).unwrap(), b"{\"version\":2,\"enabled\":tru", "not written over");
        daemon.mobile.reset();
        assert!(daemon.mobile.status(false)["store_error"].is_null());
        let fresh: Value = serde_json::from_slice(&std::fs::read(dir.join(store::FILE)).unwrap()).unwrap();
        assert_eq!(fresh["version"], 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A chat log that finds the bulk lane full waits for room; a phone that makes none for `SLOW_AFTER`
    /// is cut as a slow consumer (4411), and not before.
    #[tokio::test]
    async fn a_chat_log_with_no_room_for_ten_seconds_cuts_the_phone_as_a_slow_consumer() {
        let r = rig();
        let lanes = lanes::lanes();
        while lanes.bulk.try_send("frame".into()).is_ok() {}
        // One terminal output frame for the chat (handle 2): `u32 length | u8 kind | u32 handle | u64 seq | bytes`.
        let mut frame = vec![0, 0, 0, 0, kelpie_proto::KIND_OUTPUT];
        frame.extend_from_slice(&2u32.to_le_bytes());
        frame.extend_from_slice(&1u64.to_le_bytes());
        frame.extend_from_slice(b"hello\n");
        let (tx, rx) = mpsc::channel::<Outgoing>(8);
        let task = tokio::spawn(adapt(r.daemon.clone(), rx, lanes.bulk.clone(), lanes.slow.clone(), Arc::new(Notify::new()), "phone".into()));
        tx.send(Outgoing { data: frame.into(), credit: None }).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(9), lanes.slow.notified()).await.is_err(), "the log waits for room");
        tokio::time::timeout(Duration::from_secs(3), lanes.slow.notified()).await.expect("cut after ten seconds without room");
        task.await.unwrap();
    }

    #[tokio::test]
    async fn a_phone_that_falls_behind_is_told_to_start_over_not_queued() {
        let (tx, mut rx, overflowed) = bounded_client_channel(2);
        let credit = Arc::new(AtomicUsize::new(0));
        for _ in 0..5 {
            credit.fetch_add(10, Ordering::Relaxed);
            tx.send(Outgoing { data: Arc::from(vec![0u8; 10]), credit: Some(credit.clone()) }).expect("a full queue is not a closed one");
        }
        assert!(overflowed.load(Ordering::Relaxed));
        let mut queued = 0;
        while rx.try_recv().is_ok() {
            queued += 1;
        }
        assert_eq!(queued, 2, "the queue held two");
        assert_eq!(credit.load(Ordering::Relaxed), 20, "what was dropped is no longer counted as waiting");
        drop(rx);
        assert!(tx.send(Outgoing { data: Arc::from(vec![1u8]), credit: None }).is_err(), "a gone client is");

        // The connection's next look: the whole list again, and every watched chat asked for again.
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        r.conn.overflowed.store(true, Ordering::Relaxed);
        let told = r.conn.housekeeping();
        assert_eq!(told.iter().map(|m| m["type"].as_str().unwrap()).collect::<Vec<_>>(), ["sessions", "chat_resync"]);
        assert!(r.conn.housekeeping().is_empty());
    }

    fn sent_text(stdin: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<String> {
        let mut out = Vec::new();
        while let Ok(bytes) = stdin.try_recv() {
            for line in String::from_utf8(bytes).unwrap().lines() {
                let v: Value = serde_json::from_str(line).unwrap();
                out.push(v.pointer("/message/content/0/text").and_then(Value::as_str).unwrap_or("<other>").to_string());
            }
        }
        out
    }

    #[tokio::test]
    async fn a_phone_sends_a_message_and_a_retry_makes_no_second_one() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        let ok = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m1", "text": "hello"})).await;
        assert_eq!((ok["type"].clone(), ok["result"].clone(), ok["state"].clone(), ok["ahead"].clone()), (json!("ack"), json!("ok"), json!("sent"), json!(0)), "{ok}");
        let qid = ok["qid"].clone();
        assert_eq!(sent_text(&mut stdin), ["hello"]);
        let dup = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m1", "text": "hello"})).await;
        assert_eq!((dup["result"].clone(), dup["qid"].clone(), dup["state"].clone()), (json!("dup"), qid, json!("sent")), "{dup}");
        assert!(sent_text(&mut stdin).is_empty(), "a retry writes nothing");
        // A message that waits behind the running turn.
        let queued = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m2", "text": "next"})).await;
        assert_eq!((queued["state"].clone(), queued["ahead"].clone()), (json!("queued"), json!(0)));
        let again = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m2", "text": "next"})).await;
        assert_eq!((again["result"].clone(), again["state"].clone(), again["qid"].clone()), (json!("dup"), json!("queued"), queued["qid"].clone()));
        // Cancel it, and a second cancel is too late.
        let cancel = json!({"type": "chat_cancel", "id": "c1", "qid": queued["qid"]});
        assert_eq!(r.ask_write(cancel.clone()).await["result"], "ok");
        assert_eq!(code(&r.ask_write(cancel).await), Some("too_late"));
        let gone = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m2", "text": "next"})).await;
        assert_eq!((gone["result"].clone(), gone["state"].clone()), (json!("dup"), json!("cancelled")));
    }

    #[tokio::test]
    async fn a_chat_send_is_checked_before_it_is_queued() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        let send = |cid: &str, text: &str| json!({"type": "chat_send", "id": "c1", "cid": cid, "text": text});
        for (req, expected) in [
            (send("", "x"), "bad_request"),
            (send(&"c".repeat(65), "x"), "bad_request"),
            (json!({"type": "chat_send", "id": "c1", "cid": "a"}), "bad_request"),
            (send("a", "  \n"), "empty"),
            (send("a", &"x".repeat(MAX_TYPED + 1)), "too_long"),
            (json!({"type": "chat_send", "id": "t1", "cid": "a", "text": "x"}), "not_chat"),
            (json!({"type": "chat_send", "id": "nope", "cid": "a", "text": "x"}), "no_session"),
            (json!({"type": "chat_send", "id": "c1", "cid": "a", "text": "x", "epoch": "old"}), "epoch"),
        ] {
            let reply = r.ask_write(req.clone()).await;
            assert_eq!(code(&reply), Some(expected), "{req}: {reply}");
            r.daemon.mobile.send_rate.lock().clear();
        }
        assert!(sent_text(&mut stdin).is_empty());
        // A chat whose agent is gone.
        let dead = crate::chat::ChatSession::new(3, crate::session::SessionSpec { id: "c2".into(), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
        r.daemon.insert_for_test(Entry::Chat(dead));
        assert_eq!(code(&r.ask_write(json!({"type": "chat_send", "id": "c2", "cid": "a", "text": "x"})).await), Some("exited"));
        // The chat is being handed to a terminal.
        r.chat.hold(true);
        assert_eq!(code(&r.ask_write(send("h", "x")).await), Some("switching"));
        r.chat.hold(false);
        // Codex commands run on the computer.
        let codex = crate::chat::ChatSession::new(4, crate::session::SessionSpec { id: "cx".into(), kind: "codex".into(), cwd: ".".into(), ..Default::default() });
        let _codex_stdin = codex.live_for_test();
        r.daemon.insert_for_test(Entry::Chat(codex));
        assert_eq!(code(&r.ask_write(json!({"type": "chat_send", "id": "cx", "cid": "a", "text": "/compact"})).await), Some("command_needs_desktop"));
        // `input` on a chat is still refused: a phone's way to a chat is the verbs above.
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "c1", "text": "x"})).await), Some("chat"));
    }

    #[tokio::test]
    async fn a_phone_sends_one_message_a_second_in_bursts_of_three_and_five_may_wait() {
        let mut r = rig();
        let _stdin = r.chat.live_for_test();
        let send = |i: usize| json!({"type": "chat_send", "id": "c1", "cid": format!("m{i}"), "text": format!("t{i}")});
        for i in 0..3 {
            assert_eq!(r.ask_write(send(i)).await["type"], "ack", "{i}");
        }
        assert_eq!(code(&r.ask_write(send(3)).await), Some("rate_limited"));
        // A retry of one that was accepted is not a new send.
        assert_eq!(r.ask_write(send(1)).await["result"], "dup");
        // A second later there is one more.
        r.daemon.mobile.send_rate.lock().get_mut("phone").unwrap().at -= Duration::from_millis(1100);
        assert_eq!(r.ask_write(send(3)).await["type"], "ack");
        // Five waiting from one device is the most (the first one is running).
        for i in 4..7 {
            r.daemon.mobile.send_rate.lock().clear();
            let reply = r.ask_write(send(i)).await;
            assert_eq!(reply["type"], if i < 6 { "ack" } else { "error" }, "{i}: {reply}");
            if i >= 6 {
                assert_eq!(code(&reply), Some("queue_full"));
            }
        }
        // Another device has its own five.
        r.daemon.mobile.add_test_device("other", "Other", [5u8; 32], Permission::Write, Kind::App);
        r.conn.device = "other".into();
        r.conn.name = "Other".into();
        assert_eq!(r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "o1", "text": "x"})).await["type"], "ack");
    }

    #[tokio::test]
    async fn chat_verbs_are_never_shed_at_the_worst_load() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        r.daemon.mobile.load.set_for_test(Level::L3);
        let sent = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "a", "text": "run"})).await;
        assert_eq!(sent["result"], "ok", "{sent}");
        let queued = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "b", "text": "wait"})).await;
        assert_eq!(queued["state"], "queued");
        assert_eq!(r.ask_write(json!({"type": "chat_cancel", "id": "c1", "qid": queued["qid"]})).await["result"], "ok");
        let queued = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "c", "text": "wait again"})).await;
        assert_eq!(queued["state"], "queued");
        let stopped = r.ask_write(json!({"type": "chat_stop", "id": "c1"})).await;
        assert_eq!((stopped["type"].clone(), stopped["result"].clone()), (json!("ack"), json!("ok")));
        assert_eq!(r.ask_write(json!({"type": "chat_resume", "id": "c1"})).await["result"], "ok");
        assert_eq!(sent_text(&mut stdin), ["run", "<other>"], "the message and the interrupt");
    }

    #[tokio::test]
    async fn revoking_or_downgrading_a_device_drops_its_waiting_messages_but_not_its_running_turn() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "a", "text": "running"})).await;
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "b", "text": "waiting"})).await;
        assert_eq!(sent_text(&mut stdin), ["running"]);
        r.daemon.drop_device_queues("phone", "revoked");
        r.chat.ingest_for_test(json!({"type": "result"}).to_string());
        assert!(sent_text(&mut stdin).is_empty(), "dropped: never written");
        let dup = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "b", "text": "waiting"})).await;
        assert_eq!((dup["result"].clone(), dup["state"].clone()), (json!("dup"), json!("dropped")));
        // Through the daemon's own methods.
        let (tx, _rx) = crate::session::client_channel();
        r.daemon.mobile.send_rate.lock().clear();
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "c", "text": "running again"})).await;
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "d", "text": "waiting again"})).await;
        r.daemon.dispatch(1, &tx, &json!(1), "mobile.set_write", json!({"id": "phone", "can_write": false})).unwrap();
        r.chat.ingest_for_test(json!({"type": "result"}).to_string());
        assert_eq!(sent_text(&mut stdin), ["running again"]);
        assert_eq!(r.daemon.mobile.permission("phone"), Some(Permission::Read));
        r.daemon.mobile.send_rate.lock().clear();
        r.daemon.mobile.set_write("phone", true).unwrap();
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "e", "text": "running once more"})).await;
        r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "f", "text": "waiting once more"})).await;
        r.daemon.dispatch(1, &tx, &json!(2), "mobile.revoke", json!({"id": "phone"})).unwrap();
        r.chat.ingest_for_test(json!({"type": "result"}).to_string());
        assert_eq!(sent_text(&mut stdin), ["running once more"]);
    }

    #[tokio::test]
    async fn the_phone_hears_the_queue_of_its_chat_and_only_the_latest_state() {
        let r = rig();
        let dirty = Notify::new();
        let mut t = Translator { device: "phone".into(), ..Default::default() };
        let state = |n: usize, paused: bool| {
            let items: Vec<Value> = (0..n)
                .map(|i| {
                    let (id, name, kind) = if i == 0 { ("phone", "Pixel", "phone") } else { ("desktop", "desktop", "desktop") };
                    json!({"qid": format!("q{i}"), "cid": format!("c{i}"), "by": {"id": id, "name": name}, "kind": kind, "text": "t".repeat(500), "at": 1})
                })
                .collect();
            crate::session::json_out(&json!({"event": "chat.queue", "params": {"handle": 2, "id": "c1", "items": items, "paused": paused, "held": false, "in_flight": true}})).data
        };
        let messages = t.translate(&r.daemon, &state(2, true), &dirty);
        assert_eq!(messages.len(), 1);
        let m = &messages[0];
        assert_eq!((m["type"].clone(), m["id"].clone(), m["paused"].clone(), m["in_flight"].clone()), (json!("chat_queue"), json!("c1"), json!(true), json!(true)));
        assert_eq!((m["items"][0]["mine"].clone(), m["items"][0]["by"].clone(), m["items"][1]["mine"].clone(), m["items"][1]["kind"].clone()), (json!(true), json!("Pixel"), json!(false), json!("desktop")));
        assert_eq!(m["items"][0]["text"].as_str().unwrap().chars().count(), 300, "the chip shows the start of a long message");
        let dropped = crate::session::json_out(&json!({"event": "chat.queue", "params": {"handle": 2, "id": "c1", "items": [], "paused": false, "held": false, "in_flight": false, "dropped": [{"qid": "q9", "cid": "z", "by": {"id": "phone", "name": "Pixel"}, "text": "lost", "reason": "revoked"}]}})).data;
        let mut all: Vec<Value> = [state(3, false), state(1, false)].iter().flat_map(|f| t.translate(&r.daemon, f, &dirty)).collect();
        all.extend(t.translate(&r.daemon, &dropped, &dirty));
        all.extend(t.translate(&r.daemon, &state(0, false), &dirty));
        latest_queue_only(&mut all);
        assert_eq!(all.len(), 1, "{all:?}");
        assert_eq!(all[0]["items"].as_array().unwrap().len(), 0);
        assert_eq!((all[0]["dropped"][0]["text"].clone(), all[0]["dropped"][0]["mine"].clone()), (json!("lost"), json!(true)), "news that was in the states left out is kept");
        // Other messages stay where they are.
        let mut mixed = vec![json!({"type": "chat_queue", "id": "a", "items": []}), json!({"type": "chat", "id": "a"}), json!({"type": "chat_queue", "id": "b", "items": []}), json!({"type": "chat_queue", "id": "a", "items": [1]})];
        latest_queue_only(&mut mixed);
        let kinds: Vec<String> = mixed.iter().map(|m| format!("{}:{}", m["type"].as_str().unwrap(), m["id"].as_str().unwrap())).collect();
        assert_eq!(kinds, ["chat:a", "chat_queue:b", "chat_queue:a"]);
    }

    #[tokio::test]
    async fn a_chat_send_the_chat_refuses_costs_no_token_and_the_limit_is_as_strong_for_what_gets_in() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        for i in 0..8 {
            let reply = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": format!("blank-{i}"), "text": "  "})).await;
            assert_eq!(code(&reply), Some("empty"), "{i}: {reply}");
        }
        // Eight refusals later the whole burst is still there, and no more than the burst.
        for i in 0..3 {
            assert_eq!(r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": format!("ok-{i}"), "text": format!("t{i}")})).await["type"], "ack", "{i}");
        }
        assert_eq!(code(&r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "ok-3", "text": "t3"})).await), Some("rate_limited"));
        assert_eq!(sent_text(&mut stdin).len(), 1, "one running, two waiting");
        // A full queue costs nothing either.
        for i in 0..20 {
            r.daemon.mobile.send_rate.lock().clear();
            r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": format!("fill-{i}"), "text": "x"})).await;
        }
        r.daemon.mobile.send_rate.lock().clear();
        for i in 0..3 {
            let full = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": format!("over-{i}"), "text": "x"})).await;
            assert!(matches!(code(&full), Some("queue_full")), "{i}: {full}");
        }
    }

    #[tokio::test]
    async fn a_device_revoked_between_the_check_and_the_queue_leaves_nothing_behind() {
        let mut r = rig();
        let mut stdin = r.chat.live_for_test();
        // The connection was granted Write a moment ago; the record says Read by the time the message reaches the queue.
        r.daemon.mobile.set_write("phone", false).unwrap();
        let reply = r.conn.handle(Permission::Write, &json!({"type": "chat_send", "id": "c1", "cid": "late", "text": "too late", "req": 1})).await.unwrap();
        assert_eq!(code(&reply), Some("read_only"), "{reply}");
        assert!(sent_text(&mut stdin).is_empty());
        assert_eq!(r.chat.sent_state("phone", "late"), None);
        r.daemon.mobile.set_write("phone", true).unwrap();
        r.daemon.mobile.revoke("phone").unwrap();
        let gone = r.conn.handle(Permission::Write, &json!({"type": "chat_send", "id": "c1", "cid": "later", "text": "revoked", "req": 2})).await.unwrap();
        assert_eq!(code(&gone), Some("read_only"));
        assert!(sent_text(&mut stdin).is_empty());
    }

    #[tokio::test]
    async fn a_send_to_a_chat_that_is_not_running_says_how_to_start_it() {
        let mut r = rig();
        // The rig's chat has no process, as one restored after a daemon restart.
        let reply = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "a", "text": "hello"})).await;
        assert_eq!(code(&reply), Some("exited"));
        assert!(reply["message"].as_str().unwrap().contains("Open this chat on the desktop to resume it"), "{reply}");
    }

    #[tokio::test]
    async fn a_cancel_by_the_other_side_reaches_the_one_whose_message_it_was() {
        let mut r = rig();
        let _stdin = r.chat.live_for_test();
        r.chat.ingest_for_test(json!({"type": "assistant", "message": {}}).to_string());
        let (window, mut rx) = crate::session::client_channel();
        r.chat.attach(9, &window, &json!(1));
        let queued = r.ask_write(json!({"type": "chat_send", "id": "c1", "cid": "m1", "text": "waiting"})).await;
        while rx.try_recv().is_ok() {}
        r.chat.cancel(queued["qid"].as_str().unwrap()).unwrap();
        let mut told = None;
        while let Ok(o) = rx.try_recv() {
            if o.data[4] != kelpie_proto::KIND_JSON {
                continue;
            }
            let v: Value = serde_json::from_slice(&o.data[5..]).unwrap();
            if v["event"] == "chat.queue" {
                told = Some(v["params"].clone());
            }
        }
        assert_eq!(told.unwrap()["dropped"][0]["cancelled_by"]["id"], "desktop");
        // The phone's own view of the same event.
        let dirty = Notify::new();
        let mut t = Translator { device: "phone".into(), ..Default::default() };
        let event = crate::session::json_out(&json!({"event": "chat.queue", "params": {"handle": 2, "id": "c1", "items": [], "paused": false, "held": false, "in_flight": false,
            "dropped": [{"qid": "q1", "cid": "m1", "by": {"id": "phone", "name": "Phone"}, "text": "waiting", "reason": "cancelled", "cancelled_by": {"id": "desktop", "name": "desktop"}}]}})).data;
        let told = t.translate(&r.daemon, &event, &dirty);
        let m = &told[0];
        assert_eq!((m["dropped"][0]["reason"].clone(), m["dropped"][0]["cancelled_by"].clone(), m["dropped"][0]["mine"].clone()), (json!("cancelled"), json!("desktop"), json!(true)));
    }

    /// Nothing a phone does may queue without a limit: no unbounded channel on any of its paths.
    #[test]
    fn no_phone_path_has_an_unbounded_queue() {
        let needles = [concat!("unbounded", "_channel"), concat!("Unbounded", "Sender"), concat!("Unbounded", "Receiver"), concat!("ClientTx::", "Unbounded")];
        let sources = [
            ("mobile.rs", include_str!("mobile.rs")),
            ("mobile/sync.rs", include_str!("mobile/sync.rs")),
            ("mobile/lanes.rs", include_str!("mobile/lanes.rs")),
            ("mobile/verbs.rs", include_str!("mobile/verbs.rs")),
            ("mobile/link.rs", include_str!("mobile/link.rs")),
            ("mobile/listen.rs", include_str!("mobile/listen.rs")),
            ("mobile/audit.rs", include_str!("mobile/audit.rs")),
            ("load.rs", include_str!("load.rs")),
        ];
        // The part of each file before its tests: what ships.
        for (name, source) in sources {
            let shipped = source.split("#[cfg(test)]").next().unwrap();
            for needle in needles {
                assert!(!shipped.contains(needle), "{name} uses {needle}: a phone's queue must have a limit");
            }
        }
        // And the one place a phone connects registers a bounded queue.
        assert!(include_str!("mobile.rs").contains(concat!("bounded_client_channel(", "EVENT_QUEUE)")));
    }

}
