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
    CreateProcessW, ResumeThread, TerminateProcess, CREATE_SUSPENDED, PROCESS_CREATION_FLAGS,
    PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTUPINFOW,
};

use super::job::Job;

pub const NAME: &str = "kelpie-agent";
/// `SW_SHOWNOACTIVATE`: the program's first window shows without taking the focus from the person (a program may still take it).
const SHOW_NO_ACTIVATE: u16 = 4;
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

/// Start a program: on the agent's desktop (`hidden`), or on the person's with its first window shown and not activated.
/// `command_line` is the whole line, quoted (`scope::command_line`).
///
/// The program gets none of this helper's handles. With them it would hold the pipe this helper's output goes to: a caller that reads
/// that output to its end (an agent's tool call does) would then wait for as long as the program runs (seen in CI: a check that read
/// `launch` through a pipe never came back).
///
/// What comes back is the
/// process id and its creation time, read while the process's handle is still open: read after, a program that ended at once could have
/// left its id to another process, which would then be taken for it.
///
/// With a `job`, the process is created suspended, put in the job, and only then let run: it never runs a line of its own code, nor
/// starts a child, outside the job. A process that cannot be put in it is ended and the start fails.
pub fn start(command_line: &str, hidden: bool, job: Option<&Job>) -> Result<(u32, u64), String> {
    let mut line = wide(command_line);
    let mut desktop = wide(NAME);
    let mut info = STARTUPINFOW {
        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
        ..Default::default()
    };
    if hidden {
        info.lpDesktop = PWSTR(desktop.as_mut_ptr());
    } else {
        info.dwFlags = STARTF_USESHOWWINDOW;
        info.wShowWindow = SHOW_NO_ACTIVATE;
    }
    let mut made = PROCESS_INFORMATION::default();
    unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            false,
            if job.is_some() {
                CREATE_SUSPENDED
            } else {
                PROCESS_CREATION_FLAGS(0)
            },
            None,
            PCWSTR::null(),
            &info,
            &mut made,
        )
        .map_err(|e| format!("it could not be started: {}", e.message()))?;
        if let Some(job) = job {
            let in_job = job.assign(made.hProcess);
            // `ResumeThread` answers the count before it, or -1 when it failed.
            let resumed = in_job.is_ok() && ResumeThread(made.hThread) != u32::MAX;
            if !resumed {
                let _ = TerminateProcess(made.hProcess, 1);
                let _ = CloseHandle(made.hThread);
                let _ = CloseHandle(made.hProcess);
                return Err(match in_job {
                    Err(why) => why,
                    Ok(()) => "it could not be let run".to_string(),
                });
            }
        }
        let created = super::procs::created_of(made.hProcess);
        let _ = CloseHandle(made.hThread);
        let _ = CloseHandle(made.hProcess);
        Ok((made.dwProcessId, created))
    }
}
