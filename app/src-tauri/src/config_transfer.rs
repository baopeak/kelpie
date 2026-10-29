//! Settings export and import (Settings → Session Daemon → Export and import settings). The window
//! gathers what it owns (settings, quick commands, layouts, agent profiles) and what the daemon keeps
//! (automations); this side adds the saved SSH hosts and the plugin list, and owns the file: its format,
//! the size cap, the field allowlists and the secret filter. Both directions go through the same
//! cleaning, so a file never carries credentials or approvals, and an import never brings in a field
//! Kelpie does not know. Nothing here writes to a path taken from a file.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde_json::{json, Map, Value};

use crate::fs_ops::{blocking, err, CmdResult};
use crate::ssh_hosts::{self, Host, Vault};

pub const KIND: &str = "kelpie-config";
pub const VERSION: u64 = 1;
/// Larger files are refused before they are parsed.
pub const MAX_FILE: u64 = 2 * 1024 * 1024;
const MAX_ITEMS: usize = 1000;
const MAX_TEXT: usize = 64 * 1024;
const MAX_LIST: usize = 256;
const MAX_SETTINGS: usize = 200;
const KEPT_BACKUPS: usize = 5;

pub const SECTIONS: &[&str] = &["settings", "quickCommands", "layouts", "automations", "sshHosts", "plugins", "agents"];

/// The settings keys a file may carry. One list for the window and this side
/// (`app/src/lib/settings-transfer.json`); a unit test there makes every new setting be classified.
static SETTINGS_EXPORTED: LazyLock<Vec<String>> = LazyLock::new(|| {
    let lists: Value = serde_json::from_str(include_str!("../../src/lib/settings-transfer.json")).expect("settings-transfer.json");
    lists["exported"].as_array().expect("exported list").iter().filter_map(Value::as_str).map(str::to_string).collect()
});

const EVERY: &[&str] = &["hourly", "daily", "weekdays", "weekly"];
const EVENTS: &[&str] = &["file", "turn", "commit", "pr_checks_failed", "finished"];
const ACTIONS: &[&str] = &["agent", "quick", "shell"];
const MASK: &str = kelpie_proto::redact::MASK;

/// Name parts that mark a secret only as a whole part: `SMTP_PASS`, `--pass`, `OPENAI_KEY`, `apiKey`,
/// `GH_AUTH`, `GITHUB_PAT` — not as the end of a longer word (`BYPASS`, `MONKEY`, `AUTHOR`, `COMPAT`).
const SECRET_PARTS: &[&str] = &["PASS", "PWD", "KEY", "AUTH", "OAUTH", "COOKIE", "PAT"];
/// Words that mark a secret as a whole part or at the end of one: `DBPASSWORD`, `GHTOKEN`, `clientSecret`,
/// and compounds a secret word is commonly glued into: `SSHPASS`, `DBPASS`, `GPG_PASSPHRASE`,
/// `PRIVATEKEY`, `AWS_SECRETKEY`, `ACCESSKEY`, `MYAPIKEY`, `AUTHORIZATION`, `SLACK_WEBHOOK_URL`.
const SECRET_SUFFIXES: &[&str] = &[
    "PASSWORD", "PASSWD", "SECRET", "TOKEN", "CREDENTIAL", "CREDENTIALS", "SSHPASS", "DBPASS", "PASSPHRASE", "PRIVATEKEY", "SECRETKEY", "ACCESSKEY",
    "APIKEY", "AUTHORIZATION", "WEBHOOK",
];
/// Parts that look like a secret word but are not: never a reason to leave a value out.
const NOT_SECRET_PARTS: &[&str] = &["BYPASS", "COMPASS", "PASSTHROUGH", "PASSIVE", "PASSPORT", "MONKEY", "TURNKEY", "HOTKEY", "AUTHOR", "COMPAT", "PATH", "PATCH"];

/// A name's parts, upper case: split at anything but letters and digits, at lower → upper case
/// (`apiKey` → `API`, `KEY`) and at the end of an acronym (`HTTPAuth` → `HTTP`, `AUTH`; `OPENAIKey` →
/// `OPENAI`, `KEY`), trailing digits dropped (`DB_PASS2` → `DB`, `PASS`).
fn name_parts(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|j| chars[j]);
        let next = chars.get(i + 1).copied();
        let boundary = !c.is_ascii_alphanumeric()
            || (c.is_ascii_uppercase() && prev.is_some_and(|p| p.is_ascii_lowercase()))
            || (c.is_ascii_uppercase() && prev.is_some_and(|p| p.is_ascii_uppercase()) && next.is_some_and(|n| n.is_ascii_lowercase()));
        if boundary && !cur.is_empty() {
            parts.push(std::mem::take(&mut cur));
        }
        if c.is_ascii_alphanumeric() {
            cur.push(c.to_ascii_uppercase());
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts.into_iter().map(|p| p.trim_end_matches(|c: char| c.is_ascii_digit()).to_string()).filter(|p| !p.is_empty()).collect()
}

