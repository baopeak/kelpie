//! Agents using the windows of programs they started (`window.call {tool, args}`): which tools there are and what each may do, the
//! user's policy (`window_agents` and `window_visible` in `policy.json`), the arguments each tool takes, a rate per session, the audit
//! of what agents did, and the carrying out: the daemon runs the `kelpie-window` helper, one process per call, never through a shell.
//!
//! The helper does the work (UI Automation, the window picture); this module decides first: a tool the policy does not allow, too
//! many calls, or an argument that is not of the tool's shape never reach it. `KELPIE_WINDOW_SCOPE` names the calling session, so that
//! a session reaches only the programs it started. That is a rule for a well-behaved agent, not a boundary against a program that has a
//! shell: such a program can start the helper itself with any scope.

use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use base64::Engine;
use parking_lot::{Mutex, RwLock};
use serde_json::{json, Map, Value};

use crate::browser_agent::{scrub, Audit, Rate};
use crate::subs::cut;

/// The tools, by the short names `window.call` takes (`kelpie_window_<name>` for an agent).
pub const TOOLS: [&str; 6] = ["launch", "list", "tree", "act", "shot", "close"];

/// What a tool does to a program: looks at it, or starts it, acts on it or closes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Read,
    Act,
}

pub fn classify(tool: &str) -> Option<Class> {
    Some(match tool {
        "list" | "tree" | "shot" => Class::Read,
        "launch" | "act" | "close" => Class::Act,
        _ => return None,
    })
}

// ---------- policy ----------

/// Whether agents may use windows at all. `ask` per program is not offered yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Off,
    On,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "off" => Some(Mode::Off),
            "on" => Some(Mode::On),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::On => "on",
        }
    }
}

/// The user's choices. `visible`: a program may be started on the person's own desktop (`visible: true`), where acting on a window
/// brings it forward.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Policy {
    pub mode: Mode,
    pub visible: bool,
}

impl Policy {
    /// From `policy.json`: a key that is missing or not valid is the default.
    pub fn from_file(file: Option<&Value>) -> Policy {
        let get = |key: &str| file.and_then(|v| v.get(key));
        Policy {
            mode: get("window_agents").and_then(Value::as_str).and_then(Mode::parse).unwrap_or_default(),
            visible: get("window_visible").and_then(Value::as_bool).unwrap_or(false),
        }
    }

    /// The policy `daemon.set_policy` asks for on top of `now`; `None` when it names neither key.
    pub fn requested(params: &Value, now: &Policy) -> Result<Option<Policy>> {
        let given = |key: &str| params.get(key).filter(|v| !v.is_null());
        let mode = given("window_agents").map(|v| v.as_str().and_then(Mode::parse).ok_or_else(|| anyhow!("window_agents must be off or on"))).transpose()?;
        let visible = given("window_visible").map(|v| v.as_bool().ok_or_else(|| anyhow!("window_visible must be true or false"))).transpose()?;
        if mode.is_none() && visible.is_none() {
            return Ok(None);
        }
        Ok(Some(Policy { mode: mode.unwrap_or(now.mode), visible: visible.unwrap_or(now.visible) }))
    }

    /// Writes the keys into `policy.json`'s object, leaving the others.
    pub fn write_into(&self, file: &mut Value) {
        file["window_agents"] = json!(self.mode.as_str());
        file["window_visible"] = json!(self.visible);
    }
}

// ---------- the gate ----------

pub const OFF: &str = "Window control is off. Ask the user to turn it on: window_agents in Kelpie's policy.json (see the user guide, \"Agents using windows\").";
pub const TOO_MANY: &str = "Too many window calls; slow down.";
pub const TOO_MANY_LAUNCHES: &str = "Too many programs started in the last hour by this session; the person can start more themselves, or you can wait.";
pub const BUSY: &str = "Too many window calls are running at once; wait for one to finish.";
pub const WINDOWS_ONLY: &str = "Window tools work on Windows only.";
pub const NO_HELPER: &str = "The window helper is not installed with this build.";
pub const VISIBLE_NOT_ALLOWED: &str = "Starting or using programs on the person's own desktop is not allowed: the person has to set window_visible to true in Kelpie's policy.json. Leave `visible` out to use the hidden desktop.";

