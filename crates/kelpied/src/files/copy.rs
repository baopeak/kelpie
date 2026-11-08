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

/// Shared with the queue: progress, the room on the target, and the person's pause or cancel.
#[derive(Debug)]
pub struct Control {
    /// Bytes accounted for: written, resumed or skipped (the job's progress bar).
    pub bytes: AtomicU64,
    /// Bytes the target holds for sure in this attempt: resumed from, already complete, or written and closed (a
    /// host confirms a write only some 64 writes after it was sent). Whether a retry made progress.
    pub confirmed: AtomicU64,
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
    /// The attempt ends for another reason (a file lost the connection): the other files stop as on a pause and keep
    /// their partials.
    pub halted: AtomicBool,
    /// Free bytes on the target when the attempt started (`u64::MAX`: unknown), and what this attempt's files still
    /// have to write.
    pub free: AtomicU64,
    pub needed: AtomicU64,
    /// Woken on pause, cancel or halt, so a stalled read or write stops at once.
    pub wake: tokio::sync::Notify,
}

impl Default for Control {
    fn default() -> Self {
        Control {
            bytes: AtomicU64::new(0),
            confirmed: AtomicU64::new(0),
            cancel: AtomicBool::new(false),
            pause: AtomicBool::new(false),
            halted: AtomicBool::new(false),
            free: AtomicU64::new(u64::MAX),
            needed: AtomicU64::new(0),
            wake: tokio::sync::Notify::new(),
        }
    }
}

impl Control {
    pub fn stop(&self) -> Option<Stop> {
        if self.cancel.load(Ordering::Relaxed) {
            Some(Stop::Cancel)
        } else if self.pause.load(Ordering::Relaxed) || self.halted.load(Ordering::Relaxed) {
            Some(Stop::Pause)
        } else {
            None
        }
    }

