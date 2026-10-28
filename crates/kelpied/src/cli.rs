//! Friendly command line over the daemon RPCs, for scripts and for agents that drive other
//! sessions:
//!
//!   kelpied terminal list [--json]
//!   kelpied terminal read <session> [--lines N]
//!   kelpied terminal send <session> <text…> [--enter]
//!   kelpied terminal wait <session> [--timeout-ms N]      until it stops working
//!   kelpied worktree get [--path P] [--json]
//!   kelpied worktree set [--path P] [--comment TEXT] [--status todo|progress|review|done]
//!   kelpied review add <file>:<line> "<text>" [--old] [--base REF]    a review note on this worktree's diff
//!   kelpied review list [--json]
//!   kelpied run create|list|show|approve|retry|done|skip|cancel|delete|report|wait …  (see `run`)
//!
//! `<session>` is a handle number, a session id (or its prefix) or a session name.

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::client::Client;

/// `--name value` from `args`, removing both.
fn take_opt(args: &mut Vec<String>, name: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    args.remove(i);
    (i < args.len()).then(|| args.remove(i))
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != name);
    args.len() != before
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Resolves a session reference against `session.list`.
fn find_session<'a>(list: &'a [Value], target: &str) -> Result<&'a Value> {
    let t = target.trim();
    if let Ok(h) = t.parse::<u64>() {
        if let Some(s) = list.iter().find(|s| s["handle"].as_u64() == Some(h)) {
            return Ok(s);
        }
    }
    let matches: Vec<&Value> = list
        .iter()
        .filter(|s| str_of(s, "id").starts_with(t) || str_of(s, "name").eq_ignore_ascii_case(t))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one),
        [] => bail!("no session matches `{t}` (see `kelpied terminal list`)"),
        _ => bail!("`{t}` matches {} sessions; use the handle", matches.len()),
    }
}

async fn sessions(c: &mut Client) -> Result<Vec<Value>> {
    c.call("session.list", json!({})).await?.as_array().cloned().ok_or_else(|| anyhow!("bad session.list reply"))
}

pub async fn terminal(args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let json_out = take_flag(&mut args, "--json");
    let sub = if args.is_empty() { String::new() } else { args.remove(0) };
    let mut c = Client::connect().await?;
    match sub.as_str() {
        "list" => {
            let list = sessions(&mut c).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            println!("{:>6}  {:<8}  {:<8}  {:<24}  FOLDER", "HANDLE", "KIND", "STATUS", "NAME / TITLE");
            for s in &list {
                let label = Some(str_of(s, "name")).filter(|n| !n.is_empty()).unwrap_or(str_of(s, "title"));
                let label: String = label.chars().take(24).collect();
                println!("{:>6}  {:<8}  {:<8}  {:<24}  {}", s["handle"], str_of(s, "kind"), str_of(s, "status"), label, str_of(s, "cwd"));
            }
        }
        "read" => {
            let lines = take_opt(&mut args, "--lines").map(|n| n.parse::<u64>()).transpose()?.unwrap_or(80);
            let target = args.first().ok_or_else(|| anyhow!("usage: kelpied terminal read <session> [--lines N]"))?;
            let list = sessions(&mut c).await?;
            let s = find_session(&list, target)?;
            let r = c.call("session.read", json!({"handle": s["handle"], "lines": lines})).await?;
            print!("{}", str_of(&r, "text"));
        }
        "send" => {
            let enter = take_flag(&mut args, "--enter");
            let text = take_opt(&mut args, "--text");
            if args.is_empty() {
                bail!("usage: kelpied terminal send <session> <text…> [--enter]");
            }
            let target = args.remove(0);
            let mut text = text.unwrap_or_else(|| args.join(" "));
            if enter {
                text.push('\r');
            }
            let list = sessions(&mut c).await?;
            let s = find_session(&list, &target)?;
            c.call("session.input", json!({"handle": s["handle"], "data": text})).await?;
        }
        "wait" => {
            let timeout = take_opt(&mut args, "--timeout-ms").map(|n| n.parse::<u64>()).transpose()?.unwrap_or(600_000);
            let target = args.first().ok_or_else(|| anyhow!("usage: kelpied terminal wait <session> [--timeout-ms N]"))?.clone();
            let started = Instant::now();
            let (mut seen_working, mut quiet_polls) = (false, 0);
            loop {
                let list = sessions(&mut c).await?;
                let s = find_session(&list, &target)?;
                let status = str_of(s, "status").to_string();
                if status == "working" {
                    seen_working = true;
                    quiet_polls = 0;
                } else {
                    quiet_polls += 1;
                }
                // Just after a `send` the agent may not have started yet: give it 3 s to.
                if status == "exited" || (quiet_polls >= 3 && (seen_working || started.elapsed() > Duration::from_secs(3))) {
                    println!("{status}");
                    return Ok(());
                }
                if started.elapsed() > Duration::from_millis(timeout) {
                    println!("{status}");
                    bail!("timed out after {timeout} ms");
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        _ => bail!("usage: kelpied terminal list|read|send|wait …"),
    }
    Ok(())
}

pub async fn worktree(args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let json_out = take_flag(&mut args, "--json");
    let sub = if args.is_empty() { String::new() } else { args.remove(0) };
    // Default: the git worktree containing the current folder (that is what a project is),
    // else the folder itself.
    let path = match take_opt(&mut args, "--path") {
        Some(p) => p,
        None => std::process::Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|p| !p.is_empty())
            .unwrap_or(std::env::current_dir()?.to_string_lossy().into_owned()),
    };
    let mut c = Client::connect().await?;
    match sub.as_str() {
        "get" | "current" => {
            let m = c.call("meta.get", json!({"path": path})).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else if m.is_null() {
                println!("(no comment or status for {path})");
            } else {
                println!("status:  {}", m["status"].as_str().unwrap_or("-"));
                println!("comment: {}", m["comment"].as_str().unwrap_or("-"));
            }
        }
        "set" => {
            let comment = take_opt(&mut args, "--comment");
            let status = take_opt(&mut args, "--status");
            if comment.is_none() && status.is_none() {
                bail!("usage: kelpied worktree set [--path P] [--comment TEXT] [--status todo|progress|review|done]");
            }
            // Writes go to the folder's existing record (e.g. the worktree root above a
            // subfolder) when there is one.
            let existing = c.call("meta.get", json!({"path": path})).await?;
            let key = existing.get("path").and_then(Value::as_str).map(str::to_string).unwrap_or(path);
            let mut params = json!({"path": key});
            if let Some(v) = comment {
                params["comment"] = json!(v);
            }
            if let Some(v) = status {
                params["status"] = json!(v);
            }
            let m = c.call("meta.set", params).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&m)?);
            }
        }
        _ => bail!("usage: kelpied worktree get|set …"),
    }
    Ok(())
}

