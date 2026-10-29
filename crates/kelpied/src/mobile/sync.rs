//! Keeping a phone's view of a terminal session up to date: when something changed, the rows
//! that differ from what that phone last confirmed, not the whole screen again.
//!
//! One task per followed session (`drive`, shared by every phone following it) waits for the
//! session's wake signal and, at most once per gap the load level allows, takes a snapshot of its
//! end under the session lock; nothing runs for a session nobody follows. One task per phone and
//! session (`run`) diffs the latest snapshot against the rows the phone last *acknowledged* (`ack`),
//! outside the lock. A frame therefore names the acknowledged frame it follows and says everything
//! that changed since, so frames in flight may be lost with their connection, and a phone that
//! reconnects, or moves between the direct and the relay path, carries on from the last frame it
//! acknowledged. At most `WINDOW_FRAMES` frames / `WINDOW_BYTES` wait for an acknowledgement; past
//! that the phone gets nothing until it answers, and then only the latest state, never a queue of
//! old ones. What was acknowledged is kept per (device, session) within `Views`' bounds.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::{mpsc, watch, Notify};

use crate::choices::Prompt;
use crate::load::{Level, Load};
use crate::session::{now_millis, ScreenView, Session};

/// How long what was last acknowledged is kept after the phone's connection ended.
pub const RESUME_TTL: Duration = Duration::from_secs(300);
/// Resume cache: rows kept per (device, session) fit a frame (64 KiB), at most this many sessions
/// of a device and devices, and together this much; the least recently used go first.
const RESUME_SESSIONS_PER_DEVICE: usize = 20;
const RESUME_DEVICES: usize = 8;
const RESUME_BYTES: usize = 16 << 20;
/// Frames and bytes a phone may have sent and not acknowledged, per session.
pub const WINDOW_FRAMES: usize = 4;
pub const WINDOW_BYTES: usize = 128 * 1024;
/// Rows a view may have; also the most a phone can ask for.
pub const MAX_HEIGHT: usize = 200;
pub const MIN_HEIGHT: usize = 10;
/// A row longer than this is cut (a phone shows a hundred columns or so).
const MAX_ROW_CHARS: usize = 300;
/// The rows of a snapshot together stay under this, so that a frame stays under 64 KiB.
const MAX_ROWS_BYTES: usize = 56 * 1024;
/// How far the screen may have scrolled between two frames for the diff to still follow it.
const MAX_SHIFT: usize = 64;
const MAX_TITLE_CHARS: usize = 200;
/// A session whose frames average more than this redraws most of the screen each time (a full-screen
/// program, a flood of output): its phones get frames less often (`Level::heavy_gap`).
const HEAVY_FRAME_BYTES: f64 = 8.0 * 1024.0;

/// `line` cut to at most `chars` characters.
fn cut(line: &str, chars: usize) -> &str {
    line.char_indices().nth(chars).map_or(line, |(at, _)| &line[..at])
}

/// What a row costs in a frame: its text, the `[index,"…"],` around it, and escapes.
fn cost(line: &str) -> usize {
    line.len() + 8 + line.bytes().filter(|b| matches!(b, b'"' | b'\\') || *b < 0x20).count() * 5
}

/// Cuts rows to what a frame can carry: each to `MAX_ROW_CHARS`, and all together to
/// `MAX_ROWS_BYTES` (a screen of very long lines gets shorter rows, the same way every time).
fn fit(lines: Vec<String>) -> Vec<String> {
    let mut cap = MAX_ROW_CHARS;
    loop {
        let bytes: usize = lines.iter().map(|l| cost(cut(l, cap))).sum();
        if bytes <= MAX_ROWS_BYTES || cap <= 16 {
            return lines.into_iter().map(|l| if l.len() <= cap { l } else { cut(&l, cap).to_string() }).collect();
        }
        cap /= 2;
    }
}

/// How the rows of `new` differ from `old` after the screen scrolled up by some lines: the
/// scroll (`shift`, lines dropped from the top of `old`) that leaves the fewest rows to send, and
/// the indexes of the rows that still differ. A line printed under a full screen then costs one
/// row, not all of them.
pub fn diff(old: &[Arc<str>], new: &[Arc<str>]) -> (usize, Vec<usize>) {
    // Rows are compared by their text (a row past the old window is blank).
    let differs = |shift: usize, i: usize| match old.get(i + shift) {
        Some(o) => !(Arc::ptr_eq(o, &new[i]) || **o == *new[i]),
        None => !new[i].is_empty(),
    };
    let mut best = (0, (0..new.len()).filter(|&i| differs(0, i)).count());
    for shift in 1..=old.len().min(MAX_SHIFT) {
        if best.1 == 0 {
            break;
        }
        // Counted, and given up on as soon as it cannot beat the best so far.
        let mut count = 0;
        for i in 0..new.len() {
            count += usize::from(differs(shift, i));
            if count >= best.1 {
                break;
            }
        }
        if count < best.1 {
            best = (shift, count);
        }
    }
    (best.0, (0..new.len()).filter(|&i| differs(best.0, i)).collect())
}

/// The end of a session's screen as of one moment: shared by every phone that
/// follows the session (a phone that wants fewer rows takes the last of them).
pub struct Snapshot {
    lines: Vec<Arc<str>>,
    cursor: Option<(usize, usize)>,
    prompt: Option<Prompt>,
    waiting_since: Option<u64>,
    title: Option<String>,
}

impl Snapshot {
    fn of(view: ScreenView) -> Snapshot {
        Snapshot {
            lines: fit(view.lines).into_iter().map(Arc::from).collect(),
            cursor: view.cursor,
            prompt: view.prompt,
            waiting_since: view.waiting_since,
            title: view.title.map(|t| t.chars().take(MAX_TITLE_CHARS).collect()),
        }
    }
}

/// What a frame carries besides its rows; a change in any of it is a frame.
#[derive(Clone, PartialEq)]
struct Extras {
    height: usize,
    cursor: Option<(usize, usize)>,
    prompt: Option<Prompt>,
    waiting_since: Option<u64>,
    title: Option<String>,
}

/// The screen a phone holds after a frame: its number and what it says.
struct Held {
    rev: u64,
    rows: Vec<Arc<str>>,
    extras: Extras,
    /// Size of the frame that brought it.
    bytes: usize,
}

