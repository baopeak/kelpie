//! Running automations: the clock's, and the ones an event sets off (see `automations.rs` for the
//! model and the guards). Everything here is off the ticker: matching is cheap, and what a run does
//! (starting a session, typing into one, running a program, asking git) happens on its own thread.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::process::Stdio;

use super::*;
use crate::automations::{self, ActionKind, Event, EventKind, Firing, Heads, ShellPlan, FILE_VERSION, MIN_GAP_MS};

/// After the session or command an automation started goes quiet, events for it are still ignored for
/// this long: the watcher reports an agent's last edits a moment after its turn ended.
const QUIET_AFTER_MS: u64 = 5_000;
/// A command that runs longer than this is stopped, with everything it started.
const SHELL_TIMEOUT: Duration = Duration::from_secs(300);
/// After the output of a command ends its readers get this long to finish before they are left behind.
const READERS_GRACE: Duration = Duration::from_secs(3);
/// An event for an automation counts as possibly caused by another that fired this recently, whether or not
/// that one is still busy.
const CHAIN_WINDOW_MS: u64 = 120_000;
/// A saved prompt is not typed into a session something was typed into more recently than this: it may be
/// somebody's half-written message.
const QUICK_QUIET: Duration = Duration::from_secs(30);
/// Most output of a command that is kept (the end of it).
const OUTPUT_KEPT: usize = 16 * 1024;

#[derive(Default)]
struct Busy {
    /// Sessions event automations started or typed into, with the automation.
    sessions: HashMap<String, String>,
    /// Automation -> wall-clock ms before which its events are ignored.
    quiet_until: HashMap<String, u64>,
    /// Automations whose command is running.
    running: HashSet<String>,
    /// How many automations' work led to each automation's last run (see `Automations::on_event_chained`).
    depth: HashMap<String, u8>,
    /// When each automation last fired: what it does is a possible cause of events for a while after, busy or not.
    fired: HashMap<String, u64>,
}

pub(super) struct Runtime {
    busy: Mutex<Busy>,
    heads: Mutex<Heads>,
    /// Connections that said they are the Kelpie app (`auth` with `client: "app"`).
    app: Mutex<HashSet<u64>>,
    /// The least time between two event runs of one automation.
    gap_ms: u64,
    /// A look at the commit folders is under way (one at a time, whatever git does).
    polling: std::sync::atomic::AtomicBool,
}

impl Runtime {
    pub(super) fn new() -> Runtime {
        // A test hook, like KELPIE_NOTIFY_RESET_MARGIN_MS: the real gap is a minute.
        let gap_ms = std::env::var("KELPIE_AUTOMATION_GAP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(MIN_GAP_MS);
        Runtime { busy: Mutex::new(Busy::default()), heads: Mutex::new(Heads::default()), app: Mutex::new(HashSet::new()), gap_ms, polling: std::sync::atomic::AtomicBool::new(false) }
    }

    pub(super) fn drop_client(&self, id: u64) {
        self.app.lock().remove(&id);
    }
}

/// An automation as the app gets it: with the hash of its command, which the user approves, and whether that
/// is the hash that was approved.
fn shown(a: &Automation) -> Value {
    let mut v = serde_json::to_value(a).unwrap_or(Value::Null);
    if let (Some(hash), Some(obj)) = (a.command_hash(), v.as_object_mut()) {
        obj.insert("command_hash".into(), Value::String(hash));
        obj.insert("command_approved".into(), Value::Bool(a.approved()));
        // What the user is approving: the file that will run, and the files whose contents the approval covers.
        if let Some(sh) = a.shell.as_ref().filter(|_| a.action == ActionKind::Shell) {
            let program = automations::resolve_program(&a.cwd, &sh.program);
            obj.insert("command_program".into(), program.map_or(Value::Null, |p| Value::String(p.to_string_lossy().into_owned())));
            let files: Vec<String> = automations::covered_files(&a.cwd, &sh.program, &sh.argv).iter().map(|p| p.to_string_lossy().into_owned()).collect();
            obj.insert("command_files".into(), json!(files));
            // A program with no rule gets agent-chosen names as arguments: the user is asked to take that on.
            obj.insert("command_unvetted".into(), Value::Bool(automations::check_command(&a.cwd, &sh.program, &sh.argv).is_ok_and(|x| x.unvetted)));
        }
    }
    v
}

/// Waits for the command to end; past `limit` it is stopped with everything it started.
fn wait_command(child: &mut std::process::Child, tree: Option<&crate::proc_tree::ProcTree>, limit: Duration) -> Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) if started.elapsed() > limit => {
                match tree {
                    Some(t) => t.kill(),
                    None => {
                        let _ = child.kill();
                    }
                }
                let _ = child.wait();
                return Err(anyhow!("stopped after {} s", limit.as_secs().max(1)));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(anyhow!("{e}")),
        }
    }
}

