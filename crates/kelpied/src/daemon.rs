//! Session registry, client registry, RPC methods and the periodic status/persistence ticker.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::{Mutex, RwLock};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::agents::{self, Resume};
use crate::automations::{Automation, Automations, DELIVER_REVIEW};
use crate::awake::{self, Awake};
use crate::chat::ChatSession;
use crate::checkpoints::{self, Checkpoints};
use crate::meta::MetaStore;
use crate::notes::{self, Note, NoteStore};
use crate::notify::destinations::{Destinations, Host};
use crate::notify::{self, Notifier, Raised};
use crate::status::Status;
use crate::persist::{write_atomic, SavedSession, Store};
use crate::runs::{self, Action, NewRun, Runs, SessionView, TaskStatus};
use crate::scheduled_input::{ScheduledInputs, CONTINUE_TEXT};
use crate::session::{json_out, now_millis, Answer, AnswerError, ClientTx, Session, SessionInfo, SessionSpec};
use crate::status::{hook_event_failed, hook_event_limited, hook_event_status, turn_event};

mod automation;
mod process;

/// What delivery of alerts asks of the daemon, from the destinations' own threads.
struct NotifyHost(std::sync::Weak<Daemon>);

impl Host for NotifyHost {
    /// A daemon on its way down cannot say: what is on disk is looked at again by the next.
    fn still_waiting(&self, session: &str, wait: Option<u64>) -> bool {
        let Some(d) = self.0.upgrade() else { return true };
        d.by_id(session).is_some_and(|e| {
            let info = e.info();
            info.status == Status::Waiting && (wait.is_none() || info.waiting_since == wait)
        })
    }

    fn load_level(&self) -> crate::load::Level {
        self.0.upgrade().map_or(crate::load::Level::L0, |d| d.mobile.load.level())
    }

    /// A daemon on its way down cannot say, as above. A run that is gone waits for nobody.
    fn task_waiting(&self, run: &str, task: &str) -> bool {
        let Some(d) = self.0.upgrade() else { return true };
        let runs = d.runs.lock();
        runs.get(run).is_some_and(|r| r.tasks.iter().any(|t| t.id == task && t.status == TaskStatus::WaitingApproval))
    }

    fn phone_url(&self) -> Option<String> {
        self.0.upgrade()?.mobile.alert_url()
    }

    fn subscription_gone(&self, device: &str, endpoint: Option<&str>) {
        if let Some(d) = self.0.upgrade() {
            d.mobile.push_gone(device, endpoint);
            d.broadcast("mobile.updated", json!({}));
        }
    }
}

#[derive(Clone)]
pub enum Entry {
    Term(Arc<Session>),
    Chat(Arc<ChatSession>),
}

impl Entry {
    fn info(&self) -> SessionInfo {
        match self {
            Entry::Term(s) => s.info(),
            Entry::Chat(c) => c.info(),
        }
    }
    fn spec(&self) -> SessionSpec {
        match self {
            Entry::Term(s) => s.spec.lock().clone(),
            Entry::Chat(c) => c.spec.lock().clone(),
        }
    }
    fn detach(&self, client: u64) {
        match self {
            Entry::Term(s) => s.detach(client),
            Entry::Chat(c) => c.detach(client),
        }
    }
    pub fn input(&self, bytes: Vec<u8>) {
        match self {
            Entry::Term(s) => s.input(bytes),
            Entry::Chat(c) => c.input(bytes),
        }
    }
    fn kill(&self) {
        match self {
            Entry::Term(s) => s.kill(),
            Entry::Chat(c) => c.kill(),
        }
    }
    /// The process the session runs and its job (see `procs`); `None` once it has ended.
    fn proc_root(&self) -> Option<crate::procs::Root> {
        match self {
            Entry::Term(s) => s.proc_root(),
            Entry::Chat(c) => c.proc_root(),
        }
    }
}

pub struct Daemon {
    sessions: RwLock<BTreeMap<u32, Entry>>,
    next_handle: AtomicU32,
    clients: Mutex<HashMap<u64, ClientTx>>,
    /// Who each authenticated connection is (see `set_client_role`).
    client_roles: Mutex<HashMap<u64, crate::auth::Role>>,
    store: Store,
    pub data_dir: PathBuf,
    /// Path of this executable; agents call it back as `kelpied hook …`.
    pub exe: PathBuf,
    meta_dirty: AtomicBool,
    pub shutdown: tokio::sync::Notify,
    workspace_meta: Mutex<MetaStore>,
    /// Review notes on worktrees' diffs (see `notes.rs`).
    notes: Mutex<NoteStore>,
    automations: Mutex<Automations>,
    /// What the automations that events set off need to remember (see `daemon/automation.rs`).
    auto_rt: automation::Runtime,
    runs: Mutex<Runs>,
    pub mobile: crate::mobile::Mobile,
    /// The OS verification of the person (one prompt at a time).
    pub presence: crate::user_presence::Presence,
    awake: Awake,
    awake_mode: Mutex<awake::Mode>,
    /// Sessions no view shows run below normal CPU priority (`daemon.set_priority`).
    prioritize: AtomicBool,
    /// Programs in a session may script other sessions (`daemon.set_policy`, saved in
    /// `policy.json` so it holds while the app is closed).
    agents_control_others: AtomicBool,
    /// Per-turn snapshots of the sessions' working folders (see `checkpoints.rs`).
    checkpoints: Arc<Checkpoints>,
    /// Sessions of automations that deliver to review: session id -> folder, until their turn ends.
    deliver_review: Mutex<HashMap<String, String>>,
    /// Events for the user (inbox, desktop, phone) and the channels that carry them away.
    pub notifier: Notifier,
    /// "Continue at reset" timers.
    scheduled: ScheduledInputs,
    /// What the sessions run, looked at when the app asks (`process.*`).
    procs: crate::procs::Procs,
    /// This daemon, for threads that outlive a call.
    me: std::sync::Weak<Daemon>,
}

#[derive(Deserialize)]
struct CreateParams {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_kind")]
    kind: String,
    cwd: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    agent_session_id: Option<String>,
    #[serde(default = "default_cols")]
    cols: u16,
    #[serde(default = "default_rows")]
    rows: u16,
    /// Chat mode: spawn this argv with piped stdio instead of a PTY.
    #[serde(default)]
    chat_argv: Option<Vec<String>>,
    /// First message for an agent (as an argument, or typed once it is ready).
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    remote: Option<crate::session::RemoteSpec>,
    /// Its programs may act only on this session through `kelpied` (see `auth`).
    #[serde(default)]
    restricted: bool,
}

fn default_kind() -> String {
    "shell".into()
}
fn default_cols() -> u16 {
    120
}
fn default_rows() -> u16 {
    32
}

/// `--model X`, `--model=X` or `-m X` in an agent's extra arguments.
fn model_from_args(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--model" || a == "-m" {
            return it.next().cloned();
        }
        if let Some(m) = a.strip_prefix("--model=") {
            return Some(m.to_string());
        }
    }
    None
}

fn param<'a>(params: &'a Value, key: &str) -> Result<&'a Value> {
    params.get(key).ok_or_else(|| anyhow!("missing param `{key}`"))
}

fn handle_of(params: &Value) -> Result<u32> {
    param(params, "handle")?.as_u64().map(|h| h as u32).ok_or_else(|| anyhow!("bad handle"))
}

/// The reply to a request answered later.
fn rpc_answer(id: &Value, result: Result<Value>) -> Value {
    match result {
        Ok(v) => json!({"id": id, "result": v}),
        Err(e) => json!({"id": id, "error": {"message": format!("{e:#}")}}),
    }
}

impl Daemon {
    pub fn new(data_dir: PathBuf, exe: PathBuf) -> Arc<Daemon> {
        Self::with_secrets(data_dir, exe, Arc::new(notify::Keyring))
    }

    /// `secrets` keeps the notification channels' secrets (tests use memory, never the user's).
    pub fn with_secrets(data_dir: PathBuf, exe: PathBuf, secrets: Arc<dyn notify::SecretStore>) -> Arc<Daemon> {
        // `prioritize` starts on (until the app says otherwise).
        crate::proc_tree::set_daemon_priority(true);
        Arc::new_cyclic(|me: &std::sync::Weak<Daemon>| {
            let weak = me.clone();
            let me = me.clone();
            let emit: checkpoints::Emit = Arc::new(move |event, params| {
                if let Some(d) = me.upgrade() {
                    d.broadcast(event, params);
                }
            });
            // Shared by the two that decide about alerts: phone access says who has them on, the notifier what is sent.
            let destinations = Destinations::new(&data_dir, secrets.clone(), Arc::new(NotifyHost(weak.clone())));
            Daemon {
                sessions: RwLock::new(BTreeMap::new()),
                next_handle: AtomicU32::new(1),
                clients: Mutex::new(HashMap::new()),
                client_roles: Mutex::new(HashMap::new()),
                store: Store::new(&data_dir),
                workspace_meta: Mutex::new(MetaStore::load(&data_dir)),
                notes: Mutex::new(NoteStore::load(&data_dir)),
                automations: Mutex::new(Automations::load(&data_dir)),
                auto_rt: automation::Runtime::new(),
                runs: Mutex::new(Runs::load(&data_dir)),
                mobile: crate::mobile::Mobile::load_with(&data_dir, secrets.clone(), destinations.clone()),
                presence: crate::user_presence::Presence::default(),
                awake: Awake::new(),
                awake_mode: Mutex::new(awake::Mode::Off),
                prioritize: AtomicBool::new(true),
                agents_control_others: AtomicBool::new(load_policy(&data_dir)),
                checkpoints: Arc::new(Checkpoints::new(&data_dir, emit)),
                deliver_review: Mutex::new(HashMap::new()),
                notifier: Notifier::with_destinations(&data_dir, secrets, destinations),
                scheduled: ScheduledInputs::open(&data_dir),
                procs: crate::procs::Procs::default(),
                me: weak,
                data_dir,
                exe,
                meta_dirty: AtomicBool::new(false),
                shutdown: tokio::sync::Notify::new(),
            }
        })
    }

    /// A new Claude Code conversation gets its id from Kelpie (`claude --session-id <id>`): known
    /// from the first moment, so a restart resumes exactly it even when no hook reported it
    /// (hooks off, Claude stopped before its first hook, a host over SSH where hooks never run).
    /// Hooks still follow it when it changes (`/clear`, `/resume` inside Claude). True when an
    /// id was given.
    fn preset_conversation(spec: &mut SessionSpec) -> bool {
        if spec.kind != "claude" || spec.agent_session_id.is_some() || agents::claude_args_pick_conversation(&spec.args) {
            return false;
        }
        spec.agent_session_id = Some(uuid::Uuid::new_v4().to_string());
        true
    }

    /// What `restart` resumes: the known conversation, else (agents that report no id) the
    /// folder's most recent one.
    /// The command line of a session: local, or over SSH for a remote one.
    fn launch_for(spec: &SessionSpec, resume: Resume, prompt: Option<&str>, exe: &std::path::Path, data_dir: &std::path::Path) -> Result<agents::Launch> {
        match &spec.remote {
            Some(r) => agents::build_remote_launch(&spec.kind, &spec.args, resume, prompt, r, &spec.cwd, &spec.id),
            None => agents::build_launch(&spec.kind, &spec.args, resume, prompt, exe, data_dir),
        }
    }

