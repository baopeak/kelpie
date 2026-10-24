//! Structured ("chat mode") agent processes: Claude `-p --input-format stream-json`, Codex
//! `app-server`, ACP agents (`omp acp`). The daemon is a protocol-neutral JSON-lines relay —
//! adapters in the UI speak each dialect — plus a tiny classifier so the dashboard knows when
//! a chat needs the user even while no window is attached.
//!
//! Lines travel as `KIND_OUTPUT` frames (one or more `\n`-terminated lines per frame); the
//! log is replayed in full on attach so a reopened window rebuilds the conversation.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use base64::Engine as _;
use kelpie_proto::Frame;
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use crate::proc_tree::{Priority, ProcTree};
use crate::session::{json_out, ClientTx, Outgoing, SessionInfo, SessionSpec, SUB_BUDGET};
use crate::status::{exit_failed, Activity, Status, MIN_WORK_FOR_UNREAD};

const LOG_MAX_LINES: usize = 20_000;
/// Base64 of the replayed log must stay well under `kelpie_proto::MAX_FRAME_LEN`.
const LOG_MAX_BYTES: usize = 12 << 20;
const MAX_LINE: usize = 4 << 20;
/// Handshake responses the UI adapters need to rebuild state (and the agents' command, model
/// and skill lists the composer offers); kept when the log is trimmed.
const PINNED_IDS: &[&str] = &[
    r#""id":"kelpie-init""#,
    r#""id":"kelpie-thread""#,
    r#""id":"kelpie-session""#,
    r#""id":"kelpie-models""#,
    r#""id":"kelpie-skills""#,
    r#""request_id":"kelpie-init""#,
    r#""sessionUpdate":"available_commands_update""#,
];

struct Sub {
    client: u64,
    tx: ClientTx,
    credit: Arc<AtomicUsize>,
    stalled: bool,
}

struct Inner {
    /// Lines kept from trimmed parts of the log: the latest of each `PINNED_IDS` kind.
    pinned: Vec<(usize, Arc<[u8]>)>,
    pinned_bytes: usize,
    log: VecDeque<Arc<[u8]>>,
    log_bytes: usize,
    seq: u64,
    subs: Vec<Sub>,
    stdin: Option<tokio::sync::mpsc::UnboundedSender<Vec<u8>>>,
    tree: Option<Arc<ProcTree>>,
    pid: Option<u32>,
    alive: bool,
    exit_code: Option<i32>,
    /// Kelpie stopped the process, so its exit code says nothing about failure.
    stopped: bool,
    activity: Activity,
    focused: bool,
    unread: bool,
    last_status: Status,
    last_broadcast: Option<SessionInfo>,
    working_since: Option<Instant>,
    last_activity: u64,
    /// `last_activity` as of the last broadcast; output alone re-broadcasts at most every 30 s.
    broadcast_activity: u64,
    interrupted: bool,
    /// When the log last carried a `kelpie_time` line (unix ms).
    last_stamp: u64,
    /// The conversation's title, as the agent names it (ACP `session_info_update`).
    title: Option<String>,
    /// Wall-clock ms the current wait on the user began.
    waiting_since: Option<u64>,
}

pub struct ChatSession {
    pub handle: u32,
    pub spec: Mutex<SessionSpec>,
    inner: Mutex<Inner>,
}

impl ChatSession {
    pub fn new(handle: u32, spec: SessionSpec) -> Arc<ChatSession> {
        Arc::new(ChatSession {
            handle,
            spec: Mutex::new(spec),
            inner: Mutex::new(Inner {
                pinned: Vec::new(),
                pinned_bytes: 0,
                log: VecDeque::new(),
                log_bytes: 0,
                seq: 0,
                subs: Vec::new(),
                stdin: None,
                tree: None,
                pid: None,
                alive: false,
                exit_code: None,
                activity: Activity { alive: false, ..Default::default() },
                focused: false,
                unread: false,
                last_status: Status::Exited,
                stopped: false,
                last_broadcast: None,
                working_since: None,
                last_activity: 0,
                broadcast_activity: 0,
                title: None,
                waiting_since: None,
                interrupted: false,
                last_stamp: 0,
            }),
        })
    }

