//! The daemon's own requests to the Kelpie app: what only the window can do (a browser pane's screen and input today, the desktop's later).
//!
//! Everything else the daemon says to a client is an event nobody answers (`Daemon::broadcast`). A provider is a connection of the verified
//! app (`Role::App`, `peer_identity`) that registered for a kind of work; the daemon sends it `{"preq": n, "method", "params"}` and waits up to
//! `TIMEOUT` for `provider.reply {preq: n, result | error}`, or sends `{"pnote": method, "params"}`, which nobody answers. What the app says on its
//! own (a frame, the person pressing Stop) comes as `provider.event`. A second registration of a kind is refused, and the desktop hears of it:
//! something else of this user's tried to stand in for the window. When the provider's connection ends, every request it had is answered
//! with an error at once and the kind is free again. The same app process registering again from a new connection (a quick reconnect, while the old
//! one is still being wound down) takes the kind over with no alert: the peer's process id says it is the same window.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::sync::oneshot;

use crate::auth::Role;
use crate::session::{json_out, ClientTx};

/// How long a request waits for the app.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// The kinds of work a provider may register for.
pub const KINDS: &[&str] = &["browser"];

struct Registered {
    client: u64,
    /// The app's process, as the connection's peer said it (`peer_identity`).
    pid: Option<u32>,
    tx: ClientTx,
}

struct Waiting {
    client: u64,
    reply: oneshot::Sender<Result<Value, String>>,
}

#[derive(Default)]
struct Inner {
    by_kind: HashMap<&'static str, Registered>,
    waiting: HashMap<u64, Waiting>,
    next: u64,
}

#[derive(Default)]
pub struct Providers {
    inner: Mutex<Inner>,
}

/// What a registration came to.
#[derive(Debug, PartialEq)]
pub enum Registration {
    Registered,
    /// The same app process took the kind over from its connection before, `n`: whatever that one served ends.
    Replaced(u64),
    /// Another connection holds the kind: the desktop is told.
    Taken,
}

impl Providers {
    /// `client` provides `kind` from now on, if it is the verified app and nobody else does. The same connection registering again is no change, and
    /// neither is the same app process on a new connection: its old one is on its way out (what it was asked is answered with an error now).
    pub fn register(&self, client: u64, role: &Role, kind: &str, pid: Option<u32>, tx: &ClientTx) -> Result<Registration> {
        if *role != Role::App {
            bail!("only the Kelpie app can provide {kind}");
        }
        let kind = KINDS.iter().copied().find(|k| *k == kind).ok_or_else(|| anyhow!("unknown provider kind {kind:?}"))?;
        let mut inner = self.inner.lock();
        let old = match inner.by_kind.get(kind) {
            Some(r) if r.client != client && (pid.is_none() || r.pid != pid) => return Ok(Registration::Taken),
            Some(r) if r.client != client => Some(r.client),
            _ => None,
        };
        inner.by_kind.insert(kind, Registered { client, pid, tx: tx.clone() });
        if let Some(old) = old {
            let ids: Vec<u64> = inner.waiting.iter().filter(|(_, w)| w.client == old).map(|(id, _)| *id).collect();
            for id in ids {
                if let Some(w) = inner.waiting.remove(&id) {
                    let _ = w.reply.send(Err("the Kelpie app went away".into()));
                }
            }
        }
        Ok(match old {
            Some(old) => Registration::Replaced(old),
            None => Registration::Registered,
        })
    }

    /// The connection that provides `kind`, if one does.
    pub fn provider(&self, kind: &str) -> Option<u64> {
        self.inner.lock().by_kind.get(kind).map(|r| r.client)
    }

    /// Asks the provider of `kind` and waits for its answer, at most `TIMEOUT`.
    pub async fn request(&self, kind: &str, method: &str, params: Value) -> Result<Value> {
        self.request_within(kind, method, params, TIMEOUT).await
    }

    pub async fn request_within(&self, kind: &str, method: &str, params: Value, limit: Duration) -> Result<Value> {
        let (id, rx) = {
            let mut inner = self.inner.lock();
            let Some(r) = inner.by_kind.get(kind) else { bail!("the Kelpie app is not open on the computer") };
            let (client, tx) = (r.client, r.tx.clone());
            inner.next += 1;
            let id = inner.next;
            let (reply, rx) = oneshot::channel();
            inner.waiting.insert(id, Waiting { client, reply });
            if tx.send(json_out(&json!({"preq": id, "method": method, "params": params}))).is_err() {
                inner.waiting.remove(&id);
                bail!("the Kelpie app is not open on the computer");
            }
            (id, rx)
        };
        // A caller that is dropped before the answer (its connection closed) leaves nothing waiting behind.
        let _forget = Forget(self, id);
        match tokio::time::timeout(limit, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(anyhow!(e)),
            Ok(Err(_)) => bail!("the Kelpie app went away"),
            Err(_) => {
                self.inner.lock().waiting.remove(&id);
                bail!("the Kelpie app did not answer in time")
            }
        }
    }

