//! Context fill, tokens and cost of one agent conversation, read from the agent's own
//! transcript. Three formats (Claude Code, Codex rollouts, OMP sessions), all defensive: a line
//! that does not look like a usage record is skipped, one that looks like it but does not parse
//! is counted (and logged) so format drift shows up instead of silently hiding numbers.
//!
//! A session maps to its transcript by identity only: the conversation id the daemon recorded
//! (Claude: the hook's `transcript_path` or `<projects>/<slug>/<id>.jsonl`; Codex: the rollout
//! whose name ends in the thread id; OMP: the file whose name ends in the session id). Without an
//! id there is no transcript, never the newest file of the folder.
//!
//! Two reads per update, both bounded: the context fill comes from the last 64 KB of the file
//! (reverse scan for the last usage record); the running totals come from a per-file aggregate
//! that continues from the last complete line it counted. A transcript with no aggregate yet (or a
//! large unread gap) is caught up on a background thread and the totals follow a moment later.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::agent_history::{claude_dir, claude_project_dir, codex_dir, dirs_in, home_or, jsonl_in, line_ends_at, open_lines, valid_id, Agent};
use crate::fs_ops::{blocking, CmdResult};

/// Most a single update reads from the end of a transcript.
pub const TAIL_BYTES: u64 = 64 * 1024;

// ---------- prices ----------

#[derive(Deserialize)]
struct Rates {
    input: f64,
    output: f64,
    cache_read: f64,
}

#[derive(Deserialize)]
struct PriceRow {
    models: Vec<String>,
    #[serde(flatten)]
    rates: Rates,
    /// Context window in tokens.
    context: u64,
    /// Prompts longer than this are billed at `long` rates.
    #[serde(default)]
    long_prompt_over: Option<u64>,
    #[serde(default)]
    long: Option<Rates>,
}

#[derive(Deserialize)]
struct PriceFile {
    claude: Vec<PriceRow>,
}

static PRICES: LazyLock<(Vec<PriceRow>, HashMap<String, usize>)> = LazyLock::new(|| {
    let file: PriceFile = serde_json::from_str(include_str!("pricing.json")).expect("pricing.json is valid");
    let index = file.claude.iter().enumerate().flat_map(|(i, r)| r.models.iter().map(move |m| (m.clone(), i))).collect();
    (file.claude, index)
});

/// `claude-opus-4-8[1m]` / `claude-haiku-4-5-20251001` / `anthropic.claude-sonnet-4.6` → `opus-4-8`…
pub fn normalize_model(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    let base = lower.split('[').next().unwrap_or("");
    let base = base.find("claude-").map_or(base, |i| &base[i + 7..]);
    let base = base.strip_suffix("-latest").unwrap_or(base);
    let base = match base.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => base,
    };
    base.replace('.', "-")
}

fn price_of(model: &str) -> Option<&'static PriceRow> {
    let (rows, index) = &*PRICES;
    index.get(&normalize_model(model)).map(|&i| &rows[i])
}

/// The rates that apply to a prompt of `prompt` tokens.
fn rates_for(row: &PriceRow, prompt: u64) -> &Rates {
    match (&row.long, row.long_prompt_over) {
        (Some(long), Some(over)) if prompt > over => long,
        _ => &row.rates,
    }
}

/// API-price estimate of one Claude message, in USD.
fn claude_cost(row: &PriceRow, input: u64, cache_read: u64, write_5m: u64, write_1h: u64, output: u64) -> f64 {
    let r = rates_for(row, input + cache_read + write_5m + write_1h);
    (input as f64 * r.input + cache_read as f64 * r.cache_read + write_5m as f64 * r.input * 1.25 + write_1h as f64 * r.input * 2.0 + output as f64 * r.output) / 1e6
}

// ---------- one usage record ----------

/// What one transcript line says about usage.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sample {
    /// Message id: Claude repeats the same message (and usage) on one line per content block.
    pub id: Option<String>,
    pub ts_ms: Option<u64>,
    pub model: Option<String>,
    /// All prompt tokens (cache reads and writes included) and generated tokens.
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Codex reports running totals; the aggregate takes differences.
    pub cumulative: bool,
    /// Price of this record: an estimate for Claude, the agent's own number for OMP.
    pub cost: Option<f64>,
    /// USD per generated token at this record's prompt tier (Claude): a message repeated with more
    /// output costs that much more.
    pub out_rate: Option<f64>,
    /// Context in use after this record, and its window when known.
    pub context: Option<(u64, Option<u64>)>,
}

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "isSidechain")]
    sidechain: Option<bool>,
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    model: Option<String>,
    usage: Option<ClaudeUsage>,
}

#[derive(Deserialize)]
struct ClaudeUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
    cache_creation: Option<ClaudeCacheCreation>,
}

#[derive(Deserialize)]
struct ClaudeCacheCreation {
    ephemeral_5m_input_tokens: Option<u64>,
    ephemeral_1h_input_tokens: Option<u64>,
}

fn parse_claude(line: &str) -> Result<Option<Sample>, serde_json::Error> {
    let l: ClaudeLine = serde_json::from_str(line)?;
    if l.kind.as_deref() != Some("assistant") {
        return Ok(None);
    }
    let Some(msg) = l.message else { return Ok(None) };
    let Some(u) = msg.usage else { return Ok(None) };
    // `<synthetic>`: Claude Code's own messages (errors, interruptions), not API calls.
    if msg.model.as_deref().is_some_and(|m| m.starts_with('<')) {
        return Ok(None);
    }
    let (input, output, read) = (u.input_tokens.unwrap_or(0), u.output_tokens.unwrap_or(0), u.cache_read_input_tokens.unwrap_or(0));
    let total_write = u.cache_creation_input_tokens.unwrap_or(0);
    let (w5, w1) = match u.cache_creation {
        Some(c) if c.ephemeral_5m_input_tokens.is_some() || c.ephemeral_1h_input_tokens.is_some() => {
            (c.ephemeral_5m_input_tokens.unwrap_or(0), c.ephemeral_1h_input_tokens.unwrap_or(0))
        }
        _ => (total_write, 0),
    };
    let (tokens_in, tokens_out) = (input + read + w5 + w1, output);
    let price = msg.model.as_deref().and_then(price_of);
    // A sidechain line (older layouts wrote sub-agents into the main file) is billed but is not the
    // main conversation's context.
    let context = (!l.sidechain.unwrap_or(false) && tokens_in + tokens_out > 0).then(|| (tokens_in + tokens_out, price.map(|r| r.context)));
    Ok(Some(Sample {
        id: msg.id.or(l.request_id),
        ts_ms: l.timestamp.as_deref().and_then(crate::usage::parse_iso_ms),
        model: msg.model,
        tokens_in,
        tokens_out,
        cumulative: false,
        cost: price.map(|r| claude_cost(r, input, read, w5, w1, output)),
        out_rate: price.map(|r| rates_for(r, input + read + w5 + w1).output / 1e6),
        context,
    }))
}

