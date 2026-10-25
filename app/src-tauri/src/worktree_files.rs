//! Tauri commands over `kelpie_proto::materialize` (copies and links of ignored files into a new
//! worktree): the worktree dialog, auto-worktree and Finish's check worktree use it.

use std::path::PathBuf;

use kelpie_proto::materialize::{estimate, Estimate, Materialized};
pub(crate) use kelpie_proto::materialize::{link_dir, materialize};

use crate::fs_ops::{blocking, guard, CmdResult};

#[tauri::command]
pub async fn worktree_materialize(root: String, worktree: String, copy: Vec<String>, link: Vec<String>) -> CmdResult<Materialized> {
    let repo = guard(&root, "")?;
    let wt = PathBuf::from(worktree.trim());
    if !wt.is_absolute() || !wt.is_dir() {
        return Err("the worktree folder does not exist".into());
    }
    blocking(move || Ok(materialize(repo, wt, copy, link))).await
}

/// What `worktree_materialize` would bring into one fresh worktree, and how large the copies are.
#[tauri::command]
pub async fn worktree_materialize_estimate(root: String, copy: Vec<String>, link: Vec<String>) -> CmdResult<Estimate> {
    let repo = guard(&root, "")?;
    blocking(move || Ok(estimate(&repo, copy, link))).await
}
