//! Several Claude Code / Codex sign-ins side by side. Each extra account is its own config
//! home (`CLAUDE_CONFIG_DIR` / `CODEX_HOME`) under `<data>/accounts/<agent>/<id>`; sessions
//! started on an account get that variable, so credentials, settings and transcripts never
//! mix and switching the active account is only a pointer change in the UI. Credentials are
//! read here only to say whether an account is signed in and as whom — never returned.

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;
use serde_json::Value;

use crate::fs_ops::{err, CmdResult};

/// Marks a folder Kelpie created, so removal never touches anything else.
const MARKER: &str = ".kelpie-account";

pub(crate) fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

fn valid_agent(agent: &str) -> CmdResult<()> {
    if matches!(agent, "claude" | "codex") { Ok(()) } else { Err(format!("accounts are not supported for {agent}")) }
}

fn valid_id(id: &str) -> CmdResult<()> {
    if !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(())
    } else {
        Err("invalid account id".into())
    }
}

fn account_dir(agent: &str, id: &str) -> PathBuf {
    let dir = kelpie_proto::data_dir().join("accounts").join(agent).join(id);
    // One separator style: the data dir may come with forward slashes (KELPIE_DATA_DIR).
    if cfg!(windows) { PathBuf::from(dir.to_string_lossy().replace('/', "\\")) } else { dir }
}

/// The agent's own default home, used by the "Default" account.
fn default_home(agent: &str) -> PathBuf {
    match agent {
        "claude" => std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude")),
        _ => std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex")),
    }
}

/// Makes `CODEX_HOME/config.toml` keep the login in `auth.json` (not the OS keyring), so the
/// usage meter and the status check can read it, keeping every other setting, comment and table
/// (the same writer that edits the file's MCP servers).
fn codex_file_store(config: &str) -> CmdResult<String> {
    crate::agent_config::toml_set_top_level(config, "cli_auth_credentials_store", "file").map_err(|e| format!("Codex config.toml: {e}"))
}

/// Fills an account home from the agent's default home `src`: settings, instructions and (Codex)
/// config.toml with its MCP servers; never logins or history.
pub(crate) fn seed_account(agent: &str, dir: &Path, src: &Path) -> CmdResult<()> {
    match agent {
        "claude" => {
            for name in ["settings.json", "CLAUDE.md"] {
                let (from, to) = (src.join(name), dir.join(name));
                if from.is_file() && !to.exists() {
                    std::fs::copy(&from, &to).map_err(err)?;
                }
            }
        }
        _ => {
            let to = dir.join("config.toml");
            let base = std::fs::read_to_string(if to.exists() { to.clone() } else { src.join("config.toml") }).unwrap_or_default();
            std::fs::write(&to, codex_file_store(&base)?).map_err(err)?;
        }
    }
    Ok(())
}

/// Creates (or re-seeds) an account home. Settings come from the default account so the
/// agent behaves the same; logins and history do not.
fn create_blocking(agent: &str, id: &str) -> CmdResult<String> {
    valid_agent(agent)?;
    valid_id(id)?;
    let dir = account_dir(agent, id);
    std::fs::create_dir_all(&dir).map_err(err)?;
    std::fs::write(dir.join(MARKER), format!("{agent}\n")).map_err(err)?;
    seed_account(agent, &dir, &default_home(agent))?;
    Ok(dir.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn account_create(agent: String, id: String) -> CmdResult<String> {
    crate::fs_ops::blocking(move || create_blocking(&agent, &id)).await
}

/// Moves a Kelpie-created account home to the trash (logins and history with it).
#[tauri::command]
pub async fn account_remove(agent: String, id: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        valid_agent(&agent)?;
        valid_id(&id)?;
        let dir = account_dir(&agent, &id);
        if !dir.exists() {
            return Ok(());
        }
        // `.erka-account`: created before the rename (the data migration renames it, but a
        // folder it could not reach still counts).
        if !dir.join(MARKER).is_file() && !dir.join(".erka-account").is_file() {
            return Err("not a Kelpie account folder".into());
        }
        trash::delete(&dir).or_else(|_| std::fs::remove_dir_all(&dir)).map_err(err)
    })
    .await
}

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct AccountStatus {
    signed_in: bool,
    email: Option<String>,
    /// Subscription or auth kind ("max", "pro", "plus", "api key"…).
    plan: Option<String>,
    /// The config home in use (for display).
    home: String,
}

