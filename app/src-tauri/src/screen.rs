//! A phone sees this computer's screen: the daemon's screen broker (`kelpied`'s `screen`) asks this app, which alone may (`provider.register
//! {kind: "screen"}` from the verified app), and the app runs the helper that can, `kelpie-screen` (crates/kelpie-screen).
//!
//! - **The helper** starts with the first viewer and stops 30 s after the last one (stdin closed, then killed on drop). It speaks first:
//!   `hello {proto}`, and one that speaks another protocol is stopped and reported. Commands go to its stdin as JSON lines, in order, through
//!   one writer; its stdout is frames (`kelpie_screen::wire`): replies, events, and media for the phone.
//! - **The daemon's requests** (`screen.targets`, `screen.start`, `screen.text`) are answered from the helper's replies; its notices
//!   (`screen.input`, `screen.flow`, `screen.keyframe`, `screen.pause`, `screen.resume_load`, `screen.stop`) go straight on.
//! - **What the helper says** goes back as `provider.event {sc, kind, …}`: media (base64 on this local pipe; binary from the daemon to the
//!   phone), its state, its stats, the encoder it uses, and its end (the stop hotkey, the window closed).
//! - **The window** hears `screen-started` and `screen-ended` for the status bar's indicator, whose Stop is `screen_stop`.
//!
//! - **Budgets:** the hello waits `HELLO_WAIT` and a start `START_WAIT`, together under the daemon's 15 s, so the daemon never gives up on a start
//!   that then goes on here; a `screen.stop` for a start still under way is remembered, and that start is stopped as soon as it is done.
//! - **A stop always lands:** one that cannot be queued drops the helper (stdin closed: it stops everything).
//! - **Which helper:** the one next to this executable, in the app's own install folder, as `kelpie-stt`; `KELPIE_SCREEN_PATH` is for debug builds
//!   only (the e2e). An app run as administrator does not start it: the helper would inherit that, and drive elevated windows.
//!
//! Nothing the phone types is logged here or anywhere: it is in the helper's stdin and nowhere else.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use kelpie_screen::wire::{Deframer, Out, PROTO};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

use crate::daemon_client::DaemonClient;
use crate::fs_ops::CmdResult;

/// The helper stops this long after its last viewer.
const IDLE_EXIT: Duration = Duration::from_secs(30);
/// Commands waiting for the helper's stdin, at most.
const STDIN_QUEUE: usize = 1024;
const REQUEST_LIMIT: Duration = Duration::from_secs(10);
/// How long the helper may take to say hello, and a start to answer: together under the daemon's `START_LIMIT` (15 s).
const HELLO_WAIT: Duration = Duration::from_secs(4);
const START_WAIT: Duration = Duration::from_secs(9);
/// Starts the daemon gave up on, remembered so that one that finishes after is stopped at once.
const CANCELLED: usize = 32;

type Waiting = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

struct Helper {
    stdin: mpsc::Sender<String>,
    waiting: Waiting,
    next: AtomicU64,
    hello: Mutex<Option<oneshot::Receiver<Result<(), String>>>>,
}

#[derive(Clone)]
struct Viewer {
    device: String,
    title: String,
    /// The number of the newest pause or resume the daemon sent: one that arrives after a newer one is ignored.
    pause_v: u64,
}

/// Whether a numbered notice `v` is newer than the last one taken (`last`, moved on if so). A notice without a number is always taken.
fn newer(last: &mut u64, v: Option<u64>) -> bool {
    match v {
        Some(v) if v <= *last => false,
        Some(v) => {
            *last = v;
            true
        }
        None => true,
    }
}

/// What a person set in Settings → Mobile.
#[derive(Clone)]
struct Prefs {
    hotkey: String,
    tiles: bool,
}

pub struct Screens {
    helper: Mutex<Option<Arc<Helper>>>,
    viewers: Mutex<HashMap<String, Viewer>>,
    last_viewer: Mutex<Instant>,
    prefs: Mutex<Prefs>,
    cancelled: Mutex<VecDeque<String>>,
}

