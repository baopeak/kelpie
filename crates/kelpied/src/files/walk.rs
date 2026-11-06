//! Walks the items picked for a job and streams what it finds to the copy workers, so a transfer starts before
//! a large folder is fully listed. Only the queue of folders still to list is held, never the whole tree.
//!
//! Links: a picked link copies its target (a picked link to a folder is walked once). Inside a folder, links to
//! files are followed, links to folders are skipped and counted, so a loop cannot happen. Local junctions count
//! as links (see `local_fs`). Names that did not come through as UTF-8 are skipped and counted.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tokio::sync::mpsc;

use super::{FsResult, Kind, Meta, RemoteFs};

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

/// The last part of a path on either end (`/` or `\`).
pub fn base_name(path: &str) -> &str {
    let t = path.trim_end_matches(['/', '\\']);
    t.rsplit(['/', '\\']).next().unwrap_or(t)
}

/// Walks `picked` on `fs`. Stops early (with what was skipped so far) when `cancel` is set or the receiver
/// is gone. A picked item that cannot be read is an error; anything below it is counted instead.
pub async fn walk(fs: &dyn RemoteFs, picked: &[String], tx: &mpsc::Sender<Found>, cancel: &AtomicBool, links: Links) -> FsResult<Skipped> {
    let mut skipped = Skipped::default();
    let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();
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
        let rel = vec![base_name(path).to_string()];
        let meta = if links == Links::Report { fs.lstat(path).await? } else { fs.stat(path).await? };
        match meta.kind {
            Kind::File => send!(Found::File { path: path.clone(), rel, meta }),
            Kind::Dir => {
                send!(Found::Dir { path: path.clone(), rel: rel.clone() });
                queue.push_back((path.clone(), rel));
            }
            Kind::Link => send!(Found::Link { path: path.clone(), rel }),
            Kind::Other => skipped.special += 1,
        }
    }
    while let Some((dir, rel)) = queue.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(skipped);
        }
        let Ok(entries) = fs.list(&dir).await else {
            skipped.unlistable += 1;
            continue;
        };
        for e in entries {
            if !e.readable_name {
                skipped.unreadable_names += 1;
                continue;
            }
            let path = fs.join(&dir, &e.name);
            let mut sub = rel.clone();
            sub.push(e.name);
            match e.meta.kind {
                Kind::File => send!(Found::File { path, rel: sub, meta: e.meta }),
                Kind::Dir => {
                    send!(Found::Dir { path: path.clone(), rel: sub.clone() });
                    queue.push_back((path, sub));
                    skipped.max_queued = skipped.max_queued.max(queue.len());
                }
                Kind::Link if links == Links::Report => send!(Found::Link { path, rel: sub }),
                Kind::Link => match fs.stat(&path).await {
                    Ok(target) if target.kind == Kind::File => send!(Found::File { path, rel: sub, meta: target }),
                    Ok(target) if target.kind == Kind::Dir => skipped.folder_links += 1,
                    Ok(_) => skipped.special += 1,
                    Err(_) => skipped.broken_links += 1,
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
        assert_eq!(base_name("/a/b/c.txt"), "c.txt");
        assert_eq!(base_name(r"C:\Users\me\dir\"), "dir");
        assert_eq!(base_name("/a/b/"), "b");
        assert_eq!(base_name("x"), "x");
    }
}
