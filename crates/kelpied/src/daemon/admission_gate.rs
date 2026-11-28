//! The admission queue in the daemon (rules in `admission.rs`): what the computer can take now, the candidates waiting
//! (run tasks every tick; a sub-session when a program asks for one, in the same line), and the answer each gets.

use super::*;
use crate::admission::{self, Candidate, Capacity};
use crate::resource_policy::{Effective, Who};

/// A session younger than this may not hold yet what its agent will: the difference is counted as taken.
const YOUNG_MS: u64 = 60_000;

/// What the queue knows of the computer at one moment.
struct Room {
    cap: Capacity,
    /// Each live local agent session's kind, and private memory where measured: what an agent of a kind costs.
    kinds: HashMap<String, String>,
    private: HashMap<String, f64>,
    effective: HashMap<String, Effective>,
}

impl Room {
    /// What an agent of `kind` takes when it starts: the mean private memory of the sessions running it now (what they
    /// hold, which is more than at the start: an estimate that errs high), or the prior.
    fn agent_cost(&self, kind: &str) -> f64 {
        let seen: Vec<f64> = self.kinds.iter().filter(|(_, k)| k.as_str() == kind).filter_map(|(id, _)| self.private.get(id).copied()).filter(|mb| *mb > 0.0).collect();
        if seen.is_empty() {
            admission::AGENT_PRIOR_MB
        } else {
            seen.iter().sum::<f64>() / seen.len() as f64
        }
    }
}

impl Daemon {
    /// This computer's memory in MiB: the monitor's reading, else the system's own figure (the monitor is off, or has not
    /// sampled yet), so a limit always holds.
    pub(super) fn machine_total_mb(&self, snap: &Value) -> f64 {
        snap["machine"]["mem_total_mb"].as_f64().unwrap_or_else(|| {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            (sys.total_memory() >> 20) as f64
        })
    }

