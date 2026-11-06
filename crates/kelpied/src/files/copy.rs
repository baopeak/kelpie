//! Copying one file between two ends, and the conflict rules that decide whether it is copied at all.
//!
//! Data goes to a hidden partial beside the target (`.<name>.kelpie-part`). A resume continues from the
//! partial's size when its last 64 KiB equal the source's bytes at the same offset and the source has not
//! changed since. When the data is complete, the target is checked again: it must still be absent, or still be
//! the `(size, mtime)` the person approved to replace. Only then is the partial renamed into place. SFTP v3 has
//! no compare-and-swap, so the check and the rename are milliseconds apart, not atomic (documented).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use super::names::{aside_name, partial_name};
use super::{ErrKind, FsError, FsResult, Kind, Meta, RemoteFs};

/// Pieces handed from the reader to the writer; 32 of them in flight bound the buffer at 8 MiB.
const CHUNK: usize = 256 * 1024;
const IN_FLIGHT: usize = 32;
/// The tail compared before a resume.
pub const TAIL: u64 = 64 * 1024;
/// No byte read for this long: the connection is taken as dropped.
pub const STALL: Duration = Duration::from_secs(60);

/// What a job does when its target exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Ask,
    Replace,
    Skip,
    KeepBoth,
    /// Replace when the source is newer, else skip.
    Newer,
    /// Skip when size and time are the same, else ask.
    SkipIdentical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Copy; `approved` is the target's `(size, mtime)` that may be replaced (`None`: it must stay absent).
    Copy { approved: Option<(u64, u64)> },
    Skip,
    KeepBoth,
    /// A folder onto a folder: its contents are copied into the existing one.
    Merge,
    /// The person decides. `kind_clash`: a file and a folder of the same name (only skip or keep both).
    Ask { identical: bool, kind_clash: bool },
}

fn identical(a: &Meta, b: &Meta) -> bool {
    a.size == b.size && a.mtime == b.mtime
}

/// The decision for a source over what the target folder holds under the same name (`lstat`).
pub fn decide(policy: Policy, src: &Meta, target: Option<&Meta>) -> Decision {
    let Some(t) = target else { return Decision::Copy { approved: None } };
    let (src_dir, t_dir) = (src.kind == Kind::Dir, t.kind == Kind::Dir);
    if src_dir && t_dir {
        return Decision::Merge;
    }
    if src_dir != t_dir {
        return match policy {
            Policy::Skip => Decision::Skip,
            Policy::KeepBoth => Decision::KeepBoth,
            _ => Decision::Ask { identical: false, kind_clash: true },
        };
    }
    let same = identical(src, t);
    let replace = Decision::Copy { approved: Some((t.size, t.mtime)) };
    match policy {
        Policy::Ask => Decision::Ask { identical: same, kind_clash: false },
        Policy::Replace => replace,
        Policy::Skip => Decision::Skip,
        Policy::KeepBoth => Decision::KeepBoth,
        Policy::Newer if src.mtime > t.mtime => replace,
        Policy::Newer => Decision::Skip,
        Policy::SkipIdentical if same => Decision::Skip,
        Policy::SkipIdentical => Decision::Ask { identical: false, kind_clash: false },
    }
}

/// Shared with the queue: progress counted on bytes written, and the person's pause or cancel.
#[derive(Debug, Default)]
pub struct Control {
    /// Bytes accounted for: written, resumed or skipped (the job's progress).
    pub bytes: AtomicU64,
    /// Bytes actually written in this attempt (whether a retry made progress).
    pub written: AtomicU64,
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
    /// Woken on pause or cancel, so a stalled read or write stops at once.
    pub wake: tokio::sync::Notify,
}

