//! What a session runs, and stopping it safely.
//!
//! A process is known by its identity: (pid, start time, executable path). A PID alone is not
//! one: the system hands a dead process's number to the next one it starts. Every stop re-reads the
//! identity of the process it is about to act on and refuses when it is not the one that was listed.
//!
//! The tree of a session is the descendants of the process the session started (walking parents, a parent that
//! started after its child being a reused PID) and, on Windows, whatever else is in its Job Object (asked
//! directly): a shell may start programs outside its job (a packaged PowerShell does). Every process seen in a tree
//! is remembered by identity. One that is still running but no longer in the tree — it broke away from the job, or
//! its parent died and it outlived the session's group — is an orphan and is listed as such; one
//! whose identity no longer matches (it ended and its PID was reused) is dropped and never touched.
//!
//! Nothing here runs by itself: the app asks (`process.*`), a scan answers. A scan reads the table of processes
//! once (on Windows one `NtQuerySystemInformation` call: parents, start times, memory and CPU time of all of them).

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

use crate::proc_tree::ProcTree;

/// How long a process is given to end on its own before it is forced.
pub const GRACE: Duration = Duration::from_secs(3);

/// Who a process is. Two readings are the same process only if all three agree.
#[derive(Clone, Debug, Serialize)]
pub struct Identity {
    pub pid: u32,
    /// Seconds since the epoch on Unix; milliseconds on Windows.
    pub start_time: u64,
    pub exe: Option<String>,
}

impl Identity {
    pub fn same(&self, other: &Identity) -> bool {
        self.pid == other.pid && self.start_time == other.start_time && same_exe(&self.exe, &other.exe)
    }
}

fn same_exe(a: &Option<String>, b: &Option<String>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) if cfg!(windows) => a.eq_ignore_ascii_case(b),
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Proc {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub name: String,
    pub start_time: u64,
    pub exe: Option<String>,
    /// Resident memory, bytes.
    pub mem: u64,
    /// Percent of one core since the previous scan of this process (0 on the first).
    pub cpu: f32,
}

impl Proc {
    pub fn identity(&self) -> Identity {
        Identity { pid: self.pid, start_time: self.start_time, exe: self.exe.clone() }
    }
}

/// The processes as one scan read them, by pid. On Windows without their executable paths: those are read from
/// the processes themselves, for the ones a scan looks at (see [`proc_of`]).
type Table = HashMap<u32, Proc>;

/// A session's process, as the scanner needs to see it.
#[derive(Clone)]
pub struct Target {
    pub id: String,
    /// The process the session started, while it runs.
    pub root: Option<u32>,
    pub tree: Option<Arc<ProcTree>>,
    /// The agent's binary name, when the session runs one (its launcher may be the root).
    pub agent: Option<String>,
}

/// One session's processes at one moment.
pub struct TreeScan {
    pub id: String,
    pub root: Option<Proc>,
    /// Everything in the tree except the root, parents before their children.
    pub children: Vec<Proc>,
    pub orphans: Vec<Proc>,
    /// Remembered processes that ended or whose PID was reused, forgotten by this scan.
    pub dropped: usize,
    /// The root, and the agent when a launcher is the root: "Kill children" leaves them.
    pub keep: HashSet<u32>,
}

#[derive(Default)]
struct Tracked {
    root: Option<Identity>,
    seen: HashMap<u32, Identity>,
}

#[derive(Default)]
pub struct Procs {
    /// Reads the table of processes (and keeps what CPU use is measured against).
    reader: Mutex<os::Reader>,
    tracked: Mutex<HashMap<String, Tracked>>,
    /// Scans made since the daemon started: the app triggers them, so this stays put when nothing asks.
    scans: AtomicU64,
}

#[derive(Default, Debug, Serialize)]
pub struct KillReport {
    /// Ended on their own after being asked.
    pub closed: Vec<u32>,
    /// Still running after the grace period: forced.
    pub forced: Vec<u32>,
    /// Already gone when the stop came.
    pub gone: Vec<u32>,
    pub refused: Vec<Refused>,
}

#[derive(Debug, Serialize)]
pub struct Refused {
    pub pid: u32,
    pub reason: String,
}

pub struct KillRequest {
    pub pid: u32,
    pub start_time: u64,
    pub exe: Option<String>,
}

/// `pid` as the table has it, with its identity read now. A PID taken by another process since the table was
/// read is not this process.
fn proc_of(table: &Table, pid: u32) -> Option<Proc> {
    let p = table.get(&pid)?;
    let id = os::identity(table, pid)?;
    (id.start_time == p.start_time).then(|| Proc { exe: id.exe, ..p.clone() })
}

/// Remembered `seen` entries against what is running: those still running as the same process but outside the tree are orphans;
/// those that ended, or whose PID now belongs to another process, are removed. Returns (orphan pids, removed count).
fn reconcile(seen: &mut HashMap<u32, Identity>, running: &HashMap<u32, Proc>, members: &HashSet<u32>) -> (Vec<u32>, usize) {
    let stale: Vec<u32> = seen.iter().filter(|(pid, id)| !running.get(pid).is_some_and(|p| p.identity().same(id))).map(|(pid, _)| *pid).collect();
    for pid in &stale {
        seen.remove(pid);
    }
    (seen.keys().copied().filter(|pid| !members.contains(pid)).collect(), stale.len())
}

