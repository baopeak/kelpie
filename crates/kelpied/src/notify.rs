//! One source of "something needs you" events, and the channels that carry them off the
//! computer (ntfy, Telegram, a webhook).
//!
//! The daemon classifies session status changes ([`classify`]); the app shows the same events in
//! its inbox and as desktop notifications (`notify.event`), and the channels send them to a phone.
//! Nothing here runs unless a channel is switched on: no thread, no socket, no timer. Secrets
//! (topic, tokens, webhook address) live in the OS credential store through [`SecretStore`];
//! `notify.json` holds only what is safe to read. Text leaving the computer passes
//! `kelpie_proto::redact`, request addresses are never logged and are not part of any error text.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::persist::write_atomic;
use crate::session::{now_millis, SessionInfo};
use crate::status::Status;

/// One message per session in this window; the rest are dropped.
pub const RATE_WINDOW: Duration = Duration::from_secs(30);
/// "Computer idle" for the away filter: no keyboard or mouse input for this long.
const IDLE_AFTER: Duration = Duration::from_secs(300);
const MAX_VISIBLE: usize = 256;
/// Messages waiting for the sending thread; a full queue drops the newest, and a message that waited
/// longer than `STALE` is dropped instead of sent late. Both are counted and shown in the settings.
const QUEUE: usize = 32;
const STALE: Duration = Duration::from_secs(300);

const K_TOPIC: &str = "notify-ntfy-topic";
const K_NTFY_TOKEN: &str = "notify-ntfy-token";
const K_TELEGRAM_TOKEN: &str = "notify-telegram-token";
const K_WEBHOOK: &str = "notify-webhook-url";
const SECRET_KEYS: [&str; 4] = [K_TOPIC, K_NTFY_TOKEN, K_TELEGRAM_TOKEN, K_WEBHOOK];

// ---------- secrets ----------

/// Where channel secrets are kept.
pub trait SecretStore: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// The OS credential store, in the service named after this daemon's endpoint: a test daemon
/// (own `KELPIE_ENDPOINT`) never sees the real app's entries.
pub struct Keyring;

impl SecretStore for Keyring {
    fn get(&self, key: &str) -> Option<String> {
        kelpie_proto::secret_get(key)
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        kelpie_proto::secret_set(key, value).map_err(|e| anyhow!("credential store: {e}"))
    }
    fn delete(&self, key: &str) -> Result<()> {
        kelpie_proto::secret_delete(key).map_err(|e| anyhow!("credential store: {e}"))
    }
}

