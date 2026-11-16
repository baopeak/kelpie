//! The daemon's file methods: browsing a host (`sftp.*`) or this computer (`local.*`), and the transfer
//! queue (`xfer.*`). The app and (later) the phone call them; programs in sessions cannot (see `auth`).

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use super::local_fs::LocalFs;
use super::pool::Pool;
use super::queue::Queue;
use super::Emit;
use super::sftp_fs::SftpFs;
use super::{check_local_path, check_remote_path, Entry, ErrKind, FsError, FsResult, Meta, RemoteFs};

/// Paths one `sftp.stat_many` may ask about (a remote project's open folders and files).
pub const STAT_MANY_MAX: usize = 512;

/// The paths of `sftp.stat_many`, in order: one that is not a valid host path is `None` (answered null), so one odd
/// name never stops the rest; anything but text, or more than [`STAT_MANY_MAX`], refuses the call.
pub fn stat_targets(p: &Value) -> Result<Vec<Option<String>>> {
    let paths = p.get("paths").and_then(Value::as_array).ok_or_else(|| anyhow!("missing param `paths`"))?;
    if paths.len() > STAT_MANY_MAX {
        bail!("at most {STAT_MANY_MAX} paths at once");
    }
    paths
        .iter()
        .map(|v| {
            let path = v.as_str().ok_or_else(|| anyhow!("`paths` holds text only"))?;
            Ok(check_remote_path(path).ok().map(|_| path.to_string()))
        })
        .collect()
}

/// The answers of `sftp.stat_many`, in order: a path that cannot be read (gone, not allowed) is `None`, but a lost
/// connection fails the whole call, so a host that dropped never looks like everything was deleted.
pub fn fold_stats(results: Vec<FsResult<Meta>>) -> Result<Vec<Option<Meta>>, FsError> {
    results
        .into_iter()
        .map(|r| match r {
            Ok(m) => Ok(Some(m)),
            Err(e) if e.kind == ErrKind::Connection => Err(e),
            Err(_) => Ok(None),
        })
        .collect()
}

pub struct Files {
    pub pool: Arc<Pool>,
    pub queue: Arc<Queue>,
}

fn s<'a>(p: &'a Value, key: &str) -> Result<&'a str> {
    p.get(key).and_then(Value::as_str).ok_or_else(|| anyhow!("missing param `{key}`"))
}

fn flag(p: &Value, key: &str) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn fs_err(e: FsError) -> anyhow::Error {
    anyhow!("{e}")
}

/// The listing a pane shows: hidden names only when asked for.
fn visible(entries: Vec<Entry>, hidden: bool) -> Vec<Entry> {
    entries.into_iter().filter(|e| hidden || !e.hidden).collect()
}

impl Files {
    pub fn new(exe: PathBuf, data_dir: PathBuf, emit: Emit) -> Arc<Files> {
        let pool = Arc::new(Pool::new(exe));
        let queue = Queue::new(data_dir, pool.clone(), emit);
        Arc::new(Files { pool, queue })
    }

    /// Before the daemon exits: running jobs are saved (they load as interrupted) and sessions end.
    pub fn shutdown(&self) {
        self.queue.shutdown();
        self.pool.close_all();
    }

    async fn host(&self, p: &Value) -> Result<SftpFs> {
        Ok(SftpFs(self.pool.browse(s(p, "dest")?).await.map_err(fs_err)?))
    }

