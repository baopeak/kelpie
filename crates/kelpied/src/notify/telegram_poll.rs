//! Approving a low-risk request from Telegram, for the person who turned it on. Telegram reads these messages, a bot chat is not end-to-end
//! encrypted and a button press asks for no Face ID: so it is off by default, and only what the daemon labels `low` (`mobile::risk`) gets
//! buttons, for its one-time choice. Everything else is an alert with a link.
//!
//! - **Bound.** Turning it on shows a code; the first `/bind <code>` the bot gets within five minutes, in the chat set for Telegram, names the
//!   account that sent it. A press from any other account or chat does nothing: such presses are counted and noted in the audit log once a
//!   minute, as are presses of buttons that are no longer valid.
//! - **One-shot.** Each message's buttons carry a 128-bit nonce, kept in memory only: used once, and gone after five minutes, with the request,
//!   with a restart and with the kill switch. Six approvals a minute at most; past that, messages go without buttons until the minute is over.
//! - **The existing answer paths.** A press answers as `telegram` through `Session::answer_by` or `ChatSession::answer`: the first answer wins,
//!   from wherever it comes. Every approval and refusal goes to the phones' audit log.
//! - **Outbound only.** One thread long-polls `getUpdates` while approvals are on or a binding waits, with no listener anywhere. Telegram
//!   refuses the polling for a bot that has a webhook: that is shown, as is a token it rejects, which stops it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, MutexGuard};
use serde_json::{json, Value};

use super::destinations::Host;
use super::outbox::Entry;
use super::{random_topic, telegram_base, telegram_call, telegram_text, Config, Failure, Kind, Payload, SecretStore, TelegramMode, K_TELEGRAM_TOKEN};
use crate::persist::write_atomic;

/// How long a message's buttons work, and how long a binding code does.
pub const OFFER_FOR: Duration = Duration::from_secs(300);
pub const BIND_FOR: Duration = Duration::from_secs(300);
/// What one `getUpdates` waits for, at most.
const POLL_S: u64 = 25;
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const MINUTE: Duration = Duration::from_secs(60);
const APPROVALS_PER_MINUTE: u32 = 6;
/// Buttons open at once. Past this, messages go without.
const MAX_OFFERS: usize = 64;
/// The most of a command a message with buttons shows. It is shown whole or not at all, so a longer one gets no buttons.
const MAX_ASKED: usize = 2000;
/// Wrong codes that end a binding.
const MAX_WRONG: u32 = 5;
const BACKOFF_MAX_S: u64 = 60;
/// The most updates one `getUpdates` hands over: a reply stays well under what `telegram_call` reads, however busy the chat.
const UPDATES_PER_POLL: u32 = 20;
/// The least time between two polls, for a server that answers at once with nothing.
const MIN_POLL: Duration = Duration::from_secs(1);
/// The account Telegram shows for a group's anonymous admins: any of them, so nobody in particular.
const ANONYMOUS_ADMIN: i64 = 1_087_968_824;
/// How much of the binder's name is kept.
const NAME_MAX: usize = 64;

/// What a button answers.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// A terminal's prompt, in the wait that began at `wait`: the key that says yes this once and the one that says no.
    Term { wait: u64, yes: String, no: String },
    /// A chat's request, and the lines in its agent's dialect that approve it once and refuse it.
    Chat { request_id: Value, allow: String, deny: String },
}

/// One request a message's buttons are for, as it was when the message went.
#[derive(Clone, Debug, PartialEq)]
pub struct Approval {
    pub session: String,
    pub target: Target,
    /// What would run, as the agent's protocol or hook names it.
    pub command: String,
}

impl Approval {
    pub fn wait(&self) -> Option<u64> {
        match self.target {
            Target::Term { wait, .. } => Some(wait),
            Target::Chat { .. } => None,
        }
    }
}

/// What became of a press.
#[derive(Debug, PartialEq)]
pub enum Answered {
    Done,
    /// Somebody answered first: who.
    Already(String),
    /// The request is no longer what the button was for.
    Gone,
    /// Approvals from Telegram ended while the press was on its way.
    Off,
}

