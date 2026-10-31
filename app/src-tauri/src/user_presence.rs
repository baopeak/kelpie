//! Phone-access decisions from the app's window: approving a phone, raising what one may do, resetting phone access. The
//! daemon verifies the person itself before any of these raises access (`user_presence.rs` in kelpied) and builds what the
//! prompt says; this only carries the request on the app's verified connection, and lets the daemon's dialog come up in front
//! of this window.

use std::sync::Arc;

#[cfg(windows)]
use serde_json::json;
use serde_json::Value;
#[cfg(windows)]
use tauri::Manager;
use tauri::{AppHandle, State};

use crate::daemon_client::DaemonClient;
use crate::fs_ops::CmdResult;

/// The daemon's app-only methods (`auth::APP_ONLY` in kelpied): `daemon_call` refuses them.
pub const APP_ONLY: &[&str] = &["mobile.approve", "mobile.set_permission", "mobile.set_write", "mobile.reset"];

/// What may raise the daemon's verification prompt: the app-only methods, and the one setting whose raising is app-only.
pub const SECURE_CALLS: &[&str] = &["mobile.approve", "mobile.set_permission", "mobile.set_write", "mobile.reset", "mobile.set_options"];

/// Sends a phone-access decision. The window handle is only a hint for parenting the dialog; the daemon checks it.
#[tauri::command]
#[cfg_attr(not(windows), allow(unused_mut))]
pub async fn mobile_secure_call(app: AppHandle, client: State<'_, Arc<DaemonClient>>, method: String, mut params: Value) -> CmdResult<Value> {
    if !SECURE_CALLS.contains(&method.as_str()) {
        return Err(format!("{method} is not a phone-access decision"));
    }
    #[cfg(windows)]
    prepare_prompt(&app, &client, &mut params).await;
    #[cfg(not(windows))]
    let _ = &app;
    client.call(&method, params).await.map_err(|e| e.to_string())
}

/// A process that is not in the foreground cannot bring a window to the front: this one, where the person just clicked, lets the
/// daemon's dialog do it. Failing that the dialog still works, behind Kelpie.
#[cfg(windows)]
async fn prepare_prompt(app: &AppHandle, client: &DaemonClient, params: &mut Value) {
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    if let Ok(info) = client.call("daemon.info", json!({})).await {
        if let Some(pid) = info.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()) {
            // SAFETY: a plain Win32 call taking a process id.
            if let Err(e) = unsafe { AllowSetForegroundWindow(pid) } {
                tracing::debug!("allow the daemon's dialog in front: {}", e.message());
            }
        }
    }
    let window = app.get_webview_window("main").and_then(|w| w.hwnd().ok()).map(|h| h.0 as isize as i64);
    if let (Some(window), Some(params)) = (window, params.as_object_mut()) {
        params.insert("window".into(), json!(window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_only_method_goes_through_the_secure_call() {
        assert!(APP_ONLY.iter().all(|m| SECURE_CALLS.contains(m)));
        assert_eq!(APP_ONLY, ["mobile.approve", "mobile.set_permission", "mobile.set_write", "mobile.reset"]);
    }
}