/// Whether a variable, key, header or flag name says its value is a secret (case-insensitive). `SESSION`
/// alone is not one (`--session-id`, `XDG_SESSION_TYPE`); a session token or cookie is, by its other
/// word. A name ending in `env_key` holds the name of a variable (Codex `env_key=OPENAI_API_KEY`).
pub fn secret_name(name: &str) -> bool {
    let parts = name_parts(name);
    if parts.len() >= 2 && parts[parts.len() - 2] == "ENV" && parts[parts.len() - 1] == "KEY" {
        return false;
    }
    parts.iter().filter(|p| !NOT_SECRET_PARTS.contains(&p.as_str())).any(|p| SECRET_PARTS.contains(&p.as_str()) || SECRET_SUFFIXES.iter().any(|w| p.ends_with(w)))
}

/// Credentials in free text beyond the redaction helper's formats: a secret-named flag's value
/// (`--api-key X`, `--password=x`), `mysql -pXXX`, `sshpass -p X`, `curl -u user:pass`, secret headers
/// (`x-api-key: …`, `Authorization: Basic …`), any `Bearer` token of 8+ characters, and `NAME=value`
/// with a secret-sounding upper-case name whatever the value looks like.
static FLAG_VALUE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(^|[\s"'(])(--?[A-Za-z][\w.-]*)(=|[ \t]+)("[^"]*"|'[^']*'|[^\s"']+)"#).expect("flag pattern"));
static MYSQL_PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\b(?:mysql(?:dump|admin|sh)?|mariadb)\b[^\n|;&]*?\s-p)([^\s-]\S*)").expect("mysql pattern"));
static SSHPASS_PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\bsshpass\b[^\n|;&]*?\s-p[ \t]*)([^\s-]\S*)").expect("sshpass pattern"));
static USER_PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"((?:^|\s)(?:-u|--user)[ \t]+[^\s:]+:)(\S+)").expect("user pattern"));
static HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)(\b(?:x-[\w-]*(?:key|token|secret|auth|password)[\w-]*|api-key|private-token|(?:proxy-)?authorization)[ \t]*:[ \t]*(?:(?:basic|bearer|token|digest)[ \t]+)?)([^\s"']+)"#).expect("header pattern"));
static BEARER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\bbearer[ \t]+)([A-Za-z0-9._~+/=-]{8,})").expect("bearer pattern"));
static ENV_ASSIGN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"\b([A-Z][A-Z0-9_]*)=("[^"]*"|'[^']*'|\S+)"#).expect("env pattern"));

fn masked_value(v: &str) -> String {
    match v.chars().next() {
        Some(q @ ('"' | '\'')) => format!("{q}{MASK}{q}"),
        _ => MASK.to_string(),
    }
}

/// `text` with recognisable credentials masked.
pub fn mask_text(text: &str) -> String {
    let out = FLAG_VALUE.replace_all(text, |c: &regex::Captures| {
        if secret_name(&c[2]) { format!("{}{}{}{}", &c[1], &c[2], &c[3], masked_value(&c[4])) } else { c[0].to_string() }
    });
    let out = MYSQL_PASSWORD.replace_all(&out, format!("${{1}}{MASK}"));
    let out = SSHPASS_PASSWORD.replace_all(&out, format!("${{1}}{MASK}"));
    let out = USER_PASSWORD.replace_all(&out, format!("${{1}}{MASK}"));
    let out = HEADER.replace_all(&out, format!("${{1}}{MASK}"));
    let out = BEARER.replace_all(&out, format!("${{1}}{MASK}"));
    let out = ENV_ASSIGN.replace_all(&out, |c: &regex::Captures| {
        if secret_name(&c[1]) { format!("{}={}", &c[1], masked_value(&c[2])) } else { c[0].to_string() }
    });
    kelpie_proto::redact(&out)
}

/// The program's name without folder or extension, lower case (`C:\bin\MySQL.exe` → `mysql`).
fn tool_name(program: &str) -> String {
    let base = program.rsplit(['/', '\\']).next().unwrap_or(program).to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
}

/// An argument list (of `program`, when known) with the value after a secret-named flag masked
/// (`--api-key X`, `--token=X`), `name=value` arguments with a secret name masked (`-c api_key=X`),
/// `-u user:pass` (curl and others), `-pPASS` (mysql, mariadb, sshpass) and `-p PASS` (sshpass), and
/// every argument through the text filter.
pub fn mask_args(args: &[String], program: &str) -> Vec<String> {
    let tool = tool_name(program);
    let mysql = tool.starts_with("mysql") || tool.starts_with("mariadb");
    let sshpass = tool == "sshpass";
    let mut out = Vec::with_capacity(args.len());
    let mut hide_next = false;
    let mut user_next = false;
    for a in args {
        if std::mem::take(&mut hide_next) && !a.starts_with('-') {
            out.push(MASK.to_string());
            continue;
        }
        if std::mem::take(&mut user_next) {
            if let Some((user, _)) = a.split_once(':').filter(|(u, _)| !a.starts_with('-') && u.len() > 1) {
                out.push(format!("{user}:{MASK}"));
                continue;
            }
        }
        if (mysql || sshpass) && a.len() > 2 && a.starts_with("-p") && !a.starts_with("--") {
            out.push(format!("-p{MASK}"));
            continue;
        }
        if sshpass && a == "-p" {
            hide_next = true;
            out.push(a.clone());
            continue;
        }
        if a == "-u" || a == "--user" {
            user_next = true;
            out.push(a.clone());
            continue;
        }
        let (name, value) = match a.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (a.as_str(), None),
        };
        if secret_name(name) && (a.starts_with('-') || value.is_some()) {
            match value {
                Some(_) => out.push(format!("{name}={MASK}")),
                None => {
                    hide_next = true;
                    out.push(a.clone());
                }
            }
            continue;
        }
        out.push(mask_text(a));
    }
    out
}

