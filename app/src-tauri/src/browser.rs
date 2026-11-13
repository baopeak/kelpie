//! Browser panes: a child webview of the main window per pane, placed over the pane's DOM
//! area by the UI (`browser_bounds`). Pages get no IPC (the app capability covers the main
//! webview only); what Kelpie needs from them comes through `browser_picker.js`, whose reports
//! arrive as cancelled `window.open` calls of a sentinel URL carrying a per-page nonce.
//!
//! Events to the UI (`browser-event`): `{label, kind: "nav" | "load" | "title" | "pick" |
//! "cancel" | "popup", url?, title?, data?}`.
//!
//! A `window.open` that asks for a size (an OAuth or sign-in window) opens as a window of its own on the pane's WebView2 environment, so the page keeps
//! `window.opener` and gets the sign-in back; any other new window opens in the pane. A pane and its popups are one group for a phone that takes the
//! pane over (`takeover`).
//!
//! A pane opened for an agent while browser control is on is made private (`incognito`: InPrivate on Windows, a non-persistent data store on
//! macOS, an ephemeral context on Linux), and so are its popups: agents do not get the person's sign-ins, and what they sign in to is
//! forgotten when their panes close. On Windows all such panes share one private session; elsewhere each pane has its own.

pub(crate) mod agent;
mod agent_cdp;
mod agent_snapshot;
mod native_shot;
pub(crate) mod takeover;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::webview::{NewWindowFeatures, NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::fs_ops::{err, CmdResult};

const PICKER: &str = include_str!("browser_picker.js");
/// The engine agents act through (`agent`).
const AGENT_ENGINE: &str = include_str!("browser_agent.js");
const SENTINEL_HOST: &str = "kelpie-pick.invalid";

// The commands are registered by their own path (`browser::takeover::…`): `generate_handler!` needs the macros `#[tauri::command]` puts beside them.
pub use agent::AgentPanes;
pub use takeover::Takeovers;

/// Nonce of each browser webview (by label); reports carrying another one are ignored. The popups each pane opened, oldest first, and their pages' titles.
#[derive(Default)]
pub struct Browsers {
    nonces: Mutex<HashMap<String, String>>,
    popups: Mutex<HashMap<String, Vec<String>>>,
    popup_titles: Mutex<HashMap<String, String>>,
    next_popup: AtomicU64,
    /// Main-frame navigations of each pane: started, and finished by then (`agent` waits on them).
    navs: Mutex<HashMap<String, (u64, u64)>>,
    /// The nonce each pane's agent engine answers to (`browser_agent.js`): the page cannot call it.
    agent_nonces: Mutex<HashMap<String, String>>,
    /// Panes whose webview is private (opened for an agent while browser control was on), as the platform made it: none of the person's
    /// sign-ins, and forgotten when it closes. On Windows these panes and their popups share one private session (forgotten once the last of
    /// them closes); on macOS and Linux each pane has its own, its sized popups included. Fixed for the webview's life.
    private: Mutex<HashSet<String>>,
    /// Held while `browser_open` looks for a pane's webview and makes it: two opens of one label (the window's order does not hold across a
    /// reload of the window) never both make it, nor overwrite the nonces and the record of the one that exists.
    opening: tokio::sync::Mutex<()>,
}

impl Browsers {
    /// Whether pane `label`'s webview was made private.
    fn is_private(&self, label: &str) -> bool {
        self.private.lock().unwrap().contains(label)
    }

    fn set_private(&self, label: &str, private: bool) {
        let mut set = self.private.lock().unwrap();
        if private {
            set.insert(label.to_string());
        } else {
            set.remove(label);
        }
    }

    /// Pane `label`'s webview is closing: its page's reports and agent calls are no longer taken. Whether it is private is kept until it
    /// is gone (`set_private`): a popup it opens meanwhile still gets its session.
    fn forget(&self, label: &str) {
        self.nonces.lock().unwrap().remove(label);
        self.navs.lock().unwrap().remove(label);
        self.agent_nonces.lock().unwrap().remove(label);
    }

    fn popups_of(&self, pane: &str) -> Vec<String> {
        self.popups.lock().unwrap().get(pane).cloned().unwrap_or_default()
    }

    fn popup_page_title(&self, popup: &str) -> String {
        self.popup_titles.lock().unwrap().get(popup).cloned().unwrap_or_default()
    }

    fn agent_nonce(&self, pane: &str) -> Option<String> {
        self.agent_nonces.lock().unwrap().get(pane).cloned()
    }

    fn navs(&self, pane: &str) -> (u64, u64) {
        self.navs.lock().unwrap().get(pane).copied().unwrap_or_default()
    }

