//! Browser pane takeover: a phone with `control` sees one of the app's browser panes and drives it (a login, an OAuth consent, a 2FA code).
//!
//! The app streams the pane with the DevTools protocol (`Page.startScreencast`) and dispatches the phone's input (`Input.*`); it is reached through
//! the provider channel (`provider`), and only the verified app can be one. This broker sits between, and holds what decides who drives:
//!
//! - **A hold** is keyed by its device, the link connection it was taken on and the provider's connection, and it is the unit of control: one
//!   per pane (the app keeps the pane's popups with it), at most one per device. It rests on a grant flag (`Mobile::grant`) that a revoke, a
//!   lower permission, the app lock and phone access going off clear at once: input checks the flag every time, and the hold's own task lets go
//!   of the pane within `WATCH`. A link that drops keeps the hold for `REBIND_WINDOW`, for the same device's next connection only (`tk_rebind`
//!   after its handshake); a provider that goes ends every hold it had.
//! - **Frames** are never queued: the broker keeps the newest one and sends it when the phone has credit (`CREDIT` frames unacknowledged), at the
//!   rate of the ladder's step. The step follows the acknowledgements' round trip (`Ladder`) and the machine's load. A phone that acknowledges
//!   nothing for `PAUSE_AFTER` pauses the screencast; its next acknowledgement resumes it with a fresh frame.
//! - **Input** older than `INPUT_TTL_MS` by the computer's clock is dropped: a tap that arrives after a gap must not click a page that changed.
//!   Text goes with an answer of its own, and is never sent twice. Neither is written to the audit log, a log file or a transcript: the audit log
//!   has the start and the end of a hold, nothing in between.
//!
//! Takeover over the relay is not offered (the connection's `control` is never granted there, `mobile::transport_cap`).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::{mpsc, Notify};
use tokio::time::Instant;

use crate::daemon::Daemon;
use crate::load::Level;
use crate::session::now_millis;

/// A hold nobody typed into for this long ends.
pub const IDLE_STOP: Duration = Duration::from_secs(600);
/// How long a dropped link keeps its hold for the same device's next connection.
pub const REBIND_WINDOW: Duration = Duration::from_secs(15);
/// Input older than this by the computer's clock is dropped.
pub const INPUT_TTL_MS: u64 = 1000;
/// Frames on their way to the phone and not yet acknowledged.
pub const CREDIT: u32 = 2;
/// With frames unacknowledged this long, the screencast pauses.
pub const PAUSE_AFTER: Duration = Duration::from_secs(2);
/// The phone hears the hold is alive this often, frames or not.
pub const HEARTBEAT: Duration = Duration::from_secs(1);
/// How often a hold looks at its grant, its link and its clock.
const WATCH: Duration = Duration::from_millis(250);
/// The largest frame (base64) that goes: one link message holds 1 MiB (`kelpie_link::MAX_APP_MESSAGE`), with room for the rest of the message.
pub const MAX_FRAME_B64: usize = 700 * 1024;
/// Input events in one message, and text in one.
pub const MAX_OPS: usize = 32;
/// Input messages a hold takes in a second; the rest are dropped (a phone does not need more, and the app's queue stays short).
pub const INPUT_PER_SECOND: u32 = 60;
pub const MAX_TEXT: usize = 4096;
/// The screencast's own rate, from which `everyNthFrame` is worked out.
const SOURCE_FPS: u8 = 60;

/// Keys the phone may press: what a login form needs. Anything else is typed as text.
pub const KEYS: &[&str] = &["Enter", "Tab", "Backspace", "Delete", "Escape", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Space"];

/// How a frame is made: JPEG quality, the most it is wide, and how many go a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub quality: u8,
    pub width: u16,
    pub fps: u8,
}

/// Fewer frames first, then smaller ones, then a lower quality, never under q40.
pub const STEPS: [Step; 6] = [
    Step { quality: 70, width: 1280, fps: 10 },
    Step { quality: 70, width: 1280, fps: 5 },
    Step { quality: 70, width: 1280, fps: 2 },
    Step { quality: 55, width: 1024, fps: 2 },
    Step { quality: 45, width: 800, fps: 2 },
    Step { quality: 40, width: 800, fps: 2 },
];

impl Step {
    /// What the machine's load allows at most.
    pub fn capped(self, level: Level) -> Step {
        let cap = match level {
            Level::L0 => return self,
            Level::L1 => Step { quality: 60, width: 1280, fps: 8 },
            Level::L2 => Step { quality: 50, width: 960, fps: 4 },
            Level::L3 => Step { quality: 40, width: 800, fps: 2 },
        };
        Step { quality: self.quality.min(cap.quality), width: self.width.min(cap.width), fps: self.fps.min(cap.fps) }
    }

    fn every_nth(self) -> u8 {
        (SOURCE_FPS / self.fps.max(1)).max(1)
    }

    fn gap(self) -> Duration {
        Duration::from_millis(1000 / u64::from(self.fps.max(1)))
    }
}

/// Where on `STEPS` a hold is, from the round trips of its acknowledgements: down when the median of the last 3 is over 400 ms, up when the median
/// of the last 10 is under 150 ms and the step has held for 5 s.
#[derive(Debug)]
pub struct Ladder {
    at: usize,
    rtts: VecDeque<Duration>,
    since: Instant,
}

const DOWN_RTT: Duration = Duration::from_millis(400);
const UP_RTT: Duration = Duration::from_millis(150);
const UP_DWELL: Duration = Duration::from_secs(5);

fn median(samples: impl Iterator<Item = Duration>) -> Duration {
    let mut v: Vec<Duration> = samples.collect();
    v.sort();
    v.get(v.len() / 2).copied().unwrap_or_default()
}

impl Ladder {
    pub fn new(now: Instant) -> Ladder {
        Ladder { at: 0, rtts: VecDeque::new(), since: now }
    }

    pub fn step(&self) -> Step {
        STEPS[self.at]
    }

    /// One acknowledgement's round trip.
    pub fn sample(&mut self, rtt: Duration, now: Instant) {
        self.rtts.push_back(rtt);
        while self.rtts.len() > 10 {
            self.rtts.pop_front();
        }
        let n = self.rtts.len();
        if n >= 3 && median(self.rtts.iter().skip(n - 3).copied()) > DOWN_RTT && self.at + 1 < STEPS.len() {
            self.move_to(self.at + 1, now);
        } else if n >= 10 && median(self.rtts.iter().copied()) < UP_RTT && now.duration_since(self.since) >= UP_DWELL && self.at > 0 {
            self.move_to(self.at - 1, now);
        }
    }

