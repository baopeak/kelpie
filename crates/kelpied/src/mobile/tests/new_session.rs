//! Starting a session from a phone (`new_options`, `new_session`) through a phone's own connection (`Conn::handle`), against the launch catalogue
//! the app sends (`mobile.set_launch`): what is refused and why, what a start is made of, and what is said about it.

use super::*;
use crate::agents::{self, AgentDef};

/// A plugin agent that is a program found everywhere (a harmless shell, never a real agent), and one whose program is nowhere.
fn fake_agents() -> parking_lot::MutexGuard<'static, ()> {
    let held = agents::CUSTOM_IN_TEST.lock();
    let bin = if cfg!(windows) { "cmd" } else { "sh" };
    let def = |id: &str, label: &str, bin: &str| AgentDef { id: id.into(), label: label.into(), bin: bin.into(), launch: vec![], yolo: vec!["--yes".into()], resume_latest: vec![], prompt: String::new(), acp: vec![] };
    agents::set_custom(&[def("phone-fake", "Fake Agent", bin), def("phone-absent", "Absent Agent", "kelpie-no-such-program-anywhere")]).unwrap();
    held
}

fn folder(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kelpie-phone-start-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn catalogue(path: &str) -> Value {
    json!({
        "projects": [{"id": "p1", "name": "Shop", "path": path}, {"id": "gone", "name": "Gone", "path": format!("{path}-missing")}],
        "agents": [
            {"kind": "phone-fake", "label": "Fake Agent", "args": ["--yes", "--model", "m1"], "env": {"FAKE_AGENT_KEY": "sk-phone-secret"}},
            {"kind": "phone-absent", "label": "Absent Agent"},
        ],
    })
}

fn set_launch(d: &Arc<Daemon>, params: Value) -> Result<Option<Value>> {
    let (tx, _rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), "mobile.set_launch", params)
}

fn start(project: &str, kind: &str, cid: &str) -> Value {
    json!({"type": "new_session", "project_id": project, "kind": kind, "prompt": "echo phone-started", "cid": cid})
}

