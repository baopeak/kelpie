//! Agents using browser panes, the app's side. The daemon's `browser.agent` request goes to the window (`agent-browser-request`), which
//! picks the pane and refuses what the person did not allow; for every tool but `tabs` it then calls `browser_agent_run`, and this runs
//! the tool in the page through the injected engine (`browser_agent.js`) and answers the daemon itself.
//!
//! One call at a time per pane, each within the time the daemon waits for it (less a margin): a call that waited too long behind another
//! is refused rather than run after the agent was told it timed out. Before each, what only this side can know for sure is checked again
//! (`decide`): a phone holding the pane, another session acting in it, the person's own input in the page, the site the page is on. A
//! session that acts holds the pane (its lease) until it has been idle 20 s, the person presses Stop, a phone takes the pane over, the
//! pane closes, or a shared page leaves its site; the window hears of each (`agent-browser-lease`) and shows who acts. A call that came
//! before such an end is refused. While an agent uses a pane, its page keeps the normal memory target: a trimmed page can take seconds to
//! answer.
//!
//! Input is the engine's synthetic DOM events, or, where the platform has it (`agent_cdp`) and the computer shows the pane, trusted input
//! through the DevTools protocol; there, too, a shown page can be pictured, and a pane's requests are kept from the first agent call until
//! agents' use of it ends. Typed text, scripts and page content are never logged here.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Webview};

use base64::Engine as _;

use super::agent_cdp::{self as cdp, Fallback};
use super::agent_snapshot::{self as snap, Kind, Refs, Snapshot, STALE};
use super::{valid_label, Browsers, Takeovers, NOT_SHOWN};
use crate::daemon_client::DaemonClient;
use crate::fs_ops::CmdResult;

/// A lease ends after this long without a call.
const IDLE: Duration = Duration::from_secs(20);
/// Acting waits this long after the person's own input in the page.
const PERSON: Duration = Duration::from_secs(10);
/// How long one call into the page may take.
const PAGE_CALL: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(250);
/// What is kept back from the daemon's wait, so that the answer reaches it in time.
const MARGIN: Duration = Duration::from_secs(2);
/// A call starts only with this much of its time left.
const MIN_START: Duration = Duration::from_secs(2);
/// An action is done only with this much left: the action, a short settle and the answer.
const ACT_NEED: Duration = Duration::from_secs(3);
/// Kept back from the settle for the snapshot that follows an action.
const RESERVE: Duration = Duration::from_millis(1500);
/// The longest JSON `eval` returns.
const EVAL_MAX: usize = 8 * 1024;
/// The most changed lines an action reports.
const DIFF_LINES: usize = 20;
/// The size of the snapshots compared for a diff.
const DIFF_CHARS: usize = 20_000;
/// Requests the window may still pass on.
const PENDING: usize = 256;
/// Around each step of Kelpie's own trusted input, the page does not count input as the person's for this long: the longest the step may
/// take, and a little more. Ended when the input ends.
const BUSY_MS: u64 = PAGE_CALL.as_millis() as u64 + 500;
/// How long a call waits for the network list to start (it goes on starting after).
const NET_START: Duration = Duration::from_millis(1500);

const NOT_RESPONDING: &str = "the page is not responding";
const UNREACHABLE: &str = "Kelpie cannot reach this page now (it is loading, or shows an error page); try again in a moment";
const BUSY: &str = "Kelpie was busy with an earlier call in this pane; try again.";
const NO_TIME: &str = "Not enough time was left for this action (Kelpie was busy with an earlier call in this pane); try again.";
const STOPPED: &str = "The user stopped agents in this pane.";
const PHONE: &str = "A phone controls this pane now; agents cannot use it until the phone lets go.";
const LEFT_SITE: &str = "The page left the site it was shared on; the user has to share the pane again.";
const CLOSED: &str = "The pane was closed.";
const READ_ONLY: &str = "Browser control is set to read only: agents may look at pages but not act on them.";
const EVAL_OFF: &str = "Running JavaScript is allowed only in panes opened for agents, and only when the user turned it on in Kelpie → Settings → Agents → Browser panes.";

/// What the window says about a call: who calls, how the pane came to be usable (`opened` for the session, or `shared`), the sites a
/// shared pane may stay on, and whether scripts may run. The window also sends `isolated`, `sites` and `project`: not taken from it, this
/// side knows how it made the pane and has the daemon's own policy (`Policy`).
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Ctx {
    pub session: String,
    pub session_name: String,
    pub kind: String,
    pub origins: Vec<String>,
    pub may_eval: bool,
}

impl Ctx {
    fn kind(&self) -> Kind {
        if self.kind == "opened" {
            Kind::Opened
        } else {
            Kind::Shared
        }
    }
}

/// The daemon's browser policy, as it came with the request.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Policy {
    pub mode: String,
    pub eval: bool,
    pub sites: Vec<String>,
}

/// What a tool does to a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Read,
    Act,
    Eval,
}

pub fn class_of(tool: &str) -> Option<Class> {
    Some(match tool {
        "snapshot" | "wait" | "screenshot" | "console" | "network" => Class::Read,
        "click" | "type" | "press" | "navigate" => Class::Act,
        "eval" => Class::Eval,
        _ => return None,
    })
}

/// What is known about the pane when a call comes.
pub struct Facts<'a> {
    pub class: Class,
    pub session: &'a str,
    /// A phone holds the pane.
    pub phone: bool,
    /// The session holding the act lease, its name, and how long it has been idle.
    pub lease: Option<(&'a str, &'a str, Duration)>,
    /// Since the person's own last input in the page.
    pub person: Option<Duration>,
    /// The page's address now.
    pub url: &'a str,
    /// The pane has a browser profile of its own, as this side made it.
    pub isolated: bool,
}

/// Whether the call may go ahead, or why not. Every tool: no phone holds the pane, and the page is on a site the pane may be on. Acting
/// and scripts also: the policy allows it, no other session holds the lease, and the person has not used the page in the last 10 s.
pub fn decide(f: &Facts, ctx: &Ctx, policy: &Policy) -> Result<(), String> {
    if f.phone {
        return Err(PHONE.into());
    }
    if f.class != Class::Read {
        if policy.mode != "act" {
            return Err(READ_ONLY.into());
        }
        if f.class == Class::Eval && !(policy.eval && ctx.may_eval && ctx.kind() == Kind::Opened) {
            return Err(EVAL_OFF.into());
        }
        if let Some((holder, name, idle)) = f.lease {
            if holder != f.session && idle < IDLE {
                let who = if name.is_empty() { "Another session".to_string() } else { format!("Another session ({name})") };
                return Err(format!("{who} is acting in this pane; try again once it has been idle for 20 s."));
            }
        }
        if let Some(ago) = f.person.filter(|ago| *ago < PERSON) {
            return Err(format!(
                "The user used this pane {} s ago; agents wait 10 s after the user's own input. Try again shortly.",
                ago.as_secs()
            ));
        }
    }
    snap::site_allowed(f.url, ctx.kind(), f.isolated, &ctx.origins, &policy.sites)
}

/// Whether pane `label` was made with a browser profile of its own. None is yet: every pane shares the person's profile.
fn isolated(_label: &str) -> bool {
    false
}

/// How long this side has for a request: what the daemon waits for it (15 s; a wait or a navigation its own timeout and 5 s more), less
/// a margin for the answer to reach it.
pub fn budget(tool: &str, args: &Value) -> Duration {
    let timeout = args.get("timeout_s").and_then(Value::as_u64);
    let daemon = Duration::from_secs(match tool {
        "wait" => timeout.unwrap_or(10).min(30) + 5,
        "navigate" => timeout.unwrap_or(15).min(25) + 5,
        _ => 15,
    });
    daemon - MARGIN
}

/// Whether a call with `left` of its time may start (it may have waited behind another call in the pane).
pub fn may_start(left: Duration) -> Result<(), String> {
    if left < MIN_START {
        return Err(BUSY.into());
    }
    Ok(())
}

/// Whether an action may be done with `left` of the call's time: the person must not see it happen after the agent was told it failed.
pub fn may_act(left: Duration) -> Result<(), String> {
    if left < ACT_NEED {
        return Err(NO_TIME.into());
    }
    Ok(())
}

// ---------- requests ----------

/// A request the window was handed, until it passes it on: the tool and the session the daemon named, the policy it sent, when it came,
/// the time this side has for it, and the daemon connection it came on.
#[derive(Clone, Debug)]
pub struct Pending {
    tool: String,
    session: String,
    policy: Policy,
    received: Instant,
    deadline: Instant,
    conn: u64,
}

/// The requests the window may still pass on, by the daemon's number. A number seen again (after the daemon restarted) replaces the older
/// record.
#[derive(Default, Debug)]
struct Requests {
    map: HashMap<u64, Pending>,
}

impl Requests {
    fn insert(&mut self, preq: u64, p: Pending) {
        if self.map.len() >= PENDING && !self.map.contains_key(&preq) {
            if let Some(oldest) = self.map.iter().min_by_key(|(_, p)| p.received).map(|(k, _)| *k) {
                self.map.remove(&oldest);
            }
        }
        self.map.insert(preq, p);
    }

    fn take(&mut self, preq: u64) -> Option<Pending> {
        self.map.remove(&preq)
    }

    fn clear(&mut self) {
        self.map.clear();
    }
}

// ---------- state ----------

