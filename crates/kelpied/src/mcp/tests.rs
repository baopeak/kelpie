use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::*;

/// A daemon that answers from a script: per method, the replies in order (the last one repeats). Calls are recorded.
#[derive(Clone, Default)]
struct Fake {
    me: String,
    replies: HashMap<String, Vec<Result<Value, String>>>,
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Fake {
    fn new(me: &str) -> Fake {
        Fake { me: me.into(), ..Default::default() }
    }

    fn reply(mut self, method: &str, value: Value) -> Fake {
        self.replies.entry(method.into()).or_default().push(Ok(value));
        self
    }

    fn refuse(mut self, method: &str, text: &str) -> Fake {
        self.replies.entry(method.into()).or_default().push(Err(text.into()));
        self
    }

    fn calls_of(&self, method: &str) -> Vec<Value> {
        self.calls.lock().unwrap().iter().filter(|(m, _)| m == method).map(|(_, p)| p.clone()).collect()
    }
}

impl Backend for Fake {
    fn me(&self) -> String {
        self.me.clone()
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let n = self.calls.lock().unwrap().iter().filter(|(m, _)| m == method).count();
        self.calls.lock().unwrap().push((method.to_string(), params));
        let replies = self.replies.get(method).ok_or_else(|| anyhow::anyhow!("{method}: not scripted"))?;
        match &replies[n.min(replies.len() - 1)] {
            Ok(v) => Ok(v.clone()),
            Err(e) => Err(anyhow::anyhow!("{method}: {e}")),
        }
    }
}

struct Conn(Option<Fake>);

impl Connector for Conn {
    type B = Fake;

    async fn connect(&self) -> Result<Fake> {
        self.0.clone().ok_or_else(|| anyhow::anyhow!(NOT_FROM_KELPIE))
    }
}

async fn rpc(conn: &Conn, message: Value) -> Option<Value> {
    handle_line(&message.to_string(), conn).await
}

fn call(id: impl serde::Serialize, name: &str, args: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {"name": name, "arguments": args}})
}

fn session(handle: u64, id: &str, parent: Option<&str>, status: &str) -> Value {
    json!({"handle": handle, "id": id, "name": format!("s{handle}"), "kind": "codex", "cwd": "/p", "status": status, "parent": parent})
}

#[tokio::test]
async fn initialize_answers_a_known_version_and_carries_the_guide() {
    let conn = Conn(None);
    for (asked, answered) in [("2025-03-26", "2025-03-26"), ("2025-06-18", "2025-06-18"), ("2031-01-01", DEFAULT_VERSION)] {
        let r = rpc(&conn, json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": asked, "capabilities": {}}})).await.unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], answered);
        assert_eq!(r["result"]["serverInfo"]["name"], "kelpie");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        assert_eq!(r["result"]["instructions"], crate::sub_guide::GUIDE);
    }
}

#[tokio::test]
async fn notifications_get_no_reply_and_ping_answers() {
    let conn = Conn(None);
    assert!(rpc(&conn, json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).await.is_none());
    assert!(rpc(&conn, json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 3}})).await.is_none());
    assert!(handle_line("   ", &conn).await.is_none());
    assert_eq!(rpc(&conn, json!({"jsonrpc": "2.0", "id": "a", "method": "ping"})).await.unwrap()["result"], json!({}));
}

#[tokio::test]
async fn bad_input_gets_the_json_rpc_error() {
    let conn = Conn(None);
    let parse = handle_line("{not json", &conn).await.unwrap();
    assert_eq!((parse["error"]["code"].as_i64(), &parse["id"]), (Some(-32700), &Value::Null));
    let unknown = rpc(&conn, json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"})).await.unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    assert_eq!(rpc(&conn, json!([1, 2])).await.unwrap()["error"]["code"], -32600);
    assert_eq!(rpc(&conn, json!({"jsonrpc": "2.0", "id": 5})).await.unwrap()["error"]["code"], -32600);
    assert_eq!(rpc(&conn, json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "kelpie_nope"}})).await.unwrap()["error"]["code"], -32602);
    assert_eq!(rpc(&conn, json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "kelpie_agents", "arguments": [1]}})).await.unwrap()["error"]["code"], -32602);
}

