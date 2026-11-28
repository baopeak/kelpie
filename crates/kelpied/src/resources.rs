//! The machine's CPU, memory, disks and GPUs, and what each session and each command it runs takes of them.
//!
//! The daemon samples always, not only while a phone is connected (the load level of `load.rs` is the phone's): every two
//! seconds while a session works, every ten otherwise. A sample reads the machine (`os::Probe`) and the table of processes
//! once (`procs::Sampler`, which counts no `process.*` scan), sums each session's tree, and feeds five gauges:
//!
//! - **cpu**: busy share of all cores, the run queue against the core count, and Linux's CPU pressure;
//! - **ram**: available memory and commit, the system's low-memory signal, and Linux's memory pressure;
//! - **disk**: the busiest physical disk's busy time and queue, and Linux's I/O pressure;
//! - **space**: the fullest volume holding a session's folder or the daemon's data;
//! - **gpu**: the busiest adapter's busiest engine type.
//!
//! Each gauge is `ok`, `busy` or `saturated`, or unknown when this machine cannot tell (never read as idle). A gauge rises
//! after two samples in a row ask for more and falls only after ten seconds of asking for less, so it does not flap. For
//! every gauge that is not `ok`, the sample names who uses that resource most: sessions, and the programs outside Kelpie
//! grouped by name.
//!
//! **Commands.** Between an agent's report that it runs a shell command (Claude's `PreToolUse` on `Bash`) and its report
//! that the command ended, what the processes started since the command began take in the session's tree is the command's:
//! the agent's own process, its MCP servers and an earlier dev server are never charged to it. A command seen in at least
//! one sample is learned by the ledger (`ledger.rs`, `command-ledger.json`), which is what an estimate of the next run is
//! built from. One that never got an end report (denied, blocked by a hook, stopped with Esc) is dropped at the turn's
//! boundary, unlearned; one sent to the background is not measured. Two commands running at once in one session are both
//! charged what either started: an estimate errs high, never low.

pub mod ledger;
#[cfg(test)]
mod tests;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;
#[cfg(unix)]
use unix as os;
#[cfg(windows)]
use windows as os;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;


use parking_lot::Mutex;
use serde::Serialize;
use serde_json::{json, Value};

use crate::load::Level;
use crate::procs::{Proc, TreeUsage};
use ledger::{Demand, Fingerprint, Ledger};

/// Samples kept per session: ten minutes at the working pace.
const RING: usize = 300;
/// Samples in a row asking for a higher state before a gauge rises; time asking for less before it falls.
const ENTER_AFTER: u32 = 2;
const LEAVE_AFTER_MS: u64 = 10_000;
/// Commands one session may have open at once; one past it is not measured.
const MAX_OPEN: usize = 16;
/// Samples in a row a gauge may miss (a counter that did not answer) before it reads as unknown.
const MISSES_ALLOWED: u32 = 2;
/// A process counts as a command's when it started at most this long before the command began (Unix start
/// times are whole seconds).
const START_SLACK_MS: u64 = 1_000;
/// A command with no end reported after this long is dropped unlearned (its report was lost).
const OPEN_FOR_AT_MOST_MS: u64 = 3 * 60 * 60 * 1000;
/// Users of a contended resource listed: sessions and outside programs each.
const TOP: usize = 5;
const MB: f64 = 1_048_576.0;

