//! One job object per session that started a program. The daemon makes it with kill-on-close and keeps the handle for the session's
//! life, so that closing the session, or the daemon ending (however it ends: the system closes its handles), ends every program the
//! session started and everything those started. The helper puts each program in it before the program runs, and reaches a window
//! when its process is in it (`kelpie-window`, `KELPIE_WINDOW_JOB`).

/// The job's name for session `sid`: the id, so the helper's reach is the session's, and a random part (`nonce`: letters and digits), so
/// that no other process can guess the name and open or hold the job first. Letters, digits, `-` and `_` only, which is all the helper
/// takes, and at most 108 characters for a session id of 64 (the helper takes 160).
pub fn job_name(sid: &str, nonce: &str) -> String {
    format!("kelpie-win-{sid}-{nonce}")
}

#[cfg(windows)]
mod imp {
    use std::io;
    use windows_sys::Win32::Foundation::{CloseHandle, SetLastError, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };

    /// `ERROR_ALREADY_EXISTS`: `CreateJobObjectW` opened a job of that name that was already there.
    const ALREADY_EXISTS: i32 = 183;

    /// A named job object whose processes end when its last handle closes. The daemon holds the handle.
    pub struct Job(HANDLE);

    // The handle is only used through thread-safe Win32 calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn create(name: &str) -> io::Result<Job> {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            unsafe {
                // A successful call leaves the last error as it was: clear it, so that only this call can say "already there".
                SetLastError(0);
                let handle = CreateJobObjectW(std::ptr::null(), wide.as_ptr());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                // A job of that name that is already there is not this daemon's to limit: the name is random, so it is not expected.
                let existed = io::Error::last_os_error().raw_os_error() == Some(ALREADY_EXISTS);
                let job = Job(handle);
                if existed {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a job of that name is already there"));
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let set = SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if set == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }
    }

    impl Drop for Job {
        /// Closing the last handle ends the processes in the job.
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::io;

    /// Nothing to hold: the window tools run on Windows only, and a test of the rest of the state runs anywhere.
    pub struct Job;

    impl Job {
        pub fn create(_name: &str) -> io::Result<Job> {
            Ok(Job)
        }
    }
}

pub use imp::Job;
