use super::*;
use crate::notify::{Context, Memory, Notifier, Raised};
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use base64::Engine;
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Instant;

#[derive(Clone)]
enum Answer {
    Status(u16),
    RetryAfter(u16, &'static str),
    /// The request is had, and no answer comes in the client's time: the receiver may have acted on it.
    Hang,
    /// An answer, late.
    After(u64, u16),
    /// The request is had, and the connection is closed with no answer: the receiver may have acted on it.
    Close,
}

#[derive(Clone, Debug)]
struct Request {
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    at: Instant,
}

/// A local server that takes a push service's place: real requests over a real socket, answered as scripted, 201 once the script is out.
#[derive(Clone)]
struct PushService {
    base: String,
    requests: Arc<Mutex<Vec<Request>>>,
    script: Arc<Mutex<VecDeque<Answer>>>,
}

impl PushService {
    fn start(script: &[Answer]) -> PushService {
        Self::serving(TcpListener::bind("127.0.0.1:0").unwrap(), script)
    }

    fn start_at(addr: std::net::SocketAddr) -> PushService {
        Self::serving(TcpListener::bind(addr).unwrap(), &[])
    }

    fn serving(listener: TcpListener, script: &[Answer]) -> PushService {
        let service = PushService { base: format!("http://{}", listener.local_addr().unwrap()), requests: Arc::default(), script: Arc::new(Mutex::new(script.iter().cloned().collect())) };
        let s = service.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let s = s.clone();
                std::thread::spawn(move || s.answer(stream));
            }
        });
        service
    }

    fn answer(&self, mut stream: std::net::TcpStream) {
        let (mut buf, mut chunk) = (Vec::new(), [0u8; 4096]);
        let found = loop {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break None;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break Some(i + 4);
            }
        };
        let Some(body_at) = found else { return };
        let head = String::from_utf8_lossy(&buf[..body_at]).to_string();
        let headers: HashMap<String, String> = head.lines().skip(1).filter_map(|l| l.split_once(':')).map(|(k, v)| (k.to_lowercase(), v.trim().to_string())).collect();
        let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
        while buf.len() < body_at + len {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        let path = head.split_whitespace().nth(1).unwrap_or("").to_string();
        self.requests.lock().push(Request { path, headers, body: buf[body_at..].to_vec(), at: Instant::now() });
        let answer = self.script.lock().pop_front().unwrap_or(Answer::Status(201));
        let (status, extra) = match answer {
            Answer::Status(s) => (s, String::new()),
            Answer::RetryAfter(s, v) => (s, format!("Retry-After: {v}\r\n")),
            Answer::Hang => {
                std::thread::sleep(Duration::from_secs(30));
                return;
            }
            Answer::Close => return,
            Answer::After(ms, s) => {
                std::thread::sleep(Duration::from_millis(ms));
                (s, String::new())
            }
        };
        let _ = stream.write_all(format!("HTTP/1.1 {status} X\r\n{extra}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
    }

    fn count(&self) -> usize {
        self.requests.lock().len()
    }

    fn wait_for(&self, n: usize, within: Duration) -> Vec<Request> {
        let start = Instant::now();
        while self.count() < n && start.elapsed() < within {
            std::thread::sleep(Duration::from_millis(10));
        }
        let got = self.requests.lock().clone();
        assert!(got.len() >= n, "{} of {n} requests in {within:?}", got.len());
        got
    }
}

struct FakeHost {
    waiting: Mutex<bool>,
    level: Mutex<Level>,
    gone: Mutex<Vec<String>>,
    /// What the page's address is for a phone whose own is not known: none, as with Tailscale's origin not back yet.
    url: Mutex<Option<String>>,
    tasks: Mutex<bool>,
}

impl Host for FakeHost {
    fn still_waiting(&self, _session: &str, _wait: Option<u64>) -> bool {
        *self.waiting.lock()
    }
    fn load_level(&self) -> Level {
        *self.level.lock()
    }
    fn task_waiting(&self, _run: &str, _task: &str) -> bool {
        *self.tasks.lock()
    }
    fn phone_url(&self) -> Option<String> {
        self.url.lock().clone()
    }
    fn subscription_gone(&self, device: &str, _endpoint: Option<&str>) {
        self.gone.lock().push(device.to_string());
    }
}

struct Rig {
    notifier: Notifier,
    dests: Arc<Destinations>,
    host: Arc<FakeHost>,
    secrets: Arc<Memory>,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig() -> Rig {
    let dir = std::env::temp_dir().join(format!("kelpie-destinations-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let (host, secrets) = (Arc::new(FakeHost { waiting: Mutex::new(true), level: Mutex::new(Level::L0), gone: Mutex::default(), url: Mutex::new(Some("https://desk.tail1234.ts.net:8443/".into())), tasks: Mutex::new(true) }), Memory::new());
    let dests = Destinations::new(&dir, secrets.clone(), host.clone());
    let notifier = Notifier::with_destinations(&dir, secrets.clone(), dests.clone());
    Rig { notifier, dests, host, secrets, dir }
}

impl Rig {
    /// A phone with alerts on at `service`: what its browser would hold to read them.
    fn phone(&self, id: &str, service: &PushService, lang: &str) -> Phone {
        self.phone_at(id, service, lang, None)
    }

    fn phone_at(&self, id: &str, service: &PushService, lang: &str, origin: Option<&str>) -> Phone {
        *self.dests.shared.test_base.lock() = Some(service.base.clone());
        self.dests.vapid_public(true).unwrap();
        let (secret, p256dh, auth) = web_push::test_subscription();
        self.secrets.set(&auth_key(id), &auth).unwrap();
        let mut all = self.dests.shared.devices.lock().clone();
        all.insert(id.to_string(), Target { endpoint: format!("{}/push/{id}", service.base), p256dh, lang: lang.into(), origin: origin.map(str::to_string) });
        self.dests.set_devices(all);
        Phone { secret, auth: B64.decode(auth).unwrap() }
    }

    fn outbox(&self, dest: &Dest) -> PathBuf {
        path_of(&self.dests.shared.dir, dest)
    }
}

struct Phone {
    secret: p256::SecretKey,
    auth: Vec<u8>,
}

impl Phone {
    fn open(&self, r: &Request) -> Value {
        serde_json::from_slice(&web_push::decrypt(&r.body, &self.secret, &self.auth).expect("it opens with this phone's keys")).unwrap()
    }
}

fn waiting(session: &str) -> Raised {
    Raised { session: Some(session.into()), wait: Some(7), ..Raised::bare(Kind::Waiting) }
}

const NEEDLE: &str = "needle-text-from-the-session";

fn telling() -> Context {
    Context { project: Some(NEEDLE.into()), title: Some(NEEDLE.into()), screen: Some(NEEDLE.into()), phone_url: Some("http://elsewhere/".into()) }
}

#[test]
fn a_needs_you_event_reaches_the_service_signed_encrypted_and_says_nothing_of_the_session() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "en");
    r.notifier.send(&Raised { project_id: Some(NEEDLE.into()), detail: Some(NEEDLE.into()), error: Some(NEEDLE.into()), ..waiting("session-1") }, &telling());
    let got = &service.wait_for(1, Duration::from_secs(5))[0];
    assert_eq!(got.path, "/push/d1");
    assert_eq!(got.headers["ttl"], "600");
    assert_eq!(got.headers["urgency"], "high");
    assert_eq!(got.headers["content-encoding"], "aes128gcm");
    assert_eq!(got.headers["content-type"], "application/octet-stream");
    assert_eq!(got.headers["topic"], web_push::topic("waiting|session-1|7"), "another wait of the session is another alert; a late one replaces its own");
    assert!(got.headers["topic"].len() <= 32);
    let (token, key) = got.headers["authorization"].strip_prefix("vapid t=").unwrap().split_once(", k=").unwrap();
    assert_eq!(key, r.dests.vapid_public(false).unwrap().unwrap());
    let claims: Value = serde_json::from_slice(&B64.decode(token.split('.').nth(1).unwrap()).unwrap()).unwrap();
    assert_eq!(claims["aud"], service.base);
    // What a service, or anyone between, could read.
    let wire = String::from_utf8_lossy(&got.body).to_string();
    for needle in [NEEDLE, "session-1", "desk.tail1234", "navigate"] {
        assert!(!wire.contains(needle), "{needle}");
        assert!(!format!("{:?}", got.headers).contains(needle), "{needle}");
    }
    let alert = phone.open(got);
    assert_eq!(alert["web_push"], 8030);
    assert_eq!(alert["notification"]["title"], "Kelpie");
    assert_eq!(alert["notification"]["body"], "A session needs you");
    assert_eq!(alert["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/#s=session-1&a=1");
    assert_eq!(alert["notification"]["tag"], got.headers["topic"].as_str());
    assert!(!alert.to_string().contains(NEEDLE));
}

#[test]
fn no_kind_of_event_puts_session_text_in_what_a_phone_gets_and_each_is_in_its_language() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "vi");
    let events = [
        waiting("s1"),
        Raised { session: Some("s2".into()), turn: Some(Duration::from_secs(9)), ..Raised::bare(Kind::Finished) },
        Raised { session: Some("s3".into()), error: Some(NEEDLE.into()), ..Raised::bare(Kind::Error) },
        Raised::automation(NEEDLE, Some("s4".into()), Some(NEEDLE.into())),
        Raised { task: None, ..Raised::approval(NEEDLE, NEEDLE, Some(NEEDLE.into())) },
        Raised::cost("s6", Some(NEEDLE.into()), Some(NEEDLE.into())),
    ];
    for ev in &events {
        r.notifier.send(ev, &telling());
    }
    let got = service.wait_for(events.len(), Duration::from_secs(10));
    let mut bodies: Vec<String> = got
        .iter()
        .map(|g| phone.open(g))
        .map(|a| {
            assert!(!a.to_string().contains(NEEDLE), "{a}");
            a["notification"]["body"].as_str().unwrap().to_string()
        })
        .collect();
    bodies.sort();
    let mut want: Vec<String> = [Kind::Waiting, Kind::Finished, Kind::Error, Kind::Automation, Kind::Approval, Kind::Cost].iter().map(|k| web_push::sentence(Wording::Kind(*k), "vi")).collect();
    want.sort();
    assert_eq!(bodies, want);
}

#[test]
fn what_a_phones_outbox_keeps_while_its_service_is_down_holds_no_text() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(503)]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &telling());
    service.wait_for(1, Duration::from_secs(5));
    let file = r.outbox(&Dest::Device("d1".into()));
    let start = Instant::now();
    while !file.exists() && start.elapsed() < Duration::from_secs(2) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let kept = std::fs::read_to_string(&file).unwrap_or_default();
    assert!(!kept.contains(NEEDLE), "{kept}");
}

