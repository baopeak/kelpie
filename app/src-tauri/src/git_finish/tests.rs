use super::*;
use std::process::Command;
use std::sync::atomic::Ordering;

/// A repository with `main` and a linked worktree on `feature`, under the temp folder.
struct Fixture {
    base: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Junctions first: removing the folder must never walk into a linked folder.
        crate::git::unlink_shared(&self.wt);
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").args(args).current_dir(dir).env("LC_ALL", "C").output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn fixture(name: &str) -> Fixture {
    let base = std::env::temp_dir().join(format!("kelpie-finish-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let repo = base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    sh(&repo, &["init", "-q", "-b", "main"]);
    for (k, v) in [("user.name", "Test"), ("user.email", "test@example.com"), ("core.autocrlf", "false")] {
        sh(&repo, &["config", k, v]);
    }
    std::fs::write(repo.join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(repo.join(".gitignore"), "node_modules/\n").unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "-q", "-m", "init"]);
    let wt = base.join("wt");
    sh(&repo, &["worktree", "add", "-q", "-b", "feature", wt.to_str().unwrap()]);
    Fixture { base, repo, wt }
}

fn root(f: &Fixture) -> String {
    f.repo.to_string_lossy().into_owned()
}

fn args(f: &Fixture, strategy: &str, checks: &[&str]) -> IntegrateArgs {
    IntegrateArgs {
        root: root(f),
        worktree: f.wt.to_string_lossy().into_owned(),
        allow_config_change: false,
        branch: "feature".into(),
        base: "main".into(),
        strategy: strategy.into(),
        message: "feat: the work".into(),
        checks: checks.iter().map(|c| c.to_string()).collect(),
        copy: vec![],
        link: vec![],
        delete_branch: true,
    }
}

fn wt_path(f: &Fixture) -> String {
    f.wt.to_string_lossy().into_owned()
}

/// The WIP commit of exactly `paths`.
async fn wip(f: &Fixture, paths: &[&str]) -> WipResult {
    finish_wip_commit(wt_path(f), "feature".into(), "WIP from claude".into(), paths.iter().map(|p| p.to_string()).collect()).await.unwrap()
}

fn commit_file(dir: &Path, name: &str, text: &str, msg: &str) {
    std::fs::write(dir.join(name), text).unwrap();
    sh(dir, &["add", "-A"]);
    sh(dir, &["commit", "-q", "-m", msg]);
}

fn branch_exists(f: &Fixture, name: &str) -> bool {
    !sh(&f.repo, &["branch", "--list", name]).is_empty()
}

fn merged(r: Integrated) -> (String, Option<String>, bool) {
    match r {
        Integrated::Merged { commit, checkout_updated, branch_deleted, .. } => (commit, checkout_updated, branch_deleted),
        other => panic!("expected a merge, got {other:?}"),
    }
}

#[tokio::test]
async fn wip_commit_keeps_uncommitted_and_untracked_work() {
    let f = fixture("wip");
    std::fs::write(f.wt.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::write(f.wt.join("new.txt"), "untracked\n").unwrap();
    std::fs::write(f.wt.join("staged.txt"), "staged\n").unwrap();
    sh(&f.wt, &["add", "staged.txt"]);
    let seen = finish_inspect(root(&f), wt_path(&f), "main".into()).await.unwrap();
    assert_eq!(seen.branch.as_deref(), Some("feature"), "the branch comes from the worktree's HEAD");
    assert!(seen.uncommitted.iter().all(|u| u.size > 0), "sizes are listed");
    let mut names: Vec<_> = seen.uncommitted.iter().map(|u| u.path.as_str()).collect();
    names.sort();
    assert_eq!(names, ["a.txt", "new.txt", "staged.txt"]);
    let canon = |p: &str| std::fs::canonicalize(p).unwrap();
    assert_eq!(seen.base_checkout.as_deref().map(canon), Some(canon(&root(&f))));

    let done = wip(&f, &["a.txt", "new.txt", "staged.txt"]).await;
    assert!(done.commit.is_some() && done.extra.is_empty());
    assert_eq!(sh(&f.wt, &["log", "-1", "--format=%s", "feature"]), "WIP from claude");
    assert!(sh(&f.wt, &["status", "--porcelain"]).is_empty(), "nothing left uncommitted");

    let (commit, checkout, deleted) = merged(finish_integrate(args(&f, "squash", &[])).await.unwrap());
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), commit);
    assert_eq!(sh(&f.repo, &["show", "main:new.txt"]), "untracked");
    assert_eq!(sh(&f.repo, &["show", "main:staged.txt"]), "staged");
    assert!(sh(&f.repo, &["show", "main:a.txt"]).ends_with("four"));
    assert_eq!(sh(&f.repo, &["rev-list", "--count", "main"]), "2", "squash: one commit on top of init");
    assert!(checkout.is_some() && f.repo.join("new.txt").is_file(), "the main checkout follows");
    assert!(sh(&f.repo, &["status", "--porcelain"]).is_empty());
    assert!(deleted && !branch_exists(&f, "feature"), "squash leaves no ancestry: the tree proof deletes the branch");
    // The worktree was detached at the tip, so git sees it as clean and removes it without force.
    assert!(sh(&f.wt, &["status", "--porcelain"]).is_empty());
    crate::git::remove_linked_worktree(&root(&f), f.wt.to_str().unwrap(), false).await.unwrap();
    assert!(!f.wt.exists());
}

