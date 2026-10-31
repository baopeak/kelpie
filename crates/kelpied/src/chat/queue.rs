//! The messages waiting to be sent to a chat's agent: one bounded FIFO per chat, fed by the desktop
//! and by phones alike, in the order the daemon received them. This is the data and its rules; the
//! gate that decides when the head may be written lives with the chat session (`chat.rs`).
//!
//! Every message is remembered by `(device, cid)` for ten minutes, so a retry after a lost reply
//! learns what became of the first try instead of making a second message.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::session::By;

/// Messages waiting in one chat, and from one phone.
pub const MAX_ENTRIES: usize = 20;
pub const MAX_PER_DEVICE: usize = 5;
/// What a phone may send in one message, and the text a desktop entry shows.
pub const MAX_PHONE_TEXT: usize = 16 * 1024;
pub const MAX_DESKTOP_TEXT: usize = 1 << 20;
/// The builder of a desktop entry has this long to answer an offer before the daemon takes over (plain text). Anything else
/// waits this long again before it is offered again: a window that never answers is asked every 30 s (`REOFFER_EVERY`).
pub const OFFER_TIMEOUT: Duration = Duration::from_secs(15);
/// How often a window that does not answer is offered a message only it can build: `OFFER_TIMEOUT` to answer, then as long
/// again before the next offer.
pub const REOFFER_EVERY: Duration = Duration::from_secs(30);
/// A turn that was written and drew no output, with the agent idle, is given up after this long.
pub const FLIGHT_WATCHDOG: Duration = Duration::from_secs(30);
/// How long a message is remembered under its `cid`, and how many are.
const SEEN_FOR: Duration = Duration::from_secs(600);
const SEEN_MAX: usize = 2_000;
/// A second stop this soon after the first is the same stop.
const STOP_GAP: Duration = Duration::from_secs(1);
/// An interrupt right after "send now" is part of it and does not pause the queue.
const SPARE_FOR: Duration = Duration::from_secs(5);
/// An agent whose last classified line said it works, with no turn of ours or anyone's in flight and nothing printed for this
/// long, is taken to be between turns as far as the queue goes (a late chunk after a turn's result leaves it "working").
pub const STALE_WORKING: Duration = Duration::from_secs(60);
/// After a stop, an agent that prints nothing at all for this long is not running anything any more, as far as the queue goes.
pub const STOP_GRACE: Duration = Duration::from_secs(30);
/// A window that said "not now" is asked again after this long.
pub const RETRY_AFTER: Duration = Duration::from_secs(1);
/// Text of an entry in the events (the chips): the rest stays with its sender.
const EVENT_TEXT: usize = 2_000;

/// What became of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued,
    Sent,
    Cancelled,
    Dropped,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Sent => "sent",
            State::Cancelled => "cancelled",
            State::Dropped => "dropped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// The daemon builds the line when it writes it.
    Phone,
    /// The window that queued it builds the line when it is the entry's turn (`client` is its
    /// connection); `rich` entries carry what only that window can build (pictures).
    Desktop { client: u64, rich: bool },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub qid: String,
    pub cid: String,
    pub by: By,
    pub text: String,
    /// File names of the pictures a desktop entry carries (for the log only).
    pub images: Vec<String>,
    pub origin: Origin,
    pub at: u64,
    /// When the head was offered to its builder.
    pub offered: Option<Instant>,
    /// The builder said it does not have the message.
    pub declined: bool,
    /// The builder said "not now" (replaying, loading): asked again at this time.
    pub retry_at: Option<Instant>,
    /// When the builder was first asked: the daemon's own fallback for plain text counts from here.
    pub first_offer: Option<Instant>,
    /// A picture or the like that only the window that queued it can build, and it is not there to do it: the entry waits at
    /// the head, visible to everyone, until the window comes back, or someone cancels it.
    pub waiting_window: bool,
    /// The connection of the window that queued it is gone.
    pub orphaned: bool,
}