/// The root, and — when a launcher is the root — the agent's own process under it: the one named like the agent
/// (`node codex.js` runs `codex`), or whatever a batch file (`claude.cmd`, run by `cmd`) starts directly.
fn keep_set(root: Option<&Proc>, children: &[Proc], agent: Option<&str>) -> HashSet<u32> {
    let Some(root) = root else { return HashSet::new() };
    let mut keep = HashSet::from([root.pid]);
    if let Some(agent) = agent {
        let stem = |name: &str| name.rsplit(['/', '\\']).next().unwrap_or(name).trim_end_matches(".exe").trim_end_matches(".EXE").to_ascii_lowercase();
        let agent = agent.to_ascii_lowercase();
        let root_stem = stem(&root.name);
        if root_stem != agent {
            let batch = root_stem == "cmd";
            keep.extend(children.iter().filter(|c| c.ppid == Some(root.pid) && (batch || stem(&c.name) == agent)).map(|c| c.pid));
        }
    }
    keep
}

impl Procs {
    pub fn scans(&self) -> u64 {
        self.scans.load(Ordering::Relaxed)
    }

    /// Forgets the sessions that are gone.
    pub fn retain_sessions(&self, ids: &HashSet<String>) {
        self.tracked.lock().retain(|id, _| ids.contains(id));
    }

    /// Looks at `targets`: each session's processes (the descendants of its process, and its job), and what it
    /// ran that is still running outside them. The ports are for the caller to add.
    pub fn scan(&self, targets: &[Target]) -> Vec<TreeScan> {
        self.scans.fetch_add(1, Ordering::Relaxed);
        let mut reader = self.reader.lock();
        let mut tracked = self.tracked.lock();
        // Walking a tree needs the parent of every process. Without a session process to walk from, what the
        // jobs hold and what is remembered are all there is to read.
        let table = if targets.iter().any(|t| t.root.is_some()) {
            reader.read(None)
        } else {
            let mut wanted: Vec<u32> = Vec::new();
            for t in targets {
                wanted.extend(tracked.get(&t.id).into_iter().flat_map(|tr| tr.seen.keys().copied()));
                wanted.extend(t.tree.as_ref().and_then(|tree| tree.pids()).into_iter().flatten());
            }
            wanted.sort_unstable();
            wanted.dedup();
            reader.read(Some(&wanted))
        };
        drop(reader);
        let rows: Vec<(u32, Option<u32>, u64)> = table.values().map(|p| (p.pid, p.ppid, p.start_time)).collect();
        let children_of = kelpie_proto::procs::child_map(&rows);
        targets
            .iter()
            .map(|t| {
                let tr = tracked.entry(t.id.clone()).or_default();
                // A root whose PID now belongs to another process is not the session's.
                let root = t.root.and_then(|pid| proc_of(&table, pid)).filter(|r| tr.root.as_ref().is_none_or(|known| known.same(&r.identity())));
                if let Some(r) = &root {
                    tr.root.get_or_insert_with(|| r.identity());
                }
                // Who is in the tree: the descendants of the session's process, and the members of its job.
                let mut member_pids = root.as_ref().map(|r| kelpie_proto::procs::descendants(&children_of, r.pid)).unwrap_or_default();
                for pid in t.tree.as_ref().and_then(|tree| tree.pids()).into_iter().flatten() {
                    if Some(pid) != root.as_ref().map(|r| r.pid) && !member_pids.contains(&pid) {
                        member_pids.push(pid);
                    }
                }
                let in_tree: HashSet<u32> = member_pids.iter().copied().chain(root.iter().map(|r| r.pid)).collect();
                let mut running: HashMap<u32, Proc> = HashMap::new();
                for pid in member_pids.iter().chain(tr.seen.keys()) {
                    if let Some(p) = proc_of(&table, *pid) {
                        running.insert(*pid, p);
                    }
                }
                // What is remembered is judged before this scan adds to it.
                let (orphan_pids, dropped) = reconcile(&mut tr.seen, &running, &in_tree);
                let mut children: Vec<Proc> = member_pids.iter().filter_map(|pid| running.get(pid).cloned()).collect();
                for c in &children {
                    tr.seen.insert(c.pid, c.identity());
                }
                children.sort_by_key(|c| (c.start_time, c.pid));
                let mut orphans: Vec<Proc> = orphan_pids.iter().filter_map(|pid| running.get(pid).cloned()).collect();
                orphans.sort_by_key(|c| (c.start_time, c.pid));
                let keep = keep_set(root.as_ref(), &children, t.agent.as_deref());
                TreeScan { id: t.id.clone(), root, children, orphans, dropped, keep }
            })
            .collect()
    }

    /// The process a port belongs to, with the session that has it in its tree when one does.
    pub fn describe(&self, pid: u32) -> Option<(Proc, Option<String>)> {
        let table = self.reader.lock().read(Some(&[pid]));
        let proc = proc_of(&table, pid)?;
        let session = self.tracked.lock().iter().find(|(_, tr)| tr.seen.get(&pid).is_some_and(|id| id.same(&proc.identity()))).map(|(id, _)| id.clone());
        Some((proc, session))
    }

