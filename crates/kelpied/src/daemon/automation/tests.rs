use super::*;
use crate::auth::Role;
use crate::automations::{ShellAction, Trigger};

fn test_daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kelpie-autod-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
}

fn events(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(o) = rx.try_recv() {
        out.push(serde_json::from_slice(&o.data[5..]).unwrap());
    }
    out
}

fn wait_for(what: &str, cond: impl FnMut() -> bool) {
    wait_for_within(Duration::from_secs(20), what, cond);
}

fn wait_for_within(limit: Duration, what: &str, mut cond: impl FnMut() -> bool) {
    let until = Instant::now() + limit;
    while !cond() {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn add_session(daemon: &Daemon, id: &str, kind: &str, cwd: &str) -> Arc<Session> {
    let spec = SessionSpec { id: id.into(), kind: kind.into(), cwd: cwd.into(), ..Default::default() };
    let handle = daemon.next_handle.fetch_add(1, Ordering::Relaxed);
    let s = Session::new(handle, spec, 80, 24);
    daemon.sessions.write().insert(handle, Entry::Term(s.clone()));
    s
}

fn trigger(event: EventKind, glob: &str) -> Option<Trigger> {
    Some(Trigger { event, glob: glob.into(), daily_cap: 5 })
}

fn agent_automation(id: &str, cwd: &str, event: EventKind, glob: &str) -> Value {
    json!({"id": id, "name": id, "kind": "claude", "prompt": "update the tasks", "cwd": cwd, "every": "daily", "trigger": trigger(event, glob), "dry_run": true})
}

fn command_automation(id: &str, cwd: &str, event: EventKind, glob: &str, program: &str, argv: &[&str]) -> Value {
    json!({"id": id, "name": id, "cwd": cwd, "trigger": trigger(event, glob), "action": "shell", "shell": ShellAction { program: program.into(), argv: argv.iter().map(|a| a.to_string()).collect(), approved_hash: None }})
}

/// A user connection on `client`, the app when `app`.
fn connect(daemon: &Arc<Daemon>, client: u64, app: bool) -> (ClientTx, tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>) {
    let (tx, rx) = crate::session::client_channel();
    daemon.register_client(client, tx.clone());
    daemon.set_client_role(client, &Role::User);
    daemon.mark_app(client, app);
    (tx, rx)
}

fn call(daemon: &Arc<Daemon>, client: u64, tx: &ClientTx, method: &str, params: Value) -> Result<Value> {
    daemon.dispatch(client, tx, &json!(1), method, params).map(|v| v.unwrap())
}

fn note_of(daemon: &Daemon, id: &str) -> (Option<String>, Option<String>, u32) {
    let list = daemon.automations.lock();
    let a = list.list.iter().find(|a| a.id == id).unwrap();
    (a.last_note.clone(), a.last_error.clone(), a.cap_count)
}

const METHODS: &[&str] = &["automation.list", "automation.state", "automation.save", "automation.delete", "automation.run", "automation.approve", "automation.set_paused", "automation.resolve", "automation.event"];

#[test]
fn programs_in_a_session_can_neither_change_nor_start_automations_whatever_they_send() {
    let (daemon, dir) = test_daemon("auth");
    add_session(&daemon, "mine", "claude", &dir.to_string_lossy());
    let valid_automation = command_automation("x", "C:/p", EventKind::File, "*", "tool", &[]);
    let shapes = [
        Value::Null,
        json!({}),
        json!([]),
        json!([valid_automation.clone()]),
        json!("automation.save"),
        json!(7),
        json!(true),
        json!({"id": "x"}),
        valid_automation,
        json!({"kind": "file", "root": "C:/p", "files": ["a.md"]}),
        json!({"id": "x", "hash": "h"}),
        json!({"paused": true}),
        json!({"id": "x", "dry_run": true}),
        json!({"id": "x", "by_session": "mine", "session_id": "mine"}),
    ];
    for scripting in [true, false] {
        daemon.agents_control_others.store(scripting, Ordering::Relaxed);
        for role in [Role::Session("mine".into()), Role::Claimed("mine".into()), Role::Anonymous] {
            for method in METHODS {
                for shape in &shapes {
                    let mut params = shape.clone();
                    let verdict = crate::auth::authorize(&daemon, &role, method, &mut params);
                    assert!(verdict.is_err(), "{role:?} (scripting {scripting}) may not call {method} with {shape}");
                }
            }
        }
    }
    // The user's own connection is not held back by this check (the app-only methods are checked in the daemon).
    for method in METHODS {
        assert!(crate::auth::authorize(&daemon, &Role::User, method, &mut Value::Null).is_ok(), "{method}");
    }
    assert!(daemon.automations.lock().list.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn only_the_app_saves_commands_approves_them_and_reports_events() {
    let (daemon, dir) = test_daemon("app-only");
    let (script_tx, _script_rx) = connect(&daemon, 1, false);
    let (app_tx, _app_rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    let cmd = command_automation("c", &proj, EventKind::File, "*.md", "tool", &["{file}"]);
    // A script with the user's token has everything but this.
    let err = call(&daemon, 1, &script_tx, "automation.save", cmd.clone()).unwrap_err().to_string();
    assert!(err.contains("only available to the Kelpie app"), "{err}");
    assert!(daemon.automations.lock().list.is_empty());
    call(&daemon, 1, &script_tx, "automation.save", agent_automation("a", &proj, EventKind::Turn, "")).unwrap();
    assert!(call(&daemon, 1, &script_tx, "automation.event", json!({"kind": "file", "root": proj, "files": ["a.md"]})).is_err());
    assert!(call(&daemon, 1, &script_tx, "automation.approve", json!({"id": "c", "hash": "h"})).is_err());
    // The app can.
    let saved = call(&daemon, 2, &app_tx, "automation.save", cmd.clone()).unwrap();
    assert!(saved["shell"]["approved_hash"].is_null(), "saving approves nothing");
    // The same command saved from a script is refused even to toggle it.
    let mut toggled = saved.clone();
    toggled["enabled"] = json!(false);
    assert!(call(&daemon, 1, &script_tx, "automation.save", toggled).is_err());
    assert!(call(&daemon, 2, &app_tx, "automation.approve", json!({"id": "c", "hash": "stale"})).is_err());
    let hash = daemon.automations.lock().list.iter().find(|a| a.id == "c").unwrap().command_hash().unwrap();
    assert!(call(&daemon, 2, &app_tx, "automation.approve", json!({"id": "c", "hash": hash})).unwrap()["shell"]["approved_hash"].as_str() == Some(hash.as_str()));
    // Events: the three the window can see. Not turns or commits, which are the daemon's own to see.
    for kind in ["file", "pr_checks_failed", "finished"] {
        call(&daemon, 2, &app_tx, "automation.event", json!({"kind": kind, "root": proj, "files": ["a.md"]})).unwrap();
    }
    for kind in ["turn", "commit"] {
        let err = call(&daemon, 2, &app_tx, "automation.event", json!({"kind": kind, "root": proj})).unwrap_err().to_string();
        assert!(err.contains("cannot report"), "{kind}: {err}");
    }
    assert!(call(&daemon, 2, &app_tx, "automation.event", json!({"kind": "file", "root": ""})).is_err());
    assert!(call(&daemon, 2, &app_tx, "automation.event", json!({"kind": "nonsense", "root": proj})).is_err());
    // A connection that authenticates again as something else is no longer the app, nor is one that goes away.
    daemon.mark_app(2, false);
    assert!(call(&daemon, 2, &app_tx, "automation.approve", json!({"id": "c", "hash": "h"})).is_err());
    daemon.mark_app(2, true);
    daemon.unregister_client(2);
    assert!(!daemon.is_app(2));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_file_event_runs_an_approved_command_once_and_a_dry_run_runs_nothing() {
    let (daemon, dir) = test_daemon("fire");
    let (tx, mut rx) = connect(&daemon, 2, true);
    let proj = dir.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let proj = proj.to_string_lossy().into_owned();
    // The test program itself, asked only to list the tests whose name contains the file's name.
    let exe = std::env::current_exe().unwrap().to_string_lossy().into_owned();
    call(&daemon, 2, &tx, "automation.save", command_automation("c", &proj, EventKind::File, "**/*.md", &exe, &["--list", "{file}"])).unwrap();
    call(&daemon, 2, &tx, "automation.save", agent_automation("dry", &proj, EventKind::File, "*.md")).unwrap();
    let hostile = "docs/$(echo pwned > pwned.txt); `id` \"q\".md";
    let send = |files: &[&str]| call(&daemon, 2, &tx, "automation.event", json!({"kind": "file", "root": proj, "files": files})).unwrap();
    // Not approved yet: it does not run, and the user is told once.
    send(&[hostile]);
    wait_for("the dry run", || note_of(&daemon, "dry").0.is_some());
    assert_eq!(note_of(&daemon, "c").1.as_deref(), Some(crate::automations::NOT_APPROVED));
    let told: Vec<Value> = events(&mut rx).into_iter().filter(|e| e["event"] == "notify.event").collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(daemon.sessions.read().is_empty(), "a dry run starts nothing");
    let (note, _, _) = note_of(&daemon, "dry");
    assert!(note.unwrap().starts_with("dry run: would start claude"), "dry run says what it would do");
    // Approved: it runs, once, however many times the same change is reported within the minute.
    let hash = daemon.automations.lock().list.iter().find(|a| a.id == "c").unwrap().command_hash().unwrap();
    call(&daemon, 2, &tx, "automation.approve", json!({"id": "c", "hash": hash})).unwrap();
    send(&[hostile, "notes.txt"]);
    wait_for("the command to finish", || note_of(&daemon, "c").0.as_deref().is_some_and(|n| n.contains("finished")));
    send(&[hostile]);
    send(&[hostile]);
    std::thread::sleep(Duration::from_millis(300));
    let (note, error, count) = note_of(&daemon, "c");
    assert!(error.is_none(), "{error:?}");
    assert_eq!(count, 1, "{note:?}: one run in a minute");
    assert!(!PathBuf::from(&proj).join("pwned.txt").exists() && !std::env::current_dir().unwrap().join("pwned.txt").exists());
    // "Run now" with a dry run shows the literal arguments and runs nothing.
    let shown = call(&daemon, 2, &tx, "automation.run", json!({"id": "c", "dry_run": true})).unwrap();
    assert_eq!(shown["detail"]["args"], json!(["--list", "example.md"]));
    assert_eq!(shown["detail"]["program"].as_str(), Some(exe.as_str()));
    assert!(shown["note"].as_str().unwrap().starts_with("dry run: would run"));
    // The command's own folder and the event data are what it gets; nothing of a session's identity.
    let env = shown["detail"]["env"].as_array().unwrap();
    assert!(env.iter().any(|e| e[0] == "KELPIE_EVENT" && e[1] == "manual"));
    assert!(env.iter().all(|e| !e[0].as_str().unwrap().contains("TOKEN")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_global_switch_and_the_caps_stop_runs() {
    let (daemon, dir) = test_daemon("switch");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    let mut a = agent_automation("a", &proj, EventKind::Turn, "");
    a["trigger"]["daily_cap"] = json!(1);
    call(&daemon, 2, &tx, "automation.save", a).unwrap();
    assert_eq!(call(&daemon, 2, &tx, "automation.state", Value::Null).unwrap()["paused"], false);
    call(&daemon, 2, &tx, "automation.set_paused", json!({"paused": true})).unwrap();
    assert_eq!(call(&daemon, 2, &tx, "automation.state", Value::Null).unwrap()["paused"], true);
    assert!(call(&daemon, 2, &tx, "automation.set_paused", json!({"paused": "yes"})).is_err());
    daemon.automation_event(Event::new(EventKind::Turn, &proj));
    std::thread::sleep(Duration::from_millis(200));
    assert!(note_of(&daemon, "a").0.is_none(), "paused: nothing runs");
    // Still paused after a restart.
    let again = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    assert!(again.automations.lock().paused);
    call(&daemon, 2, &tx, "automation.set_paused", json!({"paused": false})).unwrap();
    daemon.automation_event(Event::new(EventKind::Turn, &proj));
    wait_for("the run", || note_of(&daemon, "a").0.is_some());
    assert_eq!(note_of(&daemon, "a").2, 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn what_an_automation_started_does_not_set_it_off_again() {
    let (daemon, dir) = test_daemon("busy");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    call(&daemon, 2, &tx, "automation.save", agent_automation("a", &proj, EventKind::File, "*.md")).unwrap();
    let mine = add_session(&daemon, "mine", "claude", &proj);
    let saved = daemon.automations.lock().list[0].clone();
    // The automation's session is working: its edits are not events for it.
    daemon.adopt_session(&saved, "mine");
    daemon.auto_rt.busy.lock().quiet_until.clear();
    let _typed = mine.fake_running(Status::Working);
    assert!(daemon.automation_busy("a", now_millis()));
    let ev = Event { files: vec!["a.md".into()], ..Event::new(EventKind::File, &proj) };
    daemon.automation_event(ev.clone());
    std::thread::sleep(Duration::from_millis(200));
    assert!(note_of(&daemon, "a").0.is_none());
    // It stops; for a few seconds after, the watcher's late reports of its last edits are ignored too.
    let _typed = mine.fake_running(Status::Idle);
    assert!(!daemon.automation_busy("a", now_millis()), "idle and quiet");
    daemon.automation_turn_ended(&mine.info());
    assert!(daemon.automation_busy("a", now_millis()), "just stopped");
    assert!(!daemon.automation_busy("a", now_millis() + QUIET_AFTER_MS + 1));
    daemon.automation_event(ev.clone());
    std::thread::sleep(Duration::from_millis(200));
    assert!(note_of(&daemon, "a").0.is_none());
    // Later, a real change runs it.
    daemon.auto_rt.busy.lock().quiet_until.clear();
    daemon.automation_event(ev);
    wait_for("the run", || note_of(&daemon, "a").0.is_some());
    // A command being run counts as busy until it ends.
    daemon.auto_rt.busy.lock().running.insert("a".into());
    assert!(daemon.automation_busy("a", now_millis()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_turn_ending_sets_off_turn_automations_in_that_project_only() {
    let (daemon, dir) = test_daemon("turn");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.join("proj");
    let other = dir.join("other");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let (proj, other) = (proj.to_string_lossy().into_owned(), other.to_string_lossy().into_owned());
    call(&daemon, 2, &tx, "automation.save", agent_automation("here", &proj, EventKind::Turn, "")).unwrap();
    call(&daemon, 2, &tx, "automation.save", agent_automation("there", &other, EventKind::Turn, "")).unwrap();
    let sh = add_session(&daemon, "plain", "shell", &proj);
    daemon.automation_turn_ended(&sh.info());
    std::thread::sleep(Duration::from_millis(200));
    assert!(note_of(&daemon, "here").0.is_none(), "a plain shell is not an agent");
    let agent = add_session(&daemon, "agent", "claude", &proj);
    daemon.automation_turn_ended(&agent.info());
    wait_for("the run", || note_of(&daemon, "here").0.is_some());
    assert!(note_of(&daemon, "there").0.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"]).args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn commit_automations_run_when_head_moves_and_not_when_checkpoint_refs_do() {
    let (daemon, dir) = test_daemon("commit");
    let (tx, _rx) = connect(&daemon, 2, true);
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "1").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let root = repo.to_string_lossy().into_owned();
    call(&daemon, 2, &tx, "automation.save", agent_automation("c", &root, EventKind::Commit, "")).unwrap();
    daemon.check_head(&root, None, false);
    // A turn checkpoint: refs/kelpie moves, HEAD does not.
    std::fs::write(repo.join("a.txt"), "2").unwrap();
    git(&repo, &["add", "."]);
    let tree = git(&repo, &["write-tree"]);
    let snapshot = git(&repo, &["commit-tree", &tree, "-p", "HEAD", "-m", "checkpoint"]);
    git(&repo, &["update-ref", "refs/kelpie/s1/1", &snapshot]);
    daemon.check_head(&root, None, true);
    daemon.poll_commits(false);
    std::thread::sleep(Duration::from_millis(600));
    assert!(note_of(&daemon, "c").0.is_none(), "checkpoint refs are not commits");
    // A commit is.
    git(&repo, &["commit", "-q", "-m", "two"]);
    daemon.check_head(&root, None, true);
    wait_for("the run", || note_of(&daemon, "c").0.is_some());
    assert_eq!(note_of(&daemon, "c").2, 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_damaged_automations_file_reaches_the_inbox_once_the_app_is_there() {
    let dir = std::env::temp_dir().join(format!("kelpie-autod-damaged-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("automations.json"), "{ not json").unwrap();
    let daemon = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    daemon.announce_automation_problem();
    let (_script_tx, mut script_rx) = connect(&daemon, 3, false);
    daemon.announce_automation_problem();
    assert!(events(&mut script_rx).iter().all(|e| e["event"] != "notify.event"), "a script is not somebody who read it");
    let (tx, mut rx) = connect(&daemon, 2, true);
    daemon.announce_automation_problem();
    let told: Vec<Value> = events(&mut rx).into_iter().filter(|e| e["event"] == "notify.event").collect();
    assert_eq!(told.len(), 1);
    assert!(told[0]["params"]["error"].as_str().unwrap().contains("damaged"), "{told:?}");
    daemon.announce_automation_problem();
    assert!(events(&mut rx).iter().all(|e| e["event"] != "notify.event"), "only once");
    // Saving is refused and says why; the damaged file is not replaced.
    let err = call(&daemon, 2, &tx, "automation.save", agent_automation("a", "C:/p", EventKind::Turn, "")).unwrap_err().to_string();
    assert!(err.contains("damaged"), "{err}");
    assert!(!dir.join("automations.json").exists());
    assert!(call(&daemon, 2, &tx, "automation.state", Value::Null).unwrap()["error"].as_str().unwrap().contains("damaged"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn ten_due_automations_do_not_hold_up_the_ticker() {
    let (daemon, dir) = test_daemon("ticker");
    let proj = dir.to_string_lossy().into_owned();
    {
        let mut store = daemon.automations.lock();
        for i in 0..10 {
            // A command that is not approved fails at once; what matters is that nothing waits for a run.
            let mut a: Automation = serde_json::from_value(command_automation(&format!("d{i}"), &proj, EventKind::Turn, "", "tool", &[])).unwrap();
            a.trigger = None;
            a.every = crate::automations::Every::Hourly;
            store.upsert(a).unwrap();
        }
    }
    let empty = {
        let started = Instant::now();
        daemon.run_due_automations();
        started.elapsed()
    };
    let now = now_millis();
    for a in daemon.automations.lock().list.iter_mut() {
        a.next_run = now - 1_000;
    }
    let started = Instant::now();
    daemon.run_due_automations();
    let ten = started.elapsed();
    eprintln!("run_due_automations: none due {empty:?}, ten due {ten:?}");
    assert!(ten < Duration::from_millis(250), "{ten:?}");
    wait_for("every run to have been tried", || (0..10).all(|i| note_of(&daemon, &format!("d{i}")).1.is_some()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_saved_prompt_is_typed_only_into_a_session_the_automation_started() {
    let (daemon, dir) = test_daemon("quick");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    let mut a = agent_automation("q", &proj, EventKind::File, "*.md");
    a["action"] = json!("quick");
    a["dry_run"] = json!(false);
    a["prompt"] = json!("line one\nline two\u{1b}[201~ rm -rf ~");
    call(&daemon, 2, &tx, "automation.save", a).unwrap();
    let saved = daemon.automations.lock().list[0].clone();
    // The user's own idle session of that very agent, in that very folder, with a half-written message.
    let mine = add_session(&daemon, "users-own", "claude", &proj);
    let theirs = mine.fake_running(Status::Idle);
    let ev = Event { files: vec!["notes.md".into()], ..Event::new(EventKind::File, &proj) };
    // The automation has started none yet: nothing to type into (the caller starts one), and the user's is untouched.
    assert_eq!(daemon.type_into_own(&saved, &ev).unwrap(), None);
    assert!(theirs.try_iter().next().is_none(), "never typed into a session the user opened");
    // One it started: typed as one line with no escape that could end a paste, then Enter on its own.
    let started = add_session(&daemon, "started-by-q", "claude", &proj);
    let typed_into = started.fake_running(Status::Idle);
    daemon.adopt_session(&saved, "started-by-q");
    assert_eq!(daemon.type_into_own(&saved, &ev).unwrap().as_deref(), Some("started-by-q"));
    let sent: Vec<Vec<u8>> = typed_into.try_iter().filter_map(|c| if let crate::session::Ctl::Input(b) = c { Some(b) } else { None }).collect();
    assert_eq!(sent.len(), 1);
    let text = String::from_utf8(sent[0].clone()).unwrap();
    assert!(!text.contains('\u{1b}') && !text.contains('\n'), "{text:?}");
    assert!(text.starts_with("line one line two[201~ rm -rf ~") && text.contains("Triggered by: files changed"), "{text:?}");
    wait_for("Enter", || typed_into.try_iter().any(|c| matches!(c, crate::session::Ctl::Input(b) if b == b"\r")));
    // A moment later it is not typed into again (it may be somebody's half-written message now), and not
    // while it works: an error, not a second session.
    let err = daemon.type_into_own(&saved, &ev).unwrap_err().to_string();
    assert!(err.contains("nothing was typed"), "{err}");
    let _working = started.fake_running(Status::Working);
    assert!(daemon.type_into_own(&saved, &ev).is_err());
    assert!(theirs.try_iter().next().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_waiting_on_a_question_is_still_busy() {
    let (daemon, dir) = test_daemon("waiting");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    call(&daemon, 2, &tx, "automation.save", agent_automation("a", &proj, EventKind::File, "*.md")).unwrap();
    let saved = daemon.automations.lock().list[0].clone();
    let s = add_session(&daemon, "mine", "claude", &proj);
    daemon.adopt_session(&saved, "mine");
    daemon.auto_rt.busy.lock().quiet_until.clear();
    for (status, busy) in [(Status::Working, true), (Status::Waiting, true), (Status::Idle, false)] {
        let _keep = s.fake_running(status);
        assert_eq!(daemon.automation_busy("a", now_millis()), busy, "{status:?}");
    }
    // A session that is gone is not busy, whatever it last was.
    daemon.sessions.write().clear();
    assert!(!daemon.automation_busy("a", now_millis()));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_automation_set_off_by_another_ones_work_is_one_step_further_down_a_chain() {
    let (daemon, dir) = test_daemon("chain");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    for id in ["a", "b", "c", "d"] {
        call(&daemon, 2, &tx, "automation.save", agent_automation(id, &proj, EventKind::File, "*.md")).unwrap();
    }
    let saved = daemon.automations.lock().list.clone();
    let mut sessions = Vec::new();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        let s = add_session(&daemon, &format!("s-{id}"), "claude", &proj);
        sessions.push(s.fake_running(Status::Working));
        daemon.adopt_session(&saved[i], &format!("s-{id}"));
        daemon.auto_rt.busy.lock().depth.insert(id.to_string(), i as u8);
    }
    daemon.auto_rt.busy.lock().quiet_until.clear();
    // Three automations are busy in the folder, the longest chain two links long: a fourth would be the third.
    let ev = Event { files: vec!["x.md".into()], ..Event::new(EventKind::File, &proj) };
    daemon.automation_event(ev.clone());
    std::thread::sleep(Duration::from_millis(300));
    let d = daemon.automations.lock().list.iter().find(|a| a.id == "d").cloned().unwrap();
    assert_eq!(d.last_error.as_deref(), Some(crate::automations::CHAIN_LIMIT));
    assert!(d.last_note.is_none() && d.cap_count == 0, "not run");
    // With only the first two busy it runs, and is recorded one step further.
    daemon.sessions.write().retain(|_, e| e.spec().id != "s-c");
    daemon.auto_rt.busy.lock().sessions.remove("s-c");
    daemon.automation_event(ev);
    wait_for("the run", || note_of(&daemon, "d").0.is_some());
    assert_eq!(daemon.auto_rt.busy.lock().depth.get("d"), Some(&2));
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn a_command_that_runs_too_long_is_stopped_with_what_it_started() {
    let (daemon, dir) = test_daemon("timeout");
    drop(daemon);
    // cmd starts ping (a grandchild) that would run for 40 s and keeps cmd's output open.
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/d", "/c", "ping -n 40 127.0.0.1 > nul"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().unwrap();
    let tree = crate::proc_tree::ProcTree::adopt(child.id(), None, None).unwrap();
    let readers = (child.stdout.take().map(keep_end), child.stderr.take().map(keep_end));
    wait_for("ping to start", || !crate::proc_tree::children_for_test(child.id(), "ping.exe").is_empty());
    let started = Instant::now();
    let result = wait_command(&mut child, Some(&tree), Duration::from_millis(400));
    assert!(result.unwrap_err().to_string().contains("stopped after"));
    assert!(crate::proc_tree::children_for_test(child.id(), "ping.exe").is_empty() || { std::thread::sleep(Duration::from_millis(500)); crate::proc_tree::children_for_test(child.id(), "ping.exe").is_empty() }, "the grandchild is gone");
    join_by(readers.0, READERS_GRACE);
    join_by(readers.1, READERS_GRACE);
    assert!(started.elapsed() < Duration::from_secs(10), "{:?}: nothing waited for the 40 s ping", started.elapsed());
    let _ = std::fs::remove_dir_all(dir);
}

/// A folder of scripts and a project to run them in, under the system temp folder.
#[cfg(windows)]
fn script_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-autod-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[cfg(windows)]
#[test]
fn a_real_batch_file_and_a_real_powershell_script_get_hostile_file_names_literally() {
    let (daemon, dir) = test_daemon("scripts");
    let (tx, _rx) = connect(&daemon, 2, true);
    let work = script_dir("work");
    // A name made to run something in cmd (& > ^ %), in PowerShell ($( ) `), and in a POSIX shell.
    let hostile = "n $(New-Item pwned_ps.txt) `New-Item pwned_tick.txt`; it's & echo pwned_amp>amp.txt & echo y>cmd_pwned.txt ^caret %PATH%.md";
    // The batch file is a script that treats what it is given as data: it quotes it.
    std::fs::write(work.join("show.cmd"), "@echo off\r\n>\"%~dp0cmd-args.txt\" echo \"%~1\"\r\n>\"%~dp0cmd-env.txt\" echo \"%KELPIE_FILE%\"\r\n").unwrap();
    std::fs::write(work.join("show.ps1"), "Set-Content -LiteralPath (Join-Path $PSScriptRoot 'ps-args.txt') -Value $args[0] -Encoding UTF8\r\nSet-Content -LiteralPath (Join-Path $PSScriptRoot 'ps-env.txt') -Value $env:KELPIE_FILE -Encoding UTF8\r\n").unwrap();
    let root = work.to_string_lossy().into_owned();
    // A program relative to the automation's folder (this test's own folder is somewhere else), and one found by name in PATH.
    call(&daemon, 2, &tx, "automation.save", command_automation("cmd", &root, EventKind::File, "*.md", ".\\show.cmd", &["{file}"])).unwrap();
    call(&daemon, 2, &tx, "automation.save", command_automation("ps", &root, EventKind::File, "*.md", "powershell", &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "show.ps1", "{file}"])).unwrap();
    // The forms that would run the name as code cannot even be saved.
    for (program, argv) in [("powershell", vec!["-NoProfile", "-Command", "{file}"]), ("cmd", vec!["/c", "echo", "{file}"])] {
        let err = call(&daemon, 2, &tx, "automation.save", command_automation("bad", &root, EventKind::File, "*.md", program, &argv)).unwrap_err().to_string();
        assert!(err.contains("script file"), "{program}: {err}");
    }
    for id in ["cmd", "ps"] {
        let hash = daemon.automations.lock().list.iter().find(|a| a.id == id).unwrap().command_hash().unwrap();
        call(&daemon, 2, &tx, "automation.approve", json!({"id": id, "hash": hash})).unwrap();
    }
    daemon.automation_event(Event { files: vec![hostile.into()], ..Event::new(EventKind::File, &root) });
    for id in ["cmd", "ps"] {
        // A real powershell.exe: on a busy machine it alone can take over 10 s to start (measured), with the whole test suite beside it.
        // This is only how long the wait may last: it ends as soon as the command has finished.
        wait_for_within(Duration::from_secs(90), &format!("{id} to finish"), || note_of(&daemon, id).0.as_deref().is_some_and(|n| n.contains("finished")));
        assert_eq!(note_of(&daemon, id).1, None, "{id}");
    }
    let read = |name: &str| std::fs::read_to_string(work.join(name)).unwrap_or_default();
    let (cmd_args, cmd_env, ps_args, ps_env) = (read("cmd-args.txt"), read("cmd-env.txt"), read("ps-args.txt"), read("ps-env.txt"));
    assert!(cmd_args.contains(hostile), "the batch file got the name as one argument: {cmd_args:?}");
    assert!(cmd_env.contains(hostile), "…and in the environment: {cmd_env:?}");
    assert_eq!(ps_args.trim_start_matches('\u{feff}').trim(), hostile, "powershell -File got it as one argument");
    assert_eq!(ps_env.trim_start_matches('\u{feff}').trim(), hostile);
    let strays: Vec<String> = [&work, &std::env::current_dir().unwrap(), &std::env::temp_dir()]
        .iter()
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("pwned") || n == "amp.txt" || n == "cmd_pwned.txt" || n == "y")
        .collect();
    assert!(strays.is_empty(), "nothing in the name was executed: {strays:?}");
    let _ = std::fs::remove_dir_all(work);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_chain_follows_recent_runs_even_when_nothing_is_busy_any_more() {
    let (daemon, dir) = test_daemon("chain-recent");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    for id in ["a", "b", "c", "d"] {
        call(&daemon, 2, &tx, "automation.save", agent_automation(id, &proj, EventKind::File, &format!("{id}.md"))).unwrap();
    }
    let ev = |name: &str| Event { files: vec![name.into()], ..Event::new(EventKind::File, &proj) };
    // Each run is dry and ends at once: no session, no command is ever busy. Still, runs within two minutes
    // of each other in a folder are one chain: 0, 1, 2, and the fourth link is not run.
    for (id, depth) in [("a", 0u8), ("b", 1), ("c", 2)] {
        daemon.automation_event(ev(&format!("{id}.md")));
        wait_for(&format!("{id} to run"), || note_of(&daemon, id).0.is_some());
        assert_eq!(daemon.auto_rt.busy.lock().depth.get(id), Some(&depth), "{id}");
    }
    daemon.automation_event(ev("d.md"));
    std::thread::sleep(Duration::from_millis(300));
    let d = daemon.automations.lock().list.iter().find(|a| a.id == "d").cloned().unwrap();
    assert_eq!(d.last_error.as_deref(), Some(crate::automations::CHAIN_LIMIT));
    assert!(d.last_note.is_none());
    // Two minutes on, it was something else.
    for t in daemon.auto_rt.busy.lock().fired.values_mut() {
        *t = t.saturating_sub(CHAIN_WINDOW_MS + 1);
    }
    daemon.automation_event(ev("d.md"));
    wait_for("d to run", || note_of(&daemon, "d").0.is_some());
    assert_eq!(daemon.auto_rt.busy.lock().depth.get("d"), Some(&0));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_commit_made_while_a_baseline_is_taken_is_not_swallowed() {
    let (daemon, dir) = test_daemon("baseline");
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "1").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "one"]);
    let (tx, _rx) = connect(&daemon, 2, true);
    let root = repo.to_string_lossy().into_owned();
    call(&daemon, 2, &tx, "automation.save", agent_automation("c", &root, EventKind::Commit, "")).unwrap();
    daemon.check_head(&root, None, false);
    // Saving the automation again (a toggle does) records a baseline only where there is none.
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "two"]);
    daemon.check_head(&root, None, false);
    assert!(note_of(&daemon, "c").0.is_none());
    daemon.check_head(&root, None, true);
    wait_for("the commit to run it", || note_of(&daemon, "c").0.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[cfg(windows)]
#[test]
fn what_a_command_launched_is_left_running_when_it_ends_normally() {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/d", "/c", "ping -n 20 127.0.0.1 > nul"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().unwrap();
    let tree = crate::proc_tree::ProcTree::adopt(child.id(), None, None).unwrap();
    wait_for("ping to start", || !crate::proc_tree::children_for_test(child.id(), "ping.exe").is_empty());
    let pings = crate::proc_tree::children_for_test(child.id(), "ping.exe");
    tree.release();
    drop(tree);
    std::thread::sleep(Duration::from_millis(500));
    assert!(child.try_wait().unwrap().is_none(), "closing the released job did not kill cmd");
    assert!(!crate::proc_tree::children_for_test(child.id(), "ping.exe").is_empty(), "…nor what it started");
    // Tidy up what the test started.
    let _ = child.kill();
    for pid in pings {
        let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    }
}

#[cfg(windows)]
#[test]
fn a_real_bash_script_gets_hostile_file_names_literally() {
    let bash = PathBuf::from(r"C:\Program Files\Git\bin\bash.exe");
    if !bash.is_file() {
        eprintln!("no Git for Windows bash here: skipped");
        return;
    }
    let (daemon, dir) = test_daemon("bash-script");
    let (tx, _rx) = connect(&daemon, 2, true);
    let work = script_dir("bash-work");
    let hostile = "b $(touch pwned_sh.txt) `touch pwned_tick.txt`; it's & echo pwned_amp ^caret %PATH%.md";
    std::fs::write(work.join("hook.sh"), "printf '%s' \"$1\" > sh-args.txt\nprintf '%s' \"$KELPIE_FILE\" > sh-env.txt\n").unwrap();
    let root = work.to_string_lossy().into_owned();
    call(&daemon, 2, &tx, "automation.save", command_automation("sh", &root, EventKind::File, "*.md", &bash.to_string_lossy(), &["hook.sh", "{file}"])).unwrap();
    // `-c` in any form is not accepted, with or without the name.
    let err = call(&daemon, 2, &tx, "automation.save", command_automation("bad", &root, EventKind::File, "*.md", &bash.to_string_lossy(), &["-c", "echo {file}"]));
    assert!(err.is_ok(), "no event data in a `-c` command is fine");
    let err = call(&daemon, 2, &tx, "automation.save", command_automation("bad2", &root, EventKind::File, "*.md", &bash.to_string_lossy(), &["-c", "{file}"])).unwrap_err().to_string();
    assert!(err.contains("script file"), "{err}");
    let hash = daemon.automations.lock().list.iter().find(|a| a.id == "sh").unwrap().command_hash().unwrap();
    call(&daemon, 2, &tx, "automation.approve", json!({"id": "sh", "hash": hash})).unwrap();
    daemon.automation_event(Event { files: vec![hostile.into()], ..Event::new(EventKind::File, &root) });
    wait_for("the script to finish", || note_of(&daemon, "sh").0.as_deref().is_some_and(|n| n.contains("finished")));
    let read = |n: &str| std::fs::read_to_string(work.join(n)).unwrap_or_default();
    assert_eq!(read("sh-args.txt"), hostile);
    assert_eq!(read("sh-env.txt"), hostile);
    let strays: Vec<String> = std::fs::read_dir(&work).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.starts_with("pwned")).collect();
    assert!(strays.is_empty(), "{strays:?}");
    let _ = std::fs::remove_dir_all(work);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_command_cannot_have_caused_a_finished_turn() {
    let (daemon, dir) = test_daemon("chain-kinds");
    let (tx, _rx) = connect(&daemon, 2, true);
    let proj = dir.to_string_lossy().into_owned();
    let mut cmd = command_automation("cmd", &proj, EventKind::File, "*.md", "tool", &[]);
    cmd["dry_run"] = json!(true);
    call(&daemon, 2, &tx, "automation.save", cmd).unwrap();
    call(&daemon, 2, &tx, "automation.save", agent_automation("t", &proj, EventKind::Turn, "")).unwrap();
    call(&daemon, 2, &tx, "automation.save", agent_automation("c", &proj, EventKind::Commit, "")).unwrap();
    {
        let mut b = daemon.auto_rt.busy.lock();
        b.fired.insert("cmd".into(), now_millis());
        b.depth.insert("cmd".into(), 1);
    }
    // A finished turn is the end of an agent's work, not of a command's.
    daemon.automation_event(Event::new(EventKind::Turn, &proj));
    wait_for("t to run", || note_of(&daemon, "t").0.is_some());
    assert_eq!(daemon.auto_rt.busy.lock().depth.get("t"), Some(&0));
    // A commit may be what the command did.
    let mut ev = Event::new(EventKind::Commit, &proj);
    ev.commit = Some("abc".into());
    ev.branch = Some("main".into());
    daemon.automation_event(ev);
    wait_for("c to run", || note_of(&daemon, "c").0.is_some());
    assert_eq!(daemon.auto_rt.busy.lock().depth.get("c"), Some(&2), "the command is a link of the chain");
    let _ = std::fs::remove_dir_all(dir);
}
