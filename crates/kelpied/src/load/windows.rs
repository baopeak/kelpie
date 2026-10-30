//! What Windows says about the machine's load (see `load`).

use std::mem::size_of;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
use windows_sys::Win32::System::Memory::{CreateMemoryResourceNotification, LowMemoryResourceNotification, QueryMemoryResourceNotification};
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetSystemTimes, GetThreadTimes, OpenThread, ProcessPowerThrottling, SetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, THREAD_QUERY_LIMITED_INFORMATION,
};

use super::Reading;

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

#[derive(Default)]
pub struct Probe {
    /// (idle, total) as of the last reading.
    cpu: Option<(u64, u64)>,
    /// The system's low-memory notification (a `HANDLE`, kept as a number so the probe can move between threads).
    low_memory: Option<isize>,
}

impl Probe {
    pub fn read(&mut self) -> Reading {
        Reading { cpu: self.cpu(), memory_free: memory_free(), low_memory: self.low_memory(), battery_saver: battery_saver(), thermal: false }
    }

    fn cpu(&mut self) -> Option<f64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut idle, mut kernel, mut user) = (zero, zero, zero);
        // SAFETY: three valid out-pointers.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        // Kernel time includes idle time.
        let now = (ticks(idle), ticks(kernel) + ticks(user));
        let before = self.cpu.replace(now)?;
        let (idle, total) = (now.0.saturating_sub(before.0), now.1.saturating_sub(before.1));
        (total > 0).then(|| 100.0 * (1.0 - idle as f64 / total as f64))
    }

    fn low_memory(&mut self) -> bool {
        // SAFETY: creating a notification object takes no pointers.
        let handle = *self.low_memory.get_or_insert_with(|| unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) } as isize);
        if handle == 0 {
            return false;
        }
        let mut low = 0;
        // SAFETY: a handle this probe created and a valid out-pointer.
        unsafe { QueryMemoryResourceNotification(handle as _, &mut low) != 0 && low != 0 }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(handle) = self.low_memory.filter(|h| *h != 0) {
            // SAFETY: the handle was created by this probe and is closed once.
            unsafe { CloseHandle(handle as _) };
        }
    }
}

/// Percent of physical memory available.
fn memory_free() -> Option<f64> {
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `dwLength` is set, as the call requires.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0 && status.ullTotalPhys > 0).then(|| 100.0 * status.ullAvailPhys as f64 / status.ullTotalPhys as f64)
}

/// Windows' battery saver is on.
fn battery_saver() -> bool {
    let mut power: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out-pointer.
    unsafe { GetSystemPowerStatus(&mut power) != 0 && power.SystemStatusFlag & 1 != 0 }
}

/// Windows slows a process it thinks nobody is looking at (EcoQoS) and the daemon has no window:
/// ask not to be, so that its timers and sockets keep their pace when the machine is busy. The
/// process class is left alone: the sessions it starts would inherit a higher one.
pub fn opt_out_of_throttling() {
    let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED, StateMask: 0 };
    // SAFETY: the current process, and a state of the size the call is told.
    let ok = unsafe { SetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling, &state as *const _ as *const _, size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32) };
    if ok == 0 {
        tracing::warn!("could not opt out of power throttling: {}", std::io::Error::last_os_error());
    }
}

/// Nothing to hold on Windows: the opt-out above is for the whole process.
pub struct Activity;

impl Activity {
    pub fn begin() -> Activity {
        Activity
    }
}

/// The five busiest threads of this process by CPU time, as `id:seconds`, for the log when the
/// event loop has stalled (the thread that spins shows up here).
pub fn thread_report() -> String {
    // SAFETY: plain API calls; every handle opened is closed.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return String::new();
        }
        let (pid, mut busy) = (GetCurrentProcessId(), Vec::new());
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, entry.th32ThreadID);
                if !thread.is_null() {
                    let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
                    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
                    if GetThreadTimes(thread, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
                        busy.push((ticks(kernel) + ticks(user), entry.th32ThreadID));
                    }
                    CloseHandle(thread);
                }
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
        busy.sort_unstable_by(|a, b| b.cmp(a));
        busy.iter().take(5).map(|(t, id)| format!("{id}:{:.1}s", *t as f64 / 1e7)).collect::<Vec<_>>().join(" ")
    }
}
