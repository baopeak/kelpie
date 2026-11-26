//! `Host`, which the stdin commands reach, and the Windows side of one stream: the `stream_core::Backend` over D3D11, Desktop Duplication,
//! Windows Graphics Capture and Media Foundation, the border with the stop hotkey, and the input thread.
//!
//! A stream is two threads. The **stream thread** owns the device, the capture, the converter and the encoder and runs `stream_core::run`
//! (what decides anything, in the order it is decided: see there). The **input thread** does the phone's input as it comes, so that encoding
//! never delays a click; it reads the target's place and what the state allows from what the stream thread last wrote (`Backend::publish`).
//! Sharing does not start without the border and the stop hotkey.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::{json, Value};
use windows::Win32::Foundation::HWND;
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
use crate::control::Params;
use crate::geom::{self, Rect};
use crate::input::{fresh, parse_hotkey, Hotkey, QUEUE};
use crate::states::{input_policy, InputPolicy, Observed, State};
use crate::stream_core::{self, emit, Backend, Cmd, Config, Encoded, EncoderInfo, Grab, InputSignal, Parsed};
use crate::targets::{self, TargetId};
use crate::wire::Shape;

/// How long `start` waits for the stream's first look (the app waits a little longer, the daemon longer still).
const START_WAIT: Duration = Duration::from_secs(7);

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