    fn nav_started(&self, pane: &str) {
        self.navs.lock().unwrap().entry(pane.to_string()).or_default().0 += 1;
    }

    fn nav_finished(&self, pane: &str) {
        let mut navs = self.navs.lock().unwrap();
        let n = navs.entry(pane.to_string()).or_default();
        n.1 = n.0;
    }

    fn forget_popup(&self, pane: &str, popup: &str) {
        if let Some(list) = self.popups.lock().unwrap().get_mut(pane) {
            list.retain(|p| p != popup);
        }
        self.popup_titles.lock().unwrap().remove(popup);
    }
}

/// A request or a notice of the daemon for this window: an agent's browser tool (`agent`), else a phone's takeover (`takeover`).
pub fn provider_message(app: &AppHandle, msg: Value) {
    if let (Some(preq), Some("browser.agent")) = (msg["preq"].as_u64(), msg["method"].as_str()) {
        agent::on_request(app, preq, msg["params"].clone());
        return;
    }
    takeover::on_message(app, msg);
}

/// The daemon's connection dropped.
pub fn daemon_gone(app: &AppHandle) {
    agent::daemon_gone(app);
    takeover::daemon_gone(app);
}

/// A sized `window.open` of pane `pane` as a window of its own, on the pane's WebView2 environment (`window_features` carries it), private when
/// the pane is, so that a sign-in there reaches the pane's own session. Like the pane it has no IPC (the app's capability covers the main
/// webview only) and stays on the web; it opens nothing further. None when it could not be made: the pane opens it instead.
fn open_popup(app: &AppHandle, pane: &str, url: &Url, features: NewWindowFeatures) -> Option<tauri::WebviewWindow> {
    let state = app.state::<Browsers>();
    let label = format!("{pane}-popup-{}", state.next_popup.fetch_add(1, Ordering::Relaxed) + 1);
    valid_label(&label).ok()?;
    let (title_app, title_label, title_pane) = (app.clone(), label.clone(), pane.to_string());
    let window = tauri::WebviewWindowBuilder::new(app, &label, WebviewUrl::External(Url::parse("about:blank").ok()?))
        .window_features(features)
        .incognito(state.is_private(pane))
        .title(url.host_str().unwrap_or_default())
        .on_navigation(|u| u.host_str() != Some(SENTINEL_HOST) && matches!(u.scheme(), "http" | "https" | "about" | "data" | "blob"))
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_page_load({
            let (app, pane) = (app.clone(), pane.to_string());
            move |w, payload| {
                if matches!(payload.event(), PageLoadEvent::Finished) {
                    takeover::popup_loaded(&app, &pane, w.label());
                }
            }
        })
        .on_document_title_changed(move |w, title| {
            title_app.state::<Browsers>().popup_titles.lock().unwrap().insert(title_label.clone(), title.clone());
            let _ = w.set_title(&title_app.state::<Takeovers>().popup_title(&title_pane, &title));
        })
        .build()
        .ok()?;
    // `window.close()` in the page: wry destroys only the page's own view and leaves the window; the window goes with it here, so the group and the
    // phone go back to the pane.
    #[cfg(windows)]
    {
        let (close_app, close_label) = (app.clone(), label.clone());
        let _ = window.with_webview(move |pv| unsafe {
            if let Ok(core) = pv.controller().CoreWebView2() {
                let mut token = 0i64;
                let handler = webview2_com::WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                    if let Some(w) = close_app.get_webview_window(&close_label) {
                        let _ = w.close();
                    }
                    Ok(())
                }));
                let _ = core.add_WindowCloseRequested(&handler, &mut token);
            }
        });
    }
    state.popups.lock().unwrap().entry(pane.to_string()).or_default().push(label.clone());
    let (gone_app, gone_pane) = (app.clone(), pane.to_string());
    window.on_window_event(move |e| {
        if let tauri::WindowEvent::Destroyed = e {
            gone_app.state::<Browsers>().forget_popup(&gone_pane, &label);
            takeover::retarget(&gone_app, &gone_pane);
        }
    });
    takeover::retarget(app, pane);
    Some(window)
}

#[derive(Serialize, Clone)]
struct BrowserEvent {
    label: String,
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

fn emit(app: &AppHandle, label: &str, kind: &'static str, url: Option<String>, title: Option<String>, data: Option<Value>) {
    let _ = app.emit_to("main", "browser-event", BrowserEvent { label: label.to_string(), kind, url, title, data });
}

fn valid_label(label: &str) -> CmdResult<()> {
    if label.starts_with("browser-") && label.len() <= 64 && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(())
    } else {
        Err("invalid browser label".into())
    }
}

