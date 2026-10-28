//! The daemon copies the app starts from (`<data dir>/daemon/a` and `…/b`, so updates never stop
//! it; see the app's `daemon_runtime`). A daemon starting from one of them removes anything else
//! there no daemon runs from: folders of earlier layouts and abandoned copies.

use std::path::Path;

const SLOTS: &[&str] = &["a", "b"];

/// Tidies `root` when `exe` is itself one of its slots.
pub fn prune(exe: &Path, root: &Path) {
    let Some(current) = exe.parent() else { return };
    if current.parent() != Some(root) {
        return; // started from the install folder or a build folder: nothing of ours to tidy
    }
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && !SLOTS.iter().any(|s| entry.file_name() == *s) {
            remove_unused(&path);
        }
    }
}

/// Removes `dir` unless a daemon still runs from it: its program is removed first, which Windows
/// refuses while it runs, so a copy in use is never half-deleted.
fn remove_unused(dir: &Path) {
    let exe = dir.join(if cfg!(windows) { "kelpied.exe" } else { "kelpied" });
    if exe.exists() && std::fs::remove_file(&exe).is_err() {
        return;
    }
    if let Err(e) = std::fs::remove_dir_all(dir) {
        tracing::debug!("old daemon copy {}: {e}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make(root: &Path, name: &str) -> std::path::PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(if cfg!(windows) { "kelpied.exe" } else { "kelpied" }), name).unwrap();
        std::fs::write(dir.join("conpty.dll"), "x").unwrap();
        dir
    }

    #[test]
    fn keeps_both_slots_and_removes_everything_else() {
        let root = std::env::temp_dir().join(format!("kelpie-runtimes-{}", std::process::id())).join("daemon");
        let _ = std::fs::remove_dir_all(&root);
        let a = make(&root, "a");
        let b = make(&root, "b");
        let old = make(&root, "0.1.3-6a1b-2f00");
        let tmp = make(&root, ".tmp-1-2");
        prune(&a.join(if cfg!(windows) { "kelpied.exe" } else { "kelpied" }), &root);
        assert!(a.exists() && b.exists(), "slots stay");
        assert!(!old.exists() && !tmp.exists(), "earlier layouts and leftovers go");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn leaves_folders_alone_when_not_started_from_a_copy() {
        let base = std::env::temp_dir().join(format!("kelpie-runtimes-install-{}", std::process::id()));
        let root = base.join("daemon");
        let old = make(&root, "0.0.1-z");
        prune(&base.join("install").join("kelpied.exe"), &root);
        assert!(old.exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}
