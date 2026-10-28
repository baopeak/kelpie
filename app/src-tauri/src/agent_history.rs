//! Read access to agents' own on-disk session stores: resume id discovery when switching a CLI
//! session to chat mode, chat history, and the session history list. These formats are internal
//! to each agent, so everything here is defensive: unknown lines are skipped.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::fs_ops::{err, CmdResult};

pub(crate) fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

pub(crate) fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

/// An account's config home (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`), else the default one.
pub(crate) fn home_or(dir: Option<&str>, default: fn() -> PathBuf) -> PathBuf {
    dir.filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(default)
}

/// Claude Code names project dirs after the cwd with every non-alphanumeric char → '-'.
pub(crate) fn claude_project_dir(root: &Path, cwd: &str) -> PathBuf {
    let escaped: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    root.join("projects").join(escaped)
}

fn newest(files: impl Iterator<Item = PathBuf>) -> Option<PathBuf> {
    files
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

pub(crate) fn jsonl_in(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
}

#[derive(Serialize)]
pub struct AgentSessionRef {
    id: String,
    /// Session file (OMP resumes by path).
    path: String,
}

/// Most recent session of `kind` started in `cwd` (Claude / OMP; Codex uses `thread/list`).
#[tauri::command]
pub async fn agent_latest_session(kind: String, cwd: String, home: Option<String>) -> CmdResult<Option<AgentSessionRef>> {
    crate::fs_ops::blocking(move || agent_latest_session_blocking(kind, cwd, home)).await
}

fn agent_latest_session_blocking(kind: String, cwd: String, account: Option<String>) -> CmdResult<Option<AgentSessionRef>> {
    match kind.as_str() {
        "claude" => Ok(newest(jsonl_in(&claude_project_dir(&home_or(account.as_deref(), claude_dir), &cwd))).map(|p| AgentSessionRef {
            id: p.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
            path: p.to_string_lossy().into_owned(),
        })),
        "omp" => {
            let root = home().join(".omp/agent/sessions");
            let mut files: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(&root)
                .map_err(err)?
                .filter_map(|e| e.ok())
                .flat_map(|d| jsonl_in(&d.path()).collect::<Vec<_>>())
                .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
                .collect();
            files.sort_by_key(|f| std::cmp::Reverse(f.0));
            let want = normalize_cwd(&cwd);
            for (_, path) in files.into_iter().take(300) {
                let Ok(file) = std::fs::File::open(&path) else { continue };
                for line in BufReader::new(file).lines().take(4).map_while(Result::ok) {
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                    if v["type"] == "session" && v["cwd"].as_str().map(normalize_cwd) == Some(want.clone()) {
                        return Ok(Some(AgentSessionRef {
                            id: v["id"].as_str().unwrap_or_default().to_string(),
                            path: path.to_string_lossy().into_owned(),
                        }));
                    }
                }
            }
            Ok(None)
        }
        _ => Ok(None),
    }
}

fn normalize_cwd(p: &str) -> String {
    let s = p.replace('\\', "/");
    let s = s.trim_end_matches('/');
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

/// Raw JSONL lines of a Claude Code transcript (tail, capped), for rendering chat history.
#[tauri::command]
pub async fn agent_claude_transcript(session_id: String, cwd: Option<String>, home: Option<String>) -> CmdResult<Vec<String>> {
    crate::fs_ops::blocking(move || agent_claude_transcript_blocking(session_id, cwd, home)).await
}

fn agent_claude_transcript_blocking(session_id: String, cwd: Option<String>, home: Option<String>) -> CmdResult<Vec<String>> {
    let file_name = format!("{session_id}.jsonl");
    if file_name.contains(['/', '\\']) {
        return Err("invalid session id".into());
    }
    let root = home_or(home.as_deref(), claude_dir);
    let direct = cwd.map(|c| claude_project_dir(&root, &c).join(&file_name)).filter(|p| p.exists());
    let path = match direct {
        Some(p) => p,
        None => std::fs::read_dir(root.join("projects"))
            .map_err(err)?
            .filter_map(|e| e.ok())
            .map(|d| d.path().join(&file_name))
            .find(|p| p.exists())
            .ok_or_else(|| format!("transcript {session_id} not found"))?,
    };
    let file = std::fs::File::open(path).map_err(err)?;
    let mut lines: Vec<String> = BufReader::new(file).lines().map_while(Result::ok).collect();
    const MAX_LINES: usize = 6000;
    if lines.len() > MAX_LINES {
        lines.drain(..lines.len() - MAX_LINES);
    }
    Ok(lines)
}

// ---------- session history list ----------
//
// Transcripts add up to gigabytes (a long agent session alone can be hundreds of MB), so the
// list is built from the start of each file only (plus Claude's tail for its latest title).
// Prompt counts need the whole file: they are computed in the background, resumed from the
// last counted byte for append-only `.jsonl` files, and kept on disk across launches.

/// One past agent conversation, as shown in the Sessions panel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HistoryItem {
    agent: String,
    /// Id the agent resumes by.
    id: String,
    /// Folder the conversation ran in, as the agent recorded it (resume must use exactly this:
    /// agents look transcripts up by it).
    cwd: String,
    /// `cwd` in canonical form for matching sidebar projects (long names, no `\\?\` prefix).
    folder: String,
    title: Option<String>,
    first_prompt: Option<String>,
    /// Last write to the transcript, ms since the epoch.
    updated_at: u64,
    /// User prompts (Claude / OMP) or turns (Codex); `None` until the background count gets here.
    turns: Option<u32>,
    /// Config home of the account the conversation belongs to (`None` = the agent's default):
    /// resuming must run on the same account.
    #[serde(default)]
    home: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Agent {
    Claude,
    Codex,
    Omp,
}

const MAX_ITEMS: usize = 3000;
const MAX_TEXT: usize = 240;
/// Ids, folder and first prompt are all near the top of a transcript.
const HEAD_BYTES: u64 = 512 * 1024;
/// Claude appends a new AI title as the conversation moves on: the newest is near the end.
const TAIL_BYTES: u64 = 256 * 1024;

#[derive(Serialize, Deserialize, Clone)]
struct Meta {
    mtime_ms: u64,
    len: u64,
    item: Option<HistoryItem>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
struct TurnCount {
    mtime_ms: u64,
    len: u64,
    /// End of the last complete line counted (plain files resume counting here).
    offset: u64,
    turns: u32,
}

/// Bump when parsing changes, so stale entries from older builds are re-read.
const CACHE_VERSION: u32 = 2;

/// Head-parsed transcripts and prompt counts by path, kept on disk: after the first launch a
/// scan only stats files and re-reads the ones that changed.
#[derive(Serialize, Deserialize, Default, Clone)]
struct Cache {
    version: u32,
    meta: HashMap<PathBuf, Meta>,
    turns: HashMap<PathBuf, TurnCount>,
}

static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| Mutex::new(load_cache()));
static COUNTING: AtomicBool = AtomicBool::new(false);
/// Scans run one at a time (fast project switching must not stack all-core scans).
static SCANNING: Mutex<()> = Mutex::new(());
/// Writers of the cache file share one temp path.
static SAVING: Mutex<()> = Mutex::new(());

fn cache_file() -> PathBuf {
    kelpie_proto::data_dir().join("history-cache.json")
}

fn load_cache() -> Cache {
    std::fs::read(cache_file())
        .ok()
        .and_then(|b| serde_json::from_slice::<Cache>(&b).ok())
        .filter(|c| c.version == CACHE_VERSION)
        .unwrap_or(Cache { version: CACHE_VERSION, ..Default::default() })
}

fn save_cache() {
    let _saving = SAVING.lock();
    // Serialize a copy: scans and counting workers must not wait on the encoder.
    let snapshot = CACHE.lock().clone();
    let Ok(json) = serde_json::to_vec(&snapshot) else { return };
    let path = cache_file();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub(crate) fn dirs_in(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir).into_iter().flatten().filter_map(|e| e.ok()).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path())
}

pub(crate) fn codex_dir() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
}

