//! Host ↔ host copies: **direct** (host A pushes to B with A's own login to B) when that works without lending A the
//! person's keys and A provably reaches the same B, else **through this computer** (two SFTP sessions, nothing
//! written to this computer's disk).
//!
//! - The ssh to A always has `-T` and no forwarding, tunnel, remote or local command, whatever the hosts' config
//!   says: Kelpie never lends A the agent, and a terminal never holds the batch open.
//! - Direct is never tried when this computer reaches B through a jump host, a proxy command or a local address
//!   (`ssh -G`): A could reach another machine by that address.
//! - The probe runs a fixed one-line script on A with quoted positional values: `sftp`, `ssh` and `ssh-keygen` must
//!   exist there, and `ssh -o BatchMode=yes -o StrictHostKeyChecking=yes -p <port> -l <user> <hostname> true` must
//!   work. `BatchMode` means A's own known hosts must already trust B; nothing is ever accepted on A. The script then
//!   lists the keys A's known hosts hold for B: every one must be a key this computer trusts for B, so A can only
//!   have logged in to the same machine, before any data leaves.
//! - Files go through one `sftp -b -` on A per job (restarted every 50 files): `put` for a fresh partial, `reput` to
//!   continue one, then a marker, so each file's end is seen. It reports its process id first, so a cancel stops it
//!   there. Names that would need more than plain double quotes in a batch line go through this computer instead.
//! - Probe results are cached per (A, B) for 10 minutes.

use std::collections::{BTreeSet, HashMap};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use super::{ErrKind, FsError, FsResult};

/// How long a probe result holds.
pub const CACHE_FOR: Duration = Duration::from_secs(10 * 60);
/// Files one `sftp` on A copies before it is started again.
pub const FILES_PER_LOGIN: u32 = 50;

/// B as A has to reach it: what this computer's ssh would use for B.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub hostname: String,
    pub port: u16,
    pub user: String,
}

impl Target {
    /// `user@host` for sftp (an IPv6 address in brackets, or sftp reads its colons as a path).
    pub fn host_for_sftp(&self) -> String {
        if self.hostname.contains(':') && !self.hostname.starts_with('[') {
            format!("[{}]", self.hostname)
        } else {
            self.hostname.clone()
        }
    }

    /// The name known_hosts files hold B under.
    pub fn known_hosts_name(&self) -> String {
        if self.port == 22 {
            self.hostname.clone()
        } else {
            format!("[{}]:{}", self.hostname.trim_start_matches('[').trim_end_matches(']'), self.port)
        }
    }
}

/// What `ssh -G` says about B here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub target: Target,
    /// `proxyjump` / `proxycommand` other than `none`.
    pub proxied: bool,
    /// `hostkeyalias` when set: the name this computer's known hosts use for B.
    pub host_key_alias: Option<String>,
    /// This computer's known-hosts files for B.
    pub known_hosts: Vec<String>,
}

/// POSIX shell single quoting.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The ssh options for the connection to A (before the destination): Kelpie's terminal options, no terminal, and
/// never any forwarding, tunnel or extra command, whatever the host's config says.
pub fn outer_args(config: &std::path::Path, a: &str) -> Vec<String> {
    let mut args = vec!["-F".to_string(), config.to_string_lossy().into_owned(), "-T".to_string()];
    for o in [
        "ForwardAgent=no", "ForwardX11=no", "ClearAllForwardings=yes", "Tunnel=no", "RequestTTY=no", "RemoteCommand=none", "PermitLocalCommand=no", "NumberOfPasswordPrompts=1",
        "ConnectTimeout=15", "ServerAliveInterval=15", "ServerAliveCountMax=3", "LogLevel=ERROR",
    ] {
        args.push("-o".into());
        args.push(o.into());
    }
    args.push(a.to_string());
    args
}

/// Exit code of the probe script when A has no `sftp`, `ssh` or `ssh-keygen` of its own.
const NO_CLIENT: i32 = 41;
/// The probe's ssh to B failed (its own 255, renumbered so it never looks like the ssh to A failing).
const NO_LOGIN: i32 = 42;
const KEYS_MARK: &str = "kelpie-keys";

