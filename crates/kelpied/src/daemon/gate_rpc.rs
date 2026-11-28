//! `gate.acquire`: a session's agent asks, from its own pre-tool hook, before a shell command runs (rules in `gate.rs`).
//!
//! The answer is `{decision: run}`, `{decision: rewrite, command, note}` or `{decision: run, note}` after a wait. A heavy
//! command that may not run yet is held here, looked at again every second, for 90 s at most and never more than half
//! the tool's own timeout; then it runs, with the least parallelism it allows, and the note says so. The commands let
//! through are kept as running until the agent reports their end (`PostToolUse`), its turn ends, or the session goes, so
//! the next ask shares the machine with them.
//!
//! Only a program in the session itself may ask (its token proves it, see `auth`). Turned off with `KELPIE_COMMAND_GATE=0`.

use std::time::Duration;

use super::*;
use crate::gate::{self, Ask, Machine, Running, Verdict};
use crate::resources::ledger::fingerprint;

/// The longest a command is held.
const HOLD_AT_MOST: Duration = Duration::from_secs(90);
/// A started command's estimate counts as not yet in the memory reading for this long.
const PENDING_FOR: Duration = Duration::from_secs(30);

pub(super) fn gate_enabled() -> bool {
    std::env::var("KELPIE_COMMAND_GATE").map_or(true, |v| v != "0")
}

/// A command the gate let through.
pub(super) struct Granted {
    pub running: Running,
    pub at: Instant,
}

impl Daemon {
    /// The machine as the gate sees it: cores, available memory, what is kept free, the disk.
    fn gate_machine(&self, owners: &[String]) -> Machine {
        let snap = self.resources.snapshot(false, Some(""));
        let total = self.machine_total_mb(&snap);
        // The sessions are read before the policy is locked: the two locks are never held together.
        let alive: HashSet<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
        let policy = self.priorities.lock();
        let mut reserved = 0.0;
        for (id, s) in &policy.file.sessions {
            if let Some(r) = s.reserve.filter(|_| alive.contains(id) && !owners.contains(id)) {
                reserved += r.ram_mb as f64;
            }
        }
        for (id, p) in &policy.file.projects {
            if let Some(r) = p.reserve.filter(|_| !owners.contains(id)) {
                reserved += r.ram_mb as f64;
            }
        }
        drop(policy);
        let pending = self.gate.lock().values().filter(|g| g.at.elapsed() < PENDING_FOR).map(|g| g.running.ram_mb).sum();
        Machine {
            cores: snap["machine"]["cores"].as_u64().filter(|c| *c > 0).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u64)) as u32,
            avail_mb: snap["machine"]["mem_avail_mb"].as_f64(),
            floor_mb: crate::admission::floor_mb(total),
            reserved_others_mb: reserved,
            pending_mb: pending,
            disk_busy: matches!(snap["gauges"]["disk"].as_str(), Some("busy" | "saturated")),
        }
    }

    /// The answer for `gate.acquire {session_id, call, command, permission_mode?, timeout_ms?}`. Blocks while the command
    /// is held: called on a thread of its own.
    pub(super) fn gate_acquire(&self, params: &Value) -> Result<Value> {
        let session = params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("gate.acquire is for a program in a session"))?.to_string();
        let call = params.get("call").and_then(Value::as_str).unwrap_or_default().to_string();
        let command = params.get("command").and_then(Value::as_str).unwrap_or_default().to_string();
        let run = || Ok(json!({"decision": "run"}));
        if !gate_enabled() || command.is_empty() || call.is_empty() {
            return run();
        }
        let Some(f) = fingerprint(&command) else { return run() };
        let profile = gate::profile(&f);
        if !profile.heavy {
            return run();
        }
        let Some(entry) = self.by_id(&session) else { return run() };
        let spec = entry.spec();
        let project = spec.project_id.clone().unwrap_or_else(|| spec.cwd.clone());
        let owners = vec![project.clone(), session.clone()];
        let learned = self.resources.learned(&project, &f);
        // A changed command is refused in Claude's auto mode (it was reviewed as written): there it is only held.
        let auto_mode = params.get("permission_mode").and_then(Value::as_str) == Some("auto");
        let tool_timeout = params.get("timeout_ms").and_then(Value::as_u64).map(Duration::from_millis);
        let limit = tool_timeout.map_or(HOLD_AT_MOST, |t| (t / 2).min(HOLD_AT_MOST));
        let began = Instant::now();
        let mut held_for: Option<String> = None;
        loop {
            let effective = self.effective_priority(&session);
            let (weight, critical, tier) = effective.as_ref().map_or((2.0, false, crate::resource_policy::Tier::Normal), |e| (e.weight, e.boosted || e.tier == crate::resource_policy::Tier::Critical, e.tier));
            let switches = self.priorities.lock().file.switches.clone();
            let may_rewrite = !auto_mode
                && match tier {
                    crate::resource_policy::Tier::Critical => switches.critical.rewrite,
                    crate::resource_policy::Tier::Normal => switches.normal.rewrite,
                    crate::resource_policy::Tier::Background => switches.background.rewrite,
                };
            let ask = Ask { fingerprint: &f, profile, learned: learned.as_ref(), weight, critical, may_rewrite };
            let running: Vec<Running> = self.gate.lock().iter().filter(|(k, _)| !k.starts_with(&format!("{session}/"))).map(|(_, g)| g.running.clone()).collect();
            let verdict = match decide_or_give_up(&ask, &running, &self.gate_machine(&owners), began.elapsed() >= limit) {
                Verdict::Hold(why) => {
                    if held_for.is_none() {
                        tracing::info!(session = %session, command = %f.key, why = %why, "command held");
                    }
                    held_for = Some(why);
                    std::thread::sleep(Duration::from_secs(1));
                    continue;
                }
                v => v,
            };
            let (estimate, rewritten) = match &verdict {
                Verdict::Rewrite(e) => (*e, profile.knob.and_then(|k| gate::rewrite(&command, &f, k, e.jobs))),
                Verdict::Run(e) => (*e, None),
                Verdict::Hold(_) => unreachable!(),
            };
            self.gate.lock().insert(
                format!("{session}/{call}"),
                Granted { running: Running { session: session.clone(), fingerprint: f.key.clone(), ram_mb: estimate.ram_mb, cores: estimate.cores, io: profile.io, gpu: profile.gpu, weight }, at: Instant::now() },
            );
            let waited = held_for.as_ref().map(|why| format!("Kelpie held this command for {} s: {why}.", began.elapsed().as_secs()));
            return Ok(match rewritten {
                Some(new) => {
                    let jobs = if estimate.jobs == 1 { "one job fits".to_string() } else { format!("{} jobs fit", estimate.jobs) };
                    let note = format!(
                        "Kelpie ran `{new}` instead of `{command}`: {jobs} this session's share of the computer now.{}",
                        waited.map(|w| format!(" {w}")).unwrap_or_default()
                    );
                    json!({"decision": "rewrite", "command": new, "note": note})
                }
                None => match waited {
                    Some(w) => json!({"decision": "run", "note": w}),
                    None => json!({"decision": "run"}),
                },
            });
        }
    }

    /// The command `call` of `session` ended, or everything of `session` (its turn ended, it went): its share is free.
    pub(super) fn gate_release(&self, session: &str, call: Option<&str>) {
        let mut g = self.gate.lock();
        match call {
            Some(call) => {
                g.remove(&format!("{session}/{call}"));
            }
            None => g.retain(|k, _| !k.starts_with(&format!("{session}/"))),
        }
    }
}

