//! Mobile companion: a small web app the daemon serves on the local network, so a phone can see
//! every session, what needs the user, read a session's screen and (when allowed) answer it.
//!
//! Protocol v2 (`/ws`, JSON text messages): the session list is pushed when it changes; a
//! terminal session's screen is followed with `sub` and arrives as row diffs (`sync`); a chat
//! session is watched with `chat_sub` and its permission requests answered with `chat_answer`.
//! Sessions are addressed by id (handles are not stable across daemon restarts) and what a phone
//! may ask for is the closed table in `verbs`.
//!
//! The page is a remote shell on this machine, so: off by default; bound to one chosen IPv4
//! address (a private or Tailscale one), never 0.0.0.0; a device pairs with a one-time code
//! (in a QR code, valid 2 minutes, single use) and only after the user approves it on the
//! desktop; each device gets its own 256-bit token, stored hashed, revocable, read-only unless
//! allowed to type; WebSockets must come from the page's own origin; and HTTPS is used when a
//! certificate is configured (e.g. from `tailscale cert`).

mod sync;
mod verbs;

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, oneshot, Notify};

use crate::daemon::{Daemon, Entry};
use crate::persist::write_atomic;
use crate::session::{next_client_id, now_millis, Answer, AnswerError, ClientTx, Outgoing};
use verbs::{Permission, Verb};

pub const DEFAULT_PORT: u16 = 4817;
const CODE_TTL: Duration = Duration::from_secs(120);
/// Failed pairing attempts one address may make in a minute.
const PAIR_FAILURES: usize = 10;
/// Addresses the limiter remembers at once.
const LIMITER_ADDRESSES: usize = 1024;
/// Followed and watched sessions per connection.
const MAX_FOLLOWED: usize = 16;
/// Text a phone may type in one message; a chat answer line may be longer (a tool's input).
const MAX_TYPED: usize = 16 * 1024;
const MAX_ANSWER_LINE: usize = 64 * 1024;
/// A chat message to the phone carries lines up to about this much (a longer line goes alone).
const CHAT_MESSAGE_BYTES: usize = 64 * 1024;
/// Least time between two `chat_sub` for one chat on one connection: each can mean a log of 2 MiB.
const CHAT_SUB_GAP: Duration = Duration::from_secs(1);

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
    stop: Arc<Generation>,
}

/// Sticky shutdown: late subscribers (including HTTP upgrades) also see cancellation.
struct Generation {
    stopped: tokio::sync::watch::Sender<bool>,
}

impl Generation {
    fn new() -> Self {
        Self { stopped: tokio::sync::watch::channel(false).0 }
    }

    fn cancel(&self) {
        self.stopped.send_replace(true);
    }

    fn is_cancelled(&self) -> bool {
        *self.stopped.borrow()
    }

    async fn cancelled(&self) {
        let mut stopped = self.stopped.subscribe();
        let _ = stopped.wait_for(|stopped| *stopped).await;
    }
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
    /// Failed pairing attempts by source address.
    failures: Mutex<HashMap<IpAddr, VecDeque<Instant>>>,
    /// Random per daemon start: a phone that sees it change knows ids, positions and anything
    /// it had in flight belong to the daemon before.
    epoch: String,
    /// The open connection of each device: a newer one for the same device ends the older.
    connections: Mutex<HashMap<String, (u64, Arc<Notify>)>>,
    /// What each phone was last sent of each session, and the renders they share (see `sync`).
    views: Arc<sync::Views>,
    /// Project names from the app (the daemon only knows ids).
    projects: Mutex<HashMap<String, String>>,
    /// Revoked device ids: their open sockets close.
    revoked: broadcast::Sender<(String, bool)>,
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
            failures: Mutex::new(HashMap::new()),
            epoch: uuid::Uuid::new_v4().simple().to_string(),
            connections: Mutex::new(HashMap::new()),
            views: Arc::new(sync::Views::default()),
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

    /// What the device may do now (None while disabled or once revoked).
    fn permission(&self, id: &str) -> Option<Permission> {
        let c = self.config.lock();
        if !c.enabled {
            return None;
        }
        c.devices.iter().find(|d| d.id == id).map(|d| if d.can_write { Permission::Write } else { Permission::Read })
    }

    /// Whether `ip` has failed to pair too often lately.
    fn pairing_blocked(&self, ip: IpAddr) -> bool {
        let mut by_ip = self.failures.lock();
        let recent = by_ip.get_mut(&ip).map(|f| {
            while f.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
                f.pop_front();
            }
            f.len()
        });
        if recent == Some(0) {
            by_ip.remove(&ip);
        }
        recent.is_some_and(|n| n >= PAIR_FAILURES)
    }

    fn pairing_failed(&self, ip: IpAddr) {
        let mut by_ip = self.failures.lock();
        if by_ip.len() >= LIMITER_ADDRESSES && !by_ip.contains_key(&ip) {
            // Addresses whose failures all aged out go first; if the table is still full, it starts over.
            by_ip.retain(|_, f| f.back().is_some_and(|t| t.elapsed() <= Duration::from_secs(60)));
            if by_ip.len() >= LIMITER_ADDRESSES {
                by_ip.clear();
            }
        }
        by_ip.entry(ip).or_default().push_back(Instant::now());
    }

    fn device_for(&self, token: &str) -> Option<Device> {
        let hash = sha256_hex(token);
        let mut c = self.config.lock();
        if !c.enabled {
            return None;
        }
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
        let _ = self.revoked.send((id.to_string(), true));
        Ok(json!(true))
    }

    pub fn set_write(&self, id: &str, can_write: bool) -> Result<Value> {
        let mut c = self.config.lock();
        let d = c.devices.iter_mut().find(|d| d.id == id).ok_or_else(|| anyhow!("no such device"))?;
        d.can_write = can_write;
        drop(c);
        self.save();
        // Reconnecting picks the new permission up (the device stays paired).
        let _ = self.revoked.send((id.to_string(), false));
        Ok(json!(true))
    }

    pub fn disable(&self) {
        self.config.lock().enabled = false;
        if let Some(r) = self.running.lock().take() {
            r.stop.cancel();
        }
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
        if let Some(r) = m.running.lock().take() {
            r.stop.cancel();
        }
        c.enabled = true;
    }
    m.save();
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
    let stop = Arc::new(Generation::new());
    let app = Router::new()
        .route("/", get(page))
        .route("/app.js", get(page))
        .route("/app.css", get(page))
        .route("/manifest.webmanifest", get(page))
        .route("/icon.svg", get(page))
        .route("/pair", post(pair))
        .route("/ws", get(socket))
        .with_state(daemon.clone())
        .layer(axum::Extension(stop.clone()));
    let stop2 = stop.clone();
    {
        let c = daemon.mobile.config.lock();
        if !c.enabled {
            bail!("mobile access was disabled while starting");
        }
        if let Some(old) = daemon.mobile.running.lock().replace(Running { url: url.clone(), stop: stop.clone() }) {
            old.stop.cancel();
        }
    }
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                biased;
                _ = stop2.cancelled() => break,
                a = listener.accept() => a,
            };
            let Ok((tcp, peer)) = accepted else { continue };
            // Who is calling, for the pairing limiter.
            let app = app.clone().layer(axum::Extension(ConnectInfo(peer)));
            let tls = tls.clone();
            let connection_stop = stop2.clone();
            tokio::spawn(async move {
                tokio::select! {
                    biased;
                    _ = connection_stop.cancelled() => {}
                    _ = async move {
                        let svc = hyper_util::service::TowerToHyperService::new(app);
                        let builder = hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new());
                        let _ = match tls {
                            Some(acceptor) => match acceptor.accept(tcp).await {
                                Ok(s) => builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(s), svc).await,
                                Err(_) => return,
                            },
                            None => builder.serve_connection_with_upgrades(hyper_util::rt::TokioIo::new(tcp), svc).await,
                        };
                    } => {}
                }
            });
        }
    });
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

