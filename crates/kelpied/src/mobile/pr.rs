//! What a phone asks about a project's pull request: `pr_get` reads it (`pulls`), and `pr_challenge` with `pr_merge` merge it.
//!
//! A merge is the one thing here that changes anything outside this computer, so it is bound end to end. The phone asks for a challenge for one
//! merge: this repository, pull request, head commit and method. The daemon keeps that in a table of its own (not the app lock's, whose
//! challenges unlock a phone), single use and good for a minute, and answers with the line the passkey will be asked to sign
//! (`pulls::bound_command`). The phone signs it, with a request id that only a merge's challenge can have (`pulls::challenge_request`), and sends
//! `pr_merge`. The daemon then rebuilds the line from what the merge request says and refuses unless it is the one it kept, checks the assertion
//! with the same check a dangerous approval's passkey goes through (`Mobile::lock_recheck`), reads the pull request again, and merges only if
//! GitHub, the head commit and the required checks still allow it (`pulls::merge_refusal`). Every attempt that gets as far as the passkey is
//! audited, with what it was and what came of it.
//!
//! The folder of a project never goes to a phone, and neither does anything that names it.

use std::collections::HashMap;

use parking_lot::Mutex;
use serde_json::{json, Value};

use super::{launch, refuse, same, take_device_token, Conn, Gate, Mobile, Permission, Refusal, SendBucket};
use crate::pulls::{self, Bound};
use crate::session::now_millis;

/// Times one device may read a project's pull request: this many at once, then one every five seconds (each is one or two calls to GitHub).
const READ_BURST: f64 = 4.0;
const READ_RATE: f64 = 0.2;
/// How long a merge's challenge may wait for the person.
const ASK_TTL_MS: u64 = 60_000;
/// Challenges one device may have open at once; asking for more drops its oldest.
const ASKS_PER_DEVICE: usize = 4;

/// One merge's challenge: which device asked, the line its passkey will sign, and until when (by this computer's clock).
struct Ask {
    device: String,
    nonce: String,
    command: String,
    expiry: u64,
}

#[derive(Default)]
pub struct PrMemory {
    /// How fast each device has been reading pull requests (`allow_read`).
    reads: Mutex<HashMap<String, SendBucket>>,
    /// The merges' challenges that wait for a passkey. In memory only, and none outlives this daemon.
    asks: Mutex<Vec<Ask>>,
}

impl PrMemory {
    pub fn allow_read(&self, device: &str) -> bool {
        take_device_token(&self.reads, device, READ_RATE, READ_BURST)
    }

    /// A new challenge for `device` over `command`, as (nonce, expiry in ms). The device's oldest one goes when it has too many.
    pub fn ask(&self, device: &str, command: String, now: u64) -> (String, u64) {
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let expiry = now + ASK_TTL_MS;
        let mut asks = self.asks.lock();
        asks.retain(|a| a.expiry > now);
        while asks.iter().filter(|a| a.device == device).count() >= ASKS_PER_DEVICE {
            match asks.iter().position(|a| a.device == device) {
                Some(oldest) => {
                    asks.remove(oldest);
                }
                None => break,
            }
        }
        asks.push(Ask { device: device.to_string(), nonce: nonce.clone(), command, expiry });
        (nonce, expiry)
    }

    /// The challenge `nonce` of `device`, used up by asking for it: (the line, its expiry), or `None` when it is unknown, was used, is another
    /// device's or has expired.
    pub fn take(&self, device: &str, nonce: &str, now: u64) -> Option<(String, u64)> {
        let mut asks = self.asks.lock();
        asks.retain(|a| a.expiry > now);
        let at = asks.iter().position(|a| a.device == device && same(&a.nonce, nonce))?;
        let ask = asks.remove(at);
        Some((ask.command, ask.expiry))
    }
}

impl Mobile {
    /// The folder of project `id`, from the launch catalogue the window pushed with `mobile.set_launch`: (code, message) when there is none.
    /// The catalogue lives in memory only, so a daemon the window has not talked to since it started knows no project.
    pub fn project_folder(&self, id: &str) -> Result<String, (&'static str, String)> {
        match &*self.launch.lock() {
            Err(launch::Missing::NotSent) => Err(("not_ready", "Kelpie's window has not told the daemon about your projects since the daemon started: open Kelpie on the computer.".to_string())),
            Err(refused) => Err(refused.refusal()),
            Ok(catalogue) => catalogue
                .projects
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.path.clone())
                .ok_or(("no_project", "Kelpie's window has not told the daemon about this project: it is not on this computer, or it runs over SSH.".to_string())),
        }
    }
}

/// What a merge request binds, each part checked: the repository as `owner/name`, a pull request number, a head commit and a method.
fn bound_of(req: &Value) -> Result<Bound, Refusal> {
    let bad = |what: &str| refuse("bad_request", format!("missing or bad {what}"));
    let repo = req["repo"].as_str().filter(|r| pulls::valid_repo(r)).ok_or_else(|| bad("repo"))?;
    let number = req["number"].as_u64().filter(|n| *n > 0 && *n < (1 << 40)).ok_or_else(|| bad("number"))?;
    let head = req["head"].as_str().filter(|h| pulls::valid_head(h)).ok_or_else(|| bad("head"))?;
    let method = req["method"].as_str().and_then(pulls::merge_method).ok_or_else(|| bad("method"))?;
    Ok(Bound { repo: repo.to_string(), number, head: head.to_string(), method })
}

