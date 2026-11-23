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

fn merge_requests() -> (Value, Value) {
    let bound = json!({"project": "p1", "repo": "o/n", "number": 12, "head": "0123456789abcdef0123456789abcdef01234567", "method": "squash"});
    let mut challenge = bound.clone();
    challenge["type"] = json!("pr_challenge");
    let mut merge = bound;
    merge["type"] = json!("pr_merge");
    merge["nonce"] = json!("n1");
    (challenge, merge)
}

#[tokio::test]
async fn a_merge_needs_control_and_then_a_passkey_this_connection_can_give() {
    let mut r = rig();
    let (challenge, merge) = merge_requests();
    // Control is what a pane's takeover needs: a device whose record is below it is told so, and nothing is asked of GitHub.
    for granted in [Permission::Read, Permission::Write] {
        for req in [&challenge, &merge] {
            let reply = r.ask(granted, req.clone()).await;
            assert_eq!((reply["type"].as_str(), code(&reply)), (Some("error"), Some("no_control")), "{granted:?}: {reply}");
        }
    }
    // With control but over a connection that has no passkey (the rig's is a plain local one), a merge has nothing to be confirmed with.
    for req in [&challenge, &merge] {
        let reply = r.ask(Permission::Control, req.clone()).await;
        assert_eq!(code(&reply), Some("no_passkey"), "{reply}");
    }
    assert!(r.typed.writes().is_empty());
}

#[tokio::test]
async fn a_merge_request_is_checked_before_any_challenge_is_kept() {
    let mut r = rig();
    let (challenge, merge) = merge_requests();
    for (key, bad) in [("repo", json!("o")), ("number", json!(0)), ("head", json!("main")), ("method", json!("--admin"))] {
        for request in [&challenge, &merge] {
            let mut req = request.clone();
            req[key] = bad.clone();
            let reply = r.ask(Permission::Control, req).await;
            assert_eq!(code(&reply), Some("bad_request"), "{key}: {reply}");
        }
    }
    // A request that is whole but has no passkey behind it keeps no challenge.
    assert_eq!(code(&r.ask(Permission::Control, challenge).await), Some("no_passkey"));
    assert!(r.daemon.mobile.pr.asks.lock().is_empty(), "nothing was kept");
}
