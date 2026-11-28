//! Which work matters most: the person's ranking of projects and sessions, and what is kept free for them
//! (`resource-policy.json`, the daemon's so it holds with the window closed).
//!
//! - **Tier and rank per project:** critical, normal (the default) or background, and an order inside the tier that the
//!   person sets by dragging. The top of a tier weighs 1.5 times its bottom.
//! - **Session override:** a session takes its project's tier and rank unless the person sets its own.
//! - **Boost:** "prioritize this session" for 30 min, 1 h, 2 h or until its current turn ends. A boosted session is the top
//!   of critical. At most two at once, so the rest still get their share, and every boost ends on its own.
//! - **Reservation:** cores, memory and GPU memory kept free for a project or session while it has a live session. All
//!   reservations together must leave the system and the window their share (checked when one is set).
//!
//! [`Policy::effective`] is the one place all of it is turned into a session's priority: admission, the command gate,
//! the contention ladder and the token router read that. A session opened by another (a sub-session, a run's task)
//! never ranks above the session that opened it.
//!
//! Only the person sets any of this (the app, or the phone's boost): an agent can neither raise itself nor lower another.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Boosts at once.
pub const MAX_BOOSTS: usize = 2;
/// A boost "until this turn ends" ends after this long all the same: a shell, or an agent left at its prompt, may never end a turn.
pub const TURN_AT_MOST_MS: u64 = 2 * 60 * 60 * 1000;
/// The most a reservation may name (anything larger is a typing slip, and sums of it must not overflow).
const MAX_RESERVE_MB: u64 = 1 << 30;
const MAX_RESERVE_CORES: f64 = 4096.0;
/// Projects and sessions with a policy of their own.
const MAX_ENTRIES: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Background,
    #[default]
    Normal,
    Critical,
}

impl Tier {
    pub fn parse(s: &str) -> Option<Tier> {
        match s {
            "critical" => Some(Tier::Critical),
            "normal" => Some(Tier::Normal),
            "background" => Some(Tier::Background),
            _ => None,
        }
    }

    /// Its share against the others when resources are divided.
    pub fn weight(self) -> f64 {
        match self {
            Tier::Critical => 4.0,
            Tier::Normal => 2.0,
            Tier::Background => 1.0,
        }
    }
}

/// What a boosted session weighs.
pub const BOOST_WEIGHT: f64 = 8.0;

/// Resources kept free for a project or a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Reserve {
    pub cores: f64,
    pub ram_mb: u64,
    pub vram_mb: u64,
}

impl Reserve {
    fn is_empty(&self) -> bool {
        self.cores <= 0.0 && self.ram_mb == 0 && self.vram_mb == 0
    }

    fn add(&mut self, other: &Reserve) {
        self.cores += other.cores;
        self.ram_mb = self.ram_mb.saturating_add(other.ram_mb);
        self.vram_mb = self.vram_mb.saturating_add(other.vram_mb);
    }

