//! Agents using the browser panes the person sees (`browser.call {tool, args}`): which tools there are and what each may do, the
//! user's policy (`browser_agents`, `browser_eval` and `browser_sites` in `policy.json`), the arguments each tool takes, a rate per
//! session, the audit of what agents did to pages, and the request to the app that does it (`browser.agent` to the `browser` provider).
//!
//! The daemon decides first: a tool the policy does not allow, too many calls or a bad argument never reach the window. The window
//! then decides which pane, and whether this session may use it (a pane opened for it, or one the person shared).

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Map, Value};

use crate::provider::Providers;
use crate::subs::cut;

/// The tools, by the short names `browser.call` takes (`kelpie_browser_<name>` for an agent).
pub const TOOLS: [&str; 11] = ["tabs", "snapshot", "click", "type", "press", "navigate", "wait", "screenshot", "console", "network", "eval"];

/// What a tool does to a page: looks at it, acts on it, or runs the agent's own script in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Read,
    Act,
    Eval,
}

pub fn classify(tool: &str) -> Option<Class> {
    Some(match tool {
        "tabs" | "snapshot" | "wait" | "screenshot" | "console" | "network" => Class::Read,
        "click" | "type" | "press" | "navigate" => Class::Act,
        "eval" => Class::Eval,
        _ => return None,
    })
}

// ---------- policy ----------

/// How far agents may use browser panes: not at all, look only, or look and act.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    Read,
    Act,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "off" => Some(Mode::Off),
            "read" => Some(Mode::Read),
            "act" => Some(Mode::Act),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Read => "read",
            Mode::Act => "act",
        }
    }
}

/// The user's choices. `sites` are the origins agents may also use besides the local computer, for panes opened for agents whose page is not private.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    pub mode: Mode,
    pub eval: bool,
    pub sites: Vec<String>,
}

/// The most sites the list holds.
pub const MAX_SITES: usize = 50;

/// A site as `scheme://host[:port]` (the default port left out), http or https only, with no user, path, query or fragment.
pub fn site(given: &str) -> Result<String, String> {
    let given = given.trim();
    let shown = cut(given, 120);
    let not_a_site = || format!("`{shown}` is not a site: give scheme://host[:port], e.g. http://localhost:3000, without a path");
    let url = url::Url::parse(given).map_err(|_| not_a_site())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("`{shown}`: only http:// and https:// sites"));
    }
    if !url.username().is_empty() || url.password().is_some() || url.host_str().is_none_or(str::is_empty) || !matches!(url.path(), "" | "/") || url.query().is_some() || url.fragment().is_some() {
        return Err(not_a_site());
    }
    Ok(url.origin().ascii_serialization())
}

impl Policy {
    /// From `policy.json`: a key that is missing or not valid is the default, and so is a site list that is not valid as a whole.
    pub fn from_file(file: Option<&Value>) -> Policy {
        let get = |key: &str| file.and_then(|v| v.get(key));
        Policy {
            mode: get("browser_agents").and_then(Value::as_str).and_then(Mode::parse).unwrap_or_default(),
            eval: get("browser_eval").and_then(Value::as_bool).unwrap_or(false),
            sites: get("browser_sites").and_then(|v| sites(v).ok()).unwrap_or_default(),
        }
    }

    /// The policy `daemon.set_policy` asks for on top of `now`, checked as a whole; `None` when it names none of the keys.
    pub fn requested(params: &Value, now: &Policy) -> Result<Option<Policy>> {
        let given = |key: &str| params.get(key).filter(|v| !v.is_null());
        let mode = given("browser_agents").map(|v| v.as_str().and_then(Mode::parse).ok_or_else(|| anyhow!("browser_agents must be off, read or act"))).transpose()?;
        let eval = given("browser_eval").map(|v| v.as_bool().ok_or_else(|| anyhow!("browser_eval must be true or false"))).transpose()?;
        let list = given("browser_sites").map(|v| sites(v).map_err(|e| anyhow!(e))).transpose()?;
        if mode.is_none() && eval.is_none() && list.is_none() {
            return Ok(None);
        }
        Ok(Some(Policy { mode: mode.unwrap_or(now.mode), eval: eval.unwrap_or(now.eval), sites: list.unwrap_or_else(|| now.sites.clone()) }))
    }