/// A message that will not be sent, and why.
#[derive(Debug, Clone)]
pub struct Dropped {
    pub qid: String,
    pub cid: String,
    pub by: By,
    pub text: String,
    /// File names of its pictures.
    pub images: Vec<String>,
    pub reason: &'static str,
    /// Who took it out of the queue, when that was not the one who queued it (`reason` is `cancelled`).
    pub cancelled_by: Option<By>,
}

/// The turn the agent is running because a message was written (or because someone wrote a turn
/// line to its stdin: then there is no `qid`).
#[derive(Debug, Clone)]
pub struct InFlight {
    pub qid: Option<String>,
    /// The key of the request that started it: an error answer to it ends the turn.
    pub req: Option<String>,
    pub since: Instant,
    /// The agent printed something since.
    pub output: bool,
}

/// What a new message is.
pub struct NewEntry {
    pub cid: String,
    pub by: By,
    pub text: String,
    pub images: Vec<String>,
    pub origin: Origin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Exited,
    /// The chat is being handed to a terminal.
    Switching,
    Empty,
    TooLong,
    QueueFull,
    /// A command that only the desktop's client can run.
    NeedsDesktop,
    /// The device may not send (any more).
    ReadOnly,
}

impl Refused {
    pub fn code(self) -> &'static str {
        match self {
            Refused::Exited => "exited",
            Refused::Switching => "switching",
            Refused::Empty => "empty",
            Refused::TooLong => "too_long",
            Refused::QueueFull => "queue_full",
            Refused::NeedsDesktop => "command_needs_desktop",
            Refused::ReadOnly => "read_only",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Refused::Exited => "This chat is not running. Open this chat on the desktop to resume it.",
            Refused::Switching => "This chat is being moved to a terminal.",
            Refused::Empty => "There is nothing to send.",
            Refused::TooLong => "That is too long to send in one go.",
            Refused::QueueFull => "Too many messages are waiting for this chat.",
            Refused::NeedsDesktop => "This command runs only on the computer.",
            Refused::ReadOnly => "This device may read but not type.",
        }
    }
}

/// An entry that is not in the queue to be changed: already offered or written, cancelled, or gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooLate;

struct Seen {
    qid: String,
    state: State,
    at: Instant,
}

pub struct Queue {
    pub items: VecDeque<Entry>,
    /// Stopping means wait: nothing goes out until the user sends again or resumes.
    pub paused: bool,
    /// The chat is being handed to a terminal: nothing goes out and nothing new comes in.
    pub held: bool,
    pub flight: Option<InFlight>,
    seen: HashMap<(String, String), Seen>,
    order: VecDeque<(String, String)>,
    next: u64,
    /// While set and in the future, an interrupt that is observed does not pause the queue.
    pub spare_interrupt: Option<Instant>,
    last_stop: Option<Instant>,
    /// A stop was asked for or seen (an interrupt line) while a turn ran, and the turn has not ended yet: nothing is written
    /// into a turn that is still stopping.
    pub stopped_at: Option<Instant>,
    /// The stopped turn printed nothing at all for `STOP_GRACE`: the queue takes the agent to be idle until its next classified
    /// line. This is the queue's own view; the session's status is left as the agent's lines made it.
    pub released: bool,
}

impl Default for Queue {
    fn default() -> Self {
        Queue::new()
    }
}

impl Queue {
    pub fn new() -> Queue {
        Queue { items: VecDeque::new(), paused: false, held: false, flight: None, seen: HashMap::new(), order: VecDeque::new(), next: 0, spare_interrupt: None, last_stop: None, stopped_at: None, released: false }
    }

    fn forget_old(&mut self, now: Instant) {
        while let Some(key) = self.order.front() {
            // A message still waiting is remembered for as long as it waits.
            let expired = self.seen.get(key).is_none_or(|s| s.state != State::Queued && now.duration_since(s.at) > SEEN_FOR);
            if !expired && self.order.len() <= SEEN_MAX {
                break;
            }
            let key = self.order.pop_front().expect("front exists");
            self.seen.remove(&key);
        }
    }

    /// What became of the message `cid` of `device`, if it is still remembered.
    pub fn lookup(&mut self, device: &str, cid: &str, now: Instant) -> Option<(String, State)> {
        self.forget_old(now);
        self.seen.get(&(device.to_string(), cid.to_string())).map(|s| (s.qid.clone(), s.state))
    }

