//! Crash-safe files: writes that survive a sudden power cut, and loads that recover from one.
//!
//! A write goes to a new temporary file beside the target, is flushed to the disk (`sync_all`) and
//! only then replaces the target in one step. Without the flush a file system may keep the rename
//! but not the data: after a power cut the file has its full size and every byte is zero.
//!
//! [`write_with_prev`] also keeps the last good version as `<name>.prev`, and [`load`] falls back to
//! it when the file is missing, empty, all zeros or does not parse. At every instant at least one
//! of the two holds a complete file: the previous version is replaced (atomically) only by a copy
//! of a target that was itself valid, and the target only after that.

use std::fmt::Display;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Copies of damaged files kept per target; older ones are removed.
const DAMAGED_KEPT: usize = 3;
/// A temporary file this old was left by a write that never finished (a crash), not by one under way.
const STALE_TEMP: Duration = Duration::from_secs(24 * 60 * 60);

/// Atomically and durably replaces `path` with `data`. Keeps the target's permissions when it
/// exists; a new file gets the usual default ones. A link is followed: the file it points to is
/// replaced and the link stays.
pub fn write(path: &Path, data: &[u8]) -> io::Result<()> {
    replace(path, data, NewMode::Default)
}

/// [`write`] for a file holding a secret: a new file is readable by this user only (Unix).
pub fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    replace(path, data, NewMode::Private)
}

/// [`write`], first keeping the current file as `<name>.prev` when it is whole and `valid` accepts
/// it. A damaged current file never replaces a good previous copy.
pub fn write_with_prev(path: &Path, data: &[u8], valid: impl Fn(&[u8]) -> bool) -> io::Result<()> {
    match std::fs::read(path) {
        Ok(current) if current != data && damage(&current).is_none() && valid(&current) => {
            let prev = prev_path(path);
            replace(&prev, &current, mode_of(path))?;
        }
        // Missing, damaged, unchanged, or unreadable for now: the previous copy stays as it is.
        _ => {}
    }
    replace(path, data, NewMode::Default)
}

/// Copies `src` over `dest` the way [`write`] writes: a power cut leaves either the old `dest` or a
/// whole copy, never an empty or zeroed one. Streams, so a large file is not held in memory.
pub fn copy(src: &Path, dest: &Path) -> io::Result<()> {
    let mut from = std::fs::File::open(src)?;
    replace_with(dest, mode_of(src), |file| io::copy(&mut from, file).map(drop))
}

/// A validator for [`write_with_prev`]: the bytes are a JSON document.
pub fn is_json(bytes: &[u8]) -> bool {
    serde_json::from_slice::<serde::de::IgnoredAny>(bytes).is_ok()
}

/// `<name>.prev` beside `path`.
pub fn prev_path(path: &Path) -> PathBuf {
    with_suffix(path, ".prev")
}

/// Which file a [`load`] used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The file itself.
    Current,
    /// The file was missing or damaged: the previous version was used.
    Previous,
}

/// A file that could not be used, and where a copy of it was kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Damaged {
    pub reason: String,
    pub kept_at: Option<PathBuf>,
}

#[derive(Debug)]
pub struct Loaded<T> {
    pub value: T,
    pub source: Source,
    /// Set when the current file was damaged (so `source` is `Previous`).
    pub damaged: Option<Damaged>,
}

#[derive(Debug)]
pub enum LoadError {
    /// A file exists but could not be read (locked, no permission): nothing was decided, and the
    /// caller must not write over it.
    Io(io::Error),
    /// The file is damaged and no usable previous version exists. A copy was kept.
    Damaged(Damaged),
}

impl Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
            LoadError::Damaged(d) => match &d.kept_at {
                Some(at) => write!(f, "{} (a copy was kept at {})", d.reason, at.display()),
                None => write!(f, "{}", d.reason),
            },
        }
    }
}

impl std::error::Error for LoadError {}