    /// Whether the figures are ones a person could mean.
    pub fn check(&self) -> Result<(), String> {
        if !self.cores.is_finite() || self.cores < 0.0 || self.cores > MAX_RESERVE_CORES {
            return Err("cores must be a number from 0".into());
        }
        if self.ram_mb > MAX_RESERVE_MB || self.vram_mb > MAX_RESERVE_MB {
            return Err("that is more memory than any computer has".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectPolicy {
    pub tier: Tier,
    /// Place inside its tier: 0 is the top.
    pub rank: u32,
    pub reserve: Option<Reserve>,
}

/// How long a boost lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Boost {
    /// Until this time (ms since the epoch).
    Until(u64),
    /// Until the session's current (or next) turn ends, and this time at the latest.
    Turn(u64),
}

impl Boost {
    /// When it ends at the latest.
    fn deadline(self) -> u64 {
        match self {
            Boost::Until(t) | Boost::Turn(t) => t,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionPolicy {
    /// Its own tier; none: its project's.
    pub tier: Option<Tier>,
    pub boost: Option<Boost>,
    /// When the boost was set (the oldest goes first when a third is asked for and the person says so).
    pub boosted_at: u64,
    pub reserve: Option<Reserve>,
}

impl SessionPolicy {
    fn is_empty(&self) -> bool {
        self.tier.is_none() && self.boost.is_none() && self.reserve.is_none()
    }
}

/// Per tier, what Kelpie may do to that tier's work when resources run short.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Switches {
    /// Lower the parallelism of a heavy command so it fits its share (`cargo build -j 2`).
    pub rewrite: bool,
    /// Give the builds and tests it runs less CPU, memory and disk priority while a resource is saturated.
    pub demote: bool,
    /// End the agent of an idle session to free its memory, keeping its conversation to resume.
    pub park: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TierSwitches {
    pub critical: Switches,
    pub normal: Switches,
    pub background: Switches,
}

impl Default for TierSwitches {
    /// Rewriting is on for every tier (the owner's choice); demoting and parking never touch critical work by default.
    fn default() -> TierSwitches {
        TierSwitches {
            critical: Switches { rewrite: true, demote: false, park: false },
            normal: Switches { rewrite: true, demote: true, park: false },
            background: Switches { rewrite: true, demote: true, park: true },
        }
    }
}

/// `resource-policy.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct File {
    pub projects: BTreeMap<String, ProjectPolicy>,
    pub sessions: BTreeMap<String, SessionPolicy>,
    /// Running agent sessions in all (0: worked out from the machine).
    pub max_agents: u32,
    pub max_agents_per_project: Option<u32>,
    pub switches: TierSwitches,
}

/// What a session needs for its effective priority.
#[derive(Debug, Clone, Default)]
pub struct Who {
    pub id: String,
    pub project: Option<String>,
}

/// A session's priority as everything else reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Effective {
    pub tier: Tier,
    /// Its share when a resource is divided: the tier's weight times its rank factor (1.0–1.5), or 8 when boosted.
    pub weight: f64,
    pub boosted: bool,
    /// When the boost ends (ms since the epoch), or none for "until the turn ends".
    pub boost_until: Option<u64>,
    /// What decided it: `boost`, `session`, `project`, `parent` or `default`.
    pub source: &'static str,
    /// What is kept free for it: its own, or its project's.
    pub reserve: Option<Reserve>,
}

/// The machine a reservation must fit: memory in MiB and cores.
#[derive(Debug, Clone, Copy)]
pub struct Machine {
    pub ram_mb: u64,
    pub cores: u32,
    /// GPU memory, when the machine can tell.
    pub vram_mb: Option<u64>,
}

impl Machine {
    /// What all reservations together may hold: memory above what the system and the window need (the larger of 3 GiB
    /// and 40 %: about 4.7 GiB of an 8 GB machine), every core but one, and GPU memory but a quarter.
    pub fn reservable(&self) -> Reserve {
        let base = (self.ram_mb * 2 / 5).max(3072);
        Reserve {
            cores: f64::from(self.cores.saturating_sub(1)),
            ram_mb: self.ram_mb.saturating_sub(base),
            vram_mb: self.vram_mb.map_or(0, |v| v * 3 / 4),
        }
    }
}

pub struct Policy {
    path: Option<PathBuf>,
    pub file: File,
    /// The file is there but could not be read: nothing is written over it.
    unreadable: bool,
    /// Counts changes, so a client keeps the newest of two answers that crossed.
    pub seq: u64,
    /// The sessions the last change notice knew of: a session that starts or closes is announced with its priority.
    pub known: std::collections::BTreeSet<String>,
}

/// `30m`, `1h`, `2h` → ms; `turn` → until the turn ends.
pub fn parse_boost(s: &str, now: u64) -> Result<Option<Boost>, String> {
    let minutes = match s {
        "off" => return Ok(None),
        "turn" => return Ok(Some(Boost::Turn(now + TURN_AT_MOST_MS))),
        "30m" => 30,
        "1h" => 60,
        "2h" => 120,
        other => return Err(format!("a boost lasts 30m, 1h, 2h or turn (until this turn ends), not `{other}`")),
    };
    Ok(Some(Boost::Until(now + minutes * 60_000)))
}

impl Policy {
    /// The policy in `path` (from its previous version when damaged). One that can be read neither way is left alone:
    /// the policy starts from the defaults and writes nothing over the file.
    pub fn open(path: Option<PathBuf>) -> Policy {
        let (mut file, unreadable) = match path.as_deref().map(crate::persist::load_state::<File>) {
            Some(Ok(Some(f))) => (f, false),
            Some(Err(e)) => {
                tracing::warn!("resource policy not read ({e}); the defaults hold until the daemon restarts and nothing is saved over it");
                (File::default(), true)
            }
            _ => (File::default(), false),
        };
        // A turn that was running when the daemon stopped never reports its end to this one.
        for s in file.sessions.values_mut() {
            if matches!(s.boost, Some(Boost::Turn(_))) {
                s.boost = None;
            }
        }
        file.sessions.retain(|_, s| !s.is_empty());
        Policy { path, file, unreadable, seq: 0, known: Default::default() }
    }

    /// What to write and where (none when there is no file, or it could not be read and is left alone): the caller
    /// writes it with [`save_file`] after letting go of the policy.
    pub fn to_save(&self) -> Option<(PathBuf, File)> {
        self.path.clone().filter(|_| !self.unreadable).map(|p| (p, self.file.clone()))
    }

    #[cfg(test)]
    pub fn save(&self) {
        if let Some((path, file)) = self.to_save() {
            save_file(&path, &file);
        }
    }

    /// A project's tier and its rank factor: 1.5 at the top of its tier, 1.0 at the bottom.
    fn project(&self, project: &str) -> (Tier, f64, Option<Reserve>) {
        let Some(p) = self.file.projects.get(project) else { return (Tier::Normal, 1.0, None) };
        let mut ranks: Vec<u32> = self.file.projects.values().filter(|q| q.tier == p.tier).map(|q| q.rank).collect();
        ranks.sort_unstable();
        ranks.dedup();
        let place = ranks.iter().position(|r| *r == p.rank).unwrap_or(0);
        let factor = if ranks.len() <= 1 { 1.0 } else { 1.5 - 0.5 * place as f64 / (ranks.len() - 1) as f64 };
        (p.tier, factor, p.reserve)
    }

    /// A boost still running at `now`.
    fn live_boost(&self, id: &str, now: u64) -> Option<Boost> {
        self.file.sessions.get(id)?.boost.filter(|b| b.deadline() > now)
    }

    /// `who`'s priority at `now`; `parent` gives the priority of the session that opened it (its ceiling).
    pub fn effective(&self, who: &Who, now: u64, parent: Option<&Effective>) -> Effective {
        let own = self.file.sessions.get(&who.id);
        let (ptier, factor, preserve) = who.project.as_deref().map_or((Tier::Normal, 1.0, None), |p| self.project(p));
        let reserve = own.and_then(|s| s.reserve).or(preserve);
        let mut e = if let Some(boost) = self.live_boost(&who.id, now) {
            Effective {
                tier: Tier::Critical,
                weight: BOOST_WEIGHT,
                boosted: true,
                boost_until: match boost {
                    Boost::Until(t) => Some(t),
                    Boost::Turn(_) => None,
                },
                source: "boost",
                reserve,
            }
        } else if let Some(tier) = own.and_then(|s| s.tier) {
            Effective { tier, weight: tier.weight() * if tier == ptier { factor } else { 1.0 }, boosted: false, boost_until: None, source: "session", reserve }
        } else {
            let known = who.project.as_deref().is_some_and(|p| self.file.projects.contains_key(p));
            Effective { tier: ptier, weight: ptier.weight() * factor, boosted: false, boost_until: None, source: if known { "project" } else { "default" }, reserve }
        };
        if let Some(p) = parent {
            if (e.weight, e.tier) > (p.weight, p.tier) {
                // Capped by the session that opened it. Its own boost still shows (and holds one of the two places),
                // so the person sees it and can end it.
                e = Effective { source: "parent", reserve: e.reserve, boosted: e.boosted, boost_until: e.boost_until, ..p.clone() };
            }
        }
        e
    }

    /// Sessions boosted at `now`, oldest boost first.
    pub fn boosted(&self, now: u64) -> Vec<String> {
        let mut ids: Vec<(&String, u64)> =
            self.file.sessions.iter().filter(|(id, _)| self.live_boost(id, now).is_some()).map(|(id, s)| (id, s.boosted_at)).collect();
        ids.sort_by_key(|(_, at)| *at);
        ids.into_iter().map(|(id, _)| id.clone()).collect()
    }

    /// Sets a session's tier (`Some(None)`: back to its project's), boost and reservation. A third boost is refused
    /// naming the two running, unless `replace` names one of them, which then ends.
    pub fn set_session(&mut self, id: &str, tier: Option<Option<Tier>>, boost: Option<Option<Boost>>, replace: Option<&str>, reserve: Option<Option<Reserve>>, now: u64) -> Result<(), String> {
        if let Some(Some(_)) = boost {
            let others: Vec<String> = self.boosted(now).into_iter().filter(|b| b != id).collect();
            if others.len() >= MAX_BOOSTS {
                match replace.filter(|r| others.iter().any(|o| o == r)) {
                    Some(r) => {
                        if let Some(s) = self.file.sessions.get_mut(r) {
                            s.boost = None;
                        }
                    }
                    None => return Err(format!("{} sessions are boosted already ({}); end one, or name it in `replace`", MAX_BOOSTS, others.join(", "))),
                }
            }
        }
        if !self.file.sessions.contains_key(id) && self.file.sessions.len() + self.file.projects.len() >= MAX_ENTRIES {
            return Err("too many sessions and projects have a priority of their own".into());
        }
        let s = self.file.sessions.entry(id.to_string()).or_default();
        if let Some(t) = tier {
            s.tier = t;
        }
        if let Some(b) = boost {
            s.boost = b;
            if b.is_some() {
                s.boosted_at = now;
            }
        }
        if let Some(r) = reserve {
            s.reserve = r.filter(|r| !r.is_empty());
        }
        if s.is_empty() {
            self.file.sessions.remove(id);
        }
        Ok(())
    }

    /// Sets a project's tier, rank and reservation (`None` leaves each as it is; `Some(None)` removes the reservation).
    pub fn set_project(&mut self, id: &str, tier: Option<Tier>, rank: Option<u32>, reserve: Option<Option<Reserve>>) -> Result<(), String> {
        if !self.file.projects.contains_key(id) && self.file.sessions.len() + self.file.projects.len() >= MAX_ENTRIES {
            return Err("too many sessions and projects have a priority of their own".into());
        }
        // A project moved to another tier without a place in it goes to that tier's bottom.
        let bottom = |tier: Tier| self.file.projects.iter().filter(|(k, q)| q.tier == tier && k.as_str() != id).map(|(_, q)| q.rank + 1).max().unwrap_or(0);
        let rank = match (tier, rank) {
            (Some(t), None) if self.file.projects.get(id).is_none_or(|p| p.tier != t) => Some(bottom(t)),
            _ => rank,
        };
        // Kept even when it equals the defaults: rank 0 is the top of its tier, which a removal would lose.
        let p = self.file.projects.entry(id.to_string()).or_default();
        if let Some(t) = tier {
            p.tier = t;
        }
        if let Some(r) = rank {
            p.rank = r;
        }
        if let Some(r) = reserve {
            p.reserve = r.filter(|r| !r.is_empty());
        }
        Ok(())
    }

    /// Puts the projects of one tier in this order (the top first), keeping the others' ranks.
    pub fn order(&mut self, tier: Tier, ids: &[String]) {
        for (i, id) in ids.iter().enumerate() {
            let p = self.file.projects.entry(id.clone()).or_default();
            p.tier = tier;
            p.rank = i as u32;
        }
    }

    /// All reservations together.
    pub fn reserved(&self) -> Reserve {
        let mut sum = Reserve::default();
        for r in self.file.projects.values().filter_map(|p| p.reserve.as_ref()).chain(self.file.sessions.values().filter_map(|s| s.reserve.as_ref())) {
            sum.add(r);
        }
        sum
    }

    /// Whether every reservation fits on `machine`: an error with the numbers when it does not.
    pub fn check_reservations(&self, machine: &Machine) -> Result<(), String> {
        let (want, can) = (self.reserved(), machine.reservable());
        let mut over = Vec::new();
        if want.ram_mb > can.ram_mb {
            over.push(format!("memory {} MiB of at most {} MiB ({} MiB total, the rest stays for the system and the window)", want.ram_mb, can.ram_mb, machine.ram_mb));
        }
        if want.cores > can.cores + 1e-9 {
            over.push(format!("{} cores of at most {} (one stays free)", want.cores, can.cores));
        }
        if want.vram_mb > 0 && machine.vram_mb.is_none() {
            over.push("GPU memory, which this computer does not report".to_string());
        } else if want.vram_mb > can.vram_mb {
            over.push(format!("GPU memory {} MiB of at most {} MiB", want.vram_mb, can.vram_mb));
        }
        if over.is_empty() {
            Ok(())
        } else {
            Err(format!("the reservations together would hold {}", over.join("; ")))
        }
    }

    /// Drops what has ended: boosts past their time or on a session whose process ended (no menu offers to end those, and
    /// they would hold a place), and the policies of sessions that are gone. True when anything changed.
    pub fn sweep(&mut self, exists: &dyn Fn(&str) -> bool, alive: &dyn Fn(&str) -> bool, now: u64) -> bool {
        let before = self.file.sessions.clone();
        for (id, s) in self.file.sessions.iter_mut() {
            if s.boost.is_some_and(|b| b.deadline() <= now) || (s.boost.is_some() && !alive(id)) {
                s.boost = None;
            }
        }
        self.file.sessions.retain(|id, s| exists(id) && !s.is_empty());
        self.file.sessions != before
    }

    /// The session's turn ended: a boost "until the turn ends" ends with it. True when one did.
    pub fn turn_ended(&mut self, id: &str) -> bool {
        let Some(s) = self.file.sessions.get_mut(id) else { return false };
        if !matches!(s.boost, Some(Boost::Turn(_))) {
            return false;
        }
        s.boost = None;
        if s.is_empty() {
            self.file.sessions.remove(id);
        }
        true
    }

    pub fn json(&self) -> Value {
        json!(self.file)
    }
}

/// Writes the policy (see [`Policy::to_save`]).
pub fn save_file(path: &std::path::Path, file: &File) {
    if let Err(e) = serde_json::to_vec_pretty(file).map_err(anyhow::Error::from).and_then(|b| crate::persist::save_state(path, &b)) {
        tracing::warn!("resource policy not saved: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(id: &str, project: &str) -> Who {
        Who { id: id.into(), project: Some(project.into()) }
    }

    #[test]
    fn a_boost_beats_a_session_override_which_beats_the_project() {
        let mut p = Policy::open(None);
        p.set_project("web", Some(Tier::Background), None, None).unwrap();
        let e = p.effective(&who("s", "web"), 0, None);
        assert_eq!((e.tier, e.weight, e.source), (Tier::Background, 1.0, "project"));
        p.set_session("s", Some(Some(Tier::Critical)), None, None, None, 0).unwrap();
        let e = p.effective(&who("s", "web"), 0, None);
        assert_eq!((e.tier, e.weight, e.source), (Tier::Critical, 4.0, "session"));
        p.set_session("s", None, Some(parse_boost("30m", 0).unwrap()), None, None, 0).unwrap();
        let e = p.effective(&who("s", "web"), 1000, None);
        assert_eq!((e.tier, e.weight, e.boosted, e.boost_until), (Tier::Critical, BOOST_WEIGHT, true, Some(30 * 60_000)));
        let after = p.effective(&who("s", "web"), 30 * 60_000, None);
        assert_eq!((after.boosted, after.source), (false, "session"), "the boost ended on its own");
        let unknown = p.effective(&who("x", "elsewhere"), 0, None);
        assert_eq!((unknown.tier, unknown.weight, unknown.source), (Tier::Normal, 2.0, "default"));
    }

    #[test]
    fn the_top_of_a_tier_weighs_half_again_its_bottom() {
        let mut p = Policy::open(None);
        p.order(Tier::Critical, &["a".into(), "b".into(), "c".into()]);
        let w = |id: &str| p.effective(&who("s", id), 0, None).weight;
        assert_eq!((w("a"), w("b"), w("c")), (6.0, 5.0, 4.0));
        p.order(Tier::Normal, &["d".into()]);
        assert_eq!(p.effective(&who("s", "d"), 0, None).weight, 2.0, "alone in its tier");
        // The top of normal keeps its place when its reservation goes, and one moved to another tier goes to its bottom.
        p.order(Tier::Normal, &["d".into(), "e".into()]);
        p.set_project("d", None, None, Some(Some(Reserve { ram_mb: 512, ..Reserve::default() }))).unwrap();
        p.set_project("d", None, None, Some(None)).unwrap();
        assert_eq!(p.effective(&who("s", "d"), 0, None).weight, 3.0);
        p.set_project("a", Some(Tier::Normal), None, None).unwrap();
        assert_eq!(p.file.projects["a"].rank, 2, "below d and e");
        assert!(Reserve { ram_mb: u64::MAX, ..Reserve::default() }.check().is_err());
    }

    #[test]
    fn a_child_never_ranks_above_the_session_that_opened_it() {
        let mut p = Policy::open(None);
        p.set_project("web", Some(Tier::Critical), None, None).unwrap();
        p.set_session("parent", Some(Some(Tier::Background)), None, None, None, 0).unwrap();
        let parent = p.effective(&who("parent", "web"), 0, None);
        let child = p.effective(&who("child", "web"), 0, Some(&parent));
        assert_eq!((child.tier, child.weight, child.source), (Tier::Background, 1.0, "parent"));
        // A child's own boost is capped too, but still shows: it holds one of the two places, and must be endable.
        p.set_session("child", None, Some(parse_boost("1h", 0).unwrap()), None, None, 0).unwrap();
        let child = p.effective(&who("child", "web"), 1, Some(&parent));
        assert_eq!((child.weight, child.boosted, child.source), (1.0, true, "parent"));
        // A child set lower than its parent stays lower.
        p.set_session("low", Some(Some(Tier::Background)), None, None, None, 0).unwrap();
        let normal = p.effective(&who("p2", "other"), 0, None);
        assert_eq!(p.effective(&who("low", "other"), 0, Some(&normal)).tier, Tier::Background);
    }

    #[test]
    fn at_most_two_boosts_and_a_third_names_them() {
        let mut p = Policy::open(None);
        let turn = parse_boost("turn", 0).unwrap();
        for (i, id) in ["a", "b"].iter().enumerate() {
            p.set_session(id, None, Some(turn), None, None, i as u64).unwrap();
        }
        let err = p.set_session("c", None, Some(turn), None, None, 5).unwrap_err();
        assert!(err.contains("a, b"), "{err}");
        p.set_session("c", None, Some(turn), Some("a"), None, 5).unwrap();
        assert_eq!(p.boosted(6), ["b", "c"]);
        // Boosting one already boosted again is not a third.
        p.set_session("b", None, Some(parse_boost("1h", 7).unwrap()), None, None, 7).unwrap();
        assert!(parse_boost("3h", 0).is_err());
    }

    #[test]
    fn a_turn_boost_ends_with_the_turn_and_gone_sessions_are_forgotten() {
        let mut p = Policy::open(None);
        p.set_session("a", None, Some(parse_boost("turn", 0).unwrap()), None, None, 0).unwrap();
        p.set_session("b", Some(Some(Tier::Critical)), Some(parse_boost("30m", 0).unwrap()), None, None, 0).unwrap();
        assert!(p.turn_ended("a"));
        assert!(!p.turn_ended("a"));
        assert!(p.file.sessions.get("a").is_none(), "nothing left of it");
        let all = |_: &str| true;
        assert!(p.sweep(&all, &all, 31 * 60_000), "b's boost ran out");
        assert_eq!(p.file.sessions["b"].tier, Some(Tier::Critical), "its own tier stays");
        assert!(p.sweep(&|id| id != "b", &all, 31 * 60_000));
        assert!(p.file.sessions.is_empty());
        // A turn boost ends after two hours at the latest; a boost on a session whose process ended goes at once.
        p.set_session("c", None, Some(parse_boost("turn", 0).unwrap()), None, None, 0).unwrap();
        assert!(!p.sweep(&all, &all, TURN_AT_MOST_MS - 1));
        assert!(p.sweep(&all, &all, TURN_AT_MOST_MS));
        p.set_session("d", Some(Some(Tier::Critical)), Some(parse_boost("2h", 0).unwrap()), None, None, 0).unwrap();
        assert!(p.sweep(&all, &|id| id != "d", 1));
        assert_eq!((p.file.sessions["d"].boost, p.file.sessions["d"].tier), (None, Some(Tier::Critical)));
    }

    #[test]
    fn reservations_must_leave_the_system_its_share() {
        let machine = Machine { ram_mb: 8083, cores: 4, vram_mb: None };
        assert_eq!(machine.reservable(), Reserve { cores: 3.0, ram_mb: 8083 - 3233, vram_mb: 0 });
        let mut p = Policy::open(None);
        p.set_project("web", None, None, Some(Some(Reserve { cores: 2.0, ram_mb: 2048, vram_mb: 0 }))).unwrap();
        assert!(p.check_reservations(&machine).is_ok());
        p.set_session("s", None, None, None, Some(Some(Reserve { cores: 1.0, ram_mb: 3000, vram_mb: 0 })), 0).unwrap();
        let err = p.check_reservations(&machine).unwrap_err();
        assert!(err.contains("5048 MiB of at most 4850 MiB"), "{err}");
        p.set_session("s", None, None, None, Some(Some(Reserve { cores: 2.0, ram_mb: 1000, vram_mb: 0 })), 0).unwrap();
        assert!(p.check_reservations(&machine).unwrap_err().contains("4 cores of at most 3"));
        p.set_session("s", None, None, None, Some(Some(Reserve { vram_mb: 512, ..Reserve::default() })), 0).unwrap();
        assert!(p.check_reservations(&machine).unwrap_err().contains("does not report"));
        // A session's reservation is what holds for it; without one, its project's.
        p.set_session("s", None, None, None, Some(None), 0).unwrap();
        assert_eq!(p.effective(&who("s", "web"), 0, None).reserve.unwrap().ram_mb, 2048);
    }

    #[test]
    fn the_file_keeps_the_policy_and_an_unreadable_one_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("kelpie-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("resource-policy.json");
        let mut p = Policy::open(Some(path.clone()));
        p.set_project("web", Some(Tier::Critical), Some(0), None).unwrap();
        p.save();
        let again = Policy::open(Some(path.clone()));
        assert_eq!(again.file.projects["web"].tier, Tier::Critical);
        assert_eq!(again.file.switches, TierSwitches::default());
        // Older or newer files: missing keys take their defaults, unknown ones are ignored. A turn boost does not
        // outlive the daemon that saw the turn start.
        std::fs::write(&path, br#"{"projects":{"a":{"tier":"background"}},"sessions":{"s":{"boost":{"turn":99999999999999}}},"future":1}"#).unwrap();
        let loaded = Policy::open(Some(path.clone()));
        assert_eq!(loaded.file.projects["a"].tier, Tier::Background);
        assert!(loaded.file.sessions.is_empty());
        // A file that cannot be read at all (here a folder in its place) is never written over.
        let blocked = dir.join("blocked.json");
        std::fs::create_dir_all(&blocked).unwrap();
        let p = Policy::open(Some(blocked.clone()));
        assert!(p.to_save().is_none());
        assert!(blocked.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
