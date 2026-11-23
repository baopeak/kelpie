//! What a phone asks about a project's pull request: `pr_get` reads it (`pulls`), and `pr_challenge` with `pr_merge` merge it.
//!
//! `git` and `gh` can take a minute each, so none of these waits for them on the phone's connection (whose one loop also sends the heartbeats):
//! each is answered later on the connection's control queue, with the request's `req` tag, as the screen's slow verbs are.
//!
//! A merge is the one thing here that changes anything outside this computer, so it is bound end to end. The phone asks for a challenge for a
//! merge by a method, and the daemon itself reads the project: its repository from `origin`, and the pull request of its branch, with that
//! pull request's number, head commit and title, from `gh`. It refuses at once what could not be merged now. What it read it keeps in a table of
//! its own (not the app lock's, whose challenges unlock a phone), single use and good for a minute, as the line the passkey will be asked to sign
//! (`pulls::bound_command`), and gives the phone the same fields to show. The phone signs the line, with a request id that is the merge's own
//! (`pulls::challenge_request`), and sends `pr_merge`. The daemon then rebuilds the line from what the merge request says and refuses unless it
//! is the one it kept, checks the assertion with the same check a dangerous approval's passkey goes through (`Mobile::lock_recheck`), reads the
//! pull request again, and merges only if GitHub, the head commit and the required checks still allow it (`pulls::merge_refusal`). Every attempt
//! that gets as far as the passkey is audited, with what it was and what came of it.
//!
//! A `pr_merge` carries a `cid`. Its turn and its remembered reply belong to the task that carries the merge out, not to the connection: a
//! repeat of the request (after a dropped connection, over this or another one) that comes while the merge runs waits a moment for it and is
//! then told `in_progress` (the phone asks again); once the merge has answered, the repeat gets that answer again, marked `dup`. Either way it
//! never starts a second merge, and no lock is held while a program runs.
//!
//! The folder of a project never goes to a phone, and neither does anything that names it.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use parking_lot::Mutex;
use serde_json::{json, Value};

use super::{launch, refuse, same, take_device_token, CidTurn, Conn, Daemon, Gate, LockPath, Mobile, Permission, Refusal, SendBucket, Verb, ANSWER_DEDUPE, CID_WAIT, SLOW_AFTER};
use crate::pulls::{self, Bound};
use crate::session::now_millis;

/// Times one device may read a project's pull request, or ask for a merge's challenge (each is one or two calls to GitHub): this many at once,
/// then one every five seconds.
const READ_BURST: f64 = 4.0;
const READ_RATE: f64 = 0.2;
/// How long a merge's challenge may wait for the person.
const ASK_TTL_MS: u64 = 60_000;
/// Challenges one device may have open at once; asking for more is refused until one is used or ends.
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

    /// A new challenge for `device` over `command`, as (nonce, expiry in ms); `None` when the device already has all it may have open. A challenge
    /// is never dropped to make room: the person may be about to use it.
    pub fn ask(&self, device: &str, command: String, now: u64) -> Option<(String, u64)> {
        let mut asks = self.asks.lock();
        asks.retain(|a| a.expiry > now);
        if asks.iter().filter(|a| a.device == device).count() >= ASKS_PER_DEVICE {
            return None;
        }
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let expiry = now + ASK_TTL_MS;
        asks.push(Ask { device: device.to_string(), nonce: nonce.clone(), command, expiry });
        Some((nonce, expiry))
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

    /// How many challenges are kept now.
    #[cfg(test)]
    pub fn kept(&self) -> usize {
        self.asks.lock().len()
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
    let method = method_of(req)?;
    Ok(Bound { repo: repo.to_string(), number, head: head.to_string(), method })
}

/// How the request says to merge: the only part of a challenge the phone chooses.
fn method_of(req: &Value) -> Result<&'static str, Refusal> {
    req["method"].as_str().and_then(pulls::merge_method).ok_or_else(|| refuse("bad_request", "missing or bad method"))
}

