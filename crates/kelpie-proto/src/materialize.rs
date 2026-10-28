//! Brings gitignored files into a new worktree, which starts as a clean checkout:
//! - **copy**: `.worktreeinclude` lines and `.kelpie.yaml` `copy:` entries (e.g. `.env`) get a
//!   private copy;
//! - **link**: `.kelpie.yaml` `link:` directories (e.g. `node_modules`) are shared with the
//!   repository through a symlink (a junction on Windows: no admin rights needed).
//!
//! Only literal, repository-relative paths that exist in the repository and are ignored by
//! git qualify; anything else is reported as skipped. Copies are bounded. Shared by the app
//! (worktree dialog, auto-worktree) and the daemon (run worktrees).

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

const MAX_COPY_BYTES: u64 = 2 << 30;
const MAX_COPY_ENTRIES: u64 = 50_000;

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct Materialized {
    pub copied: Vec<String>,
    pub linked: Vec<String>,
    pub skipped: Vec<Skipped>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Skipped {
    pub path: String,
    pub reason: String,
}

/// What `materialize` would bring into one worktree: shown before a run starts.
#[derive(Serialize, Default, Debug)]
pub struct Estimate {
    pub copy_bytes: u64,
    pub copy_entries: u64,
    pub copied: Vec<String>,
    pub linked: Vec<String>,
    pub skipped: Vec<Skipped>,
}

/// `a/b` with no absolute, `..`, `.` or `.git` parts.
fn clean_rel(raw: &str) -> Option<String> {
    let p = raw.trim().replace('\\', "/");
    let p = p.trim_start_matches("./").trim_end_matches('/');
    if p.is_empty() || p.contains(['*', '?']) || p.starts_with('!') {
        return None;
    }
    let path = Path::new(p);
    let ok = path.components().all(|c| matches!(c, Component::Normal(s) if s != ".git"))
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..");
    ok.then(|| p.to_string())
}

/// One `git check-ignore` run over `specs`: the ones it reports ignored, or `None` when git failed
/// (it stops at the first path it cannot look at, e.g. one beyond a tracked symlink).
fn check_ignore(repo: &Path, specs: &[String]) -> Option<std::collections::HashSet<String>> {
    use std::io::Write;
    use std::process::Stdio;
    let mut cmd = std::process::Command::new("git");
    cmd.args(["check-ignore", "-z", "--stdin"])
        .current_dir(repo)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let input: Vec<u8> = specs.iter().flat_map(|s| s.bytes().chain([0])).collect();
        // The paths are few and short: the pipe never fills before git reads them.
        let _ = stdin.write_all(&input);
    }
    let out = child.wait_with_output().ok()?;
    // 0: some ignored, 1: none; anything else is git giving up.
    matches!(out.status.code(), Some(0 | 1)).then(|| out.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).trim_end_matches('/').to_string()).collect())
}

/// Which of `rels` git ignores, from one `git check-ignore` run (starting git is the slow part
/// when several worktrees are made at once); if that run fails, one run per entry, so a single
/// bad entry only loses itself.
fn ignored_among(repo: &Path, rels: &[String]) -> std::collections::HashSet<String> {
    if rels.is_empty() {
        return Default::default();
    }
    // check-ignore does not look at the disk: a folder needs its trailing slash to match
    // directory-only patterns such as `node_modules/`.
    let spec = |rel: &String| if repo.join(rel).is_dir() { format!("{rel}/") } else { rel.clone() };
    let all: Vec<String> = rels.iter().map(spec).collect();
    if let Some(found) = check_ignore(repo, &all) {
        return found;
    }
    rels.iter().filter(|rel| check_ignore(repo, &[spec(rel)]).is_some_and(|f| f.contains(*rel))).cloned().collect()
}

/// `a` and `b` are the same path or one is inside the other.
fn overlaps(a: &str, b: &str) -> bool {
    a == b || a.starts_with(&format!("{b}/")) || b.starts_with(&format!("{a}/"))
}

/// Bytes and entries under `p` (stops counting past the budget).
fn measure(p: &Path, bytes: &mut u64, entries: &mut u64) {
    *entries += 1;
    if *entries > MAX_COPY_ENTRIES || *bytes > MAX_COPY_BYTES {
        return;
    }
    match std::fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
                measure(&e.path(), bytes, entries);
            }
        }
        Ok(m) => *bytes += m.len(),
        Err(_) => {}
    }
}

fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(src, dst).map(|_| ())
    }
}

pub fn link_dir(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    #[cfg(windows)]
    {
        create_junction(src, dst).map_err(|e| e.to_string())
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(src, dst).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
fn create_junction(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::{
        Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT},
        System::{IO::DeviceIoControl, Ioctl::FSCTL_SET_REPARSE_POINT},
    };

    // Canonicalization supplies an absolute, verbatim Unicode path, including
    // long paths. Reparse targets use the NT prefix instead of Win32's \\?\.
    let target = std::fs::canonicalize(src)?;
    let wide: Vec<u16> = target.as_os_str().encode_wide().collect();
    let target = wide.strip_prefix(&[92, 92, 63, 92]).unwrap_or(&wide);
    let substitute_units = 4 + target.len(); // \??\ + target (UNC\... for shares)
    let print_units = target.len();
    let data_bytes = 8 + (substitute_units + 1 + print_units + 1) * 2;
    // MAXIMUM_REPARSE_DATA_BUFFER_SIZE includes the eight-byte common header.
    if data_bytes + 8 > 16 * 1024 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "junction target is too long"));
    }
    // REPARSE_DATA_BUFFER, with the mount-point variant. Offsets/lengths are
    // bytes relative to PathBuffer; lengths exclude the two NUL terminators.
    // u16 storage keeps both the header and UTF-16 names suitably aligned.
    let mut data = Vec::<u16>::with_capacity((data_bytes + 8) / 2);
    data.extend_from_slice(&[
        0x0003, 0xa000, // IO_REPARSE_TAG_MOUNT_POINT = 0xa0000003
        data_bytes as u16, 0, // ReparseDataLength, Reserved
        0, (substitute_units * 2) as u16,
        ((substitute_units + 1) * 2) as u16, (print_units * 2) as u16,
        92, 63, 63, 92, // \??\
    ]);
    data.extend_from_slice(target);
    data.push(0);
    data.extend_from_slice(target);
    data.push(0);

    // create_dir, unlike create_dir_all, atomically refuses an existing
    // destination. Rollback below therefore owns only this newly-created dir.
    std::fs::create_dir(dst)?;
    let result = (|| {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(dst)?;
        let mut returned = 0;
        // SAFETY: file owns a live directory handle; data is an aligned,
        // length-checked mount-point buffer and remains alive for the call.
        // This is synchronous I/O, with no output buffer or OVERLAPPED.
        let ok = unsafe {
            DeviceIoControl(
                file.as_raw_handle(),
                FSCTL_SET_REPARSE_POINT,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
                std::ptr::null_mut(),
                0,
                &mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
        // file closes before rollback, including when DeviceIoControl fails.
    })();
    if result.is_err() {
        // Never recurse: remove_dir removes only the empty destination or the
        // junction itself, not anything in the target. Preserve the I/O error.
        let _ = std::fs::remove_dir(dst);
    }
    result
}

/// The entries that qualify (`(path, linked)`, links first) and the ones that do not. With a
/// `worktree`, an entry already there is skipped too.
fn plan(repo: &Path, worktree: Option<&Path>, copy: Vec<String>, link: Vec<String>) -> (Vec<(String, bool)>, Vec<Skipped>) {
    let mut skipped = Vec::new();
    let mut skip = |path: &str, reason: &str| skipped.push(Skipped { path: path.into(), reason: reason.into() });
    let include = std::fs::read_to_string(repo.join(".worktreeinclude")).unwrap_or_default();
    let mut copy_list: Vec<String> = include.lines().filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).map(str::to_string).collect();
    copy_list.extend(copy);
    let mut seen = std::collections::HashSet::new();
    let mut found = Vec::new();
    for (raw, linked) in link.iter().map(|l| (l, true)).chain(copy_list.iter().map(|c| (c, false))) {
        let Some(rel) = clean_rel(raw) else {
            skip(raw.trim(), "not a plain relative path");
            continue;
        };
        if !seen.insert(rel.clone()) {
            continue;
        }
        if !repo.join(&rel).exists() {
            skip(&rel, "missing in the repository");
        } else if worktree.is_some_and(|w| w.join(&rel).exists()) {
            skip(&rel, "already in the worktree");
        } else {
            found.push((rel, linked));
        }
    }
    let ignored = ignored_among(repo, &found.iter().map(|(rel, _)| rel.clone()).collect::<Vec<_>>());
    let mut items: Vec<(String, bool)> = Vec::new();
    for (rel, linked) in found {
        if !ignored.contains(&rel) {
            skip(&rel, "not ignored by git (tracked files are already checked out)");
        } else if linked && !repo.join(&rel).is_dir() {
            skip(&rel, "only folders can be linked");
        } else if items.iter().any(|(done, _)| overlaps(done, &rel)) {
            // What an earlier entry brings (a link, a copied folder) is already there when this one
            // comes: copying into a link would write onto the repository's own file.
            skip(&rel, "already in the worktree");
        } else {
            items.push((rel, linked));
        }
    }
    (items, skipped)
}

