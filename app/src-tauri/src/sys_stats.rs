//! Memory meter: resident memory of the app (incl. its WebView processes), the session daemon,
//! and everything the daemon runs (shells, agents and their children).

use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::Mutex;
use serde::Serialize;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

use crate::fs_ops::{err, CmdResult};

static SYS: LazyLock<Mutex<System>> = LazyLock::new(|| Mutex::new(System::new()));

#[derive(Serialize)]
pub struct MemoryStats {
    /// App process + WebView processes, bytes.
    app: u64,
    daemon: u64,
    /// Sessions: every descendant of the daemon.
    sessions: u64,
    session_processes: u32,
}

/// Sum of memory of `root`'s descendants (not `root` itself), and their count.
fn descendants(children: &HashMap<Pid, Vec<Pid>>, mem: &HashMap<Pid, u64>, root: Pid) -> (u64, u32) {
    let all = kelpie_proto::procs::descendants(children, root);
    (all.iter().map(|p| mem.get(p).copied().unwrap_or(0)).sum(), all.len() as u32)
}

fn collect(daemon_pid: Option<u32>) -> MemoryStats {
    let mut sys = SYS.lock();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, ProcessRefreshKind::nothing().with_memory());
    let mem: HashMap<Pid, u64> = sys.processes().iter().map(|(pid, p)| (*pid, p.memory())).collect();
    let rows: Vec<(Pid, Option<Pid>, u64)> = sys.processes().iter().map(|(pid, p)| (*pid, p.parent(), p.start_time())).collect();
    let children = kelpie_proto::procs::child_map(&rows);
    let me = Pid::from_u32(std::process::id());
    let daemon = daemon_pid.map(Pid::from_u32);
    let (app_children, _) = descendants(&children, &mem, me);
    let (sessions, session_processes) = daemon.map_or((0, 0), |d| descendants(&children, &mem, d));
    let daemon_mem = daemon.and_then(|d| mem.get(&d).copied()).unwrap_or(0);
    // A daemon the app spawned is its child: count it (and its sessions) once, not in `app`.
    let overlap = daemon.filter(|d| sys.process(*d).and_then(|p| p.parent()) == Some(me)).map_or(0, |_| daemon_mem + sessions);
    MemoryStats {
        app: (mem.get(&me).copied().unwrap_or(0) + app_children).saturating_sub(overlap),
        daemon: daemon_mem,
        sessions,
        session_processes,
    }
}

/// Which of `pids` are still running (e.g. while waiting for stopped sessions to exit).
#[tauri::command]
pub async fn processes_alive(pids: Vec<u32>) -> CmdResult<Vec<u32>> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut sys = SYS.lock();
        let list: Vec<Pid> = pids.iter().map(|p| Pid::from_u32(*p)).collect();
        sys.refresh_processes_specifics(ProcessesToUpdate::Some(&list), true, ProcessRefreshKind::nothing());
        pids.into_iter().filter(|p| sys.process(Pid::from_u32(*p)).is_some()).collect()
    })
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn memory_stats(daemon_pid: Option<u32>) -> CmdResult<MemoryStats> {
    tauri::async_runtime::spawn_blocking(move || collect(daemon_pid)).await.map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_survive_cycles() {
        let (a, b, c) = (Pid::from_u32(1), Pid::from_u32(2), Pid::from_u32(3));
        let children = HashMap::from([(a, vec![b]), (b, vec![c]), (c, vec![a, b])]);
        let mem = HashMap::from([(a, 1), (b, 10), (c, 100)]);
        assert_eq!(descendants(&children, &mem, a), (110, 2));
    }
}
