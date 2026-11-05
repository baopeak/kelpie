use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;

use super::*;
use crate::notify::destinations::Destinations;
use crate::notify::{Memory, Notifier, Raised, Telegram};

const TOKEN: &str = "123456:BOTtoken";
const ME: i64 = 42;

/// A local stand-in for the Bot API: what it was asked, and the updates it hands out.
#[derive(Default)]
struct Api {
    calls: Vec<(String, Value)>,
    updates: VecDeque<Value>,
    next_update: i64,
    next_message: i64,
    /// What `getUpdates` answers instead of updates.
    refuse: Option<u16>,
    /// A message with buttons is refused, as a malformed one would be.
    refuse_buttons: bool,
}

struct Fake {
    url: String,
    api: Arc<Mutex<Api>>,
}

impl Fake {
    fn start() -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let api = Arc::new(Mutex::new(Api { next_update: 1, next_message: 100, ..Api::default() }));
        let its = api.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let api = its.clone();
                std::thread::spawn(move || serve(stream, &api));
            }
        });
        Fake { url, api }
    }

    fn push(&self, kind: &str, body: Value) {
        let mut api = self.api.lock();
        let id = api.next_update;
        api.next_update += 1;
        api.updates.push_back(json!({"update_id": id, kind: body}));
    }

    fn calls(&self, method: &str) -> Vec<Value> {
        self.api.lock().calls.iter().filter(|(m, _)| m == method).map(|(_, b)| b.clone()).collect()
    }

    /// A press of a button of message `message`, by `from` in `chat`.
    fn press(&self, data: &str, from: i64, chat: i64, message: i64) {
        self.push("callback_query", json!({"id": format!("cb{}", uuid::Uuid::new_v4()), "from": {"id": from}, "data": data, "message": {"message_id": message, "chat": {"id": chat}}}));
    }

    /// The buttons of the last message sent with some, and its id.
    fn buttons(&self) -> Option<(String, String, i64)> {
        let sent = self.calls("sendMessage");
        let body = sent.iter().rev().find(|b| b.get("reply_markup").is_some())?;
        let row = &body["reply_markup"]["inline_keyboard"][0];
        let id = body["_message_id"].as_i64()?;
        Some((row[0]["callback_data"].as_str()?.to_string(), row[1]["callback_data"].as_str()?.to_string(), id))
    }
}