async fn pair(State(daemon): State<Arc<Daemon>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Json(req): Json<PairRequest>) -> Response {
    let m = &daemon.mobile;
    let ip = peer.ip().to_canonical();
    let refuse = |status: StatusCode, msg: &str| secure_headers((status, Json(json!({"error": msg}))).into_response());
    if m.pairing_blocked(ip) {
        return refuse(StatusCode::TOO_MANY_REQUESTS, "too many attempts, wait a minute");
    }
    // Single use: whatever happens, this code is spent.
    let valid = m.code.lock().take().is_some_and(|(code, at)| at.elapsed() < CODE_TTL && same(&code, &req.code));
    if !valid {
        m.pairing_failed(ip);
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

async fn socket(State(daemon): State<Arc<Daemon>>, axum::Extension(generation): axum::Extension<Arc<Generation>>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    if generation.is_cancelled() || !daemon.mobile.config.lock().enabled {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    ws.max_message_size(256 * 1024).on_upgrade(move |s| client(daemon, s, generation))
}

fn sessions(daemon: &Daemon) -> Value {
    json!(daemon
        .list()
        .into_iter()
        .map(|s| json!({
            "id": s.spec.id, "name": s.spec.name, "kind": s.spec.kind, "title": s.title,
            "status": s.status, "unread": s.unread, "alive": s.alive, "chat": s.chat,
            "project_id": s.spec.project_id, "cwd": s.spec.cwd, "last_activity": s.last_activity,
            "waiting_since": s.waiting_since,
        }))
        .collect::<Vec<_>>())
}

/// A socket that takes longer than this to accept a message is cut: the phone is gone, or too far
/// behind to catch up (it reconnects and gets the current state).
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

async fn send(ws: &mut WebSocket, text: impl Into<String>) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, ws.send(Message::Text(text.into().into()))).await, Ok(Ok(())))
}

/// Close codes: 4401 the device is not paired (the page forgets its token); 4408 the connection must
/// be made again, the device stays paired (its permission changed, or it fell too far behind);
/// 4409 another connection of this device took over (the page waits until it is looked at again
/// instead of fighting for the slot).
const NOT_PAIRED: u16 = 4401;
const RECONNECT: u16 = 4408;
const REPLACED: u16 = 4409;

/// How long a close frame may take to leave: a peer that stopped reading must not keep its
/// connection (and what hangs on it) alive.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether `fut` finished within `limit`.
async fn within<F: std::future::Future>(limit: Duration, fut: F) -> bool {
    tokio::time::timeout(limit, fut).await.is_ok()
}

async fn close(ws: &mut WebSocket, code: u16, reason: &'static str) {
    within(CLOSE_TIMEOUT, ws.send(Message::Close(Some(CloseFrame { code, reason: reason.into() })))).await;
}

/// What a message on the revocation channel means for the connection of `device`: close with this
/// code, or nothing. The channel carries (device, removed): a removed device is unpaired; a changed
/// one, or a receiver that fell behind and may have missed either, reconnects and finds out.
fn closing(event: Result<(String, bool), broadcast::error::RecvError>, device: &str) -> Option<(u16, &'static str)> {
    match event {
        Ok((id, removed)) if id == device => Some(if removed { (NOT_PAIRED, "revoked") } else { (RECONNECT, "changed") }),
        Ok(_) => None,
        Err(broadcast::error::RecvError::Lagged(_)) => Some((RECONNECT, "behind")),
        Err(broadcast::error::RecvError::Closed) => Some((RECONNECT, "closed")),
    }
}

/// A request that was not carried out: `code` for programs, `message` for people; `reason` says
/// why an answer is stale.
struct Refusal {
    code: &'static str,
    reason: Option<&'static str>,
    message: String,
}

fn refuse(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal { code, reason: None, message: message.into() }
}

impl From<AnswerError> for Refusal {
    fn from(e: AnswerError) -> Refusal {
        match e {
            AnswerError::Prompt => refuse("prompt", "A prompt is waiting on this session: answer it, or send the text anyway."),
            AnswerError::Stale(reason) => Refusal { code: "stale", reason: Some(reason), message: "That prompt was answered already, or has changed.".into() },
            AnswerError::UnknownKey => refuse("bad_request", "unknown key"),
        }
    }
}

impl From<crate::chat::NotAnswered> for Refusal {
    fn from(e: crate::chat::NotAnswered) -> Refusal {
        match e {
            crate::chat::NotAnswered::NotAnAnswer => refuse(e.code(), "That is not the answer to that request."),
            crate::chat::NotAnswered::AlreadyAnswered => Refusal { code: e.code(), reason: Some("answered"), message: "That request was answered already.".into() },
        }
    }
}

/// One phone's connection.
struct Conn {
    daemon: Arc<Daemon>,
    device: String,
    /// This connection as a client of the daemon (it hears the daemon's events, and attaches to chats).
    id: u64,
    tx: ClientTx,
    /// Where frames and chat messages (JSON text) wait for the socket.
    out: mpsc::Sender<String>,
    /// Terminal sessions followed, by session id.
    followed: HashMap<String, tokio::task::JoinHandle<()>>,
    /// Chat sessions watched.
    watched: HashSet<String>,
    /// When each chat was last asked for (`chat_sub`).
    chat_at: HashMap<String, Instant>,
    /// Chat output sent to this connection and not yet taken by its socket.
    waiting: Arc<AtomicUsize>,
    list: Option<(Value, Value)>,
    rev: u64,
}

impl Conn {
    /// The session list and project names as they are now, with the next `rev`, when they differ
    /// from what this phone has (or `force`).
    fn publish(&mut self, force: bool) -> Option<Value> {
        let now = (sessions(&self.daemon), json!(self.daemon.mobile.projects.lock().clone()));
        if !force && self.list.as_ref() == Some(&now) {
            return None;
        }
        self.rev += 1;
        let msg = json!({"type": "sessions", "rev": self.rev, "sessions": now.0, "projects": now.1});
        self.list = Some(now);
        Some(msg)
    }

    async fn handle(&mut self, granted: Permission, req: &Value) -> Option<Value> {
        let name = req["type"].as_str()?;
        let tag = req.get("req").filter(|r| r.is_string() || r.is_number()).cloned();
        match self.run(granted, name, req).await {
            Ok(Some(mut reply)) => {
                if let Some(t) = tag {
                    reply["req"] = t;
                }
                Some(reply)
            }
            Ok(None) => tag.map(|t| json!({"type": "ack", "req": t})),
            Err(r) => Some(json!({"type": "error", "req": tag, "code": r.code, "reason": r.reason, "message": r.message})),
        }
    }

    fn entry(&self, req: &Value) -> Result<(String, Entry), Refusal> {
        let id = req["id"].as_str().filter(|i| !i.is_empty() && i.len() <= 128).ok_or_else(|| refuse("bad_request", "missing session id"))?;
        self.daemon.by_id(id).map(|e| (id.to_string(), e)).ok_or_else(|| refuse("no_session", "That session is gone."))
    }

    fn terminal(&self, req: &Value) -> Result<(String, Arc<crate::session::Session>), Refusal> {
        match self.entry(req)? {
            (id, Entry::Term(s)) => Ok((id, s)),
            _ => Err(refuse("chat", "This is a chat session: watch it with chat_sub.")),
        }
    }

