//! Remote and branch operations for the Changes panel: push / pull / sync, an explicit
//! force-with-lease, aborting a merge or rebase, deleting a branch, listing refs for the
//! worktree start-from picker, and diff text for AI actions.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;

use crate::fs_ops::{guard, CmdResult};
use crate::git::git;

/// Network git: never prompts on a terminal (there is none) and gives up after a while.
async fn git_net(cwd: &Path, args: &[&str]) -> CmdResult<String> {
    if let Some(dir) = cwd.to_str().filter(|d| crate::remote::is_remote(d)) {
        let out = crate::remote::git(dir, args).await?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), out.stderr);
        return if out.ok { Ok(text.trim().to_string()) } else { Err(text.trim().to_string()) };
    }
    let mut cmd = tokio::process::Command::new("git");
    crate::daemon_runtime::clean_env(cmd.as_std_mut());
    cmd.args(["-c", "color.ui=false"])
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(180), cmd.output())
        .await
        .map_err(|_| format!("git {} timed out", args.first().unwrap_or(&"")))?
        .map_err(|e| format!("git: {e}"))?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        return Err(text.trim().to_string());
    }
    Ok(text.trim().to_string())
}

async fn output(cwd: &Path, args: &[&str]) -> CmdResult<String> {
    let out = git(cwd, args).await?;
    if !out.ok {
        return Err(out.stderr.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The remote to publish a branch to: `origin` when present, else the first one.
async fn default_remote(dir: &Path) -> CmdResult<String> {
    let remotes = output(dir, &["remote"]).await?;
    let list: Vec<&str> = remotes.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if list.contains(&"origin") {
        return Ok("origin".into());
    }
    list.first().map(|r| r.to_string()).ok_or_else(|| "this repository has no remote".to_string())
}

/// `push` (publishes with upstream the first time), `pull` (fast-forward only), `sync`
/// (pull then push) or `force_push` (`--force-with-lease`; only ever on request).
#[tauri::command]
pub async fn git_remote(root: String, action: String) -> CmdResult<String> {
    let dir = guard(&root, "")?;
    let has_upstream = git(&dir, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).await?.ok;
    let push = |dir: std::path::PathBuf| async move {
        if has_upstream {
            git_net(&dir, &["push"]).await
        } else {
            let remote = default_remote(&dir).await?;
            git_net(&dir, &["push", "-u", remote.as_str(), "HEAD"]).await
        }
    };
    match action.as_str() {
        "push" => push(dir).await,
        "pull" => {
            if !has_upstream {
                return Err("this branch has no upstream yet: push it first".into());
            }
            git_net(&dir, &["pull", "--ff-only"]).await
        }
        "sync" => {
            let pulled = if has_upstream { git_net(&dir, &["pull", "--ff-only"]).await? } else { String::new() };
            let pushed = push(dir).await?;
            Ok(format!("{pulled}\n{pushed}").trim().to_string())
        }
        "force_push" => {
            if !has_upstream {
                return Err("nothing to force: this branch has no upstream".into());
            }
            git_net(&dir, &["push", "--force-with-lease"]).await
        }
        other => Err(format!("unknown action {other}")),
    }
}

/// The operation in progress in this worktree (merge, rebase, cherry-pick, revert), if any.
pub async fn operation(dir: &Path) -> Option<&'static str> {
    if let Some(d) = dir.to_str().filter(|d| crate::remote::is_remote(d)) {
        // The same markers, looked up on the host.
        let r = crate::remote::parse_root(d)?;
        let script = format!(
            "cd {} && g=$(git rev-parse --absolute-git-dir) || exit 0; if [ -e \"$g/rebase-merge\" ] || [ -e \"$g/rebase-apply\" ]; then echo rebase; elif [ -e \"$g/MERGE_HEAD\" ]; then echo merge; elif [ -e \"$g/CHERRY_PICK_HEAD\" ]; then echo cherry-pick; elif [ -e \"$g/REVERT_HEAD\" ]; then echo revert; fi",
            crate::remote::q(&r.path)
        );
        return match crate::remote::exec_ok(&r, &script).await.ok()?.trim() {
            "rebase" => Some("rebase"),
            "merge" => Some("merge"),
            "cherry-pick" => Some("cherry-pick"),
            "revert" => Some("revert"),
            _ => None,
        };
    }
    let gd = output(dir, &["rev-parse", "--absolute-git-dir"]).await.ok()?;
    let gd = Path::new(gd.trim());
    if gd.join("rebase-merge").exists() || gd.join("rebase-apply").exists() {
        Some("rebase")
    } else if gd.join("MERGE_HEAD").exists() {
        Some("merge")
    } else if gd.join("CHERRY_PICK_HEAD").exists() {
        Some("cherry-pick")
    } else if gd.join("REVERT_HEAD").exists() {
        Some("revert")
    } else {
        None
    }
}

#[derive(Serialize)]
pub struct RepoState {
    operation: Option<&'static str>,
    has_remote: bool,
}

#[tauri::command]
pub async fn git_repo_state(root: String) -> CmdResult<RepoState> {
    let dir = guard(&root, "")?;
    let has_remote = output(&dir, &["remote"]).await.map(|r| !r.trim().is_empty()).unwrap_or(false);
    Ok(RepoState { operation: operation(&dir).await, has_remote })
}

#[tauri::command]
pub async fn git_abort(root: String) -> CmdResult<String> {
    let dir = guard(&root, "")?;
    let op = operation(&dir).await.ok_or("no merge, rebase, cherry-pick or revert in progress")?;
    output(&dir, &[op, "--abort"]).await.map(|_| format!("{op} aborted"))
}

/// `git branch -d` (refuses unmerged work) or, with `force`, `-D`.
#[tauri::command]
pub async fn git_branch_delete(root: String, branch: String, force: bool) -> CmdResult<()> {
    let dir = guard(&root, "")?;
    if branch.trim().is_empty() || branch.starts_with('-') {
        return Err("invalid branch name".into());
    }
    output(&dir, &["branch", if force { "-D" } else { "-d" }, "--", branch.trim()]).await.map(|_| ())
}

#[derive(Serialize)]
pub struct GitRef {
    name: String,
    /// local | remote | tag
    kind: &'static str,
}

/// Branches (local and remote) and tags, most recent first, for the start-from picker.
#[tauri::command]
pub async fn git_refs(root: String) -> CmdResult<Vec<GitRef>> {
    let dir = guard(&root, "")?;
    let out = output(&dir, &["for-each-ref", "--sort=-committerdate", "--count=400", "--format=%(refname)", "refs/heads", "refs/remotes", "refs/tags"]).await?;
    Ok(out
        .lines()
        .filter_map(|r| {
            let (kind, name) = if let Some(n) = r.strip_prefix("refs/heads/") {
                ("local", n)
            } else if let Some(n) = r.strip_prefix("refs/remotes/") {
                ("remote", n)
            } else {
                ("tag", r.strip_prefix("refs/tags/")?)
            };
            // `origin/HEAD` is an alias, not a branch.
            (!name.ends_with("/HEAD")).then(|| GitRef { name: name.to_string(), kind })
        })
        .collect())
}

/// Text an AI action works from, capped at `max` bytes: `staged` (stat + staged patch), `worktree`
/// (every uncommitted change) or `branch` (commits and stat since `base`).
#[tauri::command]
pub async fn git_context(root: String, kind: String, base: Option<String>, max: Option<usize>) -> CmdResult<String> {
    let dir = guard(&root, "")?;
    let max = max.unwrap_or(60_000);
    let mut text = match kind.as_str() {
        "staged" => {
            let stat = output(&dir, &["diff", "--cached", "--stat"]).await?;
            if stat.trim().is_empty() {
                // Nothing staged: "Commit all" commits every change, so describe those.
                let stat = output(&dir, &["diff", "HEAD", "--stat"]).await.unwrap_or_default();
                let patch = output(&dir, &["diff", "HEAD", "--no-ext-diff"]).await.unwrap_or_default();
                let untracked = output(&dir, &["ls-files", "--others", "--exclude-standard"]).await.unwrap_or_default();
                format!("{stat}\nNew files:\n{untracked}\n{patch}")
            } else {
                let patch = output(&dir, &["diff", "--cached", "--no-ext-diff"]).await?;
                format!("{stat}\n{patch}")
            }
        }
        "branch" => {
            let base = base.filter(|b| !b.trim().is_empty() && !b.starts_with('-')).ok_or("missing base branch")?;
            let range = format!("{}...HEAD", base.trim());
            let log = output(&dir, &["log", "--format=- %s%n%b", &format!("{}..HEAD", base.trim())]).await?;
            let stat = output(&dir, &["diff", "--stat", &range]).await?;
            let patch = output(&dir, &["diff", "--no-ext-diff", &range]).await.unwrap_or_default();
            format!("Commits:\n{log}\nFiles:\n{stat}\n{patch}")
        }
        // Everything uncommitted, staged or not, and the files git does not know yet; empty when there is none.
        "worktree" => {
            let patch = output(&dir, &["diff", "HEAD", "--no-ext-diff"]).await.unwrap_or_default();
            let untracked = output(&dir, &["ls-files", "--others", "--exclude-standard"]).await.unwrap_or_default();
            if untracked.trim().is_empty() {
                patch
            } else {
                format!("{patch}\nNew files:\n{untracked}")
            }
        }
        other => return Err(format!("unknown context {other}")),
    };
    if text.len() > max {
        let mut cut = max;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n… (truncated)");
    }
    Ok(text)
}