    /// Writes the keys into `policy.json`'s object, leaving the others.
    pub fn write_into(&self, file: &mut Value) {
        file["browser_agents"] = json!(self.mode.as_str());
        file["browser_eval"] = json!(self.eval);
        file["browser_sites"] = json!(self.sites);
    }

    /// What the app is told with each request.
    pub fn for_app(&self) -> Value {
        json!({"mode": self.mode.as_str(), "eval": self.eval, "sites": self.sites})
    }
}

/// A list of sites, each normalised, duplicates dropped, at most [`MAX_SITES`].
fn sites(v: &Value) -> Result<Vec<String>, String> {
    let items = v.as_array().ok_or("browser_sites must be a list of sites")?;
    let mut out: Vec<String> = Vec::new();
    for item in items {
        let s = site(item.as_str().ok_or("browser_sites must be a list of sites")?)?;
        if !out.contains(&s) {
            out.push(s);
        }
    }
    if out.len() > MAX_SITES {
        return Err(format!("browser_sites holds at most {MAX_SITES} sites"));
    }
    Ok(out)
}

// ---------- the gate ----------

pub const OFF: &str = "Browser control is off. Ask the user to turn it on in Kelpie → Settings → Agents → Browser panes.";
pub const EVAL_OFF: &str = "Running JavaScript in a page (kelpie_browser_eval) is off. Ask the user to turn it on in Kelpie → Settings → Agents → Browser panes.";
pub const TOO_MANY: &str = "Too many browser calls; slow down.";
pub const NOT_OPEN: &str = "Kelpie's window is not open: browser panes exist only while the Kelpie app runs.";
pub const OLD_APP: &str = "This Kelpie app is older than its daemon: update or restart Kelpie.";

pub fn unknown_tool(tool: &str) -> String {
    format!("unknown browser tool `{}`: one of {}", cut(tool, 40), TOOLS.join(", "))
}

/// Whether the policy lets `tool` run at all: its class, or why not.
pub fn gate(policy: &Policy, tool: &str) -> Result<Class, String> {
    let class = classify(tool).ok_or_else(|| unknown_tool(tool))?;
    match (policy.mode, class) {
        (Mode::Off, _) => Err(OFF.into()),
        (Mode::Read, Class::Act | Class::Eval) => Err(format!(
            "Browser control is set to read only, so kelpie_browser_{tool}, which acts on the page, is refused. Ask the user to allow reading and acting in Kelpie → Settings → Agents → Browser panes."
        )),
        (Mode::Act, Class::Eval) if !policy.eval => Err(EVAL_OFF.into()),
        _ => Ok(class),
    }
}

// ---------- arguments ----------

/// The most text a snapshot may ask for: what an MCP result carries (20 000 characters) less room for the page's header.
pub const MAX_SNAPSHOT_CHARS: u64 = 19_000;
const MAX_TEXT: usize = 10_000;
const MAX_FIND: usize = 1_000;
const MAX_EXPRESSION: usize = 4_000;
const KEYS: &[&str] = &["Enter", "Tab", "Escape", "Backspace", "Delete", "ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight", "Home", "End", "PageUp", "PageDown", "Space"];
const MODIFIERS: &[&str] = &["Control", "Shift", "Alt", "Meta"];
const DIALOG: &[&str] = &["accept", "dismiss"];
const AFTER: &[&str] = &["none", "diff", "full"];

/// The arguments each tool takes besides `pane`.
fn keys_of(tool: &str) -> &'static [&'static str] {
    match tool {
        "snapshot" => &["mode", "root", "max_chars"],
        "click" => &["ref", "button", "double", "dialog", "snapshot_after"],
        "type" => &["ref", "text", "clear", "submit", "dialog", "snapshot_after"],
        "press" => &["key", "modifiers", "ref", "dialog", "snapshot_after"],
        "navigate" => &["url", "action", "timeout_s"],
        "wait" => &["text", "text_gone", "selector", "url_contains", "load", "timeout_s"],
        "screenshot" => &["ref", "full_page"],
        "console" => &["level", "since", "max"],
        "network" => &["filter", "failed_only", "since", "max"],
        "eval" => &["expression"],
        _ => &[],
    }
}

