//! Mobile companion: a small web app the daemon serves on the local network, so a phone can see
//! every session, what needs the user, read a session's screen and (when allowed) answer it.
//!
//! The page is a remote shell on this machine, so: off by default; bound to one chosen IPv4
//! address (a private or Tailscale one), never 0.0.0.0; a device pairs with a one-time code
//! (in a QR code, valid 2 minutes, single use) and only after the user approves it on the
//! desktop; each device gets its own 256-bit token, stored hashed, revocable, read-only unless
//! allowed to type; WebSockets must come from the page's own origin; and HTTPS is used when a
//! certificate is configured (e.g. from `tailscale cert`).

use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, oneshot, Notify};

use crate::daemon::{Daemon, Entry};
use crate::persist::write_atomic;
use crate::session::now_millis;

pub const DEFAULT_PORT: u16 = 4817;
const CODE_TTL: Duration = Duration::from_secs(120);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    token_sha256: String,
    pub can_write: bool,
    pub created_at: u64,
    #[serde(default)]
    pub last_seen: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct Config {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    ip: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    /// PEM certificate chain and key: serve HTTPS (secure context, installable app).
    #[serde(default)]
    cert: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    devices: Vec<Device>,
}

struct Running {
    url: String,
    stop: Arc<Notify>,
}

struct Pending {
    name: String,
    reply: oneshot::Sender<Option<(String, Device)>>,
}

pub struct Mobile {
    path: PathBuf,
    config: Mutex<Config>,
    running: Mutex<Option<Running>>,
    last_error: Mutex<Option<String>>,
    code: Mutex<Option<(String, Instant)>>,
    pending: Mutex<HashMap<String, Pending>>,
    failures: Mutex<VecDeque<Instant>>,
    /// Project names from the app (the daemon only knows ids).
    projects: Mutex<HashMap<String, String>>,
    /// Revoked device ids: their open sockets close.
    revoked: broadcast::Sender<String>,
}

fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Compares two strings in time independent of where they differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 256 random bits as hex (two v4 UUIDs: the OS random generator).
fn secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Addresses a phone could reach: private ranges and Tailscale's (100.64.0.0/10).
pub fn usable(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || (o[0] == 100 && (64..128).contains(&o[1]))
}

fn addresses() -> Vec<(String, String)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() && usable(v4) => Some((v4.to_string(), i.name.clone())),
            _ => None,
        })
        .collect()
}

impl Mobile {
    pub fn load(dir: &Path) -> Mobile {
        let path = dir.join("mobile.json");
        let config = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Mobile {
            path,
            config: Mutex::new(config),
            running: Mutex::new(None),
            last_error: Mutex::new(None),
            code: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            failures: Mutex::new(VecDeque::new()),
            projects: Mutex::new(HashMap::new()),
            revoked: broadcast::channel(16).0,
        }
    }

    fn save(&self) {
        if let Ok(bytes) = serde_json::to_vec_pretty(&*self.config.lock()) {
            let _ = write_atomic(&self.path, &bytes);
        }
    }

    pub fn status(&self) -> Value {
        let c = self.config.lock();
        let running = self.running.lock();
        let code = self.code.lock().as_ref().filter(|(_, at)| at.elapsed() < CODE_TTL).map(|(_, at)| (CODE_TTL - at.elapsed()).as_secs());
        json!({
            "enabled": c.enabled,
            "running": running.is_some(),
            "url": running.as_ref().map(|r| r.url.clone()),
            "error": self.last_error.lock().clone(),
            "ip": c.ip,
            "port": c.port.unwrap_or(DEFAULT_PORT),
            "tls": c.cert.is_some() && c.key.is_some(),
            "cert": c.cert,
            "key": c.key,
            "ips": addresses().into_iter().map(|(ip, name)| json!({"ip": ip, "name": name})).collect::<Vec<_>>(),
            "devices": c.devices.iter().map(|d| json!({"id": d.id, "name": d.name, "can_write": d.can_write, "created_at": d.created_at, "last_seen": d.last_seen})).collect::<Vec<_>>(),
            "pairing_expires_in": code,
            "pending": self.pending.lock().iter().map(|(id, p)| json!({"id": id, "name": p.name})).collect::<Vec<_>>(),
        })
    }

