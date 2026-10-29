//! Settings export and import (Settings → Session Daemon → Export and import settings). The window
//! gathers what it owns (settings, quick commands, layouts, agent profiles) and what the daemon keeps
//! (automations); this side adds the saved SSH hosts and the plugin list, and owns the file: its format,
//! the size cap, the field allowlists and the secret filter. Both directions go through the same
//! cleaning, so a file never carries credentials or approvals, and an import never brings in a field
//! Kelpie does not know. Nothing here writes to a path taken from a file.

use std::io::Read;
use std::path::{Path, PathBuf};

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

/// Settings keys never written to or taken from a file: sections of their own, sign-ins and local
/// files of this computer, and answers given on this computer.
const SETTINGS_EXCLUDED: &[&str] = &["profiles", "quickCommands", "layouts", "accounts", "activeAccounts", "sshConfig", "rootYoloOk", "skippedUpdate"];

const EVERY: &[&str] = &["hourly", "daily", "weekdays", "weekly"];
const EVENTS: &[&str] = &["file", "turn", "commit", "pr_checks_failed", "finished"];
const ACTIONS: &[&str] = &["agent", "quick", "shell"];

/// Whether the redaction helper takes `value` (named `name`) for a credential: a known token format,
/// or a value under a secret-sounding name (`GITHUB_TOKEN`, `"apiKey"`…).
pub fn is_secret(name: &str, value: &str) -> bool {
    if value.trim().is_empty() {
        return false;
    }
    let changed = |s: String| kelpie_proto::redact(&s) != s;
    changed(value.to_string()) || changed(format!("{name}={value}")) || changed(format!("\"{name}\": \"{value}\""))
}

/// Masks every string that holds a credential, keeping the rest of the text where the redaction
/// helper can (`export GITHUB_TOKEN=[redacted]`).
fn scrub(v: &mut Value, name: &str) {
    match v {
        Value::String(s) if is_secret(name, s) => {
            let masked = kelpie_proto::redact(s);
            *s = if masked != *s { masked } else { kelpie_proto::redact::MASK.to_string() };
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

fn texts(o: &Map<String, Value>, k: &str) -> Vec<String> {
    o.get(k)
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).filter(|s| s.len() <= MAX_TEXT && !s.contains('\0')).take(MAX_LIST).map(str::to_string).collect())
        .unwrap_or_default()
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

/// An environment as a file holds it, sorted by name: every name; a value only when values are wanted
/// and the redaction helper does not take it for a secret. Reads a `{NAME: value}` map (the app's) or a
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
                let value = value.filter(|v| with_values && v.len() <= MAX_TEXT && !v.contains('\0') && !is_secret(&name, v));
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

fn settings(v: &Value) -> CmdResult<Value> {
    let o = v.as_object().ok_or("the settings section is not an object")?;
    let mut out = Map::new();
    for (k, val) in o {
        if !SETTINGS_EXCLUDED.contains(&k.as_str()) && out.len() < MAX_SETTINGS {
            out.insert(k.clone(), val.clone());
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
    let cwd = filled(o, "cwd")?;
    let action = one_of(o, "action", ACTIONS, "agent");
    let shell = if action == "shell" {
        let sh = o.get("shell")?.as_object()?;
        json!({ "program": filled(sh, "program")?, "argv": texts(sh, "argv") })
    } else {
        Value::Null
    };
    let trigger = o.get("trigger").and_then(Value::as_object).and_then(|t| {
        let event = t.get("event")?.as_str().filter(|e| EVENTS.contains(e))?;
        let cap = t.get("daily_cap").and_then(Value::as_u64).filter(|c| (1..=1000).contains(c)).unwrap_or(10);
        Some(json!({ "event": event, "glob": text(t, "glob").unwrap_or_default(), "daily_cap": cap }))
    });
    let time = text(o, "time").filter(|t| t.len() == 5 && t.as_bytes()[2] == b':' && t.chars().enumerate().all(|(i, c)| i == 2 || c.is_ascii_digit())).unwrap_or_else(|| "09:00".into());
    Some(json!({
        "id": id_of(o),
        "name": name,
        "enabled": false,
        "kind": if action == "shell" { String::new() } else { text(o, "kind").filter(|k| k.len() <= 64).unwrap_or_default() },
        "prompt": text(o, "prompt").unwrap_or_default(),
        "cwd": cwd,
        "project_id": text(o, "project_id").filter(|s| s.len() <= 128),
        "args": texts(o, "args"),
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
    }))
}

/// A saved host without credentials (it has no field for them; anything else is dropped) or what this
/// computer recorded about it, checked like the Hosts page checks it.
fn ssh_host(v: &Value) -> Option<Value> {
    let mut h: Host = serde_json::from_value(v.clone()).ok()?;
    h.os.clear();
    h.last_connected = 0;
    h.env.retain(|k, val| !is_secret(k, val));
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
        "args": texts(o, "args"),
        "env": env_entries(o.get("env"), with_values),
    });
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
    Ok(json!({
        "doc": { "kind": KIND, "version": VERSION, "exportedAt": stamp("exportedAt"), "appVersion": stamp("appVersion"), "sections": clean.sections },
        "skipped": clean.skipped,
        "ignored": ignored,
    }))
}

/// At most [`MAX_FILE`] bytes of a regular file; a larger one is refused unread.
fn read_capped(path: &Path) -> CmdResult<Vec<u8>> {
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

/// Beside the target, then renamed over it: a crash never leaves half a file.
fn write_file_atomic(path: &Path, bytes: &[u8]) -> CmdResult<()> {
    if path.is_dir() {
        return Err("choose a file, not a folder".into());
    }
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).ok_or("choose a file in a folder")?;
    let name = path.file_name().ok_or("choose a file in a folder")?.to_string_lossy().into_owned();
    let tmp = parent.join(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes).map_err(err)?;
    std::fs::rename(&tmp, path).map_err(|e| {
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
/// `wanted` names every section to include. Returns the item count of each section written.
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
        Ok(json!({ "path": path, "counts": counts }))
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