/// Reads `path` with `parse`, falling back to `<name>.prev` when the file is missing, empty, all
/// zeros or rejected by `parse`. A damaged file is left in place (the next write replaces it) and a
/// timestamped copy is kept beside it for diagnosis. `Ok(None)`: there is no file at all, or only a
/// damaged previous copy, which is what a first start looks like. A read that fails for another
/// reason (a scanner holding the file) is tried again briefly before it is an error.
pub fn load<T, E: Display>(path: &Path, parse: impl Fn(&[u8]) -> Result<T, E>) -> Result<Option<Loaded<T>>, LoadError> {
    load_with(path, parse, true)
}

/// [`load`] for a file whose previous version must not come back when the file was removed (by the
/// person, as a reset): only a file that is there and damaged falls back to `<name>.prev`.
pub fn load_unless_removed<T, E: Display>(path: &Path, parse: impl Fn(&[u8]) -> Result<T, E>) -> Result<Option<Loaded<T>>, LoadError> {
    load_with(path, parse, false)
}

fn load_with<T, E: Display>(path: &Path, parse: impl Fn(&[u8]) -> Result<T, E>, when_missing: bool) -> Result<Option<Loaded<T>>, LoadError> {
    sweep_stale_temps(path, STALE_TEMP);
    let damaged = match read_usable(path, &parse) {
        Ok(Some(value)) => return Ok(Some(Loaded { value, source: Source::Current, damaged: None })),
        Ok(None) if !when_missing => return Ok(None),
        Ok(None) => None,
        Err(Bad::Io(e)) => return Err(LoadError::Io(e)),
        Err(Bad::Damaged(reason, bytes)) => Some(Damaged { reason, kept_at: keep_damaged(path, &bytes) }),
    };
    match read_usable(&prev_path(path), &parse) {
        Ok(Some(value)) => Ok(Some(Loaded { value, source: Source::Previous, damaged })),
        Ok(None) | Err(Bad::Damaged(..)) => damaged.map_or(Ok(None), |d| Err(LoadError::Damaged(d))),
        Err(Bad::Io(e)) => Err(LoadError::Io(e)),
    }
}

/// The file, or else its previous version, when one of them is usable; `None` otherwise. Never writes (no damaged
/// copy, no sweep): for a helper process that only looks.
pub fn read_either<T, E: Display>(path: &Path, parse: impl Fn(&[u8]) -> Result<T, E>) -> Option<T> {
    [path.to_path_buf(), prev_path(path)].iter().find_map(|p| std::fs::read(p).ok().filter(|b| damage(b).is_none()).and_then(|b| parse(&b).ok()))
}

enum Bad {
    Io(io::Error),
    Damaged(String, Vec<u8>),
}