fn is_zst(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "zst")
}

/// Extra account homes (beyond the agents' defaults) whose transcripts are listed too.
#[derive(Deserialize, Default, Clone)]
pub struct AccountHomes {
    #[serde(default)]
    claude: Vec<String>,
    #[serde(default)]
    codex: Vec<String>,
}

/// Every transcript file of every agent, with the account home it belongs to (`None` = the
/// default). Codex nests rollouts under `sessions/YYYY/MM/DD/`.
pub(crate) fn transcript_files(homes: &AccountHomes) -> Vec<(Agent, PathBuf, Option<String>)> {
    let mut out = Vec::new();
    let claude_roots = std::iter::once((claude_dir(), None)).chain(homes.claude.iter().map(|h| (PathBuf::from(h), Some(h.clone()))));
    for (root, home) in claude_roots {
        for d in dirs_in(&root.join("projects")) {
            out.extend(jsonl_in(&d).map(|p| (Agent::Claude, p, home.clone())));
        }
    }
    let codex_roots = std::iter::once((codex_dir(), None)).chain(homes.codex.iter().map(|h| (PathBuf::from(h), Some(h.clone()))));
    for (root, home) in codex_roots {
        let mut stack = vec![root.join("sessions")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().filter_map(|e| e.ok()) {
                let p = e.path();
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(p);
                } else if p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.ends_with(".jsonl") || n.ends_with(".jsonl.zst")) {
                    out.push((Agent::Codex, p, home.clone()));
                }
            }
        }
    }
    for d in dirs_in(&home().join(".omp/agent/sessions")) {
        out.extend(jsonl_in(&d).map(|p| (Agent::Omp, p, None)));
    }
    out
}