/// An error reply for `r`, to the request tagged `tag`.
fn refusal_reply(tag: &Option<Value>, r: Refusal) -> Value {
    let mut reply = json!({"type": "error", "req": tag, "code": r.code, "reason": r.reason, "message": r.message});
    if let Some(busy) = r.busy {
        reply["busy"] = busy;
    }
    reply
}

/// What is left to do for a request once it has been checked: it waits for programs, so it is run off the connection's loop.
type Work = Pin<Box<dyn Future<Output = Result<Value, Refusal>> + Send>>;

/// Remembers a merge's reply for a repeat of its `cid` (none: nothing is remembered).
fn keep_reply(mobile: &Mobile, device: &str, cid: Option<&str>, reply: &Value) {
    let Some(cid) = cid else { return };
    let mut kept = reply.clone();
    if let Some(o) = kept.as_object_mut() {
        o.remove("req");
    }
    mobile.remember_reply(device, cid, ANSWER_DEDUPE, &kept);
}

/// What a connection stands on, read again after the waits of a merge: phone access on, still paired, control still its own, not locked. What `handle`
/// checked before may have changed since.
struct Standing {
    daemon: Arc<Daemon>,
    device: String,
    cap: Permission,
    lock_path: LockPath,
    resume: Option<String>,
    browser: bool,
}

impl Standing {
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
        if gate != Gate::Open || record.min(self.cap) < Permission::Control || !m.control_now(&self.device, self.lock_path, gate, self.browser) {
            return Err(refuse("no_control", "This phone may not take control now."));
        }
        Ok(())
    }
}

/// A merge request that passed every check that costs nothing: its challenge is used up, and what is left is the passkey and GitHub.
struct MergeJob {
    bound: Bound,
    folder: String,
    command: String,
    request: String,
    proof: Value,
    name: String,
    standing: Standing,
}

/// Carries a merge out: the passkey's assertion, whether this phone may still merge, then the merge itself. Audited from the thread that merges, so
/// that a phone that goes away meanwhile still leaves its line.
async fn carry_out(daemon: &Arc<Daemon>, device: &str, job: MergeJob) -> Result<Value, Refusal> {
    let MergeJob { bound, folder, command, request, proof, name, standing } = job;
    let (checker, who) = (daemon.clone(), device.to_string());
    tokio::task::spawn_blocking(move || checker.mobile.lock_recheck(&who, &proof, &request, &command)).await.map_err(|_| refuse("not_kept", "That did not finish."))??;
    if let Err(refused) = standing.may_still_merge() {
        daemon.mobile.audit(device, "pr_merge", &pulls::merge_summary(&name, &bound, &Err(refused.message.clone())));
        return Err(refused);
    }
    let (neutral, merger, who, merged) = (daemon.data_dir.clone(), daemon.clone(), device.to_string(), bound.clone());
    let done = tokio::task::spawn_blocking(move || {
        let result = pulls::merge_now(&folder, &neutral, &merged);
        merger.mobile.audit(&who, "pr_merge", &pulls::merge_summary(&name, &merged, &result));
        result
    })
    .await;
    match done {
        Ok(Ok(pulls::Done::Merged)) => Ok(json!({"type": "ack", "result": "merged", "repo": bound.repo, "number": bound.number})),
        Ok(Ok(pulls::Done::Pending(state))) => Ok(json!({"type": "ack", "result": "pending", "state": state, "repo": bound.repo, "number": bound.number, "message": format!("GitHub took the merge but the pull request is not merged yet ({state}): it may be queued.")})),
        Ok(Err(why)) => Err(refuse("merge_refused", why)),
        Err(_) => Err(refuse("not_kept", "The merge did not finish: look at the pull request on GitHub to see where it stands.")),
    }
}