fn serve(mut stream: std::net::TcpStream, api: &Mutex<Api>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let body_at = loop {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..body_at]).to_string();
    let len: usize = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0))).unwrap_or(0);
    while buf.len() < body_at + len {
        let n = stream.read(&mut chunk).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
    let mut body: Value = serde_json::from_slice(&buf[body_at..(body_at + len).min(buf.len())]).unwrap_or(Value::Null);
    let method = path.rsplit('/').next().unwrap_or("").to_string();
    let (status, reply) = if !path.starts_with(&format!("/bot{TOKEN}/")) {
        (404, json!({"ok": false, "error_code": 404}))
    } else if method == "getUpdates" {
        let refuse = api.lock().refuse;
        match refuse {
            Some(code) => (code, json!({"ok": false, "error_code": code})),
            None => {
                // A short wait for something to say, as a long poll would.
                let mut waited = 0;
                loop {
                    let ready: Vec<Value> = api.lock().updates.drain(..).collect();
                    if !ready.is_empty() || waited >= 10 {
                        break (200, json!({"ok": true, "result": ready}));
                    }
                    waited += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    } else if method == "sendMessage" && body.get("reply_markup").is_some() && api.lock().refuse_buttons {
        (400, json!({"ok": false, "error_code": 400}))
    } else if method == "sendMessage" {
        let mut a = api.lock();
        a.next_message += 1;
        body["_message_id"] = json!(a.next_message);
        (200, json!({"ok": true, "result": {"message_id": a.next_message, "chat": {"id": body["chat_id"]}}}))
    } else {
        (200, json!({"ok": true, "result": true}))
    };
    api.lock().calls.push((method, body));
    let text = reply.to_string();
    let _ = stream.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len()).as_bytes());
}

/// What the daemon would offer and do, for sessions named here.
#[derive(Default)]
struct Sessions {
    offered: Mutex<HashMap<String, Approval>>,
    /// Sessions somebody else answered first, and who.
    taken: Mutex<HashMap<String, String>>,
    answers: Mutex<Vec<(String, bool)>>,
    audit: Mutex<Vec<(String, String)>>,
    changed: Mutex<Vec<String>>,
}

impl Sessions {
    fn offer(&self, session: &str, wait: u64) {
        let approval = Approval { session: session.into(), target: Target::Term { wait, yes: "1".into(), no: "3".into() }, command: "ls -la".into() };
        self.offered.lock().insert(session.into(), approval);
    }

    fn actions(&self) -> Vec<String> {
        self.audit.lock().iter().map(|(a, _)| a.clone()).collect()
    }
}

struct TestHost(Arc<Sessions>);

impl Host for TestHost {
    fn telegram_approval(&self, session: &str, wait: Option<u64>) -> Option<Approval> {
        self.0.offered.lock().get(session).filter(|a| a.wait() == wait || wait.is_none()).cloned()
    }
    fn telegram_answer(&self, approval: &Approval, allow: bool) -> Answered {
        if let Some(who) = self.0.taken.lock().get(&approval.session) {
            return Answered::Already(who.clone());
        }
        if self.0.offered.lock().remove(&approval.session).is_none() {
            return Answered::Gone;
        }
        self.0.taken.lock().insert(approval.session.clone(), "Telegram".into());
        self.0.answers.lock().push((approval.session.clone(), allow));
        Answered::Done
    }
    fn telegram_audit(&self, action: &str, summary: &str) {
        self.0.audit.lock().push((action.into(), summary.into()));
    }
    fn telegram_changed(&self, what: &str) {
        self.0.changed.lock().push(what.into());
    }
}

struct Rig {
    n: Notifier,
    fake: Fake,
    sessions: Arc<Sessions>,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.n.telegram_off("the test is over");
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Rig {
    fn approvals(&self) -> &Arc<Approvals> {
        self.n.destinations.telegram()
    }

    fn waiting(&self, session: &str, wait: u64) {
        let ev = Raised { session: Some(session.into()), wait: Some(wait), ..Raised::bare(Kind::Waiting) };
        self.n.send(&ev, &crate::notify::Context::default());
    }

    /// Binds this test's account in its own chat, as the person would.
    fn bound(self) -> Rig {
        let code = self.n.telegram_bind().unwrap()["code"].as_str().unwrap().to_string();
        self.fake.push("message", json!({"message_id": 1, "from": {"id": ME}, "chat": {"id": ME}, "text": format!("/bind {code}")}));
        until(|| self.n.config().telegram.approving(), "bound");
        self
    }
}

fn rig(mode: TelegramMode) -> Rig {
    let dir = std::env::temp_dir().join(format!("kelpie-telegram-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let secrets = Memory::new();
    let sessions = Arc::new(Sessions::default());
    let destinations = Destinations::new(&dir, secrets.clone(), Arc::new(TestHost(sessions.clone())));
    let fake = Fake::start();
    destinations.telegram().set_test_base(&fake.url);
    let n = Notifier::with_destinations(&dir, secrets, destinations);
    let mode = if mode == TelegramMode::ApproveLow { TelegramMode::AlertLink } else { mode };
    let c = Config { telegram: Telegram { enabled: true, chat_id: "-100777".into(), mode, user_id: String::new() }, ..Config::default() };
    n.update(Some(c), &json!({"telegram_token": TOKEN})).unwrap();
    Rig { n, fake, sessions, dir }
}

fn until(cond: impl Fn() -> bool, what: &str) {
    let began = Instant::now();
    while !cond() {
        assert!(began.elapsed() < Duration::from_secs(10), "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn alerts_alone_start_no_thread_and_make_no_request_of_their_own() {
    for mode in [TelegramMode::Alert, TelegramMode::AlertLink] {
        let r = rig(mode);
        r.sessions.offer("s1", 5);
        r.waiting("s1", 5);
        until(|| !r.fake.calls("sendMessage").is_empty(), "the alert");
        let sent = r.fake.calls("sendMessage");
        assert!(sent[0].get("reply_markup").is_none(), "no buttons: {}", sent[0]);
        assert!(!r.approvals().polling() && r.approvals().polls() == 0, "{mode:?}");
        assert!(r.fake.calls("getUpdates").is_empty());
    }
}

#[test]
fn a_binding_needs_its_code_and_binds_the_account_and_the_chat_it_came_from() {
    let r = rig(TelegramMode::AlertLink);
    let v = r.n.telegram_bind().unwrap();
    let code = v["code"].as_str().unwrap().to_string();
    assert_eq!(v["command"], format!("/bind {code}"));
    assert_eq!(v["expires_in"], 300);
    until(|| r.approvals().polls() > 0, "polling while a binding waits");
    r.fake.push("message", json!({"message_id": 1, "from": {"id": 7}, "chat": {"id": 7}, "text": "/bind WRONGCODE"}));
    r.fake.push("message", json!({"message_id": 2, "from": {"id": 7}, "chat": {"id": 7}, "text": "hello"}));
    until(|| r.fake.api.lock().updates.is_empty(), "read");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!r.n.config().telegram.approving(), "a wrong code binds nothing");
    r.fake.push("message", json!({"message_id": 3, "from": {"id": ME}, "chat": {"id": 555}, "text": format!("/bind@kelpie_bot {}", code.to_lowercase())}));
    until(|| r.n.config().telegram.approving(), "bound");
    let t = r.n.config().telegram;
    assert_eq!((t.user_id.as_str(), t.chat_id.as_str(), t.mode), ("42", "555", TelegramMode::ApproveLow));
    let saved: Config = serde_json::from_slice(&std::fs::read(r.dir.join("notify.json")).unwrap()).unwrap();
    assert_eq!(saved.telegram, t, "kept for the next start");
    until(|| r.fake.calls("sendMessage").iter().any(|b| b["chat_id"] == 555), "the chat is told");
    assert!(r.sessions.actions().contains(&"telegram_bound".to_string()));
    assert!(r.sessions.changed.lock().contains(&"bound".to_string()));
    // The code was for once.
    r.fake.push("message", json!({"message_id": 4, "from": {"id": 9}, "chat": {"id": 9}, "text": format!("/bind {code}")}));
    until(|| r.fake.api.lock().updates.is_empty(), "read");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.n.config().telegram.user_id, "42");
}

#[test]
fn only_the_bound_account_in_its_chat_answers_and_only_once() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "a message with buttons");
    let (yes, no, message) = r.fake.buttons().unwrap();
    assert!(yes.starts_with("a:") && no.starts_with("d:") && yes.len() == 34 && yes.len() <= 64, "{yes}");
    let sent = r.fake.calls("sendMessage").into_iter().find(|b| b.get("reply_markup").is_some()).unwrap();
    assert!(sent["text"].as_str().unwrap().contains("ls -la") && sent["disable_web_page_preview"] == true);
    // Not the account, not the chat, not a nonce it was given, not the message it came with.
    r.fake.press(&yes, 7, ME, message);
    r.fake.press(&yes, ME, 999, message);
    r.fake.press("a:00000000000000000000000000000000", ME, ME, message);
    r.fake.press(&yes, ME, ME, message + 1);
    r.fake.press("x:whatever", ME, ME, message);
    until(|| r.fake.api.lock().updates.is_empty(), "read");
    std::thread::sleep(Duration::from_millis(300));
    assert!(r.sessions.answers.lock().is_empty(), "nothing answered");
    // The one that counts, then the same press again.
    r.fake.press(&yes, ME, ME, message);
    until(|| r.sessions.answers.lock().len() == 1, "answered");
    r.fake.press(&yes, ME, ME, message);
    r.fake.press(&no, ME, ME, message);
    until(|| r.fake.api.lock().updates.is_empty(), "read");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(*r.sessions.answers.lock(), vec![("s1".to_string(), true)], "once");
    let actions = r.sessions.actions();
    assert_eq!(actions.iter().filter(|a| *a == "telegram_approve").count(), 1);
    assert!(actions.iter().filter(|a| *a == "telegram_refused").count() >= 3, "{actions:?}");
    let summary = r.sessions.audit.lock().iter().find(|(a, _)| a == "telegram_approve").unwrap().1.clone();
    assert!(summary.starts_with("s1: approved once") && !summary.contains("ls -la"), "{summary}");
    assert!(r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message && b.get("reply_markup").is_none()), "its buttons are gone");
    // Strangers are noted in the log once a minute, not per press.
    until(|| r.sessions.actions().contains(&"telegram_ignored".to_string()), "the strangers' note");
    assert_eq!(r.sessions.actions().iter().filter(|a| *a == "telegram_ignored").count(), 1);
}

#[test]
fn a_press_after_somebody_else_answered_is_told_so_and_answers_nothing() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (yes, _, message) = r.fake.buttons().unwrap();
    r.sessions.taken.lock().insert("s1".into(), "desktop".into());
    r.fake.press(&yes, ME, ME, message);
    until(|| r.fake.calls("answerCallbackQuery").iter().any(|b| b["text"].as_str().is_some_and(|t| t.contains("on the computer"))), "told");
    assert!(r.sessions.answers.lock().is_empty());
}