#[tokio::test]
async fn nothing_to_commit_is_not_an_error() {
    let f = fixture("nowip");
    let none = wip(&f, &[]).await;
    assert!(none.commit.is_none() && none.extra.is_empty());
    // Nothing on the branch either: nothing to merge, the empty branch is simply deleted.
    match finish_integrate(args(&f, "squash", &[])).await.unwrap() {
        Integrated::Nothing { branch_deleted, .. } => assert!(branch_deleted),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn wip_commit_ignores_the_users_hooks() {
    let f = fixture("hooks");
    let hook = f.repo.join(".git").join("hooks").join("pre-commit");
    std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::write(f.wt.join("x.txt"), "x\n").unwrap();
    assert!(wip(&f, &["x.txt"]).await.commit.is_some());
}

#[tokio::test]
async fn conflicts_stop_before_anything_moves() {
    let f = fixture("conflict");
    commit_file(&f.repo, "a.txt", "ONE\ntwo\nthree\n", "main edit");
    commit_file(&f.wt, "a.txt", "uno\ntwo\nthree\n", "feature edit");
    let (main_tip, feature_tip) = (sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]));
    for strategy in ["squash", "merge", "rebase"] {
        match finish_integrate(args(&f, strategy, &[])).await.unwrap() {
            Integrated::Conflict { files } => assert_eq!(files, ["a.txt"], "{strategy}"),
            other => panic!("{strategy}: {other:?}"),
        }
    }
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), main_tip);
    assert_eq!(sh(&f.repo, &["rev-parse", "feature"]), feature_tip);
    assert_eq!(std::fs::read_to_string(f.wt.join("a.txt")).unwrap(), "uno\ntwo\nthree\n");
    assert!(sh(&f.repo, &["status", "--porcelain"]).is_empty(), "the main checkout was not touched");
}

#[tokio::test]
async fn a_failing_check_leaves_the_base_untouched() {
    let f = fixture("badcheck");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    let main_tip = sh(&f.repo, &["rev-parse", "main"]);
    match finish_integrate(args(&f, "squash", &["git --version", "git rev-parse --verify no-such-ref"])).await.unwrap() {
        Integrated::CheckFailed { checks, cleanup_error } => {
            assert_eq!(checks.len(), 2);
            assert!(checks[0].ok && !checks[1].ok);
            assert!(cleanup_error.is_none());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), main_tip);
    assert!(branch_exists(&f, "feature"));
    assert!(!f.repo.join("new.txt").exists());
    assert_eq!(sh(&f.repo, &["worktree", "list", "--porcelain"]).matches("worktree ").count(), 2, "the scratch worktree is gone");
}

#[tokio::test]
async fn checks_run_on_the_merged_tree() {
    let f = fixture("goodcheck");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    commit_file(&f.repo, "from-main.txt", "m\n", "main work");
    // Both files exist only in the merge result, never in either branch alone.
    let r = finish_integrate(args(&f, "squash", &["git ls-files --error-unmatch new.txt from-main.txt"])).await.unwrap();
    match r {
        Integrated::Merged { checks, .. } => assert!(checks.len() == 1 && checks[0].ok, "{checks:?}"),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn the_branchs_own_config_and_programs_never_run() {
    let f = fixture("candidate");
    // The candidate asks for a command and ships a program of that name.
    let marker = f.base.join("pwned");
    let script = if cfg!(windows) { ("pwn.cmd", format!("@echo off\r\necho x> \"{}\"\r\n", marker.display())) } else { ("pwn", format!("#!/bin/sh\ntouch '{}'\n", marker.display())) };
    std::fs::write(f.wt.join(script.0), &script.1).unwrap();
    std::fs::write(f.wt.join(".kelpie.yaml"), "check:\n  - pwn\n").unwrap();
    sh(&f.wt, &["add", "-A"]);
    sh(&f.wt, &["commit", "-q", "-m", "candidate config"]);
    // Only the approved list runs (the caller read it from the base).
    let mut a = args(&f, "squash", &["git --version"]);
    assert!(matches!(finish_integrate(args(&f, "squash", &["git --version"])).await.unwrap(), Integrated::ConfigBlocked { .. }), "the edited config blocks the checks");
    a.allow_config_change = true;
    match finish_integrate(a).await.unwrap() {
        Integrated::Merged { checks, .. } => assert_eq!(checks.len(), 1),
        other => panic!("{other:?}"),
    }
    assert!(!marker.exists());
    // Even a command with that name does not find the program in the checked-out folder.
    let f = fixture("candidate2");
    std::fs::write(f.wt.join(script.0), &script.1).unwrap();
    sh(&f.wt, &["add", "-A"]);
    sh(&f.wt, &["commit", "-q", "-m", "program"]);
    let name = script.0.trim_end_matches(".cmd");
    match finish_integrate(args(&f, "squash", &[name])).await.unwrap() {
        Integrated::CheckFailed { checks, .. } => assert!(checks[0].output.contains("not found"), "{checks:?}"),
        other => panic!("{other:?}"),
    }
    assert!(!marker.exists());
}

#[tokio::test]
async fn the_base_only_moves_from_where_it_was() {
    let f = fixture("cas");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    let old = sh(&f.repo, &["rev-parse", "main"]);
    let candidate = sh(&f.repo, &["rev-parse", "feature"]);
    // Someone commits on main after Finish looked at it.
    commit_file(&f.repo, "other.txt", "o\n", "main moves");
    let moved = sh(&f.repo, &["rev-parse", "main"]);
    assert!(!advance_base(&f.repo, "refs/heads/main", &candidate, &old, "test").await.unwrap(), "stale expectation is refused");
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), moved);
    assert!(advance_base(&f.repo, "refs/heads/main", &candidate, &moved, "test").await.unwrap());
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), candidate);
}