    fn resume_for(spec: &SessionSpec) -> Resume<'_> {
        if spec.remote.is_some() {
            // No hooks reach us from another host: the conversation id is rarely known, and
            // the folder's latest conversation is the one this session had.
            return match spec.agent_session_id.as_deref() {
                Some(id) => Resume::Id(id),
                None if agents::preset_for(&spec.kind).is_some_and(|p| !p.resume_latest.is_empty()) => Resume::Latest,
                None => Resume::None,
            };
        }
        match spec.agent_session_id.as_deref() {
            // Claude saves a conversation only with its first turn: `--resume` of one never
            // saved fails, so that session starts it under the same id.
            Some(id) if spec.kind == "claude" && !agents::claude_conversation_saved(id, &spec.env) => {
                if agents::claude_args_pick_conversation(&spec.args) { Resume::None } else { Resume::New(id) }
            }
            Some(id) => Resume::Id(id),
            None if !matches!(spec.kind.as_str(), "claude" | "codex")
                && agents::preset_for(&spec.kind).is_some_and(|p| !p.resume_latest.is_empty()) =>
            {
                Resume::Latest
            }
            None => Resume::None,
        }
    }

    /// Writes a folder's comment / status and tells the apps (`meta.set`).
    fn set_meta(&self, path: &str, comment: Option<&str>, status: Option<&str>) -> Result<Value> {
        let (key, m) = self.workspace_meta.lock().set(path, comment, status)?;
        let v = json!({"path": key, "comment": m.comment, "status": m.status, "updated_at": m.updated_at});
        self.broadcast("meta.updated", v.clone());
        Ok(v)
    }

    /// The automation's run finished (its turn ended or the session exited): its folder goes to review.
    fn deliver_to_review(&self, session: &str) {
        let Some(cwd) = self.deliver_review.lock().remove(session) else { return };
        // The folder's existing record (the worktree root above a subfolder), like `kelpied worktree set`.
        let path = self.workspace_meta.lock().find(&cwd).map(|(k, _)| k).unwrap_or(cwd);
        if let Err(e) = self.set_meta(&path, None, Some("review")) {
            tracing::warn!("deliver to review: {e:#}");
        }
    }

    // ---------- review notes (see notes.rs) ----------

    fn note_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        let text = |k: &str| params.get(k).and_then(Value::as_str).unwrap_or_default();
        let path = text("path");
        if method == "note.list" && path.is_empty() {
            // Every worktree's notes are the user's: a session's request always names its own.
            anyhow::ensure!(params.get("by_session").is_none(), "note.list: only this session's own worktree");
            return Ok(json!(self.notes.lock().all()));
        }
        // The key each folder's notes are filed under (the app asks once per project).
        if method == "note.keys" {
            let paths = params.get("paths").and_then(Value::as_array).ok_or_else(|| anyhow!("missing param `paths`"))?;
            let keys: serde_json::Map<String, Value> = paths.iter().filter_map(Value::as_str).take(500).map(|p| (p.to_string(), json!(notes::worktree_key(p)))).collect();
            return Ok(Value::Object(keys));
        }
        anyhow::ensure!(!path.is_empty(), "missing param `path` (the worktree folder)");
        // `exact`: `path` already is a key (what `note.keys` said); it is not looked up again, so a folder
        // that is half deleted cannot stand for the repository around it.
        let key = if method == "note.clear" && params.get("exact").and_then(Value::as_bool) == Some(true) { crate::meta::key(path) } else { notes::worktree_key(path) };
        match method {
            "note.list" => Ok(json!(self.notes.lock().list(&key))),
            "note.add" => {
                let line = params.get("line").and_then(Value::as_u64).and_then(|l| u32::try_from(l).ok()).ok_or_else(|| anyhow!("missing param `line`"))?;
                // The app quotes the line it showed; an agent names a line and the diff says the rest.
                let (side, quoted) = match (text("side"), params.get("text").and_then(Value::as_str)) {
                    (side @ ("new" | "old"), Some(quoted)) => (side.to_string(), quoted.to_string()),
                    _ => {
                        let top = crate::checkpoints::find_top(std::path::Path::new(path)).ok_or_else(|| anyhow!("{path} is not inside a git worktree"))?;
                        let base = Some(text("base")).filter(|b| !b.is_empty());
                        notes::locate(&top, text("file"), line, params.get("old").and_then(Value::as_bool) == Some(true), base)?
                    }
                };
                // Named after the agent that wrote it; the user's own notes carry no name.
                let by = params.get("by_session").and_then(Value::as_str).map(|sid| {
                    self.by_id(sid).map(|e| e.spec()).map(|s| s.name.filter(|n| !n.is_empty()).unwrap_or(s.kind)).unwrap_or_else(|| "agent".to_string())
                });
                let note = Note {
                    id: Some(text("id")).filter(|i| !i.is_empty()).map(str::to_string).unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
                    path: text("file").to_string(),
                    line,
                    side,
                    text: quoted,
                    note: text("note").to_string(),
                    at: params.get("at").and_then(Value::as_u64).unwrap_or_else(now_millis),
                    by,
                };
                let added = self.notes.lock().add(&key, note)?;
                self.notes_changed(&key);
                Ok(json!({"path": key, "note": added, "notes": self.notes.lock().list(&key)}))
            }
            "note.clear" => {
                let ids: Option<Vec<String>> = params.get("ids").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect());
                let left = self.notes.lock().clear(&key, ids.as_deref())?;
                self.notes_changed(&key);
                Ok(json!({"path": key, "notes": left}))
            }
            other => bail!("unknown method {other}"),
        }
    }

    /// Tells the user's connections (the app) of a worktree's notes, and a session's own connection
    /// when it is that session's worktree. Nobody else: not a session of another worktree, not a
    /// connection that has not authenticated.
    fn notes_changed(&self, key: &str) {
        let msg = json_out(&json!({"event": "notes.changed", "params": {"path": key, "notes": self.notes.lock().list(key)}}));
        // Who is told is worked out first (it looks at folders); the clients lock is held only to send.
        let told: Vec<u64> = self
            .client_roles
            .lock()
            .clone()
            .into_iter()
            .filter(|(_, role)| match role {
                crate::auth::Role::User | crate::auth::Role::App => true,
                crate::auth::Role::Session(sid) => self.session_policy(sid).is_some_and(|(_, cwd)| notes::worktree_key(&cwd) == key),
                _ => false,
            })
            .map(|(id, _)| id)
            .collect();
        let clients = self.clients.lock();
        for id in told {
            if let Some(tx) = clients.get(&id) {
                let _ = tx.send(crate::session::Outgoing { data: msg.data.clone(), credit: None });
            }
        }
    }

    /// What a connection proved itself to be (`auth`): decides which events it is sent.
    pub fn set_client_role(&self, id: u64, role: &crate::auth::Role) {
        self.client_roles.lock().insert(id, role.clone());
    }

    // ---------- notifications (see notify.rs) ----------

    /// An event the user should hear about. The app gets it for its inbox and desktop notification
    /// (`tell_app`), and the channels carry it off the computer. What a focused window already
    /// shows is not raised at all, so the desktop and the phone always agree.
    fn raise(&self, ev: Raised, tell_app: bool) {
        use notify::Kind;
        let seen = ev.session.as_deref().is_some_and(|s| self.notifier.looking_at(s));
        if seen && matches!(ev.kind, Kind::Waiting | Kind::Finished | Kind::Error) {
            return;
        }
        if tell_app {
            self.broadcast("notify.event", ev.to_json());
        }
        if seen && ev.kind == Kind::Cost {
            return;
        }
        if self.notifier.wants_push(&ev, Instant::now()) {
            let ctx = self.push_context(&ev);
            self.notifier.send(&ev, &ctx);
        }
    }

    /// What a message needs to know around an event; read only for one that is being sent.
    fn push_context(&self, ev: &Raised) -> notify::Context {
        let mut ctx = notify::Context { phone_url: self.notifier.config().phone_link.then(|| self.mobile.running_url()).flatten(), ..Default::default() };
        ctx.project = ev.project_id.as_deref().and_then(|p| self.mobile.project_name(p));
        let Some(entry) = ev.session.as_deref().and_then(|s| self.by_id(s)) else { return ctx };
        let info = entry.info();
        let spec = &info.spec;
        let kind_label = agents::preset_for(&spec.kind).map_or(spec.kind.as_str(), |p| p.label);
        ctx.title = Some(notify::session_label(spec.name.as_deref(), info.title.as_deref(), &spec.kind, kind_label, spec.remote.as_ref().map(|r| r.dest.as_str())));
        if self.notifier.config().include_screen {
            if let Entry::Term(s) = &entry {
                let tail = s.text_tail(40);
                let lines: Vec<&str> = tail.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
                ctx.screen = Some(lines[lines.len().saturating_sub(14)..].join("\n"));
            }
        }
        ctx
    }

    /// One status change of a session, from the ticker or a spec update.
    fn observe(&self, info: &SessionInfo, now: Instant) {
        if let Some(ev) = self.notifier.observe(info, now) {
            self.raise(ev, true);
        }
    }

    /// A run's tasks that just began waiting for the user's approval.
    fn announce_approvals(&self, run: &runs::Run) {
        let tasks: Vec<(String, String, bool)> = run.tasks.iter().map(|t| (t.id.clone(), t.title.clone(), t.status == TaskStatus::WaitingApproval)).collect();
        for ev in self.notifier.new_approvals(&run.id, &run.name, run.project_id.as_deref(), &tasks) {
            self.raise(ev, true);
        }
    }

    // ---------- continue at reset (see scheduled_input.rs) ----------

    /// A timer is due: type `continue` if the session is still stopped by the limit it was set
    /// for and nobody typed since; otherwise drop it and say why.
    fn fire_scheduled(&self, item: crate::scheduled_input::Scheduled) {
        let result = match self.by_id(&item.session) {
            Some(Entry::Term(s)) => s
                .continue_if_limited(item.generation, item.agent_session_id.as_deref(), item.turn, crate::session::typed_text(CONTINUE_TEXT).into_bytes())
                .map(|written| (s, written)),
            _ => Err(crate::session::Refused::Exited),
        };
        match result {
            Ok((s, written)) => {
                // Enter is a separate write (a TUI takes one in the same write as text), and only
                // if nothing was typed in between; the app is told what happened.
                let (me, session) = (self.me.clone(), item.session.clone());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(120));
                    let sent = s.input_if_last_typed_at(written, b"\r".to_vec());
                    if let Some(d) = me.upgrade() {
                        let event = if sent { json!({"session": session, "at": null, "result": "sent"}) } else { json!({"session": session, "at": null, "result": "cancelled", "reason": "input"}) };
                        d.broadcast("scheduled.updated", event);
                    }
                });
            }
            Err(why) => self.broadcast("scheduled.updated", json!({"session": item.session, "at": null, "result": "cancelled", "reason": why.code()})),
        }
    }

    /// The session's timer goes (it was closed or is starting again): the app is told why.
    fn drop_timer(&self, session: &str, reason: &str) {
        if self.scheduled.cancel(session) {
            self.broadcast("scheduled.updated", json!({"session": session, "at": null, "result": "cancelled", "reason": reason}));
        }
    }

    /// `session.set_env_restart`: gives a stopped Claude Code / Codex session another account's
    /// config home and starts it again on the same conversation.
    fn set_env_restart(&self, handle: u32, env: &Value) -> Result<SessionInfo> {
        let Entry::Term(session) = self.entry(handle)? else { bail!("only terminal sessions can change account") };
        anyhow::ensure!(!session.is_alive(), "stop the session first");
        let (var, remote) = {
            let spec = session.spec.lock();
            (match spec.kind.as_str() {
                "claude" => "CLAUDE_CONFIG_DIR",
                "codex" => "CODEX_HOME",
                other => bail!("{other} sessions have no account to switch"),
            }, spec.remote.is_some())
        };
        anyhow::ensure!(!remote, "a session on another machine keeps its account");
        let map = env.as_object().ok_or_else(|| anyhow!("env must be an object"))?;
        anyhow::ensure!(map.keys().all(|k| k == var), "only {var} can change");
        let value = match map.get(var) {
            None | Some(Value::Null) => None,
            Some(Value::String(dir)) if std::path::Path::new(dir).is_absolute() && std::path::Path::new(dir).is_dir() => Some(dir.clone()),
            Some(_) => bail!("{var} must be an existing folder"),
        };
        let set = |dir: Option<String>| {
            let mut spec = session.spec.lock();
            match dir {
                Some(dir) => spec.env.insert(var.into(), dir),
                None => spec.env.remove(var),
            }
        };
        let old = set(value);
        self.meta_dirty.store(true, Ordering::Relaxed);
        // A restart that fails leaves the session as it was.
        self.restart(handle).inspect_err(|_| {
            set(old);
        })
    }

    /// Answers on `tx` once the one-shot run finishes (see `oneshot.rs`).
    fn agent_run(&self, tx: &ClientTx, id: &Value, params: &Value) -> Result<()> {
        let kind = params.get("kind").and_then(Value::as_str).unwrap_or("claude").to_string();
        let prompt = param(params, "prompt")?.as_str().unwrap_or_default().to_string();
        let cwd = params.get("cwd").and_then(Value::as_str).unwrap_or(".").to_string();
        let timeout = Duration::from_secs(params.get("timeout_s").and_then(Value::as_u64).unwrap_or(180).clamp(5, 900));
        let env: BTreeMap<String, String> = params.get("env").cloned().and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
        let (tx, id) = (tx.clone(), id.clone());
        std::thread::Builder::new().name("kelpie-oneshot".into()).spawn(move || {
            let reply = match crate::oneshot::run(&kind, &prompt, &cwd, &env, timeout) {
                Ok(text) => json!({"id": id, "result": {"text": text}}),
                Err(e) => json!({"id": id, "error": {"message": format!("{e:#}")}}),
            };
            let _ = tx.send(json_out(&reply));
        })?;
        Ok(())
    }

    /// Whether this connection proved it is the Kelpie app (`auth`).
    fn is_verified_app(&self, client: u64) -> bool {
        self.client_roles.lock().get(&client) == Some(&crate::auth::Role::App)
    }

    /// Runs `then` once the person passed the OS verification, and answers request `id` on `tx`. The prompt runs on its own thread,
    /// so the caller's other requests keep flowing meanwhile. Nothing is shown to a caller that is not the verified app, and
    /// `then` runs only after the OS said yes; what the prompt says is the daemon's, in the language `lang` names. A verification
    /// that fails leaves a line in the audit log; whatever happens, even a panic, the request is answered and the screen is free.
    fn after_verifying(self: &Arc<Self>, client: u64, tx: &ClientTx, id: &Value, params: &Value, prompt: crate::user_presence::Prompt, then: impl FnOnce(&Arc<Daemon>) -> Result<Value> + Send + 'static) -> Result<()> {
        if !self.is_verified_app(client) {
            bail!("this needs the Kelpie app: phone access is decided in Kelpie → Settings → Mobile");
        }
        let turn = self.presence.begin().map_err(|e| anyhow!(e))?;
        let window = params.get("window").and_then(Value::as_i64);
        let lang = crate::user_presence::Lang::from_code(params.get("lang").and_then(Value::as_str));
        let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
        std::thread::Builder::new().name("kelpie-verify".into()).spawn(move || {
            use crate::user_presence::Lang;
            let verified = me.presence.verify(window, &prompt.text(lang));
            // Noted before the turn is given up: nobody can ask in the gap after the refusal that stops the prompts.
            match &verified {
                Ok(()) => me.presence.passed(),
                Err(why) => {
                    me.mobile.audit("-", "verify_failed", &format!("{} {}", prompt.text(Lang::En), why.message));
                    if me.presence.refused(why) {
                        me.mobile.audit("-", "verify_paused", "too many refused or unanswered verifications: no new prompts for a while");
                    }
                }
            }
            drop(turn);
            let done = verified.map_err(|e| anyhow!(e.message)).and_then(|()| std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| then(&me))).unwrap_or_else(|_| Err(anyhow!("that could not be finished: an internal error"))));
            let reply = match done {
                Ok(v) => json!({"id": id, "result": v}),
                Err(e) => json!({"id": id, "error": {"message": format!("{e:#}")}}),
            };
            let _ = tx.send(json_out(&reply));
        })?;
        Ok(())
    }

    /// Sets what a device may do and tells everyone: its waiting messages go when it may no longer write.
    fn apply_permission(&self, device: &str, permission: crate::mobile::Permission) -> Result<()> {
        let event = self.mobile.set_permission(device, permission)?;
        if permission < crate::mobile::Permission::Write {
            self.drop_device_queues(device, "revoked");
        }
        self.broadcast("mobile.device_changed", event.json());
        self.broadcast("mobile.updated", json!({}));
        Ok(())
    }

    /// Holds the wake request while the mode asks for it (called from the ticker).
    fn update_awake(&self) {
        let want = match *self.awake_mode.lock() {
            awake::Mode::Off => false,
            awake::Mode::Always => true,
            awake::Mode::Agents => self.sessions.read().values().any(|e| {
                let info = e.info();
                info.status == Status::Working && info.spec.kind != "shell"
            }),
        };
        self.awake.set(want);
    }

    // ---------- orchestration runs (see runs.rs) ----------

    /// Saves runs and tells clients about the ones that changed.
    fn runs_changed(&self, ids: &[String]) {
        if ids.is_empty() {
            return;
        }
        let runs = self.runs.lock();
        if let Err(e) = runs.save() {
            tracing::warn!("save runs: {e:#}");
        }
        for id in ids {
            if let Some(r) = runs.get(id) {
                self.broadcast("run.updated", json!(r));
                self.announce_approvals(r);
            }
        }
    }

    /// One engine step: finish, skip and start tasks (called every second by the ticker).
    fn run_tick(self: &Arc<Self>) {
        let view = |sid: &str| {
            self.by_id(sid).map(|e| {
                let i = e.info();
                SessionView { alive: i.alive, working: i.status == Status::Working, idle: i.status == Status::Idle, exit_code: i.exit_code, last_activity: i.last_activity, interrupted: i.interrupted }
            })
        };
        let (actions, changed) = self.runs.lock().step(now_millis(), view);
        self.runs_changed(&changed);
        for a in actions {
            let me = self.clone();
            // Worktrees and process starts take a moment: never on the ticker itself.
            tokio::task::spawn_blocking(move || match a {
                Action::Start { run, task } => me.start_task(&run, &task),
                Action::Capture { run, task, session } => me.capture_task(&run, &task, &session),
                Action::Release { session } => me.release_session(&session),
            });
        }
    }

    /// Creates the task's worktree (when asked) and starts its agent with the task prompt.
    fn start_task(&self, run_id: &str, task_id: &str) {
        let Some((run, task)) = self.runs.lock().get(run_id).and_then(|r| r.tasks.iter().find(|t| t.id == task_id).cloned().map(|t| (r.clone(), t))) else { return };
        let prepared = if task.worktree { create_task_worktree(&run, &task).map(Some) } else { Ok(None) };
        // The session id is the task's before its process starts: an agent that reports at once
        // must find its task.
        let sid = uuid::Uuid::new_v4().to_string();
        if let Ok((t, _)) = self.runs.lock().task_mut(run_id, task_id) {
            t.session_id = Some(sid.clone());
        }
        let started = prepared.and_then(|wt| {
            // On a host the folder is the one there; the session starts like any remote one.
            let cwd = match &task.host {
                Some(h) => h.path.clone(),
                None => wt.as_ref().map_or_else(|| run.cwd.clone(), |w| w.path.clone()),
            };
            let prompt = runs::task_prompt(&run, &task, &self.exe);
            let info = self.create(json!({
                "id": sid,
                "kind": task.kind,
                "cwd": cwd,
                "project_id": run.project_id,
                "name": format!("{} · {}", run.name, task.title),
                "args": task.args,
                "env": task.env,
                "remote": task.host.as_ref().map(runs::TaskHost::remote),
                "prompt": prompt,
            }))?;
            Ok((info.spec.id, cwd, wt))
        });
        {
            let mut runs = self.runs.lock();
            let Ok((t, _)) = runs.task_mut(run_id, task_id) else { return };
            match started {
                Ok((sid, cwd, wt)) => {
                    // A report may already have finished it.
                    if t.status == TaskStatus::Starting {
                        t.status = TaskStatus::Running;
                    }
                    t.session_id = Some(sid);
                    t.cwd = Some(cwd);
                    if let Some(w) = wt {
                        t.branch = Some(w.branch);
                        t.base = w.base;
                        t.brought = w.brought;
                    }
                    t.started_at = now_millis();
                }
                Err(e) => {
                    t.status = TaskStatus::Failed;
                    t.error = Some(format!("{e:#}"));
                    t.finished_at = now_millis();
                }
            }
        }
        self.runs_changed(&[run_id.to_string()]);
    }

    /// Called at the end of `restore`, before any client is served: the tasks on a host that were running when the daemon stopped
    /// fail and their sessions are released, so no app can resume them first.
    fn settle_host_tasks(&self) {
        let view = |sid: &str| {
            self.by_id(sid).map(|e| {
                let i = e.info();
                SessionView { alive: i.alive, working: i.status == Status::Working, idle: i.status == Status::Idle, exit_code: i.exit_code, last_activity: i.last_activity, interrupted: i.interrupted }
            })
        };
        let (sessions, changed) = self.runs.lock().settle_interrupted_hosts(now_millis(), view);
        for s in &sessions {
            self.release_session(s);
        }
        self.runs_changed(&changed);
    }

    /// A session of a failed task on a host is not resumed by the app after a restart: tells the clients.
    fn release_session(&self, session: &str) {
        if let Some(Entry::Term(s)) = self.by_id(session) {
            s.clear_interrupted();
            // Saved as it is now: a second stop must not bring it back as an interrupted session.
            self.meta_dirty.store(true, Ordering::Relaxed);
            self.broadcast("session.updated", json!(s.info()));
        }
    }

    /// Keeps the end of a finished task's output when its agent reported no summary.
    fn capture_task(&self, run_id: &str, task_id: &str, session: &str) {
        let tail = match self.by_id(session) {
            Some(Entry::Term(s)) => s.text_tail(40),
            _ => return,
        };
        let text: Vec<&str> = tail.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
        let tail = text[text.len().saturating_sub(12)..].join("\n");
        {
            let mut runs = self.runs.lock();
            let Ok((t, _)) = runs.task_mut(run_id, task_id) else { return };
            if t.summary.is_some() || tail.is_empty() {
                return;
            }
            t.summary = Some(tail.chars().take(2000).collect());
        }
        self.runs_changed(&[run_id.to_string()]);
    }

    fn run_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        let id = || params.get("id").and_then(Value::as_str).unwrap_or_default().to_string();
        let task = || params.get("task").and_then(Value::as_str).unwrap_or_default().to_string();
        let mut kill: Vec<String> = Vec::new();
        let result = match method {
            "run.list" => return Ok(json!(self.runs.lock().list)),
            "run.get" => return Ok(json!(self.runs.lock().get(&id()).ok_or_else(|| anyhow!("no run {}", id()))?)),
            "run.create" => {
                let new: NewRun = serde_json::from_value(params.clone()).context("run.create params")?;
                let run = runs::build_run(new, uuid::Uuid::new_v4().to_string(), now_millis())?;
                let out = json!(run);
                let rid = run.id.clone();
                self.runs.lock().list.insert(0, run);
                self.runs_changed(&[rid]);
                return Ok(out);
            }
            "run.approve" => {
                let mut runs = self.runs.lock();
                let (t, rid) = runs.task_mut(&id(), &task())?;
                t.approved = true;
                if t.status == TaskStatus::WaitingApproval {
                    t.status = TaskStatus::Pending;
                }
                rid
            }
            "run.retry" => {
                let mut runs = self.runs.lock();
                let r = runs.get_mut(&id()).ok_or_else(|| anyhow!("no run {}", id()))?;
                let tid = task();
                let t = r.tasks.iter_mut().find(|t| t.id == tid).ok_or_else(|| anyhow!("no task {tid}"))?;
                anyhow::ensure!(matches!(t.status, TaskStatus::Failed | TaskStatus::Skipped | TaskStatus::Cancelled), "only a failed, skipped or cancelled task can be retried");
                // It and the tasks skipped because of it run again.
                for t in r.tasks.iter_mut().filter(|t| t.id == tid || t.status == TaskStatus::Skipped) {
                    t.status = TaskStatus::Pending;
                    t.session_id = None;
                    t.error = None;
                    t.summary = None;
                    t.finished_at = 0;
                    t.worked = false;
                }
                r.status = runs::RunStatus::Running;
                r.finished_at = 0;
                r.id.clone()
            }
            "run.task_done" | "run.task_skip" => {
                let mut runs = self.runs.lock();
                let (t, rid) = runs.task_mut(&id(), &task())?;
                anyhow::ensure!(!t.status.finished(), "the task has already finished");
                if let Some(sid) = t.session_id.clone().filter(|_| t.status == TaskStatus::Running && method == "run.task_skip") {
                    kill.push(sid);
                }
                t.status = if method == "run.task_done" { TaskStatus::Done } else { TaskStatus::Skipped };
                if let Some(s) = params.get("summary").and_then(Value::as_str).filter(|s| !s.trim().is_empty()) {
                    t.summary = Some(s.trim().to_string());
                }
                t.finished_at = now_millis();
                rid
            }
            "run.cancel" => {
                let mut runs = self.runs.lock();
                let r = runs.get_mut(&id()).ok_or_else(|| anyhow!("no run {}", id()))?;
                for t in r.tasks.iter_mut().filter(|t| !t.status.finished()) {
                    if let Some(sid) = t.session_id.clone().filter(|_| t.status == TaskStatus::Running) {
                        kill.push(sid);
                    }
                    t.status = TaskStatus::Cancelled;
                    t.finished_at = now_millis();
                }
                if r.status == runs::RunStatus::Running {
                    r.status = runs::RunStatus::Cancelled;
                    r.finished_at = now_millis();
                }
                r.id.clone()
            }
            "run.delete" => {
                let mut runs = self.runs.lock();
                let rid = runs.get(&id()).map(|r| r.id.clone()).ok_or_else(|| anyhow!("no run {}", id()))?;
                anyhow::ensure!(runs.get(&rid).is_some_and(|r| r.status != runs::RunStatus::Running), "cancel the run before deleting it");
                runs.list.retain(|r| r.id != rid);
                self.notifier.forget_run(&rid);
                runs.save()?;
                self.broadcast("run.deleted", json!({"id": rid}));
                return Ok(json!(true));
            }
            "run.report" => {
                let sid = params.get("session_id").and_then(Value::as_str).unwrap_or_default().to_string();
                let failed = params.get("status").and_then(Value::as_str) == Some("failed");
                let summary = params.get("summary").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
                let mut runs = self.runs.lock();
                let (r, i) = runs.by_session(&sid).ok_or_else(|| anyhow!("this session is not running a task of any run"))?;
                let t = &mut r.tasks[i];
                t.status = if failed { TaskStatus::Failed } else { TaskStatus::Done };
                if failed {
                    t.error = summary.clone().or_else(|| Some("reported as failed".into()));
                }
                t.summary = summary;
                t.finished_at = now_millis();
                let out = json!({"run": r.id, "task": t.id});
                let rid = r.id.clone();
                drop(runs);
                self.runs_changed(&[rid]);
                return Ok(out);
            }
            other => bail!("unknown method {other}"),
        };
        for sid in kill {
            if let Some(e) = self.by_id(&sid) {
                e.kill();
            }
        }
        self.runs_changed(std::slice::from_ref(&result));
        Ok(json!(self.runs.lock().get(&result)))
    }

    pub fn register_client(&self, id: u64, tx: ClientTx) {
        self.clients.lock().insert(id, tx);
    }

    pub fn unregister_client(&self, id: u64) {
        self.clients.lock().remove(&id);
        self.client_roles.lock().remove(&id);
        self.auto_rt.drop_client(id);
        self.notifier.drop_client(id);
        for entry in self.sessions.read().values() {
            entry.detach(id);
            // A window that is gone cannot build what it queued.
            if let Entry::Chat(c) = entry {
                c.client_gone(id);
            }
        }
    }

    pub fn broadcast(&self, event: &str, params: Value) {
        self.broadcast_event(event, params, None);
    }

    fn broadcast_event(&self, event: &str, params: Value, closed_session: Option<&str>) {
        use crate::auth::EventVisibility;
        // Resolve session policy before locking clients, just as notes_changed does.
        // Callers must release session-map guards before publishing an event.
        let told: Vec<_> = self
            .client_roles
            .lock()
            .clone()
            .into_iter()
            .filter_map(|(id, role)| {
                let visibility = crate::auth::event_visibility(self, &role, event, &params, closed_session);
                (!matches!(visibility, EventVisibility::Hidden)).then_some((id, visibility))
            })
            .collect();
        let mut full = None;
        let mut redacted = None;
        let clients = self.clients.lock();
        for (id, visibility) in told {
            let Some(tx) = clients.get(&id) else { continue };
            let msg = match visibility {
                EventVisibility::Full => full.get_or_insert_with(|| json_out(&json!({"event": event, "params": &params}))),
                EventVisibility::WithoutEnv => redacted.get_or_insert_with(|| {
                    let mut safe = params.clone();
                    if let Some(object) = safe.as_object_mut() {
                        object.remove("env");
                    }
                    json_out(&json!({"event": event, "params": safe}))
                }),
                EventVisibility::Hidden => unreachable!(),
            };
            let _ = tx.send(crate::session::Outgoing { data: msg.data.clone(), credit: None });
        }
    }

    /// Adds a session that has no process, for tests of what talks to sessions.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, entry: Entry) {
        let handle = match &entry {
            Entry::Term(s) => s.handle,
            Entry::Chat(c) => c.handle,
        };
        self.sessions.write().insert(handle, entry);
    }

    /// Takes a session away, as closing it does.
    #[cfg(test)]
    pub(crate) fn remove_for_test(&self, handle: u32) {
        self.sessions.write().remove(&handle);
    }

    /// Drops the chat messages a phone has waiting (it was revoked, or may no longer write).
    pub fn drop_device_queues(&self, device: &str, reason: &'static str) {
        let chats: Vec<Arc<ChatSession>> = self.sessions.read().values().filter_map(|e| if let Entry::Chat(c) = e { Some(c.clone()) } else { None }).collect();
        for c in chats {
            c.drop_device(device, reason);
        }
    }

    pub fn entry(&self, handle: u32) -> Result<Entry> {
        self.sessions.read().get(&handle).cloned().ok_or_else(|| anyhow!("no session {handle}"))
    }

    pub fn by_id(&self, id: &str) -> Option<Entry> {
        self.sessions.read().values().find(|e| e.spec().id == id).cloned()
    }

    /// For `auth`: whether session `id` is restricted and its folder; `None` if it is gone.
    pub fn session_policy(&self, id: &str) -> Option<(bool, String)> {
        self.by_id(id).map(|e| {
            let spec = e.spec();
            (spec.restricted, spec.cwd)
        })
    }

    pub fn session_id_of(&self, handle: u32) -> Option<String> {
        self.sessions.read().get(&handle).map(|e| e.spec().id)
    }

    pub fn agents_control_others(&self) -> bool {
        self.agents_control_others.load(Ordering::Relaxed)
    }

    /// Restores sessions recorded by a previous daemon run as exited-but-restartable entries
    /// whose screens show the last persisted snapshot. Sessions that were running are flagged
    /// `interrupted`: the app resumes them (agents into the same conversation) when shown, at
    /// once, or on request (`resumeSessions` setting). Chat sessions come back without a
    /// process; the app starts a chat front-end resuming the conversation in the same pane.
    pub fn restore(&self) {
        // Plugin agents first: restored sessions may be theirs.
        if let Some(defs) = std::fs::read(self.data_dir.join("custom-agents.json")).ok().and_then(|b| serde_json::from_slice::<Vec<agents::AgentDef>>(&b).ok()) {
            if let Err(e) = agents::set_custom(&defs) {
                tracing::warn!("plugin agents: {e:#}");
            }
        }
        for saved in self.store.load_sessions() {
            let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
            if saved.chat {
                let chat = ChatSession::new(handle, saved.spec.clone());
                if saved.alive {
                    chat.mark_interrupted(saved.last_activity);
                }
                self.notifier.seed(&chat.info());
                self.sessions.write().insert(handle, Entry::Chat(chat));
                continue;
            }
            let session = Session::new(handle, saved.spec.clone(), saved.cols, saved.rows);
            if let Some(snapshot) = self.store.load_snapshot(&saved.spec.id) {
                session.ingest(&snapshot);
                if saved.alive {
                    session.ingest(b"\r\n\x1b[0;2m[stopped when Kelpie's session daemon stopped]\x1b[0m\r\n");
                }
            }
            session.set_last_activity(saved.last_activity);
            if saved.alive {
                session.mark_interrupted(saved.last_activity);
            }
            self.notifier.seed(&session.info());
            self.sessions.write().insert(handle, Entry::Term(session));
        }
        self.settle_host_tasks();
    }

    pub fn create(&self, params: Value) -> Result<SessionInfo> {
        let p: CreateParams = serde_json::from_value(params).context("session.create params")?;
        let model = model_from_args(&p.args);
        let mut spec = SessionSpec {
            id: p.id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            project_id: p.project_id,
            name: p.name,
            kind: p.kind,
            cwd: p.cwd,
            args: p.args,
            env: p.env,
            model,
            agent_session_id: p.agent_session_id,
            created_at: now_millis(),
            remote: p.remote,
            restricted: p.restricted,
        };
        if self.by_id(&spec.id).is_some() {
            bail!("session id {} already exists", spec.id);
        }
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        let entry = if let Some(argv) = p.chat_argv {
            anyhow::ensure!(!argv.is_empty(), "chat_argv is empty");
            let argv = if spec.kind == "claude" && spec.remote.is_none() { agents::claude_chat_argv(argv, &spec.env) } else { argv };
            let chat = ChatSession::new(handle, spec);
            chat.spawn(&agents::platform_argv(argv)?)?;
            Entry::Chat(chat)
        } else {
            let fresh = Self::preset_conversation(&mut spec);
            let resume = match spec.agent_session_id.as_deref() {
                Some(id) if fresh => Resume::New(id),
                Some(_) => Self::resume_for(&spec),
                None => Resume::None,
            };
            let launch = Self::launch_for(&spec, resume, p.prompt.as_deref(), &self.exe, &self.data_dir)?;
            let session = Session::new(handle, spec, p.cols, p.rows);
            session.spawn(&launch)?;
            Entry::Term(session)
        };
        let info = entry.info();
        self.sessions.write().insert(handle, entry);
        self.meta_dirty.store(true, Ordering::Relaxed);
        self.notifier.seed(&info);
        self.broadcast("session.updated", serde_json::to_value(&info)?);
        self.checkpoint_baseline(&info.spec, "spawn");
        Ok(info)
    }

    pub fn restart(&self, handle: u32) -> Result<SessionInfo> {
        let Entry::Term(session) = self.entry(handle)? else { bail!("chat sessions cannot be restarted") };
        anyhow::ensure!(!session.is_alive(), "session is still running");
        // A local session from before ids were preset, whose conversation the app could not
        // find either, starts a new one with a known id. One over SSH keeps resuming the
        // folder's latest conversation, which is the one it had.
        let spec = {
            let mut spec = session.spec.lock();
            if spec.remote.is_none() && Self::preset_conversation(&mut spec) {
                self.meta_dirty.store(true, Ordering::Relaxed);
            }
            spec.clone()
        };
        let launch = Self::launch_for(&spec, Self::resume_for(&spec), None, &self.exe, &self.data_dir)?;
        self.drop_timer(&spec.id, "restarted");
        // The program starts below the old output, which stays readable above it. A resumed agent
        // redraws only the latest part of its conversation (Claude Code shows about a screenful),
        // so clearing the scrollback here would lose everything said before the restart.
        if matches!(spec.kind.as_str(), "shell" | "custom") {
            session.ingest(b"\x1b[0m\r\n\x1b[2m--- restarted ---\x1b[0m\r\n");
        } else {
            session.ingest(b"\x1b[0m\r\n\x1b[2m--- resumed ---\x1b[0m\r\n");
        }
        session.spawn(&launch)?;
        self.checkpoint_baseline(&spec, "restart");
        Ok(session.info())
    }

    pub fn close(&self, handle: u32) -> Result<()> {
        let entry = self.sessions.write().remove(&handle).ok_or_else(|| anyhow!("no session {handle}"))?;
        self.notifier.forget(handle);
        self.drop_timer(&entry.spec().id, "exited");
        entry.kill();
        self.store.remove_snapshot(&entry.spec().id);
        // Waits for a snapshot of its repository that is running: not on this request's worker.
        let (checkpoints, id, now) = (self.checkpoints.clone(), entry.spec().id, now_millis());
        let _ = std::thread::Builder::new().name("kelpie-checkpoint-drop".into()).spawn(move || checkpoints.close_session(&id, now));
        self.meta_dirty.store(true, Ordering::Relaxed);
        self.broadcast_event("session.closed", json!({"handle": handle}), Some(&entry.spec().id));
        Ok(())
    }

    pub fn update_spec(&self, handle: u32, params: &Value) -> Result<SessionInfo> {
        let entry = self.entry(handle)?;
        let spec_lock = match &entry {
            Entry::Term(s) => &s.spec,
            Entry::Chat(c) => &c.spec,
        };
        {
            let mut spec = spec_lock.lock();
            if let Some(name) = params.get("name") {
                spec.name = name.as_str().map(str::to_string);
            }
            if let Some(project) = params.get("project_id") {
                spec.project_id = project.as_str().map(str::to_string);
            }
            if let Some(id) = params.get("agent_session_id") {
                spec.agent_session_id = id.as_str().map(str::to_string);
            }
            if let Some(args) = params.get("args").and_then(Value::as_array) {
                spec.args = args.iter().filter_map(|a| a.as_str().map(str::to_string)).collect();
            }
            if let Some(model) = params.get("model") {
                spec.model = model.as_str().map(str::to_string);
            }
        }
        self.meta_dirty.store(true, Ordering::Relaxed);
        // Spec changes are user-visible: broadcast now (via tick, so the 250 ms ticker does not
        // send the same change a second time).
        let ticked = match &entry {
            Entry::Term(s) => s.tick(Instant::now()),
            Entry::Chat(c) => c.tick(Instant::now()),
        };
        if let Some(info) = &ticked {
            self.observe(info, Instant::now());
        }
        if let Some(v) = ticked.and_then(|i| serde_json::to_value(&i).ok()) {
            self.broadcast("session.updated", v);
        }
        Ok(entry.info())
    }

    /// `kelpied hook` reports from Claude Code / Codex running inside a session.
    pub fn hook(&self, params: &Value) -> Result<()> {
        let session_id = param(params, "session_id")?.as_str().unwrap_or_default();
        let agent = params.get("agent").and_then(Value::as_str).unwrap_or("");
        let payload = params.get("payload").cloned().unwrap_or(Value::Null);
        let Some(Entry::Term(session)) = self.by_id(session_id) else { return Ok(()) };
        // `auth` adds `verified`: true when the caller proved it is this session (its token), false for a
        // claimed id. Only a proof makes the session strict; a caller with the user's token proves nothing
        // about the agent. Once strict, unproven reports change nothing, not even the conversation id.
        let (proven, claimed) = (params.get("verified") == Some(&Value::Bool(true)), params.get("verified") == Some(&Value::Bool(false)));
        if !session.accepts_report(!claimed, proven) {
            return Ok(());
        }
        let event = payload
            .get("hook_event_name")
            .or_else(|| payload.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        tracing::debug!(session_id, agent, event, keys = ?payload.as_object().map(|o| o.keys().cloned().collect::<Vec<_>>()), "hook");
        let agent_conv = match agent {
            "claude" => payload.get("session_id"),
            "codex" => payload.get("thread-id").or_else(|| payload.get("thread_id")),
            _ => None,
        }
        .and_then(Value::as_str);
        if let Some(conv) = agent_conv {
            let mut spec = session.spec.lock();
            if spec.agent_session_id.as_deref() != Some(conv) {
                spec.agent_session_id = Some(conv.to_string());
                self.meta_dirty.store(true, Ordering::Relaxed);
            }
        }
        // Claude's SessionStart carries the model (string or {id, display_name}).
        let model = payload.get("model").and_then(|m| m.as_str().map(str::to_string).or_else(|| {
            m.get("display_name").or_else(|| m.get("id")).and_then(Value::as_str).map(str::to_string)
        }));
        if let Some(model) = model {
            let mut spec = session.spec.lock();
            if spec.model.as_deref() != Some(model.as_str()) {
                spec.model = Some(model);
                self.meta_dirty.store(true, Ordering::Relaxed);
            }
        }
        if let Some(status) = hook_event_status(agent, event, &payload) {
            session.set_hook_status(status, hook_event_failed(event));
            if hook_event_limited(event, &payload) {
                session.set_limited();
            }
        }
        // The app reads the conversation's usage when a turn moves on (it debounces).
        if !event.is_empty() {
            self.broadcast("session.turn", turn_event(session_id, event, &payload, agent_conv));
        }
        Ok(())
    }

    // ---------- turn checkpoints (see checkpoints.rs) ----------

    /// Agents on this computer get snapshots; shells and sessions on another host do not.
    fn checkpoint_eligible(spec: &SessionSpec) -> bool {
        spec.remote.is_none() && spec.kind != "shell"
    }

    /// Why a session has no snapshots when it never will (`None`: it can have them).
    fn checkpoint_why(&self, sid: &str) -> Option<&'static str> {
        let spec = self.by_id(sid)?.spec();
        if spec.remote.is_some() {
            Some("remote")
        } else if spec.kind == "shell" {
            Some("shell")
        } else if checkpoints::find_top(std::path::Path::new(&spec.cwd)).is_none() {
            Some("not-git")
        } else {
            None
        }
    }

    /// Where a session's history starts: its spawn or restart.
    fn checkpoint_baseline(&self, spec: &SessionSpec, source: &str) {
        if !Self::checkpoint_eligible(spec) {
            return;
        }
        let now = now_millis();
        self.checkpoints.submit(checkpoints::Job { sid: spec.id.clone(), cwd: spec.cwd.clone(), kind: checkpoints::Kind::Baseline, source: source.into(), approximate: false, started: now, at: now });
    }

    /// A turn ended: Claude `Stop`, Codex `agent-turn-complete`, a chat's result, or (for agents
    /// that report nothing) the output going quiet after input. The status the ticker sees is
    /// the one boundary for all of them, so a hook report alone never starts a snapshot. A turn
    /// that ends waiting on the user or with the agent exiting counts too. For a guessed boundary
    /// (`typed`: something was typed since the last one) a repaint of a quiet screen is not a turn.
    fn checkpoint_turn_end(&self, info: &SessionInfo, hooks: bool, typed: bool, started: u64) {
        if !Self::checkpoint_eligible(&info.spec) {
            return;
        }
        let at = now_millis();
        let (source, approximate) = if info.chat {
            ("chat", false)
        } else if hooks && matches!(info.spec.kind.as_str(), "claude" | "codex") {
            ("hook", false)
        } else {
            ("idle", true)
        };
        if approximate && !typed {
            return;
        }
        self.checkpoints.submit(checkpoints::Job { sid: info.spec.id.clone(), cwd: info.spec.cwd.clone(), kind: checkpoints::Kind::Turn, source: source.into(), approximate, started, at });
    }

    /// A turn is starting (Idle -> Working: a typed prompt, a chat message): a detection-only look at
    /// the folder, so what changed between turns is known even if the agent edits it too. A guessed
    /// boundary needs typing since the last one, as the end of a turn does.
    fn checkpoint_turn_start(&self, info: &SessionInfo, hooks: bool, typed: bool) {
        if !Self::checkpoint_eligible(&info.spec) {
            return;
        }
        let guessed = !info.chat && !(hooks && matches!(info.spec.kind.as_str(), "claude" | "codex"));
        if guessed && !typed {
            return;
        }
        let now = now_millis();
        self.checkpoints.submit(checkpoints::Job { sid: info.spec.id.clone(), cwd: info.spec.cwd.clone(), kind: checkpoints::Kind::Start, source: "start".into(), approximate: false, started: now, at: now });
    }

    /// At startup, off the main path: snapshots of sessions that no longer exist go.
    pub fn collect_checkpoints(self: &Arc<Self>) {
        let known: std::collections::HashSet<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
        let me = self.clone();
        let _ = std::thread::Builder::new().name("kelpie-checkpoint-gc".into()).spawn(move || {
            let removed = me.checkpoints.gc(&known);
            if removed > 0 {
                tracing::info!(removed, "removed checkpoints of sessions that no longer exist");
            }
        });
    }

    fn session_label(&self, sid: &str) -> String {
        match self.by_id(sid) {
            Some(e) => {
                let spec = e.spec();
                spec.name.filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{} {}", spec.kind, sid.chars().take(6).collect::<String>()))
            }
            None => format!("a closed session ({})", sid.chars().take(6).collect::<String>()),
        }
    }

    /// What undo needs to know about the sessions around `sid`.
    fn undo_ctx(&self, sid: &str) -> checkpoints::UndoCtx {
        // Output within the window status uses for "working" counts whatever a hook last said: a
        // report without the session's token (any program that knows its id) must not make a busy
        // agent look idle.
        let busy = |e: &Entry| {
            matches!(e.info().status, Status::Working | Status::Waiting) || matches!(e, Entry::Term(s) if s.output_within(Duration::from_millis(1500), Instant::now()))
        };
        let owner_working = self.by_id(sid).is_some_and(|e| busy(&e));
        let key = self.checkpoints.session_repo_key(sid);
        let others: Vec<String> = self
            .sessions
            .read()
            .values()
            .filter(|e| busy(e))
            .map(|e| e.spec())
            .filter(|spec| spec.id != sid && key.is_some() && checkpoints::find_top(std::path::Path::new(&spec.cwd)).map(|t| checkpoints::repo_key(&t)) == key)
            .map(|spec| spec.id)
            .collect();
        let working_others = others.into_iter().map(|id| { let name = self.session_label(&id); (id, name) }).collect();
        checkpoints::UndoCtx { owner_working, working_others }
    }

    fn checkpoint_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        let sid = || param(params, "session").map(|v| v.as_str().unwrap_or_default().to_string());
        let n = || param(params, "n").and_then(|v| v.as_u64().map(|n| n as u32).ok_or_else(|| anyhow!("bad checkpoint number")));
        let name = |s: &str| self.session_label(s);
        Ok(match method {
            "checkpoint.status" => self.checkpoints.status(),
            "checkpoint.set_enabled" => {
                self.checkpoints.set_enabled(params.get("enabled").and_then(Value::as_bool), params.get("repo").and_then(Value::as_str));
                self.checkpoints.status()
            }
            "checkpoint.list" => {
                let sid = sid()?;
                self.checkpoints.list(&sid, self.checkpoint_why(&sid))
            }
            "checkpoint.owners" => self.checkpoints.owners(&sid()?, &name),
            "checkpoint.diff" => self.checkpoints.diff(&sid()?, n()?)?,
            "checkpoint.blob" => {
                let path = param(params, "path")?.as_str().unwrap_or_default();
                let flag = |k: &str| params.get(k).and_then(Value::as_bool).unwrap_or(false);
                self.checkpoints.blob(&sid()?, n()?, path, flag("base64"), flag("before"))?
            }
            "checkpoint.undo_plan" => {
                let sid = sid()?;
                self.checkpoints.undo_plan(&sid, n()?, &self.undo_ctx(&sid), &name)?
            }
            "checkpoint.undo" => {
                let sid = sid()?;
                let list = |k: &str| params.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect::<Vec<_>>());
                let (paths, confirm) = (list("paths"), list("confirm").unwrap_or_default());
                let req = checkpoints::UndoRequest { n: n()?, paths: paths.as_deref(), confirm: &confirm, at: now_millis() };
                self.checkpoints.undo(&sid, &req, &self.undo_ctx(&sid), &name)?
            }
            "checkpoint.drop" => {
                let worktree = param(params, "worktree")?.as_str().unwrap_or_default();
                json!({"dropped": self.checkpoints.drop_worktree(worktree, params.get("root").and_then(Value::as_str))})
            }
            other => bail!("unknown method {other}"),
        })
    }

    pub fn list(&self) -> Vec<SessionInfo> {
        self.sessions.read().values().map(Entry::info).collect()
    }

    fn saved_sessions(&self) -> Vec<SavedSession> {
        self.sessions
            .read()
            .values()
            .map(|e| {
                let info = e.info();
                SavedSession {
                    spec: info.spec,
                    title: info.title,
                    cols: info.cols,
                    rows: info.rows,
                    chat: info.chat,
                    last_activity: info.last_activity,
                    alive: info.alive || info.interrupted,
                }
            })
            .collect()
    }

    /// Persists metadata (when dirty) and screen snapshots (when changed or `force`).
    pub fn persist(&self, force: bool) {
        if force || self.meta_dirty.swap(false, Ordering::Relaxed) {
            if let Err(e) = self.store.save_sessions(&self.saved_sessions()) {
                tracing::warn!("save sessions: {e:#}");
            }
        }
        let terms: Vec<Arc<Session>> = self
            .sessions
            .read()
            .values()
            .filter_map(|e| if let Entry::Term(s) = e { Some(s.clone()) } else { None })
            .collect();
        for s in terms {
            if let Some(snapshot) = s.snapshot_if_changed(force) {
                let id = s.spec.lock().id.clone();
                if let Err(e) = self.store.save_snapshot(&id, &snapshot) {
                    tracing::warn!("save snapshot {id}: {e:#}");
                }
            }
        }
    }

    /// Status changes are coalesced here (250 ms) instead of being pushed per output chunk,
    /// so 20 busy agents cost the UI at most a few small events per second.
    pub async fn ticker(self: Arc<Self>) {
        let mut interval = tokio::time::interval(Duration::from_millis(250));
        let mut last_persist = Instant::now();
        let mut last_schedule = Instant::now();
        let mut last_alive: HashMap<u32, bool> = HashMap::new();
        let mut last_status: HashMap<u32, Status> = HashMap::new();
        let mut last_boundary: HashMap<u32, Instant> = HashMap::new();
        let mut last_run_tick = Instant::now();
        loop {
            interval.tick().await;
            let now = Instant::now();
            if now.duration_since(last_run_tick) >= Duration::from_secs(1) {
                last_run_tick = now;
                self.run_tick();
            }
            for item in self.scheduled.take_due(now_millis()) {
                self.fire_scheduled(item);
            }
            if now.duration_since(last_schedule) >= Duration::from_secs(10) {
                last_schedule = now;
                self.update_awake();
                // A phone-access change that could not be saved is tried again, while the server may be off.
                if self.mobile.save_pending() && self.mobile.claim_retry() {
                    let me = self.clone();
                    tokio::task::spawn_blocking(move || {
                        let saved = me.mobile.retry_save();
                        me.mobile.retry_ended();
                        if saved {
                            me.broadcast("mobile.updated", json!({}));
                        }
                    });
                }
                if self.presence.newly_stuck() {
                    self.mobile.audit("-", "verify_stuck", "a verification prompt has been up for over 10 minutes; no new one can be asked until it ends");
                }
                // Each on its own thread: a run that takes a while never holds up the ticker.
                self.run_due_automations();
                self.poll_commits(false);
                self.announce_automation_problem();
            }
            let entries: Vec<Entry> = self.sessions.read().values().cloned().collect();
            let prioritize = self.prioritize.load(Ordering::Relaxed);
            let priorities: Vec<_> = entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Term(s) => s.priority(prioritize),
                    Entry::Chat(c) => c.priority(prioritize),
                })
                .collect();
            // Process snapshots and per-process calls block: keep them off the async workers.
            if !priorities.is_empty() {
                tokio::task::spawn_blocking(move || {
                    for (tree, p) in priorities {
                        tree.set_priority(p);
                    }
                });
            }
            for entry in entries {
                let changed = match &entry {
                    Entry::Term(s) => s.tick(now),
                    Entry::Chat(c) => c.tick(now),
                };
                if let Some(info) = changed {
                    // Started or stopped: `sessions.json` records which sessions were running,
                    // so a later restart resumes only those (see `restore`).
                    if last_alive.insert(info.handle, info.alive) != Some(info.alive) {
                        self.meta_dirty.store(true, Ordering::Relaxed);
                    }
                    if let Ok(v) = serde_json::to_value(&info) {
                        self.broadcast("session.updated", v);
                    }
                    self.observe(&info, now);
                    // A turn ended (also for agents without hooks): the app reads the usage.
                    let prev = if info.alive { last_status.insert(info.handle, info.status) } else { last_status.remove(&info.handle) };
                    if info.status == Status::Working && prev != Some(Status::Working) {
                        self.checkpoints.work_started(&info.spec.id, now_millis());
                        let (hooks, input) = match &entry {
                            Entry::Term(s) => (s.hooks_seen(), s.last_input()),
                            Entry::Chat(_) => (false, None),
                        };
                        let typed = input.is_some_and(|i| last_boundary.get(&info.handle).is_none_or(|b| i > *b));
                        let (me, info) = (self.clone(), info.clone());
                        tokio::task::spawn_blocking(move || me.checkpoint_turn_start(&info, hooks, typed));
                    }
                    if matches!(prev, Some(Status::Working | Status::Waiting)) && matches!(info.status, Status::Idle | Status::Exited) {
                        let (hooks, input) = match &entry {
                            Entry::Term(s) => (s.hooks_seen(), s.last_input()),
                            Entry::Chat(_) => (false, None),
                        };
                        let boundary = last_boundary.insert(info.handle, now);
                        let typed = input.is_some_and(|i| boundary.is_none_or(|b| i > b));
                        // Taken here, not in the blocking task: the next turn may start before it runs.
                        let started = self.checkpoints.take_work_start(&info.spec.id, now_millis());
                        // Looking for the repository touches the disk: not on the ticker.
                        let (me, info) = (self.clone(), info.clone());
                        // Automations see the turn end first, in case it was their own session's.
                        {
                            let me = me.clone();
                            let info = info.clone();
                            tokio::task::spawn_blocking(move || me.automation_turn_ended(&info));
                        }
                        tokio::task::spawn_blocking(move || me.checkpoint_turn_end(&info, hooks, typed, started));
                    }
                    if (matches!(prev, Some(Status::Working | Status::Waiting)) && matches!(info.status, Status::Idle | Status::Exited)) || !info.alive {
                        self.deliver_to_review(&info.spec.id);
                    }
                    if prev == Some(Status::Working) && matches!(info.status, Status::Idle | Status::Waiting) && !info.chat {
                        self.broadcast("session.turn", turn_event(&info.spec.id, "turn_end", &Value::Null, info.spec.agent_session_id.as_deref()));
                    }
                }
            }
            if now.duration_since(last_persist) > Duration::from_secs(30) || self.meta_dirty.load(Ordering::Relaxed) {
                let me = self.clone();
                let full = now.duration_since(last_persist) > Duration::from_secs(30);
                if full {
                    last_persist = now;
                }
                let _ = tokio::task::spawn_blocking(move || {
                    if full {
                        me.persist(false)
                    } else if me.meta_dirty.swap(false, Ordering::Relaxed) {
                        let _ = me.store.save_sessions(&me.saved_sessions());
                    }
                })
                .await;
            }
        }
    }

    /// Handles one JSON-RPC style request. Returns `None` when the method already replied
    /// (attach replies inline to preserve ordering with the output stream).
    pub fn dispatch(self: &Arc<Self>, client: u64, tx: &ClientTx, id: &Value, method: &str, params: Value) -> Result<Option<Value>> {
        Ok(Some(match method {
            "daemon.info" => json!({
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": kelpie_proto::PROTOCOL_VERSION,
                "api": kelpie_proto::API_LEVEL,
                "security": kelpie_proto::DAEMON_SECURITY_LEVEL,
                "pid": std::process::id(),
                "test_user_presence": crate::user_presence::test_seam(),
                // Where it runs from: a copy outside the install folder survives updates.
                "exe": self.exe,
                "data_dir": self.data_dir,
                "endpoint": kelpie_proto::endpoint().display(),
                "sessions": self.sessions.read().len(),
                // Looks at running processes made so far: only the app's requests make one.
                "process_scans": self.procs.scans(),
            }),
            "daemon.policy" => json!({"agents_control_others": self.agents_control_others()}),
            "daemon.set_policy" => {
                if let Some(on) = params.get("agents_control_others").and_then(Value::as_bool) {
                    self.agents_control_others.store(on, Ordering::Relaxed);
                    save_policy(&self.data_dir, on);
                }
                json!({"agents_control_others": self.agents_control_others()})
            }
            "daemon.set_theme" => {
                let light = params.get("theme").and_then(Value::as_str) == Some("light");
                match params.get("palette").filter(|p| p.is_object()) {
                    Some(palette) => crate::vt::set_palette(light, palette),
                    None => crate::vt::set_light_theme(light),
                }
                json!({})
            }
            "daemon.set_priority" => {
                let enabled = params.get("enabled").and_then(Value::as_bool).unwrap_or(true);
                self.prioritize.store(enabled, Ordering::Relaxed);
                crate::proc_tree::set_daemon_priority(enabled);
                json!({"enabled": enabled})
            }
            "daemon.set_awake" => {
                *self.awake_mode.lock() = awake::Mode::parse(params.get("mode").and_then(Value::as_str).unwrap_or("off"));
                self.update_awake();
                json!({"held": self.awake.held()})
            }
            // The window tells which sessions it shows (API 12): what a focused window shows is not
            // raised as an event, for the desktop and the phone alike.
            "ui.focus" => {
                let focused = params.get("focused").and_then(Value::as_bool).unwrap_or(false);
                let visible = params.get("visible").and_then(Value::as_array).map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()).unwrap_or_default();
                self.notifier.set_ui(client, focused, visible, params.get("active_at").and_then(Value::as_u64));
                json!(true)
            }
            "notify.get" => self.notifier.describe(params.get("reveal_topic").and_then(Value::as_bool) == Some(true)),
            "notify.set" => {
                let config = match params.get("config") {
                    Some(c) if !c.is_null() => Some(serde_json::from_value(c.clone()).context("notify config")?),
                    _ => None,
                };
                self.notifier.update(config, params.get("secrets").unwrap_or(&Value::Null))?;
                self.notifier.describe(false)
            }
            "notify.rotate_topic" => json!({"topic": self.notifier.rotate_topic()?}),
            "notify.clear" => {
                self.notifier.clear()?;
                self.notifier.describe(false)
            }
            // Sends on a thread of its own: a slow server holds no other request.
            "notify.test" => {
                let channel = notify::Channel::parse(param(&params, "channel")?.as_str().unwrap_or_default()).ok_or_else(|| anyhow!("unknown channel"))?;
                let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
                std::thread::Builder::new().name("kelpie-notify-test".into()).spawn(move || {
                    let reply = match me.notifier.test(channel) {
                        Ok(()) => json!({"id": id, "result": true}),
                        Err(e) => json!({"id": id, "error": {"message": e}}),
                    };
                    let _ = tx.send(json_out(&reply));
                })?;
                return Ok(None);
            }
            // Only the app reads usage, so only it knows a session passed its cost cap: the
            // channels carry that on (the app shows it itself).
            "notify.report" => {
                anyhow::ensure!(param(&params, "kind")?.as_str() == Some("cost"), "unknown report");
                let sid = param(&params, "session")?.as_str().unwrap_or_default();
                let project = self.by_id(sid).and_then(|e| e.spec().project_id);
                let detail = params.get("detail").and_then(Value::as_str).map(|s| s.chars().take(200).collect());
                self.raise(Raised::cost(sid, project, detail), false);
                json!(true)
            }
            "session.schedule_continue" => {
                let Entry::Term(s) = self.entry(handle_of(&params)?)? else { bail!("only terminal sessions can continue at reset") };
                anyhow::ensure!(s.info().limited, "the session is not stopped by a usage limit");
                let at = param(&params, "at")?.as_u64().ok_or_else(|| anyhow!("bad reset time"))?;
                let source = param(&params, "source")?.as_str().unwrap_or_default();
                let (sid, conversation) = {
                    let spec = s.spec.lock();
                    (spec.id.clone(), spec.agent_session_id.clone())
                };
                let item = self.scheduled.schedule(&sid, s.generation(), conversation, s.turn_seq(), at, source, now_millis())?;
                self.broadcast("scheduled.updated", json!({"session": sid, "at": item.fire_at, "source": item.source}));
                json!(item)
            }
            "session.cancel_scheduled" => {
                let sid = self.session_id_of(handle_of(&params)?).ok_or_else(|| anyhow!("no such session"))?;
                let cancelled = self.scheduled.cancel(&sid);
                if cancelled {
                    self.broadcast("scheduled.updated", json!({"session": sid, "at": null, "result": "cancelled", "reason": "user"}));
                }
                json!(cancelled)
            }
            // The timers now, and the ones a daemon that stopped left behind (reported once).
            "session.scheduled" => json!({"timers": self.scheduled.list(), "lost": self.scheduled.take_lost()}),
            "session.set_env_restart" => serde_json::to_value(self.set_env_restart(handle_of(&params)?, params.get("env").unwrap_or(&Value::Null))?)?,
            "agent.presets" => {
                let installed = agents::installed_presets();
                json!(agents::all_presets()
                    .into_iter()
                    .map(|p| json!({
                        "id": p.id,
                        "label": p.label,
                        "bin": p.bin,
                        "yolo": p.yolo,
                        "resume_latest": !p.resume_latest.is_empty(),
                        "acp": p.acp,
                        "installed": installed.contains(&p.id),
                    }))
                    .collect::<Vec<_>>())
            }
            "agent.run" => {
                self.agent_run(tx, id, &params)?;
                return Ok(None);
            }
            "agent.set_custom" => {
                let defs: Vec<agents::AgentDef> = serde_json::from_value(params.get("agents").cloned().unwrap_or_else(|| json!([]))).context("agents")?;
                agents::set_custom(&defs)?;
                // Kept for the next start: restored sessions of plugin agents must still resume.
                write_atomic(&self.data_dir.join("custom-agents.json"), &serde_json::to_vec_pretty(&defs)?)?;
                json!(true)
            }
            "mobile.status" => self.mobile.status(self.client_roles.lock().get(&client) == Some(&crate::auth::Role::App)),
            "mobile.load" => {
                if params.get("reset").and_then(Value::as_bool) == Some(true) {
                    self.mobile.load.reset_lags();
                }
                self.mobile.load.report()
            }
            "mobile.enable" => {
                // Binding the listener is async: answer when it is up (or failed).
                let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
                tokio::spawn(async move {
                    let reply = match crate::mobile::enable(me.clone(), params).await {
                        Ok(v) => json!({"id": id, "result": v}),
                        Err(e) => json!({"id": id, "error": {"message": format!("{e:#}")}}),
                    };
                    let _ = tx.send(json_out(&reply));
                    me.broadcast("mobile.updated", json!({}));
                });
                return Ok(None);
            }
            "mobile.disable" => {
                if self.mobile.is_on() {
                    self.mobile.audit("-", "phone_access_off", "turned off");
                }
                self.mobile.disable();
                crate::mobile::tailscale::remove_after_off(self);
                // Nobody is let in any more: what phones had waiting is not written.
                for e in self.sessions.read().values() {
                    if let Entry::Chat(c) = e {
                        c.drop_phones("revoked");
                    }
                }
                self.broadcast("mobile.updated", json!({}));
                self.mobile.unsaved(crate::mobile::NOT_SAVED_OFF)?;
                json!(true)
            }
            "mobile.pair_start" => self.mobile.pair_start(params.get("plain").and_then(Value::as_bool) == Some(true))?,
            // Runs the Tailscale CLI, so never on this thread. Only when someone asks: Settings → Mobile, open.
            "mobile.tailscale_status" => {
                let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
                tokio::spawn(async move {
                    let _ = tx.send(json_out(&rpc_answer(&id, crate::mobile::tailscale::status(me).await)));
                });
                return Ok(None);
            }
            // The verified app only (`auth::APP_ONLY`). Turning it off takes access away and asks nobody. Turning it on is checked first, with
            // no prompt (a port someone else uses, Tailscale not ready), then the person verifies, and only then is anything published.
            "mobile.set_tailscale" => {
                if !self.is_verified_app(client) {
                    bail!("this needs the Kelpie app: phone access is decided in Kelpie → Settings → Mobile");
                }
                let on = param(&params, "on")?.as_bool().ok_or_else(|| anyhow!("on must be true or false"))?;
                let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
                tokio::spawn(async move {
                    use crate::mobile::tailscale;
                    let answer = |r: Result<Value>| {
                        let _ = tx.send(json_out(&rpc_answer(&id, r)));
                    };
                    if !on {
                        return answer(tailscale::turn_off(me).await);
                    }
                    let asked_at = match tailscale::check_can_turn_on(&me).await {
                        Ok(at) => at,
                        Err(e) => return answer(Err(e)),
                    };
                    let runtime = tokio::runtime::Handle::current();
                    let asked = me.after_verifying(client, &tx, &id, &params, crate::user_presence::Prompt::Tailscale, move |me| runtime.block_on(tailscale::turn_on(me.clone(), asked_at)));
                    if let Err(e) = asked {
                        answer(Err(e));
                    }
                });
                return Ok(None);
            }
            // The verified app only (`auth::APP_ONLY`). Declining needs nothing more. Allowing is checked first, with no prompt
            // (a wrong code counts, and is answered at once), then the person verifies, and only then is the device added.
            "mobile.approve" => {
                if params.get("allow").and_then(Value::as_bool) != Some(true) {
                    self.mobile.decline(&params)?;
                    self.broadcast("mobile.updated", json!({}));
                    return Ok(Some(json!(false)));
                }
                let checked = self.mobile.check_approval(&params);
                if self.mobile.take_lockout() {
                    self.broadcast("mobile.lockout", json!({"reason": "sas"}));
                    self.broadcast("mobile.updated", json!({}));
                }
                let checked = checked?;
                let prompt = checked.prompt();
                self.after_verifying(client, tx, id, &params, prompt, move |me| {
                    let approved = me.mobile.commit_approval(&checked)?;
                    for old in &approved.replaced {
                        me.drop_device_queues(old, "revoked");
                    }
                    me.broadcast("mobile.device_changed", approved.event.json());
                    me.broadcast("mobile.updated", json!({}));
                    Ok(approved.reply)
                })?;
                return Ok(None);
            }
            "mobile.revoke" => {
                // The permission goes first, then what the device has waiting: a message that was being queued as this runs finds
                // the permission gone (`enqueue_if`), or is dropped here. A turn already written runs on.
                let id = param(&params, "id")?.as_str().unwrap_or_default();
                let event = self.mobile.revoke(id)?;
                self.drop_device_queues(id, "revoked");
                self.broadcast("mobile.device_changed", event.json());
                self.broadcast("mobile.updated", json!({}));
                self.mobile.unsaved(crate::mobile::NOT_SAVED_REMOVED)?;
                json!(true)
            }
            // `mobile.set_write {id, can_write}` is the name from before API 22, kept for one release. One reading of the parameters decides
            // both whether this raises access and what is applied: lowering takes effect at once, raising after the person verified.
            "mobile.set_permission" | "mobile.set_write" => {
                let device = param(&params, "id")?.as_str().unwrap_or_default().to_string();
                let permission = crate::mobile::requested_permission(&params)?;
                let Some(prompt) = self.mobile.raise_prompt(&device, permission)? else {
                    self.apply_permission(&device, permission)?;
                    self.mobile.unsaved(crate::mobile::NOT_SAVED_LOWERED)?;
                    return Ok(Some(json!(true)));
                };
                self.after_verifying(client, tx, id, &params, prompt, move |me| {
                    me.apply_permission(&device, permission)?;
                    Ok(json!(true))
                })?;
                return Ok(None);
            }
            // The kill switch: phone access off and every device removed, online or not. It only takes access away: any
            // connection of the user may ask, with no verification.
            "mobile.revoke_all" => {
                let events = self.mobile.revoke_all();
                crate::mobile::tailscale::remove_after_off(self);
                for e in self.sessions.read().values() {
                    if let Entry::Chat(c) = e {
                        c.drop_phones("revoked");
                    }
                }
                for event in events {
                    self.broadcast("mobile.device_changed", event.json());
                }
                self.broadcast("mobile.updated", json!({}));
                self.mobile.unsaved(crate::mobile::NOT_SAVED_REMOVED)?;
                json!(true)
            }
            // Keeping unused phones longer (or for ever) weakens phone access: the verified app and the person only.
            "mobile.set_options" => {
                let days = crate::mobile::requested_dormant_days(&params)?;
                match days.and_then(|days| self.mobile.dormant_prompt(days)) {
                    Some(prompt) => {
                        if !self.is_verified_app(client) {
                            bail!("{method} that keeps unused phones longer needs the Kelpie app: change it in Kelpie → Settings → Mobile");
                        }
                        self.after_verifying(client, tx, id, &params, prompt, move |me| {
                            let v = me.mobile.set_options(days);
                            me.broadcast("mobile.updated", json!({}));
                            Ok(v)
                        })?;
                        return Ok(None);
                    }
                    None => {
                        let v = self.mobile.set_options(days);
                        self.broadcast("mobile.updated", json!({}));
                        v
                    }
                }
            }
            // What phones did: newest first, and whether the log's hash chain holds.
            "mobile.audit" => self.mobile.audit_entries(params.get("limit").and_then(Value::as_u64).unwrap_or(200) as usize),
            // Forget every phone, make new keys, turn phone access off (also the way out of an unreadable store file). Always verified.
            "mobile.reset" => {
                self.after_verifying(client, tx, id, &params, crate::user_presence::Prompt::Reset, |me| {
                    let events = me.mobile.reset();
                    crate::mobile::tailscale::remove_after_off(me);
                    for e in me.sessions.read().values() {
                        if let Entry::Chat(c) = e {
                            c.drop_phones("revoked");
                        }
                    }
                    for event in events {
                        me.broadcast("mobile.device_changed", event.json());
                    }
                    me.broadcast("mobile.updated", json!({}));
                    me.mobile.unsaved(crate::mobile::NOT_SAVED_REMOVED)?;
                    Ok(json!(true))
                })?;
                return Ok(None);
            }
            "mobile.set_projects" => {
                self.mobile.set_projects(&params);
                json!(true)
            }
            "meta.list" => serde_json::to_value(&self.workspace_meta.lock().map)?,
            "meta.get" => {
                let path = param(&params, "path")?.as_str().unwrap_or_default();
                match self.workspace_meta.lock().find(path) {
                    Some((key, m)) => json!({"path": key, "comment": m.comment, "status": m.status, "updated_at": m.updated_at}),
                    None => Value::Null,
                }
            }
            "meta.set" => {
                let path = param(&params, "path")?.as_str().unwrap_or_default();
                let opt = |k: &str| params.get(k).map(|v| v.as_str().unwrap_or(""));
                self.set_meta(path, opt("comment"), opt("status"))?
            }
            m if m.starts_with("automation.") => self.automation_rpc(client, m, params)?,
            m if m.starts_with("run.") => {
                let out = self.run_rpc(m, &params)?;
                // What lets a task start (a new run, an approval, a retry, a task that ended) does not wait for the next tick.
                if matches!(m, "run.create" | "run.approve" | "run.retry" | "run.task_done" | "run.task_skip") && tokio::runtime::Handle::try_current().is_ok() {
                    self.run_tick();
                }
                out
            }
            m if m.starts_with("note.") => self.note_rpc(m, &params)?,
            m if m.starts_with("process.") => return self.process_rpc(tx, id, m, params),
            // Undo, its preview and drop wait for the repository's snapshots and run git: they
            // answer from their own thread so this client's other requests are not held up.
            m @ ("checkpoint.undo_plan" | "checkpoint.undo" | "checkpoint.drop") => {
                let (me, tx, id, method) = (self.clone(), tx.clone(), id.clone(), m.to_string());
                std::thread::Builder::new().name("kelpie-checkpoint-rpc".into()).spawn(move || {
                    let reply = match me.checkpoint_rpc(&method, &params) {
                        Ok(v) => json!({"id": id, "result": v}),
                        Err(e) => json!({"id": id, "error": {"message": format!("{e:#}")}}),
                    };
                    let _ = tx.send(json_out(&reply));
                })?;
                return Ok(None);
            }
            m if m.starts_with("checkpoint.") => self.checkpoint_rpc(m, &params)?,
            "daemon.shutdown" => {
                // A hand-over to a newer daemon: only when nothing runs any more (checked here,
                // not from a session list the app read a moment ago).
                if params.get("if_idle").and_then(Value::as_bool) == Some(true) && self.sessions.read().values().any(|e| e.info().alive) {
                    return Ok(Some(json!(false)));
                }
                if params.get("kill_sessions").and_then(Value::as_bool).unwrap_or(false) {
                    for e in self.sessions.read().values() {
                        e.kill();
                    }
                    // Unix kills escalate to SIGKILL after 1.5 s; stay alive until then.
                    std::thread::sleep(std::time::Duration::from_millis(1700));
                }
                self.shutdown.notify_one();
                json!(true)
            }
            "session.list" => serde_json::to_value(self.list())?,
            "session.create" => serde_json::to_value(self.create(params)?)?,
            "session.attach" => {
                let handle = handle_of(&params)?;
                match self.entry(handle)? {
                    Entry::Term(s) => {
                        let since = params.get("since_seq").and_then(Value::as_u64);
                        let interactive = params.get("interactive").and_then(Value::as_bool).unwrap_or(true);
                        s.attach(client, tx, id, since, interactive);
                    }
                    Entry::Chat(c) => c.attach(client, tx, id),
                }
                return Ok(None);
            }
            "session.detach" => {
                self.entry(handle_of(&params)?)?.detach(client);
                json!(true)
            }
            "session.input" => {
                let data = param(&params, "data")?.as_str().unwrap_or_default();
                self.entry(handle_of(&params)?)?.input(data.as_bytes().to_vec());
                json!(true)
            }
            // An answer to the prompt a session waits on, from a quick-answer popover: typed only
            // while it still waits on that same prompt (`waiting_since`), so an answer given
            // elsewhere meanwhile (phone, terminal) or a prompt that changed never gets stray keys.
            "session.answer" => {
                let Entry::Term(s) = self.entry(handle_of(&params)?)? else { bail!("session.answer is for terminal sessions") };
                let since = params.get("waiting_since").and_then(Value::as_u64);
                // `key`: Esc or Enter alone (text goes through `typed_text`, which drops controls).
                let answer = match params.get("key").and_then(Value::as_str) {
                    Some(key) => Answer::Key(key),
                    None => Answer::Text(param(&params, "data")?.as_str().unwrap_or_default()),
                };
                let enter = params.get("enter").and_then(Value::as_bool) == Some(true);
                match s.answer(since, answer, enter) {
                    Ok(()) => json!(true),
                    Err(AnswerError::UnknownKey) => bail!("unknown key"),
                    Err(_) => bail!("the session no longer waits on that prompt"),
                }
            }
            "session.resize" => {
                let cols = param(&params, "cols")?.as_u64().unwrap_or(80) as u16;
                let rows = param(&params, "rows")?.as_u64().unwrap_or(24) as u16;
                if let Entry::Term(s) = self.entry(handle_of(&params)?)? {
                    s.resize(cols.clamp(2, 1000), rows.clamp(1, 500));
                }
                json!(true)
            }
            "session.focus" => {
                let focused = params.get("focused").and_then(Value::as_bool).unwrap_or(true);
                match self.entry(handle_of(&params)?)? {
                    Entry::Term(s) => s.set_focus(focused),
                    Entry::Chat(c) => c.set_focus(focused),
                }
                json!(true)
            }
            "session.set_unread" => {
                let unread = params.get("unread").and_then(Value::as_bool).unwrap_or(true);
                match self.entry(handle_of(&params)?)? {
                    Entry::Term(s) => s.set_unread(unread),
                    Entry::Chat(c) => c.set_unread(unread),
                }
                json!(true)
            }
            "session.kill" => {
                self.entry(handle_of(&params)?)?.kill();
                json!(true)
            }
            "session.close" => {
                self.close(handle_of(&params)?)?;
                json!(true)
            }
            "session.restart" => serde_json::to_value(self.restart(handle_of(&params)?)?)?,
            "session.update" => serde_json::to_value(self.update_spec(handle_of(&params)?, &params)?)?,
            "session.read" => match self.entry(handle_of(&params)?)? {
                Entry::Term(s) => {
                    // `lines`: that many lines including scrollback; default the visible screen.
                    let text = match params.get("lines").and_then(Value::as_u64) {
                        Some(n) => s.text_tail(n.clamp(1, crate::session::SCROLLBACK as u64) as usize),
                        None => s.screen_text(),
                    };
                    // `waiting_since` with the screen: an answer names the prompt it was shown.
                    json!({"text": text, "title": s.title(), "prompt": s.screen_prompt(), "waiting_since": s.info().waiting_since})
                }
                Entry::Chat(_) => bail!("session.read is for terminal sessions"),
            },
            // The turn queue (`chat.rs`): the desktop's messages go through it with a phone's.
            m @ ("chat.send" | "chat.write_turn" | "chat.queue_op") => {
                let Entry::Chat(c) = self.entry(handle_of(&params)?)? else { bail!("{m} is for chat sessions") };
                c.queue_rpc(client, m, &params)?
            }
            "chat.close_stdin" => {
                if let Entry::Chat(c) = self.entry(handle_of(&params)?)? {
                    c.close_stdin();
                }
                json!(true)
            }
            "hook.report" => {
                self.hook(&params)?;
                json!(true)
            }
            other => bail!("unknown method {other}"),
        }))
    }
}

