//! A phone's view of this computer's screen: a monitor or one window, as H.264, and the phone's input back.
//!
//! The helper `kelpie-screen` (spawned by the Kelpie app, which alone can: `provider` kind `screen`) captures and encodes; the app hands each
//! media message to this broker as `provider.event {sc, kind: "media", data}` (base64 on the local pipe), and the broker sends it to the phone
//! as a binary message on the phone's encrypted link (`mobile::lanes`, the media lane). JSON goes beside it: `sc_state`, `sc_stats`, `sc_fhb`,
//! `sc_end`. Like a browser pane's takeover (`takeover`), and with the same rules:
//!
//! - **A hold** is keyed by its device, the link connection and the provider's connection, rests on a grant flag (`Mobile::grant`) that a
//!   revoke, a lower permission, the app lock and phone access going off clear at once, and is checked by every input; its own task lets go
//!   within `WATCH`. A dropped link keeps it `REBIND_WINDOW` for the same device's next connection only. One screen at a time, for one device.
//! - **Flow:** the link is TCP, so nothing is lost and nothing should pile up. The helper encodes only while fewer than 3 frames wait for the
//!   phone's acknowledgement (`sc_ack`, forwarded with its round trip as `screen.flow`). Should the lane still be full, or more than
//!   `MAX_UNACKED_BYTES` wait, video is dropped up to the next keyframe, which is asked for at most once a second: a decoder given a frame
//!   whose reference never came would show garbage.
//! - **Input** older than 1 s by the computer's clock is dropped, and so is input while the phone has not acknowledged anything for
//!   `STALE_AFTER` (it cannot see what it would click). Text goes with an answer of its own. None of it is logged: the audit log has the start
//!   (with what was shared) and the end.
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
use crate::takeover::{Refused, IDLE_STOP, REBIND_WINDOW};

const WATCH: Duration = Duration::from_millis(250);
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// Media sent and not acknowledged, at most.
pub const MAX_UNACKED_BYTES: usize = 512 * 1024;
/// With nothing acknowledged this long while frames wait, input is not sent.
pub const STALE_AFTER: Duration = Duration::from_secs(2);
const KEY_GAP: Duration = Duration::from_secs(1);
/// Starting the helper and its first look can take a while on a busy machine.
const START_LIMIT: Duration = Duration::from_secs(15);
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
    /// Video is dropped until a keyframe, which was asked for.
    need_key: bool,
    key_asked: Option<Instant>,
    last_ack: Instant,
    last_input: Instant,
    last_beat: Instant,
    /// A tile could not be sent: the helper paints every still cell again.
    lost_tiles: bool,
    level: Level,
    idle: Duration,
    rebind: Duration,
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
}

fn short(v: Option<&Value>, n: usize) -> String {
    v.and_then(Value::as_str).unwrap_or_default().chars().filter(|c| !c.is_control()).take(n).collect()
}

fn end_message(sc: &str, reason: &str) -> String {
    json!({"type": "sc_end", "sc": sc, "reason": reason, "server_ts": now_millis()}).to_string()
}

