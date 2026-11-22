//! A desktop of the agent's own. Windows gives each desktop its own windows, focus and front window, and shows one desktop at a time:
//! an app started on this one cannot come in front of the person's windows or take their keyboard, whatever it does to itself.
//!
//! Acting on a web view makes its window ask for the front (seen in the trial of 2026-11-22: `set`, `invoke`, `toggle` and `expand` on
//! a Chromium page did, reading and the picture did not), so an app an agent is to use while the person works is started here.
//! A thread sees only the windows of the desktop it is on: every thread of this helper that lists or reads windows joins it first.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::StationsAndDesktops::{
    CreateDesktopW, SetThreadDesktop, DESKTOP_CONTROL_FLAGS,
};
use windows::Win32::System::Threading::{
    CreateProcessW, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
};

pub const NAME: &str = "kelpie-agent";
/// `GENERIC_ALL`.
const ALL_ACCESS: u32 = 0x1000_0000;

static ENTERED: AtomicBool = AtomicBool::new(false);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Put the calling thread on the agent's desktop, creating the desktop when it is not there. Before the thread makes any window or
/// starts COM.
pub fn enter() -> Result<(), String> {
    unsafe {
        // Creating a desktop that exists opens it. The handle is kept for the life of the process.
        let desk = CreateDesktopW(
            PCWSTR(wide(NAME).as_ptr()),
            PCWSTR::null(),
            None,
            DESKTOP_CONTROL_FLAGS(0),
            ALL_ACCESS,
            None,
        )
        .map_err(|e| format!("the agent's desktop could not be opened: {}", e.message()))?;
        SetThreadDesktop(desk)
            .map_err(|e| format!("the agent's desktop could not be joined: {}", e.message()))?;
    }
    ENTERED.store(true, Ordering::Relaxed);
    Ok(())
}

/// For a thread this helper starts later: on the agent's desktop when the first thread is.
pub fn rejoin() {
    if ENTERED.load(Ordering::Relaxed) {
        let _ = enter();
    }
}

/// Start a program on the agent's desktop. `command_line` is the whole line, quoted (`scope::command_line`).
pub fn start(command_line: &str) -> Result<u32, String> {
    let mut line = wide(command_line);
    let mut desktop = wide(NAME);
    let info = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut made = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            PCWSTR::null(),
            &info,
            &mut made,
        )
        .map_err(|e| format!("it could not be started: {}", e.message()))?;
        let _ = CloseHandle(made.hThread);
        let _ = CloseHandle(made.hProcess);
    }
    Ok(made.dwProcessId)
}