#[test]
fn a_201_ends_it_and_is_in_the_status_list() {
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(1, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(service.count(), 1, "once");
    let described = r.notifier.describe(false);
    assert!(described["status"]["webpush"]["ok_at"].is_u64());
    assert_eq!(described["webpush"]["devices"], 1);
    assert!(!r.outbox(&Dest::Device("d1".into())).exists());
}

#[test]
fn a_404_or_410_ends_the_subscription_and_nothing_is_tried_again() {
    for status in [404, 410] {
        let (r, service) = (rig(), PushService::start(&[Answer::Status(status)]));
        r.phone("d1", &service, "en");
        r.notifier.send(&waiting("s1"), &Context::default());
        service.wait_for(1, Duration::from_secs(5));
        let start = Instant::now();
        while r.host.gone.lock().is_empty() && start.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*r.host.gone.lock(), ["d1"], "{status}");
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(service.count(), 1, "{status}: no second try");
        assert!(!r.outbox(&Dest::Device("d1".into())).exists());
        assert_eq!(r.notifier.describe(false)["status"]["webpush"]["error"], format!("HTTP {status}"));
    }
}

#[test]
fn a_429_is_tried_again_no_sooner_than_it_asks_and_delivers_once() {
    let (r, service) = (rig(), PushService::start(&[Answer::RetryAfter(429, "2")]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    let got = service.wait_for(2, Duration::from_secs(8));
    assert!(got[1].at.duration_since(got[0].at) >= Duration::from_millis(1900), "{:?}", got[1].at.duration_since(got[0].at));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(service.count(), 2, "and is done");
}

#[test]
fn three_503s_then_success_is_one_delivery_with_a_link_within_the_minute() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(503), Answer::Status(503), Answer::Status(503)]));
    let phone = r.phone("d1", &service, "en");
    let start = Instant::now();
    r.notifier.send(&waiting("s1"), &Context::default());
    let got = service.wait_for(4, Duration::from_secs(20));
    assert!(start.elapsed() < Duration::from_secs(60));
    assert_eq!(phone.open(&got[3])["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/#s=s1&a=1");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(service.count(), 4);
    assert!(!r.outbox(&Dest::Device("d1".into())).exists());
}

