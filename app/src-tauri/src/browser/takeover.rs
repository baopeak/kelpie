//! A browser pane a phone took over: the daemon's broker (`kelpied`'s `takeover`) asks this window, which alone can reach the page, through the
//! provider channel (`provider.register`, then `preq` requests and `pnote` notices; replies and frames go back as `provider.reply` and `provider.event`).
//!
//! - The pane is streamed with `Page.startScreencast` (JPEG); each `Page.screencastFrame` is acknowledged at once and handed to the daemon, which keeps
//!   only the newest and paces it to the phone. The event receiver is attached for a takeover only: with none held, no DevTools subscription exists.
//! - **One queue per takeover, in order**: the phone's input (a closed set of events, `tap`, `down`, `move`, `up`, `wheel`, `key`, that the daemon checked)
//!   and its text both go through it, so text never overtakes the tap that chose its field. Each item names the window it was meant for (`window_id`
//!   of the frame the phone mapped it against): meant for another (a popup that opened or closed meanwhile) it is dropped. Anything but a release
//!   older than `INPUT_TTL_MS` by the phone's reading of this computer's clock is dropped when its turn comes, and the queue holds at most `QUEUE`
//!   items. A button left pressed is released when the takeover ends or its window changes. None of it is logged.
//! - A pane and the popups it opened (`window.open` with a size: OAuth and sign-in windows) are one group: the popup on top is streamed while it is
//!   open, and closing it goes back to the pane. The window's title says the group is controlled from the phone; the pane's own banner is the app's
//!   (`browser-pane.tsx`).
//!
//! What the window shows (picking the pane, its tab brought to the front, the banner) is the interface's: `browser.panes` and `browser.takeover_start` go to it
//! as a `provider-request` event, and it answers with `provider_reply` after `browser_takeover_start`. A start the daemon has given up on (it said
//! `browser.takeover_stop` first) is refused.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::mpsc;

use super::{valid_label, Browsers, NOT_SHOWN};
use crate::daemon_client::DaemonClient;
use crate::fs_ops::CmdResult;

/// Input older than this when its turn comes is dropped (the daemon checked it once already, on arrival).
const INPUT_TTL_MS: u64 = 1000;
/// Items one takeover's queue holds; more are dropped (text is answered as not sent).
const QUEUE: usize = 64;
/// Takeovers the daemon gave up on, remembered so that a start that comes after is refused.
const CANCELLED: usize = 32;

/// How a frame is made, as the daemon's ladder says.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Settings {
    quality: u8,
    max_width: u16,
    every_nth: u8,
}

impl Settings {
    fn from(v: &Value) -> Settings {
        let n = |k: &str, lo: u64, hi: u64, default: u64| v[k].as_u64().unwrap_or(default).clamp(lo, hi);
        Settings { quality: n("quality", 20, 90, 70) as u8, max_width: n("max_width", 320, 2560, 1280) as u16, every_nth: n("every_nth", 1, 60, 6) as u8 }
    }

    fn screencast(self) -> Value {
        json!({"format": "jpeg", "quality": self.quality, "maxWidth": self.max_width, "maxHeight": u32::from(self.max_width) * 2, "everyNthFrame": self.every_nth})
    }
}

/// What waits in a takeover's queue: input, or text with the request to answer once it is typed.
#[derive(Debug)]
enum Queued {
    Ops { ops: Vec<Value>, window: u64, st: u64 },
    Text { text: String, window: u64, st: u64, preq: u64 },
}

/// Where the mouse button was pressed and not yet released, for each takeover.
type Pressed = Arc<Mutex<Option<(f64, f64)>>>;

struct Held {
    /// The pane's webview, and the one streamed now (the pane, or its popup on top).
    pane: String,
    target: String,
    /// The window's title says who controls it.
    banner: String,
    settings: Settings,
    paused: bool,
    /// The streamed window's id: frames carry it (`window_id`), input names it back, and frames or input of another are dropped.
    generation: u64,
    /// The `Page.screencastFrame` receiver on `target`.
    token: Option<i64>,
    input: mpsc::Sender<Queued>,
    pressed: Pressed,
    /// When the last screencast frame came (ms): a page that sends none (hidden, or a window behind another) is captured instead (`fallback`).
    last_frame: Arc<AtomicU64>,
    /// When the phone last sent input (ms): a still page is captured more often again once it may change.
    last_input: Arc<AtomicU64>,
    /// The number of the last notice of the daemon's applied (`apply_settings`).
    version: u64,
    /// Counts the stream's starts over (a resume, a restart, a new window): the captures forget what they sent before.
    fresh: Arc<AtomicU64>,
}

/// A frame as WebView2 handed it: takeover, generation, the event's JSON.
type FrameSink = mpsc::UnboundedSender<(String, u64, String)>;

#[derive(Default)]
pub struct Takeovers {
    held: Mutex<HashMap<String, Held>>,
    generation: AtomicU64,
    sink: Mutex<Option<FrameSink>>,
    cancelled: Mutex<VecDeque<String>>,
    /// Panes the window hides now (another tab in front): WebView2 keeps input sent to one until it is shown again and then runs it on whatever the
    /// page shows by then, so none is sent there.
    hidden: Mutex<HashSet<String>>,
}

impl Takeovers {
    fn tk_of_pane(&self, pane: &str) -> Option<String> {
        self.held.lock().iter().find(|(_, h)| h.pane == pane).map(|(tk, _)| tk.clone())
    }

    /// The webview streamed for `tk`, while `generation` is its latest.
    fn target(&self, tk: &str, generation: Option<u64>) -> Option<String> {
        self.held.lock().get(tk).filter(|h| generation.is_none_or(|g| g == h.generation)).map(|h| h.target.clone())
    }

    /// The window streamed for `tk` and its id.
    fn window(&self, tk: &str) -> Option<(String, u64)> {
        self.held.lock().get(tk).map(|h| (h.target.clone(), h.generation))
    }

    /// The window shows or hides a pane (`browser_visible`).
    pub(super) fn set_visible(&self, label: &str, visible: bool) {
        let mut hidden = self.hidden.lock();
        if visible {
            hidden.remove(label);
        } else {
            hidden.insert(label.to_string());
        }
    }

    fn hidden(&self, label: &str) -> bool {
        self.hidden.lock().contains(label)
    }

    /// The title a popup of `pane` shows: who controls the group, while someone does.
    pub(super) fn popup_title(&self, pane: &str, title: &str) -> String {
        match self.held.lock().values().find(|h| h.pane == pane) {
            Some(h) => format!("{} — {title}", h.banner),
            None => title.to_string(),
        }
    }

    fn cancel(&self, tk: &str) {
        let mut c = self.cancelled.lock();
        if c.len() >= CANCELLED {
            c.pop_front();
        }
        c.push_back(tk.to_string());
    }
}

fn client(app: &AppHandle) -> Option<Arc<DaemonClient>> {
    app.try_state::<Arc<DaemonClient>>().map(|c| c.inner().clone())
}

fn reply(app: &AppHandle, preq: u64, result: Result<Value, String>) {
    if let Some(c) = client(app) {
        let _ = c.provider_reply(preq, result);
    }
}

