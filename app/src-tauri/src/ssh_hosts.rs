//! Saved SSH hosts, keys and known hosts (the Hosts page). Kelpie keeps using the system OpenSSH
//! client: saved hosts become `Host` entries of a generated config (`<data>/ssh/config`) that
//! every ssh / scp Kelpie runs reads with `-F`, and that ends by including the user's own config,
//! so their aliases, keys and agent keep working. Passwords and key passphrases live in the OS
//! credential store; ssh asks for them through `kelpied` as its `SSH_ASKPASS` (see
//! `crates/kelpied/src/askpass.rs`). Hosts and port-forward rules are stored in
//! `<data>/ssh/hosts.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::RwLock;
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use crate::fs_ops::{blocking, err, CmdResult};

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Host {
    pub id: String,
    pub label: String,
    /// The `Host` name in the generated config: what ssh, remote projects and sessions use.
    pub alias: String,
    pub group: String,
    pub tags: Vec<String>,
    pub hostname: String,
    pub port: Option<u16>,
    pub user: String,
    /// `key` (a key file or the agent) or `password` (saved in the credential store).
    pub auth: String,
    pub identity_file: String,
    /// `ProxyJump`: another saved host's alias or `[user@]host[:port]`, comma separated.
    pub jump: String,
    pub forward_agent: bool,
    /// `ServerAliveInterval` in seconds (0 = ssh's default).
    pub keepalive: u32,
    /// `SetEnv` (the server must accept them).
    pub env: BTreeMap<String, String>,
    /// Typed into a new shell on the host (Termius' "startup snippet").
    pub startup: String,
    /// Folder that terminals and agents start in (empty: the login folder).
    pub folder: String,
    /// Keep terminals in the host's tmux when it has one.
    pub tmux: bool,
    pub notes: String,
    pub color: String,
    /// Terminal colour theme for its panes (a theme id; empty: the app's terminal theme).
    pub terminal_theme: String,
    /// Operating system seen on the last check (`ubuntu`, `debian`, `darwin`…), for its badge.
    pub os: String,
    pub last_connected: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Forward {
    pub id: String,
    pub host_id: String,
    pub label: String,
    /// `local` (-L), `remote` (-R) or `dynamic` (-D, SOCKS).
    pub kind: String,
    pub bind_address: String,
    pub bind_port: u16,
    pub dest_host: String,
    pub dest_port: u16,
    /// Started with the app.
    pub auto_start: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Vault {
    pub version: u32,
    pub hosts: Vec<Host>,
    pub forwards: Vec<Forward>,
}

impl Default for Vault {
    fn default() -> Self {
        Vault { version: 1, hosts: Vec::new(), forwards: Vec::new() }
    }
}

/// Settings → Remote: SSH config file. Empty: `~/.ssh/config`. The generated config includes it.
static USER_CONFIG: RwLock<Option<String>> = RwLock::new(None);
/// Serialises vault writes (the page saves while a connect records `lastConnected`).
static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default()
}

/// `~/.ssh`; `KELPIE_SSH_DIR` replaces it (the end-to-end tests never touch the real one).
fn ssh_dir() -> PathBuf {
    std::env::var_os("KELPIE_SSH_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".ssh"))
}

pub(crate) fn vault_path() -> PathBuf {
    kelpie_proto::data_dir().join("ssh").join("hosts.json")
}

/// The generated ssh config every Kelpie ssh / scp reads.
pub fn config_path() -> PathBuf {
    kelpie_proto::data_dir().join("ssh").join("config")
}

pub fn user_config() -> PathBuf {
    USER_CONFIG.read().unwrap().clone().map(PathBuf::from).unwrap_or_else(|| ssh_dir().join("config"))
}

/// Sets the user's own config (included by the generated one) and rewrites the generated file.
pub fn set_user_config(config: Option<String>) {
    *USER_CONFIG.write().unwrap() = config.filter(|c| !c.trim().is_empty());
    if let Err(e) = write_config(&load()) {
        tracing::warn!("ssh config: {e}");
    }
}

pub fn load() -> Vault {
    std::fs::read(vault_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The saved hosts, or an error when the file exists but cannot be read or understood: a caller that
/// writes the vault back must never take a damaged file for an empty one.
pub fn load_strict() -> CmdResult<Vault> {
    match std::fs::read(vault_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("the saved SSH hosts could not be read: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vault::default()),
        Err(e) => Err(format!("the saved SSH hosts could not be read: {e}")),
    }
}

/// Replaces the vault only while it is still `expected` (what the caller looked at); returns what was saved.
pub fn save_checked(expected: &Vault, next: Vault) -> CmdResult<Vault> {
    let v = normalize(next)?;
    let _guard = WRITE.lock().unwrap();
    if &load_strict()? != expected {
        return Err("the saved SSH hosts changed since the preview; import again".into());
    }
    let path = vault_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
    write_atomic(&path, &serde_json::to_vec_pretty(&v).map_err(err)?).map_err(err)?;
    write_config(&v)?;
    Ok(v)
}

/// `kelpied`, which answers ssh's password / passphrase prompts (`SSH_ASKPASS`).
fn askpass_exe() -> PathBuf {
    crate::daemon_client::daemon_exe()
}

/// Environment that lets ssh ask Kelpie for saved secrets instead of a terminal. `host_id` is the
/// host the connection is for (its password wins when a jump host has the same address).
pub fn askpass_env(host_id: &str) -> Vec<(String, String)> {
    vec![
        ("SSH_ASKPASS".into(), askpass_exe().to_string_lossy().into_owned()),
        ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        ("KELPIE_ASKPASS".into(), if host_id.is_empty() { "-".into() } else { host_id.to_string() }),
        // Older OpenSSH only uses askpass with a display set.
        ("DISPLAY".into(), std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into())),
    ]
}

/// A saved host by alias.
pub fn host_by_alias(alias: &str) -> Option<Host> {
    load().hosts.into_iter().find(|h| h.alias == alias)
}

/// An OpenSSH program. On Windows, the system's own (`System32\OpenSSH`) when present: a Git
/// Bash one earlier on PATH reads `C:\…` as a host named `C` and cannot reach the Windows
/// ssh-agent service.
pub fn program(name: &str) -> PathBuf {
    if cfg!(windows) {
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("OpenSSH").join(format!("{name}.exe"));
        if system.is_file() {
            return system;
        }
    }
    PathBuf::from(name)
}

/// A process for an OpenSSH tool (no console window on Windows).
pub fn tool(name: &str) -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(program(name));
    crate::daemon_runtime::clean_env(cmd.as_std_mut());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    cmd
}

fn not_found(program: &str) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| if e.kind() == std::io::ErrorKind::NotFound { format!("{program} is not installed (OpenSSH client)") } else { format!("{program}: {e}") }
}

/// Temp file + rename, so a crash never leaves a torn file.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

// ---------- validation and the generated config ----------

fn bad(s: &str) -> bool {
    s.chars().any(|c| c.is_control() || c == '"')
}

fn valid_alias(a: &str) -> bool {
    !a.is_empty() && a.len() <= 64 && !a.starts_with('-') && a.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn valid_hostname(h: &str) -> bool {
    !h.is_empty() && !h.starts_with('-') && h.len() <= 255 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '[' | ']' | '%'))
}

fn valid_user(u: &str) -> bool {
    !u.starts_with('-') && u.len() <= 64 && u.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '\\' | '$'))
}

