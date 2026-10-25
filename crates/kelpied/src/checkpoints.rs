//! Per-turn snapshots of a session's working tree, so any agent's turn can be inspected and undone.
//!
//! A session gets one baseline snapshot when it starts; afterwards one snapshot when a turn ends
//! (turn N = end N-1 → end N), so no start hook is needed and no start race exists. A snapshot
//! never touches the user's index, HEAD, branch or files: a copy of `.git/index` (which keeps
//! git's stat cache) is filled with `add -A`, written as a tree, committed with `commit-tree -p
//! HEAD` and stored under `refs/kelpie/<session>/<n>`. Every git call is literal-pathspec only.
//!
//! Records live in `<data_dir>/checkpoints/<session>.json`. Snapshots of one repository run one
//! after another on a worker thread; a repository whose snapshots take over a second twice in a
//! row is switched off (and says so once).
//!
//! Undo restores the files one turn changed, never silently over somebody else's work: it needs
//! the owning session idle, and a file another session touched in an overlapping window, or that
//! changed since the turn, is only restored when the caller confirms that file.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::persist::write_atomic;

/// Snapshots kept per session (one more is kept as the base of the oldest turn).
pub const KEEP: usize = 50;
/// Paths remembered per record (who touched what, overlap checks).
const PATHS_CAP: usize = 400;
/// Files listed in one diff.
const DIFF_CAP: usize = 2000;
/// A snapshot slower than this counts against the repository.
const SLOW: Duration = Duration::from_secs(1);
/// Slow snapshots in a row that switch a repository off.
const SLOW_LIMIT: u32 = 2;
/// A trivial `git --version` slower than this means the machine is busy, not the repository big.
const BUSY: Duration = Duration::from_millis(300);
/// Git CPU time below this was not really counted (a launcher that started the real git before it
/// joined the job object): the measurement says nothing, and the wall time has to be judged instead.
const MIN_COUNTED_CPU: Duration = Duration::from_millis(30);
/// Without git's CPU time, only a repository with an index this big can be slow because of its size.
const BIG_INDEX: u64 = 256 << 10;
/// A snapshot read this long after the turn ended is labelled approximate.
const DELAYED_MS: u64 = 3000;
/// A file older than its turn's start by more than this was not written by that turn.
const STALE_SLACK_MS: u64 = 2000;
/// Gap windows (work that left no snapshot) remembered per session.
const WINDOWS_CAP: usize = 100;
const ATTEMPTS: usize = 3;
const BLOB_CAP: u64 = 8 << 20;
/// Off until the user turns them on: a snapshot of a 10k-file repository costs far more than the
/// 150 ms budget on Windows (numbers in docs/architecture.md), so they are opt-in.
pub const DEFAULT_ENABLED: bool = false;
const ZERO_OID: &str = "0000000000000000000000000000000000000000";

pub type Emit = Arc<dyn Fn(&str, Value) + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Where a session's history starts (spawn, restart, or the first snapshot after a gap).
    Baseline,
    Turn,
    /// The state right after an undo, so the next turn does not include it.
    Undo,
    /// A turn is starting: a detection-only look at the folder (no commit, no ref, never recorded).
    Start,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// Snapshot number: baseline 0, then counting up; skipped (unchanged) turns take none.
    pub n: u32,
    pub kind: Kind,
    pub commit: String,
    pub tree: String,
    /// The snapshot this one is measured from (turns and undos).
    #[serde(default)]
    pub base: Option<String>,
    #[serde(default)]
    pub base_n: Option<u32>,
    /// When the snapshot finished reading the folder (or the turn ended, if later).
    pub at: u64,
    /// When the snapshot began reading the folder.
    #[serde(default)]
    pub read_at: u64,
    /// Start of the stretch this record's diff covers: when the previous snapshot began reading.
    /// 0 means unknown (older records): it overlaps everything.
    #[serde(default)]
    pub from: u64,
    /// When the session started working on this turn.
    pub started: u64,
    /// hook | acp | idle | spawn | restart | undo
    pub source: String,
    /// The boundary is a guess (an agent without hooks: output went quiet).
    #[serde(default)]
    pub approximate: bool,
    #[serde(default)]
    pub files: u32,
    #[serde(default)]
    pub added: u32,
    #[serde(default)]
    pub removed: u32,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub truncated: bool,
    /// Files git could not read (another program held them): not in this snapshot.
    #[serde(default)]
    pub unreadable: Vec<String>,
    /// Files that differed between the previous snapshot's end and the moment this turn began:
    /// changed by someone else in between, even if the agent then edited them too.
    #[serde(default)]
    pub between: Vec<String>,
    /// More than `PATHS_CAP` such files, or the look at the start failed: treat every file as changed.
    #[serde(default)]
    pub between_truncated: bool,
    #[serde(default)]
    pub undo_of: Option<u32>,
}

/// What the look at the start of the running turn found, until the turn's own snapshot takes it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StartInfo {
    paths: Vec<String>,
    truncated: bool,
}

/// A stretch in which a session may have changed any file but left no snapshot (one failed).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Window {
    from: u64,
    at: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Log {
    session: String,
    /// Repository top as git printed it, and the folder key sessions are compared by.
    repo: String,
    key: String,
    records: Vec<Record>,
    #[serde(default)]
    windows: Vec<Window>,
    /// A snapshot failed since the last record: the next one is approximate.
    #[serde(default)]
    unsure: bool,
    #[serde(default)]
    start: Option<StartInfo>,
    /// The snapshot number the last look at a turn's start came after (whether or not it found
    /// anything): the turn measured from another snapshot had no look, and nothing is known.
    #[serde(default)]
    looked_after: Option<u32>,
    /// The session is gone: its refs are deleted; what remains tells another session's undo
    /// that it touched files.
    #[serde(default)]
    closed: bool,
}

#[derive(Debug, Clone, Serialize)]
struct Disabled {
    at: u64,
    reason: String,
    repo: String,
}

#[derive(Default)]
struct Inner {
    logs: HashMap<String, Log>,
    work_start: HashMap<String, u64>,
    disabled: BTreeMap<String, Disabled>,
    slow: HashMap<String, u32>,
    refs_synced: HashSet<String>,
    dropped: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub sid: String,
    pub cwd: String,
    pub kind: Kind,
    pub source: String,
    pub approximate: bool,
    pub started: u64,
    pub at: u64,
}

struct Queue {
    jobs: VecDeque<Job>,
}

pub struct Checkpoints {
    dir: PathBuf,
    emit: Emit,
    /// A turn without a look at its start after its base snapshot has every file flagged (tests that
    /// do not exercise the look turn this off).
    look_required: AtomicBool,
    /// Wall clock in ms (a test replaces it).
    clock: Box<dyn Fn() -> u64 + Send + Sync>,
    /// Turns kept per session (a test lowers it).
    keep: usize,
    enabled: AtomicBool,
    inner: Mutex<Inner>,
    queues: Mutex<HashMap<String, Queue>>,
    repo_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    repos: Mutex<HashMap<String, Arc<Repo>>>,
    /// The latest snapshot's wall time, git CPU time and busy probe (logged when slow; tests read it).
    last_timing: Mutex<(Duration, Duration, Option<Duration>)>,
}

/// What an undo is asked to do: turn `n`, all its files or `paths`, with `confirm` naming the shared or
/// changed-since files that may be overwritten; `at` stamps the undo step.
#[derive(Clone, Copy)]
pub struct UndoRequest<'a> {
    pub n: u32,
    pub paths: Option<&'a [String]>,
    pub confirm: &'a [String],
    pub at: u64,
}

/// What the daemon knows about the sessions around an undo.
#[derive(Debug, Clone, Default)]
pub struct UndoCtx {
    pub owner_working: bool,
    /// Other sessions working in the same folder right now: (id, name).
    pub working_others: Vec<(String, String)>,
}

// ---------- git ----------

/// These git calls look for hooks in `/dev/null` (git resolves it itself on Windows): update-ref and
/// restore would otherwise run the repository's reference-transaction and post-checkout scripts for work
/// the user never asked for, and nothing can be planted in a path that is not a folder.
const NO_HOOKS: &str = "core.hooksPath=/dev/null";

