//! Files on this computer and on SSH hosts, for the Files tab and the transfer queue. The daemon owns them
//! so a transfer outlives the window and the phone can reach the same hosts.
//!
//! Every end of a transfer is a [`RemoteFs`]: [`local_fs::LocalFs`] for this computer, an SFTP session for a
//! host (spoken over the user's own OpenSSH, `ssh -F <generated config> -s <dest> sftp`, so their config,
//! ProxyJump, agent and Kelpie's saved passwords apply exactly as in a terminal), and an in-memory one for
//! tests. Paths are strings: absolute POSIX paths on a host, absolute OS paths here.

pub mod local_fs;
#[cfg(test)]
pub mod mem_fs;
pub mod names;

use std::fmt;

use async_trait::async_trait;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    File,
    Dir,
    Link,
    Other,
}

/// What `stat` / `lstat` tell about a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Meta {
    pub kind: Kind,
    pub size: u64,
    /// Seconds since the epoch (SFTP v3 has whole seconds only).
    pub mtime: u64,
    /// Permission bits (`0o7777`), when the end has them (not local Windows).
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
}

/// One name in a folder listing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Entry {
    pub name: String,
    #[serde(flatten)]
    pub meta: Meta,
    /// From the listing's long name (`ls -l` form), best effort.
    pub owner: Option<String>,
    pub group: Option<String>,
    pub link_target: Option<String>,
    /// False when the host's bytes are not UTF-8: such a name cannot be named back to the host, so it is
    /// shown but never selected or copied.
    pub readable_name: bool,
    /// A dot name, or (this computer, Windows) the hidden attribute: shown only when the pane asks.
    pub hidden: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrKind {
    NotFound,
    Denied,
    Exists,
    /// The ssh connection failed or ended (exit 255, EOF on the SFTP stream).
    Connection,
    /// The end cannot do it (no `posix-rename`, chmod on Windows).
    Unsupported,
    Invalid,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsError {
    pub kind: ErrKind,
    pub message: String,
}

impl FsError {
    pub fn new(kind: ErrKind, message: impl Into<String>) -> Self {
        FsError { kind, message: message.into() }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrKind::Invalid, message)
    }
}

impl fmt::Display for FsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FsError {}

impl From<std::io::Error> for FsError {
    fn from(e: std::io::Error) -> Self {
        use std::io::ErrorKind as K;
        let kind = match e.kind() {
            K::NotFound => ErrKind::NotFound,
            K::PermissionDenied => ErrKind::Denied,
            K::AlreadyExists => ErrKind::Exists,
            K::BrokenPipe | K::UnexpectedEof | K::ConnectionReset | K::ConnectionAborted => ErrKind::Connection,
            K::Unsupported => ErrKind::Unsupported,
            K::InvalidInput => ErrKind::Invalid,
            _ => ErrKind::Other,
        };
        FsError { kind, message: e.to_string() }
    }
}

pub type FsResult<T> = Result<T, FsError>;

/// An SFTP status (draft-ietf-secsh-filexfer-02 codes) as a plain message. `server` is the server's own text,
/// kept when it says more than the code.
pub fn sftp_status(code: u32, server: &str) -> FsError {
    let (kind, text) = match code {
        2 => (ErrKind::NotFound, "no such file or folder"),
        3 => (ErrKind::Denied, "permission denied"),
        4 => (ErrKind::Other, "the host refused the operation"),
        5 => (ErrKind::Other, "the host did not understand the request"),
        6 | 7 => (ErrKind::Connection, "the connection to the host was lost"),
        8 => (ErrKind::Unsupported, "the host does not support this"),
        _ => (ErrKind::Other, "the host reported an error"),
    };
    let server = server.trim();
    if server.is_empty() || server.eq_ignore_ascii_case(text) || matches!(server.to_ascii_lowercase().as_str(), "failure" | "no such file" | "permission denied") {
        FsError::new(kind, text)
    } else {
        FsError::new(kind, format!("{text}: {server}"))
    }
}