fn valid_jump(j: &str) -> bool {
    j.split(',').all(|part| {
        let p = part.trim();
        !p.is_empty() && !p.starts_with('-') && p.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '@' | '[' | ']'))
    })
}

/// `My Server #2` → `my-server-2`.
fn slug(label: &str) -> String {
    let mut s = String::new();
    for c in label.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_matches('-').chars().take(48).collect::<String>();
    if s.is_empty() { "host".into() } else { s }
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{:016x}", h.finish())
}

/// Checks every field that reaches the config or a command line, fills ids and unique aliases.
pub fn normalize(mut v: Vault) -> CmdResult<Vault> {
    v.version = 1;
    let mut aliases = std::collections::HashSet::new();
    for h in &mut v.hosts {
        if h.id.is_empty() {
            h.id = new_id();
        }
        h.label = h.label.trim().to_string();
        h.hostname = h.hostname.trim().to_string();
        h.user = h.user.trim().to_string();
        if h.label.is_empty() {
            h.label = h.hostname.clone();
        }
        if !valid_hostname(&h.hostname) {
            return Err(format!("{}: invalid host name or address", h.label));
        }
        if !valid_user(&h.user) {
            return Err(format!("{}: invalid user name", h.label));
        }
        if !h.jump.trim().is_empty() && !valid_jump(&h.jump) {
            return Err(format!("{}: invalid jump host", h.label));
        }
        h.jump = h.jump.trim().to_string();
        if bad(&h.identity_file) || bad(&h.label) || bad(&h.group) {
            return Err(format!("{}: invalid characters", h.label));
        }
        for (k, val) in &h.env {
            if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.starts_with(|c: char| c.is_ascii_digit()) || bad(val) || val.contains('\\') {
                return Err(format!("{}: invalid environment variable {k}", h.label));
            }
        }
        if !matches!(h.auth.as_str(), "key" | "password") {
            h.auth = "key".into();
        }
        h.os = token(&h.os);
        // Theme ids come from plugins too (`plugin.theme`, any characters): only kept on one line.
        h.terminal_theme = h.terminal_theme.trim().chars().filter(|c| !c.is_control()).take(200).collect();
        let wanted = if h.alias.trim().is_empty() { slug(&h.label) } else { h.alias.trim().to_string() };
        if !valid_alias(&wanted) {
            return Err(format!("{}: the SSH alias may only have letters, digits, - _ .", h.label));
        }
        let mut alias = wanted.clone();
        let mut n = 2;
        while !aliases.insert(alias.clone()) {
            alias = format!("{wanted}-{n}");
            n += 1;
        }
        h.alias = alias;
    }
    let ids: std::collections::HashSet<String> = v.hosts.iter().map(|h| h.id.clone()).collect();
    v.forwards.retain(|f| ids.contains(&f.host_id));
    for f in &mut v.forwards {
        if f.id.is_empty() {
            f.id = new_id();
        }
        if !matches!(f.kind.as_str(), "local" | "remote" | "dynamic") {
            return Err(format!("{}: unknown forward kind", f.label));
        }
        if f.bind_port == 0 || (f.kind != "dynamic" && (f.dest_port == 0 || !valid_hostname(&f.dest_host))) {
            return Err(format!("{}: ports and destination are required", f.label));
        }
        if !f.bind_address.is_empty() && !valid_hostname(&f.bind_address) {
            return Err(format!("{}: invalid bind address", f.label));
        }
        if bad(&f.label) {
            return Err(format!("{}: invalid characters", f.label));
        }
    }
    Ok(v)
}

/// ssh config paths: forward slashes (Windows' OpenSSH reads them) and quoted.
fn config_quote(p: &str) -> String {
    format!("\"{}\"", p.replace('\\', "/"))
}

