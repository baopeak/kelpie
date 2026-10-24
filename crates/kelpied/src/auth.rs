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
//!   that type into a session, changing a session's account…) are the user's only. Other sessions' environments are left
//!   out of what they read.
//! - **Claimed** (`KELPIE_SESSION_ID` without its token — some agents strip `*TOKEN*` variables
//!   from the programs they run, hooks included): hook reports for that session only, which is
//!   all a forged id could do before tokens existed.
//!
//! Programs run as the same OS user and can read the user token if they are not sandboxed:
//! this scopes what agents do through Kelpie by default, it is not a boundary against
//! deliberate same-user code.

use std::sync::OnceLock;

use anyhow::{anyhow, bail, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::daemon::Daemon;

#[derive(Debug, Clone, PartialEq)]
pub enum Role {
    Anonymous,
    User,
    Session(String),
    Claimed(String),
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
    "run.task_done", "run.task_skip", "run.cancel", "run.report", "hook.report",
];
/// Methods a restricted session's programs may call, on their own session only.
const OWN: &[&str] = &["daemon.info", "agent.presets", "session.list", "session.read", "session.input", "meta.get", "meta.set", "run.report", "hook.report"];
/// Methods of a session whose token went missing.
const CLAIMED: &[&str] = &["daemon.info", "hook.report"];

/// The method part of the policy: may a caller with this much reach call `method` at all?
fn allowed(role: &Role, scripting: bool, method: &str) -> bool {
    match role {
        Role::User => true,
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
    if matches!(method, "hook.report" | "run.report") {
        if let Some(obj) = params.as_object_mut() {
            obj.insert("session_id".into(), Value::String(id.clone()));
            // Whether the caller proved it is that session (a claimed id proves nothing).
            if method == "hook.report" {
                obj.insert("verified".into(), Value::Bool(matches!(role, Role::Session(_))));
            }
        }
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

/// `path` is the session's folder or inside it, compared the way the metadata store keys
/// folders (canonical where they exist); `..` never passes.
fn own_folder(path: &str, cwd: &str) -> bool {
    if path.split(['/', '\\']).any(|part| part == "..") {
        return false;
    }
    within(&crate::meta::key(path), &crate::meta::key(cwd))
}

/// `path` is `root` or inside it (case-insensitive and separator-agnostic on Windows).
fn within(path: &str, root: &str) -> bool {
    let norm = |p: &str| {
        let p = p.replace('\\', "/");
        let p = p.trim_end_matches('/').to_string();
        if cfg!(windows) { p.to_lowercase() } else { p }
    };
    let (p, r) = (norm(path), norm(root));
    !r.is_empty() && (p == r || p.starts_with(&format!("{r}/")))
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
    fn capability_matrix() {
        let user = Role::User;
        let s = Role::Session("s".into());
        let claimed = Role::Claimed("s".into());
        let admin = ["agent.set_custom", "automation.save", "automation.run", "daemon.shutdown", "daemon.set_policy", "mobile.approve", "mobile.set_write", "session.close", "session.kill", "session.restart", "session.update", "session.attach", "session.answer", "run.approve", "run.delete", "agent.run", "checkpoint.status", "checkpoint.set_enabled", "checkpoint.list", "checkpoint.owners", "checkpoint.diff", "checkpoint.blob", "checkpoint.undo_plan", "checkpoint.undo", "checkpoint.drop", "ui.focus", "notify.get", "notify.set", "notify.test", "notify.rotate_topic", "notify.clear", "notify.report", "session.schedule_continue", "session.cancel_scheduled", "session.scheduled", "session.set_env_restart"];
        for m in admin {
            assert!(allowed(&user, false, m), "user: {m}");
            assert!(!allowed(&s, true, m), "scripting session: {m}");
            assert!(!allowed(&s, false, m), "own-only session: {m}");
            assert!(!allowed(&claimed, true, m), "claimed: {m}");
        }
        for m in ["session.list", "session.read", "session.input", "session.create", "meta.set", "run.create", "run.report", "hook.report"] {
            assert!(allowed(&s, true, m), "scripting session: {m}");
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
}