#[test]
fn another_4xx_is_final_and_counted() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(400)]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(1, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(service.count(), 1);
    assert!(r.host.gone.lock().is_empty(), "the subscription is not what was refused");
    assert_eq!(r.notifier.describe(false)["dropped"], 1);
}

#[test]
fn a_session_answered_before_the_next_try_is_not_told() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(503)]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(1, Duration::from_secs(5));
    *r.host.waiting.lock() = false;
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(service.count(), 1, "nothing after the person answered");
    assert!(!r.outbox(&Dest::Device("d1".into())).exists(), "and nothing kept");
}

/// A destination that hangs: another has its message long before the first gives up.
#[test]
fn a_service_that_hangs_holds_up_no_other_destination() {
    let (r, hung, fine) = (rig(), PushService::start(&[Answer::Hang]), PushService::start(&[]));
    r.phone("slow", &hung, "en");
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = fine.base.clone();
    r.notifier.update(Some(c), &Value::Null).unwrap();
    let start = Instant::now();
    r.notifier.send(&waiting("first"), &Context::default());
    hung.wait_for(1, Duration::from_secs(3));
    fine.wait_for(1, Duration::from_secs(2));
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    assert!(r.notifier.describe(false)["status"]["webpush"].is_null(), "the hung one has not answered yet");
}

/// A push service that hangs does not delay a channel, and the other way round.
#[test]
fn a_channel_that_hangs_does_not_delay_a_phone() {
    let (r, hung, fine) = (rig(), PushService::start(&[Answer::Hang]), PushService::start(&[]));
    let phone = r.phone("d1", &fine, "en");
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = hung.base.clone();
    r.notifier.update(Some(c), &Value::Null).unwrap();
    let start = Instant::now();
    r.notifier.send(&waiting("first"), &Context::default());
    let got = fine.wait_for(1, Duration::from_secs(2));
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    assert_eq!(phone.open(&got[0])["notification"]["body"], "A session needs you");
}

#[test]
fn a_timeout_is_said_without_an_address_and_tried_again() {
    let (r, service) = (rig(), PushService::start(&[Answer::Hang]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    // Ten seconds for the client's timeout: said at once, until the try after it, at least a tenth of a second later, goes through.
    let start = Instant::now();
    let mut status = String::new();
    while !status.contains("timed out") && start.elapsed() < Duration::from_secs(13) {
        status = r.notifier.describe(false)["status"]["webpush"].to_string();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(status.contains("timed out") && !status.contains("127.0.0.1") && !status.contains("/push/"), "{status}");
    let got = service.wait_for(2, Duration::from_secs(4));
    assert_eq!(got.len(), 2);
}

#[test]
fn with_a_phone_and_no_channel_the_notifier_counts_as_on_and_starts_only_with_a_message() {
    let (r, service) = (rig(), PushService::start(&[]));
    let now = Instant::now();
    assert!(!r.notifier.active() && !r.notifier.started());
    r.phone("d1", &service, "en");
    assert!(r.notifier.active(), "a subscribed device is a destination");
    assert!(!r.notifier.started(), "no thread yet");
    assert_eq!(service.count(), 0);
    assert!(r.notifier.wants_push(&waiting("s1"), now));
    r.notifier.send(&waiting("s1"), &Context::default());
    assert!(r.notifier.started());
    service.wait_for(1, Duration::from_secs(5));
    r.dests.set_devices(HashMap::new());
    assert!(!r.notifier.active(), "the last one gone: quiet again");
    assert!(!r.notifier.started(), "its thread let go");
    assert!(!r.notifier.wants_push(&waiting("s2"), now));
}

#[test]
fn removal_stops_what_a_device_still_had_to_send_before_it_returns() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(503)]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(1, Duration::from_secs(5));
    std::thread::sleep(Duration::from_millis(50));
    r.dests.set_devices(HashMap::new());
    assert!(!r.outbox(&Dest::Device("d1".into())).exists(), "gone when this returned");
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(service.count(), 1, "and no retry follows");
    assert!(!r.outbox(&Dest::Device("d1".into())).exists());
}

#[test]
fn an_address_edited_in_behind_the_guard_gets_no_request() {
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    *r.dests.shared.test_base.lock() = None;
    let err = r.dests.test_device("d1").unwrap_err();
    assert_eq!(err, "not a push service");
    assert_eq!(*r.host.gone.lock(), ["d1"]);
    assert_eq!(service.count(), 0);
    assert!(!err.contains("127.0.0.1"));
}

#[test]
fn a_secret_that_is_gone_ends_the_subscription() {
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    r.secrets.delete(&auth_key("d1")).unwrap();
    assert!(r.dests.test_device("d1").is_err());
    assert_eq!(*r.host.gone.lock(), ["d1"]);
    assert_eq!(service.count(), 0);
}

#[test]
fn a_test_alert_goes_to_each_phone_and_says_what_came_of_it() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(500)]));
    assert_eq!(r.notifier.test(Channel::WebPush).unwrap_err(), "no phone has alerts on");
    let phone = r.phone("d1", &service, "vi");
    assert_eq!(r.dests.test_device("d1").unwrap_err(), "HTTP 500", "said, and left to the person to try again");
    r.notifier.test(Channel::WebPush).unwrap();
    let got = service.wait_for(2, Duration::from_secs(1));
    let alert = phone.open(&got[1]);
    assert_eq!(alert["notification"]["body"], web_push::sentence(Wording::Test, "vi"));
    assert_eq!(alert["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/");
}

