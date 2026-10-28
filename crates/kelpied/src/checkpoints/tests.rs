use super::*;
use std::cell::Cell;
use std::sync::Mutex as StdMutex;

thread_local! {
    /// The clock of the test's own thread: a job sets it to its `at`, so times are the test's.
    static NOW: Cell<u64> = const { Cell::new(0) };
}

type Events = Arc<StdMutex<Vec<(String, Value)>>>;

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-cp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Plain git for fixtures (never `command`, so the code under test is not what sets them up).
fn g(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "core.autocrlf=false", "-c", "commit.gpgsign=false", "-C"])
        .arg(dir)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

fn write(repo: &Path, name: &str, text: &str) {
    let p = repo.join(name);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn read(repo: &Path, name: &str) -> String {
    std::fs::read_to_string(repo.join(name)).unwrap_or_else(|_| "<missing>".into())
}

fn fixture() -> PathBuf {
    let repo = scratch().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    g(&repo, &["init", "-q", "-b", "main"]);
    g(&repo, &["config", "core.autocrlf", "false"]);
    write(&repo, ".gitignore", "*.log\n");
    for f in ["a.txt", "b.txt", "d.txt", "[ab].txt"] {
        write(&repo, f, &format!("{f} v0\n"));
    }
    g(&repo, &["add", "-A"]);
    g(&repo, &["commit", "-qm", "init"]);
    repo
}

fn store(data: &Path) -> (Arc<Checkpoints>, Events) {
    let events: Events = Arc::new(StdMutex::new(Vec::new()));
    let sink = events.clone();
    let emit: Emit = Arc::new(move |e, v| sink.lock().unwrap().push((e.to_string(), v)));
    let cp = Checkpoints::new(data, emit).with_clock(|| NOW.with(Cell::get));
    // Most tests take turns without the look at their start; the ones about it use `strict`.
    cp.look_required.store(false, Ordering::Relaxed);
    (Arc::new(cp), events)
}

fn strict(cp: &Checkpoints) {
    cp.look_required.store(true, Ordering::Relaxed);
}

fn job(sid: &str, repo: &Path, kind: Kind, started: u64, at: u64) -> Job {
    NOW.with(|c| c.set(at));
    Job { sid: sid.into(), cwd: repo.to_string_lossy().into_owned(), kind, source: "hook".into(), approximate: false, started, at }
}

fn records(cp: &Checkpoints, sid: &str) -> Vec<Record> {
    cp.with_log(sid, |l| l.map(|l| l.records.clone()).unwrap_or_default())
}

fn names(s: &str) -> String {
    format!("agent {s}")
}

fn index_bytes(repo: &Path) -> Vec<u8> {
    std::fs::read(g(repo, &["rev-parse", "--path-format=absolute", "--git-path", "index"])).unwrap()
}

fn user_state(repo: &Path) -> (Vec<u8>, String, String, String) {
    (index_bytes(repo), g(repo, &["status", "--porcelain=v2", "-z", "--untracked-files=all"]), g(repo, &["rev-parse", "HEAD"]), g(repo, &["symbolic-ref", "HEAD"]))
}

fn ref_count(repo: &Path, sid: &str) -> usize {
    g(repo, &["for-each-ref", "--format=%(refname)", &ref_prefix(sid)]).lines().filter(|l| !l.is_empty()).count()
}

const IDLE: UndoCtx = UndoCtx { owner_working: false, working_others: Vec::new() };

fn undo(cp: &Checkpoints, sid: &str, n: u32, paths: Option<&[String]>, confirm: &[String], ctx: &UndoCtx) -> Result<Value> {
    cp.undo(sid, &UndoRequest { n, paths, confirm, at: 10_000 }, ctx, &names)
}

#[test]
fn snapshots_leave_index_head_and_files_alone_and_capture_untracked() {
    let repo = fixture();
    let (cp, events) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    // A realistic mess: an unstaged edit, a staged new file, an untracked one, an ignored one.
    write(&repo, "a.txt", "edited\n");
    write(&repo, "staged.txt", "staged\n");
    g(&repo, &["add", "staged.txt"]);
    write(&repo, "untracked.txt", "u\n");
    write(&repo, "noise.log", "ignored\n");
    let before = user_state(&repo);
    cp.run_job(&job("s1", &repo, Kind::Turn, 2, 3)).unwrap();
    assert_eq!(user_state(&repo), before, "index, status, HEAD and branch are byte-identical");
    assert_eq!(read(&repo, "a.txt"), "edited\n");

    let recs = records(&cp, "s1");
    assert_eq!(recs.iter().map(|r| (r.n, r.kind)).collect::<Vec<_>>(), vec![(0, Kind::Baseline), (1, Kind::Turn)]);
    let tree = g(&repo, &["ls-tree", "-r", "--name-only", &recs[1].commit]);
    assert!(tree.lines().any(|l| l == "untracked.txt"), "untracked files are captured: {tree}");
    assert!(tree.lines().any(|l| l == "staged.txt"));
    assert!(!tree.lines().any(|l| l == "noise.log"), "ignored files are not: {tree}");
    assert_eq!(recs[1].base.as_deref(), Some(recs[0].commit.as_str()));
    let mut paths = recs[1].paths.clone();
    paths.sort();
    assert_eq!(paths, ["a.txt", "staged.txt", "untracked.txt"]);
    assert_eq!(recs[1].files, 3);
    // The snapshot commit hangs off HEAD, so dropping its ref frees the objects.
    assert_eq!(g(&repo, &["rev-parse", &format!("{}^", recs[1].commit)]), g(&repo, &["rev-parse", "HEAD"]));
    assert!(events.lock().unwrap().iter().any(|(e, v)| e == "checkpoint.updated" && v["n"] == 1));
    assert!(!cp.tmp_index("s1").exists(), "the temporary index is removed");
}

#[test]
fn an_unchanged_tree_makes_no_snapshot() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("s1", &repo, Kind::Turn, 2, 3)).unwrap();
    assert_eq!(records(&cp, "s1").len(), 1);
    assert_eq!(ref_count(&repo, "s1"), 1);
    write(&repo, "a.txt", "x\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 4, 5)).unwrap();
    cp.run_job(&job("s1", &repo, Kind::Turn, 6, 7)).unwrap();
    assert_eq!(records(&cp, "s1").len(), 2);
}

