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
//! A Linux AppImage copies its daemon too, as its files are only mounted while the app runs. There
//! nothing stops a running program's file from being removed, so the copy goes to slot `a` every
//! time (`b` only when `a` cannot be written): a daemon already running from `a` keeps its own,
//! now unlinked, file, as a daemon under a replaced macOS bundle or .deb does.

use std::path::{Path, PathBuf};

/// The files the daemon needs next to it: itself and the sideloaded ConPTY (whose
/// `conpty.dll` starts `OpenConsole.exe` from its own folder).
const FILES: &[&str] = &["kelpied.exe", "conpty.dll", "OpenConsole.exe"];
const SLOTS: &[&str] = &["a", "b"];
/// Which build a slot holds; written last, so a slot is used only once complete.
const BUILD_FILE: &str = "build.txt";

/// The daemon to start: a copy outside the install folder on Windows release builds and when
/// running from a Linux AppImage, the installed binary everywhere else (or when no slot can take
/// the copy). An AppImage's files live in a mount that goes away when the app quits and moves on
/// every launch, while the daemon outlives the app and agents' hook settings name its path.
pub fn runnable(installed: &Path) -> PathBuf {
    let from_appimage = cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some();
    if !(cfg!(windows) || from_appimage) || cfg!(debug_assertions) || std::env::var_os("KELPIE_DAEMON_PATH").is_some() {
        return installed.to_path_buf();
    }
    let root = kelpie_proto::data_dir().join("daemon");
    match copy_runtime(installed, &root, env!("CARGO_PKG_VERSION")) {
        Ok(exe) => exe,
        Err(e) if from_appimage => {
            // Started from the mount it still runs, but it stops when Kelpie quits (its files go away),
            // and with it every session; the update dialog says so (`daemonSurvivesUpdate`).
            tracing::warn!("could not copy the daemon out of the AppImage ({e:#}): it runs from the AppImage and stops when Kelpie quits");
            installed.to_path_buf()
        }
        Err(e) => {
            tracing::warn!("daemon runs from the install folder ({e:#}); updates will have to stop it");
            installed.to_path_buf()
        }
    }
}

/// Variables only the AppImage runtime sets (the mount, the AppImage file, how it was started).
const APPIMAGE_ONLY: &[&str] = &["APPIMAGE", "APPDIR", "ARGV0", "OWD"];
/// Search paths the runtime or Tauri's GTK hook (linuxdeploy-plugin-gtk) put `$APPDIR` entries
/// into: only those entries go, the user's own stay. Any other variable whose value lies inside
/// the mount (the hook's module and loader files, its GTK prefixes) is removed whole.
const APPIMAGE_LISTS: &[&str] = &[
    "XDG_DATA_DIRS",
    "PATH",
    "LD_LIBRARY_PATH",
    "PYTHONPATH",
    "PERL5LIB",
    "GTK_PATH",
    "GI_TYPELIB_PATH",
    "GIO_EXTRA_MODULES",
    "GSETTINGS_SCHEMA_DIR",
];
/// How a started program's environment differs from the app's when the app runs from an AppImage: each
/// `(name, None)` is removed, each `(name, Some(value))` replaces the app's value. The daemon outlives
/// the app, and every terminal and agent inherits its environment: none of them may see the mount,
/// which disappears when the app quits, or the bundled GTK modules, which belong to the AppImage's
/// own libraries. The runtime and hook keep no copy of the values they replace, so a forced
/// `GDK_BACKEND=x11` or `GTK_THEME=Adwaita:dark|light` is removed rather than restored.
pub fn appimage_env_fixes(vars: &[(String, String)]) -> Vec<(String, Option<String>)> {
    let get = |k: &str| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if get("APPIMAGE").is_none() {
        return Vec::new();
    }
    let appdir = get("APPDIR").map(|d| d.trim_end_matches('/')).filter(|d| d.len() > 1);
    let inside = |v: &str| appdir.is_some_and(|d| v == d || v.starts_with(&format!("{d}/")));
    let mut out = Vec::new();
    for (name, value) in vars {
        let forced = match name.as_str() {
            "GDK_BACKEND" => value == "x11",
            "GTK_THEME" => value == "Adwaita:dark" || value == "Adwaita:light",
            _ => false,
        };
        if APPIMAGE_ONLY.contains(&name.as_str()) || forced {
            out.push((name.clone(), None));
        } else if APPIMAGE_LISTS.contains(&name.as_str()) {
            let kept: Vec<&str> = value.split(':').filter(|e| !e.is_empty() && !inside(e)).collect();
            if kept.len() != value.split(':').count() {
                out.push((name.clone(), (!kept.is_empty()).then(|| kept.join(":"))));
            }
        } else if inside(value) {
            out.push((name.clone(), None));
        }
    }
    out
}