#[test]
fn a_daemon_that_starts_again_sends_what_the_last_left_and_lets_go_of_what_has_no_destination() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "en");
    let dest = Dest::Device("d1".into());
    std::fs::create_dir_all(&r.dests.shared.dir).unwrap();
    let mut left = Outbox::open(&r.outbox(&dest));
    left.add(Entry { id: "e1".into(), kind: "waiting".into(), session: Some("s1".into()), wait: Some(1), task: None, at: now_millis(), hold_until: 0, payload: None, unsure: 0 });
    let orphan = r.dests.shared.dir.join("device-orphan.json");
    std::fs::write(&orphan, b"{}").unwrap();
    let off = r.outbox(&Dest::Channel(Channel::Webhook));
    std::fs::write(&off, b"{}").unwrap();
    assert!(!r.notifier.started());
    r.notifier.resume();
    assert!(r.notifier.started(), "only what has something to send");
    let got = service.wait_for(1, Duration::from_secs(5));
    assert_eq!(phone.open(&got[0])["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/#s=s1&a=1");
    assert!(!orphan.exists() && !off.exists());
}

#[test]
fn with_no_destination_a_start_does_nothing() {
    let r = rig();
    r.notifier.resume();
    assert!(!r.notifier.started());
    assert!(!r.dests.shared.dir.exists(), "not even the folder");
    assert!(r.secrets.0.lock().is_empty());
}

#[test]
fn at_a_busy_load_what_needs_someone_goes_at_once_and_the_rest_leaves_as_one() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "en");
    *r.host.level.lock() = Level::L3;
    let start = Instant::now();
    r.notifier.send(&waiting("urgent"), &Context::default());
    let got = service.wait_for(1, Duration::from_secs(1));
    assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
    assert_eq!(phone.open(&got[0])["notification"]["body"], "A session needs you");
    *r.host.level.lock() = Level::L1;
    for s in ["a", "b", "c"] {
        r.notifier.send(&Raised { session: Some(s.into()), ..Raised::bare(Kind::Finished) }, &Context::default());
    }
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(service.count(), 1, "held for a moment");
    let got = service.wait_for(2, Duration::from_secs(4));
    assert_eq!(phone.open(&got[1])["notification"]["body"], "3 updates from your sessions");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(service.count(), 2, "one for all three");
}

#[test]
fn at_l2_what_can_wait_waits_longer() {
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    *r.host.level.lock() = Level::L2;
    r.notifier.send(&Raised { session: Some("a".into()), ..Raised::bare(Kind::Finished) }, &Context::default());
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(service.count(), 0, "past the second of L1");
    service.wait_for(1, Duration::from_secs(6));
}

#[test]
fn at_a_calm_load_nothing_is_held() {
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    let start = Instant::now();
    r.notifier.send(&Raised { session: Some("a".into()), ..Raised::bare(Kind::Finished) }, &Context::default());
    service.wait_for(1, Duration::from_secs(1));
    assert!(start.elapsed() < Duration::from_millis(900), "{:?}", start.elapsed());
}

#[test]
fn several_after_an_outage_are_one_summary_in_the_phones_language() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(503), Answer::Status(503), Answer::Status(503)]));
    let phone = r.phone("d1", &service, "vi");
    r.notifier.send(&waiting("a"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    r.notifier.send(&waiting("b"), &Context::default());
    r.notifier.send(&Raised { session: Some("c".into()), ..Raised::bare(Kind::Finished) }, &Context::default());
    let got = service.wait_for(4, Duration::from_secs(12));
    let alert = phone.open(&got[3]);
    assert_eq!(alert["notification"]["body"], web_push::sentence(Wording::NeedYouAndUpdates(2, 1), "vi"), "what waits, counted apart");
    assert_eq!(alert["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/", "the list, not one of them");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(service.count(), 4, "nothing else follows");
}

/// Master's channels kept each message as it was through a failure, and so do these.
#[test]
fn one_failed_attempt_leaves_each_message_with_its_own_text_and_link() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(503)]));
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = service.base.clone();
    r.notifier.update(Some(c), &Value::Null).unwrap();
    let telling_of = |title: &str| Context { project: Some("Proj".into()), title: Some(title.into()), phone_url: Some("http://192.168.1.5:4817/".into()), ..Default::default() };
    r.notifier.send(&waiting("a"), &telling_of("first"));
    service.wait_for(1, Duration::from_secs(3));
    r.notifier.send(&waiting("b"), &telling_of("second"));
    let got = service.wait_for(3, Duration::from_secs(6));
    let bodies: Vec<Value> = got.iter().map(|g| serde_json::from_slice(&g.body).unwrap()).collect();
    assert_eq!(bodies[1]["message"], "first: Needs your input");
    assert_eq!(bodies[1]["click"], "http://192.168.1.5:4817/#s=a");
    assert_eq!(bodies[1]["title"], "Kelpie · Proj");
    assert_eq!(bodies[2]["message"], "second: Needs your input");
    assert_eq!(bodies[2]["click"], "http://192.168.1.5:4817/#s=b");
}

/// What master did with a send that ended without a word, which a receiver may well have acted on: one more try, whoever waited how long.
/// Nothing like it for each of the next ten minutes.
#[test]
fn a_channel_gets_a_message_that_may_have_arrived_twice_at_most_as_on_master() {
    let (r, service) = (rig(), PushService::start(&[Answer::Hang, Answer::Hang, Answer::Hang, Answer::Hang]));
    let mut c = r.notifier.config();
    c.webhook.enabled = true;
    r.notifier.update(Some(c), &serde_json::json!({"webhook_url": format!("{}/hook", service.base)})).unwrap();
    r.notifier.send(&waiting("still-waiting"), &Context::default());
    // Ten seconds each and the wait between them, then long enough for a third to have shown.
    let got = service.wait_for(2, Duration::from_secs(15));
    std::thread::sleep(Duration::from_secs(11));
    assert_eq!(service.count(), 2, "the session still waits, and that buys no third");
    assert_eq!(r.notifier.describe(false)["dropped"], 1, "said");
    assert!(r.notifier.describe(false)["status"]["webhook"]["error"].to_string().contains("timed out"));
    assert!(!r.outbox(&Dest::Channel(Channel::Webhook)).exists());
    // A receiver can tell the second for what it is.
    let ids: Vec<Value> = got.iter().map(|g| serde_json::from_slice::<Value>(&g.body).unwrap()["event"]["id"].clone()).collect();
    assert!(ids[0].is_string());
    assert_eq!(ids[0], ids[1]);
}

