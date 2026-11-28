//! `gate.acquire`: a session's agent asks, from its own pre-tool hook, before a shell command runs (rules in `gate.rs`).
//!
//! The answer is `{decision: run}`, `{decision: rewrite, command, note}` or `{decision: run, note}` after a wait. A heavy
//! command that may not run yet is held here, looked at again every two seconds, for 90 s at most and never more than half
//! the tool's own timeout; then it runs, with the least parallelism the gate may give it, and the note says so. A hold
//! ends at once when the hook that asked has gone. The commands let through are kept as running until the agent reports
//! their end (`PostToolUse`), its turn ends, the session goes, or the tool's own timeout has passed, so the next ask shares
//! the machine with them.
//!
//! Only a program in the session itself may ask, and only for itself (`auth` puts the caller's own id in `session_id`).
//! A session holds at most a few grants and a few holds at once. Turned off with `KELPIE_COMMAND_GATE=0`.

use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use super::*;
use crate::gate::{self, Ask, Machine, Running, Verdict};
use crate::resources::ledger::fingerprint;

/// The longest a command is held.
const HOLD_AT_MOST: Duration = Duration::from_secs(90);
/// How often a held command is looked at again.
const LOOK_EVERY: Duration = Duration::from_secs(2);
/// A started command's estimate counts as not yet in the memory reading for this long.
const PENDING_FOR: Duration = Duration::from_secs(30);
/// A grant whose end was never reported is dropped after the tool's own timeout and this much more (or, without a known
/// timeout, after ten minutes): a command stuck at a permission prompt, or denied, holds no share for ever.
const GRANT_MARGIN: Duration = Duration::from_secs(60);
const GRANT_DEFAULT: Duration = Duration::from_secs(600);
/// Grants and holds one session may have at once; past them a command runs as written.
const MAX_GRANTS_PER_SESSION: usize = 8;
const MAX_HOLDS: usize = 16;

/// Claude's permission modes in which a changed command is applied as the person's own rules allow it. Auto mode refuses a
/// call changed after its review, and dontAsk denies whatever an allow rule does not cover: no rewrite there.
const REWRITE_MODES: [&str; 3] = ["default", "acceptEdits", "bypassPermissions"];

pub(super) fn gate_enabled() -> bool {
    std::env::var("KELPIE_COMMAND_GATE").map_or(true, |v| v != "0")
}

/// A command the gate let through.
pub(super) struct Granted {
    pub running: Running,
    pub at: Instant,
    /// When it is dropped if its end is never reported.
    pub until: Instant,
}

/// Holds waiting now (each on a thread of its own).
static HOLDS: AtomicUsize = AtomicUsize::new(0);

