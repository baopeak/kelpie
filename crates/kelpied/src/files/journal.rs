//! The transfer journal (`<data>/transfers.json`): the jobs, so the queue survives a daemon restart. Written
//! atomically. A file that cannot be understood is copied aside and never replaced by an empty journal;
//! one that cannot be read at all turns saving off until the daemon restarts.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
/// Finished jobs are kept this long, and at most this many.
pub const KEEP_SECS: u64 = 7 * 24 * 3600;
pub const KEEP_FINISHED: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Journal<J> {
    pub version: u32,
    pub jobs: Vec<J>,
    /// Fields a newer daemon wrote, kept as they are.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl<J> Default for Journal<J> {
    fn default() -> Self {
        Journal { version: VERSION, jobs: Vec::new(), extra: Default::default() }
    }
}

#[derive(Debug)]
pub enum Loaded<J> {
    /// No journal yet.
    Fresh,
    Read(Journal<J>),
    /// Not understood: kept at `kept_at`; a new journal may be written.
    Damaged { kept_at: PathBuf, error: String },
    /// Could not be read (permissions, I/O): nothing may be written over it.
    Unreadable { error: String },
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("transfers.json")
}

pub fn load<J: DeserializeOwned>(file: &Path) -> Loaded<J> {
    let bytes = match std::fs::read(file) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Fresh,
        Err(e) => return Loaded::Unreadable { error: e.to_string() },
    };
    match serde_json::from_slice::<Journal<J>>(&bytes) {
        Ok(j) => Loaded::Read(j),
        Err(e) => {
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let kept_at = file.with_extension(format!("json.damaged-{stamp}"));
            match std::fs::copy(file, &kept_at) {
                Ok(_) => Loaded::Damaged { kept_at, error: e.to_string() },
                Err(copy) => Loaded::Unreadable { error: format!("{e}; and it could not be kept aside: {copy}") },
            }
        }
    }
}

pub fn save<J: Serialize>(file: &Path, journal: &Journal<J>) -> anyhow::Result<()> {
    crate::persist::write_atomic(file, &serde_json::to_vec_pretty(journal)?)
}

/// Drops finished jobs older than [`KEEP_SECS`], then the oldest finished ones beyond [`KEEP_FINISHED`].
/// `finished_at` is `None` for a job that has not finished.
pub fn prune<J>(jobs: &mut Vec<J>, now: u64, finished_at: impl Fn(&J) -> Option<u64>) {
    jobs.retain(|j| finished_at(j).is_none_or(|t| now.saturating_sub(t) <= KEEP_SECS));
    let mut finished: Vec<u64> = jobs.iter().filter_map(&finished_at).collect();
    if finished.len() > KEEP_FINISHED {
        finished.sort_unstable();
        let cut = finished[finished.len() - KEEP_FINISHED];
        let mut over = finished.iter().filter(|t| **t < cut).count();
        jobs.retain(|j| match finished_at(j) {
            Some(t) if t < cut && over > 0 => {
                over -= 1;
                false
            }
            _ => true,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kelpie-journal-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn round_trips_and_keeps_unknown_fields() {
        let d = temp("rt");
        let f = path(&d);
        assert!(matches!(load::<serde_json::Value>(&f), Loaded::Fresh));
        std::fs::write(&f, br#"{"version":1,"jobs":[{"id":"a","future":1}],"later":true}"#).unwrap();
        let Loaded::Read(j) = load::<serde_json::Value>(&f) else { panic!() };
        assert_eq!(j.extra.get("later"), Some(&serde_json::json!(true)));
        save(&f, &j).unwrap();
        let back: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
        assert_eq!(back["later"], true);
        assert_eq!(back["jobs"][0]["future"], 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_damaged_journal_is_kept_aside() {
        let d = temp("damaged");
        let f = path(&d);
        std::fs::write(&f, b"{not json").unwrap();
        let Loaded::Damaged { kept_at, .. } = load::<serde_json::Value>(&f) else { panic!("expected damaged") };
        assert_eq!(std::fs::read(&kept_at).unwrap(), b"{not json");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn prunes_old_and_surplus_finished_jobs() {
        let now = 10 * KEEP_SECS;
        // (id, finished_at)
        let mut jobs: Vec<(u32, Option<u64>)> = vec![(0, None), (1, Some(now - KEEP_SECS - 1)), (2, Some(now - 10))];
        prune(&mut jobs, now, |j| j.1);
        assert_eq!(jobs.iter().map(|j| j.0).collect::<Vec<_>>(), [0, 2]);

        let mut many: Vec<(u32, Option<u64>)> = (0..250).map(|i| (i, Some(now - 1000 + i as u64))).collect();
        many.push((999, None));
        prune(&mut many, now, |j| j.1);
        assert_eq!(many.len(), KEEP_FINISHED + 1);
        assert!(many.iter().any(|j| j.0 == 999), "running jobs are never pruned");
        assert!(many.iter().all(|j| j.0 >= 50 || j.0 == 999), "the oldest finished go first");
    }
}