pub(crate) fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Prompt text for a list row: markup stripped, whitespace collapsed, capped.
fn clean_text(s: &str) -> Option<String> {
    static TAGS: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"</?[A-Za-z][\w:-]*(\s[^>]*)?/?>").unwrap());
    let stripped = TAGS.replace_all(s, " ");
    let mut out = String::new();
    for word in stripped.split_whitespace() {
        if out.len() >= MAX_TEXT {
            out.push('…');
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    (!out.is_empty()).then_some(out)
}

/// First text block of a message `content` (a string or an array of typed blocks).
fn content_text(content: &Value) -> Option<&str> {
    match content {
        Value::String(s) => Some(s),
        Value::Array(blocks) => blocks.iter().find_map(|b| match b["type"].as_str() {
            Some("text" | "input_text") => b["text"].as_str(),
            _ => None,
        }),
        _ => None,
    }
}

/// Complete lines of a (possibly zstd-compressed) transcript, from byte `offset` of a plain file,
/// reading at most `limit` bytes. A last line without its newline (cut by the limit or still
/// being written) is not returned, so `consumed` always ends on a line boundary.
pub(crate) struct Lines {
    reader: BufReader<std::io::Take<Box<dyn Read>>>,
    buf: Vec<u8>,
    /// Bytes of the complete lines returned so far.
    pub(crate) consumed: u64,
}

pub(crate) fn open_lines(path: &Path, offset: u64, limit: u64) -> Option<Lines> {
    let mut file = std::fs::File::open(path).ok()?;
    let inner: Box<dyn Read> = if is_zst(path) {
        Box::new(zstd::stream::read::Decoder::new(file).ok()?)
    } else {
        file.seek(SeekFrom::Start(offset)).ok()?;
        Box::new(file)
    };
    Some(Lines { reader: BufReader::with_capacity(256 * 1024, inner.take(limit)), buf: Vec::new(), consumed: 0 })
}

impl Iterator for Lines {
    type Item = String;
    fn next(&mut self) -> Option<String> {
        self.buf.clear();
        let n = self.reader.read_until(b'\n', &mut self.buf).ok()?;
        if n == 0 || self.buf.last() != Some(&b'\n') {
            return None;
        }
        self.consumed += n as u64;
        Some(String::from_utf8_lossy(&self.buf).into_owned())
    }
}

/// User lines Claude records that the user did not type as a prompt: slash commands and their
/// output, interruption markers.
fn is_claude_command(text: &str) -> bool {
    text.starts_with("<command-") || text.starts_with("<local-command") || text.starts_with("Caveat:") || text.starts_with("[Request interrupted")
}

/// Ids are passed to the agent's `--resume`; anything else (a leading `-`, spaces, quotes) is
/// not a real conversation id.
pub(crate) fn valid_id(id: &str) -> bool {
    static ID: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:-]*$").unwrap());
    ID.is_match(id)
}

/// Folder for matching sidebar projects: on Windows, long names instead of 8.3 short names
/// (`PROGRA~1`) and no `\\?\` prefix. A folder that no longer exists keeps its recorded form.
pub(crate) fn canonical_folder(cwd: &str) -> String {
    #[cfg(windows)]
    if !cwd.is_empty() {
        if let Ok(p) = std::fs::canonicalize(cwd) {
            let s = p.to_string_lossy();
            if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
                return format!(r"\\{rest}");
            }
            return s.strip_prefix(r"\\?\").unwrap_or(&s).to_string();
        }
    }
    cwd.to_string()
}