#[test]
fn numbering_survives_a_restart_and_a_lost_log() {
    let repo = fixture();
    let data = scratch();
    let (cp, _) = store(&data);
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "1\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 2, 3)).unwrap();
    drop(cp);

    let (cp, _) = store(&data);
    write(&repo, "a.txt", "2\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 4, 5)).unwrap();
    assert_eq!(records(&cp, "s1").iter().map(|r| r.n).collect::<Vec<_>>(), [0, 1, 2], "continues from the saved records");
    drop(cp);

    std::fs::remove_file(data.join("checkpoints").join("s1.json")).unwrap();
    let (cp, _) = store(&data);
    write(&repo, "b.txt", "lost the log\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 6, 7)).unwrap();
    let recs = records(&cp, "s1");
    assert_eq!(recs.last().map(|r| r.n), Some(3), "continues from the highest ref");
    // Only the last change belongs to the new turn, not everything since the baseline.
    assert_eq!(recs.last().unwrap().paths, ["b.txt"]);
    assert_eq!(ref_count(&repo, "s1"), 4);
}

#[test]
fn gc_removes_what_belongs_to_unknown_sessions() {
    let repo = fixture();
    let data = scratch();
    let (cp, _) = store(&data);
    for sid in ["keep-me", "gone"] {
        cp.run_job(&job(sid, &repo, Kind::Baseline, 1, 1)).unwrap();
    }
    // A ref this daemon never recorded (another daemon's) is left alone.
    g(&repo, &["update-ref", "refs/kelpie/stranger/0", &g(&repo, &["rev-parse", "HEAD"])]);
    drop(cp);

    let (cp, _) = store(&data);
    let known: HashSet<String> = ["keep-me".to_string()].into();
    assert_eq!(cp.gc(&known), 1);
    assert_eq!(ref_count(&repo, "gone"), 0);
    assert_eq!(ref_count(&repo, "keep-me"), 1);
    assert_eq!(ref_count(&repo, "stranger"), 1);
    assert!(!data.join("checkpoints").join("gone.json").exists());
    assert!(data.join("checkpoints").join("keep-me.json").exists());
}

#[test]
fn only_the_last_turns_are_kept() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    let cp = Arc::new(Checkpoints { keep: 3, ..Arc::try_unwrap(cp).ok().expect("sole owner") });
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    for i in 0..6 {
        write(&repo, "a.txt", &format!("{i}\n"));
        cp.run_job(&job("s1", &repo, Kind::Turn, 10 + i, 11 + i)).unwrap();
    }
    let recs = records(&cp, "s1");
    assert_eq!(recs.iter().map(|r| r.n).collect::<Vec<_>>(), [3, 4, 5, 6], "three turns and the base of the oldest");
    assert_eq!(ref_count(&repo, "s1"), 4);
    let list = cp.list("s1", None);
    assert_eq!(list["first_is_base"], true);
    // The oldest kept turn can still be compared.
    assert!(cp.diff("s1", 4).is_ok());
    assert!(cp.diff("s1", 2).is_err());
}

#[test]
fn undoing_turn_two_keeps_turn_three() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "turn one\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    write(&repo, "b.txt", "turn two\n");
    write(&repo, "sub/c.txt", "new in two\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 30, 40)).unwrap();
    write(&repo, "d.txt", "turn three\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 50, 60)).unwrap();

    let plan = cp.undo_plan("s1", 2, &IDLE, &names).unwrap();
    let items = plan["items"].as_array().unwrap();
    let find = |p: &str| items.iter().find(|i| i["path"] == p).unwrap_or_else(|| panic!("{p} in {items:?}"));
    assert_eq!((find("b.txt")["action"].as_str(), find("b.txt")["state"].as_str()), (Some("restore"), Some("clean")));
    assert_eq!((find("sub/c.txt")["action"].as_str(), find("sub/c.txt")["state"].as_str()), (Some("delete"), Some("clean")));
    assert_eq!(items.len(), 2, "only turn two's files");

    let before = (index_bytes(&repo), g(&repo, &["rev-parse", "HEAD"]));
    let done = undo(&cp, "s1", 2, None, &[], &IDLE).unwrap();
    assert_eq!(done["applied"], true, "{done}");
    assert_eq!(read(&repo, "b.txt"), "b.txt v0\n");
    assert_eq!(read(&repo, "sub/c.txt"), "<missing>");
    assert!(!repo.join("sub").exists(), "the folder the file left empty goes too");
    assert_eq!(read(&repo, "a.txt"), "turn one\n", "turn one stays");
    assert_eq!(read(&repo, "d.txt"), "turn three\n", "turn three stays");
    assert_eq!((index_bytes(&repo), g(&repo, &["rev-parse", "HEAD"])), before, "undo never touches the index or HEAD");

    let recs = records(&cp, "s1");
    let last = recs.last().unwrap();
    assert_eq!((last.kind, last.undo_of), (Kind::Undo, Some(2)));
    // The undo is its own step: the next turn does not contain it.
    write(&repo, "a.txt", "turn four\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 70, 80)).unwrap();
    assert_eq!(records(&cp, "s1").last().unwrap().paths, ["a.txt"]);
}

#[test]
fn a_file_changed_since_the_turn_needs_its_own_confirmation() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "b.txt", "turn one\n");
    write(&repo, "a.txt", "turn one\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    write(&repo, "b.txt", "later work\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 30, 40)).unwrap();

    let refused = undo(&cp, "s1", 1, None, &[], &IDLE).unwrap();
    assert_eq!(refused["applied"], false);
    let needs = refused["needs_confirmation"].as_array().unwrap();
    assert_eq!(needs.len(), 1);
    assert_eq!((needs[0]["path"].as_str(), needs[0]["state"].as_str()), (Some("b.txt"), Some("modified")));
    assert_eq!(read(&repo, "a.txt"), "turn one\n", "nothing was changed");

    // The clean file alone goes without asking.
    let only = undo(&cp, "s1", 1, Some(&["a.txt".to_string()]), &[], &IDLE).unwrap();
    assert_eq!(only["applied"], true);
    assert_eq!((read(&repo, "a.txt").as_str(), read(&repo, "b.txt").as_str()), ("a.txt v0\n", "later work\n"));
    // Asked again for the whole turn, the file already put back is left out.
    let all = undo(&cp, "s1", 1, None, &["b.txt".to_string()], &IDLE).unwrap();
    assert_eq!(all["applied"], true, "{all}");
    assert_eq!(all["files"], serde_json::json!(["b.txt"]));
    assert_eq!(all["skipped"][0]["path"], "a.txt");
    assert_eq!(read(&repo, "b.txt"), "b.txt v0\n");
}

