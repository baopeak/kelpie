//! The transfer queue the daemon owns: copy, delete and chmod jobs between any two ends, with live progress,
//! speed and ETA, pause, cancel and retry, conflict questions, and resume after a dropped connection or a
//! daemon restart. Jobs are kept in the journal (`<data>/transfers.json`), so the queue outlives the window.
//!
//! - At most 3 jobs run at once; the rest wait in order. A job copies up to 4 files at once, a file of 8 MiB or
//!   more alone. Each job opens its own SFTP sessions, so browsing never waits behind it.
//! - A dropped connection is retried after 2, 5, 15, 30 and 60 s for up to 5 minutes; files resume from their
//!   partials, and files already complete (same size and time) are not copied again.
//! - A daemon restart loads unfinished jobs as interrupted; the person resumes them.
//! - Conflicts follow the job's policy; `ask` puts a question on the job while its other files go on.
//! - Logs name the job, its kind and byte counts, never file contents or names.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Semaphore};

use super::copy::{copy_file, decide, Control, Copied, Decision, FileCopy, Policy, Stop};
use super::journal::{self, Journal, Loaded};
use super::local_fs::LocalFs;
use super::names::{keep_both, partial_name, with_suffix, windows_safe};
use super::pool::Pool;
use super::sftp_fs::SftpFs;
use super::walk::{walk, Found, Links, Skipped};
use super::{check_local_path, check_remote_path, ErrKind, FsError, FsResult, Kind as FsKind, Meta, RemoteFs};

pub use super::Emit;

pub const MAX_RUNNING: usize = 3;
const FILES_AT_ONCE: u32 = 4;
const BIG_FILE: u64 = 8 << 20;
const BACKOFF: [u64; 5] = [2, 5, 15, 30, 60];
const RETRY_FOR: Duration = Duration::from_secs(5 * 60);
/// Per-file detail kept for failures; beyond it only the count.
const KEEP_FAILURES: usize = 200;
const KEEP_RENAMED: usize = 50;

/// One end of a job: this computer (no `dest`) or a host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct End {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    Copy,
    Delete,
    Chmod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Queued,
    Scanning,
    Running,
    /// Every file left waits for an answer.
    Waiting,
    Paused,
    /// The connection dropped: `attempt` and `next_s` say when it tries again.
    Reconnecting,
    /// The daemon restarted while the job ran: the person resumes it.
    Interrupted,
    Failed,
    Done,
    Cancelled,
}

impl State {
    fn finished(self) -> bool {
        matches!(self, State::Done | State::Cancelled | State::Failed)
    }
    fn active(self) -> bool {
        matches!(self, State::Queued | State::Scanning | State::Running | State::Waiting | State::Reconnecting)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Totals {
    pub files: u64,
    pub bytes: u64,
    pub done_files: u64,
    pub done_bytes: u64,
    pub skipped: u64,
    pub failed: u64,
}

/// What a waiting file asks the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Question {
    pub id: u64,
    /// The target that is in the way.
    pub path: String,
    pub src: Meta,
    pub dst: Meta,
    pub identical: bool,
    /// A file and a folder of the same name: only skip or keep both.
    pub kind_clash: bool,
    /// The target changed after it was approved, or since a file was opened for editing.
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub path: String,
    pub error: String,
}

/// The size and time a target must still have for an upload to replace it (an edited file's upload).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expect {
    pub size: u64,
    pub mtime: u64,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub kind: JobKind,
    pub src: End,
    #[serde(default)]
    pub dst: End,
    pub items: Vec<String>,
    #[serde(default)]
    pub dst_dir: String,
    #[serde(default = "ask")]
    pub conflict: Policy,
    #[serde(default = "yes")]
    pub preserve: bool,
    /// chmod: the bits; with `dirs_only_x`, files lose x and folders get x wherever they have r.
    #[serde(default)]
    pub mode: Option<u32>,
    #[serde(default)]
    pub dirs_only_x: bool,
    #[serde(default)]
    pub expect: Option<Expect>,
    pub state: State,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub next_s: u64,
    #[serde(default)]
    pub totals: Totals,
    #[serde(default)]
    pub speed_bps: u64,
    #[serde(default)]
    pub eta_s: Option<u64>,
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub questions: Vec<Question>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub failures: Vec<Failure>,
    #[serde(default)]
    pub walk: Skipped,
    /// Names the target could not hold, as (wanted, saved as).
    #[serde(default)]
    pub renamed: Vec<(String, String)>,
    /// Partials this job writes: partial path -> the source's (size, mtime) when it was started. Only these
    /// are ever resumed or removed.
    #[serde(default)]
    pub partials: BTreeMap<String, (u64, u64)>,
    pub created: u64,
    pub updated: u64,
    #[serde(default)]
    pub finished: Option<u64>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

fn ask() -> Policy {
    Policy::Ask
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The person's answer to a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    Replace,
    Skip,
    KeepBoth,
    Newer,
    SkipIdentical,
    /// An edited file whose server copy changed: replace it anyway.
    Overwrite,
    /// ... keep both: the upload goes to `name (edited).ext`.
    SaveCopy,
    /// ... do not upload.
    Discard,
}

impl Choice {
    /// The policy "apply to all" sets for the rest of the job.
    fn policy(self) -> Policy {
        match self {
            Choice::Replace | Choice::Overwrite => Policy::Replace,
            Choice::Skip | Choice::Discard => Policy::Skip,
            Choice::KeepBoth | Choice::SaveCopy => Policy::KeepBoth,
            Choice::Newer => Policy::Newer,
            Choice::SkipIdentical => Policy::SkipIdentical,
        }
    }
}

/// A running job's controls.
#[derive(Default)]
struct Live {
    ctl: Control,
    /// Set on pause or cancel: stops the walk too.
    halt: AtomicBool,
    answers: Mutex<HashMap<u64, oneshot::Sender<(Choice, bool)>>>,
    next_q: AtomicU64,
    /// Lower-cased targets written by this job (case-insensitive targets only).
    written: Mutex<HashSet<String>>,
    speed: Mutex<Speed>,
}

#[derive(Default)]
struct Speed {
    at: Option<Instant>,
    bytes: u64,
    ema: f64,
    emitted: Option<Instant>,
    emitted_bytes: u64,
}

pub struct Queue {
    file: PathBuf,
    pool: Arc<Pool>,
    emit: Emit,
    jobs: Mutex<Vec<Job>>,
    live: Mutex<HashMap<String, Arc<Live>>>,
    slots: Arc<Semaphore>,
    /// Off when the journal exists but could not be read: nothing is written over it.
    can_save: AtomicBool,
    notice: Mutex<Option<String>>,
    dirty: AtomicBool,
    ticker: std::sync::Once,
    /// Tests: in-memory hosts by destination instead of ssh.
    #[cfg(test)]
    test_ends: Mutex<HashMap<String, Arc<dyn RemoteFs>>>,
}

/// What one attempt of a job ended with.
enum Outcome {
    Done,
    Paused,
    Cancelled,
}

impl Queue {
    pub fn new(data_dir: PathBuf, pool: Arc<Pool>, emit: Emit) -> Arc<Queue> {
        let file = journal::path(&data_dir);
        let (jobs, can_save, notice) = match journal::load::<Job>(&file) {
            Loaded::Fresh => (Vec::new(), true, None),
            Loaded::Read(j) => (j.jobs, true, None),
            Loaded::Damaged { kept_at, error } => {
                tracing::warn!("transfers journal could not be understood ({error}); kept aside");
                (Vec::new(), true, Some(format!("The transfer list could not be read and was kept as {}", kept_at.display())))
            }
            Loaded::Unreadable { error } => {
                tracing::warn!("transfers journal could not be read: {error}");
                (Vec::new(), false, Some(format!("The transfer list could not be read ({error}); transfers are not saved until Kelpie's background service restarts")))
            }
        };
        let mut jobs = jobs;
        for j in &mut jobs {
            if j.state.active() || j.state == State::Paused {
                j.state = State::Interrupted;
            }
            j.questions.clear();
            j.speed_bps = 0;
            j.eta_s = None;
            j.current = None;
        }
        let q = Arc::new(Queue {
            file,
            pool,
            emit,
            jobs: Mutex::new(jobs),
            live: Mutex::new(HashMap::new()),
            slots: Arc::new(Semaphore::new(MAX_RUNNING)),
            can_save: AtomicBool::new(can_save),
            notice: Mutex::new(notice),
            dirty: AtomicBool::new(false),
            ticker: std::sync::Once::new(),
            #[cfg(test)]
            test_ends: Mutex::new(HashMap::new()),
        });
        q.prune_and_clean();
        q
    }

