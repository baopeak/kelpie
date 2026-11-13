//! What a phone may start (`new_options`, `new_session`): the launch catalogue the app resolves and pushes with `mobile.set_launch`.
//!
//! The app owns the agent profiles, the permission mode, the accounts and the hidden agents, so it works out each agent's arguments and
//! environment exactly as its own launchers would, and the daemon keeps the result. A phone then starts a session with those and nothing
//! else, also while the window is closed. The catalogue lives in memory only, like the project names: profile environments can hold API
//! keys, and none of them is written again or logged. A daemon that has not heard from the app since it started has none, and a phone is
//! told to open an up-to-date Kelpie on the computer. A catalogue the daemon refuses clears the one it held: a phone must never start with
//! settings the person has since changed (Yolo turned off, say), so until the app sends one that is accepted, phones can start nothing.
//!
//! Remote (SSH) projects and chat-only agents are never in it: the first need the app's saved secrets and a dialog, the second the app's
//! chat front-end.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;
use serde_json::{json, Value};

/// Most projects, agents and arguments of one agent a catalogue holds, and the most a `mobile.set_launch` may weigh.
pub const MAX_PROJECTS: usize = 500;
pub const MAX_AGENTS: usize = 64;
pub const MAX_ARGS: usize = 64;
pub const MAX_MESSAGE: usize = 256 * 1024;

/// What the phone is told while there is no catalogue. An app from before phones could start sessions sends none either, hence "up-to-date".
pub const NOT_READY: &str = "Open an up-to-date Kelpie on your computer so this phone can start sessions.";
/// What the phone is told after the app sent a catalogue the daemon refused (the app says why on the computer).
pub const REFUSED: &str = "Kelpie on your computer could not offer its projects to this phone. Open Kelpie there to see why.";

/// Why there is no catalogue to start from.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Missing {
    /// The app has not sent one since this daemon started.
    NotSent,
    /// The last one the app sent was refused, and the one before it dropped.
    Refused,
}

impl Missing {
    /// The refusal a phone gets: (code, message).
    pub fn refusal(self) -> (&'static str, String) {
        match self {
            Missing::NotSent => ("not_ready", NOT_READY.to_string()),
            Missing::Refused => ("launch_refused", REFUSED.to_string()),
        }
    }
}

/// Most bytes of a project's id, name and folder, and of an agent's kind and label. The app trims to these before it sends
/// (`app/src/lib/phone-launch.ts` keeps the same numbers).
pub const MAX_PROJECT_ID: usize = 128;
pub const MAX_PROJECT_NAME: usize = 256;
pub const MAX_PATH: usize = 4096;
pub const MAX_KIND: usize = 64;
pub const MAX_LABEL: usize = 128;
/// Most bytes of a name in an agent's environment. A name is never empty and holds neither `=` nor a control character (NUL among them):
/// the operating system could not set it, or would read it as another variable.
pub const MAX_ENV_KEY: usize = 128;

#[derive(Deserialize, Clone, PartialEq, Debug)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// The folder, as the app has it. Never sent to a phone.
    pub path: String,
}

#[derive(Deserialize, Clone, PartialEq)]
pub struct Agent {
    pub kind: String,
    pub label: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// The environment's names only: its values can be keys, and nothing that prints an agent may show them.
impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent").field("kind", &self.kind).field("label", &self.label).field("args", &self.args).field("env", &self.env.keys().collect::<Vec<_>>()).finish()
    }
}

#[derive(Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Catalogue {
    pub projects: Vec<Project>,
    pub agents: Vec<Agent>,
}