    fn chat(&self, req: &Value) -> Result<(String, Arc<crate::chat::ChatSession>), Refusal> {
        match self.entry(req)? {
            (id, Entry::Chat(c)) => Ok((id, c)),
            _ => Err(refuse("not_chat", "This is a terminal session.")),
        }
    }

    /// Room for one more followed or watched session (finished ones do not count).
    fn make_room(&mut self, id: &str) -> Result<(), Refusal> {
        self.followed.retain(|_, t| !t.is_finished());
        // A chat that was closed while watched is no longer open.
        let daemon = self.daemon.clone();
        self.watched.retain(|w| daemon.by_id(w).is_some());
        self.chat_at.retain(|c, _| daemon.by_id(c).is_some());
        if !self.followed.contains_key(id) && !self.watched.contains(id) && self.followed.len() + self.watched.len() >= MAX_FOLLOWED {
            return Err(refuse("too_many", "Too many sessions open at once."));
        }
        Ok(())
    }

    /// Carries out one request of the verb table; `Some` is a reply with content, `None` a plain ack.
    async fn run(&mut self, granted: Permission, name: &str, req: &Value) -> Result<Option<Value>, Refusal> {
        let verb = Verb::parse(name).ok_or_else(|| refuse("unknown_verb", format!("unknown request {name:?}")))?;
        if verb.needs() > granted {
            return Err(refuse("read_only", "This device may read but not type. Allow it in Kelpie → Settings → Mobile."));
        }
        // A phone that names the daemon run it talked to: after a restart its ids and positions mean nothing.
        if verb.needs() >= Permission::Write && req.get("epoch").is_some_and(|e| e.as_str() != Some(&self.daemon.mobile.epoch)) {
            return Err(refuse("epoch", "Kelpie restarted since this was sent."));
        }
        let enter = req["enter"].as_bool() == Some(true);
        match verb {
            Verb::List => return Ok(self.publish(true)),
            Verb::Sub => {
                let (id, session) = self.terminal(req)?;
                self.make_room(&id)?;
                let height = req["height"].as_u64().map_or(sync::MAX_HEIGHT, |h| h as usize).clamp(sync::MIN_HEIGHT, sync::MAX_HEIGHT);
                if let Some(previous) = self.followed.remove(&id) {
                    previous.abort();
                }
                let m = &self.daemon.mobile;
                m.views.open(&self.device, &id, self.id, req["rev"].as_u64());
                let follow = sync::Follow { views: m.views.clone(), device: self.device.clone(), conn: self.id, id: id.clone(), height, session: Arc::downgrade(&session), out: self.out.clone() };
                self.followed.insert(id, tokio::spawn(sync::run(follow)));
            }
            Verb::Unsub => {
                // Let go first: a session that is gone still counts against the limit until then.
                let id = req["id"].as_str().filter(|i| !i.is_empty() && i.len() <= 128).ok_or_else(|| refuse("bad_request", "missing session id"))?;
                if let Some(task) = self.followed.remove(id) {
                    task.abort();
                }
                self.chat_at.remove(id);
                if self.watched.remove(id) {
                    if let Some(Entry::Chat(c)) = self.daemon.by_id(id) {
                        c.detach(self.id);
                    }
                }
            }
            Verb::ChatSub => {
                let (id, chat) = self.chat(req)?;
                self.make_room(&id)?;
                if self.chat_at.get(&id).is_some_and(|at| at.elapsed() < CHAT_SUB_GAP) {
                    return Err(refuse("too_fast", "Asked for that chat again too soon."));
                }
                chat.observe(self.id, &self.tx, &json!(format!("chat:{id}")), req["since_seq"].as_u64(), &self.waiting)
                    .map_err(|_| refuse("busy", "The last chat log is still being sent."))?;
                self.chat_at.insert(id.clone(), Instant::now());
                self.watched.insert(id);
            }
            Verb::Input => {
                let (_, session) = self.terminal(req)?;
                let text = req["text"].as_str().ok_or_else(|| refuse("bad_request", "missing text"))?;
                if text.len() > MAX_TYPED {
                    return Err(refuse("too_long", "too long"));
                }
                if !session.is_alive() {
                    return Err(refuse("exited", "This session has stopped."));
                }
                let typed = session.type_message(text, req["force"].as_bool() == Some(true))?;
                if enter {
                    // Waited for here: the next message of this phone then starts after this Enter.
                    tokio::time::sleep(crate::session::ENTER_DELAY).await;
                    // Skipped when someone else typed meanwhile or a prompt came up: the text is on the
                    // prompt line, not sent.
                    let entered = session.enter_if(typed);
                    return Ok((!entered || req.get("req").is_some()).then(|| json!({"type": "ack", "entered": entered})));
                }
            }
            Verb::Key => {
                let (_, session) = self.terminal(req)?;
                let bytes = req["key"].as_str().and_then(verbs::key_bytes).ok_or_else(|| refuse("bad_request", "unknown key"))?;
                if !session.is_alive() {
                    return Err(refuse("exited", "This session has stopped."));
                }
                session.input(bytes);
            }
            Verb::Answer => {
                let (_, session) = self.terminal(req)?;
                let answer = match (req["key"].as_str(), req["text"].as_str()) {
                    (Some(key), _) => Answer::Key(key),
                    (None, Some(text)) if text.len() <= MAX_TYPED => Answer::Text(text),
                    _ => return Err(refuse("bad_request", "an answer is a key or text")),
                };
                session.answer(req["waiting_since"].as_u64(), answer, enter)?;
            }
            Verb::ChatAnswer => {
                let (_, chat) = self.chat(req)?;
                let request_id = req.get("request_id").filter(|r| r.is_string() || r.is_number()).ok_or_else(|| refuse("bad_request", "missing request_id"))?;
                let line = req["line"].as_str().filter(|l| l.len() <= MAX_ANSWER_LINE).ok_or_else(|| refuse("bad_request", "missing or too long line"))?;
                chat.answer(request_id, line)?;
            }
        }
        Ok(None)
    }

    fn end(&mut self) {
        for (_, task) in self.followed.drain() {
            task.abort();
        }
        self.daemon.mobile.views.leave(&self.device, self.id);
        self.daemon.unregister_client(self.id);
    }
}

async fn client(daemon: Arc<Daemon>, mut ws: WebSocket, generation: Arc<Generation>) {
    // Cancellation covers authentication too, including an upgrade completed before disable.
    let token = tokio::select! {
        biased;
        _ = generation.cancelled() => return,
        token = tokio::time::timeout(Duration::from_secs(10), ws.recv()) => match token {
            Ok(Some(Ok(Message::Text(t)))) => serde_json::from_str::<Value>(&t).ok().and_then(|v| v["token"].as_str().map(str::to_string)),
            _ => None,
        },
    };
    if generation.is_cancelled() {
        return;
    }
    let Some(device) = token.and_then(|t| daemon.mobile.device_for(&t)) else {
        close(&mut ws, NOT_PAIRED, "not paired").await;
        return;
    };
    // The daemon's events reach the phone through an adapter: a connection is a client like the app.
    let id = next_client_id();
    let (tx, rx) = mpsc::unbounded_channel();
    daemon.register_client(id, tx.clone());
    daemon.set_client_role(id, &crate::auth::Role::User);
    let (out, mut out_rx) = mpsc::channel(16);
    let dirty = Arc::new(Notify::new());
    let mut adapter = tokio::spawn(adapt(daemon.clone(), rx, out.clone(), dirty.clone()));
    let mut conn = Conn { daemon: daemon.clone(), device: device.id.clone(), id, tx, out, followed: HashMap::new(), watched: HashSet::new(), chat_at: HashMap::new(), waiting: Arc::new(AtomicUsize::new(0)), list: None, rev: 0 };
    // The device's older connection, if any, is told it was replaced.
    let replaced = Arc::new(Notify::new());
    if let Some((_, older)) = daemon.mobile.connections.lock().insert(device.id.clone(), (id, replaced.clone())) {
        older.notify_one();
    }
    // Dropping serve cancels an in-flight request/send, then the common cleanup still runs.
    tokio::select! {
        biased;
        _ = generation.cancelled() => {}
        _ = serve(&mut conn, &mut ws, &device, &mut out_rx, &dirty, &mut adapter, &replaced, &generation) => {}
    }
    adapter.abort();
    {
        let mut open = daemon.mobile.connections.lock();
        if open.get(&device.id).is_some_and(|(c, _)| *c == id) {
            open.remove(&device.id);
        }
    }
    conn.end();
}