pub fn unknown_tool(tool: &str) -> String {
    format!("unknown window tool `{}`: one of {}", cut(tool, 40), TOOLS.join(", "))
}

/// Whether the policy lets `tool` run at all: its class, or why not.
pub fn gate(policy: &Policy, tool: &str) -> Result<Class, String> {
    let class = classify(tool).ok_or_else(|| unknown_tool(tool))?;
    match policy.mode {
        Mode::Off => Err(OFF.into()),
        Mode::On => Ok(class),
    }
}

// ---------- arguments ----------

/// The most text a `set` takes, in bytes (the helper's own bound).
pub const MAX_TEXT_BYTES: usize = 4096;
/// The most outline lines `tree` may ask for.
pub const MAX_TREE: u64 = 1500;
const MAX_PROGRAM: usize = 1024;
const MAX_ARGS: usize = 32;
const MAX_ARG: usize = 1024;
const MAX_ARGS_TOTAL: usize = 8000;
const MAX_ID: usize = 200;
const MAX_ID_PARTS: usize = 16;
const ACTIONS: &[&str] = &["invoke", "set", "toggle", "expand", "collapse", "select", "show"];
/// The helper's own command-line options (`tree`'s); a program's argument is never one of them.
const HELPER_OPTIONS: &[&str] = &["--max", "--rects"];

/// The arguments each tool takes. Every tool takes `visible`.
fn keys_of(tool: &str) -> &'static [&'static str] {
    match tool {
        "launch" => &["program", "args"],
        "tree" => &["window", "max"],
        "act" => &["window", "id", "action", "text"],
        "shot" | "close" => &["window"],
        _ => &[],
    }
}

/// A window target as `kelpie-window list` prints it: `w:<digits>:<digits>`.
pub fn window_ok(target: &str) -> bool {
    let Some(rest) = target.strip_prefix("w:") else { return false };
    let Some((hwnd, pid)) = rest.split_once(':') else { return false };
    let digits = |s: &str| !s.is_empty() && s.len() <= 20 && s.bytes().all(|b| b.is_ascii_digit());
    digits(hwnd) && digits(pid)
}

/// An element id from a tree: numbers joined by dots, each possibly negative.
pub fn id_ok(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID || id.split('.').count() > MAX_ID_PARTS {
        return false;
    }
    id.split('.').all(|part| {
        let digits = part.strip_prefix('-').unwrap_or(part);
        !digits.is_empty() && digits.len() <= 11 && digits.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Whether a program's argument could be taken for one of the helper's own options.
fn helper_option(arg: &str) -> bool {
    HELPER_OPTIONS.iter().any(|o| arg == *o || arg.strip_prefix(o).is_some_and(|rest| rest.starts_with('=')))
}

fn bad(key: &str, what: &str) -> String {
    format!("`{key}` {what}")
}

/// A text argument: absent is `None`.
fn text<'a>(given: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, String> {
    match given.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.as_str())),
        Some(_) => Err(bad(key, "must be text")),
    }
}