    fn device_for(&self, token: &str) -> Option<Device> {
        let hash = sha256_hex(token);
        let mut c = self.config.lock();
        // Every entry is compared, whatever matches, in constant time.
        let mut found = None;
        for d in c.devices.iter_mut() {
            if same(&d.token_sha256, &hash) {
                d.last_seen = now_millis();
                found = Some(d.clone());
            }
        }
        found
    }
}

// ---------- RPCs (from the desktop app) ----------

impl Mobile {
    /// A project's name as the app last sent it.
    pub fn project_name(&self, id: &str) -> Option<String> {
        self.projects.lock().get(id).cloned()
    }

    /// The phone web app's address while phone access is on.
    pub fn running_url(&self) -> Option<String> {
        self.running.lock().as_ref().map(|r| r.url.clone())
    }

    pub fn set_projects(&self, params: &Value) {
        let map = params.get("projects").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string()))).collect()).unwrap_or_default();
        *self.projects.lock() = map;
    }

    pub fn pair_start(&self) -> Result<Value> {
        let url = self.running.lock().as_ref().map(|r| r.url.clone()).ok_or_else(|| anyhow!("turn mobile access on first"))?;
        let code = secret();
        *self.code.lock() = Some((code.clone(), Instant::now()));
        // The code travels in the fragment: browsers never send it in a request or Referer.
        let link = format!("{url}#p={code}");
        let svg = qrcode::QrCode::new(link.as_bytes())?
            .render::<qrcode::render::svg::Color>()
            .min_dimensions(240, 240)
            .dark_color(qrcode::render::svg::Color("#141823"))
            .light_color(qrcode::render::svg::Color("#ffffff"))
            .build();
        Ok(json!({"url": link, "qr": svg, "expires_in": CODE_TTL.as_secs()}))
    }

    pub fn approve(&self, params: &Value) -> Result<Value> {
        let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
        let allow = params.get("allow").and_then(Value::as_bool).unwrap_or(false);
        let can_write = params.get("can_write").and_then(Value::as_bool).unwrap_or(false);
        let p = self.pending.lock().remove(id).ok_or_else(|| anyhow!("no such request (it may have timed out)"))?;
        if !allow {
            let _ = p.reply.send(None);
            return Ok(json!(false));
        }
        let token = secret();
        let device = Device { id: uuid::Uuid::new_v4().to_string(), name: p.name.clone(), token_sha256: sha256_hex(&token), can_write, created_at: now_millis(), last_seen: now_millis() };
        self.config.lock().devices.push(device.clone());
        self.save();
        let _ = p.reply.send(Some((token, device)));
        Ok(json!(true))
    }

    pub fn revoke(&self, id: &str) -> Result<Value> {
        let before = self.config.lock().devices.len();
        self.config.lock().devices.retain(|d| d.id != id);
        if self.config.lock().devices.len() == before {
            bail!("no such device");
        }
        self.save();
        let _ = self.revoked.send(id.to_string());
        Ok(json!(true))
    }

    pub fn set_write(&self, id: &str, can_write: bool) -> Result<Value> {
        let mut c = self.config.lock();
        let d = c.devices.iter_mut().find(|d| d.id == id).ok_or_else(|| anyhow!("no such device"))?;
        d.can_write = can_write;
        drop(c);
        self.save();
        // Reconnecting picks the new permission up.
        let _ = self.revoked.send(id.to_string());
        Ok(json!(true))
    }

    pub fn disable(&self) {
        if let Some(r) = self.running.lock().take() {
            r.stop.notify_waiters();
        }
        self.config.lock().enabled = false;
        *self.code.lock() = None;
        self.save();
    }
}