impl Control {
    pub fn stop(&self) -> Option<Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            Some(Stop::Cancel)
        } else if self.pause.load(Ordering::Relaxed) {
            Some(Stop::Pause)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The partial is removed.
    Cancel,
    /// The partial is kept for the resume.
    Pause,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Copied {
    /// In place. `resumed_at`: the offset a resume continued from (0 for a fresh copy). `warning`: times or
    /// permissions that could not be kept.
    Done { bytes: u64, resumed_at: u64, warning: Option<String> },
    /// The target changed after the answer: the person is asked again (the partial is kept).
    TargetChanged { now: Meta },
    Stopped(Stop),
}

/// One file to copy.
pub struct FileCopy<'a> {
    pub src: &'a dyn RemoteFs,
    pub src_path: &'a str,
    pub dst: &'a dyn RemoteFs,
    pub dst_dir: &'a str,
    pub dst_name: &'a str,
    /// The job's tag in its hidden partial and moved-aside names.
    pub tag: &'a str,
    /// The target `(size, mtime)` that may be replaced; `None`: the target must be absent.
    pub approved: Option<(u64, u64)>,
    /// The partial beside the target belongs to this job (it is in the journal): it may be resumed.
    pub resume: bool,
    /// The source's `(size, mtime)` when the partial was started; a change means starting again.
    pub source_seen: Option<(u64, u64)>,
    /// Set the target's time (and mode, where the end has modes) from the source.
    pub preserve: bool,
}