impl Conn {
    /// The folder of the project a request names.
    fn project_folder_of(&self, req: &Value) -> Result<String, Refusal> {
        let id = req["project"].as_str().filter(|p| !p.is_empty() && p.len() <= launch::MAX_PROJECT_ID).ok_or_else(|| refuse("bad_request", "missing project"))?;
        self.daemon.mobile.project_folder(id).map_err(|(code, message)| refuse(code, message))
    }

    /// One of the pull request verbs, answered later: what costs nothing is checked here, and the rest is carried out on a task that sends the answer
    /// on the connection's control queue. `None` is returned for it, unless it was refused here.
    pub(super) async fn pr_verb(&self, granted: Permission, name: &str, req: &Value, tag: Option<Value>) -> Option<Value> {
        let Some(verb) = Verb::parse(name) else { return Some(refusal_reply(&tag, refuse("unknown_verb", format!("unknown request {name:?}")))) };
        if matches!(verb, Verb::PrMerge) {
            return self.pr_merge(verb, granted, req, tag).await;
        }
        let work = self.control_refused(verb, granted).and_then(|()| self.verb_allowed(verb, granted, req)).and_then(|()| if matches!(verb, Verb::PrGet) { self.pr_get(req) } else { self.pr_challenge(req) });
        match work {
            Ok(work) => self.answer_later(tag, work),
            Err(refused) => Some(refusal_reply(&tag, refused)),
        }
    }

    /// Sends what `work` comes to on this connection's control queue, tagged as the request was; the connection does not wait for it.
    fn answer_later(&self, tag: Option<Value>, work: Work) -> Option<Value> {
        let out = self.ctl.clone();
        tokio::spawn(async move {
            let reply = match work.await {
                Ok(mut reply) => {
                    reply["req"] = tag.unwrap_or(Value::Null);
                    reply
                }
                Err(refused) => refusal_reply(&tag, refused),
            };
            // Waits for room: an answer is never dropped (the queue is a few messages a second at most).
            let _ = tokio::time::timeout(SLOW_AFTER, out.send(reply.to_string())).await;
        });
        None
    }

    /// `pr_get {project}`: the pull request of the project's branch and GitHub's word on merging it (`pulls::status`), or `{none: true}`.
    fn pr_get(&self, req: &Value) -> Result<Work, Refusal> {
        if !self.daemon.mobile.pr.allow_read(&self.device) {
            return Err(refuse("rate_limited", "Asking too often: wait a moment."));
        }
        let folder = self.project_folder_of(req)?;
        let neutral = self.daemon.data_dir.clone();
        Ok(Box::pin(async move {
            let read = tokio::task::spawn_blocking(move || pulls::status(&folder, &neutral)).await.map_err(|_| refuse("not_kept", "That did not finish."))?;
            read.map(|pr| json!({"type": "ack", "result": "ok", "pr": pr})).map_err(|why| refuse("pr_unavailable", why))
        }))
    }

    /// A merge needs the passkey on this connection: one that cannot give it (a phone that never had a secure origin) cannot merge.
    fn merge_needs_passkey(&self) -> Result<(), Refusal> {
        if self.lock_path.passkey() {
            return Ok(());
        }
        Err(refuse("no_passkey", "Merging needs this phone's passkey, and this connection has none: pair the phone through Tailscale and set up the app lock."))
    }