/// A file being written; `finish` closes it and reports what the close found (an SFTP close can fail).
#[async_trait]
pub trait WriteFile: AsyncWrite + Send + Unpin {
    async fn finish(self: Box<Self>, fsync: bool) -> FsResult<()>;
}

/// One end of a listing or a transfer.
#[async_trait]
pub trait RemoteFs: Send + Sync {
    async fn list(&self, dir: &str) -> FsResult<Vec<Entry>>;
    async fn stat(&self, path: &str) -> FsResult<Meta>;
    async fn lstat(&self, path: &str) -> FsResult<Meta>;
    async fn readlink(&self, path: &str) -> FsResult<String>;
    /// `.` (or `~`) is the login folder on a host, the home folder here.
    async fn realpath(&self, path: &str) -> FsResult<String>;
    async fn mkdir(&self, path: &str) -> FsResult<()>;
    /// With `overwrite` false an existing `to` is refused; with true it is replaced in one step where the end
    /// can (`posix-rename`), else `Unsupported` (the caller then moves the old file aside first).
    async fn rename(&self, from: &str, to: &str, overwrite: bool) -> FsResult<()>;
    async fn remove_file(&self, path: &str) -> FsResult<()>;
    async fn remove_dir(&self, path: &str) -> FsResult<()>;
    async fn chmod(&self, path: &str, mode: u32) -> FsResult<()>;
    async fn set_times(&self, path: &str, atime: u64, mtime: u64) -> FsResult<()>;
    async fn open_read(&self, path: &str, offset: u64) -> FsResult<Box<dyn AsyncRead + Send + Unpin>>;
    /// Opens for writing at `offset`; anything past `offset` is cut off (a resume continues from what is
    /// kept, a restart passes 0). `create` makes a missing file.
    async fn open_write(&self, path: &str, offset: u64, create: bool) -> FsResult<Box<dyn WriteFile>>;
    /// Free bytes on the file system holding `path`, when the end can tell.
    async fn free_space(&self, path: &str) -> FsResult<Option<u64>>;
}

/// The kind in an SFTP / POSIX mode (`S_IFMT`).
pub fn kind_from_mode(mode: u32) -> Kind {
    match mode & 0o170000 {
        0o040000 => Kind::Dir,
        0o100000 => Kind::File,
        0o120000 => Kind::Link,
        _ => Kind::Other,
    }
}

/// Owner and group from an `ls -l` style long name (`-rw-r--r--  1 deploy staff 1234 Jan  1 12:00 x`, GNU and
/// BSD forms). Anything else gives `None`: servers may send any text there.
pub fn parse_longname(line: &str) -> Option<(String, String)> {
    let mut words = line.split_whitespace();
    let perms = words.next()?;
    let links = words.next()?;
    let owner = words.next()?;
    let group = words.next()?;
    let size = words.next()?;
    let perms_ok = perms.is_ascii() && perms.len() >= 10 && "-dlcbpsD".contains(&perms[..1]) && perms[1..10].chars().all(|c| "rwxsStTl-".contains(c));
    (perms_ok && links.chars().all(|c| c.is_ascii_digit()) && size.chars().all(|c| c.is_ascii_digit() || c == ',')).then(|| (owner.to_string(), group.to_string()))
}

/// Whether a host name came through intact: the SFTP layer replaces bytes that are not UTF-8 with U+FFFD.
pub fn readable_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('\u{FFFD}') && !name.contains('/') && name != "." && name != ".."
}

/// A host path Kelpie may hand to the SFTP server: absolute, without control characters.
pub fn check_remote_path(path: &str) -> FsResult<()> {
    if !path.starts_with('/') {
        return Err(FsError::invalid(format!("a path on the host must start with /: {path}")));
    }
    if path.chars().any(char::is_control) {
        return Err(FsError::invalid("a path on the host cannot contain control characters"));
    }
    Ok(())
}

/// A path on this computer Kelpie may use: absolute (or `""` for the list of drives on Windows).
pub fn check_local_path(path: &str) -> FsResult<()> {
    if path.is_empty() && cfg!(windows) {
        return Ok(());
    }
    if !std::path::Path::new(path).is_absolute() {
        return Err(FsError::invalid(format!("a path on this computer must be absolute: {path}")));
    }
    if path.chars().any(char::is_control) {
        return Err(FsError::invalid("a path cannot contain control characters"));
    }
    Ok(())
}

