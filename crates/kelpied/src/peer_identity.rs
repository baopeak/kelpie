//! Is the program at the other end of a connection the Kelpie app?
//!
//! The user token proves only "a program of this OS user": any of them can read it. Approving a phone, raising what a phone
//! may do and resetting phone access may be asked for only over a connection whose peer process runs the Kelpie app's
//! executable (`Role::App`). The peer is found from the connection itself (Windows `GetNamedPipeClientProcessId`, macOS
//! `LOCAL_PEERPID`, Linux `SO_PEERCRED`) and its executable from the process (`QueryFullProcessImageNameW`, `proc_pidpath`,
//! `/proc/<pid>/exe`), so a program cannot just claim to be the app.
//!
//! This is a speed bump, not the gate: a program of this user can write a file where the paths below look and run it. All that
//! buys it is the right to raise a prompt whose text the daemon wrote; the gate is the OS verification (`user_presence`).
//!
//! The app's executable is `kelpie` next to this daemon (macOS app bundle, development builds), or next to the daemon the
//! app copied into a runtime slot (`<data>/daemon/a|b`, Windows): the app names its own folder in the slot's `source.txt`.
//! A debug build of the daemon also trusts the paths in `KELPIE_TRUSTED_APP` (test scripts); a release build never reads it.

use std::path::{Path, PathBuf};

/// The executable's file name.
const APP_NAME: &str = if cfg!(windows) { "kelpie.exe" } else { "kelpie" };
/// Written by the app into a runtime slot next to the daemon copy: the folder the app runs from.
pub const SLOT_SOURCE: &str = "source.txt";

/// Where the daemon's own executable is.
fn own_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// Executables trusted as the app, as written (canonicalized when compared).
pub fn trusted_apps() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(dir) = own_dir() {
        out.push(dir.join(APP_NAME));
        // A runtime slot: the app said where it is installed.
        if let Ok(source) = std::fs::read_to_string(dir.join(SLOT_SOURCE)) {
            let source = source.trim();
            if !source.is_empty() {
                out.push(Path::new(source).join(APP_NAME));
            }
        }
    }
    #[cfg(debug_assertions)]
    if let Some(list) = std::env::var_os("KELPIE_TRUSTED_APP") {
        out.extend(std::env::split_paths(&list).filter(|p| !p.as_os_str().is_empty()));
    }
    out
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => {
            if cfg!(windows) {
                a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
            } else {
                a == b
            }
        }
        _ => false,
    }
}

/// Whether process `pid` runs one of the trusted executables.
pub fn is_trusted_app(pid: u32) -> bool {
    let Some(image) = image_of(pid) else { return false };
    trusted_apps().iter().any(|t| same_file(&image, t))
}

/// The executable process `pid` runs.
#[cfg(windows)]
pub fn image_of(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: plain Win32 calls with a buffer we own; the handle is closed on every path.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = vec![0u16; 32_768];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| PathBuf::from(std::ffi::OsString::from_wide(&buf[..len as usize])))
    }
}

#[cfg(target_os = "macos")]
pub fn image_of(pid: u32) -> Option<PathBuf> {
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is as large as the call may write.
    let n = unsafe { libc::proc_pidpath(pid as libc::c_int, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn image_of(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// The process at the other end of a daemon connection (before it is split).
#[cfg(not(target_os = "macos"))]
#[allow(clippy::useless_conversion)] // a process id's type differs per OS
pub fn peer_pid(stream: &interprocess::local_socket::tokio::Stream) -> Option<u32> {
    use interprocess::local_socket::traits::StreamCommon as _;
    stream.peer_creds().ok()?.pid().and_then(|p| u32::try_from(p).ok())
}

/// The process at the other end of a daemon connection, from its receiving half (`LOCAL_PEERPID`: Darwin's peer credentials
/// carry no process id).
#[cfg(target_os = "macos")]
pub fn peer_pid_of(rx: &interprocess::local_socket::tokio::RecvHalf) -> Option<u32> {
    use std::os::fd::{AsFd as _, AsRawFd as _};
    let interprocess::local_socket::tokio::RecvHalf::UdSocket(half) = rx;
    let fd = half.as_fd().as_raw_fd();
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: getsockopt writes at most `len` bytes into `pid`.
    let r = unsafe { libc::getsockopt(fd, libc::SOL_LOCAL, libc::LOCAL_PEERPID, (&mut pid as *mut libc::pid_t).cast(), &mut len) };
    (r == 0 && pid > 0).then_some(pid as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_program_is_not_the_app_unless_named_and_names_are_compared_as_files() {
        let me = std::process::id();
        let image = image_of(me).expect("own image");
        assert!(same_file(&image, &std::env::current_exe().unwrap()));
        // The test binary is not called kelpie: not trusted (KELPIE_TRUSTED_APP is not set by cargo test).
        if std::env::var_os("KELPIE_TRUSTED_APP").is_none() {
            assert!(!is_trusted_app(me));
        }
        assert!(image_of(u32::MAX - 7).is_none(), "no such process");
        assert!(!same_file(Path::new("does-not-exist-a"), Path::new("does-not-exist-a")), "missing files never match");
    }
}