    // ---- the model ----

    fn get(&self, id: &str) -> Option<Job> {
        self.jobs.lock().iter().find(|j| j.id == id).cloned()
    }

    /// Changes a job, tells the clients and saves the journal.
    fn update(&self, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
        let job = {
            let mut jobs = self.jobs.lock();
            let j = jobs.iter_mut().find(|j| j.id == id)?;
            f(j);
            j.updated = now();
            j.clone()
        };
        (self.emit)("xfer.updated", json!({ "job": job }));
        self.save();
        Some(job)
    }

    /// Changes a job's counters only: sent by the ticker, saved at most once a second.
    fn count(&self, id: &str, f: impl FnOnce(&mut Job)) {
        if let Some(j) = self.jobs.lock().iter_mut().find(|j| j.id == id) {
            f(j);
        }
        self.dirty.store(true, Ordering::Relaxed);
    }

    fn save(&self) {
        if !self.can_save.load(Ordering::Relaxed) {
            return;
        }
        self.dirty.store(false, Ordering::Relaxed);
        let snapshot = Journal { version: journal::VERSION, jobs: self.jobs.lock().clone(), extra: Default::default() };
        if let Err(e) = journal::save(&self.file, &snapshot) {
            tracing::warn!("transfers journal: {e:#}");
        }
    }

    /// Finished jobs beyond 7 days or 200 go; their partials (only possible after a cancel failed to remove them)
    /// are removed with them.
    fn prune_and_clean(self: &Arc<Self>) {
        let removed: Vec<Job> = {
            let mut jobs = self.jobs.lock();
            let before: Vec<Job> = jobs.clone();
            journal::prune(&mut jobs, now(), |j| j.state.finished().then_some(j.finished.unwrap_or(j.updated)));
            before.into_iter().filter(|b| !jobs.iter().any(|j| j.id == b.id)).collect()
        };
        for j in removed {
            self.remove_partials(j);
        }
    }

