use super::*;
use crate::auth::Role;
use crate::session::fake;

fn daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
}

/// A running agent session without a process; what is typed into it arrives on the returned receiver.
fn add(d: &Daemon, handle: u32, id: &str, kind: &str, parent: Option<(&str, u8)>) -> (Arc<Session>, fake::Typed) {
    let (session, typed) = fake::running(handle, id, kind);
    {
        let mut spec = session.spec.lock();
        spec.name = Some(id.into());
        spec.parent = parent.map(|(p, _)| p.to_string());
        spec.depth = parent.map_or(0, |(_, depth)| depth);
    }
    // The ticker would have looked at it by now: its status is what the daemon reads.
    session.tick(Instant::now());
    d.insert_for_test(Entry::Term(session.clone()));
    (session, typed)
}

fn call(d: &Arc<Daemon>, method: &str, params: Value) -> Result<Value> {
    let (tx, _rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), method, params).map(|v| v.expect("answered at once"))
}

fn decoded(rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(o) = rx.try_recv() {
        out.push(serde_json::from_slice(&o.data[5..]).unwrap());
    }
    out
}

fn mail_from(text: &str) -> NewMail {
    NewMail { from_id: "x".into(), from_label: "x".into(), kind: MailKind::Message, status: None, text: text.into() }
}

fn request(id: &str, session: &str) -> Request {
    Request { id: id.into(), session: session.into(), from_label: "x".into(), agent: "codex".into(), cwd: ".".into(), worktree: false, name: None, task: "t".into(), model: None, skips_prompts: false, at: 1 }
}

