//! A project's pull request as GitHub's own tool reads it, and whether GitHub would merge it.
//!
//! The daemon asks the user's `gh`, which holds the sign-in; it keeps no token of its own. `git` is run in the project's folder the way the
//! commit watcher runs it (hooks off, no prompts, a time limit); `gh` is never run there: a program in a session can change a worktree's git
//! configuration, so `gh` gets the repository with `-R` and runs from the daemon's data folder. Everything that reads or runs a program here
//! blocks: callers move it off the async threads.
//!
//! The line a phone is shown (`merge_line`) follows GitHub's own merge state (`mergeStateStatus`, `mergeable`), and a check counts for or
//! against it only when branch protection requires it. A read that failed is `unknown`, never "can merge".

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The only host a pull request is read from.
pub const HOST: &str = "github.com";
/// Longest message about a pull request that goes to a phone or the audit log, in characters.
pub const MAX_MESSAGE: usize = 200;
/// Time limits of one `git` look and one `gh` call (the network is behind it).
const GIT_LIMIT: Duration = Duration::from_secs(10);
const GH_LIMIT: Duration = Duration::from_secs(60);
/// Most bytes read from one program's output.
const MAX_OUTPUT: u64 = 2 * 1024 * 1024;
/// How long output still being read is waited for once its program has ended or was killed.
const DRAIN_WAIT: Duration = Duration::from_secs(2);
/// What `gh pr view` is asked for.
const VIEW_FIELDS: &str = "number,title,url,state,isDraft,baseRefName,headRefOid,mergeable,mergeStateStatus,statusCheckRollup";

const GH_MISSING: &str = "GitHub's command line tool (`gh`) is not installed here, or not on the computer's PATH.";
const GH_SIGNED_OUT: &str = "`gh` is not signed in to GitHub: run `gh auth login` on the computer.";
const GH_TIMED_OUT: &str = "GitHub did not answer within a minute.";

// ---------- running programs ----------

enum RunError {
    Missing,
    TimedOut,
    Failed(String),
}

struct Ran {
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Reads a stream to its end (or `MAX_OUTPUT`) on a thread of its own; the text comes through the channel.
fn drain<R: Read + Send + 'static>(stream: Option<R>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(stream) = stream {
            let _ = stream.take(MAX_OUTPUT).read_to_end(&mut bytes);
        }
        let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    rx
}

/// Runs `program` in `cwd` with no input, its output read and bounded, and killed when `limit` passes. `env` is set and `unset` removed.
fn run(program: &str, args: &[String], cwd: &Path, env: &[(&str, &str)], unset: &[&str], limit: Duration) -> Result<Ran, RunError> {
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in env {
        cmd.env(key, value);
    }
    for key in unset {
        cmd.env_remove(key);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { RunError::Missing } else { RunError::Failed(e.to_string()) })?;
    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());
    let until = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() > until => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(15)),
            Err(e) => return Err(RunError::Failed(e.to_string())),
        }
    };
    Ok(Ran { ok: status.success(), stdout: out.recv_timeout(DRAIN_WAIT).unwrap_or_default(), stderr: err.recv_timeout(DRAIN_WAIT).unwrap_or_default() })
}

/// `git` in `dir`, read only, as the commit watcher runs it (`automations::head_of`). Its output on success.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let mut all: Vec<String> = ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false"].iter().map(|s| s.to_string()).collect();
    all.extend(args.iter().map(|s| s.to_string()));
    let env = [("GIT_OPTIONAL_LOCKS", "0"), ("GIT_TERMINAL_PROMPT", "0"), ("LC_ALL", "C")];
    match run("git", &all, dir, &env, &["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"], GIT_LIMIT) {
        Ok(ran) if ran.ok => Ok(ran.stdout),
        Ok(ran) => Err(one_line(&ran.stderr, MAX_MESSAGE)),
        Err(RunError::Missing) => Err("`git` is not installed here, or not on the computer's PATH.".to_string()),
        Err(RunError::TimedOut) => Err("`git` did not answer in time.".to_string()),
        Err(RunError::Failed(e)) => Err(one_line(&e, MAX_MESSAGE)),
    }
}

/// `gh` from `neutral` (the daemon's data folder), with prompts, colour and the user's repository and host overrides out of the way.
fn gh(neutral: &Path, args: Vec<String>) -> Result<Ran, String> {
    let env = [("GH_PROMPT_DISABLED", "1"), ("NO_COLOR", "1"), ("GH_NO_UPDATE_NOTIFIER", "1"), ("GH_SPINNER_DISABLED", "1")];
    match run("gh", &args, neutral, &env, &["GH_REPO", "GH_HOST", "GH_FORCE_TTY", "GH_PAGER", "PAGER"], GH_LIMIT) {
        Ok(ran) => Ok(ran),
        Err(RunError::Missing) => Err(GH_MISSING.to_string()),
        Err(RunError::TimedOut) => Err(GH_TIMED_OUT.to_string()),
        Err(RunError::Failed(e)) => Err(one_line(&e, MAX_MESSAGE)),
    }
}

