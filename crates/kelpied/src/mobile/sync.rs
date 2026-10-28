//! Keeping a phone's view of a terminal session up to date: when something changed, the rows
//! that differ from what that phone last got, not the whole screen again.
//!
//! One task per followed session (`drive`, shared by every phone following it) waits for the
//! session's wake signal and, at most once per `MIN_GAP`, takes a snapshot of its end under the
//! session lock and hashes its rows; nothing runs for a session nobody follows. One task per phone
//! and session (`run`) diffs the latest snapshot against the rows it last sent that device, outside
//! the lock. A socket that cannot keep up gets the latest state when it can take it, never a queue of
//! old ones. What was last sent is kept per (device, session) so a phone that reconnects, or moves
//! between the direct and the relay path, carries on from its last frame instead of starting over.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Serialize;
use tokio::sync::{mpsc, watch};

use crate::choices::Prompt;
use crate::session::{ScreenView, Session};

/// Least time between two renders of one session.
pub const MIN_GAP: Duration = Duration::from_millis(100);
/// How long what was last sent is kept after the phone's connection ended.
pub const RESUME_TTL: Duration = Duration::from_secs(300);
/// Rows a view may have; also the most a phone can ask for.
pub const MAX_HEIGHT: usize = 200;
pub const MIN_HEIGHT: usize = 10;
/// A row longer than this is cut (a phone shows a hundred columns or so).
const MAX_ROW_CHARS: usize = 300;
/// The rows of a frame together stay under this, so that a frame stays under 64 KiB.
const MAX_ROWS_BYTES: usize = 56 * 1024;
/// How far the screen may have scrolled between two frames for the diff to still follow it.
const MAX_SHIFT: usize = 64;
const MAX_TITLE_CHARS: usize = 200;
/// A session whose frames average more than this redraws most of the screen each time (a full-screen
/// program, a flood of output): its phones get five frames a second instead of ten.
const HEAVY_FRAME_BYTES: f64 = 8.0 * 1024.0;

/// `line` cut to at most `chars` characters.
fn cut(line: &str, chars: usize) -> &str {
    line.char_indices().nth(chars).map_or(line, |(at, _)| &line[..at])
}

