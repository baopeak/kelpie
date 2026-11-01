//! Kelpie desktop app (Tauri). The UI process owns no PTYs: terminals and agent processes
//! live in `kelpied`; this crate relays them to the webview and serves file/git/search
//! commands for the workbench.

mod accounts;
mod agent_config;
mod agent_history;
mod browser;
mod config_transfer;
mod daemon_client;
mod daemon_runtime;
mod external_editor;
mod fs_ops;
mod git;
mod git_basics;
mod git_finish;
mod git_sync;
mod github;
mod integrations;
mod plugins;
mod voice;
mod remote;
mod search;
mod session_export;
mod session_usage;
mod ssh_hosts;
mod sys_stats;
mod updater;
mod usage;
mod usage_report;
mod user_presence;
mod watcher;
mod worktree_files;

use std::sync::Arc;

use daemon_client::DaemonClient;
use fs_ops::{err, CmdResult};
use serde_json::Value;
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::{Manager, State};

#[tauri::command]
async fn daemon_call(client: State<'_, Arc<DaemonClient>>, method: String, params: Value) -> CmdResult<Value> {
    // Phone-access decisions go through mobile_secure_call, the one path that lets the daemon's prompt come up in front.
    if user_presence::APP_ONLY.contains(&method.as_str()) {
        return Err(format!("{method} must go through mobile_secure_call"));
    }
    client.call(&method, params).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn term_attach(client: State<'_, Arc<DaemonClient>>, handle: u32, since: Option<u64>, channel: Channel<InvokeResponseBody>) -> CmdResult<Value> {
    client.attach(handle, since, channel).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn term_detach(client: State<'_, Arc<DaemonClient>>, handle: u32) -> CmdResult<()> {
    client.detach(handle).await.map_err(|e| e.to_string())
}

/// Keystrokes / paste / chat JSON lines. Sent as a binary frame, never JSON-wrapped.
/// `binary`: xterm "binary strings" (one byte per char, e.g. legacy mouse reports).
#[tauri::command]
fn term_input(client: State<'_, Arc<DaemonClient>>, handle: u32, data: String, binary: Option<bool>) -> CmdResult<()> {
    let bytes = if binary.unwrap_or(false) { data.chars().map(|c| c as u32 as u8).collect() } else { data.into_bytes() };
    client.input(handle, bytes).map_err(|e| e.to_string())
}

/// App UI state (projects, layouts, settings) — a single JSON document owned by the UI.
#[tauri::command]
async fn state_load() -> CmdResult<Value> {
    crate::fs_ops::blocking(state_load_blocking).await
}

fn state_load_blocking() -> CmdResult<Value> {
    let path = kelpie_proto::data_dir().join("app-state.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(err),
        // Only a missing file means "no state yet"; a locked or unreadable one is an error the
        // UI must not paper over with a fresh state (it would be saved over the file).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Value::Null),
        Err(e) => Err(err(e)),
    }
}

#[tauri::command]
async fn state_save(state: Value) -> CmdResult<()> {
    crate::fs_ops::blocking(move || state_save_blocking(state)).await
}

fn state_save_blocking(state: Value) -> CmdResult<()> {
    let dir = kelpie_proto::data_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join("app-state.json");
    let tmp = dir.join("app-state.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&state).map_err(err)?).map_err(err)?;
    std::fs::rename(&tmp, &path).map_err(err)
}

/// Copies `app-state.json` to `app-state.<label>-<time>.bak.json` before a migration or a
/// fresh start could rewrite it. Each call keeps its own copy.
#[tauri::command]
async fn state_backup(label: String) -> CmdResult<Option<String>> {
    crate::fs_ops::blocking(move || state_backup_blocking(label)).await
}

fn state_backup_blocking(label: String) -> CmdResult<Option<String>> {
    if label.is_empty() || !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("invalid backup label".into());
    }
    let dir = kelpie_proto::data_dir();
    let src = dir.join("app-state.json");
    if !src.exists() {
        return Ok(None);
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let dst = dir.join(format!("app-state.{label}-{secs}.bak.json"));
    std::fs::copy(&src, &dst).map_err(err)?;
    // A migration that may run at every start (review notes) leaves a few copies, not one per start.
    if label == "notes" {
        prune_backups(&dir, "notes", 3);
    }
    Ok(Some(dst.to_string_lossy().into_owned()))
}

/// Deletes all but the newest `keep` of `app-state.<label>-<secs>.bak.json` (the time sorts them).
fn prune_backups(dir: &std::path::Path, label: &str, keep: usize) {
    let prefix = format!("app-state.{label}-");
    let mut found: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let secs = name.strip_prefix(&prefix)?.strip_suffix(".bak.json")?.parse::<u64>().ok()?;
            Some((secs, e.path()))
        })
        .collect();
    found.sort_by_key(|(secs, _)| std::cmp::Reverse(*secs));
    for (_, path) in found.into_iter().skip(keep) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod backup_tests {
    use super::prune_backups;

    #[test]
    fn only_the_newest_backups_of_a_label_are_kept() {
        let dir = std::env::temp_dir().join(format!("kelpie-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["app-state.notes-100.bak.json", "app-state.notes-200.bak.json", "app-state.notes-300.bak.json", "app-state.notes-400.bak.json", "app-state.notes-1000.bak.json", "app-state.v2-50.bak.json", "app-state.json", "app-state.notes-x.bak.json"] {
            std::fs::write(dir.join(name), "{}").unwrap();
        }
        prune_backups(&dir, "notes", 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, ["app-state.json", "app-state.notes-1000.bak.json", "app-state.notes-300.bak.json", "app-state.notes-400.bak.json", "app-state.notes-x.bak.json", "app-state.v2-50.bak.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Replies the user saved from chats, with their full text: they outlast the session, and
/// `app-state.json` is rewritten too often to carry them.
const BOOKMARKS_FILE: &str = "chat-bookmarks.json";

const MAX_BOOKMARKS_PER_PROJECT: usize = 500;
/// A temporary file this old was left by a crash, not by a write under way.
const BOOKMARKS_STALE_TEMP: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

#[tauri::command]
async fn bookmarks_load() -> CmdResult<Vec<Value>> {
    crate::fs_ops::blocking(|| {
        let dir = kelpie_proto::data_dir();
        if let Some(cutoff) = std::time::SystemTime::now().checked_sub(BOOKMARKS_STALE_TEMP) {
            bookmarks_sweep_temps(&dir, cutoff);
        }
        bookmarks_read(&dir)
    })
    .await
}

/// Removes temporary files of writes that never finished (see `write_file_atomic`), older than `cutoff`.
fn bookmarks_sweep_temps(dir: &std::path::Path, cutoff: std::time::SystemTime) {
    let prefix = format!(".{BOOKMARKS_FILE}.");
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at < cutoff);
        if old && name.starts_with(&prefix) && name.ends_with(".tmp") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn bookmarks_read(dir: &std::path::Path) -> CmdResult<Vec<Value>> {
    match std::fs::read(dir.join(BOOKMARKS_FILE)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(err),
        // As with the app state: only a missing file means "none yet". A file that is corrupt or
        // locked is an error, and the UI then keeps its hands off it.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(err(e)),
    }
}

/// The file after a change. `at_limit`: `add` was refused, its project has this many.
#[derive(serde::Serialize, Debug, PartialEq)]
struct BookmarksApplied {
    items: Vec<Value>,
    at_limit: Option<usize>,
}

/// Applies one change to the file as it is now, never to what a window remembers: another running
/// Kelpie's changes, a restored copy and items this build does not understand all stay.
#[tauri::command]
async fn bookmarks_apply(add: Option<Value>, remove: Option<String>) -> CmdResult<BookmarksApplied> {
    crate::fs_ops::blocking(move || bookmarks_change(&kelpie_proto::data_dir(), add, remove.as_deref())).await
}

fn bookmarks_change(dir: &std::path::Path, add: Option<Value>, remove: Option<&str>) -> CmdResult<BookmarksApplied> {
    // Within this process. Across processes the window is the few milliseconds of this function.
    static WRITER: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    // An error here also stops a file that went bad after the UI read it: it is never replaced.
    let mut items = bookmarks_read(dir)?;
    let before = items.len();
    if let Some(id) = remove {
        items.retain(|b| b.get("id").and_then(Value::as_str) != Some(id));
    }
    let mut changed = items.len() != before;
    let mut at_limit = None;
    if let Some(item) = add {
        if !items.iter().any(|b| same_bookmark(b, &item)) {
            if items.iter().filter(|b| b.get("projectId") == item.get("projectId")).count() >= MAX_BOOKMARKS_PER_PROJECT {
                at_limit = Some(MAX_BOOKMARKS_PER_PROJECT);
            } else {
                items.push(item);
                changed = true;
            }
        }
    }
    if changed {
        std::fs::create_dir_all(dir).map_err(err)?;
        config_transfer::write_file_atomic(&dir.join(BOOKMARKS_FILE), &serde_json::to_vec_pretty(&items).map_err(err)?)?;
    }
    Ok(BookmarksApplied { items, at_limit })
}

/// The same bookmark, or the same reply saved already (from another window, say): one anchor in one
/// session or conversation, as the UI's `isSaved` sees it.
fn same_bookmark(a: &Value, b: &Value) -> bool {
    let text = |v: &'_ Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
    let same = |key: &str| text(a, key).is_some() && text(a, key) == text(b, key);
    same("id") || (same("anchor") && (same("sessionId") || same("conversationId")))
}

#[cfg(test)]
mod bookmarks_tests {
    use super::{bookmarks_change, bookmarks_read, bookmarks_sweep_temps, BOOKMARKS_FILE, MAX_BOOKMARKS_PER_PROJECT};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime};

    fn item(id: &str, project: &str, anchor: &str) -> Value {
        json!({ "id": id, "projectId": project, "sessionId": format!("session-of-{id}"), "anchor": anchor, "text": "café\n```\ncode\n```", "at": 1 })
    }

    fn ids(items: &[Value]) -> Vec<&str> {
        items.iter().map(|b| b["id"].as_str().unwrap_or("?")).collect()
    }

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-bookmarks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn names(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }

    #[test]
    fn a_missing_file_is_an_empty_list() {
        assert_eq!(bookmarks_read(&dir("missing")).unwrap(), Vec::<serde_json::Value>::new());
    }

    #[test]
    fn what_is_added_reads_back_and_goes_again_with_nothing_left_beside_it() {
        let dir = dir("round-trip");
        let done = bookmarks_change(&dir, Some(item("a", "p", "1")), None).unwrap();
        assert_eq!(done.items, [item("a", "p", "1")]);
        assert_eq!(done.at_limit, None);
        bookmarks_change(&dir, Some(item("b", "p", "2")), None).unwrap();
        assert_eq!(bookmarks_read(&dir).unwrap(), [item("a", "p", "1"), item("b", "p", "2")]);

        let done = bookmarks_change(&dir, None, Some("a")).unwrap();
        assert_eq!(done.items, [item("b", "p", "2")]);
        assert_eq!(bookmarks_read(&dir).unwrap(), done.items);
        assert_eq!(names(&dir), [BOOKMARKS_FILE]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_starts_from_the_file_so_another_process_and_a_restored_copy_stay() {
        let dir = dir("others");
        bookmarks_change(&dir, Some(item("mine", "p", "1")), None).unwrap();
        // Another Kelpie, or a backup put back, while this one remembers only "mine".
        let theirs = serde_json::to_vec(&[item("theirs", "p", "2"), item("restored", "q", "3")]).unwrap();
        std::fs::write(dir.join(BOOKMARKS_FILE), theirs).unwrap();

        let done = bookmarks_change(&dir, Some(item("new", "p", "4")), None).unwrap();
        assert_eq!(ids(&done.items), ["theirs", "restored", "new"]);
        let done = bookmarks_change(&dir, None, Some("mine")).unwrap();
        assert_eq!(ids(&done.items), ["theirs", "restored", "new"]);
        assert_eq!(bookmarks_read(&dir).unwrap(), done.items);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn items_this_build_does_not_understand_stay_as_they_are() {
        let dir = dir("unknown");
        let unknown = [json!({ "kind": "from-a-later-build", "nested": { "a": [1, 2] } }), json!(42), json!("text"), json!({ "id": 7 })];
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(BOOKMARKS_FILE), serde_json::to_vec(&unknown).unwrap()).unwrap();

        bookmarks_change(&dir, Some(item("a", "p", "1")), None).unwrap();
        let done = bookmarks_change(&dir, None, Some("a")).unwrap();
        assert_eq!(done.items, unknown);
        assert_eq!(bookmarks_read(&dir).unwrap(), unknown);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_full_project_refuses_one_more_and_keeps_every_one_it_has() {
        let dir = dir("limit");
        let full: Vec<Value> = (0..MAX_BOOKMARKS_PER_PROJECT).map(|i| item(&format!("old{i}"), "p", &format!("a{i}"))).collect();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(BOOKMARKS_FILE);
        std::fs::write(&path, serde_json::to_vec(&full).unwrap()).unwrap();
        let bytes = std::fs::read(&path).unwrap();

        let done = bookmarks_change(&dir, Some(item("one-too-many", "p", "x")), None).unwrap();
        assert_eq!(done.at_limit, Some(MAX_BOOKMARKS_PER_PROJECT));
        assert_eq!(done.items, full);
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "nothing to write");

        // Another project has its own count, and with one gone there is room again.
        assert_eq!(bookmarks_change(&dir, Some(item("elsewhere", "q", "x")), None).unwrap().at_limit, None);
        let done = bookmarks_change(&dir, Some(item("fits-now", "p", "x")), Some("old0")).unwrap();
        assert_eq!(done.at_limit, None);
        assert!(ids(&done.items).contains(&"fits-now") && !ids(&done.items).contains(&"old0"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_reply_is_not_saved_twice_also_by_two_windows() {
        let dir = dir("twice");
        let first = json!({ "id": "a", "projectId": "p", "sessionId": "s1", "conversationId": "c1", "anchor": "1" });
        bookmarks_change(&dir, Some(first.clone()), None).unwrap();
        let path = dir.join(BOOKMARKS_FILE);
        let bytes = std::fs::read(&path).unwrap();

        for same in [
            json!({ "id": "a", "projectId": "p", "sessionId": "other", "anchor": "other" }),
            json!({ "id": "b", "projectId": "p", "sessionId": "s1", "anchor": "1" }),
            json!({ "id": "c", "projectId": "p", "sessionId": "s2", "conversationId": "c1", "anchor": "1" }),
        ] {
            assert_eq!(bookmarks_change(&dir, Some(same), None).unwrap().items, [first.clone()]);
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes, "nothing to write");

        for other in [
            json!({ "id": "d", "projectId": "p", "sessionId": "s1", "anchor": "2" }),
            json!({ "id": "e", "projectId": "p", "sessionId": "s2", "conversationId": "c2", "anchor": "1" }),
            json!({ "id": "f", "projectId": "p", "sessionId": "s3", "anchor": "1" }),
        ] {
            bookmarks_change(&dir, Some(other), None).unwrap();
        }
        assert_eq!(ids(&bookmarks_read(&dir).unwrap()), ["a", "d", "e", "f"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn changes_at_the_same_moment_do_not_lose_each_other() {
        let dir = dir("threads");
        std::thread::scope(|scope| {
            for i in 0..16 {
                let dir = &dir;
                scope.spawn(move || bookmarks_change(dir, Some(item(&format!("t{i}"), "p", &format!("a{i}"))), None).unwrap());
            }
        });
        assert_eq!(bookmarks_read(&dir).unwrap().len(), 16);
        assert_eq!(names(&dir), [BOOKMARKS_FILE]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_old_temporary_files_of_this_file_are_swept() {
        let dir = dir("sweep");
        std::fs::create_dir_all(&dir).unwrap();
        let old = SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
        let write = |name: &str, at: SystemTime| {
            std::fs::write(dir.join(name), "x").unwrap();
            std::fs::File::options().write(true).open(dir.join(name)).unwrap().set_modified(at).unwrap();
        };
        write(".chat-bookmarks.json.1-2-3.tmp", old);
        write(".chat-bookmarks.json.4-5-6.tmp", SystemTime::now());
        write(BOOKMARKS_FILE, old);
        write(".other.json.1-2-3.tmp", old);
        write("chat-bookmarks.json.tmp", old);
        write(".chat-bookmarks.json.bak", old);

        bookmarks_sweep_temps(&dir, SystemTime::now() - Duration::from_secs(24 * 60 * 60));
        let mut left = names(&dir);
        left.sort();
        assert_eq!(left, [".chat-bookmarks.json.4-5-6.tmp", ".chat-bookmarks.json.bak", ".other.json.1-2-3.tmp", BOOKMARKS_FILE, "chat-bookmarks.json.tmp"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupt_file_errors_and_stays_byte_identical() {
        let dir = dir("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(BOOKMARKS_FILE);
        // Cut short, not JSON, empty, and valid JSON that is no list.
        for bytes in [&b"[{\"id\":\"a\",\"te"[..], b"\xff\xfe not json \r\n", b"", b"{\"id\":\"a\"}"] {
            std::fs::write(&path, bytes).unwrap();
            bookmarks_read(&dir).unwrap_err();
            bookmarks_change(&dir, Some(item("new", "p", "1")), None).unwrap_err();
            bookmarks_change(&dir, None, Some("a")).unwrap_err();
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            assert_eq!(names(&dir), [BOOKMARKS_FILE]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[tauri::command]
fn platform_info() -> Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "home": std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).unwrap_or_default(),
        "data_dir": kelpie_proto::data_dir(),
        // The daemon feature level this app was built against, and the release both come from.
        "api": kelpie_proto::API_LEVEL,
        "version": env!("CARGO_PKG_VERSION"),
        "security": kelpie_proto::DAEMON_SECURITY_LEVEL,
        // Set by the end-to-end test runner: lets it drive a release build too.
        "e2e": std::env::var_os("KELPIE_E2E").is_some(),
    })
}

/// Text about to leave a session (a hand-off, Copy Context, notes or an issue sent to an agent)
/// with recognisable credentials masked.
#[tauri::command]
fn redact_batch(texts: Vec<String>) -> Vec<String> {
    texts.iter().map(|t| kelpie_proto::redact(t)).collect()
}

/// End-to-end tests only (dev builds or KELPIE_E2E): resizes the window itself. WebDriver's
/// `window/rect` moves the WebView2 control inside the window instead, so the whole UI ends up
/// offset and clipped.
#[tauri::command]
fn test_window_size(window: tauri::Window, width: f64, height: f64) -> CmdResult<()> {
    if !(cfg!(debug_assertions) || std::env::var_os("KELPIE_E2E").is_some()) {
        return Err("only available to the end-to-end tests".into());
    }
    if window.is_maximized().unwrap_or(false) {
        window.unmaximize().map_err(err)?;
    }
    window.set_size(tauri::LogicalSize::new(width, height)).map_err(err)
}

/// The webview profile (browser-pane logins, the remembered theme) lives in a folder named after
/// the app identifier, which changed with the rename to Kelpie: move the old one over once.
/// Same rules as the data migration: never for dev / test instances or debug builds.
fn move_legacy_webview_profile() {
    if std::env::var_os("KELPIE_DATA_DIR").is_some() || cfg!(debug_assertions) {
        return;
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/WebKit"))
    } else {
        None
    };
    let Some(base) = base else { return };
    let (old, new) = (base.join("dev.erka.desktop"), base.join("dev.kelpie.desktop"));
    if old.is_dir() && !new.exists() {
        // Fails while the old app is still open; its logins then stay where they are.
        if let Err(e) = std::fs::rename(&old, &new) {
            eprintln!("kelpie: could not move the Erka webview profile: {e}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Before anything reads app-state.json, starts the daemon or opens a webview.
    if let Err(e) = kelpie_proto::migrate_legacy_data_dir() {
        eprintln!("kelpie: could not move data from the Erka folder: {e}");
    }
    // Settings, history caches and the daemon copies live here: for this user only (0700 on Unix).
    match kelpie_proto::ensure_private_dir(&kelpie_proto::data_dir()) {
        Ok(None) => {}
        Ok(Some(warning)) => eprintln!("kelpie: {warning}"),
        Err(e) => eprintln!("kelpie: could not prepare the data folder: {e}"),
    }
    move_legacy_webview_profile();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // Its link interception would also run in browser panes, where it cannot work (no IPC):
        // the app opens links itself.
        .plugin(tauri_plugin_opener::Builder::new().open_js_links_on_click(false).build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(updater::PendingUpdate::default())
        .setup(|app| {
            fs_ops::sweep_temp();
            let index = Arc::new(search::FileIndex::default());
            daemon_runtime::name_app_folder(&daemon_client::daemon_exe());
            app.manage(DaemonClient::start(app.handle().clone()));
            app.manage(watcher::Watchers::new(app.handle().clone(), index.clone()));
            app.manage(index);
            app.manage(browser::Browsers::default());
            app.manage(ssh_hosts::Forwards::default());
            ssh_hosts::init();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            daemon_call,
            user_presence::mobile_secure_call,
            term_attach,
            term_detach,
            term_input,
            state_load,
            state_save,
            state_backup,
            bookmarks_load,
            bookmarks_apply,
            platform_info,
            redact_batch,
            config_transfer::config_export,
            config_transfer::config_import_read,
            config_transfer::config_backup,
            config_transfer::config_hosts_current,
            config_transfer::config_hosts_write,
            updater::update_check,
            updater::update_install,
            fs_ops::fs_list_dir,
            fs_ops::fs_is_dir,
            fs_ops::fs_canonical,
            fs_ops::fs_read_file,
            fs_ops::fs_write_file,
            fs_ops::fs_create,
            fs_ops::fs_rename,
            fs_ops::fs_delete,
            fs_ops::fs_reveal,
            fs_ops::fs_write_temp,
            fs_ops::fs_read_image,
            search::quick_open,
            search::search_text,
            watcher::fs_watch,
            watcher::fs_unwatch,
            git::git_status,
            git::git_branch_info,
            git::git_file_versions,
            git::git_stage,
            git::git_discard,
            git::git_commit,
            git::git_worktrees,
            git::git_worktree_add,
            git::git_worktree_remove,
            git::git_worktree_prune,
            git::git_show_file,
            git::git_orphan_commits,
            git::git_head,
            git_finish::review_changes,
            git_finish::finish_inspect,
            git_finish::finish_wip_commit,
            git_finish::finish_integrate,
            git_finish::finish_restore_branch,
            git_finish::finish_cancel,
            git_finish::finish_reset,
            git_sync::git_remote,
            git_sync::git_repo_state,
            git_sync::git_abort,
            git_sync::git_branch_delete,
            git_sync::git_refs,
            git_sync::git_context,
            git_basics::git_branches,
            git_basics::git_switch_branch,
            git_basics::git_log,
            git_basics::git_stashes,
            git_basics::git_stash_action,
            github::gh_pr_status,
            github::gh_default_branch,
            github::gh_pr_create,
            worktree_files::worktree_materialize,
            worktree_files::worktree_materialize_estimate,
            external_editor::editors_list,
            external_editor::editor_open,
            sys_stats::processes_alive,
            agent_history::agent_latest_session,
            agent_history::agent_claude_transcript,
            agent_history::agent_history_list,
            session_export::agent_session_export,
            usage::usage_fetch,
            session_usage::session_usage,
            usage_report::usage_report,
            accounts::account_create,
            accounts::account_remove,
            accounts::account_status,
            accounts::account_transcript,
            agent_config::agent_setup_overview,
            agent_config::agent_setup_preview,
            agent_config::agent_setup_apply,
            agent_config::agent_setup_backups,
            agent_config::agent_setup_reveal,
            agent_config::agent_registry_fetch,
            integrations::integration_secret_set,
            integrations::integration_secret_clear,
            integrations::integration_secret_has,
            integrations::linear_issues,
            integrations::linear_states,
            integrations::linear_set_state,
            integrations::linear_viewer,
            integrations::jira_issues,
            integrations::jira_transitions,
            integrations::jira_transition,
            integrations::jira_myself,
            integrations::gitlab_project,
            integrations::gitlab_mr_status,
            integrations::gitlab_default_branch,
            integrations::gitlab_mr_create,
            integrations::gitlab_issues,
            integrations::gitlab_user,
            browser::browser_open,
            browser::browser_bounds,
            browser::browser_visible,
            browser::browser_navigate,
            browser::browser_go,
            browser::browser_design,
            browser::browser_devtools,
            browser::browser_close,
            browser::browser_probe,
            browser::browser_info,
            remote::remote_configure,
            remote::remote_hosts,
            remote::remote_check,
            remote::remote_add_user,
            remote::remote_folders,
            plugins::plugin_list,
            plugins::plugin_dir,
            plugins::plugin_install_folder,
            plugins::plugin_install_git,
            plugins::plugin_remove,
            plugins::plugin_approve,
            plugins::plugin_create,
            plugins::plugin_status,
            voice::voice_available,
            voice::voice_models,
            voice::voice_devices,
            voice::voice_download,
            voice::voice_download_cancel,
            voice::voice_delete,
            voice::voice_start,
            voice::voice_stop,
            voice::voice_cancel,
            voice::voice_transcribe,
            browser::browser_screenshot,
            browser::browser_test_input,
            test_window_size,
            ssh_hosts::ssh_vault_load,
            ssh_hosts::ssh_vault_save,
            ssh_hosts::ssh_host_touch,
            ssh_hosts::ssh_paths,
            ssh_hosts::ssh_session_env,
            ssh_hosts::ssh_import_config,
            ssh_hosts::ssh_secret_set,
            ssh_hosts::ssh_secret_has,
            ssh_hosts::ssh_keys_list,
            ssh_hosts::ssh_key_generate,
            ssh_hosts::ssh_key_import,
            ssh_hosts::ssh_key_delete,
            ssh_hosts::ssh_key_install,
            ssh_hosts::ssh_agent_list,
            ssh_hosts::ssh_agent_add,
            ssh_hosts::ssh_known_hosts_list,
            ssh_hosts::ssh_known_hosts_remove,
            ssh_hosts::ssh_host_keys,
            ssh_hosts::ssh_host_trust,
            ssh_hosts::ssh_transfer,
            ssh_hosts::ssh_forward_start,
            ssh_hosts::ssh_forward_stop,
            ssh_hosts::ssh_forward_status,
            sys_stats::memory_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Kelpie");
}