impl Default for Screens {
    fn default() -> Self {
        Screens { helper: Mutex::new(None), viewers: Mutex::new(HashMap::new()), last_viewer: Mutex::new(Instant::now()), prefs: Mutex::new(Prefs { hotkey: kelpie_screen::input::DEFAULT_HOTKEY.into(), tiles: true }), cancelled: Mutex::new(VecDeque::new()) }
    }
}

/// Next to this executable (the install folder, as for `kelpie-stt`); in a debug build `KELPIE_SCREEN_PATH` first, and target/release too. A
/// release build never takes another path: the helper receives everything a phone types.
fn helper_path() -> Option<PathBuf> {
    let name = if cfg!(windows) { "kelpie-screen.exe" } else { "kelpie-screen" };
    if cfg!(debug_assertions) {
        if let Some(p) = std::env::var_os("KELPIE_SCREEN_PATH") {
            return Some(PathBuf::from(p));
        }
    }
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    let beside = dir.join(name);
    if beside.is_file() {
        return Some(beside);
    }
    let dev = dir.parent()?.join("release").join(name);
    (cfg!(debug_assertions) && dev.is_file()).then_some(dev)
}

fn client(app: &AppHandle) -> Option<Arc<DaemonClient>> {
    app.try_state::<Arc<DaemonClient>>().map(|c| c.inner().clone())
}

fn event(app: &AppHandle, params: Value) {
    if let Some(c) = client(app) {
        let _ = c.notify("provider.event", params);
    }
}

fn reply(app: &AppHandle, preq: u64, result: Result<Value, String>) {
    if let Some(c) = client(app) {
        let _ = c.provider_reply(preq, result);
    }
}

fn spawn(app: &AppHandle) -> Result<Arc<Helper>, String> {
    if elevated() {
        return Err("Kelpie is running as administrator, so it does not share the screen: the helper would be able to drive elevated windows. Start Kelpie normally".into());
    }
    let exe = helper_path().filter(|p| p.is_file()).ok_or("the screen helper (kelpie-screen) is not installed with this build")?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| format!("start the screen helper: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let (tx, mut rx) = mpsc::channel::<String>(STDIN_QUEUE);
    let (hello_tx, hello_rx) = oneshot::channel();
    let helper = Arc::new(Helper { stdin: tx, waiting: Arc::default(), next: AtomicU64::new(1), hello: Mutex::new(Some(hello_rx)) });
    // One writer: commands reach the helper in the order they were given. Its end closes stdin, which ends the helper.
    tauri::async_runtime::spawn(async move {
        while let Some(line) = rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err() || stdin.write_all(b"\n").await.is_err() {
                break;
            }
            let _ = stdin.flush().await;
        }
    });
    let (app2, me) = (app.clone(), Arc::downgrade(&helper));
    tauri::async_runtime::spawn(async move {
        let mut frames = Deframer::default();
        let mut buf = vec![0u8; 256 * 1024];
        let mut hello = Some(hello_tx);
        'read: loop {
            let n = match stdout.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            frames.push(&buf[..n]);
            while let Some(out) = frames.take() {
                if let Some(tx) = hello.take() {
                    let ok = matches!(&out, Out::Json(v) if v["event"] == "hello" && v["proto"].as_u64() == Some(u64::from(PROTO)));
                    let _ = tx.send(if ok { Ok(()) } else { Err("the screen helper does not match this version of Kelpie: reinstall Kelpie".into()) });
                    if !ok {
                        break 'read;
                    }
                    continue;
                }
                let Some(h) = me.upgrade() else { break 'read };
                on_output(&app2, &h, out);
            }
            if frames.broken() {
                break;
            }
        }
        // It ended (or crashed): what waited fails, the viewers' screens end, a new one starts next time.
        if let Some(h) = me.upgrade() {
            for (_, tx) in h.waiting.lock().drain() {
                let _ = tx.send(Err("the screen helper stopped".into()));
            }
            let state = app2.state::<Screens>();
            let mut slot = state.helper.lock();
            if slot.as_ref().is_some_and(|x| Arc::ptr_eq(x, &h)) {
                *slot = None;
            }
        }
        let gone: Vec<String> = app2.state::<Screens>().viewers.lock().drain().map(|(sc, _)| sc).collect();
        for sc in gone {
            event(&app2, json!({"sc": sc, "kind": "ended", "reason": "app_closed"}));
            let _ = app2.emit_to("main", "screen-ended", json!({"sc": sc, "reason": "app_closed"}));
        }
        drop(child);
    });
    Ok(helper)
}