/// Why a stream ended, as the phone knows the words.
const END_REASONS: &[&str] = &["hotkey", "closed", "desktop", "encoder_failed", "app_closed", "replaced"];

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

    /// Ends hold `sc`: the phone and the app are told (the app only when it did not end it itself), and the audit log has it.
    pub fn end(&self, d: &Daemon, sc: &str, reason: &str, tell_app: bool) {
        let Some(hold) = self.holds.lock().remove(sc) else { return };
        if hold.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        hold.wake.notify_one();
        if let Some((_, out, _)) = hold.state.lock().conn.take() {
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
        let gone: Vec<String> = self
            .holds
            .lock()
            .values()
            .filter(|h| {
                let mut st = h.state.lock();
                if !st.conn.as_ref().is_some_and(|(c, ..)| *c == conn) {
                    return false;
                }
                st.conn = None;
                st.gone_at = Some(Instant::now());
                st.sent.clear();
                st.unacked = 0;
                st.need_key = true;
                true
            })
            .map(|h| h.sc.clone())
            .collect();
        for sc in gone {
            d.providers.notify("screen", "screen.pause", json!({"sc": sc, "paused": true}));
        }
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
                let state = params["state"].as_str().filter(|s| matches!(*s, "live" | "locked" | "secure_desktop" | "closed" | "minimized" | "elevated" | "covered" | "paused_load" | "blocked"));
                if let Some(state) = state {
                    self.tell(&hold, json!({"type": "sc_state", "sc": sc, "state": state, "server_ts": now_millis()}));
                }
            }
            Some("stats") => {
                let s = &params["stats"];
                let n = |k: &str| s[k].as_f64().filter(|v| v.is_finite()).map_or(json!(null), |v| json!((v * 10.0).round() / 10.0));
                let reason = s["reason"].as_str().filter(|r| matches!(*r, "none" | "cpu" | "bandwidth" | "load" | "encoder")).unwrap_or("none");
                let encoder = s["encoder"].as_str().filter(|e| matches!(*e, "hw" | "sw")).unwrap_or("hw");
                self.tell(&hold, json!({"type": "sc_stats", "sc": sc, "fps": n("fps"), "w": n("w"), "h": n("h"), "kbps": n("kbps"), "encode_ms_p95": n("encode_ms_p95"), "dropped": n("dropped"), "reason": reason, "degraded": s["degraded"].as_bool() == Some(true), "level": n("level"), "encoder": encoder, "idr_ratio": n("idr_ratio")}));
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
        st.need_key = true;
        if st.key_asked.is_none_or(|at| at.elapsed() >= KEY_GAP) {
            st.key_asked = Some(Instant::now());
            d.providers.notify("screen", "screen.keyframe", json!({"sc": hold.sc}));
        }
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
            self.ask_key(d, hold, &mut st);
            return;
        }
        if video && st.unacked + bytes.len() > MAX_UNACKED_BYTES && !st.sent.is_empty() {
            self.ask_key(d, hold, &mut st);
            return;
        }
        let len = bytes.len();
        match lane.try_send(bytes) {
            Ok(()) if video => {
                st.sent.push_back((peek.seq, Instant::now(), len));
                st.unacked += len;
                if peek.key {
                    st.need_key = false;
                }
            }
            Ok(()) => {}
            Err(_) if video => self.ask_key(d, hold, &mut st),
            Err(_) => {
                if peek.kind == KIND_TILE {
                    st.lost_tiles = true;
                }
            }
        }
    }

    /// What the helper is told after an acknowledgement, and once a second: what the phone has, how long it took, the load.
    fn flow(&self, d: &Daemon, hold: &Hold, rtt_ms: u64) {
        let (acked, level, lost_tiles) = {
            let mut st = hold.state.lock();
            (st.acked, st.level.number(), std::mem::take(&mut st.lost_tiles))
        };
        d.providers.notify("screen", "screen.flow", json!({"sc": hold.sc, "acked": acked, "rtt_ms": rtt_ms, "level": level, "lost_tiles": lost_tiles}));
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
                    st.last_ack = Instant::now();
                    rtt
                };
                self.flow(d, &hold, rtt.unwrap_or(0));
                Ok(None)
            }
            "sc_input" => {
                let hold = self.held_here(&phone, req)?;
                let ops = kelpie_screen::input::checked_ops(&req["ops"]).ok_or_else(|| refused("bad_request", "not input a screen takes"))?;
                // Late input, or input while the phone sees nothing new, is dropped without a word: the phone already shows it as not live.
                let stale = {
                    let st = hold.state.lock();
                    !st.sent.is_empty() && st.last_ack.elapsed() >= STALE_AFTER
                };
                if !kelpie_screen::input::fresh(req["st"].as_u64(), now_millis()) || stale {
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
        let free = d.mobile.load.report()["memory_free"].as_f64();
        if level >= Level::L2 || free.is_some_and(|f| f < MIN_FREE_MEMORY) {
            return Err(refused("host_busy", "The computer is too busy to show its screen now. Try again in a moment."));
        }
        let Some(provider) = d.providers.provider("screen") else { return Err(refused("no_app", "Open Kelpie on the computer to see its screen.")) };
        if self.holds.lock().values().any(|h| h.device != phone.device) {
            return Err(refused("sc_taken", "Another device is looking at this computer's screen."));
        }
        let mine: Vec<String> = self.holds.lock().values().map(|h| h.sc.clone()).collect();
        for sc in mine {
            self.end(d, &sc, "replaced", true);
        }
        let grant = d.mobile.grant(phone.device);
        let sc = uuid::Uuid::new_v4().simple().to_string();
        let params = json!({"sc": sc, "target": target, "device": phone.name, "level": level.number()});
        let started = d.providers.request_within("screen", "screen.start", params, START_LIMIT).await.map_err(|e| refused("no_screen", format!("{e}")))?;
        if !grant.load(Ordering::SeqCst) {
            d.providers.notify("screen", "screen.stop", json!({"sc": sc, "reason": "revoked"}));
            return Err(refused("sc_gone", "Control was taken away."));
        }
        let now = Instant::now();
        let hold = Arc::new(Hold {
            sc: sc.clone(),
            device: phone.device.to_string(),
            provider,
            grant,
            ended: AtomicBool::new(false),
            wake: Notify::new(),
            state: Mutex::new(State {
                conn: Some((phone.conn, phone.out.clone(), phone.media.clone())),
                gone_at: None,
                sent: VecDeque::new(),
                unacked: 0,
                acked: 0,
                need_key: true,
                key_asked: None,
                last_ack: now,
                last_input: now,
                last_beat: now,
                lost_tiles: false,
                level,
                idle: IDLE_STOP,
                rebind: REBIND_WINDOW,
            }),
        });
        self.holds.lock().insert(sc.clone(), hold.clone());
        let title = short(started.get("title"), 80);
        let kind = if started["kind"] == "window" { "window" } else { "monitor" };
        d.mobile.audit(phone.device, "screen_start", &format!("{kind}: {title}"));
        tokio::spawn(watch(d.clone(), hold));
        let n = |k: &str| started[k].as_u64().unwrap_or(0).min(100_000);
        Ok(json!({"type": "ack", "result": "ok", "sc": sc, "kind": kind, "title": title, "w": n("w"), "h": n("h"), "encoder": if started["encoder"] == "sw" { "sw" } else { "hw" }}))
    }

    fn rebind(&self, d: &Daemon, phone: &Phone<'_>, req: &Value) -> Result<(), Refused> {
        let sc = req["sc"].as_str().unwrap_or_default();
        let hold = self.hold(sc).filter(|h| h.device == phone.device && h.live()).ok_or_else(|| refused("sc_gone", "That screen has ended."))?;
        {
            let mut st = hold.state.lock();
            let window = st.rebind;
            if st.conn.is_some() || st.gone_at.is_none_or(|at| at.elapsed() >= window) {
                return Err(refused("sc_gone", "That screen has ended."));
            }
            st.conn = Some((phone.conn, phone.out.clone(), phone.media.clone()));
            st.gone_at = None;
            st.sent.clear();
            st.unacked = 0;
            st.need_key = true;
            st.last_ack = Instant::now();
        }
        // The helper goes on, with a keyframe first: never a frame from before the gap.
        d.providers.notify("screen", "screen.pause", json!({"sc": sc, "paused": false}));
        hold.wake.notify_one();
        Ok(())
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.holds.lock().len()
    }
}