#[tokio::test]
async fn tools_list_has_every_tool_with_an_object_schema() {
    let r = rpc(&Conn(None), json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})).await.unwrap();
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, TOOL_NAMES);
    for t in tools {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(!t["description"].as_str().unwrap().is_empty());
        // Everything required is described.
        for need in t["inputSchema"]["required"].as_array().unwrap() {
            assert!(t["inputSchema"]["properties"][need.as_str().unwrap()].is_object(), "{t}");
        }
    }
}

#[tokio::test]
async fn outside_kelpie_every_tool_says_to_start_the_agent_from_kelpie() {
    for name in TOOL_NAMES {
        let r = rpc(&Conn(None), call(1, name, json!({}))).await.unwrap();
        assert_eq!(r["result"]["isError"], true, "{name}");
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Start this agent from Kelpie"), "{text}");
        assert!(r["result"].get("structuredContent").is_none());
    }
}

#[tokio::test]
async fn a_result_has_text_and_structured_content() {
    let fake = Fake::new("me").reply("agent.presets", json!([
        {"id": "claude", "label": "Claude Code", "installed": true, "outcomes": {"agent": "claude", "count": 4, "judged": 3, "done_share": 0.7, "median_minutes": 12.5}},
        {"id": "codex", "label": "Codex", "installed": true, "outcomes": null},
        {"id": "gemini", "label": "Gemini", "installed": false, "outcomes": null},
    ]));
    let r = rpc(&Conn(Some(fake)), call(1, "kelpie_agents", json!({}))).await.unwrap();
    assert_eq!(r["result"]["isError"], false);
    let agents = r["result"]["structuredContent"]["agents"].as_array().unwrap();
    assert_eq!(agents.len(), 2, "not installed is left out");
    assert_eq!((agents[0]["finished"].as_u64(), agents[1]["finished"].as_u64()), (Some(4), Some(0)));
    let text = r["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("claude - Claude Code: 4 finished, 70% of 3 judged done, median 12.5 min"), "{text}");
    assert!(text.contains("codex - Codex: no finished sub-session yet"), "{text}");
}

#[tokio::test]
async fn spawn_sends_only_what_was_given_and_validates_arguments() {
    let fake = Fake::new("me").reply("sub.spawn", json!({"id": "i1", "handle": 7, "name": "fix", "kind": "codex", "cwd": "/p", "worktree": {"path": "/p/wt", "branch": "kelpie/sub-fix", "base": "main"}}));
    let mut b = fake.clone();
    let out = run_tool(&mut b, "kelpie_spawn", &json!({"agent": "codex", "task": " fix #1 ", "worktree": true, "model": "gpt-x", "name": ""})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fake.calls_of("sub.spawn"), vec![json!({"agent": "codex", "task": "fix #1", "worktree": true, "model": "gpt-x"})]);
    assert!(out.text.contains("Opened sub-session #7") && out.text.contains("End your turn"), "{}", out.text);
    assert!(out.text.contains("branch kelpie/sub-fix"));
    assert_eq!(out.data["handle"], 7);

    for (args, expect) in [
        (json!({"task": "x"}), "missing `agent`"),
        (json!({"agent": "codex"}), "missing `task`"),
        (json!({"agent": "codex", "task": "x", "worktree": "yes"}), "`worktree` must be true or false"),
        (json!({"agent": "codex", "task": 5}), "`task` must be text"),
        (json!({"agent": "codex", "task": "x", "cwd": 3}), "`cwd` must be text"),
    ] {
        let out = run_tool(&mut fake.clone(), "kelpie_spawn", &args).await;
        assert!(out.is_error && out.text.contains(expect), "{args}: {}", out.text);
    }
    assert_eq!(fake.calls_of("sub.spawn").len(), 1, "an invalid call reaches no daemon");
}

#[tokio::test]
async fn the_daemons_refusal_reaches_the_model_without_the_method_prefix() {
    let reason = "#3 \"api\" (claude) runs with permission checks off and this session does not; a message from here could make it act with nobody asked. Ask the user to tell it instead";
    let mut b = Fake::new("me").refuse("sub.message", reason);
    let out = run_tool(&mut b, "kelpie_message", &json!({"to": "3", "text": "go"})).await;
    assert!(out.is_error);
    assert_eq!(out.text, reason);
    // The tool says it up front, too.
    let list = tool_list();
    let desc = list.iter().find(|t| t["name"] == "kelpie_message").unwrap()["description"].as_str().unwrap();
    assert!(desc.contains("permission checks off") && desc.contains("ask the user"), "{desc}");
}

#[tokio::test]
async fn message_says_whether_it_was_queued() {
    let queued = json!({"queued": true, "to": {"id": "i", "handle": 3, "label": "sub-session \"x\" (codex, #3)"}});
    let out = run_tool(&mut Fake::new("m").reply("sub.message", queued.clone()), "kelpie_message", &json!({"to": "x", "text": "hi"})).await;
    assert!(out.text.starts_with("Queued for #3"), "{}", out.text);
    let dup = json!({"queued": false, "to": queued["to"]});
    let out = run_tool(&mut Fake::new("m").reply("sub.message", dup), "kelpie_message", &json!({"to": "x", "text": "hi"})).await;
    assert!(out.text.starts_with("Not queued"), "{}", out.text);
    assert!(run_tool(&mut Fake::new("m"), "kelpie_message", &json!({"to": "x"})).await.text.contains("missing `text`"));
}

#[test]
fn session_rows_follow_the_family_or_show_everything() {
    let list = [
        session(1, "me", None, "working"),
        session(2, "c1", Some("me"), "working"),
        session(3, "g1", Some("c1"), "idle"),
        session(4, "other", None, "idle"),
        session(5, "stray", Some("other"), "idle"),
    ];
    let handles = |rows: &[Value]| rows.iter().map(|r| r["handle"].as_u64().unwrap()).collect::<Vec<_>>();
    assert_eq!(handles(&session_rows(&list, "me", "children").unwrap()), vec![2, 3]);
    assert_eq!(handles(&session_rows(&list, "me", "all").unwrap()), vec![1, 2, 3, 4, 5]);
    assert_eq!(handles(&session_rows(&list, "nobody", "children").unwrap()), Vec::<u64>::new());
    let rows = session_rows(&list, "me", "children").unwrap();
    assert_eq!((rows[0]["parent"].as_u64(), rows[1]["parent"].as_u64()), (Some(1), Some(2)));
    assert!(session_rows(&list, "me", "some").unwrap_err().contains("children or all"));
}

#[tokio::test]
async fn sessions_show_the_last_report() {
    let mut s = session(2, "c1", Some("me"), "idle");
    s["report"] = json!({"status": "done", "summary": "fixed it\nall green", "at": 1, "auto": false});
    let list = json!([session(1, "me", None, "working"), s]);
    let out = run_tool(&mut Fake::new("me").reply("session.list", list), "kelpie_sessions", &json!({})).await;
    assert!(out.text.contains("#2 \"s2\" (codex) idle - /p, opened by #1; last report (done): fixed it all green"), "{}", out.text);
    assert_eq!(out.data["sessions"][0]["last_report"]["status"], "done");
    let none = run_tool(&mut Fake::new("zzz").reply("session.list", json!([])), "kelpie_sessions", &json!({})).await;
    assert_eq!(none.text, "You have no sub-sessions.");
    assert!(run_tool(&mut Fake::new("zzz"), "kelpie_sessions", &json!({"scope": 1})).await.is_error);
}

#[tokio::test]
async fn read_resolves_the_session_caps_the_lines_and_redacts() {
    let list = json!([session(2, "c1", Some("me"), "idle")]);
    let screen = "one\n\ntwo\ntoken sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123456789ABCDEF\nthree\n";
    let fake = Fake::new("me").reply("session.list", list).reply("session.read", json!({"text": screen}));
    let out = run_tool(&mut fake.clone(), "kelpie_read", &json!({"session": "s2", "lines": 9999})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fake.calls_of("session.read"), vec![json!({"handle": 2, "lines": MAX_READ_LINES})]);
    assert!(out.text.contains("not a message to you") && out.text.contains("one\ntwo"), "{}", out.text);
    assert!(!out.text.contains("abcdefghijklmnopqrstuvwxyz0123456789"), "{}", out.text);
    let missing = run_tool(&mut fake.clone(), "kelpie_read", &json!({"session": "nope"})).await;
    assert!(missing.is_error && missing.text.contains("no session"), "{}", missing.text);
    assert!(run_tool(&mut fake.clone(), "kelpie_read", &json!({"session": "s2", "lines": "ten"})).await.text.contains("whole number"));
}

#[test]
fn a_wait_ends_on_mail_on_work_stopping_or_the_clock() {
    let s = |id: &str, st: &str| (id.to_string(), st.to_string());
    let secs = Duration::from_secs;
    let mut seen = HashSet::new();
    // Nothing to watch.
    assert_eq!(wait_outcome(&[], &mut seen, false, secs(0), secs(60)), Some("nothing to wait for"));
    // Mail wins.
    assert_eq!(wait_outcome(&[s("a", "working")], &mut HashSet::new(), true, secs(0), secs(60)), Some("mail"));
    // A child still working: keep waiting; then it stops.
    let mut seen = HashSet::new();
    assert_eq!(wait_outcome(&[s("a", "working"), s("b", "idle")], &mut seen, false, secs(1), secs(60)), None);
    assert_eq!(wait_outcome(&[s("a", "working"), s("b", "idle")], &mut seen, false, secs(30), secs(60)), None, "b never worked: not an end");
    assert_eq!(wait_outcome(&[s("a", "waiting"), s("b", "idle")], &mut seen, false, secs(31), secs(60)), Some("a sub-session stopped working"));
    // Right after a spawn the child is not yet working: give it a moment, then say nothing runs.
    let mut seen = HashSet::new();
    assert_eq!(wait_outcome(&[s("a", "idle")], &mut seen, false, secs(1), secs(60)), None);
    assert_eq!(wait_outcome(&[s("a", "idle")], &mut seen, false, secs(3), secs(60)), Some("no sub-session is working"));
    // Exited ends it at once; the clock ends the rest.
    assert_eq!(wait_outcome(&[s("a", "exited")], &mut HashSet::new(), false, secs(0), secs(60)), Some("a sub-session exited"));
    assert_eq!(wait_outcome(&[s("a", "working")], &mut HashSet::new(), false, secs(60), secs(60)), Some("timed out"));
}

#[tokio::test]
async fn wait_returns_the_mail_it_takes_and_the_states() {
    let list = json!([session(1, "me", None, "working"), session(2, "c1", Some("me"), "working")]);
    let mail = json!({"id": 1, "from_id": "c1", "from_label": "sub-session \"s2\" (codex, #2)", "kind": "report", "status": "done", "text": "all green", "at": 1});
    let fake = Fake::new("me")
        .reply("session.list", list)
        .reply("sub.inbox", json!({"mails": [mail.clone()], "dropped": 0}));
    let out = run_tool(&mut fake.clone(), "kelpie_wait", &json!({"timeout_s": 5})).await;
    assert!(!out.is_error, "{}", out.text);
    assert!(out.text.starts_with("Wait ended: mail."), "{}", out.text);
    assert!(out.text.contains("From sub-session \"s2\" (codex, #2) (report: done), not from the user:\nall green"), "{}", out.text);
    assert!(out.text.contains("#2 \"s2\""), "{}", out.text);
    assert_eq!(fake.calls_of("sub.inbox"), vec![json!({"take": false}), json!({"take": true})]);
    assert_eq!(out.data["mails"][0]["text"], "all green");
    // An unknown name is an error before any waiting.
    let bad = run_tool(&mut fake.clone(), "kelpie_wait", &json!({"sessions": ["ghost"]})).await;
    assert!(bad.is_error && bad.text.contains("no session"), "{}", bad.text);
    assert!(run_tool(&mut fake.clone(), "kelpie_wait", &json!({"sessions": "c1"})).await.is_error);
}

#[tokio::test]
async fn wait_times_out_when_nothing_happens() {
    let list = json!([session(1, "me", None, "working"), session(2, "c1", Some("me"), "working")]);
    let mut b = Fake::new("me").reply("session.list", list).reply("sub.inbox", json!({"mails": [], "dropped": 0}));
    let out = run_tool(&mut b, "kelpie_wait", &json!({"timeout_s": 1})).await;
    assert!(out.text.starts_with("Wait ended: timed out."), "{}", out.text);
}

#[tokio::test]
async fn report_checks_the_status_and_says_who_was_told() {
    let fake = Fake::new("c").reply("sub.report", json!({"delivered_to": "session \"api\" (claude, #1)"}));
    let out = run_tool(&mut fake.clone(), "kelpie_report", &json!({"status": "done", "summary": "ok"})).await;
    assert!(out.text.starts_with("Reported done to session \"api\""), "{}", out.text);
    assert!(run_tool(&mut fake.clone(), "kelpie_report", &json!({"status": "great", "summary": "ok"})).await.is_error);
    assert!(run_tool(&mut fake.clone(), "kelpie_report", &json!({"status": "done"})).await.text.contains("missing `summary`"));
    assert_eq!(fake.calls_of("sub.report").len(), 1);
    let orphan = run_tool(&mut Fake::new("c").reply("sub.report", json!({"delivered_to": null})), "kelpie_report", &json!({"status": "done", "summary": "x"})).await;
    assert!(orphan.text.contains("No session opened you"), "{}", orphan.text);
}

#[tokio::test]
async fn close_and_inbox() {
    let fake = Fake::new("p").reply("sub.close", json!(true));
    let out = run_tool(&mut fake.clone(), "kelpie_close", &json!({"session": "3"})).await;
    assert!(out.text.starts_with("Closed 3."), "{}", out.text);
    assert_eq!(fake.calls_of("sub.close"), vec![json!({"session": "3"})]);
    let refused = run_tool(&mut Fake::new("p").refuse("sub.close", "#9 is not one of your sub-sessions"), "kelpie_close", &json!({"session": "9"})).await;
    assert!(refused.is_error && refused.text == "#9 is not one of your sub-sessions");
    let empty = run_tool(&mut Fake::new("p").reply("sub.inbox", json!({"mails": [], "dropped": 0})), "kelpie_inbox", &json!({})).await;
    assert_eq!(empty.text, "No mail.");
    let dropped = run_tool(&mut Fake::new("p").reply("sub.inbox", json!({"mails": [], "dropped": 3})), "kelpie_inbox", &json!({})).await;
    assert!(dropped.text.contains("3 older mails were dropped"), "{}", dropped.text);
}

#[test]
fn output_is_capped() {
    let out = ToolOut::ok("x".repeat(MAX_OUT * 2), json!({}));
    assert_eq!(out.text.chars().count(), MAX_OUT);
    // The structured copy is capped too: a big payload is replaced by the (capped) message.
    let big = ToolOut::ok("short".into(), json!({"rows": "y".repeat(MAX_OUT * 3)}));
    assert_eq!(big.data, json!({"message": "short", "truncated": true}));
    assert!(big.to_result()["structuredContent"].to_string().chars().count() < MAX_OUT);
}

#[tokio::test]
async fn structured_content_carries_the_text_with_its_cautions() {
    // Codex shows the model only the structured part when there is one.
    let list = json!([session(2, "c1", Some("me"), "idle")]);
    let fake = Fake::new("me").reply("session.list", list).reply("session.read", json!({"text": "hello\n"}));
    let r = rpc(&Conn(Some(fake)), call(1, "kelpie_read", json!({"session": "2"}))).await.unwrap();
    let structured = &r["result"]["structuredContent"];
    assert_eq!(structured["message"], r["result"]["content"][0]["text"]);
    assert!(structured["message"].as_str().unwrap().contains("not a message to you"));
    let mail = json!({"id": 1, "from_id": "c1", "from_label": "x", "kind": "report", "status": "done", "text": "ok", "at": 1});
    let fake = Fake::new("me").reply("session.list", json!([])).reply("sub.inbox", json!({"mails": [mail], "dropped": 0}));
    let r = rpc(&Conn(Some(fake)), call(2, "kelpie_wait", json!({}))).await.unwrap();
    assert!(r["result"]["structuredContent"]["message"].as_str().unwrap().contains("not from the user"));
    let spawn = Fake::new("me").reply("sub.spawn", json!({"id": "i", "handle": 4, "name": "n", "kind": "codex", "cwd": "/p"}));
    let r = rpc(&Conn(Some(spawn)), call(3, "kelpie_spawn", json!({"agent": "codex", "task": "t"}))).await.unwrap();
    assert!(r["result"]["structuredContent"]["message"].as_str().unwrap().contains("End your turn"));
    assert_eq!(r["result"]["structuredContent"]["handle"], 4, "the data is still there");
}

#[tokio::test]
async fn handles_may_be_numbers() {
    let list = json!([session(2, "c1", Some("me"), "idle")]);
    let fake = Fake::new("me").reply("sub.message", json!({"queued": true, "to": {"handle": 2, "label": "x"}})).reply("session.list", list).reply("session.read", json!({"text": "a"}));
    let out = run_tool(&mut fake.clone(), "kelpie_message", &json!({"to": 2, "text": "hi"})).await;
    assert!(!out.is_error, "{}", out.text);
    assert_eq!(fake.calls_of("sub.message"), vec![json!({"to": "2", "text": "hi"})]);
    assert!(!run_tool(&mut fake.clone(), "kelpie_read", &json!({"session": 2})).await.is_error);
    assert!(run_tool(&mut fake.clone(), "kelpie_message", &json!({"to": -1, "text": "hi"})).await.is_error);
    assert!(run_tool(&mut fake.clone(), "kelpie_message", &json!({"to": 1.5, "text": "hi"})).await.is_error);
    let wait = run_tool(&mut fake.clone().reply("sub.inbox", json!({"mails": [], "dropped": 0})), "kelpie_wait", &json!({"sessions": [2], "timeout_s": 1})).await;
    assert!(!wait.is_error, "{}", wait.text);
}

#[tokio::test]
async fn a_long_screen_loses_its_oldest_lines_not_its_newest() {
    let list = json!([session(2, "c1", Some("me"), "idle")]);
    let screen: String = (0..400).map(|i| format!("line {i:03} {}\n", "x".repeat(150))).collect();
    let fake = Fake::new("me").reply("session.list", list).reply("session.read", json!({"text": screen}));
    let out = run_tool(&mut fake.clone(), "kelpie_read", &json!({"session": "2", "lines": 200})).await;
    assert!(out.text.chars().count() <= MAX_OUT);
    assert!(out.text.contains("line 399"), "the newest line stays");
    assert!(out.text.contains("older lines cut") && !out.text.contains("line 200 "), "{}", &out.text[..300]);
}

#[tokio::test]
async fn a_session_that_had_exited_before_the_wait_does_not_end_it() {
    let list = json!([session(1, "me", None, "working"), session(2, "dead", Some("me"), "exited"), session(3, "busy", Some("me"), "working")]);
    let fake = Fake::new("me").reply("session.list", list).reply("sub.inbox", json!({"mails": [], "dropped": 0}));
    let out = run_tool(&mut fake.clone(), "kelpie_wait", &json!({"timeout_s": 2})).await;
    assert!(out.text.starts_with("Wait ended: timed out."), "{}", out.text);
    assert!(out.text.contains("#2 \"s2\" (codex) exited"), "it is still shown: {}", out.text);
    // Only exited children: nothing to wait for.
    let only = Fake::new("me").reply("session.list", json!([session(1, "me", None, "working"), session(2, "dead", Some("me"), "exited")])).reply("sub.inbox", json!({"mails": [], "dropped": 0}));
    assert!(run_tool(&mut only.clone(), "kelpie_wait", &json!({})).await.text.starts_with("Wait ended: nothing to wait for."));
}

#[test]
fn only_the_listing_tools_say_they_read_only() {
    let hints: Vec<(String, bool)> = tool_list().iter().map(|t| (t["name"].as_str().unwrap().to_string(), t["annotations"]["readOnlyHint"].as_bool().unwrap())).collect();
    for (name, read_only) in hints {
        assert_eq!(read_only, matches!(name.as_str(), "kelpie_agents" | "kelpie_sessions" | "kelpie_read"), "{name}");
    }
}

// ---------- serving: tasks, cancellation, input ----------

use std::rc::Rc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// A daemon connection that never answers `sub.inbox` (a wait that goes on), counts how many are open, and answers the rest.
struct Held {
    open: Arc<AtomicUsize>,
    inner: Fake,
}

impl Drop for Held {
    fn drop(&mut self) {
        self.open.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Backend for Held {
    fn me(&self) -> String {
        self.inner.me()
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        if method == "sub.inbox" {
            std::future::pending::<()>().await;
        }
        self.inner.call(method, params).await
    }
}

struct HeldConn {
    open: Arc<AtomicUsize>,
}

impl Connector for HeldConn {
    type B = Held;

    async fn connect(&self) -> Result<Held> {
        self.open.fetch_add(1, Ordering::SeqCst);
        let inner = Fake::new("me").reply("session.list", json!([session(2, "c1", Some("me"), "working")]));
        Ok(Held { open: self.open.clone(), inner })
    }
}

struct Wire {
    input: UnboundedSender<Input>,
    output: UnboundedReceiver<Value>,
}

impl Wire {
    fn send(&self, message: Value) {
        self.input.send(Input::Line(message.to_string())).unwrap();
    }

    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), self.output.recv()).await.expect("a reply in time").expect("the server is running")
    }
}