/// `kelpied review …`: review notes on the diff of the worktree this folder is in. A session's
/// programs reach their own worktree only; the daemon fills in which side of the diff the line is
/// on and quotes it (see notes.rs).
///
///   kelpied review add <file>:<line> "<text>" [--old] [--base REF]
///   kelpied review list [--json]
pub async fn review(args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let json_out = take_flag(&mut args, "--json");
    let old = take_flag(&mut args, "--old");
    let base = take_opt(&mut args, "--base");
    let sub = if args.is_empty() { String::new() } else { args.remove(0) };
    let path = std::env::current_dir()?.to_string_lossy().into_owned();
    let mut c = Client::connect().await?;
    match sub.as_str() {
        "add" => {
            let usage = || anyhow!("usage: kelpied review add <file>:<line> \"<text>\" [--old] [--base REF]");
            let target = args.first().ok_or_else(usage)?;
            let (file, line) = target.rsplit_once(':').and_then(|(f, l)| Some((f, l.parse::<u32>().ok()?))).ok_or_else(usage)?;
            let note = args[1..].join(" ");
            if note.trim().is_empty() {
                return Err(usage());
            }
            let mut params = json!({"path": path, "file": file, "line": line, "note": note, "old": old});
            if let Some(b) = base {
                params["base"] = json!(b);
            }
            let r = c.call("note.add", params).await?;
            let n = &r["note"];
            println!("noted {}:{} ({} side): {}", str_of(n, "path"), n["line"], if str_of(n, "side") == "old" { "removed" } else { "new" }, str_of(n, "text").trim());
        }
        "list" => {
            let r = c.call("note.list", json!({"path": path})).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&r)?);
                return Ok(());
            }
            for n in r.as_array().into_iter().flatten() {
                println!("{}:{}{}  {}", str_of(n, "path"), if str_of(n, "side") == "old" { "-" } else { "" }, n["line"], str_of(n, "note").replace('\n', " "));
            }
        }
        _ => bail!("usage: kelpied review add|list …"),
    }
    Ok(())
}

