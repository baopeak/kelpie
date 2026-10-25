//! Review notes: comments on lines of a worktree's diff, written by the user in the app's diff
//! view or by a reviewing agent (`kelpied review add <file>:<line> "<text>"`), and sent to the
//! agent that has to act on them. One store per daemon, keyed by worktree folder, so the diff
//! gutter, the review card and `@note` read the same notes whoever wrote them. Stored in
//! `review-notes.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};

use crate::checkpoints::{find_top, git};
use crate::persist::write_atomic;

const MAX_NOTES: usize = 500;
const MAX_NOTE: usize = 4000;
const MAX_TEXT: usize = 400;
const MAX_FILE: usize = 1024;
/// Largest file read to quote a line that is not part of any diff.
const MAX_QUOTE_FILE: u64 = 5 << 20;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Note {
    pub id: String,
    /// File, relative to the worktree, with `/`.
    pub path: String,
    /// 1-based line number on `side`.
    pub line: u32,
    /// `new` (the file as it is now) or `old` (a removed line).
    pub side: String,
    /// The line as it read when the note was made.
    pub text: String,
    pub note: String,
    pub at: u64,
    /// The agent that wrote it; none for the user's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
}

pub struct NoteStore {
    path: PathBuf,
    map: BTreeMap<String, Vec<Note>>,
}

/// The key of the worktree `dir` is in: the folder holding `.git`, else `dir` itself, spelled the
/// way the metadata store keys folders. A folder that does not exist (a removed worktree) is its own
/// key: looking upward from it would find the repository around it and mistake it for that.
pub fn worktree_key(dir: &str) -> String {
    if !Path::new(dir.trim()).exists() {
        return crate::meta::key(dir);
    }
    let top = find_top(Path::new(dir.trim())).map(|p| p.to_string_lossy().into_owned());
    crate::meta::key(top.as_deref().unwrap_or(dir))
}

/// Control characters (a note ends up in an agent's input) and bidi overrides out.
fn clean_text(s: &str, max: usize) -> String {
    s.chars().filter(|c| matches!(c, '\n' | '\t') || !(c.is_control() || ('\u{202a}'..='\u{202e}').contains(c) || ('\u{2066}'..='\u{2069}').contains(c))).take(max).collect::<String>().trim().to_string()
}

/// `a/b.rs`: relative, no `..`, `.git` or control characters.
fn clean_file(raw: &str) -> Result<String> {
    let p = raw.trim().replace('\\', "/");
    let p = p.trim_start_matches("./");
    ensure!(!p.is_empty() && p.len() <= MAX_FILE && !p.chars().any(char::is_control), "not a file path");
    ensure!(!p.starts_with('/') && !p.contains(':'), "the file must be relative to the worktree");
    ensure!(p.split('/').all(|s| !s.is_empty() && s != "." && s != ".." && !s.eq_ignore_ascii_case(".git")), "the file must be inside the worktree");
    Ok(p.to_string())
}

/// A branch, tag or commit name that cannot be taken for an option.
fn clean_ref(r: &str) -> Result<&str> {
    let r = r.trim();
    ensure!(!r.is_empty() && !r.starts_with('-') && r.len() <= 200 && r.chars().all(|c| c.is_ascii_alphanumeric() || "._/~^@-".contains(c)), "not a git ref");
    Ok(r)
}

/// The text of line `line` of the new file (or, with `old`, of the old one) when it is part of
/// the zero-context `diff` of one file.
fn line_in_diff(diff: &str, line: u32, old: bool) -> Option<String> {
    let (mut old_ln, mut new_ln) = (0u32, 0u32);
    for l in diff.lines() {
        if let Some(h) = l.strip_prefix("@@ -") {
            let num = |s: &str| s.split(',').next().and_then(|n| n.parse::<u32>().ok());
            let mut parts = h.split_whitespace();
            old_ln = parts.next().and_then(num).unwrap_or(0);
            new_ln = parts.next().and_then(|p| p.strip_prefix('+')).and_then(num).unwrap_or(0);
        } else if let Some(t) = l.strip_prefix('+').filter(|_| new_ln > 0 && !l.starts_with("+++")) {
            if !old && new_ln == line {
                return Some(t.to_string());
            }
            new_ln += 1;
        } else if let Some(t) = l.strip_prefix('-').filter(|_| old_ln > 0 && !l.starts_with("---")) {
            if old && old_ln == line {
                return Some(t.to_string());
            }
            old_ln += 1;
        }
    }
    None
}

/// The branch the work will be merged into, as far as git can tell: a `main` or `master`, local or
/// remote, and where this worktree's HEAD left it.
fn default_base(top: &Path) -> Option<String> {
    ["main", "master", "origin/main", "origin/master"].iter().find_map(|b| git(top, None, &["merge-base", "HEAD", b]).ok().filter(|s| !s.is_empty()))
}

