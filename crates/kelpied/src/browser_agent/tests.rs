use super::*;
use crate::auth::Role;

fn policy(mode: Mode, eval: bool) -> Policy {
    Policy { mode, eval, sites: Vec::new() }
}

#[test]
fn every_tool_has_a_class_and_nothing_else_does() {
    let classes: Vec<Class> = TOOLS.iter().map(|t| classify(t).unwrap()).collect();
    assert_eq!(classes.iter().filter(|c| **c == Class::Read).count(), 6);
    assert_eq!(classes.iter().filter(|c| **c == Class::Act).count(), 4);
    assert_eq!(classify("eval"), Some(Class::Eval));
    for t in ["click", "type", "press", "navigate"] {
        assert_eq!(classify(t), Some(Class::Act), "{t}");
    }
    for t in ["", "kelpie_browser_tabs", "Tabs", "close"] {
        assert_eq!(classify(t), None, "{t}");
    }
}

#[test]
fn the_gate_follows_the_policy() {
    for t in TOOLS {
        assert_eq!(gate(&policy(Mode::Off, true), t).unwrap_err(), OFF, "{t}");
    }
    let read = policy(Mode::Read, true);
    assert_eq!(gate(&read, "snapshot"), Ok(Class::Read));
    for t in ["click", "type", "press", "navigate", "eval"] {
        let e = gate(&read, t).unwrap_err();
        assert!(e.contains("read only") && e.contains(&format!("kelpie_browser_{t}")) && e.contains("Settings → Agents → Browser panes"), "{e}");
    }
    let act = policy(Mode::Act, false);
    assert_eq!(gate(&act, "click"), Ok(Class::Act));
    assert_eq!(gate(&act, "eval").unwrap_err(), EVAL_OFF);
    assert_eq!(gate(&policy(Mode::Act, true), "eval"), Ok(Class::Eval));
    assert!(gate(&act, "drag").unwrap_err().contains("unknown browser tool"));
}

#[test]
fn sites_are_origins() {
    assert_eq!(site("http://LocalHost:3000/").unwrap(), "http://localhost:3000");
    assert_eq!(site(" https://example.com:443 ").unwrap(), "https://example.com");
    assert_eq!(site("http://127.0.0.1").unwrap(), "http://127.0.0.1");
    for bad in ["example.com", "ftp://example.com", "file:///c:/x", "http://u:p@example.com", "https://example.com/admin", "https://example.com/?q=1", "https://example.com/#x", "javascript:alert(1)", ""] {
        assert!(site(bad).is_err(), "{bad}");
    }
}

#[test]
fn the_policy_reads_and_writes_its_keys_and_a_bad_request_changes_nothing() {
    assert_eq!(Policy::from_file(None), Policy::default());
    assert_eq!(Policy::default().mode, Mode::Off);
    let file = json!({"browser_agents": "act", "browser_eval": true, "browser_sites": ["http://localhost:5173/", "http://localhost:5173"]});
    let p = Policy::from_file(Some(&file));
    assert_eq!(p, Policy { mode: Mode::Act, eval: true, sites: vec!["http://localhost:5173".into()] });
    // A value that is not valid in the file is the default, not an error.
    let odd = Policy::from_file(Some(&json!({"browser_agents": "all", "browser_eval": "yes", "browser_sites": ["nope"]})));
    assert_eq!(odd, Policy::default());
    let mut written = json!({"other": 1});
    p.write_into(&mut written);
    assert_eq!(written, json!({"other": 1, "browser_agents": "act", "browser_eval": true, "browser_sites": ["http://localhost:5173"]}));

    let now = Policy { mode: Mode::Read, eval: false, sites: vec!["http://a.test".into()] };
    assert_eq!(Policy::requested(&json!({"sub_sessions": "allow"}), &now).unwrap(), None);
    assert_eq!(Policy::requested(&json!({"browser_eval": true}), &now).unwrap(), Some(Policy { eval: true, ..now.clone() }));
    let all = Policy::requested(&json!({"browser_agents": "act", "browser_sites": ["https://b.test/"]}), &now).unwrap().unwrap();
    assert_eq!((all.mode, all.sites), (Mode::Act, vec!["https://b.test".to_string()]));
    let many: Vec<String> = (0..=MAX_SITES).map(|i| format!("http://h{i}.test")).collect();
    for bad in [
        json!({"browser_agents": "on"}),
        json!({"browser_agents": 2}),
        json!({"browser_eval": "true"}),
        json!({"browser_sites": "http://a.test"}),
        json!({"browser_sites": ["http://a.test/path"]}),
        json!({"browser_sites": [1]}),
        json!({"browser_sites": many}),
        json!({"browser_agents": "act", "browser_sites": ["ftp://x"]}),
    ] {
        assert!(Policy::requested(&bad, &now).is_err(), "{bad}");
    }
}

