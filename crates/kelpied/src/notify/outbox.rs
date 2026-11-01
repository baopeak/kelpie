//! What one destination still has to send, kept on disk: a computer that restarts or a service that is down for a while loses
//! nothing and repeats nothing.
//!
//! Before every attempt each entry is looked at again, so what the person already dealt with is never sent. A failure waits 1, 2,
//! 4, 8, 16, 32, then 60 s, with full jitter so phones and channels do not retry together, and at least what the service's
//! `Retry-After` asks. An entry older than ten minutes goes, unless it says someone is still waiting: that is still true, and is
//! told once the service answers. Several entries that piled up during an outage leave as one message.
//!
//! The file is written atomically, holds at most [`MAX_ENTRIES`], and holds only what is already safe to send: a phone's entries
//! carry no text at all. A file that cannot be used is moved aside, not overwritten; one that cannot even be read keeps this
//! outbox in memory, and nothing is written over it.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Kind, Payload};
use crate::persist::write_atomic;

pub const MAX_ENTRIES: usize = 50;
pub const MAX_AGE_MS: u64 = 10 * 60 * 1000;
const VERSION: u32 = 1;
const BACKOFF_S: [u64; 7] = [1, 2, 4, 8, 16, 32, 60];
/// However the dice fall: a failed attempt is never followed at once by the next.
const LEAST_WAIT_MS: u64 = 100;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: String,
    /// `Kind::code`.
    pub kind: String,
    /// The session, by id. The id is opaque; a phone's message uses it for its link and nothing else.
    #[serde(default)]
    pub session: Option<String>,
    /// Which wait of the session this is about: another wait is another alert.
    #[serde(default)]
    pub wait: Option<u64>,
    /// The run and task whose approval this is about.
    #[serde(default)]
    pub task: Option<(String, String)>,
    /// When it happened (ms).
    pub at: u64,
    /// Not before this time (ms), when the machine is busy and it can wait; 0 when it was not held back.
    #[serde(default)]
    pub hold_until: u64,
    /// The message for a channel. A phone's message is built when it is sent and has no text to keep.
    #[serde(default)]
    pub payload: Option<Payload>,
}

impl Entry {
    fn kind(&self) -> Option<Kind> {
        Kind::parse(&self.kind)
    }

    /// Someone is, or was when last looked, waiting on the person: this outlasts [`MAX_AGE_MS`].
    fn waits(&self) -> bool {
        self.kind().is_some_and(Kind::waits)
    }

    fn urgent(&self) -> bool {
        self.kind().is_some_and(Kind::urgent)
    }

    fn about(&self) -> &str {
        self.session.as_deref().unwrap_or(&self.id)
    }
}

/// What one attempt is to send.
#[derive(Clone, Debug, PartialEq)]
pub enum Attempt {
    One(Box<Entry>),
    /// Several as one: `sessions` different things are behind `ids`.
    Together { ids: Vec<String>, sessions: usize, needs_you: bool, latest: u64 },
}

#[derive(Debug, PartialEq)]
pub enum Pick {
    Idle,
    Wait(Duration),
    Go(Attempt),
}

#[derive(Serialize, Deserialize, Default)]
struct OnDisk {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    failures: u32,
    #[serde(default)]
    entries: Vec<Entry>,
}

pub struct Outbox {
    path: PathBuf,
    /// False when the file could not be read: nothing is written over what may still be there.
    durable: bool,
    entries: Vec<Entry>,
    failures: u32,
    next_at: u64,
}

