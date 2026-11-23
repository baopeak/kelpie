//! The session's job object, which the daemon names in `KELPIE_WINDOW_JOB`: the programs `launch` starts are put in it, and a window is
//! in reach when its process is in it. A process a program starts joins the job with it, so reach no longer depends on parent links.

use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, OpenJobObjectW,
};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// `JOB_OBJECT_QUERY`: enough to ask whether a process is in the job.
const QUERY: u32 = 0x0004;

pub struct Job(HANDLE);

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

impl Job {
    /// The named job, made when it is not there (the daemon makes it, with the limits; one made here has none, and lasts while a
    /// program is in it).
    pub fn create(name: &str) -> Result<Job, String> {
        let name = wide(name);
        unsafe { CreateJobObjectW(None, PCWSTR(name.as_ptr())) }
            .map(Job)
            .map_err(|e| format!("the session's job could not be opened: {}", e.message()))
    }

    /// The named job for reading, or `None` when it is not there (nothing was started, or all of it has ended): nothing is then in
    /// reach.
    pub fn open(name: &str) -> Option<Job> {
        let name = wide(name);
        unsafe { OpenJobObjectW(QUERY, false, PCWSTR(name.as_ptr())) }
            .ok()
            .map(Job)
    }

    /// Puts a process in the job. `process` needs the rights `CreateProcessW` gives its own handle.
    pub fn assign(&self, process: HANDLE) -> Result<(), String> {
        unsafe { AssignProcessToJobObject(self.0, process) }
            .map_err(|e| format!("it could not be put in the session's job: {}", e.message()))
    }

    /// Whether the process is in the job; false when it cannot be asked (gone, or not this user's).
    pub fn contains(&self, pid: u32) -> bool {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut inside = BOOL(0);
            let asked = IsProcessInJob(process, Some(self.0), &mut inside).is_ok();
            let _ = CloseHandle(process);
            asked && inside.as_bool()
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
