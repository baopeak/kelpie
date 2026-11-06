//! How Kelpie starts OpenSSH, shared by the app and the daemon so both run the same `ssh` the same way:
//! which program, the generated `-F` config, the askpass environment that answers saved passwords, and the
//! one rule for what may be handed to ssh as a destination.

use std::path::{Path, PathBuf};

/// An OpenSSH program. On Windows, the system's own (`System32\OpenSSH`) when present: a Git Bash one
/// earlier on PATH reads `C:\…` as a host named `C` and cannot reach the Windows ssh-agent service.
pub fn program(name: &str) -> PathBuf {
    if cfg!(windows) {
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("OpenSSH").join(format!("{name}.exe"));
        if system.is_file() {
            return system;
        }
    }
    PathBuf::from(name)
}

/// The generated ssh config every Kelpie ssh / scp / sftp reads (`<data>/ssh/config`).
pub fn config_path() -> PathBuf {
    crate::data_dir().join("ssh").join("config")
}

/// The saved hosts (`<data>/ssh/hosts.json`), written by the app only.
pub fn hosts_path() -> PathBuf {
    crate::data_dir().join("ssh").join("hosts.json")
}

/// Environment that lets ssh ask Kelpie (`exe` = `kelpied`) for saved secrets instead of a terminal.
/// `host_id` is the saved host the connection is for (its password wins when a jump host has the same
/// address); empty when the destination is not a saved host.
pub fn askpass_env(exe: &Path, host_id: &str) -> Vec<(String, String)> {
    vec![
        ("SSH_ASKPASS".into(), exe.to_string_lossy().into_owned()),
        ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        ("KELPIE_ASKPASS".into(), if host_id.is_empty() { "-".into() } else { host_id.to_string() }),
        // Older OpenSSH only uses askpass with a display set.
        ("DISPLAY".into(), std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into())),
    ]
}

/// The id of the saved host named `dest` (an alias, or `user@alias`), read from `hosts.json`. Never writes; a
/// missing or unreadable file gives `None` (the connection then runs without a saved secret). `user@alias` with a
/// user other than the saved host's gives `None`: a saved password is never offered for another account. A saved
/// host without a user logs in as whoever ssh's config says: `user@alias` gets its password only when `ssh -G`
/// reports that same user for the alias.
pub fn host_id_for_alias(dest: &str) -> Option<String> {
    let bytes = std::fs::read(hosts_path()).ok()?;
    host_id_in(&serde_json::from_slice(&bytes).ok()?, dest, config_user)
}

/// The user ssh logs in as for `alias` with Kelpie's config (`ssh -G`, which reads the config and connects nowhere).
/// Asked only for `user@alias` against a saved host without a user.
fn config_user(alias: &str) -> Option<String> {
    if !valid_dest(alias) {
        return None;
    }
    let mut cmd = std::process::Command::new(program("ssh"));
    cmd.arg("-F").arg(config_path()).arg("-G").arg(alias).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().ok().filter(|o| o.status.success())?;
    String::from_utf8_lossy(&out.stdout).lines().find_map(|l| l.strip_prefix("user ").map(|u| u.trim().to_string()))
}

fn host_id_in(hosts: &serde_json::Value, dest: &str, config_user: impl FnOnce(&str) -> Option<String>) -> Option<String> {
    let (user, alias) = match dest.rsplit_once('@') {
        Some((u, a)) => (Some(u), a),
        None => (None, dest),
    };
    let h = hosts.get("hosts")?.as_array()?.iter().find(|h| h.get("alias").and_then(|a| a.as_str()) == Some(alias))?;
    if let Some(u) = user {
        let saved = h.get("user").and_then(|v| v.as_str()).unwrap_or("");
        let same = if saved.is_empty() { config_user(alias).as_deref() == Some(u) } else { saved == u };
        if !same {
            return None;
        }
    }
    h.get("id")?.as_str().map(str::to_string)
}

/// What may be handed to `ssh` as its destination: an alias, `user@host`, an IPv4 or bracketed IPv6
/// address (with a zone). Nothing that ssh could read as an option, a second word or a quote.
pub fn valid_dest(dest: &str) -> bool {
    !dest.is_empty() && dest.len() <= 255 && !dest.starts_with('-') && dest.chars().all(|c| c.is_ascii_alphanumeric() || "._@:%[]-".contains(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations() {
        for ok in ["prod", "deploy@prod", "me@10.0.0.5", "[::1]", "me@[fe80::1%eth0]", "box-1.example.com", "a_b"] {
            assert!(valid_dest(ok), "{ok}");
        }
        for bad in ["", "-oProxyCommand=x", "a b", "a'b", "a\"b", "a\nb", "a;b", "a`b", "$(x)", "a\\b", "x".repeat(256).as_str()] {
            assert!(!valid_dest(bad), "{bad:?}");
        }
    }

    #[test]
    fn askpass_keys() {
        let env = askpass_env(Path::new("kelpied"), "h1");
        let keys: Vec<&str> = env.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["SSH_ASKPASS", "SSH_ASKPASS_REQUIRE", "KELPIE_ASKPASS", "DISPLAY"]);
        assert_eq!(env[1].1, "force");
        assert_eq!(env[2].1, "h1");
        assert_eq!(askpass_env(Path::new("kelpied"), "")[2].1, "-");
    }

    #[test]
    fn host_ids_by_alias() {
        let hosts = serde_json::json!({"hosts": [{"id": "a", "alias": "prod", "user": "deploy"}, {"id": "b", "alias": "dev"}]});
        let none = |_: &str| -> Option<String> { None };
        let me = |alias: &str| (alias == "dev").then(|| "me".to_string());
        assert_eq!(host_id_in(&hosts, "dev", none).as_deref(), Some("b"));
        assert_eq!(host_id_in(&hosts, "other", none), None);
        assert_eq!(host_id_in(&hosts, "deploy@prod", none).as_deref(), Some("a"), "the saved user");
        assert_eq!(host_id_in(&hosts, "root@prod", me), None, "another account never gets the saved password");
        // A saved host without a user: the user ssh's config gives the alias decides.
        assert_eq!(host_id_in(&hosts, "me@dev", me).as_deref(), Some("b"), "the user ssh logs in as anyway");
        assert_eq!(host_id_in(&hosts, "root@dev", me), None, "another account");
        assert_eq!(host_id_in(&hosts, "me@dev", none), None, "ssh could not tell: no password");
        assert_eq!(host_id_in(&serde_json::json!({}), "dev", none), None);
    }

    #[cfg(windows)]
    #[test]
    fn prefers_system_openssh() {
        let p = program("ssh");
        if p.is_absolute() {
            assert!(p.to_string_lossy().to_lowercase().contains("system32\\openssh"), "{p:?}");
        }
    }
}