/// Starts (or restarts) the server with `params` (`ip`, `port`, `cert`, `key`) merged into the
/// saved settings. Returns the status.
pub async fn enable(daemon: Arc<Daemon>, params: Value) -> Result<Value> {
    let m = &daemon.mobile;
    {
        let mut c = m.config.lock();
        if let Some(ip) = params.get("ip").and_then(Value::as_str) {
            c.ip = Some(ip.to_string()).filter(|s| !s.is_empty());
        }
        if let Some(port) = params.get("port").and_then(Value::as_u64) {
            c.port = Some(u16::try_from(port).ok().filter(|p| *p >= 1024).ok_or_else(|| anyhow!("port must be 1024–65535"))?);
        }
        let path = |k: &str| params.get(k).map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string));
        if let Some(v) = path("cert") {
            c.cert = v;
        }
        if let Some(v) = path("key") {
            c.key = v;
        }
        c.enabled = true;
    }
    m.save();
    if let Some(r) = m.running.lock().take() {
        r.stop.notify_waiters();
    }
    match start(daemon.clone()).await {
        Ok(_) => *m.last_error.lock() = None,
        Err(e) => {
            *m.last_error.lock() = Some(format!("{e:#}"));
            return Err(e);
        }
    }
    Ok(m.status())
}

/// At daemon start: serve again if it was on.
pub async fn autostart(daemon: Arc<Daemon>) {
    if daemon.mobile.config.lock().enabled {
        if let Err(e) = start(daemon.clone()).await {
            tracing::warn!("mobile access: {e:#}");
            *daemon.mobile.last_error.lock() = Some(format!("{e:#}"));
        }
    }
}

async fn start(daemon: Arc<Daemon>) -> Result<String> {
    let (ip, port, cert, key) = {
        let c = daemon.mobile.config.lock();
        (c.ip.clone(), c.port.unwrap_or(DEFAULT_PORT), c.cert.clone(), c.key.clone())
    };
    let ip: Ipv4Addr = match ip {
        Some(s) => s.parse().context("invalid address")?,
        None => addresses().first().map(|(ip, _)| ip.parse().unwrap()).ok_or_else(|| anyhow!("no private network address found"))?,
    };
    if !usable(ip) && !ip.is_loopback() {
        bail!("{ip} is not a private network address");
    }
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((ip, port))).await.with_context(|| format!("listen on {ip}:{port}"))?;
    let tls = match (cert, key) {
        (Some(c), Some(k)) => Some(tls_acceptor(&c, &k)?),
        _ => None,
    };
    let url = format!("{}://{ip}:{port}/", if tls.is_some() { "https" } else { "http" });
    let stop = Arc::new(Notify::new());
    let app = Router::new()
        .route("/", get(page))
        .route("/app.js", get(page))
        .route("/app.css", get(page))
        .route("/manifest.webmanifest", get(page))
        .route("/icon.svg", get(page))
        .route("/pair", post(pair))
        .route("/ws", get(socket))
        .with_state(daemon.clone());
    let stop2 = stop.clone();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                _ = stop2.notified() => break,
                a = listener.accept() => a,
            };
            let Ok((tcp, _)) = accepted else { continue };
            let app = app.clone();
            let tls = tls.clone();
            tokio::spawn(async move {
                let svc = hyper_util::service::TowerToHyperService::new(app);
                let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
                let _ = match tls {
                    Some(acceptor) => match acceptor.accept(tcp).await {
                        Ok(s) => builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(s), svc).await,
                        Err(_) => return,
                    },
                    None => builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(tcp), svc).await,
                };
            });
        }
    });
    *daemon.mobile.running.lock() = Some(Running { url: url.clone(), stop });
    tracing::info!("mobile access on {url}");
    Ok(url)
}

fn tls_acceptor(cert: &str, key: &str) -> Result<tokio_rustls::TlsAcceptor> {
    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert).context("certificate file")?.collect::<Result<_, _>>().context("certificate file")?;
    let key = PrivateKeyDer::from_pem_file(key).context("key file")?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

// ---------- HTTP ----------

const INDEX: &str = include_str!("../mobile/index.html");
const APP_JS: &str = include_str!("../mobile/app.js");
const APP_CSS: &str = include_str!("../mobile/app.css");
const MANIFEST: &str = include_str!("../mobile/manifest.webmanifest");
const ICON: &str = include_str!("../mobile/icon.svg");

fn secure_headers(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; connect-src 'self'; img-src 'self' data:; style-src 'self'; script-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
    h.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    h.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    r
}