    /// Room for `n` more bytes on the target: counted when a file's offset is known, so what is already there
    /// (complete, skipped, resumed) never needs room. False when the target had less free when the attempt started.
    pub fn reserve(&self, n: u64) -> bool {
        let need = self.needed.fetch_add(n, Ordering::Relaxed).saturating_add(n);
        need <= self.free.load(Ordering::Relaxed)
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
    /// The target has no room for what is left of this file (when the attempt started). The partial is kept.
    NoRoom { free: u64 },
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
    /// Where an earlier attempt moved the old target aside (in the journal): only that is ever repaired.
    pub aside: Option<&'a str>,
    /// Records the moved-aside name before the move (`None` once the replace is through), in the journal.
    pub record_aside: &'a (dyn Fn(Option<String>) + Send + Sync),
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
/// only in its moved-aside name, which the journal recorded. With the target gone it is put back; with the target
/// there and equal to the source (the new one took its place) the old one is removed; anything else is left for the
/// person, untouched. Nothing is looked up when no move-aside was recorded.
pub async fn recover_aside(c: &FileCopy<'_>, src: &Meta) -> FsResult<()> {
    let Some(old) = c.aside else { return Ok(()) };
    if absent(c.dst.lstat(old).await)?.is_none() {
        (c.record_aside)(None);
        return Ok(());
    }
    let target = c.dst.join(c.dst_dir, c.dst_name);
    match absent(c.dst.lstat(&target).await)? {
        None => c.dst.rename(old, &target, false).await?,
        Some(t) if t.kind == Kind::File && t.size == src.size && t.mtime == src.mtime => c.dst.remove_file(old).await?,
        Some(_) => {}
    }
    (c.record_aside)(None);
    Ok(())
}

/// Copies one file as described at the top of this module.
pub async fn copy_file(c: &FileCopy<'_>, ctl: &Control) -> FsResult<Copied> {
    let src = c.src.stat(c.src_path).await?;
    if src.kind != Kind::File {
        return Err(FsError::invalid(format!("{} is not a file", c.src_path)));
    }
    let partial = c.dst.join(c.dst_dir, &partial_name(c.dst_name, c.tag));
    recover_aside(c, &src).await?;
    let offset = resume_offset(c, &src, &partial).await?;
    if !ctl.reserve(src.size - offset) {
        return Ok(Copied::NoRoom { free: ctl.free.load(Ordering::Relaxed) });
    }
    ctl.bytes.fetch_add(offset, Ordering::Relaxed);
    ctl.confirmed.fetch_add(offset, Ordering::Relaxed);

    let reader = c.src.open_read(c.src_path, offset).await?;
    // A fresh partial is made new (0600, never through a link someone left there); a resume continues the job's own.
    let mut writer = c.dst.open_write(&partial, offset, true).await?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>(IN_FLIGHT);
    let (read, write) = tokio::join!(read_side(reader, tx, ctl), write_side(&mut writer, rx, ctl));
    let closed = writer.finish(true).await;
    let stopped = match (read, write) {
        (Err(e), _) | (_, Err(e)) => return Err(e),
        (Ok(Read::Stopped(stop)), _) => Some(stop),
        // The reader reached the source's end and the writer took it all, unless a stop came meanwhile.
        (Ok(_), Ok(_)) => ctl.stop(),
    };
    if let Some(stop) = stopped {
        if stop == Stop::Cancel {
            let _ = c.dst.remove_file(&partial).await;
        }
        return Ok(Copied::Stopped(stop));
    }
    closed?;
    let out = finish_partial(c, &src, &partial, offset).await?;
    if matches!(out, Copied::Done { .. }) {
        ctl.confirmed.fetch_add(src.size - offset, Ordering::Relaxed);
    }
    Ok(out)
}

/// Puts a complete partial in place: checks its size and the source again, then the target (absent, or what was
/// approved), renames, and keeps times and mode. Also the end of a direct host-to-host copy.
pub async fn finish_partial(c: &FileCopy<'_>, src: &Meta, partial: &str, offset: u64) -> FsResult<Copied> {
    let src = src.clone();
    let partial = partial.to_string();
    let target = c.dst.join(c.dst_dir, c.dst_name);
    let have = c.dst.lstat(&partial).await?.size;
    let after = c.src.stat(c.src_path).await?;
    if have != src.size || (after.size, after.mtime) != (src.size, src.mtime) {
        // Bytes from two versions of the source: never put in place, never resumed.
        let _ = c.dst.remove_file(&partial).await;
        return Err(FsError::new(ErrKind::Other, "the source changed while it was copied".to_string()));
    }

    // The re-check: replace only what was approved, never what appeared or changed since.
    let replaced = absent(c.dst.lstat(&target).await)?;
    if let Some(now) = &replaced {
        if c.approved != Some((now.size, now.mtime)) || now.kind == Kind::Dir {
            return Ok(Copied::TargetChanged { now: now.clone() });
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
    }
    // The partial was owner-only: the file always gets its final mode. The source's when it is kept, else the
    // replaced file's, else what a new file gets on the target. rwx only: setuid, setgid and sticky bits are never
    // carried over (as OpenSSH's sftp does), nor kept from a replaced file.
    let mode = c.preserve.then_some(src.mode).flatten().or(replaced.and_then(|m| m.mode)).map_or_else(|| c.dst.new_file_mode(), |m| m & 0o777);
    if let Err(e) = c.dst.chmod(&target, mode).await {
        if e.kind != ErrKind::Unsupported {
            warning = Some(format!("the permissions could not be set: {e}"));
        }
    }
    Ok(Copied::Done { bytes: src.size, resumed_at: offset, warning })
}

/// How the reader ended.
#[derive(Debug, PartialEq, Eq)]
enum Read {
    /// The source's end.
    End,
    Stopped(Stop),
    /// The writer is gone (it failed: its error is the one reported).
    WriterGone,
}

/// Awaits `fut`; a wake ends the wait only when it is a stop (pause, cancel, halt), and the same future is awaited
/// again otherwise, so nothing it holds (a chunk on its way) is dropped.
async fn unless_stopped<T>(fut: impl std::future::Future<Output = T>, ctl: &Control) -> Result<T, Stop> {
    tokio::pin!(fut);
    loop {
        if let Some(stop) = ctl.stop() {
            return Err(stop);
        }
        tokio::select! {
            out = &mut fut => return Ok(out),
            _ = ctl.wake.notified() => {}
        }
    }
}

/// Reads the source into the bounded channel until its end, a stop, or an error.
async fn read_side(mut reader: Box<dyn tokio::io::AsyncRead + Send + Unpin>, tx: mpsc::Sender<Vec<u8>>, ctl: &Control) -> FsResult<Read> {
    loop {
        let mut buf = vec![0u8; CHUNK];
        let n = match unless_stopped(tokio::time::timeout(STALL, reader.read(&mut buf)), ctl).await {
            Err(stop) => return Ok(Read::Stopped(stop)),
            Ok(Ok(r)) => r?,
            Ok(Err(_)) => return Err(FsError::new(ErrKind::Connection, "no data for 60 s: the connection seems lost")),
        };
        if n == 0 {
            return Ok(Read::End);
        }
        buf.truncate(n);
        match unless_stopped(tx.send(buf), ctl).await {
            Err(stop) => return Ok(Read::Stopped(stop)),
            Ok(Err(_)) => return Ok(Read::WriterGone),
            Ok(Ok(())) => {}
        }
    }
}

/// Writes what the reader hands over, counting progress on bytes handed to the target. Ends when the reader is done,
/// or at once on a stop (what is written so far stays in the partial).
async fn write_side(writer: &mut Box<dyn super::WriteFile>, mut rx: mpsc::Receiver<Vec<u8>>, ctl: &Control) -> FsResult<u64> {
    let mut written = 0u64;
    loop {
        let buf = match unless_stopped(rx.recv(), ctl).await {
            Err(_) | Ok(None) => return Ok(written),
            Ok(Some(b)) => b,
        };
        match unless_stopped(writer.write_all(&buf), ctl).await {
            Err(_) => return Ok(written),
            Ok(r) => r?,
        }
        written += buf.len() as u64;
        ctl.bytes.fetch_add(buf.len() as u64, Ordering::Relaxed);
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
    // A stale moved-aside file (left by a stop `recover_aside` could not judge) is never removed: another name is used,
    // and the name used is what the journal records, so a repair finds this one.
    let mut old = c.dst.join(c.dst_dir, &aside_name(c.dst_name, c.tag));
    let mut n = 1;
    while absent(c.dst.lstat(&old).await)?.is_some() {
        n += 1;
        old = c.dst.join(c.dst_dir, &format!("{}.{n}", aside_name(c.dst_name, c.tag)));
    }
    (c.record_aside)(Some(old.clone()));
    if let Err(e) = c.dst.rename(target, &old, false).await {
        (c.record_aside)(None);
        return Err(e);
    }
    if let Err(e) = c.dst.rename(partial, target, false).await {
        if c.dst.rename(&old, target, false).await.is_ok() {
            (c.record_aside)(None);
        }
        return Err(e);
    }
    let _ = c.dst.remove_file(&old).await;
    (c.record_aside)(None);
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

    fn no_record(_: Option<String>) {}

    fn job<'a>(src: &'a MemFs, dst: &'a MemFs, approved: Option<(u64, u64)>) -> FileCopy<'a> {
        FileCopy { src, src_path: "/src/big.bin", dst, dst_dir: "/dst", dst_name: "big.bin", tag: "t1", approved, resume: true, source_seen: None, preserve: true, aside: None, record_aside: &no_record }
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
    async fn a_replace_cut_between_its_renames_is_repaired_from_the_recorded_name() {
        let src = MemFs::new();
        let dst = MemFs::new().without_posix_rename();
        src.put("/src/big.bin", &data(1000), 1);
        dst.put("/dst/big.bin", b"old", 5);
        let recorded: parking_lot::Mutex<Vec<Option<String>>> = Default::default();
        let record = |a: Option<String>| recorded.lock().push(a);
        // The second rename (partial -> target) fails: the old file is put back, and nothing stays recorded.
        dst.fail_rename_to(Some("/dst/big.bin"));
        let mut j = job(&src, &dst, Some((3, 5)));
        j.record_aside = &record;
        assert!(copy_file(&j, &Control::default()).await.is_err());
        assert_eq!(dst.read("/dst/big.bin").unwrap(), b"old");
        assert_eq!(*recorded.lock(), [Some("/dst/.big.bin.t1.kelpie-old".to_string()), None], "recorded before the move");
        // As if the daemon stopped between the renames of a replace that found a stale moved-aside file: the person's
        // file is only under the numbered name the journal recorded.
        dst.fail_rename_to(None);
        dst.put("/dst/.big.bin.t1.kelpie-old", b"stale", 1);
        dst.rename("/dst/big.bin", "/dst/.big.bin.t1.kelpie-old.2", false).await.unwrap();
        let mut again = job(&src, &dst, Some((3, 5)));
        again.aside = Some("/dst/.big.bin.t1.kelpie-old.2");
        recover_aside(&again, &src.stat("/src/big.bin").await.unwrap()).await.unwrap();
        assert_eq!(dst.read("/dst/big.bin").unwrap(), b"old", "the newest original is back, not the stale one");
        assert_eq!(dst.read("/dst/.big.bin.t1.kelpie-old").unwrap(), b"stale", "a stale moved-aside file is never removed");
        // Nothing recorded: nothing is looked up or touched.
        recover_aside(&job(&src, &dst, None), &src.stat("/src/big.bin").await.unwrap()).await.unwrap();
        assert!(dst.exists("/dst/.big.bin.t1.kelpie-old"));
        // A later replace is not blocked by the stale name.
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
    async fn the_final_mode_is_the_sources_else_the_replaced_files_else_a_new_files() {
        // This computer on Windows has no modes.
        let src = MemFs::new().without_modes();
        let dst = MemFs::new();
        src.put("/src/big.bin", &data(10), 1);
        dst.mkdir_all("/dst");
        copy_file(&job(&src, &dst, None), &Control::default()).await.unwrap();
        assert_eq!(dst.mode("/dst/big.bin"), Some(0o644), "a new file gets what new files get there, never the partial's 0600");
        dst.chmod("/dst/big.bin", 0o755).await.unwrap();
        let m = dst.lstat("/dst/big.bin").await.unwrap();
        copy_file(&job(&src, &dst, Some((m.size, m.mtime))), &Control::default()).await.unwrap();
        assert_eq!(dst.mode("/dst/big.bin"), Some(0o755), "a replaced file keeps its mode");
        // Without "keep times and permissions" the source's mode is not carried.
        let unix = MemFs::new();
        unix.put("/src/big.bin", &data(10), 1);
        unix.chmod("/src/big.bin", 0o700).await.unwrap();
        dst.remove_file("/dst/big.bin").await.unwrap();
        let mut j = job(&unix, &dst, None);
        j.preserve = false;
        copy_file(&j, &Control::default()).await.unwrap();
        assert_eq!(dst.mode("/dst/big.bin"), Some(0o644));
    }

    #[tokio::test]
    async fn only_bytes_the_target_holds_count_as_a_retrys_progress() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        let all = data(800_000);
        src.put("/src/big.bin", &all, 1);
        dst.mkdir_all("/dst");
        src.fail_reads_after(Some(300_000));
        let ctl = Control::default();
        assert!(copy_file(&job(&src, &dst, None), &ctl).await.is_err());
        assert!(ctl.bytes.load(Ordering::Relaxed) > 0);
        assert_eq!(ctl.confirmed.load(Ordering::Relaxed), 0, "handed to the target is not held by it");
        src.fail_reads_after(None);
        let ctl = Control::default();
        copy_file(&job(&src, &dst, None), &ctl).await.unwrap();
        assert_eq!(ctl.confirmed.load(Ordering::Relaxed), 800_000, "resumed from, then written and closed");
    }

    #[tokio::test]
    async fn room_is_needed_only_for_what_is_left_to_write() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        let all = data(500_000);
        src.put("/src/big.bin", &all, 1);
        dst.put("/dst/.big.bin.t1.kelpie-part", &all[..300_000], 1);
        let ctl = Control::default();
        ctl.free.store(250_000, Ordering::Relaxed);
        assert!(matches!(copy_file(&job(&src, &dst, None), &ctl).await.unwrap(), Copied::Done { resumed_at: 300_000, .. }));
        dst.remove_file("/dst/big.bin").await.unwrap();
        let ctl = Control::default();
        ctl.free.store(250_000, Ordering::Relaxed);
        assert_eq!(copy_file(&job(&src, &dst, None), &ctl).await.unwrap(), Copied::NoRoom { free: 250_000 });
        assert!(!dst.exists("/dst/big.bin"));
        assert!(Control::default().reserve(u64::MAX / 2), "unknown free space never refuses");
    }

    #[tokio::test]
    async fn a_halt_stops_a_file_still_copying_and_keeps_every_byte_of_its_partial() {
        let (src, dst) = (MemFs::new(), MemFs::new());
        src.put("/src/big.bin", &data(800_000), 1);
        dst.mkdir_all("/dst");
        // Its reads stop answering after 300 000 bytes while another file of the job loses the connection.
        src.stall_reads_of("/src/big.bin", Some(300_000));
        let ctl = Control::default();
        let partial = "/dst/.big.bin.t1.kelpie-part";
        let halter = async {
            for _ in 0..400 {
                if dst.read(partial).map_or(0, |b| b.len()) >= 300_000 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            ctl.halted.store(true, Ordering::Relaxed);
            ctl.wake.notify_waiters();
        };
        let fc = job(&src, &dst, None);
        let (out, ()) = tokio::join!(copy_file(&fc, &ctl), halter);
        assert_eq!(out.unwrap(), Copied::Stopped(Stop::Pause), "stopped as on a pause, never \"the source changed\"");
        assert_eq!(dst.read(partial).unwrap(), data(800_000)[..300_000], "kept whole for the resume");
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
