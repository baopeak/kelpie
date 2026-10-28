//! Quick open (fuzzy file paths) and search-in-files.
//!
//! The index is built once per project with a parallel, `.gitignore`-aware walk and cached; the
//! watcher invalidates it. It holds files and folders (a folder ends in `/`); callers that only
//! want files leave `dirs` off. Fuzzy ranking uses nucleo (the matcher behind Helix), with a
//! filename bonus so `main` finds `src/main.rs` before `domain/x.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use ignore::{WalkBuilder, WalkState};
use parking_lot::Mutex;
use serde::Serialize;

use crate::fs_ops::{err, guard, CmdResult};

mod rank;
#[cfg(test)]
mod tests;

use rank::{rank, QuickOpenHit};

const MAX_INDEX: usize = 300_000;

#[derive(Default)]
pub struct FileIndex {
    by_root: Mutex<HashMap<String, Arc<Vec<String>>>>,
    /// When each remote project's list was fetched (no watcher there: it is refreshed after a
    /// minute instead).
    remote_at: Mutex<HashMap<String, std::time::Instant>>,
}

const REMOTE_INDEX_TTL: std::time::Duration = std::time::Duration::from_secs(60);

impl FileIndex {
    pub fn invalidate(&self, root: &str) {
        self.by_root.lock().remove(root);
    }

    fn get(&self, root: &str) -> Arc<Vec<String>> {
        if let Some(files) = self.by_root.lock().get(root) {
            return files.clone();
        }
        let files = Arc::new(walk_index(Path::new(root)));
        self.by_root.lock().insert(root.to_string(), files.clone());
        files
    }
}

fn walker(root: &Path) -> WalkBuilder {
    let mut b = WalkBuilder::new(root);
    b.hidden(false).git_ignore(true).git_global(true).git_exclude(true).ignore(true).parents(true);
    b.filter_entry(|e| {
        let name = e.file_name().to_string_lossy();
        !matches!(name.as_ref(), ".git" | "node_modules" | ".hg" | ".svn")
    });
    b.threads(std::thread::available_parallelism().map(|n| n.get().min(8)).unwrap_or(4));
    b
}

/// Every file of the project, and every folder as `dir/`.
fn walk_index(root: &Path) -> Vec<String> {
    let out = Mutex::new(Vec::with_capacity(4096));
    walker(root).build_parallel().run(|| {
        let out = &out;
        Box::new(move |entry| {
            let Ok(entry) = entry else { return WalkState::Continue };
            let Some(kind) = entry.file_type() else { return WalkState::Continue };
            if kind.is_file() || (kind.is_dir() && entry.depth() > 0) {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    let mut out = out.lock();
                    if out.len() >= MAX_INDEX {
                        return WalkState::Quit;
                    }
                    let mut path = rel.to_string_lossy().replace('\\', "/");
                    if kind.is_dir() {
                        path.push('/');
                    }
                    out.push(path);
                }
            }
            WalkState::Continue
        })
    });
    out.into_inner()
}

/// A remote project lists files only: its folders are the parents of those.
fn with_parent_dirs(mut files: Vec<String>) -> Vec<String> {
    let mut dirs = std::collections::BTreeSet::new();
    for f in &files {
        let mut end = f.len();
        while let Some(i) = f[..end].rfind('/') {
            if !dirs.insert(f[..=i].to_string()) {
                break;
            }
            end = i;
        }
    }
    files.extend(dirs);
    files
}

#[tauri::command]
pub async fn quick_open(index: tauri::State<'_, Arc<FileIndex>>, root: String, query: String, limit: Option<usize>, dirs: Option<bool>) -> CmdResult<Vec<QuickOpenHit>> {
    let index = index.inner().clone();
    if crate::remote::is_remote(&root) && !index.remote_at.lock().get(&root).is_some_and(|t| t.elapsed() < REMOTE_INDEX_TTL) {
        let files = crate::remote::list_files(&root).await?;
        index.by_root.lock().insert(root.clone(), Arc::new(with_parent_dirs(files)));
        index.remote_at.lock().insert(root.clone(), std::time::Instant::now());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let files = index.get(&root);
        rank(&files, &query, limit.unwrap_or(50), dirs.unwrap_or(false))
    })
    .await
    .map_err(err)
}

#[derive(Serialize)]
pub struct TextHit {
    path: String,
    line: u32,
    column: u32,
    text: String,
}

#[derive(Serialize)]
pub struct SearchResult {
    hits: Vec<TextHit>,
    truncated: bool,
    files_searched: usize,
}

#[tauri::command]
pub async fn search_text(root: String, query: String, regex: bool, case_sensitive: bool, max_hits: Option<usize>, folder: Option<String>) -> CmdResult<SearchResult> {
    if crate::remote::is_remote(&root) {
        let (hits, truncated) = crate::remote::search_text(&root, &query, regex, case_sensitive, folder.as_deref(), max_hits.unwrap_or(2000)).await?;
        let files = hits.iter().map(|h| h.path.as_str()).collect::<std::collections::HashSet<_>>().len();
        return Ok(SearchResult {
            hits: hits.into_iter().map(|h| TextHit { path: h.path, line: h.line as u32, column: h.column as u32, text: h.text }).collect(),
            truncated,
            files_searched: files,
        });
    }
    let base = guard(&root, "")?;
    // Find in folder: walk only that folder; hit paths stay relative to the project.
    let start = match folder.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => guard(&root, f)?,
        None => base.clone(),
    };
    tauri::async_runtime::spawn_blocking(move || {
        let pattern = if regex { query.clone() } else { regex::escape(&query) };
        let re = regex::RegexBuilder::new(&pattern).case_insensitive(!case_sensitive).build().map_err(err)?;
        let max_hits = max_hits.unwrap_or(2000);
        let hits = Mutex::new(Vec::new());
        let files = std::sync::atomic::AtomicUsize::new(0);
        let done = std::sync::atomic::AtomicBool::new(false);
        walker(&start).build_parallel().run(|| {
            let (hits, files, done, re, base) = (&hits, &files, &done, &re, &base);
            Box::new(move |entry| {
                if done.load(std::sync::atomic::Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) || entry.metadata().map(|m| m.len() > 4 << 20).unwrap_or(true) {
                    return WalkState::Continue;
                }
                let Ok(bytes) = std::fs::read(entry.path()) else { return WalkState::Continue };
                if bytes[..bytes.len().min(4096)].contains(&0) {
                    return WalkState::Continue;
                }
                files.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let text = String::from_utf8_lossy(&bytes);
                let rel = entry.path().strip_prefix(base).unwrap_or(entry.path()).to_string_lossy().replace('\\', "/");
                for (i, line) in text.lines().enumerate() {
                    if let Some(m) = re.find(line) {
                        let mut hits = hits.lock();
                        if hits.len() >= max_hits {
                            done.store(true, std::sync::atomic::Ordering::Relaxed);
                            return WalkState::Quit;
                        }
                        let snippet: String = line.chars().take(300).collect();
                        hits.push(TextHit { path: rel.clone(), line: i as u32 + 1, column: line[..m.start()].chars().count() as u32 + 1, text: snippet });
                    }
                }
                WalkState::Continue
            })
        });
        let mut hits = hits.into_inner();
        hits.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        Ok(SearchResult { truncated: done.into_inner(), files_searched: files.into_inner(), hits })
    })
    .await
    .map_err(err)?
}