/// `Ok(None)`: no such file.
fn read_usable<T, E: Display>(path: &Path, parse: &impl Fn(&[u8]) -> Result<T, E>) -> Result<Option<T>, Bad> {
    let mut tries = 0;
    let bytes = loop {
        match std::fs::read(path) {
            Ok(b) => break b,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            // A folder, or a file that stays locked: an error, never a fallback.
            Err(e) if tries >= 3 || std::fs::metadata(path).is_ok_and(|m| m.is_dir()) => return Err(Bad::Io(e)),
            Err(_) => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    if let Some(reason) = damage(&bytes) {
        return Err(Bad::Damaged(reason.into(), bytes));
    }
    parse(&bytes).map(Some).map_err(|e| Bad::Damaged(e.to_string(), bytes))
}

/// What a power cut leaves behind, whatever the format.
fn damage(bytes: &[u8]) -> Option<&'static str> {
    if bytes.is_empty() {
        Some("the file is empty")
    } else if bytes.iter().all(|&b| b == 0) {
        Some("the file holds only zero bytes")
    } else {
        None
    }
}

/// Keeps `<name>.damaged-<ms>` beside `path`, unless the newest copy already holds these bytes, and
/// removes all but the newest few. For callers that read a file their own way; [`load`] does it itself.
pub fn keep_damaged(path: &Path, bytes: &[u8]) -> Option<PathBuf> {
    let mut copies = damaged_copies(path);
    if let Some((_, newest)) = copies.last() {
        if std::fs::read(newest).is_ok_and(|b| b == bytes) {
            return Some(newest.clone());
        }
    }
    let ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let at = with_suffix(path, &format!(".damaged-{ms}"));
    let kept = replace(&at, bytes, mode_of(path)).is_ok();
    if kept {
        copies.push((ms, at.clone()));
    }
    let excess = copies.len().saturating_sub(DAMAGED_KEPT);
    for (_, old) in copies.drain(..excess) {
        let _ = std::fs::remove_file(old);
    }
    kept.then_some(at)
}

/// `<name>.damaged-<ms>` files beside `path`, oldest first.
fn damaged_copies(path: &Path) -> Vec<(u128, PathBuf)> {
    let (Some(dir), Some(name)) = (parent(path), path.file_name()) else { return Vec::new() };
    let prefix = format!("{}.damaged-", name.to_string_lossy());
    let mut found: Vec<(u128, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let ms = e.file_name().to_string_lossy().strip_prefix(&prefix)?.parse().ok()?;
            Some((ms, e.path()))
        })
        .collect();
    found.sort();
    found
}

/// Removes temporary files of writes to `path` that never finished, older than `age`.
pub fn sweep_stale_temps(path: &Path, age: Duration) {
    let (Some(dir), Some(name)) = (parent(path), path.file_name()) else { return };
    let Some(cutoff) = SystemTime::now().checked_sub(age) else { return };
    let prefix = format!(".{}.", name.to_string_lossy());
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = entry.file_name().to_string_lossy().into_owned();
        if n.starts_with(&prefix) && n.ends_with(".tmp") && entry.metadata().and_then(|m| m.modified()).is_ok_and(|at| at < cutoff) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn parent(path: &Path) -> Option<&Path> {
    match path.parent() {
        Some(p) if p.as_os_str().is_empty() => Some(Path::new(".")),
        p => p,
    }
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// Permissions for a file that does not exist yet.
#[derive(Clone, Copy)]
enum NewMode {
    Default,
    Private,
    #[cfg_attr(not(unix), allow(dead_code))]
    Like(u32),
}

/// The permissions of a file being replaced: its temporary file has them from the start, so a private
/// file's new content is never readable by others, even for a moment.
fn existing_mode(target: &Path) -> Option<NewMode> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(target).ok().map(|m| NewMode::Like(m.permissions().mode() & 0o7777))
    }
    #[cfg(not(unix))]
    {
        let _ = target;
        None
    }
}

/// A copy of `path` gets its permissions (a private file's copy stays private).
fn mode_of(path: &Path) -> NewMode {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(path) {
            return NewMode::Like(meta.permissions().mode() & 0o7777);
        }
    }
    let _ = path;
    NewMode::Private
}

fn replace(path: &Path, data: &[u8], mode: NewMode) -> io::Result<()> {
    replace_with(path, mode, |file| file.write_all(data))
}

