//! Tokens and cost across every agent transcript on this machine, for the Usage page: by day,
//! week, project, agent and model. The numbers are the per-file aggregates of `session_usage`
//! (counted once, then only the new lines of a grown file); a warm report only stats the files.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::agent_history::{canonical_folder, transcript_files, AccountHomes, Agent};
use crate::fs_ops::{err, CmdResult};
use crate::session_usage::{claim, count_file, normalize_model, release, save_cache, stat, subagent_files, Totals, FILES};

/// One cell of the report: a local day of one agent, project and model.
#[derive(Serialize, Debug, PartialEq)]
pub struct UsageRow {
    /// Days since 1970-01-01 in the caller's time zone.
    day: i64,
    agent: &'static str,
    /// Folder the conversation ran in (empty: the transcript does not say).
    project: String,
    model: String,
    tokens_in: u64,
    tokens_out: u64,
    /// Sum of the records that have a price.
    cost: f64,
    /// Tokens of records without one (unknown model, Codex).
    unpriced: u64,
}

#[derive(Serialize)]
pub struct UsageReport {
    rows: Vec<UsageRow>,
    files: usize,
    /// Transcripts still being counted: the rows cover what is known so far.
    pending: usize,
}

type Stale = Vec<(Agent, PathBuf, u64, u64)>;

fn agent_name(a: Agent) -> &'static str {
    match a {
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Omp => "omp",
    }
}

/// Rows from the aggregates that exist, and the files whose aggregate is missing or out of date.
fn build(homes: &AccountHomes, tz_offset_min: i64) -> (UsageReport, Stale, HashSet<PathBuf>) {
    // A Claude conversation's sub-agents (`<id>/subagents/*.jsonl`) hold much of its spend.
    let mut listed: Vec<(Agent, PathBuf)> = transcript_files(homes).into_iter().map(|(a, p, _)| (a, p)).collect();
    let subs: Vec<PathBuf> = listed.iter().filter(|(a, _)| *a == Agent::Claude).flat_map(|(_, p)| subagent_files(p)).collect();
    listed.extend(subs.into_iter().map(|p| (Agent::Claude, p)));
    let files: Vec<(Agent, PathBuf, u64, u64)> = listed.into_iter().filter_map(|(a, p)| stat(&p).map(|(m, l)| (a, p, m, l))).collect();
    let cache = FILES.lock();
    let stale: Stale = files.iter().filter(|(_, p, m, l)| !cache.get(p).is_some_and(|u| u.mtime_ms == *m && u.len == *l)).cloned().collect();
    let mut cells: HashMap<(i64, Agent, String, String), Totals> = HashMap::new();
    let mut folders: HashMap<String, String> = HashMap::new();
    for (agent, path, _, _) in &files {
        let Some(u) = cache.get(path) else { continue };
        let project = match u.cwd.as_deref() {
            Some(cwd) => folders.entry(cwd.to_string()).or_insert_with(|| canonical_folder(cwd)).clone(),
            None => String::new(),
        };
        for (key, t) in &u.buckets {
            let Some((hour, model)) = key.split_once('|') else { continue };
            // Hour 0 holds records without a date.
            let Some(hour) = hour.parse::<i64>().ok().filter(|h| *h > 0) else { continue };
            let day = local_day(hour, tz_offset_min);
            let model = if *agent == Agent::Claude && !model.is_empty() { normalize_model(model) } else { model.to_string() };
            cells.entry((day, *agent, project.clone(), model)).or_default().add(t);
        }
    }
    let live = files.iter().map(|f| f.1.clone()).collect();
    let mut rows: Vec<UsageRow> = cells
        .into_iter()
        .map(|((day, agent, project, model), t)| UsageRow { day, agent: agent_name(agent), project, model, tokens_in: t.tokens_in, tokens_out: t.tokens_out, cost: t.cost, unpriced: t.unpriced })
        .collect();
    rows.sort_by(|a, b| (b.day, a.agent, &a.project, &a.model).cmp(&(a.day, b.agent, &b.project, &b.model)));
    (UsageReport { rows, files: files.len(), pending: stale.len() }, stale, live)
}