#[derive(Debug, Clone, Default, Serialize)]
pub struct DiskReading {
    /// The disk as the system names it (`0 C: D:`, `nvme0n1`).
    pub name: String,
    pub busy_pct: Option<f64>,
    /// Requests waiting or in flight.
    pub queue: Option<f64>,
    /// Bytes moved per second, MiB.
    pub mbps: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct VolumeReading {
    pub path: String,
    pub free_mb: u64,
    pub total_mb: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct GpuReading {
    pub id: String,
    /// Its busiest engine type, percent.
    pub util_pct: Option<f64>,
    pub dedicated_mb: Option<u64>,
    pub shared_mb: Option<u64>,
}

/// Linux's pressure stall information: percent of the last ten seconds tasks waited on it.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Pressure {
    pub cpu: Option<f64>,
    pub memory: Option<f64>,
    pub io: Option<f64>,
}

/// One reading of the machine; `None` where it cannot tell.
#[derive(Debug, Clone, Default, Serialize)]
pub struct MachineReading {
    pub cores: u32,
    pub cpu_pct: Option<f64>,
    /// Threads ready to run but waiting for a core (Windows).
    pub cpu_queue: Option<f64>,
    pub mem_total_mb: Option<u64>,
    pub mem_avail_mb: Option<u64>,
    /// What may be committed (memory plus page file) and what of it is left.
    pub commit_limit_mb: Option<u64>,
    pub commit_avail_mb: Option<u64>,
    pub low_memory: bool,
    pub pressure: Pressure,
    pub disks: Vec<DiskReading>,
    pub volumes: Vec<VolumeReading>,
    pub gpus: Vec<GpuReading>,
    /// GPU use by process: (busiest engine percent, memory MiB).
    #[serde(skip)]
    pub gpu_by_pid: HashMap<u32, (f64, u64)>,
}

/// How much a resource is in demand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Ok,
    Busy,
    Saturated,
}

/// The gauges, in this order.
pub const KINDS: [&str; 5] = ["cpu", "ram", "disk", "space", "gpu"];

fn pick(saturated: bool, busy: bool) -> State {
    if saturated {
        State::Saturated
    } else if busy {
        State::Busy
    } else {
        State::Ok
    }
}

/// The state each resource asks for in this reading (in `KINDS` order).
pub fn wants(r: &MachineReading) -> [Option<State>; 5] {
    let cores = f64::from(r.cores.max(1));
    let cpu = r.cpu_pct.map(|c| {
        let queue = r.cpu_queue.unwrap_or(0.0) / cores;
        let psi = r.pressure.cpu.unwrap_or(0.0);
        pick(c >= 95.0 || (queue >= 4.0 && c >= 85.0) || psi >= 50.0, c >= 85.0 || (queue >= 2.0 && c >= 70.0) || psi >= 20.0)
    });
    let ram = match (r.mem_total_mb, r.mem_avail_mb) {
        (Some(total), Some(avail)) if total > 0 => {
            let free = avail as f64 * 100.0 / total as f64;
            let commit = match (r.commit_limit_mb, r.commit_avail_mb) {
                (Some(limit), Some(left)) if limit > 0 => left as f64 * 100.0 / limit as f64,
                _ => 100.0,
            };
            let psi = r.pressure.memory.unwrap_or(0.0);
            Some(pick(r.low_memory || free < 8.0 || commit < 5.0 || psi >= 10.0, free < 15.0 || commit < 10.0 || psi >= 2.0))
        }
        _ => None,
    };
    let disks: Vec<State> = r
        .disks
        .iter()
        .filter_map(|d| {
            let busy = d.busy_pct?;
            let queue = d.queue.unwrap_or(0.0);
            Some(pick(busy >= 95.0 && queue >= 2.0, busy >= 80.0 || queue >= 2.0))
        })
        .chain(r.pressure.io.map(|psi| pick(psi >= 30.0, psi >= 10.0)))
        .collect();
    let space = r
        .volumes
        .iter()
        .filter(|v| v.total_mb > 0)
        .map(|v| {
            let free = v.free_mb as f64 * 100.0 / v.total_mb as f64;
            pick(free < 5.0 || v.free_mb < 2048, free < 10.0 || v.free_mb < 5120)
        })
        .max();
    let gpu = r.gpus.iter().filter_map(|g| g.util_pct).reduce(f64::max).map(|u| pick(u >= 95.0, u >= 80.0));
    [cpu, ram, disks.into_iter().max(), space, gpu]
}

/// A state that holds: up after two samples asking for more, down after ten seconds asking for less.
#[derive(Debug, Default, Clone)]
pub struct Gauge {
    pub state: Option<State>,
    /// Samples in a row above, and the least of what they asked.
    above: Option<(u32, State)>,
    /// Since when (ms) the samples asked for less, and the most of what they asked.
    below: Option<(u64, State)>,
    /// Samples in a row that could not tell.
    misses: u32,
}

