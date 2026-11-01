//! One terminal session: PTY + headless VT state + subscribers.
//!
//! Threading model (per session):
//! - reader thread: blocking PTY read → [`Session::ingest`] (parse, ring, fan-out). Never
//!   blocks on consumers: a subscriber that falls behind its byte budget is marked stalled and
//!   told to resync, the PTY keeps draining.
//! - writer thread: owns the PTY master; serialises input writes and resizes so a large paste
//!   can never deadlock against the reader (ConPTY requires separate read/write threads).
//! - waiter thread: blocks on child exit, then closes the PTY.

use std::collections::{BTreeMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use base64::Engine as _;
use kelpie_proto::Frame;
use parking_lot::Mutex;
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::watch;

use crate::agents::{Launch, TypedPrompt};
use crate::proc_tree::{console_hosts, Priority, ProcTree};
use crate::status::{exit_failed, Activity, Status, MIN_WORK_FOR_UNREAD};
use crate::vt::{Vt, VtEvent};

/// Raw output kept for cheap, exact replays.
pub const RING_CAP: usize = 1 << 20;
/// Above this many bytes behind, a snapshot is cheaper than a replay.
pub const REPLAY_LIMIT: u64 = 256 * 1024;
/// Unsent bytes allowed per subscriber before it must resync.
pub const SUB_BUDGET: usize = 4 << 20;
pub const SCROLLBACK: usize = 5000;

/// Held while a pseudoconsole is created, so its console host can be told apart (`spawn`).
static CONSOLE_SPAWN: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
pub const SNAPSHOT_HISTORY: usize = 2000;


#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionSpec {
    pub id: String,
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// shell | claude | codex | omp | custom
    pub kind: String,
    pub cwd: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Agent-side conversation id (Claude session id, Codex thread id…) used for resume.
    #[serde(default)]
    pub agent_session_id: Option<String>,
    /// Model the agent runs (from `--model`, hooks or the chat protocol), for display.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub created_at: u64,
    /// Runs on another machine over SSH (`cwd` is a path there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<RemoteSpec>,
    /// Its programs may only act on this session through `kelpied`, whatever the user's
    /// scripting setting (see `auth`).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub restricted: bool,
}

/// Where a remote session runs: an ssh destination (`user@host` or a `~/.ssh/config` alias).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RemoteSpec {
    pub dest: String,
    #[serde(default)]
    pub port: Option<u16>,
    /// `ssh -F` config file.
    #[serde(default)]
    pub config: Option<String>,
    /// Run inside the host's tmux (when installed), so the program outlives the connection.
    #[serde(default)]
    pub tmux: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionInfo {
    pub handle: u32,
    #[serde(flatten)]
    pub spec: SessionSpec,
    pub title: Option<String>,
    pub status: Status,
    pub unread: bool,
    pub alive: bool,
    pub exit_code: Option<i32>,
    /// The last agent turn errored, or the process exited with a failure code on its own.
    pub failed: bool,
    /// The last turn ended on the subscription's usage limit (Claude Code reports it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub limited: bool,
    pub pid: Option<u32>,
    pub cols: u16,
    pub rows: u16,
    pub seq: u64,
    /// Not a terminal session: a structured chat process (see `chat.rs`).
    pub chat: bool,
    /// Wall-clock ms of the last output or input (0 = never). Not a broadcast trigger.
    pub last_activity: u64,
    /// Was running when the daemon stopped (reboot, update, crash) and has not been restarted.
    pub interrupted: bool,
    /// Wall-clock ms since it has been waiting on the user (`status == waiting`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waiting_since: Option<u64>,
    /// A chat's next message waits for the desktop window that queued it (one only that window can build, such as a picture),
    /// and everything behind it waits too. The app says so outside the chat pane, which may be closed.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub waits_for_window: bool,
}

/// How long after typed text its Enter follows (the desktop waits the same: `typeInto`).
pub const ENTER_DELAY: Duration = Duration::from_millis(250);

/// Text typed into a session, to decide later whether an Enter still belongs to it.
#[derive(Debug, Clone, Copy)]
pub struct Typed {
    written: Option<Instant>,
    wait: Option<u64>,
}

/// What answers a prompt: a named key (`esc`, `enter`) or text, which `typed_text` cleans.
#[derive(Debug, Clone, Copy)]
pub enum Answer<'a> {
    Key(&'a str),
    Text(&'a str),
}

/// Why a typed answer or message was not written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnswerError {
    UnknownKey,
    /// The prompt is gone, another one, or answered by someone not remembered (`not_waiting`,
    /// `moved`, `answered`).
    Stale(&'static str),
    /// That wait was answered, by this one.
    Answered(Resolution),
    /// A prompt is shown and the text was not forced.
    Prompt,
}

/// Why a guarded write was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Exited,
    Restarted,
    Conversation,
    NotLimited,
    InputSince,
}

impl Refused {
    pub fn code(self) -> &'static str {
        match self {
            Refused::Exited => "exited",
            Refused::Restarted => "restarted",
            Refused::Conversation => "conversation",
            Refused::NotLimited => "not_limited",
            Refused::InputSince => "input",
        }
    }
}

/// One queued write to a client socket. `credit` is decremented once written.
pub struct Outgoing {
    pub data: Arc<[u8]>,
    pub credit: Option<Arc<AtomicUsize>>,
}

/// The end of a session's screen as a remote view shows it (see `Session::screen_view`).
pub struct ScreenView {
    pub lines: Vec<String>,
    pub cursor: Option<(usize, usize)>,
    pub prompt: Option<crate::choices::Prompt>,
    pub waiting_since: Option<u64>,
    pub title: Option<String>,
    /// While a prompt or a wait is up: what a card says about it.
    pub wait: Option<crate::mobile::risk::WaitInfo>,
}

/// Requests a wait is said to be about at once, far beyond what an agent asks in parallel. Past this the mildest goes (`risk::mildest`).
const WAIT_REQUESTS_KEPT: usize = 16;

/// Where a client's messages from the daemon wait. The app and the CLI have an unbounded queue; a
/// phone has a bounded one (`bounded_client_channel`): when it is full the message is dropped and
/// the queue says so (`overflowed`), so a phone that cannot keep up is told to resync instead of the
/// daemon holding a backlog for it.
#[derive(Clone)]
pub enum ClientTx {
    Unbounded(tokio::sync::mpsc::UnboundedSender<Outgoing>),
    Bounded { tx: tokio::sync::mpsc::Sender<Outgoing>, overflowed: Arc<std::sync::atomic::AtomicBool> },
}

/// The client is gone.
#[derive(Debug)]
pub struct Closed;

impl ClientTx {
    pub fn send(&self, message: Outgoing) -> Result<(), Closed> {
        match self {
            ClientTx::Unbounded(tx) => tx.send(message).map_err(|_| Closed),
            ClientTx::Bounded { tx, overflowed } => match tx.try_send(message) {
                Ok(()) => Ok(()),
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => Err(Closed),
                Err(tokio::sync::mpsc::error::TrySendError::Full(dropped)) => {
                    // What was counted for it will never be taken.
                    if let Some(credit) = dropped.credit {
                        credit.fetch_sub(dropped.data.len(), Ordering::Relaxed);
                    }
                    overflowed.store(true, Ordering::Relaxed);
                    Ok(())
                }
            },
        }
    }
}

/// A queue without a limit, for the app and the CLI.
pub fn client_channel() -> (ClientTx, tokio::sync::mpsc::UnboundedReceiver<Outgoing>) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (ClientTx::Unbounded(tx), rx)
}

/// A queue of at most `cap` messages, and the flag that says one was dropped because it was full.
pub fn bounded_client_channel(cap: usize) -> (ClientTx, tokio::sync::mpsc::Receiver<Outgoing>, Arc<std::sync::atomic::AtomicBool>) {
    let (tx, rx) = tokio::sync::mpsc::channel(cap);
    let overflowed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    (ClientTx::Bounded { tx, overflowed: overflowed.clone() }, rx, overflowed)
}

/// Who answered a wait: the desktop (`id` "desktop"), or a paired phone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct By {
    pub id: String,
    pub name: String,
}

impl By {
    pub fn desktop() -> By {
        By { id: "desktop".into(), name: "desktop".into() }
    }

    pub fn is_desktop(&self) -> bool {
        self.id == "desktop"
    }
}

/// An answered wait: by whom and when (ms since the epoch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    pub by: By,
    pub at: u64,
}

/// Waits whose answer is remembered per session, and for how long: a tap from a push that arrives
/// late is told who answered it and when.
const RESOLVED_KEPT: usize = 20;
const RESOLVED_TTL_MS: u64 = 10 * 60 * 1000;

pub fn json_out(value: &Value) -> Outgoing {
    Outgoing { data: Frame::json(value).encode().into(), credit: None }
}

struct Sub {
    client: u64,
    tx: ClientTx,
    credit: Arc<AtomicUsize>,
    interactive: bool,
    stalled: bool,
}