/// 128 random bits as hex (the OS generator through v4 UUIDs; the bytes of the first that hold
/// the version and variant are replaced by fully random bytes of the second).
pub fn random_topic() -> String {
    let (a, b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let mut bytes = *a.as_bytes();
    bytes[6] = b.as_bytes()[0];
    bytes[8] = b.as_bytes()[1];
    bytes.iter().map(|x| format!("{x:02x}")).collect()
}

// ---------- configuration ----------

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ntfy {
    pub enabled: bool,
    pub server: String,
}

impl Default for Ntfy {
    fn default() -> Self {
        Ntfy { enabled: false, server: "https://ntfy.sh".into() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Telegram {
    pub enabled: bool,
    pub chat_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Webhook {
    pub enabled: bool,
}

/// Which events leave the computer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Events {
    pub needs_you: bool,
    pub done: bool,
    pub failed: bool,
    pub automation: bool,
    pub approval: bool,
    pub cost_cap: bool,
}

impl Default for Events {
    fn default() -> Self {
        Events { needs_you: true, done: true, failed: true, automation: true, approval: true, cost_cap: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ntfy: Ntfy,
    pub telegram: Telegram,
    pub webhook: Webhook,
    pub events: Events,
    /// A finished turn shorter than this many seconds is not sent.
    pub min_turn_s: u32,
    /// Send only while the window is not focused or the computer is idle.
    pub away_only: bool,
    /// Add the end of the session's screen (redacted) to the message.
    pub include_screen: bool,
    /// Add a link to the session in the phone app (it holds this computer's address on the network).
    pub phone_link: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config { ntfy: Ntfy::default(), telegram: Telegram::default(), webhook: Webhook::default(), events: Events::default(), min_turn_s: 30, away_only: false, include_screen: false, phone_link: true }
    }
}

impl Config {
    fn any_enabled(&self) -> bool {
        self.ntfy.enabled || self.telegram.enabled || self.webhook.enabled
    }
}

/// An http(s) address on this computer.
fn is_loopback(url: &str) -> bool {
    let rest = url.strip_prefix("http://").or_else(|| url.strip_prefix("https://")).unwrap_or("");
    matches!(rest.split(['/', ':']).next(), Some("127.0.0.1" | "localhost"))
}

fn http_url(s: &str) -> bool {
    let rest = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://"));
    rest.is_some_and(|r| !r.is_empty() && !r.contains(['@', '#', ' ']) && !r.starts_with('/') && !s.chars().any(char::is_control))
}

/// Checks what the app may set; nothing is stored when anything is wrong.
fn validate_config(c: &Config) -> Result<()> {
    if c.ntfy.server.len() > 200 || !http_url(&c.ntfy.server) {
        bail!("the ntfy server must be an http(s) address without a login or fragment");
    }
    let chat = &c.telegram.chat_id;
    let numeric = chat.strip_prefix('-').unwrap_or(chat);
    let ok = chat.len() <= 64 && ((!numeric.is_empty() && numeric.chars().all(|x| x.is_ascii_digit())) || (chat.starts_with('@') && chat.len() > 1 && chat[1..].chars().all(|x| x.is_ascii_alphanumeric() || x == '_')));
    if !chat.is_empty() && !ok {
        bail!("the Telegram chat id is a number (or @channelname)");
    }
    if c.min_turn_s > 86_400 {
        bail!("the turn length filter is at most a day");
    }
    Ok(())
}

fn validate_secret(key: &str, v: &str) -> Result<()> {
    let plain = !v.is_empty() && !v.chars().any(|c| c.is_control() || c.is_whitespace());
    match key {
        K_WEBHOOK if v.len() > 1000 || !http_url(v) => bail!("the webhook address must be an http(s) address"),
        K_NTFY_TOKEN if !plain || v.len() > 200 => bail!("the ntfy access token has no spaces and at most 200 characters"),
        K_TELEGRAM_TOKEN if !plain || v.len() > 120 || !v.contains(':') => bail!("the Telegram bot token looks like 123456:ABC… (from @BotFather)"),
        _ => Ok(()),
    }
}

// ---------- classification ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The agent needs the user (inbox kind `waiting`).
    Waiting,
    Finished,
    Error,
    Automation,
    Approval,
    Cost,
}

impl Kind {
    pub fn code(self) -> &'static str {
        match self {
            Kind::Waiting => "waiting",
            Kind::Finished => "finished",
            Kind::Error => "error",
            Kind::Automation => "automation",
            Kind::Approval => "approval",
            Kind::Cost => "cost",
        }
    }

    /// The word the payload's `status` carries.
    fn status(self) -> &'static str {
        match self {
            Kind::Waiting => "needs_you",
            Kind::Finished => "done",
            Kind::Error => "failed",
            Kind::Automation => "automation",
            Kind::Approval => "approval",
            Kind::Cost => "cost_cap",
        }
    }

    fn enabled(self, e: &Events) -> bool {
        match self {
            Kind::Waiting => e.needs_you,
            Kind::Finished => e.done,
            Kind::Error => e.failed,
            Kind::Automation => e.automation,
            Kind::Approval => e.approval,
            Kind::Cost => e.cost_cap,
        }
    }
}

/// A session status change worth telling the user about: a turn that ended (or a process that
/// died) in an error, once when it happens, or work that stopped to wait or finish.
pub fn classify(prev_status: Status, prev_failed: bool, next: &SessionInfo) -> Option<Kind> {
    let failed = next.failed && !prev_failed;
    if failed {
        return Some(Kind::Error);
    }
    if prev_status != Status::Working || !matches!(next.status, Status::Idle | Status::Waiting) {
        return None;
    }
    Some(if next.status == Status::Waiting { Kind::Waiting } else { Kind::Finished })
}

struct Seen {
    status: Status,
    failed: bool,
    turn_start: Option<Instant>,
}

/// An event, as the app and the channels get it.
#[derive(Clone, Debug)]
pub struct Raised {
    pub kind: Kind,
    pub session: Option<String>,
    pub project_id: Option<String>,
    pub alive: bool,
    pub exit_code: Option<i32>,
    /// Automation name, run and task, or the cost sentence.
    pub detail: Option<String>,
    pub error: Option<String>,
    /// An approval's run and task names (the app words the title from them).
    pub run: Option<(String, String)>,
    pub turn: Option<Duration>,
    pub at: u64,
}

impl Raised {
    pub(crate) fn bare(kind: Kind) -> Raised {
        Raised { kind, session: None, project_id: None, alive: true, exit_code: None, detail: None, error: None, run: None, turn: None, at: now_millis() }
    }

    pub fn automation(name: &str, session: Option<String>, error: Option<String>) -> Raised {
        Raised { session, detail: Some(name.into()), error, ..Raised::bare(Kind::Automation) }
    }

    pub fn approval(run: &str, task: &str, project_id: Option<String>) -> Raised {
        Raised { project_id, detail: Some(format!("{run} · {task}")), run: Some((run.into(), task.into())), ..Raised::bare(Kind::Approval) }
    }

    pub fn cost(session: &str, project_id: Option<String>, detail: Option<String>) -> Raised {
        Raised { session: Some(session.into()), project_id, detail, ..Raised::bare(Kind::Cost) }
    }

    pub fn to_json(&self) -> Value {
        json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "kind": self.kind.code(),
            "session": self.session,
            "project_id": self.project_id,
            "alive": self.alive,
            "exit_code": self.exit_code,
            "detail": self.detail,
            "run": self.run.as_ref().map(|r| &r.0),
            "task": self.run.as_ref().map(|r| &r.1),
            "error": self.error,
            "at": self.at,
        })
    }

    /// What the rate limit counts: the session, else the thing the event is about.
    fn key(&self) -> String {
        match (&self.session, self.kind) {
            // The app reports a cap seconds after the turn's own message: it has its own allowance.
            (Some(s), Kind::Cost) => format!("cost:{s}"),
            (Some(s), _) => s.clone(),
            (None, _) => format!("{}:{}", self.kind.code(), self.detail.as_deref().unwrap_or_default()),
        }
    }
}

