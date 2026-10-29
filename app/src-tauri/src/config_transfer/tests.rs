use super::*;

const GH: &str = "ghp_0123456789abcdefghijABCDEFGHIJ012345";
const ANT: &str = "sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123";
const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAA\n-----END OPENSSH PRIVATE KEY-----";
const DB_PW: &str = "hunter2hunter2";
const URL_PW: &str = "s3cr3tpw";
const HOST_PW: &str = "host-password-9182";
const APPROVED: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-config-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The window's sections with a credential in every place one could hide.
fn seeded_raw() -> Map<String, Value> {
    json!({
        "settings": {
            "fontSize": 15,
            "theme": "light",
            "notifyToken": "ntfytok123456",
            "accounts": [{ "id": "a", "agent": "claude", "label": "Work", "dir": "C:/Users/me/.claude-work" }],
            "activeAccounts": { "claude": "a" },
            "sshConfig": "C:/Users/me/.ssh/config",
            "integrations": { "linear": true, "jira": { "url": format!("https://me:{URL_PW}@jira.example.com"), "email": "me@example.com" }, "gitlabHosts": [] },
        },
        "quickCommands": [
            { "id": "q1", "label": "Publish", "text": format!("export GITHUB_TOKEN={GH} && npm publish"), "action": "terminal", "enter": true, "projectId": null, "extra": GH },
            { "id": "q2", "label": "", "text": "no label" },
        ],
        "layouts": [{ "id": "l1", "name": format!("Grid {ANT}"), "template": { "type": "split", "dir": "row", "sizes": [0.5, 0.5], "children": [{ "type": "leaf", "kind": "claude" }, { "type": "leaf", "kind": null, "secret": GH }] } }],
        "automations": [{
            "id": "au1", "name": "Nightly", "enabled": true, "kind": "", "prompt": format!("use {ANT}"), "cwd": "C:/work/p", "project_id": "p1",
            "args": [], "env": { "GITHUB_TOKEN": GH, "PUBLIC_FLAG": "1" }, "every": "daily", "time": "02:30", "weekday": 1,
            "last_run": 5, "last_session": "s1", "last_error": "boom", "next_run": 9, "cap_day": "2026-10-29", "cap_count": 3,
            "action": "shell", "shell": { "program": "python", "argv": ["scripts/x.py", "{file}"], "approved_hash": APPROVED },
            "approval": APPROVED, "command_hash": APPROVED, "command_approved": true,
            "trigger": { "event": "file", "glob": "**/*.md", "daily_cap": 5 },
        }],
        "agents": [{ "kind": "claude", "label": "Claude Code", "args": ["--model", "opus"], "permission": "plan", "env": { "GITHUB_TOKEN": GH, "DB_PASSWORD": DB_PW, "PUBLIC_FLAG": "1", "PRIVATE": PEM } }],
    })
    .as_object()
    .unwrap()
    .clone()
}

fn seeded_hosts() -> Vault {
    let host = json!({
        "id": "h1", "label": "Box", "alias": "box", "hostname": "10.0.0.5", "user": "me", "auth": "password",
        "identityFile": "~/.ssh/id_ed25519", "env": { "API_TOKEN": "tok_abcdef123456", "LANG": "C.UTF-8" },
        "notes": format!("token {GH}"), "os": "ubuntu", "lastConnected": 1234,
        "password": HOST_PW, "passphrase": HOST_PW, "privateKey": PEM,
    });
    Vault { version: 1, hosts: vec![serde_json::from_value(host).unwrap()], forwards: Vec::new() }
}

fn all_sections() -> Vec<String> {
    SECTIONS.iter().map(|s| s.to_string()).collect()
}

fn export(with_values: bool) -> Value {
    let plugins = vec![json!({ "id": "acme-tools", "name": "Acme", "version": "1.0.0", "token": GH })];
    export_doc(&seeded_raw(), &all_sections(), with_values, Some(&seeded_hosts()), Some(plugins), "2026-10-29T10:00:00.000Z").unwrap()
}

