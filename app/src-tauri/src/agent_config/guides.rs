//! Instruction files (CLAUDE.md, AGENTS.md, GEMINI.md…), "one file for all agents", and the
//! read-only list of skills, commands and subagents the agents find in their folders.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::formats::{json_at, json_read, json_set};
use super::mcp::{warn, Warn};
use super::{confine, read_opt, same_file, Change, Ctx, FileKind, Scope};

type Res<T> = Result<T, String>;

const IMPORT: &str = "@AGENTS.md";
const MAX_ITEMS: usize = 600;
const MAX_PER_SOURCE: usize = 200;
const HEAD_BYTES: usize = 4096;

// ---------- instruction files ----------

#[derive(Serialize, Debug)]
pub struct InstructionFile {
    /// claude | agents | gemini | copilot | kiro
    pub agent: &'static str,
    pub scope: Scope,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
}

#[derive(Serialize, Debug, Default)]
pub struct Instructions {
    pub files: Vec<InstructionFile>,
    /// CLAUDE.md pulls AGENTS.md in with `@AGENTS.md`.
    pub claude_imports_agents: bool,
    /// Gemini's `contextFileName` lists AGENTS.md.
    pub gemini_reads_agents: bool,
}

fn has_import(text: &str) -> bool {
    text.lines().any(|l| l.trim() == IMPORT)
}

fn file(agent: &'static str, scope: Scope, path: PathBuf) -> InstructionFile {
    let meta = std::fs::metadata(&path).ok().filter(|m| m.is_file());
    InstructionFile { agent, scope, path: path.to_string_lossy().into_owned(), exists: meta.is_some(), bytes: meta.map_or(0, |m| m.len()) }
}

/// Where Gemini's list of context file names lives in a settings document.
fn gemini_names_path(doc: &Value) -> (&'static [&'static str], &'static str) {
    if doc.get("contextFileName").is_some() { (&[], "contextFileName") } else { (&["context"], "fileName") }
}

fn gemini_names(doc: &Value) -> Vec<String> {
    let (path, key) = gemini_names_path(doc);
    match json_at(doc, path).and_then(|o| o.get(key)) {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
        _ => Vec::new(),
    }
}

pub fn instructions(ctx: &Ctx) -> Instructions {
    let r = &ctx.roots;
    let mut files = vec![
        file("claude", Scope::User, r.claude_dir.join("CLAUDE.md")),
        file("agents", Scope::User, r.codex_home.join("AGENTS.md")),
        file("gemini", Scope::User, r.gemini_dir.join("GEMINI.md")),
    ];
    let mut out = Instructions::default();
    if let Some(p) = &ctx.project {
        files.push(file("claude", Scope::Project, p.join("CLAUDE.md")));
        files.push(file("agents", Scope::Project, p.join("AGENTS.md")));
        files.push(file("gemini", Scope::Project, p.join("GEMINI.md")));
        files.push(file("copilot", Scope::Project, p.join(".github").join("copilot-instructions.md")));
        if let Ok(rd) = std::fs::read_dir(p.join(".kiro").join("steering")) {
            let mut steering: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|f| f.extension().is_some_and(|x| x == "md")).collect();
            steering.sort();
            files.extend(steering.into_iter().take(50).map(|f| file("kiro", Scope::Project, f)));
        }
        // A file that links out of the project is not read.
        let inside = |f: &Path| confine(p, f).is_ok();
        let claude = p.join("CLAUDE.md");
        let settings = p.join(".gemini").join("settings.json");
        out.claude_imports_agents = inside(&claude) && (same_file(&claude, &p.join("AGENTS.md")) || read_opt(&claude).ok().flatten().is_some_and(|t| has_import(&t)));
        out.gemini_reads_agents = inside(&settings) && read_opt(&settings).ok().flatten().and_then(|t| json_read(&t).ok()).is_some_and(|d| gemini_names(&d).iter().any(|n| n == "AGENTS.md"));
    }
    out.files = files;
    out
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GeminiMode {
    /// `contextFileName` in `.gemini/settings.json` lists AGENTS.md.
    #[default]
    Setting,
    /// GEMINI.md becomes a copy of AGENTS.md (it drifts when AGENTS.md changes).
    Copy,
}

fn change(path: PathBuf, old: Option<String>, new: String, kind: FileKind) -> Change {
    Change { path, old, new: Some(new), kind }
}

/// The text without its `@AGENTS.md` lines (a file that imports AGENTS.md must not move that line into it).
fn without_import(text: &str) -> String {
    let kept: Vec<&str> = text.lines().filter(|l| l.trim() != IMPORT).collect();
    if kept.is_empty() { String::new() } else { format!("{}\n", kept.join("\n")) }
}