/// `gh`'s refusal as one short line a phone can show; a sign-in problem says what to do.
fn gh_refusal(ran: &Ran) -> String {
    let text = if ran.stderr.trim().is_empty() { &ran.stdout } else { &ran.stderr };
    let lower = text.to_ascii_lowercase();
    if lower.contains("gh auth login") || lower.contains("not logged in") || lower.contains("http 401") {
        return GH_SIGNED_OUT.to_string();
    }
    one_line(text, MAX_MESSAGE)
}

/// Text from a program or from GitHub as one line without control characters, at most `limit` characters.
pub fn one_line(text: &str, limit: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let clean: String = joined.chars().filter(|c| !c.is_control()).collect();
    if clean.chars().count() <= limit {
        return clean;
    }
    let mut cut: String = clean.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// `message` with every mention of the project's `folder` taken out: a phone is never told where a project is.
fn without_folder(message: &str, folder: &str) -> String {
    let mut out = message.to_string();
    for form in [folder.to_string(), folder.replace('\\', "/"), folder.replace('/', "\\")] {
        if !form.is_empty() {
            out = out.replace(&form, "the project's folder");
        }
    }
    out
}

// ---------- the repository ----------

/// `owner/name` of a github.com remote (https, ssh or scp-like); `None` for any other host or shape. Credentials in the URL are dropped.
pub fn parse_github_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let path = if let Some(rest) = url.strip_prefix("git@github.com:") {
        rest
    } else if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        rest
    } else if let Some(rest) = url.strip_prefix("https://") {
        let (authority, path) = rest.split_once('/')?;
        let host = authority.rsplit('@').next()?;
        if !host.eq_ignore_ascii_case(HOST) {
            return None;
        }
        path
    } else {
        return None;
    };
    let path = path.split(|c: char| c == '?' || c == '#').next()?.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (valid_part(owner) && valid_part(name)).then(|| format!("{owner}/{name}"))
}

fn valid_part(part: &str) -> bool {
    !part.is_empty() && part.len() <= 100 && part != "." && part != ".." && !part.starts_with('-') && part.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Whether `repo` is an `owner/name` as `parse_github_remote` gives it.
pub fn valid_repo(repo: &str) -> bool {
    repo.split_once('/').is_some_and(|(owner, name)| valid_part(owner) && valid_part(name))
}

/// What `gh -R` is given: the host is always spelled out, so a `GH_HOST` of the user's cannot move the call elsewhere.
fn repo_arg(repo: &str) -> String {
    format!("{HOST}/{repo}")
}

/// The repository a project's `origin` names, read now from its folder.
pub fn repo_of(folder: &str) -> Result<String, String> {
    let dir = Path::new(folder);
    if !dir.is_dir() {
        return Err("The project's folder is not on this computer any more.".to_string());
    }
    let remote = git(dir, &["remote", "get-url", "origin"]).map_err(|why| without_folder(&why, folder))?;
    parse_github_remote(remote.lines().next().unwrap_or_default()).ok_or_else(|| "This project's `origin` is not a repository on github.com.".to_string())
}

/// The branch the project's folder is on, `None` when it is detached or has no commit.
fn branch_of(folder: &str) -> Result<String, String> {
    let out = git(Path::new(folder), &["rev-parse", "--abbrev-ref", "HEAD"]).map_err(|why| without_folder(&why, folder))?;
    let branch = out.trim();
    if branch.is_empty() || branch == "HEAD" || branch.starts_with('-') || branch.chars().any(char::is_control) {
        return Err("This project is not on a branch, so it has no pull request.".to_string());
    }
    Ok(branch.to_string())
}

// ---------- what GitHub says ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckResult {
    Passed,
    Failed,
    Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub name: String,
    pub result: CheckResult,
}

/// A pull request as `gh pr view --json` gives it, cleaned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub url: String,
    /// `OPEN`, `CLOSED` or `MERGED`.
    pub state: String,
    pub draft: bool,
    pub base: String,
    /// The head commit.
    pub head: String,
    /// `MERGEABLE`, `CONFLICTING` or `UNKNOWN`.
    pub mergeable: String,
    /// GitHub's `mergeStateStatus`: `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`, `DRAFT`, `HAS_HOOKS`, `UNSTABLE` or `UNKNOWN`.
    pub merge_state: String,
    pub checks: Vec<Check>,
}