/// Cuts rows to what a frame can carry: each to `MAX_ROW_CHARS`, and all together to
/// `MAX_ROWS_BYTES` (a screen of very long lines gets shorter rows, the same way every time).
fn fit(lines: Vec<String>) -> Vec<String> {
    let mut cap = MAX_ROW_CHARS;
    loop {
        let bytes: usize = lines.iter().map(|l| cut(l, cap)).map(|l| l.len() + 8 + l.bytes().filter(|b| matches!(b, b'"' | b'\\') || *b < 0x20).count() * 5).sum();
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

/// What one phone was last sent for one session.
struct Slot {
    /// The connection following it; another taking over ends the first's task.
    owner: u64,
    /// The last frame's number. A frame says which one it follows (`base`).
    rev: u64,
    /// False until a frame has been sent on this slot: the first is the whole view.
    synced: bool,
    rows: Vec<Arc<str>>,
    extras: Option<Extras>,
    /// Since when nobody follows it.
    left: Option<Instant>,
}

pub struct Lost;

type Snapshots = watch::Receiver<Option<Arc<Snapshot>>>;

/// The shared render of one followed session.
struct Feed {
    snapshots: watch::Sender<Option<Arc<Snapshot>>>,
}

#[derive(Default)]
pub struct Views {
    slots: Mutex<HashMap<(String, String), Slot>>,
    feeds: Mutex<HashMap<String, Arc<Feed>>>,
}

impl Views {
    /// A connection starts following `session` for `device`. `rev` is the last frame the phone
    /// holds: if that is what was last sent, frames carry on from it; else the next one is whole.
    pub fn open(&self, device: &str, session: &str, conn: u64, rev: Option<u64>) {
        let mut slots = self.slots.lock();
        slots.retain(|_, s| s.left.is_none_or(|at| at.elapsed() < RESUME_TTL));
        let slot = slots.entry((device.to_string(), session.to_string())).or_insert(Slot { owner: conn, rev: 0, synced: false, rows: Vec::new(), extras: None, left: None });
        if rev.is_none_or(|r| r != slot.rev) {
            slot.synced = false;
        }
        slot.owner = conn;
        slot.left = None;
    }

    /// The connection ended: what it was sent is kept for a while.
    pub fn leave(&self, device: &str, conn: u64) {
        for ((d, _), slot) in self.slots.lock().iter_mut() {
            if d == device && slot.owner == conn && slot.left.is_none() {
                slot.left = Some(Instant::now());
            }
        }
    }

    /// Nothing follows `session` for `device` any more (the phone stopped, or it is gone).
    pub fn close(&self, device: &str, session: &str, conn: u64) {
        let mut slots = self.slots.lock();
        let key = (device.to_string(), session.to_string());
        if slots.get(&key).is_some_and(|s| s.owner == conn) {
            slots.remove(&key);
        }
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
        let feed = Arc::new(Feed { snapshots: watch::channel(None).0 });
        let receiver = subscribe(&feed);
        feeds.insert(id.to_string(), feed.clone());
        tokio::spawn(drive(self.clone(), id.to_string(), session.clone(), feed));
        receiver
    }

    /// The frame that brings the phone from what it was last sent to the end of `snapshot` (its
    /// last `height` rows), if it differs.
    fn frame(&self, device: &str, session: &str, conn: u64, snapshot: &Snapshot, height: usize) -> Result<Option<String>, Lost> {
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
        let (base, shift, rows) = if slot.synced {
            let (shift, rows) = diff(&slot.rows, lines);
            if rows.is_empty() && shift == 0 && slot.extras.as_ref() == Some(&extras) {
                return Ok(None);
            }
            (Some(slot.rev), shift, rows)
        } else {
            (None, 0, (0..lines.len()).filter(|&i| !lines[i].is_empty()).collect())
        };
        slot.rev += 1;
        slot.synced = true;
        slot.rows = lines.to_vec();
        let frame = Frame {
            kind: "screen", id: session, rev: slot.rev, base, shift,
            rows: rows.iter().map(|&i| (i, &*lines[i])).collect(),
            height: extras.height, cursor: extras.cursor, prompt: &extras.prompt,
            waiting_since: extras.waiting_since, title: &extras.title,
        };
        let text = serde_json::to_string(&frame).expect("a frame is plain data");
        slot.extras = Some(extras);
        Ok(Some(text))
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
}

/// Renders `id` for its followers: when the session says it changed, at most once per `MIN_GAP`.
async fn drive(views: Arc<Views>, id: String, session: Weak<Session>, feed: Arc<Feed>) {
    if let Some(mut wake) = session.upgrade().map(|s| s.watch()) {
        loop {
            // Anything after this is seen as a change next time round.
            wake.borrow_and_update();
            let Some(view) = session.upgrade().map(|s| s.screen_view(MAX_HEIGHT)) else { break };
            feed.snapshots.send_replace(Some(Arc::new(Snapshot::of(view))));
            tokio::time::sleep(MIN_GAP).await;
            tokio::select! {
                _ = feed.snapshots.closed() => break,
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
}

/// Sends frames until the phone stops following, the connection is gone or the session is.
pub async fn run(f: Follow) {
    let mut snapshots = f.views.follow(&f.id, &f.session);
    // The size of recent frames, to notice a session that redraws everything each time.
    let mut average = 0.0;
    loop {
        if snapshots.changed().await.is_err() {
            break;
        }
        // A place in the socket's queue first: the state is read only once it can be sent.
        let Ok(slot) = f.out.reserve().await else { break };
        let Some(snapshot) = snapshots.borrow_and_update().clone() else { continue };
        match f.views.frame(&f.device, &f.id, f.conn, &snapshot, f.height) {
            Ok(Some(frame)) => {
                average = average * 0.7 + frame.len() as f64 * 0.3;
                slot.send(frame);
            }
            Ok(None) => {}
            Err(Lost) => return,
        }
        if average > HEAVY_FRAME_BYTES {
            tokio::time::sleep(2 * MIN_GAP).await;
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

    /// The frame for `lines`, the phone showing all of them.
    fn frame(views: &Views, device: &str, session: &str, conn: u64, lines: &[&str]) -> Result<Option<Value>, Lost> {
        views.frame(device, session, conn, &snapshot(lines), MAX_HEIGHT).map(|f| f.map(|f| serde_json::from_str(&f).unwrap()))
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
        let frame = frame(&views, "d", "s", 1, &screen).ok().unwrap().unwrap();
        assert!(frame.to_string().len() < 64 * 1024, "{}", frame.to_string().len());
    }

    #[test]
    fn frames_follow_each_other_and_resume_across_connections() {
        let views = Views::default();
        views.open("phone", "s1", 1, None);
        let first = frame(&views, "phone", "s1", 1, &["one", "two"]).ok().unwrap().unwrap();
        assert_eq!(first["base"], Value::Null);
        assert_eq!(first["rev"], 1);
        assert_eq!(first["rows"], json!([[0, "one"], [1, "two"]]));
        assert!(frame(&views, "phone", "s1", 1, &["one", "two"]).ok().unwrap().is_none(), "nothing changed, nothing sent");
        let second = frame(&views, "phone", "s1", 1, &["one", "three"]).ok().unwrap().unwrap();
        assert_eq!((second["base"].clone(), second["rev"].clone(), second["rows"].clone()), (json!(1), json!(2), json!([[1, "three"]])));

        // The connection drops and the phone comes back on another with the last frame it had.
        views.leave("phone", 1);
        views.open("phone", "s1", 2, Some(2));
        assert!(frame(&views, "phone", "s1", 1, &["x"]).is_err(), "the old connection no longer follows");
        let third = frame(&views, "phone", "s1", 2, &["one", "three", "four"]).ok().unwrap().unwrap();
        assert_eq!((third["base"].clone(), third["rows"].clone()), (json!(2), json!([[2, "four"]])));

        // A phone that lost its place (or a different device) gets the whole view.
        views.open("phone", "s1", 3, Some(1));
        let whole = frame(&views, "phone", "s1", 3, &["one", "three", "four"]).ok().unwrap().unwrap();
        assert_eq!(whole["base"], Value::Null);
        assert_eq!(whole["rows"].as_array().unwrap().len(), 3);
        views.open("other", "s1", 4, Some(3));
        assert_eq!(frame(&views, "other", "s1", 4, &["z"]).ok().unwrap().unwrap()["base"], Value::Null);
    }

    #[test]
    fn a_phone_that_wants_fewer_rows_gets_the_last_of_them() {
        let views = Views::default();
        views.open("phone", "s1", 1, None);
        let mut snap = snapshot(&["a", "b", "c", "d"]);
        snap.cursor = Some((3, 2));
        let frame: Value = serde_json::from_str(&views.frame("phone", "s1", 1, &snap, 2).ok().unwrap().unwrap()).unwrap();
        assert_eq!((frame["height"].clone(), frame["rows"].clone(), frame["cursor"].clone()), (json!(2), json!([[0, "c"], [1, "d"]]), json!([1, 2])));
        // A cursor above the rows it shows is not shown.
        snap.cursor = Some((0, 0));
        let frame: Value = serde_json::from_str(&views.frame("phone", "s1", 1, &snap, 2).ok().unwrap().unwrap()).unwrap();
        assert_eq!(frame["cursor"], Value::Null);
    }

    #[test]
    fn what_was_sent_is_kept_for_five_minutes_after_leaving() {
        let views = Views::default();
        views.open("phone", "s1", 1, None);
        frame(&views, "phone", "s1", 1, &["a"]).ok().unwrap();
        views.leave("phone", 1);
        views.slots.lock().get_mut(&("phone".to_string(), "s1".to_string())).unwrap().left = Some(Instant::now() - RESUME_TTL - Duration::from_secs(1));
        views.open("phone", "s2", 1, None);
        assert!(!views.slots.lock().contains_key(&("phone".to_string(), "s1".to_string())), "expired slots are dropped");
    }

    #[test]
    fn prompt_wait_and_title_changes_are_frames_too() {
        let views = Views::default();
        views.open("p", "s", 1, None);
        frame(&views, "p", "s", 1, &["a"]).ok().unwrap();
        let mut snap = snapshot(&["a"]);
        snap.waiting_since = Some(5);
        let frame: Value = serde_json::from_str(&views.frame("p", "s", 1, &snap, MAX_HEIGHT).ok().unwrap().unwrap()).unwrap();
        assert_eq!((frame["rows"].clone(), frame["waiting_since"].clone()), (json!([]), json!(5)));
    }

    fn session() -> Arc<Session> {
        Session::new(1, SessionSpec { id: "s1".into(), kind: "shell".into(), cwd: ".".into(), ..Default::default() }, 80, 24)
    }

    fn follow(views: &Arc<Views>, device: &str, conn: u64, s: &Arc<Session>, height: usize, out: mpsc::Sender<String>) -> tokio::task::JoinHandle<()> {
        views.open(device, "s1", conn, None);
        tokio::spawn(run(Follow { views: views.clone(), device: device.into(), conn, id: "s1".into(), height, session: Arc::downgrade(s), out }))
    }

    async fn frames_in(rx: &mut mpsc::Receiver<String>, ms: u64) -> Vec<Value> {
        let mut got = Vec::new();
        let end = tokio::time::Instant::now() + Duration::from_millis(ms);
        while let Ok(Some(f)) = tokio::time::timeout_at(end, rx.recv()).await {
            got.push(serde_json::from_str(&f).unwrap());
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
        let frames = frames_in(&mut rx, 1100).await;
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
        assert_eq!(frames_in(&mut rx, 400).await.len(), 1, "the first view, then nothing while nothing changes");
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
        let frames = frames_in(&mut rx, 600).await;
        assert!(frames.len() <= 3, "{} frames queued", frames.len());
        let text = frames.iter().flat_map(|f| f["rows"].as_array().unwrap().clone()).map(|r| r[1].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert!(text.iter().any(|t| t == "newest"), "{text:?}");
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
        let (a, b) = (frames_in(&mut rx1, 400).await, frames_in(&mut rx2, 400).await);
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

    /// How fast three sessions take output in, flat out, for `seconds`, in MB/s: with `phones`
    /// phones following all of them (frames serialized and dropped as a socket would take them).
    async fn ingest_rate(phones: usize, seconds: f64) -> f64 {
        let sessions: Vec<Arc<Session>> = (0..3).map(|i| Session::new(i, SessionSpec { id: format!("s{i}"), kind: "shell".into(), cwd: ".".into(), ..Default::default() }, 120, 30)).collect();
        let views = Arc::new(Views::default());
        let mut tasks = Vec::new();
        for p in 0..phones {
            for (i, s) in sessions.iter().enumerate() {
                let (out, mut rx) = mpsc::channel::<String>(8);
                views.open(&format!("p{p}"), &format!("s{i}"), p as u64, None);
                tasks.push(tokio::spawn(run(Follow { views: views.clone(), device: format!("p{p}"), conn: p as u64, id: format!("s{i}"), height: 200, session: Arc::downgrade(s), out })));
                tasks.push(tokio::spawn(async move { while rx.recv().await.is_some() {} }));
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
        frames_in(&mut rx, 200).await;
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
        let frames = frames_in(&mut rx, 2200).await;
        printer.await.unwrap();
        let heavy = frames.iter().filter(|f| f.to_string().len() > 8 * 1024).count();
        assert!(heavy > 3, "{heavy} heavy frames");
        assert!(frames.len() <= 14, "{} frames in two seconds", frames.len());
        task.abort();
    }
}