/// A push service does not act on a repeat as a webhook would: every try goes under one `Topic` and `tag`, and takes the last one's place.
#[test]
fn a_phone_keeps_being_tried_past_two_sends_that_may_have_arrived() {
    let (r, service) = (rig(), PushService::start(&[Answer::Hang, Answer::Hang]));
    let phone = r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    let got = service.wait_for(3, Duration::from_secs(26));
    assert_eq!(got[0].headers["topic"], got[2].headers["topic"]);
    assert_eq!(phone.open(&got[2])["notification"]["tag"], got[2].headers["topic"].as_str());
}

fn outbox_entry(id: &str, kind: Kind) -> Entry {
    Entry { id: id.into(), kind: kind.code().into(), session: Some("s".into()), wait: None, task: None, at: now_millis(), hold_until: 0, payload: None, unsure: 0 }
}

#[test]
fn a_kind_never_takes_the_place_of_another() {
    let of = |kind: Kind| Alert::of(&Attempt::One(Box::new(outbox_entry("e", kind)))).about;
    let all = [Kind::Waiting, Kind::Finished, Kind::Error, Kind::Automation, Kind::Approval, Kind::Cost].map(of);
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(web_push::topic(a), web_push::topic(b));
        }
    }
    // The same thing again is the same alert: a repeat replaces it.
    assert_eq!(of(Kind::Finished), of(Kind::Finished));
    let summary = |id: &str| Alert::of(&Attempt::Together { ids: vec![id.into()], need_you: 2, updates: 0, urgent: true, latest: 0 }).about;
    assert_ne!(summary("one"), summary("two"), "a second outage's does not replace a first not read yet");
    assert_eq!(summary("one"), summary("one"), "tries of one do");
    let first = Alert::test().about;
    std::thread::sleep(Duration::from_millis(3));
    assert_ne!(first, Alert::test().about, "a second test shows something");
}

#[test]
fn a_finished_and_an_error_of_one_session_both_arrive_under_their_own_names() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "en");
    r.notifier.send(&Raised { session: Some("s1".into()), ..Raised::bare(Kind::Finished) }, &Context::default());
    r.notifier.send(&Raised { session: Some("s1".into()), ..Raised::bare(Kind::Error) }, &Context::default());
    let got = service.wait_for(2, Duration::from_secs(5));
    assert_ne!(got[0].headers["topic"], got[1].headers["topic"]);
    let tags: Vec<Value> = got.iter().map(|g| phone.open(g)["notification"]["tag"].clone()).collect();
    assert_ne!(tags[0], tags[1]);
}

#[test]
fn a_phones_alert_leads_to_the_origin_that_holds_its_pairing() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone_at("d1", &service, "en", Some("https://192.168.1.5:4817"));
    r.notifier.send(&waiting("s1"), &Context::default());
    let got = service.wait_for(1, Duration::from_secs(5));
    assert_eq!(phone.open(&got[0])["notification"]["navigate"], "https://192.168.1.5:4817/#s=s1&a=1", "not the address the daemon would name");
    r.dests.test_device("d1").unwrap();
    let got = service.wait_for(2, Duration::from_secs(2));
    assert_eq!(phone.open(&got[1])["notification"]["navigate"], "https://192.168.1.5:4817/");
}

#[test]
fn a_phone_with_no_origin_on_record_waits_while_there_is_no_right_address() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "en");
    *r.host.url.lock() = None;
    r.notifier.send(&waiting("s1"), &Context::default());
    let start = Instant::now();
    while r.notifier.describe(false)["status"]["webpush"]["error"].is_null() && start.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(r.notifier.describe(false)["status"]["webpush"]["error"], "no address for the phone page");
    assert_eq!(service.count(), 0, "no link to somewhere else");
    *r.host.url.lock() = Some("https://desk.tail1234.ts.net:8443/".into());
    let got = service.wait_for(1, Duration::from_secs(5));
    assert_eq!(phone.open(&got[0])["notification"]["navigate"], "https://desk.tail1234.ts.net:8443/#s=s1&a=1");
}

#[test]
fn an_approval_is_asked_of_the_run_and_so_survives_a_restart() {
    let (r, service) = (rig(), PushService::start(&[]));
    let phone = r.phone("d1", &service, "en");
    let dest = Dest::Device("d1".into());
    std::fs::create_dir_all(&r.dests.shared.dir).unwrap();
    let mut left = Outbox::open(&r.outbox(&dest));
    left.add(Entry { task: Some(("run".into(), "task".into())), session: None, ..outbox_entry("approval", Kind::Approval) });
    r.notifier.resume();
    let got = service.wait_for(1, Duration::from_secs(5));
    assert_eq!(phone.open(&got[0])["notification"]["body"], "A run is waiting for your approval");
    // Decided meanwhile: nothing goes.
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    *r.host.tasks.lock() = false;
    std::fs::create_dir_all(&r.dests.shared.dir).unwrap();
    let mut left = Outbox::open(&r.outbox(&dest));
    left.add(Entry { task: Some(("run".into(), "task".into())), session: None, ..outbox_entry("approval", Kind::Approval) });
    r.notifier.resume();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(service.count(), 0);
}