async fn read_at(fs: &dyn RemoteFs, path: &str, offset: u64, len: u64) -> FsResult<Vec<u8>> {
    let mut r = fs.open_read(path, offset).await?;
    let mut buf = Vec::with_capacity(len as usize);
    (&mut r).take(len).read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Where a resume may continue: the partial's size when its tail matches the source, else 0.
pub async fn resume_offset(c: &FileCopy<'_>, src: &Meta, partial: &str) -> FsResult<u64> {
    if !c.resume || c.source_seen.is_some_and(|seen| seen != (src.size, src.mtime)) {
        return Ok(0);
    }
    let Ok(p) = c.dst.lstat(partial).await else { return Ok(0) };
    if p.kind != Kind::File || p.size == 0 || p.size > src.size {
        return Ok(0);
    }
    let n = TAIL.min(p.size);
    let at = p.size - n;
    let (a, b) = (read_at(c.src, c.src_path, at, n).await?, read_at(c.dst, partial, at, n).await?);
    Ok(if a.len() as u64 == n && a == b { p.size } else { 0 })
}

fn absent<T>(r: FsResult<T>) -> FsResult<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind == ErrKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// After a stop between the two renames of a replace (a crash, an update, a dropped connection), the old file is
/// only in its moved-aside name. With the target gone it is put back; with the target there and equal to the
/// source (the new one took its place) the old one is removed; anything else is left for the person, untouched.
pub async fn recover_aside(dst: &dyn RemoteFs, dir: &str, name: &str, tag: &str, src: &Meta) -> FsResult<()> {
    let old = dst.join(dir, &aside_name(name, tag));
    if absent(dst.lstat(&old).await)?.is_none() {
        return Ok(());
    }
    let target = dst.join(dir, name);
    match absent(dst.lstat(&target).await)? {
        None => dst.rename(&old, &target, false).await,
        Some(t) if t.kind == Kind::File && t.size == src.size && t.mtime == src.mtime => dst.remove_file(&old).await,
        Some(_) => Ok(()),
    }
}

/// Copies one file as described at the top of this module.
pub async fn copy_file(c: &FileCopy<'_>, ctl: &Control) -> FsResult<Copied> {
    let src = c.src.stat(c.src_path).await?;
    if src.kind != Kind::File {
        return Err(FsError::invalid(format!("{} is not a file", c.src_path)));
    }
    let partial = c.dst.join(c.dst_dir, &partial_name(c.dst_name, c.tag));
    let target = c.dst.join(c.dst_dir, c.dst_name);
    recover_aside(c.dst, c.dst_dir, c.dst_name, c.tag, &src).await?;
    let offset = resume_offset(c, &src, &partial).await?;
    ctl.bytes.fetch_add(offset, Ordering::Relaxed);

    let reader = c.src.open_read(c.src_path, offset).await?;
    // A fresh partial is made new (0600, never through a link someone left there); a resume continues the job's own.
    let mut writer = c.dst.open_write(&partial, offset, true).await?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>(IN_FLIGHT);
    let (read, write) = tokio::join!(read_side(reader, tx, ctl), write_side(&mut writer, rx, ctl));
    let closed = writer.finish(true).await;
    let stopped = match (read, write) {
        (Err(e), _) | (_, Err(e)) => return Err(e),
        (Ok(stop), Ok(_)) => stop.or_else(|| ctl.stop()),
    };
    if let Some(stop) = stopped {
        if stop == Stop::Cancel {
            let _ = c.dst.remove_file(&partial).await;
        }
        return Ok(Copied::Stopped(stop));
    }
    closed?;
    let have = c.dst.lstat(&partial).await?.size;
    let after = c.src.stat(c.src_path).await?;
    if have != src.size || (after.size, after.mtime) != (src.size, src.mtime) {
        // Bytes from two versions of the source: never put in place, never resumed.
        let _ = c.dst.remove_file(&partial).await;
        return Err(FsError::new(ErrKind::Other, "the source changed while it was copied".to_string()));
    }

    // The re-check: replace only what was approved, never what appeared or changed since.
    if let Some(now) = absent(c.dst.lstat(&target).await)? {
        if c.approved != Some((now.size, now.mtime)) || now.kind == Kind::Dir {
            return Ok(Copied::TargetChanged { now });
        }
        replace(c, &partial, &target).await?;
    } else {
        c.dst.rename(&partial, &target, false).await?;
    }

    let mut warning = None;
    if c.preserve {
        if let Err(e) = c.dst.set_times(&target, src.mtime, src.mtime).await {
            if e.kind != ErrKind::Unsupported {
                warning = Some(format!("the time was not kept: {e}"));
            }
        }
        // rwx only: setuid, setgid and sticky bits are never carried over (as OpenSSH's sftp does).
        if let Some(mode) = src.mode {
            if let Err(e) = c.dst.chmod(&target, mode & 0o777).await {
                if e.kind != ErrKind::Unsupported {
                    warning = Some(format!("the permissions were not kept: {e}"));
                }
            }
        }
    }
    Ok(Copied::Done { bytes: src.size, resumed_at: offset, warning })
}

/// Reads the source into the bounded channel until its end, a pause or a cancel (returned), or an error.
async fn read_side(mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>, tx: mpsc::Sender<Vec<u8>>, ctl: &Control) -> FsResult<Option<Stop>> {
    loop {
        if let Some(stop) = ctl.stop() {
            return Ok(Some(stop));
        }
        let mut buf = vec![0u8; CHUNK];
        let n = tokio::select! {
            r = tokio::time::timeout(STALL, reader.read(&mut buf)) => match r {
                Ok(r) => r?,
                Err(_) => return Err(FsError::new(ErrKind::Connection, "no data for 60 s: the connection seems lost")),
            },
            _ = ctl.wake.notified() => continue,
        };
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        tokio::select! {
            sent = tx.send(buf) => if sent.is_err() { return Ok(None) }, // the writer failed; its error is the one reported
            _ = ctl.wake.notified() => continue,
        }
    }
}

/// Writes what the reader hands over, counting progress on bytes written. Ends when the reader is done, or at once on
/// a pause or cancel (what is written so far stays in the partial).
async fn write_side(writer: &mut Box<dyn super::WriteFile>, mut rx: mpsc::Receiver<Vec<u8>>, ctl: &Control) -> FsResult<u64> {
    let mut written = 0u64;
    loop {
        let buf = tokio::select! {
            b = rx.recv() => match b { Some(b) => b, None => return Ok(written) },
            _ = ctl.wake.notified() => return Ok(written),
        };
        if ctl.stop().is_some() {
            return Ok(written);
        }
        tokio::select! {
            r = writer.write_all(&buf) => r?,
            _ = ctl.wake.notified() => return Ok(written),
        }
        written += buf.len() as u64;
        ctl.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
        ctl.written.fetch_add(buf.len() as u64, Ordering::Relaxed);
    }
}

/// Renames the partial over an approved target: in one step where the end can, else the old file is moved aside
/// (under the job's own name) first and put back if the new one cannot take its place. A stop between the two
/// renames is repaired by `recover_aside` on the next attempt.
async fn replace(c: &FileCopy<'_>, partial: &str, target: &str) -> FsResult<()> {
    match c.dst.rename(partial, target, true).await {
        Err(e) if e.kind == ErrKind::Unsupported => {}
        other => return other,
    }
    // A stale moved-aside file (left by a stop `recover_aside` could not judge) is never removed: another name is used.
    let mut old = c.dst.join(c.dst_dir, &aside_name(c.dst_name, c.tag));
    let mut n = 1;
    while absent(c.dst.lstat(&old).await)?.is_some() {
        n += 1;
        old = c.dst.join(c.dst_dir, &format!("{}.{n}", aside_name(c.dst_name, c.tag)));
    }
    c.dst.rename(target, &old, false).await?;
    if let Err(e) = c.dst.rename(partial, target, false).await {
        let _ = c.dst.rename(&old, target, false).await;
        return Err(e);
    }
    let _ = c.dst.remove_file(&old).await;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::mem_fs::MemFs;

    fn meta(kind: Kind, size: u64, mtime: u64) -> Meta {
        Meta { kind, size, mtime, mode: Some(0o644), uid: None, gid: None }
    }

    #[test]
    fn conflict_table() {
        use Decision as D;
        let src = meta(Kind::File, 10, 100);
        let file_same = meta(Kind::File, 10, 100);
        let older = meta(Kind::File, 5, 50);
        let newer = meta(Kind::File, 5, 500);
        let folder = meta(Kind::Dir, 0, 1);
        let ask = |identical| D::Ask { identical, kind_clash: false };
        let clash = D::Ask { identical: false, kind_clash: true };
        let replaces = |m: &Meta| D::Copy { approved: Some((m.size, m.mtime)) };
        let fresh = D::Copy { approved: None };
        let rows: [(Policy, [Decision; 5]); 6] = [
            // absent, identical, older target, newer target, folder target
            (Policy::Ask, [fresh.clone(), ask(true), ask(false), ask(false), clash.clone()]),
            (Policy::Replace, [fresh.clone(), replaces(&file_same), replaces(&older), replaces(&newer), clash.clone()]),
            (Policy::Skip, [fresh.clone(), D::Skip, D::Skip, D::Skip, D::Skip]),
            (Policy::KeepBoth, [fresh.clone(), D::KeepBoth, D::KeepBoth, D::KeepBoth, D::KeepBoth]),
            (Policy::Newer, [fresh.clone(), D::Skip, replaces(&older), D::Skip, clash.clone()]),
            (Policy::SkipIdentical, [fresh.clone(), D::Skip, ask(false), ask(false), clash.clone()]),
        ];
        for (policy, want) in rows {
            let got = [None, Some(&file_same), Some(&older), Some(&newer), Some(&folder)].map(|t| decide(policy, &src, t));
            assert_eq!(got, want, "{policy:?}");
        }
        assert_eq!(decide(Policy::Replace, &folder, Some(&folder)), D::Merge);
        assert_eq!(decide(Policy::Replace, &folder, Some(&older)), clash);
    }

    fn job<'a>(src: &'a MemFs, dst: &'a MemFs, approved: Option<(u64, u64)>) -> FileCopy<'a> {
        FileCopy { src, src_path: "/src/big.bin", dst, dst_dir: "/dst", dst_name: "big.bin", tag: "t1", approved, resume: true, source_seen: None, preserve: true }
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    #[tokio::test]
    async fn copies_into_place_with_time_and_mode() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        src.put("/src/big.bin", &data(1_000_000), 1234);
        dst.mkdir_all("/dst");
        let out = copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap();
        assert_eq!(out, Copied::Done { bytes: 1_000_000, resumed_at: 0, warning: None });
        assert_eq!(dst.read("/dst/big.bin").unwrap(), data(1_000_000));
        assert!(!dst.exists("/dst/.big.bin.t1.kelpie-part"));
        assert_eq!(dst.stat("/dst/big.bin").await.unwrap().mtime, 1234);
    }

    #[tokio::test]
    async fn resumes_from_a_matching_tail_and_restarts_otherwise() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        let all = data(500_000);
        src.put("/src/big.bin", &all, 1);
        dst.put("/dst/.big.bin.t1.kelpie-part", &all[..300_000], 1);
        let ctl = Control::default();
        let out = copy_file(&job(&src, &dst, None), &ctl).await.unwrap();
        assert!(matches!(out, Copied::Done { resumed_at: 300_000, .. }), "{out:?}");
        assert_eq!(dst.read("/dst/big.bin").unwrap(), all);

        // A partial whose tail differs starts again from 0.
        let mut wrong = all[..300_000].to_vec();
        wrong[299_999] ^= 0xff;
        dst.remove_file("/dst/big.bin").await.unwrap();
        dst.put("/dst/.big.bin.t1.kelpie-part", &wrong, 1);
        let out = copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap();
        assert!(matches!(out, Copied::Done { resumed_at: 0, .. }), "{out:?}");
        assert_eq!(dst.read("/dst/big.bin").unwrap(), all);

        // A source that changed since the partial was started starts again too.
        dst.remove_file("/dst/big.bin").await.unwrap();
        dst.put("/dst/.big.bin.t1.kelpie-part", &all[..300_000], 1);
        let mut j = job(&src, &dst, None);
        j.source_seen = Some((all.len() as u64, 999));
        assert!(matches!(copy_file(&j, &Control::default()).await.unwrap(), Copied::Done { resumed_at: 0, .. }));

        // A partial that is not the job's own is never continued.
        dst.remove_file("/dst/big.bin").await.unwrap();
        dst.put("/dst/.big.bin.t1.kelpie-part", &all[..300_000], 1);
        let mut j = job(&src, &dst, None);
        j.resume = false;
        assert!(matches!(copy_file(&j, &Control::default()).await.unwrap(), Copied::Done { resumed_at: 0, .. }));
    }

    #[tokio::test]
    async fn a_target_that_appeared_or_changed_is_asked_about_again() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        src.put("/src/big.bin", &data(1000), 1);
        dst.put("/dst/big.bin", b"someone else's", 77);
        let out = copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap();
        assert!(matches!(out, Copied::TargetChanged { .. }), "{out:?}");
        assert_eq!(dst.read("/dst/big.bin").unwrap(), b"someone else's");
        assert!(dst.exists("/dst/.big.bin.t1.kelpie-part"), "kept for the answer");

        // Approved as it was: replaced. Approved but changed since: asked again.
        let approved = Some((14, 77));
        assert!(matches!(copy_file(&job(&src, &dst, approved), &Control::default()).await.unwrap(), Copied::Done { .. }));
        assert_eq!(dst.read("/dst/big.bin").unwrap(), data(1000));
        dst.put("/dst/big.bin", b"changed", 78);
        assert!(matches!(copy_file(&job(&src, &dst, approved), &Control::default()).await.unwrap(), Copied::TargetChanged { .. }));
    }

    #[tokio::test]
    async fn replaces_without_posix_rename_by_moving_the_old_file_aside() {
        let src = MemFs::new();
        let dst = MemFs::new().without_posix_rename();
        src.put("/src/big.bin", &data(1000), 1);
        dst.put("/dst/big.bin", b"old", 5);
        let out = copy_file(&job(&src, &dst, Some((3, 5))), &Control::default()).await.unwrap();
        assert!(matches!(out, Copied::Done { .. }), "{out:?}");
        assert_eq!(dst.read("/dst/big.bin").unwrap(), data(1000));
        assert!(dst.paths().iter().all(|p| !p.contains("kelpie-old") && !p.contains("kelpie-part")), "{:?}", dst.paths());
    }

    #[tokio::test]
    async fn a_replace_cut_between_its_renames_is_repaired_on_the_next_attempt() {
        let src = MemFs::new();
        let dst = MemFs::new().without_posix_rename();
        src.put("/src/big.bin", &data(1000), 1);
        dst.put("/dst/big.bin", b"old", 5);
        // The second rename (partial -> target) fails: the old file is put back.
        dst.fail_rename_to(Some("/dst/big.bin"));
        assert!(copy_file(&job(&src, &dst, Some((3, 5))), &Control::default()).await.is_err());
        assert_eq!(dst.read("/dst/big.bin").unwrap(), b"old");
        // As if the daemon stopped between the renames: only the moved-aside copy is left.
        dst.fail_rename_to(None);
        dst.rename("/dst/big.bin", "/dst/.big.bin.t1.kelpie-old", false).await.unwrap();
        recover_aside(&dst, "/dst", "big.bin", "t1", &src.stat("/src/big.bin").await.unwrap()).await.unwrap();
        assert_eq!(dst.read("/dst/big.bin").unwrap(), b"old", "the person's file is back");
        assert!(!dst.exists("/dst/.big.bin.t1.kelpie-old"));
        // And a later replace is not blocked by a stale moved-aside name.
        dst.put("/dst/.big.bin.t1.kelpie-old", b"stale", 1);
        assert!(matches!(copy_file(&job(&src, &dst, Some((3, 5))), &Control::default()).await.unwrap(), Copied::Done { .. }));
        assert_eq!(dst.read("/dst/big.bin").unwrap(), data(1000));
    }

    #[tokio::test]
    async fn a_source_rewritten_during_the_copy_is_never_put_in_place() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        src.put("/src/big.bin", &data(1000), 1);
        dst.mkdir_all("/dst");
        src.bump_mtime_on_read("/src/big.bin");
        let err = copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap_err();
        assert!(err.message.contains("changed while it was copied"), "{err:?}");
        assert!(!dst.exists("/dst/big.bin") && !dst.exists("/dst/.big.bin.t1.kelpie-part"));
    }

    #[tokio::test]
    async fn partials_are_private_and_special_bits_never_travel() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        src.put("/src/big.bin", &data(10), 1);
        src.chmod("/src/big.bin", 0o4755).await.unwrap();
        dst.mkdir_all("/dst");
        let ctl = Control::default();
        ctl.pause.store(true, Ordering::Relaxed);
        copy_file(&job(&src, &dst, None), &ctl).await.unwrap();
        assert_eq!(dst.lstat("/dst/.big.bin.t1.kelpie-part").await.unwrap().mode, Some(0o600));
        copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap();
        assert_eq!(dst.stat("/dst/big.bin").await.unwrap().mode, Some(0o755));
    }

    #[tokio::test]
    async fn cancel_removes_the_partial_and_pause_keeps_it() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        src.put("/src/big.bin", &data(1000), 1);
        dst.mkdir_all("/dst");
        let ctl = Control::default();
        ctl.pause.store(true, Ordering::Relaxed);
        assert_eq!(copy_file(&job(&src, &dst, None), &ctl).await.unwrap(), Copied::Stopped(Stop::Pause));
        assert!(dst.exists("/dst/.big.bin.t1.kelpie-part"));
        let ctl = Control::default();
        ctl.cancel.store(true, Ordering::Relaxed);
        assert_eq!(copy_file(&job(&src, &dst, None), &ctl).await.unwrap(), Copied::Stopped(Stop::Cancel));
        assert!(!dst.exists("/dst/.big.bin.t1.kelpie-part"));
        assert!(!dst.exists("/dst/big.bin"));
    }

    #[tokio::test]
    async fn a_dropped_read_keeps_the_partial_for_the_resume() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        let all = data(800_000);
        src.put("/src/big.bin", &all, 1);
        dst.mkdir_all("/dst");
        src.fail_reads_after(Some(300_000));
        let err = copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap_err();
        assert_eq!(err.kind, ErrKind::Connection);
        let kept = dst.read("/dst/.big.bin.t1.kelpie-part").unwrap();
        assert_eq!(kept, all[..kept.len()]);
        src.fail_reads_after(None);
        let out = copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap();
        assert!(matches!(out, Copied::Done { resumed_at, .. } if resumed_at == kept.len() as u64 && resumed_at > 0), "{out:?}");
        assert_eq!(dst.read("/dst/big.bin").unwrap(), all);
    }
}