/// Fills a new temporary file beside `path` with `fill`, flushes it to the disk and swaps it in.
fn replace_with(path: &Path, mode: NewMode, fill: impl FnOnce(&mut std::fs::File) -> io::Result<()>) -> io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    // Through a link: a dotfiles repository's symlink stays a link.
    let target = match std::fs::symlink_metadata(path) {
        // A link to nothing is replaced by the file, as a plain rename would.
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        _ => path.to_path_buf(),
    };
    let dir = parent(&target).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file path"))?;
    let name = target.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "not a file path"))?.to_string_lossy().into_owned();
    // A name of its own, created new: two writers never share a temporary file, and a file or link
    // already at that name is never written through.
    let mut opened = None;
    for _ in 0..16 {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
        let tmp = dir.join(format!(".{name}.{}-{}-{nanos:08x}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
        match open_new(&tmp, existing_mode(&target).unwrap_or(mode)) {
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            r => {
                opened = Some(r.map(|f| (tmp, f)));
                break;
            }
        }
    }
    let (tmp, mut file) = opened.unwrap_or_else(|| Err(io::Error::new(io::ErrorKind::AlreadyExists, "could not create a temporary file")))?;
    let result = fill(&mut file).and_then(|()| file.sync_all()).and_then(|()| {
        #[cfg(unix)]
        {
            if let Ok(meta) = std::fs::metadata(&target) {
                let _ = file.set_permissions(meta.permissions());
            }
        }
        swap_in(file, &tmp, &target)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn open_new(tmp: &Path, mode: NewMode) -> io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(match mode {
            NewMode::Default => 0o666,
            NewMode::Private => 0o600,
            NewMode::Like(m) => m,
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::DELETE;
        // DELETE: the handle itself renames the file over the target.
        opts.access_mode(GENERIC_WRITE | DELETE);
        let _ = mode;
    }
    opts.open(tmp)
}

/// Retries a rename Windows refuses for a moment (a virus scanner or indexer holding the target).
#[cfg(windows)]
fn retrying(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match op() {
            // 32: ERROR_SHARING_VIOLATION.
            Err(e) if (e.kind() == io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(32)) && attempt < 8 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(25 * attempt));
            }
            done => return done,
        }
    }
}

#[cfg(unix)]
fn swap_in(file: std::fs::File, tmp: &Path, target: &Path) -> io::Result<()> {
    drop(file);
    std::fs::rename(tmp, target)?;
    // The rename itself is durable only once the folder is flushed. Some file systems refuse to
    // flush a folder; the file is in place either way.
    if let Some(dir) = parent(target) {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

#[cfg(windows)]
fn swap_in(file: std::fs::File, tmp: &Path, target: &Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED};
    match retrying(|| posix_replace(&file, target)) {
        Ok(()) => {
            // Commits the rename to the file system's log as well.
            let _ = file.sync_all();
            Ok(())
        }
        // A file system without POSIX renames (FAT, some network shares): a plain replace.
        Err(e) if [ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED, ERROR_INVALID_FUNCTION].map(|c| c as i32).contains(&e.raw_os_error().unwrap_or(0)) => {
            drop(file);
            retrying(|| move_replace(tmp, target))
        }
        Err(e) => Err(e),
    }
}

/// `MoveFileEx` replacing the target, returning once the move is on the disk.
#[cfg(windows)]
fn move_replace(from: &Path, to: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (from, to) = (wide(from), wide(to));
    // SAFETY: both are NUL-terminated UTF-16 strings that live across the call.
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Renames the open file over `destination` with POSIX semantics: unlike `MoveFileEx`, readers that
/// have the old file open keep reading it, and new readers open the new file instead of meeting a
/// delete-pending name.
/// https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information
#[cfg(windows)]
fn posix_replace(file: &std::fs::File, destination: &Path) -> io::Result<()> {
    use std::mem::{align_of, offset_of, size_of};
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{FileRenameInfoEx, SetFileInformationByHandle, FILE_RENAME_INFO};

    const REPLACE_IF_EXISTS: u32 = 1;
    const POSIX_SEMANTICS: u32 = 2;
    const { assert!(align_of::<usize>() >= align_of::<FILE_RENAME_INFO>()) };
    let too_long = || io::Error::new(io::ErrorKind::InvalidInput, "destination path is too long");
    let destination = std::path::absolute(destination)?;
    let name_units = destination.as_os_str().encode_wide().count();
    let name_bytes = u32::try_from(name_units.checked_mul(2).ok_or_else(too_long)?).map_err(|_| too_long())?;
    let offset = offset_of!(FILE_RENAME_INFO, FileName);
    let size = size_of::<FILE_RENAME_INFO>().max(offset + name_bytes as usize + 2);
    let size_u32 = u32::try_from(size).map_err(|_| too_long())?;
    let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: usize storage supplies native alignment, all bytes are initialized,
    // and the allocation includes the complete variable-length UTF-16 filename.
    unsafe {
        (*info).Anonymous.Flags = REPLACE_IF_EXISTS | POSIX_SEMANTICS;
        (*info).FileNameLength = name_bytes;
        let name = storage.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>();
        for (i, unit) in destination.as_os_str().encode_wide().chain([0]).enumerate() {
            name.add(i).write(unit);
        }
    }
    // SAFETY: the file owns a live handle with DELETE access; info points to the
    // initialized, aligned buffer above and stays alive until this synchronous call ends.
    let renamed = unsafe { SetFileInformationByHandle(file.as_raw_handle(), FileRenameInfoEx, info.cast(), size_u32) };
    if renamed == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);
    impl Dir {
        fn new() -> Dir {
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let d = std::env::temp_dir().join(format!("kelpie-durable-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn names(&self) -> Vec<String> {
            let mut n: Vec<String> = std::fs::read_dir(&self.0).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
            n.sort();
            n
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn parse(b: &[u8]) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::from_slice(b)
    }

    fn load_json(path: &Path) -> Result<Option<Loaded<serde_json::Value>>, LoadError> {
        load(path, parse)
    }

    #[test]
    fn a_write_replaces_the_file_and_leaves_no_temporary_file() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        write(&path, b"{\"a\":1}").unwrap();
        write(&path, b"{\"a\":2}").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"{\"a\":2}");
        assert_eq!(d.names(), ["state.json"]);
    }

    #[test]
    fn a_swap_that_fails_after_the_temporary_file_exists_removes_it() {
        let d = Dir::new();
        // A folder with something in it cannot be replaced by a file.
        let target = d.0.join("out.md");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("inside"), b"kept").unwrap();
        assert!(write(&target, b"new text").is_err());
        assert_eq!(std::fs::read(target.join("inside")).unwrap(), b"kept", "the target is untouched");
        assert_eq!(d.names(), ["out.md"], "no temporary file is left");
    }

    #[test]
    fn a_copy_replaces_the_destination_whole() {
        let d = Dir::new();
        let (src, dest) = (d.0.join("src.bin"), d.0.join("dest.bin"));
        let body: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &body).unwrap();
        std::fs::write(&dest, b"old").unwrap();
        copy(&src, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(d.names(), ["dest.bin", "src.bin"]);
        assert!(copy(&d.0.join("missing"), &dest).is_err());
        assert_eq!(std::fs::read(&dest).unwrap(), body, "a failed copy leaves the destination alone");
    }

    #[test]
    fn the_previous_version_is_kept_and_used_when_the_file_is_all_zeros() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        write_with_prev(&path, br#"{"v":1}"#, is_json).unwrap();
        assert!(!prev_path(&path).exists(), "nothing to keep before the first write");
        write_with_prev(&path, br#"{"v":2}"#, is_json).unwrap();
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), br#"{"v":1}"#);
        // What the power cut left: the right size, every byte zero.
        std::fs::write(&path, vec![0u8; 7]).unwrap();
        let loaded = load_json(&path).unwrap().unwrap();
        assert_eq!(loaded.value, serde_json::json!({"v": 1}));
        assert_eq!(loaded.source, Source::Previous);
        let damaged = loaded.damaged.unwrap();
        assert!(damaged.reason.contains("zero"), "{}", damaged.reason);
        let kept = damaged.kept_at.unwrap();
        assert_eq!(std::fs::read(&kept).unwrap(), vec![0u8; 7]);
        assert_eq!(std::fs::read(&path).unwrap(), vec![0u8; 7], "the damaged file is left for the next write");
    }

    #[test]
    fn a_truncated_or_empty_file_falls_back_to_the_previous_version() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        write_with_prev(&path, br#"{"v":1}"#, is_json).unwrap();
        write_with_prev(&path, br#"{"v":2}"#, is_json).unwrap();
        for bad in [&br#"{"v":"#[..], b""] {
            std::fs::write(&path, bad).unwrap();
            let loaded = load_json(&path).unwrap().unwrap();
            assert_eq!((loaded.value, loaded.source), (serde_json::json!({"v": 1}), Source::Previous));
        }
    }

    #[test]
    fn a_missing_file_with_a_previous_version_loads_that_version() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        std::fs::write(prev_path(&path), br#"{"v":1}"#).unwrap();
        let loaded = load_json(&path).unwrap().unwrap();
        assert_eq!((loaded.value, loaded.source, loaded.damaged), (serde_json::json!({"v": 1}), Source::Previous, None));
    }

    #[test]
    fn a_removed_file_does_not_come_back_when_asked_but_a_damaged_one_does() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        write_with_prev(&path, br#"{"v":1}"#, is_json).unwrap();
        write_with_prev(&path, br#"{"v":2}"#, is_json).unwrap();
        std::fs::write(&path, [0u8; 7]).unwrap();
        let loaded = load_unless_removed(&path, parse).unwrap().unwrap();
        assert_eq!((loaded.value, loaded.source), (serde_json::json!({"v": 1}), Source::Previous));
        std::fs::remove_file(&path).unwrap();
        assert!(load_unless_removed(&path, parse).unwrap().is_none(), "removed: a reset, not damage");
        assert!(load_json(&path).unwrap().is_some(), "the plain load still restores it");
    }

    #[test]
    fn a_good_file_loads_as_current_and_no_file_at_all_is_none() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        assert!(load_json(&path).unwrap().is_none());
        write_with_prev(&path, br#"{"v":1}"#, is_json).unwrap();
        let loaded = load_json(&path).unwrap().unwrap();
        assert_eq!((loaded.value, loaded.source), (serde_json::json!({"v": 1}), Source::Current));
    }

    #[test]
    fn both_damaged_is_an_error_with_a_copy_kept_and_nothing_overwritten() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        std::fs::write(&path, b"{oops").unwrap();
        std::fs::write(prev_path(&path), [0u8; 4]).unwrap();
        let Err(LoadError::Damaged(damaged)) = load_json(&path) else { panic!("expected damaged") };
        assert_eq!(std::fs::read(damaged.kept_at.unwrap()).unwrap(), b"{oops");
        assert_eq!(std::fs::read(&path).unwrap(), b"{oops");
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), [0u8; 4]);
        // Only a damaged previous copy, no file: a first start.
        std::fs::remove_file(&path).unwrap();
        assert!(load_json(&path).unwrap().is_none());
    }

    #[test]
    fn the_previous_version_is_replaced_only_by_a_good_file() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        write_with_prev(&path, br#"{"v":1}"#, is_json).unwrap();
        write_with_prev(&path, br#"{"v":2}"#, is_json).unwrap();
        for bad in [vec![0u8; 9], b"{trunc".to_vec(), Vec::new()] {
            std::fs::write(&path, &bad).unwrap();
            write_with_prev(&path, br#"{"v":3}"#, is_json).unwrap();
            assert_eq!(std::fs::read(prev_path(&path)).unwrap(), br#"{"v":1}"#, "a damaged file never becomes the previous version");
            assert_eq!(std::fs::read(&path).unwrap(), br#"{"v":3}"#);
        }
        // A validator stricter than JSON is honoured too.
        write_with_prev(&path, br#"{"v":4}"#, |b| b.starts_with(b"{\"v\":9")).unwrap();
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), br#"{"v":1}"#);
        // Rewriting the same bytes keeps the older version rather than a copy of the current one.
        write_with_prev(&path, br#"{"v":4}"#, is_json).unwrap();
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), br#"{"v":1}"#);
        write_with_prev(&path, br#"{"v":5}"#, is_json).unwrap();
        assert_eq!(std::fs::read(prev_path(&path)).unwrap(), br#"{"v":4}"#);
    }

    #[test]
    fn damaged_copies_are_not_repeated_and_only_the_newest_few_are_kept() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        std::fs::write(prev_path(&path), br#"{"v":1}"#).unwrap();
        std::fs::write(&path, [0u8; 3]).unwrap();
        let first = load_json(&path).unwrap().unwrap().damaged.unwrap().kept_at.unwrap();
        let again = load_json(&path).unwrap().unwrap().damaged.unwrap().kept_at.unwrap();
        assert_eq!(first, again, "the same damage is kept once");
        for i in 0..6u8 {
            std::fs::write(&path, [b'{', b'x', i]).unwrap();
            std::thread::sleep(Duration::from_millis(2));
            load_json(&path).unwrap();
        }
        assert_eq!(damaged_copies(&path).len(), DAMAGED_KEPT);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_error_not_a_fallback() {
        let d = Dir::new();
        // A folder where the file should be: reading it fails with an error other than "not found".
        let path = d.0.join("state.json");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(prev_path(&path), br#"{"v":1}"#).unwrap();
        assert!(matches!(load_json(&path), Err(LoadError::Io(_))));
    }

    #[test]
    fn stale_temporary_files_are_swept_and_fresh_ones_kept() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        std::fs::write(d.0.join(".state.json.1-2-3.tmp"), b"x").unwrap();
        std::fs::write(d.0.join(".other.json.1-2-3.tmp"), b"x").unwrap();
        sweep_stale_temps(&path, Duration::from_secs(3600));
        assert_eq!(d.names(), [".other.json.1-2-3.tmp", ".state.json.1-2-3.tmp"]);
        std::thread::sleep(Duration::from_millis(20));
        sweep_stale_temps(&path, Duration::from_millis(10));
        assert_eq!(d.names(), [".other.json.1-2-3.tmp"]);
    }

    #[test]
    fn concurrent_writers_leave_one_whole_version() {
        let d = Dir::new();
        let path = d.0.join("state.json");
        std::thread::scope(|s| {
            for i in 0..8 {
                let path = &path;
                s.spawn(move || {
                    for _ in 0..10 {
                        write_with_prev(path, format!("{{\"w\":{i},\"pad\":\"{}\"}}", "x".repeat(4000)).as_bytes(), is_json).unwrap();
                    }
                });
            }
        });
        assert!(is_json(&std::fs::read(&path).unwrap()));
        assert!(is_json(&std::fs::read(prev_path(&path)).unwrap()));
        assert_eq!(d.names(), ["state.json", "state.json.prev"]);
    }

    #[cfg(unix)]
    #[test]
    fn private_files_and_their_copies_stay_private() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new();
        let path = d.0.join("secret.json");
        write_private(&path, b"{}").unwrap();
        write_with_prev(&path, b"{\"a\":1}", is_json).unwrap();
        for p in [path.clone(), prev_path(&path)] {
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "{}", p.display());
        }
    }

    #[cfg(windows)]
    #[test]
    fn replacing_a_published_file_preserves_old_and_new_readers() {
        use std::io::Read;
        let d = Dir::new();
        let path = d.0.join("state-雪.json");
        write(&path, br#"{"revision":1}"#).unwrap();
        let mut old_reader = std::fs::File::open(&path).unwrap();
        write_with_prev(&path, br#"{"revision":2}"#, is_json).unwrap();
        let new_bytes = std::fs::read(&path).unwrap();
        let mut old_bytes = Vec::new();
        old_reader.read_to_end(&mut old_bytes).unwrap();
        drop(old_reader);
        assert_eq!(old_bytes, br#"{"revision":1}"#);
        assert_eq!(new_bytes, br#"{"revision":2}"#);
    }
}