    /// Adds a message at the end; returns its id and how many are ahead of it.
    pub fn push(&mut self, new: NewEntry, now: Instant, at: u64) -> Result<(String, usize), Refused> {
        if self.held {
            return Err(Refused::Switching);
        }
        let phone = new.origin == Origin::Phone;
        if new.text.trim().is_empty() && new.images.is_empty() {
            return Err(Refused::Empty);
        }
        if new.text.len() > if phone { MAX_PHONE_TEXT } else { MAX_DESKTOP_TEXT } {
            return Err(Refused::TooLong);
        }
        if self.items.len() >= MAX_ENTRIES || (phone && self.items.iter().filter(|e| e.by.id == new.by.id).count() >= MAX_PER_DEVICE) {
            return Err(Refused::QueueFull);
        }
        self.forget_old(now);
        self.next += 1;
        let qid = format!("q{}", self.next);
        let ahead = self.items.len();
        let key = (new.by.id.clone(), new.cid.clone());
        if self.seen.insert(key.clone(), Seen { qid: qid.clone(), state: State::Queued, at: now }).is_none() {
            self.order.push_back(key);
        }
        self.items.push_back(Entry { qid: qid.clone(), cid: new.cid, by: new.by, text: new.text, images: new.images, origin: new.origin, at, offered: None, declined: false, retry_at: None, first_offer: None, waiting_window: false, orphaned: false });
        Ok((qid, ahead))
    }

    /// Records what became of an entry (it may already have left the queue).
    pub fn mark(&mut self, entry: &Entry, state: State, now: Instant) {
        if let Some(s) = self.seen.get_mut(&(entry.by.id.clone(), entry.cid.clone())) {
            s.state = state;
            s.at = now;
        }
    }

    fn position(&self, qid: &str) -> Option<usize> {
        self.items.iter().position(|e| e.qid == qid)
    }

    /// Takes a queued entry out of the line (cancelled by whoever asked): `TooLate` once it was
    /// offered to its builder or written, or when it is not there.
    pub fn cancel(&mut self, qid: &str, now: Instant) -> Result<Entry, TooLate> {
        let i = self.position(qid).filter(|i| self.items[*i].offered.is_none()).ok_or(TooLate)?;
        let entry = self.items.remove(i).ok_or(TooLate)?;
        self.mark(&entry, State::Cancelled, now);
        Ok(entry)
    }

    /// Moves a queued entry to the head: it is the next to go whatever came before it.
    pub fn front(&mut self, qid: &str) -> Result<(), TooLate> {
        let i = self.position(qid).filter(|i| self.items[*i].offered.is_none()).ok_or(TooLate)?;
        // Behind a head that was offered already: that one is on its way.
        let at = usize::from(self.items.front().is_some_and(|e| e.offered.is_some()));
        if let Some(entry) = self.items.remove(i) {
            self.items.insert(at.min(self.items.len()), entry);
        }
        Ok(())
    }

    /// Empties the queue (the process ended, or the chat moved to a terminal).
    pub fn drop_all(&mut self, reason: &'static str, now: Instant) -> Vec<Dropped> {
        self.drop_where(|_| true, reason, now)
    }

    pub fn drop_where(&mut self, mut which: impl FnMut(&Entry) -> bool, reason: &'static str, now: Instant) -> Vec<Dropped> {
        let mut dropped = Vec::new();
        let mut kept = VecDeque::with_capacity(self.items.len());
        for entry in std::mem::take(&mut self.items) {
            if which(&entry) {
                self.mark(&entry, State::Dropped, now);
                dropped.push(Dropped { qid: entry.qid, cid: entry.cid, by: entry.by, text: entry.text, images: entry.images, reason, cancelled_by: None });
            } else {
                kept.push_back(entry);
            }
        }
        self.items = kept;
        dropped
    }

    /// A stop is asked for: false when one was just made (it is the same stop).
    pub fn stop_now(&mut self, now: Instant) -> bool {
        if self.last_stop.is_some_and(|t| now.duration_since(t) < STOP_GAP) {
            return false;
        }
        self.last_stop = Some(now);
        true
    }

