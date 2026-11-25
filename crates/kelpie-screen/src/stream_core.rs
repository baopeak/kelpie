//! One stream's loop, whatever captures and encodes under it: everything of a stream that is not the machine's own API.
//!
//! The loop looks at the machine four times a second (states, the window's place), takes the newest picture when the phone has room and the
//! pacer says so (`pace::Pacer`: a constant-rate timer from the last capture's start, one frame in the encoder unless it asks for another),
//! encodes it, rewrites the SPS (`sps`), sends video, paint-over tiles (`tiles`) and the cursor, and once a second the stats with the stages'
//! timings. A still picture is topped off in the codec first (`pace::TopOff`, not for a caret's few pixels), and only then painted over with
//! tiles; an update or a keyframe too large to arrive in time is encoded at a one-frame bitrate dip (`pace::BigFrame`), undone however that
//! frame leaves the encoder (`pace::Dip`).
//!
//! Frames lost on the way never hold the stream (`control::Flight`): a keyframe request, or the daemon saying what it dropped, gives up the
//! frames in flight. A frame that is not sent after all (too large) puts its unpaint rectangles back, so a crisp tile is never left over a
//! change.
//!
//! The machine's side is a `Backend`: it grabs, converts, encodes and reads back, and says what it sees; it decides nothing. The input thread,
//! the border and the stop hotkey belong to it too (it reports the hotkey and what input found out). The loop is the same on every OS.

use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, SyncSender, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::control::{Controller, Event, Flight, Params, Stats, MAX_KBPS, MIN_KBPS};
use crate::geom::{self, Rect};
use crate::pace::{self, BigFrame, Dip, EncoderRate, Pacer, ReencodeGate, TopOff, BIG_FRAME_PERCENT};
use crate::sps;
use crate::states::{input_policy, InputPolicy, Observed, State, Tracker};
use crate::tile_image;
use crate::tiles::{Grid, Sharpen};
use crate::wire::{self, Shape, Video};

const LOOK_EVERY: Duration = Duration::from_millis(250);
/// How often the monitors are looked at again (one plugged in or out moves the virtual desktop the input is mapped over).
pub const DESKTOP_EVERY: Duration = Duration::from_secs(2);
const CURSOR_EVERY: Duration = Duration::from_millis(33);
/// How long the loop waits for a picture while frames are still in the encoder (its output is taken on the way round).
const ENCODER_POLL_MS: u64 = 4;
/// How long the loop waits for a change once a capture is due (it comes round for the encoder, the cursor and the app at least this often).
const CAPTURE_WAIT_MS: u64 = 30;
/// Captures or conversions failed this many times in a row: the machine is asked whether the device is still there.
const DEVICE_CHECK_AFTER: u32 = 20;
/// Paint-over tiles: at most this share of the video's rate, with this much at once.
const TILE_SHARE: f64 = 0.5;
const TILE_BURST: f64 = 256.0 * 1024.0;
const TILE_CELLS_AT_ONCE: usize = 32;

/// Wall-clock milliseconds, the clock the daemon stamps input with.
pub fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// One event or reply for the app, on its way to stdout.
pub fn emit(out: &SyncSender<Vec<u8>>, v: Value) {
    let _ = out.send(wire::out_json(&v));
}

/// What the app says to a running stream.
pub enum Cmd {
    /// `link_full_ms`: how long the daemon has seen the link's window full (`Bandwidth::on_link_window`); `win`: the daemon takes a window
    /// sized to the link (`Controller::window`), said with the `window` event.
    Flow { acked: u32, rtt_ms: u32, level: u8, lost_tiles: bool, dropped_through: u32, raw: u64, wire: u64, link_full_ms: u64, win: bool },
    Keyframe,
    /// The part of the picture the phone shows, sent crisp once (`tiles::Sharpen`).
    Sharpen(Rect),
    Pause(bool),
    ResumeLoad,
    /// The most the stream may send from now on (kbps; None: no more than any stream): the way changed (the relay WebSocket is held to a rate,
    /// peer to peer is not), or the daemon holds it under the rate a lossy path carries.
    Cap(Option<u32>),
    Stop(String),
}

/// What a stdin line says to the loop.
pub enum Parsed {
    Cmd(Cmd),
    /// A command of the loop with nothing to do (a sharpen that is not a rectangle).
    Nothing,
    /// Not a command of the loop.
    Unknown,
}

impl Cmd {
    /// The loop's own commands (`flow`, `sharpen`, `keyframe`, `pause`, `resume_load`, `cap`); `stop`, `input` and `text` belong to the host.
    pub fn parse(cmd: &str, req: &Value) -> Parsed {
        let n = |k: &str| req[k].as_u64().unwrap_or(0);
        Parsed::Cmd(match cmd {
            "flow" => Cmd::Flow {
                acked: n("acked") as u32,
                rtt_ms: n("rtt_ms").min(60_000) as u32,
                level: n("level").min(3) as u8,
                lost_tiles: req["lost_tiles"].as_bool() == Some(true),
                dropped_through: n("dropped_through") as u32,
                raw: n("raw"),
                wire: n("wire"),
                link_full_ms: n("link_full_ms").min(3_600_000),
                win: req["win"].as_bool() == Some(true),
            },
            "sharpen" => {
                let rect = &req["rect"];
                let v = |k: &str| rect[k].as_u64().filter(|v| *v <= u64::from(u16::MAX)).map(|v| v as i32);
                match (v("x"), v("y"), v("w"), v("h")) {
                    (Some(x), Some(y), Some(w), Some(h)) => Cmd::Sharpen(Rect::new(x, y, w, h)),
                    _ => return Parsed::Nothing,
                }
            }
            "keyframe" => Cmd::Keyframe,
            "pause" => Cmd::Pause(req["paused"].as_bool() == Some(true)),
            "resume_load" => Cmd::ResumeLoad,
            "cap" => Cmd::Cap(req["max_kbps"].as_u64().map(|k| k.min(u64::from(u32::MAX)) as u32)),
            _ => return Parsed::Unknown,
        })
    }
}

/// What a stream is, as the host found it.
pub struct Config {
    pub sc: String,
    /// A window target (else a monitor).
    pub window: bool,
    /// The picture's size, even sides.
    pub src: (u32, u32),
    /// Where the target is on the screen.
    pub rect: Rect,
    pub level: u8,
    /// Crisp paint-over tiles are on in Settings.
    pub tiles: bool,
    /// The most the stream may send (kbps), for a way that is held to a rate (the relay WebSocket).
    pub max_kbps: Option<u32>,
    /// The target's title (shown on the desktop and the phone) and, for a window, its program's file name (all the audit log records of it).
    pub title: String,
    pub exe: String,
}

/// The controller a stream starts with: the backend opens its encoder at `params()` of it.
pub fn controller(cfg: &Config) -> Controller {
    let mut ctl = Controller::new(cfg.src, cfg.window, false, 0);
    ctl.set_level(cfg.level, 0);
    // Started on a loaded machine (L2 for the CPU, lag or heat; the daemon tells the helper the level without memory): the cheapest picture first.
    if cfg.level >= 2 {
        ctl.start_low();
    }
    if let Some(cap) = cfg.max_kbps {
        ctl.bandwidth.set_cap(cap);
    }
    ctl
}