/// http(s) only (plus about:blank); a bare host gets https://, localhost http://.
pub fn parse_url(input: &str) -> CmdResult<Url> {
    let s = input.trim();
    if s.is_empty() || s == "about:blank" {
        return Url::parse("about:blank").map_err(err);
    }
    let with_scheme = if s.contains("://") {
        s.to_string()
    } else if s.starts_with("localhost") || s.starts_with("127.0.0.1") || s.starts_with("[::1]") || s.starts_with("0.0.0.0") {
        format!("http://{s}")
    } else {
        format!("https://{s}")
    };
    let url = Url::parse(&with_scheme).map_err(|e| format!("not a URL: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("only http and https pages open in the browser pane".into());
    }
    Ok(url)
}

/// The report inside a sentinel URL, if its nonce matches.
fn decode_report(url: &Url, nonce: &str) -> Option<(String, Value)> {
    if url.host_str() != Some(SENTINEL_HOST) {
        return None;
    }
    let frag = url.fragment()?;
    let (got, body) = frag.split_once('.')?;
    if got != nonce {
        return None;
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body).ok()?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    Some((v["kind"].as_str()?.to_string(), v["data"].clone()))
}

fn new_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    // Two independently seeded SipHashes of the time: unguessable for a page, no extra crate.
    let mut out = String::new();
    for _ in 0..2 {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

#[derive(serde::Deserialize, Clone, Copy)]
pub struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Creates the pane's webview (or moves and shows the existing one). `hidden`: for an agent that uses a pane nobody has opened yet, the
/// webview is made at `rect` (off screen) and hidden at once; an existing one is left exactly as it is (the pane may be mounting). `agent`:
/// the pane was opened for an agent while browser control is on, so its page is private (a private session, none of the person's
/// sign-ins) and keeps its console for agents from the start; fixed when the webview is made, so an existing webview keeps what it was made
/// with. Returns whether the pane's webview is private, as the platform made it.
#[tauri::command]
pub async fn browser_open(app: AppHandle, state: tauri::State<'_, Browsers>, label: String, url: String, rect: Rect, hidden: Option<bool>, agent: Option<bool>) -> CmdResult<bool> {
    valid_label(&label)?;
    let hidden = hidden == Some(true);
    let _opening = state.opening.lock().await;
    if let Some(wv) = app.get_webview(&label) {
        if hidden {
            return Ok(state.is_private(&label));
        }
        wv.set_position(LogicalPosition::new(rect.x, rect.y)).map_err(err)?;
        wv.set_size(LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0))).map_err(err)?;
        app.state::<Takeovers>().set_visible(&label, true);
        wv.show().map_err(err)?;
        crate::webview_memory::fit(&wv);
        return Ok(state.is_private(&label));
    }
    let url = parse_url(&url)?;
    let window = app.get_window("main").ok_or("no main window")?;
    let asked = agent == Some(true);
    let nonce = new_nonce();
    state.nonces.lock().unwrap().insert(label.clone(), nonce.clone());
    let agent_nonce = new_nonce();
    state.agent_nonces.lock().unwrap().insert(label.clone(), agent_nonce.clone());
    // Recorded before the page exists: its first sized `window.open` already asks for it. Corrected below to what the platform made.
    state.set_private(&label, asked);
    let engine = AGENT_ENGINE.replace("__KELPIE_AGENT_NONCE__", &agent_nonce).replace("__KELPIE_AGENT_PANE__", if asked { "true" } else { "false" });
    let (nav_app, load_app, title_app, popup_app) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (nav_label, load_label, title_label, popup_label) = (label.clone(), label.clone(), label.clone(), label.clone());
    let popup_nonce = nonce.clone();
    let builder = WebviewBuilder::new(&label, WebviewUrl::External(url))
        .incognito(asked)
        .initialization_script(PICKER.replace("__KELPIE_NONCE__", &nonce))
        .initialization_script(engine)
        .on_navigation(move |u| {
            if u.host_str() == Some(SENTINEL_HOST) {
                return false;
            }
            emit(&nav_app, &nav_label, "nav", Some(u.to_string()), None, None);
            // Pages stay on the web: no file:, javascript: or app schemes.
            let allowed = matches!(u.scheme(), "http" | "https" | "about" | "data" | "blob");
            // WebView2 asks here for the main frame only; WebKit asks for frames too, so there the start of a load counts instead.
            #[cfg(windows)]
            if allowed {
                nav_app.state::<Browsers>().nav_started(&nav_label);
                agent::navigating(&nav_app, &nav_label, u.as_str());
            }
            allowed
        })
        .on_page_load(move |_wv, payload| {
            #[cfg(not(windows))]
            if matches!(payload.event(), PageLoadEvent::Started) {
                load_app.state::<Browsers>().nav_started(&load_label);
            }
            if matches!(payload.event(), PageLoadEvent::Finished) {
                load_app.state::<Browsers>().nav_finished(&load_label);
                emit(&load_app, &load_label, "load", Some(payload.url().to_string()), None, None);
            }
        })
        .on_document_title_changed(move |_, title| emit(&title_app, &title_label, "title", None, Some(title), None))
        .on_new_window(move |u, features| {
            match decode_report(&u, &popup_nonce) {
                Some((kind, data)) => emit(&popup_app, &popup_label, if kind == "pick" { "pick" } else { "cancel" }, None, None, Some(data)),
                None if matches!(u.scheme(), "http" | "https") && u.host_str() != Some(SENTINEL_HOST) => {
                    // A window with a size is a sign-in window: its own, beside the pane. Links that open a new window open in this pane instead.
                    if features.size().is_some() || features.position().is_some() {
                        if let Some(window) = open_popup(&popup_app, &popup_label, &u, features) {
                            return NewWindowResponse::Create { window };
                        }
                    }
                    emit(&popup_app, &popup_label, "popup", Some(u.to_string()), None, None)
                }
                None => {}
            }
            NewWindowResponse::Deny
        });
    let wv = match window.add_child(builder, LogicalPosition::new(rect.x, rect.y), LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0))) {
        Ok(wv) => wv,
        Err(e) => {
            state.forget(&label);
            state.set_private(&label, false);
            return Err(err(e));
        }
    };
    // What the platform made counts, not what was asked: an older WebView2 makes a normal page without a word, and the agents' site rule
    // must not widen for a page that has the person's sign-ins. Recorded whatever was recorded before.
    let private = asked && recorded_private(made_private(&wv).await);
    state.set_private(&label, private);
    if hidden {
        app.state::<Takeovers>().set_visible(&label, false);
        wv.hide().map_err(err)?;
    }
    crate::webview_memory::fit(&wv);
    Ok(private)
}