impl Outbox {
    pub fn open(path: &Path) -> Outbox {
        let mut out = Outbox { path: path.to_path_buf(), durable: true, entries: Vec::new(), failures: 0, next_at: 0 };
        match std::fs::read(path) {
            Ok(bytes) => match serde_json::from_slice::<OnDisk>(&bytes) {
                Ok(d) if d.version == VERSION => {
                    out.entries = d.entries;
                    out.entries.truncate(MAX_ENTRIES);
                    out.failures = d.failures;
                }
                _ => {
                    let aside = path.with_extension(format!("unreadable-{}", crate::session::now_millis()));
                    tracing::warn!("notify outbox {:?} cannot be used; moving it aside", path.file_name());
                    if std::fs::rename(path, &aside).is_err() {
                        out.durable = false;
                    }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!("notify outbox {:?} cannot be read: {e}; what is sent now is kept in memory only", path.file_name());
                out.durable = false;
            }
        }
        out
    }

    fn save(&self) {
        if !self.durable {
            return;
        }
        if self.entries.is_empty() {
            let _ = std::fs::remove_file(&self.path);
            return;
        }
        let disk = OnDisk { version: VERSION, failures: self.failures, entries: self.entries.clone() };
        let written = serde_json::to_vec(&disk).map_err(anyhow::Error::from).and_then(|bytes| write_atomic(&self.path, &bytes));
        if let Err(e) = written {
            // No path in the text: the name of a phone's outbox is its device's.
            tracing::warn!("notify outbox could not be written: {:#}", e.root_cause());
        }
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Takes an entry. Entries held back for the machine's load share one window, so that they leave together. A full outbox loses
    /// the oldest thing that waits for nobody, else the oldest; the count is what was lost.
    pub fn add(&mut self, mut entry: Entry) -> u64 {
        if entry.hold_until > 0 {
            if let Some(first) = self.entries.iter().filter(|e| e.hold_until > 0).map(|e| e.hold_until).min() {
                entry.hold_until = first.max(entry.at);
            }
        }
        let mut lost = 0;
        if self.entries.len() >= MAX_ENTRIES {
            let victim = self.entries.iter().position(|e| !e.urgent()).unwrap_or(0);
            self.entries.remove(victim);
            lost = 1;
        }
        self.entries.push(entry);
        self.save();
        lost
    }

    /// An attempt may be made now.
    pub fn due(&self, now: u64) -> bool {
        !self.entries.is_empty() && now >= self.next_at
    }

    /// The look before an attempt: what is settled goes, and what is too old unless someone still waits. The count is what went
    /// unsaid because it was too old (what was settled needed no saying).
    pub fn prune(&mut self, now: u64, still_so: &dyn Fn(&Entry) -> bool) -> u64 {
        let before = self.entries.len();
        let mut lost = 0;
        self.entries.retain(|e| {
            if !still_so(e) {
                return false;
            }
            let keep = e.waits() || now.saturating_sub(e.at) <= MAX_AGE_MS;
            if !keep {
                lost += 1;
            }
            keep
        });
        if self.entries.len() != before {
            if self.entries.is_empty() {
                self.failures = 0;
                self.next_at = 0;
            }
            self.save();
        }
        lost
    }

    pub fn pick(&self, now: u64) -> Pick {
        if self.entries.is_empty() {
            return Pick::Idle;
        }
        if now < self.next_at {
            return Pick::Wait(Duration::from_millis(self.next_at - now));
        }
        let ready: Vec<&Entry> = self.entries.iter().filter(|e| e.hold_until <= now).collect();
        if ready.is_empty() {
            let first = self.entries.iter().map(|e| e.hold_until).min().unwrap_or(now);
            return Pick::Wait(Duration::from_millis(first.saturating_sub(now).max(1)));
        }
        // After a failure everything that piled up is one message. Held back for the load: the same, for what can wait.
        let held: Vec<&Entry> = ready.iter().copied().filter(|e| e.hold_until > 0).collect();
        let together = if self.failures > 0 && ready.len() > 1 {
            ready.clone()
        } else if held.len() > 1 {
            held
        } else {
            Vec::new()
        };
        if !together.is_empty() {
            let sessions = together.iter().map(|e| e.about()).collect::<HashSet<_>>().len();
            return Pick::Go(Attempt::Together {
                ids: together.iter().map(|e| e.id.clone()).collect(),
                sessions,
                needs_you: together.iter().any(|e| e.waits()),
                latest: together.iter().map(|e| e.at).max().unwrap_or(now),
            });
        }
        let first = ready.iter().find(|e| e.urgent()).or(ready.first()).expect("not empty");
        Pick::Go(Attempt::One(Box::new((*first).clone())))
    }

    fn forget(&mut self, attempt: &Attempt) {
        match attempt {
            Attempt::One(e) => self.entries.retain(|x| x.id != e.id),
            Attempt::Together { ids, .. } => self.entries.retain(|x| !ids.contains(&x.id)),
        }
    }

    /// It went out.
    pub fn sent(&mut self, attempt: &Attempt) {
        self.forget(attempt);
        self.failures = 0;
        self.next_at = 0;
        self.save();
    }

    /// The service refused it for good: trying again cannot help. The count is what was lost.
    pub fn refused(&mut self, attempt: &Attempt) -> u64 {
        let before = self.entries.len();
        self.forget(attempt);
        self.save();
        (before - self.entries.len()) as u64
    }

    /// It did not go out and may later. `dice(n)` is a number below `n`.
    pub fn failed(&mut self, now: u64, retry_after: Option<Duration>, dice: &mut dyn FnMut(u64) -> u64) {
        let base = BACKOFF_S[(self.failures as usize).min(BACKOFF_S.len() - 1)] * 1000;
        let jittered = dice(base + 1).max(LEAST_WAIT_MS);
        let asked = retry_after.map_or(0, |d| d.as_millis().min(MAX_AGE_MS as u128) as u64);
        self.failures = self.failures.saturating_add(1);
        self.next_at = now + jittered.max(asked);
        self.save();
    }

    /// The destination is gone, and what it had to send with it.
    pub fn discard(&mut self) {
        self.entries.clear();
        if self.durable {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kelpie-outbox-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("o.json")
    }

    fn entry(id: &str, kind: Kind, at: u64) -> Entry {
        Entry { id: id.into(), kind: kind.code().into(), session: Some(format!("s-{id}")), wait: None, task: None, at, hold_until: 0, payload: None }
    }

    fn always(_: &Entry) -> bool {
        true
    }

    /// Top of the range every time: the longest it could wait.
    fn slowest(n: u64) -> u64 {
        n - 1
    }

    #[test]
    fn it_survives_a_restart_and_leaves_no_file_when_empty() {
        let path = scratch("restart");
        let mut o = Outbox::open(&path);
        o.add(entry("a", Kind::Waiting, 1));
        assert!(path.exists());
        let mut again = Outbox::open(&path);
        assert_eq!(again.entries.len(), 1);
        let Pick::Go(a) = again.pick(2) else { panic!("due at once") };
        again.sent(&a);
        assert!(!path.exists(), "nothing left, no file");
        assert!(Outbox::open(&path).is_empty());
    }

    #[test]
    fn a_file_that_cannot_be_used_is_moved_aside_never_written_over() {
        for (tag, body) in [("garbage", &b"{not json"[..]), ("newer", &br#"{"version":9,"entries":[]}"#[..])] {
            let path = scratch(tag);
            std::fs::write(&path, body).unwrap();
            let mut o = Outbox::open(&path);
            assert!(o.is_empty());
            o.add(entry("a", Kind::Waiting, 1));
            let aside: Vec<_> = std::fs::read_dir(path.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("unreadable-")).collect();
            assert_eq!(aside.len(), 1, "{tag}");
            assert_eq!(std::fs::read(aside[0].path()).unwrap(), body, "{tag}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_not_written_over() {
        // A folder where the file should be: reading it fails with something other than "not there".
        let path = scratch("unreadable");
        std::fs::create_dir(&path).unwrap();
        let mut o = Outbox::open(&path);
        assert!(!o.durable);
        o.add(entry("a", Kind::Waiting, 1));
        assert!(path.is_dir());
        assert_eq!(o.entries.len(), 1, "it still goes out");
        o.discard();
        assert!(path.is_dir());
    }

    #[test]
    fn it_holds_fifty_and_loses_what_waits_for_nobody_first() {
        let path = scratch("full");
        let mut o = Outbox::open(&path);
        o.add(entry("old-done", Kind::Finished, 1));
        for i in 0..(MAX_ENTRIES - 1) {
            assert_eq!(o.add(entry(&format!("w{i}"), Kind::Waiting, 2)), 0);
        }
        assert_eq!(o.add(entry("new", Kind::Waiting, 3)), 1);
        assert_eq!(o.entries.len(), MAX_ENTRIES);
        assert!(!o.entries.iter().any(|e| e.id == "old-done"));
        assert_eq!(o.add(entry("newer", Kind::Waiting, 3)), 1);
        assert!(!o.entries.iter().any(|e| e.id == "w0"), "with none of those, the oldest");
    }

    #[test]
    fn the_waits_follow_the_table_and_top_out() {
        let mut o = Outbox::open(&scratch("backoff"));
        o.add(entry("a", Kind::Waiting, 0));
        let mut now = 0;
        for want in [1000, 2000, 4000, 8000, 16000, 32000, 60000, 60000, 60000] {
            o.failed(now, None, &mut slowest);
            assert_eq!(o.next_at, now + want);
            assert!(!o.due(now + want - 1) && o.due(now + want));
            now = o.next_at;
        }
    }

    #[test]
    fn full_jitter_is_used_with_a_floor_and_retry_after_wins_when_longer() {
        let mut o = Outbox::open(&scratch("jitter"));
        o.add(entry("a", Kind::Waiting, 0));
        o.failed(0, None, &mut |_| 0);
        assert_eq!(o.next_at, LEAST_WAIT_MS, "never at once");
        o.failed(0, None, &mut |n| {
            assert_eq!(n, 2001, "the whole range below the second step");
            500
        });
        assert_eq!(o.next_at, 500);
        o.failed(0, Some(Duration::from_secs(30)), &mut slowest);
        assert_eq!(o.next_at, 30_000, "what the service asked for");
        o.failed(0, Some(Duration::from_secs(1)), &mut slowest);
        assert_eq!(o.next_at, 8_000, "not less than the step");
        o.failed(0, Some(Duration::from_secs(86_400)), &mut slowest);
        assert_eq!(o.next_at, MAX_AGE_MS, "and not past what an entry may live");
    }

    /// One send through a sink that answers 503 three times and then takes it: exactly one delivery, under a minute late.
    #[test]
    fn three_failures_then_success_is_one_delivery_within_a_minute() {
        let mut o = Outbox::open(&scratch("503"));
        o.add(entry("a", Kind::Waiting, 0));
        let (mut now, mut answers, mut delivered) = (0u64, [503, 503, 503, 200].into_iter(), Vec::new());
        while !o.is_empty() {
            now = now.max(o.next_at);
            assert!(o.due(now));
            o.prune(now, &always);
            let Pick::Go(a) = o.pick(now) else { panic!("due") };
            match answers.next().unwrap() {
                200 => {
                    delivered.push(a.clone());
                    o.sent(&a);
                }
                _ => o.failed(now, None, &mut slowest),
            }
        }
        assert_eq!(delivered.len(), 1);
        assert!(matches!(&delivered[0], Attempt::One(e) if e.id == "a"), "itself, with its link");
        assert!(now <= 60_000, "{now} ms late");
    }

    /// A sink down for twelve minutes, stepping through every wait. `settled` says when the person answered the session.
    fn twelve_minute_outage(entries: &[Entry], settled_at: Option<u64>) -> Vec<Attempt> {
        let mut o = Outbox::open(&scratch("outage"));
        for e in entries {
            o.add(e.clone());
        }
        let back = 12 * 60 * 1000;
        let mut now = 0u64;
        let mut delivered = Vec::new();
        for _ in 0..500 {
            if o.is_empty() {
                break;
            }
            now = now.max(o.next_at);
            if !o.due(now) {
                continue;
            }
            let settled = settled_at.is_some_and(|at| now >= at);
            o.prune(now, &|e| !(settled && e.waits()));
            let Pick::Go(a) = o.pick(now) else { continue };
            if now >= back {
                delivered.push(a.clone());
                o.sent(&a);
            } else {
                o.failed(now, None, &mut slowest);
            }
        }
        assert!(o.is_empty(), "nothing is left behind");
        delivered
    }

    #[test]
    fn answered_during_a_twelve_minute_outage_means_no_message_at_all() {
        assert!(twelve_minute_outage(&[entry("a", Kind::Waiting, 0)], Some(5 * 60 * 1000)).is_empty());
    }

    #[test]
    fn still_waiting_after_a_twelve_minute_outage_is_told_once() {
        let delivered = twelve_minute_outage(&[entry("a", Kind::Waiting, 0)], None);
        assert!(matches!(&delivered[..], [Attempt::One(e)] if e.id == "a"), "{delivered:?}");
    }

    #[test]
    fn several_after_a_twelve_minute_outage_are_one_summary() {
        let entries = [entry("a", Kind::Waiting, 0), entry("b", Kind::Waiting, 1000), entry("c", Kind::Approval, 2000)];
        let delivered = twelve_minute_outage(&entries, None);
        assert_eq!(delivered.len(), 1, "{delivered:?}");
        assert!(matches!(&delivered[0], Attempt::Together { sessions: 3, needs_you: true, ids, .. } if ids.len() == 3));
    }

    #[test]
    fn what_waits_for_nobody_goes_at_ten_minutes_and_is_counted() {
        let mut o = Outbox::open(&scratch("age"));
        o.add(entry("done", Kind::Finished, 0));
        o.add(entry("waiting", Kind::Waiting, 0));
        assert_eq!(o.prune(MAX_AGE_MS, &always), 0, "not yet");
        assert_eq!(o.prune(MAX_AGE_MS + 1, &always), 1);
        assert_eq!(o.entries.len(), 1);
        assert_eq!(o.entries[0].id, "waiting");
    }

    #[test]
    fn a_session_counts_once_in_a_summary_however_many_entries_it_has() {
        let mut o = Outbox::open(&scratch("once"));
        let mut a = entry("a", Kind::Finished, 0);
        a.session = Some("same".into());
        let mut b = entry("b", Kind::Waiting, 1);
        b.session = Some("same".into());
        o.add(a);
        o.add(b);
        o.failed(0, None, &mut slowest);
        let Pick::Go(Attempt::Together { sessions, needs_you, .. }) = o.pick(o.next_at) else { panic!("together") };
        assert_eq!((sessions, needs_you), (1, true));
    }

    #[test]
    fn a_refusal_is_final_and_a_retry_never_follows() {
        let mut o = Outbox::open(&scratch("refused"));
        o.add(entry("a", Kind::Waiting, 0));
        let Pick::Go(a) = o.pick(0) else { panic!("due") };
        assert_eq!(o.refused(&a), 1);
        assert_eq!(o.pick(0), Pick::Idle);
    }

    #[test]
    fn at_load_what_can_wait_is_gathered_and_what_needs_someone_is_not_held() {
        let mut o = Outbox::open(&scratch("load"));
        let mut one = entry("one", Kind::Finished, 100);
        one.hold_until = 1100;
        let mut two = entry("two", Kind::Automation, 600);
        two.hold_until = 1600;
        o.add(one);
        o.add(two);
        assert_eq!(o.entries[1].hold_until, 1100, "the same window");
        assert_eq!(o.pick(700), Pick::Wait(Duration::from_millis(400)));
        o.add(entry("urgent", Kind::Waiting, 800));
        let Pick::Go(a) = o.pick(800) else { panic!("not held") };
        assert!(matches!(&a, Attempt::One(e) if e.id == "urgent"));
        o.sent(&a);
        let Pick::Go(a) = o.pick(1100) else { panic!("window over") };
        assert!(matches!(&a, Attempt::Together { sessions: 2, needs_you: false, .. }));
        o.sent(&a);
        assert!(o.is_empty());
        // Alone in its window, it is itself.
        let mut one = entry("one", Kind::Finished, 100);
        one.hold_until = 1100;
        o.add(one);
        assert!(matches!(o.pick(1100), Pick::Go(Attempt::One(_))));
    }

    #[test]
    fn what_a_phones_entry_keeps_holds_no_text() {
        let path = scratch("text");
        let mut o = Outbox::open(&path);
        o.add(Entry { payload: None, ..entry("a", Kind::Waiting, 1) });
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(file.contains("\"payload\":null") && file.contains("s-a"), "{file}");
    }
}