    /// Stops `req.pid` and what runs under it, but only when it is a process of this session (in its tree,
    /// or remembered as having left it) and is still the process that was listed.
    pub fn kill(&self, t: &Target, req: &KillRequest, grace: Duration) -> Result<KillReport, String> {
        if req.pid == std::process::id() {
            return Err("that is the Kelpie daemon".into());
        }
        let scan = self.scan(std::slice::from_ref(t)).pop().expect("one target, one scan");
        if scan.root.as_ref().is_some_and(|r| r.pid == req.pid) {
            return Err("that is the session's own process: stop the session instead".into());
        }
        let orphan = scan.orphans.iter().find(|p| p.pid == req.pid);
        let proc = scan.children.iter().find(|p| p.pid == req.pid).or(orphan).ok_or("not a process of this session (it may have ended)")?;
        if proc.start_time != req.start_time {
            return Err("the process with that id started at another time: it is not the one that was listed".into());
        }
        if req.exe.is_some() && !same_exe(&req.exe, &proc.exe) {
            return Err("the process with that id runs another program: it is not the one that was listed".into());
        }
        // The process and what it started, deepest first. An orphan stands alone (its children are listed on their own).
        let mut victims = vec![proc.clone()];
        if orphan.is_none() {
            let rows: Vec<(u32, Option<u32>, u64)> = scan.children.iter().map(|c| (c.pid, c.ppid, c.start_time)).collect();
            let below = kelpie_proto::procs::descendants(&kelpie_proto::procs::child_map(&rows), proc.pid);
            victims.extend(below.iter().filter_map(|pid| scan.children.iter().find(|c| c.pid == *pid).cloned()));
            victims.reverse();
        }
        Ok(terminate(&victims, grace))
    }

    /// Stops everything the session started except the session's own process (and its agent).
    pub fn kill_children(&self, t: &Target, grace: Duration) -> KillReport {
        let scan = self.scan(std::slice::from_ref(t)).pop().expect("one target, one scan");
        let mut victims: Vec<Proc> = scan.children.iter().filter(|c| !scan.keep.contains(&c.pid)).cloned().collect();
        victims.reverse();
        terminate(&victims, grace)
    }
}

/// Stops `victims` (each re-checked against the identity it was listed with, right before it is acted on):
/// asks them to end, forces the ones still running after `grace`.
fn terminate(victims: &[Proc], grace: Duration) -> KillReport {
    let victims: Vec<Identity> = victims.iter().map(Proc::identity).collect();
    os::terminate(&victims, grace)
}