/// Whether a page asked to be private is recorded as private: only when the platform says it is. Unknown counts as not private.
fn recorded_private(made: Option<bool>) -> bool {
    made == Some(true)
}

/// Whether a webview asked to be private is: on Windows its WebView2 profile says (`IsInPrivateModeEnabled`), None when that cannot be read
/// (a runtime without profiles, or no answer within 5 s).
#[cfg(windows)]
async fn made_private(wv: &tauri::Webview) -> Option<bool> {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_13;
    use windows::core::Interface;
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<bool>>();
    let tx = Mutex::new(Some(tx));
    wv.with_webview(move |pv| unsafe {
        let read = || -> windows::core::Result<bool> {
            let core = pv.controller().CoreWebView2()?.cast::<ICoreWebView2_13>()?;
            let mut on = windows::core::BOOL::default();
            core.Profile()?.IsInPrivateModeEnabled(&mut on)?;
            Ok(on.as_bool())
        };
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(read().ok());
        }
    })
    .ok()?;
    tokio::time::timeout(std::time::Duration::from_secs(5), rx).await.ok()?.ok()?
}

/// Elsewhere wry makes what was asked (a non-persistent data store on macOS, an ephemeral context on Linux), and nothing reads it back.
#[cfg(not(windows))]
async fn made_private(_wv: &tauri::Webview) -> Option<bool> {
    Some(true)
}

/// Which of `labels` have a webview now, and whether it is private (made for an agent, as the platform made it). A label left out has no
/// webview yet.
#[tauri::command]
pub async fn browser_private(app: AppHandle, state: tauri::State<'_, Browsers>, labels: Vec<String>) -> CmdResult<HashMap<String, bool>> {
    Ok(labels.into_iter().filter(|l| valid_label(l).is_ok() && app.get_webview(l).is_some()).map(|l| (l.clone(), state.is_private(&l))).collect())
}

fn webview(app: &AppHandle, label: &str) -> CmdResult<tauri::Webview> {
    valid_label(label)?;
    app.get_webview(label).ok_or_else(|| format!("no browser {label}"))
}

#[tauri::command]
pub async fn browser_bounds(app: AppHandle, label: String, rect: Rect) -> CmdResult<()> {
    let wv = webview(&app, &label)?;
    wv.set_position(LogicalPosition::new(rect.x, rect.y)).map_err(err)?;
    wv.set_size(LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0))).map_err(err)
}

