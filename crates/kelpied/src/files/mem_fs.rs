//! An in-memory host for the transfer tests: folders, files, links, an optional `posix-rename`, a free-space
//! figure, and reads that fail after a number of bytes (a dropped connection).

use std::collections::BTreeMap;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use super::{join_remote, parent_remote, readable_name, Entry, ErrKind, FsError, FsResult, Kind, Meta, RemoteFs, WriteFile};

#[derive(Clone, Debug)]
enum Body {
    Dir,
    File(Vec<u8>),
    Link(String),
}

#[derive(Clone, Debug)]
struct Node {
    body: Body,
    mtime: u64,
    mode: u32,
}

struct State {
    nodes: BTreeMap<String, Node>,
    posix_rename: bool,
    free: Option<u64>,
    fail_reads_after: Option<u64>,
    now: u64,
}

#[derive(Clone)]
pub struct MemFs(Arc<Mutex<State>>);

pub const HOME: &str = "/home/test";

fn not_found(p: &str) -> FsError {
    FsError::new(ErrKind::NotFound, format!("no such file or folder: {p}"))
}

impl Default for MemFs {
    fn default() -> Self {
        Self::new()
    }
}

impl MemFs {
    pub fn new() -> Self {
        let fs = MemFs(Arc::new(Mutex::new(State { nodes: BTreeMap::new(), posix_rename: true, free: None, fail_reads_after: None, now: 1_700_000_000 })));
        fs.0.lock().nodes.insert("/".into(), Node { body: Body::Dir, mtime: 0, mode: 0o755 });
        fs.mkdir_all(HOME);
        fs
    }

    pub fn without_posix_rename(self) -> Self {
        self.0.lock().posix_rename = false;
        self
    }

    pub fn set_free(&self, free: Option<u64>) {
        self.0.lock().free = free;
    }

    /// Every reader opened from now on fails after `n` bytes; `None` heals.
    pub fn fail_reads_after(&self, n: Option<u64>) {
        self.0.lock().fail_reads_after = n;
    }

    pub fn mkdir_all(&self, path: &str) {
        let mut s = self.0.lock();
        let mut cur = String::new();
        for part in path.split('/').filter(|p| !p.is_empty()) {
            cur = format!("{cur}/{part}");
            let now = s.now;
            s.nodes.entry(cur.clone()).or_insert(Node { body: Body::Dir, mtime: now, mode: 0o755 });
        }
    }

    pub fn put(&self, path: &str, bytes: &[u8], mtime: u64) {
        self.mkdir_all(parent_remote(path));
        self.0.lock().nodes.insert(path.into(), Node { body: Body::File(bytes.to_vec()), mtime, mode: 0o644 });
    }

    pub fn link(&self, path: &str, target: &str) {
        self.0.lock().nodes.insert(path.into(), Node { body: Body::Link(target.into()), mtime: 0, mode: 0o777 });
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        match &self.0.lock().nodes.get(path)?.body {
            Body::File(b) => Some(b.clone()),
            _ => None,
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        self.0.lock().nodes.contains_key(path)
    }

    pub fn paths(&self) -> Vec<String> {
        self.0.lock().nodes.keys().cloned().collect()
    }

    fn resolve(s: &State, path: &str) -> FsResult<String> {
        let mut p = path.to_string();
        for _ in 0..8 {
            match &s.nodes.get(&p).ok_or_else(|| not_found(path))?.body {
                Body::Link(t) => p = if t.starts_with('/') { t.clone() } else { join_remote(parent_remote(&p), t) },
                _ => return Ok(p),
            }
        }
        Err(FsError::new(ErrKind::Other, "too many levels of links"))
    }

    fn meta(n: &Node) -> Meta {
        let (kind, size) = match &n.body {
            Body::Dir => (Kind::Dir, 0),
            Body::File(b) => (Kind::File, b.len() as u64),
            Body::Link(t) => (Kind::Link, t.len() as u64),
        };
        Meta { kind, size, mtime: n.mtime, mode: Some(n.mode), uid: Some(1000), gid: Some(1000) }
    }
}

#[async_trait]
impl RemoteFs for MemFs {
    async fn list(&self, dir: &str) -> FsResult<Vec<Entry>> {
        let s = self.0.lock();
        let dir = MemFs::resolve(&s, dir)?;
        if !matches!(s.nodes[&dir].body, Body::Dir) {
            return Err(FsError::new(ErrKind::Other, "not a folder"));
        }
        Ok(s
            .nodes
            .iter()
            .filter(|(p, _)| p.as_str() != "/" && parent_remote(p) == dir && p.as_str() != dir)
            .map(|(p, n)| {
                let name = p.rsplit('/').next().unwrap_or_default().to_string();
                Entry {
                    readable_name: readable_name(&name),
                    hidden: name.starts_with('.'),
                    link_target: match &n.body {
                        Body::Link(t) => Some(t.clone()),
                        _ => None,
                    },
                    name,
                    meta: MemFs::meta(n),
                    owner: Some("test".into()),
                    group: Some("test".into()),
                }
            })
            .collect())
    }