/// Waits for a reader thread, but not for ever.
fn join_by(h: Option<std::thread::JoinHandle<Vec<u8>>>, grace: Duration) -> Vec<u8> {
    let Some(h) = h else { return Vec::new() };
    let until = Instant::now() + grace;
    while !h.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    if h.is_finished() { h.join().unwrap_or_default() } else { Vec::new() }
}

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The end of `bytes` that is text, one line, and has no secrets in it.
fn gist(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    let line: String = line.chars().take(200).collect();
    kelpie_proto::redact(&line)
}

fn keep_end(mut r: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let (mut kept, mut buf) = (Vec::new(), [0u8; 4096]);
        while let Ok(n) = r.read(&mut buf) {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&buf[..n]);
            if kept.len() > OUTPUT_KEPT * 2 {
                kept.drain(..kept.len() - OUTPUT_KEPT);
            }
        }
        kept
    })
}

/// What a command is asked to run, for a dry run.
fn describe_plan(plan: &ShellPlan) -> String {
    let args: Vec<String> = plan.args.iter().map(|a| format!("{a:?}")).collect();
    format!("would run {:?} {} in {}", plan.program, args.join(" "), plan.cwd)
}

impl Daemon {
    // ---------- who is the app ----------

    /// The connection that just authenticated as the user says whether it is the Kelpie app.
    pub fn mark_app(&self, client: u64, app: bool) {
        if app {
            self.auto_rt.app.lock().insert(client);
        } else {
            self.auto_rt.app.lock().remove(&client);
        }
    }

    fn is_app(&self, client: u64) -> bool {
        self.auto_rt.app.lock().contains(&client)
    }

    fn app_only(&self, client: u64, method: &str) -> Result<()> {
        anyhow::ensure!(self.is_app(client), "{method} is only available to the Kelpie app");
        Ok(())
    }

    // ---------- the RPCs ----------