/// `known_hosts`: a replacement for ssh's own file (`KELPIE_SSH_DIR`), so the host keys the Hosts
/// page trusts are the ones ssh checks.
pub fn render_config(v: &Vault, include: &Path, known_hosts: Option<&Path>) -> String {
    let mut out = String::from("# Generated by Kelpie from hosts.json: change the hosts in Kelpie (Hosts page).\n");
    for h in &v.hosts {
        out += &format!("\nHost {}\n  HostName {}\n", h.alias, h.hostname);
        if let Some(p) = h.port {
            out += &format!("  Port {p}\n");
        }
        if !h.user.is_empty() {
            out += &format!("  User {}\n", h.user);
        }
        if !h.identity_file.trim().is_empty() {
            out += &format!("  IdentityFile {}\n  IdentitiesOnly yes\n", config_quote(h.identity_file.trim()));
        }
        if !h.jump.is_empty() {
            out += &format!("  ProxyJump {}\n", h.jump);
        }
        if h.forward_agent {
            out += "  ForwardAgent yes\n";
        }
        if h.keepalive > 0 {
            out += &format!("  ServerAliveInterval {}\n  ServerAliveCountMax 4\n", h.keepalive);
        }
        if h.auth == "password" {
            out += "  PreferredAuthentications password,keyboard-interactive,publickey\n";
        }
        if !h.env.is_empty() {
            out += &format!("  SetEnv {}\n", h.env.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect::<Vec<_>>().join(" "));
        }
    }
    // `Match all` ends the last Host block, so the include applies to every host.
    out += "\nMatch all\n";
    if let Some(k) = known_hosts {
        out += &format!("  UserKnownHostsFile {}\n", config_quote(&k.to_string_lossy()));
    }
    out += &format!("Include {}\n", config_quote(&include.to_string_lossy()));
    out
}

fn write_config(v: &Vault) -> CmdResult<()> {
    let path = config_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
    let known_hosts = std::env::var_os("KELPIE_SSH_DIR").map(|_| known_hosts_file());
    write_atomic(&path, render_config(v, &user_config(), known_hosts.as_deref()).as_bytes()).map_err(err)
}

/// Writes the generated config at startup (sessions and remote projects read it).
pub fn init() {
    if let Err(e) = write_config(&load()) {
        tracing::warn!("ssh config: {e}");
    }
}

// ---------- hosts ----------

#[tauri::command]
pub async fn ssh_vault_load() -> CmdResult<Vault> {
    blocking(|| Ok(load())).await
}

/// Saves hosts and forward rules; returns them with ids and aliases filled in.
#[tauri::command]
pub async fn ssh_vault_save(vault: Vault) -> CmdResult<Vault> {
    blocking(move || {
        let v = normalize(vault)?;
        let _guard = WRITE.lock().unwrap();
        let path = vault_path();
        std::fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
        write_atomic(&path, &serde_json::to_vec_pretty(&v).map_err(err)?).map_err(err)?;
        write_config(&v)?;
        Ok(v)
    })
    .await
}

/// A short identifier (an OS name): letters, digits, `.`, `_`, `-`.
fn token(s: &str) -> String {
    s.trim().chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).take(48).collect()
}

/// Records a connection to a host and the OS it reported (only those fields, so it never undoes
/// another change).
#[tauri::command]
pub async fn ssh_host_touch(id: String, os: Option<String>) -> CmdResult<Vault> {
    blocking(move || {
        let _guard = WRITE.lock().unwrap();
        let mut v = load();
        if let Some(h) = v.hosts.iter_mut().find(|h| h.id == id) {
            h.last_connected = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
            if let Some(os) = os.as_deref().map(token).filter(|o| !o.is_empty()) {
                h.os = os;
            }
            write_atomic(&vault_path(), &serde_json::to_vec_pretty(&v).map_err(err)?).map_err(err)?;
        }
        Ok(v)
    })
    .await
}

#[derive(Serialize)]
pub struct SshPaths {
    config: String,
    known_hosts: String,
    ssh_dir: String,
}

#[tauri::command]
pub fn ssh_paths() -> SshPaths {
    SshPaths {
        config: config_path().to_string_lossy().into_owned(),
        known_hosts: ssh_dir().join("known_hosts").to_string_lossy().into_owned(),
        ssh_dir: ssh_dir().to_string_lossy().into_owned(),
    }
}

/// Environment for a terminal, agent or port forward to a saved host: answers its password /
/// passphrase prompts from the credential store. Empty when nothing is saved, so ssh asks in the
/// terminal as usual.
#[tauri::command]
pub async fn ssh_session_env(alias: String) -> CmdResult<BTreeMap<String, String>> {
    blocking(move || {
        let alias = alias.rsplit('@').next().unwrap_or(&alias).to_string();
        let Some(h) = host_by_alias(&alias) else { return Ok(BTreeMap::new()) };
        let saved = (h.auth == "password" && secret(&kelpie_proto::ssh_password_key(&h.id)).is_some())
            || (!h.identity_file.trim().is_empty() && secret(&kelpie_proto::ssh_passphrase_key(&expand_home(&h.identity_file))).is_some());
        Ok(if saved { askpass_env(&h.id).into_iter().collect() } else { BTreeMap::new() })
    })
    .await
}

/// Hosts of the user's ssh config (`Host` blocks without wildcards) as saved-host drafts.
#[tauri::command]
pub async fn ssh_import_config() -> CmdResult<Vec<Host>> {
    blocking(|| Ok(parse_config(&std::fs::read_to_string(user_config()).unwrap_or_default()))).await
}

pub fn parse_config(text: &str) -> Vec<Host> {
    let mut out: Vec<Host> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (key, value) = match l.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_ascii_lowercase(), v.trim().trim_start_matches('=').trim().trim_matches('"').to_string()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for name in value.split_whitespace() {
                    if name.contains(['*', '?', '!']) {
                        continue;
                    }
                    current.push(out.len());
                    out.push(Host { label: name.into(), alias: name.into(), hostname: name.into(), auth: "key".into(), tmux: true, ..Default::default() });
                }
            }
            "match" => current.clear(),
            _ => {
                for &i in &current {
                    let h = &mut out[i];
                    match key.as_str() {
                        "hostname" => h.hostname = value.clone(),
                        "user" => h.user = value.clone(),
                        "port" => h.port = value.parse().ok(),
                        "identityfile" if h.identity_file.is_empty() => h.identity_file = value.clone(),
                        "proxyjump" => h.jump = value.clone(),
                        "forwardagent" => h.forward_agent = value.eq_ignore_ascii_case("yes"),
                        "serveraliveinterval" => h.keepalive = value.parse().unwrap_or(0),
                        _ => {}
                    }
                }
            }
        }
    }
    out
}

