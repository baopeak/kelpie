//! This computer's files as one end of a listing or a transfer. Blocking calls run on Tokio's blocking pool.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncSeekExt, AsyncWrite, AsyncWriteExt};

use super::{readable_name, Entry, ErrKind, FsError, FsResult, Kind, Meta, RemoteFs, WriteFile};

pub struct LocalFs;

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> FsResult<T> + Send + 'static) -> FsResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| FsError::new(ErrKind::Other, e.to_string()))?
}

fn secs(t: std::io::Result<SystemTime>) -> u64 {
    t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs())
}

/// `std` reports Windows junctions as links too, so a junction is never walked into as a folder.
fn meta_of(m: &std::fs::Metadata) -> Meta {
    let ft = m.file_type();
    let kind = if ft.is_symlink() {
        Kind::Link
    } else if ft.is_dir() {
        Kind::Dir
    } else if ft.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    #[cfg(unix)]
    let (mode, uid, gid) = {
        use std::os::unix::fs::MetadataExt;
        (Some(m.mode() & 0o7777), Some(m.uid()), Some(m.gid()))
    };
    #[cfg(not(unix))]
    let (mode, uid, gid) = (None, None, None);
    Meta { kind, size: if kind == Kind::Dir { 0 } else { m.len() }, mtime: secs(m.modified()), mode, uid, gid }
}

fn hidden(name: &str, m: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
        if m.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0 {
            return true;
        }
    }
    #[cfg(not(windows))]
    let _ = m;
    name.starts_with('.')
}

/// `\\?\C:\x` → `C:\x` and `\\?\UNC\srv\share` → `\\srv\share` (what `canonicalize` returns on Windows).
pub fn simplify(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.as_bytes().get(1) == Some(&b':') => PathBuf::from(rest),
        _ => p,
    }
}

fn path_string(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn drives() -> Vec<Entry> {
    // SAFETY: no arguments; returns a bit mask of the drive letters in use.
    let mask = unsafe { windows_sys::Win32::Storage::FileSystem::GetLogicalDrives() };
    (0..26u8)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| Entry {
            name: format!("{}:\\", (b'A' + i) as char),
            meta: Meta { kind: Kind::Dir, size: 0, mtime: 0, mode: None, uid: None, gid: None },
            owner: None,
            group: None,
            link_target: None,
            readable_name: true,
            hidden: false,
        })
        .collect()
}

fn list_blocking(dir: &str) -> FsResult<Vec<Entry>> {
    #[cfg(windows)]
    if dir.is_empty() {
        return Ok(drives());
    }
    let mut out = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let raw = item.file_name();
        let (name, readable) = match raw.to_str() {
            Some(s) => (s.to_string(), true),
            None => (raw.to_string_lossy().into_owned(), false),
        };
        // An entry that vanished between the listing and its stat is skipped, not an error.
        let Ok(m) = std::fs::symlink_metadata(item.path()) else { continue };
        let meta = meta_of(&m);
        let link_target = (meta.kind == Kind::Link).then(|| std::fs::read_link(item.path()).ok().map(|t| path_string(&t))).flatten();
        out.push(Entry { hidden: hidden(&name, &m), readable_name: readable && readable_name(&name), name, meta, owner: None, group: None, link_target });
    }
    Ok(out)
}

#[async_trait]
impl RemoteFs for LocalFs {
    async fn list(&self, dir: &str) -> FsResult<Vec<Entry>> {
        let dir = dir.to_string();
        blocking(move || list_blocking(&dir)).await
    }

    async fn stat(&self, path: &str) -> FsResult<Meta> {
        Ok(meta_of(&tokio::fs::metadata(path).await?))
    }

    async fn lstat(&self, path: &str) -> FsResult<Meta> {
        Ok(meta_of(&tokio::fs::symlink_metadata(path).await?))
    }

    async fn readlink(&self, path: &str) -> FsResult<String> {
        Ok(path_string(&tokio::fs::read_link(path).await?))
    }

    async fn realpath(&self, path: &str) -> FsResult<String> {
        if matches!(path, "" | "." | "~") {
            return dirs::home_dir().map(|h| path_string(&h)).ok_or_else(|| FsError::new(ErrKind::NotFound, "no home folder"));
        }
        Ok(path_string(&simplify(tokio::fs::canonicalize(path).await?)))
    }

    async fn mkdir(&self, path: &str) -> FsResult<()> {
        Ok(tokio::fs::create_dir(path).await?)
    }

    async fn rename(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
        // `std::fs::rename` replaces an existing file on every platform; refusing one is a check first
        // (best effort: nothing else writes into a transfer's target folder by design).
        if !overwrite && tokio::fs::symlink_metadata(to).await.is_ok() {
            return Err(FsError::new(ErrKind::Exists, format!("{to} already exists")));
        }
        Ok(tokio::fs::rename(from, to).await?)
    }

    async fn remove_file(&self, path: &str) -> FsResult<()> {
        Ok(tokio::fs::remove_file(path).await?)
    }

    async fn remove_dir(&self, path: &str) -> FsResult<()> {
        Ok(tokio::fs::remove_dir(path).await?)
    }