/// `args` for `tool` checked against what the tool takes, with the defaults left to the helper: what the command is built from.
/// Unknown arguments are refused. A reason may name what was wrong, never the value (a typed text may be a secret).
pub fn check_args(tool: &str, args: &Value) -> Result<Value, String> {
    let empty = Map::new();
    let given = match args {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err("`args` must be an object".into()),
    };
    let takes = keys_of(tool);
    if let Some(k) = given.keys().find(|k| k.as_str() != "visible" && !takes.contains(&k.as_str())) {
        let list = takes.iter().copied().chain(std::iter::once("visible")).collect::<Vec<_>>().join(", ");
        return Err(format!("kelpie_window_{tool} takes no `{}`; it takes {list}", cut(k, 40)));
    }
    let mut out = Map::new();
    match given.get("visible") {
        None | Some(Value::Null) => {}
        Some(Value::Bool(b)) => {
            out.insert("visible".into(), json!(b));
        }
        Some(_) => return Err(bad("visible", "must be true or false")),
    }
    let window = || -> Result<String, String> {
        let w = text(given, "window")?.ok_or("missing `window`")?;
        if window_ok(w) {
            Ok(w.to_string())
        } else {
            Err(bad("window", "is a target from kelpie_window_list, like w:123:456"))
        }
    };
    match tool {
        "list" => {}
        "launch" => {
            let program = text(given, "program")?.map(str::trim).filter(|p| !p.is_empty()).ok_or("missing `program`")?;
            if program.chars().count() > MAX_PROGRAM || program.chars().any(char::is_control) || program.starts_with('-') {
                return Err(bad("program", "must be the path of a program: no control characters, not starting with `-`, at most 1024 characters"));
            }
            out.insert("program".into(), json!(program));
            let list: Vec<String> = match given.get("args") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(items)) => {
                    if items.len() > MAX_ARGS {
                        return Err(bad("args", "holds at most 32 arguments"));
                    }
                    let mut list = Vec::new();
                    for item in items {
                        let a = item.as_str().ok_or_else(|| bad("args", "is a list of text"))?;
                        if a.chars().count() > MAX_ARG || a.contains('\0') {
                            return Err(bad("args", "holds an argument that is too long (1024 characters) or has a NUL"));
                        }
                        if helper_option(a) {
                            return Err(bad("args", "holds an argument that the window helper would take for its own option (--max, --rects)"));
                        }
                        list.push(a.to_string());
                    }
                    if list.iter().map(String::len).sum::<usize>() > MAX_ARGS_TOTAL {
                        return Err(bad("args", "is too long in all"));
                    }
                    list
                }
                Some(_) => return Err(bad("args", "is a list of text")),
            };
            out.insert("args".into(), json!(list));
        }
        "tree" => {
            out.insert("window".into(), json!(window()?));
            match given.get("max") {
                None | Some(Value::Null) => {}
                Some(v) => {
                    let n = v.as_u64().filter(|n| (1..=MAX_TREE).contains(n)).ok_or_else(|| bad("max", "must be a whole number from 1 to 1500"))?;
                    out.insert("max".into(), json!(n));
                }
            }
        }
        "shot" | "close" => {
            out.insert("window".into(), json!(window()?));
        }
        "act" => {
            out.insert("window".into(), json!(window()?));
            let id = text(given, "id")?.ok_or("missing `id`")?;
            if !id_ok(id) {
                return Err(bad("id", "is the id in [brackets] in kelpie_window_tree, like 42.1312340.4.7"));
            }
            out.insert("id".into(), json!(id));
            let action = text(given, "action")?.ok_or("missing `action`")?;
            let action = ACTIONS.iter().copied().find(|a| *a == action).ok_or_else(|| format!("`action` is one of {}", ACTIONS.join(", ")))?;
            out.insert("action".into(), json!(action));
            match (action, text(given, "text")?) {
                ("set", Some(t)) if t.len() > MAX_TEXT_BYTES => return Err(format!("`text` is longer than {MAX_TEXT_BYTES} bytes")),
                ("set", Some(t)) if t.contains('\0') => return Err(bad("text", "has a NUL")),
                ("set", Some(t)) => {
                    out.insert("text".into(), json!(t));
                }
                ("set", None) => return Err("`set` needs `text`".into()),
                (_, Some(_)) => return Err("`text` goes only with the action `set`".into()),
                (_, None) => {}
            }
        }
        other => return Err(unknown_tool(other)),
    }
    Ok(Value::Object(out))
}

/// How long the daemon lets the helper run: `launch` waits for the program's first window, a read for a web view's page.
pub fn limit(tool: &str) -> Duration {
    Duration::from_secs(if tool == "launch" { 30 } else { 20 })
}

// ---------- the command ----------

/// A helper process to run: never started through a shell.
#[derive(Clone, Debug, PartialEq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<&'static str>,
    pub limit: Duration,
}

