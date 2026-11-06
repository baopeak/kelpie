//! A host's file edited in an external editor: a private copy in `<data>/sftp-edit/<id>/<name>` (the name kept so
//! the right app opens), opened with the system's default app or one the person picked, and watched. A save shows
//! up as `sftp-edit-changed {id}` once the file has been quiet for 500 ms and its size is stable; the window then
//! uploads it through the daemon's queue with the server copy's size and time as `expect`, so a change made on the
//! server meanwhile is asked about, never overwritten. The download and the uploads are ordinary transfer jobs.
//!
//! The folder is watched rather than the file: many editors save by writing a new file and renaming it over the
//! old one. Their swap and backup files are ignored. Copies are removed on Stop, and copies older than 7 days at
//! the next start.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

use crate::fs_ops::{err, CmdResult};

const QUIET: Duration = Duration::from_millis(500);
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

#[derive(Default)]
pub struct EditSessions {
    live: Mutex<HashMap<String, Live>>,
}

struct Live {
    _watcher: RecommendedWatcher,
}

fn root() -> PathBuf {
    kelpie_proto::data_dir().join("sftp-edit")
}

/// Session ids are made here: hex and dashes only, so one can never name a path outside the root.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']) && !name.chars().any(char::is_control)
}

/// Editors' own temporary files next to the one being edited.
pub fn editor_temp(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.ends_with(".swp") || lower.ends_with(".swx") || lower.ends_with(".swo") || lower.ends_with('~') || lower.ends_with(".tmp") || lower.ends_with(".bak") || lower.starts_with(".#") || lower.starts_with("~$") || lower == "4913" || lower.starts_with(".~lock.")
}

/// Whether a change to `name` in the session's folder is a save of the edited file `watched`.
pub fn concerns(name: &str, watched: &str) -> bool {
    name == watched && !editor_temp(name)
}

#[derive(Serialize)]
pub struct Prepared {
    id: String,
    path: String,
}

/// A private folder for one edit session; returns where the copy is to be downloaded.
#[tauri::command]
pub fn sftp_edit_prepare(name: String) -> CmdResult<Prepared> {
    if !valid_name(&name) {
        return Err("invalid file name".into());
    }
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let id = format!("{nanos:x}-{:x}-{:x}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed));
    let dir = root().join(&id);
    kelpie_proto::ensure_private_dir(&root()).map_err(err)?;
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(Prepared { id, path: dir.join(&name).to_string_lossy().into_owned() })
}

/// Opens the downloaded copy (with `open_with`, an app the person picked, else the default one) and starts watching it.
#[tauri::command]
pub fn sftp_edit_watch(app: AppHandle, state: State<'_, EditSessions>, id: String, name: String, open_with: Option<String>) -> CmdResult<()> {
    if !valid_id(&id) || !valid_name(&name) {
        return Err("invalid edit session".into());
    }
    let dir = root().join(&id);
    let file = dir.join(&name);
    if !file.is_file() {
        return Err("the downloaded copy is missing".into());
    }
    let (tx, rx) = mpsc::channel::<()>();
    let watched = name.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        let Ok(event) = res else { return };
        if matches!(event.kind, EventKind::Access(_) | EventKind::Remove(_)) {
            return;
        }
        if event.paths.iter().any(|p| p.file_name().is_some_and(|n| concerns(&n.to_string_lossy(), &watched))) {
            let _ = tx.send(());
        }
    })
    .map_err(err)?;
    watcher.watch(&dir, RecursiveMode::NonRecursive).map_err(err)?;
    let (emit_id, path) = (id.clone(), file.clone());
    std::thread::Builder::new()
        .name("kelpie-sftp-edit".into())
        .spawn(move || debounce(rx, &path, || {
            let _ = app.emit("sftp-edit-changed", serde_json::json!({ "id": emit_id }));
        }))
        .map_err(err)?;
    state.live.lock().insert(id, Live { _watcher: watcher });
    let opened = match open_with.filter(|w| !w.trim().is_empty()) {
        Some(app) => tauri_plugin_opener::open_path(&file, Some(app)),
        None => tauri_plugin_opener::open_path(&file, None::<&str>),
    };
    opened.map_err(|e| format!("could not open the file: {e}"))
}