/// Which side a note on `file:line` is on and the line's text, read from `git diff` (against
/// `base`; else uncommitted changes, then the default branch). A line outside every diff is
/// quoted from the file itself; a removed line must be in a diff.
pub fn locate(top: &Path, file: &str, line: u32, old: bool, base: Option<&str>) -> Result<(String, String)> {
    let rel = clean_file(file)?;
    ensure!(line >= 1, "lines start at 1");
    let mut bases: Vec<String> = vec![];
    match base.filter(|b| !b.trim().is_empty()) {
        Some(b) => bases.push(clean_ref(b)?.to_string()),
        None => {
            bases.push("HEAD".into());
            bases.extend(default_base(top));
        }
    }
    for b in &bases {
        let diff = git(top, None, &["diff", "--no-color", "--no-ext-diff", "--no-textconv", "-U0", b, "--", &rel]).unwrap_or_default();
        if let Some(text) = line_in_diff(&diff, line, old) {
            return Ok((if old { "old" } else { "new" }.into(), text));
        }
    }
    ensure!(!old, "line {line} of the old {rel} is not a removed line of the changes");
    let full = top.join(&rel);
    let meta = std::fs::symlink_metadata(&full).map_err(|_| anyhow::anyhow!("{rel} is not a file of this worktree"))?;
    ensure!(meta.is_file() && meta.len() <= MAX_QUOTE_FILE, "{rel} is not a regular file of a size a note can quote");
    // A link or junction in the middle of the path would lead out of the worktree.
    let inside = std::fs::canonicalize(&full).ok().zip(std::fs::canonicalize(top).ok()).is_some_and(|(f, t)| f.starts_with(t));
    ensure!(inside, "{rel} is not inside this worktree");
    let text = String::from_utf8_lossy(&std::fs::read(&full)?).into_owned();
    match text.lines().nth(line as usize - 1) {
        Some(l) => Ok(("new".into(), l.to_string())),
        None => bail!("{rel} has {} lines: no line {line}", text.lines().count()),
    }
}

impl NoteStore {
    pub fn load(dir: &Path) -> NoteStore {
        let path = dir.join("review-notes.json");
        let map = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        NoteStore { path, map }
    }

