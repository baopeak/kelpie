use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::TcpListener;

use super::*;
use crate::notify::destinations::Destinations;
use crate::notify::{Memory, Notifier, Raised, Telegram};

const TOKEN: &str = "123456:BOTtoken";
const ME: i64 = 42;
/// The chat set for Telegram in these tests: a group.
const CHAT: i64 = -100777;

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
    /// What the last `getUpdates` asked for: as Telegram does, an update of another kind made meanwhile is never kept.
    allowed: Option<Vec<String>>,
    /// Updates dropped for that.
    dropped: usize,
    /// `answerCallbackQuery` takes this long.
    slow_answers: Option<Duration>,
    /// A message to any chat but the one set for Telegram takes this long.
    slow_strangers: Option<Duration>,
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
        if api.allowed.as_ref().is_some_and(|a| !a.iter().any(|k| k == kind)) {
            api.dropped += 1;
            return;
        }
        let id = api.next_update;
        api.next_update += 1;
        api.updates.push_back(json!({"update_id": id, kind: body}));
    }

    fn calls(&self, method: &str) -> Vec<Value> {
        self.api.lock().calls.iter().filter(|(m, _)| m == method).map(|(_, b)| b.clone()).collect()
    }

    /// A press of a button of message `message`, by `from` in `chat`.
    fn press(&self, data: &str, from: i64, chat: i64, message: i64) {
        self.push("callback_query", press(data, from, chat, message));
    }

    /// The buttons of every message sent with some, and its id, oldest first.
    fn all_buttons(&self) -> Vec<(String, String, i64)> {
        self.calls("sendMessage")
            .iter()
            .filter(|b| b.get("reply_markup").is_some())
            .filter_map(|body| {
                let row = &body["reply_markup"]["inline_keyboard"][0];
                Some((row[0]["callback_data"].as_str()?.to_string(), row[1]["callback_data"].as_str()?.to_string(), body["_message_id"].as_i64()?))
            })
            .collect()
    }

    /// The buttons of the last message sent with some, and its id.
    fn buttons(&self) -> Option<(String, String, i64)> {
        self.all_buttons().pop()
    }

    /// What the bot said to `chat` on its own (`reply`), not alerts.
    fn said_to(&self, chat: i64) -> Vec<String> {
        self.calls("sendMessage").iter().filter(|b| b["chat_id"] == chat).filter_map(|b| b["text"].as_str().map(str::to_string)).collect()
    }

    fn told(&self, words: &str) -> bool {
        self.calls("answerCallbackQuery").iter().any(|b| b["text"].as_str().is_some_and(|t| t.contains(words)))
    }

    fn read(&self) {
        until(|| self.api.lock().updates.is_empty(), "read");
        std::thread::sleep(Duration::from_millis(300));
    }
}

