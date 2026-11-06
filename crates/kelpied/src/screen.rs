//! A phone's view of this computer's screen: a monitor or one window, as H.264, and the phone's input back.
//!
//! The helper `kelpie-screen` (spawned by the Kelpie app, which alone can: `provider` kind `screen`) captures and encodes; the app hands each
//! media message to this broker as `provider.event {sc, kind: "media", data}` (base64 on the local pipe), and the broker sends it to the phone
//! as a binary message on the phone's encrypted link (`mobile::lanes`, the media lane). JSON goes beside it: `sc_state`, `sc_stats`, `sc_fhb`,
//! `sc_end`. Like a browser pane's takeover (`takeover`), and with the same rules:
//!
//! - **A hold** is keyed by its device, the link connection and the provider's connection, rests on a grant flag (`Mobile::grant`) that a
//!   revoke, a lower permission, the app lock and phone access going off clear at once, and on the record as it is now (`control_held`, looked
//!   at again by its task); every input checks it, and its own task lets go within `WATCH`. A dropped link keeps it `REBIND_WINDOW` for the same
//!   device's next connection only (the same connection asking again is no change). **One screen at a time**: a start reserves the screen for
//!   its whole length, so a second one that comes meanwhile is refused as in use, and the first holder keeps it.
//! - **Flow:** the link is TCP, so nothing is lost and nothing should pile up. The helper encodes only while fewer than 3 frames wait for the
//!   phone's acknowledgement (`sc_ack`, forwarded with its round trip as `screen.flow`). Should the lane still be full, or more than
//!   `MAX_UNACKED_BYTES` wait, video is dropped up to the next keyframe, which is asked for at most once a second: a decoder given a frame
//!   whose reference never came would show garbage. **What is dropped is said**: `screen.flow` carries `dropped_through`, so the helper never
//!   waits for an acknowledgement that cannot come, and `lost_tiles`, since a dropped frame may have carried the unpaint of a crisp tile.
//!   Tiles and the cursor go only while the lane has room for video beside them. `screen.flow` also carries the bytes on the wire for the bytes
//!   of media (the link pads every message), so the encoder is asked for what the link really carries.
//! - **Input** older than 1 s by the computer's clock is dropped, and so is input while the phone's picture is stale (a frame unacknowledged
//!   for `STALE_AFTER`, or a keyframe owed for as long); at most `INPUT_PER_SECOND` messages a second. Text goes with an answer of its own.
//!   None of it is logged: the audit log has the start (the program, or a monitor) and the end.
//!
//! Never over the relay: `control` is never granted on a relayed connection (`mobile::transport_cap`).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use kelpie_screen::wire::{self, KIND_TILE, KIND_VIDEO};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use crate::daemon::Daemon;
use crate::load::Level;
use crate::session::now_millis;
use crate::takeover::{Refused, IDLE_STOP, INPUT_PER_SECOND, REBIND_WINDOW};

const WATCH: Duration = Duration::from_millis(250);
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// Media sent and not acknowledged, at most.
pub const MAX_UNACKED_BYTES: usize = 512 * 1024;
/// The phone's picture is stale when a frame waited this long for its acknowledgement, or a keyframe has been owed this long: input is not sent.
pub const STALE_AFTER: Duration = Duration::from_secs(2);
/// Room the media lane keeps for video: a tile or the cursor goes only with at least this many places free.
pub const ROOM_FOR_VIDEO: usize = 3;
const KEY_GAP: Duration = Duration::from_secs(1);
/// Starting the helper and its first look can take a while on a busy machine. The app answers within its own shorter budget.
pub const START_LIMIT: Duration = Duration::from_secs(15);
/// New screens are refused with less memory free than this (percent).
const MIN_FREE_MEMORY: f64 = 20.0;

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused { code, message: message.into() }
}

/// The phone connection a verb came on.
pub struct Phone<'a> {
    pub device: &'a str,
    pub name: &'a str,
    pub conn: u64,
    /// Its JSON lane and its media lane.
    pub out: &'a mpsc::Sender<String>,
    pub media: &'a mpsc::Sender<Vec<u8>>,
}