/// [`gate::decide`], except that a hold past its time runs with the least parallelism the command allows.
fn decide_or_give_up(ask: &Ask, running: &[Running], m: &Machine, out_of_time: bool) -> Verdict {
    match gate::decide(ask, running, m) {
        Verdict::Hold(_) if out_of_time => {
            if ask.may_rewrite && ask.profile.knob.is_some() {
                Verdict::Rewrite(gate::estimate(&ask.profile, 1, None))
            } else {
                Verdict::Run(gate::estimate(&ask.profile, gate::jobs_as_written(ask.fingerprint, &ask.profile, m.cores), ask.learned))
            }
        }
        v => v,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heavy_command_alone_runs_and_is_kept_until_its_end() {
        let dir = std::env::temp_dir().join(format!("kelpie-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        let spec = SessionSpec { id: "s".into(), kind: "claude".into(), cwd: dir.to_string_lossy().into_owned(), ..Default::default() };
        d.insert_for_test(Entry::Term(Session::new(1, spec, 80, 24)));
        let light = d.gate_acquire(&json!({"session_id": "s", "call": "c0", "command": "git status"})).unwrap();
        assert_eq!(light, json!({"decision": "run"}));
        assert!(d.gate.lock().is_empty(), "a light command is not kept");
        let heavy = d.gate_acquire(&json!({"session_id": "s", "call": "c1", "command": "cargo build"})).unwrap();
        assert!(matches!(heavy["decision"].as_str(), Some("run" | "rewrite")), "{heavy}");
        assert!(d.gate.lock().contains_key("s/c1"));
        d.gate_release("s", Some("c1"));
        assert!(d.gate.lock().is_empty());
        // A held command past its time runs with one job (a calm daemon in a test has no memory reading: nothing holds).
        let none = d.gate_acquire(&json!({"session_id": "s", "call": "c2", "command": "cargo build", "permission_mode": "auto"})).unwrap();
        assert_eq!(none["decision"], "run", "auto mode: never a changed command");
        d.gate_release("s", None);
        assert!(d.gate.lock().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_hold_past_its_time_runs_with_the_least_parallelism() {
        let f = fingerprint("cargo build").unwrap();
        let ask = Ask { fingerprint: &f, profile: gate::profile(&f), learned: None, weight: 1.0, critical: false, may_rewrite: true };
        let running = [Running { session: "o".into(), fingerprint: "cargo build".into(), ram_mb: 3000.0, cores: 3.0, io: false, gpu: false, weight: 4.0 }];
        let full = Machine { cores: 4, avail_mb: Some(1200.0), floor_mb: 1000.0, ..Machine::default() };
        assert!(matches!(decide_or_give_up(&ask, &running, &full, false), Verdict::Hold(_)));
        assert!(matches!(decide_or_give_up(&ask, &running, &full, true), Verdict::Rewrite(e) if e.jobs == 1));
    }
}