    /// An interrupt line was seen (written by someone, or by the daemon): the queue waits, unless
    /// "send now" just announced it.
    pub fn interrupted(&mut self, now: Instant) -> bool {
        if self.spare_interrupt.take().is_some_and(|until| now < until) {
            return false;
        }
        if self.items.is_empty() || self.paused {
            return false;
        }
        self.paused = true;
        true
    }

    pub fn spare_next_interrupt(&mut self, now: Instant) {
        self.spare_interrupt = Some(now + SPARE_FOR);
    }

    /// The queue as the clients are told it. `dropped` lists what just left without being sent.
    pub fn view(&self, handle: u32, id: &str, dropped: &[Dropped]) -> Value {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|e| {
                json!({
                    "qid": e.qid, "cid": e.cid, "by": {"id": e.by.id, "name": e.by.name},
                    "kind": if e.origin == Origin::Phone { "phone" } else { "desktop" },
                    "text": clip(&e.text, EVENT_TEXT), "at": e.at, "needs_window": e.waiting_window,
                })
            })
            .collect();
        let mut v = json!({"handle": handle, "id": id, "items": items, "paused": self.paused, "held": self.held, "in_flight": self.flight.is_some()});
        if !dropped.is_empty() {
            v["dropped"] = json!(dropped
                .iter()
                .map(|d| {
                    let mut v = json!({"qid": d.qid, "cid": d.cid, "by": {"id": d.by.id, "name": d.by.name}, "text": clip(&d.text, EVENT_TEXT), "images": d.images, "reason": d.reason});
                    if let Some(c) = &d.cancelled_by {
                        v["cancelled_by"] = json!({"id": c.id, "name": c.name});
                    }
                    v
                })
                .collect::<Vec<_>>());
        }
        v
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
pub fn clip(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn by(id: &str) -> By {
        By { id: id.into(), name: format!("name-{id}") }
    }

    fn phone(device: &str, cid: &str, text: &str) -> NewEntry {
        NewEntry { cid: cid.into(), by: by(device), text: text.into(), images: vec![], origin: Origin::Phone }
    }

    fn desktop(cid: &str, text: &str) -> NewEntry {
        NewEntry { cid: cid.into(), by: By::desktop(), text: text.into(), images: vec![], origin: Origin::Desktop { client: 1, rich: false } }
    }

    #[test]
    fn messages_go_out_in_the_order_they_came_whoever_sent_them() {
        let (mut q, now) = (Queue::new(), Instant::now());
        let a = q.push(phone("p1", "a", "first"), now, 1).unwrap();
        let b = q.push(desktop("b", "second"), now, 2).unwrap();
        let c = q.push(phone("p2", "c", "third"), now, 3).unwrap();
        assert_eq!([a.1, b.1, c.1], [0, 1, 2], "how many are ahead");
        let texts: Vec<_> = q.items.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["first", "second", "third"]);
    }

    #[test]
    fn the_queue_is_bounded_per_chat_and_per_device() {
        let (mut q, now) = (Queue::new(), Instant::now());
        for i in 0..MAX_PER_DEVICE {
            q.push(phone("p1", &format!("c{i}"), "x"), now, 0).unwrap();
        }
        assert_eq!(q.push(phone("p1", "more", "x"), now, 0).unwrap_err(), Refused::QueueFull, "five from one phone");
        q.push(phone("p2", "other", "x"), now, 0).unwrap();
        // The desktop is not held to the phone limit, only to the chat's.
        for i in 0..(MAX_ENTRIES - MAX_PER_DEVICE - 1) {
            q.push(desktop(&format!("d{i}"), "x"), now, 0).unwrap();
        }
        assert_eq!(q.items.len(), MAX_ENTRIES);
        assert_eq!(q.push(phone("p3", "late", "x"), now, 0).unwrap_err(), Refused::QueueFull);
        assert_eq!(q.push(desktop("late", "x"), now, 0).unwrap_err(), Refused::QueueFull);
    }

