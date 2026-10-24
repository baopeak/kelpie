//! File system commands for the explorer and viewer. Every path is checked against the
//! project root the UI passes in, so a crafted path can never escape the workspace.

use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use base64::Engine as _;
use serde::Serialize;

pub type CmdResult<T> = Result<T, String>;

pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Lexically normalises `path` and requires it to stay inside `root`.
pub fn guard(root: &str, path: &str) -> CmdResult<PathBuf> {
    if crate::remote::is_remote(root) {
        return crate::remote::guard(root, path);
    }
    let root = normalize(Path::new(root));
    let full = normalize(&root.join(path));
    if !full.starts_with(&root) {
        return Err(format!("path {path} is outside the project"));
    }
    Ok(full)
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[derive(Serialize)]
pub struct DirEntry {
    name: String,
    path: String,
    is_dir: bool,
    is_symlink: bool,
    size: u64,
    /// Matched by .gitignore / .ignore (shown dimmed, still listed).
    ignored: bool,
}

/// "file9" < "file10": digit runs compare numerically, the rest case-insensitively.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let mut na = String::new();
                while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                    na.push(c);
                    ai.next();
                }
                let mut nb = String::new();
                while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                    nb.push(c);
                    bi.next();
                }
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != Ordering::Equal {
                    return ord;
                }
                ai.next();
                bi.next();
            }
        }
    }
}

/// Runs file-system work on the blocking pool. Synchronous Tauri commands run on the main
/// thread, where a slow disk, a huge file or a network path would freeze the whole window.
pub async fn blocking<T: Send + 'static>(f: impl FnOnce() -> CmdResult<T> + Send + 'static) -> CmdResult<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?
}

/// Whether `path` is an existing folder (no listing, so a large folder costs nothing).
#[tauri::command]
pub async fn fs_is_dir(path: String) -> CmdResult<bool> {
    if crate::remote::is_remote(&path) {
        return Ok(crate::remote::parse_root(&path).is_some());
    }
    crate::fs_ops::blocking(move || Ok(fs_is_dir_blocking(path))).await
}

fn fs_is_dir_blocking(path: String) -> bool {
    std::path::Path::new(&path).is_dir()
}

/// Canonical form of a folder (long names instead of 8.3 short ones, no `\\?\` prefix), for
/// comparing paths that may be spelled differently; the input when it does not exist.
#[tauri::command]
pub async fn fs_canonical(path: String) -> CmdResult<String> {
    if crate::remote::is_remote(&path) {
        return Ok(path);
    }
    crate::fs_ops::blocking(move || Ok(fs_canonical_blocking(path))).await
}

fn fs_canonical_blocking(path: String) -> String {
    match std::fs::canonicalize(&path) {
        Ok(p) => {
            let s = p.to_string_lossy().into_owned();
            s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
        }
        Err(_) => path,
    }
}

/// Async + blocking pool: listing a huge folder must never stall the UI thread.
#[tauri::command]
pub async fn fs_list_dir(root: String, path: String) -> CmdResult<Vec<DirEntry>> {
    if crate::remote::is_remote(&root) {
        let base = path.trim_matches(['/', '\\']).to_string();
        return Ok(crate::remote::list_dir(&root, &path)
            .await?
            .into_iter()
            .map(|(name, is_dir, is_symlink, size)| DirEntry { path: if base.is_empty() { name.clone() } else { format!("{base}/{name}") }, name, is_dir, is_symlink, size, ignored: false })
            .collect());
    }
    tauri::async_runtime::spawn_blocking(move || list_dir(&root, &path)).await.map_err(err)?
}