#[cfg(windows)]
mod os {
    use super::{Identity, KillReport, Proc, Refused, Table};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    use windows_sys::Wdk::System::SystemInformation::{NtQuerySystemInformation, SystemProcessInformation};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, FILETIME, HANDLE, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE, PROCESS_SYNCHRONIZE,
    };
    use windows_sys::Win32::System::WindowsProgramming::SYSTEM_PROCESS_INFORMATION;
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};

    /// An open process. While it is held its PID cannot be given to another process, so what was
    /// checked through it is still what is stopped through it.
    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    enum Opened {
        Gone,
        Denied(String),
    }

    const ERROR_INVALID_PARAMETER: u32 = 87;
    const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

    fn open(pid: u32, access: u32) -> Result<Handle, Opened> {
        let h = unsafe { OpenProcess(access, 0, pid) };
        if !h.is_null() {
            return Ok(Handle(h));
        }
        match unsafe { GetLastError() } {
            // No process has that id.
            ERROR_INVALID_PARAMETER => Err(Opened::Gone),
            code => Err(Opened::Denied(format!("cannot open the process (error {code})"))),
        }
    }

    /// 100 ns ticks since 1601 -> ms since 1970.
    fn epoch_ms(ticks: u64) -> u64 {
        (ticks / 10_000).saturating_sub(11_644_473_600_000)
    }

    /// Start and image path, read from the handle.
    fn identity_of(h: &Handle, pid: u32) -> Option<Identity> {
        unsafe {
            let (mut created, mut exited, mut kernel, mut user): (FILETIME, FILETIME, FILETIME, FILETIME) =
                (std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed(), std::mem::zeroed());
            if GetProcessTimes(h.0, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
                return None;
            }
            let start_time = epoch_ms((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime));
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let exe = (QueryFullProcessImageNameW(h.0, 0, buf.as_mut_ptr(), &mut len) != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]));
            Some(Identity { pid, start_time, exe })
        }
    }

    pub fn identity_of_pid(pid: u32) -> Option<Identity> {
        identity_of(&open(pid, PROCESS_QUERY_LIMITED_INFORMATION).ok()?, pid)
    }

    /// The identity of `pid` now, from the process itself (the table has no executable paths).
    pub fn identity(_table: &Table, pid: u32) -> Option<Identity> {
        identity_of_pid(pid)
    }

    /// Reads the table of processes. Every call reads all of them (there is no cheaper way to ask for a few).
    #[derive(Default)]
    pub struct Reader {
        /// Whole words, so every entry the system writes in it is aligned.
        buf: Vec<u64>,
        /// pid -> (start, CPU time in 100 ns, when): the previous reading, for CPU use.
        cpu: HashMap<u32, (u64, u64, Instant)>,
    }

    impl Reader {
        pub fn read(&mut self, _wanted: Option<&[u32]>) -> Table {
            if self.buf.is_empty() {
                self.buf = vec![0; 64 * 1024];
            }
            let mut filled = 0u32;
            let mut ok = false;
            for _ in 0..4 {
                let bytes = (self.buf.len() * 8) as u32;
                let status = unsafe { NtQuerySystemInformation(SystemProcessInformation, self.buf.as_mut_ptr().cast(), bytes, &mut filled) };
                if status == STATUS_INFO_LENGTH_MISMATCH {
                    // Processes start between the calls: ask again with room to spare.
                    self.buf = vec![0; (filled as usize + 64 * 1024) / 8];
                    continue;
                }
                ok = status >= 0;
                break;
            }
            if !ok {
                return Table::new();
            }
            let now = Instant::now();
            let base = self.buf.as_ptr().cast::<u8>();
            let end = (filled as usize).min(self.buf.len() * 8);
            let mut table = Table::new();
            let mut cpu = HashMap::new();
            let mut at = 0usize;
            while at + std::mem::size_of::<SYSTEM_PROCESS_INFORMATION>() <= end {
                // SAFETY: the system lays the entries out one after another in the buffer it filled, each aligned,
                // `NextEntryOffset` bytes apart; the image name points into that same buffer.
                let e = unsafe { &*base.add(at).cast::<SYSTEM_PROCESS_INFORMATION>() };
                // Not in the documented part of the layout: create, user and kernel time are its 4th, 5th and 6th 64-bit values.
                let time = |i: usize| u64::from_le_bytes(e.Reserved1[i * 8..i * 8 + 8].try_into().unwrap());
                let (pid, ppid) = (e.UniqueProcessId as usize as u32, e.Reserved2 as usize as u32);
                let name = if e.ImageName.Buffer.is_null() {
                    String::new()
                } else {
                    String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(e.ImageName.Buffer, usize::from(e.ImageName.Length) / 2) })
                };
                let (start_time, cpu_time) = (epoch_ms(time(3)), time(4) + time(5));
                let used = match self.cpu.get(&pid) {
                    Some(&(start, before, then)) if start == start_time && cpu_time >= before => {
                        let wall = now.duration_since(then).as_secs_f64();
                        if wall > 0.0 { ((cpu_time - before) as f64 / 1e7 / wall * 100.0) as f32 } else { 0.0 }
                    }
                    _ => 0.0,
                };
                cpu.insert(pid, (start_time, cpu_time, now));
                table.insert(pid, Proc { pid, ppid: (pid != 0).then_some(ppid), name, start_time, exe: None, mem: e.WorkingSetSize as u64, cpu: used });
                if e.NextEntryOffset == 0 {
                    break;
                }
                at += e.NextEntryOffset as usize;
            }
            self.cpu = cpu;
            table
        }
    }

    fn running(h: &Handle) -> bool {
        unsafe { WaitForSingleObject(h.0, 0) == WAIT_TIMEOUT }
    }

    struct Windows {
        pid: u32,
        asked: bool,
    }

    unsafe extern "system" fn close_window(hwnd: HANDLE, param: isize) -> i32 {
        let w = &mut *(param as *mut Windows);
        let mut owner = 0u32;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner == w.pid {
            w.asked |= PostMessageW(hwnd, WM_CLOSE, 0, 0) != 0;
        }
        1
    }

    /// Asks the process to close its windows (a console program has none and is not asked anything).
    fn ask_to_close(pid: u32) -> bool {
        let mut w = Windows { pid, asked: false };
        unsafe {
            EnumWindows(Some(close_window), &mut w as *mut Windows as isize);
        }
        w.asked
    }

    pub fn terminate(victims: &[Identity], grace: Duration) -> KillReport {
        let mut report = KillReport::default();
        let mut held: Vec<(&Identity, Handle)> = Vec::new();
        for v in victims {
            let refuse = |report: &mut KillReport, reason: &str| report.refused.push(Refused { pid: v.pid, reason: reason.into() });
            let h = match open(v.pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE) {
                Ok(h) => h,
                Err(Opened::Gone) => {
                    report.gone.push(v.pid);
                    continue;
                }
                Err(Opened::Denied(why)) => {
                    refuse(&mut report, &why);
                    continue;
                }
            };
            // Judged through the open handle: this is the process that will be stopped.
            if !identity_of(&h, v.pid).is_some_and(|now| now.same(v)) {
                refuse(&mut report, "the process changed: it is not the one that was listed");
            } else if !running(&h) {
                report.gone.push(v.pid);
            } else {
                held.push((v, h));
            }
        }
        // Only a program with windows can be asked; the rest has no gentler way to end than being terminated.
        let asked = held.iter().fold(false, |any, (v, _)| ask_to_close(v.pid) | any);
        if asked {
            let deadline = Instant::now() + grace;
            while Instant::now() < deadline && held.iter().any(|(_, h)| running(h)) {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        for (v, h) in &held {
            if !running(h) {
                report.closed.push(v.pid);
                continue;
            }
            unsafe {
                TerminateProcess(h.0, 1);
                WaitForSingleObject(h.0, 2000);
            }
            report.forced.push(v.pid);
        }
        report
    }
}

#[cfg(unix)]
mod os {
    use super::{Identity, KillReport, Proc, Refused, Table};
    use std::time::{Duration, Instant};
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System, UpdateKind};

    fn refresh_kind() -> ProcessRefreshKind {
        ProcessRefreshKind::nothing().with_memory().with_cpu().with_exe(UpdateKind::OnlyIfNotSet)
    }

    fn live(p: &sysinfo::Process) -> bool {
        !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead)
    }

    pub fn identity(table: &Table, pid: u32) -> Option<Identity> {
        table.get(&pid).map(Proc::identity)
    }

    /// Reads the table of processes: all of them, or the ones asked for.
    #[derive(Default)]
    pub struct Reader {
        sys: System,
    }

    impl Reader {
        pub fn read(&mut self, wanted: Option<&[u32]>) -> Table {
            match wanted {
                None => self.sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind()),
                Some(w) => {
                    let pids: Vec<Pid> = w.iter().copied().map(Pid::from_u32).collect();
                    self.sys.refresh_processes_specifics(ProcessesToUpdate::Some(&pids), true, refresh_kind())
                }
            };
            self.sys
                .processes()
                .iter()
                .filter(|(pid, p)| live(p) && wanted.is_none_or(|w| w.contains(&pid.as_u32())))
                .map(|(pid, p)| {
                    let pid = pid.as_u32();
                    let exe = p.exe().map(|e| e.to_string_lossy().into_owned());
                    (pid, Proc { pid, ppid: p.parent().map(|p| p.as_u32()), name: p.name().to_string_lossy().into_owned(), start_time: p.start_time(), exe, mem: p.memory(), cpu: p.cpu_usage() })
                })
                .collect()
        }
    }

    /// Whether `v` is still running as the same process: `Some(true)`, `Some(false)` when its PID
    /// is another process's now, `None` when it has ended.
    fn check(sys: &mut System, v: &Identity) -> Option<bool> {
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[Pid::from_u32(v.pid)]), true, refresh_kind());
        let p = sys.process(Pid::from_u32(v.pid)).filter(|p| live(p))?;
        let now = Identity { pid: v.pid, start_time: p.start_time(), exe: p.exe().map(|e| e.to_string_lossy().into_owned()) };
        Some(now.same(v))
    }

    pub fn terminate(victims: &[Identity], grace: Duration) -> KillReport {
        let mut report = KillReport::default();
        let mut sys = System::new();
        let mut asked: Vec<&Identity> = Vec::new();
        for v in victims {
            match check(&mut sys, v) {
                None => report.gone.push(v.pid),
                Some(false) => report.refused.push(Refused { pid: v.pid, reason: "the process changed: it is not the one that was listed".into() }),
                Some(true) => {
                    unsafe {
                        libc::kill(v.pid as i32, libc::SIGTERM);
                    }
                    asked.push(v);
                }
            }
        }
        let deadline = Instant::now() + grace;
        let mut waiting: Vec<&Identity> = asked.clone();
        while !waiting.is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
            waiting.retain(|v| check(&mut sys, v) == Some(true));
        }
        for v in asked {
            // Judged again: the grace period is long enough for the number to have been reused.
            match check(&mut sys, v) {
                Some(true) => {
                    unsafe {
                        libc::kill(v.pid as i32, libc::SIGKILL);
                    }
                    report.forced.push(v.pid);
                    let end = Instant::now() + Duration::from_secs(2);
                    while Instant::now() < end && check(&mut sys, v) == Some(true) {
                        std::thread::sleep(Duration::from_millis(25));
                    }
                }
                _ => report.closed.push(v.pid),
            }
        }
        report
    }
}