/// Claims of a JWT, without verifying it (only to show which account this is).
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn claude_status(dir: &Path) -> AccountStatus {
    let creds = read_json(&dir.join(".credentials.json")).or_else(|| crate::usage::claude_keychain_json(dir));
    let oauth = creds.as_ref().map(|c| &c["claudeAiOauth"]);
    let signed_in = oauth.and_then(|o| o["accessToken"].as_str()).is_some_and(|t| !t.is_empty())
        || oauth.and_then(|o| o["refreshToken"].as_str()).is_some_and(|t| !t.is_empty());
    // `.claude.json` sits in the config dir when CLAUDE_CONFIG_DIR is set, else in the home.
    let profile = read_json(&dir.join(".claude.json")).or_else(|| (dir == default_home("claude")).then(|| read_json(&home().join(".claude.json"))).flatten());
    let email = profile.as_ref().and_then(|p| p["oauthAccount"]["emailAddress"].as_str()).map(str::to_string);
    let plan = oauth.and_then(|o| o["subscriptionType"].as_str()).map(str::to_string);
    AccountStatus { signed_in, email: email.filter(|_| signed_in), plan, home: dir.to_string_lossy().into_owned() }
}

fn codex_status(dir: &Path) -> AccountStatus {
    let auth = read_json(&dir.join("auth.json"));
    let tokens = auth.as_ref().map(|a| &a["tokens"]);
    let oauth = tokens.and_then(|t| t["access_token"].as_str()).is_some_and(|t| !t.is_empty());
    let api_key = auth.as_ref().and_then(|a| a["OPENAI_API_KEY"].as_str()).is_some_and(|k| !k.is_empty());
    let claims = tokens.and_then(|t| t["id_token"].as_str()).and_then(jwt_claims);
    let email = claims.as_ref().and_then(|c| c["email"].as_str()).map(str::to_string);
    let plan = claims
        .as_ref()
        .and_then(|c| c["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str())
        .map(str::to_string)
        .or_else(|| (api_key && !oauth).then(|| "api key".to_string()));
    AccountStatus { signed_in: oauth || api_key, email, plan, home: dir.to_string_lossy().into_owned() }
}

/// Whether the account (`dir` = its home; none = the agent's default) is signed in.
#[tauri::command]
pub async fn account_status(agent: String, dir: Option<String>) -> CmdResult<AccountStatus> {
    crate::fs_ops::blocking(move || {
        valid_agent(&agent)?;
        let dir = dir.filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| default_home(&agent));
        Ok(if agent == "claude" { claude_status(&dir) } else { codex_status(&dir) })
    })
    .await
}

// ---------- switching a running session to another account ----------

/// What moving a session's conversation to another account's home takes, or why it cannot be done.
#[derive(Serialize, Default, Debug, PartialEq)]
pub struct TranscriptMove {
    /// Why the switch is refused; nothing was changed when this is set.
    refusal: Option<String>,
    /// Files copied (with `apply`), or that would be.
    files: u32,
    bytes: u64,
}

fn refused(why: impl Into<String>) -> CmdResult<TranscriptMove> {
    Ok(TranscriptMove { refusal: Some(why.into()), ..Default::default() })
}

/// A home Kelpie may switch to or from: the agent's own, or one it created.
fn known_home(agent: &str, dir: &Path) -> bool {
    dir == default_home(agent) || dir.join(MARKER).is_file()
}

/// `a`'s bytes are the start of `b`'s (a conversation that went on after it was copied), or the same.
fn is_start_of(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let Some(len) = fa.metadata().ok().map(|m| m.len()) else { return false };
    if fb.metadata().ok().is_none_or(|m| m.len() < len) {
        return false;
    }
    let (mut fa, mut fb) = (fa.take(len), fb.take(len));
    let (mut x, mut y) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(n), Ok(m)) = (fa.read(&mut x), fb.read(&mut y)) else { return false };
        if n != m || x[..n] != y[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// Same bytes, read in blocks (transcripts can be large).
fn same_content(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    if fa.metadata().ok().map(|m| m.len()) != fb.metadata().ok().map(|m| m.len()) {
        return false;
    }
    let (mut x, mut y) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(n), Ok(m)) = (fa.read(&mut x), fb.read(&mut y)) else { return false };
        if n != m || x[..n] != y[..m] {
            return false;
        }
        if n == 0 {
            return true;
        }
    }
}

