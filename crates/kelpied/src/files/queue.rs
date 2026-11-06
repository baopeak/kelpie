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

use super::copy::{copy_file, decide, finish_partial, recover_aside, resume_offset, Control, Copied, Decision, FileCopy, Policy, Stop};
use super::journal::{self, Journal, Loaded};
use super::local_fs::LocalFs;
use super::names::{keep_both, partial_name, with_suffix, windows_safe};
use super::pool::Pool;
use super::route::{self, Target};
use super::sftp_fs::SftpFs;
use super::walk::{walk, Found, Links, Skipped};
use super::{check_local_path, check_remote_path, ErrKind, FsError, FsResult, Kind as FsKind, Meta, RemoteFs};

pub use super::Emit;

pub const MAX_RUNNING: usize = 3;
const FILES_AT_ONCE: u32 = 4;
const BIG_FILE: u64 = 8 << 20;
const BACKOFF: [u64; 5] = [2, 5, 15, 30, 60];
/// How long after a connection is lost the job keeps trying (from the drop, not from the job's start).
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

/// How a host-to-host copy travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Direct when the first host can log in to the second by itself, else through this computer.
    #[default]
    Auto,
    /// Always through this computer.
    Relay,
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
    /// chmod: the bits the person changed (only these take `mode`'s value); none: all of them.
    #[serde(default)]
    pub mask: Option<u32>,
    #[serde(default)]
    pub expect: Option<Expect>,
    /// Host to host: how it should travel, how it did (`direct` / `relay`), and why when it did not go direct.
    #[serde(default)]
    pub route: Route,
    #[serde(default)]
    pub route_used: Option<String>,
    #[serde(default)]
    pub route_reason: Option<String>,
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
    /// Set on pause, cancel or a failure that ends the attempt: stops the walk too.
    halt: AtomicBool,
    answers: Mutex<HashMap<u64, oneshot::Sender<(Choice, bool)>>>,
    next_q: AtomicU64,
    /// Targets this job has put in place (keys as in `target_key`): a second file for one of them is "keep both".
    written: Mutex<HashSet<String>>,
    speed: Mutex<Speed>,
    /// Bytes of the files found, and of those that need no space (skipped, complete, resumed).
    discovered: AtomicU64,
    not_needed: AtomicU64,
    /// Direct copiers an attempt left on the first host (it could not be told to stop): stopped before the next starts.
    stale_pids: Mutex<Vec<u32>>,
}

/// A job's copier on the first host, shared by its direct files (one at a time).
type DirectSession = tokio::sync::Mutex<Option<route::Session>>;

impl Live {
    /// Ends the attempt: the walk stops, and files waiting on a question are released (their question is dropped).
    fn halt(&self) {
        self.halt.store(true, Ordering::Relaxed);
        self.answers.lock().clear();
        self.ctl.wake.notify_waiters();
    }
}

#[derive(Default)]
struct Speed {
    at: Option<Instant>,
    bytes: u64,
    ema: f64,
    emitted: Option<Instant>,
    emitted_bytes: u64,
}

/// When to try again after a lost connection: the 5 minutes run from the first drop, and an attempt that wrote
/// something starts the count again.
#[derive(Debug, Default)]
pub struct Retry {
    first_drop: Option<Instant>,
    pub attempt: u32,
}

impl Retry {
    /// Seconds to wait before the next attempt, or `None` to give up.
    pub fn on_drop(&mut self, now: Instant, progressed: bool) -> Option<u64> {
        if progressed {
            self.first_drop = None;
            self.attempt = 0;
        }
        let first = *self.first_drop.get_or_insert(now);
        if now.duration_since(first) >= RETRY_FOR {
            return None;
        }
        self.attempt += 1;
        Some(BACKOFF[(self.attempt as usize - 1).min(BACKOFF.len() - 1)])
    }
}

/// Whether the files found so far need more room than the target had when the job started.
pub fn over_space(discovered: u64, not_needed: u64, free: Option<u64>) -> bool {
    free.is_some_and(|f| discovered.saturating_sub(not_needed) > f)
}

/// A job's tag in its hidden file names (8 characters of its id): no two jobs share a partial.
pub fn tag(id: &str) -> String {
    id.chars().filter(char::is_ascii_alphanumeric).take(8).collect()
}

/// The key a target is claimed by across the queue: its end and its path (lower-cased where the end ignores case).
fn target_key(end: &End, dst: &dyn RemoteFs, target: &str) -> String {
    let path = if dst.case_insensitive() { target.to_lowercase() } else { target.to_string() };
    format!("{}\u{0}{path}", end.dest.as_deref().unwrap_or(""))
}

/// A target being written: no other file of any job writes it at the same time.
struct Claim<'a> {
    set: &'a Mutex<HashSet<String>>,
    key: String,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        self.set.lock().remove(&self.key);
    }
}