fn event(app: &AppHandle, params: Value) {
    if let Some(c) = client(app) {
        let _ = c.notify("provider.event", params);
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Puts `item` in the queue of takeover `tk`. Text that does not fit, or has no takeover to go to, is answered as not sent.
fn enqueue(app: &AppHandle, tk: &str, item: Queued) {
    let sent = match app.try_state::<Takeovers>() {
        Some(state) => match state.held.lock().get(tk) {
            Some(h) => {
                h.last_input.store(now_ms(), Ordering::Relaxed);
                h.input.try_send(item).map_err(|e| e.into_inner())
            }
            None => Err(item),
        },
        None => Err(item),
    };
    if let Err(Queued::Text { preq, .. }) = sent {
        reply(app, preq, Err("not sent".into()));
    }
}

/// A request or a notice from the daemon (`daemon_client`'s read loop).
pub fn on_message(app: &AppHandle, msg: Value) {
    if let Some(preq) = msg["preq"].as_u64() {
        let method = msg["method"].as_str().unwrap_or_default().to_string();
        let params = msg["params"].clone();
        match method.as_str() {
            // The interface knows the panes and shows the takeover: it answers (`provider_reply`).
            "browser.panes" | "browser.takeover_start" => {
                if app.emit_to("main", "provider-request", json!({"preq": preq, "method": method, "params": params})).is_err() {
                    reply(app, preq, Err("the window is not ready".into()));
                }
            }
            // Behind the input that came before it, never ahead.
            "browser.takeover_text" => {
                let item = Queued::Text { text: params["text"].as_str().unwrap_or_default().to_string(), window: params["w"].as_u64().unwrap_or(0), st: params["st"].as_u64().unwrap_or(0), preq };
                enqueue(app, params["tk"].as_str().unwrap_or_default(), item);
            }
            _ => reply(app, preq, Err(format!("unknown request {method}"))),
        }
        return;
    }
    let params = msg["params"].clone();
    let tk = params["tk"].as_str().unwrap_or_default().to_string();
    match msg["pnote"].as_str() {
        Some("browser.takeover_input") => {
            if let Some(ops) = params["ops"].as_array() {
                enqueue(app, &tk, Queued::Ops { ops: ops.clone(), window: params["w"].as_u64().unwrap_or(0), st: params["st"].as_u64().unwrap_or(0) });
            }
        }
        Some("browser.takeover_set") => {
            // Taken in here, in the order the daemon numbered it: the task that follows only makes the stream match what the hold says by then, so two
            // such tasks in either order end the same way.
            if let Some(state) = app.try_state::<Takeovers>() {
                if !apply_settings(&mut state.held.lock(), &tk, &params) {
                    return;
                }
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move { reconcile(&app, &tk).await });
        }
        Some("browser.takeover_stop") => {
            let app = app.clone();
            let reason = params["reason"].as_str().unwrap_or("phone").to_string();
            tauri::async_runtime::spawn(async move { stop(&app, &tk, &reason).await });
        }
        _ => {}
    }
}

/// Starts streaming pane `label` for takeover `tk` (the interface calls it once it showed the pane); its URL's host. `banner` is what the popups' titles
/// say while it lasts, in the window's language.
#[tauri::command]
pub async fn browser_takeover_start(app: AppHandle, tk: String, label: String, banner: String, settings: Value) -> CmdResult<Value> {
    valid_label(&label)?;
    if tk.is_empty() || tk.len() > 64 {
        return Err("bad takeover".into());
    }
    let wv = app.get_webview(&label).ok_or_else(|| format!("no browser {label}"))?;
    let state = app.state::<Takeovers>();
    if state.cancelled.lock().iter().any(|c| *c == tk) {
        return Err("the phone's request ended before the pane was ready".into());
    }
    if state.tk_of_pane(&label).is_some() {
        return Err("that pane is taken over already".into());
    }
    // A popup of the pane already open is what the phone sees first.
    let target = app.state::<Browsers>().popups_of(&label).last().cloned().unwrap_or_else(|| label.clone());
    let (input, rx) = mpsc::channel(QUEUE);
    let pressed: Pressed = Arc::new(Mutex::new(None));
    let generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
    let first = target.clone();
    state.held.lock().insert(tk.clone(), Held { pane: label.clone(), target, banner: banner.chars().take(80).collect(), settings: Settings::from(&settings), paused: false, generation, token: None, input, pressed: pressed.clone(), last_frame: Arc::new(AtomicU64::new(now_ms())), last_input: Arc::new(AtomicU64::new(now_ms())), version: 0, fresh: Arc::new(AtomicU64::new(0)) });
    tauri::async_runtime::spawn(inputs(app.clone(), tk.clone(), rx, pressed));
    tauri::async_runtime::spawn(fallback(app.clone(), tk.clone()));
    if let Err(e) = stream_on(&app, &tk).await {
        state.held.lock().remove(&tk);
        return Err(e);
    }
    // The daemon may have given up meanwhile (a slow page): its stop found the hold already, and a screencast started after that stop must not run on.
    if !state.held.lock().contains_key(&tk) {
        if let Some(wv) = app.get_webview(&first) {
            let _ = cdp(&wv, "Page.stopScreencast", json!({})).await;
        }
        return Err("the phone's request ended before the pane was ready".into());
    }
    retitle_popups(&app, &label);
    let host = wv.url().ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    Ok(json!({"host": host}))
}

/// The person pressed Stop on the computer.
#[tauri::command]
pub async fn browser_takeover_stop(app: AppHandle, tk: String) -> CmdResult<()> {
    if stop(&app, &tk, "desktop").await {
        event(&app, json!({"kind": "ended", "tk": tk, "reason": "desktop"}));
    }
    Ok(())
}

/// The interface's answer to a `provider-request`.
#[tauri::command]
pub fn provider_reply(app: AppHandle, preq: u64, result: Option<Value>, error: Option<String>) -> CmdResult<()> {
    reply(&app, preq, match error {
        Some(e) => Err(e.chars().take(300).collect()),
        None => Ok(result.unwrap_or(Value::Null)),
    });
    Ok(())
}

/// Releases the mouse button in `target` if a drag left it pressed.
async fn release(app: &AppHandle, target: &str, pressed: &Pressed) {
    let at = pressed.lock().take();
    if let (Some((x, y)), Some(wv)) = (at, app.get_webview(target)) {
        let _ = cdp(&wv, "Input.dispatchMouseEvent", json!({"type": "mouseReleased", "x": x, "y": y, "button": "left", "buttons": 0, "clickCount": 1})).await;
    }
}

/// Ends takeover `tk` here: the stream stops, the interface takes its banner down. False when there was none: a start for it that comes later is refused.
async fn stop(app: &AppHandle, tk: &str, reason: &str) -> bool {
    let state = app.state::<Takeovers>();
    let Some(held) = state.held.lock().remove(tk) else {
        state.cancel(tk);
        return false;
    };
    release(app, &held.target, &held.pressed).await;
    stream_off(app, &held.target, held.token).await;
    retitle_popups(app, &held.pane);
    let _ = app.emit_to("main", "takeover-ended", json!({"tk": tk, "pane": held.pane, "reason": reason}));
    true
}

/// A pane's webview is closing: its takeover ends, and the daemon hears why.
pub(super) async fn pane_closed(app: &AppHandle, label: &str) {
    app.state::<Takeovers>().set_visible(label, true);
    let tk = app.state::<Takeovers>().tk_of_pane(label);
    if let Some(tk) = tk {
        if stop(app, &tk, "closed").await {
            event(app, json!({"kind": "ended", "tk": tk, "reason": "closed"}));
        }
    }
}

/// The window is about to hide pane `label` (`browser_visible`). From now on nothing but a release goes to it, and a button the phone holds down there
/// is released first, while the page is still shown: WebView2 would keep a release sent later until the pane shows again, and then run it on whatever
/// is under that point by then.
pub(super) async fn hiding(app: &AppHandle, label: &str) {
    let state = app.state::<Takeovers>();
    state.set_visible(label, false);
    let pressed = state.held.lock().values().find(|h| h.target == label).map(|h| h.pressed.clone());
    if let Some(pressed) = pressed {
        release(app, label, &pressed).await;
    }
}

/// The daemon's connection dropped: it ended every takeover with it.
pub(super) fn daemon_gone(app: &AppHandle) {
    let Some(state) = app.try_state::<Takeovers>() else { return };
    let tks: Vec<String> = state.held.lock().keys().cloned().collect();
    for tk in tks {
        let app = app.clone();
        tauri::async_runtime::spawn(async move { stop(&app, &tk, "app_closed").await });
    }
}

/// The group's top window changed (a popup opened or closed): the phone sees that one from now on, under a new window id, so that input aimed at the
/// window before goes nowhere.
pub(super) fn retarget(app: &AppHandle, pane: &str) {
    let Some(tk) = app.state::<Takeovers>().tk_of_pane(pane) else { return };
    let app = app.clone();
    let pane = pane.to_string();
    tauri::async_runtime::spawn(async move {
        let next = app.state::<Browsers>().popups_of(&pane).last().cloned().unwrap_or_else(|| pane.clone());
        let (old, generation) = {
            let state = app.state::<Takeovers>();
            let mut held = state.held.lock();
            let Some(h) = held.get_mut(&tk) else { return };
            if h.target == next {
                return;
            }
            let old = (h.target.clone(), h.token.take(), h.pressed.clone());
            h.target = next;
            h.generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
            h.fresh.fetch_add(1, Ordering::Relaxed);
            (old, h.generation)
        };
        release(&app, &old.0, &old.2).await;
        stream_off(&app, &old.0, old.1).await;
        // A popup that has just been made may not take it yet: tried again a few times, and once more when its page has loaded (`popup_loaded`). Not
        // once the window changed again: that change has its own tries.
        for _ in 0..10 {
            if stream_on(&app, &tk).await.is_ok() || app.state::<Takeovers>().window(&tk).is_none_or(|(_, w)| w != generation) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        }
        retitle_popups(&app, &pane);
    });
}

/// A popup's page finished loading: if it is what the phone sees, its screencast starts again there.
pub(super) fn popup_loaded(app: &AppHandle, pane: &str, popup: &str) {
    let Some(tk) = app.state::<Takeovers>().tk_of_pane(pane) else { return };
    if app.state::<Takeovers>().target(&tk, None).as_deref() != Some(popup) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = stream_on(&app, &tk).await;
    });
}