#[derive(Deserialize)]
struct OmpLine {
    id: Option<String>,
    timestamp: Option<String>,
    message: Option<OmpMessage>,
}

#[derive(Deserialize)]
struct OmpMessage {
    role: Option<String>,
    model: Option<String>,
    usage: Option<OmpUsage>,
    #[serde(rename = "contextSnapshot")]
    context_snapshot: Option<OmpSnapshot>,
}

#[derive(Deserialize)]
struct OmpUsage {
    input: Option<u64>,
    output: Option<u64>,
    #[serde(rename = "cacheRead")]
    cache_read: Option<u64>,
    #[serde(rename = "cacheWrite")]
    cache_write: Option<u64>,
    cost: Option<OmpCost>,
}

#[derive(Deserialize)]
struct OmpCost {
    total: Option<f64>,
}

#[derive(Deserialize)]
struct OmpSnapshot {
    #[serde(rename = "promptTokens")]
    prompt_tokens: Option<u64>,
}

fn parse_omp(line: &str) -> Result<Option<Sample>, serde_json::Error> {
    let l: OmpLine = serde_json::from_str(line)?;
    let Some(msg) = l.message else { return Ok(None) };
    let (true, Some(u)) = (msg.role.as_deref() == Some("assistant"), msg.usage) else { return Ok(None) };
    let tokens_in = u.input.unwrap_or(0) + u.cache_read.unwrap_or(0) + u.cache_write.unwrap_or(0);
    // OMP records no window: the ring stays hidden, the numbers are still shown.
    let context = msg.context_snapshot.and_then(|c| c.prompt_tokens).map(|p| (p, None));
    Ok(Some(Sample {
        id: l.id,
        ts_ms: l.timestamp.as_deref().and_then(crate::usage::parse_iso_ms),
        model: msg.model,
        tokens_in,
        tokens_out: u.output.unwrap_or(0),
        cumulative: false,
        cost: u.cost.and_then(|c| c.total),
        out_rate: None,
        context,
    }))
}

#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<String>,
    payload: Option<CodexPayload>,
}

#[derive(Deserialize)]
struct CodexPayload {
    #[serde(rename = "type")]
    kind: Option<String>,
    model: Option<String>,
    info: Option<CodexInfo>,
}

#[derive(Deserialize)]
struct CodexInfo {
    total_token_usage: Option<CodexTokens>,
    last_token_usage: Option<CodexTokens>,
    model_context_window: Option<u64>,
}

#[derive(Deserialize)]
struct CodexTokens {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    total_tokens: Option<u64>,
}

fn parse_codex(line: &str) -> Result<Option<Sample>, serde_json::Error> {
    let l: CodexLine = serde_json::from_str(line)?;
    let Some(p) = l.payload else { return Ok(None) };
    if l.kind.as_deref() != Some("event_msg") || p.kind.as_deref() != Some("token_count") {
        return Ok(None);
    }
    // Rate-limit-only records carry `info: null`.
    let Some(info) = p.info else { return Ok(None) };
    let Some(total) = info.total_token_usage else { return Ok(None) };
    let context = info.last_token_usage.map(|t| (t.total_tokens.unwrap_or_else(|| t.input_tokens.unwrap_or(0) + t.output_tokens.unwrap_or(0)), info.model_context_window));
    Ok(Some(Sample {
        id: None,
        ts_ms: l.timestamp.as_deref().and_then(crate::usage::parse_iso_ms),
        model: None,
        tokens_in: total.input_tokens.unwrap_or(0),
        tokens_out: total.output_tokens.unwrap_or(0),
        cumulative: true,
        cost: None,
        out_rate: None,
        context,
    }))
}

/// Codex names the model on `turn_context` records, not on the token counts.
fn codex_turn_model(line: &str) -> Option<String> {
    if !line.contains("\"turn_context\"") {
        return None;
    }
    let l: CodexLine = serde_json::from_str(line).ok()?;
    (l.kind.as_deref() == Some("turn_context")).then_some(())?;
    l.payload?.model
}

/// `Ok(None)`: not a usage record. `Err`: it looks like one but does not parse.
pub fn sample(agent: Agent, line: &str) -> Result<Option<Sample>, serde_json::Error> {
    match agent {
        Agent::Claude if line.contains("\"usage\"") && line.contains("\"assistant\"") => parse_claude(line),
        Agent::Omp if line.contains("\"usage\"") && line.contains("\"assistant\"") => parse_omp(line),
        Agent::Codex if line.contains("\"token_count\"") => parse_codex(line),
        _ => Ok(None),
    }
}

static PARSE_FAILURES: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

/// Counts a usage-looking line that did not parse; logs the first and every hundredth.
fn note_parse_failure(agent: Agent) {
    let i = match agent {
        Agent::Claude => 0,
        Agent::Codex => 1,
        Agent::Omp => 2,
    };
    let n = PARSE_FAILURES[i].fetch_add(1, Ordering::Relaxed) + 1;
    if n == 1 || n.is_multiple_of(100) {
        tracing::warn!(?agent, count = n, "transcript usage lines that did not parse");
    }
}

// ---------- the end of a transcript ----------

/// Context fill after the last usage record.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Last {
    pub context_used: u64,
    pub context_window: Option<u64>,
    pub model: Option<String>,
}

/// The last `max` bytes of `path`, starting on a line boundary (the cut line is dropped).
fn read_tail(path: &Path, max: u64) -> std::io::Result<Vec<u8>> {
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(max);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(max).read_to_end(&mut buf)?;
    if start > 0 {
        match buf.iter().position(|&b| b == b'\n') {
            Some(i) => {
                buf.drain(..=i);
            }
            None => buf.clear(),
        }
    }
    Ok(buf)
}

/// The newest usage record in `bytes` (reverse scan). A last line cut by a writer still at work
/// does not parse and is skipped.
pub fn last_in(agent: Agent, bytes: &[u8]) -> Option<Last> {
    /// Claude writes this line when `/compact` has run: the fill before it is gone.
    const COMPACTED: &str = "\"subtype\":\"compact_boundary\"";
    let mut found: Option<(u64, Option<u64>, Option<String>)> = None;
    let mut model = None;
    for raw in bytes.split(|&b| b == b'\n').rev() {
        if raw.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(raw);
        if found.is_none() {
            if agent == Agent::Claude && line.contains(COMPACTED) {
                return None;
            }
            if let Ok(Some(s)) = sample(agent, &line) {
                if let Some((used, window)) = s.context {
                    found = Some((used, window, s.model));
                    if agent != Agent::Codex || model.is_some() {
                        break;
                    }
                    continue;
                }
            }
        }
        // Codex: the model of the turn comes from the `turn_context` before its token counts.
        if agent == Agent::Codex && model.is_none() {
            model = codex_turn_model(&line);
            if found.is_some() && model.is_some() {
                break;
            }
        }
    }
    found.map(|(context_used, context_window, m)| Last { context_used, context_window, model: m.or(model) })
}