/// The helper, started if it is not running, once it said hello.
async fn helper(app: &AppHandle) -> Result<Arc<Helper>, String> {
    let state = app.state::<Screens>();
    let h = {
        let mut slot = state.helper.lock();
        match slot.as_ref() {
            Some(h) => h.clone(),
            None => {
                let h = spawn(app)?;
                *slot = Some(h.clone());
                // The idle time counts from now, not from when the app started.
                *state.last_viewer.lock() = Instant::now();
                watch_idle(app);
                h
            }
        }
    };
    let hello = h.hello.lock().take();
    if let Some(rx) = hello {
        match tokio::time::timeout(HELLO_WAIT, rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(e))) => return Err(e),
            _ => return Err("the screen helper did not start".into()),
        }
    }
    Ok(h)
}

/// Stops the helper 30 s after its last viewer.
fn watch_idle(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let state = app.state::<Screens>();
            if state.helper.lock().is_none() {
                return;
            }
            if state.viewers.lock().is_empty() && state.last_viewer.lock().elapsed() >= IDLE_EXIT && state.helper.lock().as_ref().is_some_and(|h| h.waiting.lock().is_empty()) {
                // Dropping the last handle closes stdin: the helper ends.
                *state.helper.lock() = None;
                return;
            }
        }
    });
}

fn send(h: &Helper, cmd: Value) {
    let _ = h.stdin.try_send(cmd.to_string());
}

/// A stop that must land: if it cannot be queued, the helper is dropped (its stdin closes, and it stops everything).
fn send_stop(app: &AppHandle, sc: &str, reason: &str) {
    let state = app.state::<Screens>();
    let helper = state.helper.lock().clone();
    if let Some(h) = helper {
        if h.stdin.try_send(json!({"cmd": "stop", "sc": sc, "reason": reason}).to_string()).is_err() {
            *state.helper.lock() = None;
        }
    }
}

#[cfg(windows)]
fn elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut e = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let ok = GetTokenInformation(token, TokenElevation, Some(&mut e as *mut _ as *mut _), std::mem::size_of::<TOKEN_ELEVATION>() as u32, &mut len).is_ok();
        let _ = CloseHandle(token);
        ok && e.TokenIsElevated != 0
    }
}

#[cfg(not(windows))]
fn elevated() -> bool {
    false
}

async fn ask(h: &Helper, cmd: Value) -> Result<Value, String> {
    ask_within(h, cmd, REQUEST_LIMIT).await
}

async fn ask_within(h: &Helper, mut cmd: Value, limit: Duration) -> Result<Value, String> {
    let id = h.next.fetch_add(1, Ordering::Relaxed);
    cmd["id"] = json!(id);
    let (tx, rx) = oneshot::channel();
    h.waiting.lock().insert(id, tx);
    if h.stdin.try_send(cmd.to_string()).is_err() {
        h.waiting.lock().remove(&id);
        return Err("the screen helper is not taking commands".into());
    }
    match tokio::time::timeout(limit, rx).await {
        Ok(Ok(r)) => r,
        _ => {
            h.waiting.lock().remove(&id);
            Err("the screen helper did not answer in time".into())
        }
    }
}