async fn serve(conn: &mut Conn, ws: &mut WebSocket, device: &Device, out_rx: &mut mpsc::Receiver<String>, dirty: &Notify, adapter: &mut tokio::task::JoinHandle<()>, replaced: &Notify, generation: &Generation) {
    if generation.is_cancelled() {
        return;
    }
    let mut revoked = conn.daemon.mobile.revoked.subscribe();
    let first = conn.publish(true).expect("forced");
    let hello = json!({
        "type": "hello", "v": 2, "api": kelpie_proto::API_LEVEL, "daemon_epoch": conn.daemon.mobile.epoch, "rev": first["rev"],
        "device": {"name": device.name, "can_write": device.can_write}, "sessions": first["sessions"], "projects": first["projects"],
    });
    if !send(ws, hello.to_string()).await {
        return;
    }
    loop {
        // Also check synchronously: a busy socket can keep this future ready without yielding.
        if generation.is_cancelled() {
            return;
        }
        tokio::select! {
            r = revoked.recv() => {
                if let Some((code, reason)) = closing(r, &device.id) {
                    close(ws, code, reason).await;
                    return;
                }
            }
            _ = &mut *adapter => return,
            _ = replaced.notified() => {
                close(ws, REPLACED, "replaced").await;
                return;
            }
            _ = dirty.notified() => {
                if let Some(list) = conn.publish(false) {
                    if !send(ws, list.to_string()).await {
                        return;
                    }
                }
            }
            Some(msg) = out_rx.recv() => {
                if !send(ws, msg).await {
                    return;
                }
            }
            msg = ws.recv() => {
                let Some(Ok(msg)) = msg else { return };
                let Message::Text(text) = msg else { continue };
                let Ok(req) = serde_json::from_str::<Value>(&text) else { continue };
                if generation.is_cancelled() {
                    return;
                }
                // Every request is checked against the device's record as it is now.
                let Some(granted) = conn.daemon.mobile.permission(&device.id) else {
                    close(ws, NOT_PAIRED, "revoked").await;
                    return;
                };
                if let Some(reply) = conn.handle(granted, &req).await {
                    if !send(ws, reply.to_string()).await {
                        return;
                    }
                }
            }
        }
    }
}