/// `mobile.set_launch`'s parameters as a whole catalogue, checked. What it says when refused never quotes what was sent.
pub fn parse(params: &Value) -> Result<Catalogue, String> {
    let size = serde_json::to_vec(params).map(|b| b.len()).unwrap_or(usize::MAX);
    if size > MAX_MESSAGE {
        return Err(format!("the launch catalogue is too large ({size} bytes; at most {MAX_MESSAGE})"));
    }
    let c: Catalogue = serde_json::from_value(params.clone()).map_err(|_| "mobile.set_launch takes {projects: [{id, name, path}], agents: [{kind, label, args, env}]}".to_string())?;
    if c.projects.len() > MAX_PROJECTS {
        return Err(format!("at most {MAX_PROJECTS} projects"));
    }
    if c.agents.len() > MAX_AGENTS {
        return Err(format!("at most {MAX_AGENTS} agents"));
    }
    let text = |s: &str, most: usize| s.len() <= most && !s.chars().any(char::is_control);
    let plain = |s: &str, most: usize| !s.is_empty() && text(s, most);
    for p in &c.projects {
        if !plain(&p.id, MAX_PROJECT_ID) || !text(&p.name, MAX_PROJECT_NAME) || !plain(&p.path, MAX_PATH) {
            return Err(format!("each project needs an id (at most {MAX_PROJECT_ID} bytes), a name (at most {MAX_PROJECT_NAME}) and a folder, without control characters"));
        }
    }
    for a in &c.agents {
        if !plain(&a.kind, MAX_KIND) || !text(&a.label, MAX_LABEL) {
            return Err(format!("each agent needs a kind (at most {MAX_KIND} bytes) and a label (at most {MAX_LABEL}), without control characters"));
        }
        if a.args.len() > MAX_ARGS {
            return Err(format!("at most {MAX_ARGS} arguments for one agent"));
        }
        // Values are passed as they are; what is refused is never quoted.
        if !a.env.keys().all(|k| plain(k, MAX_ENV_KEY) && !k.contains('=')) {
            return Err(format!("each name in an agent's environment needs at most {MAX_ENV_KEY} bytes, without \"=\" or control characters"));
        }
    }
    Ok(c)
}

/// The phone's `new_options`: the projects in the catalogue's order (their folders stay here), and the catalogue's agents that are
/// installed, each saying whether it runs without asking before it acts (as `sub_rpc::skips_permissions` reads a session's arguments).
/// While none was sent it is an answer that says so (`ready: false`, as phones before a refusal code know it); after one was refused, a
/// refusal (code, message).
pub fn options(c: Result<&Catalogue, Missing>, installed: &[&str]) -> Result<Value, (&'static str, String)> {
    let c = match c {
        Ok(c) => c,
        Err(Missing::NotSent) => return Ok(json!({"type": "new_options", "ready": false, "projects": [], "agents": [], "why": NOT_READY})),
        Err(refused) => return Err(refused.refusal()),
    };
    let projects: Vec<Value> = c.projects.iter().map(|p| json!({"id": p.id, "name": p.name})).collect();
    let agents: Vec<Value> = c
        .agents
        .iter()
        .filter(|a| installed.contains(&a.kind.as_str()))
        .map(|a| {
            let yolo = crate::agents::preset_for(&a.kind).map_or(&[][..], |p| p.yolo);
            json!({"kind": a.kind, "label": a.label, "skips": crate::subs::args_skip_permissions(&a.kind, yolo, &a.args)})
        })
        .collect();
    Ok(json!({"type": "new_options", "ready": true, "projects": projects, "agents": agents}))
}

/// A `new_session` as checked against the catalogue, before what needs the disk (the agent installed, the folder there).
pub struct Planned {
    pub project: Project,
    pub agent: Agent,
    /// The first prompt, cleaned as anything typed into an agent is; `None` when there is none.
    pub prompt: Option<String>,
}

impl Planned {
    /// What it is started with: the catalogue's arguments and environment, nothing of the phone's but the prompt, at the size the desktop's
    /// launchers use, in the project's folder as the app gives it. That is the folder the desktop starts in too: a canonical one (a mapped
    /// drive resolved to its share, a symlink to its target) would be another folder to the agents, which key their history and trust by it.
    pub fn create_params(&self) -> Value {
        json!({
            "kind": self.agent.kind, "cwd": self.project.path, "project_id": self.project.id, "args": self.agent.args, "env": self.agent.env,
            "prompt": self.prompt, "cols": 120, "rows": 32,
        })
    }

    /// The audit log's line for the session `id` it started: which agent in which project, and the session (the app reads it back to show
    /// starts it missed while it was closed, `app/src/lib/phone-starts.ts`). Never the prompt.
    pub fn summary(&self, id: &str) -> String {
        let label = if self.agent.label.is_empty() { &self.agent.kind } else { &self.agent.label };
        let project = if self.project.name.is_empty() { &self.project.id } else { &self.project.name };
        format!("{label} in {project} (session {id})")
    }
}