#[test]
fn a_state_that_could_not_be_read_deletes_nothing_at_the_start() {
    let r = rig();
    std::fs::create_dir_all(&r.dests.shared.dir).unwrap();
    let (channel, device) = (r.outbox(&Dest::Channel(Channel::Ntfy)), r.outbox(&Dest::Device("d1".into())));
    std::fs::write(&channel, b"{}").unwrap();
    std::fs::write(&device, b"{}").unwrap();
    std::fs::write(r.dir.join("notify.json"), b"{not json").unwrap();
    let dests = Destinations::new(&r.dir, r.secrets.clone(), r.host.clone());
    let n = Notifier::with_destinations(&r.dir, r.secrets.clone(), dests.clone());
    dests.set_devices_known(false);
    n.resume();
    assert!(channel.exists() && device.exists(), "what they had to send stays");
    assert!(!n.started());
    // Once it is known that nothing is on, they go.
    let dests = Destinations::new(&r.dir, r.secrets.clone(), r.host.clone());
    std::fs::remove_file(r.dir.join("notify.json")).unwrap();
    let n = Notifier::with_destinations(&r.dir, r.secrets.clone(), dests);
    n.resume();
    assert!(!channel.exists() && !device.exists());
}

#[test]
fn clearing_the_channels_clears_what_was_moved_aside() {
    let r = rig();
    std::fs::create_dir_all(&r.dests.shared.dir).unwrap();
    let aside = r.dests.shared.dir.join("ntfy.unreadable-1");
    std::fs::write(&aside, b"text").unwrap();
    r.notifier.clear().unwrap();
    assert!(!aside.exists());
}

/// A toggle while a request is under way: the thread from before leaves the file to the one after.
#[test]
fn a_channel_switched_off_and_on_during_a_request_has_one_writer() {
    let (r, service) = (rig(), PushService::start(&[Answer::After(800, 503)]));
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = service.base.clone();
    r.notifier.update(Some(c.clone()), &Value::Null).unwrap();
    r.notifier.send(&waiting("old"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    let mut off = c.clone();
    off.ntfy.enabled = false;
    r.notifier.update(Some(off), &Value::Null).unwrap();
    r.notifier.update(Some(c), &Value::Null).unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(service.count(), 1, "what it had went with the first switch");
    assert!(!r.outbox(&Dest::Channel(Channel::Ntfy)).exists(), "and the late answer wrote nothing");
    r.notifier.send(&waiting("new"), &Context::default());
    let got = service.wait_for(2, Duration::from_secs(3));
    assert!(String::from_utf8_lossy(&got[1].body).contains("Needs your input"));
}

#[test]
fn a_late_answer_about_an_address_the_phone_left_keeps_what_it_has_to_send() {
    let (r, service) = (rig(), PushService::start(&[Answer::After(500, 410)]));
    let phone = r.phone("d1", &service, "en");
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    // The phone subscribed again while that request was out.
    let phone2 = {
        let (secret, p256dh, auth) = web_push::test_subscription();
        r.secrets.set(&auth_key("d1"), &auth).unwrap();
        let mut all = r.dests.shared.devices.lock().clone();
        all.insert("d1".to_string(), Target { endpoint: format!("{}/push/new", service.base), p256dh, lang: "en".into(), origin: None });
        r.dests.set_devices(all);
        Phone { secret, auth: B64.decode(auth).unwrap() }
    };
    let _ = phone;
    let got = service.wait_for(2, Duration::from_secs(5));
    assert_eq!(got[1].path, "/push/new");
    assert_eq!(phone2.open(&got[1])["notification"]["body"], "A session needs you");
}

#[test]
fn a_worker_never_makes_a_signing_key_under_subscriptions_made_with_another() {
    let (r, service) = (rig(), PushService::start(&[]));
    r.phone("d1", &service, "en");
    r.secrets.delete(K_VAPID).unwrap();
    r.dests.forget_vapid();
    assert_eq!(r.dests.test_device("d1").unwrap_err(), "no signing key");
    assert_eq!(*r.host.gone.lock(), ["d1"], "the phone subscribes again");
    assert_eq!(service.count(), 0);
    assert_eq!(r.secrets.get(K_VAPID), None, "and none was made");
}

#[test]
fn a_channel_says_the_summary_too_after_an_outage() {
    let service = PushService::start(&[Answer::Status(503), Answer::Status(503), Answer::Status(503)]);
    let r = rig();
    let mut c = r.notifier.config();
    c.webhook.enabled = true;
    r.notifier.update(Some(c), &serde_json::json!({"webhook_url": format!("{}/hook", service.base)})).unwrap();
    r.notifier.send(&waiting("a"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    r.notifier.send(&waiting("b"), &Context::default());
    let got = service.wait_for(4, Duration::from_secs(12));
    let body: Value = serde_json::from_slice(&got[3].body).unwrap();
    assert_eq!(body["event"]["summary"], "2 sessions need you");
    assert_eq!(body["event"]["status"], "needs_you");
    assert!(body["event"]["id"].is_string());
    assert_eq!(got[3].path, "/hook");
}

/// A webhook's flow may act and then answer 500; master tried it twice, and so does this.
#[test]
fn a_webhook_answering_a_server_error_gets_a_message_twice_at_most() {
    for status in [500, 504] {
        let (r, service) = (rig(), PushService::start(&[Answer::Status(status), Answer::Status(status), Answer::Status(status)]));
        let mut c = r.notifier.config();
        c.webhook.enabled = true;
        r.notifier.update(Some(c), &serde_json::json!({"webhook_url": format!("{}/hook", service.base)})).unwrap();
        r.notifier.send(&waiting("still-waiting"), &Context::default());
        let got = service.wait_for(2, Duration::from_secs(8));
        assert!(got[1].at.duration_since(got[0].at) >= Duration::from_millis(2900), "{:?}", got[1].at.duration_since(got[0].at));
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(service.count(), 2, "{status}: the session still waits, and that buys no third");
        assert_eq!(r.notifier.describe(false)["dropped"], 1);
    }
}

/// A service asking for less than the wait a second send always had does not shorten it.
#[test]
fn a_short_retry_after_does_not_shorten_the_wait_before_a_second_send() {
    let (r, service) = (rig(), PushService::start(&[Answer::RetryAfter(500, "0")]));
    let mut c = r.notifier.config();
    c.webhook.enabled = true;
    r.notifier.update(Some(c), &serde_json::json!({"webhook_url": format!("{}/hook", service.base)})).unwrap();
    r.notifier.send(&waiting("s1"), &Context::default());
    let got = service.wait_for(2, Duration::from_secs(8));
    assert!(got[1].at.duration_since(got[0].at) >= Duration::from_millis(2900), "{:?}", got[1].at.duration_since(got[0].at));
}

/// 502 and 503 say the flow was not reached, and an outage is what the retries are for.
#[test]
fn a_webhook_behind_something_that_says_it_is_not_there_keeps_being_tried() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(502), Answer::Status(503), Answer::Status(502)]));
    let mut c = r.notifier.config();
    c.webhook.enabled = true;
    r.notifier.update(Some(c), &serde_json::json!({"webhook_url": format!("{}/hook", service.base)})).unwrap();
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(4, Duration::from_secs(12));
    assert_eq!(r.notifier.describe(false)["dropped"], 0);
}