fn press(data: &str, from: i64, chat: i64, message: i64) -> Value {
    json!({"id": format!("cb{}", uuid::Uuid::new_v4()), "from": {"id": from}, "data": data, "message": {"message_id": message, "chat": {"id": chat}}})
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
        let refuse = {
            let mut a = api.lock();
            if let Some(kinds) = body["allowed_updates"].as_array() {
                a.allowed = Some(kinds.iter().filter_map(|k| k.as_str().map(str::to_string)).collect());
            }
            a.refuse
        };
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
    } else if method == "getMe" {
        (200, json!({"ok": true, "result": {"id": 123456, "is_bot": true, "username": "kelpie_bot"}}))
    } else if method == "sendMessage" && body.get("reply_markup").is_some() && api.lock().refuse_buttons {
        (400, json!({"ok": false, "error_code": 400}))
    } else if method == "sendMessage" {
        let slow = api.lock().slow_strangers.filter(|_| body["chat_id"] != CHAT);
        if let Some(d) = slow {
            std::thread::sleep(d);
        }
        let mut a = api.lock();
        a.next_message += 1;
        body["_message_id"] = json!(a.next_message);
        (200, json!({"ok": true, "result": {"message_id": a.next_message, "chat": {"id": body["chat_id"]}}}))
    } else {
        let slow = if method == "answerCallbackQuery" { api.lock().slow_answers } else { None };
        if let Some(d) = slow {
            std::thread::sleep(d);
        }
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
    /// Sessions somebody else answered first, and who, by id.
    taken: Mutex<HashMap<String, String>>,
    answers: Mutex<Vec<(String, bool)>>,
    audit: Mutex<Vec<(String, String)>>,
    changed: Mutex<Vec<String>>,
    /// Runs once inside the next answer, after its wait and before `go` is asked.
    before_go: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Sessions {
    fn offer(&self, session: &str, wait: u64) {
        let approval = Approval { session: session.into(), target: Target::Term { wait, yes: "1".into(), no: "3".into() }, command: "ls -la".into() };
        self.offered.lock().insert(session.into(), approval);
    }

    fn actions(&self) -> Vec<String> {
        self.audit.lock().iter().map(|(a, _)| a.clone()).collect()
    }

    fn noted(&self, action: &str) -> Vec<String> {
        self.audit.lock().iter().filter(|(a, _)| a == action).map(|(_, s)| s.clone()).collect()
    }
}

struct TestHost(Arc<Sessions>);

impl Host for TestHost {
    fn telegram_approval(&self, session: &str, wait: Option<u64>) -> Option<Approval> {
        self.0.offered.lock().get(session).filter(|a| a.wait() == wait || wait.is_none()).cloned()
    }
    fn telegram_answer(&self, approval: &Approval, allow: bool, go: &dyn Fn() -> bool) -> Answered {
        if let Some(who) = self.0.taken.lock().get(&approval.session) {
            return Answered::Already(who.clone());
        }
        if !self.0.offered.lock().contains_key(&approval.session) {
            return Answered::Gone;
        }
        let before = self.0.before_go.lock().take();
        if let Some(before) = before {
            before();
        }
        if !go() {
            return Answered::Off;
        }
        self.0.offered.lock().remove(&approval.session);
        self.0.taken.lock().insert(approval.session.clone(), "telegram".into());
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

    fn code(&self) -> String {
        self.n.telegram_bind().unwrap()["code"].as_str().unwrap().to_string()
    }

    /// Binds this test's account in the chat set, as the person would.
    fn bound(self) -> Rig {
        let code = self.code();
        let chat: i64 = self.n.config().telegram.chat_id.parse().unwrap();
        self.fake.push("message", json!({"message_id": 1, "from": {"id": ME, "username": "me_here", "first_name": "Me"}, "chat": {"id": chat}, "text": format!("/bind {code}")}));
        until(|| self.n.config().telegram.approving(), "bound");
        self
    }

    /// The alerts sent to the chat set (the bot's own replies go by number).
    fn alerts(&self) -> Vec<Value> {
        let chat = self.n.config().telegram.chat_id;
        self.fake.calls("sendMessage").into_iter().filter(|b| b["chat_id"] == chat.as_str()).collect()
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
    let c = Config { telegram: Telegram { enabled: true, chat_id: CHAT.to_string(), mode, ..Telegram::default() }, ..Config::default() };
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
fn a_binding_needs_its_code_from_the_chat_set_and_binds_the_account_that_sent_it() {
    let r = rig(TelegramMode::AlertLink);
    let v = r.n.telegram_bind().unwrap();
    let code = v["code"].as_str().unwrap().to_string();
    assert_eq!(v["command"], format!("/bind {code}"));
    assert_eq!(v["expires_in"], 300);
    until(|| r.approvals().polls() > 0, "polling while a binding waits");
    // Another chat, even with the code: refused and told where to send it. Its wrong codes do not use the binding up.
    for i in 0..MAX_WRONG {
        r.fake.push("message", json!({"message_id": i, "from": {"id": 7}, "chat": {"id": 555}, "text": "/bind WRONGCODE"}));
    }
    r.fake.push("message", json!({"message_id": 9, "from": {"id": ME}, "chat": {"id": 555}, "text": format!("/bind {code}")}));
    // The group's anonymous admins: the group speaking, not an account.
    r.fake.push("message", json!({"message_id": 10, "from": {"id": ANONYMOUS_ADMIN}, "sender_chat": {"id": CHAT}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}));
    r.fake.push("message", json!({"message_id": 11, "from": {"id": 8}, "sender_chat": {"id": CHAT}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}));
    r.fake.read();
    assert!(!r.n.config().telegram.approving(), "nothing bound");
    assert!(r.approvals().describe()["binding_expires_in"].as_u64().is_some(), "the binding still waits");
    // Each chat is answered once a minute at most, however many codes it sends.
    until(|| r.fake.said_to(555).len() == 1 && r.fake.said_to(CHAT).len() == 1, "one answer for each chat");
    assert!(r.fake.said_to(555)[0].contains("only in the chat set") && r.fake.said_to(CHAT)[0].contains("as yourself"), "{:?}", r.fake.said_to(CHAT));
    // A wrong code in the chat set binds nothing, and says so; a command for another bot is that bot's, and no business of this one.
    r.approvals().forget_replies();
    r.fake.push("message", json!({"message_id": 12, "from": {"id": ME}, "chat": {"id": CHAT}, "text": format!("/bind@other_bot {code}")}));
    r.fake.push("message", json!({"message_id": 13, "from": {"id": 7}, "chat": {"id": CHAT}, "text": "/bind WRONGCODE"}));
    r.fake.push("message", json!({"message_id": 14, "from": {"id": 7}, "chat": {"id": CHAT}, "text": "hello"}));
    r.fake.read();
    assert!(!r.n.config().telegram.approving(), "a wrong code binds nothing, nor a code sent to another bot");
    until(|| r.fake.said_to(CHAT).len() == 2, "the wrong code is answered");
    assert!(r.fake.said_to(CHAT)[1].contains("not the code Kelpie shows"), "{:?}", r.fake.said_to(CHAT));
    assert_eq!(r.approvals().state.lock().binding.as_ref().map(|b| b.wrong), Some(1), "another bot's command is no wrong code");
    // The code, in the chat set, binds the account that sent it, and keeps its name to show.
    r.approvals().forget_replies();
    r.fake.push("message", json!({"message_id": 15, "from": {"id": ME, "first_name": "Bao\u{202e}\u{7}"}, "chat": {"id": CHAT}, "text": format!("/bind@Kelpie_Bot {}", code.to_lowercase())}));
    until(|| r.n.config().telegram.approving(), "bound");
    let t = r.n.config().telegram;
    assert_eq!((t.user_id.as_str(), t.user_name.as_str(), t.chat_id.as_str(), t.mode), ("42", "Bao", "-100777", TelegramMode::ApproveLow));
    let saved: Config = serde_json::from_slice(&std::fs::read(r.dir.join("notify.json")).unwrap()).unwrap();
    assert_eq!(saved.telegram, t, "kept for the next start");
    until(|| r.fake.said_to(CHAT).iter().any(|t| t.contains("can now approve")), "the chat is told");
    assert!(r.sessions.noted("telegram_bound").iter().any(|s| s.contains("Bao (42)") && s.contains("-100777")), "{:?}", r.sessions.noted("telegram_bound"));
    assert!(r.sessions.changed.lock().contains(&"bound".to_string()));
    // The code was for once, and a late one is told so.
    r.approvals().forget_replies();
    r.fake.push("message", json!({"message_id": 16, "from": {"id": 9}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}));
    until(|| r.fake.said_to(CHAT).iter().any(|t| t.contains("No code is waiting")), "told the code was used");
    assert_eq!(r.n.config().telegram.user_id, "42");
}

#[test]
fn five_wrong_codes_from_the_chat_set_end_a_binding() {
    let r = rig(TelegramMode::AlertLink);
    r.code();
    until(|| r.approvals().polls() > 0, "polling");
    for i in 0..MAX_WRONG {
        r.fake.push("message", json!({"message_id": i, "from": {"id": 7}, "chat": {"id": CHAT}, "text": "/bind WRONGCODE"}));
    }
    until(|| r.sessions.changed.lock().contains(&"bind_failed".to_string()), "ended");
    assert!(r.approvals().describe()["binding_expires_in"].is_null());
    until(|| r.fake.said_to(CHAT).iter().any(|t| t.contains("Too many wrong codes")), "told");
    assert!(r.sessions.actions().contains(&"telegram_bind_failed".to_string()));
}

#[test]
fn a_binding_ends_when_the_person_chooses_otherwise() {
    for change in ["alert_link", "alert", "off", "chat", "token"] {
        let r = rig(TelegramMode::AlertLink);
        let code = r.code();
        until(|| r.approvals().polls() > 0, "polling");
        let mut c = r.n.config();
        match change {
            "alert_link" => c.telegram.mode = TelegramMode::AlertLink,
            "alert" => c.telegram.mode = TelegramMode::Alert,
            "off" => c.telegram.enabled = false,
            "chat" => c.telegram.chat_id = "-100999".into(),
            _ => {}
        }
        match change {
            "token" => r.n.update(None, &json!({"telegram_token": "999:OTHERbot"})).unwrap(),
            // Choosing a mode is said so by the page (`end_binding`), even the one already saved.
            "alert_link" | "alert" => r.n.update_choosing(Some(c), &Value::Null, true).unwrap(),
            _ => r.n.update(Some(c), &Value::Null).unwrap(),
        }
        assert!(r.approvals().describe()["binding_expires_in"].is_null(), "{change}");
        assert!(r.sessions.changed.lock().contains(&"bind_cancelled".to_string()), "{change}: the page is told");
        // The code arriving now, from the chat set, binds nothing.
        let chat: i64 = r.n.config().telegram.chat_id.parse().unwrap();
        r.approvals().handle(&json!({"update_id": 99, "message": {"message_id": 1, "from": {"id": ME}, "chat": {"id": chat}, "text": format!("/bind {code}")}}));
        let t = r.n.config().telegram;
        assert!(!t.approving() && t.user_id.is_empty(), "{change}: {t:?}");
        until(|| !r.approvals().polling(), "polling stops");
    }
    // Something other than Telegram changing, with approvals on, leaves them be.
    let r = rig(TelegramMode::ApproveLow).bound();
    let mut c = r.n.config();
    c.min_turn_s = 5;
    r.n.update(Some(c), &Value::Null).unwrap();
    assert!(r.n.config().telegram.approving());
}

#[test]
fn a_save_of_anything_else_while_a_code_is_shown_leaves_the_code_be() {
    let r = rig(TelegramMode::AlertLink);
    let code = r.code();
    until(|| r.approvals().polls() > 0, "polling");
    // What the page and Settings → Notifications save on each change: ntfy switched on, an event, the shortest turn.
    let mut c = r.n.config();
    c.ntfy.enabled = true;
    c.min_turn_s = 5;
    r.n.update(Some(c), &Value::Null).unwrap();
    // As the page sends a mode change with nothing to say about a code.
    r.n.update(Some(r.n.config()), &Value::Null).unwrap();
    assert!(r.approvals().describe()["binding_expires_in"].as_u64().is_some(), "the code still waits");
    assert!(!r.sessions.changed.lock().contains(&"bind_cancelled".to_string()));
    r.fake.push("message", json!({"message_id": 1, "from": {"id": ME, "username": "me_here"}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}));
    until(|| r.n.config().telegram.approving(), "bound after all");
    assert!(r.n.config().ntfy.enabled, "and the other change stands");
}

#[test]
fn a_code_read_while_the_person_chose_otherwise_saves_nothing() {
    for change in ["cancelled", "mode"] {
        let r = rig(TelegramMode::AlertLink);
        let code = r.code();
        // The settings are being saved when the code arrives: it is read, then waits to be saved.
        let saving = r.approvals().saving();
        let approvals = r.approvals().clone();
        let reading = std::thread::spawn(move || approvals.handle(&json!({"update_id": 1, "message": {"message_id": 1, "from": {"id": ME}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}})));
        until(|| r.approvals().describe()["binding_expires_in"].is_null(), "the code read");
        // What that save does meanwhile: ends the code (the person chose another mode), or sets another mode without saying so (an older app).
        match change {
            "cancelled" => r.approvals().cancel_binding(),
            _ => r.approvals().config.lock().telegram.mode = TelegramMode::Alert,
        }
        drop(saving);
        reading.join().unwrap();
        let t = r.n.config().telegram;
        assert!(!t.approving() && t.user_id.is_empty() && t.mode != TelegramMode::ApproveLow, "{change}: {t:?}");
        assert!(r.fake.said_to(CHAT).iter().any(|t| t.starts_with("Nothing was bound")), "{change}: the chat is told");
        assert!(r.sessions.changed.lock().contains(&"bind_failed".to_string()), "{change}: the page drops the code");
        assert!(r.sessions.noted("telegram_bind_failed").iter().any(|s| s.contains("settings changed")), "{change}");
    }
}

#[test]
fn a_code_read_right_after_the_kill_switch_binds_nothing() {
    let r = rig(TelegramMode::AlertLink);
    let code = r.code();
    // The code arrives the moment the kill switch has saved the settings and let them go, before it is done.
    let approvals = Arc::downgrade(r.approvals());
    *r.approvals().after_off_saved.lock() = Some(Box::new(move || {
        if let Some(a) = approvals.upgrade() {
            a.handle(&json!({"update_id": 1, "message": {"message_id": 1, "from": {"id": ME}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}}));
        }
    }));
    r.n.telegram_off("every phone was revoked");
    let t = r.n.config().telegram;
    assert!(!t.approving() && t.user_id.is_empty() && t.mode == TelegramMode::AlertLink, "{t:?}");
    let saved: Config = serde_json::from_slice(&std::fs::read(r.dir.join("notify.json")).unwrap()).unwrap();
    assert!(saved.telegram.user_id.is_empty() && saved.telegram.mode != TelegramMode::ApproveLow, "{:?}", saved.telegram);
    assert!(r.approvals().describe()["binding_expires_in"].is_null());
    until(|| r.fake.said_to(CHAT).iter().any(|t| t.contains("No code is waiting")), "the chat is told");
}

#[test]
fn a_reply_left_out_does_not_mute_its_chat_and_strangers_cannot_crowd_out_the_chat_set() {
    let r = rig(TelegramMode::AlertLink);
    r.code();
    until(|| r.approvals().polls() > 0, "polling");
    // Telegram is slow to take what is said to other chats, and more of them send `/bind` than replies may be under way at once.
    r.fake.api.lock().slow_strangers = Some(Duration::from_secs(2));
    let strangers: Vec<i64> = (1..=ASIDE_MAX as i64 + 2).map(|i| 500 + i).collect();
    for (i, &chat) in strangers.iter().enumerate() {
        r.fake.push("message", json!({"message_id": i, "from": {"id": chat}, "chat": {"id": chat}, "text": "/bind WRONGCODE"}));
    }
    r.fake.push("message", json!({"message_id": 99, "from": {"id": ME}, "chat": {"id": CHAT}, "text": "/bind WRONGCODE"}));
    // The chat set is answered at once, while the strangers' replies are still under way.
    until(|| r.fake.said_to(CHAT).len() == 1, "the chat set is answered");
    assert!(r.fake.said_to(CHAT)[0].contains("not the code Kelpie shows"), "{:?}", r.fake.said_to(CHAT));
    assert!(strangers.iter().all(|&c| r.fake.said_to(c).is_empty()), "ahead of the strangers");
    until(|| r.approvals().aside_calls.load(Ordering::SeqCst) == 0, "the replies under way are done");
    let answered = strangers.iter().filter(|&&c| !r.fake.said_to(c).is_empty()).count();
    assert_eq!(answered, ASIDE_MAX - 1, "the strangers leave a reply free for the chat set");
    // A chat whose reply was left out is not muted for it: its next `/bind` is answered, and a chat answered already is not, within the minute.
    for (i, &chat) in strangers.iter().enumerate() {
        r.fake.push("message", json!({"message_id": 100 + i, "from": {"id": chat}, "chat": {"id": chat}, "text": "/bind WRONGCODE"}));
    }
    until(|| strangers.iter().all(|&c| r.fake.said_to(c).len() == 1), "each stranger answered once");
    until(|| r.approvals().aside_calls.load(Ordering::SeqCst) == 0, "done");
    assert!(strangers.iter().all(|&c| r.fake.said_to(c).len() == 1));
}

#[test]
fn a_bot_name_counts_only_for_the_token_it_was_learnt_with() {
    let r = rig(TelegramMode::AlertLink);
    r.code();
    until(|| r.approvals().state.lock().bot.is_some(), "the name learnt");
    assert!(r.approvals().is_bind("/bind@Kelpie_Bot") && !r.approvals().is_bind("/bind@other_bot"));
    // Another bot: Telegram does not answer for it here, so its name is not known, and the old one's is no longer this bot's.
    r.n.update(None, &json!({"telegram_token": "999:OTHERbot"})).unwrap();
    assert!(!r.approvals().is_bind("/bind@kelpie_bot") && r.approvals().is_bind("/bind"));
    // Nor when the old token's name is kept after the change, as a lookup on its way meanwhile would.
    r.approvals().state.lock().bot = Some((token_key(TOKEN), "kelpie_bot".into()));
    assert!(!r.approvals().is_bind("/bind@kelpie_bot"));
    r.approvals().state.lock().bot = Some((token_key("999:OTHERbot"), "other_bot".into()));
    assert!(r.approvals().is_bind("/bind@other_bot"), "the name learnt for this token");
}

#[test]
fn polls_ask_for_a_bounded_batch_and_always_for_chat_messages() {
    let r = rig(TelegramMode::AlertLink);
    r.approvals().bind_for_test(ME, CHAT);
    until(|| r.fake.calls("getUpdates").len() >= 2, "polls");
    // Telegram keeps only what the last poll asked for: a code sent while only presses were asked for would never arrive.
    for body in r.fake.calls("getUpdates") {
        assert_eq!((body["limit"].clone(), body["allowed_updates"].clone()), (json!(UPDATES_PER_POLL), json!(["message", "callback_query"])), "{body}");
    }
    // A server that answers at once with nothing is not asked again at the speed of the network.
    let polls = r.approvals().polls();
    std::thread::sleep(Duration::from_millis(2500));
    let more = r.approvals().polls() - polls;
    assert!((1..=4).contains(&more), "{more} polls in 2.5 s");
    // A code sent at once, while approvals were on and polling went on, arrives.
    let code = r.code();
    r.fake.push("message", json!({"message_id": 1, "from": {"id": ME, "username": "me_here"}, "chat": {"id": CHAT}, "text": format!("/bind {code}")}));
    until(|| r.fake.said_to(CHAT).iter().any(|t| t.contains("can now approve")), "bound again");
    assert_eq!(r.fake.api.lock().dropped, 0);
}

#[test]
fn a_first_name_cannot_pass_for_a_username() {
    let name = |from: Value| binder_name(&json!({"from": from}));
    assert_eq!(name(json!({"username": "alice_real", "first_name": "Alice"})), "@alice_real");
    assert_eq!(name(json!({"first_name": "@alice_real"})), "alice_real");
    assert_eq!(name(json!({"first_name": " \u{200b}@ ＠@alice"})), "alice");
    assert_eq!(name(json!({"username": "", "first_name": "Alice"})), "Alice");
    assert_eq!(name(json!({"id": 1})), "");
}

#[test]
fn a_press_on_old_buttons_does_not_hold_up_the_polling() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (yes, _, message) = r.fake.buttons().unwrap();
    // Telegram is slow to take the note each press gets: ten presses of an old button, then the real one, in one batch.
    r.fake.api.lock().slow_answers = Some(Duration::from_secs(3));
    for _ in 0..10 {
        r.fake.press("a:00000000000000000000000000000000", ME, CHAT, message);
    }
    let began = Instant::now();
    r.fake.press(&yes, ME, CHAT, message);
    until(|| r.sessions.answers.lock().len() == 1, "answered");
    assert!(began.elapsed() < Duration::from_secs(3), "answered after {:?}", began.elapsed());
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
    r.fake.press(&yes, 7, CHAT, message);
    r.fake.press(&yes, ME, 999, message);
    r.fake.press("a:00000000000000000000000000000000", ME, CHAT, message);
    r.fake.press(&yes, ME, CHAT, message + 1);
    r.fake.press("x:whatever", ME, CHAT, message);
    r.fake.read();
    assert!(r.sessions.answers.lock().is_empty(), "nothing answered");
    // The one that counts, then the same press again.
    r.fake.press(&yes, ME, CHAT, message);
    until(|| r.sessions.answers.lock().len() == 1, "answered");
    r.fake.press(&yes, ME, CHAT, message);
    r.fake.press(&no, ME, CHAT, message);
    r.fake.read();
    assert_eq!(*r.sessions.answers.lock(), vec![("s1".to_string(), true)], "once");
    assert_eq!(r.sessions.noted("telegram_approve").len(), 1);
    let summary = r.sessions.noted("telegram_approve")[0].clone();
    assert!(summary.starts_with("s1: approved once") && !summary.contains("ls -la"), "{summary}");
    assert!(r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message && b.get("reply_markup").is_none()), "its buttons are gone");
    // Strangers, and buttons no longer valid, are noted in the log once a minute with a count, not per press.
    until(|| r.sessions.actions().contains(&"telegram_ignored".to_string()), "the strangers' note");
    until(|| !r.sessions.noted("telegram_refused").is_empty(), "the stale buttons' note");
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(r.sessions.noted("telegram_ignored").len(), 1);
    let refused = r.sessions.noted("telegram_refused");
    assert!(refused.len() == 1 && refused[0].contains("presses of buttons that are no longer valid"), "{refused:?}");
    assert!(r.fake.told("no longer valid"), "each press is still told");
}

#[test]
fn deny_first_answers_no_once() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (yes, no, message) = r.fake.buttons().unwrap();
    r.fake.press(&no, ME, CHAT, message);
    until(|| r.sessions.answers.lock().len() == 1, "answered");
    assert_eq!(*r.sessions.answers.lock(), vec![("s1".to_string(), false)], "the no");
    until(|| r.fake.told("Denied from Telegram."), "told");
    until(|| r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message && b["text"].as_str().is_some_and(|t| t.ends_with("Denied from Telegram."))), "stripped");
    assert!(r.sessions.noted("telegram_deny").iter().any(|s| s.starts_with("s1: denied")));
    r.fake.press(&yes, ME, CHAT, message);
    r.fake.read();
    assert_eq!(r.sessions.answers.lock().len(), 1, "and nothing after it");
}

