//! SFTP sessions over the user's own OpenSSH: `ssh -F <generated config> ... -s <dest> sftp`, with Kelpie's
//! askpass, so saved passwords, ProxyJump, the agent and the user's config apply exactly as in a terminal.
//!
//! One browse session per destination, opened on first use and closed after 5 minutes without use; transfer
//! jobs open their own sessions so a listing never waits behind a large copy. A session whose ssh ended is
//! replaced on the next call, never in the middle of one. The children are `kill_on_drop`; the spike showed ssh
//! (and a ProxyJump's inner ssh) also exit by themselves when the daemon dies and their pipes close.

use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use russh_sftp::client::{Config, SftpSession};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::{ErrKind, FsError, FsResult};

/// Requests kept in flight per file (the sftp CLI's default; the spike's 64 kept up with it).
const IN_FLIGHT: usize = 64;
const IDLE: Duration = Duration::from_secs(5 * 60);
/// ssh's stderr kept for the error text.
const STDERR_KEEP: usize = 16 * 1024;

/// One ssh child speaking SFTP.
pub struct Conn {
    pub sftp: SftpSession,
    pub dest: String,
    child: Mutex<Child>,
    stderr: Arc<Mutex<Vec<u8>>>,
    last_used: Mutex<Instant>,
}

/// The child's stdout and stdin as the one stream russh-sftp speaks over.
struct Stdio2 {
    r: ChildStdout,
    w: ChildStdin,
}

impl AsyncRead for Stdio2 {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().r).poll_read(cx, buf)
    }
}

impl AsyncWrite for Stdio2 {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().w).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().w).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().w).poll_shutdown(cx)
    }
}

/// The arguments of the ssh child for `dest` (after the program).
pub fn ssh_args(config: &std::path::Path, dest: &str) -> Vec<String> {
    let mut args = vec!["-F".to_string(), config.to_string_lossy().into_owned()];
    for o in ["NumberOfPasswordPrompts=1", "ConnectTimeout=15", "ServerAliveInterval=15", "ServerAliveCountMax=3", "LogLevel=ERROR"] {
        args.push("-o".into());
        args.push(o.into());
    }
    args.extend(["-s".to_string(), dest.to_string(), "sftp".to_string()]);
    args
}

