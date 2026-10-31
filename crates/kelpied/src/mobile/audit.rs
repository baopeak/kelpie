//! What phones did and what was decided about them: an append-only, hash-chained log on this computer
//! (`mobile-audit.jsonl`): device id, time, action, and a summary that never holds a secret or the text typed (a length and a
//! hash instead). Each line's hash covers the previous line's, so an edit or a deletion in the middle shows.
//!
//! Lines are written by a thread of their own behind a bounded queue, so a phone's typing never waits for the disk. When the
//! queue is full (the disk is stuck) entries are counted and the count is written as an entry of its own. The thread starts with
//! the first entry. The file is rotated once at 4 MiB (`mobile-audit.1.jsonl`, the older one is dropped); a new file's chain
//! carries on from the old one's last hash.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::OnceLock;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub const FILE: &str = "mobile-audit.jsonl";
const OLD_FILE: &str = "mobile-audit.1.jsonl";
const ROTATE_AT: u64 = 4 * 1024 * 1024;
const QUEUE: usize = 1024;
/// What the first line of a new chain follows.
const GENESIS: &str = "kelpie-audit/1";
/// Most entries one `read` returns.
const READ_MAX: usize = 2000;

enum Job {
    Entry { at: u64, device: String, action: String, summary: String },
    Flush(SyncSender<()>),
}

pub struct AuditLog {
    dir: PathBuf,
    tx: OnceLock<SyncSender<Job>>,
    dropped: AtomicU64,
}

