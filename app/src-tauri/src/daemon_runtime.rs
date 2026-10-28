//! Where the session daemon runs from, so updates never stop it.
//!
//! On Windows a running program cannot be replaced, and the daemon (plus each session's
//! `OpenConsole.exe`) used to run from the install folder: every update had to stop it, and every
//! agent with it. Instead the app copies the daemon and its ConPTY files into one of two fixed
//! slots, `<data dir>/daemon/a` or `…/b`, and starts it there. The installer then replaces the
//! install folder freely while the daemon — and every session — keeps running; the updated app
//! reconnects to it. A newer daemon takes over only when nothing runs, or when the user asks
//! (`checkDaemonApi` in the store), and goes into the slot the running one does not use. Two fixed
//! paths, not one per build: firewall rules (phone access) and paths an agent was told keep working.

use std::path::{Path, PathBuf};

/// The files the daemon needs next to it: itself and the sideloaded ConPTY (whose
/// `conpty.dll` starts `OpenConsole.exe` from its own folder).
const FILES: &[&str] = &["kelpied.exe", "conpty.dll", "OpenConsole.exe"];
const SLOTS: &[&str] = &["a", "b"];
/// Which build a slot holds; written last, so a slot is used only once complete.
const BUILD_FILE: &str = "build.txt";

/// The daemon to start: a copy outside the install folder on Windows release builds, the
/// installed binary everywhere else (or when no slot can take the copy).
pub fn runnable(installed: &Path) -> PathBuf {
    if !cfg!(windows) || cfg!(debug_assertions) || std::env::var_os("KELPIE_DAEMON_PATH").is_some() {
        return installed.to_path_buf();
    }
    let root = kelpie_proto::data_dir().join("daemon");
    match copy_runtime(installed, &root, env!("CARGO_PKG_VERSION")) {
        Ok(exe) => exe,
        Err(e) => {
            tracing::warn!("daemon runs from the install folder ({e:#}); updates will have to stop it");
            installed.to_path_buf()
        }
    }
}

/// Puts the daemon files next to `installed` into a slot under `root` (unless one already holds
/// this build) and returns the slot's daemon.
fn copy_runtime(installed: &Path, root: &Path, version: &str) -> std::io::Result<PathBuf> {
    let src = installed.parent().ok_or_else(|| std::io::Error::other("daemon has no folder"))?;
    let exe_name = installed.file_name().ok_or_else(|| std::io::Error::other("daemon has no name"))?;
    let meta = std::fs::metadata(installed)?;
    // The build, not only the version: two builds of one version must not share a copy.
    let built = meta.modified()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let build = format!("{version} {built:x} {:x}", meta.len());
    for slot in SLOTS {
        let dir = root.join(slot);
        if std::fs::read_to_string(dir.join(BUILD_FILE)).is_ok_and(|b| b == build) && dir.join(exe_name).is_file() {
            return Ok(dir.join(exe_name));
        }
    }
    let mut last_err = std::io::Error::other("no free slot");
    for slot in SLOTS {
        let dir = root.join(slot);
        match fill_slot(&dir, src, exe_name, &build) {
            Ok(exe) => return Ok(exe),
            Err(e) => last_err = e, // in use by a running daemon: try the other one
        }
    }
    Err(last_err)
}

fn fill_slot(dir: &Path, src: &Path, exe_name: &std::ffi::OsStr, build: &str) -> std::io::Result<PathBuf> {
    let exe = dir.join(exe_name);
    // Its program first: Windows refuses while a daemon runs from it, and then nothing else in
    // the slot is touched.
    if exe.exists() {
        std::fs::remove_file(&exe)?;
    }
    let _ = std::fs::remove_file(dir.join(BUILD_FILE));
    std::fs::create_dir_all(dir)?;
    let names = FILES.iter().map(Path::new).chain(std::iter::once(Path::new(exe_name)));
    let mut copied = false;
    for name in names {
        let from = src.join(name);
        if from.is_file() {
            std::fs::copy(&from, dir.join(name))?;
            copied |= name == Path::new(exe_name);
        }
    }
    if !copied {
        return Err(std::io::Error::other("daemon binary missing"));
    }
    std::fs::write(dir.join(BUILD_FILE), build)?;
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(base: &Path, daemon: &str) -> PathBuf {
        let install = base.join("install");
        std::fs::create_dir_all(&install).unwrap();
        for (name, body) in [("kelpied.exe", daemon), ("conpty.dll", "dll"), ("OpenConsole.exe", "host"), ("Kelpie.exe", "app")] {
            std::fs::write(install.join(name), body).unwrap();
        }
        install.join("kelpied.exe")
    }

    #[test]
    fn copies_into_a_slot_once_per_build_and_alternates_while_one_is_in_use() {
        let base = std::env::temp_dir().join(format!("kelpie-runtime-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("data").join("daemon");
        let exe = copy_runtime(&install(&base, "daemon v1"), &root, "0.1.5").unwrap();
        assert_eq!(exe, root.join("a").join("kelpied.exe"));
        assert_eq!(std::fs::read_to_string(root.join("a").join("conpty.dll")).unwrap(), "dll");
        assert!(root.join("a").join("OpenConsole.exe").is_file());
        assert!(!root.join("a").join("Kelpie.exe").exists(), "only the daemon's files");
        // Same build: the same copy.
        assert_eq!(copy_runtime(&base.join("install").join("kelpied.exe"), &root, "0.1.5").unwrap(), exe);
        // A new build while slot a is in use (held open here, as a running program is): slot b.
        #[cfg(windows)]
        let held = {
            use std::os::windows::fs::OpenOptionsExt;
            const FILE_SHARE_READ: u32 = 1; // no delete sharing: like a program's running image
            std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&exe).unwrap()
        };
        #[cfg(not(windows))]
        let held = std::fs::File::open(&exe).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100)); // a new modification time
        let next = copy_runtime(&install(&base, "daemon version two"), &root, "0.1.6").unwrap();
        if cfg!(windows) {
            assert_eq!(next, root.join("b").join("kelpied.exe"));
            assert_eq!(std::fs::read_to_string(&exe).unwrap(), "daemon v1", "the running copy is untouched");
        }
        assert_eq!(std::fs::read_to_string(&next).unwrap(), "daemon version two");
        drop(held);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_missing_daemon_is_an_error() {
        let base = std::env::temp_dir().join(format!("kelpie-runtime-missing-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        assert!(copy_runtime(&base.join("kelpied.exe"), &base.join("daemon"), "0.1.5").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
