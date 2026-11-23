//! A phone reading a project's pull request (`pr_get`) through its own connection (`Conn::handle`): where the folder comes from, what is refused
//! and why, and that nothing in the answer names the folder.

use super::*;

fn folder(tag: &str) -> String {
    let dir = std::env::temp_dir().join(format!("kelpie-phone-pr-{tag}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.to_string_lossy().into_owned()
}

fn set_launch(d: &Arc<Daemon>, params: Value) {
    let (tx, _rx) = crate::session::client_channel();
    d.dispatch(1, &tx, &json!(1), "mobile.set_launch", params).unwrap();
}

#[tokio::test]
async fn a_phone_reads_a_pull_request_only_of_a_project_the_window_told_the_daemon_about() {
    let mut r = rig();
    // Nothing sent since the daemon started: the phone is told why, in words.
    let none = r.ask(Permission::Read, json!({"type": "pr_get", "project": "p1"})).await;
    assert_eq!((none["type"].as_str(), code(&none)), (Some("error"), Some("not_ready")), "{none}");
    assert!(none["message"].as_str().unwrap().contains("window"));
    let path = folder("catalogue");
    set_launch(&r.daemon, json!({"projects": [{"id": "p1", "name": "Shop", "path": path}], "agents": []}));
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get", "project": "other"})).await), Some("no_project"));
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get"})).await), Some("bad_request"));
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get", "project": ""})).await), Some("bad_request"));
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get", "project": "x".repeat(launch::MAX_PROJECT_ID + 1)})).await), Some("bad_request"));
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_project_whose_folder_is_gone_is_a_message_and_the_folder_is_never_said() {
    let mut r = rig();
    let path = folder("gone");
    let missing = format!("{path}-missing");
    set_launch(&r.daemon, json!({"projects": [{"id": "p1", "name": "Shop", "path": missing}], "agents": []}));
    let reply = r.ask(Permission::Read, json!({"type": "pr_get", "project": "p1"})).await;
    assert_eq!((reply["type"].as_str(), code(&reply)), (Some("error"), Some("pr_unavailable")), "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("not on this computer"), "{reply}");
    assert!(!reply.to_string().contains(&missing) && !reply.to_string().contains(&path), "{reply}");
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn reading_pull_requests_over_and_over_is_held_back() {
    let mut r = rig();
    for _ in 0..4 {
        assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get", "project": "p1"})).await), Some("not_ready"));
    }
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get", "project": "p1"})).await), Some("rate_limited"));
}