    #[test]
    fn text_is_checked_by_where_it_comes_from() {
        let (mut q, now) = (Queue::new(), Instant::now());
        assert_eq!(q.push(phone("p", "a", "  \n "), now, 0).unwrap_err(), Refused::Empty);
        assert_eq!(q.push(phone("p", "b", &"x".repeat(MAX_PHONE_TEXT + 1)), now, 0).unwrap_err(), Refused::TooLong);
        assert!(q.push(phone("p", "c", &"x".repeat(MAX_PHONE_TEXT)), now, 0).is_ok(), "16 KiB exactly");
        assert!(q.push(desktop("d", &"x".repeat(MAX_PHONE_TEXT * 4)), now, 0).is_ok(), "a desktop entry may be longer");
        assert_eq!(q.push(desktop("e", &"x".repeat(MAX_DESKTOP_TEXT + 1)), now, 0).unwrap_err(), Refused::TooLong);
        // Pictures alone are a message.
        let mut pictures = desktop("f", "");
        pictures.images = vec!["a.png".into()];
        assert!(q.push(pictures, now, 0).is_ok());
    }

    #[test]
    fn a_retry_finds_what_became_of_the_first_try() {
        let (mut q, now) = (Queue::new(), Instant::now());
        let (qid, _) = q.push(phone("p1", "c1", "hi"), now, 0).unwrap();
        assert_eq!(q.lookup("p1", "c1", now), Some((qid.clone(), State::Queued)));
        assert_eq!(q.lookup("p2", "c1", now), None, "a cid belongs to its device");
        let entry = q.items.pop_front().unwrap();
        q.mark(&entry, State::Sent, now);
        assert_eq!(q.lookup("p1", "c1", now), Some((qid, State::Sent)));
        // Ten minutes later it is forgotten.
        assert_eq!(q.lookup("p1", "c1", now + SEEN_FOR + Duration::from_secs(1)), None);
    }

    #[test]
    fn a_message_that_still_waits_is_remembered_for_as_long_as_it_waits() {
        let (mut q, now) = (Queue::new(), Instant::now());
        let (qid, _) = q.push(phone("p1", "c1", "slow"), now, 0).unwrap();
        let later = now + SEEN_FOR + Duration::from_secs(60);
        assert_eq!(q.lookup("p1", "c1", later), Some((qid, State::Queued)), "a retry from an old chip finds it");
        let entry = q.items.pop_front().unwrap();
        q.mark(&entry, State::Sent, later);
        assert!(q.lookup("p1", "c1", later + SEEN_FOR + Duration::from_secs(1)).is_none(), "and once it is out the ten minutes count");
    }

    #[test]
    fn what_is_remembered_is_bounded() {
        let (mut q, now) = (Queue::new(), Instant::now());
        for i in 0..(SEEN_MAX + 50) {
            q.push(phone("p", &format!("c{i}"), "x"), now, 0).unwrap();
            q.items.clear();
        }
        assert!(q.seen.len() <= SEEN_MAX + 1 && q.order.len() <= SEEN_MAX + 1, "{} remembered", q.seen.len());
        assert_eq!(q.lookup("p", "c0", now), None, "the oldest went first");
        assert!(q.lookup("p", &format!("c{}", SEEN_MAX + 49), now).is_some());
    }

    #[test]
    fn cancel_works_on_what_still_waits_and_says_too_late_otherwise() {
        let (mut q, now) = (Queue::new(), Instant::now());
        let (a, _) = q.push(phone("p1", "a", "one"), now, 0).unwrap();
        let (b, _) = q.push(desktop("b", "two"), now, 0).unwrap();
        let (c, _) = q.push(desktop("c", "three"), now, 0).unwrap();
        // Either side may cancel either side's message.
        assert_eq!(q.cancel(&b, now).unwrap().text, "two");
        assert_eq!(q.lookup("desktop", "b", now).map(|x| x.1), Some(State::Cancelled));
        assert_eq!(q.cancel(&b, now).err(), Some(TooLate), "gone already");
        q.items[0].offered = Some(now);
        assert!(q.cancel(&a, now).is_err(), "offered: too late");
        assert!(q.cancel("nope", now).is_err());
        assert!(q.cancel(&c, now).is_ok());
    }