    pub(super) fn automation_rpc(self: &Arc<Self>, client: u64, method: &str, params: Value) -> Result<Value> {
        Ok(match method {
            "automation.list" => Value::Array(self.automations.lock().list.iter().map(shown).collect()),
            "automation.state" => {
                let a = self.automations.lock();
                json!({"paused": a.paused, "error": a.broken(), "fix": a.fix().code(), "version": FILE_VERSION, "min_gap_ms": self.auto_rt.gap_ms})
            }
            "automation.save" => {
                let a: Automation = serde_json::from_value(params).context("automation")?;
                // Looks for the program and reads its scripts here, not under the lock the ticker waits for.
                let _ = a.command_hash();
                // A program is something the user set up in the app and approved there (an "agent" that is a plain
                // shell types one).
                if a.runs_commands() || a.shell.is_some() {
                    self.app_only(client, "saving a command")?;
                }
                let saved = self.automations.lock().upsert(a)?;
                if saved.trigger.as_ref().is_some_and(|t| t.event == EventKind::Commit) {
                    self.watch_head(&saved.cwd);
                }
                self.broadcast("automation.changed", json!({}));
                shown(&saved)
            }
            "automation.delete" => {
                self.automations.lock().remove(param(&params, "id")?.as_str().unwrap_or_default())?;
                self.broadcast("automation.changed", json!({}));
                json!(true)
            }
            "automation.approve" => {
                self.app_only(client, "approving a command")?;
                let text = |k: &str| param(&params, k).map(|v| v.as_str().unwrap_or_default().to_string());
                let approved = self.automations.lock().approve(&text("id")?, &text("hash")?)?;
                self.broadcast("automation.changed", json!({}));
                shown(&approved)
            }
            "automation.resolve" => {
                self.app_only(client, "automation.resolve")?;
                let action = param(&params, "action")?.as_str().unwrap_or_default().to_string();
                self.automations.lock().resolve(&action)?;
                self.broadcast("automation.changed", json!({}));
                json!(true)
            }
            "automation.set_paused" => {
                let paused = param(&params, "paused")?.as_bool().ok_or_else(|| anyhow!("`paused` must be true or false"))?;
                self.automations.lock().set_paused(paused)?;
                self.broadcast("automation.changed", json!({}));
                json!({"paused": paused})
            }
            "automation.run" => {
                let dry = params.get("dry_run").and_then(Value::as_bool) == Some(true);
                self.run_automation(param(&params, "id")?.as_str().unwrap_or_default(), dry, true)?
            }
            "automation.event" => {
                self.app_only(client, "automation.event")?;
                let ev: Event = serde_json::from_value(params).context("event")?;
                // Turns and commits are the daemon's own to see; the app reports what only it can.
                anyhow::ensure!(matches!(ev.kind, EventKind::File | EventKind::PrChecksFailed | EventKind::Finished), "the app cannot report {} events", ev.kind.code());
                self.automation_event(ev.sanitized()?);
                json!(true)
            }
            other => bail!("unknown method {other}"),
        })
    }

    // ---------- events ----------

    /// Something happened: runs the automations it sets off, each on its own thread.
    pub fn automation_event(self: &Arc<Self>, ev: Event) {
        let now = now_millis();
        let outcome = {
            let mut store = self.automations.lock();
            let neighbours = self.busy_neighbours(&store.list, &ev, now);
            store.on_event_chained(&ev, now, &today(), self.auto_rt.gap_ms, &|id| self.automation_busy(id, now), &|a| neighbours.iter().filter(|(id, _)| *id != a.id).map(|(_, d)| d.saturating_add(1)).max().unwrap_or(0))
        };
        if outcome.fire.is_empty() && outcome.notices.is_empty() {
            return;
        }
        {
            let mut b = self.auto_rt.busy.lock();
            for f in &outcome.fire {
                b.depth.insert(f.auto.id.clone(), f.depth);
                b.fired.insert(f.auto.id.clone(), now);
            }
        }
        for (a, text) in outcome.notices {
            self.raise(Raised { project_id: a.project_id.clone(), ..Raised::automation(&a.name, None, Some(text)) }, true);
        }
        for firing in outcome.fire {
            let me = self.clone();
            let spawned = std::thread::Builder::new().name("kelpie-automation".into()).spawn(move || me.run_fired(firing));
            if let Err(e) = spawned {
                tracing::warn!("automation: cannot start a thread: {e}");
            }
        }
        self.broadcast("automation.changed", json!({}));
    }

    /// Whether an automation still has a command running, a session of its own working, or only just
    /// stopped: edits during that time are its own and must not set it off again.
    fn automation_busy(&self, id: &str, now: u64) -> bool {
        let sessions: Vec<String> = {
            let b = self.auto_rt.busy.lock();
            if b.running.contains(id) || b.quiet_until.get(id).is_some_and(|q| *q > now) {
                return true;
            }
            b.sessions.iter().filter(|(_, a)| a.as_str() == id).map(|(s, _)| s.clone()).collect()
        };
        // Waiting (a permission question, say) is still the same piece of work: its edits are not new events.
        sessions.iter().any(|s| self.by_id(s).is_some_and(|e| {
            let i = e.info();
            i.alive && matches!(i.status, Status::Working | Status::Waiting)
        }))
    }

