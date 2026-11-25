//! `Host`, which the stdin commands reach, and the Mac side of one stream: the `stream_core::Backend` over ScreenCaptureKit and VideoToolbox,
//! the border with the stop hotkey, and the input thread. The same shape as the Windows side (`win::stream`): a stream thread runs
//! `stream_core::run`, an input thread does the phone's input as it comes from what the stream thread last published, and nothing is shared
//! without the border and the stop hotkey.
//!
//! Places are global points (what Core Graphics and the events use) and pictures are pixels (points × the backing scale).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::border_mac::Border;
use super::capture_sck::{Capture, Grabbed, Source};
use super::encode_vt::Encoder;
use super::ffi::{Cf, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferUnlockBaseAddress, kCVPixelBufferLock_ReadOnly};
use super::input_cg::{self, Geometry, Outcome};
use super::system_mac::{interactive_thread, Activity};
use super::{states_mac, targets_mac};
use crate::control::Params;
use crate::geom::Rect;
use crate::input::{fresh, parse_hotkey, Hotkey, QUEUE};
use crate::states::{input_policy, InputPolicy, Observed, State};
use crate::stream_core::{self, emit, wall_ms, Backend, Cmd, Config, Encoded, EncoderInfo, Grab, InputSignal, Parsed};
use crate::targets::{self, TargetId, MAC_DISPLAY};
use crate::wire::Shape;

/// How long `start` waits for the stream's first look (the app waits a little longer, the daemon longer still).
const START_WAIT: Duration = Duration::from_secs(7);
/// The most a capture delivers; the loop's pacer decides what is encoded.
const CAPTURE_FPS: u32 = 60;
/// How often a display's place and mode are looked at again (a change ends the stream: `display_changed`).
const DISPLAY_EVERY: Duration = Duration::from_secs(2);

enum InputCmd {
    Ops(Vec<Value>, Option<u64>),
    Text(String, Option<u64>, u64),
}

struct Shared {
    geometry: Geometry,
    policy: InputPolicy,
}

