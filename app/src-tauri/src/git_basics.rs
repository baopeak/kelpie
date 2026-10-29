//! Branch switcher, compact log and stash list for the Changes panel. Each command is one short
//! git call made when the user opens or acts on that part of the panel; nothing here polls.
//!
//! Names from the interface never reach git as options: a branch must be a plain local branch
//! name that exists under `refs/heads/`, and a stash must be `stash@{N}` whose commit is still the
//! one the list showed (the stash is shared by every worktree of the repository, so its indexes
//! move when another worktree stashes or drops).

use std::path::Path;

use serde::Serialize;

use crate::fs_ops::{guard, CmdResult};
use crate::git::{git, git_env, git_ok, git_worktree_env, parse_status, Worktree};

/// The compact log shows this many commits.
pub const LOG_LIMIT: usize = 100;
const BRANCH_LIMIT: usize = 500;
/// Field and record separators in the `--format` strings below (never in a name or subject).
const FS: char = '\x1f';
const RS: char = '\x1e';

/// A local branch name the interface may pass to git: never an option, a revision expression
/// (`a..b`, `x^`, `@{…}`), a pattern or a path trick. Git's own rules (`check-ref-format`), kept
/// strict.
pub fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && name != "@"
        && !name.starts_with('-')
        && !name.ends_with('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && name.split('/').all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
        && name.chars().all(|c| !c.is_control() && !c.is_whitespace() && !"~^:?*[\\".contains(c))
}

/// `stash@{N}`, nothing else.
pub fn valid_stash(name: &str) -> bool {
    name.strip_prefix("stash@{")
        .and_then(|rest| rest.strip_suffix('}'))
        .is_some_and(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()))
}