/// The arguments as given, read one by one. Values are never repeated in an error: a typed text may be a secret.
struct Given<'a>(&'a Map<String, Value>);

impl Given<'_> {
    fn text(&self, key: &str, max: usize) -> Result<Option<String>, String> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
            Some(Value::String(s)) if s.chars().count() > max => Err(format!("`{key}` is longer than {max} characters")),
            Some(Value::String(s)) => Ok(Some(s.trim().to_string())),
            Some(_) => Err(format!("`{key}` must be text")),
        }
    }

    fn boolean(&self, key: &str) -> Result<Option<bool>, String> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::Bool(b)) => Ok(Some(*b)),
            Some(_) => Err(format!("`{key}` must be true or false")),
        }
    }

    fn number(&self, key: &str, min: u64, max: u64) -> Result<Option<u64>, String> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v.as_u64().filter(|n| (min..=max).contains(n)).map(Some).ok_or_else(|| format!("`{key}` must be a whole number from {min} to {max}")),
        }
    }

    fn choice(&self, key: &str, of: &[&'static str]) -> Result<Option<&'static str>, String> {
        match self.0.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => of.iter().copied().find(|c| *c == s).map(Some).ok_or_else(|| format!("`{key}` is one of {}", of.join(", "))),
            Some(_) => Err(format!("`{key}` is one of {}", of.join(", "))),
        }
    }

    /// An element's ref from a snapshot: `e` and a number.
    fn reference(&self, key: &str) -> Result<Option<String>, String> {
        let bad = || format!("`{key}` is a ref from kelpie_browser_snapshot, like e12");
        match self.text(key, 12).map_err(|_| bad())? {
            None => Ok(None),
            Some(r) if r.len() > 1 && r.starts_with('e') && r[1..].bytes().all(|b| b.is_ascii_digit()) => Ok(Some(r)),
            Some(_) => Err(bad()),
        }
    }

    fn pane(&self) -> Result<Option<String>, String> {
        let bad = || "`pane` is an id from kelpie_browser_tabs".to_string();
        match self.0.get("pane") {
            Some(Value::Number(n)) if n.is_u64() => Ok(Some(n.to_string())),
            _ => match self.text("pane", 100).map_err(|_| bad())? {
                Some(p) if p.chars().any(char::is_control) => Err(bad()),
                p => Ok(p),
            },
        }
    }
}

fn required<T>(v: Option<T>, key: &str) -> Result<T, String> {
    v.ok_or_else(|| format!("missing `{key}`"))
}

/// A key `press` takes: a named one, F1 to F12, or a single printed character.
fn key_ok(key: &str) -> bool {
    let mut chars = key.chars();
    KEYS.contains(&key) || (1..=12).any(|n| key == format!("F{n}")) || matches!((chars.next(), chars.next()), (Some(c), None) if !c.is_control() && !c.is_whitespace())
}