// ---------- secrets ----------

fn secret(key: &str) -> Option<String> {
    kelpie_proto::secret_get(key)
}

fn set_secret(key: &str, value: &str) -> CmdResult<()> {
    if value.is_empty() {
        return kelpie_proto::secret_delete(key).map_err(err);
    }
    kelpie_proto::secret_set(key, value).map_err(err)
}

fn secret_key(kind: &str, target: &str) -> CmdResult<String> {
    match kind {
        "password" if !target.is_empty() && target.chars().all(|c| c.is_ascii_alphanumeric()) => Ok(kelpie_proto::ssh_password_key(target)),
        "passphrase" if !target.trim().is_empty() && !bad(target) => Ok(kelpie_proto::ssh_passphrase_key(&expand_home(target))),
        _ => Err("unknown secret".into()),
    }
}

/// Saves (empty value: forgets) a host's password (`kind` password, `target` host id) or a
/// key's passphrase (`kind` passphrase, `target` key path). Values never come back to the UI.
#[tauri::command]
pub async fn ssh_secret_set(kind: String, target: String, value: String) -> CmdResult<()> {
    blocking(move || set_secret(&secret_key(&kind, &target)?, &value)).await
}

#[tauri::command]
pub async fn ssh_secret_has(kind: String, target: String) -> CmdResult<bool> {
    blocking(move || Ok(secret(&secret_key(&kind, &target)?).is_some())).await
}

fn expand_home(p: &str) -> String {
    let p = p.trim();
    match p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        Some(rest) => home().join(rest).to_string_lossy().into_owned(),
        None => p.to_string(),
    }
}

// ---------- keys ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyInfo {
    path: String,
    name: String,
    kind: String,
    bits: u32,
    fingerprint: String,
    comment: String,
    public: String,
    /// Protected by a passphrase.
    encrypted: bool,
    passphrase_saved: bool,
}

/// `<type> <base64> [comment]` → (type, SHA256 fingerprint as ssh prints it, comment).
pub fn public_key_info(line: &str) -> Option<(String, String, String)> {
    let mut it = line.split_whitespace();
    let kind = it.next()?.to_string();
    let blob = base64::engine::general_purpose::STANDARD.decode(it.next()?).ok()?;
    let fp = base64::engine::general_purpose::STANDARD_NO_PAD.encode(sha2::Sha256::digest(&blob));
    Some((kind, format!("SHA256:{fp}"), it.collect::<Vec<_>>().join(" ")))
}

/// Private keys in `~/.ssh` (files with a matching `.pub`).
#[tauri::command]
pub async fn ssh_keys_list() -> CmdResult<Vec<KeyInfo>> {
    let dir = ssh_dir();
    let mut keys = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(keys) };
    let mut names: Vec<String> = rd.filter_map(|e| e.ok()).filter_map(|e| e.file_name().into_string().ok()).collect();
    names.sort();
    for name in names.iter().filter(|n| n.ends_with(".pub")) {
        let private = dir.join(name.trim_end_matches(".pub"));
        if !private.is_file() {
            continue;
        }
        let public = std::fs::read_to_string(dir.join(name)).unwrap_or_default().trim().to_string();
        let Some((kind, fingerprint, comment)) = public_key_info(&public) else { continue };
        let path = private.to_string_lossy().into_owned();
        // `-y` with an empty passphrase fails only for an encrypted key.
        let encrypted = match tool("ssh-keygen").args(["-y", "-P", "", "-f"]).arg(&private).output().await {
            Ok(o) => !o.status.success(),
            Err(_) => false,
        };
        let bits = match kind.as_str() {
            "ssh-ed25519" => 256,
            k if k.starts_with("ecdsa-sha2-nistp") => k.trim_start_matches("ecdsa-sha2-nistp").parse().unwrap_or(0),
            _ => tool("ssh-keygen").arg("-lf").arg(dir.join(name)).output().await.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).split_whitespace().next().and_then(|b| b.parse().ok())).unwrap_or(0),
        };
        let p = path.clone();
        let passphrase_saved = blocking(move || Ok(secret(&kelpie_proto::ssh_passphrase_key(&p)).is_some())).await.unwrap_or(false);
        keys.push(KeyInfo { name: name.trim_end_matches(".pub").to_string(), path, kind, bits, fingerprint, comment, public, encrypted, passphrase_saved });
    }
    Ok(keys)
}

