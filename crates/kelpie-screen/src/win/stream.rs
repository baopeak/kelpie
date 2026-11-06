//! One stream, from `start` to its end, and `Host`, which the stdin commands reach.
//!
//! A stream is two threads. The **stream thread** owns the device, the capture, the converter and the encoder: it looks at the machine four
//! times a second (states, the window's place, the border), takes the newest picture when the phone has room and the rung's interval has
//! passed (`control`), encodes it, rewrites the SPS (`sps`), sends video, paint-over tiles (`tiles`) and the cursor, and once a second the
//! stats. The **input thread** does the phone's input as it comes, so that encoding never delays a click; it reads the target's place and
//! what the state allows from what the stream thread last wrote.
//!
//! Frames lost on the way never hold the stream (`control::Flight`): a keyframe request, or the daemon saying what it dropped, gives up the
//! frames in flight. A frame that is not sent after all (too large) puts its unpaint rectangles back, so a crisp tile is never left over a
//! change. Sharing does not start without the border and the stop hotkey.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

use super::border::Border;
use super::capture_dxgi::{self, Dxgi};
use super::capture_wgc::{self, Wgc};
use super::convert::Converter;
use super::d3d::{self, Device, Readback};
use super::encode_mf::{self, Encoder};
use super::input_win::{self, Geometry, Outcome};
use super::states_win::Looker;
use super::targets_win::{self, hwnd};
use super::{system, wall_ms};
use crate::control::{Controller, Event, Flight, Params, Stats};
use crate::geom::{self, Rect};
use crate::input::{fresh, parse_hotkey, Hotkey, QUEUE};
use crate::sps;
use crate::states::{input_policy, InputPolicy, State, Tracker};
use crate::targets::{self, TargetId};
use crate::tiles::Grid;
use crate::tile_image;
use crate::wire::{self, Video};

const LOOK_EVERY: Duration = Duration::from_millis(250);
/// How often the monitors are looked at again (one plugged in or out moves the virtual desktop `SendInput` maps over).
const DESKTOP_EVERY: Duration = Duration::from_secs(2);
const CURSOR_EVERY: Duration = Duration::from_millis(33);
/// How long the loop waits for a picture while frames are still in the encoder (its output is taken on the way round).
const ENCODER_POLL_MS: u64 = 4;
/// Paint-over tiles: at most this share of the video's rate, with this much at once.
const TILE_SHARE: f64 = 0.5;
const TILE_BURST: f64 = 256.0 * 1024.0;
const TILE_CELLS_AT_ONCE: usize = 32;
/// How long `start` waits for the stream's first look (the app waits a little longer, the daemon longer still).
const START_WAIT: Duration = Duration::from_secs(7);

fn emit(out: &SyncSender<Vec<u8>>, v: Value) {
    let _ = out.send(wire::out_json(&v));
}

enum Cmd {
    Flow { acked: u32, rtt_ms: u32, level: u8, lost_tiles: bool, dropped_through: u32, raw: u64, wire: u64 },
    Keyframe,
    Pause(bool),
    ResumeLoad,
    Stop(String),
}

enum InputCmd {
    Ops(Vec<Value>, Option<u64>),
    Text(String, Option<u64>, u64),
}

enum Source {
    Monitor(Dxgi, ID3D11Texture2D),
    Window(Wgc, u64, u32),
}

struct Shared {
    geometry: Geometry,
    policy: InputPolicy,
}

unsafe impl Send for Shared {}

struct Running {
    sc: String,
    cmds: Sender<Cmd>,
    input: SyncSender<InputCmd>,
    /// Set before the stream is told to stop: input still queued is not done.
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

pub struct Host {
    out: SyncSender<Vec<u8>>,
    running: Option<Running>,
}

impl Host {
    pub fn new(out: SyncSender<Vec<u8>>) -> Host {
        Host { out, running: None }
    }

    pub fn targets(&self) -> Value {
        let monitors = d3d::monitors();
        let windows = targets_win::windows();
        json!({"targets": targets::targets(&monitors, &windows, std::process::id())})
    }