/// Whether a transcript line is one user prompt (Claude / OMP) or turn (Codex).
fn is_turn(agent: Agent, line: &str) -> bool {
    match agent {
        Agent::Codex => line.contains("\"type\":\"task_started\""),
        Agent::Claude => {
            // Tool results are user lines too, and often huge: reject them before parsing.
            // Compaction summaries and sub-agent (sidechain) lines are not the user's prompts.
            if !line.contains("\"type\":\"user\"")
                || line.contains("\"tool_result\"")
                || line.contains("\"isMeta\":true")
                || line.contains("\"isCompactSummary\":true")
                || line.contains("\"isSidechain\":true")
            {
                return false;
            }
            serde_json::from_str::<Value>(line).is_ok_and(|v| content_text(&v["message"]["content"]).is_some_and(|t| !is_claude_command(t)))
        }
        Agent::Omp => line.contains("\"role\":\"user\"") && serde_json::from_str::<Value>(line).is_ok_and(|v| v["message"]["role"] == "user"),
    }
}

fn claude_title(line: &str) -> Option<String> {
    if !line.contains("\"type\":\"ai-title\"") && !line.contains("\"type\":\"summary\"") {
        return None;
    }
    let v = serde_json::from_str::<Value>(line).ok()?;
    v["aiTitle"].as_str().or(v["summary"].as_str()).and_then(clean_text)
}

fn parse_claude(path: &Path, len: u64) -> Option<HistoryItem> {
    let (mut cwd, mut title, mut prompt) = (String::new(), None, None);
    for line in open_lines(path, 0, HEAD_BYTES)? {
        if let Some(t) = claude_title(&line) {
            title = Some(t);
        } else if prompt.is_none() && is_turn(Agent::Claude, &line) {
            let v: Value = serde_json::from_str(&line).ok()?;
            prompt = content_text(&v["message"]["content"]).and_then(clean_text);
        }
        if cwd.is_empty() && line.contains("\"cwd\":") {
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                cwd = v["cwd"].as_str().unwrap_or_default().to_string();
            }
        }
    }
    if cwd.is_empty() {
        cwd = cwd_from_prefix(path).unwrap_or_default();
    }
    if len > HEAD_BYTES {
        // The tail starts mid-line: skip that fragment.
        let start = len.saturating_sub(TAIL_BYTES).max(HEAD_BYTES);
        if let Some(t) = open_lines(path, start, u64::MAX)?.skip(1).filter_map(|l| claude_title(&l)).last() {
            title = Some(t);
        }
    }
    let id = path.file_stem()?.to_string_lossy().into_owned();
    (prompt.is_some() || title.is_some()).then_some(HistoryItem { agent: "claude".into(), id, cwd, folder: String::new(), title, first_prompt: prompt, updated_at: 0, turns: None, home: None })
}

/// First text block of a Codex user message that the user typed: Codex and IDEs inject
/// context blocks (AGENTS.md, environment, open file...) as user messages too. Slash-command
/// prompts (`<command-message>`) are kept, tags stripped.
fn codex_prompt(content: &Value) -> Option<String> {
    const INJECTED: [&str; 9] = [
        "# AGENTS.md",
        "<environment_context",
        "<recommended_plugins",
        "<ide_opened_file",
        "<ide_selection",
        "<task-notification",
        "<local-command-stdout",
        "<user_instructions",
        "<permissions",
    ];
    content.as_array()?.iter().find_map(|b| {
        let text = b["text"].as_str().filter(|_| b["type"] == "input_text" || b["type"] == "text")?;
        if INJECTED.iter().any(|p| text.starts_with(p)) { None } else { clean_text(text) }
    })
}

/// `cwd` from the raw start of a transcript whose first line is too long to read whole (e.g.
/// a pasted image): it precedes the message in Claude's line layout.
fn cwd_from_prefix(path: &Path) -> Option<String> {
    static CWD: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r#""cwd":"((?:[^"\\]|\\.)*)""#).unwrap());
    let mut buf = vec![0; 64 * 1024];
    let n = std::fs::File::open(path).ok()?.read(&mut buf).ok()?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let raw = CWD.captures(&head)?.get(1)?.as_str();
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()
}