/// Every regular file under `dir`, at most `limit` of them.
fn files_under(dir: &Path, depth: u8, out: &mut Vec<PathBuf>, limit: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= limit {
            return;
        }
        let Ok(t) = e.file_type() else { continue };
        if t.is_file() {
            out.push(e.path());
        } else if t.is_dir() && depth < 4 {
            files_under(&e.path(), depth + 1, out, limit);
        }
    }
}

/// Largest transcript set one switch copies.
const MAX_COPY_BYTES: u64 = 1 << 30;
const MAX_FILES: usize = 2000;

/// Copies a conversation's transcript from one account home into another, so the agent resumes it
/// there. Claude keeps `projects/<folder>/<id>.jsonl` (and its sub-agents in `<id>/`), Codex
/// `sessions/YYYY/MM/DD/rollout-…-<id>.jsonl`; both are found by the conversation id and copied to
/// the same place under the target home. A conversation that was never saved has nothing to copy
/// (the agent starts it under the same id). Refused: another agent, the same account, a home Kelpie
/// did not create, an account that is not signed in, a different (or newer) conversation already under
/// that id in the target, more than a gigabyte or 2000 files. An older copy of the same conversation (the
/// start of the file, left by an earlier switch) is replaced; with `apply` off nothing is written.
fn transcript_move(agent: &str, from: &Path, to: &Path, cwd: &str, id: &str, apply: bool) -> CmdResult<TranscriptMove> {
    valid_agent(agent)?;
    if !crate::agent_history::valid_id(id) {
        return refused("this session has no conversation to move yet");
    }
    if from == to {
        return refused("The session is already on that account.");
    }
    if !known_home(agent, from) || !known_home(agent, to) {
        return refused("Only the default account and accounts made in Kelpie can be switched between.");
    }
    let status = if agent == "claude" { claude_status(to) } else { codex_status(to) };
    if !status.signed_in {
        return refused("That account is not signed in. Sign in to it first (Settings → Accounts).");
    }
    let (kind, relative_root) = if agent == "claude" { (crate::agent_history::Agent::Claude, "projects") } else { (crate::agent_history::Agent::Codex, "sessions") };
    let Some(main) = crate::session_usage::resolve_in(kind, from, id, None, Some(cwd)) else {
        return Ok(TranscriptMove::default());
    };
    // The main file, and for Claude the folder named after the conversation (its sub-agents).
    let mut sources = vec![main.clone()];
    if agent == "claude" {
        files_under(&main.with_extension(""), 0, &mut sources, MAX_FILES + 1);
        if sources.len() > MAX_FILES {
            return refused("The conversation has too many files to copy safely.");
        }
    }
    let base = from.join(relative_root);
    // (from, to, size): a file the target has an older copy of is replaced by the longer one.
    let mut moves: Vec<(PathBuf, PathBuf, u64)> = Vec::new();
    for src in sources {
        let Ok(rel) = src.strip_prefix(&base) else { return refused("The conversation's file is not where the agent keeps its history.") };
        let dst = to.join(relative_root).join(rel);
        let len = std::fs::metadata(&src).map(|m| m.len()).map_err(err)?;
        if dst.exists() {
            if same_content(&src, &dst) {
                continue;
            }
            // The conversation went on after an earlier switch: the target's copy is its start.
            if is_start_of(&dst, &src) {
                moves.push((src, dst, len));
                continue;
            }
            if is_start_of(&src, &dst) {
                return refused("That account has a newer copy of this conversation, so nothing was copied.");
            }
            return refused("That account already has a different conversation with the same id.");
        }
        moves.push((src, dst, len));
    }
    let bytes: u64 = moves.iter().map(|m| m.2).sum();
    if bytes > MAX_COPY_BYTES {
        return refused("The conversation is too large to copy (over 1 GB).");
    }
    if apply {
        for (src, dst, _) in &moves {
            if let Some(dir) = dst.parent() {
                std::fs::create_dir_all(dir).map_err(err)?;
            }
            // Through a temporary name: an interrupted copy never looks like a conversation.
            let mut part = dst.as_os_str().to_owned();
            part.push(".part");
            if let Err(e) = std::fs::copy(src, &part).and_then(|_| std::fs::rename(&part, dst)) {
                let _ = std::fs::remove_file(&part);
                return Err(err(e));
            }
        }
    }
    Ok(TranscriptMove { refusal: None, files: moves.len() as u32, bytes })
}