pub fn materialize(repo: PathBuf, worktree: PathBuf, copy: Vec<String>, link: Vec<String>) -> Materialized {
    let (items, skipped) = plan(&repo, Some(&worktree), copy, link);
    let mut out = Materialized { skipped, ..Default::default() };
    let (mut budget_bytes, mut budget_entries) = (0u64, 0u64);
    for (rel, linked) in items {
        let src = repo.join(&rel);
        let dst = worktree.join(&rel);
        let mut skip = |reason: String| out.skipped.push(Skipped { path: rel.clone(), reason });
        // Looked at again now: an earlier entry may have put something here.
        if dst.symlink_metadata().is_ok() {
            skip("already in the worktree".into());
            continue;
        }
        if linked {
            match link_dir(&src, &dst) {
                Ok(()) => out.linked.push(rel.clone()),
                Err(e) => skip(format!("link failed: {e}")),
            }
        } else {
            let (mut bytes, mut entries) = (0, 0);
            measure(&src, &mut bytes, &mut entries);
            if budget_bytes + bytes > MAX_COPY_BYTES || budget_entries + entries > MAX_COPY_ENTRIES {
                skip("too large to copy (2 GiB / 50,000 files per worktree); link it instead".into());
                continue;
            }
            budget_bytes += bytes;
            budget_entries += entries;
            match copy_tree(&src, &dst) {
                Ok(()) => out.copied.push(rel.clone()),
                Err(e) => skip(format!("copy failed: {e}")),
            }
        }
    }
    out
}