/// Runs a helper and answers its text, or its complaint. Tests answer from a script; [`run_helper`] starts the process.
pub type Runner = Arc<dyn Fn(&Invocation) -> Result<String, String> + Send + Sync>;

/// The variables the helper (and so the programs it starts) must not inherit from the daemon: they name this Kelpie, or speak for a
/// session. Kelpie itself is then refused by the helper (it is started only as a separate one, with its own endpoint and data folder).
const NOT_INHERITED: &[&str] = &["KELPIE_ENDPOINT", "KELPIE_DATA_DIR", "KELPIE_SESSION_ENDPOINT", "KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN"];

/// The helper's program and arguments for `tool` with checked `args` (what [`check_args`] answered). `shot` is the file the picture
/// is written to, which the daemon chose.
pub fn command(helper: &Path, tool: &str, args: &Value, shot: Option<&Path>) -> (PathBuf, Vec<String>) {
    let s = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut out = vec![tool.to_string()];
    match tool {
        "launch" => {
            out.push(s("program"));
            out.extend(args.get("args").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
        }
        "tree" => {
            out.push(s("window"));
            if let Some(n) = args.get("max").and_then(Value::as_u64) {
                out.push("--max".into());
                out.push(n.to_string());
            }
        }
        "act" => {
            out.extend([s("window"), s("id"), s("action")]);
            if args.get("text").is_some() {
                out.push(s("text"));
            }
        }
        "shot" => {
            out.push(s("window"));
            out.push(shot.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
        }
        "close" => out.push(s("window")),
        _ => {}
    }
    (helper.to_path_buf(), out)
}

fn invocation(helper: &Path, tool: &str, args: &Value, shot: Option<&Path>, sid: &str, visible: bool, limit: Duration) -> Invocation {
    let (program, args) = command(helper, tool, args, shot);
    let mut env = vec![("KELPIE_WINDOW_SCOPE".to_string(), sid.to_string())];
    if !visible {
        env.push(("KELPIE_WINDOW_DESKTOP".to_string(), "hidden".to_string()));
    }
    Invocation { program, args, env, env_remove: NOT_INHERITED.to_vec(), limit }
}

/// Reads `source` to its end but keeps at most `max` bytes: the child is never left blocked on a full pipe.
fn read_bounded(mut source: impl Read, max: usize) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        match source.read(&mut buf) {
            Ok(0) | Err(_) => return kept,
            Ok(n) => {
                let room = max.saturating_sub(kept.len());
                kept.extend_from_slice(&buf[..n.min(room)]);
            }
        }
    }
}

const MAX_STDOUT: usize = 256 * 1024;
const MAX_STDERR: usize = 16 * 1024;

/// Starts the helper with no input, reads what it prints within bounds, and ends it when it does not finish within the limit.
pub fn run_helper(inv: &Invocation) -> Result<String, String> {
    let mut cmd = std::process::Command::new(&inv.program);
    cmd.args(&inv.args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    for name in &inv.env_remove {
        cmd.env_remove(name);
    }
    cmd.envs(inv.env.iter().map(|(k, v)| (k, v)));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window for the helper.
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("the window helper could not be started: {e}"))?;
    let out = child.stdout.take().map(|s| std::thread::spawn(move || read_bounded(s, MAX_STDOUT)));
    let err = child.stderr.take().map(|s| std::thread::spawn(move || read_bounded(s, MAX_STDERR)));
    let until = Instant::now() + inv.limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() > until => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(format!("the window helper could not be waited for: {e}")),
        }
    };
    let text = |h: Option<std::thread::JoinHandle<Vec<u8>>>| h.and_then(|h| h.join().ok()).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let (out, err) = (text(out), text(err));
    match status {
        None => Err(format!("the window helper did not answer within {} s: the program may be busy or not answering", inv.limit.as_secs())),
        Some(s) if s.success() => Ok(out),
        Some(_) => Err(if err.trim().is_empty() { out } else { err }),
    }
}

// ---------- where the helper is ----------

/// The helper's file name.
pub const HELPER_NAME: &str = if cfg!(windows) { "kelpie-window.exe" } else { "kelpie-window" };

