//! Durable session metadata (`sessions.json`) and last-screen snapshots (`snapshots/<id>.ansi`).
//! All writes are atomic (temp file + rename) so a crash never leaves a torn file.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::session::SessionSpec;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedSession {
    pub spec: SessionSpec,
    #[serde(default)]
    pub title: Option<String>,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub chat: bool,
    #[serde(default)]
    pub last_activity: u64,
    /// Running (or interrupted and not yet resumed) when saved. Files from before this field
    /// count every session as running.
    #[serde(default = "yes")]
    pub alive: bool,
}

fn yes() -> bool {
    true
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: &Path) -> Store {
        let _ = std::fs::create_dir_all(dir.join("snapshots"));
        Store { dir: dir.to_path_buf() }
    }

    pub fn load_sessions(&self) -> Vec<SavedSession> {
        std::fs::read(self.dir.join("sessions.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save_sessions(&self, sessions: &[SavedSession]) -> Result<()> {
        write_atomic(&self.dir.join("sessions.json"), &serde_json::to_vec_pretty(sessions)?)
    }

    fn snapshot_path(&self, id: &str) -> PathBuf {
        let safe: String = id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        self.dir.join("snapshots").join(format!("{safe}.ansi"))
    }

    pub fn save_snapshot(&self, id: &str, data: &[u8]) -> Result<()> {
        write_atomic(&self.snapshot_path(id), data)
    }

    pub fn load_snapshot(&self, id: &str) -> Option<Vec<u8>> {
        std::fs::read(self.snapshot_path(id)).ok()
    }

    pub fn remove_snapshot(&self, id: &str) {
        let _ = std::fs::remove_file(self.snapshot_path(id));
    }
}

pub fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    // Unique temp name: concurrent writers (ticker vs shutdown) never share a temp file.
    let tmp = path.with_extension(format!("{}.{n}.tmp", std::process::id()));
    let result = std::fs::write(&tmp, data).and_then(|()| {
        #[cfg(windows)]
        {
            replace_open_file(&tmp, path)
        }
        #[cfg(not(windows))]
        {
            std::fs::rename(&tmp, path)
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(Into::into)
}

#[cfg(windows)]
fn replace_open_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::mem::{align_of, offset_of, size_of};
    use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{SetFileInformationByHandle, FileRenameInfoEx, DELETE, FILE_RENAME_INFO};

    // Unlike MoveFileEx, POSIX replacement keeps old readers usable and lets new
    // readers open the new file instead of encountering a delete-pending name.
    // https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information
    const REPLACE_IF_EXISTS: u32 = 1;
    const POSIX_SEMANTICS: u32 = 2;
    const { assert!(align_of::<usize>() >= align_of::<FILE_RENAME_INFO>()) };
    let destination = std::path::absolute(destination)?;
    let name_units = destination.as_os_str().encode_wide().count();
    let name_bytes = u32::try_from(name_units.checked_mul(2).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination path is too long"))?)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination path is too long"))?;
    let offset = offset_of!(FILE_RENAME_INFO, FileName);
    let size = size_of::<FILE_RENAME_INFO>().max(offset + name_bytes as usize + 2);
    let size_u32 = u32::try_from(size).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "destination path is too long"))?;
    let mut storage = vec![0usize; size.div_ceil(size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: usize storage supplies native alignment, all bytes are initialized,
    // and the allocation includes the complete variable-length UTF-16 filename.
    unsafe {
        (*info).Anonymous.Flags = REPLACE_IF_EXISTS | POSIX_SEMANTICS;
        (*info).FileNameLength = name_bytes;
        let name = storage.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>();
        for (i, unit) in destination.as_os_str().encode_wide().chain([0]).enumerate() {
            name.add(i).write(unit);
        }
    }
    let file = std::fs::OpenOptions::new().access_mode(DELETE).open(source)?;
    // SAFETY: the file owns a live handle with DELETE access; info points to the
    // initialized, aligned buffer above and stays alive until this synchronous call ends.
    let renamed = unsafe { SetFileInformationByHandle(file.as_raw_handle(), FileRenameInfoEx, info.cast(), size_u32) };
    if renamed == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn replacing_a_published_file_preserves_old_and_new_readers() {
        use std::io::Read;

        let dir = std::env::temp_dir().join(format!("kelpie-publish-reader-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state-雪.json");
        write_atomic(&path, br#"{"revision":1}"#).unwrap();
        let mut old_reader = std::fs::File::open(&path).unwrap();
        write_atomic(&path, br#"{"revision":2}"#).unwrap();
        let new_bytes = std::fs::read(&path).unwrap();
        let mut old_bytes = Vec::new();
        old_reader.read_to_end(&mut old_bytes).unwrap();
        drop(old_reader);
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(old_bytes, br#"{"revision":1}"#);
        assert_eq!(new_bytes, br#"{"revision":2}"#);
    }

    #[test]
    fn sessions_saved_before_alive_existed_count_as_running() {
        let old = r#"[{"spec":{"id":"a","kind":"shell","cwd":"."},"cols":80,"rows":24}]"#;
        let saved: Vec<SavedSession> = serde_json::from_str(old).unwrap();
        assert!(saved[0].alive);
        let stopped = r#"[{"spec":{"id":"a","kind":"shell","cwd":"."},"cols":80,"rows":24,"alive":false}]"#;
        let saved: Vec<SavedSession> = serde_json::from_str(stopped).unwrap();
        assert!(!saved[0].alive);
    }
}