    /// Tells the provider of `kind` something it does not answer. False when there is none.
    pub fn notify(&self, kind: &str, method: &str, params: Value) -> bool {
        let inner = self.inner.lock();
        inner.by_kind.get(kind).is_some_and(|r| r.tx.send(json_out(&json!({"pnote": method, "params": params}))).is_ok())
    }

    /// `provider.reply` from `client`: answers its own requests only.
    pub fn reply(&self, client: u64, params: &Value) {
        let Some(id) = params.get("preq").and_then(Value::as_u64) else { return };
        let mut inner = self.inner.lock();
        if inner.waiting.get(&id).is_none_or(|w| w.client != client) {
            return;
        }
        let waiting = inner.waiting.remove(&id).expect("present");
        let answer = match params.get("error") {
            Some(e) if !e.is_null() => Err(e.get("message").and_then(Value::as_str).or_else(|| e.as_str()).unwrap_or("the Kelpie app refused").chars().take(300).collect()),
            _ => Ok(params.get("result").cloned().unwrap_or(Value::Null)),
        };
        let _ = waiting.reply.send(answer);
    }

    /// `client` is gone: the kinds it provided are free, and what it was asked is answered with an error now. The kinds it provided.
    pub fn client_gone(&self, client: u64) -> Vec<&'static str> {
        let mut inner = self.inner.lock();
        let kinds: Vec<&'static str> = inner.by_kind.iter().filter(|(_, r)| r.client == client).map(|(k, _)| *k).collect();
        for k in &kinds {
            inner.by_kind.remove(k);
        }
        let ids: Vec<u64> = inner.waiting.iter().filter(|(_, w)| w.client == client).map(|(id, _)| *id).collect();
        for id in ids {
            if let Some(w) = inner.waiting.remove(&id) {
                let _ = w.reply.send(Err("the Kelpie app went away".into()));
            }
        }
        kinds
    }

    #[cfg(test)]
    fn waiting(&self) -> usize {
        self.inner.lock().waiting.len()
    }
}

/// Removes request `.1` from the waiting ones when it goes out of scope, however it ended.
struct Forget<'a>(&'a Providers, u64);

