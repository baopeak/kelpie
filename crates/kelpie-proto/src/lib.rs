//! Wire protocol shared by `kelpied` (session daemon) and its clients (the Kelpie app, hook
//! helper, benchmarks).
//!
//! Every frame is `u32 LE length | u8 kind | payload` where `length` counts `kind + payload`.
//! - `KIND_JSON`: UTF-8 JSON (requests, replies, events).
//! - `KIND_OUTPUT`: daemon → client terminal bytes: `u32 handle | u64 seq | bytes`.
//!   `seq` is the stream offset *after* this chunk, so a client can resume with it.
//! - `KIND_INPUT`: client → daemon keystrokes: `u32 handle | bytes`.
//!
//! Terminal bytes never go through JSON so the hot path stays allocation-light.

use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub mod materialize;
pub mod redact;
pub mod worktree_lock;
pub use redact::redact;

pub const PROTOCOL_VERSION: u32 = 1;

/// Feature level of the daemon's RPC surface. Unlike `PROTOCOL_VERSION` (which names the
/// endpoint, so changing it orphans running daemons) this only lets a newer app notice an
/// older daemon that is still running and offer to restart it. 14: a run's task may run on an SSH
/// host (`runs::TaskHost`); an older daemon would ignore the field and run it here, so the app asks for it only at 14. 15: automations can have event
/// triggers and approved commands; an older daemon would ignore those fields, so the app offers them only at 15.
pub const API_LEVEL: u32 = 15;
/// Raised by a release that fixes a security problem in the daemon. Updates keep the previous
/// daemon running with its sessions; an app whose level is higher keeps asking the user to switch
/// (it never stops sessions by itself). Daemons that predate it report none: level 1.
pub const DAEMON_SECURITY_LEVEL: u32 = 1;
pub const KIND_JSON: u8 = 1;
pub const KIND_OUTPUT: u8 = 2;
pub const KIND_INPUT: u8 = 3;
/// Hard cap so a corrupted length prefix can never trigger a huge allocation.
pub const MAX_FRAME_LEN: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Json(Vec<u8>),
    Output { handle: u32, seq: u64, data: Vec<u8> },
    Input { handle: u32, data: Vec<u8> },
}

impl Frame {
    pub fn json<T: serde::Serialize>(value: &T) -> Frame {
        Frame::Json(serde_json::to_vec(value).expect("serializable value"))
    }

    /// Encodes the frame into a single buffer (length prefix included).
    pub fn encode(&self) -> Vec<u8> {
        let mut out;
        match self {
            Frame::Json(body) => {
                out = Vec::with_capacity(5 + body.len());
                push_header(&mut out, KIND_JSON, body.len());
                out.extend_from_slice(body);
            }
            Frame::Output { handle, seq, data } => {
                out = Vec::with_capacity(17 + data.len());
                push_header(&mut out, KIND_OUTPUT, 12 + data.len());
                out.extend_from_slice(&handle.to_le_bytes());
                out.extend_from_slice(&seq.to_le_bytes());
                out.extend_from_slice(data);
            }
            Frame::Input { handle, data } => {
                out = Vec::with_capacity(9 + data.len());
                push_header(&mut out, KIND_INPUT, 4 + data.len());
                out.extend_from_slice(&handle.to_le_bytes());
                out.extend_from_slice(data);
            }
        }
        out
    }

    fn decode(kind: u8, mut body: Vec<u8>) -> io::Result<Frame> {
        match kind {
            KIND_JSON => Ok(Frame::Json(body)),
            KIND_OUTPUT => {
                if body.len() < 12 {
                    return Err(invalid("short output frame"));
                }
                let handle = u32::from_le_bytes(body[0..4].try_into().unwrap());
                let seq = u64::from_le_bytes(body[4..12].try_into().unwrap());
                body.drain(..12);
                Ok(Frame::Output { handle, seq, data: body })
            }
            KIND_INPUT => {
                if body.len() < 4 {
                    return Err(invalid("short input frame"));
                }
                let handle = u32::from_le_bytes(body[0..4].try_into().unwrap());
                body.drain(..4);
                Ok(Frame::Input { handle, data: body })
            }
            other => Err(invalid(&format!("unknown frame kind {other}"))),
        }
    }
}