    /// A frame too large to send: the next step with smaller frames.
    pub fn too_big(&mut self, now: Instant) {
        let width = self.step().width;
        if let Some(next) = (self.at + 1..STEPS.len()).find(|&i| STEPS[i].width < width).or((self.at + 1 < STEPS.len()).then_some(STEPS.len() - 1)) {
            self.move_to(next, now);
        }
    }

    fn move_to(&mut self, at: usize, now: Instant) {
        self.at = at;
        self.since = now;
        self.rtts.clear();
    }
}

/// A request that was not carried out: `code` for programs, `message` for people.
#[derive(Debug)]
pub struct Refused {
    pub code: &'static str,
    pub message: String,
}

fn refused(code: &'static str, message: impl Into<String>) -> Refused {
    Refused { code, message: message.into() }
}

/// The phone connection a verb came on.
pub struct Phone<'a> {
    pub device: &'a str,
    pub name: &'a str,
    pub conn: u64,
    /// Where its messages go (the connection's bulk lane).
    pub out: &'a mpsc::Sender<String>,
}

struct Frame {
    data: String,
    meta: Value,
}

struct State {
    /// The link connection the hold is on, and its lane; none while a dropped link may come back.
    conn: Option<(u64, mpsc::Sender<String>)>,
    gone_at: Option<Instant>,
    credit: u32,
    /// The newest frame not sent yet.
    pending: Option<Frame>,
    seq: u64,
    /// Frames sent and not acknowledged: number, when.
    sent: VecDeque<(u64, Instant)>,
    last_sent: Option<Instant>,
    last_input: Instant,
    last_beat: Instant,
    paused: bool,
    ladder: Ladder,
    /// What the app was last told to make.
    applied: Step,
    /// `IDLE_STOP` and `REBIND_WINDOW` (tests shorten them).
    idle: Duration,
    rebind: Duration,
    /// Input messages taken in the current second, and when it began.
    inputs: (Instant, u32),
    /// Times the hold's task went round its loop (a paused hold must not spin).
    wakes: u64,
}

pub struct Hold {
    tk: String,
    device: String,
    pane: String,
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
    /// Panes and devices whose start is under way: a second start for either is refused at once.
    starting: Mutex<Vec<String>>,
}

/// A start under way, for a pane and a device, until it is dropped.
struct Starting<'a> {
    broker: &'a Broker,
    keys: [String; 2],
}

impl Drop for Starting<'_> {
    fn drop(&mut self) {
        self.broker.starting.lock().retain(|k| !self.keys.contains(k));
    }
}

/// What the app is told a hold should make now.
fn settings(tk: &str, step: Step, paused: bool, fresh: bool) -> Value {
    json!({"tk": tk, "quality": step.quality, "max_width": step.width, "every_nth": step.every_nth(), "paused": paused, "fresh": fresh})
}

fn end_message(tk: &str, reason: &str) -> String {
    json!({"type": "tk_end", "tk": tk, "reason": reason, "server_ts": now_millis()}).to_string()
}

fn number(v: &Value, key: &str, max: f64) -> Option<f64> {
    v.get(key).and_then(Value::as_f64).filter(|n| n.is_finite() && n.abs() <= max)
}

/// The phone's input, as the app takes it: a closed set of events with numbers in range, CSS pixels of the page's viewport. Anything else is refused whole.
pub fn checked_ops(ops: &Value) -> Option<Vec<Value>> {
    let ops = ops.as_array().filter(|o| !o.is_empty() && o.len() <= MAX_OPS)?;
    ops.iter()
        .map(|op| {
            let kind = op.get("k").and_then(Value::as_str)?;
            let at = || Some((number(op, "x", 20_000.0).filter(|x| *x >= 0.0)?, number(op, "y", 20_000.0).filter(|y| *y >= 0.0)?));
            Some(match kind {
                "tap" | "down" | "move" | "up" => {
                    let (x, y) = at()?;
                    json!({"k": kind, "x": x, "y": y})
                }
                "wheel" => {
                    let (x, y) = at()?;
                    json!({"k": "wheel", "x": x, "y": y, "dx": number(op, "dx", 10_000.0)?, "dy": number(op, "dy", 10_000.0)?})
                }
                "key" => {
                    let key = op.get("key").and_then(Value::as_str).filter(|k| KEYS.contains(k))?;
                    json!({"k": "key", "key": key})
                }
                _ => return None,
            })
        })
        .collect()
}

/// Whether input stamped `st` (the phone's reading of the computer's clock) is still worth doing.
fn fresh(st: Option<u64>, now: u64) -> bool {
    st.is_some_and(|st| st <= now + INPUT_TTL_MS && now.saturating_sub(st) <= INPUT_TTL_MS)
}

fn short(v: Option<&Value>, n: usize) -> String {
    v.and_then(Value::as_str).unwrap_or_default().chars().filter(|c| !c.is_control()).take(n).collect()
}

impl Broker {
    fn hold(&self, tk: &str) -> Option<Arc<Hold>> {
        self.holds.lock().get(tk).cloned()
    }