    /// Starts a stream (ending the one before, if any): its kind, title and size once its first look is done.
    pub fn start(&mut self, req: &Value) -> Result<Value, String> {
        self.stop("replaced");
        let sc = req["sc"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).ok_or("missing sc")?.to_string();
        let target = req["target"].as_str().and_then(TargetId::parse).ok_or("not a target this computer has")?;
        let device: String = req["device"].as_str().unwrap_or("a phone").chars().filter(|c| !c.is_control()).take(40).collect();
        let hotkey_text = req["hotkey"].as_str().unwrap_or(crate::input::DEFAULT_HOTKEY).to_string();
        // The stop hotkey is always on: a screen is not shared without one that works.
        let hotkey = parse_hotkey(&hotkey_text).ok_or("the stop hotkey in Settings → Mobile is not one that can be used")?;
        let opts = Opts { sc: sc.clone(), target, device, hotkey, hotkey_text, level: req["level"].as_u64().unwrap_or(0).min(3) as u8, tiles: req["tiles"].as_bool() != Some(false) };
        let (cmds, cmd_rx) = channel();
        let (input, input_rx) = sync_channel(QUEUE);
        let (started_tx, started_rx) = channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let out = self.out.clone();
        let flag = stopping.clone();
        let (told, told_sc) = (self.out.clone(), sc.clone());
        let thread = std::thread::spawn(move || {
            // A stream that fails on its way is said to have ended, never left frozen without a word (its border and input go with it).
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(opts, cmd_rx, input_rx, out, started_tx, flag))).is_err() {
                system::streaming(false);
                emit(&told, json!({"event": "ended", "sc": told_sc, "reason": "failed"}));
            }
        });
        let mut running = Running { sc, cmds, input, stopping, thread: Some(thread) };
        match started_rx.recv_timeout(START_WAIT) {
            Ok(Ok(v)) => {
                self.running = Some(running);
                Ok(v)
            }
            Ok(Err(e)) => {
                if let Some(t) = running.thread.take() {
                    let _ = t.join();
                }
                Err(e)
            }
            Err(_) => {
                // Too slow: it is stopped, not left to start after the app gave up on it.
                self.running = Some(running);
                self.stop("failed");
                Err("the screen did not start in time".into())
            }
        }
    }

    fn current(&self, req: &Value) -> Option<&Running> {
        self.running.as_ref().filter(|r| Some(r.sc.as_str()) == req["sc"].as_str())
    }

    pub fn stop(&mut self, reason: &str) {
        if let Some(mut r) = self.running.take() {
            r.stopping.store(true, Ordering::SeqCst);
            let _ = r.cmds.send(Cmd::Stop(reason.to_string()));
            if let Some(t) = r.thread.take() {
                let _ = t.join();
            }
        }
    }

    /// A command for the stream (`main.rs` parses the line).
    pub fn command(&mut self, cmd: &str, req: &Value) -> Option<Result<Value, String>> {
        if cmd == "stop" {
            if self.current(req).is_some() {
                self.stop(req["reason"].as_str().unwrap_or("stop"));
            }
            return None;
        }
        let Some(r) = self.current(req) else {
            return (cmd == "text").then(|| Err("that screen has ended".to_string()));
        };
        let t = req["t"].as_u64();
        let n = |k: &str| req[k].as_u64().unwrap_or(0);
        match cmd {
            "input" => {
                let ops = crate::input::checked_ops(&req["ops"])?;
                // A full queue drops the newest: the phone's next input finds room.
                let _ = r.input.try_send(InputCmd::Ops(ops, t));
                None
            }
            "text" => {
                let Some(text) = req["text"].as_str().and_then(crate::input::checked_text) else { return Some(Err("not text that can be typed".into())) };
                Some(match r.input.try_send(InputCmd::Text(text.to_string(), t, n("id"))) {
                    // The answer comes from the input thread, once typed.
                    Ok(()) => return None,
                    Err(_) => Err("too much input waiting".into()),
                })
            }
            "flow" => {
                let _ = r.cmds.send(Cmd::Flow {
                    acked: n("acked") as u32,
                    rtt_ms: n("rtt_ms").min(60_000) as u32,
                    level: n("level").min(3) as u8,
                    lost_tiles: req["lost_tiles"].as_bool() == Some(true),
                    dropped_through: n("dropped_through") as u32,
                    raw: n("raw"),
                    wire: n("wire"),
                });
                None
            }
            "keyframe" => {
                let _ = r.cmds.send(Cmd::Keyframe);
                None
            }
            "pause" => {
                let _ = r.cmds.send(Cmd::Pause(req["paused"].as_bool() == Some(true)));
                None
            }
            "resume_load" => {
                let _ = r.cmds.send(Cmd::ResumeLoad);
                None
            }
            _ => Some(Err(format!("unknown command {cmd:?}"))),
        }
    }
}