impl Conn {
    /// Starts ssh for `dest` and the SFTP handshake. `exe` is this daemon (ssh's askpass).
    pub async fn open(dest: &str, exe: &std::path::Path) -> FsResult<Arc<Conn>> {
        if !kelpie_proto::ssh::valid_dest(dest) {
            return Err(FsError::invalid(format!("invalid ssh destination `{dest}`")));
        }
        let mut cmd = tokio::process::Command::new(kelpie_proto::ssh::program("ssh"));
        cmd.args(ssh_args(&kelpie_proto::ssh::config_path(), dest))
            .envs(kelpie_proto::ssh::askpass_env(exe, &kelpie_proto::ssh::host_id_for_alias(dest).unwrap_or_default()))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == io::ErrorKind::NotFound {
                FsError::new(ErrKind::Other, "ssh is not installed (OpenSSH client)")
            } else {
                FsError::new(ErrKind::Other, format!("ssh: {e}"))
            }
        })?;
        let stderr = Arc::new(Mutex::new(Vec::new()));
        if let Some(mut err) = child.stderr.take() {
            let keep = stderr.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok(n) = err.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut k = keep.lock();
                    let room = STDERR_KEEP.saturating_sub(k.len());
                    k.extend_from_slice(&buf[..n.min(room)]);
                }
            });
        }
        let (w, r) = (child.stdin.take().expect("piped stdin"), child.stdout.take().expect("piped stdout"));
        let cfg = Config { max_concurrent_reads: IN_FLIGHT, max_concurrent_writes: IN_FLIGHT, request_timeout_secs: 30, ..Default::default() };
        let started = tokio::time::timeout(Duration::from_secs(40), SftpSession::new_with_config(Stdio2 { r, w }, cfg)).await;
        match started {
            Ok(Ok(sftp)) => Ok(Arc::new(Conn { sftp, dest: dest.to_string(), child: Mutex::new(child), stderr, last_used: Mutex::new(Instant::now()) })),
            failed => {
                // ssh's own words (auth, host key) arrive just before it exits.
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
                let text = String::from_utf8_lossy(&stderr.lock()).trim().to_string();
                Err(match (failed, text.is_empty()) {
                    (_, false) => FsError::new(ErrKind::Connection, format!("ssh {dest}: {text}")),
                    (Err(_), true) => FsError::new(ErrKind::Connection, format!("{dest} did not answer in time")),
                    (Ok(Err(e)), true) => FsError::new(ErrKind::Connection, format!("ssh {dest}: the SFTP service did not start ({e})")),
                    (Ok(Ok(_)), true) => unreachable!(),
                })
            }
        }
    }

    /// Whether the ssh child still runs.
    pub fn alive(&self) -> bool {
        matches!(self.child.lock().try_wait(), Ok(None))
    }

    pub fn touch(&self) {
        *self.last_used.lock() = Instant::now();
    }

    fn idle_for(&self) -> Duration {
        self.last_used.lock().elapsed()
    }

    /// What ssh said, for a connection that ended.
    pub fn stderr_text(&self) -> String {
        String::from_utf8_lossy(&self.stderr.lock()).trim().to_string()
    }

    /// An SFTP error as a plain message. Only a status is the host's answer; everything else (a timeout, a closed
    /// stream, a packet that makes no sense) means the session is gone, even before ssh is seen to exit: a spurious
    /// retry costs little, a lost connection taken for a file error loses the resume.
    pub fn error(&self, e: russh_sftp::client::error::Error) -> FsError {
        classify(e, || if self.alive() { None } else { Some(self.stderr_text()) }, &self.dest)
    }

    /// An I/O error from a file on this connection: a status inside it is the host's answer, anything else a lost
    /// connection.
    pub fn io_error(&self, e: io::Error) -> FsError {
        io_classify(e, || if self.alive() { None } else { Some(self.stderr_text()) }, &self.dest)
    }
}

/// See [`Conn::error`]; `ended` gives ssh's words once it has exited.
pub fn classify(e: russh_sftp::client::error::Error, ended: impl Fn() -> Option<String>, dest: &str) -> FsError {
    use russh_sftp::client::error::Error as E;
    match e {
        E::Status(s) => super::sftp_status(status_number(&s.status_code), &s.error_message),
        E::Timeout => FsError::new(ErrKind::Connection, format!("{dest} did not answer in time")),
        _ => lost_text(ended(), dest),
    }
}

/// See [`Conn::io_error`].
pub fn io_classify(e: io::Error, ended: impl Fn() -> Option<String>, dest: &str) -> FsError {
    if let Some(inner) = e.get_ref().and_then(|i| i.downcast_ref::<russh_sftp::client::error::Error>()) {
        if let russh_sftp::client::error::Error::Status(s) = inner {
            return super::sftp_status(status_number(&s.status_code), &s.error_message);
        }
    }
    match e.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidInput => FsError::from(e),
        _ => lost_text(ended(), dest),
    }
}

/// A lost connection, in the words `remote::exec` uses (so the app's host-key check still recognises it).
fn lost_text(stderr: Option<String>, dest: &str) -> FsError {
    match stderr.filter(|s| !s.is_empty()) {
        Some(text) => FsError::new(ErrKind::Connection, format!("ssh {dest}: {text}")),
        None => FsError::new(ErrKind::Connection, format!("ssh {dest}: the connection ended")),
    }
}

fn status_number(code: &russh_sftp::protocol::StatusCode) -> u32 {
    use russh_sftp::protocol::StatusCode as S;
    match code {
        S::Ok => 0,
        S::Eof => 1,
        S::NoSuchFile => 2,
        S::PermissionDenied => 3,
        S::Failure => 4,
        S::BadMessage => 5,
        S::NoConnection => 6,
        S::ConnectionLost => 7,
        S::OpUnsupported => 8,
    }
}

/// The browse sessions, one per destination.
pub struct Pool {
    exe: PathBuf,
    browse: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Option<Arc<Conn>>>>>>,
    reaper: std::sync::Once,
}