    pub fn spawn(self: &Arc<Self>, argv: &[String]) -> Result<()> {
        let spec = self.spec.lock().clone();
        let mut cmd = tokio::process::Command::new(&argv[0]);
        cmd.args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .env("KELPIE_SESSION_ID", &spec.id)
            .env(kelpie_proto::SESSION_TOKEN_ENV, crate::auth::session_token(&spec.id))
            .env(kelpie_proto::SESSION_ENDPOINT_ENV, kelpie_proto::endpoint_key(&kelpie_proto::endpoint()));
        if std::path::Path::new(&spec.cwd).is_dir() {
            cmd.current_dir(&spec.cwd);
        }
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = cmd.spawn().with_context(|| format!("spawn {argv:?}"))?;
        let pid = child.id();
        let tree = pid.and_then(|p| ProcTree::adopt(p, None, None).ok()).map(Arc::new);
        let mut stdin = child.stdin.take().context("stdin")?;
        let stdout = child.stdout.take().context("stdout")?;
        let stderr = child.stderr.take().context("stderr")?;
        let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        {
            let mut g = self.inner.lock();
            g.stdin = Some(in_tx);
            g.tree = tree;
            g.pid = pid;
            g.alive = true;
            g.exit_code = None;
            g.stopped = false;
            g.interrupted = false;
            g.activity = Activity { alive: true, ..Default::default() };
            g.last_status = Status::Idle;
        }
        tokio::spawn(async move {
            while let Some(bytes) = in_rx.recv().await {
                if stdin.write_all(&bytes).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let me = Arc::clone(self);
        tokio::spawn(async move {
            // `read_until` rather than `lines()`: one invalid UTF-8 byte must not end the relay.
            let mut reader = BufReader::with_capacity(256 * 1024, stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(_) => me.ingest_line(String::from_utf8_lossy(&buf).trim_end().to_string()),
                }
            }
        });
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                me.ingest_line(json!({"type": "kelpie_stderr", "text": line}).to_string());
            }
        });
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let code = child.wait().await.ok().and_then(|s| s.code()).unwrap_or(-1);
            {
                let mut g = me.inner.lock();
                g.alive = false;
                g.stdin = None;
                g.tree = None;
                g.exit_code = Some(code);
                g.activity.alive = false;
            }
            me.ingest_line(json!({"type": "kelpie_exit", "code": code}).to_string());
        });
        Ok(())
    }

    fn ingest_line(&self, mut line: String) {
        if line.trim().is_empty() {
            return;
        }
        if line.len() > MAX_LINE {
            // Keep frames bounded; the agent's line is replaced by a notice.
            line = json!({"type": "kelpie_stderr", "text": format!("[dropped a {} byte line]", line.len())}).to_string();
        }
        let now = Instant::now();
        let derived = classify(&line);
        let failed_turn = derived == Some(Status::Idle) && turn_failed(&line);
        let title = conversation_title(&line);
        line.push('\n');
        let mut g = self.inner.lock();
        if title.is_some() {
            g.title = title;
        }
        // The agents' lines carry no time: a `kelpie_time` line ahead of the first one of each
        // second lets a replayed transcript show when every message came.
        let wall = crate::session::now_millis();
        if wall >= g.last_stamp + 1000 {
            g.last_stamp = wall;
            line.insert_str(0, &format!("{}\n", json!({"type": "kelpie_time", "at": wall})));
        }
        let bytes: Arc<[u8]> = Arc::from(line.into_bytes());
        g.seq += bytes.len() as u64;
        g.log_bytes += bytes.len();
        g.log.push_back(bytes.clone());
        while g.log.len() > LOG_MAX_LINES || g.log_bytes + g.pinned_bytes > LOG_MAX_BYTES {
            let Some(old) = g.log.pop_front() else { break };
            g.log_bytes -= old.len();
            let head = String::from_utf8_lossy(&old[..old.len().min(256)]).into_owned();
            let Some(kind) = PINNED_IDS.iter().position(|id| head.contains(id)).filter(|_| old.len() < 1 << 20) else { continue };
            // A newer answer of the same kind (a new thread, a later command list) replaces it.
            if let Some(i) = g.pinned.iter().position(|(k, _)| *k == kind) {
                let (_, prev) = g.pinned.remove(i);
                g.pinned_bytes -= prev.len();
            }
            g.pinned_bytes += old.len();
            g.pinned.push((kind, old));
        }
        g.activity.on_output(now);
        g.last_activity = crate::session::now_millis();
        if let Some(status) = derived {
            g.activity.on_hook(status, failed_turn, now);
        }
        if g.subs.is_empty() {
            return;
        }
        let handle = self.handle;
        let frame: Arc<[u8]> = Frame::Output { handle, seq: g.seq, data: bytes.to_vec() }.encode().into();
        let len = frame.len();
        g.subs.retain_mut(|s| {
            if s.stalled {
                return true;
            }
            if s.credit.load(Ordering::Relaxed) + len > SUB_BUDGET {
                s.stalled = true;
                return s.tx.send(json_out(&json!({"event": "session.resync", "params": {"handle": handle}}))).is_ok();
            }
            s.credit.fetch_add(len, Ordering::Relaxed);
            s.tx.send(Outgoing { data: frame.clone(), credit: Some(s.credit.clone()) }).is_ok()
        });
    }

    pub fn attach(&self, client: u64, tx: &ClientTx, req_id: &Value) {
        let mut g = self.inner.lock();
        g.subs.retain(|s| s.client != client);
        let mut data = Vec::with_capacity(g.log_bytes);
        for line in g.pinned.iter().map(|(_, l)| l).chain(g.log.iter()) {
            data.extend_from_slice(line);
        }
        let reply = json!({"id": req_id, "result": {
            "mode": "reset", "raw": true, "seq": g.seq, "cols": 0, "rows": 0,
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        }});
        let _ = tx.send(json_out(&reply));
        g.subs.push(Sub { client, tx: tx.clone(), credit: Arc::new(AtomicUsize::new(0)), stalled: false });
        g.focused = true;
        g.unread = false;
    }

    pub fn detach(&self, client: u64) {
        let mut g = self.inner.lock();
        g.subs.retain(|s| s.client != client);
        if g.subs.is_empty() {
            g.focused = false;
        }
    }

    /// Writes raw bytes (normally one JSON line) to the agent's stdin.
    pub fn input(&self, bytes: Vec<u8>) {
        let mut g = self.inner.lock();
        g.activity.last_input = Some(Instant::now());
        g.last_activity = crate::session::now_millis();
        if let Some(stdin) = &g.stdin {
            let _ = stdin.send(bytes);
        }
    }

    pub fn close_stdin(&self) {
        self.inner.lock().stdin = None;
    }

    pub fn kill(&self) {
        let mut g = self.inner.lock();
        g.stopped = true;
        if let Some(tree) = &g.tree {
            tree.kill();
        } else if let Some(pid) = g.pid {
            kill_pid(pid);
        }
    }

    pub fn set_unread(&self, unread: bool) {
        let mut g = self.inner.lock();
        g.unread = unread;
        g.last_broadcast = None;
    }

    /// See `Session::priority`. Chat view types into its own field, so a shown chat needs no
    /// boost; one nobody looks at still yields.
    pub fn priority(&self, prioritize: bool) -> Option<(Arc<ProcTree>, Priority)> {
        let g = self.inner.lock();
        Some((g.tree.clone()?, if prioritize && !g.focused { Priority::Background } else { Priority::Normal }))
    }

    pub fn set_focus(&self, focused: bool) {
        let mut g = self.inner.lock();
        g.focused = focused;
        if focused {
            g.unread = false;
        }
    }

    fn info_locked(&self, g: &Inner, status: Status) -> SessionInfo {
        SessionInfo {
            handle: self.handle,
            spec: self.spec.lock().clone(),
            title: g.title.clone(),
            status,
            unread: g.unread,
            alive: g.alive,
            exit_code: g.exit_code,
            limited: false,
            failed: match g.exit_code {
                Some(code) => !g.stopped && exit_failed(code),
                None => g.activity.failed,
            },
            pid: g.pid,
            cols: 0,
            rows: 0,
            seq: g.seq,
            chat: true,
            last_activity: g.last_activity,
            interrupted: g.interrupted,
            waiting_since: g.waiting_since,
        }
    }

    /// Restored (without a process) after the daemon stopped while this chat ran.
    pub fn mark_interrupted(&self, last_activity: u64) {
        let mut g = self.inner.lock();
        g.interrupted = true;
        g.last_activity = last_activity;
    }

    pub fn info(&self) -> SessionInfo {
        let g = self.inner.lock();
        self.info_locked(&g, g.last_status)
    }

    pub fn tick(&self, now: Instant) -> Option<SessionInfo> {
        let mut g = self.inner.lock();
        let status = g.activity.status(now);
        g.waiting_since = match (status, g.waiting_since) {
            (Status::Waiting, Some(t)) => Some(t),
            (Status::Waiting, None) => Some(crate::session::now_millis()),
            _ => None,
        };
        if status == Status::Working && g.last_status != Status::Working {
            g.working_since = Some(now);
        }
        if g.last_status == Status::Working && matches!(status, Status::Idle | Status::Waiting) && !g.focused {
            // A shell printing its prompt is not "work finished"; require a real stretch.
            let worked = g.working_since.is_some_and(|t| now.duration_since(t) >= MIN_WORK_FOR_UNREAD);
            g.unread |= worked || status == Status::Waiting || g.activity.hook.is_some();
        }
        g.last_status = status;
        let mut info = self.info_locked(&g, status);
        info.seq = 0;
        info.last_activity = 0;
        if g.last_broadcast.as_ref() == Some(&info) && g.last_activity < g.broadcast_activity + crate::session::ACTIVITY_REBROADCAST_MS {
            return None;
        }
        g.last_broadcast = Some(info.clone());
        g.broadcast_activity = g.last_activity;
        info.seq = g.seq;
        info.last_activity = g.last_activity;
        Some(info)
    }
}