    fn save(&self) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(&self.map)?)
    }

    pub fn list(&self, key: &str) -> Vec<Note> {
        self.map.get(key).cloned().unwrap_or_default()
    }

    pub fn all(&self) -> &BTreeMap<String, Vec<Note>> {
        &self.map
    }

    /// Adds `note` to the worktree `key`. A note with an id already there is the same note
    /// arriving again (the app moving its saved notes over): kept once.
    pub fn add(&mut self, key: &str, mut note: Note) -> Result<Note> {
        note.path = clean_file(&note.path)?;
        note.note = clean_text(&note.note, MAX_NOTE);
        note.text = clean_text(&note.text, MAX_TEXT);
        ensure!(!note.note.is_empty(), "a note needs some text");
        ensure!(note.line >= 1, "lines start at 1");
        ensure!(note.side == "new" || note.side == "old", "side is new or old");
        let list = self.map.entry(key.to_string()).or_default();
        if let Some(same) = list.iter().find(|n| n.id == note.id) {
            return Ok(same.clone());
        }
        ensure!(list.len() < MAX_NOTES, "this worktree has {MAX_NOTES} notes already: resolve some first");
        list.push(note.clone());
        self.save()?;
        Ok(note)
    }

    /// Removes the notes `ids` (all of the worktree's with none); the notes left.
    pub fn clear(&mut self, key: &str, ids: Option<&[String]>) -> Result<Vec<Note>> {
        if let Some(list) = self.map.get_mut(key) {
            match ids {
                Some(ids) => list.retain(|n| !ids.contains(&n.id)),
                None => list.clear(),
            }
            if list.is_empty() {
                self.map.remove(key);
            }
            self.save()?;
        }
        Ok(self.list(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, path: &str, line: u32) -> Note {
        Note { id: id.into(), path: path.into(), line, side: "new".into(), text: "code".into(), note: "why?".into(), at: 1, by: None }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kelpie-notes-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).current_dir(dir).status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn notes_are_kept_per_worktree_and_survive_a_restart() {
        let dir = scratch("store");
        let mut s = NoteStore::load(&dir);
        s.add("c:/w/a", note("1", "src/a.rs", 3)).unwrap();
        s.add("c:/w/a", note("2", "src/b.rs", 9)).unwrap();
        s.add("c:/w/b", note("3", "src/a.rs", 3)).unwrap();
        assert_eq!(NoteStore::load(&dir).list("c:/w/a").len(), 2);
        // The same id again (the app migrating twice) adds nothing.
        s.add("c:/w/a", note("1", "src/a.rs", 3)).unwrap();
        assert_eq!(s.list("c:/w/a").len(), 2);
        let left = s.clear("c:/w/a", Some(&["1".to_string()])).unwrap();
        assert_eq!(left.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(), ["2"]);
        assert_eq!(s.list("c:/w/b").len(), 1, "another worktree is untouched");
        s.clear("c:/w/a", None).unwrap();
        assert!(NoteStore::load(&dir).all().get("c:/w/a").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_note_is_checked_before_it_is_kept() {
        let dir = scratch("check");
        let mut s = NoteStore::load(&dir);
        for bad in ["../x.rs", "/etc/passwd", "a/../../b", "C:/x", ".git/config", ""] {
            assert!(s.add("k", note("1", bad, 1)).is_err(), "{bad}");
        }
        assert!(s.add("k", note("1", "a.rs", 0)).is_err());
        let mut empty = note("1", "a.rs", 1);
        empty.note = " \u{1b}\u{7} ".into();
        assert!(s.add("k", empty).is_err(), "only control characters is no note");
        let mut loud = note("2", "a.rs", 1);
        loud.note = "fix\u{1b}[2J this\u{202e}".into();
        assert_eq!(s.add("k", loud).unwrap().note, "fix[2J this", "escapes and bidi overrides are dropped");
        for i in 0..MAX_NOTES - 1 {
            s.add("k", note(&format!("n{i}"), "a.rs", 1)).unwrap();
        }
        assert!(s.add("k", note("over", "a.rs", 1)).unwrap_err().to_string().contains("500 notes"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lines_are_found_in_a_zero_context_diff() {
        let diff = "diff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -2,2 +2,3 @@ fn x\n-old two\n-old three\n+new two\n+new three\n+new four\n@@ -10 +11 @@\n-ten\n+eleven\n";
        assert_eq!(line_in_diff(diff, 3, false).as_deref(), Some("new three"));
        assert_eq!(line_in_diff(diff, 4, false).as_deref(), Some("new four"));
        assert_eq!(line_in_diff(diff, 11, false).as_deref(), Some("eleven"));
        assert_eq!(line_in_diff(diff, 3, true).as_deref(), Some("old three"));
        assert_eq!(line_in_diff(diff, 10, true).as_deref(), Some("ten"));
        assert_eq!(line_in_diff(diff, 1, false), None);
        assert_eq!(line_in_diff(diff, 5, false), None);
    }

    #[test]
    fn side_and_text_come_from_the_diff() {
        let dir = scratch("locate");
        run_git(&dir, &["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("f.txt"), "one\ntwo\nthree\n").unwrap();
        run_git(&dir, &["add", "."]);
        run_git(&dir, &["commit", "-qm", "init"]);
        run_git(&dir, &["checkout", "-qb", "work"]);
        std::fs::write(dir.join("f.txt"), "one\nTWO\nthree\nfour\n").unwrap();
        run_git(&dir, &["commit", "-qam", "edit"]);
        // Uncommitted on top.
        std::fs::write(dir.join("f.txt"), "one\nTWO\nthree\nfour\nfive\n").unwrap();
        // Committed change, found through the default branch.
        assert_eq!(locate(&dir, "f.txt", 2, false, None).unwrap(), ("new".into(), "TWO".into()));
        // A removed line, with and without naming the base.
        assert_eq!(locate(&dir, "f.txt", 2, true, Some("main")).unwrap(), ("old".into(), "two".into()));
        assert_eq!(locate(&dir, "f.txt", 2, true, None).unwrap(), ("old".into(), "two".into()));
        // Uncommitted line; an unchanged line is quoted from the file.
        assert_eq!(locate(&dir, "f.txt", 5, false, None).unwrap(), ("new".into(), "five".into()));
        assert_eq!(locate(&dir, "f.txt", 1, false, None).unwrap(), ("new".into(), "one".into()));
        // Nothing to quote.
        assert!(locate(&dir, "f.txt", 99, false, None).unwrap_err().to_string().contains("no line 99"));
        assert!(locate(&dir, "f.txt", 1, true, None).is_err(), "line 1 was not removed");
        assert!(locate(&dir, "nope.txt", 1, false, None).is_err());
        assert!(locate(&dir, "../f.txt", 1, false, None).is_err());
        assert!(locate(&dir, "f.txt", 2, false, Some("--output=x")).is_err(), "a base cannot be an option");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_removed_worktree_is_its_own_key_not_the_repository_around_it() {
        let repo = scratch("nested");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let nested = repo.join(".claude/worktrees/x");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join(".git"), "gitdir: elsewhere").unwrap();
        let (outer_key, nested_key) = (worktree_key(&repo.to_string_lossy()), worktree_key(&nested.to_string_lossy()));
        assert_ne!(outer_key, nested_key, "a nested worktree has its own notes");
        std::fs::remove_dir_all(&nested).unwrap();
        assert_eq!(worktree_key(&nested.to_string_lossy()), crate::meta::key(&nested.to_string_lossy()));
        assert_ne!(worktree_key(&nested.to_string_lossy()), outer_key, "clearing it cannot reach the repository's notes");
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    fn the_worktree_is_where_the_git_marker_is() {
        let dir = scratch("key");
        std::fs::create_dir_all(dir.join("sub/deep")).unwrap();
        std::fs::write(dir.join(".git"), "gitdir: elsewhere").unwrap();
        let top = worktree_key(&dir.to_string_lossy());
        assert_eq!(worktree_key(&dir.join("sub/deep").to_string_lossy()), top);
        assert_ne!(worktree_key(&scratch("other").to_string_lossy()), top);
        let _ = std::fs::remove_dir_all(dir);
    }
}
