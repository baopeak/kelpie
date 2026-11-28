//! Who may start an agent now: one queue for the work Kelpie starts on its own (run tasks, sub-sessions, automations).
//! A session a person opens is never held back here, but it counts.
//!
//! Each candidate carries its effective priority (`resource_policy`), the memory an agent of its kind takes when it starts
//! (learned from the sessions running it, a prior of 400 MiB otherwise) and when it began to wait. The queue orders by the
//! priority, raised one tier (doubled) for every 15 minutes of waiting so background work is never starved (a boosted
//! session is never overtaken that way), then by who waited longest. Then, in that order, a candidate starts when:
//!
//! - fewer agents run than the computer's limit (and its project's, when one is set);
//! - no resource that a start needs (CPU, memory, disk, disk space) is saturated;
//! - the memory available after what is kept free for the system and for other live reservations covers its start.
//!   A project or session with a reservation of its own starts against that first.
//!
//! Critical work (a critical tier, or a boost) skips the last two checks, keeping only a small margin of memory free: it
//! may use what is kept free for others. Each start takes its share before the next candidate is looked at, so one tick
//! never starts more than fits.

use std::collections::HashMap;

use crate::resource_policy::{Tier, BOOST_WEIGHT};

/// What an agent takes when it starts, before any measurement of one.
pub const AGENT_PRIOR_MB: f64 = 400.0;
/// A wait this long raises a candidate one tier.
pub const AGING_MS: u64 = 15 * 60 * 1000;
/// What critical work leaves free all the same.
const CRITICAL_MARGIN_MB: f64 = 256.0;
/// The automatic limit on running agents: what the memory beyond the system's share holds at the prior, within these.
const AUTO_MIN: usize = 2;
const AUTO_MAX: usize = 32;

/// The computer as the queue sees it at one moment.
#[derive(Debug, Clone, Default)]
pub struct Capacity {
    /// Agent sessions running now (a person's included).
    pub running: usize,
    pub per_project: HashMap<String, usize>,
    pub max_agents: usize,
    pub max_per_project: Option<usize>,
    /// Memory available now, MiB (none: this computer cannot tell, and memory is not checked).
    pub avail_mb: Option<f64>,
    /// Memory kept free for the system and the window, MiB.
    pub floor_mb: f64,
    /// Live reservations: (owner, MiB). An owner is a project or a session id.
    pub reserved: Vec<(String, f64)>,
    /// Resources a start needs that are saturated now (`cpu`, `ram`, `disk`, `space`).
    pub saturated: Vec<&'static str>,
}

/// The automatic limit on running agents for a computer with `total_mb` of memory: what is left after the system's
/// share holds at an agent's usual start, from 2 to 32.
pub fn auto_max_agents(total_mb: f64, floor_mb: f64) -> usize {
    (((total_mb - floor_mb).max(0.0) / AGENT_PRIOR_MB) as usize).clamp(AUTO_MIN, AUTO_MAX)
}

/// The memory kept free for the system on a computer of `total_mb`: 12 %, at least 1 GiB.
pub fn floor_mb(total_mb: f64) -> f64 {
    (total_mb * 0.12).max(1024.0)
}

#[derive(Debug, Clone)]
pub struct Candidate {
    /// What it is, for the answer (`run/task`, `sub:<caller>`).
    pub key: String,
    pub project: Option<String>,
    pub tier: Tier,
    pub weight: f64,
    pub boosted: bool,
    /// The projects and sessions whose reservations it may use (its own project's, the session that opens it).
    pub owners: Vec<String>,
    /// When it began to wait (ms since the epoch; `now` for one asking for the first time).
    pub waiting_since: u64,
    /// What its agent takes when it starts, MiB.
    pub cost_mb: f64,
}

impl Candidate {
    fn critical(&self) -> bool {
        self.boosted || self.tier == Tier::Critical
    }

    /// Its place in the queue: its weight, doubled per 15 minutes of waiting, never past a boost.
    pub fn aged_weight(&self, now: u64) -> f64 {
        if self.boosted {
            return self.weight;
        }
        let steps = (now.saturating_sub(self.waiting_since) / AGING_MS).min(8) as i32;
        (self.weight * 2f64.powi(steps)).min(BOOST_WEIGHT - 0.01).max(self.weight)
    }
}