/// AGENTS.md as the one source: CLAUDE.md imports it, Gemini lists it (or gets a copy). Nothing
/// is deleted: a CLAUDE.md with text of its own keeps it, and its text moves to AGENTS.md only
/// when there is no AGENTS.md yet. Files are only read and written inside the project: a link
/// that leads out is refused (its real target is named), and two names for one file, or two
/// files with the same text, are already one set of instructions.
pub fn unify(ctx: &Ctx, gemini: GeminiMode) -> Res<(Vec<Change>, Vec<Warn>)> {
    let root = ctx.project.as_ref().ok_or("Open a project first: this sets up the project's instruction files")?;
    let (agents_path, claude_path, gemini_path) = (root.join("AGENTS.md"), root.join("CLAUDE.md"), root.join("GEMINI.md"));
    let settings_path = root.join(".gemini").join("settings.json");
    for f in [&agents_path, &claude_path, &gemini_path, &settings_path] {
        confine(root, f)?;
    }
    let (agents, claude) = (read_opt(&agents_path)?, read_opt(&claude_path)?);
    let mut warnings = Vec::new();
    let mut out = Vec::new();
    let import = format!("{IMPORT}\n");
    let meaningful = |t: &Option<String>| t.as_deref().is_some_and(|t| !t.trim().is_empty() && !t.lines().all(|l| l.trim().is_empty() || l.trim() == IMPORT));

    let linked = same_file(&agents_path, &claude_path);
    let same_text = agents.as_deref().is_some_and(|a| !a.trim().is_empty() && Some(a) == claude.as_deref());
    let mut source = agents.clone().filter(|t| !t.trim().is_empty());
    if linked || same_text {
        // Already one set of instructions: CLAUDE.md and AGENTS.md are not touched.
        warnings.push(warn(if linked { "already_linked" } else { "already_same_text" }, &[]));
    } else {
        let mut new_claude: Option<String> = None;
        match (&source, meaningful(&claude)) {
            // No AGENTS.md: CLAUDE.md's text becomes it, and CLAUDE.md just imports it.
            (None, true) => {
                source = claude.as_deref().map(without_import);
                new_claude = Some(import.clone());
            }
            (None, false) => {
                source = Some("# AGENTS.md\n\nInstructions for every coding agent working in this repository.\n".into());
                if claude.as_deref().is_none_or(|t| t.trim().is_empty()) {
                    new_claude = Some(import.clone());
                }
            }
            (Some(_), true) => {
                if !claude.as_deref().is_some_and(has_import) {
                    let mut t = claude.clone().unwrap_or_default();
                    if !t.ends_with('\n') {
                        t.push('\n');
                    }
                    new_claude = Some(format!("{t}\n{import}"));
                    warnings.push(warn("claude_has_text", &[]));
                }
            }
            (Some(_), false) => {
                if !claude.as_deref().is_some_and(has_import) {
                    new_claude = Some(import.clone());
                }
            }
        }
        let text = source.clone().unwrap_or_default();
        if agents.as_deref() != Some(text.as_str()) {
            out.push(change(agents_path, agents, text, FileKind::Text));
        }
        if let Some(text) = new_claude {
            if claude.as_deref() != Some(text.as_str()) {
                out.push(change(claude_path, claude, text, FileKind::Text));
            }
        }
    }
    let source = source.unwrap_or_default();
    match gemini {
        GeminiMode::Setting => {
            let old = read_opt(&settings_path)?;
            let doc = json_read(old.as_deref().unwrap_or(""))?;
            let mut names = gemini_names(&doc);
            if !names.iter().any(|n| n == "AGENTS.md") {
                if names.is_empty() {
                    // Naming any file replaces Gemini's default, so keep GEMINI.md in the list.
                    names.push("GEMINI.md".into());
                }
                names.insert(0, "AGENTS.md".into());
                let (parent, key) = gemini_names_path(&doc);
                let new = json_set(old.as_deref().unwrap_or(""), parent, key, &Value::Array(names.into_iter().map(Value::String).collect()))?;
                out.push(change(settings_path, old, new, FileKind::Json));
            }
        }
        GeminiMode::Copy => {
            let old = read_opt(&gemini_path)?;
            match old {
                None => out.push(change(gemini_path, None, source, FileKind::Text)),
                Some(t) if t == source => {}
                Some(_) => warnings.push(warn("gemini_exists", &[])),
            }
        }
    }
    Ok((out, warnings))
}

// ---------- skills, commands and subagents ----------

#[derive(Serialize, Debug, PartialEq)]
pub struct SkillItem {
    /// skill | command | subagent
    pub kind: &'static str,
    pub agent: &'static str,
    pub scope: Scope,
    pub name: String,
    pub description: String,
    pub path: String,
}

#[derive(Clone, Copy)]
enum Layout {
    /// `<dir>/<name>/SKILL.md`
    SkillDirs,
    /// Files with these extensions, subfolders included.
    Files(&'static [&'static str]),
}

struct Source {
    kind: &'static str,
    agent: &'static str,
    scope: Scope,
    dir: PathBuf,
    layout: Layout,
}