#[test]
fn two_sessions_in_one_folder_share_files_and_undo_names_the_other() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    // A works 100..200 and edits f.txt; B works 150..250 and edits f.txt and g.txt.
    write(&repo, "f.txt", "from a\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 100, 200)).unwrap();
    write(&repo, "f.txt", "from a and b\n");
    write(&repo, "g.txt", "from b\n");
    cp.run_job(&job("b", &repo, Kind::Turn, 150, 250)).unwrap();

    let refused = undo(&cp, "a", 1, None, &[], &IDLE).unwrap();
    assert_eq!(refused["applied"], false);
    let needs = refused["needs_confirmation"].as_array().unwrap();
    assert_eq!(needs[0]["path"], "f.txt");
    assert_eq!(needs[0]["state"], "shared");
    assert_eq!(needs[0]["with"][0]["session"], "b");
    assert_eq!(needs[0]["with"][0]["name"], "agent b");
    assert_eq!(read(&repo, "f.txt"), "from a and b\n", "refused without confirmation: nothing changed");

    let owners = cp.owners("a", &names);
    let f = owners["files"].as_array().unwrap().iter().find(|r| r["path"] == "f.txt").unwrap();
    assert_eq!(f["shared"], true);
    assert_eq!(f["sessions"].as_array().unwrap().len(), 2);
    let g_row = owners["files"].as_array().unwrap().iter().find(|r| r["path"] == "g.txt").unwrap();
    assert_eq!(g_row["shared"], false);

    // The owner must be idle; another session working there now taints every file.
    assert!(undo(&cp, "a", 1, None, &["f.txt".into()], &UndoCtx { owner_working: true, ..UndoCtx::default() }).is_err());
    let busy = UndoCtx { owner_working: false, working_others: vec![("b".into(), "agent b".into())] };
    let plan = cp.undo_plan("a", 1, &busy, &names).unwrap();
    assert!(plan["items"].as_array().unwrap().iter().all(|i| i["state"] == "shared"));

    let done = undo(&cp, "a", 1, None, &["f.txt".into()], &IDLE).unwrap();
    assert_eq!(done["applied"], true, "{done}");
    assert_eq!(read(&repo, "f.txt"), "<missing>", "the file turn one added is gone");
}

#[test]
fn turns_that_do_not_overlap_in_time_are_not_shared() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "a was here\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 100, 200)).unwrap();
    // B starts after A's turn: its stretch begins after A's ended.
    cp.run_job(&job("b", &repo, Kind::Baseline, 250, 250)).unwrap();
    write(&repo, "a.txt", "a was here\nand then b\n");
    cp.run_job(&job("b", &repo, Kind::Turn, 300, 400)).unwrap();
    let plan = cp.undo_plan("a", 1, &IDLE, &names).unwrap();
    // B came later and changed the same file: that is a later change, not a concurrent one.
    assert_eq!(plan["items"][0]["state"], "modified");
    assert!(plan["items"][0]["with"].as_array().unwrap().is_empty());
}

#[test]
fn a_literal_path_never_widens_the_restore() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    // As a pathspec `[ab].txt` would also match a.txt and b.txt.
    write(&repo, "[ab].txt", "turn edit\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    write(&repo, "a.txt", "keep my edit\n");
    write(&repo, "b.txt", "keep my edit\n");
    let done = undo(&cp, "s1", 1, None, &[], &IDLE).unwrap();
    assert_eq!(done["applied"], true, "{done}");
    assert_eq!(read(&repo, "[ab].txt"), "[ab].txt v0\n");
    assert_eq!((read(&repo, "a.txt").as_str(), read(&repo, "b.txt").as_str()), ("keep my edit\n", "keep my edit\n"));
}

#[cfg(unix)]
#[test]
fn a_file_named_like_a_pathspec_magic_does_not_widen_undo() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    write(&repo, ":(top)*", "v0\n");
    g(&repo, &["add", "-A"]);
    g(&repo, &["commit", "-qm", "magic"]);
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, ":(top)*", "turn edit\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    write(&repo, "a.txt", "keep my edit\n");
    undo(&cp, "s1", 1, None, &[], &IDLE).unwrap();
    assert_eq!(read(&repo, ":(top)*"), "v0\n");
    assert_eq!(read(&repo, "a.txt"), "keep my edit\n");
    assert_eq!(cp.blob("s1", 1, ":(top)*", false, false).unwrap()["text"], "turn edit\n");
}

#[test]
fn blobs_and_diffs_read_a_turn() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "one\ntwo\n");
    std::fs::write(repo.join("bin.dat"), [0u8, 1, 2, 255]).unwrap();
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    let diff = cp.diff("s1", 1).unwrap();
    let files = diff["files"].as_array().unwrap();
    let a = files.iter().find(|f| f["path"] == "a.txt").unwrap();
    assert_eq!((a["status"].as_str(), a["added"].as_u64(), a["removed"].as_u64()), (Some("M"), Some(2), Some(1)));
    assert_eq!(files.iter().find(|f| f["path"] == "bin.dat").unwrap()["binary"], true);
    assert_eq!(cp.blob("s1", 1, "a.txt", false, false).unwrap()["text"], "one\ntwo\n");
    assert_eq!(cp.blob("s1", 0, "a.txt", false, false).unwrap()["text"], "a.txt v0\n");
    assert_eq!(cp.blob("s1", 0, "bin.dat", false, false).unwrap()["exists"], false);
    assert_eq!(cp.blob("s1", 1, "bin.dat", false, false).unwrap()["binary"], true);
    assert!(cp.blob("s1", 1, "../x", false, false).is_err());
    assert_eq!(cp.blob("s1", 1, "a.txt", false, true).unwrap()["text"], "a.txt v0\n", "before: as the turn found it");
    assert!(cp.blob("s1", 0, "a.txt", false, true).is_err());
    assert!(cp.diff("s1", 0).is_err(), "a baseline has nothing to compare with");
}

#[test]
fn a_repository_that_is_slow_twice_in_a_row_is_switched_off_once() {
    let repo = fixture();
    let (cp, events) = store(&scratch());
    cp.set_enabled(Some(true), None);
    let top = find_top(&repo).unwrap();
    let key = repo_key(&top);
    cp.note_duration(&key, &top, Duration::from_millis(1500), Duration::from_millis(1500));
    cp.note_duration(&key, &top, Duration::from_millis(50), Duration::from_millis(50));
    cp.note_duration(&key, &top, Duration::from_millis(1500), Duration::from_millis(1500));
    assert!(events.lock().unwrap().is_empty(), "a fast one in between resets the count");
    cp.note_duration(&key, &top, Duration::from_millis(1500), Duration::from_millis(1500));
    cp.note_duration(&key, &top, Duration::from_millis(1500), Duration::from_millis(1500));
    let told: Vec<_> = events.lock().unwrap().iter().filter(|(e, _)| e == "checkpoint.disabled").cloned().collect();
    assert_eq!(told.len(), 1, "says so once");
    assert_eq!(cp.status()["disabled"].as_array().map(Vec::len), Some(1));
    // While it is off the repository is skipped; turning it back on clears it.
    cp.submit(job("s1", &repo, Kind::Baseline, 1, 1));
    assert!(cp.queues.lock().is_empty());
    cp.set_enabled(None, Some(&top.to_string_lossy()));
    assert!(cp.inner.lock().disabled.is_empty());
    // An automatic switch-off is not kept across a restart: the next run measures again.
    cp.note_duration(&key, &top, Duration::from_millis(1500), Duration::from_millis(1500));
    cp.note_duration(&key, &top, Duration::from_millis(1500), Duration::from_millis(1500));
    assert_eq!(cp.status()["disabled"].as_array().map(Vec::len), Some(1));
    let data = cp.dir.parent().unwrap().to_path_buf();
    assert_eq!(store(&data).0.status()["disabled"].as_array().map(Vec::len), Some(0));
}

