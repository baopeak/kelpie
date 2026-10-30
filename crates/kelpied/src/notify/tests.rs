use std::io::{Read, Write};
use std::net::TcpListener;

use super::*;
use crate::session::SessionSpec;

fn info(handle: u32, status: Status, failed: bool, alive: bool) -> SessionInfo {
    SessionInfo {
        handle,
        spec: SessionSpec { id: format!("s{handle}"), kind: "claude".into(), cwd: ".".into(), project_id: Some("p1".into()), ..Default::default() },
        title: None,
        status,
        unread: false,
        alive,
        exit_code: None,
        failed,
        limited: false,
        pid: None,
        cols: 80,
        rows: 24,
        seq: 0,
        chat: false,
        last_activity: 0,
        interrupted: false,
        waiting_since: None,
        waits_for_window: false,
    }
}

/// A scratch folder that goes when the test ends, whatever happens.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = std::path::Path;
    fn deref(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn notifier() -> (Notifier, Arc<Memory>, Scratch) {
    let dir = Scratch(std::env::temp_dir().join(format!("kelpie-notify-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&dir.0).unwrap();
    let secrets = Memory::new();
    (Notifier::new(&dir, secrets.clone()), secrets, dir)
}

fn finished(session: &str, secs: u64) -> Raised {
    Raised { session: Some(session.into()), turn: Some(Duration::from_secs(secs)), ..Raised::bare(Kind::Finished) }
}

// ---------- the old app-side rules, case by case ----------

/// Every case the app's `maybeNotify` handled: (previous status, previous failed, next status,
/// next failed, expected).
#[test]
fn classification_matches_every_case_of_the_old_app_logic() {
    use Status::*;
    let cases: &[(Status, bool, Status, bool, Option<Kind>)] = &[
        // Work that stopped: finished, or waiting for the user.
        (Working, false, Idle, false, Some(Kind::Finished)),
        (Working, false, Waiting, false, Some(Kind::Waiting)),
        // Nothing changed or the change is not the end of work.
        (Working, false, Working, false, None),
        (Idle, false, Idle, false, None),
        (Idle, false, Working, false, None),
        (Idle, false, Waiting, false, None),
        (Waiting, false, Idle, false, None),
        (Waiting, false, Working, false, None),
        (Waiting, false, Waiting, false, None),
        (Exited, false, Idle, false, None),
        // The process ended without an error: not an event.
        (Working, false, Exited, false, None),
        (Idle, false, Exited, false, None),
        // A turn or process that newly failed is reported once, whatever the status was.
        (Working, false, Idle, true, Some(Kind::Error)),
        (Idle, false, Idle, true, Some(Kind::Error)),
        (Working, false, Exited, true, Some(Kind::Error)),
        (Waiting, false, Waiting, true, Some(Kind::Error)),
        (Idle, false, Waiting, true, Some(Kind::Error)),
        // Already failed before: the end of work after it is an ordinary end.
        (Idle, true, Idle, true, None),
        (Working, true, Idle, true, Some(Kind::Finished)),
        (Working, true, Waiting, true, Some(Kind::Waiting)),
        // The failure clearing is not an event.
        (Idle, true, Idle, false, None),
        (Idle, true, Working, false, None),
    ];
    for (prev, prev_failed, next, next_failed, want) in cases {
        let got = classify(*prev, *prev_failed, &info(1, *next, *next_failed, *next != Exited));
        assert_eq!(got, *want, "{prev:?}/{prev_failed} -> {next:?}/{next_failed}");
    }
}

#[test]
fn the_first_sighting_of_a_session_is_not_an_event() {
    let (n, _, _) = notifier();
    let now = Instant::now();
    assert!(n.observe(&info(1, Status::Idle, true, true), now).is_none(), "no previous state: nothing to compare with");
    // Seeded when it was created: its first change counts.
    n.seed(&info(2, Status::Idle, false, true));
    let ev = n.observe(&info(2, Status::Idle, true, true), now).expect("a failure right after creation");
    assert_eq!(ev.kind, Kind::Error);
    assert_eq!(ev.session.as_deref(), Some("s2"));
    n.forget(2);
    assert!(n.observe(&info(2, Status::Idle, true, true), now).is_none(), "forgotten sessions start over");
}

#[test]
fn an_event_carries_what_the_app_needs_to_word_it() {
    let (n, _, _) = notifier();
    let t0 = Instant::now();
    n.seed(&info(1, Status::Idle, false, true));
    assert!(n.observe(&info(1, Status::Working, false, true), t0).is_none());
    let mut dead = info(1, Status::Exited, true, false);
    dead.exit_code = Some(3);
    let ev = n.observe(&dead, t0 + Duration::from_secs(70)).unwrap();
    let j = ev.to_json();
    assert_eq!((j["kind"].as_str(), j["session"].as_str(), j["project_id"].as_str()), (Some("error"), Some("s1"), Some("p1")));
    assert_eq!((j["alive"].as_bool(), j["exit_code"].as_i64()), (Some(false), Some(3)));
    assert!(j["id"].as_str().is_some_and(|i| i.len() == 36), "every event has its own id");
    assert_eq!(ev.turn.map(|t| t.as_secs()), Some(70), "the turn is timed from the first work after idle");
}

#[test]
fn a_turn_is_timed_across_a_wait_and_restarts_after_idle() {
    let (n, _, _) = notifier();
    let t0 = Instant::now();
    n.seed(&info(1, Status::Idle, false, true));
    n.observe(&info(1, Status::Working, false, true), t0);
    let waiting = n.observe(&info(1, Status::Waiting, false, true), t0 + Duration::from_secs(10)).unwrap();
    assert_eq!((waiting.kind, waiting.turn.unwrap().as_secs()), (Kind::Waiting, 10));
    n.observe(&info(1, Status::Working, false, true), t0 + Duration::from_secs(40));
    let done = n.observe(&info(1, Status::Idle, false, true), t0 + Duration::from_secs(100)).unwrap();
    assert_eq!((done.kind, done.turn.unwrap().as_secs()), (Kind::Finished, 100), "answering a prompt continues the same turn");
    n.observe(&info(1, Status::Working, false, true), t0 + Duration::from_secs(200));
    let next = n.observe(&info(1, Status::Idle, false, true), t0 + Duration::from_secs(205)).unwrap();
    assert_eq!(next.turn.unwrap().as_secs(), 5, "a new turn starts its own clock");
}

#[test]
fn approvals_are_announced_once_per_waiting_task() {
    let (n, _, _) = notifier();
    let tasks = |waiting: bool| vec![("a".to_string(), "Build".to_string(), waiting), ("b".to_string(), "Test".to_string(), false)];
    let first = n.new_approvals("r1", "Release", Some("p1"), &tasks(true));
    assert_eq!(first.len(), 1);
    assert_eq!((first[0].kind, first[0].detail.as_deref(), first[0].project_id.as_deref()), (Kind::Approval, Some("Release · Build"), Some("p1")));
    assert!(n.new_approvals("r1", "Release", Some("p1"), &tasks(true)).is_empty(), "still waiting: said already");
    assert!(n.new_approvals("r1", "Release", Some("p1"), &tasks(false)).is_empty());
    assert_eq!(n.new_approvals("r1", "Release", Some("p1"), &tasks(true)).len(), 1, "waiting again after it moved on");
}

// ---------- what the user is looking at ----------

#[test]
fn only_a_focused_window_that_shows_the_session_counts_as_looking() {
    let (n, _, _) = notifier();
    assert!(!n.looking_at("a"), "no window connected");
    n.set_ui(1, true, vec!["a".into(), "b".into()], None);
    assert!(n.looking_at("a") && n.looking_at("b") && !n.looking_at("c"));
    n.set_ui(1, false, vec!["a".into()], None);
    assert!(!n.looking_at("a"), "an unfocused window shows nothing the user sees");
    n.set_ui(1, true, vec!["a".into()], None);
    n.drop_client(1);
    assert!(!n.looking_at("a"), "the window closed");
    n.set_ui(2, true, vec!["x".repeat(65), "ok".into()], None);
    assert!(n.looking_at("ok") && !n.looking_at(&"x".repeat(65)), "ids are bounded");
}

// ---------- when a message leaves ----------

fn on(n: &Notifier, tweak: impl FnOnce(&mut Config)) {
    let mut c = n.config();
    c.ntfy.enabled = true;
    tweak(&mut c);
    n.update(Some(c), &Value::Null).unwrap();
}

#[test]
fn with_no_channel_nothing_is_started() {
    let (n, secrets, dir) = notifier();
    let now = Instant::now();
    n.seed(&info(1, Status::Idle, false, true));
    n.observe(&info(1, Status::Working, false, true), now);
    let ev = n.observe(&info(1, Status::Waiting, false, true), now + Duration::from_secs(60)).unwrap();
    assert!(!n.active());
    assert!(!n.wants_push(&ev, now), "no channel: no message");
    assert!(n.worker.lock().is_none(), "no worker thread (and so no socket) until a message is sent");
    assert!(n.limiter.lock().last.is_empty(), "the rate limiter is untouched");
    assert!(secrets.0.lock().is_empty(), "nothing was written to the credential store");
    assert!(!dir.join("notify.json").exists(), "nor to disk");
    // A channel switched on and off again goes back to the same quiet state.
    on(&n, |_| {});
    assert!(n.active());
    n.update(Some(Config::default()), &Value::Null).unwrap();
    assert!(!n.active() && !n.wants_push(&ev, now));
}

#[test]
fn filters_decide_what_leaves() {
    let (n, _, _dir) = notifier();
    let now = Instant::now();
    on(&n, |c| {
        c.events.done = false;
        c.min_turn_s = 0;
    });
    assert!(!n.wants_push(&finished("a", 99), now), "done events are switched off");
    let waiting = Raised { session: Some("a".into()), ..Raised::bare(Kind::Waiting) };
    assert!(n.wants_push(&waiting, now), "needs-you passes");
    // Turn length: only finished turns are measured.
    on(&n, |c| {
        c.events.done = true;
        c.min_turn_s = 60;
    });
    assert!(!n.wants_push(&finished("b", 59), now), "a short turn stays quiet");
    assert!(n.wants_push(&finished("c", 60), now), "a long enough turn is sent");
    let waiting_fast = Raised { session: Some("d".into()), turn: Some(Duration::from_secs(1)), ..Raised::bare(Kind::Waiting) };
    assert!(n.wants_push(&waiting_fast, now), "needing you is not measured");
    // Each event kind has its own switch.
    on(&n, |c| c.events = Events { needs_you: false, done: false, failed: false, automation: true, approval: false, cost_cap: false });
    for (kind, want) in [(Kind::Waiting, false), (Kind::Finished, false), (Kind::Error, false), (Kind::Automation, true), (Kind::Approval, false), (Kind::Cost, false)] {
        let ev = Raised { session: Some(format!("k-{}", kind.code())), turn: Some(Duration::from_secs(999)), ..Raised::bare(kind) };
        assert_eq!(n.wants_push(&ev, now), want, "{kind:?}");
    }
}

#[test]
fn the_away_filter_sends_only_without_a_focused_window() {
    let (n, _, _dir) = notifier();
    let now = Instant::now();
    on(&n, |c| c.away_only = true);
    let ev = |s: &str| Raised { session: Some(s.into()), ..Raised::bare(Kind::Waiting) };
    n.set_ui(1, true, vec![], None);
    // The computer may also be idle (Windows tells); a test machine in use is not.
    if os_idle().is_none_or(|i| i < IDLE_AFTER) {
        assert!(!n.wants_push(&ev("a"), now), "the window is focused");
    }
    n.set_ui(1, false, vec![], None);
    assert!(n.wants_push(&ev("b"), now), "window not focused");
    n.set_ui(1, true, vec![], None);
    n.drop_client(1);
    assert!(n.wants_push(&ev("c"), now), "window closed");
}

#[test]
fn one_message_per_session_per_window() {
    let mut l = RateLimiter::new(Duration::from_secs(30));
    let t0 = Instant::now();
    assert!(l.allow("a", t0));
    assert!(!l.allow("a", t0 + Duration::from_secs(29)));
    assert!(l.allow("b", t0 + Duration::from_secs(29)), "another session has its own window");
    assert!(!l.allow("a", t0 + Duration::from_secs(29)), "a refused message does not restart the window");
    assert!(l.allow("a", t0 + Duration::from_secs(30)));
    for i in 0..600 {
        l.allow(&format!("many-{i}"), t0 + Duration::from_secs(100));
    }
    assert!(l.allow("late", t0 + Duration::from_secs(200)));
    assert_eq!(l.last.len(), 1, "entries older than the window are dropped once the table is large");
}

#[test]
fn the_notifier_limits_each_session_to_one_message_in_thirty_seconds() {
    let (n, _, _dir) = notifier();
    let now = Instant::now();
    on(&n, |c| c.min_turn_s = 0);
    assert!(n.wants_push(&finished("a", 5), now));
    assert!(!n.wants_push(&finished("a", 5), now + Duration::from_secs(10)));
    assert!(!n.wants_push(&Raised { session: Some("a".into()), ..Raised::bare(Kind::Error) }, now + Duration::from_secs(20)), "any kind counts for the session");
    assert!(n.wants_push(&finished("b", 5), now + Duration::from_secs(10)));
    assert!(n.wants_push(&finished("a", 5), now + Duration::from_secs(31)));
    // Events without a session are limited by what they are about.
    let auto = |name: &str| Raised::automation(name, None, None);
    assert!(n.wants_push(&auto("nightly"), now));
    assert!(!n.wants_push(&auto("nightly"), now + Duration::from_secs(5)));
    assert!(n.wants_push(&auto("weekly"), now + Duration::from_secs(5)));
}

// ---------- what a message holds ----------

#[test]
fn a_payload_is_redacted_and_screen_text_is_off_unless_asked_for() {
    let key = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
    let ctx = Context {
        project: Some("secret-project API_KEY=abcdef123456".into()),
        title: Some(format!("Claude · fixing {key}")),
        screen: Some(format!("$ export GITHUB_TOKEN={key}\nline two\npassword: hunter2hunter2")),
        phone_url: Some("http://192.168.1.5:4817/".into()),
    };
    let ev = Raised { session: Some("abc-123".into()), turn: Some(Duration::from_secs(125)), ..Raised::bare(Kind::Waiting) };

    let p = build_payload(&ev, &ctx, false);
    assert_eq!(p.status, "needs_you");
    assert!(p.screen.is_none(), "screen text is off by default");
    assert_eq!(p.duration_s, Some(125));
    let all = serde_json::to_string(&p).unwrap() + &p.title() + &p.text();
    assert!(!all.contains(key), "the title is redacted: {all}");
    assert!(!all.contains("abcdef123456"), "the project name is redacted: {all}");
    assert!(all.contains(kelpie_proto::redact::MASK));
    assert!(p.text().contains("(2m 5s)"), "{}", p.text());

    let p = build_payload(&ev, &ctx, true);
    let screen = p.screen.as_deref().expect("asked for");
    assert!(!screen.contains(key) && !screen.contains("hunter2hunter2"), "screen text is redacted: {screen}");
    assert!(screen.contains("line two"));
    assert!(p.text().contains("line two"));

    // Only the end of a long screen is kept.
    let long = Context { screen: Some(format!("{}\nTAIL", "x".repeat(5000))), ..Default::default() };
    let p = build_payload(&ev, &long, true);
    assert!(p.screen.as_deref().is_some_and(|s| s.chars().count() <= SCREEN_CHARS && s.ends_with("TAIL")));
}

#[test]
fn names_from_the_terminal_become_one_plain_line() {
    let ctx = Context { title: Some(format!("build\r\n\x1b[31mDONE\x1b[0m {}", "x".repeat(400))), project: Some("a\nb".into()), ..Default::default() };
    let p = build_payload(&Raised { session: Some("s".into()), ..Raised::bare(Kind::Waiting) }, &ctx, false);
    let session = p.session.unwrap();
    assert!(!session.chars().any(char::is_control), "{session:?}");
    assert!(session.chars().count() <= 200);
    assert_eq!(p.project.as_deref(), Some("a b"));
    assert!(session.starts_with("build"));
}

#[test]
fn a_deep_link_never_carries_a_code_or_a_token() {
    assert_eq!(deep_link("http://192.168.1.5:4817/", "abc-123").as_deref(), Some("http://192.168.1.5:4817/#s=abc-123"));
    // A base that held a pairing fragment or a query loses it.
    assert_eq!(deep_link("https://pc.tail.ts.net:4817/#p=deadbeefcafe", "abc").as_deref(), Some("https://pc.tail.ts.net:4817/#s=abc"));
    assert_eq!(deep_link("https://pc:4817/?token=zzz", "abc").as_deref(), Some("https://pc:4817/#s=abc"));
    for bad in ["", "javascript:alert(1)", "ftp://x/", "http://user:pw@host/", "not a url"] {
        assert!(deep_link(bad, "abc").is_none(), "{bad}");
    }
    assert!(deep_link("http://h/", "a b").is_none() && deep_link("http://h/", "a/../b").is_none() && deep_link("http://h/", &"a".repeat(65)).is_none());
    // Only session events have a link, and only while phone access is on.
    let ctx = Context { phone_url: Some("http://h:1/#p=code".into()), ..Default::default() };
    let with = build_payload(&Raised { session: Some("s1".into()), ..Raised::bare(Kind::Waiting) }, &ctx, false);
    assert_eq!(with.link.as_deref(), Some("http://h:1/#s=s1"));
    assert!(!serde_json::to_string(&with).unwrap().contains("code"));
    assert!(build_payload(&Raised::bare(Kind::Approval), &ctx, false).link.is_none());
    assert!(build_payload(&Raised { session: Some("s1".into()), ..Raised::bare(Kind::Waiting) }, &Context::default(), false).link.is_none());
}

#[test]
fn session_names_in_messages_follow_the_apps_rules() {
    assert_eq!(session_label(Some("My task"), Some("anything"), "claude", "Claude Code", None), "My task");
    assert_eq!(session_label(None, Some("⠂ Refactor parser"), "claude", "Claude Code", None), "Claude Code · Refactor parser");
    assert_eq!(session_label(None, Some("Claude Code"), "claude", "Claude Code", None), "Claude Code");
    assert_eq!(session_label(None, Some("~/proj"), "shell", "Terminal", None), "~/proj");
    assert_eq!(session_label(None, Some("C:\\Windows\\system32\\cmd.exe"), "shell", "Terminal", None), "Terminal");
    assert_eq!(session_label(None, None, "codex", "Codex", Some("dev@box")), "Codex @dev@box");
    assert_eq!(session_label(Some("  "), Some(&"t".repeat(60)), "codex", "Codex", None), "Codex");
}

// ---------- configuration and secrets ----------

#[test]
fn the_topic_is_128_random_bits() {
    let topics: Vec<String> = (0..300).map(|_| random_topic()).collect();
    assert!(topics.iter().all(|t| t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit())));
    assert_eq!(topics.iter().collect::<HashSet<_>>().len(), 300);
    // No fixed version or variant bits (where a v4 UUID has them): those nibbles vary too.
    for at in [12, 16] {
        let nibbles: HashSet<char> = topics.iter().map(|t| t.chars().nth(at).unwrap()).collect();
        assert!(nibbles.len() > 8, "nibble {at}: {nibbles:?}");
    }
}

#[test]
fn secrets_live_in_the_store_and_never_in_the_file_or_the_reply() {
    let (n, secrets, dir) = notifier();
    let c = Config { ntfy: Ntfy { enabled: true, server: "https://ntfy.example.org".into() }, telegram: Telegram { enabled: true, chat_id: "-100123".into() }, webhook: Webhook { enabled: true }, ..Config::default() };
    let hook = "https://hooks.example.org/in/abcdefSECRET";
    n.update(Some(c.clone()), &json!({"ntfy_token": "tk_ntfySECRETvalue", "telegram_token": "123456:TELEGRAMsecret", "webhook_url": hook})).unwrap();
    let topic = secrets.get(K_TOPIC).expect("switching ntfy on creates the topic");
    assert_eq!(topic.len(), 32);
    assert_eq!(secrets.get(K_WEBHOOK).as_deref(), Some(hook));

    let file = std::fs::read_to_string(dir.join("notify.json")).unwrap();
    let reply = n.describe(false).to_string();
    for text in [&file, &reply] {
        for secret in [topic.as_str(), "tk_ntfySECRETvalue", "TELEGRAMsecret", "abcdefSECRET"] {
            assert!(!text.contains(secret), "{secret} leaked into {text}");
        }
    }
    let v: Value = serde_json::from_str(&reply).unwrap();
    assert_eq!(v["secrets"], json!({"ntfy_topic": true, "ntfy_token": true, "telegram_token": true, "webhook_url": true}));
    assert!(v.get("ntfy_topic").is_none());
    assert_eq!(n.describe(true)["ntfy_topic"], json!(topic), "the topic is shown only when asked for");

    // A restart reads the same configuration; the secrets come from the store.
    let again = Notifier::new(&dir, secrets.clone());
    assert_eq!(again.config(), c);
    assert!(again.active());

    // A topic is replaced on request; null removes a secret; a missing key leaves it.
    let rotated = n.rotate_topic().unwrap();
    assert_ne!(rotated, topic);
    assert_eq!(secrets.get(K_TOPIC), Some(rotated));
    n.update(None, &json!({"ntfy_token": null})).unwrap();
    assert!(secrets.get(K_NTFY_TOKEN).is_none() && secrets.get(K_TELEGRAM_TOKEN).is_some());

    n.clear().unwrap();
    assert!(secrets.0.lock().is_empty(), "clear removes every secret");
    assert!(!n.active() && n.config() == Config::default());
}

#[test]
fn bad_settings_are_refused_and_change_nothing() {
    let (n, secrets, dir) = notifier();
    let good = Config::default();
    for bad in [
        Config { ntfy: Ntfy { enabled: true, server: "ftp://x".into() }, ..good.clone() },
        Config { ntfy: Ntfy { enabled: true, server: "https://user:pw@ntfy.sh".into() }, ..good.clone() },
        Config { ntfy: Ntfy { enabled: true, server: "https://".into() }, ..good.clone() },
        Config { telegram: Telegram { enabled: true, chat_id: "me; drop".into() }, ..good.clone() },
        Config { min_turn_s: 100_000, ..good.clone() },
    ] {
        assert!(n.update(Some(bad), &Value::Null).is_err());
    }
    for (field, value) in [("webhook_url", "javascript:1"), ("webhook_url", "http://x y"), ("telegram_token", "no-colon"), ("telegram_token", "1:a b"), ("ntfy_token", "has space")] {
        let before = secrets.0.lock().len();
        assert!(n.update(Some(good.clone()), &json!({ field: value })).is_err(), "{field}={value}");
        assert_eq!(secrets.0.lock().len(), before, "a refused change writes nothing");
    }
    assert!(!dir.join("notify.json").exists());
    assert!(n.update(Some(Config { telegram: Telegram { enabled: false, chat_id: "@my_channel".into() }, ..good }), &Value::Null).is_ok());
}

#[test]
fn a_telegram_address_can_be_replaced_only_by_a_test_run_with_a_loopback_one() {
    let real = "https://api.telegram.org";
    assert_eq!(telegram_base_for(false, None), real);
    assert_eq!(telegram_base_for(false, Some("http://127.0.0.1:9")), real, "not a test run");
    assert_eq!(telegram_base_for(true, Some("http://127.0.0.1:9/")), "http://127.0.0.1:9");
    assert_eq!(telegram_base_for(true, Some("http://localhost:9")), "http://localhost:9");
    for bad in ["https://evil.example", "http://evil.example", "http://127.0.0.1@evil.example/", "http://192.168.1.1:9", "ftp://127.0.0.1"] {
        assert_eq!(telegram_base_for(true, Some(bad)), real, "{bad}");
    }
}

// ---------- delivery, against a local server ----------

struct Request {
    path: String,
    authorization: Option<String>,
    body: Value,
}

/// A local HTTP server that answers `status` to the next `count` requests and hands them over.
fn sink(count: usize, status: u16) -> (String, mpsc::Receiver<Request>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..count {
            let Ok((mut stream, _)) = listener.accept() else { return };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            let body_at = loop {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break None;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(i + 4);
                }
            };
            let Some(body_at) = body_at else { continue };
            let head = String::from_utf8_lossy(&buf[..body_at]).to_string();
            let header = |name: &str| head.lines().find_map(|l| l.to_lowercase().strip_prefix(&format!("{name}:")).map(|v| v.trim().to_string()));
            let len: usize = header("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
            while buf.len() < body_at + len {
                let n = stream.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let authorization = head.lines().find_map(|l| l.strip_prefix("authorization: ").or_else(|| l.strip_prefix("Authorization: "))).map(str::to_string);
            let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
            let body = serde_json::from_slice(&buf[body_at..body_at + len.min(buf.len() - body_at)]).unwrap_or(Value::Null);
            let reply = format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            let _ = stream.write_all(reply.as_bytes());
            let _ = tx.send(Request { path, authorization, body });
        }
    });
    (addr, rx)
}

fn configured(sink_url: &str) -> (Notifier, Arc<Memory>, Scratch) {
    let (n, secrets, dir) = notifier();
    let mut c = Config { min_turn_s: 0, ..Config::default() };
    c.ntfy = Ntfy { enabled: true, server: sink_url.into() };
    n.update(Some(c), &json!({"ntfy_token": "tk_accessTOKEN"})).unwrap();
    (n, secrets, dir)
}

#[test]
fn ntfy_gets_json_with_the_topic_in_the_body_and_the_token_in_a_header() {
    let (url, rx) = sink(1, 200);
    let (n, secrets, _dir) = configured(&url);
    n.test_via(Channel::Ntfy, "unused").unwrap();
    let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(r.path, "/", "the topic is not part of the address");
    assert_eq!(r.authorization.as_deref(), Some("Bearer tk_accessTOKEN"));
    assert_eq!(r.body["topic"], json!(secrets.get(K_TOPIC).unwrap()));
    assert!(r.body["message"].as_str().unwrap().contains("Your Kelpie notifications work"));
    assert!(n.describe(false)["status"]["ntfy"]["ok_at"].is_u64());
}

#[test]
fn telegram_and_webhook_get_their_messages() {
    let (url, rx) = sink(2, 200);
    let (n, _, _dir) = notifier();
    let c = Config { telegram: Telegram { enabled: true, chat_id: "-100777".into() }, webhook: Webhook { enabled: true }, ..Config::default() };
    n.update(Some(c), &json!({"telegram_token": "123456:BOTtoken", "webhook_url": format!("{url}/hook/1")})).unwrap();
    n.test_via(Channel::Telegram, &url).unwrap();
    let tg = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(tg.path, "/bot123456:BOTtoken/sendMessage");
    assert_eq!((tg.body["chat_id"].as_str(), tg.body["disable_web_page_preview"].as_bool()), (Some("-100777"), Some(true)));
    assert!(tg.body["text"].as_str().unwrap().contains("Your Kelpie notifications work"));
    n.test_via(Channel::Webhook, &url).unwrap();
    let hook = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(hook.path, "/hook/1");
    assert_eq!(hook.body["source"], "kelpie");
    assert_eq!(hook.body["event"]["status"], "automation");
}

#[test]
fn errors_say_what_failed_and_never_where() {
    let (url, rx) = sink(1, 500);
    let (n, _, dir) = configured(&url);
    let err = n.test_via(Channel::Ntfy, "unused").unwrap_err();
    assert_eq!(err, "HTTP 500");
    let _ = rx.recv_timeout(Duration::from_secs(5));
    // Nothing listens there any more.
    let gone = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let (n2, _, dir2) = configured(&gone);
    let err = n2.test_via(Channel::Ntfy, "unused").unwrap_err();
    assert!(!err.contains("127.0.0.1") && !err.contains("tk_accessTOKEN") && !err.contains("://"), "{err}");
    let status = n2.describe(false)["status"].to_string();
    assert!(!status.contains("127.0.0.1"), "{status}");
    // Channels without their secrets fail without a request.
    let (n3, _, dir3) = notifier();
    n3.update(Some(Config { telegram: Telegram { enabled: true, chat_id: "1".into() }, ..Config::default() }), &Value::Null).unwrap();
    assert_eq!(n3.test_via(Channel::Telegram, "http://127.0.0.1:1").unwrap_err(), "no bot token");
    drop((dir, dir2, dir3));
}

#[test]
fn an_event_is_posted_once_by_the_worker_and_carries_a_link_only_with_phone_access() {
    let (url, rx) = sink(1, 200);
    let (n, _, _dir) = configured(&url);
    let now = Instant::now();
    let ev = finished("s1", 120);
    let ctx = Context { project: Some("Kelpie".into()), title: Some("Claude · docs".into()), phone_url: Some("http://192.168.1.5:4817/".into()), ..Default::default() };
    assert!(n.wants_push(&ev, now));
    n.send(&ev, &ctx);
    let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(r.body["title"], "Kelpie · Kelpie");
    assert_eq!(r.body["message"], "Claude · docs: Finished (2m 0s)");
    assert_eq!(r.body["click"], "http://192.168.1.5:4817/#s=s1");
    assert!(!n.wants_push(&ev, now + Duration::from_secs(1)), "the same session again within 30 s is dropped");
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err(), "exactly one request");
}

// ---------- cost ----------

/// `cargo test -p kelpied --release classification_cost -- --nocapture`
#[test]
fn classification_costs_well_under_a_tenth_of_a_millisecond() {
    let (n, _, _dir) = notifier();
    let t0 = Instant::now();
    for h in 0..20 {
        n.seed(&info(h, Status::Idle, false, true));
    }
    // 20 sessions each going working -> waiting -> idle over and over: one change in three is an event.
    let cycle = [Status::Working, Status::Waiting, Status::Idle];
    let infos: Vec<SessionInfo> = (0..20u32).flat_map(|h| cycle.iter().map(move |s| info(h, *s, false, true))).collect();
    let rounds = 3000;
    let mut events = 0u32;
    let start = Instant::now();
    for r in 0..rounds {
        // Walk each session through its cycle in order.
        for step in 0..3 {
            for h in 0..20usize {
                if n.observe(&infos[h * 3 + step], t0 + Duration::from_millis(r as u64)).is_some() {
                    events += 1;
                }
            }
        }
    }
    let calls = rounds * 60;
    let per_call = start.elapsed() / calls as u32;
    println!("observe: {per_call:?} per status change over {calls} calls ({events} events)");
    assert!(events > 0);
    assert!(per_call < Duration::from_micros(100), "{per_call:?} per status change");
}

// ---------- review fixes ----------

#[test]
fn an_idle_computer_is_not_looking_even_with_the_window_in_front() {
    let (n, _, _dir) = notifier();
    n.set_ui(1, true, vec!["a".into()], None);
    *n.idle.lock() = Some(Duration::from_secs(10));
    assert!(n.looking_at("a"), "someone is at the keyboard");
    *n.idle.lock() = Some(IDLE_AFTER);
    assert!(!n.looking_at("a"), "away for five minutes: the event goes to the inbox and the phone");
    *n.idle.lock() = Some(IDLE_AFTER - Duration::from_secs(1));
    assert!(n.looking_at("a"));
}

#[test]
fn the_cost_cap_has_its_own_allowance_next_to_the_turns_own_message() {
    let (n, _, _dir) = notifier();
    let now = Instant::now();
    on(&n, |c| c.min_turn_s = 0);
    assert!(n.wants_push(&finished("a", 5), now));
    let cost = Raised::cost("a", None, Some("over".into()));
    assert!(n.wants_push(&cost, now + Duration::from_secs(3)), "not swallowed by the message just sent");
    assert!(!n.wants_push(&cost, now + Duration::from_secs(6)), "but still one per 30 s of its own");
    assert!(!n.wants_push(&finished("a", 5), now + Duration::from_secs(7)), "turn events keep theirs");
}

#[test]
fn a_backed_up_queue_drops_messages_and_says_so() {
    // A server that takes the connection and never answers holds the sending thread for its timeout.
    let hung = TcpListener::bind("127.0.0.1:0").unwrap();
    let (n, _, _dir) = configured(&format!("http://{}", hung.local_addr().unwrap()));
    let ctx = Context::default();
    for i in 0..(QUEUE + 20) {
        n.send(&Raised { session: Some(format!("s{i}")), ..Raised::bare(Kind::Waiting) }, &ctx);
    }
    let dropped = n.describe(false)["dropped"].as_u64().unwrap();
    assert!(dropped >= 19, "{dropped} dropped of {}", QUEUE + 20);
    drop(hung);
}

#[test]
fn a_token_is_never_sent_in_clear_to_another_computer() {
    let (n, _, _dir) = notifier();
    let cfg = |server: &str| Config { ntfy: Ntfy { enabled: true, server: server.into() }, ..Config::default() };
    assert!(n.update(Some(cfg("http://ntfy.lan:8080")), &json!({"ntfy_token": "tk_secret"})).is_err());
    assert!(n.update(Some(cfg("https://ntfy.example.org")), &json!({"ntfy_token": "tk_secret"})).is_ok());
    assert!(n.update(Some(cfg("http://ntfy.lan:8080")), &Value::Null).is_err(), "a saved token counts too");
    assert!(n.update(Some(cfg("http://ntfy.lan:8080")), &json!({"ntfy_token": null})).is_ok(), "without the token, plain http is the user's choice");
    assert!(n.update(Some(cfg("http://127.0.0.1:8080")), &json!({"ntfy_token": "tk_secret"})).is_ok(), "this computer is fine");
}

#[test]
fn the_phone_link_is_a_setting_that_starts_on() {
    assert!(Config::default().phone_link);
    let (n, _, _dir) = notifier();
    n.update(Some(Config { phone_link: false, ..Config::default() }), &Value::Null).unwrap();
    assert!(!n.config().phone_link);
    assert!(!Notifier::new(&_dir, Memory::new()).config().phone_link, "it is remembered");
}

#[test]
fn a_deleted_runs_waiting_tasks_are_forgotten() {
    let (n, _, _dir) = notifier();
    let tasks = vec![("a".to_string(), "Build".to_string(), true)];
    assert_eq!(n.new_approvals("r1", "Release", None, &tasks).len(), 1);
    n.forget_run("r1");
    assert_eq!(n.new_approvals("r1", "Release", None, &tasks).len(), 1, "a run with the same id starts over");
}

#[test]
fn without_a_system_clock_the_windows_own_activity_decides_whether_the_person_is_there() {
    let (n, _, _dir) = notifier();
    let five = IDLE_AFTER.as_millis() as u64;
    let now = 10_000_000u64;
    assert!(!n.idle_at(None, now), "no window, nobody to say");
    n.set_ui(1, true, vec!["a".into()], None);
    assert!(!n.idle_at(None, now), "an app that does not report activity is never idle");
    n.set_ui(1, true, vec!["a".into()], Some(now - five + 1000));
    assert!(!n.idle_at(None, now));
    n.set_ui(1, true, vec!["a".into()], Some(now - five));
    assert!(n.idle_at(None, now), "focused, but untouched for five minutes");
    // Another window used a moment ago: someone is there.
    n.set_ui(2, true, vec![], Some(now - 1000));
    assert!(!n.idle_at(None, now));
    // An unfocused window's activity says nothing.
    n.set_ui(2, false, vec![], Some(now - 1000));
    assert!(n.idle_at(None, now));
    // Where the system tells, it decides.
    assert!(!n.idle_at(Some(Duration::from_secs(1)), now));
    assert!(n.idle_at(Some(IDLE_AFTER), now));
}