fn line_hash(prev: &str, at: u64, device: &str, action: &str, summary: &str) -> String {
    let mut h = Sha256::new();
    for field in [prev, &at.to_string(), device, action, summary] {
        h.update((field.len() as u32).to_le_bytes());
        h.update(field.as_bytes());
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The hash of a log file's last whole line. A line cut short (the disk filled, the machine went down mid-write) is skipped:
/// the chain carries on from the last line it covers, and nothing it covers is lost.
fn last_hash(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().rev().find_map(|line| serde_json::from_str::<Value>(line).ok()?["hash"].as_str().map(str::to_string))
}

/// Ends a line left cut short, so the next entry starts a line of its own.
fn end_torn_line(path: &Path) {
    let torn = std::fs::read(path).is_ok_and(|bytes| bytes.last().is_some_and(|b| *b != b'\n'));
    if torn {
        if let Err(e) = std::fs::OpenOptions::new().append(true).open(path).and_then(|mut f| f.write_all(b"\n")) {
            tracing::warn!("phone audit log: {e}");
        }
    }
}

fn writer(dir: PathBuf, rx: Receiver<Job>) {
    let path = dir.join(FILE);
    let old = dir.join(OLD_FILE);
    let mut prev = last_hash(&path).or_else(|| last_hash(&old)).unwrap_or_else(|| GENESIS.to_string());
    end_torn_line(&path);
    while let Ok(job) = rx.recv() {
        match job {
            Job::Flush(done) => {
                let _ = done.send(());
            }
            Job::Entry { at, device, action, summary } => {
                if std::fs::metadata(&path).is_ok_and(|m| m.len() >= ROTATE_AT) {
                    let _ = std::fs::rename(&path, &old);
                }
                let hash = line_hash(&prev, at, &device, &action, &summary);
                let line = json!({"at": at, "device": device, "action": action, "summary": summary, "prev": prev, "hash": hash}).to_string();
                let written = std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(format!("{line}\n").as_bytes()));
                match written {
                    Ok(()) => prev = hash,
                    Err(e) => tracing::warn!("phone audit log: {e}"),
                }
            }
        }
    }
}

impl AuditLog {
    pub fn new(dir: &Path) -> AuditLog {
        AuditLog { dir: dir.to_path_buf(), tx: OnceLock::new(), dropped: AtomicU64::new(0) }
    }

    fn sender(&self) -> &SyncSender<Job> {
        self.tx.get_or_init(|| {
            let (tx, rx) = sync_channel(QUEUE);
            let dir = self.dir.clone();
            if let Err(e) = std::thread::Builder::new().name("kelpie-phone-audit".into()).spawn(move || writer(dir, rx)) {
                tracing::warn!("phone audit log: {e}");
            }
            tx
        })
    }

    /// Adds an entry. Never waits: a full queue counts it as dropped, and the next entry that fits says how many were.
    pub fn add(&self, device: &str, action: &str, summary: &str) {
        let tx = self.sender();
        let at = crate::session::now_millis();
        let lost = self.dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            let note = Job::Entry { at, device: "-".into(), action: "audit_dropped".into(), summary: format!("{lost} entries were not written: the disk was too slow") };
            if tx.try_send(note).is_err() {
                self.dropped.fetch_add(lost, Ordering::Relaxed);
            }
        }
        match tx.try_send(Job::Entry { at, device: device.into(), action: action.into(), summary: summary.into() }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Waits until what was added so far is in the file.
    fn flush(&self) {
        let Some(tx) = self.tx.get() else { return };
        let (done, wait) = sync_channel(1);
        if tx.send(Job::Flush(done)).is_ok() {
            let _ = wait.recv_timeout(std::time::Duration::from_secs(5));
        }
    }

    /// The newest `limit` entries, newest first, and whether the current file's chain holds together. Reads the file: call it
    /// off the event loop.
    pub fn read(&self, limit: usize) -> Value {
        self.flush();
        let text = std::fs::read_to_string(self.dir.join(FILE)).unwrap_or_default();
        let mut entries: Vec<Value> = Vec::new();
        let mut intact = true;
        let mut prev: Option<String> = None;
        for line in text.lines().filter(|l| !l.trim().is_empty()) {
            // A line that does not parse holds no entry. If it was one, the next line's `prev` no longer matches and that shows;
            // a line cut short while being written was never part of the chain.
            let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
            let text = |k: &str| v[k].as_str().unwrap_or_default().to_string();
            let hash = line_hash(&text("prev"), v["at"].as_u64().unwrap_or(0), &text("device"), &text("action"), &text("summary"));
            if hash != text("hash") || prev.as_ref().is_some_and(|p| *p != text("prev")) {
                intact = false;
            }
            prev = Some(text("hash"));
            entries.push(json!({"at": v["at"], "device": v["device"], "action": v["action"], "summary": v["summary"]}));
        }
        let limit = limit.clamp(1, READ_MAX);
        let newest: Vec<Value> = entries.into_iter().rev().take(limit).collect();
        json!({"entries": newest, "intact": intact})
    }
}

/// A summary of text a phone sent: its length and the start of its hash, never the text.
pub fn text_summary(text: &str) -> String {
    let digest: String = Sha256::digest(text.as_bytes()).iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{} bytes, sha256 {digest}", text.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kelpie-audit-{tag}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn entries_chain_and_an_edit_shows() {
        let d = dir("chain");
        let log = AuditLog::new(&d);
        log.add("d1", "pair", "Pixel, read");
        log.add("d1", "input", &text_summary("secret text"));
        log.add("-", "revoke_all", "2 devices");
        let read = log.read(10);
        assert_eq!(read["intact"], true);
        let entries = read["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["action"], "revoke_all", "newest first");
        let file = std::fs::read_to_string(d.join(FILE)).unwrap();
        assert!(!file.contains("secret text"), "only a hash of what was typed");
        // A new log on the same file carries the chain on.
        let again = AuditLog::new(&d);
        again.add("d2", "revoke", "Tablet");
        assert_eq!(again.read(10)["intact"], true);
        assert_eq!(again.read(1)["entries"].as_array().unwrap().len(), 1);
        // An edited line and a removed line are noticed.
        let text = std::fs::read_to_string(d.join(FILE)).unwrap();
        std::fs::write(d.join(FILE), text.replace("Pixel, read", "Pixel, write")).unwrap();
        assert_eq!(AuditLog::new(&d).read(10)["intact"], false);
        let without: Vec<&str> = text.lines().enumerate().filter(|(i, _)| *i != 1).map(|(_, l)| l).collect();
        std::fs::write(d.join(FILE), without.join("\n") + "\n").unwrap();
        assert_eq!(AuditLog::new(&d).read(10)["intact"], false);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_line_cut_short_while_writing_is_no_false_alarm_and_the_chain_carries_on() {
        let d = dir("torn");
        let log = AuditLog::new(&d);
        log.add("d1", "pair", "Pixel, read");
        log.add("d1", "input", "1 bytes");
        assert_eq!(log.read(10)["intact"], true);
        drop(log);
        // The last write stopped halfway.
        let mut file = std::fs::OpenOptions::new().append(true).open(d.join(FILE)).unwrap();
        file.write_all(br#"{"at":17,"device":"d1","action":"inp"#).unwrap();
        drop(file);
        assert_eq!(AuditLog::new(&d).read(10)["intact"], true, "nothing the chain covers is lost");
        let again = AuditLog::new(&d);
        again.add("d2", "revoke", "Tablet");
        let read = again.read(10);
        assert_eq!(read["intact"], true, "{read}");
        assert_eq!(read["entries"].as_array().unwrap().len(), 3);
        assert_eq!(read["entries"][0]["action"], "revoke", "a line of its own");
        // A whole line damaged in the middle still shows: what followed it no longer chains.
        let text = std::fs::read_to_string(d.join(FILE)).unwrap();
        let second = text.lines().nth(1).unwrap();
        std::fs::write(d.join(FILE), text.replacen(second, &second[..second.len() / 2], 1)).unwrap();
        let later = AuditLog::new(&d).read(10);
        assert_eq!(later["intact"], false, "{later}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_is_started_or_written_before_the_first_entry() {
        let d = dir("idle");
        let log = AuditLog::new(&d);
        assert_eq!(log.read(5)["entries"].as_array().unwrap().len(), 0);
        assert_eq!(log.read(5)["intact"], true);
        assert!(log.tx.get().is_none());
        assert!(!d.join(FILE).exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