/// The input thread, stopped and joined when this goes (also on a panic).
struct InputGuard {
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for InputGuard {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Running {
    sc: String,
    cmds: Sender<Cmd>,
    input: SyncSender<InputCmd>,
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
        json!({"targets": targets::targets(&targets_mac::monitors(), &targets_mac::windows(), std::process::id())})
    }

    /// Starts a stream (ending the one before, if any): its kind, title and size once its first look is done.
    pub fn start(&mut self, req: &Value) -> Result<Value, String> {
        self.stop("replaced");
        let sc = req["sc"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).ok_or("missing sc")?.to_string();
        let target = req["target"].as_str().and_then(TargetId::parse).ok_or("not a target this computer has")?;
        let device: String = req["device"].as_str().unwrap_or("a phone").chars().filter(|c| !c.is_control()).take(40).collect();
        let hotkey_text = req["hotkey"].as_str().unwrap_or(crate::input::DEFAULT_HOTKEY).to_string();
        let hotkey = parse_hotkey(&hotkey_text).ok_or("the stop hotkey in Settings → Mobile is not one that can be used")?;
        let opts = Opts { sc: sc.clone(), target, device, hotkey, hotkey_text, level: req["level"].as_u64().unwrap_or(0).min(3) as u8, tiles: req["tiles"].as_bool() != Some(false), max_kbps: req["max_kbps"].as_u64().map(|k| k.clamp(u64::from(crate::control::MIN_KBPS), u64::from(crate::control::MAX_KBPS)) as u32) };
        let (cmds, cmd_rx) = channel();
        let (input, input_rx) = sync_channel(QUEUE);
        let (started_tx, started_rx) = channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let out = self.out.clone();
        let flag = stopping.clone();
        let (told, told_sc) = (self.out.clone(), sc.clone());
        let thread = std::thread::spawn(move || {
            crate::guarded(|| run(opts, cmd_rx, input_rx, out, started_tx, flag), || emit(&told, json!({"event": "ended", "sc": told_sc, "reason": "failed"})));
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
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(t) = running.thread.take() {
                    let _ = t.join();
                }
                Err("the screen failed to start".into())
            }
            Err(RecvTimeoutError::Timeout) => {
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
        match cmd {
            "input" => {
                let ops = crate::input::checked_ops(&req["ops"])?;
                let _ = r.input.try_send(InputCmd::Ops(ops, t));
                None
            }
            "text" => {
                let Some(text) = req["text"].as_str().and_then(crate::input::checked_text) else { return Some(Err("not text that can be typed".into())) };
                match r.input.try_send(InputCmd::Text(text.to_string(), t, req["id"].as_u64().unwrap_or(0))) {
                    Ok(()) => None,
                    Err(_) => Some(Err("too much input waiting".into())),
                }
            }
            _ => match Cmd::parse(cmd, req) {
                Parsed::Cmd(c) => {
                    let _ = r.cmds.send(c);
                    None
                }
                Parsed::Nothing => None,
                Parsed::Unknown => Some(Err(format!("unknown command {cmd:?}"))),
            },
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
    max_kbps: Option<u32>,
}

/// Even sides (the encoder's NV12 needs them), at least 2.
fn even(w: f64, h: f64) -> (u32, u32) {
    ((w.round().max(2.0) as u32) & !1, (h.round().max(2.0) as u32) & !1)
}

/// What to capture, where it is (points), its picture's size (pixels), the backing scale, its title and, for a window, its app.
struct Found {
    source: Source,
    window: Option<(u32, i32)>,
    display: Option<targets_mac::Display>,
    rect: Rect,
    src: (u32, u32),
    scale: f64,
    title: String,
    exe: String,
}

fn find(target: &TargetId) -> Result<Found, String> {
    match target {
        TargetId::Monitor(name) => {
            let id: u32 = name.strip_prefix(MAC_DISPLAY).and_then(|n| n.parse().ok()).ok_or("that screen is no longer connected")?;
            let displays = targets_mac::displays();
            let index = displays.iter().position(|d| d.id == id).ok_or("that screen is no longer connected")?;
            let d = displays[index];
            Ok(Found { source: Source::Display(id), window: None, display: Some(d), rect: d.bounds, src: even(f64::from(d.pixels.0), f64::from(d.pixels.1)), scale: d.scale, title: format!("Screen {}", index + 1), exe: String::new() })
        }
        TargetId::Window { hwnd, pid } => {
            let number = u32::try_from(*hwnd).map_err(|_| "that window was closed")?;
            let info = targets_mac::info(number).filter(|i| i.pid == *pid).ok_or("that window was closed")?;
            if !targets::listed(&info, std::process::id()) {
                return Err("that window cannot be shared".into());
            }
            let rect = info.rect;
            let scale = targets_mac::scale_at(rect);
            Ok(Found {
                source: Source::Window(number),
                window: Some((number, *pid as i32)),
                display: None,
                rect,
                src: even(f64::from(rect.w) * scale, f64::from(rect.h) * scale),
                scale,
                title: info.title.chars().take(120).collect(),
                exe: info.exe,
            })
        }
    }
}

/// The machine of one stream. Its fields drop in this order: the input thread first, then the border and its hotkey, the encoder, and the
/// capture last.
struct MacBackend {
    input_guard: Option<InputGuard>,
    border: Option<Border>,
    shared: Option<Arc<Mutex<Shared>>>,
    hk_rx: Receiver<()>,
    outcome_rx: Receiver<Outcome>,
    stopping: Arc<AtomicBool>,
    activity: Option<Activity>,
    /// The last readback (BGRA, `src` wide).
    buf: Vec<u8>,
    stride: usize,
    window: Option<(u32, i32)>,
    display: Option<targets_mac::Display>,
    display_at: Instant,
    scale: f64,
    /// The size the capture was last asked for (a window's follows it).
    capture_size: (u32, u32),
    encoder: Encoder,
    capture: Capture,
}

impl Backend for MacBackend {
    type Picture = Cf;
    type Input = Cf;

    fn hotkey_pressed(&mut self) -> bool {
        self.hk_rx.try_recv().is_ok()
    }

    fn input_signal(&mut self) -> Option<InputSignal> {
        while let Ok(o) = self.outcome_rx.try_recv() {
            match o {
                Outcome::Covered => return Some(InputSignal::Covered),
                Outcome::ForeignDialog => return Some(InputSignal::ForeignDialog),
                Outcome::NotFront => return Some(InputSignal::NotFront),
                Outcome::Blocked => return Some(InputSignal::Blocked),
                _ => {}
            }
        }
        None
    }

    fn healthy(&self) -> bool {
        true
    }

    fn look(&mut self) -> Observed {
        states_mac::look(self.window)
    }

    fn window_rect(&mut self) -> Option<Rect> {
        let (number, pid) = self.window?;
        let w = targets_mac::window(number).filter(|w| w.pid == pid && w.onscreen)?;
        let r = w.bounds.rect();
        if let Some(border) = &self.border {
            border.place(r);
        }
        let size = even(f64::from(r.w) * self.scale, f64::from(r.h) * self.scale);
        if size != self.capture_size {
            self.capture_size = size;
            self.capture.resize(size);
        }
        Some(r)
    }

    fn publish(&mut self, rect: Rect, src: (u32, u32), policy: InputPolicy, _refresh_desktop: bool) {
        if let Some(shared) = &self.shared {
            if let Ok(mut s) = shared.lock() {
                s.geometry.rect = rect;
                s.geometry.src = src;
                s.policy = policy;
            }
        }
    }

    fn grab(&mut self, wait_ms: u64) -> Grab<Cf> {
        if let Some(d) = self.display {
            if self.display_at.elapsed() >= DISPLAY_EVERY {
                self.display_at = Instant::now();
                match targets_mac::display(d.id) {
                    Some(now) if now.bounds == d.bounds && now.pixels == d.pixels => {}
                    _ => return Grab::End("display_changed"),
                }
            }
        }
        match self.capture.grab(Duration::from_millis(wait_ms)) {
            Grabbed::Frame(f) => {
                let p = f.picture.ptr() as *mut std::ffi::c_void;
                let size = unsafe { ((CVPixelBufferGetWidth(p) as u32) & !1, (CVPixelBufferGetHeight(p) as u32) & !1) };
                Grab::Frame { picture: f.picture, size, dirty: f.dirty, first_full: None }
            }
            Grabbed::Idle => Grab::Idle,
            Grabbed::Stopped => Grab::End(if self.window.is_some() { "closed" } else { "display_changed" }),
        }
    }

    fn resize(&mut self, src: (u32, u32), params: &Params, hardware: bool) -> Result<(), String> {
        self.encoder = Encoder::open(params.w, params.h, u32::from(params.fps), params.kbps, hardware)?;
        let _ = src;
        Ok(())
    }

    fn software_encoder(&mut self, params: &Params) -> Result<(), String> {
        self.encoder = Encoder::open(params.w, params.h, u32::from(params.fps), params.kbps, false)?;
        Ok(())
    }

    fn encoder_info(&self) -> EncoderInfo {
        EncoderInfo { hardware: self.encoder.hardware, name: self.encoder.name.clone(), refused: self.encoder.refused.clone(), first_output_ms: self.encoder.first_output_ms(), sequence_header: None }
    }

    fn hardware(&self) -> bool {
        self.encoder.hardware
    }

    fn held(&self) -> u64 {
        self.encoder.held()
    }

    fn first_output_ms(&self) -> Option<f64> {
        self.encoder.first_output_ms()
    }

    fn asks_for_input(&self) -> bool {
        false
    }

    fn set_bitrate(&mut self, kbps: u32) {
        self.encoder.set_bitrate(kbps);
    }

    fn convert(&mut self, picture: &Cf) -> Result<Cf, String> {
        self.encoder.convert(picture)
    }

    fn encode(&mut self, input: &Cf, time: i64, key: bool) -> Result<bool, String> {
        self.encoder.encode(input, time, key)
    }

    fn poll(&mut self) -> Result<Vec<Encoded>, String> {
        Ok(self.encoder.poll())
    }

    fn readback(&mut self, picture: &Cf, src: (u32, u32), dirty: Option<&[Rect]>) -> bool {
        let p = picture.ptr() as *mut std::ffi::c_void;
        let stride = src.0 as usize * 4;
        if self.buf.len() != stride * src.1 as usize {
            self.buf = vec![0; stride * src.1 as usize];
            self.stride = 0;
        }
        unsafe {
            if CVPixelBufferLockBaseAddress(p, kCVPixelBufferLock_ReadOnly) != 0 {
                return false;
            }
            let base = CVPixelBufferGetBaseAddress(p) as *const u8;
            let row = CVPixelBufferGetBytesPerRow(p);
            let (w, h) = ((CVPixelBufferGetWidth(p) as u32).min(src.0), (CVPixelBufferGetHeight(p) as u32).min(src.1));
            if !base.is_null() {
                let all = [Rect::new(0, 0, w as i32, h as i32)];
                // The first readback, or a size that changed, reads everything.
                let rects = if self.stride == 0 { &all[..] } else { dirty.unwrap_or(&all) };
                for r in rects.iter().filter_map(|r| r.intersect(all[0])) {
                    for y in r.y..r.bottom() {
                        let from = base.add(y as usize * row + r.x as usize * 4);
                        let to = y as usize * stride + r.x as usize * 4;
                        std::ptr::copy_nonoverlapping(from, self.buf[to..].as_mut_ptr(), r.w as usize * 4);
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(p, kCVPixelBufferLock_ReadOnly);
            if base.is_null() {
                return false;
            }
        }
        self.stride = stride;
        true
    }

    fn pixels(&self) -> (&[u8], usize) {
        (&self.buf, self.stride)
    }

    fn cursor(&mut self) -> (Shape, bool, i32, i32) {
        targets_mac::cursor()
    }

    fn finish(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        drop(self.border.take());
        drop(self.shared.take());
        drop(self.input_guard.take());
        drop(self.activity.take());
    }
}

fn run(opts: Opts, cmds: Receiver<Cmd>, input_rx: Receiver<InputCmd>, out: SyncSender<Vec<u8>>, started: Sender<Result<Value, String>>, stopping: Arc<AtomicBool>) {
    interactive_thread();
    let found = match find(&opts.target) {
        Ok(f) => f,
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    let window = found.window.is_some();
    let cfg = Config { sc: opts.sc.clone(), window, src: found.src, rect: found.rect, level: opts.level, tiles: opts.tiles, max_kbps: opts.max_kbps, title: found.title.clone(), exe: found.exe.clone() };
    let mut ctl = stream_core::controller(&cfg);
    let mut params = ctl.params();
    let fps = u32::from(params.fps);
    let encoder = match Encoder::open(params.w, params.h, fps, params.kbps, true).or_else(|_| Encoder::open(params.w, params.h, fps, params.kbps, false)) {
        Ok(e) => e,
        Err(e) => {
            let _ = started.send(Err(format!("no H.264 encoder works on this computer: {e}")));
            return;
        }
    };
    // No hardware encoder (VideoToolbox gave its software one): the software ladder, and the encoder opened as software for it.
    let encoder = if encoder.hardware {
        encoder
    } else {
        ctl.set_software(true);
        params = ctl.params();
        drop(encoder);
        match Encoder::open(params.w, params.h, u32::from(params.fps), params.kbps, false) {
            Ok(e) => e,
            Err(e) => {
                let _ = started.send(Err(format!("no H.264 encoder works on this computer: {e}")));
                return;
            }
        }
    };
    // The border and the hotkey first (the capture leaves the border out, which it must see to): without them nothing is shared.
    let (hk_tx, hk_rx) = channel();
    let label = format!("Shared with {} · {} stops it", opts.device, opts.hotkey_text);
    let border = match Border::show(found.rect, &label, opts.hotkey, hk_tx) {
        Ok(b) => b,
        Err(e) => {
            let _ = started.send(Err(e));
            return;
        }
    };
    let capture = match Capture::start(found.source, found.src, CAPTURE_FPS) {
        Ok(c) => c,
        Err(e) => {
            drop(border);
            let _ = started.send(Err(e));
            return;
        }
    };
    let activity = Activity::begin();
    let shared = Arc::new(Mutex::new(Shared { geometry: Geometry { window: found.window, rect: found.rect, src: found.src, scale: found.scale }, policy: input_policy(State::Live, window) }));
    let (outcome_tx, outcome_rx) = channel::<Outcome>();
    let input_guard = {
        let (shared, out, stopping) = (shared.clone(), out.clone(), stopping.clone());
        InputGuard { stopping: stopping.clone(), thread: Some(std::thread::spawn(move || input_loop(input_rx, shared, outcome_tx, out, stopping))) }
    };
    let mut backend = MacBackend {
        input_guard: Some(input_guard),
        border: Some(border),
        shared: Some(shared),
        hk_rx,
        outcome_rx,
        stopping,
        activity,
        buf: Vec::new(),
        stride: 0,
        window: found.window,
        display: found.display,
        display_at: Instant::now(),
        scale: found.scale,
        capture_size: found.src,
        encoder,
        capture,
    };
    stream_core::run(cfg, ctl, &mut backend, cmds, out, started);
}

/// The phone's input, in the order it came, as long as it is fresh, the stream is not stopping and the state allows it. A button a `down` left
/// pressed is released when it ends.
fn input_loop(rx: Receiver<InputCmd>, shared: Arc<Mutex<Shared>>, outcomes: Sender<Outcome>, out: SyncSender<Vec<u8>>, stopping: Arc<AtomicBool>) {
    interactive_thread();
    let mut pressed: Option<input_cg::Held> = None;
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
            if let InputCmd::Text(_, _, id) = cmd {
                emit(&out, json!({"id": id, "err": "the screen was stopped"}));
            }
            continue;
        }
        let (g, p) = match shared.lock() {
            Ok(s) => (s.geometry, s.policy),
            Err(_) => break,
        };
        objc2::rc::autoreleasepool(|_| match cmd {
            InputCmd::Ops(ops, t) => {
                if !fresh(t, wall_ms()) {
                    if ops.iter().any(|op| op["k"] == "up" || op["k"] == "release") {
                        input_cg::release(&mut pressed);
                    }
                    return;
                }
                let o = input_cg::run(&ops, &g, p, &mut pressed);
                if o != Outcome::Done {
                    let _ = outcomes.send(o);
                }
            }
            InputCmd::Text(text, t, id) => {
                let reply = if !fresh(t, wall_ms()) {
                    json!({"id": id, "err": "it came too late"})
                } else {
                    match input_cg::type_text(&text, &g, p) {
                        (Outcome::Done, _) => json!({"id": id, "ok": true}),
                        (Outcome::NotFront, 0) => {
                            let _ = outcomes.send(Outcome::NotFront);
                            json!({"id": id, "err": "not_front: the shared window is not in front on the computer, so nothing was typed"})
                        }
                        (Outcome::NotFront | Outcome::ForeignDialog, typed) => {
                            let _ = outcomes.send(Outcome::NotFront);
                            json!({"id": id, "err": format!("not_front_partial: another window took the focus after {typed} characters; the rest was not typed")})
                        }
                        (Outcome::Blocked, _) => json!({"id": id, "err": "no_accessibility: The Mac has not allowed Kelpie to control the computer. Allow it in System Settings → Privacy & Security → Accessibility, on the Mac"}),
                        _ => json!({"id": id, "err": "typing is not possible in this state"}),
                    }
                };
                emit(&out, reply);
            }
        });
    }
    input_cg::release(&mut pressed);
}