pub(crate) enum Ctl {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct Pty {
    ctl: std::sync::mpsc::Sender<Ctl>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    tree: Option<Arc<ProcTree>>,
    pid: Option<u32>,
    /// When that process started (`procs::start_time_of`), read right after it was started.
    start: Option<u64>,
}

struct Ring {
    buf: VecDeque<u8>,
}

impl Ring {
    fn push(&mut self, bytes: &[u8]) {
        if bytes.len() >= RING_CAP {
            self.buf.clear();
            self.buf.extend(&bytes[bytes.len() - RING_CAP..]);
            return;
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(RING_CAP);
        self.buf.drain(..overflow);
        self.buf.extend(bytes);
    }

    /// Bytes after stream offset `from`, given the stream is currently at `seq`.
    fn since(&self, from: u64, seq: u64) -> Option<Vec<u8>> {
        let start = seq - self.buf.len() as u64;
        if from < start || from > seq {
            return None;
        }
        let skip = (from - start) as usize;
        Some(self.buf.iter().skip(skip).copied().collect())
    }
}

struct Inner {
    vt: Vt,
    ring: Ring,
    seq: u64,
    /// Raw replay from before this offset would be rendered at the wrong size.
    replay_floor: u64,
    subs: Vec<Sub>,
    activity: Activity,
    focused: bool,
    unread: bool,
    last_status: Status,
    exit_code: Option<i32>,
    /// Kelpie stopped the process, so its exit code says nothing about failure.
    stopped: bool,
    pty: Option<Pty>,
    generation: u64,
    persisted_seq: u64,
    last_broadcast: Option<SessionInfo>,
    working_since: Option<Instant>,
    /// Wall-clock ms the current wait on the user began.
    waiting_since: Option<u64>,
    /// The prompt found on the screen, and the output position it was looked for at.
    screen_prompt: Option<crate::choices::Prompt>,
    prompt_checked_seq: u64,
    /// The screen up to the prompt found (`choices::detect_with_screen`), and the one the user
    /// last answered: the same screen found again is that prompt still showing, not a new one.
    prompt_fingerprint: Option<String>,
    answered_prompt: Option<String>,
    last_activity: u64,
    /// `last_activity` as of the last broadcast; output alone re-broadcasts at most every 30 s.
    broadcast_activity: u64,
    /// A first prompt typed once the program has started and gone quiet (agents that take no
    /// prompt argument, automations, hand-offs).
    pending_prompt: Option<PendingPrompt>,
    last_output: Option<Instant>,
    interrupted: bool,
    /// The wait on the user that `waiting_since` names has been answered (typed into): a hook
    /// keeps saying `waiting` until its next report, and nothing may answer the same wait twice.
    answered_wait: bool,
    /// The requests the agent's hooks say are open, oldest first (`request_hook`).
    wait_requests: Vec<crate::mobile::risk::Request>,
    /// Counts what was typed into the terminal, and what the count was when the list above last changed by a request coming or going (`typing_may_approve_danger`).
    typed_count: u64,
    typed_when_requests_changed: u64,
    /// The last waits answered, newest last (see `RESOLVED_KEPT`), by their `waiting_since`.
    resolved: VecDeque<(u64, Resolution)>,
    /// Who answered the wait that is open now (kept as long as it is, past the ring's ten minutes).
    answered_now: Option<Resolution>,
    /// Who the next write answers for (set by the answering paths just before they write).
    typing_by: Option<By>,
}

struct PendingPrompt {
    prompt: TypedPrompt,
    since: Instant,
    /// Text sent, Enter still to send (a separate write: TUIs treat a fast Enter as pasted text).
    pasted: bool,
}

/// Wait at least this long after spawn, and for this much quiet, before typing a prompt.
const PROMPT_MIN_WAIT: Duration = Duration::from_millis(2500);
const PROMPT_QUIET: Duration = Duration::from_millis(1200);
const PROMPT_MAX_WAIT: Duration = Duration::from_secs(30);
/// After pasting a prompt, this much quiet (the paste drawn) before Enter; at most the max.
const PASTE_SETTLE: Duration = Duration::from_millis(300);
/// A screen must have been quiet this long before it is read for a prompt waiting on the user,
/// and this many of its last lines are read.
const PROMPT_LOOK_QUIET: Duration = Duration::from_millis(700);
const PROMPT_LOOK_LINES: usize = 14;
const PASTE_MAX_WAIT: Duration = Duration::from_secs(3);

pub struct Session {
    pub handle: u32,
    pub spec: Mutex<SessionSpec>,
    inner: Mutex<Inner>,
    /// Sent (the output position) whenever what a remote view shows may have changed; the
    /// phone's sync waits on it instead of polling.
    wake: watch::Sender<u64>,
}

/// How stale a broadcast `last_activity` may get while a session keeps printing.
pub const ACTIVITY_REBROADCAST_MS: u64 = 30_000;

pub fn now_millis() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Session {
    pub fn new(handle: u32, spec: SessionSpec, cols: u16, rows: u16) -> Arc<Session> {
        Arc::new(Session {
            handle,
            spec: Mutex::new(spec),
            wake: watch::channel(0).0,
            inner: Mutex::new(Inner {
                vt: Vt::new(cols, rows, SCROLLBACK),
                ring: Ring { buf: VecDeque::with_capacity(64 * 1024) },
                seq: 0,
                replay_floor: 0,
                subs: Vec::new(),
                activity: Activity { alive: false, ..Default::default() },
                focused: false,
                unread: false,
                last_status: Status::Exited,
                exit_code: None,
                stopped: false,
                pty: None,
                generation: 0,
                persisted_seq: 0,
                last_broadcast: None,
                working_since: None,
                waiting_since: None,
                screen_prompt: None,
                prompt_checked_seq: 0,
                prompt_fingerprint: None,
                answered_prompt: None,
                last_activity: 0,
                broadcast_activity: 0,
                pending_prompt: None,
                last_output: None,
                interrupted: false,
                answered_wait: false,
                wait_requests: Vec::new(),
                typed_count: 0,
                typed_when_requests_changed: 0,
                resolved: VecDeque::new(),
                answered_now: None,
                typing_by: None,
            }),
        })
    }

    pub fn spawn(self: &Arc<Self>, launch: &Launch) -> Result<()> {
        let spec = self.spec.lock().clone();
        let (cols, rows) = self.inner.lock().vt.size();
        // The pseudoconsole starts its console host right away; the one that was not there
        // before is this session's (serialised, so two spawns cannot swap them).
        let (pair, console) = {
            let _one_at_a_time = CONSOLE_SPAWN.lock();
            let before = console_hosts();
            let pair = native_pty_system()
                .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
                .context("open pty")?;
            (pair, console_hosts().into_iter().find(|p| !before.contains(p)))
        };
        let mut cmd = CommandBuilder::new(&launch.argv[0]);
        cmd.args(&launch.argv[1..]);
        if std::path::Path::new(&spec.cwd).is_dir() {
            cmd.cwd(&spec.cwd);
        }
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("TERM_PROGRAM", "Kelpie");
        cmd.env("KELPIE_SESSION_ID", &spec.id);
        cmd.env(kelpie_proto::SESSION_TOKEN_ENV, crate::auth::session_token(&spec.id));
        cmd.env(kelpie_proto::SESSION_ENDPOINT_ENV, kelpie_proto::endpoint_key(&kelpie_proto::endpoint()));
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let mut child = pair.slave.spawn_command(cmd).with_context(|| format!("spawn {:?}", launch.argv))?;
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let master = pair.master;
        let pid = child.process_id();
        // Read while the child is ours and unreaped: this is the start the session's process is known by.
        let start = pid.and_then(crate::procs::start_time_of);
        let agent = crate::agents::preset_for(&spec.kind).map(|p| p.bin);
        let tree = pid.and_then(|p| ProcTree::adopt(p, console, agent).map(Arc::new).map_err(|e| tracing::warn!("job object: {e}")).ok());
        let quiet = tree.as_ref().map(|t| t.quiet_flag());
        let killer = child.clone_killer();
        let (ctl_tx, ctl_rx) = std::sync::mpsc::channel::<Ctl>();

        let generation = {
            let mut g = self.inner.lock();
            if g.pty.is_some() {
                // Lost a race with another spawn/restart: never run two processes in one session.
                drop(g);
                let _ = child.kill();
                anyhow::bail!("session is already running");
            }
            g.generation += 1;
            g.exit_code = None;
            g.stopped = false;
            g.interrupted = false;
            g.activity = Activity { alive: true, ..Default::default() };
            g.last_status = Status::Idle;
            g.pty = Some(Pty { ctl: ctl_tx.clone(), killer, tree, pid, start });
            g.generation
        };

        let name = format!("kelpie-pty-{}", self.handle);
        std::thread::Builder::new().name(format!("{name}-w")).spawn(move || {
            while let Ok(msg) = ctl_rx.recv() {
                match msg {
                    Ctl::Input(bytes) => {
                        if writer.write_all(&bytes).and_then(|_| writer.flush()).is_err() {
                            break;
                        }
                    }
                    Ctl::Resize(cols, rows) => {
                        let _ = master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
                    }
                    Ctl::Close => break,
                }
            }
            // Dropping the master closes the pseudoconsole, which unblocks the reader.
            drop(writer);
            drop(master);
        })?;

        let me = Arc::clone(self);
        std::thread::Builder::new().name(format!("{name}-r")).spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            let mut quiet_now = false;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        // A session out of sight parses its output at the priority of other
                        // programs, not ahead of them (the daemon itself runs above normal).
                        if let Some(q) = quiet.as_ref().map(|q| q.load(std::sync::atomic::Ordering::Relaxed)).filter(|&q| q != quiet_now) {
                            crate::proc_tree::set_current_thread_quiet(q);
                            quiet_now = q;
                        }
                        me.ingest(&buf[..n]);
                    }
                }
            }
        })?;

        let me = Arc::clone(self);
        std::thread::Builder::new().name(format!("{name}-x")).spawn(move || {
            let code = child.wait().map(|s| s.exit_code() as i32).unwrap_or(-1);
            let _ = ctl_tx.send(Ctl::Close);
            me.on_exit(generation, code);
        })?;
        if let Some(prompt) = &launch.typed_prompt {
            self.queue_prompt(prompt.clone());
        }
        Ok(())
    }

    fn on_exit(&self, generation: u64, code: i32) {
        {
            let mut g = self.inner.lock();
            if g.generation != generation {
                return;
            }
            // Dropping the job kills anything the process left running in its tree.
            g.pty = None;
            g.exit_code = Some(code);
            g.activity.alive = false;
        }
        // Give the reader a moment to deliver the final output before the marker.
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.ingest(format!("\r\n\x1b[0;2m[process exited · code {code}]\x1b[0m\r\n").as_bytes());
    }

    /// Parse in order, remember, fan out. Hot path: called for every PTY chunk.
    pub fn ingest(&self, bytes: &[u8]) {
        let now = Instant::now();
        let mut g = self.inner.lock();
        let events = g.vt.feed(bytes);
        g.seq += bytes.len() as u64;
        g.ring.push(bytes);
        self.wake.send_replace(g.seq);
        g.activity.on_output(now);
        g.last_output = Some(now);
        g.last_activity = now_millis();
        let handle = self.handle;
        if !g.subs.is_empty() {
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
        if events.is_empty() {
            return;
        }
        let interactive = g.subs.iter().any(|s| s.interactive && !s.stalled);
        for ev in events {
            match ev {
                // An attached xterm answers queries itself; answering twice corrupts input.
                VtEvent::Reply(reply) if !interactive => {
                    if let Some(pty) = &g.pty {
                        let _ = pty.ctl.send(Ctl::Input(reply.into_bytes()));
                    }
                }
                VtEvent::Bell => g.activity.on_bell(now),
                VtEvent::Clipboard(text) => {
                    let msg = json!({"event": "session.clipboard", "params": {"handle": handle, "text": text}});
                    for s in g.subs.iter().filter(|s| s.interactive) {
                        let _ = s.tx.send(json_out(&msg));
                    }
                }
                _ => {}
            }
        }
    }

    /// Subscribes `client` and sends the attach reply (inline, under the lock, so no output
    /// frame can slip between the reply's `seq` and the live stream).
    pub fn attach(&self, client: u64, tx: &ClientTx, req_id: &Value, since: Option<u64>, interactive: bool) {
        let mut g = self.inner.lock();
        g.subs.retain(|s| s.client != client);
        let seq = g.seq;
        let delta = since
            .filter(|&s| s >= g.replay_floor && seq.saturating_sub(s) <= REPLAY_LIMIT)
            .and_then(|s| g.ring.since(s, seq));
        let (mode, raw, data) = if let Some(bytes) = delta {
            ("delta", true, bytes)
        } else if g.replay_floor == 0 && seq <= REPLAY_LIMIT && g.ring.buf.len() as u64 == seq {
            ("reset", true, g.ring.buf.iter().copied().collect())
        } else {
            ("reset", false, g.vt.snapshot(SNAPSHOT_HISTORY))
        };
        let (cols, rows) = g.vt.size();
        let reply = json!({"id": req_id, "result": {
            "mode": mode, "raw": raw, "seq": seq, "cols": cols, "rows": rows,
            "data": base64::engine::general_purpose::STANDARD.encode(&data),
        }});
        let _ = tx.send(json_out(&reply));
        g.subs.push(Sub { client, tx: tx.clone(), credit: Arc::new(AtomicUsize::new(0)), interactive, stalled: false });
        if interactive {
            g.focused = true;
            g.unread = false;
        }
    }

    pub fn detach(&self, client: u64) {
        let mut g = self.inner.lock();
        g.subs.retain(|s| s.client != client);
        if !g.subs.iter().any(|s| s.interactive) {
            g.focused = false;
        }
    }

    pub fn input(&self, bytes: Vec<u8>) {
        self.input_by(bytes, By::desktop());
    }

    /// [`Self::input`] for `by`: if the session waits on the user, this is whose answer it is.
    pub fn input_by(&self, bytes: Vec<u8>, by: By) {
        let mut g = self.inner.lock();
        g.typing_by = Some(by);
        Self::write_input(&mut g, bytes);
        g.typing_by = None;
    }

    fn write_input(g: &mut Inner, bytes: Vec<u8>) {
        // Focus reports (`ESC [ I` / `ESC [ O`) are not the user answering anything: a prompt keeps
        // waiting while they look elsewhere. Mouse reports and query replies are not typing either,
        // for the usage-limit timer; a click can still answer a prompt.
        if !is_terminal_report(&bytes) {
            let now = Instant::now();
            g.typed_count += 1;
            g.activity.last_typed = Some(now);
            // Keys that answer a permission prompt or a question go to that dialog, not to the prompt line.
            if !matches!(g.activity.hook, Some((Status::Waiting, _))) {
                g.activity.last_draft = Some(now);
            }
        }
        if !is_focus_report(&bytes) {
            if g.screen_prompt.is_some() {
                g.answered_prompt = g.prompt_fingerprint.clone();
            }
            // Typing answers a wait the agent reported (a hook outlives the answer); the
            // terminal's own mouse and query reports do not.
            if !is_terminal_report(&bytes) && g.activity.status(Instant::now()) == Status::Waiting {
                // Only the first write answers the wait: whoever types into it later is not who answered.
                let first = !g.answered_wait;
                g.answered_wait = true;
                let by = g.typing_by.take().unwrap_or_else(By::desktop);
                if first {
                    Self::note_resolved(g, by);
                }
            }
            g.typing_by = None;
            g.activity.last_input = Some(Instant::now());
            g.last_activity = now_millis();
            g.activity.bell = None;
        }
        if let Some(pty) = &g.pty {
            let _ = pty.ctl.send(Ctl::Input(bytes));
        }
    }

    /// What was last typed and the wait it was typed into, for an Enter that follows.
    fn typed_marker(g: &Inner) -> Typed {
        Typed { written: g.activity.last_typed, wait: g.waiting_since }
    }

    /// Types `bytes` only while the session still waits on the prompt that began at
    /// `waiting_since` and nobody has answered that wait — checked and written under one lock, so
    /// nothing can answer it in between. Refused with why: not waiting any more, a different
    /// prompt, or already answered.
    fn answer_bytes(&self, waiting_since: Option<u64>, bytes: Vec<u8>, by: By) -> Result<Typed, AnswerError> {
        let mut g = self.inner.lock();
        // A wait answered before says who answered it, whatever the session is doing now.
        let now = now_millis();
        g.resolved.retain(|(_, r)| now.saturating_sub(r.at) < RESOLVED_TTL_MS);
        if let Some((_, resolution)) = waiting_since.and_then(|w| g.resolved.iter().rev().find(|(since, _)| *since == w)) {
            return Err(AnswerError::Answered(resolution.clone()));
        }
        if g.activity.status(Instant::now()) != Status::Waiting || g.waiting_since.is_none() {
            return Err(AnswerError::Stale("not_waiting"));
        }
        if g.waiting_since != waiting_since {
            return Err(AnswerError::Stale("moved"));
        }
        if g.answered_wait {
            // Past the ring's ten minutes the open wait still knows who answered it.
            return Err(g.answered_now.clone().map_or(AnswerError::Stale("answered"), AnswerError::Answered));
        }
        g.typing_by = Some(by);
        Self::write_input(&mut g, bytes);
        Ok(Self::typed_marker(&g))
    }

    /// Remembers that the current wait was answered by `by`.
    fn note_resolved(g: &mut Inner, by: By) {
        let Some(since) = g.waiting_since else { return };
        let at = now_millis();
        g.resolved.retain(|(_, r)| at.saturating_sub(r.at) < RESOLVED_TTL_MS);
        if g.resolved.iter().all(|(s, _)| *s != since) {
            g.resolved.push_back((since, Resolution { by: by.clone(), at }));
            g.answered_now = Some(Resolution { by, at });
        }
        while g.resolved.len() > RESOLVED_KEPT {
            g.resolved.pop_front();
        }
    }

    /// The one way a person answers the prompt a session waits on from a surface that is not its
    /// terminal (the desktop's quick answer, the phone): `answer` is typed only while the session
    /// still waits on the prompt named by `waiting_since`, then `enter` follows after the agent's
    /// Enter delay (see [`Self::enter_if`]).
    pub fn answer(self: &Arc<Self>, waiting_since: Option<u64>, answer: Answer<'_>, enter: bool) -> Result<(), AnswerError> {
        self.answer_by(waiting_since, answer, enter, By::desktop())
    }

    /// [`Self::answer`] on behalf of `by`, who is told apart from whoever answers next.
    pub fn answer_by(self: &Arc<Self>, waiting_since: Option<u64>, answer: Answer<'_>, enter: bool, by: By) -> Result<(), AnswerError> {
        let bytes = match answer {
            Answer::Key("esc") => b"\x1b".to_vec(),
            Answer::Key("enter") => b"\r".to_vec(),
            Answer::Key(_) => return Err(AnswerError::UnknownKey),
            Answer::Text(text) => typed_text(text).into_bytes(),
        };
        let typed = self.answer_bytes(waiting_since, bytes, by)?;
        // An answer leaves the open requests as they are. It does not say which it answered, and it may be for a dialog no hook spoke of: taking one off could take the label off the next
        // dialog on screen. A tool that ran is taken off by its own hook, and one refused by the end of the turn. So a card may say more than it must, and says less only when the turn's end
        // is reported while a background agent's dialog is still open, which clears the list.
        if enter {
            self.enter_later(typed);
        }
        Ok(())
    }

    /// Types a message on the prompt line. Refused while a prompt is shown (the text would go to
    /// its dialog) unless `force`. The caller sends Enter with [`Self::enter_if`] after
    /// `ENTER_DELAY`, waiting itself so that the next message does not run into this one's Enter.
    #[cfg(test)]
    pub fn type_message(&self, text: &str, force: bool) -> Result<Typed, AnswerError> {
        self.type_message_by(text, force, By::desktop())
    }

    /// [`Self::type_message`] for `by` (see [`Self::input_by`]).
    pub fn type_message_by(&self, text: &str, force: bool, by: By) -> Result<Typed, AnswerError> {
        let mut g = self.inner.lock();
        if !force && g.screen_prompt.is_some() {
            return Err(AnswerError::Prompt);
        }
        let body = typed_text(text);
        // Lines of a message are one paste: Enter keys inside it would submit each.
        let bytes = if !body.contains('\n') {
            body.into_bytes()
        } else if g.vt.bracketed_paste() {
            format!("\x1b[200~{}\x1b[201~", body.replace('\n', "\r")).into_bytes()
        } else {
            body.replace('\n', "\r").into_bytes()
        };
        g.typing_by = Some(by);
        Self::write_input(&mut g, bytes);
        Ok(Self::typed_marker(&g))
    }

    /// Enter as its own write, `ENTER_DELAY` after the text: agent TUIs take an Enter that follows
    /// a burst of input as part of it (Codex: within 120 ms) and insert a newline instead of
    /// submitting. It is sent only if nothing else was typed meanwhile and no new prompt came up,
    /// else it would answer or submit something it was not meant for.
    pub fn enter_if(&self, typed: Typed) -> bool {
        let mut g = self.inner.lock();
        let same_wait = g.waiting_since.is_none() || g.waiting_since == typed.wait;
        if g.pty.is_none() || g.activity.last_typed != typed.written || !same_wait {
            return false;
        }
        Self::write_input(&mut g, b"\r".to_vec());
        true
    }

    /// [`Self::enter_if`] after the delay, off the caller.
    fn enter_later(self: &Arc<Self>, typed: Typed) {
        let me = Arc::clone(self);
        std::thread::spawn(move || {
            std::thread::sleep(ENTER_DELAY);
            me.enter_if(typed);
        });
    }

    /// Types `bytes` for a timer that waits out the usage limit: only into the same run of the
    /// process (`generation`) and conversation that the limit stopped, and only while that is
    /// still so with nothing typed since. Checked and written under one lock, like [`Self::answer`].
    pub fn continue_if_limited(&self, generation: u64, agent_session_id: Option<&str>, turn_seq: u64, bytes: Vec<u8>) -> Result<Instant, Refused> {
        let mut g = self.inner.lock();
        if g.pty.is_none() {
            return Err(Refused::Exited);
        }
        if g.generation != generation {
            return Err(Refused::Restarted);
        }
        if self.spec.lock().agent_session_id.as_deref() != agent_session_id {
            return Err(Refused::Conversation);
        }
        // Another turn has begun since the timer was set: the limit it was for is over.
        if !(g.activity.failed && g.activity.limited) || g.activity.turn_seq != turn_seq {
            return Err(Refused::NotLimited);
        }
        if !g.activity.limit_untouched() {
            // Whoever typed has taken over; a later report of a limit cannot hand the session back.
            g.activity.limit_dead = true;
            return Err(Refused::InputSince);
        }
        if g.activity.status(Instant::now()) != Status::Idle {
            return Err(Refused::NotLimited);
        }
        Self::write_input(&mut g, bytes);
        Ok(g.activity.last_typed.unwrap_or_else(Instant::now))
    }

    /// Types `text` as one message, but only while the agent sits idle at its prompt (checked and
    /// written under one lock, like [`Self::answer`]). Returns when it was typed, for the Enter that follows.
    /// Not when something was typed within `quiet` (it may be somebody's half-written message).
    pub fn type_when_idle(&self, text: &str, quiet: Duration) -> Option<Instant> {
        let mut g = self.inner.lock();
        if g.pty.is_none() || g.activity.status(Instant::now()) != Status::Idle || g.activity.last_typed.is_some_and(|t| t.elapsed() < quiet) {
            return None;
        }
        let body = typed_text(text);
        let bytes = if g.vt.bracketed_paste() { format!("\x1b[200~{body}\x1b[201~") } else { body.replace('\n', " ") };
        Self::write_input(&mut g, bytes.into_bytes());
        Some(g.activity.last_typed.unwrap_or_else(Instant::now))
    }

    /// Sends `bytes` only if the write at `written` is still the last thing typed (the Enter that
    /// follows a timer's text must not follow anything the user typed in between).
    pub fn input_if_last_typed_at(&self, written: Instant, bytes: Vec<u8>) -> bool {
        let mut g = self.inner.lock();
        if g.pty.is_none() || g.activity.last_typed != Some(written) {
            return false;
        }
        Self::write_input(&mut g, bytes);
        true
    }

    /// Whether a report of the agent's state may be applied. `proven`: it came with the session's token;
    /// once one has, reports that are only claimed (a program in the session posing as the agent) are not.
    /// `allowed` is false for a claimed report.
    pub fn accepts_report(&self, allowed: bool, proven: bool) -> bool {
        let mut g = self.inner.lock();
        g.activity.verified_hooks |= proven;
        allowed || !g.activity.verified_hooks
    }

    /// Turns begun by this run of the agent (see `Activity::turn_seq`).
    pub fn turn_seq(&self) -> u64 {
        self.inner.lock().activity.turn_seq
    }

    /// The turn that just failed ended on the usage limit.
    pub fn set_limited(&self) {
        self.inner.lock().activity.on_limit();
    }

    /// Which run of the process this is: it grows with every spawn.
    pub fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let mut g = self.inner.lock();
        if g.vt.size() == (cols, rows) {
            return;
        }
        g.vt.resize(cols, rows);
        g.replay_floor = g.seq;
        self.wake.send_replace(g.seq);
        if let Some(pty) = &g.pty {
            let _ = pty.ctl.send(Ctl::Resize(cols, rows));
        }
    }

    pub fn set_unread(&self, unread: bool) {
        let mut g = self.inner.lock();
        g.unread = unread;
        // Force the next tick to broadcast.
        g.last_broadcast = None;
    }

    /// Restored sessions keep the activity time of their previous life, not the replay's.
    pub fn set_last_activity(&self, at: u64) {
        let mut g = self.inner.lock();
        g.last_activity = at;
        g.broadcast_activity = at;
    }

    pub fn set_focus(&self, focused: bool) {
        let mut g = self.inner.lock();
        g.focused = focused;
        if focused {
            g.unread = false;
        }
    }

    /// With `prioritize`, a session on screen runs ahead of other work and one no view shows
    /// yields to it (see `proc_tree`). Returned rather than applied: the Win32 calls run off
    /// the session lock and the async ticker. A custom command's own process is usually the
    /// workload itself (a dev server), so it is never raised.
    pub fn priority(&self, prioritize: bool) -> Option<(Arc<ProcTree>, Priority)> {
        let custom = self.spec.lock().kind == "custom";
        let g = self.inner.lock();
        let tree = g.pty.as_ref().and_then(|p| p.tree.clone())?;
        Some((tree, match (prioritize, g.focused) {
            (false, _) => Priority::Normal,
            (true, true) if custom => Priority::Normal,
            (true, true) => Priority::Foreground,
            (true, false) => Priority::Background,
        }))
    }

    /// The process the session runs, when it started, and its job when there is one; `None` once it has ended.
    pub fn proc_root(&self) -> Option<crate::procs::Root> {
        let g = self.inner.lock();
        let pty = g.pty.as_ref()?;
        Some(crate::procs::Root { pid: pty.pid?, start: pty.start, tree: pty.tree.clone() })
    }

    pub fn kill(&self) {
        let mut g = self.inner.lock();
        g.stopped = true;
        if let Some(pty) = &g.pty {
            match &pty.tree {
                Some(tree) => tree.kill(),
                None => {
                    let mut killer = pty.killer.clone_killer();
                    let _ = killer.kill();
                }
            }
        }
    }

    pub fn is_alive(&self) -> bool {
        self.inner.lock().pty.is_some()
    }

    /// Whether the agent has ever reported through a hook (else its turns are guessed from output).
    pub fn hooks_seen(&self) -> bool {
        self.inner.lock().activity.hook.is_some()
    }

    /// When something was last typed into the session.
    pub fn last_input(&self) -> Option<Instant> {
        self.inner.lock().activity.last_input
    }

    /// Whether the session printed within `within`, whatever a hook last said about it.
    pub fn output_within(&self, within: Duration, now: Instant) -> bool {
        self.inner.lock().activity.last_output.is_some_and(|o| now.duration_since(o) < within)
    }

    pub fn set_hook_status(&self, status: Status, failed: bool) {
        let mut g = self.inner.lock();
        g.activity.on_hook(status, failed, Instant::now());
        // The agent moved on: whatever it asks next is a new wait, even within one tick. A repeated
        // `waiting` (a second notice of the same request) is not.
        if status != Status::Waiting {
            g.answered_wait = false;
            g.answered_now = None;
            g.waiting_since = None;
        }
        // The turn is over, and so is whatever it asked. Another tool's work is not that: it says nothing of a dialog still open.
        if status == Status::Idle {
            g.wait_requests.clear();
        }
    }

    /// A receiver that fires when the screen, the prompt or the wait may have changed.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.wake.subscribe()
    }

    /// What a remote view shows right now, taken in one go under the lock. Labels are worked out after it is let go: reading a command is not to hold up the session, its reader or the ticker.
    pub fn screen_view(&self, height: usize) -> ScreenView {
        let worktree = self.spec.lock().cwd.clone();
        let (mut view, open) = {
            let g = self.inner.lock();
            let tail = g.vt.tail_lines(height);
            let waiting = g.screen_prompt.is_some() || g.waiting_since.is_some();
            let view = ScreenView {
                lines: tail.lines,
                cursor: tail.cursor,
                prompt: g.screen_prompt.clone(),
                waiting_since: g.waiting_since,
                title: g.vt.title().map(str::to_string),
                wait: None,
            };
            (view, waiting.then(|| g.wait_requests.clone()))
        };
        view.wait = open.map(|open| crate::mobile::risk::WaitInfo::of(&open, &worktree));
        view
    }

    /// Keeps track of what this session's wait is about, from its hooks. Each request stays until that tool has run or failed, or the turn is over (`set_hook_status`): a wait can outlast other
    /// tools' events, a long silence and the answer to another dialog, and two requests can be open at once, the same one twice as well. What a card says is about all of them (`WaitInfo::of`).
    pub fn request_hook(&self, event: &str, payload: &Value) {
        if !matches!(event, "PermissionRequest" | "PostToolUse" | "PostToolUseFailure" | "UserPromptSubmit") {
            return;
        }
        let cwd = self.spec.lock().cwd.clone();
        let request = payload.get("tool_name").and_then(Value::as_str).map(|tool| crate::mobile::risk::from_tool(tool, payload.get("tool_input").unwrap_or(&Value::Null), Some(&cwd)));
        let mut g = self.inner.lock();
        match (event, request) {
            ("PermissionRequest", Some(request)) => {
                g.wait_requests.push(request);
                g.typed_when_requests_changed = g.typed_count;
                // Which goes takes reading them all, and that is not done under the lock.
                if g.wait_requests.len() > WAIT_REQUESTS_KEPT {
                    let open = g.wait_requests.clone();
                    drop(g);
                    let mildest = crate::mobile::risk::mildest(&open, &cwd);
                    g = self.inner.lock();
                    if g.wait_requests.len() > WAIT_REQUESTS_KEPT {
                        if let Some(at) = mildest.and_then(|at| g.wait_requests.iter().position(|still| *still == open[at])) {
                            g.wait_requests.remove(at);
                        }
                    }
                }
            }
            ("PostToolUse" | "PostToolUseFailure", Some(request)) => {
                let Some(at) = g.wait_requests.iter().position(|open| *open == request) else { return };
                g.wait_requests.remove(at);
                // The next dialog may be up now, and nobody has typed at that one.
                g.typed_when_requests_changed = g.typed_count;
            }
            // A new turn: nothing from the one before is still being asked.
            ("UserPromptSubmit", _) => g.wait_requests.clear(),
            _ => return,
        }
        self.wake.send_replace(g.seq);
    }

    /// What must be approved in this wait, when the daemon's label on any of it says dangerous: what the card shows, which is what an assertion is over.
    pub fn dangerous_wait(&self) -> Option<String> {
        let worktree = self.spec.lock().cwd.clone();
        let open = self.inner.lock().wait_requests.clone();
        let info = crate::mobile::risk::WaitInfo::of(&open, &worktree);
        (info.risk.level == crate::mobile::risk::Level::Dangerous).then(|| info.command.unwrap_or_default())
    }

    /// Whether what is typed now could be the answer to a dangerous request. The screen and the status cannot say: a busy agent never goes quiet for a prompt to be found, and another tool's
    /// report ends a reported wait while the dialog stays up. So it counts as yes while a dangerous request is open, unless the only one open has been typed at since it came. Then Esc, a card's
    /// answer or the computer's keys have settled that dialog, and its entry only lasts until the turn ends. A key refused for this writes nothing, so it cannot lift this itself.
    pub fn typing_may_approve_danger(&self) -> bool {
        {
            let g = self.inner.lock();
            if g.wait_requests.len() == 1 && g.typed_count > g.typed_when_requests_changed {
                return false;
            }
        }
        self.dangerous_wait().is_some()
    }

    pub fn screen_text(&self) -> String {
        self.inner.lock().vt.screen_text()
    }

    /// The last `lines` lines of text, scrollback included.
    pub fn text_tail(&self, lines: usize) -> String {
        self.inner.lock().vt.text_tail(lines)
    }

    /// Whether `key` chooses, on the prompt shown now, something that outlasts it (`Prompt::lasting`).
    pub fn choice_lasts(&self, key: &str) -> bool {
        // Where a prompt tells `a` from `A`, each is its own choice; otherwise a key is the same whichever way it is typed.
        self.inner.lock().screen_prompt.as_ref().is_some_and(|p| p.lasting.iter().any(|k| k == key) || (!p.keys.iter().any(|k| k == key) && p.lasting.iter().any(|k| k.eq_ignore_ascii_case(key))))
    }

    /// The prompt on screen the session waits on, if any (`choices::detect`).
    pub fn screen_prompt(&self) -> Option<crate::choices::Prompt> {
        self.inner.lock().screen_prompt.clone()
    }

    /// Once output has been quiet a moment, reads the bottom of a screen that changed since the
    /// last look for a menu or yes/no question: once per screen, never while output streams.
    fn look_for_prompt(g: &mut Inner, now: Instant) {
        if g.pty.is_none() || g.seq == g.prompt_checked_seq {
            return;
        }
        if g.activity.last_output.is_some_and(|o| now.duration_since(o) < PROMPT_LOOK_QUIET) {
            return;
        }
        g.prompt_checked_seq = g.seq;
        let (found, fingerprint) = match crate::choices::detect_with_screen(&g.vt.text_tail(PROMPT_LOOK_LINES)) {
            // Already answered: it is still on screen until the program moves on.
            Some((_, fp)) if g.answered_prompt.as_deref() == Some(fp.as_str()) => (None, None),
            Some((p, fp)) => (Some(p), Some(fp)),
            None => (None, None),
        };
        g.prompt_fingerprint = fingerprint;
        // The same prompt still unanswered (the screen only repainted): the same wait.
        let same_wait = found.is_some()
            && found == g.screen_prompt
            && g.activity.screen_prompt.is_some_and(|at| g.activity.last_input.is_none_or(|i| i < at));
        if !same_wait {
            g.activity.screen_prompt = found.as_ref().map(|_| now);
        }
        g.screen_prompt = found;
    }

    /// Types `text` as a prompt once the program is ready (see [`PendingPrompt`]).
    pub fn queue_prompt(&self, prompt: TypedPrompt) {
        self.inner.lock().pending_prompt = Some(PendingPrompt { prompt, since: Instant::now(), pasted: false });
    }

    fn deliver_prompt(g: &mut Inner, now: Instant) {
        let Some(p) = &mut g.pending_prompt else { return };
        let Some(pty) = &g.pty else {
            g.pending_prompt = None;
            return;
        };
        if p.pasted {
            // Enter only once the program has drawn the paste: a TUI still busy (redrawing a
            // resumed conversation) takes an early Enter as part of the paste and never submits.
            let since = now.duration_since(p.since);
            let settled = g.last_output.is_none_or(|t| now.duration_since(t) >= PASTE_SETTLE);
            if (since >= Duration::from_millis(400) && settled) || since >= PASTE_MAX_WAIT {
                let _ = pty.ctl.send(Ctl::Input(b"\r".to_vec()));
                g.pending_prompt = None;
            }
            return;
        }
        let waited = now.duration_since(p.since);
        // Quiet only counts once the program has drawn something: an agent resuming a
        // conversation can be silent for seconds while it loads, and keys typed then reach its
        // input before it handles Enter (the text shows up, the Enter is lost).
        let quiet = g.last_output.map_or(Duration::ZERO, |t| now.duration_since(t));
        if (waited >= PROMPT_MIN_WAIT && quiet >= PROMPT_QUIET) || waited >= PROMPT_MAX_WAIT {
            let body = typed_text(&p.prompt.text);
            let bytes = if p.prompt.paste { format!("\x1b[200~{body}\x1b[201~") } else { body.replace('\n', "\r") };
            let _ = pty.ctl.send(Ctl::Input(bytes.into_bytes()));
            p.pasted = true;
            p.since = now;
        }
    }

    /// Restored after the daemon stopped while this session ran.
    pub fn mark_interrupted(&self, last_activity: u64) {
        let mut g = self.inner.lock();
        g.interrupted = true;
        g.last_activity = last_activity;
    }

    /// Not to be resumed after all (see `Action::Release`).
    pub fn clear_interrupted(&self) {
        self.inner.lock().interrupted = false;
    }

    pub fn info(&self) -> SessionInfo {
        let g = self.inner.lock();
        self.info_locked(&g, g.last_status)
    }

    fn info_locked(&self, g: &Inner, status: Status) -> SessionInfo {
        let (cols, rows) = g.vt.size();
        SessionInfo {
            handle: self.handle,
            spec: self.spec.lock().clone(),
            title: g.vt.title().map(str::to_string),
            status,
            unread: g.unread,
            alive: g.pty.is_some(),
            exit_code: g.exit_code,
            failed: match g.exit_code {
                Some(code) => !g.stopped && exit_failed(code),
                None => g.activity.failed,
            },
            limited: g.exit_code.is_none() && g.activity.failed && g.activity.limited,
            pid: g.pty.as_ref().and_then(|p| p.pid),
            cols,
            rows,
            seq: g.seq,
            chat: false,
            last_activity: g.last_activity,
            interrupted: g.interrupted,
            waiting_since: g.waiting_since,
            waits_for_window: false,
        }
    }

    /// Recomputes status; returns the new info when anything user-visible changed.
    pub fn tick(&self, now: Instant) -> Option<SessionInfo> {
        let mut g = self.inner.lock();
        if g.pending_prompt.is_some() {
            Self::deliver_prompt(&mut g, now);
        }
        let watched = self.wake.receiver_count() > 0;
        let before = watched.then(|| (g.waiting_since, g.screen_prompt.clone()));
        Self::look_for_prompt(&mut g, now);
        let status = g.activity.status(now);
        g.waiting_since = match (status, g.waiting_since) {
            (Status::Waiting, Some(t)) => Some(t),
            (Status::Waiting, None) => Some(now_millis()),
            _ => None,
        };
        if status != Status::Waiting {
            g.answered_wait = false;
            g.answered_now = None;
        }
        if status == Status::Exited {
            g.wait_requests.clear();
        }
        // A remote view shows the prompt and the wait too, which change without any output.
        if before.is_some_and(|b| b != (g.waiting_since, g.screen_prompt.clone())) {
            self.wake.send_replace(g.seq);
        }
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
        // seq / activity time change constantly; not a reason to broadcast
        info.seq = 0;
        info.last_activity = 0;
        if g.last_broadcast.as_ref() == Some(&info) && g.last_activity < g.broadcast_activity + ACTIVITY_REBROADCAST_MS {
            return None;
        }
        g.last_broadcast = Some(info.clone());
        g.broadcast_activity = g.last_activity;
        info.seq = g.seq;
        info.last_activity = g.last_activity;
        Some(info)
    }

    /// Snapshot for persistence when output arrived since the last one.
    pub fn snapshot_if_changed(&self, force: bool) -> Option<Vec<u8>> {
        let mut g = self.inner.lock();
        if !force && g.seq == g.persisted_seq {
            return None;
        }
        g.persisted_seq = g.seq;
        Some(g.vt.snapshot(SNAPSHOT_HISTORY))
    }

    pub fn title(&self) -> Option<String> {
        self.inner.lock().vt.title().map(str::to_string)
    }
}