/// The probe, run on A as `sh -c <script> kelpie <hostname> <port> <user> <known-hosts name>`: values travel as
/// positional parameters, never inside the script. One line: csh and tcsh login shells refuse newlines in quotes.
pub const PROBE: &str = r#"command -v sftp >/dev/null 2>&1 && command -v ssh >/dev/null 2>&1 && command -v ssh-keygen >/dev/null 2>&1 || exit 41; err=$(ssh -T -o BatchMode=yes -o ConnectTimeout=5 -o StrictHostKeyChecking=yes -o ForwardAgent=no -o ForwardX11=no -o ClearAllForwardings=yes -p "$2" -l "$3" "$1" true 2>&1 >/dev/null); code=$?; [ "$code" -eq 255 ] && code=42; printf '%s\n' "$err"; echo kelpie-keys; ssh-keygen -F "$4" 2>/dev/null; exit $code"#;

pub fn probe_command(t: &Target) -> String {
    format!("sh -c {} kelpie {} {} {} {}", q(PROBE), q(&t.hostname), q(&t.port.to_string()), q(&t.user), q(&t.known_hosts_name()))
}

/// The probe's result: `Ok` when A logs in to B by itself, else the one-line reason the copy goes through this computer.
pub fn parse_probe(code: Option<i32>, out: &str) -> Result<(), String> {
    let lower = out.to_lowercase();
    match code {
        Some(0) => Ok(()),
        Some(NO_CLIENT) => Err("the first host has no SSH client".into()),
        _ if lower.contains("host key verification failed") || lower.contains("no matching host key") || lower.contains("host key for") => Err("the first host does not know the second host's key".into()),
        _ if lower.contains("permission denied") || lower.contains("too many authentication failures") => Err("the first host has no login of its own on the second host".into()),
        _ if lower.contains("timed out") || lower.contains("could not resolve") || lower.contains("refused") || lower.contains("no route") || lower.contains("unreachable") || lower.contains("name or service not known") => {
            Err("the first host cannot reach the second host".into())
        }
        Some(127) => Err("the first host has no shell Kelpie can use".into()),
        _ => Err("the first host could not log in to the second host".into()),
    }
}

/// `(type, key)` pairs from known_hosts lines (as `ssh-keygen -F` prints them; comments and markers skipped).
pub fn host_keys(text: &str) -> BTreeSet<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut words: Vec<&str> = l.split_whitespace().collect();
            if words.first().is_some_and(|w| w.starts_with('@')) {
                if words[0] == "@revoked" {
                    return None;
                }
                words.remove(0);
            }
            (words.len() >= 3).then(|| (words[1].to_string(), words[2].to_string()))
        })
        .collect()
}

/// A provably reaches this computer's B: it holds keys for B, and every one is a key this computer trusts for B.
pub fn same_machine(a_keys: &BTreeSet<(String, String)>, ours: &BTreeSet<(String, String)>) -> Result<(), String> {
    if a_keys.is_empty() || ours.is_empty() {
        return Err("the first host's key for the second host could not be compared with this computer's".into());
    }
    if a_keys.is_subset(ours) {
        Ok(())
    } else {
        Err("the first host knows the second host by another key, so it may not reach the same machine".into())
    }
}

/// Why B must not be reached directly from A, judged from this computer's own view of B.
pub fn direct_refused(r: &Resolved, allow_loopback: bool) -> Option<String> {
    if r.proxied {
        return Some("this computer reaches the second host through a jump host or proxy, so the first host may not reach the same machine".into());
    }
    let h = r.target.hostname.trim_start_matches('[').trim_end_matches(']').to_lowercase();
    let local = h == "localhost" || h.starts_with("127.") || h == "::1" || h.starts_with("169.254.") || h.starts_with("fe80:") || h == "0.0.0.0" || h == "::";
    (local && !allow_loopback).then(|| "the second host's address is local to this computer (a tunnel or a forward), so the first host would reach another machine".into())
}

/// A path as one argument of an sftp batch line, or `None` when plain double quotes cannot carry it safely.
pub fn batch_quote(path: &str) -> Option<String> {
    let unsafe_char = |c: char| c.is_control() || matches!(c, '"' | '\\' | '*' | '?' | '[' | ']' | '{' | '}' | '~');
    (!path.is_empty() && !path.chars().any(unsafe_char)).then(|| format!("\"{path}\""))
}

/// The direct copier, run on A as `sh -c <script> kelpie <port> <user> <host>`: says its process id, then becomes
/// `sftp` in batch mode reading commands from stdin. One line (see `PROBE`).
pub const DIRECT: &str = r#"echo "kelpie-pid $$"; exec sftp -q -b - -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes -o ForwardAgent=no -o ForwardX11=no -o ClearAllForwardings=yes -P "$1" "$2@$3""#;