struct State {
    conn: Option<(u64, mpsc::Sender<String>, mpsc::Sender<Vec<u8>>)>,
    gone_at: Option<Instant>,
    /// Video frames sent and not acknowledged: number, when, size.
    sent: VecDeque<(u32, Instant, usize)>,
    unacked: usize,
    acked: u32,
    /// The last video frame dropped here: the helper hears it, and stops counting those frames as in flight.
    dropped_through: u32,
    /// Video is dropped until a keyframe, which was asked for, since then.
    need_key: bool,
    need_key_since: Instant,
    key_asked: Option<Instant>,
    last_input: Instant,
    last_beat: Instant,
    /// A tile could not be sent, or a frame that may have carried an unpaint was dropped: the helper paints every still cell again.
    lost_tiles: bool,
    /// Media bytes sent and what they took on the wire, since the helper was last told.
    raw: u64,
    wire: u64,
    inputs: (Instant, u32),
    level: Level,
    idle: Duration,
    rebind: Duration,
    /// The number of the last pause or resume told to the app: those are sent after the lock is let go, so two may arrive out of order, and the
    /// app keeps the newest.
    version: u64,
}

pub struct Hold {
    sc: String,
    device: String,
    provider: u64,
    grant: Arc<AtomicBool>,
    ended: AtomicBool,
    wake: Notify,
    state: Mutex<State>,
}

impl Hold {
    fn live(&self) -> bool {
        !self.ended.load(Ordering::SeqCst) && self.grant.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
pub struct Broker {
    holds: Mutex<HashMap<String, Arc<Hold>>>,
    /// A start under way: the screen is reserved for it.
    starting: Mutex<bool>,
}

/// The screen reserved for a start, until it is dropped.
struct Starting<'a>(&'a Broker);

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        *self.0.starting.lock() = false;
    }
}

fn short(v: Option<&Value>, n: usize) -> String {
    v.and_then(Value::as_str).unwrap_or_default().chars().filter(|c| !c.is_control()).take(n).collect()
}

/// An executable's file name as the audit log has it (`Code.exe`): a name only, never a path, nothing else.
fn program(v: Option<&Value>) -> String {
    let name: String = short(v, 200).rsplit(['\\', '/']).next().unwrap_or_default().chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ')).take(60).collect();
    if name.trim().is_empty() {
        "unknown program".into()
    } else {
        name
    }
}

/// What a message of `len` bytes takes on the link: chunks of `MAX_CHUNK`, each with its header byte and tag, the last padded to its bucket
/// (`kelpie_link::PAD_BUCKETS`). Phones that read padded chunks are all of them now, so it is counted for every phone.
pub fn wire_len(len: usize) -> usize {
    const TAG: usize = 16;
    let chunk = kelpie_link::MAX_CHUNK;
    let (full, rest) = (len / chunk, len % chunk);
    let mut total = full * (chunk + 1 + TAG);
    if rest > 0 || full == 0 {
        let plain = rest + 1;
        total += kelpie_link::PAD_BUCKETS.iter().copied().find(|b| *b >= plain).unwrap_or(plain) + TAG;
    }
    total
}

/// The app's `screen.pause` for a hold, numbered under its lock.
fn pause_notice(st: &mut State, sc: &str, paused: bool) -> Value {
    st.version += 1;
    json!({"sc": sc, "paused": paused, "v": st.version})
}

fn end_message(sc: &str, reason: &str) -> String {
    json!({"type": "sc_end", "sc": sc, "reason": reason, "server_ts": now_millis()}).to_string()
}

/// Why a stream ended, as the phone knows the words.
const END_REASONS: &[&str] = &["hotkey", "closed", "desktop", "encoder_failed", "app_closed", "replaced", "display_changed", "failed"];
/// What the helper says about the picture, as the phone knows the words.
const STATES: &[&str] = &["live", "locked", "secure_desktop", "closed", "minimized", "elevated", "covered", "paused_load", "blocked", "not_front"];

/// A target as the phone is told it: only known fields, short strings.
fn target_row(t: &Value) -> Option<Value> {
    let id = t["id"].as_str().filter(|s| kelpie_screen::targets::TargetId::parse(s).is_some())?;
    let kind = t["kind"].as_str().filter(|k| matches!(*k, "monitor" | "window"))?;
    let shortcut = t["shortcut"].as_str().filter(|s| matches!(*s, "ide" | "kelpie"));
    let n = |k: &str| t[k].as_i64().map_or(0, |v| v.clamp(0, 100_000));
    Some(json!({"id": id, "kind": kind, "title": short(t.get("title"), 120), "app": short(t.get("app"), 60), "shortcut": shortcut, "w": n("w"), "h": n("h"), "minimized": t["minimized"].as_bool() == Some(true), "primary": t["primary"].as_bool() == Some(true)}))
}

