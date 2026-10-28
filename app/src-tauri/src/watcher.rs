//! Recursive file watching for the projects the user is looking at (active + recent).
//!
//! Raw notify events are filtered (dependency/build dirs, `.git` internals except the files
//! that change git status) and coalesced for 150 ms, then emitted as one `fs-changed` event
//! per root with the set of affected directories, so the explorer reloads only those. The changed
//! files (a bounded list) also go to the daemon, which matches them against automations' file triggers.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::daemon_client::DaemonClient;
use crate::fs_ops::{err, CmdResult};
use crate::search::FileIndex;

const IGNORED_DIRS: &[&str] = &["node_modules", "target", "dist", "build", ".next", ".venv", "__pycache__", ".cache", ".turbo"];
const DEBOUNCE: Duration = Duration::from_millis(150);
/// Most changed files kept per batch (a build or a checkout can touch thousands).
const MAX_FILES: usize = 200;

#[derive(Default)]
struct Pending {
    dirs: BTreeSet<String>,
    /// Changed files, relative to the root.
    files: BTreeSet<String>,
    git: bool,
    structural: bool,
}

/// A root's watcher (kept for as long as it watches; dropping it stops), or the ticket of the
/// call still creating it.
enum Slot {
    Creating(u64),
    Live { _watcher: RecommendedWatcher },
}

pub struct Watchers {
    app: AppHandle,
    index: Arc<FileIndex>,
    active: Mutex<HashMap<String, Slot>>,
    tickets: AtomicU64,
    pending: Arc<Mutex<HashMap<String, Pending>>>,
    wake: Arc<tokio::sync::Notify>,
}

impl Watchers {
    pub fn new(app: AppHandle, index: Arc<FileIndex>) -> Arc<Watchers> {
        let w = Arc::new(Watchers {
            app,
            index,
            active: Mutex::new(HashMap::new()),
            tickets: AtomicU64::new(0),
            pending: Arc::new(Mutex::new(HashMap::new())),
            wake: Arc::new(tokio::sync::Notify::new()),
        });
        tauri::async_runtime::spawn(w.clone().flush_loop());
        w
    }

    async fn flush_loop(self: Arc<Self>) {
        loop {
            self.wake.notified().await;
            tokio::time::sleep(DEBOUNCE).await;
            let batch: Vec<(String, Pending)> = self.pending.lock().drain().collect();
            for (root, p) in batch {
                if p.structural {
                    self.index.invalidate(&root);
                }
                let _ = self.app.emit("fs-changed", json!({"root": root, "dirs": p.dirs, "git": p.git}));
                self.report_files(root, p.files);
            }
        }
    }

    /// Tells the daemon which files changed, for file-triggered automations. Fire and forget: a daemon
    /// that predates them answers with an error, and without one there is nobody to tell.
    fn report_files(&self, root: String, files: BTreeSet<String>) {
        if files.is_empty() {
            return;
        }
        let Some(client) = self.app.try_state::<Arc<DaemonClient>>() else { return };
        let client = client.inner().clone();
        tauri::async_runtime::spawn(async move {
            let _ = client.call("automation.event", json!({"kind": "file", "root": root, "files": files})).await;
        });
    }