/// The encoder the backend has now.
#[derive(Clone, Debug, Default)]
pub struct EncoderInfo {
    pub hardware: bool,
    pub name: String,
    /// Settings the encoder refused.
    pub refused: Vec<String>,
    pub first_output_ms: Option<f64>,
    /// SPS and PPS in Annex B, when the encoder has them before its first output.
    pub sequence_header: Option<Vec<u8>>,
}

/// One frame out of the encoder.
#[derive(Clone, Debug)]
pub struct Encoded {
    pub data: Vec<u8>,
    pub key: bool,
    /// The time the frame was given with (the key to match it with what went in).
    pub time: i64,
    /// The quantizer, when the encoder says.
    pub qp: Option<u32>,
}

/// What a grab found.
pub enum Grab<P> {
    /// A new picture, `size` pixels (a window that was resized says so here), and the rectangles it changed when the capture knows them.
    /// `first_full`: whether the capture's first picture said everything changed (kept for the stats).
    Frame { picture: P, size: (u32, u32), dirty: Option<Vec<Rect>>, first_full: Option<bool> },
    /// Nothing new yet.
    Idle,
    /// The capture failed (counted: many in a row make the loop ask whether the device is still there).
    Failed,
    /// The target is gone: the stream ends with this reason (`display_changed`, `closed`).
    End(&'static str),
}

/// What the input found out, for the state the phone is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSignal {
    /// A click was dropped because another window covers the target there.
    Covered,
    /// Another program's dialog owned by the target is in front.
    ForeignDialog,
    /// The target is not in front for keys.
    NotFront,
    /// The system did not take the input.
    Blocked,
}

/// What the machine does for a stream. Every method is called on the stream's thread.
pub trait Backend {
    /// A picture as the capture hands it over, kept for the readback, the conversion and the next pass over a still picture.
    type Picture: Clone;
    /// A picture converted for the encoder.
    type Input;

    /// The stop hotkey was pressed since the last call.
    fn hotkey_pressed(&mut self) -> bool;
    /// What the input thread found out since the last call, one at a time.
    fn input_signal(&mut self) -> Option<InputSignal>;
    /// Whether the graphics device is still there (a driver that restarted takes it).
    fn healthy(&self) -> bool;
    /// One look at the machine.
    fn look(&mut self) -> Observed;
    /// A window target's place now (and the border follows it); None for a monitor or a window that cannot be placed.
    fn window_rect(&mut self) -> Option<Rect>;
    /// What the input thread reads: the target's place, the picture's size and what the state allows. `refresh_desktop`: the monitors are
    /// looked at again (`DESKTOP_EVERY`).
    fn publish(&mut self, rect: Rect, src: (u32, u32), policy: InputPolicy, refresh_desktop: bool);
    /// The newest picture, waiting at most `wait_ms` for a change.
    fn grab(&mut self, wait_ms: u64) -> Grab<Self::Picture>;
    /// The target's picture is now `src` pixels, or the ladder's size changed: a new converter and a new encoder for `params`. Both or neither.
    fn resize(&mut self, src: (u32, u32), params: &Params, hardware: bool) -> Result<(), String>;
    /// A new software encoder for `params`, the converter as it is.
    fn software_encoder(&mut self, params: &Params) -> Result<(), String>;
    fn encoder_info(&self) -> EncoderInfo;
    fn hardware(&self) -> bool;
    /// Frames the encoder holds.
    fn held(&self) -> u64;
    fn first_output_ms(&self) -> Option<f64>;
    /// Whether the encoder asks for another frame while it holds one.
    fn asks_for_input(&self) -> bool;
    fn set_bitrate(&mut self, kbps: u32);
    fn convert(&mut self, picture: &Self::Picture) -> Result<Self::Input, String>;
    /// Gives the encoder a frame: false when it has no room for it now.
    fn encode(&mut self, input: &Self::Input, time: i64, key: bool) -> Result<bool, String>;
    /// What the encoder finished.
    fn poll(&mut self) -> Result<Vec<Encoded>, String>;
    /// Reads the picture (or the `dirty` part of it) back as BGRA into the buffer `pixels` returns: false when it could not.
    fn readback(&mut self, picture: &Self::Picture, src: (u32, u32), dirty: Option<&[Rect]>) -> bool;
    /// The last readback: BGRA and its row stride (0 before the first one).
    fn pixels(&self) -> (&[u8], usize);
    /// The system cursor: its shape, whether it shows, and where on the screen.
    fn cursor(&mut self) -> (Shape, bool, i32, i32);
    /// The stream is over: the input thread, the border and the hotkey go, and the machine is told it is no longer streaming. Called once,
    /// before the end is said; what is left is dropped with the backend.
    fn finish(&mut self);
}

/// `KELPIE_SCREEN_TEST_ENCODE_DELAY_MS`: added to every encode, to drive the ladder where there is no GPU worth measuring.
fn test_delay() -> Option<Duration> {
    std::env::var("KELPIE_SCREEN_TEST_ENCODE_DELAY_MS").ok()?.parse::<u64>().ok().filter(|ms| *ms > 0 && *ms <= 2000).map(Duration::from_millis)
}

/// A frame in the encoder: its time (the encoder's key for it), when it went in, when its picture was taken (ms since the stream started),
/// the conversion's time, and whether it is a top-off pass of a still picture.
struct InEncoder {
    time: i64,
    began: Instant,
    captured: u64,
    convert_ms: f64,
    pass: bool,
    /// The pixels its picture changed (all of it for a keyframe): a small change does not start a top-off.
    area: u64,
}

