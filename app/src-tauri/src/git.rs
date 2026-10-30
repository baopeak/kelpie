//! Git integration via the `git` CLI (same behaviour as the user's terminal, hooks included).
//! At most `GIT_CONCURRENCY` git processes run at once so a burst of watcher events across
//! projects can never fork-bomb the machine.

use std::path::Path;
use std::process::Stdio;
use std::sync::LazyLock;

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::fs_ops::{err, guard, CmdResult};

const GIT_CONCURRENCY: usize = 3;
static GIT_SLOTS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(GIT_CONCURRENCY));
/// Review summaries and overlap scans (a refresh of 20 worktrees is dozens of processes) wait here, so
/// they never queue the sidebar, the Changes panel or a diff behind them.
static REVIEW_SLOTS: LazyLock<Semaphore> = LazyLock::new(|| Semaphore::new(2));

pub struct GitOutput {
    pub ok: bool,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub async fn git(cwd: &Path, args: &[&str]) -> CmdResult<GitOutput> {
    git_env(cwd, args, &[]).await
}

/// `git` with extra environment variables (the finish sequence runs with English messages and
/// without prompts).
pub async fn git_env(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> CmdResult<GitOutput> {
    run_git(&GIT_SLOTS, cwd, args, env).await
}

/// Worktree metadata is shared with daemon runs. Wait for the repository's cross-process
/// lock off the async executor; release it before materialization or long-running checks.
pub(crate) async fn git_worktree_env(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> CmdResult<GitOutput> {
    if cwd.to_str().is_some_and(crate::remote::is_remote) {
        return git_env(cwd, args, env).await;
    }
    let repo = cwd.to_path_buf();
    let _held = tokio::task::spawn_blocking(move || kelpie_proto::worktree_lock::acquire(&repo))
        .await.map_err(err)?.map_err(err)?;
    git_env(cwd, args, env).await
}

/// `git` on the review pool (see `REVIEW_SLOTS`).
pub async fn git_review(cwd: &Path, args: &[&str]) -> CmdResult<GitOutput> {
    run_git(&REVIEW_SLOTS, cwd, args, &[]).await
}

async fn run_git(slots: &Semaphore, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> CmdResult<GitOutput> {
    let _slot = slots.acquire().await.map_err(err)?;
    // A remote project's folder (`ssh://…`): the same git command, on the host.
    if let Some(dir) = cwd.to_str().filter(|d| crate::remote::is_remote(d)) {
        return crate::remote::git(dir, args).await;
    }
    let mut cmd = tokio::process::Command::new("git");
    crate::daemon_runtime::clean_env(cmd.as_std_mut());
    cmd.args(["-c", "core.quotepath=off", "-c", "color.ui=false"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_OPTIONAL_LOCKS", "0") // never fight the user's own git for index.lock
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true); // a call given up on (a timeout, Cancel) does not linger
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flash from a GUI app
    let out = cmd.output().await.map_err(|e| format!("git: {e}"))?;
    Ok(GitOutput { ok: out.status.success(), stdout: out.stdout, stderr: String::from_utf8_lossy(&out.stderr).into_owned() })
}

pub(crate) async fn git_ok(cwd: &Path, args: &[&str]) -> CmdResult<String> {
    let out = git(cwd, args).await?;
    if !out.ok {
        return Err(out.stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn worktree_ok(cwd: &Path, args: &[&str]) -> CmdResult<String> {
    let out = git_worktree_env(cwd, args, &[]).await?;
    if !out.ok {
        return Err(out.stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[derive(Serialize, Default)]
pub struct GitStatus {
    pub(crate) is_repo: bool,
    pub(crate) branch: Option<String>,
    pub(crate) upstream: Option<String>,
    pub(crate) ahead: i64,
    pub(crate) behind: i64,
    pub(crate) files: Vec<GitFile>,
    pub(crate) truncated: bool,
}

#[derive(Serialize, Debug, PartialEq)]
pub struct GitFile {
    pub(crate) path: String,
    pub(crate) orig_path: Option<String>,
    /// Index (staged) status letter, '.' when unchanged; '?' untracked; 'U' conflict.
    pub(crate) index: char,
    /// Working tree status letter.
    pub(crate) worktree: char,
}

const MAX_STATUS_FILES: usize = 5000;

/// Parses `git status --porcelain=v2 -z --branch`.
pub fn parse_status(raw: &[u8]) -> GitStatus {
    let mut st = GitStatus { is_repo: true, ..Default::default() };
    let text = String::from_utf8_lossy(raw);
    let mut records = text.split('\0').peekable();
    while let Some(rec) = records.next() {
        if rec.is_empty() {
            continue;
        }
        if let Some(rest) = rec.strip_prefix("# ") {
            let mut parts = rest.splitn(2, ' ');
            let (key, value) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            match key {
                "branch.head" if value != "(detached)" => st.branch = Some(value.to_string()),
                "branch.upstream" => st.upstream = Some(value.to_string()),
                "branch.ab" => {
                    for n in value.split(' ') {
                        if let Some(a) = n.strip_prefix('+') {
                            st.ahead = a.parse().unwrap_or(0);
                        } else if let Some(b) = n.strip_prefix('-') {
                            st.behind = b.parse().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        if st.files.len() >= MAX_STATUS_FILES {
            st.truncated = true;
            break;
        }
        let kind = rec.as_bytes()[0];
        let fields: Vec<&str> = rec.splitn(match kind { b'1' => 9, b'2' => 10, b'u' => 11, _ => 2 }, ' ').collect();
        match kind {
            b'1' | b'2' | b'u' => {
                let xy: Vec<char> = fields.get(1).map(|s| s.chars().collect()).unwrap_or_default();
                let path = fields.last().copied().unwrap_or("").to_string();
                // Renames carry the original path as the next NUL-separated record.
                let orig_path = if kind == b'2' { records.next().map(str::to_string) } else { None };
                let (index, worktree) = if kind == b'u' { ('U', 'U') } else { (xy.first().copied().unwrap_or('.'), xy.get(1).copied().unwrap_or('.')) };
                st.files.push(GitFile { path, orig_path, index, worktree });
            }
            b'?' => st.files.push(GitFile { path: rec[2..].to_string(), orig_path: None, index: '?', worktree: '?' }),
            _ => {}
        }
    }
    st
}

#[tauri::command]
pub async fn git_status(root: String) -> CmdResult<GitStatus> {
    let dir = guard(&root, "")?;
    let out = git(&dir, &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"]).await?;
    if !out.ok {
        return Ok(GitStatus::default()); // not a repository
    }
    Ok(parse_status(&out.stdout))
}

#[derive(Serialize)]
pub struct BranchInfo {
    is_repo: bool,
    branch: Option<String>,
    /// Tracked files changed (untracked files are not scanned: this runs for every project).
    dirty: bool,
    ahead: i64,
    behind: i64,
}

/// Cheap per-project summary for the sidebar (`-uno` skips the untracked walk).
#[tauri::command]
pub async fn git_branch_info(root: String) -> CmdResult<BranchInfo> {
    let dir = guard(&root, "")?;
    let out = git(&dir, &["status", "--porcelain=v2", "-z", "--branch", "-uno"]).await?;
    if !out.ok {
        return Ok(BranchInfo { is_repo: false, branch: None, dirty: false, ahead: 0, behind: 0 });
    }
    let st = parse_status(&out.stdout);
    Ok(BranchInfo { is_repo: true, dirty: !st.files.is_empty(), branch: st.branch, ahead: st.ahead, behind: st.behind })
}

#[derive(Serialize)]
pub struct FileVersions {
    /// Content at HEAD, or at the fork point from `base` (None when the file is new).
    original: Option<String>,
    /// Working tree (or index when `staged`) content (None when deleted).
    modified: Option<String>,
}

#[tauri::command]
pub async fn git_file_versions(root: String, path: String, staged: bool, base: Option<String>) -> CmdResult<FileVersions> {
    let dir = guard(&root, "")?;
    let full = guard(&root, &path)?;
    // Against a branch (a review): the file where this one forked from it, not at HEAD.
    let from = match base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        Some(b) if b.starts_with('-') => return Err("invalid base".into()),
        Some(b) => git_ok(&dir, &["merge-base", &format!("refs/heads/{b}"), "HEAD"]).await?.trim().to_string(),
        None => "HEAD".to_string(),
    };
    let head = git(&dir, &["show", &format!("{from}:{path}")]).await?;
    let original = head.ok.then(|| String::from_utf8_lossy(&head.stdout).into_owned());
    let modified = if staged {
        let idx = git(&dir, &["show", &format!(":{path}")]).await?;
        idx.ok.then(|| String::from_utf8_lossy(&idx.stdout).into_owned())
    } else if crate::remote::is_remote(&root) {
        crate::remote::read_file(&root, &path, 8 << 20).await.ok().map(|(b, _, _)| String::from_utf8_lossy(&b).into_owned())
    } else {
        std::fs::read(&full).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    Ok(FileVersions { original, modified })
}

#[tauri::command]
pub async fn git_stage(root: String, paths: Vec<String>, stage: bool) -> CmdResult<()> {
    let dir = guard(&root, "")?;
    for p in &paths {
        guard(&root, p)?;
    }
    let mut args: Vec<&str> = if stage { vec!["add", "--"] } else { vec!["restore", "--staged", "--"] };
    args.extend(paths.iter().map(String::as_str));
    git_ok(&dir, &args).await.map(|_| ())
}

/// Discards working-tree changes. Untracked files go to the OS trash.
#[tauri::command]
pub async fn git_discard(root: String, paths: Vec<String>, untracked: Vec<String>) -> CmdResult<()> {
    let dir = guard(&root, "")?;
    for p in &untracked {
        if crate::remote::is_remote(&root) {
            crate::remote::delete(&root, p).await?;
        } else {
            trash::delete(guard(&root, p)?).map_err(err)?;
        }
    }
    if paths.is_empty() {
        return Ok(());
    }
    for p in &paths {
        guard(&root, p)?;
    }
    let mut args = vec!["restore", "--worktree", "--"];
    args.extend(paths.iter().map(String::as_str));
    git_ok(&dir, &args).await.map(|_| ())
}

#[tauri::command]
pub async fn git_commit(root: String, message: String, amend: bool) -> CmdResult<String> {
    let dir = guard(&root, "")?;
    // Amending without a new message keeps the old one.
    let mut args = if amend && message.trim().is_empty() { vec!["commit", "--no-edit"] } else { vec!["commit", "-m", message.as_str()] };
    if amend {
        args.push("--amend");
    }
    git_ok(&dir, &args).await
}

#[derive(Serialize)]
pub struct Worktree {
    pub(crate) path: String,
    head: Option<String>,
    pub(crate) branch: Option<String>,
    pub(crate) bare: bool,
    detached: bool,
    /// Registered but its folder is gone (`git worktree prune` would drop it).
    pub(crate) prunable: bool,
}

#[cfg(test)]
impl Worktree {
    pub(crate) fn for_test(path: &str, branch: Option<&str>) -> Self {
        Worktree { path: path.into(), head: None, branch: branch.map(Into::into), bare: false, detached: branch.is_none(), prunable: false }
    }
}

#[tauri::command]
pub async fn git_worktrees(root: String) -> CmdResult<Vec<Worktree>> {
    let dir = guard(&root, "")?;
    let out = worktree_ok(&dir, &["worktree", "list", "--porcelain"]).await?;
    let mut list = Vec::new();
    for block in out.split("\n\n") {
        let mut wt = Worktree { path: String::new(), head: None, branch: None, bare: false, detached: false, prunable: false };
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                wt.path = p.to_string();
            } else if let Some(h) = line.strip_prefix("HEAD ") {
                wt.head = Some(h.to_string());
            } else if let Some(b) = line.strip_prefix("branch ") {
                wt.branch = Some(b.trim_start_matches("refs/heads/").to_string());
            } else if line == "bare" {
                wt.bare = true;
            } else if line == "detached" {
                wt.detached = true;
            } else if line.starts_with("prunable") {
                wt.prunable = true;
            }
        }
        if !wt.path.is_empty() {
            list.push(wt);
        }
    }
    Ok(list)
}

/// Creates `branch` from `base` in a new worktree at `path` (absolute). With `existing`, checks out
/// the local branch `branch` there instead (`base` is ignored).
#[tauri::command]
pub async fn git_worktree_add(root: String, path: String, branch: String, base: Option<String>, existing: Option<bool>) -> CmdResult<()> {
    let dir = guard(&root, "")?;
    let path = path.trim();
    if !Path::new(path).is_absolute() {
        return Err("the worktree folder must be an absolute path".into());
    }
    // A leading "-" would be read as an option, not a name.
    if branch.starts_with('-') || base.as_deref().is_some_and(|b| b.trim().starts_with('-')) {
        return Err("branch and base must not start with '-'".into());
    }
    if existing.unwrap_or(false) {
        // Only a real local branch: any other name would give a detached checkout of that commit.
        crate::git_basics::require_local_branch(&dir, &branch).await?;
        return worktree_ok(&dir, &["worktree", "add", "--", path, branch.as_str()]).await.map(|_| ());
    }
    let mut args = vec!["worktree", "add", "-b", branch.as_str(), "--", path];
    if let Some(b) = base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        args.push(b);
    }
    worktree_ok(&dir, &args).await.map(|_| ())
}

/// Drops worktree records whose folder is gone (after an interrupted removal).
#[tauri::command]
pub async fn git_worktree_prune(root: String) -> CmdResult<()> {
    let dir = guard(&root, "")?;
    worktree_ok(&dir, &["worktree", "prune"]).await.map(|_| ())
}

/// A file as committed at `rev` (e.g. the base of a new worktree); `None` when absent.
#[tauri::command]
pub async fn git_show_file(root: String, rev: String, path: String) -> CmdResult<Option<String>> {
    let dir = guard(&root, "")?;
    if rev.starts_with('-') || path.contains("..") {
        return Err("invalid revision or path".into());
    }
    let spec = format!("{}:{}", if rev.trim().is_empty() { "HEAD" } else { rev.trim() }, path);
    let out = git(&dir, &["show", spec.as_str()]).await?;
    Ok(out.ok.then(|| String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Commits reachable from this worktree's HEAD but from no branch, tag or remote: they live
/// only in its HEAD (detached checkouts) and would be lost with the worktree.
#[tauri::command]
pub async fn git_orphan_commits(root: String) -> CmdResult<u32> {
    orphan_count(&guard(&root, "")?).await
}

pub(crate) async fn orphan_count(dir: &Path) -> CmdResult<u32> {
    let out = git_ok(dir, &["rev-list", "--count", "HEAD", "--not", "--branches", "--tags", "--remotes"]).await?;
    out.trim().parse().map_err(|_| "unexpected git output".to_string())
}

/// The commit a worktree's HEAD is at.
#[tauri::command]
pub async fn git_head(root: String) -> CmdResult<String> {
    let dir = guard(&root, "")?;
    Ok(git_ok(&dir, &["rev-parse", "HEAD"]).await?.trim().to_string())
}

/// A symlink / junction found (and removed) inside a worktree.
pub struct SharedLink {
    path: std::path::PathBuf,
    target: std::path::PathBuf,
}

/// Removes the symlinks / junctions inside a worktree (the links only, never what they point
/// at): shared folders such as a linked `node_modules` must survive the worktree's deletion.
/// Returns what it removed so a failed deletion can put the links back (`relink_shared`).
pub(crate) fn unlink_shared(dir: &Path) -> Vec<SharedLink> {
    let mut removed = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    let mut seen = 0usize;
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > 200_000 {
                return removed;
            }
            let p = e.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            #[cfg(windows)]
            let is_link = {
                use std::os::windows::fs::MetadataExt;
                meta.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT: symlink or junction
            };
            #[cfg(not(windows))]
            let is_link = meta.file_type().is_symlink();
            if is_link {
                let target = std::fs::read_link(&p).ok();
                // A directory link is removed with remove_dir (Windows), a file link with remove_file.
                if std::fs::remove_dir(&p).or_else(|_| std::fs::remove_file(&p)).is_ok() {
                    if let Some(target) = target {
                        removed.push(SharedLink { path: p, target });
                    }
                }
            } else if meta.is_dir() && e.file_name() != ".git" {
                stack.push(p);
            }
        }
    }
    removed
}

/// Puts back the links `unlink_shared` removed; returns the ones that could not be restored.
pub(crate) fn relink_shared(links: &[SharedLink]) -> Vec<String> {
    let mut failed = Vec::new();
    for l in links {
        if std::fs::symlink_metadata(&l.path).is_ok() {
            continue;
        }
        // A junction's target can come back with the verbatim `\\?\` prefix.
        let raw = l.target.to_string_lossy();
        let target = std::path::PathBuf::from(raw.strip_prefix(r"\\?\").unwrap_or(&raw));
        if !target.is_dir() || crate::worktree_files::link_dir(&target, &l.path).is_err() {
            failed.push(l.path.to_string_lossy().into_owned());
        }
    }
    failed
}

fn same_folder(a: &str, b: &str) -> bool {
    let norm = |p: &str| std::fs::canonicalize(p).map(|c| c.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string()).replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(a) == norm(b)
}

/// `git worktree remove` for a linked worktree. Its shared links go first (so a linked folder is
/// never traversed) and come back when git cannot delete the folder.
pub(crate) async fn remove_linked_worktree(root: &str, path: &str, force: bool) -> CmdResult<()> {
    let dir = guard(root, "")?;
    // Only ever inside a linked worktree git knows about (never the main checkout).
    let canon = |p: &str| std::fs::canonicalize(p).ok();
    let registered = git_worktrees(root.to_string()).await.unwrap_or_default();
    let linked = canon(path).is_some_and(|c| registered.iter().skip(1).any(|w| canon(&w.path).as_ref() == Some(&c)));
    let mut unlinked = Vec::new();
    if linked {
        let wt = std::path::PathBuf::from(path);
        unlinked = tokio::task::spawn_blocking(move || unlink_shared(&wt)).await.map_err(err)?;
    }
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);
    let mut removed = worktree_ok(&dir, &args).await.map(|_| ());
    if let Err(e) = &mut removed {
        // Git refusing cleanly leaves the worktree in use: its shared folders must stay reachable. But on
        // Windows a locked file makes it delete most of the folder and its own records first; links put
        // back into that leftover would make deleting it by hand reach into the shared folders.
        let still_registered = git_worktrees(root.to_string()).await.is_ok_and(|l| l.iter().skip(1).any(|w| same_folder(&w.path, path)));
        if still_registered {
            if unlinked.is_empty() {
                return removed;
            }
            let failed = relink_shared(&unlinked);
            if !failed.is_empty() {
                *e = format!("{e}\nCould not restore the links: {}", failed.join(", "));
            }
        } else {
            *e = format!("{e}\nThe worktree was partly deleted and git no longer lists it; the leftover folder {path} can be deleted by hand (its shared folders were not linked back).");
        }
    }
    removed
}

#[tauri::command]
pub async fn git_worktree_remove(client: tauri::State<'_, std::sync::Arc<crate::daemon_client::DaemonClient>>, root: String, path: String, force: bool) -> CmdResult<()> {
    let removed = remove_linked_worktree(&root, &path, force).await;
    // Once it is gone: the daemon deletes the snapshots of the sessions that worked in it (their refs
    // live in the main repository). An older daemon or none running is fine; they are collected
    // when their sessions close. A removal that failed keeps them.
    if removed.is_ok() {
        let _ = client.call("checkpoint.drop", serde_json::json!({"worktree": path, "root": root})).await;
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2() {
        let raw = b"# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
1 .M N... 100644 100644 100644 aaa bbb src/a b.rs\0\
2 R. N... 100644 100644 100644 aaa bbb R100 new.rs\0old.rs\0\
u UU N... 100644 100644 100644 100644 a b c conflict.rs\0\
? notes.txt\0";
        let st = parse_status(raw);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!(st.files.len(), 4);
        assert_eq!(st.files[0], GitFile { path: "src/a b.rs".into(), orig_path: None, index: '.', worktree: 'M' });
        assert_eq!(st.files[1].orig_path.as_deref(), Some("old.rs"));
        assert_eq!(st.files[1].path, "new.rs");
        assert_eq!(st.files[2].index, 'U');
        assert_eq!(st.files[3].path, "notes.txt");
    }
}