/// Real processes the tests here and the daemon's start (nothing is mocked); each test ends what it started.
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Child, Command, Stdio};
    use std::time::Instant;
    use sysinfo::{Pid, ProcessRefreshKind, ProcessStatus, ProcessesToUpdate, System};

    /// A program that sits there for a minute, as its own process (and group, so stopping it never touches the test run).
    pub(crate) fn sleeper() -> Child {
        let mut cmd;
        #[cfg(windows)]
        {
            cmd = Command::new("ping");
            cmd.args(["-n", "60", "127.0.0.1"]);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd = Command::new("sleep");
            cmd.arg("60").process_group(0);
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("start a sleeper")
    }

    /// A launcher that waits for a line on its input, then runs two long programs one after the other.
    /// Told to go on only once it is in its job, so what it runs is in the job too.
    pub(crate) fn launcher() -> Child {
        let mut cmd;
        #[cfg(windows)]
        {
            cmd = Command::new("cmd");
            cmd.args(["/c", "set /p x=& ping -n 60 127.0.0.1 >nul & ping -n 60 127.0.0.1 >nul"]);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd = Command::new("sh");
            cmd.args(["-c", "read x; sleep 60; sleep 60"]).process_group(0);
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("start a launcher")
    }

    pub(crate) fn alive(pid: u32) -> bool {
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&[Pid::from_u32(pid)]), true, ProcessRefreshKind::nothing());
        sys.process(Pid::from_u32(pid)).is_some_and(|p| !matches!(p.status(), ProcessStatus::Zombie | ProcessStatus::Dead))
    }

    /// The launcher, in a job of its own (on Windows), told to go on.
    pub(crate) fn session(id: &str) -> (Child, Target) {
        let mut root = launcher();
        let tree = ProcTree::adopt(root.id(), None, None).ok().map(Arc::new);
        let target = Target { id: id.into(), root: Some(root.id()), tree, agent: None };
        root.stdin.take().expect("piped").write_all(b"\n").expect("tell the launcher to go on");
        (root, target)
    }

    fn wait_for_children(procs: &Procs, t: &Target) -> TreeScan {
        let end = Instant::now() + Duration::from_secs(15);
        loop {
            let scan = procs.scan(std::slice::from_ref(t)).pop().unwrap();
            if !scan.children.is_empty() || Instant::now() > end {
                return scan;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    pub(crate) fn reap(mut c: Child) {
        let _ = c.kill();
        let _ = c.wait();
    }

    fn id(pid: u32, start: u64, exe: &str) -> Identity {
        Identity { pid, start_time: start, exe: Some(exe.into()) }
    }

    fn running(pid: u32, start: u64, exe: &str) -> Proc {
        Proc { pid, ppid: None, name: "x".into(), start_time: start, exe: Some(exe.into()), mem: 0, cpu: 0.0 }
    }

    #[test]
    fn identity_needs_all_three_to_agree() {
        let a = id(7, 100, "/bin/node");
        assert!(a.same(&id(7, 100, "/bin/node")));
        assert!(!a.same(&id(8, 100, "/bin/node")));
        assert!(!a.same(&id(7, 101, "/bin/node")));
        assert!(!a.same(&id(7, 100, "/bin/python")));
        assert!(!a.same(&Identity { exe: None, ..id(7, 100, "") }));
        assert!(Identity { exe: None, ..a.clone() }.same(&Identity { exe: None, ..a }));
    }

    #[test]
    fn remembered_processes_are_orphans_only_while_they_are_the_same_process() {
        let mut seen = HashMap::from([(10, id(10, 1, "a")), (11, id(11, 1, "b")), (12, id(12, 1, "c")), (13, id(13, 1, "d")), (14, id(14, 1, "e"))]);
        let running_now: HashMap<u32, Proc> = HashMap::from([
            (10, running(10, 1, "a")), // still there, in the tree
            (11, running(11, 1, "b")), // still there, outside the tree: an orphan
            (12, running(12, 99, "c")), // the PID was reused: another start time
            (13, running(13, 1, "other")), // the PID was reused: another program
                                          // 14 ended
        ]);
        let (orphans, dropped) = reconcile(&mut seen, &running_now, &HashSet::from([10]));
        assert_eq!(orphans, vec![11]);
        assert_eq!(dropped, 3);
        assert_eq!(seen.keys().copied().collect::<HashSet<_>>(), HashSet::from([10, 11]));
    }

    #[test]
    fn the_agent_under_a_launcher_is_kept() {
        let p = |pid, ppid, name: &str| Proc { pid, ppid: Some(ppid), name: name.into(), ..running(pid, 1, "x") };
        let root = p(1, 0, "node.exe");
        let kids = [p(2, 1, "codex.exe"), p(3, 1, "git.exe"), p(4, 2, "rg.exe")];
        assert_eq!(keep_set(Some(&root), &kids, Some("codex")), HashSet::from([1, 2]));
        // The agent is the root itself: nothing but the root is kept.
        let direct = p(1, 0, "/usr/bin/claude");
        assert_eq!(keep_set(Some(&direct), &kids, Some("claude")), HashSet::from([1]));
        assert_eq!(keep_set(Some(&root), &kids, None), HashSet::from([1]));
        // A batch file named like the agent: what it starts directly is the agent, whatever its name.
        let batch = p(1, 0, "cmd.exe");
        let wrapped = [p(2, 1, "python.exe"), p(3, 2, "python.exe")];
        assert_eq!(keep_set(Some(&batch), &wrapped, Some("claude")), HashSet::from([1, 2]));
        // A shell session keeps only its shell.
        assert_eq!(keep_set(Some(&batch), &wrapped, None), HashSet::from([1]));
        assert!(keep_set(None, &kids, Some("codex")).is_empty());
    }

    #[test]
    fn a_session_tree_lists_its_processes_by_identity() {
        let procs = Procs::default();
        let (root, t) = session("s");
        let scan = wait_for_children(&procs, &t);
        let r = scan.root.as_ref().expect("the root runs");
        assert_eq!(r.pid, root.id());
        assert!(r.start_time > 0 && r.exe.is_some(), "{r:?}");
        let child = scan.children.first().expect("the launcher started a program");
        assert_ne!(child.pid, r.pid);
        assert!(child.start_time > 0 && child.exe.is_some(), "{child:?}");
        assert!(scan.children.iter().all(|c| !scan.keep.contains(&c.pid)));
        assert!(scan.orphans.is_empty());
        // A program the session did not start is not in its tree.
        let stranger = sleeper();
        let again = procs.scan(std::slice::from_ref(&t)).pop().unwrap();
        assert!(!again.children.iter().any(|c| c.pid == stranger.id()) && again.root.as_ref().is_some_and(|r| r.pid != stranger.id()));
        reap(stranger);
        reap(root);
    }

    #[test]
    fn a_wrong_identity_never_stops_anything() {
        let procs = Procs::default();
        let (root, t) = session("s");
        let scan = wait_for_children(&procs, &t);
        let child = scan.children.first().expect("a child").clone();
        let ask = |start_time, exe: Option<String>| procs.kill(&t, &KillRequest { pid: child.pid, start_time, exe }, Duration::from_millis(200));
        // Another start time: the PID was reused (or the app is looking at an old list).
        assert!(ask(child.start_time + 1, child.exe.clone()).is_err());
        assert!(ask(child.start_time.saturating_sub(1), None).is_err());
        // Another program with that id and start.
        assert!(ask(child.start_time, Some("C:/elsewhere/other.exe".into())).is_err());
        assert!(alive(child.pid), "nothing was stopped");
        // A process that is not in the session's tree, whatever identity is claimed for it.
        let stranger = sleeper();
        let sid = Procs::default().describe(stranger.id()).expect("the stranger runs").0.identity();
        let by_claim = procs.kill(&t, &KillRequest { pid: stranger.id(), start_time: sid.start_time, exe: sid.exe.clone() }, Duration::from_millis(200));
        assert!(by_claim.unwrap_err().contains("not a process of this session"));
        assert!(alive(stranger.id()), "an unrelated process is never stopped");
        // The session's own process and a PID that does not exist.
        assert!(procs.kill(&t, &KillRequest { pid: root.id(), start_time: scan.root.as_ref().unwrap().start_time, exe: None }, Duration::from_millis(200)).unwrap_err().contains("own process"));
        assert!(procs.kill(&t, &KillRequest { pid: std::process::id(), start_time: 1, exe: None }, Duration::from_millis(200)).is_err());
        assert!(alive(root.id()) && alive(child.pid));
        reap(stranger);
        reap(root);
    }

    #[test]
    fn a_listed_process_is_stopped_with_what_it_started_and_the_session_stays() {
        let procs = Procs::default();
        let (root, t) = session("s");
        let scan = wait_for_children(&procs, &t);
        let child = scan.children.first().expect("a child").clone();
        let report = procs.kill(&t, &KillRequest { pid: child.pid, start_time: child.start_time, exe: child.exe.clone() }, Duration::from_millis(300)).unwrap();
        assert!(report.refused.is_empty(), "{report:?}");
        assert!(report.closed.contains(&child.pid) || report.forced.contains(&child.pid) || report.gone.contains(&child.pid), "{report:?}");
        let end = Instant::now() + Duration::from_secs(5);
        while alive(child.pid) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(child.pid));
        reap(root);
    }

    #[test]
    fn killing_the_children_keeps_the_session_process() {
        let procs = Procs::default();
        let (root, t) = session("s");
        let scan = wait_for_children(&procs, &t);
        let kids: Vec<u32> = scan.children.iter().map(|c| c.pid).collect();
        assert!(!kids.is_empty());
        let report = procs.kill_children(&t, Duration::from_millis(300));
        assert!(report.refused.is_empty(), "{report:?}");
        let end = Instant::now() + Duration::from_secs(5);
        while kids.iter().any(|k| alive(*k)) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(kids.iter().all(|k| !alive(*k)));
        // The launcher is the session's own process: it goes on and starts its next program.
        std::thread::sleep(Duration::from_millis(500));
        assert!(alive(root.id()));
        reap(root);
    }

    /// A descendant that is not in the job (it started before the job held its parent, as a program the system's
    /// shell starts through another process does) is still in the tree by its parent, and can be stopped as a member.
    #[cfg(windows)]
    #[test]
    fn a_descendant_outside_the_job_is_in_the_tree_by_its_parent() {
        let mut launcher = Command::new("cmd");
        launcher.args(["/c", "ping -n 60 127.0.0.1 >nul"]).stdout(Stdio::null()).stderr(Stdio::null());
        let root = launcher.spawn().unwrap();
        let end = Instant::now() + Duration::from_secs(15);
        let mut kids = Vec::new();
        while kids.is_empty() && Instant::now() < end {
            kids = crate::proc_tree::children_for_test(root.id(), "ping.exe");
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!kids.is_empty(), "ping runs under cmd");
        let tree = ProcTree::adopt(root.id(), None, None).ok().map(Arc::new);
        assert!(!tree.as_ref().unwrap().pids().unwrap().contains(&kids[0]), "the job holds only cmd");
        let t = Target { id: "s".into(), root: Some(root.id()), tree, agent: None };
        let procs = Procs::default();
        let scan = procs.scan(std::slice::from_ref(&t)).pop().unwrap();
        let member = scan.children.iter().find(|c| c.pid == kids[0]).expect("ping is in the tree by its parent").clone();
        assert!(scan.orphans.is_empty());
        let report = procs.kill(&t, &KillRequest { pid: member.pid, start_time: member.start_time, exe: member.exe.clone() }, Duration::from_millis(200)).unwrap();
        assert!(report.refused.is_empty(), "{report:?}");
        let end = Instant::now() + Duration::from_secs(5);
        while alive(member.pid) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(member.pid));
        reap(root);
    }

    #[test]
    fn a_remembered_process_whose_pid_was_reused_is_dropped_and_survives() {
        let procs = Procs::default();
        let (root, t) = session("s");
        wait_for_children(&procs, &t);
        // The tree remembers a process under the number a stranger now holds (what a PID reuse looks like).
        let stranger = sleeper();
        {
            let mut tracked = procs.tracked.lock();
            tracked.get_mut("s").unwrap().seen.insert(stranger.id(), id(stranger.id(), 1, "C:/old/server.exe"));
        }
        let scan = procs.scan(std::slice::from_ref(&t)).pop().unwrap();
        assert!(scan.dropped >= 1);
        assert!(scan.orphans.is_empty(), "a stale entry is no orphan");
        let err = procs.kill(&t, &KillRequest { pid: stranger.id(), start_time: 1, exe: Some("C:/old/server.exe".into()) }, Duration::from_millis(200));
        assert!(err.is_err());
        assert!(alive(stranger.id()), "the stranger was not touched");
        reap(stranger);
        reap(root);
    }

    #[test]
    fn a_process_that_left_the_session_is_an_orphan_until_it_ends() {
        let procs = Procs::default();
        let (root, t) = session("s");
        let scan = wait_for_children(&procs, &t);
        let child = scan.children.first().expect("a child").clone();
        // Remembered as part of the tree, then the tree is asked about without it (as when it broke away):
        // here the session is told it has no process, which puts everything remembered outside it.
        let gone = Target { root: None, tree: None, ..t.clone() };
        let after = procs.scan(std::slice::from_ref(&gone)).pop().unwrap();
        assert!(after.children.is_empty());
        assert!(after.orphans.iter().any(|o| o.pid == child.pid), "still running, remembered, outside the tree: {:?}", after.orphans.iter().map(|o| o.pid).collect::<Vec<_>>());
        // It can be stopped as an orphan, with its full identity.
        let report = procs.kill(&gone, &KillRequest { pid: child.pid, start_time: child.start_time, exe: child.exe.clone() }, Duration::from_millis(300)).unwrap();
        assert!(report.refused.is_empty(), "{report:?}");
        let end = Instant::now() + Duration::from_secs(5);
        while alive(child.pid) && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive(child.pid));
        // Once it has ended it is dropped, not listed, not stopped.
        let later = procs.scan(std::slice::from_ref(&gone)).pop().unwrap();
        assert!(later.orphans.is_empty());
        reap(root);
    }

    #[test]
    fn the_table_has_each_process_with_its_parent_and_the_start_it_is_known_by() {
        let child = sleeper();
        let mut reader = os::Reader::default();
        let table = reader.read(None);
        let row = table.get(&child.id()).expect("the sleeper is in the table");
        assert_eq!(row.ppid, Some(std::process::id()));
        assert!(row.name.to_ascii_lowercase().starts_with(if cfg!(windows) { "ping" } else { "sleep" }), "{row:?}");
        let listed = proc_of(&table, child.id()).expect("its identity can be read");
        assert!(listed.exe.is_some() && listed.start_time == row.start_time, "{listed:?}");
        assert!(table.get(&std::process::id()).is_some_and(|me| me.mem > 0));
        // Asked for one process, the table has it (on Windows it has them all: there is no cheaper way to ask).
        assert!(reader.read(Some(&[child.id()])).contains_key(&child.id()));
        reap(child);
    }

    /// A process whose parent has gone is reparented to init and stays running: the shape of a leftover dev server.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_process_that_outlives_its_parent_is_listed_as_an_orphan() {
        let procs = Procs::default();
        let mut root = Command::new("sh").args(["-c", "sleep 60 & echo $!; wait"]).stdout(Stdio::piped()).spawn().unwrap();
        let t = Target { id: "s".into(), root: Some(root.id()), tree: None, agent: None };
        let scan = wait_for_children(&procs, &t);
        let child = scan.children.iter().find(|c| c.name.contains("sleep")).expect("the background program").clone();
        let _ = root.kill();
        let _ = root.wait();
        let after = procs.scan(std::slice::from_ref(&t)).pop().unwrap();
        assert!(after.orphans.iter().any(|o| o.pid == child.pid), "{:?}", after.orphans);
        assert!(after.orphans.iter().all(|o| o.start_time == child.start_time));
        let report = procs.kill(&t, &KillRequest { pid: child.pid, start_time: child.start_time, exe: child.exe.clone() }, Duration::from_millis(500)).unwrap();
        assert!(report.refused.is_empty());
    }
}