fn valid_key_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 64 && !n.starts_with(['-', '.']) && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// `ssh-keygen` into `~/.ssh/<name>`; with `save_passphrase`, the passphrase goes to the
/// credential store so Kelpie can unlock the key.
#[tauri::command]
pub async fn ssh_key_generate(name: String, kind: String, passphrase: String, comment: String, save_passphrase: bool) -> CmdResult<String> {
    if !valid_key_name(&name) {
        return Err("key name: letters, digits, - _ . only".into());
    }
    let (t, bits) = match kind.as_str() {
        "ed25519" => ("ed25519", None),
        "rsa" => ("rsa", Some("4096")),
        "ecdsa" => ("ecdsa", Some("521")),
        _ => return Err("unknown key type".into()),
    };
    let dir = ssh_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let path = dir.join(&name);
    if path.exists() || dir.join(format!("{name}.pub")).exists() {
        return Err(format!("{} already exists", path.display()));
    }
    let mut cmd = tool("ssh-keygen");
    cmd.args(["-q", "-t", t]);
    if let Some(b) = bits {
        cmd.args(["-b", b]);
    }
    cmd.arg("-f").arg(&path).arg("-N").arg(&passphrase).arg("-C").arg(if comment.trim().is_empty() { name.clone() } else { comment.trim().to_string() });
    let out = cmd.output().await.map_err(not_found("ssh-keygen"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let p = path.to_string_lossy().into_owned();
    if save_passphrase && !passphrase.is_empty() {
        let (k, v) = (kelpie_proto::ssh_passphrase_key(&p), passphrase.clone());
        blocking(move || set_secret(&k, &v)).await?;
    }
    Ok(p)
}

/// Copies a private key into `~/.ssh/<name>` and writes its public half next to it.
#[tauri::command]
pub async fn ssh_key_import(source: String, name: String, passphrase: String) -> CmdResult<String> {
    if !valid_key_name(&name) {
        return Err("key name: letters, digits, - _ . only".into());
    }
    let dir = ssh_dir();
    std::fs::create_dir_all(&dir).map_err(err)?;
    let target = dir.join(&name);
    if target.exists() {
        return Err(format!("{} already exists", target.display()));
    }
    let out = tool("ssh-keygen").args(["-y", "-P"]).arg(&passphrase).arg("-f").arg(&source).output().await.map_err(not_found("ssh-keygen"))?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if e.contains("incorrect passphrase") || passphrase.is_empty() && e.contains("passphrase") { "this key needs its passphrase".into() } else { format!("not a usable private key: {e}") });
    }
    std::fs::copy(&source, &target).map_err(err)?;
    std::fs::write(dir.join(format!("{name}.pub")), &out.stdout).map_err(err)?;
    Ok(target.to_string_lossy().into_owned())
}

/// Moves a key pair from `~/.ssh` to the recycle bin.
#[tauri::command]
pub async fn ssh_key_delete(path: String) -> CmdResult<()> {
    let p = PathBuf::from(&path);
    if p.parent() != Some(ssh_dir().as_path()) || !valid_key_name(&p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()) {
        return Err("not a key in ~/.ssh".into());
    }
    let public = PathBuf::from(format!("{path}.pub"));
    let items: Vec<PathBuf> = [p, public].into_iter().filter(|x| x.exists()).collect();
    trash::delete_all(&items).map_err(err)?;
    let key = kelpie_proto::ssh_passphrase_key(&path);
    blocking(move || set_secret(&key, "")).await
}

/// Adds the public key to `~/.ssh/authorized_keys` on the host (like `ssh-copy-id`).
#[tauri::command]
pub async fn ssh_key_install(alias: String, key_path: String) -> CmdResult<()> {
    let public = std::fs::read_to_string(format!("{key_path}.pub")).map_err(|_| "the key has no .pub file".to_string())?;
    let line = public.trim();
    public_key_info(line).ok_or("not a public key")?;
    let r = crate::remote::parse_root(&format!("ssh://{alias}/")).ok_or("invalid host")?;
    let q = crate::remote::q(line);
    let script = format!("umask 077; mkdir -p ~/.ssh && touch ~/.ssh/authorized_keys && (grep -qxF {q} ~/.ssh/authorized_keys || printf '%s\\n' {q} >> ~/.ssh/authorized_keys)");
    crate::remote::exec_ok(&r, &script).await.map(|_| ())
}

#[derive(Serialize)]
pub struct AgentState {
    /// An agent answered (`ssh-add -l` exit 0 or 1).
    running: bool,
    keys: Vec<String>,
    error: String,
}

#[tauri::command]
pub async fn ssh_agent_list() -> CmdResult<AgentState> {
    let out = tool("ssh-add").arg("-l").output().await.map_err(not_found("ssh-add"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(match out.status.code() {
        Some(0) => AgentState { running: true, keys: stdout.lines().map(str::to_string).collect(), error: String::new() },
        Some(1) => AgentState { running: true, keys: Vec::new(), error: String::new() },
        _ => AgentState { running: false, keys: Vec::new(), error: String::from_utf8_lossy(&out.stderr).trim().to_string() },
    })
}

/// `ssh-add <key>`; a saved passphrase unlocks it.
#[tauri::command]
pub async fn ssh_agent_add(key_path: String) -> CmdResult<()> {
    let out = tool("ssh-add").envs(askpass_env("")).arg(&key_path).output().await.map_err(not_found("ssh-add"))?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if msg.is_empty() { format!("scp failed (exit code {:?})", out.status.code()) } else { msg })
}

// ---------- known hosts ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnownHost {
    /// 0-based line in the file (what `ssh_known_hosts_remove` takes).
    line: usize,
    hosts: String,
    hashed: bool,
    kind: String,
    fingerprint: String,
    marker: String,
}

fn known_hosts_file() -> PathBuf {
    ssh_dir().join("known_hosts")
}

pub fn parse_known_hosts(text: &str) -> Vec<KnownHost> {
    text.lines()
        .enumerate()
        .filter_map(|(i, l)| {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                return None;
            }
            let (marker, rest) = if l.starts_with('@') { l.split_once(char::is_whitespace).map(|(m, r)| (m.to_string(), r.trim()))? } else { (String::new(), l) };
            let (hosts, key) = rest.split_once(char::is_whitespace)?;
            let (kind, fingerprint, _) = public_key_info(key.trim())?;
            Some(KnownHost { line: i, hashed: hosts.starts_with("|1|"), hosts: hosts.to_string(), kind, fingerprint, marker })
        })
        .collect()
}

#[tauri::command]
pub async fn ssh_known_hosts_list() -> CmdResult<Vec<KnownHost>> {
    blocking(|| Ok(parse_known_hosts(&std::fs::read_to_string(known_hosts_file()).unwrap_or_default()))).await
}