/// A full commit id (SHA-1 or SHA-256).
fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Errors unless `branch` is a valid name of an existing local branch.
pub(crate) async fn require_local_branch(dir: &Path, branch: &str) -> CmdResult<()> {
    if !valid_branch(branch) {
        return Err(format!("invalid branch name: {branch}"));
    }
    let full = format!("refs/heads/{branch}");
    if !git(dir, &["show-ref", "--verify", "--quiet", full.as_str()]).await?.ok {
        return Err(format!("no local branch named {branch}"));
    }
    Ok(())
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Branch {
    name: String,
    /// Checked out in this worktree.
    current: bool,
    /// Checked out in another worktree of the repository (its folder): git refuses to switch to it here.
    worktree: Option<String>,
    /// Committer date of its tip (Unix seconds).
    time: i64,
}

/// Parses `for-each-ref --format=%(refname)%1f%(HEAD)%1f%(committerdate:unix)%1e refs/heads`;
/// `worktrees` says which branches other worktrees have checked out.
pub fn parse_branches(raw: &str, worktrees: &[Worktree]) -> Vec<Branch> {
    raw.split(RS)
        .filter_map(|rec| {
            let mut f = rec.trim_start_matches(['\r', '\n']).split(FS);
            let name = f.next()?.strip_prefix("refs/heads/")?.to_string();
            let current = f.next()? == "*";
            let time = f.next().and_then(|t| t.trim().parse().ok()).unwrap_or(0);
            // The current branch is in this worktree; any other worktree on a branch holds it.
            let worktree = if current { None } else { worktrees.iter().find(|w| w.branch.as_deref() == Some(name.as_str())).map(|w| w.path.clone()) };
            Some(Branch { name, current, worktree, time })
        })
        .collect()
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Commit {
    hash: String,
    short: String,
    author: String,
    /// Author date (Unix seconds).
    time: i64,
    subject: String,
}

/// Parses `log --format=%H%x1f%h%x1f%an%x1f%at%x1f%s%x1e`.
pub fn parse_log(raw: &str) -> Vec<Commit> {
    raw.split(RS)
        .filter_map(|rec| {
            let rec = rec.trim_start_matches(['\r', '\n']);
            let mut f = rec.splitn(5, FS);
            let hash = f.next().filter(|h| !h.is_empty())?.to_string();
            let short = f.next()?.to_string();
            let author = f.next()?.to_string();
            let time = f.next()?.trim().parse().unwrap_or(0);
            let subject = f.next().unwrap_or("").trim_end().to_string();
            Some(Commit { hash, short, author, time, subject })
        })
        .collect()
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Stash {
    /// `stash@{N}`.
    name: String,
    /// Its commit: an action checks it is still the same stash.
    oid: String,
    /// When it was made (Unix seconds).
    time: i64,
    /// "WIP on main: abc1234 subject" or "On main: message".
    subject: String,
}

/// Parses `stash list --format=%gd%x1f%H%x1f%ct%x1f%gs%x1e`.
pub fn parse_stashes(raw: &str) -> Vec<Stash> {
    raw.split(RS)
        .filter_map(|rec| {
            let rec = rec.trim_start_matches(['\r', '\n']);
            let mut f = rec.splitn(4, FS);
            let name = f.next().filter(|n| valid_stash(n))?.to_string();
            let oid = f.next()?.to_string();
            let time = f.next()?.trim().parse().unwrap_or(0);
            let subject = f.next().unwrap_or("").trim_end().to_string();
            Some(Stash { name, oid, time, subject })
        })
        .collect()
}

/// Local branches, most recently committed first.
#[tauri::command]
pub async fn git_branches(root: String) -> CmdResult<Vec<Branch>> {
    let dir = guard(&root, "")?;
    let count = format!("--count={BRANCH_LIMIT}");
    let out = git_ok(&dir, &["for-each-ref", "--sort=-committerdate", count.as_str(), "--format=%(refname)%1f%(HEAD)%1f%(committerdate:unix)%1e", "refs/heads"]).await?;
    let worktrees = crate::git::git_worktrees(root).await.unwrap_or_default();
    Ok(parse_branches(&out, &worktrees))
}

/// Why a switch did not happen (nothing was changed), or that it did.
#[derive(Serialize, Debug, PartialEq)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum SwitchResult {
    Switched,
    /// Uncommitted changes (tracked or untracked files): they would follow to the other branch.
    Dirty { files: usize },
    /// The branch is checked out in another worktree (git refuses a second checkout).
    InWorktree { path: String },
    /// A merge, rebase, cherry-pick or revert is in progress here.
    Operation { operation: String },
    /// HEAD is detached on commits no branch, tag or remote has: a switch would leave them behind.
    Orphans { commits: u32 },
}

/// What stops switching this worktree to `target`, if anything. A branch held elsewhere comes before
/// uncommitted changes: a new worktree could not check it out either. `orphans`: commits only a
/// detached HEAD reaches (0 on a branch).
fn refusal(status: &crate::git::GitStatus, operation: Option<&str>, target: &str, worktrees: &[Worktree], orphans: u32) -> Option<SwitchResult> {
    if let Some(op) = operation {
        return Some(SwitchResult::Operation { operation: op.to_string() });
    }
    if let Some(w) = worktrees.iter().find(|w| w.branch.as_deref() == Some(target)) {
        return Some(SwitchResult::InWorktree { path: w.path.clone() });
    }
    if !status.files.is_empty() {
        return Some(SwitchResult::Dirty { files: status.files.len() });
    }
    (orphans > 0).then_some(SwitchResult::Orphans { commits: orphans })
}

/// Git too old for a command (`switch` needs 2.23, e.g. on a remote host): a readable error.
fn old_git(stderr: &str, needs: &str) -> String {
    if stderr.contains("is not a git command") || stderr.contains("unknown option") || stderr.contains("unrecognized argument") {
        format!("this needs git {needs} or newer: {}", stderr.trim())
    } else {
        stderr.trim().to_string()
    }
}

/// Switches this worktree to the local branch `branch`, only from a clean working tree.
#[tauri::command]
pub async fn git_switch_branch(root: String, branch: String) -> CmdResult<SwitchResult> {
    let dir = guard(&root, "")?;
    require_local_branch(&dir, &branch).await?;
    let out = git(&dir, &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=normal"]).await?;
    if !out.ok {
        return Err(out.stderr.trim().to_string());
    }
    let status = parse_status(&out.stdout);
    if status.branch.as_deref() == Some(branch.as_str()) {
        return Ok(SwitchResult::Switched);
    }
    let operation = crate::git_sync::operation(&dir).await;
    let worktrees = crate::git::git_worktrees(root.clone()).await?;
    // Detached: commits made there live only in this HEAD (git switch --quiet would drop them silently).
    let orphans = if status.branch.is_none() { crate::git::orphan_count(&dir).await? } else { 0 };
    if let Some(refused) = refusal(&status, operation, &branch, &worktrees, orphans) {
        return Ok(refused);
    }
    // Under the repository's worktree lock: a daemon run may be adding a worktree on this branch.
    // Hooks (post-checkout) run, as when the user switches in a terminal.
    let switched = git_worktree_env(&dir, &["switch", "--no-guess", "--quiet", branch.as_str()], &[]).await?;
    if !switched.ok {
        return Err(old_git(&switched.stderr, "2.23"));
    }
    Ok(SwitchResult::Switched)
}

/// The last `LOG_LIMIT` commits of HEAD; empty in a repository without commits.
#[tauri::command]
pub async fn git_log(root: String) -> CmdResult<Vec<Commit>> {
    let dir = guard(&root, "")?;
    let count = format!("--max-count={LOG_LIMIT}");
    let out = git(&dir, &["log", count.as_str(), "--encoding=UTF-8", "--no-show-signature", "--format=%H%x1f%h%x1f%aN%x1f%at%x1f%s%x1e", "HEAD", "--"]).await?;
    if !out.ok {
        if !git(&dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).await?.ok {
            return Ok(Vec::new()); // no commit yet
        }
        return Err(old_git(&out.stderr, "2.10"));
    }
    Ok(parse_log(&String::from_utf8_lossy(&out.stdout)))
}

/// The repository's stashes (shared by all its worktrees), newest first.
#[tauri::command]
pub async fn git_stashes(root: String) -> CmdResult<Vec<Stash>> {
    let dir = guard(&root, "")?;
    let out = git_ok(&dir, &["stash", "list", "--encoding=UTF-8", "--format=%gd%x1f%H%x1f%ct%x1f%gs%x1e"]).await?;
    Ok(parse_stashes(&out))
}

const STASH_MOVED: &str = "the stash list changed (another worktree may have stashed or dropped): refresh it and try again";

/// The commit id in git's `Dropped stash@{N} (<oid>)` line.
fn dropped_oid(output: &str) -> Option<&str> {
    let start = output.rfind('(')? + 1;
    let oid = &output[start..start + output[start..].find(')')?];
    valid_oid(oid).then_some(oid)
}

/// After `stash drop`: when git dropped another stash than `expected` (the list moved between the
/// check and the drop), stores that one back and reports the conflict.
async fn verify_drop(dir: &Path, expected: &str, output: &str) -> CmdResult<()> {
    let Some(dropped) = dropped_oid(output) else { return Ok(()) };
    if dropped.eq_ignore_ascii_case(expected) {
        return Ok(());
    }
    // A stash commit's own subject is the message it was listed with ("On main: …").
    let subject = git_ok(dir, &["log", "-1", "--encoding=UTF-8", "--format=%s", dropped]).await.unwrap_or_default();
    let message = if subject.trim().is_empty() { "restored by Kelpie" } else { subject.trim() };
    match git_ok(dir, &["stash", "store", "-m", message, dropped]).await {
        Ok(_) => Err(format!("{STASH_MOVED} (nothing was dropped)")),
        Err(e) => Err(format!("{STASH_MOVED}; another stash was dropped and could not be put back ({e}): recover it with git stash store {dropped}")),
    }
}

/// `apply` or `drop` the stash `stash` (`stash@{N}`), only while it is still the commit `oid` the
/// list showed. Apply uses the commit itself; drop needs `stash@{N}`, so what git dropped is checked
/// afterwards (and put back when it was another stash). Returns git's last line of output.
#[tauri::command]
pub async fn git_stash_action(root: String, stash: String, oid: String, action: String) -> CmdResult<String> {
    let dir = guard(&root, "")?;
    if !valid_stash(&stash) || !valid_oid(&oid) {
        return Err("invalid stash".into());
    }
    let action = match action.as_str() {
        "apply" => "apply",
        "drop" => "drop",
        other => return Err(format!("unknown stash action {other}")),
    };
    let current = git(&dir, &["rev-parse", "--verify", "--quiet", stash.as_str()]).await?;
    if !current.ok || !String::from_utf8_lossy(&current.stdout).trim().eq_ignore_ascii_case(&oid) {
        return Err(STASH_MOVED.into());
    }
    // English output: the drop line is parsed.
    let target = if action == "apply" { oid.as_str() } else { stash.as_str() };
    let out = git_env(&dir, &["stash", action, target], &[("LC_ALL", "C"), ("LANGUAGE", "C")]).await?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout).trim(), out.stderr.trim());
    if !out.ok {
        return Err(text.trim().to_string());
    }
    if action == "drop" {
        verify_drop(&dir, &oid, &text).await?;
    }
    Ok(text.lines().map(str::trim).rev().find(|l| !l.is_empty()).unwrap_or("").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// A repository with commits on `main` and a branch `feature`, removed on drop.
    struct Repo {
        base: PathBuf,
        dir: PathBuf,
    }

    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git").args(args).current_dir(dir).env("LC_ALL", "C").output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn repo(name: &str) -> Repo {
        let base = std::env::temp_dir().join(format!("kelpie-git-basics-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("repo");
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "Test Author"), ("user.email", "test@example.com"), ("core.autocrlf", "false"), ("commit.gpgsign", "false")] {
            sh(&dir, &["config", k, v]);
        }
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        sh(&dir, &["add", "-A"]);
        sh(&dir, &["commit", "-q", "-m", "init"]);
        sh(&dir, &["branch", "feature"]);
        Repo { base, dir }
    }

    fn root(r: &Repo) -> String {
        r.dir.to_string_lossy().into_owned()
    }

    fn head(r: &Repo) -> String {
        sh(&r.dir, &["rev-parse", "--abbrev-ref", "HEAD"])
    }

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree::for_test(path, branch)
    }

    #[test]
    fn branch_names_are_checked() {
        for ok in ["main", "feature/x", "wt/amber-12", "release-1.2", "a_b", "über"] {
            assert!(valid_branch(ok), "{ok}");
        }
        for bad in ["", "-f", "--force", "--orphan=x", "a..b", "x^", "x~1", "a:b", "a b", "a\tb", "@", "a@{1}", "x/", "/x", "a//b", ".hidden", "x/.y", "x.lock", "x.", "a*", "a?", "a[b", "a\\b", "a\nb"] {
            assert!(!valid_branch(bad), "{bad:?}");
        }
    }

    #[test]
    fn stash_names_are_checked() {
        assert!(valid_stash("stash@{0}") && valid_stash("stash@{12}"));
        for bad in ["stash", "stash@{}", "stash@{-1}", "stash@{0}x", "--all", "stash@{1.day.ago}", "refs/stash", "stash@{0} --index"] {
            assert!(!valid_stash(bad), "{bad}");
        }
        assert!(valid_oid(&"a".repeat(40)) && valid_oid(&"0".repeat(64)) && !valid_oid("abc") && !valid_oid(&"g".repeat(40)));
    }

    #[test]
    fn parses_branches() {
        let raw = "refs/heads/main\x1f*\x1f1700000300\x1e\nrefs/heads/feature\x1f \x1f1700000200\x1e\nrefs/heads/other\x1f \x1f1700000100\x1e\n";
        let list = parse_branches(raw, &[wt("/r", Some("main")), wt("/r-wt/feature", Some("feature")), wt("/r-wt/detached", None)]);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0], Branch { name: "main".into(), current: true, worktree: None, time: 1_700_000_300 });
        assert_eq!(list[1].worktree.as_deref(), Some("/r-wt/feature"));
        assert_eq!(list[2], Branch { name: "other".into(), current: false, worktree: None, time: 1_700_000_100 });
        assert!(parse_branches("", &[]).is_empty());
    }

    #[test]
    fn parses_log() {
        let raw = format!("{h1}\x1fabc1234\x1fAnn Lee\x1f1700000000\x1ffix: a | b \x1f c\x1e\n{h2}\x1fdef5678\x1fBob\x1f1690000000\x1f\x1e\n", h1 = "a".repeat(40), h2 = "b".repeat(40));
        let log = parse_log(&raw);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0], Commit { hash: "a".repeat(40), short: "abc1234".into(), author: "Ann Lee".into(), time: 1_700_000_000, subject: "fix: a | b \x1f c".into() });
        assert_eq!(log[1].subject, "");
        assert!(parse_log("").is_empty());
    }

    #[test]
    fn parses_stashes() {
        let raw = format!("stash@{{0}}\x1f{o}\x1f1700000000\x1fOn main: wip notes\x1e\nstash@{{1}}\x1f{o}\x1f1690000000\x1fWIP on feature: abc1234 init\x1e\nnot-a-stash\x1fx\x1f1\x1fy\x1e", o = "c".repeat(40));
        let list = parse_stashes(&raw);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0], Stash { name: "stash@{0}".into(), oid: "c".repeat(40), time: 1_700_000_000, subject: "On main: wip notes".into() });
        assert_eq!(list[1].name, "stash@{1}");
    }

    #[tokio::test]
    async fn switches_a_clean_worktree() {
        let r = repo("clean");
        assert_eq!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::Switched);
        assert_eq!(head(&r), "feature");
        // Already there: nothing to do.
        assert_eq!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::Switched);
    }

    #[tokio::test]
    async fn refuses_a_dirty_worktree() {
        let r = repo("dirty");
        std::fs::write(r.dir.join("a.txt"), "changed\n").unwrap();
        assert_eq!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::Dirty { files: 1 });
        assert_eq!(head(&r), "main");
        // Staged and untracked files count too.
        sh(&r.dir, &["add", "a.txt"]);
        std::fs::write(r.dir.join("new.txt"), "x\n").unwrap();
        assert_eq!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::Dirty { files: 2 });
        assert_eq!(head(&r), "main");
        assert_eq!(std::fs::read_to_string(r.dir.join("a.txt")).unwrap(), "changed\n");
    }

    #[tokio::test]
    async fn refuses_a_branch_checked_out_in_another_worktree() {
        let r = repo("inwt");
        let other = r.base.join("other");
        sh(&r.dir, &["worktree", "add", "-q", other.to_str().unwrap(), "feature"]);
        match git_switch_branch(root(&r), "feature".into()).await.unwrap() {
            SwitchResult::InWorktree { path } => assert!(path.replace('\\', "/").ends_with("/other"), "{path}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(head(&r), "main");
        let list = git_branches(root(&r)).await.unwrap();
        let feature = list.iter().find(|b| b.name == "feature").unwrap();
        assert!(!feature.current && feature.worktree.is_some());
        assert!(list.iter().any(|b| b.name == "main" && b.current && b.worktree.is_none()));
        // Held elsewhere comes first: with uncommitted changes too (a new worktree could not take it either).
        std::fs::write(r.dir.join("a.txt"), "changed\n").unwrap();
        assert!(matches!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::InWorktree { .. }));
        sh(&r.dir, &["checkout", "--", "a.txt"]);
        // Opening a branch in a new worktree needs an existing local branch that no worktree holds.
        let folder = r.base.join("third").to_string_lossy().into_owned();
        assert!(crate::git::git_worktree_add(root(&r), folder.clone(), "feature".into(), None, Some(true)).await.is_err());
        assert!(crate::git::git_worktree_add(root(&r), folder.clone(), "nope".into(), None, Some(true)).await.is_err());
        sh(&r.dir, &["branch", "third"]);
        crate::git::git_worktree_add(root(&r), folder.clone(), "third".into(), None, Some(true)).await.unwrap();
        assert_eq!(sh(Path::new(&folder), &["rev-parse", "--abbrev-ref", "HEAD"]), "third");
    }

    #[tokio::test]
    async fn refuses_to_leave_commits_only_a_detached_head_has() {
        let r = repo("detached");
        sh(&r.dir, &["switch", "-q", "--detach"]);
        sh(&r.dir, &["commit", "-q", "--allow-empty", "-m", "on no branch"]);
        let tip = sh(&r.dir, &["rev-parse", "HEAD"]);
        assert_eq!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::Orphans { commits: 1 });
        assert_eq!(sh(&r.dir, &["rev-parse", "HEAD"]), tip, "HEAD stays on the commit");
        // Detached on a commit a branch has: nothing is left behind, the switch goes ahead.
        sh(&r.dir, &["switch", "-q", "--detach", "main"]);
        assert_eq!(git_switch_branch(root(&r), "feature".into()).await.unwrap(), SwitchResult::Switched);
        assert_eq!(head(&r), "feature");
    }

    #[test]
    fn reads_the_dropped_stash_and_old_git() {
        let oid = "d".repeat(40);
        assert_eq!(dropped_oid(&format!("Dropped stash@{{0}} ({oid})")), Some(oid.as_str()));
        assert_eq!(dropped_oid("Dropped stash@{0} (not-an-id)"), None);
        assert_eq!(dropped_oid("nothing"), None);
        assert!(old_git("git: 'switch' is not a git command. See 'git --help'.", "2.23").starts_with("this needs git 2.23 or newer"));
        assert_eq!(old_git("fatal: invalid reference: x\n", "2.23"), "fatal: invalid reference: x");
    }

    #[tokio::test]
    async fn a_drop_that_hit_another_stash_is_put_back() {
        let r = repo("droprace");
        std::fs::write(r.dir.join("a.txt"), "older\n").unwrap();
        sh(&r.dir, &["stash", "push", "-q", "-m", "older"]);
        std::fs::write(r.dir.join("a.txt"), "newer\n").unwrap();
        sh(&r.dir, &["stash", "push", "-q", "-m", "newer"]);
        let list = git_stashes(root(&r)).await.unwrap();
        let (newer, older) = (&list[0], &list[1]);
        // The list moved under us: stash@{0} was dropped while the user meant `older`.
        let out = sh(&r.dir, &["stash", "drop", "stash@{0}"]);
        let err = verify_drop(&r.dir, &older.oid, &out).await.unwrap_err();
        assert!(err.contains("nothing was dropped"), "{err}");
        let after = git_stashes(root(&r)).await.unwrap();
        assert_eq!(after.len(), 2);
        assert!(after.iter().any(|s| s.oid == newer.oid && s.subject == "On main: newer"), "{after:?}");
        // The expected one dropped: fine.
        let out = sh(&r.dir, &["stash", "drop", &after.iter().find(|s| s.oid == older.oid).unwrap().name]);
        verify_drop(&r.dir, &older.oid, &out).await.unwrap();
        assert_eq!(git_stashes(root(&r)).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn refuses_during_a_merge() {
        let r = repo("merge");
        std::fs::write(r.dir.join("a.txt"), "main side\n").unwrap();
        sh(&r.dir, &["commit", "-qam", "main side"]);
        sh(&r.dir, &["switch", "-q", "feature"]);
        std::fs::write(r.dir.join("a.txt"), "feature side\n").unwrap();
        sh(&r.dir, &["commit", "-qam", "feature side"]);
        sh(&r.dir, &["switch", "-q", "main"]);
        let merge = Command::new("git").args(["merge", "feature"]).current_dir(&r.dir).output().unwrap();
        assert!(!merge.status.success());
        let res = git_switch_branch(root(&r), "feature".into()).await.unwrap();
        assert_eq!(res, SwitchResult::Operation { operation: "merge".into() });
    }

    #[tokio::test]
    async fn rejects_bad_or_unknown_names() {
        let r = repo("names");
        for bad in ["--orphan=x", "-", "main..feature", "HEAD~1", "nope"] {
            assert!(git_switch_branch(root(&r), bad.into()).await.is_err(), "{bad}");
        }
        // A tag is not a local branch.
        sh(&r.dir, &["tag", "v1"]);
        assert!(git_switch_branch(root(&r), "v1".into()).await.is_err());
        assert_eq!(head(&r), "main");
    }

    #[tokio::test]
    async fn log_lists_the_last_hundred() {
        let r = repo("log");
        // 105 more commits in one git process (one `git commit` each is slow on Windows).
        let mut stream = String::new();
        for i in 0..105 {
            let msg = format!("commit {i}");
            stream += &format!("commit refs/heads/main\ncommitter Test Author <test@example.com> {} +0000\ndata {}\n{msg}\n", 1_700_000_000 + i, msg.len());
            if i == 0 {
                stream += "from refs/heads/main^0\n";
            }
            stream += "\n";
        }
        let mut child = Command::new("git").args(["fast-import", "--quiet"]).current_dir(&r.dir).stdin(std::process::Stdio::piped()).spawn().unwrap();
        std::io::Write::write_all(child.stdin.as_mut().unwrap(), stream.as_bytes()).unwrap();
        drop(child.stdin.take());
        assert!(child.wait().unwrap().success());
        let log = git_log(root(&r)).await.unwrap();
        assert_eq!(log.len(), LOG_LIMIT);
        assert_eq!(log[0].subject, "commit 104");
        assert_eq!(log[0].author, "Test Author");
        assert!(log[0].short.len() >= 7);
        assert!(log[0].hash.starts_with(&log[0].short) && log[0].time > 1_600_000_000);
        // No commit yet: an empty log, not an error.
        let empty = r.base.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        sh(&empty, &["init", "-q"]);
        assert!(git_log(empty.to_string_lossy().into_owned()).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stash_apply_and_drop() {
        let r = repo("stash");
        std::fs::write(r.dir.join("a.txt"), "first\n").unwrap();
        sh(&r.dir, &["stash", "push", "-q", "-m", "first"]);
        std::fs::write(r.dir.join("a.txt"), "second\n").unwrap();
        sh(&r.dir, &["stash", "push", "-q", "-m", "second"]);
        let list = git_stashes(root(&r)).await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].subject.as_str()), ("stash@{0}", "On main: second"));
        // A stale id (the list moved) is refused.
        let (s0, s1) = (&list[0], &list[1]);
        assert!(git_stash_action(root(&r), s0.name.clone(), s1.oid.clone(), "drop".into()).await.is_err());
        assert!(git_stash_action(root(&r), "--all".into(), s0.oid.clone(), "drop".into()).await.is_err());
        assert!(git_stash_action(root(&r), s0.name.clone(), s0.oid.clone(), "pop".into()).await.is_err());
        git_stash_action(root(&r), s1.name.clone(), s1.oid.clone(), "apply".into()).await.unwrap();
        assert_eq!(std::fs::read_to_string(r.dir.join("a.txt")).unwrap(), "first\n");
        assert_eq!(git_stashes(root(&r)).await.unwrap().len(), 2, "apply keeps the stash");
        git_stash_action(root(&r), s0.name.clone(), s0.oid.clone(), "drop".into()).await.unwrap();
        let left = git_stashes(root(&r)).await.unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!((left[0].name.as_str(), left[0].oid.as_str()), ("stash@{0}", s1.oid.as_str()));
    }
}