#[test]
fn arguments_get_their_defaults() {
    let ok = |tool: &str, args: Value| check_args(tool, &args).unwrap_or_else(|e| panic!("{tool} {args}: {e}"));
    assert_eq!(ok("tabs", Value::Null), json!({}));
    assert_eq!(ok("snapshot", json!({})), json!({"mode": "compact", "max_chars": 8000}));
    assert_eq!(ok("snapshot", json!({"mode": "full", "root": "e4", "pane": 7})), json!({"pane": "7", "mode": "full", "root": "e4", "max_chars": 18000}));
    assert_eq!(ok("click", json!({"ref": "e6"})), json!({"ref": "e6", "button": "left", "double": false, "dialog": "dismiss", "snapshot_after": "diff"}));
    assert_eq!(
        ok("type", json!({"ref": "e3", "text": "  two spaces  "})),
        json!({"ref": "e3", "text": "  two spaces  ", "clear": true, "submit": false, "dialog": "dismiss", "snapshot_after": "diff"})
    );
    assert_eq!(ok("type", json!({"ref": "e3", "text": ""}))["text"], "", "empty clears the field");
    assert_eq!(ok("press", json!({"key": "Enter", "modifiers": ["Control", "Shift", "Control"]}))["modifiers"], json!(["Control", "Shift"]));
    for key in ["F12", "a", "/", "ArrowDown", "Space", "é"] {
        ok("press", json!({"key": key}));
    }
    assert_eq!(ok("navigate", json!({"url": "http://localhost:3000"})), json!({"url": "http://localhost:3000/", "timeout_s": 15}));
    assert_eq!(ok("navigate", json!({"action": "back", "timeout_s": 25})), json!({"action": "back", "timeout_s": 25}));
    assert_eq!(ok("wait", json!({"text": "Welcome"})), json!({"text": "Welcome", "timeout_s": 10}));
    assert_eq!(ok("wait", json!({"load": true, "timeout_s": 30})), json!({"load": true, "timeout_s": 30}));
    assert_eq!(ok("screenshot", json!({})), json!({"full_page": false}));
    assert_eq!(ok("console", json!({"since": 12})), json!({"level": "all", "since": 12, "max": 50}));
    assert_eq!(ok("network", json!({"filter": "/api", "max": 300})), json!({"filter": "/api", "failed_only": false, "max": 300}));
    assert_eq!(ok("eval", json!({"expression": "document.title"})), json!({"expression": "document.title"}));
}