    /// The automations that are busy, or fired in the last two minutes, in the folder an event is about, with how
    /// long their chain is: what they did may be what caused the event (whether or not it is still going on).
    fn busy_neighbours(&self, list: &[Automation], ev: &Event, now: u64) -> Vec<(String, u8)> {
        {
            let b = self.auto_rt.busy.lock();
            if b.fired.values().all(|t| now.saturating_sub(*t) >= CHAIN_WINDOW_MS) && b.sessions.is_empty() && b.running.is_empty() && b.quiet_until.values().all(|q| *q <= now) {
                return Vec::new();
            }
        }
        let recent = |id: &str| self.auto_rt.busy.lock().fired.get(id).is_some_and(|t| now.saturating_sub(*t) < CHAIN_WINDOW_MS);
        // What an automation's work can lead to: edits and commits from any; a finished turn only from one that
        // started an agent; the window's own events (pull requests, finished worktrees) from none.
        let could_cause = |x: &Automation| match ev.kind {
            EventKind::File | EventKind::Commit => true,
            EventKind::Turn => x.action != ActionKind::Shell,
            EventKind::PrChecksFailed | EventKind::Finished => false,
        };
        let near: Vec<&Automation> = list.iter().filter(|x| could_cause(x) && (crate::auth::within(&ev.root, &x.cwd) || crate::auth::within(&x.cwd, &ev.root))).collect();
        near.into_iter()
            .filter(|x| recent(&x.id) || self.automation_busy(&x.id, now))
            .map(|x| (x.id.clone(), self.auto_rt.busy.lock().depth.get(&x.id).copied().unwrap_or(0)))
            .collect()
    }

    /// `session` is now the work of `automation`: its edits are not events for it.
    fn adopt_session(&self, automation: &Automation, session: &str) {
        let known: HashSet<String> = self.sessions.read().values().map(|e| e.spec().id).collect();
        let mut b = self.auto_rt.busy.lock();
        b.sessions.retain(|s, _| known.contains(s));
        b.sessions.insert(session.to_string(), automation.id.clone());
        b.quiet_until.insert(automation.id.clone(), now_millis() + QUIET_AFTER_MS);
    }

    /// A session finished a turn: its automation (if it is one's) goes quiet, then `turn` events and a
    /// look at HEAD for commits it made. Blocking (git): not for the ticker.
    pub(super) fn automation_turn_ended(self: &Arc<Self>, info: &SessionInfo) {
        let id = &info.spec.id;
        {
            let mut b = self.auto_rt.busy.lock();
            if let Some(a) = b.sessions.get(id).cloned() {
                b.quiet_until.insert(a, now_millis() + QUIET_AFTER_MS);
            }
        }
        if info.spec.kind == "shell" {
            return;
        }
        let mut ev = Event::new(EventKind::Turn, &info.spec.cwd);
        ev.session = Some(id.clone());
        self.automation_event(ev);
        // Commits the agent made: only in folders a commit automation watches, against what the poll last saw.
        let roots: Vec<String> = self.automations.lock().commit_roots().into_iter().filter(|r| crate::auth::within(&info.spec.cwd, r)).collect();
        for root in roots {
            self.check_head(&root, Some(id), true);
        }
    }

    /// Looks at HEAD of `root`; when a commit landed since the last look (and `fire`), a `commit` event.
    fn check_head(self: &Arc<Self>, root: &str, session: Option<&str>, fire: bool) {
        // One look at a time, from asking git to recording the answer: two looks cannot record out of order
        // (a stale answer taken for a move), and a baseline is only taken where there is none yet.
        let mut heads = self.auto_rt.heads.lock();
        let key = crate::meta::key(root);
        if !fire && heads.knows(&key) {
            return;
        }
        let Some((commit, branch)) = automations::head_of(root) else { return };
        let moved = heads.observe(&key, &commit, &branch);
        drop(heads);
        if moved && fire {
            let mut ev = Event::new(EventKind::Commit, root);
            ev.commit = Some(commit);
            ev.branch = Some(branch);
            ev.session = session.map(str::to_string);
            self.automation_event(ev);
        }
    }