#[test]
fn turning_snapshots_off_is_remembered() {
    let data = scratch();
    let (cp, _) = store(&data);
    assert_eq!(cp.enabled(), DEFAULT_ENABLED);
    cp.set_enabled(Some(true), None);
    assert!(store(&data).0.enabled());
    cp.set_enabled(Some(false), None);
    assert!(!store(&data).0.enabled());
    assert_eq!(cp.list("none", None)["why"], "off");
}

#[test]
fn dropping_a_worktree_removes_its_sessions_refs() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("s2", &repo, Kind::Baseline, 1, 1)).unwrap();
    assert_eq!(cp.drop_worktree(&repo.to_string_lossy(), None), 2);
    assert_eq!((ref_count(&repo, "s1"), ref_count(&repo, "s2")), (0, 0));
    assert!(records(&cp, "s1").is_empty());
    // A session dropped for good is not snapshotted again.
    cp.run_job(&job("s1", &repo, Kind::Turn, 2, 3)).unwrap();
    assert_eq!(ref_count(&repo, "s1"), 0);
}

#[test]
fn the_jobs_of_one_session_coalesce_and_run_in_order() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.set_enabled(Some(true), None);
    let settle = || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !cp.queues.lock().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    cp.submit(job("s1", &repo, Kind::Baseline, 1, 1));
    settle();
    write(&repo, "a.txt", "x\n");
    cp.submit(job("s1", &repo, Kind::Turn, 2, 3));
    cp.submit(job("s1", &repo, Kind::Turn, 4, 5));
    settle();
    let recs = records(&cp, "s1");
    assert_eq!(recs.iter().map(|r| r.kind).collect::<Vec<_>>(), [Kind::Baseline, Kind::Turn]);
}

#[test]
fn diff_tree_output_is_parsed_with_its_counts() {
    let raw = b":100644 100644 aaa bbb M\0src/a b.rs\0:000000 100644 000 ccc A\0new\xc3\xa9.txt\0:100644 000000 ddd 000 D\0gone.bin\0:100755 100644 e f T\0mode.sh\0\
3\t1\tsrc/a b.rs\0-\t-\tgone.bin\0";
    let list = parse_diff_tree(raw);
    assert_eq!(list.len(), 4);
    assert_eq!((list[0].path.as_str(), list[0].status, list[0].added, list[0].removed), ("src/a b.rs", 'M', Some(3), Some(1)));
    assert_eq!(list[1].path, "newé.txt");
    assert_eq!(list[1].status, 'A');
    assert!(list[2].binary && list[2].added.is_none());
    assert_eq!((list[3].status, list[3].old_mode.as_str()), ('T', "100755"));
}

#[test]
fn restored_paths_must_stay_inside_the_folder() {
    for ok in ["a.txt", "dir/a.txt", "dir/with space/ä.txt", "[ab].txt"] {
        assert!(safe_rel(ok).is_ok(), "{ok}");
    }
    for bad in ["", "/etc/passwd", "\\x", "../x", "a/../../x", "a//b", ".git/config", "a/.GIT/x", "./a", "nul\0"] {
        assert!(safe_rel(bad).is_err(), "{bad:?}");
    }
    if cfg!(windows) {
        assert!(safe_rel("C:/x").is_err());
        assert!(safe_rel("a.txt:stream").is_err());
    }
}

