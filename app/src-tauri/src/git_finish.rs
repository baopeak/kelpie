//! Closing a worktree's life: the change summaries behind the Review queue and the overlap chips,
//! and the Finish sequence (merge-tree, checks in a scratch worktree, compare-and-swap advance of
//! the base branch, branch deletion only with proof, shared links restored when removal fails).
//!
//! The user's work is only ever added to: nothing here uses `--force` or `reset`, user hooks do not
//! run for this plumbing, and every ref moves with an expected old value.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::fs_ops::{err, guard, CmdResult};
use crate::git::{git, git_env, git_review, git_worktrees, parse_status, GitOutput};

const SCAN_TTL: Duration = Duration::from_secs(30);
const MAX_FILES: usize = 2000;
const MAX_UNTRACKED_COUNTED: usize = 500;
const CHECK_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// A commit that has to be signed may wait for a passphrase or a hardware key; not forever.
const SIGN_TIMEOUT: Duration = Duration::from_secs(120);
const OUTPUT_TAIL: usize = 6000;
/// Files per `git add` (command lines are limited to ~32 KB on Windows).
const ADD_CHUNK: usize = 100;
const ENV: [(&str, &str); 2] = [("LC_ALL", "C"), ("GIT_TERMINAL_PROMPT", "0")];

// ---------- plumbing ----------

/// Finish is Kelpie's own bookkeeping, so the user's commit and reference hooks do not run for it:
/// git looks for hooks in `/dev/null`, where nothing can be planted (also on Windows, where git
/// resolves it itself). No folder to empty, no window between emptying and use.
const NO_HOOKS: &str = "core.hooksPath=/dev/null";

async fn plumb_env(cwd: &Path, args: &[&str], extra: &[(&str, &str)]) -> CmdResult<GitOutput> {
    let mut full = vec!["-c", NO_HOOKS];
    full.extend_from_slice(args);
    let mut env = ENV.to_vec();
    env.extend_from_slice(extra);
    git_env(cwd, &full, &env).await
}

async fn plumb(cwd: &Path, args: &[&str]) -> CmdResult<GitOutput> {
    plumb_env(cwd, args, &[]).await
}

fn failure(out: &GitOutput) -> String {
    let e = out.stderr.trim();
    if e.is_empty() { String::from_utf8_lossy(&out.stdout).trim().to_string() } else { e.to_string() }
}

