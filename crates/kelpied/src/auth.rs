//! Who is calling, and what they may call.
//!
//! The endpoint's owner-only access keeps other OS users out; within the user, a connection
//! names itself with `auth`:
//! - **User** (the user token from `kelpie_proto::auth_token_path`): the app, `kelpied` run
//!   outside a session, the user's scripts. Everything, as before tokens existed.
//! - **Session** (`KELPIE_SESSION_ID` + `KELPIE_SESSION_TOKEN` from the session's environment):
//!   the programs a session runs. They get the documented scripting surface (`kelpied terminal`,
//!   `worktree`, `run`, `call session.*`) unless the user turned that off or the session was
//!   started restricted; then only their own session. Hook and run reports always speak for the
//!   caller's own session. Admin methods (custom agents, automations, phone access, daemon
//!   settings, closing sessions, turn checkpoints and their undo, notification channels, timers
//!   that type into a session, changing a session's account, resolving review notes, listing and stopping a session's processes…) are the user's only. Other sessions' environments are left
//!   out of what they read.
//! - **Claimed** (`KELPIE_SESSION_ID` without its token — some agents strip `*TOKEN*` variables
//!   from the programs they run, hooks included): hook reports for that session only, which is
//!   all a forged id could do before tokens existed.
//!
//! - **App** (the user token, `client: "app"`, *and* the connection's peer process runs the installed
//!   Kelpie app, `peer_identity`): everything a User may, plus the phone-access decisions in
//!   [`APP_ONLY`] (approving a phone, raising its permission, resetting phone access, Tailscale mode). The role only
//!   decides who may ask: what raises access takes effect once the person passed the OS verification
//!   the daemon runs itself (`user_presence`).
//!
//! Programs run as the same OS user and can read the user token if they are not sandboxed:
//! this scopes what agents do through Kelpie by default, it is not a boundary against
//! deliberate same-user code. The App role is a speed bump for such code (writing a file and running
//! it defeats it, and buys only a prompt with the daemon's text); the OS verification is the gate.

use std::sync::OnceLock;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::daemon::Daemon;

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Anonymous,
    User,
    /// The user, through the verified Kelpie app (see `peer_identity`).
    App,
    Session(String),
    Claimed(String),
}

impl Role {
    /// The user's own connection: the app (verified or not) and the user's scripts.
    pub fn is_user(&self) -> bool {
        matches!(self, Role::User | Role::App)
    }
}

/// Decisions about phone access that only the verified app may ask for (the daemon then verifies the person): a program that
/// holds the user token cannot add a phone or raise what one may do, nor raise the prompt that would.
pub const APP_ONLY: &[&str] = &["mobile.approve", "mobile.set_permission", "mobile.set_write", "mobile.reset", "mobile.set_tailscale"];

/// The role an `auth` with the user token gets: App when it says it is the app and its peer process is the installed app.
pub fn user_role(says_app: bool, peer: Option<u32>) -> Role {
    if says_app && peer.is_some_and(crate::peer_identity::is_trusted_app) {
        Role::App
    } else {
        Role::User
    }
}

/// Whether the user's connection `role` may call `method` (`Role::User` cannot make the app-only decisions).
pub fn user_may(role: &Role, method: &str) -> Result<()> {
    if *role == Role::User && APP_ONLY.contains(&method) {
        bail!("{method} needs the Kelpie app: approve phones and change what they may do in Kelpie → Settings → Mobile");
    }
    Ok(())
}

/// Per-daemon secret session tokens are derived from; a new daemon invalidates old tokens
/// (its sessions are respawned with new ones).
static SECRET: OnceLock<String> = OnceLock::new();
static USER_TOKEN: OnceLock<String> = OnceLock::new();