/// A commit id as git writes it: 40 hex digits (64 in a repository that uses SHA-256).
pub fn valid_head(head: &str) -> bool {
    (head.len() == 40 || head.len() == 64) && head.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn check_of(entry: &Value) -> Option<Check> {
    // A status context (an older kind of check) names itself `context` and says `state`; a check run says `name`, `status` and `conclusion`.
    if entry["__typename"] == "StatusContext" || entry.get("context").is_some() {
        let name = entry["context"].as_str()?;
        let result = match entry["state"].as_str()? {
            "SUCCESS" => CheckResult::Passed,
            "FAILURE" | "ERROR" => CheckResult::Failed,
            _ => CheckResult::Pending,
        };
        return Some(Check { name: one_line(name, 120), result });
    }
    let name = entry["name"].as_str()?;
    let result = if entry["status"].as_str() != Some("COMPLETED") {
        CheckResult::Pending
    } else {
        match entry["conclusion"].as_str().unwrap_or_default() {
            "SUCCESS" | "NEUTRAL" | "SKIPPED" => CheckResult::Passed,
            "" => CheckResult::Pending,
            _ => CheckResult::Failed,
        }
    };
    Some(Check { name: one_line(name, 120), result })
}

/// What `gh pr view --json <VIEW_FIELDS>` printed.
pub fn parse_pull(text: &str) -> Result<Pull, String> {
    let unreadable = || "GitHub's answer about the pull request could not be read.".to_string();
    let v: Value = serde_json::from_str(text).map_err(|_| unreadable())?;
    let number = v["number"].as_u64().filter(|n| *n > 0).ok_or_else(unreadable)?;
    let head = v["headRefOid"].as_str().filter(|h| valid_head(h)).ok_or_else(unreadable)?.to_string();
    let word = |key: &str| one_line(v[key].as_str().unwrap_or_default(), 40);
    let url = v["url"].as_str().filter(|u| u.starts_with("https://github.com/")).map(|u| one_line(u, 300)).unwrap_or_default();
    let checks: Vec<Check> = v["statusCheckRollup"].as_array().map(|all| all.iter().filter_map(check_of).collect()).unwrap_or_default();
    Ok(Pull {
        number,
        title: one_line(v["title"].as_str().unwrap_or_default(), 200),
        url,
        state: word("state"),
        draft: v["isDraft"].as_bool().unwrap_or(false),
        base: one_line(v["baseRefName"].as_str().unwrap_or_default(), 100),
        head,
        mergeable: word("mergeable"),
        merge_state: word("mergeStateStatus"),
        checks,
    })
}

/// The names `gh pr checks --required --json name` printed. Anything else (an error's text, no output) is none.
pub fn parse_required(text: &str) -> Vec<String> {
    let Ok(Value::Array(all)) = serde_json::from_str::<Value>(text) else { return Vec::new() };
    let mut names: Vec<String> = all.iter().filter_map(|c| c["name"].as_str()).map(|n| one_line(n, 120)).collect();
    names.sort();
    names.dedup();
    names
}

/// The required checks that failed, and those still running or not reported at all.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Required {
    pub failing: Vec<String>,
    pub running: Vec<String>,
}

pub fn required_state(checks: &[Check], required: &[String]) -> Required {
    let mut state = Required::default();
    for name in required {
        let (mut seen, mut failed, mut pending) = (false, false, false);
        for check in checks.iter().filter(|c| &c.name == name) {
            seen = true;
            match check.result {
                CheckResult::Failed => failed = true,
                CheckResult::Pending => pending = true,
                CheckResult::Passed => {}
            }
        }
        if failed {
            state.failing.push(name.clone());
        } else if pending || !seen {
            state.running.push(name.clone());
        }
    }
    state
}

// ---------- the line ----------

/// Whether a pull request can be merged, and if not, the first reason that holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub can: bool,
    /// `unknown`, `not_open`, `draft`, `conflicts`, `checks_failing`, `checks_running` or `blocked`; none when it can.
    pub reason: Option<&'static str>,
    pub detail: String,
}

impl Line {
    fn can() -> Line {
        Line { can: true, reason: None, detail: String::new() }
    }

    fn cannot(reason: &'static str, detail: impl Into<String>) -> Line {
        Line { can: false, reason: Some(reason), detail: detail.into() }
    }

    pub fn to_json(&self) -> Value {
        json!({"can": self.can, "reason": self.reason, "detail": self.detail})
    }

    /// The line as a sentence, for a refusal or the audit log.
    pub fn sentence(&self) -> String {
        match (self.can, self.reason) {
            (true, _) => "it can be merged".to_string(),
            (false, Some(reason)) if !self.detail.is_empty() => format!("it cannot be merged ({}): {}", reason.replace('_', " "), self.detail),
            (false, Some(reason)) => format!("it cannot be merged ({})", reason.replace('_', " ")),
            (false, None) => "it cannot be merged".to_string(),
        }
    }
}

fn names(list: &[String]) -> String {
    let shown: Vec<&str> = list.iter().take(3).map(String::as_str).collect();
    let more = list.len().saturating_sub(shown.len());
    if more > 0 {
        format!("{} and {more} more", shown.join(", "))
    } else {
        shown.join(", ")
    }
}