#[tauri::command]
pub async fn browser_visible(app: AppHandle, label: String, visible: bool) -> CmdResult<()> {
    let Some(wv) = app.get_webview(&label) else { return Ok(()) };
    if visible {
        app.state::<Takeovers>().set_visible(&label, true);
        wv.show().map_err(err)?;
    } else {
        takeover::hiding(&app, &label).await;
        wv.hide().map_err(err)?;
    }
    // A hidden pane keeps less memory; posted after the show/hide, so it sees the new state.
    crate::webview_memory::fit(&wv);
    Ok(())
}

#[tauri::command]
pub async fn browser_navigate(app: AppHandle, label: String, url: String) -> CmdResult<String> {
    let wv = webview(&app, &label)?;
    let url = parse_url(&url)?;
    let s = url.to_string();
    wv.navigate(url).map_err(err)?;
    Ok(s)
}

/// back (-1), forward (1), reload (0).
#[tauri::command]
pub async fn browser_go(app: AppHandle, label: String, delta: i32) -> CmdResult<()> {
    let wv = webview(&app, &label)?;
    if delta == 0 { wv.reload() } else { wv.eval(format!("history.go({})", delta.clamp(-1, 1))) }.map_err(err)
}

#[tauri::command]
pub async fn browser_design(app: AppHandle, label: String, on: bool) -> CmdResult<()> {
    let wv = webview(&app, &label)?;
    wv.eval(format!("window.__kelpieDesign && window.__kelpieDesign.set({on})")).map_err(err)
}

#[tauri::command]
pub async fn browser_devtools(app: AppHandle, label: String) -> CmdResult<()> {
    let wv = webview(&app, &label)?;
    if wv.is_devtools_open() { wv.close_devtools() } else { wv.open_devtools() };
    Ok(())
}

/// Where the webview is (logical px of the window) and what it shows.
#[tauri::command]
pub async fn browser_info(app: AppHandle, label: String) -> CmdResult<Value> {
    let wv = webview(&app, &label)?;
    let scale = app.get_window("main").and_then(|w| w.scale_factor().ok()).unwrap_or(1.0);
    let pos = wv.position().map_err(err)?.to_logical::<f64>(scale);
    let size = wv.size().map_err(err)?.to_logical::<f64>(scale);
    Ok(json!({"x": pos.x, "y": pos.y, "width": size.width, "height": size.height, "url": wv.url().map(|u| u.to_string()).unwrap_or_default()}))
}

#[tauri::command]
pub async fn browser_close(app: AppHandle, state: tauri::State<'_, Browsers>, label: String) -> CmdResult<()> {
    state.forget(&label);
    agent::pane_closed(&app, &label);
    // A takeover of the pane ends, and its popups go with it.
    takeover::pane_closed(&app, &label).await;
    let popups = state.popups.lock().unwrap().remove(&label).unwrap_or_default();
    for popup in popups {
        if let Some(w) = app.get_webview_window(&popup) {
            let _ = w.close();
        }
    }
    let closed = match app.get_webview(&label) {
        Some(wv) => wv.close().map_err(err),
        None => Ok(()),
    };
    state.set_private(&label, false);
    closed
}

/// What a local port serves when it serves a web page.
#[derive(Serialize, Debug, PartialEq)]
pub struct WebPage {
    title: Option<String>,
}

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
const PROBE_MAX_BYTES: u64 = 64 * 1024;

/// The text of the first `<title>` of a page, on one line.
fn page_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = html[start..end].split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then(|| title.chars().take(80).collect())
}

/// A page answered with HTML, or a redirect (usually to a login or a start page). Anything else on the port (a 404, a
/// JSON API, a program that does not speak HTTP) is not something to show a person.
fn probe_page(host: &str, port: u16) -> Option<WebPage> {
    use std::io::Read as _;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(PROBE_TIMEOUT)).max_redirects(0).http_status_as_error(false).proxy(None).build().into();
    let mut resp = agent.get(format!("http://{host}:{port}/")).header("Accept", "text/html").call().ok()?;
    let status = resp.status().as_u16();
    if (300..400).contains(&status) {
        return resp.headers().contains_key("location").then_some(WebPage { title: None });
    }
    let html = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|v| v.to_ascii_lowercase().starts_with("text/html"));
    if !(200..300).contains(&status) || !html {
        return None;
    }
    let mut body = Vec::new();
    let _ = resp.body_mut().as_reader().take(PROBE_MAX_BYTES).read_to_end(&mut body);
    Some(WebPage { title: page_title(&String::from_utf8_lossy(&body)) })
}