fn retitle_popups(app: &AppHandle, pane: &str) {
    let browsers = app.state::<Browsers>();
    for popup in browsers.popups_of(pane) {
        if let Some(w) = app.get_webview_window(&popup) {
            let title = browsers.popup_page_title(&popup);
            let _ = w.set_title(&app.state::<Takeovers>().popup_title(pane, &title));
        }
    }
}

/// What the daemon's ladder says now (`browser.takeover_set`), kept on the hold if it is newer than the last one applied: false when there is nothing to
/// do (an older notice, or no such takeover). A notice that resumes or asks for a fresh frame (`fresh`) starts the captures over too, so the next capture
/// goes even if it shows the same picture as one sent before: the phone may never have got that one. A new ladder step alone does not: nothing was lost.
fn apply_settings(held: &mut HashMap<String, Held>, tk: &str, params: &Value) -> bool {
    let Some(h) = held.get_mut(tk) else { return false };
    let version = params["v"].as_u64().unwrap_or(0);
    let fresh = params["fresh"].as_bool() == Some(true);
    if version != 0 && version <= h.version {
        // Overtaken by a newer notice, its ask for a fresh frame still stands (a rebind's, overtaken by a ladder step that says nothing of it), unless
        // the hold is paused now: the resume will ask again.
        if fresh && !h.paused {
            h.fresh.fetch_add(1, Ordering::Relaxed);
        }
        return false;
    }
    h.version = version.max(h.version);
    let was_paused = h.paused;
    h.paused = params["paused"].as_bool() == Some(true);
    h.settings = Settings::from(params);
    if !h.paused && (was_paused || fresh) {
        h.fresh.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// Makes the stream match the hold: stopped while paused, started again with its settings (which also brings a fresh frame) otherwise. Two of these
/// at once end the same way: the hold is read again after the stop, and a notice applied meanwhile leaves the start to its own task.
async fn reconcile(app: &AppHandle, tk: &str) {
    let Some((target, version)) = app.state::<Takeovers>().held.lock().get(tk).map(|h| (h.target.clone(), h.version)) else { return };
    let Some(wv) = app.get_webview(&target) else { return };
    let _ = cdp(&wv, "Page.stopScreencast", json!({})).await;
    let Some(settings) = start_after_stop(&app.state::<Takeovers>().held.lock(), tk, &target, version) else { return };
    let _ = cdp(&wv, "Page.startScreencast", settings.screencast()).await;
}

/// What `reconcile` starts the screencast again with, once it stopped it: nothing if the takeover ended, is paused, streams another window now, or
/// took a newer notice than the one this task was for (`version`).
fn start_after_stop(held: &HashMap<String, Held>, tk: &str, target: &str, version: u64) -> Option<Settings> {
    held.get(tk).filter(|h| !h.paused && h.target == target && h.version == version).map(|h| h.settings)
}

/// Attaches the frame receiver (once per target) and starts the screencast, unless the takeover is paused: again on a page that has loaded since,
/// which a new popup's has (its DevTools state starts over when it takes the opener's request). Two callers at once attach one receiver: the second
/// to finish removes its own.
async fn stream_on(app: &AppHandle, tk: &str) -> CmdResult<()> {
    let state = app.state::<Takeovers>();
    let (target, generation, settings, subscribed) = {
        let held = state.held.lock();
        let h = held.get(tk).ok_or("that takeover has ended")?;
        (h.target.clone(), h.generation, h.settings, h.token.is_some())
    };
    let wv = app.get_webview(&target).ok_or_else(|| format!("no browser {target}"))?;
    if !subscribed {
        let token = subscribe(&wv, frame_sink(app), tk.to_string(), generation).await?;
        let kept = {
            let mut held = state.held.lock();
            match held.get_mut(tk).filter(|h| h.generation == generation) {
                Some(h) if h.token.is_none() => {
                    h.token = Some(token);
                    Ok(())
                }
                // Another call attached one meanwhile: this one goes.
                Some(_) => Err(false),
                None => Err(true),
            }
        };
        match kept {
            Ok(()) => {}
            Err(ended) => {
                unsubscribe(&wv, token);
                if ended {
                    return Err("that takeover has ended".into());
                }
            }
        }
    }
    if state.held.lock().get(tk).is_none_or(|h| h.paused || h.generation != generation) {
        return Ok(());
    }
    let _ = cdp(&wv, "Page.stopScreencast", json!({})).await;
    cdp(&wv, "Page.startScreencast", settings.screencast()).await.map(|_| ())
}

async fn stream_off(app: &AppHandle, target: &str, token: Option<i64>) {
    if let Some(wv) = app.get_webview(target) {
        let _ = cdp(&wv, "Page.stopScreencast", json!({})).await;
        if let Some(token) = token {
            unsubscribe(&wv, token);
        }
    }
}

/// Where frames go: one task for all takeovers, made with the first.
fn frame_sink(app: &AppHandle) -> FrameSink {
    let state = app.state::<Takeovers>();
    let mut sink = state.sink.lock();
    if let Some(s) = sink.as_ref().filter(|s| !s.is_closed()) {
        return s.clone();
    }
    let (tx, rx) = mpsc::unbounded_channel();
    tauri::async_runtime::spawn(frames(app.clone(), rx));
    *sink = Some(tx.clone());
    tx
}

/// Each frame: acknowledged to the page at once (the daemon keeps only the newest, so the page never waits on the phone), then handed to the daemon
/// with the id of the window it shows.
async fn frames(app: AppHandle, mut rx: mpsc::UnboundedReceiver<(String, u64, String)>) {
    while let Some((tk, generation, json)) = rx.recv().await {
        let Some(target) = app.state::<Takeovers>().target(&tk, Some(generation)) else { continue };
        let Ok(mut v) = serde_json::from_str::<Value>(&json) else { continue };
        drop(json);
        if let Some(wv) = app.get_webview(&target) {
            ack(&wv, v["sessionId"].clone());
        }
        let mut meta = v.get_mut("metadata").map(Value::take).unwrap_or_else(|| json!({}));
        meta["window"] = json!(if target.contains("-popup-") { "popup" } else { "pane" });
        meta["window_id"] = json!(generation);
        if let Some(h) = app.state::<Takeovers>().held.lock().get(&tk) {
            h.last_frame.store(now_ms(), Ordering::Relaxed);
        }
        let data = v.get_mut("data").map(Value::take).unwrap_or(Value::Null);
        event(&app, json!({"kind": "frame", "tk": tk, "data": data, "meta": meta}));
    }
}

/// How long a stream may stay silent before the page is captured instead.
const STARVED_MS: u64 = 1000;
/// The shortest gap between captures, whatever the ladder allows: a capture costs the window a full render and encode.
const CAPTURE_MIN_MS: u64 = 500;
/// After this many identical captures in a row, and no input for `STILL_INPUT_MS`, the page counts as still and is looked at only every `STILL_MS`.
const STILL_AFTER: u32 = 3;
const STILL_INPUT_MS: u64 = 2000;
const STILL_MS: u64 = 2000;
const CAPTURE_TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// The captures of one takeover: when the last was taken, and how many in a row showed the same picture (which is not sent again).
#[derive(Debug, Default)]
struct Captures {
    last_at: u64,
    last_hash: Option<u64>,
    same: u32,
}

impl Captures {
    /// The gap the next capture waits: the ladder's rate (`every_nth` of the screencast's 60 a second), never under `CAPTURE_MIN_MS`, and `STILL_MS` on a
    /// still page nobody touches.
    fn gap(&self, settings: Settings, since_input: u64) -> u64 {
        let rate = (1000 * u64::from(settings.every_nth) / 60).max(CAPTURE_MIN_MS);
        if self.same >= STILL_AFTER && since_input >= STILL_INPUT_MS {
            rate.max(STILL_MS)
        } else {
            rate
        }
    }

    /// Whether a capture is due at `now`: the stream has said nothing for `STARVED_MS`, and the gap has passed.
    fn due(&self, now: u64, last_frame: u64, settings: Settings, since_input: u64) -> bool {
        now.saturating_sub(last_frame) >= STARVED_MS && now.saturating_sub(self.last_at) >= self.gap(settings, since_input)
    }

    /// A capture was taken: whether it goes to the phone. The same picture as the last one does not (the phone has it already, and would have
    /// nothing to acknowledge it with).
    fn taken(&mut self, now: u64, data: &str) -> bool {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        data.hash(&mut h);
        let hash = h.finish();
        self.last_at = now;
        if self.last_hash == Some(hash) {
            self.same += 1;
            return false;
        }
        self.last_hash = Some(hash);
        self.same = 0;
        true
    }

    /// The screencast sent a frame: what the phone shows is that one now.
    fn streamed(&mut self) {
        self.last_hash = None;
        self.same = 0;
    }

    /// The stream starts over: nothing sent before counts, and the next capture is due at once.
    fn forget(&mut self) {
        self.streamed();
        self.last_at = 0;
    }
}

/// A page the screencast sends nothing from (WebView2 paints no frames for a pane that is hidden or a window another covers, nor for a page that does
/// not change) is captured with `Page.captureScreenshot` instead, at the step's quality and width and no faster than its rate, until frames come again.
/// Nothing is captured while the takeover is paused (the phone is behind, or its link is down). The same picture is not sent twice, and a still page
/// nobody touches is looked at less often.
///
/// It also watches whether the computer shows the window at all (`known_hidden`, and the page's own word), and tells the daemon when that changes, so
/// that the phone says the pane is hidden rather than show a picture that looks live. Nothing is captured of a pane the computer does not show:
/// WebView2 renders none, and the capture would only wait.
async fn fallback(app: AppHandle, tk: String) {
    let mut captures = Captures::default();
    let (mut seen_frame, mut seen_fresh) = (0, 0);
    let mut told: Option<bool> = None;
    loop {
        tokio::time::sleep(CAPTURE_TICK).await;
        let Some((target, generation, settings, paused, last, input, fresh)) = app.state::<Takeovers>().held.lock().get(&tk).map(|h| (h.target.clone(), h.generation, h.settings, h.paused, h.last_frame.load(Ordering::Relaxed), h.last_input.load(Ordering::Relaxed), h.fresh.load(Ordering::Relaxed))) else { return };
        if last != seen_frame {
            seen_frame = last;
            captures.streamed();
        }
        if fresh != seen_fresh {
            // The daemon asked for a fresh frame, or the window changed: the next capture goes at once, whatever was sent before.
            seen_fresh = fresh;
            captures.forget();
        }
        let Some(wv) = app.get_webview(&target) else { continue };
        // A page between two documents cannot say: what was told stands.
        let hidden = if known_hidden(&app, &target, &wv) { Some(true) } else { page_hidden(&wv).await };
        if let Some(hidden) = hidden.filter(|h| told != Some(*h)) {
            if told == Some(true) {
                // Shown again: the phone has said "hidden" meanwhile, so the picture goes at once even if it is the one before.
                captures.forget();
            }
            told = Some(hidden);
            event(&app, json!({"kind": "hidden", "tk": tk, "on": hidden}));
        }
        let now = now_ms();
        if told == Some(true) || paused || !captures.due(now, last, settings, now.saturating_sub(input)) {
            continue;
        }
        let Ok(metrics) = cdp(&wv, "Page.getLayoutMetrics", json!({})).await else { continue };
        let view = &metrics["cssVisualViewport"];
        let (width, height) = (view["clientWidth"].as_f64().unwrap_or(0.0), view["clientHeight"].as_f64().unwrap_or(0.0));
        if width < 1.0 || height < 1.0 {
            continue;
        }
        let scale = (f64::from(settings.max_width) / width).min(1.0);
        let shot = cdp(&wv, "Page.captureScreenshot", json!({"format": "jpeg", "quality": settings.quality, "clip": {"x": view["pageX"].as_f64().unwrap_or(0.0), "y": view["pageY"].as_f64().unwrap_or(0.0), "width": width, "height": height, "scale": scale}})).await;
        let Ok(mut shot) = shot else { continue };
        let data = shot.get_mut("data").map(Value::take).unwrap_or(Value::Null);
        let Some(text) = data.as_str() else { continue };
        if !captures.taken(now_ms(), text) {
            continue;
        }
        // Still the window it was taken of, and still no screencast frame meanwhile.
        let current = app.state::<Takeovers>().held.lock().get(&tk).is_some_and(|h| h.generation == generation && h.last_frame.load(Ordering::Relaxed) == last && !h.paused);
        if !current {
            captures.streamed();
            continue;
        }
        let meta = json!({"deviceWidth": width, "deviceHeight": height, "offsetTop": 0, "pageScaleFactor": 1, "window": if target.contains("-popup-") { "popup" } else { "pane" }, "window_id": generation, "captured": true});
        event(&app, json!({"kind": "frame", "tk": tk, "data": data, "meta": meta}));
    }
}

/// What one queued item comes to, at its turn: the protocol calls to make in order, and for text the request to answer and whether it may go.
#[derive(Debug, PartialEq)]
enum Plan {
    Calls(Vec<(&'static str, Value)>),
    Text { preq: u64, text: Option<String>, why: &'static str },
}

const WINDOW_CHANGED: &str = "the window changed: check the page, then type it again";
const TOO_LATE: &str = "not sent: it came too late";
const HIDDEN_TEXT: &str = "not sent: the pane is hidden on the computer";
const ENDED: &str = "that takeover has ended";
const MOUSE: &str = "Input.dispatchMouseEvent";

/// Input stamped `st` (the phone's reading of this computer's clock) is too old, or from too far ahead, at `now`.
fn stale(st: u64, now: u64) -> bool {
    now.saturating_sub(st) > INPUT_TTL_MS || st > now + INPUT_TTL_MS
}

fn item_st(item: &Queued) -> u64 {
    match item {
        Queued::Ops { st, .. } | Queued::Text { st, .. } => *st,
    }
}

/// Decides an item at its turn against the window streamed now (`window`, `hidden` while the computer does not show it) and the clock. Input meant for
/// another window is dropped; stale input, or input for a hidden pane, is dropped but for releases, which never leave a button pressed.
fn plan(item: Queued, window: u64, hidden: bool, now: u64, pressed: &mut Option<(f64, f64)>) -> Plan {
    match item {
        Queued::Ops { ops, window: w, st } => {
            if w != window {
                return Plan::Calls(vec![]);
            }
            if !stale(st, now) && !hidden {
                return Plan::Calls(ops.iter().flat_map(|op| protocol_events(op, pressed)).collect());
            }
            // Late, or for a pane the computer does not show: only a release, at the point the button went down, so a drag that waited moves nothing.
            let Some((x, y)) = *pressed else { return Plan::Calls(vec![]) };
            let up = ops.iter().any(|op| op["k"] == "up");
            Plan::Calls(if up { protocol_events(&json!({"k": "up", "x": x, "y": y}), pressed) } else { vec![] })
        }
        Queued::Text { text, window: w, st, preq } => {
            if w != window {
                Plan::Text { preq, text: None, why: WINDOW_CHANGED }
            } else if stale(st, now) {
                Plan::Text { preq, text: None, why: TOO_LATE }
            } else if hidden {
                Plan::Text { preq, text: None, why: HIDDEN_TEXT }
            } else {
                Plan::Text { preq, text: Some(text), why: "" }
            }
        }
    }
}

fn point(params: &Value) -> (f64, f64) {
    (params["x"].as_f64().unwrap_or(0.0), params["y"].as_f64().unwrap_or(0.0))
}

/// Runs one takeover's queue, in order. At each item's turn `look` says which window is streamed now and whether the computer shows it (asking the
/// page itself; none once the takeover ended); before each call but a release `window` says it again from what is known here, and the clock is read
/// again, so that a pane hidden, a window changed or input grown stale in the middle of an item ends the item there. `call(window, method, params,
/// guarded)` makes a protocol call on that window only, and, `guarded`, only while the window thread finds the pane shown (it answers `NOT_SHOWN`
/// otherwise). `pressed` follows what was sent, call by call, so that a release is sent once, by whoever comes first (this queue, a takeover's end,
/// the window before it hides the pane). `answer` answers text's request once it is typed or refused.
async fn run_queue<L, LF, C, F>(mut rx: mpsc::Receiver<Queued>, mut look: L, window: impl Fn() -> Option<(u64, bool)>, pressed: Pressed, mut call: C, mut answer: impl FnMut(u64, Result<Value, String>))
where
    L: FnMut() -> LF,
    LF: Future<Output = Option<(u64, bool)>>,
    C: FnMut(u64, &'static str, Value, bool) -> F,
    F: Future<Output = CmdResult<Value>>,
{
    while let Some(item) = rx.recv().await {
        let Some((current, hidden)) = look().await else {
            if let Queued::Text { preq, .. } = item {
                answer(preq, Err(ENDED.into()));
            }
            return;
        };
        let st = item_st(&item);
        let still = || match window() {
            None => Err(ENDED),
            Some((w, _)) if w != current => Err(WINDOW_CHANGED),
            Some((_, true)) => Err(HIDDEN_TEXT),
            Some(_) if stale(st, now_ms()) => Err(TOO_LATE),
            Some(_) => Ok(()),
        };
        // Planned against a copy: `pressed` itself changes only as calls go.
        let mut planned = *pressed.lock();
        match plan(item, current, hidden, now_ms(), &mut planned) {
            Plan::Calls(calls) => {
                for (method, params) in calls {
                    let kind = params["type"].as_str().unwrap_or_default().to_string();
                    let release = method == MOUSE && kind == "mouseReleased";
                    if release {
                        // Released already: by the window before it hid the pane, or by the takeover's end.
                        if pressed.lock().take().is_none() {
                            continue;
                        }
                    } else {
                        if still().is_err() {
                            break;
                        }
                        if method == MOUSE && kind == "mousePressed" {
                            *pressed.lock() = Some(point(&params));
                        } else if method == MOUSE && kind == "mouseMoved" && params["buttons"] == 1 {
                            let mut p = pressed.lock();
                            if p.is_some() {
                                *p = Some(point(&params));
                            }
                        }
                    }
                    // A page that refuses one event gets the next: what was refused is not said anywhere. One the computer does not show ends the item.
                    let done = call(current, method, params, !release).await;
                    if matches!(&done, Err(e) if e == NOT_SHOWN) {
                        if kind == "mousePressed" {
                            *pressed.lock() = None;
                        }
                        break;
                    }
                }
            }
            Plan::Text { preq, text: Some(text), .. } => {
                let done = match still() {
                    Ok(()) => call(current, "Input.insertText", json!({"text": text}), true).await,
                    Err(why) => Err(why.into()),
                };
                answer(preq, done.map(|_| json!(true)).map_err(|e| if e == NOT_SHOWN { HIDDEN_TEXT.into() } else { e }));
            }
            Plan::Text { preq, text: None, why } => answer(preq, Err(why.into())),
        }
    }
}

/// How long the page may take to say whether it is shown.
const PROBE: std::time::Duration = std::time::Duration::from_millis(500);

/// Whether the page says it is hidden (`document.visibilityState`): WebView2 says so for a view it hides, and for a window minimised or covered as far
/// as it tracks that. One that does not answer in `PROBE` counts as hidden (WebView2 may hold calls to a page it does not render); none when it
/// answered with an error (a page between two documents).
async fn page_hidden(wv: &tauri::Webview) -> Option<bool> {
    match tokio::time::timeout(PROBE, cdp(wv, "Runtime.evaluate", json!({"expression": "document.visibilityState", "returnByValue": true}))).await {
        Ok(Ok(v)) => Some(v["result"]["value"].as_str() != Some("visible")),
        Ok(Err(_)) => None,
        Err(_) => Some(true),
    }
}

/// Whether the computer shows `target` now as far as is known here: not hidden by the window (`browser_visible`), and its window not minimised.
fn known_hidden(app: &AppHandle, target: &str, wv: &tauri::Webview) -> bool {
    app.state::<Takeovers>().hidden(target) || wv.window().is_minimized().unwrap_or(false)
}

/// The phone's input and text for `tk`, in the order they came, into the window streamed now.
async fn inputs(app: AppHandle, tk: String, rx: mpsc::Receiver<Queued>, pressed: Pressed) {
    let (a, b, c, d) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (ta, tb, td) = (tk.clone(), tk.clone(), tk.clone());
    run_queue(
        rx,
        move || {
            let (app, tk) = (a.clone(), ta.clone());
            async move {
                let (target, w) = app.state::<Takeovers>().window(&tk)?;
                // Asked of the page only when nothing here says it is hidden already; a page that cannot say counts as hidden.
                let hidden = match app.get_webview(&target) {
                    Some(wv) => known_hidden(&app, &target, &wv) || page_hidden(&wv).await.unwrap_or(true),
                    None => true,
                };
                Some((w, hidden))
            }
        },
        move || {
            let state = d.state::<Takeovers>();
            state.window(&td).map(|(target, w)| (w, state.hidden(&target)))
        },
        pressed,
        move |window, method, params, guarded| {
            let target = b.state::<Takeovers>().target(&tb, Some(window));
            let wv = target.as_ref().and_then(|t| b.get_webview(t));
            let app = b.clone();
            async move {
                let (Some(wv), Some(target)) = (wv, target) else { return Err("the page is gone".into()) };
                let hidden: Option<Box<dyn FnOnce() -> bool + Send>> = if guarded { Some(Box::new(move || app.state::<Takeovers>().hidden(&target))) } else { None };
                cdp_when(&wv, method, params, hidden).await
            }
        },
        move |preq, result| reply(&c, preq, result),
    )
    .await;
}

/// `(key, code, Windows virtual key, text)` of the keys the daemon lets through.
fn key_info(key: &str) -> Option<(&'static str, &'static str, u32, Option<&'static str>)> {
    Some(match key {
        "Enter" => ("Enter", "Enter", 13, Some("\r")),
        "Tab" => ("Tab", "Tab", 9, None),
        "Backspace" => ("Backspace", "Backspace", 8, None),
        "Delete" => ("Delete", "Delete", 46, None),
        "Escape" => ("Escape", "Escape", 27, None),
        "ArrowUp" => ("ArrowUp", "ArrowUp", 38, None),
        "ArrowDown" => ("ArrowDown", "ArrowDown", 40, None),
        "ArrowLeft" => ("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowRight" => ("ArrowRight", "ArrowRight", 39, None),
        "Home" => ("Home", "Home", 36, None),
        "End" => ("End", "End", 35, None),
        "PageUp" => ("PageUp", "PageUp", 33, None),
        "PageDown" => ("PageDown", "PageDown", 34, None),
        "Space" => (" ", "Space", 32, Some(" ")),
        _ => return None,
    })
}

/// One input event as the DevTools protocol takes it. `pressed` is where the button went down, while it is down.
fn protocol_events(op: &Value, pressed: &mut Option<(f64, f64)>) -> Vec<(&'static str, Value)> {
    let (x, y) = (op["x"].as_f64().unwrap_or(0.0), op["y"].as_f64().unwrap_or(0.0));
    let mouse = |kind: &str, button: &str, buttons: u8, clicks: u8| json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks});
    match op["k"].as_str() {
        Some("tap") => {
            *pressed = None;
            vec![(MOUSE, mouse("mouseMoved", "none", 0, 0)), (MOUSE, mouse("mousePressed", "left", 1, 1)), (MOUSE, mouse("mouseReleased", "left", 0, 1))]
        }
        Some("down") => {
            *pressed = Some((x, y));
            vec![(MOUSE, mouse("mouseMoved", "none", 0, 0)), (MOUSE, mouse("mousePressed", "left", 1, 1))]
        }
        Some("move") => {
            let down = pressed.is_some();
            if down {
                *pressed = Some((x, y));
            }
            vec![(MOUSE, mouse("mouseMoved", if down { "left" } else { "none" }, u8::from(down), 0))]
        }
        // A release with no press recorded (its press was dropped as late) sends nothing: the page must not see a click it never had.
        Some("up") => match pressed.take() {
            Some(_) => vec![(MOUSE, mouse("mouseReleased", "left", 0, 1))],
            None => vec![],
        },
        Some("wheel") => vec![(MOUSE, json!({"type": "mouseWheel", "x": x, "y": y, "deltaX": op["dx"].as_f64().unwrap_or(0.0), "deltaY": op["dy"].as_f64().unwrap_or(0.0)}))],
        Some("key") => {
            let Some((key, code, vk, text)) = op["key"].as_str().and_then(key_info) else { return vec![] };
            let mut down = json!({"type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk});
            if let Some(t) = text {
                down["text"] = json!(t);
                down["unmodifiedText"] = json!(t);
            }
            vec![("Input.dispatchKeyEvent", down), ("Input.dispatchKeyEvent", json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk}))]
        }
        _ => vec![],
    }
}

#[cfg(windows)]
async fn cdp(wv: &tauri::Webview, method: &'static str, params: Value) -> CmdResult<Value> {
    super::devtools(wv, method, params).await
}

#[cfg(not(windows))]
async fn cdp(_wv: &tauri::Webview, _method: &'static str, _params: Value) -> CmdResult<Value> {
    Err("taking over a pane is not available on this platform yet".into())
}

/// `cdp`, with `hidden` given only while the computer shows the page (`devtools_when`).
#[cfg(windows)]
async fn cdp_when(wv: &tauri::Webview, method: &'static str, params: Value, hidden: Option<Box<dyn FnOnce() -> bool + Send>>) -> CmdResult<Value> {
    super::devtools_when(wv, method, params, hidden).await
}

#[cfg(not(windows))]
async fn cdp_when(wv: &tauri::Webview, method: &'static str, params: Value, _hidden: Option<Box<dyn FnOnce() -> bool + Send>>) -> CmdResult<Value> {
    cdp(wv, method, params).await
}

/// Acknowledges a frame without waiting for the answer: the page sends the next one when it has it.
#[cfg(windows)]
fn ack(wv: &tauri::Webview, session_id: Value) {
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows::core::HSTRING;
    let params = json!({"sessionId": session_id}).to_string();
    let _ = wv.with_webview(move |pv| unsafe {
        if let Ok(core) = pv.controller().CoreWebView2() {
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
            let _ = core.CallDevToolsProtocolMethod(&HSTRING::from("Page.screencastFrameAck"), &HSTRING::from(params.as_str()), &handler);
        }
    });
}

#[cfg(not(windows))]
fn ack(_wv: &tauri::Webview, _session_id: Value) {}

/// Attaches a `Page.screencastFrame` receiver to the page; its registration token. The handler runs on the window's thread and only hands the event's JSON on.
#[cfg(windows)]
async fn subscribe(wv: &tauri::Webview, sink: FrameSink, tk: String, generation: u64) -> CmdResult<i64> {
    use webview2_com::DevToolsProtocolEventReceivedEventHandler;
    use windows::core::{HSTRING, PWSTR};
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<i64, String>>();
    wv.with_webview(move |pv| unsafe {
        let attached = (|| -> windows::core::Result<i64> {
            let core = pv.controller().CoreWebView2()?;
            let receiver = core.GetDevToolsProtocolEventReceiver(&HSTRING::from("Page.screencastFrame"))?;
            let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    let mut json = PWSTR::null();
                    if args.ParameterObjectAsJson(&mut json).is_ok() {
                        let _ = sink.send((tk.clone(), generation, webview2_com::take_pwstr(json)));
                    }
                }
                Ok(())
            }));
            let mut token = 0i64;
            receiver.add_DevToolsProtocolEventReceived(&handler, &mut token)?;
            Ok(token)
        })();
        let _ = tx.send(attached.map_err(|e| e.to_string()));
    })
    .map_err(crate::fs_ops::err)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), rx).await.map_err(|_| "the page did not answer".to_string())?.map_err(|_| "cancelled".to_string())?
}

