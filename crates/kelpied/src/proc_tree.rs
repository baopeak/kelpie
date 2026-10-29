//! Whole-process-tree ownership and termination.
//!
//! Windows: every session gets its own Job Object (handle owned by the daemon) with
//! `KILL_ON_JOB_CLOSE`, so killing a session — or the daemon dying — never leaves orphaned
//! agent subprocesses. macOS/Unix: portable-pty `setsid()`s the child, so the process group
//! id equals the child pid and `killpg` reaches the whole tree.
//!
//! CPU priority (Windows): Windows gives its foreground boost to Kelpie's window, never to the
//! agents behind it, so on a busy machine a keystroke waits behind everything else at each hop
//! (daemon → console host → agent → console host → daemon). The session on screen therefore
//! runs its agent process and its console host above normal (the agent's own children —
//! builds, tests — stay normal) and the daemon runs above normal. Nothing is ever lowered:
//! measured on a 4-core machine, a shell or agent at below-normal priority printed nothing for
//! 30 s and more while other programs kept the CPU busy. A hidden session only has the
//! daemon's reader of its output yield. Measured with `kelpied bench-echo` (see
//! `echo_bench.rs`). Unix has none of it: an unprivileged process cannot raise a niceness back.

/// Scheduling of one session's processes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    /// No view shows it: normal priority; the daemon's reader of its output yields.
    Background,
    /// The policy is off (or nothing to favour): everything normal.
    Normal,
    /// On screen: the agent process and its console host above normal.
    Foreground,
}