    fn remove_partials(self: &Arc<Self>, job: Job) {
        if job.partials.is_empty() {
            return;
        }
        let me = self.clone();
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            if let Ok(fs) = me.end(&job.dst).await {
                for p in job.partials.keys() {
                    let _ = fs.remove_file(p).await;
                }
            }
        });
    }

    async fn end(&self, end: &End) -> FsResult<Arc<dyn RemoteFs>> {
        #[cfg(test)]
        if let Some(fs) = end.dest.as_ref().and_then(|d| self.test_ends.lock().get(d).cloned()) {
            return Ok(fs);
        }
        Ok(match &end.dest {
            None => Arc::new(LocalFs),
            Some(d) => Arc::new(SftpFs(self.pool.transfer(d).await?)),
        })
    }

    // ---- methods ----

    pub async fn rpc(self: &Arc<Self>, method: &str, p: &Value) -> Result<Value> {
        self.start_ticker();
        let id = || p.get("id").and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("missing param `id`"));
        match method {
            "xfer.start" => {
                let job = self.new_job(p)?;
                let id = job.id.clone();
                tracing::info!(job = %id, kind = ?job.kind, src = ?job.src.dest, dst = ?job.dst.dest, items = job.items.len(), "transfer queued");
                self.jobs.lock().push(job.clone());
                (self.emit)("xfer.updated", json!({ "job": job }));
                self.save();
                self.launch(&id, false);
                Ok(json!({ "id": id }))
            }
            "xfer.list" => Ok(json!({ "jobs": self.jobs.lock().clone(), "notice": self.notice.lock().clone() })),
            "xfer.control" => {
                let id = id()?;
                let action = p.get("action").and_then(Value::as_str).ok_or_else(|| anyhow!("missing param `action`"))?;
                self.control(&id, action)?;
                Ok(json!(true))
            }
            "xfer.answer" => {
                let id = id()?;
                let choice: Choice = serde_json::from_value(p.get("choice").cloned().unwrap_or(Value::Null)).map_err(|_| anyhow!("unknown choice"))?;
                let all = p.get("all").and_then(Value::as_bool).unwrap_or(false);
                let qid = p.get("question").and_then(Value::as_u64);
                self.answer(&id, qid, choice, all)?;
                Ok(json!(true))
            }
            "xfer.clear_finished" => {
                let removed: Vec<Job> = {
                    let mut jobs = self.jobs.lock();
                    let (gone, kept): (Vec<Job>, Vec<Job>) = jobs.drain(..).partition(|j| j.state.finished());
                    *jobs = kept;
                    gone
                };
                for j in removed {
                    (self.emit)("xfer.removed", json!({ "id": j.id }));
                    self.remove_partials(j);
                }
                self.save();
                Ok(json!(true))
            }
            other => bail!("unknown method {other}"),
        }
    }

    fn new_job(&self, p: &Value) -> Result<Job> {
        let kind: JobKind = serde_json::from_value(p.get("kind").cloned().unwrap_or(json!("copy"))).map_err(|_| anyhow!("unknown kind"))?;
        let end = |k: &str| -> Result<End> {
            let e: End = serde_json::from_value(p.get(k).cloned().unwrap_or(json!({}))).map_err(|_| anyhow!("bad `{k}`"))?;
            if let Some(d) = &e.dest {
                if !kelpie_proto::ssh::valid_dest(d) {
                    bail!("invalid ssh destination `{d}`");
                }
            }
            Ok(e)
        };
        let src = end("src")?;
        let dst = end("dst")?;
        let items: Vec<String> = p.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
        if items.is_empty() {
            bail!("nothing to transfer");
        }
        let check = |end: &End, path: &str| -> Result<()> {
            match end.dest {
                Some(_) => check_remote_path(path),
                None => check_local_path(path),
            }
            .map_err(|e| anyhow!("{e}"))
        };
        for i in &items {
            check(&src, i)?;
        }
        let dst_dir = p.get("dst_dir").and_then(Value::as_str).unwrap_or_default().to_string();
        match kind {
            JobKind::Copy => {
                if src.dest.is_none() && dst.dest.is_none() {
                    bail!("copies on this computer are for Explorer or Finder");
                }
                check(&dst, &dst_dir)?;
            }
            JobKind::Delete | JobKind::Chmod => {
                // A local delete goes to the trash through the app; neither runs on this computer here.
                if src.dest.is_none() {
                    bail!("{kind:?} jobs run on a host only");
                }
            }
        }
        let mode = p.get("mode").and_then(Value::as_u64).map(|m| m as u32 & 0o7777);
        if kind == JobKind::Chmod && mode.is_none() {
            bail!("missing param `mode`");
        }
        let expect: Option<Expect> = p.get("expect").cloned().filter(|v| !v.is_null()).map(serde_json::from_value).transpose().map_err(|_| anyhow!("bad `expect`"))?;
        if expect.is_some() && (items.len() != 1 || kind != JobKind::Copy) {
            bail!("`expect` is for one file's upload");
        }
        let conflict: Policy = serde_json::from_value(p.get("conflict").cloned().unwrap_or(json!("ask"))).map_err(|_| anyhow!("unknown conflict policy"))?;
        let t = now();
        Ok(Job {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            src,
            dst,
            items,
            dst_dir,
            conflict,
            preserve: p.get("preserve").and_then(Value::as_bool).unwrap_or(true),
            mode,
            dirs_only_x: p.get("dirs_only_x").and_then(Value::as_bool).unwrap_or(false),
            expect,
            state: State::Queued,
            attempt: 0,
            next_s: 0,
            totals: Totals::default(),
            speed_bps: 0,
            eta_s: None,
            current: None,
            questions: Vec::new(),
            error: None,
            failures: Vec::new(),
            walk: Skipped::default(),
            renamed: Vec::new(),
            partials: BTreeMap::new(),
            created: t,
            updated: t,
            finished: None,
            extra: Default::default(),
        })
    }

    fn control(self: &Arc<Self>, id: &str, action: &str) -> Result<()> {
        let job = self.get(id).ok_or_else(|| anyhow!("no transfer {id}"))?;
        let live = self.live.lock().get(id).cloned();
        match action {
            "pause" => {
                let live = live.ok_or_else(|| anyhow!("the transfer is not running"))?;
                live.ctl.pause.store(true, Ordering::Relaxed);
                live.halt.store(true, Ordering::Relaxed);
                live.answers.lock().clear();
            }
            "cancel" => match live {
                Some(live) => {
                    live.ctl.cancel.store(true, Ordering::Relaxed);
                    live.halt.store(true, Ordering::Relaxed);
                    live.answers.lock().clear();
                }
                None if !job.state.finished() => {
                    self.update(id, |j| {
                        j.state = State::Cancelled;
                        j.finished = Some(now());
                    });
                    self.update(id, |j| j.partials.clear());
                    self.remove_partials(job);
                }
                None => bail!("the transfer has already finished"),
            },
            "resume" | "retry" => {
                if live.is_some() {
                    bail!("the transfer is running");
                }
                if job.state == State::Done || job.state == State::Cancelled {
                    bail!("the transfer has finished");
                }
                self.update(id, |j| {
                    j.state = State::Queued;
                    j.error = None;
                    j.finished = None;
                });
                self.launch(id, true);
            }
            "clear" => {
                if !job.state.finished() {
                    bail!("only a finished transfer can be cleared");
                }
                self.jobs.lock().retain(|j| j.id != id);
                (self.emit)("xfer.removed", json!({ "id": id }));
                self.save();
                self.remove_partials(job);
            }
            other => bail!("unknown action {other}"),
        }
        Ok(())
    }

    fn answer(&self, id: &str, qid: Option<u64>, choice: Choice, all: bool) -> Result<()> {
        let live = self.live.lock().get(id).cloned().ok_or_else(|| anyhow!("the transfer is not running"))?;
        if all {
            self.update(id, |j| j.conflict = choice.policy());
        }
        let senders: Vec<oneshot::Sender<(Choice, bool)>> = {
            let mut answers = live.answers.lock();
            match (all, qid) {
                (true, _) => answers.drain().map(|(_, s)| s).collect(),
                (false, Some(q)) => answers.remove(&q).into_iter().collect(),
                (false, None) => {
                    let first = answers.keys().min().copied();
                    first.and_then(|q| answers.remove(&q)).into_iter().collect()
                }
            }
        };
        if senders.is_empty() {
            bail!("no question is waiting");
        }
        for s in senders {
            let _ = s.send((choice, all));
        }
        Ok(())
    }

    /// Before the daemon exits: what is running is saved as it is (it loads as interrupted).
    pub fn shutdown(&self) {
        for live in self.live.lock().values() {
            live.ctl.pause.store(true, Ordering::Relaxed);
            live.halt.store(true, Ordering::Relaxed);
        }
        self.save();
    }

    // ---- progress ----

    fn start_ticker(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.ticker.call_once(|| {
            tokio::spawn(async move {
                let mut saved = Instant::now();
                loop {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let Some(q) = me.upgrade() else { return };
                    q.tick();
                    if saved.elapsed() >= Duration::from_secs(1) && q.dirty.load(Ordering::Relaxed) {
                        q.save();
                        saved = Instant::now();
                    }
                }
            });
        });
    }

    /// At most 4 updates a second per job, with the speed as an average over about 3 s.
    fn tick(&self) {
        let live: Vec<(String, Arc<Live>)> = self.live.lock().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        for (id, l) in live {
            let bytes = l.ctl.bytes.load(Ordering::Relaxed);
            let (speed, changed) = {
                let mut s = l.speed.lock();
                let t = Instant::now();
                if let Some(at) = s.at {
                    let dt = t.duration_since(at).as_secs_f64().max(1e-3);
                    let inst = bytes.saturating_sub(s.bytes) as f64 / dt;
                    let alpha = (dt / 3.0).min(1.0);
                    s.ema += alpha * (inst - s.ema);
                }
                s.at = Some(t);
                s.bytes = bytes;
                let changed = s.emitted_bytes != bytes || s.emitted.is_none_or(|e| e.elapsed() >= Duration::from_secs(1));
                if changed {
                    s.emitted = Some(t);
                    s.emitted_bytes = bytes;
                }
                (s.ema.max(0.0) as u64, changed)
            };
            if !changed {
                continue;
            }
            let job = {
                let mut jobs = self.jobs.lock();
                let Some(j) = jobs.iter_mut().find(|j| j.id == id) else { continue };
                j.totals.done_bytes = bytes;
                j.speed_bps = speed;
                j.eta_s = (speed > 0 && j.state != State::Scanning).then(|| j.totals.bytes.saturating_sub(bytes) / speed);
                j.clone()
            };
            self.dirty.store(true, Ordering::Relaxed);
            (self.emit)("xfer.updated", json!({ "job": job }));
        }
    }

    // ---- running ----

    fn launch(self: &Arc<Self>, id: &str, resumed: bool) {
        let live = Arc::new(Live::default());
        self.live.lock().insert(id.to_string(), live.clone());
        let (me, id) = (self.clone(), id.to_string());
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            me.run(&id, live, resumed).await;
            me.live.lock().remove(&id);
        });
    }

    async fn run(self: &Arc<Self>, id: &str, live: Arc<Live>, mut resumed: bool) {
        let Ok(_slot) = self.slots.clone().acquire_owned().await else { return };
        if live.halt.load(Ordering::Relaxed) {
            let state = if live.ctl.cancel.load(Ordering::Relaxed) { State::Cancelled } else { State::Paused };
            self.update(id, |j| j.state = state);
            return;
        }
        let started = Instant::now();
        let mut attempt = 0u32;
        loop {
            self.update(id, |j| {
                j.state = State::Scanning;
                j.totals = Totals::default();
                j.walk = Skipped::default();
                j.attempt = attempt;
                j.next_s = 0;
            });
            live.ctl.bytes.store(0, Ordering::Relaxed);
            *live.speed.lock() = Speed::default();
            let result = self.attempt(id, &live, resumed).await;
            match result {
                Ok(Outcome::Done) => {
                    let job = self.update(id, |j| {
                        j.state = State::Done;
                        j.finished = Some(now());
                        j.current = None;
                        j.questions.clear();
                        j.speed_bps = 0;
                        j.eta_s = None;
                        j.totals.done_bytes = j.totals.done_bytes.max(live.ctl.bytes.load(Ordering::Relaxed));
                    });
                    if let Some(j) = job {
                        tracing::info!(job = %id, files = j.totals.done_files, bytes = j.totals.done_bytes, failed = j.totals.failed, "transfer done");
                    }
                }
                Ok(Outcome::Paused) => {
                    self.update(id, |j| {
                        j.state = State::Paused;
                        j.questions.clear();
                        j.speed_bps = 0;
                        j.eta_s = None;
                    });
                }
                Ok(Outcome::Cancelled) => {
                    let job = self.update(id, |j| {
                        j.state = State::Cancelled;
                        j.finished = Some(now());
                        j.questions.clear();
                        j.speed_bps = 0;
                        j.eta_s = None;
                    });
                    if let Some(j) = job {
                        self.update(id, |j| j.partials.clear());
                        self.remove_partials(j);
                    }
                }
                Err(e) if e.kind == ErrKind::Connection && started.elapsed() < RETRY_FOR => {
                    attempt += 1;
                    let wait = BACKOFF[(attempt as usize - 1).min(BACKOFF.len() - 1)];
                    tracing::info!(job = %id, attempt, wait, "transfer connection lost; retrying");
                    for left in (1..=wait).rev() {
                        self.update(id, |j| {
                            j.state = State::Reconnecting;
                            j.attempt = attempt;
                            j.next_s = left;
                            j.error = Some(e.message.clone());
                            j.speed_bps = 0;
                            j.eta_s = None;
                            j.questions.clear();
                        });
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        if live.halt.load(Ordering::Relaxed) {
                            break;
                        }
                    }
                    if live.halt.load(Ordering::Relaxed) {
                        let cancelled = live.ctl.cancel.load(Ordering::Relaxed);
                        let job = self.update(id, |j| {
                            j.state = if cancelled { State::Cancelled } else { State::Paused };
                            j.finished = cancelled.then(now);
                        });
                        if let (true, Some(j)) = (cancelled, job) {
                            self.update(id, |j| j.partials.clear());
                            self.remove_partials(j);
                        }
                        return;
                    }
                    resumed = true;
                    continue;
                }
                Err(e) => {
                    tracing::info!(job = %id, kind = ?e.kind, "transfer failed");
                    self.update(id, |j| {
                        j.state = State::Failed;
                        j.error = Some(e.message.clone());
                        j.finished = Some(now());
                        j.questions.clear();
                        j.speed_bps = 0;
                        j.eta_s = None;
                    });
                }
            }
            return;
        }
    }

    async fn attempt(self: &Arc<Self>, id: &str, live: &Arc<Live>, resumed: bool) -> FsResult<Outcome> {
        let job = self.get(id).ok_or_else(|| FsError::new(ErrKind::Other, "the transfer is gone"))?;
        let src = self.end(&job.src).await?;
        match job.kind {
            JobKind::Copy => {
                let dst = self.end(&job.dst).await?;
                self.copy_job(&job, src, dst, live, resumed).await
            }
            JobKind::Delete | JobKind::Chmod => self.tree_job(&job, src, live).await,
        }
    }

    fn halted(live: &Live) -> Option<Outcome> {
        if live.ctl.cancel.load(Ordering::Relaxed) {
            Some(Outcome::Cancelled)
        } else if live.ctl.pause.load(Ordering::Relaxed) {
            Some(Outcome::Paused)
        } else {
            None
        }
    }

    async fn copy_job(self: &Arc<Self>, job: &Job, src: Arc<dyn RemoteFs>, dst: Arc<dyn RemoteFs>, live: &Arc<Live>, resumed: bool) -> FsResult<Outcome> {
        let free = dst.free_space(&job.dst_dir).await.ok().flatten();
        let (tx, mut rx) = mpsc::channel::<Found>(256);
        let walker = {
            let (src, items, live) = (src.clone(), job.items.clone(), live.clone());
            async move {
                let r = walk(src.as_ref(), &items, &tx, &live.halt, Links::Follow).await;
                drop(tx);
                r
            }
        };
        let consumer = async {
            // Target folder for each source folder (relative parts); `None`: skipped, with everything below.
            let mut dirs: HashMap<Vec<String>, Option<String>> = HashMap::new();
            dirs.insert(Vec::new(), Some(job.dst_dir.clone()));
            let files = Arc::new(Semaphore::new(FILES_AT_ONCE as usize));
            let mut tasks = tokio::task::JoinSet::new();
            let mut first_error: Option<FsError> = None;
            let mut started_running = false;
            while let Some(found) = rx.recv().await {
                if first_error.is_some() || live.halt.load(Ordering::Relaxed) {
                    continue; // drain so the walk can finish
                }
                match found {
                    Found::Dir { rel, path } => {
                        let parent = dirs.get(&rel[..rel.len() - 1]).cloned().flatten();
                        let target = match parent {
                            None => None,
                            Some(parent) => match self.target_dir(job, &src, &dst, live, &path, &parent, rel.last().unwrap()).await {
                                Ok(t) => t,
                                Err(e) if e.kind == ErrKind::Connection => {
                                    first_error = Some(e);
                                    None
                                }
                                Err(e) => {
                                    self.failed(&job.id, &path, &e);
                                    None
                                }
                            },
                        };
                        if target.is_none() {
                            self.count(&job.id, |j| j.totals.skipped += 1);
                        }
                        dirs.insert(rel, target);
                    }
                    Found::File { path, rel, meta } => {
                        let Some(dir) = dirs.get(&rel[..rel.len() - 1]).cloned().flatten() else {
                            self.count(&job.id, |j| j.totals.skipped += 1);
                            continue;
                        };
                        if !started_running {
                            started_running = true;
                            self.update(&job.id, |j| j.state = State::Running);
                        }
                        let over = {
                            let mut jobs = self.jobs.lock();
                            jobs.iter_mut().find(|j| j.id == job.id).map(|j| {
                                j.totals.files += 1;
                                j.totals.bytes += meta.size;
                                free.is_some_and(|f| j.totals.bytes.saturating_sub(live.ctl.bytes.load(Ordering::Relaxed)) > f)
                            })
                        };
                        if over == Some(true) {
                            first_error = Some(FsError::new(ErrKind::Other, format!("not enough free space on the target ({} free)", free.unwrap_or(0))));
                            live.halt.store(true, Ordering::Relaxed);
                            continue;
                        }
                        let permits = if meta.size >= BIG_FILE { FILES_AT_ONCE } else { 1 };
                        let Ok(permit) = files.clone().acquire_many_owned(permits).await else { break };
                        let (me, live, src, dst, job, name) = (self.clone(), live.clone(), src.clone(), dst.clone(), job.clone(), rel.last().unwrap().clone());
                        tasks.spawn(async move {
                            let r = me.copy_one(&job, &live, src.as_ref(), dst.as_ref(), &path, &meta, &dir, &name, resumed).await;
                            drop(permit);
                            r
                        });
                    }
                    Found::Link { .. } => {}
                }
                while let Some(done) = tasks.try_join_next() {
                    if let Ok(Err(e)) = done {
                        first_error.get_or_insert(e);
                        live.halt.store(true, Ordering::Relaxed);
                    }
                }
            }
            while let Some(done) = tasks.join_next().await {
                if let Ok(Err(e)) = done {
                    first_error.get_or_insert(e);
                }
            }
            first_error
        };
        let (walked, failed) = tokio::join!(walker, consumer);
        let skipped = walked?;
        self.update(&job.id, |j| {
            j.totals.skipped += skipped.folder_links + skipped.unreadable_names + skipped.broken_links + skipped.special;
            j.walk = skipped;
            j.current = None;
        });
        if let Some(stop) = Self::halted(live) {
            return Ok(stop);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(Outcome::Done),
        }
    }

    /// The target folder for a source folder: made when absent, merged when a folder, and a file in the way
    /// follows the policy (never replaced).
    #[allow(clippy::too_many_arguments)]
    async fn target_dir(&self, job: &Job, src: &Arc<dyn RemoteFs>, dst: &Arc<dyn RemoteFs>, live: &Arc<Live>, src_path: &str, parent: &str, name: &str) -> FsResult<Option<String>> {
        let name = self.mapped_name(job, dst.as_ref(), name);
        let path = dst.join(parent, &name);
        match dst.lstat(&path).await {
            Err(e) if e.kind == ErrKind::NotFound => {
                dst.mkdir(&path).await?;
                Ok(Some(path))
            }
            Err(e) => Err(e),
            Ok(m) if m.kind == FsKind::Dir => Ok(Some(path)),
            Ok(m) => {
                let src_meta = src.stat(src_path).await?;
                let policy = self.policy(&job.id);
                let mut decision = decide(policy, &src_meta, Some(&m));
                if let Decision::Ask { .. } = decision {
                    let choice = self.ask(&job.id, live, Question { id: 0, path: path.clone(), src: src_meta, dst: m, identical: false, kind_clash: true, changed: false }).await;
                    decision = match choice {
                        Some(Choice::KeepBoth | Choice::SaveCopy) => Decision::KeepBoth,
                        _ => Decision::Skip,
                    };
                }
                if decision == Decision::KeepBoth {
                    let other = self.free_name(dst.as_ref(), parent, &name).await?;
                    let path = dst.join(parent, &other);
                    dst.mkdir(&path).await?;
                    return Ok(Some(path));
                }
                Ok(None)
            }
        }
    }

    fn policy(&self, id: &str) -> Policy {
        self.jobs.lock().iter().find(|j| j.id == id).map_or(Policy::Ask, |j| j.conflict)
    }

    /// A name the target can hold (Windows), reported when it had to change.
    fn mapped_name(&self, job: &Job, dst: &dyn RemoteFs, name: &str) -> String {
        if !dst.windows_names() {
            return name.to_string();
        }
        let (safe, changed) = windows_safe(name);
        if changed {
            let wanted = name.to_string();
            let saved = safe.clone();
            self.count(&job.id, move |j| {
                if j.renamed.len() < KEEP_RENAMED {
                    j.renamed.push((wanted, saved));
                }
            });
        }
        safe
    }

    /// The first `name (n)` free in `dir`.
    async fn free_name(&self, dst: &dyn RemoteFs, dir: &str, name: &str) -> FsResult<String> {
        let mut taken = HashSet::new();
        loop {
            let candidate = keep_both(name, |c| taken.contains(c));
            match dst.lstat(&dst.join(dir, &candidate)).await {
                Err(e) if e.kind == ErrKind::NotFound => return Ok(candidate),
                Err(e) => return Err(e),
                Ok(_) => {
                    taken.insert(candidate);
                }
            }
        }
    }

    /// Puts a question on the job and waits for the answer; `None` when the job is paused or cancelled meanwhile.
    async fn ask(&self, id: &str, live: &Arc<Live>, mut q: Question) -> Option<Choice> {
        // A policy set by "apply to all" while this file was on its way answers it.
        let qid = live.next_q.fetch_add(1, Ordering::Relaxed) + 1;
        q.id = qid;
        let (tx, rx) = oneshot::channel();
        live.answers.lock().insert(qid, tx);
        let question = q.clone();
        self.update(id, |j| {
            j.questions.push(question);
            j.state = State::Waiting;
        });
        let answer = rx.await.ok();
        self.update(id, |j| {
            j.questions.retain(|x| x.id != qid);
            if j.questions.is_empty() && j.state == State::Waiting {
                j.state = State::Running;
            }
        });
        answer.map(|(c, _)| c)
    }

    fn failed(&self, id: &str, path: &str, e: &FsError) {
        let (path, error) = (path.to_string(), e.message.clone());
        self.count(id, move |j| {
            j.totals.failed += 1;
            if j.failures.len() < KEEP_FAILURES {
                j.failures.push(Failure { path, error });
            }
        });
    }

    /// One file of a copy job. Errors that are not a lost connection are counted on the job, not returned.
    #[allow(clippy::too_many_arguments)]
    async fn copy_one(&self, job: &Job, live: &Arc<Live>, src: &dyn RemoteFs, dst: &dyn RemoteFs, path: &str, meta: &Meta, dir: &str, name: &str, resumed: bool) -> FsResult<()> {
        let mut name = self.mapped_name(job, dst, name);
        self.count(&job.id, |j| j.current = Some(path.to_string()));
        let mut approved_override: Option<Option<(u64, u64)>> = None;
        let mut changed = false;
        loop {
            if live.halt.load(Ordering::Relaxed) {
                return Ok(());
            }
            let target = dst.join(dir, &name);
            let existing = match dst.lstat(&target).await {
                Ok(m) => Some(m),
                Err(e) if e.kind == ErrKind::NotFound => None,
                Err(e) if e.kind == ErrKind::Connection => return Err(e),
                Err(e) => {
                    self.failed(&job.id, path, &e);
                    return Ok(());
                }
            };
            // After a reconnect or a resume: a file already in place with the source's size and time is complete.
            if resumed && approved_override.is_none() && existing.as_ref().is_some_and(|m| m.kind == FsKind::File && m.size == meta.size && m.mtime == meta.mtime) {
                live.ctl.bytes.fetch_add(meta.size, Ordering::Relaxed);
                self.count(&job.id, |j| j.totals.done_files += 1);
                return Ok(());
            }
            // Two names that differ only in case, both from this job, on a target that ignores case.
            let lower = target.to_lowercase();
            let same_job_clash = dst.case_insensitive() && existing.is_some() && live.written.lock().contains(&lower);
            let decision = if same_job_clash {
                Decision::KeepBoth
            } else if let Some(approved) = approved_override.take() {
                Decision::Copy { approved }
            } else if let Some(e) = job.expect {
                match &existing {
                    None => Decision::Copy { approved: None },
                    Some(m) if m.size == e.size && m.mtime == e.mtime => Decision::Copy { approved: Some((e.size, e.mtime)) },
                    Some(_) => Decision::Ask { identical: false, kind_clash: false },
                }
            } else {
                decide(self.policy(&job.id), meta, existing.as_ref())
            };
            let decision = match decision {
                Decision::Ask { identical, kind_clash } => {
                    let dst_meta = existing.clone().unwrap_or(Meta { kind: FsKind::File, size: 0, mtime: 0, mode: None, uid: None, gid: None });
                    let changed_now = changed || job.expect.is_some();
                    let Some(choice) = self.ask(&job.id, live, Question { id: 0, path: target.clone(), src: meta.clone(), dst: dst_meta.clone(), identical, kind_clash, changed: changed_now }).await else {
                        return Ok(());
                    };
                    match choice {
                        Choice::Replace | Choice::Overwrite if !kind_clash => Decision::Copy { approved: Some((dst_meta.size, dst_meta.mtime)) },
                        Choice::Newer if !kind_clash => decide(Policy::Newer, meta, Some(&dst_meta)),
                        Choice::SkipIdentical if identical => Decision::Skip,
                        Choice::SaveCopy => {
                            // `name (edited).ext`, numbered only when that is taken too.
                            let wanted = with_suffix(&name, " (edited)");
                            name = match dst.lstat(&dst.join(dir, &wanted)).await {
                                Err(e) if e.kind == ErrKind::NotFound => wanted,
                                Err(e) if e.kind == ErrKind::Connection => return Err(e),
                                _ => self.free_name(dst, dir, &wanted).await?,
                            };
                            approved_override = Some(None);
                            changed = false;
                            continue;
                        }
                        Choice::KeepBoth => Decision::KeepBoth,
                        _ => Decision::Skip,
                    }
                }
                d => d,
            };
            match decision {
                Decision::Skip | Decision::Merge => {
                    live.ctl.bytes.fetch_add(meta.size, Ordering::Relaxed);
                    self.count(&job.id, |j| j.totals.skipped += 1);
                    return Ok(());
                }
                Decision::KeepBoth => {
                    name = self.free_name(dst, dir, &name).await?;
                    approved_override = Some(None);
                    continue;
                }
                Decision::Ask { .. } => unreachable!("answered above"),
                Decision::Copy { approved } => {
                    let partial = dst.join(dir, &partial_name(&name));
                    let seen = self.get(&job.id).and_then(|j| j.partials.get(&partial).copied());
                    let resume = resumed && seen.is_some();
                    if seen != Some((meta.size, meta.mtime)) {
                        // Saved with the next journal write (within a second). A partial whose entry was lost to a
                        // crash is not the job's, so it is started again rather than resumed: safe either way.
                        let (p, s) = (partial.clone(), (meta.size, meta.mtime));
                        self.count(&job.id, move |j| {
                            j.partials.insert(p, s);
                        });
                    }
                    let fc = FileCopy { src, src_path: path, dst, dst_dir: dir, dst_name: &name, approved, resume, source_seen: seen, preserve: job.preserve };
                    match copy_file(&fc, &live.ctl).await {
                        Ok(Copied::Done { warning, .. }) => {
                            self.count(&job.id, |j| {
                                j.partials.remove(&partial);
                                j.totals.done_files += 1;
                            });
                            if let Some(w) = warning {
                                self.failed(&job.id, &target, &FsError::new(ErrKind::Other, w));
                            }
                            if dst.case_insensitive() {
                                live.written.lock().insert(lower);
                            }
                            return Ok(());
                        }
                        Ok(Copied::TargetChanged { now }) => {
                            // Asked again with what is there now; the partial stays for the answer.
                            changed = true;
                            let _ = now;
                            continue;
                        }
                        Ok(Copied::Stopped(Stop::Pause)) => return Ok(()),
                        Ok(Copied::Stopped(Stop::Cancel)) => {
                            self.count(&job.id, |j| {
                                j.partials.remove(&partial);
                            });
                            return Ok(());
                        }
                        Err(e) if e.kind == ErrKind::Connection => return Err(e),
                        Err(e) => {
                            self.failed(&job.id, path, &e);
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    /// A delete or chmod job on a host: files as they are found, folders at the end (deepest first, so a delete
    /// empties them first and a chmod never locks the walk out).
    async fn tree_job(self: &Arc<Self>, job: &Job, fs: Arc<dyn RemoteFs>, live: &Arc<Live>) -> FsResult<Outcome> {
        let (tx, mut rx) = mpsc::channel::<Found>(256);
        let walker = {
            let (fs, items, live) = (fs.clone(), job.items.clone(), live.clone());
            async move {
                let r = walk(fs.as_ref(), &items, &tx, &live.halt, Links::Report).await;
                drop(tx);
                r
            }
        };
        let (file_mode, dir_mode) = chmod_modes(job.mode.unwrap_or(0), job.dirs_only_x);
        let consumer = async {
            let mut dirs: Vec<(usize, String)> = Vec::new();
            let mut first_error = None;
            let mut running = false;
            while let Some(found) = rx.recv().await {
                if first_error.is_some() || live.halt.load(Ordering::Relaxed) {
                    continue;
                }
                if !running {
                    running = true;
                    self.update(&job.id, |j| j.state = State::Running);
                }
                let (path, result) = match found {
                    Found::Dir { path, rel } => {
                        dirs.push((rel.len(), path));
                        continue;
                    }
                    Found::File { path, .. } => {
                        self.count(&job.id, |j| j.totals.files += 1);
                        let r = if job.kind == JobKind::Delete { fs.remove_file(&path).await } else { fs.chmod(&path, file_mode).await };
                        (path, r)
                    }
                    Found::Link { path, .. } => {
                        if job.kind == JobKind::Chmod {
                            // Setting a mode on a link would change its target, which may be outside the folder.
                            self.count(&job.id, |j| j.totals.skipped += 1);
                            continue;
                        }
                        self.count(&job.id, |j| j.totals.files += 1);
                        let r = fs.remove_file(&path).await;
                        (path, r)
                    }
                };
                match result {
                    Ok(()) => self.count(&job.id, |j| j.totals.done_files += 1),
                    Err(e) if e.kind == ErrKind::Connection => first_error = Some(e),
                    Err(e) => self.failed(&job.id, &path, &e),
                }
            }
            if first_error.is_none() && !live.halt.load(Ordering::Relaxed) {
                dirs.sort_by(|a, b| b.0.cmp(&a.0));
                for (_, path) in dirs {
                    if live.halt.load(Ordering::Relaxed) {
                        break;
                    }
                    self.count(&job.id, |j| j.totals.files += 1);
                    let r = if job.kind == JobKind::Delete { fs.remove_dir(&path).await } else { fs.chmod(&path, dir_mode).await };
                    match r {
                        Ok(()) => self.count(&job.id, |j| j.totals.done_files += 1),
                        Err(e) if e.kind == ErrKind::Connection => {
                            first_error = Some(e);
                            break;
                        }
                        Err(e) => self.failed(&job.id, &path, &e),
                    }
                }
            }
            first_error
        };
        let (walked, failed) = tokio::join!(walker, consumer);
        let skipped = walked?;
        self.update(&job.id, |j| j.walk = skipped);
        if let Some(stop) = Self::halted(live) {
            return Ok(stop);
        }
        match failed {
            Some(e) => Err(e),
            None => Ok(Outcome::Done),
        }
    }
}

/// The bits a recursive chmod sets on files and on folders. With `dirs_only_x` files lose x and folders get x
/// wherever they have r, so they stay searchable.
pub fn chmod_modes(mode: u32, dirs_only_x: bool) -> (u32, u32) {
    let mode = mode & 0o7777;
    if dirs_only_x {
        (mode & !0o111, mode | ((mode & 0o444) >> 2))
    } else {
        (mode, mode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chmod_bits() {
        assert_eq!(chmod_modes(0o755, false), (0o755, 0o755));
        assert_eq!(chmod_modes(0o644, true), (0o644, 0o755));
        assert_eq!(chmod_modes(0o750, true), (0o640, 0o750));
        assert_eq!(chmod_modes(0o100640, false), (0o640, 0o640));
    }

    #[test]
    fn choices_set_the_policy_for_the_rest() {
        assert_eq!(Choice::Replace.policy(), Policy::Replace);
        assert_eq!(Choice::Discard.policy(), Policy::Skip);
        assert_eq!(Choice::SaveCopy.policy(), Policy::KeepBoth);
    }

    use crate::files::mem_fs::MemFs;

    struct Rig {
        q: Arc<Queue>,
        a: MemFs,
        b: MemFs,
        events: Arc<Mutex<Vec<(String, Value)>>>,
        dir: PathBuf,
    }

    fn rig(name: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("kelpie-queue-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let events: Arc<Mutex<Vec<(String, Value)>>> = Default::default();
        let sink = events.clone();
        let emit: Emit = Arc::new(move |e: &str, p: Value| sink.lock().push((e.to_string(), p)));
        let q = Queue::new(dir.clone(), Arc::new(Pool::new(PathBuf::from("kelpied"))), emit);
        let (a, b) = (MemFs::new(), MemFs::new());
        q.test_ends.lock().insert("a".into(), Arc::new(a.clone()));
        q.test_ends.lock().insert("b".into(), Arc::new(b.clone()));
        Rig { q, a, b, events, dir }
    }

    async fn start(r: &Rig, p: Value) -> String {
        r.q.rpc("xfer.start", &p).await.unwrap()["id"].as_str().unwrap().to_string()
    }

    async fn until(r: &Rig, id: &str, what: impl Fn(&Job) -> bool) -> Job {
        for _ in 0..400 {
            if let Some(j) = r.q.get(id).filter(|j| what(j)) {
                return j;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("timed out: {:?}", r.q.get(id));
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 253) as u8).collect()
    }

    #[tokio::test]
    async fn copies_a_folder_between_two_hosts() {
        let r = rig("copy");
        r.a.put("/src/p/one.txt", b"1", 10);
        r.a.put("/src/p/sub/two.bin", &data(3 << 20), 20);
        r.b.mkdir_all("/dst");
        let id = start(&r, json!({"kind": "copy", "src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/p"], "dst_dir": "/dst", "conflict": "ask"})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Done, "{j:?}");
        assert_eq!((j.totals.files, j.totals.done_files, j.totals.failed), (2, 2, 0));
        assert_eq!(r.b.read("/dst/p/sub/two.bin").unwrap(), data(3 << 20));
        assert_eq!(r.b.stat("/dst/p/one.txt").await.unwrap().mtime, 10, "times are kept");
        assert!(r.b.paths().iter().all(|p| !p.contains("kelpie-part")));
        assert!(r.events.lock().iter().any(|(e, _)| e == "xfer.updated"));
        let saved: Value = serde_json::from_slice(&std::fs::read(r.dir.join("transfers.json")).unwrap()).unwrap();
        assert_eq!(saved["jobs"][0]["state"], "done");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_conflict_waits_for_the_answer_and_keeps_both() {
        let r = rig("ask");
        r.a.put("/src/x.txt", b"new", 10);
        r.b.put("/dst/x.txt", b"old", 5);
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/x.txt"], "dst_dir": "/dst"})).await;
        let j = until(&r, &id, |j| !j.questions.is_empty()).await;
        assert_eq!(j.state, State::Waiting);
        assert_eq!(j.questions[0].path, "/dst/x.txt");
        r.q.rpc("xfer.answer", &json!({"id": id, "choice": "keep_both"})).await.unwrap();
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Done);
        assert_eq!(r.b.read("/dst/x.txt").unwrap(), b"old", "never overwritten unasked");
        assert_eq!(r.b.read("/dst/x (2).txt").unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn apply_to_all_answers_the_rest_of_the_job_only() {
        let r = rig("all");
        for n in ["a", "b", "c"] {
            r.a.put(&format!("/src/d/{n}"), b"new", 10);
            r.b.put(&format!("/dst/d/{n}"), b"old", 5);
        }
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/d"], "dst_dir": "/dst"})).await;
        until(&r, &id, |j| !j.questions.is_empty()).await;
        r.q.rpc("xfer.answer", &json!({"id": id, "choice": "replace", "all": true})).await.unwrap();
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!((j.state, j.conflict), (State::Done, Policy::Replace));
        for n in ["a", "b", "c"] {
            assert_eq!(r.b.read(&format!("/dst/d/{n}")).unwrap(), b"new", "{n}");
        }
        // A new job starts with its own policy again.
        r.a.put("/src/e", b"new", 10);
        r.b.put("/dst/e", b"old", 5);
        let id2 = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/e"], "dst_dir": "/dst"})).await;
        until(&r, &id2, |j| !j.questions.is_empty()).await;
        r.q.rpc("xfer.control", &json!({"id": id2, "action": "cancel"})).await.unwrap();
        until(&r, &id2, |j| j.state == State::Cancelled).await;
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_dropped_connection_is_retried_and_resumes_from_the_partial() {
        let r = rig("retry");
        let all = data(2 << 20);
        r.a.put("/src/big.bin", &all, 10);
        r.b.mkdir_all("/dst");
        r.a.fail_reads_after(Some(1 << 20));
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/big.bin"], "dst_dir": "/dst", "conflict": "replace"})).await;
        let j = until(&r, &id, |j| j.state == State::Reconnecting).await;
        assert_eq!(j.attempt, 1);
        assert!(r.b.exists("/dst/.big.bin.kelpie-part"), "kept for the resume");
        r.a.fail_reads_after(None);
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Done, "{j:?}");
        assert_eq!(r.b.read("/dst/big.bin").unwrap(), all);
        assert!(j.partials.is_empty());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn cancel_removes_the_partial() {
        let r = rig("cancel");
        r.a.put("/src/big.bin", &data(1 << 20), 10);
        r.b.mkdir_all("/dst");
        r.a.fail_reads_after(Some(300_000));
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/big.bin"], "dst_dir": "/dst"})).await;
        until(&r, &id, |j| j.state == State::Reconnecting).await;
        r.q.rpc("xfer.control", &json!({"id": id, "action": "cancel"})).await.unwrap();
        until(&r, &id, |j| j.state == State::Cancelled).await;
        for _ in 0..100 {
            if !r.b.exists("/dst/.big.bin.kelpie-part") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!r.b.exists("/dst/.big.bin.kelpie-part"));
        assert!(!r.b.exists("/dst/big.bin"));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn deletes_a_folder_children_first_and_never_follows_links() {
        let r = rig("delete");
        r.a.put("/srv/old/a.txt", b"a", 1);
        r.a.put("/srv/old/sub/b.txt", b"b", 1);
        r.a.put("/keep/k.txt", b"k", 1);
        r.a.link("/srv/old/to-keep", "/keep");
        let id = start(&r, json!({"kind": "delete", "src": {"dest": "a"}, "items": ["/srv/old"]})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Done, "{j:?}");
        assert!(!r.a.exists("/srv/old"));
        assert_eq!(r.a.read("/keep/k.txt").unwrap(), b"k", "a link's target is never touched");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_restart_loads_unfinished_jobs_as_interrupted_and_a_bad_journal_is_kept() {
        let r = rig("restart");
        r.a.put("/src/x", b"x", 1);
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/src/x"], "dst_dir": "/nowhere", "conflict": "replace"})).await;
        until(&r, &id, |j| j.state.finished()).await;
        // As if the daemon stopped while it ran.
        let mut saved: Value = serde_json::from_slice(&std::fs::read(r.dir.join("transfers.json")).unwrap()).unwrap();
        saved["jobs"][0]["state"] = json!("running");
        std::fs::write(r.dir.join("transfers.json"), serde_json::to_vec(&saved).unwrap()).unwrap();
        let again = Queue::new(r.dir.clone(), Arc::new(Pool::new(PathBuf::from("kelpied"))), Arc::new(|_: &str, _: Value| {}));
        assert_eq!(again.get(&id).unwrap().state, State::Interrupted);
        std::fs::write(r.dir.join("transfers.json"), b"{broken").unwrap();
        let fresh = Queue::new(r.dir.clone(), Arc::new(Pool::new(PathBuf::from("kelpied"))), Arc::new(|_: &str, _: Value| {}));
        assert!(fresh.jobs.lock().is_empty());
        assert!(fresh.notice.lock().is_some());
        assert!(std::fs::read_dir(&r.dir).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("damaged")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn refuses_jobs_it_cannot_run() {
        let r = rig("refuse");
        for bad in [
            json!({"src": {}, "dst": {}, "items": ["/x"], "dst_dir": "/y"}),
            json!({"src": {"dest": "a"}, "dst": {}, "items": [], "dst_dir": "/y"}),
            json!({"src": {"dest": "-oProxyCommand=x"}, "dst": {}, "items": ["/x"], "dst_dir": "C:\\y"}),
            json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["relative"], "dst_dir": "/y"}),
            json!({"kind": "delete", "src": {}, "items": ["C:\\x"]}),
            json!({"kind": "chmod", "src": {"dest": "a"}, "items": ["/x"]}),
        ] {
            assert!(r.q.rpc("xfer.start", &bad).await.is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn states_that_load_as_interrupted() {
        for s in [State::Queued, State::Scanning, State::Running, State::Waiting, State::Reconnecting] {
            assert!(s.active(), "{s:?}");
        }
        for s in [State::Done, State::Cancelled, State::Failed] {
            assert!(s.finished() && !s.active(), "{s:?}");
        }
        assert!(!State::Paused.active() && !State::Interrupted.active());
    }
}