fn random_hex() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// Creates this daemon's secrets and publishes the user token for clients of `endpoint`.
pub fn init(endpoint: &kelpie_proto::Endpoint) -> Result<()> {
    let user = random_hex();
    kelpie_proto::write_user_token(endpoint, &user).map_err(|e| anyhow!("write auth token: {e}"))?;
    let _ = USER_TOKEN.set(user);
    let _ = SECRET.set(random_hex());
    Ok(())
}

/// The token set in session `id`'s environment.
pub fn session_token(id: &str) -> String {
    let secret = SECRET.get_or_init(random_hex);
    let digest = Sha256::new().chain_update(secret.as_bytes()).chain_update([0u8]).chain_update(id.as_bytes()).finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Resolves an `auth` request to a role.
pub fn authenticate(d: &Daemon, params: &Value) -> Result<Role> {
    let token = params.get("token").and_then(Value::as_str).unwrap_or_default();
    let id = params.get("session_id").and_then(Value::as_str).unwrap_or_default();
    if token.is_empty() {
        if !id.is_empty() && d.session_policy(id).is_some() {
            return Ok(Role::Claimed(id.to_string()));
        }
        bail!("missing token");
    }
    if USER_TOKEN.get().is_some_and(|u| same(u, token)) {
        return Ok(Role::User);
    }
    if !id.is_empty() && same(&session_token(id), token) && d.session_policy(id).is_some() {
        return Ok(Role::Session(id.to_string()));
    }
    bail!("invalid token")
}

/// Methods a session's programs may call while it may script other sessions. Not
/// `run.approve`: an approval gate waits for the user, so an agent cannot pass it.
const SCRIPTING: &[&str] = &[
    "daemon.info", "agent.presets", "session.list", "session.read", "session.input", "session.create",
    "meta.list", "meta.get", "meta.set", "run.list", "run.get", "run.create", "run.retry",
    "run.task_done", "run.task_skip", "run.cancel", "run.report", "hook.report", "note.add", "note.list",
];
/// Methods a restricted session's programs may call, on their own session only.
const OWN: &[&str] = &["daemon.info", "agent.presets", "session.list", "session.read", "session.input", "meta.get", "meta.set", "run.report", "hook.report", "note.add", "note.list"];
/// Methods of a session whose token went missing.
const CLAIMED: &[&str] = &["daemon.info", "hook.report"];

/// The method part of the policy: may a caller with this much reach call `method` at all?
fn allowed(role: &Role, scripting: bool, method: &str) -> bool {
    match role {
        Role::App => true,
        Role::User => !APP_ONLY.contains(&method),
        Role::Anonymous => method == "daemon.info",
        Role::Claimed(_) => CLAIMED.contains(&method),
        Role::Session(_) => (if scripting { SCRIPTING } else { OWN }).contains(&method),
    }
}

/// Session callers: (own id, may script other sessions).
fn reach(d: &Daemon, role: &Role) -> Result<Option<(String, bool, String)>> {
    match role {
        Role::Session(id) | Role::Claimed(id) => {
            let (restricted, cwd) = d.session_policy(id).ok_or_else(|| anyhow!("the calling session no longer exists"))?;
            let scripting = matches!(role, Role::Session(_)) && !restricted && d.agents_control_others();
            Ok(Some((id.clone(), scripting, cwd)))
        }
        _ => Ok(None),
    }
}

/// Checks `method` for `role`; may rewrite `params` so reports speak for the caller's session.
pub fn authorize(d: &Daemon, role: &Role, method: &str, params: &mut Value) -> Result<()> {
    let Some((id, scripting, cwd)) = reach(d, role)? else {
        if allowed(role, false, method) {
            return Ok(());
        }
        bail!("not authenticated: this client must send `auth` first (Kelpie {} or newer)", env!("CARGO_PKG_VERSION"));
    };
    if !allowed(role, scripting, method) {
        let why = match role {
            Role::Claimed(_) => " (its KELPIE_SESSION_TOKEN is missing)",
            _ if !scripting => " (scripting other sessions is turned off)",
            _ => "",
        };
        bail!("{method} is not available to programs running in a Kelpie session{why}");
    }
    // A session's request names its parameters: none at all is an empty object, anything else that is
    // not an object (an array deserialises into a struct field by field) is refused before any
    // method looks at it.
    if matches!(role, Role::Session(_) | Role::Claimed(_)) {
        object_params(method, params)?;
    }
    if matches!(method, "hook.report" | "run.report") {
        if let Some(obj) = params.as_object_mut() {
            obj.insert("session_id".into(), Value::String(id.clone()));
            // Whether the caller proved it is that session (a claimed id proves nothing).
            if method == "hook.report" {
                obj.insert("verified".into(), Value::Bool(matches!(role, Role::Session(_))));
            }
        }
    }
    if matches!(role, Role::Session(_)) {
        scope_session_params(method, &id, &cwd, params)?;
    }
    if !scripting {
        match method {
            "session.read" | "session.input" => {
                let handle = params.get("handle").and_then(Value::as_u64).ok_or_else(|| anyhow!("missing handle"))? as u32;
                if d.session_id_of(handle).as_deref() != Some(id.as_str()) {
                    bail!("{method}: only this session's own terminal");
                }
            }
            "meta.set" => {
                let path = params.get("path").and_then(Value::as_str).unwrap_or_default();
                if !own_folder(path, &cwd) {
                    bail!("meta.set: only this session's own folder");
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// `params` as an object: missing (null) becomes `{}`, any other shape is an error.
fn object_params(method: &str, params: &mut Value) -> Result<()> {
    match params {
        Value::Object(_) => Ok(()),
        Value::Null => {
            *params = Value::Object(Default::default());
            Ok(())
        }
        _ => bail!("{method}: params must be an object"),
    }
}

/// What a session's programs cannot decide for themselves. Review notes go to the worktree the
/// session is in and carry its name; what the user's app fills in (id, time, side, quoted text) is
/// worked out by the daemon. A run's `materialize` lists are what the user approved in the app: a
/// program cannot widen them.
fn scope_session_params(method: &str, id: &str, cwd: &str, params: &mut Value) -> Result<()> {
    object_params(method, params)?;
    let Some(obj) = params.as_object_mut() else { return Ok(()) };
    match method {
        "note.add" | "note.list" => {
            if let Some(path) = obj.get("path").and_then(Value::as_str).filter(|p| !p.is_empty()) {
                if crate::notes::worktree_key(path) != crate::notes::worktree_key(cwd) {
                    bail!("{method}: only this session's own worktree");
                }
            }
            obj.insert("path".into(), Value::String(cwd.to_string()));
            for key in ["id", "at", "side", "text", "by"] {
                obj.remove(key);
            }
            obj.insert("by_session".into(), Value::String(id.to_string()));
        }
        "run.create" => {
            obj.remove("materialize");
            // A task on an SSH host starts a program on another machine with the user's ssh setup: only the
            // user's own app asks for that. Refused outright, not dropped (the task would run here instead).
            // Only a list of objects: a task sent as an array would be read field by field, and its tenth
            // element is the host.
            match obj.get("tasks") {
                None | Some(Value::Null) => {}
                Some(Value::Array(tasks)) => {
                    if tasks.iter().any(|x| !x.is_object()) {
                        bail!("each task must be an object");
                    }
                    if tasks.iter().any(|x| x.get("host").is_some_and(|h| !h.is_null())) {
                        bail!("a program cannot start a task on an SSH host");
                    }
                }
                Some(_) => bail!("tasks must be a list"),
            }
        }
        _ => {}
    }
    Ok(())
}

/// What a session's programs get back: other sessions' environments left out (they may hold
/// API keys), and only their own session when they may not script others.
pub fn filter_reply(d: &Daemon, role: &Role, method: &str, result: &mut Value) {
    let Ok(Some((id, scripting, _))) = reach(d, role) else { return };
    let strip = |s: &mut Value| {
        if s.get("id").and_then(Value::as_str) != Some(id.as_str()) {
            if let Some(o) = s.as_object_mut() {
                o.remove("env");
            }
        }
    };
    match method {
        "session.list" => {
            if let Some(list) = result.as_array_mut() {
                if !scripting {
                    list.retain(|s| s.get("id").and_then(Value::as_str) == Some(id.as_str()));
                }
                list.iter_mut().for_each(strip);
            }
        }
        "session.create" => strip(result),
        _ => {}
    }
}

/// Event access follows the same reach as replies; unknown/admin events are user-only.
#[derive(Clone, Copy)]
pub(crate) enum EventVisibility {
    Hidden,
    Full,
    WithoutEnv,
}

pub(crate) fn event_visibility(d: &Daemon, role: &Role, event: &str, params: &Value, closed_session: Option<&str>) -> EventVisibility {
    use EventVisibility::{Full, Hidden, WithoutEnv};
    let id = match role {
        Role::User | Role::App => return Full,
        Role::Session(id) => id,
        Role::Anonymous | Role::Claimed(_) => return Hidden,
    };
    // Closing removes the session before publishing. Its authenticated owner must still
    // receive that last event, without adding an identity field to the wire payload.
    if event == "session.closed" && closed_session == Some(id.as_str()) {
        return Full;
    }
    let Ok(Some((_, scripting, cwd))) = reach(d, role) else { return Hidden };
    match event {
        "session.updated" => match params.get("id").and_then(Value::as_str) {
            Some(subject) if subject == id => Full,
            Some(_) if scripting => WithoutEnv,
            _ => Hidden,
        },
        "session.turn" => {
            if params.get("session").and_then(Value::as_str).is_some_and(|subject| subject == id || scripting) { Full } else { Hidden }
        }
        "session.closed" => {
            if scripting {
                return Full;
            }
            let own = closed_session.map(|subject| subject == id).unwrap_or_else(|| {
                params.get("handle").and_then(Value::as_u64).and_then(|h| u32::try_from(h).ok()).and_then(|h| d.session_id_of(h)).as_deref() == Some(id.as_str())
            });
            if own { Full } else { Hidden }
        }
        "meta.updated" => {
            if params.get("path").and_then(Value::as_str).is_some_and(|path| scripting || own_folder(path, &cwd)) { Full } else { Hidden }
        }
        "run.updated" | "run.deleted" if scripting => Full,
        _ => Hidden,
    }
}

/// `path` is the session's folder or inside it, compared the way the metadata store keys
/// folders (canonical where they exist); `..` never passes.
fn own_folder(path: &str, cwd: &str) -> bool {
    if path.split(['/', '\\']).any(|part| part == "..") {
        return false;
    }
    within(&crate::meta::key(path), &crate::meta::key(cwd))
}

/// A path the way folders are compared: `/` separators, no trailing one, lowercase on Windows.
pub(crate) fn norm(p: &str) -> String {
    let p = p.replace('\\', "/");
    let p = p.trim_end_matches('/').to_string();
    if cfg!(windows) { p.to_lowercase() } else { p }
}

/// `path` is `root` or inside it (case-insensitive and separator-agnostic on Windows).
pub(crate) fn within(path: &str, root: &str) -> bool {
    within_norm(&norm(path), &norm(root))
}

/// [`within`] for paths already through [`norm`].
pub(crate) fn within_norm(path: &str, root: &str) -> bool {
    !root.is_empty() && path.starts_with(root) && (path.len() == root.len() || path.as_bytes()[root.len()] == b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_tokens_differ_per_session_and_compare_exactly() {
        let a = session_token("a");
        assert_eq!(a, session_token("a"));
        assert_ne!(a, session_token("b"));
        assert!(same(&a, &session_token("a")));
        assert!(!same(&a, &a[1..]));
    }

    #[test]
    fn within_matches_root_and_children_only() {
        assert!(within("D:/proj", "D:/proj"));
        assert!(within("D:/proj/sub", "D:/proj/"));
        assert!(!within("D:/project", "D:/proj"));
        assert!(!within("D:/other", "D:/proj"));
        assert!(!within("D:/proj", ""));
        if cfg!(windows) {
            assert!(within(r"d:\Proj\sub", "D:/proj"));
        }
    }

    #[test]
    fn parent_steps_never_count_as_the_own_folder() {
        let dir = std::env::temp_dir().join(format!("kelpie-auth-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let root = dir.to_string_lossy().to_string();
        assert!(own_folder(&dir.join("sub").to_string_lossy(), &root));
        assert!(!own_folder(&format!("{root}/sub/../.."), &root));
        assert!(!own_folder(&dir.parent().unwrap().to_string_lossy(), &root));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_writes_notes_only_in_its_own_worktree() {
        let dir = std::env::temp_dir().join(format!("kelpie-auth-notes-{}", uuid::Uuid::new_v4()));
        let (mine, other) = (dir.join("mine"), dir.join("other"));
        for d in [&mine, &other] {
            std::fs::create_dir_all(d.join("src")).unwrap();
            std::fs::write(d.join(".git"), "gitdir: x").unwrap();
        }
        let cwd = mine.join("src").to_string_lossy().into_owned();
        // No path: its own worktree. The user's fields are not its to set.
        let mut p = serde_json::json!({"file": "a.rs", "line": 3, "note": "n", "id": "x", "at": 1, "side": "old", "text": "forged", "by": "the user"});
        scope_session_params("note.add", "s1", &cwd, &mut p).unwrap();
        assert_eq!(p["path"], cwd.as_str());
        assert_eq!(p["by_session"], "s1");
        for k in ["id", "at", "side", "text", "by"] {
            assert!(p.get(k).is_none(), "{k}");
        }
        assert_eq!(p["note"], "n");
        // Naming the worktree it is in (from a subfolder, or its root) is fine; another one is not.
        for ok in [mine.to_string_lossy(), mine.join("src").to_string_lossy()] {
            scope_session_params("note.list", "s1", &cwd, &mut serde_json::json!({"path": ok})).unwrap();
        }
        let err = scope_session_params("note.add", "s1", &cwd, &mut serde_json::json!({"path": other.to_string_lossy(), "file": "a.rs", "line": 1, "note": "n"})).unwrap_err();
        assert!(err.to_string().contains("own worktree"), "{err}");
        // A program cannot widen what the user approved for a run's worktrees.
        let mut run = serde_json::json!({"name": "r", "materialize": {"link": ["node_modules"]}, "tasks": []});
        scope_session_params("run.create", "s1", &cwd, &mut run).unwrap();
        assert!(run.get("materialize").is_none() && run["name"] == "r");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_cannot_get_around_the_scoping_with_the_shape_of_its_params() {
        let dir = std::env::temp_dir().join(format!("kelpie-auth-shape-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".git"), "gitdir: x").unwrap();
        let cwd = dir.to_string_lossy().into_owned();
        for method in ["note.add", "note.list", "run.create"] {
            // No params: an empty object, scoped like any other (a note.list without a folder is its own worktree's).
            let mut none = Value::Null;
            scope_session_params(method, "s1", &cwd, &mut none).unwrap();
            assert!(none.is_object(), "{method}");
            if method.starts_with("note.") {
                assert_eq!(none["path"], cwd.as_str(), "{method}: a session never asks for every worktree");
                assert_eq!(none["by_session"], "s1");
            }
            for bad in [serde_json::json!([]), serde_json::json!(["r", "", cwd, null, [], null, [[".env"], ["node_modules"]]]), serde_json::json!("note.list"), serde_json::json!(7), serde_json::json!(true)] {
                let mut p = bad.clone();
                let err = scope_session_params(method, "s1", &cwd, &mut p).unwrap_err();
                assert!(err.to_string().contains("must be an object"), "{method} {bad}: {err}");
            }
        }
        // The same for every other method a session may call.
        for method in ["session.list", "meta.set", "hook.report", "run.report", "daemon.info"] {
            assert!(object_params(method, &mut serde_json::json!([1])).is_err(), "{method}");
            let mut null = Value::Null;
            object_params(method, &mut null).unwrap();
            assert_eq!(null, serde_json::json!({}));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capability_matrix() {
        let user = Role::User;
        let s = Role::Session("s".into());
        let claimed = Role::Claimed("s".into());
        let admin = ["chat.send", "chat.write_turn", "chat.queue_op", "chat.close_stdin", "agent.set_custom", "automation.list", "automation.state", "automation.save", "automation.delete", "automation.run", "automation.approve", "automation.set_paused", "automation.event", "daemon.shutdown", "daemon.set_policy", "mobile.approve", "mobile.set_write", "session.close", "session.kill", "session.restart", "session.update", "session.attach", "session.answer", "run.approve", "run.delete", "agent.run", "checkpoint.status", "checkpoint.set_enabled", "checkpoint.list", "checkpoint.owners", "checkpoint.diff", "checkpoint.blob", "checkpoint.undo_plan", "checkpoint.undo", "checkpoint.drop", "ui.focus", "notify.get", "notify.set", "notify.test", "notify.rotate_topic", "notify.clear", "notify.report", "session.schedule_continue", "session.cancel_scheduled", "session.scheduled", "session.set_env_restart", "note.clear", "note.keys", "process.list", "process.ports", "process.port_owner", "process.kill", "process.kill_children"];
        // Phone-access decisions: the verified app only, never the bare user token or a session.
        for m in APP_ONLY {
            assert!(allowed(&Role::App, false, m), "app: {m}");
            assert!(!allowed(&user, false, m), "user token alone: {m}");
            assert!(user_may(&user, m).is_err(), "user token alone: {m}");
            assert!(user_may(&Role::App, m).is_ok(), "app: {m}");
            assert!(!allowed(&s, true, m) && !allowed(&claimed, true, m) && !allowed(&Role::Anonymous, false, m), "{m}");
        }
        for m in ["mobile.status", "mobile.enable", "mobile.disable", "mobile.pair_start", "mobile.revoke", "session.list"] {
            assert!(allowed(&user, false, m) && user_may(&user, m).is_ok(), "user: {m}");
            assert!(allowed(&Role::App, false, m), "app: {m}");
        }
        assert!(Role::App.is_user() && Role::User.is_user() && !s.is_user() && !Role::Anonymous.is_user());
        // A connection that only says it is the app, or whose peer is unknown, is the user.
        assert_eq!(user_role(true, None), Role::User);
        assert_eq!(user_role(false, Some(std::process::id())), Role::User);
        assert_eq!(user_role(true, Some(std::process::id())), if crate::peer_identity::is_trusted_app(std::process::id()) { Role::App } else { Role::User });
        for m in admin {
            assert!(allowed(&user, false, m) || APP_ONLY.contains(&m), "user: {m}");
            assert!(!allowed(&s, true, m), "scripting session: {m}");
            assert!(!allowed(&s, false, m), "own-only session: {m}");
            assert!(!allowed(&claimed, true, m), "claimed: {m}");
        }
        for m in ["session.list", "session.read", "session.input", "session.create", "meta.set", "run.create", "run.report", "hook.report", "note.add", "note.list"] {
            assert!(allowed(&s, true, m), "scripting session: {m}");
        }
        for m in ["note.add", "note.list"] {
            assert!(allowed(&s, false, m), "own-only session: {m}");
            assert!(!allowed(&claimed, true, m), "claimed: {m}");
        }
        assert!(!allowed(&s, false, "session.create"));
        assert!(!allowed(&s, false, "run.create"));
        assert!(allowed(&claimed, true, "hook.report"));
        assert!(!allowed(&claimed, true, "session.input"));
        assert!(!allowed(&claimed, true, "run.report"));
        assert!(allowed(&Role::Anonymous, false, "daemon.info"));
        assert!(!allowed(&Role::Anonymous, false, "session.list"));
        assert!(OWN.iter().all(|m| SCRIPTING.contains(m)));
    }

    #[test]
    fn programs_cannot_point_a_run_at_a_host() {
        let task = |host: Value| serde_json::json!({"title": "t", "prompt": "p", "host": host});
        let create = |tasks: Value| serde_json::json!({"name": "R", "cwd": "/p", "tasks": tasks});
        let mut hosted = create(serde_json::json!([task(serde_json::json!({"dest": "evil", "path": "/"}))]));
        let err = scope_session_params("run.create", "s1", "/p", &mut hosted).unwrap_err().to_string();
        assert!(err.contains("SSH host"), "{err}");
        // The second task of several, and as a string or an array: still a host.
        let mut later = create(serde_json::json!([serde_json::json!({"title": "t", "prompt": "p"}), task(serde_json::json!("evil"))]));
        assert!(scope_session_params("run.create", "s1", "/p", &mut later).is_err());
        // Local tasks, and a null host, pass; materialize is still dropped.
        let mut plain = create(serde_json::json!([task(Value::Null), {"title": "u", "prompt": "q"}]));
        plain["materialize"] = serde_json::json!({"copy": [".env"], "link": []});
        scope_session_params("run.create", "s1", "/p", &mut plain).unwrap();
        assert!(plain.get("materialize").is_none());
        // Other methods are not affected, and a claimed session cannot create a run at all.
        let mut other = serde_json::json!({"tasks": [task(serde_json::json!({"dest": "x"}))]});
        assert!(scope_session_params("run.list", "s1", "/p", &mut other).is_ok());
        assert!(!allowed(&Role::Claimed("s1".into()), true, "run.create"));
        assert!(allowed(&Role::Session("s1".into()), true, "run.create"));
        assert!(allowed(&Role::User, false, "run.create"));
    }

    #[test]
    fn no_shape_of_task_gets_a_host_past_the_scoping() {
        let host = serde_json::json!({"dest": "127.0.0.1", "port": 1, "path": "/tmp"});
        let check = |tasks: Value| {
            let mut p = serde_json::json!({"name": "R", "cwd": "/p", "tasks": tasks});
            scope_session_params("run.create", "s1", "/p", &mut p).map(|_| p)
        };
        // A task as an array deserialises field by field: the tenth element is `host`.
        let array = serde_json::json!([[null, "t", "p", "shell", [], {}, [], true, false, host]]);
        assert!(check(array).unwrap_err().to_string().contains("must be an object"));
        assert!(check(serde_json::json!([null])).is_err());
        assert!(check(serde_json::json!(["a task"])).is_err());
        assert!(check(serde_json::json!([1, {"title": "t", "prompt": "p"}])).is_err());
        assert!(check(serde_json::json!("tasks")).is_err());
        assert!(check(serde_json::json!({"0": {"title": "t", "prompt": "p"}})).is_err());
        assert!(check(serde_json::json!(7)).is_err());
        // The host as an array is a host too.
        assert!(check(serde_json::json!([{"title": "t", "prompt": "p", "host": ["h", null, null, "/x"]}])).is_err());
        // A missing or null `tasks` is left for `build_run` to refuse; plain object tasks pass.
        assert!(check(Value::Null).is_ok());
        assert!(check(serde_json::json!([{"title": "t", "prompt": "p"}])).is_ok());
        let msg = check(serde_json::json!([{"host": host}])).unwrap_err().to_string();
        assert_eq!(msg, "a program cannot start a task on an SSH host");
    }
}