impl Gauge {
    /// Takes what a sample at `now` (ms) asks for; true when the state changed.
    pub fn feed(&mut self, now: u64, want: Option<State>) -> bool {
        let Some(want) = want else {
            // One counter that did not answer is not a change: the state holds for a couple of samples.
            self.misses += 1;
            if self.misses <= MISSES_ALLOWED {
                return false;
            }
            let changed = self.state.is_some();
            *self = Gauge::default();
            return changed;
        };
        self.misses = 0;
        let Some(state) = self.state else {
            // The first thing known about it stands.
            self.state = Some(want);
            return true;
        };
        let mut to = None;
        if want > state {
            self.below = None;
            let (n, least) = self.above.map_or((1, want), |(n, least)| (n + 1, least.min(want)));
            self.above = Some((n, least));
            if n >= ENTER_AFTER {
                to = Some(least);
            }
        } else if want < state {
            self.above = None;
            let (since, most) = self.below.map_or((now, want), |(since, most)| (since, most.max(want)));
            self.below = Some((since, most));
            if now.saturating_sub(since) >= LEAVE_AFTER_MS {
                to = Some(most);
            }
        } else {
            self.above = None;
            self.below = None;
        }
        match to {
            Some(s) if s != state => {
                *self = Gauge { state: Some(s), ..Gauge::default() };
                true
            }
            _ => false,
        }
    }
}

/// What a session's processes took in one sample.
#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq)]
pub struct Usage {
    /// ms since the epoch.
    pub at: u64,
    /// Resident (working set) and private memory, MiB.
    pub ws_mb: f64,
    pub private_mb: f64,
    /// CPU, in cores.
    pub cores: f64,
    pub io_mbps: f64,
    pub gpu_pct: f64,
    pub gpu_mb: f64,
    pub procs: usize,
}

/// A command a session runs, while it runs. What it takes is what the processes started since it began take: the
/// agent's own process (and its MCP servers, an earlier dev server) are never charged to it.
#[derive(Debug, Clone)]
struct Open {
    project: String,
    fingerprint: Fingerprint,
    began: u64,
    samples: u32,
    demand: Demand,
    cores_sum: f64,
}

#[derive(Debug, Default)]
struct Track {
    project: Option<String>,
    ring: VecDeque<Usage>,
    peak_private_mb: f64,
    /// The job's own peak (Windows), MiB.
    peak_commit_mb: Option<f64>,
    open: HashMap<String, Open>,
}

/// A session as the sampler sees it: its processes and what it belongs to.
pub struct SessionSample {
    pub tree: TreeUsage,
    pub project: Option<String>,
    /// The job's peak commit in bytes, where the system keeps one.
    pub peak_commit: Option<u64>,
}

#[derive(Default)]
struct Inner {
    reading: MachineReading,
    gauges: [Gauge; 5],
    tracks: HashMap<String, Track>,
    /// Bytes each process had read and written at the last sample, by (pid, start).
    io_seen: HashMap<(u32, u64), u64>,
    /// Who uses each contended resource most, by gauge.
    users: HashMap<&'static str, Vec<Value>>,
    at: u64,
    samples: u64,
}

pub struct Resources {
    inner: Mutex<Inner>,
    ledger: Mutex<Ledger>,
    /// Set when a command starts: the sampler waiting out an idle pause samples at once and keeps the working pace.
    wake: (std::sync::Mutex<bool>, std::sync::Condvar),
}

/// When a process started, in ms since the epoch (the scans keep whole seconds on Unix).
fn started_ms(p: &Proc) -> u64 {
    if cfg!(windows) {
        p.start_time
    } else {
        p.start_time.saturating_mul(1000)
    }
}

/// The machine's probe, kept by the sampling thread.
#[derive(Default)]
pub struct Probe(os::Probe);

impl Probe {
    pub fn read(&mut self, volumes: &[PathBuf]) -> MachineReading {
        self.0.read(volumes)
    }
}

fn mb(bytes: u64) -> f64 {
    bytes as f64 / MB
}

impl Resources {
    /// With the ledger kept in `ledger` (none: in memory only, for tests).
    pub fn new(ledger: Option<PathBuf>) -> Resources {
        Resources { inner: Mutex::new(Inner::default()), ledger: Mutex::new(Ledger::open(ledger)), wake: Default::default() }
    }