/// `kelpied run …`: orchestration runs (see runs.rs).
///
///   kelpied run create <file.json | ->           a run: {name, objective, cwd, tasks: [...]}
///   kelpied run list [--json]
///   kelpied run show <run> [--json]
///   kelpied run approve|retry|done|skip <run> <task>
///   kelpied run cancel|delete <run>
///   kelpied run report done|failed [summary…]  from inside a task's agent session
///   kelpied run wait <run> [--timeout-ms N]      until the run has finished
pub async fn run(args: &[String]) -> Result<()> {
    let mut args = args.to_vec();
    let json_out = take_flag(&mut args, "--json");
    let timeout = take_opt(&mut args, "--timeout-ms").map(|n| n.parse::<u64>()).transpose()?.unwrap_or(3_600_000);
    let sub = if args.is_empty() { String::new() } else { args.remove(0) };
    let mut c = Client::connect().await?;
    let arg = |i: usize, what: &str| args.get(i).cloned().ok_or_else(|| anyhow!("missing {what} (see `kelpied run` usage)"));
    let print_run = |r: &Value| {
        println!("{}  {}  [{}]", &str_of(r, "id")[..8.min(str_of(r, "id").len())], str_of(r, "name"), str_of(r, "status"));
        for t in r["tasks"].as_array().into_iter().flatten() {
            let deps = t["deps"].as_array().map(|d| d.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",")).unwrap_or_default();
            println!(
                "  {:<12} {:<16} {:<8} {}{}",
                str_of(t, "id"),
                str_of(t, "status"),
                str_of(t, "kind"),
                str_of(t, "title"),
                if deps.is_empty() { String::new() } else { format!("  (after {deps})") }
            );
            if let Some(s) = t["summary"].as_str().or(t["error"].as_str()) {
                println!("               {}", s.lines().next().unwrap_or(""));
            }
        }
    };
    match sub.as_str() {
        "create" => {
            let src = arg(0, "a run file")?;
            let text = if src == "-" {
                let mut s = String::new();
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut s)?;
                s
            } else {
                std::fs::read_to_string(&src)?
            };
            let mut spec: Value = serde_json::from_str(&text).map_err(|e| anyhow!("run file is not valid JSON: {e}"))?;
            if spec.get("cwd").and_then(Value::as_str).is_none_or(str::is_empty) {
                spec["cwd"] = json!(std::env::current_dir()?.to_string_lossy());
            }
            let r = c.call("run.create", spec).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                print_run(&r);
            }
        }
        "list" => {
            let list = c.call("run.list", json!({})).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&list)?);
                return Ok(());
            }
            for r in list.as_array().into_iter().flatten() {
                let tasks = r["tasks"].as_array().map_or(0, Vec::len);
                let done = r["tasks"].as_array().map_or(0, |t| t.iter().filter(|t| t["status"] == "done").count());
                println!("{}  {:<10} {done}/{tasks}  {}", &str_of(r, "id")[..8.min(str_of(r, "id").len())], str_of(r, "status"), str_of(r, "name"));
            }
        }
        "show" => {
            let r = c.call("run.get", json!({"id": arg(0, "a run id")?})).await?;
            if json_out {
                println!("{}", serde_json::to_string_pretty(&r)?);
            } else {
                print_run(&r);
            }
        }
        "approve" | "retry" | "done" | "skip" => {
            let method = match sub.as_str() {
                "done" => "run.task_done",
                "skip" => "run.task_skip",
                s => if s == "approve" { "run.approve" } else { "run.retry" },
            };
            let r = c.call(method, json!({"id": arg(0, "a run id")?, "task": arg(1, "a task id")?})).await?;
            print_run(&r);
        }
        "cancel" | "delete" => {
            c.call(&format!("run.{sub}"), json!({"id": arg(0, "a run id")?})).await?;
        }
        "report" => {
            let status = arg(0, "done or failed")?;
            anyhow::ensure!(status == "done" || status == "failed", "usage: kelpied run report done|failed [summary…]");
            let session = std::env::var("KELPIE_SESSION_ID").map_err(|_| anyhow!("run this from inside the task's agent session (KELPIE_SESSION_ID is not set)"))?;
            let summary = args[1..].join(" ");
            let r = c.call("run.report", json!({"session_id": session, "status": status, "summary": summary})).await?;
            println!("reported {status} for task {} of run {}", str_of(&r, "task"), &str_of(&r, "run")[..8.min(str_of(&r, "run").len())]);
        }
        "wait" => {
            let id = arg(0, "a run id")?;
            let started = Instant::now();
            loop {
                let r = c.call("run.get", json!({"id": id})).await?;
                let status = str_of(&r, "status").to_string();
                if status != "running" {
                    print_run(&r);
                    if status == "done" { return Ok(()) } else { bail!("run {status}") }
                }
                if started.elapsed() > Duration::from_millis(timeout) {
                    bail!("timed out after {timeout} ms");
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        _ => bail!("usage: kelpied run create|list|show|approve|retry|done|skip|cancel|delete|report|wait …"),
    }
    Ok(())
}