/// Context fill from the end of the transcript, reading at most [`TAIL_BYTES`].
pub fn tail_last(agent: Agent, path: &Path) -> Option<Last> {
    last_in(agent, &read_tail(path, TAIL_BYTES).ok()?)
}

// ---------- per-file aggregate ----------

#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq)]
pub struct Totals {
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Sum of the records that have a price.
    pub cost: f64,
    /// Tokens of records without one (unknown model, Codex): their cost is not guessed.
    pub unpriced: u64,
}

impl Totals {
    pub fn add(&mut self, o: &Totals) {
        self.tokens_in += o.tokens_in;
        self.tokens_out += o.tokens_out;
        self.cost += o.cost;
        self.unpriced += o.unpriced;
    }
}

/// Everything counted so far in one transcript, resumable from `offset`.
#[derive(Serialize, Deserialize, Clone, Default, Debug)]
pub struct FileUsage {
    pub mtime_ms: u64,
    pub len: u64,
    /// End of the last complete line counted.
    pub offset: u64,
    /// Message id hash → output tokens counted so far (a message appears on several lines, far
    /// apart; sub-agent files repeat it with a growing output count, so the highest wins).
    seen: HashMap<u64, u64>,
    prev_total: Option<(u64, u64)>,
    /// Codex: the model of the latest `turn_context`.
    model: Option<String>,
    last_ts: Option<u64>,
    pub cwd: Option<String>,
    /// `"<epoch hour>|<model>"` → totals.
    pub buckets: HashMap<String, Totals>,
    /// Model of the newest record.
    pub last_model: Option<String>,
    /// Context after the newest record of this file (none after a compaction).
    pub last: Option<Last>,
}