#[cfg(not(windows))]
async fn subscribe(_wv: &tauri::Webview, _sink: FrameSink, _tk: String, _generation: u64) -> CmdResult<i64> {
    Err("taking over a pane is not available on this platform yet".into())
}

#[cfg(windows)]
fn unsubscribe(wv: &tauri::Webview, token: i64) {
    use windows::core::HSTRING;
    let _ = wv.with_webview(move |pv| unsafe {
        if let Ok(core) = pv.controller().CoreWebView2() {
            if let Ok(receiver) = core.GetDevToolsProtocolEventReceiver(&HSTRING::from("Page.screencastFrame")) {
                let _ = receiver.remove_DevToolsProtocolEventReceived(token);
            }
        }
    });
}

#[cfg(not(windows))]
fn unsubscribe(_wv: &tauri::Webview, _token: i64) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_s_settings_are_kept_in_range() {
        let s = Settings::from(&json!({"quality": 500, "max_width": 1, "every_nth": 0}));
        assert_eq!(s, Settings { quality: 90, max_width: 320, every_nth: 1 });
        let cast = Settings::from(&json!({"quality": 55, "max_width": 1024, "every_nth": 12})).screencast();
        assert_eq!((cast["format"].as_str(), cast["quality"].as_u64(), cast["maxWidth"].as_u64(), cast["everyNthFrame"].as_u64()), (Some("jpeg"), Some(55), Some(1024), Some(12)));
    }

    #[test]
    fn a_tap_is_a_press_and_a_release_and_a_drag_holds_the_button() {
        let mut pressed = None;
        let tap = protocol_events(&json!({"k": "tap", "x": 10.5, "y": 20}), &mut pressed);
        let kinds: Vec<&str> = tap.iter().map(|(_, p)| p["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
        assert_eq!((tap[1].1["x"].as_f64(), tap[1].1["button"].as_str(), tap[1].1["clickCount"].as_u64()), (Some(10.5), Some("left"), Some(1)));
        protocol_events(&json!({"k": "down", "x": 1, "y": 1}), &mut pressed);
        let moved = protocol_events(&json!({"k": "move", "x": 5, "y": 5}), &mut pressed);
        assert_eq!((moved[0].1["button"].as_str(), moved[0].1["buttons"].as_u64()), (Some("left"), Some(1)));
        assert_eq!(pressed, Some((5.0, 5.0)), "where it would be released");
        protocol_events(&json!({"k": "up", "x": 5, "y": 5}), &mut pressed);
        assert_eq!(pressed, None);
        let wheel = protocol_events(&json!({"k": "wheel", "x": 1, "y": 2, "dx": 0, "dy": 120}), &mut pressed);
        assert_eq!((wheel[0].1["type"].as_str(), wheel[0].1["deltaY"].as_f64()), (Some("mouseWheel"), Some(120.0)));
    }

    #[test]
    fn a_release_with_no_press_sends_nothing() {
        let mut pressed = None;
        assert!(protocol_events(&json!({"k": "up", "x": 3, "y": 3}), &mut pressed).is_empty());
    }

    #[test]
    fn a_fresh_start_sends_the_same_picture_again_at_once() {
        // The capture went, and may never have reached the phone (a link that dropped, a frame the daemon dropped): after the daemon's fresh request
        // the same picture is due at once and goes.
        let mut c = Captures::default();
        assert!(c.taken(10_000, "H"));
        assert!(!c.taken(10_500, "H"));
        c.forget();
        assert!(c.due(10_600, 0, settings(6), 0), "due at once");
        assert!(c.taken(10_600, "H"), "and it goes");
    }

    #[test]
    fn an_older_notice_of_the_daemon_is_ignored_and_a_resume_starts_the_captures_over() {
        let (input, _rx) = mpsc::channel(1);
        let mut held = HashMap::new();
        held.insert("tk".to_string(), Held { pane: "browser-a".into(), target: "browser-a".into(), banner: String::new(), settings: Settings::from(&json!({})), paused: false, generation: 1, token: None, input, pressed: Arc::new(Mutex::new(None)), last_frame: Arc::new(AtomicU64::new(0)), last_input: Arc::new(AtomicU64::new(0)), version: 0, fresh: Arc::new(AtomicU64::new(0)) });
        // Resume (3) then pause (2), as two notices sent out of order arrive: the resume stands.
        assert!(apply_settings(&mut held, "tk", &json!({"v": 3, "paused": false, "fresh": true})));
        assert!(!apply_settings(&mut held, "tk", &json!({"v": 2, "paused": true})));
        let h = &held["tk"];
        assert!(!h.paused && h.version == 3);
        assert_eq!(h.fresh.load(Ordering::Relaxed), 1, "the resume started the captures over");
        assert!(apply_settings(&mut held, "tk", &json!({"v": 4, "paused": true})));
        assert!(held["tk"].paused);
        assert_eq!(held["tk"].fresh.load(Ordering::Relaxed), 1, "a pause does not");
        assert!(apply_settings(&mut held, "tk", &json!({"v": 5, "paused": false, "fresh": false})));
        assert_eq!(held["tk"].fresh.load(Ordering::Relaxed), 2, "coming out of a pause does, whatever it says");
        assert!(apply_settings(&mut held, "tk", &json!({"v": 6, "paused": false, "fresh": false, "quality": 40})));
        assert_eq!(held["tk"].fresh.load(Ordering::Relaxed), 2, "a new ladder step alone does not: the same picture is not sent again for it");
        // A rebind's fresh request (5) overtaken by a ladder step (6) that says nothing of it: the step stands, and so does the request.
        assert!(!apply_settings(&mut held, "tk", &json!({"v": 5, "paused": false, "fresh": true})));
        assert_eq!((held["tk"].version, held["tk"].settings.quality), (6, 40), "the newer notice's settings stand");
        assert_eq!(held["tk"].fresh.load(Ordering::Relaxed), 3, "but the captures start over");
        assert!(apply_settings(&mut held, "tk", &json!({"v": 7, "paused": true})));
        assert!(!apply_settings(&mut held, "tk", &json!({"v": 6, "paused": false, "fresh": true})));
        assert_eq!(held["tk"].fresh.load(Ordering::Relaxed), 3, "not while paused: the resume asks again");
        assert!(!apply_settings(&mut held, "other", &json!({"v": 9})));
    }

    #[test]
    fn a_stream_started_again_follows_the_hold_as_it_is_after_the_stop() {
        let (input, _rx) = mpsc::channel(1);
        let mut held = HashMap::new();
        held.insert("tk".to_string(), Held { pane: "browser-a".into(), target: "browser-a".into(), banner: String::new(), settings: Settings::from(&json!({})), paused: false, generation: 1, token: None, input, pressed: Arc::new(Mutex::new(None)), last_frame: Arc::new(AtomicU64::new(0)), last_input: Arc::new(AtomicU64::new(0)), version: 0, fresh: Arc::new(AtomicU64::new(0)) });
        // A resume (3) read the hold, then stopped the stream; a pause (4) was applied meanwhile: the resume's task does not start it again.
        assert!(apply_settings(&mut held, "tk", &json!({"v": 3, "paused": false})));
        assert!(start_after_stop(&held, "tk", "browser-a", 3).is_some(), "nothing came between");
        assert!(apply_settings(&mut held, "tk", &json!({"v": 4, "paused": true})));
        assert_eq!(start_after_stop(&held, "tk", "browser-a", 3), None, "a newer notice owns the stream");
        assert_eq!(start_after_stop(&held, "tk", "browser-a", 4), None, "paused, the pause's own task starts nothing");
        assert!(apply_settings(&mut held, "tk", &json!({"v": 5, "paused": false, "quality": 50})));
        assert_eq!(start_after_stop(&held, "tk", "browser-a", 5).map(|s| s.quality), Some(50), "with the settings as they are now");
        assert_eq!(start_after_stop(&held, "tk", "browser-a-popup-1", 5), None, "another window is streamed now");
        assert_eq!(start_after_stop(&held, "gone", "browser-a", 5), None);
    }

    #[test]
    fn keys_carry_their_codes_and_only_known_ones_go() {
        let mut pressed = None;
        let enter = protocol_events(&json!({"k": "key", "key": "Enter"}), &mut pressed);
        assert_eq!((enter[0].0, enter[0].1["type"].as_str(), enter[0].1["text"].as_str(), enter[0].1["windowsVirtualKeyCode"].as_u64()), ("Input.dispatchKeyEvent", Some("keyDown"), Some("\r"), Some(13)));
        assert_eq!(enter[1].1["type"], "keyUp");
        let tab = protocol_events(&json!({"k": "key", "key": "Tab"}), &mut pressed);
        assert_eq!(tab[0].1["type"], "rawKeyDown", "a key with no text");
        assert!(protocol_events(&json!({"k": "key", "key": "F12"}), &mut pressed).is_empty());
        assert!(protocol_events(&json!({"k": "eval", "x": 1, "y": 1}), &mut pressed).is_empty());
    }

    #[test]
    fn input_meant_for_another_window_or_too_old_is_dropped_but_a_release_always_goes() {
        let now = 10_000;
        let mut pressed = None;
        let tap = || vec![json!({"k": "tap", "x": 1, "y": 1})];
        assert_eq!(plan(Queued::Ops { ops: tap(), window: 2, st: now }, 3, false, now, &mut pressed), Plan::Calls(vec![]), "aimed at the window before a popup opened");
        assert_eq!(plan(Queued::Ops { ops: tap(), window: 3, st: now - 1500 }, 3, false, now, &mut pressed), Plan::Calls(vec![]), "a tap that waited too long");
        let late = |pressed: &mut Option<(f64, f64)>| plan(Queued::Ops { ops: vec![json!({"k": "move", "x": 9, "y": 9}), json!({"k": "up", "x": 4, "y": 4})], window: 3, st: now - 1500 }, 3, false, now, pressed);
        assert_eq!(late(&mut None), Plan::Calls(vec![]), "a late release with no press recorded sends nothing");
        let mut held_down = Some((1.0, 2.0));
        let Plan::Calls(late_release) = late(&mut held_down) else { panic!() };
        assert_eq!(late_release.iter().map(|(_, p)| (p["type"].as_str().unwrap(), p["x"].as_f64(), p["y"].as_f64())).collect::<Vec<_>>(), [("mouseReleased", Some(1.0), Some(2.0))], "only the release, where the button went down");
        assert_eq!(held_down, None);
        let Plan::Calls(fresh) = plan(Queued::Ops { ops: tap(), window: 3, st: now - 100 }, 3, false, now, &mut pressed) else { panic!() };
        assert_eq!(fresh.len(), 3);
        assert_eq!(plan(Queued::Text { text: "pw".into(), window: 2, st: now, preq: 5 }, 3, false, now, &mut pressed), Plan::Text { preq: 5, text: None, why: "the window changed: check the page, then type it again" });
        assert_eq!(plan(Queued::Text { text: "pw".into(), window: 3, st: now - 5000, preq: 6 }, 3, false, now, &mut pressed), Plan::Text { preq: 6, text: None, why: "not sent: it came too late" });
        assert_eq!(plan(Queued::Text { text: "pw".into(), window: 3, st: now, preq: 7 }, 3, false, now, &mut pressed), Plan::Text { preq: 7, text: Some("pw".into()), why: "" });
    }

    #[test]
    fn nothing_but_a_release_goes_to_a_pane_the_window_hides() {
        // WebView2 keeps such input until the pane is shown again, and then clicks whatever is there by then.
        let now = 10_000;
        let tap = || vec![json!({"k": "tap", "x": 1, "y": 1})];
        assert_eq!(plan(Queued::Ops { ops: tap(), window: 3, st: now }, 3, true, now, &mut None), Plan::Calls(vec![]), "a tap");
        let mut held_down = Some((1.0, 2.0));
        let Plan::Calls(release) = plan(Queued::Ops { ops: vec![json!({"k": "up", "x": 4, "y": 4})], window: 3, st: now }, 3, true, now, &mut held_down) else { panic!() };
        assert_eq!(release.iter().map(|(_, p)| (p["type"].as_str().unwrap(), p["x"].as_f64())).collect::<Vec<_>>(), [("mouseReleased", Some(1.0))], "the release, where the button went down");
        assert_eq!(plan(Queued::Text { text: "pw".into(), window: 3, st: now, preq: 8 }, 3, true, now, &mut None), Plan::Text { preq: 8, text: None, why: "not sent: the pane is hidden on the computer" });
        let t = Takeovers::default();
        t.set_visible("browser-a", false);
        assert!(t.hidden("browser-a") && !t.hidden("browser-b"));
        t.set_visible("browser-a", true);
        assert!(!t.hidden("browser-a"), "shown again, input goes");
    }

    /// The calls a queue made, and its answers to text.
    type Driven = (Vec<String>, Vec<(u64, Result<Value, String>)>);

    /// How the queue runs `items` on window 1. `look_hidden`: what the look at each item's turn says. `hidden_after`: once that many calls were made,
    /// what is known here says the pane is hidden. `refused_at`: the window thread finds it hidden at that call. The calls made (`type`, "text", and
    /// " unguarded" for one the window thread does not check), and the answers to text.
    fn drive(items: Vec<Queued>, pressed: Pressed, look_hidden: bool, hidden_after: Option<usize>, refused_at: Option<usize>) -> Driven {
        tauri::async_runtime::block_on(async move {
            let (tx, rx) = mpsc::channel(QUEUE);
            for item in items {
                tx.try_send(item).unwrap();
            }
            drop(tx);
            let calls = Arc::new(Mutex::new(Vec::<String>::new()));
            let answers = Arc::new(Mutex::new(Vec::new()));
            let (seen, made, told) = (calls.clone(), calls.clone(), answers.clone());
            let refused = Arc::new(AtomicU64::new(0));
            run_queue(
                rx,
                move || async move { Some((1, look_hidden)) },
                move || Some((1, hidden_after.is_some_and(|n| made.lock().len() >= n))),
                pressed,
                move |window, method, params, guarded| {
                    let (seen, refused) = (seen.clone(), refused.clone());
                    async move {
                        assert_eq!(window, 1, "the window the item was meant for");
                        // Each call takes a while, as a round trip to the window's thread does.
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        if guarded && refused_at == Some(seen.lock().len()) && refused.fetch_add(1, Ordering::Relaxed) == 0 {
                            return Err(NOT_SHOWN.to_string());
                        }
                        let kind = if method == "Input.insertText" { "text" } else { params["type"].as_str().unwrap_or_default() };
                        seen.lock().push(format!("{kind}{}", if guarded { "" } else { " unguarded" }));
                        Ok(json!({}))
                    }
                },
                move |preq, result| told.lock().push((preq, result)),
            )
            .await;
            let made = calls.lock().clone();
            let told = answers.lock().clone();
            (made, told)
        })
    }

    fn ops(window: u64, ops: Vec<Value>) -> Queued {
        Queued::Ops { ops, window, st: now_ms() }
    }

    #[test]
    fn text_never_overtakes_the_tap_before_it() {
        let items = vec![ops(1, vec![json!({"k": "tap", "x": 150, "y": 118})]), Queued::Text { text: "secret".into(), window: 1, st: now_ms(), preq: 9 }];
        let (calls, answers) = drive(items, Arc::new(Mutex::new(None)), false, None, None);
        assert_eq!(calls, ["mouseMoved", "mousePressed", "mouseReleased unguarded", "text"]);
        assert_eq!(answers, [(9, Ok(json!(true)))], "answered once it is typed");
    }

    #[test]
    fn a_pane_hidden_in_the_middle_of_a_tap_gets_nothing_more() {
        let pressed: Pressed = Arc::new(Mutex::new(None));
        let (calls, _) = drive(vec![ops(1, vec![json!({"k": "tap", "x": 5, "y": 5})])], pressed.clone(), false, Some(1), None);
        assert_eq!(calls, ["mouseMoved"], "no press, so no click when it shows again");
        assert_eq!(*pressed.lock(), None);
    }

    #[test]
    fn a_drag_whose_pane_is_hidden_moves_no_further_and_is_released_once() {
        let pressed: Pressed = Arc::new(Mutex::new(None));
        let items = vec![ops(1, vec![json!({"k": "down", "x": 10, "y": 10})]), ops(1, vec![json!({"k": "move", "x": 20, "y": 20})]), ops(1, vec![json!({"k": "up", "x": 20, "y": 20})])];
        let (calls, _) = drive(items, pressed.clone(), false, Some(2), None);
        assert_eq!(calls, ["mouseMoved", "mousePressed", "mouseReleased unguarded"]);
        assert_eq!(*pressed.lock(), None);
    }

    #[test]
    fn a_press_the_window_thread_refuses_is_not_counted_as_down() {
        let pressed: Pressed = Arc::new(Mutex::new(None));
        let (calls, _) = drive(vec![ops(1, vec![json!({"k": "tap", "x": 5, "y": 5})]), ops(1, vec![json!({"k": "up", "x": 5, "y": 5})])], pressed.clone(), false, None, Some(1));
        assert_eq!(calls, ["mouseMoved"], "the item ends at the refusal, and the next release has no press to release");
        assert_eq!(*pressed.lock(), None);
    }

    #[test]
    fn a_release_someone_else_sent_first_is_not_sent_again() {
        // The window released the button before it hid the pane (`hiding`): the queue finds nothing left to release.
        let pressed: Pressed = Arc::new(Mutex::new(Some((3.0, 3.0))));
        let p = pressed.clone();
        let (calls, _) = tauri::async_runtime::block_on(async move {
            let (tx, rx) = mpsc::channel(QUEUE);
            tx.try_send(ops(1, vec![json!({"k": "move", "x": 4, "y": 4}), json!({"k": "up", "x": 4, "y": 4})])).unwrap();
            drop(tx);
            let calls = Arc::new(Mutex::new(Vec::<String>::new()));
            let seen = calls.clone();
            run_queue(
                rx,
                || async { Some((1, false)) },
                || Some((1, false)),
                p.clone(),
                move |_, _, params, _| {
                    let (seen, p) = (seen.clone(), p.clone());
                    async move {
                        // The move goes; meanwhile the window releases the button and hides the pane.
                        p.lock().take();
                        seen.lock().push(params["type"].as_str().unwrap_or_default().to_string());
                        Ok(json!({}))
                    }
                },
                |_, _| {},
            )
            .await;
            let made = calls.lock().clone();
            (made, ())
        });
        assert_eq!(calls, ["mouseMoved"]);
        assert_eq!(*pressed.lock(), None);
    }

    #[test]
    fn text_for_a_pane_hidden_since_its_turn_is_not_typed() {
        let text = || Queued::Text { text: "pw".into(), window: 1, st: now_ms(), preq: 4 };
        assert_eq!(drive(vec![text()], Arc::new(Mutex::new(None)), true, None, None), (vec![], vec![(4, Err(HIDDEN_TEXT.to_string()))]), "hidden at its turn");
        assert_eq!(drive(vec![text()], Arc::new(Mutex::new(None)), false, Some(0), None), (vec![], vec![(4, Err(HIDDEN_TEXT.to_string()))]), "hidden by the time it goes");
        assert_eq!(drive(vec![text()], Arc::new(Mutex::new(None)), false, None, Some(0)), (vec![], vec![(4, Err(HIDDEN_TEXT.to_string()))]), "found hidden on the window thread");
        assert_eq!(drive(vec![ops(1, vec![json!({"k": "tap", "x": 1, "y": 1})])], Arc::new(Mutex::new(None)), true, None, None).0, Vec::<String>::new(), "and no tap at its turn either");
    }

    #[test]
    fn a_full_queue_refuses_more_rather_than_growing() {
        let (tx, _rx) = mpsc::channel::<Queued>(QUEUE);
        for _ in 0..QUEUE {
            tx.try_send(Queued::Ops { ops: vec![], window: 1, st: 0 }).unwrap();
        }
        assert!(tx.try_send(Queued::Ops { ops: vec![], window: 1, st: 0 }).is_err());
    }

    fn settings(every_nth: u8) -> Settings {
        Settings { quality: 70, max_width: 1280, every_nth }
    }

    #[test]
    fn a_silent_stream_is_captured_at_the_ladders_rate_and_never_faster_than_twice_a_second() {
        let c = Captures::default();
        assert!(!c.due(10_000, 9_500, settings(6), 0), "frames came lately: nothing to capture");
        assert!(c.due(10_000, 8_000, settings(6), 0));
        assert_eq!(c.gap(settings(6), 0), CAPTURE_MIN_MS, "10 a second on the ladder is still 2 captures a second");
        assert_eq!(c.gap(settings(60), 0), 1000, "1 a second on the ladder: 1 capture a second");
        let mut c = Captures::default();
        assert!(c.taken(10_000, "picture"));
        assert!(!c.due(10_200, 0, settings(6), 0), "the gap since the last capture");
        assert!(c.due(10_500, 0, settings(6), 0));
    }

    #[test]
    fn the_same_capture_is_not_sent_again_and_a_still_page_is_looked_at_less_often() {
        let mut c = Captures::default();
        assert!(c.taken(0, "still"));
        for i in 1..=STILL_AFTER {
            assert!(!c.taken(u64::from(i) * 500, "still"), "the phone has it");
        }
        assert_eq!(c.gap(settings(6), STILL_INPUT_MS), STILL_MS, "a still page nobody touches");
        assert_eq!(c.gap(settings(6), 100), CAPTURE_MIN_MS, "input: it may change now");
        assert!(c.taken(5_000, "changed"), "a new picture goes");
        assert_eq!(c.gap(settings(6), STILL_INPUT_MS), CAPTURE_MIN_MS);
        assert!(!c.taken(5_500, "changed"));
        c.streamed();
        assert!(c.taken(6_000, "changed"), "after a screencast frame the phone shows that one: the capture is new to it");
    }
}