struct Offer {
    approval: Approval,
    at: Instant,
    chat: String,
    message: Option<i64>,
    text: String,
}

struct Binding {
    code: String,
    until: Instant,
    wrong: u32,
}

#[derive(Default)]
struct State {
    /// By nonce.
    offers: HashMap<String, Offer>,
    binding: Option<Binding>,
    /// When the last binding ended: messages are still read for a minute, so a code sent late is answered.
    binding_ended: Option<Instant>,
    /// The next update wanted, which also tells Telegram the earlier ones were read.
    offset: Option<i64>,
    /// When this minute's count began, and how many approvals it has.
    minute: Option<(Instant, u32)>,
    strangers: u64,
    strangers_noted: Option<Instant>,
    /// Presses of buttons that are no longer valid, noted the same way.
    stale: u64,
    stale_noted: Option<Instant>,
    /// What stopped or holds up the polling, for the settings.
    error: Option<String>,
    conflict_noted: bool,
}

pub struct Approvals {
    secrets: Arc<dyn SecretStore>,
    config: Arc<Mutex<Config>>,
    host: Arc<dyn Host>,
    /// `notify.json`, which a binding and the kill switch write.
    path: PathBuf,
    /// One writer of that file at a time (`Notifier::update` takes it too).
    saving: Mutex<()>,
    state: Mutex<State>,
    /// The polling thread should run, and it does.
    want: AtomicBool,
    alive: AtomicBool,
    thread: Mutex<Option<std::thread::Thread>>,
    /// `getUpdates` calls made.
    polls: AtomicU64,
    /// One more each time approvals end: a press taken before that answers nothing after it.
    generation: AtomicU64,
    #[cfg(test)]
    test_base: Mutex<Option<String>>,
}

impl State {
    fn end_binding(&mut self, now: Instant) -> bool {
        let had = self.binding.take().is_some();
        if had {
            self.binding_ended = Some(now);
        }
        had
    }
}

impl Approvals {
    pub fn new(data_dir: &Path, secrets: Arc<dyn SecretStore>, config: Arc<Mutex<Config>>, host: Arc<dyn Host>) -> Arc<Approvals> {
        Arc::new(Approvals {
            secrets,
            config,
            host,
            path: data_dir.join("notify.json"),
            saving: Mutex::new(()),
            state: Mutex::new(State::default()),
            want: AtomicBool::new(false),
            alive: AtomicBool::new(false),
            thread: Mutex::new(None),
            polls: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            #[cfg(test)]
            test_base: Mutex::new(None),
        })
    }