// ---------- rate limit ----------

/// One message per key per window.
pub struct RateLimiter {
    window: Duration,
    last: HashMap<String, Instant>,
}

impl RateLimiter {
    pub fn new(window: Duration) -> Self {
        RateLimiter { window, last: HashMap::new() }
    }

    /// True (and counted) when `key` may send now.
    pub fn allow(&mut self, key: &str, now: Instant) -> bool {
        if self.last.len() > 512 {
            self.last.retain(|_, t| now.duration_since(*t) < self.window);
        }
        match self.last.get(key) {
            Some(t) if now.duration_since(*t) < self.window => false,
            _ => {
                self.last.insert(key.to_string(), now);
                true
            }
        }
    }
}

// ---------- the message ----------

/// What the daemon knows around an event, gathered only when it is going to be sent.
#[derive(Default, Clone, Debug)]
pub struct Context {
    pub project: Option<String>,
    pub title: Option<String>,
    pub screen: Option<String>,
    /// The phone web app's address, when phone access is on.
    pub phone_url: Option<String>,
}

/// The message as every channel gets it. All text is redacted.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Payload {
    pub status: &'static str,
    pub project: Option<String>,
    pub session: Option<String>,
    pub summary: String,
    pub duration_s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    pub at: u64,
}

const SCREEN_CHARS: usize = 1000;

/// A link to a session in the phone web app. The page needs a device that is already paired:
/// the link carries no pairing code and no token, and any query or fragment of the base is dropped.
pub fn deep_link(base: &str, session: &str) -> Option<String> {
    let base = base.split(['?', '#']).next().unwrap_or(base);
    if !http_url(base) || session.is_empty() || session.len() > 64 || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    Some(format!("{}/#s={session}", base.trim_end_matches('/')))
}

fn summary(ev: &Raised) -> String {
    match ev.kind {
        Kind::Waiting => "Needs your input".into(),
        Kind::Finished => "Finished".into(),
        Kind::Error if !ev.alive => format!("Exited with code {}", ev.exit_code.map_or(String::new(), |c| c.to_string())),
        Kind::Error => "Stopped with an error".into(),
        Kind::Automation => match &ev.error {
            Some(e) => format!("Automation failed: {e}"),
            None => "Automation started".into(),
        },
        Kind::Approval => "Waiting for your approval to start".into(),
        Kind::Cost => ev.detail.clone().unwrap_or_else(|| "Passed the cost cap".into()),
    }
}

