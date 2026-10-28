//! Remote projects over SSH. A remote project is rooted at `ssh://[user@]host[:port]/abs/path`
//! (`host` may be a `~/.ssh/config` alias). Everything runs through the system `ssh` client, so
//! the user's config, keys and agent apply; commands run non-interactively (`BatchMode`) and
//! never prompt. File operations, git and search for such a root become small POSIX shell
//! commands on the host; terminals and agents are `ssh -t` sessions owned by the local daemon
//! (see `crates/kelpied/src/agents.rs`, optionally inside the host's tmux).

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use serde::Serialize;

use crate::fs_ops::{err, CmdResult};

/// Settings → Remote: SSH config file. Kelpie's generated config (saved hosts) includes it.
#[tauri::command]
pub fn remote_configure(config: Option<String>) {
    crate::ssh_hosts::set_user_config(config);
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRoot {
    /// ssh destination without the port (`user@host` or an alias).
    pub dest: String,
    pub port: Option<u16>,
    /// Absolute path on the host.
    pub path: String,
}

impl RemoteRoot {
    pub fn root(&self) -> String {
        match self.port {
            Some(p) => format!("ssh://{}:{p}{}", self.dest, self.path),
            None => format!("ssh://{}{}", self.dest, self.path),
        }
    }
}

pub fn is_remote(root: &str) -> bool {
    root.starts_with("ssh://")
}

/// `ssh://[user@]host[:port]/abs/path`.
pub fn parse_root(root: &str) -> Option<RemoteRoot> {
    let rest = root.strip_prefix("ssh://")?;
    let slash = rest.find('/')?;
    let (authority, path) = rest.split_at(slash);
    let (dest, port) = match authority.rsplit_once(':') {
        Some((d, p)) if !d.is_empty() && p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (d.to_string(), p.parse().ok()),
        _ => (authority.to_string(), None),
    };
    if dest.is_empty() || dest.starts_with('-') || dest.contains([' ', '\'', '"', '`', '$']) {
        return None;
    }
    let path = path.trim_end_matches('/');
    Some(RemoteRoot { dest, port, path: if path.is_empty() { "/".into() } else { path.to_string() } })
}

/// A project-relative path inside the remote root (no `..` escapes), as a root string.
pub fn guard(root: &str, rel: &str) -> CmdResult<PathBuf> {
    let r = parse_root(root).ok_or("invalid remote root")?;
    let mut parts: Vec<&str> = Vec::new();
    for p in rel.split(['/', '\\']) {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err("path escapes the project folder".into());
                }
            }
            p => parts.push(p),
        }
    }
    let joined = if parts.is_empty() { r.path.clone() } else { format!("{}/{}", r.path.trim_end_matches('/'), parts.join("/")) };
    Ok(PathBuf::from(RemoteRoot { path: joined, ..r }.root()))
}

/// POSIX shell single quoting.
pub fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Never prompts: saved passwords / passphrases come from Kelpie's askpass (`ssh_env`), anything
/// else (no saved secret, an unknown host key) fails instead of waiting for a terminal.
fn ssh_base(r: &RemoteRoot) -> Vec<String> {
    let mut args = vec!["-F".to_string(), crate::ssh_hosts::config_path().to_string_lossy().into_owned()];
    args.extend(["-o", "NumberOfPasswordPrompts=1", "-o", "ConnectTimeout=15", "-o", "LogLevel=ERROR"].map(str::to_string));
    if let Some(p) = r.port {
        args.extend(["-p".to_string(), p.to_string()]);
    }
    args.push(r.dest.clone());
    args.push("--".into());
    args
}

/// Askpass environment for the saved host `r` points at (if any).
fn ssh_env(r: &RemoteRoot) -> Vec<(String, String)> {
    let alias = r.dest.rsplit('@').next().unwrap_or(&r.dest);
    crate::ssh_hosts::askpass_env(&crate::ssh_hosts::host_by_alias(alias).map(|h| h.id).unwrap_or_default())
}

pub struct Output {
    pub ok: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

/// Runs `script` with `sh -c` on the host (stdin optional).
pub async fn exec(r: &RemoteRoot, script: &str, stdin: Option<Vec<u8>>, timeout: Duration) -> CmdResult<Output> {
    let mut cmd = tokio::process::Command::new(crate::ssh_hosts::program("ssh"));
    cmd.args(ssh_base(r))
        .envs(ssh_env(r))
        .arg(format!("sh -c {}", q(script)))
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);
    let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { "ssh is not installed (OpenSSH client)".to_string() } else { format!("ssh: {e}") })?;
    if let Some(data) = stdin {
        use tokio::io::AsyncWriteExt;
        let mut sin = child.stdin.take().ok_or("ssh stdin")?;
        tokio::spawn(async move {
            let _ = sin.write_all(&data).await;
            let _ = sin.shutdown().await;
        });
    }
    let out = tokio::time::timeout(timeout, child.wait_with_output()).await.map_err(|_| format!("{} did not answer in time", r.dest))?.map_err(err)?;
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // 255 = ssh itself failed (connection, auth).
    if out.status.code() == Some(255) {
        return Err(format!("ssh {}: {}", r.dest, if stderr.is_empty() { "connection failed".into() } else { stderr }));
    }
    Ok(Output { ok: out.status.success(), code: out.status.code(), stdout: out.stdout, stderr })
}

