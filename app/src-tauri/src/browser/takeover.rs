//! A browser pane a phone took over: the daemon's broker (`kelpied`'s `takeover`) asks this window, which alone can reach the page, through the
//! provider channel (`provider.register`, then `preq` requests and `pnote` notices; replies and frames go back as `provider.reply` and `provider.event`).
//!
//! - The pane is streamed with `Page.startScreencast` (JPEG); each `Page.screencastFrame` is acknowledged at once and handed to the daemon, which keeps
//!   only the newest and paces it to the phone. The event receiver is attached for a takeover only: with none held, no DevTools subscription exists.
//! - The phone's input arrives as a closed set of events (`tap`, `down`, `move`, `up`, `wheel`, `key`) that the daemon checked, and goes into the page
//!   in order as `Input.dispatchMouseEvent` / `Input.dispatchKeyEvent`; text as `Input.insertText`. None of it is logged.
//! - A pane and the popups it opened (`window.open` with a size: OAuth and sign-in windows) are one group: the popup on top is streamed while it is
//!   open, and closing it goes back to the pane. The window's title says the group is controlled from the phone; the pane's own banner is the app's
//!   (`browser-pane.tsx`).
//!
//! What the window shows (picking the pane, its tab brought to the front, the banner) is the interface's: `browser.panes` and `browser.takeover_start` go to it
//! as a `provider-request` event, and it answers with `provider_reply` after `browser_takeover_start`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use super::{valid_label, Browsers};
use crate::daemon_client::DaemonClient;
use crate::fs_ops::CmdResult;

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

struct Held {
    /// The pane's webview, and the one streamed now (the pane, or its popup on top).
    pane: String,
    target: String,
    /// The window's title says who controls it.
    banner: String,
    settings: Settings,
    paused: bool,
    /// Frames of an older target (or an older start of this one) are dropped.
    generation: u64,
    /// The `Page.screencastFrame` receiver on `target`.
    token: Option<i64>,
    input: tokio::sync::mpsc::UnboundedSender<Vec<Value>>,
}

/// A frame as WebView2 handed it: takeover, generation, the event's JSON.
type FrameSink = tokio::sync::mpsc::UnboundedSender<(String, u64, String)>;

#[derive(Default)]
pub struct Takeovers {
    held: Mutex<HashMap<String, Held>>,
    generation: AtomicU64,
    sink: Mutex<Option<FrameSink>>,
}

impl Takeovers {
    fn tk_of_pane(&self, pane: &str) -> Option<String> {
        self.held.lock().iter().find(|(_, h)| h.pane == pane).map(|(tk, _)| tk.clone())
    }

    /// The webview streamed for `tk`, while `generation` is its latest.
    fn target(&self, tk: &str, generation: Option<u64>) -> Option<String> {
        self.held.lock().get(tk).filter(|h| generation.is_none_or(|g| g == h.generation)).map(|h| h.target.clone())
    }