/// Whether a port on this computer serves a web page; other hosts are never asked.
#[tauri::command]
pub async fn browser_probe(host: String, port: u16) -> CmdResult<Option<WebPage>> {
    if !matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return Ok(None);
    }
    tauri::async_runtime::spawn_blocking(move || probe_page(&host, port)).await.map_err(err)
}

/// A PNG of the page (or of `clip`, in CSS pixels of the visible page), saved under the data
/// dir so an agent can open it. Returns `{path, data}` (`data` = base64 PNG).
#[tauri::command]
pub async fn browser_screenshot(app: AppHandle, label: String, clip: Option<Rect>) -> CmdResult<Value> {
    let wv = webview(&app, &label)?;
    let png = capture(&wv, clip).await?;
    let path = save_shot(None, &png, "png")?;
    Ok(json!({"path": path.to_string_lossy(), "data": base64::engine::general_purpose::STANDARD.encode(&png)}))
}

/// Saves a screenshot (`ext` png or jpg) under `<data>/browser-shots`, or its subfolder `sub` (agents' own, pruned apart from the person's),
/// where agents can open it; its path.
pub(crate) fn save_shot(sub: Option<&str>, bytes: &[u8], ext: &str) -> CmdResult<std::path::PathBuf> {
    let dir = kelpie_proto::data_dir().join("browser-shots");
    let dir = match sub {
        Some(sub) => dir.join(sub),
        None => dir,
    };
    std::fs::create_dir_all(&dir).map_err(err)?;
    prune_shots(&dir);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let path = dir.join(format!("{stamp}.{ext}"));
    std::fs::write(&path, bytes).map_err(err)?;
    Ok(path)
}

/// `Page.captureScreenshot`'s `clip`: a region in CSS pixels of the document, drawn at `scale`.
pub(crate) fn clip(x: f64, y: f64, width: f64, height: f64, scale: f64) -> Value {
    json!({"x": x.max(0.0), "y": y.max(0.0), "width": width.max(1.0), "height": height.max(1.0), "scale": scale})
}

/// Keeps the newest 200 screenshots.
fn prune_shots(dir: &std::path::Path) {
    let mut files: Vec<_> =
        std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png" || x == "jpg")).collect();
    if files.len() <= 200 {
        return;
    }
    files.sort();
    for f in &files[..files.len() - 200] {
        let _ = std::fs::remove_file(f);
    }
}

/// The page through the DevTools protocol, only while the pane is shown (as on macOS and Linux).
#[cfg(windows)]
async fn capture(wv: &tauri::Webview, clip: Option<Rect>) -> CmdResult<Vec<u8>> {
    let (app, label) = (wv.app_handle().clone(), wv.label().to_string());
    let hidden: Box<dyn FnOnce() -> bool + Send> = Box::new(move || app.state::<Takeovers>().hidden(&label));
    let mut params = json!({"format": "png"});
    if let Some(r) = clip {
        // Page.captureScreenshot clips in document coordinates; the picker reports the
        // viewport box, so the page's scroll offset is added by the caller's rect origin.
        params["clip"] = self::clip(r.x, r.y, r.width, r.height, 1.0);
        params["captureBeyondViewport"] = json!(true);
    }
    let v = devtools_when(wv, "Page.captureScreenshot", params, Some(hidden)).await?;
    base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().ok_or("no screenshot data")?).map_err(err)
}

/// Calls a DevTools protocol method on the page's WebView2 and returns its JSON result.
#[cfg(windows)]
pub(crate) async fn devtools(wv: &tauri::Webview, method: &'static str, params: Value) -> CmdResult<Value> {
    devtools_when(wv, method, params, None).await
}

/// What a call made only while the page is shown answers when it is not.
pub(crate) const NOT_SHOWN: &str = "the pane is hidden on the computer";

/// Whether the view is shown now: not hidden (`SetIsVisible`) and its window not minimised. On the window thread.
#[cfg(windows)]
pub(crate) unsafe fn shown(controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, IsIconic, GA_ROOT};
    let mut visible = windows::core::BOOL::default();
    if controller.IsVisible(&mut visible).is_err() || !visible.as_bool() {
        return false;
    }
    let mut parent = windows::Win32::Foundation::HWND::default();
    if controller.ParentWindow(&mut parent).is_err() {
        return false;
    }
    !IsIconic(GetAncestor(parent, GA_ROOT)).as_bool()
}

