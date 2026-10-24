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

/// Writes a remote file; with `expected_mtime`, refuses when it changed meanwhile (seconds).
pub async fn write_file(root: &str, rel: &str, content: Vec<u8>, expected_mtime: Option<u64>) -> CmdResult<(bool, u64)> {
    let f = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    let check = match expected_mtime {
        Some(m) => format!("if [ -f \"$f\" ]; then m=$(stat -c %Y \"$f\" 2>/dev/null || stat -f %m \"$f\"); [ \"$m\" = {m} ] || {{ echo conflict; exit 4; }}; fi; "),
        None => String::new(),
    };
    let script = format!("f={}; {check}cat > \"$f\" && (stat -c %Y \"$f\" 2>/dev/null || stat -f %m \"$f\")", q(&f.path));
    let out = exec(&f, &script, Some(content), Duration::from_secs(60)).await?;
    if out.code == Some(4) {
        return Ok((false, 0));
    }
    if !out.ok {
        return Err(out.stderr);
    }
    Ok((true, String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)))
}

pub async fn create(root: &str, rel: &str, dir: bool) -> CmdResult<()> {
    let f = parse_root(&guard(root, rel)?.to_string_lossy()).ok_or("invalid path")?;
    let script = if dir { format!("mkdir -p -- {}", q(&f.path)) } else { format!("f={}; [ -e \"$f\" ] && {{ echo 'already exists' >&2; exit 1; }}; mkdir -p -- \"$(dirname -- \"$f\")\" && : > \"$f\"", q(&f.path)) };
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
}