/// What the helper said: a reply, an event, or media for a phone.
fn on_output(app: &AppHandle, h: &Helper, out: Out) {
    match out {
        Out::Media { sc, media } => {
            if app.state::<Screens>().viewers.lock().contains_key(&sc) {
                event(app, json!({"sc": sc, "kind": "media", "data": base64::engine::general_purpose::STANDARD.encode(&media)}));
            }
        }
        Out::Json(v) => {
            if let Some(id) = v["id"].as_u64() {
                if let Some(tx) = h.waiting.lock().remove(&id) {
                    let _ = tx.send(match v.get("err") {
                        Some(e) => Err(e.as_str().unwrap_or("the screen helper refused").chars().take(300).collect()),
                        None => Ok(v["ok"].clone()),
                    });
                }
                return;
            }
            let sc = v["sc"].as_str().unwrap_or_default().to_string();
            match v["event"].as_str() {
                Some("state") => event(app, json!({"sc": sc, "kind": "state", "state": v["state"]})),
                Some("paused") => event(app, json!({"sc": sc, "kind": "state", "state": "paused_load"})),
                Some("input_blocked") => event(app, json!({"sc": sc, "kind": "state", "state": "blocked"})),
                Some("input_refused") => event(app, json!({"sc": sc, "kind": "state", "state": "not_front"})),
                Some("stats") => event(app, json!({"sc": sc, "kind": "stats", "stats": v})),
                Some("encoder") => {
                    event(app, json!({"sc": sc, "kind": "encoder", "encoder": v["encoder"], "name": v["name"]}));
                    let _ = app.emit_to("main", "screen-note", json!({"sc": sc, "note": "encoder", "encoder": v["encoder"], "name": v["name"]}));
                }
                Some("hotkey_taken") => {
                    let _ = app.emit_to("main", "screen-note", json!({"sc": sc, "note": "hotkey_taken", "hotkey": v["hotkey"]}));
                }
                Some("ended") => {
                    let reason = v["reason"].as_str().unwrap_or("app").to_string();
                    ended(app, &sc, &reason, true);
                }
                _ => {}
            }
        }
    }
}

/// A stream ended here: the window takes its indicator down, and the daemon hears why (unless it asked for the end itself).
fn ended(app: &AppHandle, sc: &str, reason: &str, tell_daemon: bool) {
    let state = app.state::<Screens>();
    if state.viewers.lock().remove(sc).is_none() {
        return;
    }
    *state.last_viewer.lock() = Instant::now();
    if tell_daemon {
        event(app, json!({"sc": sc, "kind": "ended", "reason": reason}));
    }
    let _ = app.emit_to("main", "screen-ended", json!({"sc": sc, "reason": reason}));
}

/// A request or a notice of the daemon's whose method starts with `screen.` (`daemon_client`'s read loop).
pub fn provider_message(app: &AppHandle, msg: Value) {
    let params = msg["params"].clone();
    if let Some(preq) = msg["preq"].as_u64() {
        let method = msg["method"].as_str().unwrap_or_default().to_string();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let result = request(&app, &method, params).await;
            reply(&app, preq, result);
        });
        return;
    }
    let note = msg["pnote"].as_str().unwrap_or_default();
    let Some(h) = app.state::<Screens>().helper.lock().clone() else { return };
    let sc = params["sc"].clone();
    match note {
        "screen.input" => send(&h, json!({"cmd": "input", "sc": sc, "ops": params["ops"], "t": params["t"]})),
        "screen.flow" => send(&h, json!({"cmd": "flow", "sc": sc, "acked": params["acked"], "dropped_through": params["dropped_through"], "rtt_ms": params["rtt_ms"], "level": params["level"], "lost_tiles": params["lost_tiles"], "raw": params["raw"], "wire": params["wire"]})),
        "screen.keyframe" => send(&h, json!({"cmd": "keyframe", "sc": sc})),
        "screen.pause" => {
            let take = sc.as_str().is_some_and(|sc| app.state::<Screens>().viewers.lock().get_mut(sc).is_none_or(|v| newer(&mut v.pause_v, params["v"].as_u64())));
            if take {
                send(&h, json!({"cmd": "pause", "sc": sc, "paused": params["paused"]}));
            }
        }
        "screen.resume_load" => send(&h, json!({"cmd": "resume_load", "sc": sc})),
        "screen.stop" => {
            let reason = params["reason"].as_str().unwrap_or("phone").to_string();
            if let Some(sc) = sc.as_str() {
                let state = app.state::<Screens>();
                // A stop for a start still under way: that start is stopped as soon as it is done.
                if !state.viewers.lock().contains_key(sc) {
                    let mut c = state.cancelled.lock();
                    if c.len() >= CANCELLED {
                        c.pop_front();
                    }
                    c.push_back(sc.to_string());
                }
                send_stop(app, sc, &reason);
                ended(app, sc, &reason, false);
            }
        }
        _ => {}
    }
}