    /// `pr_challenge {project, method}`: the challenge for merging the pull request of the project's branch, answered `{nonce, expiry, command, repo,
    /// number, head, method, title}`. Everything but the method is what this computer read just now (`pulls::challenge_read`), and the line the
    /// passkey will sign is built from it; the phone shows these fields and refuses a pull request or head other than the one the person was shown.
    fn pr_challenge(&self, req: &Value) -> Result<Work, Refusal> {
        let method = method_of(req)?;
        self.merge_needs_passkey()?;
        if !self.daemon.mobile.pr.allow_read(&self.device) {
            return Err(refuse("rate_limited", "Asking too often: wait a moment."));
        }
        let folder = self.project_folder_of(req)?;
        let (daemon, device) = (self.daemon.clone(), self.device.clone());
        Ok(Box::pin(async move {
            let neutral = daemon.data_dir.clone();
            let read = tokio::task::spawn_blocking(move || pulls::challenge_read(&folder, &neutral, method)).await.map_err(|_| refuse("not_kept", "That did not finish."))?;
            let read = read.map_err(|why| refuse("pr_unavailable", why))?;
            let command = pulls::bound_command(&read.bound);
            let Some((nonce, expiry)) = daemon.mobile.pr.ask(&device, command.clone(), now_millis()) else {
                return Err(refuse("merges_waiting", "Too many merges are waiting for a passkey on this phone: finish or cancel one, or wait a minute."));
            };
            let b = &read.bound;
            Ok(json!({"type": "ack", "result": "ok", "nonce": nonce, "expiry": expiry, "command": command, "repo": b.repo, "number": b.number, "head": b.head, "method": b.method, "title": read.title}))
        }))
    }

    /// Whether this phone may still merge, as the merge's task will read it again after the waits.
    fn standing(&self) -> Standing {
        Standing { daemon: self.daemon.clone(), device: self.device.clone(), cap: self.cap, lock_path: self.lock_path, resume: self.resume.clone(), browser: self.page_origin.is_some() }
    }

    /// The checks of a merge request that cost nothing, in order: its fields, the passkey on this connection, the project, and the challenge, which
    /// is used up by being asked for, whatever comes of the rest. It must be this device's, still good, and over the very line the request describes.
    fn merge_job(&self, req: &Value) -> Result<MergeJob, Refusal> {
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
        Ok(MergeJob { bound, folder, command, request: pulls::challenge_request(nonce), proof: req["recheck"].clone(), name: self.name.clone(), standing: self.standing() })
    }