async fn page(uri: axum::http::Uri) -> Response {
    let (body, ty) = match uri.path() {
        "/app.js" => (APP_JS, "text/javascript; charset=utf-8"),
        "/app.css" => (APP_CSS, "text/css; charset=utf-8"),
        "/manifest.webmanifest" => (MANIFEST, "application/manifest+json"),
        "/icon.svg" => (ICON, "image/svg+xml"),
        _ => (INDEX, "text/html; charset=utf-8"),
    };
    secure_headers(([(header::CONTENT_TYPE, ty)], body).into_response())
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    #[serde(default)]
    name: String,
}

async fn pair(State(daemon): State<Arc<Daemon>>, Json(req): Json<PairRequest>) -> Response {
    let m = &daemon.mobile;
    let refuse = |status: StatusCode, msg: &str| secure_headers((status, Json(json!({"error": msg}))).into_response());
    {
        let mut f = m.failures.lock();
        while f.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            f.pop_front();
        }
        if f.len() >= 10 {
            return refuse(StatusCode::TOO_MANY_REQUESTS, "too many attempts, wait a minute");
        }
    }
    // Single use: whatever happens, this code is spent.
    let valid = m.code.lock().take().is_some_and(|(code, at)| at.elapsed() < CODE_TTL && same(&code, &req.code));
    if !valid {
        m.failures.lock().push_back(Instant::now());
        return refuse(StatusCode::FORBIDDEN, "this pairing code is not valid any more; show a new one in Kelpie");
    }
    let name: String = req.name.trim().chars().filter(|c| !c.is_control()).take(40).collect();
    let name = if name.is_empty() { "Phone".to_string() } else { name };
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    m.pending.lock().insert(id.clone(), Pending { name: name.clone(), reply: tx });
    daemon.broadcast("mobile.pair_request", json!({"id": id, "name": name}));
    let answer = tokio::time::timeout(CODE_TTL, rx).await;
    m.pending.lock().remove(&id);
    daemon.broadcast("mobile.updated", json!({}));
    match answer {
        Ok(Ok(Some((token, device)))) => secure_headers(Json(json!({"token": token, "device": {"name": device.name, "can_write": device.can_write}})).into_response()),
        Ok(Ok(None)) => refuse(StatusCode::FORBIDDEN, "declined on the computer"),
        _ => refuse(StatusCode::REQUEST_TIMEOUT, "not approved in time"),
    }
}

/// WebSockets only from the page itself (no cross-site socket hijacking).
fn same_origin(headers: &HeaderMap) -> bool {
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    match headers.get(header::ORIGIN).and_then(|o| o.to_str().ok()) {
        Some(origin) => origin.split_once("://").is_some_and(|(_, rest)| rest == host),
        None => true, // non-browser clients send no Origin
    }
}

async fn socket(State(daemon): State<Arc<Daemon>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(256 * 1024).on_upgrade(move |s| client(daemon, s))
}

fn sessions(daemon: &Daemon) -> Value {
    json!(daemon
        .list()
        .into_iter()
        .map(|s| json!({
            "handle": s.handle, "id": s.spec.id, "name": s.spec.name, "kind": s.spec.kind, "title": s.title,
            "status": s.status, "unread": s.unread, "alive": s.alive, "chat": s.chat,
            "project_id": s.spec.project_id, "cwd": s.spec.cwd, "last_activity": s.last_activity,
        }))
        .collect::<Vec<_>>())
}

async fn send(ws: &mut WebSocket, v: Value) -> bool {
    ws.send(Message::Text(v.to_string().into())).await.is_ok()
}