#[cfg(windows)]
mod imp {
    use super::Priority;
    use parking_lot::Mutex;
    use std::io;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicProcessIdList, JobObjectExtendedLimitInformation,
        QueryInformationJobObject, SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_PROCESS_ID_LIST,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetCurrentThread, OpenProcess, SetPriorityClass, SetThreadPriority,
        ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_SET_INFORMATION, PROCESS_SET_QUOTA,
        PROCESS_TERMINATE, THREAD_PRIORITY_LOWEST, THREAD_PRIORITY_NORMAL,
    };

    /// A raised session is raised again this often: a launcher starts the agent's own process
    /// a moment after the session starts.
    const RESWEEP: Duration = Duration::from_secs(5);

    pub struct ProcTree {
        job: HANDLE,
        /// The process the session started (the agent's TUI).
        root: u32,
        /// The session's ConPTY host (OpenConsole.exe / conhost.exe), outside the job.
        console: Option<u32>,
        /// The agent's own executable when `root` only launches it (an npm shim runs
        /// `node codex.js`, which starts `codex.exe`): that child is the one to favour.
        agent_image: Option<String>,
        applied: Mutex<Option<(Priority, Instant)>>,
        /// Set while in the background; the session's output reader yields with it.
        quiet: Arc<AtomicBool>,
    }

    // The job handle is only used through thread-safe Win32 calls.
    unsafe impl Send for ProcTree {}
    unsafe impl Sync for ProcTree {}

    impl ProcTree {
        /// `agent`: the agent's binary name (`codex`), when the session runs one.
        pub fn adopt(pid: u32, console: Option<u32>, agent: Option<&str>) -> io::Result<ProcTree> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if job.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut tree = ProcTree { job, root: pid, console, agent_image: None, applied: Mutex::new(None), quiet: Arc::new(AtomicBool::new(false)) };
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as *const _,
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let process = OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid);
                if process.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let ok = AssignProcessToJobObject(job, process);
                CloseHandle(process);
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                // Looked up only now: anything the root starts before it is in the job escapes
                // kill-on-close, so nothing slow may come first.
                tree.agent_image = agent.map(|a| format!("{}.exe", a.to_ascii_lowercase())).filter(|image| image_of(pid).as_deref() != Some(image.as_str()));
                Ok(tree)
            }
        }

        pub fn kill(&self) {
            unsafe {
                TerminateJobObject(self.job, 1);
            }
        }

        /// Every process in the session's job right now. `None` when the job cannot be asked.
        pub fn pids(&self) -> Option<Vec<u32>> {
            self.pids_from(256)
        }

        /// [`Self::pids`] starting with room for `capacity` ids (a job with more is asked again).
        pub(super) fn pids_from(&self, mut capacity: usize) -> Option<Vec<u32>> {
            let header = std::mem::offset_of!(JOBOBJECT_BASIC_PROCESS_ID_LIST, ProcessIdList);
            let word = std::mem::size_of::<usize>();
            loop {
                // Whole words: two counters, then the ids as an array of pointer-sized integers.
                let mut buf = vec![0usize; header / word + capacity];
                let ok = unsafe { QueryInformationJobObject(self.job, JobObjectBasicProcessIdList, buf.as_mut_ptr().cast(), (buf.len() * word) as u32, std::ptr::null_mut()) };
                let list = buf.as_ptr().cast::<JOBOBJECT_BASIC_PROCESS_ID_LIST>();
                let (assigned, listed) = unsafe { ((*list).NumberOfAssignedProcesses as usize, (*list).NumberOfProcessIdsInList as usize) };
                // A buffer too small fails with ERROR_MORE_DATA after filling itself: ask again with room for all.
                if ok == 0 && assigned > capacity {
                    capacity = assigned + 16;
                    continue;
                }
                if ok == 0 {
                    return None;
                }
                let first = header / word;
                return Some(buf[first..first + listed.min(capacity)].iter().map(|&p| p as u32).collect());
            }
        }

        /// Lets go of the processes still in the job: closing it no longer kills them (a command that opened
        /// an application keeps it).
        pub fn release(&self) {
            unsafe {
                let info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                SetInformationJobObject(self.job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32);
            }
        }

        /// The flag `set_current_thread_quiet` follows in the session's output reader.
        pub fn quiet_flag(&self) -> Arc<AtomicBool> {
            self.quiet.clone()
        }

        /// Applies `p` (again after `RESWEEP` while on screen). Only the agent process (and the
        /// one its launcher started) and the console host change; the rest of the tree — the
        /// agent's builds and tests, a tool that lowered itself — keeps its own priority.
        pub fn set_priority(&self, p: Priority) {
            let mut applied = self.applied.lock();
            let now = Instant::now();
            if matches!(*applied, Some((q, at)) if q == p && (p != Priority::Foreground || now.duration_since(at) < RESWEEP)) {
                return;
            }
            *applied = Some((p, now));
            self.quiet.store(p == Priority::Background, Ordering::Relaxed);
            let class = if p == Priority::Foreground { ABOVE_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS };
            let mut agents = vec![self.root];
            if let Some(image) = &self.agent_image {
                agents.extend(children_of(self.root, &[image.as_str()]));
            }
            for &pid in agents.iter().chain(self.console.iter()) {
                set_class(pid, class);
            }
        }
    }

    fn set_class(pid: u32, class: u32) {
        unsafe {
            let process = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
            if process.is_null() {
                return;
            }
            SetPriorityClass(process, class);
            CloseHandle(process);
        }
    }

    /// The daemon sits on every keystroke's path: above normal while the policy is on. Its
    /// children still start at normal (only idle and below-normal classes are inherited).
    pub fn set_daemon_priority(raised: bool) {
        unsafe {
            SetPriorityClass(GetCurrentProcess(), if raised { ABOVE_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
        }
    }

    /// For a background session's output reader: in a daemon raised above normal, parsing its
    /// output then competes with other programs as an equal, not ahead of them.
    pub fn set_current_thread_quiet(quiet: bool) {
        unsafe {
            SetThreadPriority(GetCurrentThread(), if quiet { THREAD_PRIORITY_LOWEST } else { THREAD_PRIORITY_NORMAL });
        }
    }

    /// Calls `f(pid, parent pid, lower-case image name)` for every process.
    fn each_process(mut f: impl FnMut(u32, u32, &str)) {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return;
            }
            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut entry) != 0;
            while ok {
                let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(entry.szExeFile.len());
                let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_ascii_lowercase();
                f(entry.th32ProcessID, entry.th32ParentProcessID, &name);
                ok = Process32NextW(snap, &mut entry) != 0;
            }
            CloseHandle(snap);
        }
    }

    fn children_of(parent: u32, images: &[&str]) -> Vec<u32> {
        let mut out = Vec::new();
        each_process(|pid, ppid, name| {
            if ppid == parent && images.contains(&name) {
                out.push(pid);
            }
        });
        out
    }

    #[cfg(test)]
    pub fn children_for_test(parent: u32, image: &str) -> Vec<u32> {
        children_of(parent, &[image])
    }

    fn image_of(pid: u32) -> Option<String> {
        let mut found = None;
        each_process(|p, _, name| {
            if p == pid {
                found = Some(name.to_string());
            }
        });
        found
    }

    /// Console hosts (OpenConsole.exe / conhost.exe) this daemon started. Compared before and
    /// after creating a pseudoconsole, the new one is that session's.
    pub fn console_hosts() -> Vec<u32> {
        children_of(unsafe { GetCurrentProcessId() }, &["openconsole.exe", "conhost.exe"])
    }

    impl Drop for ProcTree {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.job);
            }
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::Priority;
    use std::io;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    pub struct ProcTree {
        pgid: i32,
        released: std::sync::atomic::AtomicBool,
    }

    impl ProcTree {
        pub fn adopt(pid: u32, _console: Option<u32>, _agent: Option<&str>) -> io::Result<ProcTree> {
            Ok(ProcTree { pgid: pid as i32, released: std::sync::atomic::AtomicBool::new(false) })
        }

        /// SIGHUP+SIGTERM now, SIGKILL after a grace period for anything still alive.
        pub fn kill(&self) {
            let pgid = self.pgid;
            unsafe {
                libc::killpg(pgid, libc::SIGHUP);
                libc::killpg(pgid, libc::SIGTERM);
            }
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            });
        }

        /// Lets go of the group: dropping this no longer hangs it up.
        pub fn release(&self) {
            self.released.store(true, std::sync::atomic::Ordering::Relaxed);
        }

        /// There is no list to ask for: the processes are found by walking parents from `root`.
        pub fn pids(&self) -> Option<Vec<u32>> {
            None
        }

        pub fn quiet_flag(&self) -> Arc<AtomicBool> {
            Arc::default()
        }

        /// See the module docs: niceness could not be restored once the session is shown again.
        pub fn set_priority(&self, _p: Priority) {}
    }

    pub fn set_daemon_priority(_raised: bool) {}

    pub fn set_current_thread_quiet(_quiet: bool) {}

    pub fn console_hosts() -> Vec<u32> {
        Vec::new()
    }

    /// Like the Windows job's kill-on-close: whatever the session left running goes too.
    impl Drop for ProcTree {
        fn drop(&mut self) {
            if !self.released.load(std::sync::atomic::Ordering::Relaxed) {
                unsafe {
                    libc::killpg(self.pgid, libc::SIGHUP);
                }
            }
        }
    }
}