    /// Where a commit automation's folder is now, so its next commit is seen (on its own thread).
    fn watch_head(self: &Arc<Self>, root: &str) {
        let (me, root) = (self.clone(), root.to_string());
        let _ = std::thread::Builder::new().name("kelpie-automation-head".into()).spawn(move || me.check_head(&root, None, false));
    }

    /// Once at startup and then every few seconds from the ticker: commits made by hand, in a terminal
    /// or by a tool other than an agent of ours are seen by asking git about the folders of commit
    /// automations. Not awaited.
    pub fn poll_commits(self: &Arc<Self>, first: bool) {
        use std::sync::atomic::Ordering::SeqCst;
        let roots = self.automations.lock().commit_roots();
        // One look at a time: a slow disk or a git held up elsewhere must not pile threads up.
        if roots.is_empty() || self.auto_rt.polling.swap(true, SeqCst) {
            return;
        }
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("kelpie-automation-head".into()).spawn(move || {
            for root in roots {
                me.check_head(&root, None, !first);
            }
            me.auto_rt.polling.store(false, SeqCst);
        });
        if spawned.is_err() {
            self.auto_rt.polling.store(false, SeqCst);
        }
    }

    /// Tells the user once that the automations file was damaged or is newer than this Kelpie, as soon as
    /// the app is connected to hear it.
    pub(super) fn announce_automation_problem(&self) {
        // Only the app can show it: a hook report or a script connecting is not somebody who read it.
        if self.auto_rt.app.lock().is_empty() {
            return;
        }
        let Some(why) = self.automations.lock().unannounced.take() else { return };
        self.raise(Raised::automation("Automations", None, Some(why)), true);
        self.broadcast("automation.changed", json!({}));
    }

    /// Starts every clock automation that is due, each on its own thread so none waits for another and
    /// the ticker does not wait for any.
    pub(super) fn run_due_automations(self: &Arc<Self>) {
        let due = self.automations.lock().due(now_millis());
        for id in due {
            let me = self.clone();
            let _ = std::thread::Builder::new().name("kelpie-automation".into()).spawn(move || {
                if let Err(e) = me.run_automation(&id, false, false) {
                    tracing::warn!("automation {id}: {e:#}");
                }
            });
        }
    }

    // ---------- runs ----------

    /// "Run now" (`explicit`), a dry run, or a clock run: not an event, so the guards do not apply. A clock
    /// run of a dry-run automation only shows what it would do.
    pub fn run_automation(self: &Arc<Self>, id: &str, dry: bool, explicit: bool) -> Result<Value> {
        let a = self.automations.lock().list.iter().find(|a| a.id == id).cloned().ok_or_else(|| anyhow!("no automation {id}"))?;
        let ev = Event::sample(&a);
        self.run_one(&a, &ev, dry || (!explicit && a.dry_run))
    }

    fn run_fired(self: &Arc<Self>, f: Firing) {
        if let Err(e) = self.run_one(&f.auto, &f.event, f.auto.dry_run) {
            tracing::warn!("automation {}: {e:#}", f.auto.name);
        }
    }

    /// Does what `a` does for `ev` (or says what it would do) and records it.
    fn run_one(self: &Arc<Self>, a: &Automation, ev: &Event, dry: bool) -> Result<Value> {
        let result = if dry { self.describe(a, ev) } else { self.execute(a, ev) };
        let (session, error, note) = match &result {
            Ok(done) => (done.session.clone(), None, Some(done.note.clone())),
            Err(e) => (None, Some(format!("{e:#}")), None),
        };
        self.automations.lock().record(&a.id, now_millis(), session.clone(), error.clone(), note.clone());
        self.broadcast("automation.ran", json!({"id": a.id, "name": a.name, "session_id": session, "error": error, "dry_run": dry, "note": note}));
        // A dry run only shows what would happen, and a command is announced when it ends (`finish_shell`),
        // unless it could not even start.
        if !dry && (a.action != ActionKind::Shell || error.is_some()) {
            self.raise(Raised { project_id: a.project_id.clone(), ..Raised::automation(&a.name, session, error) }, true);
        }
        let done = result?;
        Ok(json!({"note": done.note, "session_id": done.session, "dry_run": dry, "detail": done.detail, "session": done.info}))
    }