/// What one phone was sent of one session.
struct Slot {
    /// The connection following it; another taking over ends the first's task.
    owner: u64,
    /// The last frame number given out. Numbers are never reused, so a phone that holds a frame
    /// from before a reconnect cannot mistake it for another.
    counter: u64,
    /// The last frame the phone confirmed holding; frames follow it. None: it holds nothing known.
    acked: Option<Held>,
    /// Frames sent after it and not confirmed, oldest first.
    flying: VecDeque<Held>,
    /// Since when nobody follows it, and when it was last used (the least recently used are dropped first).
    left: Option<Instant>,
    used: Instant,
    /// Woken when an acknowledgement arrives: a frame that waited for room may go.
    wake: Arc<Notify>,
    /// The phone never acknowledges (see `open_as`).
    legacy: bool,
}

impl Slot {
    fn latest(&self) -> Option<&Held> {
        self.flying.back().or(self.acked.as_ref())
    }

    /// The memory this slot holds while nobody follows it.
    fn kept(&self) -> usize {
        self.acked.as_ref().map_or(0, |a| a.rows.iter().map(|r| r.len()).sum())
    }
}

#[derive(Debug)]
pub struct Lost;

/// What the next frame for a phone is.
pub enum Next {
    /// A frame to send; `more` when rows that changed did not fit and another should follow.
    Frame { text: String, more: bool },
    /// The phone holds this already.
    Same,
    /// Frames are waiting for an acknowledgement: nothing until it comes.
    Blocked,
}

type Snapshots = watch::Receiver<Option<Arc<Snapshot>>>;

/// The shared render of one followed session.
struct Feed {
    snapshots: watch::Sender<Option<Arc<Snapshot>>>,
    /// Until when (ms since the epoch) someone is typing into the session: its frames go at the calm rate
    /// whatever the load, since the echo of what is typed is what the person waits for.
    fast_until: AtomicU64,
    /// Woken when typing begins, so that a render waiting out a long gap does not make the echo wait with it.
    poke: Notify,
}

/// How long after a key a session keeps its calm frame rate.
const TYPING_BOOST_MS: u64 = 3000;

pub struct Views {
    slots: Mutex<HashMap<(String, String), Slot>>,
    feeds: Mutex<HashMap<String, Arc<Feed>>>,
    load: Arc<Load>,
}

impl Default for Views {
    fn default() -> Self {
        Views::new(Load::new())
    }
}

impl Views {
    pub fn new(load: Arc<Load>) -> Views {
        Views { slots: Mutex::new(HashMap::new()), feeds: Mutex::new(HashMap::new()), load }
    }

    /// A connection starts following `session` for `device`. `have` is the frame the phone says it
    /// holds: if that is the last it confirmed, frames carry on from it; else the next one is whole.
    /// The returned signal fires when an acknowledgement arrives.
    #[cfg(test)]
    pub fn open(&self, device: &str, session: &str, conn: u64, have: Option<u64>) -> Arc<Notify> {
        self.open_as(device, session, conn, have, false)
    }

    /// [`Self::open`] for a phone that may not acknowledge (`legacy`: a page from before frames were
    /// acknowledged): every frame it is sent counts as confirmed at once, as the first protocol had it,
    /// so no window holds it back.
    pub fn open_as(&self, device: &str, session: &str, conn: u64, have: Option<u64>, legacy: bool) -> Arc<Notify> {
        let mut slots = self.slots.lock();
        let now = Instant::now();
        let slot = slots.entry((device.to_string(), session.to_string())).or_insert_with(|| Slot { owner: conn, counter: 0, acked: None, flying: VecDeque::new(), left: None, used: now, wake: Arc::new(Notify::new()), legacy });
        if have.is_none_or(|h| slot.acked.as_ref().is_none_or(|a| a.rev != h)) {
            slot.acked = None;
        }
        // What was sent and not confirmed may or may not have arrived: the phone's word is the base.
        slot.flying.clear();
        slot.owner = conn;
        slot.legacy = legacy;
        slot.left = None;
        slot.used = now;
        let wake = slot.wake.clone();
        self.trim(&mut slots);
        wake
    }

    /// The connection ended: what was confirmed is kept for a while.
    pub fn leave(&self, device: &str, conn: u64) {
        let mut slots = self.slots.lock();
        let now = Instant::now();
        for ((d, _), slot) in slots.iter_mut() {
            if d == device && slot.owner == conn && slot.left.is_none() {
                slot.left = Some(now);
                slot.used = now;
                slot.flying.clear();
            }
        }
        self.trim(&mut slots);
    }

    /// Nothing follows `session` for `device` any more (the phone stopped, or it is gone).
    pub fn close(&self, device: &str, session: &str, conn: u64) {
        let mut slots = self.slots.lock();
        let key = (device.to_string(), session.to_string());
        if slots.get(&key).is_some_and(|s| s.owner == conn) {
            slots.remove(&key);
        }
    }

    /// The phone confirms it holds frame `rev` of `session`: frames follow it from now on.
    pub fn ack(&self, device: &str, session: &str, conn: u64, rev: u64) {
        let mut slots = self.slots.lock();
        let Some(slot) = slots.get_mut(&(device.to_string(), session.to_string())).filter(|s| s.owner == conn) else { return };
        if let Some(at) = slot.flying.iter().position(|h| h.rev == rev) {
            slot.acked = slot.flying.drain(..=at).next_back();
            slot.used = Instant::now();
            slot.wake.notify_one();
        }
    }

    /// Slots held, followed or not.
    #[cfg(test)]
    pub fn slot_count(&self) -> usize {
        self.slots.lock().len()
    }

    /// Drops what has been kept too long (see `trim`).
    pub fn sweep(&self) {
        self.trim(&mut self.slots.lock());
    }

    /// Enforces the resume cache's limits: too old, too many for a device, too many devices, too much
    /// memory. Only slots nobody follows are ever dropped, the least recently used first.
    fn trim(&self, slots: &mut HashMap<(String, String), Slot>) {
        slots.retain(|_, s| s.left.is_none_or(|at| at.elapsed() < RESUME_TTL));
        // Short of memory (not merely busy): nothing is kept for phones that are away. Checked wherever
        // the cache is touched, so a phone that comes back finds it empty even if no one else is connected.
        if self.load.status().short_of_memory() {
            slots.retain(|_, s| s.left.is_none());
        }
        let oldest = |slots: &HashMap<(String, String), Slot>, of: &dyn Fn(&(String, String)) -> bool| {
            slots.iter().filter(|(k, s)| s.left.is_some() && of(k)).min_by_key(|(_, s)| s.used).map(|(k, _)| k.clone())
        };
        loop {
            let mut per_device: HashMap<&str, usize> = HashMap::new();
            for (d, _) in slots.keys() {
                *per_device.entry(d).or_default() += 1;
            }
            let crowded = per_device.iter().find(|(_, n)| **n > RESUME_SESSIONS_PER_DEVICE).map(|(d, _)| d.to_string());
            let victim = if let Some(d) = crowded {
                oldest(slots, &|k| k.0 == d)
            } else if per_device.len() > RESUME_DEVICES {
                // The device whose newest slot is oldest, and its oldest slot.
                let stalest = per_device.keys().filter_map(|d| slots.iter().filter(|(k, s)| k.0 == *d && s.left.is_some()).map(|(_, s)| s.used).max().map(|used| (used, d.to_string()))).min().map(|(_, d)| d);
                stalest.and_then(|d| oldest(slots, &|k| k.0 == d))
            } else if slots.values().map(Slot::kept).sum::<usize>() > RESUME_BYTES {
                oldest(slots, &|_| true)
            } else {
                None
            };
            match victim {
                Some(key) => {
                    slots.remove(&key);
                }
                None => return,
            }
        }
    }