#[tokio::test]
async fn a_base_that_moves_during_the_checks_is_not_overwritten() {
    let f = fixture("moves");
    commit_file(&f.wt, "new.txt", "n
", "work");
    let feature_tip = sh(&f.repo, &["rev-parse", "feature"]);
    // The check itself commits on main (someone pushing, a second Kelpie…) while Finish waits for it.
    let mover = format!("git -C \"{}\" commit -q --allow-empty -m moved", f.repo.display());
    match finish_integrate(args(&f, "squash", &[&mover])).await.unwrap() {
        Integrated::BaseMoved => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(sh(&f.repo, &["log", "-1", "--format=%s", "main"]), "moved", "their commit is the base's tip, ours was not forced over it");
    assert!(!f.repo.join("new.txt").exists());
    assert_eq!(sh(&f.repo, &["rev-parse", "feature"]), feature_tip, "the branch is untouched");
    // Trying again merges onto the new tip.
    let (_, _, deleted) = merged(finish_integrate(args(&f, "squash", &[])).await.unwrap());
    assert!(deleted && f.repo.join("new.txt").is_file());
    assert_eq!(sh(&f.repo, &["log", "--format=%s", "-3", "main"]).lines().nth(1), Some("moved"));
}

#[tokio::test]
async fn rebase_replays_commits_and_keeps_authors() {
    let f = fixture("rebase");
    commit_file(&f.repo, "from-main.txt", "m\n", "main moves");
    sh(&f.wt, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "--allow-empty", "-m", "empty"]);
    std::fs::write(f.wt.join("one.txt"), "1\n").unwrap();
    sh(&f.wt, &["add", "-A"]);
    sh(&f.wt, &["-c", "user.name=Ada", "-c", "user.email=ada@example.com", "commit", "-q", "-m", "add one"]);
    commit_file(&f.wt, "two.txt", "2\n", "add two");
    let (commit, _, deleted) = merged(finish_integrate(args(&f, "rebase", &[])).await.unwrap());
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), commit);
    assert_eq!(sh(&f.repo, &["log", "--format=%s", "-3", "main"]).lines().collect::<Vec<_>>(), ["add two", "add one", "main moves"], "linear, empty commit dropped");
    assert_eq!(sh(&f.repo, &["log", "-1", "--format=%an", "main~1"]), "Ada");
    assert!(f.repo.join("one.txt").is_file() && f.repo.join("from-main.txt").is_file());
    assert!(deleted && !branch_exists(&f, "feature"));
}

#[tokio::test]
async fn merge_strategy_keeps_both_parents_and_proves_by_ancestry() {
    let f = fixture("merge");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    commit_file(&f.repo, "other.txt", "o\n", "main moves");
    let (commit, _, deleted) = merged(finish_integrate(args(&f, "merge", &[])).await.unwrap());
    assert_eq!(sh(&f.repo, &["rev-list", "--parents", "-1", &commit]).split(' ').count(), 3, "a merge commit");
    assert!(deleted);
}

#[tokio::test]
async fn a_branch_is_deleted_only_with_proof() {
    let f = fixture("proof");
    commit_file(&f.wt, "new.txt", "n\n", "unmerged work");
    let tip = sh(&f.repo, &["rev-parse", "feature"]);
    let why = delete_if_merged(&f.repo, "refs/heads/main", "refs/heads/feature", &tip, None).await.unwrap();
    assert!(why.is_some(), "its commit is not in main");
    assert!(branch_exists(&f, "feature"));
    // After a squash merge by hand the tree proof holds even though the commit is not an ancestor.
    sh(&f.repo, &["merge", "-q", "--squash", "feature"]);
    sh(&f.repo, &["commit", "-q", "-m", "squashed"]);
    assert!(delete_if_merged(&f.repo, "refs/heads/main", "refs/heads/feature", &tip, None).await.unwrap().is_none());
    assert!(!branch_exists(&f, "feature"));
    // A stale tip is refused by the reference update itself.
    sh(&f.repo, &["branch", "other", "main"]);
    let wrong = sh(&f.repo, &["rev-parse", "main~1"]);
    assert!(delete_if_merged(&f.repo, "refs/heads/main", "refs/heads/other", &wrong, None).await.unwrap().is_some());
    assert!(branch_exists(&f, "other"));
}

#[tokio::test]
async fn a_dirty_main_checkout_blocks_the_merge() {
    let f = fixture("dirty");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    std::fs::write(f.repo.join("a.txt"), "my edit\n").unwrap();
    let (main_tip, feature_tip) = (sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]));
    match finish_integrate(args(&f, "squash", &[])).await.unwrap() {
        Integrated::CheckoutDirty { .. } => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), main_tip);
    assert_eq!(sh(&f.repo, &["rev-parse", "feature"]), feature_tip);
    assert_eq!(std::fs::read_to_string(f.repo.join("a.txt")).unwrap(), "my edit\n");
}

#[tokio::test]
async fn the_branch_can_be_put_back() {
    let f = fixture("restore");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    let tip = sh(&f.repo, &["rev-parse", "feature"]);
    sh(&f.wt, &["switch", "--detach", "--quiet"]);
    sh(&f.repo, &["update-ref", "-d", "refs/heads/feature"]);
    let wt = f.wt.to_string_lossy().into_owned();
    finish_restore_branch(root(&f), "feature".into(), tip.clone(), Some(wt.clone())).await.unwrap();
    assert_eq!(sh(&f.repo, &["rev-parse", "feature"]), tip);
    assert_eq!(sh(&f.wt, &["symbolic-ref", "HEAD"]), "refs/heads/feature", "the worktree is on its branch again");
    // Never over an existing branch.
    assert!(finish_restore_branch(root(&f), "feature".into(), sh(&f.repo, &["rev-parse", "main"]), None).await.is_err());
    assert_eq!(sh(&f.repo, &["rev-parse", "feature"]), tip);
    assert!(finish_restore_branch(root(&f), "-x".into(), tip, None).await.is_err());
}