/// `cargo test -p kelpied --release snapshot_cost -- --ignored --nocapture`, with
/// `KELPIE_CP_FILES` files (default 10000), `KELPIE_CP_RUNS` timed snapshots (default 30) and
/// `KELPIE_CP_KEEP` to leave the fixture in place. Prints the numbers the default is based on.
#[test]
#[ignore]
fn snapshot_cost() {
    let files: usize = std::env::var("KELPIE_CP_FILES").ok().and_then(|v| v.parse().ok()).unwrap_or(10_000);
    let runs: usize = std::env::var("KELPIE_CP_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(30);
    let repo = scratch().join("big");
    std::fs::create_dir_all(&repo).unwrap();
    g(&repo, &["init", "-q", "-b", "main"]);
    g(&repo, &["config", "core.autocrlf", "false"]);
    for i in 0..files {
        write(&repo, &format!("src/m{}/f{i}.rs", i % 200), &format!("// file {i}\nfn f{i}() -> usize {{ {i} }}\n").repeat(8));
    }
    g(&repo, &["add", "-A"]);
    g(&repo, &["commit", "-qm", "big"]);
    let (cp, _) = store(&scratch());
    let t = Instant::now();
    cp.run_job(&job("bench", &repo, Kind::Baseline, 1, 1)).unwrap();
    println!("files {files}: baseline {} ms", t.elapsed().as_millis());
    let size = |repo: &Path| g(repo, &["count-objects", "-v"]).lines().filter(|l| l.starts_with("size:") || l.starts_with("size-pack:")).map(|l| l.split(':').nth(1).unwrap().trim().parse::<u64>().unwrap_or(0)).sum::<u64>();
    let before = size(&repo);
    // Is git's CPU time counted past the Git for Windows launcher? (cpu should be most of the wall time)
    let top = find_top(&repo).unwrap();
    let handle = cp.repo(&top, &repo_key(&top)).unwrap();
    take_cpu();
    let t = Instant::now();
    working_tree(&handle, &cp.tmp_index("probe")).unwrap();
    println!("add -A + write-tree: wall {} ms, cpu counted {} ms", t.elapsed().as_millis(), take_cpu().as_millis());
    let (mut changed, mut quiet, mut started) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..runs {
        // The look at the start of the turn (nothing changed between turns).
        let t = Instant::now();
        cp.run_job(&job("bench", &repo, Kind::Start, 10 + i as u64, 10 + i as u64)).unwrap();
        started.push(t.elapsed().as_millis());
        // A turn that touches three files and adds one, then one that changes nothing.
        for k in 0..3 {
            write(&repo, &format!("src/m{}/f{}.rs", k, k), &format!("turn {i} edit {k}\n").repeat(20));
        }
        write(&repo, &format!("src/new{i}.rs"), "fn new() {}\n");
        let t = Instant::now();
        cp.run_job(&job("bench", &repo, Kind::Turn, 10 + i as u64, 11 + i as u64)).unwrap();
        changed.push(t.elapsed().as_millis());
        let t = Instant::now();
        cp.run_job(&job("bench", &repo, Kind::Turn, 10 + i as u64, 11 + i as u64)).unwrap();
        quiet.push(t.elapsed().as_millis());
    }
    let pct = |v: &mut Vec<u128>, p: f64| {
        v.sort();
        v[((v.len() as f64 * p).ceil() as usize).clamp(1, v.len()) - 1]
    };
    println!("changed turn: p50 {} ms, p95 {} ms, max {} ms", pct(&mut changed.clone(), 0.5), pct(&mut changed.clone(), 0.95), changed.iter().max().unwrap());
    println!("unchanged turn: p50 {} ms, p95 {} ms, max {} ms", pct(&mut quiet.clone(), 0.5), pct(&mut quiet.clone(), 0.95), quiet.iter().max().unwrap());
    println!("turn start look: p50 {} ms, p95 {} ms, max {} ms", pct(&mut started.clone(), 0.5), pct(&mut started.clone(), 0.95), started.iter().max().unwrap());
    let grown = size(&repo).saturating_sub(before);
    println!("git object growth over {runs} turns: {grown} KiB ({} KiB per 100 turns)", grown * 100 / runs as u64);
    if std::env::var("KELPIE_CP_KEEP").is_err() {
        let _ = std::fs::remove_dir_all(repo.parent().unwrap());
    }
}

/// `cargo test -p kelpied snapshots_on_a_loaded_machine -- --ignored --nocapture`: busy threads at
/// normal priority on every core for a bounded time, snapshots of a five-file repository meanwhile.
#[test]
#[ignore]
fn snapshots_on_a_loaded_machine() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let burners: Vec<_> = (0..std::thread::available_parallelism().map_or(4, |n| n.get()))
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut x = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    x = x.wrapping_add(1);
                    std::hint::black_box(x);
                }
            })
        })
        .collect();
    let end = Instant::now() + Duration::from_secs(150);
    for i in 0..10u64 {
        if Instant::now() > end {
            break;
        }
        write(&repo, "a.txt", &format!("turn {i}\n"));
        let t = Instant::now();
        cp.run_job(&job("s1", &repo, Kind::Turn, 10 + i, 11 + i)).unwrap();
        let (wall, cpu, probe) = *cp.last_timing.lock();
        println!("turn {i}: run {} ms; counted wall {} ms, cpu {} ms, probe {:?} ms", t.elapsed().as_millis(), wall.as_millis(), cpu.as_millis(), probe.map(|p| p.as_millis()));
    }
    stop.store(true, Ordering::Relaxed);
    for b in burners {
        b.join().unwrap();
    }
    println!("disabled: {:?}", cp.status()["disabled"]);
    assert!(cp.status()["disabled"].as_array().unwrap().is_empty(), "a busy machine switched a five-file repository off");
}

#[test]
fn a_busy_machine_does_not_switch_a_small_repository_off() {
    let repo = fixture();
    let (cp, events) = store(&scratch());
    let top = find_top(&repo).unwrap();
    let key = repo_key(&top);
    cp.repo(&top, &key).unwrap();
    // What was measured with every core busy: seconds of wall time, a fraction of one of git CPU.
    for wall in [1400, 7500, 2300, 2900, 1800] {
        cp.note_duration(&key, &top, Duration::from_millis(wall), Duration::from_millis(190));
    }
    assert_eq!(cp.status()["disabled"].as_array().map(Vec::len), Some(0));
    assert!(events.lock().unwrap().is_empty());
    // And when git's CPU time could not be counted, a small index is never the reason either.
    for _ in 0..4 {
        cp.note_duration(&key, &top, Duration::from_millis(7500), Duration::ZERO);
    }
    assert_eq!(cp.status()["disabled"].as_array().map(Vec::len), Some(0));
    // Real work still counts: two snapshots of over a second of git CPU switch it off.
    cp.note_duration(&key, &top, Duration::from_millis(1600), Duration::from_millis(1500));
    cp.note_duration(&key, &top, Duration::from_millis(1700), Duration::from_millis(1600));
    assert_eq!(cp.status()["disabled"].as_array().map(Vec::len), Some(1));
}

#[test]
fn without_counted_cpu_a_big_index_and_a_prompt_machine_make_a_snapshot_slow() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    let top = find_top(&repo).unwrap();
    let key = repo_key(&top);
    let handle = cp.repo(&top, &key).unwrap();
    // A big index: the real one grown past the limit.
    let mut grown = std::fs::read(&handle.index).unwrap();
    grown.resize(BIG_INDEX as usize + 1, 0);
    std::fs::write(&handle.index, grown).unwrap();
    assert!(cp.index_is_big(&key));
    cp.note_duration(&key, &top, Duration::from_millis(2000), Duration::ZERO);
    cp.note_duration(&key, &top, Duration::from_millis(2000), Duration::ZERO);
    // Judged on the wall time now, but only while a trivial git starts promptly: the verdict can only
    // be "off" (promptly), never anything else.
    let probe = cp.last_timing.lock().2;
    assert!(probe.is_some(), "the machine was probed");
    assert!(cp.status()["disabled"].as_array().is_some_and(|d| d.len() <= 1));
}

fn plan_of(cp: &Checkpoints, sid: &str, n: u32) -> Vec<Value> {
    cp.undo_plan(sid, n, &IDLE, &names).unwrap()["items"].as_array().unwrap().clone()
}

fn item<'a>(items: &'a [Value], path: &str) -> &'a Value {
    items.iter().find(|i| i["path"] == path).unwrap_or_else(|| panic!("{path} in {items:?}"))
}

#[test]
fn another_sessions_work_in_the_idle_gap_before_a_turn_is_not_reverted_silently() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "a turn one\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 100, 200)).unwrap();
    // B works while A is idle; A's next turn then edits only d.txt, but its diff holds b.txt too.
    write(&repo, "b.txt", "b was here\n");
    cp.run_job(&job("b", &repo, Kind::Turn, 300, 400)).unwrap();
    write(&repo, "d.txt", "a turn two\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 500, 600)).unwrap();
    let items = plan_of(&cp, "a", 2);
    assert_eq!(item(&items, "b.txt")["state"], "shared", "{items:?}");
    assert_eq!(item(&items, "b.txt")["with"][0]["session"], "b");
    assert_eq!(item(&items, "d.txt")["state"], "clean");
    let refused = undo(&cp, "a", 2, None, &[], &IDLE).unwrap();
    assert_eq!(refused["applied"], false);
    assert_eq!(read(&repo, "b.txt"), "b was here\n");
}