/// ntfy and Telegram do not act and then answer 500: for them it is an outage like any other.
#[test]
fn ntfy_answering_a_server_error_keeps_being_tried() {
    let (r, service) = (rig(), PushService::start(&[Answer::Status(500), Answer::Status(500), Answer::Status(500)]));
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = service.base.clone();
    r.notifier.update(Some(c), &Value::Null).unwrap();
    r.notifier.send(&waiting("s1"), &Context::default());
    service.wait_for(4, Duration::from_secs(12));
    assert_eq!(r.notifier.describe(false)["dropped"], 0);
}

/// Cut off with the request delivered: twice at most, the second after what a channel always waited, so that a blip passes.
#[test]
fn a_connection_cut_after_the_request_is_tried_once_more_after_three_seconds() {
    let (r, service) = (rig(), PushService::start(&[Answer::Close]));
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = service.base.clone();
    r.notifier.update(Some(c), &Value::Null).unwrap();
    r.notifier.send(&waiting("s1"), &Context::default());
    let got = service.wait_for(2, Duration::from_secs(8));
    assert!(got[1].at.duration_since(got[0].at) >= Duration::from_millis(2900), "{:?}", got[1].at.duration_since(got[0].at));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(service.count(), 2);
    assert_eq!(r.notifier.describe(false)["dropped"], 0);
    assert!(!r.outbox(&Dest::Channel(Channel::Ntfy)).exists(), "delivered on the second");
}

fn ntfy_at(r: &Rig, server: &str) {
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = server.into();
    r.notifier.update(Some(c), &Value::Null).unwrap();
}

/// Nothing listens for a moment, and then something does: refused sends nothing, however often, and the message gets there.
#[test]
fn a_blip_where_nothing_was_there_reaches_the_receiver_after_it() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let r = rig();
    ntfy_at(&r, &format!("http://{addr}"));
    r.notifier.send(&waiting("s1"), &Context::default());
    // Through more tries than a cap on what may have arrived would allow.
    std::thread::sleep(Duration::from_secs(4));
    assert_eq!(r.notifier.describe(false)["status"]["ntfy"]["error"], "connection failed");
    assert_eq!(r.notifier.describe(false)["dropped"], 0, "still to go");
    let service = PushService::start_at(addr);
    let got = service.wait_for(1, Duration::from_secs(16));
    assert_eq!(got.len(), 1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(service.count(), 1, "once");
}

/// A TLS handshake that fails has delivered nothing: an `https` address at a server that speaks none.
#[test]
fn a_tls_failure_sent_nothing_and_keeps_being_tried() {
    let service = PushService::start(&[]);
    let r = rig();
    ntfy_at(&r, &service.base.replace("http://", "https://"));
    r.notifier.send(&waiting("s1"), &Context::default());
    let start = Instant::now();
    while r.notifier.describe(false)["status"]["ntfy"]["error"].is_null() && start.elapsed() < Duration::from_secs(8) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_secs(5));
    assert_eq!(r.notifier.describe(false)["dropped"], 0, "past two tries");
    assert_eq!(service.count(), 0, "and none of them was a request");
    assert!(r.outbox(&Dest::Channel(Channel::Ntfy)).exists());
}

/// A name that does not resolve: `.invalid` never does.
#[test]
fn a_name_that_does_not_resolve_keeps_being_tried() {
    let r = rig();
    ntfy_at(&r, "http://kelpie-test-name.invalid");
    r.notifier.send(&waiting("s1"), &Context::default());
    let start = Instant::now();
    while r.notifier.describe(false)["status"]["ntfy"]["error"].is_null() && start.elapsed() < Duration::from_secs(12) {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_secs(3));
    assert!(!r.notifier.describe(false)["status"]["ntfy"]["error"].is_null());
    assert_eq!(r.notifier.describe(false)["dropped"], 0);
    assert!(r.outbox(&Dest::Channel(Channel::Ntfy)).exists());
}

/// A channel's messages keep their title, text and link at any load, as they always did; only a phone's are gathered.
#[test]
fn at_a_busy_load_a_channel_keeps_each_message_as_it_is() {
    let service = PushService::start(&[]);
    let r = rig();
    ntfy_at(&r, &service.base);
    *r.host.level.lock() = Level::L2;
    let telling_of = |title: &str| Context { project: Some("Proj".into()), title: Some(title.into()), phone_url: Some("http://192.168.1.5:4817/".into()), ..Default::default() };
    let start = Instant::now();
    r.notifier.send(&Raised { session: Some("a".into()), ..Raised::bare(Kind::Finished) }, &telling_of("first"));
    r.notifier.send(&Raised { session: Some("b".into()), ..Raised::bare(Kind::Finished) }, &telling_of("second"));
    let got = service.wait_for(2, Duration::from_secs(2));
    assert!(start.elapsed() < Duration::from_secs(2), "not held: {:?}", start.elapsed());
    let bodies: Vec<Value> = got.iter().map(|g| serde_json::from_slice(&g.body).unwrap()).collect();
    assert_eq!((bodies[0]["message"].as_str(), bodies[0]["click"].as_str()), (Some("first: Finished"), Some("http://192.168.1.5:4817/#s=a")));
    assert_eq!((bodies[1]["message"].as_str(), bodies[1]["click"].as_str()), (Some("second: Finished"), Some("http://192.168.1.5:4817/#s=b")));
    assert_eq!(bodies[0]["title"], "Kelpie · Proj");
}

