use super::*;
use crate::auth::Role;
use crate::session::fake;

fn daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
}

fn add(d: &Daemon, handle: u32, id: &str, restricted: bool) {
    let (session, _typed) = fake::running(handle, id, "claude");
    session.spec.lock().restricted = restricted;
    d.insert_for_test(Entry::Term(session));
}

fn call(d: &Arc<Daemon>, method: &str, params: Value) -> Result<Value> {
    let (tx, _rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), method, params).map(|v| v.expect("answered at once"))
}

#[test]
fn the_window_policy_is_off_by_default_saved_with_other_keys_kept_and_checked_as_a_whole() {
    let (d, dir) = daemon("window-policy");
    let p = call(&d, "daemon.policy", json!({})).unwrap();
    assert_eq!((p["window_agents"].clone(), p["window_visible"].clone()), (json!("off"), json!(false)));
    assert_eq!(call(&d, "window.policy", json!({})).unwrap(), json!({"mode": "off", "visible": false}));
    std::fs::write(dir.join(POLICY_FILE), r#"{"sub_sessions": "allow", "from_the_future": true}"#).unwrap();
    let d = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    let set = call(&d, "daemon.set_policy", json!({"window_agents": "on", "window_visible": true})).unwrap();
    assert_eq!((set["window_agents"].clone(), set["window_visible"].clone(), set["sub_sessions"].clone()), (json!("on"), json!(true), json!("allow")));
    let file: Value = serde_json::from_slice(&std::fs::read(dir.join(POLICY_FILE)).unwrap()).unwrap();
    assert_eq!((file["from_the_future"].clone(), file["window_agents"].clone(), file["window_visible"].clone()), (json!(true), json!("on"), json!(true)));
    // The tools are offered only where the helper runs.
    let mode = call(&d, "window.policy", json!({})).unwrap()["mode"].clone();
    assert_eq!(mode, if cfg!(windows) { "on" } else { "off" });
    // A bad value anywhere in the request changes nothing, the other settings in it included.
    for bad in [json!({"window_agents": "ask", "sub_sessions": "off"}), json!({"window_visible": "yes", "agents_control_others": false}), json!({"window_agents": 1})] {
        assert!(call(&d, "daemon.set_policy", bad.clone()).is_err(), "{bad}");
        let p = call(&d, "daemon.policy", json!({})).unwrap();
        assert_eq!((p["window_agents"].clone(), p["sub_sessions"].clone(), p["agents_control_others"].clone()), (json!("on"), json!("allow"), json!(true)), "{bad}");
    }
    // It holds over a restart, and turning the browser tools on or off leaves it alone.
    let again = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    call(&again, "daemon.set_policy", json!({"browser_agents": "read"})).unwrap();
    let p = call(&again, "daemon.policy", json!({})).unwrap();
    assert_eq!((p["window_agents"].clone(), p["window_visible"].clone(), p["browser_agents"].clone()), (json!("on"), json!(true), json!("read")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_restricted_session_has_no_window_tools_and_a_window_call_speaks_for_its_own_session() {
    let (d, dir) = daemon("window-auth");
    add(&d, 1, "r", true);
    add(&d, 2, "c", false);
    // The restricted one is refused by the method list, and the daemon refuses it again whatever the list says.
    for m in ["window.call", "window.policy"] {
        assert!(crate::auth::authorize(&d, &Role::Session("r".into()), m, &mut json!({})).is_err(), "{m}");
    }
    let err = call(&d, "window.call", json!({"session_id": "r", "tool": "list"})).unwrap_err().to_string();
    assert!(err.contains("restricted session"), "{err}");
    // A session that may script others is offered both, and its own id replaces any other.
    let mut forged = json!({"session_id": "r", "tool": "list", "args": {}});
    crate::auth::authorize(&d, &Role::Session("c".into()), "window.call", &mut forged).unwrap();
    assert_eq!(forged["session_id"], "c");
    crate::auth::authorize(&d, &Role::Session("c".into()), "window.policy", &mut json!({})).unwrap();
    // The audit is the user's.
    assert!(crate::auth::authorize(&d, &Role::Session("c".into()), "window.audit", &mut json!({})).is_err());
    // With scripting other sessions turned off, a session has the caller's own session only: no window tools.
    call(&d, "daemon.set_policy", json!({"agents_control_others": false})).unwrap();
    assert!(crate::auth::authorize(&d, &Role::Session("c".into()), "window.call", &mut json!({})).is_err());
    assert!(crate::auth::authorize(&d, &Role::Claimed("c".into()), "window.call", &mut json!({})).is_err());
    // The user's own connection is no session: it has nobody to speak for.
    let err = call(&d, "window.call", json!({"tool": "list"})).unwrap_err().to_string();
    assert!(err.contains("programs running in a Kelpie session"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn agents_get_kelpies_tools_while_window_control_is_on_too() {
    let (d, dir) = daemon("window-injection");
    for (subs, windows, on) in [("off", "off", false), ("off", "on", true), ("ask", "off", true)] {
        call(&d, "daemon.set_policy", json!({"sub_sessions": subs, "browser_agents": "off", "window_agents": windows})).unwrap();
        assert_eq!(d.agent_tools_on(), on, "{subs} {windows}");
    }
    let _ = std::fs::remove_dir_all(dir);
}