#[test]
fn a_file_older_than_the_turn_was_not_written_by_it() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    // The user's editor changed it; the turn (as the daemon's clock says) began a minute later.
    write(&repo, "a.txt", "typed by the user\n");
    let later = crate::session::now_millis() + 60_000;
    cp.run_job(&job("s1", &repo, Kind::Turn, later, later + 10)).unwrap();
    let items = plan_of(&cp, "s1", 1);
    assert_eq!(item(&items, "a.txt")["state"], "modified");
    assert_eq!(item(&items, "a.txt")["reason"], "changed before this turn started");
}

#[test]
fn a_closed_session_still_counts_as_having_touched_files() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "f.txt", "from a\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 100, 200)).unwrap();
    write(&repo, "g.txt", "from b\n");
    cp.run_job(&job("b", &repo, Kind::Turn, 150, 250)).unwrap();
    cp.close_session("b", 300);
    assert_eq!(ref_count(&repo, "b"), 0, "its refs are gone");
    let items = plan_of(&cp, "a", 1);
    assert_eq!(item(&items, "f.txt")["state"], "shared", "{items:?}");
    assert_eq!(item(&items, "f.txt")["with"][0]["session"], "b");
    // Still there after a restart.
    let again = Checkpoints::new(cp.dir.parent().unwrap(), Arc::new(|_, _| {})).with_clock(|| 0);
    assert!(again.inner.lock().logs.get("b").is_some_and(|l| l.closed));
}

#[test]
fn closed_sessions_are_forgotten_once_no_live_history_reaches_them() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "f.txt", "x\n");
    cp.run_job(&job("b", &repo, Kind::Turn, 10, 20)).unwrap();
    cp.close_session("b", 30);
    assert!(cp.inner.lock().logs.contains_key("b"), "A's history starts before it ended");
    // A's next turn reaches back only to 500: the tombstone can go.
    write(&repo, "a.txt", "y\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 500, 600)).unwrap();
    write(&repo, "d.txt", "z\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 700, 800)).unwrap();
    cp.keep_only_recent("a", 0);
    cp.prune_tombstones();
    assert!(!cp.inner.lock().logs.contains_key("b"));
    assert!(!cp.dir.join("b.json").exists());
}

#[test]
fn a_snapshot_that_fails_leaves_a_window_and_a_guess() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "x.txt", "someone's work\n");
    // B's snapshot cannot be made (its copy of the index is unreadable).
    let index = g(&repo, &["rev-parse", "--path-format=absolute", "--git-path", "index"]);
    let good = std::fs::read(&index).unwrap();
    std::fs::write(&index, b"not an index").unwrap();
    assert!(cp.run_job(&job("b", &repo, Kind::Turn, 100, 200)).is_err());
    std::fs::write(&index, good).unwrap();
    assert_eq!(cp.with_log("b", |l| l.map(|l| l.windows.len())), Some(1));
    // A's turn holds x.txt; B may have written it: that cannot be ruled out.
    cp.run_job(&job("a", &repo, Kind::Turn, 150, 250)).unwrap();
    let items = plan_of(&cp, "a", 1);
    assert_eq!(item(&items, "x.txt")["state"], "shared", "{items:?}");
    assert_eq!(item(&items, "x.txt")["with"][0]["session"], "b");
    // B's next record says its stretch is a guess.
    write(&repo, "b.txt", "later\n");
    cp.run_job(&job("b", &repo, Kind::Turn, 300, 400)).unwrap();
    assert!(records(&cp, "b").last().unwrap().approximate);
}

#[test]
fn a_queued_snapshot_is_widened_not_dropped_when_another_arrives() {
    let mut jobs = VecDeque::new();
    let repo = Path::new(".");
    coalesce(&mut jobs, job("s", repo, Kind::Turn, 100, 200));
    coalesce(&mut jobs, job("s", repo, Kind::Turn, 50, 400));
    coalesce(&mut jobs, job("t", repo, Kind::Turn, 10, 20));
    assert_eq!(jobs.len(), 2);
    // The window grows to the latest end; the start kept is the latest, for the "older than the turn" test.
    assert_eq!((jobs[0].started, jobs[0].at), (100, 400));
}

#[test]
fn a_held_index_lock_neither_blocks_nor_counts_as_slow() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    let index = g(&repo, &["rev-parse", "--path-format=absolute", "--git-path", "index"]);
    std::fs::write(format!("{index}.lock"), b"").unwrap();
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "x\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 2, 3)).unwrap();
    assert_eq!(records(&cp, "s1").len(), 2);
    assert!(cp.inner.lock().slow.is_empty() && cp.inner.lock().disabled.is_empty());
}

#[test]
fn a_stale_lock_on_the_temporary_index_does_not_block_the_session() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    let tmp = cp.tmp_index("s1");
    std::fs::create_dir_all(tmp.parent().unwrap()).unwrap();
    std::fs::write(tmp.with_extension("index.lock"), b"").unwrap();
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    assert_eq!(records(&cp, "s1").len(), 1);
}