#[test]
fn bad_arguments_are_refused_without_repeating_their_values() {
    let secret = "hunter2-very-secret";
    for (tool, args, expect) in [
        ("tabs", json!([1]), "must be an object"),
        ("tabs", json!({"secret": 1}), "takes no `secret`"),
        ("click", json!({"ref": "e1", "text": secret}), "takes no `text`"),
        ("click", json!({}), "missing `ref`"),
        ("click", json!({"ref": "button"}), "like e12"),
        ("click", json!({"ref": "e1", "button": "middle"}), "`button` is one of left, right"),
        ("click", json!({"ref": "e1", "double": "yes"}), "true or false"),
        ("type", json!({"ref": "e1"}), "missing `text`"),
        ("type", json!({"ref": "e1", "text": 5}), "`text` must be text"),
        ("type", json!({"ref": "e1", "text": "x".repeat(MAX_TEXT + 1)}), "longer than"),
        ("press", json!({"key": secret}), "`key` is one of"),
        ("press", json!({"key": "F13"}), "`key` is one of"),
        ("press", json!({"key": "Enter", "modifiers": ["Hyper"]}), "`modifiers`"),
        ("navigate", json!({}), "either `url` or `action`"),
        ("navigate", json!({"url": "http://a.test", "action": "back"}), "either `url` or `action`"),
        ("navigate", json!({"url": format!("ftp://a.test/?token={secret}")}), "http:// or https://"),
        ("navigate", json!({"url": "http://a.test", "timeout_s": 26}), "from 1 to 25"),
        ("wait", json!({}), "exactly one"),
        ("wait", json!({"text": "a", "selector": "#b"}), "exactly one"),
        ("wait", json!({"load": false}), "`load`"),
        ("wait", json!({"text": "a", "timeout_s": 31}), "from 1 to 30"),
        ("snapshot", json!({"max_chars": MAX_SNAPSHOT_CHARS + 1}), "from 200 to 19000"),
        ("snapshot", json!({"mode": "tree"}), "compact, full"),
        ("console", json!({"max": 0}), "from 1 to 300"),
        ("console", json!({"level": "debug"}), "error, warning, all"),
        ("eval", json!({}), "missing `expression`"),
        ("eval", json!({"expression": "1".repeat(MAX_EXPRESSION + 1)}), "longer than"),
        ("tabs", json!({"pane": {"id": 1}}), "`pane` is an id"),
        ("drag", json!({}), "unknown browser tool"),
    ] {
        let e = check_args(tool, &args).expect_err(&format!("{tool} {args}"));
        assert!(e.contains(expect), "{tool} {args}: {e}");
        assert!(!e.contains(secret), "{e}");
    }
}

#[test]
fn a_tool_that_waits_is_given_its_time_and_the_rest_15_s() {
    let secs = Duration::from_secs;
    assert_eq!(limit("wait", &json!({"timeout_s": 10})), secs(15));
    assert_eq!(limit("wait", &json!({"timeout_s": 30})), secs(35));
    assert_eq!(limit("wait", &json!({"timeout_s": 999})), secs(35));
    assert_eq!(limit("navigate", &json!({"timeout_s": 25})), secs(30));
    assert_eq!(limit("navigate", &json!({})), secs(20));
    for t in ["snapshot", "click", "type", "screenshot", "eval"] {
        assert_eq!(limit(t, &json!({})), secs(15), "{t}");
    }
}

#[test]
fn the_rate_is_ten_a_second_and_twenty_at_once_per_session() {
    let mut rate = Rate::default();
    let t0 = Instant::now();
    for i in 0..20 {
        assert!(rate.take("a", t0), "burst {i}");
    }
    assert!(!rate.take("a", t0), "the 21st at once");
    assert!(rate.take("b", t0), "another session has its own");
    assert!(!rate.take("a", t0 + Duration::from_millis(50)), "half a call back");
    assert!(rate.take("a", t0 + Duration::from_millis(110)), "one back after 100 ms");
    assert!(!rate.take("a", t0 + Duration::from_millis(120)));
    // Left alone, a session gets its burst back, and no more.
    let later = t0 + Duration::from_secs(60);
    for i in 0..20 {
        assert!(rate.take("a", later), "again {i}");
    }
    assert!(!rate.take("a", later));
    assert_eq!(rate.buckets.len(), 1, "idle sessions are forgotten");
}