struct Opts {
    sc: String,
    target: TargetId,
    device: String,
    hotkey: Hotkey,
    hotkey_text: String,
    level: u8,
    tiles: bool,
}

/// `KELPIE_SCREEN_TEST_ENCODE_DELAY_MS`: added to every encode, to drive the ladder where there is no GPU worth measuring.
fn test_delay() -> Option<Duration> {
    std::env::var("KELPIE_SCREEN_TEST_ENCODE_DELAY_MS").ok()?.parse::<u64>().ok().filter(|ms| *ms > 0 && *ms <= 2000).map(Duration::from_millis)
}

/// The device, the capture, where the target is, its title (shown on the desktop and the phone) and, for a window, its program's file name (all
/// the audit log records of it).
fn open_source(opts: &Opts) -> Result<(Device, Source, Rect, String, String), String> {
    match &opts.target {
        TargetId::Monitor(name) => {
            let outputs = d3d::outputs().map_err(|e| e.to_string())?;
            let mut sorted: Vec<_> = outputs.into_iter().collect();
            sorted.sort_by_key(|(m, ..)| !m.primary);
            let index = sorted.iter().position(|(m, ..)| &m.name == name).ok_or("that screen is no longer connected")?;
            let (monitor, adapter, output) = sorted.swap_remove(index);
            let device = Device::on(&adapter).map_err(|e| e.to_string())?;
            let dxgi = Dxgi::new(&device, output, monitor.rect)?;
            // The encoder's sides must be even: the picture is the monitor's top-left even part.
            let tex = device.frame_texture(monitor.rect.w.max(2) as u32 & !1, monitor.rect.h.max(2) as u32 & !1).map_err(|e| e.to_string())?;
            Ok((device, Source::Monitor(dxgi, tex), monitor.rect, format!("Screen {}", index + 1), String::new()))
        }
        TargetId::Window { hwnd: h, pid } => {
            let win = hwnd(*h);
            if !targets_win::exists(win, *pid) {
                return Err("that window was closed".into());
            }
            // Only what the list would offer: not a window on another virtual desktop, a tool palette, or the sharing border.
            let info = targets_win::info(win);
            if !targets::listed(&info, std::process::id()) {
                return Err("that window cannot be shared".into());
            }
            let rect = targets_win::window_rect(win).ok_or("that window cannot be shown")?;
            let adapter = d3d::adapter_at(rect.x + rect.w / 2, rect.y + rect.h / 2).map_err(|e| e.to_string())?;
            let device = Device::on(&adapter).map_err(|e| e.to_string())?;
            let wgc = Wgc::start(&device, win).map_err(|e| format!("this window cannot be captured: {e}"))?;
            let title = info.title.chars().take(120).collect();
            Ok((device, Source::Window(wgc, *h, *pid), rect, title, info.exe))
        }
    }
}