impl Conn {
    /// The folder of the project a request names.
    fn project_folder_of(&self, req: &Value) -> Result<String, Refusal> {
        let id = req["project"].as_str().filter(|p| !p.is_empty() && p.len() <= launch::MAX_PROJECT_ID).ok_or_else(|| refuse("bad_request", "missing project"))?;
        self.daemon.mobile.project_folder(id).map_err(|(code, message)| refuse(code, message))
    }

    /// `pr_get {project}`: the pull request of the project's branch and GitHub's word on merging it (`pulls::status`), or `{none: true}`.
    pub(super) async fn pr_get(&self, req: &Value) -> Result<Value, Refusal> {
        if !self.daemon.mobile.pr.allow_read(&self.device) {
            return Err(refuse("rate_limited", "Asking too often: wait a moment."));
        }
        let folder = self.project_folder_of(req)?;
        let neutral = self.daemon.data_dir.clone();
        let read = tokio::task::spawn_blocking(move || pulls::status(&folder, &neutral)).await.map_err(|_| refuse("not_kept", "That did not finish."))?;
        read.map(|pr| json!({"type": "ack", "result": "ok", "pr": pr})).map_err(|why| refuse("pr_unavailable", why))
    }

    /// A merge needs the passkey on this connection: one that cannot give it (a phone that never had a secure origin) cannot merge.
    fn merge_needs_passkey(&self) -> Result<(), Refusal> {
        if self.lock_path.passkey() {
            return Ok(());
        }
        Err(refuse("no_passkey", "Merging needs this phone's passkey, and this connection has none: pair the phone through Tailscale and set up the app lock."))
    }

    /// Whether this phone may still merge, read again after the waits of a merge: phone access on, still paired, control still its own, not locked.
    /// What `handle` checked before may have changed since.
    fn may_still_merge(&self) -> Result<(), Refusal> {
        let m = &self.daemon.mobile;
        if !m.is_on() {
            return Err(refuse("mobile_off", "Phone access was turned off on the computer."));
        }
        let Some(record) = m.permission(&self.device) else {
            return Err(refuse("unpaired", "This phone is no longer paired with the computer."));
        };
        let gate = m.lock_gate(&self.device, self.lock_path, self.resume.as_deref());
        if gate == Gate::Locked {
            return Err(refuse("locked", "Kelpie is locked on this phone."));
        }
        if gate != Gate::Open || record.min(self.cap) < Permission::Control || !m.control_now(&self.device, self.lock_path, gate, self.page_origin.is_some()) {
            return Err(refuse("no_control", "This phone may not take control now."));
        }
        Ok(())
    }

    /// `pr_challenge {project, repo, number, head, method}`: the challenge for one merge, answered `{nonce, expiry, command}`. The repository must be
    /// the project's own now (read from its folder), so that what the person was shown is what the passkey will cover.
    pub(super) async fn pr_challenge(&self, req: &Value) -> Result<Value, Refusal> {
        let bound = bound_of(req)?;
        self.merge_needs_passkey()?;
        let folder = self.project_folder_of(req)?;
        let repo = tokio::task::spawn_blocking(move || pulls::repo_of(&folder)).await.map_err(|_| refuse("not_kept", "That did not finish."))?.map_err(|why| refuse("pr_unavailable", why))?;
        if repo != bound.repo {
            return Err(refuse("repo_changed", "This project's repository is not the one that was shown: look again."));
        }
        let command = pulls::bound_command(&bound);
        let (nonce, expiry) = self.daemon.mobile.pr.ask(&self.device, command.clone(), now_millis());
        Ok(json!({"type": "ack", "result": "ok", "nonce": nonce, "expiry": expiry, "command": command}))
    }