    /// `pr_merge {project, repo, number, head, method, nonce, recheck, cid}`: merges the pull request once the passkey has covered it, answering later.
    /// The request's `cid` is held, and its reply remembered, by the task that merges (see the module's note), so that a repeat never merges twice.
    async fn pr_merge(&self, verb: Verb, granted: Permission, req: &Value, tag: Option<Value>) -> Option<Value> {
        let cid = req.get("cid").and_then(Value::as_str).filter(|c| !c.is_empty() && c.len() <= 64).filter(|_| verb.needs() <= granted);
        let mut turn = None;
        if let Some(cid) = cid {
            match CidTurn::take(&self.daemon.mobile.in_flight, &self.device, cid, CID_WAIT).await {
                Some(held) => turn = Some(held),
                // Not kept: the phone asks again with the same cid, and gets the first one's reply once there is one.
                None => return Some(json!({"type": "error", "req": tag, "code": "in_progress", "reason": null, "message": "That is still being done on the computer."})),
            }
            if let Some(mut reply) = self.daemon.mobile.repeated(&self.device, cid) {
                reply["dup"] = json!(true);
                if let Some(t) = tag {
                    reply["req"] = t;
                }
                return Some(reply);
            }
        }
        let prepared = self.control_refused(verb, granted).and_then(|()| self.verb_allowed(verb, granted, req)).and_then(|()| self.merge_job(req));
        let job = match prepared {
            Ok(job) => job,
            Err(refused) => {
                let reply = refusal_reply(&tag, refused);
                keep_reply(&self.daemon.mobile, &self.device, cid, &reply);
                return Some(reply);
            }
        };
        let (daemon, device, cid, out) = (self.daemon.clone(), self.device.clone(), cid.map(str::to_string), self.ctl.clone());
        tokio::spawn(async move {
            let mut reply = match carry_out(&daemon, &device, job).await {
                Ok(reply) => reply,
                Err(refused) => refusal_reply(&None, refused),
            };
            // Kept before the turn ends: a repeat that waits for the turn finds the reply.
            keep_reply(&daemon.mobile, &device, cid.as_deref(), &reply);
            drop(turn);
            reply["req"] = tag.unwrap_or(Value::Null);
            let _ = tokio::time::timeout(SLOW_AFTER, out.send(reply.to_string())).await;
        });
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A challenge that was just made.
    fn ask(m: &PrMemory, device: &str, command: &str, now: u64) -> (String, u64) {
        m.ask(device, command.to_string(), now).expect("room for a challenge")
    }

    #[test]
    fn a_challenge_is_used_once_by_the_device_that_asked_and_only_for_a_minute() {
        let m = PrMemory::default();
        let (nonce, expiry) = ask(&m, "phone", "merge github.com o/n #1 abc squash", 1_000);
        assert_eq!(expiry, 1_000 + ASK_TTL_MS);
        assert_eq!(nonce.len(), 32);
        // Another device cannot use it, and trying does not use it up.
        assert_eq!(m.take("other", &nonce, 2_000), None);
        assert_eq!(m.take("phone", "not-the-nonce", 2_000), None);
        assert_eq!(m.take("phone", &nonce, 2_000), Some(("merge github.com o/n #1 abc squash".to_string(), expiry)));
        assert_eq!(m.take("phone", &nonce, 2_000), None, "used up");
        let (late, expiry) = ask(&m, "phone", "x", 1_000);
        assert_eq!(m.take("phone", &late, expiry), None, "it ends when its minute does");
        let (kept, expiry) = ask(&m, "phone", "y", 1_000);
        assert!(m.take("phone", &kept, expiry - 1).is_some());
        // Two challenges are two nonces.
        assert_ne!(ask(&m, "phone", "z", 1_000).0, ask(&m, "phone", "z", 1_000).0);
    }

    #[test]
    fn a_device_holds_a_few_challenges_and_asking_for_more_is_refused_and_never_costs_it_one_it_holds() {
        let m = PrMemory::default();
        let theirs = ask(&m, "other", "theirs", 1_000).0;
        let mine: Vec<String> = (0..ASKS_PER_DEVICE).map(|i| ask(&m, "phone", &format!("c{i}"), 1_000).0).collect();
        assert_eq!(m.ask("phone", "one more".into(), 1_000), None, "over the limit");
        assert_eq!(m.kept(), ASKS_PER_DEVICE + 1, "nothing was dropped to make room");
        // Another device is not held back by this one's.
        assert!(m.ask("third", "x".into(), 1_000).is_some());
        // Using one frees a place; so does the end of their minute.
        assert!(m.take("phone", &mine[0], 1_001).is_some());
        assert!(m.ask("phone", "now it fits".into(), 1_001).is_some());
        assert_eq!(m.ask("phone", "full again".into(), 1_001), None);
        assert!(m.ask("phone", "a minute later".into(), 1_000 + ASK_TTL_MS + 1).is_some());
        assert_eq!(m.take("other", &theirs, 1_001), None, "theirs ended with the minute too, and was swept");
        // Old ones are swept as new ones come.
        let m = PrMemory::default();
        let old = ask(&m, "phone", "old", 1_000).0;
        ask(&m, "phone", "new", 1_000 + ASK_TTL_MS + 1);
        assert_eq!(m.kept(), 1);
        assert_eq!(m.take("phone", &old, 1_001), None);
    }

    #[test]
    fn a_merge_request_binds_a_repository_a_number_a_head_commit_and_a_method_each_checked() {
        let head = "0123456789abcdef0123456789abcdef01234567";
        let good = json!({"repo": "o/n", "number": 12, "head": head, "method": "squash"});
        let Ok(bound) = bound_of(&good) else { panic!("a whole request was refused") };
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

    #[test]
    fn the_method_is_the_one_field_a_challenge_takes_from_the_phone() {
        assert_eq!(method_of(&json!({"method": "rebase"})).ok(), Some("rebase"));
        for bad in [json!({}), json!({"method": "--admin"}), json!({"method": 1})] {
            assert_eq!(method_of(&bad).map(drop).unwrap_err().code, "bad_request", "{bad}");
        }
    }
}
