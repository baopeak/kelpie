//! Fuzzy ranking of the index's paths: nucleo (the matcher behind Helix), with a filename bonus so
//! `main` finds `src/main.rs` before `domain/x.rs`. No app or file system access, so it can be
//! benchmarked alone.

use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use serde::Serialize;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Serialize)]
pub struct QuickOpenHit {
    pub path: String,
    pub score: u32,
    /// Matched character indices in `path` (for highlighting).
    pub indices: Vec<u32>,
    /// A folder (its `path` ends in `/`); only returned when the caller asked for folders.
    pub is_dir: bool,
}

/// Indexes at least this big are scored by several workers.
const PARALLEL_FROM: usize = 8_000;
/// Half the cores, at most this many: a search must not take the machine from the programs it is
/// searching for (on a busy one, more workers only made the slowest of them later).
const MAX_THREADS: usize = 4;
/// How many of the best path matches get the filename bonus (the bonus is a second match per hit:
/// ranking every hit twice would double the time of a query that matches most of a big project).
const BONUS_CANDIDATES: usize = 400;

/// What a hit is ranked by: higher score, then the shorter path (so `main.rs` comes before
/// `domain/x/main.rs`), then the earlier one, so the order does not depend on how the work was split.
fn rank_key(files: &[String], score: u32, i: usize) -> (std::cmp::Reverse<u32>, usize, usize) {
    (std::cmp::Reverse(score.saturating_sub((files[i].len() / 16) as u32)), files[i].len(), i)
}

/// Entries a worker claims at a time.
const CHUNK: usize = 2_048;

/// The best `keep` matches among the entries this worker claims from `next` (the others take the
/// rest), by how well the whole path matches, as (score, position). Workers claim chunks as they
/// finish, so one that is slow to get a core does not hold the others up.
fn best_paths(files: &[String], next: &AtomicUsize, pattern: &Pattern, dirs: bool, keep: usize) -> Vec<(u32, usize)> {
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut hits = Vec::new();
    loop {
        let start = next.fetch_add(CHUNK, Ordering::Relaxed);
        if start >= files.len() {
            break;
        }
        for (i, path) in files.iter().enumerate().take((start + CHUNK).min(files.len())).skip(start) {
            if !dirs && path.ends_with('/') {
                continue;
            }
            if let Some(score) = pattern.score(Utf32Str::new(path, &mut buf), &mut matcher) {
                hits.push((score, i));
            }
        }
        if hits.len() > keep * 4 {
            hits.sort_unstable_by_key(|h| rank_key(files, h.0, h.1));
            hits.truncate(keep);
        }
    }
    hits.sort_unstable_by_key(|h| rank_key(files, h.0, h.1));
    hits.truncate(keep);
    hits
}

/// Best matches of `query` among `files` (folders only when `dirs`), best first.
pub fn rank(files: &[String], query: &str, limit: usize, dirs: bool) -> Vec<QuickOpenHit> {
    let threads = if files.len() >= PARALLEL_FROM { (std::thread::available_parallelism().map_or(1, |n| n.get()) / 2).clamp(1, MAX_THREADS) } else { 1 };
    rank_on(files, query, limit, dirs, threads)
}

/// `rank` scored by `threads` workers (the same result whatever their number).
pub fn rank_on(files: &[String], query: &str, limit: usize, dirs: bool, threads: usize) -> Vec<QuickOpenHit> {
    let hit = |score: u32, path: &String, indices: Vec<u32>| QuickOpenHit { path: path.clone(), score, indices, is_dir: path.ends_with('/') };
    let query = query.trim();
    if query.is_empty() {
        return files.iter().filter(|p| dirs || !p.ends_with('/')).take(limit).map(|p| hit(0, p, vec![])).collect();
    }
    let pattern = Pattern::new(query, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy);
    let keep = BONUS_CANDIDATES.max(limit * 4);
    let threads = threads.max(1);
    let next = AtomicUsize::new(0);
    let mut candidates: Vec<(u32, usize)> = std::thread::scope(|scope| {
        // The caller works too: the helpers only add to it.
        let helpers: Vec<_> = (1..threads).map(|_| scope.spawn(|| best_paths(files, &next, &pattern, dirs, keep))).collect();
        let mut all = best_paths(files, &next, &pattern, dirs, keep);
        for h in helpers {
            all.extend(h.join().unwrap_or_default());
        }
        all
    });
    candidates.sort_unstable_by_key(|h| rank_key(files, h.0, h.1));
    candidates.truncate(keep);

    // The bonus: a match of the file name alone, so `main` finds `src/main.rs` before `domain/x.rs`.
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let (mut buf, mut name_buf) = (Vec::new(), Vec::new());
    for (score, i) in candidates.iter_mut() {
        let path = &files[*i];
        let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or(path);
        *score += pattern.score(Utf32Str::new(name, &mut name_buf), &mut matcher).map_or(0, |s| s / 2);
    }
    candidates.sort_unstable_by_key(|h| rank_key(files, h.0, h.1));
    candidates.truncate(limit);
    candidates
        .into_iter()
        .map(|(score, i)| {
            let mut indices = Vec::new();
            pattern.indices(Utf32Str::new(&files[i], &mut buf), &mut matcher, &mut indices);
            indices.sort_unstable();
            indices.dedup();
            hit(score.saturating_sub((files[i].len() / 16) as u32), &files[i], indices)
        })
        .collect()
}