    pub fn watch(&self, root: &str) -> CmdResult<()> {
        // Creating a recursive watcher can take seconds (more on a busy machine). An unwatch in
        // the meantime removes the ticket, and the finished watcher is then dropped instead of
        // holding the folder open for good (Windows cannot delete a watched folder).
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed);
        {
            let mut active = self.active.lock();
            if active.contains_key(root) {
                return Ok(());
            }
            active.insert(root.to_string(), Slot::Creating(ticket));
        }
        let created = self.create(root);
        let mut active = self.active.lock();
        if matches!(active.get(root), Some(Slot::Creating(t)) if *t == ticket) {
            match created {
                Ok(watcher) => {
                    active.insert(root.to_string(), Slot::Live { _watcher: watcher });
                }
                Err(e) => {
                    active.remove(root);
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn create(&self, root: &str) -> CmdResult<RecommendedWatcher> {
        let root_path = PathBuf::from(root);
        let (pending, wake, key) = (self.pending.clone(), self.wake.clone(), root.to_string());
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            let structural = matches!(event.kind, EventKind::Create(_) | EventKind::Remove(_) | EventKind::Modify(notify::event::ModifyKind::Name(_)));
            let mut changed = false;
            let mut guard = pending.lock();
            let entry = guard.entry(key.clone()).or_default();
            for path in &event.paths {
                match classify(&root_path, path) {
                    Change::Ignored => {}
                    Change::Git => {
                        entry.git = true;
                        changed = true;
                    }
                    Change::File { dir, path } => {
                        entry.dirs.insert(dir);
                        if entry.files.len() < MAX_FILES {
                            entry.files.insert(path);
                        }
                        entry.structural |= structural;
                        // Working-tree edits change `git status` too.
                        entry.git = true;
                        changed = true;
                    }
                }
            }
            if changed {
                drop(guard);
                wake.notify_one();
            }
        })
        .map_err(err)?;
        watcher.watch(Path::new(root), RecursiveMode::Recursive).map_err(err)?;
        Ok(watcher)
    }

    pub fn unwatch(&self, root: &str) {
        // Dropped after the lock is released: stopping a watcher can be slow on Windows.
        let slot = self.active.lock().remove(root);
        drop(slot);
    }
}

enum Change {
    Ignored,
    Git,
    /// The changed entry (relative, `/`-separated) and its parent directory ("" for root).
    File { dir: String, path: String },
}

fn classify(root: &Path, path: &Path) -> Change {
    let Ok(rel) = path.strip_prefix(root) else { return Change::Ignored };
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    if parts.first().map(String::as_str) == Some(".git") {
        // Turn checkpoints write refs under refs/kelpie: not a change of the user's git state.
        if parts.get(1).map(String::as_str) == Some("refs") && parts.get(2).map(String::as_str) == Some("kelpie") {
            return Change::Ignored;
        }
        let interesting = matches!(parts.get(1).map(String::as_str), Some("HEAD" | "index" | "refs" | "MERGE_HEAD" | "REBASE_HEAD"));
        return if interesting { Change::Git } else { Change::Ignored };
    }
    if parts.iter().any(|p| IGNORED_DIRS.contains(&p.as_str())) {
        return Change::Ignored;
    }
    Change::File { dir: parts[..parts.len().saturating_sub(1)].join("/"), path: parts.join("/") }
}

// Creating or dropping a recursive watcher on a large tree can take a long time on Windows;
// off the main thread, a project switch never waits for it.
#[tauri::command]
pub async fn fs_watch(watchers: tauri::State<'_, Arc<Watchers>>, root: String) -> CmdResult<()> {
    // Remote projects have no local folder to watch (their views refresh on demand).
    if crate::remote::is_remote(&root) {
        return Ok(());
    }
    let w = watchers.inner().clone();
    crate::fs_ops::blocking(move || w.watch(&root)).await
}

#[tauri::command]
pub async fn fs_unwatch(watchers: tauri::State<'_, Arc<Watchers>>, root: String) -> CmdResult<()> {
    let w = watchers.inner().clone();
    crate::fs_ops::blocking(move || {
        w.unwatch(&root);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_paths() {
        let root = Path::new("/p");
        assert!(matches!(classify(root, Path::new("/p/src/a.rs")), Change::File { dir, path } if dir == "src" && path == "src/a.rs"));
        assert!(matches!(classify(root, Path::new("/p/a.rs")), Change::File { dir, path } if dir.is_empty() && path == "a.rs"));
        assert!(matches!(classify(root, Path::new("/p/.git/index")), Change::Git));
        assert!(matches!(classify(root, Path::new("/p/.git/objects/ab/cd")), Change::Ignored));
        assert!(matches!(classify(root, Path::new("/p/.git/refs/heads/main")), Change::Git));
        assert!(matches!(classify(root, Path::new("/p/.git/refs/kelpie/abc/3")), Change::Ignored));
        assert!(matches!(classify(root, Path::new("/p/.git/refs/kelpie/abc/3.lock")), Change::Ignored));
        assert!(matches!(classify(root, Path::new("/p/.git/refs/kelpie")), Change::Ignored));
        assert!(matches!(classify(root, Path::new("/p/web/node_modules/x/y.js")), Change::Ignored));
    }
}