#[test]
fn the_file_has_the_format_header_and_only_the_wanted_sections() {
    let doc = export_doc(&seeded_raw(), &["quickCommands".into(), "plugins".into()], false, None, Some(Vec::new()), "2026-10-29T10:00:00.000Z").unwrap();
    assert_eq!(doc["kind"], KIND);
    assert_eq!(doc["version"], VERSION);
    assert_eq!(doc["exportedAt"], "2026-10-29T10:00:00.000Z");
    assert_eq!(doc["appVersion"], env!("CARGO_PKG_VERSION"));
    let names: Vec<&String> = doc["sections"].as_object().unwrap().keys().collect();
    assert_eq!(names, ["quickCommands", "plugins"]);
    assert!(export_doc(&seeded_raw(), &["nonsense".into()], false, None, None, "").is_err());
}

#[test]
fn no_secret_reaches_the_file_with_or_without_values() {
    for with_values in [false, true] {
        let dir = temp_dir(&format!("secrets-{with_values}"));
        let path = dir.join("out.json");
        write_file_atomic(&path, &serde_json::to_vec_pretty(&export(with_values)).unwrap()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        for secret in [GH, ANT, "PRIVATE KEY", "b3BlbnNzaC1rZXktdjEAAAAA", DB_PW, URL_PW, HOST_PW, APPROVED, "tok_abcdef123456", "ntfytok123456", ".claude-work", ".ssh/config\""] {
            assert!(!text.contains(secret), "{secret} leaked (values {with_values}):\n{text}");
        }
        for gone in ["\"accounts\":", "\"activeAccounts\":", "\"sshConfig\":", "approved_hash", "\"approval\":", "command_hash", "\"password\":", "\"passphrase\":", "privateKey", "last_run", "cap_count"] {
            assert!(!text.contains(gone), "{gone} written (values {with_values})");
        }
        // Names stay; a key path is not a secret.
        for kept in ["GITHUB_TOKEN", "DB_PASSWORD", "PUBLIC_FLAG", "~/.ssh/id_ed25519", "LANG"] {
            assert!(text.contains(kept), "{kept} missing (values {with_values})");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn env_values_only_when_asked_and_never_secret_ones() {
    let env = |doc: &Value| doc["sections"]["agents"][0]["env"].clone();
    assert_eq!(env(&export(false)), json!([{ "name": "DB_PASSWORD" }, { "name": "GITHUB_TOKEN" }, { "name": "PRIVATE" }, { "name": "PUBLIC_FLAG" }]));
    assert_eq!(env(&export(true)), json!([{ "name": "DB_PASSWORD" }, { "name": "GITHUB_TOKEN" }, { "name": "PRIVATE" }, { "name": "PUBLIC_FLAG", "value": "1" }]));
    let auto = &export(true)["sections"]["automations"][0]["env"];
    assert_eq!(auto, &json!([{ "name": "GITHUB_TOKEN" }, { "name": "PUBLIC_FLAG", "value": "1" }]));
}

#[test]
fn text_with_a_credential_keeps_the_rest() {
    let doc = export(false);
    assert_eq!(doc["sections"]["quickCommands"][0]["text"], "export GITHUB_TOKEN=[redacted] && npm publish");
    // Not a classified setting: not written at all.
    assert!(doc["sections"]["settings"].get("notifyToken").is_none());
    assert_eq!(doc["sections"]["settings"]["fontSize"], 15);
    // Ordinary values are left alone.
    assert!(!is_secret("fontFamily", "\"JetBrains Mono\", monospace"));
    assert!(!is_secret("text", "Review the uncommitted changes and list problems."));
    assert!(!is_secret("keepAwake", "agents"));
    assert!(is_secret("DB_PASSWORD", DB_PW));
    assert!(is_secret("apiKey", "abcdef123456"));
}

#[test]
fn imported_automations_are_off_and_carry_no_approval_or_history() {
    let a = &export(true)["sections"]["automations"][0];
    assert_eq!(a["enabled"], false);
    assert_eq!(a["shell"], json!({ "program": "python", "argv": ["scripts/x.py", "{file}"] }));
    for k in ["approval", "command_hash", "command_approved", "last_run", "last_session", "last_error", "next_run", "cap_day", "cap_count"] {
        assert!(a.get(k).is_none(), "{k} kept");
    }
    // A file that says otherwise changes nothing.
    let file = json!({ "kind": KIND, "version": 1, "sections": { "automations": [{ "id": "x", "name": "n", "cwd": "C:/p", "enabled": true, "action": "shell", "shell": { "program": "bash", "argv": [], "approved_hash": APPROVED }, "approval": APPROVED }] } });
    let read = read_doc(&serde_json::to_vec(&file).unwrap()).unwrap();
    let a = &read["doc"]["sections"]["automations"][0];
    assert_eq!(a["enabled"], false);
    assert!(!read.to_string().contains(APPROVED));
}

#[test]
fn hosts_lose_credentials_and_what_this_computer_recorded() {
    let h = &export(true)["sections"]["sshHosts"][0];
    assert_eq!(h["alias"], "box");
    assert_eq!(h["identityFile"], "~/.ssh/id_ed25519");
    assert_eq!(h["env"], json!({ "LANG": "C.UTF-8" }));
    assert_eq!(h["notes"], "token [redacted]");
    assert!(h.get("os").is_none() && h.get("lastConnected").is_none());
}

#[test]
fn reading_an_export_gives_the_same_sections() {
    for with_values in [false, true] {
        let doc = export(with_values);
        let read = read_doc(&serde_json::to_vec(&doc).unwrap()).unwrap();
        assert_eq!(read["doc"]["sections"], doc["sections"], "values {with_values}");
        assert_eq!(read["skipped"], json!({}));
        assert_eq!(read["ignored"], json!([]));
    }
}

#[test]
fn unusable_items_unknown_fields_and_sections_are_left_out() {
    let file = json!({
        "kind": KIND, "version": 1, "exportedAt": "x", "appVersion": "0.0.1", "comment": "hi",
        "sections": {
            "quickCommands": [{ "id": "a", "label": "L", "text": "T", "evil": "x" }, { "label": "no text" }, 7],
            "layouts": [{ "id": "l", "name": "bad", "template": { "type": "split", "dir": "diagonal", "children": [] } }],
            "plugins": [{ "id": "../../etc", "name": "x" }, { "id": "ok-plugin" }],
            "sshHosts": [{ "id": "h", "hostname": "-oProxyCommand=calc", "label": "bad" }, { "id": "h2", "hostname": "good.example", "port": 22 }],
            "themes": [],
        }
    });
    let read = read_doc(&serde_json::to_vec(&file).unwrap()).unwrap();
    let s = &read["doc"]["sections"];
    assert_eq!(s["quickCommands"], json!([{ "id": "a", "label": "L", "text": "T", "action": "terminal", "enter": true, "projectId": null }]));
    assert_eq!(s["layouts"], json!([]));
    assert_eq!(s["plugins"], json!([{ "id": "ok-plugin", "name": "ok-plugin" }]));
    assert_eq!(s["sshHosts"].as_array().unwrap().len(), 1);
    assert_eq!(read["skipped"], json!({ "quickCommands": 2, "layouts": 1, "plugins": 1, "sshHosts": 1 }));
    assert_eq!(read["ignored"], json!(["themes", "comment"]));
}

#[test]
fn wrong_files_are_refused() {
    let doc = |v: Value| read_doc(&serde_json::to_vec(&v).unwrap());
    assert!(read_doc(b"not json").unwrap_err().contains("not a Kelpie settings file"));
    assert!(doc(json!([1, 2])).is_err());
    assert!(doc(json!({ "kind": "other", "version": 1, "sections": {} })).unwrap_err().contains("not a Kelpie settings file"));
    assert!(doc(json!({ "kind": KIND, "version": 2, "sections": {} })).unwrap_err().contains("newer Kelpie"));
    assert!(doc(json!({ "kind": KIND, "version": "1", "sections": {} })).unwrap_err().contains("unknown settings file format"));
    assert!(doc(json!({ "kind": KIND, "version": 1 })).unwrap_err().contains("no sections"));
    assert!(doc(json!({ "kind": KIND, "version": 1, "sections": { "layouts": {} } })).unwrap_err().contains("not a list"));
    assert!(doc(json!({ "kind": KIND, "version": 1, "sections": { "settings": [] } })).unwrap_err().contains("not an object"));
}

#[test]
fn an_oversized_file_is_refused_unread() {
    let dir = temp_dir("big");
    let path = dir.join("big.json");
    let mut big = format!("{{\"kind\":\"{KIND}\",\"version\":1,\"sections\":{{\"settings\":{{\"pad\":\"");
    big.push_str(&"x".repeat(MAX_FILE as usize));
    big.push_str("\"}}}");
    std::fs::write(&path, &big).unwrap();
    assert!(read_capped(&path).unwrap_err().contains("larger than 2 MB"));
    assert!(read_doc(big.as_bytes()).unwrap_err().contains("larger than 2 MB"));
    assert!(read_capped(&dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_export_replaces_the_target_whole() {
    let dir = temp_dir("atomic");
    let path = dir.join("cfg.json");
    std::fs::write(&path, "old").unwrap();
    write_file_atomic(&path, b"{\"new\":1}").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"new\":1}");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
    assert!(write_file_atomic(&dir, b"x").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn backups_hold_everything_and_only_the_newest_are_kept() {
    let dir = temp_dir("backup");
    let hosts = dir.join("hosts.json");
    std::fs::write(&hosts, "{\"version\":1,\"hosts\":[]}").unwrap();
    let root = dir.join("backups");
    let first = backup_to(&root, &hosts, &json!({ "version": 3 }), &json!([{ "id": "a" }])).unwrap();
    assert_eq!(std::fs::read_to_string(first.join("ssh-hosts.json")).unwrap(), "{\"version\":1,\"hosts\":[]}");
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(first.join("app-state.json")).unwrap()).unwrap(), json!({ "version": 3 }));
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(first.join("automations.json")).unwrap()).unwrap(), json!([{ "id": "a" }]));
    // No hosts file yet: nothing to copy, not an error.
    let none = backup_to(&root, &dir.join("missing.json"), &json!({}), &json!([])).unwrap();
    assert!(!none.join("ssh-hosts.json").exists());
    for ms in 1..=8u32 {
        std::fs::create_dir_all(root.join(format!("config-{ms}"))).unwrap();
    }
    prune_backups(&root);
    let mut left: Vec<String> = std::fs::read_dir(&root).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left.len(), KEPT_BACKUPS);
    assert!(left.contains(&first.file_name().unwrap().to_string_lossy().into_owned()), "{left:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn secret_sounding_names_never_carry_a_value() {
    for name in ["DB_PASSWORD", "SMTP_PASS", "OPENAI_KEY", "API_KEY", "apiKey", "GH_AUTH", "oauth", "MY_SECRET", "GITHUB_TOKEN", "AWS_CREDENTIALS", "SESSION_COOKIE", "MY_SESSION", "--api-key", "MYSQL_PWD"] {
        assert!(secret_name(name), "{name}");
    }
    for name in ["PUBLIC_FLAG", "LANG", "PATH", "NODE_OPTIONS", "--model", "--permission-mode", "AUTHOR_NAME", "fontSize", "resumeSessions"] {
        assert!(!secret_name(name), "{name}");
    }
    let env = json!({ "DB_PASSWORD": "p@ss:w0rd!", "DB_PASS2": "ab;cdefgh", "GITHUB_TOKEN": "my token value", "SMTP_PASS": "plain", "OPENAI_KEY": "k", "GH_AUTH": "a", "SESSION_COOKIE": "c", "PUBLIC_FLAG": "1" });
    let entries = env_entries(Some(&env), true);
    let with_value: Vec<&str> = entries.as_array().unwrap().iter().filter(|e| e.get("value").is_some()).map(|e| e["name"].as_str().unwrap()).collect();
    assert_eq!(with_value, ["PUBLIC_FLAG"]);
    assert_eq!(entries.as_array().unwrap().len(), 8, "every name is kept");
}

#[test]
fn arguments_lose_the_value_of_a_secret_flag() {
    let args = |a: &[&str]| mask_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(args(&["--api-key", "c0ffee1234", "--model", "opus"]), ["--api-key", MASK, "--model", "opus"]);
    assert_eq!(args(&["--token", "abc"]), ["--token", MASK]);
    assert_eq!(args(&["--api-key=myrealkey123"]), [format!("--api-key={MASK}")]);
    assert_eq!(args(&["--password=hunter"]), [format!("--password={MASK}")]);
    assert_eq!(args(&["-c", "api_key=xyz", "-c", "model=o3"]), ["-c", &format!("api_key={MASK}"), "-c", "model=o3"]);
    assert_eq!(args(&["--api-key", "--verbose"]), ["--api-key", "--verbose"], "a flag is never taken for the value");
    assert_eq!(args(&["--settings", &format!("{{\"k\":\"{GH}\"}}")]), ["--settings", "{\"k\":\"[redacted]\"}"]);
    assert_eq!(args(&["--model", "opus", "--permission-mode", "plan"]), ["--model", "opus", "--permission-mode", "plan"]);
}

#[test]
fn credentials_in_command_text_are_masked() {
    assert_eq!(mask_text("mysql -u root -phunter2 shop"), format!("mysql -u root -p{MASK} shop"));
    assert_eq!(mask_text("curl -u admin:hunter2 https://x.example"), format!("curl -u admin:{MASK} https://x.example"));
    assert_eq!(mask_text("DB_PASSWORD=p@ss:w0rd! npm start"), format!("DB_PASSWORD={MASK} npm start"));
    assert_eq!(mask_text("deploy --token abc123 --env prod"), format!("deploy --token {MASK} --env prod"));
    assert_eq!(mask_text("tool --api-key=\"my key\""), format!("tool --api-key=\"{MASK}\""));
    for plain in ["npm test -- --all", "git push -u origin main", "Review the diff and list problems.", "cargo build -p kelpie"] {
        assert_eq!(mask_text(plain), plain);
    }
}

#[test]
fn every_seeded_case_stays_out_of_the_file() {
    let raw = json!({
        "settings": { "shellArgs": ["pwsh", "-Command", "connect --token s3cr3tshell"] },
        "quickCommands": [
            { "id": "a", "label": "DB", "text": "mysql -u root -phunter2 shop" },
            { "id": "b", "label": "Curl", "text": "curl -u admin:hunter2 https://x.example" },
        ],
        "agents": [{ "kind": "claude", "args": ["--api-key", "c0ffee1234", "--token=tok9876", "-c", "api_key=xyzxyz"], "env": { "DB_PASSWORD": "p@ss:w0rd!", "SMTP_PASS": "smtppass1", "OPENAI_KEY": "openaikey1", "GH_AUTH": "ghauth1" } }],
        "automations": [{ "name": "n", "cwd": "C:/p", "args": ["--password", "autopass1"], "action": "shell", "shell": { "program": "tool", "argv": ["--api-key=argvkey1"] } }],
    });
    let hosts: Vault = serde_json::from_value(json!({ "version": 1, "hosts": [{ "id": "h-1!", "hostname": "h.example", "startup": "mysql -phostpass1", "env": { "SMTP_PASS": "hostenv1" } }], "forwards": [] })).unwrap();
    let wanted: Vec<String> = ["settings", "quickCommands", "agents", "automations", "sshHosts"].iter().map(|s| s.to_string()).collect();
    let doc = export_doc(raw.as_object().unwrap(), &wanted, true, Some(&hosts), None, "t").unwrap();
    let text = doc.to_string();
    for secret in ["s3cr3tshell", "hunter2", "c0ffee1234", "tok9876", "xyzxyz", "p@ss:w0rd!", "smtppass1", "openaikey1", "ghauth1", "autopass1", "argvkey1", "hostpass1", "hostenv1"] {
        assert!(!text.contains(secret), "{secret} leaked: {text}");
    }
    let id = doc["sections"]["sshHosts"][0]["id"].as_str().unwrap();
    assert!(!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()), "a host id passwords cannot be saved under is replaced: {id}");
}

#[test]
fn settings_carry_only_classified_keys() {
    let lists: Value = serde_json::from_str(include_str!("../../../src/lib/settings-transfer.json")).unwrap();
    let excluded: Vec<&str> = lists["excluded"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    let mut raw = Map::new();
    for k in SETTINGS_EXPORTED.iter().map(String::as_str).chain(excluded.iter().copied()).chain(["madeUp"]) {
        raw.insert(k.to_string(), json!(1));
    }
    let kept = settings(&Value::Object(raw)).unwrap();
    let kept: Vec<&String> = kept.as_object().unwrap().keys().collect();
    assert_eq!(kept.len(), SETTINGS_EXPORTED.len());
    assert!(excluded.iter().all(|k| !SETTINGS_EXPORTED.iter().any(|e| e == k)));
}

#[test]
fn a_path_that_is_not_a_regular_file_is_not_opened() {
    let dir = temp_dir("notfile");
    assert_eq!(read_capped(&dir).unwrap_err(), "not a file");
    assert!(read_capped(&dir.join("missing.json")).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