/// Where the helper may be, in the order looked at: next to the daemon, next to the app (the folder a runtime slot's `source.txt` names:
/// on Windows the daemon runs from a copy in `<data>/daemon/a|b` while the app and its helpers stay in the install folder), and in a
/// debug build `KELPIE_WINDOW_PATH` first and a development build's `release` folder last. A release build takes no other path.
pub fn helper_candidates(own_dir: Option<&Path>, source: Option<&str>, env_path: Option<&str>, debug: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if debug {
        out.extend(env_path.filter(|p| !p.is_empty()).map(PathBuf::from));
    }
    if let Some(dir) = own_dir {
        out.push(dir.join(HELPER_NAME));
    }
    if let Some(source) = source.map(str::trim).filter(|s| !s.is_empty()) {
        out.push(Path::new(source).join(HELPER_NAME));
    }
    if debug {
        out.extend(own_dir.and_then(Path::parent).map(|p| p.join("release").join(HELPER_NAME)));
    }
    out
}

/// The helper installed with this build, if there is one.
pub fn find_helper() -> Option<PathBuf> {
    let own = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let source = own.as_deref().and_then(|d| std::fs::read_to_string(d.join(crate::peer_identity::SLOT_SOURCE)).ok());
    let env_path = std::env::var("KELPIE_WINDOW_PATH").ok();
    helper_candidates(own.as_deref(), source.as_deref(), env_path.as_deref(), cfg!(debug_assertions)).into_iter().find(|p| p.is_file())
}

// ---------- rate ----------

/// The most programs a session starts in an hour.
pub const LAUNCHES_PER_HOUR: usize = 20;
const HOUR: Duration = Duration::from_secs(3600);

/// When each session started its programs in the last hour. A cap on starts, since what is alive cannot be counted without a job
/// object per session.
#[derive(Default)]
pub struct Launches {
    by_session: HashMap<String, VecDeque<Instant>>,
}

impl Launches {
    /// One start of session `sid` at `now`; false when it has used its hour's share.
    pub fn take(&mut self, sid: &str, now: Instant) -> bool {
        self.by_session.retain(|_, times| {
            while times.front().is_some_and(|t| now.saturating_duration_since(*t) >= HOUR) {
                times.pop_front();
            }
            !times.is_empty()
        });
        let times = self.by_session.entry(sid.to_string()).or_default();
        if times.len() >= LAUNCHES_PER_HOUR {
            return false;
        }
        times.push_back(now);
        true
    }
}

// ---------- audit ----------

pub const AUDIT_FILE: &str = "window-audit.jsonl";

/// The file name of a program, from its path as given.
fn file_name(program: &str) -> &str {
    program.rsplit(['\\', '/']).next().unwrap_or(program)
}

/// One audit line. Only these fields, whatever the call carried: never the text of a `set`. `target` is the window, or for a start the
/// program's file name (not its path or arguments); `error` a failure the caller already chose to keep.
pub fn audit_line(at: u64, session: &str, label: &str, tool: &str, args: &Value, error: Option<&str>) -> Value {
    let target = match tool {
        "launch" => args.get("program").and_then(Value::as_str).map(file_name).map(|p| scrub(p, 60)),
        _ => args.get("window").and_then(Value::as_str).map(|w| scrub(w, 60)),
    };
    json!({
        "at": at, "session": session, "label": cut(label, 80), "tool": tool, "target": target,
        "action": args.get("action").and_then(Value::as_str), "ok": error.is_none(), "error": error.map(|e| scrub(e, 120)),
    })
}

// ---------- the state ----------

/// A session that used windows: whether it started a program on the person's desktop.
#[derive(Clone, Copy, Default)]
struct Used {
    visible: bool,
}

/// What the daemon keeps for window control.
pub struct State {
    pub policy: RwLock<Policy>,
    pub rate: Mutex<Rate>,
    launches: Mutex<Launches>,
    pub audit: Arc<Audit>,
    /// Sessions that started a program, for what to close when one ends and which desktop a call without `visible` means.
    used: Mutex<HashMap<String, Used>>,
    running: AtomicUsize,
    shots: PathBuf,
    runner: Runner,
    /// The helper to run; `None`: look for the installed one at each call.
    helper: Option<PathBuf>,
    /// Whether this computer runs the helper (Windows).
    pub supported: bool,
}