    /// Takes one sample: the machine, every process, and each session's tree among them. `secs` is the time since the
    /// previous sample. Returns whether a gauge changed.
    pub fn record(&self, now: u64, reading: MachineReading, all: &[Proc], sessions: Vec<SessionSample>, secs: f64) -> bool {
        {
            let mut g = self.inner.lock();
            let g = &mut *g;
            g.samples += 1;
            g.at = now;
            // I/O moved since the last sample, by process: a process seen for the first time counts all it moved
            // (it started since); one that ended took its share with it.
            // The sampler keeps samples at least a second apart; a rate over less would be noise.
            let secs = secs.max(0.5);
            let mut io_now: HashMap<(u32, u64), u64> = HashMap::with_capacity(all.len());
            let mut io_delta: HashMap<u32, u64> = HashMap::with_capacity(all.len());
            for p in all {
                let key = (p.pid, p.start_time);
                let before = g.io_seen.get(&key).copied();
                // Counted from scratch only when the previous table was read: after a failed read every process
                // would look new and its whole lifetime's traffic would show as one burst.
                let fresh = if g.samples > 1 && !g.io_seen.is_empty() { p.io } else { 0 };
                io_delta.insert(p.pid, before.map_or(fresh, |b| p.io.saturating_sub(b)));
                io_now.insert(key, p.io);
            }
            // A failed read of the table keeps what the last good one saw.
            if !all.is_empty() {
                g.io_seen = io_now;
            }
            let mut in_sessions: HashSet<u32> = HashSet::new();
            let live: HashSet<String> = sessions.iter().map(|s| s.tree.id.clone()).collect();
            g.tracks.retain(|id, _| live.contains(id));
            for s in sessions {
                let mut u = Usage { at: now, procs: s.tree.procs.len(), ..Usage::default() };
                for p in &s.tree.procs {
                    in_sessions.insert(p.pid);
                    u.ws_mb += mb(p.mem);
                    u.private_mb += mb(p.private);
                    u.cores += f64::from(p.cpu) / 100.0;
                    u.io_mbps += mb(io_delta.get(&p.pid).copied().unwrap_or(0)) / secs;
                    if let Some((pct, gmb)) = reading.gpu_by_pid.get(&p.pid) {
                        u.gpu_pct = u.gpu_pct.max(*pct);
                        u.gpu_mb += *gmb as f64;
                    }
                }
                let t = g.tracks.entry(s.tree.id.clone()).or_default();
                t.project = s.project;
                t.peak_private_mb = t.peak_private_mb.max(u.private_mb);
                t.peak_commit_mb = s.peak_commit.map(mb).or(t.peak_commit_mb);
                for o in t.open.values_mut() {
                    let mut c = Usage::default();
                    for p in s.tree.procs.iter().filter(|p| started_ms(p) + START_SLACK_MS >= o.began) {
                        c.private_mb += mb(p.private);
                        c.cores += f64::from(p.cpu) / 100.0;
                        c.io_mbps += mb(io_delta.get(&p.pid).copied().unwrap_or(0)) / secs;
                        if let Some((pct, gmb)) = reading.gpu_by_pid.get(&p.pid) {
                            c.gpu_pct = c.gpu_pct.max(*pct);
                            c.gpu_mb += *gmb as f64;
                        }
                    }
                    o.samples += 1;
                    o.cores_sum += c.cores;
                    let d = &mut o.demand;
                    d.ram_mb = d.ram_mb.max(c.private_mb);
                    d.peak_cores = d.peak_cores.max(c.cores);
                    d.io_mb += c.io_mbps * secs;
                    d.peak_io_mbps = d.peak_io_mbps.max(c.io_mbps);
                    d.gpu_pct = d.gpu_pct.max(c.gpu_pct);
                    d.gpu_mb = d.gpu_mb.max(c.gpu_mb);
                }
                let lost: Vec<String> = t.open.iter().filter(|(_, o)| now.saturating_sub(o.began) > OPEN_FOR_AT_MOST_MS).map(|(k, _)| k.clone()).collect();
                for k in lost {
                    t.open.remove(&k);
                }
                if t.ring.len() == RING {
                    t.ring.pop_front();
                }
                t.ring.push_back(u);
            }
            let want = wants(&reading);
            let mut changed = false;
            for (gauge, w) in g.gauges.iter_mut().zip(want) {
                changed |= gauge.feed(now, w);
            }
            g.users = users(&g.gauges, &g.tracks, all, &in_sessions, &io_delta, &reading, secs);
            g.reading = reading;
            changed
        }
    }