/// FNV-1a: stable across builds, unlike `DefaultHasher` (the hashes are persisted).
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Folder a transcript was started in, from the first line that names one.
fn find_cwd(line: &str) -> Option<String> {
    static CWD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""cwd":"((?:[^"\\]|\\.)*)""#).unwrap());
    let raw = CWD.captures(line)?.get(1)?.as_str();
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok().filter(|c| !c.is_empty())
}

impl FileUsage {
    fn feed(&mut self, agent: Agent, line: &str) {
        if self.cwd.is_none() && line.contains("\"cwd\":\"") {
            self.cwd = find_cwd(line);
        }
        if agent == Agent::Codex {
            if let Some(m) = codex_turn_model(line) {
                self.model = Some(m);
            }
        }
        if agent == Agent::Claude && line.contains("\"subtype\":\"compact_boundary\"") {
            self.last = None;
            return;
        }
        let mut s = match sample(agent, line) {
            Ok(Some(s)) => s,
            Ok(None) => return,
            Err(_) => return note_parse_failure(agent),
        };
        if let Some((used, window)) = s.context {
            self.last = Some(Last { context_used: used, context_window: window, model: s.model.clone() });
        }
        if let Some(id) = s.id.as_deref() {
            match self.seen.insert(fnv(id), s.tokens_out) {
                None => {}
                // The same message again: only output beyond what was counted is new, at this
                // message's own output rate (what a record without one cannot say stays as counted).
                Some(counted) => match s.out_rate {
                    Some(rate) if s.tokens_out > counted => {
                        let extra = s.tokens_out - counted;
                        (s.tokens_in, s.tokens_out, s.cost) = (0, extra, Some(extra as f64 * rate));
                    }
                    _ => {
                        self.seen.insert(fnv(id), counted.max(s.tokens_out));
                        return;
                    }
                },
            }
        }
        let (mut tin, mut tout) = (s.tokens_in, s.tokens_out);
        if s.cumulative {
            // Running totals: only the growth since the previous record is new (a drop means the
            // counter restarted, and everything is new).
            if let Some((pi, po)) = self.prev_total.replace((tin, tout)) {
                if tin >= pi && tout >= po {
                    (tin, tout) = (tin - pi, tout - po);
                }
            }
        }
        let model = s.model.or_else(|| self.model.clone());
        if let Some(ts) = s.ts_ms {
            self.last_ts = Some(ts);
        }
        // Undated records join the last dated one's hour (bucket 0 = no date at all).
        let hour = s.ts_ms.or(self.last_ts).map_or(0, |t| t / 3_600_000);
        let t = self.buckets.entry(format!("{hour}|{}", model.as_deref().unwrap_or(""))).or_default();
        t.tokens_in += tin;
        t.tokens_out += tout;
        match s.cost {
            Some(c) => t.cost += c,
            None => t.unpriced += tin + tout,
        }
        if model.is_some() {
            self.last_model = model;
        }
    }

    pub fn totals(&self) -> Totals {
        let mut sum = Totals::default();
        for t in self.buckets.values() {
            sum.add(t);
        }
        sum
    }
}

/// Counts the lines of `path` that `prev` has not: unchanged files keep their counts, grown plain
/// files continue from the last complete line, anything else is counted afresh.
pub fn advance(agent: Agent, path: &Path, prev: Option<FileUsage>, mtime_ms: u64, len: u64) -> Option<FileUsage> {
    let zst = path.extension().is_some_and(|e| e == "zst");
    let mut u = match prev {
        Some(p) if p.mtime_ms == mtime_ms && p.len == len => return Some(p),
        Some(p) if !zst && len >= p.offset && line_ends_at(path, p.offset) => p,
        _ => FileUsage::default(),
    };
    // Only up to the size seen with `mtime_ms`: what is appended meanwhile is the next round's.
    let from = if zst { 0 } else { u.offset };
    let limit = if zst { u64::MAX } else { len.saturating_sub(from) };
    let mut lines = open_lines(path, from, limit)?;
    for line in lines.by_ref() {
        u.feed(agent, &line);
    }
    u.offset = from + lines.consumed;
    u.mtime_ms = mtime_ms;
    u.len = len;
    Some(u)
}

// ---------- cache of aggregates ----------

const CACHE_VERSION: u32 = 2;

#[derive(Serialize, Deserialize, Default)]
struct Cache {
    version: u32,
    files: HashMap<PathBuf, FileUsage>,
}

/// Aggregates by transcript path, kept on disk: after a restart only the new lines are read.
pub(crate) static FILES: LazyLock<Mutex<HashMap<PathBuf, FileUsage>>> = LazyLock::new(|| {
    let cache = std::fs::read(cache_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
        .filter(|c| c.version == CACHE_VERSION)
        .unwrap_or_default();
    Mutex::new(cache.files)
});
/// Paths being counted right now (one worker per file).
static BUSY: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static SAVING: Mutex<()> = Mutex::new(());
static LAST_SAVE: Mutex<Option<Instant>> = Mutex::new(None);

fn cache_file() -> PathBuf {
    kelpie_proto::data_dir().join("usage-cache.json")
}

pub(crate) fn save_cache() {
    let _saving = SAVING.lock();
    *LAST_SAVE.lock() = Some(Instant::now());
    let snapshot = Cache { version: CACHE_VERSION, files: FILES.lock().clone() };
    let Ok(json) = serde_json::to_vec(&snapshot) else { return };
    let path = cache_file();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Saves unless a save happened lately (a live session grows its transcript all the time).
fn save_cache_soon() {
    if LAST_SAVE.lock().is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
        save_cache();
    }
}

/// Claims `path` for counting; `false` when another worker has it.
pub(crate) fn claim(path: &Path) -> bool {
    BUSY.lock().insert(path.to_path_buf())
}

pub(crate) fn release(path: &Path) {
    BUSY.lock().remove(path);
}

/// Brings the aggregate of a claimed `path` up to date and stores it.
pub(crate) fn count_file(agent: Agent, path: &Path, mtime_ms: u64, len: u64) {
    let prev = FILES.lock().remove(path);
    // A file that cannot be read counts for nothing until it changes: it must not keep every
    // scan (and the page waiting on it) busy.
    let u = advance(agent, path, prev, mtime_ms, len).unwrap_or(FileUsage { mtime_ms, len, ..Default::default() });
    FILES.lock().insert(path.to_path_buf(), u);
}

/// (mtime in ms, length) of a file.
pub(crate) fn stat(path: &Path) -> Option<(u64, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((crate::agent_history::millis(m.modified().ok()?), m.len()))
}

/// A Claude conversation's sub-agent transcripts: `<main file without .jsonl>/subagents/*.jsonl`.
/// The folder is named after the conversation id, so they belong to it as exactly as the main file.
pub(crate) fn subagent_files(main: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = jsonl_in(&main.with_extension("").join("subagents")).collect();
    files.sort();
    files
}

/// What the aggregates of a conversation's files say now.
struct Counted {
    totals: Totals,
    /// Model and context of the main file.
    model: Option<String>,
    last: Option<Last>,
    /// Every file is counted to its end.
    complete: bool,
}

/// The conversation's totals (main file plus, for Claude, its sub-agents) and the main file's
/// context. What is new since the last count is read on the spot while it fits one update's
/// budget ([`TAIL_BYTES`] in all); the rest is counted on one background thread.
fn session_counted(agent: Agent, main: &Path) -> Option<Counted> {
    let mut files = vec![main.to_path_buf()];
    if agent == Agent::Claude {
        files.extend(subagent_files(main));
    }
    let mut counted = Counted { totals: Totals::default(), model: None, last: None, complete: true };
    let mut deferred: Vec<(PathBuf, u64, u64)> = Vec::new();
    let mut budget = TAIL_BYTES;
    for (i, path) in files.iter().enumerate() {
        let Some((mtime, len)) = stat(path) else {
            if i == 0 {
                return None;
            }
            continue;
        };
        let (fresh, gap) = {
            let g = FILES.lock();
            let u = g.get(path);
            // Bytes not counted yet (a rewritten file starts over).
            let gap = match u {
                Some(u) if len >= u.offset => len - u.offset,
                _ => len,
            };
            (u.is_some_and(|u| u.mtime_ms == mtime && u.len == len), gap)
        };
        if !fresh {
            if !claim(path) {
                counted.complete = false;
            } else if gap <= budget {
                budget -= gap;
                count_file(agent, path, mtime, len);
                release(path);
            } else {
                counted.complete = false;
                deferred.push((path.clone(), mtime, len));
            }
        }
        if i == 0 && !counted.complete {
            // The main file is read from its end for the context: nothing else this update.
            budget = 0;
        }
        if let Some(u) = FILES.lock().get(path) {
            counted.totals.add(&u.totals());
            if i == 0 {
                counted.model = u.last_model.clone();
                counted.last = u.last.clone();
            }
        }
    }
    if !deferred.is_empty() {
        std::thread::spawn(move || {
            for (path, mtime, len) in deferred {
                count_file(agent, &path, mtime, len);
                release(&path);
            }
            save_cache_soon();
        });
    }
    Some(counted)
}

// ---------- finding a conversation's transcript ----------

/// Looked-up transcripts by (agent, root, id). A miss is remembered only briefly: the agent
/// creates the file a moment after it announces the id.
type Found = (Option<PathBuf>, Instant);
static PATHS: LazyLock<Mutex<HashMap<String, Found>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

fn cached_find(key: String, miss_ttl: Duration, search: impl FnOnce() -> Option<PathBuf>) -> Option<PathBuf> {
    if let Some((found, at)) = PATHS.lock().get(&key) {
        match found {
            Some(p) if p.is_file() => return Some(p.clone()),
            None if at.elapsed() < miss_ttl => return None,
            _ => {}
        }
    }
    let found = search();
    PATHS.lock().insert(key, (found.clone(), Instant::now()));
    found
}

/// The newest file under `dir` (date folders descending) named `rollout-*<suffix>`.
fn find_rollout(dir: &Path, suffix: &str, depth: u8) -> Option<PathBuf> {
    if depth > 5 {
        return None;
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for e in entries {
        let path = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if let Some(found) = find_rollout(&path, suffix, depth + 1) {
                return Some(found);
            }
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(suffix)) {
            return Some(path);
        }
    }
    None
}

/// Separators unified, trailing ones dropped, lower case on Windows: paths compared as text.
fn path_key(p: &str) -> String {
    let flat = p.replace('\\', "/");
    let flat = flat.trim_end_matches('/');
    if cfg!(windows) { flat.to_lowercase() } else { flat.to_string() }
}

/// The accepted `transcript_path` of a Claude hook: exactly `<root>/projects/<slug>/<id>.jsonl`,
/// local, and still inside `<root>/projects` once symlinks are resolved. The hook payload is the
/// session's program talking: network paths (a UNC path would make Windows open an SMB
/// connection) and `..` are refused by their text, before any file is touched.
fn claude_hook_path(path: &str, id: &str, root: &Path) -> Option<PathBuf> {
    let flat = path.replace('\\', "/");
    if flat.starts_with("//") || flat.split('/').any(|c| c == "..") {
        return None;
    }
    let p = Path::new(path);
    let projects = root.join("projects");
    let shaped = p.extension().is_some_and(|e| e == "jsonl")
        && p.file_stem().is_some_and(|s| s == id)
        && p.parent().and_then(Path::parent).is_some_and(|d| path_key(&d.to_string_lossy()) == path_key(&projects.to_string_lossy()));
    if !shaped || !p.is_file() {
        return None;
    }
    let (real, inside) = (p.canonicalize().ok()?, projects.canonicalize().ok()?);
    real.starts_with(inside).then(|| p.to_path_buf())
}

/// Days since 1970-01-01 → (year, month, day) (Howard Hinnant's civil-from-days).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Codex thread ids are UUIDv7: the first 48 bits are the creation time. The rollout sits in the
/// day folder of that time (the folder uses local time: a day either side is looked at too), so
/// a few stats find it without walking the whole tree.
fn find_rollout_by_date(root: &Path, id: &str, suffix: &str) -> Option<PathBuf> {
    let ms = u64::from_str_radix(id.replace('-', "").get(..12)?, 16).ok()?;
    let day = (ms / 86_400_000) as i64;
    [day, day - 1, day + 1].into_iter().find_map(|d| {
        let (y, m, dd) = civil_from_days(d);
        let folder = root.join("sessions").join(format!("{y:04}")).join(format!("{m:02}")).join(format!("{dd:02}"));
        std::fs::read_dir(folder).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("rollout-") && n.ends_with(suffix)))
    })
}

/// The transcript of conversation `id` under an agent's `root` (Claude / Codex: the config home;
/// OMP: its sessions folder). `None` when there is none yet.
pub fn resolve_in(agent: Agent, root: &Path, id: &str, transcript_path: Option<&str>, cwd: Option<&str>) -> Option<PathBuf> {
    if !valid_id(id) || id == "latest" {
        return None;
    }
    match agent {
        Agent::Claude => {
            if let Some(p) = transcript_path.and_then(|p| claude_hook_path(p, id, root)) {
                return Some(p);
            }
            let file = format!("{id}.jsonl");
            // The folder the session started in: one stat, never remembered as a miss (the file
            // appears a moment after the id is announced).
            if let Some(p) = cwd.map(|c| claude_project_dir(root, c).join(&file)).filter(|p| p.is_file()) {
                return Some(p);
            }
            cached_find(format!("claude|{}|{id}", root.display()), Duration::from_secs(3), || dirs_in(&root.join("projects")).map(|d| d.join(&file)).find(|p| p.is_file()))
        }
        Agent::Codex => {
            let suffix = format!("-{id}.jsonl");
            if let Some(p) = find_rollout_by_date(root, id, &suffix) {
                return Some(p);
            }
            // Not where its id says (a thread resumed on a later day, an odd id): walk the tree, but
            // remember a miss for a while, the walk is not cheap.
            cached_find(format!("codex|{}|{id}", root.display()), Duration::from_secs(20), || find_rollout(&root.join("sessions"), &suffix, 0))
        }
        Agent::Omp => {
            let suffix = format!("_{id}.jsonl");
            cached_find(format!("omp|{}|{id}", root.display()), Duration::from_secs(10), || {
                dirs_in(root).flat_map(|d| jsonl_in(&d).collect::<Vec<_>>()).find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(&suffix)))
            })
        }
    }
}