#[test]
fn what_the_daemon_does_not_offer_has_no_buttons() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.waiting("dangerous", 5);
    until(|| r.fake.calls("sendMessage").iter().any(|b| b["chat_id"] == "42"), "the alert");
    assert!(r.fake.buttons().is_none());
    // A wait other than the one offered has none either.
    r.sessions.offer("s1", 5);
    r.waiting("s1", 6);
    until(|| r.fake.calls("sendMessage").iter().filter(|b| b["chat_id"] == "42").count() == 2, "the second alert");
    assert!(r.fake.buttons().is_none());
}

#[test]
fn six_approvals_a_minute_then_alerts_without_buttons() {
    let r = rig(TelegramMode::ApproveLow).bound();
    for i in 0..6 {
        let s = format!("s{i}");
        r.sessions.offer(&s, 5);
        r.waiting(&s, 5);
        until(|| r.fake.calls("sendMessage").iter().filter(|b| b.get("reply_markup").is_some()).count() == i + 1, "buttons");
        let (yes, _, message) = r.fake.buttons().unwrap();
        r.fake.press(&yes, ME, ME, message);
        until(|| r.sessions.answers.lock().len() == i + 1, "answered");
    }
    r.sessions.offer("s6", 5);
    r.waiting("s6", 5);
    until(|| r.fake.calls("sendMessage").iter().filter(|b| b["chat_id"] == "42").count() == 7, "the seventh");
    assert_eq!(r.fake.calls("sendMessage").iter().filter(|b| b.get("reply_markup").is_some()).count(), 6, "alert only");
}