/// Calls `changed` once per save: after `QUIET` without events, and only when the size holds still.
pub fn debounce(rx: mpsc::Receiver<()>, path: &Path, mut changed: impl FnMut()) {
    while rx.recv().is_ok() {
        let mut last = Instant::now();
        loop {
            match rx.recv_timeout(QUIET.saturating_sub(last.elapsed())) {
                Ok(()) => last = Instant::now(),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
        // A save still being written: wait until two looks agree.
        let mut size = std::fs::metadata(path).map(|m| m.len()).ok();
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(200));
            let now = std::fs::metadata(path).map(|m| m.len()).ok();
            if now == size {
                break;
            }
            size = now;
        }
        if size.is_some() {
            changed();
        }
    }
}

/// Stops watching and deletes the local copy.
#[tauri::command]
pub fn sftp_edit_stop(state: State<'_, EditSessions>, id: String) -> CmdResult<()> {
    if !valid_id(&id) {
        return Err("invalid edit session".into());
    }
    state.live.lock().remove(&id);
    match std::fs::remove_dir_all(root().join(&id)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
        _ => Ok(()),
    }
}

/// At start: copies older than 7 days go.
pub fn cleanup() {
    remove_older_than(&root(), KEEP, SystemTime::now());
}

pub fn remove_older_than(root: &Path, keep: Duration, now: SystemTime) {
    let Ok(dirs) = std::fs::read_dir(root) else { return };
    for d in dirs.flatten() {
        let name = d.file_name().to_string_lossy().into_owned();
        if !valid_id(&name) {
            continue;
        }
        let old = d.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_dir_all(d.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_editor_swap_files() {
        for n in [".main.rs.swp", "main.rs~", "x.tmp", "4913", ".#main.rs", "~$doc.docx", ".~lock.a.odt#"] {
            assert!(editor_temp(n), "{n}");
        }
        for n in ["main.rs", "notes.txt", "swap.rs"] {
            assert!(!editor_temp(n), "{n}");
        }
        assert!(concerns("main.rs", "main.rs"));
        assert!(!concerns(".main.rs.swp", "main.rs") && !concerns("main.rs~", "main.rs") && !concerns("other.rs", "main.rs"));
    }

    #[test]
    fn ids_and_names_cannot_leave_the_folder() {
        assert!(valid_id("18c3f-1a2b-0") && !valid_id("../x") && !valid_id("") && !valid_id("a/b"));
        assert!(valid_name("report (1).pdf") && !valid_name("..") && !valid_name("a/b") && !valid_name("a\\b") && !valid_name(""));
    }

    #[test]
    fn three_quick_writes_are_one_upload() {
        let dir = std::env::temp_dir().join(format!("kelpie-edit-debounce-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"v1").unwrap();
        let (tx, rx) = mpsc::channel();
        let count = std::sync::Arc::new(AtomicU64::new(0));
        let c = count.clone();
        let path = file.clone();
        let t = std::thread::spawn(move || debounce(rx, &path, || {
            c.fetch_add(1, Ordering::Relaxed);
        }));
        for _ in 0..3 {
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(60));
        }
        std::thread::sleep(Duration::from_millis(1500));
        drop(tx);
        t.join().unwrap();
        assert_eq!(count.load(Ordering::Relaxed), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cleanup_keeps_recent_copies() {
        let root = std::env::temp_dir().join(format!("kelpie-edit-cleanup-{}", std::process::id()));
        std::fs::create_dir_all(root.join("abc1")).unwrap();
        std::fs::create_dir_all(root.join("not an id")).unwrap();
        remove_older_than(&root, KEEP, SystemTime::now());
        assert!(root.join("abc1").exists(), "a recent copy stays");
        remove_older_than(&root, KEEP, SystemTime::now() + KEEP + Duration::from_secs(60));
        assert!(!root.join("abc1").exists(), "an old copy goes");
        assert!(root.join("not an id").exists(), "only Kelpie's own folders are touched");
        let _ = std::fs::remove_dir_all(&root);
    }
}