fn agent_of(kind: &str) -> Option<Agent> {
    match kind {
        "claude" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        "omp" => Some(Agent::Omp),
        _ => None,
    }
}

// ---------- the command ----------

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SessionUsage {
    context_used: u64,
    /// `None`: the agent does not say (OMP) or the model is not in the price table.
    context_window: Option<u64>,
    tokens_in: u64,
    tokens_out: u64,
    /// Whole-conversation cost; `None` when any of it has no price (unknown model, Codex).
    cost: Option<f64>,
    /// The agent's own number (OMP) rather than an estimate at API prices (Claude).
    cost_exact: bool,
    model: Option<String>,
    /// The totals cover the whole transcript (false while a background count is still running).
    totals: bool,
}

fn usage_of(agent: Agent, path: &Path) -> Option<SessionUsage> {
    let c = session_counted(agent, path)?;
    // A fully counted file knows its own last context (one small read per update); only while
    // the count is still running does the end of the file say it.
    let last = if c.complete { c.last.clone() } else { tail_last(agent, path) };
    let t = c.totals;
    Some(SessionUsage {
        context_used: last.as_ref().map_or(0, |l| l.context_used),
        context_window: last.as_ref().and_then(|l| l.context_window),
        tokens_in: t.tokens_in,
        tokens_out: t.tokens_out,
        cost: (t.unpriced == 0 && t.tokens_in + t.tokens_out > 0).then_some(t.cost),
        cost_exact: agent == Agent::Omp,
        model: last.and_then(|l| l.model).or(c.model),
        totals: c.complete,
    })
}