static NEXT_CLIENT: AtomicU64 = AtomicU64::new(1);

pub fn next_client_id() -> u64 {
    NEXT_CLIENT.fetch_add(1, Ordering::Relaxed)
}

/// Only focus reports (`ESC [ I`, `ESC [ O`): the terminal view saying it gained or lost focus.
fn is_focus_report(bytes: &[u8]) -> bool {
    !bytes.is_empty() && bytes.chunks(3).all(|c| c == b"\x1b[I" || c == b"\x1b[O")
}

/// Only what the terminal view sends on its own: focus reports, mouse reports (SGR and X10),
/// and replies to the program's queries (device attributes, cursor position, status, mode and
/// colour reports). Anything else, a key or a paste, is the user.
fn is_terminal_report(bytes: &[u8]) -> bool {
    let mut at = 0;
    while at < bytes.len() {
        match report_len(&bytes[at..]) {
            Some(n) => at += n,
            None => return false,
        }
    }
    !bytes.is_empty()
}

/// Length of the report at the start of `b`, if it is one.
fn report_len(b: &[u8]) -> Option<usize> {
    if b.len() < 3 || b[0] != 0x1b {
        return None;
    }
    let fields = |p: &[u8]| p.split(|c| *c == b';').filter(|f| !f.is_empty() && f.iter().all(u8::is_ascii_digit)).count();
    match b[1] {
        b'[' => {
            if b[2] == b'M' {
                // X10 mouse: three bytes follow.
                return (b.len() >= 6).then_some(6);
            }
            let mut j = 2;
            let marker = matches!(b[j], b'<' | b'?' | b'>').then(|| b[j]);
            j += usize::from(marker.is_some());
            let params_at = j;
            while j < b.len() && (b[j].is_ascii_digit() || b[j] == b';') {
                j += 1;
            }
            let params = &b[params_at..j];
            let dollar = b.get(j) == Some(&b'$');
            j += usize::from(dollar);
            let last = *b.get(j)?;
            let is_report = match (marker, last, dollar) {
                (None, b'I' | b'O', false) => params.is_empty(),
                (Some(b'<'), b'M' | b'm', false) => fields(params) == 3,
                (Some(b'?' | b'>'), b'c', false) => true,
                (None, b'R', false) => fields(params) == 2,
                (None | Some(b'?'), b'n', false) => matches!(fields(params), 1 | 2),
                (None | Some(b'?'), b'y', true) => true,
                _ => false,
            };
            is_report.then_some(j + 1)
        }
        // A colour or title reply: up to BEL or ST.
        b']' => b.iter().position(|c| *c == 7).map(|p| p + 1).or_else(|| b.windows(2).position(|w| w == b"\x1b\\").map(|p| p + 2)),
        _ => None,
    }
}