#[test]
fn a_button_expires_and_loses_its_buttons() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (yes, _, message) = r.fake.buttons().unwrap();
    r.approvals().age_offers(OFFER_FOR);
    until(|| r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message && b["text"].as_str().is_some_and(|t| t.contains("Expired"))), "stripped");
    r.fake.press(&yes, ME, ME, message);
    until(|| !r.fake.calls("answerCallbackQuery").is_empty(), "told");
    assert!(r.sessions.answers.lock().is_empty());
}

#[test]
fn a_request_answered_elsewhere_loses_its_buttons() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (_, _, message) = r.fake.buttons().unwrap();
    r.sessions.offered.lock().clear();
    until(|| r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message && b["text"].as_str().is_some_and(|t| t.contains("Answered elsewhere"))), "stripped");
    assert_eq!(r.approvals().describe()["open"], 0);
}

#[test]
fn the_kill_switch_stops_polling_burns_the_buttons_and_stays_off() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (yes, _, message) = r.fake.buttons().unwrap();
    r.n.telegram_off("every phone was revoked");
    assert!(!r.n.config().telegram.approving());
    assert_eq!(r.n.config().telegram.mode, TelegramMode::AlertLink);
    assert!(r.n.config().telegram.user_id.is_empty());
    let saved: Config = serde_json::from_slice(&std::fs::read(r.dir.join("notify.json")).unwrap()).unwrap();
    assert!(saved.telegram.user_id.is_empty() && saved.telegram.mode == TelegramMode::AlertLink, "and it holds after a restart");
    until(|| !r.approvals().polling(), "polling stopped");
    assert_eq!(r.approvals().describe()["open"], 0, "every nonce burnt");
    until(|| r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message), "buttons removed");
    assert!(r.sessions.actions().contains(&"telegram_off".to_string()));
    // A press now is heard by nobody.
    let polls = r.approvals().polls();
    r.fake.press(&yes, ME, ME, message);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.approvals().polls(), polls);
    assert!(r.sessions.answers.lock().is_empty());
}

