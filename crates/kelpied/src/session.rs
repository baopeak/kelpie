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

pub type ClientTx = tokio::sync::mpsc::UnboundedSender<Outgoing>;

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

enum Ctl {
    Input(Vec<u8>),
    Resize(u16, u16),
    Close,
}

struct Pty {
    ctl: std::sync::mpsc::Sender<Ctl>,
    killer: Box<dyn ChildKiller + Send + Sync>,
    tree: Option<Arc<ProcTree>>,
    pid: Option<u32>,
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
            g.pty = Some(Pty { ctl: ctl_tx.clone(), killer, tree, pid });
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
        let mut g = self.inner.lock();
        Self::write_input(&mut g, bytes);
    }

    fn write_input(g: &mut Inner, bytes: Vec<u8>) {
        // Focus reports (`ESC [ I` / `ESC [ O`) are not the user answering anything: a prompt keeps
        // waiting while they look elsewhere. Mouse reports and query replies are not typing either,
        // for the usage-limit timer; a click can still answer a prompt.
        if !is_terminal_report(&bytes) {
            let now = Instant::now();
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
            g.activity.last_input = Some(Instant::now());
            g.last_activity = now_millis();
            g.activity.bell = None;
        }
        if let Some(pty) = &g.pty {
            let _ = pty.ctl.send(Ctl::Input(bytes));
        }
    }

    /// Types `bytes` only while the session still waits on the prompt that began at
    /// `waiting_since` — checked and written under one lock, so nothing can answer it in between.
    pub fn answer(&self, waiting_since: Option<u64>, bytes: Vec<u8>) -> bool {
        let mut g = self.inner.lock();
        let waiting = g.activity.status(Instant::now()) == Status::Waiting;
        if !waiting || g.waiting_since.is_none() || g.waiting_since != waiting_since {
            return false;
        }
        Self::write_input(&mut g, bytes);
        true
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
        self.inner.lock().activity.on_hook(status, failed, Instant::now());
    }

    pub fn screen_text(&self) -> String {
        self.inner.lock().vt.screen_text()
    }

    /// The last `lines` lines of text, scrollback included.
    pub fn text_tail(&self, lines: usize) -> String {
        self.inner.lock().vt.text_tail(lines)
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
        }
    }

    /// Recomputes status; returns the new info when anything user-visible changed.
    pub fn tick(&self, now: Instant) -> Option<SessionInfo> {
        let mut g = self.inner.lock();
        if g.pending_prompt.is_some() {
            Self::deliver_prompt(&mut g, now);
        }
        Self::look_for_prompt(&mut g, now);
        let status = g.activity.status(now);
        g.waiting_since = match (status, g.waiting_since) {
            (Status::Waiting, Some(t)) => Some(t),
            (Status::Waiting, None) => Some(now_millis()),
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

    /// A claude session that looks running (no process behind it) and was stopped by the usage
    /// limit; what it is sent shows up on the returned channel.
    fn limited_session(generation: u64) -> (Arc<Session>, std::sync::mpsc::Receiver<Ctl>) {
        let spec = SessionSpec { id: "s1".into(), kind: "claude".into(), cwd: ".".into(), agent_session_id: Some("conv-1".into()), ..Default::default() };
        let session = Session::new(1, spec, 80, 24);
        let (tx, rx) = std::sync::mpsc::channel();
        {
            let mut g = session.inner.lock();
            g.pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree: None, pid: None });
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
        session.inner.lock().pty = Some(Pty { ctl: tx, killer: Box::new(NoKill), tree: None, pid: None });
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
}