fn sources(ctx: &Ctx) -> Vec<Source> {
    let r = &ctx.roots;
    let mut out = Vec::new();
    let mut add = |agent, scope, base: &Path, rels: &[(&'static str, &str, Layout)]| {
        for &(kind, rel, layout) in rels {
            out.push(Source { kind, agent, scope, dir: base.join(rel), layout });
        }
    };
    const MD: Layout = Layout::Files(&["md"]);
    let claude = [("skill", "skills", Layout::SkillDirs), ("command", "commands", MD), ("subagent", "agents", MD)];
    add("claude", Scope::User, &r.claude_dir, &claude);
    add("codex", Scope::User, &r.codex_home, &[("skill", "skills", Layout::SkillDirs), ("command", "prompts", MD)]);
    add("gemini", Scope::User, &r.gemini_dir, &[("skill", "skills", Layout::SkillDirs), ("command", "commands", Layout::Files(&["toml"]))]);
    add("cursor", Scope::User, &r.home.join(".cursor"), &[("command", "commands", MD)]);
    let oc: [(&'static str, &str, Layout); 6] = [
        ("skill", "skill", Layout::SkillDirs),
        ("skill", "skills", Layout::SkillDirs),
        ("command", "command", MD),
        ("command", "commands", MD),
        ("subagent", "agent", MD),
        ("subagent", "agents", MD),
    ];
    add("opencode", Scope::User, &r.opencode_dir, &oc);
    if let Some(p) = &ctx.project {
        add("claude", Scope::Project, &p.join(".claude"), &claude);
        add("codex", Scope::Project, &p.join(".codex"), &[("skill", "skills", Layout::SkillDirs)]);
        add("codex", Scope::Project, p, &[("skill", ".agents/skills", Layout::SkillDirs)]);
        add("gemini", Scope::Project, &p.join(".gemini"), &[("skill", "skills", Layout::SkillDirs), ("command", "commands", Layout::Files(&["toml"]))]);
        add("cursor", Scope::Project, &p.join(".cursor"), &[("command", "commands", MD)]);
        add("opencode", Scope::Project, &p.join(".opencode"), &oc);
    }
    out
}

/// `description` from a YAML front matter block or a TOML key, in the first few KB of a file.
fn describe(path: &Path) -> String {
    use std::io::Read;
    let mut buf = vec![0u8; HEAD_BYTES];
    let n = std::fs::File::open(path).and_then(|mut f| f.read(&mut buf)).unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    let mut lines = head.lines();
    while let Some(line) = lines.next() {
        let l = line.trim();
        let rest = l.strip_prefix("description:").or_else(|| l.strip_prefix("description =").or_else(|| l.strip_prefix("description=")));
        let Some(rest) = rest else { continue };
        let mut v = rest.trim().to_string();
        if matches!(v.as_str(), ">" | "|" | ">-" | "|-" | "") {
            v = lines.by_ref().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").to_string();
        }
        let v = v.trim_matches(|c| c == '"' || c == '\'');
        return v.chars().take(200).collect();
    }
    String::new()
}

fn scan_files(base: &Path, dir: &Path, exts: &[&str], depth: u8, out: &mut Vec<(String, PathBuf)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= MAX_PER_SOURCE {
            return;
        }
        let path = e.path();
        let Ok(t) = e.file_type() else { continue };
        if t.is_dir() && depth < 3 {
            scan_files(base, &path, exts, depth + 1, out);
        } else if t.is_file() && path.extension().and_then(|x| x.to_str()).is_some_and(|x| exts.contains(&x)) {
            let rel = path.strip_prefix(base).unwrap_or(&path).with_extension("");
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

/// Everything the agents would find, as the user sees it in each agent's own folders. Read-only.
pub fn skills(ctx: &Ctx) -> Vec<SkillItem> {
    let mut out: Vec<SkillItem> = Vec::new();
    for s in sources(ctx) {
        if out.len() >= MAX_ITEMS {
            break;
        }
        let mut found: Vec<(String, PathBuf)> = Vec::new();
        match &s.layout {
            Layout::SkillDirs => {
                if let Ok(rd) = std::fs::read_dir(&s.dir) {
                    let mut dirs: Vec<_> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).collect();
                    dirs.sort_by_key(|e| e.file_name());
                    found = dirs.into_iter().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().join("SKILL.md"))).filter(|(_, f)| f.is_file()).take(MAX_PER_SOURCE).collect();
                }
            }
            Layout::Files(exts) => scan_files(&s.dir, &s.dir, exts, 0, &mut found),
        }
        for (name, path) in found {
            out.push(SkillItem { kind: s.kind, agent: s.agent, scope: s.scope, description: String::new(), name, path: path.to_string_lossy().into_owned() });
        }
    }
    out.truncate(MAX_ITEMS);
    // Each description is a file open and read: do them side by side.
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    let chunk = out.len().div_ceil(workers).max(1);
    std::thread::scope(|scope| {
        for part in out.chunks_mut(chunk) {
            scope.spawn(move || {
                for item in part {
                    item.description = describe(Path::new(&item.path));
                }
            });
        }
    });
    out
}