impl Daemon {
    /// The machine as the gate sees it: cores, available memory, what is kept free, the disk. `pending_mb` is filled in
    /// under the gate's lock.
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
        Machine {
            cores: snap["machine"]["cores"].as_u64().filter(|c| *c > 0).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u64)) as u32,
            avail_mb: snap["machine"]["mem_avail_mb"].as_f64(),
            floor_mb: crate::admission::floor_mb(total),
            reserved_others_mb: reserved,
            pending_mb: 0.0,
            disk_busy: matches!(snap["gauges"]["disk"].as_str(), Some("busy" | "saturated")),
        }
    }

    /// The answer for `gate.acquire {session_id, call, command, permission_mode?, timeout_ms?}`. Blocks while the command
    /// is held (called on a thread of its own); `gone` says the hook that asked is no longer waiting.
    pub(super) fn gate_acquire(&self, params: &Value, gone: &dyn Fn() -> bool) -> Result<Value> {
        let session = params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| anyhow!("gate.acquire is for a program in a session"))?.to_string();
        let call = params.get("call").and_then(Value::as_str).filter(|c| !c.is_empty() && c.len() <= 128).unwrap_or_default().to_string();
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
        let mode_rewrites = params.get("permission_mode").and_then(Value::as_str).is_none_or(|m| REWRITE_MODES.contains(&m));
        // Whether this command can be given another parallelism at all, in its own words (see `gate::rewrite`).
        let rewritable = profile.knob.is_some_and(|k| gate::rewrite(&command, &f, k, 1).is_some());
        let tool_timeout = params.get("timeout_ms").and_then(Value::as_u64).map(Duration::from_millis);
        let limit = tool_timeout.map_or(HOLD_AT_MOST, |t| (t / 2).min(HOLD_AT_MOST));
        let lifetime = tool_timeout.unwrap_or(GRANT_DEFAULT) + GRANT_MARGIN;
        let began = Instant::now();
        let mut held_for: Option<String> = None;
        let mut holding = false;
        let result = loop {
            if gone() {
                break run();
            }
            let effective = self.effective_priority(&session);
            let (weight, critical, tier) = effective.as_ref().map_or((2.0, false, crate::resource_policy::Tier::Normal), |e| (e.weight, e.boosted || e.tier == crate::resource_policy::Tier::Critical, e.tier));
            let switches = self.priorities.lock().file.switches.clone();
            let tier_rewrites = match tier {
                crate::resource_policy::Tier::Critical => switches.critical.rewrite,
                crate::resource_policy::Tier::Normal => switches.normal.rewrite,
                crate::resource_policy::Tier::Background => switches.background.rewrite,
            };
            let ask = Ask { fingerprint: &f, profile, learned: learned.as_ref(), weight, critical, may_rewrite: mode_rewrites && tier_rewrites && rewritable };
            let mut machine = self.gate_machine(&owners);
            // Decided and granted under one lock, so two asks in the same moment cannot both take the same room.
            let mut grants = self.gate.lock();
            grants.retain(|_, g| g.until > Instant::now());
            if grants.keys().filter(|k| k.starts_with(&format!("{session}/"))).count() >= MAX_GRANTS_PER_SESSION {
                break run();
            }
            machine.pending_mb = grants.values().filter(|g| g.at.elapsed() < PENDING_FOR).map(|g| g.running.ram_mb).sum();
            let running: Vec<Running> = grants.values().map(|g| g.running.clone()).collect();
            let verdict = match decide_or_give_up(&ask, &running, &machine, began.elapsed() >= limit) {
                Verdict::Hold(why) => {
                    drop(grants);
                    if !holding {
                        // Past the number of holds the computer keeps waiting, a command runs as written.
                        if HOLDS.fetch_add(1, Ordering::SeqCst) >= MAX_HOLDS {
                            HOLDS.fetch_sub(1, Ordering::SeqCst);
                            break run();
                        }
                        holding = true;
                        tracing::info!(session = %session, command = %f.key, why = %why, "command held");
                    }
                    held_for = Some(why);
                    std::thread::sleep(LOOK_EVERY);
                    continue;
                }
                v => v,
            };
            let (estimate, rewritten) = match &verdict {
                Verdict::Rewrite(e) => (*e, profile.knob.and_then(|k| gate::rewrite(&command, &f, k, e.jobs)).filter(|new| *new != command)),
                Verdict::Run(e) => (*e, None),
                Verdict::Hold(_) => unreachable!(),
            };
            // A rewrite that did not happen runs as written, and is charged as written.
            let estimate = if matches!(verdict, Verdict::Rewrite(_)) && rewritten.is_none() {
                gate::estimate(&profile, gate::jobs_as_written(&f, &profile, machine.cores), learned.as_ref())
            } else {
                estimate
            };
            let now = Instant::now();
            grants.insert(
                format!("{session}/{call}"),
                Granted { running: Running { session: session.clone(), fingerprint: f.key.clone(), ram_mb: estimate.ram_mb, cores: estimate.cores, io: profile.io, gpu: profile.gpu, weight }, at: now, until: now + lifetime },
            );
            drop(grants);
            if rewritten.is_some() {
                // The ledger learns this run at the parallelism it really had.
                self.resources.command_jobs(&session, &call, estimate.jobs);
            }
            let waited = held_for.as_ref().map(|why| format!("Kelpie held this command for {} s: {why}.", began.elapsed().as_secs()));
            break Ok(match rewritten {
                Some(new) => {
                    let jobs = if estimate.jobs == 1 { "one job fits".to_string() } else { format!("{} jobs fit", estimate.jobs) };
                    let note = format!("Kelpie ran `{new}` instead of `{command}`: {jobs} this session's share of the computer now.{}", waited.map(|w| format!(" {w}")).unwrap_or_default());
                    json!({"decision": "rewrite", "command": new, "note": note})
                }
                None => match waited {
                    Some(w) => json!({"decision": "run", "note": w}),
                    None => json!({"decision": "run"}),
                },
            });
        };
        if holding {
            HOLDS.fetch_sub(1, Ordering::SeqCst);
        }
        result
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

/// [`gate::decide`], except that a hold past its time runs: with one job when it may be lowered, else as written.
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

    fn daemon() -> (Arc<Daemon>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelpie-gate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        let spec = SessionSpec { id: "s".into(), kind: "claude".into(), cwd: dir.to_string_lossy().into_owned(), ..Default::default() };
        d.insert_for_test(Entry::Term(Session::new(1, spec, 80, 24)));
        (d, dir)
    }

    #[test]
    fn a_heavy_command_alone_runs_and_is_kept_until_its_end() {
        let (d, dir) = daemon();
        let here = || false;
        let light = d.gate_acquire(&json!({"session_id": "s", "call": "c0", "command": "git status"}), &here).unwrap();
        assert_eq!(light, json!({"decision": "run"}));
        assert!(d.gate.lock().is_empty(), "a light command is not kept");
        let heavy = d.gate_acquire(&json!({"session_id": "s", "call": "c1", "command": "cargo build"}), &here).unwrap();
        assert!(matches!(heavy["decision"].as_str(), Some("run" | "rewrite")), "{heavy}");
        assert!(d.gate.lock().contains_key("s/c1"));
        d.gate_release("s", Some("c1"));
        assert!(d.gate.lock().is_empty());
        let auto = d.gate_acquire(&json!({"session_id": "s", "call": "c2", "command": "cargo build", "permission_mode": "auto"}), &here).unwrap();
        assert_eq!(auto["decision"], "run", "auto mode: never a changed command");
        d.gate_release("s", None);
        assert!(d.gate.lock().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grants_run_out_and_a_session_holds_only_a_few() {
        let (d, dir) = daemon();
        let here = || false;
        // A grant whose end is never reported goes after the tool's timeout and a minute.
        d.gate_acquire(&json!({"session_id": "s", "call": "old", "command": "cargo build", "timeout_ms": 1}), &here).unwrap();
        d.gate.lock().get_mut("s/old").unwrap().until = Instant::now();
        for i in 0..MAX_GRANTS_PER_SESSION + 3 {
            d.gate_acquire(&json!({"session_id": "s", "call": format!("c{i}"), "command": "cargo build"}), &here).unwrap();
        }
        let g = d.gate.lock();
        assert!(!g.contains_key("s/old"), "expired");
        assert_eq!(g.len(), MAX_GRANTS_PER_SESSION, "past the cap a command runs as written and holds nothing");
        drop(g);
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