/// `args` for `tool` checked against what the tool takes, with every default filled in: what the app is sent. Unknown arguments are refused.
pub fn check_args(tool: &str, args: &Value) -> Result<Value, String> {
    let empty = Map::new();
    let given = match args {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err("`args` must be an object".into()),
    };
    let takes = keys_of(tool);
    if let Some(k) = given.keys().find(|k| k.as_str() != "pane" && !takes.contains(&k.as_str())) {
        let list = std::iter::once("pane").chain(takes.iter().copied()).collect::<Vec<_>>().join(", ");
        return Err(format!("kelpie_browser_{tool} takes no `{}`; it takes {list}", cut(k, 40)));
    }
    let g = Given(given);
    let mut out = Map::new();
    if let Some(pane) = g.pane()? {
        out.insert("pane".into(), json!(pane));
    }
    let dialog_and_after = |out: &mut Map<String, Value>| -> Result<(), String> {
        out.insert("dialog".into(), json!(g.choice("dialog", DIALOG)?.unwrap_or("dismiss")));
        out.insert("snapshot_after".into(), json!(g.choice("snapshot_after", AFTER)?.unwrap_or("diff")));
        Ok(())
    };
    match tool {
        "tabs" => {}
        "snapshot" => {
            let mode = g.choice("mode", &["compact", "full"])?.unwrap_or("compact");
            out.insert("mode".into(), json!(mode));
            if let Some(root) = g.reference("root")? {
                out.insert("root".into(), json!(root));
            }
            out.insert("max_chars".into(), json!(g.number("max_chars", 200, MAX_SNAPSHOT_CHARS)?.unwrap_or(if mode == "full" { 18_000 } else { 8_000 })));
        }
        "click" => {
            out.insert("ref".into(), json!(required(g.reference("ref")?, "ref")?));
            out.insert("button".into(), json!(g.choice("button", &["left", "right"])?.unwrap_or("left")));
            out.insert("double".into(), json!(g.boolean("double")?.unwrap_or(false)));
            dialog_and_after(&mut out)?;
        }
        "type" => {
            out.insert("ref".into(), json!(required(g.reference("ref")?, "ref")?));
            // Typed as given: not trimmed, and empty is allowed (with `clear`, it empties the field).
            let text = match given.get("text") {
                Some(Value::String(s)) if s.chars().count() > MAX_TEXT => return Err(format!("`text` is longer than {MAX_TEXT} characters")),
                Some(Value::String(s)) => s.clone(),
                None | Some(Value::Null) => return Err("missing `text`".into()),
                Some(_) => return Err("`text` must be text".into()),
            };
            out.insert("text".into(), json!(text));
            out.insert("clear".into(), json!(g.boolean("clear")?.unwrap_or(true)));
            out.insert("submit".into(), json!(g.boolean("submit")?.unwrap_or(false)));
            dialog_and_after(&mut out)?;
        }
        "press" => {
            let key = match given.get("key") {
                Some(Value::String(k)) if key_ok(k) => k.clone(),
                None | Some(Value::Null) => return Err("missing `key`".into()),
                Some(_) => return Err(format!("`key` is one of {}, F1 to F12, or a single character", KEYS.join(", "))),
            };
            out.insert("key".into(), json!(key));
            let modifiers = match given.get("modifiers") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => {
                    let mut seen: Vec<&'static str> = Vec::new();
                    for item in items {
                        let m = item.as_str().and_then(|s| MODIFIERS.iter().copied().find(|m| *m == s)).ok_or_else(|| format!("`modifiers` is a list of {}", MODIFIERS.join(", ")))?;
                        if !seen.contains(&m) {
                            seen.push(m);
                        }
                    }
                    seen
                }
                Some(_) => return Err(format!("`modifiers` is a list of {}", MODIFIERS.join(", "))),
            };
            out.insert("modifiers".into(), json!(modifiers));
            if let Some(r) = g.reference("ref")? {
                out.insert("ref".into(), json!(r));
            }
            dialog_and_after(&mut out)?;
        }
        "navigate" => {
            let url = match g.text("url", crate::daemon::MAX_PAGE_URL + 1).map_err(|_| "`url` must be an http:// or https:// address".to_string())? {
                // The address checked as `kelpie_open_page` checks it; the reason never repeats it (its query may hold a secret).
                Some(u) => Some(crate::daemon::page_url(&u).map_err(|_| "`url` must be an http:// or https:// address with a host, without a user name or password, spaces or control characters".to_string())?),
                None => None,
            };
            let action = g.choice("action", &["back", "forward", "reload"])?;
            match (url, action) {
                (Some(url), None) => out.insert("url".into(), json!(url)),
                (None, Some(action)) => out.insert("action".into(), json!(action)),
                _ => return Err("give either `url` or `action` (back, forward or reload)".into()),
            };
            out.insert("timeout_s".into(), json!(g.number("timeout_s", 1, 25)?.unwrap_or(15)));
        }
        "wait" => {
            let mut conditions = Vec::new();
            for key in ["text", "text_gone", "selector", "url_contains"] {
                if let Some(v) = g.text(key, MAX_FIND)? {
                    conditions.push((key, json!(v)));
                }
            }
            match g.boolean("load")? {
                Some(true) => conditions.push(("load", json!(true))),
                Some(false) => return Err("`load` waits for the page to load: give true, or leave it out".into()),
                None => {}
            }
            let [(key, value)] = <[_; 1]>::try_from(conditions).map_err(|_| "give exactly one of `text`, `text_gone`, `selector`, `url_contains` or `load`".to_string())?;
            out.insert(key.into(), value);
            out.insert("timeout_s".into(), json!(g.number("timeout_s", 1, 30)?.unwrap_or(10)));
        }
        "screenshot" => {
            if let Some(r) = g.reference("ref")? {
                out.insert("ref".into(), json!(r));
            }
            out.insert("full_page".into(), json!(g.boolean("full_page")?.unwrap_or(false)));
        }
        "console" | "network" => {
            if tool == "console" {
                out.insert("level".into(), json!(g.choice("level", &["error", "warning", "all"])?.unwrap_or("all")));
            } else {
                if let Some(f) = g.text("filter", 500)? {
                    out.insert("filter".into(), json!(f));
                }
                out.insert("failed_only".into(), json!(g.boolean("failed_only")?.unwrap_or(false)));
            }
            if let Some(since) = g.number("since", 0, u64::MAX)? {
                out.insert("since".into(), json!(since));
            }
            out.insert("max".into(), json!(g.number("max", 1, 300)?.unwrap_or(50)));
        }
        "eval" => {
            out.insert("expression".into(), json!(required(g.text("expression", MAX_EXPRESSION)?, "expression")?));
        }
        other => return Err(unknown_tool(other)),
    }
    Ok(Value::Object(out))
}