/// What `pull` comes to, in this order: not open, a draft, conflicts, a required check that failed, one still running, then GitHub's own merge state.
/// A check that is not required never decides; with none known as required, GitHub's merge state is all there is.
pub fn merge_line(pull: &Pull, required: &[String]) -> Line {
    if pull.state != "OPEN" {
        return Line::cannot("not_open", pull.state.to_ascii_lowercase());
    }
    if pull.draft || pull.merge_state == "DRAFT" {
        return Line::cannot("draft", "");
    }
    if pull.mergeable == "CONFLICTING" || pull.merge_state == "DIRTY" {
        return Line::cannot("conflicts", format!("with {}", pull.base));
    }
    let state = required_state(&pull.checks, required);
    if !state.failing.is_empty() {
        return Line::cannot("checks_failing", names(&state.failing));
    }
    if !state.running.is_empty() {
        return Line::cannot("checks_running", names(&state.running));
    }
    match pull.merge_state.as_str() {
        // Unstable: only checks that are not required are not passing.
        "CLEAN" | "HAS_HOOKS" | "UNSTABLE" => Line::can(),
        "BEHIND" => Line::cannot("blocked", format!("the branch is behind {}", pull.base)),
        "BLOCKED" => Line::cannot("blocked", "GitHub wants something more first, such as a review"),
        _ => Line::cannot("unknown", "GitHub has not worked out yet whether it merges"),
    }
}

/// What `pr.status` answers for a pull request that exists.
pub fn status_json(repo: &str, pull: &Pull, line: &Line) -> Value {
    json!({"repo": repo, "number": pull.number, "title": pull.title, "url": pull.url, "base": pull.base, "head": pull.head, "line": line.to_json()})
}

// ---------- reading ----------

/// The pull request of `selector` (a branch or a number) in `repo` and the checks branch protection requires of it; `None` when there is none.
fn read_pull(repo: &str, selector: &str, neutral: &Path) -> Result<Option<(Pull, Vec<String>)>, String> {
    let view = vec!["pr".to_string(), "view".to_string(), selector.to_string(), "-R".to_string(), repo_arg(repo), "--json".to_string(), VIEW_FIELDS.to_string()];
    let ran = gh(neutral, view)?;
    if !ran.ok {
        if ran.stderr.to_ascii_lowercase().contains("no pull requests found") {
            return Ok(None);
        }
        return Err(gh_refusal(&ran));
    }
    let pull = parse_pull(&ran.stdout)?;
    // Its exit code says whether the checks are all passing, so it is the output that counts. Any failure to read is "none required".
    let checks = vec!["pr".to_string(), "checks".to_string(), pull.number.to_string(), "-R".to_string(), repo_arg(repo), "--required".to_string(), "--json".to_string(), "name".to_string()];
    let required = gh(neutral, checks).map(|ran| parse_required(&ran.stdout)).unwrap_or_default();
    Ok(Some((pull, required)))
}

/// `pr.status` for the project in `folder`: the pull request of the branch it is on, with the line; `{none: true}` when the branch has none.
/// An error is a plain message, with no folder in it.
pub fn status(folder: &str, neutral: &Path) -> Result<Value, String> {
    let read = || -> Result<Value, String> {
        let repo = repo_of(folder)?;
        let branch = branch_of(folder)?;
        Ok(match read_pull(&repo, &branch, neutral)? {
            Some((pull, required)) => status_json(&repo, &pull, &merge_line(&pull, &required)),
            None => json!({"none": true}),
        })
    };
    read().map_err(|why| without_folder(&why, folder))
}

// ---------- merging ----------

/// How `gh pr merge` may merge, as its flags name them.
pub const METHODS: [&str; 3] = ["squash", "merge", "rebase"];

/// `method` as one of `METHODS`, or `None`.
pub fn merge_method(name: &str) -> Option<&'static str> {
    METHODS.iter().copied().find(|m| *m == name)
}

/// What one merge is bound to: the repository, the pull request, the head commit the person was shown, and how to merge. The passkey's
/// assertion covers all four (`bound_command`), and the daemon reads the pull request again before it acts on any of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bound {
    pub repo: String,
    pub number: u64,
    pub head: String,
    pub method: &'static str,
}

/// What the passkey is asked to sign for a merge, as one line: the host, the repository, the pull request, the head commit and the method.
/// The phone builds the same line from what it shows (`app/phone/src/merge.ts`), so a daemon that described another merge gets an assertion
/// that does not check.
pub fn bound_command(b: &Bound) -> String {
    format!("merge {HOST} {} #{} {} {}", b.repo, b.number, b.head, b.method)
}

/// The request id a merge's assertion is over. It cannot be any other kind's: a dangerous approval's is a number, a screen's starts `screen:`.
pub fn challenge_request(nonce: &str) -> String {
    format!("merge:{nonce}")
}

/// The arguments `gh` is run with to merge. The repository is spelled with its host, the head commit must still be the one named
/// (`--match-head-commit`: GitHub refuses when it moved), and `--admin` is never passed, so branch protection holds for a phone too.
pub fn merge_args(b: &Bound) -> Vec<String> {
    vec![
        "pr".to_string(),
        "merge".to_string(),
        b.number.to_string(),
        "-R".to_string(),
        repo_arg(&b.repo),
        "--match-head-commit".to_string(),
        b.head.clone(),
        format!("--{}", b.method),
    ]
}