#[test]
fn a_press_after_somebody_else_answered_is_told_so_and_answers_nothing() {
    let r = rig(TelegramMode::ApproveLow).bound();
    for (session, who, said) in [("s1", "desktop", "on the computer"), ("s2", "phone-4f2a", "on a paired phone")] {
        r.sessions.offer(session, 5);
        r.waiting(session, 5);
        until(|| r.fake.all_buttons().len() == if session == "s1" { 1 } else { 2 }, "buttons");
        let (yes, _, message) = r.fake.buttons().unwrap();
        r.sessions.taken.lock().insert(session.into(), who.into());
        r.fake.press(&yes, ME, CHAT, message);
        until(|| r.fake.told(said), said);
    }
    assert!(r.sessions.answers.lock().is_empty());
    let to_telegram = format!("{:?}{:?}", r.fake.calls("answerCallbackQuery"), r.fake.calls("editMessageText"));
    assert!(!to_telegram.contains("phone-4f2a"), "Telegram is not told a phone's name or id");
}

#[test]
fn what_the_daemon_does_not_offer_has_no_buttons() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.waiting("dangerous", 5);
    until(|| !r.alerts().is_empty(), "the alert");
    assert!(r.fake.buttons().is_none());
    // A wait other than the one offered has none either.
    r.sessions.offer("s1", 5);
    r.waiting("s1", 6);
    until(|| r.alerts().len() == 2, "the second alert");
    assert!(r.fake.buttons().is_none());
}