/// Text to type or paste as a prompt: line breaks become `\n`, tabs stay, every other control
/// character goes (ESC above all). Prompts quote files, diffs and other sessions' output; an ESC
/// left in would let a `\x1b[201~` inside them end the bracketed paste early, and the rest would
/// arrive as keystrokes (commands run, prompts answered).
pub fn typed_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .chars()
        .filter_map(|c| match c {
            '\r' => Some('\n'),
            '\n' | '\t' => Some(c),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect()
}

/// Sessions without a process, for tests of what is written into them.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    #[derive(Debug, Clone)]
    struct NoKill;

    impl ChildKiller for NoKill {
        fn kill(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(NoKill)
        }
    }

    /// A terminal session that looks running; what is typed into it arrives on the receiver.
    pub fn running(handle: u32, id: &str, kind: &str) -> (Arc<Session>, Typed) {
        let spec = SessionSpec { id: id.into(), kind: kind.into(), cwd: ".".into(), ..Default::default() };
        let session = Session::new(handle, spec, 80, 24);
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut g = session.inner.lock();
            g.pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree: None, pid: None, start: None });
            g.generation = 1;
            g.activity = Activity { alive: true, ..Default::default() };
        }
        (session, Typed(rx))
    }

    pub struct Typed(std::sync::mpsc::Receiver<Ctl>);

    impl Typed {
        /// Everything written so far, one entry per write.
        pub fn writes(&self) -> Vec<Vec<u8>> {
            self.0.try_iter().filter_map(|c| if let Ctl::Input(b) = c { Some(b) } else { None }).collect()
        }

        /// The next write, waiting up to `ms` for it.
        pub fn next_within(&self, ms: u64) -> Option<Vec<u8>> {
            match self.0.recv_timeout(Duration::from_millis(ms)) {
                Ok(Ctl::Input(b)) => Some(b),
                _ => None,
            }
        }
    }

    /// The process ends.
    pub fn stop(session: &Session) {
        session.inner.lock().pty = None;
    }

    /// A menu is on screen, waiting for a key.
    pub fn show_prompt(session: &Session) {
        session.inner.lock().screen_prompt = Some(crate::choices::Prompt { keys: vec!["1".into(), "2".into()], question: None, line: false, lasting: Vec::new(), selected: None });
    }

    /// A menu is on screen where `key` chooses something that outlasts it.
    pub fn show_lasting_prompt(session: &Session, key: &str) {
        session.inner.lock().screen_prompt = Some(crate::choices::Prompt { keys: vec!["1".into(), "2".into(), "3".into()], question: None, line: false, lasting: vec![key.into()], selected: None });
    }

    /// The wait was reported long ago and the screen has printed since, so its status now goes by the output.
    pub fn wait_went_stale(session: &Session) {
        let now = Instant::now();
        {
            let mut g = session.inner.lock();
            g.activity.hook = Some((Status::Waiting, now.checked_sub(Duration::from_secs(600)).unwrap_or(now)));
            g.activity.on_output(now);
        }
        session.tick(now);
    }

    /// The agent reports a permission request; returns the wait's `waiting_since`.
    pub fn wait_on_user(session: &Session) -> u64 {
        let now = Instant::now();
        session.inner.lock().activity.on_hook(Status::Waiting, false, now);
        session.tick(now);
        session.info().waiting_since.expect("waiting")
    }
}