fn push_header(out: &mut Vec<u8>, kind: u8, payload_len: usize) {
    out.extend_from_slice(&((payload_len + 1) as u32).to_le_bytes());
    out.push(kind);
}

fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

/// Reads one frame. Returns `Ok(None)` on clean EOF at a frame boundary.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Frame>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len_buf) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(invalid(&format!("bad frame length {len}")));
    }
    let mut kind = [0u8; 1];
    reader.read_exact(&mut kind).await?;
    let mut body = vec![0u8; len - 1];
    reader.read_exact(&mut body).await?;
    Frame::decode(kind[0], body).map(Some)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, frame: &Frame) -> io::Result<()> {
    writer.write_all(&frame.encode()).await
}

/// Per-user endpoint. Windows: named pipe `\\.\pipe\kelpie-<user>-v1`.
/// Unix: socket file in the user's private data dir (not world-shared `/tmp`).
pub fn endpoint() -> Endpoint {
    if let Ok(custom) = std::env::var("KELPIE_ENDPOINT") {
        return if cfg!(windows) { Endpoint::Namespaced(custom) } else { Endpoint::Path(custom.into()) };
    }
    if cfg!(windows) {
        Endpoint::Namespaced(format!("kelpie-{}-v{PROTOCOL_VERSION}", endpoint_user()))
    } else {
        Endpoint::Path(data_dir().join(format!("kelpied-v{PROTOCOL_VERSION}.sock")))
    }
}

/// Where a daemon of the pre-rename install listens: it outlives its app, so it may still be
/// running when Kelpie starts for the first time.
fn legacy_endpoint() -> Endpoint {
    if cfg!(windows) {
        Endpoint::Namespaced(format!("erka-{}-v{PROTOCOL_VERSION}", endpoint_user()))
    } else {
        Endpoint::Path(data_base().join(LEGACY_NAME).join(format!("erkad-v{PROTOCOL_VERSION}.sock")))
    }
}

