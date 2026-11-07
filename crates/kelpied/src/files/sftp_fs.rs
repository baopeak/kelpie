//! A host's files over one SFTP session ([`pool::Conn`]), as one end of a listing or a transfer.

use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use russh_sftp::client::fs::File;
use russh_sftp::protocol::{FileAttributes, OpenFlags};
use tokio::io::{AsyncRead, AsyncSeekExt, AsyncWrite, ReadBuf};

use super::pool::Conn;
use super::{kind_from_mode, readable_name, Entry, ErrKind, FsError, FsResult, Kind, Meta, RemoteFs, WriteFile};

pub struct SftpFs(pub Arc<Conn>);

/// What a listing or `stat` says about a path.
pub fn meta_of(a: &FileAttributes) -> Meta {
    let kind = match a.permissions {
        Some(p) => kind_from_mode(p),
        None if a.is_dir() => Kind::Dir,
        None => Kind::File,
    };
    Meta {
        kind,
        size: if kind == Kind::Dir { 0 } else { a.size.unwrap_or(0) },
        mtime: a.mtime.map_or(0, u64::from),
        mode: a.permissions.map(|p| p & 0o7777),
        uid: a.uid,
        gid: a.gid,
    }
}

/// Owner and group: the names when the server sends them, else the numbers.
fn owner(name: &Option<String>, id: Option<u32>) -> Option<String> {
    name.clone().filter(|n| !n.is_empty()).or_else(|| id.map(|i| i.to_string()))
}

impl SftpFs {
    fn c(&self) -> &Conn {
        self.0.touch();
        &self.0
    }
}

#[async_trait]
impl RemoteFs for SftpFs {
    async fn list(&self, dir: &str) -> FsResult<Vec<Entry>> {
        let c = self.c();
        let rd = c.sftp.read_dir(dir).await.map_err(|e| c.error(e))?;
        let mut out = Vec::new();
        for e in rd {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let a = e.metadata();
            let meta = meta_of(&a);
            let link_target = if meta.kind == Kind::Link { c.sftp.read_link(super::join_remote(dir, &name)).await.ok() } else { None };
            out.push(Entry {
                readable_name: readable_name(&name),
                hidden: name.starts_with('.'),
                owner: owner(&a.user, a.uid),
                group: owner(&a.group, a.gid),
                link_target,
                name,
                meta,
            });
        }
        Ok(out)
    }

    async fn stat(&self, path: &str) -> FsResult<Meta> {
        let c = self.c();
        c.sftp.metadata(path).await.map(|a| meta_of(&a)).map_err(|e| c.error(e))
    }

    async fn lstat(&self, path: &str) -> FsResult<Meta> {
        let c = self.c();
        c.sftp.symlink_metadata(path).await.map(|a| meta_of(&a)).map_err(|e| c.error(e))
    }


    async fn realpath(&self, path: &str) -> FsResult<String> {
        let c = self.c();
        let p = if matches!(path, "" | "~") { "." } else { path };
        c.sftp.canonicalize(p).await.map_err(|e| c.error(e))
    }

    async fn mkdir(&self, path: &str) -> FsResult<()> {
        let c = self.c();
        c.sftp.create_dir(path).await.map_err(|e| c.error(e))
    }

    async fn rename(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
        let c = self.c();
        // SFTP v3's rename never replaces, and this engine does not reach `posix-rename@openssh.com`: a replace
        // is the caller's move-aside (`copy::replace`).
        if overwrite {
            return Err(FsError::new(ErrKind::Unsupported, "the host cannot replace a file in one step"));
        }
        if c.sftp.try_exists(to).await.map_err(|e| c.error(e))? {
            return Err(FsError::new(ErrKind::Exists, format!("{to} already exists")));
        }
        c.sftp.rename(from, to).await.map_err(|e| c.error(e))
    }

    async fn remove_file(&self, path: &str) -> FsResult<()> {
        let c = self.c();
        c.sftp.remove_file(path).await.map_err(|e| c.error(e))
    }

    async fn remove_dir(&self, path: &str) -> FsResult<()> {
        let c = self.c();
        c.sftp.remove_dir(path).await.map_err(|e| c.error(e))
    }

    async fn chmod(&self, path: &str, mode: u32) -> FsResult<()> {
        let c = self.c();
        let mut a = FileAttributes::empty();
        a.permissions = Some(mode & 0o7777);
        c.sftp.set_metadata(path, a).await.map_err(|e| c.error(e))
    }

    async fn set_times(&self, path: &str, atime: u64, mtime: u64) -> FsResult<()> {
        let c = self.c();
        let mut a = FileAttributes::empty();
        a.atime = Some(atime.min(u32::MAX as u64) as u32);
        a.mtime = Some(mtime.min(u32::MAX as u64) as u32);
        c.sftp.set_metadata(path, a).await.map_err(|e| c.error(e))
    }

    async fn open_read(&self, path: &str, offset: u64) -> FsResult<Box<dyn AsyncRead + Send + Unpin>> {
        let c = self.c();
        let mut f = c.sftp.open(path).await.map_err(|e| c.error(e))?;
        if offset > 0 {
            f.seek(io::SeekFrom::Start(offset)).await.map_err(|e| c.io_error(e))?;
        }
        Ok(Box::new(SftpFile { f, conn: self.0.clone() }))
    }

