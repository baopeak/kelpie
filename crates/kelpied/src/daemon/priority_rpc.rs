//! `policy.resources.*` and `session.priority.set`: the person's ranking of projects and sessions (rules and state in
//! `resource_policy.rs`). The user's only: a program in a session is given none of these (see `auth`), so an agent can
//! neither raise itself nor lower another. The `policy.resources.changed` event follows every change.

use super::*;
use crate::resource_policy::{parse_boost, Effective, Machine, Reserve, Tier, TierSwitches, Who};

/// Sessions are looked up the chain of parents at most this far (sub-sessions nest three deep).
const MAX_CHAIN: usize = 8;

fn parse_reserve(v: &Value) -> Result<Option<Reserve>> {
    if v.is_null() {
        return Ok(None);
    }
    let r: Reserve = serde_json::from_value(v.clone()).map_err(|e| anyhow!("a reservation is {{cores, ram_mb, vram_mb}}: {e}"))?;
    anyhow::ensure!(r.cores.is_finite() && r.cores >= 0.0, "cores must be a number from 0");
    Ok(Some(r))
}

fn parse_tier(v: &Value) -> Result<Tier> {
    v.as_str().and_then(Tier::parse).ok_or_else(|| anyhow!("a tier is critical, normal or background"))
}

impl Daemon {
    /// Session `id`'s priority now: its own, its project's, capped by the session that opened it.
    pub fn effective_priority(&self, id: &str) -> Option<Effective> {
        let now = now_millis();
        // The chain from `id` up to the session a person opened.
        let mut chain: Vec<Who> = Vec::new();
        let mut next = Some(id.to_string());
        while let Some(sid) = next.take() {
            if chain.len() >= MAX_CHAIN || chain.iter().any(|w| w.id == sid) {
                break;
            }
            let Some(entry) = self.by_id(&sid) else { break };
            let spec = entry.spec();
            next = spec.parent.clone();
            chain.push(Who { id: spec.id, project: spec.project_id.or(Some(spec.cwd)) });
        }
        if chain.is_empty() {
            return None;
        }
        let policy = self.priorities.lock();
        let mut above: Option<Effective> = None;
        for who in chain.iter().rev() {
            above = Some(policy.effective(who, now, above.as_ref()));
        }
        above
    }