pub async fn exec_ok(r: &RemoteRoot, script: &str) -> CmdResult<String> {
    let out = exec(r, script, None, Duration::from_secs(60)).await?;
    if !out.ok {
        return Err(if out.stderr.is_empty() { format!("exit code {:?}", out.code) } else { out.stderr });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Blocking variant for synchronous callers (GitLab project lookup).
pub fn exec_blocking(r: &RemoteRoot, script: &str) -> Option<String> {
    let mut cmd = std::process::Command::new(crate::ssh_hosts::program("ssh"));
    cmd.args(ssh_base(r)).envs(ssh_env(r)).arg(format!("sh -c {}", q(script))).stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git` in the remote folder `dir` (a root string), with Kelpie's usual config overrides.
pub async fn git(dir: &str, args: &[&str]) -> CmdResult<crate::git::GitOutput> {
    let r = parse_root(dir).ok_or("invalid remote root")?;
    let script = format!(
        "cd {} && GIT_OPTIONAL_LOCKS=0 GIT_TERMINAL_PROMPT=0 git -c core.quotepath=off -c color.ui=false {}",
        q(&r.path),
        args.iter().map(|a| q(a)).collect::<Vec<_>>().join(" ")
    );
    let out = exec(&r, &script, None, Duration::from_secs(120)).await?;
    Ok(crate::git::GitOutput { ok: out.ok, stdout: out.stdout, stderr: out.stderr })
}

// ---------- files ----------

/// Entries of a folder: (name, is_dir, is_symlink, size). Symlinks to folders count as folders.
pub async fn list_dir(root: &str, rel: &str) -> CmdResult<Vec<(String, bool, bool, u64)>> {
    let dir = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    // One line per entry: type (d/f/l), size, name. `ls -p` would be shorter but unreadable
    // for names with newlines; find's -printf is GNU only, so a portable loop.
    let script = format!(
        "cd {} || exit 3; for f in .[!.]* ..?* *; do [ -e \"$f\" ] || [ -L \"$f\" ] || continue; if [ -d \"$f\" ]; then t=d; s=0; else t=f; s=$(wc -c < \"$f\" 2>/dev/null || echo 0); fi; if [ -L \"$f\" ]; then l=1; else l=0; fi; printf '%s\\t%s\\t%s\\t%s\\0' \"$t\" \"$l\" \"$s\" \"$f\"; done",
        q(&dir.path)
    );
    let out = exec(&dir, &script, None, Duration::from_secs(30)).await?;
    if out.code == Some(3) {
        return Err(format!("{} is not a folder", dir.path));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut entries: Vec<(String, bool, bool, u64)> = text
        .split('\0')
        .filter_map(|rec| {
            let mut it = rec.splitn(4, '\t');
            let (t, l, s, name) = (it.next()?, it.next()?, it.next()?, it.next()?);
            (!name.is_empty() && name != ".git").then(|| (name.to_string(), t == "d", l == "1", s.trim().parse().unwrap_or(0)))
        })
        .collect();
    entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| crate::fs_ops::natural_cmp(&a.0, &b.0)));
    Ok(entries)
}

/// Content (up to `max` bytes), size and mtime (seconds) of a remote file.
pub async fn read_file(root: &str, rel: &str, max: u64) -> CmdResult<(Vec<u8>, u64, u64)> {
    let f = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    let script = format!(
        "f={}; [ -f \"$f\" ] || exit 3; m=$(stat -c %Y \"$f\" 2>/dev/null || stat -f %m \"$f\"); s=$(wc -c < \"$f\"); printf '%s %s\\n' \"$s\" \"$m\"; head -c {max} \"$f\"",
        q(&f.path)
    );
    let out = exec(&f, &script, None, Duration::from_secs(60)).await?;
    if out.code == Some(3) {
        return Err(format!("{rel}: not a file"));
    }
    if !out.ok {
        return Err(out.stderr);
    }
    let nl = out.stdout.iter().position(|b| *b == b'\n').ok_or("unexpected reply")?;
    let head = String::from_utf8_lossy(&out.stdout[..nl]).to_string();
    let mut it = head.split_whitespace();
    let size = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mtime = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    Ok((out.stdout[nl + 1..].to_vec(), size, mtime))
}

/// POSIX `cksum` of `bytes` as the command prints it ("<crc> <bytes>"): CRC-32 (polynomial 0x04C11DB7, most significant bit
/// first) of the bytes and then of their count, complemented.
pub fn cksum(bytes: &[u8]) -> String {
    fn step(crc: u32, b: u8) -> u32 {
        let mut c = crc ^ ((b as u32) << 24);
        for _ in 0..8 {
            c = if c & 0x8000_0000 != 0 { (c << 1) ^ 0x04C1_1DB7 } else { c << 1 };
        }
        c
    }
    let mut crc = bytes.iter().fold(0u32, |c, b| step(c, *b));
    let mut n = bytes.len();
    while n > 0 {
        crc = step(crc, (n & 0xff) as u8);
        n >>= 8;
    }
    format!("{} {}", !crc, bytes.len())
}

/// The shell part of `write_file`. Its values arrive as positional parameters (`set -- …` in front, every value quoted), never
/// pasted into the text, so nothing in a file name can reach the script: `$1` the path, `$2` the project's folder, `$3` the
/// cksum of what is written, `$4` the cksum the file must still have ("" for none), `$5` the time (seconds) it must still have
/// ("" for none), `$6` 1 to keep the target inside the project.
const WRITE_SCRIPT: &str = r#"f=$1; root=$2; new=$3; sum=$4; mtime=$5; confine=$6
t=
trap 'rm -f "$t"' EXIT
command -v mktemp >/dev/null 2>&1 || exit 8
resolve() { p=$1; n=0; while [ -L "$p" ] && [ "$n" -lt 40 ]; do l=$(readlink -- "$p") || return 1; case "$l" in /*) p=$l;; *) p=$(dirname -- "$p")/$l;; esac; n=$((n+1)); done; [ -L "$p" ] && return 1; printf '%s' "$p"; }
if [ -L "$f" ]; then f=$(resolve "$f") || exit 6; fi
if [ "$confine" = 1 ]; then
  R=$(cd "$root" && pwd -P) || exit 7
  D=$(cd "$(dirname -- "$f")" && pwd -P) || exit 7
  case "$R" in /) ;; *) case "$D/" in "$R"/*) ;; *) exit 7;; esac;; esac
fi
if [ -n "$mtime" ] && [ -f "$f" ]; then m=$(stat -c %Y "$f" 2>/dev/null || stat -f %m "$f"); [ "$m" = "$mtime" ] || { echo conflict; exit 4; }; fi
if [ -n "$sum" ]; then
  [ -f "$f" ] || { echo conflict; exit 4; }
  c=$(cksum < "$f" | tr -s ' ' ' '); [ "$c" = "$sum" ] || { echo conflict; exit 4; }
fi
atomic=0
# A file made by mktemp is made exclusively (a name that is already there, a link included, is never opened) with mode 0600.
if [ ! -e "$f" ] || [ -O "$f" ]; then
  if t=$(mktemp "$f.kelpie-tmp.XXXXXX" 2>/dev/null); then atomic=1; else t=; fi
fi
[ "$atomic" = 1 ] || t=$(mktemp) || exit 1
cat > "$t" || exit 1
[ "$(cksum < "$t" | tr -s ' ' ' ')" = "$new" ] || exit 5
if [ "$atomic" = 1 ]; then
  if [ -f "$f" ]; then m=$(stat -c %a "$f" 2>/dev/null || stat -f %Lp "$f" 2>/dev/null); else m=$(printf '%o' $((0666 & ~$(umask)))); fi
  [ -z "$m" ] || chmod "$m" "$t"
  mv -f "$t" "$f" || exit 1
else
  cat "$t" > "$f" || exit 1
fi
stat -c %Y "$f" 2>/dev/null || stat -f %m "$f"
"#;

/// The script `write_file` runs on the host for `content` (see there), with its values in front as parameters.
fn write_script(path: &str, project: &str, content: &[u8], expected_mtime: Option<u64>, expected_cksum: Option<&str>, confine: bool) -> CmdResult<String> {
    if let Some(sum) = expected_cksum {
        if sum.is_empty() || !sum.chars().all(|c| c.is_ascii_digit() || c == ' ') {
            return Err("invalid checksum".into());
        }
    }
    let mtime = expected_mtime.map(|m| m.to_string()).unwrap_or_default();
    Ok(format!(
        "set -- {} {} {} {} {} {}\n{WRITE_SCRIPT}",
        q(path),
        q(project),
        q(&cksum(content)),
        q(expected_cksum.unwrap_or("")),
        q(&mtime),
        if confine { "1" } else { "0" }
    ))
}

/// What the script's exit says: written (with the file's new time), refused because the file changed, or an error.
fn write_outcome(out: &Output) -> CmdResult<(bool, u64)> {
    match out.code {
        Some(4) => return Ok((false, 0)),
        Some(5) => return Err("the file did not arrive complete: nothing was changed".into()),
        Some(6) => return Err("could not follow the link to the file".into()),
        Some(7) => return Err("the file is a link that leads out of the project: nothing was written".into()),
        Some(8) => return Err("this host has no mktemp: the file was not written".into()),
        _ => {}
    }
    if !out.ok {
        return Err(out.stderr.clone());
    }
    Ok((true, String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)))
}

/// Writes a remote file. A link is written through to its target, whose own folder gets the temporary file: the content goes
/// there first and replaces the file in one step, so a connection that drops half way leaves the old file, never a truncated
/// one; what arrived is checked (size and `cksum`) before anything is replaced. A folder that cannot be written to falls back to
/// writing the file in place, but only once the whole content is known to have arrived; so does a file that belongs to someone else, because a new file would be the ssh user's. The mode of the file stays.
///
/// With `expected_mtime` (seconds) and / or `expected_cksum` (the POSIX `cksum` of the content the caller read, "<crc> <bytes>")
/// it refuses when the file changed meanwhile: seconds alone cannot tell two edits of one second apart. With a checksum the file
/// must still be there (a conditional write never recreates it). `confine`: the target, after links, must be inside the project's
/// folder (the spec index asks for it; the editor's save keeps writing wherever its links lead).
pub async fn write_file(root: &str, rel: &str, content: Vec<u8>, expected_mtime: Option<u64>, expected_cksum: Option<&str>, confine: bool) -> CmdResult<(bool, u64)> {
    let f = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    let top = parse_root(root).ok_or("invalid remote root")?;
    let script = write_script(&f.path, &top.path, &content, expected_mtime, expected_cksum, confine)?;
    write_outcome(&exec(&f, &script, Some(content), Duration::from_secs(60)).await?)
}

pub async fn create(root: &str, rel: &str, dir: bool) -> CmdResult<()> {
    let f = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    let script = if dir { format!("mkdir -p -- {}", q(&f.path)) } else { format!("f={}; {{ [ -e \"$f\" ] || [ -L \"$f\" ]; }} && {{ echo 'already exists' >&2; exit 1; }}; mkdir -p -- \"$(dirname -- \"$f\")\" && : > \"$f\"", q(&f.path)) };
    exec_ok(&f, &script).await.map(|_| ())
}

pub async fn rename(root: &str, from: &str, to: &str) -> CmdResult<()> {
    let a = parse_root(&guard(root, from)?.to_string_lossy()).ok_or("invalid path")?;
    let b = parse_root(&guard(root, to)?.to_string_lossy()).ok_or("invalid path")?;
    exec_ok(&a, &format!("[ -e {1} ] && {{ echo 'already exists' >&2; exit 1; }}; mv -- {0} {1}", q(&a.path), q(&b.path))).await.map(|_| ())
}

/// Removes a file or folder on the host (there is no trash to move it to).
pub async fn delete(root: &str, rel: &str) -> CmdResult<()> {
    if rel.trim_matches(['/', '\\', '.']).is_empty() {
        return Err("refusing to delete the project folder".into());
    }
    let f = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    exec_ok(&f, &format!("rm -rf -- {}", q(&f.path))).await.map(|_| ())
}

// ---------- search ----------

#[derive(Serialize)]
pub struct Hit {
    pub path: String,
    pub line: u64,
    pub column: u64,
    pub text: String,
}

/// `grep` in the project (or one folder of it); .git and binaries skipped.
pub async fn search_text(root: &str, query: &str, regex: bool, case_sensitive: bool, folder: Option<&str>, max: usize) -> CmdResult<(Vec<Hit>, bool)> {
    let r = parse_root(root).ok_or("invalid remote root")?;
    let sub = match folder.filter(|f| !f.is_empty()) {
        Some(f) => {
            guard(root, f)?;
            f.trim_matches('/').to_string()
        }
        None => ".".into(),
    };
    let flags = format!("-rnI{}{}", if regex { "E" } else { "F" }, if case_sensitive { "" } else { "i" });
    let script = format!("cd {} && grep {flags} --exclude-dir=.git --exclude-dir=node_modules -e {} -- {} 2>/dev/null | head -n {}", q(&r.path), q(query), q(&sub), max + 1);
    let out = exec(&r, &script, None, Duration::from_secs(60)).await?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut hits: Vec<Hit> = text
        .lines()
        .filter_map(|l| {
            let (path, rest) = l.split_once(':')?;
            let (line, content) = rest.split_once(':')?;
            let line = line.parse().ok()?;
            let needle = if case_sensitive { query.to_string() } else { query.to_lowercase() };
            let hay = if case_sensitive { content.to_string() } else { content.to_lowercase() };
            let column = if regex { 1 } else { hay.find(&needle).map_or(1, |c| c as u64 + 1) };
            Some(Hit { path: path.trim_start_matches("./").to_string(), line, column, text: content.chars().take(400).collect() })
        })
        .collect();
    let truncated = hits.len() > max;
    hits.truncate(max);
    Ok((hits, truncated))
}

/// Files of the project for quick open: tracked + untracked-not-ignored, or a bounded find.
pub async fn list_files(root: &str) -> CmdResult<Vec<String>> {
    let r = parse_root(root).ok_or("invalid remote root")?;
    let script = format!(
        "cd {} || exit 3; git ls-files --cached --others --exclude-standard 2>/dev/null | head -n 50000 || true; [ -d .git ] || find . -type d \\( -name .git -o -name node_modules \\) -prune -o -type f -print | head -n 20000",
        q(&r.path)
    );
    let out = exec(&r, &script, None, Duration::from_secs(60)).await?;
    Ok(String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim_start_matches("./").to_string()).filter(|l| !l.is_empty()).collect())
}

// ---------- hosts ----------

/// Saved hosts' aliases, then `Host` aliases from the user's SSH config (no wildcards).
#[tauri::command]
pub async fn remote_hosts() -> CmdResult<Vec<String>> {
    crate::fs_ops::blocking(|| {
        let mut hosts: Vec<String> = crate::ssh_hosts::load().hosts.into_iter().map(|h| h.alias).collect();
        let imported = crate::ssh_hosts::parse_config(&std::fs::read_to_string(crate::ssh_hosts::user_config()).unwrap_or_default());
        for h in imported {
            if !hosts.contains(&h.alias) {
                hosts.push(h.alias);
            }
        }
        Ok(hosts)
    })
    .await
}

#[derive(Serialize)]
pub struct HostCheck {
    os: String,
    /// `ID` of `/etc/os-release` (ubuntu, debian, alpine…); empty where there is none (macOS).
    distro: String,
    home: String,
    /// The account it logs in as. `root`: uid 0 without `IS_SANDBOX=1` in the login shell, where
    /// Claude Code refuses to skip permission prompts.
    user: String,
    root: bool,
    git: bool,
    tmux: bool,
    /// Agent CLIs found on the host's PATH (login shell).
    agents: Vec<String>,
}

/// The probe's `uid=` value (`0`, `1000`, `0-sandbox1`): root, without the `IS_SANDBOX=1` that
/// lets Claude Code skip permission prompts as root (the daemon's launch checks the same).
fn root_refused(v: &str) -> bool {
    let (uid, sandbox) = v.trim().split_once("-sandbox").unwrap_or((v.trim(), ""));
    uid == "0" && sandbox != "1"
}

/// Connects to `dest` (`user@host[:port]` or an alias) and reports what it has.
#[tauri::command]
pub async fn remote_check(dest: String, agents: Vec<String>) -> CmdResult<HostCheck> {
    let r = parse_root(&format!("ssh://{dest}/")).ok_or("invalid host")?;
    let bins = agents.iter().filter(|a| a.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).cloned().collect::<Vec<_>>();
    // A login shell, so PATH matches what terminals get. The loop's status is the last tool's;
    // `true` keeps a missing one from failing the whole check.
    let probe = format!(
        "printf 'os=%s\\nhome=%s\\ndistro=%s\\nuser=%s\\nuid=%s\\n' \"$(uname -sr)\" \"$HOME\" \"$( (. /etc/os-release 2>/dev/null && printf %s \"$ID\") )\" \"$(id -un)\" \"$(id -u)${{IS_SANDBOX:+-sandbox$IS_SANDBOX}}\"; for b in git tmux {}; do command -v \"$b\" >/dev/null 2>&1 && echo \"has=$b\"; done; true",
        bins.join(" ")
    );
    let script = format!("exec \"${{SHELL:-/bin/sh}}\" -lc {}", q(&probe));
    let text = exec_ok(&r, &script).await?;
    let mut c = HostCheck { os: String::new(), distro: String::new(), home: String::new(), user: String::new(), root: false, git: false, tmux: false, agents: Vec::new() };
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("os=") {
            c.os = v.trim().into();
        } else if let Some(v) = line.strip_prefix("user=") {
            c.user = v.trim().into();
        } else if let Some(v) = line.strip_prefix("uid=") {
            c.root = root_refused(v);
        } else if let Some(v) = line.strip_prefix("distro=") {
            c.distro = v.trim().to_ascii_lowercase();
        } else if let Some(v) = line.strip_prefix("home=") {
            c.home = v.trim().into();
        } else if let Some(v) = line.strip_prefix("has=") {
            match v.trim() {
                "git" => c.git = true,
                "tmux" => c.tmux = true,
                other => c.agents.push(other.to_string()),
            }
        }
    }
    if c.home.is_empty() {
        return Err(format!("{dest} did not answer the probe"));
    }
    Ok(c)
}

#[derive(Serialize, Debug, PartialEq)]
pub struct AddedUser {
    home: String,
    /// The user was there already (it only got root's keys).
    existed: bool,
    /// sshd's AllowUsers / AllowGroups / DenyUsers / DenyGroups lines: the user may need adding
    /// there before it can log in.
    sshd: Vec<String>,
}

/// A login name `useradd` takes everywhere (and never root).
fn valid_user_name(n: &str) -> bool {
    let mut c = n.chars();
    n.len() <= 32 && n != "root" && c.next().is_some_and(|f| f.is_ascii_lowercase() || f == '_') && c.all(|x| x.is_ascii_lowercase() || x.is_ascii_digit() || x == '_' || x == '-')
}

/// Run as root: creates the user (no password, so it logs in with keys only), then gives it
/// root's authorized SSH keys after any it has (in order, without duplicates). An existing user
/// only gets the keys, and only a regular account (a login uid and shell) whose `.ssh` holds no
/// symlinks: root writes there, and must not follow a link the user planted.
const ADD_USER: &str = r#"
[ "$(id -u)" = 0 ] || { echo "log in as root to add a user" >&2; exit 3; }
command -v useradd >/dev/null 2>&1 || { echo "useradd is not available on this host" >&2; exit 3; }
existed=1
if ! id -u "$n" >/dev/null 2>&1; then
  existed=0
  useradd -m -U -s "$(command -v bash || echo /bin/sh)" -p '*' "$n" || exit 4
fi
[ "$(id -u "$n")" != 0 ] || { echo "$n is a root account" >&2; exit 3; }
home=$(getent passwd "$n" | cut -d: -f6)
g=$(id -gn "$n")
[ -n "$home" ] && [ -d "$home" ] || { echo "$n has no home folder" >&2; exit 3; }
k="$home/.ssh/authorized_keys"
if [ "$existed" = 1 ]; then
  umin=$(awk '$1 == "UID_MIN" { print $2 }' /etc/login.defs 2>/dev/null)
  [ "$(id -u "$n")" -ge "${umin:-1000}" ] || { echo "$n is a system account: choose another name" >&2; exit 3; }
  case "$(getent passwd "$n" | cut -d: -f7)" in */nologin|*/false|"") echo "$n cannot log in (no shell): choose another name" >&2; exit 3;; esac
  [ ! -L "$home/.ssh" ] && [ ! -L "$k" ] || { echo "$n's ~/.ssh is a symbolic link: Kelpie does not write through it" >&2; exit 3; }
fi
install -d -m 700 -o "$n" -g "$g" "$home/.ssh" || exit 4
t=$(mktemp) || exit 4
{ [ -f "$k" ] && cat "$k"; echo; cat "$HOME/.ssh/authorized_keys" 2>/dev/null; echo; } | grep -v '^[[:space:]]*$' | awk '!seen[$0]++' > "$t"
grep -q . "$HOME/.ssh/authorized_keys" 2>/dev/null || { rm -f "$t"; echo "root has no authorized SSH keys to give $n" >&2; exit 3; }
install -m 600 -o "$n" -g "$g" "$t" "$k" || { rm -f "$t"; exit 4; }
rm -f "$t"
command -v restorecon >/dev/null 2>&1 && restorecon -R "$home/.ssh" >/dev/null 2>&1
printf 'home=%s\nexisted=%s\n' "$home" "$existed"
sshd -T 2>/dev/null | grep -Ei '^(allowusers|allowgroups|denyusers|denygroups) ' | sed 's/^/sshd=/'
true
"#;

/// Adds a regular user on a host logged in as root, for agents that refuse to run as root
/// unguarded (Claude Code's Yolo mode).
#[tauri::command]
pub async fn remote_add_user(dest: String, name: String) -> CmdResult<AddedUser> {
    if !valid_user_name(&name) {
        return Err("invalid user name".into());
    }
    let r = parse_root(&format!("ssh://{dest}/")).ok_or("invalid host")?;
    parse_added_user(&exec_ok(&r, &format!("n={}\n{ADD_USER}", q(&name))).await?)
}

fn parse_added_user(text: &str) -> CmdResult<AddedUser> {
    let mut u = AddedUser { home: String::new(), existed: false, sshd: Vec::new() };
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("home=") {
            u.home = v.trim().into();
        } else if let Some(v) = line.strip_prefix("existed=") {
            u.existed = v.trim() == "1";
        } else if let Some(v) = line.strip_prefix("sshd=") {
            u.sshd.push(v.trim().into());
        }
    }
    if u.home.is_empty() {
        return Err("the host did not confirm the new user".into());
    }
    Ok(u)
}

/// Folders of an absolute path on the host (for choosing a project folder).
#[tauri::command]
pub async fn remote_folders(dest: String, path: String) -> CmdResult<Vec<String>> {
    let r = parse_root(&format!("ssh://{dest}{}", if path.starts_with('/') { path.clone() } else { format!("/{path}") })).ok_or("invalid host or path")?;
    let script = format!("cd {} || exit 3; for f in */; do [ -d \"$f\" ] && printf '%s\\0' \"${{f%/}}\"; done", q(&r.path));
    let out = exec(&r, &script, None, Duration::from_secs(30)).await?;
    if out.code == Some(3) {
        return Err(format!("{path} is not a folder on {dest}"));
    }
    let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout).split('\0').filter(|s| !s.is_empty() && *s != "*").map(str::to_string).collect();
    v.sort_by(|a, b| crate::fs_ops::natural_cmp(a, b));
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_roots() {
        assert_eq!(parse_root("ssh://devbox/home/me/app"), Some(RemoteRoot { dest: "devbox".into(), port: None, path: "/home/me/app".into() }));
        assert_eq!(parse_root("ssh://me@10.0.0.2:2222/srv/x/"), Some(RemoteRoot { dest: "me@10.0.0.2".into(), port: Some(2222), path: "/srv/x".into() }));
        assert_eq!(parse_root("ssh://box/").unwrap().path, "/");
        assert!(parse_root("ssh://-oProxyCommand=x/p").is_none());
        assert!(parse_root("C:/x").is_none());
        assert_eq!(parse_root("ssh://me@h:22/p").unwrap().root(), "ssh://me@h:22/p");
    }

    #[test]
    fn user_names_useradd_takes() {
        for ok in ["dev", "_svc", "web-1", "a_b"] {
            assert!(valid_user_name(ok), "{ok}");
        }
        for bad in ["", "root", "Dev", "1dev", "-dev", "de v", "dev;rm", "dev$", &"a".repeat(33)] {
            assert!(!valid_user_name(bad), "{bad}");
        }
    }

    #[test]
    fn root_unless_a_sandbox() {
        assert!(root_refused("0"));
        assert!(root_refused("0-sandbox0"));
        assert!(!root_refused("0-sandbox1"));
        assert!(!root_refused("1000"));
        assert!(!root_refused("1000-sandbox1"));
    }

    #[test]
    fn reads_the_added_user() {
        let u = parse_added_user("home=/home/dev\nexisted=0\nsshd=allowusers root\n").unwrap();
        assert_eq!(u, AddedUser { home: "/home/dev".into(), existed: false, sshd: vec!["allowusers root".into()] });
        assert!(parse_added_user("existed=1\n").is_err());
    }

    #[test]
    fn guards_remote_paths() {
        assert_eq!(guard("ssh://box/home/me/app", "src/a.rs").unwrap().to_string_lossy(), "ssh://box/home/me/app/src/a.rs");
        assert_eq!(guard("ssh://box/home/me/app", "").unwrap().to_string_lossy(), "ssh://box/home/me/app");
        assert!(guard("ssh://box/home/me/app", "../etc/passwd").is_err());
        assert_eq!(guard("ssh://box/home/me/app", "a/../b").unwrap().to_string_lossy(), "ssh://box/home/me/app/b");
    }

    #[test]
    fn quotes_for_sh() {
        assert_eq!(q("it's"), r"'it'\''s'");
        assert_eq!(q("$(rm -rf /)"), "'$(rm -rf /)'");
    }

    #[test]
    fn cksum_is_the_posix_one() {
        assert_eq!(cksum(b"123456789"), "930766865 9");
        assert_eq!(cksum(b""), "4294967295 0");
        assert_eq!(cksum(b"a\r\nb"), "893934516 4");
        assert!(cksum(&vec![7u8; 70_000]).ends_with(" 70000"));
    }

    // The write script, run by a local shell (Git Bash on Windows, with its link emulation on; none: the tests do nothing).
    fn shell() -> Option<std::path::PathBuf> {
        let candidates = [std::env::var("KELPIE_TEST_BASH").unwrap_or_default(), r"C:\Program Files\Git\bin\bash.exe".into(), "/bin/sh".into()];
        candidates.into_iter().filter(|c| !c.is_empty()).map(std::path::PathBuf::from).find(|p| p.exists())
    }

    fn sh(shell: &std::path::Path, script: &str, stdin: &[u8]) -> Output {
        use std::io::Write;
        let mut child = std::process::Command::new(shell)
            .arg("-c")
            .arg(script)
            .env("MSYS", "winsymlinks:lnk")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(stdin).ok();
        let o = child.wait_with_output().unwrap();
        Output { ok: o.status.success(), code: o.status.code(), stdout: o.stdout, stderr: String::from_utf8_lossy(&o.stderr).trim().to_string() }
    }

    fn scratch(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("kelpie-write-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn a_remote_write_keeps_links_and_never_commits_a_short_file() {
        let Some(shell) = shell() else { return };
        let dir = scratch("w");
        let (project, outside) = (format!("{dir}/project"), format!("{dir}/outside"));
        let run = |script: &str| sh(&shell, script, b"");
        run(&format!("mkdir -p {p} {o} && printf 'old\\n' > {p}/real.md && printf 'elsewhere\\n' > {o}/target.md && ln -s real.md {p}/link.md && ln -s {o}/target.md {p}/out.md && ln -s missing.md {p}/dangling.md", p = q(&project), o = q(&outside)));
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
        let write = |rel: &str, body: &[u8], mtime: Option<u64>, sum: Option<&str>, confine: bool| {
            let script = write_script(&format!("{project}/{rel}"), &project, body, mtime, sum, confine).unwrap();
            write_outcome(&sh(&shell, &script, body))
        };

        // A plain file: replaced whole, no temporary file left.
        assert!(write("real.md", b"new\n", None, None, true).unwrap().0);
        assert_eq!(read(&format!("{project}/real.md")), "new\n");
        assert!(!std::fs::read_dir(&project).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("kelpie-tmp")));

        // A link inside the project is written through: the link stays and its target changes.
        assert!(write("link.md", b"through\n", None, Some(&cksum(b"new\n")), true).unwrap().0);
        assert_eq!(read(&format!("{project}/real.md")), "through\n");
        assert!(sh(&shell, &format!("[ -L {}/link.md ]", q(&project)), b"").ok, "the link is still a link");

        // The editor's save (no confine) writes through a link that leads out; the spec index (confine) refuses and changes nothing.
        let err = write("out.md", b"nope\n", None, Some(&cksum(b"elsewhere\n")), true).unwrap_err();
        assert!(err.contains("leads out of the project"), "{err}");
        assert_eq!(read(&format!("{outside}/target.md")), "elsewhere\n");
        assert!(write("out.md", b"saved\n", None, None, false).unwrap().0);
        assert_eq!(read(&format!("{outside}/target.md")), "saved\n");
        assert!(sh(&shell, &format!("[ -L {}/out.md ]", q(&project)), b"").ok);

        // A short input (the connection dropped part way) is refused and the file is untouched.
        let whole = b"a whole new content of some length\n";
        let script = write_script(&format!("{project}/real.md"), &project, whole, None, None, false).unwrap();
        let err = write_outcome(&sh(&shell, &script, &whole[..10])).unwrap_err();
        assert!(err.contains("did not arrive complete"), "{err}");
        assert_eq!(read(&format!("{project}/real.md")), "through\n");
        assert!(!std::fs::read_dir(&project).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("kelpie-tmp")));

        // A conditional write refuses a changed file, and a file that is gone (it never recreates it); the editor's save recreates.
        assert!(!write("real.md", b"x\n", None, Some(&cksum(b"something else\n")), true).unwrap().0);
        std::fs::remove_file(format!("{project}/real.md")).unwrap();
        assert!(!write("real.md", b"x\n", None, Some(&cksum(b"through\n")), true).unwrap().0);
        assert!(!std::path::Path::new(&format!("{project}/real.md")).exists());
        assert!(write("real.md", b"again\n", None, None, false).unwrap().0);
        assert_eq!(read(&format!("{project}/real.md")), "again\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_remote_write_makes_its_temp_file_exclusively_and_reads_names_as_names() {
        let Some(shell) = shell() else { return };
        let dir = scratch("x");
        let project = format!("{dir}/project");
        let log = format!("{dir}/mktemp.log");
        std::fs::create_dir_all(&project).unwrap();
        let run = |script: &str, stdin: &[u8]| {
            use std::io::Write;
            // `mktemp` is a function that notes how it is asked, and then does the real thing.
            let script = format!("mktemp() {{ echo \"$@\" >> {}; command mktemp \"$@\"; }}
{script}", q(&log));
            let mut c = std::process::Command::new(&shell).arg("-c").arg(&script).env("MSYS", "winsymlinks:lnk").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
            c.stdin.take().unwrap().write_all(stdin).ok();
            let o = c.wait_with_output().unwrap();
            Output { ok: o.status.success(), code: o.status.code(), stdout: o.stdout, stderr: String::from_utf8_lossy(&o.stderr).trim().to_string() }
        };
        let put = |name: &str, body: &[u8]| {
            let script = write_script(&format!("{project}/{name}"), &project, body, None, None, true).unwrap();
            write_outcome(&run(&script, body))
        };

        // Names that an attacker plants where a temporary file would go: a file, the template itself, a link to a file elsewhere.
        std::fs::write(format!("{dir}/victim.md"), "victim\n").unwrap();
        for n in ["real.md.kelpie-tmp.XXXXXX", "real.md.kelpie-tmp.1", "real.md.kelpie-tmp.4242"] {
            std::fs::write(format!("{project}/{n}"), "planted\n").unwrap();
        }
        assert!(run(&format!("ln -s ../victim.md {}/real.md.kelpie-tmp.7", q(&project)), b"").ok);
        assert!(put("real.md", b"written\n").unwrap().0);
        assert_eq!(std::fs::read_to_string(format!("{project}/real.md")).unwrap(), "written\n");
        for n in ["real.md.kelpie-tmp.XXXXXX", "real.md.kelpie-tmp.1", "real.md.kelpie-tmp.4242"] {
            assert_eq!(std::fs::read_to_string(format!("{project}/{n}")).unwrap(), "planted\n", "{n} was not touched");
        }
        assert_eq!(std::fs::read_to_string(format!("{dir}/victim.md")).unwrap(), "victim\n", "a planted link was not followed");
        let asked = std::fs::read_to_string(&log).unwrap();
        assert!(asked.contains(&format!("{project}/real.md.kelpie-tmp.XXXXXX")), "the temporary file is made by mktemp, in the target's folder: {asked}");

        // File names that look like the script's own placeholders, or like shell.
        for name in ["a__NEW__b.md", "a__CHECKS__b.md", "a__CONFINE__b.md", "a__F__b.md", "a__ROOT__b.md", "x'y z $(echo hi) ;.md", "$1.md", "-n.md", "`tick`.md"] {
            let body = format!("body of {name}\n");
            assert!(put(name, body.as_bytes()).unwrap().0, "{name}");
            assert_eq!(std::fs::read_to_string(format!("{project}/{name}")).unwrap(), body, "{name}");
            // And a conditional write of such a file names the file's own checksum.
            let again = write_script(&format!("{project}/{name}"), &project, b"again\n", None, Some(&cksum(body.as_bytes())), true).unwrap();
            assert!(write_outcome(&run(&again, b"again\n")).unwrap().0, "{name}");
        }
        // Nothing but what was planted, what was written and the two helper files is in the folder.
        let names: Vec<String> = std::fs::read_dir(&project).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert!(!names.iter().any(|n| n.contains("kelpie-tmp") && !n.starts_with("real.md.kelpie-tmp.")), "{names:?}");
        // A project folder that is the root of the host: every target is inside it.
        let script = write_script(&format!("{project}/real.md"), "/", b"root\n", None, None, true).unwrap();
        assert!(write_outcome(&run(&script, b"root\n")).unwrap().0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
