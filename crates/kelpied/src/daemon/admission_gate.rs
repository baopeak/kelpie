//! The admission queue in the daemon (rules in `admission.rs`): what the computer can take now, the candidates waiting
//! (run tasks every tick; a sub-session when a program asks for one), and the answer each gets.

use super::*;
use crate::admission::{self, Candidate, Capacity};
use crate::resource_policy::Who;

impl Daemon {
    /// What an agent of `kind` takes when it starts: the mean private memory of the sessions running it now (what they
    /// hold, which is more than at the start: an estimate that errs high), or the prior.
    fn agent_cost(&self, kind: &str, kinds: &HashMap<String, String>, private: &HashMap<String, f64>) -> f64 {
        let seen: Vec<f64> = kinds.iter().filter(|(_, k)| k.as_str() == kind).filter_map(|(id, _)| private.get(id).copied()).filter(|mb| *mb > 0.0).collect();
        if seen.is_empty() {
            admission::AGENT_PRIOR_MB
        } else {
            seen.iter().sum::<f64>() / seen.len() as f64
        }
    }

    /// The computer as the queue sees it, with each live local agent session's kind and private memory (for the cost of a
    /// start). Tasks already starting count as running.
    fn admission_capacity(&self) -> (Capacity, HashMap<String, String>, HashMap<String, f64>) {
        let snap = self.resources.snapshot(false, None);
        let private: HashMap<String, f64> = snap["sessions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| Some((s["id"].as_str()?.to_string(), s["now"]["private_mb"].as_f64()?)))
            .collect();
        let mut kinds = HashMap::new();
        let mut per_project: HashMap<String, usize> = HashMap::new();
        let mut alive = HashSet::new();
        for e in self.sessions.read().values() {
            let info = e.info();
            if !info.alive {
                continue;
            }
            alive.insert(info.spec.id.clone());
            if info.spec.kind == "shell" || info.spec.remote.is_some() {
                continue;
            }
            if let Some(p) = info.spec.project_id.clone() {
                *per_project.entry(p).or_default() += 1;
            }
            kinds.insert(info.spec.id.clone(), info.spec.kind.clone());
        }
        let starting = self.runs.lock().list.iter().flat_map(|r| r.tasks.iter()).filter(|t| t.status == TaskStatus::Starting).count();
        let total = snap["machine"]["mem_total_mb"].as_f64();
        let floor = total.map_or(1024.0, admission::floor_mb);
        let policy = self.priorities.lock();
        let max_agents = match policy.file.max_agents {
            0 => total.map_or(usize::MAX, |t| admission::auto_max_agents(t, floor)),
            n => n as usize,
        };
        // A reservation holds while its owner has a live session: a session's own, or its project's.
        let mut reserved: Vec<(String, f64)> = Vec::new();
        for (id, s) in &policy.file.sessions {
            if let Some(r) = s.reserve.filter(|_| alive.contains(id)) {
                reserved.push((id.clone(), r.ram_mb as f64));
            }
        }
        let live_projects: HashSet<&String> = per_project.keys().collect();
        for (id, p) in &policy.file.projects {
            if let Some(r) = p.reserve.filter(|_| live_projects.contains(id)) {
                reserved.push((id.clone(), r.ram_mb as f64));
            }
        }
        let max_per_project = policy.file.max_agents_per_project.map(|n| n as usize);
        drop(policy);
        let gauges = &snap["gauges"];
        let saturated: Vec<&'static str> = ["cpu", "ram", "disk", "space"].into_iter().filter(|k| gauges[*k] == "saturated").collect();
        let cap = Capacity {
            running: kinds.len() + starting,
            per_project,
            max_agents,
            max_per_project,
            avail_mb: snap["machine"]["mem_avail_mb"].as_f64(),
            floor_mb: floor,
            reserved,
            saturated,
        };
        (cap, kinds, private)
    }

    /// A candidate for work in `project` that `parent` (the session whose program asks, if any) opens: its priority is its
    /// project's, never above `parent`'s.
    fn admission_candidate(&self, key: String, project: Option<String>, parent: Option<&str>, kind: &str, waiting_since: u64, costs: (&HashMap<String, String>, &HashMap<String, f64>)) -> Candidate {
        let effective = {
            let above = parent.and_then(|p| self.effective_priority(p));
            self.priorities.lock().effective(&Who { id: key.clone(), project: project.clone() }, now_millis(), above.as_ref())
        };
        // The reservation it may use first: its parent session's, else its project's.
        let owner = parent
            .filter(|p| self.priorities.lock().file.sessions.get(*p).is_some_and(|s| s.reserve.is_some()))
            .map(str::to_string)
            .or_else(|| project.clone());
        Candidate {
            key,
            project,
            tier: effective.tier,
            weight: effective.weight,
            boosted: effective.boosted,
            owner,
            waiting_since,
            cost_mb: self.agent_cost(kind, costs.0, costs.1),
        }
    }

    /// The queue's answer for every ready run task this tick, by `run/task`. A task waiting for the first time waits from now.
    pub(super) fn admission_for_tasks(&self) -> HashMap<String, Result<(), String>> {
        let ready = self.runs.lock().ready();
        if ready.is_empty() {
            return HashMap::new();
        }
        let now = now_millis();
        let (cap, kinds, private) = self.admission_capacity();
        let candidates: Vec<Candidate> = ready
            .into_iter()
            .map(|t| {
                let since = if t.waiting_since == 0 { now } else { t.waiting_since };
                let project = t.project.or(Some(t.cwd));
                self.admission_candidate(format!("{}/{}", t.run, t.task), project, t.creator.as_deref(), &t.kind, since, (&kinds, &private))
            })
            .collect();
        admission::plan(candidates, &cap, now).into_iter().map(|(c, r)| (c.key, r)).collect()
    }

    /// Whether a program in session `caller` may open an agent (`kind`, when known) now; why not, in words it can act on.
    pub(super) fn admit_sub_session(&self, caller: &str, spec: &SessionSpec, kind: Option<&str>) -> Result<()> {
        let (cap, kinds, private) = self.admission_capacity();
        let project = spec.project_id.clone().or(Some(spec.cwd.clone()));
        let c = self.admission_candidate(format!("sub:{caller}"), project, Some(caller), kind.unwrap_or(&spec.kind), now_millis(), (&kinds, &private));
        match admission::plan(vec![c], &cap, now_millis()).pop().map(|(_, r)| r) {
            Some(Err(why)) => bail!("{}; wait with kelpie_wait, or close a session", why.trim_start_matches("queued (1st in line): ")),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_of_four_parallel_tasks_starts_two_under_a_limit_of_two_then_the_rest() {
        let dir = std::env::temp_dir().join(format!("kelpie-admission-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        d.priority_rpc("policy.resources.set", &json!({"max_agents": 2})).unwrap();
        let folder = dir.to_string_lossy().into_owned();
        let tasks: Vec<Value> = ["a", "b", "c", "d"].iter().map(|id| json!({"id": id, "title": id, "prompt": "p", "kind": "codex"})).collect();
        let new: NewRun = serde_json::from_value(json!({"name": "R", "cwd": folder, "tasks": tasks})).unwrap();
        d.runs.lock().list.push(runs::build_run(new, "r1".into(), 0).unwrap());
        let step = |now: u64| {
            let admission = d.admission_for_tasks();
            let gate = |ready: runs::Ready<'_>, _: &HashSet<String>| match admission.get(&format!("{}/{}", ready.run, ready.task)) {
                Some(Err(why)) => runs::Gate::Wait(why.clone()),
                _ => runs::Gate::Open,
            };
            d.runs.lock().step_gated(now, |_| None, gate).0
        };
        let started = |actions: &[Action]| actions.iter().filter(|a| matches!(a, Action::Start { .. })).count();
        assert_eq!(started(&step(1)), 2, "two places");
        let notes: Vec<Option<String>> = d.runs.lock().list[0].tasks.iter().map(|t| t.note.clone()).collect();
        assert_eq!(notes.iter().filter(|n| n.as_deref().is_some_and(|n| n.contains("2 agents are running"))).count(), 2, "{notes:?}");
        assert!(notes.iter().flatten().any(|n| n.starts_with("queued (1st in line)")) && notes.iter().flatten().any(|n| n.starts_with("queued (2nd in line)")));
        // Starting tasks hold their places on the next tick too.
        assert_eq!(started(&step(2)), 0);
        // The two finish: the other two start.
        for t in d.runs.lock().list[0].tasks.iter_mut().filter(|t| t.status == TaskStatus::Starting) {
            t.status = TaskStatus::Done;
        }
        assert_eq!(started(&step(3)), 2);
        assert!(d.runs.lock().list[0].tasks.iter().all(|t| t.note.is_none()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_background_project_waits_while_the_computer_is_busy_and_others_do_not() {
        let dir = std::env::temp_dir().join(format!("kelpie-admission-bg-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
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