/// Whether `value` (named `name`) holds a credential: the text filter changes it, or the redaction
/// helper takes it for one under its name (`GITHUB_TOKEN=…`, `"apiKey": "…"`).
pub fn is_secret(name: &str, value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let changed = |s: String| kelpie_proto::redact(&s) != s;
    mask_text(value) != value || changed(format!("{name}={value}")) || changed(format!("\"{name}\": \"{value}\""))
}

/// Masks every string that holds a credential, keeping the rest of the text where it can
/// (`export GITHUB_TOKEN=[redacted]`).
fn scrub(v: &mut Value, name: &str) {
    match v {
        Value::String(s) if is_secret(name, s) => {
            let masked = mask_text(s);
            *s = if masked != *s { masked } else { MASK.to_string() };
        }
        Value::Array(a) => a.iter_mut().for_each(|x| scrub(x, name)),
        Value::Object(o) => o.iter_mut().for_each(|(k, x)| scrub(x, k)),
        _ => {}
    }
}

// ---------- field helpers ----------

fn text(o: &Map<String, Value>, k: &str) -> Option<String> {
    o.get(k).and_then(Value::as_str).filter(|s| s.len() <= MAX_TEXT && !s.contains('\0')).map(str::to_string)
}

fn filled(o: &Map<String, Value>, k: &str) -> Option<String> {
    text(o, k).filter(|s| !s.trim().is_empty())
}

/// The field on an agent or automation saying why one of its argument lists is not in the file (written by
/// the export, kept by the import): [`ARGS_TOO_MANY`] or [`ARGS_INVALID`].
const ARGS_LEFT_OUT: &str = "argsLeftOut";
/// More than [`MAX_LIST`] arguments.
const ARGS_TOO_MANY: &str = "tooMany";
/// An argument that is not text, is longer than [`MAX_TEXT`] or has a NUL.
const ARGS_INVALID: &str = "invalid";

/// An argument list whole, or why it cannot be carried: a list is never shortened, since without one of
/// its elements the rest can mean something else (a flag taking the next argument as its value). A missing
/// list is empty, unless `marked` (the item's [`ARGS_LEFT_OUT`]) says the export left it out.
fn arg_list(v: Option<&Value>, marked: Option<&str>) -> Result<Vec<String>, &'static str> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return match marked {
            Some(ARGS_TOO_MANY) => Err(ARGS_TOO_MANY),
            Some(ARGS_INVALID) => Err(ARGS_INVALID),
            _ => Ok(Vec::new()),
        };
    };
    let a = v.as_array().ok_or(ARGS_INVALID)?;
    if a.len() > MAX_LIST {
        return Err(ARGS_TOO_MANY);
    }
    a.iter().map(|x| x.as_str().filter(|s| s.len() <= MAX_TEXT && !s.contains('\0')).map(str::to_string).ok_or(ARGS_INVALID)).collect()
}

/// The [`ARGS_LEFT_OUT`] mark an item carries already (a file written by an export).
fn marked(o: &Map<String, Value>) -> Option<&str> {
    o.get(ARGS_LEFT_OUT).and_then(Value::as_str)
}

fn flag(o: &Map<String, Value>, k: &str, default: bool) -> bool {
    o.get(k).and_then(Value::as_bool).unwrap_or(default)
}

fn one_of(o: &Map<String, Value>, k: &str, allowed: &[&str], default: &str) -> String {
    o.get(k).and_then(Value::as_str).filter(|s| allowed.contains(s)).unwrap_or(default).to_string()
}

fn id_of(o: &Map<String, Value>) -> String {
    text(o, "id").filter(|s| s.len() <= 128 && !s.chars().any(char::is_control)).unwrap_or_default()
}

fn valid_env_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 256 && n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !n.starts_with(|c: char| c.is_ascii_digit())
}

/// An environment as a file holds it, sorted by name: every name; a value only when values are wanted,
/// the name does not sound secret and the value does not look like one. Reads a `{NAME: value}` map (the app's) or a
/// `[{name, value?}]` list (a file's).
fn env_entries(v: Option<&Value>, with_values: bool) -> Value {
    let mut pairs: Vec<(String, Option<String>)> = match v {
        Some(Value::Object(m)) => m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string))).collect(),
        Some(Value::Array(a)) => a.iter().filter_map(|e| Some((e.get("name")?.as_str()?.to_string(), e.get("value").and_then(Value::as_str).map(str::to_string)))).collect(),
        _ => Vec::new(),
    };
    pairs.retain(|(n, _)| valid_env_name(n));
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs.dedup_by(|a, b| a.0 == b.0);
    pairs.truncate(MAX_LIST);
    Value::Array(
        pairs
            .into_iter()
            .map(|(name, value)| {
                let value = value.filter(|v| with_values && v.len() <= MAX_TEXT && !v.contains('\0') && !secret_name(&name) && !is_secret(&name, v));
                let mut e = json!({ "name": name });
                if let Some(v) = value {
                    e["value"] = json!(v);
                }
                e
            })
            .collect(),
    )
}