/// Why `pull`, as just read from `repo` with the checks `required` of it, may not be merged as `b` says; `Ok` when it may. It must be the
/// repository and the pull request that were bound, open and not a draft, at the head commit that was bound, with every required check passed,
/// and GitHub's own merge state must allow it.
pub fn merge_refusal(repo: &str, pull: &Pull, required: &[String], b: &Bound) -> Result<(), String> {
    if repo != b.repo {
        return Err("This project's repository is not the one you confirmed.".to_string());
    }
    if pull.number != b.number {
        return Err("That is not the pull request you confirmed.".to_string());
    }
    if pull.state != "OPEN" {
        return Err(format!("The pull request is not open ({}).", pull.state.to_ascii_lowercase()));
    }
    if pull.draft {
        return Err("The pull request is a draft.".to_string());
    }
    if pull.head != b.head {
        return Err("The pull request has new commits since you looked: look again before merging.".to_string());
    }
    let state = required_state(&pull.checks, required);
    if !state.failing.is_empty() {
        return Err(format!("A required check failed: {}.", names(&state.failing)));
    }
    if !state.running.is_empty() {
        return Err(format!("A required check has not passed yet: {}.", names(&state.running)));
    }
    let line = merge_line(pull, required);
    if !line.can {
        return Err(format!("GitHub does not allow it now: {}.", line.sentence()));
    }
    Ok(())
}

/// Merges `b`'s pull request of the project in `folder`: reads the project's repository and the pull request again, refuses unless
/// `merge_refusal` says it may be merged, then has `gh` merge it. `Err` is GitHub's own refusal or ours, one line without the folder.
pub fn merge_now(folder: &str, neutral: &Path, b: &Bound) -> Result<(), String> {
    let attempt = || -> Result<(), String> {
        let repo = repo_of(folder)?;
        if repo != b.repo {
            return Err("This project's repository is not the one you confirmed.".to_string());
        }
        let (pull, required) = read_pull(&repo, &b.number.to_string(), neutral)?.ok_or_else(|| "That pull request is gone.".to_string())?;
        merge_refusal(&repo, &pull, &required, b)?;
        let ran = gh(neutral, merge_args(b))?;
        if ran.ok {
            Ok(())
        } else {
            Err(gh_refusal(&ran))
        }
    };
    attempt().map_err(|why| without_folder(&why, folder))
}

