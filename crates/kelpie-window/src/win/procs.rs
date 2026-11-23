//! The running processes as `scope` wants them: id, parent id, creation time.

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::scope::Proc;

/// When the process behind an open handle was created; 0 when it cannot be read. While the handle is open the process id is not given
/// to another process, so this and the id name the same process.
pub fn created_of(p: HANDLE) -> u64 {
    unsafe {
        let (mut made, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if GetProcessTimes(p, &mut made, &mut exit, &mut kernel, &mut user).is_ok() {
            (u64::from(made.dwHighDateTime) << 32) | u64::from(made.dwLowDateTime)
        } else {
            0
        }
    }
}

/// When a process was created, as one number; 0 when it cannot be read (gone, or another user's).
pub fn created(pid: u32) -> u64 {
    unsafe {
        let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return 0;
        };
        let (mut made, mut exit, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        let ok = GetProcessTimes(p, &mut made, &mut exit, &mut kernel, &mut user).is_ok();
        let _ = CloseHandle(p);
        if ok {
            (u64::from(made.dwHighDateTime) << 32) | u64::from(made.dwLowDateTime)
        } else {
            0
        }
    }
}

/// The path of a process's program, or `None` when it cannot be read (gone, or another user's).
pub fn image_of(pid: u32) -> Option<String> {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let got = QueryFullProcessImageNameW(
            p,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(p);
        got.ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

pub fn processes() -> Vec<Proc> {
    let mut out = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return out;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut e).is_ok() {
            loop {
                out.push(Proc {
                    pid: e.th32ProcessID,
                    parent: e.th32ParentProcessID,
                    created: created(e.th32ProcessID),
                });
                if Process32NextW(snap, &mut e).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    out
}