    /// The computer as the queue sees it. Agents of run tasks that finished do not count (their sessions may idle on);
    /// tasks still starting, and spawns under way, do. Memory the starts of the last minute have not taken yet is counted
    /// as taken, so a reading a few seconds old does not let each tick start another batch.
    fn admission_room(&self) -> Room {
        let snap = self.resources.snapshot(false, None);
        let private: HashMap<String, f64> = snap["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| Some((s["id"].as_str()?.to_string(), s["now"]["private_mb"].as_f64()?)))
            .collect();
        let finished = self.finished_task_sessions();
        let now = now_millis();
        let mut kinds = HashMap::new();
        let mut young: Vec<(String, String)> = Vec::new();
        let mut per_project: HashMap<String, usize> = HashMap::new();
        let mut alive = HashSet::new();
        for e in self.sessions.read().values() {
            let info = e.info();
            if !info.alive {
                continue;
            }
            alive.insert(info.spec.id.clone());
            if info.spec.kind == "shell" || info.spec.remote.is_some() || finished.contains(&info.spec.id) {
                continue;
            }
            *per_project.entry(info.spec.project_id.clone().unwrap_or_else(|| info.spec.cwd.clone())).or_default() += 1;
            kinds.insert(info.spec.id.clone(), info.spec.kind.clone());
            if now.saturating_sub(info.spec.created_at) < YOUNG_MS {
                young.push((info.spec.id.clone(), info.spec.kind.clone()));
            }
        }
        // Tasks starting whose session does not exist yet (with one, the session is counted above).
        let starting: Vec<String> = self
            .runs
            .lock()
            .list
            .iter()
            .flat_map(|r| r.tasks.iter())
            .filter(|t| t.status == TaskStatus::Starting && t.host.is_none() && t.session_id.as_ref().is_none_or(|s| !alive.contains(s)))
            .map(|t| t.kind.clone())
            .collect();
        let total = self.machine_total_mb(&snap);
        let floor = admission::floor_mb(total);
        let policy = self.priorities.lock();
        let max_agents = match policy.file.max_agents {
            0 => admission::auto_max_agents(total, floor),
            n => n as usize,
        };
        // A reservation holds while its owner has a live session: a session's own, or its project's.
        let mut reserved: Vec<(String, f64)> = Vec::new();
        for (id, s) in &policy.file.sessions {
            if let Some(r) = s.reserve.filter(|_| alive.contains(id)) {
                reserved.push((id.clone(), r.ram_mb as f64));
            }
        }
        for (id, p) in &policy.file.projects {
            if let Some(r) = p.reserve.filter(|_| per_project.contains_key(id)) {
                reserved.push((id.clone(), r.ram_mb as f64));
            }
        }
        let max_per_project = policy.file.max_agents_per_project.map(|n| n as usize);
        drop(policy);
        let gauges = &snap["gauges"];
        let saturated: Vec<&'static str> = ["cpu", "ram", "disk", "space"].into_iter().filter(|k| gauges[*k] == "saturated").collect();
        let mut room = Room {
            cap: Capacity { running: 0, per_project, max_agents, max_per_project, avail_mb: snap["machine"]["mem_avail_mb"].as_f64(), floor_mb: floor, reserved, saturated },
            kinds,
            private,
            effective: self.effective_map(),
        };
        // Spawns under way hold a place in the sub-session caps until their session exists; tasks starting are among them
        // when a program made the run. Counted once: the larger of the two.
        room.cap.running = room.kinds.len() + starting.len().max(self.subs.in_flight());
        let pending: f64 = starting.iter().map(|k| room.agent_cost(k)).sum::<f64>()
            + young.iter().map(|(id, k)| (room.agent_cost(k) - room.private.get(id).copied().unwrap_or(0.0)).max(0.0)).sum::<f64>();
        room.cap.avail_mb = room.cap.avail_mb.map(|a| a - pending);
        room
    }

    /// A candidate for work in `project` that `parent` (the session whose program asks, if any) opens: its priority is its
    /// project's, never above `parent`'s; it may use its project's reservation and its parent's.
    fn admission_candidate(&self, room: &Room, key: String, project: Option<String>, parent: Option<&str>, kind: &str, waiting_since: u64) -> Candidate {
        let above = parent.and_then(|p| room.effective.get(p));
        let effective = self.priorities.lock().effective(&Who { id: key.clone(), project: project.clone() }, now_millis(), above);
        let owners = project.iter().cloned().chain(parent.map(str::to_string)).collect();
        Candidate { key, project, tier: effective.tier, weight: effective.weight, boosted: effective.boosted, owners, waiting_since, cost_mb: room.agent_cost(kind) }
    }

    /// The ready run tasks as candidates, and the gates already decided without the queue: a task on another computer is not
    /// this one's to hold back, and a task of a run a program made must first fit the sub-session limits.
    fn task_candidates(&self, room: &Room, finished: &HashSet<String>) -> (Vec<Candidate>, HashMap<String, runs::Gate>) {
        let now = now_millis();
        let mut candidates = Vec::new();
        let mut decided = HashMap::new();
        let ready = self.runs.lock().ready();
        for t in ready {
            let key = format!("{}/{}", t.run, t.task);
            if let Some(creator) = t.creator.as_deref() {
                let gate = self.run_gate(creator, finished);
                if gate != runs::Gate::Open {
                    decided.insert(key, gate);
                    continue;
                }
            }
            if t.remote {
                decided.insert(key, runs::Gate::Open);
                continue;
            }
            let since = if t.waiting_since == 0 { now } else { t.waiting_since };
            candidates.push(self.admission_candidate(room, key, t.project.or(Some(t.cwd)), t.creator.as_deref(), &t.kind, since));
        }
        (candidates, decided)
    }

    /// The gate for every ready run task this tick, by `run/task`. A task that became ready after this was worked out is not
    /// in it, and waits for the next tick.
    pub(super) fn admission_for_tasks(&self) -> HashMap<String, runs::Gate> {
        if self.runs.lock().ready().is_empty() {
            return HashMap::new();
        }
        let room = self.admission_room();
        let finished = self.finished_task_sessions();
        let (candidates, mut gates) = self.task_candidates(&room, &finished);
        for (c, r) in admission::plan(candidates, &room.cap, now_millis()) {
            gates.insert(c.key, match r {
                Ok(()) => runs::Gate::Open,
                Err(why) => runs::Gate::Wait(why),
            });
        }
        gates
    }

    /// Whether a program in session `caller` may open an agent (`kind`, when known) now, in line with the run tasks that
    /// wait; why not, in words it can act on.
    pub(super) fn admit_sub_session(&self, caller: &str, spec: &SessionSpec, kind: Option<&str>) -> Result<()> {
        let room = self.admission_room();
        let finished = self.finished_task_sessions();
        let (mut candidates, _) = self.task_candidates(&room, &finished);
        let key = format!("sub:{caller}");
        let project = spec.project_id.clone().or(Some(spec.cwd.clone()));
        candidates.push(self.admission_candidate(&room, key.clone(), project, Some(caller), kind.unwrap_or(&spec.kind), now_millis()));
        match admission::plan(candidates, &room.cap, now_millis()).into_iter().find(|(c, _)| c.key == key).map(|(_, r)| r) {
            Some(Err(why)) => bail!("{why}; wait with kelpie_wait, or close a session"),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon(tag: &str) -> (Arc<Daemon>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelpie-admission-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        (Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new()), dir)
    }

    fn step(d: &Arc<Daemon>, now: u64) -> Vec<Action> {
        let admission = d.admission_for_tasks();
        let gate = |ready: runs::Ready<'_>, _: &HashSet<String>| admission.get(&format!("{}/{}", ready.run, ready.task)).cloned().unwrap_or(runs::Gate::Later);
        d.runs.lock().step_gated(now, |_| None, gate).0
    }

    fn started(actions: &[Action]) -> usize {
        actions.iter().filter(|a| matches!(a, Action::Start { .. })).count()
    }

    fn run(d: &Daemon, dir: &std::path::Path, tasks: Value) {
        let new: NewRun = serde_json::from_value(json!({"name": "R", "cwd": dir.to_string_lossy(), "tasks": tasks})).unwrap();
        d.runs.lock().list.push(runs::build_run(new, "r1".into(), 0).unwrap());
    }

    #[test]
    fn a_run_of_four_parallel_tasks_starts_two_under_a_limit_of_two_then_the_rest() {
        let (d, dir) = daemon("four");
        d.priority_rpc("policy.resources.set", &json!({"max_agents": 2})).unwrap();
        let tasks: Vec<Value> = ["a", "b", "c", "d"].iter().map(|id| json!({"id": id, "title": id, "prompt": "p", "kind": "codex"})).collect();
        run(&d, &dir, json!(tasks));
        assert_eq!(started(&step(&d, 1)), 2, "two places");
        let notes: Vec<Option<String>> = d.runs.lock().list[0].tasks.iter().map(|t| t.note.clone()).collect();
        assert_eq!(notes.iter().flatten().filter(|n| n.contains("2 agents are running")).count(), 2, "{notes:?}");
        assert!(notes.iter().flatten().any(|n| n.ends_with("(in line: 1)")) && notes.iter().flatten().any(|n| n.ends_with("(in line: 2)")));
        // Starting tasks hold their places on the next tick too.
        assert_eq!(started(&step(&d, 2)), 0);
        // The two finish: the other two start.
        for t in d.runs.lock().list[0].tasks.iter_mut().filter(|t| t.status == TaskStatus::Starting) {
            t.status = TaskStatus::Done;
        }
        assert_eq!(started(&step(&d, 3)), 2);
        assert!(d.runs.lock().list[0].tasks.iter().all(|t| t.note.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_that_becomes_ready_within_a_step_waits_for_the_next_look() {
        let (d, dir) = daemon("chain");
        d.priority_rpc("policy.resources.set", &json!({"max_agents": 1})).unwrap();
        run(&d, &dir, json!([{"id": "a", "title": "a", "prompt": "p", "kind": "codex"}, {"id": "b", "title": "b", "prompt": "p", "kind": "codex", "deps": ["a"]}]));
        assert_eq!(started(&step(&d, 1)), 1);
        // `a` is done: `b` becomes ready inside the next step, which did not ask the queue about it. It is not started
        // unchecked there, and the step after starts it.
        d.runs.lock().list[0].tasks[0].status = TaskStatus::Done;
        assert_eq!(started(&step(&d, 2)), 1, "asked about at once: it was ready before the step");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_limit_holds_before_the_first_sample() {
        let (d, dir) = daemon("blind");
        let tasks: Vec<Value> = (0..40).map(|i| json!({"id": format!("t{i}"), "title": "t", "prompt": "p", "kind": "codex"})).collect();
        run(&d, &dir, json!(tasks));
        let n = started(&step(&d, 1));
        assert!((2..=32).contains(&n), "the automatic limit from the system's own figure: {n}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_background_project_waits_while_the_computer_is_busy_and_others_do_not() {
        let (d, dir) = daemon("bg");
        d.priority_rpc("policy.resources.set", &json!({"projects": {"docs": {"tier": "background"}}})).unwrap();
        let reading = |cpu: f64| crate::resources::MachineReading { cores: 4, cpu_pct: Some(cpu), ..Default::default() };
        d.resources.record(0, reading(10.0), &[], vec![], 2.0);
        assert_eq!(d.busy_for_background(Some("docs")), None, "calm");
        d.resources.record(2_000, reading(90.0), &[], vec![], 2.0);
        d.resources.record(4_000, reading(90.0), &[], vec![], 2.0);
        assert_eq!(d.busy_for_background(Some("docs")), Some("the CPU"));
        assert_eq!(d.busy_for_background(Some("web")), None, "a normal project goes ahead");
        assert_eq!(d.busy_for_background(None), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