/// The most helpers running at once, for all sessions.
const MAX_RUNNING: usize = 6;
/// The newest pictures kept per session.
pub const KEEP_SHOTS: usize = 20;
/// The largest picture handed to an agent; a larger one is kept in the file and named.
pub const MAX_SHOT_BYTES: usize = 4 << 20;

/// Counts a helper that runs and stops counting when it is dropped.
struct Running<'a>(&'a AtomicUsize);

impl<'a> Running<'a> {
    fn start(counter: &'a AtomicUsize) -> Option<Running<'a>> {
        if counter.fetch_add(1, Ordering::SeqCst) >= MAX_RUNNING {
            counter.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Running(counter))
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl State {
    pub fn new(policy: Policy, data_dir: &Path) -> State {
        State {
            policy: RwLock::new(policy),
            rate: Mutex::new(Rate::default()),
            launches: Mutex::new(Launches::default()),
            audit: Arc::new(Audit::named(data_dir, AUDIT_FILE)),
            used: Mutex::new(HashMap::new()),
            running: AtomicUsize::new(0),
            shots: data_dir.join("window-shots"),
            runner: Arc::new(run_helper),
            helper: None,
            supported: cfg!(windows),
        }
    }

    /// A state that runs `runner` as the helper `helper`, on any computer (tests).
    #[cfg(test)]
    pub fn with_runner(policy: Policy, data_dir: &Path, helper: &Path, runner: Runner) -> State {
        State { runner, helper: Some(helper.to_path_buf()), supported: true, ..State::new(policy, data_dir) }
    }

    /// Whether session `sid` started a program (so something may be left to close when it ends).
    pub fn has_session(&self, sid: &str) -> bool {
        self.used.lock().contains_key(sid)
    }

    fn shot_dir(&self, sid: &str) -> PathBuf {
        self.shots.join(sid)
    }
}

/// A session id as a folder and scope name: what the daemon makes (letters, digits, `-`, `_`), bounded.
fn id_safe(sid: &str) -> bool {
    !sid.is_empty() && sid.len() <= 64 && sid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Which desktop a call means: its own `visible` when it says one (the policy must allow the person's desktop), else where this session
/// last started a program.
fn wants_visible(policy: &Policy, args: &Value, used: Option<Used>) -> Result<bool, String> {
    match args.get("visible").and_then(Value::as_bool) {
        Some(true) if !policy.visible => Err(VISIBLE_NOT_ALLOWED.into()),
        Some(v) => Ok(v),
        None => Ok(used.is_some_and(|u| u.visible) && policy.visible),
    }
}

/// Keeps the newest [`KEEP_SHOTS`] pictures of a folder (blocking).
fn trim_shots(dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = read
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "png"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort();
    let extra = files.len().saturating_sub(KEEP_SHOTS);
    for (_, path) in files.into_iter().take(extra) {
        let _ = std::fs::remove_file(path);
    }
}

/// The helper's `shot` answer: `<width>x<height> <path>`.
fn shot_size(answer: &str) -> (u64, u64) {
    let first = answer.split_whitespace().next().unwrap_or_default();
    let (w, h) = first.split_once('x').unwrap_or_default();
    (w.parse().unwrap_or(0), h.parse().unwrap_or(0))
}

/// A picture's reply, or the file named when it is too large to pass on.
fn shot_reply(path: &Path, answer: &str) -> Result<Value, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("the picture could not be read back: {e}"))?;
    let (width, height) = shot_size(answer);
    let shown = path.to_string_lossy().into_owned();
    if bytes.len() > MAX_SHOT_BYTES {
        return Ok(json!({"text": format!("Saved {shown} ({width}×{height}, {} MiB): too large to show here; a smaller window is pictured whole.", bytes.len() >> 20)}));
    }
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(json!({"text": "", "image": {"mime": "image/png", "data": data, "path": shown, "width": width, "height": height}}))
}

/// The most text of the helper's answer passed on.
const MAX_REPLY_CHARS: usize = 30_000;

/// One call of session `sid` (named `label` in the audit) to `tool` with `args`, at `now` (milliseconds): the policy, rate and
/// argument checks, then the helper. Answers `{text, image?: {mime, data, path, width, height}}`, or the reason to tell the agent.
/// Blocking: run it off the event loop.
pub fn execute(state: &State, sid: &str, label: &str, tool: &str, args: &Value, now: u64) -> Result<Value, String> {
    let policy = *state.policy.read();
    gate(&policy, tool)?;
    if !state.supported {
        return Err(WINDOWS_ONLY.into());
    }
    if !id_safe(sid) {
        return Err("window tools are for programs running in a Kelpie session".into());
    }
    if !state.rate.lock().take(sid, Instant::now()) {
        return Err(TOO_MANY.into());
    }
    let args = check_args(tool, args)?;
    let visible = wants_visible(&policy, &args, state.used.lock().get(sid).copied())?;
    let helper = state.helper.clone().or_else(find_helper).ok_or(NO_HELPER)?;
    if tool == "launch" && !state.launches.lock().take(sid, Instant::now()) {
        return Err(TOO_MANY_LAUNCHES.into());
    }
    let _running = Running::start(&state.running).ok_or(BUSY)?;
    let shot = (tool == "shot").then(|| {
        let dir = state.shot_dir(sid);
        let _ = std::fs::create_dir_all(&dir);
        trim_shots(&dir);
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        dir.join(format!("{now}-{}.png", SEQ.fetch_add(1, Ordering::Relaxed)))
    });
    let inv = invocation(&helper, tool, &args, shot.as_deref(), sid, visible, limit(tool));
    if tool == "launch" {
        let mut used = state.used.lock();
        let entry = used.entry(sid.to_string()).or_default();
        entry.visible |= visible;
    }
    let result = (state.runner)(&inv).and_then(|text| match &shot {
        Some(path) => shot_reply(path, &text),
        None => Ok(json!({"text": cut(text.trim(), MAX_REPLY_CHARS)})),
    });
    let result = result.map_err(|e| scrub(&e, 400));
    if classify(tool) == Some(Class::Act) {
        let line = audit_line(now, sid, label, tool, &args, result.as_ref().err().map(String::as_str));
        if let Err(e) = state.audit.append(&line) {
            tracing::warn!("window audit: {e}");
        }
    }
    result
}

/// The targets in the helper's `list` answer: the first word of each line, when it is a window target.
fn listed_targets(answer: &str) -> Vec<String> {
    answer.lines().filter_map(|l| l.split_whitespace().next()).filter(|t| window_ok(t)).map(str::to_string).collect()
}

/// The most windows asked to close when a session ends.
const MAX_CLOSE: usize = 16;

/// When session `sid` ends: asks the windows of the programs it started to close (`WM_CLOSE`: a program may ask a question first or
/// ignore it), and removes its pictures. Best effort, each helper call bounded to a few seconds; blocking. The programs are not ended by
/// force: that needs a job object per session.
pub fn end_session(state: &State, sid: &str) {
    let Some(used) = state.used.lock().remove(sid) else { return };
    let _ = std::fs::remove_dir_all(state.shot_dir(sid));
    if !state.supported || !id_safe(sid) {
        return;
    }
    let Some(helper) = state.helper.clone().or_else(find_helper) else { return };
    let few = Duration::from_secs(5);
    let desktops: &[bool] = if used.visible { &[false, true] } else { &[false] };
    for &visible in desktops {
        let list = invocation(&helper, "list", &Value::Null, None, sid, visible, few);
        let Ok(answer) = (state.runner)(&list) else { continue };
        for target in listed_targets(&answer).into_iter().take(MAX_CLOSE) {
            let close = invocation(&helper, "close", &json!({"window": target}), None, sid, visible, few);
            if let Err(e) = (state.runner)(&close) {
                tracing::debug!("window close at the end of a session: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests;