impl Broker {
    fn hold(&self, sc: &str) -> Option<Arc<Hold>> {
        self.holds.lock().get(sc).cloned()
    }

    /// The hold `sc`, if it is this device's, still live, and on this connection.
    fn held_here(&self, phone: &Phone<'_>, req: &Value) -> Result<Arc<Hold>, Refused> {
        let sc = req["sc"].as_str().unwrap_or_default();
        let hold = self.hold(sc).filter(|h| h.device == phone.device).ok_or_else(|| refused("sc_gone", "That screen has ended."))?;
        if !hold.live() {
            return Err(refused("sc_gone", "That screen has ended."));
        }
        if hold.state.lock().conn.as_ref().map(|(c, ..)| *c) != Some(phone.conn) {
            return Err(refused("sc_elsewhere", "That screen is on another connection."));
        }
        Ok(hold)
    }

    /// Whether one more input message fits in this second.
    fn input_allowed(hold: &Hold) -> bool {
        let mut st = hold.state.lock();
        let now = Instant::now();
        if now.duration_since(st.inputs.0) >= Duration::from_secs(1) {
            st.inputs = (now, 0);
        }
        st.inputs.1 += 1;
        st.inputs.1 <= INPUT_PER_SECOND
    }

    /// Whether the phone's picture is too old to act on.
    fn stale(st: &State) -> bool {
        st.sent.front().is_some_and(|(_, at, _)| at.elapsed() >= STALE_AFTER) || (st.need_key && st.need_key_since.elapsed() >= STALE_AFTER)
    }

