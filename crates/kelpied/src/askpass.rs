//! `SSH_ASKPASS` helper. For the SSH hosts saved in Kelpie, the app starts `ssh` / `scp` with
//! `SSH_ASKPASS=<kelpied>`, `SSH_ASKPASS_REQUIRE=force` and `KELPIE_ASKPASS=<host id>`; ssh then runs
//! `kelpied "<prompt>"` instead of reading a terminal. Password and key-passphrase prompts are
//! answered from the OS credential store (written by the app's Hosts page). Anything else —
//! most of all "continue connecting to an unknown host?" — is declined, so ssh fails at once
//! instead of hanging; the app asks the user to check the host key first.

/// Prints the answer for `prompt` and returns the exit code (1 = no answer).
pub fn run(prompt: &str) -> i32 {
    let hosts = kelpie_proto::durable::read_either(&kelpie_proto::ssh::hosts_path(), |b| serde_json::from_slice::<serde_json::Value>(b)).unwrap_or(serde_json::Value::Null);
    let preferred = std::env::var("KELPIE_ASKPASS").unwrap_or_default();
    match answer(prompt, &hosts, &preferred).and_then(|key| secret(&key)) {
        Some(s) => {
            println!("{s}");
            0
        }
        None => 1,
    }
}

/// The credential-store key that answers `prompt`.
fn answer(prompt: &str, hosts: &serde_json::Value, preferred: &str) -> Option<String> {
    let lower = prompt.to_lowercase();
    if lower.contains("passphrase") {
        // "Enter passphrase for key 'C:\Users\me/.ssh/id_ed25519': "
        let path = prompt.split('\'').nth(1)?;
        return Some(kelpie_proto::ssh_passphrase_key(path));
    }
    if !lower.contains("password") {
        return None;
    }
    // "me@10.0.0.2's password: " or, keyboard-interactive, "(me@10.0.0.2) Password: ".
    let (user, host) = prompt_target(prompt)?;
    let list = hosts.get("hosts").and_then(|h| h.as_array()).cloned().unwrap_or_default();
    let field = |h: &serde_json::Value, k: &str| h.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let matches = |h: &serde_json::Value| field(h, "hostname").eq_ignore_ascii_case(&host) && (field(h, "user").is_empty() || field(h, "user") == user);
    // The host the connection was started for wins (a jump host has its own entry).
    let found = list.iter().find(|h| field(h, "id") == preferred && matches(h)).or_else(|| list.iter().find(|h| matches(h)))?;
    Some(kelpie_proto::ssh_password_key(&field(found, "id")))
}

/// `user` and `host` from a password prompt.
fn prompt_target(prompt: &str) -> Option<(String, String)> {
    let at = prompt.find('@')?;
    let user = prompt[..at].rsplit(|c: char| c.is_whitespace() || c == '(').next()?.to_string();
    let rest = &prompt[at + 1..];
    let end = rest.find(['\'', ')', ' ', ':']).unwrap_or(rest.len());
    let host = rest[..end].to_string();
    (!user.is_empty() && !host.is_empty()).then_some((user, host))
}

fn secret(key: &str) -> Option<String> {
    kelpie_proto::secret_get(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_prompt_targets() {
        assert_eq!(prompt_target("me@10.0.0.2's password: "), Some(("me".into(), "10.0.0.2".into())));
        assert_eq!(prompt_target("(deploy@box.example.com) Password: "), Some(("deploy".into(), "box.example.com".into())));
        assert_eq!(prompt_target("Password: "), None);
    }

    #[test]
    fn maps_prompts_to_secrets() {
        let hosts = serde_json::json!({"hosts": [
            {"id": "a", "hostname": "10.0.0.2", "user": "me"},
            {"id": "b", "hostname": "10.0.0.2", "user": ""},
            {"id": "jump", "hostname": "bastion", "user": "ops"},
        ]});
        assert_eq!(answer("me@10.0.0.2's password: ", &hosts, "").as_deref(), Some("ssh-password:a"));
        assert_eq!(answer("me@10.0.0.2's password: ", &hosts, "b").as_deref(), Some("ssh-password:b"));
        // A jump host's prompt while connecting to another host.
        assert_eq!(answer("ops@bastion's password: ", &hosts, "a").as_deref(), Some("ssh-password:jump"));
        assert_eq!(answer("x@unknown's password: ", &hosts, "a"), None);
        assert_eq!(answer("Are you sure you want to continue connecting (yes/no/[fingerprint])? ", &hosts, "a"), None);
        assert_eq!(answer("Enter passphrase for key 'C:\\Users\\Me/.ssh/id_ed25519': ", &hosts, "a").as_deref(), Some(kelpie_proto::ssh_passphrase_key("C:/Users/Me/.ssh/id_ed25519").as_str()));
    }
}
