//! `process.*`: what a session runs and which ports it holds, and stopping it. The app asks (when its
//! Processes panel is open, or a visible session had children at the last look); the daemon has no
//! timer of its own for any of it. Only the user's own connections may call these: a program running in
//! a session cannot list or stop processes (see `auth`).

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use super::*;
use crate::ports::{self, Listener};
use crate::procs::{KillReport, KillRequest, Proc, Target, TreeScan, GRACE};

/// Ports of one process, as the panel and the chips show them.
fn port_json(l: &Listener) -> Value {
    json!({"port": l.port, "host": l.host(), "addr": l.addr})
}

fn proc_json(p: &Proc, ports: &HashMap<u32, Vec<&Listener>>, orphan: bool) -> Value {
    let mut v = json!(p);
    v["ports"] = Value::Array(ports.get(&p.pid).into_iter().flatten().map(|l| port_json(l)).collect());
    if orphan {
        v["orphan"] = json!(true);
    }
    v
}

fn session_ports(scan: &TreeScan, ports: &HashMap<u32, Vec<&Listener>>) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for p in scan.root.iter().chain(&scan.children).chain(&scan.orphans) {
        for l in ports.get(&p.pid).into_iter().flatten() {
            if !out.iter().any(|o| o["port"] == l.port && o["pid"] == p.pid) {
                let mut v = port_json(l);
                v["pid"] = json!(p.pid);
                v["name"] = json!(p.name);
                out.push(v);
            }
        }
    }
    out
}

/// Listening sockets by owner.
fn by_owner(listeners: &[Listener]) -> HashMap<u32, Vec<&Listener>> {
    let mut m: HashMap<u32, Vec<&Listener>> = HashMap::new();
    for l in listeners {
        m.entry(l.pid).or_default().push(l);
    }
    m
}

fn pids_of(scans: &[TreeScan]) -> Vec<u32> {
    let mut pids: Vec<u32> = scans.iter().flat_map(|s| s.root.iter().chain(&s.children).chain(&s.orphans)).map(|p| p.pid).collect();
    pids.sort_unstable();
    pids.dedup();
    pids
}

impl Daemon {
    /// The session's process as the scanner needs it.
    fn process_target(&self, id: &str) -> Result<Target> {
        let entry = self.by_id(id).ok_or_else(|| anyhow!("no session {id}"))?;
        let (root, tree) = match entry.proc_root() {
            Some((pid, tree)) => (Some(pid), tree),
            None => (None, None),
        };
        let agent = crate::agents::preset_for(&entry.spec().kind).map(|p| p.bin.to_string());
        Ok(Target { id: id.to_string(), root, tree, agent })
    }

    fn forget_closed_sessions(&self) {
        let ids: HashSet<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
        self.procs.retain_sessions(&ids);
    }

    pub(super) fn process_rpc(self: &Arc<Self>, tx: &ClientTx, id: &Value, method: &str, params: Value) -> Result<Option<Value>> {
        Ok(Some(match method {
            // One session's processes with RAM, CPU and ports.
            "process.list" => {
                let started = Instant::now();
                let sid = param(&params, "session")?.as_str().unwrap_or_default();
                let target = self.process_target(sid)?;
                self.forget_closed_sessions();
                let scan = self.procs.scan(std::slice::from_ref(&target)).pop().expect("one target, one scan");
                let listeners = ports::listeners(Some(&pids_of(std::slice::from_ref(&scan))));
                let ports = by_owner(&listeners);
                json!({
                    "session": sid,
                    "root": scan.root.as_ref().map(|p| proc_json(p, &ports, false)),
                    "children": scan.children.iter().map(|p| proc_json(p, &ports, false)).collect::<Vec<_>>(),
                    "orphans": scan.orphans.iter().map(|p| proc_json(p, &ports, true)).collect::<Vec<_>>(),
                    "keep": scan.keep.iter().collect::<Vec<_>>(),
                    "dropped": scan.dropped,
                    "took_us": started.elapsed().as_micros() as u64,
                })
            }
            // The chips: for each session asked about, how many processes it has besides its own and the
            // ports they listen on. One look at the system for all of them.
            "process.ports" => {
                let started = Instant::now();
                let ids = param(&params, "sessions")?.as_array().ok_or_else(|| anyhow!("sessions must be a list"))?;
                let targets: Vec<Target> = ids.iter().filter_map(Value::as_str).filter_map(|sid| self.process_target(sid).ok()).collect();
                self.forget_closed_sessions();
                let scans = if targets.is_empty() { Vec::new() } else { self.procs.scan(&targets) };
                // The ports are asked for only when something runs under a session: a shell or agent with nothing
                // under it (asked about after each burst of output) costs no socket lookup (on macOS, an `lsof`).
                let listeners = if scans.iter().all(|s| s.children.is_empty() && s.orphans.is_empty()) { Vec::new() } else { ports::listeners(Some(&pids_of(&scans))) };
                let ports = by_owner(&listeners);
                let mut sessions = serde_json::Map::new();
                for s in &scans {
                    sessions.insert(s.id.clone(), json!({"children": s.children.len(), "orphans": s.orphans.len(), "ports": session_ports(s, &ports)}));
                }
                json!({"sessions": sessions, "took_us": started.elapsed().as_micros() as u64})
            }
            // Who holds a port, whether or not a session started it.
            "process.port_owner" => {
                let port = param(&params, "port")?.as_u64().and_then(|p| u16::try_from(p).ok()).ok_or_else(|| anyhow!("bad port"))?;
                match ports::listeners(None).into_iter().find(|l| l.port == port) {
                    Some(l) => match self.procs.describe(l.pid) {
                        Some((p, session)) => json!({"port": port, "pid": l.pid, "name": p.name, "exe": p.exe, "session": session}),
                        None => json!({"port": port, "pid": l.pid, "name": null, "exe": null, "session": null}),
                    },
                    None => Value::Null,
                }
            }
            // Stopping waits for the grace period: answered from a thread of its own.
            "process.kill" | "process.kill_children" => {
                let sid = param(&params, "session")?.as_str().unwrap_or_default();
                let target = self.process_target(sid)?;
                let request = if method == "process.kill" {
                    let pid = param(&params, "pid")?.as_u64().and_then(|p| u32::try_from(p).ok()).ok_or_else(|| anyhow!("bad pid"))?;
                    let start_time = param(&params, "start_time")?.as_u64().ok_or_else(|| anyhow!("bad start_time"))?;
                    Some(KillRequest { pid, start_time, exe: params.get("exe").and_then(Value::as_str).map(str::to_string) })
                } else {
                    None
                };
                let (me, tx, id) = (self.clone(), tx.clone(), id.clone());
                std::thread::Builder::new().name("kelpie-process-kill".into()).spawn(move || {
                    let reply = match me.stop_processes(&target, request.as_ref()) {
                        Ok(report) => json!({"id": id, "result": report}),
                        Err(e) => json!({"id": id, "error": {"message": e}}),
                    };
                    let _ = tx.send(json_out(&reply));
                })?;
                return Ok(None);
            }
            other => bail!("unknown method {other}"),
        }))
    }

    fn stop_processes(&self, target: &Target, request: Option<&KillRequest>) -> Result<KillReport, String> {
        match request {
            Some(req) => self.procs.kill(target, req, GRACE),
            None => Ok(self.procs.kill_children(target, GRACE)),
        }
    }
}