pub use imp::{console_hosts, set_current_thread_quiet, set_daemon_priority, ProcTree};
#[cfg(all(test, windows))]
pub use imp::children_for_test;

#[cfg(all(test, windows))]
mod tests {
    use super::{Priority, ProcTree};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        GetPriorityClass, OpenProcess, ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    fn class_of(pid: u32) -> u32 {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            assert!(!h.is_null());
            let c = GetPriorityClass(h);
            CloseHandle(h);
            c
        }
    }

    #[test]
    fn only_the_shown_agent_is_raised_and_nothing_is_lowered() {
        // cmd (the root) starts a long ping after a second, by then inside the job.
        let mut child = std::process::Command::new("cmd.exe").args(["/c", "ping -n 2 127.0.0.1 >nul & ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
        let root = child.id();
        let tree = ProcTree::adopt(root, None, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1800));
        let kids = super::imp::children_for_test(root, "ping.exe");
        assert!(!kids.is_empty(), "ping runs under cmd");
        // On some CI machines the test runs below normal and its children inherit that,
        // so "left alone" means keeping the class they started with.
        let started: Vec<u32> = kids.iter().map(|&k| class_of(k)).collect();
        tree.set_priority(Priority::Foreground);
        assert_eq!(class_of(root), ABOVE_NORMAL_PRIORITY_CLASS);
        // The agent's children (builds, tests) are never raised…
        for (&k, &class) in kids.iter().zip(&started) {
            assert_eq!(class_of(k), class);
        }
        // …and out of sight nothing goes below normal.
        tree.set_priority(Priority::Background);
        assert_eq!(class_of(root), NORMAL_PRIORITY_CLASS);
        for (&k, &class) in kids.iter().zip(&started) {
            assert_eq!(class_of(k), class);
        }
        tree.set_priority(Priority::Normal);
        assert_eq!(class_of(root), NORMAL_PRIORITY_CLASS);
        tree.kill();
        let _ = child.wait();
    }

    #[test]
    fn the_job_lists_the_processes_in_it() {
        // cmd (the root) starts a long ping after a second, by then inside the job.
        let mut child = std::process::Command::new("cmd.exe").args(["/c", "ping -n 2 127.0.0.1 >nul & ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
        let root = child.id();
        let tree = ProcTree::adopt(root, None, None).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1800));
        let kids = super::imp::children_for_test(root, "ping.exe");
        assert!(!kids.is_empty(), "ping runs under cmd");
        let pids = tree.pids().expect("the job can be asked");
        assert!(pids.contains(&root) && kids.iter().all(|k| pids.contains(k)), "{pids:?} vs {root} {kids:?}");
        // The test run itself is not in the job.
        assert!(!pids.contains(&std::process::id()));
        // A list that does not fit the first buffer is asked for again.
        let mut again = tree.pids_from(1).expect("asked again with room for all");
        let mut all = pids.clone();
        again.sort_unstable();
        all.sort_unstable();
        assert_eq!(again, all);
        tree.kill();
        let _ = child.wait();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(tree.pids().expect("an empty job can be asked").is_empty());
    }

    #[test]
    fn a_launcher_child_counts_as_the_agent() {
        // cmd launches ping.exe, as a node shim launches codex.exe.
        let mut child = std::process::Command::new("cmd.exe").args(["/c", "ping -n 30 127.0.0.1 >nul"]).spawn().unwrap();
        let root = child.id();
        // On a busy machine cmd may take a while to start ping.
        let mut agent = Vec::new();
        for _ in 0..50 {
            agent = super::imp::children_for_test(root, "ping.exe");
            if !agent.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let tree = ProcTree::adopt(root, None, Some("PING")).unwrap();
        assert!(!agent.is_empty(), "ping is the launcher's child");
        tree.set_priority(Priority::Foreground);
        for &a in &agent {
            assert_eq!(class_of(a), ABOVE_NORMAL_PRIORITY_CLASS);
        }
        tree.set_priority(Priority::Background);
        for &a in &agent {
            assert_eq!(class_of(a), NORMAL_PRIORITY_CLASS);
        }
        tree.kill();
        let _ = child.wait();
    }
}