pub fn build_payload(ev: &Raised, ctx: &Context, include_screen: bool) -> Payload {
    use kelpie_proto::redact;
    let session = match ev.kind {
        Kind::Automation | Kind::Approval => ev.detail.as_deref(),
        _ => ctx.title.as_deref(),
    };
    let screen = include_screen
        .then_some(ctx.screen.as_deref())
        .flatten()
        .map(|s| {
            let s = redact(s.trim_end());
            let skip = s.chars().count().saturating_sub(SCREEN_CHARS);
            s.chars().skip(skip).collect::<String>()
        })
        .filter(|s| !s.trim().is_empty());
    let link = ev.session.as_deref().zip(ctx.phone_url.as_deref()).and_then(|(s, base)| deep_link(base, s));
    // Names and titles come from the program in the session (its terminal title): one plain line.
    let line = |s: &str| redact(&s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(200).collect::<String>());
    Payload {
        status: ev.kind.status(),
        project: ctx.project.as_deref().map(line),
        session: session.map(line),
        summary: line(&summary(ev)),
        duration_s: ev.turn.map(|d| d.as_secs()),
        screen,
        link,
        at: ev.at,
    }
}

fn duration_text(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

impl Payload {
    pub fn title(&self) -> String {
        match &self.project {
            Some(p) => format!("Kelpie · {p}"),
            None => "Kelpie".into(),
        }
    }

    /// The text of the message: what happened, to which session, how long it took, and the screen.
    pub fn text(&self) -> String {
        let mut out = match &self.session {
            Some(s) => format!("{s}: {}", self.summary),
            None => self.summary.clone(),
        };
        if let Some(d) = self.duration_s.filter(|d| *d > 0) {
            out.push_str(&format!(" ({})", duration_text(d)));
        }
        if let Some(screen) = &self.screen {
            out.push_str("\n\n");
            out.push_str(screen);
        }
        out
    }
}

// ---------- channels ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Channel {
    Ntfy,
    Telegram,
    Webhook,
}

impl Channel {
    pub fn parse(s: &str) -> Option<Channel> {
        match s {
            "ntfy" => Some(Channel::Ntfy),
            "telegram" => Some(Channel::Telegram),
            "webhook" => Some(Channel::Webhook),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Channel::Ntfy => "ntfy",
            Channel::Telegram => "telegram",
            Channel::Webhook => "webhook",
        }
    }
}

/// The Telegram API's address. Tests may point it at a local fake (`KELPIE_NOTIFY_TELEGRAM_BASE`),
/// but only for a loopback address and only in a test run (`KELPIE_E2E`): a bot token is never
/// sent to an address the environment chose.
pub fn telegram_base() -> String {
    telegram_base_for(std::env::var("KELPIE_E2E").is_ok(), std::env::var("KELPIE_NOTIFY_TELEGRAM_BASE").ok().as_deref())
}

fn telegram_base_for(test_run: bool, over: Option<&str>) -> String {
    let loopback = |u: &str| {
        let rest = u.strip_prefix("http://").unwrap_or("");
        let host = rest.split(['/', ':']).next().unwrap_or("");
        matches!(host, "127.0.0.1" | "localhost") && !rest.contains('@')
    };
    match over {
        Some(u) if test_run && loopback(u) => u.trim_end_matches('/').to_string(),
        _ => "https://api.telegram.org".into(),
    }
}

