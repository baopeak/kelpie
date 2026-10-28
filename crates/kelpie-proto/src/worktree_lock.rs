//! Coordinates Kelpie's local worktree operations across the daemon and desktop processes.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use fs2::FileExt;

/// Keep the returned file open until the Git worktree command has exited. The common Git
/// directory makes a repository and all its linked worktrees share the same OS lock.
/// Never remove the lock file: another process could otherwise lock a different inode.
/// Installed agent CLIs and other external Git commands do not participate in this lock.
pub fn acquire(repo: &Path) -> Result<File> {
    let mut git = Command::new("git");
    git.args(["rev-parse", "--git-common-dir"]).current_dir(repo).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        git.creation_flags(0x0800_0000);
    }
    let output = git.output().context("find common Git directory for worktree operation")?;
    if !output.status.success() {
        bail!("find common Git directory: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let common = String::from_utf8(output.stdout).context("common Git directory is not UTF-8")?;
    let common = repo.join(common.trim_end_matches(['\r', '\n']));
    let file = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(common.join("kelpie-worktrees.lock")).context("open repository worktree lock")?;
    FileExt::lock_exclusive(&file).context("lock repository worktree operations")?;
    Ok(file)
}
