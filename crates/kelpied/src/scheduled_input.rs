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

use crate::persist::write_atomic;

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
    /// Where the reset time came from: the usage meter, or the user typed it in.
    pub source: String,
}

#[derive(Default)]
pub struct ScheduledInputs {
    path: Option<PathBuf>,
    items: Mutex<Vec<Scheduled>>,
    /// Timers a previous daemon run left behind: reported once, never fired.
    lost: Mutex<Vec<Scheduled>>,
}

impl ScheduledInputs {
    /// Reads what an earlier run left in `<dir>/scheduled.json` as lost, and starts empty.
    pub fn open(dir: &std::path::Path) -> ScheduledInputs {
        let path = dir.join("scheduled.json");
        let lost: Vec<Scheduled> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        ScheduledInputs { path: Some(path), items: Mutex::new(Vec::new()), lost: Mutex::new(lost) }
    }

    fn save(&self, items: &[Scheduled]) {
        let Some(path) = &self.path else { return };
        let written: Result<()> = if items.is_empty() {
            match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            }
        } else {
            serde_json::to_vec(items).map_err(anyhow::Error::from).and_then(|b| write_atomic(path, &b))
        };
        if let Err(e) = written {
            tracing::warn!("save scheduled timers: {e:#}");
        }
    }

    /// Sets (or replaces) the session's timer. `reset_at` must come from the usage meter or be
    /// confirmed by the user, and lie ahead.
    #[allow(clippy::too_many_arguments)] // one value per guard of the timer
    pub fn schedule(&self, session: &str, generation: u64, agent_session_id: Option<String>, turn: u64, reset_at: u64, source: &str, now: u64) -> Result<Scheduled> {
        if !matches!(source, "meter" | "confirmed") {
            bail!("the reset time comes from the usage meter or is confirmed by the user");
        }
        if reset_at <= now {
            bail!("that reset time has passed");
        }
        if reset_at > now + MAX_AHEAD_MS {
            bail!("that reset time is more than a week away");
        }
        let item = Scheduled { session: session.into(), generation, agent_session_id, turn, fire_at: reset_at + reset_margin_ms(), source: source.into() };
        let mut items = self.items.lock();
        items.retain(|i| i.session != session);
        items.push(item.clone());
        self.save(&items);
        Ok(item)
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
        assert!(ScheduledInputs::open(&dir).take_lost().is_empty());
        // The last timer going away empties the file.
        again.schedule("c", 1, None, 1, 10_000, "meter", 0).unwrap();
        assert!(again.take_due(u64::MAX / 2).len() == 1 && !dir.join("scheduled.json").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