    pub fn saving(&self) -> MutexGuard<'_, ()> {
        self.saving.lock()
    }

    /// The Bot API's address (`telegram_base`), or a unit test's stand-in.
    pub(super) fn base(&self) -> String {
        #[cfg(test)]
        if let Some(base) = self.test_base.lock().clone() {
            return base;
        }
        telegram_base()
    }

    #[cfg(test)]
    pub fn set_test_base(&self, base: &str) {
        *self.test_base.lock() = Some(base.to_string());
    }

    #[cfg(test)]
    pub fn polls(&self) -> u64 {
        self.polls.load(Ordering::SeqCst)
    }

    /// Binds `user` in `chat` as `/bind` would, without Telegram.
    #[cfg(test)]
    pub fn bind_for_test(self: &Arc<Self>, user: i64, chat: i64) {
        {
            let mut c = self.config.lock();
            c.telegram.user_id = user.to_string();
            c.telegram.chat_id = chat.to_string();
            c.telegram.mode = TelegramMode::ApproveLow;
        }
        self.refresh();
    }

    /// Makes every open button `by` older.
    #[cfg(test)]
    pub fn age_offers(&self, by: Duration) {
        for offer in self.state.lock().offers.values_mut() {
            offer.at = offer.at.checked_sub(by).expect("a clock that old");
        }
    }

    /// Whether the polling thread is running.
    pub fn polling(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    fn approving(&self) -> bool {
        self.config.lock().telegram.approving()
    }

    fn binding_open(&self, now: Instant) -> bool {
        self.state.lock().binding.as_ref().is_some_and(|b| now < b.until)
    }

    /// Chat messages are Kelpie's business only while a binding waits, and for a minute after it ended. Otherwise only presses are asked for.
    fn wants_messages(&self, now: Instant) -> bool {
        let s = self.state.lock();
        s.binding.as_ref().is_some_and(|b| now < b.until) || s.binding_ended.is_some_and(|t| now.duration_since(t) < MINUTE)
    }

    /// Ends a binding under way, because the person chose otherwise in the settings: its code binds nothing from now on.
    pub fn cancel_binding(&self) {
        if self.state.lock().end_binding(Instant::now()) {
            self.host.telegram_changed("bind_cancelled");
        }
    }

    /// Starts the polling while approvals are on or a binding waits, and stops it otherwise. Nothing runs in any other case.
    pub fn refresh(self: &Arc<Self>) {
        let on = self.approving() || self.binding_open(Instant::now());
        if !on {
            self.want.store(false, Ordering::SeqCst);
            self.wake();
            return;
        }
        self.want.store(true, Ordering::SeqCst);
        if !self.alive.swap(true, Ordering::SeqCst) {
            let me = self.clone();
            match std::thread::Builder::new().name("kelpie-telegram".into()).spawn(move || run(me)) {
                Ok(handle) => *self.thread.lock() = Some(handle.thread().clone()),
                Err(e) => {
                    self.alive.store(false, Ordering::SeqCst);
                    tracing::warn!("telegram polling: {e}");
                }
            }
        }
    }

    fn wake(&self) {
        if let Some(t) = self.thread.lock().as_ref() {
            t.unpark();
        }
    }

    /// A new code for `/bind <code>`, for the next five minutes. The caller checked the app and the person.
    pub fn begin_binding(self: &Arc<Self>) -> (String, u64) {
        let code = random_topic()[..12].to_ascii_uppercase();
        {
            let mut s = self.state.lock();
            s.binding = Some(Binding { code: code.clone(), until: Instant::now() + BIND_FOR, wrong: 0 });
            s.error = None;
        }
        self.refresh();
        (code, BIND_FOR.as_secs())
    }

    /// What the settings show.
    pub fn describe(&self) -> Value {
        let approving = self.approving();
        let now = Instant::now();
        let s = self.state.lock();
        json!({
            "approving": approving,
            "polling": self.polling(),
            "binding_expires_in": s.binding.as_ref().filter(|b| now < b.until).map(|b| b.until.duration_since(now).as_secs()),
            "error": s.error,
            "open": s.offers.len(),
        })
    }

    /// The kill switch, and a token Telegram rejected: approvals off and saved so, the binding forgotten, every button dead.
    pub fn turn_off(self: &Arc<Self>, why: &str) {
        let was_on = {
            let _saving = self.saving.lock();
            let mut c = self.config.lock().clone();
            let was_on = c.telegram.mode == TelegramMode::ApproveLow || !c.telegram.user_id.is_empty();
            if was_on {
                c.telegram.mode = TelegramMode::AlertLink;
                c.telegram.user_id.clear();
                c.telegram.user_name.clear();
                // Held in memory whatever the disk says: a kill switch that a failed write undoes is none.
                if let Err(e) = serde_json::to_vec_pretty(&c).map_err(anyhow::Error::from).and_then(|bytes| write_atomic(&self.path, &bytes)) {
                    tracing::warn!("telegram approvals off, not saved: {e:#}");
                }
                *self.config.lock() = c;
            }
            was_on
        };
        let binding = self.state.lock().end_binding(Instant::now());
        self.ended(why, was_on || binding);
        if was_on || binding {
            self.host.telegram_changed("off");
        }
        self.refresh();
    }

    /// Approvals ended (`turn_off`, or a change in the settings): every nonce is burnt, and the messages lose their buttons.
    pub(super) fn ended(self: &Arc<Self>, why: &str, note: bool) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        let offers: Vec<Offer> = self.state.lock().offers.drain().map(|(_, o)| o).collect();
        if note || !offers.is_empty() {
            self.host.telegram_audit("telegram_off", why);
        }
        if offers.iter().any(|o| o.message.is_some()) {
            let me = self.clone();
            let _ = std::thread::Builder::new().name("kelpie-telegram-strip".into()).spawn(move || {
                for offer in &offers {
                    me.strip(offer, "Approvals from Telegram are off: answer on the computer or the phone.");
                }
            });
        }
    }

    fn minute_full(&self, now: Instant) -> bool {
        matches!(self.state.lock().minute, Some((start, n)) if now.duration_since(start) < MINUTE && n >= APPROVALS_PER_MINUTE)
    }

    /// The message about a wait, with buttons when it may have them: approvals on, fewer than six this minute, and a request the daemon labels
    /// low that a button can answer once. None means the message goes as an alert, the way it always did.
    pub(super) fn send(&self, c: &Config, e: &Entry, p: &Payload) -> Option<Result<(), Failure>> {
        if !c.telegram.approving() || Kind::parse(&e.kind) != Some(Kind::Waiting) {
            return None;
        }
        let session = e.session.as_deref()?;
        let token = self.secrets.read(K_TELEGRAM_TOKEN).ok().flatten()?;
        let now = Instant::now();
        if self.minute_full(now) || self.state.lock().offers.len() >= MAX_OFFERS {
            return None;
        }
        let approval = self.host.telegram_approval(session, e.wait)?;
        let shown = kelpie_proto::redact(&approval.command);
        if shown.chars().count() > MAX_ASKED || shown.chars().any(crate::mobile::risk::hidden_mark) {
            return None;
        }
        let nonce = random_topic();
        let text = telegram_text(p, Some(&format!("Kelpie labels this low risk. Approve it this once?\n{shown}")));
        self.state.lock().offers.insert(nonce.clone(), Offer { approval, at: now, chat: c.telegram.chat_id.clone(), message: None, text: text.clone() });
        let keyboard = json!({"inline_keyboard": [[{"text": "Approve once", "callback_data": format!("a:{nonce}")}, {"text": "Deny", "callback_data": format!("d:{nonce}")}]]});
        let body = json!({"chat_id": c.telegram.chat_id, "text": text, "disable_web_page_preview": true, "reply_markup": keyboard});
        let out = telegram_call(&self.base(), &token, "sendMessage", &body, SEND_TIMEOUT);
        let mut s = self.state.lock();
        match &out {
            Ok(sent) => match (s.offers.get_mut(&nonce), sent["message_id"].as_i64()) {
                (Some(offer), Some(id)) => offer.message = Some(id),
                _ => {
                    s.offers.remove(&nonce);
                }
            },
            Err(_) => {
                s.offers.remove(&nonce);
            }
        }
        // Refused as it was (not for the rate or the server): the alert goes the way it always did, without the buttons.
        if out.as_ref().is_err_and(|f| matches!(f.status, Some(400..=499)) && f.status != Some(429)) {
            return None;
        }
        Some(out.map(|_| ()))
    }

    fn poll(&self) -> Result<Vec<Value>, Failure> {
        let token = match self.secrets.read(K_TELEGRAM_TOKEN) {
            Ok(Some(t)) => t,
            Ok(None) => return Err(Failure { status: Some(401), ..Failure::from("no bot token") }),
            Err(_) => return Err(Failure::again("the credential store did not answer")),
        };
        let allowed = if self.wants_messages(Instant::now()) { json!(["message", "callback_query"]) } else { json!(["callback_query"]) };
        let mut body = json!({"timeout": POLL_S, "limit": UPDATES_PER_POLL, "allowed_updates": allowed});
        if let Some(offset) = self.state.lock().offset {
            body["offset"] = json!(offset);
        }
        self.polls.fetch_add(1, Ordering::SeqCst);
        let result = telegram_call(&self.base(), &token, "getUpdates", &body, Duration::from_secs(POLL_S + 10))?;
        let updates = result.as_array().cloned().unwrap_or_default();
        if let Some(last) = updates.iter().filter_map(|u| u["update_id"].as_i64()).max() {
            self.state.lock().offset = Some(last + 1);
        }
        Ok(updates)
    }

    fn handle(self: &Arc<Self>, update: &Value) {
        if let Some(press) = update.get("callback_query") {
            self.press(press);
        } else if let Some(message) = update.get("message") {
            self.message(message);
        }
    }

    /// `/bind <code>` while a binding waits, in the chat set for Telegram (any, while none is), binds the account that sent it. Any other
    /// message is not Kelpie's business. Wrong codes count only from that chat, so nobody else can use them up.
    fn message(self: &Arc<Self>, m: &Value) {
        let Some(text) = m["text"].as_str() else { return };
        let mut words = text.split_whitespace();
        let command = words.next().unwrap_or_default();
        if command != "/bind" && !command.starts_with("/bind@") {
            return;
        }
        let given = words.next().unwrap_or_default().to_ascii_uppercase();
        let (Some(from), Some(chat)) = (m.pointer("/from/id").and_then(Value::as_i64), m.pointer("/chat/id").and_then(Value::as_i64)) else { return };
        let set = self.config.lock().telegram.chat_id.clone();
        let named = m.pointer("/chat/username").and_then(Value::as_str).is_some_and(|u| set.strip_prefix('@').is_some_and(|s| s.eq_ignore_ascii_case(u)));
        if !set.is_empty() && set != chat.to_string() && !named {
            return self.reply(chat, "Kelpie takes the code only in the chat set in Kelpie → Settings → Notifications: send it there.");
        }
        // An anonymous admin is the group speaking: any of its admins, so nobody who could be the one account that approves.
        if m.get("sender_chat").is_some_and(|c| !c.is_null()) || from == ANONYMOUS_ADMIN {
            return self.reply(chat, "Send the code as yourself, not anonymously for the group: the account that sends it is the one that approves.");
        }
        let now = Instant::now();
        {
            let mut s = self.state.lock();
            let Some(b) = s.binding.as_mut().filter(|b| now < b.until) else {
                drop(s);
                return self.reply(chat, "No code is waiting: it was used already, or it ran out. Check Kelpie → Settings → Mobile, and choose approvals again for a new code.");
            };
            if !same(&b.code, &given) {
                b.wrong += 1;
                let over = b.wrong >= MAX_WRONG;
                if over {
                    s.end_binding(now);
                }
                drop(s);
                if over {
                    self.host.telegram_audit("telegram_bind_failed", &format!("{MAX_WRONG} wrong codes"));
                    self.host.telegram_changed("bind_failed");
                    self.reply(chat, "Too many wrong codes, so nothing was bound. Choose approvals again in Kelpie → Settings → Mobile for a new code.");
                    self.refresh();
                } else {
                    self.reply(chat, "That is not the code Kelpie shows now, or it was used already: check Kelpie → Settings → Mobile.");
                }
                return;
            }
            s.end_binding(now);
        }
        let name = binder_name(m);
        let stored = {
            let _saving = self.saving.lock();
            let mut c = self.config.lock().clone();
            // Telegram switched off, or set to another chat, since the message was read: what the person chose last stands.
            if !c.telegram.enabled || c.telegram.chat_id != set {
                false
            } else {
                c.telegram.user_id = from.to_string();
                c.telegram.user_name = name.clone();
                c.telegram.chat_id = chat.to_string();
                c.telegram.mode = TelegramMode::ApproveLow;
                match serde_json::to_vec_pretty(&c).map_err(anyhow::Error::from).and_then(|bytes| write_atomic(&self.path, &bytes)) {
                    Ok(()) => {
                        *self.config.lock() = c;
                        true
                    }
                    Err(e) => {
                        tracing::warn!("telegram binding not saved: {e:#}");
                        false
                    }
                }
            }
        };
        if stored {
            let who = if name.is_empty() { from.to_string() } else { format!("{name} ({from})") };
            self.host.telegram_audit("telegram_bound", &format!("account {who}, chat {chat}"));
            self.reply(chat, "Your Telegram account can now approve low-risk requests on this computer from this chat, one at a time. Turn it off in Kelpie → Settings → Mobile.");
            self.host.telegram_changed("bound");
        } else {
            self.state.lock().error = Some("The binding could not be saved: turn approvals on again.".into());
            self.host.telegram_changed("error");
        }
        self.refresh();
    }

    /// A button press: counted as a stranger's unless it comes from the bound account in the bound chat, then answered at most once.
    fn press(self: &Arc<Self>, cq: &Value) {
        let id = cq["id"].as_str().unwrap_or_default().to_string();
        let from = cq.pointer("/from/id").and_then(Value::as_i64).map(|v| v.to_string());
        let chat = cq.pointer("/message/chat/id").and_then(Value::as_i64).map(|v| v.to_string());
        let message = cq.pointer("/message/message_id").and_then(Value::as_i64);
        // Taken before anything is read: approvals that end from here on are seen right before the answer goes (`go`).
        let generation = self.generation.load(Ordering::SeqCst);
        let t = self.config.lock().telegram.clone();
        if !t.approving() || from.as_deref() != Some(t.user_id.as_str()) || chat.as_deref() != Some(t.chat_id.as_str()) {
            self.state.lock().strangers += 1;
            return;
        }
        let (allow, nonce) = match cq["data"].as_str().and_then(|d| d.split_once(':')) {
            Some(("a", n)) => (true, n),
            Some(("d", n)) => (false, n),
            _ => return self.tell(&id, "That button is not Kelpie's."),
        };
        // Single use: taken out before anything is answered, and only by the message it was sent with.
        let offer = {
            let mut s = self.state.lock();
            match s.offers.get(nonce) {
                Some(o) if o.message.is_some() && o.message == message => s.offers.remove(nonce),
                _ => None,
            }
        };
        let Some(offer) = offer else {
            // Counted, and noted once a minute: pressing old buttons does not fill the log.
            self.state.lock().stale += 1;
            return self.tell(&id, "This button is no longer valid: answer on the computer or the phone.");
        };
        let summary = |what: &str| format!("{}: {what}, {}", offer.approval.session, crate::mobile::audit::text_summary(&offer.approval.command));
        let now = Instant::now();
        if now.duration_since(offer.at) >= OFFER_FOR {
            self.host.telegram_audit("telegram_refused", &summary("expired"));
            self.tell(&id, "This button expired: answer on the computer or the phone.");
            return self.strip(&offer, "Expired: answer on the computer or the phone.");
        }
        if allow && self.minute_full(now) {
            self.host.telegram_audit("telegram_refused", &summary("too many approvals this minute"));
            self.tell(&id, "Six approvals from Telegram this minute already: answer this one on the computer or the phone.");
            return self.strip(&offer, "Not approved: too many approvals from Telegram this minute.");
        }
        let go = || self.generation.load(Ordering::SeqCst) == generation && self.approving();
        match self.host.telegram_answer(&offer.approval, allow, &go) {
            Answered::Done => {
                if allow {
                    let mut s = self.state.lock();
                    s.minute = match s.minute {
                        Some((start, n)) if now.duration_since(start) < MINUTE => Some((start, n + 1)),
                        _ => Some((now, 1)),
                    };
                }
                let (action, what, said) = if allow { ("telegram_approve", "approved once", "Approved once from Telegram.") } else { ("telegram_deny", "denied", "Denied from Telegram.") };
                self.host.telegram_audit(action, &summary(what));
                self.tell(&id, said);
                self.strip(&offer, said);
            }
            Answered::Already(by) => {
                // Telegram is not told a phone's name.
                let who = match by.as_str() {
                    "desktop" => "on the computer",
                    "telegram" => "from Telegram",
                    _ => "on a paired phone",
                };
                self.host.telegram_audit("telegram_refused", &summary(&format!("already answered {who}")));
                self.tell(&id, &format!("Already answered {who}."));
                self.strip(&offer, &format!("Answered {who}."));
            }
            Answered::Gone => {
                self.host.telegram_audit("telegram_refused", &summary("no longer waiting"));
                self.tell(&id, "That request is no longer waiting.");
                self.strip(&offer, "No longer waiting.");
            }
            Answered::Off => {
                self.host.telegram_audit("telegram_refused", &summary("approvals from Telegram were turned off"));
                self.tell(&id, "Approvals from Telegram are off: answer on the computer or the phone.");
                self.strip(&offer, "Approvals from Telegram are off: answer on the computer or the phone.");
            }
        }
    }

    /// What runs before each poll: a binding that ran out, strangers' presses noted once a minute, and buttons whose request was answered
    /// elsewhere or that expired, which lose them.
    fn housekeeping(self: &Arc<Self>, now: Instant) {
        let binding_over = {
            let mut s = self.state.lock();
            s.binding.as_ref().is_some_and(|b| now >= b.until) && s.end_binding(now)
        };
        if binding_over {
            self.host.telegram_changed("bind_expired");
            self.refresh();
        }
        let (strangers, stale) = {
            let mut s = self.state.lock();
            let s = &mut *s;
            (once_a_minute(&mut s.strangers, &mut s.strangers_noted, now), once_a_minute(&mut s.stale, &mut s.stale_noted, now))
        };
        if let Some(n) = strangers {
            self.host.telegram_audit("telegram_ignored", &format!("{n} button presses from another Telegram account or chat"));
        }
        if let Some(n) = stale {
            self.host.telegram_audit("telegram_refused", &format!("{n} presses of buttons that are no longer valid"));
        }
        let open: Vec<(String, Approval, Instant)> = self.state.lock().offers.iter().filter(|(_, o)| o.message.is_some()).map(|(n, o)| (n.clone(), o.approval.clone(), o.at)).collect();
        for (nonce, approval, at) in open {
            let note = if now.duration_since(at) >= OFFER_FOR {
                "Expired: answer on the computer or the phone."
            } else if !self.host.telegram_still(&approval) {
                "Answered elsewhere, or no longer waiting."
            } else {
                continue;
            };
            let gone = self.state.lock().offers.remove(&nonce);
            if let Some(offer) = gone {
                self.strip(&offer, note);
            }
        }
    }

    fn call(&self, method: &str, body: &Value) {
        if let Ok(Some(token)) = self.secrets.read(K_TELEGRAM_TOKEN) {
            if let Err(f) = telegram_call(&self.base(), &token, method, body, SEND_TIMEOUT) {
                tracing::warn!("telegram approvals: {}", f.reason);
            }
        }
    }

    /// The message without its buttons, saying what became of them.
    fn strip(&self, offer: &Offer, note: &str) {
        let Some(message) = offer.message else { return };
        self.call("editMessageText", &json!({"chat_id": offer.chat, "message_id": message, "text": format!("{}\n\n{note}", offer.text), "disable_web_page_preview": true}));
    }

    /// The little note Telegram shows the person who pressed.
    fn tell(&self, callback: &str, text: &str) {
        if !callback.is_empty() {
            self.call("answerCallbackQuery", &json!({"callback_query_id": callback, "text": text}));
        }
    }

    fn reply(&self, chat: i64, text: &str) {
        self.call("sendMessage", &json!({"chat_id": chat, "text": text}));
    }

    /// Sleeps for `d`, or until the polling should stop.
    fn pause(&self, d: Duration) {
        let until = Instant::now() + d;
        while self.want.load(Ordering::SeqCst) {
            let now = Instant::now();
            if now >= until {
                break;
            }
            std::thread::park_timeout(until - now);
        }
    }

    fn failed(self: &Arc<Self>, f: &Failure, failures: u32) {
        match f.status {
            // Telegram answers 404 for a token that names no bot.
            Some(401 | 404) => {
                self.turn_off("the bot token was rejected");
                self.state.lock().error = Some(if f.reason == "no bot token" { "Telegram has no bot token, so approvals from Telegram stopped. Set it in Settings → Notifications, then turn approvals on again.".into() } else { "Telegram rejected the bot token, so approvals from Telegram stopped. Check the token in Settings → Notifications, then turn approvals on again.".into() });
                self.host.telegram_changed("error");
            }
            Some(409) => {
                let first = {
                    let mut s = self.state.lock();
                    s.error = Some("Telegram gives this bot's messages to something else: the bot has a webhook, or another program reads its messages. Remove the webhook (deleteWebhook) or stop that program.".into());
                    !std::mem::replace(&mut s.conflict_noted, true)
                };
                if first {
                    tracing::warn!("telegram polling: refused, another poller or a webhook has this bot");
                    self.host.telegram_changed("error");
                }
            }
            _ => {
                if failures >= 3 {
                    self.state.lock().error = Some(format!("Telegram could not be reached ({}). Buttons sent meanwhile expire; alerts still go out when it can be.", f.reason));
                }
            }
        }
    }
}