fn list_dir(root: &str, path: &str) -> CmdResult<Vec<DirEntry>> {
    let (root, path) = (root.to_string(), path.to_string());
    let dir = guard(&root, &path)?;
    let mut entries: Vec<DirEntry> = std::fs::read_dir(&dir)
        .map_err(err)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" {
                return None;
            }
            let file_type = e.file_type().ok()?;
            let is_symlink = file_type.is_symlink();
            let meta = if is_symlink { std::fs::metadata(e.path()).ok() } else { e.metadata().ok() };
            let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
            let rel = Path::new(&path).join(&name).to_string_lossy().replace('\\', "/");
            Some(DirEntry { name, path: rel.trim_start_matches("./").to_string(), is_dir, is_symlink, size: meta.map(|m| m.len()).unwrap_or(0), ignored: false })
        })
        .collect();
    // One-level, ignore-aware walk: whatever it does not yield is ignored.
    let visible: std::collections::HashSet<std::ffi::OsString> = ignore::WalkBuilder::new(&dir)
        .max_depth(Some(1))
        .hidden(false)
        .parents(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.depth() == 1)
        .map(|e| e.file_name().to_os_string())
        .collect();
    for e in &mut entries {
        e.ignored = !visible.contains(std::ffi::OsStr::new(&e.name));
    }
    entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
    Ok(entries)
}

#[derive(Serialize)]
pub struct FileContent {
    /// UTF-8 text, or `None` for binary files.
    text: Option<String>,
    /// Base64 payload for images (when requested and small enough).
    base64: Option<String>,
    size: u64,
    mtime: u64,
    truncated: bool,
    binary: bool,
}

fn mtime_of(meta: &std::fs::Metadata) -> u64 {
    meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Reads up to `max_bytes`. `as_base64` is for image viewers.
#[tauri::command]
pub async fn fs_read_file(root: String, path: String, max_bytes: Option<u64>, as_base64: Option<bool>) -> CmdResult<FileContent> {
    if crate::remote::is_remote(&root) {
        let limit = max_bytes.unwrap_or(8 << 20);
        let (buf, size, mtime_s) = crate::remote::read_file(&root, &path, limit).await?;
        return Ok(file_content(buf, size, mtime_s * 1000, size > limit, as_base64.unwrap_or(false)));
    }
    crate::fs_ops::blocking(move || fs_read_file_blocking(root, path, max_bytes, as_base64)).await
}

fn fs_read_file_blocking(root: String, path: String, max_bytes: Option<u64>, as_base64: Option<bool>) -> CmdResult<FileContent> {
    use std::io::Read;
    let full = guard(&root, &path)?;
    let meta = std::fs::metadata(&full).map_err(err)?;
    let limit = max_bytes.unwrap_or(8 << 20);
    let mut buf = Vec::with_capacity(meta.len().min(limit) as usize);
    std::fs::File::open(&full).map_err(err)?.take(limit).read_to_end(&mut buf).map_err(err)?;
    Ok(file_content(buf, meta.len(), mtime_of(&meta), meta.len() > limit, as_base64.unwrap_or(false)))
}

/// A file's bytes as the viewers want them: base64 (images), or text unless binary.
fn file_content(buf: Vec<u8>, size: u64, mtime: u64, truncated: bool, as_base64: bool) -> FileContent {
    if as_base64 {
        return FileContent { text: None, base64: Some(base64::engine::general_purpose::STANDARD.encode(&buf)), size, mtime, truncated, binary: true };
    }
    let sniff = &buf[..buf.len().min(8192)];
    let binary = sniff.contains(&0);
    let text = if binary {
        None
    } else {
        Some(match String::from_utf8(buf) {
            Ok(s) => s,
            Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
        })
    };
    FileContent { text, base64: None, size, mtime, truncated, binary }
}

#[derive(Serialize)]
pub struct WriteResult {
    ok: bool,
    /// Set when the file changed on disk since `expected_mtime` (nothing was written).
    conflict: bool,
    mtime: u64,
}

#[tauri::command]
pub async fn fs_write_file(root: String, path: String, content: String, expected_mtime: Option<u64>, force: Option<bool>) -> CmdResult<WriteResult> {
    if crate::remote::is_remote(&root) {
        // Remote mtimes are whole seconds (the viewer keeps milliseconds).
        let expected = if force.unwrap_or(false) { None } else { expected_mtime.map(|m| m / 1000) };
        let (ok, mtime_s) = crate::remote::write_file(&root, &path, content.into_bytes(), expected).await?;
        return Ok(WriteResult { ok, conflict: !ok, mtime: mtime_s * 1000 });
    }
    crate::fs_ops::blocking(move || fs_write_file_blocking(root, path, content, expected_mtime, force)).await
}

fn fs_write_file_blocking(root: String, path: String, content: String, expected_mtime: Option<u64>, force: Option<bool>) -> CmdResult<WriteResult> {
    let full = guard(&root, &path)?;
    if let (Some(expected), false) = (expected_mtime, force.unwrap_or(false)) {
        if let Ok(meta) = std::fs::metadata(&full) {
            let current = mtime_of(&meta);
            if current != expected {
                return Ok(WriteResult { ok: false, conflict: true, mtime: current });
            }
        }
    }
    // Write through symlinks to their target, and keep the file's permissions (exec bit…).
    let full = std::fs::canonicalize(&full).unwrap_or(full);
    let tmp = full.with_extension(format!("{}.kelpie-tmp", full.extension().and_then(|e| e.to_str()).unwrap_or("")));
    std::fs::write(&tmp, content.as_bytes()).map_err(err)?;
    if let Ok(meta) = std::fs::metadata(&full) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &full).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(e)
    })?;
    let mtime = std::fs::metadata(&full).map(|m| mtime_of(&m)).unwrap_or(0);
    Ok(WriteResult { ok: true, conflict: false, mtime })
}