pub fn direct_command(t: &Target) -> String {
    format!("sh -c {} kelpie {} {} {}", q(DIRECT), q(&t.port.to_string()), q(&t.user), q(&t.host_for_sftp()))
}

/// How one file goes direct, from what the partial already holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirectPlan {
    /// The partial already holds the whole file (an empty file included): nothing to send.
    Complete,
    /// A fresh partial: `put` (`reput` refuses a destination that is not smaller than the source).
    Put,
    /// Continue the partial from its size.
    Reput,
}

pub fn direct_plan(offset: u64, size: u64) -> DirectPlan {
    if offset >= size {
        DirectPlan::Complete
    } else if offset == 0 {
        DirectPlan::Put
    } else {
        DirectPlan::Reput
    }
}

/// The batch lines for one file: the copy (its errors do not stop the batch: `-`), then a marker the session waits for.
pub fn batch_lines(plan: DirectPlan, src: &str, partial: &str, seq: u64) -> Option<String> {
    let verb = match plan {
        DirectPlan::Complete => return Some(String::new()),
        DirectPlan::Put => "put",
        DirectPlan::Reput => "reput",
    };
    Some(format!("-{verb} {} {}\n!echo kelpie-done-{seq}\n", batch_quote(src)?, batch_quote(partial)?))
}

/// The process id in the first lines of the copier's output (a login banner may come first).
pub fn find_pid(line: &str) -> Option<u32> {
    line.trim().strip_prefix("kelpie-pid ").and_then(|p| p.trim().parse().ok())
}

/// What `ssh -G` says this computer's ssh would use for `dest`.
pub fn parse_ssh_g(out: &str) -> Option<Resolved> {
    let (mut hostname, mut port, mut user, mut proxied, mut alias, mut known) = (None, None, None, false, None, Vec::new());
    for line in out.lines() {
        let mut it = line.splitn(2, ' ');
        let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
        let v = v.trim();
        match k {
            "hostname" => hostname = Some(v.to_string()),
            "port" => port = v.parse().ok(),
            "user" => user = Some(v.to_string()),
            "proxyjump" | "proxycommand" => proxied |= !v.eq_ignore_ascii_case("none"),
            "hostkeyalias" if !v.eq_ignore_ascii_case("none") => alias = Some(v.to_string()),
            "userknownhostsfile" | "globalknownhostsfile" => known.extend(v.split_whitespace().map(str::to_string)),
            _ => {}
        }
    }
    let target = Target { hostname: hostname?, port: port.unwrap_or(22), user: user? };
    // Values that a remote shell or ssh could read as something else are never passed on.
    let plain = |s: &str| !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-:%[]".contains(c));
    (plain(&target.hostname) && plain(&target.user)).then_some(Resolved { target, proxied, host_key_alias: alias, known_hosts: known })
}