async fn plumb_ok(cwd: &Path, args: &[&str]) -> CmdResult<String> {
    let out = plumb(cwd, args).await?;
    if !out.ok {
        return Err(failure(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn is_sha(s: &str) -> bool {
    (s.len() == 40 || s.len() == 64) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn local_root(root: &str) -> CmdResult<PathBuf> {
    if crate::remote::is_remote(root) {
        return Err("finishing a worktree works on local folders only".into());
    }
    guard(root, "")
}

/// `refs/heads/<name>` for a valid branch name (never something git could read as an option).
async fn head_ref(dir: &Path, name: &str) -> CmdResult<String> {
    let full = format!("refs/heads/{}", name.trim());
    if name.trim().is_empty() || name.trim().starts_with('-') || !plumb(dir, &["check-ref-format", &full]).await?.ok {
        return Err(format!("\"{name}\" is not a valid branch name"));
    }
    Ok(full)
}

async fn commit_of(dir: &Path, rev: &str) -> CmdResult<String> {
    let spec = format!("{rev}^{{commit}}");
    plumb_ok(dir, &["rev-parse", "--verify", "--quiet", &spec]).await.map_err(|e| if e.is_empty() { format!("{rev} does not exist") } else { e })
}

/// Author/committer for commits made here: the user's own, or a neutral one when none is configured.
async fn identity(dir: &Path) -> Vec<(&'static str, &'static str)> {
    if plumb(dir, &["var", "GIT_COMMITTER_IDENT"]).await.is_ok_and(|o| o.ok) {
        return Vec::new();
    }
    vec![("GIT_AUTHOR_NAME", "Kelpie"), ("GIT_AUTHOR_EMAIL", "kelpie@localhost"), ("GIT_COMMITTER_NAME", "Kelpie"), ("GIT_COMMITTER_EMAIL", "kelpie@localhost")]
}

/// The branch a worktree's HEAD is on (`None`: detached).
async fn current_branch(wt: &Path) -> CmdResult<Option<String>> {
    let out = plumb(wt, &["symbolic-ref", "-q", "HEAD"]).await?;
    Ok(out.ok.then(|| String::from_utf8_lossy(&out.stdout).trim().trim_start_matches("refs/heads/").to_string()).filter(|b| !b.is_empty()))
}

/// Finish works on the branch the worktree is really on, never on a name remembered earlier: a commit
/// made on a detached HEAD (or another branch) would be lost with the worktree.
async fn require_on_branch(wt: &Path, branch: &str) -> CmdResult<()> {
    match current_branch(wt).await? {
        Some(b) if b == branch => Ok(()),
        Some(b) => Err(format!("the worktree is on branch \"{b}\", not \"{branch}\"; nothing was changed")),
        None => Err(format!("the worktree is on a detached HEAD, not on \"{branch}\": commits made there would be lost with the worktree. Create a branch for them first (git switch -c <name>); nothing was changed")),
    }
}

// ---------- cancel, timeouts, process trees ----------

/// Worktrees whose Finish was cancelled. Cancel belongs to one Finish: it is cleared when a Finish
/// begins and ends (`finish_reset`, and when `finish_integrate` returns), never carried to the next.
static CANCELLED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

fn cancel_key(worktree: &str) -> String {
    worktree.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

fn is_cancelled(key: &str) -> bool {
    CANCELLED.lock().unwrap().contains(key)
}

/// Resolves once Cancel was pressed for this Finish.
async fn cancelled(key: String) {
    while !is_cancelled(&key) {
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// Stops this worktree's running Finish at its next step boundary. Before the base moves that means
/// nothing changed; after it, the rest of the run is only cleanup and is reported as it is.
#[tauri::command]
pub fn finish_cancel(worktree: String) {
    CANCELLED.lock().unwrap().insert(cancel_key(&worktree));
}

/// A Finish begins or ends: forget any Cancel left for the worktree.
#[tauri::command]
pub fn finish_reset(worktree: String) {
    CANCELLED.lock().unwrap().remove(&cancel_key(&worktree));
}

/// Everything a check starts, killed together when this is dropped (a timeout, Cancel, or the
/// check ending while something it started still runs).
#[cfg(windows)]
struct Tree(Option<usize>);

#[cfg(windows)]
impl Tree {
    fn new() -> Tree {
        use windows::Win32::System::JobObjects::*;
        // SAFETY: plain Win32 calls on a handle created here and closed in `drop`.
        unsafe {
            let Ok(job) = CreateJobObjectW(None, windows::core::PCWSTR::null()) else { return Tree(None) };
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const core::ffi::c_void, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            if set.is_err() {
                let _ = windows::Win32::Foundation::CloseHandle(job);
                return Tree(None);
            }
            Tree(Some(job.0 as usize))
        }
    }

    fn add(&self, child: &tokio::process::Child) {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        if let (Some(job), Some(raw)) = (self.0, child.raw_handle()) {
            // SAFETY: both handles are alive for the call.
            let _ = unsafe { AssignProcessToJobObject(HANDLE(job as *mut core::ffi::c_void), HANDLE(raw)) };
        }
    }
}

#[cfg(windows)]
impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(job) = self.0 {
            // SAFETY: closing the handle created in `new` (KILL_ON_JOB_CLOSE ends the processes in it).
            let _ = unsafe { windows::Win32::Foundation::CloseHandle(windows::Win32::Foundation::HANDLE(job as *mut core::ffi::c_void)) };
        }
    }
}

#[cfg(not(windows))]
struct Tree(std::cell::Cell<Option<u32>>);

#[cfg(not(windows))]
impl Tree {
    fn new() -> Tree {
        Tree(std::cell::Cell::new(None))
    }

    fn add(&self, child: &tokio::process::Child) {
        self.0.set(child.id());
    }
}

#[cfg(not(windows))]
impl Drop for Tree {
    fn drop(&mut self) {
        if let Some(pid) = self.0.get() {
            // The check was started as the leader of its own process group.
            let _ = std::process::Command::new("kill").args(["-KILL", &format!("-{pid}")]).output();
        }
    }
}

// ---------- change summaries ----------

#[derive(Deserialize)]
pub struct ReviewTarget {
    path: String,
    /// The branch the worktree will be merged into.
    base: String,
    /// Only which files changed (the overlap watch): no line counts, no commit count.
    #[serde(default)]
    names_only: bool,
}

#[derive(Serialize, Clone, Default, Debug)]
pub struct ReviewChanges {
    path: String,
    ok: bool,
    error: Option<String>,
    branch: Option<String>,
    /// Committed since the base, staged, modified and untracked: one entry per path.
    files: Vec<String>,
    truncated: bool,
    commits: u32,
    uncommitted: u32,
    added: u32,
    deleted: u32,
    /// The change touches `.kelpie.yaml`: its `check:` list is not trusted for this review.
    config_changed: bool,
}

struct Cached {
    fingerprint: String,
    at: Instant,
    value: ReviewChanges,
}

static CACHE: LazyLock<Mutex<HashMap<String, Cached>>> = LazyLock::new(Default::default);
/// Scans actually run (cache misses): the overlap and queue budgets are counted in these.
pub static SCANS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn gitdir_of(wt: &Path) -> Option<PathBuf> {
    let dot = wt.join(".git");
    if dot.is_dir() {
        return Some(dot);
    }
    let text = std::fs::read_to_string(&dot).ok()?;
    let rel = text.lines().find_map(|l| l.strip_prefix("gitdir:"))?.trim();
    let p = Path::new(rel);
    Some(if p.is_absolute() { p.to_path_buf() } else { wt.join(p) })
}

fn stamp(p: &Path) -> String {
    match std::fs::metadata(p) {
        Ok(m) => format!("{:?}/{}", m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()), m.len()),
        Err(_) => "-".into(),
    }
}

/// What a summary depends on, read without running git: HEAD, the index, the branch's and the
/// base's references. `None` when it cannot be worked out (no caching then).
fn fingerprint(wt: &Path, base: &str) -> Option<String> {
    let gd = gitdir_of(wt)?;
    let common = std::fs::read_to_string(gd.join("commondir")).ok().map(|c| gd.join(c.trim())).unwrap_or_else(|| gd.clone());
    let head = std::fs::read_to_string(gd.join("HEAD")).ok()?;
    let mut parts = vec![head.trim().to_string(), stamp(&gd.join("index")), stamp(&common.join("packed-refs")), stamp(&common.join("refs/heads").join(base))];
    // References kept in the reftable format live in one table list instead of files.
    parts.push(stamp(&common.join("reftable").join("tables.list")));
    if let Some(r) = head.trim().strip_prefix("ref: ") {
        parts.push(stamp(&common.join(r)));
    }
    Some(parts.join("|"))
}

const CONFIG_FILES: [&str; 2] = [".kelpie.yaml", ".erka.yaml"];

fn count_lines(p: &Path) -> u32 {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(p) else { return 0 };
    let mut buf = Vec::new();
    if f.take(1 << 20).read_to_end(&mut buf).is_err() || buf.iter().take(8000).any(|b| *b == 0) {
        return 0; // binary
    }
    let n = buf.iter().filter(|b| **b == b'\n').count() + usize::from(buf.last().is_some_and(|b| *b != b'\n'));
    n as u32
}

async fn scan(wt: &Path, base: &str, names_only: bool) -> CmdResult<ReviewChanges> {
    SCANS.fetch_add(1, Ordering::Relaxed);
    let base_ref = format!("refs/heads/{base}");
    let range = format!("{base_ref}..HEAD");
    let diff_args = ["diff", if names_only { "--name-only" } else { "--numstat" }, "--no-renames", "-z", "--merge-base", base_ref.as_str()];
    let status_args = ["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"];
    let count_args = ["rev-list", "--count", range.as_str()];
    // Three parallel processes on the review pool (a diff against the merge base needs no separate merge-base call).
    let (status, mut diff, ahead) = tokio::join!(git_review(wt, &status_args), git_review(wt, &diff_args), async {
        if names_only { Ok(None) } else { git_review(wt, &count_args).await.map(Some) }
    });
    let (status, ahead) = (status?, ahead?);
    if !status.ok {
        return Err(failure(&status));
    }
    let mut diff_out = diff.as_mut().ok().filter(|d| d.ok).map(|d| std::mem::take(&mut d.stdout));
    if diff_out.is_none() {
        // A git older than 2.30 has no --merge-base: ask for the merge base first.
        let mb = git_review(wt, &["merge-base", &base_ref, "HEAD"]).await?;
        if !mb.ok {
            return Err(format!("{base} and this branch share no history"));
        }
        let mb = String::from_utf8_lossy(&mb.stdout).trim().to_string();
        let d = git_review(wt, &["diff", if names_only { "--name-only" } else { "--numstat" }, "--no-renames", "-z", &mb]).await?;
        if !d.ok {
            return Err(failure(&d));
        }
        diff_out = Some(d.stdout);
    }
    let st = parse_status(&status.stdout);
    let mut files = BTreeSet::new();
    let (mut added, mut deleted) = (0u32, 0u32);
    for rec in String::from_utf8_lossy(&diff_out.unwrap_or_default()).split('\0').filter(|r| !r.is_empty()) {
        if names_only {
            files.insert(rec.to_string());
            continue;
        }
        let mut f = rec.splitn(3, '\t');
        let (a, d, path) = (f.next().unwrap_or("-"), f.next().unwrap_or("-"), f.next().unwrap_or(""));
        added += a.parse::<u32>().unwrap_or(0);
        deleted += d.parse::<u32>().unwrap_or(0);
        if !path.is_empty() {
            files.insert(path.to_string());
        }
    }
    let untracked: Vec<String> = st.files.iter().filter(|f| f.index == '?').map(|f| f.path.clone()).collect();
    for f in &st.files {
        files.insert(f.path.clone());
    }
    let counted = if names_only {
        0
    } else {
        let dir = wt.to_path_buf();
        tokio::task::spawn_blocking(move || untracked.iter().take(MAX_UNTRACKED_COUNTED).map(|p| count_lines(&dir.join(p))).sum::<u32>()).await.map_err(err)?
    };
    let config_changed = files.iter().any(|f| CONFIG_FILES.contains(&f.as_str()));
    let total = files.len();
    Ok(ReviewChanges {
        path: wt.to_string_lossy().into_owned(),
        ok: true,
        error: None,
        branch: st.branch,
        files: files.into_iter().take(MAX_FILES).collect(),
        truncated: total > MAX_FILES,
        commits: ahead.map(|a| String::from_utf8_lossy(&a.stdout).trim().parse().unwrap_or(0)).unwrap_or(0),
        uncommitted: st.files.len() as u32,
        added: added + counted,
        deleted,
        config_changed,
    })
}

async fn changes_of(target: ReviewTarget, force: bool) -> ReviewChanges {
    let key = format!("{}|{}|{}", target.path, target.base, if target.names_only { "names" } else { "full" });
    let wt = PathBuf::from(&target.path);
    let print = fingerprint(&wt, &target.base);
    if let (false, Some(print)) = (force, &print) {
        if let Some(c) = CACHE.lock().unwrap().get(&key).filter(|c| &c.fingerprint == print && c.at.elapsed() < SCAN_TTL) {
            return c.value.clone();
        }
    }
    let value = match local_root(&target.path) {
        Ok(_) => scan(&wt, &target.base, target.names_only).await,
        Err(e) => Err(e),
    }
    .unwrap_or_else(|e| ReviewChanges { path: target.path.clone(), ok: false, error: Some(e), ..Default::default() });
    if let (true, Some(fingerprint)) = (value.ok, print) {
        CACHE.lock().unwrap().insert(key, Cached { fingerprint, at: Instant::now(), value: value.clone() });
    }
    value
}

/// What each worktree changed against its base. Answers from a cache while HEAD, the index and the
/// two references are unchanged (and for 30 s: edits of tracked files do not touch the index);
/// `force` skips it (a turn just ended).
#[tauri::command]
pub async fn review_changes(targets: Vec<ReviewTarget>, force: bool) -> CmdResult<Vec<ReviewChanges>> {
    let mut set = tokio::task::JoinSet::new();
    for (i, t) in targets.into_iter().enumerate() {
        set.spawn(async move { (i, changes_of(t, force).await) });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        out.push(r.map_err(err)?);
    }
    out.sort_by_key(|(i, _)| *i);
    Ok(out.into_iter().map(|(_, c)| c).collect())
}

// ---------- Finish: inspect, WIP commit ----------

#[derive(Serialize)]
pub struct FinishFile {
    path: String,
    /// `M` modified, `A` added, `D` deleted, `?` untracked, `U` conflicted.
    status: char,
    /// Bytes on disk (0 for a deleted file).
    size: u64,
}

#[derive(Serialize)]
pub struct FinishInspect {
    /// The branch the worktree's HEAD is on, read here (`None`: detached HEAD).
    branch: Option<String>,
    uncommitted: Vec<FinishFile>,
    /// Folders with a git repository of their own inside the worktree: they cannot be committed as files.
    embedded: Vec<String>,
    conflicts: Vec<String>,
    operation: Option<&'static str>,
    /// The merge of the branch's commits into the base would conflict in these files.
    merge_conflicts: Vec<String>,
    /// Where the base branch is checked out, and whether that folder has changes to tracked files.
    base_checkout: Option<String>,
    base_checkout_dirty: bool,
}

fn letter(f: &crate::git::GitFile) -> char {
    match (f.index, f.worktree) {
        ('U', _) => 'U',
        ('?', _) => '?',
        ('A', _) => 'A',
        (_, 'D') | ('D', _) => 'D',
        _ => 'M',
    }
}

/// The folder where `branch` is checked out, if any.
async fn checkout_of(root: &str, branch: &str) -> Option<PathBuf> {
    let list = git_worktrees(root.to_string()).await.ok()?;
    list.into_iter().find(|w| !w.bare && !w.prunable && w.branch.as_deref() == Some(branch)).map(|w| PathBuf::from(w.path))
}

/// No change to a tracked file, staged or not. Untracked and ignored files are not a change here:
/// only the ones the merge would overwrite matter (`collisions`).
async fn tracked_clean(dir: &Path) -> CmdResult<bool> {
    // Refresh stat data first: a file only touched since the last status would read as modified to read-tree.
    let _ = plumb(dir, &["update-index", "-q", "--refresh"]).await;
    let out = git(dir, &["status", "--porcelain", "-z", "--untracked-files=no"]).await?;
    if !out.ok {
        return Err(failure(&out));
    }
    Ok(out.stdout.is_empty())
}

/// Paths the merge `old` -> `new` adds that already exist in `dir` (an untracked or ignored file, or a
/// file where a folder would go): `read-tree -u` would replace them without a word.
async fn collisions(dir: &Path, old: &str, new: &str) -> CmdResult<Vec<String>> {
    let out = plumb(dir, &["diff", "--name-only", "-z", "--no-renames", "--diff-filter=A", old, new]).await?;
    if !out.ok {
        return Err(failure(&out));
    }
    let mut hit = Vec::new();
    for p in String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()) {
        let full = dir.join(p);
        let blocked = std::fs::symlink_metadata(&full).is_ok()
            || Path::new(p).ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()).any(|a| std::fs::symlink_metadata(dir.join(a)).is_ok_and(|m| !m.is_dir()));
        if blocked {
            hit.push(p.to_string());
        }
    }
    Ok(hit)
}

struct WorktreeState {
    status: crate::git::GitStatus,
    embedded: Vec<String>,
}

async fn worktree_state(wt: &Path) -> CmdResult<WorktreeState> {
    let status = git(wt, &["status", "--porcelain=v2", "-z", "--untracked-files=all"]).await?;
    if !status.ok {
        return Err(failure(&status));
    }
    let st = parse_status(&status.stdout);
    let embedded = st.files.iter().filter(|f| f.index == '?' && f.path.ends_with('/')).map(|f| f.path.clone()).collect();
    Ok(WorktreeState { status: st, embedded })
}

#[tauri::command]
pub async fn finish_inspect(root: String, worktree: String, base: String) -> CmdResult<FinishInspect> {
    let repo = local_root(&root)?;
    let wt = local_root(&worktree)?;
    let base_ref = head_ref(&repo, &base).await?;
    commit_of(&repo, &base_ref).await?;
    let branch = current_branch(&wt).await?;
    let state = worktree_state(&wt).await?;
    if state.status.truncated {
        return Err("more than 5000 files changed; commit them yourself first".into());
    }
    let (base_checkout, base_checkout_dirty) = match checkout_of(&root, &base).await {
        Some(dir) => {
            let dirty = !tracked_clean(&dir).await?;
            (Some(dir.to_string_lossy().into_owned()), dirty)
        }
        None => (None, false),
    };
    let merge_conflicts = match &branch {
        Some(b) if *b != base => match head_ref(&repo, b).await {
            Ok(branch_ref) => merge_trees(&repo, &[&base_ref, &branch_ref]).await.map(|(_, c)| c).unwrap_or_default(),
            Err(_) => Vec::new(),
        },
        _ => Vec::new(),
    };
    let st = &state.status;
    Ok(FinishInspect {
        branch,
        conflicts: st.files.iter().filter(|f| f.index == 'U').map(|f| f.path.clone()).collect(),
        uncommitted: st
            .files
            .iter()
            .map(|f| FinishFile { path: f.path.clone(), status: letter(f), size: std::fs::metadata(wt.join(&f.path)).map(|m| if m.is_file() { m.len() } else { 0 }).unwrap_or(0) })
            .collect(),
        embedded: state.embedded,
        operation: crate::git_sync::operation(&wt).await,
        merge_conflicts,
        base_checkout,
        base_checkout_dirty,
    })
}

#[derive(Serialize, Debug)]
pub struct WipResult {
    /// The WIP commit (`None`: nothing was committed).
    commit: Option<String>,
    /// Changed files the user did not confirm: when there are any, nothing is committed.
    extra: Vec<String>,
}

/// One `git` call that may wait for a passphrase or a hardware key (a signed commit): bounded by a
/// timeout and by Cancel, and the process is killed when either happens.
async fn run_signed(dir: &Path, args: &[&str], env: &[(&str, &str)], key: &str) -> CmdResult<GitOutput> {
    tokio::select! {
        r = tokio::time::timeout(SIGN_TIMEOUT, plumb_env(dir, args, env)) => r.unwrap_or_else(|_| Err("it took longer than two minutes (is a passphrase or a security key waiting?)".into())),
        _ = cancelled(key.to_string()) => Err("cancelled".into()),
    }
}

/// Commits exactly the files the user confirmed (staged, modified, untracked) as one commit on the
/// branch the worktree is on. Anything else changed meanwhile is reported in `extra` and nothing is
/// committed. Refuses while a merge/rebase or conflicts are open, and for repositories inside the worktree.
#[tauri::command]
pub async fn finish_wip_commit(worktree: String, branch: String, message: String, paths: Vec<String>) -> CmdResult<WipResult> {
    let wt = local_root(&worktree)?;
    require_on_branch(&wt, &branch).await?;
    if let Some(op) = crate::git_sync::operation(&wt).await {
        return Err(format!("a {op} is in progress in this worktree; finish or abort it first"));
    }
    let message = message.trim();
    if message.is_empty() {
        return Err("the commit needs a message".into());
    }
    let state = worktree_state(&wt).await?;
    if state.status.truncated {
        return Err("more than 5000 files changed; commit them yourself first".into());
    }
    if !state.embedded.is_empty() {
        return Err(format!("these folders hold a git repository of their own and cannot be committed as files: {}", state.embedded.join(", ")));
    }
    if state.status.files.iter().any(|f| f.index == 'U') {
        return Err("the worktree has unresolved conflicts".into());
    }
    let confirmed: BTreeSet<&str> = paths.iter().map(String::as_str).collect();
    let extra: Vec<String> = state.status.files.iter().filter(|f| !confirmed.contains(f.path.as_str())).map(|f| f.path.clone()).collect();
    if !extra.is_empty() {
        return Ok(WipResult { commit: None, extra });
    }
    if state.status.files.is_empty() {
        return Ok(WipResult { commit: None, extra });
    }
    // Literal pathspecs: a file named like a glob or a magic prefix is still just that file. A deletion
    // that is already staged has nothing left to add.
    let mut specs: Vec<String> = Vec::new();
    for f in state.status.files.iter().filter(|f| !(f.index == 'D' && f.worktree == '.')) {
        specs.push(format!(":(literal){}", f.path));
        if let Some(o) = &f.orig_path {
            specs.push(format!(":(literal){o}"));
        }
    }
    for chunk in specs.chunks(ADD_CHUNK) {
        let mut args = vec!["add", "-A", "--"];
        args.extend(chunk.iter().map(String::as_str));
        let add = plumb(&wt, &args).await?;
        if !add.ok {
            return Err(failure(&add));
        }
    }
    let id = identity(&wt).await;
    let out = run_signed(&wt, &["commit", "--no-verify", "-q", "-m", message], &id, &cancel_key(&worktree)).await.map_err(|e| format!("the WIP commit failed: {e}"))?;
    if !out.ok {
        return Err(failure(&out));
    }
    commit_of(&wt, "HEAD").await.map(|c| WipResult { commit: Some(c), extra })
}

// ---------- Finish: merge, checks, advance ----------

#[derive(Deserialize)]
pub struct IntegrateArgs {
    /// The repository (main checkout) the branches belong to.
    root: String,
    /// The worktree the branch is checked out in; its HEAD must be on `branch`.
    worktree: String,
    branch: String,
    base: String,
    /// `squash`, `merge` or `rebase`.
    strategy: String,
    message: String,
    /// Commands already approved by the user for this repository (from the base branch's file).
    checks: Vec<String>,
    copy: Vec<String>,
    link: Vec<String>,
    /// The user chose to run the base's checks although the branch edits `.kelpie.yaml`.
    allow_config_change: bool,
    delete_branch: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct CheckResult {
    command: String,
    ok: bool,
    code: Option<i32>,
    output: String,
}

#[derive(Serialize, Debug)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Integrated {
    Merged {
        commit: String,
        /// The branch's tip when it was merged (to put the branch back if the worktree cannot be removed).
        tip: String,
        /// Where the main checkout was brought up to date (None: the base is not checked out anywhere).
        checkout_updated: Option<String>,
        branch_deleted: bool,
        /// Why the branch was kept, when it was.
        branch_note: Option<String>,
        checks: Vec<CheckResult>,
    },
    /// The branch already is in the base: nothing to merge (the branch may still be deleted).
    Nothing { tip: String, branch_deleted: bool, branch_note: Option<String> },
    Conflict { files: Vec<String> },
    CheckFailed { checks: Vec<CheckResult>, cleanup_error: Option<String> },
    /// Someone moved the base branch while Finish was working: nothing was changed.
    BaseMoved,
    /// The folder where the base is checked out has changes to tracked files, or the merge would
    /// overwrite files it holds (`files`): nothing was changed.
    CheckoutDirty { path: String, files: Vec<String> },
    /// The worktree has changes that were not committed (files that appeared after the list was confirmed).
    WorktreeChanged { files: Vec<String> },
    /// The branch edits `.kelpie.yaml` and the checks were not allowed to run: nothing was changed.
    ConfigBlocked { files: Vec<String> },
    /// Cancel was pressed: nothing was changed.
    Cancelled,
}

enum Candidate {
    Ready { commit: String },
    Conflict(Vec<String>),
    Nothing,
}

/// How commits are made: whose identity, and whether to sign them (`commit-tree` ignores
/// `commit.gpgSign`, so the flag is passed).
struct Maker<'a> {
    id: &'a [(&'a str, &'a str)],
    sign: bool,
    /// The Finish whose Cancel ends a signing that waits.
    key: &'a str,
}

impl Maker<'_> {
    async fn commit_tree(&self, repo: &Path, rest: &[&str], author: &[(&str, &str)]) -> CmdResult<String> {
        let mut args = vec!["commit-tree"];
        if self.sign {
            args.push("-S");
        }
        args.extend_from_slice(rest);
        let mut env: Vec<(&str, &str)> = self.id.to_vec();
        env.extend_from_slice(author);
        let out = run_signed(repo, &args, &env, self.key).await.map_err(|e| if self.sign { format!("signing the commit failed: {e}; nothing was changed") } else { e })?;
        if !out.ok {
            return Err(if self.sign { format!("signing the commit failed: {}; nothing was changed", failure(&out)) } else { failure(&out) });
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Whether the repository signs commits (`commit.gpgSign`, from any config level).
async fn signs_commits(repo: &Path) -> bool {
    plumb(repo, &["config", "--type=bool", "--get", "commit.gpgSign"]).await.is_ok_and(|o| o.ok && String::from_utf8_lossy(&o.stdout).trim() == "true")
}

/// `(tree, conflicted paths)` of a `merge-tree --write-tree` run.
fn parse_merge_tree(out: &GitOutput) -> Result<(String, Vec<String>), String> {
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let tree = lines.next().unwrap_or("").trim().to_string();
    if !is_sha(&tree) {
        return Err(failure(out));
    }
    let files: BTreeSet<String> = lines.take_while(|l| !l.is_empty()).map(str::to_string).collect();
    if out.ok {
        return Ok((tree, Vec::new()));
    }
    Ok((tree, if files.is_empty() { vec!["(unknown)".into()] } else { files.into_iter().collect() }))
}

async fn merge_trees(repo: &Path, args: &[&str]) -> CmdResult<(String, Vec<String>)> {
    let mut full = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
    full.extend_from_slice(args);
    parse_merge_tree(&plumb(repo, &full).await?).map_err(|e| format!("merge-tree: {e}"))
}

async fn is_ancestor(repo: &Path, a: &str, b: &str) -> CmdResult<bool> {
    Ok(plumb(repo, &["merge-base", "--is-ancestor", a, b]).await?.ok)
}

/// Replays the branch's own commits one by one on top of `base` (each as a three-way merge against
/// its parent), keeping their authors and messages. No working tree is touched.
async fn replay(repo: &Path, base: &str, tip: &str, maker: &Maker<'_>) -> CmdResult<Candidate> {
    let range = format!("{base}..{tip}");
    if plumb_ok(repo, &["rev-list", "--merges", "--count", &range]).await? != "0" {
        return Err("the branch contains merge commits; use squash or merge instead of rebase".into());
    }
    let list = plumb_ok(repo, &["rev-list", "--reverse", &range]).await?;
    let mut cur = base.to_string();
    for c in list.lines() {
        let parent = plumb_ok(repo, &["rev-parse", "--verify", &format!("{c}^")]).await.map_err(|_| "the branch starts at a root commit; use squash or merge".to_string())?;
        let (tree, conflicts) = merge_trees(repo, &[&format!("--merge-base={parent}"), &cur, c]).await?;
        if !conflicts.is_empty() {
            return Ok(Candidate::Conflict(conflicts));
        }
        let cur_tree = plumb_ok(repo, &["rev-parse", &format!("{cur}^{{tree}}")]).await?;
        if tree == cur_tree {
            continue; // already there: the commit becomes empty
        }
        let meta = plumb_ok(repo, &["log", "-1", "--format=%an%x1f%ae%x1f%aI%x1f%B", c]).await?;
        let mut f = meta.splitn(4, '\x1f');
        let (an, ae, ad, body) = (f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or(""), f.next().unwrap_or("").trim());
        cur = maker
            .commit_tree(repo, &[&tree, "-p", &cur, "-m", if body.is_empty() { "(no message)" } else { body }], &[("GIT_AUTHOR_NAME", an), ("GIT_AUTHOR_EMAIL", ae), ("GIT_AUTHOR_DATE", ad)])
            .await?;
    }
    if cur == base {
        return Ok(Candidate::Nothing);
    }
    Ok(Candidate::Ready { commit: cur })
}

async fn candidate(repo: &Path, strategy: &str, base: &str, tip: &str, message: &str, maker: &Maker<'_>) -> CmdResult<Candidate> {
    if is_ancestor(repo, tip, base).await? {
        return Ok(Candidate::Nothing);
    }
    if strategy == "rebase" {
        return replay(repo, base, tip, maker).await;
    }
    let (tree, conflicts) = merge_trees(repo, &[base, tip]).await?;
    if !conflicts.is_empty() {
        return Ok(Candidate::Conflict(conflicts));
    }
    if tree == plumb_ok(repo, &["rev-parse", &format!("{base}^{{tree}}")]).await? {
        return Ok(Candidate::Nothing);
    }
    let message = if message.trim().is_empty() { "Merge" } else { message.trim() };
    let mut args = vec![tree.as_str(), "-p", base];
    if strategy == "merge" {
        args.extend(["-p", tip]);
    }
    args.extend(["-m", message]);
    Ok(Candidate::Ready { commit: maker.commit_tree(repo, &args, &[]).await? })
}

/// Words of a command line; double quotes group, nothing else is special (no shell runs it).
fn tokens(line: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (Vec::new(), String::new(), false, false);
    for ch in line.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The program a command names: a path as written, or the first match on PATH (with the
/// extensions Windows adds). Never the working folder, which holds the candidate's files.
fn resolve_program(name: &str, cwd: &Path) -> Option<PathBuf> {
    if name.contains(['/', '\\']) {
        let p = cwd.join(name);
        return p.is_file().then_some(p);
    }
    let exts: Vec<String> = if cfg!(windows) {
        let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        let mut v: Vec<String> = pathext.split(';').filter(|e| !e.is_empty()).map(str::to_string).collect();
        if Path::new(name).extension().is_some() {
            v.insert(0, String::new());
        }
        v
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        for ext in &exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// The last `OUTPUT_TAIL` characters of a stream, without keeping the rest in memory.
async fn read_tail<R: AsyncRead + Unpin>(mut r: R) -> Vec<u8> {
    let (mut buf, mut chunk) = (Vec::new(), [0u8; 8192]);
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > OUTPUT_TAIL * 4 {
                    let cut = buf.len() - OUTPUT_TAIL * 2;
                    buf.drain(..cut);
                }
            }
        }
    }
    buf
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let n = text.chars().count();
    if n <= OUTPUT_TAIL { text.into_owned() } else { text.chars().skip(n - OUTPUT_TAIL).collect() }
}

/// Runs one approved command. The bool says Cancel ended it. Whatever it started is killed with it.
async fn run_check(dir: &Path, line: &str, timeout: Duration, stop: impl std::future::Future<Output = ()>) -> (CheckResult, bool) {
    let fail = |output: String| (CheckResult { command: line.to_string(), ok: false, code: None, output }, false);
    let words = tokens(line);
    let Some(program) = words.first().and_then(|w| resolve_program(w, dir)) else {
        return fail(format!("command not found: {}", words.first().map(String::as_str).unwrap_or("")));
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(&words[1..])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .env("NoDefaultCurrentDirectoryInExePath", "1") // cmd.exe would look in the candidate's folder first
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    #[cfg(unix)]
    cmd.process_group(0);
    let tree = Tree::new();
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(e.to_string()),
    };
    tree.add(&child);
    let out = tokio::spawn(read_tail(child.stdout.take().expect("piped")));
    let errs = tokio::spawn(read_tail(child.stderr.take().expect("piped")));
    let (status, note, was_cancelled) = tokio::select! {
        s = child.wait() => (s.ok(), None, false),
        _ = tokio::time::sleep(timeout) => (None, Some(format!("timed out after {} minutes and stopped", timeout.as_secs() / 60)), false),
        _ = stop => (None, Some("cancelled".to_string()), true),
    };
    if status.is_none() {
        // The whole tree, not just the program: a shell's children keep running (and holding the folder) otherwise.
        drop(tree);
        let _ = child.kill().await;
    } else {
        drop(tree);
    }
    let grace = Duration::from_secs(3);
    let mut all = tokio::time::timeout(grace, out).await.ok().and_then(Result::ok).unwrap_or_default();
    all.extend(tokio::time::timeout(grace, errs).await.ok().and_then(Result::ok).unwrap_or_default());
    let mut output = tail(&all);
    if let Some(n) = note {
        output = format!("{output}\n{n}");
    }
    (CheckResult { command: line.to_string(), ok: status.is_some_and(|s| s.success()), code: status.and_then(|s| s.code()), output }, was_cancelled)
}

/// Check worktrees a crashed Kelpie left behind (`kelpie-finish-<pid>-…` in the temp folder, the process
/// gone): they hold live links to shared folders, so the links go first (never followed), then the
/// folder, then git's record.
async fn prune_stale_scratch(repo: &Path) {
    let Ok(list) = git_worktrees(repo.to_string_lossy().into_owned()).await else { return };
    let temp = std::fs::canonicalize(std::env::temp_dir()).ok();
    let mut removed = false;
    for w in list.iter().skip(1) {
        let path = PathBuf::from(&w.path);
        let Some(pid) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_prefix("kelpie-finish-")).and_then(|r| r.split('-').next()).and_then(|p| p.parse::<u32>().ok()) else { continue };
        let in_temp = path.parent().and_then(|p| std::fs::canonicalize(p).ok()) == temp && temp.is_some();
        if !in_temp || pid == std::process::id() || !crate::sys_stats::processes_alive(vec![pid]).await.unwrap_or_else(|_| vec![pid]).is_empty() {
            continue;
        }
        let dir = path.clone();
        let _ = tokio::task::spawn_blocking(move || {
            crate::git::unlink_shared(&dir);
            std::fs::remove_dir_all(&dir)
        })
        .await;
        removed = true;
    }
    if removed {
        let _ = plumb(repo, &["worktree", "prune"]).await;
    }
}

/// Runs the approved commands in a scratch worktree checked out at `commit`, which is deleted
/// afterwards (its files are removed directly: no forced `git worktree remove`). Stops at the
/// first failure or Cancel. Returns the results, whether Cancel ended it, and a note when the
/// scratch folder could not be removed.
async fn run_checks(repo: &Path, commit: &str, checks: &[String], copy: &[String], link: &[String], key: &str) -> CmdResult<(Vec<CheckResult>, bool, Option<String>)> {
    prune_stale_scratch(repo).await;
    let nanos = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let scratch = std::env::temp_dir().join(format!("kelpie-finish-{}-{nanos}", std::process::id()));
    let scratch_arg = scratch.to_string_lossy().into_owned();
    plumb_ok(repo, &["worktree", "add", "--detach", "--quiet", &scratch_arg, commit]).await.map_err(|e| format!("could not set up the check folder: {e}"))?;
    let (r, c, l, s) = (repo.to_path_buf(), copy.to_vec(), link.to_vec(), scratch.clone());
    // Ignored folders the checks need (node_modules, .env), brought in the way a new worktree gets them.
    let _ = tokio::task::spawn_blocking(move || crate::worktree_files::materialize(r, s, c, l)).await;
    let mut results = Vec::new();
    let mut stopped = false;
    for line in checks {
        let (r, was_cancelled) = run_check(&scratch, line, CHECK_TIMEOUT, cancelled(key.to_string())).await;
        let ok = r.ok;
        results.push(r);
        if was_cancelled {
            stopped = true;
        }
        if !ok {
            break;
        }
    }
    let dir = scratch.clone();
    let removed = tokio::task::spawn_blocking(move || {
        crate::git::unlink_shared(&dir);
        let mut last = None;
        for _ in 0..5 {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => return None,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
                Err(e) => last = Some(e.to_string()),
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        last
    })
    .await
    .map_err(err)?;
    let _ = plumb(repo, &["worktree", "prune"]).await;
    Ok((results, stopped, removed.map(|e| format!("{}: {e}", scratch.display()))))
}

/// Moves `base_ref` from `expected` to `new` only if it still is at `expected`.
async fn advance_base(repo: &Path, base_ref: &str, new: &str, expected: &str, note: &str) -> CmdResult<bool> {
    let out = plumb(repo, &["update-ref", "-m", note, base_ref, new, expected]).await?;
    if out.ok {
        return Ok(true);
    }
    match commit_of(repo, base_ref).await {
        Ok(now) if now != expected => Ok(false),
        _ => Err(failure(&out)),
    }
}

/// Deletes `branch_ref` (at `tip`) only when its content is in `base_ref`: its tip is an ancestor,
/// or merging it into the base would change nothing (squash and rebase leave no ancestry). Returns
/// why it was kept otherwise. A worktree that has the branch checked out is detached at the tip first
/// (its files do not change): a worktree on a deleted branch reads as full of staged files and could
/// not be removed without force.
async fn delete_if_merged(repo: &Path, base_ref: &str, branch_ref: &str, tip: &str, worktree: Option<&Path>) -> CmdResult<Option<String>> {
    let proven = if is_ancestor(repo, tip, base_ref).await? {
        true
    } else {
        let base_tree = plumb_ok(repo, &["rev-parse", &format!("{base_ref}^{{tree}}")]).await?;
        let (tree, conflicts) = merge_trees(repo, &[base_ref, tip]).await?;
        conflicts.is_empty() && tree == base_tree
    };
    if !proven {
        return Ok(Some("the branch has changes the base does not contain".into()));
    }
    if let Some(wt) = worktree {
        let detach = plumb(wt, &["switch", "--detach", "--quiet", tip]).await?;
        if !detach.ok {
            return Ok(Some(format!("the worktree could not be detached from it: {}", failure(&detach))));
        }
    }
    let out = plumb(repo, &["update-ref", "-d", branch_ref, tip]).await?;
    if !out.ok {
        // Still the worktree's branch: put it back on it.
        if let Some(wt) = worktree {
            let _ = plumb(wt, &["switch", "--quiet", branch_ref.trim_start_matches("refs/heads/")]).await;
        }
        return Ok(Some(failure(&out)));
    }
    if let Some(name) = branch_ref.strip_prefix("refs/heads/") {
        let _ = plumb(repo, &["config", "--remove-section", &format!("branch.{name}")]).await;
    }
    Ok(None)
}

/// Files of `.kelpie.yaml` / `.erka.yaml` that differ between the base and the candidate.
async fn config_edits(repo: &Path, base: &str, commit: &str) -> CmdResult<Vec<String>> {
    let out = plumb(repo, &["diff", "--name-only", "-z", "--no-renames", base, commit, "--", ".kelpie.yaml", ".erka.yaml"]).await?;
    if !out.ok {
        return Err(failure(&out));
    }
    Ok(String::from_utf8_lossy(&out.stdout).split('\0').filter(|p| !p.is_empty()).map(str::to_string).collect())
}

#[tauri::command]
pub async fn finish_integrate(args: IntegrateArgs) -> CmdResult<Integrated> {
    let key = cancel_key(&args.worktree);
    let mut moved = false;
    let result = integrate(args, &key, &mut moved).await;
    // An error before the base moved while Cancel was pressed is the cancel; after it, it is whatever it is.
    let cancelled_early = !moved && is_cancelled(&key);
    // This Finish is over: its Cancel must not reach the next one.
    CANCELLED.lock().unwrap().remove(&key);
    match result {
        Err(_) if cancelled_early => Ok(Integrated::Cancelled),
        r => r,
    }
}

async fn integrate(args: IntegrateArgs, key: &str, moved: &mut bool) -> CmdResult<Integrated> {
    let repo = local_root(&args.root)?;
    let wt = local_root(&args.worktree)?;
    if !matches!(args.strategy.as_str(), "squash" | "merge" | "rebase") {
        return Err(format!("unknown strategy {}", args.strategy));
    }
    let (base_ref, branch_ref) = (head_ref(&repo, &args.base).await?, head_ref(&repo, &args.branch).await?);
    if base_ref == branch_ref {
        return Err("the branch and its base are the same".into());
    }
    // The branch the worktree is really on, and nothing in it that was not committed: both are
    // checked here, at the boundary, whatever the dialog believed.
    require_on_branch(&wt, &args.branch).await?;
    let state = worktree_state(&wt).await?;
    if !state.status.files.is_empty() {
        return Ok(Integrated::WorktreeChanged { files: state.status.files.iter().map(|f| f.path.clone()).collect() });
    }
    let (base_old, tip) = (commit_of(&repo, &base_ref).await?, commit_of(&repo, &branch_ref).await?);
    let id = identity(&repo).await;
    let maker = Maker { id: &id, sign: signs_commits(&repo).await, key };
    let (commit, nothing) = match candidate(&repo, &args.strategy, &base_old, &tip, &args.message, &maker).await? {
        Candidate::Conflict(files) => return Ok(Integrated::Conflict { files }),
        Candidate::Nothing => (base_old.clone(), true),
        Candidate::Ready { commit } => (commit, false),
    };
    if !nothing && !args.checks.is_empty() && !args.allow_config_change {
        let edits = config_edits(&repo, &base_old, &commit).await?;
        if !edits.is_empty() {
            return Ok(Integrated::ConfigBlocked { files: edits });
        }
    }
    // Where the base is checked out must have no change to tracked files, and nothing there may be
    // overwritten by what the merge adds (an untracked or ignored file of the same name).
    let checkout = if nothing { None } else { checkout_of(&args.root, &args.base).await };
    if let Some(dir) = &checkout {
        if let Some(blocked) = checkout_blocked(dir, &base_old, &commit).await? {
            return Ok(blocked);
        }
    }
    if is_cancelled(key) {
        return Ok(Integrated::Cancelled);
    }
    let mut checks = Vec::new();
    if !nothing && !args.checks.is_empty() {
        let (results, stopped, cleanup_error) = run_checks(&repo, &commit, &args.checks, &args.copy, &args.link, key).await?;
        if stopped {
            return Ok(Integrated::Cancelled);
        }
        if results.last().is_some_and(|r| !r.ok) {
            return Ok(Integrated::CheckFailed { checks: results, cleanup_error });
        }
        checks = results;
    }
    let mut checkout_updated = None;
    if !nothing {
        // The checks (and a signing prompt) took a while: look again.
        if let Some(dir) = &checkout {
            if let Some(blocked) = checkout_blocked(dir, &base_old, &commit).await? {
                return Ok(blocked);
            }
        }
        if is_cancelled(key) {
            return Ok(Integrated::Cancelled);
        }
        let note = format!("kelpie finish: {} {}", args.strategy, args.branch);
        if !advance_base(&repo, &base_ref, &commit, &base_old, &note).await? {
            return Ok(Integrated::BaseMoved);
        }
        *moved = true;
        if let Some(dir) = checkout {
            let rt = plumb(&dir, &["read-tree", "-m", "-u", &base_old, &commit]).await;
            if !rt.as_ref().is_ok_and(|o| o.ok) {
                // Put the reference back: a checkout that does not match its branch is worse than no merge.
                let undone = advance_base(&repo, &base_ref, &base_old, &commit, "kelpie finish: undo").await;
                let put_back = undone.is_ok_and(|b| b);
                *moved = !put_back;
                return Err(format!(
                    "the main checkout could not be updated ({}); {}",
                    rt.map(|o| failure(&o)).unwrap_or_else(|e| e),
                    if put_back { "the branch was left where it was" } else { "the branch could not be put back: check it by hand" }
                ));
            }
            checkout_updated = Some(dir.to_string_lossy().into_owned());
        }
    }
    // From here on the base has moved: whatever goes wrong is reported next to the merge, never as "stopped".
    let (branch_deleted, branch_note) = if args.delete_branch {
        let on_branch = checkout_of(&args.root, &args.branch).await;
        match delete_if_merged(&repo, &base_ref, &branch_ref, &tip, on_branch.as_deref()).await {
            Ok(None) => (true, None),
            Ok(Some(why)) => (false, Some(why)),
            Err(e) => (false, Some(e)),
        }
    } else {
        (false, None)
    };
    Ok(if nothing { Integrated::Nothing { tip, branch_deleted, branch_note } } else { Integrated::Merged { commit, tip, checkout_updated, branch_deleted, branch_note, checks } })
}

/// `CheckoutDirty` when the folder where the base is checked out cannot take the merge.
async fn checkout_blocked(dir: &Path, old: &str, new: &str) -> CmdResult<Option<Integrated>> {
    let path = dir.to_string_lossy().into_owned();
    if !tracked_clean(dir).await? {
        return Ok(Some(Integrated::CheckoutDirty { path, files: Vec::new() }));
    }
    let files = collisions(dir, old, new).await?;
    Ok((!files.is_empty()).then_some(Integrated::CheckoutDirty { path, files }))
}

/// Puts a branch back at `tip` when it no longer exists (the worktree could not be removed after
/// the branch was deleted), and puts the worktree back on it.
#[tauri::command]
pub async fn finish_restore_branch(root: String, branch: String, tip: String, worktree: Option<String>) -> CmdResult<()> {
    let repo = local_root(&root)?;
    if !is_sha(&tip) {
        return Err("invalid commit".into());
    }
    let branch_ref = head_ref(&repo, &branch).await?;
    plumb_ok(&repo, &["update-ref", &branch_ref, &tip, ""]).await?;
    if let Some(wt) = worktree {
        // Best effort: the folder may be half deleted. The branch exists either way.
        let _ = plumb(&local_root(&wt)?, &["switch", "--quiet", branch.trim()]).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