/// What a hold's task does on its own: lets go when its grant, its link or its idle time runs out, says it is alive, and tells the helper
/// the load once a second.
async fn watch(d: Arc<Daemon>, hold: Arc<Hold>) {
    loop {
        tokio::select! {
            _ = hold.wake.notified() => {}
            _ = tokio::time::sleep(WATCH) => {}
        }
        if hold.ended.load(Ordering::SeqCst) {
            return;
        }
        if !hold.grant.load(Ordering::SeqCst) {
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
mod tests {
    use super::*;
    use crate::auth::Role;
    use kelpie_screen::geom::Rect;
    use kelpie_screen::wire::Video;

    struct Rig {
        d: Arc<Daemon>,
        out: mpsc::Sender<String>,
        media: mpsc::Sender<Vec<u8>>,
        phone: mpsc::Receiver<String>,
        frames: mpsc::Receiver<Vec<u8>>,
        notes: Arc<Mutex<Vec<Value>>>,
    }

    const APP: u64 = 901;

    fn rig_with(lane: usize) -> Rig {
        let dir = std::env::temp_dir().join(format!("kelpie-screen-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Daemon::with_secrets(dir, std::path::PathBuf::from("kelpied"), crate::notify::Memory::new());
        let (tx, mut rx) = crate::session::client_channel();
        d.providers.register(APP, &Role::App, "screen", None, &tx).unwrap();
        let notes = Arc::new(Mutex::new(Vec::new()));
        let (app, kept) = (d.clone(), notes.clone());
        tokio::spawn(async move {
            while let Some(o) = rx.recv().await {
                let m: Value = serde_json::from_slice(&o.data[5..]).unwrap();
                if let Some(id) = m["preq"].as_u64() {
                    let result = match m["method"].as_str() {
                        Some("screen.targets") => json!({"targets": [
                            {"id": r"m:\\.\DISPLAY1", "kind": "monitor", "title": "Screen 1", "w": 2560, "h": 1440, "primary": true},
                            {"id": "w:66:7", "kind": "window", "title": "main.rs \u{7}— kelpie", "app": "VS Code", "shortcut": "ide", "w": 1200, "h": 800},
                            {"id": "nonsense", "kind": "window", "title": "x"},
                            {"id": "w:1:2", "kind": "toaster", "title": "x"},
                        ]}),
                        Some("screen.start") => json!({"kind": "window", "title": "main.rs — kelpie", "w": 1200, "h": 800, "encoder": "hw"}),
                        _ => json!(true),
                    };
                    app.providers.reply(APP, &json!({"preq": id, "result": result}));
                } else {
                    kept.lock().push(m);
                }
            }
        });
        let (out, phone) = mpsc::channel(32);
        let (media, frames) = mpsc::channel(lane);
        Rig { d, out, media, phone, frames, notes }
    }

    fn rig() -> Rig {
        rig_with(8)
    }

    fn video(seq: u32, key: bool, size: usize) -> Vec<u8> {
        let mut au = vec![0, 0, 0, 1, if key { 0x65 } else { 0x41 }];
        au.resize(size.max(5), 0x11);
        wire::video(&Video { key, seq, capture_ms: 0, src: (1200, 800), coded: (1200, 800), unpaint: vec![], unpaint_all: false }, &au)
    }

    impl Rig {
        fn phone(&self) -> Phone<'_> {
            Phone { device: "phone", name: "Phone", conn: 1, out: &self.out, media: &self.media }
        }

        async fn verb(&self, verb: &str, req: Value) -> Result<Option<Value>, Refused> {
            self.d.screen.phone_verb(&self.d, self.phone(), verb, &req).await
        }

        async fn start(&self) -> String {
            let reply = self.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap().unwrap();
            reply["sc"].as_str().unwrap().to_string()
        }

        fn send(&self, sc: &str, bytes: &[u8]) {
            self.d.screen.on_event(&self.d, APP, &json!({"sc": sc, "kind": "media", "data": base64::engine::general_purpose::STANDARD.encode(bytes)}));
        }

        async fn next(&mut self, kind: &str) -> Option<Value> {
            let end = Instant::now() + Duration::from_secs(3);
            while Instant::now() < end {
                match tokio::time::timeout(Duration::from_millis(100), self.phone.recv()).await {
                    Ok(Some(m)) => {
                        let v: Value = serde_json::from_str(&m).unwrap();
                        if v["type"] == kind {
                            return Some(v);
                        }
                    }
                    Ok(None) => return None,
                    Err(_) => {}
                }
            }
            None
        }

        fn frames(&mut self) -> Vec<wire::Peek> {
            let mut out = Vec::new();
            while let Ok(f) = self.frames.try_recv() {
                out.push(wire::peek(&f).unwrap());
            }
            out
        }

        async fn told_now(&self, method: &str) -> Vec<Value> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.told(method)
        }

        fn told(&self, method: &str) -> Vec<Value> {
            self.notes.lock().iter().filter(|n| n["pnote"] == method).map(|n| n["params"].clone()).collect()
        }
    }

    #[tokio::test]
    async fn targets_come_from_the_app_checked_and_the_stream_goes_on_the_media_lane() {
        let mut r = rig();
        let listed = r.verb("screens", json!({})).await.unwrap().unwrap();
        let rows = listed["targets"].as_array().unwrap();
        assert_eq!(rows.len(), 2, "unknown ids and kinds are left out: {rows:?}");
        assert_eq!((rows[1]["title"].as_str(), rows[1]["shortcut"].as_str()), (Some("main.rs — kelpie"), Some("ide")));
        let sc = r.start().await;
        r.send(&sc, &video(1, true, 2000));
        r.send(&sc, &video(2, false, 500));
        r.send(&sc, &wire::tile(2, 1, Rect::new(0, 0, 64, 64), wire::FORMAT_PNG, b"\x89PNG..."));
        r.send(&sc, &wire::cursor(wire::Shape::Arrow, true, 5, 5));
        let got = r.frames();
        assert_eq!(got.iter().map(|p| (p.kind, p.seq)).collect::<Vec<_>>(), [(KIND_VIDEO, 1), (KIND_VIDEO, 2), (KIND_TILE, 2), (wire::KIND_CURSOR, 0)]);
        // The acknowledgement goes to the helper with its round trip.
        r.verb("sc_ack", json!({"sc": sc, "seq": 2})).await.unwrap();
        let flow = r.told_now("screen.flow").await;
        assert_eq!(flow.last().unwrap()["acked"], 2);
        assert!(r.next("sc_fhb").await.is_some(), "a heartbeat, frames or not");
        // State and stats pass through, with only what the phone knows.
        r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "state", "state": "elevated"}));
        assert_eq!(r.next("sc_state").await.unwrap()["state"], "elevated");
        r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "state", "state": "<script>"}));
        r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "stats", "stats": {"fps": 15, "w": 1280, "h": 720, "reason": "load", "degraded": true, "secret": "x"}}));
        let stats = r.next("sc_stats").await.unwrap();
        assert_eq!((stats["fps"].as_f64(), stats["reason"].as_str(), stats.get("secret")), (Some(15.0), Some("load"), None));
        // Input goes to the app with the computer's clock; stop from the phone tells the app; the audit log has the start, the end, nothing typed.
        r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 10, "y": 20}], "st": now_millis()})).await.unwrap();
        let input = r.told_now("screen.input").await;
        assert_eq!(input[0]["ops"][0]["b"], "left");
        r.verb("sc_stop", json!({"sc": sc})).await.unwrap();
        assert_eq!(r.next("sc_end").await.unwrap()["reason"], "phone");
        assert_eq!(r.told_now("screen.stop").await.len(), 1);
        let audit = r.d.mobile.audit_entries(20).to_string();
        assert!(audit.contains("screen_start") && audit.contains("window: main.rs") && audit.contains("screen_stop") && !audit.contains("\"x\""), "{audit}");
    }

    #[tokio::test]
    async fn video_waits_for_its_first_keyframe() {
        let mut r = rig();
        let sc = r.start().await;
        r.send(&sc, &video(1, false, 100));
        assert!(r.frames().is_empty(), "a decoder cannot start from a P-frame");
        assert_eq!(r.told_now("screen.keyframe").await.len(), 1, "a keyframe is asked for");
        r.send(&sc, &video(2, false, 100));
        assert_eq!(r.told_now("screen.keyframe").await.len(), 1, "at most one a second");
        r.send(&sc, &video(3, true, 100));
        r.send(&sc, &video(4, false, 100));
        assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [3, 4]);
    }

    #[tokio::test]
    async fn a_full_lane_drops_video_up_to_the_next_keyframe() {
        let mut r = rig_with(2);
        let sc = r.start().await;
        for seq in 1..=4 {
            r.send(&sc, &video(seq, seq == 1, 100));
        }
        assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [1, 2], "the lane held two");
        r.send(&sc, &video(5, false, 100));
        assert!(r.frames().is_empty(), "after a drop, nothing until a keyframe: frame 5 refers to 3 and 4");
        r.send(&sc, &video(6, true, 100));
        assert_eq!(r.frames().iter().map(|p| (p.seq, p.key)).collect::<Vec<_>>(), [(6, true)]);
        assert!(!r.told_now("screen.keyframe").await.is_empty());
    }

    #[tokio::test]
    async fn no_more_than_half_a_megabyte_waits_for_the_phone() {
        let mut r = rig_with(64);
        let sc = r.start().await;
        r.send(&sc, &video(1, true, 300 * 1024));
        r.send(&sc, &video(2, false, 300 * 1024));
        assert_eq!(r.frames().len(), 1, "the second would pass the bound");
        r.verb("sc_ack", json!({"sc": sc, "seq": 1})).await.unwrap();
        r.send(&sc, &video(3, true, 300 * 1024));
        assert_eq!(r.frames().iter().map(|p| p.seq).collect::<Vec<_>>(), [3]);
    }

    #[tokio::test]
    async fn a_revoke_stops_input_at_once_and_ends_the_screen_within_a_second() {
        let mut r = rig();
        let sc = r.start().await;
        let at = std::time::Instant::now();
        let _ = r.d.mobile.revoke("phone");
        let late = r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 1, "y": 1}], "st": now_millis()})).await;
        assert_eq!(late.unwrap_err().code, "sc_gone", "the grant is checked on every input");
        assert_eq!(r.next("sc_end").await.expect("told")["reason"], "revoked");
        assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
        assert!(r.told_now("screen.input").await.is_empty());
        assert_eq!(r.told("screen.stop").len(), 1, "the helper stops");
        assert_eq!(r.d.screen.count(), 0);
    }

    #[tokio::test]
    async fn a_screen_nobody_touches_ends_after_ten_minutes() {
        let mut r = rig();
        let sc = r.start().await;
        {
            let hold = r.d.screen.hold(&sc).unwrap();
            hold.state.lock().idle = Duration::from_millis(1200);
            hold.wake.notify_one();
        }
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis()})).await.unwrap();
        }
        assert_eq!(r.d.screen.count(), 1, "input starts the time over");
        assert_eq!(r.next("sc_end").await.expect("told")["reason"], "idle");
    }

    #[tokio::test]
    async fn a_dropped_link_pauses_the_helper_and_keeps_the_screen_for_the_same_device() {
        let mut r = rig();
        let sc = r.start().await;
        r.send(&sc, &video(1, true, 100));
        r.frames();
        r.d.screen.link_closed(&r.d, 1);
        assert_eq!(r.told_now("screen.pause").await[0]["paused"], true);
        let (out2, mut phone2) = mpsc::channel(8);
        let (media2, mut frames2) = mpsc::channel(8);
        let other = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2, media: &media2 };
        assert_eq!(r.d.screen.phone_verb(&r.d, other, "sc_rebind", &json!({"sc": sc})).await.unwrap_err().code, "sc_gone", "another device cannot take it");
        let back = Phone { device: "phone", name: "Phone", conn: 3, out: &out2, media: &media2 };
        r.d.screen.phone_verb(&r.d, back, "sc_rebind", &json!({"sc": sc})).await.unwrap();
        assert_eq!(r.told_now("screen.pause").await.last().unwrap()["paused"], false);
        // Never a frame from before the gap: a P-frame waits for the keyframe the helper sends first.
        r.send(&sc, &video(2, false, 100));
        r.send(&sc, &video(3, true, 100));
        assert_eq!(wire::peek(&frames2.try_recv().unwrap()).unwrap().seq, 3);
        assert!(tokio::time::timeout(Duration::from_secs(2), phone2.recv()).await.is_ok(), "the heartbeat goes to the new connection");
        // Gone for longer than the window: ended.
        {
            let hold = r.d.screen.hold(&sc).unwrap();
            hold.state.lock().rebind = Duration::from_millis(600);
        }
        r.d.screen.link_closed(&r.d, 3);
        tokio::time::sleep(Duration::from_millis(600) + WATCH * 3).await;
        assert_eq!(r.d.screen.count(), 0);
    }

    #[tokio::test]
    async fn late_or_blind_input_is_dropped_and_only_input_a_screen_takes_goes() {
        let r = rig();
        let sc = r.start().await;
        let old = now_millis() - 1500;
        r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}], "st": old})).await.unwrap();
        r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}]})).await.unwrap();
        assert!(r.told_now("screen.input").await.is_empty(), "a click after a gap never clicks");
        assert_eq!(r.verb("sc_text", json!({"sc": sc, "text": "hunter2", "st": old})).await.unwrap_err().code, "not_sent");
        for bad in [json!([]), json!([{"k": "eval"}]), json!([{"k": "key", "key": "LWin"}]), json!([{"k": "move", "x": -1, "y": 0}])] {
            assert_eq!(r.verb("sc_input", json!({"sc": sc, "ops": bad, "st": now_millis()})).await.unwrap_err().code, "bad_request", "{bad}");
        }
        let sent = r.verb("sc_text", json!({"sc": sc, "text": "hunter2", "st": now_millis()})).await.unwrap().unwrap();
        assert_eq!(sent["result"], "sent");
        // Frames that the phone has not acknowledged for two seconds: it is looking at an old picture.
        r.send(&sc, &video(1, true, 100));
        r.d.screen.hold(&sc).unwrap().state.lock().last_ack = Instant::now() - STALE_AFTER;
        r.verb("sc_input", json!({"sc": sc, "ops": [{"k": "click", "x": 5, "y": 5}], "st": now_millis()})).await.unwrap();
        assert!(r.told_now("screen.input").await.is_empty());
        let other = Phone { device: "phone", name: "Phone", conn: 9, out: &r.out, media: &r.media };
        assert_eq!(r.d.screen.phone_verb(&r.d, other, "sc_input", &json!({"sc": sc, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis()})).await.unwrap_err().code, "sc_elsewhere");
    }

    #[tokio::test]
    async fn a_busy_computer_starts_no_screen_one_device_at_a_time_and_none_without_the_app() {
        let r = rig();
        r.d.mobile.load.set_for_test(Level::L2);
        assert_eq!(r.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap_err().code, "host_busy");
        r.d.mobile.load.set_for_test(Level::L0);
        assert_eq!(r.verb("sc_start", json!({"target": "C:\\Windows"})).await.unwrap_err().code, "bad_request");
        let first = r.start().await;
        let (out2, _rx) = mpsc::channel(4);
        let (media2, _frames) = mpsc::channel(4);
        let tablet = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2, media: &media2 };
        assert_eq!(r.d.screen.phone_verb(&r.d, tablet, "sc_start", &json!({"target": "w:66:7"})).await.unwrap_err().code, "sc_taken");
        let second = r.start().await;
        assert_ne!(first, second);
        assert_eq!(r.d.screen.count(), 1, "the same device's new screen replaces its old one");
        r.d.screen.provider_gone(&r.d, APP);
        assert_eq!(r.d.screen.count(), 0, "the app went: its screens end");
        r.d.providers.client_gone(APP);
        assert_eq!(r.verb("screens", json!({})).await.unwrap_err().code, "no_app");
        assert_eq!(r.verb("sc_start", json!({"target": "w:66:7"})).await.unwrap_err().code, "no_app");
    }

    #[tokio::test]
    async fn only_the_hold_s_own_provider_is_heard_and_what_it_ends_is_ended() {
        let mut r = rig();
        let sc = r.start().await;
        r.d.screen.on_event(&r.d, APP + 1, &json!({"sc": sc, "kind": "ended", "reason": "hotkey"}));
        assert_eq!(r.d.screen.count(), 1, "another connection cannot end it");
        r.d.screen.on_event(&r.d, APP, &json!({"sc": sc, "kind": "ended", "reason": "hotkey"}));
        assert_eq!(r.next("sc_end").await.unwrap()["reason"], "hotkey");
        assert!(r.told_now("screen.stop").await.is_empty(), "the helper ended it itself");
        assert!(Broker::owns_event(&json!({"sc": "x"})) && !Broker::owns_event(&json!({"tk": "x"})));
    }
}