#[test]
fn six_approvals_a_minute_then_alerts_without_buttons() {
    let r = rig(TelegramMode::ApproveLow).bound();
    for i in 0..6 {
        let s = format!("s{i}");
        r.sessions.offer(&s, 5);
        r.waiting(&s, 5);
        until(|| r.fake.all_buttons().len() == i + 1, "buttons");
        let (yes, _, message) = r.fake.buttons().unwrap();
        r.fake.press(&yes, ME, CHAT, message);
        until(|| r.sessions.answers.lock().len() == i + 1, "answered");
    }
    r.sessions.offer("s6", 5);
    r.waiting("s6", 5);
    until(|| r.alerts().len() == 7, "the seventh");
    assert_eq!(r.fake.all_buttons().len(), 6, "alert only");
}

#[test]
fn the_seventh_approval_pressed_in_a_minute_is_refused() {
    let r = rig(TelegramMode::ApproveLow).bound();
    for i in 0..7 {
        let s = format!("s{i}");
        r.sessions.offer(&s, 5);
        r.waiting(&s, 5);
        until(|| r.fake.all_buttons().len() == i + 1, "buttons");
    }
    for (yes, _, message) in r.fake.all_buttons() {
        r.fake.press(&yes, ME, CHAT, message);
    }
    until(|| r.fake.told("Six approvals from Telegram this minute"), "the seventh is told");
    r.fake.read();
    let answered: Vec<String> = r.sessions.answers.lock().iter().map(|(s, _)| s.clone()).collect();
    assert_eq!(answered, (0..6).map(|i| format!("s{i}")).collect::<Vec<_>>());
    assert!(r.sessions.noted("telegram_refused").iter().any(|s| s.starts_with("s6: too many approvals this minute")));
    assert!(r.fake.calls("editMessageText").iter().any(|b| b["text"].as_str().is_some_and(|t| t.ends_with("too many approvals from Telegram this minute."))));
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
    r.fake.press(&yes, ME, CHAT, message);
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
    let t = r.n.config().telegram;
    assert!(!t.approving() && t.mode == TelegramMode::AlertLink && t.user_id.is_empty() && t.user_name.is_empty(), "{t:?}");
    let saved: Config = serde_json::from_slice(&std::fs::read(r.dir.join("notify.json")).unwrap()).unwrap();
    assert!(saved.telegram.user_id.is_empty() && saved.telegram.mode == TelegramMode::AlertLink, "and it holds after a restart");
    until(|| !r.approvals().polling(), "polling stopped");
    assert_eq!(r.approvals().describe()["open"], 0, "every nonce burnt");
    until(|| r.fake.calls("editMessageText").iter().any(|b| b["message_id"] == message), "buttons removed");
    assert!(r.sessions.actions().contains(&"telegram_off".to_string()));
    // A valid press that reaches the daemon anyway answers nothing: the filter itself, not only the stopped polling.
    r.approvals().handle(&json!({"update_id": 99, "callback_query": press(&yes, ME, CHAT, message)}));
    assert!(r.sessions.answers.lock().is_empty());
}