    /// Ends hold `sc`: the phone and the app are told (the app only when it did not end it itself), and the audit log has it.
    pub fn end(&self, d: &Daemon, sc: &str, reason: &str, tell_app: bool) {
        let Some(hold) = self.holds.lock().remove(sc) else { return };
        if hold.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        hold.wake.notify_one();
        let conn = hold.state.lock().conn.take();
        if let Some((_, out, _)) = conn {
            if let Err(mpsc::error::TrySendError::Full(msg)) = out.try_send(end_message(sc, reason)) {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(5), out.send(msg)).await;
                    });
                }
            }
        }
        if tell_app {
            d.providers.notify("screen", "screen.stop", json!({"sc": sc, "reason": reason}));
        }
        d.mobile.audit(&hold.device, "screen_stop", reason);
    }

    /// The link connection `conn` closed: its holds wait for the same device's next one, and the helper stops encoding meanwhile.
    pub fn link_closed(&self, d: &Daemon, conn: u64) {
        let gone: Vec<Value> = self
            .holds
            .lock()
            .values()
            .filter_map(|h| {
                let mut st = h.state.lock();
                if !st.conn.as_ref().is_some_and(|(c, ..)| *c == conn) {
                    return None;
                }
                Self::set_gone(&mut st);
                Some(pause_notice(&mut st, &h.sc, true))
            })
            .collect();
        for params in gone {
            d.providers.notify("screen", "screen.pause", params);
        }
    }

    fn set_gone(st: &mut State) {
        st.conn = None;
        st.gone_at = Some(Instant::now());
        st.sent.clear();
        st.unacked = 0;
        st.need_key = true;
        st.need_key_since = Instant::now();
    }

    /// The provider's connection is gone: every hold it had ends.
    pub fn provider_gone(&self, d: &Daemon, client: u64) {
        let gone: Vec<String> = self.holds.lock().values().filter(|h| h.provider == client).map(|h| h.sc.clone()).collect();
        for sc in gone {
            self.end(d, &sc, "app_closed", false);
        }
    }

    /// Whether `provider.event` params are this broker's.
    pub fn owns_event(params: &Value) -> bool {
        params.get("sc").is_some()
    }

    /// `provider.event` from `client`: media, a state, the stats, or a stream the helper ended. Only the hold's own provider is heard.
    pub fn on_event(&self, d: &Daemon, client: u64, params: &Value) {
        let sc = params["sc"].as_str().unwrap_or_default();
        let Some(hold) = self.hold(sc).filter(|h| h.provider == client) else { return };
        match params["kind"].as_str() {
            Some("media") => {
                let Some(bytes) = params["data"].as_str().and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok()) else { return };
                self.media(d, &hold, bytes);
            }
            Some("state") => {
                if let Some(state) = params["state"].as_str().filter(|s| STATES.contains(s)) {
                    self.tell(&hold, json!({"type": "sc_state", "sc": sc, "state": state, "server_ts": now_millis()}));
                }
            }
            Some("stats") => {
                let s = &params["stats"];
                let n = |k: &str| s[k].as_f64().filter(|v| v.is_finite()).map_or(json!(null), |v| json!((v * 10.0).round() / 10.0));
                let reason = s["reason"].as_str().filter(|r| matches!(*r, "none" | "cpu" | "bandwidth" | "load" | "encoder")).unwrap_or("none");
                let encoder = s["encoder"].as_str().filter(|e| matches!(*e, "hw" | "sw")).unwrap_or("hw");
                self.tell(&hold, json!({"type": "sc_stats", "sc": sc, "fps": n("fps"), "w": n("w"), "h": n("h"), "kbps": n("kbps"), "encode_ms_p95": n("encode_ms_p95"), "dropped": n("dropped"), "reason": reason, "degraded": s["degraded"].as_bool() == Some(true), "level": n("level"), "encoder": encoder, "idr_ratio": n("idr_ratio"), "crisp": s["crisp"].as_bool()}));
            }
            Some("encoder") => {
                let encoder = params["encoder"].as_str().filter(|e| matches!(*e, "hw" | "sw")).unwrap_or("hw");
                self.tell(&hold, json!({"type": "sc_state", "sc": sc, "encoder": encoder, "state": "encoder"}));
            }
            Some("ended") => {
                let reason = params["reason"].as_str().and_then(|r| END_REASONS.iter().copied().find(|x| *x == r)).unwrap_or("app");
                self.end(d, sc, reason, false);
            }
            _ => {}
        }
    }

    /// A JSON message to the hold's phone, if it is connected; one that finds no room is dropped (the next state or stats replaces it).
    fn tell(&self, hold: &Hold, msg: Value) {
        if let Some((_, out, _)) = hold.state.lock().conn.as_ref() {
            let _ = out.try_send(msg.to_string());
        }
    }

    fn ask_key(&self, d: &Daemon, hold: &Hold, st: &mut State) {
        if !st.need_key {
            st.need_key = true;
            st.need_key_since = Instant::now();
        }
        if st.key_asked.is_none_or(|at| at.elapsed() >= KEY_GAP) {
            st.key_asked = Some(Instant::now());
            d.providers.notify("screen", "screen.keyframe", json!({"sc": hold.sc}));
        }
    }

    /// A video frame that does not go: the helper hears it (no acknowledgement will come for it), a keyframe is asked for, and the tiles are
    /// painted again (the frame may have carried their unpaint).
    fn drop_video(&self, d: &Daemon, hold: &Hold, st: &mut State, seq: u32) {
        st.dropped_through = st.dropped_through.max(seq);
        st.lost_tiles = true;
        self.ask_key(d, hold, st);
    }

    fn media(&self, d: &Daemon, hold: &Hold, bytes: Vec<u8>) {
        let Some(peek) = wire::peek(&bytes) else { return };
        let mut st = hold.state.lock();
        let Some((_, _, lane)) = st.conn.clone() else { return };
        if !hold.live() {
            return;
        }
        let video = peek.kind == KIND_VIDEO;
        if video && st.need_key && !peek.key {
            return self.drop_video(d, hold, &mut st, peek.seq);
        }
        if video && st.unacked + bytes.len() > MAX_UNACKED_BYTES && !st.sent.is_empty() {
            return self.drop_video(d, hold, &mut st, peek.seq);
        }
        // A tile or the cursor never takes the room a video frame needs: it goes only with room to spare, and a tile that does not is painted again.
        if !video && lane.capacity() < ROOM_FOR_VIDEO {
            if peek.kind == KIND_TILE {
                st.lost_tiles = true;
            }
            return;
        }
        let len = bytes.len();
        match lane.try_send(bytes) {
            Ok(()) => {
                st.raw += len as u64;
                st.wire += wire_len(len) as u64;
                if video {
                    st.sent.push_back((peek.seq, Instant::now(), len));
                    st.unacked += len;
                    if peek.key {
                        st.need_key = false;
                    }
                }
            }
            Err(_) if video => self.drop_video(d, hold, &mut st, peek.seq),
            Err(_) => {
                if peek.kind == KIND_TILE {
                    st.lost_tiles = true;
                }
            }
        }
    }

    /// What the helper is told after an acknowledgement, and once a second: what the phone has, what was dropped, how long it took, what the
    /// wire costs, the load.
    fn flow(&self, d: &Daemon, hold: &Hold, rtt_ms: u64) {
        let msg = {
            let mut st = hold.state.lock();
            let lost_tiles = std::mem::take(&mut st.lost_tiles);
            let (raw, wire) = (std::mem::take(&mut st.raw), std::mem::take(&mut st.wire));
            json!({"sc": hold.sc, "acked": st.acked, "dropped_through": st.dropped_through, "rtt_ms": rtt_ms, "level": st.level.number(), "lost_tiles": lost_tiles, "raw": raw, "wire": wire})
        };
        d.providers.notify("screen", "screen.flow", msg);
    }

    /// The phone's screen verbs. `Some` is a reply with content, `None` a plain ack.
    pub async fn phone_verb(&self, d: &Arc<Daemon>, phone: Phone<'_>, verb: &str, req: &Value) -> Result<Option<Value>, Refused> {
        match verb {
            "screens" => self.targets(d).await.map(Some),
            "sc_start" => self.start(d, &phone, req).await.map(Some),
            "sc_stop" => {
                let hold = self.held_here(&phone, req)?;
                self.end(d, &hold.sc, "phone", true);
                Ok(None)
            }
            "sc_ack" => {
                let hold = self.held_here(&phone, req)?;
                let seq = req["seq"].as_u64().filter(|s| *s <= u64::from(u32::MAX)).ok_or_else(|| refused("bad_request", "missing seq"))? as u32;
                let rtt = {
                    let mut st = hold.state.lock();
                    let mut rtt = None;
                    while let Some(&(s, at, len)) = st.sent.front() {
                        if s > seq {
                            break;
                        }
                        st.sent.pop_front();
                        st.unacked = st.unacked.saturating_sub(len);
                        if s == seq {
                            rtt = Some(at.elapsed().as_millis() as u64);
                        }
                    }
                    st.acked = st.acked.max(seq);
                    rtt
                };
                self.flow(d, &hold, rtt.unwrap_or(0));
                Ok(None)
            }
            "sc_input" => {
                let hold = self.held_here(&phone, req)?;
                let ops = kelpie_screen::input::checked_ops(&req["ops"]).ok_or_else(|| refused("bad_request", "not input a screen takes"))?;
                // Late input, input onto a stale picture, or more than a phone needs is dropped without a word: the phone already shows it as not live.
                let stale = Self::stale(&hold.state.lock());
                if !kelpie_screen::input::fresh(req["st"].as_u64(), now_millis()) || stale || !Self::input_allowed(&hold) {
                    // A release goes all the same, so a mouse button is never left pressed on the computer (keys go down and up in one message).
                    let releases: Vec<Value> = ops.into_iter().filter(|op| op["k"] == "up").collect();
                    if !releases.is_empty() && hold.live() {
                        d.providers.notify("screen", "screen.input", json!({"sc": hold.sc, "ops": releases, "t": now_millis()}));
                    }
                    return Ok(None);
                }
                if !hold.live() {
                    return Err(refused("sc_gone", "That screen has ended."));
                }
                hold.state.lock().last_input = Instant::now();
                d.providers.notify("screen", "screen.input", json!({"sc": hold.sc, "ops": ops, "t": now_millis()}));
                Ok(None)
            }
            "sc_text" => {
                let hold = self.held_here(&phone, req)?;
                let text = req["text"].as_str().and_then(kelpie_screen::input::checked_text).ok_or_else(|| refused("bad_request", "missing or too long text"))?;
                if !kelpie_screen::input::fresh(req["st"].as_u64(), now_millis()) {
                    return Err(refused("not_sent", "Not sent: it came too late. Check the screen, then type it again."));
                }
                hold.state.lock().last_input = Instant::now();
                if !hold.live() {
                    return Err(refused("sc_gone", "That screen has ended."));
                }
                match d.providers.request("screen", "screen.text", json!({"sc": hold.sc, "text": text, "t": now_millis()})).await {
                    Ok(_) => Ok(Some(json!({"type": "ack", "result": "sent"}))),
                    // The helper types only into the shared window, in front: anywhere else, nothing is typed, and the phone says so.
                    Err(e) if e.to_string().starts_with("not_front") => Err(refused("not_front", "Not typed: the shared window is not in front on the computer. Tap it, then type again.")),
                    Err(e) => Err(refused("not_sent", format!("Not sent: {e}. Check the screen, then type it again."))),
                }
            }
            "sc_key" => {
                let hold = self.held_here(&phone, req)?;
                let mut st = hold.state.lock();
                self.ask_key(d, &hold, &mut st);
                Ok(None)
            }
            "sc_resume" => {
                let hold = self.held_here(&phone, req)?;
                hold.state.lock().last_input = Instant::now();
                d.providers.notify("screen", "screen.resume_load", json!({"sc": hold.sc}));
                Ok(None)
            }
            "sc_rebind" => self.rebind(d, &phone, req).map(|()| None),
            _ => Err(refused("unknown_verb", format!("unknown request {verb:?}"))),
        }
    }

    async fn targets(&self, d: &Arc<Daemon>) -> Result<Value, Refused> {
        if d.providers.provider("screen").is_none() {
            return Err(refused("no_app", "Open Kelpie on the computer to see its screen."));
        }
        let got = d.providers.request_within("screen", "screen.targets", json!({}), START_LIMIT).await.map_err(|e| refused("no_app", format!("{e}")))?;
        let rows: Vec<Value> = got["targets"].as_array().map(|a| a.iter().take(kelpie_screen::targets::MAX_TARGETS).filter_map(target_row).collect()).unwrap_or_default();
        Ok(json!({"type": "ack", "result": "ok", "targets": rows}))
    }

    async fn start(&self, d: &Arc<Daemon>, phone: &Phone<'_>, req: &Value) -> Result<Value, Refused> {
        let target = req["target"].as_str().filter(|t| kelpie_screen::targets::TargetId::parse(t).is_some()).ok_or_else(|| refused("bad_request", "missing target"))?.to_string();
        let level = d.mobile.load.level();
        let free = d.mobile.load.memory_free();
        if level >= Level::L2 || free.is_some_and(|f| f < MIN_FREE_MEMORY) {
            return Err(refused("host_busy", "The computer is too busy to show its screen now. Try again in a moment."));
        }
        let Some(provider) = d.providers.provider("screen") else { return Err(refused("no_app", "Open Kelpie on the computer to see its screen.")) };
        let in_use = || refused("sc_taken", "The computer's screen is in use by another device.");
        if self.holds.lock().values().any(|h| h.device != phone.device) {
            return Err(in_use());
        }
        // Reserved for the whole start, through the app's answer: a second start, by any device, does not slip in meanwhile.
        let _starting = {
            let mut starting = self.starting.lock();
            if *starting {
                return Err(in_use());
            }
            *starting = true;
            Starting(self)
        };
        let mine: Vec<String> = self.holds.lock().values().map(|h| h.sc.clone()).collect();
        for sc in mine {
            self.end(d, &sc, "replaced", true);
        }
        let grant = d.mobile.grant(phone.device);
        let sc = uuid::Uuid::new_v4().simple().to_string();
        let params = json!({"sc": sc, "target": target, "device": phone.name, "level": level.number()});
        let started = match d.providers.request_within("screen", "screen.start", params, START_LIMIT).await {
            Ok(v) => v,
            Err(e) => {
                // It may yet start in the app after this gave up on it (a timeout): it is told to let go.
                d.providers.notify("screen", "screen.stop", json!({"sc": sc, "reason": "failed"}));
                return Err(refused("no_screen", format!("{e}")));
            }
        };
        if !grant.load(Ordering::SeqCst) {
            d.providers.notify("screen", "screen.stop", json!({"sc": sc, "reason": "revoked"}));
            return Err(refused("sc_gone", "Control was taken away."));
        }
        let now = Instant::now();
        let mut state = State {
            conn: Some((phone.conn, phone.out.clone(), phone.media.clone())),
            gone_at: None,
            sent: VecDeque::new(),
            unacked: 0,
            acked: 0,
            dropped_through: 0,
            need_key: true,
            need_key_since: now,
            key_asked: None,
            last_input: now,
            last_beat: now,
            lost_tiles: false,
            raw: 0,
            wire: 0,
            inputs: (now, 0),
            level,
            idle: IDLE_STOP,
            rebind: REBIND_WINDOW,
            version: 0,
        };
        // The link closed while the app was starting: the hold waits for the same device's next connection, like one whose link dropped.
        let link_gone = phone.out.is_closed() || phone.media.is_closed();
        let pause = link_gone.then(|| {
            Self::set_gone(&mut state);
            pause_notice(&mut state, &sc, true)
        });
        let hold = Arc::new(Hold { sc: sc.clone(), device: phone.device.to_string(), provider, grant, ended: AtomicBool::new(false), wake: Notify::new(), state: Mutex::new(state) });
        self.holds.lock().insert(sc.clone(), hold.clone());
        if let Some(params) = pause {
            d.providers.notify("screen", "screen.pause", params);
        }
        let title = short(started.get("title"), 80);
        let kind = if started["kind"] == "window" { "window" } else { "monitor" };
        // The program, never the window's title, which can name a document.
        let what = if kind == "window" { format!("window: {}", program(started.get("exe"))) } else { "monitor".to_string() };
        d.mobile.audit(phone.device, "screen_start", &what);
        tokio::spawn(watch(d.clone(), hold));
        let n = |k: &str| started[k].as_u64().unwrap_or(0).min(100_000);
        Ok(json!({"type": "ack", "result": "ok", "sc": sc, "kind": kind, "title": title, "w": n("w"), "h": n("h"), "encoder": if started["encoder"] == "sw" { "sw" } else { "hw" }}))
    }

    fn rebind(&self, d: &Daemon, phone: &Phone<'_>, req: &Value) -> Result<(), Refused> {
        let sc = req["sc"].as_str().unwrap_or_default();
        let hold = self.hold(sc).filter(|h| h.device == phone.device && h.live()).ok_or_else(|| refused("sc_gone", "That screen has ended."))?;
        let params = {
            let mut st = hold.state.lock();
            // The connection that holds it already (a hello after an unlock, on the same connection): nothing to do.
            if st.conn.as_ref().is_some_and(|(c, ..)| *c == phone.conn) {
                return Ok(());
            }
            let window = st.rebind;
            if st.conn.is_some() || st.gone_at.is_none_or(|at| at.elapsed() >= window) {
                return Err(refused("sc_gone", "That screen has ended."));
            }
            st.conn = Some((phone.conn, phone.out.clone(), phone.media.clone()));
            st.gone_at = None;
            st.sent.clear();
            st.unacked = 0;
            st.need_key = true;
            st.need_key_since = Instant::now();
            pause_notice(&mut st, sc, false)
        };
        // The helper goes on, with a keyframe first: never a frame from before the gap.
        d.providers.notify("screen", "screen.pause", params);
        hold.wake.notify_one();
        Ok(())
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.holds.lock().len()
    }
}