    fn describe(&self, a: &Automation, ev: &Event) -> Result<Done> {
        let (note, detail) = match a.action {
            ActionKind::Shell => {
                let plan = automations::shell_plan(a, ev)?;
                (describe_plan(&plan), json!({"program": plan.program, "args": plan.args, "env": plan.env, "cwd": plan.cwd}))
            }
            ActionKind::Agent => {
                let prompt = automations::event_prompt(a, ev);
                (format!("would start {} in {} with a prompt of {} characters", a.kind, a.cwd, prompt.chars().count()), json!({"prompt": prompt}))
            }
            ActionKind::Quick => {
                let prompt = automations::event_prompt(a, ev);
                (format!("would type the saved prompt ({} characters) into the idle {} session it started in {}, or start one", prompt.chars().count(), a.kind, a.cwd), json!({"prompt": prompt}))
            }
        };
        Ok(Done { session: None, info: None, note: format!("dry run: {note}"), detail })
    }

    fn execute(self: &Arc<Self>, a: &Automation, ev: &Event) -> Result<Done> {
        match a.action {
            ActionKind::Shell => self.start_shell(a, ev),
            ActionKind::Quick => match self.type_into_own(a, ev)? {
                Some(session) => Ok(Done { session: Some(session), info: None, note: "typed the saved prompt into the session it started".into(), detail: Value::Null }),
                None => self.start_agent(a, ev),
            },
            ActionKind::Agent => self.start_agent(a, ev),
        }
    }

    fn start_agent(&self, a: &Automation, ev: &Event) -> Result<Done> {
        // A plain shell types what it is given: only the command the user approved.
        anyhow::ensure!(a.approved(), "{}", a.not_approved_reason());
        let info = self.create(json!({
            "kind": a.kind,
            "cwd": a.cwd,
            "project_id": a.project_id,
            "name": a.name,
            "args": a.args,
            "env": a.env,
            "prompt": automations::event_prompt(a, ev),
        }))?;
        let id = info.spec.id.clone();
        if a.deliver.as_deref() == Some(DELIVER_REVIEW) {
            self.deliver_review.lock().insert(id.clone(), a.cwd.clone());
        }
        self.adopt_session(a, &id);
        Ok(Done { session: Some(id), info: Some(info), note: format!("started a {} session", a.kind), detail: Value::Null })
    }

