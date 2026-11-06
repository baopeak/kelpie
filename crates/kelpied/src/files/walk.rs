//! Walks the items picked for a job and streams what it finds to the copy workers, so a transfer starts before
//! a large folder is fully listed. Only the queue of folders still to list is held, never the whole tree.
//!
//! Links: a picked link copies its target (a picked link to a folder is walked once). Inside a folder, links to
//! files are followed, links to folders are skipped and counted, so a loop cannot happen. Local junctions count
//! as links (see `local_fs`). Names that did not come through as UTF-8 are skipped and counted. A folder's names
//! are walked in name order, so every attempt of a job meets two sources for one target in the same order.
//!
//! A delete or chmod checks each folder right before it is listed and acted on; SFTP has no `openat`-style calls,
//! so a co-user who can write the parent could still swap a checked folder for a link in between (documented).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tokio::sync::mpsc;

use super::{ErrKind, FsError, FsResult, Kind, Meta, RemoteFs};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// A folder to make on the target (path parts below the target folder), sent before its contents.
    Dir { path: String, rel: Vec<String> },
    File { path: String, rel: Vec<String>, meta: Meta },
    /// A link itself, not followed ([`Links::Report`] only).
    Link { path: String, rel: Vec<String> },
}

/// What happens to links on the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Links {
    /// A copy: links to files are followed, links to folders skipped (a picked link is followed).
    Follow,
    /// A delete or chmod: every link is reported as itself and never followed.
    Report,
}

/// What the walk skipped, for the job's summary ("3 links to folders not followed").
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Skipped {
    pub folder_links: u64,
    pub broken_links: u64,
    pub unreadable_names: u64,
    /// Devices, sockets, pipes.
    pub special: u64,
    /// Folders that could not be listed (permission denied, gone meanwhile).
    pub unlistable: u64,
    /// The longest the folder queue grew: the walk's memory, for the tests.
    #[serde(skip)]
    pub max_queued: usize,
}

/// The last part of a path: on a host and on macOS / Linux `/` only (`\` is an ordinary character in a name there),
/// on Windows `\` too.
pub fn base_name(path: &str, windows: bool) -> &str {
    let seps: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let t = path.trim_end_matches(seps);
    t.rsplit(seps).next().unwrap_or(t)
}