fn install_logging_hooks(repo: &Path) -> PathBuf {
    let hooks = PathBuf::from(g(repo, &["rev-parse", "--path-format=absolute", "--git-path", "hooks"]));
    std::fs::create_dir_all(&hooks).unwrap();
    for name in ["reference-transaction", "post-checkout", "pre-commit"] {
        let file = hooks.join(name);
        std::fs::write(&file, "#!/bin/sh\necho ran >> \"$0.log\"\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    hooks
}

#[test]
fn snapshots_and_undo_run_none_of_the_repositorys_hooks() {
    let repo = fixture();
    let hooks = install_logging_hooks(&repo);
    // The hooks are real: git run the ordinary way triggers them.
    g(&repo, &["update-ref", "refs/probe/x", "HEAD"]);
    assert!(hooks.join("reference-transaction.log").exists(), "the probe hook runs for plain git");
    let _ = std::fs::remove_file(hooks.join("reference-transaction.log"));
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "a.txt", "x\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    assert_eq!(undo(&cp, "s1", 1, None, &[], &IDLE).unwrap()["applied"], true);
    cp.close_session("s1", 30);
    for name in ["reference-transaction", "post-checkout", "pre-commit"] {
        assert!(!hooks.join(format!("{name}.log")).exists(), "{name} ran");
    }
}

#[cfg(windows)]
#[test]
fn a_file_another_program_holds_open_does_not_cost_the_snapshot() {
    use std::os::windows::fs::OpenOptionsExt;
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "locked.db", "data\n");
    let _held = std::fs::OpenOptions::new().read(true).write(true).share_mode(0).open(repo.join("locked.db")).unwrap();
    write(&repo, "a.txt", "turn edit\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    let rec = records(&cp, "s1").pop().unwrap();
    assert_eq!(rec.unreadable, ["locked.db"]);
    assert_eq!(rec.paths, ["a.txt"], "everything else is captured");
}

#[test]
fn git_reports_of_unreadable_files_are_parsed() {
    let err = "error: open(\"locked.db\"): Permission denied\nerror: unable to index file 'dir/locked.db'\nfatal: adding files failed\n";
    assert_eq!(unreadable_files(err), ["dir/locked.db"]);
    assert!(unreadable_files("warning: nothing\n").is_empty());
}

#[test]
fn a_turn_is_not_merged_across_a_waiting_look_at_a_later_turns_start() {
    let repo = Path::new(".");
    let mut jobs = VecDeque::new();
    coalesce(&mut jobs, job("s", repo, Kind::Turn, 100, 200));
    coalesce(&mut jobs, job("s", repo, Kind::Start, 300, 300));
    coalesce(&mut jobs, job("s", repo, Kind::Turn, 300, 400));
    let order: Vec<(Kind, u64)> = jobs.iter().map(|j| (j.kind, j.at)).collect();
    assert_eq!(order, [(Kind::Turn, 200), (Kind::Start, 300), (Kind::Turn, 400)]);
    // With no look waiting in between they still merge.
    let mut jobs = VecDeque::new();
    coalesce(&mut jobs, job("s", repo, Kind::Turn, 100, 200));
    coalesce(&mut jobs, job("s", repo, Kind::Turn, 300, 400));
    assert_eq!(jobs.len(), 1);
}

#[test]
fn a_turn_with_no_look_after_its_base_has_every_file_flagged() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    strict(&cp);
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    // The merged-jobs case: the look for this turn never ran before its end snapshot.
    write(&repo, "a.txt", "agent\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    assert!(records(&cp, "s1").last().unwrap().between_truncated);
    let items = plan_of(&cp, "s1", 1);
    assert_eq!(item(&items, "a.txt")["state"], "modified", "{items:?}");
    assert_eq!(item(&items, "a.txt")["reason"], "changed between turns, before this turn started");
    // A look that ran after the base: nothing in doubt.
    cp.run_job(&job("s1", &repo, Kind::Start, 30, 30)).unwrap();
    write(&repo, "b.txt", "agent two\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 30, 40)).unwrap();
    assert!(!records(&cp, "s1").last().unwrap().between_truncated);
    assert_eq!(item(&plan_of(&cp, "s1", 2), "b.txt")["state"], "clean");
    // A look before an undo or a restart baseline belongs to the old base: the next turn is in doubt again.
    cp.run_job(&job("s1", &repo, Kind::Start, 50, 50)).unwrap();
    write(&repo, "gap.txt", "x");
    cp.run_job(&job("s1", &repo, Kind::Baseline, 55, 55)).unwrap();
    write(&repo, "d.txt", "agent three\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 60, 70)).unwrap();
    assert!(records(&cp, "s1").last().unwrap().between_truncated);
}

#[test]
fn a_backed_up_queue_cannot_hide_a_between_turns_edit() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    strict(&cp);
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    // Turn 1's end is still waiting when turn 2 ends: the queue is [turn(300..400), start(300)] after
    // the merge the old code made. Whatever order it runs in, turn 2's record has no look after its base.
    write(&repo, "a.txt", "turn one\n");
    write(&repo, "b.txt", "user fix\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 300, 400)).unwrap();
    cp.run_job(&job("s1", &repo, Kind::Start, 300, 300)).unwrap();
    let items = plan_of(&cp, "s1", 1);
    assert!(items.iter().all(|i| i["state"] == "modified"), "{items:?}");
    assert_eq!(undo(&cp, "s1", 1, None, &[], &IDLE).unwrap()["applied"], false);
    assert_eq!(read(&repo, "b.txt"), "user fix\n");
}

#[test]
fn a_turn_job_refused_because_the_session_closed_still_leaves_a_stretch() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.set_enabled(Some(true), None);
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.close_session("b", 100);
    write(&repo, "g.txt", "B's last turn\n");
    cp.submit(job("b", &repo, Kind::Turn, 150, 250));
    assert!(cp.queues.lock().is_empty(), "nothing was queued");
    let wins = cp.with_log("b", |l| l.map(|l| l.windows.len()));
    assert_eq!(wins, Some(1), "the refused job became a window");
    write(&repo, "f.txt", "A\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 300, 300)).unwrap();
    assert_eq!(item(&plan_of(&cp, "a", 1), "g.txt")["state"], "shared");
}

fn queue_job(cp: &Checkpoints, job: Job) {
    // As the ticker's submit leaves it, with the worker not yet picking it up.
    let key = repo_key(&find_top(Path::new(&job.cwd)).unwrap());
    cp.queues.lock().entry(key).or_insert_with(|| Queue { jobs: VecDeque::new() }).jobs.push_back(job);
}

#[test]
fn closing_a_session_with_its_turn_still_queued_keeps_that_work_as_a_stretch() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "g.txt", "B's turn output\n");
    queue_job(&cp, job("b", &repo, Kind::Turn, 150, 250));
    cp.close_session("b", 260);
    assert!(cp.queues.lock().values().all(|q| q.jobs.is_empty()), "its job left the queue");
    write(&repo, "f.txt", "A's turn output\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 300, 300)).unwrap();
    let items = plan_of(&cp, "a", 1);
    assert_eq!(item(&items, "g.txt")["state"], "shared", "{items:?}");
    assert_eq!(item(&items, "g.txt")["with"][0]["session"], "b");
    let refused = undo(&cp, "a", 1, None, &[], &IDLE).unwrap();
    assert_eq!(refused["applied"], false);
    assert_eq!(read(&repo, "g.txt"), "B's turn output\n");
}

#[test]
fn a_snapshot_job_that_was_already_taken_when_its_session_closed_still_leaves_its_stretch() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.work_started("b", 150);
    write(&repo, "g.txt", "B's turn output\n");
    let started = cp.take_work_start("b", 250);
    cp.close_session("b", 260);
    cp.run_job(&job("b", &repo, Kind::Turn, started, 250)).unwrap();
    write(&repo, "f.txt", "A's turn output\n");
    cp.run_job(&job("a", &repo, Kind::Turn, 300, 300)).unwrap();
    let items = plan_of(&cp, "a", 1);
    assert_eq!(item(&items, "g.txt")["state"], "shared", "{items:?}");
    assert_eq!(read(&repo, "g.txt"), "B's turn output\n");
    assert!(cp.inner.lock().logs.get("b").is_some_and(|l| l.closed && !l.windows.is_empty()));
}

#[test]
fn closing_a_session_whose_snapshot_is_running_waits_for_it_and_keeps_the_record() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("b", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "g.txt", "x\n");
    let key = repo_key(&find_top(&repo).unwrap());
    // The repository's lock is what a running snapshot holds.
    let lock = cp.repo_lock(&key);
    let held = lock.lock();
    let closer = {
        let cp = cp.clone();
        std::thread::spawn(move || cp.close_session("b", 300))
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(!closer.is_finished(), "closing waits for the running snapshot");
    // The snapshot finishes (record pushed under the lock), then the close goes on.
    cp.run_job(&job("b", &repo, Kind::Turn, 100, 200)).unwrap();
    drop(held);
    closer.join().unwrap();
    let rec = cp.inner.lock().logs.get("b").map(|l| (l.closed, l.records.len()));
    assert_eq!(rec, Some((true, 2)));
}

#[test]
fn an_edit_between_turns_to_a_file_the_agent_then_edits_too_is_flagged() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    let t0 = crate::session::now_millis();
    cp.run_job(&job("s1", &repo, Kind::Baseline, t0, t0)).unwrap();
    // The user (or a shell) edits a.txt in the gap, and deletes d.txt.
    write(&repo, "a.txt", "a.txt v0\nuser fix\n");
    std::fs::remove_file(repo.join("d.txt")).unwrap();
    // The turn starts: the look at its start sees the difference.
    let started = crate::session::now_millis();
    cp.run_job(&job("s1", &repo, Kind::Start, started, started)).unwrap();
    // The agent then appends to the same file and edits another.
    write(&repo, "a.txt", "a.txt v0\nuser fix\nagent line\n");
    write(&repo, "b.txt", "agent edit\n");
    let end = crate::session::now_millis();
    cp.run_job(&job("s1", &repo, Kind::Turn, started, end)).unwrap();
    let items = plan_of(&cp, "s1", 1);
    assert_eq!(item(&items, "a.txt")["state"], "modified", "{items:?}");
    assert_eq!(item(&items, "a.txt")["reason"], "changed between turns, before this turn started");
    assert_eq!(item(&items, "d.txt")["state"], "modified");
    assert_eq!(item(&items, "b.txt")["state"], "clean", "the agent's own file is not flagged");
    let refused = undo(&cp, "s1", 1, None, &[], &IDLE).unwrap();
    assert_eq!(refused["applied"], false);
    assert_eq!(read(&repo, "a.txt"), "a.txt v0\nuser fix\nagent line\n", "nothing was reverted");
    assert_eq!(records(&cp, "s1").len(), 2, "the look at the start is not a record");
    assert_eq!(ref_count(&repo, "s1"), 2, "and made no ref");
}

#[test]
fn a_start_with_nothing_changed_between_turns_flags_nothing() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.run_job(&job("s1", &repo, Kind::Start, 5, 5)).unwrap();
    write(&repo, "a.txt", "agent\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 5, 10)).unwrap();
    let items = plan_of(&cp, "s1", 1);
    assert_eq!(item(&items, "a.txt")["state"], "clean");
    assert!(records(&cp, "s1").last().unwrap().between.is_empty());
}

#[test]
fn a_restart_baseline_covers_the_work_before_it() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("a", &repo, Kind::Baseline, 1, 1)).unwrap();
    write(&repo, "x.txt", "done before the restart\n");
    cp.run_job(&job("a", &repo, Kind::Baseline, 100, 100)).unwrap();
    let log = cp.with_log("a", |l| l.cloned()).unwrap();
    let wins = windows_of(&log);
    assert_eq!(wins.len(), 1, "the restart baseline is a stretch");
    assert!(wins[0].paths.as_ref().is_some_and(|p| p.contains("x.txt")));
}