/// `devtools`, but with `hidden` (a record of what the window hides) given, only while the page is shown: that, the view's own visibility and its
/// window's minimised state are looked at on the window thread right before the call, so a hide posted after the look cannot overtake the call.
/// Refused, it answers `NOT_SHOWN`.
#[cfg(windows)]
pub(crate) async fn devtools_when(wv: &tauri::Webview, method: &'static str, params: Value, hidden: Option<Box<dyn FnOnce() -> bool + Send>>) -> CmdResult<Value> {
    use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
    use windows::core::HSTRING;
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String, String>>();
    let tx = std::sync::Mutex::new(Some(tx));
    let params = params.to_string();
    wv.with_webview(move |pv| unsafe {
        let send = move |r: Result<String, String>| {
            if let Some(tx) = tx.lock().unwrap().take() {
                let _ = tx.send(r);
            }
        };
        if let Some(hidden) = hidden {
            if hidden() || !shown(&pv.controller()) {
                return send(Err(NOT_SHOWN.into()));
            }
        }
        let core = match pv.controller().CoreWebView2() {
            Ok(c) => c,
            Err(e) => return send(Err(e.to_string())),
        };
        let send = std::sync::Arc::new(send);
        let done = send.clone();
        let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |res, json| {
            done(res.map(|_| json).map_err(|e| e.to_string()));
            Ok(())
        }));
        if let Err(e) = core.CallDevToolsProtocolMethod(&HSTRING::from(method), &HSTRING::from(params.as_str()), &handler) {
            send(Err(e.to_string()));
        }
    })
    .map_err(err)?;
    let json = tokio::time::timeout(std::time::Duration::from_secs(15), rx).await.map_err(|_| format!("{method} timed out"))?.map_err(|_| format!("{method} cancelled"))??;
    serde_json::from_str(&json).map_err(err)
}