    /// Someone typed into `session`: for a few seconds it is rendered and sent at the calm rate.
    pub fn boost(&self, session: &str) {
        if let Some(feed) = self.feeds.lock().get(session) {
            feed.fast_until.store(now_millis() + TYPING_BOOST_MS, Ordering::Relaxed);
            feed.poke.notify_one();
        }
    }

    fn typing(&self, session: &str) -> bool {
        self.feeds.lock().get(session).is_some_and(|f| f.fast_until.load(Ordering::Relaxed) > now_millis())
    }

    /// The renders of `id`, started if nobody has them yet. Its task ends with its last follower.
    fn follow(self: &Arc<Self>, id: &str, session: &Weak<Session>) -> Snapshots {
        let mut feeds = self.feeds.lock();
        // A new follower has not seen what is already rendered.
        let subscribe = |feed: &Feed| {
            let mut receiver = feed.snapshots.subscribe();
            receiver.mark_changed();
            receiver
        };
        if let Some(feed) = feeds.get(id).filter(|f| f.snapshots.receiver_count() > 0) {
            return subscribe(feed);
        }
        let feed = Arc::new(Feed { snapshots: watch::channel(None).0, fast_until: AtomicU64::new(0), poke: Notify::new() });
        let receiver = subscribe(&feed);
        feeds.insert(id.to_string(), feed.clone());
        tokio::spawn(drive(self.clone(), id.to_string(), session.clone(), feed));
        receiver
    }

    /// The frame that brings the phone from the last frame it confirmed to the end of `snapshot`
    /// (its last `height` rows), if it differs from what it holds now. At most the rows `level`
    /// allows go in one frame, the ones at the bottom first.
    pub fn frame(&self, device: &str, session: &str, conn: u64, snapshot: &Snapshot, height: usize, level: Level) -> Result<Next, Lost> {
        let skip = snapshot.lines.len().saturating_sub(height);
        let lines = &snapshot.lines[skip..];
        let extras = Extras {
            height: lines.len(),
            cursor: snapshot.cursor.and_then(|(row, col)| row.checked_sub(skip).map(|row| (row, col))),
            prompt: snapshot.prompt.clone(),
            waiting_since: snapshot.waiting_since,
            title: snapshot.title.clone(),
        };
        let mut slots = self.slots.lock();
        let slot = slots.get_mut(&(device.to_string(), session.to_string())).filter(|s| s.owner == conn).ok_or(Lost)?;
        slot.used = Instant::now();
        if slot.flying.len() >= WINDOW_FRAMES || slot.flying.iter().map(|h| h.bytes).sum::<usize>() >= WINDOW_BYTES {
            return Ok(Next::Blocked);
        }
        if let Some(latest) = slot.latest() {
            let (shift, rows) = diff(&latest.rows, lines);
            if rows.is_empty() && shift == 0 && latest.extras == extras {
                return Ok(Next::Same);
            }
        }
        let (base, shift, changed) = match &slot.acked {
            Some(a) => {
                let (shift, rows) = diff(&a.rows, lines);
                (Some(a.rev), shift, rows)
            }
            None => (None, 0, (0..lines.len()).filter(|&i| !lines[i].is_empty()).collect()),
        };
        // From the bottom: what the phone looks at first.
        let (mut budget, mut sent) = (level.frame_rows_bytes(), Vec::new());
        for &i in changed.iter().rev() {
            let c = cost(&lines[i]);
            if c > budget && !sent.is_empty() {
                break;
            }
            budget = budget.saturating_sub(c);
            sent.push(i);
        }
        sent.reverse();
        let more = sent.len() < changed.len();
        // What the phone holds once it has applied this: the confirmed screen scrolled, with these rows in.
        let blank: Arc<str> = Arc::from("");
        let mut rows: Vec<Arc<str>> = (0..lines.len()).map(|i| slot.acked.as_ref().and_then(|a| a.rows.get(i + shift)).unwrap_or(&blank).clone()).collect();
        for &i in &sent {
            rows[i] = lines[i].clone();
        }
        slot.counter += 1;
        let rev = slot.counter;
        let frame = Frame {
            kind: "screen", id: session, rev, base, shift,
            rows: sent.iter().map(|&i| (i, &*lines[i])).collect(),
            height: extras.height, cursor: extras.cursor, prompt: &extras.prompt,
            waiting_since: extras.waiting_since, title: &extras.title,
            server_ts: now_millis(), level: level.number(),
        };
        let text = serde_json::to_string(&frame).expect("a frame is plain data");
        let held = Held { rev, rows, extras, bytes: text.len() };
        if slot.legacy {
            slot.acked = Some(held);
        } else {
            slot.flying.push_back(held);
        }
        Ok(Next::Frame { text, more })
    }
}

/// A screen frame as it goes out.
#[derive(Serialize)]
struct Frame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    id: &'a str,
    rev: u64,
    base: Option<u64>,
    shift: usize,
    rows: Vec<(usize, &'a str)>,
    height: usize,
    cursor: Option<(usize, usize)>,
    prompt: &'a Option<Prompt>,
    waiting_since: Option<u64>,
    title: &'a Option<String>,
    /// When the daemon made it, and the load level then: the phone can show how old what it sees is.
    server_ts: u64,
    level: u8,
}