fn ssh_tool(exe: &std::path::Path, dest: &str, tool: &str) -> Command {
    let mut cmd = Command::new(kelpie_proto::ssh::program(tool));
    cmd.envs(kelpie_proto::ssh::askpass_env(exe, &kelpie_proto::ssh::host_id_for_alias(dest).unwrap_or_default()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

/// B as this computer's ssh resolves it.
pub async fn resolve(exe: &std::path::Path, dest: &str) -> FsResult<Resolved> {
    let mut cmd = ssh_tool(exe, dest, "ssh");
    cmd.arg("-F").arg(kelpie_proto::ssh::config_path()).arg("-G").arg(dest);
    let out = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await.map_err(|_| FsError::new(ErrKind::Other, "ssh -G did not answer"))?.map_err(FsError::from)?;
    parse_ssh_g(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| FsError::new(ErrKind::Other, format!("the address of {dest} could not be read from the ssh config")))
}

/// The keys this computer trusts for B (its known-hosts files, under B's host-key alias when it has one).
pub async fn our_keys(exe: &std::path::Path, dest: &str, r: &Resolved) -> BTreeSet<(String, String)> {
    let name = r.host_key_alias.clone().unwrap_or_else(|| r.target.known_hosts_name());
    let mut keys = BTreeSet::new();
    for file in &r.known_hosts {
        let path = if let Some(rest) = file.strip_prefix("~/") { dirs::home_dir().map(|h| h.join(rest)).unwrap_or_default() } else { std::path::PathBuf::from(file) };
        if !path.is_file() {
            continue;
        }
        let mut cmd = ssh_tool(exe, dest, "ssh-keygen");
        cmd.arg("-F").arg(&name).arg("-f").arg(&path);
        if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
            keys.extend(host_keys(&String::from_utf8_lossy(&out.stdout)));
        }
    }
    keys
}

/// Runs `command` on A over ssh (no terminal, no forwarding); exit code and stdout + stderr.
pub async fn run_on(exe: &std::path::Path, a: &str, command: &str, timeout: Duration) -> FsResult<(Option<i32>, String)> {
    let mut cmd = ssh_tool(exe, a, "ssh");
    cmd.args(outer_args(&kelpie_proto::ssh::config_path(), a)).arg(command);
    let out = tokio::time::timeout(timeout, cmd.output()).await.map_err(|_| FsError::new(ErrKind::Connection, format!("{a} did not answer in time")))?.map_err(FsError::from)?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
}

/// Stops a copier on A, only while that process id is still an `sftp` (a reused id is left alone).
pub fn kill_command(pid: u32) -> String {
    format!("case \"$(ps -p {pid} -o comm= 2>/dev/null)\" in *sftp*) kill {pid}; sleep 1; case \"$(ps -p {pid} -o comm= 2>/dev/null)\" in *sftp*) kill -9 {pid};; esac;; esac; true")
}

pub async fn kill_on(exe: &std::path::Path, a: &str, pid: u32) -> bool {
    matches!(run_on(exe, a, &kill_command(pid), Duration::from_secs(20)).await, Ok((Some(0), _)))
}

/// Probe results per (A, B).
#[derive(Default)]
pub struct Probes(Mutex<HashMap<(String, String), (Instant, Result<Target, String>)>>);

impl Probes {
    /// B's target when A can copy to it directly, else the reason it cannot; cached for 10 minutes.
    pub async fn direct(&self, exe: &std::path::Path, a: &str, b: &str) -> Result<Target, String> {
        let key = (a.to_string(), b.to_string());
        if let Some((at, r)) = self.0.lock().get(&key).cloned() {
            if at.elapsed() < CACHE_FOR {
                return r;
            }
        }
        let resolved = resolve(exe, b).await.map_err(|e| e.message)?;
        // The e2e's hosts all live on this computer: there a local address is the same machine.
        let allow_loopback = std::env::var_os("KELPIE_E2E").is_some();
        let result = if let Some(reason) = direct_refused(&resolved, allow_loopback) {
            Err(reason)
        } else {
            let (code, out) = run_on(exe, a, &probe_command(&resolved.target), Duration::from_secs(30)).await.map_err(|e| e.message)?;
            if code == Some(255) {
                // The ssh to A itself failed: not a property of the route, so not cached (the copy finds the same).
                return Err(format!("ssh {a}: {}", out.trim()));
            }
            let (said, keys) = out.split_once(KEYS_MARK).unwrap_or((&out, ""));
            match parse_probe(code, said) {
                Ok(()) => same_machine(&host_keys(keys), &our_keys(exe, b, &resolved).await).map(|_| resolved.target.clone()),
                Err(reason) => Err(reason),
            }
        };
        self.0.lock().insert(key, (Instant::now(), result.clone()));
        result
    }

    #[cfg(test)]
    pub fn put(&self, a: &str, b: &str, at: Instant, r: Result<Target, String>) {
        self.0.lock().insert((a.into(), b.into()), (at, r));
    }

    #[cfg(test)]
    pub fn cached(&self, a: &str, b: &str) -> Option<Result<Target, String>> {
        self.0.lock().get(&(a.to_string(), b.to_string())).filter(|(at, _)| at.elapsed() < CACHE_FOR).map(|(_, r)| r.clone())
    }
}

/// Whether `sftp`'s words on A say its connection to B was lost.
pub fn lost(err: &str) -> bool {
    let e = err.to_lowercase();
    ["connection closed", "connection reset", "broken pipe", "connection refused", "timed out", "connection lost", "no route"].iter().any(|w| e.contains(w))
}

/// One `sftp -b -` on A for a job's direct files: commands in on stdin, each file's marker out on stdout.
pub struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    stderr: Arc<Mutex<String>>,
    /// `sftp`'s process id on A (for a cancel or a retry).
    pub pid: Option<u32>,
    seq: u64,
    /// Files sent through this login.
    pub files: u32,
}

impl Session {
    /// Starts the copier on A. `stale`: copiers an earlier attempt left on A (a dropped connection), stopped first so
    /// two never write one partial.
    pub async fn start(exe: &std::path::Path, a: &str, b: &Target, stale: &[u32]) -> FsResult<Session> {
        for pid in stale {
            kill_on(exe, a, *pid).await;
        }
        let mut cmd = ssh_tool(exe, a, "ssh");
        cmd.args(outer_args(&kelpie_proto::ssh::config_path(), a)).arg(direct_command(b)).stdin(Stdio::piped());
        let mut child = cmd.spawn().map_err(FsError::from)?;
        let stdin = child.stdin.take().ok_or_else(|| FsError::new(ErrKind::Other, "ssh stdin"))?;
        let stdout = child.stdout.take().ok_or_else(|| FsError::new(ErrKind::Other, "ssh stdout"))?;
        // stderr is read all along (a full pipe would stall the copier), keeping the last 8 KiB.
        let stderr = Arc::new(Mutex::new(String::new()));
        if let Some(mut e) = child.stderr.take() {
            let keep = stderr.clone();
            tokio::spawn(async move {
                let mut buf = [0u8; 2048];
                while let Ok(n) = e.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let mut k = keep.lock();
                    k.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if k.len() > 8192 {
                        let cut = k.len() - 8192;
                        let cut = (cut..k.len()).find(|i| k.is_char_boundary(*i)).unwrap_or(k.len());
                        k.drain(..cut);
                    }
                }
            });
        }
        let (tx, mut lines) = mpsc::channel(256);
        tokio::spawn(async move {
            let mut r = BufReader::new(stdout).lines();
            while let Ok(Some(l)) = r.next_line().await {
                if tx.send(l).await.is_err() {
                    break;
                }
            }
        });
        // The process id, past whatever a login banner prints first.
        let mut pid = None;
        for _ in 0..200 {
            match tokio::time::timeout(Duration::from_secs(30), lines.recv()).await {
                Ok(Some(l)) => {
                    if let Some(p) = find_pid(&l) {
                        pid = Some(p);
                        break;
                    }
                }
                _ => break,
            }
        }
        let mut s = Session { child, stdin, lines, stderr, pid, seq: 0, files: 0 };
        if s.pid.is_none() {
            let text = s.stderr_text();
            s.child.start_kill().ok();
            return Err(FsError::new(ErrKind::Connection, if text.is_empty() { format!("the copier did not start on {a}") } else { format!("ssh {a}: {text}") }));
        }
        Ok(s)
    }

    pub fn stderr_text(&self) -> String {
        self.stderr.lock().trim().to_string()
    }

    /// Hands the copier one file's lines; returns the marker to wait for.
    pub async fn send(&mut self, plan: DirectPlan, src: &str, partial: &str) -> FsResult<u64> {
        self.seq += 1;
        self.files += 1;
        let lines = batch_lines(plan, src, partial, self.seq).ok_or_else(|| FsError::invalid("the name does not fit a batch line"))?;
        self.stdin.write_all(lines.as_bytes()).await.map_err(|e| FsError::new(ErrKind::Connection, format!("the copier on the first host stopped ({e})")))?;
        self.stdin.flush().await.map_err(|e| FsError::new(ErrKind::Connection, e.to_string()))?;
        Ok(self.seq)
    }

    /// Waits for a file's marker (cancel-safe: dropping this loses nothing). The copier ending first is a lost connection.
    pub async fn done(&mut self, seq: u64) -> FsResult<()> {
        let mark = format!("kelpie-done-{seq}");
        while let Some(l) = self.lines.recv().await {
            if l.trim() == mark {
                return Ok(());
            }
        }
        let text = self.stderr_text();
        Err(FsError::new(ErrKind::Connection, if text.is_empty() { "the copier on the first host stopped".into() } else { text }))
    }

    /// Ends the copier: on A by its process id (only while it is still `sftp`), and the ssh here. False when A could
    /// not be told (the id is then kept and stopped before the next copier starts).
    pub async fn stop(mut self, exe: &std::path::Path, a: &str) -> bool {
        let told = match self.pid {
            Some(pid) => kill_on(exe, a, pid).await,
            None => true,
        };
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
        told
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ssh_to_the_first_host_never_forwards_or_opens_a_terminal() {
        let args = outer_args(std::path::Path::new("/d/ssh/config"), "a-host");
        assert!(args.contains(&"-T".to_string()));
        for o in ["ForwardAgent=no", "ForwardX11=no", "ClearAllForwardings=yes", "Tunnel=no", "RequestTTY=no", "RemoteCommand=none", "PermitLocalCommand=no"] {
            assert!(args.windows(2).any(|w| w == ["-o", o]), "{o}");
        }
        assert_eq!(args.last().unwrap(), "a-host");
    }

    #[test]
    fn values_reach_the_scripts_only_as_quoted_parameters_on_one_line() {
        let t = Target { hostname: "10.0.0.5".into(), port: 2222, user: "deploy".into() };
        let p = probe_command(&t);
        assert!(p.starts_with("sh -c '") && p.ends_with(" kelpie '10.0.0.5' '2222' 'deploy' '[10.0.0.5]:2222'"), "{p}");
        assert!(!PROBE.contains("10.0.0.5") && !PROBE.contains('\n') && !DIRECT.contains('\n'), "one line for csh login shells");
        assert_eq!(q("it's"), r"'it'\''s'");
        assert!(direct_command(&t).ends_with(" kelpie '2222' 'deploy' '10.0.0.5'"));
        for s in [PROBE, DIRECT] {
            assert!(s.contains("ForwardAgent=no") && s.contains("ClearAllForwardings=yes") && s.contains("StrictHostKeyChecking=yes"));
        }
        // IPv6 for sftp in brackets; known_hosts names without them.
        let v6 = Target { hostname: "fe80::1".into(), port: 22, user: "u".into() };
        assert_eq!(v6.host_for_sftp(), "[fe80::1]");
        assert_eq!(Target { port: 2200, ..v6 }.known_hosts_name(), "[fe80::1]:2200");
    }

    #[test]
    fn probe_exits_become_reasons() {
        assert_eq!(parse_probe(Some(0), ""), Ok(()));
        assert_eq!(parse_probe(Some(41), "").unwrap_err(), "the first host has no SSH client");
        assert_eq!(parse_probe(Some(NO_LOGIN), "Host key verification failed.").unwrap_err(), "the first host does not know the second host's key");
        assert_eq!(parse_probe(Some(NO_LOGIN), "deploy@10.0.0.5: Permission denied (publickey).").unwrap_err(), "the first host has no login of its own on the second host");
        assert_eq!(parse_probe(Some(NO_LOGIN), "ssh: connect to host 10.0.0.5 port 22: Connection timed out").unwrap_err(), "the first host cannot reach the second host");
    }

    #[test]
    fn a_host_reached_through_a_jump_or_a_local_address_never_goes_direct() {
        let base = parse_ssh_g("hostname 10.0.1.20\nport 22\nuser deploy\nproxyjump none\nproxycommand none\n").unwrap();
        assert_eq!(direct_refused(&base, false), None);
        let jumped = parse_ssh_g("hostname 10.0.1.20\nport 22\nuser deploy\nproxyjump bastion\n").unwrap();
        assert!(direct_refused(&jumped, false).unwrap().contains("jump host"));
        let proxied = parse_ssh_g("hostname 10.0.1.20\nport 22\nuser deploy\nproxycommand nc %h %p\n").unwrap();
        assert!(direct_refused(&proxied, false).is_some());
        for local in ["127.0.0.1", "localhost", "::1", "169.254.3.4", "fe80::2"] {
            let r = parse_ssh_g(&format!("hostname {local}\nport 2222\nuser u\n")).unwrap();
            assert!(direct_refused(&r, false).is_some(), "{local}");
            assert!(direct_refused(&r, true).is_none(), "{local}: the e2e's hosts live here");
        }
    }

    #[test]
    fn the_first_host_must_know_the_second_only_by_keys_this_computer_trusts() {
        let ours = host_keys("# Host [b]:22 found: line 3\n[b]:2222 ssh-ed25519 AAAAkey1\n[b]:2222 ssh-rsa AAAAkey2\n");
        assert_eq!(same_machine(&host_keys("|1|salt|hash ssh-ed25519 AAAAkey1\n"), &ours), Ok(()));
        assert!(same_machine(&host_keys("|1|x|y ssh-ed25519 AAAAother\n"), &ours).is_err(), "another machine under the same name");
        assert!(same_machine(&host_keys("|1|x|y ssh-ed25519 AAAAkey1\n|1|x|y ssh-rsa AAAAother\n"), &ours).is_err(), "one key of another machine is enough to refuse");
        assert!(same_machine(&host_keys(""), &ours).is_err());
        assert!(host_keys("@revoked b ssh-ed25519 AAAAx\n").is_empty());
        assert_eq!(host_keys("@cert-authority *.x ssh-ed25519 AAAAca\n").len(), 1);
    }

    #[test]
    fn empty_complete_and_fresh_files_are_planned_without_reput_refusing_them() {
        assert_eq!(direct_plan(0, 0), DirectPlan::Complete, "an empty file needs no sftp");
        assert_eq!(direct_plan(100, 100), DirectPlan::Complete, "a partial that is already complete");
        assert_eq!(direct_plan(0, 100), DirectPlan::Put);
        assert_eq!(direct_plan(40, 100), DirectPlan::Reput);
        assert_eq!(batch_lines(DirectPlan::Put, "/a/x", "/b/.x.t.kelpie-part", 7).unwrap(), "-put \"/a/x\" \"/b/.x.t.kelpie-part\"\n!echo kelpie-done-7\n");
        assert!(batch_lines(DirectPlan::Reput, "/a/x", "/b/p", 1).unwrap().starts_with("-reput "));
        assert_eq!(batch_lines(DirectPlan::Complete, "/a/x", "/b/p", 1).unwrap(), "");
        assert_eq!(batch_lines(DirectPlan::Put, "/a/x\"", "/b/p", 1), None);
    }

    #[test]
    fn the_process_id_is_found_past_a_login_banner() {
        assert_eq!(find_pid("Welcome to prod! Last login: Mon"), None);
        assert_eq!(find_pid("kelpie-pid 4242"), Some(4242));
        assert_eq!(find_pid("  kelpie-pid 7 "), Some(7));
        assert_eq!(find_pid("kelpie-pid x"), None);
        let k = kill_command(4242);
        assert!(k.contains("ps -p 4242") && k.contains("*sftp*"), "only while it is still sftp: {k}");
    }

    #[test]
    fn names_that_need_more_than_double_quotes_go_through_this_computer() {
        assert_eq!(batch_quote("/srv/a b/report (1).pdf").as_deref(), Some("\"/srv/a b/report (1).pdf\""));
        assert_eq!(batch_quote("/srv/it's $(x) `y`.txt").as_deref(), Some("\"/srv/it's $(x) `y`.txt\""), "no shell reads batch lines");
        for bad in ["/a\"b", "/a\\b", "/a\nb", "/a*b", "/a?b", "/a[1]", "/a{b}", "~/x", ""] {
            assert_eq!(batch_quote(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn reads_what_ssh_would_use_for_the_second_host() {
        let out = "user deploy\nhostname 10.0.0.5\nport 2222\nhostkeyalias prod-b\nuserknownhostsfile ~/.ssh/known_hosts ~/.ssh/known_hosts2\nglobalknownhostsfile /etc/ssh/ssh_known_hosts\n";
        let r = parse_ssh_g(out).unwrap();
        assert_eq!(r.target, Target { hostname: "10.0.0.5".into(), port: 2222, user: "deploy".into() });
        assert_eq!(r.host_key_alias.as_deref(), Some("prod-b"));
        assert_eq!(r.known_hosts.len(), 3);
        assert_eq!(parse_ssh_g("hostname -oProxyCommand=x\nuser a\nport 22\n"), None);
        assert_eq!(parse_ssh_g("port 22\n"), None);
    }

    #[test]
    fn a_lost_second_host_is_a_lost_connection_not_a_failed_file() {
        assert!(lost("Connection closed") && lost("client_loop: send disconnect: Broken pipe"));
        assert!(!lost("remote open(\"/x\"): Permission denied"));
    }

    #[test]
    fn probe_results_expire() {
        let p = Probes::default();
        let t = Target { hostname: "h".into(), port: 22, user: "u".into() };
        p.put("a", "b", Instant::now(), Ok(t.clone()));
        assert_eq!(p.cached("a", "b"), Some(Ok(t)));
        p.put("a", "c", Instant::now() - CACHE_FOR - Duration::from_secs(1), Err("x".into()));
        assert_eq!(p.cached("a", "c"), None);
    }
}
