//! Keep the computer awake (no idle sleep) always or while an agent is working, so a long
//! agent run is not cut off by the power plan. The display may still turn off.
//!
//! Windows: `SetThreadExecutionState` from one dedicated thread (the request belongs to the
//! calling thread). macOS: a `caffeinate -i -w <daemon pid>` child. Linux: a `systemd-inhibit`
//! child. Elsewhere: no-op.

use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Off,
    Agents,
    Always,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "always" => Mode::Always,
            "agents" => Mode::Agents,
            _ => Mode::Off,
        }
    }
}

pub struct Awake {
    tx: mpsc::Sender<bool>,
    held: parking_lot::Mutex<bool>,
}

impl Awake {
    pub fn new() -> Awake {
        let (tx, rx) = mpsc::channel::<bool>();
        let _ = std::thread::Builder::new().name("kelpie-awake".into()).spawn(move || run(rx));
        Awake { tx, held: parking_lot::Mutex::new(false) }
    }

    /// Holds or releases the wake request; cheap when nothing changes.
    pub fn set(&self, hold: bool) {
        let mut held = self.held.lock();
        if *held != hold {
            *held = hold;
            let _ = self.tx.send(hold);
            tracing::info!(hold, "keep awake");
        }
    }

    pub fn held(&self) -> bool {
        *self.held.lock()
    }
}

#[cfg(windows)]
fn run(rx: mpsc::Receiver<bool>) {
    use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED};
    while let Ok(hold) = rx.recv() {
        let flags = if hold { ES_CONTINUOUS | ES_SYSTEM_REQUIRED } else { ES_CONTINUOUS };
        unsafe { SetThreadExecutionState(flags) };
    }
}

#[cfg(target_os = "macos")]
fn run(rx: mpsc::Receiver<bool>) {
    let mut child: Option<std::process::Child> = None;
    while let Ok(hold) = rx.recv() {
        if let Some(mut c) = child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if hold {
            // `-w`: caffeinate exits with the daemon even if we never get to kill it.
            child = std::process::Command::new("/usr/bin/caffeinate")
                .args(["-i", "-w", &std::process::id().to_string()])
                .spawn()
                .map_err(|e| tracing::warn!("caffeinate: {e}"))
                .ok();
        }
    }
}

/// Linux: a `systemd-inhibit … sleep infinity` child holds an idle + sleep inhibitor lock from
/// logind for as long as it runs. It gets SIGTERM when this thread ends (`PR_SET_PDEATHSIG`), so a
/// daemon that dies never leaves the computer awake; `systemd-inhibit` in turn takes its `sleep`
/// down with it. Without systemd (or logind) the request is logged once and otherwise ignored.
#[cfg(target_os = "linux")]
fn run(rx: mpsc::Receiver<bool>) {
    let mut child: Option<std::process::Child> = None;
    let mut warned = false;
    while let Ok(hold) = rx.recv() {
        if let Some(mut c) = child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if hold {
            match inhibit() {
                Ok(c) => child = Some(c),
                Err(e) if !warned => {
                    warned = true;
                    tracing::warn!("keep awake is unavailable: {e}");
                }
                Err(e) => tracing::debug!("keep awake: {e}"),
            }
        }
    }
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
}

#[cfg(any(target_os = "linux", test))]
const INHIBIT_ARGS: &[&str] = &["--what=idle:sleep", "--who=Kelpie", "--why=An agent is working", "--mode=block", "sleep", "infinity"];

#[cfg(target_os = "linux")]
fn inhibit() -> Result<std::process::Child, String> {
    use std::io::Read as _;
    use std::os::unix::process::CommandExt as _;
    let parent = std::process::id() as libc::pid_t;
    let mut cmd = std::process::Command::new("systemd-inhibit");
    cmd.args(INHIBIT_ARGS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // SAFETY: only async-signal-safe calls between fork and exec, and no allocation.
    unsafe {
        cmd.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            // The daemon died before the line above took effect: nobody would ever stop us.
            if libc::getppid() != parent {
                return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "systemd-inhibit was not found (a system without systemd); the computer may sleep while agents work".to_string(),
        _ => format!("systemd-inhibit: {e}"),
    })?;
    // It exits at once when logind refuses the lock (no session, no permission).
    std::thread::sleep(std::time::Duration::from_millis(300));
    match child.try_wait() {
        Ok(None) => Ok(child),
        Ok(Some(status)) => {
            let mut why = String::new();
            if let Some(mut err) = child.stderr.take() {
                let _ = err.read_to_string(&mut why);
            }
            Err(format!("systemd-inhibit exited ({status}): {}", why.trim()))
        }
        Err(e) => Err(format!("systemd-inhibit: {e}")),
    }
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn run(rx: mpsc::Receiver<bool>) {
    while rx.recv().is_ok() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn linux_inhibitor_blocks_idle_and_sleep_until_it_is_stopped() {
        let args = super::INHIBIT_ARGS;
        assert_eq!(&args[..4], ["--what=idle:sleep", "--who=Kelpie", "--why=An agent is working", "--mode=block"]);
        assert_eq!(&args[4..], ["sleep", "infinity"], "runs until killed");
    }

    /// The real thing on a Linux machine: holding and releasing never panics or blocks, whether or
    /// not systemd-inhibit and logind are there (CI runners have no user session).
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_hold_and_release_survive_a_missing_or_refusing_logind() {
        let awake = super::Awake::new();
        awake.set(true);
        assert!(awake.held());
        std::thread::sleep(std::time::Duration::from_millis(500));
        awake.set(false);
        assert!(!awake.held());
    }
}