    /// The saved prompt, typed as one message into a session this automation started: never into one the
    /// user opened, and never on top of text somebody may be in the middle of writing. `Ok(None)`: it has
    /// no session alive (start one); an error: it has one, but it is busy or was typed into a moment ago.
    fn type_into_own(self: &Arc<Self>, a: &Automation, ev: &Event) -> Result<Option<String>> {
        let own: Vec<String> = self.auto_rt.busy.lock().sessions.iter().filter(|(_, owner)| **owner == a.id).map(|(s, _)| s.clone()).collect();
        let alive: Vec<Arc<Session>> = own
            .iter()
            .filter_map(|s| match self.by_id(s) {
                Some(Entry::Term(t)) if t.is_alive() && t.info().spec.remote.is_none() => Some(t),
                _ => None,
            })
            .collect();
        if alive.is_empty() {
            return Ok(None);
        }
        let text = automations::event_prompt(a, ev);
        let Some((target, written)) = alive.into_iter().find_map(|t| t.type_when_idle(&text, QUICK_QUIET).map(|w| (t, w))) else {
            bail!("the session this automation started is busy, or was typed into a moment ago: nothing was typed");
        };
        let id = target.info().spec.id;
        self.adopt_session(a, &id);
        // Enter is its own write (a TUI takes one in the same write as text taken for a paste), and only
        // if nothing was typed in between.
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            target.input_if_last_typed_at(written, b"\r".to_vec());
        });
        Ok(Some(id))
    }

    /// Starts the approved program (no shell: the program and its arguments as they are) and returns; a
    /// thread waits for it, records how it ended and, if it failed, tells the user.
    fn start_shell(self: &Arc<Self>, a: &Automation, ev: &Event) -> Result<Done> {
        // The approval is a hash of the command and of the files it runs, read a moment before they are started.
        // Another process of the same user could still swap a script between that read and `CreateProcess`; no
        // content hash can close that window, and Kelpie is not a boundary against code the user's own account
        // runs (an agent that can rewrite the script can run it directly). It guards against drift: an edit made
        // by an agent between approval and a run is caught.
        anyhow::ensure!(a.approved(), "{}", a.not_approved_reason());
        let plan = automations::shell_plan(a, ev)?;
        anyhow::ensure!(plan.found, "{} was not found (looked in the automation's folder and on PATH)", plan.program);
        let mut cmd = std::process::Command::new(&plan.program);
        cmd.args(&plan.args).current_dir(&plan.cwd).envs(plan.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn().with_context(|| format!("start {}", plan.program))?;
        // Everything the command starts is stopped with it (a job object, or its process group).
        let tree = crate::proc_tree::ProcTree::adopt(child.id(), None, None).ok();
        self.auto_rt.busy.lock().running.insert(a.id.clone());
        let (out, err) = (child.stdout.take().map(keep_end), child.stderr.take().map(keep_end));
        let (me, a) = (self.clone(), a.clone());
        let program = plan.program.clone();
        let started_note = format!("started {program}");
        std::thread::Builder::new().name("kelpie-automation-command".into()).spawn(move || {
            let status = wait_command(&mut child, tree.as_ref(), SHELL_TIMEOUT);
            // What the command launched and left running (an editor it opened) is left alone; its output is not
            // waited for beyond a few seconds. Only a command that timed out is stopped with all it started
            // (`wait_command`).
            if let Some(t) = &tree {
                t.release();
            }
            let (out, err) = (join_by(out, READERS_GRACE), join_by(err, READERS_GRACE));
            me.finish_shell(&a, &program, status, if err.is_empty() { &out } else { &err });
        })?;
        Ok(Done { session: None, info: None, note: started_note, detail: Value::Null })
    }

    fn finish_shell(&self, a: &Automation, program: &str, status: Result<std::process::ExitStatus>, output: &[u8]) {
        let said = gist(output);
        let (error, note) = match status {
            Ok(s) if s.success() => (None, format!("{program} finished{}", if said.is_empty() { String::new() } else { format!(": {said}") })),
            Ok(s) => {
                let why = format!("{program} exited with {}{}", s.code().map_or("a signal".to_string(), |c| format!("code {c}")), if said.is_empty() { String::new() } else { format!(": {said}") });
                (Some(why.clone()), why)
            }
            Err(e) => (Some(format!("{program}: {e:#}")), format!("{program} did not finish")),
        };
        {
            let mut b = self.auto_rt.busy.lock();
            b.running.remove(&a.id);
            b.quiet_until.insert(a.id.clone(), now_millis() + QUIET_AFTER_MS);
        }
        let at = self.automations.lock().list.iter().find(|x| x.id == a.id).map_or(0, |x| x.last_run);
        self.automations.lock().record(&a.id, at, None, error.clone(), Some(note.clone()));
        self.broadcast("automation.ran", json!({"id": a.id, "name": a.name, "session_id": null, "error": error, "dry_run": false, "note": note}));
        if error.is_some() {
            self.raise(Raised { project_id: a.project_id.clone(), ..Raised::automation(&a.name, None, error) }, true);
        }
    }
}

/// What one run did.
struct Done {
    session: Option<String>,
    info: Option<SessionInfo>,
    note: String,
    detail: Value,
}

#[cfg(test)]
mod tests;