/// Renders `id` for its followers: when the session says it changed, at most as often as the load
/// level allows.
async fn drive(views: Arc<Views>, id: String, session: Weak<Session>, feed: Arc<Feed>) {
    if let Some(mut wake) = session.upgrade().map(|s| s.watch()) {
        loop {
            // Anything after this is seen as a change next time round.
            wake.borrow_and_update();
            let Some(view) = session.upgrade().map(|s| s.screen_view(MAX_HEIGHT)) else { break };
            feed.snapshots.send_replace(Some(Arc::new(Snapshot::of(view))));
            let gap = if feed.fast_until.load(Ordering::Relaxed) > now_millis() { Level::L0.focused_gap() } else { views.load.level().focused_gap() };
            tokio::select! {
                _ = tokio::time::sleep(gap) => {}
                _ = feed.poke.notified() => {}
            }
            tokio::select! {
                _ = feed.snapshots.closed() => {
                    // Decided under the lock `follow` subscribes under: a follower that arrived since the
                    // last one left keeps the render going (else it would wait on a render nobody makes).
                    let mut feeds = views.feeds.lock();
                    if feed.snapshots.receiver_count() == 0 {
                        if feeds.get(&id).is_some_and(|f| Arc::ptr_eq(f, &feed)) {
                            feeds.remove(&id);
                        }
                        return;
                    }
                }
                changed = wake.changed() => if changed.is_err() { break },
            }
        }
    }
    let mut feeds = views.feeds.lock();
    if feeds.get(&id).is_some_and(|f| Arc::ptr_eq(f, &feed)) {
        feeds.remove(&id);
    }
}

/// One phone following one session.
pub struct Follow {
    pub views: Arc<Views>,
    pub device: String,
    pub conn: u64,
    pub id: String,
    pub height: usize,
    pub session: Weak<Session>,
    pub out: mpsc::Sender<String>,
    /// The session the phone is looking at (the others get frames less often when the machine is busy).
    pub focused: Arc<AtomicBool>,
    /// Woken by an acknowledgement (`Views::open` gives it).
    pub acked: Arc<Notify>,
}