impl Drop for Forget<'_> {
    fn drop(&mut self) {
        self.0.inner.lock().waiting.remove(&self.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn channel() -> (ClientTx, tokio::sync::mpsc::UnboundedReceiver<crate::session::Outgoing>) {
        crate::session::client_channel()
    }

    fn body(o: &crate::session::Outgoing) -> Value {
        // A JSON frame: u32 length, one byte of kind, the JSON.
        serde_json::from_slice(&o.data[5..]).unwrap()
    }

    #[test]
    fn only_the_verified_app_may_provide_and_only_one_of_it() {
        let p = Providers::default();
        let (tx, _rx) = channel();
        for role in [Role::User, Role::Anonymous, Role::Session("s".into()), Role::Claimed("s".into())] {
            assert!(p.register(1, &role, "browser", None, &tx).is_err(), "{role:?}");
        }
        assert_eq!(p.provider("browser"), None);
        assert!(p.register(1, &Role::App, "screen-of-everything", None, &tx).is_err(), "an unknown kind");
        assert_eq!(p.register(1, &Role::App, "browser", Some(40), &tx).unwrap(), Registration::Registered);
        assert_eq!(p.register(1, &Role::App, "browser", Some(40), &tx).unwrap(), Registration::Registered, "the same connection again changes nothing");
        assert_eq!(p.register(2, &Role::App, "browser", Some(41), &tx).unwrap(), Registration::Taken, "a second one is refused");
        assert_eq!(p.register(2, &Role::App, "browser", None, &tx).unwrap(), Registration::Taken, "and one whose process is not known");
        assert_eq!(p.provider("browser"), Some(1));
        assert_eq!(p.client_gone(2), Vec::<&str>::new(), "the refused one held nothing");
        assert_eq!(p.client_gone(1), vec!["browser"]);
        assert_eq!(p.register(2, &Role::App, "browser", Some(41), &tx).unwrap(), Registration::Registered, "free once the first is gone");
    }

    #[tokio::test]
    async fn a_request_is_answered_by_its_provider_only() {
        let p = Arc::new(Providers::default());
        let (tx, mut rx) = channel();
        p.register(7, &Role::App, "browser", None, &tx).unwrap();
        let asking = tokio::spawn({
            let p = p.clone();
            async move { p.request("browser", "browser.panes", json!({"x": 1})).await }
        });
        let sent = body(&rx.recv().await.unwrap());
        assert_eq!((sent["method"].as_str(), sent["params"]["x"].as_u64()), (Some("browser.panes"), Some(1)));
        let id = sent["preq"].as_u64().unwrap();
        p.reply(8, &json!({"preq": id, "result": "forged"}));
        assert_eq!(p.waiting(), 1, "another connection cannot answer it");
        p.reply(7, &json!({"preq": id, "result": {"panes": []}}));
        assert_eq!(asking.await.unwrap().unwrap(), json!({"panes": []}));
        assert_eq!(p.waiting(), 0);
        // An error is an error, with the app's words.
        let asking = tokio::spawn({
            let p = p.clone();
            async move { p.request("browser", "browser.takeover_start", json!({})).await }
        });
        let id = body(&rx.recv().await.unwrap())["preq"].as_u64().unwrap();
        p.reply(7, &json!({"preq": id, "error": {"message": "no such pane"}}));
        assert_eq!(asking.await.unwrap().unwrap_err().to_string(), "no such pane");
    }

    #[tokio::test]
    async fn a_request_times_out_and_leaves_nothing_behind() {
        let p = Providers::default();
        let (tx, _rx) = channel();
        p.register(1, &Role::App, "browser", None, &tx).unwrap();
        let err = p.request_within("browser", "browser.panes", json!({}), Duration::from_millis(100)).await.unwrap_err();
        assert!(err.to_string().contains("in time"), "{err}");
        assert_eq!(p.waiting(), 0);
        assert!(p.request("other", "x", json!({})).await.is_err(), "nobody provides it");
    }

    #[tokio::test]
    async fn a_provider_that_goes_answers_what_it_was_asked_at_once() {
        let p = Arc::new(Providers::default());
        let (tx, mut rx) = channel();
        p.register(3, &Role::App, "browser", None, &tx).unwrap();
        let asking = tokio::spawn({
            let p = p.clone();
            async move { p.request("browser", "browser.panes", json!({})).await }
        });
        rx.recv().await.unwrap();
        let started = std::time::Instant::now();
        assert_eq!(p.client_gone(3), vec!["browser"]);
        let err = asking.await.unwrap().unwrap_err();
        assert!(err.to_string().contains("went away") && started.elapsed() < Duration::from_secs(1), "{err}");
        assert_eq!((p.waiting(), p.provider("browser")), (0, None));
        assert!(!p.notify("browser", "browser.takeover_input", json!({})), "nobody to tell");
    }

    #[tokio::test]
    async fn the_same_app_reconnecting_keeps_the_kind_and_what_its_old_connection_was_asked_ends() {
        let p = Arc::new(Providers::default());
        let (old_tx, mut old_rx) = channel();
        p.register(1, &Role::App, "browser", Some(500), &old_tx).unwrap();
        let asking = tokio::spawn({
            let p = p.clone();
            async move { p.request("browser", "browser.panes", json!({})).await }
        });
        old_rx.recv().await.unwrap();
        let (new_tx, _new_rx) = channel();
        assert_eq!(p.register(2, &Role::App, "browser", Some(500), &new_tx).unwrap(), Registration::Replaced(1), "the same process: no alert");
        assert_eq!(p.provider("browser"), Some(2));
        assert!(asking.await.unwrap().unwrap_err().to_string().contains("went away"));
        assert_eq!(p.client_gone(1), Vec::<&str>::new(), "the old connection going takes nothing from the new one");
        assert_eq!(p.provider("browser"), Some(2));
    }

    #[tokio::test]
    async fn a_request_whose_caller_is_dropped_leaves_nothing_waiting() {
        let p = Arc::new(Providers::default());
        let (tx, mut rx) = channel();
        p.register(1, &Role::App, "browser", None, &tx).unwrap();
        let asking = tokio::spawn({
            let p = p.clone();
            async move { p.request("browser", "browser.takeover_text", json!({})).await }
        });
        rx.recv().await.unwrap();
        assert_eq!(p.waiting(), 1);
        asking.abort();
        let _ = asking.await;
        assert_eq!(p.waiting(), 0);
    }
}