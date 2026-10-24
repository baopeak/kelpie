//! Kelpie desktop app (Tauri). The UI process owns no PTYs: terminals and agent processes
//! live in `kelpied`; this crate relays them to the webview and serves file/git/search
//! commands for the workbench.

mod accounts;
mod agent_history;
mod browser;
mod daemon_client;
mod daemon_runtime;
mod external_editor;
mod fs_ops;
mod git;
mod git_finish;
mod git_sync;
mod github;
mod integrations;
mod plugins;
mod voice;
mod remote;
mod search;
mod session_usage;
mod ssh_hosts;
mod sys_stats;
mod updater;
mod usage;
mod usage_report;
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
    Ok(Some(dst.to_string_lossy().into_owned()))
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
            let index = Arc::new(search::FileIndex::default());
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
            term_attach,
            term_detach,
            term_input,
            state_load,
            state_save,
            state_backup,
            platform_info,
            redact_batch,
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
            github::gh_pr_status,
            github::gh_default_branch,
            github::gh_pr_create,
            worktree_files::worktree_materialize,
            external_editor::editors_list,
            external_editor::editor_open,
            sys_stats::processes_alive,
            agent_history::agent_latest_session,
            agent_history::agent_claude_transcript,
            agent_history::agent_history_list,
            usage::usage_fetch,
            session_usage::session_usage,
            usage_report::usage_report,
            accounts::account_create,
            accounts::account_remove,
            accounts::account_status,
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