/// The audit line for a merge attempt: what was asked for, by which phone, and what came of it.
pub fn merge_summary(by: &str, b: &Bound, result: &Result<(), String>) -> String {
    let outcome = match result {
        Ok(()) => "merged".to_string(),
        Err(why) => format!("refused: {}", one_line(why, MAX_MESSAGE)),
    };
    format!("{} #{} head {} ({}) by {}: {outcome}", b.repo, b.number, b.head, b.method, one_line(by, 60))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn pull(state: &str, draft: bool, mergeable: &str, merge_state: &str, checks: &[(&str, CheckResult)]) -> Pull {
        Pull {
            number: 12,
            title: "Fix the thing".into(),
            url: "https://github.com/o/n/pull/12".into(),
            state: state.into(),
            draft,
            base: "master".into(),
            head: HEAD.into(),
            mergeable: mergeable.into(),
            merge_state: merge_state.into(),
            checks: checks.iter().map(|(name, result)| Check { name: name.to_string(), result: *result }).collect(),
        }
    }

    fn required(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn a_github_remote_is_read_in_every_form_and_nothing_else_is() {
        for url in [
            "https://github.com/o/n",
            "https://github.com/o/n.git",
            "https://github.com/o/n.git/",
            "https://github.com/o/n?tab=readme#x",
            "https://user:secret@github.com/o/n.git",
            "https://GitHub.com/o/n",
            "git@github.com:o/n.git",
            "git@github.com:o/n",
            "ssh://git@github.com/o/n.git",
            "  https://github.com/o/n.git\n",
        ] {
            assert_eq!(parse_github_remote(url).as_deref(), Some("o/n"), "{url:?}");
        }
        assert_eq!(parse_github_remote("https://github.com/Some-Org/my_repo.v2").as_deref(), Some("Some-Org/my_repo.v2"));
        for url in [
            "",
            "https://gitlab.com/o/n.git",
            "https://github.com.evil.example/o/n",
            "https://evil.example/github.com/o/n",
            "https://github.com@evil.example/o/n",
            "http://github.com/o/n",
            "git://github.com/o/n",
            "git@gitlab.com:o/n.git",
            "git@github.com.evil.example:o/n.git",
            "ssh://git@github.com:22/o/n.git",
            "https://github.com/o",
            "https://github.com/o/n/extra",
            "https://github.com/../n",
            "https://github.com/o/..",
            "https://github.com/-o/n",
            "https://github.com/o/n x",
            "C:/work/shop",
            "/home/me/shop",
            "../shop",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url:?}");
        }
        assert!(valid_repo("o/n") && !valid_repo("o") && !valid_repo("o/n/x") && !valid_repo("o/n m") && !valid_repo("/n") && !valid_repo("o/"));
        assert_eq!(repo_arg("o/n"), "github.com/o/n");
    }

    #[test]
    fn a_pull_request_is_read_from_what_gh_prints() {
        let text = json!({
            "number": 12, "title": "Fix\tthe\nthing", "url": "https://github.com/o/n/pull/12", "state": "OPEN", "isDraft": false, "baseRefName": "master",
            "headRefOid": HEAD, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN",
            "statusCheckRollup": [
                {"__typename": "CheckRun", "name": "build", "status": "COMPLETED", "conclusion": "SUCCESS"},
                {"__typename": "CheckRun", "name": "test", "status": "IN_PROGRESS", "conclusion": ""},
                {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "FAILURE"},
                {"__typename": "CheckRun", "name": "docs", "status": "COMPLETED", "conclusion": "SKIPPED"},
                {"__typename": "StatusContext", "context": "ci/legacy", "state": "PENDING"},
                {"__typename": "StatusContext", "context": "ci/other", "state": "ERROR"},
                {"__typename": "Unknown"},
            ],
        })
        .to_string();
        let p = parse_pull(&text).unwrap();
        assert_eq!((p.number, p.title.as_str(), p.state.as_str(), p.draft, p.base.as_str(), p.head.as_str()), (12, "Fix the thing", "OPEN", false, "master", HEAD));
        assert_eq!((p.mergeable.as_str(), p.merge_state.as_str(), p.url.as_str()), ("MERGEABLE", "CLEAN", "https://github.com/o/n/pull/12"));
        let results: Vec<(&str, CheckResult)> = p.checks.iter().map(|c| (c.name.as_str(), c.result)).collect();
        assert_eq!(
            results,
            [("build", CheckResult::Passed), ("test", CheckResult::Pending), ("lint", CheckResult::Failed), ("docs", CheckResult::Passed), ("ci/legacy", CheckResult::Pending), ("ci/other", CheckResult::Failed)]
        );
        // Nothing readable is a refusal, never a pull request that can be merged; a head commit that is not a commit id is not believed.
        let short = json!({"number": 1, "headRefOid": "main"}).to_string();
        let upper = json!({"number": 1, "headRefOid": HEAD.to_uppercase()}).to_string();
        for bad in ["", "not json", "[]", "{}", r#"{"number": 0}"#, short.as_str(), upper.as_str()] {
            assert!(parse_pull(bad).is_err(), "{bad}");
        }
        // A url that is not GitHub's is dropped rather than shown.
        let odd = parse_pull(&json!({"number": 1, "headRefOid": HEAD, "url": "javascript:alert(1)"}).to_string()).unwrap();
        assert_eq!(odd.url, "");
        assert!(odd.checks.is_empty() && !odd.draft);
    }

    #[test]
    fn the_required_checks_are_the_names_gh_lists_and_a_failure_to_read_is_none() {
        assert_eq!(parse_required(r#"[{"name": "test"}, {"name": "build"}, {"name": "test"}]"#), ["build", "test"]);
        for none in ["", "[]", "no required checks reported on the 'x' branch", "{\"message\": \"bad\"}", "[{\"state\": \"x\"}]"] {
            assert!(parse_required(none).is_empty(), "{none:?}");
        }
    }

    #[test]
    fn the_line_follows_githubs_merge_state_and_the_required_checks_only() {
        let passed = [("build", CheckResult::Passed), ("test", CheckResult::Passed)];
        let line = |p: &Pull, req: &[&str]| merge_line(p, &required(req));
        let ok = line(&pull("OPEN", false, "MERGEABLE", "CLEAN", &passed), &["build", "test"]);
        assert_eq!((ok.can, ok.reason, ok.detail.as_str()), (true, None, ""));
        // Not open comes first, whatever else is true.
        for state in ["MERGED", "CLOSED"] {
            let l = line(&pull(state, true, "CONFLICTING", "DIRTY", &[("build", CheckResult::Failed)]), &["build"]);
            assert_eq!((l.can, l.reason, l.detail.as_str()), (false, Some("not_open"), state.to_ascii_lowercase().as_str()));
        }
        assert_eq!(line(&pull("OPEN", true, "MERGEABLE", "CLEAN", &passed), &[]).reason, Some("draft"));
        assert_eq!(line(&pull("OPEN", false, "MERGEABLE", "DRAFT", &passed), &[]).reason, Some("draft"));
        let conflicts = line(&pull("OPEN", false, "CONFLICTING", "DIRTY", &passed), &[]);
        assert_eq!((conflicts.reason, conflicts.detail.as_str()), (Some("conflicts"), "with master"));
        assert_eq!(line(&pull("OPEN", false, "MERGEABLE", "DIRTY", &passed), &[]).reason, Some("conflicts"));
        // A required check that failed, then one that runs or has not reported, both ahead of what GitHub's state says.
        let mixed = [("build", CheckResult::Failed), ("test", CheckResult::Pending), ("lint", CheckResult::Passed)];
        let failing = line(&pull("OPEN", false, "MERGEABLE", "BLOCKED", &mixed), &["build", "test", "lint"]);
        assert_eq!((failing.reason, failing.detail.as_str()), (Some("checks_failing"), "build"));
        let running = line(&pull("OPEN", false, "MERGEABLE", "BLOCKED", &mixed), &["test", "lint"]);
        assert_eq!((running.reason, running.detail.as_str()), (Some("checks_running"), "test"));
        let unreported = line(&pull("OPEN", false, "MERGEABLE", "BLOCKED", &passed), &["build", "deploy"]);
        assert_eq!((unreported.reason, unreported.detail.as_str()), (Some("checks_running"), "deploy"));
        // A check that is not required never decides: GitHub's own state does.
        let optional = line(&pull("OPEN", false, "MERGEABLE", "UNSTABLE", &mixed), &["lint"]);
        assert!(optional.can, "{optional:?}");
        let none_required = line(&pull("OPEN", false, "MERGEABLE", "BLOCKED", &mixed), &[]);
        assert_eq!((none_required.can, none_required.reason), (false, Some("blocked")));
        assert!(line(&pull("OPEN", false, "MERGEABLE", "HAS_HOOKS", &[]), &[]).can);
        assert_eq!(line(&pull("OPEN", false, "MERGEABLE", "BEHIND", &passed), &[]).reason, Some("blocked"));
        // GitHub has not said, or said something this does not know: never "can merge".
        for state in ["UNKNOWN", "", "SOMETHING_NEW"] {
            assert_eq!(line(&pull("OPEN", false, "UNKNOWN", state, &passed), &[]).reason, Some("unknown"), "{state:?}");
        }
        // Many failing names are cut to three.
        let many: Vec<(&str, CheckResult)> = ["a", "b", "c", "d", "e"].iter().map(|n| (*n, CheckResult::Failed)).collect();
        let l = line(&pull("OPEN", false, "MERGEABLE", "BLOCKED", &many), &["a", "b", "c", "d", "e"]);
        assert_eq!(l.detail, "a, b, c and 2 more");
        // The line's JSON, as phones read it.
        assert_eq!(ok.to_json(), json!({"can": true, "reason": null, "detail": ""}));
        assert_eq!(conflicts.to_json(), json!({"can": false, "reason": "conflicts", "detail": "with master"}));
        assert!(conflicts.sentence().contains("conflicts") && ok.sentence().contains("can be merged"));
    }

    #[test]
    fn what_goes_to_a_phone_is_one_short_line_and_never_a_folder() {
        assert_eq!(one_line("  a \n\t b\u{7}c  ", 50), "a bc");
        assert_eq!(one_line(&"x".repeat(300), 10).chars().count(), 10);
        assert!(one_line(&"x".repeat(300), 10).ends_with('…'));
        assert_eq!(without_folder("fatal: cannot change to 'C:\\work\\shop'", "C:\\work\\shop"), "fatal: cannot change to 'the project's folder'");
        assert_eq!(without_folder("cannot open C:/work/shop/objects", "C:\\work\\shop"), "cannot open the project's folder/objects");
        let p = pull("OPEN", false, "MERGEABLE", "CLEAN", &[]);
        let shown = status_json("o/n", &p, &merge_line(&p, &[]));
        assert_eq!(shown["repo"], "o/n");
        assert_eq!((shown["number"].as_u64(), shown["head"].as_str(), shown["base"].as_str()), (Some(12), Some(HEAD), Some("master")));
        assert_eq!(shown["line"]["can"], true);
        let keys: Vec<&str> = shown.as_object().unwrap().keys().map(String::as_str).collect();
        assert!(!keys.iter().any(|k| k.contains("path") || k.contains("folder") || k.contains("cwd")), "{keys:?}");
    }

    fn bound(method: &'static str) -> Bound {
        Bound { repo: "o/n".into(), number: 12, head: HEAD.into(), method }
    }

    #[test]
    fn a_merge_method_is_one_of_three_flags_and_nothing_else() {
        for method in ["squash", "merge", "rebase"] {
            assert_eq!(merge_method(method), Some(method));
        }
        for other in ["", "Squash", "squash ", "--admin", "auto", "merge --admin", "fast-forward"] {
            assert_eq!(merge_method(other), None, "{other:?}");
        }
    }

    #[test]
    fn the_command_the_passkey_covers_names_the_host_the_repository_the_pull_request_the_head_and_the_method() {
        let base = bound_command(&bound("squash"));
        assert_eq!(base, format!("merge github.com o/n #12 {HEAD} squash"));
        let mut other = bound("squash");
        other.repo = "o/m".into();
        assert_ne!(bound_command(&other), base);
        let mut other = bound("squash");
        other.number = 13;
        assert_ne!(bound_command(&other), base);
        let mut other = bound("squash");
        other.head = "f".repeat(40);
        assert_ne!(bound_command(&other), base);
        assert_ne!(bound_command(&bound("rebase")), base);
        // Its request id is a kind of its own: not a number (a dangerous approval's), nor a screen's.
        assert_eq!(challenge_request("abc"), "merge:abc");
    }

    #[test]
    fn gh_is_given_the_repository_with_its_host_the_head_to_match_and_never_admin() {
        for method in METHODS {
            let args = merge_args(&bound(method));
            let flag = format!("--{method}");
            assert_eq!(args, ["pr", "merge", "12", "-R", "github.com/o/n", "--match-head-commit", HEAD, flag.as_str()]);
            for banned in ["--admin", "--auto", "--delete-branch", "--body", "--subject"] {
                assert!(!args.iter().any(|a| a == banned), "{method}: {banned}");
            }
        }
    }

    #[test]
    fn a_merge_is_refused_unless_it_is_the_bound_pull_request_open_unchanged_and_allowed() {
        let passed = [("build", CheckResult::Passed), ("test", CheckResult::Passed)];
        let may = |p: &Pull, req: &[&str]| merge_refusal("o/n", p, &required(req), &bound("squash"));
        let good = pull("OPEN", false, "MERGEABLE", "CLEAN", &passed);
        assert_eq!(may(&good, &["build", "test"]), Ok(()));
        assert_eq!(may(&good, &[]), Ok(()), "none required: GitHub's state is all there is");
        // What was bound.
        assert!(merge_refusal("o/other", &good, &[], &bound("squash")).unwrap_err().contains("repository"));
        let mut moved = good.clone();
        moved.number = 13;
        assert!(may(&moved, &[]).unwrap_err().contains("not the pull request"));
        let mut newer = good.clone();
        newer.head = "f".repeat(40);
        assert!(may(&newer, &[]).unwrap_err().contains("new commits"));
        // Open and not a draft.
        for state in ["MERGED", "CLOSED"] {
            assert!(may(&pull(state, false, "MERGEABLE", "CLEAN", &passed), &[]).unwrap_err().contains("not open"), "{state}");
        }
        assert!(may(&pull("OPEN", true, "MERGEABLE", "CLEAN", &passed), &[]).unwrap_err().contains("draft"));
        // Every required check passed: one that failed, one still running, one that never reported.
        let failed = pull("OPEN", false, "MERGEABLE", "CLEAN", &[("build", CheckResult::Passed), ("test", CheckResult::Failed)]);
        assert!(may(&failed, &["build", "test"]).unwrap_err().contains("failed: test"));
        let running = pull("OPEN", false, "MERGEABLE", "CLEAN", &[("build", CheckResult::Passed), ("test", CheckResult::Pending)]);
        assert!(may(&running, &["build", "test"]).unwrap_err().contains("not passed yet: test"));
        assert!(may(&good, &["build", "deploy"]).unwrap_err().contains("not passed yet: deploy"));
        // GitHub's own state says no, or has not said: no, even with every required check green.
        for state in ["BLOCKED", "BEHIND", "DIRTY", "UNKNOWN", ""] {
            assert!(may(&pull("OPEN", false, "MERGEABLE", state, &passed), &["build", "test"]).is_err(), "{state:?}");
        }
        assert!(may(&pull("OPEN", false, "CONFLICTING", "CLEAN", &passed), &[]).is_err());
        // A check that is not required and failed does not stop what GitHub allows.
        let optional = pull("OPEN", false, "MERGEABLE", "UNSTABLE", &[("build", CheckResult::Passed), ("lint", CheckResult::Failed)]);
        assert_eq!(may(&optional, &["build"]), Ok(()));
    }

    #[test]
    fn a_merge_attempt_is_recorded_with_what_was_asked_who_asked_and_what_came_of_it() {
        let merged = merge_summary("Phone", &bound("squash"), &Ok(()));
        assert_eq!(merged, format!("o/n #12 head {HEAD} (squash) by Phone: merged"));
        let refused = merge_summary("Phone", &bound("rebase"), &Err("GitHub says:\nnot mergeable".to_string()));
        assert_eq!(refused, format!("o/n #12 head {HEAD} (rebase) by Phone: refused: GitHub says: not mergeable"));
        assert!(!merge_summary("A\nB", &bound("merge"), &Ok(())).contains('\n'));
    }

    #[test]
    fn a_folder_that_is_gone_is_a_plain_message_without_the_folder() {
        let folder = std::env::temp_dir().join(format!("kelpie-pulls-gone-{}", uuid::Uuid::new_v4())).to_string_lossy().into_owned();
        let why = status(&folder, &std::env::temp_dir()).unwrap_err();
        assert!(why.contains("not on this computer"), "{why}");
        assert!(!why.contains(&folder));
    }
}
