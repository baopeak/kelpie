//! One look at the Mac for `states`: whether the session is locked (the lock screen, the login window, another user at the console) or the
//! screen saver is up, whether the target window is still there or minimized, and whether a password field holds the keyboard (secure event
//! input). A Mac has no elevated windows that drop posted input the way UIPI does.

use super::ffi::{bool_of, cfstr, dict_get, Cf, CGSessionCopyCurrentDictionary, IsSecureEventInputEnabled};
use super::targets_mac;
use crate::states::{Desktop, Observed, TargetSeen};

fn session_locked() -> bool {
    let Some(d) = Cf::take(unsafe { CGSessionCopyCurrentDictionary() }) else {
        // No session for this process: nothing it shows is the person's desktop.
        return true;
    };
    let locked = bool_of(dict_get(d.ptr(), cfstr("CGSSessionScreenIsLocked").ptr())).unwrap_or(false);
    let on_console = bool_of(dict_get(d.ptr(), cfstr("kCGSSessionOnConsoleKey").ptr())).unwrap_or(true);
    locked || !on_console
}

fn screen_saver() -> bool {
    targets_mac::on_screen().iter().any(|w| w.owner.contains("ScreenSaver"))
}

/// `window`: the target window's number and its process, for a window target.
pub fn look(window: Option<(u32, i32)>) -> Observed {
    let session_locked = session_locked() || screen_saver();
    let target = match window {
        None => TargetSeen::Shown,
        Some((number, pid)) => match targets_mac::window(number) {
            Some(w) if w.pid == pid => {
                // Off screen: minimized, hidden with its app, or on another Space; a click brings it back in each case.
                if w.onscreen {
                    TargetSeen::Shown
                } else {
                    TargetSeen::Minimized
                }
            }
            _ => TargetSeen::Closed,
        },
    };
    Observed { desktop: Desktop::Default, session_locked, foreground_elevated: false, target_elevated: false, target, secure_input: unsafe { IsSecureEventInputEnabled() } != 0 }
}
