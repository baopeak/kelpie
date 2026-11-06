//! Host ↔ host copies: **direct** (host A pushes to B with A's own login to B) when that works without lending A the
//! person's keys, else **through this computer** (two SFTP sessions, nothing written to this computer's disk).
//!
//! - The ssh to A always has `-o ForwardAgent=no -o ForwardX11=no -o ClearAllForwardings=yes`, whatever the hosts'
//!   config says: Kelpie never lends A the agent.
//! - The probe runs a fixed script on A with quoted positional values: `sftp` and `ssh` must exist there, and
//!   `ssh -o BatchMode=yes -p <port> -l <user> <hostname> true` must work. `BatchMode` means A's own known hosts must
//!   already trust B; an unknown key fails and nothing is accepted on A. B's hostname, port and user are what this
//!   computer's ssh would use for B (`ssh -G`), so A logs in as the same user Kelpie uses for B.
//! - The direct copy of one file runs `sftp -b -` on A with `reput "<src>" "<partial>"` (which resumes), and reports
//!   its process id first, so a cancel can stop it there. Names that would need more than plain double quotes in a
//!   batch file (`"`, `\`, glob characters, controls) go through this computer instead.
//! - Probe results are cached per (A, B) for 10 minutes.

use std::collections::HashMap;
use std::process::Stdio;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use super::{ErrKind, FsError, FsResult};

/// How long a probe result holds.
pub const CACHE_FOR: Duration = Duration::from_secs(10 * 60);

/// B as A has to reach it: what this computer's ssh would use for B.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Target {
    pub hostname: String,
    pub port: u16,
    pub user: String,
}

/// POSIX shell single quoting.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The ssh options for the connection to A (before the destination): Kelpie's terminal options, and never any
/// forwarding, whatever the host's config says.
pub fn outer_args(config: &std::path::Path, a: &str) -> Vec<String> {
    let mut args = vec!["-F".to_string(), config.to_string_lossy().into_owned()];
    for o in ["ForwardAgent=no", "ForwardX11=no", "ClearAllForwardings=yes", "NumberOfPasswordPrompts=1", "ConnectTimeout=15", "ServerAliveInterval=15", "ServerAliveCountMax=3", "LogLevel=ERROR"] {
        args.push("-o".into());
        args.push(o.into());
    }
    args.push(a.to_string());
    args
}

/// Exit code of the probe script when A has no `sftp` or `ssh` of its own.
const NO_CLIENT: i32 = 41;
/// The probe's ssh to B failed (its own 255, renumbered so it never looks like the ssh to A failing).
const NO_LOGIN: i32 = 42;

/// The probe, run on A as `sh -c <script> kelpie <hostname> <port> <user>`: values travel as positional parameters,
/// never inside the script.
pub const PROBE: &str = r#"command -v sftp >/dev/null 2>&1 && command -v ssh >/dev/null 2>&1 || exit 41
err=$(ssh -o BatchMode=yes -o ConnectTimeout=5 -o StrictHostKeyChecking=yes -o ForwardAgent=no -p "$2" -l "$3" "$1" true 2>&1 >/dev/null)
code=$?
[ "$code" -eq 255 ] && code=42
printf '%s' "$err"
exit $code"#;