    pub async fn rpc(&self, method: &str, p: Value) -> Result<Value> {
        match method {
            "sftp.list" => {
                let path = s(&p, "path")?;
                check_remote_path(path).map_err(fs_err)?;
                let fs = self.host(&p).await?;
                Ok(json!(visible(fs.list(path).await.map_err(fs_err)?, flag(&p, "hidden"))))
            }
            "sftp.stat" => {
                let path = s(&p, "path")?;
                check_remote_path(path).map_err(fs_err)?;
                let fs = self.host(&p).await?;
                let link = fs.lstat(path).await.map_err(fs_err)?;
                let target = if link.kind == super::Kind::Link { fs.stat(path).await.ok() } else { Some(link.clone()) };
                Ok(json!({"lstat": link, "stat": target}))
            }
            "sftp.stat_many" => {
                // Times and sizes of many paths over the host's one browse session: a remote project's live updates
                // poll these while the window is shown, and read again only what changed.
                let targets = stat_targets(&p)?;
                if targets.is_empty() {
                    return Ok(json!([]));
                }
                let conn = self.host(&p).await?.0;
                let mut tasks = tokio::task::JoinSet::new();
                for (i, path) in targets.iter().enumerate() {
                    if let Some(path) = path.clone() {
                        let fs = SftpFs(conn.clone());
                        tasks.spawn(async move { (i, fs.stat(&path).await) });
                    }
                }
                // An invalid path stands as "not found": null, like a path that is not there.
                let mut results: Vec<FsResult<Meta>> = targets.iter().map(|_| Err(FsError::new(ErrKind::NotFound, "invalid path"))).collect();
                while let Some(done) = tasks.join_next().await {
                    let (i, r) = done.map_err(|e| anyhow!("{e}"))?;
                    results[i] = r;
                }
                Ok(json!(fold_stats(results).map_err(fs_err)?))
            }
            "sftp.realpath" => {
                // `~`, `.` and an empty path are the login folder; anything else must be absolute.
                let path = p.get("path").and_then(Value::as_str).unwrap_or("~");
                if !matches!(path, "" | "." | "~") {
                    check_remote_path(path).map_err(fs_err)?;
                }
                let fs = self.host(&p).await?;
                Ok(json!({"path": fs.realpath(path).await.map_err(fs_err)?}))
            }
            "sftp.mkdir" => {
                let path = s(&p, "path")?;
                check_remote_path(path).map_err(fs_err)?;
                self.host(&p).await?.mkdir(path).await.map_err(fs_err)?;
                Ok(json!(true))
            }
            "sftp.create" => {
                let path = s(&p, "path")?;
                check_remote_path(path).map_err(fs_err)?;
                self.host(&p).await?.create_empty(path).await.map_err(fs_err)?;
                Ok(json!(true))
            }
            "sftp.rename" => {
                let (from, to) = (s(&p, "from")?, s(&p, "to")?);
                check_remote_path(from).map_err(fs_err)?;
                check_remote_path(to).map_err(fs_err)?;
                self.host(&p).await?.rename(from, to, false).await.map_err(fs_err)?;
                Ok(json!(true))
            }
            "sftp.chmod" => {
                // One path or several, not recursive: a recursive change is a queue job (`xfer.start` kind `chmod`).
                if flag(&p, "recursive") {
                    bail!("a recursive change runs as a transfer job (xfer.start with kind chmod)");
                }
                let mode = p.get("mode").and_then(Value::as_u64).ok_or_else(|| anyhow!("missing param `mode`"))? as u32;
                let paths: Vec<&str> = p.get("paths").and_then(Value::as_array).ok_or_else(|| anyhow!("missing param `paths`"))?.iter().filter_map(Value::as_str).collect();
                for path in &paths {
                    check_remote_path(path).map_err(fs_err)?;
                }
                let fs = self.host(&p).await?;
                for path in paths {
                    fs.chmod(path, mode).await.map_err(fs_err)?;
                }
                Ok(json!(true))
            }
            "local.list" => {
                let path = s(&p, "path")?;
                check_local_path(path).map_err(fs_err)?;
                Ok(json!(visible(LocalFs.list(path).await.map_err(fs_err)?, flag(&p, "hidden"))))
            }
            "local.home" => Ok(json!({"path": LocalFs.realpath("~").await.map_err(fs_err)?})),
            m if m.starts_with("xfer.") => self.queue.rpc(m, &p).await,
            other => bail!("unknown method {other}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::Kind;

    fn meta(mtime: u64) -> Meta {
        Meta { kind: Kind::File, size: 1, mtime, mode: None, uid: None, gid: None }
    }

    #[test]
    fn a_missing_path_is_null_and_a_lost_connection_fails_the_call() {
        let gone = FsError::new(ErrKind::NotFound, "no such file or folder");
        let denied = FsError::new(ErrKind::Denied, "permission denied");
        assert_eq!(fold_stats(vec![Ok(meta(5)), Err(gone), Err(denied)]).unwrap(), vec![Some(meta(5)), None, None]);
        let lost = FsError::new(ErrKind::Connection, "the connection to the host was lost");
        assert!(fold_stats(vec![Ok(meta(5)), Err(lost)]).is_err());
    }

    #[test]
    fn an_odd_path_is_answered_null_and_the_rest_still_asked() {
        let t = stat_targets(&json!({"paths": ["/srv/a", "relative", "/srv/b\u{7}", "/srv/c"]})).unwrap();
        assert_eq!(t, vec![Some("/srv/a".into()), None, None, Some("/srv/c".into())]);
        assert!(stat_targets(&json!({"paths": ["/srv/a", 5]})).is_err());
        assert!(stat_targets(&json!({"paths": vec!["/a"; STAT_MANY_MAX + 1]})).is_err());
        assert!(stat_targets(&json!({})).is_err());
    }
}