/// Runs a stream to its end, which it says (`ended`) before it returns. The backend has its encoder open for `ctl.params()`, its border and
/// hotkey up and its input running: what is said here is `started`, the encoder, and from then on everything the phone sees.
pub fn run<B: Backend>(cfg: Config, mut ctl: Controller, backend: &mut B, cmds: Receiver<Cmd>, out: SyncSender<Vec<u8>>, started: Sender<Result<Value, String>>) {
    let sc = cfg.sc.as_str();
    let window = cfg.window;
    let mut rect = cfg.rect;
    let mut src = cfg.src;
    let started_at = Instant::now();
    // The wall clock at the start, from which each frame's capture time is counted: the phone tells how old a picture is with it.
    let t0 = wall_ms();
    let now_ms = || started_at.elapsed().as_millis() as u64;
    let mut params: Params = ctl.params();
    let delay = test_delay();
    let info = backend.encoder_info();
    let mut ps = sps::Params::default();
    if let Some(h) = &info.sequence_header {
        ps.seed(h);
    }
    let _ = started.send(Ok(json!({"kind": if window { "window" } else { "monitor" }, "title": cfg.title, "exe": cfg.exe, "w": src.0, "h": src.1, "encoder": if info.hardware { "hw" } else { "sw" }, "encoder_name": info.name, "refused": info.refused, "t0": t0})));
    emit(&out, json!({"event": "encoder", "sc": sc, "encoder": if info.hardware { "hw" } else { "sw" }, "name": info.name, "refused": info.refused, "first_output_ms": info.first_output_ms}));

    let mut tracker = Tracker::default();
    let mut stats = Stats::new(0);
    let mut grid = cfg.tiles.then(|| Grid::new(src.0, src.1, 0));
    let mut flight = Flight::default();
    let mut pacer = Pacer::default();
    let mut top_off = TopOff::default();
    let mut big = BigFrame::default();
    let mut sharpen = Sharpen::default();
    // The window last told to the app (frames, bytes), once the daemon takes one.
    let mut window_told: Option<(u32, usize)> = None;
    let mut rate = EncoderRate::new(params.kbps);
    // A big frame's bitrate dip, undone however that frame leaves the encoder.
    let mut dip = Dip::default();
    let mut in_encoder: VecDeque<InEncoder> = VecDeque::new();
    let mut link_paused = false;
    let (mut unpaint, mut unpaint_all) = (Vec::<Rect>::new(), false);
    let (mut tile_id, mut tile_tokens, mut tile_at) = (0u32, TILE_BURST, now_ms());
    let (mut last_look, mut last_desktop, mut last_cursor) = (Instant::now() - LOOK_EVERY, Instant::now(), Instant::now());
    let mut cursor_told: Option<(u8, bool, i32, i32)> = None;
    // The last picture taken: encoded again when a keyframe is owed and nothing new came (a still window sends no frames by itself), and for
    // the top-off passes of a still one.
    let mut last_picture: Option<B::Picture> = None;
    let mut reencode = ReencodeGate::default();
    // Captures and conversions failed in a row (`DEVICE_CHECK_AFTER`).
    let mut failures = 0u32;

    // The software encoder from now on, said with `why`: true when it opened.
    macro_rules! to_software {
        ($why:expr) => {{
            if backend.software_encoder(&params).is_ok() {
                ctl.set_software(true);
                in_encoder.clear();
                pacer.clear();
                rate = EncoderRate::new(params.kbps);
                dip.clear();
                flight.restart();
                let info = backend.encoder_info();
                emit(&out, json!({"event": "encoder", "sc": sc, "encoder": "sw", "name": info.name, "refused": info.refused, "first_output_ms": info.first_output_ms, "why": $why}));
                true
            } else {
                false
            }
        }};
    }

    let reason: String = 'stream: loop {
        let now = now_ms();
        // What the app said.
        loop {
            match cmds.try_recv() {
                Ok(Cmd::Stop(r)) => break 'stream r,
                Ok(Cmd::Flow { acked, rtt_ms, level, lost_tiles, dropped_through, raw, wire, link_full_ms, win }) => {
                    ctl.set_win(win);
                    flight.acked(acked);
                    ctl.bandwidth.on_link_window(link_full_ms);
                    // Frames the daemon dropped will never be acknowledged: they no longer count as in flight.
                    flight.dropped_through(dropped_through);
                    if rtt_ms > 0 {
                        ctl.bandwidth.on_ack(rtt_ms, now);
                        stats.ack(rtt_ms);
                    }
                    ctl.bandwidth.on_wire(raw, wire);
                    ctl.set_level(level, now);
                    if lost_tiles {
                        if let Some(g) = &mut grid {
                            g.forget_painted();
                        }
                        unpaint_all = true;
                    }
                }
                Ok(Cmd::Keyframe) => flight.ask_key(now),
                // Only with crisp tiles on in Settings, only while they are off for the link (else they go anyway), and not on a machine at L3,
                // which has no CPU for them.
                Ok(Cmd::Sharpen(r)) => {
                    if grid.is_some() && ctl.level() < 3 && sharpen.ask(r, ctl.crisp_ok(flight.inflight(), now), now) {
                        top_off.again(now);
                        // Paced as tiles are, at their share of the video's rate: no burst ahead of the video on the slow link it is for.
                        tile_tokens = tile_tokens.min(f64::from(params.kbps) * 1000.0 / 8.0 * TILE_SHARE);
                    }
                }
                Ok(Cmd::Pause(p)) => {
                    link_paused = p;
                    top_off.stop();
                    if !p {
                        // A new connection: a fresh keyframe, and every tile painted again on it.
                        flight.restart();
                        if let Some(g) = &mut grid {
                            g.forget_painted();
                        }
                        unpaint_all = true;
                    }
                }
                Ok(Cmd::Cap(kbps)) => ctl.bandwidth.set_cap(kbps.unwrap_or(MAX_KBPS)),
                Ok(Cmd::ResumeLoad) => {
                    ctl.resume(now);
                    flight.restart();
                    emit(&out, json!({"event": "state", "sc": sc, "state": tracker.state().name()}));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'stream "app_closed".to_string(),
            }
        }
        if backend.hotkey_pressed() {
            break 'stream "hotkey".to_string();
        }
        while let Some(signal) = backend.input_signal() {
            match signal {
                InputSignal::Covered => {
                    if let Some(s) = tracker.covered() {
                        emit(&out, json!({"event": "state", "sc": sc, "state": s.name()}));
                    }
                }
                InputSignal::ForeignDialog => {
                    if let Some(s) = tracker.covered_by_dialog() {
                        emit(&out, json!({"event": "state", "sc": sc, "state": s.name()}));
                    }
                }
                InputSignal::NotFront => emit(&out, json!({"event": "input_refused", "sc": sc, "why": "not_front"})),
                InputSignal::Blocked => emit(&out, json!({"event": "input_blocked", "sc": sc})),
            }
        }
        // Four times a second: the states, the window's place; every two seconds, the monitors.
        if last_look.elapsed() >= LOOK_EVERY {
            last_look = Instant::now();
            // A graphics driver that restarted took the device: found within a look, even when no frame comes to fail.
            if !backend.healthy() {
                break 'stream "device_lost".to_string();
            }
            if let Some(s) = tracker.look(&backend.look(), window) {
                emit(&out, json!({"event": "state", "sc": sc, "state": s.name()}));
                if s.ends() {
                    break 'stream "closed".to_string();
                }
                if s == State::Live {
                    flight.restart();
                }
            }
            if let Some(r) = backend.window_rect() {
                rect = r;
            }
            let refresh_desktop = last_desktop.elapsed() >= DESKTOP_EVERY;
            if refresh_desktop {
                last_desktop = Instant::now();
            }
            backend.publish(rect, src, input_policy(tracker.state(), window), refresh_desktop);
        }
        flight.tick(now);
        if let Some(Event::PausedForLoad) = ctl.tick(flight.inflight(), now) {
            emit(&out, json!({"event": "paused", "sc": sc, "reason": "load"}));
        }
        // The window sized to the link, said to the daemon when it changes (only to one that takes it).
        let link_window = ctl.window();
        if ctl.win() && window_told != Some(link_window) {
            window_told = Some(link_window);
            emit(&out, json!({"event": "window", "sc": sc, "frames": link_window.0, "bytes": link_window.1}));
        }
        // What the ladder says now: a new size is a new converter and encoder; a new bitrate is set live, when it is worth a reconfiguration
        // (`pace::EncoderRate`), and not while a big frame's dip holds.
        let next = ctl.params();
        if (next.w, next.h) != (params.w, params.h) {
            if backend.resize(src, &next, !ctl.software()).is_err() {
                break 'stream "encoder_failed".to_string();
            }
            if let Some(h) = &backend.encoder_info().sequence_header {
                ps.seed(h);
            }
            in_encoder.clear();
            pacer.clear();
            top_off.stop();
            rate = EncoderRate::new(next.kbps);
            dip.clear();
            flight.restart();
        } else if !dip.active() {
            if let Some(k) = rate.next(next.kbps) {
                backend.set_bitrate(k);
            }
        }
        params = next;

        // A frame the encoder never gave back (it may drop one) is not waited for after a second, and counts as a second's encode: an encoder
        // stalled behind a game on the GPU is slow, not instant, and may give way to the software one.
        let mut stalled = false;
        while in_encoder.front().is_some_and(|f| f.began.elapsed() > Duration::from_secs(1)) {
            if let Some(gone) = in_encoder.pop_front() {
                if let Some(back) = dip.left(gone.time) {
                    backend.set_bitrate(back);
                }
            }
            pacer.done();
            stalled |= ctl.on_encoded(1000.0, now) == Some(Event::UseSoftware);
        }
        if stalled {
            to_software!("slow");
        }
        let interval = ctl.interval_ms();
        let due = pacer.due_in(interval, now, backend.asks_for_input());
        // Room for a frame on the link, nothing gating a keyframe, a picture that may move: then the pacer says when.
        let ready = !link_paused && !flight.gated() && tracker.state().picture_moves() && ctl.may_encode(flight.inflight(), flight.inflight_bytes(), None);
        let may = ready && due == Some(0);
        let top_off_due = may && top_off.due(now) && (ctl.crisp_ok(flight.inflight(), now) || sharpen.rect.is_some()) && last_picture.is_some();
        // A keyframe owed on a still window: the last picture again, at most once a second (`pace::ReencodeGate`).
        let reencode_due = may && flight.wants_key() && last_picture.is_some() && reencode.ready(now);
        // While a frame is in the encoder the loop comes back within a few milliseconds to take it, so that its time is the encoder's own;
        // once due it waits for the next change (a still picture gives nothing until then), but not past a top-off pass that is due.
        let wait_ms = if pacer.pending() {
            ENCODER_POLL_MS
        } else if may {
            // Something to encode again at once (a keyframe owed, a top-off pass): no waiting for a change first.
            if top_off_due || reencode_due { 0 } else { CAPTURE_WAIT_MS }
        } else if ready {
            due.unwrap_or(ENCODER_POLL_MS).clamp(1, CAPTURE_WAIT_MS)
        } else {
            50
        };
        let grabbed: Option<(B::Picture, Option<Vec<Rect>>)> = if !may {
            std::thread::sleep(Duration::from_millis(wait_ms));
            None
        } else {
            match backend.grab(wait_ms) {
                Grab::Frame { picture, size, dirty, first_full } => {
                    if let Some(full) = first_full {
                        stats.dxgi_first_full = Some(u8::from(full));
                    }
                    if size != src {
                        // Resized: everything that depends on the size again.
                        src = size;
                        ctl.set_source(src);
                        grid = cfg.tiles.then(|| Grid::new(src.0, src.1, now));
                        unpaint_all = true;
                        params = ctl.params();
                        if backend.resize(src, &params, !ctl.software()).is_err() {
                            break 'stream "encoder_failed".to_string();
                        }
                        in_encoder.clear();
                        pacer.clear();
                        top_off.stop();
                        rate = EncoderRate::new(params.kbps);
                        dip.clear();
                        flight.restart();
                    }
                    Some((picture, dirty))
                }
                Grab::Failed => {
                    failures += 1;
                    None
                }
                Grab::Idle => None,
                Grab::End(why) => break 'stream why.to_string(),
            }
        };
        // Captures or conversions that keep failing: a graphics driver that restarted (or was updated) took the device with it, and nothing
        // will reach the encoder again. Said and ended, never a silent freeze.
        if failures >= DEVICE_CHECK_AFTER {
            failures = 0;
            if !backend.healthy() {
                break 'stream "device_lost".to_string();
            }
        }
        // A new picture; else, with a keyframe owed and allowed, the last one again (the phone, and the daemon, which holds input back while
        // one is owed, would wait for the window to change); else a top-off pass of the still one.
        let (picture, dirty, pass) = match grabbed {
            Some((pic, dirty)) => {
                last_picture = Some(pic.clone());
                (Some(pic), dirty, false)
            }
            None if reencode_due => {
                reencode.used(now);
                (last_picture.clone(), None, false)
            }
            None if top_off_due => (last_picture.clone(), None, true),
            None => (None, None, false),
        };
        if let Some(pic) = picture {
            let mut area = u64::from(src.0) * u64::from(src.1);
            if !pass {
                if let Some(g) = &mut grid {
                    if backend.readback(&pic, src, dirty.as_deref()) {
                        let (px, stride) = backend.pixels();
                        unpaint.extend(g.update(px, stride, now, dirty.as_deref()));
                        area = g.last_changed_area();
                    }
                } else if let Some(rects) = &dirty {
                    area = pace::area(rects);
                }
            }
            let converting = Instant::now();
            let converted = backend.convert(&pic);
            if converted.is_err() {
                failures += 1;
            }
            if let Ok(input) = converted {
                failures = 0;
                let convert_ms = converting.elapsed().as_secs_f64() * 1000.0;
                let time = (started_at.elapsed().as_nanos() / 100) as i64;
                let key = flight.wants_key();
                // Too large to arrive in time at this bitrate: a coarser frame now, not a late one (CRD's guard), with the bitrate back once
                // it is out. A keyframe and a top-off pass are what they are.
                // A keyframe is the whole picture: guarded too, so that no media message takes most of the link for long (a reply waits for at
                // most the one being written).
                let area = if key { u64::from(src.0) * u64::from(src.1) } else { area };
                // On a slow link (the lowest rungs) a keyframe always takes the dip, so that it stays small.
                if !pass && !dip.active() && (big.check(area, params.kbps, interval) || ctl.key_dip(key)) {
                    backend.set_bitrate((params.kbps * BIG_FRAME_PERCENT / 100).max(MIN_KBPS));
                    dip.start(time, rate.applied);
                    stats.big_frames += 1;
                }
                let began = Instant::now();
                match backend.encode(&input, time, key) {
                    Ok(true) => {
                        if let Some(extra) = delay {
                            std::thread::sleep(extra);
                        }
                        pacer.started(now);
                        flight.encoded(key, now);
                        in_encoder.push_back(InEncoder { time, began, captured: now, convert_ms, pass, area });
                        while in_encoder.len() > 8 {
                            if let Some(gone) = in_encoder.pop_front() {
                                if let Some(back) = dip.left(gone.time) {
                                    backend.set_bitrate(back);
                                }
                            }
                        }
                    }
                    Ok(false) => {
                        // No room in the encoder now: a moment for it, not a spin.
                        stats.dropped_encoder += 1;
                        std::thread::sleep(Duration::from_millis(ENCODER_POLL_MS));
                        if let Some(back) = dip.left(time) {
                            backend.set_bitrate(back);
                        }
                        // A top-off pass the encoder had no room for counts as one, so a pass is never tried again on every turn.
                        if pass {
                            top_off.passed(now, interval, usize::MAX, None);
                        }
                    }
                    Err(_) => {
                        // The hardware encoder failed: the software one from now on, and said so.
                        if backend.hardware() && to_software!("failed") {
                            continue;
                        }
                        break 'stream "encoder_failed".to_string();
                    }
                }
            }
        }
        // What the encoder finished.
        match backend.poll() {
            Ok(done) => {
                for e in done {
                    let entry = in_encoder.iter().position(|f| f.time == e.time).and_then(|i| in_encoder.remove(i));
                    // One frame in the encoder at a time: the next capture may be scheduled.
                    pacer.done();
                    // Output that matches no frame given (one already counted as a second's stall) is not timed: it would read as instant.
                    let measured = entry.as_ref().map(|f| f.began.elapsed().as_secs_f64() * 1000.0 + delay.map_or(0.0, |d| d.as_secs_f64() * 1000.0));
                    let encode_ms = measured.unwrap_or(0.0);
                    if let Some(back) = dip.left(e.time) {
                        backend.set_bitrate(back);
                    }
                    if let Some(Event::UseSoftware) = measured.and_then(|ms| ctl.on_encoded(ms, now)) {
                        if to_software!("slow") {
                            break;
                        }
                    }
                    let Some(au) = ps.fix(&e.data) else {
                        // Not sendable: what follows would refer to it. A keyframe, through the gate (at most one a second).
                        flight.ask_key(now);
                        continue;
                    };
                    let key = e.key || sps::is_keyframe(&au);
                    let captured = entry.as_ref().map_or(now, |f| f.captured);
                    let v = Video { key, seq: flight.seq() + 1, capture_ms: captured as u32, src: (src.0 as u16, src.1 as u16), coded: (params.w as u16, params.h as u16), unpaint: std::mem::take(&mut unpaint), unpaint_all: std::mem::take(&mut unpaint_all) };
                    let msg = wire::video(&v, &au);
                    if msg.len() > wire::MAX_MEDIA {
                        // Too large for one message: not sent. Its unpaint goes with the next frame, less bitrate, and a keyframe.
                        unpaint = v.unpaint;
                        unpaint_all = v.unpaint_all;
                        ctl.bandwidth.cut_for_size(flight.inflight());
                        flight.ask_key(now);
                        stats.dropped_link += 1;
                        continue;
                    }
                    flight.sent(msg.len());
                    ctl.bandwidth.on_sent(now);
                    stats.frame(msg.len(), key, encode_ms);
                    let _ = out.send(wire::out_media(sc, &msg));
                    let sent_at = now_ms();
                    stats.timing(sent_at, sent_at.saturating_sub(captured) as f64, entry.as_ref().map_or(0.0, |f| f.convert_ms));
                    // A new picture starts the top-off of what follows it; a pass says how far it got.
                    if entry.as_ref().is_some_and(|f| f.pass) {
                        stats.top_off += 1;
                        top_off.passed(sent_at, interval, msg.len(), e.qp);
                    } else {
                        top_off.changed(sent_at, interval, e.qp, entry.as_ref().map_or(u64::MAX, |f| f.area));
                    }
                }
            }
            Err(_) => break 'stream "encoder_failed".to_string(),
        }
        // Paint-over: still regions as crisp tiles, from the last picture read, within a share of the video's rate, once the top-off has
        // refined the video itself (tiles then cover what the codec cannot: text at the source's own resolution and colour).
        if let Some(g) = &mut grid {
            let rate_bytes = f64::from(params.kbps) * 1000.0 / 8.0 * TILE_SHARE;
            tile_tokens = (tile_tokens + rate_bytes * now.saturating_sub(tile_at) as f64 / 1000.0).min(TILE_BURST);
            tile_at = now;
            let (px, stride) = backend.pixels();
            let tiles_may = flight.seq() > 0 && !flight.wants_key() && !top_off.active() && !link_paused && stride > 0 && g.size() == src;
            // Sharpen: the part the phone shows, once its top-off is done, within its own budget whatever the link says of crisp tiles, and
            // at the tiles' share of the rate.
            if tiles_may && sharpen.rect.is_some() {
                let parts = if tile_tokens > 32_768.0 { g.due_within(now, TILE_CELLS_AT_ONCE, sharpen.rect) } else { Vec::new() };
                if parts.is_empty() && tile_tokens > 32_768.0 {
                    sharpen.done();
                }
                for r in parts {
                    match tile_image::encode(px, stride, r) {
                        Some((format, image)) if (image.len() as f64) <= tile_tokens && sharpen.spend(image.len()) => {
                            tile_id = tile_id.wrapping_add(1);
                            tile_tokens -= image.len() as f64;
                            let _ = out.send(wire::out_media(sc, &wire::tile(flight.seq(), tile_id, r, format, &image)));
                        }
                        _ => g.unmark(r),
                    }
                }
            }
            // Off by itself while the link is slow or was just cut (`Controller::crisp_ok`): the video needs the room.
            else if tiles_may && ctl.crisp_ok(flight.inflight(), now) && tile_tokens > 32_768.0 && g.any_due(now) {
                for r in g.due(now, TILE_CELLS_AT_ONCE) {
                    match tile_image::encode(px, stride, r) {
                        Some((format, image)) if (image.len() as f64) <= tile_tokens => {
                            tile_id = tile_id.wrapping_add(1);
                            tile_tokens -= image.len() as f64;
                            let _ = out.send(wire::out_media(sc, &wire::tile(flight.seq(), tile_id, r, format, &image)));
                        }
                        _ => g.unmark(r),
                    }
                }
            }
        }
        // The cursor, where it is in the picture.
        if last_cursor.elapsed() >= CURSOR_EVERY {
            last_cursor = Instant::now();
            let (shape, shows, x, y) = backend.cursor();
            let at = geom::screen_to_source(x, y, rect, src.0, src.1);
            let now_c = (shape as u8, shows && at.is_some(), at.map_or(0, |p| p.0), at.map_or(0, |p| p.1));
            if cursor_told != Some(now_c) {
                cursor_told = Some(now_c);
                let _ = out.send(wire::out_media(sc, &wire::cursor(shape, now_c.1, now_c.2, now_c.3)));
            }
        }
        let crisp = cfg.tiles.then(|| ctl.crisp_ok(flight.inflight(), now));
        for c in ctl.bandwidth.take_cuts() {
            stats.cut(c);
        }
        stats.link_cuts = ctl.bandwidth.cut_counts;
        stats.inflight_limit = link_window.0;
        if let Some(r) = stats.report_with(now, &params, ctl.level(), ctl.software(), crisp, backend.held(), backend.first_output_ms()) {
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["event"] = json!("stats");
            v["sc"] = json!(sc);
            emit(&out, v);
        }
    };
    drop(cmds);
    backend.finish();
    emit(&out, json!({"event": "ended", "sc": sc, "reason": reason}));
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{channel, sync_channel, RecvTimeoutError};
    use std::sync::{Arc, Mutex};
    use std::thread::JoinHandle;

    use super::*;
    use crate::wire::{Deframer, Out, KIND_TILE, KIND_VIDEO, MEDIA};

    const SIZE: u32 = 128;

    /// What a test tells the fake machine to do, and what it saw done.
    struct Script {
        /// The screen, BGRA.
        pixels: Vec<u8>,
        /// The encoder keeps every frame it is given.
        hold: bool,
        hotkey: bool,
        end: Option<&'static str>,
        panic_on_encode: bool,
        encodes: Vec<(i64, bool)>,
        finished: u32,
        dropped: bool,
    }

    impl Script {
        fn new() -> Script {
            Script { pixels: vec![128; (SIZE * SIZE * 4) as usize], hold: false, hotkey: false, end: None, panic_on_encode: false, encodes: Vec::new(), finished: 0, dropped: false }
        }
    }

    /// A machine that captures a screen of one colour as fast as it is asked and encodes by wrapping a few bytes.
    struct Fake {
        script: Arc<Mutex<Script>>,
        pictures: u64,
        ready: Vec<Encoded>,
        buf: Vec<u8>,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            if let Ok(mut s) = self.script.lock() {
                s.dropped = true;
            }
        }
    }

    fn sps_nal() -> Vec<u8> {
        let mut w = sps::Writer::default();
        w.bits(66, 8);
        w.bits(0, 8);
        w.bits(40, 8);
        w.ue(0); // id
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(0); // pic_order_cnt_type
        w.ue(2); // log2_max_pic_order_cnt_lsb_minus4
        w.ue(1); // reference frames
        w.bit(0);
        w.ue(SIZE / 16 - 1);
        w.ue(SIZE / 16 - 1);
        w.bit(1); // frame_mbs_only
        w.bit(1); // direct_8x8
        w.bit(0); // no cropping
        w.bit(0); // no VUI
        let mut nal = vec![0x67];
        nal.extend(sps::escape(&w.trailing()));
        nal
    }

    fn access_unit(key: bool) -> Vec<u8> {
        const START: [u8; 4] = [0, 0, 0, 1];
        let mut au = Vec::new();
        let mut nal = |bytes: &[u8]| {
            au.extend_from_slice(&START);
            au.extend_from_slice(bytes);
        };
        if key {
            nal(&sps_nal());
            nal(&[0x68, 0xce, 0x3c, 0x80]);
            nal(&[0x65, 0x88, 0x84, 0x21, 0xa0]);
        } else {
            nal(&[0x41, 0x9a, 0x24, 0x6c, 0x42]);
        }
        au
    }

    impl Backend for Fake {
        type Picture = u64;
        type Input = ();

        fn hotkey_pressed(&mut self) -> bool {
            std::mem::take(&mut self.script.lock().unwrap().hotkey)
        }
        fn input_signal(&mut self) -> Option<InputSignal> {
            None
        }
        fn healthy(&self) -> bool {
            true
        }
        fn look(&mut self) -> Observed {
            Observed::normal()
        }
        fn window_rect(&mut self) -> Option<Rect> {
            None
        }
        fn publish(&mut self, _: Rect, _: (u32, u32), _: InputPolicy, _: bool) {}
        fn grab(&mut self, wait_ms: u64) -> Grab<u64> {
            if let Some(why) = self.script.lock().unwrap().end {
                return Grab::End(why);
            }
            std::thread::sleep(Duration::from_millis(wait_ms.min(2)));
            self.pictures += 1;
            Grab::Frame { picture: self.pictures, size: (SIZE, SIZE), dirty: None, first_full: None }
        }
        fn resize(&mut self, _: (u32, u32), _: &Params, _: bool) -> Result<(), String> {
            Ok(())
        }
        fn software_encoder(&mut self, _: &Params) -> Result<(), String> {
            Ok(())
        }
        fn encoder_info(&self) -> EncoderInfo {
            EncoderInfo { hardware: true, name: "fake".into(), ..EncoderInfo::default() }
        }
        fn hardware(&self) -> bool {
            true
        }
        fn held(&self) -> u64 {
            0
        }
        fn first_output_ms(&self) -> Option<f64> {
            None
        }
        fn asks_for_input(&self) -> bool {
            false
        }
        fn set_bitrate(&mut self, _: u32) {}
        fn convert(&mut self, _: &u64) -> Result<(), String> {
            Ok(())
        }
        fn encode(&mut self, _: &(), time: i64, key: bool) -> Result<bool, String> {
            let (boom, hold) = {
                let mut s = self.script.lock().unwrap();
                s.encodes.push((time, key));
                (s.panic_on_encode, s.hold)
            };
            if boom {
                panic!("the encoder fell over");
            }
            if !hold {
                // A quantizer at the best there is: a still picture is not topped off, so that only the tiles under test follow.
                self.ready.push(Encoded { data: access_unit(key), key, time, qp: Some(8) });
            }
            Ok(true)
        }
        fn poll(&mut self) -> Result<Vec<Encoded>, String> {
            Ok(std::mem::take(&mut self.ready))
        }
        fn readback(&mut self, _: &u64, _: (u32, u32), _: Option<&[Rect]>) -> bool {
            self.buf.clone_from(&self.script.lock().unwrap().pixels);
            true
        }
        fn pixels(&self) -> (&[u8], usize) {
            (&self.buf, if self.buf.is_empty() { 0 } else { (SIZE * 4) as usize })
        }
        fn cursor(&mut self) -> (Shape, bool, i32, i32) {
            (Shape::Arrow, false, 0, 0)
        }
        fn finish(&mut self) {
            self.script.lock().unwrap().finished += 1;
        }
    }

    #[derive(Clone, Debug, PartialEq)]
    enum Seen {
        Video { key: bool, seq: u32, unpaint: Vec<Rect>, unpaint_all: bool, at: Duration },
        Tile { rect: Rect },
        Cursor,
        Event(Value),
    }

    impl Seen {
        fn is_video(&self) -> bool {
            matches!(self, Seen::Video { .. })
        }
        fn is_tile(&self) -> bool {
            matches!(self, Seen::Tile { .. })
        }
        fn event(&self) -> Option<&str> {
            match self {
                Seen::Event(v) => v["event"].as_str(),
                _ => None,
            }
        }
    }

    fn videos(seen: &[Seen]) -> Vec<&Seen> {
        seen.iter().filter(|s| s.is_video()).collect()
    }

    fn u16_at(m: &[u8], i: usize) -> i32 {
        i32::from(u16::from_le_bytes([m[i], m[i + 1]]))
    }

    fn rect_at(m: &[u8], i: usize) -> Rect {
        Rect::new(u16_at(m, i), u16_at(m, i + 2), u16_at(m, i + 4), u16_at(m, i + 6))
    }

    /// A stream on the fake machine, driven as the app would: its commands in, its stdout frames out.
    struct Run {
        cmds: Option<Sender<Cmd>>,
        out: Receiver<Vec<u8>>,
        deframer: Deframer,
        script: Arc<Mutex<Script>>,
        thread: Option<JoinHandle<()>>,
        began: Instant,
        /// Every video frame is acknowledged as it arrives (the phone keeps up).
        ack: bool,
        started: Value,
    }

    impl Run {
        fn start(tiles: bool, script: Script) -> Run {
            let script = Arc::new(Mutex::new(script));
            let (cmds, cmd_rx) = channel();
            let (out, out_rx) = sync_channel(4096);
            let (started_tx, started_rx) = channel();
            let (fake_script, told) = (script.clone(), out.clone());
            let began = Instant::now();
            let thread = std::thread::spawn(move || {
                // As the host runs one: a stream that panics is said to have ended, and the machine is let go of.
                crate::guarded(
                    || {
                        let cfg = Config { sc: "sc1".into(), window: false, src: (SIZE, SIZE), rect: Rect::new(0, 0, SIZE as i32, SIZE as i32), level: 0, tiles, max_kbps: None, title: "Screen 1".into(), exe: String::new() };
                        let ctl = controller(&cfg);
                        let mut fake = Fake { script: fake_script, pictures: 0, ready: Vec::new(), buf: Vec::new() };
                        run(cfg, ctl, &mut fake, cmd_rx, out, started_tx);
                    },
                    || emit(&told, json!({"event": "ended", "sc": "sc1", "reason": "failed"})),
                );
            });
            let started = started_rx.recv_timeout(Duration::from_secs(5)).ok().and_then(Result::ok).unwrap_or(Value::Null);
            Run { cmds: Some(cmds), out: out_rx, deframer: Deframer::default(), script, thread: Some(thread), began, ack: true, started }
        }

        fn send(&self, cmd: Cmd) {
            if let Some(c) = &self.cmds {
                let _ = c.send(cmd);
            }
        }

        fn flow(acked: u32) -> Cmd {
            Cmd::Flow { acked, rtt_ms: 0, level: 0, lost_tiles: false, dropped_through: 0, raw: 0, wire: 0, link_full_ms: 0, win: false }
        }

        fn see(&mut self, o: Out) -> Option<Seen> {
            match o {
                Out::Json(v) => Some(Seen::Event(v)),
                Out::Media { media, .. } => {
                    assert_eq!(media[0], MEDIA);
                    match media[1] {
                        KIND_VIDEO => {
                            let (n, seq) = (usize::from(media[3]), u32::from_le_bytes([media[4], media[5], media[6], media[7]]));
                            let unpaint = (0..n).map(|i| rect_at(&media, 20 + i * 8)).collect();
                            if self.ack {
                                self.send(Run::flow(seq));
                            }
                            Some(Seen::Video { key: media[2] & wire::FLAG_KEY != 0, seq, unpaint, unpaint_all: media[2] & wire::FLAG_UNPAINT_ALL != 0, at: self.began.elapsed() })
                        }
                        KIND_TILE => Some(Seen::Tile { rect: rect_at(&media, 12) }),
                        _ => Some(Seen::Cursor),
                    }
                }
            }
        }

        /// What the stream says in the next `ms` milliseconds.
        fn pump(&mut self, ms: u64) -> Vec<Seen> {
            let until = Instant::now() + Duration::from_millis(ms);
            let mut seen = Vec::new();
            loop {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return seen;
                }
                match self.out.recv_timeout(left) {
                    Ok(bytes) => {
                        self.deframer.push(&bytes);
                        while let Some(o) = self.deframer.take() {
                            seen.extend(self.see(o));
                        }
                    }
                    Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return seen,
                }
            }
        }

        /// What it says until `done` holds of all of it, or `max_ms` have passed.
        fn until(&mut self, max_ms: u64, done: impl Fn(&[Seen]) -> bool) -> Vec<Seen> {
            let until = Instant::now() + Duration::from_millis(max_ms);
            let mut seen = Vec::new();
            while Instant::now() < until && !done(&seen) {
                seen.extend(self.pump(20));
            }
            seen
        }

        /// The stream's end: what was left to say, with the thread joined.
        fn end(&mut self) -> Vec<Seen> {
            if let Some(t) = self.thread.take() {
                t.join().expect("the stream thread ends without a panic escaping");
            }
            let mut seen = Vec::new();
            while let Ok(bytes) = self.out.try_recv() {
                self.deframer.push(&bytes);
                while let Some(o) = self.deframer.take() {
                    seen.extend(self.see(o));
                }
            }
            seen
        }

        /// Ends it with `reason` as the app does, and says why it said it ended.
        fn stop(&mut self, reason: &str) -> String {
            self.send(Cmd::Stop(reason.into()));
            self.ended()
        }

        fn ended(&mut self) -> String {
            let mut seen = self.pump(0);
            seen.extend(self.end());
            let last = seen.last().expect("the stream said something");
            assert_eq!(last.event(), Some("ended"), "the end is the last thing said: {last:?}");
            match last {
                Seen::Event(v) => v["reason"].as_str().unwrap_or_default().to_string(),
                _ => unreachable!(),
            }
        }
    }

    impl Drop for Run {
        fn drop(&mut self) {
            self.send(Cmd::Stop("test".into()));
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }

    #[test]
    fn a_stream_says_how_it_started_and_its_first_frame_is_a_keyframe() {
        let mut run = Run::start(false, Script::new());
        assert_eq!((run.started["kind"].as_str(), run.started["w"].as_u64(), run.started["encoder"].as_str(), run.started["encoder_name"].as_str()), (Some("monitor"), Some(128), Some("hw"), Some("fake")));
        let seen = run.until(3000, |s| videos(s).len() >= 4);
        assert_eq!(seen[0].event(), Some("encoder"), "the encoder is told before any frame");
        let keys: Vec<bool> = videos(&seen).iter().map(|v| matches!(v, Seen::Video { key: true, .. })).collect();
        assert_eq!(&keys[..4], &[true, false, false, false], "a keyframe first, then deltas");
        let seqs: Vec<u32> = videos(&seen).iter().map(|v| if let Seen::Video { seq, .. } = v { *seq } else { 0 }).collect();
        assert_eq!(&seqs[..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn a_keyframe_asked_for_twice_in_the_first_second_comes_once_after_it() {
        let mut run = Run::start(false, Script::new());
        run.until(3000, |s| videos(s).len() >= 3);
        let asked = run.began.elapsed();
        run.send(Cmd::Keyframe);
        run.send(Cmd::Keyframe);
        let is_key = |s: &Seen| matches!(s, Seen::Video { key: true, .. });
        let seen = run.until(3000, |s| s.iter().any(is_key));
        let at_of = |s: &Seen| if let Seen::Video { at, .. } = s { *at } else { Duration::ZERO };
        let Some(key) = seen.iter().find(|s| is_key(s)) else { panic!("no keyframe came after the request") };
        assert!(at_of(key) >= Duration::from_millis(900), "not within a second of the first keyframe: {:?}", at_of(key));
        // While the keyframe waits for its second, no other frame is encoded (it would only be dropped): what came before it was already on
        // its way when it was asked for.
        for v in videos(&seen).into_iter().filter(|s| !is_key(s)) {
            assert!(at_of(v) < asked + Duration::from_millis(250), "a frame at {:?} while the keyframe waited", at_of(v));
        }
        let after = run.pump(400);
        assert!(!videos(&after).is_empty());
        assert!(videos(&after).iter().all(|v| matches!(v, Seen::Video { key: false, .. })), "asked twice, made once");
    }

    #[test]
    fn frames_the_phone_has_not_acknowledged_stop_at_the_window_and_go_on_when_it_does() {
        let mut run = Run::start(false, Script::new());
        run.ack = false;
        let seen = run.pump(700);
        assert_eq!(videos(&seen).len(), crate::control::MIN_WINDOW as usize, "no more than the window in flight");
        run.send(Run::flow(3));
        let more = run.until(3000, |s| videos(s).len() >= 2);
        assert!(videos(&more).len() >= 2, "room again once it is acknowledged");
    }

    #[test]
    fn one_frame_is_in_the_encoder_until_it_comes_back_or_a_second_has_passed() {
        let script = Script { hold: true, ..Script::new() };
        let mut run = Run::start(false, script);
        run.pump(600);
        assert_eq!(run.script.lock().unwrap().encodes.len(), 1, "the next picture waits for the frame in the encoder");
        run.pump(900);
        assert!(run.script.lock().unwrap().encodes.len() >= 2, "a frame the encoder never gives back is given up on");
    }

    #[test]
    fn a_painted_tile_is_unpainted_with_the_next_frame_when_its_cell_changes() {
        let mut run = Run::start(true, Script::new());
        let seen = run.until(4000, |s| s.iter().any(Seen::is_tile));
        let Some(Seen::Tile { rect }) = seen.iter().find(|s| s.is_tile()) else { panic!("no tile came for a still picture") };
        assert_eq!(*rect, Rect::new(0, 0, SIZE as i32, SIZE as i32), "the still cells come as one region");
        assert!(videos(&seen).iter().all(|v| matches!(v, Seen::Video { unpaint, unpaint_all: false, .. } if unpaint.is_empty())), "nothing was painted before");
        {
            let mut s = run.script.lock().unwrap();
            for row in 0..64usize {
                let start = row * (SIZE as usize) * 4;
                s.pixels[start..start + 64 * 4].fill(250);
            }
        }
        let seen = run.until(3000, |s| s.iter().any(|v| matches!(v, Seen::Video { unpaint, .. } if !unpaint.is_empty())));
        let Some(Seen::Video { unpaint, unpaint_all, .. }) = seen.iter().find(|v| matches!(v, Seen::Video { unpaint, .. } if !unpaint.is_empty())) else { panic!("the changed cell was never unpainted") };
        assert_eq!((unpaint.as_slice(), *unpaint_all), (&[Rect::new(0, 0, 64, 64)][..], false), "only the cell that changed");
    }

    #[test]
    fn a_new_connection_gets_a_keyframe_that_unpaints_everything() {
        let mut run = Run::start(true, Script::new());
        run.until(4000, |s| s.iter().any(Seen::is_tile));
        run.send(Cmd::Pause(true));
        run.pump(100);
        assert!(videos(&run.pump(400)).is_empty(), "nothing is sent to a link that is paused");
        run.send(Cmd::Pause(false));
        let seen = run.until(3000, |s| s.iter().any(Seen::is_video));
        assert!(matches!(seen.iter().find(|s| s.is_video()), Some(Seen::Video { key: true, unpaint_all: true, .. })), "{seen:?}");
    }

    #[test]
    fn a_sharpen_sends_only_the_part_asked_for_when_the_link_is_too_slow_for_tiles() {
        let mut run = Run::start(true, Script::new());
        // Below the bitrate crisp tiles need: they do not go by themselves.
        run.send(Cmd::Cap(Some(500)));
        assert!(!run.pump(1200).iter().any(Seen::is_tile), "no crisp tile on a slow link");
        run.send(Cmd::Sharpen(Rect::new(0, 0, 64, 64)));
        let seen = run.until(4000, |s| s.iter().any(Seen::is_tile));
        let Some(Seen::Tile { rect }) = seen.iter().find(|s| s.is_tile()) else { panic!("no tile came for the part asked for") };
        assert_eq!(*rect, Rect::new(0, 0, 64, 64));
        assert!(!run.pump(800).iter().any(Seen::is_tile), "the rest of the picture stays with the video");
    }

    #[test]
    fn a_stream_ends_for_the_reason_it_was_given_and_lets_the_machine_go_once() {
        let mut run = Run::start(false, Script::new());
        run.until(2000, |s| !videos(s).is_empty());
        assert_eq!(run.stop("replaced"), "replaced");
        assert_eq!(run.script.lock().unwrap().finished, 1);
        assert!(run.script.lock().unwrap().dropped);

        let mut run = Run::start(false, Script { hotkey: true, ..Script::new() });
        assert_eq!(run.ended(), "hotkey");
        assert_eq!(run.script.lock().unwrap().finished, 1);

        let mut run = Run::start(false, Script { end: Some("display_changed"), ..Script::new() });
        assert_eq!(run.ended(), "display_changed", "what the capture says");
        assert_eq!(run.script.lock().unwrap().finished, 1);

        let mut run = Run::start(false, Script::new());
        run.until(2000, |s| !videos(s).is_empty());
        run.cmds = None;
        assert_eq!(run.ended(), "app_closed", "the app went away");
        assert_eq!(run.script.lock().unwrap().finished, 1);
    }

    #[test]
    fn a_stream_that_panics_says_it_ended_and_still_lets_the_machine_go() {
        let mut run = Run::start(false, Script { panic_on_encode: true, ..Script::new() });
        assert_eq!(run.ended(), "failed");
        let s = run.script.lock().unwrap();
        assert!(s.dropped, "the backend is dropped as the panic unwinds");
        assert_eq!(s.finished, 0, "a normal end is not claimed");
    }

    #[test]
    fn the_loops_commands_are_read_from_a_line_and_the_rest_are_left_to_the_host() {
        let flow = json!({"acked": 7, "rtt_ms": 90_000, "level": 9, "lost_tiles": true, "link_full_ms": 99_999_999, "win": true});
        assert!(matches!(Cmd::parse("flow", &flow), Parsed::Cmd(Cmd::Flow { acked: 7, rtt_ms: 60_000, level: 3, lost_tiles: true, link_full_ms: 3_600_000, win: true, .. })));
        assert!(matches!(Cmd::parse("sharpen", &json!({"rect": {"x": 1, "y": 2, "w": 3, "h": 4}})), Parsed::Cmd(Cmd::Sharpen(r)) if r == Rect::new(1, 2, 3, 4)));
        assert!(matches!(Cmd::parse("sharpen", &json!({"rect": {"x": 1, "y": 2, "w": 70_000, "h": 4}})), Parsed::Nothing), "not a rectangle the picture can have");
        assert!(matches!(Cmd::parse("cap", &json!({"max_kbps": 500})), Parsed::Cmd(Cmd::Cap(Some(500)))));
        assert!(matches!(Cmd::parse("pause", &json!({"paused": true})), Parsed::Cmd(Cmd::Pause(true))));
        for host_only in ["input", "text", "stop", "nonsense"] {
            assert!(matches!(Cmd::parse(host_only, &json!({})), Parsed::Unknown), "{host_only}");
        }
    }
}
