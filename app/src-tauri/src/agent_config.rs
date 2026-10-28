//! Agent setup: the MCP servers, instruction files and skills of the coding agents on this
//! computer, in one place. Everything is read when the page opens (no watcher, no timer) and
//! changed only through the same path: a preview (the diff, with secrets masked), a backup of each
//! file, the write, and a check that the file still parses, undone at once when it does not.
//!
//! Trust (see docs/architecture.md): a server found in a repository is `project` scope and
//! untrusted; copying it to a user-level file would skip the agent's own "trust this server?"
//! prompt, so it needs an explicit confirmation that shows the exact command. Environment and
//! header values are never written into project files and never leave this module: the UI sees
//! their names only.

mod backup;
mod formats;
mod guides;
mod mcp;
mod registry;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::fs_ops::{err, CmdResult};
use formats::{json_array_member, json_at, json_check, json_is_strict, json_read, json_read_keys, json_remove, json_set, toml_check, toml_remove_server, toml_servers, toml_set_server, toml_set_server_key};
use guides::{GeminiMode, Instructions, SkillItem};
use mcp::{decode, encode, for_destination, warn, Kind, McpServer, Transport, Warn};

type Res<T> = Result<T, String>;

/// Largest config file read or written.
const MAX_FILE_BYTES: u64 = 32 << 20;
/// Lines of unchanged text shown around a change.
const CONTEXT_LINES: usize = 3;
/// Lines of either side sent for one file's diff.
const MAX_DIFF_LINES: usize = 3000;
const MASK: &str = "********";

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Project,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Target {
    pub agent: String,
    pub scope: Scope,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileKind {
    Json,
    Toml,
    Text,
}

// ---------- where things live ----------

/// The agents' config homes. `KELPIE_AGENT_CONFIG_HOME` replaces the user's home for a test run
/// (end-to-end runs only; then the agents' own variables are ignored, so a stray `CLAUDE_CONFIG_DIR`
/// cannot send a test to the real folders). An end-to-end run without it may read but not write.
#[derive(Clone, Debug)]
pub struct Roots {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub claude_json: PathBuf,
    pub codex_home: PathBuf,
    /// The `.gemini` folder.
    pub gemini_dir: PathBuf,
    pub opencode_dir: PathBuf,
    pub writable: bool,
}

impl Roots {
    /// Every location under `home`, as the agents default to.
    pub fn under(home: &Path) -> Roots {
        Roots {
            home: home.to_path_buf(),
            claude_dir: home.join(".claude"),
            claude_json: home.join(".claude.json"),
            codex_home: home.join(".codex"),
            gemini_dir: home.join(".gemini"),
            opencode_dir: home.join(".config").join("opencode"),
            writable: true,
        }
    }

    pub fn from_env() -> Roots {
        Roots::from_vars(|k| std::env::var_os(k), crate::accounts::home())
    }

    /// The homes for a given environment (a function, so the rules can be tested without touching
    /// this process's variables). `real_home` is the user's home directory.
    pub fn from_vars(get: impl Fn(&str) -> Option<std::ffi::OsString>, real_home: PathBuf) -> Roots {
        let var = |k: &str| get(k).filter(|v| !v.is_empty()).map(PathBuf::from);
        if get("KELPIE_E2E").is_some() {
            return match var("KELPIE_AGENT_CONFIG_HOME") {
                Some(home) => Roots::under(&home),
                None => Roots { writable: false, ..Roots::under(&real_home) },
            };
        }
        let mut r = Roots::under(&real_home);
        if let Some(d) = var("CLAUDE_CONFIG_DIR") {
            r.claude_json = d.join(".claude.json");
            r.claude_dir = d;
        }
        if let Some(d) = var("CODEX_HOME") {
            r.codex_home = d;
        }
        if let Some(d) = var("GEMINI_CLI_HOME") {
            r.gemini_dir = d.join(".gemini");
        }
        if let Some(d) = var("OPENCODE_CONFIG_DIR") {
            r.opencode_dir = d;
        } else if let Some(d) = var("XDG_CONFIG_HOME") {
            r.opencode_dir = d.join("opencode");
        }
        r
    }
}

/// Everything one operation needs, so tests run against folders of their own.
pub struct Ctx {
    pub roots: Roots,
    pub project: Option<PathBuf>,
    pub backups: PathBuf,
    /// End-to-end runs only: when this file exists the next write is corrupted (and the file removed).
    pub corrupt_marker: Option<PathBuf>,
}

impl Ctx {
    pub fn from_env(project: Option<&str>) -> Ctx {
        let e2e = std::env::var_os("KELPIE_E2E").is_some();
        let data = kelpie_proto::data_dir();
        Ctx {
            roots: Roots::from_env(),
            project: local_project(project),
            backups: data.join("agent-config-backups"),
            corrupt_marker: e2e.then(|| data.join("agent-setup-corrupt-next")),
        }
    }
}

fn local_project(project: Option<&str>) -> Option<PathBuf> {
    let p = project.filter(|p| !p.is_empty() && !crate::remote::is_remote(p))?;
    Path::new(p).is_dir().then(|| PathBuf::from(p))
}

pub fn read_opt(path: &Path) -> Res<Option<String>> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if !meta.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{} is too large to edit here", path.display()));
    }
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => Err(format!("{} is not UTF-8 text", path.display())),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// (agent, scope) pairs that have an MCP config file.
pub const MCP_TARGETS: &[(&str, Scope)] = &[
    ("claude", Scope::User),
    ("claude", Scope::Project),
    ("codex", Scope::User),
    ("gemini", Scope::User),
    ("gemini", Scope::Project),
    ("opencode", Scope::User),
    ("opencode", Scope::Project),
    ("cursor", Scope::User),
    ("cursor", Scope::Project),
];

fn opencode_file(dir: &Path) -> PathBuf {
    let jsonc = dir.join("opencode.jsonc");
    if jsonc.is_file() { jsonc } else { dir.join("opencode.json") }
}