/// The day (days since the epoch) of epoch hour `hour` in a zone `tz_offset_min` minutes east of UTC.
fn local_day(hour: i64, tz_offset_min: i64) -> i64 {
    (hour * 60 + tz_offset_min).div_euclid(1440)
}

/// Scans run one at a time; the page asks again when `usage-scanned` arrives.
static SCANNING: AtomicBool = AtomicBool::new(false);

fn spawn_scan(app: AppHandle, mut stale: Stale, live: HashSet<PathBuf>) {
    if stale.is_empty() || SCANNING.swap(true, Ordering::AcqRel) {
        return;
    }
    // Small files first: most rows fill in quickly.
    stale.sort_by_key(|f| f.3);
    std::thread::spawn(move || {
        /// Clears the flag however the pass ends (a panicking worker included).
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                SCANNING.store(false, Ordering::Release);
            }
        }
        let _done = Done;
        let next = AtomicUsize::new(0);
        // Background work: two workers, so the terminals keep their CPU.
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    while let Some((agent, path, mtime, len)) = stale.get(next.fetch_add(1, Ordering::Relaxed)) {
                        if claim(path) {
                            count_file(*agent, path, *mtime, *len);
                            release(path);
                        }
                    }
                });
            }
        });
        FILES.lock().retain(|p, _| live.contains(p));
        save_cache();
        let _ = app.emit("usage-scanned", ());
    });
}

/// Rows for the Usage page. `tz_offset_min`: minutes east of UTC (`-new Date().getTimezoneOffset()`).
/// While transcripts are being counted `pending` says how many, and `usage-scanned` is emitted
/// when the pass ends.
#[tauri::command]
pub async fn usage_report(app: AppHandle, tz_offset_min: i64, homes: Option<AccountHomes>) -> CmdResult<UsageReport> {
    let homes = homes.unwrap_or_default();
    let (report, stale, live) = tauri::async_runtime::spawn_blocking(move || build(&homes, tz_offset_min)).await.map_err(err)?;
    spawn_scan(app, stale, live);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p kelpie --lib --release -- --ignored --nocapture report_real` — cold and warm
    /// report over this machine's real transcripts (the warm one must stay under 100 ms).
    #[test]
    #[ignore]
    fn report_real_transcripts() {
        let homes = AccountHomes::default();
        let t = std::time::Instant::now();
        let (cold, stale, _) = build(&homes, 420);
        println!("cold: {} files, {} to count, {} rows, {}ms", cold.files, stale.len(), cold.rows.len(), t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let mut total = 0usize;
        for (agent, path, m, l) in &stale {
            if claim(path) {
                count_file(*agent, path, *m, *l);
                release(path);
                total += 1;
            }
        }
        println!("counted {total} files in {}ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let listed = transcript_files(&homes);
        println!("list: {} files {}ms", listed.len(), t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let statted = listed.iter().filter(|(_, p, _)| stat(p).is_some()).count();
        println!("stat: {statted} files {}ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let (warm, stale, _) = build(&homes, 420);
        let ms = t.elapsed().as_millis();
        let cost: f64 = warm.rows.iter().map(|r| r.cost).sum();
        let tokens: u64 = warm.rows.iter().map(|r| r.tokens_in + r.tokens_out).sum();
        println!("warm: {} rows, {} stale, {tokens} tokens, ${cost:.2}, {ms}ms", warm.rows.len(), stale.len());
        for agent in ["claude", "codex", "omp"] {
            let (i, o) = warm.rows.iter().filter(|r| r.agent == agent).fold((0, 0), |a, r| (a.0 + r.tokens_in, a.1 + r.tokens_out));
            println!("{agent}: in {i} out {o}");
        }
        assert!(ms <= 100, "warm report took {ms}ms");
    }

    #[test]
    fn days_follow_the_local_time_zone() {
        // 2026-10-24 18:00 UTC is already the 25th in UTC+7 (Asia/Saigon), still the 24th in UTC-5.
        assert_eq!(local_day(498_018, 0), 20_750);
        assert_eq!(local_day(498_018, 420), 20_751);
        assert_eq!(local_day(498_018, -300), 20_750);
        assert_eq!(local_day(498_017, 420), 20_751, "17:00 UTC is 00:00 in UTC+7: the 25th starts there");
        assert_eq!(local_day(498_016, 420), 20_750);
    }
}