/// How long the daemon waits for the window: a wait or a navigation as long as it may take and a little more, else 15 s.
pub fn limit(tool: &str, args: &Value) -> Duration {
    let timeout = args.get("timeout_s").and_then(Value::as_u64);
    Duration::from_secs(match tool {
        "wait" => timeout.unwrap_or(10).min(30) + 5,
        "navigate" => timeout.unwrap_or(15).min(25) + 5,
        _ => 15,
    })
}

// ---------- rate ----------

const RATE_PER_S: f64 = 10.0;
const BURST: f64 = 20.0;

/// A token bucket per session: 10 calls a second, 20 at once.
#[derive(Default)]
pub struct Rate {
    buckets: HashMap<String, (f64, Instant)>,
}

impl Rate {
    /// One call of session `sid` at `now`; false when it is over its rate (a refused call costs nothing).
    pub fn take(&mut self, sid: &str, now: Instant) -> bool {
        // A bucket left alone this long is full again: the same as none.
        let full_after = Duration::from_secs_f64(BURST / RATE_PER_S);
        self.buckets.retain(|_, (_, at)| now.saturating_duration_since(*at) < full_after);
        let (tokens, at) = self.buckets.entry(sid.to_string()).or_insert((BURST, now));
        let refilled = (*tokens + now.saturating_duration_since(*at).as_secs_f64() * RATE_PER_S).min(BURST);
        *at = now;
        if refilled < 1.0 {
            *tokens = refilled;
            return false;
        }
        *tokens = refilled - 1.0;
        true
    }
}

// ---------- audit ----------

pub const AUDIT_FILE: &str = "browser-audit.jsonl";
/// The file moves to `.1` (replacing the one before) once it is this big.
const ROTATE_AT: u64 = 1 << 20;

/// What agents did to pages, one JSON line each: `<data>/browser-audit.jsonl`, the one before in `.1`.
pub struct Audit {
    path: PathBuf,
    writing: Mutex<()>,
}

impl Audit {
    pub fn new(data_dir: &Path) -> Audit {
        Audit { path: data_dir.join(AUDIT_FILE), writing: Mutex::new(()) }
    }

    fn older(&self) -> PathBuf {
        self.path.with_extension("jsonl.1")
    }

    /// Appends a line (blocking: called off the event loop).
    pub fn append(&self, line: &Value) -> std::io::Result<()> {
        let _one = self.writing.lock();
        if std::fs::metadata(&self.path).is_ok_and(|m| m.len() >= ROTATE_AT) {
            // A file another program holds open cannot move: the line is still written, to the file as it is.
            if let Err(e) = std::fs::rename(&self.path, self.older()) {
                tracing::warn!("browser audit: could not move the full file aside: {e}");
            }
        }
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)?;
        writeln!(file, "{line}")
    }

    /// The last `limit` lines, newest last (blocking).
    pub fn tail(&self, limit: usize) -> Vec<Value> {
        let _one = self.writing.lock();
        let read = |p: &Path| std::fs::read_to_string(p).unwrap_or_default();
        let mut lines: Vec<Value> = read(&self.path).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
        if lines.len() < limit {
            let mut older: Vec<Value> = read(&self.older()).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
            older.append(&mut lines);
            lines = older;
        }
        lines.split_off(lines.len().saturating_sub(limit))
    }
}