#[test]
fn an_audit_line_keeps_only_what_it_may() {
    let password = "correct-horse-battery-staple";
    let args = json!({"ref": "e3", "text": password, "url": "https://bank.test/login?token=abc"});
    let reply = json!({"text": format!("Typed into {password}"), "audit": {"host": "user@bank.test/path?q=1", "target": format!("textbox \"Password\" {}", "x".repeat(100))}});
    let line = audit_line(1, "s1", "api", "type", &args, Some(&reply), None);
    let text = line.to_string();
    assert!(!text.contains(password) && !text.contains("token") && !text.contains("q=1") && !text.contains("user@"), "{text}");
    assert_eq!(line["host"], "bank.test");
    assert_eq!(line["target"].as_str().unwrap().chars().count(), 60);
    assert_eq!((line["ok"].as_bool(), &line["error"]), (Some(true), &Value::Null));
    let keys: Vec<&String> = line.as_object().unwrap().keys().collect();
    assert_eq!(keys.len(), 8, "{keys:?}");
    // A navigation's host comes from its address when the app named none; its query never.
    let nav = audit_line(2, "s1", "api", "navigate", &args, None, Some("Kelpie's window is not open"));
    assert_eq!((nav["host"].as_str(), nav["ok"].as_bool()), (Some("bank.test"), Some(false)));
    assert!(!nav.to_string().contains("token"));
    // An error that carries a key loses it, and an address its query and fragment.
    let leaky = audit_line(3, "s1", "api", "click", &Value::Null, None, Some("failed: sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF"));
    assert!(!leaky.to_string().contains("abcdefghijklmnopqrstuvwxyz0123456789"), "{leaky}");
    let error = format!("Navigation to \"https://bank.test/cb?code=S3CRET-CODE#frag\" is not allowed for this pane\n{}", "x".repeat(300));
    let refused = audit_line(4, "s1", "api", "navigate", &Value::Null, Some(&json!({"audit": {"host": "https://bank.test/x?e=a@x.com", "target": "link \"https://bank.test/?t=S3CRET-CODE\""}})), Some(&error));
    let text = refused.to_string();
    assert!(!text.contains("S3CRET") && !text.contains("frag") && !text.contains("x.com"), "{text}");
    assert!(refused["error"].as_str().unwrap().starts_with("Navigation to \"https://bank.test/cb is not allowed"), "{refused}");
    assert_eq!(refused["error"].as_str().unwrap().chars().count(), 120);
    assert_eq!(refused["host"], "bank.test");
    assert_eq!(audit_line(5, "s", "l", "click", &Value::Null, Some(&json!({"audit": {"host": "not a host"}})), None)["host"], Value::Null);
}

#[test]
fn page_text_is_scrubbed_of_secrets_and_queries() {
    assert_eq!(scrub("see http://a.test/p?token=abc and https://b.test/#x", 200), "see http://a.test/p and https://b.test/");
    assert_eq!(scrub("two\nlines", 200), "two lines");
    assert_eq!(scrub("a plain? text# stays", 200), "a plain? text# stays");
    assert!(!scrub("Bearer abcdefghijklmnopqrstuvwxyz012345", 200).contains("abcdefghijklmnopqrstuvwxyz"));
    assert_eq!(scrub(&"y".repeat(500), 60).chars().count(), 60);
}

