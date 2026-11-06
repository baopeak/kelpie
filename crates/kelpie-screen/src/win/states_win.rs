//! One look at the machine for `states`: which desktop takes input (the user's, or the secure one of UAC and the lock screen), whether the
//! session is locked, and whether the window in front (or the target) runs at a higher integrity level than this helper.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Security::{GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY};
use windows::Win32::System::RemoteDesktop::{WTSFreeMemory, WTSQuerySessionInformationW, WTSSessionInfoEx, WTSINFOEXW, WTS_CURRENT_SERVER_HANDLE, WTS_CURRENT_SESSION, WTS_SESSIONSTATE_LOCK};
use windows::Win32::System::StationsAndDesktops::{CloseDesktop, GetUserObjectInformationW, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, UOI_NAME};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use super::from_wide;
use super::targets_win::{exists, minimized, process_of};
use crate::states::{Desktop, Observed, TargetSeen};

fn input_desktop() -> Desktop {
    unsafe {
        // The secure desktop cannot be opened from the user's session at all; the user's own one is called "Default".
        let Ok(desk) = OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) else { return Desktop::Secure };
        let mut name = [0u16; 64];
        let mut needed = 0u32;
        let ok = GetUserObjectInformationW(HANDLE(desk.0), UOI_NAME, Some(name.as_mut_ptr() as *mut _), (name.len() * 2) as u32, Some(&mut needed)).is_ok();
        let _ = CloseDesktop(desk);
        if ok && from_wide(&name).eq_ignore_ascii_case("Default") {
            Desktop::Default
        } else {
            Desktop::Secure
        }
    }
}

fn session_locked() -> bool {
    unsafe {
        let mut buf = windows::core::PWSTR::null();
        let mut len = 0u32;
        if WTSQuerySessionInformationW(Some(WTS_CURRENT_SERVER_HANDLE), WTS_CURRENT_SESSION, WTSSessionInfoEx, &mut buf, &mut len).is_err() || buf.is_null() {
            return false;
        }
        let info = &*(buf.0 as *const WTSINFOEXW);
        let locked = info.Level == 1 && info.Data.WTSInfoExLevel1.SessionFlags == WTS_SESSIONSTATE_LOCK as i32;
        WTSFreeMemory(buf.0 as *mut _);
        locked
    }
}

/// The integrity level's RID of a process's token (`SECURITY_MANDATORY_*_RID`), or None when it cannot be read: a process this helper
/// cannot even query is taken as higher.
fn integrity(process: HANDLE) -> Option<u32> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
        let mut buf = [0u8; 64];
        let mut len = 0u32;
        let got = GetTokenInformation(token, TokenIntegrityLevel, Some(buf.as_mut_ptr() as *mut _), buf.len() as u32, &mut len);
        let _ = CloseHandle(token);
        got.ok()?;
        let label = &*(buf.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *windows::Win32::Security::GetSidSubAuthorityCount(sid);
        Some(*windows::Win32::Security::GetSidSubAuthority(sid, u32::from(count.saturating_sub(1))))
    }
}

fn own_integrity() -> u32 {
    integrity(unsafe { GetCurrentProcess() }).unwrap_or(0x2000)
}

fn elevated(h: HWND, mine: u32) -> bool {
    if h.is_invalid() {
        return false;
    }
    let pid = process_of(h);
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return true };
        let level = integrity(p);
        let _ = CloseHandle(p);
        level.is_none_or(|l| l > mine)
    }
}

pub struct Looker {
    mine: u32,
}

impl Looker {
    pub fn new() -> Looker {
        Looker { mine: own_integrity() }
    }

    /// `window`: the target window and its process, for a window target.
    pub fn look(&self, window: Option<(HWND, u32)>) -> Observed {
        let desktop = input_desktop();
        let session_locked = session_locked();
        let fg = unsafe { GetForegroundWindow() };
        let foreground_elevated = desktop == Desktop::Default && elevated(fg, self.mine);
        let (target, target_elevated) = match window {
            None => (TargetSeen::Shown, false),
            Some((h, pid)) if !exists(h, pid) => (TargetSeen::Closed, false),
            Some((h, _)) => (if minimized(h) { TargetSeen::Minimized } else { TargetSeen::Shown }, elevated(h, self.mine)),
        };
        Observed { desktop, session_locked, foreground_elevated, target_elevated, target }
    }
}

impl Default for Looker {
    fn default() -> Self {
        Looker::new()
    }
}