/// A process that cannot be killed, for sessions that only look running.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct NoKill;

#[cfg(test)]
impl ChildKiller for NoKill {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }
    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        Box::new(NoKill)
    }
}

#[cfg(test)]
impl Session {
    /// Makes the session look like it runs process `pid` (in `tree`, when it has a job).
    pub(crate) fn fake_process(&self, pid: u32, tree: Option<Arc<ProcTree>>) {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut g = self.inner.lock();
        g.pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree, pid: Some(pid), start: crate::procs::start_time_of(pid) });
    }

    /// Makes the session look like a running agent in `status`; what is typed into it shows up on the channel.
    pub(crate) fn fake_running(&self, status: Status) -> std::sync::mpsc::Receiver<Ctl> {
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut g = self.inner.lock();
            g.pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree: None, pid: None, start: None });
            g.activity = Activity { alive: true, ..Default::default() };
            g.activity.on_hook(status, false, Instant::now());
        }
        self.tick(Instant::now());
        rx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_that_proved_itself_ignores_reports_without_the_token() {
        let (s, _rx) = limited_session(1);
        assert!(s.accepts_report(false, false), "nobody has proved anything yet");
        assert!(s.accepts_report(true, false), "a caller with the user's token proves nothing about the agent");
        assert!(s.accepts_report(false, false));
        assert!(s.accepts_report(true, true));
        assert!(!s.accepts_report(false, false), "now a claimed report is a program posing as the agent");
        assert!(s.accepts_report(true, false) && s.accepts_report(true, true));
    }

    #[test]
    fn a_mouse_report_is_not_typing_for_the_timer_but_still_answers_a_prompt() {
        let (s, rx) = limited_session(1);
        s.input(b"\x1b[<0;5;5M\x1b[I".to_vec());
        assert!(s.inner.lock().activity.last_input.is_some(), "a click is input as far as prompts go");
        assert!(s.inner.lock().activity.last_typed.is_none());
        assert!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()).is_ok());
        assert_eq!(typed(&rx).len(), 2);
    }

    #[test]
    fn focus_reports_are_not_input() {
        assert!(is_focus_report(b"\x1b[I"));
        assert!(is_focus_report(b"\x1b[O\x1b[I"));
        assert!(!is_focus_report(b"\x1b[A"));
        assert!(!is_focus_report(b"y"));
        assert!(!is_focus_report(b""));
    }

    #[test]
    fn what_the_terminal_view_sends_by_itself_is_not_typing() {
        for report in [
            &b"\x1b[I"[..], b"\x1b[O\x1b[I", b"\x1b[<0;41;12M", b"\x1b[<35;100;7m", b"\x1b[M #!", b"\x1b[?1;2c", b"\x1b[>0;276;0c", b"\x1b[12;40R",
            b"\x1b[0n", b"\x1b[?997;1n", b"\x1b[?2004;2$y", b"\x1b]11;rgb:1e1e/1e1e/1e1e\x07", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\", b"\x1b[<64;5;5M\x1b[<64;5;5M\x1b[I",
        ] {
            assert!(is_terminal_report(report), "{:?}", String::from_utf8_lossy(report));
        }
        for typed in [&b"y"[..], b"", b"\x1b[A", b"\x1b[1;5C", b"\x1b", b"\x1b[<0;41M", b"hello\x1b[I", b"\x1b[I\r", b"\x1b[200~x\x1b[201~", b"\x1b[?1;2", b"\x1bOP"] {
            assert!(!is_terminal_report(typed), "{:?}", String::from_utf8_lossy(typed));
        }
    }

    #[test]
    fn typed_text_drops_escapes_that_could_end_a_paste() {
        assert_eq!(typed_text("a\x1b[201~\rrm -rf ~\r"), "a[201~\nrm -rf ~\n");
        assert_eq!(typed_text("x\u{9b}31m\u{7}y\tz\r\nw"), "x31my\tz\nw");
        assert_eq!(typed_text("héllo — 日本"), "héllo — 日本");
    }

    /// A claude session that looks running (no process behind it) and was stopped by the usage
    /// limit; what it is sent shows up on the returned channel.
    fn limited_session(generation: u64) -> (Arc<Session>, std::sync::mpsc::Receiver<Ctl>) {
        let spec = SessionSpec { id: "s1".into(), kind: "claude".into(), cwd: ".".into(), agent_session_id: Some("conv-1".into()), ..Default::default() };
        let session = Session::new(1, spec, 80, 24);
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut g = session.inner.lock();
            g.pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree: None, pid: None, start: None });
            g.generation = generation;
            g.activity = Activity { alive: true, ..Default::default() };
            g.activity.on_hook(Status::Working, false, Instant::now());
            g.activity.on_hook(Status::Idle, true, Instant::now());
        }
        session.set_limited();
        (session, rx)
    }

    fn typed(rx: &std::sync::mpsc::Receiver<Ctl>) -> Vec<Vec<u8>> {
        rx.try_iter().filter_map(|c| if let Ctl::Input(b) = c { Some(b) } else { None }).collect()
    }

    #[test]
    fn the_continue_timer_types_only_into_the_run_and_conversation_it_was_set_for() {
        let (s, rx) = limited_session(3);
        assert!(s.info().limited && s.info().failed);
        assert!(s.continue_if_limited(3, Some("conv-1"), 1, b"continue".to_vec()).is_ok());
        assert_eq!(typed(&rx), vec![b"continue".to_vec()]);
        // Typing counts as input: a second timer for the same stop would be refused.
        assert_eq!(s.continue_if_limited(3, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::InputSince));
        assert!(typed(&rx).is_empty());

        // The process was restarted since: a different generation.
        let (s, rx) = limited_session(4);
        assert_eq!(s.continue_if_limited(3, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::Restarted));
        // /clear or /resume moved the session to another conversation.
        s.spec.lock().agent_session_id = Some("conv-2".into());
        assert_eq!(s.continue_if_limited(4, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::Conversation));
        s.spec.lock().agent_session_id = None;
        assert_eq!(s.continue_if_limited(4, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::Conversation));
        assert!(typed(&rx).is_empty(), "nothing was typed in any refused case");
    }

    #[test]
    fn a_draft_typed_while_the_turn_ran_blocks_the_timer_and_a_forged_limit_cannot_bring_it_back() {
        let spec = SessionSpec { id: "s1".into(), kind: "claude".into(), cwd: ".".into(), agent_session_id: Some("conv-1".into()), ..Default::default() };
        let session = Session::new(1, spec, 80, 24);
        let (tx, rx) = std::sync::mpsc::channel();
        session.inner.lock().pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree: None, pid: None, start: None });
        session.inner.lock().generation = 1;
        let hook = |status, failed| session.inner.lock().activity.on_hook(status, failed, Instant::now());
        // The prompt is sent, the turn starts, the user starts typing the next message, the turn fails.
        session.input(b"first prompt\r".to_vec());
        std::thread::sleep(Duration::from_millis(5));
        hook(Status::Working, false);
        std::thread::sleep(Duration::from_millis(5));
        session.input(b"half a mess".to_vec());
        std::thread::sleep(Duration::from_millis(5));
        hook(Status::Idle, true);
        session.set_limited();
        assert!(session.info().limited);
        assert_eq!(session.continue_if_limited(1, Some("conv-1"), session.turn_seq(), b"continue".to_vec()), Err(Refused::InputSince));
        let sent: Vec<Vec<u8>> = rx.try_iter().filter_map(|c| if let Ctl::Input(b) = c { Some(b) } else { None }).collect();
        assert!(!sent.iter().any(|b| b == b"continue"), "the draft was not submitted");
        // A program reporting the limit again does not bring the timer or the banner back.
        hook(Status::Idle, true);
        session.set_limited();
        assert!(!session.info().limited);
    }

    #[test]
    fn a_timer_is_void_once_another_turn_has_begun() {
        let (s, rx) = limited_session(1);
        let old = s.turn_seq();
        // A forged "prompt submitted" followed by a forged limit report is another turn.
        s.inner.lock().activity.on_hook(Status::Working, false, Instant::now());
        s.inner.lock().activity.on_hook(Status::Idle, true, Instant::now());
        s.set_limited();
        assert_eq!(s.continue_if_limited(1, Some("conv-1"), old, b"continue".to_vec()), Err(Refused::NotLimited));
        assert!(typed(&rx).is_empty());
    }

    #[test]
    fn enter_follows_the_timers_text_only_if_nothing_was_typed_between() {
        let (s, rx) = limited_session(1);
        let at = s.continue_if_limited(1, Some("conv-1"), s.turn_seq(), b"continue".to_vec()).unwrap();
        assert!(s.input_if_last_typed_at(at, b"\r".to_vec()));
        let (s, rx2) = limited_session(1);
        let at = s.continue_if_limited(1, Some("conv-1"), s.turn_seq(), b"continue".to_vec()).unwrap();
        std::thread::sleep(Duration::from_millis(5));
        s.input(b"x".to_vec());
        assert!(!s.input_if_last_typed_at(at, b"\r".to_vec()), "the user typed meanwhile");
        assert_eq!(typed(&rx), vec![b"continue".to_vec(), b"\r".to_vec()]);
        assert_eq!(typed(&rx2), vec![b"continue".to_vec(), b"x".to_vec()]);
    }

    #[test]
    fn the_continue_timer_gives_way_to_the_user_and_to_a_session_that_moved_on() {
        // The user typed since the limit was reported.
        let (s, rx) = limited_session(1);
        s.input(b"hello".to_vec());
        assert_eq!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::InputSince));
        assert_eq!(typed(&rx), vec![b"hello".to_vec()], "only the user's own input reached the process");

        // A focus report is not typing.
        let (s, rx) = limited_session(1);
        s.input(b"\x1b[I".to_vec());
        assert!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()).is_ok());
        assert_eq!(typed(&rx).last(), Some(&b"continue".to_vec()));

        // The agent works again (the user continued by other means): no longer limited.
        let (s, rx) = limited_session(1);
        s.inner.lock().activity.on_hook(Status::Working, false, Instant::now());
        assert!(!s.info().limited);
        assert_eq!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::NotLimited));

        // Failed for another reason than the limit.
        let (s, rx2) = limited_session(1);
        s.inner.lock().activity.on_hook(Status::Idle, true, Instant::now());
        assert!(!s.info().limited);
        assert_eq!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::NotLimited));

        // Output on the screen: the agent is busy, whatever the failure flag says.
        let (s, rx3) = limited_session(1);
        {
            let mut g = s.inner.lock();
            g.activity.hook = None;
            g.activity.on_output(Instant::now());
        }
        assert_eq!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::NotLimited));

        // The process is gone.
        let (s, _rx4) = limited_session(1);
        s.inner.lock().pty = None;
        assert_eq!(s.continue_if_limited(1, Some("conv-1"), 1, b"continue".to_vec()), Err(Refused::Exited));
        assert!(typed(&rx).is_empty() && typed(&rx2).is_empty() && typed(&rx3).is_empty());
    }

    #[test]
    fn ring_since_bounds() {
        let mut r = Ring { buf: VecDeque::new() };
        r.push(b"hello");
        r.push(b" world");
        assert_eq!(r.since(5, 11).unwrap(), b" world");
        assert_eq!(r.since(11, 11).unwrap(), b"");
        assert!(r.since(12, 11).is_none());
        let big = vec![b'x'; RING_CAP + 10];
        r.push(&big);
        let seq = 11 + big.len() as u64;
        assert!(r.since(0, seq).is_none());
        assert_eq!(r.since(seq - 3, seq).unwrap(), b"xxx");
    }

    #[test]
    fn a_wait_is_answered_once() {
        let (s, rx) = fake::running(1, "s1", "claude");
        let since = fake::wait_on_user(&s);
        assert_eq!(s.answer(Some(since), Answer::Text("1"), false), Ok(()));
        // A second tap, from this device or another: the wait was answered, by the desktop.
        assert_eq!(answered_by(s.answer(Some(since), Answer::Text("1"), false)), Some("desktop".to_string()));
        assert_eq!(rx.writes(), vec![b"1".to_vec()], "only the first answer was typed");
    }

    /// Who the error says answered, when it says that.
    fn answered_by(result: Result<(), AnswerError>) -> Option<String> {
        match result {
            Err(AnswerError::Answered(r)) => Some(r.by.name),
            _ => None,
        }
    }

    #[test]
    fn a_late_answer_is_told_who_answered_and_when() {
        let (s, rx) = fake::running(1, "s1", "claude");
        let since = fake::wait_on_user(&s);
        let phone = By { id: "d1".into(), name: "Pixel".into() };
        assert_eq!(s.answer_by(Some(since), Answer::Text("1"), false, phone.clone()), Ok(()));
        let after = now_millis();
        let late = s.answer_by(Some(since), Answer::Text("2"), false, By { id: "d2".into(), name: "iPad".into() });
        let Err(AnswerError::Answered(r)) = late else { panic!("{late:?}") };
        assert_eq!((r.by, r.at <= after), (phone, true));
        // Still told after the agent moved on and asked again: the wait it answered is remembered.
        s.set_hook_status(Status::Working, false);
        std::thread::sleep(Duration::from_millis(3));
        let second = fake::wait_on_user(&s);
        assert_ne!(since, second);
        assert_eq!(answered_by(s.answer(Some(since), Answer::Text("3"), false)), Some("Pixel".to_string()));
        assert_eq!(rx.writes(), vec![b"1".to_vec()]);
    }

    #[test]
    fn an_open_wait_knows_who_answered_it_after_the_ring_has_forgotten() {
        let (s, _rx) = fake::running(1, "s1", "claude");
        let since = fake::wait_on_user(&s);
        let by = By { id: "d1".into(), name: "Pixel".into() };
        s.input_by(b"1".to_vec(), by.clone());
        s.inner.lock().resolved.clear();
        let late = s.answer_by(Some(since), Answer::Text("2"), false, By { id: "d2".into(), name: "iPad".into() });
        let Err(AnswerError::Answered(r)) = late else { panic!("{late:?}") };
        assert_eq!(r.by, by);
        // Once the agent moves on there is nothing to say about it any more.
        s.set_hook_status(Status::Working, false);
        assert!(s.inner.lock().answered_now.is_none());
    }

    #[test]
    fn the_first_to_answer_stays_the_one_who_did_past_the_rings_ten_minutes() {
        let (s, _rx) = fake::running(1, "s1", "claude");
        let since = fake::wait_on_user(&s);
        let first = By { id: "d1".into(), name: "Pixel".into() };
        s.input_by(b"1".to_vec(), first.clone());
        // The ring has forgotten the wait; someone else types into it (it is still open) and asks.
        s.inner.lock().resolved.clear();
        s.input_by(b"2".to_vec(), By { id: "d2".into(), name: "iPad".into() });
        s.input(b"3".to_vec());
        let late = s.answer_by(Some(since), Answer::Text("4"), false, By { id: "d3".into(), name: "Tab".into() });
        let Err(AnswerError::Answered(r)) = late else { panic!("{late:?}") };
        assert_eq!(r.by, first, "the later keys did not take the credit");
        assert!(s.inner.lock().resolved.is_empty(), "and left no entry of their own");
    }

    #[test]
    fn the_last_twenty_answered_waits_are_remembered() {
        let (s, _rx) = fake::running(1, "s1", "claude");
        let mut waits = Vec::new();
        for _ in 0..(RESOLVED_KEPT + 3) {
            let since = fake::wait_on_user(&s);
            assert_eq!(s.answer(Some(since), Answer::Text("1"), false), Ok(()));
            waits.push(since);
            s.set_hook_status(Status::Working, false);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(s.inner.lock().resolved.len(), RESOLVED_KEPT);
        // The oldest are forgotten: all that is known of them is that they are gone.
        assert_eq!(s.answer(Some(waits[0]), Answer::Text("x"), false), Err(AnswerError::Stale("not_waiting")));
        assert!(answered_by(s.answer(Some(*waits.last().unwrap()), Answer::Text("x"), false)).is_some());
        // And nothing is remembered past ten minutes.
        for (_, r) in s.inner.lock().resolved.iter_mut() {
            r.at -= RESOLVED_TTL_MS + 1;
        }
        assert_eq!(s.answer(Some(*waits.last().unwrap()), Answer::Text("x"), false), Err(AnswerError::Stale("not_waiting")));
    }

    #[test]
    fn typing_at_the_terminal_answers_a_hooks_wait_for_everyone_else() {
        // The hook keeps saying `waiting` after the key press, until the agent's next report.
        let (s, rx) = fake::running(1, "s1", "claude");
        let since = fake::wait_on_user(&s);
        s.input(b"1".to_vec());
        assert_eq!(s.info().status, Status::Waiting, "the hook still says so");
        assert_eq!(answered_by(s.answer(Some(since), Answer::Text("1"), false)), Some("desktop".to_string()));
        assert_eq!(rx.writes(), vec![b"1".to_vec()]);

        // What the terminal view reports on its own is not an answer.
        let (s, rx) = fake::running(2, "s2", "claude");
        let since = fake::wait_on_user(&s);
        s.input(b"\x1b[I\x1b[?1;2c".to_vec());
        assert_eq!(s.answer(Some(since), Answer::Text("1"), false), Ok(()));
        assert_eq!(rx.writes().len(), 2);
    }

    #[test]
    fn the_next_wait_can_be_answered_again() {
        let (s, rx) = fake::running(1, "s1", "claude");
        let first = fake::wait_on_user(&s);
        assert_eq!(s.answer(Some(first), Answer::Text("1"), false), Ok(()));
        // The agent works on, then asks again.
        s.inner.lock().activity.on_hook(Status::Working, false, Instant::now());
        s.tick(Instant::now());
        assert!(answered_by(s.answer(Some(first), Answer::Text("1"), false)).is_some(), "the first wait was answered");
        std::thread::sleep(Duration::from_millis(3));
        let second = fake::wait_on_user(&s);
        assert_ne!(first, second);
        assert!(answered_by(s.answer(Some(first), Answer::Text("1"), false)).is_some(), "an answer to the old prompt");
        assert_eq!(s.answer(Some(second), Answer::Text("2"), false), Ok(()));
        assert_eq!(rx.writes(), vec![b"1".to_vec(), b"2".to_vec()]);
        assert!(answered_by(s.answer(Some(second), Answer::Text("2"), false)).is_some());
    }

    #[test]
    fn an_answer_is_a_key_or_cleaned_text_and_enter_follows_after_the_agents_delay() {
        let (s, rx) = fake::running(1, "s1", "codex");
        let since = fake::wait_on_user(&s);
        assert_eq!(s.answer(Some(since), Answer::Key("tab"), false), Err(AnswerError::UnknownKey));
        assert!(rx.writes().is_empty());
        let at = Instant::now();
        assert_eq!(s.answer(Some(since), Answer::Text("y\x1b[201~"), true), Ok(()));
        assert_eq!(rx.next_within(50), Some(b"y[201~".to_vec()), "text first, cleaned of escapes");
        assert_eq!(rx.next_within(1000), Some(b"\r".to_vec()));
        assert!(at.elapsed() >= ENTER_DELAY, "Enter came after {:?}", at.elapsed());
    }

    #[test]
    fn a_message_is_refused_while_a_prompt_is_shown_unless_forced() {
        let (s, rx) = fake::running(1, "s1", "claude");
        fake::show_prompt(&s);
        assert_eq!(s.type_message("hello", false).map(|_| ()), Err(AnswerError::Prompt));
        assert!(rx.writes().is_empty());
        assert_eq!(s.type_message("hello", true).map(|_| ()), Ok(()));
        assert_eq!(rx.writes(), vec![b"hello".to_vec()]);
    }

    #[test]
    fn a_message_cannot_break_out_of_its_paste() {
        let (s, rx) = fake::running(1, "s1", "claude");
        assert_eq!(s.type_message("a\x1b[201~\rrm -rf ~", false).map(|_| ()), Ok(()));
        // The escape is gone; no paste markers were asked for, so a line break is an Enter key.
        assert_eq!(rx.writes(), vec![b"a[201~\rrm -rf ~".to_vec()]);
        s.ingest(b"\x1b[?2004h");
        assert_eq!(s.type_message("one\ntwo\x1b[201~", false).map(|_| ()), Ok(()));
        assert_eq!(rx.writes(), vec![b"\x1b[200~one\rtwo[201~\x1b[201~".to_vec()], "the program asked for bracketed paste: the lines are one paste");
    }

    #[test]
    fn watchers_wake_for_output_and_for_a_new_prompt() {
        let (s, _rx) = fake::running(1, "s1", "claude");
        let mut wake = s.watch();
        assert!(!wake.has_changed().unwrap());
        s.ingest(b"hello\r\n");
        assert!(wake.has_changed().unwrap(), "output wakes the sync");
        wake.borrow_and_update();
        fake::wait_on_user(&s);
        assert!(wake.has_changed().unwrap(), "so does a wait that came with no output");
        wake.borrow_and_update();
        s.tick(Instant::now());
        assert!(!wake.has_changed().unwrap(), "a tick that changes nothing does not");
        s.resize(100, 30);
        assert!(wake.has_changed().unwrap());
        let view = s.screen_view(10);
        assert_eq!((view.lines, view.waiting_since.is_some()), (vec!["hello".to_string()], true));
    }

    #[test]
    fn a_quick_second_prompt_is_a_new_wait_that_can_be_answered() {
        let (s, rx) = fake::running(1, "s1", "claude");
        let first = fake::wait_on_user(&s);
        assert_eq!(s.answer(Some(first), Answer::Text("1"), false), Ok(()));
        // Working and asking again between two ticks: the tick never sees the agent working.
        std::thread::sleep(Duration::from_millis(3));
        s.set_hook_status(Status::Working, false);
        s.set_hook_status(Status::Waiting, false);
        s.tick(Instant::now());
        let second = s.info().waiting_since.expect("waiting again");
        assert_ne!(first, second);
        assert!(answered_by(s.answer(Some(first), Answer::Text("1"), false)).is_some());
        assert_eq!(s.answer(Some(second), Answer::Text("2"), false), Ok(()));
        assert_eq!(rx.writes(), vec![b"1".to_vec(), b"2".to_vec()]);
        // A second notice of the same request is the same wait, and stays answered.
        s.set_hook_status(Status::Waiting, false);
        assert!(answered_by(s.answer(Some(second), Answer::Text("2"), false)).is_some());
    }

    #[test]
    fn an_enter_that_would_land_on_something_else_is_not_sent() {
        // Nothing happened meanwhile: it goes.
        let (s, rx) = fake::running(1, "s1", "codex");
        let typed = s.type_message("hello", false).unwrap();
        assert!(s.enter_if(typed));
        assert_eq!(rx.writes(), vec![b"hello".to_vec(), b"\r".to_vec()]);

        // The user typed in between.
        let typed = s.type_message("again", false).unwrap();
        std::thread::sleep(Duration::from_millis(3));
        s.input(b"x".to_vec());
        assert!(!s.enter_if(typed));
        assert_eq!(rx.writes(), vec![b"again".to_vec(), b"x".to_vec()]);

        // A prompt came up within the delay: the Enter would answer it.
        let (s, rx) = fake::running(2, "s2", "codex");
        let typed = s.type_message("hello", false).unwrap();
        fake::wait_on_user(&s);
        assert!(!s.enter_if(typed));
        assert_eq!(rx.writes(), vec![b"hello".to_vec()]);

        // The same through an answer given with Enter.
        let (s, rx) = fake::running(3, "s3", "codex");
        let since = fake::wait_on_user(&s);
        assert_eq!(s.answer(Some(since), Answer::Text("y"), true), Ok(()));
        std::thread::sleep(Duration::from_millis(3));
        s.set_hook_status(Status::Working, false);
        s.set_hook_status(Status::Waiting, false);
        s.tick(Instant::now());
        assert_eq!(rx.next_within(50), Some(b"y".to_vec()));
        assert_eq!(rx.next_within(600), None, "the delayed Enter found another prompt and stayed away");
    }
}