#[test]
fn what_failed_is_not_told_as_an_ordinary_update_after_an_outage() {
    let attempt = Attempt::Together { ids: vec!["e".into()], need_you: 0, updates: 2, urgent: true, latest: 0 };
    assert!(Alert::of(&attempt).high);
    assert_eq!(together_payload(&["e".into()], 0, 2, true, 0).status, "failed");
    assert_eq!(together_payload(&["e".into()], 0, 2, false, 0).status, "summary");
    assert_eq!(together_payload(&["e".into()], 1, 2, true, 0).status, "needs_you");
}

#[test]
fn a_failed_read_of_a_channels_secret_is_tried_again_and_loses_nothing() {
    struct Locked;
    impl SecretStore for Locked {
        fn get(&self, _: &str) -> Option<String> {
            None
        }
        fn read(&self, _: &str) -> anyhow::Result<Option<String>> {
            anyhow::bail!("locked")
        }
        fn set(&self, _: &str, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn delete(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let payload = crate::notify::build_payload(&waiting("s1"), &Context::default(), false);
    for ch in [Channel::Ntfy, Channel::Telegram, Channel::Webhook] {
        let f = deliver(&Locked, &Config::default(), ch, &payload, "http://127.0.0.1:1").unwrap_err();
        assert!(f.retryable(), "{ch:?}: {}", f.reason);
    }
}

#[test]
fn switching_a_channel_off_ends_what_it_had_to_send() {
    let service = PushService::start(&[Answer::Status(503)]);
    let r = rig();
    let mut c = r.notifier.config();
    c.ntfy.enabled = true;
    c.ntfy.server = service.base.clone();
    r.notifier.update(Some(c.clone()), &Value::Null).unwrap();
    r.notifier.send(&waiting("a"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    std::thread::sleep(Duration::from_millis(50));
    c.ntfy.enabled = false;
    r.notifier.update(Some(c), &Value::Null).unwrap();
    assert!(!r.notifier.started());
    assert!(!r.outbox(&Dest::Channel(Channel::Ntfy)).exists());
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(service.count(), 1);
}

/// `Event → enqueue` stays under a millisecond with a destination that does not answer at all.
#[test]
fn handing_a_message_over_takes_well_under_a_millisecond() {
    let (r, service) = (rig(), PushService::start(&[Answer::Hang]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("first"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    let start = Instant::now();
    for i in 0..200 {
        r.notifier.send(&waiting(&format!("s{i}")), &Context::default());
    }
    let each = start.elapsed() / 200;
    assert!(each < Duration::from_millis(1), "{each:?}");
}

#[test]
fn a_full_queue_says_so() {
    let (r, service) = (rig(), PushService::start(&[Answer::Hang]));
    r.phone("d1", &service, "en");
    r.notifier.send(&waiting("first"), &Context::default());
    service.wait_for(1, Duration::from_secs(3));
    for i in 0..(QUEUE + 10) {
        r.notifier.send(&waiting(&format!("s{i}")), &Context::default());
    }
    assert!(r.notifier.describe(false)["dropped"].as_u64().unwrap() >= 10);
}

#[test]
fn a_failed_read_of_the_signing_key_makes_no_new_one() {
    struct Broken;
    impl SecretStore for Broken {
        fn get(&self, _: &str) -> Option<String> {
            None
        }
        fn read(&self, _: &str) -> anyhow::Result<Option<String>> {
            anyhow::bail!("locked")
        }
        fn set(&self, _: &str, _: &str) -> anyhow::Result<()> {
            panic!("nothing is written over a key that could not be read")
        }
        fn delete(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
    }
    let dir = std::env::temp_dir().join(format!("kelpie-vapid-{}", uuid::Uuid::new_v4()));
    let d = Destinations::new(&dir, Arc::new(Broken), Arc::new(NoHost));
    assert!(d.vapid_public(true).is_err());
    let damaged = Memory::new();
    damaged.set(K_VAPID, "not a key").unwrap();
    let d = Destinations::new(&dir, damaged.clone(), Arc::new(NoHost));
    assert!(d.vapid_public(true).unwrap_err().contains("damaged"));
    assert_eq!(damaged.get(K_VAPID).unwrap(), "not a key", "left as it was");
}

#[test]
fn the_signing_key_is_made_once_and_kept() {
    let r = rig();
    assert_eq!(r.dests.vapid_public(false).unwrap(), None, "not before it is needed");
    let first = r.dests.vapid_public(true).unwrap().unwrap();
    assert_eq!(r.dests.vapid_public(true).unwrap().unwrap(), first);
    let kept = r.secrets.get(K_VAPID).unwrap();
    assert!(!r.notifier.describe(true).to_string().contains(&kept), "never read back");
    let again = Destinations::new(&r.dir, r.secrets.clone(), Arc::new(NoHost));
    assert_eq!(again.vapid_public(false).unwrap().unwrap(), first, "a restart reads it");
    r.dests.forget_vapid();
    r.secrets.delete(K_VAPID).unwrap();
    assert_ne!(r.dests.vapid_public(true).unwrap().unwrap(), first, "after a reset, another");
}

#[test]
fn retry_after_is_read_as_seconds_or_a_date() {
    use crate::notify::retry_after;
    assert_eq!(retry_after("120", 0), Some(Duration::from_secs(120)));
    assert_eq!(retry_after(" 5 ", 0), Some(Duration::from_secs(5)));
    assert_eq!(retry_after("Thu, 01 Jan 1970 00:00:10 GMT", 4_000), Some(Duration::from_secs(6)));
    assert_eq!(retry_after("Thu, 01 Jan 1970 00:00:01 GMT", 4_000), Some(Duration::ZERO), "already past");
    assert_eq!(retry_after("soon", 0), None);
}