#[tauri::command]
pub async fn fs_create(root: String, path: String, dir: bool) -> CmdResult<()> {
    if crate::remote::is_remote(&root) {
        return crate::remote::create(&root, &path, dir).await;
    }
    crate::fs_ops::blocking(move || fs_create_blocking(root, path, dir)).await
}

fn fs_create_blocking(root: String, path: String, dir: bool) -> CmdResult<()> {
    let full = guard(&root, &path)?;
    if full.exists() {
        return Err(format!("{path} already exists"));
    }
    if dir {
        std::fs::create_dir_all(&full).map_err(err)
    } else {
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).map_err(err)?;
        }
        std::fs::write(&full, b"").map_err(err)
    }
}

#[tauri::command]
pub async fn fs_rename(root: String, from: String, to: String) -> CmdResult<()> {
    if crate::remote::is_remote(&root) {
        return crate::remote::rename(&root, &from, &to).await;
    }
    crate::fs_ops::blocking(move || fs_rename_blocking(root, from, to)).await
}

fn fs_rename_blocking(root: String, from: String, to: String) -> CmdResult<()> {
    let (a, b) = (guard(&root, &from)?, guard(&root, &to)?);
    if b.exists() {
        return Err(format!("{to} already exists"));
    }
    std::fs::rename(a, b).map_err(err)
}

/// Moves to the OS trash (recoverable) rather than deleting.
#[tauri::command]
pub async fn fs_delete(root: String, path: String) -> CmdResult<()> {
    if crate::remote::is_remote(&root) {
        return crate::remote::delete(&root, &path).await;
    }
    crate::fs_ops::blocking(move || fs_delete_blocking(root, path)).await
}

fn fs_delete_blocking(root: String, path: String) -> CmdResult<()> {
    let full = guard(&root, &path)?;
    if full == normalize(Path::new(&root)) {
        return Err("refusing to delete the project root".into());
    }
    trash::delete(&full).map_err(err)
}

/// Shows the item in Explorer / Finder.
#[tauri::command]
pub async fn fs_reveal(root: String, path: String) -> CmdResult<()> {
    if crate::remote::is_remote(&root) {
        return Err("files of a remote project are on the remote host".into());
    }
    crate::fs_ops::blocking(move || fs_reveal_blocking(root, path)).await
}

fn fs_reveal_blocking(root: String, path: String) -> CmdResult<()> {
    let full = guard(&root, &path)?;
    #[cfg(windows)]
    let result = std::process::Command::new("explorer").arg(format!("/select,{}", full.display())).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg("-R").arg(&full).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(full.parent().unwrap_or(&full)).spawn();
    result.map(|_| ()).map_err(err)
}

const TEMP_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "txt", "md", "diff", "log"];
const TEMP_MAX_BYTES: usize = 25 << 20;
const TEMP_KEEP: std::time::Duration = std::time::Duration::from_secs(3 * 24 * 3600);