/// `dir` + `/` + `name` on a host.
pub fn join_remote(dir: &str, name: &str) -> String {
    let dir = dir.trim_end_matches('/');
    format!("{dir}/{name}")
}

/// The folder holding a host path (`/` for a top-level name).
pub fn parent_remote(path: &str) -> &str {
    match path.trim_end_matches('/').rfind('/') {
        Some(0) | None => "/",
        Some(i) => &path[..i],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_modes() {
        assert_eq!(kind_from_mode(0o100644), Kind::File);
        assert_eq!(kind_from_mode(0o040755), Kind::Dir);
        assert_eq!(kind_from_mode(0o120777), Kind::Link);
        assert_eq!(kind_from_mode(0o020666), Kind::Other);
        assert_eq!(kind_from_mode(0o644), Kind::Other);
    }

    #[test]
    fn long_names() {
        let gnu = "-rw-r--r--    1 deploy   www-data     1234 Jan  1 12:00 index.html";
        assert_eq!(parse_longname(gnu), Some(("deploy".into(), "www-data".into())));
        let bsd = "drwxr-xr-x   12 me  staff  384 Nov  6 01:03 Projects";
        assert_eq!(parse_longname(bsd), Some(("me".into(), "staff".into())));
        let numeric = "lrwxrwxrwx 1 1000 1000 7 Mar  3  2024 link -> target";
        assert_eq!(parse_longname(numeric), Some(("1000".into(), "1000".into())));
        let sticky = "drwxrwxrwt 10 root root 4096 Nov  6 01:03 tmp";
        assert_eq!(parse_longname(sticky), Some(("root".into(), "root".into())));
        for garbage in ["", "index.html", "-rw-r--r-- x deploy staff 12", "hello world this is not ls", "-rw 1 a b 2", "ürw-r--r-- 1 a b 2"] {
            assert_eq!(parse_longname(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn names_that_came_through() {
        assert!(readable_name("report.pdf"));
        assert!(readable_name("ünïcødé 日本語"));
        assert!(!readable_name("bad\u{FFFD}name"));
        assert!(!readable_name(""));
        assert!(!readable_name(".."));
        assert!(!readable_name("a/b"));
    }

    #[test]
    fn host_paths() {
        assert!(check_remote_path("/home/me").is_ok());
        assert_eq!(check_remote_path("home/me").unwrap_err().kind, ErrKind::Invalid);
        assert!(check_remote_path("/home/me\nx").is_err());
        assert!(check_remote_path("/tmp/\u{7f}").is_err());
        assert_eq!(join_remote("/", "etc"), "/etc");
        assert_eq!(join_remote("/home/me/", "a b"), "/home/me/a b");
        assert_eq!(parent_remote("/home/me/x"), "/home/me");
        assert_eq!(parent_remote("/etc"), "/");
        assert_eq!(parent_remote("/"), "/");
    }

    #[test]
    fn local_paths() {
        assert!(check_local_path("relative/x").is_err());
        if cfg!(windows) {
            assert!(check_local_path("").is_ok());
            assert!(check_local_path(r"C:\Users").is_ok());
        } else {
            assert!(check_local_path("").is_err());
            assert!(check_local_path("/home").is_ok());
        }
    }

    #[test]
    fn sftp_statuses() {
        assert_eq!(sftp_status(2, "No such file"), FsError::new(ErrKind::NotFound, "no such file or folder"));
        assert_eq!(sftp_status(3, "").kind, ErrKind::Denied);
        assert_eq!(sftp_status(4, "Failure").message, "the host refused the operation");
        assert_eq!(sftp_status(4, "disk quota exceeded").message, "the host refused the operation: disk quota exceeded");
        assert_eq!(sftp_status(7, "").kind, ErrKind::Connection);
        assert_eq!(sftp_status(8, "").kind, ErrKind::Unsupported);
    }
}