pub struct Queue {
    file: PathBuf,
    pool: Arc<Pool>,
    emit: Emit,
    jobs: Mutex<Vec<Job>>,
    live: Mutex<HashMap<String, Arc<Live>>>,
    slots: Arc<Semaphore>,
    claims: Mutex<HashSet<String>>,
    /// Whether a first host can copy to a second by itself (cached).
    probes: route::Probes,
    /// Off when the journal exists but could not be read: nothing is written over it.
    can_save: AtomicBool,
    notice: Mutex<Option<String>>,
    dirty: AtomicBool,
    /// Journal writes in order: each snapshot has a generation, and an older one never lands after a newer one.
    generation: AtomicU64,
    saved: Arc<Mutex<u64>>,
    /// Cleanups of hosts' partials that wait for the runtime (jobs pruned at start).
    pending_cleanup: Mutex<Vec<Job>>,
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

/// Picked items Kelpie can name: no `.` or `..` parts, not the root, a readable last part.
fn check_item(end: &End, path: &str) -> Result<()> {
    match end.dest {
        Some(_) => check_remote_path(path),
        None => check_local_path(path),
    }
    .map_err(|e| anyhow!("{e}"))?;
    let seps: &[char] = if end.dest.is_none() && cfg!(windows) { &['/', '\\'] } else { &['/'] };
    let parts: Vec<&str> = path.split(seps).filter(|p| !p.is_empty()).collect();
    if parts.iter().any(|p| *p == "." || *p == "..") {
        bail!("{path}: a path with . or .. is not transferred");
    }
    match parts.last() {
        Some(last) if super::readable_name(last) && !(end.dest.is_none() && cfg!(windows) && parts.len() == 1 && last.ends_with(':')) => Ok(()),
        _ => bail!("{path}: a whole drive or the root folder is not transferred"),
    }
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
            claims: Mutex::new(HashSet::new()),
            probes: route::Probes::default(),
            can_save: AtomicBool::new(can_save),
            notice: Mutex::new(notice),
            dirty: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            saved: Arc::new(Mutex::new(0)),
            pending_cleanup: Mutex::new(Vec::new()),
            ticker: std::sync::Once::new(),
            #[cfg(test)]
            test_ends: Mutex::new(HashMap::new()),
        });
        q.prune_at_start();
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

    /// Writes the journal off the async workers (synchronously when there is no runtime: start, shutdown).
    fn save(&self) {
        if !self.can_save.load(Ordering::Relaxed) {
            return;
        }
        self.dirty.store(false, Ordering::Relaxed);
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let snapshot = Journal { version: journal::VERSION, jobs: self.jobs.lock().clone(), extra: Default::default() };
        let (file, saved) = (self.file.clone(), self.saved.clone());
        let write = move || {
            let mut last = saved.lock();
            if generation > *last {
                if let Err(e) = journal::save(&file, &snapshot) {
                    tracing::warn!("transfers journal: {e:#}");
                }
                *last = generation;
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => drop(rt.spawn_blocking(write)),
            Err(_) => write(),
        }
    }

    /// Finished jobs beyond 7 days or 200 go, with their partials: this computer's now, a host's when the runtime runs.
    fn prune_at_start(self: &Arc<Self>) {
        let removed = self.prune();
        let (local, remote): (Vec<Job>, Vec<Job>) = removed.into_iter().partition(|j| j.dst.dest.is_none());
        for j in &local {
            for p in j.partials.keys() {
                let _ = std::fs::remove_file(p);
            }
        }
        self.pending_cleanup.lock().extend(remote);
    }

    fn prune(&self) -> Vec<Job> {
        let mut jobs = self.jobs.lock();
        let before: Vec<Job> = jobs.clone();
        journal::prune(&mut jobs, now(), |j| j.state.finished().then_some(j.finished.unwrap_or(j.updated)));
        before.into_iter().filter(|b| !jobs.iter().any(|j| j.id == b.id)).collect()
    }

    /// Removes the partials of jobs that are gone, one host after another, and puts back an old file a stopped
    /// replace left only under its moved-aside name.
    fn remove_partials(self: &Arc<Self>, jobs: Vec<Job>) {
        let jobs: Vec<Job> = jobs.into_iter().filter(|j| !j.partials.is_empty()).collect();
        if jobs.is_empty() {
            return;
        }
        let me = self.clone();
        let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
        rt.spawn(async move {
            let mut by_end: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
            for j in jobs {
                by_end.entry(j.dst.dest.clone()).or_default().extend(j.partials.into_keys());
            }
            for (dest, partials) in by_end {
                let Ok(fs) = me.end(&End { dest }).await else { continue };
                for p in partials {
                    let _ = fs.remove_file(&p).await;
                    restore_aside(fs.as_ref(), &p).await;
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
                for j in &removed {
                    (self.emit)("xfer.removed", json!({ "id": j.id }));
                }
                self.remove_partials(removed);
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
        for i in &items {
            check_item(&src, i)?;
        }
        let dst_dir = p.get("dst_dir").and_then(Value::as_str).unwrap_or_default().to_string();
        match kind {
            JobKind::Copy => {
                if src.dest.is_none() && dst.dest.is_none() {
                    bail!("copies on this computer are for Explorer or Finder");
                }
                match dst.dest {
                    Some(_) => check_remote_path(&dst_dir),
                    None => check_local_path(&dst_dir),
                }
                .map_err(|e| anyhow!("{e}"))?;
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
            mask: p.get("mask").and_then(Value::as_u64).map(|m| m as u32 & 0o7777),
            expect,
            route: serde_json::from_value(p.get("route").cloned().unwrap_or(json!("auto"))).map_err(|_| anyhow!("unknown route"))?,
            route_used: None,
            route_reason: None,
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
                live.halt();
            }
            "cancel" => match live {
                Some(live) => {
                    live.ctl.cancel.store(true, Ordering::Relaxed);
                    live.halt();
                }
                None if !job.state.finished() => {
                    self.update(id, |j| {
                        j.state = State::Cancelled;
                        j.finished = Some(now());
                        j.partials.clear();
                    });
                    self.remove_partials(vec![job]);
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
                self.remove_partials(vec![job]);
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

    /// Transfers running or waiting to run (not paused, interrupted or finished): a daemon hand-over (an update)
    /// waits for them (`daemon.shutdown` with `if_idle` or `if_no_transfers`).
    pub fn active_count(&self) -> usize {
        self.jobs.lock().iter().filter(|j| j.state.active()).count()
    }

    /// Tests: a job waiting to run, never started (no ssh is opened).
    #[cfg(test)]
    pub fn queue_for_test(&self) -> String {
        let dir = if cfg!(windows) { "C:\\" } else { "/" };
        let job = self.new_job(&json!({"src": {"dest": "host"}, "dst": {}, "items": ["/x"], "dst_dir": dir})).unwrap();
        let id = job.id.clone();
        self.jobs.lock().push(job);
        id
    }

    /// Before the daemon exits: what is running is saved as it is, and loads as interrupted with Resume. Only a hand-over
    /// the person asked for despite running transfers gets here with any (it waits for them otherwise).
    pub fn shutdown(&self) {
        for live in self.live.lock().values() {
            live.ctl.pause.store(true, Ordering::Relaxed);
            live.halt();
        }
        self.save();
    }

    // ---- progress ----

    fn start_ticker(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let pending: Vec<Job> = std::mem::take(&mut *self.pending_cleanup.lock());
        self.remove_partials(pending);
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
        let mut retry = Retry::default();
        loop {
            if let Some(stop) = Self::halted(&live) {
                return self.finish_stopped(id, stop);
            }
            self.update(id, |j| {
                j.state = State::Scanning;
                j.totals = Totals::default();
                j.walk = Skipped::default();
                j.failures.clear();
                j.renamed.clear();
                j.attempt = retry.attempt;
                j.next_s = 0;
            });
            live.ctl.bytes.store(0, Ordering::Relaxed);
            live.ctl.written.store(0, Ordering::Relaxed);
            live.discovered.store(0, Ordering::Relaxed);
            live.not_needed.store(0, Ordering::Relaxed);
            live.halt.store(false, Ordering::Relaxed);
            *live.speed.lock() = Speed::default();
            match self.attempt(id, &live, resumed).await {
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
                Ok(stop) => self.finish_stopped(id, stop),
                Err(e) if e.kind == ErrKind::Connection => {
                    let progressed = live.ctl.written.load(Ordering::Relaxed) > 0;
                    let Some(wait) = retry.on_drop(Instant::now(), progressed) else {
                        tracing::info!(job = %id, "transfer failed: the connection stayed lost");
                        self.fail(id, &e);
                        return;
                    };
                    tracing::info!(job = %id, attempt = retry.attempt, wait, "transfer connection lost; retrying");
                    for left in (1..=wait).rev() {
                        self.update(id, |j| {
                            j.state = State::Reconnecting;
                            j.attempt = retry.attempt;
                            j.next_s = left;
                            j.error = Some(e.message.clone());
                            j.speed_bps = 0;
                            j.eta_s = None;
                            j.questions.clear();
                        });
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                            _ = live.ctl.wake.notified() => {}
                        }
                        if Self::halted(&live).is_some() {
                            break;
                        }
                    }
                    resumed = true;
                    continue;
                }
                Err(e) => {
                    tracing::info!(job = %id, kind = ?e.kind, "transfer failed");
                    self.fail(id, &e);
                }
            }
            return;
        }
    }

    fn fail(&self, id: &str, e: &FsError) {
        self.update(id, |j| {
            j.state = State::Failed;
            j.error = Some(e.message.clone());
            j.finished = Some(now());
            j.questions.clear();
            j.speed_bps = 0;
            j.eta_s = None;
        });
    }

    /// Paused (partials kept for the resume) or cancelled (partials removed).
    fn finish_stopped(self: &Arc<Self>, id: &str, stop: Outcome) {
        let cancelled = matches!(stop, Outcome::Cancelled);
        let job = self.update(id, |j| {
            j.state = if cancelled { State::Cancelled } else { State::Paused };
            j.finished = cancelled.then(now);
            j.questions.clear();
            j.speed_bps = 0;
            j.eta_s = None;
        });
        if let (true, Some(j)) = (cancelled, job) {
            self.update(id, |j| j.partials.clear());
            self.remove_partials(vec![j]);
        }
    }

    async fn attempt(self: &Arc<Self>, id: &str, live: &Arc<Live>, resumed: bool) -> FsResult<Outcome> {
        let job = self.get(id).ok_or_else(|| FsError::new(ErrKind::Other, "the transfer is gone"))?;
        // Opening a session can take up to ssh's connect timeout: a pause or cancel does not wait for it.
        let open = |end: End| {
            let live = live.clone();
            async move {
                tokio::select! {
                    r = self.end(&end) => r.map(Some),
                    _ = live.ctl.wake.notified() => Ok(None),
                }
            }
        };
        let Some(src) = open(job.src.clone()).await? else { return Ok(Self::halted(live).unwrap_or(Outcome::Paused)) };
        match job.kind {
            JobKind::Copy => {
                let Some(dst) = open(job.dst.clone()).await? else { return Ok(Self::halted(live).unwrap_or(Outcome::Paused)) };
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
        // The target folder must be there: files must not all "skip" into a folder that is missing.
        match dst.stat(&job.dst_dir).await {
            Ok(m) if m.kind == FsKind::Dir => {}
            Ok(_) => return Err(FsError::invalid(format!("{} is not a folder", job.dst_dir))),
            Err(e) if e.kind == ErrKind::Connection => return Err(e),
            Err(e) => return Err(FsError::new(e.kind, format!("the target folder {}: {e}", job.dst_dir))),
        }
        let free = dst.free_space(&job.dst_dir).await.ok().flatten();
        let direct = self.route_for(job).await;
        let session: Arc<DirectSession> = Arc::new(tokio::sync::Mutex::new(None));
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
            // Target folder for each source folder (its parts joined); `None`: skipped, with everything below. The walk
            // is breadth first, so only the last two depths are ever looked up and older entries are dropped.
            let mut dirs: HashMap<String, Option<String>> = HashMap::new();
            let mut deepest = 0usize;
            dirs.insert(String::new(), Some(job.dst_dir.clone()));
            let files = Arc::new(Semaphore::new(FILES_AT_ONCE as usize));
            let mut tasks = tokio::task::JoinSet::new();
            let mut first_error: Option<FsError> = None;
            let mut started_running = false;
            let key = |rel: &[String]| rel.join("\u{0}");
            while let Some(found) = rx.recv().await {
                if first_error.is_some() || live.halt.load(Ordering::Relaxed) {
                    continue; // drain so the walk can finish
                }
                let depth = match &found {
                    Found::Dir { rel, .. } | Found::File { rel, .. } | Found::Link { rel, .. } => rel.len(),
                };
                if depth > deepest {
                    deepest = depth;
                    dirs.retain(|k, _| k.is_empty() || k.split('\u{0}').count() + 1 >= depth);
                }
                match found {
                    Found::Dir { rel, path } => {
                        let parent = dirs.get(&key(&rel[..rel.len() - 1])).cloned().flatten();
                        let target = match parent {
                            None => None,
                            Some(parent) => match self.target_dir(job, &src, &dst, live, &path, &parent, rel.last().unwrap()).await {
                                Ok(t) => t,
                                Err(e) if e.kind == ErrKind::Connection => {
                                    first_error = Some(e);
                                    live.halt();
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
                        dirs.insert(key(&rel), target);
                    }
                    Found::File { path, rel, meta } => {
                        let Some(dir) = dirs.get(&key(&rel[..rel.len() - 1])).cloned().flatten() else {
                            self.count(&job.id, |j| j.totals.skipped += 1);
                            continue;
                        };
                        if !started_running {
                            started_running = true;
                            self.update(&job.id, |j| j.state = State::Running);
                        }
                        self.count(&job.id, |j| {
                            j.totals.files += 1;
                            j.totals.bytes += meta.size;
                        });
                        let found = live.discovered.fetch_add(meta.size, Ordering::Relaxed) + meta.size;
                        if over_space(found, live.not_needed.load(Ordering::Relaxed), free) {
                            first_error = Some(FsError::new(ErrKind::Other, format!("not enough free space on the target ({} bytes free when the transfer started)", free.unwrap_or(0))));
                            live.halt();
                            continue;
                        }
                        let permits = if meta.size >= BIG_FILE { FILES_AT_ONCE } else { 1 };
                        let Ok(permit) = files.clone().acquire_many_owned(permits).await else { break };
                        let (me, live, src, dst, job, name, direct, session) = (self.clone(), live.clone(), src.clone(), dst.clone(), job.clone(), rel.last().unwrap().clone(), direct.clone(), session.clone());
                        tasks.spawn(async move {
                            let r = me.copy_one(&job, &live, src.as_ref(), dst.as_ref(), &path, &meta, &dir, &name, resumed, direct.as_deref().map(|t| (t, session.as_ref()))).await;
                            drop(permit);
                            r
                        });
                    }
                    Found::Link { .. } => {}
                }
                while let Some(done) = tasks.try_join_next() {
                    if let Ok(Err(e)) = done {
                        first_error.get_or_insert(e);
                        live.halt();
                    }
                }
            }
            while let Some(done) = tasks.join_next().await {
                if let Ok(Err(e)) = done {
                    first_error.get_or_insert(e);
                    live.halt();
                }
            }
            first_error
        };
        let (walked, failed) = tokio::join!(walker, consumer);
        if let Some(a) = &job.src.dest {
            let left = session.lock().await.take();
            self.stop_copier(live, left, a).await;
        }
        if let Some(stop) = Self::halted(live) {
            return Ok(stop);
        }
        // A lost connection in the walk is a lost connection of the job (retried), never a job that is "done".
        let skipped = match (walked, failed) {
            (_, Some(e)) => return Err(e),
            (Err(e), None) => return Err(e),
            (Ok(s), None) => s,
        };
        self.update(&job.id, |j| {
            j.totals.skipped += skipped.folder_links + skipped.unreadable_names + skipped.broken_links + skipped.special;
            j.walk = skipped;
            j.current = None;
        });
        Ok(Outcome::Done)
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

    /// The first `name (n)` free in `dir` (on the target, and not claimed by a file on its way there).
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

    /// Waits until no other file of any job is writing `key`, then holds it; `None` when the job is halted meanwhile.
    async fn claim(&self, key: String, live: &Live) -> Option<Claim<'_>> {
        loop {
            if self.claims.lock().insert(key.clone()) {
                return Some(Claim { set: &self.claims, key });
            }
            if live.halt.load(Ordering::Relaxed) {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Puts a question on the job and waits for the answer; `None` when the job is paused, cancelled or halted meanwhile.
    async fn ask(&self, id: &str, live: &Arc<Live>, mut q: Question) -> Option<Choice> {
        let qid = live.next_q.fetch_add(1, Ordering::Relaxed) + 1;
        q.id = qid;
        let (tx, rx) = oneshot::channel();
        live.answers.lock().insert(qid, tx);
        // "Apply to all" given while this file was on its way answers it too (never a changed or clashing target).
        if !q.changed && !q.kind_clash {
            if let Some(choice) = Choice::for_policy(self.policy(id)) {
                live.answers.lock().remove(&qid);
                return Some(choice);
            }
        }
        if live.halt.load(Ordering::Relaxed) {
            live.answers.lock().remove(&qid);
            return None;
        }
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
    async fn copy_one(&self, job: &Job, live: &Arc<Live>, src: &dyn RemoteFs, dst: &dyn RemoteFs, path: &str, meta: &Meta, dir: &str, name: &str, resumed: bool, direct: Option<(&Target, &DirectSession)>) -> FsResult<()> {
        let mut name = self.mapped_name(job, dst, name);
        let tag = tag(&job.id);
        self.count(&job.id, |j| j.current = Some(path.to_string()));
        let mut approved_override: Option<Option<(u64, u64)>> = None;
        let mut changed = false;
        let mut changes = 0;
        loop {
            if live.halt.load(Ordering::Relaxed) {
                return Ok(());
            }
            let target = dst.join(dir, &name);
            let key = target_key(&job.dst, dst, &target);
            // Two files (of this job or another) that would write one target take turns.
            let Some(_claim) = self.claim(key.clone(), live).await else { return Ok(()) };
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
            if resumed && approved_override.is_none() && !changed && existing.as_ref().is_some_and(|m| m.kind == FsKind::File && m.size == meta.size && m.mtime == meta.mtime) {
                live.ctl.bytes.fetch_add(meta.size, Ordering::Relaxed);
                live.not_needed.fetch_add(meta.size, Ordering::Relaxed);
                self.count(&job.id, |j| j.totals.done_files += 1);
                return Ok(());
            }
            // A second file for a target this job already wrote (same names from two folders, names that differ only
            // in case, names that map to the same Windows name).
            let same_job_clash = existing.is_some() && live.written.lock().contains(&key);
            let decision = if same_job_clash {
                Decision::KeepBoth
            } else if let Some(approved) = approved_override.take() {
                Decision::Copy { approved }
            } else if changed {
                // A target that changed after it was approved is always asked about, never decided by policy again.
                Decision::Ask { identical: false, kind_clash: false }
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
                    live.not_needed.fetch_add(meta.size, Ordering::Relaxed);
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
                    let partial = dst.join(dir, &partial_name(&name, &tag));
                    // Only a partial this job recorded is ever continued.
                    let seen = self.get(&job.id).and_then(|j| j.partials.get(&partial).copied());
                    if seen != Some((meta.size, meta.mtime)) {
                        // Saved with the next journal write (within a second). A partial whose entry was lost to a
                        // crash is not the job's, so it is started again rather than resumed: safe either way.
                        let (p, s) = (partial.clone(), (meta.size, meta.mtime));
                        self.count(&job.id, move |j| {
                            j.partials.insert(p, s);
                        });
                    }
                    let fc = FileCopy { src, src_path: path, dst, dst_dir: dir, dst_name: &name, tag: &tag, approved, resume: seen.is_some(), source_seen: seen, preserve: job.preserve };
                    // Direct when the route allows and the names fit an sftp batch line; else through this computer.
                    let result = match (direct, &job.src.dest) {
                        (Some((t, session)), Some(a)) if route::batch_quote(path).is_some() && route::batch_quote(&partial).is_some() => self.direct_one(&fc, live, a, t, &partial, session).await,
                        _ => copy_file(&fc, &live.ctl).await,
                    };
                    match result {
                        Ok(Copied::Done { warning, resumed_at, .. }) => {
                            live.not_needed.fetch_add(resumed_at, Ordering::Relaxed);
                            live.written.lock().insert(key);
                            self.count(&job.id, |j| {
                                j.partials.remove(&partial);
                                j.totals.done_files += 1;
                            });
                            if let Some(w) = warning {
                                self.failed(&job.id, &target, &FsError::new(ErrKind::Other, w));
                            }
                            return Ok(());
                        }
                        Ok(Copied::TargetChanged { .. }) => {
                            // Asked again with what is there now; the partial stays and is continued after the answer.
                            changes += 1;
                            if changes > MAX_TARGET_CHANGES {
                                self.drop_partial(job, dst, &partial).await;
                                self.failed(&job.id, path, &FsError::new(ErrKind::Other, format!("{target} keeps changing on the target; not replaced")));
                                return Ok(());
                            }
                            changed = true;
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
                            // A file that failed for another reason is not resumed: its partial goes.
                            self.drop_partial(job, dst, &partial).await;
                            self.failed(&job.id, path, &e);
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    /// How a host-to-host copy travels this attempt (direct: B as A reaches it), recorded on the job with the reason.
    async fn route_for(&self, job: &Job) -> Option<Arc<Target>> {
        let (Some(a), Some(b)) = (&job.src.dest, &job.dst.dest) else { return None };
        // Tests: in-memory hosts have no ssh to probe with.
        #[cfg(test)]
        let route = if self.test_ends.lock().contains_key(a) { Route::Relay } else { job.route };
        #[cfg(not(test))]
        let route = job.route;
        let (used, reason, target) = match route {
            Route::Relay => ("relay", Some("you chose to copy through this computer".to_string()), None),
            Route::Auto => match self.probes.direct(self.pool.exe(), a, b).await {
                Ok(t) => ("direct", None, Some(Arc::new(t))),
                Err(reason) => ("relay", Some(reason), None),
            },
        };
        tracing::info!(job = %job.id, route = used, "host to host");
        self.update(&job.id, |j| {
            j.route_used = Some(used.to_string());
            j.route_reason = reason;
        });
        target
    }

    /// One file copied by host A itself, through the job's copier on A (`put` for a fresh partial, `reput` to continue
    /// one), with progress read from the partial's size through Kelpie's own session to B; then put in place as any
    /// copy is.
    #[allow(clippy::too_many_arguments)]
    async fn direct_one(&self, fc: &FileCopy<'_>, live: &Arc<Live>, a: &str, b: &Target, partial: &str, session: &DirectSession) -> FsResult<Copied> {
        let src = fc.src.stat(fc.src_path).await?;
        recover_aside(fc.dst, fc.dst_dir, fc.dst_name, fc.tag, &src).await?;
        // The same checks as any resume: the job's own partial, a regular file (never through a link), the tail.
        let offset = resume_offset(fc, &src, partial).await?;
        if offset == 0 {
            // Made new and owner-only here; whatever had the name (a stale partial, a planted link) goes first.
            fc.dst.open_write(partial, 0, true).await?.finish(false).await?;
        }
        live.ctl.bytes.fetch_add(offset, Ordering::Relaxed);
        let plan = route::direct_plan(offset, src.size);
        if plan != route::DirectPlan::Complete {
            let exe = self.pool.exe();
            let mut guard = session.lock().await;
            if guard.as_ref().is_some_and(|s| s.files >= route::FILES_PER_LOGIN) {
                self.stop_copier(live, guard.take(), a).await;
            }
            if guard.is_none() {
                let stale: Vec<u32> = std::mem::take(&mut *live.stale_pids.lock());
                *guard = Some(route::Session::start(exe, a, b, &stale).await?);
            }
            let seq = guard.as_mut().expect("started").send(plan, fc.src_path, partial).await?;
            let mut last = offset;
            let finished = loop {
                tokio::select! {
                    r = guard.as_mut().expect("started").done(seq) => break r,
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        if let Ok(m) = fc.dst.lstat(partial).await {
                            let d = m.size.saturating_sub(last);
                            live.ctl.bytes.fetch_add(d, Ordering::Relaxed);
                            live.ctl.written.fetch_add(d, Ordering::Relaxed);
                            last = last.max(m.size);
                        }
                    }
                    _ = live.ctl.wake.notified() => {}
                }
                if let Some(stop) = live.ctl.stop() {
                    self.stop_copier(live, guard.take(), a).await;
                    if stop == Stop::Cancel {
                        let _ = fc.dst.remove_file(partial).await;
                    }
                    return Ok(Copied::Stopped(stop));
                }
            };
            if let Err(e) = finished {
                // The copier is gone (a dropped connection): it is stopped on A before any retry starts another.
                self.stop_copier(live, guard.take(), a).await;
                return Err(e);
            }
            let said = guard.as_ref().map(|s| s.stderr_text()).unwrap_or_default();
            drop(guard);
            let have = fc.dst.lstat(partial).await.map(|m| m.size).unwrap_or(0);
            live.ctl.bytes.fetch_add(have.saturating_sub(last), Ordering::Relaxed);
            live.ctl.written.fetch_add(have.saturating_sub(last), Ordering::Relaxed);
            if have != src.size {
                let kind = if route::lost(&said) { ErrKind::Connection } else { ErrKind::Other };
                return Err(FsError::new(kind, if said.is_empty() { "the copy on the first host did not finish".to_string() } else { format!("the copy on the first host failed: {said}") }));
            }
        }
        finish_partial(fc, &src, partial, offset).await
    }

    /// Ends a job's copier on A; one that A could not be told about is stopped before the next copier starts.
    async fn stop_copier(&self, live: &Live, session: Option<route::Session>, a: &str) {
        if let Some(s) = session {
            let pid = s.pid;
            if !s.stop(self.pool.exe(), a).await {
                live.stale_pids.lock().extend(pid);
            }
        }
    }
    async fn drop_partial(&self, job: &Job, dst: &dyn RemoteFs, partial: &str) {
        let _ = dst.remove_file(partial).await;
        let p = partial.to_string();
        self.count(&job.id, move |j| {
            j.partials.remove(&p);
        });
    }

    /// A delete or chmod job on a host: files as they are found, folders at the end (deepest first, so a delete
    /// empties them first and a chmod never locks the walk out). Links are acted on as links, never followed.
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
        let (mode, only_x) = (job.mode.unwrap_or(0), job.dirs_only_x);
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
                    Found::File { path, meta, .. } => {
                        self.count(&job.id, |j| j.totals.files += 1);
                        let r = if job.kind == JobKind::Delete { fs.remove_file(&path).await } else { fs.chmod(&path, chmod_file_mode(mode, only_x, meta.mode, job.mask)).await };
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
                    Err(e) if e.kind == ErrKind::Connection => {
                        first_error = Some(e);
                        live.halt();
                    }
                    Err(e) => self.failed(&job.id, &path, &e),
                }
            }
            (first_error, dirs)
        };
        let (walked, (failed, mut dirs)) = tokio::join!(walker, consumer);
        if let Some(stop) = Self::halted(live) {
            return Ok(stop);
        }
        let skipped = match (walked, failed) {
            (_, Some(e)) => return Err(e),
            (Err(e), None) => return Err(e),
            (Ok(s), None) => s,
        };
        // Folders only after a complete walk (a lost connection mid-walk left some unvisited), deepest first.
        dirs.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, path) in dirs {
            if let Some(stop) = Self::halted(live) {
                return Ok(stop);
            }
            self.count(&job.id, |j| j.totals.files += 1);
            let r = if job.kind == JobKind::Delete {
                fs.remove_dir(&path).await
            } else {
                let current = fs.lstat(&path).await.ok().and_then(|m| m.mode);
                fs.chmod(&path, chmod_dir_mode(mode, only_x, current, job.mask)).await
            };
            match r {
                Ok(()) => self.count(&job.id, |j| j.totals.done_files += 1),
                Err(e) if e.kind == ErrKind::Connection => return Err(e),
                Err(e) => self.failed(&job.id, &path, &e),
            }
        }
        self.update(&job.id, |j| j.walk = skipped);
        Ok(Outcome::Done)
    }
}

/// A partial's old file left under its moved-aside name, with the target gone: put back.
async fn restore_aside(fs: &dyn RemoteFs, partial: &str) {
    let seps: &[char] = if fs.windows_names() { &['/', '\\'] } else { &['/'] };
    let (dir, file) = match partial.rsplit_once(seps) {
        Some((d, f)) => (if d.is_empty() { "/" } else { d }, f),
        None => return,
    };
    let Some((name, tag)) = super::names::split_hidden(file) else { return };
    let (old, target) = (fs.join(dir, &super::names::aside_name(name, tag)), fs.join(dir, name));
    if fs.lstat(&old).await.is_ok() && fs.lstat(&target).await.is_err() {
        let _ = fs.rename(&old, &target, false).await;
    }
}

/// How often a target that keeps changing under a copy is asked about before that file fails.
const MAX_TARGET_CHANGES: u32 = 3;

impl Choice {
    /// The answer a job's policy gives without asking (`None` for `ask`, and for `skip_identical`, which asks about
    /// files that differ).
    fn for_policy(p: Policy) -> Option<Choice> {
        Some(match p {
            Policy::Ask | Policy::SkipIdentical => return None,
            Policy::Replace => Choice::Replace,
            Policy::Skip => Choice::Skip,
            Policy::KeepBoth => Choice::KeepBoth,
            Policy::Newer => Choice::Newer,
        })
    }
}

/// The bits a recursive chmod sets on a file whose mode is now `current`: only the rwx bits in `mask` (the ones the
/// person changed; all of rwx without one) take `mode`'s value. A file's setuid, setgid and sticky bits are never
/// changed, so a folder's special bits never spread to the files inside. With `dirs_only_x` ("x for folders only")
/// a file keeps its own x bits, so a script stays runnable and nothing else becomes runnable.
pub fn chmod_file_mode(mode: u32, dirs_only_x: bool, current: Option<u32>, mask: Option<u32>) -> u32 {
    let cur = current.unwrap_or(0) & 0o7777;
    let mask = mask.unwrap_or(0o777) & 0o777;
    let mut new = (cur & !mask) | (mode & mask);
    if dirs_only_x {
        new = (new & !0o111) | (cur & 0o111);
    }
    new
}

/// The bits a recursive chmod sets on a folder whose mode is now `current`: the bits in `mask` (all without one) take
/// `mode`'s value. With `dirs_only_x` folders always get x wherever they have r, so they stay searchable.
pub fn chmod_dir_mode(mode: u32, dirs_only_x: bool, current: Option<u32>, mask: Option<u32>) -> u32 {
    let cur = current.unwrap_or(0) & 0o7777;
    let mask = mask.unwrap_or(0o7777) & 0o7777;
    let mut new = (cur & !mask) | (mode & mask);
    if dirs_only_x {
        new |= (new & 0o444) >> 2;
    }
    new
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chmod_bits() {
        assert_eq!((chmod_file_mode(0o755, false, Some(0o644), None), chmod_dir_mode(0o755, false, Some(0o700), None)), (0o755, 0o755));
        // x for folders only: folders always get it, files keep their own.
        assert_eq!(chmod_dir_mode(0o644, true, Some(0o700), None), 0o755);
        assert_eq!(chmod_file_mode(0o644, true, Some(0o600), None), 0o644, "a plain file does not become runnable");
        assert_eq!(chmod_file_mode(0o644, true, Some(0o755), None), 0o755, "a script stays runnable");
        assert_eq!(chmod_file_mode(0o750, true, Some(0o700), None), 0o740);
        assert_eq!(chmod_file_mode(0o640, true, None, None), 0o640);
    }

    #[test]
    fn a_recursive_chmod_changes_only_the_bits_the_person_changed_and_never_spreads_special_bits() {
        // On a setgid shared folder (2775) the person only turns on "others: read" (mask 0o004).
        let (mode, mask) = (0o2775 | 0o004, Some(0o004));
        assert_eq!(chmod_dir_mode(mode, false, Some(0o2770), mask), 0o2774, "the folder keeps its setgid, gets o+r");
        assert_eq!(chmod_file_mode(mode, false, Some(0o640), mask), 0o644, "files get o+r and nothing else");
        assert_eq!(chmod_file_mode(mode, true, Some(0o750), mask), 0o754, "a script keeps its x, gets no setgid");
        // Without a mask (all bits): still never setuid / setgid on files.
        assert_eq!(chmod_file_mode(0o4755, false, Some(0o644), None), 0o755);
        assert_eq!(chmod_file_mode(0o755, false, Some(0o4700), None), 0o4755, "a file's own special bit is not removed either");
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

    fn partial(id: &str, dir: &str, name: &str) -> String {
        format!("{dir}/{}", partial_name(name, &tag(id)))
    }

    /// The journal is written off the async workers: read it once it says what is expected.
    async fn journal_until(r: &Rig, ok: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..200 {
            if let Some(v) = std::fs::read(r.dir.join("transfers.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).filter(|v| ok(v)) {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the journal never said so");
    }

    fn data(n: usize) -> Vec<u8> {        (0..n).map(|i| (i * 7 % 253) as u8).collect()
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
        let saved = journal_until(&r, |v| v["jobs"][0]["state"] == "done").await;
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
        assert!(r.b.exists(&partial(&id, "/dst", "big.bin")), "kept for the resume");
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
            if !r.b.exists(&partial(&id, "/dst", "big.bin")) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(!r.b.exists(&partial(&id, "/dst", "big.bin")));
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
        let mut saved = journal_until(&r, |v| v["jobs"][0]["state"] == "done").await;
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

    #[tokio::test]
    async fn two_files_for_one_target_never_share_a_partial() {
        // Same name from two folders in one job, and a second job to the same folder.
        let r = rig("claims");
        let (one, two) = (data(3 << 20), (0..3 << 20).map(|i| (i % 7) as u8).collect::<Vec<u8>>());
        r.a.put("/x/readme.md", &one, 10);
        r.a.put("/y/readme.md", &two, 20);
        r.b.mkdir_all("/dst");
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/x/readme.md", "/y/readme.md"], "dst_dir": "/dst"})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!((j.state, j.totals.done_files), (State::Done, 2), "{j:?}");
        let got = [r.b.read("/dst/readme.md").unwrap(), r.b.read("/dst/readme (2).md").unwrap()];
        assert!(got.contains(&one) && got.contains(&two), "each target holds one whole source");
        // Two jobs at once to one folder, replacing: the result is one source, whole.
        r.b.remove_file("/dst/readme.md").await.unwrap();
        let first = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/x/readme.md"], "dst_dir": "/dst", "conflict": "replace"})).await;
        let second = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/y/readme.md"], "dst_dir": "/dst", "conflict": "replace"})).await;
        until(&r, &first, |j| j.state.finished()).await;
        until(&r, &second, |j| j.state.finished()).await;
        let end = r.b.read("/dst/readme.md").unwrap();
        assert!(end == one || end == two, "never a mix of the two");
        assert!(r.b.paths().iter().all(|p| !p.contains("kelpie-part")), "{:?}", r.b.paths());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn the_retry_window_runs_from_the_drop() {
        let t0 = Instant::now();
        let mut r = Retry::default();
        // A drop long after the job started still retries; the waits grow.
        assert_eq!(r.on_drop(t0 + Duration::from_secs(3600), false), Some(2));
        assert_eq!(r.on_drop(t0 + Duration::from_secs(3610), false), Some(5));
        // An attempt that wrote something starts the count again.
        assert_eq!(r.on_drop(t0 + Duration::from_secs(3700), true), Some(2));
        assert_eq!(r.attempt, 1);
        // Five minutes after the first drop without progress, it gives up.
        assert_eq!(r.on_drop(t0 + Duration::from_secs(3700) + RETRY_FOR, false), None);
    }

    #[tokio::test]
    async fn a_connection_lost_while_listing_is_retried_not_done() {
        let r = rig("walkdrop");
        for d in 0..6 {
            r.a.put(&format!("/t/d{d}/f.txt"), b"x", 1);
        }
        r.b.mkdir_all("/dst");
        r.a.fail_lists_after(Some(2));
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/t"], "dst_dir": "/dst", "conflict": "replace"})).await;
        let j = until(&r, &id, |j| j.state == State::Reconnecting || j.state.finished()).await;
        assert_eq!(j.state, State::Reconnecting, "never done with folders left out");
        r.a.fail_lists_after(None);
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!((j.state, j.totals.done_files), (State::Done, 6), "{j:?}");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_failure_releases_files_waiting_on_a_question() {
        let r = rig("release");
        r.a.put("/s/a-asks.txt", b"new", 10);
        r.a.put("/s/b-big.bin", &data(1 << 20), 10);
        r.b.put("/dst/s/a-asks.txt", b"old", 5);
        // The first file waits on its question; the second loses the connection.
        r.a.fail_reads_after(Some(100_000));
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/s"], "dst_dir": "/dst"})).await;
        let j = until(&r, &id, |j| j.state == State::Reconnecting || j.state.finished()).await;
        // Without the release this waits for an answer with a dead connection.
        assert_eq!(j.state, State::Reconnecting, "{j:?}");
        r.q.rpc("xfer.control", &json!({"id": id, "action": "cancel"})).await.unwrap();
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn free_space_counts_only_new_bytes() {
        assert!(!over_space(100, 0, None));
        assert!(!over_space(100, 0, Some(100)));
        assert!(over_space(101, 0, Some(100)));
        // Skipped, complete and resumed bytes need no room.
        assert!(!over_space(150, 60, Some(100)));
    }

    #[tokio::test]
    async fn a_target_that_keeps_changing_fails_plainly_instead_of_looping() {
        let r = rig("changing");
        r.a.put("/s/log.txt", &data(1000), 10);
        r.b.put("/dst/log.txt", b"live", 5);
        r.b.bump_mtime_on_lstat("/dst/log.txt");
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/s/log.txt"], "dst_dir": "/dst", "conflict": "replace"})).await;
        // Every re-check finds it changed: asked each time, then the file fails.
        for _ in 0..10 {
            let j = until(&r, &id, |j| !j.questions.is_empty() || j.state.finished()).await;
            if j.state.finished() {
                break;
            }
            assert!(j.questions[0].changed, "a changed target is asked about, never decided by policy");
            let _ = r.q.rpc("xfer.answer", &json!({"id": id, "choice": "replace"})).await;
        }
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.totals.failed, 1, "{j:?}");
        assert!(j.failures[0].error.contains("keeps changing"));
        assert_eq!(r.b.read("/dst/log.txt").unwrap(), b"live");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_host_to_host_copy_says_how_it_travelled() {
        let r = rig("route");
        r.a.put("/s/x", b"x", 1);
        r.b.mkdir_all("/dst");
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/s/x"], "dst_dir": "/dst", "route": "relay"})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!((j.route, j.route_used.as_deref(), j.route_reason.as_deref()), (Route::Relay, Some("relay"), Some("you chose to copy through this computer")));
        assert_eq!(r.b.read("/dst/x").unwrap(), b"x");
        assert!(r.q.rpc("xfer.start", &json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/s/x"], "dst_dir": "/dst", "route": "teleport"})).await.is_err());
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_missing_target_folder_fails_the_job() {
        let r = rig("nofolder");
        r.a.put("/s/x", b"x", 1);
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/s/x"], "dst_dir": "/not/there"})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Failed, "never a job of skips into nowhere");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn names_windows_cannot_hold_and_unicode_names_arrive_on_this_computer() {
        let r = rig("names");
        r.a.put("/s/ünïcødé 日本語.txt", b"u", 1);
        r.a.put("/s/a:b?.txt", b"w", 1);
        let local = r.dir.join("down");
        std::fs::create_dir_all(&local).unwrap();
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {}, "items": ["/s"], "dst_dir": local.to_string_lossy()})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Done, "{j:?}");
        assert_eq!(std::fs::read(local.join("s").join("ünïcødé 日本語.txt")).unwrap(), b"u");
        if cfg!(windows) {
            assert_eq!(std::fs::read(local.join("s").join("a_b_.txt")).unwrap(), b"w");
            assert_eq!(j.renamed, vec![("a:b?.txt".to_string(), "a_b_.txt".to_string())]);
        }
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn a_delete_on_a_server_whose_listing_follows_links_keeps_the_target() {
        let r = rig("deletefollow");
        let host = MemFs::new().following_links(true);
        r.q.test_ends.lock().insert("f".into(), Arc::new(host.clone()));
        host.put("/srv/old/a.txt", b"a", 1);
        host.put("/keep/k.txt", b"k", 1);
        host.link("/srv/old/to-keep", "/keep");
        let id = start(&r, json!({"kind": "delete", "src": {"dest": "f"}, "items": ["/srv/old"]})).await;
        let j = until(&r, &id, |j| j.state.finished()).await;
        assert_eq!(j.state, State::Done, "{j:?}");
        assert!(!host.exists("/srv/old"));
        assert_eq!(host.read("/keep/k.txt").unwrap(), b"k", "data outside the selection is never touched");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[tokio::test]
    async fn counts_the_transfers_an_update_waits_for() {
        let r = rig("active");
        assert_eq!(r.q.active_count(), 0);
        r.a.put("/s/x", b"x", 1);
        r.b.put("/dst/x", b"old", 1);
        let id = start(&r, json!({"src": {"dest": "a"}, "dst": {"dest": "b"}, "items": ["/s/x"], "dst_dir": "/dst"})).await;
        until(&r, &id, |j| !j.questions.is_empty()).await;
        assert_eq!(r.q.active_count(), 1, "a transfer waiting on a question still runs");
        r.q.rpc("xfer.control", &json!({"id": id, "action": "pause"})).await.unwrap();
        until(&r, &id, |j| j.state == State::Paused).await;
        assert_eq!(r.q.active_count(), 0, "a paused one does not hold an update");
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