/// Removes entries by line; the previous file is kept as `known_hosts.old` (as ssh-keygen does).
#[tauri::command]
pub async fn ssh_known_hosts_remove(lines: Vec<usize>) -> CmdResult<()> {
    blocking(move || {
        let file = known_hosts_file();
        let text = std::fs::read_to_string(&file).map_err(err)?;
        let kept: Vec<&str> = text.lines().enumerate().filter(|(i, _)| !lines.contains(i)).map(|(_, l)| l).collect();
        std::fs::copy(&file, file.with_extension("old")).map_err(err)?;
        write_atomic(&file, format!("{}\n", kept.join("\n")).as_bytes()).map_err(err)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScannedKey {
    kind: String,
    fingerprint: String,
    /// The known_hosts line to add.
    line: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostKeys {
    /// The name known_hosts uses (`host` or `[host]:port`).
    name: String,
    keys: Vec<ScannedKey>,
    /// known_hosts already has a key for this name.
    known: bool,
    /// ...and it matches one of the scanned keys.
    matches: bool,
}

/// The host's keys (`ssh-keyscan`, using the address and port the config resolves for `alias`),
/// so the user can compare fingerprints before trusting it.
#[tauri::command]
pub async fn ssh_host_keys(alias: String) -> CmdResult<HostKeys> {
    if !valid_alias(&alias) && !valid_jump(&alias) {
        return Err("invalid host".into());
    }
    let dest = alias.rsplit('@').next().unwrap_or(&alias).to_string();
    let g = tool("ssh").arg("-F").arg(config_path()).arg("-G").arg(&alias).output().await.map_err(not_found("ssh"))?;
    let cfg = String::from_utf8_lossy(&g.stdout);
    let get = |k: &str| cfg.lines().find_map(|l| l.strip_prefix(&format!("{k} ")).map(str::to_string));
    let hostname = get("hostname").unwrap_or(dest);
    let port: u16 = get("port").and_then(|p| p.parse().ok()).unwrap_or(22);
    if !valid_hostname(&hostname) {
        return Err("invalid host name".into());
    }
    let out = tokio::time::timeout(Duration::from_secs(20), tool("ssh-keyscan").args(["-T", "10", "-p", &port.to_string(), &hostname]).output())
        .await
        .map_err(|_| format!("{hostname} did not answer"))?
        .map_err(not_found("ssh-keyscan"))?;
    let keys: Vec<ScannedKey> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (_, key) = l.split_once(char::is_whitespace)?;
            let (kind, fingerprint, _) = public_key_info(key)?;
            Some(ScannedKey { kind, fingerprint, line: l.trim().to_string() })
        })
        .collect();
    if keys.is_empty() {
        return Err(format!("no host keys from {hostname}:{port}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    let name = if port == 22 { hostname.clone() } else { format!("[{hostname}]:{port}") };
    let found = tool("ssh-keygen").arg("-F").arg(&name).arg("-f").arg(known_hosts_file()).output().await.map(|o| String::from_utf8_lossy(&o.stdout).into_owned()).unwrap_or_default();
    let known_fps: Vec<String> = found.lines().filter(|l| !l.starts_with('#')).filter_map(|l| l.split_once(char::is_whitespace).and_then(|(_, k)| public_key_info(k)).map(|i| i.1)).collect();
    let matches = keys.iter().any(|k| known_fps.contains(&k.fingerprint));
    Ok(HostKeys { name, known: !known_fps.is_empty(), matches, keys })
}

/// Appends scanned host keys to known_hosts.
#[tauri::command]
pub async fn ssh_host_trust(lines: Vec<String>) -> CmdResult<()> {
    blocking(move || {
        let mut add = String::new();
        for l in &lines {
            let (hosts, key) = l.trim().split_once(char::is_whitespace).ok_or("invalid key line")?;
            if hosts.is_empty() || l.contains(['\n', '\r']) || public_key_info(key).is_none() {
                return Err("invalid key line".into());
            }
            add += l.trim();
            add.push('\n');
        }
        let file = known_hosts_file();
        std::fs::create_dir_all(file.parent().unwrap()).map_err(err)?;
        let mut text = std::fs::read_to_string(&file).unwrap_or_default();
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text += &add;
        write_atomic(&file, text.as_bytes()).map_err(err)
    })
    .await
}

// ---------- port forwards ----------

/// Running `ssh -N` forwards by rule id. They stop with the app (rules marked auto-start are
/// started again when it opens).
#[derive(Default)]
pub struct Forwards(std::sync::Mutex<std::collections::HashMap<String, ForwardProc>>);

struct ForwardProc {
    child: tokio::process::Child,
    stderr: std::sync::Arc<std::sync::Mutex<String>>,
}

/// `-L` / `-R` / `-D` and its spec for a rule.
pub fn forward_flag(f: &Forward) -> (String, String) {
    let bind = if f.bind_address.is_empty() { String::new() } else if f.bind_address.contains(':') { format!("[{}]:", f.bind_address) } else { format!("{}:", f.bind_address) };
    let dest = if f.dest_host.contains(':') { format!("[{}]", f.dest_host) } else { f.dest_host.clone() };
    match f.kind.as_str() {
        "remote" => ("-R".into(), format!("{bind}{}:{dest}:{}", f.bind_port, f.dest_port)),
        "dynamic" => ("-D".into(), format!("{bind}{}", f.bind_port)),
        _ => ("-L".into(), format!("{bind}{}:{dest}:{}", f.bind_port, f.dest_port)),
    }
}

#[derive(Serialize)]
pub struct ForwardState {
    running: bool,
    /// Why it stopped (ssh's own message).
    error: String,
}

/// Starts a saved rule; fails with ssh's message when it exits within the first seconds (port
/// in use, login refused, unknown host key).
#[tauri::command]
pub async fn ssh_forward_start(state: tauri::State<'_, Forwards>, id: String) -> CmdResult<()> {
    let v = load();
    let f = v.forwards.iter().find(|f| f.id == id).cloned().ok_or("no such forward")?;
    let h = v.hosts.iter().find(|h| h.id == f.host_id).cloned().ok_or("the forward's host was removed")?;
    if let Some(p) = state.0.lock().unwrap().get_mut(&id) {
        if p.child.try_wait().ok().flatten().is_none() {
            return Ok(());
        }
    }
    let (flag, spec) = forward_flag(&f);
    let mut cmd = tool("ssh");
    cmd.envs(askpass_env(&h.id))
        .arg("-F")
        .arg(config_path())
        .args(["-N", "-o", "ExitOnForwardFailure=yes", "-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=3", "-o", "NumberOfPasswordPrompts=1", "-o", "ConnectTimeout=15"])
        .arg(flag)
        .arg(spec)
        .arg(&h.alias)
        .stdout(Stdio::null());
    let mut child = cmd.spawn().map_err(not_found("ssh"))?;
    let stderr = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    if let Some(mut pipe) = child.stderr.take() {
        let sink = stderr.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                let mut s = sink.lock().unwrap();
                if s.len() < 16 << 10 {
                    s.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
            }
        });
    }
    // ssh reports a refused login or a busy port within a moment; a working forward stays up.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if child.try_wait().map_err(err)?.is_some() {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let msg = stderr.lock().unwrap().trim().to_string();
            return Err(if msg.is_empty() { "ssh exited".into() } else { msg });
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    state.0.lock().unwrap().insert(id, ForwardProc { child, stderr });
    Ok(())
}

#[tauri::command]
pub async fn ssh_forward_stop(state: tauri::State<'_, Forwards>, id: String) -> CmdResult<()> {
    let proc = state.0.lock().unwrap().remove(&id);
    if let Some(mut p) = proc {
        let _ = p.child.kill().await;
    }
    Ok(())
}

/// State of every rule started in this run of the app.
#[tauri::command]
pub fn ssh_forward_status(state: tauri::State<'_, Forwards>) -> BTreeMap<String, ForwardState> {
    let mut map = state.0.lock().unwrap();
    map.iter_mut()
        .map(|(id, p)| {
            let running = matches!(p.child.try_wait(), Ok(None));
            (id.clone(), ForwardState { running, error: if running { String::new() } else { p.stderr.lock().unwrap().trim().to_string() } })
        })
        .collect()
}

// ---------- file transfer ----------

/// Copies files and folders between this computer and a host with `scp -r`: `upload` sends
/// local `sources` into the remote folder `target`; otherwise remote `sources` land in the
/// local folder `target`.
#[tauri::command]
pub async fn ssh_transfer(alias: String, upload: bool, sources: Vec<String>, target: String) -> CmdResult<()> {
    if !valid_alias(&alias) && !valid_jump(&alias) {
        return Err("invalid host".into());
    }
    if sources.is_empty() {
        return Ok(());
    }
    let r = crate::remote::parse_root(&format!("ssh://{alias}/")).ok_or("invalid host")?;
    let remote_path = |p: &str| -> CmdResult<String> {
        if !p.starts_with('/') || p.contains(['\n', '\r']) {
            return Err(format!("{p}: not an absolute path on the host"));
        }
        Ok(match r.port {
            Some(_) => format!("scp://{}{}", r.dest, p),
            None => format!("{}:{p}", r.dest),
        })
    };
    // `-S`: scp starts `ssh` from PATH otherwise (see `program`).
    let mut args: Vec<String> = vec!["-S".into(), program("ssh").to_string_lossy().into_owned(), "-F".into(), config_path().to_string_lossy().into_owned(), "-r".into(), "-p".into(), "-o".into(), "ConnectTimeout=15".into(), "-o".into(), "NumberOfPasswordPrompts=1".into()];
    if let Some(p) = r.port {
        args.extend(["-P".into(), p.to_string()]);
    }
    if upload {
        for s in &sources {
            if !Path::new(s).exists() {
                return Err(format!("{s} does not exist"));
            }
            args.push(s.clone());
        }
        args.push(remote_path(&format!("{}/", target.trim_end_matches('/')))?);
    } else {
        for s in &sources {
            args.push(remote_path(s)?);
        }
        if !Path::new(&target).is_dir() {
            return Err(format!("{target} is not a folder"));
        }
        args.push(target);
    }
    let host_id = host_by_alias(&r.dest).map(|h| h.id).unwrap_or_default();
    let out = tokio::time::timeout(Duration::from_secs(3600), tool("scp").envs(askpass_env(&host_id)).args(&args).output())
        .await
        .map_err(|_| "the transfer took more than an hour".to_string())?
        .map_err(not_found("scp"))?;
    if out.status.success() {
        return Ok(());
    }
    let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
    Err(if msg.is_empty() { format!("scp failed (exit code {:?})", out.status.code()) } else { msg })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(label: &str, hostname: &str) -> Host {
        Host { label: label.into(), hostname: hostname.into(), ..Default::default() }
    }

    #[test]
    fn normalizes_and_rejects_injection() {
        let v = normalize(Vault { hosts: vec![host("My Server #2", "10.0.0.2"), host("my server 2", "10.0.0.3")], ..Default::default() }).unwrap();
        assert_eq!(v.hosts[0].alias, "my-server-2");
        assert_eq!(v.hosts[1].alias, "my-server-2-2");
        assert!(!v.hosts[0].id.is_empty());
        assert_eq!(v.hosts[0].auth, "key");
        for bad_host in ["-oProxyCommand=calc", "a b", "x\nHost *"] {
            assert!(normalize(Vault { hosts: vec![host("x", bad_host)], ..Default::default() }).is_err(), "{bad_host}");
        }
        let mut h = host("x", "box");
        h.user = "me\n  ProxyCommand calc".into();
        assert!(normalize(Vault { hosts: vec![h], ..Default::default() }).is_err());
        let mut h = host("x", "box");
        h.identity_file = "C:\\k\"\nProxyCommand x".into();
        assert!(normalize(Vault { hosts: vec![h], ..Default::default() }).is_err());
        let mut h = host("x", "box");
        h.env.insert("A".into(), "1\" B=2".into());
        assert!(normalize(Vault { hosts: vec![h], ..Default::default() }).is_err());
        // Display-only fields are reduced to an identifier, never rejected.
        let mut h = host("x", "box");
        h.os = " Ubuntu 22.04\nHost *".into();
        h.terminal_theme = "my plugin.Night Owl\n".into();
        let v = normalize(Vault { hosts: vec![h], ..Default::default() }).unwrap();
        assert_eq!(v.hosts[0].os, "Ubuntu22.04Host");
        assert_eq!(v.hosts[0].terminal_theme, "my plugin.Night Owl");
    }

    #[test]
    fn drops_forwards_of_removed_hosts() {
        let mut h = host("a", "box");
        h.id = "h1".into();
        let f = |host_id: &str| Forward { host_id: host_id.into(), kind: "local".into(), bind_port: 8080, dest_host: "localhost".into(), dest_port: 80, ..Default::default() };
        let v = normalize(Vault { hosts: vec![h], forwards: vec![f("h1"), f("gone")], ..Default::default() }).unwrap();
        assert_eq!(v.forwards.len(), 1);
        assert!(normalize(Vault { hosts: vec![host("a", "b")], forwards: vec![Forward { host_id: "x".into(), kind: "local".into(), ..Default::default() }], ..Default::default() }).is_ok());
    }

    #[test]
    fn renders_config_with_include_last() {
        let mut h = host("Prod", "10.0.0.5");
        h.alias = "prod".into();
        h.port = Some(2222);
        h.user = "deploy".into();
        h.identity_file = "C:\\Users\\me\\.ssh\\id_ed25519".into();
        h.jump = "bastion".into();
        h.forward_agent = true;
        h.keepalive = 30;
        h.auth = "password".into();
        h.env.insert("LANG".into(), "C.UTF-8".into());
        let text = render_config(&Vault { hosts: vec![h], ..Default::default() }, Path::new("C:\\Users\\me\\.ssh\\config"), None);
        assert!(text.contains("Host prod\n  HostName 10.0.0.5\n  Port 2222\n  User deploy\n  IdentityFile \"C:/Users/me/.ssh/id_ed25519\"\n  IdentitiesOnly yes\n  ProxyJump bastion\n  ForwardAgent yes\n  ServerAliveInterval 30\n"));
        assert!(text.contains("PreferredAuthentications password"));
        assert!(text.contains("SetEnv LANG=\"C.UTF-8\""));
        assert!(text.trim_end().ends_with("Match all\nInclude \"C:/Users/me/.ssh/config\""));
        let text = render_config(&Vault::default(), Path::new("/c"), Some(Path::new("/t/known_hosts")));
        assert!(text.contains("Match all\n  UserKnownHostsFile \"/t/known_hosts\"\nInclude \"/c\""));
    }

    #[test]
    fn imports_ssh_config() {
        let cfg = "Host *\n  User root\nHost devbox gpu\n  HostName 1.2.3.4\n  User me\n  Port 2200\n  IdentityFile ~/.ssh/dev\n  ForwardAgent yes\nHost bastion-?\nHost web\n  HostName=web.example.com\nMatch host x\n  User nobody\n";
        let hosts = parse_config(cfg);
        assert_eq!(hosts.iter().map(|h| h.alias.as_str()).collect::<Vec<_>>(), vec!["devbox", "gpu", "web"]);
        assert_eq!((hosts[0].hostname.as_str(), hosts[0].user.as_str(), hosts[0].port, hosts[0].identity_file.as_str(), hosts[0].forward_agent), ("1.2.3.4", "me", Some(2200), "~/.ssh/dev", true));
        assert_eq!(hosts[1].hostname, "1.2.3.4");
        assert_eq!(hosts[2].hostname, "web.example.com");
        assert_eq!(hosts[2].user, "");
    }

    #[test]
    fn builds_forward_flags() {
        let f = |kind: &str, bind: &str| Forward { kind: kind.into(), bind_address: bind.into(), bind_port: 8080, dest_host: "db.internal".into(), dest_port: 5432, ..Default::default() };
        assert_eq!(forward_flag(&f("local", "")), ("-L".into(), "8080:db.internal:5432".into()));
        assert_eq!(forward_flag(&f("local", "0.0.0.0")), ("-L".into(), "0.0.0.0:8080:db.internal:5432".into()));
        assert_eq!(forward_flag(&f("remote", "")), ("-R".into(), "8080:db.internal:5432".into()));
        assert_eq!(forward_flag(&f("dynamic", "::1")), ("-D".into(), "[::1]:8080".into()));
    }

    #[test]
    fn fingerprints_like_ssh() {
        // ssh-keygen -lf prints SHA256:... for this key.
        let line = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl test@example";
        let (kind, fp, comment) = public_key_info(line).unwrap();
        assert_eq!(kind, "ssh-ed25519");
        assert!(fp.starts_with("SHA256:") && !fp.ends_with('='));
        assert_eq!(comment, "test@example");
        let known = parse_known_hosts(&format!("# c\n\nbox,10.0.0.1 {}\n|1|abc=|def= {}\n@revoked old {}\n", &line[..line.rfind(' ').unwrap()], &line[..line.rfind(' ').unwrap()], &line[..line.rfind(' ').unwrap()]));
        assert_eq!(known.len(), 3);
        assert_eq!(known[0].line, 2);
        assert!(known[1].hashed);
        assert_eq!(known[2].marker, "@revoked");
        assert_eq!(known[0].fingerprint, fp);
    }
}