/// The polling thread: housekeeping, then one long poll, until it should stop.
fn run(me: Arc<Approvals>) {
    let mut failures = 0u32;
    loop {
        if !me.want.load(Ordering::SeqCst) {
            me.alive.store(false, Ordering::SeqCst);
            // Asked for again while it was ending: it carries on.
            if me.want.load(Ordering::SeqCst) && !me.alive.swap(true, Ordering::SeqCst) {
                continue;
            }
            return;
        }
        me.housekeeping(Instant::now());
        if !me.want.load(Ordering::SeqCst) {
            continue;
        }
        let began = Instant::now();
        match me.poll() {
            Ok(updates) => {
                if failures > 0 || me.state.lock().conflict_noted {
                    let mut s = me.state.lock();
                    s.error = None;
                    s.conflict_noted = false;
                }
                failures = 0;
                for update in &updates {
                    if !me.want.load(Ordering::SeqCst) {
                        break;
                    }
                    me.handle(update);
                }
                // Nothing, at once: a server that does not hold the poll open is not asked again at the speed of the network.
                if updates.is_empty() {
                    me.pause(MIN_POLL.saturating_sub(began.elapsed()));
                }
            }
            Err(f) => {
                failures = failures.saturating_add(1);
                me.failed(&f, failures);
                if me.want.load(Ordering::SeqCst) {
                    me.pause(backoff(failures, f.retry_after));
                }
            }
        }
    }
}