    /// This machine as a reservation must fit it.
    fn reservation_machine(&self) -> Machine {
        let snap = self.resources.snapshot(false, Some(""));
        let cores = snap["machine"]["cores"].as_u64().filter(|c| *c > 0).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as u64)) as u32;
        let ram_mb = snap["machine"]["mem_total_mb"].as_u64().unwrap_or_else(|| {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.total_memory() >> 20
        });
        Machine { ram_mb, cores, vram_mb: None }
    }

    /// The policy, each live session's effective priority, the boosts running and what is reserved of what may be.
    fn priorities_json(&self) -> Value {
        let ids: Vec<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
        let effective: serde_json::Map<String, Value> = ids.iter().filter_map(|id| self.effective_priority(id).map(|e| (id.clone(), json!(e)))).collect();
        let machine = self.reservation_machine();
        let policy = self.priorities.lock();
        json!({
            "policy": policy.json(),
            "effective": effective,
            "boosted": policy.boosted(now_millis()),
            "reserved": policy.reserved(),
            "reservable": machine.reservable(),
            "max_boosts": crate::resource_policy::MAX_BOOSTS,
        })
    }

    fn priorities_changed(&self) {
        self.priorities.lock().save();
        self.broadcast("policy.resources.changed", self.priorities_json());
    }

    /// Ends boosts past their time and forgets the sessions that are gone (once a second, from the ticker).
    pub(super) fn priority_sweep(&self) {
        let changed = {
            // The live sessions are read before the policy is locked: the two locks are never held together.
            let ids: std::collections::HashSet<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
            self.priorities.lock().sweep(&|id: &str| ids.contains(id), now_millis())
        };
        if changed {
            self.priorities_changed();
        }
    }

    /// A turn of session `id` ended: a boost "until this turn ends" ends.
    pub(super) fn priority_turn_ended(&self, id: &str) {
        let ended = self.priorities.lock().turn_ended(id);
        if ended {
            self.priorities_changed();
        }
    }

    /// `policy.resources.get`, `policy.resources.set {projects?, order?, max_agents?, max_agents_per_project?, switches?}` and
    /// `session.priority.set {id, tier?, boost?, replace?, reserve?}`.
    pub(crate) fn priority_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "policy.resources.get" => return Ok(self.priorities_json()),
            "policy.resources.set" => {
                let machine = self.reservation_machine();
                let mut policy = self.priorities.lock();
                let before = policy.file.clone();
                let applied = (|| -> Result<()> {
                    if let Some(projects) = params.get("projects").and_then(Value::as_object) {
                        for (id, p) in projects {
                            let tier = p.get("tier").map(parse_tier).transpose()?;
                            let rank = p.get("rank").map(|r| r.as_u64().map(|r| r as u32).ok_or_else(|| anyhow!("a rank is a whole number"))).transpose()?;
                            let reserve = p.get("reserve").map(parse_reserve).transpose()?;
                            policy.set_project(id, tier, rank, reserve).map_err(|e| anyhow!(e))?;
                        }
                    }
                    if let Some(order) = params.get("order") {
                        let tier = parse_tier(order.get("tier").unwrap_or(&Value::Null))?;
                        let ids: Vec<String> = order.get("ids").and_then(Value::as_array).ok_or_else(|| anyhow!("order needs ids"))?.iter().filter_map(Value::as_str).map(str::to_string).collect();
                        policy.order(tier, &ids);
                    }
                    if let Some(n) = params.get("max_agents") {
                        policy.file.max_agents = n.as_u64().filter(|n| *n <= 64).ok_or_else(|| anyhow!("max_agents is a whole number from 0 (automatic) to 64"))? as u32;
                    }
                    if let Some(n) = params.get("max_agents_per_project") {
                        policy.file.max_agents_per_project = if n.is_null() { None } else { Some(n.as_u64().filter(|n| (1..=64).contains(n)).ok_or_else(|| anyhow!("max_agents_per_project is 1 to 64, or null"))? as u32) };
                    }
                    if let Some(s) = params.get("switches") {
                        policy.file.switches = serde_json::from_value::<TierSwitches>(s.clone()).map_err(|e| anyhow!("switches: {e}"))?;
                    }
                    policy.check_reservations(&machine).map_err(|e| anyhow!(e))
                })();
                if let Err(e) = applied {
                    policy.file = before;
                    return Err(e);
                }
            }
            "session.priority.set" => {
                let id = param(params, "id")?.as_str().unwrap_or_default().to_string();
                anyhow::ensure!(self.by_id(&id).is_some(), "no session {id}");
                let tier = match params.get("tier").and_then(Value::as_str) {
                    None => None,
                    Some("project") => Some(None),
                    Some(t) => Some(Some(Tier::parse(t).ok_or_else(|| anyhow!("a tier is critical, normal, background or project"))?)),
                };
                let now = now_millis();
                let boost = params.get("boost").and_then(Value::as_str).map(|b| parse_boost(b, now)).transpose().map_err(|e| anyhow!(e))?;
                let reserve = params.get("reserve").map(parse_reserve).transpose()?;
                let replace = params.get("replace").and_then(Value::as_str);
                let machine = self.reservation_machine();
                let mut policy = self.priorities.lock();
                let before = policy.file.clone();
                let applied = policy.set_session(&id, tier, boost, replace, reserve, now).and_then(|_| policy.check_reservations(&machine));
                if let Err(e) = applied {
                    policy.file = before;
                    bail!(e);
                }
            }
            other => bail!("unknown method {other}"),
        }
        self.priorities_changed();
        Ok(self.priorities_json())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daemon_with(sessions: &[(u32, &str, Option<&str>, Option<&str>)]) -> (Arc<Daemon>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelpie-priority-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let daemon = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        for (handle, id, project, parent) in sessions {
            let spec = SessionSpec {
                id: id.to_string(),
                cwd: dir.to_string_lossy().into_owned(),
                project_id: project.map(str::to_string),
                parent: parent.map(str::to_string),
                ..Default::default()
            };
            daemon.insert_for_test(Entry::Term(Session::new(*handle, spec, 80, 24)));
        }
        (daemon, dir)
    }

    #[test]
    fn the_person_ranks_projects_and_sessions_and_children_follow_their_parent() {
        let (d, dir) = daemon_with(&[(1, "lead", Some("web"), None), (2, "child", Some("web"), Some("lead")), (3, "docs", Some("docs"), None)]);
        d.priority_rpc("policy.resources.set", &json!({"order": {"tier": "critical", "ids": ["web"]}, "projects": {"docs": {"tier": "background"}}})).unwrap();
        assert_eq!(d.effective_priority("lead").unwrap().tier, Tier::Critical);
        assert_eq!(d.effective_priority("docs").unwrap().weight, 1.0);
        d.priority_rpc("session.priority.set", &json!({"id": "lead", "tier": "background"})).unwrap();
        let child = d.effective_priority("child").unwrap();
        assert_eq!((child.tier, child.source), (Tier::Background, "parent"), "never above the session that opened it");
        let got = d.priority_rpc("session.priority.set", &json!({"id": "docs", "boost": "turn"})).unwrap();
        assert_eq!(got["boosted"], json!(["docs"]));
        assert_eq!(got["effective"]["docs"]["weight"], 8.0);
        d.priority_turn_ended("docs");
        assert!(!d.effective_priority("docs").unwrap().boosted, "a boost for the turn ends with it");
        // The policy is the daemon's: it holds after a restart.
        drop(d);
        let again = Daemon::with_secrets(dir.clone(), PathBuf::from("kelpied"), notify::Memory::new());
        assert_eq!(again.priorities.lock().file.projects["docs"].tier, Tier::Background);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_refused_change_leaves_the_policy_as_it_was() {
        let (d, dir) = daemon_with(&[(1, "s", Some("web"), None)]);
        let err = d.priority_rpc("policy.resources.set", &json!({"projects": {"web": {"tier": "critical", "reserve": {"ram_mb": 1_000_000}}}})).unwrap_err().to_string();
        assert!(err.contains("reservations together"), "{err}");
        assert!(d.priorities.lock().file.projects.is_empty(), "the tier given with it is not kept either");
        assert!(d.priority_rpc("session.priority.set", &json!({"id": "nobody", "boost": "1h"})).is_err());
        assert!(d.priority_rpc("session.priority.set", &json!({"id": "s", "boost": "5h"})).unwrap_err().to_string().contains("30m, 1h, 2h"));
        assert!(d.priority_rpc("policy.resources.set", &json!({"projects": {"web": {"tier": "urgent"}}})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