#[cfg(windows)]
#[tokio::test]
async fn a_removal_blocked_by_an_open_handle_puts_the_links_back() {
    use std::os::windows::fs::OpenOptionsExt;
    let f = fixture("handle");
    let shared = f.repo.join("node_modules");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("dep.js"), "dep").unwrap();
    commit_file(&f.wt, "work.txt", "w\n", "work");
    crate::worktree_files::link_dir(&shared, &f.wt.join("node_modules")).unwrap();
    // Another process reading a file of the worktree: Windows refuses to delete it.
    let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(f.wt.join("work.txt")).unwrap();
    let r = crate::git::remove_linked_worktree(&root(&f), f.wt.to_str().unwrap(), false).await;
    let message = r.expect_err("removal must fail while the handle is open");
    assert_eq!(std::fs::read_to_string(shared.join("dep.js")).unwrap(), "dep", "what it points at is intact");
    // Windows may delete most of the folder and git's records before it reaches the locked file. Then
    // the folder is a leftover and gets no link into the shared folder; otherwise the worktree is as it was.
    let registered = sh(&f.repo, &["worktree", "list", "--porcelain"]).lines().filter(|l| l.starts_with("worktree ")).count() == 2;
    let link = f.wt.join("node_modules").join("dep.js");
    if registered {
        assert_eq!(std::fs::read_to_string(link).unwrap(), "dep", "registered: the link is back and works");
    } else {
        assert!(!link.exists() && message.contains("partly deleted"), "unregistered leftover: no link, and it says so: {message}");
    }
    drop(held);
}

#[tokio::test]
async fn removal_keeps_what_the_links_point_at() {
    let f = fixture("remove");
    let shared = f.repo.join("node_modules");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("dep.js"), "dep").unwrap();
    crate::worktree_files::link_dir(&shared, &f.wt.join("node_modules")).unwrap();
    crate::git::remove_linked_worktree(&root(&f), f.wt.to_str().unwrap(), false).await.unwrap();
    assert!(!f.wt.exists());
    assert_eq!(std::fs::read_to_string(shared.join("dep.js")).unwrap(), "dep");
}

#[tokio::test]
async fn summaries_count_committed_and_uncommitted_and_cache() {
    let f = fixture("review");
    commit_file(&f.wt, "committed.txt", "1\n2\n3\n", "work");
    std::fs::write(f.wt.join("a.txt"), "one\ntwo\nthree\nfour\n").unwrap();
    std::fs::write(f.wt.join("untracked.txt"), "x\ny\n").unwrap();
    let target = || vec![ReviewTarget { path: f.wt.to_string_lossy().into_owned(), base: "main".into(), names_only: false }];
    let before = SCANS.load(Ordering::Relaxed);
    let r = review_changes(target(), false).await.unwrap().remove(0);
    assert!(r.ok, "{:?}", r.error);
    assert_eq!(r.branch.as_deref(), Some("feature"));
    assert_eq!(r.files, ["a.txt", "committed.txt", "untracked.txt"]);
    assert_eq!((r.commits, r.uncommitted), (1, 2));
    assert_eq!((r.added, r.deleted), (3 + 1 + 2, 0));
    assert!(!r.config_changed);
    assert_eq!(SCANS.load(Ordering::Relaxed), before + 1);
    // Same HEAD, index and references: answered from the cache.
    review_changes(target(), false).await.unwrap();
    assert_eq!(SCANS.load(Ordering::Relaxed), before + 1);
    // `force` scans again; a new commit changes the fingerprint.
    review_changes(target(), true).await.unwrap();
    assert_eq!(SCANS.load(Ordering::Relaxed), before + 2);
    commit_file(&f.wt, ".kelpie.yaml", "check: [x]\n", "config");
    let r = review_changes(target(), false).await.unwrap().remove(0);
    assert_eq!(SCANS.load(Ordering::Relaxed), before + 3);
    assert!(r.config_changed && r.commits == 2);
    // A folder that is not a repository is reported, not fatal.
    let bad = review_changes(vec![ReviewTarget { path: f.base.to_string_lossy().into_owned(), base: "main".into(), names_only: false }], false).await.unwrap();
    assert!(!bad[0].ok && bad[0].error.is_some());
}

#[tokio::test]
async fn twenty_cached_summaries_are_fast() {
    let f = fixture("perf");
    let mut targets = Vec::new();
    for i in 0..20 {
        let wt = f.base.join(format!("wt{i}"));
        sh(&f.repo, &["worktree", "add", "-q", "-b", &format!("b{i}"), wt.to_str().unwrap()]);
        std::fs::write(wt.join("f.txt"), format!("{i}\n")).unwrap();
        targets.push(ReviewTarget { path: wt.to_string_lossy().into_owned(), base: "main".into(), names_only: false });
    }
    let cold = Instant::now();
    let first = review_changes(targets.iter().map(|t| ReviewTarget { path: t.path.clone(), base: t.base.clone(), names_only: false }).collect(), false).await.unwrap();
    let cold = cold.elapsed();
    assert!(first.iter().all(|c| c.ok));
    let warm = Instant::now();
    review_changes(targets, false).await.unwrap();
    let warm = warm.elapsed();
    eprintln!("20 worktrees: cold {cold:?}, cached {warm:?}");
    assert!(warm < Duration::from_millis(50), "cached refresh took {warm:?}");
}