/// Turns what the daemon tells this connection (a client like any other) into messages for the
/// phone: a changed session list wakes the connection to publish it, a chat's log arrives as
/// `chat` messages, and a request answered elsewhere as `chat_resolved`.
async fn adapt(daemon: Arc<Daemon>, mut rx: mpsc::UnboundedReceiver<Outgoing>, out: mpsc::Sender<String>, dirty: Arc<Notify>) {
    let mut translator = Translator::default();
    while let Some(o) = rx.recv().await {
        for msg in translator.translate(&daemon, &o.data, &dirty) {
            if !matches!(tokio::time::timeout(SEND_TIMEOUT, out.send(msg.to_string())).await, Ok(Ok(()))) {
                return;
            }
        }
        // Written off only once the phone's queue took it: a phone that cannot keep up makes the
        // chat stop feeding it (and tell it to resync) instead of a backlog growing here.
        if let Some(credit) = o.credit {
            credit.fetch_sub(o.data.len(), std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// What `adapt` remembers between frames.
#[derive(Default)]
struct Translator {
    /// Session id of each handle seen.
    ids: HashMap<u32, String>,
    /// Chats whose attach reply came: the next output frame of that handle is the log it promised,
    /// in this mode.
    awaiting: HashMap<u32, String>,
}

impl Translator {
    fn session_of(&mut self, daemon: &Daemon, handle: u32) -> Option<String> {
        if let Some(id) = self.ids.get(&handle) {
            return Some(id.clone());
        }
        let id = daemon.session_id_of(handle)?;
        self.ids.insert(handle, id.clone());
        Some(id)
    }

    fn translate(&mut self, daemon: &Daemon, frame: &[u8], dirty: &Notify) -> Vec<Value> {
        // A frame is `u32 length | u8 kind | payload`.
        match frame.get(4).copied() {
            Some(kelpie_proto::KIND_OUTPUT) if frame.len() >= 17 => {
                let handle = u32::from_le_bytes(frame[5..9].try_into().unwrap());
                let seq = u64::from_le_bytes(frame[9..17].try_into().unwrap());
                let mode = self.awaiting.remove(&handle).unwrap_or_else(|| "live".into());
                self.session_of(daemon, handle).map(|id| chat_messages(&id, &mode, seq, &frame[17..])).unwrap_or_default()
            }
            Some(kelpie_proto::KIND_JSON) => {
                let body = &frame[5..];
                let is = |event: &str| body.starts_with(format!(r#"{{"event":"{event}""#).as_bytes());
                if is("session.updated") || is("session.closed") {
                    dirty.notify_one();
                    return Vec::new();
                }
                let parse = || serde_json::from_slice::<Value>(body).ok();
                if is("chat.resolved") {
                    return parse().map(|v| vec![json!({"type": "chat_resolved", "id": v["params"]["id"], "request_id": v["params"]["request_id"], "by": v["params"]["by"], "dropped": v["params"]["dropped"]})]).unwrap_or_default();
                }
                if is("session.resync") {
                    let handle = parse().and_then(|v| v["params"]["handle"].as_u64()).and_then(|h| u32::try_from(h).ok());
                    return handle.and_then(|h| self.session_of(daemon, h)).map(|id| vec![json!({"type": "chat_resync", "id": id})]).unwrap_or_default();
                }
                // The reply to a `chat_sub` attach (its request id names the session); the log follows.
                if body.starts_with(br#"{"id":"chat:"#) {
                    let v = parse().unwrap_or_default();
                    if let (Some(handle), Some(result)) = (v["result"]["handle"].as_u64().and_then(|h| u32::try_from(h).ok()), v.get("result")) {
                        self.awaiting.insert(handle, result["mode"].as_str().unwrap_or("reset").to_string());
                    }
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// A chat's log lines as messages of about `CHAT_MESSAGE_BYTES`. `seq` is the log position after
/// the last line; each message says the position after its own, so a phone that loses the
/// connection halfway resumes from the last whole message. The first message of a reset or delta
/// carries `mode`, the rest say `more`.
fn chat_messages(id: &str, mode: &str, seq: u64, data: &[u8]) -> Vec<Value> {
    let mut groups: Vec<(Vec<String>, usize)> = Vec::new();
    for line in data.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let size = line.len() + 1;
        match groups.last_mut() {
            Some((lines, bytes)) if *bytes + size <= CHAT_MESSAGE_BYTES => {
                lines.push(String::from_utf8_lossy(line).into_owned());
                *bytes += size;
            }
            _ => groups.push((vec![String::from_utf8_lossy(line).into_owned()], size)),
        }
    }
    if groups.is_empty() {
        groups.push((Vec::new(), 0));
    }
    let mut later: usize = groups.iter().map(|g| g.1).sum();
    let rest = if mode == "live" { "live" } else { "more" };
    groups
        .into_iter()
        .enumerate()
        .map(|(i, (lines, bytes))| {
            later -= bytes;
            json!({"type": "chat", "id": id, "mode": if i == 0 { mode } else { rest }, "seq": seq.saturating_sub(later as u64), "lines": lines})
        })
        .collect()
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
    fn pairing_failures_are_counted_per_address() {
        let m = Mobile::load(&std::env::temp_dir().join(format!("kelpie-limit-{}", uuid::Uuid::new_v4())));
        let (a, b): (IpAddr, IpAddr) = ("192.168.1.20".parse().unwrap(), "192.168.1.21".parse().unwrap());
        for _ in 0..PAIR_FAILURES {
            assert!(!m.pairing_blocked(a));
            m.pairing_failed(a);
        }
        assert!(m.pairing_blocked(a));
        assert!(!m.pairing_blocked(b), "another address is not held up by it");
        // Failures older than a minute are forgotten.
        for t in m.failures.lock().get_mut(&a).unwrap().iter_mut() {
            *t = Instant::now() - Duration::from_secs(61);
        }
        assert!(!m.pairing_blocked(a));
        assert!(m.failures.lock().get(&a).is_none(), "and so is the address");
        // The table is bounded.
        for i in 0..(LIMITER_ADDRESSES + 5) {
            m.pairing_failed(IpAddr::from([10, 0, (i / 256) as u8, (i % 256) as u8]));
        }
        assert!(m.failures.lock().len() <= LIMITER_ADDRESSES);
    }

    /// A daemon with a terminal session `t1` (a Claude Code that is not asking anything) and a
    /// chat `c1`, and a connection of the device `phone`.
    struct Rig {
        daemon: Arc<Daemon>,
        conn: Conn,
        out: mpsc::Receiver<String>,
        term: Arc<crate::session::Session>,
        typed: crate::session::fake::Typed,
        chat: Arc<crate::chat::ChatSession>,
    }

    fn rig() -> Rig {
        let dir = std::env::temp_dir().join(format!("kelpie-phone-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let daemon = Daemon::with_secrets(dir, std::path::PathBuf::from("kelpied"), crate::notify::Memory::new());
        let (term, typed) = crate::session::fake::running(1, "t1", "claude");
        daemon.insert_for_test(Entry::Term(term.clone()));
        let chat = crate::chat::ChatSession::new(2, crate::session::SessionSpec { id: "c1".into(), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
        daemon.insert_for_test(Entry::Chat(chat.clone()));
        let device = Device { id: "phone".into(), name: "Phone".into(), token_sha256: String::new(), can_write: true, created_at: 0, last_seen: 0 };
        daemon.mobile.config.lock().devices.push(device);
        daemon.mobile.config.lock().enabled = true;
        let (tx, _rx) = mpsc::unbounded_channel();
        let (out_tx, out) = mpsc::channel(16);
        let conn = Conn { daemon: daemon.clone(), device: "phone".into(), id: 1, tx, out: out_tx, followed: HashMap::new(), watched: HashSet::new(), chat_at: HashMap::new(), waiting: Arc::new(AtomicUsize::new(0)), list: None, rev: 0 };
        Rig { daemon, conn, out, term, typed, chat }
    }

    impl Rig {
        /// What the phone is told about a request (every request carries `req`, so there is always an answer).
        async fn ask(&mut self, granted: Permission, mut req: Value) -> Value {
            req["req"] = json!(1);
            self.conn.handle(granted, &req).await.expect("a reply")
        }

        async fn ask_write(&mut self, req: Value) -> Value {
            self.ask(Permission::Write, req).await
        }
    }

    /// Real loopback transports, isolated from the user's daemon and mobile settings.
    struct NetworkRig {
        rig: Rig,
        dir: PathBuf,
        addr: SocketAddr,
    }

    impl NetworkRig {
        async fn new() -> Self {
            let rig = rig();
            let dir = rig.daemon.mobile.path.parent().unwrap().to_path_buf();
            rig.daemon.mobile.config.lock().devices[0].token_sha256 = sha256_hex("test-phone-token");
            let mut network = Self { rig, dir, addr: "127.0.0.1:0".parse().unwrap() };
            network.restart().await;
            network
        }

        async fn restart(&mut self) {
            // Reserve an OS-selected port, never the daemon's default port.
            let reservation = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            self.addr = reservation.local_addr().unwrap();
            drop(reservation);
            enable(self.rig.daemon.clone(), json!({"ip": "127.0.0.1", "port": self.addr.port()})).await.unwrap();
        }

        async fn http(&self) -> tokio::net::TcpStream {
            tokio::net::TcpStream::connect(self.addr).await.unwrap()
        }

        async fn websocket(&self) -> tokio::net::TcpStream {
            use tokio::io::AsyncWriteExt;
            let mut tcp = self.http().await;
            tcp.write_all(format!(
                "GET /ws HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
                self.addr
            ).as_bytes()).await.unwrap();
            assert!(http_head(&mut tcp).await.starts_with("HTTP/1.1 101"));
            tcp
        }
    }

    impl Drop for NetworkRig {
        fn drop(&mut self) {
            self.rig.daemon.mobile.disable();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn http_head(tcp: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                bytes.push(tcp.read_u8().await.unwrap());
                assert!(bytes.len() < 16 * 1024);
            }
            String::from_utf8(bytes).unwrap()
        }).await.expect("HTTP response")
    }

    async fn phone_send(tcp: &mut tokio::net::TcpStream, value: Value) -> std::io::Result<()> {
        use tokio::io::AsyncWriteExt;
        let text = value.to_string();
        assert!(text.len() < 126);
        let mask = [17, 29, 43, 59];
        let mut frame = vec![0x81, 0x80 | text.len() as u8];
        frame.extend(mask);
        frame.extend(text.bytes().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        tcp.write_all(&frame).await
    }

    /// None means the transport ended or the server sent a WebSocket close, not a timeout.
    async fn phone_recv(tcp: &mut tokio::net::TcpStream) -> Option<Value> {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(Duration::from_secs(2), async {
            let opcode = match tcp.read_u8().await {
                Ok(byte) => byte & 0x0f,
                Err(_) => return None,
            };
            let size = tcp.read_u8().await.ok()? & 0x7f;
            let size = match size {
                126 => tcp.read_u16().await.ok()? as usize,
                127 => usize::try_from(tcp.read_u64().await.ok()?).unwrap(),
                n => n as usize,
            };
            assert!(size < 1024 * 1024);
            let mut body = vec![0; size];
            tcp.read_exact(&mut body).await.ok()?;
            if opcode == 8 {
                return None;
            }
            assert_eq!(opcode, 1, "expected a JSON text frame");
            Some(serde_json::from_slice(&body).unwrap())
        }).await.expect("mobile transport must respond or close promptly")
    }

    async fn authenticate(tcp: &mut tokio::net::TcpStream) {
        phone_send(tcp, json!({"token": "test-phone-token"})).await.unwrap();
        assert_eq!(phone_recv(tcp).await.unwrap()["type"], "hello");
    }

    #[tokio::test]
    async fn security_regression_mobile_disable_ends_authenticated_socket_and_output() {
        let mut network = NetworkRig::new().await;
        let mut phone = network.websocket().await;
        authenticate(&mut phone).await;
        phone_send(&mut phone, json!({"type": "input", "id": "t1", "text": "before", "req": 1})).await.unwrap();
        assert_eq!(phone_recv(&mut phone).await.unwrap()["type"], "ack");
        assert_eq!(network.rig.typed.writes(), vec![b"before".to_vec()]);

        network.rig.daemon.mobile.disable();
        // A queued event must not leak through an already authenticated connection.
        network.rig.daemon.broadcast("session.updated", json!({"id": "t1"}));
        let _ = phone_send(&mut phone, json!({"type": "input", "id": "t1", "text": "after", "req": 2})).await;
        assert!(phone_recv(&mut phone).await.is_none(), "disabled sockets must not reply or publish events");
        assert!(network.rig.typed.writes().is_empty(), "disable removes write authority");
        assert!(network.rig.daemon.mobile.device_for("test-phone-token").is_none());
        assert_eq!(network.rig.daemon.mobile.permission("phone"), None);

        network.restart().await;
        let mut fresh = network.websocket().await;
        authenticate(&mut fresh).await;
        phone_send(&mut fresh, json!({"type": "list", "req": 3})).await.unwrap();
        assert!(phone_recv(&mut fresh).await.is_some(), "paired device reconnects on the new server");
        assert!(phone_recv(&mut phone).await.is_none(), "old socket cannot revive");
    }

    #[tokio::test]
    async fn security_regression_mobile_shutdown_is_sticky_before_websocket_authentication() {
        let mut network = NetworkRig::new().await;
        // The upgrade has finished, but its token has not arrived when shutdown wins.
        let mut old = network.websocket().await;
        network.rig.daemon.mobile.disable();
        network.restart().await;
        let _ = phone_send(&mut old, json!({"token": "test-phone-token"})).await;
        assert!(phone_recv(&mut old).await.is_none(), "old generation cannot authenticate after re-enable");
        let mut fresh = network.websocket().await;
        authenticate(&mut fresh).await;
    }

    #[tokio::test]
    async fn security_regression_mobile_disable_closes_accepted_http_connection() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let network = NetworkRig::new().await;
        let mut http = network.http().await;
        // HEAD has no body: consuming the headers leaves an idle, established keep-alive connection.
        let request = format!("HEAD / HTTP/1.1\r\nHost: {}\r\n\r\n", network.addr);
        http.write_all(request.as_bytes()).await.unwrap();
        assert!(http_head(&mut http).await.starts_with("HTTP/1.1 200"));
        network.rig.daemon.mobile.disable();
        let _ = http.write_all(request.as_bytes()).await;
        let mut byte = [0];
        let ended = tokio::time::timeout(Duration::from_secs(2), http.read(&mut byte)).await
            .expect("disabled accepted HTTP connection must close");
        assert!(matches!(ended, Ok(0) | Err(_)), "old HTTP connection still serves requests");
    }

    fn code(reply: &Value) -> Option<&str> {
        reply["code"].as_str()
    }

    #[tokio::test]
    async fn every_verb_is_refused_below_its_permission_and_unknown_ones_always() {
        let mut r = rig();
        let requests = [
            json!({"type": "list"}),
            json!({"type": "sub", "id": "t1"}),
            json!({"type": "unsub", "id": "t1"}),
            json!({"type": "chat_sub", "id": "c1"}),
            json!({"type": "input", "id": "t1", "text": "hi"}),
            json!({"type": "key", "id": "t1", "key": "enter"}),
            json!({"type": "answer", "id": "t1", "waiting_since": 1, "text": "1"}),
            json!({"type": "chat_answer", "id": "c1", "request_id": "r", "line": "{}"}),
        ];
        for req in &requests {
            let name = req["type"].as_str().unwrap();
            let reply = r.ask(Permission::Read, req.clone()).await;
            let writes = Verb::parse(name).unwrap().needs() == Permission::Write;
            assert_eq!(code(&reply) == Some("read_only"), writes, "{name} as a reader: {reply}");
            assert!(r.typed.writes().is_empty(), "{name}: a reader typed something");
            assert_ne!(code(&reply), Some("unknown_verb"), "{name}");
        }
        for name in ["session.input", "session.update", "session.attach", "session.close", "mobile.revoke", "read", "daemon.shutdown", ""] {
            for granted in [Permission::Read, Permission::Write] {
                let reply = r.ask(granted, json!({"type": name, "id": "t1", "handle": 1, "data": "x"})).await;
                assert_eq!((reply["type"].as_str(), code(&reply)), (Some("error"), Some("unknown_verb")), "{name}");
            }
        }
        assert!(r.typed.writes().is_empty());
        // A request without a type is not answered at all.
        assert!(r.conn.handle(Permission::Write, &json!({"id": "t1"})).await.is_none());
    }

    #[tokio::test]
    async fn the_device_record_decides_not_the_connection() {
        let r = rig();
        assert_eq!(r.daemon.mobile.permission("phone"), Some(Permission::Write));
        r.daemon.mobile.set_write("phone", false).unwrap();
        assert_eq!(r.daemon.mobile.permission("phone"), Some(Permission::Read));
        r.daemon.mobile.revoke("phone").unwrap();
        assert_eq!(r.daemon.mobile.permission("phone"), None);
    }

    #[tokio::test]
    async fn typing_from_the_phone_is_cleaned_and_goes_through_the_agents_enter_delay() {
        let mut r = rig();
        let began = Instant::now();
        let reply = r.ask_write(json!({"type": "input", "id": "t1", "text": "a\u{1b}[201~b", "enter": true})).await;
        assert_eq!(reply["type"], "ack");
        assert!(began.elapsed() >= crate::session::ENTER_DELAY, "the reply waits for the Enter");
        assert_eq!(r.typed.writes(), vec![b"a[201~b".to_vec(), b"\r".to_vec()], "the paste break-out is gone, and Enter is a write of its own");
        // Limits and the session's state.
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "t1", "text": "x".repeat(MAX_TYPED + 1)})).await), Some("too_long"));
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "nope", "text": "x"})).await), Some("no_session"));
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "c1", "text": "x"})).await), Some("chat"));
        assert_eq!(code(&r.ask_write(json!({"type": "key", "id": "t1", "key": "rm -rf"})).await), Some("bad_request"));
        assert_eq!(r.ask_write(json!({"type": "key", "id": "t1", "key": "ctrl+d"})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![vec![4u8]]);
        crate::session::fake::stop(&r.term);
        assert_eq!(code(&r.ask_write(json!({"type": "input", "id": "t1", "text": "x"})).await), Some("exited"));
    }

    #[tokio::test]
    async fn text_is_refused_while_a_prompt_is_shown_unless_forced() {
        let mut r = rig();
        crate::session::fake::show_prompt(&r.term);
        let reply = r.ask_write(json!({"type": "input", "id": "t1", "text": "hello", "enter": false})).await;
        assert_eq!(code(&reply), Some("prompt"), "{reply}");
        assert!(r.typed.writes().is_empty());
        assert_eq!(r.ask_write(json!({"type": "input", "id": "t1", "text": "hello", "force": true})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![b"hello".to_vec()]);
    }

    #[tokio::test]
    async fn an_answer_lands_once_and_never_after_the_desktop_answered() {
        let mut r = rig();
        let since = crate::session::fake::wait_on_user(&r.term);
        let answer = json!({"type": "answer", "id": "t1", "waiting_since": since, "text": "1"});
        assert_eq!(r.ask_write(answer.clone()).await["type"], "ack");
        // The second tap, and the same from another phone: stale.
        let second = r.ask_write(answer.clone()).await;
        assert_eq!((code(&second), second["reason"].as_str()), (Some("stale"), Some("answered")));
        assert_eq!(r.typed.writes(), vec![b"1".to_vec()]);

        // A hook wait the user answered at the keyboard stays `waiting` until the next hook.
        let (term2, typed2) = crate::session::fake::running(3, "t2", "claude");
        r.daemon.insert_for_test(Entry::Term(term2.clone()));
        let since = crate::session::fake::wait_on_user(&term2);
        term2.input(b"2".to_vec());
        let reply = r.ask_write(json!({"type": "answer", "id": "t2", "waiting_since": since, "text": "1"})).await;
        assert_eq!((code(&reply), reply["reason"].as_str()), (Some("stale"), Some("answered")));
        assert_eq!(typed2.writes(), vec![b"2".to_vec()], "only the desktop's key reached the agent");

        // A prompt that is not the one the phone showed.
        let (term3, typed3) = crate::session::fake::running(4, "t3", "claude");
        r.daemon.insert_for_test(Entry::Term(term3.clone()));
        let since = crate::session::fake::wait_on_user(&term3);
        let reply = r.ask_write(json!({"type": "answer", "id": "t3", "waiting_since": since - 1, "text": "1"})).await;
        assert_eq!(reply["reason"].as_str(), Some("moved"));
        assert_eq!(code(&r.ask_write(json!({"type": "answer", "id": "t3", "waiting_since": since, "key": "tab"})).await), Some("bad_request"));
        assert_eq!(code(&r.ask_write(json!({"type": "answer", "id": "t3", "waiting_since": since})).await), Some("bad_request"));
        assert!(typed3.writes().is_empty());
    }

    #[tokio::test]
    async fn requests_from_before_a_restart_are_dropped() {
        let mut r = rig();
        let epoch = r.daemon.mobile.epoch.clone();
        let stale = r.ask_write(json!({"type": "input", "id": "t1", "text": "x", "epoch": "an older daemon"})).await;
        assert_eq!(code(&stale), Some("epoch"));
        assert!(r.typed.writes().is_empty());
        assert_eq!(r.ask_write(json!({"type": "input", "id": "t1", "text": "x", "epoch": epoch})).await["type"], "ack");
        assert_eq!(r.typed.writes(), vec![b"x".to_vec()]);
    }

    #[tokio::test]
    async fn a_chat_permission_is_answered_once_across_phone_and_desktop() {
        let mut r = rig();
        r.chat.ingest_for_test(json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "can_use_tool"}}).to_string());
        let line = json!({"type": "control_response", "response": {"subtype": "success", "request_id": "r1", "response": {"behavior": "allow"}}}).to_string();
        let answer = json!({"type": "chat_answer", "id": "c1", "request_id": "r1", "line": line});
        // A request the agent never made cannot be answered, whatever the line.
        let unasked = json!({"type": "chat_answer", "id": "c1", "request_id": "r0", "line": line.replace("r1", "r0")});
        assert_eq!(code(&r.ask_write(unasked).await), Some("bad_answer"));
        assert_eq!(r.ask_write(answer.clone()).await["type"], "ack");
        let again = r.ask_write(answer).await;
        assert_eq!((code(&again), again["reason"].as_str()), (Some("stale"), Some("answered")));
        // The desktop's answer to it is dropped too (the chat checks every line written to the agent).
        r.chat.input(format!("{line}\n").into_bytes());
        // A line that is not that request's answer is not an answer.
        let other = json!({"type": "chat_answer", "id": "c1", "request_id": "r2", "line": line});
        assert_eq!(code(&r.ask_write(other).await), Some("bad_answer"));
        let message = json!({"type": "chat_answer", "id": "c1", "request_id": "r2", "line": json!({"type": "user", "message": {"role": "user", "content": "rm -rf"}}).to_string()});
        assert_eq!(code(&r.ask_write(message).await), Some("bad_answer"));
        assert_eq!(code(&r.ask_write(json!({"type": "chat_answer", "id": "t1", "request_id": "r2", "line": line})).await), Some("not_chat"));
    }

    #[tokio::test]
    async fn the_list_is_pushed_when_it_changes_and_carries_the_waiting_prompt() {
        let mut r = rig();
        let first = r.conn.publish(false).expect("first list");
        assert_eq!(first["rev"], 1);
        let ids: Vec<&str> = first["sessions"].as_array().unwrap().iter().map(|s| s["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["t1", "c1"]);
        assert!(first["sessions"][0].get("handle").is_none(), "sessions are addressed by id");
        assert!(r.conn.publish(false).is_none(), "nothing changed, nothing pushed");
        crate::session::fake::wait_on_user(&r.term);
        let second = r.conn.publish(false).expect("a change is pushed");
        assert_eq!(second["rev"], 2);
        assert_eq!(second["sessions"][0]["status"], "waiting");
        assert!(second["sessions"][0]["waiting_since"].as_u64().is_some());
        // Asking for it gives it again.
        assert_eq!(r.ask_write(json!({"type": "list"})).await["rev"], 3);
    }

    async fn next_frame(out: &mut mpsc::Receiver<String>) -> Option<Value> {
        let text = tokio::time::timeout(Duration::from_secs(2), out.recv()).await.ok()??;
        serde_json::from_str(&text).ok()
    }

    #[tokio::test]
    async fn a_followed_session_sends_frames_until_the_phone_lets_go() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "sub", "id": "t1", "height": 20})).await["type"], "ack");
        let first = next_frame(&mut r.out).await.unwrap();
        assert_eq!((first["type"].as_str(), first["id"].as_str(), first["base"].clone()), (Some("screen"), Some("t1"), Value::Null));
        r.term.ingest(b"hello\r\n");
        let next = next_frame(&mut r.out).await.unwrap();
        assert_eq!((next["base"].clone(), next["rows"].clone()), (first["rev"].clone(), json!([[0, "hello"]])));
        assert_eq!(code(&r.ask_write(json!({"type": "sub", "id": "c1"})).await), Some("chat"));
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "t1"})).await["type"], "ack");
        tokio::time::sleep(Duration::from_millis(150)).await;
        while r.out.try_recv().is_ok() {}
        r.term.ingest(b"more\r\n");
        assert!(tokio::time::timeout(Duration::from_millis(400), r.out.recv()).await.is_err(), "nothing for a session nobody follows");
        // A phone cannot follow more than it can show.
        for i in 0..MAX_FOLLOWED {
            let (s, _typed) = crate::session::fake::running(10 + i as u32, &format!("many-{i}"), "shell");
            r.daemon.insert_for_test(Entry::Term(s));
            let reply = r.ask_write(json!({"type": "sub", "id": format!("many-{i}")})).await;
            assert_eq!(reply["type"], "ack", "{i}");
        }
        assert_eq!(code(&r.ask_write(json!({"type": "sub", "id": "t1"})).await), Some("too_many"));
        r.conn.end();
    }

    #[tokio::test]
    async fn the_daemons_events_reach_the_phone_as_messages() {
        let r = rig();
        let dirty = Notify::new();
        let mut t = Translator::default();
        let event = |name: &str, params: Value| crate::session::json_out(&json!({"event": name, "params": params})).data;
        for name in ["session.updated", "session.closed"] {
            assert!(t.translate(&r.daemon, &event(name, json!({})), &dirty).is_empty());
            tokio::time::timeout(Duration::from_millis(50), dirty.notified()).await.unwrap_or_else(|_| panic!("{name} did not wake the list"));
        }
        assert!(t.translate(&r.daemon, &event("notify.event", json!({"kind": "waiting"})), &dirty).is_empty());
        assert!(tokio::time::timeout(Duration::from_millis(50), dirty.notified()).await.is_err(), "other events are not about the list");

        let resolved = t.translate(&r.daemon, &event("chat.resolved", json!({"handle": 2, "id": "c1", "request_id": "r1", "by": "phone", "dropped": false})), &dirty);
        assert_eq!(resolved, vec![json!({"type": "chat_resolved", "id": "c1", "request_id": "r1", "by": "phone", "dropped": false})]);
        let resync = t.translate(&r.daemon, &event("session.resync", json!({"handle": 2})), &dirty);
        assert_eq!(resync, vec![json!({"type": "chat_resync", "id": "c1"})]);

        // Log lines of a watched chat, live: addressed by id, not handle.
        let frame = kelpie_proto::Frame::Output { handle: 2, seq: 40, data: b"{\"a\":1}\n{\"b\":2}\n".to_vec() }.encode();
        assert_eq!(t.translate(&r.daemon, &frame, &dirty), vec![json!({"type": "chat", "id": "c1", "mode": "live", "seq": 40, "lines": ["{\"a\":1}", "{\"b\":2}"]})]);
        // The reply to an attach announces the log, which follows as the next output frame of that chat.
        let reply = crate::session::json_out(&json!({"id": "chat:c1", "result": {"mode": "delta", "raw": true, "seq": 9, "handle": 2, "observer": true}})).data;
        assert!(t.translate(&r.daemon, &reply, &dirty).is_empty());
        let log = kelpie_proto::Frame::Output { handle: 2, seq: 9, data: b"{\"a\":1}\n".to_vec() }.encode();
        assert_eq!(t.translate(&r.daemon, &log, &dirty), vec![json!({"type": "chat", "id": "c1", "mode": "delta", "seq": 9, "lines": ["{\"a\":1}"]})]);
        // After that it is live again.
        assert_eq!(t.translate(&r.daemon, &frame, &dirty)[0]["mode"], "live");
        // Replies to anything else are none of the phone's business.
        let other = crate::session::json_out(&json!({"id": 5, "result": true})).data;
        assert!(t.translate(&r.daemon, &other, &dirty).is_empty());
    }

    #[test]
    fn a_long_log_goes_out_in_messages_that_each_say_where_they_end() {
        let line = format!("{{\"text\":\"{}\"}}", "x".repeat(30 * 1024));
        let data = format!("{line}\n{line}\n{line}\n").into_bytes();
        let size = line.len() as u64 + 1;
        let messages = chat_messages("c1", "reset", 100_000, &data);
        // Two lines fit in a message, the third starts the next.
        assert_eq!(messages.iter().map(|m| m["lines"].as_array().unwrap().len()).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(messages.iter().map(|m| m["mode"].as_str().unwrap()).collect::<Vec<_>>(), ["reset", "more"]);
        assert_eq!(messages[0]["seq"].as_u64(), Some(100_000 - size));
        assert_eq!(messages[1]["seq"].as_u64(), Some(100_000));
        // A line larger than a message goes alone; nothing is lost or cut.
        let big = format!("{{\"text\":\"{}\"}}", "y".repeat(CHAT_MESSAGE_BYTES * 2));
        let messages = chat_messages("c1", "live", 5, format!("{big}\n").as_bytes());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0]["lines"][0].as_str().unwrap().len(), big.len());
        // No lines is still an answer: the phone knows it is up to date.
        let none = chat_messages("c1", "delta", 77, b"");
        assert_eq!((none.len(), none[0]["seq"].as_u64(), none[0]["lines"].as_array().unwrap().len()), (1, Some(77), 0));
    }

    #[tokio::test]
    async fn a_watched_chat_that_closed_is_let_go_and_does_not_fill_the_connection() {
        let mut r = rig();
        for i in 0..MAX_FOLLOWED {
            let chat = crate::chat::ChatSession::new(100 + i as u32, crate::session::SessionSpec { id: format!("gone-{i}"), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
            r.daemon.insert_for_test(Entry::Chat(chat));
            assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": format!("gone-{i}")})).await["type"], "ack", "{i}");
        }
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("too_many"));
        // They are closed on the computer; the phone lets go of one, and the others no longer count.
        for i in 0..MAX_FOLLOWED {
            r.daemon.remove_for_test(100 + i as u32);
        }
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "gone-0"})).await["type"], "ack");
        assert!(!r.conn.watched.contains("gone-0"));
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        assert_eq!(r.conn.watched.len(), 1, "{:?}", r.conn.watched);
        // Letting go of something that is not there is not an error either.
        assert_eq!(r.ask_write(json!({"type": "unsub", "id": "never-was"})).await["type"], "ack");
    }

    #[tokio::test]
    async fn asking_for_a_chat_again_at_once_is_refused() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        assert_eq!(code(&r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await), Some("too_fast"));
        r.conn.chat_at.insert("c1".into(), Instant::now() - CHAT_SUB_GAP);
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
    }

    #[tokio::test]
    async fn two_messages_sent_at_once_are_two_submits() {
        let mut r = rig();
        r.ask_write(json!({"type": "input", "id": "t1", "text": "A", "enter": true})).await;
        r.ask_write(json!({"type": "input", "id": "t1", "text": "B", "enter": true})).await;
        // Each is followed by its own Enter before the next is typed.
        assert_eq!(r.typed.writes(), vec![b"A".to_vec(), b"\r".to_vec(), b"B".to_vec(), b"\r".to_vec()]);
    }

    #[tokio::test]
    async fn a_permission_change_or_a_missed_event_reconnects_and_only_a_removal_unpairs() {
        let r = rig();
        let mut events = r.daemon.mobile.revoked.subscribe();
        r.daemon.mobile.set_write("phone", false).unwrap();
        assert_eq!(closing(events.recv().await, "phone"), Some((RECONNECT, "changed")), "still paired: the page keeps its token");
        // Somebody else's change is none of this connection's business.
        r.daemon.mobile.set_write("phone", true).unwrap();
        assert_eq!(closing(events.recv().await, "other"), None);
        // Falling behind may have missed a removal or a change: reconnect, which finds out.
        let (tx, mut slow) = broadcast::channel::<(String, bool)>(2);
        for _ in 0..5 {
            tx.send(("x".into(), false)).unwrap();
        }
        assert_eq!(closing(slow.recv().await, "phone"), Some((RECONNECT, "behind")));
        // Only a removed device is unpaired.
        r.daemon.mobile.revoke("phone").unwrap();
        assert_eq!(closing(events.recv().await, "phone"), Some((NOT_PAIRED, "revoked")));
        assert_ne!(NOT_PAIRED, RECONNECT);
        assert_ne!(RECONNECT, REPLACED);
    }

    #[tokio::test]
    async fn a_close_that_cannot_leave_is_given_up_on() {
        assert!(!within(Duration::from_millis(50), std::future::pending::<()>()).await);
        assert!(within(Duration::from_millis(50), async {}).await);
        assert_eq!(CLOSE_TIMEOUT, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn a_message_whose_enter_was_skipped_says_so() {
        let mut r = rig();
        // Someone types at the keyboard during the delay: the Enter would submit their text too.
        let term = r.term.clone();
        let typist = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            term.input(b"x".to_vec());
        });
        let reply = r.ask_write(json!({"type": "input", "id": "t1", "text": "hello", "enter": true})).await;
        typist.await.unwrap();
        assert_eq!((reply["type"].as_str(), reply["entered"].clone(), reply["req"].clone()), (Some("ack"), json!(false), json!(1)));
        assert_eq!(r.typed.writes(), vec![b"hello".to_vec(), b"x".to_vec()], "no Enter followed");
    }

    #[tokio::test]
    async fn repeating_chat_sub_without_reading_is_refused_as_busy() {
        let mut r = rig();
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
        r.conn.chat_at.clear();
        // The first log has not been taken by the socket (nothing drains the connection here).
        r.conn.waiting.store(crate::chat::RESET_WAITING_MAX, std::sync::atomic::Ordering::Relaxed);
        let reply = r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await;
        assert_eq!(code(&reply), Some("busy"), "{reply}");
        r.conn.waiting.store(0, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(r.ask_write(json!({"type": "chat_sub", "id": "c1"})).await["type"], "ack");
    }

}