struct Lease {
    session: String,
    name: String,
    last: Instant,
}

#[derive(Default)]
struct PaneState {
    lease: Option<Lease>,
    /// When agents' use of the pane last ended, and why (`stopped`, `phone`, `left-site`, `closed`): a call that came before is refused.
    ended: Option<(Instant, &'static str)>,
    last_use: Option<Instant>,
    /// The idle watcher runs.
    watching: bool,
    refs: Refs,
    /// The pane's requests, kept while `net_on` (receivers `net_tokens`); `net_gen` changes whenever keeping them stops.
    net: Arc<Mutex<cdp::NetLog>>,
    net_on: bool,
    net_tokens: cdp::Tokens,
    net_gen: u64,
    /// The sites the latest call's agent may use: a page elsewhere ends the list (`navigating`).
    #[cfg_attr(not(windows), allow(dead_code))]
    net_scope: Option<Scope>,
}

/// The sites a call's agent may use in a pane.
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Scope {
    kind: Kind,
    isolated: bool,
    origins: Vec<String>,
    sites: Vec<String>,
}

#[derive(Default)]
struct Pane {
    turn: tokio::sync::Mutex<()>,
    state: Mutex<PaneState>,
    /// Starting and stopping the network list.
    net_turn: tokio::sync::Mutex<()>,
}

/// The agent state of every pane an agent used, the requests the window holds, and the number of the daemon connection.
#[derive(Default)]
pub struct AgentPanes {
    panes: Mutex<HashMap<String, Arc<Pane>>>,
    requests: Mutex<Requests>,
    conn: AtomicU64,
}

impl AgentPanes {
    fn pane(&self, label: &str) -> Arc<Pane> {
        self.panes.lock().entry(label.to_string()).or_default().clone()
    }

    fn existing(&self, label: &str) -> Option<Arc<Pane>> {
        self.panes.lock().get(label).cloned()
    }
}

/// Answers the daemon's request `preq`, unless the connection it came on is gone: a new daemon numbers its requests from the start again.
fn reply(app: &AppHandle, conn: u64, preq: u64, result: Result<Value, String>) {
    if app.try_state::<AgentPanes>().is_none_or(|s| s.conn.load(Ordering::Relaxed) != conn) {
        return;
    }
    if let Some(c) = app.try_state::<Arc<DaemonClient>>() {
        let _ = c.provider_reply(preq, result);
    }
}

fn lease_event(app: &AppHandle, label: &str, session: &str, name: &str, active: bool, reason: Option<&str>) {
    let mut e = json!({"label": label, "session": session, "name": name, "active": active});
    if let Some(r) = reason {
        e["reason"] = json!(r);
    }
    let _ = app.emit_to("main", "agent-browser-lease", e);
}

fn ended_text(reason: &str) -> &'static str {
    match reason {
        "phone" => PHONE,
        "left-site" => LEFT_SITE,
        "closed" => CLOSED,
        _ => STOPPED,
    }
}

fn engine_js(nonce: &str, func: &str, args: &Value) -> String {
    format!("(window.__kelpieAgent?window.__kelpieAgent.{func}({},{args}):null)", quoted(nonce))
}

/// An agent's expression, run as the page's own script would; the engine only turns its value (or what it threw) into JSON. The script
/// carries no nonce: the expression may call functions the page replaced, which can read the source of the script that called them.
fn eval_js(expr: &str) -> String {
    format!("(()=>{{const k=window.__kelpieAgent;if(!k)return null;let v;try{{v=(\n{expr}\n);}}catch(e){{return k.value(e,true);}}return k.value(v);}})()")
}

/// Ends the dialog answering an action started, without waiting for the page.
fn dialogs_off(app: &AppHandle, label: &str, wv: &Webview) {
    if let Some(nonce) = app.state::<Browsers>().agent_nonce(label) {
        let _ = wv.eval(engine_js(&nonce, "dialogs", &json!({})));
    }
}

/// The daemon's `browser.agent` request: handed to the window, which knows the panes. Remembered, so that the window's call can be checked
/// against it, the daemon's own policy applied, and the call kept within the daemon's wait.
pub fn on_request(app: &AppHandle, preq: u64, params: Value) {
    let tool = params["tool"].as_str().unwrap_or_default().to_string();
    let session = params["session"]["id"].as_str().or(params["session"].as_str()).unwrap_or_default().to_string();
    let policy: Policy = serde_json::from_value(params["policy"].clone()).unwrap_or_default();
    let received = Instant::now();
    let deadline = received + budget(&tool, &params["args"]);
    let conn = app.try_state::<AgentPanes>().map_or(0, |s| s.conn.load(Ordering::Relaxed));
    if let Some(state) = app.try_state::<AgentPanes>() {
        state.requests.lock().insert(preq, Pending { tool, session, policy, received, deadline, conn });
    }
    let payload = json!({"preq": preq, "session": params["session"], "tool": params["tool"], "args": params["args"], "policy": params["policy"]});
    if app.emit_to("main", "agent-browser-request", payload).is_err() {
        forget_request(app, preq);
        reply(app, conn, preq, Err("the window is not ready".into()));
    }
}

/// The window answered request `preq` itself (`provider_reply`): it will not pass it on.
pub(crate) fn forget_request(app: &AppHandle, preq: u64) {
    if let Some(state) = app.try_state::<AgentPanes>() {
        state.requests.lock().take(preq);
    }
}

/// The daemon's connection dropped: its requests are gone, and answers to them must not reach the next daemon.
pub(super) fn daemon_gone(app: &AppHandle) {
    if let Some(state) = app.try_state::<AgentPanes>() {
        state.conn.fetch_add(1, Ordering::Relaxed);
        state.requests.lock().clear();
    }
}

/// Ends any agent's use of pane `label` now: calls that came before are refused, an action's dialog answering stops, the lease goes, and
/// the window hears `reason`.
fn end(app: &AppHandle, label: &str, reason: &'static str) {
    let lease = app.state::<AgentPanes>().existing(label).and_then(|pane| {
        let mut st = pane.state.lock();
        st.ended = Some((Instant::now(), reason));
        st.lease.take()
    });
    let (session, name) = lease.map(|l| (l.session, l.name)).unwrap_or_default();
    lease_event(app, label, &session, &name, false, Some(reason));
    if let Some(pane) = app.state::<AgentPanes>().existing(label) {
        network_stop(app, label, &pane);
    }
    if let Some(wv) = app.get_webview(label) {
        dialogs_off(app, label, &wv);
        crate::webview_memory::keep_normal(&wv, false);
    }
}

/// The person pressed Stop on the pane's banner.
#[tauri::command]
pub async fn browser_agent_stop(app: AppHandle, label: String) -> CmdResult<()> {
    valid_label(&label)?;
    end(&app, &label, "stopped");
    Ok(())
}

/// A phone took pane `label` over: the agent acting in it loses it.
pub(super) fn preempted(app: &AppHandle, label: &str) {
    end(app, label, "phone");
}

/// The pane's webview is closing.
pub(super) fn pane_closed(app: &AppHandle, label: &str) {
    let removed = app.state::<AgentPanes>().panes.lock().remove(label);
    if let Some(pane) = removed {
        let lease = {
            let mut st = pane.state.lock();
            st.ended = Some((Instant::now(), "closed"));
            st.lease.take()
        };
        if let Some(l) = lease {
            lease_event(app, label, &l.session, &l.name, false, Some("closed"));
        }
        network_stop(app, label, &pane);
    }
    crate::webview_memory::forget(label);
}

/// Stops keeping the pane's requests and forgets those kept: the next agent call starts the list again, numbered on from where it was (a
/// cursor stays a cursor). The page stops reporting requests (`Network.disable`) unless a newer start came first: starts and stops of a
/// pane's list take turns (`net_turn`).
fn network_stop(app: &AppHandle, label: &str, pane: &Arc<Pane>) {
    let tokens = {
        let mut st = pane.state.lock();
        st.net_gen += 1;
        st.net_on = false;
        let last = st.net.lock().last();
        st.net = Arc::new(Mutex::new(cdp::NetLog::after(last)));
        std::mem::take(&mut st.net_tokens)
    };
    let wv = app.get_webview(label);
    let had = !tokens.is_empty();
    cdp::network_off(wv.as_ref(), tokens);
    if let (true, Some(wv)) = (had, wv) {
        let pane = pane.clone();
        tauri::async_runtime::spawn(async move {
            let _turn = pane.net_turn.lock().await;
            if !pane.state.lock().net_on {
                cdp::network_disable(&wv).await;
            }
        });
    }
}