pub fn probe_command(t: &Target) -> String {
    format!("sh -c {} kelpie {} {} {}", q(PROBE), q(&t.hostname), q(&t.port.to_string()), q(&t.user))
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

/// A path as one argument of an sftp batch line, or `None` when plain double quotes cannot carry it safely.
pub fn batch_quote(path: &str) -> Option<String> {
    let unsafe_char = |c: char| c.is_control() || matches!(c, '"' | '\\' | '*' | '?' | '[' | ']' | '{' | '}' | '~');
    (!path.is_empty() && !path.chars().any(unsafe_char)).then(|| format!("\"{path}\""))
}

/// The direct copy, run on A as `sh -c <script> kelpie <port> <user> <hostname>`: prints its process id, then becomes
/// `sftp` in batch mode, reading the `reput` line from stdin.
pub const DIRECT: &str = r#"echo $$
exec sftp -q -b - -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=yes -o ForwardAgent=no -P "$1" "$2@$3""#;

pub fn direct_command(t: &Target) -> String {
    format!("sh -c {} kelpie {} {} {}", q(DIRECT), q(&t.port.to_string()), q(&t.user), q(&t.hostname))
}

/// The batch line that copies `src` (on A) to `partial` (on B), continuing what is there.
pub fn reput_line(src: &str, partial: &str) -> Option<String> {
    Some(format!("reput {} {}\n", batch_quote(src)?, batch_quote(partial)?))
}

/// What `ssh -G` says this computer's ssh would use for `dest`.
pub fn parse_ssh_g(out: &str) -> Option<Target> {
    let mut hostname = None;
    let mut port = None;
    let mut user = None;
    for line in out.lines() {
        let mut it = line.splitn(2, ' ');
        match (it.next(), it.next()) {
            (Some("hostname"), Some(v)) => hostname = Some(v.trim().to_string()),
            (Some("port"), Some(v)) => port = v.trim().parse().ok(),
            (Some("user"), Some(v)) => user = Some(v.trim().to_string()),
            _ => {}
        }
    }
    let t = Target { hostname: hostname?, port: port.unwrap_or(22), user: user? };
    // Values that a remote shell or ssh could read as something else are never passed on.
    let plain = |s: &str| !s.is_empty() && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-:%[]".contains(c));
    (plain(&t.hostname) && plain(&t.user)).then_some(t)
}

fn ssh_command(exe: &std::path::Path, dest: &str) -> Command {
    let mut cmd = Command::new(kelpie_proto::ssh::program("ssh"));
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
pub async fn resolve(exe: &std::path::Path, dest: &str) -> FsResult<Target> {
    let mut cmd = ssh_command(exe, dest);
    cmd.arg("-F").arg(kelpie_proto::ssh::config_path()).arg("-G").arg(dest);
    let out = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await.map_err(|_| FsError::new(ErrKind::Other, "ssh -G did not answer"))?.map_err(FsError::from)?;
    parse_ssh_g(&String::from_utf8_lossy(&out.stdout)).ok_or_else(|| FsError::new(ErrKind::Other, format!("the address of {dest} could not be read from the ssh config")))
}

/// Runs `command` on A over ssh (no forwarding); exit code and stdout + stderr.
pub async fn run_on(exe: &std::path::Path, a: &str, command: &str, timeout: Duration) -> FsResult<(Option<i32>, String)> {
    let mut cmd = ssh_command(exe, a);
    cmd.args(outer_args(&kelpie_proto::ssh::config_path(), a)).arg(command);
    let out = tokio::time::timeout(timeout, cmd.output()).await.map_err(|_| FsError::new(ErrKind::Connection, format!("{a} did not answer in time")))?.map_err(FsError::from)?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code(), text))
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
        let target = resolve(exe, b).await.map_err(|e| e.message)?;
        let (code, out) = run_on(exe, a, &probe_command(&target), Duration::from_secs(30)).await.map_err(|e| e.message)?;
        if code == Some(255) {
            // The ssh to A itself failed: not a property of the route, so not cached (the copy finds the same).
            return Err(format!("ssh {a}: {}", out.trim()));
        }
        let result = parse_probe(code, &out).map(|_| target);
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

/// Whether sftp's words on A say its connection to B was lost (it exits 1 then, not 255).
pub fn lost(err: &str) -> bool {
    let e = err.to_lowercase();
    ["connection closed", "connection reset", "broken pipe", "connection refused", "timed out", "connection lost", "no route"].iter().any(|w| e.contains(w))
}

/// A direct copy running on A.
pub struct Direct {
    child: Child,
    /// `sftp`'s process id on A (for a cancel).
    pub pid: Option<u32>,
    stderr: Option<tokio::process::ChildStderr>,
}

impl Direct {
    /// Starts `sftp -b -` on A and hands it the `reput` line.
    pub async fn start(exe: &std::path::Path, a: &str, b: &Target, line: &str) -> FsResult<Direct> {
        let mut cmd = ssh_command(exe, a);
        cmd.args(outer_args(&kelpie_proto::ssh::config_path(), a)).arg(direct_command(b)).stdin(Stdio::piped());
        let mut child = cmd.spawn().map_err(FsError::from)?;
        let mut stdin = child.stdin.take().ok_or_else(|| FsError::new(ErrKind::Other, "ssh stdin"))?;
        stdin.write_all(line.as_bytes()).await.map_err(|e| FsError::new(ErrKind::Connection, e.to_string()))?;
        drop(stdin); // the end of the batch
        let mut stdout = child.stdout.take().ok_or_else(|| FsError::new(ErrKind::Other, "ssh stdout"))?;
        // The first line is the process id on A.
        let mut first = Vec::new();
        let mut byte = [0u8; 1];
        let pid = loop {
            match tokio::time::timeout(Duration::from_secs(30), stdout.read(&mut byte)).await {
                Ok(Ok(1)) if byte[0] == b'\n' => break String::from_utf8_lossy(&first).trim().parse().ok(),
                Ok(Ok(1)) if first.len() < 32 => first.push(byte[0]),
                _ => break None,
            }
        };
        let stderr = child.stderr.take();
        Ok(Direct { child, pid, stderr })
    }

    /// Waits for the copy to end: `Ok` when `sftp` succeeded.
    pub async fn wait(&mut self) -> FsResult<()> {
        let status = self.child.wait().await.map_err(FsError::from)?;
        let mut err = String::new();
        if let Some(mut e) = self.stderr.take() {
            let _ = e.read_to_string(&mut err).await;
        }
        let err = err.trim().chars().take(400).collect::<String>();
        match status.code() {
            Some(0) => Ok(()),
            // ssh to A failed, or A lost B: retried like any lost connection, resuming with `reput`.
            Some(255) | None => Err(FsError::new(ErrKind::Connection, if err.is_empty() { "the connection to the first host ended".into() } else { err })),
            Some(_) if lost(&err) => Err(FsError::new(ErrKind::Connection, err)),
            Some(_) => Err(FsError::new(ErrKind::Other, if err.is_empty() { "the copy on the first host failed".into() } else { format!("the copy on the first host failed: {err}") })),
        }
    }

    /// Stops the copy on A (by its process id there) and the ssh here.
    pub async fn stop(mut self, exe: &std::path::Path, a: &str) {
        if let Some(pid) = self.pid {
            let _ = run_on(exe, a, &format!("kill {pid} 2>/dev/null; sleep 1; kill -9 {pid} 2>/dev/null; true"), Duration::from_secs(20)).await;
        }
        let _ = self.child.start_kill();
        let _ = self.child.wait().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ssh_to_the_first_host_never_forwards_anything() {
        let args = outer_args(std::path::Path::new("/d/ssh/config"), "a-host");
        for o in ["ForwardAgent=no", "ForwardX11=no", "ClearAllForwardings=yes"] {
            assert!(args.windows(2).any(|w| w == ["-o", o]), "{o}");
        }
        assert_eq!(args.last().unwrap(), "a-host");
        // Options given on the command line win over the config file in OpenSSH: they come before the destination.
        assert!(args.iter().position(|a| a == "ForwardAgent=no").unwrap() < args.len() - 1);
    }

    #[test]
    fn values_reach_the_scripts_only_as_quoted_parameters() {
        let t = Target { hostname: "10.0.0.5".into(), port: 2222, user: "deploy".into() };
        let p = probe_command(&t);
        assert!(p.starts_with("sh -c '") && p.ends_with(" kelpie '10.0.0.5' '2222' 'deploy'"), "{p}");
        assert!(!PROBE.contains("10.0.0.5"));
        assert_eq!(q("it's"), r"'it'\''s'");
        assert!(direct_command(&t).ends_with(" kelpie '2222' 'deploy' '10.0.0.5'"));
        assert!(DIRECT.contains("ForwardAgent=no") && PROBE.contains("BatchMode=yes"));
    }

    #[test]
    fn probe_exits_become_reasons() {
        assert_eq!(parse_probe(Some(0), ""), Ok(()));
        assert_eq!(parse_probe(Some(41), "").unwrap_err(), "the first host has no SSH client");
        assert_eq!(parse_probe(Some(NO_LOGIN), "Host key verification failed.").unwrap_err(), "the first host does not know the second host's key");
        assert_eq!(parse_probe(Some(NO_LOGIN), "deploy@10.0.0.5: Permission denied (publickey).").unwrap_err(), "the first host has no login of its own on the second host");
        assert_eq!(parse_probe(Some(NO_LOGIN), "ssh: connect to host 10.0.0.5 port 22: Connection timed out").unwrap_err(), "the first host cannot reach the second host");
        assert_eq!(parse_probe(Some(NO_LOGIN), "ssh: Could not resolve hostname db: Name or service not known").unwrap_err(), "the first host cannot reach the second host");
    }

    #[test]
    fn names_that_need_more_than_double_quotes_go_through_this_computer() {
        assert_eq!(batch_quote("/srv/a b/report (1).pdf").as_deref(), Some("\"/srv/a b/report (1).pdf\""));
        assert_eq!(batch_quote("/srv/ünïcødé.txt").as_deref(), Some("\"/srv/ünïcødé.txt\""));
        for bad in ["/a\"b", "/a\\b", "/a\nb", "/a*b", "/a?b", "/a[1]", "/a{b}", "~/x", ""] {
            assert_eq!(batch_quote(bad), None, "{bad:?}");
        }
        assert_eq!(reput_line("/x/a", "/y/.a.t.kelpie-part").as_deref(), Some("reput \"/x/a\" \"/y/.a.t.kelpie-part\"\n"));
        assert_eq!(reput_line("/x/a\"", "/y/p"), None);
    }

    #[test]
    fn reads_what_ssh_would_use_for_the_second_host() {
        let out = "user deploy\nhostname 10.0.0.5\nport 2222\nforwardagent yes\n";
        assert_eq!(parse_ssh_g(out), Some(Target { hostname: "10.0.0.5".into(), port: 2222, user: "deploy".into() }));
        assert_eq!(parse_ssh_g("hostname -oProxyCommand=x\nuser a\nport 22\n"), None);
        assert_eq!(parse_ssh_g("hostname h\nuser 'x;y'\nport 22\n"), None);
        assert_eq!(parse_ssh_g("port 22\n"), None);
    }

    #[test]
    fn a_lost_second_host_is_a_lost_connection_not_a_failed_file() {
        assert!(lost("Connection closed") && lost("client_loop: send disconnect: Broken pipe") && lost("ssh: connect to host 127.0.0.1 port 47923: Connection refused"));
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