/// A host as the audit keeps it, from a host or a whole address: no scheme, user, port, path, query or fragment.
fn host_only(given: &str) -> Option<String> {
    let given = given.trim();
    let url = if given.contains("://") { url::Url::parse(given) } else { url::Url::parse(&format!("http://{given}")) };
    url.ok()?.host_str().filter(|h| !h.is_empty()).map(|h| cut(h, 255))
}

/// Text the app or a page wrote, made safe to keep or to show: one line, secrets redacted, every web address without its query and
/// fragment (where tokens and codes travel), at most `max` characters.
pub fn scrub(text: &str, max: usize) -> String {
    let line = kelpie_proto::redact(&text.replace(['\n', '\r'], " "));
    let words: Vec<String> = line
        .split(' ')
        .map(|word| match word.find("://").and_then(|at| word[at..].find(['?', '#']).map(|end| at + end)) {
            Some(end) => word[..end].to_string(),
            None => word.to_string(),
        })
        .collect();
    cut(&words.join(" "), max)
}

/// One audit line. Only these fields, whatever the call carried: never typed text, an address's path or query, or a script (`args`
/// is read for a navigation's host alone). `target` is the element the app says it acted on (role and name), `error` a refusal or
/// failure the caller already chose to keep.
pub fn audit_line(at: u64, session: &str, label: &str, tool: &str, args: &Value, reply: Option<&Value>, error: Option<&str>) -> Value {
    let audit = reply.and_then(|r| r.get("audit"));
    let navigated = || (tool == "navigate").then(|| args.get("url").and_then(Value::as_str).and_then(|u| url::Url::parse(u).ok()).and_then(|u| u.host_str().map(str::to_string))).flatten();
    let host = audit.and_then(|a| a.get("host")).and_then(Value::as_str).and_then(host_only).or_else(navigated);
    let target = audit.and_then(|a| a.get("target")).and_then(Value::as_str).map(|t| scrub(t, 60));
    json!({
        "at": at, "session": session, "label": cut(label, 80), "tool": tool, "host": host, "target": target,
        "ok": error.is_none(), "error": error.map(|e| scrub(e, 120)),
    })
}

// ---------- the request to the app ----------

/// What the daemon keeps for browser control.
pub struct State {
    pub policy: RwLock<Policy>,
    pub rate: Mutex<Rate>,
    pub audit: Arc<Audit>,
}

impl State {
    pub fn new(policy: Policy, data_dir: &Path) -> State {
        State { policy: RwLock::new(policy), rate: Mutex::new(Rate::default()), audit: Arc::new(Audit::new(data_dir)) }
    }
}

/// The `browser.agent` request: who asks (`session` as `{id, name, kind, cwd, parent}`), what, and the policy the window applies.
pub fn request(session: Value, tool: &str, args: &Value, policy: &Policy) -> Value {
    json!({"session": session, "tool": tool, "args": args, "policy": policy.for_app()})
}

/// The window's error as the agent is told it: the channel's own failures (`provider.rs`) in words of what to do, the window's own
/// refusals scrubbed like everything else that may carry a page's text.
pub fn app_error(error: &str, limit: Duration) -> String {
    match error {
        "the Kelpie app is not open on the computer" | "the Kelpie app went away" => NOT_OPEN.into(),
        "the Kelpie app did not answer in time" => {
            format!("Kelpie's window did not answer within {} s; the page may be busy. Try again, or take a snapshot to see where it is.", limit.as_secs())
        }
        e if e.starts_with("unknown request") => OLD_APP.into(),
        e => scrub(e, 300),
    }
}

/// Asks the app and waits at most `limit`. Its answer is an object (`{text, data?, page?, image?, audit?}`), or the error to show.
pub async fn forward(providers: &Providers, params: Value, limit: Duration) -> Result<Value, String> {
    match providers.request_within("browser", "browser.agent", params, limit).await {
        Ok(v) if v.is_object() => Ok(v),
        Ok(_) => Err(OLD_APP.into()),
        Err(e) => Err(app_error(&format!("{e:#}"), limit)),
    }
}

#[cfg(test)]
mod tests;