/// Hands each DevTools protocol `event` of the page to `on` (its parameters as JSON, on the window's thread); the registration's token.
#[cfg(windows)]
pub(crate) async fn devtools_subscribe(wv: &tauri::Webview, event: &'static str, on: impl Fn(String) + Send + 'static) -> CmdResult<i64> {
    use webview2_com::DevToolsProtocolEventReceivedEventHandler;
    use windows::core::{HSTRING, PWSTR};
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<i64, String>>();
    wv.with_webview(move |pv| unsafe {
        let attached = (|| -> windows::core::Result<i64> {
            let core = pv.controller().CoreWebView2()?;
            let receiver = core.GetDevToolsProtocolEventReceiver(&HSTRING::from(event))?;
            let handler = DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    let mut json = PWSTR::null();
                    if args.ParameterObjectAsJson(&mut json).is_ok() {
                        on(webview2_com::take_pwstr(json));
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
    .map_err(err)?;
    tokio::time::timeout(std::time::Duration::from_secs(5), rx).await.map_err(|_| "the page did not answer".to_string())?.map_err(|_| "cancelled".to_string())?
}

/// Ends a `devtools_subscribe` registration.
#[cfg(windows)]
pub(crate) fn devtools_unsubscribe(wv: &tauri::Webview, event: &'static str, token: i64) {
    use windows::core::HSTRING;
    let _ = wv.with_webview(move |pv| unsafe {
        if let Ok(core) = pv.controller().CoreWebView2() {
            if let Ok(receiver) = core.GetDevToolsProtocolEventReceiver(&HSTRING::from(event)) {
                let _ = receiver.remove_DevToolsProtocolEventReceived(token);
            }
        }
    });
}

/// End-to-end tests only (dev builds or KELPIE_E2E): trusted mouse / key input into the page
/// through the DevTools protocol, which works whether or not the window is on screen —
/// unlike synthetic OS input, which needs a visible, focused window on an idle desktop.
/// `events`: `[{"method": "Input.dispatchMouseEvent" | "Input.dispatchKeyEvent", "params": {…}}]`.
#[tauri::command]
pub async fn browser_test_input(app: AppHandle, label: String, events: Vec<Value>) -> CmdResult<()> {
    if !(cfg!(debug_assertions) || std::env::var_os("KELPIE_E2E").is_some()) {
        return Err("only available to the end-to-end tests".into());
    }
    let wv = webview(&app, &label)?;
    for e in events {
        let method = match e["method"].as_str() {
            Some("Input.dispatchMouseEvent") => "Input.dispatchMouseEvent",
            Some("Input.dispatchKeyEvent") => "Input.dispatchKeyEvent",
            _ => return Err("only Input.dispatchMouseEvent / Input.dispatchKeyEvent".into()),
        };
        test_input(&wv, method, e["params"].clone()).await?;
    }
    Ok(())
}

#[cfg(windows)]
async fn test_input(wv: &tauri::Webview, method: &'static str, params: Value) -> CmdResult<Value> {
    devtools(wv, method, params).await
}

#[cfg(not(windows))]
async fn test_input(_wv: &tauri::Webview, _method: &'static str, _params: Value) -> CmdResult<Value> {
    Err("not available on this platform yet".into())
}

/// Whether panes can be pictured on this platform: through the DevTools protocol on Windows, WebKit's snapshots on macOS and Linux.
pub(crate) const SCREENSHOTS: bool = cfg!(windows) || native_shot::AVAILABLE;

/// WebKit's snapshot of the page (`native_shot`), only while the pane is shown; a `clip` outside the visible page comes from the whole
/// document where WebKit pictures it (Linux), else is cut to the visible page (macOS).
#[cfg(not(windows))]
async fn capture(wv: &tauri::Webview, clip: Option<Rect>) -> CmdResult<Vec<u8>> {
    let (app, label) = (wv.app_handle().clone(), wv.label().to_string());
    let hidden: native_shot::Hidden = Box::new(move || app.state::<Takeovers>().hidden(&label));
    native_shot::take(wv, clip.map(|r| [r.x, r.y, r.width, r.height]), 1.0, None, Some(hidden), native_shot::TIMEOUT).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(parse_url("localhost:5173").unwrap().as_str(), "http://localhost:5173/");
        assert_eq!(parse_url("example.com/a").unwrap().as_str(), "https://example.com/a");
        assert!(parse_url("file:///C:/x").is_err());
        assert!(parse_url("javascript:alert(1)").is_err());
        assert_eq!(parse_url("").unwrap().as_str(), "about:blank");
    }

    #[test]
    fn a_page_counts_as_private_only_when_the_platform_says_so() {
        assert!(recorded_private(Some(true)));
        // A runtime that made a normal page, or one that could not be asked: the page has the person's sign-ins.
        assert!(!recorded_private(Some(false)));
        assert!(!recorded_private(None));
    }

    #[test]
    fn the_private_record_follows_the_webview() {
        let b = Browsers::default();
        b.set_private("browser-a", true);
        assert!(b.is_private("browser-a") && !b.is_private("browser-b"));
        // Closing forgets the page's reports and agent calls first, and its privacy only once it is gone.
        b.forget("browser-a");
        assert!(b.is_private("browser-a"));
        b.set_private("browser-a", false);
        assert!(!b.is_private("browser-a"));
    }

    #[test]
    fn reports_need_the_nonce() {
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r##"{"kind":"pick","data":{"selector":"#a"}}"##);
        let u = Url::parse(&format!("http://kelpie-pick.invalid/#n1.{body}")).unwrap();
        let (kind, data) = decode_report(&u, "n1").unwrap();
        assert_eq!((kind.as_str(), data["selector"].as_str()), ("pick", Some("#a")));
        assert!(decode_report(&u, "other").is_none());
        assert!(decode_report(&Url::parse("http://example.com/#n1.x").unwrap(), "n1").is_none());
        assert_ne!(new_nonce(), new_nonce());
    }

    /// A one-shot server on a free local port that answers with `response`.
    fn serve(response: &'static str) -> u16 {
        use std::io::{Read as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut c, _)) = listener.accept() {
                let mut buf = [0u8; 2048];
                let _ = c.read(&mut buf);
                let _ = c.write_all(response.as_bytes());
            }
        });
        port
    }

    #[test]
    fn a_page_with_html_counts_and_brings_its_title() {
        let port = serve("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 44\r\nConnection: close\r\n\r\n<html><title>\n  My  Studio </title></html>  ");
        assert_eq!(probe_page("127.0.0.1", port), Some(WebPage { title: Some("My Studio".into()) }));
    }

    #[test]
    fn a_redirect_counts_as_a_page_without_a_title() {
        let port = serve("HTTP/1.1 302 Found\r\nLocation: /login\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert_eq!(probe_page("127.0.0.1", port), Some(WebPage { title: None }));
    }

    #[test]
    fn what_is_not_a_page_does_not_count() {
        for response in [
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            "this program does not speak HTTP\r\n",
        ] {
            assert_eq!(probe_page("127.0.0.1", serve(response)), None, "{response}");
        }
    }

    #[test]
    fn a_closed_port_does_not_count() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        assert_eq!(probe_page("127.0.0.1", port), None);
    }

    #[test]
    fn titles_are_read_whatever_their_case_and_length() {
        assert_eq!(page_title("<HTML><TITLE lang=en>Hi</TITLE>").as_deref(), Some("Hi"));
        assert_eq!(page_title("<title></title>"), None);
        assert_eq!(page_title("<p>no title</p>"), None);
        assert_eq!(page_title(&format!("<title>{}</title>", "é".repeat(200))).map(|t| t.chars().count()), Some(80));
    }
}
