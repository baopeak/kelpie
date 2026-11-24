//! A session the usage limit stops gets its "continue" timer from the daemon itself, when the setting is on.
use super::*;
use crate::session::fake;

fn daemon(name: &str) -> (Arc<Daemon>, PathBuf) {
    let dir = std::env::temp_dir().join(format!("kelpie-{name}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
}

fn call(d: &Arc<Daemon>, method: &str, params: Value) -> Result<Value> {
    let (tx, _rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), method, params).map(|v| v.expect("answered at once"))
}

/// What Claude Code prints when the limit stops it, for a reset two hours from now (local clock, as it prints it).
fn limit_text() -> (String, u64) {
    let at = chrono::Local::now() + chrono::Duration::hours(2);
    let text = format!("You've hit your 5-hour limit · resets {} (Asia/Ho_Chi_Minh)", at.format("%-I:%M%P"));
    (text, at.timestamp_millis() as u64)
}

/// A Claude terminal session that runs a turn and stops on the usage limit (its hooks, as Claude Code reports them).
fn limited_terminal(d: &Daemon, id: &str) -> Arc<Session> {
    let (session, _typed) = fake::running(1, id, "claude");
    d.insert_for_test(Entry::Term(session.clone()));
    let hook = |payload: Value| d.hook(&json!({"session_id": id, "agent": "claude", "payload": payload})).unwrap();
    hook(json!({"hook_event_name": "UserPromptSubmit", "session_id": "conv-1"}));
    hook(json!({"hook_event_name": "StopFailure", "session_id": "conv-1", "error": "rate_limit", "last_assistant_message": limit_text().0}));
    session
}

#[test]
fn a_terminal_stopped_by_the_limit_gets_a_timer_at_the_time_its_message_names() {
    let (d, dir) = daemon("auto-continue-term");
    let session = limited_terminal(&d, "t1");
    assert!(session.info().limited);
    let timers = d.scheduled.list();
    assert_eq!(timers.len(), 1, "{timers:?}");
    let item = &timers[0];
    assert_eq!((item.session.as_str(), item.source.as_str(), item.agent_session_id.as_deref()), ("t1", "message", Some("conv-1")));
    assert_eq!((item.generation, item.turn), (session.generation(), session.turn_seq()));
    // To the minute the message names, plus the margin.
    let reset = limit_text().1;
    assert!(item.fire_at > reset - 61_000 && item.fire_at <= reset + crate::scheduled_input::RESET_MARGIN_MS, "{} vs {reset}", item.fire_at);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn with_the_setting_off_nothing_is_set_and_the_choice_survives_a_restart() {
    let (d, dir) = daemon("auto-continue-off");
    assert_eq!(call(&d, "daemon.policy", json!({})).unwrap()["auto_continue"], true, "on by default");
    let set = call(&d, "daemon.set_policy", json!({"auto_continue": false})).unwrap();
    assert_eq!(set["auto_continue"], false);
    let session = limited_terminal(&d, "t1");
    assert!(session.info().limited, "the banner still shows");
    assert!(d.scheduled.list().is_empty(), "no timer: the person decides");
    let again = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
    assert_eq!(call(&again, "daemon.policy", json!({})).unwrap()["auto_continue"], false, "kept in policy.json");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_limit_message_without_a_time_sets_nothing() {
    let (d, dir) = daemon("auto-continue-no-time");
    let (session, _typed) = fake::running(1, "t1", "claude");
    d.insert_for_test(Entry::Term(session.clone()));
    let hook = |payload: Value| d.hook(&json!({"session_id": "t1", "agent": "claude", "payload": payload})).unwrap();
    hook(json!({"hook_event_name": "UserPromptSubmit"}));
    hook(json!({"hook_event_name": "StopFailure", "error": "rate_limit", "last_assistant_message": "You've hit your limit"}));
    assert!(session.info().limited);
    assert!(d.scheduled.list().is_empty(), "never set on a guess (the app's usage meter may fill it in)");
    let _ = std::fs::remove_dir_all(dir);
}

/// A Claude chat whose turn ends on the usage limit, as `claude -p --output-format stream-json` prints it.
fn limited_chat(d: &Daemon, reset_ms: u64) -> (Arc<ChatSession>, tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) {
    let chat = ChatSession::new(2, SessionSpec { id: "c1".into(), kind: "claude".into(), cwd: ".".into(), ..Default::default() });
    let stdin = chat.live_for_test();
    d.insert_for_test(Entry::Chat(chat.clone()));
    chat.ingest_for_test(json!({"type": "system", "subtype": "init", "session_id": "conv-c"}).to_string());
    chat.ingest_for_test(json!({"type": "rate_limit_event", "rate_limit_info": {"status": "rejected", "resetsAt": reset_ms / 1000, "rateLimitType": "five_hour"}, "session_id": "conv-c"}).to_string());
    chat.ingest_for_test(
        json!({"type": "assistant", "error": "rate_limit", "session_id": "conv-c", "message": {"model": "<synthetic>", "role": "assistant", "content": [{"type": "text", "text": limit_text().0}]}}).to_string(),
    );
    chat.ingest_for_test(json!({"type": "result", "subtype": "success", "is_error": true, "result": limit_text().0, "session_id": "conv-c"}).to_string());
    (chat, stdin)
}

fn written_turns(stdin: &mut tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>) -> Vec<Value> {
    let mut out = Vec::new();
    while let Ok(bytes) = stdin.try_recv() {
        out.extend(bytes.split(|b| *b == b'\n').filter_map(|l| serde_json::from_slice::<Value>(l).ok()));
    }
    out
}

#[test]
fn a_chat_stopped_by_the_limit_is_continued_through_its_queue_once() {
    let (d, dir) = daemon("auto-continue-chat");
    let reset = limit_text().1;
    let (chat, mut stdin) = limited_chat(&d, reset);
    assert!(chat.info().limited && chat.info().failed);
    d.chat_limit(&chat);
    d.chat_limit(&chat);
    let timers = d.scheduled.list();
    assert_eq!(timers.len(), 1, "{timers:?}");
    assert_eq!((timers[0].source.as_str(), timers[0].agent_session_id.as_deref()), ("message", Some("conv-c")));
    // The event's own reset, to the second.
    assert_eq!(timers[0].fire_at, reset / 1000 * 1000 + crate::scheduled_input::RESET_MARGIN_MS);
    d.fire_scheduled(timers[0].clone());
    let turns = written_turns(&mut stdin);
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert_eq!(turns[0]["type"], "user");
    assert_eq!(turns[0].pointer("/message/content").map(|c| c.to_string().contains("continue")), Some(true), "{}", turns[0]);
    assert!(!chat.info().limited, "a message went: the limit is over as far as Kelpie can tell");
    // A second firing of the same timer finds the turn moved on.
    d.fire_scheduled(timers[0].clone());
    assert!(written_turns(&mut stdin).is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_chat_the_person_wrote_to_or_that_moved_on_is_never_continued() {
    let (d, dir) = daemon("auto-continue-chat-guards");
    let (chat, mut stdin) = limited_chat(&d, limit_text().1);
    let limit = chat.limited_now().expect("limited");
    // The person sends something first.
    chat.input(format!("{}\n", json!({"type": "user", "message": {"role": "user", "content": "hello"}})).into_bytes());
    let _ = written_turns(&mut stdin);
    assert!(!chat.info().limited);
    assert_eq!(chat.continue_if_limited(&limit, "continue"), Err(crate::session::Refused::InputSince));
    assert!(written_turns(&mut stdin).is_empty());
    // Another conversation, or another run of the process.
    let (chat, mut stdin) = limited_chat(&d, limit_text().1);
    let limit = chat.limited_now().unwrap();
    let other = crate::chat::ChatLimit { conversation: Some("conv-other".into()), ..limit.clone() };
    assert_eq!(chat.continue_if_limited(&other, "continue"), Err(crate::session::Refused::Conversation));
    let older = crate::chat::ChatLimit { run: limit.run + 1, ..limit.clone() };
    assert_eq!(chat.continue_if_limited(&older, "continue"), Err(crate::session::Refused::Restarted));
    assert!(written_turns(&mut stdin).is_empty());
    // The setting off: the chat shows the banner, and nothing is set.
    call(&d, "daemon.set_policy", json!({"auto_continue": false})).unwrap();
    let (chat, _stdin) = limited_chat(&d, limit_text().1);
    d.chat_limit(&chat);
    assert!(chat.info().limited && d.scheduled.list().is_empty());
    // The person sets one from the banner instead.
    call(&d, "session.schedule_continue", json!({"handle": 2, "at": now_millis() + 60_000, "source": "meter"})).unwrap();
    assert_eq!(d.scheduled.list().len(), 1);
    assert!(call(&d, "session.schedule_continue", json!({"handle": 2, "at": now_millis() + 60_000, "source": "message"})).is_err(), "only the daemon reads the message");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_cancel_sticks_for_that_stop_and_turning_the_setting_off_drops_what_kelpie_set() {
    let (d, dir) = daemon("auto-continue-cancel");
    let session = limited_terminal(&d, "t1");
    let handle = session.handle;
    assert_eq!(d.scheduled.list().len(), 1);
    assert_eq!(call(&d, "session.cancel_scheduled", json!({"handle": handle})).unwrap(), true);
    // The window comes back and its banner would fill in the meter's time by itself, or the agent reports the same stop again.
    let soon = now_millis() + 60_000;
    assert!(call(&d, "session.schedule_continue", json!({"handle": handle, "at": soon, "source": "meter-auto"})).is_err(), "cancelled for this stop");
    d.hook(&json!({"session_id": "t1", "agent": "claude", "payload": {"hook_event_name": "StopFailure", "session_id": "conv-1", "error": "rate_limit", "last_assistant_message": limit_text().0}})).unwrap();
    assert!(d.scheduled.list().is_empty(), "a repeated report does not set it again");
    // The person may still set one.
    call(&d, "session.schedule_continue", json!({"handle": handle, "at": soon, "source": "meter"})).unwrap();
    assert_eq!(d.scheduled.list()[0].source, "meter");
    // Turning the setting off drops only what Kelpie set by itself, and the banner's fill-in is refused.
    let (other, _typed) = fake::running(2, "t2", "claude");
    d.insert_for_test(Entry::Term(other.clone()));
    let hook = |payload: Value| d.hook(&json!({"session_id": "t2", "agent": "claude", "payload": payload})).unwrap();
    hook(json!({"hook_event_name": "UserPromptSubmit"}));
    hook(json!({"hook_event_name": "StopFailure", "error": "rate_limit", "last_assistant_message": limit_text().0}));
    assert_eq!(d.scheduled.list().len(), 2);
    call(&d, "daemon.set_policy", json!({"auto_continue": false})).unwrap();
    let left = d.scheduled.list();
    assert_eq!((left.len(), left[0].session.as_str()), (1, "t1"), "{left:?}");
    assert!(call(&d, "session.schedule_continue", json!({"handle": 2, "at": soon, "source": "meter-auto"})).is_err(), "off: nothing by itself");
    let _ = std::fs::remove_dir_all(dir);
}