// ---------- sections ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    Local,
    /// `\\server\share`, `//server/share`, `\\?\UNC\server\…`, `\??\UNC\server\…`: opening one on Windows
    /// sends the user's credentials (NTLM) to that server.
    Network,
    /// Windows device and raw paths (`\\.\pipe\…`, `\\?\C:\…`, `\??\C:\…`): even reading their metadata
    /// opens them.
    Device,
}

/// What kind of path `path` is on Windows, whatever its separators (`/` and `\` are the same to Windows,
/// so `\/server`, `/\?\…` and `//./pipe` count too).
pub fn path_kind(path: &str) -> PathKind {
    let p = path.trim().replace('/', "\\");
    let upper = p.to_ascii_uppercase();
    for prefix in ["\\\\?\\", "\\\\.\\", "\\??\\"] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            return if rest.starts_with("UNC\\") { PathKind::Network } else { PathKind::Device };
        }
    }
    if p.starts_with("\\\\") { PathKind::Network } else { PathKind::Local }
}

/// A path a file must never bring in for something Kelpie or the daemon opens: a network or device path.
pub fn foreign_path(path: &str) -> bool {
    path_kind(path) != PathKind::Local
}

/// A program argument that is, or ends in (`--file=…`, `-File:…`), a network or device path.
pub fn foreign_arg(arg: &str) -> bool {
    foreign_path(arg) || arg.split_once('=').is_some_and(|(_, v)| foreign_path(v)) || (arg.starts_with('-') && arg.split_once(':').is_some_and(|(_, v)| foreign_path(v)))
}

/// Only the settings classified as portable; the window checks each value's shape and range.
fn settings(v: &Value) -> CmdResult<Value> {
    let o = v.as_object().ok_or("the settings section is not an object")?;
    let mut out = Map::new();
    for (k, val) in o {
        if SETTINGS_EXPORTED.contains(k) && out.len() < MAX_SETTINGS {
            let val = match (k.as_str(), val.as_array()) {
                // The terminal's program and arguments: same masking as an agent's arguments; never a
                // network or device path, never shortened (left out whole instead, see `args_left_out`).
                ("shellArgs", _) => {
                    let Ok(args) = arg_list(Some(val), None) else { continue };
                    if args.iter().any(|x| foreign_arg(x)) {
                        continue;
                    }
                    json!(mask_args(&args, args.first().map_or("", String::as_str)))
                }
                _ => val.clone(),
            };
            out.insert(k.clone(), val);
        }
    }
    Ok(Value::Object(out))
}

fn quick_command(o: &Map<String, Value>) -> Option<Value> {
    let label = filled(o, "label")?;
    let body = text(o, "text").filter(|s| !s.is_empty())?;
    Some(json!({
        "id": id_of(o),
        "label": label,
        "text": body,
        "action": one_of(o, "action", &["terminal", "agent"], "terminal"),
        "enter": flag(o, "enter", true),
        "projectId": text(o, "projectId").filter(|s| s.len() <= 128),
    }))
}