/// Checks (and with `apply`, does) the file side of switching a session to another account.
/// `from` / `to` are config homes (none = the agent's default).
#[tauri::command]
pub async fn account_transcript(agent: String, from: Option<String>, to: Option<String>, cwd: String, conversation: Option<String>, apply: bool) -> CmdResult<TranscriptMove> {
    crate::fs_ops::blocking(move || {
        valid_agent(&agent)?;
        let home_of = |d: Option<String>| d.filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| default_home(&agent));
        transcript_move(&agent, &home_of(from), &home_of(to), &cwd, conversation.as_deref().unwrap_or(""), apply)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_config_keeps_settings_and_stores_login_in_a_file() {
        assert_eq!(codex_file_store("").unwrap(), "cli_auth_credentials_store = \"file\"\n");
        let cfg = "model = \"gpt-5\"\ncli_auth_credentials_store = \"keyring\"\n[mcp_servers.x]\ncommand = \"y\"\n";
        let out = codex_file_store(cfg).unwrap();
        assert_eq!(out, "model = \"gpt-5\"\ncli_auth_credentials_store = \"file\"\n[mcp_servers.x]\ncommand = \"y\"\n");
        // A key of the same name inside a table is not the top-level one.
        let out = codex_file_store("[profiles.a]\ncli_auth_credentials_store = \"x\"\n").unwrap();
        assert!(out.starts_with("cli_auth_credentials_store = \"file\"\n[profiles.a]\ncli_auth_credentials_store = \"x\""));
    }

    #[test]
    fn reads_status_without_exposing_tokens() {
        let dir = std::env::temp_dir().join(format!("kelpie-acct-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!claude_status(&dir).signed_in);
        std::fs::write(dir.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"t","subscriptionType":"max"}}"#).unwrap();
        std::fs::write(dir.join(".claude.json"), r#"{"oauthAccount":{"emailAddress":"a@b.c"}}"#).unwrap();
        let s = claude_status(&dir);
        assert!(s.signed_in);
        assert_eq!(s.email.as_deref(), Some("a@b.c"));
        assert_eq!(s.plan.as_deref(), Some("max"));
        let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"email":"x@y.z","https://api.openai.com/auth":{"chatgpt_plan_type":"plus"}}"#);
        std::fs::write(dir.join("auth.json"), format!(r#"{{"tokens":{{"access_token":"t","id_token":"h.{claims}.s"}}}}"#)).unwrap();
        let s = codex_status(&dir);
        assert!(s.signed_in);
        assert_eq!((s.email.as_deref(), s.plan.as_deref()), (Some("x@y.z"), Some("plus")));
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("\"t\""));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A scratch pair of account homes (both Kelpie-made, so the default home is never touched).
    struct Homes {
        root: PathBuf,
        from: PathBuf,
        to: PathBuf,
        cwd: String,
    }

    impl Homes {
        fn new(agent: &str, signed_in: bool) -> Homes {
            let root = std::env::temp_dir().join(format!("kelpie-switch-{}-{}", std::process::id(), uuid_like()));
            let (from, to) = (root.join("from"), root.join("to"));
            for d in [&from, &to] {
                std::fs::create_dir_all(d).unwrap();
                std::fs::write(d.join(MARKER), format!("{agent}\n")).unwrap();
            }
            if signed_in {
                let (file, body) = if agent == "claude" { (".credentials.json", r#"{"claudeAiOauth":{"accessToken":"t"}}"#) } else { ("auth.json", r#"{"tokens":{"access_token":"t"}}"#) };
                std::fs::write(to.join(file), body).unwrap();
            }
            let cwd = root.join("work").to_string_lossy().into_owned();
            Homes { root, from, to, cwd }
        }

        fn claude_file(&self, home: &Path, id: &str, text: &str) -> PathBuf {
            let dir = home.join("projects").join("C--work-proj");
            std::fs::create_dir_all(&dir).unwrap();
            let file = dir.join(format!("{id}.jsonl"));
            std::fs::write(&file, text).unwrap();
            file
        }

        fn run(&self, agent: &str, id: &str, apply: bool) -> TranscriptMove {
            transcript_move(agent, &self.from, &self.to, &self.cwd, id, apply).unwrap()
        }
    }

    impl Drop for Homes {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        format!("{}-{}", N.fetch_add(1, Ordering::Relaxed), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())
    }

    const CLAUDE_ID: &str = "0b9f2c1e-5d4a-4c0e-9a36-7d1e8f2a3b4c";
    const CODEX_ID: &str = "019f0a3c-1b2d-7c4e-8a90-123456789abc";

    #[test]
    fn a_claude_conversation_moves_with_its_subagents_and_nothing_is_written_before_apply() {
        let h = Homes::new("claude", true);
        let main = h.claude_file(&h.from, CLAUDE_ID, "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n");
        let sub = main.with_extension("").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("agent-a1.jsonl"), "{}\n").unwrap();
        // A neighbour of another conversation stays where it is.
        h.claude_file(&h.from, "11111111-2222-4333-8444-555555555555", "other");

        let plan = h.run("claude", CLAUDE_ID, false);
        assert_eq!((plan.refusal.clone(), plan.files), (None, 2));
        assert!(!h.to.join("projects").exists(), "a check writes nothing");

        let done = h.run("claude", CLAUDE_ID, true);
        assert_eq!((done.refusal, done.files), (None, 2));
        let moved = h.to.join("projects").join("C--work-proj").join(format!("{CLAUDE_ID}.jsonl"));
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{\"type\":\"user\"}\n{\"type\":\"assistant\"}\n");
        assert!(moved.with_extension("").join("subagents").join("agent-a1.jsonl").is_file());
        assert!(!h.to.join("projects/C--work-proj/11111111-2222-4333-8444-555555555555.jsonl").exists());
        assert!(main.is_file(), "the original stays with the old account");
        assert!(!std::fs::read_dir(moved.parent().unwrap()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".part")));

        // Done again: the copy is already there, so nothing is copied or refused.
        let again = h.run("claude", CLAUDE_ID, true);
        assert_eq!((again.refusal, again.files), (None, 0));
    }

    #[test]
    fn switching_back_replaces_the_older_copy_and_alternating_works() {
        let h = Homes::new("claude", true);
        // The first account is signed in too, so the way back is open.
        std::fs::write(h.from.join(".credentials.json"), r#"{"claudeAiOauth":{"accessToken":"t"}}"#).unwrap();
        let a = h.claude_file(&h.from, CLAUDE_ID, "one\ntwo\n");
        let sub = a.with_extension("").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("agent-1.jsonl"), "s1\n").unwrap();
        assert_eq!(h.run("claude", CLAUDE_ID, true).files, 2);
        // It goes on in the second account.
        let b = h.to.join("projects/C--work-proj").join(format!("{CLAUDE_ID}.jsonl"));
        std::fs::write(&b, "one\ntwo\nthree\n").unwrap();
        std::fs::write(b.with_extension("").join("subagents/agent-1.jsonl"), "s1\ns2\n").unwrap();
        // Back: the older copy is replaced, nothing else is touched.
        let back = transcript_move("claude", &h.to, &h.from, &h.cwd, CLAUDE_ID, true).unwrap();
        assert_eq!((back.refusal, back.files), (None, 2));
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one\ntwo\nthree\n");
        assert_eq!(std::fs::read_to_string(sub.join("agent-1.jsonl")).unwrap(), "s1\ns2\n");
        assert!(!std::fs::read_dir(a.parent().unwrap()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".part")));
        // And forth again with nothing new: nothing to do.
        let again = h.run("claude", CLAUDE_ID, true);
        assert_eq!((again.refusal, again.files), (None, 0));
        // The second account having gone further meanwhile is a newer copy: the way forth does not overwrite it.
        std::fs::write(&b, "one\ntwo\nthree\nfour\n").unwrap();
        let newer = h.run("claude", CLAUDE_ID, true);
        assert!(newer.refusal.is_some_and(|m| m.contains("newer copy")));
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "one\ntwo\nthree\nfour\n");
    }

    #[test]
    fn a_conversation_with_too_many_files_is_refused_not_copied_in_part() {
        let h = Homes::new("claude", true);
        let main = h.claude_file(&h.from, CLAUDE_ID, "x");
        let sub = main.with_extension("").join("subagents");
        std::fs::create_dir_all(&sub).unwrap();
        for i in 0..=MAX_FILES {
            std::fs::write(sub.join(format!("a{i}.jsonl")), "").unwrap();
        }
        let r = h.run("claude", CLAUDE_ID, true);
        assert!(r.refusal.is_some_and(|m| m.contains("too many files")));
        assert!(!h.to.join("projects").exists());
    }

    #[test]
    fn a_different_conversation_under_the_same_id_is_never_overwritten() {
        let h = Homes::new("claude", true);
        h.claude_file(&h.from, CLAUDE_ID, "mine");
        let theirs = h.claude_file(&h.to, CLAUDE_ID, "theirs");
        let r = h.run("claude", CLAUDE_ID, true);
        assert!(r.refusal.as_deref().is_some_and(|m| m.contains("different conversation")), "{r:?}");
        assert_eq!(std::fs::read_to_string(theirs).unwrap(), "theirs");
    }

    #[test]
    fn a_conversation_that_was_never_saved_has_nothing_to_move() {
        let h = Homes::new("claude", true);
        assert_eq!(h.run("claude", CLAUDE_ID, true), TranscriptMove::default());
        assert!(!h.to.join("projects").exists());
    }

    #[test]
    fn switching_is_refused_without_a_sign_in_a_conversation_or_a_known_home() {
        let h = Homes::new("claude", false);
        h.claude_file(&h.from, CLAUDE_ID, "x");
        assert!(h.run("claude", CLAUDE_ID, true).refusal.is_some_and(|m| m.contains("not signed in")));
        assert!(!h.to.join("projects").exists());
        assert!(h.run("claude", "", true).refusal.is_some_and(|m| m.contains("no conversation")));
        assert!(h.run("claude", "../escape", true).refusal.is_some());
        // The same account.
        let same = transcript_move("claude", &h.from, &h.from, &h.cwd, CLAUDE_ID, true).unwrap();
        assert!(same.refusal.is_some_and(|m| m.contains("already")));
        // A folder Kelpie did not make.
        let stranger = h.root.join("stranger");
        std::fs::create_dir_all(&stranger).unwrap();
        let r = transcript_move("claude", &h.from, &stranger, &h.cwd, CLAUDE_ID, true).unwrap();
        assert!(r.refusal.is_some_and(|m| m.contains("accounts made in Kelpie")));
        assert!(!stranger.join("projects").exists());
        // Agents without accounts.
        assert!(transcript_move("omp", &h.from, &h.to, &h.cwd, CLAUDE_ID, true).is_err());
    }

    #[test]
    fn a_codex_rollout_keeps_its_date_folder() {
        let h = Homes::new("codex", true);
        let day = h.from.join("sessions/2026/10/24");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join(format!("rollout-2026-10-24T10-00-00-{CODEX_ID}.jsonl")), "{}\n").unwrap();
        // Another thread's rollout is not touched.
        std::fs::write(day.join("rollout-2026-10-24T11-00-00-019f0a3c-0000-7000-8000-000000000000.jsonl"), "{}\n").unwrap();
        let r = h.run("codex", CODEX_ID, true);
        assert_eq!((r.refusal, r.files), (None, 1));
        let moved = h.to.join("sessions/2026/10/24");
        let names: Vec<String> = std::fs::read_dir(&moved).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![format!("rollout-2026-10-24T10-00-00-{CODEX_ID}.jsonl")]);
    }

    #[test]
    fn rejects_unsafe_ids() {
        assert!(valid_id("a-1_b").is_ok());
        for bad in ["", "..", "a/b", "a\\b", "a b"] {
            assert!(valid_id(bad).is_err(), "{bad}");
        }
    }
}