/// Full jitter: a random time up to 1 s, 2 s, 4 s… and at most a minute; never less than what Telegram asked for.
fn backoff(failures: u32, asked: Option<Duration>) -> Duration {
    let cap = 1u64.checked_shl(failures.saturating_sub(1)).unwrap_or(BACKOFF_MAX_S).min(BACKOFF_MAX_S) * 1000;
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let roll = u64::from_le_bytes(bytes[..8].try_into().expect("8 of 16 bytes")) % (cap + 1);
    Duration::from_millis(roll).max(asked.unwrap_or_default().min(Duration::from_secs(BACKOFF_MAX_S)))
}

/// A count to note in the log: all of it, at most once a minute.
fn once_a_minute(count: &mut u64, noted: &mut Option<Instant>, now: Instant) -> Option<u64> {
    let due = *count > 0 && noted.is_none_or(|t| now.duration_since(t) >= MINUTE);
    due.then(|| {
        *noted = Some(now);
        std::mem::take(count)
    })
}

/// Who bound, as Telegram names them: `@username`, else the first name, without control characters or hidden marks, at most `NAME_MAX` long.
fn binder_name(m: &Value) -> String {
    let name = match (m.pointer("/from/username").and_then(Value::as_str), m.pointer("/from/first_name").and_then(Value::as_str)) {
        (Some(u), _) if !u.is_empty() => format!("@{u}"),
        (_, Some(first)) => first.to_string(),
        _ => String::new(),
    };
    name.chars().filter(|&c| !c.is_control() && !crate::mobile::risk::hidden_mark(c)).take(NAME_MAX).collect::<String>().trim().to_string()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests;