    /// An agent in session `id` is about to run `command` (`call` names this run, as the agent's end report will).
    pub fn command_started(&self, id: &str, call: &str, command: &str, project: &str, now: u64) {
        let Some(fingerprint) = ledger::fingerprint(command) else { return };
        let mut g = self.inner.lock();
        let t = g.tracks.entry(id.to_string()).or_default();
        if t.open.len() >= MAX_OPEN {
            return;
        }
        t.open.insert(call.to_string(), Open { project: project.to_string(), fingerprint, began: now, samples: 0, demand: Demand::default(), cores_sum: 0.0 });
        drop(g);
        let (flag, cv) = &self.wake;
        *flag.lock().unwrap_or_else(|e| e.into_inner()) = true;
        cv.notify_all();
    }

    /// Waits up to `pause` for the next sample, or less when a command starts meanwhile.
    pub fn wait(&self, pause: std::time::Duration) {
        let (flag, cv) = &self.wake;
        let guard = flag.lock().unwrap_or_else(|e| e.into_inner());
        let (mut guard, _) = cv.wait_timeout_while(guard, pause, |woken| !*woken).unwrap_or_else(|e| e.into_inner());
        *guard = false;
    }

    /// Whether a session has a command open: the sampler keeps the working pace while one runs.
    pub fn commands_running(&self) -> bool {
        self.inner.lock().tracks.values().any(|t| !t.open.is_empty())
    }

    /// The run `call` of session `id` ended; it is learned when at least one sample saw it.
    pub fn command_ended(&self, id: &str, call: &str, failed: bool, now: u64) {
        let open = self.inner.lock().tracks.get_mut(id).and_then(|t| t.open.remove(call));
        if let Some(o) = open {
            learn(&mut self.ledger.lock(), o, failed, now);
        }
    }

    /// A turn began or ended: a command still open never ran to an end the agent reported (the person denied it,
    /// another hook blocked it, Esc stopped the turn). It is dropped, not learned: what it cost is not known.
    pub fn turn_boundary(&self, id: &str) {
        if let Some(t) = self.inner.lock().tracks.get_mut(id) {
            t.open.clear();
        }
    }

    /// The level the sub-session caps read: `L3` while memory is saturated (as the phone's load level says it).
    pub fn caps_level(&self) -> Level {
        if self.inner.lock().gauges[1].state == Some(State::Saturated) {
            Level::L3
        } else {
            Level::L0
        }
    }

    pub fn samples(&self) -> u64 {
        self.inner.lock().samples
    }

    /// Writes the ledger when it learned something.
    pub fn save(&self) {
        self.ledger.lock().save();
    }

    /// The machine, the gauges, each session (with its last ten minutes when `history`), the commands running and who
    /// uses each contended resource. `session` limits the sessions to one.
    pub fn snapshot(&self, history: bool, session: Option<&str>) -> Value {
        let g = self.inner.lock();
        let gauges: serde_json::Map<String, Value> = KINDS.iter().zip(&g.gauges).map(|(k, gg)| (k.to_string(), json!(gg.state))).collect();
        let sessions: Vec<Value> = g
            .tracks
            .iter()
            .filter(|(id, _)| session.is_none_or(|s| s == id.as_str()))
            .map(|(id, t)| {
                let now = t.ring.back().copied().unwrap_or_default();
                let mut v = json!({
                    "id": id,
                    "project": t.project,
                    "now": now,
                    "peak_private_mb": t.peak_private_mb,
                    "peak_commit_mb": t.peak_commit_mb,
                    "commands": t.open.values().map(|o| json!({
                        "fingerprint": o.fingerprint.key, "jobs": o.fingerprint.jobs, "began": o.began, "samples": o.samples,
                        "ram_mb": o.demand.ram_mb, "peak_cores": o.demand.peak_cores, "io_mb": o.demand.io_mb, "gpu_pct": o.demand.gpu_pct,
                    })).collect::<Vec<_>>(),
                });
                if history {
                    v["history"] = json!(t.ring);
                }
                v
            })
            .collect();
        json!({
            "at": g.at,
            "samples": g.samples,
            "machine": g.reading,
            "gauges": gauges,
            "sessions": sessions,
            "users": g.users,
        })
    }