#[test]
fn a_full_audit_file_that_cannot_move_aside_still_gets_the_line() {
    let dir = std::env::temp_dir().join(format!("kelpie-browser-audit-stuck-{}", uuid::Uuid::new_v4()));
    // A folder where the older file goes: the move fails.
    std::fs::create_dir_all(dir.join("browser-audit.jsonl.1").join("in-the-way")).unwrap();
    let audit = Audit::new(&dir);
    audit.append(&json!({"n": 0, "pad": "x".repeat(ROTATE_AT as usize)})).unwrap();
    audit.append(&json!({"n": 1})).unwrap();
    assert_eq!(audit.tail(1)[0]["n"], 1, "written to the file as it is");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_audit_rotates_and_reads_back_newest_last() {
    let dir = std::env::temp_dir().join(format!("kelpie-browser-audit-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let audit = Audit::new(&dir);
    assert!(audit.tail(10).is_empty());
    // One big line puts the file over the size: the next one starts a new file.
    let big = json!({"n": 0, "pad": "x".repeat(ROTATE_AT as usize)});
    audit.append(&big).unwrap();
    for n in 1..=3 {
        audit.append(&json!({"n": n})).unwrap();
    }
    assert!(dir.join("browser-audit.jsonl.1").exists());
    assert!(std::fs::metadata(dir.join(AUDIT_FILE)).unwrap().len() < 100);
    let ns = |v: Vec<Value>| v.iter().map(|l| l["n"].as_u64().unwrap()).collect::<Vec<_>>();
    assert_eq!(ns(audit.tail(2)), vec![2, 3]);
    assert_eq!(ns(audit.tail(10)), vec![0, 1, 2, 3], "the older file fills in");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- the request to the app ----------

fn app() -> (Arc<Providers>, tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>) {
    let p = Arc::new(Providers::default());
    let (tx, rx) = crate::session::client_channel();
    p.register(9, &Role::App, "browser", None, &tx).unwrap();
    (p, rx)
}

fn sent(o: &crate::session::Outgoing) -> Value {
    serde_json::from_slice(&o.data[5..]).unwrap()
}

#[tokio::test]
async fn the_app_is_asked_with_the_session_and_the_policy() {
    let (p, mut rx) = app();
    let pol = Policy { mode: Mode::Act, eval: false, sites: vec!["http://a.test".into()] };
    let session = json!({"id": "s1", "name": "api", "kind": "claude", "cwd": "/p", "parent": null});
    let params = request(session.clone(), "click", &json!({"ref": "e1"}), &pol);
    let asking = tokio::spawn({
        let p = p.clone();
        async move { forward(&p, params, Duration::from_secs(5)).await }
    });
    let req = sent(&rx.recv().await.unwrap());
    assert_eq!(req["method"], "browser.agent");
    assert_eq!(req["params"]["session"], session);
    assert_eq!(req["params"]["tool"], "click");
    assert_eq!(req["params"]["policy"], json!({"mode": "act", "eval": false, "sites": ["http://a.test"]}));
    let answer = json!({"text": "Clicked.", "audit": {"host": "a.test", "target": "button \"Go\""}});
    p.reply(9, &json!({"preq": req["preq"], "result": answer.clone()}));
    assert_eq!(asking.await.unwrap().unwrap(), answer);
}

#[tokio::test]
async fn errors_from_the_app_say_what_to_do() {
    // No window at all.
    let none = Providers::default();
    assert_eq!(forward(&none, json!({}), Duration::from_secs(1)).await.unwrap_err(), NOT_OPEN);
    // Its own refusal, as it said it.
    let (p, mut rx) = app();
    let ask = |p: Arc<Providers>, limit: Duration| tokio::spawn(async move { forward(&p, json!({}), limit).await });
    let asking = ask(p.clone(), Duration::from_secs(5));
    let preq = sent(&rx.recv().await.unwrap())["preq"].clone();
    p.reply(9, &json!({"preq": preq, "error": {"message": "This pane was not opened for you"}}));
    assert_eq!(asking.await.unwrap().unwrap_err(), "This pane was not opened for you");
    // Words like the channel's own in the window's refusal are the window's; a page's address in it loses its query.
    let asking = ask(p.clone(), Duration::from_secs(5));
    let preq = sent(&rx.recv().await.unwrap())["preq"].clone();
    p.reply(9, &json!({"preq": preq, "error": {"message": "the tab went away while loading http://a.test/x?code=S3CRET"}}));
    assert_eq!(asking.await.unwrap().unwrap_err(), "the tab went away while loading http://a.test/x");
    // An app older than its daemon.
    let asking = ask(p.clone(), Duration::from_secs(5));
    let preq = sent(&rx.recv().await.unwrap())["preq"].clone();
    p.reply(9, &json!({"preq": preq, "error": {"message": "unknown request browser.agent"}}));
    assert_eq!(asking.await.unwrap().unwrap_err(), OLD_APP);
    // An answer that is not the reply's shape is an older app too.
    let asking = ask(p.clone(), Duration::from_secs(5));
    let preq = sent(&rx.recv().await.unwrap())["preq"].clone();
    p.reply(9, &json!({"preq": preq, "result": true}));
    assert_eq!(asking.await.unwrap().unwrap_err(), OLD_APP);
    // No answer within the tool's time.
    let started = Instant::now();
    let e = ask(p.clone(), Duration::from_millis(200)).await.unwrap().unwrap_err();
    assert!(e.starts_with("Kelpie's window did not answer within 0 s") && started.elapsed() < Duration::from_secs(2), "{e}");
    rx.recv().await.unwrap();
    // The window going away answers at once.
    let asking = ask(p.clone(), Duration::from_secs(30));
    rx.recv().await.unwrap();
    let started = Instant::now();
    p.client_gone(9);
    assert_eq!(asking.await.unwrap().unwrap_err(), NOT_OPEN);
    assert!(started.elapsed() < Duration::from_secs(1));
}