#[test]
fn command_lines_are_split_without_a_shell() {
    assert_eq!(tokens("npm test"), ["npm", "test"]);
    assert_eq!(tokens("  cargo   test --  \"a b\" c  "), ["cargo", "test", "--", "a b", "c"]);
    assert_eq!(tokens("echo \"\""), ["echo", ""]);
    assert!(tokens("   ").is_empty());
    // Shell syntax is just text.
    assert_eq!(tokens("a && b | c"), ["a", "&&", "b", "|", "c"]);
}

#[test]
fn merge_tree_output_is_parsed() {
    let sha = "a".repeat(40);
    let ok = GitOutput { ok: true, stdout: format!("{sha}\n").into_bytes(), stderr: String::new() };
    assert_eq!(parse_merge_tree(&ok).unwrap(), (sha.clone(), vec![]));
    let bad = GitOutput { ok: false, stdout: format!("{sha}\nb.txt\na.txt\nb.txt\n\nCONFLICT\n").into_bytes(), stderr: String::new() };
    assert_eq!(parse_merge_tree(&bad).unwrap().1, ["a.txt", "b.txt"]);
    let err = GitOutput { ok: false, stdout: vec![], stderr: "fatal: nope".into() };
    assert_eq!(parse_merge_tree(&err).unwrap_err(), "fatal: nope");
}

// ---------- findings of the review ----------

#[tokio::test]
async fn a_detached_worktree_is_never_finished() {
    let f = fixture("detached");
    commit_file(&f.wt, "two.txt", "2\n", "on the branch");
    sh(&f.wt, &["switch", "--detach", "--quiet"]);
    commit_file(&f.wt, "three.txt", "3\n", "only on the detached HEAD");
    let (main_tip, feature_tip, head) = (sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]), sh(&f.wt, &["rev-parse", "HEAD"]));
    let seen = finish_inspect(root(&f), wt_path(&f), "main".into()).await.unwrap();
    assert_eq!(seen.branch, None, "Rust reports the real state, whatever the app remembered");
    std::fs::write(f.wt.join("late.txt"), "late\n").unwrap();
    let refused = finish_wip_commit(wt_path(&f), "feature".into(), "WIP".into(), vec!["late.txt".into()]).await.unwrap_err();
    assert!(refused.contains("detached"), "{refused}");
    let refused = finish_integrate(args(&f, "squash", &[])).await.unwrap_err();
    assert!(refused.contains("detached"), "{refused}");
    assert_eq!((sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]), sh(&f.wt, &["rev-parse", "HEAD"])), (main_tip, feature_tip, head), "nothing moved");
    assert!(f.wt.join("late.txt").is_file() && branch_exists(&f, "feature"));
    // On another branch the same refusal.
    sh(&f.wt, &["switch", "--quiet", "-c", "other"]);
    assert!(finish_integrate(args(&f, "squash", &[])).await.unwrap_err().contains("\"other\""));
}