#[test]
fn the_policy_defaults_to_asking_and_survives_a_restart_with_unknown_keys_kept() {
    let (d, dir) = daemon("sub-policy");
    let p = call(&d, "daemon.policy", json!({})).unwrap();
    assert_eq!(p["agents_control_others"], true);
    assert_eq!(p["sub_sessions"], "ask");
    assert_eq!(p["sub_max"], 0);
    assert_eq!(p["sub_auto_max"], subs::machine_cap());
    // A file from before sub-sessions, plus a key some other version wrote.
    std::fs::write(dir.join(POLICY_FILE), r#"{"agents_control_others": false, "from_the_future": [1, 2]}"#).unwrap();
    let old = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    let p = call(&old, "daemon.policy", json!({})).unwrap();
    assert_eq!((p["agents_control_others"].clone(), p["sub_sessions"].clone(), p["sub_max"].clone()), (json!(false), json!("ask"), json!(0)));
    let set = call(&old, "daemon.set_policy", json!({"sub_sessions": "allow", "sub_max": 3})).unwrap();
    assert_eq!((set["sub_sessions"].clone(), set["sub_max"].clone(), set["agents_control_others"].clone()), (json!("allow"), json!(3), json!(false)));
    let file: Value = serde_json::from_slice(&std::fs::read(dir.join(POLICY_FILE)).unwrap()).unwrap();
    assert_eq!(file["from_the_future"], json!([1, 2]));
    let again = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    let p = call(&again, "daemon.policy", json!({})).unwrap();
    assert_eq!((p["sub_sessions"].clone(), p["sub_max"].clone(), p["agents_control_others"].clone()), (json!("allow"), json!(3), json!(false)));
    // The old setting alone changes only itself.
    let only = call(&again, "daemon.set_policy", json!({"agents_control_others": true})).unwrap();
    assert_eq!((only["sub_sessions"].clone(), only["agents_control_others"].clone()), (json!("allow"), json!(true)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_bad_policy_value_changes_nothing() {
    let (d, dir) = daemon("sub-policy-bad");
    for bad in [json!({"sub_sessions": "yes"}), json!({"sub_sessions": 1}), json!({"sub_max": -1}), json!({"sub_max": 65}), json!({"sub_max": "3"})] {
        let mut params = bad.clone();
        params["agents_control_others"] = json!(false);
        assert!(call(&d, "daemon.set_policy", params).is_err(), "{bad}");
        let p = call(&d, "daemon.policy", json!({})).unwrap();
        assert_eq!((p["agents_control_others"].clone(), p["sub_sessions"].clone(), p["sub_max"].clone()), (json!(true), json!("ask"), json!(0)), "{bad}");
    }
    assert!(!dir.join(POLICY_FILE).exists(), "nothing was saved");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_program_cannot_speak_for_another_session_or_for_the_user() {
    let (d, dir) = daemon("sub-auth");
    add(&d, 1, "c", "codex", Some(("p", 1)));
    let me = Role::Session("c".into());
    for method in ["sub.spawn", "sub.message", "sub.report", "sub.inbox", "sub.close"] {
        let mut forged = json!({"session_id": "someone-else", "status": "done"});
        crate::auth::authorize(&d, &me, method, &mut forged).unwrap();
        assert_eq!(forged["session_id"], "c", "{method}");
    }
    for method in ["sub.answer", "sub.pending"] {
        assert!(crate::auth::authorize(&d, &me, method, &mut json!({"id": "x", "decision": "once"})).is_err(), "{method}");
    }
    // A restricted session reports and reads its own mail, and nothing else of this.
    let (restricted, _) = add(&d, 2, "r", "claude", Some(("p", 1)));
    restricted.spec.lock().restricted = true;
    let me = Role::Session("r".into());
    for method in ["sub.report", "sub.inbox"] {
        assert!(crate::auth::authorize(&d, &me, method, &mut json!({})).is_ok(), "{method}");
    }
    for method in ["sub.spawn", "sub.message", "sub.close"] {
        assert!(crate::auth::authorize(&d, &me, method, &mut json!({})).is_err(), "{method}");
    }
    // The events about requests are the user's alone.
    for event in ["sub.request", "sub.request_done"] {
        for role in [Role::Session("c".into()), Role::Claimed("c".into()), Role::Anonymous] {
            assert!(matches!(crate::auth::event_visibility(&d, &role, event, &json!({}), None), crate::auth::EventVisibility::Hidden), "{event} {role:?}");
        }
        assert!(matches!(crate::auth::event_visibility(&d, &Role::User, event, &json!({}), None), crate::auth::EventVisibility::Full));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_report_shows_on_the_row_and_goes_to_the_parent_that_reads_or_receives_it() {
    let (d, dir) = daemon("sub-report");
    add(&d, 1, "p", "claude", None);
    let (child, _) = add(&d, 2, "c", "codex", Some(("p", 1)));
    let out = d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "done", "summary": "fixed #1 \u{1b}[31mred"})).unwrap();
    assert!(out["delivered_to"].as_str().unwrap().starts_with("session \"p\""), "{out}");
    let row = child.spec.lock().report.clone().unwrap();
    assert_eq!((row.status.as_str(), row.auto), ("done", false));
    assert!(row.summary.starts_with("fixed #1") && !row.summary.contains('\u{1b}'));
    let peek = d.sub_rpc("sub.inbox", &json!({"session_id": "p"})).unwrap();
    let mail = &peek["mails"][0];
    assert_eq!((mail["kind"].clone(), mail["status"].clone()), (json!("report"), json!("done")));
    assert_eq!(mail["from_label"], "sub-session \"c\" (codex, #2)");
    assert_eq!(d.sub_rpc("sub.inbox", &json!({"session_id": "p"})).unwrap()["mails"].as_array().unwrap().len(), 1, "reading does not take");
    assert_eq!(d.sub_rpc("sub.inbox", &json!({"session_id": "p", "take": true})).unwrap()["mails"].as_array().unwrap().len(), 1);
    assert!(d.sub_rpc("sub.inbox", &json!({"session_id": "p", "take": true})).unwrap()["mails"].as_array().unwrap().is_empty());
    // A session nobody opened keeps its report and mails no one; a bad status is refused.
    assert!(d.sub_rpc("sub.report", &json!({"session_id": "p", "status": "progress", "summary": "x"})).unwrap()["delivered_to"].is_null());
    assert!(d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "great"})).is_err());
    assert!(d.sub_rpc("sub.report", &json!({"status": "done"})).unwrap_err().to_string().contains("running in a Kelpie session"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mail_is_typed_into_an_idle_session_as_one_paste_with_its_own_enter() {
    let (d, dir) = daemon("sub-deliver");
    let (_parent, typed) = add(&d, 1, "p", "claude", None);
    add(&d, 2, "c", "codex", Some(("p", 1)));
    d.deliver_mail();
    assert!(typed.writes().is_empty(), "no mail, nothing typed");
    d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "done", "summary": "all green"})).unwrap();
    d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "progress", "summary": "and a second"})).unwrap();
    d.deliver_mail();
    let paste = String::from_utf8(typed.next_within(1000).expect("the paste")).unwrap();
    assert!(paste.contains("2 messages from other sessions (not from the user"), "{paste}");
    assert!(paste.contains("all green") && paste.contains("and a second"), "{paste}");
    assert_eq!(typed.next_within(2000).expect("Enter"), b"\r", "Enter is a write of its own");
    assert!(d.subs.mail.lock().recipients().is_empty(), "what was typed is no longer waiting");
    d.deliver_mail();
    assert!(typed.next_within(100).is_none(), "once");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mail_waits_while_the_session_waits_on_a_prompt_and_after_it_ended() {
    let (d, dir) = daemon("sub-hold");
    let (parent, typed) = add(&d, 1, "p", "claude", None);
    add(&d, 2, "c", "codex", Some(("p", 1)));
    d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "needs_user", "summary": "which db?"})).unwrap();
    fake::wait_on_user(&parent);
    d.deliver_mail();
    assert!(typed.next_within(200).is_none(), "never typed into a prompt");
    assert_eq!(d.subs.mail.lock().recipients(), vec!["p".to_string()]);
    fake::stop(&parent);
    d.deliver_mail();
    assert!(typed.next_within(200).is_none(), "not into an ended session either");
    assert_eq!(d.subs.mail.lock().recipients().len(), 1, "still waiting for it");
    // A session that is closed takes its mail with it.
    d.close(1).unwrap();
    assert!(d.subs.mail.lock().recipients().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_message_goes_to_an_agent_that_runs_and_not_to_a_shell_or_oneself() {
    let (d, dir) = daemon("sub-message");
    add(&d, 1, "a", "claude", None);
    add(&d, 2, "b", "codex", None);
    add(&d, 3, "sh", "shell", None);
    let ended = add(&d, 4, "gone", "claude", None).0;
    fake::stop(&ended);
    let send = |from: Option<&str>, to: &str, text: &str| {
        let mut params = json!({"to": to, "text": text});
        if let Some(from) = from {
            params["session_id"] = json!(from);
        }
        d.sub_rpc("sub.message", &params)
    };
    let sent = send(Some("a"), "b", "which port?").unwrap();
    assert_eq!((sent["queued"].clone(), sent["to"]["handle"].clone()), (json!(true), json!(2)));
    assert_eq!(send(Some("a"), "b", "which port?").unwrap()["queued"], false, "the same text again is one mail");
    assert!(send(Some("a"), "a", "hi").unwrap_err().to_string().contains("itself"));
    assert!(send(Some("a"), "sh", "rm -rf x").unwrap_err().to_string().contains("not an agent"));
    assert!(send(Some("a"), "gone", "hi").unwrap_err().to_string().contains("not running"));
    assert!(send(Some("a"), "nobody", "hi").is_err());
    assert!(send(Some("a"), "b", "   ").is_err());
    // The user's own message (no session) is "you".
    send(None, "b", "from a script of the user").unwrap();
    let mails = d.sub_rpc("sub.inbox", &json!({"session_id": "b"})).unwrap();
    let labels: Vec<&str> = mails["mails"].as_array().unwrap().iter().map(|m| m["from_label"].as_str().unwrap()).collect();
    assert_eq!(labels, ["session \"a\" (claude, #1)", "a program run by the user"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_closes_its_own_descendants_and_nobody_elses() {
    let (d, dir) = daemon("sub-close");
    add(&d, 1, "p", "claude", None);
    add(&d, 2, "c", "codex", Some(("p", 1)));
    add(&d, 3, "g", "codex", Some(("c", 2)));
    add(&d, 4, "other", "claude", None);
    let close = |caller: &str, target: &str| d.sub_rpc("sub.close", &json!({"session_id": caller, "session": target}));
    assert!(close("p", "other").unwrap_err().to_string().contains("can close only the sub-sessions it opened"));
    assert!(close("c", "p").is_err(), "not an ancestor");
    assert!(close("g", "c").is_err());
    assert!(close("c", "c").is_err(), "not itself");
    d.subs.mail.lock().add("g", mail_from("hi"), Instant::now(), 1, true).unwrap();
    assert_eq!(close("p", "g").unwrap(), json!(true), "a grandchild is a descendant");
    assert!(d.by_id("g").is_none() && d.subs.mail.lock().recipients().is_empty(), "its mail went with it");
    assert_eq!(close("p", "2").unwrap(), json!(true), "by handle");
    assert!(d.by_id("c").is_none() && d.by_id("p").is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_spawn_is_refused_in_the_order_of_the_caps_before_anything_is_asked() {
    let (d, dir) = daemon("sub-caps");
    let (caller, _) = add(&d, 1, "p", "claude", None);
    let spawn = |agent: &str| call(&d, "sub.spawn", json!({"session_id": "p", "agent": agent, "task": "t"})).unwrap_err().to_string();
    assert!(call(&d, "sub.spawn", json!({"agent": "codex", "task": "t"})).unwrap_err().to_string().contains("running in a Kelpie session"));
    d.subs.policy.write().mode = Mode::Off;
    assert!(spawn("codex").contains("turned off"));
    d.subs.policy.write().mode = Mode::Allow;
    assert!(spawn("no-such-agent").contains("unknown agent `no-such-agent`"));
    assert!(spawn("shell").contains("unknown agent"), "only agents the user could open");
    caller.spec.lock().depth = 3;
    assert!(spawn("codex").contains("3 levels"));
    caller.spec.lock().depth = 0;
    caller.spec.lock().restricted = true;
    assert!(spawn("codex").contains("restricted"));
    caller.spec.lock().restricted = false;
    // The computer's limit, counted from the sessions that run.
    d.subs.policy.write().max = 2;
    add(&d, 2, "k1", "codex", Some(("elsewhere", 1)));
    let (second, _) = add(&d, 3, "k2", "codex", Some(("elsewhere", 1)));
    assert_eq!(spawn("codex"), "2 sub-sessions are running, the limit on this computer; wait for one with kelpie_wait or close one");
    // A child that ended no longer counts (the next spawn would have a place; this test never lets one start).
    assert_eq!(d.live_sub_counts("p", &Default::default()), (0, 2));
    fake::stop(&second);
    assert_eq!(d.live_sub_counts("p", &Default::default()), (0, 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_request_for_an_installed_agent_is_shown_to_the_user_and_a_denial_answers_the_caller() {
    let Some(agent) = agents::installed_presets().first().copied() else { return };
    let (d, dir) = daemon("sub-ask");
    add(&d, 1, "p", "claude", None);
    // The user's window hears of the request; a program in a session does not.
    let (user_tx, mut user_rx) = crate::session::client_channel();
    d.register_client(7, user_tx.clone());
    d.set_client_role(7, &Role::User);
    let (program_tx, mut program_rx) = crate::session::client_channel();
    d.register_client(8, program_tx);
    d.set_client_role(8, &Role::Session("p".into()));
    let folder = dir.to_string_lossy().into_owned();
    let task = "t".repeat(900);
    let asked = d.dispatch(7, &user_tx, &json!(5), "sub.spawn", json!({"session_id": "p", "agent": agent, "task": task, "cwd": folder, "worktree": true, "name": "fix"})).unwrap();
    assert!(asked.is_none(), "the reply waits for the user");
    let pending = call(&d, "sub.pending", json!({})).unwrap();
    let request = &pending[0];
    assert_eq!((request["agent"].clone(), request["name"].clone(), request["worktree"].clone()), (json!(agent), json!("fix"), json!(true)));
    assert_eq!(request["task"].as_str().unwrap().chars().count(), subs::TASK_PREVIEW);
    assert_eq!(request["from_label"], "session \"p\" (claude, #1)");
    let seen = decoded(&mut user_rx);
    assert!(seen.iter().any(|e| e["event"] == "sub.request" && e["params"]["id"] == request["id"]), "{seen:?}");
    assert!(decoded(&mut program_rx).iter().all(|e| e["event"] != "sub.request"));
    // A bad decision and an unknown request are refused.
    assert!(call(&d, "sub.answer", json!({"id": request["id"], "decision": "maybe"})).is_err());
    assert!(call(&d, "sub.answer", json!({"id": "nope", "decision": "once"})).is_err());
    assert_eq!(call(&d, "sub.answer", json!({"id": request["id"], "decision": "deny"})).unwrap(), json!(true));
    let reply = loop {
        let o = user_rx.blocking_recv().expect("a reply");
        let v: Value = serde_json::from_slice(&o.data[5..]).unwrap();
        if v.get("id") == Some(&json!(5)) {
            break v;
        }
    };
    assert!(reply["error"]["message"].as_str().unwrap().contains("denied"), "{reply}");
    assert!(call(&d, "sub.pending", json!({})).unwrap().as_array().unwrap().is_empty());
    assert!(call(&d, "sub.answer", json!({"id": request["id"], "decision": "once"})).is_err(), "answered once");
    // Nothing was started.
    assert_eq!(d.list().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn allowing_for_a_session_or_always_is_remembered_as_far_as_it_says() {
    let (d, dir) = daemon("sub-grant");
    add(&d, 1, "p", "claude", None);
    // The waiting end of each request is kept alive for the test.
    let mut waiters = Vec::new();
    let mut open = |id: &str, session: &str| waiters.push(d.subs.pending.lock().open(request(id, session)).unwrap());
    open("r1", "p");
    d.sub_answer(&json!({"id": "r1", "decision": "once"})).unwrap();
    assert!(!d.subs.granted("p"));
    open("r2", "p");
    d.sub_answer(&json!({"id": "r2", "decision": "session"})).unwrap();
    assert!(d.subs.granted("p"));
    assert_eq!(d.subs.policy.read().mode, Mode::Ask, "only that session");
    d.close(1).unwrap();
    assert!(!d.subs.granted("p"), "the grant goes with the session");
    add(&d, 2, "q", "claude", None);
    open("r3", "q");
    d.sub_answer(&json!({"id": "r3", "decision": "always"})).unwrap();
    assert_eq!(d.subs.policy.read().mode, Mode::Allow);
    let file: Value = serde_json::from_slice(&std::fs::read(dir.join(POLICY_FILE)).unwrap()).unwrap();
    assert_eq!(file["sub_sessions"], "allow");
    // Closing the asking session withdraws what it asked.
    open("r4", "q");
    d.close(2).unwrap();
    assert!(d.subs.pending.lock().list().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_automatic_report_follows_the_child_to_its_row_and_its_parent() {
    let (d, dir) = daemon("sub-auto");
    add(&d, 1, "p", "claude", None);
    let (child, _) = add(&d, 2, "c", "codex", Some(("p", 1)));
    let info = |status: Status, code: Option<i32>| {
        let mut i = d.by_id("c").unwrap().info();
        i.status = status;
        i.exit_code = code;
        i
    };
    let entry = d.by_id("c").unwrap();
    let t0 = Instant::now();
    d.sub_status_changed(&entry, &info(Status::Working, None), t0);
    d.sub_status_changed(&entry, &info(Status::Idle, None), t0 + Duration::from_secs(10));
    let row = child.spec.lock().report.clone().unwrap();
    assert_eq!((row.status.as_str(), row.auto), ("idle", true));
    // The agent speaks for itself in the next turn: its words stay on the row when the daemon sees the turn end.
    d.sub_status_changed(&entry, &info(Status::Working, None), t0 + Duration::from_secs(20));
    d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "done", "summary": "merged"})).unwrap();
    d.sub_status_changed(&entry, &info(Status::Idle, None), t0 + Duration::from_secs(30));
    d.sub_status_changed(&entry, &info(Status::Exited, Some(1)), t0 + Duration::from_secs(31));
    // Going idle after it reported leaves its words on the row; the exit is news and replaces them.
    let row = child.spec.lock().report.clone().unwrap();
    assert_eq!((row.status.as_str(), row.auto), ("exited", true));
    let mails = d.sub_rpc("sub.inbox", &json!({"session_id": "p"})).unwrap();
    let said: Vec<(String, String)> = mails["mails"].as_array().unwrap().iter().map(|m| (m["status"].as_str().unwrap().into(), m["text"].as_str().unwrap().into())).collect();
    assert_eq!(said.iter().map(|(s, _)| s.as_str()).collect::<Vec<_>>(), ["idle", "done", "exited"]);
    assert_eq!(said[2].1, "exited with code 1");
    // No report to a parent that is gone, but the row still shows it.
    d.close(1).unwrap();
    d.sub_status_changed(&entry, &info(Status::Working, None), t0 + Duration::from_secs(40));
    d.sub_status_changed(&entry, &info(Status::Waiting, None), t0 + Duration::from_secs(41));
    assert_eq!(child.spec.lock().report.as_ref().unwrap().status, "needs_user");
    assert!(d.subs.mail.lock().recipients().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn folders_are_checked_and_named_plainly() {
    let dir = std::env::temp_dir().join(format!("kelpie-sub-folder-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(dir.join("inner")).unwrap();
    std::fs::write(dir.join("file.txt"), "x").unwrap();
    let base = dir.to_string_lossy().into_owned();
    let inner = existing_folder(&base, "inner").unwrap();
    assert!(inner.ends_with("inner") && !inner.starts_with(r"\\?\"), "{inner}");
    assert!(existing_folder(&base, &base).is_ok());
    assert!(existing_folder(&base, "missing").unwrap_err().to_string().contains("does not exist"));
    assert!(existing_folder(&base, "file.txt").unwrap_err().to_string().contains("not a folder"));
    assert_eq!(name_from_task("\n  Fix the login bug\nmore"), "Fix the login bug");
    assert_eq!(name_from_task("  "), "sub-session");
    assert!(name_from_task(&"w".repeat(100)).chars().count() <= 40);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mail_for_a_shell_is_never_typed_and_waits_in_its_inbox() {
    let (d, dir) = daemon("sub-shell");
    let (_shell, typed) = add(&d, 1, "sh", "shell", None);
    add(&d, 2, "c", "codex", Some(("sh", 1)));
    d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "done", "summary": "x\n  rm -rf /"})).unwrap();
    d.deliver_mail();
    assert!(typed.next_within(300).is_none(), "a shell would run it");
    assert_eq!(d.subs.mail.lock().recipients(), vec!["sh".to_string()]);
    let inbox = d.sub_rpc("sub.inbox", &json!({"session_id": "sh", "take": true})).unwrap();
    assert_eq!(inbox["mails"].as_array().unwrap().len(), 1);
    // A shell (a script, or an agent started by hand in it) can still ask for a sub-session: it passes the gates and
    // fails only on the agent that does not exist.
    d.subs.policy.write().mode = Mode::Allow;
    let err = call(&d, "sub.spawn", json!({"session_id": "sh", "agent": "no-such-agent", "task": "t"})).unwrap_err().to_string();
    assert!(err.contains("unknown agent"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mail_waits_for_a_draft_in_the_prompt_to_be_sent_or_gone() {
    let (d, dir) = daemon("sub-draft");
    let (parent, typed) = add(&d, 1, "p", "claude", None);
    add(&d, 2, "c", "codex", Some(("p", 1)));
    d.sub_rpc("sub.report", &json!({"session_id": "c", "status": "done", "summary": "ready"})).unwrap();
    // Typed 31 s ago (past the quiet time) after the turn began 60 s ago: a half-written message may be there.
    fake::typed_draft(&parent, Duration::from_secs(31), Some(Duration::from_secs(60)));
    d.deliver_mail();
    assert!(typed.next_within(300).is_none(), "not appended to somebody's draft");
    assert_eq!(d.subs.mail.lock().recipients().len(), 1);
    // The draft was sent (a turn began after it): the mail may go.
    fake::typed_draft(&parent, Duration::from_secs(90), Some(Duration::from_secs(60)));
    d.deliver_mail();
    assert!(typed.next_within(1000).is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_that_asks_before_acting_cannot_make_one_that_does_not_ask_act() {
    let (d, dir) = daemon("sub-yolo");
    add(&d, 1, "careful", "claude", None);
    let (bold, _) = add(&d, 2, "bold", "claude", None);
    bold.spec.lock().args = vec!["--dangerously-skip-permissions".into()];
    let (bold_codex, _) = add(&d, 3, "daring", "codex", None);
    bold_codex.spec.lock().args = vec!["--dangerously-bypass-approvals-and-sandbox".into()];
    let say = |from: Option<&str>, to: &str| {
        let mut params = json!({"to": to, "text": "do this"});
        if let Some(from) = from {
            params["session_id"] = json!(from);
        }
        d.sub_rpc("sub.message", &params)
    };
    let refused = say(Some("careful"), "bold").unwrap_err().to_string();
    assert!(refused.contains("permission checks off") && refused.contains("Ask the user"), "{refused}");
    assert!(say(Some("careful"), "daring").is_err());
    assert!(say(Some("bold"), "daring").is_ok(), "one that skips its prompts too");
    assert!(say(Some("bold"), "careful").is_ok(), "towards a session that asks is fine");
    assert!(say(None, "bold").is_ok(), "the user always may");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn session_create_from_a_session_is_a_capped_sub_session_and_the_users_is_unchanged() {
    let (d, dir) = daemon("sub-create");
    add(&d, 1, "p", "claude", None);
    d.register_client(7, crate::session::client_channel().0);
    d.set_client_role(7, &Role::Session("p".into()));
    d.register_client(8, crate::session::client_channel().0);
    d.set_client_role(8, &Role::User);
    let folder = dir.to_string_lossy().into_owned();
    let create = |client: u64, params: Value| {
        let (tx, _rx) = crate::session::client_channel();
        d.dispatch(client, &tx, &json!(1), "session.create", params).map(|v| v.expect("answered at once")).unwrap_err().to_string()
    };
    // The user: the old door, with its own errors only.
    assert!(create(8, json!({"kind": "custom", "cwd": folder, "args": []})).contains("custom session needs a command"));
    // A program in a session: agents only, and nothing that gets round the gates.
    for kind in ["shell", "custom"] {
        let why = create(7, json!({"kind": kind, "cwd": folder}));
        assert!(why.contains("agent sessions only"), "{why}");
    }
    assert!(create(7, json!({"cwd": folder})).contains("agent sessions only"), "shell is the default kind");
    for (key, value) in [("id", json!("p")), ("env", json!({"A": "b"})), ("chat_argv", json!(["x"])), ("remote", json!({"dest": "h"})), ("agent_session_id", json!("c1"))] {
        let mut params = json!({"kind": "codex", "cwd": folder});
        params[key] = value;
        d.subs.policy.write().mode = Mode::Allow;
        // Reaches the parse (the policy lets it), where the key is refused before anything else.
        let why = create(7, params);
        assert!(why.contains(&format!("cannot set `{key}`")), "{key}: {why}");
    }
    // The policy and the caps apply as for sub.spawn.
    d.subs.policy.write().mode = Mode::Off;
    assert!(create(7, json!({"kind": "codex", "cwd": folder})).contains("turned off"));
    d.subs.policy.write().mode = Mode::Allow;
    d.subs.policy.write().max = 1;
    add(&d, 2, "k1", "codex", Some(("elsewhere", 1)));
    assert!(create(7, json!({"kind": "codex", "cwd": folder})).contains("1 sub-session is running"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_create_child_is_linked_and_gets_only_the_model_the_caller_chose() {
    let (d, dir) = daemon("sub-create-args");
    let (caller, _) = add(&d, 1, "p", "claude", None);
    caller.spec.lock().depth = 1;
    caller.spec.lock().project_id = Some("proj".into());
    let spec = caller.spec.lock().clone();
    let known = ["claude", "codex", "omp"];
    let folder = dir.to_string_lossy().into_owned();
    let plan = |params: Value| d.parse_create(&params, &spec, &known, &known).unwrap();
    let careful = plan(json!({"kind": "claude", "cwd": folder, "args": ["--model", "x"], "prompt": "  do it ", "name": "n", "cols": 90, "rows": 30, "restricted": true, "project_id": "other"}));
    assert_eq!(careful.model.as_deref(), Some("x"));
    let (params, depth) = child_params(&careful, &spec, false, "session p", &folder, Some("n"));
    assert_eq!(depth, 2, "one below the caller");
    assert_eq!(params["args"], json!(["--model", "x"]), "the prompts stay on");
    assert_eq!((params["project_id"].clone(), params["prompt"].clone(), params["cols"].clone(), params["restricted"].clone()), (json!("proj"), json!("do it"), json!(90), json!(true)));
    let codex = plan(json!({"kind": "codex", "cwd": folder}));
    assert_eq!(child_params(&codex, &spec, false, "p", &folder, None).0["args"], json!([]));
    assert_eq!(child_params(&codex, &spec, false, "p", &folder, None).0["prompt"], Value::Null, "no prompt, none sent");
    // Nothing but a model, whatever the agent and whoever asks.
    for (kind, args) in [
        ("omp", json!(["--auto-approve"])),
        ("omp", json!(["--approval-mode=yolo"])),
        ("claude", json!(["--allowedTools", "Bash"])),
        ("claude", json!(["--mcp-config", "{}"])),
        ("claude", json!(["--settings", "{\"permissions\":{\"defaultMode\":\"bypassPermissions\"}}"])),
        ("codex", json!(["-c", "approval_policy=\"never\""])),
        ("codex", json!(["-p", "any-profile"])),
        ("codex", json!(["--model", "m", "--full-auto"])),
        ("claude", json!(["--dangerously-skip-permissions"])),
    ] {
        let why = d.parse_create(&json!({"kind": kind, "cwd": folder, "args": args}), &spec, &known, &known).err().unwrap().to_string();
        assert!(why.contains("only a model"), "{kind} {args}: {why}");
    }
    // A caller that runs without asking gives its child the child agent's own flags, and the same model.
    let mut bold = spec.clone();
    bold.args = vec!["--permission-mode".into(), "bypassPermissions".into()];
    assert!(skips_permissions(&Entry::Term({
        let (s, _) = add(&d, 5, "bold", "claude", None);
        s.spec.lock().args = bold.args.clone();
        s
    })));
    let (params, _) = child_params(&careful, &bold, true, "p", &folder, Some("n"));
    assert_eq!(params["args"], json!(["--dangerously-skip-permissions", "--model", "x"]));
    let (params, _) = child_params(&plan(json!({"kind": "omp", "cwd": folder})), &bold, true, "p", &folder, None);
    assert_eq!(params["args"], json!(["--auto-approve"]), "omp has its flag now");
    // The sub.spawn shape: the brief and the model; the agent's own flags only when the caller skips prompts.
    let sub = d.parse_spawn(&json!({"agent": "codex", "task": "fix it", "model": "m"}), &spec, &known, &known).unwrap();
    let (params, depth) = child_params(&sub, &spec, false, "session p", &folder, Some("fix it"));
    assert_eq!((depth, params["args"].clone()), (2, json!(["--model", "m"])));
    assert!(params["prompt"].as_str().unwrap().contains("Task:\nfix it"));
    let (params, _) = child_params(&sub, &bold, true, "session p", &folder, Some("fix it"));
    assert_eq!(params["args"], json!(["--dangerously-bypass-approvals-and-sandbox", "--model", "m"]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_that_skips_prompts_is_recognised_by_its_arguments_and_by_a_chats_mode() {
    let (d, dir) = daemon("sub-skips");
    for (kind, args) in [
        ("claude", vec!["--permission-mode", "bypassPermissions"]),
        ("claude", vec!["--allowedTools", "Bash"]),
        ("omp", vec!["--auto-approve"]),
        ("codex", vec!["-a", "never", "-s", "danger-full-access"]),
        ("codex", vec!["-c", "approval_policy=never"]),
        ("gemini", vec!["--approval-mode=yolo"]),
    ] {
        let (s, _) = add(&d, 9, "x", kind, None);
        s.spec.lock().args = args.iter().map(|a| a.to_string()).collect();
        assert!(skips_permissions(&Entry::Term(s)), "{kind} {args:?}");
    }
    let (plain, _) = add(&d, 10, "y", "claude", None);
    assert!(!skips_permissions(&Entry::Term(plain)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_id_a_closed_parent_left_on_its_children_cannot_be_taken_over() {
    let (d, dir) = daemon("sub-id");
    add(&d, 1, "c", "codex", Some(("gone-parent", 1)));
    let why = d.create(json!({"id": "gone-parent", "kind": "custom", "cwd": "."})).unwrap_err().to_string();
    assert!(why.contains("history of a sub-session"), "{why}");
    let ok = d.create(json!({"id": "fresh", "kind": "custom", "cwd": "."})).unwrap_err().to_string();
    assert!(ok.contains("custom session needs a command"), "an ordinary id gets as far as starting: {ok}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_worktree_made_for_a_session_that_would_not_start_is_taken_back() {
    let dir = std::env::temp_dir().join(format!("kelpie-sub-wt-{}", uuid::Uuid::new_v4()));
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| assert!(std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(&repo).status().unwrap().success());
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("a.txt"), "a").unwrap();
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);
    let cwd = repo.to_string_lossy().into_owned();
    let w = create_worktree(&cwd, "sub-fix-ab12", None, "a sub-session worktree needs the folder").unwrap();
    assert!(std::path::Path::new(&w.path).is_dir());
    assert_eq!(discard_worktree(&cwd, &w), None);
    assert!(!std::path::Path::new(&w.path).exists());
    assert!(run_git(&["-C", &cwd, "branch", "--list", &w.branch]).unwrap().is_empty(), "the branch went too");
    let err = create_worktree(&dir.to_string_lossy(), "x", None, "a sub-session worktree needs the folder").err().unwrap().to_string();
    assert!(err.contains("git repository"), "{err}");
    assert_eq!(branch_slug("   "), "task");
    assert_eq!(branch_slug("Run tests"), "run-tests");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unreadable_policy_file_is_kept_not_written_over() {
    let (d, dir) = daemon("sub-policy-keep");
    std::fs::write(dir.join(POLICY_FILE), "{ not json").unwrap();
    call(&d, "daemon.set_policy", json!({"sub_sessions": "allow"})).unwrap();
    assert_eq!(std::fs::read_to_string(dir.join(format!("{POLICY_FILE}.unreadable"))).unwrap(), "{ not json");
    let file: Value = serde_json::from_slice(&std::fs::read(dir.join(POLICY_FILE)).unwrap()).unwrap();
    assert_eq!(file["sub_sessions"], "allow");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_create_from_a_session_asks_the_user_like_a_spawn() {
    let Some(agent) = agents::installed_presets().first().copied() else { return };
    let (d, dir) = daemon("sub-create-ask");
    add(&d, 1, "p", "claude", None);
    let (tx, mut rx) = crate::session::client_channel();
    d.register_client(7, tx.clone());
    d.set_client_role(7, &Role::Session("p".into()));
    let folder = dir.to_string_lossy().into_owned();
    let asked = d.dispatch(7, &tx, &json!(9), "session.create", json!({"kind": agent, "cwd": folder})).unwrap();
    assert!(asked.is_none(), "waits for the user");
    let pending = call(&d, "sub.pending", json!({})).unwrap();
    assert_eq!(pending[0]["agent"], json!(agent));
    call(&d, "sub.answer", json!({"id": pending[0]["id"], "decision": "deny"})).unwrap();
    let reply = loop {
        let o = rx.blocking_recv().expect("a reply");
        let v: Value = serde_json::from_slice(&o.data[5..]).unwrap();
        if v.get("id") == Some(&json!(9)) {
            break v;
        }
    };
    assert!(reply["error"]["message"].as_str().unwrap().contains("denied"), "{reply}");
    assert_eq!(d.list().len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

fn new_run(folder: &str, tasks: Value) -> NewRun {
    serde_json::from_value(json!({"name": "R", "cwd": folder, "tasks": tasks})).unwrap()
}

#[test]
fn a_run_a_session_makes_is_asked_for_checked_and_kept_to_a_model() {
    let (d, dir) = daemon("sub-run-prepare");
    let (caller, _) = add(&d, 1, "p", "claude", None);
    caller.spec.lock().project_id = Some("proj".into());
    let spec = caller.spec.lock().clone();
    let folder = dir.to_string_lossy().into_owned();
    let known = ["claude", "codex"];
    let prepare = |tasks: Value| d.prepare_session_run("p", &spec, new_run(&folder, tasks), &known, &known);
    let task = |extra: Value| {
        let mut t = json!({"title": "t", "prompt": "p"});
        t.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        t
    };
    // Ask (the default): every task waits for the user on the Runs page.
    let asked = prepare(json!([task(json!({"kind": "codex", "args": ["--model", "m"]})), task(json!({"approve": false}))])).unwrap();
    assert!(asked.tasks.iter().all(|t| t.approve));
    assert_eq!(asked.tasks[0].args, vec!["--model", "m"]);
    assert_eq!(asked.tasks[1].kind.as_deref(), Some("claude"), "the default agent is named");
    assert_eq!(asked.project_id.as_deref(), Some("proj"));
    assert!(asked.materialize.is_none());
    // Allowed (or allowed for this session): the tasks start on their own, unless the program asked for approval itself.
    d.subs.policy.write().mode = Mode::Allow;
    let allowed = prepare(json!([task(json!({})), task(json!({"approve": true}))])).unwrap();
    assert_eq!(allowed.tasks.iter().map(|t| t.approve).collect::<Vec<_>>(), [false, true]);
    d.subs.policy.write().mode = Mode::Ask;
    d.subs.grant("p");
    assert!(!prepare(json!([task(json!({}))])).unwrap().tasks[0].approve);
    d.subs.forget("p");
    // Off: no run at all.
    d.subs.policy.write().mode = Mode::Off;
    assert!(prepare(json!([task(json!({}))])).unwrap_err().to_string().contains("turned off"));
    d.subs.policy.write().mode = Mode::Allow;
    // What a task of a program may not hold.
    for (extra, why) in [
        (json!({"kind": "shell"}), "unknown agent"),
        (json!({"kind": "custom", "args": ["cmd"]}), "unknown agent"),
        (json!({"kind": "codex", "args": ["--full-auto"]}), "only a model"),
        (json!({"args": ["--dangerously-skip-permissions"]}), "only a model"),
        (json!({"env": {"A": "b"}}), "cannot set `env`"),
        (json!({"host": {"dest": "h", "path": "/x"}}), "SSH host"),
    ] {
        let err = prepare(json!([task(extra.clone())])).unwrap_err().to_string();
        assert!(err.contains(why), "{extra}: {err}");
    }
    assert!(d.prepare_session_run("p", &spec, new_run(&dir.join("missing").to_string_lossy(), json!([task(json!({}))])), &known, &known).unwrap_err().to_string().contains("does not exist"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_create_from_a_session_is_gated_and_the_user_run_is_unchanged() {
    let (d, dir) = daemon("sub-run-create");
    add(&d, 1, "p", "claude", None);
    d.register_client(7, crate::session::client_channel().0);
    d.set_client_role(7, &Role::Session("p".into()));
    d.register_client(8, crate::session::client_channel().0);
    d.set_client_role(8, &Role::User);
    let folder = dir.to_string_lossy().into_owned();
    let create = |client: u64, kind: &str| {
        let (tx, _rx) = crate::session::client_channel();
        d.dispatch(client, &tx, &json!(1), "run.create", json!({"name": "R", "cwd": folder, "tasks": [{"title": "t", "prompt": "p", "kind": kind, "approve": false}]}))
    };
    // The user: a shell task, as ever; no creator.
    let made = create(8, "shell").unwrap().unwrap();
    assert!(made.get("creator").is_none());
    assert_eq!((made["tasks"][0]["kind"].clone(), made["tasks"][0]["approve"].clone()), (json!("shell"), json!(false)));
    // A program in a session: sub-sessions off refuses; otherwise a shell is no agent.
    d.subs.policy.write().mode = Mode::Off;
    assert!(create(7, "codex").unwrap_err().to_string().contains("turned off"));
    d.subs.policy.write().mode = Mode::Allow;
    assert!(create(7, "shell").unwrap_err().to_string().contains("unknown agent"));
    assert_eq!(d.runs.lock().list.len(), 1, "only the user run exists");
    if let Some(agent) = agents::installed_presets().first().copied() {
        let made = create(7, agent).unwrap().unwrap();
        assert_eq!(made["creator"], "p");
        assert_eq!(d.runs.lock().list.len(), 2);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_task_of_a_session_run_waits_for_room_and_fails_when_it_never_can_start() {
    let (d, dir) = daemon("sub-run-start");
    add(&d, 1, "p", "claude", None);
    let folder = dir.to_string_lossy().into_owned();
    let made = |creator: Option<&str>| {
        let mut run = runs::build_run(new_run(&folder, json!([{"id": "t", "title": "t", "prompt": "p", "kind": "codex", "args": ["--model", "m"]}])), uuid::Uuid::new_v4().to_string(), 0).unwrap();
        run.creator = creator.map(str::to_string);
        run.tasks[0].status = TaskStatus::Starting;
        let id = run.id.clone();
        d.runs.lock().list.insert(0, run);
        id
    };
    let status = |run: &str| {
        let runs = d.runs.lock();
        let t = &runs.get(run).unwrap().tasks[0];
        (t.status, t.error.clone())
    };
    // The limit is full: the task goes back to ready, nothing starts, nothing fails.
    d.subs.policy.write().mode = Mode::Allow;
    d.subs.policy.write().max = 1;
    add(&d, 2, "k1", "codex", Some(("elsewhere", 1)));
    let waiting = made(Some("p"));
    assert!(matches!(d.run_gate("p", &Default::default()), runs::Gate::Wait(_)));
    d.start_task(&waiting, "t");
    assert_eq!(status(&waiting), (TaskStatus::Pending, None));
    assert_eq!(d.list().len(), 2, "no session was started");
    // Sub-sessions turned off, or the session that made the run closed: it fails, with the reason.
    d.subs.policy.write().mode = Mode::Off;
    assert!(matches!(d.run_gate("p", &Default::default()), runs::Gate::Fail(_)));
    let off = made(Some("p"));
    d.start_task(&off, "t");
    let (state, error) = status(&off);
    assert_eq!(state, TaskStatus::Failed);
    assert!(error.unwrap().contains("turned off"));
    d.subs.policy.write().mode = Mode::Allow;
    d.close(1).unwrap();
    let gone = made(Some("p"));
    assert!(matches!(d.run_gate("p", &Default::default()), runs::Gate::Fail(_)));
    d.start_task(&gone, "t");
    assert!(status(&gone).1.unwrap().contains("closed"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_session_runs_tasks_get_the_model_and_only_a_skipping_creators_flags() {
    let (d, dir) = daemon("sub-run-args");
    let (careful, _) = add(&d, 1, "p", "claude", None);
    let (bold, _) = add(&d, 2, "b", "claude", None);
    bold.spec.lock().args = vec!["--permission-mode".into(), "bypassPermissions".into()];
    let folder = dir.to_string_lossy().into_owned();
    let run = runs::build_run(new_run(&folder, json!([{"title": "t", "prompt": "p", "kind": "codex", "args": ["--model", "m"]}])), "r1".into(), 0).unwrap();
    let task = &run.tasks[0];
    let (depth, args, _slot) = d.task_lineage("p", task).unwrap();
    assert_eq!((depth, args), (1, vec!["--model".to_string(), "m".to_string()]));
    let (_, args, _slot) = d.task_lineage("b", task).unwrap();
    assert_eq!(args, vec!["--dangerously-bypass-approvals-and-sandbox", "--model", "m"]);
    careful.spec.lock().depth = 3;
    assert!(matches!(d.task_lineage("p", task), Err(runs::Gate::Fail(_))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_child_inherits_only_from_a_parent_in_full_bypass() {
    let (d, dir) = daemon("sub-strict");
    let folder = dir.to_string_lossy().into_owned();
    let known = ["claude", "codex"];
    let parent_with = |handle: u32, id: &str, kind: &str, args: &[&str]| {
        let (s, _) = add(&d, handle, id, kind, None);
        s.spec.lock().args = args.iter().map(|a| a.to_string()).collect();
        s
    };
    let child_args = |parent: &Arc<Session>, agent: &str| {
        let spec = parent.spec.lock().clone();
        let plan = d.parse_spawn(&json!({"agent": agent, "task": "t"}), &spec, &known, &known).unwrap();
        child_params(&plan, &spec, bypasses_permissions(&Entry::Term(parent.clone())), "p", &folder, None).0["args"].clone()
    };
    // Partial or sandboxed ways of not asking: the child asks.
    for (kind, args) in [
        ("claude", vec!["--allowedTools", "Read"]),
        ("claude", vec!["--allowedTools", "Bash"]),
        ("codex", vec!["--full-auto"]),
        ("codex", vec!["-a", "never"]),
        ("codex", vec!["-c", "approval_policy=never"]),
    ] {
        let parent = parent_with(1, "x", kind, &args);
        assert!(skips_permissions(&Entry::Term(parent.clone())), "{kind} {args:?}: still the broad answer for messages");
        assert!(!bypasses_permissions(&Entry::Term(parent.clone())), "{kind} {args:?}");
        assert_eq!(child_args(&parent, "codex"), json!([]), "{kind} {args:?}");
        assert_eq!(child_args(&parent, "claude"), json!([]), "{kind} {args:?}");
    }
    // Full bypass: the child agent's own flags.
    for (kind, args) in [
        ("claude", vec!["--dangerously-skip-permissions"]),
        ("claude", vec!["--permission-mode=bypassPermissions"]),
        ("codex", vec!["--dangerously-bypass-approvals-and-sandbox"]),
        ("codex", vec!["--yolo"]),
    ] {
        let parent = parent_with(2, "y", kind, &args);
        assert_eq!(child_args(&parent, "claude"), json!(["--dangerously-skip-permissions"]), "{kind} {args:?}");
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_chat_started_in_bypass_is_known_by_its_command_line() {
    let claude_argv: Vec<String> = ["claude", "-p", "--input-format", "stream-json", "--permission-mode", "bypassPermissions"].iter().map(|a| a.to_string()).collect();
    let codex_argv: Vec<String> = ["codex", "app-server", "-c", "approval_policy=never"].iter().map(|a| a.to_string()).collect();
    let plain: Vec<String> = ["claude", "-p", "--permission-mode", "plan"].iter().map(|a| a.to_string()).collect();
    assert_eq!(subs::launch_permissions("claude", &["--dangerously-skip-permissions"], &claude_argv), (true, true));
    assert_eq!(subs::launch_permissions("codex", &[], &codex_argv), (true, false), "never asking, but sandboxed: not a bypass");
    assert_eq!(subs::launch_permissions("claude", &["--dangerously-skip-permissions"], &plain), (false, false));
    let chat = crate::chat::ChatSession::new(3, SessionSpec { id: "c".into(), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
    chat.set_launch_permissions(true, true);
    let entry = Entry::Chat(chat);
    assert!(skips_permissions(&entry) && bypasses_permissions(&entry));
}

#[test]
fn only_busy_children_count_and_a_run_of_three_tasks_finishes_under_a_cap_of_two() {
    let (d, dir) = daemon("sub-busy");
    add(&d, 1, "p", "claude", None);
    d.subs.policy.write().mode = Mode::Allow;
    d.subs.policy.write().max = 2;
    let (sa, _) = add(&d, 2, "sa", "codex", Some(("p", 1)));
    let (sb, _) = add(&d, 3, "sb", "codex", Some(("p", 1)));
    let folder = dir.to_string_lossy().into_owned();
    let mut run = runs::build_run(
        new_run(&folder, json!([{"id": "a", "title": "a", "prompt": "p", "kind": "codex"}, {"id": "b", "title": "b", "prompt": "p", "kind": "codex", "deps": ["a"]}, {"id": "c", "title": "c", "prompt": "p", "kind": "codex", "deps": ["b"]}])),
        "r1".into(),
        0,
    )
    .unwrap();
    run.creator = Some("p".into());
    // a and b are done; their agents idle on, alive.
    for (i, sid) in [(0, "sa"), (1, "sb")] {
        run.tasks[i].status = TaskStatus::Done;
        run.tasks[i].session_id = Some(sid.into());
    }
    d.runs.lock().list.insert(0, run);
    assert_eq!(d.live_sub_counts("p", &Default::default()), (2, 2), "counted as running, the two places are taken");
    assert_eq!(d.live_sub_counts("p", &d.finished_task_sessions()), (0, 0), "a finished task's agent holds no place");
    let (actions, _) = d.runs.lock().step_gated(1, |_| None, |creator, finished| d.run_gate(creator, finished));
    assert_eq!(actions, vec![Action::Start { run: "r1".into(), task: "c".into() }], "the third task starts");
    // The agent that reported itself done holds no place either.
    d.runs.lock().list[0].tasks[2].status = TaskStatus::Pending;
    let (busy, _) = add(&d, 4, "sc", "codex", Some(("p", 1)));
    assert_eq!(d.live_sub_counts("p", &d.finished_task_sessions()).0, 1);
    d.sub_rpc("sub.report", &json!({"session_id": "sc", "status": "done", "summary": "x"})).unwrap();
    assert_eq!(d.live_sub_counts("p", &d.finished_task_sessions()).0, 0);
    // Given more work, a finished child (by its report or by its run task) is busy again and counts.
    busy.set_hook_status(Status::Working, false);
    sa.set_hook_status(Status::Waiting, false);
    busy.tick(Instant::now());
    sa.tick(Instant::now());
    assert_eq!(d.live_sub_counts("p", &d.finished_task_sessions()), (2, 2), "working and waiting children hold their places");
    drop((busy, sa, sb));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_task_that_waits_for_room_says_why_once_and_tells_its_session_once() {
    let (d, dir) = daemon("sub-wait");
    add(&d, 1, "p", "claude", None);
    d.subs.policy.write().mode = Mode::Allow;
    d.subs.policy.write().max = 2;
    add(&d, 2, "s1", "codex", Some(("p", 1)));
    add(&d, 3, "s2", "codex", Some(("p", 1)));
    let folder = dir.to_string_lossy().into_owned();
    let mut run = runs::build_run(new_run(&folder, json!([{"id": "c", "title": "the third", "prompt": "p", "kind": "codex"}])), "r1".into(), 0).unwrap();
    run.creator = Some("p".into());
    d.runs.lock().list.insert(0, run);
    let step = |now: u64| d.runs.lock().step_gated(now, |_| None, |creator, finished| d.run_gate(creator, finished)).0;
    let actions = step(1);
    let [Action::Waiting { run, task, reason }] = actions.as_slice() else { panic!("{actions:?}") };
    assert_eq!(reason, "waiting for a free sub-session slot (2 of 2 busy)");
    assert_eq!(d.runs.lock().get("r1").unwrap().tasks[0].note.as_deref(), Some(reason.as_str()));
    d.announce_waiting(run, task, reason);
    assert!(step(2).is_empty() && step(3).is_empty(), "not at every tick");
    let mails = d.sub_rpc("sub.inbox", &json!({"session_id": "p"})).unwrap();
    assert_eq!(mails["mails"].as_array().unwrap().len(), 1);
    let mail = &mails["mails"][0];
    assert_eq!(mail["from_label"], "run \"R\"");
    assert!(mail["text"].as_str().unwrap().contains("the third") && mail["text"].as_str().unwrap().contains("2 of 2 busy"), "{mail}");
    // A place frees: it starts and the note goes.
    let Some(Entry::Term(s2)) = d.by_id("s2") else { unreachable!() };
    fake::stop(&s2);
    assert!(matches!(step(4).as_slice(), [Action::Start { .. }]));
    assert_eq!(d.runs.lock().get("r1").unwrap().tasks[0].note, None);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn alt_enter_and_backslash_enter_do_not_send_a_draft() {
    let (d, dir) = daemon("sub-keys");
    let blocked = |keys: &[&[u8]]| {
        let (s, _) = add(&d, 1, "p", "claude", None);
        for k in keys {
            s.input(k.to_vec());
        }
        s.type_mail_when_idle("m", Duration::ZERO).is_none()
    };
    assert!(blocked(&[b"half a sentence"]), "a draft");
    assert!(!blocked(&[b"a sentence", b"\r"]), "Enter sends it");
    assert!(blocked(&[b"line one", b"\x1b\r"]), "Alt+Enter is a new line");
    assert!(blocked(&[b"line one\\", b"\r"]), "backslash, Enter is a new line");
    assert!(blocked(&[b"line one\\\r"]), "in one write too");
    assert!(!blocked(&[b"line one\\", b"x", b"\r"]), "a backslash further back does not matter");
    assert!(!blocked(&[b"a sentence", b"\r", b"\x1b[D", b"\x1b[A"]), "cursor keys leave nothing to send");
    assert!(blocked(&[b"a sentence", b"\r", b"more"]), "a new draft after the send");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_run_of_a_session_at_the_deepest_level_is_refused_and_the_user_adopts_an_orphaned_one() {
    let (d, dir) = daemon("sub-adopt");
    let (deep, _) = add(&d, 1, "p", "claude", None);
    d.subs.policy.write().mode = Mode::Allow;
    let folder = dir.to_string_lossy().into_owned();
    let known = ["codex"];
    let mut spec = deep.spec.lock().clone();
    spec.depth = 3;
    let tasks = json!([{"title": "t", "prompt": "p", "kind": "codex"}]);
    assert!(d.prepare_session_run("p", &spec, new_run(&folder, tasks.clone()), &known, &known).unwrap_err().to_string().contains("levels deep"));
    spec.depth = 2;
    assert!(d.prepare_session_run("p", &spec, new_run(&folder, tasks), &known, &known).is_ok());
    // The creating session is closed; the user retries a failed task: the run is theirs now, and a session cannot do that.
    let mut run = runs::build_run(new_run(&folder, json!([{"id": "t", "title": "t", "prompt": "p", "kind": "codex"}])), "r1".into(), 0).unwrap();
    run.creator = Some("gone".into());
    run.tasks[0].status = TaskStatus::Failed;
    d.runs.lock().list.insert(0, run);
    d.register_client(7, crate::session::client_channel().0);
    d.set_client_role(7, &Role::Session("p".into()));
    d.register_client(8, crate::session::client_channel().0);
    d.set_client_role(8, &Role::User);
    let retry = |client: u64| {
        let (tx, _rx) = crate::session::client_channel();
        d.runs.lock().get_mut("r1").unwrap().tasks[0].status = TaskStatus::Failed;
        d.dispatch(client, &tx, &json!(1), "run.retry", json!({"id": "r1", "task": "t"})).unwrap();
        d.runs.lock().get("r1").unwrap().creator.clone()
    };
    assert_eq!(retry(7).as_deref(), Some("gone"), "a program in a session does not adopt it");
    assert_eq!(retry(8), None);
    let _ = std::fs::remove_dir_all(dir);
}