/// Why a send failed, without the address it went to.
fn post_json(url: &str, headers: &[(&str, &str)], body: &Value) -> std::result::Result<(), String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .http_status_as_error(false)
        .max_redirects(0)
        .build()
        .into();
    let mut req = agent.post(url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let text = body.to_string();
    match req.content_type("application/json").send(text.as_str()) {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => Err(format!("HTTP {}", resp.status().as_u16())),
        Err(ureq::Error::Timeout(_)) => Err("timed out".into()),
        Err(ureq::Error::HostNotFound) => Err("host not found".into()),
        Err(ureq::Error::ConnectionFailed) => Err("connection failed".into()),
        Err(ureq::Error::Io(e)) if matches!(e.kind(), std::io::ErrorKind::ConnectionRefused | std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => Err("connection failed".into()),
        Err(_) => Err("network error".into()),
    }
}

/// Transient: worth one more try.
fn retryable(reason: &str) -> bool {
    matches!(reason, "timed out" | "connection failed" | "network error" | "HTTP 429") || reason.starts_with("HTTP 5")
}

#[derive(Clone, Debug, Default, Serialize, PartialEq)]
pub struct ChannelStatus {
    pub ok_at: Option<u64>,
    pub error: Option<String>,
}

enum Job {
    Send(Payload, Instant),
}

// ---------- the notifier ----------

#[derive(Default)]
struct Ui {
    focused: bool,
    visible: HashSet<String>,
    /// When the person last used the window (wall-clock ms, as the app saw it); none from an app that does not say.
    active_at: Option<u64>,
}

pub struct Notifier {
    path: PathBuf,
    secrets: Arc<dyn SecretStore>,
    config: Arc<Mutex<Config>>,
    /// Mirror of "some channel is on", read on every status change.
    active: AtomicBool,
    ui: Mutex<HashMap<u64, Ui>>,
    seen: Mutex<HashMap<u32, Seen>>,
    waiting_tasks: Mutex<HashSet<(String, String)>>,
    limiter: Mutex<RateLimiter>,
    status: Arc<Mutex<HashMap<&'static str, ChannelStatus>>>,
    /// The sending thread's queue; none until the first message.
    worker: Mutex<Option<mpsc::SyncSender<Job>>>,
    /// Messages not sent because the queue was full or they waited too long.
    dropped: Arc<AtomicU64>,
    /// Test hook: how long the computer has been idle.
    #[cfg(test)]
    idle: Mutex<Option<Duration>>,
}

impl Notifier {
    pub fn new(data_dir: &std::path::Path, secrets: Arc<dyn SecretStore>) -> Notifier {
        let path = data_dir.join("notify.json");
        let config: Config = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Notifier {
            path,
            secrets,
            active: AtomicBool::new(config.any_enabled()),
            config: Arc::new(Mutex::new(config)),
            ui: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            waiting_tasks: Mutex::new(HashSet::new()),
            limiter: Mutex::new(RateLimiter::new(RATE_WINDOW)),
            status: Arc::new(Mutex::new(HashMap::new())),
            worker: Mutex::new(None),
            dropped: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            idle: Mutex::new(None),
        }
    }

    // ----- what the app shows -----

    /// The app reports its window (`ui.focus`): focused or not, and the sessions on screen.
    pub fn set_ui(&self, client: u64, focused: bool, visible: Vec<String>, active_at: Option<u64>) {
        let visible = visible.into_iter().filter(|s| s.len() <= 64).take(MAX_VISIBLE).collect();
        self.ui.lock().insert(client, Ui { focused, visible, active_at });
    }

    pub fn drop_client(&self, client: u64) {
        self.ui.lock().remove(&client);
    }

    /// Some window has focus and shows this session, and the person is there: it needs no
    /// notification. A computer left idle for five minutes with Kelpie in front is not being looked at.
    pub fn looking_at(&self, session: &str) -> bool {
        let shown = self.ui.lock().values().any(|u| u.focused && u.visible.contains(session));
        shown && !self.idle()
    }

    /// No keyboard or mouse input for [`IDLE_AFTER`] (where the system says).
    fn idle(&self) -> bool {
        // Tests decide for themselves: the machine they run on may well be idle.
        #[cfg(test)]
        return self.idle.lock().is_some_and(|d| d >= IDLE_AFTER);
        #[cfg(not(test))]
        self.idle_at(os_idle(), now_millis())
    }

    /// Idle by the system's clock where it has one; elsewhere (macOS, Linux) when every focused window
    /// says its last keyboard or pointer use is that long ago. An app that reports nothing is never idle.
    fn idle_at(&self, system: Option<Duration>, now_ms: u64) -> bool {
        if let Some(i) = system {
            return i >= IDLE_AFTER;
        }
        let ui = self.ui.lock();
        let mut focused = ui.values().filter(|u| u.focused).peekable();
        focused.peek().is_some() && focused.all(|u| u.active_at.is_some_and(|a| now_ms.saturating_sub(a) >= IDLE_AFTER.as_millis() as u64))
    }

    fn window_focused(&self) -> bool {
        self.ui.lock().values().any(|u| u.focused)
    }

    // ----- classification -----

    /// Remembers a session the app already knows (its creation or restore), so its first change
    /// can be told from a first sighting.
    pub fn seed(&self, info: &SessionInfo) {
        self.seen.lock().insert(info.handle, Seen { status: info.status, failed: info.failed, turn_start: None });
    }

    pub fn forget(&self, handle: u32) {
        self.seen.lock().remove(&handle);
    }

    /// Takes one status change; returns the event it amounts to, if any. Cheap: a map lookup and
    /// two comparisons, run for every changed session on the 250 ms tick.
    pub fn observe(&self, info: &SessionInfo, now: Instant) -> Option<Raised> {
        let mut seen = self.seen.lock();
        let Some(prev) = seen.get_mut(&info.handle) else {
            seen.insert(info.handle, Seen { status: info.status, failed: info.failed, turn_start: None });
            return None;
        };
        let kind = classify(prev.status, prev.failed, info);
        let turn = prev.turn_start.map(|t| now.duration_since(t));
        prev.turn_start = match info.status {
            Status::Working if matches!(prev.status, Status::Idle | Status::Exited) => Some(now),
            Status::Working | Status::Waiting => prev.turn_start,
            _ => None,
        };
        prev.status = info.status;
        prev.failed = info.failed;
        drop(seen);
        kind.map(|kind| Raised {
            kind,
            session: Some(info.spec.id.clone()),
            project_id: info.spec.project_id.clone(),
            alive: info.alive,
            exit_code: info.exit_code,
            turn,
            ..Raised::bare(kind)
        })
    }

    /// A run's tasks that now wait for approval and did not before.
    pub fn new_approvals(&self, run_id: &str, run: &str, project_id: Option<&str>, tasks: &[(String, String, bool)]) -> Vec<Raised> {
        let mut set = self.waiting_tasks.lock();
        let mut out = Vec::new();
        for (id, title, waiting) in tasks {
            let key = (run_id.to_string(), id.clone());
            if !waiting {
                set.remove(&key);
            } else if set.insert(key) {
                out.push(Raised::approval(run, title, project_id.map(str::to_string)));
            }
        }
        out
    }

    /// A run went away: its waiting tasks are forgotten.
    pub fn forget_run(&self, run_id: &str) {
        self.waiting_tasks.lock().retain(|(r, _)| r != run_id);
    }

    // ----- channels -----

    pub fn config(&self) -> Config {
        self.config.lock().clone()
    }

    /// A channel is on (cheap; the common case is no).
    pub fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Whether the event leaves the computer, counting it against the rate limit when it does.
    /// The caller has already dropped what the user is looking at.
    pub fn wants_push(&self, ev: &Raised, now: Instant) -> bool {
        if !self.active() {
            return false;
        }
        let c = self.config.lock();
        if !ev.kind.enabled(&c.events) {
            return false;
        }
        if ev.kind == Kind::Finished && ev.turn.is_some_and(|t| t < Duration::from_secs(c.min_turn_s as u64)) {
            return false;
        }
        if c.away_only && self.window_focused() && !self.idle() {
            return false;
        }
        drop(c);
        self.limiter.lock().allow(&ev.key(), now)
    }

    /// Queues the message for every channel that is on. The first one starts the sending thread.
    /// A full queue drops the message (counted, shown in the settings, logged).
    pub fn send(&self, ev: &Raised, ctx: &Context) {
        let payload = build_payload(ev, ctx, self.config.lock().include_screen);
        let mut worker = self.worker.lock();
        if worker.is_none() {
            let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE);
            let (secrets, status, config, dropped) = (self.secrets.clone(), self.status.clone(), self.config.clone(), self.dropped.clone());
            let spawned = std::thread::Builder::new().name("kelpie-notify".into()).spawn(move || {
                while let Ok(Job::Send(p, queued)) = rx.recv() {
                    if queued.elapsed() > STALE {
                        dropped.fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let c = config.lock().clone();
                    for ch in [Channel::Ntfy, Channel::Telegram, Channel::Webhook] {
                        if !channel_on(&c, ch) {
                            continue;
                        }
                        let mut result = deliver(secrets.as_ref(), &c, ch, &p, &telegram_base());
                        if result.as_ref().is_err_and(|e| retryable(e)) {
                            std::thread::sleep(Duration::from_secs(3));
                            result = deliver(secrets.as_ref(), &c, ch, &p, &telegram_base());
                        }
                        record(&status, ch, &result);
                    }
                }
            });
            match spawned {
                Ok(_) => *worker = Some(tx),
                Err(e) => {
                    // Nothing is stored: the next message tries again.
                    tracing::warn!("notify worker: {e}");
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
        if let Some(tx) = &*worker {
            if tx.try_send(Job::Send(payload, Instant::now())).is_err() {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                tracing::warn!("notify: the queue is full, a message was dropped");
            }
        }
    }

    /// Whether a message was ever queued (the worker thread exists).
    #[cfg(test)]
    pub fn started(&self) -> bool {
        self.worker.lock().is_some()
    }

    /// Sends a test message to one channel now, on the caller's thread.
    pub fn test(&self, ch: Channel) -> std::result::Result<(), String> {
        self.test_via(ch, &telegram_base())
    }

    fn test_via(&self, ch: Channel, telegram: &str) -> std::result::Result<(), String> {
        let ev = Raised { detail: Some("Test notification".into()), ..Raised::bare(Kind::Automation) };
        let payload = Payload { summary: "Your Kelpie notifications work".into(), ..build_payload(&ev, &Context::default(), false) };
        let result = deliver(self.secrets.as_ref(), &self.config(), ch, &payload, telegram);
        record(&self.status, ch, &result);
        result
    }

    // ----- RPCs -----

    /// The settings as the app sees them: no secret value, only whether each is set.
    pub fn describe(&self, reveal_topic: bool) -> Value {
        let set = |k: &str| self.secrets.get(k).is_some();
        let status = self.status.lock().iter().map(|(k, v)| (k.to_string(), json!(v))).collect::<serde_json::Map<_, _>>();
        // What the windows reported with `ui.focus`.
        let windows = {
            let ui = self.ui.lock();
            json!({"count": ui.len(), "focused": ui.values().any(|u| u.focused), "visible": ui.values().filter(|u| u.focused).flat_map(|u| u.visible.iter().cloned()).collect::<Vec<_>>()})
        };
        let mut v = json!({
            "config": self.config(),
            "secrets": {"ntfy_topic": set(K_TOPIC), "ntfy_token": set(K_NTFY_TOKEN), "telegram_token": set(K_TELEGRAM_TOKEN), "webhook_url": set(K_WEBHOOK)},
            "status": status,
            "windows": windows,
            "dropped": self.dropped.load(Ordering::Relaxed),
        });
        if reveal_topic {
            v["ntfy_topic"] = json!(self.secrets.get(K_TOPIC));
        }
        v
    }

    /// Replaces the configuration and changes the secrets named in `secrets` (a string sets,
    /// null removes). Switching ntfy on creates its random topic.
    pub fn update(&self, config: Option<Config>, secrets: &Value) -> Result<()> {
        let mut changes: Vec<(&str, Option<String>)> = Vec::new();
        for (field, key) in [("ntfy_token", K_NTFY_TOKEN), ("telegram_token", K_TELEGRAM_TOKEN), ("webhook_url", K_WEBHOOK)] {
            match secrets.get(field) {
                None => {}
                Some(Value::Null) => changes.push((key, None)),
                Some(Value::String(s)) if s.is_empty() => changes.push((key, None)),
                Some(Value::String(s)) => {
                    validate_secret(key, s)?;
                    changes.push((key, Some(s.clone())));
                }
                Some(_) => bail!("{field} must be text"),
            }
        }
        if let Some(c) = &config {
            validate_config(c)?;
        }
        // A bearer token is never sent in clear to another computer.
        let token_after = changes.iter().find(|(k, _)| *k == K_NTFY_TOKEN).map_or_else(|| self.secrets.get(K_NTFY_TOKEN).is_some(), |(_, v)| v.is_some());
        let server = config.as_ref().map_or_else(|| self.config.lock().ntfy.server.clone(), |c| c.ntfy.server.clone());
        if token_after && server.starts_with("http://") && !is_loopback(&server) {
            bail!("an ntfy access token is sent only to an https server: use https, or remove the token");
        }
        for (key, value) in &changes {
            match value {
                Some(v) => self.secrets.set(key, v)?,
                None => self.secrets.delete(key)?,
            }
        }
        if let Some(c) = config {
            if c.ntfy.enabled && self.secrets.get(K_TOPIC).is_none() {
                self.secrets.set(K_TOPIC, &random_topic())?;
            }
            write_atomic(&self.path, &serde_json::to_vec_pretty(&c)?)?;
            self.active.store(c.any_enabled(), Ordering::Relaxed);
            *self.config.lock() = c;
        }
        Ok(())
    }

    /// A new topic: whoever had the old one stops receiving.
    pub fn rotate_topic(&self) -> Result<String> {
        let topic = random_topic();
        self.secrets.set(K_TOPIC, &topic)?;
        Ok(topic)
    }

    /// Switches every channel off and removes every secret.
    pub fn clear(&self) -> Result<()> {
        for key in SECRET_KEYS {
            self.secrets.delete(key)?;
        }
        self.update(Some(Config::default()), &Value::Null)
    }
}

fn channel_on(c: &Config, ch: Channel) -> bool {
    match ch {
        Channel::Ntfy => c.ntfy.enabled,
        Channel::Telegram => c.telegram.enabled,
        Channel::Webhook => c.webhook.enabled,
    }
}

fn record(status: &Mutex<HashMap<&'static str, ChannelStatus>>, ch: Channel, result: &std::result::Result<(), String>) {
    let mut s = status.lock();
    let entry = s.entry(ch.code()).or_default();
    match result {
        Ok(()) => *entry = ChannelStatus { ok_at: Some(now_millis()), error: None },
        Err(e) => {
            entry.error = Some(e.clone());
            tracing::warn!("notify {}: {e}", ch.code());
        }
    }
}

/// Sends `p` through one channel. Errors say what failed, never where.
fn deliver(secrets: &dyn SecretStore, c: &Config, ch: Channel, p: &Payload, telegram: &str) -> std::result::Result<(), String> {
    match ch {
        Channel::Ntfy => {
            let topic = secrets.get(K_TOPIC).ok_or("no topic")?;
            let mut body = json!({"topic": topic, "title": p.title(), "message": p.text(), "priority": if matches!(p.status, "needs_you" | "failed" | "approval") { 4 } else { 3 }});
            if let Some(link) = &p.link {
                body["click"] = json!(link);
            }
            let bearer = secrets.get(K_NTFY_TOKEN).map(|t| format!("Bearer {t}"));
            let headers: Vec<(&str, &str)> = bearer.iter().map(|b| ("Authorization", b.as_str())).collect();
            post_json(c.ntfy.server.trim_end_matches('/'), &headers, &body)
        }
        Channel::Telegram => {
            let token = secrets.get(K_TELEGRAM_TOKEN).ok_or("no bot token")?;
            if c.telegram.chat_id.is_empty() {
                return Err("no chat id".into());
            }
            let mut text = format!("{}\n{}", p.title(), p.text());
            if let Some(link) = &p.link {
                text.push_str(&format!("\n\n{link}"));
            }
            let url = format!("{telegram}/bot{token}/sendMessage");
            post_json(&url, &[], &json!({"chat_id": c.telegram.chat_id, "text": text, "disable_web_page_preview": true}))
        }
        Channel::Webhook => {
            let url = secrets.get(K_WEBHOOK).ok_or("no address")?;
            post_json(&url, &[], &json!({"source": "kelpie", "title": p.title(), "text": p.text(), "event": p}))
        }
    }
}

/// Time since the last keyboard or mouse input, where the system tells.
#[cfg(windows)]
fn os_idle() -> Option<Duration> {
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
    let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: `info` is a valid, initialised LASTINPUTINFO with cbSize set.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // SAFETY: no arguments.
    let now = unsafe { GetTickCount() };
    Some(Duration::from_millis(now.wrapping_sub(info.dwTime) as u64))
}

#[cfg(not(windows))]
fn os_idle() -> Option<Duration> {
    None
}

/// A terminal title without the spinner or status glyph agents put in front of it.
fn strip_glyph(title: &str) -> &str {
    let mut chars = title.chars();
    match chars.next() {
        Some(c) if ('\u{2800}'..='\u{28FF}').contains(&c) || "✳✶✻✽·*◐◑◒◓".contains(c) => chars.as_str().trim(),
        _ => title.trim(),
    }
}

/// Name of a session in a message: its own name, else the agent and what its title says.
pub fn session_label(name: Option<&str>, title: Option<&str>, kind: &str, kind_label: &str, remote: Option<&str>) -> String {
    let base = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => n.to_string(),
        None => {
            let clean = title.map(strip_glyph).unwrap_or("");
            let same = clean.eq_ignore_ascii_case(kind) || clean.eq_ignore_ascii_case(kind_label);
            let drive_path = clean.as_bytes().get(1..3) == Some(b":\\") && clean.as_bytes()[0].is_ascii_alphabetic();
            if !clean.is_empty() && !same && clean.chars().count() < 48 && !drive_path && !clean.to_lowercase().ends_with(".exe") {
                if kind == "shell" { clean.to_string() } else { format!("{kind_label} · {clean}") }
            } else {
                kind_label.to_string()
            }
        }
    };
    match remote {
        Some(dest) => format!("{base} @{dest}"),
        None => base,
    }
}

#[cfg(test)]
pub struct Memory(pub Mutex<HashMap<String, String>>);

#[cfg(test)]
impl Memory {
    pub fn new() -> Arc<Memory> {
        Arc::new(Memory(Mutex::new(HashMap::new())))
    }
}

#[cfg(test)]
impl SecretStore for Memory {
    fn get(&self, key: &str) -> Option<String> {
        self.0.lock().get(key).cloned()
    }
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.0.lock().insert(key.into(), value.into());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.0.lock().remove(key);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
