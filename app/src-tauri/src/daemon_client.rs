//! Connection from the app to `kelpied`.
//!
//! - Spawns the daemon detached (it must outlive the app) when nothing is listening.
//! - One reader task demultiplexes frames: replies → pending calls, events → `daemon-event`,
//!   terminal output → the attached view's binary `Channel`.
//! - Output for a view is coalesced for ~4 ms so a chatty agent costs the webview at most a
//!   few hundred IPC messages per second instead of one per PTY read.
//! - Reconnects (and respawns the daemon) if the connection drops.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Result};
use kelpie_proto::{read_frame, transport, Frame};
use interprocess::local_socket::traits::tokio::Stream as _;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot, Notify};

const FLUSH_DELAY: Duration = Duration::from_millis(4);

struct View {
    channel: Channel<InvokeResponseBody>,
    /// Request id of the attach that owns this view; only its reply may open the stream.
    attach_id: u64,
    /// Output frames older than the pending attach reply are stale and dropped.
    awaiting_reply: bool,
    buf: Vec<u8>,
    seq: u64,
}

enum Pending {
    Call(oneshot::Sender<Result<Value, String>>),
    Attach(u32, oneshot::Sender<Result<Value, String>>),
}

pub struct DaemonClient {
    app: AppHandle,
    out: Mutex<Option<mpsc::UnboundedSender<Vec<u8>>>>,
    pending: Mutex<HashMap<u64, Pending>>,
    views: Mutex<HashMap<u32, View>>,
    dirty: Mutex<HashSet<u32>>,
    flush: Notify,
    next_id: AtomicU64,
    connected: AtomicBool,
    connected_notify: Notify,
}

impl DaemonClient {
    pub fn start(app: AppHandle) -> Arc<DaemonClient> {
        let client = Arc::new(DaemonClient {
            app,
            out: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            views: Mutex::new(HashMap::new()),
            dirty: Mutex::new(HashSet::new()),
            flush: Notify::new(),
            next_id: AtomicU64::new(1),
            connected: AtomicBool::new(false),
            connected_notify: Notify::new(),
        });
        tauri::async_runtime::spawn(client.clone().connection_loop());
        tauri::async_runtime::spawn(client.clone().flush_loop());
        client
    }

