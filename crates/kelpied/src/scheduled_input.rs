//! "Continue at reset": a timer that types `continue` into a session a usage limit stopped, once
//! the limit lifts. The daemon keeps it, so it fires with the window closed.
//!
//! The timer remembers which run of the process (`generation`), which conversation and which turn
//! it was set for. When it fires it types only if that session is still stopped by the limit with
//! nothing typed since the failed turn began (`Session::continue_if_limited`, checked and written
//! under the session's lock); otherwise it is dropped and the app is told why, for an inbox note.
//! Nothing is ever typed blind.
//!
//! Timers are written to `scheduled.json`. A process cannot outlive its daemon, so after a restart
//! none can pass its guard: the ones found at start are reported once as lost (`take_lost`), and
//! the app turns them into inbox notes instead of letting them vanish.

use std::path::PathBuf;

use anyhow::{bail, Result};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::persist::save_state;

/// What is typed.
pub const CONTINUE_TEXT: &str = "continue";
/// After the reset time the limit may still answer for a moment: wait this long. A test run
/// (`KELPIE_E2E`) may shorten it with `KELPIE_NOTIFY_RESET_MARGIN_MS`.
pub const RESET_MARGIN_MS: u64 = 30_000;

fn reset_margin_ms() -> u64 {
    let test_run = std::env::var_os("KELPIE_E2E").is_some();
    std::env::var("KELPIE_NOTIFY_RESET_MARGIN_MS").ok().filter(|_| test_run).and_then(|v| v.parse().ok()).unwrap_or(RESET_MARGIN_MS)
}

/// A reset further away than this is not a usage window.
const MAX_AHEAD_MS: u64 = 8 * 24 * 3600 * 1000;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Scheduled {
    pub session: String,
    pub generation: u64,
    pub agent_session_id: Option<String>,
    /// The turn the limit ended (`Session::turn_seq`).
    #[serde(default)]
    pub turn: u64,
    /// When it fires (wall-clock ms): the reset time plus [`RESET_MARGIN_MS`].
    pub fire_at: u64,
    /// Where the reset time came from: the usage meter, the user typed it in, or the agent's own limit message
    /// (`message`, set by the daemon itself when the session stopped on the limit).
    pub source: String,
}

#[derive(Default)]
pub struct ScheduledInputs {
    path: Option<PathBuf>,
    items: Mutex<Vec<Scheduled>>,
    /// Timers a previous daemon run left behind: reported once, never fired.
    lost: Mutex<Vec<Scheduled>>,
    /// The stops the person cancelled the timer of, `(session, generation, turn)`, one per session: nothing sets a timer for
    /// that stop by itself again (a Cancel sticks through a reopened window). Kept in memory: a new daemon runs new processes.
    declined: Mutex<Vec<(String, u64, u64)>>,
}

/// Sources of a timer Kelpie sets by itself: from the agent's limit message (the daemon) or the usage meter (the app, when the
/// message named no time). They give way to the person: a stop whose timer was cancelled, or that has one, is left alone.
pub const AUTO_SOURCES: [&str; 2] = ["message", "meter-auto"];

impl ScheduledInputs {
    /// Reads what an earlier run left in `<dir>/scheduled.json` as lost, and starts empty.
    pub fn open(dir: &std::path::Path) -> ScheduledInputs {
        let path = dir.join("scheduled.json");
        let lost: Vec<Scheduled> = crate::persist::load_state_or_default(&path);
        ScheduledInputs { path: Some(path), items: Mutex::new(Vec::new()), lost: Mutex::new(lost), declined: Mutex::new(Vec::new()) }
    }