fn start<C: Connector + 'static>(connector: C) -> (Wire, tokio::task::JoinHandle<()>) {
    let (input, rx) = unbounded_channel();
    let (out_tx, output) = unbounded_channel();
    let server = tokio::task::spawn_local(serve_io(rx, out_tx, Rc::new(connector)));
    (Wire { input, output }, server)
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn a_waiting_call_does_not_hold_up_the_server_and_a_cancel_drops_it() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let open = Arc::new(AtomicUsize::new(0));
            let (mut wire, _server) = start(HeldConn { open: open.clone() });
            wire.send(call(7, "kelpie_wait", json!({"timeout_s": 600})));
            settle().await;
            assert_eq!(open.load(Ordering::SeqCst), 1, "the call is running, holding its connection");
            // Meanwhile the server answers.
            wire.send(json!({"jsonrpc": "2.0", "id": 8, "method": "ping"}));
            assert_eq!(wire.next().await["id"], 8);
            wire.send(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"}));
            assert_eq!(wire.next().await["id"], 9);
            // The client gives up on 7: the task and its connection go, and nothing is answered for it.
            wire.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7}}));
            settle().await;
            assert_eq!(open.load(Ordering::SeqCst), 0, "the daemon connection was closed");
            wire.send(json!({"jsonrpc": "2.0", "id": 10, "method": "ping"}));
            assert_eq!(wire.next().await["id"], 10, "no reply to 7 came first");
        })
        .await;
}