/// A saved layout's shape: leaves (an agent kind or empty, maybe on a remote target) and splits.
fn template(v: &Value, depth: u32) -> Option<Value> {
    let o = v.as_object()?;
    match o.get("type")?.as_str()? {
        "leaf" => {
            let kind = match o.get("kind") {
                None | Some(Value::Null) => Value::Null,
                Some(Value::String(s)) if s.len() <= 64 => json!(s),
                _ => return None,
            };
            let mut leaf = json!({ "type": "leaf", "kind": kind });
            if let Some(t) = o.get("target").and_then(Value::as_object) {
                let mut target = json!({ "dest": filled(t, "dest")?, "path": text(t, "path")?, "tmux": flag(t, "tmux", true) });
                if let Some(p) = t.get("port").and_then(Value::as_u64).filter(|p| (1..65536).contains(p)) {
                    target["port"] = json!(p);
                }
                leaf["target"] = target;
            }
            Some(leaf)
        }
        "split" if depth < 8 => {
            let dir = o.get("dir")?.as_str().filter(|d| matches!(*d, "row" | "col"))?;
            let children = o.get("children")?.as_array()?.iter().map(|c| template(c, depth + 1)).collect::<Option<Vec<_>>>()?;
            if children.is_empty() || children.len() > 12 {
                return None;
            }
            let sizes: Vec<f64> = o.get("sizes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
            let sizes = if sizes.len() == children.len() && sizes.iter().all(|s| s.is_finite() && *s > 0.0) { sizes } else { vec![1.0 / children.len() as f64; children.len()] };
            Some(json!({ "type": "split", "dir": dir, "sizes": sizes, "children": children }))
        }
        _ => None,
    }
}

fn layout(o: &Map<String, Value>) -> Option<Value> {
    Some(json!({ "id": id_of(o), "name": filled(o, "name")?, "template": template(o.get("template")?, 0)? }))
}

/// An automation without anything the daemon owns: no approval, no run history, and switched off.
fn automation(o: &Map<String, Value>, with_values: bool) -> Option<Value> {
    let name = filled(o, "name")?;
    let cwd = filled(o, "cwd").filter(|c| !foreign_path(c))?;
    let action = one_of(o, "action", ACTIONS, "agent");
    // A list that cannot be carried whole is left out with the reason (the window keeps this computer's
    // list, or leaves the automation out).
    let mut left_out = None;
    let shell = if action == "shell" {
        // The daemon reads the program and the scripts it is given when it saves the automation: a network
        // or device path anywhere in it leaves the whole automation out.
        let sh = o.get("shell")?.as_object()?;
        let program = filled(sh, "program").filter(|p| !foreign_path(p))?;
        match arg_list(sh.get("argv"), marked(o)) {
            Ok(argv) => {
                if argv.iter().any(|a| foreign_arg(a)) {
                    return None;
                }
                let argv = mask_args(&argv, &program);
                json!({ "program": program, "argv": argv })
            }
            Err(why) => {
                left_out = Some(why);
                json!({ "program": program })
            }
        }
    } else {
        Value::Null
    };
    let args = match arg_list(o.get("args"), marked(o)) {
        Ok(a) => Some(mask_args(&a, "")),
        Err(why) => {
            left_out = left_out.or(Some(why));
            None
        }
    };
    let trigger = o.get("trigger").and_then(Value::as_object).and_then(|t| {
        let event = t.get("event")?.as_str().filter(|e| EVENTS.contains(e))?;
        let cap = t.get("daily_cap").and_then(Value::as_u64).filter(|c| (1..=1000).contains(c)).unwrap_or(10);
        Some(json!({ "event": event, "glob": text(t, "glob").unwrap_or_default(), "daily_cap": cap }))
    });
    let time = text(o, "time").filter(|t| t.len() == 5 && t.as_bytes()[2] == b':' && t.chars().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit())).unwrap_or_else(|| "09:00".into());
    let mut out = json!({
        "id": id_of(o),
        "name": name,
        "enabled": false,
        "kind": if action == "shell" { String::new() } else { text(o, "kind").filter(|k| k.len() <= 64).unwrap_or_default() },
        "prompt": text(o, "prompt").unwrap_or_default(),
        "cwd": cwd,
        "project_id": text(o, "project_id").filter(|s| s.len() <= 128),
        "env": env_entries(o.get("env"), with_values),
        "every": one_of(o, "every", EVERY, "daily"),
        "time": time,
        "weekday": o.get("weekday").and_then(Value::as_u64).filter(|d| *d < 7).unwrap_or(1),
        "deliver": o.get("deliver").and_then(Value::as_str).filter(|d| *d == "review"),
        "trigger": trigger,
        "action": action,
        "shell": shell,
        "quick_label": text(o, "quick_label").filter(|s| s.len() <= 256),
        "dry_run": flag(o, "dry_run", false),
    });
    if let Some(args) = args {
        out["args"] = json!(args);
    }
    if let Some(why) = left_out {
        out[ARGS_LEFT_OUT] = json!(why);
    }
    Some(out)
}

/// A saved host without credentials (it has no field for them; anything else is dropped) or what this
/// computer recorded about it, checked like the Hosts page checks it.
fn ssh_host(v: &Value) -> Option<Value> {
    let mut h: Host = serde_json::from_value(v.clone()).ok()?;
    if foreign_path(&h.identity_file) {
        return None;
    }
    h.os.clear();
    h.last_connected = 0;
    h.env.retain(|k, val| !secret_name(k) && !is_secret(k, val));
    // Saved passwords are keyed by id, which must be letters and digits: anything else gets a new one.
    if !h.id.chars().all(|c| c.is_ascii_alphanumeric()) || h.id.len() > 64 {
        h.id.clear();
    }
    let checked = ssh_hosts::normalize(Vault { version: 1, hosts: vec![h], forwards: Vec::new() }).ok()?;
    let mut out = serde_json::to_value(checked.hosts.into_iter().next()?).ok()?;
    let o = out.as_object_mut()?;
    o.remove("os");
    o.remove("lastConnected");
    Some(out)
}

fn plugin(o: &Map<String, Value>) -> Option<Value> {
    let id = text(o, "id").filter(|i| crate::plugins::valid_plugin_id(i))?;
    let mut p = json!({ "id": id, "name": filled(o, "name").filter(|n| n.len() <= 256).unwrap_or_else(|| id.clone()) });
    if let Some(v) = filled(o, "version").filter(|v| v.len() <= 64) {
        p["version"] = json!(v);
    }
    Some(p)
}

fn agent(o: &Map<String, Value>, with_values: bool) -> Option<Value> {
    let kind = filled(o, "kind").filter(|k| k.len() <= 64)?;
    let mut a = json!({
        "kind": kind,
        "label": filled(o, "label").unwrap_or_else(|| kind.clone()),
        "env": env_entries(o.get("env"), with_values),
    });
    // A list that cannot be carried whole is left out with the reason: the window keeps this computer's.
    match arg_list(o.get("args"), marked(o)) {
        Ok(args) => a["args"] = json!(mask_args(&args, "")),
        Err(why) => a[ARGS_LEFT_OUT] = json!(why),
    }
    if let Some(p) = filled(o, "permission").filter(|p| p.len() <= 64) {
        a["permission"] = json!(p);
    }
    Some(a)
}