/// The candidates in queue order, each with `Ok` (it starts now) or why it waits.
pub fn plan(mut candidates: Vec<Candidate>, cap: &Capacity, now: u64) -> Vec<(Candidate, Result<(), String>)> {
    candidates.sort_by(|a, b| b.aged_weight(now).total_cmp(&a.aged_weight(now)).then(a.waiting_since.cmp(&b.waiting_since)));
    let mut running = cap.running;
    let mut per_project = cap.per_project.clone();
    let mut avail = cap.avail_mb;
    let mut waiting = 0;
    let mut out = Vec::with_capacity(candidates.len());
    for c in candidates {
        let why = check(&c, cap, running, &per_project, avail);
        match why {
            None => {
                running += 1;
                if let Some(p) = &c.project {
                    *per_project.entry(p.clone()).or_default() += 1;
                }
                avail = avail.map(|a| a - c.cost_mb);
                out.push((c, Ok(())));
            }
            Some(why) => {
                waiting += 1;
                // Fixed words around the numbers, so the window can translate them; no live figure, so a waiting task's
                // note changes only when its place or its reason does.
                out.push((c, Err(format!("Queued: {why} (in line: {waiting})"))));
            }
        }
    }
    out
}

/// Why `c` cannot start now, or none.
fn check(c: &Candidate, cap: &Capacity, running: usize, per_project: &HashMap<String, usize>, avail: Option<f64>) -> Option<String> {
    if running >= cap.max_agents {
        return Some(format!("{running} agents are running, the limit on this computer"));
    }
    if let (Some(max), Some(p)) = (cap.max_per_project, &c.project) {
        let n = per_project.get(p).copied().unwrap_or(0);
        if n >= max {
            return Some(format!("{n} agents of this project are running, its own limit"));
        }
    }
    if !c.critical() {
        if let Some(r) = cap.saturated.first() {
            let what = match *r {
                "cpu" => "the CPU",
                "ram" => "memory",
                "disk" => "the disk",
                _ => "disk space",
            };
            return Some(format!("{what} is saturated"));
        }
    }
    // A computer that cannot tell its memory is checked on the counts alone.
    let avail = avail?;
    if c.critical() {
        return (avail - c.cost_mb < CRITICAL_MARGIN_MB).then(|| "too little memory is available, even for critical work".to_string());
    }
    // Others' live reservations stay free; its own project's and its opener's are its to use.
    let others: f64 = cap.reserved.iter().filter(|(owner, _)| !c.owners.contains(owner)).map(|(_, mb)| mb).sum();
    let free = avail - cap.floor_mb - others;
    (c.cost_mb > free).then(|| {
        if others > 0.0 { "not enough memory is free beyond what is kept for the system and reserved for others" } else { "not enough memory is free beyond what is kept for the system" }
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(key: &str, tier: Tier, weight: f64, since: u64) -> Candidate {
        Candidate { key: key.into(), project: Some("p".into()), tier, weight, boosted: false, owners: vec!["p".into()], waiting_since: since, cost_mb: 400.0 }
    }

    fn roomy() -> Capacity {
        Capacity { running: 0, max_agents: 10, avail_mb: Some(8000.0), floor_mb: 1000.0, ..Capacity::default() }
    }

    fn keys(plan: &[(Candidate, Result<(), String>)]) -> Vec<(&str, bool)> {
        plan.iter().map(|(c, r)| (c.key.as_str(), r.is_ok())).collect()
    }

    #[test]
    fn the_higher_priority_starts_first_then_who_waited_longest() {
        let cap = Capacity { max_agents: 2, ..roomy() };
        let p = plan(vec![cand("bg", Tier::Background, 1.0, 0), cand("late", Tier::Normal, 2.0, 50), cand("early", Tier::Normal, 2.0, 10)], &cap, 100);
        assert_eq!(keys(&p), [("early", true), ("late", true), ("bg", false)]);
        assert_eq!(p[2].1.as_ref().unwrap_err(), "Queued: 2 agents are running, the limit on this computer (in line: 1)");
    }

    #[test]
    fn a_long_wait_raises_background_work_but_never_past_a_boost() {
        let mut bg = cand("bg", Tier::Background, 1.0, 0);
        assert_eq!(bg.aged_weight(AGING_MS - 1), 1.0);
        assert_eq!(bg.aged_weight(2 * AGING_MS), 4.0, "two tiers up after half an hour");
        assert!(bg.aged_weight(10 * AGING_MS) < BOOST_WEIGHT);
        bg.waiting_since = 0;
        let boosted = Candidate { boosted: true, weight: BOOST_WEIGHT, ..cand("boost", Tier::Critical, BOOST_WEIGHT, 10 * AGING_MS) };
        let p = plan(vec![bg, boosted], &Capacity { max_agents: 1, ..roomy() }, 10 * AGING_MS);
        assert_eq!(keys(&p), [("boost", true), ("bg", false)]);
    }

    #[test]
    fn memory_kept_for_the_system_and_for_others_reservations_is_not_given_away() {
        // 2000 available, 1000 kept for the system, 700 reserved for another project: 300 free, an agent needs 400.
        let cap = Capacity { avail_mb: Some(2000.0), reserved: vec![("other".into(), 700.0)], ..roomy() };
        let p = plan(vec![cand("a", Tier::Normal, 2.0, 0)], &cap, 0);
        assert!(p[0].1.as_ref().unwrap_err().contains("kept for the system and reserved for others"), "{:?}", p[0].1);
        // Its own project's reservation is its to use.
        let own = Candidate { owners: vec!["other".into()], ..cand("a", Tier::Normal, 2.0, 0) };
        assert!(plan(vec![own], &cap, 0)[0].1.is_ok());
        // Critical work may use what is kept free, but leaves a margin.
        assert!(plan(vec![cand("c", Tier::Critical, 4.0, 0)], &cap, 0)[0].1.is_ok());
        let tight = Capacity { avail_mb: Some(500.0), ..cap };
        assert!(plan(vec![cand("c", Tier::Critical, 4.0, 0)], &tight, 0)[0].1.is_err());
    }

    #[test]
    fn each_start_takes_its_share_before_the_next_is_looked_at() {
        let cap = Capacity { avail_mb: Some(2000.0), ..roomy() };
        let p = plan(vec![cand("a", Tier::Normal, 2.0, 0), cand("b", Tier::Normal, 2.0, 1), cand("c", Tier::Normal, 2.0, 2)], &cap, 3);
        assert_eq!(keys(&p), [("a", true), ("b", true), ("c", false)], "1000 free: two agents of 400");
    }

    #[test]
    fn a_saturated_resource_holds_back_all_but_critical_work_and_a_project_has_its_limit() {
        let cap = Capacity { saturated: vec!["disk"], ..roomy() };
        let p = plan(vec![cand("n", Tier::Normal, 2.0, 0), cand("c", Tier::Critical, 4.0, 0)], &cap, 0);
        assert_eq!(keys(&p), [("c", true), ("n", false)]);
        assert!(p[1].1.as_ref().unwrap_err().contains("the disk is saturated"));
        let cap = Capacity { max_per_project: Some(1), per_project: HashMap::from([("p".into(), 1)]), ..roomy() };
        assert!(plan(vec![cand("n", Tier::Normal, 2.0, 0)], &cap, 0)[0].1.as_ref().unwrap_err().contains("of this project"));
        // A computer that cannot tell its memory checks only the counts.
        let blind = Capacity { avail_mb: None, saturated: vec![], ..roomy() };
        assert!(plan(vec![cand("n", Tier::Normal, 2.0, 0)], &blind, 0)[0].1.is_ok());
    }

    #[test]
    fn the_automatic_limit_follows_the_memory() {
        assert_eq!(floor_mb(8083.0), 1024.0);
        assert_eq!(auto_max_agents(8083.0, floor_mb(8083.0)), 17);
        assert_eq!(auto_max_agents(2048.0, 1024.0), 2);
        assert_eq!(auto_max_agents(131_072.0, floor_mb(131_072.0)), 32);
    }
}