/// Writes pasted bytes (an image from the clipboard) or a snippet too long to type into a file
/// in the system temp folder and returns its path, for an agent to read by path. Only known
/// extensions are written, and files older than a few days are removed on the way.
fn write_temp(dir: &Path, label: &str, ext: &str, data: &[u8]) -> CmdResult<PathBuf> {
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    if !TEMP_EXTENSIONS.contains(&ext.as_str()) {
        return Err(format!("unsupported extension {ext}"));
    }
    if data.len() > TEMP_MAX_BYTES {
        return Err(format!("{} MiB is too large to attach (limit {} MiB)", data.len() >> 20, TEMP_MAX_BYTES >> 20));
    }
    std::fs::create_dir_all(dir).map_err(err)?;
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let label: String = label.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').take(24).collect();
    let stamp = std::time::SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let name = format!("{}-{stamp}-{}.{ext}", if label.is_empty() { "paste" } else { &label }, COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let path = dir.join(name);
    std::fs::write(&path, data).map_err(err)?;
    Ok(path)
}

#[derive(Serialize)]
pub struct ImageData {
    mime: &'static str,
    base64: String,
}

fn image_mime(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

/// A picture the user dropped on the composer (it can live anywhere), for a chat to attach.
/// Only picture types, and not past the temp-file limit.
#[tauri::command]
pub async fn fs_read_image(path: String) -> CmdResult<ImageData> {
    blocking(move || {
        use std::io::Read;
        // A network path would make Windows sign in to whatever host it names.
        if path.starts_with("\\\\") || path.starts_with("//") {
            return Err("pictures on a network path are not read".into());
        }
        let path = Path::new(&path);
        let mime = path.extension().and_then(|e| e.to_str()).and_then(image_mime).ok_or("not a picture file")?;
        if std::fs::symlink_metadata(path).map_err(err)?.file_type().is_symlink() {
            return Err("a link to a picture is not followed".into());
        }
        let mut data = Vec::new();
        std::fs::File::open(path).map_err(err)?.take(TEMP_MAX_BYTES as u64 + 1).read_to_end(&mut data).map_err(err)?;
        if data.len() > TEMP_MAX_BYTES {
            return Err(format!("larger than {} MiB: too large to attach", TEMP_MAX_BYTES >> 20));
        }
        Ok(ImageData { mime, base64: base64::engine::general_purpose::STANDARD.encode(data) })
    })
    .await
}

/// Removes what the composer left in the temp folder more than a few days ago (also at start-up:
/// it holds unmasked selections and diffs).
pub fn sweep_temp() {
    let dir = std::env::temp_dir().join("kelpie-composer");
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > TEMP_KEEP) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[tauri::command]
pub async fn fs_write_temp(label: String, ext: String, base64: String) -> CmdResult<String> {
    blocking(move || {
        let data = base64::engine::general_purpose::STANDARD.decode(base64.as_bytes()).map_err(err)?;
        sweep_temp();
        let path = write_temp(&std::env::temp_dir().join("kelpie-composer"), &label, &ext, &data)?;
        Ok(path.to_string_lossy().into_owned())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_files_keep_their_bytes_and_extension() {
        let dir = std::env::temp_dir().join(format!("kelpie-temp-test-{}", std::process::id()));
        let a = write_temp(&dir, "image", ".PNG", &[1, 2, 3]).unwrap();
        let b = write_temp(&dir, "image", "png", &[4]).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.extension().unwrap(), "png");
        assert_eq!(std::fs::read(&a).unwrap(), [1, 2, 3]);
        // A label cannot walk out of the folder or smuggle in an extension.
        let c = write_temp(&dir, "../../evil.exe", "txt", b"x").unwrap();
        assert_eq!(c.parent().unwrap(), dir);
        assert!(c.file_name().unwrap().to_string_lossy().starts_with("evilexe-"));
        assert!(write_temp(&dir, "x", "exe", b"x").is_err());
        assert!(write_temp(&dir, "x", "", b"x").is_err());
        assert!(write_temp(&dir, "x", "png", &vec![0; TEMP_MAX_BYTES + 1]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn natural_order() {
        let mut v = vec!["file10", "file9", "File2", "a", "file09b"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "File2", "file9", "file09b", "file10"]);
    }

    #[test]
    fn guard_blocks_escape() {
        let root = if cfg!(windows) { "C:\\proj" } else { "/proj" };
        assert!(guard(root, "src/main.rs").is_ok());
        assert!(guard(root, "../etc/passwd").is_err());
        assert!(guard(root, "src/../../x").is_err());
        assert!(guard(root, "").is_ok());
    }
}