/// Keeps the pane's requests from now on, unless that runs already. Stopped meanwhile (`net_gen` changed), the receivers go again at once.
async fn network_start(pane: Arc<Pane>, wv: Webview) {
    let _turn = pane.net_turn.lock().await;
    let (log, generation) = {
        let mut st = pane.state.lock();
        if st.net_on {
            return;
        }
        st.net_on = true;
        (st.net.clone(), st.net_gen)
    };
    let started = cdp::network_on(&wv, log).await;
    let late = {
        let mut st = pane.state.lock();
        match started {
            Some(tokens) if st.net_gen == generation => {
                st.net_tokens = tokens;
                None
            }
            Some(tokens) => Some(tokens),
            None => {
                if st.net_gen == generation {
                    st.net_on = false;
                }
                None
            }
        }
    };
    if let Some(tokens) = late {
        cdp::network_off(Some(&wv), tokens);
        cdp::network_disable(&wv).await;
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
/// Pane `label`'s page starts loading `url` (main frame). A page there that the agents its requests are kept for may not use ends the
/// list: what the person does on another site is not kept for agents.
pub(super) fn navigating(app: &AppHandle, label: &str, url: &str) {
    let Some(pane) = app.try_state::<AgentPanes>().and_then(|s| s.existing(label)) else { return };
    let leaves = {
        let st = pane.state.lock();
        st.net_on && st.net_scope.as_ref().is_some_and(|s| snap::site_allowed(url, s.kind, s.isolated, &s.origins, &s.sites).is_err())
    };
    if leaves {
        let (app, label) = (app.clone(), label.to_string());
        tauri::async_runtime::spawn(async move { network_stop(&app, &label, &pane) });
    }
}
/// Runs one tool of an agent in pane `label` and answers the daemon's request `preq`. An error means the daemon was not answered (the
/// window answers it then); never both.
#[tauri::command]
pub async fn browser_agent_run(app: AppHandle, preq: u64, label: String, tool: String, args: Value, ctx: Ctx) -> CmdResult<()> {
    valid_label(&label)?;
    let pending = app.state::<AgentPanes>().requests.lock().take(preq).ok_or("that request is not one Kelpie is waiting on")?;
    if pending.tool != tool || pending.session != ctx.session {
        return Err("the call does not match the agent's request".into());
    }
    let result = run(&app, &label, &tool, &args, &ctx, &pending).await;
    reply(&app, pending.conn, preq, result);
    Ok(())
}

async fn run(app: &AppHandle, label: &str, tool: &str, args: &Value, ctx: &Ctx, p: &Pending) -> Result<Value, String> {
    class_of(tool).ok_or_else(|| format!("unknown browser tool `{tool}`"))?;
    if !cdp::AVAILABLE && matches!(tool, "screenshot" | "network") {
        return Err(format!("kelpie_browser_{tool} is not available in this version of Kelpie yet."));
    }
    let wv = app.get_webview(label).ok_or("The pane's page is not open.")?;
    let nonce = app.state::<Browsers>().agent_nonce(label).ok_or("The pane's page is not open.")?;
    let pane = app.state::<AgentPanes>().pane(label);
    let left = p.deadline.saturating_duration_since(Instant::now());
    let Ok(_turn) = tokio::time::timeout(left, pane.turn.lock()).await else { return Err(BUSY.into()) };
    let call = Call { app, label, wv, nonce, pane: &pane, received: p.received, deadline: p.deadline, ctx, policy: &p.policy };
    may_start(call.left())?;
    call.alive()?;
    call.touch();
    pane.state.lock().net_scope = Some(Scope { kind: ctx.kind(), isolated: isolated(label), origins: ctx.origins.clone(), sites: p.policy.sites.clone() });
    call.record_network().await;
    let out = match tool {
        "snapshot" => call.snapshot(args).await,
        "click" | "type" | "press" => call.act(tool, args).await,
        "navigate" => call.navigate(args).await,
        "wait" => call.wait(args).await,
        "console" => call.console(args).await,
        "eval" => call.eval(args).await,
        "screenshot" => call.screenshot(args).await,
        "network" => call.network(args).await,
        _ => Err(format!("kelpie_browser_{tool} cannot run in a pane")),
    };
    call.touch();
    // Ended meanwhile: what the call found is not handed on.
    match out {
        Ok(_) => call.alive().and(out),
        out => out,
    }
}

// ---------- calls into the page ----------

/// Why a call into the page did not answer.
enum Fail {
    /// No answer in time.
    Timeout,
    /// No engine there, or the call did not run (a page between two documents, an error page, a script that does not parse).
    Absent,
    /// The engine's own error.
    Page(String),
}

impl Fail {
    fn text(self) -> String {
        match self {
            Fail::Timeout => NOT_RESPONDING.into(),
            Fail::Absent => UNREACHABLE.into(),
            Fail::Page(e) => e,
        }
    }
}

/// Runs `js` in the page and reads the engine's answer: the JSON string it returns, `{"ok": …}` or `{"error": "…"}`, within `limit`.
async fn page_raw(wv: &Webview, js: String, limit: Duration) -> Result<Value, Fail> {
    if limit.is_zero() {
        return Err(Fail::Timeout);
    }
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));
    wv.eval_with_callback(js, move |s| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(s);
        }
    })
    .map_err(|e| Fail::Page(e.to_string()))?;
    let s = tokio::time::timeout(limit, rx).await.map_err(|_| Fail::Timeout)?.map_err(|_| Fail::Absent)?;
    let Ok(Value::String(inner)) = serde_json::from_str::<Value>(&s) else { return Err(Fail::Absent) };
    let v: Value = serde_json::from_str(&inner).map_err(|_| Fail::Absent)?;
    if let Some(e) = v.get("error") {
        return Err(Fail::Page(e.as_str().unwrap_or("the page refused").to_string()));
    }
    Ok(v.get("ok").cloned().unwrap_or(Value::Null))
}

/// The page's state as the engine sees it.
#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct Info {
    doc: String,
    url: String,
    title: String,
    ready: String,
    /// Ms since the person's last input; -1 when none.
    person: i64,
    /// Ms since the page last changed after an action; -1 when not watched.
    quiet: i64,
    dialogs: Vec<Dialog>,
    /// What the page asked for during an agent's trusted input and did not get (`file`, `fullscreen`).
    refused: Vec<String>,
}

/// How an action went in: what the engine says it acted on, how (`trusted`, `synthetic`, `synthetic: <why>`), a note for the reply, and
/// whether a double click went as a single one.
struct Done {
    target: Value,
    how: String,
    note: Option<String>,
    single: bool,
}

/// Whether a webview's page has the keyboard focus (none when it does not say in time).
async fn has_focus(wv: &Webview) -> Option<bool> {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));
    wv.eval_with_callback("document.hasFocus()", move |s| {
        if let Some(tx) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = tx.send(s);
        }
    })
    .ok()?;
    let s = tokio::time::timeout(Duration::from_millis(200), rx).await.ok()?.ok()?;
    serde_json::from_str(&s).ok()
}

#[derive(Deserialize, Default, Clone, Debug)]
#[serde(default)]
struct Dialog {
    #[serde(rename = "type")]
    kind: String,
    message: String,
    answer: String,
}

/// What the engine says it acted on.
#[derive(Deserialize, Default, Debug)]
#[serde(default)]
struct Target {
    id: u64,
    role: String,
    name: String,
    picked: bool,
}

fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_default()
}

fn page_of(url: &str, title: &str) -> Value {
    json!({"title": title, "url": url})
}

fn host_of(url: &str) -> String {
    tauri::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default()
}

struct Call<'a> {
    app: &'a AppHandle,
    label: &'a str,
    wv: Webview,
    nonce: String,
    pane: &'a Arc<Pane>,
    received: Instant,
    deadline: Instant,
    ctx: &'a Ctx,
    policy: &'a Policy,
}

