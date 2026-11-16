//! Durable session metadata (`sessions.json`) and last-screen snapshots (`snapshots/<id>.ansi`).
//! All writes are atomic and flushed to the disk before the rename, so neither a crash nor a power
//! cut leaves a torn or zero-filled file; `sessions.json` also keeps its previous version.

use std::path::{Path, PathBuf};

use anyhow::Result;
use kelpie_proto::durable::{LoadError, Loaded, Source};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::session::SessionSpec;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub spec: SessionSpec,
    #[serde(default)]
    pub title: Option<String>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub chat: bool,
    #[serde(default)]
    pub last_activity: u64,
    /// Running (or interrupted and not yet resumed) when saved. Files from before this field
    /// count every session as running.
    #[serde(default = "yes")]
    pub alive: bool,
}

fn yes() -> bool {
    true
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        let _ = std::fs::create_dir_all(dir.join("snapshots"));
        Store { dir: dir.to_path_buf() }
    }

    pub fn load_sessions(&self) -> Vec<SavedSession> {
        load_state_or_default(&self.dir.join("sessions.json"))
    }

    pub fn save_sessions(&self, sessions: &[SavedSession]) -> Result<()> {
        save_state(&self.dir.join("sessions.json"), &serde_json::to_vec_pretty(sessions)?)
    }

    fn snapshot_path(&self, id: &str) -> PathBuf {
        let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        self.dir.join("snapshots").join(format!("{safe}.ansi"))
    }

    pub fn save_snapshot(&self, id: &str, data: &[u8]) -> Result<()> {
        write_atomic(&self.snapshot_path(id), data)
    }

    pub fn load_snapshot(&self, id: &str) -> Option<Vec<u8>> {
        std::fs::read(self.snapshot_path(id)).ok()
    }

    pub fn remove_snapshot(&self, id: &str) {
        let _ = std::fs::remove_file(self.snapshot_path(id));
    }
}

/// Atomically and durably replaces `path` (see [`kelpie_proto::durable`]). For files that are
/// rebuilt or rewritten anyway (snapshots, generated configuration); state uses [`save_state`].
pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    kelpie_proto::durable::write(path, data).map_err(Into::into)
}

/// Saves a JSON state file, keeping its last good version as `<name>.prev` for [`load_state`].
pub fn save_state(path: &Path, data: &[u8]) -> Result<()> {
    kelpie_proto::durable::write_with_prev(path, data, kelpie_proto::durable::is_json).map_err(Into::into)
}

/// Reads a state file written by [`save_state`]: from its previous version when the file is
/// missing or damaged (a power cut), which is logged. `Ok(None)`: no file yet. `Err`: a file is
/// there and neither it nor its previous version can be used (a copy of the damaged one was kept).
pub fn load_state<T: DeserializeOwned>(path: &Path) -> std::result::Result<Option<T>, LoadError> {
    load_state_with(path, |b| serde_json::from_slice::<T>(b))
}

/// [`load_state`] with a parser of its own (one that also rejects a document of the wrong shape).
pub fn load_state_with<T, E: std::fmt::Display>(path: &Path, parse: impl Fn(&[u8]) -> std::result::Result<T, E>) -> std::result::Result<Option<T>, LoadError> {
    load_state_from(path, parse, false).map(|o| o.map(|l| l.value))
}

/// The load behind [`load_state`], saying which version was used. `unless_removed`: a missing file stays
/// missing (a store whose removal is a reset, or whose previous version could undo a revocation).
pub fn load_state_from<T, E: std::fmt::Display>(path: &Path, parse: impl Fn(&[u8]) -> std::result::Result<T, E>, unless_removed: bool) -> std::result::Result<Option<Loaded<T>>, LoadError> {
    let loaded = if unless_removed { kelpie_proto::durable::load_unless_removed(path, parse) } else { kelpie_proto::durable::load(path, parse) };
    match &loaded {
        Ok(Some(l)) if l.source == Source::Previous => match &l.damaged {
            Some(d) => tracing::warn!("{} is damaged ({}); restored its previous version{}", path.display(), d.reason, d.kept_at.as_ref().map(|k| format!(", a copy was kept at {}", k.display())).unwrap_or_default()),
            None => tracing::warn!("{} is missing; restored its previous version", path.display()),
        },
        Err(e) => tracing::warn!("{}: {e}", path.display()),
        _ => {}
    }
    loaded
}

/// [`load_state`] for a store that starts empty when nothing usable is there, as it always did.
pub fn load_state_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    load_state(path).ok().flatten().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_zeroed_by_a_power_cut_come_back_from_the_previous_save() {
        let dir = std::env::temp_dir().join(format!("kelpie-persist-{}", uuid::Uuid::new_v4()));
        let store = Store::new(&dir);
        let one = |id: &str| serde_json::from_str::<Vec<SavedSession>>(&format!(r#"[{{"spec":{{"id":"{id}","kind":"shell","cwd":"."}},"cols":80,"rows":24}}]"#)).unwrap();
        store.save_sessions(&one("a")).unwrap();
        store.save_sessions(&one("b")).unwrap();
        assert_eq!(store.load_sessions()[0].spec.id, "b");
        let file = dir.join("sessions.json");
        std::fs::write(&file, vec![0u8; std::fs::metadata(&file).unwrap().len() as usize]).unwrap();
        assert_eq!(store.load_sessions()[0].spec.id, "a", "the previous save");
        // The next save replaces the zeroed file and keeps the good previous version.
        store.save_sessions(&one("c")).unwrap();
        assert_eq!(std::fs::read(dir.join("sessions.json.prev")).unwrap(), serde_json::to_vec_pretty(&one("a")).unwrap());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn sessions_saved_before_alive_existed_count_as_running() {
        let old = r#"[{"spec":{"id":"a","kind":"shell","cwd":"."},"cols":80,"rows":24}]"#;
        let saved: Vec<SavedSession> = serde_json::from_str(old).unwrap();
        assert!(saved[0].alive);
        let stopped = r#"[{"spec":{"id":"a","kind":"shell","cwd":"."},"cols":80,"rows":24,"alive":false}]"#;
        let saved: Vec<SavedSession> = serde_json::from_str(stopped).unwrap();
        assert!(!saved[0].alive);
    }

    #[test]
    fn sessions_saved_before_sub_sessions_load_without_a_parent_and_new_fields_round_trip() {
        let old = r#"[{"spec":{"id":"a","kind":"shell","cwd":"."},"cols":80,"rows":24}]"#;
        let saved: Vec<SavedSession> = serde_json::from_str(old).unwrap();
        assert_eq!((saved[0].spec.parent.clone(), saved[0].spec.depth, saved[0].spec.report.clone()), (None, 0, None));
        // A session nobody opened writes none of the new keys, so an older Kelpie reads the file as before.
        let written = serde_json::to_string(&saved).unwrap();
        for key in ["parent", "depth", "report"] {
            assert!(!written.contains(&format!("\"{key}\"")), "{key}: {written}");
        }
        let mut child = saved[0].clone();
        child.spec.parent = Some("p".into());
        child.spec.depth = 2;
        child.spec.report = Some(crate::session::SubReport { status: "done".into(), summary: "fixed".into(), at: 5, auto: false });
        let back: Vec<SavedSession> = serde_json::from_str(&serde_json::to_string(&[child.clone()]).unwrap()).unwrap();
        assert_eq!(back[0].spec, child.spec);
        assert!(!serde_json::to_string(&child.spec.report).unwrap().contains("auto"), "a report the agent made carries no flag");
    }
}