    /// The title a popup of `pane` shows: who controls the group, while someone does.
    pub(super) fn popup_title(&self, pane: &str, title: &str) -> String {
        match self.held.lock().values().find(|h| h.pane == pane) {
            Some(h) => format!("{} — {title}", h.banner),
            None => title.to_string(),
        }
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
            "browser.takeover_text" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let tk = params["tk"].as_str().unwrap_or_default();
                    let done = insert_text(&app, tk, params["text"].as_str().unwrap_or_default()).await;
                    reply(&app, preq, done.map(|()| json!(true)));
                });
            }
            _ => reply(app, preq, Err(format!("unknown request {method}"))),
        }
        return;
    }
    let params = msg["params"].clone();
    let tk = params["tk"].as_str().unwrap_or_default().to_string();
    match msg["pnote"].as_str() {
        Some("browser.takeover_input") => {
            let Some(state) = app.try_state::<Takeovers>() else { return };
            let held = state.held.lock();
            if let (Some(h), Some(ops)) = (held.get(&tk), params["ops"].as_array()) {
                let _ = h.input.send(ops.clone());
            }
        }
        Some("browser.takeover_set") => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { set(&app, &tk, &params).await });
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
    if state.tk_of_pane(&label).is_some() {
        return Err("that pane is taken over already".into());
    }
    // A popup of the pane already open is what the phone sees first.
    let target = app.state::<Browsers>().popups_of(&label).last().cloned().unwrap_or_else(|| label.clone());
    let (input, rx) = tokio::sync::mpsc::unbounded_channel();
    let generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
    state.held.lock().insert(tk.clone(), Held { pane: label.clone(), target, banner: banner.chars().take(80).collect(), settings: Settings::from(&settings), paused: false, generation, token: None, input });
    tauri::async_runtime::spawn(inputs(app.clone(), tk.clone(), rx));
    if let Err(e) = stream_on(&app, &tk).await {
        state.held.lock().remove(&tk);
        return Err(e);
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

/// Ends takeover `tk` here: the stream stops, the interface takes its banner down. False when there was none.
async fn stop(app: &AppHandle, tk: &str, reason: &str) -> bool {
    let state = app.state::<Takeovers>();
    let Some(held) = state.held.lock().remove(tk) else { return false };
    stream_off(app, &held.target, held.token).await;
    retitle_popups(app, &held.pane);
    let _ = app.emit_to("main", "takeover-ended", json!({"tk": tk, "pane": held.pane, "reason": reason}));
    true
}

/// A pane's webview is closing: its takeover ends, and the daemon hears why.
pub(super) async fn pane_closed(app: &AppHandle, label: &str) {
    let tk = app.state::<Takeovers>().tk_of_pane(label);
    if let Some(tk) = tk {
        if stop(app, &tk, "closed").await {
            event(app, json!({"kind": "ended", "tk": tk, "reason": "closed"}));
        }
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

/// The group's top window changed (a popup opened or closed): the phone sees that one from now on.
pub(super) fn retarget(app: &AppHandle, pane: &str) {
    let Some(tk) = app.state::<Takeovers>().tk_of_pane(pane) else { return };
    let app = app.clone();
    let pane = pane.to_string();
    tauri::async_runtime::spawn(async move {
        let next = app.state::<Browsers>().popups_of(&pane).last().cloned().unwrap_or_else(|| pane.clone());
        let old = {
            let state = app.state::<Takeovers>();
            let mut held = state.held.lock();
            let Some(h) = held.get_mut(&tk) else { return };
            if h.target == next {
                return;
            }
            let old = (h.target.clone(), h.token.take());
            h.target = next;
            h.generation = state.generation.fetch_add(1, Ordering::Relaxed) + 1;
            old
        };
        stream_off(&app, &old.0, old.1).await;
        let _ = stream_on(&app, &tk).await;
        retitle_popups(&app, &pane);
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

/// What the daemon's ladder says now: a new rate or size restarts the stream (which also brings a fresh frame), a pause stops it.
async fn set(app: &AppHandle, tk: &str, params: &Value) {
    let (target, restart) = {
        let state = app.state::<Takeovers>();
        let mut held = state.held.lock();
        let Some(h) = held.get_mut(tk) else { return };
        let settings = Settings::from(params);
        let paused = params["paused"].as_bool() == Some(true);
        let restart = !paused && (settings != h.settings || h.paused || params["fresh"].as_bool() == Some(true));
        let was_paused = std::mem::replace(&mut h.paused, paused);
        h.settings = settings;
        (h.target.clone(), if paused && !was_paused { Some(false) } else if restart { Some(true) } else { None })
    };
    let Some(wv) = app.get_webview(&target) else { return };
    match restart {
        Some(false) => {
            let _ = cdp(&wv, "Page.stopScreencast", json!({})).await;
        }
        Some(true) => {
            let _ = cdp(&wv, "Page.stopScreencast", json!({})).await;
            let settings = app.state::<Takeovers>().held.lock().get(tk).map(|h| h.settings);
            if let Some(s) = settings {
                let _ = cdp(&wv, "Page.startScreencast", s.screencast()).await;
            }
        }
        None => {}
    }
}

/// Attaches the frame receiver to the takeover's target and starts its screencast.
async fn stream_on(app: &AppHandle, tk: &str) -> CmdResult<()> {
    let state = app.state::<Takeovers>();
    let (target, generation, settings) = {
        let held = state.held.lock();
        let h = held.get(tk).ok_or("that takeover has ended")?;
        (h.target.clone(), h.generation, h.settings)
    };
    let wv = app.get_webview(&target).ok_or_else(|| format!("no browser {target}"))?;
    let token = subscribe(&wv, frame_sink(app), tk.to_string(), generation).await?;
    let current = {
        let mut held = state.held.lock();
        match held.get_mut(tk).filter(|h| h.generation == generation) {
            Some(h) => {
                h.token = Some(token);
                true
            }
            None => false,
        }
    };
    if !current {
        unsubscribe(&wv, token);
        return Err("that takeover has ended".into());
    }
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
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tauri::async_runtime::spawn(frames(app.clone(), rx));
    *sink = Some(tx.clone());
    tx
}

/// Each frame: acknowledged to the page at once (the daemon keeps only the newest, so the page never waits on the phone), then handed to the daemon.
async fn frames(app: AppHandle, mut rx: tokio::sync::mpsc::UnboundedReceiver<(String, u64, String)>) {
    while let Some((tk, generation, json)) = rx.recv().await {
        let Some(target) = app.state::<Takeovers>().target(&tk, Some(generation)) else { continue };
        let Ok(mut v) = serde_json::from_str::<Value>(&json) else { continue };
        drop(json);
        if let Some(wv) = app.get_webview(&target) {
            ack(&wv, v["sessionId"].clone());
        }
        let mut meta = v.get_mut("metadata").map(Value::take).unwrap_or_else(|| json!({}));
        meta["window"] = json!(if target.contains("-popup-") { "popup" } else { "pane" });
        let data = v.get_mut("data").map(Value::take).unwrap_or(Value::Null);
        event(&app, json!({"kind": "frame", "tk": tk, "data": data, "meta": meta}));
    }
}

/// The phone's input for `tk`, in the order it came.
async fn inputs(app: AppHandle, tk: String, mut rx: tokio::sync::mpsc::UnboundedReceiver<Vec<Value>>) {
    let mut pressed = false;
    while let Some(ops) = rx.recv().await {
        let Some(target) = app.state::<Takeovers>().target(&tk, None) else { return };
        let Some(wv) = app.get_webview(&target) else { continue };
        for op in ops {
            // A page that refuses one event gets the next: what was refused is not said anywhere.
            let _ = dispatch(&wv, &op, &mut pressed).await;
        }
    }
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

/// One input event as the DevTools protocol takes it.
fn protocol_events(op: &Value, pressed: &mut bool) -> Vec<(&'static str, Value)> {
    let (x, y) = (op["x"].as_f64().unwrap_or(0.0), op["y"].as_f64().unwrap_or(0.0));
    let mouse = |kind: &str, button: &str, buttons: u8, clicks: u8| json!({"type": kind, "x": x, "y": y, "button": button, "buttons": buttons, "clickCount": clicks});
    const MOUSE: &str = "Input.dispatchMouseEvent";
    match op["k"].as_str() {
        Some("tap") => {
            *pressed = false;
            vec![(MOUSE, mouse("mouseMoved", "none", 0, 0)), (MOUSE, mouse("mousePressed", "left", 1, 1)), (MOUSE, mouse("mouseReleased", "left", 0, 1))]
        }
        Some("down") => {
            *pressed = true;
            vec![(MOUSE, mouse("mouseMoved", "none", 0, 0)), (MOUSE, mouse("mousePressed", "left", 1, 1))]
        }
        Some("move") => vec![(MOUSE, mouse("mouseMoved", if *pressed { "left" } else { "none" }, u8::from(*pressed), 0))],
        Some("up") => {
            *pressed = false;
            vec![(MOUSE, mouse("mouseReleased", "left", 0, 1))]
        }
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

async fn dispatch(wv: &tauri::Webview, op: &Value, pressed: &mut bool) -> CmdResult<()> {
    for (method, params) in protocol_events(op, pressed) {
        cdp(wv, method, params).await?;
    }
    Ok(())
}

async fn insert_text(app: &AppHandle, tk: &str, text: &str) -> Result<(), String> {
    let target = app.state::<Takeovers>().target(tk, None).ok_or("that takeover has ended")?;
    let wv = app.get_webview(&target).ok_or("the page is gone")?;
    cdp(&wv, "Input.insertText", json!({"text": text})).await.map(|_| ())
}

#[cfg(windows)]
async fn cdp(wv: &tauri::Webview, method: &'static str, params: Value) -> CmdResult<Value> {
    super::devtools(wv, method, params).await
}

#[cfg(not(windows))]
async fn cdp(_wv: &tauri::Webview, _method: &'static str, _params: Value) -> CmdResult<Value> {
    Err("taking over a pane is not available on this platform yet".into())
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
        let mut pressed = false;
        let tap = protocol_events(&json!({"k": "tap", "x": 10.5, "y": 20}), &mut pressed);
        let kinds: Vec<&str> = tap.iter().map(|(_, p)| p["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["mouseMoved", "mousePressed", "mouseReleased"]);
        assert_eq!((tap[1].1["x"].as_f64(), tap[1].1["button"].as_str(), tap[1].1["clickCount"].as_u64()), (Some(10.5), Some("left"), Some(1)));
        protocol_events(&json!({"k": "down", "x": 1, "y": 1}), &mut pressed);
        let moved = protocol_events(&json!({"k": "move", "x": 5, "y": 5}), &mut pressed);
        assert_eq!((moved[0].1["button"].as_str(), moved[0].1["buttons"].as_u64()), (Some("left"), Some(1)));
        protocol_events(&json!({"k": "up", "x": 5, "y": 5}), &mut pressed);
        assert!(!pressed);
        let wheel = protocol_events(&json!({"k": "wheel", "x": 1, "y": 2, "dx": 0, "dy": 120}), &mut pressed);
        assert_eq!((wheel[0].1["type"].as_str(), wheel[0].1["deltaY"].as_f64()), (Some("mouseWheel"), Some(120.0)));
    }

    #[test]
    fn keys_carry_their_codes_and_only_known_ones_go() {
        let mut pressed = false;
        let enter = protocol_events(&json!({"k": "key", "key": "Enter"}), &mut pressed);
        assert_eq!((enter[0].0, enter[0].1["type"].as_str(), enter[0].1["text"].as_str(), enter[0].1["windowsVirtualKeyCode"].as_u64()), ("Input.dispatchKeyEvent", Some("keyDown"), Some("\r"), Some(13)));
        assert_eq!(enter[1].1["type"], "keyUp");
        let tab = protocol_events(&json!({"k": "key", "key": "Tab"}), &mut pressed);
        assert_eq!(tab[0].1["type"], "rawKeyDown", "a key with no text");
        assert!(protocol_events(&json!({"k": "key", "key": "F12"}), &mut pressed).is_empty());
        assert!(protocol_events(&json!({"k": "eval", "x": 1, "y": 1}), &mut pressed).is_empty());
    }
}
