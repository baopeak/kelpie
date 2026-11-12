//! What a phone may start (`new_options`, `new_session`): the launch catalogue the app resolves and pushes with `mobile.set_launch`.
//!
//! The app owns the agent profiles, the permission mode, the accounts and the hidden agents, so it works out each agent's arguments and
//! environment exactly as its own launchers would, and the daemon keeps the result. A phone then starts a session with those and nothing
//! else, also while the window is closed. The catalogue lives in memory only, like the project names: profile environments can hold API
//! keys, and none of them is written again or logged. A daemon that has not heard from the app since it started has none, and a phone is
//! told to open Kelpie on the computer once.
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

/// What the phone is told while there is no catalogue.
pub const NOT_READY: &str = "Open Kelpie on your computer once so this phone can start sessions.";

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
    let plain = |s: &str, most: usize| !s.is_empty() && s.len() <= most && !s.chars().any(char::is_control);
    for p in &c.projects {
        if !plain(&p.id, 128) || p.name.len() > 256 || !plain(&p.path, 4096) {
            return Err("each project needs an id (at most 128 bytes), a name (at most 256) and a folder".into());
        }
    }
    for a in &c.agents {
        if !plain(&a.kind, 64) || a.label.len() > 128 {
            return Err("each agent needs a kind (at most 64 bytes) and a label (at most 128)".into());
        }
        if a.args.len() > MAX_ARGS {
            return Err(format!("at most {MAX_ARGS} arguments for one agent"));
        }
    }
    Ok(c)
}

/// The phone's `new_options`: the projects in the catalogue's order (their folders stay here), and the catalogue's agents that are
/// installed, each saying whether it runs without asking before it acts (as `sub_rpc::skips_permissions` reads a session's arguments).
pub fn options(c: Option<&Catalogue>, installed: &[&str]) -> Value {
    let Some(c) = c else {
        return json!({"type": "new_options", "ready": false, "projects": [], "agents": [], "why": NOT_READY});
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
    json!({"type": "new_options", "ready": true, "projects": projects, "agents": agents})
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
    /// launchers use. `cwd` is the project's folder as it is on disk now.
    pub fn create_params(&self, cwd: &str) -> Value {
        json!({
            "kind": self.agent.kind, "cwd": cwd, "project_id": self.project.id, "args": self.agent.args, "env": self.agent.env,
            "prompt": self.prompt, "cols": 120, "rows": 32,
        })
    }

    /// The audit log's line: which agent in which project. Never the prompt.
    pub fn summary(&self) -> String {
        let label = if self.agent.label.is_empty() { &self.agent.kind } else { &self.agent.label };
        let project = if self.project.name.is_empty() { &self.project.id } else { &self.project.name };
        format!("{label} in {project}")
    }
}

/// Checks a phone's `new_session` against the catalogue: (code, message) when it is refused.
pub fn plan(c: Option<&Catalogue>, req: &Value) -> Result<Planned, (&'static str, String)> {
    let c = c.ok_or(("not_ready", NOT_READY.to_string()))?;
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
        for bad in [json!({}), json!({"projects": [], "agents": [{"kind": "", "label": "C"}]}), json!({"projects": [{"id": "p", "name": "n", "path": ""}], "agents": []}), json!({"projects": [], "agents": [{"kind": "claude", "label": "C", "env": {"K": 1}}]})] {
            assert!(parse(&bad).is_err(), "{bad}");
        }
        // What a refusal says never quotes what was sent.
        let refused = parse(&json!({"projects": [], "agents": [{"kind": "claude", "label": "C", "env": "sk-leak"}]})).unwrap_err();
        assert!(!refused.contains("sk-leak"), "{refused}");
    }

    #[test]
    fn options_list_the_projects_without_folders_and_the_installed_agents_with_whether_they_ask() {
        assert_eq!(options(None, &["claude"]), json!({"type": "new_options", "ready": false, "projects": [], "agents": [], "why": NOT_READY}));
        let c = parse(&catalogue()).unwrap();
        let o = options(Some(&c), &["claude", "codex", "omp"]);
        assert_eq!(o["ready"], true);
        assert_eq!(o["projects"], json!([{"id": "p1", "name": "Shop"}, {"id": "p2", "name": "Docs"}]), "in the catalogue's order");
        assert!(!o.to_string().contains("work/shop") && !o.to_string().contains("sk-secret"), "neither folders nor environments go to a phone");
        assert_eq!(o["agents"], json!([{"kind": "claude", "label": "Claude Code", "skips": true}, {"kind": "codex", "label": "Codex", "skips": false}]), "gemini is not installed; omp is not offered");
        assert_eq!(options(Some(&c), &[])["agents"], json!([]));
        assert_eq!(options(Some(&Catalogue::default()), &["claude"])["ready"], true, "an empty catalogue was still sent by the app");
    }

    #[test]
    fn a_start_is_checked_against_the_catalogue_and_takes_its_arguments_and_environment_only() {
        let c = parse(&catalogue()).unwrap();
        let ask = |req: Value| plan(Some(&c), &req).map(|p| p.create_params("C:/real/shop")).map_err(|(code, _)| code);
        assert_eq!(plan(None, &json!({"project_id": "p1", "kind": "claude"})).err().map(|e| e.0), Some("not_ready"));
        assert_eq!(ask(json!({"project_id": "nope", "kind": "claude"})).err(), Some("no_project"));
        assert_eq!(ask(json!({"project_id": "p1", "kind": "omp"})).err(), Some("no_agent"));
        assert_eq!(ask(json!({"kind": "claude"})).err(), Some("bad_request"));
        assert_eq!(ask(json!({"project_id": "p1", "kind": "claude", "prompt": 3})).err(), Some("bad_request"));
        assert_eq!(ask(json!({"project_id": "p1", "kind": "claude", "prompt": "x".repeat(crate::subs::MAX_TASK + 1)})).err(), Some("too_long"));
        assert!(ask(json!({"project_id": "p1", "kind": "claude", "prompt": "é".repeat(crate::subs::MAX_TASK)})).is_ok(), "characters, not bytes");
        // What the phone sends besides the project, the agent and the prompt changes nothing.
        let params = ask(json!({"project_id": "p1", "kind": "claude", "prompt": "  fix the build\x1b[201~\r\n ", "args": ["--evil"], "env": {"X": "y"}, "cwd": "/"})).unwrap();
        assert_eq!(params, json!({"kind": "claude", "cwd": "C:/real/shop", "project_id": "p1", "args": ["--dangerously-skip-permissions"], "env": {"ANTHROPIC_API_KEY": "sk-secret"}, "prompt": "fix the build[201~", "cols": 120, "rows": 32}));
        assert_eq!(ask(json!({"project_id": "p2", "kind": "codex", "prompt": "   "})).unwrap()["prompt"], Value::Null, "a blank prompt is none");
        let planned = plan(Some(&c), &json!({"project_id": "p1", "kind": "claude", "prompt": "the secret plan"})).unwrap();
        assert_eq!(planned.summary(), "Claude Code in Shop");
    }
}