fn run(opts: Opts, cmds: Receiver<Cmd>, input_rx: Receiver<InputCmd>, out: SyncSender<Vec<u8>>, started: Sender<Result<Value, String>>, stopping: Arc<AtomicBool>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let _ = encode_mf::startup();
    let _mmcss = system::Mmcss::capture();
    let (d, mut source, mut rect, title, exe) = match open_source(&opts) {
        Ok(v) => v,
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    let window = matches!(source, Source::Window(..));
    let window_handle = match &source {
        Source::Window(_, h, pid) => Some((hwnd(*h), *pid)),
        _ => None,
    };
    let mut src = (rect.w.max(2) as u32 & !1, rect.h.max(2) as u32 & !1);
    let started_at = Instant::now();
    let now_ms = || started_at.elapsed().as_millis() as u64;
    let mut ctl = Controller::new(src, window, false, 0);
    ctl.set_level(opts.level, 0);
    let mut params: Params = ctl.params();
    let delay = test_delay();
    let mut encoder = match Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, true).or_else(|_| Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, false)) {
        Ok(e) => e,
        Err(e) => {
            let _ = started.send(Err(format!("no H.264 encoder works on this computer: {e}")));
            return;
        }
    };
    if !encoder.hardware {
        ctl.set_software(true);
        params = ctl.params();
    }
    let mut conv = match Converter::new(&d, src, (params.w, params.h), u32::from(params.fps)) {
        Ok(c) => c,
        Err(e) => {
            let _ = started.send(Err(format!("the picture cannot be converted for the encoder: {e}")));
            return;
        }
    };
    let mut ps = sps::Params::default();
    if let Some(h) = &encoder.sequence_header {
        ps.seed(h);
    }
    // The border and the hotkey first: without them nothing is shared, and the phone is told why.
    let (hk_tx, hk_rx) = channel();
    let label = format!("Shared with {} · {} stops it", opts.device, opts.hotkey_text);
    let border = match Border::show(rect, &label, opts.hotkey, hk_tx) {
        Ok(b) => b,
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    system::streaming(true);
    // Input: its own thread, with the place and the policy the stream thread keeps up to date.
    let mut desktop = geom::virtual_desktop(&d3d::monitors());
    let shared = Arc::new(Mutex::new(Shared { geometry: Geometry { window: window_handle.map(|(h, _)| h), rect, src, desktop }, policy: input_policy(State::Live, window) }));
    let (outcome_tx, outcome_rx) = channel::<Outcome>();
    let input_thread = {
        let (shared, out, stopping) = (shared.clone(), out.clone(), stopping.clone());
        std::thread::spawn(move || input_loop(input_rx, shared, outcome_tx, out, stopping))
    };
    let _ = started.send(Ok(json!({"kind": if window { "window" } else { "monitor" }, "title": title, "exe": exe, "w": src.0, "h": src.1, "encoder": if encoder.hardware { "hw" } else { "sw" }, "encoder_name": encoder.name})));
    emit(&out, json!({"event": "encoder", "sc": opts.sc, "encoder": if encoder.hardware { "hw" } else { "sw" }, "name": encoder.name, "refused": encoder.refused}));

    let mut tracker = Tracker::default();
    let looker = Looker::new();
    let mut stats = Stats::new(0);
    let mut grid = opts.tiles.then(|| Grid::new(src.0, src.1, 0));
    let mut readback = Readback::new();
    let mut flight = Flight::default();
    let mut last_encode: Option<u64> = None;
    let mut times: std::collections::VecDeque<(i64, Instant)> = std::collections::VecDeque::new();
    let mut link_paused = false;
    let (mut unpaint, mut unpaint_all) = (Vec::<Rect>::new(), false);
    let (mut tile_id, mut tile_tokens, mut tile_at) = (0u32, TILE_BURST, now_ms());
    let (mut last_look, mut last_desktop, mut last_cursor) = (Instant::now() - LOOK_EVERY, Instant::now(), Instant::now());
    let mut cursor_told: Option<(u8, bool, i32, i32)> = None;
    // The last picture taken: encoded again when a keyframe is owed and nothing new came (a still window sends no frames by itself).
    let mut last_picture: Option<ID3D11Texture2D> = None;
    let reason: String = 'stream: loop {
        let now = now_ms();
        // What the app said.
        loop {
            match cmds.try_recv() {
                Ok(Cmd::Stop(r)) => break 'stream r,
                Ok(Cmd::Flow { acked, rtt_ms, level, lost_tiles, dropped_through, raw, wire }) => {
                    flight.acked(acked);
                    // Frames the daemon dropped will never be acknowledged: they no longer count as in flight.
                    flight.dropped_through(dropped_through);
                    if rtt_ms > 0 {
                        ctl.bandwidth.on_ack(rtt_ms, now);
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
                Ok(Cmd::Pause(p)) => {
                    link_paused = p;
                    if !p {
                        // A new connection: a fresh keyframe, and every tile painted again on it.
                        flight.restart();
                        if let Some(g) = &mut grid {
                            g.forget_painted();
                        }
                        unpaint_all = true;
                    }
                }
                Ok(Cmd::ResumeLoad) => {
                    ctl.resume(now);
                    flight.restart();
                    emit(&out, json!({"event": "state", "sc": opts.sc, "state": tracker.state().name()}));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'stream "app_closed".to_string(),
            }
        }
        if hk_rx.try_recv().is_ok() {
            break 'stream "hotkey".to_string();
        }
        while let Ok(o) = outcome_rx.try_recv() {
            match o {
                Outcome::Covered => {
                    if let Some(s) = tracker.covered() {
                        emit(&out, json!({"event": "state", "sc": opts.sc, "state": s.name()}));
                    }
                }
                Outcome::NotFront => emit(&out, json!({"event": "input_refused", "sc": opts.sc, "why": "not_front"})),
                Outcome::Blocked => emit(&out, json!({"event": "input_blocked", "sc": opts.sc})),
                _ => {}
            }
        }
        // Four times a second: the states, the window's place; every two seconds, the monitors.
        if last_look.elapsed() >= LOOK_EVERY {
            last_look = Instant::now();
            if let Some(s) = tracker.look(&looker.look(window_handle), window) {
                emit(&out, json!({"event": "state", "sc": opts.sc, "state": s.name()}));
                if s.ends() {
                    break 'stream "closed".to_string();
                }
                if s == State::Live {
                    flight.restart();
                }
            }
            if let Some((h, _)) = window_handle {
                if let Some(r) = targets_win::window_rect(h) {
                    rect = r;
                    border.place(r);
                }
            }
            if last_desktop.elapsed() >= DESKTOP_EVERY {
                last_desktop = Instant::now();
                let monitors = d3d::monitors();
                if !monitors.is_empty() {
                    desktop = geom::virtual_desktop(&monitors);
                }
            }
            if let Ok(mut s) = shared.lock() {
                s.geometry.rect = rect;
                s.geometry.src = src;
                s.geometry.desktop = desktop;
                s.policy = input_policy(tracker.state(), window);
            }
        }
        flight.tick(now);
        if let Some(Event::PausedForLoad) = ctl.tick(flight.inflight(), now) {
            emit(&out, json!({"event": "paused", "sc": opts.sc, "reason": "load"}));
        }
        // What the ladder says now: a new size is a new converter and encoder; a new bitrate is set live.
        let next = ctl.params();
        if (next.w, next.h) != (params.w, params.h) {
            match (Converter::new(&d, src, (next.w, next.h), u32::from(next.fps)), Encoder::open(&d, next.w, next.h, u32::from(next.fps), next.kbps, !ctl.software())) {
                (Ok(c), Ok(e)) => {
                    conv = c;
                    encoder = e;
                    if let Some(h) = &encoder.sequence_header {
                        ps.seed(h);
                    }
                    times.clear();
                    flight.restart();
                }
                _ => break 'stream "encoder_failed".to_string(),
            }
        } else if next.kbps != params.kbps {
            encoder.set_bitrate(next.kbps);
        }
        params = next;

        let since_last = last_encode.map(|t| now.saturating_sub(t));
        // A keyframe owed but not yet allowed: anything else would only be dropped on the way.
        let may = !link_paused && !flight.gated() && tracker.state().picture_moves() && ctl.may_encode(flight.inflight(), since_last);
        // A frame the encoder never gave back (it may drop one) is not waited for after a second.
        while times.front().is_some_and(|(_, at)| at.elapsed() > Duration::from_secs(1)) {
            times.pop_front();
        }
        // With frames in the encoder the loop comes back within a few milliseconds to take them: its time is then the encoder's own, not
        // how long the loop waited for a picture (which the overuse detector would read as a slow encoder, and lower the rate for).
        let wait = Duration::from_millis(if !times.is_empty() { ENCODER_POLL_MS } else if may { 30 } else { 50 });
        let grabbed: Option<(ID3D11Texture2D, Option<Vec<Rect>>)> = if !may {
            std::thread::sleep(wait);
            None
        } else {
            match &mut source {
                Source::Monitor(dxgi, tex) => match dxgi.grab(&d, wait.as_millis() as u32, tex) {
                    capture_dxgi::Grab::Frame { dirty } => Some((tex.clone(), dirty)),
                    capture_dxgi::Grab::Changed => break 'stream "display_changed".to_string(),
                    _ => None,
                },
                Source::Window(wgc, ..) => match wgc.grab(wait) {
                    (capture_wgc::Grab::Frame { w, h }, Some(t)) => {
                        if (w, h) != src {
                            // Resized: everything that depends on the size again.
                            src = (w, h);
                            ctl.set_source(src);
                            grid = opts.tiles.then(|| Grid::new(w, h, now));
                            unpaint_all = true;
                            params = ctl.params();
                            match (Converter::new(&d, src, (params.w, params.h), u32::from(params.fps)), Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, !ctl.software())) {
                                (Ok(c), Ok(e)) => {
                                    conv = c;
                                    encoder = e;
                                    times.clear();
                                    flight.restart();
                                }
                                _ => break 'stream "encoder_failed".to_string(),
                            }
                        }
                        Some((t, None))
                    }
                    _ => {
                        if wgc.closed.load(Ordering::SeqCst) {
                            break 'stream "closed".to_string();
                        }
                        None
                    }
                },
            }
        };
        // A keyframe owed and allowed, and no new picture: the last one again, or the phone (and the daemon, which holds input back while one
        // is owed) would wait for the window to change.
        let grabbed = match grabbed {
            Some((tex, dirty)) => {
                last_picture = Some(tex.clone());
                Some((tex, dirty))
            }
            None if may && flight.wants_key() => last_picture.clone().map(|t| (t, None)),
            None => None,
        };
        if let Some((tex, dirty)) = grabbed {
            if let Some(g) = &mut grid {
                if readback.read(&d, &tex, src.0, src.1).is_ok() {
                    unpaint.extend(g.update(&readback.buf, readback.stride, now, dirty.as_deref()));
                }
            }
            if let Ok(nv12) = conv.convert(&tex) {
                let time = (started_at.elapsed().as_nanos() / 100) as i64;
                let began = Instant::now();
                let key = flight.wants_key();
                match encoder.encode(&d, &nv12, time, key) {
                    Ok(true) => {
                        if let Some(extra) = delay {
                            std::thread::sleep(extra);
                        }
                        flight.encoded(key, now);
                        last_encode = Some(now);
                        times.push_back((time, began));
                        while times.len() > 8 {
                            times.pop_front();
                        }
                    }
                    Ok(false) => stats.dropped_encoder += 1,
                    Err(_) => {
                        // The hardware encoder failed: the software one from now on, and said so.
                        if encoder.hardware {
                            if let Ok(e) = Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, false) {
                                encoder = e;
                                ctl.set_software(true);
                                flight.restart();
                                emit(&out, json!({"event": "encoder", "sc": opts.sc, "encoder": "sw", "name": encoder.name, "refused": encoder.refused, "why": "failed"}));
                                continue;
                            }
                        }
                        break 'stream "encoder_failed".to_string();
                    }
                }
            }
        }
        // What the encoder finished.
        match encoder.poll() {
            Ok(done) => {
                for e in done {
                    let encode_ms = times.iter().position(|(t, _)| *t == e.time).map(|i| times.remove(i).expect("found").1.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0) + delay.map_or(0.0, |d| d.as_secs_f64() * 1000.0);
                    if let Some(Event::UseSoftware) = ctl.on_encoded(encode_ms, now) {
                        if let Ok(sw) = Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, false) {
                            encoder = sw;
                            ctl.set_software(true);
                            flight.restart();
                            emit(&out, json!({"event": "encoder", "sc": opts.sc, "encoder": "sw", "name": encoder.name, "refused": encoder.refused, "why": "slow"}));
                            break;
                        }
                    }
                    let Some(au) = ps.fix(&e.data) else {
                        // Not sendable: what follows would refer to it.
                        flight.restart();
                        continue;
                    };
                    let key = e.key || sps::is_keyframe(&au);
                    let v = Video { key, seq: flight.seq() + 1, capture_ms: now as u32, src: (src.0 as u16, src.1 as u16), coded: (params.w as u16, params.h as u16), unpaint: std::mem::take(&mut unpaint), unpaint_all: std::mem::take(&mut unpaint_all) };
                    let msg = wire::video(&v, &au);
                    if msg.len() > wire::MAX_MEDIA {
                        // Too large for one message: not sent. Its unpaint goes with the next frame, less bitrate, and a keyframe.
                        unpaint = v.unpaint;
                        unpaint_all = v.unpaint_all;
                        ctl.bandwidth.kbps = (ctl.bandwidth.kbps * 3 / 4).max(crate::control::MIN_KBPS);
                        flight.restart();
                        stats.dropped_link += 1;
                        continue;
                    }
                    flight.sent();
                    stats.frame(msg.len(), key, encode_ms);
                    let _ = out.send(wire::out_media(&opts.sc, &msg));
                }
            }
            Err(_) => break 'stream "encoder_failed".to_string(),
        }
        // Paint-over: still regions as crisp tiles, from the last picture read, within a share of the video's rate.
        if let Some(g) = &mut grid {
            let rate = f64::from(params.kbps) * 1000.0 / 8.0 * TILE_SHARE;
            tile_tokens = (tile_tokens + rate * now.saturating_sub(tile_at) as f64 / 1000.0).min(TILE_BURST);
            tile_at = now;
            // Off by itself while the link is slow or was just cut (`Controller::crisp_ok`): the video needs the room.
            if flight.seq() > 0 && !flight.wants_key() && !link_paused && ctl.crisp_ok(flight.inflight(), now) && readback.stride > 0 && tile_tokens > 32_768.0 && g.any_due(now) && g.size() == src {
                for r in g.due(now, TILE_CELLS_AT_ONCE) {
                    match tile_image::encode(&readback.buf, readback.stride, r) {
                        Some((format, image)) if (image.len() as f64) <= tile_tokens => {
                            tile_id = tile_id.wrapping_add(1);
                            tile_tokens -= image.len() as f64;
                            let _ = out.send(wire::out_media(&opts.sc, &wire::tile(flight.seq(), tile_id, r, format, &image)));
                        }
                        _ => g.unmark(r),
                    }
                }
            }
        }
        // The cursor, where it is in the picture.
        if last_cursor.elapsed() >= CURSOR_EVERY {
            last_cursor = Instant::now();
            let (shape, shows, x, y) = targets_win::cursor();
            let at = geom::screen_to_source(x, y, rect, src.0, src.1);
            let now_c = (shape as u8, shows && at.is_some(), at.map_or(0, |p| p.0), at.map_or(0, |p| p.1));
            if cursor_told != Some(now_c) {
                cursor_told = Some(now_c);
                let _ = out.send(wire::out_media(&opts.sc, &wire::cursor(shape, now_c.1, now_c.2, now_c.3)));
            }
        }
        let crisp = opts.tiles.then(|| ctl.crisp_ok(flight.inflight(), now));
        if let Some(r) = stats.report(now, &params, ctl.level(), ctl.software(), crisp) {
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["event"] = json!("stats");
            v["sc"] = json!(opts.sc);
            emit(&out, v);
        }
    };
    stopping.store(true, Ordering::SeqCst);
    drop(border);
    drop(cmds);
    // The input thread ends once it sees it holds the last reference to what it reads.
    drop(shared);
    let _ = input_thread.join();
    system::streaming(false);
    emit(&out, json!({"event": "ended", "sc": opts.sc, "reason": reason}));
}