#[cfg(unix)]
fn kill_pid(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
}

/// The title an ACP agent gives the conversation (`session_info_update`), shown as the session's.
fn conversation_title(line: &str) -> Option<String> {
    if !line.contains("session_info_update") {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let u = v.pointer("/params/update")?;
    if u.get("sessionUpdate").and_then(Value::as_str) != Some("session_info_update") {
        return None;
    }
    let title = u.get("title").and_then(Value::as_str)?.trim();
    (!title.is_empty()).then(|| title.chars().take(120).collect())
}

/// Cheap, dialect-aware status hint from one stdout line.
fn classify(line: &str) -> Option<Status> {
    // Avoid a full parse for the bulk of streaming deltas.
    if line.len() > 512 * 1024 {
        return Some(Status::Working);
    }
    let v: Value = serde_json::from_str(line).ok()?;
    // Claude stream-json
    if let Some(t) = v.get("type").and_then(Value::as_str) {
        return match t {
            "result" => Some(Status::Idle),
            "control_request" => {
                let sub = v.pointer("/request/subtype").and_then(Value::as_str);
                (sub == Some("can_use_tool")).then_some(Status::Waiting)
            }
            "assistant" | "stream_event" | "user" => Some(Status::Working),
            _ => None,
        };
    }
    // Codex app-server / ACP (JSON-RPC)
    let method = v.get("method").and_then(Value::as_str).unwrap_or("");
    if method.contains("requestApproval") || method == "session/request_permission" || method.ends_with("requestUserInput") || method.contains("elicitation") {
        return Some(Status::Waiting);
    }
    // ACP: only the turn's own content means work; usage, title, plan, mode and command-list
    // updates also arrive between turns (often right after the stop reason) and change nothing.
    if method == "session/update" {
        return match v.pointer("/params/update/sessionUpdate").and_then(Value::as_str) {
            Some("agent_message_chunk" | "agent_thought_chunk" | "tool_call" | "tool_call_update") => Some(Status::Working),
            _ => None,
        };
    }
    match method {
        "turn/started" => return Some(Status::Working),
        "turn/completed" => return Some(Status::Idle),
        m if m.starts_with("item/") => return Some(Status::Working),
        _ => {}
    }
    if v.pointer("/result/stopReason").is_some() {
        return Some(Status::Idle);
    }
    // ACP: the answer to Kelpie's `session/new` / `session/load` ends a replayed history (its
    // chunks counted as work) and no turn is running then.
    if v.get("result").is_some() && v.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("kelpie-session")) {
        return Some(Status::Idle);
    }
    None
}