/// The usable items of a list section, and how many were left out.
fn items(v: &Value, name: &str, f: impl Fn(&Value) -> Option<Value>) -> CmdResult<(Value, usize)> {
    let list = v.as_array().ok_or_else(|| format!("the {name} section is not a list"))?;
    let kept: Vec<Value> = list.iter().take(MAX_ITEMS).filter_map(&f).collect();
    let skipped = list.len() - kept.len();
    Ok((Value::Array(kept), skipped))
}

fn object_item(f: impl Fn(&Map<String, Value>) -> Option<Value>) -> impl Fn(&Value) -> Option<Value> {
    move |v| v.as_object().and_then(&f)
}

pub struct Clean {
    pub sections: Map<String, Value>,
    /// Items left out, by section.
    pub skipped: Map<String, Value>,
    /// Sections this version does not know.
    pub ignored: Vec<String>,
}

/// Every known section through its allowlist, then every string through the secret filter.
fn clean_sections(input: &Map<String, Value>, with_values: bool) -> CmdResult<Clean> {
    let mut sections = Map::new();
    let mut skipped = Map::new();
    let mut ignored = Vec::new();
    for (name, v) in input {
        let (value, left) = match name.as_str() {
            "settings" => (settings(v)?, 0),
            "quickCommands" => items(v, name, object_item(quick_command))?,
            "layouts" => items(v, name, object_item(layout))?,
            "automations" => items(v, name, object_item(|o| automation(o, with_values)))?,
            "sshHosts" => items(v, name, ssh_host)?,
            "plugins" => items(v, name, object_item(plugin))?,
            "agents" => items(v, name, object_item(|o| agent(o, with_values)))?,
            other => {
                ignored.push(other.to_string());
                continue;
            }
        };
        if left > 0 {
            skipped.insert(name.clone(), json!(left));
        }
        sections.insert(name.clone(), value);
    }
    let mut all = Value::Object(sections);
    scrub(&mut all, "");
    let Value::Object(sections) = all else { unreachable!() };
    Ok(Clean { sections, skipped, ignored })
}

/// Items a file leaves out because they point at a network or device path, by name: hosts with such a key
/// file, automations with such a folder, program or argument, and the terminal's program and arguments.
pub fn foreign_items(sections: &Map<String, Value>) -> Vec<Value> {
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or_default().to_string();
    let mut out = Vec::new();
    for h in sections.get("sshHosts").and_then(Value::as_array).into_iter().flatten() {
        if foreign_path(&s(h, "identityFile")) {
            out.push(json!({ "section": "sshHosts", "name": s(h, "alias").trim().to_string().chars().take(64).collect::<String>() }));
        }
    }
    for a in sections.get("automations").and_then(Value::as_array).into_iter().flatten() {
        let shell = a.get("shell").filter(|_| s(a, "action") == "shell");
        let argv = shell.and_then(|sh| sh.get("argv")).and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str);
        if foreign_path(&s(a, "cwd")) || shell.is_some_and(|sh| foreign_path(&s(sh, "program"))) || argv.into_iter().any(foreign_arg) {
            out.push(json!({ "section": "automations", "name": s(a, "name").chars().take(64).collect::<String>() }));
        }
    }
    let shell_args = sections.get("settings").and_then(|v| v.get("shellArgs")).and_then(Value::as_array);
    if shell_args.into_iter().flatten().filter_map(Value::as_str).any(foreign_arg) {
        out.push(json!({ "section": "settings", "name": "shellArgs" }));
    }
    out
}

/// Argument lists left out whole (never shortened), with the reason ([`ARGS_TOO_MANY`], [`ARGS_INVALID`]):
/// the terminal's, checked on `raw_settings` (the settings section before cleaning), and each agent's
/// (by label) and automation's (by name) marked in the cleaned `sections`.
pub fn args_left_out(sections: &Map<String, Value>, raw_settings: Option<&Value>) -> Vec<Value> {
    let mut out = Vec::new();
    if let Some(Err(why)) = raw_settings.and_then(|s| s.get("shellArgs")).map(|v| arg_list(Some(v), None)) {
        out.push(json!({ "section": "settings", "name": "shellArgs", "reason": why }));
    }
    for (section, name) in [("agents", "label"), ("automations", "name")] {
        for item in sections.get(section).and_then(Value::as_array).into_iter().flatten() {
            if let Some(why) = item.get(ARGS_LEFT_OUT).and_then(Value::as_str) {
                out.push(json!({ "section": section, "name": item.get(name).and_then(Value::as_str).unwrap_or_default(), "reason": why }));
            }
        }
    }
    out
}