fn endpoint_user() -> String {
    std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".into())
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

#[derive(Debug, Clone)]
pub enum Endpoint {
    Namespaced(String),
    Path(std::path::PathBuf),
}

impl Endpoint {
    pub fn display(&self) -> String {
        match self {
            Endpoint::Namespaced(n) => format!(r"\\.\pipe\{n}"),
            Endpoint::Path(p) => p.display().to_string(),
        }
    }
}

// ---------- who is calling ----------
//
// The endpoint only keeps other OS users out. A connection then names itself with `auth`:
// the user token (the app, `kelpied` outside a session, the user's scripts) gives full access;
// a session token (set in each session's environment) gives that session's programs the
// scripting surface and binds hook / run reports to that session.

/// Environment variable carrying a session's token, next to `KELPIE_SESSION_ID`.
pub const SESSION_TOKEN_ENV: &str = "KELPIE_SESSION_TOKEN";
/// The daemon a session belongs to ([`endpoint_key`]): its token means nothing to another one
/// (a dev daemon driven from a Kelpie terminal).
pub const SESSION_ENDPOINT_ENV: &str = "KELPIE_SESSION_ENDPOINT";

/// `endpoint` as a file-name-safe key.
pub fn endpoint_key(endpoint: &Endpoint) -> String {
    let raw = match endpoint {
        Endpoint::Namespaced(n) => n.clone(),
        Endpoint::Path(p) => p.display().to_string(),
    };
    raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// File holding the daemon's user token for `endpoint`. Keyed by the endpoint rather than the
/// data folder: a dev app and a dev daemon agree on the endpoint even when only one of them
/// sets `KELPIE_DATA_DIR`.
pub fn auth_token_path(endpoint: &Endpoint) -> std::path::PathBuf {
    data_base().join("Kelpie").join("auth").join(format!("{}.token", endpoint_key(endpoint)))
}

/// The user token the running daemon wrote for `endpoint`, if readable.
pub fn read_user_token(endpoint: &Endpoint) -> Option<String> {
    let t = std::fs::read_to_string(auth_token_path(endpoint)).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// Writes the user token readable by this OS user only.
pub fn write_user_token(endpoint: &Endpoint, token: &str) -> io::Result<()> {
    let path = auth_token_path(endpoint);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    use std::io::Write as _;
    opts.open(&path)?.write_all(token.as_bytes())
}

/// `auth` params for a command-line client. Inside a session of this daemon it speaks for that
/// session: with its token, or — when the token was stripped (some agents drop variables named
/// `*TOKEN*` from the programs they run) — as an unverified claim the daemon accepts for hook
/// reports only. Never falls back to the user token there. Elsewhere: the user token.
pub fn cli_auth_params(endpoint: &Endpoint) -> Option<serde_json::Value> {
    let id = std::env::var("KELPIE_SESSION_ID").ok().filter(|s| !s.is_empty());
    let this_daemon = std::env::var(SESSION_ENDPOINT_ENV).map_or(true, |e| e == endpoint_key(endpoint));
    if let (Some(id), true) = (id, this_daemon) {
        return Some(match std::env::var(SESSION_TOKEN_ENV).ok().filter(|t| !t.is_empty()) {
            Some(token) => serde_json::json!({"session_id": id, "token": token}),
            None => serde_json::json!({"session_id": id}),
        });
    }
    user_auth_params(endpoint)
}

/// `auth` params with the user token, wherever the caller runs.
pub fn user_auth_params(endpoint: &Endpoint) -> Option<serde_json::Value> {
    read_user_token(endpoint).map(|token| serde_json::json!({"token": token}))
}

/// The project's name before it was renamed to Kelpie: the data folder and credential-store
/// service of existing installs still carry it, so their data is carried over once.
const LEGACY_NAME: &str = "Erka";

/// OS credential store service for Kelpie's secrets. Dev and test instances use their own
/// endpoint name, so their secrets never mix with the real ones.
pub fn keyring_service() -> String {
    keyring_service_named("Kelpie")
}

fn keyring_service_named(name: &str) -> String {
    match std::env::var("KELPIE_ENDPOINT") {
        Ok(e) if !e.is_empty() => format!("{name} ({e})"),
        _ => name.into(),
    }
}

/// Reads a secret from the OS credential store. One saved before the rename (under the old
/// service name) is moved to the current service on first read.
pub fn secret_get(key: &str) -> Option<String> {
    secret_get_moving(&keyring_service(), &keyring_service_named(LEGACY_NAME), key)
}

fn secret_get_moving(service: &str, legacy: &str, key: &str) -> Option<String> {
    let read = |service: &str| keyring::Entry::new(service, key).ok()?.get_password().ok().filter(|s| !s.is_empty());
    if let Some(value) = read(service) {
        return Some(value);
    }
    let value = read(legacy)?;
    if keyring::Entry::new(service, key).and_then(|e| e.set_password(&value)).is_ok() {
        let _ = keyring::Entry::new(legacy, key).and_then(|e| e.delete_credential());
    }
    Some(value)
}

pub fn secret_set(key: &str, value: &str) -> keyring::Result<()> {
    keyring::Entry::new(&keyring_service(), key)?.set_password(value)
}

/// Deletes a secret, including a copy still stored under the pre-rename service name.
pub fn secret_delete(key: &str) -> keyring::Result<()> {
    for service in [keyring_service(), keyring_service_named(LEGACY_NAME)] {
        match keyring::Entry::new(&service, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Credential-store key of a saved SSH host's password.
pub fn ssh_password_key(host_id: &str) -> String {
    format!("ssh-password:{host_id}")
}

/// Credential-store key of a private key's passphrase. ssh names the key in its prompt with
/// mixed separators (`C:\Users\me/.ssh/id_ed25519`), so the path is normalised.
pub fn ssh_passphrase_key(key_path: &str) -> String {
    let p = key_path.trim().replace('\\', "/");
    format!("ssh-passphrase:{}", if cfg!(windows) { p.to_lowercase() } else { p })
}

/// Root for daemon state: `%LOCALAPPDATA%\Kelpie` / `~/Library/Application Support/Kelpie`.
pub fn data_dir() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var("KELPIE_DATA_DIR") {
        return dir.into();
    }
    data_base().join("Kelpie")
}

fn data_base() -> std::path::PathBuf {
    if cfg!(windows) {
        std::env::var("LOCALAPPDATA").map(Into::into).unwrap_or_else(|_| std::env::temp_dir())
    } else if cfg!(target_os = "macos") {
        std::env::var("HOME")
            .map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
            .unwrap_or_else(|_| std::env::temp_dir())
    } else {
        std::env::var("XDG_DATA_HOME")
            .map(Into::into)
            .or_else(|_| std::env::var("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))
            .unwrap_or_else(|_| std::env::temp_dir())
    }
}

// ---------- moving data from the pre-rename install ----------

/// Files whose presence means a data folder holds user state.
const STATE_FILES: [&str; 2] = ["app-state.json", "sessions.json"];
/// Written into the new data folder once everything was carried over.
const MIGRATED: &str = ".migrated-from-erka";
/// Present while a move is under way, or after one that was interrupted or partly failed: the
/// next start finishes it.
const MIGRATING: &str = ".migrating-from-erka";

/// Moves user data from the pre-rename data folder into the current one, once.
///
/// A daemon of the old install that is still running is stopped first, with its sessions (they
/// come back here as exited and resume from their conversation), so no agent ever runs twice.
/// Entries are moved, not copied: instant on the same volume, even for speech models. State
/// files go last and paths stored in them are pointed at the new folder. Skipped for dev and
/// test instances (`KELPIE_DATA_DIR`) and debug builds, so they never touch real data.
pub fn migrate_legacy_data_dir() -> io::Result<bool> {
    if std::env::var_os("KELPIE_DATA_DIR").is_some() || cfg!(debug_assertions) {
        return Ok(false);
    }
    migrate_data(&data_base().join(LEGACY_NAME), &data_dir(), &stop_legacy_daemon)
}

fn migrate_data(legacy: &std::path::Path, dir: &std::path::Path, stop_old_daemon: &dyn Fn()) -> io::Result<bool> {
    let has_state = |d: &std::path::Path| STATE_FILES.iter().any(|f| d.join(f).is_file());
    let resuming = dir.join(MIGRATING).exists();
    if dir.join(MIGRATED).exists() || (!resuming && (!has_state(legacy) || has_state(dir))) {
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    // The app and its daemon both start here; the second one waits for the first to finish.
    let Some(_lock) = MigrationLock::acquire(&dir.join("migrate.lock"))? else {
        return Ok(false);
    };
    std::fs::write(dir.join(MIGRATING), b"")?;
    stop_old_daemon();
    let mut failed = Vec::new();
    move_tree(legacy, dir, true, &mut failed);
    rename_account_markers(&dir.join("accounts"), &mut failed);
    rewrite_state(dir, legacy, &mut failed);
    if let Some(e) = failed.into_iter().next() {
        return Err(e);
    }
    std::fs::write(dir.join(MIGRATED), b"")?;
    let _ = std::fs::remove_file(dir.join(MIGRATING));
    Ok(true)
}

/// Held while moving. Another process that finds it waits until it is gone (moves are quick)
/// and then leaves the work to its owner; a lock left by a killed process expires.
struct MigrationLock(std::path::PathBuf);

impl MigrationLock {
    fn acquire(path: &std::path::Path) -> io::Result<Option<MigrationLock>> {
        const STALE: std::time::Duration = std::time::Duration::from_secs(120);
        for _ in 0..2 {
            match std::fs::OpenOptions::new().write(true).create_new(true).open(path) {
                Ok(_) => return Ok(Some(MigrationLock(path.to_path_buf()))),
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e),
                Err(_) => {}
            }
            let age = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
            if age.is_some_and(|a| a > STALE) {
                let _ = std::fs::remove_file(path);
                continue;
            }
            for _ in 0..300 {
                std::thread::sleep(std::time::Duration::from_millis(100));
                if !path.exists() {
                    return Ok(None);
                }
            }
            return Ok(None);
        }
        Ok(None)
    }
}

impl Drop for MigrationLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What the old install keeps next to its data: program files, the uninstaller, runtime locks
/// and pids, and logs. The generated Claude hook settings are rewritten on every launch.
fn left_behind(name: &str) -> bool {
    const NAMES: [&str; 2] = ["logs", "claude-hooks.json"];
    const EXTS: [&str; 5] = [".exe", ".dll", ".lock", ".pid", ".sock"];
    let n = name.to_lowercase();
    NAMES.contains(&n.as_str()) || EXTS.iter().any(|e| n.ends_with(e))
}

/// Moves `from`'s entries into `to`, merging into folders that already exist there and never
/// overwriting a file. When a rename fails (another program holds the file), it copies instead.
/// Errors are collected per entry so one stubborn file never strands the rest.
fn move_tree(from: &std::path::Path, to: &std::path::Path, top: bool, failed: &mut Vec<io::Error>) {
    let mut entries: Vec<_> = match std::fs::read_dir(from) {
        Ok(rd) => rd.filter_map(Result::ok).filter(|e| !(top && left_behind(&e.file_name().to_string_lossy()))).collect(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => return failed.push(e),
    };
    // State files last: once the new folder holds them, everything else is there too.
    entries.sort_by_key(|e| STATE_FILES.contains(&e.file_name().to_string_lossy().as_ref()));
    if let Err(e) = std::fs::create_dir_all(to) {
        return failed.push(e);
    }
    for entry in entries {
        let (src, dest) = (entry.path(), to.join(entry.file_name()));
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if dest.exists() {
            if is_dir && dest.is_dir() {
                move_tree(&src, &dest, false, failed);
            }
            continue;
        }
        if std::fs::rename(&src, &dest).is_ok() {
            continue;
        }
        if is_dir {
            move_tree(&src, &dest, false, failed);
        } else if let Err(e) = copy_file(&src, &dest) {
            failed.push(io::Error::new(e.kind(), format!("{}: {e}", src.display())));
        }
    }
}

/// Copies through a temporary name, so an interrupted copy never looks like a finished file.
fn copy_file(src: &std::path::Path, dest: &std::path::Path) -> io::Result<()> {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    std::fs::copy(src, &part)?;
    std::fs::rename(&part, dest)
}

/// Account homes created by the old install carry its marker file; Kelpie only removes folders
/// that carry its own.
fn rename_account_markers(accounts: &std::path::Path, failed: &mut Vec<io::Error>) {
    let homes = std::fs::read_dir(accounts).into_iter().flatten().flatten().flat_map(|agent| std::fs::read_dir(agent.path()).into_iter().flatten().flatten());
    for home in homes {
        let (old, new) = (home.path().join(".erka-account"), home.path().join(".kelpie-account"));
        if old.is_file() && !new.exists() {
            if let Err(e) = std::fs::rename(&old, &new) {
                failed.push(e);
            }
        }
    }
}

/// The moved state stores absolute paths into the old folder (account homes, the ssh askpass
/// helper and config) and environment names of the old install: point them at the new ones.
fn rewrite_state(dir: &std::path::Path, legacy: &std::path::Path, failed: &mut Vec<io::Error>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let new = rewrite_legacy_refs(&text, legacy, dir);
        if new != text {
            let mut part = path.as_os_str().to_owned();
            part.push(".part");
            if let Err(e) = std::fs::write(&part, new).and_then(|_| std::fs::rename(&part, &path)) {
                failed.push(e);
            }
        }
    }
}

fn rewrite_legacy_refs(text: &str, legacy: &std::path::Path, dir: &std::path::Path) -> String {
    let (old, new) = (legacy.to_string_lossy(), dir.to_string_lossy());
    let mut out = text.to_string();
    // The folder as JSON stores it: escaped backslashes, or forward slashes. The binaries of the
    // old install lived in it on Windows; the new install puts its own in the new folder.
    // Binaries first, for every form: off Windows both forms are the same string, so renaming the
    // folder inside the loop would hide the binary paths from the next form.
    let forms = [(old.replace('\\', "\\\\"), new.replace('\\', "\\\\"), "\\\\"), (old.replace('\\', "/"), new.replace('\\', "/"), "/")];
    for (old_dir, new_dir, sep) in &forms {
        for (bin_old, bin_new) in [("erkad.exe", "kelpied.exe"), ("erka-stt.exe", "kelpie-stt.exe"), ("erka.exe", "kelpie.exe")] {
            out = replace_ignore_case(&out, &format!("{old_dir}{sep}{bin_old}"), &format!("{new_dir}{sep}{bin_new}"));
        }
    }
    for (old_dir, new_dir, _) in &forms {
        out = replace_ignore_case(&out, old_dir, new_dir);
    }
    // macOS keeps the binaries in the app bundle.
    for (o, n) in [("Erka.app/Contents/MacOS/erkad", "Kelpie.app/Contents/MacOS/kelpied"), ("Erka.app/Contents/MacOS/erka-stt", "Kelpie.app/Contents/MacOS/kelpie-stt")] {
        out = out.replace(o, n);
    }
    // Environment of saved sessions (`ERKA_ASKPASS`, `ERKA_SESSION_ID`, …).
    out.replace("\"ERKA_", "\"KELPIE_")
}

/// ASCII case-insensitive replace (Windows paths compare that way). Lowercasing ASCII keeps
/// byte offsets, so indices found in the lowered text are valid in the original.
fn replace_ignore_case(text: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return text.to_string();
    }
    let (hay, needle) = (text.to_ascii_lowercase(), from.to_ascii_lowercase());
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in hay.match_indices(&needle) {
        out.push_str(&text[last..i]);
        out.push_str(to);
        last = i + from.len();
    }
    out.push_str(&text[last..]);
    out
}

/// Asks a daemon of the pre-rename install to stop, ending its sessions, and waits until it is
/// gone and has saved its state. Nothing happens when none is running.
fn stop_legacy_daemon() {
    use std::time::Duration;
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else { return };
    let endpoint = legacy_endpoint();
    let running = rt.block_on(async {
        let Ok(Ok(mut stream)) = tokio::time::timeout(Duration::from_secs(2), transport::connect(&endpoint)).await else {
            return false;
        };
        let stop = Frame::json(&serde_json::json!({"id": 1, "method": "daemon.shutdown", "params": {"kill_sessions": true}}));
        if write_frame(&mut stream, &stop).await.is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream)).await;
        }
        true
    });
    if !running {
        return;
    }
    // Windows: the daemon holds its lock file without sharing until it exits, after its final
    // save. Elsewhere (flock is not available here) a fixed grace period stands in.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        let lock = data_base().join(LEGACY_NAME).join("erkad.lock");
        for _ in 0..100 {
            if !lock.exists() || std::fs::OpenOptions::new().read(true).share_mode(0).open(&lock).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(not(windows))]
    std::thread::sleep(Duration::from_secs(3));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uses the real OS credential store, so it only runs on request:
    /// `cargo test -p kelpie-proto -- --ignored`.
    #[test]
    #[ignore]
    fn legacy_secret_moves_to_the_current_service() {
        let (service, legacy) = (format!("kelpie-test-{}", std::process::id()), format!("erka-test-{}", std::process::id()));
        let entry = |s: &str| keyring::Entry::new(s, "ssh-password:h1").unwrap();
        entry(&legacy).set_password("s3cret").unwrap();

        assert_eq!(secret_get_moving(&service, &legacy, "ssh-password:h1").as_deref(), Some("s3cret"));
        assert_eq!(entry(&service).get_password().unwrap(), "s3cret");
        assert!(matches!(entry(&legacy).get_password(), Err(keyring::Error::NoEntry)));
        entry(&service).delete_credential().unwrap();
    }

    fn migration_root(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("kelpie-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        (root.join("Erka"), root.join("Kelpie"), root)
    }

    fn write_all(dir: &std::path::Path, files: &[(&str, &str)]) {
        for (path, body) in files {
            let path = dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    #[test]
    fn legacy_data_is_moved_once_and_program_files_stay() {
        let (legacy, dir, root) = migration_root("once");
        write_all(&legacy, &[
            ("app-state.json", "{}"),
            ("sessions.json", "[]"),
            ("snapshots/1.bin", "screen"),
            ("speech-models/base.bin", "model"),
            ("logs/erkad.log", "log"),
            ("erka.exe", "bin"),
            ("conpty.dll", "bin"),
            ("erkad.lock", ""),
            ("claude-hooks.json", "{}"),
        ]);
        // The new installer already created the folder with its own program files.
        write_all(&dir, &[("kelpie.exe", "new")]);
        let stops = std::cell::Cell::new(0);

        assert!(migrate_data(&legacy, &dir, &|| stops.set(stops.get() + 1)).unwrap());
        assert_eq!(stops.get(), 1, "the old daemon is stopped before moving");
        assert_eq!(std::fs::read_to_string(dir.join("snapshots/1.bin")).unwrap(), "screen");
        assert_eq!(std::fs::read_to_string(dir.join("speech-models/base.bin")).unwrap(), "model");
        assert!(dir.join("app-state.json").is_file() && dir.join("sessions.json").is_file());
        assert!(!legacy.join("app-state.json").exists() && !legacy.join("snapshots").exists(), "moved, not copied");
        for left in ["logs", "erka.exe", "conpty.dll", "erkad.lock", "claude-hooks.json"] {
            assert!(!dir.join(left).exists() && legacy.join(left).exists(), "{left} stays with the old install");
        }
        assert_eq!(std::fs::read_to_string(dir.join("kelpie.exe")).unwrap(), "new");
        assert!(dir.join(MIGRATED).exists() && !dir.join(MIGRATING).exists() && !dir.join("migrate.lock").exists());

        // Done once: a later start leaves both folders alone, even if the old app wrote again.
        write_all(&legacy, &[("app-state.json", "{\"changed\":true}")]);
        assert!(!migrate_data(&legacy, &dir, &|| stops.set(stops.get() + 1)).unwrap());
        assert_eq!(stops.get(), 1);
        assert_eq!(std::fs::read_to_string(dir.join("app-state.json")).unwrap(), "{}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn moved_state_points_at_the_new_folder_and_names() {
        let (legacy, dir, root) = migration_root("paths");
        let (old, new) = (legacy.to_string_lossy().to_string(), dir.to_string_lossy().to_string());
        let sep = std::path::MAIN_SEPARATOR;
        let state = serde_json::json!({ "settings": { "accounts": [{ "dir": format!("{old}{sep}accounts{sep}claude{sep}work") }] } });
        let sessions = serde_json::json!([{ "env": {
            "SSH_ASKPASS": format!("{old}{sep}erkad.exe"),
            "ERKA_ASKPASS": "host-1",
            "CLAUDE_CONFIG_DIR": format!("{old}{sep}accounts{sep}claude{sep}work"),
        }, "remote": { "config": format!("{}/ssh/config", old.replace('\\', "/")) } }]);
        write_all(&legacy, &[
            ("app-state.json", &state.to_string()),
            ("sessions.json", &sessions.to_string()),
            ("accounts/claude/work/.erka-account", ""),
        ]);

        assert!(migrate_data(&legacy, &dir, &|| {}).unwrap());
        let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("app-state.json")).unwrap()).unwrap();
        assert_eq!(state["settings"]["accounts"][0]["dir"], format!("{new}{sep}accounts{sep}claude{sep}work"));
        let sessions: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("sessions.json")).unwrap()).unwrap();
        let env = &sessions[0]["env"];
        assert_eq!(env["SSH_ASKPASS"], format!("{new}{sep}kelpied.exe"));
        assert_eq!(env["KELPIE_ASKPASS"], "host-1");
        assert!(env.get("ERKA_ASKPASS").is_none());
        assert_eq!(sessions[0]["remote"]["config"], format!("{}/ssh/config", new.replace('\\', "/")));
        assert!(dir.join("accounts/claude/work/.kelpie-account").is_file());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_interrupted_move_is_finished_on_the_next_start() {
        let (legacy, dir, root) = migration_root("resume");
        // A first run moved the state files and stopped before the rest (crash, killed app).
        write_all(&legacy, &[("ssh/hosts.json", "[]"), ("plugins/p/kelpie-plugin.json", "{}")]);
        write_all(&dir, &[("app-state.json", "{}"), ("sessions.json", "[]"), (MIGRATING, "")]);

        assert!(migrate_data(&legacy, &dir, &|| {}).unwrap());
        assert!(dir.join("ssh/hosts.json").is_file() && dir.join("plugins/p/kelpie-plugin.json").is_file());
        assert!(dir.join(MIGRATED).exists() && !dir.join(MIGRATING).exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_fresh_kelpie_install_is_left_alone() {
        let (legacy, dir, root) = migration_root("fresh");
        write_all(&dir, &[("app-state.json", "{\"mine\":1}")]);
        assert!(!migrate_data(&legacy, &dir, &|| panic!("nothing to stop")).unwrap());
        // Legacy data next to Kelpie state that was never migrated is not merged into it.
        write_all(&legacy, &[("app-state.json", "{}")]);
        assert!(!migrate_data(&legacy, &dir, &|| panic!("nothing to stop")).unwrap());
        assert_eq!(std::fs::read_to_string(dir.join("app-state.json")).unwrap(), "{\"mine\":1}");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn slash_paths_get_the_new_folder_and_binary_names() {
        let (legacy, dir) = (std::path::Path::new("/h/Erka"), std::path::Path::new("/h/Kelpie"));
        let out = rewrite_legacy_refs(r#"{"askpass":"/h/Erka/erkad.exe","home":"/h/Erka/accounts/a"}"#, legacy, dir);
        assert_eq!(out, r#"{"askpass":"/h/Kelpie/kelpied.exe","home":"/h/Kelpie/accounts/a"}"#);
    }

    #[test]
    fn replace_ignore_case_keeps_other_text() {
        assert_eq!(replace_ignore_case("A c:\\X\\erka b C:\\x\\ERKA", "c:\\x\\erka", "D:\\K"), "A D:\\K b D:\\K");
        assert_eq!(replace_ignore_case("héllo Erka", "erka", "Kelpie"), "héllo Kelpie");
    }

    #[tokio::test]
    async fn roundtrip_all_kinds() {
        let frames = vec![
            Frame::Json(br#"{"id":1}"#.to_vec()),
            Frame::Output { handle: 7, seq: 1 << 40, data: b"\x1b[31mhi".to_vec() },
            Frame::Input { handle: 3, data: vec![] },
        ];
        let mut buf = Vec::new();
        for f in &frames {
            write_frame(&mut buf, f).await.unwrap();
        }
        let mut cursor = std::io::Cursor::new(buf);
        for f in &frames {
            assert_eq!(read_frame(&mut cursor).await.unwrap().as_ref(), Some(f));
        }
        assert!(read_frame(&mut cursor).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn rejects_oversized_length() {
        let mut bytes = ((MAX_FRAME_LEN + 2) as u32).to_le_bytes().to_vec();
        bytes.push(KIND_JSON);
        let mut cursor = std::io::Cursor::new(bytes);
        assert!(read_frame(&mut cursor).await.is_err());
    }
}

pub mod transport {
    //! Local-socket helpers shared by the daemon (bind) and clients (connect).
    use std::io;

    use interprocess::local_socket::tokio::{prelude::*, Listener, Stream};
    use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name};

    use super::Endpoint;

    fn name(endpoint: &Endpoint) -> io::Result<Name<'static>> {
        match endpoint {
            Endpoint::Namespaced(n) => n.clone().to_ns_name::<GenericNamespaced>(),
            Endpoint::Path(p) => p.clone().to_fs_name::<GenericFilePath>(),
        }
    }

    pub async fn connect(endpoint: &Endpoint) -> io::Result<Stream> {
        Stream::connect(name(endpoint)?).await
    }

    /// Binds the daemon endpoint. Callers must hold the single-instance lock first, which is
    /// what makes overwriting a stale Unix socket file safe.
    pub fn bind(endpoint: &Endpoint) -> io::Result<Listener> {
        if let Endpoint::Path(p) = endpoint {
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let _ = std::fs::remove_file(p);
        }
        let opts = ListenerOptions::new().name(name(endpoint)?);
        #[cfg(windows)]
        let opts = {
            use interprocess::os::windows::local_socket::ListenerOptionsExt;
            use interprocess::os::windows::security_descriptor::SecurityDescriptor;
            // Protected DACL: only SYSTEM and the pipe owner (the current user). The default
            // named-pipe DACL would let every local account open it for reading.
            let sddl = widestring::U16CString::from_str("D:P(A;;GA;;;SY)(A;;GA;;;OW)")
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
            opts.security_descriptor(SecurityDescriptor::deserialize(&sddl)?)
        };
        let listener = opts.create_tokio()?;
        #[cfg(unix)]
        if let Endpoint::Path(p) = endpoint {
            use std::os::unix::fs::PermissionsExt;
            // Owner-only, like the Windows DACL above.
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(listener)
    }
}