fn parent_of(fs: &dyn RemoteFs, path: &str) -> String {
    if fs.windows_names() {
        std::path::Path::new(path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        super::parent_remote(path).to_string()
    }
}

/// Errors that end a walk: anything but a missing or forbidden path (a lost connection above all) would otherwise
/// leave part of the tree out of a job that then reports itself done.
fn fatal(e: FsError) -> FsResult<()> {
    match e.kind {
        ErrKind::NotFound | ErrKind::Denied => Ok(()),
        _ => Err(e),
    }
}

/// Whether `path` (named `name` in a folder whose real path is `parent_real`) is a folder of its own and not a link to
/// one. Some servers (the e2e fixture among them) report a link's target in listings and even in `lstat`; the real
/// path then differs from where the name sits, which catches them. Where `lstat` is exact (this computer) it is
/// enough. A host that cannot give the real path: a copy trusts `lstat`, a delete or chmod does not go in.
async fn real_dir(fs: &dyn RemoteFs, path: &str, parent_real: &str, name: &str, links: Links) -> FsResult<Option<String>> {
    match fs.lstat(path).await {
        Ok(m) if m.kind == Kind::Dir => {}
        Ok(_) => return Ok(None),
        Err(e) => return fatal(e).map(|_| None),
    }
    if fs.lstat_is_exact() {
        return Ok(Some(fs.join(parent_real, name)));
    }
    match fs.realpath(path).await {
        Ok(real) if real == fs.join(parent_real, name) => Ok(Some(real)),
        Ok(_) => Ok(None),
        Err(e) if matches!(e.kind, ErrKind::NotFound | ErrKind::Denied | ErrKind::Connection) => fatal(e).map(|_| None),
        Err(_) if links == Links::Follow => Ok(Some(fs.join(parent_real, name))),
        Err(_) => Ok(None),
    }
}

/// How many folder checks run at once on a host (each is two round trips).
const CHECKS_AT_ONCE: usize = 8;

/// Runs `futs` together and returns their results in order (a tiny `join_all`).
async fn join_all<F: std::future::Future>(futs: Vec<F>) -> Vec<F::Output> {
    let mut futs: Vec<std::pin::Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = futs.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (f, o) in futs.iter_mut().zip(out.iter_mut()) {
            if o.is_none() {
                match f.as_mut().poll(cx) {
                    std::task::Poll::Ready(v) => *o = Some(v),
                    std::task::Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            std::task::Poll::Pending
        } else {
            std::task::Poll::Ready(())
        }
    })
    .await;
    out.into_iter().map(|o| o.expect("every future finished")).collect()
}

/// Walks `picked` on `fs`. Stops early (with what was skipped so far) when `cancel` is set or the receiver is gone.
/// A picked item that cannot be read is an error; below it a missing or forbidden entry is counted, anything else
/// (a lost connection) ends the walk with that error.
pub async fn walk(fs: &dyn RemoteFs, picked: &[String], tx: &mpsc::Sender<Found>, cancel: &AtomicBool, links: Links) -> FsResult<Skipped> {
    let mut skipped = Skipped::default();
    // Folders still to list: (path, parts below the target, real path).
    let mut queue: VecDeque<(String, Vec<String>, String)> = VecDeque::new();
    macro_rules! send {
        ($item:expr) => {
            if tx.send($item).await.is_err() {
                return Ok(skipped);
            }
        };
    }
    for path in picked {
        if cancel.load(Ordering::Relaxed) {
            return Ok(skipped);
        }
        let name = base_name(path, fs.windows_names());
        if !super::readable_name(name) {
            return Err(FsError::invalid(format!("{path} cannot be transferred by name")));
        }
        let rel = vec![name.to_string()];
        let meta = if links == Links::Report { fs.lstat(path).await? } else { fs.stat(path).await? };
        match meta.kind {
            Kind::File => send!(Found::File { path: path.clone(), rel, meta }),
            Kind::Dir => {
                let real = if links == Links::Report {
                    // A delete or chmod acts on a picked link itself, even on a server whose lstat follows links.
                    match fs.realpath(&parent_of(fs, path)).await {
                        Ok(p) => real_dir(fs, path, &p, name, links).await?,
                        Err(e) if e.kind == ErrKind::Connection => return Err(e),
                        // No real path to check with: not gone into.
                        Err(_) if !fs.lstat_is_exact() => None,
                        Err(_) => Some(path.clone()),
                    }
                } else if fs.lstat_is_exact() {
                    Some(path.clone())
                } else {
                    // A copy follows a picked link; where the host cannot say the real path, the picked path stands in.
                    match fs.realpath(path).await {
                        Ok(p) => Some(p),
                        Err(e) if e.kind == ErrKind::Connection => return Err(e),
                        Err(_) => Some(path.clone()),
                    }
                };
                match real {
                    Some(real) => {
                        send!(Found::Dir { path: path.clone(), rel: rel.clone() });
                        queue.push_back((path.clone(), rel, real));
                    }
                    None => send!(Found::Link { path: path.clone(), rel }),
                }
            }
            Kind::Link => send!(Found::Link { path: path.clone(), rel }),
            Kind::Other => skipped.special += 1,
        }
    }
    while let Some((dir, rel, real)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(skipped);
        }
        let mut entries = match fs.list(&dir).await {
            Ok(entries) => entries,
            Err(e) => {
                fatal(e)?;
                skipped.unlistable += 1;
                continue;
            }
        };
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        // The folders' checks, a few at a time (on a host each costs round trips), in listing order.
        let mut checks: std::collections::HashMap<usize, FsResult<Option<String>>> = std::collections::HashMap::new();
        let dirs: Vec<usize> = entries.iter().enumerate().filter(|(_, e)| e.readable_name && e.meta.kind == Kind::Dir).map(|(i, _)| i).collect();
        for chunk in dirs.chunks(CHECKS_AT_ONCE) {
            let paths: Vec<String> = chunk.iter().map(|&i| fs.join(&dir, &entries[i].name)).collect();
            let found = join_all(chunk.iter().zip(&paths).map(|(&i, p)| real_dir(fs, p, &real, &entries[i].name, links)).collect()).await;
            checks.extend(chunk.iter().copied().zip(found));
        }
        for (i, e) in entries.into_iter().enumerate() {
            if !e.readable_name {
                skipped.unreadable_names += 1;
                continue;
            }
            let path = fs.join(&dir, &e.name);
            let mut sub = rel.clone();
            sub.push(e.name.clone());
            match e.meta.kind {
                Kind::File => send!(Found::File { path, rel: sub, meta: e.meta }),
                Kind::Dir => match checks.remove(&i).expect("checked above")? {
                    Some(sub_real) => {
                        send!(Found::Dir { path: path.clone(), rel: sub.clone() });
                        queue.push_back((path, sub, sub_real));
                        skipped.max_queued = skipped.max_queued.max(queue.len());
                    }
                    // A link the listing showed as a folder.
                    None if links == Links::Report => send!(Found::Link { path, rel: sub }),
                    None => skipped.folder_links += 1,
                },
                Kind::Link if links == Links::Report => send!(Found::Link { path, rel: sub }),
                Kind::Link => match fs.stat(&path).await {
                    Ok(target) if target.kind == Kind::File => send!(Found::File { path, rel: sub, meta: target }),
                    Ok(target) if target.kind == Kind::Dir => skipped.folder_links += 1,
                    Ok(_) => skipped.special += 1,
                    Err(e) => {
                        fatal(e)?;
                        skipped.broken_links += 1;
                    }
                },
                Kind::Other => skipped.special += 1,
            }
        }
    }
    Ok(skipped)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::mem_fs::MemFs;

    async fn collect(fs: &MemFs, picked: &[&str]) -> (Vec<Found>, Skipped) {
        let (tx, mut rx) = mpsc::channel(64);
        let picked: Vec<String> = picked.iter().map(|s| s.to_string()).collect();
        let cancel = AtomicBool::new(false);
        let fs2 = fs.clone();
        let walker = tokio::spawn(async move { walk(&fs2, &picked, &tx, &cancel, Links::Follow).await });
        let mut found = Vec::new();
        while let Some(f) = rx.recv().await {
            found.push(f);
        }
        (found, walker.await.unwrap().unwrap())
    }

    fn rels(found: &[Found]) -> Vec<String> {
        let mut v: Vec<String> = found
            .iter()
            .map(|f| match f {
                Found::Dir { rel, .. } => format!("{}/", rel.join("/")),
                Found::Link { rel, .. } => format!("{}@", rel.join("/")),
                Found::File { rel, .. } => rel.join("/"),
            })
            .collect();
        v.sort();
        v
    }

    #[tokio::test]
    async fn follows_file_links_and_skips_folder_links() {
        let fs = MemFs::new();
        fs.put("/p/a.txt", b"a", 1);
        fs.put("/p/sub/b.txt", b"bb", 1);
        fs.put("/elsewhere/c.txt", b"ccc", 1);
        fs.link("/p/to-c", "/elsewhere/c.txt");
        fs.link("/p/to-elsewhere", "/elsewhere");
        fs.link("/p/broken", "/nowhere");
        let (found, skipped) = collect(&fs, &["/p"]).await;
        assert_eq!(rels(&found), ["p/", "p/a.txt", "p/sub/", "p/sub/b.txt", "p/to-c"]);
        assert_eq!((skipped.folder_links, skipped.broken_links), (1, 1));
        let to_c = found.iter().find_map(|f| match f {
            Found::File { rel, meta, .. } if rel.last().unwrap() == "to-c" => Some(meta.size),
            _ => None,
        });
        assert_eq!(to_c, Some(3), "a followed link carries its target's size");
    }

    #[tokio::test]
    async fn a_picked_folder_link_is_walked_once() {
        let fs = MemFs::new();
        fs.put("/real/x.txt", b"x", 1);
        fs.link("/real/loop", "/real");
        fs.link("/pick", "/real");
        let (found, skipped) = collect(&fs, &["/pick"]).await;
        assert_eq!(rels(&found), ["pick/", "pick/x.txt"]);
        assert_eq!(skipped.folder_links, 1);
    }

    #[tokio::test]
    async fn unreadable_names_are_counted() {
        let fs = MemFs::new();
        fs.put("/d/ok.txt", b"", 1);
        fs.put("/d/bad\u{FFFD}.txt", b"", 1);
        let (found, skipped) = collect(&fs, &["/d"]).await;
        assert_eq!(rels(&found), ["d/", "d/ok.txt"]);
        assert_eq!(skipped.unreadable_names, 1);
    }

    #[tokio::test]
    async fn many_files_stream_with_a_small_folder_queue() {
        let fs = MemFs::new();
        for d in 0..20 {
            for f in 0..1000 {
                fs.put(&format!("/big/d{d}/f{f}"), b"", 1);
            }
        }
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = AtomicBool::new(false);
        let fs2 = fs.clone();
        let walker = tokio::spawn(async move { walk(&fs2, &["/big".to_string()], &tx, &cancel, Links::Follow).await });
        let mut files = 0;
        while let Some(f) = rx.recv().await {
            if matches!(f, Found::File { .. }) {
                files += 1;
            }
        }
        let skipped = walker.await.unwrap().unwrap();
        assert_eq!(files, 20_000);
        assert!(skipped.max_queued <= 20, "only folders are queued, not files: {}", skipped.max_queued);
    }

    #[tokio::test]
    async fn a_delete_walk_reports_links_without_following_them() {
        let fs = MemFs::new();
        fs.put("/d/a.txt", b"a", 1);
        fs.put("/elsewhere/keep.txt", b"k", 1);
        fs.link("/d/to-elsewhere", "/elsewhere");
        fs.link("/d/to-file", "/elsewhere/keep.txt");
        let (tx, mut rx) = mpsc::channel(16);
        let cancel = AtomicBool::new(false);
        let fs2 = fs.clone();
        let walker = tokio::spawn(async move { walk(&fs2, &["/d".to_string()], &tx, &cancel, Links::Report).await });
        let mut found = Vec::new();
        while let Some(f) = rx.recv().await {
            found.push(f);
        }
        walker.await.unwrap().unwrap();
        assert_eq!(rels(&found), ["d/", "d/a.txt", "d/to-elsewhere@", "d/to-file@"]);
    }

    async fn walk_all(fs: &MemFs, picked: &str, links: Links) -> (Vec<Found>, FsResult<Skipped>) {
        let (tx, mut rx) = mpsc::channel(64);
        let cancel = AtomicBool::new(false);
        let (fs2, picked) = (fs.clone(), vec![picked.to_string()]);
        let walker = tokio::spawn(async move { walk(&fs2, &picked, &tx, &cancel, links).await });
        let mut found = Vec::new();
        while let Some(f) = rx.recv().await {
            found.push(f);
        }
        (found, walker.await.unwrap())
    }

    #[tokio::test]
    async fn a_connection_lost_while_listing_ends_the_walk_with_that_error() {
        let fs = MemFs::new();
        for d in 0..5 {
            fs.put(&format!("/t/d{d}/f"), b"x", 1);
        }
        fs.fail_lists_after(Some(2));
        let (_, r) = walk_all(&fs, "/t", Links::Follow).await;
        assert_eq!(r.unwrap_err().kind, ErrKind::Connection, "never a walk that reports itself complete");
    }

    #[tokio::test]
    async fn servers_whose_listing_or_lstat_follow_links_are_never_followed_into() {
        for lstat_too in [false, true] {
            let fs = MemFs::new().following_links(lstat_too);
            fs.put("/d/a.txt", b"a", 1);
            fs.put("/outside/keep.txt", b"k", 1);
            fs.link("/d/to-outside", "/outside");
            let (found, r) = walk_all(&fs, "/d", Links::Report).await;
            r.unwrap();
            assert_eq!(rels(&found), ["d/", "d/a.txt", "d/to-outside@"], "lstat follows: {lstat_too}");
            let (found, r) = walk_all(&fs, "/d", Links::Follow).await;
            assert_eq!(rels(&found), ["d/", "d/a.txt"], "lstat follows: {lstat_too}");
            assert_eq!(r.unwrap().folder_links, 1);
        }
        // A picked link, on a server whose lstat follows it, is acted on as the link itself.
        let fs = MemFs::new().following_links(true);
        fs.put("/outside/keep.txt", b"k", 1);
        fs.link("/pick", "/outside");
        let (found, _) = walk_all(&fs, "/pick", Links::Report).await;
        assert_eq!(rels(&found), ["pick@"]);
    }

    #[tokio::test]
    async fn without_real_paths_a_copy_still_walks_and_a_delete_does_not_go_in() {
        // A server whose lstat follows links and that cannot give real paths either.
        let fs = MemFs::new().following_links(true).without_realpath();
        fs.put("/d/a.txt", b"a", 1);
        fs.put("/d/sub/b.txt", b"b", 1);
        let (found, r) = walk_all(&fs, "/d", Links::Follow).await;
        r.unwrap();
        assert_eq!(rels(&found), ["d/", "d/a.txt", "d/sub/", "d/sub/b.txt"], "a plain copy does not fail");
        let (found, r) = walk_all(&fs, "/d", Links::Report).await;
        r.unwrap();
        assert_eq!(rels(&found), ["d@"], "nothing is deleted or changed inside a folder that cannot be checked");
        // This computer's lstat never follows: no real path is asked for at all.
        assert!(crate::files::local_fs::LocalFs.lstat_is_exact());
    }

    #[tokio::test]
    async fn a_folders_names_come_in_name_order() {
        let fs = MemFs::new();
        for n in ["c.txt", "a.txt", "B.txt", "b"] {
            fs.put(&format!("/o/{n}"), b"", 1);
        }
        let (found, _) = walk_all(&fs, "/o", Links::Follow).await;
        let names: Vec<String> = found.iter().filter_map(|f| if let Found::File { rel, .. } = f { Some(rel[1].clone()) } else { None }).collect();
        assert_eq!(names, ["B.txt", "a.txt", "b", "c.txt"]);
    }

    #[tokio::test]
    async fn refuses_picked_items_without_a_name() {
        let fs = MemFs::new();
        for bad in ["/", "/home/.."] {
            let (_, r) = walk_all(&fs, bad, Links::Report).await;
            assert_eq!(r.unwrap_err().kind, ErrKind::Invalid, "{bad}");
        }
    }

    #[tokio::test]
    async fn cancel_stops_the_walk() {
        let fs = MemFs::new();
        fs.put("/c/a/1", b"", 1);
        let (tx, _rx) = mpsc::channel(16);
        let cancel = AtomicBool::new(true);
        assert_eq!(walk(&fs, &["/c".into()], &tx, &cancel, Links::Follow).await.unwrap(), Skipped::default());
    }

    #[test]
    fn base_names() {
        assert_eq!(base_name("/a/b/c.txt", false), "c.txt");
        assert_eq!(base_name(r"C:\Users\me\dir\", true), "dir");
        assert_eq!(base_name("/a/b/", false), "b");
        assert_eq!(base_name("x", false), "x");
        assert_eq!(base_name("/srv/a\\b", false), "a\\b", "a backslash is part of a host name");
    }
}