#[test]
fn polling_stops_when_the_mode_changes() {
    let r = rig(TelegramMode::ApproveLow).bound();
    assert!(r.approvals().polling());
    let mut c = r.n.config();
    c.telegram.mode = TelegramMode::AlertLink;
    r.n.update(Some(c), &Value::Null).unwrap();
    until(|| !r.approvals().polling(), "stopped");
    assert!(r.n.config().telegram.user_id.is_empty(), "the binding goes with it");
    let polls = r.approvals().polls();
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.approvals().polls(), polls, "no request after it stopped");
}

#[test]
fn the_app_cannot_turn_approvals_on_nor_name_the_account() {
    let r = rig(TelegramMode::AlertLink);
    let mut c = r.n.config();
    c.telegram.mode = TelegramMode::ApproveLow;
    c.telegram.user_id = "7".into();
    assert!(r.n.update(Some(c), &Value::Null).is_err());
    assert!(!r.approvals().polling());
    let r = r.bound();
    // A setting that only resends what is there keeps the binding, whatever account it names.
    let mut c = r.n.config();
    c.telegram.user_id = "7".into();
    r.n.update(Some(c), &Value::Null).unwrap();
    assert_eq!(r.n.config().telegram.user_id, "42");
    assert!(r.n.config().telegram.approving());
    // Another chat, or another bot, ends it.
    let mut c = r.n.config();
    c.telegram.chat_id = "-100999".into();
    r.n.update(Some(c), &Value::Null).unwrap();
    assert!(!r.n.config().telegram.approving());
    let r = r.bound();
    r.n.update(None, &json!({"telegram_token": "999:OTHERbot"})).unwrap();
    assert!(!r.n.config().telegram.approving(), "a new bot token");
    assert_eq!(r.n.config().telegram.mode, TelegramMode::AlertLink);
}

#[test]
fn a_rejected_token_stops_polling_and_says_so_without_the_token_or_the_address() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.fake.api.lock().refuse = Some(401);
    until(|| !r.approvals().polling(), "stopped");
    let shown = r.n.describe(false).to_string();
    let error = r.approvals().describe()["error"].as_str().unwrap_or_default().to_string();
    assert!(error.contains("rejected the bot token"), "{error}");
    assert!(!r.n.config().telegram.approving());
    assert!(!shown.contains("BOTtoken") && !shown.contains("127.0.0.1"), "{shown}");
}

#[test]
fn another_poller_or_a_webhook_is_shown_and_polling_goes_on() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.fake.api.lock().refuse = Some(409);
    until(|| r.approvals().describe()["error"].as_str().is_some_and(|e| e.contains("webhook")), "shown");
    assert!(r.approvals().polling() && r.n.config().telegram.approving());
    r.fake.api.lock().refuse = None;
    until(|| r.approvals().describe()["error"].is_null(), "cleared once it works again");
}

#[test]
fn backoff_is_full_jitter_up_to_a_minute_and_honours_what_telegram_asks() {
    for failures in 1..20 {
        let cap = Duration::from_secs((1u64 << (failures - 1).min(6)).min(60));
        for _ in 0..20 {
            assert!(backoff(failures, None) <= cap, "{failures}");
        }
    }
    assert!(backoff(1, Some(Duration::from_secs(7))) >= Duration::from_secs(7));
    assert!(backoff(1, Some(Duration::from_secs(7200))) <= Duration::from_secs(60));
}

#[test]
fn a_message_telegram_refuses_with_buttons_goes_as_the_alert_it_always_was() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.fake.api.lock().refuse_buttons = true;
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.calls("sendMessage").iter().any(|b| b["chat_id"] == "42" && b.get("reply_markup").is_none()), "the plain alert");
    assert_eq!(r.approvals().describe()["open"], 0, "no button is left open");
}