#[tokio::test]
async fn a_merge_does_not_overwrite_an_ignored_file_in_the_main_checkout() {
    let f = fixture("collide");
    std::fs::write(f.repo.join(".gitignore"), "node_modules/\nsecret.env\n").unwrap();
    sh(&f.repo, &["commit", "-qam", "ignore secrets"]);
    sh(&f.wt, &["merge", "-q", "main"]);
    // The user's own, ignored file in the main checkout; the branch adds a tracked file of that name.
    std::fs::write(f.repo.join("secret.env"), "MY REAL SECRET").unwrap();
    std::fs::write(f.wt.join("secret.env"), "template").unwrap();
    sh(&f.wt, &["add", "-f", "secret.env"]);
    sh(&f.wt, &["commit", "-qm", "template"]);
    let (main_tip, feature_tip) = (sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]));
    match finish_integrate(args(&f, "squash", &[])).await.unwrap() {
        Integrated::CheckoutDirty { files, .. } => assert_eq!(files, ["secret.env"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(std::fs::read_to_string(f.repo.join("secret.env")).unwrap(), "MY REAL SECRET");
    assert_eq!((sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"])), (main_tip, feature_tip));
}

#[tokio::test]
async fn harmless_untracked_files_in_the_main_checkout_do_not_block() {
    let f = fixture("untracked");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    std::fs::write(f.repo.join("notes.txt"), "my notes").unwrap();
    // A file the branch adds under a name the user has as a file where a folder would go.
    std::fs::write(f.repo.join("scratch"), "a file").unwrap();
    std::fs::create_dir_all(f.wt.join("scratch")).unwrap();
    commit_file(&f.wt, "scratch/in.txt", "x\n", "folder");
    match finish_integrate(args(&f, "squash", &[])).await.unwrap() {
        Integrated::CheckoutDirty { files, .. } => assert_eq!(files, ["scratch/in.txt"], "a file where a folder would go collides"),
        other => panic!("{other:?}"),
    }
    std::fs::remove_file(f.repo.join("scratch")).unwrap();
    let (_, checkout, _) = merged(finish_integrate(args(&f, "squash", &[])).await.unwrap());
    assert!(checkout.is_some() && f.repo.join("new.txt").is_file());
    assert_eq!(std::fs::read_to_string(f.repo.join("notes.txt")).unwrap(), "my notes", "the untracked file is untouched");
}

#[tokio::test]
async fn no_hook_runs_wherever_it_is_configured() {
    let f = fixture("planted");
    let marker = f.base.join("hook-ran.txt");
    let script = |name: &str| format!("#!/bin/sh\necho {name} >> '{}'\n", marker.to_string_lossy().replace('\\', "/"));
    // The repository's own hooks, and a hooks folder the repository's config points at (husky style,
    // possibly from the branch being merged).
    let own = f.repo.join(".git").join("hooks");
    let husky = f.base.join("husky");
    std::fs::create_dir_all(&husky).unwrap();
    for name in ["post-commit", "reference-transaction", "post-checkout", "pre-commit"] {
        std::fs::write(own.join(name), script(name)).unwrap();
        std::fs::write(husky.join(name), script(name)).unwrap();
    }
    sh(&f.repo, &["config", "core.hooksPath", &husky.to_string_lossy().replace('\\', "/")]);
    std::fs::write(f.wt.join("x.txt"), "x\n").unwrap();
    assert!(wip(&f, &["x.txt"]).await.commit.is_some());
    merged(finish_integrate(args(&f, "squash", &["git --version"])).await.unwrap());
    assert!(!marker.exists(), "no hook ran: {:?}", std::fs::read_to_string(&marker));
}

#[tokio::test]
async fn the_wip_commit_is_exactly_the_confirmed_files() {
    let f = fixture("exact");
    std::fs::write(f.wt.join("a.txt"), "changed\n").unwrap();
    std::fs::write(f.wt.join("mine.txt"), "mine\n").unwrap();
    // A file the agent wrote after the list was confirmed: nothing is committed, it is named.
    std::fs::write(f.wt.join("late.env"), "SECRET=1\n").unwrap();
    let r = wip(&f, &["a.txt", "mine.txt"]).await;
    assert!(r.commit.is_none() && r.extra == ["late.env"], "{:?}", r.extra);
    assert_eq!(sh(&f.wt, &["rev-parse", "HEAD"]), sh(&f.repo, &["rev-parse", "main"]), "no commit");
    assert!(sh(&f.wt, &["diff", "--cached", "--name-only"]).is_empty(), "nothing was staged either");
    // Confirmed again with the new file: all three, and nothing else.
    let r = wip(&f, &["a.txt", "mine.txt", "late.env"]).await;
    assert!(r.commit.is_some());
    assert_eq!(sh(&f.wt, &["show", "--name-only", "--format=", "HEAD"]).lines().collect::<BTreeSet<_>>(), BTreeSet::from(["a.txt", "late.env", "mine.txt"]));
    // File names that look like globs or magic stay literal.
    std::fs::write(f.wt.join("[ab].txt"), "star\n").unwrap();
    std::fs::write(f.wt.join("b.txt"), "b\n").unwrap();
    let r = wip(&f, &["[ab].txt", "b.txt"]).await;
    assert_eq!(sh(&f.wt, &["show", "--name-only", "--format=", "HEAD"]).lines().collect::<BTreeSet<_>>(), BTreeSet::from(["[ab].txt", "b.txt"]), "{:?}", r.extra);
}

#[tokio::test]
async fn files_that_appear_after_an_empty_list_stop_the_merge() {
    let f = fixture("late");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    std::fs::write(f.wt.join("late.txt"), "late\n").unwrap();
    let main_tip = sh(&f.repo, &["rev-parse", "main"]);
    match finish_integrate(args(&f, "squash", &[])).await.unwrap() {
        Integrated::WorktreeChanged { files } => assert_eq!(files, ["late.txt"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), main_tip);
    assert!(f.wt.join("late.txt").is_file() && branch_exists(&f, "feature"));
}

#[tokio::test]
async fn a_repository_inside_the_worktree_is_not_committed_as_a_file() {
    let f = fixture("embedded");
    let inner = f.wt.join("vendor").join("lib");
    std::fs::create_dir_all(&inner).unwrap();
    sh(&inner, &["init", "-q"]);
    std::fs::write(inner.join("x.txt"), "x\n").unwrap();
    sh(&inner, &["add", "-A"]);
    sh(&inner, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "inner"]);
    std::fs::write(f.wt.join("ok.txt"), "ok\n").unwrap();
    let seen = finish_inspect(root(&f), wt_path(&f), "main".into()).await.unwrap();
    assert_eq!(seen.embedded, ["vendor/lib/"]);
    let refused = finish_wip_commit(wt_path(&f), "feature".into(), "WIP".into(), vec!["ok.txt".into(), "vendor/lib/".into()]).await.unwrap_err();
    assert!(refused.contains("vendor/lib/"), "{refused}");
    assert_eq!(sh(&f.wt, &["rev-parse", "HEAD"]), sh(&f.repo, &["rev-parse", "main"]), "nothing committed");
    assert!(inner.join("x.txt").is_file());
}

/// Writes a script git can run as `gpg.program`: on unix it must be executable.
fn write_script(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A stand-in for gpg that records its calls (and signs, or fails).
fn fake_signer(f: &Fixture, works: bool) -> PathBuf {
    let log = f.base.join("signer-calls.txt");
    let script = f.base.join("fake-gpg.sh");
    let signature = if works { "echo '[GNUPG:] SIG_CREATED ' >&2\ncat >/dev/null\nprintf -- '-----BEGIN PGP SIGNATURE-----\\n\\nZmFrZQ==\\n-----END PGP SIGNATURE-----\\n'\n" } else { "cat >/dev/null\nexit 1\n" };
    write_script(&script, &format!("#!/bin/sh\necho called >> '{}'\n{signature}", log.to_string_lossy().replace('\\', "/")));
    sh(&f.repo, &["config", "gpg.program", &script.to_string_lossy().replace('\\', "/")]);
    log
}

fn calls(log: &Path) -> usize {
    std::fs::read_to_string(log).map(|t| t.lines().count()).unwrap_or(0)
}

#[tokio::test]
async fn commits_are_signed_when_the_repository_signs() {
    for strategy in ["squash", "merge", "rebase"] {
        let f = fixture(&format!("sign-{strategy}"));
        let log = fake_signer(&f, true);
        sh(&f.repo, &["config", "commit.gpgsign", "true"]);
        commit_file(&f.wt, "a-new.txt", "1\n", "first");
        sh(&f.wt, &["-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", "x"]);
        commit_file(&f.repo, "other.txt", "o\n", "main moves");
        let before = calls(&log);
        let (commit, _, _) = merged(finish_integrate(args(&f, strategy, &[])).await.unwrap());
        assert!(calls(&log) > before, "{strategy}: git's signer was asked");
        assert!(sh(&f.repo, &["cat-file", "commit", &commit]).contains("BEGIN PGP SIGNATURE"), "{strategy}: the commit carries the signature");
    }
    // The WIP commit follows the same setting.
    let f = fixture("sign-wip");
    let log = fake_signer(&f, true);
    sh(&f.repo, &["config", "commit.gpgsign", "true"]);
    std::fs::write(f.wt.join("x.txt"), "x\n").unwrap();
    let r = wip(&f, &["x.txt"]).await;
    assert!(r.commit.is_some() && calls(&log) == 1);
}

#[tokio::test]
async fn nothing_is_signed_when_the_repository_does_not_sign() {
    let f = fixture("nosign");
    let log = fake_signer(&f, true);
    sh(&f.repo, &["config", "commit.gpgsign", "false"]);
    commit_file(&f.wt, "new.txt", "n\n", "work");
    merged(finish_integrate(args(&f, "squash", &[])).await.unwrap());
    assert_eq!(calls(&log), 0);
}

#[tokio::test]
async fn a_failing_signer_changes_nothing() {
    for strategy in ["squash", "rebase"] {
        let f = fixture(&format!("badsign-{strategy}"));
        sh(&f.wt, &["commit", "-q", "--allow-empty", "-m", "x"]);
        commit_file(&f.wt, "new.txt", "n\n", "work");
        fake_signer(&f, false);
        sh(&f.repo, &["config", "commit.gpgsign", "true"]);
        let (main_tip, feature_tip) = (sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]));
        let e = finish_integrate(args(&f, strategy, &[])).await.unwrap_err();
        assert!(e.contains("signing") && e.contains("nothing was changed"), "{e}");
        assert_eq!((sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"])), (main_tip, feature_tip));
        assert!(!f.repo.join("new.txt").exists() && f.wt.is_dir());
    }
}

#[tokio::test]
async fn the_branch_editing_the_config_cannot_run_checks_unless_allowed() {
    let f = fixture("config");
    commit_file(&f.wt, ".kelpie.yaml", "check:\n  - evil\n", "config");
    let main_tip = sh(&f.repo, &["rev-parse", "main"]);
    match finish_integrate(args(&f, "squash", &["git --version"])).await.unwrap() {
        Integrated::ConfigBlocked { files } => assert_eq!(files, [".kelpie.yaml"]),
        other => panic!("{other:?}"),
    }
    assert_eq!(sh(&f.repo, &["rev-parse", "main"]), main_tip);
    // Without checks to run there is nothing to block; with the user's explicit choice the base's list runs.
    let mut allowed = args(&f, "squash", &["git --version"]);
    allowed.allow_config_change = true;
    match finish_integrate(allowed).await.unwrap() {
        Integrated::Merged { checks, .. } => assert_eq!(checks.len(), 1),
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_timed_out_check_takes_what_it_started_with_it() {
    let f = fixture("tree");
    let marker = f.base.join("grandchild-alive.txt");
    // The check starts a child that would write the marker after a while; the tree is stopped first.
    let script = f.base.join("slow.py");
    std::fs::write(&script, format!("import subprocess, sys, time\nsubprocess.Popen([sys.executable, '-c', \"import time; time.sleep(4); open(r'{}', 'w').write('x')\"])\ntime.sleep(60)\n", marker.to_string_lossy().replace('\\', "/"))).unwrap();
    let python = which_python();
    let started = Instant::now();
    let (r, cancelled) = run_check(&f.repo, &format!("\"{}\" \"{}\"", python.display(), script.display()), Duration::from_millis(1500), std::future::pending()).await;
    assert!(!r.ok && !cancelled && r.output.contains("timed out"), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "the timeout ended the wait");
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(!marker.exists(), "the grandchild was killed with the check");
}

fn which_python() -> PathBuf {
    ["python", "python3"].iter().find_map(|n| resolve_program(n, Path::new("."))).expect("python on PATH")
}

#[tokio::test]
async fn cancel_ends_a_running_check() {
    let f = fixture("cancel");
    let script = f.base.join("forever.py");
    std::fs::write(&script, "import time\ntime.sleep(60)\n").unwrap();
    let python = which_python();
    let line = format!("\"{}\" \"{}\"", python.display(), script.display());
    let repo = f.repo.clone();
    let press = std::sync::Arc::new(tokio::sync::Notify::new());
    let pressed = press.clone();
    let task = tokio::spawn(async move { run_check(&repo, &line, Duration::from_secs(120), async move { pressed.notified().await }).await });
    tokio::time::sleep(Duration::from_millis(800)).await;
    press.notify_one();
    let (r, cancelled) = tokio::time::timeout(Duration::from_secs(10), task).await.expect("Cancel ends the check").unwrap();
    assert!(cancelled && !r.ok);
}

#[tokio::test]
async fn errors_after_the_base_moved_are_reported_with_the_merge() {
    let f = fixture("after");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    // A branch whose deletion git refuses (a lock): the merge is still reported.
    let lock = f.repo.join(".git").join("refs").join("heads").join("feature.lock");
    std::fs::write(&lock, "").unwrap();
    match finish_integrate(args(&f, "squash", &[])).await.unwrap() {
        Integrated::Merged { branch_deleted, branch_note, .. } => assert!(!branch_deleted && branch_note.is_some()),
        other => panic!("{other:?}"),
    }
    assert!(f.repo.join("new.txt").is_file());
    let _ = std::fs::remove_file(lock);
}

#[tokio::test]
async fn names_only_summaries_have_no_counts_and_their_own_cache() {
    let f = fixture("names");
    commit_file(&f.wt, "committed.txt", "1\n2\n", "work");
    std::fs::write(f.wt.join("untracked.txt"), "x\n").unwrap();
    let t = |names_only| vec![ReviewTarget { path: wt_path(&f), base: "main".into(), names_only }];
    let names = review_changes(t(true), true).await.unwrap().remove(0);
    assert_eq!(names.files, ["committed.txt", "untracked.txt"]);
    assert_eq!((names.added, names.commits), (0, 0));
    let full = review_changes(t(false), false).await.unwrap().remove(0);
    assert_eq!((full.added, full.commits), (3, 1), "a names-only answer is never served for a full request");
}

#[tokio::test]
async fn cancel_belongs_to_one_finish() {
    let (a, b) = (fixture("cancel-a"), fixture("cancel-b"));
    for f in [&a, &b] {
        commit_file(&f.wt, "new.txt", "n\n", "work");
    }
    // Pressed for one worktree: the other Finish is not touched…
    finish_cancel(wt_path(&a));
    merged(finish_integrate(args(&b, "squash", &[])).await.unwrap());
    // …and this one stops before the base moves (nothing changed).
    let main_tip = sh(&a.repo, &["rev-parse", "main"]);
    assert!(matches!(finish_integrate(args(&a, "squash", &[])).await.unwrap(), Integrated::Cancelled));
    assert_eq!(sh(&a.repo, &["rev-parse", "main"]), main_tip);
    assert!(branch_exists(&a, "feature") && !a.repo.join("new.txt").exists());
    // The cancelled Finish is over: its Cancel does not carry to the next one.
    merged(finish_integrate(args(&a, "squash", &[])).await.unwrap());
}

#[tokio::test]
async fn a_stale_cancel_does_not_fail_the_next_wip_commit() {
    let f = fixture("stale-cancel");
    std::fs::write(f.wt.join("x.txt"), "x\n").unwrap();
    finish_cancel(wt_path(&f));
    let e = finish_wip_commit(wt_path(&f), "feature".into(), "WIP".into(), vec!["x.txt".into()]).await.unwrap_err();
    assert!(e.contains("cancelled"), "Cancel is honoured while it stands: {e}");
    assert_eq!(sh(&f.wt, &["rev-parse", "HEAD"]), sh(&f.repo, &["rev-parse", "main"]), "nothing was committed");
    // A Finish begins: whatever was left is forgotten.
    finish_reset(wt_path(&f));
    assert!(wip(&f, &["x.txt"]).await.commit.is_some());
}

#[tokio::test]
async fn a_cancel_pressed_while_signing_waits_changes_nothing() {
    let f = fixture("cancel-sign");
    commit_file(&f.wt, "new.txt", "n\n", "work");
    // A signer that never answers.
    let script = f.base.join("slow-gpg.sh");
    write_script(&script, "#!/bin/sh\nsleep 60\n");
    sh(&f.repo, &["config", "gpg.program", &script.to_string_lossy().replace('\\', "/")]);
    sh(&f.repo, &["config", "commit.gpgsign", "true"]);
    let (main_tip, feature_tip) = (sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"]));
    let wt = wt_path(&f);
    let task = tokio::spawn({
        let a = args(&f, "squash", &[]);
        async move { finish_integrate(a).await }
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    finish_cancel(wt);
    let r = tokio::time::timeout(Duration::from_secs(20), task).await.expect("Cancel ends the wait for the signer").unwrap().unwrap();
    assert!(matches!(r, Integrated::Cancelled), "{r:?}");
    assert_eq!((sh(&f.repo, &["rev-parse", "main"]), sh(&f.repo, &["rev-parse", "feature"])), (main_tip, feature_tip));
}

#[tokio::test]
async fn check_worktrees_left_by_a_crashed_kelpie_are_removed_without_following_their_links() {
    let f = fixture("stale");
    let shared = f.repo.join("node_modules");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("dep.js"), "dep").unwrap();
    // What a dead process (no such pid) left in the temp folder: a registered worktree with a live link.
    let stale = std::env::temp_dir().join("kelpie-finish-4294967000-1");
    let _ = std::fs::remove_dir_all(&stale);
    sh(&f.repo, &["worktree", "add", "--detach", "-q", stale.to_str().unwrap(), "HEAD"]);
    crate::worktree_files::link_dir(&shared, &stale.join("node_modules")).unwrap();
    // One of this process stays (a Finish of ours may be running).
    let ours = std::env::temp_dir().join(format!("kelpie-finish-{}-1", std::process::id()));
    let _ = std::fs::remove_dir_all(&ours);
    sh(&f.repo, &["worktree", "add", "--detach", "-q", ours.to_str().unwrap(), "HEAD"]);
    let commit = sh(&f.repo, &["rev-parse", "HEAD"]);
    let (results, _, _) = run_checks(&f.repo, &commit, &[], &[], &[], "stale-test").await.unwrap();
    assert!(results.is_empty());
    assert!(!stale.exists(), "the leftover folder is gone");
    assert_eq!(std::fs::read_to_string(shared.join("dep.js")).unwrap(), "dep", "what its link pointed at survived");
    assert!(!sh(&f.repo, &["worktree", "list", "--porcelain"]).contains("kelpie-finish-4294967000"), "and git's record");
    assert!(ours.exists(), "a live process's check folder is left alone");
    crate::git::unlink_shared(&ours);
    let _ = std::fs::remove_dir_all(&ours);
    let _ = plumb(&f.repo, &["worktree", "prune"]).await;
}