/// Checks a phone's `new_session` against the catalogue: (code, message) when it is refused.
pub fn plan(c: Result<&Catalogue, Missing>, req: &Value) -> Result<Planned, (&'static str, String)> {
    let c = c.map_err(Missing::refusal)?;
    let text = |k: &str| req.get(k).and_then(Value::as_str).filter(|s| !s.is_empty());
    let project_id = text("project_id").ok_or(("bad_request", "missing project_id".to_string()))?;
    let kind = text("kind").ok_or(("bad_request", "missing kind".to_string()))?;
    let project = c.projects.iter().find(|p| p.id == project_id).ok_or(("no_project", "That project cannot be started from the phone: it is not on this computer, or it runs over SSH.".to_string()))?;
    let agent = c.agents.iter().find(|a| a.kind == kind).ok_or(("no_agent", "That agent cannot be started from the phone: it is hidden, or chat-only.".to_string()))?;
    let prompt = match req.get("prompt") {
        None | Some(Value::Null) => None,
        Some(Value::String(p)) => Some(crate::session::typed_text(p.trim())).filter(|p| !p.is_empty()),
        Some(_) => return Err(("bad_request", "the prompt is text".to_string())),
    };
    if prompt.as_ref().is_some_and(|p| p.chars().count() > crate::subs::MAX_TASK) {
        return Err(("too_long", format!("The first prompt is too long (at most {} characters).", crate::subs::MAX_TASK)));
    }
    Ok(Planned { project: project.clone(), agent: agent.clone(), prompt })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalogue() -> Value {
        json!({
            "projects": [{"id": "p1", "name": "Shop", "path": "C:/work/shop"}, {"id": "p2", "name": "Docs", "path": "/home/me/docs"}],
            "agents": [
                {"kind": "claude", "label": "Claude Code", "args": ["--dangerously-skip-permissions"], "env": {"ANTHROPIC_API_KEY": "sk-secret"}},
                {"kind": "codex", "label": "Codex", "args": ["-c", "approval_policy=on-request"]},
                {"kind": "gemini", "label": "Gemini"},
            ],
        })
    }

    #[test]
    fn a_catalogue_is_read_whole_and_bounded() {
        let c = parse(&catalogue()).unwrap();
        assert_eq!(c.projects.len(), 2);
        assert_eq!(c.agents[0].env.get("ANTHROPIC_API_KEY").map(String::as_str), Some("sk-secret"));
        assert!(c.agents[2].args.is_empty() && c.agents[2].env.is_empty(), "args and env may be left out");
        assert!(!format!("{c:?}").contains("sk-secret"), "printing it shows the environment's names only");
        let many = |n: usize, item: Value| Value::Array(vec![item; n]);
        let project = json!({"id": "p", "name": "n", "path": "/p"});
        let agent = json!({"kind": "claude", "label": "C"});
        assert!(parse(&json!({"projects": many(MAX_PROJECTS, project.clone()), "agents": many(MAX_AGENTS, agent.clone())})).is_ok());
        assert!(parse(&json!({"projects": many(MAX_PROJECTS + 1, project.clone()), "agents": []})).unwrap_err().contains("projects"));
        assert!(parse(&json!({"projects": [], "agents": many(MAX_AGENTS + 1, agent.clone())})).unwrap_err().contains("agents"));
        let args = json!({"kind": "claude", "label": "C", "args": vec!["-x"; MAX_ARGS + 1]});
        assert!(parse(&json!({"projects": [], "agents": [args]})).unwrap_err().contains("arguments"));
        let heavy = json!({"kind": "claude", "label": "C", "env": {"BIG": "x".repeat(MAX_MESSAGE)}});
        assert!(parse(&json!({"projects": [], "agents": [heavy]})).unwrap_err().contains("too large"));
        let named = |name: &str, label: &str| json!({"projects": [{"id": "p", "name": name, "path": "/p"}], "agents": [{"kind": "claude", "label": label}]});
        assert!(parse(&named(&"n".repeat(MAX_PROJECT_NAME), &"l".repeat(MAX_LABEL))).is_ok(), "at the bounds");
        assert!(parse(&named("", "")).is_ok(), "an empty name or label is shown as the id or the kind");
        let env_named = |key: &str| json!({"projects": [], "agents": [{"kind": "claude", "label": "C", "env": {key: "v=1"}}]});
        assert!(parse(&env_named(&"K".repeat(MAX_ENV_KEY))).is_ok(), "a name at the bound; a value may hold \"=\"");
        assert!(!parse(&env_named("A=sk-leak")).unwrap_err().contains("sk-leak"), "a refused name is not quoted");
        for bad in [
            json!({}),
            json!({"projects": [], "agents": [{"kind": "", "label": "C"}]}),
            json!({"projects": [{"id": "p", "name": "n", "path": ""}], "agents": []}),
            json!({"projects": [], "agents": [{"kind": "claude", "label": "C", "env": {"K": 1}}]}),
            named(&"n".repeat(MAX_PROJECT_NAME + 1), "C"),
            named("n", &"l".repeat(MAX_LABEL + 1)),
            named("a\u{7}b", "C"),
            named("n", "C\nD"),
            json!({"projects": [{"id": "p", "name": "n", "path": "/p\u{0}q"}], "agents": []}),
            env_named(""),
            env_named("A=B"),
            env_named("A\u{0}B"),
            env_named("A\nB"),
            env_named(&"K".repeat(MAX_ENV_KEY + 1)),
        ] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
        // What a refusal says never quotes what was sent.
        let refused = parse(&json!({"projects": [], "agents": [{"kind": "claude", "label": "C", "env": "sk-leak"}]})).unwrap_err();
        assert!(!refused.contains("sk-leak"), "{refused}");
    }

    #[test]
    fn options_list_the_projects_without_folders_and_the_installed_agents_with_whether_they_ask() {
        assert_eq!(options(Err(Missing::NotSent), &["claude"]), Ok(json!({"type": "new_options", "ready": false, "projects": [], "agents": [], "why": NOT_READY})));
        assert_eq!(options(Err(Missing::Refused), &["claude"]), Err(("launch_refused", REFUSED.to_string())));
        let c = parse(&catalogue()).unwrap();
        let o = options(Ok(&c), &["claude", "codex", "omp"]).unwrap();
        assert_eq!(o["ready"], true);
        assert_eq!(o["projects"], json!([{"id": "p1", "name": "Shop"}, {"id": "p2", "name": "Docs"}]), "in the catalogue's order");
        assert!(!o.to_string().contains("work/shop") && !o.to_string().contains("sk-secret"), "neither folders nor environments go to a phone");
        assert_eq!(o["agents"], json!([{"kind": "claude", "label": "Claude Code", "skips": true}, {"kind": "codex", "label": "Codex", "skips": false}]), "gemini is not installed; omp is not offered");
        assert_eq!(options(Ok(&c), &[]).unwrap()["agents"], json!([]));
        assert_eq!(options(Ok(&Catalogue::default()), &["claude"]).unwrap()["ready"], true, "an empty catalogue was still sent by the app");
    }

    #[test]
    fn a_start_is_checked_against_the_catalogue_and_takes_its_arguments_and_environment_only() {
        let c = parse(&catalogue()).unwrap();
        let ask = |req: Value| plan(Ok(&c), &req).map(|p| p.create_params()).map_err(|(code, _)| code);
        assert_eq!(plan(Err(Missing::NotSent), &json!({"project_id": "p1", "kind": "claude"})).err().map(|e| e.0), Some("not_ready"));
        assert_eq!(plan(Err(Missing::Refused), &json!({"project_id": "p1", "kind": "claude"})).err().map(|e| e.0), Some("launch_refused"));
        assert_eq!(ask(json!({"project_id": "nope", "kind": "claude"})).err(), Some("no_project"));
        assert_eq!(ask(json!({"project_id": "p1", "kind": "omp"})).err(), Some("no_agent"));
        assert_eq!(ask(json!({"kind": "claude"})).err(), Some("bad_request"));
        assert_eq!(ask(json!({"project_id": "p1", "kind": "claude", "prompt": 3})).err(), Some("bad_request"));
        assert_eq!(ask(json!({"project_id": "p1", "kind": "claude", "prompt": "x".repeat(crate::subs::MAX_TASK + 1)})).err(), Some("too_long"));
        assert!(ask(json!({"project_id": "p1", "kind": "claude", "prompt": "é".repeat(crate::subs::MAX_TASK)})).is_ok(), "characters, not bytes");
        // What the phone sends besides the project, the agent and the prompt changes nothing.
        let params = ask(json!({"project_id": "p1", "kind": "claude", "prompt": "  fix the build\x1b[201~\r\n ", "args": ["--evil"], "env": {"X": "y"}, "cwd": "/"})).unwrap();
        // The folder is the catalogue's, as the desktop starts in it.
        assert_eq!(params, json!({"kind": "claude", "cwd": "C:/work/shop", "project_id": "p1", "args": ["--dangerously-skip-permissions"], "env": {"ANTHROPIC_API_KEY": "sk-secret"}, "prompt": "fix the build[201~", "cols": 120, "rows": 32}));
        assert_eq!(ask(json!({"project_id": "p2", "kind": "codex", "prompt": "   "})).unwrap()["prompt"], Value::Null, "a blank prompt is none");
        let planned = plan(Ok(&c), &json!({"project_id": "p1", "kind": "claude", "prompt": "the secret plan"})).unwrap();
        assert_eq!(planned.summary("s-1"), "Claude Code in Shop (session s-1)");
    }
}