#[test]
fn a_file_git_could_not_read_before_is_flagged_in_the_next_turn() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    cp.with_log("s1", |l| l.unwrap().records.last_mut().unwrap().unreadable.push("a.txt".into()));
    write(&repo, "a.txt", "now readable and changed\n");
    cp.run_job(&job("s1", &repo, Kind::Turn, 10, 20)).unwrap();
    let items = plan_of(&cp, "s1", 1);
    assert_eq!(item(&items, "a.txt")["state"], "modified");
    assert_eq!(item(&items, "a.txt")["reason"], "could not be read when a snapshot was taken");
}

#[test]
fn a_file_removed_before_the_turn_began_is_flagged_by_its_folder_age() {
    let repo = fixture();
    let (cp, _) = store(&scratch());
    cp.run_job(&job("s1", &repo, Kind::Baseline, 1, 1)).unwrap();
    std::fs::remove_file(repo.join("d.txt")).unwrap();
    write(&repo, "a.txt", "turn\n");
    // The turn began long after the folder last changed (as the daemon's clock says).
    let later = crate::session::now_millis() + 60_000;
    cp.run_job(&job("s1", &repo, Kind::Turn, later, later + 10)).unwrap();
    let items = plan_of(&cp, "s1", 1);
    assert_eq!(item(&items, "d.txt")["reason"], "removed before this turn started", "{items:?}");
}

#[test]
fn no_hook_runs_for_snapshot_git_calls() {
    // Not a folder anyone can fill: the hooks path is /dev/null, ahead of any config the repository has.
    let args: Vec<String> = command(&std::env::temp_dir(), None).get_args().map(|a| a.to_string_lossy().into_owned()).collect();
    assert!(args.windows(2).any(|w| w[0] == "-c" && w[1] == "core.hooksPath=/dev/null"), "{args:?}");
    // And it holds against a repository whose own hooks would run.
    let repo = fixture();
    let marker = repo.join("hook-ran.txt");
    let hook = repo.join(".git").join("hooks").join("reference-transaction");
    std::fs::write(&hook, format!("#!/bin/sh\necho x >> '{}'\n", marker.to_string_lossy().replace('\\', "/"))).unwrap();
    let mut c = command(&repo, None);
    c.args(["update-ref", "refs/kelpie/test", "HEAD"]);
    assert!(run(c, None).unwrap().status.success());
    assert!(!marker.exists(), "the repository's reference-transaction hook did not run");
}

#[test]
fn git_runs_with_english_messages() {
    let out = run({
        let mut c = command(&std::env::temp_dir(), None);
        c.args(["config", "--get-regexp", "^$"]);
        c
    }, None)
    .unwrap();
    assert!(out.status.code().is_some());
    let cmd = command(&std::env::temp_dir(), None);
    let envs: Vec<_> = cmd.get_envs().filter_map(|(k, v)| Some((k.to_str()?.to_string(), v?.to_str()?.to_string()))).collect();
    assert!(envs.contains(&("LC_ALL".into(), "C".into())));
}