/// The phone's input, in the order it came, as long as it is fresh, the stream is not stopping and the state allows it. A button a `down` left
/// pressed is released when it ends.
fn input_loop(rx: Receiver<InputCmd>, shared: Arc<Mutex<Shared>>, outcomes: Sender<Outcome>, out: SyncSender<Vec<u8>>, stopping: Arc<AtomicBool>) {
    let _mmcss = system::Mmcss::capture();
    let mut pressed: Option<&'static str> = None;
    loop {
        let cmd = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                if Arc::strong_count(&shared) == 1 || stopping.load(Ordering::SeqCst) {
                    break;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if stopping.load(Ordering::SeqCst) {
            // What still waits is not done: the screen was stopped.
            if let InputCmd::Text(_, _, id) = cmd {
                emit(&out, json!({"id": id, "err": "the screen was stopped"}));
            }
            continue;
        }
        let (g, p) = match shared.lock() {
            Ok(s) => (s.geometry, s.policy),
            Err(_) => break,
        };
        match cmd {
            InputCmd::Ops(ops, t) => {
                if !fresh(t, wall_ms()) {
                    // Late: dropped, but a release still goes, so that no button stays pressed.
                    if ops.iter().any(|op| op["k"] == "up") {
                        input_win::release(&mut pressed);
                    }
                    continue;
                }
                let o = input_win::run(&ops, &g, p, &mut pressed);
                if o != Outcome::Done {
                    let _ = outcomes.send(o);
                }
            }
            InputCmd::Text(text, t, id) => {
                let reply = if !fresh(t, wall_ms()) {
                    json!({"id": id, "err": "it came too late"})
                } else {
                    match input_win::type_text(&text, &g, p) {
                        Outcome::Done => json!({"id": id, "ok": true}),
                        Outcome::NotFront => {
                            let _ = outcomes.send(Outcome::NotFront);
                            json!({"id": id, "err": "not_front: the shared window is not in front on the computer, so nothing was typed"})
                        }
                        Outcome::Blocked => json!({"id": id, "err": "Windows did not take it: an app run as administrator, or a secure screen"}),
                        _ => json!({"id": id, "err": "typing is not possible in this state"}),
                    }
                };
                emit(&out, reply);
            }
        }
    }
    input_win::release(&mut pressed);
}
