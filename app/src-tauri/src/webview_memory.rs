//! Tells WebView2 how much memory a view should keep (Windows).
//!
//! A view nobody has seen for a few seconds — the main window minimised, or a browser pane hidden —
//! is asked for the low memory target (`ICoreWebView2_19::SetMemoryUsageTargetLevel`): WebView2
//! trims its working set and drops caches. Scripts keep running, so terminals, chat and
//! notifications behave as before. Shown again, the view goes back to the normal target at once.
//! The delay keeps a pane that a menu or dialog covers for a moment from being trimmed and paged
//! back in. Other platforms have no such knob.

#[cfg(windows)]
use tauri::Manager;

/// How long a view stays out of sight before it is asked to keep less memory.
#[cfg(windows)]
const SETTLE: std::time::Duration = std::time::Duration::from_secs(5);

/// Follows the main window: minimising it lowers the target of every view inside it, restoring it
/// brings back each view that is shown. Called once at start-up.
pub fn watch_main_window(app: &tauri::AppHandle) {
    #[cfg(windows)]
    {
        let Some(window) = app.get_window("main") else { return };
        let handle = app.clone();
        window.on_window_event(move |e| {
            // Minimise and restore arrive as a resize; focus covers a restore reported late.
            if matches!(e, tauri::WindowEvent::Resized(_) | tauri::WindowEvent::Focused(_)) {
                for wv in handle.webviews().into_values() {
                    if wv.window().label() == "main" {
                        fit(&wv);
                    }
                }
            }
        });
    }
    #[cfg(not(windows))]
    let _ = app;
}

/// Sets the view's target from what is on screen: normal at once when it is shown; when it is not,
/// low only if it is still not shown `SETTLE` later. Runs on the window thread after anything
/// posted before it (a pending `hide`/`show` included), and changes the level only when it differs,
/// so the many resizes of a drag cost a read each.
pub fn fit(wv: &tauri::Webview) {
    #[cfg(windows)]
    apply(wv, false);
    #[cfg(not(windows))]
    let _ = wv;
}

/// Browser panes an agent uses now: they keep the normal target even when not shown, since a
/// trimmed page can take seconds to answer the agent's calls.
#[cfg(windows)]
static KEEP: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// An agent starts (`on`) or stops using the view: while it does, the view keeps the normal target.
pub fn keep_normal(wv: &tauri::Webview, on: bool) {
    #[cfg(windows)]
    {
        let label = wv.label().to_string();
        let changed = {
            let mut keep = KEEP.lock().unwrap_or_else(|e| e.into_inner());
            let had = keep.contains(&label);
            if on && !had {
                keep.push(label);
            } else if !on && had {
                keep.retain(|l| *l != label);
            }
            on != had
        };
        if changed {
            fit(wv);
        }
    }
    #[cfg(not(windows))]
    let _ = (wv, on);
}

/// The view `label` is gone.
pub fn forget(label: &str) {
    #[cfg(windows)]
    KEEP.lock().unwrap_or_else(|e| e.into_inner()).retain(|l| l != label);
    #[cfg(not(windows))]
    let _ = label;
}

/// On the window thread: normal when shown or an agent uses it; when not shown, low if `settled`,
/// else a check again after `SETTLE`.
#[cfg(windows)]
fn apply(wv: &tauri::Webview, settled: bool) {
    let later = wv.clone();
    let kept = KEEP.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|l| l == wv.label());
    let _ = wv.with_webview(move |pv| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::{
            ICoreWebView2_19, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW,
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        };
        use windows::core::Interface;
        let controller = pv.controller();
        // An older WebView2 runtime without the interface keeps its default.
        let Ok(core) = controller.CoreWebView2().and_then(|c| c.cast::<ICoreWebView2_19>()) else { return };
        let want = if kept || crate::browser::shown(&controller) {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
        } else if settled {
            COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
        } else {
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(SETTLE).await;
                apply(&later, true);
            });
            return;
        };
        let mut now = COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL::default();
        if core.MemoryUsageTargetLevel(&mut now).is_ok() && now == want {
            return;
        }
        let _ = core.SetMemoryUsageTargetLevel(want);
    });
}