    async fn open_write(&self, path: &str, offset: u64, create: bool) -> FsResult<Box<dyn WriteFile>> {
        let c = self.c();
        let mut f = if create && offset == 0 {
            // Made new with an exclusive create, 0600. Only when that fails is what has the name (a stale partial, a
            // planted link) removed and the create tried once more: protocol 3 has no "exists" status to tell by.
            match self.create_new(path).await {
                Ok(f) => f,
                Err(first) if matches!(first.kind, ErrKind::Other | ErrKind::Exists) => match c.sftp.remove_file(path).await.map_err(|e| c.error(e)) {
                    Ok(()) => self.create_new(path).await?,
                    Err(e) if e.kind == ErrKind::NotFound => return Err(first),
                    Err(e) => return Err(e),
                },
                Err(e) => return Err(e),
            }
        } else {
            // A resume continues a file that is there: it is never created (a partial gone meanwhile must not come
            // back zero-filled up to the offset).
            c.sftp.open_with_flags(path, OpenFlags::WRITE).await.map_err(|e| c.error(e))?
        };
        if offset > 0 {
            // Anything past the offset (a partial longer than what is kept) is cut off first.
            let mut a = FileAttributes::empty();
            a.size = Some(offset);
            f.set_metadata(a).await.map_err(|e| c.error(e))?;
            f.seek(io::SeekFrom::Start(offset)).await.map_err(|e| c.io_error(e))?;
        }
        Ok(Box::new(SftpFile { f, conn: self.0.clone() }))
    }

    async fn free_space(&self, path: &str) -> FsResult<Option<u64>> {
        let c = self.c();
        let s = c.sftp.fs_info(path).await.map_err(|e| c.error(e))?;
        Ok(s.map(|s| s.blocks_avail.saturating_mul(if s.fragment_size > 0 { s.fragment_size } else { s.block_size })))
    }

    fn case_insensitive(&self) -> bool {
        self.0.windows
    }

    fn new_file_mode(&self) -> u32 {
        self.0.new_mode.lock().unwrap_or(0o644)
    }
}

impl SftpFs {
    /// An exclusive create, owner-only. The session's first one is made with the host's default mode and read back
    /// (`0666` less the host's umask: what a new file gets there), then narrowed to 0600 before anything is written.
    async fn create_new(&self, path: &str) -> FsResult<File> {
        let c = &self.0;
        let flags = OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::EXCLUDE;
        let learned = c.new_mode.lock().is_some();
        let mut a = FileAttributes::empty();
        if learned {
            a.permissions = Some(0o600);
        }
        let f = c.sftp.open_with_flags_and_attributes(path, flags, a).await.map_err(|e| c.error(e))?;
        if !learned {
            if let Some(mode) = f.metadata().await.ok().and_then(|m| m.permissions) {
                *c.new_mode.lock() = Some(mode & 0o777);
            }
            let mut a = FileAttributes::empty();
            a.permissions = Some(0o600);
            f.set_metadata(a).await.map_err(|e| c.error(e))?;
        }
        Ok(f)
    }
}

/// A remote file; I/O errors become a lost connection when the ssh child ended.
struct SftpFile {
    f: File,
    conn: Arc<Conn>,
}

impl SftpFile {
    /// A lost connection comes out as `ConnectionReset` (an `ErrKind::Connection` for the copy); the host's own
    /// answers keep their kind.
    fn map(&self, r: io::Result<()>) -> io::Result<()> {
        r.map_err(|e| {
            let fs = self.conn.io_error(e);
            let kind = match fs.kind {
                ErrKind::Connection => io::ErrorKind::ConnectionReset,
                ErrKind::NotFound => io::ErrorKind::NotFound,
                ErrKind::Denied => io::ErrorKind::PermissionDenied,
                _ => io::ErrorKind::Other,
            };
            io::Error::new(kind, fs.message)
        })
    }
}

impl AsyncRead for SftpFile {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        match Pin::new(&mut me.f).poll_read(cx, buf) {
            Poll::Ready(r) => Poll::Ready(me.map(r)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for SftpFile {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let me = self.get_mut();
        match Pin::new(&mut me.f).poll_write(cx, data) {
            Poll::Ready(Ok(n)) => Poll::Ready(Ok(n)),
            Poll::Ready(Err(e)) => Poll::Ready(me.map(Err(e)).map(|_| 0)),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        match Pin::new(&mut me.f).poll_flush(cx) {
            Poll::Ready(r) => Poll::Ready(me.map(r)),
            Poll::Pending => Poll::Pending,
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let me = self.get_mut();
        match Pin::new(&mut me.f).poll_shutdown(cx) {
            Poll::Ready(r) => Poll::Ready(me.map(r)),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[async_trait]
impl WriteFile for SftpFile {
    async fn finish(self: Box<Self>, fsync: bool) -> FsResult<()> {
        let SftpFile { f, conn } = *self;
        if fsync {
            // Skipped (and reported as done) by the crate when the server has no `fsync@openssh.com`.
            f.sync_all().await.map_err(|e| conn.error(e))?;
        }
        f.close().await.map_err(|e| conn.io_error(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_attributes() {
        let mut a = FileAttributes::empty();
        a.permissions = Some(0o100640);
        a.size = Some(12);
        a.mtime = Some(1_700_000_000);
        a.uid = Some(1000);
        let m = meta_of(&a);
        assert_eq!((m.kind, m.size, m.mtime, m.mode, m.uid), (Kind::File, 12, 1_700_000_000, Some(0o640), Some(1000)));
        a.permissions = Some(0o040755);
        assert_eq!((meta_of(&a).kind, meta_of(&a).size), (Kind::Dir, 0));
        a.permissions = Some(0o120777);
        assert_eq!(meta_of(&a).kind, Kind::Link);
        assert_eq!(owner(&Some("deploy".into()), Some(1000)).as_deref(), Some("deploy"));
        assert_eq!(owner(&None, Some(1000)).as_deref(), Some("1000"));
        assert_eq!(owner(&Some(String::new()), None), None);
    }
}