async fn client(daemon: Arc<Daemon>, mut ws: WebSocket) {
    // First message: the device token.
    let token = match tokio::time::timeout(Duration::from_secs(10), ws.recv()).await {
        Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<Value>(&t).ok().and_then(|v| v["token"].as_str().map(str::to_string)),
        _ => None,
    };
    let Some(device) = token.and_then(|t| daemon.mobile.device_for(&t)) else {
        let _ = ws.send(Message::Close(Some(CloseFrame { code: 4401, reason: "not paired".into() }))).await;
        return;
    };
    let mut revoked = daemon.mobile.revoked.subscribe();
    let projects = || json!(daemon.mobile.projects.lock().clone());
    let mut last = sessions(&daemon);
    if !send(&mut ws, json!({"type": "hello", "device": {"name": device.name, "can_write": device.can_write}, "sessions": last, "projects": projects()})).await {
        return;
    }
    let mut tick = tokio::time::interval(Duration::from_millis(1000));
    loop {
        tokio::select! {
            r = revoked.recv() => {
                if r.is_ok_and(|id| id == device.id) {
                    let _ = ws.send(Message::Close(Some(CloseFrame { code: 4401, reason: "revoked".into() }))).await;
                    return;
                }
            }
            _ = tick.tick() => {
                let now = sessions(&daemon);
                if now != last {
                    last = now.clone();
                    if !send(&mut ws, json!({"type": "sessions", "sessions": now, "projects": projects()})).await {
                        return;
                    }
                }
            }
            msg = ws.recv() => {
                let Some(Ok(msg)) = msg else { return };
                let Message::Text(text) = msg else { continue };
                let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
                if let Some(reply) = handle(&daemon, &device, &req).await {
                    if !send(&mut ws, reply).await {
                        return;
                    }
                }
            }
        }
    }
}

/// Bytes for a named key (the phone's quick keys).
fn key_bytes(key: &str) -> Option<&'static [u8]> {
    Some(match key {
        "enter" => b"\r",
        "esc" => b"\x1b",
        "ctrl-c" => b"\x03",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "tab" => b"\t",
        "y" => b"y",
        "n" => b"n",
        "1" => b"1",
        "2" => b"2",
        "3" => b"3",
        _ => return None,
    })
}

async fn handle(daemon: &Daemon, device: &Device, req: &Value) -> Option<Value> {
    let handle = req["handle"].as_u64().and_then(|h| u32::try_from(h).ok());
    let entry = handle.and_then(|h| daemon.entry(h).ok());
    match req["type"].as_str()? {
        "read" => match entry? {
            Entry::Term(s) => Some(json!({"type": "screen", "handle": handle, "text": s.text_tail(req["lines"].as_u64().unwrap_or(200).clamp(10, 400) as usize)})),
            Entry::Chat(_) => Some(json!({"type": "screen", "handle": handle, "text": "This is a chat-view session: open it on the computer."})),
        },
        "input" | "key" if !device.can_write => Some(json!({"type": "error", "message": "This device may read but not type. Allow it in Kelpie → Settings → Mobile."})),
        "input" => {
            let e = entry?;
            let text = req["text"].as_str().unwrap_or("");
            if text.len() > 16 * 1024 {
                return Some(json!({"type": "error", "message": "too long"}));
            }
            e.input(text.as_bytes().to_vec());
            if req["enter"].as_bool() == Some(true) {
                // A separate write: TUIs take an Enter in the same write as pasted text.
                tokio::time::sleep(Duration::from_millis(120)).await;
                e.input(b"\r".to_vec());
            }
            None
        }
        "key" => {
            entry?.input(key_bytes(req["key"].as_str()?)?.to_vec());
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_a_phone_can_use() {
        assert!(usable("192.168.1.20".parse().unwrap()));
        assert!(usable("10.0.0.5".parse().unwrap()));
        assert!(usable("172.20.1.1".parse().unwrap()));
        assert!(usable("100.101.102.103".parse().unwrap()), "tailscale");
        assert!(!usable("8.8.8.8".parse().unwrap()));
        assert!(!usable("100.20.0.1".parse().unwrap()));
    }

    #[test]
    fn tokens_and_comparisons() {
        let t = secret();
        assert_eq!(t.len(), 64);
        assert_ne!(t, secret());
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        assert_eq!(sha256_hex("abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn origin_must_match_host() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "192.168.1.2:4817".parse().unwrap());
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, "http://192.168.1.2:4817".parse().unwrap());
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
        assert!(!same_origin(&h));
    }

    #[test]
    fn quick_keys() {
        assert_eq!(key_bytes("ctrl-c"), Some(&b"\x03"[..]));
        assert_eq!(key_bytes("rm -rf"), None);
    }
}