    /// What the ledger learned, for one project or all.
    pub fn ledger(&self, project: Option<&str>) -> Value {
        let list = self.ledger.lock().list(project);
        Value::Array(
            list.into_iter()
                .map(|(project, key, jobs, e)| {
                    json!({
                        "project": project, "fingerprint": key, "jobs": jobs, "runs": e.runs, "failed": e.failed, "last": e.last,
                        "plan_ram_mb": e.plan_ram_mb(), "ram_mb": e.ram_mb.mean, "ram_sd": e.ram_mb.sd(), "cores": e.cores.mean,
                        "peak_cores": e.peak_cores.mean, "io_mb": e.io_mb.mean, "peak_io_mbps": e.peak_io_mbps.mean, "gpu_pct": e.gpu_pct.mean,
                        "gpu_mb": e.gpu_mb.mean, "secs": e.secs.mean,
                    })
                })
                .collect(),
        )
    }
}

fn learn(ledger: &mut Ledger, o: Open, failed: bool, now: u64) {
    if o.samples == 0 {
        return;
    }
    let mut d = o.demand;
    d.ram_mb = d.ram_mb.max(0.0);
    d.cores = o.cores_sum / f64::from(o.samples);
    d.secs = now.saturating_sub(o.began) as f64 / 1000.0;
    ledger.learn(&o.project, &o.fingerprint, &d, failed, now);
}

/// For each gauge that is not `ok`, its biggest users: sessions, and programs outside them grouped by name.
fn users(
    gauges: &[Gauge; 5],
    tracks: &HashMap<String, Track>,
    all: &[Proc],
    in_sessions: &HashSet<u32>,
    io_delta: &HashMap<u32, u64>,
    reading: &MachineReading,
    secs: f64,
) -> HashMap<&'static str, Vec<Value>> {
    let mut out = HashMap::new();
    for (i, kind) in KINDS.iter().enumerate() {
        if !matches!(gauges[i].state, Some(State::Busy | State::Saturated)) || *kind == "space" {
            continue;
        }
        // What one sample or process counts for this resource, and its unit.
        let of_session = |u: &Usage| match *kind {
            "cpu" => u.cores,
            "ram" => u.private_mb,
            "disk" => u.io_mbps,
            _ => u.gpu_pct,
        };
        let of_proc = |p: &Proc| match *kind {
            "cpu" => f64::from(p.cpu) / 100.0,
            "ram" => mb(p.private),
            "disk" => mb(io_delta.get(&p.pid).copied().unwrap_or(0)) / secs,
            _ => reading.gpu_by_pid.get(&p.pid).map_or(0.0, |g| g.0),
        };
        let mut list: Vec<(f64, Value)> = tracks
            .iter()
            .filter_map(|(id, t)| t.ring.back().map(|u| (of_session(u), json!({"session": id, "project": t.project, "value": of_session(u)}))))
            .collect();
        let mut outside: HashMap<&str, (f64, usize)> = HashMap::new();
        for p in all.iter().filter(|p| !in_sessions.contains(&p.pid) && p.pid != 0) {
            let e = outside.entry(p.name.as_str()).or_default();
            e.0 += of_proc(p);
            e.1 += 1;
        }
        let mut programs: Vec<(f64, Value)> = outside.into_iter().map(|(name, (v, n))| (v, json!({"program": name, "count": n, "value": v}))).collect();
        list.sort_by(|a, b| b.0.total_cmp(&a.0));
        programs.sort_by(|a, b| b.0.total_cmp(&a.0));
        let top: Vec<Value> = list.into_iter().filter(|(v, _)| *v > 0.0).take(TOP).chain(programs.into_iter().filter(|(v, _)| *v > 0.0).take(TOP)).map(|(_, v)| v).collect();
        out.insert(*kind, top);
    }
    out
}
