//! What became of the sub-sessions that finished: Kelpie's own record, shown as routing hints (`kelpie_agents`).
//! Local and verifiable only: how many each agent took, how many of those ended with a verdict and how many of
//! those were good, how long they took. `sub-stats.json`.
//!
//! A verdict is something the work itself said: an explicit report of done or failed, or the end of a run task.
//! A session that was only closed, or whose agent exited, has none: it counts as finished but not in the share,
//! so agents that never report (those without the MCP tools) are not scored as failures.

use std::path::PathBuf;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// The last this many finished sub-sessions are kept.
pub const KEEP: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    /// The session's id: one sub-session counts once.
    pub id: String,
    pub agent: String,
    /// done | failed | exited | closed
    pub status: String,
    /// The work gave a verdict (reported done/failed, or a run task ended).
    #[serde(default)]
    pub judged: bool,
    /// Time the daemon watched it work; unknown when it did not see the start (the downtime of a restart is never counted).
    #[serde(default)]
    pub minutes: Option<f64>,
}

/// One agent's record.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentStats {
    pub agent: String,
    pub count: usize,
    /// How many of them had a verdict.
    pub judged: usize,
    /// Share of the judged ones that ended `done`, 0 to 1; none before a verdict.
    pub done_share: Option<f64>,
    pub median_minutes: Option<f64>,
}

/// Per agent, in the order each first appears.
pub fn aggregate(outcomes: &[Outcome]) -> Vec<AgentStats> {
    let mut agents: Vec<&str> = Vec::new();
    for o in outcomes {
        if !agents.contains(&o.agent.as_str()) {
            agents.push(&o.agent);
        }
    }
    agents
        .into_iter()
        .map(|agent| {
            let mine: Vec<&Outcome> = outcomes.iter().filter(|o| o.agent == agent).collect();
            let judged = mine.iter().filter(|o| o.judged).count();
            let done = mine.iter().filter(|o| o.judged && o.status == "done").count();
            let mut minutes: Vec<f64> = mine.iter().filter_map(|o| o.minutes).collect();
            minutes.sort_by(f64::total_cmp);
            let median = match minutes.len() {
                0 => None,
                n if n % 2 == 1 => Some(minutes[n / 2]),
                n => Some((minutes[n / 2 - 1] + minutes[n / 2]) / 2.0),
            };
            AgentStats {
                agent: agent.to_string(),
                count: mine.len(),
                judged,
                done_share: (judged > 0).then(|| round1(done as f64 / judged as f64)),
                median_minutes: median.map(round1),
            }
        })
        .collect()
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[derive(Default)]
pub struct SubStats {
    path: Option<PathBuf>,
    outcomes: Mutex<Vec<Outcome>>,
}

impl SubStats {
    /// Reads `path`; a missing or unreadable file is an empty record (it is only a hint).
    pub fn load(path: PathBuf) -> SubStats {
        let outcomes = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice::<Vec<Outcome>>(&b).ok()).unwrap_or_default();
        SubStats { path: Some(path), outcomes: Mutex::new(outcomes) }
    }

    /// Notes how a sub-session ended, once per session.
    pub fn record(&self, id: &str, agent: &str, status: &str, judged: bool, minutes: Option<f64>) {
        let snapshot = {
            let mut all = self.outcomes.lock();
            if all.iter().any(|o| o.id == id) {
                return;
            }
            all.push(Outcome { id: id.into(), agent: agent.into(), status: status.into(), judged, minutes: minutes.map(round1) });
            if all.len() > KEEP {
                let extra = all.len() - KEEP;
                all.drain(..extra);
            }
            serde_json::to_vec(&*all)
        };
        // Written outside the lock.
        if let Some(path) = &self.path {
            if let Err(e) = snapshot.map_err(anyhow::Error::from).and_then(|b| crate::persist::write_atomic(path, &b)) {
                tracing::warn!("sub-stats: {e:#}");
            }
        }
    }

    pub fn by_agent(&self) -> Vec<AgentStats> {
        aggregate(&self.outcomes.lock())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn o(id: &str, agent: &str, status: &str, judged: bool, minutes: Option<f64>) -> Outcome {
        Outcome { id: id.into(), agent: agent.into(), status: status.into(), judged, minutes }
    }

    #[test]
    fn the_share_counts_only_sessions_with_a_verdict_and_the_median_only_known_times() {
        let all = [
            o("1", "claude", "done", true, Some(10.0)),
            o("2", "codex", "failed", true, Some(3.0)),
            o("3", "claude", "closed", false, Some(20.0)),
            o("4", "claude", "done", true, None),
            o("5", "codex", "done", true, Some(5.0)),
            o("6", "gemini", "closed", false, None),
            o("7", "gemini", "exited", false, None),
        ];
        let stats = aggregate(&all);
        assert_eq!(stats[0], AgentStats { agent: "claude".into(), count: 3, judged: 2, done_share: Some(1.0), median_minutes: Some(15.0) });
        assert_eq!(stats[1], AgentStats { agent: "codex".into(), count: 2, judged: 2, done_share: Some(0.5), median_minutes: Some(4.0) });
        // Never judged: no share, so it is not scored as a failure; no time known: no median.
        assert_eq!(stats[2], AgentStats { agent: "gemini".into(), count: 2, judged: 0, done_share: None, median_minutes: None });
        assert!(aggregate(&[]).is_empty());
    }

    #[test]
    fn a_session_counts_once_and_the_record_is_capped() {
        let stats = SubStats::default();
        stats.record("a", "claude", "done", true, Some(2.0));
        stats.record("a", "claude", "closed", false, Some(99.0));
        assert_eq!(stats.by_agent(), vec![AgentStats { agent: "claude".into(), count: 1, judged: 1, done_share: Some(1.0), median_minutes: Some(2.0) }]);
        for i in 0..KEEP + 20 {
            stats.record(&format!("s{i}"), "codex", "done", true, Some(1.0));
        }
        assert_eq!(stats.outcomes.lock().len(), KEEP);
        // The oldest went.
        assert!(stats.by_agent().iter().all(|a| a.agent == "codex"));
    }

    #[test]
    fn the_record_survives_a_restart_and_reads_the_older_shape() {
        let dir = std::env::temp_dir().join(format!("kelpie-sub-stats-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sub-stats.json");
        SubStats::load(path.clone()).record("a", "gemini", "failed", true, None);
        assert_eq!(SubStats::load(path.clone()).by_agent()[0].agent, "gemini");
        std::fs::write(&path, br#"[{"id":"x","agent":"codex","status":"done","minutes":4.0}]"#).unwrap();
        let old = SubStats::load(path.clone()).by_agent();
        assert_eq!((old[0].count, old[0].judged, old[0].median_minutes), (1, 0, Some(4.0)));
        std::fs::write(&path, b"not json").unwrap();
        assert!(SubStats::load(path).by_agent().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
