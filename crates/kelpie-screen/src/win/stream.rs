//! One stream, from `start` to its end, and `Host`, which the stdin commands reach.
//!
//! A stream is two threads. The **stream thread** owns the device, the capture, the converter and the encoder: it looks at the machine four
//! times a second (states, the window's place, the border), takes the newest picture when the phone has room and the rung's interval has
//! passed (`control`), encodes it, rewrites the SPS (`sps`), sends video, paint-over tiles (`tiles`) and the cursor, and once a second the
//! stats. The **input thread** does the phone's input as it comes, so that encoding never delays a click; it reads the target's place and
//! what the state allows from what the stream thread last wrote.

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
use crate::control::{Controller, Event, Params, Stats};
use crate::geom::{self, Rect};
use crate::input::{fresh, parse_hotkey, QUEUE};
use crate::sps;
use crate::states::{input_policy, InputPolicy, State, Tracker};
use crate::targets::{self, TargetId};
use crate::tiles::Grid;
use crate::tile_image;
use crate::wire::{self, Video};

const LOOK_EVERY: Duration = Duration::from_millis(250);
const CURSOR_EVERY: Duration = Duration::from_millis(33);
/// Paint-over tiles: at most this much a second over the video's own rate, with this much at once.
const TILE_BURST: f64 = 512.0 * 1024.0;
const TILE_CELLS_AT_ONCE: usize = 64;

fn emit(out: &SyncSender<Vec<u8>>, v: Value) {
    let _ = out.send(wire::out_json(&v));
}