/// What a hold's task does on its own: lets go when its grant, its record, its link or its idle time runs out, says it is alive, and tells the
/// helper the flow and the load once a second.
async fn watch(d: Arc<Daemon>, hold: Arc<Hold>) {
    loop {
        tokio::select! {
            _ = hold.wake.notified() => {}
            _ = tokio::time::sleep(WATCH) => {}
        }
        if hold.ended.load(Ordering::SeqCst) {
            return;
        }
        // The grant first, then what it rests on as it is now: every path that takes control away clears the grant; this holds whatever a later one forgets.
        if !hold.grant.load(Ordering::SeqCst) || !d.mobile.control_held(&hold.device) {
            return d.screen.end(&d, &hold.sc, "revoked", true);
        }
        let now = Instant::now();
        let level = d.mobile.load.level();
        let (ending, beat) = {
            let mut st = hold.state.lock();
            st.level = level;
            if st.gone_at.is_some_and(|at| now.duration_since(at) >= st.rebind) {
                (Some("link_lost"), false)
            } else if now.duration_since(st.last_input) >= st.idle {
                (Some("idle"), false)
            } else if now.duration_since(st.last_beat) >= HEARTBEAT {
                st.last_beat = now;
                if let Some((_, out, _)) = st.conn.as_ref() {
                    let _ = out.try_send(json!({"type": "sc_fhb", "sc": hold.sc, "server_ts": now_millis()}).to_string());
                }
                (None, true)
            } else {
                (None, false)
            }
        };
        if let Some(reason) = ending {
            return d.screen.end(&d, &hold.sc, reason, true);
        }
        if beat {
            d.screen.flow(&d, &hold, 0);
        }
    }
}

#[cfg(test)]
mod tests;
