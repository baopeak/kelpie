//! Browser panes: a child webview of the main window per pane, placed over the pane's DOM
//! area by the UI (`browser_bounds`). Pages get no IPC (the app capability covers the main
//! webview only); what Kelpie needs from them comes through `browser_picker.js`, whose reports
//! arrive as cancelled `window.open` calls of a sentinel URL carrying a per-page nonce.
//!
//! Events to the UI (`browser-event`): `{label, kind: "nav" | "load" | "title" | "pick" |
//! "cancel" | "popup", url?, title?, data?}`.

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine as _;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::webview::{NewWindowResponse, PageLoadEvent, WebviewBuilder};
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Url, WebviewUrl};

use crate::fs_ops::{err, CmdResult};

const PICKER: &str = include_str!("browser_picker.js");
const SENTINEL_HOST: &str = "kelpie-pick.invalid";

/// Nonce of each browser webview (by label); reports carrying another one are ignored.
#[derive(Default)]
pub struct Browsers {
    nonces: Mutex<HashMap<String, String>>,
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

/// Creates the pane's webview (or moves and shows the existing one).
#[tauri::command]
pub async fn browser_open(app: AppHandle, state: tauri::State<'_, Browsers>, label: String, url: String, rect: Rect) -> CmdResult<()> {
    valid_label(&label)?;
    if let Some(wv) = app.get_webview(&label) {
        wv.set_position(LogicalPosition::new(rect.x, rect.y)).map_err(err)?;
        wv.set_size(LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0))).map_err(err)?;
        return wv.show().map_err(err);
    }
    let url = parse_url(&url)?;
    let window = app.get_window("main").ok_or("no main window")?;
    let nonce = new_nonce();
    state.nonces.lock().unwrap().insert(label.clone(), nonce.clone());
    let (nav_app, load_app, title_app, popup_app) = (app.clone(), app.clone(), app.clone(), app.clone());
    let (nav_label, load_label, title_label, popup_label) = (label.clone(), label.clone(), label.clone(), label.clone());
    let popup_nonce = nonce.clone();
    let builder = WebviewBuilder::new(&label, WebviewUrl::External(url))
        .initialization_script(PICKER.replace("__KELPIE_NONCE__", &nonce))
        .on_navigation(move |u| {
            if u.host_str() == Some(SENTINEL_HOST) {
                return false;
            }
            emit(&nav_app, &nav_label, "nav", Some(u.to_string()), None, None);
            // Pages stay on the web: no file:, javascript: or app schemes.
            matches!(u.scheme(), "http" | "https" | "about" | "data" | "blob")
        })
        .on_page_load(move |_wv, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                emit(&load_app, &load_label, "load", Some(payload.url().to_string()), None, None);
            }
        })
        .on_document_title_changed(move |_, title| emit(&title_app, &title_label, "title", None, Some(title), None))
        .on_new_window(move |u, _features| {
            match decode_report(&u, &popup_nonce) {
                Some((kind, data)) => emit(&popup_app, &popup_label, if kind == "pick" { "pick" } else { "cancel" }, None, None, Some(data)),
                // Links that open a new window open in this pane instead.
                None if matches!(u.scheme(), "http" | "https") && u.host_str() != Some(SENTINEL_HOST) => emit(&popup_app, &popup_label, "popup", Some(u.to_string()), None, None),
                None => {}
            }
            NewWindowResponse::Deny
        });
    window
        .add_child(builder, LogicalPosition::new(rect.x, rect.y), LogicalSize::new(rect.width.max(1.0), rect.height.max(1.0)))
        .map_err(err)?;
    Ok(())
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
    if visible { wv.show() } else { wv.hide() }.map_err(err)
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
    state.nonces.lock().unwrap().remove(&label);
    match app.get_webview(&label) {
        Some(wv) => wv.close().map_err(err),
        None => Ok(()),
    }
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
    let dir = kelpie_proto::data_dir().join("browser-shots");
    std::fs::create_dir_all(&dir).map_err(err)?;
    prune_shots(&dir);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let path = dir.join(format!("{stamp}.png"));
    std::fs::write(&path, &png).map_err(err)?;
    Ok(json!({"path": path.to_string_lossy(), "data": base64::engine::general_purpose::STANDARD.encode(&png)}))
}

/// Keeps the newest 200 screenshots.
fn prune_shots(dir: &std::path::Path) {
    let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "png")).collect();
    if files.len() <= 200 {
        return;
    }
    files.sort();
    for f in &files[..files.len() - 200] {
        let _ = std::fs::remove_file(f);
    }
}

#[cfg(windows)]
async fn capture(wv: &tauri::Webview, clip: Option<Rect>) -> CmdResult<Vec<u8>> {
    let mut params = json!({"format": "png"});
    if let Some(r) = clip {
        // Page.captureScreenshot clips in document coordinates; the picker reports the
        // viewport box, so the page's scroll offset is added by the caller's rect origin.
        params["clip"] = json!({"x": r.x.max(0.0), "y": r.y.max(0.0), "width": r.width.max(1.0), "height": r.height.max(1.0), "scale": 1});
        params["captureBeyondViewport"] = json!(true);
    }
    let v = devtools(wv, "Page.captureScreenshot", params).await?;
    base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().ok_or("no screenshot data")?).map_err(err)
}

/// Calls a DevTools protocol method on the page's WebView2 and returns its JSON result.
#[cfg(windows)]
async fn devtools(wv: &tauri::Webview, method: &'static str, params: Value) -> CmdResult<Value> {
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

#[cfg(not(windows))]
async fn capture(_wv: &tauri::Webview, _clip: Option<Rect>) -> CmdResult<Vec<u8>> {
    Err("browser screenshots are not available on this platform yet".into())
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