impl Pool {
    pub fn new(exe: PathBuf) -> Self {
        Pool { exe, browse: Mutex::new(HashMap::new()), reaper: std::sync::Once::new() }
    }

    /// The browse session for `dest`, started (or restarted after its ssh ended) when needed.
    pub async fn browse(self: &Arc<Self>, dest: &str) -> FsResult<Arc<Conn>> {
        self.start_reaper();
        let slot = self.browse.lock().entry(dest.to_string()).or_default().clone();
        let mut held = slot.lock().await;
        if let Some(c) = held.as_ref().filter(|c| c.alive()) {
            c.touch();
            return Ok(c.clone());
        }
        let c = Conn::open(dest, &self.exe).await?;
        *held = Some(c.clone());
        Ok(c)
    }

    /// A session of its own for a transfer job; it closes when the job drops it.
    pub async fn transfer(&self, dest: &str) -> FsResult<Arc<Conn>> {
        Conn::open(dest, &self.exe).await
    }

    /// Closes browse sessions unused for 5 minutes, and those whose ssh ended.
    pub fn reap(&self) {
        let slots: Vec<_> = self.browse.lock().values().cloned().collect();
        for slot in slots {
            if let Ok(mut held) = slot.try_lock() {
                if held.as_ref().is_some_and(|c| !c.alive() || c.idle_for() > IDLE) {
                    *held = None;
                }
            }
        }
    }

    /// Ends every browse session (daemon shutdown).
    pub fn close_all(&self) {
        self.browse.lock().clear();
    }

    fn start_reaper(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        self.reaper.call_once(|| {
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    match me.upgrade() {
                        Some(p) => p.reap(),
                        None => return,
                    }
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_args_are_the_terminal_ones_plus_the_subsystem() {
        let args = ssh_args(std::path::Path::new("/d/ssh/config"), "deploy@prod");
        assert_eq!(args[..2], ["-F", "/d/ssh/config"]);
        assert!(args.windows(2).any(|w| w == ["-o", "NumberOfPasswordPrompts=1"]));
        assert!(args.windows(2).any(|w| w == ["-o", "ServerAliveInterval=15"]));
        assert_eq!(args[args.len() - 3..], ["-s", "deploy@prod", "sftp"]);
    }

    #[tokio::test]
    async fn refuses_a_destination_ssh_could_read_as_an_option() {
        let err = Conn::open("-oProxyCommand=calc", std::path::Path::new("kelpied")).await.err().unwrap();
        assert_eq!(err.kind, ErrKind::Invalid);
    }

    #[test]
    fn only_a_status_is_the_hosts_answer_everything_else_is_a_lost_connection() {
        use russh_sftp::client::error::Error as E;
        use russh_sftp::protocol::{Status, StatusCode};
        let status = || E::Status(Status { id: 1, status_code: StatusCode::PermissionDenied, error_message: "Permission denied".into(), language_tag: String::new() });
        let alive = || None;
        assert_eq!(classify(status(), alive, "prod").kind, ErrKind::Denied);
        for e in [E::UnexpectedBehavior("RecvError".into()), E::IO("broken pipe".into()), E::UnexpectedPacket, E::Timeout] {
            assert_eq!(classify(e, alive, "prod").kind, ErrKind::Connection);
        }
        assert_eq!(classify(E::IO("x".into()), || Some("Connection reset by peer".into()), "prod").message, "ssh prod: Connection reset by peer");
        // Inside an I/O error (what a file's reads and writes return).
        assert_eq!(io_classify(io::Error::other(status()), alive, "prod").kind, ErrKind::Denied);
        assert_eq!(io_classify(io::Error::other(E::UnexpectedBehavior("sender dropped".into())), alive, "prod").kind, ErrKind::Connection);
        assert_eq!(io_classify(io::Error::new(io::ErrorKind::BrokenPipe, "pipe"), alive, "prod").kind, ErrKind::Connection);
    }

    #[test]
    fn status_numbers_follow_the_protocol() {
        use russh_sftp::protocol::StatusCode as S;
        assert_eq!(status_number(&S::NoSuchFile), 2);
        assert_eq!(status_number(&S::OpUnsupported), 8);
    }
}