/// Every program the app starts (the daemon, editors, `xdg-open`, git and its helpers, user
/// commands) gets the user's environment, not the AppImage's: on Linux this applies
/// [`appimage_env_fixes`] of the app's own environment, worked out once. Elsewhere, and outside
/// an AppImage, it changes nothing.
pub fn clean_env(cmd: &mut std::process::Command) -> &mut std::process::Command {
    static FIXES: std::sync::OnceLock<Vec<(String, Option<String>)>> = std::sync::OnceLock::new();
    let fixes = FIXES.get_or_init(|| {
        if !cfg!(target_os = "linux") {
            return Vec::new();
        }
        let vars: Vec<(String, String)> = std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))).collect();
        appimage_env_fixes(&vars)
    });
    for (name, value) in fixes {
        match value {
            Some(v) => cmd.env(name, v),
            None => cmd.env_remove(name),
        };
    }
    cmd
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
            name_source(&dir, src);
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
    name_source(dir, src);
    std::fs::write(dir.join(BUILD_FILE), build)?;
    Ok(exe)
}

/// Tells the daemon in a slot where the app is installed: only that app's executable may ask for phone-access decisions
/// (`peer_identity.rs` in kelpied, `SLOT_SOURCE`).
fn name_source(dir: &Path, src: &Path) {
    let file = dir.join("source.txt");
    let text = src.to_string_lossy();
    if std::fs::read_to_string(&file).ok().as_deref() != Some(text.as_ref()) {
        if let Err(e) = std::fs::write(&file, text.as_bytes()) {
            tracing::warn!("daemon slot: cannot name the app's folder ({e}); phone pairing will not be available");
        }
    }
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
        assert_eq!(std::fs::read_to_string(root.join("a").join("source.txt")).unwrap(), base.join("install").to_string_lossy(), "the app's folder, for the daemon's trust in it");
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

    fn vars(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn the_daemon_never_inherits_the_appimage_mount_or_its_gtk_setup() {
        let d = "/tmp/.mount_KelpieAb12";
        // What the type-2 runtime and Tauri's GTK hook leave in the app's environment, next to the user's own.
        let app = vars(&[
            ("APPIMAGE", "/home/me/Apps/Kelpie_0.1.8_amd64.AppImage"),
            ("APPDIR", d),
            ("ARGV0", "./Kelpie.AppImage"),
            ("OWD", "/home/me"),
            ("GDK_BACKEND", "x11"),
            ("GTK_THEME", "Adwaita:dark"),
            ("XDG_DATA_DIRS", &format!("{d}/usr/share:/usr/share:/usr/local/share:/usr/share")),
            ("GSETTINGS_SCHEMA_DIR", &format!("{d}//usr/share/glib-2.0/schemas")),
            ("GTK_EXE_PREFIX", &format!("{d}//usr")),
            ("GTK_PATH", &format!("{d}//usr/lib/gtk-3.0")),
            ("GTK_IM_MODULE_FILE", &format!("{d}//usr/lib/gtk-3.0/3.0.0/immodules.cache")),
            ("GDK_PIXBUF_MODULE_FILE", &format!("{d}//usr/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache")),
            ("GIO_EXTRA_MODULES", &format!("{d}//usr/lib/gio/modules")),
            ("SOME_HOOK_VALUE", &format!("{d}/usr/lib/thing")),
            ("PATH", &format!("{d}/usr/bin:/home/me/.local/bin:/usr/bin")),
            ("LD_LIBRARY_PATH", &format!("{d}/usr/lib")),
            ("PYTHONPATH", "/home/me/py"),
            ("HOME", "/home/me"),
            ("SHELL", "/bin/bash"),
            ("WAYLAND_DISPLAY", "wayland-0"),
            ("NOT_THE_MOUNT", &format!("{d}-other/x")),
        ]);
        let fixes = appimage_env_fixes(&app);
        let fix = |k: &str| fixes.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        for gone in ["APPIMAGE", "APPDIR", "ARGV0", "OWD", "GDK_BACKEND", "GTK_THEME", "GSETTINGS_SCHEMA_DIR", "GTK_EXE_PREFIX", "GTK_PATH", "GTK_IM_MODULE_FILE", "GDK_PIXBUF_MODULE_FILE", "GIO_EXTRA_MODULES", "SOME_HOOK_VALUE", "LD_LIBRARY_PATH"] {
            assert_eq!(fix(gone), Some(None), "{gone} is removed");
        }
        assert_eq!(fix("XDG_DATA_DIRS"), Some(Some("/usr/share:/usr/local/share:/usr/share".into())), "only the mount's entry goes");
        assert_eq!(fix("PATH"), Some(Some("/home/me/.local/bin:/usr/bin".into())));
        for kept in ["PYTHONPATH", "HOME", "SHELL", "WAYLAND_DISPLAY", "NOT_THE_MOUNT"] {
            assert_eq!(fix(kept), None, "{kept} is the user's");
        }
        // GTK search paths and files the user set themselves stay; only the mount's entries go.
        let mixed = appimage_env_fixes(&vars(&[
            ("APPIMAGE", "/a.AppImage"),
            ("APPDIR", d),
            ("GTK_PATH", &format!("{d}//usr/lib/gtk-3.0:/opt/gtk-extra")),
            ("GI_TYPELIB_PATH", "/usr/local/lib/girepository-1.0"),
            ("GIO_EXTRA_MODULES", &format!("{d}//usr/lib/gio/modules")),
            ("GSETTINGS_SCHEMA_DIR", "/home/me/.local/share/glib-2.0/schemas"),
            ("GDK_PIXBUF_MODULE_FILE", "/home/me/loaders.cache"),
        ]));
        let mfix = |k: &str| mixed.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(mfix("GTK_PATH"), Some(Some("/opt/gtk-extra".into())));
        assert_eq!(mfix("GIO_EXTRA_MODULES"), Some(None));
        for kept in ["GI_TYPELIB_PATH", "GSETTINGS_SCHEMA_DIR", "GDK_PIXBUF_MODULE_FILE"] {
            assert_eq!(mfix(kept), None, "{kept} is the user's");
        }
        // A theme or backend the user chose is theirs.
        assert!(appimage_env_fixes(&vars(&[("APPIMAGE", "/a.AppImage"), ("APPDIR", d), ("GDK_BACKEND", "wayland"), ("GTK_THEME", "Yaru")])).iter().all(|(n, _)| n != "GDK_BACKEND" && n != "GTK_THEME"));
        // Not an AppImage (a .deb install, Windows, macOS): nothing changes, whatever the variables say.
        assert!(appimage_env_fixes(&vars(&[("APPDIR", d), ("GDK_BACKEND", "x11"), ("GTK_PATH", "/usr/lib/gtk-3.0")])).is_empty());
    }

    /// Outside an AppImage (every test run, a .deb, Windows, macOS) programs keep the app's environment untouched.
    #[test]
    fn started_programs_keep_the_environment_outside_an_appimage() {
        if std::env::var_os("APPIMAGE").is_some() {
            return;
        }
        let mut cmd = std::process::Command::new("git");
        clean_env(&mut cmd);
        assert_eq!(cmd.get_envs().count(), 0);
    }

    #[test]
    fn a_missing_daemon_is_an_error() {
        let base = std::env::temp_dir().join(format!("kelpie-runtime-missing-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        assert!(copy_runtime(&base.join("kelpied.exe"), &base.join("daemon"), "0.1.5").is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}