/// Whether a turn-ending line reports an error (Claude `result.is_error`, Codex failed turn).
fn turn_failed(line: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return false };
    v.get("is_error").and_then(Value::as_bool) == Some(true) || v.pointer("/params/turn/status").and_then(Value::as_str) == Some("failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_dialects() {
        assert_eq!(classify(r#"{"type":"result","subtype":"success"}"#), Some(Status::Idle));
        assert_eq!(
            classify(r#"{"type":"control_request","request_id":"1","request":{"subtype":"can_use_tool"}}"#),
            Some(Status::Waiting)
        );
        assert_eq!(classify(r#"{"method":"turn/completed","params":{}}"#), Some(Status::Idle));
        assert_eq!(classify(r#"{"method":"item/commandExecution/requestApproval","id":3}"#), Some(Status::Waiting));
        assert_eq!(classify(r#"{"id":4,"result":{"stopReason":"end_turn"}}"#), Some(Status::Idle));
        assert_eq!(classify("not json"), None);
    }

    #[test]
    fn acp_updates_mean_work_only_for_the_turns_content() {
        let update = |kind: &str| classify(&format!(r#"{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s","update":{{"sessionUpdate":"{kind}"}}}}}}"#));
        for kind in ["agent_message_chunk", "agent_thought_chunk", "tool_call", "tool_call_update"] {
            assert_eq!(update(kind), Some(Status::Working), "{kind}");
        }
        // These arrive between turns too (a usage update right after the stop reason): no change.
        for kind in ["usage_update", "session_info_update", "plan", "available_commands_update", "current_mode_update", "config_option_update", "user_message_chunk", "something_new"] {
            assert_eq!(update(kind), None, "{kind}");
        }
        assert_eq!(classify(r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{}}"#), Some(Status::Waiting));
        assert_eq!(classify(r#"{"jsonrpc":"2.0","id":8,"method":"elicitation/create","params":{}}"#), Some(Status::Waiting));
        // A resumed conversation: its replay ends with the answer to session/load.
        assert_eq!(classify(r#"{"jsonrpc":"2.0","id":"kelpie-session","result":{"sessionId":"s"}}"#), Some(Status::Idle));
        assert_eq!(classify(r#"{"jsonrpc":"2.0","id":"kelpie-session-new","result":{}}"#), Some(Status::Idle));
        assert_eq!(classify(r#"{"jsonrpc":"2.0","id":"kelpie-config-3","result":{}}"#), None);
    }

    #[test]
    fn acp_titles_name_the_session() {
        let line = r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s","update":{"sessionUpdate":"session_info_update","title":" Fix the login bug "}}}"#;
        assert_eq!(conversation_title(line).as_deref(), Some("Fix the login bug"));
        assert_eq!(conversation_title(r#"{"method":"session/update","params":{"update":{"sessionUpdate":"session_info_update","title":""}}}"#), None);
        assert_eq!(conversation_title(r#"{"method":"session/update","params":{"update":{"sessionUpdate":"agent_message_chunk","content":{"text":"session_info_update"}}}}"#), None);
    }

    #[test]
    fn detects_failed_turns() {
        assert!(turn_failed(r#"{"type":"result","subtype":"error_during_execution","is_error":true}"#));
        assert!(!turn_failed(r#"{"type":"result","subtype":"success","is_error":false}"#));
        assert!(turn_failed(r#"{"method":"turn/completed","params":{"turn":{"status":"failed"}}}"#));
        assert!(!turn_failed(r#"{"method":"turn/completed","params":{"turn":{"status":"completed"}}}"#));
    }
}