/// Sends frames until the phone stops following, the connection is gone or the session is.
pub async fn run(f: Follow) {
    let mut snapshots = f.views.follow(&f.id, &f.session);
    let mut levels = f.views.load.subscribe();
    // The size of recent frames, to notice a session that redraws everything each time; when the last went.
    let mut average = 0.0;
    let mut last: Option<Instant> = None;
    let mut more = false;
    'frames: loop {
        if !more {
            tokio::select! {
                changed = snapshots.changed() => if changed.is_err() { break },
                _ = f.acked.notified() => {}
                _ = levels.changed() => {}
            }
        }
        more = false;
        // Spaced out by the level (a calm machine's gap is the render's own): the wait comes before the
        // state is read, so that what is sent is the newest. It is looked at again every 50 ms, since the
        // level may change, and so may someone start typing, while it lasts.
        let level = loop {
            let level = f.views.load.level();
            let calm = f.views.typing(&f.id).then(|| Level::L0.focused_gap());
            let gap = calm.or(if f.focused.load(Ordering::Relaxed) { Some(level.focused_gap()) } else { level.background_gap() });
            // Paused (a background session at L3): the next change of level or of the session wakes it.
            let Some(gap) = gap else { continue 'frames };
            let gap = if average > HEAVY_FRAME_BYTES { gap.max(level.heavy_gap()) } else { gap };
            match last.map(|at| gap.saturating_sub(at.elapsed())).filter(|w| !w.is_zero()) {
                Some(wait) => tokio::time::sleep(wait.min(Duration::from_millis(50))).await,
                None => break level,
            }
        };
        // A place in the socket's queue first: the state is read only once it can be sent.
        let Ok(slot) = f.out.reserve().await else { break };
        let Some(snapshot) = snapshots.borrow_and_update().clone() else { continue };
        match f.views.frame(&f.device, &f.id, f.conn, &snapshot, f.height, level) {
            Ok(Next::Frame { text, more: rest }) => {
                average = average * 0.7 + text.len() as f64 * 0.3;
                slot.send(text);
                last = Some(Instant::now());
                more = rest;
            }
            Ok(Next::Same | Next::Blocked) => {}
            Err(Lost) => return,
        }
    }
    f.views.close(&f.device, &f.id, f.conn);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionSpec;
    use serde_json::{json, Value};

    fn rows(lines: &[&str]) -> Vec<Arc<str>> {
        lines.iter().map(|l| Arc::from(*l)).collect()
    }

    fn view(lines: &[&str]) -> ScreenView {
        ScreenView { lines: lines.iter().map(|l| l.to_string()).collect(), cursor: None, prompt: None, waiting_since: None, title: None }
    }

    fn snapshot(lines: &[&str]) -> Snapshot {
        Snapshot::of(view(lines))
    }

    fn json_of(next: Result<Next, Lost>) -> Option<Value> {
        match next {
            Ok(Next::Frame { text, .. }) => Some(serde_json::from_str(&text).unwrap()),
            Ok(_) => None,
            Err(_) => panic!("the connection no longer follows"),
        }
    }

    /// The frame for `lines`, the phone showing all of them (at the calmest level).
    fn frame(views: &Views, device: &str, session: &str, conn: u64, lines: &[&str]) -> Option<Value> {
        json_of(views.frame(device, session, conn, &snapshot(lines), MAX_HEIGHT, Level::L0))
    }

    #[test]
    fn only_changed_rows_are_sent() {
        let old = rows(&["a", "b", "c", "d"]);
        assert_eq!(diff(&old, &old), (0, vec![]));
        // Same text in another allocation, and a different row with the same length.
        assert_eq!(diff(&old, &rows(&["a", "b", "c", "d"])), (0, vec![]));
        assert_eq!(diff(&old, &rows(&["a", "x", "c", "d"])), (0, vec![1]));
        assert_eq!(diff(&old, &rows(&["a", "B", "c", "d"])), (0, vec![1]));
        // A screen redrawn from top to bottom costs every row.
        assert_eq!(diff(&old, &rows(&["w", "x", "y", "z"])).1.len(), 4);
    }

    #[test]
    fn a_scrolled_screen_costs_the_new_lines_only() {
        let old = rows(&["l0", "l1", "l2", "l3"]);
        // Two lines were printed: the window moved up by two.
        assert_eq!(diff(&old, &rows(&["l2", "l3", "l4", "l5"])), (2, vec![2, 3]));
        // The window grew while the log was shorter than it.
        assert_eq!(diff(&rows(&["x"]), &rows(&["x", "y"])), (0, vec![1]));
        // Rows past the old window that are empty need no sending.
        assert_eq!(diff(&rows(&["x"]), &rows(&["x", ""])), (0, vec![]));
    }

    #[test]
    fn rows_are_cut_to_a_frame() {
        let long = "x".repeat(MAX_ROW_CHARS * 2);
        let cut = fit(vec![long.clone(), "short".into(), "é".repeat(MAX_ROW_CHARS + 1)]);
        assert_eq!((cut[0].chars().count(), cut[1].as_str(), cut[2].chars().count()), (MAX_ROW_CHARS, "short", MAX_ROW_CHARS));
        // A full screen of the longest rows still makes a frame under 64 KiB.
        let views = Views::default();
        views.open("d", "s", 1, None);
        let screen: Vec<&str> = vec![long.as_str(); MAX_HEIGHT];
        let frame = frame(&views, "d", "s", 1, &screen).unwrap();
        assert!(frame.to_string().len() < 64 * 1024, "{}", frame.to_string().len());
    }

    #[test]
    fn frames_follow_the_last_confirmed_one_and_resume_across_connections() {
        let views = Views::default();
        views.open("phone", "s1", 1, None);
        let first = frame(&views, "phone", "s1", 1, &["one", "two"]).unwrap();
        assert_eq!(first["base"], Value::Null);
        assert_eq!(first["rev"], 1);
        assert_eq!(first["rows"], json!([[0, "one"], [1, "two"]]));
        assert!(first["server_ts"].as_u64().unwrap() > 0 && first["level"] == 0, "{first}");
        assert!(frame(&views, "phone", "s1", 1, &["one", "two"]).is_none(), "nothing changed, nothing sent");
        // Nothing is confirmed yet: the next frame is whole again, and says so.
        let second = frame(&views, "phone", "s1", 1, &["one", "three"]).unwrap();
        assert_eq!((second["base"].clone(), second["rev"].clone(), second["rows"].clone()), (Value::Null, json!(2), json!([[0, "one"], [1, "three"]])));
        views.ack("phone", "s1", 1, 2);
        // From a confirmed frame the next carries only what differs from it.
        let third = frame(&views, "phone", "s1", 1, &["one", "three", "four"]).unwrap();
        assert_eq!((third["base"].clone(), third["rows"].clone()), (json!(2), json!([[2, "four"]])));
        // The phone confirms frame 3 late: frames sent before that still follow frame 2, and say everything since.
        let fourth = frame(&views, "phone", "s1", 1, &["one", "three", "four", "five"]).unwrap();
        assert_eq!((fourth["base"].clone(), fourth["rows"].clone()), (json!(2), json!([[2, "four"], [3, "five"]])));
        views.ack("phone", "s1", 1, 4);

        // The connection drops and the phone comes back on another with the last frame it confirmed.
        views.leave("phone", 1);
        views.open("phone", "s1", 2, Some(4));
        assert!(views.frame("phone", "s1", 1, &snapshot(&["x"]), MAX_HEIGHT, Level::L0).is_err(), "the old connection no longer follows");
        let resumed = frame(&views, "phone", "s1", 2, &["one", "three", "four", "five", "six"]).unwrap();
        assert_eq!((resumed["base"].clone(), resumed["rows"].clone(), resumed["rev"].clone()), (json!(4), json!([[4, "six"]]), json!(5)), "numbers are not reused");

        // A phone that lost its place (or a different device) gets the whole view.
        views.open("phone", "s1", 3, Some(3));
        let whole = frame(&views, "phone", "s1", 3, &["one", "three", "four", "five", "six"]).unwrap();
        assert_eq!(whole["base"], Value::Null);
        assert_eq!(whole["rows"].as_array().unwrap().len(), 5);
        views.open("other", "s1", 4, Some(4));
        assert_eq!(frame(&views, "other", "s1", 4, &["z"]).unwrap()["base"], Value::Null);
    }

    #[test]
    fn at_most_four_frames_wait_for_an_acknowledgement() {
        let views = Views::default();
        views.open("p", "s", 1, None);
        for n in 1..=WINDOW_FRAMES {
            let lines = [format!("line {n}")];
            let sent = frame(&views, "p", "s", 1, &[lines[0].as_str()]);
            assert_eq!(sent.map(|f| f["rev"].as_u64().unwrap()), Some(n as u64));
        }
        let before = views.frame("p", "s", 1, &snapshot(&["line 5"]), MAX_HEIGHT, Level::L0);
        assert!(matches!(before, Ok(Next::Blocked)), "the fifth waits");
        // Acknowledging frame 2 makes room, and what goes next is the latest state, after frame 2.
        views.ack("p", "s", 1, 2);
        let after = frame(&views, "p", "s", 1, &["line 5"]).unwrap();
        assert_eq!((after["base"].clone(), after["rows"].clone()), (json!(2), json!([[0, "line 5"]])));
        // An acknowledgement of a frame never sent, or an old one, changes nothing.
        views.ack("p", "s", 1, 99);
        views.ack("p", "s", 1, 1);
        assert_eq!(views.slots.lock()[&("p".to_string(), "s".to_string())].acked.as_ref().unwrap().rev, 2);
    }

    #[test]
    fn a_phone_that_cannot_acknowledge_is_not_held_back_and_frames_follow_the_last_sent() {
        let views = Views::default();
        views.open_as("old", "s", 1, None, true);
        let mut previous = None;
        for n in 1..=(2 * WINDOW_FRAMES) {
            let line = format!("line {n}");
            let sent = frame(&views, "old", "s", 1, &[line.as_str()]).expect("never blocked");
            assert_eq!(sent["base"], previous.map_or(Value::Null, |rev: u64| json!(rev)), "frame {n} follows the one before");
            previous = sent["rev"].as_u64();
        }
        // An acknowledging phone, for contrast, is stopped after four.
        views.open("new", "s", 2, None);
        let sent: usize = (0..2 * WINDOW_FRAMES).filter(|n| frame(&views, "new", "s", 2, &[format!("line {n}").as_str()]).is_some()).count();
        assert_eq!(sent, WINDOW_FRAMES);
    }

    #[test]
    fn the_window_is_bytes_too() {
        let views = Views::default();
        views.open("p", "s", 1, None);
        // Screens of long rows: a frame is about 55 KB, so two fit in 128 KiB and a third is the last.
        let big = |c: char| vec![c.to_string().repeat(280); 195];
        let mut frames = 0;
        for c in ['a', 'b', 'c', 'd'] {
            let lines = big(c);
            let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
            match views.frame("p", "s", 1, &snapshot(&refs), MAX_HEIGHT, Level::L0).unwrap() {
                Next::Frame { text, .. } => {
                    assert!(text.len() < 64 * 1024, "{}", text.len());
                    frames += 1;
                }
                Next::Blocked => break,
                Next::Same => unreachable!(),
            }
        }
        assert_eq!(frames, 3, "the window is {WINDOW_BYTES} bytes, not only {WINDOW_FRAMES} frames");
    }

    #[test]
    fn when_the_machine_is_loaded_a_frame_carries_the_bottom_rows_that_fit_and_the_rest_follows() {
        let views = Views::default();
        views.open("p", "s", 1, None);
        let lines: Vec<String> = (0..60).map(|i| format!("{i:03} {}", "x".repeat(290))).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let snap = snapshot(&refs);
        let Ok(Next::Frame { text, more }) = views.frame("p", "s", 1, &snap, MAX_HEIGHT, Level::L3) else { panic!("a frame") };
        let first: Value = serde_json::from_str(&text).unwrap();
        assert!(text.len() <= 16 * 1024, "{} bytes at L3", text.len());
        assert!(more, "not everything fit");
        let sent: Vec<u64> = first["rows"].as_array().unwrap().iter().map(|r| r[0].as_u64().unwrap()).collect();
        assert_eq!(*sent.last().unwrap(), 59, "the bottom row first");
        assert!(sent.len() < 60 && sent.windows(2).all(|w| w[1] == w[0] + 1), "{sent:?}");
        assert_eq!(first["level"], 3);
        // Once it is confirmed, the rest goes (in as many frames as it takes), and nothing is sent twice.
        let mut got = sent.len();
        let mut rev = first["rev"].as_u64().unwrap();
        loop {
            views.ack("p", "s", 1, rev);
            match views.frame("p", "s", 1, &snap, MAX_HEIGHT, Level::L3).unwrap() {
                Next::Frame { text, .. } => {
                    let frame: Value = serde_json::from_str(&text).unwrap();
                    got += frame["rows"].as_array().unwrap().len();
                    rev = frame["rev"].as_u64().unwrap();
                }
                Next::Same => break,
                Next::Blocked => panic!("blocked after an acknowledgement"),
            }
        }
        assert_eq!(got, 60);
    }

    #[test]
    fn a_phone_that_wants_fewer_rows_gets_the_last_of_them() {
        let views = Views::default();
        views.open("phone", "s1", 1, None);
        let mut snap = snapshot(&["a", "b", "c", "d"]);
        snap.cursor = Some((3, 2));
        let frame = json_of(views.frame("phone", "s1", 1, &snap, 2, Level::L0)).unwrap();
        assert_eq!((frame["height"].clone(), frame["rows"].clone(), frame["cursor"].clone()), (json!(2), json!([[0, "c"], [1, "d"]]), json!([1, 2])));
        // A cursor above the rows it shows is not shown.
        snap.cursor = Some((0, 0));
        let frame = json_of(views.frame("phone", "s1", 1, &snap, 2, Level::L0)).unwrap();
        assert_eq!(frame["cursor"], Value::Null);
    }

    #[test]
    fn what_was_confirmed_is_kept_for_five_minutes_after_leaving() {
        let views = Views::default();
        views.open("phone", "s1", 1, None);
        frame(&views, "phone", "s1", 1, &["a"]).unwrap();
        views.ack("phone", "s1", 1, 1);
        views.leave("phone", 1);
        views.slots.lock().get_mut(&("phone".to_string(), "s1".to_string())).unwrap().left = Some(Instant::now() - RESUME_TTL - Duration::from_secs(1));
        views.open("phone", "s2", 1, None);
        assert!(!views.slots.lock().contains_key(&("phone".to_string(), "s1".to_string())), "expired slots are dropped");
    }

    #[test]
    fn the_resume_cache_has_limits_and_drops_the_least_recently_used() {
        let views = Views::default();
        // A device keeps twenty sessions; the twenty-first pushes out the one used longest ago.
        for i in 0..=RESUME_SESSIONS_PER_DEVICE {
            views.open("phone", &format!("s{i}"), i as u64, None);
            views.leave("phone", i as u64);
        }
        let held = |views: &Views, d: &str| views.slots.lock().keys().filter(|k| k.0 == d).count();
        assert_eq!(held(&views, "phone"), RESUME_SESSIONS_PER_DEVICE);
        assert!(!views.slots.lock().contains_key(&("phone".to_string(), "s0".to_string())));
        // Eight devices; a ninth takes the place of the one used longest ago.
        for d in 0..=RESUME_DEVICES {
            views.open(&format!("device{d}"), "s", 100 + d as u64, None);
            views.leave(&format!("device{d}"), 100 + d as u64);
        }
        let devices: std::collections::HashSet<String> = views.slots.lock().keys().map(|k| k.0.clone()).collect();
        assert_eq!(devices.len(), RESUME_DEVICES, "{devices:?}");
        assert!(devices.contains(&format!("device{RESUME_DEVICES}")));
        // What a phone that is connected follows is never dropped for the cache's sake.
        views.open("live", "s", 500, None);
        for d in 0..20 {
            views.open(&format!("other{d}"), "s", 600 + d, None);
            views.leave(&format!("other{d}"), 600 + d);
        }
        assert!(views.slots.lock().contains_key(&("live".to_string(), "s".to_string())));
        // Short of memory, everything nobody follows goes; busy for other reasons, it stays.
        let before = views.slots.lock().len();
        assert!(before > 1);
        views.load.set_reasons_for_test(Level::L3, &["cpu"]);
        views.sweep();
        assert_eq!(views.slots.lock().len(), before, "L3 for CPU or lag frees almost nothing: kept");
        views.load.set_reasons_for_test(Level::L3, &["memory"]);
        views.sweep();
        assert_eq!(views.slots.lock().len(), 1);
    }

    #[test]
    fn prompt_wait_and_title_changes_are_frames_too() {
        let views = Views::default();
        views.open("p", "s", 1, None);
        frame(&views, "p", "s", 1, &["a"]).unwrap();
        views.ack("p", "s", 1, 1);
        let mut snap = snapshot(&["a"]);
        snap.waiting_since = Some(5);
        let frame = json_of(views.frame("p", "s", 1, &snap, MAX_HEIGHT, Level::L0)).unwrap();
        assert_eq!((frame["rows"].clone(), frame["waiting_since"].clone()), (json!([]), json!(5)));
    }

    fn session() -> Arc<Session> {
        Session::new(1, SessionSpec { id: "s1".into(), kind: "shell".into(), cwd: ".".into(), ..Default::default() }, 80, 24)
    }

    fn follow(views: &Arc<Views>, device: &str, conn: u64, s: &Arc<Session>, height: usize, out: mpsc::Sender<String>) -> tokio::task::JoinHandle<()> {
        let acked = views.open(device, "s1", conn, None);
        tokio::spawn(run(Follow { views: views.clone(), device: device.into(), conn, id: "s1".into(), height, session: Arc::downgrade(s), out, focused: Arc::new(AtomicBool::new(true)), acked }))
    }

    /// The frames that arrive in `ms`, each confirmed as a phone does.
    async fn frames_in(views: &Views, who: (&str, u64), rx: &mut mpsc::Receiver<String>, ms: u64) -> Vec<Value> {
        let mut got = Vec::new();
        let end = tokio::time::Instant::now() + Duration::from_millis(ms);
        while let Ok(Some(f)) = tokio::time::timeout_at(end, rx.recv()).await {
            let frame: Value = serde_json::from_str(&f).unwrap();
            views.ack(who.0, "s1", who.1, frame["rev"].as_u64().unwrap());
            got.push(frame);
        }
        got
    }

    #[tokio::test]
    async fn a_session_printing_flat_out_costs_ten_frames_a_second() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out, mut rx) = mpsc::channel(8);
        let task = follow(&views, "phone", 1, &s, 50, out);
        // Output as often as the timer allows, for a second.
        let printer = {
            let s = s.clone();
            tokio::spawn(async move {
                let start = Instant::now();
                let mut i = 0;
                while start.elapsed() < Duration::from_secs(1) {
                    s.ingest(format!("line {i}\r\n").as_bytes());
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            })
        };
        let frames = frames_in(&views, ("phone", 1), &mut rx, 1100).await;
        printer.await.unwrap();
        assert!((5..=12).contains(&frames.len()), "{} frames in a second", frames.len());
        let last = frames.last().unwrap();
        assert!(last["rows"].as_array().unwrap().iter().any(|r| r[1].as_str().unwrap().starts_with("line ")), "{last}");
        task.abort();
    }

    #[tokio::test]
    async fn an_idle_session_sends_nothing_and_a_change_arrives_at_once() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out, mut rx) = mpsc::channel(8);
        let task = follow(&views, "phone", 1, &s, 50, out);
        assert_eq!(frames_in(&views, ("phone", 1), &mut rx, 400).await.len(), 1, "the first view, then nothing while nothing changes");
        let at = Instant::now();
        s.ingest(b"hello\r\n");
        let frame: Value = serde_json::from_str(&tokio::time::timeout(Duration::from_millis(500), rx.recv()).await.unwrap().unwrap()).unwrap();
        assert!(at.elapsed() < Duration::from_millis(150), "{:?}", at.elapsed());
        assert_eq!(frame["rows"], json!([[0, "hello"]]));
        task.abort();
    }

    #[tokio::test]
    async fn a_slow_socket_gets_the_latest_state_not_a_queue() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out, mut rx) = mpsc::channel(1);
        let task = follow(&views, "phone", 1, &s, 50, out);
        // Nobody reads for a while, and the session prints meanwhile.
        for i in 0..20 {
            s.ingest(format!("old {i}\r\n").as_bytes());
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        s.ingest(b"newest\r\n");
        let frames = frames_in(&views, ("phone", 1), &mut rx, 600).await;
        assert!(frames.len() <= 3, "{} frames queued", frames.len());
        let text = frames.iter().flat_map(|f| f["rows"].as_array().unwrap().clone()).map(|r| r[1].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert!(text.iter().any(|t| t == "newest"), "{text:?}");
        task.abort();
    }

    #[tokio::test]
    async fn a_phone_that_never_acknowledges_gets_four_frames_and_then_the_latest_state_once_it_does() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out, mut rx) = mpsc::channel(16);
        let task = follow(&views, "phone", 1, &s, 50, out);
        for i in 0..12 {
            s.ingest(format!("line {i}\r\n").as_bytes());
            tokio::time::sleep(Duration::from_millis(130)).await;
        }
        let mut got = Vec::new();
        while let Ok(f) = rx.try_recv() {
            got.push(serde_json::from_str::<Value>(&f).unwrap());
        }
        assert_eq!(got.len(), WINDOW_FRAMES, "{} frames with no acknowledgement", got.len());
        views.ack("phone", "s1", 1, got[1]["rev"].as_u64().unwrap());
        let next: Value = serde_json::from_str(&tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap()).unwrap();
        assert_eq!(next["base"], got[1]["rev"], "it follows what was confirmed");
        assert!(next["rows"].as_array().unwrap().iter().any(|r| r[1] == "line 11"), "the latest state: {next}");
        task.abort();
    }

    #[tokio::test]
    async fn phones_share_one_render_which_stops_with_the_last_of_them() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out1, mut rx1) = mpsc::channel(8);
        let (out2, mut rx2) = mpsc::channel(8);
        let first = follow(&views, "one", 1, &s, 50, out1);
        let second = follow(&views, "two", 2, &s, 10, out2);
        s.ingest(b"a\r\nb\r\nc\r\n");
        let (a, b) = (frames_in(&views, ("one", 1), &mut rx1, 400).await, frames_in(&views, ("two", 2), &mut rx2, 400).await);
        assert_eq!(views.feeds.lock().len(), 1, "one render for both");
        assert_eq!((a.last().unwrap()["height"].clone(), b.last().unwrap()["height"].clone()), (json!(3), json!(3)));
        first.abort();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(views.feeds.lock().len(), 1, "still followed");
        second.abort();
        // The render ends when its last follower is gone.
        let ended = ps_wait(|| views.feeds.lock().is_empty()).await;
        assert!(ended, "the render outlived its followers");
    }

    async fn ps_wait(mut done: impl FnMut() -> bool) -> bool {
        for _ in 0..40 {
            if done() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_busy_machine_gets_fewer_frames_and_the_sessions_in_the_background_least() {
        let s = session();
        let load = Load::new();
        let views = Arc::new(Views::new(load.clone()));
        let printer = |s: Arc<Session>| {
            tokio::spawn(async move {
                let start = Instant::now();
                let mut i = 0;
                while start.elapsed() < Duration::from_millis(1700) {
                    s.ingest(format!("line {i}\r\n").as_bytes());
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };
        let count = |level: Level, focused: bool| {
            let (s, views, load) = (s.clone(), views.clone(), load.clone());
            async move {
                load.set_for_test(level);
                let (out, mut rx) = mpsc::channel(8);
                let acked = views.open("phone", "s1", 7, None);
                let flag = Arc::new(AtomicBool::new(focused));
                let task = tokio::spawn(run(Follow { views: views.clone(), device: "phone".into(), conn: 7, id: "s1".into(), height: 50, session: Arc::downgrade(&s), out, focused: flag, acked }));
                let feeder = printer(s.clone());
                let frames = frames_in(&views, ("phone", 7), &mut rx, 1500).await.len();
                feeder.await.unwrap();
                task.abort();
                views.close("phone", "s1", 7);
                frames
            }
        };
        let calm = count(Level::L0, true).await;
        let loaded = count(Level::L2, true).await;
        let loaded_background = count(Level::L2, false).await;
        let overloaded_background = count(Level::L3, false).await;
        assert!(calm >= 6, "{calm} frames in 1.5 s when calm");
        assert!((4..=7).contains(&loaded), "{loaded} frames at L2 (250 ms apart)");
        assert!((1..=3).contains(&loaded_background), "{loaded_background} frames of a background session at L2 (1 s apart)");
        assert_eq!(overloaded_background, 0, "background sessions pause at L3");
    }

    #[tokio::test]
    async fn typing_into_a_session_gets_its_echo_at_the_calm_rate_even_when_the_machine_is_overloaded() {
        let s = session();
        let load = Load::new();
        load.set_for_test(Level::L3);
        let views = Arc::new(Views::new(load));
        let (out, mut rx) = mpsc::channel(8);
        let task = follow(&views, "phone", 1, &s, 50, out);
        assert_eq!(frames_in(&views, ("phone", 1), &mut rx, 200).await.len(), 1, "the first view");
        // Output now waits for the L3 gap (a second)…
        s.ingest(b"quiet\r\n");
        assert!(frames_in(&views, ("phone", 1), &mut rx, 300).await.is_empty());
        // …unless somebody types: the echo of a key is what they wait for.
        views.boost("s1");
        let at = Instant::now();
        s.ingest(b"echo\r\n");
        let frames = frames_in(&views, ("phone", 1), &mut rx, 400).await;
        assert!(!frames.is_empty() && at.elapsed() < Duration::from_millis(600), "{} frames", frames.len());
        let text: String = frames.iter().flat_map(|f| f["rows"].as_array().unwrap().iter().map(|r| r[1].as_str().unwrap().to_string())).collect();
        assert!(text.contains("echo"), "{text}");
        task.abort();
    }

    /// How fast three sessions take output in, flat out, for `seconds`, in MB/s: with `phones`
    /// phones following all of them (frames serialized and dropped as a socket would take them).
    async fn ingest_rate(phones: usize, seconds: f64) -> f64 {
        let sessions: Vec<Arc<Session>> = (0..3).map(|i| Session::new(i, SessionSpec { id: format!("s{i}"), kind: "shell".into(), cwd: ".".into(), ..Default::default() }, 120, 30)).collect();
        let views = Arc::new(Views::default());
        let mut tasks = Vec::new();
        for p in 0..phones {
            for (i, s) in sessions.iter().enumerate() {
                let (out, mut rx) = mpsc::channel::<String>(8);
                let acked = views.open(&format!("p{p}"), &format!("s{i}"), p as u64, None);
                tasks.push(tokio::spawn(run(Follow { views: views.clone(), device: format!("p{p}"), conn: p as u64, id: format!("s{i}"), height: 200, session: Arc::downgrade(s), out, focused: Arc::new(AtomicBool::new(true)), acked })));
                let (views, device, id, conn) = (views.clone(), format!("p{p}"), format!("s{i}"), p as u64);
                tasks.push(tokio::spawn(async move {
                    while let Some(f) = rx.recv().await {
                        if let Some(rev) = serde_json::from_str::<Value>(&f).ok().and_then(|f| f["rev"].as_u64()) {
                            views.ack(&device, &id, conn, rev);
                        }
                    }
                }));
            }
        }
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let total = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let feeders: Vec<_> = sessions
            .iter()
            .map(|s| {
                let (s, stop, total) = (s.clone(), stop.clone(), total.clone());
                std::thread::spawn(move || {
                    // 64 KiB reads of coloured lines, as the PTY reader thread gets them.
                    let mut chunk = String::new();
                    let mut n = 0u64;
                    while chunk.len() < 64 * 1024 {
                        n += 1;
                        chunk.push_str(&format!("\x1b[32mline {n:08}\x1b[0m {}\r\n", "0123456789abcdef".repeat(4)));
                    }
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        s.ingest(chunk.as_bytes());
                        total.fetch_add(chunk.len() as u64, std::sync::atomic::Ordering::Relaxed);
                    }
                })
            })
            .collect();
        tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for f in feeders {
            f.join().unwrap();
        }
        for t in tasks {
            t.abort();
        }
        total.load(std::sync::atomic::Ordering::Relaxed) as f64 / seconds / 1e6
    }

    /// The phones' effect on how fast output is taken in. Meaningful in a release build only:
    /// `cargo test --release -p kelpied --bin kelpied -- --ignored ingest_speed --nocapture`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn ingest_speed_with_eight_phones() {
        let (mut without, mut with) = (Vec::new(), Vec::new());
        // Alternating, and many: a run varies by ten percent on its own.
        for _ in 0..7 {
            without.push(ingest_rate(0, 2.5).await);
            with.push(ingest_rate(8, 2.5).await);
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let (a, b) = (median(&mut without), median(&mut with));
        println!("ingest, three sessions flat out: {a:.1} MB/s without phones ({without:.1?}), {b:.1} MB/s with eight ({with:.1?}), {:+.1} %", (b / a - 1.0) * 100.0);
        // The target is 5 %, but one invocation varies by about 10 % on its own (ten invocations are
        // needed to see it): this catches a real slowdown, not noise.
        assert!(b >= a * 0.85, "ingest slowed from {a:.1} to {b:.1} MB/s");
    }

    #[tokio::test]
    async fn a_closed_session_ends_its_tasks() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out, mut rx) = mpsc::channel(8);
        let task = follow(&views, "phone", 1, &s, 50, out);
        frames_in(&views, ("phone", 1), &mut rx, 200).await;
        drop(s);
        tokio::time::timeout(Duration::from_secs(2), task).await.expect("the task ends with the session").unwrap();
        assert!(views.slots.lock().is_empty());
        assert!(ps_wait(|| views.feeds.lock().is_empty()).await);
    }

    #[tokio::test]
    async fn a_session_that_redraws_everything_is_followed_at_five_frames_a_second() {
        let s = session();
        let views = Arc::new(Views::default());
        let (out, mut rx) = mpsc::channel(64);
        let task = follow(&views, "phone", 1, &s, 200, out);
        let printer = {
            let s = s.clone();
            tokio::spawn(async move {
                let start = Instant::now();
                let mut i = 0u64;
                // A screenful of fresh long lines each time: every frame is heavy.
                while start.elapsed() < Duration::from_secs(2) {
                    for n in 0..250 {
                        s.ingest(format!("{i:06} {n:03} {}\r\n", "abcdef0123456789".repeat(5)).as_bytes());
                    }
                    i += 1;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
        };
        let frames = frames_in(&views, ("phone", 1), &mut rx, 2200).await;
        printer.await.unwrap();
        let heavy = frames.iter().filter(|f| f.to_string().len() > 8 * 1024).count();
        assert!(heavy > 3, "{heavy} heavy frames");
        assert!(frames.len() <= 14, "{} frames in two seconds", frames.len());
        task.abort();
    }
}