fn parse_codex(path: &Path) -> Option<HistoryItem> {
    let (mut id, mut cwd, mut prompt) = (String::new(), String::new(), None);
    for line in open_lines(path, 0, HEAD_BYTES)? {
        let wanted = (id.is_empty() && line.contains("\"session_meta\""))
            || (prompt.is_none() && (line.contains("\"user_message\"") || line.contains("\"role\":\"user\"")));
        if !wanted {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let p = &v["payload"];
        match (v["type"].as_str(), p["type"].as_str()) {
            (Some("session_meta"), _) => {
                id = p["id"].as_str().unwrap_or_default().to_string();
                cwd = p["cwd"].as_str().unwrap_or_default().to_string();
            }
            (Some("event_msg"), Some("user_message")) => prompt = p["message"].as_str().and_then(clean_text),
            (Some("response_item"), Some("message")) if p["role"] == "user" => prompt = codex_prompt(&p["content"]),
            _ => {}
        }
        if !id.is_empty() && prompt.is_some() {
            break;
        }
    }
    (!id.is_empty()).then_some(HistoryItem { agent: "codex".into(), id, cwd, folder: String::new(), title: None, first_prompt: prompt, updated_at: 0, turns: None, home: None })
}

fn parse_omp(path: &Path) -> Option<HistoryItem> {
    let (mut id, mut cwd, mut title, mut prompt) = (String::new(), String::new(), None, None);
    for line in open_lines(path, 0, HEAD_BYTES)? {
        let head = line.get(..40).unwrap_or(&line);
        if head.contains("\"type\":\"title") {
            // The header title line is rewritten in place on every rename.
            if let Some(t) = serde_json::from_str::<Value>(&line).ok().and_then(|v| v["title"].as_str().and_then(clean_text)) {
                title = Some(t);
            }
        } else if head.contains("\"type\":\"session\"") {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            id = v["id"].as_str().unwrap_or_default().to_string();
            cwd = v["cwd"].as_str().unwrap_or_default().to_string();
        } else if prompt.is_none() && is_turn(Agent::Omp, &line) {
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            prompt = content_text(&v["message"]["content"]).and_then(clean_text);
            break;
        }
    }
    (!id.is_empty()).then_some(HistoryItem { agent: "omp".into(), id, cwd, folder: String::new(), title, first_prompt: prompt, updated_at: 0, turns: None, home: None })
}

/// Codex keeps thread names in a separate append-only index (later lines win).
fn codex_titles(homes: &AccountHomes) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for root in std::iter::once(codex_dir()).chain(homes.codex.iter().map(PathBuf::from)) {
        let Ok(text) = std::fs::read_to_string(root.join("session_index.jsonl")) else { continue };
        for line in text.lines() {
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            if let (Some(id), Some(name)) = (v["id"].as_str(), v["thread_name"].as_str().and_then(clean_text)) {
                out.insert(id.to_string(), name);
            }
        }
    }
    out
}

/// Whether byte `offset - 1` is a newline: a file rewritten (not appended) since the last count
/// would otherwise be counted from the middle of a line.
pub(crate) fn line_ends_at(path: &Path, offset: u64) -> bool {
    if offset == 0 {
        return true;
    }
    let mut byte = [0u8];
    std::fs::File::open(path)
        .and_then(|mut f| {
            f.seek(SeekFrom::Start(offset - 1))?;
            f.read_exact(&mut byte)
        })
        .is_ok_and(|_| byte[0] == b'\n')
}

/// Brings a prompt count up to date: unchanged files keep theirs, grown plain files continue
/// from the last counted line, anything else is recounted.
fn count_turns(agent: Agent, path: &Path, prev: Option<TurnCount>, mtime_ms: u64, len: u64) -> Option<TurnCount> {
    let (offset, base) = match prev {
        Some(p) if p.mtime_ms == mtime_ms && p.len == len => return Some(p),
        Some(p) if !is_zst(path) && len >= p.offset && line_ends_at(path, p.offset) => (p.offset, p.turns),
        _ => (0, 0),
    };
    let mut lines = open_lines(path, offset, u64::MAX)?;
    let turns = base + lines.by_ref().filter(|l| is_turn(agent, l)).count() as u32;
    Some(TurnCount { mtime_ms, len, offset: offset + lines.consumed, turns })
}