#[tokio::test]
async fn a_phone_sees_what_it_may_start_only_once_the_app_has_said_and_never_a_folder_or_an_environment() {
    let _plugins = fake_agents();
    let mut r = rig();
    let none = r.ask_write(json!({"type": "new_options"})).await;
    assert_eq!((none["ready"].as_bool(), none["why"].as_str()), (Some(false), Some(launch::NOT_READY)), "{none}");
    let path = folder("options");
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    let o = r.ask_write(json!({"type": "new_options"})).await;
    assert_eq!(o["ready"], true);
    assert_eq!(o["projects"], json!([{"id": "p1", "name": "Shop"}, {"id": "gone", "name": "Gone"}]));
    assert_eq!(o["agents"], json!([{"kind": "phone-fake", "label": "Fake Agent", "skips": true}]), "the absent agent is not installed: {o}");
    assert!(!o.to_string().contains(&path) && !o.to_string().contains("sk-phone-secret"));
    // The whole catalogue each time: one sent later replaces it.
    set_launch(&r.daemon, json!({"projects": [{"id": "p9", "name": "Other", "path": path}], "agents": []})).unwrap();
    let o = r.ask_write(json!({"type": "new_options"})).await;
    assert_eq!((o["projects"].clone(), o["agents"].clone()), (json!([{"id": "p9", "name": "Other"}]), json!([])));
    // A device that may only read is told so, as for any typing.
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "new_options"})).await), Some("read_only"));
    // Asking over and over is held back, a few times a second at most.
    r.daemon.mobile.options_rate.lock().clear();
    for _ in 0..OPTIONS_BURST as usize {
        assert!(r.daemon.mobile.allow_options("phone"));
    }
    assert_eq!(code(&r.ask_write(json!({"type": "new_options"})).await), Some("rate_limited"));
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_refused_catalogue_clears_the_one_before_it_so_nothing_starts_with_settings_since_changed() {
    let _plugins = fake_agents();
    let mut r = rig();
    let path = folder("refused-catalogue");
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    assert_eq!(r.ask_write(json!({"type": "new_options"})).await["ready"], true);
    // The app turned bypass off, and the same push carries a project the daemon refuses: the old catalogue, bypass and all, is gone.
    let refused = json!({"projects": [{"id": "p1", "name": "a\u{7}b", "path": path}], "agents": [{"kind": "phone-fake", "label": "Fake Agent"}]});
    let why = set_launch(&r.daemon, refused).unwrap_err().to_string();
    assert!(why.contains("control characters"), "the app is told why: {why}");
    let before = r.daemon.list().len();
    for req in [json!({"type": "new_options"}), start("p1", "phone-fake", "after-refusal")] {
        let reply = r.ask_write(req.clone()).await;
        assert_eq!(code(&reply), Some("launch_refused"), "{req}: {reply}");
        assert_eq!(reply["message"], launch::REFUSED);
    }
    assert_eq!(r.daemon.list().len(), before);
    // The next one accepted is used again.
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    assert_eq!(r.ask_write(json!({"type": "new_options"})).await["ready"], true);
    let big = json!({"projects": [], "agents": [{"kind": "phone-fake", "label": "F", "env": {"K": "x".repeat(launch::MAX_MESSAGE)}}]});
    assert!(set_launch(&r.daemon, big).unwrap_err().to_string().contains("too large"));
    assert_eq!(code(&r.ask_write(json!({"type": "new_options"})).await), Some("launch_refused"));
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn every_way_a_start_is_refused_starts_nothing() {
    let _plugins = fake_agents();
    let mut r = rig();
    let before = r.daemon.list().len();
    assert_eq!(code(&r.ask_write(start("p1", "phone-fake", "c0")).await), Some("not_ready"), "no catalogue yet");
    let path = folder("refused");
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    let long = json!({"type": "new_session", "project_id": "p1", "kind": "phone-fake", "prompt": "x".repeat(crate::subs::MAX_TASK + 1), "cid": "c7"});
    for (req, expected) in [
        (json!({"type": "new_session", "project_id": "p1", "kind": "phone-fake"}), "bad_request"),
        (start("ssh-project", "phone-fake", "c1"), "no_project"),
        (start("p1", "claude", "c2"), "no_agent"),
        (start("p1", "phone-absent", "c3"), "not_installed"),
        (start("gone", "phone-fake", "c4"), "no_folder"),
        (long, "too_long"),
        (json!({"type": "new_session", "project_id": "p1", "kind": "phone-fake", "cid": "c5", "epoch": "an older daemon"}), "epoch"),
    ] {
        let reply = r.ask_write(req.clone()).await;
        assert_eq!(code(&reply), Some(expected), "{req}: {reply}");
    }
    assert_eq!(code(&r.ask(Permission::Read, start("p1", "phone-fake", "c6")).await), Some("read_only"));
    r.daemon.mobile.load.set_for_test(Level::L3);
    assert_eq!(code(&r.ask_write(start("p1", "phone-fake", "c8")).await), Some("busy"));
    r.daemon.mobile.load.set_for_test(Level::L0);
    // Three at once, then one every ten seconds (counted afresh: the refusals on disk above spent two).
    r.daemon.mobile.start_rate.lock().clear();
    for _ in 0..3 {
        assert!(r.daemon.mobile.allow_start("phone"));
    }
    assert_eq!(code(&r.ask_write(start("p1", "phone-fake", "c9")).await), Some("rate_limited"));
    // A start the look on disk refuses has spent its token: that look is what the rate holds back, so asking for a folder that is gone
    // over and over is held back like any start.
    r.daemon.mobile.start_rate.lock().clear();
    for cid in ["r1", "r2", "r3"] {
        assert_eq!(code(&r.ask_write(start("gone", "phone-fake", cid)).await), Some("no_folder"));
    }
    assert_eq!(code(&r.ask_write(start("gone", "phone-fake", "r4")).await), Some("rate_limited"));
    r.daemon.mobile.start_rate.lock().clear();
    for cid in ["i1", "i2", "i3"] {
        assert_eq!(code(&r.ask_write(start("p1", "phone-absent", cid)).await), Some("not_installed"));
    }
    assert!(!r.daemon.mobile.allow_start("phone"), "so did an agent that is not installed");
    assert_eq!(r.daemon.list().len(), before, "nothing was started");
    assert!(!r.daemon.mobile.audit_entries(50).to_string().contains("session_new"));
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_start_takes_the_catalogues_arguments_and_environment_is_done_once_and_is_said_without_the_prompt() {
    let _plugins = fake_agents();
    let mut r = rig();
    let (tx, mut events) = crate::session::client_channel();
    r.daemon.register_client(9, tx);
    r.daemon.set_client_role(9, &crate::auth::Role::App);
    let path = folder("started");
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    let reply = r.ask_write(start("p1", "phone-fake", "same")).await;
    assert_eq!(reply["result"], "ok", "{reply}");
    let id = reply["id"].as_str().unwrap().to_string();
    let info = r.daemon.list().into_iter().find(|i| i.spec.id == id).expect("started");
    let spec = info.spec.clone();
    assert_eq!((spec.kind.as_str(), spec.project_id.as_deref(), spec.args.clone()), ("phone-fake", Some("p1"), vec!["--yes".to_string(), "--model".into(), "m1".into()]));
    assert_eq!(spec.env, std::collections::BTreeMap::from([("FAKE_AGENT_KEY".to_string(), "sk-phone-secret".to_string())]));
    assert_eq!(spec.cwd, path, "the project's folder as the app names it, as the desktop starts in it");
    // A retry of the same start (a lost reply) gets the same session, and no second one.
    let sessions = r.daemon.list().len();
    let again = r.ask_write(start("p1", "phone-fake", "same")).await;
    assert_eq!((again["id"].as_str(), again["dup"].as_bool()), (Some(id.as_str()), Some(true)), "{again}");
    assert_eq!(r.daemon.list().len(), sessions);
    // The app hears which phone started what; the audit log says which agent in which project, never the prompt.
    let mut told = Vec::new();
    while let Ok(o) = events.try_recv() {
        let v: Value = serde_json::from_slice(&o.data[5..]).unwrap_or_default();
        if v["event"] == "mobile.session_started" {
            told.push(v["params"].clone());
        }
    }
    assert_eq!(told, [json!({"id": id, "device": "phone", "name": "Phone", "kind": "phone-fake", "project_id": "p1"})]);
    let audit = r.daemon.mobile.audit_entries(50);
    let notes: Vec<(String, String)> = audit["entries"].as_array().unwrap().iter().filter(|e| e["action"] == "session_new").map(|e| (e["device"].as_str().unwrap_or_default().to_string(), e["summary"].as_str().unwrap_or_default().to_string())).collect();
    assert_eq!(notes, [("phone".to_string(), format!("Fake Agent in Shop (session {id})"))], "{audit}");
    assert!(!audit.to_string().contains("phone-started") && !audit.to_string().contains("sk-phone-secret"));
    let _ = r.daemon.close(info.handle);
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_start_sent_again_while_the_first_still_runs_waits_for_it_and_never_starts_a_second() {
    let _plugins = fake_agents();
    let mut r = rig();
    let path = folder("in-flight");
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    // The same start over two connections of the phone at once (a reconnect re-sent it while the computer was still starting it).
    let mut second = other_conn(&r, "phone", "Phone", 7);
    let mut req = start("p1", "phone-fake", "twice");
    req["req"] = json!(1);
    let (a, b) = tokio::join!(r.conn.handle(Permission::Write, &req), second.handle(Permission::Write, &req));
    let (a, mut b) = (a.expect("a reply"), b.expect("a reply"));
    assert_eq!(a["result"], "ok", "{a}");
    // The second waited for the first and got its reply; or, on a computer slow enough that the first took longer than the wait, it was
    // told the start is still under way, and asked again (as the phone does) it gets the first's reply.
    if code(&b) == Some("in_progress") {
        b = second.handle(Permission::Write, &req).await.expect("a reply");
    }
    assert_eq!(a["id"], b["id"], "{a} {b}");
    assert_eq!((a["dup"].as_bool(), b["dup"].as_bool()), (None, Some(true)), "the second got the first's reply: {a} {b}");
    let starts = |r: &Rig| r.daemon.mobile.audit_entries(50)["entries"].as_array().unwrap().iter().filter(|e| e["action"] == "session_new").count();
    assert_eq!(starts(&r), 1, "one session was started");
    assert!(r.daemon.mobile.in_flight.lock().is_empty(), "nothing is left reserved");
    let id = a["id"].as_str().unwrap().to_string();
    if let Some(info) = r.daemon.list().into_iter().find(|i| i.spec.id == id) {
        let _ = r.daemon.close(info.handle);
    }

    // A first that takes longer than a repeat may wait: the repeat is told it is still under way, starts nothing, and is not kept,
    // so the same cid asked again gets the first one's reply once it is there.
    let first = CidTurn::take(&r.daemon.mobile.in_flight, "phone", "slow", CID_WAIT).await.unwrap();
    let busy = r.ask_write(start("p1", "phone-fake", "slow")).await;
    assert_eq!(code(&busy), Some("in_progress"), "{busy}");
    r.daemon.mobile.remember_reply("phone", "slow", ANSWER_DEDUPE, &json!({"type": "ack", "result": "ok", "id": "the-first"}));
    drop(first);
    let again = r.ask_write(start("p1", "phone-fake", "slow")).await;
    assert_eq!((again["id"].as_str(), again["dup"].as_bool()), (Some("the-first"), Some(true)), "{again}");
    assert_eq!(starts(&r), 1, "neither started one");
    assert!(r.daemon.mobile.in_flight.lock().is_empty());
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn whether_a_phone_may_still_start_is_read_again_after_the_look_on_disk() {
    let mut r = rig();
    assert!(r.conn.may_still_start().is_ok());
    r.daemon.mobile.set_write("phone", false).unwrap();
    assert_eq!(r.conn.may_still_start().err().map(|e| e.code), Some("read_only"), "downgraded meanwhile");
    r.daemon.mobile.set_write("phone", true).unwrap();
    // A connection whose phone's unlocked window is over (here: there never was one).
    r.conn.lock_path = LockPath::PasskeyAlways;
    assert_eq!(r.conn.may_still_start().err().map(|e| e.code), Some("locked"), "locked meanwhile");
    r.conn.lock_path = LockPath::Local;
    assert!(r.conn.may_still_start().is_ok());
    // Phone access turned off is said as that, not as a pairing gone: the pairing stands.
    r.daemon.mobile.config.lock().enabled = false;
    assert_eq!(r.conn.may_still_start().err().map(|e| e.code), Some("mobile_off"), "turned off meanwhile");
    r.daemon.mobile.config.lock().enabled = true;
    assert!(r.conn.may_still_start().is_ok());
    r.daemon.mobile.revoke("phone").unwrap();
    assert_eq!(r.conn.may_still_start().err().map(|e| e.code), Some("unpaired"), "revoked meanwhile");
}

#[tokio::test]
async fn a_start_goes_by_the_catalogue_as_it_is_after_the_look_on_disk() {
    let _plugins = fake_agents();
    let r = rig();
    let path = folder("changed");
    set_launch(&r.daemon, catalogue(&path)).unwrap();
    let req = start("p1", "phone-fake", "c");
    let planned = r.conn.still_planned(&req, &path).ok().expect("unchanged: planned as before");
    assert_eq!(planned.agent.args, ["--yes", "--model", "m1"]);
    // Bypass turned off while the disk was looked at: the start takes the arguments and environment that hold now.
    let calmer = json!({"projects": [{"id": "p1", "name": "Shop", "path": path}], "agents": [{"kind": "phone-fake", "label": "Fake Agent", "args": ["--model", "m2"]}]});
    set_launch(&r.daemon, calmer).unwrap();
    let planned = r.conn.still_planned(&req, &path).ok().expect("still offered");
    assert_eq!((planned.agent.args.clone(), planned.agent.env.is_empty()), (vec!["--model".to_string(), "m2".into()], true));
    assert_eq!(planned.create_params()["args"], json!(["--model", "m2"]));
    // Gone from the catalogue, moved to another folder than the one looked at, or every start shut off by a refused catalogue: refused.
    let refused_now = |r: &Rig| r.conn.still_planned(&req, &path).err().map(|e| (e.code, e.message));
    let changed = Some(("launch_changed", "Your computer's settings changed; try again.".to_string()));
    set_launch(&r.daemon, json!({"projects": [{"id": "p1", "name": "Shop", "path": path}], "agents": []})).unwrap();
    assert_eq!(refused_now(&r), changed, "the agent was hidden");
    set_launch(&r.daemon, json!({"projects": [], "agents": [{"kind": "phone-fake", "label": "Fake Agent"}]})).unwrap();
    assert_eq!(refused_now(&r), changed, "the project was removed");
    set_launch(&r.daemon, json!({"projects": [{"id": "p1", "name": "Shop", "path": format!("{path}-elsewhere")}], "agents": [{"kind": "phone-fake", "label": "Fake Agent"}]})).unwrap();
    assert_eq!(refused_now(&r), changed, "the project's folder changed");
    let _ = set_launch(&r.daemon, json!({"projects": [{"id": "p1", "name": "a\u{7}b", "path": path}], "agents": []}));
    assert_eq!(refused_now(&r), changed, "the latest catalogue was refused");
    let _ = std::fs::remove_dir_all(path);
}

/// What the daemon's log was given while `f` ran on this thread.
fn logged(f: impl FnOnce()) -> String {
    use std::io::Write;
    use tracing_subscriber::util::SubscriberInitExt;
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);
    impl Write for Captured {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let out = Captured::default();
    let writer = out.clone();
    let filter = crate::log_filter(tracing_subscriber::EnvFilter::new("trace"));
    let subscriber = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(move || writer.clone()).finish();
    {
        let _guard = subscriber.set_default();
        tracing::info!("capture on");
        f();
    }
    let log = String::from_utf8_lossy(&out.0.lock()).to_string();
    log
}

#[test]
fn the_catalogues_environment_never_reaches_the_log() {
    let _plugins = fake_agents();
    let r = rig();
    let path = folder("log");
    let log = logged(|| {
        set_launch(&r.daemon, catalogue(&path)).unwrap();
        // Refused, with the environment in it, and then sent whole.
        let _ = set_launch(&r.daemon, json!({"projects": [], "agents": [{"kind": "phone-fake", "label": "F", "env": {"K": "sk-phone-secret"}, "args": 3}]}));
        set_launch(&r.daemon, catalogue(&path)).unwrap();
        let _ = r.daemon.mobile.launch_options(&["phone-fake"]);
        let planned = launch::plan(r.daemon.mobile.launch.lock().as_ref().map_err(|m| *m), &start("p1", "phone-fake", "c")).ok().unwrap();
        let info = r.daemon.create_for_phone(planned.create_params(), "phone", "Phone", |id| planned.summary(id)).unwrap();
        let _ = r.daemon.close(info.handle);
    });
    assert!(log.contains("capture on"), "the capture sees the log: {log}");
    assert!(!log.contains("sk-phone-secret"), "{log}");
    let _ = std::fs::remove_dir_all(path);
}
