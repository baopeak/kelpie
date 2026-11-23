//! A phone reading a project's pull request (`pr_get`) and merging it (`pr_challenge`, `pr_merge`) through its own connection (`Conn::handle`): where
//! the folder comes from, what is refused and why, that nothing in an answer names the folder, and, with a software authenticator for the passkey and
//! `git` and `gh` answered by the test (`pulls::fake`), that a merge is bound to what the daemon read, done once, and never done twice.

use super::lock::{enrolled, proof};
use super::*;
use crate::mobile::webauthn::soft::Authenticator;
use crate::pulls::fake::{self, Answer};

const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

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
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_project_id_longer_than_any_the_window_sends_is_a_bad_request() {
    // A connection of its own: a phone's reads are limited, and the test above already made as many as a moment allows.
    let mut r = rig();
    assert_eq!(code(&r.ask(Permission::Read, json!({"type": "pr_get", "project": "x".repeat(launch::MAX_PROJECT_ID + 1)})).await), Some("bad_request"));
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

fn challenge_request(method: &str) -> Value {
    json!({"type": "pr_challenge", "project": "p1", "method": method})
}

fn merge_request(nonce: &str, recheck: Value, cid: &str) -> Value {
    json!({"type": "pr_merge", "project": "p1", "repo": "o/n", "number": 12, "head": HEAD, "method": "squash", "nonce": nonce, "recheck": recheck, "cid": cid})
}

#[tokio::test]
async fn a_merge_needs_control_and_then_a_passkey_this_connection_can_give() {
    let mut r = rig();
    let merge = merge_request("n1", json!(null), "c1");
    // Control is what a pane's takeover needs: a device whose record is below it is told so, and nothing is asked of GitHub.
    for granted in [Permission::Read, Permission::Write] {
        for req in [&challenge_request("squash"), &merge] {
            let reply = r.ask(granted, req.clone()).await;
            assert_eq!((reply["type"].as_str(), code(&reply)), (Some("error"), Some("no_control")), "{granted:?}: {reply}");
        }
    }
    // With control but over a connection that has no passkey (the rig's is a plain local one), a merge has nothing to be confirmed with.
    for req in [&challenge_request("squash"), &merge] {
        let reply = r.ask(Permission::Control, req.clone()).await;
        assert_eq!(code(&reply), Some("no_passkey"), "{reply}");
    }
    assert!(r.typed.writes().is_empty());
}

#[tokio::test]
async fn a_merge_request_is_checked_before_any_challenge_is_kept() {
    let mut r = rig();
    let merge = merge_request("n1", json!(null), "c1");
    for (key, bad) in [("repo", json!("o")), ("number", json!(0)), ("head", json!("main")), ("method", json!("--admin"))] {
        let mut req = merge.clone();
        req[key] = bad.clone();
        // Each with a cid of its own: a repeat of one would be answered with the first's answer.
        req["cid"] = json!(format!("c-{key}"));
        let reply = r.ask(Permission::Control, req).await;
        assert_eq!(code(&reply), Some("bad_request"), "{key}: {reply}");
    }
    // The method is all a challenge takes from the phone.
    for bad in [json!("--admin"), json!("Squash"), json!(null)] {
        let mut req = challenge_request("squash");
        req["method"] = bad.clone();
        assert_eq!(code(&r.ask(Permission::Control, req).await), Some("bad_request"), "{bad}");
    }
    // A request that is whole but has no passkey behind it keeps no challenge.
    assert_eq!(code(&r.ask(Permission::Control, challenge_request("squash")).await), Some("no_passkey"));
    assert_eq!(r.daemon.mobile.pr.kept(), 0, "nothing was kept");
}

// ---------- with a passkey, and git and gh answered by the test ----------

/// What the fake `git` and `gh` say about one pull request, number 12 of `o/n` on the branch `feature`, and what has been done to it.
struct Github {
    merged: AtomicBool,
    merges: AtomicUsize,
    /// A merge queue: `gh pr merge` succeeds and the pull request stays open.
    queue: bool,
    /// What `gh pr view` takes before it answers.
    view_takes: Duration,
    /// What `gh pr merge` takes before it answers.
    merge_takes: Duration,
    /// What `gh pr checks --required` says: None is "no required checks reported".
    checks: Option<Answer>,
}

impl Github {
    fn new() -> Github {
        Github { merged: AtomicBool::new(false), merges: AtomicUsize::new(0), queue: false, view_takes: Duration::ZERO, merge_takes: Duration::ZERO, checks: None }
    }

    fn merges(&self) -> usize {
        self.merges.load(Ordering::SeqCst)
    }

    fn pull(&self) -> String {
        let state = if self.merged.load(Ordering::SeqCst) { "MERGED" } else { "OPEN" };
        json!({
            "number": 12, "title": "Fix\u{202e} the thing", "url": "https://github.com/o/n/pull/12", "state": state, "isDraft": false, "baseRefName": "master",
            "headRefOid": HEAD, "mergeable": "MERGEABLE", "mergeStateStatus": "CLEAN", "statusCheckRollup": [],
        })
        .to_string()
    }

    fn answer(&self, program: &str, args: &[String]) -> Answer {
        let at = |i: usize| args.get(i).map(String::as_str).unwrap_or_default();
        match (program, at(0), at(1)) {
            ("git", _, _) if args.iter().any(|a| a == "get-url") => Answer::out("https://github.com/o/n.git\n"),
            ("git", _, _) if args.iter().any(|a| a == "--abbrev-ref") => Answer::out("feature\n"),
            ("gh", "pr", "view") => {
                std::thread::sleep(self.view_takes);
                Answer::out(self.pull())
            }
            ("gh", "pr", "checks") => match &self.checks {
                Some(said) => Answer { ok: said.ok, stdout: said.stdout.clone(), stderr: said.stderr.clone() },
                None => Answer::failed("no required checks reported on the 'feature' branch"),
            },
            ("gh", "pr", "merge") => {
                self.merges.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(self.merge_takes);
                self.merged.store(!self.queue, Ordering::SeqCst);
                Answer::out("")
            }
            _ => Answer::failed(format!("unexpected {program} {args:?}")),
        }
    }

    /// Answers `git` in the project's folder and `gh` in the daemon's data folder, where the daemon runs it.
    fn install(self, r: &Rig, project: &str) -> Arc<Github> {
        let github = Arc::new(self);
        for cwd in [std::path::PathBuf::from(project), r.daemon.data_dir.clone()] {
            let github = github.clone();
            fake::answer_in(&cwd, move |program, args| github.answer(program, args));
        }
        github
    }
}

/// The rig's phone with a passkey (enrolled, unlocked) and control, and a project `p1` whose folder is `path`.
async fn merging_phone(path: &str) -> (Rig, Authenticator) {
    let (r, a) = enrolled().await;
    let key = r.daemon.mobile.mac_key(true).unwrap().unwrap();
    {
        let mut c = r.daemon.mobile.config.lock();
        let phone = c.devices.iter_mut().find(|d| d.id == "phone").unwrap();
        phone.permission = Permission::Control;
        store::seal(phone, &key);
    }
    set_launch(&r.daemon, json!({"projects": [{"id": "p1", "name": "Shop", "path": path}], "agents": []}));
    (r, a)
}

/// What the daemon says about the merges it was asked to do, from its audit log: the summaries, oldest first.
fn merged_lines(r: &Rig) -> Vec<String> {
    let log = r.daemon.mobile.audit_of(50, Some("pr_merge"));
    let mut lines: Vec<String> = log["entries"].as_array().unwrap().iter().map(|e| e["summary"].as_str().unwrap().to_string()).collect();
    lines.reverse();
    lines
}

/// Asks for a merge's challenge and signs it: (the challenge's reply, a merge request with its assertion).
async fn challenged(r: &mut Rig, a: &mut Authenticator, method: &str, cid: &str) -> (Value, Value) {
    let challenge = r.ask(Permission::Control, challenge_request(method)).await;
    assert_eq!(challenge["result"], "ok", "{challenge}");
    let (nonce, command, expiry) = (challenge["nonce"].as_str().unwrap(), challenge["command"].as_str().unwrap(), challenge["expiry"].as_u64().unwrap());
    let signed = proof(a, &crate::pulls::challenge_request(nonce), command, expiry);
    let mut merge = merge_request(nonce, signed, cid);
    merge["method"] = json!(method);
    (challenge.clone(), merge)
}

#[tokio::test]
async fn the_daemon_builds_what_the_passkey_signs_from_what_it_read_and_merges_that_once() {
    let path = folder("merge");
    let (mut r, mut a) = merging_phone(&path).await;
    let github = Github::new().install(&r, &path);
    let (challenge, merge) = challenged(&mut r, &mut a, "squash", "c1").await;
    // What the phone is shown comes from the daemon's own reading: the repository from `origin`, the pull request of the branch, its head and title.
    assert_eq!(challenge["command"], format!("merge github.com o/n #12 {HEAD} squash"));
    assert_eq!((challenge["repo"].as_str(), challenge["number"].as_u64(), challenge["head"].as_str(), challenge["method"].as_str()), (Some("o/n"), Some(12), Some(HEAD), Some("squash")));
    assert_eq!(challenge["title"], "Fix the thing", "no direction-changing characters");
    assert!(!challenge.to_string().contains(&path), "{challenge}");
    assert_eq!(github.merges(), 0, "asking for a challenge merges nothing");

    let merged = r.ask(Permission::Control, merge.clone()).await;
    assert_eq!((merged["type"].as_str(), merged["result"].as_str(), merged["number"].as_u64(), merged["req"].clone()), (Some("ack"), Some("merged"), Some(12), json!(1)), "{merged}");
    assert_eq!(github.merges(), 1);
    // The same request again (a retry after a lost answer) gets the first answer back and merges nothing.
    let again = r.ask(Permission::Control, merge.clone()).await;
    assert_eq!((again["result"].as_str(), again["dup"].clone()), (Some("merged"), json!(true)), "{again}");
    // A new request that uses the same challenge is a different merge attempt, and the challenge is gone.
    let mut other = merge.clone();
    other["cid"] = json!("c2");
    assert_eq!(code(&r.ask(Permission::Control, other).await), Some("challenge"));
    assert_eq!(github.merges(), 1);
    let lines = merged_lines(&r);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0], format!("o/n #12 head {HEAD} (squash) by Phone: merged"));
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_merge_that_github_only_queued_is_not_called_merged() {
    let path = folder("queue");
    let (mut r, mut a) = merging_phone(&path).await;
    let github = Github { queue: true, ..Github::new() }.install(&r, &path);
    let (_, merge) = challenged(&mut r, &mut a, "squash", "c1").await;
    let reply = r.ask(Permission::Control, merge).await;
    assert_eq!((reply["type"].as_str(), reply["result"].as_str(), reply["state"].as_str()), (Some("ack"), Some("pending"), Some("open")), "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("not merged yet"), "{reply}");
    assert_eq!(github.merges(), 1);
    assert!(merged_lines(&r)[0].ends_with("accepted, not merged yet (open)"), "{:?}", merged_lines(&r));
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_challenge_is_for_one_device_one_line_one_time_and_one_use() {
    let path = folder("bound");
    let (mut r, mut a) = merging_phone(&path).await;
    let github = Github::new().install(&r, &path);
    let command = format!("merge github.com o/n #12 {HEAD} squash");

    // Another device's challenge, even signed, is not this phone's.
    let (theirs, expiry) = r.daemon.mobile.pr.ask("tablet", command.clone(), now_millis()).unwrap();
    let signed = proof(&mut a, &crate::pulls::challenge_request(&theirs), &command, expiry);
    assert_eq!(code(&r.ask(Permission::Control, merge_request(&theirs, signed, "d1")).await), Some("challenge"), "another device's");

    // A request that describes another merge than the one asked about (here another method) is refused, and the challenge is used up by it.
    let (_, mut other_method) = challenged(&mut r, &mut a, "squash", "d2").await;
    let straight = other_method.clone();
    other_method["method"] = json!("rebase");
    assert_eq!(code(&r.ask(Permission::Control, other_method).await), Some("challenge"), "another line");
    let mut again = straight;
    again["cid"] = json!("d3");
    assert_eq!(code(&r.ask(Permission::Control, again).await), Some("challenge"), "used up");

    // An assertion over a time other than the challenge's own.
    let (challenge, mut late) = challenged(&mut r, &mut a, "squash", "d4").await;
    let (nonce, expiry) = (challenge["nonce"].as_str().unwrap(), challenge["expiry"].as_u64().unwrap());
    late["recheck"] = proof(&mut a, &crate::pulls::challenge_request(nonce), &command, expiry + 30_000);
    assert_eq!(code(&r.ask(Permission::Control, late).await), Some("recheck"), "another time");

    // An assertion over something else (here a dangerous approval's request id) is not a merge's.
    let (challenge, mut borrowed) = challenged(&mut r, &mut a, "squash", "d5").await;
    let expiry = challenge["expiry"].as_u64().unwrap();
    borrowed["recheck"] = proof(&mut a, "r1", &command, expiry);
    assert_eq!(code(&r.ask(Permission::Control, borrowed).await), Some("lock_refused"), "another request id");
    assert_eq!(github.merges(), 0, "nothing of this reached gh");
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn a_merge_without_an_assertion_is_refused_and_one_that_lost_its_standing_is_refused_and_noted() {
    let path = folder("standing");
    let (mut r, mut a) = merging_phone(&path).await;
    let github = Github::new().install(&r, &path);
    let (_, mut bare) = challenged(&mut r, &mut a, "squash", "e1").await;
    bare.as_object_mut().unwrap().remove("recheck");
    assert_eq!(code(&r.ask(Permission::Control, bare).await), Some("recheck"));

    // The passkey is good, and then phone access is turned off before the merge starts: refused, and the audit log says so.
    let (_, merge) = challenged(&mut r, &mut a, "squash", "e2").await;
    r.daemon.mobile.config.lock().enabled = false;
    assert_eq!(code(&r.ask(Permission::Control, merge).await), Some("mobile_off"));
    assert_eq!(github.merges(), 0);
    let lines = merged_lines(&r);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("by Phone: refused: Phone access was turned off"), "{lines:?}");
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn required_checks_that_cannot_be_read_stop_the_challenge_and_the_line() {
    let path = folder("checks");
    let (mut r, _a) = merging_phone(&path).await;
    Github { checks: Some(Answer::failed("HTTP 502: Bad Gateway")), ..Github::new() }.install(&r, &path);
    let reply = r.ask(Permission::Control, challenge_request("squash")).await;
    assert_eq!(code(&reply), Some("pr_unavailable"), "{reply}");
    assert!(reply["message"].as_str().unwrap().contains("required checks could not be read"), "{reply}");
    assert_eq!(r.daemon.mobile.pr.kept(), 0);
    let line = r.ask(Permission::Read, json!({"type": "pr_get", "project": "p1"})).await;
    assert_eq!((line["pr"]["line"]["can"].as_bool(), line["pr"]["line"]["reason"].as_str()), (Some(false), Some("unknown")), "{line}");
    let _ = std::fs::remove_dir_all(path);
}

#[tokio::test]
async fn the_connection_goes_on_while_gh_is_asked_and_a_repeated_merge_waits_for_the_first() {
    let path = folder("slow");
    let (mut r, mut a) = merging_phone(&path).await;
    let github = Github { view_takes: Duration::from_millis(400), merge_takes: Duration::from_millis(2_500), ..Github::new() }.install(&r, &path);

    // A read that waits for gh leaves the connection free at once, and its answer comes later with the request's own tag.
    let started = Instant::now();
    let direct = r.conn.handle(Permission::Read, &json!({"type": "pr_get", "project": "p1", "req": 7})).await;
    assert!(direct.is_none() && started.elapsed() < Duration::from_millis(300), "answered on the loop after {:?}: {direct:?}", started.elapsed());
    let later: Value = serde_json::from_str(&r.ctl.recv().await.unwrap()).unwrap();
    assert_eq!((later["req"].clone(), later["pr"]["number"].as_u64()), (json!(7), Some(12)), "{later}");

    // The same goes for a merge, and a repeat of it while it runs does not start another: it is told the first is still under way.
    let (_, merge) = challenged(&mut r, &mut a, "squash", "f1").await;
    let mut first = merge.clone();
    first["req"] = json!(8);
    let direct = r.conn.handle(Permission::Control, &first).await;
    assert!(direct.is_none(), "{direct:?}");
    let mut repeat = merge.clone();
    repeat["req"] = json!(9);
    let during = r.conn.handle(Permission::Control, &repeat).await.expect("an answer");
    assert_eq!((code(&during), during["req"].clone()), (Some("in_progress"), json!(9)), "{during}");
    assert!(github.merges() <= 1, "one merge is under way, not two");
    // The first one's answer is still delivered, and once it is, the repeat gets it back.
    let done: Value = serde_json::from_str(&r.ctl.recv().await.unwrap()).unwrap();
    assert_eq!((done["result"].as_str(), done["req"].clone()), (Some("merged"), json!(8)), "{done}");
    let after = r.conn.handle(Permission::Control, &repeat).await.expect("the first answer");
    assert_eq!((after["result"].as_str(), after["dup"].clone(), after["req"].clone()), (Some("merged"), json!(true), json!(9)), "{after}");
    assert_eq!(github.merges(), 1);
    assert_eq!(merged_lines(&r).len(), 1);
    let _ = std::fs::remove_dir_all(path);
}