    async fn stat(&self, path: &str) -> FsResult<Meta> {
        let s = self.0.lock();
        let p = MemFs::resolve(&s, path)?;
        Ok(MemFs::meta(&s.nodes[&p]))
    }

    async fn lstat(&self, path: &str) -> FsResult<Meta> {
        self.0.lock().nodes.get(path).map(MemFs::meta).ok_or_else(|| not_found(path))
    }

    async fn readlink(&self, path: &str) -> FsResult<String> {
        match &self.0.lock().nodes.get(path).ok_or_else(|| not_found(path))?.body {
            Body::Link(t) => Ok(t.clone()),
            _ => Err(FsError::invalid("not a link")),
        }
    }

    async fn realpath(&self, path: &str) -> FsResult<String> {
        if matches!(path, "" | "." | "~") {
            return Ok(HOME.into());
        }
        MemFs::resolve(&self.0.lock(), path)
    }

    async fn mkdir(&self, path: &str) -> FsResult<()> {
        let mut s = self.0.lock();
        if s.nodes.contains_key(path) {
            return Err(FsError::new(ErrKind::Exists, format!("{path} already exists")));
        }
        if !matches!(s.nodes.get(parent_remote(path)).map(|n| &n.body), Some(Body::Dir)) {
            return Err(not_found(parent_remote(path)));
        }
        let now = s.now;
        s.nodes.insert(path.into(), Node { body: Body::Dir, mtime: now, mode: 0o755 });
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()> {
        let mut s = self.0.lock();
        if !s.nodes.contains_key(from) {
            return Err(not_found(from));
        }
        if s.nodes.contains_key(to) {
            if !overwrite {
                return Err(FsError::new(ErrKind::Exists, format!("{to} already exists")));
            }
            if !s.posix_rename {
                return Err(FsError::new(ErrKind::Unsupported, "the host cannot replace a file in one step"));
            }
        }
        let moved: Vec<String> = s.nodes.keys().filter(|k| k.as_str() == from || k.starts_with(&format!("{from}/"))).cloned().collect();
        for k in moved {
            let n = s.nodes.remove(&k).unwrap();
            s.nodes.insert(format!("{to}{}", &k[from.len()..]), n);
        }
        Ok(())
    }

    async fn remove_file(&self, path: &str) -> FsResult<()> {
        let mut s = self.0.lock();
        match s.nodes.get(path).map(|n| &n.body) {
            None => Err(not_found(path)),
            Some(Body::Dir) => Err(FsError::invalid("is a folder")),
            Some(_) => {
                s.nodes.remove(path);
                Ok(())
            }
        }
    }

    async fn remove_dir(&self, path: &str) -> FsResult<()> {
        let mut s = self.0.lock();
        if !matches!(s.nodes.get(path).map(|n| &n.body), Some(Body::Dir)) {
            return Err(not_found(path));
        }
        if s.nodes.keys().any(|k| k.starts_with(&format!("{path}/"))) {
            return Err(FsError::new(ErrKind::Other, "the folder is not empty"));
        }
        s.nodes.remove(path);
        Ok(())
    }

    async fn chmod(&self, path: &str, mode: u32) -> FsResult<()> {
        let mut s = self.0.lock();
        let p = MemFs::resolve(&s, path)?;
        s.nodes.get_mut(&p).unwrap().mode = mode & 0o7777;
        Ok(())
    }

    async fn set_times(&self, path: &str, _atime: u64, mtime: u64) -> FsResult<()> {
        let mut s = self.0.lock();
        let p = MemFs::resolve(&s, path)?;
        s.nodes.get_mut(&p).unwrap().mtime = mtime;
        Ok(())
    }

    async fn open_read(&self, path: &str, offset: u64) -> FsResult<Box<dyn AsyncRead + Send + Unpin>> {
        let s = self.0.lock();
        let p = MemFs::resolve(&s, path)?;
        let Body::File(b) = &s.nodes[&p].body else { return Err(FsError::invalid("not a file")) };
        let start = (offset as usize).min(b.len());
        Ok(Box::new(FailAfter { data: b[start..].to_vec(), pos: 0, fail_after: s.fail_reads_after }))
    }

    async fn open_write(&self, path: &str, offset: u64, create: bool) -> FsResult<Box<dyn WriteFile>> {
        let mut s = self.0.lock();
        if !matches!(s.nodes.get(parent_remote(path)).map(|n| &n.body), Some(Body::Dir)) {
            return Err(not_found(parent_remote(path)));
        }
        let now = s.now;
        match s.nodes.get_mut(path) {
            Some(Node { body: Body::File(b), mtime, .. }) => {
                b.truncate(offset as usize);
                *mtime = now;
            }
            Some(_) => return Err(FsError::invalid("not a file")),
            None if create => {
                s.nodes.insert(path.into(), Node { body: Body::File(Vec::new()), mtime: now, mode: 0o644 });
            }
            None => return Err(not_found(path)),
        }
        Ok(Box::new(MemWrite { fs: self.clone(), path: path.into(), pos: offset as usize }))
    }

    async fn free_space(&self, _path: &str) -> FsResult<Option<u64>> {
        Ok(self.0.lock().free)
    }
}

struct FailAfter {
    data: Vec<u8>,
    pos: usize,
    fail_after: Option<u64>,
}

impl AsyncRead for FailAfter {
    fn poll_read(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let me = &mut *self;
        let mut end = me.data.len();
        if let Some(limit) = me.fail_after {
            if me.pos as u64 >= limit {
                return Poll::Ready(Err(io::Error::new(io::ErrorKind::ConnectionReset, "connection lost")));
            }
            end = end.min(limit as usize);
        }
        let n = (end - me.pos).min(buf.remaining());
        buf.put_slice(&me.data[me.pos..me.pos + n]);
        me.pos += n;
        Poll::Ready(Ok(()))
    }
}

struct MemWrite {
    fs: MemFs,
    path: String,
    pos: usize,
}

impl AsyncWrite for MemWrite {
    fn poll_write(mut self: Pin<&mut Self>, _cx: &mut Context<'_>, data: &[u8]) -> Poll<io::Result<usize>> {
        let me = &mut *self;
        let mut s = me.fs.0.lock();
        let Some(Node { body: Body::File(b), .. }) = s.nodes.get_mut(&me.path) else {
            return Poll::Ready(Err(io::Error::new(io::ErrorKind::NotFound, "the file went away")));
        };
        if b.len() < me.pos {
            b.resize(me.pos, 0);
        }
        let overlap = (b.len() - me.pos).min(data.len());
        b[me.pos..me.pos + overlap].copy_from_slice(&data[..overlap]);
        b.extend_from_slice(&data[overlap..]);
        me.pos += data.len();
        Poll::Ready(Ok(data.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

#[async_trait]
impl WriteFile for MemWrite {
    async fn finish(self: Box<Self>, _fsync: bool) -> FsResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn behaves_like_a_host() {
        let fs = MemFs::new();
        fs.put("/srv/a.txt", b"hello", 100);
        fs.link("/srv/to-a", "a.txt");
        fs.link("/srv/to-srv", "/srv");
        let mut names: Vec<String> = fs.list("/srv").await.unwrap().into_iter().map(|e| e.name).collect();
        names.sort();
        assert_eq!(names, ["a.txt", "to-a", "to-srv"]);
        assert_eq!(fs.stat("/srv/to-a").await.unwrap().kind, Kind::File);
        assert_eq!(fs.lstat("/srv/to-a").await.unwrap().kind, Kind::Link);
        assert_eq!(fs.realpath("/srv/to-srv").await.unwrap(), "/srv");
        assert_eq!(fs.realpath(".").await.unwrap(), HOME);

        let mut w = fs.open_write("/srv/b", 0, true).await.unwrap();
        w.write_all(b"abcdef").await.unwrap();
        w.finish(false).await.unwrap();
        let mut w = fs.open_write("/srv/b", 3, false).await.unwrap();
        w.write_all(b"XY").await.unwrap();
        w.finish(false).await.unwrap();
        assert_eq!(fs.read("/srv/b").unwrap(), b"abcXY");

        fs.fail_reads_after(Some(2));
        let mut r = fs.open_read("/srv/b", 0).await.unwrap();
        let mut buf = Vec::new();
        assert_eq!(r.read_to_end(&mut buf).await.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(buf, b"ab");

        assert_eq!(fs.rename("/srv/b", "/srv/a.txt", false).await.unwrap_err().kind, ErrKind::Exists);
        let strict = MemFs::new().without_posix_rename();
        strict.put("/x", b"1", 0);
        strict.put("/y", b"2", 0);
        assert_eq!(strict.rename("/x", "/y", true).await.unwrap_err().kind, ErrKind::Unsupported);
        assert!(fs.remove_dir("/srv").await.is_err());
    }
}