#[test]
fn a_kill_switch_while_a_press_is_on_its_way_answers_nothing() {
    let r = rig(TelegramMode::ApproveLow).bound();
    r.sessions.offer("s1", 5);
    r.waiting("s1", 5);
    until(|| r.fake.buttons().is_some(), "buttons");
    let (yes, _, message) = r.fake.buttons().unwrap();
    // The kill switch lands while the press waits for its request (a terminal's menu, say), after the press was let through, and approvals
    // are turned on again before it goes on: they are on when it looks, so only the count of times they ended can tell.
    let approvals = Arc::downgrade(r.approvals());
    *r.sessions.before_go.lock() = Some(Box::new(move || {
        if let Some(a) = approvals.upgrade() {
            a.turn_off("every phone was revoked");
            a.bind_for_test(ME, CHAT);
            assert!(a.approving(), "on again");
        }
    }));
    r.fake.press(&yes, ME, CHAT, message);
    until(|| r.sessions.noted("telegram_refused").iter().any(|s| s.contains("turned off")), "refused");
    assert!(r.sessions.answers.lock().is_empty(), "nothing answered");
    until(|| r.fake.told("Approvals from Telegram are off"), "told");
    assert!(r.n.config().telegram.approving(), "and they stay on for what comes next");
}

#[test]
fn polling_stops_when_the_mode_changes() {
    let r = rig(TelegramMode::ApproveLow).bound();
    assert!(r.approvals().polling());
    let mut c = r.n.config();
    c.telegram.mode = TelegramMode::AlertLink;
    r.n.update(Some(c), &Value::Null).unwrap();
    until(|| !r.approvals().polling(), "stopped");
    let t = r.n.config().telegram;
    assert!(t.user_id.is_empty() && t.user_name.is_empty(), "the binding goes with it");
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
    c.telegram.user_name = "@someone_else".into();
    r.n.update(Some(c), &Value::Null).unwrap();
    let t = r.n.config().telegram;
    assert_eq!((t.user_id.as_str(), t.user_name.as_str()), ("42", "@me_here"));
    assert!(t.approving());
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

/// What a test's log subscriber was given.
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

#[test]
fn neither_the_token_nor_the_address_reaches_the_log() {
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::EnvFilter;
    // Telegram's calls that fail, as they are made and logged, under `filter`: what the log got, and the fake's address.
    let logged = |filter: EnvFilter| -> (String, String) {
        let out = Captured::default();
        let writer = out.clone();
        let subscriber = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(move || writer.clone()).finish();
        let fake = Fake::start();
        fake.api.lock().refuse = Some(401);
        {
            let _guard = subscriber.set_default();
            for (base, method) in [(fake.url.as_str(), "getUpdates"), (fake.url.as_str(), "sendMessage"), ("http://127.0.0.1:9", "getUpdates")] {
                if let Err(f) = telegram_call(base, TOKEN, method, &json!({"chat_id": 1, "text": "x"}), Duration::from_secs(5)) {
                    tracing::warn!("telegram approvals: {}", f.reason);
                }
            }
        }
        let log = String::from_utf8_lossy(&out.0.lock()).to_string();
        (log, fake.url.trim_start_matches("http://").to_string())
    };
    // The daemon's own filter, at the most the user can ask for.
    let (log, address) = logged(crate::log_filter(EnvFilter::new("trace")));
    assert!(log.contains("telegram approvals: HTTP 401"), "the failures are logged: {log}");
    assert!(!log.contains("BOTtoken") && !log.contains(&address) && !log.contains("127.0.0.1:9"), "{log}");
    // Without it the client says both: the capture does see what the client logs.
    let (log, address) = logged(EnvFilter::new("trace"));
    assert!(log.contains("BOTtoken") && log.contains(&address), "{log}");
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
    until(|| r.alerts().iter().any(|b| b.get("reply_markup").is_none()), "the plain alert");
    assert_eq!(r.approvals().describe()["open"], 0, "no button is left open");
}
