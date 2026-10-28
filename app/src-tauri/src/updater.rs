//! In-app updates from GitHub Releases (`tauri-plugin-updater`, signed with minisign).
//!
//! The endpoint and public key are compile-time configuration (`tauri.conf.json`). Installing
//! never touches the session daemon: it runs from its own copy (`daemon_runtime`), so every
//! session keeps running while the installer replaces the app, and the restarted app reconnects
//! to it. Dev builds and end-to-end runs never look for updates (an install from them would
//! replace the real Kelpie); test overrides exist only with the `e2e-updater` feature.

use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::Mutex;
use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::fs_ops::{err, CmdResult};

/// The update found by the last check, kept until it is installed (a failed download can be
/// retried), and whether an install is under way.
#[derive(Default)]
pub struct PendingUpdate {
    update: Mutex<Option<Update>>,
    installing: AtomicBool,
}

#[derive(Serialize)]
pub struct UpdateInfo {
    version: String,
    current_version: String,
    notes: Option<String>,
    date: Option<String>,
}

#[cfg(feature = "e2e-updater")]
fn test_endpoint() -> Option<String> {
    std::env::var("KELPIE_UPDATE_ENDPOINT").ok()
}
#[cfg(not(feature = "e2e-updater"))]
fn test_endpoint() -> Option<String> {
    None
}

/// Only installed release builds update themselves (and test builds pointed at a test server).
fn updates_enabled() -> bool {
    test_endpoint().is_some() || !(cfg!(debug_assertions) || std::env::var_os("KELPIE_E2E").is_some())
}

fn updater(app: &AppHandle) -> Result<tauri_plugin_updater::Updater, String> {
    #[allow(unused_mut)]
    let mut builder = app.updater_builder();
    #[cfg(feature = "e2e-updater")]
    {
        if let Some(url) = test_endpoint() {
            builder = builder.endpoints(vec![url.parse().map_err(err)?]).map_err(err)?;
        }
        if let Ok(key) = std::env::var("KELPIE_UPDATE_PUBKEY") {
            builder = builder.pubkey(key);
        }
    }
    builder.build().map_err(err)
}

/// A newer release, or `None` when this one is current (or this build does not update).
#[tauri::command]
pub async fn update_check(app: AppHandle, pending: State<'_, PendingUpdate>) -> CmdResult<Option<UpdateInfo>> {
    if !updates_enabled() {
        return Ok(None);
    }
    let found = updater(&app)?.check().await.map_err(err)?;
    let info = found.as_ref().map(|u| UpdateInfo {
        version: u.version.clone(),
        current_version: u.current_version.clone(),
        notes: u.body.clone(),
        date: u.date.map(|d| d.to_string()),
    });
    *pending.update.lock() = found;
    Ok(info)
}

/// Downloads and verifies the pending update (progress as `update-progress` events), then runs
/// the installer. Windows: the installer takes over, this app exits and the installer starts the
/// new one. macOS: the bundle is replaced in place and the app restarts.
#[tauri::command]
pub async fn update_install(app: AppHandle, pending: State<'_, PendingUpdate>) -> CmdResult<()> {
    if pending.installing.swap(true, Ordering::SeqCst) {
        return Err("an update is already being installed".into());
    }
    let result = install(&app, &pending).await;
    pending.installing.store(false, Ordering::SeqCst);
    result
}

async fn install(app: &AppHandle, pending: &PendingUpdate) -> CmdResult<()> {
    let update = pending.update.lock().clone().ok_or("no update to install; check again")?;
    let mut downloaded = 0usize;
    // The plugin verifies the signature here; nothing is installed without it.
    let bytes = update
        .download(
            |chunk, total| {
                downloaded += chunk;
                let _ = app.emit("update-progress", json!({"downloaded": downloaded, "total": total}));
            },
            || {},
        )
        .await
        .map_err(err)?;
    #[cfg(feature = "e2e-updater")]
    if std::env::var_os("KELPIE_UPDATE_DRY_INSTALL").is_some() {
        pending.update.lock().take();
        let _ = app.emit("update-progress", json!({"installed": "dry"}));
        return Ok(());
    }
    update.install(bytes).map_err(err)?;
    pending.update.lock().take();
    app.restart();
}