impl Call<'_> {
    /// Refused once agents' use of the pane ended (Stop, a phone, a shared page leaving its site, the pane closing) after the call came.
    fn alive(&self) -> Result<(), String> {
        match self.pane.state.lock().ended {
            Some((at, why)) if at > self.received => Err(ended_text(why).into()),
            _ => Ok(()),
        }
    }

    /// What is left of the call's time.
    fn left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// The pane is in use: its page keeps the normal memory target, and the idle watcher runs. Not for a pane that closed meanwhile.
    fn touch(&self) {
        let registered = self.app.state::<AgentPanes>().panes.lock().get(self.label).is_some_and(|p| Arc::ptr_eq(p, self.pane));
        if !registered {
            return;
        }
        let start = {
            let mut st = self.pane.state.lock();
            st.last_use = Some(Instant::now());
            if let Some(l) = st.lease.as_mut().filter(|l| l.session == self.ctx.session) {
                l.last = Instant::now();
            }
            !std::mem::replace(&mut st.watching, true)
        };
        crate::webview_memory::keep_normal(&self.wv, true);
        if start {
            tauri::async_runtime::spawn(idle_watch(self.app.clone(), self.label.to_string(), self.pane.clone()));
        }
    }

    /// One call into the page, within what is left of the call's time.
    async fn page(&self, func: &str, args: &Value) -> Result<Value, Fail> {
        page_raw(&self.wv, engine_js(&self.nonce, func, args), PAGE_CALL.min(self.left())).await
    }

    /// Calls engine function `func`; a read-only call that gets no answer is tried once more while there is time.
    async fn engine(&self, func: &str, args: Value, read: bool) -> Result<Value, String> {
        match self.page(func, &args).await {
            Ok(v) => Ok(v),
            Err(Fail::Page(e)) => Err(e),
            Err(_) if read && self.left() > Duration::from_secs(1) => {
                tokio::time::sleep(Duration::from_millis(300)).await;
                self.page(func, &args).await.map_err(Fail::text)
            }
            Err(f) => Err(f.text()),
        }
    }

    async fn info(&self, take_dialogs: bool) -> Result<Info, String> {
        let v = self.engine("info", json!({"take": take_dialogs}), true).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    /// `info`, without retrying or failing: for polling.
    async fn peek(&self) -> Option<Info> {
        self.page("info", &json!({})).await.ok().and_then(|v| serde_json::from_value(v).ok())
    }

    fn phone(&self) -> bool {
        self.app.state::<Takeovers>().pane_held(self.label)
    }

    /// Checks the call (`decide`) against the page now.
    async fn gate(&self, class: Class) -> Result<Info, String> {
        let info = self.info(false).await?;
        let lease = self.pane.state.lock().lease.as_ref().map(|l| (l.session.clone(), l.name.clone(), l.last.elapsed()));
        let facts = Facts {
            class,
            session: &self.ctx.session,
            phone: self.phone(),
            lease: lease.as_ref().map(|(s, n, idle)| (s.as_str(), n.as_str(), *idle)),
            person: u64::try_from(info.person).ok().map(Duration::from_millis),
            url: &info.url,
            isolated: isolated(self.label),
        };
        decide(&facts, self.ctx, self.policy)?;
        Ok(info)
    }

    /// The session takes or renews the act lease: once the call is sure to act.
    fn take_lease(&self) {
        let new = {
            let mut st = self.pane.state.lock();
            match st.lease.as_mut() {
                Some(l) if l.session == self.ctx.session => {
                    l.last = Instant::now();
                    false
                }
                _ => {
                    st.lease = Some(Lease { session: self.ctx.session.clone(), name: self.ctx.session_name.clone(), last: Instant::now() });
                    true
                }
            }
        };
        if new {
            lease_event(self.app, self.label, &self.ctx.session, &self.ctx.session_name, true, None);
        }
    }

    /// The page ended on `url`: if the pane may not be there, a shared pane loses its agents (`left-site`), and the call fails without
    /// naming the site.
    fn landed(&self, url: &str) -> Result<(), String> {
        let Err(why) = snap::site_allowed(url, self.ctx.kind(), isolated(self.label), &self.ctx.origins, &self.policy.sites) else { return Ok(()) };
        if self.ctx.kind() == Kind::Shared {
            end(self.app, self.label, "left-site");
            return Err(LEFT_SITE.into());
        }
        Err(why)
    }

    fn resolve(&self, r: &str, doc: &str) -> Result<u64, String> {
        self.pane.state.lock().refs.resolve(r, doc)
    }

    fn pane_id(&self) -> &str {
        self.label.strip_prefix("browser-").unwrap_or(self.label)
    }

    async fn take_snapshot(&self, mode: &str, root: Option<u64>, max: usize) -> Result<Snapshot, String> {
        let v = self.engine("snapshot", json!({"mode": mode, "root": root, "max": max}), true).await?;
        serde_json::from_value(v).map_err(|e| e.to_string())
    }

    async fn snapshot(&self, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Read).await?;
        let full = args["mode"].as_str() == Some("full");
        let max = args["max_chars"].as_u64().map_or(if full { 20_000 } else { 8_000 }, |m| m.clamp(200, 50_000) as usize);
        let root = args["root"].as_str().map(|r| self.resolve(r, &info.doc)).transpose()?;
        let s = self.take_snapshot(if full { "full" } else { "compact" }, root, max).await?;
        if root.is_some() && s.doc != info.doc {
            return Err(STALE.into());
        }
        self.landed(&s.url)?;
        let r = snap::render(&s, &mut self.pane.state.lock().refs, max, None);
        Ok(json!({
            "text": r.text,
            "data": {"pane": self.pane_id(), "url": s.url, "title": s.title, "truncated": r.truncated, "elements": r.elements},
            "page": page_of(&s.url, &s.title),
        }))
    }

    /// Waits for the page to settle after an action: a navigation it started to load (within 6 s), else its DOM to stay still for 300 ms
    /// (within 2 s), either within the call's time less what the snapshot after needs. The page's state then, dialogs included, and
    /// whether it finished loading.
    async fn settle(&self, navs_before: u64) -> (Option<Info>, bool) {
        let start = Instant::now();
        let browsers = self.app.state::<Browsers>();
        let room = self.left().saturating_sub(RESERVE);
        let mut loaded = false;
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let (started, finished) = browsers.navs(self.label);
            let navigating = started > navs_before;
            if let Some(i) = self.peek().await {
                if finished >= started && i.ready == "complete" && (navigating || i.quiet < 0 || i.quiet >= 300) {
                    loaded = true;
                    break;
                }
            }
            let limit = room.min(Duration::from_secs(if navigating { 6 } else { 2 }));
            if start.elapsed() >= limit || self.alive().is_err() {
                break;
            }
        }
        let now = self.page("info", &json!({"take": true})).await.ok().and_then(|v| serde_json::from_value(v).ok());
        (now, loaded)
    }

    /// An action, with the dialog answering it starts ended right after, whether it worked or not.
    async fn act(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let out = self.act_once(tool, args).await;
        dialogs_off(self.app, self.label, &self.wv);
        out
    }

    async fn act_once(&self, tool: &str, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Act).await?;
        let after = args["snapshot_after"].as_str().unwrap_or("diff");
        let r = args["ref"].as_str();
        let id = r.map(|r| self.resolve(r, &info.doc)).transpose()?;
        // A call refused for want of time takes no lease (no banner for an act that never happened).
        may_act(self.left())?;
        self.take_lease();
        // The page before, to compare with; only when there is time for both snapshots.
        let before = if after == "diff" && self.left() > ACT_NEED + Duration::from_secs(4) { self.take_snapshot("compact", None, DIFF_CHARS).await.ok() } else { None };
        let navs = self.app.state::<Browsers>().navs(self.label).0;
        let popups = self.app.state::<Browsers>().popups_of(self.label).len();
        let mut a = json!({"doc": info.doc, "id": id.unwrap_or(0), "op": tool, "dialog": args["dialog"].as_str().unwrap_or("dismiss"), "quiet": PERSON.as_millis() as u64});
        for k in ["button", "double", "text", "clear", "submit", "key", "modifiers"] {
            if let Some(v) = args.get(k) {
                a[k] = v.clone();
            }
        }
        may_act(self.left())?;
        self.alive()?;
        if self.phone() {
            return Err(PHONE.into());
        }
        let done = self.input(tool, a, args).await?;
        let target: Target = serde_json::from_value(done.target).unwrap_or_default();
        let how = done.how;
        let (now, loaded) = self.settle(navs).await;
        let now = now.unwrap_or_else(|| Info { url: self.wv.url().map(|u| u.to_string()).unwrap_or_default(), ..Info::default() });
        self.landed(&now.url)?;
        let target_ref = match (r, target.id) {
            (Some(r), _) => Some(r.to_string()),
            (None, 0) => None,
            (None, tid) => Some(self.pane.state.lock().refs.name(&info.doc, tid)),
        };
        let what = describe(&target, target_ref.as_deref());
        let mut text = match tool {
            "click" => {
                let verb = if args["button"].as_str() == Some("right") {
                    "Right-clicked"
                } else if args["double"].as_bool() == Some(true) && !done.single {
                    "Double-clicked"
                } else {
                    "Clicked"
                };
                format!("{verb} {what} ({how}).")
            }
            "type" if target.picked => format!("Picked the matching option in {what}."),
            "type" => {
                let n = args["text"].as_str().map_or(0, |t| t.chars().count());
                let submitted = if args["submit"].as_bool() == Some(true) { " Pressed Enter." } else { "" };
                format!("Typed {n} character{} into {what} ({how}).{submitted}", if n == 1 { "" } else { "s" })
            }
            _ => {
                let combo = key_combo(args);
                if what.is_empty() { format!("Pressed {combo} ({how}).") } else { format!("Pressed {combo} on {what} ({how}).") }
            }
        };
        if let Some(note) = &done.note {
            text.push(' ');
            text.push_str(note);
        }
        self.after_action(&mut text, &info, &now, loaded, popups);
        let audit = json!({"host": host_of(&info.url), "target": if what.is_empty() { key_combo(args) } else { format!("{} {}", target.role, quoted(&target.name)) }});
        // The field typed into never shows its value back.
        let mask = (tool == "type").then_some(id).flatten();
        match after {
            "diff" if before.is_some() && self.left() > Duration::from_secs(1) => self.diff_into(&mut text, before.as_ref(), mask).await,
            "diff" => text.push_str("\nTake a snapshot to see what changed."),
            "full" if self.left() > Duration::from_secs(1) => {
                if let Ok(s) = self.take_snapshot("compact", None, 8_000).await {
                    let r = snap::render(&s, &mut self.pane.state.lock().refs, 8_000, mask);
                    text.push_str(&format!("\nPage now:\n{}", r.text));
                }
            }
            _ => {}
        }
        Ok(json!({"text": text, "page": page_of(&now.url, &now.title), "audit": audit}))
    }

    /// Whether the computer shows the pane now, as far as is known here: the window does not hide it, and its window is not minimised. The
    /// window thread looks again right before each trusted event (`cdp`).
    fn shown(&self) -> bool {
        !self.app.state::<Takeovers>().hidden(self.label) && !self.wv.window().is_minimized().unwrap_or(false)
    }

    /// One DevTools protocol call within `limit` and the call's time. `guarded`: only while the computer shows the pane, no phone holds it
    /// and agents' use of it has not ended since the call came, as the window thread finds right before it (else `NOT_SHOWN`).
    async fn cdp(&self, method: &'static str, params: Value, guarded: bool, limit: Duration) -> Result<Value, String> {
        let hidden: Option<cdp::Hidden> = guarded.then(|| {
            let (app, label, pane, received) = (self.app.clone(), self.label.to_string(), self.pane.clone(), self.received);
            Box::new(move || {
                let t = app.state::<Takeovers>();
                t.hidden(&label) || t.pane_held(&label) || pane.state.lock().ended.is_some_and(|(at, _)| at > received)
            }) as cdp::Hidden
        });
        cdp::call(&self.wv, method, params, hidden, limit.min(self.left())).await
    }
    /// Keeps the pane's requests from the first agent call on; waits a little for that to start, so that what this call does is in them.
    async fn record_network(&self) {
        if !cdp::AVAILABLE || self.pane.state.lock().net_on {
            return;
        }
        let task = tauri::async_runtime::spawn(network_start(self.pane.clone(), self.wv.clone()));
        let _ = tokio::time::timeout(NET_START, task).await;
    }

    /// The engine's synthetic events do action `a`; `why` says why they and not trusted input did.
    async fn synthetic(&self, a: Value, why: Option<&str>) -> Result<Done, String> {
        let target = self.engine("act", a, false).await?;
        Ok(Done { target, how: why.map_or_else(|| "synthetic".to_string(), |w| format!("synthetic: {w}")), note: None, single: false })
    }

    /// Does action `a` (`act`'s arguments) of `tool`: as trusted input where the platform has it and it is safe, else as the engine's
    /// synthetic events. Trusted input needs the pane shown at 100 % zoom, an element of the top page, a left click, a key without
    /// Control, Alt or Meta, and a field that takes any text. Each step goes only while nothing ended the agent's use of the pane, with the
    /// page told the input is Kelpie's, on the same page and element: a press only where the element itself is, typing's keys and text only
    /// while the focus is in the field. A step refused before anything acted on the page lets the synthetic events do it; one refused
    /// because the page, the element or the focus changed stops the action, and the agent hears what went.
    async fn input(&self, tool: &str, a: Value, args: &Value) -> Result<Done, String> {
        let names: Vec<&str> = args["modifiers"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        let mods = cdp::modifiers(&names);
        let key = args["key"].as_str().unwrap_or_default();
        if !(cdp::AVAILABLE && self.shown()) {
            return self.synthetic(a, None).await;
        }
        // A trusted right click opens the browser's own menu on the person's screen.
        if tool == "click" && args["button"].as_str() == Some("right") {
            return self.synthetic(a, None).await;
        }
        if let Some(why) = (tool == "press").then(|| cdp::synthetic_key(key, mods)).flatten() {
            return self.synthetic(a, Some(why)).await;
        }
        if tool != "press" {
            match cdp::zoom_factor(&self.wv).await {
                Some(z) if (z - 1.0).abs() < 0.01 => {}
                Some(_) => return self.synthetic(a, Some("the pane is zoomed")).await,
                None => return self.synthetic(a, Some("Kelpie could not read the pane's zoom")).await,
            }
        }
        let mut at = a.clone();
        at["op"] = json!("locate");
        at["for"] = json!(tool);
        let spot = self.engine("act", at, false).await?;
        let kind = spot["kind"].as_str().unwrap_or_default().to_string();
        // A list's option is picked the engine's way, and a number, date or similar field is typed into it, which notices refused text.
        if matches!(kind.as_str(), "select" | "strict") {
            return self.synthetic(a, None).await;
        }
        let point = spot["x"].as_f64().zip(spot["y"].as_f64());
        let text = args["text"].as_str().unwrap_or_default();
        let (clear, submit, double) = (args["clear"].as_bool() != Some(false), args["submit"].as_bool() == Some(true), args["double"].as_bool() == Some(true));
        let steps = match (tool, point) {
            ("press", _) => cdp::key_steps(key, mods, cdp::Effect::Input).ok_or_else(|| format!("Kelpie cannot press the key {}", quoted(key)))?,
            ("click", Some((x, y))) => cdp::click_steps(x, y, false, double, mods, cdp::Effect::Input),
            ("type", Some((x, y))) => cdp::type_steps(x, y, text, clear, submit),
            // No point on screen to press.
            _ => {
                let why = match spot["why"].as_str() {
                    Some("zoomed") => Some("the page is zoomed in"),
                    Some("frame") => Some("the element is in a frame"),
                    _ => None,
                };
                return self.synthetic(a, why).await;
            }
        };
        may_act(self.left())?;
        self.alive()?;
        if self.phone() {
            return Err(PHONE.into());
        }
        let (id, doc) = (a["id"].clone(), a["doc"].clone());
        // A page that cannot be told the input is Kelpie's would take it for the person's.
        if !self.engine("busy", json!({"ms": BUSY_MS, "doc": doc, "id": id}), false).await.is_ok_and(|v| v["hit"] != false) {
            let _ = self.engine("busy", json!({"ms": 0}), false).await;
            return self.synthetic(a, None).await;
        }
        let main_focused = self.main_focus().await;
        let measure = tool == "type" && matches!(kind.as_str(), "field" | "lines");
        // Keys go to the focused element: for typing, and for a key pressed on an element, it must be the one.
        let in_focus = tool == "type" || (tool == "press" && id.as_u64().is_some_and(|i| i > 0));
        let (stop, length) = (Mutex::new((String::new(), String::new())), Mutex::new(None::<u64>));
        let sent = cdp::send(&steps, |method, params, guarded| {
            let press = params["type"] == "mousePressed";
            let keys = method != "Input.dispatchMouseEvent";
            let enter = measure && params["key"] == "Enter" && params["type"] == "keyDown";
            let (id, doc, stop, length) = (&id, &doc, &stop, &length);
            async move {
                if guarded {
                    let mut b = json!({"ms": BUSY_MS, "doc": doc, "id": id});
                    if press {
                        b["x"] = params["x"].clone();
                        b["y"] = params["y"].clone();
                    } else if keys && in_focus {
                        b["focus"] = json!(true);
                    }
                    match self.engine("busy", b, false).await {
                        Ok(v) if v["hit"] == false => {
                            let why = v["why"].as_str().unwrap_or("something changed").to_string();
                            *stop.lock() = (v["kind"].as_str().unwrap_or("page").to_string(), why);
                            return Err(cdp::MISSED.to_string());
                        }
                        Ok(_) => {}
                        Err(_) => return Err(cdp::NOT_SENT.to_string()),
                    }
                    // The text is in: what the field holds before Enter submits it.
                    if enter {
                        let n = self.engine("length", json!({"doc": doc, "id": id}), false).await.ok().and_then(|v| v.as_u64());
                        *length.lock() = n;
                    }
                }
                self.cdp(method, params, guarded, PAGE_CALL).await
            }
        })
        .await;
        let _ = self.engine("busy", json!({"ms": 0}), false).await;
        self.restore_focus(main_focused).await;
        let trusted = |note: Option<String>, single: bool| Done { target: spot.clone(), how: "trusted".into(), note, single };
        match sent {
            cdp::Sent::All => {
                let mut note = None;
                if measure {
                    // Read before Enter submitted it, else now (a submitted field that was not read is not judged).
                    let before_enter = *length.lock();
                    let held = match before_enter {
                        Some(n) => Some(n),
                        None if submit => None,
                        None => self.engine("length", json!({"doc": doc, "id": id}), false).await.ok().and_then(|v| v.as_u64()),
                    };
                    let typed = cdp::typed_len(text, kind == "lines");
                    let want = if clear { typed } else { spot["len"].as_u64().unwrap_or(0) + typed };
                    if let Some(n) = held.filter(|n| *n != want) {
                        note = Some(format!("The field now holds {n} characters, not {want}: it refused or changed some of the text."));
                    }
                }
                Ok(trusted(note, false))
            }
            cdp::Sent::Failed { at, error } if cdp::acted(&steps, at) => Err(format!("{error}; part of the action may have happened: take a snapshot to see the page now.")),
            cdp::Sent::Failed { error, .. } => Err(error),
            cdp::Sent::Refused { at, why } => {
                // A phone took the pane, or agents' use of it ended: nothing more is done.
                if self.phone() {
                    return Err(PHONE.into());
                }
                self.alive()?;
                let stopped = stop.lock().clone();
                let (changed, reason) = match why.as_str() {
                    cdp::MISSED => stopped,
                    NOT_SHOWN => ("hidden".to_string(), "the pane was hidden".to_string()),
                    _ => ("silent".to_string(), "the page did not answer".to_string()),
                };
                // A double click whose second press did not go: the first click did.
                let presses: Vec<usize> = steps.iter().enumerate().filter(|(_, s)| s.params["type"] == "mousePressed").map(|(i, _)| i).collect();
                if double && presses.get(1) == Some(&at) {
                    return Ok(trusted(Some(format!("Only the first click went: {reason} before the second.")), true));
                }
                // While no more than the preparation for typing went on the same page, the synthetic events do the action on the element
                // itself; otherwise the agent hears what went, and nothing more is done.
                match cdp::after_refusal(&steps, at, &changed) {
                    Fallback::Action => self.synthetic(a, Some(&reason)).await,
                    Fallback::Submit => {
                        let mut enter = a;
                        enter["op"] = json!("press");
                        enter["key"] = json!("Enter");
                        enter["modifiers"] = json!([]);
                        self.engine("act", enter, false).await?;
                        Ok(Done { target: spot.clone(), how: format!("trusted, then Enter synthetic: {reason}"), note: None, single: false })
                    }
                    Fallback::Nothing => Err(cdp::stopped_text(tool, &steps, at, &reason)),
                }
            }
        }
    }

    /// Whether the main window's own page (Kelpie's, not a web page) has the keyboard focus now; none when it does not say in time.
    async fn main_focus(&self) -> Option<bool> {
        has_focus(&self.app.get_webview("main")?).await
    }

    /// Trusted input may move the keyboard focus into the pane. The main window's page gets it back when it had it before the run
    /// (`before`), lost it, and the Kelpie window is still the one in front. Focus never goes to a pane: a web page's answer is not trusted.
    async fn restore_focus(&self, before: Option<bool>) {
        if before != Some(true) || !self.wv.window().is_focused().unwrap_or(false) || self.main_focus().await != Some(false) {
            return;
        }
        if let Some(main) = self.app.get_webview("main") {
            let _ = main.set_focus();
        }
    }
    /// The lines an action adds about the page: where it is now, dialogs it answered, a window that opened.
    fn after_action(&self, text: &mut String, before: &Info, now: &Info, loaded: bool, popups: usize) {
        if now.url != before.url || now.title != before.title || !loaded {
            text.push_str(&format!("\nNow at {} {}{}.", quoted(&now.title), now.url, if loaded { "" } else { " (still loading)" }));
        }
        for d in &now.dialogs {
            text.push_str(&format!("\nA dialog ({}) said {}; it was {}.", d.kind, quoted(&d.message), d.answer));
        }
        for what in &now.refused {
            text.push_str(&format!("\nThe page asked for {}; agents cannot use that.", if what == "file" { "a file" } else { "fullscreen" }));
        }
        let open = self.app.state::<Browsers>().popups_of(self.label);
        if open.len() > popups {
            let url = open.last().and_then(|p| self.app.get_webview_window(p)).and_then(|w| w.url().ok()).map(|u| snap::scrub_url(u.as_str())).unwrap_or_default();
            text.push_str(&format!("\nA window opened: {url} (a sign-in window; agents cannot use it)."));
        }
    }

    async fn diff_into(&self, text: &mut String, before: Option<&Snapshot>, mask: Option<u64>) {
        let Some(before) = before else { return };
        let Ok(after) = self.take_snapshot("compact", None, DIFF_CHARS).await else {
            text.push_str("\nTake a snapshot to see the page now.");
            return;
        };
        let changes = {
            let mut st = self.pane.state.lock();
            let new = snap::lines(&after, &mut st.refs, mask);
            if after.doc != before.doc {
                let mut out: Vec<String> = new.iter().take(DIFF_LINES).map(|l| format!("+ {l}")).collect();
                if new.len() > DIFF_LINES {
                    out.push(format!("… {} more (take a snapshot)", new.len() - DIFF_LINES));
                }
                text.push_str("\nA new page loaded:");
                out
            } else {
                let old = snap::lines(before, &mut st.refs, mask);
                snap::diff(&old, &new, DIFF_LINES)
            }
        };
        if changes.is_empty() {
            text.push_str("\nNo visible change.");
        } else {
            if after.doc == before.doc {
                text.push_str("\nChanges:");
            }
            for l in changes {
                text.push('\n');
                text.push_str(&l);
            }
        }
    }

    /// When a wait or navigation of `timeout_s` must end: its own timeout from the request's arrival, within the call's time.
    fn end_of(&self, timeout: Duration) -> Instant {
        (self.received + timeout).min(self.deadline)
    }

    async fn navigate(&self, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Act).await?;
        let timeout = Duration::from_secs(args["timeout_s"].as_u64().unwrap_or(15).clamp(1, 25));
        let browsers = self.app.state::<Browsers>();
        let action = args["action"].as_str();
        let url = args["url"].as_str().map(super::parse_url).transpose()?;
        if let Some(url) = &url {
            snap::site_allowed(url.as_str(), self.ctx.kind(), isolated(self.label), &self.ctx.origins, &self.policy.sites)?;
        } else if !matches!(action, Some("back" | "forward" | "reload")) {
            return Err("give either `url` or `action` (back, forward or reload)".into());
        }
        may_act(self.left())?;
        self.take_lease();
        self.alive()?;
        let navs = browsers.navs(self.label).0;
        let target = match (url, action) {
            (Some(url), _) => {
                let host = url.host_str().unwrap_or_default().to_string();
                self.wv.navigate(url).map_err(|e| e.to_string())?;
                host
            }
            (None, Some("reload")) => {
                self.wv.reload().map_err(|e| e.to_string())?;
                "reload".into()
            }
            // Through the history functions the engine took at document start: the page cannot replace them.
            (None, Some(a)) => {
                self.engine("go", json!({"delta": if a == "back" { -1 } else { 1 }}), false).await?;
                a.to_string()
            }
            (None, None) => unreachable!("checked above"),
        };
        let start = Instant::now();
        let end = self.end_of(timeout);
        let (mut now, mut loaded, mut misses) = (None::<Info>, false, 0);
        loop {
            tokio::time::sleep(POLL).await;
            self.alive()?;
            let (started, finished) = browsers.navs(self.label);
            let peeked = self.peek().await;
            misses = if peeked.is_some() { 0 } else { misses + 1 };
            if let Some(i) = peeked {
                let moved = started > navs || i.doc != info.doc || i.url != info.url;
                if moved && finished >= started && i.ready == "complete" {
                    now = Some(i);
                    loaded = true;
                    break;
                }
                if !moved && matches!(action, Some("back" | "forward")) && start.elapsed() >= Duration::from_secs(2) {
                    return Ok(json!({
                        "text": format!("There is no page to go {} to; still at {} {}.", action.unwrap_or_default(), quoted(&i.title), i.url),
                        "page": page_of(&i.url, &i.title),
                        "audit": {"host": host_of(&i.url), "target": target},
                    }));
                }
                now = Some(i);
            }
            // A page that failed to load shows an error page the engine does not run in.
            if (started > navs && finished >= started && misses >= 4) || Instant::now() >= end {
                break;
            }
        }
        let (url, title) = match &now {
            Some(i) if loaded => (i.url.clone(), i.title.clone()),
            _ => (self.wv.url().map(|u| u.to_string()).unwrap_or_default(), now.as_ref().map(|i| i.title.clone()).unwrap_or_default()),
        };
        self.landed(&url)?;
        let state = if loaded { "loaded" } else if misses >= 4 { "the page did not load" } else { "still loading" };
        Ok(json!({
            "text": format!("Now at {} {url} ({state}).", quoted(&title)),
            "page": page_of(&url, &title),
            "audit": {"host": host_of(&url), "target": target},
        }))
    }

    async fn wait(&self, args: &Value) -> Result<Value, String> {
        self.gate(Class::Read).await?;
        let timeout = Duration::from_secs(args["timeout_s"].as_u64().unwrap_or(10).clamp(1, 30));
        let mut cond = json!({});
        for k in ["text", "text_gone", "selector", "url_contains", "load"] {
            if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
                cond[k] = v.clone();
            }
        }
        let start = Instant::now();
        let end = self.end_of(timeout);
        let browsers = self.app.state::<Browsers>();
        let (mut url, mut title) = (String::new(), String::new());
        loop {
            self.alive()?;
            match self.page("check", &cond).await {
                Ok(v) => {
                    url = v["url"].as_str().unwrap_or_default().to_string();
                    title = v["title"].as_str().unwrap_or_default().to_string();
                    let (started, finished) = browsers.navs(self.label);
                    let settled = cond.get("load").is_none() || finished >= started;
                    if v["met"].as_bool() == Some(true) && settled {
                        self.landed(&url)?;
                        let secs = start.elapsed().as_secs_f64();
                        return Ok(json!({"text": format!("Met after {secs:.1} s."), "page": page_of(&url, &title)}));
                    }
                }
                Err(Fail::Page(e)) => return Err(e),
                Err(_) => {}
            }
            if Instant::now() + POLL >= end {
                break;
            }
            tokio::time::sleep(POLL).await;
        }
        if url.is_empty() {
            url = self.wv.url().map(|u| u.to_string()).unwrap_or_default();
        }
        // A page on a site agents may not use here is not named.
        self.landed(&url).map_err(|e| format!("Not met within {} s. {e}", timeout.as_secs()))?;
        Err(format!("Not met within {} s; page now {} {url}", timeout.as_secs(), quoted(&title)))
    }

    async fn console(&self, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Read).await?;
        let mut a = json!({"level": args["level"].as_str().unwrap_or("all"), "max": args["max"].as_u64().unwrap_or(50).clamp(1, 300)});
        if let Some(s) = args["since"].as_u64() {
            a["since"] = json!(s);
        }
        let v = self.engine("console", a, true).await?;
        let mut lines: Vec<String> = Vec::new();
        if v["sinceAgent"].as_bool() == Some(true) {
            lines.push("(This pane was not opened for an agent: its console is kept only since an agent first used this page, page errors from the start.)".into());
        }
        if let Some(n) = v["dropped"].as_u64().filter(|n| *n > 0) {
            lines.push(format!("({n} earlier messages were dropped: the page keeps the last 300.)"));
        }
        let mut any = false;
        for e in v["entries"].as_array().into_iter().flatten() {
            let s = |k: &str| e[k].as_str().unwrap_or_default().to_string();
            let src = s("src");
            lines.push(format!("[{}] {} {}{}", s("level"), s("time"), s("text").replace(['\r', '\n'], " "), if src.is_empty() { String::new() } else { format!(" ({src})") }));
            any = true;
        }
        if !any {
            lines.push("No console messages.".into());
        }
        let next = v["next"].as_u64().unwrap_or(0);
        if let Some(m) = v["more"].as_u64().filter(|m| *m > 0) {
            lines.push(format!("… {m} more: call again with since={next}"));
        }
        lines.push(format!("next cursor: {next}"));
        Ok(json!({"text": lines.join("\n"), "page": page_of(&info.url, &info.title)}))
    }

    /// A picture of the visible page, of the element `ref`, or of the whole page (`full_page`, at most 8000 px high): at most 1280 px wide,
    /// a PNG, or a JPEG (no longer than 1.5 MB as base64) when the PNG is large; saved under `browser-shots/agent`. Only of a pane the
    /// computer shows, and only while the page stays the one checked: a page that changed meanwhile is not handed on.
    async fn screenshot(&self, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Read).await?;
        if !self.shown() {
            return Err(self.not_shown());
        }
        let r = args["ref"].as_str();
        let id = r.map(|r| self.resolve(r, &info.doc)).transpose()?;
        let full = args["full_page"].as_bool() == Some(true) && id.is_none();
        let g = self.engine("geometry", json!({"doc": info.doc, "id": id}), true).await?;
        let g: cdp::Geometry = serde_json::from_value(g).map_err(|e| e.to_string())?;
        let mut shot = cdp::plan_shot(&g, full);
        let unreadable = || "the page sent a picture Kelpie cannot read".to_string();
        let mut png = self.capture(&shot, None).await?;
        let (mut width, mut height) = cdp::png_size(&png).ok_or_else(unreadable)?;
        // The page draws at a pixel ratio it may not have told: a picture still too wide is taken again, smaller.
        if f64::from(width) > cdp::MAX_WIDTH + 0.5 {
            shot.scale *= cdp::MAX_WIDTH / f64::from(width);
            png = self.capture(&shot, None).await?;
            (width, height) = cdp::png_size(&png).ok_or_else(unreadable)?;
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let (mime, ext, data, bytes) = if cdp::jpeg_instead(b64.len()) {
            let mut found = None;
            for (quality, by) in cdp::JPEG_TRIES {
                let smaller = cdp::Shot { scale: shot.scale * by, ..shot.clone() };
                let jpeg = self.capture(&smaller, Some(quality)).await?;
                let data = base64::engine::general_purpose::STANDARD.encode(&jpeg);
                if !cdp::jpeg_instead(data.len()) {
                    (width, height) = cdp::jpeg_size(&jpeg).ok_or_else(unreadable)?;
                    found = Some((data, jpeg));
                    break;
                }
            }
            let (data, jpeg) = found.ok_or("The picture is too large even as a small JPEG; picture one element (ref) or the visible page instead.")?;
            ("image/jpeg", "jpg", data, jpeg)
        } else {
            ("image/png", "png", b64, png)
        };
        // The page may have gone elsewhere while it was pictured.
        let now = self.info(false).await?;
        if now.doc != info.doc {
            return Err("The page changed while it was pictured; take the screenshot again.".into());
        }
        self.landed(&now.url)?;
        let path = tauri::async_runtime::spawn_blocking(move || super::save_shot(Some("agent"), &bytes, ext)).await.map_err(|e| e.to_string())??;
        let what = match r {
            Some(r) => format!("Screenshot of [{r}]"),
            None if full => "Screenshot of the whole page".into(),
            None => "Screenshot of the visible page".into(),
        };
        let mut text = format!("{what}{}.", if shot.cut { ", cut at 8000 px high" } else { "" });
        if mime == "image/jpeg" {
            text.push_str(" A JPEG: the PNG was over 1.5 MB.");
        }
        Ok(json!({
            "text": text,
            "image": {"mime": mime, "data": data, "path": path.to_string_lossy(), "width": width, "height": height},
            "page": page_of(&now.url, &now.title),
        }))
    }

    /// Why the pane cannot be used for a picture or trusted input now: a phone holds it, agents' use of it ended, its window is minimised, or
    /// it is a background tab.
    fn not_shown(&self) -> String {
        if self.phone() {
            return PHONE.into();
        }
        if let Err(e) = self.alive() {
            return e;
        }
        if self.wv.window().is_minimized().unwrap_or(false) {
            return cdp::SHOT_MINIMISED.into();
        }
        cdp::SHOT_HIDDEN.into()
    }

    /// The picture `shot` (a PNG, or a JPEG of quality `jpeg`), within the call's time; refused when the pane cannot be pictured.
    async fn capture(&self, shot: &cdp::Shot, jpeg: Option<u8>) -> Result<Vec<u8>, String> {
        let v = self.cdp("Page.captureScreenshot", shot.params(jpeg), true, self.left()).await.map_err(|e| if e == NOT_SHOWN { self.not_shown() } else { e })?;
        base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().ok_or("the page sent no picture")?).map_err(|e| e.to_string())
    }

    /// The requests kept since an agent first used the pane (`kelpie_browser_network`): only those made by a page on a site this call may
    /// use (a page elsewhere refuses the call at `gate`).
    async fn network(&self, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Read).await?;
        let log = {
            let st = self.pane.state.lock();
            st.net_on.then(|| st.net.clone())
        };
        let log = log.ok_or("Kelpie could not keep this page's requests; try again.")?;
        let q = cdp::NetQuery {
            filter: args["filter"].as_str(),
            failed_only: args["failed_only"].as_bool() == Some(true),
            since: args["since"].as_u64().unwrap_or(0),
            max: args["max"].as_u64().unwrap_or(50).clamp(1, 300) as usize,
        };
        let origin = tauri::Url::parse(&info.url).map(|u| u.origin().ascii_serialization()).unwrap_or_default();
        let own = isolated(self.label);
        let allowed = |doc: &str| snap::site_allowed(doc, self.ctx.kind(), own, &self.ctx.origins, &self.policy.sites).is_ok();
        let text = log.lock().render(&q, &origin, &allowed);
        Ok(json!({"text": text, "page": page_of(&info.url, &info.title)}))
    }
    async fn eval(&self, args: &Value) -> Result<Value, String> {
        let info = self.gate(Class::Eval).await?;
        let expr = args["expression"].as_str().ok_or("missing `expression`")?;
        may_act(self.left())?;
        self.take_lease();
        let v = page_raw(&self.wv, eval_js(expr), PAGE_CALL.min(self.left())).await.map_err(|f| match f {
            Fail::Absent => "The expression did not run: it is not one valid JavaScript expression, or the page is between two documents.".to_string(),
            f => f.text(),
        })?;
        let mut text = v.as_str().unwrap_or("null").to_string();
        if text.len() > EVAL_MAX {
            let total = text.len();
            let mut end = EVAL_MAX;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str(&format!("\n… cut: the value's JSON is {} KB; at most 8 KB is returned.", total.div_ceil(1024)));
        }
        let now = self.peek().await.unwrap_or(info);
        self.landed(&now.url)?;
        Ok(json!({"text": text, "page": page_of(&now.url, &now.title), "audit": {"host": host_of(&now.url), "target": "expression"}}))
    }
}