    async fn connection_loop(self: Arc<Self>) {
        let endpoint = kelpie_proto::endpoint();
        let mut spawned_at: Option<std::time::Instant> = None;
        loop {
            match transport::connect(&endpoint).await {
                Ok(stream) => {
                    spawned_at = None;
                    let (rx, mut tx) = stream.split();
                    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Vec<u8>>();
                    // First frame on the connection: the app speaks as the user (never as a
                    // session, even when started from a Kelpie terminal). A daemon older than
                    // tokens answers with an error the read loop ignores.
                    if let Some(token) = kelpie_proto::read_user_token(&endpoint) {
                        let _ = out_tx.send(Frame::json(&json!({"id": "auth", "method": "auth", "params": {"token": token, "client": "app"}})).encode());
                    }
                    *self.out.lock() = Some(out_tx);
                    self.connected.store(true, Ordering::SeqCst);
                    self.connected_notify.notify_waiters();
                    let _ = self.app.emit("daemon-status", json!({"connected": true}));
                    let writer = tauri::async_runtime::spawn(async move {
                        while let Some(mut bytes) = out_rx.recv().await {
                            while let Ok(more) = out_rx.try_recv() {
                                bytes.extend_from_slice(&more);
                            }
                            if tx.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                    });
                    self.read_loop(tokio::io::BufReader::with_capacity(256 * 1024, rx)).await;
                    writer.abort();
                    self.on_disconnect();
                }
                Err(_) => {
                    // Spawn at most once per 5 s; the daemon may take a moment to bind.
                    if spawned_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
                        if let Err(e) = spawn_daemon() {
                            tracing::error!("spawn kelpied: {e:#}");
                            let _ = self.app.emit("daemon-status", json!({"connected": false, "error": format!("{e:#}")}));
                        }
                        spawned_at = Some(std::time::Instant::now());
                    }
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            }
        }
    }

    fn on_disconnect(&self) {
        self.connected.store(false, Ordering::SeqCst);
        *self.out.lock() = None;
        for (_, p) in self.pending.lock().drain() {
            match p {
                Pending::Call(tx) | Pending::Attach(_, tx) => {
                    let _ = tx.send(Err("daemon disconnected".into()));
                }
            }
        }
        // Views must re-attach (with their last seq) once reconnected.
        self.views.lock().clear();
        let _ = self.app.emit("daemon-status", json!({"connected": false}));
    }

    async fn read_loop<R: tokio::io::AsyncRead + Unpin>(&self, mut rx: R) {
        loop {
            let frame = match read_frame(&mut rx).await {
                Ok(Some(f)) => f,
                _ => return,
            };
            match frame {
                Frame::Output { handle, seq, data } => {
                    let mut views = self.views.lock();
                    if let Some(view) = views.get_mut(&handle) {
                        if !view.awaiting_reply {
                            view.buf.extend_from_slice(&data);
                            view.seq = seq;
                            drop(views);
                            if self.dirty.lock().insert(handle) {
                                self.flush.notify_one();
                            }
                        }
                    }
                }
                Frame::Json(body) => {
                    let Ok(msg) = serde_json::from_slice::<Value>(&body) else { continue };
                    if let Some(id) = msg.get("id").and_then(Value::as_u64) {
                        let result = match msg.get("error") {
                            Some(err) => Err(err["message"].as_str().unwrap_or("error").to_string()),
                            None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        match self.pending.lock().remove(&id) {
                            Some(Pending::Call(tx)) => {
                                let _ = tx.send(result);
                            }
                            Some(Pending::Attach(handle, tx)) => {
                                if let Some(view) = self.views.lock().get_mut(&handle).filter(|v| v.attach_id == id) {
                                    view.awaiting_reply = false;
                                    view.buf.clear();
                                    if let Ok(r) = &result {
                                        view.seq = r["seq"].as_u64().unwrap_or(0);
                                    }
                                }
                                let _ = tx.send(result);
                            }
                            None => {}
                        }
                    } else if msg.get("event").is_some() {
                        let _ = self.app.emit("daemon-event", msg);
                    }
                }
                Frame::Input { .. } => {}
            }
        }
    }

    async fn flush_loop(self: Arc<Self>) {
        loop {
            self.flush.notified().await;
            tokio::time::sleep(FLUSH_DELAY).await;
            let handles: Vec<u32> = self.dirty.lock().drain().collect();
            let mut views = self.views.lock();
            for handle in handles {
                if let Some(view) = views.get_mut(&handle) {
                    if view.buf.is_empty() {
                        continue;
                    }
                    // Message layout: u64 LE seq (stream offset after this data) | bytes.
                    let mut msg = Vec::with_capacity(8 + view.buf.len());
                    msg.extend_from_slice(&view.seq.to_le_bytes());
                    msg.append(&mut view.buf);
                    let _ = view.channel.send(InvokeResponseBody::Raw(msg));
                }
            }
        }
    }

    async fn wait_connected(&self) -> Result<()> {
        for _ in 0..100 {
            if self.connected.load(Ordering::SeqCst) {
                return Ok(());
            }
            let _ = tokio::time::timeout(Duration::from_millis(100), self.connected_notify.notified()).await;
        }
        Err(anyhow!("kelpied is not reachable"))
    }

    fn send_frame(&self, frame: &Frame) -> Result<()> {
        let out = self.out.lock();
        let tx = out.as_ref().ok_or_else(|| anyhow!("daemon disconnected"))?;
        tx.send(frame.encode()).map_err(|_| anyhow!("daemon disconnected"))
    }

    async fn request(&self, method: &str, params: Value, pending: impl FnOnce(oneshot::Sender<Result<Value, String>>) -> Pending) -> Result<Value> {
        self.wait_connected().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.request_with_id(id, method, params, pending).await
    }

    async fn request_with_id(&self, id: u64, method: &str, params: Value, pending: impl FnOnce(oneshot::Sender<Result<Value, String>>) -> Pending) -> Result<Value> {
        let (tx, rx) = oneshot::channel();
        // A one-shot agent answer may take as long as its own timeout (daemon default 180 s).
        let wait = if method == "agent.run" { params.get("timeout_s").and_then(Value::as_u64).unwrap_or(180).clamp(5, 900) + 15 } else { 30 };
        self.pending.lock().insert(id, pending(tx));
        if let Err(e) = self.send_frame(&Frame::json(&json!({"id": id, "method": method, "params": params}))) {
            self.pending.lock().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(Duration::from_secs(wait), rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!(e)),
            _ => {
                self.pending.lock().remove(&id);
                Err(anyhow!("{method}: no reply from daemon"))
            }
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.request(method, params, Pending::Call).await
    }

    /// Registers the view's channel, then attaches. The reply carries the initial content
    /// (`mode` delta|reset, base64 `data`); live output follows on the channel.
    pub async fn attach(&self, handle: u32, since: Option<u64>, channel: Channel<InvokeResponseBody>) -> Result<Value> {
        self.wait_connected().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.views.lock().insert(handle, View { channel, attach_id: id, awaiting_reply: true, buf: Vec::new(), seq: 0 });
        let params = json!({"handle": handle, "since_seq": since, "interactive": true});
        self.request_with_id(id, "session.attach", params, |tx| Pending::Attach(handle, tx)).await
    }

    pub async fn detach(&self, handle: u32) -> Result<()> {
        self.views.lock().remove(&handle);
        self.call("session.detach", json!({"handle": handle})).await.map(|_| ())
    }

    pub fn input(&self, handle: u32, data: Vec<u8>) -> Result<()> {
        self.send_frame(&Frame::Input { handle, data })
    }
}

/// `kelpied` next to our executable (or `KELPIE_DAEMON_PATH`).
pub fn daemon_exe() -> std::path::PathBuf {
    std::env::var_os("KELPIE_DAEMON_PATH").map(std::path::PathBuf::from).unwrap_or_else(|| {
        let dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf())).unwrap_or_default();
        dir.join(if cfg!(windows) { "kelpied.exe" } else { "kelpied" })
    })
}

/// Starts `kelpied`, fully detached so closing the app (or its console job on Windows) never
/// takes the daemon — and every agent — down with it; on Windows from a copy outside the install
/// folder, so updating Kelpie never has to stop it (`daemon_runtime`).
fn spawn_daemon() -> Result<()> {
    let exe = crate::daemon_runtime::runnable(&daemon_exe());
    if !exe.exists() {
        return Err(anyhow!("daemon binary not found at {}", exe.display()));
    }
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // From an AppImage: sessions must not inherit its mount or its bundled GTK setup.
    if cfg!(target_os = "linux") {
        let vars: Vec<(String, String)> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
        for (name, value) in crate::daemon_runtime::appimage_env_fixes(&vars) {
            match value {
                Some(v) => cmd.env(name, v),
                None => cmd.env_remove(name),
            };
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        let base = CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW;
        cmd.creation_flags(base | CREATE_BREAKAWAY_FROM_JOB);
        if cmd.spawn().is_ok() {
            return Ok(());
        }
        // Breakaway is denied when our job forbids it; fall back to a plain detached spawn.
        cmd.creation_flags(base);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()?;
    Ok(())
}