const POLICY_FILE: &str = "policy.json";

/// Scripting other sessions is allowed unless the user turned it off.
fn load_policy(data_dir: &std::path::Path) -> bool {
    std::fs::read(data_dir.join(POLICY_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("agents_control_others").and_then(Value::as_bool))
        .unwrap_or(true)
}

fn save_policy(data_dir: &std::path::Path, agents_control_others: bool) {
    if let Err(e) = std::fs::write(data_dir.join(POLICY_FILE), json!({"agents_control_others": agents_control_others}).to_string()) {
        tracing::warn!("save {POLICY_FILE}: {e}");
    }
}

/// A task's new worktree.
struct TaskWorktree {
    path: String,
    branch: String,
    /// The branch the run folder was on: the worktree is forked from it.
    base: Option<String>,
    /// What it was given from the repository's ignored files (none when the run asked for nothing).
    brought: Option<kelpie_proto::materialize::Materialized>,
}

/// A new git worktree for a task: branch `kelpie/<run>-<id>-<task>` from the run folder's HEAD, in
/// `<repo>-worktrees/` next to the repository (where Kelpie's worktree dialog puts them), with the
/// ignored files the run was approved to bring (`copy:` / `link:` of `.kelpie.yaml`). Returns the
/// folder, the branch, the branch it was forked from and what was brought.
fn create_task_worktree(run: &runs::Run, task: &runs::Task) -> Result<TaskWorktree> {
    let git = |args: &[&str]| -> Result<String> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(args).stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().context("run git")?;
        if !out.status.success() {
            bail!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    // The repository's top folder and the branch it is on, from one git run.
    let head = git(&["-C", &run.cwd, "rev-parse", "--show-toplevel", "--abbrev-ref", "HEAD"]).map_err(|_| {
        // Which of the two it is: not a repository, or one without a commit to fork from.
        if git(&["-C", &run.cwd, "rev-parse", "--show-toplevel"]).is_ok() {
            anyhow!("a task worktree needs the run folder's repository to have a commit")
        } else {
            anyhow!("a task worktree needs the run folder to be a git repository")
        }
    })?;
    let mut head = head.lines();
    let top = PathBuf::from(head.next().unwrap_or_default());
    // `HEAD` itself: a detached head, no branch to merge back into.
    let base = head.next().map(str::trim).filter(|b| !b.is_empty() && *b != "HEAD").map(str::to_string);
    let name = top.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "repo".into());
    let short: String = run.id.chars().take(6).collect();
    let leaf = format!("{}-{short}-{}", runs::slug(&run.name), task.id);
    let branch = format!("kelpie/{leaf}");
    let path = top.parent().unwrap_or(&top).join(format!("{name}-worktrees")).join(&leaf);
    let path_s = path.to_string_lossy().into_owned();
    // Git can observe another add's incomplete worktree metadata while checking branches.
    // Coordinate with other daemon runs and the desktop; copies and links stay parallel.
    let worktree_lock = kelpie_proto::worktree_lock::acquire(&top)?;
    git(&["-C", &top.to_string_lossy(), "worktree", "add", "-b", &branch, &path_s, "HEAD"])?;
    drop(worktree_lock);
    let brought = run.materialize.as_ref().map(|m| kelpie_proto::materialize::materialize(top.clone(), path.clone(), m.copy.clone(), m.link.clone()));
    Ok(TaskWorktree { path: path_s, branch, base, brought })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panic after the prompt, or in it, is still an answer to the caller, and the next prompt may come up.
    #[tokio::test]
    async fn a_panic_around_the_prompt_still_answers_and_frees_the_screen() {
        use crate::user_presence::Prompt;
        let (d, dir) = test_daemon("presence-panic");
        d.set_client_role(5, &crate::auth::Role::App);
        let (tx, mut rx) = crate::session::client_channel();
        d.presence.inject(|_| Ok(()));
        d.after_verifying(5, &tx, &json!(1), &json!({}), Prompt::Reset, |_| panic!("the action fell over")).unwrap();
        let o = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("a reply").expect("a reply");
        let first: Value = serde_json::from_slice(&o.data[5..]).unwrap();
        assert_eq!(first["id"], 1);
        assert!(first["error"]["message"].as_str().unwrap().contains("internal error"), "{first}");
        d.presence.inject(|_| panic!("the prompt fell over"));
        d.after_verifying(5, &tx, &json!(2), &json!({}), Prompt::Reset, |_| Ok(json!("never"))).unwrap();
        let o = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.expect("a reply").expect("a reply");
        let second: Value = serde_json::from_slice(&o.data[5..]).unwrap();
        assert_eq!(second["id"], 2);
        assert!(second["error"]["message"].is_string(), "{second}");
        assert!(rx.try_recv().is_err(), "one answer each");
        assert!(d.presence.begin().is_ok(), "the screen is free");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
    }

    fn events(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>) -> Vec<Value> {
        let mut out = Vec::new();
        while let Ok(o) = rx.try_recv() {
            out.push(serde_json::from_slice(&o.data[5..]).unwrap());
        }
        out
    }

    fn event_session(daemon: &Daemon, dir: &std::path::Path, handle: u32, id: &str, restricted: bool) -> Arc<Session> {
        let spec = SessionSpec {
            id: id.into(),
            cwd: dir.to_string_lossy().into_owned(),
            restricted,
            env: BTreeMap::from([("PRIVATE_KEY".into(), format!("{id}-secret"))]),
            ..Default::default()
        };
        let session = Session::new(handle, spec, 80, 24);
        daemon.insert_for_test(Entry::Term(session.clone()));
        session
    }

    #[test]
    fn security_regression_unauthenticated_and_claimed_clients_receive_no_events() {
        use crate::auth::Role;
        let (daemon, dir) = test_daemon("event-unauthenticated");
        event_session(&daemon, &dir, 1, "mine", false);
        let mut receivers = Vec::new();
        for (id, role) in [(1, None), (2, Some(Role::Anonymous)), (3, Some(Role::Claimed("mine".into())))] {
            let (tx, rx) = crate::session::client_channel();
            daemon.register_client(id, tx);
            if let Some(role) = role {
                daemon.set_client_role(id, &role);
            }
            receivers.push(rx);
        }
        for event in ["session.updated", "session.closed", "session.turn", "scheduled.updated", "notify.event", "meta.updated", "run.updated", "run.deleted", "automation.changed", "automation.ran", "mobile.updated", "mobile.pair_request", "checkpoint.updated"] {
            daemon.broadcast(event, json!({"id": "mine", "handle": 1, "session": "mine", "env": {"PRIVATE_KEY": "mine-secret"}}));
        }
        for (index, rx) in receivers.iter_mut().enumerate() {
            assert!(events(rx).is_empty(), "untrusted client {index} received daemon events");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn security_regression_restricted_event_stream_is_own_session_only() {
        use crate::auth::Role;
        let (daemon, dir) = test_daemon("event-restricted");
        let own = event_session(&daemon, &dir, 1, "mine", true);
        let other = event_session(&daemon, &dir, 2, "other", false);
        let (tx, mut rx) = crate::session::client_channel();
        daemon.register_client(1, tx);
        daemon.set_client_role(1, &Role::Session("mine".into()));
        for session in [&own, &other] {
            daemon.broadcast("session.updated", json!(session.info()));
            daemon.broadcast("session.turn", turn_event(&session.info().spec.id, "turn_end", &Value::Null, None));
        }
        let got = events(&mut rx);
        assert_eq!(got.len(), 2, "only the owner's update and turn may be delivered: {got:?}");
        assert_eq!(got[0]["params"]["id"], "mine");
        assert_eq!(got[0]["params"]["env"]["PRIVATE_KEY"], "mine-secret", "the owner's environment remains available");
        assert_eq!(got[1]["params"]["session"], "mine");
        daemon.broadcast("session.closed", json!({"handle": 2}));
        assert!(events(&mut rx).is_empty(), "another session's close is not visible");
        for event in ["notify.event", "scheduled.updated", "run.updated", "run.deleted", "automation.ran", "mobile.pair_request", "checkpoint.updated", "future.private_event"] {
            daemon.broadcast(event, json!({"id": "mine", "session": "mine"}));
        }
        assert!(events(&mut rx).is_empty(), "administrative and unknown events are not a session's scripting surface");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn security_regression_scripting_events_redact_other_session_environment() {
        use crate::auth::Role;
        let (daemon, dir) = test_daemon("event-scripting");
        daemon.agents_control_others.store(true, Ordering::Relaxed);
        let own = event_session(&daemon, &dir, 1, "mine", false);
        let other = event_session(&daemon, &dir, 2, "other", false);
        let (tx, mut rx) = crate::session::client_channel();
        daemon.register_client(1, tx);
        daemon.set_client_role(1, &Role::Session("mine".into()));
        daemon.broadcast("session.updated", json!(own.info()));
        daemon.broadcast("session.updated", json!(other.info()));
        let got = events(&mut rx);
        assert_eq!(got.len(), 2, "scripting clients still see both session updates");
        assert_eq!(got[0]["params"]["env"]["PRIVATE_KEY"], "mine-secret");
        assert_eq!(got[1]["params"]["id"], "other");
        assert!(got[1]["params"].get("env").is_none(), "another session's environment must not enter the outgoing channel: {got:?}");
        daemon.agents_control_others.store(false, Ordering::Relaxed);
        daemon.broadcast("session.updated", json!(other.info()));
        assert!(events(&mut rx).is_empty(), "turning scripting off immediately narrows an existing connection");
        daemon.broadcast("session.updated", json!(own.info()));
        assert_eq!(events(&mut rx).len(), 1, "its own events remain visible");
        daemon.remove_for_test(1);
        daemon.broadcast("session.updated", json!(other.info()));
        assert!(events(&mut rx).is_empty(), "a vanished caller no longer has scripting reach");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn security_regression_trusted_app_keeps_full_event_payloads() {
        use crate::auth::Role;
        let (daemon, dir) = test_daemon("event-user");
        let session = event_session(&daemon, &dir, 1, "mine", false);
        let (tx, mut rx) = crate::session::client_channel();
        daemon.register_client(1, tx);
        daemon.set_client_role(1, &Role::User);
        let payloads = [
            ("session.updated", json!(session.info())),
            ("session.turn", turn_event("mine", "turn_end", &Value::Null, None)),
            ("session.closed", json!({"handle": 1})),
            ("notify.event", json!({"session": "mine", "kind": "waiting"})),
            ("mobile.pair_request", json!({"id": "phone", "name": "test phone"})),
            ("automation.changed", json!({})),
        ];
        for (event, params) in &payloads {
            daemon.broadcast(event, params.clone());
        }
        let got = events(&mut rx);
        assert_eq!(got.len(), payloads.len());
        for (got, (event, params)) in got.iter().zip(&payloads) {
            assert_eq!(got, &json!({"event": event, "params": params}));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn security_regression_session_update_and_close_preserve_owner_delivery() {
        use crate::auth::Role;
        let (daemon, dir) = test_daemon("event-lifecycle");
        event_session(&daemon, &dir, 1, "mine", true);
        event_session(&daemon, &dir, 2, "other", true);
        let mut receivers = Vec::new();
        for (id, role) in [(1, Role::Session("mine".into())), (2, Role::Session("other".into())), (3, Role::User)] {
            let (tx, rx) = crate::session::client_channel();
            daemon.register_client(id, tx);
            daemon.set_client_role(id, &role);
            receivers.push(rx);
        }
        daemon.update_spec(1, &json!({"name": "renamed"})).unwrap();
        for index in [0, 2] {
            let got = events(&mut receivers[index]);
            let updated = got.iter().find(|event| event["event"] == "session.updated").expect("owner and app receive the real spec update");
            assert_eq!(updated["params"]["name"], "renamed");
            assert_eq!(updated["params"]["env"]["PRIVATE_KEY"], "mine-secret");
        }
        assert!(events(&mut receivers[1]).is_empty(), "another restricted owner sees no update");
        daemon.close(1).unwrap();
        assert!(daemon.session_id_of(1).is_none());
        for index in [0, 2] {
            assert_eq!(events(&mut receivers[index]), vec![json!({"event": "session.closed", "params": {"handle": 1}})], "the final event keeps its existing handle-only wire shape after removal");
        }
        assert!(events(&mut receivers[1]).is_empty(), "another restricted owner sees no close");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn what_the_user_looks_at_is_raised_neither_to_the_app_nor_to_the_phone() {
        use notify::Kind;
        let (daemon, dir) = test_daemon("raise");
        let (tx, mut rx) = crate::session::client_channel();
        daemon.register_client(1, tx);
        daemon.set_client_role(1, &crate::auth::Role::User);
        let mut c = notify::Config { min_turn_s: 0, ..Default::default() };
        c.ntfy = notify::Ntfy { enabled: true, server: "http://127.0.0.1:1".into() };
        daemon.notifier.update(Some(c), &Value::Null).unwrap();
        daemon.notifier.set_ui(1, true, vec!["s1".into()], None);
        let of = |kind: Kind, session: &str| Raised { session: Some(session.into()), ..Raised::bare(kind) };

        // A focused window showing s1: its status events go nowhere, whether it needs you, ended or failed.
        for kind in [Kind::Waiting, Kind::Finished, Kind::Error] {
            daemon.raise(of(kind, "s1"), true);
        }
        assert!(events(&mut rx).is_empty(), "the app is not told");
        assert!(!daemon.notifier.started(), "and nothing is sent to the phone");

        // The same events for a session it does not show reach both.
        daemon.raise(of(Kind::Waiting, "s2"), true);
        let got = events(&mut rx);
        assert_eq!((got.len(), got[0]["event"].as_str(), got[0]["params"]["kind"].as_str(), got[0]["params"]["session"].as_str()), (1, Some("notify.event"), Some("waiting"), Some("s2")));
        assert!(daemon.notifier.started(), "the phone gets it");

        // Automations and approvals are not about a session on screen: always told.
        daemon.raise(Raised { session: Some("s1".into()), ..Raised::automation("nightly", None, None) }, true);
        assert_eq!(events(&mut rx).len(), 1);

        // A cost cap crossing is shown by the app itself; the daemon only carries it on.
        daemon.raise(of(Kind::Cost, "s2"), false);
        assert!(events(&mut rx).is_empty(), "reported by the app: not echoed back");
        daemon.unregister_client(1);
        daemon.raise(of(Kind::Finished, "s1"), true);
        assert!(events(&mut rx).is_empty(), "no window: nobody to tell");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_closed_window_stops_counting_as_looking() {
        let (daemon, dir) = test_daemon("looking");
        let (tx, _rx) = crate::session::client_channel();
        daemon.register_client(7, tx);
        daemon.notifier.set_ui(7, true, vec!["s1".into()], None);
        assert!(daemon.notifier.looking_at("s1"));
        daemon.unregister_client(7);
        assert!(!daemon.notifier.looking_at("s1"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_account_switch_changes_only_the_accounts_variable_of_a_stopped_session() {
        let (daemon, dir) = test_daemon("account");
        let session = |kind: &str, remote: bool| {
            let spec = SessionSpec { id: format!("a-{kind}"), kind: kind.into(), cwd: dir.to_string_lossy().into_owned(), remote: remote.then(|| crate::session::RemoteSpec { dest: "h".into(), ..Default::default() }), ..Default::default() };
            let handle = daemon.next_handle.fetch_add(1, Ordering::Relaxed);
            daemon.sessions.write().insert(handle, Entry::Term(Session::new(handle, spec, 80, 24)));
            handle
        };
        let home = dir.to_string_lossy().into_owned();
        let claude = session("claude", false);
        // Only the variable of that agent, only a real folder, never a relative path.
        for bad in [json!({"PATH": home}), json!({"CODEX_HOME": home}), json!({"CLAUDE_CONFIG_DIR": "relative/dir"}), json!({"CLAUDE_CONFIG_DIR": dir.join("missing").to_string_lossy()}), json!({"CLAUDE_CONFIG_DIR": 5}), json!("x")] {
            assert!(daemon.set_env_restart(claude, &bad).is_err(), "{bad}");
        }
        assert!(daemon.set_env_restart(session("shell", false), &json!({})).unwrap_err().to_string().contains("no account"));
        assert!(daemon.set_env_restart(session("claude", true), &json!({})).unwrap_err().to_string().contains("another machine"));
        let spec = match daemon.entry(claude).unwrap() {
            Entry::Term(s) => s.spec.lock().clone(),
            Entry::Chat(_) => unreachable!(),
        };
        assert!(spec.env.is_empty(), "a refused switch changes nothing");
        assert!(daemon.set_env_restart(9999, &json!({})).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A repository with an ignored folder and an ignored file next to a tracked one.
    fn repo_with_ignored(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join("deps/pkg")).unwrap();
        let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(&repo).status().unwrap().success());
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), "deps/\n.env\n").unwrap();
        std::fs::write(repo.join(".env"), "KEY=1").unwrap();
        std::fs::write(repo.join("deps/pkg/a.js"), "1").unwrap();
        std::fs::write(repo.join("main.txt"), "m").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        (base, repo)
    }

    fn worktree_run(repo: &std::path::Path, ids: &[&str], materialize: Option<runs::Materialize>) -> runs::Run {
        let task = |id: &str| runs::NewTask { id: Some(id.into()), title: id.into(), prompt: "p".into(), kind: None, args: vec![], env: BTreeMap::new(), deps: vec![], approve: false, worktree: true, host: None };
        let new = NewRun { name: "Try".into(), objective: String::new(), cwd: repo.to_string_lossy().into_owned(), project_id: None, tasks: ids.iter().map(|i| task(i)).collect(), idle_done_secs: None, materialize };
        runs::build_run(new, "abcdef012345".into(), 0).unwrap()
    }

    #[test]
    fn a_run_worktree_gets_the_approved_links_and_copies_only() {
        let (base, repo) = repo_with_ignored("run-links");
        let approved = runs::Materialize { copy: vec![".env".into(), "main.txt".into()], link: vec!["deps".into(), "../outside".into()] };
        let run = worktree_run(&repo, &["with", "plain"], Some(approved));
        let made = create_task_worktree(&run, &run.tasks[0]).unwrap();
        assert_eq!(made.base.as_deref(), Some("main"), "Finish merges back into the branch the run started on");
        let (wt, brought) = (PathBuf::from(made.path), made.brought);
        assert!(wt.join("deps/pkg/a.js").is_file(), "the approved link resolves to the repository's folder");
        assert_eq!(std::fs::read_to_string(wt.join(".env")).unwrap(), "KEY=1");
        let said = brought.unwrap();
        assert_eq!((said.linked.clone(), said.copied.clone()), (vec!["deps".to_string()], vec![".env".to_string()]));
        let left_out: Vec<_> = said.skipped.iter().map(|s| (s.path.as_str(), s.reason.as_str())).collect();
        assert!(left_out.contains(&("main.txt", "already in the worktree")) && left_out.contains(&("../outside", "not a plain relative path")), "tracked files and escapes are left out: {left_out:?}");

        // Nothing approved (a run made by an agent, or a repository whose file was never approved): a clean checkout.
        let mut plain = worktree_run(&repo, &["plain"], None);
        plain.id = "fedcba543210".into();
        let made = create_task_worktree(&plain, &plain.tasks[0]).unwrap();
        assert!(!PathBuf::from(&made.path).join("deps").exists() && !PathBuf::from(&made.path).join(".env").exists());
        assert!(made.brought.is_none());
        let _ = std::process::Command::new("git").args(["worktree", "prune"]).current_dir(&repo).status();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn five_run_worktrees_with_links_are_ready_within_three_seconds() {
        let (base, repo) = repo_with_ignored("run-five");
        // A folder the size of a real dependency tree is linked, never copied.
        for i in 0..300 {
            std::fs::write(repo.join(format!("deps/pkg/f{i}.js")), "x").unwrap();
        }
        let approved = runs::Materialize { copy: vec![".env".into()], link: vec!["deps".into()] };
        let run = Arc::new(worktree_run(&repo, &["a", "b", "c", "d", "e"], Some(approved)));
        let started = Instant::now();
        let ready = Arc::new(std::sync::Barrier::new(5));
        let jobs: Vec<_> = (0..5)
            .map(|i| {
                let run = run.clone();
                let ready = ready.clone();
                std::thread::spawn(move || {
                    ready.wait();
                    create_task_worktree(&run, &run.tasks[i]).unwrap()
                })
            })
            .collect();
        let made: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
        let took = started.elapsed();
        eprintln!("5 worktrees with links and a copy: {took:?}");
        assert!(made.iter().all(|w| PathBuf::from(&w.path).join("deps/pkg/f299.js").is_file()), "every worktree has the link");
        // The real bound (3 s from the run's creation) is measured end to end; here only that a loaded machine does not hang it.
        assert!(took < Duration::from_secs(30), "{took:?}");
        let _ = std::process::Command::new("git").args(["worktree", "prune"]).current_dir(&repo).status();
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn notes_are_added_listed_and_cleared_through_the_rpcs() {
        let (daemon, dir) = test_daemon("notes");
        let (tx, mut rx) = crate::session::client_channel();
        daemon.register_client(1, tx.clone());
        daemon.set_client_role(1, &crate::auth::Role::User);
        let (base, repo) = repo_with_ignored("notes-repo");
        let wt = repo.to_string_lossy().into_owned();
        let call = |method: &str, params: Value| daemon.dispatch(1, &tx, &json!(1), method, params).map(|v| v.unwrap());
        // The app quotes the line; an agent names it and the diff says the rest.
        std::fs::write(repo.join("main.txt"), "m\nsecond\n").unwrap();
        let by_app = call("note.add", json!({"path": wt, "file": "main.txt", "line": 1, "side": "new", "text": "m", "note": "why", "id": "keep-1", "at": 7})).unwrap();
        assert_eq!((by_app["note"]["id"].as_str(), by_app["note"]["at"].as_u64()), (Some("keep-1"), Some(7)));
        let by_agent = call("note.add", json!({"path": wt, "file": "main.txt", "line": 2, "note": "typo?", "by_session": "gone"})).unwrap();
        assert_eq!((by_agent["note"]["side"].as_str(), by_agent["note"]["text"].as_str(), by_agent["note"]["by"].as_str()), (Some("new"), Some("second"), Some("agent")));
        // Moving the same note over twice keeps one.
        call("note.add", json!({"path": wt, "file": "main.txt", "line": 1, "side": "new", "text": "m", "note": "why", "id": "keep-1"})).unwrap();
        assert_eq!(call("note.list", json!({"path": repo.join("deps").to_string_lossy()})).unwrap().as_array().unwrap().len(), 2, "a subfolder is the same worktree");
        assert!(events(&mut rx).iter().any(|e| e["event"] == "notes.changed" && e["params"]["notes"].as_array().is_some_and(|n| n.len() == 2)));
        assert_eq!(call("note.list", json!({})).unwrap().as_object().unwrap().len(), 1, "without a path, every worktree's");
        assert!(call("note.add", json!({"path": wt, "file": "main.txt", "line": 9, "note": "nowhere"})).is_err(), "no such line");
        let left = call("note.clear", json!({"path": wt, "ids": ["keep-1"]})).unwrap();
        assert_eq!(left["notes"].as_array().unwrap().len(), 1);
        assert!(call("note.clear", json!({"path": wt})).unwrap()["notes"].as_array().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn notes_changed_reaches_the_user_and_the_session_of_that_worktree_only() {
        use crate::auth::Role;
        let (daemon, dir) = test_daemon("notes-audience");
        let (base, repo) = repo_with_ignored("notes-audience-repo");
        let (other_base, other_repo) = repo_with_ignored("notes-audience-other");
        let wt = repo.to_string_lossy().into_owned();
        let session = |id: &str, cwd: &std::path::Path| {
            let spec = SessionSpec { id: id.into(), kind: "claude".into(), cwd: cwd.to_string_lossy().into_owned(), ..Default::default() };
            let handle = daemon.next_handle.fetch_add(1, Ordering::Relaxed);
            daemon.sessions.write().insert(handle, Entry::Term(Session::new(handle, spec, 80, 24)));
        };
        session("mine", &repo);
        session("elsewhere", &other_repo);
        let mut rx = Vec::new();
        for (cid, role) in [(1u64, Some(Role::User)), (2, Some(Role::Session("mine".into()))), (3, Some(Role::Session("elsewhere".into()))), (4, Some(Role::Claimed("mine".into()))), (5, None)] {
            let (tx, r) = crate::session::client_channel();
            daemon.register_client(cid, tx);
            if let Some(role) = role {
                daemon.set_client_role(cid, &role);
            }
            rx.push(r);
        }
        let (tx, _own) = crate::session::client_channel();
        daemon.dispatch(1, &tx, &json!(1), "note.add", json!({"path": wt, "file": "main.txt", "line": 1, "side": "new", "text": "m", "note": "n"})).unwrap();
        let told: Vec<usize> = rx.iter_mut().map(|r| events(r).iter().filter(|e| e["event"] == "notes.changed").count()).collect();
        assert_eq!(told, [1, 1, 0, 0, 0], "the user, the session in that worktree; not another worktree's session, a claimed id or an unauthenticated connection");
        // A session's own request for "every worktree" is refused whatever else it sends.
        assert!(daemon.dispatch(1, &tx, &json!(1), "note.list", json!({"by_session": "mine"})).is_err());
        // Clearing by an exact key never looks upward: a folder inside the repository clears its own notes only.
        let inner = repo.join("deps");
        daemon.dispatch(1, &tx, &json!(1), "note.add", json!({"path": wt, "file": "main.txt", "line": 1, "side": "new", "text": "m", "note": "repo note", "id": "r1"})).unwrap();
        let left = daemon.dispatch(1, &tx, &json!(1), "note.clear", json!({"path": inner.to_string_lossy(), "exact": true})).unwrap().unwrap();
        assert!(left["notes"].as_array().unwrap().is_empty(), "the key of the subfolder holds nothing");
        assert_eq!(daemon.dispatch(1, &tx, &json!(1), "note.list", json!({"path": wt})).unwrap().unwrap().as_array().unwrap().len(), 2, "the repository's notes are still there");
        // Keys: the app asks where each folder's notes are filed.
        let keys = daemon.dispatch(1, &tx, &json!(1), "note.keys", json!({"paths": [repo.join("deps").to_string_lossy(), "C:/gone/folder"]})).unwrap().unwrap();
        assert_eq!(keys[repo.join("deps").to_string_lossy().as_ref()], notes::worktree_key(&wt));
        assert_eq!(keys["C:/gone/folder"], notes::worktree_key("C:/gone/folder"));
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(other_base);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn notes_moved_over_from_the_app_survive_a_daemon_restart_untouched() {
        let (daemon, dir) = test_daemon("notes-migrate");
        let (tx, _rx) = crate::session::client_channel();
        let (base, repo) = repo_with_ignored("notes-migrate-repo");
        let wt = repo.to_string_lossy().into_owned();
        let moved = [("old-1", "main.txt", 1u64, "new", "m", "from the app", 1_700_000_000_000u64), ("old-2", "README.md", 4u64, "old", "removed line", "second", 1_700_000_001_000)];
        // The app sends each note with its own id, time, side and quoted text, and may send the lot twice.
        for _ in 0..2 {
            for (id, file, line, side, text, note, at) in moved {
                daemon.dispatch(1, &tx, &json!(1), "note.add", json!({"path": wt, "file": file, "line": line, "side": side, "text": text, "note": note, "id": id, "at": at})).unwrap();
            }
        }
        let restarted = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        let list = restarted.dispatch(1, &tx, &json!(1), "note.list", json!({"path": wt})).unwrap().unwrap();
        let got: Vec<_> = list.as_array().unwrap().iter().map(|n| (n["id"].as_str().unwrap(), n["path"].as_str().unwrap(), n["line"].as_u64().unwrap(), n["side"].as_str().unwrap(), n["text"].as_str().unwrap(), n["note"].as_str().unwrap(), n["at"].as_u64().unwrap())).collect();
        assert_eq!(got, moved, "every note, once, exactly as the app had it");
        let _ = std::fs::remove_dir_all(base);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_automation_run_delivers_its_folder_to_review_once() {
        let dir = std::env::temp_dir().join(format!("kelpie-deliver-{}", uuid::Uuid::new_v4()));
        let work = dir.join("worktree");
        std::fs::create_dir_all(&work).unwrap();
        let daemon = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        let cwd = work.to_string_lossy().into_owned();
        daemon.set_meta(&cwd, Some("refactor"), Some("progress")).unwrap();
        daemon.deliver_review.lock().insert("s1".into(), cwd.clone());
        daemon.deliver_to_review("other");
        assert_eq!(daemon.workspace_meta.lock().find(&cwd).unwrap().1.status.as_deref(), Some("progress"), "other sessions deliver nothing");
        daemon.deliver_to_review("s1");
        let (_, m) = daemon.workspace_meta.lock().find(&cwd).unwrap();
        assert_eq!((m.status.as_deref(), m.comment.as_deref()), (Some("review"), Some("refactor")), "status moves, the comment stays");
        // The user moves it back: the finished run does not push it into review again.
        daemon.set_meta(&cwd, None, Some("progress")).unwrap();
        daemon.deliver_to_review("s1");
        assert_eq!(daemon.workspace_meta.lock().find(&cwd).unwrap().1.status.as_deref(), Some("progress"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_host_task_whose_session_came_back_interrupted_is_failed_and_released_before_anyone_can_resume_it() {
        let (daemon, dir) = test_daemon("settle-host");
        let spec = SessionSpec { id: "s1".into(), kind: "claude".into(), cwd: "/srv/app".into(), remote: Some(crate::session::RemoteSpec { dest: "devbox".into(), ..Default::default() }), ..Default::default() };
        let session = Session::new(7, spec, 80, 24);
        session.mark_interrupted(5);
        daemon.sessions.write().insert(7, Entry::Term(session));
        let host = runs::TaskHost { dest: "devbox".into(), port: None, config: None, path: "/srv/app".into() };
        let task = |id: &str| runs::NewTask { id: Some(id.into()), title: id.into(), prompt: "p".into(), kind: None, args: vec![], env: BTreeMap::new(), deps: vec![], approve: false, worktree: false, host: Some(host.clone()) };
        let new = NewRun { name: "Hosted".into(), objective: String::new(), cwd: "/srv/app".into(), project_id: None, tasks: vec![task("a")], idle_done_secs: None, materialize: None };
        {
            let mut runs = daemon.runs.lock();
            runs.list.push(runs::build_run(new, "r1".into(), 0).unwrap());
            let _ = runs.step(0, |_| None);
            let (t, _) = runs.task_mut("r1", "a").unwrap();
            t.status = TaskStatus::Running;
            t.session_id = Some("s1".into());
        }
        assert!(daemon.by_id("s1").unwrap().info().interrupted);
        daemon.meta_dirty.store(false, Ordering::Relaxed);
        daemon.settle_host_tasks();
        let info = daemon.by_id("s1").unwrap().info();
        assert!(!info.interrupted, "the app has nothing to resume");
        assert!(daemon.meta_dirty.load(Ordering::Relaxed), "saved as it is now: a second stop does not bring it back");
        assert_eq!(daemon.runs.lock().get("r1").unwrap().tasks[0].status, TaskStatus::Failed);
        // Persisted as not alive: the next daemon would not flag it as interrupted either.
        assert!(!daemon.saved_sessions().iter().any(|s| s.spec.id == "s1" && s.alive));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A session that runs `root` (in a job of its own on Windows).
    fn session_running(daemon: &Arc<Daemon>, id: &str, root: &std::process::Child, job: Option<Arc<crate::proc_tree::ProcTree>>) {
        let spec = SessionSpec { id: id.into(), kind: "shell".into(), cwd: std::env::temp_dir().to_string_lossy().into_owned(), ..Default::default() };
        let handle = daemon.next_handle.fetch_add(1, Ordering::Relaxed);
        let session = Session::new(handle, spec, 80, 24);
        session.fake_process(root.id(), job);
        daemon.sessions.write().insert(handle, Entry::Term(session));
    }

    /// The reply to request `id` (stopping answers from a thread of its own).
    fn reply_to(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>, id: u64) -> Value {
        let end = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(r) = events(rx).into_iter().find(|e| e["id"] == id) {
                return r;
            }
            assert!(Instant::now() < end, "no reply to {id}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn processes_are_listed_and_stopped_only_when_they_are_the_sessions_and_the_same_process() {
        use crate::procs::tests::{alive, reap, session};
        let (daemon, dir) = test_daemon("process-rpc");
        let (tx, mut rx) = crate::session::client_channel();
        daemon.register_client(1, tx.clone());
        daemon.set_client_role(1, &crate::auth::Role::User);
        let (root, target) = session("s1");
        session_running(&daemon, "s1", &root, target.tree.clone());
        let stranger = crate::procs::tests::sleeper();
        let call = |id: u64, method: &str, params: Value| {
            daemon.dispatch(1, &tx, &json!(id), method, params).unwrap();
        };
        // The tree: the launcher and, once it has started it, a program, each with its identity.
        let end = Instant::now() + Duration::from_secs(15);
        let listed = loop {
            let a = daemon.dispatch(1, &tx, &json!(1), "process.list", json!({"session": "s1"})).unwrap().unwrap();
            if !a["children"].as_array().unwrap().is_empty() || Instant::now() > end {
                break a;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        assert_eq!(listed["root"]["pid"], root.id());
        let child = listed["children"][0].clone();
        assert!(child["start_time"].as_u64().unwrap() > 0 && child["exe"].is_string(), "{child}");
        assert!(listed["keep"].as_array().unwrap().iter().any(|p| *p == root.id()));
        // The chips' call names the session and counts what it started.
        let chips = daemon.dispatch(1, &tx, &json!(2), "process.ports", json!({"sessions": ["s1", "nope"]})).unwrap().unwrap();
        assert!(chips["sessions"]["s1"]["children"].as_u64().unwrap() >= 1 && chips["sessions"].get("nope").is_none());
        assert!(daemon.dispatch(1, &tx, &json!(3), "daemon.info", json!({})).unwrap().unwrap()["process_scans"].as_u64().unwrap() >= 2);
        // A wrong start time, another program, a stranger's PID, the session's own process: all refused, nothing ends.
        let kill = |pid: u64, start: u64, exe: Value| json!({"session": "s1", "pid": pid, "start_time": start, "exe": exe});
        let (cpid, cstart) = (child["pid"].as_u64().unwrap(), child["start_time"].as_u64().unwrap());
        let (sid, sstart) = {
            let id = crate::procs::Procs::default().describe(stranger.id()).expect("the stranger runs").0;
            (id.pid as u64, id.start_time)
        };
        for (n, params) in [
            kill(cpid, cstart + 1, Value::Null),
            kill(cpid, cstart, json!("C:/elsewhere/other.exe")),
            kill(sid, sstart, Value::Null),
            kill(root.id() as u64, cstart, Value::Null),
        ]
        .into_iter()
        .enumerate()
        {
            call(100 + n as u64, "process.kill", params);
            let r = reply_to(&mut rx, 100 + n as u64);
            assert!(r["error"]["message"].is_string(), "{n}: {r}");
        }
        assert!(alive(cpid as u32) && alive(stranger.id()) && alive(root.id()), "nothing was stopped");
        // The listed process, with its identity, is.
        call(200, "process.kill", kill(cpid, cstart, child["exe"].clone()));
        let done = reply_to(&mut rx, 200);
        assert!(done["error"].is_null() && done["result"]["refused"].as_array().unwrap().is_empty(), "{done}");
        let end = Instant::now() + Duration::from_secs(5);
        while alive(cpid as u32) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(cpid as u32));
        assert!(alive(stranger.id()), "an unrelated process is never stopped");
        // Stopping the children asks for nothing but the session.
        call(300, "process.kill_children", json!({"session": "s1"}));
        assert!(reply_to(&mut rx, 300)["error"].is_null());
        assert!(daemon.dispatch(1, &tx, &json!(4), "process.list", json!({"session": "gone"})).is_err());
        reap(stranger);
        reap(root);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// A custom session whose own process is the server (`python -m http.server` run as the session) has a chip.
    #[test]
    fn a_session_whose_own_process_listens_has_its_port_in_the_chips() {
        let (daemon, dir) = test_daemon("process-root-port");
        let (tx, _rx) = crate::session::client_channel();
        daemon.register_client(1, tx.clone());
        daemon.set_client_role(1, &crate::auth::Role::User);
        // The test run itself stands for the server: it is the session's process and it listens.
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let spec = SessionSpec { id: "srv".into(), kind: "custom".into(), cwd: std::env::temp_dir().to_string_lossy().into_owned(), ..Default::default() };
        let handle = daemon.next_handle.fetch_add(1, Ordering::Relaxed);
        let session = Session::new(handle, spec, 80, 24);
        session.fake_process(std::process::id(), None);
        daemon.sessions.write().insert(handle, Entry::Term(session));
        let chips = daemon.dispatch(1, &tx, &json!(1), "process.ports", json!({"sessions": ["srv"]})).unwrap().unwrap();
        let ports = chips["sessions"]["srv"]["ports"].as_array().unwrap();
        assert!(ports.iter().any(|p| p["port"] == port && p["pid"] == std::process::id()), "{chips}");
        drop(socket);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn programs_in_a_session_cannot_list_or_stop_processes_whatever_they_send() {
        use crate::auth::{authorize, Role};
        let (daemon, dir) = test_daemon("process-auth");
        let spec = SessionSpec { id: "s1".into(), kind: "shell".into(), cwd: dir.to_string_lossy().into_owned(), ..Default::default() };
        let handle = daemon.next_handle.fetch_add(1, Ordering::Relaxed);
        daemon.sessions.write().insert(handle, Entry::Term(Session::new(handle, spec, 80, 24)));
        let shapes = [
            Value::Null,
            json!({}),
            json!([]),
            json!(["s1", 1, 1, null]),
            json!("process.kill"),
            json!(7),
            json!(true),
            json!({"session": "s1", "pid": 1, "start_time": 1}),
            json!({"session": "other", "pid": 1, "start_time": 1, "exe": "x"}),
            json!({"sessions": ["s1"]}),
            json!({"port": 80}),
        ];
        let methods = ["process.list", "process.ports", "process.port_owner", "process.kill", "process.kill_children", "process.anything_else"];
        for scripting in [true, false] {
            daemon.agents_control_others.store(scripting, Ordering::Relaxed);
            for role in [Role::Session("s1".into()), Role::Claimed("s1".into()), Role::Anonymous] {
                for method in methods {
                    for shape in &shapes {
                        let mut params = shape.clone();
                        let err = authorize(&daemon, &role, method, &mut params).expect_err(&format!("{role:?} {method} {shape}"));
                        assert!(!err.to_string().is_empty());
                    }
                }
            }
        }
        // The user's own connection may.
        for method in methods.iter().take(5) {
            authorize(&daemon, &Role::User, method, &mut json!({})).unwrap();
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