/// `button "Sign in" [e6]`; empty when the action had no element.
fn describe(t: &Target, r: Option<&str>) -> String {
    if t.role.is_empty() {
        return String::new();
    }
    let mut s = t.role.clone();
    if !t.name.is_empty() {
        s.push(' ');
        s.push_str(&quoted(&t.name));
    }
    if let Some(r) = r {
        s.push_str(&format!(" [{r}]"));
    }
    s
}

/// `Control+Enter`; a single character key is not repeated unless it is a shortcut (it may be part of typed text).
fn key_combo(args: &Value) -> String {
    let key = args["key"].as_str().unwrap_or_default();
    let mods: Vec<&str> = args["modifiers"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let shortcut = mods.iter().any(|m| matches!(*m, "Control" | "Meta" | "Alt"));
    let shown = if key.chars().count() == 1 && !shortcut { "a character key" } else { key };
    mods.into_iter().chain(std::iter::once(shown)).collect::<Vec<_>>().join("+")
}

/// Ends the lease of an idle pane and lets its page be trimmed again: 20 s after its last call, unless a call runs. Decided and done under
/// the pane's lock, so a call that starts meanwhile is never left without its memory pin.
async fn idle_watch(app: AppHandle, label: String, pane: Arc<Pane>) {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let mut st = pane.state.lock();
        let idle = st.last_use.is_none_or(|t| t.elapsed() >= IDLE);
        if !idle || pane.turn.try_lock().is_err() {
            continue;
        }
        st.watching = false;
        if let Some(l) = st.lease.take() {
            lease_event(&app, &label, &l.session, &l.name, false, Some("idle"));
        }
        if let Some(wv) = app.get_webview(&label) {
            crate::webview_memory::keep_normal(&wv, false);
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_expression_runs_without_the_engine_nonce() {
        let js = eval_js("document.title");
        assert!(js.contains("\ndocument.title\n") && js.contains("k.value(v)") && js.contains("k.value(e,true)"));
        // Nothing in the script names a nonce: the expression could read the source of the script that called it.
        let engine = include_str!("../browser_agent.js");
        assert!(engine.contains("value: (v, thrown) => value(v, thrown)"));
        assert!(!js.contains("NONCE") && !js.contains('"'));
    }

    fn ctx(kind: &str) -> Ctx {
        Ctx { session: "s1".into(), session_name: "Claude".into(), kind: kind.into(), origins: vec!["https://app.example.com".into()], may_eval: true }
    }

    fn act_policy() -> Policy {
        Policy { mode: "act".into(), eval: true, sites: vec![] }
    }

    fn facts(class: Class) -> Facts<'static> {
        Facts { class, session: "s1", phone: false, lease: None, person: None, url: "http://localhost:5173/", isolated: false }
    }

    fn pending(tool: &str, session: &str, received: Instant) -> Pending {
        Pending { tool: tool.into(), session: session.into(), policy: act_policy(), received, deadline: received + budget(tool, &json!({})), conn: 0 }
    }

    #[test]
    fn a_phone_holding_the_pane_refuses_every_tool() {
        for class in [Class::Read, Class::Act, Class::Eval] {
            let f = Facts { phone: true, ..facts(class) };
            assert_eq!(decide(&f, &ctx("opened"), &act_policy()), Err(PHONE.to_string()));
        }
    }

    #[test]
    fn another_session_s_lease_refuses_acting_until_it_is_idle() {
        let busy = Facts { lease: Some(("s2", "Codex", Duration::from_secs(3))), ..facts(Class::Act) };
        let e = decide(&busy, &ctx("opened"), &act_policy()).unwrap_err();
        assert!(e.contains("Codex") && e.contains("20 s"), "{e}");
        // Reading is not acting.
        assert_eq!(decide(&Facts { class: Class::Read, ..busy }, &ctx("opened"), &act_policy()), Ok(()));
        // The holder itself, or a lease idle for 20 s, lets the call through.
        assert_eq!(decide(&Facts { lease: Some(("s1", "Claude", Duration::from_secs(1))), ..facts(Class::Act) }, &ctx("opened"), &act_policy()), Ok(()));
        assert_eq!(decide(&Facts { lease: Some(("s2", "Codex", IDLE)), ..facts(Class::Act) }, &ctx("opened"), &act_policy()), Ok(()));
    }

    #[test]
    fn the_person_s_input_holds_acting_back_for_ten_seconds() {
        let recent = Facts { person: Some(Duration::from_secs(4)), ..facts(Class::Act) };
        assert!(decide(&recent, &ctx("opened"), &act_policy()).unwrap_err().contains("4 s ago"));
        assert!(decide(&Facts { class: Class::Eval, ..recent }, &ctx("opened"), &act_policy()).is_err());
        assert_eq!(decide(&Facts { class: Class::Read, person: Some(Duration::from_secs(1)), ..facts(Class::Read) }, &ctx("opened"), &act_policy()), Ok(()));
        assert_eq!(decide(&Facts { person: Some(PERSON), ..facts(Class::Act) }, &ctx("opened"), &act_policy()), Ok(()));
    }

    #[test]
    fn the_page_s_site_is_checked_for_reading_and_acting() {
        let away = Facts { url: "https://elsewhere.example.org/", ..facts(Class::Read) };
        let e = decide(&away, &ctx("shared"), &act_policy()).unwrap_err();
        assert!(e.contains("shared on https://app.example.com") && !e.contains("elsewhere"), "{e}");
        assert!(decide(&Facts { class: Class::Act, ..away }, &ctx("opened"), &act_policy()).is_err());
        let on_site = Facts { url: "https://app.example.com/home", ..facts(Class::Act) };
        assert_eq!(decide(&on_site, &ctx("shared"), &act_policy()), Ok(()));
        // An opened pane: the allowlist, or any site with a profile of its own.
        let allowed = Policy { sites: vec!["https://elsewhere.example.org".into()], ..act_policy() };
        assert_eq!(decide(&Facts { class: Class::Act, ..away }, &ctx("opened"), &allowed), Ok(()));
        assert_eq!(decide(&Facts { class: Class::Act, isolated: true, ..away }, &ctx("opened"), &act_policy()), Ok(()));
        // A shared pane does not widen with a profile of its own.
        assert!(decide(&Facts { class: Class::Act, isolated: true, ..away }, &ctx("shared"), &act_policy()).is_err());
    }

    #[test]
    fn reading_only_and_scripts_follow_the_policy_and_the_pane() {
        let read_only = Policy { mode: "read".into(), ..act_policy() };
        assert_eq!(decide(&facts(Class::Act), &ctx("opened"), &read_only), Err(READ_ONLY.to_string()));
        assert_eq!(decide(&facts(Class::Read), &ctx("opened"), &read_only), Ok(()));
        assert_eq!(decide(&facts(Class::Eval), &ctx("opened"), &act_policy()), Ok(()));
        // Scripts: never in a shared pane, nor without the user's switch or the window's word.
        assert_eq!(decide(&facts(Class::Eval), &ctx("shared"), &act_policy()), Err(EVAL_OFF.to_string()));
        assert_eq!(decide(&facts(Class::Eval), &ctx("opened"), &Policy { eval: false, ..act_policy() }), Err(EVAL_OFF.to_string()));
        assert_eq!(decide(&facts(Class::Eval), &Ctx { may_eval: false, ..ctx("opened") }, &act_policy()), Err(EVAL_OFF.to_string()));
    }

    #[test]
    fn a_call_keeps_within_the_daemon_s_wait() {
        assert_eq!(budget("click", &json!({})), Duration::from_secs(13));
        assert_eq!(budget("snapshot", &json!({"timeout_s": 25})), Duration::from_secs(13));
        assert_eq!(budget("wait", &json!({})), Duration::from_secs(13));
        assert_eq!(budget("wait", &json!({"timeout_s": 30})), Duration::from_secs(33));
        assert_eq!(budget("navigate", &json!({"timeout_s": 25})), Duration::from_secs(28));
        assert_eq!(budget("navigate", &json!({"timeout_s": 99})), Duration::from_secs(28));
        // A call that waited behind another until too little was left does not start, and an action needs a little more.
        assert_eq!(may_start(Duration::from_millis(1500)), Err(BUSY.to_string()));
        assert_eq!(may_start(MIN_START), Ok(()));
        assert_eq!(may_act(Duration::from_millis(2500)), Err(NO_TIME.to_string()));
        assert_eq!(may_act(ACT_NEED), Ok(()));
    }

    #[test]
    fn requests_are_kept_by_number_and_the_newest_wins() {
        let mut r = Requests::default();
        let t0 = Instant::now();
        r.insert(7, pending("click", "old", t0));
        // The daemon restarted and numbers from the start again: the newer request replaces the record.
        r.insert(7, pending("snapshot", "new", t0 + Duration::from_secs(1)));
        let p = r.take(7).unwrap();
        assert_eq!((p.tool.as_str(), p.session.as_str()), ("snapshot", "new"));
        assert!(r.take(7).is_none());
        // Answered by the window, or the daemon gone: nothing stays behind.
        r.insert(1, pending("tabs", "s", t0));
        r.insert(2, pending("click", "s", t0));
        assert!(r.take(1).is_some());
        r.clear();
        assert!(r.map.is_empty());
        // Bounded: the oldest goes first.
        for i in 0..PENDING as u64 + 5 {
            r.insert(i, pending("click", "s", t0 + Duration::from_millis(i)));
        }
        assert_eq!(r.map.len(), PENDING);
        assert!(r.take(0).is_none() && r.take(PENDING as u64 + 4).is_some());
    }

    #[test]
    fn tools_have_their_classes() {
        assert_eq!(class_of("snapshot"), Some(Class::Read));
        assert_eq!(class_of("navigate"), Some(Class::Act));
        assert_eq!(class_of("eval"), Some(Class::Eval));
        assert_eq!(class_of("tabs"), None);
    }

    #[test]
    fn an_end_says_why() {
        assert_eq!(ended_text("phone"), PHONE);
        assert_eq!(ended_text("left-site"), LEFT_SITE);
        assert_eq!(ended_text("closed"), CLOSED);
        assert_eq!(ended_text("stopped"), STOPPED);
    }

    #[test]
    fn engine_calls_carry_the_nonce() {
        assert_eq!(engine_js("ab12", "info", &json!({"take": true})), r#"(window.__kelpieAgent?window.__kelpieAgent.info("ab12",{"take":true}):null)"#);
    }

    #[test]
    fn a_pressed_character_is_not_repeated_unless_it_is_a_shortcut() {
        assert_eq!(key_combo(&json!({"key": "Enter", "modifiers": ["Control"]})), "Control+Enter");
        assert_eq!(key_combo(&json!({"key": "x"})), "a character key");
        assert_eq!(key_combo(&json!({"key": "a", "modifiers": ["Control"]})), "Control+a");
        assert_eq!(key_combo(&json!({"key": "A", "modifiers": ["Shift"]})), "Shift+a character key");
    }

    #[test]
    fn the_window_s_context_reads_as_it_sends_it() {
        let c: Ctx = serde_json::from_value(json!({"session": "s", "sessionName": "N", "kind": "opened", "origins": [], "sites": ["https://x.example"], "mayEval": true, "project": "p", "isolated": true})).unwrap();
        assert_eq!((c.session_name.as_str(), c.may_eval, c.kind()), ("N", true, Kind::Opened));
        let p: Policy = serde_json::from_value(json!({"mode": "act", "eval": false, "sites": ["http://x"]})).unwrap();
        assert_eq!((p.mode.as_str(), p.sites.len()), ("act", 1));
    }
}