    async fn chmod(&self, path: &str, mode: u32) -> FsResult<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            Ok(tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o7777)).await?)
        }
        #[cfg(not(unix))]
        {
            let _ = (path, mode);
            Err(FsError::new(ErrKind::Unsupported, "permissions cannot be set on this computer"))
        }
    }

    async fn set_times(&self, path: &str, atime: u64, mtime: u64) -> FsResult<()> {
        let path = path.to_string();
        blocking(move || {
            let at = |s: u64| UNIX_EPOCH + std::time::Duration::from_secs(s);
            let f = std::fs::OpenOptions::new().write(true).open(&path)?;
            f.set_times(std::fs::FileTimes::new().set_accessed(at(atime)).set_modified(at(mtime)))?;
            Ok(())
        })
        .await
    }

    async fn open_read(&self, path: &str, offset: u64) -> FsResult<Box<dyn AsyncRead + Send + Unpin>> {
        let mut f = tokio::fs::File::open(path).await?;
        if offset > 0 {
            f.seek(std::io::SeekFrom::Start(offset)).await?;
        }
        Ok(Box::new(f))
    }

    async fn open_write(&self, path: &str, offset: u64, create: bool) -> FsResult<Box<dyn WriteFile>> {
        let mut f = tokio::fs::OpenOptions::new().write(true).create(create).open(path).await?;
        f.set_len(offset).await?;
        f.seek(std::io::SeekFrom::Start(offset)).await?;
        Ok(Box::new(LocalWrite(f)))
    }

    async fn free_space(&self, path: &str) -> FsResult<Option<u64>> {
        let path = path.to_string();
        blocking(move || free_space_blocking(&path)).await
    }
}

#[cfg(windows)]
fn free_space_blocking(path: &str) -> FsResult<Option<u64>> {
    let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut avail = 0u64;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the other outputs may be null.
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Some(avail))
}

#[cfg(unix)]
fn free_space_blocking(path: &str) -> FsResult<Option<u64>> {
    let c = std::ffi::CString::new(path).map_err(|_| FsError::invalid("a path cannot contain NUL"))?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `c` is a valid C string and `s` a writable statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(Some(s.f_bavail as u64 * s.f_frsize as u64))
}

struct LocalWrite(tokio::fs::File);

impl AsyncWrite for LocalWrite {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

#[async_trait]
impl WriteFile for LocalWrite {
    async fn finish(mut self: Box<Self>, fsync: bool) -> FsResult<()> {
        self.0.flush().await?;
        if fsync {
            self.0.sync_all().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kelpie-localfs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn simplifies_verbatim_paths() {
        assert_eq!(simplify(PathBuf::from(r"\\?\C:\Users\me")), PathBuf::from(r"C:\Users\me"));
        assert_eq!(simplify(PathBuf::from(r"\\?\UNC\srv\share\x")), PathBuf::from(r"\\srv\share\x"));
        assert_eq!(simplify(PathBuf::from("/home/me")), PathBuf::from("/home/me"));
    }

    #[tokio::test]
    async fn lists_writes_resumes_and_renames() {
        let d = temp("basic");
        let fs = LocalFs;
        let p = |n: &str| path_string(&d.join(n));
        std::fs::write(d.join("a.txt"), b"hello").unwrap();
        std::fs::write(d.join(".hidden"), b"").unwrap();
        std::fs::create_dir(d.join("sub")).unwrap();
        let mut names: Vec<(String, Kind, bool)> = fs.list(&path_string(&d)).await.unwrap().into_iter().map(|e| (e.name, e.meta.kind, e.hidden)).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(names, [(".hidden".into(), Kind::File, true), ("a.txt".into(), Kind::File, false), ("sub".into(), Kind::Dir, false)]);

        // A partial of 3 bytes continues at 3; anything past the offset is cut off.
        let mut w = fs.open_write(&p("b.part"), 0, true).await.unwrap();
        w.write_all(b"abcXYZ").await.unwrap();
        w.finish(true).await.unwrap();
        let mut w = fs.open_write(&p("b.part"), 3, false).await.unwrap();
        w.write_all(b"def").await.unwrap();
        w.finish(false).await.unwrap();
        assert_eq!(std::fs::read(d.join("b.part")).unwrap(), b"abcdef");

        let mut r = fs.open_read(&p("b.part"), 2).await.unwrap();
        let mut s = String::new();
        r.read_to_string(&mut s).await.unwrap();
        assert_eq!(s, "cdef");

        assert_eq!(fs.rename(&p("b.part"), &p("a.txt"), false).await.unwrap_err().kind, ErrKind::Exists);
        fs.rename(&p("b.part"), &p("a.txt"), true).await.unwrap();
        assert_eq!(std::fs::read(d.join("a.txt")).unwrap(), b"abcdef");

        fs.set_times(&p("a.txt"), 1_600_000_000, 1_700_000_000).await.unwrap();
        assert_eq!(fs.stat(&p("a.txt")).await.unwrap().mtime, 1_700_000_000);
        assert!(fs.free_space(&path_string(&d)).await.unwrap().unwrap_or(0) > 0);
        assert_eq!(fs.stat(&p("missing")).await.unwrap_err().kind, ErrKind::NotFound);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn lists_drives() {
        let drives = LocalFs.list("").await.unwrap();
        assert!(drives.iter().any(|e| e.name.ends_with(":\\") && e.meta.kind == Kind::Dir));
    }
}