/// The export document: the window's sections plus the hosts and plugins read here, cleaned.
pub fn export_doc(raw: &Map<String, Value>, wanted: &[String], with_values: bool, hosts: Option<&Vault>, plugins: Option<Vec<Value>>, exported_at: &str) -> CmdResult<Value> {
    let mut input = Map::new();
    let mut plugins = plugins;
    for w in wanted {
        let value = match w.as_str() {
            "sshHosts" => serde_json::to_value(&hosts.ok_or("no hosts to export")?.hosts).map_err(err)?,
            "plugins" => Value::Array(plugins.take().unwrap_or_default()),
            s if SECTIONS.contains(&s) => match raw.get(s) {
                Some(v) => v.clone(),
                None => continue,
            },
            other => return Err(format!("unknown section {other}")),
        };
        input.insert(w.clone(), value);
    }
    let clean = clean_sections(&input, with_values)?;
    let stamp: String = exported_at.chars().filter(|c| !c.is_control()).take(64).collect();
    Ok(json!({ "kind": KIND, "version": VERSION, "exportedAt": stamp, "appVersion": env!("CARGO_PKG_VERSION"), "sections": clean.sections }))
}

fn too_large() -> String {
    format!("the settings file is larger than {} MB", MAX_FILE / (1024 * 1024))
}

/// Checks a file's bytes and returns its cleaned document, what was left out and the sections ignored.
pub fn read_doc(bytes: &[u8]) -> CmdResult<Value> {
    if bytes.len() as u64 > MAX_FILE {
        return Err(too_large());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|e| format!("not a Kelpie settings file: {e}"))?;
    let o = v.as_object().ok_or("not a Kelpie settings file")?;
    if o.get("kind").and_then(Value::as_str) != Some(KIND) {
        return Err("not a Kelpie settings file".into());
    }
    match o.get("version").and_then(Value::as_u64) {
        Some(VERSION) => {}
        Some(n) if n > VERSION => return Err(format!("this settings file is from a newer Kelpie (format {n})")),
        _ => return Err("unknown settings file format".into()),
    }
    let sections = o.get("sections").and_then(Value::as_object).ok_or("the settings file has no sections")?;
    let clean = clean_sections(sections, true)?;
    let mut ignored = clean.ignored;
    ignored.extend(o.keys().filter(|k| !matches!(k.as_str(), "kind" | "version" | "exportedAt" | "appVersion" | "sections")).cloned());
    let stamp = |k: &str| text(o, k).map(|s| s.chars().filter(|c| !c.is_control()).take(64).collect::<String>());
    let args_out = args_left_out(&clean.sections, sections.get("settings"));
    Ok(json!({
        "doc": { "kind": KIND, "version": VERSION, "exportedAt": stamp("exportedAt"), "appVersion": stamp("appVersion"), "sections": clean.sections },
        "skipped": clean.skipped,
        "ignored": ignored,
        "foreign": foreign_items(sections),
        "argsLeftOut": args_out,
    }))
}

/// A device or raw path (see [`PathKind::Device`]): never opened for an import or an export. (A network
/// share chosen in the dialog is fine: the user picked it.)
fn device_path(path: &Path) -> bool {
    path_kind(&path.to_string_lossy()) == PathKind::Device
}

/// At most [`MAX_FILE`] bytes of a regular file; a larger one is refused unread.
fn read_capped(path: &Path) -> CmdResult<Vec<u8>> {
    // Checked before opening: opening a FIFO blocks, and a pipe path connects to a server.
    if device_path(path) || !std::fs::metadata(path).map_err(err)?.is_file() {
        return Err("not a file".into());
    }
    let f = std::fs::File::open(path).map_err(err)?;
    let meta = f.metadata().map_err(err)?;
    if !meta.is_file() {
        return Err("not a file".into());
    }
    if meta.len() > MAX_FILE {
        return Err(too_large());
    }
    let mut buf = Vec::new();
    f.take(MAX_FILE + 1).read_to_end(&mut buf).map_err(err)?;
    if buf.len() as u64 > MAX_FILE {
        return Err(too_large());
    }
    Ok(buf)
}

/// Beside the target, then renamed over it: a crash never leaves half a file. The temporary file has
/// a name of its own (two writes to one target never share it) and is created new, so a file or link
/// already at that name is never written through; it is removed when anything fails.
pub(crate) fn write_file_atomic(path: &Path, bytes: &[u8]) -> CmdResult<()> {
    write_atomic_with(path, bytes, rename_retrying)
}

/// `std::fs::rename`, tried again a few times when Windows refuses it for a moment (another rename
/// to the same target, a virus scanner holding the file).
fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 5 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * attempt));
            }
            done => return done,
        }
    }
}

/// [`write_file_atomic`] with the final rename given (tests make it fail after the temporary file exists).
pub(crate) fn write_atomic_with(path: &Path, bytes: &[u8], rename: impl Fn(&Path, &Path) -> std::io::Result<()>) -> CmdResult<()> {
    use std::io::Write;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if device_path(path) {
        return Err("not a file".into());
    }
    if path.is_dir() {
        return Err("choose a file, not a folder".into());
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or("choose a file in a folder")?;
    let name = path.file_name().ok_or("choose a file in a folder")?.to_string_lossy().into_owned();
    let (tmp, mut file) = (0..16)
        .find_map(|_| {
            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let tmp = parent.join(format!(".{name}.{}-{seq}-{nanos:08x}.tmp", std::process::id()));
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp) {
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => None,
                opened => Some(opened.map(|f| (tmp, f))),
            }
        })
        .ok_or("could not create a temporary file beside the target")?
        .map_err(err)?;
    let written = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    written.and_then(|_| rename(&tmp, path)).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })
}