/// What one worktree would get: the same selection as `materialize` against a fresh checkout,
/// with the size of the copies.
pub fn estimate(repo: &Path, copy: Vec<String>, link: Vec<String>) -> Estimate {
    let (items, mut skipped) = plan(repo, None, copy, link);
    let mut out = Estimate::default();
    for (rel, linked) in items {
        if linked {
            out.linked.push(rel);
            continue;
        }
        let (mut bytes, mut entries) = (0, 0);
        measure(&repo.join(&rel), &mut bytes, &mut entries);
        if out.copy_bytes + bytes > MAX_COPY_BYTES || out.copy_entries + entries > MAX_COPY_ENTRIES {
            skipped.push(Skipped { path: rel, reason: "too large to copy (2 GiB / 50,000 files per worktree); link it instead".into() });
            continue;
        }
        out.copy_bytes += bytes;
        out.copy_entries += entries;
        out.copied.push(rel);
    }
    out.skipped = skipped;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(repo: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git").args(args).current_dir(repo).status().unwrap().success());
    }

    /// A repository with ignored `deps/` (a folder), ignored `.env` and a tracked file.
    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("kelpie-materialize-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        // Forward slashes on purpose: the app passes such paths on Windows too.
        let repo = PathBuf::from(base.join("repo").to_string_lossy().replace('\\', "/"));
        let wt = PathBuf::from(base.join("wt").to_string_lossy().replace('\\', "/"));
        std::fs::create_dir_all(repo.join("deps/pkg")).unwrap();
        std::fs::create_dir_all(&wt).unwrap();
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join(".gitignore"), "deps/\n.env\n").unwrap();
        std::fs::write(repo.join(".worktreeinclude"), "# copies\n.env\n").unwrap();
        std::fs::write(repo.join(".env"), "X=1").unwrap();
        std::fs::write(repo.join("deps/pkg/a.js"), "1").unwrap();
        std::fs::write(repo.join("tracked.txt"), "t").unwrap();
        (base, repo, wt)
    }

    #[test]
    fn security_regression_materialize_links_literal_metacharacters() {
        let (base, repo, wt) = fixture("literal-links");
        let names = ["deps&literal", "deps-%COMSPEC%-雪"];
        std::fs::write(repo.join(".gitignore"), format!("deps/\n.env\n{}\n", names.join("\n"))).unwrap();
        for name in names {
            std::fs::create_dir(repo.join(name)).unwrap();
            std::fs::write(repo.join(name).join("original.txt"), "unchanged").unwrap();
        }
        let out = materialize(repo.clone(), wt.clone(), vec![], names.iter().map(|name| (*name).into()).collect());
        // Remove links before asserting, so failures do not leave junctions behind.
        let contents: Vec<_> = names.iter().map(|name| std::fs::read_to_string(wt.join(name).join("original.txt"))).collect();
        let originals: Vec<_> = names.iter().map(|name| std::fs::read_to_string(repo.join(name).join("original.txt")).unwrap()).collect();
        for name in names {
            let dst = wt.join(name);
            #[cfg(windows)]
            let _ = std::fs::remove_dir(dst);
            #[cfg(unix)]
            let _ = std::fs::remove_file(dst);
        }
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(out.linked, names, "{out:?}");
        for content in contents {
            assert_eq!(content.unwrap(), "unchanged", "literal destination must resolve to its source");
        }
        assert!(originals.iter().all(|content| content == "unchanged"));
    }

    #[cfg(windows)]
    #[test]
    fn security_regression_link_dir_never_executes_path_commands() {
        // No environment mutation: cmd would expand the inherited TEMP variable.
        // The only injected command is a batch file that writes a sentinel inside
        // this isolated fixture; it has no external effects.
        let temp = PathBuf::from(std::env::var_os("TEMP").expect("Windows TEMP"));
        let base_name = format!("kelpie-materialize-shell-sentinel-{}", std::process::id());
        let base = temp.join(&base_name);
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let sentinel = base.join("sentinel-executed");
        std::fs::write(base.join("sentinel.cmd"), "@echo off\r\n@echo sentinel>\"%~dp0sentinel-executed\"\r\n").unwrap();
        let src = base.join("source&%TEMP%").join(&base_name).join("sentinel.cmd&rem");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("original.txt"), "unchanged").unwrap();
        let dst = base.join("literal-destination");
        let result = link_dir(&src, &dst);
        let executed = sentinel.exists();
        let contents = std::fs::read_to_string(dst.join("original.txt"));
        let original = std::fs::read_to_string(src.join("original.txt")).unwrap();
        let _ = std::fs::remove_dir(&dst);
        let _ = std::fs::remove_dir_all(&base);
        assert!(!executed, "a path must never execute the sentinel batch command");
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(contents.unwrap(), "unchanged");
        assert_eq!(original, "unchanged");
    }

    #[cfg(windows)]
    #[test]
    fn security_regression_link_dir_supports_long_unicode_paths() {
        let (base, repo, wt) = fixture("long-junction");
        let mut src = repo.join("雪&%COMSPEC%");
        let mut dst = wt.join("雪&%COMSPEC%");
        for _ in 0..8 {
            src.push("long-directory-component-1234567890");
            dst.push("long-directory-component-1234567890");
        }
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("original.txt"), "unchanged").unwrap();
        let result = link_dir(&src, &dst);
        let contents = std::fs::read_to_string(dst.join("original.txt"));
        let write_result = std::fs::write(dst.join("through-link.txt"), "shared");
        let shared = std::fs::read_to_string(src.join("through-link.txt"));
        let original = std::fs::read_to_string(src.join("original.txt")).unwrap();
        let _ = std::fs::remove_dir(&dst);
        let _ = std::fs::remove_dir_all(&base);
        assert!(result.is_ok(), "{result:?}");
        assert_eq!(contents.unwrap(), "unchanged");
        write_result.unwrap();
        assert_eq!(shared.unwrap(), "shared", "destination must be a junction, not a copy");
        assert_eq!(original, "unchanged");
    }

    #[test]
    fn security_regression_link_dir_preserves_existing_destination() {
        let (base, repo, wt) = fixture("existing-link-destination");
        let dst = wt.join("existing");
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(dst.join("keep.txt"), "destination unchanged").unwrap();
        let result = link_dir(&repo.join("deps"), &dst);
        let kept = std::fs::read_to_string(dst.join("keep.txt")).unwrap();
        let original = std::fs::read_to_string(repo.join("deps/pkg/a.js")).unwrap();
        let _ = std::fs::remove_dir_all(&base);
        assert!(result.is_err(), "an existing directory must not be replaced");
        assert_eq!(kept, "destination unchanged");
        assert_eq!(original, "1");
    }

    #[test]
    fn relative_paths_only() {
        assert_eq!(clean_rel("./.env").as_deref(), Some(".env"));
        assert_eq!(clean_rel(".vscode\\settings.json").as_deref(), Some(".vscode/settings.json"));
        assert_eq!(clean_rel("node_modules/").as_deref(), Some("node_modules"));
        for bad in ["../x", "/etc/passwd", "a/.git/config", "*.env", "!keep", "", "a/./b"] {
            assert!(clean_rel(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn copies_links_and_skips_tracked() {
        let (base, repo, wt) = fixture("copy");
        let out = materialize(repo.clone(), wt.clone(), vec!["tracked.txt".into(), "../x".into()], vec!["deps".into()]);
        assert_eq!(out.copied, vec![".env"]);
        assert_eq!(out.linked, vec!["deps"]);
        assert!(wt.join(".env").is_file());
        assert!(wt.join("deps/pkg/a.js").is_file(), "link resolves");
        assert_eq!(out.skipped.len(), 2, "tracked file and traversal are skipped");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_copy_inside_a_link_never_writes_onto_the_repository() {
        let (base, repo, wt) = fixture("overlap");
        std::fs::write(repo.join("deps/x.txt"), "precious").unwrap();
        let out = materialize(repo.clone(), wt.clone(), vec!["deps/x.txt".into(), "deps/pkg".into()], vec!["deps".into()]);
        assert_eq!(out.linked, vec!["deps"]);
        assert!(out.copied.iter().all(|c| !c.starts_with("deps")), "{:?}", out.copied);
        assert_eq!(std::fs::read_to_string(repo.join("deps/x.txt")).unwrap(), "precious");
        assert!(out.skipped.iter().any(|s| s.path == "deps/x.txt" && s.reason == "already in the worktree"));
        let _ = std::fs::remove_dir_all(&base);

        // A copied folder with a link inside it: the link comes first, the copy finds the folder there.
        let (base, repo, wt) = fixture("overlap2");
        std::fs::create_dir_all(repo.join("deps/inner")).unwrap();
        std::fs::write(repo.join("deps/inner/y.txt"), "keep").unwrap();
        let out = materialize(repo.clone(), wt.clone(), vec!["deps".into()], vec!["deps/inner".into()]);
        assert_eq!((out.linked.clone(), out.copied.clone()), (vec!["deps/inner".to_string()], vec![".env".to_string()]), "only .worktreeinclude's copy");
        assert_eq!(std::fs::read_to_string(repo.join("deps/inner/y.txt")).unwrap(), "keep");
        // The estimate says the same.
        let est = estimate(&repo, vec!["deps".into()], vec!["deps/inner".into()]);
        assert_eq!((est.linked, est.copied), (vec!["deps/inner".to_string()], vec![".env".to_string()]));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_entry_git_cannot_look_at_only_loses_itself() {
        let (base, repo, _wt) = fixture("badentry");
        // A path outside the repository makes git give up on a batch.
        assert!(check_ignore(&repo, &["../outside".to_string(), ".env".to_string()]).is_none());
        let found = ignored_among(&repo, &["../outside".to_string(), ".env".to_string(), "deps".to_string()]);
        assert_eq!(found, [".env".to_string(), "deps".to_string()].into_iter().collect());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_estimate_selects_what_materialize_brings() {
        let (base, repo, wt) = fixture("estimate");
        let est = estimate(&repo, vec!["tracked.txt".into()], vec!["deps".into(), "missing".into()]);
        assert_eq!((est.copied.clone(), est.linked.clone()), (vec![".env".to_string()], vec!["deps".to_string()]));
        assert_eq!((est.copy_bytes, est.copy_entries), (3, 1));
        assert_eq!(est.skipped.len(), 2, "tracked and missing");
        let done = materialize(repo, wt, vec!["tracked.txt".into()], vec!["deps".into(), "missing".into()]);
        assert_eq!((done.copied, done.linked, done.skipped.len()), (est.copied, est.linked, est.skipped.len()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