enum Cmd {
    Flow { acked: u32, rtt_ms: u32, level: u8, lost_tiles: bool },
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
        let opts = Opts { sc: sc.clone(), target, device, hotkey: parse_hotkey(&hotkey_text), hotkey_text, level: req["level"].as_u64().unwrap_or(0).min(3) as u8, tiles: req["tiles"].as_bool() != Some(false) };
        let (cmds, cmd_rx) = channel();
        let (input, input_rx) = sync_channel(QUEUE);
        let (started_tx, started_rx) = channel();
        let out = self.out.clone();
        let thread = std::thread::spawn(move || run(opts, cmd_rx, input_rx, out, started_tx));
        let started = started_rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the screen did not start in time".to_string())?;
        let mut running = Running { sc, cmds, input, thread: Some(thread) };
        match started {
            Ok(v) => {
                self.running = Some(running);
                Ok(v)
            }
            Err(e) => {
                if let Some(t) = running.thread.take() {
                    let _ = t.join();
                }
                Err(e)
            }
        }
    }

    fn current(&self, req: &Value) -> Option<&Running> {
        self.running.as_ref().filter(|r| Some(r.sc.as_str()) == req["sc"].as_str())
    }

    pub fn stop(&mut self, reason: &str) {
        if let Some(mut r) = self.running.take() {
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
        match cmd {
            "input" => {
                let ops = crate::input::checked_ops(&req["ops"])?;
                // A full queue drops the newest: the phone's next input finds room.
                let _ = r.input.try_send(InputCmd::Ops(ops, t));
                None
            }
            "text" => {
                let Some(text) = req["text"].as_str().and_then(crate::input::checked_text) else { return Some(Err("not text that can be typed".into())) };
                let id = req["id"].as_u64().unwrap_or(0);
                Some(match r.input.try_send(InputCmd::Text(text.to_string(), t, id)) {
                    // The answer comes from the input thread, once typed.
                    Ok(()) => return None,
                    Err(_) => Err("too much input waiting".into()),
                })
            }
            "flow" => {
                let _ = r.cmds.send(Cmd::Flow { acked: req["acked"].as_u64().unwrap_or(0) as u32, rtt_ms: req["rtt_ms"].as_u64().unwrap_or(0).min(60_000) as u32, level: req["level"].as_u64().unwrap_or(0).min(3) as u8, lost_tiles: req["lost_tiles"].as_bool() == Some(true) });
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
    hotkey: Option<crate::input::Hotkey>,
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
fn open_source(d_for: &Opts) -> Result<(Device, Source, Rect, String, String), String> {
    match &d_for.target {
        TargetId::Monitor(name) => {
            let outputs = d3d::outputs().map_err(|e| e.to_string())?;
            let mut sorted: Vec<_> = outputs.into_iter().collect();
            sorted.sort_by_key(|(m, ..)| !m.primary);
            let index = sorted.iter().position(|(m, ..)| &m.name == name).ok_or("that screen is no longer connected")?;
            let (monitor, adapter, output) = sorted.swap_remove(index);
            let device = Device::on(&adapter).map_err(|e| e.to_string())?;
            let tex = device.frame_texture(monitor.rect.w as u32, monitor.rect.h as u32).map_err(|e| e.to_string())?;
            let dxgi = Dxgi::new(&device, output, monitor.rect);
            Ok((device, Source::Monitor(dxgi, tex), monitor.rect, format!("Screen {}", index + 1), String::new()))
        }
        TargetId::Window { hwnd: h, pid } => {
            let win = hwnd(*h);
            if !targets_win::exists(win, *pid) {
                return Err("that window was closed".into());
            }
            let rect = targets_win::window_rect(win).ok_or("that window cannot be shown")?;
            let adapter = d3d::adapter_at(rect.x + rect.w / 2, rect.y + rect.h / 2).map_err(|e| e.to_string())?;
            let device = Device::on(&adapter).map_err(|e| e.to_string())?;
            let wgc = Wgc::start(&device, win).map_err(|e| format!("this window cannot be captured: {e}"))?;
            let info = targets_win::info(win);
            let title = info.title.chars().take(120).collect();
            Ok((device, Source::Window(wgc, *h, *pid), rect, title, info.exe))
        }
    }
}

fn run(opts: Opts, cmds: Receiver<Cmd>, input_rx: Receiver<InputCmd>, out: SyncSender<Vec<u8>>, started: Sender<Result<Value, String>>) {
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
    system::streaming(true);
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
            system::streaming(false);
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
            system::streaming(false);
            return;
        }
    };
    let mut ps = sps::Params::default();
    if let Some(h) = &encoder.sequence_header {
        ps.seed(h);
    }
    // Input: its own thread, with the place and the policy the stream thread keeps up to date.
    let desktop = geom::virtual_desktop(&d3d::monitors());
    let shared = Arc::new(Mutex::new(Shared { geometry: Geometry { window: window_handle.map(|(h, _)| h), rect, src, desktop }, policy: input_policy(State::Live, window) }));
    let (outcome_tx, outcome_rx) = channel::<Outcome>();
    let input_thread = {
        let (shared, out) = (shared.clone(), out.clone());
        std::thread::spawn(move || input_loop(input_rx, shared, outcome_tx, out))
    };
    let (hk_tx, hk_rx) = channel();
    let label = format!("Shared with {} · {} stops it", opts.device, opts.hotkey_text);
    let border = Border::show(rect, &label, opts.hotkey, hk_tx);
    let _ = started.send(Ok(json!({"kind": if window { "window" } else { "monitor" }, "title": title, "exe": exe, "w": src.0, "h": src.1, "encoder": if encoder.hardware { "hw" } else { "sw" }, "encoder_name": encoder.name})));
    emit(&out, json!({"event": "encoder", "sc": opts.sc, "encoder": if encoder.hardware { "hw" } else { "sw" }, "name": encoder.name, "refused": encoder.refused}));

    let mut tracker = Tracker::default();
    let looker = Looker::new();
    let mut stats = Stats::new(0);
    let mut grid = opts.tiles.then(|| Grid::new(src.0, src.1, 0));
    let mut readback = Readback::new();
    let (mut seq, mut acked) = (0u32, 0u32);
    let mut last_encode: Option<u64> = None;
    let mut times: std::collections::VecDeque<(i64, Instant)> = std::collections::VecDeque::new();
    let (mut key_needed, mut link_paused) = (true, false);
    let (mut unpaint, mut unpaint_all) = (Vec::<Rect>::new(), false);
    let (mut tile_id, mut tile_tokens, mut tile_at) = (0u32, TILE_BURST, now_ms());
    let (mut last_look, mut last_cursor) = (Instant::now() - LOOK_EVERY, Instant::now());
    let mut cursor_told: Option<(u8, bool, i32, i32)> = None;
    let mut hotkey_told = false;
    let reason: String = 'stream: loop {
        let now = now_ms();
        // What the app said.
        loop {
            match cmds.try_recv() {
                Ok(Cmd::Stop(r)) => break 'stream r,
                Ok(Cmd::Flow { acked: a, rtt_ms, level, lost_tiles }) => {
                    acked = acked.max(a.min(seq));
                    if rtt_ms > 0 {
                        ctl.bandwidth.on_ack(rtt_ms, now);
                    }
                    ctl.set_level(level, now);
                    if lost_tiles {
                        if let Some(g) = &mut grid {
                            g.forget_painted();
                        }
                        unpaint_all = true;
                    }
                }
                Ok(Cmd::Keyframe) => {
                    if ctl.keyframes.request(now) {
                        key_needed = true;
                    }
                }
                Ok(Cmd::Pause(p)) => {
                    link_paused = p;
                    if !p {
                        // A new connection: a fresh keyframe, and every tile painted again on it.
                        key_needed = true;
                        acked = seq;
                        if let Some(g) = &mut grid {
                            g.forget_painted();
                        }
                        unpaint_all = true;
                    }
                }
                Ok(Cmd::ResumeLoad) => {
                    ctl.resume(now);
                    key_needed = true;
                    emit(&out, json!({"event": "state", "sc": opts.sc, "state": tracker.state().name()}));
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break 'stream "app_closed".to_string(),
            }
        }
        if hk_rx.try_recv().is_ok() {
            break 'stream "hotkey".to_string();
        }
        if !hotkey_told {
            if let Some(ok) = *border.hotkey_ok.lock().unwrap() {
                hotkey_told = true;
                if !ok {
                    emit(&out, json!({"event": "hotkey_taken", "sc": opts.sc, "hotkey": opts.hotkey_text}));
                }
            }
        }
        while let Ok(o) = outcome_rx.try_recv() {
            if o == Outcome::Covered {
                if let Some(s) = tracker.covered() {
                    emit(&out, json!({"event": "state", "sc": opts.sc, "state": s.name()}));
                }
            } else if o == Outcome::Blocked {
                emit(&out, json!({"event": "input_blocked", "sc": opts.sc}));
            }
        }
        // Four times a second: the states, the window's place.
        if last_look.elapsed() >= LOOK_EVERY {
            last_look = Instant::now();
            if let Some(s) = tracker.look(&looker.look(window_handle), window) {
                emit(&out, json!({"event": "state", "sc": opts.sc, "state": s.name()}));
                if s.ends() {
                    break 'stream "closed".to_string();
                }
                if s == State::Live {
                    key_needed = true;
                }
            }
            if let Some((h, _)) = window_handle {
                if let Some(r) = targets_win::window_rect(h) {
                    rect = r;
                    border.place(r);
                }
            }
            if let Ok(mut s) = shared.lock() {
                s.geometry.rect = rect;
                s.geometry.src = src;
                s.policy = input_policy(tracker.state(), window);
            }
        }
        if ctl.keyframes.due(now) {
            key_needed = true;
        }
        let inflight = seq.saturating_sub(acked);
        if let Some(Event::PausedForLoad) = ctl.tick(inflight, now) {
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
                    key_needed = true;
                }
                _ => break 'stream "encoder_failed".to_string(),
            }
        } else if next.kbps != params.kbps {
            encoder.set_bitrate(next.kbps);
        }
        params = next;

        let since_last = last_encode.map(|t| now.saturating_sub(t));
        let may = !link_paused && tracker.state().picture_moves() && ctl.may_encode(inflight, since_last);
        let wait = Duration::from_millis(if may { 30 } else { 50 });
        let grabbed: Option<(ID3D11Texture2D, Option<Vec<Rect>>)> = if !may {
            std::thread::sleep(wait);
            None
        } else {
            match &mut source {
                Source::Monitor(dxgi, tex) => match dxgi.grab(&d, wait.as_millis() as u32, tex) {
                    capture_dxgi::Grab::Frame { dirty } => Some((tex.clone(), dirty)),
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
                                    key_needed = true;
                                }
                                _ => break 'stream "encoder_failed".to_string(),
                            }
                        }
                        Some((t, None))
                    }
                    _ => {
                        if wgc.closed.load(std::sync::atomic::Ordering::SeqCst) {
                            break 'stream "closed".to_string();
                        }
                        None
                    }
                },
            }
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
                match encoder.encode(&d, &nv12, time, key_needed) {
                    Ok(true) => {
                        if let Some(extra) = delay {
                            std::thread::sleep(extra);
                        }
                        if key_needed {
                            ctl.keyframes.made(now);
                        }
                        key_needed = false;
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
                                key_needed = true;
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
                            key_needed = true;
                            emit(&out, json!({"event": "encoder", "sc": opts.sc, "encoder": "sw", "name": encoder.name, "refused": encoder.refused, "why": "slow"}));
                            break;
                        }
                    }
                    let Some(au) = ps.fix(&e.data) else {
                        key_needed = true;
                        continue;
                    };
                    let key = e.key || sps::is_keyframe(&au);
                    seq += 1;
                    let v = Video { key, seq, capture_ms: now as u32, src: (src.0 as u16, src.1 as u16), coded: (params.w as u16, params.h as u16), unpaint: std::mem::take(&mut unpaint), unpaint_all: std::mem::take(&mut unpaint_all) };
                    let msg = wire::video(&v, &au);
                    if msg.len() > wire::MAX_MEDIA {
                        // Too large for one message: the phone could not use what follows it. Less bitrate, and a keyframe.
                        ctl.bandwidth.kbps = (ctl.bandwidth.kbps * 3 / 4).max(crate::control::MIN_KBPS);
                        key_needed = true;
                        stats.dropped_link += 1;
                        seq -= 1;
                        continue;
                    }
                    stats.frame(msg.len(), key, encode_ms);
                    let _ = out.send(wire::out_media(&opts.sc, &msg));
                }
            }
            Err(_) => break 'stream "encoder_failed".to_string(),
        }
        // Paint-over: still regions as crisp tiles, from the last picture read, within a byte budget.
        if let Some(g) = &mut grid {
            let rate = f64::from(params.kbps) * 1000.0 / 8.0;
            tile_tokens = (tile_tokens + rate * now.saturating_sub(tile_at) as f64 / 1000.0).min(TILE_BURST);
            tile_at = now;
            // Off by itself while the link is slow or was just cut (`Controller::crisp_ok`): the video needs the room.
            if seq > 0 && !key_needed && !link_paused && ctl.crisp_ok(seq.saturating_sub(acked), now) && readback.stride > 0 && tile_tokens > 32_768.0 && g.any_due(now) && g.size() == src {
                for r in g.due(now, TILE_CELLS_AT_ONCE) {
                    match tile_image::encode(&readback.buf, readback.stride, r) {
                        Some((format, image)) if (image.len() as f64) <= tile_tokens => {
                            tile_id = tile_id.wrapping_add(1);
                            tile_tokens -= image.len() as f64;
                            let _ = out.send(wire::out_media(&opts.sc, &wire::tile(seq, tile_id, r, format, &image)));
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
        let crisp = opts.tiles.then(|| ctl.crisp_ok(seq.saturating_sub(acked), now));
        if let Some(r) = stats.report(now, &params, ctl.level(), ctl.software(), crisp) {
            let mut v = serde_json::to_value(&r).unwrap_or_default();
            v["event"] = json!("stats");
            v["sc"] = json!(opts.sc);
            emit(&out, v);
        }
    };
    drop(border);
    drop(cmds);
    // The input thread ends once it sees it holds the last reference to what it reads.
    drop(shared);
    let _ = input_thread.join();
    system::streaming(false);
    emit(&out, json!({"event": "ended", "sc": opts.sc, "reason": reason}));
}

/// The phone's input, in the order it came, as long as it is fresh and the state allows it.
fn input_loop(rx: Receiver<InputCmd>, shared: Arc<Mutex<Shared>>, outcomes: Sender<Outcome>, out: SyncSender<Vec<u8>>) {
    let _mmcss = system::Mmcss::capture();
    let mut fronted = false;
    loop {
        let cmd = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(c) => c,
            Err(RecvTimeoutError::Timeout) => {
                if Arc::strong_count(&shared) == 1 {
                    return;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let (g, p) = match shared.lock() {
            Ok(s) => (s.geometry, s.policy),
            Err(_) => return,
        };
        match cmd {
            InputCmd::Ops(ops, t) => {
                if !fresh(t, wall_ms()) {
                    continue;
                }
                let o = input_win::run(&ops, &g, p, &mut fronted);
                if o != Outcome::Done {
                    let _ = outcomes.send(o);
                }
            }
            InputCmd::Text(text, t, id) => {
                let reply = if !fresh(t, wall_ms()) {
                    json!({"id": id, "err": "it came too late"})
                } else {
                    match input_win::type_text(&text, &g, p, &mut fronted) {
                        Outcome::Done => json!({"id": id, "ok": true}),
                        Outcome::Blocked => json!({"id": id, "err": "Windows did not take it: an app run as administrator, or a secure screen"}),
                        _ => json!({"id": id, "err": "typing is not possible in this state"}),
                    }
                };
                emit(&out, reply);
            }
        }
    }
}
