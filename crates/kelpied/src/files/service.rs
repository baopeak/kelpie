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
use super::{check_local_path, check_remote_path, Entry, FsError, RemoteFs};

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