    #[test]
    fn front_moves_a_waiting_entry_to_the_head() {
        let (mut q, now) = (Queue::new(), Instant::now());
        q.push(phone("p", "a", "one"), now, 0).unwrap();
        let (b, _) = q.push(desktop("b", "two"), now, 0).unwrap();
        q.front(&b).unwrap();
        assert_eq!(q.items[0].text, "two");
        // The head was offered to its builder: it stays the head, and nothing is put ahead of it.
        q.items[0].offered = Some(now);
        let a = q.items[1].qid.clone();
        q.front(&a).unwrap();
        assert_eq!(q.items[0].text, "two");
        assert_eq!(q.front(&b), Err(TooLate), "an offered entry does not move");
    }

    #[test]
    fn dropping_marks_what_was_not_sent() {
        let (mut q, now) = (Queue::new(), Instant::now());
        q.push(phone("p1", "a", "one"), now, 0).unwrap();
        q.push(desktop("b", "two"), now, 0).unwrap();
        q.push(phone("p1", "c", "three"), now, 0).unwrap();
        let dropped = q.drop_where(|e| e.by.id == "p1", "revoked", now);
        assert_eq!(dropped.iter().map(|d| d.text.as_str()).collect::<Vec<_>>(), ["one", "three"]);
        assert_eq!(q.items.len(), 1);
        assert_eq!(q.lookup("p1", "a", now).map(|x| x.1), Some(State::Dropped));
        assert_eq!(q.drop_all("exited", now).len(), 1);
        assert!(q.items.is_empty());
    }

    #[test]
    fn a_held_queue_takes_nothing() {
        let (mut q, now) = (Queue::new(), Instant::now());
        q.held = true;
        assert_eq!(q.push(phone("p", "a", "x"), now, 0).unwrap_err(), Refused::Switching);
        assert_eq!(q.push(desktop("a", "x"), now, 0).unwrap_err(), Refused::Switching);
    }

    #[test]
    fn an_interrupt_pauses_a_queue_that_has_something_in_it() {
        let (mut q, now) = (Queue::new(), Instant::now());
        assert!(!q.interrupted(now), "nothing waits");
        q.push(phone("p", "a", "x"), now, 0).unwrap();
        assert!(q.interrupted(now) && q.paused);
        // "Send now" announced its interrupt: that one does not pause.
        q.paused = false;
        q.spare_next_interrupt(now);
        assert!(!q.interrupted(now) && !q.paused);
        assert!(q.interrupted(now) && q.paused, "only the one interrupt was spared");
    }

    #[test]
    fn a_second_stop_within_a_second_is_the_same_stop() {
        let (mut q, now) = (Queue::new(), Instant::now());
        assert!(q.stop_now(now));
        assert!(!q.stop_now(now + Duration::from_millis(500)));
        assert!(q.stop_now(now + Duration::from_millis(1500)));
    }

    #[test]
    fn the_view_names_who_sent_what_and_what_was_dropped() {
        let (mut q, now) = (Queue::new(), Instant::now());
        q.push(phone("p1", "a", "from the phone"), now, 5).unwrap();
        q.paused = true;
        let dropped = vec![Dropped { qid: "q9".into(), cid: "z".into(), by: By::desktop(), text: "lost".into(), images: vec![], reason: "needs_window", cancelled_by: None }];
        let v = q.view(3, "chat-1", &dropped);
        assert_eq!((v["handle"].clone(), v["id"].clone(), v["paused"].clone(), v["in_flight"].clone()), (json!(3), json!("chat-1"), json!(true), json!(false)));
        assert_eq!((v["items"][0]["kind"].clone(), v["items"][0]["by"]["name"].clone()), (json!("phone"), json!("name-p1")));
        assert_eq!(v["dropped"][0]["reason"], "needs_window");
        assert!(q.view(3, "chat-1", &[]).get("dropped").is_none());
        assert_eq!(clip("héllo", 2), "h", "cut at a character, not in the middle of one");
    }
}