fn command(dir: &Path, index: Option<&Path>) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-c").arg(NO_HOOKS);
    cmd.args(["-c", "core.quotepath=off", "-c", "core.fsmonitor=false", "-c", "core.safecrlf=false", "-c", "gc.auto=0", "-c", "commit.gpgsign=false", "-c", "core.splitIndex=false", "-c", "advice.updateSparsePath=false"])
        .arg("-C")
        .arg(dir)
        .env("GIT_LITERAL_PATHSPECS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        // Messages are read below (unreadable files, an unborn HEAD, a held lock): always English.
        .env("LC_ALL", "C")
        .env("LANGUAGE", "")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window. Normal priority: below normal starves under a build for minutes.
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

thread_local! {
    /// CPU time the git processes started by this thread have used since `take_cpu` (Windows).
    static CPU: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

fn take_cpu() -> Duration {
    CPU.with(|c| c.replace(Duration::ZERO))
}

/// Counts the CPU time of a git process and what it starts (a job object around it): how long a
/// snapshot took on a busy machine says nothing about the repository, its CPU time does.
#[cfg(windows)]
mod cpu_job {
    use std::os::windows::io::AsRawHandle;
    use std::process::Child;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation, QueryInformationJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    };

    pub struct CpuJob(HANDLE);

    impl CpuJob {
        pub fn around(child: &Child) -> Option<CpuJob> {
            // SAFETY: plain Win32 calls on handles this function creates or the child owns.
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return None;
                }
                if AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) == 0 {
                    CloseHandle(job);
                    return None;
                }
                Some(CpuJob(job))
            }
        }

        pub fn cpu(&self) -> Duration {
            // SAFETY: `info` is the structure the queried class fills in.
            unsafe {
                let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let ok = QueryInformationJobObject(self.0, JobObjectBasicAccountingInformation, &mut info as *mut _ as *mut _, std::mem::size_of_val(&info) as u32, std::ptr::null_mut());
                if ok == 0 {
                    return Duration::ZERO;
                }
                Duration::from_nanos((info.TotalUserTime.max(0) as u64 + info.TotalKernelTime.max(0) as u64) * 100)
            }
        }
    }

    impl Drop for CpuJob {
        fn drop(&mut self) {
            // SAFETY: the handle was created in `around` and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

fn run(mut cmd: Command, input: Option<Vec<u8>>) -> Result<Output> {
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().context("run git")?;
    #[cfg(windows)]
    let job = cpu_job::CpuJob::around(&child);
    let writer = input.map(|data| {
        let mut stdin = child.stdin.take().expect("piped stdin");
        std::thread::spawn(move || {
            let _ = stdin.write_all(&data);
        })
    });
    let out = child.wait_with_output().context("wait for git")?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    #[cfg(windows)]
    if let Some(job) = job {
        CPU.with(|c| c.set(c.get() + job.cpu()));
    }
    Ok(out)
}

fn git_raw(dir: &Path, index: Option<&Path>, args: &[&str], input: Option<Vec<u8>>) -> Result<Vec<u8>> {
    let mut cmd = command(dir, index);
    cmd.args(args);
    let out = run(cmd, input)?;
    if !out.status.success() {
        bail!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

pub(crate) fn git(dir: &Path, index: Option<&Path>, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8_lossy(&git_raw(dir, index, args, None)?).trim().to_string())
}

/// The folder holding `.git` at or above `cwd` (a file there means a linked worktree).
pub fn find_top(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors().find(|d| d.join(".git").exists()).map(Path::to_path_buf)
}

#[derive(Debug)]
struct Repo {
    top: PathBuf,
    index: PathBuf,
}

impl Repo {
    fn open(top: &Path) -> Result<Repo> {
        let out = git(top, None, &["rev-parse", "--path-format=absolute", "--git-path", "index"])?;
        let index = PathBuf::from(out.lines().next().ok_or_else(|| anyhow!("no index path"))?);
        Ok(Repo { top: top.to_path_buf(), index })
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        git(&self.top, None, args)
    }
}

/// A tree with the working folder's files (tracked, changed and untracked, not ignored), made
/// in a copy of the index so the real one is never opened for writing.
fn working_tree(repo: &Repo, tmp_index: &Path) -> Result<(String, Vec<String>)> {
    if let Some(parent) = tmp_index.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A lock left by a git process that was killed would block this session for good.
    let _ = std::fs::remove_file(tmp_index.with_extension("index.lock"));
    if repo.index.exists() {
        std::fs::copy(&repo.index, tmp_index).context("copy index")?;
    } else {
        let _ = std::fs::remove_file(tmp_index);
    }
    let result = (|| {
        // `--ignore-errors`: a file another program holds open must not cost the whole snapshot;
        // git then exits 1 (also for paths outside a sparse checkout) after writing the index.
        let mut cmd = command(&repo.top, Some(tmp_index));
        cmd.args(["add", "-A", "--ignore-errors"]);
        let out = run(cmd, None)?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        if !matches!(out.status.code(), Some(0 | 1)) {
            bail!("git add: {}", stderr.trim());
        }
        let unreadable = unreadable_files(&stderr);
        Ok((git(&repo.top, Some(tmp_index), &["write-tree"])?, unreadable))
    })();
    let _ = std::fs::remove_file(tmp_index);
    result
}

/// Paths from git's `unable to index file 'p'` lines.
fn unreadable_files(stderr: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in stderr.lines() {
        if let Some(rest) = line.split("unable to index file '").nth(1) {
            if let Some((path, _)) = rest.rsplit_once('\'') {
                out.push(path.to_string());
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    pub path: String,
    /// A (added), M (modified), D (deleted), T (type changed)
    pub status: char,
    #[serde(skip)]
    pub old_mode: String,
    #[serde(skip)]
    pub new_mode: String,
    pub added: Option<u32>,
    pub removed: Option<u32>,
    pub binary: bool,
}

/// Output of `git diff-tree -r -z --no-renames --raw --numstat`.
fn parse_diff_tree(raw: &[u8]) -> Vec<Change> {
    let text = String::from_utf8_lossy(raw);
    let mut tokens = text.split('\0').filter(|t| !t.is_empty());
    let mut list: Vec<Change> = Vec::new();
    let mut stats: HashMap<String, (Option<u32>, Option<u32>)> = HashMap::new();
    while let Some(tok) = tokens.next() {
        if let Some(head) = tok.strip_prefix(':') {
            let Some(path) = tokens.next() else { break };
            let f: Vec<&str> = head.split(' ').collect();
            if f.len() < 5 {
                continue;
            }
            let status = f[4].chars().next().unwrap_or('M');
            list.push(Change { path: path.to_string(), status, old_mode: f[0].to_string(), new_mode: f[1].to_string(), added: None, removed: None, binary: false });
        } else {
            let mut parts = tok.splitn(3, '\t');
            let (Some(a), Some(r), Some(p)) = (parts.next(), parts.next(), parts.next()) else { continue };
            stats.insert(p.to_string(), (a.parse().ok(), r.parse().ok()));
        }
    }
    for c in &mut list {
        if let Some((a, r)) = stats.get(&c.path) {
            c.added = *a;
            c.removed = *r;
            c.binary = a.is_none() && r.is_none();
        }
    }
    list
}

fn diff_changes(repo: &Repo, a: &str, b: &str) -> Result<Vec<Change>> {
    let raw = git_raw(&repo.top, None, &["diff-tree", "-r", "-z", "--no-renames", "--raw", "--numstat", a, b], None)?;
    Ok(parse_diff_tree(&raw))
}

/// A path git printed for a tree entry, safe to join below the working folder.
fn safe_rel(path: &str) -> Result<()> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.starts_with('\\')
        || path.contains('\0')
        // Drive letters and alternate data streams.
        || (cfg!(windows) && path.contains(':'))
        || path.split(['/', '\\']).any(|c| c.is_empty() || c == "." || c == ".." || c.eq_ignore_ascii_case(".git"));
    if bad {
        bail!("unsafe path {path:?}");
    }
    Ok(())
}

fn safe_id(id: &str) -> String {
    id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect()
}

fn ref_prefix(sid: &str) -> String {
    format!("refs/kelpie/{}/", safe_id(sid))
}

fn ref_numbers(repo: &Repo, sid: &str) -> Vec<u32> {
    let prefix = ref_prefix(sid);
    repo.git(&["for-each-ref", "--format=%(refname)", &prefix])
        .map(|out| out.lines().filter_map(|l| l.strip_prefix(prefix.as_str()).and_then(|n| n.parse().ok())).collect())
        .unwrap_or_default()
}

fn delete_refs(top: &Path, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let input: String = names.iter().map(|r| format!("delete {r}\n")).collect();
    if let Err(e) = git_raw(top, None, &["update-ref", "--stdin"], Some(input.into_bytes())) {
        tracing::warn!("delete checkpoint refs: {e:#}");
    }
}

fn session_refs(top: &Path, sid: &str) -> Vec<String> {
    git(top, None, &["for-each-ref", "--format=%(refname)", &ref_prefix(sid)]).map(|o| o.lines().map(str::to_string).collect()).unwrap_or_default()
}

struct Snap {
    commit: String,
    tree: String,
    unreadable: Vec<String>,
}

/// Snapshots the working folder unless its tree equals `prev_tree`.
fn snapshot(repo: &Repo, tmp_index: &Path, sid: &str, n: u32, prev_tree: Option<&str>) -> Result<Option<Snap>> {
    let (tree, unreadable) = working_tree(repo, tmp_index)?;
    if prev_tree == Some(tree.as_str()) {
        return Ok(None);
    }
    let message = format!("kelpie checkpoint {} #{n}", safe_id(sid));
    // `-p HEAD` is resolved by git itself; a repository without a commit yet has none.
    let commit_tree = |with_head: bool| {
        let mut cmd = command(&repo.top, None);
        cmd.args(["commit-tree", &tree, "-m", &message]);
        if with_head {
            cmd.args(["-p", "HEAD"]);
        }
        for (k, v) in [("AUTHOR", "Kelpie"), ("COMMITTER", "Kelpie")] {
            cmd.env(format!("GIT_{k}_NAME"), v).env(format!("GIT_{k}_EMAIL"), "kelpie@localhost");
        }
        run(cmd, None)
    };
    let mut out = commit_tree(true)?;
    if !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("HEAD") {
        out = commit_tree(false)?;
    }
    if !out.status.success() {
        bail!("git commit-tree: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let commit = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // The old value of zeros: a number is never written twice.
    let name = format!("{}{n}", ref_prefix(sid));
    repo.git(&["update-ref", &name, &commit, ZERO_OID])?;
    Ok(Some(Snap { commit, tree, unreadable }))
}

fn tree_of(repo: &Repo, commit: &str) -> Result<String> {
    repo.git(&["rev-parse", &format!("{commit}^{{tree}}")])
}

// ---------- the store ----------

pub fn repo_key(top: &Path) -> String {
    crate::meta::key(&top.to_string_lossy())
}

impl Checkpoints {
    /// `emit` broadcasts `checkpoint.*` events to the clients.
    pub fn new(data_dir: &Path, emit: Emit) -> Checkpoints {
        let dir = data_dir.join("checkpoints");
        let _ = std::fs::create_dir_all(&dir);
        let enabled = std::fs::read(dir.join("config.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| v.get("enabled").and_then(Value::as_bool))
            .unwrap_or(DEFAULT_ENABLED);
        // Left over from a daemon that was killed mid-snapshot: stale index copies and their locks.
        let _ = std::fs::remove_dir_all(dir.join("tmp"));
        let cp = Checkpoints {
            dir,
            emit,
            look_required: AtomicBool::new(true),
            clock: Box::new(crate::session::now_millis),
            keep: KEEP,
            enabled: AtomicBool::new(enabled),
            inner: Mutex::new(Inner::default()),
            queues: Mutex::new(HashMap::new()),
            repo_locks: Mutex::new(HashMap::new()),
            repos: Mutex::new(HashMap::new()),
            last_timing: Mutex::new((Duration::ZERO, Duration::ZERO, None)),
        };
        // Every session's records are needed to tell whose work overlapped.
        let logs: HashMap<String, Log> = cp.stored_sessions().into_iter().filter_map(|stem| cp.load_log(&stem).map(|l| (l.session.clone(), l))).collect();
        cp.inner.lock().logs = logs;
        cp
    }

    /// The folder key of the repository a session's snapshots belong to.
    pub fn session_repo_key(&self, sid: &str) -> Option<String> {
        self.with_log(sid, |l| l.map(|l| l.key.clone()))
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Turns snapshots on or off for every repository (`repo`: switches one back on instead).
    pub fn set_enabled(&self, enabled: Option<bool>, repo: Option<&str>) {
        if let Some(top) = repo {
            let key = repo_key(Path::new(top));
            let mut g = self.inner.lock();
            g.disabled.remove(&key);
            g.slow.remove(&key);
        }
        if let Some(on) = enabled {
            self.enabled.store(on, Ordering::Relaxed);
            if let Err(e) = write_atomic(&self.dir.join("config.json"), json!({"enabled": on}).to_string().as_bytes()) {
                tracing::warn!("save checkpoint settings: {e:#}");
            }
        }
    }

    fn log_path(&self, sid: &str) -> PathBuf {
        self.dir.join(format!("{}.json", safe_id(sid)))
    }

    fn tmp_index(&self, sid: &str) -> PathBuf {
        self.dir.join("tmp").join(format!("{}.index", safe_id(sid)))
    }

    fn load_log(&self, sid: &str) -> Option<Log> {
        std::fs::read(self.log_path(sid)).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn save_log(&self, log: &Log) {
        match serde_json::to_vec_pretty(log) {
            Ok(b) => {
                if let Err(e) = write_atomic(&self.log_path(&log.session), &b) {
                    tracing::warn!("save checkpoints of {}: {e:#}", log.session);
                }
            }
            Err(e) => tracing::warn!("save checkpoints of {}: {e}", log.session),
        }
    }

    /// The log of `sid`, from memory or disk.
    fn with_log<R>(&self, sid: &str, f: impl FnOnce(Option<&mut Log>) -> R) -> R {
        let mut g = self.inner.lock();
        if !g.logs.contains_key(sid) {
            if let Some(log) = self.load_log(sid) {
                g.logs.insert(sid.to_string(), log);
            }
        }
        f(g.logs.get_mut(sid))
    }

    /// The agent started working on a turn (for the window other sessions are compared in).
    pub fn work_started(&self, sid: &str, at: u64) {
        self.inner.lock().work_start.entry(sid.to_string()).or_insert(at);
    }

    /// Takes (and forgets) when the turn that just ended began.
    pub fn take_work_start(&self, sid: &str, fallback: u64) -> u64 {
        self.inner.lock().work_start.remove(sid).unwrap_or(fallback)
    }

    fn repo_lock(&self, key: &str) -> Arc<Mutex<()>> {
        self.repo_locks.lock().entry(key.to_string()).or_default().clone()
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    #[cfg(test)]
    fn with_clock(mut self, clock: impl Fn() -> u64 + Send + Sync + 'static) -> Self {
        self.clock = Box::new(clock);
        self
    }

    /// The opened repository for `top`, remembered so a snapshot does not ask git again.
    fn repo(&self, top: &Path, key: &str) -> Result<Arc<Repo>> {
        if let Some(r) = self.repos.lock().get(key) {
            return Ok(r.clone());
        }
        let r = Arc::new(Repo::open(top)?);
        self.repos.lock().insert(key.to_string(), r.clone());
        Ok(r)
    }

    fn repo_for(&self, log: &Log) -> Result<Arc<Repo>> {
        self.repo(Path::new(&log.repo), &log.key).context("the repository of these checkpoints is not available")
    }

    /// Queues a snapshot behind the repository's others. A snapshot of that session already
    /// waiting covers this one too (it sees everything up to now): the waiting job's window is
    /// widened to take in this one's.
    pub fn submit(self: &Arc<Self>, job: Job) {
        if !self.enabled() {
            return;
        }
        let Some(top) = find_top(Path::new(&job.cwd)) else { return };
        let key = repo_key(&top);
        let (off, closed) = {
            let g = self.inner.lock();
            (g.disabled.contains_key(&key), g.dropped.contains(&job.sid))
        };
        if closed && job.kind != Kind::Start {
            // Closed between the turn ending and this job: its work still counts as a stretch.
            self.add_window(&job.sid, &top, &key, job.started, job.at);
        }
        if off || closed {
            return;
        }
        let mut queues = self.queues.lock();
        match queues.get_mut(&key) {
            Some(q) => coalesce(&mut q.jobs, job),
            None => {
                queues.insert(key.clone(), Queue { jobs: VecDeque::from([job]) });
                let me = self.clone();
                if let Err(e) = std::thread::Builder::new().name("kelpie-checkpoint".into()).spawn(move || me.drain(&key)) {
                    tracing::warn!("start checkpoint worker: {e}");
                }
            }
        }
    }

    fn drain(&self, key: &str) {
        loop {
            let job = {
                let mut queues = self.queues.lock();
                match queues.get_mut(key).and_then(|q| q.jobs.pop_front()) {
                    Some(j) => j,
                    None => {
                        queues.remove(key);
                        return;
                    }
                }
            };
            let lock = self.repo_lock(key);
            let _held = lock.lock();
            if let Err(e) = self.run_job(&job) {
                tracing::warn!("checkpoint of {} skipped: {e:#}", job.sid);
            }
        }
    }

    /// One snapshot, retried a few times (another git process may hold a lock). Only the attempt
    /// that worked is timed.
    pub fn run_job(&self, job: &Job) -> Result<()> {
        let top = find_top(Path::new(&job.cwd)).ok_or_else(|| anyhow!("{} is not in a git repository", job.cwd))?;
        let key = repo_key(&top);
        let repo = self.repo(&top, &key)?;
        if self.inner.lock().dropped.contains(&job.sid) {
            // The session closed while this job waited: what it did still counts as a stretch.
            if job.kind != Kind::Start {
                self.add_window(&job.sid, &repo.top, &key, job.started, job.at);
            }
            return Ok(());
        }
        if job.kind == Kind::Start {
            return self.run_start(job, &repo, &key, &top);
        }
        // Where this session's history stands: the last snapshot, and the next number.
        let (prev, n) = self.position(&repo, &key, &job.sid)?;
        let tmp = self.tmp_index(&job.sid);
        let mut outcome = Err(anyhow!("not tried"));
        let (mut read_at, mut took, mut cpu) = (0, Duration::ZERO, Duration::ZERO);
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(150 * 3u64.pow(attempt as u32 - 1)));
            }
            read_at = self.now();
            let t = Instant::now();
            take_cpu();
            outcome = snapshot(&repo, &tmp, &job.sid, n, prev.as_ref().map(|r| r.tree.as_str()));
            took = t.elapsed();
            cpu = take_cpu();
            if outcome.is_ok() {
                break;
            }
        }
        let snap = match outcome {
            Ok(s) => s,
            Err(e) => {
                // No snapshot, yet the session may have changed any file since its last one.
                self.note_gap(&job.sid, &repo, &key, prev.as_ref().map_or(job.started, |p| p.read_at), self.now());
                return Err(e);
            }
        };
        self.note_duration(&key, &top, took, cpu);
        let Some(snap) = snap else { return Ok(()) };
        let ended = self.now();
        // Nothing before it: this snapshot is where the history starts, whatever was asked for.
        let kind = if prev.is_none() { Kind::Baseline } else { job.kind };
        let mut rec = record_from(&repo, n, kind, snap, prev.as_ref());
        rec.at = job.at.max(ended);
        rec.read_at = read_at;
        rec.started = job.started.min(job.at);
        rec.source = job.source.clone();
        // Read long after the turn ended: the folder may hold the next turn's edits too.
        rec.approximate = job.approximate || (kind != Kind::Baseline && ended.saturating_sub(job.at) > DELAYED_MS);
        let dropped = self.push_record(&job.sid, &repo, &key, rec.clone());
        delete_refs(&repo.top, &dropped);
        (self.emit)("checkpoint.updated", json!({"session": job.sid, "n": rec.n, "kind": rec.kind, "files": rec.files}));
        Ok(())
    }

    /// Work of a session that has no record (it closed with a snapshot still queued, or its record
    /// was refused): a stretch in which it may have touched any file, kept even in a tombstone.
    fn add_window(&self, sid: &str, top: &Path, key: &str, from_hint: u64, at: u64) {
        let (log, closed) = {
            let mut g = self.inner.lock();
            let closed = g.dropped.contains(sid);
            let log = g.logs.entry(sid.to_string()).or_insert_with(|| Log { session: sid.to_string(), repo: top.to_string_lossy().into_owned(), key: key.to_string(), closed, ..Log::default() });
            if log.key != key {
                return;
            }
            let from = log.records.last().map_or(from_hint, |r| r.read_at);
            log.windows.push(Window { from, at });
            if log.windows.len() > WINDOWS_CAP {
                log.windows.remove(0);
            }
            (log.clone(), closed)
        };
        self.save_log(&log);
        if closed {
            self.prune_tombstones();
        }
    }

    /// A turn is starting: look at the folder. Anything that differs from where the session's
    /// previous snapshot ended was changed between turns (by the user's editor, a shell, another
    /// session), and stays flagged for undo even if the agent edits the same file afterwards. A
    /// late look only flags more. Nothing is committed or referenced.
    fn run_start(&self, job: &Job, repo: &Repo, key: &str, top: &Path) -> Result<()> {
        let (prev, _) = self.position(repo, key, &job.sid)?;
        let Some(prev) = prev else { return Ok(()) };
        let tmp = self.tmp_index(&job.sid);
        let t = Instant::now();
        take_cpu();
        let looked = working_tree(repo, &tmp);
        let (wall, cpu) = (t.elapsed(), take_cpu());
        self.note_duration(key, top, wall, cpu);
        let start = match looked.and_then(|(tree, _)| if tree == prev.tree { Ok(None) } else { diff_changes(repo, &prev.tree, &tree).map(Some) }) {
            Ok(None) => None,
            Ok(Some(changes)) => Some(StartInfo { truncated: changes.len() > PATHS_CAP, paths: changes.iter().take(PATHS_CAP).map(|c| c.path.clone()).collect() }),
            // Not knowing is not "nothing changed".
            Err(e) => {
                tracing::warn!("look at the start of a turn of {}: {e:#}", job.sid);
                Some(StartInfo { paths: Vec::new(), truncated: true })
            }
        };
        let log = {
            let mut g = self.inner.lock();
            let Some(log) = g.logs.get_mut(&job.sid).filter(|l| l.key == key) else { return Ok(()) };
            log.start = start;
            log.looked_after = Some(prev.n);
            log.clone()
        };
        self.save_log(&log);
        Ok(())
    }

    /// A snapshot failed: remember that the session may have touched any file in that stretch,
    /// and that its next record is a guess.
    fn note_gap(&self, sid: &str, repo: &Repo, key: &str, from: u64, at: u64) {
        let log = {
            let mut g = self.inner.lock();
            if g.dropped.contains(sid) {
                return;
            }
            let log = g.logs.entry(sid.to_string()).or_insert_with(|| Log { session: sid.to_string(), repo: repo.top.to_string_lossy().into_owned(), key: key.to_string(), ..Log::default() });
            if log.key != key {
                return;
            }
            log.windows.push(Window { from, at });
            if log.windows.len() > WINDOWS_CAP {
                log.windows.remove(0);
            }
            log.unsure = true;
            log.clone()
        };
        self.save_log(&log);
    }

    /// The session's latest record and the number the next snapshot takes (the highest number
    /// in the log or among the refs, plus one: it survives a lost log and a restart).
    fn position(&self, repo: &Repo, key: &str, sid: &str) -> Result<(Option<Record>, u32)> {
        let synced = self.inner.lock().refs_synced.contains(sid);
        let refs = if synced { Vec::new() } else { ref_numbers(repo, sid) };
        let mut moved_from: Option<(String, PathBuf)> = None;
        let result = self.with_log(sid, |log| {
            if let Some(l) = &log {
                if !l.records.is_empty() && l.key != key {
                    moved_from = Some((l.session.clone(), PathBuf::from(&l.repo)));
                }
            }
            let mut last = log.as_ref().filter(|l| l.key == key).and_then(|l| l.records.last().cloned());
            let logged = log.as_ref().filter(|l| l.key == key).and_then(|l| l.records.iter().map(|r| r.n).max());
            let top_ref = refs.iter().copied().max();
            let highest = logged.into_iter().chain(top_ref).max();
            // The log is gone but its refs are not: continue from the newest ref.
            if last.is_none() {
                if let Some(h) = top_ref {
                    last = rebuild_tail(repo, sid, h);
                }
            }
            (last, highest.map_or(0, |h| h + 1))
        });
        if let Some((old_sid, old_top)) = moved_from {
            // The folder moved to another repository: the old refs belong to a history we left.
            delete_refs(&old_top, &session_refs(&old_top, &old_sid));
            self.inner.lock().logs.remove(sid);
            let _ = std::fs::remove_file(self.log_path(sid));
            self.inner.lock().refs_synced.insert(sid.to_string());
            return Ok((None, 0));
        }
        self.inner.lock().refs_synced.insert(sid.to_string());
        Ok(result)
    }

    /// Appends a record, trims to the last `keep` turns and saves; returns refs to delete.
    fn push_record(&self, sid: &str, repo: &Repo, key: &str, mut rec: Record) -> Vec<String> {
        if self.inner.lock().dropped.contains(sid) {
            // Closed meanwhile: the snapshot is not kept, but the stretch it covers is.
            self.add_window(sid, &repo.top, key, rec.from, rec.at);
            return vec![format!("{}{}", ref_prefix(sid), rec.n)];
        }
        let (log, gone) = {
            let mut g = self.inner.lock();
            if !g.logs.contains_key(sid) {
                let log = self.load_log(sid).unwrap_or_default();
                g.logs.insert(sid.to_string(), log);
            }
            let log = g.logs.get_mut(sid).expect("inserted above");
            if log.key != key {
                *log = Log { session: sid.to_string(), repo: repo.top.to_string_lossy().into_owned(), key: key.to_string(), ..Log::default() };
            }
            if log.unsure {
                rec.approximate = true;
                log.unsure = false;
            }
            if rec.kind == Kind::Turn {
                if let Some(start) = log.start.take() {
                    rec.between = start.paths;
                    rec.between_truncated = start.truncated;
                }
                // No look came after this turn's base (jobs merged, a look skipped or failed, a
                // restart): what changed between turns is unknown, which is not "nothing".
                if self.look_required.load(Ordering::Relaxed) && (rec.base_n.is_none() || log.looked_after != rec.base_n) {
                    rec.between_truncated = true;
                }
            }
            log.records.push(rec);
            let mut gone = Vec::new();
            while log.records.len() > self.keep + 1 {
                let old = log.records.remove(0);
                gone.push(format!("{}{}", ref_prefix(sid), old.n));
            }
            (log.clone(), gone)
        };
        // The disk is written after the lock is released: a slow disk never stalls the ticker.
        self.save_log(&log);
        gone
    }

    /// Whether the repository's index is big enough for its size to make a snapshot slow.
    fn index_is_big(&self, key: &str) -> bool {
        self.repos.lock().get(key).is_some_and(|r| std::fs::metadata(&r.index).is_ok_and(|m| m.len() >= BIG_INDEX))
    }

    /// Counts slow snapshots; a repository that was slow twice in a row stops being snapshotted
    /// (until the daemon restarts). A busy machine says nothing about the repository, and wall time
    /// grows with the load (a five-file repository took 2.3 s of wall time and 0.2 s of git CPU time
    /// with every core busy), so the verdict is on git's CPU time. Only when that was not counted
    /// (non-Windows, or a counting that missed the real git) the wall time is judged, and then only
    /// for a repository whose index is big and while a trivial git starts promptly.
    fn note_duration(&self, key: &str, top: &Path, took: Duration, cpu: Duration) {
        let counted = cpu >= MIN_COUNTED_CPU;
        let probe = (!counted && took >= SLOW && self.index_is_big(key)).then(machine_busy_probe);
        *self.last_timing.lock() = (took, cpu, probe);
        let slow = if counted { cpu >= SLOW } else { probe.is_some_and(|p| p <= BUSY) };
        if took >= SLOW || slow {
            tracing::info!(wall_ms = took.as_millis() as u64, cpu_ms = cpu.as_millis() as u64, probe_ms = probe.map(|p| p.as_millis() as u64), slow, "snapshot of {} took long", top.display());
        }
        let mut g = self.inner.lock();
        if !slow {
            g.slow.remove(key);
            return;
        }
        let count = g.slow.entry(key.to_string()).or_insert(0);
        *count += 1;
        if *count >= SLOW_LIMIT && !g.disabled.contains_key(key) {
            let shown = if counted { cpu } else { took };
            let reason = format!("A snapshot of this repository took {:.1} s; turn snapshots are off for it", shown.as_secs_f32());
            g.disabled.insert(key.to_string(), Disabled { at: self.now(), reason: reason.clone(), repo: top.to_string_lossy().into_owned() });
            drop(g);
            (self.emit)("checkpoint.disabled", json!({"repo": top.to_string_lossy(), "reason": reason}));
        }
    }

    // ---------- reading ----------

    /// `why`: the reason a session has no snapshots, when it has none and will not get any.
    pub fn list(&self, sid: &str, why: Option<&str>) -> Value {
        let (repo, records) = self.with_log(sid, |log| match log {
            Some(l) => (Some(l.repo.clone()), l.records.clone()),
            None => (None, Vec::new()),
        });
        let (repo_off, reason) = {
            let g = self.inner.lock();
            let off = repo.as_deref().map(|r| repo_key(Path::new(r))).and_then(|k| g.disabled.get(&k).map(|d| d.reason.clone()));
            (off.is_some(), off)
        };
        let why = match (why, self.enabled(), reason) {
            (Some(w), _, _) => Some(w.to_string()),
            (None, false, _) => Some("off".to_string()),
            (None, true, Some(_)) => Some("slow".to_string()),
            _ => None,
        };
        // The oldest record is only the base of the first turn once the history is full.
        let first_is_base = records.len() > self.keep;
        json!({"repo": repo, "enabled": self.enabled(), "repo_off": repo_off, "why": why, "records": records, "first_is_base": first_is_base})
    }

    pub fn status(&self) -> Value {
        let g = self.inner.lock();
        json!({"enabled": self.enabled(), "disabled": g.disabled.values().collect::<Vec<_>>()})
    }

    fn record(&self, sid: &str, n: u32) -> Result<(Record, Log)> {
        self.with_log(sid, |log| {
            let log = log.ok_or_else(|| anyhow!("no checkpoints for this session"))?;
            let rec = log.records.iter().find(|r| r.n == n).cloned().ok_or_else(|| anyhow!("checkpoint {n} is gone (only the last {} turns are kept)", self.keep))?;
            Ok((rec, log.clone()))
        })
    }

    /// Files one turn changed.
    pub fn diff(&self, sid: &str, n: u32) -> Result<Value> {
        let (rec, log) = self.record(sid, n)?;
        let base = rec.base.clone().ok_or_else(|| anyhow!("checkpoint {n} starts the history: nothing to compare with"))?;
        let repo = self.repo_for(&log)?;
        let mut files = diff_changes(&repo, &base, &rec.commit)?;
        let total = files.len();
        files.truncate(DIFF_CAP);
        Ok(json!({"files": files, "total": total, "base_n": rec.base_n}))
    }

    /// One file as it was at snapshot `n` (`before`: at the snapshot that turn was measured from).
    pub fn blob(&self, sid: &str, n: u32, path: &str, as_base64: bool, before: bool) -> Result<Value> {
        safe_rel(path)?;
        let (rec, log) = self.record(sid, n)?;
        let repo = self.repo_for(&log)?;
        let commit = if before { rec.base.clone().ok_or_else(|| anyhow!("checkpoint {n} starts the history: nothing before it"))? } else { rec.commit.clone() };
        let spec = format!("{commit}:{path}");
        let kind = match repo.git(&["cat-file", "-t", &spec]) {
            Ok(k) => k,
            Err(_) => return Ok(json!({"exists": false})),
        };
        if kind != "blob" {
            return Ok(json!({"exists": false}));
        }
        let size: u64 = repo.git(&["cat-file", "-s", &spec])?.parse().unwrap_or(0);
        if size > BLOB_CAP {
            return Ok(json!({"exists": true, "size": size, "too_large": true}));
        }
        let bytes = git_raw(&repo.top, None, &["cat-file", "blob", &spec], None)?;
        let binary = bytes.contains(&0);
        if binary || as_base64 {
            use base64::Engine;
            return Ok(json!({"exists": true, "size": size, "binary": binary, "base64": base64::engine::general_purpose::STANDARD.encode(&bytes)}));
        }
        Ok(json!({"exists": true, "size": size, "binary": false, "text": String::from_utf8_lossy(&bytes)}))
    }

    /// Who touched which files in the repository `sid` works in, from every session's turns.
    pub fn owners(&self, sid: &str, names: &dyn Fn(&str) -> String) -> Value {
        let g = self.inner.lock();
        let Some(mine) = g.logs.get(sid).map(|l| l.key.clone()).or_else(|| self.load_log(sid).map(|l| l.key)) else {
            return json!({"files": []});
        };
        struct Touch {
            turns: u32,
            last: u64,
            /// The newest snapshot of that session that touched the file.
            n: u32,
        }
        let mut files: BTreeMap<String, BTreeMap<String, Touch>> = BTreeMap::new();
        for log in g.logs.values().filter(|l| l.key == mine) {
            for r in log.records.iter().filter(|r| r.kind != Kind::Baseline) {
                for p in &r.paths {
                    let t = files.entry(p.clone()).or_default().entry(log.session.clone()).or_insert(Touch { turns: 0, last: 0, n: r.n });
                    t.turns += 1;
                    if r.at >= t.last {
                        t.last = r.at;
                        t.n = r.n;
                    }
                }
            }
        }
        let mut rows: Vec<Value> = files
            .into_iter()
            .map(|(path, by)| {
                let last = by.values().map(|t| t.last).max().unwrap_or(0);
                let sessions: Vec<Value> = by.iter().map(|(s, t)| json!({"session": s, "name": names(s), "turns": t.turns, "last": t.last, "n": t.n})).collect();
                json!({"path": path, "last": last, "shared": by.len() > 1, "sessions": sessions})
            })
            .collect();
        rows.sort_by(|a, b| b["last"].as_u64().cmp(&a["last"].as_u64()));
        let total = rows.len();
        rows.truncate(DIFF_CAP);
        json!({"files": rows, "total": total})
    }

    // ---------- undo ----------

    /// Every file of the turn with what undoing it would do and whether that is safe.
    pub fn undo_plan(&self, sid: &str, n: u32, ctx: &UndoCtx, names: &dyn Fn(&str) -> String) -> Result<Value> {
        let (rec, log) = self.record(sid, n)?;
        let repo = self.repo_for(&log)?;
        let lock = self.repo_lock(&log.key);
        let _held = lock.lock();
        let items = self.plan(&repo, &rec, &log, ctx, names)?;
        Ok(json!({"items": items, "owner_working": ctx.owner_working, "n": rec.n, "base_n": rec.base_n, "unreadable": rec.unreadable}))
    }

    fn plan(&self, repo: &Repo, rec: &Record, log: &Log, ctx: &UndoCtx, names: &dyn Fn(&str) -> String) -> Result<Vec<PlanItem>> {
        if rec.kind == Kind::Baseline {
            bail!("a baseline is where the history starts: there is nothing to undo");
        }
        let base = rec.base.clone().ok_or_else(|| anyhow!("no earlier snapshot to restore"))?;
        let changes = diff_changes(repo, &base, &rec.commit)?;
        // What the folder holds now, against what the turn left: a difference is a later change.
        let (now, _) = working_tree(repo, &self.tmp_index(&log.session))?;
        let drift: HashSet<String> = diff_changes(repo, &rec.tree, &now)?.into_iter().map(|c| c.path).collect();
        // And what already equals the state before the turn (an earlier undo put it back).
        let undone_already: HashSet<String> = {
            let differs: HashSet<String> = diff_changes(repo, &base, &now)?.into_iter().map(|c| c.path).collect();
            changes.iter().filter(|c| !differs.contains(&c.path)).map(|c| c.path.clone()).collect()
        };
        // Other sessions whose stretch overlaps the stretch this diff covers, the idle time
        // before the turn included: what they changed in it is in this diff too.
        let (from, at) = (rec.from, rec.at);
        let overlapping: Vec<(String, Option<HashSet<String>>)> = {
            let g = self.inner.lock();
            g.logs
                .values()
                .filter(|l| l.key == log.key && l.session != log.session)
                .flat_map(|l| windows_of(l).into_iter().filter(|w| w.from < at && from < w.at).map(|w| (l.session.clone(), w.paths)).collect::<Vec<_>>())
                .collect()
        };
        // A file already older than the turn's start was not written by it: the user's editor,
        // a shell or a session that left no record changed it before.
        let older = |p: &Path| rec.kind == Kind::Turn && rec.started > STALE_SLACK_MS && mtime_ms(p).is_some_and(|m| m + STALE_SLACK_MS < rec.started);
        let stale = |path: &str| older(&repo.top.join(path));
        // A removed file has no time of its own: its folder's changes when the entry went.
        let removed_early = |path: &str| older(&repo.top.join(Path::new(path).parent().unwrap_or(Path::new(""))));
        // What this turn's snapshot or the one before could not read.
        let mut unreadable: HashSet<&str> = rec.unreadable.iter().map(String::as_str).collect();
        if let Some(b) = rec.base_n.and_then(|n| log.records.iter().find(|r| r.n == n)) {
            unreadable.extend(b.unreadable.iter().map(String::as_str));
        }
        let between: HashSet<&str> = rec.between.iter().map(String::as_str).collect();
        let mut items = Vec::new();
        for c in changes {
            let special = [&c.old_mode, &c.new_mode].iter().any(|m| matches!(m.as_str(), "120000" | "160000"));
            let (action, mut reason) = if special {
                ("skip", Some("a link or submodule: restore it yourself"))
            } else if undone_already.contains(&c.path) {
                ("skip", Some("already as it was before the turn"))
            } else if c.status == 'A' {
                ("delete", None)
            } else {
                ("restore", None)
            };
            let mut with: Vec<(String, String)> = Vec::new();
            for (other, paths) in &overlapping {
                if paths.as_ref().is_none_or(|p| p.contains(&c.path)) && !with.iter().any(|(s, _)| s == other) {
                    with.push((other.clone(), names(other)));
                }
            }
            // A session working there now has not reported what it touches: it may be this file.
            for (other, name) in &ctx.working_others {
                if !with.iter().any(|(s, _)| s == other) {
                    with.push((other.clone(), name.clone()));
                }
            }
            let state = if action == "skip" {
                "clean"
            } else if !with.is_empty() {
                "shared"
            } else if drift.contains(&c.path) {
                "modified"
            } else if rec.kind == Kind::Turn && (rec.between_truncated || between.contains(c.path.as_str())) {
                reason = Some("changed between turns, before this turn started");
                "modified"
            } else if unreadable.contains(c.path.as_str()) {
                reason = Some("could not be read when a snapshot was taken");
                "modified"
            } else if c.status != 'D' && stale(&c.path) {
                reason = Some("changed before this turn started");
                "modified"
            } else if c.status == 'D' && removed_early(&c.path) {
                reason = Some("removed before this turn started");
                "modified"
            } else {
                "clean"
            };
            items.push(PlanItem {
                path: c.path,
                action: action.into(),
                state: state.into(),
                with: with.into_iter().map(|(session, name)| json!({"session": session, "name": name})).collect(),
                reason: reason.map(str::to_string),
            });
        }
        Ok(items)
    }

    /// Restores the files of turn `n` (all of them or `paths`). Files that are shared with
    /// another session or changed since need their path in `confirm`; without it nothing is
    /// changed and the reply lists what needs a decision.
    pub fn undo(&self, sid: &str, req: &UndoRequest, ctx: &UndoCtx, names: &dyn Fn(&str) -> String) -> Result<Value> {
        let UndoRequest { n, paths, confirm, at } = *req;
        if ctx.owner_working {
            bail!("this session is still working: undo a turn once it is idle");
        }
        let (rec, log) = self.record(sid, n)?;
        let repo = self.repo_for(&log)?;
        let lock = self.repo_lock(&log.key);
        let _held = lock.lock();
        let items = self.plan(&repo, &rec, &log, ctx, names)?;
        let chosen: Vec<&PlanItem> = match paths {
            Some(list) => {
                for p in list {
                    if !items.iter().any(|i| &i.path == p) {
                        bail!("{p} is not part of this turn");
                    }
                }
                items.iter().filter(|i| list.contains(&i.path)).collect()
            }
            None => items.iter().collect(),
        };
        let needs: Vec<&PlanItem> = chosen.iter().copied().filter(|i| i.action != "skip" && i.state != "clean" && !confirm.contains(&i.path)).collect();
        if !needs.is_empty() {
            return Ok(json!({"applied": false, "needs_confirmation": needs}));
        }
        let base = rec.base.clone().ok_or_else(|| anyhow!("no earlier snapshot to restore"))?;
        let mut restore: Vec<&str> = Vec::new();
        let mut delete: Vec<&str> = Vec::new();
        let mut skipped: Vec<Value> = Vec::new();
        for i in &chosen {
            safe_rel(&i.path)?;
            match i.action.as_str() {
                "restore" => restore.push(&i.path),
                "delete" => delete.push(&i.path),
                _ => skipped.push(json!({"path": i.path, "reason": i.reason})),
            }
        }
        if !restore.is_empty() {
            let mut input = Vec::new();
            for p in &restore {
                input.extend_from_slice(p.as_bytes());
                input.push(0);
            }
            let source = format!("--source={base}");
            // `--worktree` alone: the index is not part of the restore. git still takes its lock
            // for a moment, so one retry when the user's own git command holds it.
            let args = ["restore", &source, "--worktree", "--pathspec-from-file=-", "--pathspec-file-nul"];
            if let Err(e) = git_raw(&repo.top, None, &args, Some(input.clone())) {
                if !e.to_string().contains("index.lock") {
                    return Err(e);
                }
                std::thread::sleep(Duration::from_millis(400));
                git_raw(&repo.top, None, &args, Some(input))?;
            }
        }
        for p in &delete {
            let file = repo.top.join(p);
            if behind_a_link(&repo.top, &file) {
                bail!("refusing to delete {p}: a folder on its path is a link");
            }
            match std::fs::remove_file(&file) {
                Ok(()) => remove_empty_parents(&repo.top, &file),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => bail!("delete {p}: {e}"),
            }
        }
        // The folder as it is now becomes the session's latest snapshot, so the next turn
        // does not include the undo.
        let applied = chosen.iter().filter(|i| i.action != "skip").map(|i| i.path.clone()).collect::<Vec<_>>();
        let (prev, next) = self.position(&repo, &log.key, sid)?;
        let tmp = self.tmp_index(sid);
        let read_at = self.now();
        if let Some(snap) = snapshot(&repo, &tmp, sid, next, prev.as_ref().map(|r| r.tree.as_str()))? {
            let mut r = record_from(&repo, next, Kind::Undo, snap, prev.as_ref());
            r.at = at.max(self.now());
            r.read_at = read_at;
            r.started = at;
            r.source = "undo".into();
            r.undo_of = Some(n);
            let gone = self.push_record(sid, &repo, &log.key, r);
            delete_refs(&repo.top, &gone);
            (self.emit)("checkpoint.updated", json!({"session": sid, "n": next, "kind": "undo", "files": applied.len()}));
        }
        Ok(json!({"applied": true, "files": applied, "skipped": skipped}))
    }

    // ---------- removal ----------

    /// A session ended: its refs go (first, so a crash never leaves refs nobody knows), its record
    /// becomes a tombstone with the windows and file lists only. Another session's undo still
    /// has to see that this one touched files. `now` ends the turn it was in the middle of.
    pub fn close_session(&self, sid: &str, now: u64) {
        // Where its snapshots live: its record, else a snapshot still waiting for it.
        let queued_top = || {
            let queues = self.queues.lock();
            queues.values().flat_map(|q| q.jobs.iter()).find(|j| j.sid == sid).and_then(|j| find_top(Path::new(&j.cwd)))
        };
        let key = self.inner.lock().logs.get(sid).map(|l| l.key.clone()).or_else(|| queued_top().map(|t| repo_key(&t)));
        let Some(key) = key else {
            let mut g = self.inner.lock();
            g.dropped.insert(sid.to_string());
            g.work_start.remove(sid);
            return;
        };
        // The repository's lock first: a snapshot of this session that is running finishes and is
        // recorded before anything is deleted.
        let lock = self.repo_lock(&key);
        let _held = lock.lock();
        // Snapshots still queued for it will never run: each becomes a stretch in which it may
        // have touched any file, so another session's undo still sees it.
        let jobs: Vec<Job> = {
            let mut queues = self.queues.lock();
            match queues.get_mut(&key) {
                Some(q) => {
                    let (mine, rest): (Vec<Job>, Vec<Job>) = q.jobs.drain(..).partition(|j| j.sid == sid);
                    q.jobs = rest.into();
                    mine
                }
                None => Vec::new(),
            }
        };
        let repo_dir = jobs.iter().find_map(|j| find_top(Path::new(&j.cwd)));
        let log = {
            let mut g = self.inner.lock();
            g.dropped.insert(sid.to_string());
            g.refs_synced.remove(sid);
            let started = g.work_start.remove(sid);
            if !g.logs.contains_key(sid) {
                let Some(top) = repo_dir else { return };
                g.logs.insert(sid.to_string(), Log { session: sid.to_string(), repo: top.to_string_lossy().into_owned(), key: key.clone(), ..Log::default() });
            }
            let log = g.logs.get_mut(sid).expect("inserted above");
            if log.closed {
                return;
            }
            let last = log.records.last().map(|r| r.read_at);
            for j in jobs.iter().filter(|j| j.kind != Kind::Start) {
                log.windows.push(Window { from: last.unwrap_or(j.started), at: j.at });
            }
            if let Some(from) = started {
                log.windows.push(Window { from, at: now });
            }
            let excess = log.windows.len().saturating_sub(WINDOWS_CAP);
            log.windows.drain(..excess);
            log.closed = true;
            log.clone()
        };
        delete_refs(Path::new(&log.repo), &session_refs(Path::new(&log.repo), sid));
        self.save_log(&log);
        drop(_held);
        self.prune_tombstones();
    }

    /// Forgets a session entirely: refs (taken from `git_root` when the working folder is gone),
    /// record and tombstone.
    fn remove_session(&self, sid: &str, git_root: Option<&Path>) {
        let log = {
            let mut g = self.inner.lock();
            g.dropped.insert(sid.to_string());
            g.work_start.remove(sid);
            g.refs_synced.remove(sid);
            g.logs.remove(sid).or_else(|| self.load_log(sid))
        };
        if let Some(log) = log {
            let lock = self.repo_lock(&log.key);
            let _held = lock.lock();
            let root = git_root.map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(&log.repo));
            delete_refs(&root, &session_refs(&root, sid));
        }
        let _ = std::fs::remove_file(self.log_path(sid));
    }

    /// The worktree was removed: the snapshots of every session that worked in it go.
    pub fn drop_worktree(&self, worktree: &str, git_root: Option<&str>) -> usize {
        let key = repo_key(Path::new(worktree));
        // The folder is usually gone by now, so its canonical key may not match the one taken
        // while it existed: the spelling the sessions started with counts too.
        let plain = |p: &str| p.replace('\\', "/").trim_end_matches('/').to_lowercase();
        let wanted = plain(worktree);
        let sids: Vec<String> = {
            let g = self.inner.lock();
            g.logs.values().filter(|l| l.key == key || plain(&l.repo) == wanted).map(|l| l.session.clone()).collect()
        };
        for sid in &sids {
            self.remove_session(sid, git_root.map(Path::new));
        }
        self.repos.lock().remove(&key);
        sids.len()
    }

    #[cfg(test)]
    fn keep_only_recent(&self, sid: &str, turns: usize) {
        if let Some(l) = self.inner.lock().logs.get_mut(sid) {
            let cut = l.records.len().saturating_sub(turns + 1);
            l.records.drain(..cut);
        }
    }

    /// Tombstones go once no live session's history reaches back to them.
    fn prune_tombstones(&self) {
        let gone: Vec<String> = {
            let mut g = self.inner.lock();
            let mut floors: HashMap<String, u64> = HashMap::new();
            for l in g.logs.values().filter(|l| !l.closed) {
                let reach = l.records.iter().filter(|r| r.kind != Kind::Baseline).map(|r| r.from).min().or_else(|| l.records.last().map(|r| r.read_at)).unwrap_or(0);
                let f = floors.entry(l.key.clone()).or_insert(u64::MAX);
                *f = (*f).min(reach);
            }
            let dead: Vec<String> = g
                .logs
                .values()
                .filter(|l| l.closed)
                .filter(|l| {
                    let ended = l.records.iter().map(|r| r.at).chain(l.windows.iter().map(|w| w.at)).max().unwrap_or(0);
                    floors.get(&l.key).is_none_or(|floor| ended <= *floor)
                })
                .map(|l| l.session.clone())
                .collect();
            for sid in &dead {
                g.logs.remove(sid);
            }
            dead
        };
        for sid in gone {
            let _ = std::fs::remove_file(self.log_path(&sid));
        }
    }

    fn stored_sessions(&self) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return Vec::new() };
        rd.filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let stem = name.strip_suffix(".json")?;
                (stem != "config" && stem != "disabled").then(|| stem.to_string())
            })
            .collect()
    }

    /// At startup: sessions that no longer exist are closed (their refs deleted) and old tombstones
    /// pruned. Only sessions this daemon recorded are touched, so another daemon's refs in the same
    /// repository stay.
    pub fn gc(&self, known: &HashSet<String>) -> usize {
        let gone: Vec<String> = {
            let g = self.inner.lock();
            g.logs.values().filter(|l| !l.closed && !known.iter().any(|k| safe_id(k) == safe_id(&l.session))).map(|l| l.session.clone()).collect()
        };
        for sid in &gone {
            self.close_session(sid, 0);
        }
        self.prune_tombstones();
        gone.len()
    }
}

#[derive(Debug, Clone, Serialize)]
struct PlanItem {
    path: String,
    /// restore | delete | skip
    action: String,
    /// clean | modified (changed since the turn) | shared (another session touched it)
    state: String,
    with: Vec<Value>,
    reason: Option<String>,
}

/// A window of another session's changes: `paths` is None when it may have touched any file.
struct Win {
    from: u64,
    at: u64,
    paths: Option<HashSet<String>>,
}

/// Every stretch in which this session may have changed files: its turns (whole diff window)
/// and the stretches where a snapshot failed.
fn windows_of(log: &Log) -> Vec<Win> {
    let mut out: Vec<Win> = log
        .records
        .iter()
        .filter(|r| r.kind != Kind::Baseline || r.from > 0)
        .map(|r| Win { from: r.from, at: r.at, paths: if r.truncated || !r.unreadable.is_empty() { None } else { Some(r.paths.iter().cloned().collect()) } })
        .collect();
    out.extend(log.windows.iter().map(|w| Win { from: w.from, at: w.at, paths: None }));
    out
}

/// Adds a job to the queue, or widens the waiting job of the same session and kind to cover it.
fn coalesce(jobs: &mut VecDeque<Job>, job: Job) {
    // The newest waiting job of the session and kind; a turn's end is not merged into one that waits
    // before the same session's look at a later turn's start (the look would then follow it).
    let at = jobs.iter().rposition(|j| j.sid == job.sid && j.kind == job.kind);
    let look_after = |p: usize| job.kind == Kind::Turn && jobs.iter().skip(p + 1).any(|j| j.sid == job.sid && j.kind == Kind::Start);
    match at {
        Some(p) if !look_after(p) => {
            let j = &mut jobs[p];
            // The windows come from the snapshots' own times; this is only for the "older than the turn" test.
            j.started = j.started.max(job.started);
            j.at = j.at.max(job.at);
            j.approximate |= job.approximate;
        }
        _ => jobs.push_back(job),
    }
}

fn mtime_ms(p: &Path) -> Option<u64> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64)
}

/// How long a trivial git start takes right now.
fn machine_busy_probe() -> Duration {
    let t = Instant::now();
    let mut cmd = command(&std::env::temp_dir(), None);
    cmd.arg("--version");
    let _ = run(cmd, None);
    t.elapsed()
}

/// A record for a fresh snapshot: where it starts from, what it changed. The caller sets the times.
fn record_from(repo: &Repo, n: u32, kind: Kind, snap: Snap, prev: Option<&Record>) -> Record {
    let mut rec = Record {
        n,
        kind,
        commit: snap.commit,
        tree: snap.tree,
        base: None,
        base_n: None,
        at: 0,
        read_at: 0,
        from: 0,
        started: 0,
        source: String::new(),
        approximate: false,
        files: 0,
        added: 0,
        removed: 0,
        paths: Vec::new(),
        truncated: false,
        unreadable: snap.unreadable,
        between: Vec::new(),
        between_truncated: false,
        undo_of: None,
    };
    // A baseline after earlier work (a restart) covers that work as a turn does, though it is not
    // a turn to undo: it has no base.
    if let Some(p) = prev {
        if kind != Kind::Baseline {
            rec.base = Some(p.commit.clone());
            rec.base_n = Some(p.n);
        }
        rec.from = p.read_at;
        match diff_changes(repo, &p.commit, &rec.commit) {
            Ok(changes) => fill_changes(&mut rec, &changes),
            Err(e) => tracing::warn!("diff of checkpoint {n}: {e:#}"),
        }
    }
    rec
}

/// Whether a folder between the working folder and `file` is a link (a delete would follow it).
fn behind_a_link(top: &Path, file: &Path) -> bool {
    file.ancestors().skip(1).take_while(|a| *a != top && a.starts_with(top)).any(|a| std::fs::symlink_metadata(a).is_ok_and(|m| m.file_type().is_symlink()))
}

fn fill_changes(rec: &mut Record, changes: &[Change]) {
    rec.files = changes.len() as u32;
    rec.added = changes.iter().filter_map(|c| c.added).sum();
    rec.removed = changes.iter().filter_map(|c| c.removed).sum();
    rec.truncated = changes.len() > PATHS_CAP;
    rec.paths = changes.iter().take(PATHS_CAP).map(|c| c.path.clone()).collect();
}

/// The newest snapshot found among the refs when the log was lost: enough to continue from.
fn rebuild_tail(repo: &Repo, sid: &str, n: u32) -> Option<Record> {
    let commit = repo.git(&["rev-parse", &format!("{}{n}", ref_prefix(sid))]).ok()?;
    let tree = tree_of(repo, &commit).ok()?;
    let now = crate::session::now_millis();
    Some(Record {
        n,
        kind: Kind::Baseline,
        commit,
        tree,
        base: None,
        base_n: None,
        at: now,
        // Unknown: the next snapshot's stretch then reaches back to the beginning of time.
        read_at: 0,
        from: 0,
        started: now,
        source: "restart".into(),
        approximate: false,
        files: 0,
        added: 0,
        removed: 0,
        paths: Vec::new(),
        truncated: false,
        unreadable: Vec::new(),
        between: Vec::new(),
        between_truncated: false,
        undo_of: None,
    })
}

/// Removes folders a deleted file left empty, never the working folder itself.
fn remove_empty_parents(top: &Path, file: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == top || !d.starts_with(top) || std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(test)]
mod tests;