/// The input thread, stopped and joined when this goes: on the stream's normal end, and on a panic, which would otherwise leave it doing what
/// is queued with no border, no hotkey and no stream.
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
        let opts = Opts { sc: sc.clone(), target, device, hotkey, hotkey_text, level: req["level"].as_u64().unwrap_or(0).min(3) as u8, tiles: req["tiles"].as_bool() != Some(false), max_kbps: req["max_kbps"].as_u64().map(|k| k.clamp(u64::from(crate::control::MIN_KBPS), u64::from(crate::control::MAX_KBPS)) as u32) };
        let (cmds, cmd_rx) = channel();
        let (input, input_rx) = sync_channel(QUEUE);
        let (started_tx, started_rx) = channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let out = self.out.clone();
        let flag = stopping.clone();
        let (told, told_sc) = (self.out.clone(), sc.clone());
        let thread = std::thread::spawn(move || {
            // A stream that fails on its way is said to have ended, never left frozen without a word (its border, its input thread and its
            // hotkey go with it: `InputGuard`, the border's drop).
            crate::guarded(
                || run(opts, cmd_rx, input_rx, out, started_tx, flag),
                || {
                    system::streaming(false);
                    emit(&told, json!({"event": "ended", "sc": told_sc, "reason": "failed"}));
                },
            );
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
            // The stream failed before it was ready (it panicked: its answer will never come).
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(t) = running.thread.take() {
                    let _ = t.join();
                }
                Err("the screen failed to start".into())
            }
            Err(RecvTimeoutError::Timeout) => {
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
        match cmd {
            "input" => {
                let ops = crate::input::checked_ops(&req["ops"])?;
                // A full queue drops the newest: the phone's next input finds room.
                let _ = r.input.try_send(InputCmd::Ops(ops, t));
                None
            }
            "text" => {
                let Some(text) = req["text"].as_str().and_then(crate::input::checked_text) else { return Some(Err("not text that can be typed".into())) };
                Some(match r.input.try_send(InputCmd::Text(text.to_string(), t, req["id"].as_u64().unwrap_or(0))) {
                    // The answer comes from the input thread, once typed.
                    Ok(()) => return None,
                    Err(_) => Err("too much input waiting".into()),
                })
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
    /// The most the stream may send (kbps), for a way that is held to a rate (the relay WebSocket).
    max_kbps: Option<u32>,
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

/// The machine of one stream. Its fields drop in this order: on a panic the input thread is stopped and joined before the border and its
/// hotkey go, and the device goes last.
struct WinBackend {
    input_guard: Option<InputGuard>,
    border: Option<Border>,
    shared: Option<Arc<Mutex<Shared>>>,
    hk_rx: Receiver<()>,
    outcome_rx: Receiver<Outcome>,
    stopping: Arc<AtomicBool>,
    readback: Readback,
    looker: Looker,
    /// The window target and its process.
    window_handle: Option<(HWND, u32)>,
    desktop: Rect,
    /// The picture's size: a monitor's never changes, a window's follows `resize`.
    src: (u32, u32),
    conv: Converter,
    encoder: Encoder,
    source: Source,
    d: Device,
}

impl WinBackend {
    fn device_removed(&self) -> bool {
        unsafe { self.d.device.GetDeviceRemovedReason() }.is_err()
    }
}

impl Backend for WinBackend {
    type Picture = ID3D11Texture2D;
    type Input = ID3D11Texture2D;

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
        !self.device_removed()
    }

    fn look(&mut self) -> Observed {
        self.looker.look(self.window_handle)
    }

    fn window_rect(&mut self) -> Option<Rect> {
        let (h, _) = self.window_handle?;
        let r = targets_win::window_rect(h)?;
        if let Some(border) = &self.border {
            border.place(r);
        }
        Some(r)
    }

    fn publish(&mut self, rect: Rect, src: (u32, u32), policy: InputPolicy, refresh_desktop: bool) {
        if refresh_desktop {
            let monitors = d3d::monitors();
            if !monitors.is_empty() {
                self.desktop = geom::virtual_desktop(&monitors);
            }
        }
        if let Some(shared) = &self.shared {
            if let Ok(mut s) = shared.lock() {
                s.geometry.rect = rect;
                s.geometry.src = src;
                s.geometry.desktop = self.desktop;
                s.policy = policy;
            }
        }
    }

    fn grab(&mut self, wait_ms: u64) -> Grab<ID3D11Texture2D> {
        match &mut self.source {
            Source::Monitor(dxgi, tex) => match dxgi.grab(&self.d, wait_ms as u32, tex) {
                capture_dxgi::Grab::Frame { dirty } => Grab::Frame { picture: tex.clone(), size: self.src, dirty, first_full: dxgi.first_full.take() },
                capture_dxgi::Grab::Changed => Grab::End("display_changed"),
                capture_dxgi::Grab::Unavailable => Grab::Failed,
                _ => Grab::Idle,
            },
            Source::Window(wgc, ..) => match wgc.grab(Duration::from_millis(wait_ms)) {
                (capture_wgc::Grab::Frame { w, h }, Some(t)) => Grab::Frame { picture: t, size: (w, h), dirty: None, first_full: None },
                (capture_wgc::Grab::Failed, _) => Grab::Failed,
                _ => {
                    if wgc.closed.load(Ordering::SeqCst) {
                        Grab::End("closed")
                    } else {
                        Grab::Idle
                    }
                }
            },
        }
    }

    fn resize(&mut self, src: (u32, u32), params: &Params, hardware: bool) -> Result<(), String> {
        let fps = u32::from(params.fps);
        match (Converter::new(&self.d, src, (params.w, params.h), fps), Encoder::open(&self.d, params.w, params.h, fps, params.kbps, hardware)) {
            (Ok(c), Ok(e)) => {
                self.conv = c;
                self.encoder = e;
                self.src = src;
                Ok(())
            }
            (Err(e), _) => Err(e.to_string()),
            (_, Err(e)) => Err(e.to_string()),
        }
    }

    fn software_encoder(&mut self, params: &Params) -> Result<(), String> {
        self.encoder = Encoder::open(&self.d, params.w, params.h, u32::from(params.fps), params.kbps, false).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn encoder_info(&self) -> EncoderInfo {
        EncoderInfo { hardware: self.encoder.hardware, name: self.encoder.name.clone(), refused: self.encoder.refused.clone(), first_output_ms: self.encoder.first_output_ms, sequence_header: self.encoder.sequence_header.clone() }
    }

    fn hardware(&self) -> bool {
        self.encoder.hardware
    }

    fn held(&self) -> u64 {
        self.encoder.held()
    }

    fn first_output_ms(&self) -> Option<f64> {
        self.encoder.first_output_ms
    }

    fn asks_for_input(&self) -> bool {
        self.encoder.asks_for_input()
    }

    fn set_bitrate(&mut self, kbps: u32) {
        self.encoder.set_bitrate(kbps);
    }

    fn convert(&mut self, picture: &ID3D11Texture2D) -> Result<ID3D11Texture2D, String> {
        self.conv.convert(picture).map_err(|e| e.to_string())
    }

    fn encode(&mut self, input: &ID3D11Texture2D, time: i64, key: bool) -> Result<bool, String> {
        self.encoder.encode(&self.d, input, time, key).map_err(|e| e.to_string())
    }

    fn poll(&mut self) -> Result<Vec<Encoded>, String> {
        let done = self.encoder.poll().map_err(|e| e.to_string())?;
        Ok(done.into_iter().map(|e| Encoded { data: e.data, key: e.key, time: e.time, qp: e.qp }).collect())
    }

    fn readback(&mut self, picture: &ID3D11Texture2D, src: (u32, u32), dirty: Option<&[Rect]>) -> bool {
        self.readback.read_regions(&self.d, picture, src.0, src.1, dirty).is_ok()
    }

    fn pixels(&self) -> (&[u8], usize) {
        (&self.readback.buf, self.readback.stride)
    }

    fn cursor(&mut self) -> (Shape, bool, i32, i32) {
        targets_win::cursor()
    }

    fn finish(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        drop(self.border.take());
        // The input thread ends once it sees it holds the last reference to what it reads.
        drop(self.shared.take());
        drop(self.input_guard.take());
        system::streaming(false);
    }
}

fn run(opts: Opts, cmds: Receiver<Cmd>, input_rx: Receiver<InputCmd>, out: SyncSender<Vec<u8>>, started: Sender<Result<Value, String>>, stopping: Arc<AtomicBool>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let _ = encode_mf::startup();
    let _mmcss = system::Mmcss::capture();
    let (d, source, rect, title, exe) = match open_source(&opts) {
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
    let src = (rect.w.max(2) as u32 & !1, rect.h.max(2) as u32 & !1);
    let cfg = Config { sc: opts.sc.clone(), window, src, rect, level: opts.level, tiles: opts.tiles, max_kbps: opts.max_kbps, title, exe };
    let mut ctl = stream_core::controller(&cfg);
    let mut params = ctl.params();
    let encoder = match Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, true).or_else(|_| Encoder::open(&d, params.w, params.h, u32::from(params.fps), params.kbps, false)) {
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
    let conv = match Converter::new(&d, src, (params.w, params.h), u32::from(params.fps)) {
        Ok(c) => c,
        Err(e) => {
            let _ = started.send(Err(format!("the picture cannot be converted for the encoder: {e}")));
            return;
        }
    };
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
    let desktop = geom::virtual_desktop(&d3d::monitors());
    let shared = Arc::new(Mutex::new(Shared { geometry: Geometry { window: window_handle.map(|(h, _)| h), rect, src, desktop }, policy: input_policy(State::Live, window) }));
    let (outcome_tx, outcome_rx) = channel::<Outcome>();
    let input_guard = {
        let (shared, out, stopping) = (shared.clone(), out.clone(), stopping.clone());
        InputGuard { stopping: stopping.clone(), thread: Some(std::thread::spawn(move || input_loop(input_rx, shared, outcome_tx, out, stopping))) }
    };
    let mut backend = WinBackend { input_guard: Some(input_guard), border: Some(border), shared: Some(shared), hk_rx, outcome_rx, stopping, readback: Readback::new(), looker: Looker::new(), window_handle, desktop, src, conv, encoder, source, d };
    stream_core::run(cfg, ctl, &mut backend, cmds, out, started);
}

/// The phone's input, in the order it came, as long as it is fresh, the stream is not stopping and the state allows it. A button a `down` left
/// pressed is released when it ends.
fn input_loop(rx: Receiver<InputCmd>, shared: Arc<Mutex<Shared>>, outcomes: Sender<Outcome>, out: SyncSender<Vec<u8>>, stopping: Arc<AtomicBool>) {
    let _mmcss = system::Mmcss::capture();
    let mut pressed: Option<input_win::Held> = None;
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
                    if ops.iter().any(|op| op["k"] == "up" || op["k"] == "release") {
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
                        (Outcome::Done, _) => json!({"id": id, "ok": true}),
                        (Outcome::NotFront, 0) => {
                            let _ = outcomes.send(Outcome::NotFront);
                            json!({"id": id, "err": "not_front: the shared window is not in front on the computer, so nothing was typed"})
                        }
                        (Outcome::ForeignDialog, 0) => {
                            let _ = outcomes.send(Outcome::ForeignDialog);
                            json!({"id": id, "err": "covered_dialog: a dialog of another program is in front of the shared window, so nothing was typed"})
                        }
                        // The focus moved partway: what came before went to the shared window, the rest was not typed.
                        (Outcome::NotFront | Outcome::ForeignDialog, typed) => {
                            let _ = outcomes.send(Outcome::NotFront);
                            json!({"id": id, "err": format!("not_front_partial: another window took the focus after {typed} characters; the rest was not typed")})
                        }
                        (Outcome::Blocked, _) => json!({"id": id, "err": "Windows did not take it: an app run as administrator, or a secure screen"}),
                        _ => json!({"id": id, "err": "typing is not possible in this state"}),
                    }
                };
                emit(&out, reply);
            }
        }
    }
    input_win::release(&mut pressed);
}
