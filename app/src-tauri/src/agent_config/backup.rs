//! Copies of agent config files taken just before Kelpie changes them, kept in Kelpie's own data
//! folder (never next to the user's file or inside a repository). A copy can hold the secrets the
//! file held, so it is private to the user like the file itself. The newest few per file are kept.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const KEEP_PER_FILE: usize = 20;

type Res<T> = Result<T, String>;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BackupInfo {
    pub id: String,
    /// The file the copy belongs to.
    pub path: String,
    /// Seconds since the epoch.
    pub at: u64,
    /// false: the file did not exist then (restoring removes it).
    pub existed: bool,
    pub bytes: u64,
    /// The project folder the file was in, when it was a project file: a restore stays inside it.
    #[serde(default)]
    pub root: Option<String>,
}

fn path_hash(path: &Path) -> String {
    let digest = Sha256::digest(path.to_string_lossy().to_lowercase().as_bytes());
    digest[..4].iter().map(|b| format!("{b:02x}")).collect()
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 48 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn private(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Saves `old` (the file's content, none when it did not exist) and returns the copy's id.
pub fn save(dir: &Path, path: &Path, old: Option<&str>, root: Option<&Path>) -> Res<String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make the backup folder: {e}"))?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let hash = path_hash(path);
    let mut id = format!("{now:013}-{hash}");
    let mut n = 1;
    while dir.join(format!("{id}.json")).exists() {
        id = format!("{now:013}-{hash}-{n}");
        n += 1;
    }
    if let Some(text) = old {
        let bak = dir.join(format!("{id}.bak"));
        std::fs::write(&bak, text).map_err(|e| format!("could not write the backup: {e}"))?;
        private(&bak);
    }
    let info = BackupInfo { id: id.clone(), path: path.to_string_lossy().into_owned(), at: (now / 1000) as u64, existed: old.is_some(), bytes: old.map_or(0, |t| t.len() as u64), root: root.map(|r| r.to_string_lossy().into_owned()) };
    let meta = dir.join(format!("{id}.json"));
    std::fs::write(&meta, serde_json::to_vec(&info).map_err(|e| e.to_string())?).map_err(|e| format!("could not write the backup: {e}"))?;
    private(&meta);
    prune(dir, &hash);
    Ok(id)
}

/// Every copy, newest first.
pub fn list(dir: &Path) -> Vec<BackupInfo> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<BackupInfo> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok())
        .collect();
    out.sort_by(|a, b| b.id.cmp(&a.id));
    out
}

fn prune(dir: &Path, hash: &str) {
    for old in list(dir).into_iter().filter(|b| b.id.split('-').nth(1) == Some(hash)).skip(KEEP_PER_FILE) {
        let _ = std::fs::remove_file(dir.join(format!("{}.bak", old.id)));
        let _ = std::fs::remove_file(dir.join(format!("{}.json", old.id)));
    }
}

/// A copy's file and content (none: the file did not exist).
pub fn load(dir: &Path, id: &str) -> Res<(PathBuf, Option<String>, Option<PathBuf>)> {
    if !valid_id(id) {
        return Err("invalid backup id".into());
    }
    let info: BackupInfo = serde_json::from_slice(&std::fs::read(dir.join(format!("{id}.json"))).map_err(|_| "that backup no longer exists".to_string())?).map_err(|_| "the backup's record is unreadable".to_string())?;
    let path = PathBuf::from(&info.path);
    if !path.is_absolute() {
        return Err("the backup's record is unreadable".into());
    }
    let text = if info.existed { Some(std::fs::read_to_string(dir.join(format!("{id}.bak"))).map_err(|_| "the backup's copy is missing".to_string())?) } else { None };
    Ok((path, text, info.root.map(PathBuf::from)))
}