/// Usage of the conversation `agent_session_id` of an agent session. `None` without an id or
/// without a transcript that provably belongs to it.
#[tauri::command]
pub async fn session_usage(
    kind: String,
    agent_session_id: Option<String>,
    transcript_path: Option<String>,
    cwd: Option<String>,
    home: Option<String>,
) -> CmdResult<Option<SessionUsage>> {
    let (Some(agent), Some(id)) = (agent_of(&kind), agent_session_id) else { return Ok(None) };
    blocking(move || {
        let root = match agent {
            Agent::Claude => home_or(home.as_deref(), claude_dir),
            Agent::Codex => home_or(home.as_deref(), codex_dir),
            Agent::Omp => crate::agent_history::home().join(".omp/agent/sessions"),
        };
        let Some(path) = resolve_in(agent, &root, &id, transcript_path.as_deref(), cwd.as_deref()) else { return Ok(None) };
        Ok(usage_of(agent, &path))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(items: &[&str]) -> String {
        items.iter().map(|l| format!("{l}\n")).collect()
    }

    const CLAUDE_A1: &str = r#"{"type":"assistant","timestamp":"2026-10-24T01:00:00.000Z","cwd":"D:\\p","message":{"id":"msg_A","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_creation_input_tokens":1000,"cache_read_input_tokens":2000,"output_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":600,"ephemeral_1h_input_tokens":400}},"content":[{"type":"text","text":"hi"}]}}"#;
    // The same API message on its next content block: identical usage, same id.
    const CLAUDE_A2: &str = r#"{"type":"assistant","timestamp":"2026-10-24T01:00:01.000Z","message":{"id":"msg_A","model":"claude-opus-5-5","usage":{"input_tokens":10,"cache_creation_input_tokens":1000,"cache_read_input_tokens":2000,"output_tokens":50,"cache_creation":{"ephemeral_5m_input_tokens":600,"ephemeral_1h_input_tokens":400}},"content":[{"type":"tool_use","name":"x"}]}}"#;
    const CLAUDE_B: &str = r#"{"type":"assistant","timestamp":"2026-10-24T01:05:00.000Z","message":{"id":"msg_B","model":"claude-opus-5-5","usage":{"input_tokens":20,"cache_creation_input_tokens":0,"cache_read_input_tokens":3000,"output_tokens":100},"content":[]}}"#;
    const CLAUDE_SIDE: &str = r#"{"type":"assistant","isSidechain":true,"timestamp":"2026-10-24T01:06:00.000Z","message":{"id":"msg_S","model":"claude-opus-5-5","usage":{"input_tokens":1,"output_tokens":1},"content":[]}}"#;
    const CLAUDE_SYNTH: &str = r#"{"type":"assistant","message":{"id":"msg_X","model":"<synthetic>","usage":{"input_tokens":0,"output_tokens":0},"content":[]}}"#;

    fn feed_all(agent: Agent, items: &[&str]) -> FileUsage {
        let mut u = FileUsage::default();
        for l in items {
            u.feed(agent, l);
        }
        u
    }

    #[test]
    fn claude_counts_cache_tokens_once_per_message() {
        let u = feed_all(Agent::Claude, &[CLAUDE_A1, CLAUDE_A2, CLAUDE_B, CLAUDE_SYNTH]);
        let t = u.totals();
        // A: 10 + 2000 + 600 + 400 in, 50 out. B: 20 + 3000 in, 100 out.
        assert_eq!((t.tokens_in, t.tokens_out), (3010 + 3020, 150));
        // Opus 5.5: 4 / 20, cache read 0.20, 5 m write 5.00, 1 h write 8.00 (USD / MTok).
        let a = (10.0 * 4.0 + 2000.0 * 0.2 + 600.0 * 5.0 + 400.0 * 8.0 + 50.0 * 20.0) / 1e6;
        let b = (20.0 * 4.0 + 3000.0 * 0.2 + 100.0 * 20.0) / 1e6;
        assert!((t.cost - (a + b)).abs() < 1e-12, "{} vs {}", t.cost, a + b);
        assert_eq!(t.unpriced, 0);
        assert_eq!(u.cwd.as_deref(), Some("D:\\p"));
        assert_eq!(u.last_model.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn claude_context_is_the_last_main_chain_message() {
        let bytes = lines(&[CLAUDE_A1, CLAUDE_B, CLAUDE_SIDE]);
        let last = last_in(Agent::Claude, bytes.as_bytes()).unwrap();
        // B: input + cache read + output; the sidechain line after it is not the conversation's context.
        assert_eq!(last.context_used, 20 + 3000 + 100);
        assert_eq!(last.context_window, Some(1_000_000));
        assert_eq!(last.model.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn unknown_claude_model_has_tokens_but_no_cost_or_window() {
        let line = CLAUDE_B.replace("claude-opus-5-5", "claude-mystery-9");
        let u = feed_all(Agent::Claude, &[&line]);
        assert_eq!(u.totals().cost, 0.0);
        assert_eq!(u.totals().unpriced, 3020 + 100);
        let last = last_in(Agent::Claude, line.as_bytes()).unwrap();
        assert_eq!((last.context_used, last.context_window), (3120, None));
    }

    #[test]
    fn model_ids_normalise() {
        assert_eq!(normalize_model("claude-opus-4-8[1m]"), "opus-4-8");
        assert_eq!(normalize_model("claude-haiku-4-5-20251001"), "haiku-4-5");
        assert_eq!(normalize_model("anthropic.claude-sonnet-4.6"), "sonnet-4-6");
        assert!(price_of("claude-opus-5").is_some());
        assert!(price_of("claude-opus-5-1").is_none(), "no price is guessed for a version that is not listed");
        assert_eq!(price_of("claude-haiku-4-5-20251001").unwrap().context, 200_000);
    }

    #[test]
    fn haiku_55_long_prompts_cost_more() {
        let row = price_of("claude-haiku-5-5").unwrap();
        let short = claude_cost(row, 100_000, 0, 0, 0, 1_000_000);
        assert!((short - (100_000.0 * 0.1 + 1_000_000.0 * 0.5) / 1e6).abs() < 1e-12);
        let long = claude_cost(row, 100_001, 0, 0, 0, 1_000_000);
        assert!((long - (100_001.0 * 0.5 + 1_000_000.0 * 2.5) / 1e6).abs() < 1e-12);
    }

    const CODEX_META: &str = r#"{"type":"session_meta","payload":{"id":"t1","cwd":"/w"}}"#;
    const CODEX_CTX: &str = r#"{"timestamp":"2026-10-24T02:00:00Z","type":"turn_context","payload":{"model":"gpt-6.1-sol","cwd":"/w"}}"#;
    const CODEX_T1: &str = r#"{"timestamp":"2026-10-24T02:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":500,"output_tokens":20,"total_tokens":1020},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":500,"output_tokens":20,"total_tokens":1020},"model_context_window":258400}}}"#;
    const CODEX_NULL: &str = r#"{"timestamp":"2026-10-24T02:00:06Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"primary":{"used_percent":5.0}}}}"#;
    const CODEX_T2: &str = r#"{"timestamp":"2026-10-24T02:10:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":3000,"cached_input_tokens":500,"output_tokens":120,"total_tokens":3120},"last_token_usage":{"input_tokens":2000,"cached_input_tokens":0,"output_tokens":100,"total_tokens":2100},"model_context_window":258400}}}"#;

    #[test]
    fn codex_token_counts_skip_null_info_and_take_differences() {
        let u = feed_all(Agent::Codex, &[CODEX_META, CODEX_CTX, CODEX_T1, CODEX_NULL, CODEX_T1, CODEX_T2]);
        let t = u.totals();
        // Running totals 3000 / 120 at the end; a repeated record adds nothing.
        assert_eq!((t.tokens_in, t.tokens_out), (3000, 120));
        assert_eq!(t.cost, 0.0, "no price source for OpenAI models");
        assert_eq!(u.last_model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(u.cwd.as_deref(), Some("/w"));
        let bytes = lines(&[CODEX_META, CODEX_CTX, CODEX_T1, CODEX_T2, CODEX_NULL]);
        let last = last_in(Agent::Codex, bytes.as_bytes()).unwrap();
        assert_eq!((last.context_used, last.context_window), (2100, Some(258_400)));
        assert_eq!(last.model.as_deref(), Some("gpt-6.1-sol"));
        // Only rate-limit records: nothing to show.
        assert_eq!(last_in(Agent::Codex, lines(&[CODEX_NULL]).as_bytes()), None);
    }

    const OMP_HEADER: &str = r#"{"type":"session","version":3,"id":"o1","cwd":"C:\\w"}"#;
    const OMP_A: &str = r#"{"type":"message","id":"m1","timestamp":"2026-10-24T03:00:00.000Z","message":{"role":"assistant","model":"gpt-x","usage":{"input":15780,"output":30,"cacheRead":12224,"cacheWrite":0,"totalTokens":28034,"cost":{"input":0.02,"output":0.0001,"total":0.02540224}},"contextSnapshot":{"promptTokens":28004}}}"#;
    const OMP_USER: &str = r#"{"type":"message","id":"m0","message":{"role":"user","content":[{"type":"text","text":"hello"}]}}"#;

    #[test]
    fn omp_cost_is_the_agents_own() {
        let u = feed_all(Agent::Omp, &[OMP_HEADER, OMP_USER, OMP_A, OMP_A]);
        let t = u.totals();
        assert_eq!((t.tokens_in, t.tokens_out), (15780 + 12224, 30));
        assert!((t.cost - 0.02540224).abs() < 1e-12);
        assert_eq!(u.cwd.as_deref(), Some("C:\\w"));
        let last = last_in(Agent::Omp, lines(&[OMP_HEADER, OMP_A]).as_bytes()).unwrap();
        assert_eq!((last.context_used, last.context_window), (28004, None));
    }

    #[test]
    fn a_truncated_last_line_is_skipped() {
        let mut bytes = lines(&[CLAUDE_A1, CLAUDE_B]).into_bytes();
        bytes.extend_from_slice(&CLAUDE_A2.as_bytes()[..60]); // a writer in the middle of a line
        assert_eq!(last_in(Agent::Claude, &bytes).unwrap().context_used, 20 + 3000 + 100);
        // Malformed JSON that looks like a usage line is counted as a parse failure, not a record.
        let bad = r#"{"type":"assistant","usage": nope "assistant"}"#;
        assert!(sample(Agent::Claude, bad).is_err());
        assert_eq!(last_in(Agent::Claude, bad.as_bytes()), None);
    }

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kelpie-usage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reverse_tail_reads_at_most_64_kb() {
        let dir = temp_dir("tail");
        let path = dir.join("big.jsonl");
        // A usage record early on, then ~400 KB of lines with none: the tail must not reach it.
        let filler = format!(r#"{{"type":"user","message":{{"content":"{}"}}}}"#, "x".repeat(900));
        let mut text = lines(&[CLAUDE_A1]);
        for _ in 0..450 {
            text.push_str(&filler);
            text.push('\n');
        }
        std::fs::write(&path, &text).unwrap();
        assert!(read_tail(&path, TAIL_BYTES).unwrap().len() as u64 <= TAIL_BYTES);
        assert_eq!(tail_last(Agent::Claude, &path), None);
        // A record inside the last 64 KB is found, and a line cut by the window start is dropped.
        text.push_str(&lines(&[CLAUDE_B]));
        std::fs::write(&path, &text).unwrap();
        assert_eq!(tail_last(Agent::Claude, &path).unwrap().context_used, 3120);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn aggregate_continues_from_the_last_complete_line() {
        let dir = temp_dir("agg");
        let path = dir.join("a.jsonl");
        // The second message is still being written (no newline yet).
        let mut text = lines(&[CLAUDE_A1, CLAUDE_A2]);
        text.push_str(CLAUDE_B);
        std::fs::write(&path, &text).unwrap();
        let len = text.len() as u64;
        let first = advance(Agent::Claude, &path, None, 1, len).unwrap();
        assert_eq!(first.totals().tokens_out, 50);
        assert!(first.offset < len);
        // The line completes and another arrives: nothing is counted twice.
        text.push('\n');
        text.push_str(&lines(&[CLAUDE_A2]));
        std::fs::write(&path, &text).unwrap();
        let second = advance(Agent::Claude, &path, Some(first.clone()), 2, text.len() as u64).unwrap();
        assert_eq!(second.totals().tokens_out, 150);
        assert_eq!(second.offset, text.len() as u64);
        // Unchanged (mtime, len): the very same numbers.
        assert_eq!(advance(Agent::Claude, &path, Some(second.clone()), 2, text.len() as u64).unwrap().totals(), second.totals());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn two_claude_sessions_in_one_folder_map_to_their_own_transcripts() {
        let root = temp_dir("claude-root");
        let project = root.join("projects").join("D--work-proj");
        std::fs::create_dir_all(&project).unwrap();
        let (a, b) = ("11111111-aaaa-4000-8000-000000000001", "22222222-bbbb-4000-8000-000000000002");
        std::fs::write(project.join(format!("{a}.jsonl")), lines(&[CLAUDE_A1])).unwrap();
        std::fs::write(project.join(format!("{b}.jsonl")), lines(&[CLAUDE_B])).unwrap();
        let cwd = Some("D:\\work\\proj");
        let pa = resolve_in(Agent::Claude, &root, a, None, cwd).unwrap();
        let pb = resolve_in(Agent::Claude, &root, b, None, cwd).unwrap();
        assert_ne!(pa, pb);
        assert_eq!(usage_of(Agent::Claude, &pa).unwrap().tokens_out, 50);
        assert_eq!(usage_of(Agent::Claude, &pb).unwrap().tokens_out, 100);
        // A hook path counts only for the conversation it is named after.
        let hook_a = pa.to_string_lossy().into_owned();
        assert_eq!(resolve_in(Agent::Claude, &root, a, Some(&hook_a), None), Some(pa.clone()));
        assert_eq!(resolve_in(Agent::Claude, &root, b, Some(&hook_a), cwd), Some(pb.clone()), "a's path is not b's transcript");
        // A transcript that appears after a first look is found by the next one.
        let late = "44444444-dddd-4000-8000-000000000004";
        assert_eq!(resolve_in(Agent::Claude, &root, late, None, cwd), None);
        std::fs::write(project.join(format!("{late}.jsonl")), lines(&[CLAUDE_B])).unwrap();
        assert!(resolve_in(Agent::Claude, &root, late, None, cwd).is_some());
        // No id, no guess: the folder has transcripts but none is picked.
        assert_eq!(resolve_in(Agent::Claude, &root, "", None, cwd), None);
        assert_eq!(resolve_in(Agent::Claude, &root, "33333333-cccc-4000-8000-000000000003", None, cwd), None);
        // A path outside any `projects/<slug>/` folder is not accepted even when named right.
        let stray = root.join(format!("{a}.jsonl"));
        std::fs::write(&stray, lines(&[CLAUDE_B])).unwrap();
        assert_eq!(claude_hook_path(&stray.to_string_lossy(), a, &root), None);
        assert_eq!(resolve_in(Agent::Claude, &root, "../x", None, None), None);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn codex_rollouts_resolve_by_thread_id() {
        let root = temp_dir("codex-root");
        let day = root.join("sessions/2026/10/24");
        std::fs::create_dir_all(&day).unwrap();
        let (a, b) = ("01a16a15-af7e-7012-b903-fbdc6330f660", "01a16a99-0000-7000-8000-000000000000");
        std::fs::write(day.join(format!("rollout-2026-10-24T01-46-08-{a}.jsonl")), lines(&[CODEX_T1])).unwrap();
        std::fs::write(day.join(format!("rollout-2026-10-24T02-00-00-{b}.jsonl")), lines(&[CODEX_T2])).unwrap();
        let pa = resolve_in(Agent::Codex, &root, a, None, None).unwrap();
        let pb = resolve_in(Agent::Codex, &root, b, None, None).unwrap();
        assert!(pa.to_string_lossy().ends_with(&format!("{a}.jsonl")));
        assert_eq!(usage_of(Agent::Codex, &pa).unwrap().context_used, 1020);
        assert_eq!(usage_of(Agent::Codex, &pb).unwrap().context_used, 2100);
        assert_eq!(resolve_in(Agent::Codex, &root, "latest", None, None), None, "`latest` is a resume request, not a thread");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn omp_sessions_resolve_by_session_id() {
        let root = temp_dir("omp-root");
        let dir = root.join("proj-slug");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("2026-10-24T03-00-00-000Z_o1.jsonl"), lines(&[OMP_HEADER, OMP_A])).unwrap();
        std::fs::write(dir.join("2026-10-24T04-00-00-000Z_o2.jsonl"), lines(&[OMP_HEADER])).unwrap();
        let p = resolve_in(Agent::Omp, &root, "o1", None, None).unwrap();
        let u = usage_of(Agent::Omp, &p).unwrap();
        assert!(u.cost_exact && (u.cost.unwrap() - 0.02540224).abs() < 1e-12);
        assert_eq!(u.context_window, None);
        assert!(resolve_in(Agent::Omp, &root, "o3", None, None).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Real layout: a sub-agent repeats one message on several lines while its output grows.
    fn sub_line(id: &str, out: u64) -> String {
        format!(
            r#"{{"type":"assistant","isSidechain":true,"timestamp":"2026-10-24T01:10:00.000Z","cwd":"D:\\p","message":{{"id":"{id}","model":"claude-opus-5-5","usage":{{"input_tokens":100,"cache_read_input_tokens":1000,"cache_creation_input_tokens":0,"output_tokens":{out}}},"content":[]}}}}"#
        )
    }

    #[test]
    fn a_repeated_message_counts_its_highest_output_once() {
        // Output 1 -> 40 -> 300, then an older (lower) line again: 300 tokens in all, at the output rate.
        let (l1, l2, l3, l4) = (sub_line("msg_S", 1), sub_line("msg_S", 40), sub_line("msg_S", 300), sub_line("msg_S", 40));
        let u = feed_all(Agent::Claude, &[&l1, &l2, &l3, &l4]);
        let t = u.totals();
        assert_eq!((t.tokens_in, t.tokens_out), (1100, 300));
        assert!((t.cost - price(100, 1000, 0, 0, 300)).abs() < 1e-12, "{}", t.cost);
        // The same lines in the order a main file writes them (identical usage) change nothing.
        assert_eq!(feed_all(Agent::Claude, &[&l3, &l3, &l3]).totals(), feed_all(Agent::Claude, &[&l3]).totals());
    }

    fn price(inp: u64, read: u64, w5: u64, w1: u64, out: u64) -> f64 {
        (inp as f64 * 4.0 + read as f64 * 0.2 + w5 as f64 * 5.0 + w1 as f64 * 8.0 + out as f64 * 20.0) / 1e6
    }

    #[test]
    fn sub_agent_transcripts_add_to_the_session_but_not_to_its_context() {
        let root = temp_dir("subagents");
        let project = root.join("projects").join("D--work-proj");
        let id = "55555555-eeee-4000-8000-000000000005";
        std::fs::create_dir_all(project.join(id).join("subagents")).unwrap();
        std::fs::write(project.join(format!("{id}.jsonl")), lines(&[CLAUDE_B])).unwrap();
        let (s1, s2) = (sub_line("msg_S", 1), sub_line("msg_S", 300));
        std::fs::write(project.join(id).join("subagents").join("agent-a1.jsonl"), lines(&[&s1, &s2])).unwrap();
        std::fs::write(project.join(id).join("subagents").join("agent-a2.jsonl"), lines(&[&sub_line("msg_T", 10)])).unwrap();
        let main = resolve_in(Agent::Claude, &root, id, None, Some("D:\\work\\proj")).unwrap();
        let u = usage_of(Agent::Claude, &main).unwrap();
        // Main: 20 + 3000 in, 100 out. Sub-agents: 1100 + 1100 in, 300 + 10 out.
        assert_eq!((u.tokens_in, u.tokens_out), (3020 + 2200, 100 + 310));
        assert!((u.cost.unwrap() - (price(20, 3000, 0, 0, 100) + price(100, 1000, 0, 0, 300) + price(100, 1000, 0, 0, 10))).abs() < 1e-9);
        assert_eq!(u.context_used, 3120, "context is the main conversation's");
        assert!(u.totals);
        assert_eq!(subagent_files(&main).len(), 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_compaction_hides_the_old_fill() {
        let boundary = r#"{"type":"system","subtype":"compact_boundary","content":"Conversation compacted"}"#;
        let bytes = lines(&[CLAUDE_B, boundary]);
        assert_eq!(last_in(Agent::Claude, bytes.as_bytes()), None);
        assert_eq!(feed_all(Agent::Claude, &[CLAUDE_B, boundary]).last, None);
        let after = lines(&[CLAUDE_B, boundary, CLAUDE_A1]);
        assert_eq!(last_in(Agent::Claude, after.as_bytes()).unwrap().context_used, 3060);
    }

    #[test]
    fn hook_paths_must_be_local_and_inside_the_config_home() {
        let root = temp_dir("hookpath");
        let other = temp_dir("hookpath-other");
        let id = "66666666-ffff-4000-8000-000000000006";
        let inside = root.join("projects").join("slug");
        let outside = other.join("projects").join("slug");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(inside.join(format!("{id}.jsonl")), lines(&[CLAUDE_B])).unwrap();
        std::fs::write(outside.join(format!("{id}.jsonl")), lines(&[CLAUDE_B])).unwrap();
        let ok = inside.join(format!("{id}.jsonl"));
        assert!(claude_hook_path(&ok.to_string_lossy(), id, &root).is_some());
        // Another folder named `projects` is not this account's.
        assert_eq!(claude_hook_path(&outside.join(format!("{id}.jsonl")).to_string_lossy(), id, &root), None);
        // `..` and network paths are refused by their text.
        let dotted = format!("{}/projects/slug/../slug/{id}.jsonl", root.to_string_lossy().replace('\\', "/"));
        assert_eq!(claude_hook_path(&dotted, id, &root), None);
        for unc in [format!("\\\\host\\share\\projects\\slug\\{id}.jsonl"), format!("//host/share/projects/slug/{id}.jsonl"), format!("\\\\?\\UNC\\host\\share\\projects\\slug\\{id}.jsonl")] {
            assert_eq!(claude_hook_path(&unc, id, &root), None, "{unc}");
        }
        // A hook that lies falls back to the lookup by id, which finds the real one.
        assert_eq!(resolve_in(Agent::Claude, &root, id, Some(&outside.join(format!("{id}.jsonl")).to_string_lossy()), None), Some(ok));
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&other).ok();
    }

    #[test]
    fn codex_rollouts_are_found_by_the_date_in_their_id() {
        // 2026-10-24T01:46:08Z as the milliseconds a UUIDv7 starts with.
        let ms: u64 = 1_792_806_368_000;
        let hex = format!("{ms:012x}");
        let id = format!("{}-{}-7012-b903-fbdc6330f661", &hex[..8], &hex[8..]);
        let (y, m, d) = civil_from_days((ms / 86_400_000) as i64);
        let root = temp_dir("codex-date");
        let day = root.join("sessions").join(format!("{y:04}/{m:02}/{d:02}"));
        std::fs::create_dir_all(&day).unwrap();
        let file = day.join(format!("rollout-x-{id}.jsonl"));
        std::fs::write(&file, lines(&[CODEX_T1])).unwrap();
        assert_eq!(find_rollout_by_date(&root, &id, &format!("-{id}.jsonl")), Some(file));
        assert_eq!(civil_from_days(20_750), (2026, 10, 24));
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        std::fs::remove_dir_all(&root).ok();
    }
}