/// Counts prompts of `files` on a background thread (smallest first, so most rows fill in
/// quickly), then persists the counts and tells the UI.
fn spawn_counting(app: AppHandle, mut files: Vec<StaleFile>) {
    if files.is_empty() || COUNTING.swap(true, Ordering::AcqRel) {
        return;
    }
    files.sort_by_key(|f| f.3);
    std::thread::spawn(move || {
        /// Clears `COUNTING` however the pass ends (a panicking worker included).
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                COUNTING.store(false, Ordering::Release);
            }
        }
        let _done = Done;
        let next = AtomicUsize::new(0);
        // A first pass over gigabytes takes a while: keep progress if the app closes midway.
        let last_save = Mutex::new(std::time::Instant::now());
        // Two workers: this is background work and must not compete with the terminals.
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(|| {
                    while let Some((agent, path, mtime_ms, len)) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let prev = CACHE.lock().turns.get(path).copied();
                        if let Some(count) = count_turns(*agent, path, prev, *mtime_ms, *len) {
                            CACHE.lock().turns.insert(path.clone(), count);
                        }
                        let due = {
                            let mut t = last_save.lock();
                            let due = t.elapsed() > std::time::Duration::from_secs(10);
                            if due {
                                *t = std::time::Instant::now();
                            }
                            due
                        };
                        if due {
                            save_cache();
                        }
                    }
                });
            }
        });
        save_cache();
        let _ = app.emit("agent-history-counted", ());
    });
}

/// A transcript whose prompt count is out of date: agent, path, modified (ms), size.
type StaleFile = (Agent, PathBuf, u64, u64);

/// The list (newest first) and the files whose prompt counts are out of date.
fn scan_history(homes: &AccountHomes) -> (Vec<HistoryItem>, Vec<StaleFile>) {
    let mut account_of: HashMap<PathBuf, String> = HashMap::new();
    let files: Vec<(Agent, PathBuf, SystemTime, u64)> = transcript_files(homes)
        .into_iter()
        .filter_map(|(a, p, home)| {
            let m = std::fs::metadata(&p).ok()?;
            if let Some(h) = home {
                account_of.insert(p.clone(), h);
            }
            Some((a, p, m.modified().ok()?, m.len()))
        })
        .collect();
    let stale: Vec<usize> = {
        let cache = CACHE.lock();
        (0..files.len())
            .filter(|&i| !cache.meta.get(&files[i].1).is_some_and(|c| c.mtime_ms == millis(files[i].2) && c.len == files[i].3))
            .collect()
    };
    let next = AtomicUsize::new(0);
    // Mostly file opening and decompression; more threads would only compete with terminals.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(4).min(stale.len().max(1));
    let parsed: Vec<(usize, Option<HistoryItem>)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut out = Vec::new();
                    while let Some(&i) = stale.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let (agent, path, _, len) = &files[i];
                        let item = match agent {
                            Agent::Claude => parse_claude(path, *len),
                            Agent::Codex => parse_codex(path),
                            Agent::Omp => parse_omp(path),
                        }
                        .filter(|it| valid_id(&it.id))
                        .map(|mut it| {
                            it.folder = canonical_folder(&it.cwd);
                            it
                        });
                        out.push((i, item));
                    }
                    out
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });

    let live: std::collections::HashSet<&PathBuf> = files.iter().map(|f| &f.1).collect();
    let mut cache = CACHE.lock();
    let before = (cache.meta.len(), cache.turns.len());
    let changed = !parsed.is_empty();
    let Cache { meta, turns, .. } = &mut *cache;
    for (i, item) in parsed {
        let (_, path, mtime, len) = &files[i];
        meta.insert(path.clone(), Meta { mtime_ms: millis(*mtime), len: *len, item });
    }
    meta.retain(|p, _| live.contains(p));
    turns.retain(|p, _| live.contains(p));
    let changed = changed || before != (meta.len(), turns.len());

    let titles = codex_titles(homes);
    let mut to_count = Vec::new();
    let mut items: Vec<HistoryItem> = Vec::new();
    for (agent, path, mtime, len) in &files {
        let Some(mut item) = meta.get(path).and_then(|m| m.item.clone()) else { continue };
        item.updated_at = millis(*mtime);
        // A count a little behind a growing transcript is shown until the recount lands.
        let count = turns.get(path);
        item.turns = count.map(|c| c.turns);
        if !count.is_some_and(|c| c.mtime_ms == item.updated_at && c.len == *len) {
            to_count.push((*agent, path.clone(), item.updated_at, *len));
        }
        if item.agent == "codex" {
            item.title = titles.get(&item.id).cloned();
        }
        item.home = account_of.get(path).cloned();
        items.push(item);
    }
    drop(cache);
    if changed {
        save_cache();
    }
    items.sort_by_key(|i| std::cmp::Reverse(i.updated_at));
    items.truncate(MAX_ITEMS);
    (items, to_count)
}