    /// The hold `tk`, if it is this device's, still live, and on this connection.
    fn held_here(&self, phone: &Phone<'_>, req: &Value) -> Result<Arc<Hold>, Refused> {
        let tk = req["tk"].as_str().unwrap_or_default();
        let hold = self.hold(tk).filter(|h| h.device == phone.device).ok_or_else(|| refused("tk_gone", "That takeover has ended."))?;
        if !hold.live() {
            return Err(refused("tk_gone", "That takeover has ended."));
        }
        if hold.state.lock().conn.as_ref().map(|(c, _)| *c) != Some(phone.conn) {
            return Err(refused("tk_elsewhere", "That takeover is on another connection."));
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

    /// Ends hold `tk`: the phone and the app are told (the app only when it did not end it itself), and the audit log has it.
    pub fn end(&self, d: &Daemon, tk: &str, reason: &'static str, tell_app: bool) {
        let Some(hold) = self.holds.lock().remove(tk) else { return };
        if hold.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        hold.wake.notify_one();
        if let Some((_, out)) = hold.state.lock().conn.take() {
            // Frames may fill the lane: the end waits for room a while rather than being lost.
            if let Err(mpsc::error::TrySendError::Full(msg)) = out.try_send(end_message(tk, reason)) {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    rt.spawn(async move {
                        let _ = tokio::time::timeout(Duration::from_secs(5), out.send(msg)).await;
                    });
                }
            }
        }
        if tell_app {
            d.providers.notify("browser", "browser.takeover_stop", json!({"tk": tk, "reason": reason}));
        }
        d.mobile.audit(&hold.device, "takeover_stop", reason);
    }

    /// The link connection `conn` closed: its holds wait for the same device's next one.
    pub fn link_closed(&self, conn: u64) {
        for hold in self.holds.lock().values() {
            let mut st = hold.state.lock();
            if st.conn.as_ref().is_some_and(|(c, _)| *c == conn) {
                st.conn = None;
                st.gone_at = Some(Instant::now());
                st.credit = 0;
                st.sent.clear();
            }
        }
    }

    /// The provider's connection is gone: every hold it had ends.
    pub fn provider_gone(&self, d: &Daemon, client: u64) {
        let gone: Vec<String> = self.holds.lock().values().filter(|h| h.provider == client).map(|h| h.tk.clone()).collect();
        for tk in gone {
            self.end(d, &tk, "app_closed", false);
        }
    }

    /// `provider.event` from `client`: a frame, or a hold the app ended. Only the hold's own provider is heard.
    pub fn on_event(&self, d: &Daemon, client: u64, params: &Value) {
        let tk = params["tk"].as_str().unwrap_or_default();
        let Some(hold) = self.hold(tk).filter(|h| h.provider == client) else { return };
        match params["kind"].as_str() {
            Some("frame") => {
                let Some(data) = params["data"].as_str() else { return };
                let meta = params.get("meta").filter(|m| m.is_object()).cloned().unwrap_or_else(|| json!({}));
                let mut st = hold.state.lock();
                // Paused, a frame that still comes is not kept: the resume asks for a fresh one, and this one would be older.
                if st.paused {
                    return;
                }
                if data.len() > MAX_FRAME_B64 {
                    st.ladder.too_big(Instant::now());
                    drop(st);
                    hold.wake.notify_one();
                    return;
                }
                st.pending = Some(Frame { data: data.to_string(), meta });
                drop(st);
                hold.wake.notify_one();
            }
            Some("ended") => {
                let reason = match params["reason"].as_str() {
                    Some("desktop") => "desktop",
                    Some("closed") => "closed",
                    _ => "app",
                };
                self.end(d, tk, reason, false);
            }
            _ => {}
        }
    }

    /// The phone's takeover verbs. `Some` is a reply with content, `None` a plain ack.
    pub async fn phone_verb(&self, d: &Arc<Daemon>, phone: Phone<'_>, verb: &str, req: &Value) -> Result<Option<Value>, Refused> {
        match verb {
            "panes" => self.panes(d, phone.device).await.map(Some),
            "tk_start" => self.start(d, &phone, req).await.map(Some),
            "tk_stop" => {
                let hold = self.held_here(&phone, req)?;
                self.end(d, &hold.tk, "phone", true);
                Ok(None)
            }
            "tk_ack" => {
                let hold = self.held_here(&phone, req)?;
                let seq = req["seq"].as_u64().ok_or_else(|| refused("bad_request", "missing seq"))?;
                self.ack(d, &hold, seq);
                Ok(None)
            }
            "tk_input" => {
                let hold = self.held_here(&phone, req)?;
                let ops = checked_ops(&req["ops"]).ok_or_else(|| refused("bad_request", "not input a page takes"))?;
                // Which window the phone mapped it against (`window_id` of its frame): the app refuses input meant for another.
                let window = req["w"].as_u64().ok_or_else(|| refused("bad_request", "missing w"))?;
                let st = req["st"].as_u64().unwrap_or_default();
                // Late input is dropped without a word: the phone already shows it as not live, and a paused hold says so in its heartbeat. A release goes
                // all the same (the app lets it through too), so a button is never left pressed in the page.
                if !fresh(Some(st), now_millis()) || hold.state.lock().paused || !Self::input_allowed(&hold) {
                    let releases: Vec<Value> = ops.into_iter().filter(|op| op["k"] == "up").collect();
                    if !releases.is_empty() && hold.live() {
                        d.providers.notify("browser", "browser.takeover_input", json!({"tk": hold.tk, "ops": releases, "w": window, "st": st}));
                    }
                    return Ok(None);
                }
                if !hold.live() {
                    return Err(refused("tk_gone", "That takeover has ended."));
                }
                hold.state.lock().last_input = Instant::now();
                // `st` goes on, so that the app drops what waited too long in its own queue.
                d.providers.notify("browser", "browser.takeover_input", json!({"tk": hold.tk, "ops": ops, "w": window, "st": st}));
                Ok(None)
            }
            "tk_text" => {
                let hold = self.held_here(&phone, req)?;
                let text = req["text"].as_str().filter(|t| !t.is_empty() && t.chars().count() <= MAX_TEXT).ok_or_else(|| refused("bad_request", "missing or too long text"))?;
                let window = req["w"].as_u64().ok_or_else(|| refused("bad_request", "missing w"))?;
                let st = req["st"].as_u64().unwrap_or_default();
                if !fresh(Some(st), now_millis()) {
                    return Err(refused("not_sent", "Not sent: it came too late. Check the page, then type it again."));
                }
                hold.state.lock().last_input = Instant::now();
                // The flag again right before it goes: a revoke that came in meanwhile wins.
                if !hold.live() {
                    return Err(refused("tk_gone", "That takeover has ended."));
                }
                // The app types it in the queue of the taps before it, so it never overtakes them.
                match d.providers.request("browser", "browser.takeover_text", json!({"tk": hold.tk, "text": text, "w": window, "st": st})).await {
                    Ok(_) => Ok(Some(json!({"type": "ack", "result": "sent"}))),
                    Err(_) => Err(refused("not_sent", "Not sent. Check the page, then type it again.")),
                }
            }
            "tk_rebind" => self.rebind(d, &phone, req).map(|()| None),
            _ => Err(refused("unknown_verb", format!("unknown request {verb:?}"))),
        }
    }

    async fn panes(&self, d: &Arc<Daemon>, device: &str) -> Result<Value, Refused> {
        let got = d.providers.request("browser", "browser.panes", json!({})).await.map_err(|e| refused("no_app", format!("{e}")))?;
        // A pane this device holds is not shown as taken to it: after a reload it may take it again.
        let held: Vec<String> = self.holds.lock().values().filter(|h| h.device != device).map(|h| h.pane.clone()).collect();
        let panes: Vec<Value> = got["panes"]
            .as_array()
            .map(|a| a.iter().take(64).filter_map(|p| {
                let pane = p["pane"].as_str().filter(|s| !s.is_empty() && s.len() <= 64)?;
                Some(json!({"pane": pane, "title": short(p.get("title"), 120), "host": short(p.get("host"), 120), "project": short(p.get("project"), 120), "project_name": short(p.get("project_name"), 120), "held": held.iter().any(|h| h == pane)}))
            }).collect())
            .unwrap_or_default();
        Ok(json!({"type": "ack", "result": "ok", "panes": panes}))
    }

    async fn start(&self, d: &Arc<Daemon>, phone: &Phone<'_>, req: &Value) -> Result<Value, Refused> {
        let pane = req["pane"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).ok_or_else(|| refused("bad_request", "missing pane"))?.to_string();
        let level = d.mobile.load.level();
        if level >= Level::L2 {
            return Err(refused("host_busy", "The computer is too busy to start a takeover now. Try again in a moment."));
        }
        let Some(provider) = d.providers.provider("browser") else { return Err(refused("no_app", "The Kelpie app is not open on the computer.")) };
        if self.holds.lock().values().any(|h| h.pane == pane && h.device != phone.device) {
            return Err(refused("tk_taken", "Another device has taken over that pane."));
        }
        // Held for the whole start, through the app's answer: a second start of this pane, or by this device, does not slip in meanwhile.
        let _starting = {
            let keys = [format!("pane {pane}"), format!("device {}", phone.device)];
            let mut starting = self.starting.lock();
            if starting.iter().any(|k| keys.contains(k)) {
                return Err(refused("tk_taken", "That pane, or this device, is starting a takeover already."));
            }
            starting.extend(keys.iter().cloned());
            Starting { broker: self, keys }
        };
        // One pane at a time: the one this device held before ends.
        let mine: Vec<String> = self.holds.lock().values().filter(|h| h.device == phone.device).map(|h| h.tk.clone()).collect();
        for tk in mine {
            self.end(d, &tk, "phone", true);
        }
        let grant = d.mobile.grant(phone.device);
        let tk = uuid::Uuid::new_v4().simple().to_string();
        let now = Instant::now();
        let ladder = Ladder::new(now);
        let step = ladder.step().capped(level);
        let mut params = settings(&tk, step, false, true);
        params["pane"] = json!(pane);
        params["device"] = json!(phone.name);
        let started = match d.providers.request("browser", "browser.takeover_start", params).await {
            Ok(v) => v,
            Err(e) => {
                // It may yet start in the app after this gave up on it (a timeout): it is told to let go.
                d.providers.notify("browser", "browser.takeover_stop", json!({"tk": tk, "reason": "failed"}));
                return Err(refused("no_pane", format!("{e}")));
            }
        };
        if !grant.load(Ordering::SeqCst) {
            d.providers.notify("browser", "browser.takeover_stop", json!({"tk": tk, "reason": "revoked"}));
            return Err(refused("tk_gone", "Control was taken away."));
        }
        let hold = Arc::new(Hold {
            tk: tk.clone(),
            device: phone.device.to_string(),
            pane: pane.clone(),
            provider,
            grant,
            ended: AtomicBool::new(false),
            wake: Notify::new(),
            state: Mutex::new(State { conn: Some((phone.conn, phone.out.clone())), gone_at: None, credit: CREDIT, pending: None, seq: 0, sent: VecDeque::new(), last_sent: None, last_input: now, last_beat: now, paused: false, ladder, applied: step, idle: IDLE_STOP, rebind: REBIND_WINDOW, inputs: (now, 0), wakes: 0 }),
        });
        self.holds.lock().insert(tk.clone(), hold.clone());
        d.mobile.audit(phone.device, "takeover_start", &short(started.get("host"), 120));
        tokio::spawn(watch(d.clone(), hold));
        Ok(json!({"type": "ack", "result": "ok", "tk": tk, "title": short(started.get("title"), 120), "host": short(started.get("host"), 120)}))
    }

    fn ack(&self, d: &Daemon, hold: &Hold, seq: u64) {
        let now = Instant::now();
        let resume = {
            let mut st = hold.state.lock();
            while let Some(&(s, at)) = st.sent.front() {
                if s > seq {
                    break;
                }
                st.sent.pop_front();
                if s == seq {
                    st.ladder.sample(now.duration_since(at), now);
                }
            }
            let was_paused = std::mem::replace(&mut st.paused, false);
            if was_paused {
                // Whatever is still counted as on its way (a frame the phone never showed, or one it acknowledges under an older number) is written off:
                // the resume brings a fresh frame, and the phone may have it at once.
                st.sent.clear();
            }
            st.credit = CREDIT.saturating_sub(st.sent.len() as u32);
            was_paused.then(|| st.ladder.step().capped(d.mobile.load.level()))
        };
        if let Some(step) = resume {
            hold.state.lock().applied = step;
            d.providers.notify("browser", "browser.takeover_set", settings(&hold.tk, step, false, true));
        }
        hold.wake.notify_one();
    }

    fn rebind(&self, d: &Daemon, phone: &Phone<'_>, req: &Value) -> Result<(), Refused> {
        let tk = req["tk"].as_str().unwrap_or_default();
        let hold = self.hold(tk).filter(|h| h.device == phone.device && h.live()).ok_or_else(|| refused("tk_gone", "That takeover has ended."))?;
        let step = {
            let mut st = hold.state.lock();
            // The connection that holds it already (a hello after an unlock, on the same connection): nothing to do.
            if st.conn.as_ref().is_some_and(|(c, _)| *c == phone.conn) {
                return Ok(());
            }
            let window = st.rebind;
            if st.conn.is_some() || st.gone_at.is_none_or(|at| at.elapsed() >= window) {
                return Err(refused("tk_gone", "That takeover has ended."));
            }
            st.conn = Some((phone.conn, phone.out.clone()));
            st.gone_at = None;
            st.credit = CREDIT;
            st.sent.clear();
            st.pending = None;
            st.paused = false;
            st.applied = st.ladder.step().capped(d.mobile.load.level());
            st.applied
        };
        // A fresh frame, never one from before the gap.
        d.providers.notify("browser", "browser.takeover_set", settings(&hold.tk, step, false, true));
        hold.wake.notify_one();
        Ok(())
    }

    #[cfg(test)]
    fn count(&self) -> usize {
        self.holds.lock().len()
    }
}

/// What a hold's task does on its own: lets go when its grant, its link or its idle time runs out, sends the newest frame when the phone
/// has credit and the step's rate allows, says it is alive, pauses a phone that does not acknowledge, and follows the load.
async fn watch(d: Arc<Daemon>, hold: Arc<Hold>) {
    loop {
        let wait = {
            let mut st = hold.state.lock();
            st.wakes += 1;
            match (&st.pending, st.last_sent) {
                // Paused, a frame that waits is not due: the hold sleeps until something changes.
                (Some(_), Some(at)) if st.credit > 0 && !st.paused => (at + st.applied.gap()).saturating_duration_since(Instant::now()).min(WATCH),
                _ => WATCH,
            }
        };
        tokio::select! {
            _ = hold.wake.notified() => {}
            _ = tokio::time::sleep(wait) => {}
        }
        if hold.ended.load(Ordering::SeqCst) {
            return;
        }
        // The grant first, then what it rests on as it is now: the record, and for a page its lock. Every path that takes control away clears the
        // grant; this holds whatever a later one forgets.
        if !hold.grant.load(Ordering::SeqCst) || !d.mobile.control_held(&hold.device) {
            return d.takeover.end(&d, &hold.tk, "revoked", true);
        }
        let now = Instant::now();
        let level = d.mobile.load.level();
        let mut tell = None;
        let ending = {
            let mut st = hold.state.lock();
            if st.gone_at.is_some_and(|at| now.duration_since(at) >= st.rebind) {
                Some("link_lost")
            } else if now.duration_since(st.last_input) >= st.idle {
                Some("idle")
            } else {
                let step = st.ladder.step().capped(level);
                if let Some((_, out)) = st.conn.clone() {
                    let due = st.last_sent.is_none_or(|at| now.duration_since(at) >= st.applied.gap());
                    if st.credit > 0 && due && !st.paused {
                        if let Some(frame) = st.pending.take() {
                            st.seq += 1;
                            let msg = json!({"type": "tk_frame", "tk": hold.tk, "seq": st.seq, "data": frame.data, "meta": frame.meta, "server_ts": now_millis()}).to_string();
                            // A lane with no room drops it: the next frame replaces it anyway.
                            if out.try_send(msg).is_ok() {
                                st.credit -= 1;
                                let seq = st.seq;
                                st.sent.push_back((seq, now));
                                st.last_sent = Some(now);
                            }
                        }
                    }
                    if now.duration_since(st.last_beat) >= HEARTBEAT {
                        st.last_beat = now;
                        // `paused`: the phone's input is not taken now, until it acknowledges (it may have a frame it could not show).
                        let _ = out.try_send(json!({"type": "tk_fhb", "tk": hold.tk, "seq": st.seq, "paused": st.paused, "server_ts": now_millis()}).to_string());
                    }
                    if !st.paused && st.sent.front().is_some_and(|(_, at)| now.duration_since(*at) >= PAUSE_AFTER) {
                        st.paused = true;
                        st.pending = None;
                        tell = Some(settings(&hold.tk, step, true, false));
                    }
                } else if !st.paused {
                    // The link is down (waiting for the same device's next connection): the app stops making frames nobody can take. The rebind resumes it.
                    st.paused = true;
                    st.pending = None;
                    tell = Some(settings(&hold.tk, step, true, false));
                }
                if tell.is_none() && !st.paused && step != st.applied {
                    st.applied = step;
                    tell = Some(settings(&hold.tk, step, false, false));
                }
                None
            }
        };
        if let Some(reason) = ending {
            return d.takeover.end(&d, &hold.tk, reason, true);
        }
        if let Some(params) = tell {
            d.providers.notify("browser", "browser.takeover_set", params);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Role;

    /// A daemon with an app that provides browser panes (answering as `answer` says), and a phone connection's lane.
    struct Rig {
        d: Arc<Daemon>,
        out: mpsc::Sender<String>,
        phone: mpsc::Receiver<String>,
        /// What the app was told without a request.
        notes: Arc<Mutex<Vec<Value>>>,
    }

    const APP: u64 = 900;

    /// How the app answers a start.
    #[derive(Clone, Copy, PartialEq)]
    enum Start {
        Ok,
        Fail,
        /// After this many milliseconds.
        Slow(u64),
    }

    fn rig() -> Rig {
        rig_with(Start::Ok)
    }

    fn rig_with(start: Start) -> Rig {
        let dir = std::env::temp_dir().join(format!("kelpie-takeover-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Daemon::with_secrets(dir, std::path::PathBuf::from("kelpied"), crate::notify::Memory::new());
        // Devices given control, as the store has them: a hold rests on its record as well as on its grant.
        d.mobile.add_test_controller("phone");
        d.mobile.add_test_controller("tablet");
        let (tx, mut rx) = crate::session::client_channel();
        d.providers.register(APP, &Role::App, "browser", None, &tx).unwrap();
        let notes = Arc::new(Mutex::new(Vec::new()));
        let (app, kept) = (d.clone(), notes.clone());
        tokio::spawn(async move {
            while let Some(o) = rx.recv().await {
                let m: Value = serde_json::from_slice(&o.data[5..]).unwrap();
                if let Some(id) = m["preq"].as_u64() {
                    let (app, method) = (app.clone(), m["method"].as_str().unwrap_or_default().to_string());
                    tokio::spawn(async move {
                        let answer = match method.as_str() {
                            "browser.panes" => json!({"result": {"panes": [{"pane": "browser-a", "title": "Sign in", "host": "accounts.example", "project": "p1", "project_name": "Shop"}]}}),
                            "browser.takeover_start" => match start {
                                Start::Fail => json!({"error": {"message": "no such pane"}}),
                                Start::Slow(ms) => {
                                    tokio::time::sleep(Duration::from_millis(ms)).await;
                                    json!({"result": {"title": "Sign in", "host": "accounts.example"}})
                                }
                                Start::Ok => json!({"result": {"title": "Sign in", "host": "accounts.example"}}),
                            },
                            _ => json!({"result": true}),
                        };
                        let mut reply = answer;
                        reply["preq"] = json!(id);
                        app.providers.reply(APP, &reply);
                    });
                } else {
                    kept.lock().push(m);
                }
            }
        });
        let (out, phone) = mpsc::channel(16);
        Rig { d, out, phone, notes }
    }

    impl Rig {
        fn phone(&self) -> Phone<'_> {
            Phone { device: "phone", name: "Phone", conn: 1, out: &self.out }
        }

        async fn verb(&self, verb: &str, req: Value) -> Result<Option<Value>, Refused> {
            self.d.takeover.phone_verb(&self.d, self.phone(), verb, &req).await
        }

        async fn start(&self) -> String {
            let reply = self.verb("tk_start", json!({"pane": "browser-a"})).await.unwrap().unwrap();
            reply["tk"].as_str().unwrap().to_string()
        }

        fn frame(&self, tk: &str, data: &str) {
            self.d.takeover.on_event(&self.d, APP, &json!({"kind": "frame", "tk": tk, "data": data, "meta": {"deviceWidth": 800, "deviceHeight": 600}}));
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

        /// What the app was told, once its task has taken it.
        async fn told_now(&self, method: &str) -> Vec<Value> {
            tokio::time::sleep(Duration::from_millis(50)).await;
            self.told(method)
        }

        fn told(&self, method: &str) -> Vec<Value> {
            self.notes.lock().iter().filter(|n| n["pnote"] == method).map(|n| n["params"].clone()).collect()
        }
    }

    #[tokio::test]
    async fn panes_come_from_the_app_and_a_takeover_streams_the_newest_frame_on_credit() {
        let mut r = rig();
        let panes = r.verb("panes", json!({})).await.unwrap().unwrap();
        assert_eq!(panes["panes"][0]["host"], "accounts.example");
        assert_eq!(panes["panes"][0]["held"], false);
        let tk = r.start().await;
        assert_eq!(r.verb("panes", json!({})).await.unwrap().unwrap()["panes"][0]["held"], false, "its own hold is not taken from it");
        let (out2, _rx2) = mpsc::channel(4);
        let tablet = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2 };
        assert_eq!(r.d.takeover.phone_verb(&r.d, tablet, "panes", &json!({})).await.unwrap().unwrap()["panes"][0]["held"], true, "another device sees it taken");
        r.frame(&tk, "one");
        let first = r.next("tk_frame").await.expect("a frame");
        assert_eq!((first["seq"].as_u64(), first["data"].as_str(), first["meta"]["deviceWidth"].as_u64()), (Some(1), Some("one"), Some(800)));
        // Frames that come while the phone has no credit are not queued: the newest replaces the others.
        r.frame(&tk, "two");
        r.next("tk_frame").await.expect("the second credit");
        for data in ["three", "four", "five"] {
            r.frame(&tk, data);
        }
        tokio::time::sleep(Duration::from_millis(400)).await;
        r.verb("tk_ack", json!({"tk": tk, "seq": 2})).await.unwrap();
        let next = r.next("tk_frame").await.expect("one more once acknowledged");
        assert_eq!((next["seq"].as_u64(), next["data"].as_str()), (Some(3), Some("five")));
        assert!(r.next("tk_fhb").await.is_some(), "a heartbeat, frames or not");
        // Stop from the phone: the app is told, and the audit log has the start and the end and nothing typed.
        r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 10, "y": 20}], "st": now_millis(), "w": 1})).await.unwrap();
        r.verb("tk_stop", json!({"tk": tk})).await.unwrap();
        assert_eq!(r.next("tk_end").await.unwrap()["reason"], "phone");
        assert_eq!(r.told_now("browser.takeover_stop").await.len(), 1);
        assert_eq!(r.d.takeover.count(), 0);
        let audit = r.d.mobile.audit_entries(20).to_string();
        assert!(audit.contains("takeover_start") && audit.contains("takeover_stop") && !audit.contains("\"x\""), "{audit}");
    }

    #[tokio::test]
    async fn a_revoke_ends_the_hold_within_a_second_and_input_stops_at_once() {
        let mut r = rig();
        let tk = r.start().await;
        let at = std::time::Instant::now();
        let _ = r.d.mobile.revoke("phone");
        let late = r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 1, "y": 1}], "st": now_millis(), "w": 1})).await;
        assert_eq!(late.unwrap_err().code, "tk_gone", "the grant is checked on every input");
        assert_eq!(r.next("tk_end").await.expect("told")["reason"], "revoked");
        assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
        assert!(r.told_now("browser.takeover_input").await.is_empty());
        assert_eq!(r.told("browser.takeover_stop").len(), 1, "the app lets go of the pane");
        assert_eq!(r.d.takeover.count(), 0);
    }

    /// Shortens a hold's idle time and the window a dropped link keeps it for.
    fn shorten(r: &Rig, tk: &str, idle: Duration, rebind: Duration) {
        let hold = r.d.takeover.hold(tk).unwrap();
        {
            let mut st = hold.state.lock();
            st.idle = idle;
            st.rebind = rebind;
        }
        hold.wake.notify_one();
    }

    #[tokio::test]
    async fn a_hold_nobody_types_into_ends_after_ten_minutes() {
        let mut r = rig();
        let tk = r.start().await;
        shorten(&r, &tk, Duration::from_millis(1500), REBIND_WINDOW);
        for _ in 0..4 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "key", "key": "Tab"}], "st": now_millis(), "w": 1})).await.unwrap();
        }
        assert_eq!(r.d.takeover.count(), 1, "typing starts the time over");
        assert_eq!(r.next("tk_end").await.expect("told")["reason"], "idle");
        assert_eq!(r.told_now("browser.takeover_stop").await[0]["reason"], "idle");
    }

    #[tokio::test]
    async fn a_dropped_link_keeps_the_hold_for_the_same_device_for_a_while() {
        let r = rig();
        let tk = r.start().await;
        r.d.takeover.link_closed(1);
        let (out2, mut phone2) = mpsc::channel(16);
        let other = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2 };
        assert_eq!(r.d.takeover.phone_verb(&r.d, other, "tk_rebind", &json!({"tk": tk})).await.unwrap_err().code, "tk_gone", "another device cannot take it");
        let back = Phone { device: "phone", name: "Phone", conn: 3, out: &out2 };
        r.d.takeover.phone_verb(&r.d, back, "tk_rebind", &json!({"tk": tk})).await.unwrap();
        assert!(r.told_now("browser.takeover_set").await.iter().any(|s| s["fresh"] == true), "a fresh frame after the gap");
        r.frame(&tk, "after");
        let v: Value = serde_json::from_str(&tokio::time::timeout(Duration::from_secs(2), phone2.recv()).await.unwrap().unwrap()).unwrap();
        assert_eq!(v["type"], "tk_frame");
        // Gone for longer than the window: released.
        shorten(&r, &tk, IDLE_STOP, Duration::from_millis(800));
        r.d.takeover.link_closed(3);
        tokio::time::sleep(WATCH).await;
        assert_eq!(r.d.takeover.count(), 1, "within the window it waits");
        tokio::time::sleep(Duration::from_millis(800) + WATCH * 2).await;
        assert_eq!(r.d.takeover.count(), 0);
    }

    #[tokio::test]
    async fn the_app_going_away_ends_its_holds() {
        let mut r = rig();
        r.start().await;
        r.d.takeover.provider_gone(&r.d, APP + 1);
        assert_eq!(r.d.takeover.count(), 1, "only the hold's own provider counts");
        r.d.takeover.provider_gone(&r.d, APP);
        assert_eq!(r.next("tk_end").await.unwrap()["reason"], "app_closed");
    }

    #[tokio::test]
    async fn late_input_is_dropped_and_only_input_a_page_takes_goes() {
        let r = rig();
        let tk = r.start().await;
        let old = now_millis() - 1500;
        r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 5, "y": 5}], "st": old, "w": 1})).await.unwrap();
        r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 5, "y": 5}], "w": 1})).await.unwrap();
        assert!(r.told("browser.takeover_input").is_empty(), "a tap after a gap never clicks");
        assert_eq!(r.verb("tk_text", json!({"tk": tk, "text": "hunter2", "st": old, "w": 1})).await.unwrap_err().code, "not_sent");
        for bad in [json!([]), json!([{"k": "eval", "x": 1, "y": 1}]), json!([{"k": "tap", "x": -1, "y": 1}]), json!([{"k": "tap", "x": "1", "y": 1}]), json!([{"k": "key", "key": "F12"}]), json!(vec![json!({"k": "tap", "x": 1, "y": 1}); MAX_OPS + 1])] {
            assert_eq!(r.verb("tk_input", json!({"tk": tk, "ops": bad, "st": now_millis(), "w": 1})).await.unwrap_err().code, "bad_request", "{bad}");
        }
        r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "wheel", "x": 5, "y": 5, "dx": 0, "dy": 120}, {"k": "key", "key": "Enter"}], "st": now_millis(), "w": 1})).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(r.told("browser.takeover_input").len(), 1);
        let sent = r.verb("tk_text", json!({"tk": tk, "text": "hunter2", "st": now_millis(), "w": 1})).await.unwrap().unwrap();
        assert_eq!(sent["result"], "sent");
        let other = Phone { device: "phone", name: "Phone", conn: 9, out: &r.out };
        assert_eq!(r.d.takeover.phone_verb(&r.d, other, "tk_input", &json!({"tk": tk, "ops": [{"k": "tap", "x": 1, "y": 1}], "st": now_millis(), "w": 1})).await.unwrap_err().code, "tk_elsewhere");
    }

    #[tokio::test]
    async fn a_busy_computer_starts_no_takeover_and_a_pane_has_one_holder() {
        let r = rig();
        r.d.mobile.load.set_for_test(Level::L2);
        assert_eq!(r.verb("tk_start", json!({"pane": "browser-a"})).await.unwrap_err().code, "host_busy");
        r.d.mobile.load.set_for_test(Level::L0);
        r.start().await;
        let (out2, _rx) = mpsc::channel(4);
        let tablet = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2 };
        assert_eq!(r.d.takeover.phone_verb(&r.d, tablet, "tk_start", &json!({"pane": "browser-a"})).await.unwrap_err().code, "tk_taken");
    }

    #[test]
    fn a_frame_that_goes_fits_one_link_message_and_a_larger_one_steps_down() {
        let biggest = json!({"type": "tk_frame", "tk": "f".repeat(32), "seq": u64::MAX, "data": "A".repeat(MAX_FRAME_B64), "meta": {"deviceWidth": 1e9, "deviceHeight": 1e9, "pageScaleFactor": 1.0, "offsetTop": 0, "scrollOffsetX": 1e9, "scrollOffsetY": 1e9, "window": "popup"}, "server_ts": u64::MAX, "req": "r999999"}).to_string();
        assert!(biggest.len() < kelpie_link::MAX_APP_MESSAGE, "{}", biggest.len());
        let now = Instant::now();
        let mut ladder = Ladder::new(now);
        ladder.too_big(now);
        assert!(ladder.step().width < STEPS[0].width, "smaller frames next");
        for _ in 0..10 {
            ladder.too_big(now);
        }
        assert_eq!(ladder.step(), *STEPS.last().unwrap());
        assert!(STEPS.iter().all(|s| s.quality >= 40));
    }

    #[tokio::test]
    async fn an_oversized_frame_is_not_sent() {
        let mut r = rig();
        let tk = r.start().await;
        r.frame(&tk, &"A".repeat(MAX_FRAME_B64 + 1));
        r.frame(&tk, "small");
        assert_eq!(r.next("tk_frame").await.unwrap()["data"], "small");
    }

    #[test]
    fn the_ladder_drops_the_rate_first_and_climbs_back_slowly() {
        let t0 = Instant::now();
        let mut ladder = Ladder::new(t0);
        for _ in 0..3 {
            ladder.sample(Duration::from_millis(500), t0);
        }
        assert_eq!(ladder.step(), STEPS[1], "fewer frames before smaller ones");
        for _ in 0..10 {
            ladder.sample(Duration::from_millis(50), t0 + Duration::from_secs(1));
        }
        assert_eq!(ladder.step(), STEPS[1], "not up before the step has held 5 s");
        for _ in 0..10 {
            ladder.sample(Duration::from_millis(50), t0 + Duration::from_secs(7));
        }
        assert_eq!(ladder.step(), STEPS[0]);
        assert_eq!(STEPS[0].capped(Level::L2), Step { quality: 50, width: 960, fps: 4 });
        assert_eq!(STEPS[5].capped(Level::L1), STEPS[5], "a cap never raises");
        assert_eq!((STEPS[0].every_nth(), STEPS[2].every_nth()), (6, 30));
    }

    #[tokio::test]
    async fn input_goes_with_the_window_it_was_meant_for_and_its_clock_and_only_so_much_of_it() {
        let r = rig();
        let tk = r.start().await;
        let st = now_millis();
        assert_eq!(r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 1, "y": 1}], "st": st})).await.unwrap_err().code, "bad_request", "no window: refused");
        r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 1, "y": 1}], "st": st, "w": 7})).await.unwrap();
        let sent = r.told_now("browser.takeover_input").await;
        assert_eq!((sent[0]["w"].as_u64(), sent[0]["st"].as_u64()), (Some(7), Some(st)), "the app checks both again");
        for _ in 0..(INPUT_PER_SECOND + 20) {
            r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "move", "x": 1, "y": 1}], "st": now_millis(), "w": 7})).await.unwrap();
        }
        assert!(r.told_now("browser.takeover_input").await.len() <= INPUT_PER_SECOND as usize + 1, "a flood is cut to a rate the page can take");
    }

    #[tokio::test]
    async fn a_phone_that_stops_acknowledging_pauses_the_stream_without_spinning_and_its_ack_resumes_it() {
        let mut r = rig();
        let tk = r.start().await;
        r.frame(&tk, "one");
        let first = r.next("tk_frame").await.expect("a frame");
        // Not acknowledged: after PAUSE_AFTER the app is told to pause, and the heartbeat says so.
        tokio::time::sleep(PAUSE_AFTER + WATCH * 2).await;
        assert!(r.told("browser.takeover_set").iter().any(|s| s["paused"] == true), "{:?}", r.told("browser.takeover_set"));
        let mut paused_beat = false;
        for _ in 0..6 {
            if r.next("tk_fhb").await.expect("a heartbeat")["paused"] == true {
                paused_beat = true;
                break;
            }
        }
        assert!(paused_beat, "the heartbeat says it is paused");
        // A frame that comes now is not kept (the resume brings a fresh one), and the hold sleeps meanwhile instead of going round every millisecond.
        r.frame(&tk, "late");
        let hold = r.d.takeover.hold(&tk).unwrap();
        let before = hold.state.lock().wakes;
        tokio::time::sleep(Duration::from_millis(500)).await;
        let woke = hold.state.lock().wakes - before;
        assert!(woke <= 6, "{woke} wakes in 500 ms while paused");
        let dropped = r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "tap", "x": 1, "y": 1}], "st": now_millis(), "w": 1})).await.unwrap();
        assert!(dropped.is_none() && r.told("browser.takeover_input").is_empty(), "no input while paused");
        // The acknowledgement resumes it with a fresh frame.
        r.verb("tk_ack", json!({"tk": tk, "seq": first["seq"]})).await.unwrap();
        let resumed = r.told_now("browser.takeover_set").await;
        assert!(resumed.last().is_some_and(|s| s["paused"] == false && s["fresh"] == true), "{resumed:?}");
        r.frame(&tk, "fresh");
        assert_eq!(r.next("tk_frame").await.expect("frames again")["data"], "fresh");
    }

    #[tokio::test]
    async fn a_start_that_fails_tells_the_app_to_let_go() {
        let r = rig_with(Start::Fail);
        assert_eq!(r.verb("tk_start", json!({"pane": "browser-a"})).await.unwrap_err().code, "no_pane");
        let stops = r.told_now("browser.takeover_stop").await;
        assert_eq!(stops.len(), 1, "it may have started after all");
        assert_eq!(r.d.takeover.count(), 0);
        assert!(r.d.takeover.starting.lock().is_empty(), "nothing is left starting");
    }

    #[tokio::test]
    async fn two_starts_at_once_for_one_pane_get_one_hold() {
        let r = rig_with(Start::Slow(300));
        let first = tokio::spawn({
            let (d, out) = (r.d.clone(), r.out.clone());
            async move {
                let phone = Phone { device: "phone", name: "Phone", conn: 1, out: &out };
                d.takeover.phone_verb(&d, phone, "tk_start", &json!({"pane": "browser-a"})).await.map(|v| v.is_some()).map_err(|e| e.code)
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let (out2, _rx) = mpsc::channel(4);
        let tablet = Phone { device: "tablet", name: "Tablet", conn: 2, out: &out2 };
        assert_eq!(r.d.takeover.phone_verb(&r.d, tablet, "tk_start", &json!({"pane": "browser-a"})).await.unwrap_err().code, "tk_taken", "the pane is starting already");
        assert_eq!(r.verb("tk_start", json!({"pane": "browser-a"})).await.unwrap_err().code, "tk_taken", "and so is this device");
        assert_eq!(first.await.unwrap(), Ok(true));
        assert_eq!(r.d.takeover.count(), 1);
    }

    #[tokio::test]
    async fn a_rebind_on_the_connection_that_holds_it_keeps_it() {
        let r = rig();
        let tk = r.start().await;
        r.verb("tk_rebind", json!({"tk": tk})).await.unwrap();
        assert_eq!(r.d.takeover.count(), 1);
    }

    #[tokio::test]
    async fn a_lower_permission_ends_the_hold() {
        let mut r = rig();
        r.start().await;
        r.d.mobile.set_permission("phone", crate::mobile::Permission::Write).unwrap();
        assert_eq!(r.next("tk_end").await.expect("told")["reason"], "revoked");
        assert_eq!(r.d.takeover.count(), 0);
    }

    #[tokio::test]
    async fn an_acknowledgement_under_an_older_number_while_paused_still_resumes_the_stream() {
        // The phone acknowledged frame 1, then got frames it never acknowledged (the same picture again, which the page does not load a second
        // time). Paused, it acknowledges 1 again, as the page does on a paused heartbeat: the stream must come back.
        let mut r = rig();
        let tk = r.start().await;
        r.frame(&tk, "one");
        let first = r.next("tk_frame").await.expect("a frame");
        r.verb("tk_ack", json!({"tk": tk, "seq": first["seq"]})).await.unwrap();
        for data in ["same", "same again"] {
            r.frame(&tk, data);
            r.next("tk_frame").await.expect("frames on credit");
        }
        tokio::time::sleep(PAUSE_AFTER + WATCH * 2).await;
        assert!(r.told("browser.takeover_set").iter().any(|s| s["paused"] == true));
        r.verb("tk_ack", json!({"tk": tk, "seq": first["seq"]})).await.unwrap();
        r.frame(&tk, "fresh");
        assert_eq!(r.next("tk_frame").await.expect("frames again")["data"], "fresh");
        tokio::time::sleep(PAUSE_AFTER / 2).await;
        assert_eq!(r.d.takeover.hold(&tk).unwrap().state.lock().paused, false, "and it does not pause again at once");
    }

    #[tokio::test]
    async fn a_dropped_link_pauses_the_app_until_the_device_is_back() {
        let r = rig();
        let tk = r.start().await;
        r.d.takeover.link_closed(1);
        tokio::time::sleep(WATCH * 3).await;
        assert!(r.told_now("browser.takeover_set").await.iter().any(|s| s["paused"] == true), "no frames are made for nobody");
        let (out2, _rx2) = mpsc::channel(16);
        let back = Phone { device: "phone", name: "Phone", conn: 2, out: &out2 };
        r.d.takeover.phone_verb(&r.d, back, "tk_rebind", &json!({"tk": tk})).await.unwrap();
        let set = r.told_now("browser.takeover_set").await;
        assert!(set.last().is_some_and(|s| s["paused"] == false && s["fresh"] == true), "{set:?}");
    }

    #[tokio::test]
    async fn a_release_goes_even_when_the_rest_of_its_message_is_dropped() {
        let r = rig();
        let tk = r.start().await;
        let old = now_millis() - 5000;
        r.verb("tk_input", json!({"tk": tk, "ops": [{"k": "move", "x": 1, "y": 1}, {"k": "up", "x": 2, "y": 2}], "st": old, "w": 1})).await.unwrap();
        let sent = r.told_now("browser.takeover_input").await;
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0]["ops"], json!([{"k": "up", "x": 2.0, "y": 2.0}]), "only the release: a button is never left pressed");
    }
}