async fn request(app: &AppHandle, method: &str, params: Value) -> Result<Value, String> {
    // Someone uses the screen: the idle time starts over.
    *app.state::<Screens>().last_viewer.lock() = Instant::now();
    match method {
        "screen.targets" => ask(&*helper(app).await?, json!({"cmd": "targets"})).await,
        "screen.start" => {
            let h = helper(app).await?;
            let prefs = app.state::<Screens>().prefs.lock().clone();
            let sc = params["sc"].as_str().filter(|s| !s.is_empty() && s.len() <= 64).ok_or("bad screen")?.to_string();
            let device: String = params["device"].as_str().unwrap_or("a phone").chars().take(40).collect();
            let started = ask_within(&h, json!({"cmd": "start", "sc": sc, "target": params["target"], "device": device, "level": params["level"], "hotkey": prefs.hotkey, "tiles": prefs.tiles}), START_WAIT).await;
            let started = match started {
                Ok(v) => v,
                Err(e) => {
                    // It may still start in the helper after this gave up: told to stop.
                    send_stop(app, &sc, "failed");
                    return Err(e);
                }
            };
            let cancelled = {
                let screens = app.state::<Screens>();
                let mut c = screens.cancelled.lock();
                let was = c.iter().any(|x| *x == sc);
                c.retain(|x| *x != sc);
                was
            };
            if cancelled {
                send_stop(app, &sc, "failed");
                return Err("the phone's request ended before the screen was ready".into());
            }
            let title = started["title"].as_str().unwrap_or_default().to_string();
            app.state::<Screens>().viewers.lock().insert(sc.clone(), Viewer { device: device.clone(), title: title.clone(), pause_v: 0 });
            let _ = app.emit_to("main", "screen-started", json!({"sc": sc, "device": device, "title": title, "kind": started["kind"], "hotkey": prefs.hotkey}));
            Ok(started)
        }
        "screen.text" => {
            let h = app.state::<Screens>().helper.lock().clone().ok_or("the screen has ended")?;
            ask(&h, json!({"cmd": "text", "sc": params["sc"], "text": params["text"], "t": params["t"]})).await.map(|_| json!(true))
        }
        _ => Err(format!("unknown request {method}")),
    }
}

/// The daemon's connection dropped: it ended every screen with it.
pub fn daemon_gone(app: &AppHandle) {
    let Some(state) = app.try_state::<Screens>() else { return };
    let gone: Vec<String> = state.viewers.lock().keys().cloned().collect();
    for sc in gone {
        send_stop(app, &sc, "app_closed");
        ended(app, &sc, "app_closed", false);
    }
}

/// The person pressed Stop in the status bar.
#[tauri::command]
pub fn screen_stop(app: AppHandle, sc: String) -> CmdResult<()> {
    send_stop(&app, &sc, "desktop");
    ended(&app, &sc, "desktop", true);
    Ok(())
}

/// The stop hotkey and paint-over, as Settings has them (sent at start and whenever they change; they apply to the next screen).
#[tauri::command]
pub fn screen_prefs(app: AppHandle, hotkey: String, tiles: bool) -> CmdResult<()> {
    if kelpie_screen::input::parse_hotkey(&hotkey).is_none() {
        return Err("use two modifiers, one of them Ctrl or Alt, and a letter, a digit or F1–F12".into());
    }
    *app.state::<Screens>().prefs.lock() = Prefs { hotkey, tiles };
    Ok(())
}

/// Who sees the screen now, for a window that opens while one does.
#[tauri::command]
pub fn screen_viewers(app: AppHandle) -> Vec<Value> {
    app.state::<Screens>().viewers.lock().iter().map(|(sc, v)| json!({"sc": sc, "device": v.device, "title": v.title})).collect()
}

#[tauri::command]
pub fn screen_available() -> bool {
    cfg!(windows) && helper_path().is_some_and(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn a_pause_older_than_the_last_one_taken_is_ignored() {
        let mut last = 0;
        assert!(newer(&mut last, Some(2)));
        assert!(!newer(&mut last, Some(1)), "a pause that arrives after the resume that followed it");
        assert!(!newer(&mut last, Some(2)));
        assert!(newer(&mut last, Some(3)));
        assert!(newer(&mut last, None), "a notice without a number is taken");
        assert_eq!(last, 3);
    }
}