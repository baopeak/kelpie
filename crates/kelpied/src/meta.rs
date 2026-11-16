//! Workspace notes keyed by folder: a free-text *comment* (what this worktree is doing right
//! now) and a board *status*. Agents set them from a terminal (`kelpied worktree set …`); the app
//! shows them on sidebar rows and board cards. Stored in `workspace-meta.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::persist::save_state;
use crate::session::now_millis;

pub const STATUSES: &[&str] = &["todo", "progress", "review", "done"];
const MAX_COMMENT: usize = 4000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceMeta {
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub updated_at: u64,
}

pub struct MetaStore {
    path: PathBuf,
    pub map: BTreeMap<String, WorkspaceMeta>,
}

/// Same folder, same key: the canonical path when the folder exists (Windows 8.3 short names
/// and symlinks resolve to one spelling), forward slashes, no trailing slash, case-folded on
/// Windows/macOS.
pub fn key(path: &str) -> String {
    let path = path.trim();
    let canonical = std::fs::canonicalize(path).ok().map(|c| {
        let s = c.to_string_lossy().into_owned();
        s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
    });
    let mut k = canonical.as_deref().unwrap_or(path).replace('\\', "/");
    while k.len() > 1 && k.ends_with('/') && !k.ends_with(":/") {
        k.pop();
    }
    if cfg!(any(windows, target_os = "macos")) {
        k = k.to_lowercase();
    }
    k
}

impl MetaStore {
    pub fn load(dir: &Path) -> MetaStore {
        let path = dir.join("workspace-meta.json");
        let map = crate::persist::load_state_or_default(&path);
        MetaStore { path, map }
    }

    /// Applies `comment` / `status` (Some("") clears) and returns the key and new record.
    pub fn set(&mut self, path: &str, comment: Option<&str>, status: Option<&str>) -> Result<(String, WorkspaceMeta)> {
        if path.trim().is_empty() {
            bail!("missing path");
        }
        if let Some(s) = status.filter(|s| !s.is_empty()) {
            if !STATUSES.contains(&s) {
                bail!("status must be one of {}", STATUSES.join(", "));
            }
        }
        let k = key(path);
        let entry = self.map.entry(k.clone()).or_default();
        if let Some(c) = comment {
            let c: String = c.trim().chars().take(MAX_COMMENT).collect();
            entry.comment = (!c.is_empty()).then_some(c);
        }
        if let Some(s) = status {
            entry.status = (!s.is_empty()).then(|| s.to_string());
        }
        entry.updated_at = now_millis();
        let out = entry.clone();
        if out.comment.is_none() && out.status.is_none() {
            self.map.remove(&k);
        }
        save_state(&self.path, &serde_json::to_vec_pretty(&self.map)?)?;
        Ok((k, out))
    }

    /// The record for `path` or its nearest parent folder (a terminal may sit in a subfolder).
    pub fn find(&self, path: &str) -> Option<(String, WorkspaceMeta)> {
        let k = key(path);
        self.map
            .iter()
            .filter(|(mk, _)| k == **mk || k.starts_with(&format!("{mk}/")))
            .max_by_key(|(mk, _)| mk.len())
            .map(|(mk, m)| (mk.clone(), m.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_lookup() {
        assert_eq!(key("C:\\Work\\app\\"), key("C:/Work/app"));
        let dir = std::env::temp_dir().join(format!("kelpie-meta-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = MetaStore::load(&dir);
        m.set("/w/app", Some("fixing login"), Some("progress")).unwrap();
        assert_eq!(m.find("/w/app/src").unwrap().1.comment.as_deref(), Some("fixing login"));
        assert!(m.find("/w/application").is_none());
        assert!(m.set("/w/app", None, Some("bogus")).is_err());
        m.set("/w/app", Some(""), Some("")).unwrap();
        assert!(m.find("/w/app").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