#[tokio::test]
async fn a_cancel_names_one_call_and_the_end_of_input_ends_the_rest() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let open = Arc::new(AtomicUsize::new(0));
            let (wire, server) = start(HeldConn { open: open.clone() });
            wire.send(call("a", "kelpie_wait", json!({})));
            wire.send(call("b", "kelpie_wait", json!({})));
            settle().await;
            assert_eq!(open.load(Ordering::SeqCst), 2);
            // String ids, an unknown id and a cancel without a request id do nothing wrong.
            wire.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "zzz"}}));
            wire.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {}}));
            wire.send(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": "a"}}));
            settle().await;
            assert_eq!(open.load(Ordering::SeqCst), 1);
            let Wire { input, .. } = wire;
            drop(input);
            server.await.unwrap();
            settle().await;
            assert_eq!(open.load(Ordering::SeqCst), 0, "EOF aborted the call still running");
        })
        .await;
}

#[tokio::test]
async fn bad_lines_are_answered_and_the_server_reads_on() {
    tokio::task::LocalSet::new()
        .run_until(async {
            let (mut wire, _server) = start(Conn(None));
            wire.input.send(Input::BadUtf8).unwrap();
            let e = wire.next().await;
            assert_eq!((e["error"]["code"].as_i64(), &e["id"]), (Some(-32700), &Value::Null));
            wire.input.send(Input::Line("{broken".into())).unwrap();
            assert_eq!(wire.next().await["error"]["code"], -32700);
            wire.send(json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}));
            assert_eq!(wire.next().await["id"], 1);
        })
        .await;
}
