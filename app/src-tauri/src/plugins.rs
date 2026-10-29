//! Plugins: folders under `<data>/plugins/<id>/` with an `kelpie-plugin.json` manifest. A plugin is
//! declarative — agents, palette commands, quick commands, terminal themes, layouts and status
//! items — and nothing it declares that runs a program (agents, commands, status items) is
//! used before the user approves that exact manifest (its SHA-256): a changed manifest needs
//! approving again. Approvals live here, in `plugins/approvals.json`, not in the UI state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::fs_ops::{err, CmdResult};

const MANIFEST: &str = "kelpie-plugin.json";
/// Manifest name from before the rename to Kelpie; plugins that still use it keep working.
const LEGACY_MANIFEST: &str = "erka-plugin.json";

/// A plugin folder's manifest file: the current name, else the legacy one when only that exists.
fn manifest_path(dir: &Path) -> PathBuf {
    let path = dir.join(MANIFEST);
    let legacy = dir.join(LEGACY_MANIFEST);
    if !path.exists() && legacy.exists() { legacy } else { path }
}

fn plugins_dir() -> PathBuf {
    kelpie_proto::data_dir().join("plugins")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') && !id.starts_with('-')
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn approvals() -> BTreeMap<String, String> {
    std::fs::read(plugins_dir().join("approvals.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_approvals(map: &BTreeMap<String, String>) -> CmdResult<()> {
    std::fs::create_dir_all(plugins_dir()).map_err(err)?;
    std::fs::write(plugins_dir().join("approvals.json"), serde_json::to_vec_pretty(map).map_err(err)?).map_err(err)
}

/// What a manifest may contain; anything else is reported, not silently used.
const SECTIONS: &[&str] = &["agents", "commands", "quickCommands", "terminalThemes", "layouts", "statusItems"];

/// Checks a manifest's shape (the UI relies on it).
pub fn validate(m: &Value, folder: &str) -> Result<(), String> {
    let id = m["id"].as_str().ok_or("missing \"id\"")?;
    if !valid_id(id) {
        return Err(format!("id {id:?}: lowercase letters, digits and - only"));
    }
    if id != folder {
        return Err(format!("id {id:?} does not match its folder {folder:?}"));
    }
    if m["name"].as_str().is_none_or(str::is_empty) {
        return Err("missing \"name\"".into());
    }
    let c = m.get("contributes").cloned().unwrap_or(json!({}));
    let obj = c.as_object().ok_or("\"contributes\" must be an object")?;
    for (k, v) in obj {
        if !SECTIONS.contains(&k.as_str()) {
            return Err(format!("unknown contribution {k:?} (known: {})", SECTIONS.join(", ")));
        }
        let list = v.as_array().ok_or(format!("\"{k}\" must be a list"))?;
        for (i, item) in list.iter().enumerate() {
            let need = |field: &str| item[field].as_str().filter(|s| !s.trim().is_empty()).map(|_| ()).ok_or(format!("{k}[{i}] needs \"{field}\""));
            match k.as_str() {
                "agents" => {
                    need("id")?;
                    need("label")?;
                    need("bin")?;
                }
                "commands" => {
                    need("id")?;
                    need("label")?;
                    need("run")?;
                }
                "quickCommands" => {
                    need("label")?;
                    need("text")?;
                }
                "terminalThemes" => {
                    need("id")?;
                    need("label")?;
                    let colors = item["colors"].as_array().map_or(0, Vec::len);
                    if colors != 19 {
                        return Err(format!("{k}[{i}] needs 19 colors (background, foreground, cursor, ANSI 0–15)"));
                    }
                }
                "layouts" => {
                    need("name")?;
                    if !item["template"].is_object() {
                        return Err(format!("{k}[{i}] needs a \"template\""));
                    }
                }
                "statusItems" => {
                    need("id")?;
                    need("command")?;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[derive(Serialize)]
pub struct PluginInfo {
    id: String,
    dir: String,
    manifest: Option<Value>,
    hash: String,
    /// The manifest the user approved is this one.
    approved: bool,
    /// Approved once, but the manifest changed since: it needs reviewing again.
    changed: bool,
    error: Option<String>,
    /// The manifest's file name (`kelpie-plugin.json`, or the pre-rename `erka-plugin.json`).
    file: String,
}

fn read_plugin(dir: &Path, approvals: &BTreeMap<String, String>) -> PluginInfo {
    let id = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut info = PluginInfo { id: id.clone(), dir: dir.to_string_lossy().into_owned(), manifest: None, hash: String::new(), approved: false, changed: false, error: None, file: String::new() };
    let path = manifest_path(dir);
    info.file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            info.error = Some(format!("no {MANIFEST}"));
            return info;
        }
    };
    info.hash = sha256_hex(&bytes);
    info.approved = approvals.get(&id) == Some(&info.hash);
    info.changed = !info.approved && approvals.contains_key(&id);
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(m) => {
            if let Err(e) = validate(&m, &id) {
                info.error = Some(e);
            }
            info.manifest = Some(m);
        }
        Err(e) => info.error = Some(format!("{}: {e}", info.file)),
    }
    info
}

fn list_installed() -> Vec<PluginInfo> {
    let approvals = approvals();
    let mut out: Vec<PluginInfo> = std::fs::read_dir(plugins_dir())
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        // Dot folders are installs in progress.
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| read_plugin(&e.path(), &approvals))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Installed plugins as a settings export lists them: id, name and version (no approvals, no files).
pub fn export_list() -> Vec<Value> {
    list_installed()
        .into_iter()
        .map(|p| {
            let m = p.manifest.unwrap_or(Value::Null);
            let name = m["name"].as_str().unwrap_or(&p.id).to_string();
            let mut item = json!({ "id": p.id, "name": name });
            if let Some(v) = m["version"].as_str() {
                item["version"] = json!(v);
            }
            item
        })
        .collect()
}

pub fn valid_plugin_id(id: &str) -> bool {
    valid_id(id)
}

#[tauri::command]
pub async fn plugin_list() -> CmdResult<Vec<PluginInfo>> {
    crate::fs_ops::blocking(|| Ok(list_installed())).await
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let ty = e.file_type()?;
        if ty.is_dir() {
            copy_dir(&e.path(), &to.join(&name))?;
        } else if ty.is_file() {
            std::fs::copy(e.path(), to.join(&name))?;
        }
    }
    Ok(())
}

/// The id a folder's manifest declares (installing names the folder after it).
fn manifest_id(dir: &Path) -> CmdResult<String> {
    let m: Value = serde_json::from_slice(&std::fs::read(manifest_path(dir)).map_err(|_| format!("{} has no {MANIFEST}", dir.display()))?).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let id = m["id"].as_str().ok_or("manifest has no id")?.to_string();
    validate(&m, &id)?;
    Ok(id)
}

/// The folder plugins are installed in (created if missing).
#[tauri::command]
pub fn plugin_dir() -> CmdResult<String> {
    std::fs::create_dir_all(plugins_dir()).map_err(err)?;
    Ok(plugins_dir().to_string_lossy().into_owned())
}

/// Copies a plugin folder in (replacing an installed one of the same id when `replace`).
#[tauri::command]
pub async fn plugin_install_folder(src: String, replace: Option<bool>) -> CmdResult<String> {
    crate::fs_ops::blocking(move || {
        let src = PathBuf::from(src);
        let id = manifest_id(&src)?;
        let dest = plugins_dir().join(&id);
        if dest.exists() {
            if !replace.unwrap_or(false) {
                return Err(format!("a plugin {id} is already installed"));
            }
            std::fs::remove_dir_all(&dest).map_err(err)?;
        }
        copy_dir(&src, &dest).map_err(err)?;
        Ok(id)
    })
    .await
}

/// Remote repositories only (no local paths or `ext::` transports); plain http only on this computer.
fn git_url_allowed(url: &str) -> bool {
    let local_http = ["http://localhost:", "http://localhost/", "http://127.0.0.1:", "http://127.0.0.1/"].iter().any(|p| url.starts_with(p));
    (url.starts_with("https://") || url.starts_with("git@") || url.starts_with("ssh://") || local_http) && !url.contains(char::is_whitespace) && !url.starts_with('-')
}

/// Clones a plugin from a git URL (shallow), then installs it under its manifest id.
#[tauri::command]
pub async fn plugin_install_git(url: String) -> CmdResult<String> {
    if !git_url_allowed(&url) {
        return Err("use an https://, ssh:// or git@ repository URL".into());
    }
    let tmp = plugins_dir().join(format!(".install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(plugins_dir()).map_err(err)?;
    let clone = |shallow: bool| {
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("clone");
        if shallow {
            cmd.args(["--depth", "1"]);
        }
        cmd.args(["--", &url]).arg(&tmp).env("GIT_TERMINAL_PROMPT", "0").stdin(std::process::Stdio::null()).kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000);
        async move { tokio::time::timeout(Duration::from_secs(120), cmd.output()).await.map_err(|_| "git clone timed out".to_string())?.map_err(err) }
    };
    let mut out = clone(true).await?;
    // Static ("dumb") http hosting cannot serve a shallow clone.
    if !out.status.success() && String::from_utf8_lossy(&out.stderr).contains("shallow") {
        let _ = std::fs::remove_dir_all(&tmp);
        out = clone(false).await?;
    }
    if !out.status.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(stderr.lines().filter(|l| !l.starts_with("Cloning into")).collect::<Vec<_>>().join("\n").trim().to_string());
    }
    let result = crate::fs_ops::blocking({
        let tmp = tmp.clone();
        move || {
            let id = manifest_id(&tmp)?;
            let dest = plugins_dir().join(&id);
            if dest.exists() {
                return Err(format!("a plugin {id} is already installed"));
            }
            let _ = std::fs::remove_dir_all(tmp.join(".git"));
            std::fs::rename(&tmp, &dest).map_err(err)?;
            Ok(id)
        }
    })
    .await;
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Moves a plugin to the trash and forgets its approval.
#[tauri::command]
pub async fn plugin_remove(id: String) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        if !valid_id(&id) {
            return Err("invalid plugin id".into());
        }
        let dir = plugins_dir().join(&id);
        if dir.exists() {
            trash::delete(&dir).or_else(|_| std::fs::remove_dir_all(&dir)).map_err(err)?;
        }
        let mut a = approvals();
        a.remove(&id);
        save_approvals(&a)
    })
    .await
}

/// Approves the manifest with this hash (`None`: withdraws the approval).
#[tauri::command]
pub async fn plugin_approve(id: String, hash: Option<String>) -> CmdResult<()> {
    crate::fs_ops::blocking(move || {
        if !valid_id(&id) {
            return Err("invalid plugin id".into());
        }
        let mut a = approvals();
        match hash {
            Some(h) => {
                let current = read_plugin(&plugins_dir().join(&id), &a);
                if let Some(e) = current.error {
                    return Err(format!("the plugin is invalid: {e}"));
                }
                if current.hash != h {
                    return Err("the plugin changed since it was shown; review it again".into());
                }
                a.insert(id, h);
            }
            None => {
                a.remove(&id);
            }
        }
        save_approvals(&a)
    })
    .await
}

/// A starter plugin (folder + commented manifest) to edit.
#[tauri::command]
pub async fn plugin_create(id: String, name: String) -> CmdResult<String> {
    crate::fs_ops::blocking(move || {
        if !valid_id(&id) {
            return Err("id: lowercase letters, digits and - only".into());
        }
        let dir = plugins_dir().join(&id);
        if dir.exists() {
            return Err(format!("{id} already exists"));
        }
        // Built aside and moved in: a listing never sees the folder without its manifest.
        let tmp = plugins_dir().join(format!(".new-{id}"));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).map_err(err)?;
        let manifest = json!({
            "id": id,
            "name": name,
            "version": "0.1.0",
            "description": "What this plugin adds",
            "contributes": {
                "commands": [{ "id": "hello", "label": "Say hello", "run": "echo Hello from ${projectName}", "in": "terminal" }],
                "quickCommands": [{ "label": "Review the diff", "text": "Review the uncommitted changes and list problems.", "action": "agent" }],
                "statusItems": [{ "id": "branch", "label": "Branch", "command": "git rev-parse --abbrev-ref HEAD", "intervalSeconds": 30 }]
            }
        });
        std::fs::write(tmp.join(MANIFEST), serde_json::to_vec_pretty(&manifest).map_err(err)?).map_err(err)?;
        std::fs::rename(&tmp, &dir).map_err(err)?;
        Ok(dir.to_string_lossy().into_owned())
    })
    .await
}

/// Runs a status item's command (approved plugins only) in `cwd`; returns its output.
#[tauri::command]
pub async fn plugin_status(id: String, item: String, cwd: Option<String>) -> CmdResult<Value> {
    let (command, timeout) = crate::fs_ops::blocking({
        let (id, item) = (id.clone(), item.clone());
        move || {
            let a = approvals();
            let info = read_plugin(&plugins_dir().join(&id), &a);
            if !info.approved {
                return Err(format!("plugin {id} is not approved"));
            }
            let m = info.manifest.ok_or("no manifest")?;
            let it = m["contributes"]["statusItems"].as_array().and_then(|l| l.iter().find(|x| x["id"] == item.as_str())).cloned().ok_or("no such status item")?;
            Ok((it["command"].as_str().unwrap_or_default().to_string(), it["timeoutSeconds"].as_u64().unwrap_or(15).clamp(1, 60)))
        }
    })
    .await?;
    let mut cmd = if cfg!(windows) {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/d", "/s", "/c"]).arg(&command);
        c
    } else {
        let mut c = tokio::process::Command::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
        c.args(["-lc", &command]);
        c
    };
    if let Some(dir) = cwd.filter(|d| Path::new(d).is_dir()) {
        cmd.current_dir(dir);
    }
    cmd.stdin(std::process::Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let out = tokio::time::timeout(Duration::from_secs(timeout), cmd.output()).await.map_err(|_| "timed out".to_string())?.map_err(err)?;
    let text: String = String::from_utf8_lossy(&out.stdout).chars().take(2000).collect();
    let mut error: String = String::from_utf8_lossy(&out.stderr).chars().take(500).collect::<String>().trim().to_string();
    if !out.status.success() && error.is_empty() {
        error = format!("exited with {}", out.status.code().map_or("a signal".to_string(), |c| format!("code {c}")));
    }
    Ok(json!({"ok": out.status.success(), "text": text.trim(), "error": error}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_manifests() {
        let ok = json!({"id": "acme", "name": "Acme", "contributes": {"commands": [{"id": "a", "label": "A", "run": "echo"}]}});
        assert!(validate(&ok, "acme").is_ok());
        assert!(validate(&ok, "other").unwrap_err().contains("folder"));
        assert!(validate(&json!({"id": "Acme!", "name": "x"}), "Acme!").is_err());
        assert!(validate(&json!({"id": "a", "name": "x", "contributes": {"scripts": []}}), "a").unwrap_err().contains("unknown"));
        assert!(validate(&json!({"id": "a", "name": "x", "contributes": {"commands": [{"id": "c", "label": "C"}]}}), "a").unwrap_err().contains("run"));
        assert!(validate(&json!({"id": "a", "name": "x", "contributes": {"terminalThemes": [{"id": "t", "label": "T", "colors": ["#000"]}]}}), "a").unwrap_err().contains("19"));
    }

    #[test]
    fn git_urls() {
        assert!(git_url_allowed("https://github.com/a/b.git"));
        assert!(git_url_allowed("git@github.com:a/b.git"));
        assert!(git_url_allowed("http://127.0.0.1:8000/p.git"));
        assert!(!git_url_allowed("http://example.com/p.git"));
        assert!(!git_url_allowed("ext::sh -c touch% /tmp/x"));
        assert!(!git_url_allowed("C:/plugins/p"));
        assert!(!git_url_allowed("file:///c/p"));
    }
}