/// Past conversations of every agent, newest first. Scoping (folder / project / all) and search
/// happen in the UI. With `count`, out-of-date prompt counts are refreshed in the background
/// and `agent-history-counted` is emitted when they are ready (the UI then re-lists without
/// `count`, so an agent writing its transcript cannot cause a loop).
#[tauri::command]
pub async fn agent_history_list(app: AppHandle, count: Option<bool>, homes: Option<AccountHomes>) -> CmdResult<Vec<HistoryItem>> {
    let homes = homes.unwrap_or_default();
    let (items, to_count) = tauri::async_runtime::spawn_blocking(move || {
        let _one_at_a_time = SCANNING.lock();
        scan_history(&homes)
    })
    .await
    .map_err(err)?;
    if count.unwrap_or(true) {
        spawn_counting(app, to_count);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_text_strips_markup_and_caps() {
        assert_eq!(clean_text("<attachment>\n# TASK:  fix   it</attachment>").as_deref(), Some("# TASK: fix it"));
        assert_eq!(clean_text("<x/>  "), None);
        assert!(clean_text(&"word ".repeat(200)).unwrap().ends_with('…'));
    }

    /// `cargo test -p kelpie --lib --release -- --ignored --nocapture scan_real` — list scan
    /// (cold / warm) and full prompt count over this machine's real transcripts.
    #[test]
    #[ignore]
    fn scan_real_transcripts() {
        let t = std::time::Instant::now();
        let (cold, to_count) = scan_history(&AccountHomes::default());
        let cold_ms = t.elapsed().as_millis();
        let t = std::time::Instant::now();
        let (warm, _) = scan_history(&AccountHomes::default());
        let warm_ms = t.elapsed().as_millis();
        for agent in [Agent::Claude, Agent::Codex, Agent::Omp] {
            let t = std::time::Instant::now();
            let files: Vec<_> = transcript_files(&AccountHomes::default()).into_iter().filter(|f| f.0 == agent).map(|f| (f.0, f.1)).collect();
            let found = files.iter().filter(|(_, p)| match agent {
                Agent::Claude => parse_claude(p, std::fs::metadata(p).map_or(0, |m| m.len())),
                Agent::Codex => parse_codex(p),
                Agent::Omp => parse_omp(p),
            }.is_some_and(|i| i.first_prompt.is_some())).count();
            println!("{agent:?}: {} files, {found} with a prompt, {}ms single-threaded", files.len(), t.elapsed().as_millis());
        }
        let n = |a: &str| cold.iter().filter(|i| i.agent == a).count();
        println!("items={} claude={} codex={} omp={} cold={cold_ms}ms warm={warm_ms}ms", cold.len(), n("claude"), n("codex"), n("omp"));
        assert_eq!(cold, warm);
        let t = std::time::Instant::now();
        let turns: u32 = to_count.iter().filter_map(|(a, p, m, l)| count_turns(*a, p, None, *m, *l)).map(|c| c.turns).sum();
        println!("counted {} files, {turns} prompts in {}ms (background)", to_count.len(), t.elapsed().as_millis());
    }

    fn write_lines(path: &Path, lines: &[&str]) {
        std::fs::write(path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }

    #[test]
    fn parses_each_agent_format() {
        let dir = std::env::temp_dir().join(format!("kelpie-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let claude = dir.join("abc.jsonl");
        let claude_lines = [
            r#"{"type":"user","cwd":"D:\\p","isMeta":true,"message":{"role":"user","content":"meta"}}"#,
            r#"{"type":"user","cwd":"D:\\p","message":{"role":"user","content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","cwd":"D:\\p","message":{"role":"user","content":[{"type":"text","text":"Fix the build"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"x"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"and test it"}}"#,
            r#"{"type":"ai-title","aiTitle":"Build fix","sessionId":"abc"}"#,
        ];
        write_lines(&claude, &claude_lines);
        let c = parse_claude(&claude, 100).unwrap();
        assert_eq!((c.id.as_str(), c.cwd.as_str()), ("abc", "D:\\p"));
        assert_eq!((c.title.as_deref(), c.first_prompt.as_deref()), (Some("Build fix"), Some("Fix the build")));
        assert_eq!(claude_lines.iter().filter(|l| is_turn(Agent::Claude, l)).count(), 2);

        let codex = dir.join("rollout.jsonl");
        let codex_lines = [
            r#"{"type":"session_meta","payload":{"id":"t1","cwd":"/w"}}"#,
            r#"{"type":"event_msg","payload":{"type":"task_started"}}"#,
            r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions"}]}}"##,
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Say hi"}]}}"#,
        ];
        write_lines(&codex, &codex_lines);
        let x = parse_codex(&codex).unwrap();
        assert_eq!((x.id.as_str(), x.cwd.as_str(), x.first_prompt.as_deref()), ("t1", "/w", Some("Say hi")));
        assert_eq!(codex_lines.iter().filter(|l| is_turn(Agent::Codex, l)).count(), 1);

        let omp = dir.join("omp.jsonl");
        write_lines(
            &omp,
            &[
                r#"{"type":"title","v":1,"title":"Renamed","pad":"   "}"#,
                r#"{"type":"session","version":3,"id":"o1","cwd":"C:\\w"}"#,
                r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"<attachment>Do it</attachment>"}]}}"#,
            ],
        );
        let o = parse_omp(&omp).unwrap();
        assert_eq!((o.id.as_str(), o.title.as_deref(), o.first_prompt.as_deref()), ("o1", Some("Renamed"), Some("Do it")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rejects_non_prompts_and_bad_ids() {
        assert!(!is_turn(Agent::Claude, r#"{"type":"user","isCompactSummary":true,"message":{"content":"This session is being continued"}}"#));
        assert!(!is_turn(Agent::Claude, r#"{"type":"user","isSidechain":true,"message":{"content":"sub-agent task"}}"#));
        assert!(!is_turn(Agent::Claude, r#"{"type":"user","message":{"content":"[Request interrupted by user]"}}"#));
        assert!(valid_id("019fe5ee-4706-7a10-8578-a98f95104d9f"));
        assert!(!valid_id("-rf"));
        assert!(!valid_id("a b"));
    }

    #[test]
    fn rewritten_files_are_recounted() {
        let path = std::env::temp_dir().join(format!("kelpie-rewrite-{}.jsonl", std::process::id()));
        let turn = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        std::fs::write(&path, format!("{turn}\n{turn}\n")).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let first = count_turns(Agent::Codex, &path, None, 1, len).unwrap();
        // Rewritten with different content, longer: the old offset is mid-line now.
        std::fs::write(&path, format!("{{\"x\":1}}\n{turn}\n{turn}\n")).unwrap();
        assert!(!line_ends_at(&path, first.offset));
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(count_turns(Agent::Codex, &path, Some(first), 2, len).unwrap().turns, 2);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn counts_grow_incrementally_and_skip_partial_lines() {
        let path = std::env::temp_dir().join(format!("kelpie-turns-{}.jsonl", std::process::id()));
        let turn = r#"{"type":"event_msg","payload":{"type":"task_started"}}"#;
        std::fs::write(&path, format!("{turn}\n{turn}\n{turn}")).unwrap(); // last line still being written
        let len = std::fs::metadata(&path).unwrap().len();
        let first = count_turns(Agent::Codex, &path, None, 1, len).unwrap();
        assert_eq!(first.turns, 2);
        assert_eq!(count_turns(Agent::Codex, &path, Some(first), 1, len), Some(first));
        std::fs::write(&path, format!("{turn}\n{turn}\n{turn}\n{turn}\n")).unwrap();
        let len = std::fs::metadata(&path).unwrap().len();
        let grown = count_turns(Agent::Codex, &path, Some(first), 2, len).unwrap();
        assert_eq!((grown.turns, grown.offset), (4, len));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn escapes_like_claude() {
        let p = claude_project_dir(&claude_dir(), "C:\\Windows\\System32");
        assert!(p.ends_with("C--Windows-System32"));
    }
}