fn write_json(path: &Path, v: &Value) -> CmdResult<()> {
    std::fs::write(path, serde_json::to_vec_pretty(v).map_err(err)?).map_err(err)
}

/// Copies the configuration an import is about to change into `<root>/config-<ms>/`: the window's state,
/// the daemon's automations and the hosts file (byte for byte). Built aside and renamed in, so a backup
/// that exists is whole. Keeps the newest few.
fn backup_to(root: &Path, hosts_file: &Path, app_state: &Value, automations: &Value) -> CmdResult<PathBuf> {
    let mut ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    while root.join(format!("config-{ms}")).exists() {
        ms += 1;
    }
    let dest = root.join(format!("config-{ms}"));
    let tmp = root.join(format!(".config-{ms}.tmp"));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(err)?;
    let made = (|| {
        write_json(&tmp.join("app-state.json"), app_state)?;
        write_json(&tmp.join("automations.json"), automations)?;
        match std::fs::read(hosts_file) {
            Ok(bytes) => std::fs::write(tmp.join("ssh-hosts.json"), bytes).map_err(err)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("the saved SSH hosts could not be read: {e}")),
        }
        std::fs::rename(&tmp, &dest).map_err(err)
    })();
    if let Err(e) = made {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    prune_backups(root);
    Ok(dest)
}

fn prune_backups(root: &Path) {
    let mut found: Vec<(u128, PathBuf)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().strip_prefix("config-")?.parse::<u128>().ok()?, e.path())))
        .collect();
    found.sort_by_key(|(ms, _)| std::cmp::Reverse(*ms));
    for (_, path) in found.into_iter().skip(KEPT_BACKUPS) {
        let _ = std::fs::remove_dir_all(path);
    }
}

// ---------- commands ----------

/// Writes the export to `path` (from the save dialog). `sections` holds the window's own sections;
/// `wanted` names every section to include. Returns the item count of each section written, and by name
/// the items left out for a network or device path and the argument lists left out whole.
#[tauri::command]
pub async fn config_export(path: String, sections: Value, wanted: Vec<String>, include_env_values: bool, exported_at: String) -> CmdResult<Value> {
    blocking(move || {
        let raw = sections.as_object().cloned().unwrap_or_default();
        let hosts = if wanted.iter().any(|w| w == "sshHosts") { Some(ssh_hosts::load_strict()?) } else { None };
        let plugins = wanted.iter().any(|w| w == "plugins").then(crate::plugins::export_list);
        let doc = export_doc(&raw, &wanted, include_env_values, hosts.as_ref(), plugins, &exported_at)?;
        let bytes = serde_json::to_vec_pretty(&doc).map_err(err)?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(too_large());
        }
        write_file_atomic(Path::new(&path), &bytes)?;
        let counts: Map<String, Value> = doc["sections"].as_object().map(|s| s.iter().map(|(k, v)| (k.clone(), json!(v.as_array().map_or(1, Vec::len)))).collect()).unwrap_or_default();
        let mut considered = Map::new();
        for w in &wanted {
            match w.as_str() {
                "sshHosts" => {
                    considered.insert(w.clone(), serde_json::to_value(hosts.as_ref().map(|h| &h.hosts)).map_err(err)?);
                }
                "automations" | "settings" => {
                    if let Some(v) = raw.get(w) {
                        considered.insert(w.clone(), v.clone());
                    }
                }
                _ => {}
            }
        }
        let empty = Map::new();
        let written = doc["sections"].as_object().unwrap_or(&empty);
        let args_out = args_left_out(written, considered.get("settings"));
        Ok(json!({ "path": path, "counts": counts, "foreign": foreign_items(&considered), "argsLeftOut": args_out }))
    })
    .await
}

/// Reads and checks a file for import (from the open dialog); nothing is changed.
#[tauri::command]
pub async fn config_import_read(path: String) -> CmdResult<Value> {
    blocking(move || read_doc(&read_capped(Path::new(&path))?)).await
}

/// Keeps a copy of the current configuration before an import changes it; returns the folder.
#[tauri::command]
pub async fn config_backup(app_state: Value, automations: Value) -> CmdResult<String> {
    blocking(move || {
        let root = kelpie_proto::data_dir().join("backups");
        backup_to(&root, &ssh_hosts::vault_path(), &app_state, &automations).map(|p| p.to_string_lossy().into_owned())
    })
    .await
}

/// The saved hosts, refusing a file that cannot be read (an import must not save over it).
#[tauri::command]
pub async fn config_hosts_current() -> CmdResult<Vault> {
    blocking(ssh_hosts::load_strict).await
}

/// Saves the hosts an import produced, only while the saved ones are still `expected`.
#[tauri::command]
pub async fn config_hosts_write(expected: Vault, vault: Vault) -> CmdResult<Vault> {
    blocking(move || ssh_hosts::save_checked(&expected, vault)).await
}

#[cfg(test)]
mod tests;