/// Refuses a path in a project that, followed through links and junctions, leaves the project:
/// a repository must not be able to make Kelpie read or write a file elsewhere. A link that stays
/// inside is fine. The error names where the path really leads.
pub(crate) fn confine(root: &Path, path: &Path) -> Res<()> {
    let croot = std::fs::canonicalize(root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut probe = path;
    while std::fs::symlink_metadata(probe).is_err() {
        match probe.parent() {
            Some(p) if p != probe => probe = p,
            _ => return Ok(()),
        }
    }
    let real = std::fs::canonicalize(probe).map_err(|_| format!("{} is a link that leads nowhere", probe.display()))?;
    if real.starts_with(&croot) {
        return Ok(());
    }
    Err(format!("{} resolves outside the project: {}", path.display(), real.to_string_lossy().trim_start_matches(r"\\?\")))
}

/// Two names for one file (a link, in either direction).
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// The file holding `t`'s MCP servers and how to read it; a project's file must stay in the project.
fn mcp_path(ctx: &Ctx, t: &Target) -> Res<(PathBuf, FileKind)> {
    let found = mcp_path_raw(ctx, t)?;
    if t.scope == Scope::Project {
        confine(ctx.project.as_ref().ok_or("Open a project first")?, &found.0)?;
    }
    Ok(found)
}

fn mcp_path_raw(ctx: &Ctx, t: &Target) -> Res<(PathBuf, FileKind)> {
    let r = &ctx.roots;
    let project = || ctx.project.clone().ok_or_else(|| "Open a project first".to_string());
    Ok(match (t.agent.as_str(), t.scope) {
        ("claude", Scope::User) => (r.claude_json.clone(), FileKind::Json),
        ("claude", Scope::Project) => (project()?.join(".mcp.json"), FileKind::Json),
        ("codex", Scope::User) => (r.codex_home.join("config.toml"), FileKind::Toml),
        ("gemini", Scope::User) => (r.gemini_dir.join("settings.json"), FileKind::Json),
        ("gemini", Scope::Project) => (project()?.join(".gemini").join("settings.json"), FileKind::Json),
        ("opencode", Scope::User) => (opencode_file(&r.opencode_dir), FileKind::Json),
        ("opencode", Scope::Project) => (opencode_file(&project()?), FileKind::Json),
        ("cursor", Scope::User) => (r.home.join(".cursor").join("mcp.json"), FileKind::Json),
        ("cursor", Scope::Project) => (project()?.join(".cursor").join("mcp.json"), FileKind::Json),
        (a, s) => return Err(format!("{a} has no {} MCP file", if s == Scope::User { "user" } else { "project" })),
    })
}

fn kind_of(t: &Target) -> Res<Kind> {
    Kind::of(&t.agent).ok_or_else(|| format!("{} is not a supported agent", t.agent))
}

/// Whether this file can switch a server off (each agent has its own way, or none).
fn can_toggle(t: &Target) -> bool {
    matches!((t.agent.as_str(), t.scope), ("codex", Scope::User) | ("opencode", _) | ("gemini", _) | ("claude", Scope::Project))
}

fn claude_disabled_file(ctx: &Ctx) -> Res<PathBuf> {
    let root = ctx.project.clone().ok_or("Open a project first")?;
    let file = root.join(".claude").join("settings.local.json");
    confine(&root, &file)?;
    Ok(file)
}

fn valid_name(name: &str, strict: bool) -> Res<()> {
    let plain = if strict {
        !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    } else {
        !name.is_empty() && name.len() <= 128 && !name.chars().any(|c| c.is_control())
    };
    if plain { Ok(()) } else { Err(format!("“{name}” is not a usable server name (letters, digits, - and _)")) }
}

// ---------- reading ----------

/// The servers of a document, and (JSON) the top-level keys read, where Gemini keeps its
/// exclusion list.
fn servers_in(kind: Kind, fkind: FileKind, text: &str) -> Res<(Vec<McpServer>, Value)> {
    match fkind {
        FileKind::Toml => Ok((toml_servers(text)?.iter().map(|(n, v)| decode(kind, n, v)).collect(), Value::Null)),
        _ => {
            let doc = json_read_keys(text, &[kind.servers_key(), "mcp"])?;
            let servers = doc.get(kind.servers_key()).and_then(Value::as_object).map(|o| o.iter().map(|(n, v)| decode(kind, n, v)).collect()).unwrap_or_default();
            Ok((servers, doc))
        }
    }
}

fn string_list(doc: &Value, path: &[&str], key: &str) -> Vec<String> {
    json_at(doc, path).and_then(|o| o.get(key)).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

/// A file's servers with each agent's own on/off state applied.
fn read_servers(ctx: &Ctx, t: &Target) -> Res<(Option<String>, Vec<McpServer>)> {
    let (path, fkind) = mcp_path(ctx, t)?;
    let kind = kind_of(t)?;
    let text = read_opt(&path)?;
    let (mut servers, doc) = servers_in(kind, fkind, text.as_deref().unwrap_or(""))?;
    let disabled: Vec<String> = match (t.agent.as_str(), t.scope) {
        ("gemini", _) => string_list(&doc, &["mcp"], "excluded"),
        ("claude", Scope::Project) => {
            let root = ctx.project.clone().ok_or("Open a project first")?;
            let mut all = Vec::new();
            for f in [root.join(".claude").join("settings.json"), claude_disabled_file(ctx)?] {
                if confine(&root, &f).is_err() {
                    continue;
                }
                if let Some(doc) = read_opt(&f).ok().flatten().and_then(|t| json_read(&t).ok()) {
                    all.extend(string_list(&doc, &[], "disabledMcpjsonServers"));
                }
            }
            all
        }
        _ => Vec::new(),
    };
    for s in &mut servers {
        if disabled.contains(&s.name) {
            s.enabled = false;
        }
    }
    Ok((text, servers))
}

/// A server as the UI sees it. Environment and header values are not part of it.
#[derive(Serialize, Debug)]
pub struct McpView {
    name: String,
    transport: &'static str,
    command: String,
    args: Vec<String>,
    url: String,
    env_keys: Vec<String>,
    header_keys: Vec<String>,
    enabled: bool,
    /// Entry keys Kelpie does not model (a copy would not carry them).
    extra: Vec<String>,
}

/// The page masks commands and URLs itself (`maskArg` in lib/agent-setup.ts); the read stays free
/// of the redaction patterns, whose first use would compile them while the page opens.
fn view(s: &McpServer) -> McpView {
    McpView {
        name: s.name.clone(),
        transport: s.transport.label(),
        command: s.command.clone(),
        args: s.args.clone(),
        url: s.url.clone(),
        env_keys: s.env.iter().map(|(k, _)| k.clone()).collect(),
        header_keys: s.headers.iter().map(|(k, _)| k.clone()).collect(),
        enabled: s.enabled,
        extra: s.extra.clone(),
    }
}

#[derive(Serialize, Debug)]
pub struct McpFile {
    agent: &'static str,
    scope: Scope,
    path: String,
    exists: bool,
    /// Why the file could not be read; the others are listed regardless.
    error: Option<String>,
    can_toggle: bool,
    servers: Vec<McpView>,
}

#[derive(Serialize, Debug)]
pub struct Overview {
    project: Option<String>,
    writable: bool,
    mcp: Vec<McpFile>,
    instructions: Instructions,
    skills: Vec<SkillItem>,
    /// How long reading all of it took.
    elapsed_ms: f64,
}

pub fn overview(ctx: &Ctx) -> Overview {
    let started = std::time::Instant::now();
    let mut mcp = Vec::new();
    for &(agent, scope) in MCP_TARGETS {
        let t = Target { agent: agent.into(), scope };
        let Ok((path, _)) = mcp_path_raw(ctx, &t) else { continue };
        let (exists, result) = (path.is_file(), read_servers(ctx, &t));
        let (servers, error) = match result {
            Ok((_, s)) => (s.iter().map(view).collect(), None),
            Err(e) => (Vec::new(), Some(e)),
        };
        mcp.push(McpFile { agent, scope, path: path.to_string_lossy().into_owned(), exists, error, can_toggle: can_toggle(&t), servers });
    }
    let instructions = guides::instructions(ctx);
    let skills = guides::skills(ctx);
    Overview { project: ctx.project.as_ref().map(|p| p.to_string_lossy().into_owned()), writable: ctx.roots.writable, mcp, instructions, skills, elapsed_ms: started.elapsed().as_secs_f64() * 1000.0 }
}

// ---------- edits ----------

#[derive(Deserialize, Clone, Debug)]
pub struct NewServer {
    pub name: String,
    /// stdio | http | sse
    pub transport: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Edit {
    Toggle { target: Target, name: String, enabled: bool },
    Remove { target: Target, name: String },
    Add { targets: Vec<Target>, server: NewServer },
    Copy {
        from: Target,
        name: String,
        to: Vec<Target>,
        /// Write environment values (user-level files only); otherwise they become references.
        #[serde(default)]
        include_env: bool,
        /// The user has seen the command of a project server and agreed to trust it.
        #[serde(default)]
        confirm_untrusted: bool,
    },
    Instructions {
        #[serde(default)]
        gemini: GeminiMode,
    },
    Restore { backup: String },
}

/// One file's change: its content before (none: absent) and after (none: removed).
#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub old: Option<String>,
    pub new: Option<String>,
    pub kind: FileKind,
}

/// One variable or header of a project server, as the repository declares it.
#[derive(Serialize, Debug, Clone)]
pub struct UntrustedValue {
    key: String,
    /// What the repository says (a value or a reference). Shown as written: it came from the repo.
    value: String,
    /// The value reads the user's own variable(s): the page shows that, never what they hold.
    reference: bool,
    /// For a reference that is only one variable, its name.
    var: Option<String>,
    /// Whether this copy writes it into the user's file.
    carried: bool,
}

/// A relative path made absolute so the copy keeps running that file, not whatever sits at the same
/// place in the next project.
#[derive(Serialize, Debug, Clone)]
pub struct Resolved {
    from: String,
    to: String,
}

/// Everything a project server would run once it is copied to the user's own settings.
#[derive(Serialize, Debug, Clone)]
pub struct Untrusted {
    name: String,
    agent: String,
    /// The program and each argument, one element each (or empty for a remote server).
    argv: Vec<String>,
    url: Option<String>,
    cwd: Option<String>,
    env: Vec<UntrustedValue>,
    headers: Vec<UntrustedValue>,
    resolved: Vec<Resolved>,
    /// Variables of the user's the command line or URL reads.
    arg_vars: Vec<String>,
}

struct Plan {
    changes: Vec<Change>,
    warnings: Vec<Warn>,
    untrusted: Vec<Untrusted>,
    needs_confirm: bool,
}

impl Plan {
    fn new() -> Plan {
        Plan { changes: Vec::new(), warnings: Vec::new(), untrusted: Vec::new(), needs_confirm: false }
    }

    fn push(&mut self, c: Change) {
        // The same file twice (two agents sharing one file): the later edit builds on the earlier.
        if let Some(prev) = self.changes.iter_mut().find(|p| p.path == c.path) {
            prev.new = c.new;
        } else if c.old != c.new {
            self.changes.push(c);
        }
    }

    fn token(&self) -> String {
        let mut h = Sha256::new();
        for c in &self.changes {
            h.update(c.path.to_string_lossy().as_bytes());
            for t in [&c.old, &c.new] {
                h.update([t.is_some() as u8]);
                h.update(Sha256::digest(t.as_deref().unwrap_or("").as_bytes()));
            }
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// `text` with the server written for `kind` / `fkind`.
fn put_server(kind: Kind, fkind: FileKind, text: &str, name: &str, value: &Value) -> Res<String> {
    match fkind {
        FileKind::Toml => toml_set_server(text, name, value),
        _ => json_set(text, &[kind.servers_key()], name, value),
    }
}

fn drop_server(kind: Kind, fkind: FileKind, text: &str, name: &str) -> Res<String> {
    match fkind {
        FileKind::Toml => toml_remove_server(text, name),
        _ => json_remove(text, &[kind.servers_key()], name),
    }
}

fn plan_toggle(ctx: &Ctx, plan: &mut Plan, t: &Target, name: &str, enabled: bool) -> Res<()> {
    if !can_toggle(t) {
        return Err(format!("{} has no switch for {} servers; remove the server instead.", t.agent, if t.scope == Scope::User { "user" } else { "project" }));
    }
    let (path, fkind) = mcp_path(ctx, t)?;
    let (text, servers) = read_servers(ctx, t)?;
    if !servers.iter().any(|s| s.name == name) {
        return Err(format!("no server named {name} in {}", path.display()));
    }
    let src = text.clone().unwrap_or_default();
    let (target_path, target_kind, old, new) = match (t.agent.as_str(), t.scope) {
        ("codex", _) => (path, fkind, text, toml_set_server_key(&src, name, "enabled", &Value::Bool(enabled))?),
        ("opencode", _) => (path, fkind, text, json_set(&src, &["mcp", name], "enabled", &Value::Bool(enabled))?),
        ("gemini", _) => (path, fkind, text, json_array_member(&src, &["mcp"], "excluded", name, !enabled)?),
        _ => {
            let side = claude_disabled_file(ctx)?;
            let old = read_opt(&side)?;
            let new = json_array_member(old.as_deref().unwrap_or(""), &[], "disabledMcpjsonServers", name, !enabled)?;
            (side, FileKind::Json, old, new)
        }
    };
    plan.push(Change { path: target_path, old, new: Some(new), kind: target_kind });
    Ok(())
}

fn plan_remove(ctx: &Ctx, plan: &mut Plan, t: &Target, name: &str) -> Res<()> {
    let (path, fkind) = mcp_path(ctx, t)?;
    let (text, servers) = read_servers(ctx, t)?;
    if !servers.iter().any(|s| s.name == name) {
        return Err(format!("no server named {name} in {}", path.display()));
    }
    let new = drop_server(kind_of(t)?, fkind, text.as_deref().unwrap_or(""), name)?;
    plan.push(Change { path, old: text, new: Some(new), kind: fkind });
    Ok(())
}

/// A path that is not absolute and names a place (not a program found on PATH).
fn relative_path(s: &str) -> bool {
    let rooted = Path::new(s).is_absolute() || s.starts_with('/') || s.starts_with('\\');
    !rooted && (s.contains('/') || s.contains('\\') || s == "." || s == "..") && !s.contains("://") && !s.starts_with(['-', '@', '$'])
}

/// `rel` below `root`, with `.` and `..` folded in; refused when it would leave the project.
fn resolve_in(root: &Path, rel: &str) -> Res<String> {
    let mut out = root.to_path_buf();
    for c in Path::new(&rel.replace('\\', "/")).components() {
        match c {
            std::path::Component::Normal(p) => out.push(p),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if out == root || !out.pop() {
                    return Err(format!("{rel} points outside the project"));
                }
            }
            _ => return Err(format!("{rel} is not a path inside the project")),
        }
    }
    confine(root, &out)?;
    Ok(out.to_string_lossy().into_owned())
}

/// A server that came from a project goes into settings that apply everywhere: whatever it runs by
/// a path relative to the project is pinned to that project's file instead of to wherever the next
/// project happens to have one. Returns what was changed.
fn pin_to_project(root: &Path, s: &mut McpServer) -> Res<Vec<Resolved>> {
    let mut done = Vec::new();
    let mut fix = |value: &mut String, always: bool, bare_files: bool| -> Res<()> {
        let (prefix, rel) = match value.split_once('=') {
            Some((p, r)) if value.starts_with('-') => (format!("{p}="), r.to_string()),
            _ => (String::new(), value.clone()),
        };
        let explicit = rel.starts_with("./") || rel.starts_with(".\\") || rel.starts_with("../") || rel.starts_with("..\\") || rel == "." || rel == "..";
        // A bare file name (`node server.js`) is a path too when that file is in the project.
        let bare_file = bare_files && !rel.contains(['/', '\\']) && !rel.starts_with(['-', '@', '$']) && !rel.contains("://") && !rel.is_empty() && root.join(&rel).is_file();
        if (relative_path(&rel) && (always || explicit || root.join(&rel).exists())) || bare_file {
            let to = resolve_in(root, &rel)?;
            done.push(Resolved { from: value.clone(), to: format!("{prefix}{to}") });
            *value = format!("{prefix}{to}");
        }
        Ok(())
    };
    if s.transport == Transport::Stdio {
        fix(&mut s.command, true, false)?;
        for a in s.args.iter_mut() {
            fix(a, false, true)?;
        }
        // A working folder is a place even when it is a bare name.
        let rooted = Path::new(&s.cwd).is_absolute() || s.cwd.starts_with('/') || s.cwd.starts_with('\\');
        if !s.cwd.is_empty() && !rooted {
            let to = resolve_in(root, &s.cwd)?;
            done.push(Resolved { from: s.cwd.clone(), to: to.clone() });
            s.cwd = to;
        }
    }
    Ok(done)
}

fn untrusted_entry(src: &McpServer, prepared: &McpServer, dest: &Target, resolved: Vec<Resolved>) -> Untrusted {
    let list = |all: &[(String, String)], kept: &[(String, String)]| -> Vec<UntrustedValue> {
        all.iter()
            .map(|(k, v)| UntrustedValue {
                key: k.clone(),
                value: v.clone(),
                reference: mcp::has_reference(v),
                var: mcp::reference_of(v).map(str::to_string),
                carried: kept.iter().any(|(kk, _)| kk == k),
            })
            .collect()
    };
    let stdio = prepared.transport == Transport::Stdio;
    Untrusted {
        name: src.name.clone(),
        agent: dest.agent.clone(),
        argv: if stdio { std::iter::once(prepared.command.clone()).chain(prepared.args.iter().cloned()).collect() } else { Vec::new() },
        url: (!stdio).then(|| prepared.url.clone()),
        cwd: (!prepared.cwd.is_empty()).then(|| prepared.cwd.clone()),
        env: list(&src.env, &prepared.env),
        headers: list(&src.headers, &prepared.headers),
        resolved,
        arg_vars: Vec::new(),
    }
}

/// Writes `server` into `dest`'s file (read afresh unless an earlier step of the plan changed it).
fn plan_put(ctx: &Ctx, plan: &mut Plan, dest: &Target, server: &McpServer, include_env: bool, from_scope: Option<Scope>) -> Res<()> {
    let kind = kind_of(dest)?;
    let (path, fkind) = mcp_path(ctx, dest)?;
    valid_name(&server.name, kind == Kind::Codex)?;
    let untrusted_source = from_scope == Some(Scope::Project);
    let mut source = server.clone();
    let mut resolved = Vec::new();
    if untrusted_source && dest.scope == Scope::User {
        let root = ctx.project.as_ref().ok_or("Open a project first")?;
        resolved = pin_to_project(root, &mut source)?;
    }
    // What the command line or URL reads from the user's environment counts like an env entry.
    let arg_vars: Vec<String> = source.args.iter().chain([&source.url]).flat_map(|a| mcp::reference_vars(a)).collect();
    if untrusted_source && dest.scope == Scope::User && !include_env && !arg_vars.is_empty() {
        return Err(format!("“{}” reads your variables {} in its command or URL. Tick the box to copy the project's environment too, or leave it in the project.", server.name, arg_vars.join(", ")));
    }
    let (prepared, notes) = for_destination(&source, kind, dest.scope, include_env, untrusted_source);
    let value = encode(kind, &prepared).map_err(|e| format!("{}: {e}", dest.agent))?;
    let current = match plan.changes.iter().find(|c| c.path == path) {
        Some(c) => c.new.clone(),
        None => read_opt(&path)?,
    };
    let old_on_disk = read_opt(&path)?;
    let (existing, _) = servers_in(kind, fkind, current.as_deref().unwrap_or(""))?;
    let file = file_label(dest);
    if existing.iter().any(|s| s.name == server.name) {
        plan.warnings.push(warn("replaces", &[("file", file.clone()), ("name", server.name.clone())]));
    }
    let new = put_server(kind, fkind, current.as_deref().unwrap_or(""), &server.name, &value)?;
    plan.warnings.extend(notes.into_iter().map(|mut w| {
        w.params.insert("file", file.clone());
        w
    }));
    for r in &resolved {
        plan.warnings.push(warn("relative_resolved", &[("file", file.clone()), ("from", r.from.clone()), ("to", r.to.clone())]));
    }
    if dest.scope == Scope::Project {
        let risky = server.args.iter().chain([&server.url]).any(|a| kelpie_proto::redact(a) != *a);
        if risky {
            plan.warnings.push(warn("secret_in_project_command", &[("file", file.clone())]));
        }
    }
    if untrusted_source && dest.scope == Scope::User {
        plan.needs_confirm = true;
        let mut entry = untrusted_entry(server, &prepared, dest, resolved);
        entry.arg_vars = arg_vars;
        plan.untrusted.push(entry);
    }
    plan.push(Change { path, old: old_on_disk, new: Some(new), kind: fkind });
    Ok(())
}

fn file_label(t: &Target) -> String {
    format!("{} ({})", t.agent, if t.scope == Scope::User { "user" } else { "project" })
}

fn plan_edit(ctx: &Ctx, edit: &Edit) -> Res<Plan> {
    let mut plan = Plan::new();
    match edit {
        Edit::Toggle { target, name, enabled } => plan_toggle(ctx, &mut plan, target, name, *enabled)?,
        Edit::Remove { target, name } => plan_remove(ctx, &mut plan, target, name)?,
        Edit::Add { targets, server } => {
            let transport = match server.transport.as_str() {
                "stdio" => Transport::Stdio,
                "http" => Transport::Http,
                "sse" => Transport::Sse,
                other => return Err(format!("unknown transport {other}")),
            };
            if targets.is_empty() {
                return Err("Choose where to add the server".into());
            }
            let mut s = McpServer::new(server.name.trim(), transport);
            s.command = server.command.trim().to_string();
            s.args = server.args.iter().map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect();
            s.url = server.url.trim().to_string();
            if transport != Transport::Stdio && !(s.url.starts_with("https://") || s.url.starts_with("http://")) {
                return Err("A remote server needs an http(s) URL".into());
            }
            for t in targets {
                plan_put(ctx, &mut plan, t, &s, false, None)?;
            }
        }
        Edit::Copy { from, name, to, include_env, confirm_untrusted } => {
            let (_, servers) = read_servers(ctx, from)?;
            let src = servers.into_iter().find(|s| &s.name == name).ok_or_else(|| format!("no server named {name} in {}", file_label(from)))?;
            if to.is_empty() {
                return Err("Choose where to copy the server".into());
            }
            for t in to {
                if t == from {
                    return Err("That is where the server already is".into());
                }
                plan_put(ctx, &mut plan, t, &src, *include_env, Some(from.scope))?;
            }
            // The confirmation belongs to this edit: it cannot be assumed by a later one.
            if plan.needs_confirm && !confirm_untrusted {
                plan.warnings.insert(0, warn("untrusted_source", &[]));
            }
        }
        Edit::Instructions { gemini } => {
            let (changes, warnings) = guides::unify(ctx, *gemini)?;
            plan.warnings = warnings;
            for c in changes {
                plan.push(c);
            }
        }
        Edit::Restore { backup: id } => {
            let (path, content, root) = backup::load(&ctx.backups, id)?;
            // Inside the project the file belonged to, whichever project is open now.
            if let Some(root) = root.as_ref().or_else(|| ctx.project.as_ref().filter(|r| path.starts_with(r))) {
                confine(root, &path)?;
            }
            let old = read_opt(&path)?;
            let kind = match path.extension().and_then(|e| e.to_str()) {
                Some("json" | "jsonc") => FileKind::Json,
                Some("toml") => FileKind::Toml,
                _ => FileKind::Text,
            };
            plan.warnings.push(warn("restore_note", &[]));
            plan.push(Change { path, old, new: content, kind });
        }
    }
    plan.changes.retain(|c| c.old != c.new);
    Ok(plan)
}

// ---------- preview ----------

#[derive(Serialize, Debug)]
pub struct FileDiff {
    path: String,
    /// Where the path really leads, when a link is in the way (always inside the project for a
    /// project file).
    real_path: Option<String>,
    created: bool,
    removed: bool,
    /// The lines around the change, before and after (secrets masked); lines outside are the same.
    before: Vec<String>,
    after: Vec<String>,
    skipped_before: usize,
    skipped_after: usize,
    truncated: bool,
}

#[derive(Serialize, Debug)]
pub struct Preview {
    /// Hash of the files as read and as they would become: apply refuses when it no longer matches.
    token: String,
    files: Vec<FileDiff>,
    warnings: Vec<Warn>,
    untrusted: Vec<Untrusted>,
    needs_confirm: bool,
    noop: bool,
}

/// The document as shown: credential values found by where they sit in the parsed tree (an
/// environment or header table, a key named like a secret) are replaced, whatever way the file
/// escaped them, then anything shaped like a known token is masked too.
fn masked(kind: FileKind, text: &str) -> String {
    let m = match kind {
        FileKind::Json => formats::json_mask(text, MASK),
        FileKind::Toml => formats::toml_mask(text, MASK),
        FileKind::Text => None,
    };
    kelpie_proto::redact(&m.unwrap_or_else(|| text.to_string()))
}

/// Whether the file, or a folder above it, is a symbolic link or (on Windows) a junction.
fn has_link_in_path(path: &Path) -> bool {
    path.ancestors().any(|p| {
        let Ok(meta) = std::fs::symlink_metadata(p) else { return false };
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            meta.file_type().is_symlink() || meta.file_attributes() & 0x400 != 0
        }
        #[cfg(not(windows))]
        meta.file_type().is_symlink()
    })
}

fn diff_window(c: &Change) -> FileDiff {
    let lines = |t: &Option<String>| -> Vec<String> { t.as_deref().map(|t| masked(c.kind, t).lines().map(str::to_string).collect()).unwrap_or_default() };
    let (a, b) = (lines(&c.old), lines(&c.new));
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    let from = prefix.saturating_sub(CONTEXT_LINES);
    let (end_a, end_b) = ((a.len() - suffix + CONTEXT_LINES).min(a.len()), (b.len() - suffix + CONTEXT_LINES).min(b.len()));
    let cut = |v: &[String], end: usize| -> (Vec<String>, bool) {
        let w = &v[from.min(end)..end];
        (w.iter().take(MAX_DIFF_LINES).cloned().collect(), w.len() > MAX_DIFF_LINES)
    };
    let ((before, t1), (after, t2)) = (cut(&a, end_a), cut(&b, end_b));
    // Shown only when a link or junction is really in the way (not for a short or differently cased name).
    let real_path = if has_link_in_path(&c.path) { std::fs::canonicalize(&c.path).ok().map(|r| r.to_string_lossy().trim_start_matches(r"\\?\").to_string()) } else { None };
    FileDiff {
        path: c.path.to_string_lossy().into_owned(),
        real_path,
        created: c.old.is_none(),
        removed: c.new.is_none(),
        before,
        after,
        skipped_before: from,
        skipped_after: b.len() - end_b,
        truncated: t1 || t2,
    }
}

pub fn preview(ctx: &Ctx, edit: &Edit) -> Res<Preview> {
    let plan = plan_edit(ctx, edit)?;
    Ok(Preview {
        token: plan.token(),
        files: plan.changes.iter().map(diff_window).collect(),
        noop: plan.changes.is_empty(),
        needs_confirm: plan.needs_confirm,
        warnings: plan.warnings,
        untrusted: plan.untrusted,
    })
}

// ---------- apply ----------

#[derive(Serialize, Debug)]
pub struct Applied {
    files: Vec<AppliedFile>,
}

#[derive(Serialize, Debug)]
pub struct AppliedFile {
    path: String,
    /// The copy of the file as it was (restore from Backups).
    backup: String,
}

/// One apply at a time in this process.
static APPLY_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

#[cfg(test)]
type PathHook = Box<dyn Fn(&Path)>;

#[cfg(test)]
#[derive(Default)]
pub(crate) struct Hooks {
    /// Runs inside the lock, just before the file is compared with the preview.
    pub before_compare: Option<PathHook>,
    /// Runs right after a file was written, before it is checked.
    pub after_write: Option<PathHook>,
    /// Age at which another program's lock counts as abandoned.
    pub lock_stale: Option<std::time::Duration>,
}

#[cfg(test)]
thread_local! {
    pub(crate) static HOOKS: std::cell::RefCell<Hooks> = std::cell::RefCell::new(Hooks::default());
}

fn hook_before_compare(_path: &Path) {
    #[cfg(test)]
    HOOKS.with(|h| h.borrow().before_compare.as_ref().map(|f| f(_path)));
}

fn hook_after_write(_path: &Path) {
    #[cfg(test)]
    HOOKS.with(|h| h.borrow().after_write.as_ref().map(|f| f(_path)));
}

/// How old another program's lock must be before it is taken for abandoned (the lock library
/// Claude Code uses refreshes a held lock every few seconds and gives up on one older than ten).
fn lock_stale() -> std::time::Duration {
    #[cfg(test)]
    if let Some(d) = HOOKS.with(|h| h.borrow().lock_stale) {
        return d;
    }
    std::time::Duration::from_secs(10)
}

fn lock_wait() -> std::time::Duration {
    if cfg!(test) { std::time::Duration::from_millis(400) } else { std::time::Duration::from_secs(5) }
}

/// Claude Code saves `~/.claude.json` under a lock directory next to it (`<file>.lock`, made with
/// `mkdir`), reading the file again once it holds it. Kelpie takes the same lock around its own
/// read, compare, write and check, so neither side saves over the other.
struct ClaudeLock(PathBuf);

/// Where Claude keeps the lock: the config path as it computes it, with `.lock` added, not resolved
/// through links (a linked config is locked under the link's name by both programs).
fn claude_lock_dir(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".lock");
    PathBuf::from(name)
}

impl ClaudeLock {
    fn acquire(file: &Path) -> Res<ClaudeLock> {
        let dir = claude_lock_dir(file);
        let give_up = std::time::Instant::now() + lock_wait();
        loop {
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(ClaudeLock(dir)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let age = std::fs::metadata(&dir).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
                    if age.is_some_and(|a| a >= lock_stale()) {
                        let _ = std::fs::remove_dir(&dir);
                        continue;
                    }
                }
                Err(e) => return Err(format!("{}: {e}", dir.display())),
            }
            if std::time::Instant::now() >= give_up {
                return Err(format!("Claude Code is saving {} right now. Try again in a moment.", file.display()));
            }
            std::thread::sleep(std::time::Duration::from_millis(40));
        }
    }
}

impl Drop for ClaudeLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn clear_readonly(path: &Path) {
    if let Ok(meta) = std::fs::metadata(path) {
        let mut p = meta.permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        p.set_readonly(false);
        let _ = std::fs::set_permissions(path, p);
    }
}

/// Writes `text` next to `path` under a new name (private to the user, flushed to disk) and swaps
/// it in. When the swap fails because another program holds the file, it is tried again a few times
/// and then refused: the file is never rewritten in place, where a crash would leave half of it.
fn write_atomic(path: &Path, text: &str) -> Res<()> {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU32, Ordering};
    static N: AtomicU32 = AtomicU32::new(0);
    #[cfg(test)]
    assert!(path.starts_with(std::env::temp_dir()), "a test tried to write outside the temp folder: {}", path.display());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Through a link: a dotfiles repository's symlink stays a link.
    let target = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = target.with_file_name(format!(".{name}.{}-{}.kelpie-tmp", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let written = opts.open(&tmp).and_then(|mut f| f.write_all(text.as_bytes()).and_then(|_| f.sync_all()));
    if let Err(e) = written {
        clear_readonly(&tmp);
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("{}: {e}", path.display()));
    }
    #[cfg(unix)]
    if let Ok(meta) = std::fs::metadata(&target) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    let mut last = String::new();
    for attempt in 0..8u64 {
        match std::fs::rename(&tmp, &target) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(std::time::Duration::from_millis(25 * (attempt + 1)));
    }
    clear_readonly(&tmp);
    let _ = std::fs::remove_file(&tmp);
    Err(format!("{} is in use by another program: nothing was written ({last})", path.display()))
}

fn put_back(path: &Path, old: &Option<String>) -> Res<()> {
    match old {
        Some(text) => write_atomic(path, text),
        None => match std::fs::remove_file(path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
            _ => Ok(()),
        },
    }
}

/// Why an apply stopped.
enum Failure {
    /// What Kelpie wrote is not a document the agent can read (or could not be written).
    Mine(String),
    /// The file held something else right after the write: another program saved it meanwhile.
    Foreign(String),
    /// The file was not as previewed when its turn came.
    Stale,
    Other(String),
}

/// A file Kelpie wrote: what was there before and what it put there.
struct Written<'a> {
    change: &'a Change,
    text: Option<String>,
    backup: String,
}

/// What is on disk is what Kelpie wrote, and still a document its agent can read. Anything else on
/// disk is somebody else's write and is not judged here.
fn verify(c: &Change, written: &Option<String>) -> Result<(), Failure> {
    let on_disk = read_opt(&c.path).map_err(Failure::Other)?;
    if &on_disk != written {
        return Err(Failure::Foreign(c.path.to_string_lossy().into_owned()));
    }
    if let Some(now) = &on_disk {
        let bad = |e: String| Failure::Mine(format!("{} no longer parses: {e}", c.path.display()));
        match c.kind {
            FileKind::Json => {
                json_check(now).map_err(bad)?;
                // Strict stays strict: no comma or comment an agent's own reader would reject.
                if c.old.as_deref().is_none_or(json_is_strict) && !json_is_strict(now) {
                    return Err(Failure::Mine(format!("{} would no longer be plain JSON", c.path.display())));
                }
            }
            FileKind::Toml => toml_check(now).map_err(bad)?,
            FileKind::Text => {}
        }
    }
    Ok(())
}

/// Puts back the files Kelpie wrote, newest first, but only those that still hold what Kelpie put
/// there: a file another program saved since is left alone. Returns (could not restore, left alone).
fn undo(ctx: &Ctx, done: &[Written]) -> (Vec<String>, Vec<String>) {
    let (mut failed, mut left) = (Vec::new(), Vec::new());
    for w in done.iter().rev() {
        let _lock = if w.change.path == ctx.roots.claude_json { ClaudeLock::acquire(&w.change.path).ok() } else { None };
        match read_opt(&w.change.path) {
            Ok(now) if now == w.text => {
                if let Err(e) = put_back(&w.change.path, &w.change.old) {
                    failed.push(e);
                }
            }
            Ok(_) => left.push(format!("{} (backup {})", w.change.path.display(), w.backup)),
            Err(e) => failed.push(e),
        }
    }
    (failed, left)
}

pub fn apply(ctx: &Ctx, edit: &Edit, token: &str) -> Res<Applied> {
    if !ctx.roots.writable {
        return Err("This run has no fixture config folder (set KELPIE_AGENT_CONFIG_HOME): writes are off.".into());
    }
    let _one_at_a_time = APPLY_LOCK.lock();
    let plan = plan_edit(ctx, edit)?;
    if plan.token() != token {
        return Err("The files changed since the preview. Review the change again.".into());
    }
    if plan.needs_confirm && !matches!(edit, Edit::Copy { confirm_untrusted: true, .. }) {
        return Err("This copies a project server to your user settings: confirm it first.".into());
    }
    let mut done: Vec<Written> = Vec::new();
    for c in &plan.changes {
        // Claude's own lock around read, compare, write and check, when it is Claude's file.
        let step = (|| {
            let _lock = if c.path == ctx.roots.claude_json { Some(ClaudeLock::acquire(&c.path).map_err(Failure::Other)?) } else { None };
            write_one(ctx, c, &mut done)
        })();
        if let Err(f) = step {
            let (failed, left) = undo(ctx, &done);
            let mut msg = match f {
                Failure::Stale => "The files changed since the preview. Review the change again.".to_string(),
                Failure::Mine(e) | Failure::Other(e) => e,
                Failure::Foreign(p) => format!("{p} was changed by another program right after Kelpie wrote it. Check that your change is still there"),
            };
            if !left.is_empty() {
                msg.push_str(&format!(". Left as another program saved it: {}", left.join("; ")));
            }
            msg.push_str(&if !failed.is_empty() {
                format!(". Putting files back also failed ({}): restore them from Backups.", failed.join("; "))
            } else if done.len() > left.len() {
                ". The files Kelpie had written are back as they were.".to_string()
            } else if left.is_empty() {
                ". Nothing was changed by Kelpie.".to_string()
            } else {
                String::new()
            });
            return Err(msg);
        }
    }
    Ok(Applied { files: done.iter().map(|w| AppliedFile { path: w.change.path.to_string_lossy().into_owned(), backup: w.backup.clone() }).collect() })
}

/// One file of an apply, with its lock held: compare with the preview, back up, write, check.
fn write_one<'a>(ctx: &Ctx, c: &'a Change, done: &mut Vec<Written<'a>>) -> Result<(), Failure> {
    hook_before_compare(&c.path);
    // Right before the swap: the file must still be what the preview was made from.
    if read_opt(&c.path).map_err(Failure::Other)? != c.old {
        return Err(Failure::Stale);
    }
    let root = ctx.project.as_ref().filter(|r| c.path.starts_with(r));
    // Looked at again now that the file is about to be written: a folder swapped for a link meanwhile is refused.
    if let Some(root) = root {
        confine(root, &c.path).map_err(Failure::Other)?;
    }
    let backup = backup::save(&ctx.backups, &c.path, c.old.as_deref(), root.map(|r| r.as_path())).map_err(Failure::Other)?;
    let mut text = c.new.clone();
    if let Some(t) = text.as_mut() {
        if ctx.corrupt_marker.as_ref().is_some_and(|m| std::fs::remove_file(m).is_ok()) {
            t.push_str("\n}{ corrupted");
        }
    }
    let wrote = match &text {
        Some(t) => write_atomic(&c.path, t),
        None => put_back(&c.path, &None),
    };
    // It counts as written (and is undone with the rest) only once something may have landed.
    wrote.map_err(Failure::Other)?;
    done.push(Written { change: c, text: text.clone(), backup });
    hook_after_write(&c.path);
    verify(c, &text)
}

// ---------- commands ----------

/// What the setup page shows, read once when it opens.
#[tauri::command]
pub async fn agent_setup_overview(project: Option<String>) -> CmdResult<Overview> {
    crate::fs_ops::blocking(move || Ok(overview(&Ctx::from_env(project.as_deref())))).await
}

#[tauri::command]
pub async fn agent_setup_preview(project: Option<String>, edit: Edit) -> CmdResult<Preview> {
    crate::fs_ops::blocking(move || preview(&Ctx::from_env(project.as_deref()), &edit)).await
}

#[tauri::command]
pub async fn agent_setup_apply(project: Option<String>, edit: Edit, token: String) -> CmdResult<Applied> {
    crate::fs_ops::blocking(move || apply(&Ctx::from_env(project.as_deref()), &edit, &token)).await
}

#[tauri::command]
pub async fn agent_setup_backups() -> CmdResult<Vec<backup::BackupInfo>> {
    crate::fs_ops::blocking(|| Ok(backup::list(&Ctx::from_env(None).backups).into_iter().take(60).collect())).await
}

/// Whether `path` is one of the places this page deals with (an agent's config folder,
/// `~/.claude.json`, the open project, Kelpie's backups), with no `..` in it.
fn revealable(ctx: &Ctx, path: &str) -> bool {
    let r = &ctx.roots;
    let cursor = r.home.join(".cursor");
    let allowed = [Some(&r.claude_dir), Some(&r.codex_home), Some(&r.gemini_dir), Some(&r.opencode_dir), Some(&cursor), ctx.project.as_ref(), Some(&ctx.backups)];
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    let wanted = norm(Path::new(path));
    let inside = allowed.iter().flatten().any(|base| wanted == norm(base) || wanted.starts_with(&format!("{}/", norm(base))));
    !Path::new(path).components().any(|c| matches!(c, std::path::Component::ParentDir)) && (inside || wanted == norm(&r.claude_json))
}

/// Shows a file of the setup page in the system file manager. Only files under an agent's config
/// folders, `~/.claude.json`, the open project or Kelpie's backups.
#[tauri::command]
pub async fn agent_setup_reveal(project: Option<String>, path: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        let ctx = Ctx::from_env(project.as_deref());
        if !revealable(&ctx, &path) {
            return Err("that file is not one of the agents' config files".into());
        }
        let p = PathBuf::from(&path);
        let shown = if p.exists() { p } else { p.parent().map(Path::to_path_buf).unwrap_or(p) };
        #[cfg(windows)]
        let result = {
            use std::os::windows::process::CommandExt;
            let shown = shown.to_string_lossy().replace('/', "\\");
            if shown.contains('"') {
                return Err("that path cannot be shown".into());
            }
            // The path is quoted whole: a comma or a space in a folder name must not split it.
            std::process::Command::new("explorer").raw_arg(format!("/select,\"{shown}\"")).spawn()
        };
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("open").arg("-R").arg(&shown).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let result = std::process::Command::new("xdg-open").arg(if shown.is_dir() { shown.clone() } else { shown.parent().map(Path::to_path_buf).unwrap_or(shown.clone()) }).spawn();
        result.map(|_| ()).map_err(err)
    })
    .await
}

/// The ACP registry, fetched on a click. The address is fixed outside end-to-end runs.
#[tauri::command]
pub async fn agent_registry_fetch() -> CmdResult<Vec<registry::RegistryAgent>> {
    crate::fs_ops::blocking(|| {
        let (url, allow_http) = registry::registry_url();
        registry::fetch_agents(&url, allow_http)
    })
    .await
}

/// Sets a top-level string key of a Codex `config.toml`, keeping comments, order and every table
/// (accounts keep their login in a file this way).
pub fn toml_set_top_level(text: &str, key: &str, value: &str) -> Res<String> {
    formats::toml_set_top_level(text, key, value)
}