    /// `pr_merge {project, repo, number, head, method, nonce, recheck}`: merges the pull request once the passkey has covered it. The challenge is
    /// used up by being asked for, whatever comes of the rest.
    pub(super) async fn pr_merge(&self, req: &Value) -> Result<Value, Refusal> {
        let bound = bound_of(req)?;
        self.merge_needs_passkey()?;
        let nonce = req["nonce"].as_str().filter(|n| !n.is_empty() && n.len() <= 64).ok_or_else(|| refuse("bad_request", "missing nonce"))?;
        let folder = self.project_folder_of(req)?;
        let command = pulls::bound_command(&bound);
        let Some((asked, expiry)) = self.daemon.mobile.pr.take(&self.device, nonce, now_millis()) else {
            return Err(refuse("challenge", "That took too long, or was already used: start again."));
        };
        if asked != command {
            return Err(refuse("challenge", "That is not the merge the passkey was asked about: start again."));
        }
        // The assertion must be over this challenge's own time too: the phone cannot stretch a minute into two.
        if req["recheck"]["expiry"].as_u64() != Some(expiry) {
            return Err(refuse("recheck", "Confirm this merge with your passkey."));
        }
        self.check_recheck(req, pulls::challenge_request(nonce), command).await?;
        self.may_still_merge()?;
        // The line goes in the audit log from the thread that merges, so that a phone that goes away meanwhile still leaves it.
        let (neutral, daemon, device, name, merged) = (self.daemon.data_dir.clone(), self.daemon.clone(), self.device.clone(), self.name.clone(), bound.clone());
        let done = tokio::task::spawn_blocking(move || {
            let result = pulls::merge_now(&folder, &neutral, &merged);
            daemon.mobile.audit(&device, "pr_merge", &pulls::merge_summary(&name, &merged, &result));
            result
        })
        .await;
        match done {
            Ok(Ok(())) => Ok(json!({"type": "ack", "result": "merged", "repo": bound.repo, "number": bound.number})),
            Ok(Err(why)) => Err(refuse("merge_refused", why)),
            Err(_) => Err(refuse("not_kept", "The merge did not finish: look at the pull request on GitHub to see where it stands.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_challenge_is_used_once_by_the_device_that_asked_and_only_for_a_minute() {
        let m = PrMemory::default();
        let (nonce, expiry) = m.ask("phone", "merge github.com o/n #1 abc squash".into(), 1_000);
        assert_eq!(expiry, 1_000 + ASK_TTL_MS);
        assert_eq!(nonce.len(), 32);
        // Another device cannot use it, and trying does not use it up.
        assert_eq!(m.take("other", &nonce, 2_000), None);
        assert_eq!(m.take("phone", "not-the-nonce", 2_000), None);
        assert_eq!(m.take("phone", &nonce, 2_000), Some(("merge github.com o/n #1 abc squash".to_string(), expiry)));
        assert_eq!(m.take("phone", &nonce, 2_000), None, "used up");
        let (late, expiry) = m.ask("phone", "x".into(), 1_000);
        assert_eq!(m.take("phone", &late, expiry), None, "it ends when its minute does");
        let (kept, expiry) = m.ask("phone", "y".into(), 1_000);
        assert!(m.take("phone", &kept, expiry - 1).is_some());
        // Two challenges are two nonces.
        assert_ne!(m.ask("phone", "z".into(), 1_000).0, m.ask("phone", "z".into(), 1_000).0);
    }

    #[test]
    fn a_device_holds_a_few_challenges_and_asking_for_more_drops_its_oldest_not_another_devices() {
        let m = PrMemory::default();
        let theirs = m.ask("other", "theirs".into(), 1_000).0;
        let mine: Vec<String> = (0..ASKS_PER_DEVICE + 2).map(|i| m.ask("phone", format!("c{i}"), 1_000).0).collect();
        assert_eq!(m.take("phone", &mine[0], 1_001), None);
        assert_eq!(m.take("phone", &mine[1], 1_001), None);
        for nonce in &mine[2..] {
            assert!(m.take("phone", nonce, 1_001).is_some());
        }
        assert!(m.take("other", &theirs, 1_001).is_some());
        // Old ones are swept as new ones come.
        let m = PrMemory::default();
        let old = m.ask("phone", "old".into(), 1_000).0;
        m.ask("phone", "new".into(), 1_000 + ASK_TTL_MS + 1);
        assert_eq!(m.asks.lock().len(), 1);
        assert_eq!(m.take("phone", &old, 1_001), None);
    }

    #[test]
    fn a_merge_request_binds_a_repository_a_number_a_head_commit_and_a_method_each_checked() {
        let head = "0123456789abcdef0123456789abcdef01234567";
        let good = json!({"repo": "o/n", "number": 12, "head": head, "method": "squash"});
        let bound = bound_of(&good).unwrap();
        assert_eq!((bound.repo.as_str(), bound.number, bound.head.as_str(), bound.method), ("o/n", 12, head, "squash"));
        for (key, bad) in [
            ("repo", json!("o")),
            ("repo", json!("o/n/x")),
            ("repo", json!("o/n --admin")),
            ("repo", json!(7)),
            ("number", json!(0)),
            ("number", json!("12")),
            ("number", json!(-1)),
            ("number", json!(1u64 << 41)),
            ("head", json!("main")),
            ("head", json!(head.to_uppercase())),
            ("head", json!(&head[..39])),
            ("method", json!("--admin")),
            ("method", json!("Squash")),
            ("method", json!(null)),
        ] {
            let mut req = good.clone();
            req[key] = bad.clone();
            assert_eq!(bound_of(&req).map(drop).unwrap_err().code, "bad_request", "{key}: {bad}");
        }
        for key in ["repo", "number", "head", "method"] {
            let mut req = good.clone();
            req.as_object_mut().unwrap().remove(key);
            assert!(bound_of(&req).is_err(), "{key} left out");
        }
        // The challenge's line is the same whatever carries the fields.
        assert_eq!(pulls::bound_command(&bound), format!("merge github.com o/n #12 {head} squash"));
    }
}
