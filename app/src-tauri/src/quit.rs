//! Closing Kelpie. Closing the main window, or quitting the app another way (Cmd+Q, the last
//! window going), is held back and handed to the window as a `quit-requested` event: the UI
//! decides whether the session daemon stops too (Settings → Session Daemon → When Kelpie closes),
//! may ask first, and then calls `quit_app`, which lets the exit through.
//!
//! What never comes here: an update (`app.restart()` asks to exit with a code, and the Windows
//! installer exits the process directly or through Restart Manager's `WM_ENDSESSION`), an exit
//! with a code, a window that is not the main one, and a UI that is not listening or stopped
//! answering. Those close as they always did, and never stop the daemon.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, WindowEvent};

use crate::daemon_client::DaemonClient;
use crate::fs_ops::CmdResult;

pub const MAIN: &str = "main";
/// A request the UI has not even acknowledged after this long means it is hung: the next close goes through.
const UNANSWERED: Duration = Duration::from_secs(5);
/// How long a stop may wait for the daemon before the app exits anyway.
const STOP_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Decision {
    /// Close or exit as usual.
    Allow,
    /// Hold it and ask the UI.
    AskUi,
}

#[derive(Default)]
struct GateState {
    /// The UI listens for `quit-requested`.
    ui_ready: bool,
    /// `quit_app` was called (or an update restarts the app): every close and exit goes through.
    confirmed: bool,
    /// The main window's close went through: the exit that follows is not asked again.
    main_closing: bool,
    /// When the UI was last asked, and whether it acknowledged that.
    asked: Option<Instant>,
    seen: bool,
}

impl GateState {
    fn on_close(&mut self, now: Instant) -> Decision {
        let unanswered = self.asked.is_some_and(|at| !self.seen && now.duration_since(at) >= UNANSWERED);
        if self.confirmed || !self.ui_ready || unanswered {
            self.main_closing = true;
            return Decision::Allow;
        }
        if self.asked.is_none() || self.seen {
            self.asked = Some(now);
            self.seen = false;
        }
        Decision::AskUi
    }

    /// `code`: an exit someone asked for with a code (`app.exit`, `app.restart`) is never held.
    fn on_exit(&mut self, code: Option<i32>, main_exists: bool, now: Instant) -> Decision {
        if code.is_some() || !main_exists || self.main_closing {
            return Decision::Allow;
        }
        self.on_close(now)
    }
}

/// Whether closes and exits are held for the UI; managed state.
#[derive(Default)]
pub struct QuitGate(Mutex<GateState>);

impl QuitGate {
    /// Lets every close and exit through from now on.
    pub fn confirm(&self) {
        self.0.lock().confirmed = true;
    }
}

/// The main window's events: a close is held and handed to the UI.
pub fn on_window_event(window: &tauri::Window, event: &WindowEvent) {
    let WindowEvent::CloseRequested { api, .. } = event else { return };
    if window.label() != MAIN {
        return;
    }
    let app = window.app_handle();
    if app.state::<QuitGate>().0.lock().on_close(Instant::now()) == Decision::AskUi {
        api.prevent_close();
        let _ = app.emit_to(MAIN, "quit-requested", json!({}));
    }
}

/// The app's run events: an exit with no code (Cmd+Q, the last window closed) is held while the main window is there.
pub fn on_run_event(app: &AppHandle, event: &RunEvent) {
    let RunEvent::ExitRequested { code, api, .. } = event else { return };
    let main_exists = app.get_webview_window(MAIN).is_some();
    if app.state::<QuitGate>().0.lock().on_exit(*code, main_exists, Instant::now()) == Decision::AskUi {
        api.prevent_exit();
        let _ = app.emit_to(MAIN, "quit-requested", json!({}));
    }
}

/// The UI listens for `quit-requested` (called once it is ready).
#[tauri::command]
pub fn quit_guard(gate: State<'_, QuitGate>) {
    gate.0.lock().ui_ready = true;
}

/// The UI got a `quit-requested` (it may still be asking the person).
#[tauri::command]
pub fn quit_seen(gate: State<'_, QuitGate>) {
    gate.0.lock().seen = true;
}

/// Exits the app; `stop_daemon` first stops the session daemon gracefully (sessions come back
/// interrupted and agents resume; transfers show Interrupted). The app does not start it again.
#[tauri::command]
pub async fn quit_app(app: AppHandle, gate: State<'_, QuitGate>, client: State<'_, Arc<DaemonClient>>, stop_daemon: bool) -> CmdResult<()> {
    gate.confirm();
    if stop_daemon {
        client.stop_respawning();
        // The reply may race the daemon exiting; an unreachable daemon is not waited for long.
        let _ = tokio::time::timeout(STOP_WAIT, client.call("daemon.shutdown", json!({"kill_sessions": false}))).await;
    }
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> GateState {
        GateState { ui_ready: true, ..Default::default() }
    }

    #[test]
    fn a_close_goes_to_the_ui_only_once_it_listens() {
        let now = Instant::now();
        assert_eq!(GateState::default().on_close(now), Decision::Allow);
        assert_eq!(ready().on_close(now), Decision::AskUi);
    }

    #[test]
    fn a_confirmed_quit_is_never_asked_again() {
        let now = Instant::now();
        let mut g = ready();
        assert_eq!(g.on_close(now), Decision::AskUi);
        g.confirmed = true;
        assert_eq!(g.on_close(now), Decision::Allow);
        assert_eq!(g.on_exit(None, true, now), Decision::Allow);
    }

    #[test]
    fn exits_with_a_code_or_without_the_main_window_are_never_held() {
        let now = Instant::now();
        let mut g = ready();
        // app.exit(0) after the UI decided, and app.restart() after an update.
        assert_eq!(g.on_exit(Some(0), true, now), Decision::Allow);
        assert_eq!(g.on_exit(Some(i32::MAX), true, now), Decision::Allow);
        assert_eq!(g.on_exit(None, false, now), Decision::Allow);
        assert_eq!(g.on_exit(None, true, now), Decision::AskUi);
    }

    #[test]
    fn the_exit_after_a_main_close_that_went_through_is_not_asked() {
        let now = Instant::now();
        let mut g = GateState::default();
        assert_eq!(g.on_close(now), Decision::Allow);
        g.ui_ready = true;
        assert_eq!(g.on_exit(None, true, now), Decision::Allow);
    }

    #[test]
    fn a_ui_that_never_acknowledges_does_not_hold_the_window_forever() {
        let start = Instant::now();
        let mut g = ready();
        assert_eq!(g.on_close(start), Decision::AskUi);
        // Clicked again at once: asked again, not let through.
        assert_eq!(g.on_close(start + Duration::from_secs(1)), Decision::AskUi);
        assert_eq!(g.on_close(start + UNANSWERED), Decision::Allow);
    }

    #[test]
    fn a_ui_that_answers_keeps_holding_closes_while_the_person_decides() {
        let start = Instant::now();
        let mut g = ready();
        assert_eq!(g.on_close(start), Decision::AskUi);
        g.seen = true;
        // The dialog stays up a minute; each close in that time is asked, and each must be seen again.
        assert_eq!(g.on_close(start + Duration::from_secs(60)), Decision::AskUi);
        assert!(!g.seen);
        assert_eq!(g.on_close(start + Duration::from_secs(60) + UNANSWERED), Decision::Allow);
    }
}
