use super::*;
use crate::auth::Role;
use crate::session::fake;

fn daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
}

fn add(d: &Daemon, handle: u32, id: &str, restricted: bool) -> Arc<Session> {
    let (session, _typed) = fake::running(handle, id, "claude");
    {
        let mut spec = session.spec.lock();
        spec.name = Some(format!("{id}-name"));
        spec.restricted = restricted;
    }
    d.insert_for_test(Entry::Term(session.clone()));
    session
}

fn call(d: &Arc<Daemon>, method: &str, params: Value) -> Result<Value> {
    let (tx, _rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), method, params).map(|v| v.expect("answered at once"))
}

fn body(o: &crate::session::Outgoing) -> Value {
    serde_json::from_slice(&o.data[5..]).unwrap()
}

/// The audit file's lines, once there are `n` of them (they are written off the event loop).
async fn audit_lines(dir: &std::path::Path, n: usize) -> Vec<Value> {
    for _ in 0..100 {
        let lines: Vec<Value> = std::fs::read_to_string(dir.join(crate::browser_agent::AUDIT_FILE)).unwrap_or_default().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        if lines.len() >= n {
            return lines;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no {n} audit lines");
}

#[test]
fn the_browser_policy_is_off_by_default_saved_with_unknown_keys_kept_and_checked_as_a_whole() {
    let (d, dir) = daemon("browser-policy");
    let p = call(&d, "daemon.policy", json!({})).unwrap();
    assert_eq!((p["browser_agents"].clone(), p["browser_eval"].clone(), p["browser_sites"].clone()), (json!("off"), json!(false), json!([])));
    assert_eq!(call(&d, "browser.policy", json!({})).unwrap(), json!({"mode": "off", "eval": false, "sub_sessions": "ask"}));
    std::fs::write(dir.join(POLICY_FILE), r#"{"sub_sessions": "allow", "from_the_future": true}"#).unwrap();
    let d = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    let set = call(&d, "daemon.set_policy", json!({"browser_agents": "act", "browser_sites": ["http://LOCALHOST:3000/"]})).unwrap();
    assert_eq!((set["browser_agents"].clone(), set["browser_sites"].clone(), set["sub_sessions"].clone()), (json!("act"), json!(["http://localhost:3000"]), json!("allow")));
    let file: Value = serde_json::from_slice(&std::fs::read(dir.join(POLICY_FILE)).unwrap()).unwrap();
    assert_eq!((file["from_the_future"].clone(), file["browser_agents"].clone(), file["browser_eval"].clone()), (json!(true), json!("act"), json!(false)));
    // A bad value anywhere in the request changes nothing, the other settings in it included.
    for bad in [json!({"browser_agents": "all", "sub_sessions": "off"}), json!({"browser_sites": ["ftp://x"], "agents_control_others": false}), json!({"browser_eval": 1}), json!({"sub_sessions": "maybe", "browser_agents": "read"})] {
        assert!(call(&d, "daemon.set_policy", bad.clone()).is_err(), "{bad}");
        let p = call(&d, "daemon.policy", json!({})).unwrap();
        assert_eq!((p["browser_agents"].clone(), p["sub_sessions"].clone(), p["agents_control_others"].clone()), (json!("act"), json!("allow"), json!(true)), "{bad}");
    }
    // It holds over a restart.
    let again = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    call(&again, "daemon.set_policy", json!({"browser_eval": true})).unwrap();
    assert_eq!(call(&again, "browser.policy", json!({})).unwrap(), json!({"mode": "act", "eval": true, "sub_sessions": "allow"}));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_browser_call_speaks_for_the_calling_session_and_the_audit_is_the_users() {
    let (d, dir) = daemon("browser-auth");
    add(&d, 1, "r", true);
    add(&d, 2, "c", false);
    for me in ["r", "c"] {
        let mut forged = json!({"session_id": "someone-else", "tool": "tabs", "args": {}});
        crate::auth::authorize(&d, &Role::Session(me.into()), "browser.call", &mut forged).unwrap();
        assert_eq!(forged["session_id"], me);
        crate::auth::authorize(&d, &Role::Session(me.into()), "browser.policy", &mut json!({})).unwrap();
        assert!(crate::auth::authorize(&d, &Role::Session(me.into()), "browser.audit", &mut json!({})).is_err());
    }
    // With scripting other sessions turned off, still the caller's own panes.
    call(&d, "daemon.set_policy", json!({"agents_control_others": false})).unwrap();
    let mut forged = json!({"session_id": "r"});
    crate::auth::authorize(&d, &Role::Session("c".into()), "browser.call", &mut forged).unwrap();
    assert_eq!(forged["session_id"], "c");
    assert!(crate::auth::authorize(&d, &Role::Claimed("c".into()), "browser.call", &mut json!({})).is_err());
    // A session's params are an object.
    assert!(crate::auth::authorize(&d, &Role::Session("c".into()), "browser.call", &mut json!(["tabs"])).is_err());
    // The user's own connection is no session: it has nobody to speak for.
    let err = call(&d, "browser.call", json!({"tool": "tabs"})).unwrap_err().to_string();
    assert!(err.contains("programs running in a Kelpie session"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn agents_get_kelpies_tools_while_sub_sessions_or_browser_control_are_on() {
    let (d, dir) = daemon("browser-injection");
    for (subs, browser, on) in [("ask", "off", true), ("off", "read", true), ("off", "act", true), ("allow", "act", true), ("off", "off", false)] {
        call(&d, "daemon.set_policy", json!({"sub_sessions": subs, "browser_agents": browser})).unwrap();
        assert_eq!(d.agent_tools_on(), on, "{subs} {browser}");
        assert_eq!(d.sub_tools_on(), subs != "off", "{subs} {browser}");
        // The MCP server is told which groups to list.
        let p = call(&d, "browser.policy", json!({})).unwrap();
        assert_eq!((p["sub_sessions"].as_str(), p["mode"].as_str()), (Some(subs), Some(browser)));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_refused_call_reaches_no_window_and_is_not_audited() {
    let (d, dir) = daemon("browser-gate");
    add(&d, 1, "s1", false);
    let (app_tx, mut app_rx) = crate::session::client_channel();
    d.providers.register(9, &Role::App, "browser", None, &app_tx).unwrap();
    let ask = |tool: &str, args: Value| call(&d, "browser.call", json!({"session_id": "s1", "tool": tool, "args": args}));
    // Off by default: every tool, nothing asked of the app.
    assert_eq!(ask("tabs", json!({})).unwrap_err().to_string(), crate::browser_agent::OFF);
    assert_eq!(ask("type", json!({"ref": "e1", "text": "pa55word-in-flight"})).unwrap_err().to_string(), crate::browser_agent::OFF);
    // Read only: acting is refused naming the mode; eval needs act and its own switch.
    call(&d, "daemon.set_policy", json!({"browser_agents": "read"})).unwrap();
    assert!(ask("click", json!({"ref": "e1"})).unwrap_err().to_string().contains("read only"));
    call(&d, "daemon.set_policy", json!({"browser_agents": "act"})).unwrap();
    assert_eq!(ask("eval", json!({"expression": "1"})).unwrap_err().to_string(), crate::browser_agent::EVAL_OFF);
    // Bad arguments and an unknown tool.
    assert!(ask("click", json!({"ref": "e1", "text": "x"})).unwrap_err().to_string().contains("takes no `text`"));
    assert!(ask("drag", json!({})).unwrap_err().to_string().contains("unknown browser tool"));
    assert!(app_rx.try_recv().is_err(), "the window was asked nothing");
    // Nor is the over-rate refusal: a loop of refused calls writes nothing, so it cannot push what was done out of the audit.
    for _ in 0..200 {
        assert!(ask("click", json!({})).is_err(), "a bad call, or one over the rate");
    }
    assert_eq!(ask("click", json!({"ref": "e1"})).unwrap_err().to_string(), crate::browser_agent::TOO_MANY);
    assert!(app_rx.try_recv().is_err(), "nothing went to the window");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!dir.join(crate::browser_agent::AUDIT_FILE).exists(), "no refusal is audited");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_call_goes_to_the_window_and_comes_back_with_the_audit_written_first() {
    let (d, dir) = daemon("browser-call");
    add(&d, 1, "s1", false);
    let (app_tx, mut app_rx) = crate::session::client_channel();
    d.providers.register(9, &Role::App, "browser", None, &app_tx).unwrap();
    call(&d, "daemon.set_policy", json!({"browser_agents": "act", "browser_sites": ["http://localhost:3000"]})).unwrap();
    let password = "correct-horse-battery-staple";
    let (tx, mut rx) = crate::session::client_channel();
    let asked = d.dispatch(1, &tx, &json!(5), "browser.call", json!({"session_id": "s1", "tool": "type", "args": {"ref": "e3", "text": password, "submit": true}})).unwrap();
    assert!(asked.is_none(), "answered later");
    let req = body(&app_rx.recv().await.unwrap());
    assert_eq!(req["method"], "browser.agent");
    let p = &req["params"];
    assert_eq!(p["session"], json!({"id": "s1", "name": "s1-name", "kind": "claude", "cwd": ".", "parent": null, "restricted": false}));
    assert_eq!((p["tool"].as_str(), p["args"]["text"].as_str(), p["args"]["clear"].as_bool()), (Some("type"), Some(password), Some(true)));
    assert_eq!(p["policy"], json!({"mode": "act", "eval": false, "sites": ["http://localhost:3000"]}));
    let reply = json!({"text": "Typed 28 characters into textbox \"Password\" [e3].", "page": {"title": "Sign in", "url": "http://localhost:3000/login"}, "audit": {"host": "localhost", "target": "textbox \"Password\""}});
    d.providers.reply(9, &json!({"preq": req["preq"], "result": reply.clone()}));
    let answer = body(&tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap());
    assert_eq!((answer["id"].clone(), answer["result"].clone()), (json!(5), reply));
    // Already on record when the answer came.
    let line = &audit_lines(&dir, 1).await[0];
    assert_eq!((line["tool"].as_str(), line["host"].as_str(), line["target"].as_str(), line["ok"].as_bool()), (Some("type"), Some("localhost"), Some("textbox \"Password\""), Some(true)));
    // The typed text is in no file of the data folder.
    for entry in std::fs::read_dir(&dir).unwrap().flatten() {
        if entry.path().is_file() {
            assert!(!String::from_utf8_lossy(&std::fs::read(entry.path()).unwrap()).contains(password), "{:?}", entry.path());
        }
    }
    // The user reads the audit back; a read tool's call is not in it.
    let read = d.dispatch(1, &tx, &json!(6), "browser.call", json!({"session_id": "s1", "tool": "snapshot", "args": {}})).unwrap();
    assert!(read.is_none());
    let req = body(&app_rx.recv().await.unwrap());
    d.providers.reply(9, &json!({"preq": req["preq"], "error": {"message": "the Kelpie app is not open on the computer"}}));
    let failed = body(&tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap());
    assert_eq!(failed["error"]["message"], crate::browser_agent::NOT_OPEN);
    assert!(d.dispatch(1, &tx, &json!(7), "browser.audit", json!({"limit": 9999})).unwrap().is_none());
    let listed = body(&tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap());
    let entries = listed["result"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["tool"], "type");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn the_windows_refusal_reaches_neither_agent_nor_audit_with_a_pages_secret() {
    let (d, dir) = daemon("browser-refused");
    add(&d, 1, "r", true);
    let (app_tx, mut app_rx) = crate::session::client_channel();
    d.providers.register(9, &Role::App, "browser", None, &app_tx).unwrap();
    call(&d, "daemon.set_policy", json!({"browser_agents": "act"})).unwrap();
    let (tx, mut rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), "browser.call", json!({"session_id": "r", "tool": "navigate", "args": {"url": "https://bank.test/"}})).unwrap();
    let req = body(&app_rx.recv().await.unwrap());
    assert_eq!(req["params"]["session"]["restricted"], true, "a restricted session says so");
    let refusal = "Navigation to https://bank.test/cb?code=S3CRET-CODE is not allowed for this pane";
    d.providers.reply(9, &json!({"preq": req["preq"], "error": {"message": refusal}}));
    let answer = body(&tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap());
    assert_eq!(answer["error"]["message"], "Navigation to https://bank.test/cb is not allowed for this pane");
    let line = &audit_lines(&dir, 1).await[0];
    assert_eq!((line["ok"].as_bool(), line["error"].as_str()), (Some(false), Some("Navigation to https://bank.test/cb is not allowed for this pane")));
    assert!(!std::fs::read_to_string(dir.join(crate::browser_agent::AUDIT_FILE)).unwrap().contains("S3CRET"));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn with_no_window_an_act_call_fails_at_once_and_is_audited() {
    let (d, dir) = daemon("browser-no-app");
    add(&d, 1, "s1", false);
    call(&d, "daemon.set_policy", json!({"browser_agents": "act"})).unwrap();
    let (tx, mut rx) = crate::session::client_channel();
    let started = Instant::now();
    d.dispatch(1, &tx, &json!(1), "browser.call", json!({"session_id": "s1", "tool": "navigate", "args": {"url": "https://example.test/x?token=abc"}})).unwrap();
    let answer = body(&tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap());
    assert_eq!(answer["error"]["message"], crate::browser_agent::NOT_OPEN);
    assert!(started.elapsed() < Duration::from_secs(1));
    let line = &audit_lines(&dir, 1).await[0];
    assert_eq!((line["host"].as_str(), line["ok"].as_bool()), (Some("example.test"), Some(false)));
    assert!(!line.to_string().contains("token"));
    let _ = std::fs::remove_dir_all(dir);
}
