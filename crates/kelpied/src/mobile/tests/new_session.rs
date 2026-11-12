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
    // The whole catalogue each time: one sent later replaces it, and one refused leaves the last.
    set_launch(&r.daemon, json!({"projects": [{"id": "p9", "name": "Other", "path": path}], "agents": []})).unwrap();
    let o = r.ask_write(json!({"type": "new_options"})).await;
    assert_eq!((o["projects"].clone(), o["agents"].clone()), (json!([{"id": "p9", "name": "Other"}]), json!([])));
    let big = json!({"projects": [], "agents": [{"kind": "phone-fake", "label": "F", "env": {"K": "x".repeat(launch::MAX_MESSAGE)}}]});
    assert!(set_launch(&r.daemon, big).unwrap_err().to_string().contains("too large"));
    assert_eq!(r.ask_write(json!({"type": "new_options"})).await["projects"], json!([{"id": "p9", "name": "Other"}]));
    // A device that may only read is told so, as for any typing.
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "new_options"})).await), Some("read_only"));
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
    // Three at once, then one every ten seconds.
    for _ in 0..3 {
        assert!(r.daemon.mobile.allow_start("phone"));
    }
    assert_eq!(code(&r.ask_write(start("p1", "phone-fake", "c9")).await), Some("rate_limited"));
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
    assert_eq!(spec.cwd, crate::daemon::existing_folder(&path, &path).unwrap());
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
    assert_eq!(notes, [("phone".to_string(), "Fake Agent in Shop".to_string())], "{audit}");
    assert!(!audit.to_string().contains("phone-started") && !audit.to_string().contains("sk-phone-secret"));
    let _ = r.daemon.close(info.handle);
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
        let planned = launch::plan(r.daemon.mobile.launch.lock().as_ref(), &start("p1", "phone-fake", "c")).ok().unwrap();
        let info = r.daemon.create_for_phone(planned.create_params(&path), "phone", "Phone").unwrap();
        let _ = r.daemon.close(info.handle);
    });
    assert!(log.contains("capture on"), "the capture sees the log: {log}");
    assert!(!log.contains("sk-phone-secret"), "{log}");
    let _ = std::fs::remove_dir_all(path);
}