    fn save(&self, items: &[Scheduled]) {
        let Some(path) = &self.path else { return };
        let written: Result<()> = if items.is_empty() {
            // The previous version goes first: left alone, it would come back as lost timers.
            [kelpie_proto::durable::prev_path(path), path.clone()].iter().try_for_each(|p| match std::fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            })
        } else {
            serde_json::to_vec(items).map_err(anyhow::Error::from).and_then(|b| save_state(path, &b))
        };
        if let Err(e) = written {
            tracing::warn!("save scheduled timers: {e:#}");
        }
    }

    /// Sets (or replaces) the session's timer. `reset_at` must come from the usage meter, be
    /// confirmed by the user or be read from the agent's limit message, and lie ahead.
    #[allow(clippy::too_many_arguments)] // one value per guard of the timer
    pub fn schedule(&self, session: &str, generation: u64, agent_session_id: Option<String>, turn: u64, reset_at: u64, source: &str, now: u64) -> Result<Scheduled> {
        if !matches!(source, "meter" | "confirmed") && !AUTO_SOURCES.contains(&source) {
            bail!("the reset time comes from the usage meter, the agent's limit message, or is confirmed by the user");
        }
        if reset_at <= now {
            bail!("that reset time has passed");
        }
        if reset_at > now + MAX_AHEAD_MS {
            bail!("that reset time is more than a week away");
        }
        let item = Scheduled { session: session.into(), generation, agent_session_id, turn, fire_at: reset_at + reset_margin_ms(), source: source.into() };
        let mut items = self.items.lock();
        if AUTO_SOURCES.contains(&source) {
            if self.declined.lock().iter().any(|(s, g, t)| s == session && *g == generation && *t == turn) {
                bail!("the timer for this stop was cancelled");
            }
            if items.iter().any(|i| i.session == session && i.generation == generation && i.turn == turn) {
                bail!("this stop already has a timer");
            }
        }
        items.retain(|i| i.session != session);
        items.push(item.clone());
        self.save(&items);
        Ok(item)
    }

    /// The person cancelled the session's timer: it goes, and Kelpie sets none by itself for that stop again.
    pub fn decline(&self, session: &str) -> bool {
        let found = self.items.lock().iter().find(|i| i.session == session).map(|i| (i.generation, i.turn));
        if let Some((generation, turn)) = found {
            let mut declined = self.declined.lock();
            declined.retain(|(s, ..)| s != session);
            declined.push((session.to_string(), generation, turn));
        }
        self.cancel(session)
    }

    /// The timers Kelpie set by itself go (the setting was turned off); their sessions are returned.
    pub fn cancel_auto(&self) -> Vec<String> {
        let gone: Vec<String> = self.items.lock().iter().filter(|i| AUTO_SOURCES.contains(&i.source.as_str())).map(|i| i.session.clone()).collect();
        for session in &gone {
            self.cancel(session);
        }
        gone
    }

    pub fn cancel(&self, session: &str) -> bool {
        let mut items = self.items.lock();
        let before = items.len();
        items.retain(|i| i.session != session);
        let changed = items.len() != before;
        if changed {
            self.save(&items);
        }
        changed
    }

    /// The timers whose time has come; they are removed.
    pub fn take_due(&self, now: u64) -> Vec<Scheduled> {
        let mut items = self.items.lock();
        if items.is_empty() {
            return Vec::new();
        }
        let (due, rest): (Vec<_>, Vec<_>) = items.drain(..).partition(|i| i.fire_at <= now);
        *items = rest;
        if !due.is_empty() {
            self.save(&items);
        }
        due
    }

    pub fn list(&self) -> Vec<Scheduled> {
        self.items.lock().clone()
    }

    /// Timers an earlier daemon run lost, once.
    /// The file keeps them until then: a daemon that stops before any app asked reports them next time.
    pub fn take_lost(&self) -> Vec<Scheduled> {
        let lost = std::mem::take(&mut *self.lost.lock());
        if !lost.is_empty() {
            self.save(&self.items.lock());
        }
        lost
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timer_needs_a_trusted_future_time() {
        let t = ScheduledInputs::default();
        assert!(t.schedule("s", 1, None, 1, 10_000, "meter", 5_000).is_ok());
        assert!(t.schedule("s", 1, None, 1, 4_000, "meter", 5_000).is_err(), "in the past");
        assert!(t.schedule("s", 1, None, 1, 5_000, "meter", 5_000).is_err(), "now is not ahead");
        assert!(t.schedule("s", 1, None, 1, 10_000, "guess", 5_000).is_err(), "unknown source");
        assert!(t.schedule("s", 1, None, 1, 10_000, "message", 5_000).is_ok(), "read from the limit message");
        assert!(t.schedule("s", 1, None, 1, 5_000 + MAX_AHEAD_MS + 1, "confirmed", 5_000).is_err());
        assert!(t.schedule("s", 1, None, 1, 5_000 + MAX_AHEAD_MS, "confirmed", 5_000).is_ok());
    }

    #[test]
    fn it_fires_after_the_margin_once_and_one_per_session() {
        let t = ScheduledInputs::default();
        t.schedule("a", 3, Some("conv".into()), 1, 10_000, "meter", 0).unwrap();
        t.schedule("a", 4, Some("conv".into()), 1, 20_000, "meter", 0).unwrap();
        t.schedule("b", 1, None, 1, 12_000, "confirmed", 0).unwrap();
        assert_eq!(t.list().len(), 2, "a second timer for the same session replaces the first");
        assert!(t.take_due(12_000 + RESET_MARGIN_MS - 1).is_empty(), "the margin is waited out");
        let due = t.take_due(12_000 + RESET_MARGIN_MS);
        assert_eq!(due.len(), 1);
        assert_eq!((due[0].session.as_str(), due[0].fire_at), ("b", 12_000 + RESET_MARGIN_MS));
        assert!(t.take_due(u64::MAX / 2).iter().all(|i| i.session == "a" && i.generation == 4));
        assert!(t.take_due(u64::MAX / 2).is_empty(), "taken once");
    }

    #[test]
    fn a_cancelled_stop_is_not_set_again_by_itself_and_an_automatic_time_never_replaces_another() {
        let t = ScheduledInputs::default();
        t.schedule("a", 1, None, 4, 10_000, "message", 0).unwrap();
        assert!(t.schedule("a", 1, None, 4, 20_000, "meter-auto", 0).is_err(), "the meter does not replace the message's time");
        assert!(t.decline("a"));
        for source in AUTO_SOURCES {
            assert!(t.schedule("a", 1, None, 4, 10_000, source, 0).is_err(), "{source}: the person said no for this stop");
        }
        assert!(t.schedule("a", 1, None, 5, 10_000, "message", 0).is_ok(), "a later stop is a new stop");
        assert!(t.schedule("a", 1, None, 6, 10_000, "meter-auto", 0).is_ok(), "and a timer left from an earlier stop gives way");
        t.cancel("a");
        assert!(t.schedule("a", 1, None, 4, 10_000, "confirmed", 0).is_ok(), "the person may still set one");
        t.schedule("b", 1, None, 1, 10_000, "meter-auto", 0).unwrap();
        assert_eq!(t.cancel_auto(), vec!["b".to_string()], "turning the setting off drops what Kelpie set, not what the person set");
        assert_eq!(t.list().len(), 1);
    }

    #[test]
    fn cancel_removes_only_that_session() {
        let t = ScheduledInputs::default();
        t.schedule("a", 1, None, 1, 10_000, "meter", 0).unwrap();
        t.schedule("b", 1, None, 1, 10_000, "meter", 0).unwrap();
        assert!(t.cancel("a"));
        assert!(!t.cancel("a"));
        assert_eq!(t.list().len(), 1);
    }

    #[test]
    fn timers_survive_in_the_file_and_are_reported_as_lost_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("kelpie-scheduled-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = ScheduledInputs::open(&dir);
        t.schedule("a", 2, Some("conv".into()), 3, 10_000, "meter", 0).unwrap();
        t.schedule("b", 1, None, 1, 10_000, "confirmed", 0).unwrap();
        t.cancel("b");
        assert!(dir.join("scheduled.json").exists());
        // The daemon stops and starts again: nothing runs, the timer is reported once.
        let unasked = ScheduledInputs::open(&dir);
        drop(unasked); // stops again before any app asked
        let again = ScheduledInputs::open(&dir);
        assert!(again.list().is_empty(), "a timer of a dead process never fires");
        let lost = again.take_lost();
        assert_eq!(lost.len(), 1);
        assert_eq!((lost[0].session.as_str(), lost[0].generation, lost[0].turn), ("a", 2, 3));
        assert!(again.take_lost().is_empty(), "reported once");
        assert!(!dir.join("scheduled.json").exists(), "and not again after another restart");
        assert!(!dir.join("scheduled.json.prev").exists(), "nor from the previous version");
        assert!(ScheduledInputs::open(&dir).take_lost().is_empty());
        // The last timer going away empties the file.
        again.schedule("c", 1, None, 1, 10_000, "meter", 0).unwrap();
        assert!(again.take_due(u64::MAX / 2).len() == 1 && !dir.join("scheduled.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
