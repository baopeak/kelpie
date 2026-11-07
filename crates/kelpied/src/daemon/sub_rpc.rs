//! `sub.*`: sessions opening, steering and messaging other sessions (rules and state in `subs.rs`).
//!
//! Programs in a session reach these through the scripting surface (`auth`): `auth::authorize` puts the
//! caller's own id into `session_id`, so a program can only act as itself. The user's app answers
//! the requests (`sub.answer`) and sees them (`sub.request`); nothing a program sends ever stands for that answer.
//!
//! `session.create` from a program in a session is a sub-session too (`session_create_from_session`): it goes through
//! the same gates as `sub.spawn`, so the policy and the caps cannot be got around by the older door.

use super::*;
use crate::chat::{MessageOrigin, NewMessage};
use crate::session::{By, SubReport};
use crate::subs::{self, Added, Auto, CapsInput, Decision, MailKind, Mode, NewMail, Named, Request, SubPolicy, Waited};

/// Which call opened the child, and so how it is answered and what it is started with.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `sub.spawn`: the agent gets the brief and may be given a model; the reply is `{id, handle, name, kind, cwd, worktree?}`.
    Sub,
    /// `session.create`: the reply is the session's info, as ever.
    Create,
}

/// A spawn as asked for and checked: nothing is started from it until the caps and the user agree.
struct SpawnPlan {
    shape: Shape,
    agent: String,
    /// What the child is to do (`sub.spawn`), or the first prompt `session.create` gave (may be empty).
    task: String,
    name: Option<String>,
    /// Absolute, an existing folder.
    cwd: String,
    worktree: bool,
    /// The model asked for, and the agent's flag for it. The only argument a program in a session chooses.
    model: Option<String>,
    model_args: Vec<String>,
    /// `cols`, `rows` and `restricted` of `session.create`, passed on as given.
    extra: serde_json::Map<String, Value>,
}

#[cfg(test)]
mod tests;

/// `session_id` as `auth::authorize` sets it: the calling session.
fn caller_of(method: &str, params: &Value) -> Result<String> {
    params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string).ok_or_else(|| anyhow!("{method} is for programs running in a Kelpie session"))
}

/// A folder as the rest of Kelpie compares them: canonical, without Windows' `\\?\` prefix (`\\?\UNC\server\share` is `\\server\share`).
fn existing_folder(base: &str, given: &str) -> Result<String> {
    let path = std::path::Path::new(given);
    let full = if path.is_absolute() { path.to_path_buf() } else { std::path::Path::new(base).join(path) };
    let real = std::fs::canonicalize(&full).map_err(|_| anyhow!("cwd `{given}` does not exist"))?;
    anyhow::ensure!(real.is_dir(), "cwd `{given}` is not a folder");
    let text = real.to_string_lossy().into_owned();
    Ok(match text.strip_prefix(r"\\?\") {
        Some(share) if share.starts_with("UNC\\") => format!(r"\\{}", &share[4..]),
        Some(plain) => plain.to_string(),
        None => text,
    })
}

/// The first line of the task as a name, when the program gave none.
fn name_from_task(task: &str) -> String {
    let line = task.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("sub-session");
    subs::cut(line, 40)
}

/// The part of a branch name that comes from a name: `runs::slug`, but a name with no letters it can keep is not "run".
fn branch_slug(name: &str) -> String {
    let slug = runs::slug(name);
    if slug == "run" && !name.to_lowercase().contains("run") { "task".into() } else { slug }
}

/// The branch a worktree `leaf` would use is taken (checked with git, not by what its error says).
fn branch_exists(cwd: &str, leaf: &str) -> bool {
    run_git(&["-C", cwd, "show-ref", "--verify", "--quiet", &format!("refs/heads/kelpie/{leaf}")]).is_ok()
}

/// The session runs without asking before it acts, as far as the daemon can tell: the one answer for D6 (a child has no more
/// permission than its parent) and for who may message whom. Its arguments, in every spelling `subs::args_skip_permissions`
/// knows, and for a chat the mode it was last told to run in (a mode chip). Best effort: a spelling or a way of changing the mode
/// the daemon does not know is not seen, and a chat's launch arguments are those the app passed as `args`.
fn skips_permissions(entry: &Entry) -> bool {
    let spec = entry.spec();
    let yolo = agents::preset_for(&spec.kind).map_or(&[][..], |p| p.yolo);
    subs::args_skip_permissions(&spec.kind, yolo, &spec.args) || matches!(entry, Entry::Chat(c) if c.skips_permissions())
}

/// The session runs in full bypass (nothing asked, no sandbox): the only state a child inherits from. Strict by design, since a
/// child must never have more permission than its parent; see `skips_permissions` for the broad question.
fn bypasses_permissions(entry: &Entry) -> bool {
    let spec = entry.spec();
    let yolo = agents::preset_for(&spec.kind).map_or(&[][..], |p| p.yolo);
    subs::args_full_bypass(&spec.kind, yolo, &spec.args) || matches!(entry, Entry::Chat(c) if c.bypasses_permissions())
}

/// A value a program put in a parameter that it may not: anything but null, false, empty text, list or object.
fn is_set(v: &Value) -> bool {
    match v {
        Value::Null | Value::Bool(false) => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
        _ => true,
    }
}

/// What the child is started with (`session.create`'s parameters) and its depth. The arguments are the daemon's, never the
/// program's: the model it chose and, when the caller runs without asking (`parent_skips`), the child agent's own flags for
/// that, so a child has no more permission than the session that opened it.
fn child_params(plan: &SpawnPlan, caller: &SessionSpec, parent_skips: bool, parent_label: &str, cwd: &str, name: Option<&str>) -> (Value, u8) {
    let yolo = agents::preset_for(&plan.agent).map_or(&[][..], |p| p.yolo);
    let args = subs::child_args(yolo, parent_skips, plan.model_args.clone());
    let prompt = match plan.shape {
        Shape::Sub => Some(subs::child_brief(parent_label, &plan.task)),
        Shape::Create => (!plan.task.is_empty()).then(|| plan.task.clone()),
    };
    let mut params = json!({"kind": plan.agent, "cwd": cwd, "project_id": caller.project_id, "name": name, "args": args, "prompt": prompt});
    if let Some(obj) = params.as_object_mut() {
        obj.extend(plan.extra.clone());
    }
    (params, caller.depth.saturating_add(1))
}

impl Daemon {
    // ---------- policy ----------

    /// `daemon.policy`: what the user allows programs in sessions to do.
    pub(super) fn policy_json(&self) -> Value {
        let sub = *self.subs.policy.read();
        json!({
            "agents_control_others": self.agents_control_others(),
            "sub_sessions": sub.mode.as_str(),
            "sub_max": sub.max,
            // What 0 means on this computer, for the settings page to show.
            "sub_auto_max": subs::machine_cap(),
        })
    }

    pub(super) fn persist_policy(&self) {
        // One writer at a time, each taking its snapshot inside the lock: the last write is the newest.
        let _writing = self.subs.policy_file.lock();
        save_policy(&self.data_dir, &Policy { agents_control_others: self.agents_control_others(), sub: *self.subs.policy.read() });
    }

    /// The sub-session policy `daemon.set_policy` asks for, checked as a whole; `None` when it names neither key.
    pub(super) fn requested_sub_policy(&self, params: &Value) -> Result<Option<SubPolicy>> {
        let given = |key: &str| params.get(key).filter(|v| !v.is_null());
        let mode = given("sub_sessions").map(|v| v.as_str().and_then(Mode::parse).ok_or_else(|| anyhow!("sub_sessions must be off, ask or allow"))).transpose()?;
        let max = given("sub_max")
            .map(|v| v.as_u64().filter(|n| *n <= u64::from(subs::MAX_SUB_CAP)).ok_or_else(|| anyhow!("sub_max must be a whole number from 0 (automatic) to {}", subs::MAX_SUB_CAP)))
            .transpose()?;
        if mode.is_none() && max.is_none() {
            return Ok(None);
        }
        let now = *self.subs.policy.read();
        Ok(Some(SubPolicy { mode: mode.unwrap_or(now.mode), max: max.map_or(now.max, |n| n as u32) }))
    }

    // ---------- naming ----------

    /// A session as another session's prompt names it: `sub-session "fix-1" (codex, #12)`.
    pub(super) fn session_tag(&self, info: &SessionInfo) -> String {
        let spec = &info.spec;
        let kind_label = agents::preset_for(&spec.kind).map_or(spec.kind.as_str(), |p| p.label);
        let name = notify::session_label(spec.name.as_deref(), info.title.as_deref(), &spec.kind, kind_label, spec.remote.as_ref().map(|r| r.dest.as_str()));
        let name = subs::label_text(&name);
        if spec.parent.is_some() {
            return format!("sub-session \"{name}\" ({}, #{})", spec.kind, info.handle);
        }
        match spec.project_id.as_deref().and_then(|p| self.mobile.project_name(p)) {
            Some(project) => format!("session \"{name}\" (project {}, #{})", subs::label_text(&project), info.handle),
            None => format!("session \"{name}\" ({}, #{})", spec.kind, info.handle),
        }
    }

    // ---------- spawn ----------

    /// The sessions of tasks that ended, as the run list says.
    fn finished_task_sessions(&self) -> HashSet<String> {
        self.runs.lock().finished_sessions()
    }

    /// (busy children of `caller`, busy sub-sessions) from the session list. Busy: running and not finished. A session is
    /// finished when its run task is (`finished` holds those sessions) or when it last reported itself done or failed,
    /// and it is not working or waiting on the user now; a session that has done its work and sits idle does not hold a
    /// place that another task needs.
    fn live_sub_counts(&self, caller: &str, finished: &HashSet<String>) -> (usize, usize) {
        let (mut children, mut total) = (0, 0);
        for entry in self.sessions.read().values() {
            let info = entry.info();
            let reported_end = info.spec.report.as_ref().is_some_and(|r| !r.auto && matches!(r.status.as_str(), "done" | "failed"));
            // Finished counts only while it holds: a finished child that was given more work (a follow-up
            // message, a run task continued by hand) is busy again and takes its place back.
            let at_work = matches!(info.status, Status::Working | Status::Waiting);
            let finished_now = (reported_end || finished.contains(&info.spec.id)) && !at_work;
            if info.alive && info.spec.parent.is_some() && !finished_now {
                total += 1;
                if info.spec.parent.as_deref() == Some(caller) {
                    children += 1;
                }
            }
        }
        (children, total)
    }

    /// The gate every way a program opens a session passes: the policy and every cap, checked and held (a place in them) until
    /// the returned slot is dropped. `sub.spawn` and `session.create` from a session both go through it.
    fn reserve_slot(&self, caller: &str, spec: &SessionSpec) -> Result<subs::Slot<'_>> {
        self.reserve_slot_with(caller, spec, &self.finished_task_sessions())
    }

    /// [`Self::reserve_slot`] with the finished task sessions already known (the run engine holds the run list's lock).
    fn reserve_slot_with(&self, caller: &str, spec: &SessionSpec, finished: &HashSet<String>) -> Result<subs::Slot<'_>> {
        let input = CapsInput { mode: self.subs.policy.read().mode, restricted: spec.restricted, caller_depth: spec.depth, caller_children: 0, live_total: 0, cap: self.subs.cap(), load: self.mobile.load.level() };
        self.subs.reserve(caller, input, || self.live_sub_counts(caller, finished)).map_err(|e| anyhow!(e))
    }

    /// `known` and `installed` are the agents Kelpie can start and those on this computer.
    fn parse_spawn(&self, params: &Value, caller: &SessionSpec, known: &[&str], installed: &[&str]) -> Result<SpawnPlan> {
        anyhow::ensure!(caller.remote.is_none(), "a session on another machine cannot open sub-sessions");
        let text = |key: &str| params.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
        let agent = text("agent").ok_or_else(|| anyhow!("missing param `agent`"))?.to_string();
        let task = crate::session::typed_text(text("task").ok_or_else(|| anyhow!("missing param `task`: say what the sub-session is to do"))?);
        anyhow::ensure!(task.chars().count() <= subs::MAX_TASK, "the task is too long (at most {} characters); put the details in a file and name it in the task", subs::MAX_TASK);
        subs::check_agent(&agent, known, installed).map_err(|e| anyhow!(e))?;
        let model = text("model").map(str::to_string);
        let model_args = match &model {
            Some(model) => subs::model_args(&agent, model).map_err(|e| anyhow!(e))?,
            None => Vec::new(),
        };
        let cwd = existing_folder(&caller.cwd, text("cwd").unwrap_or(&caller.cwd))?;
        let name = text("name").map(|n| subs::cut(&crate::session::typed_text(n).replace('\n', " "), 80));
        Ok(SpawnPlan {
            shape: Shape::Sub,
            agent,
            task,
            name,
            cwd,
            worktree: params.get("worktree").and_then(Value::as_bool).unwrap_or(false),
            model,
            model_args,
            extra: Default::default(),
        })
    }

    /// `session.create` from a program in a session, as a plan. Only an installed agent can be opened this way, and only with what
    /// has no way past the gates: no id, environment, remote host, command line (`chat_argv`) or conversation to resume, and
    /// no arguments but a model.
    fn parse_create(&self, params: &Value, caller: &SessionSpec, known: &[&str], installed: &[&str]) -> Result<SpawnPlan> {
        anyhow::ensure!(caller.remote.is_none(), "a session on another machine cannot open sessions");
        let obj = params.as_object().ok_or_else(|| anyhow!("session.create: params must be an object"))?;
        for key in ["id", "env", "chat_argv", "remote", "agent_session_id", "lineage_from"] {
            if obj.get(key).is_some_and(is_set) {
                bail!("session.create from a program in a session cannot set `{key}`");
            }
        }
        let kind = obj.get("kind").and_then(Value::as_str).unwrap_or("shell").to_string();
        if matches!(kind.as_str(), "shell" | "custom") {
            bail!("a program in a session can open agent sessions only, not `{kind}`: a {kind} session could start any program, with any permission, outside the sub-session limits and your approval");
        }
        subs::check_agent(&kind, known, installed).map_err(|e| anyhow!(e))?;
        let given = obj.get("cwd").and_then(Value::as_str).filter(|c| !c.trim().is_empty()).ok_or_else(|| anyhow!("missing param `cwd`"))?;
        let cwd = existing_folder(&caller.cwd, given.trim())?;
        let given_args = match obj.get("args") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| anyhow!("args must be a list of text"))).collect::<Result<Vec<_>>>()?,
            Some(_) => bail!("args must be a list of text"),
        };
        // Only a model: what else an agent may be started with decides what it may do, and is the user's to choose.
        let model_args = subs::only_model(&kind, &given_args).map_err(|e| anyhow!(e))?;
        let model = model_args.get(1).cloned();
        let task = crate::session::typed_text(obj.get("prompt").and_then(Value::as_str).unwrap_or_default().trim());
        anyhow::ensure!(task.chars().count() <= subs::MAX_TASK, "the prompt is too long (at most {} characters)", subs::MAX_TASK);
        let name = obj.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty()).map(|n| subs::cut(&crate::session::typed_text(n).replace('\n', " "), 80));
        let mut extra = serde_json::Map::new();
        for key in ["cols", "rows", "restricted"] {
            if let Some(v) = obj.get(key).filter(|v| !v.is_null()) {
                extra.insert(key.into(), v.clone());
            }
        }
        Ok(SpawnPlan { shape: Shape::Create, agent: kind, task, name, cwd, worktree: false, model, model_args, extra })
    }

    /// `sub.spawn`.
    pub(super) fn sub_spawn(self: &Arc<Self>, tx: &ClientTx, id: &Value, params: &Value) -> Result<Option<Value>> {
        let caller_id = caller_of("sub.spawn", params)?;
        let caller = self.by_id(&caller_id).ok_or_else(|| anyhow!("the calling session no longer exists"))?;
        let caller_spec = caller.spec();
        // Refused before the user is asked about something that cannot happen.
        drop(self.reserve_slot(&caller_id, &caller_spec)?);
        let installed = agents::installed_presets();
        let known: Vec<&str> = agents::all_presets().into_iter().map(|p| p.id).collect();
        let plan = self.parse_spawn(params, &caller_spec, &known, &installed)?;
        self.open_child(tx, id, &caller_id, &caller, plan)
    }

    /// `session.create` called by a program in session `caller_id`: a sub-session of it, through the same gates. The reply keeps
    /// the shape it always had.
    pub(super) fn session_create_from_session(self: &Arc<Self>, tx: &ClientTx, id: &Value, caller_id: &str, params: &Value) -> Result<Option<Value>> {
        let caller = self.by_id(caller_id).ok_or_else(|| anyhow!("the calling session no longer exists"))?;
        let caller_spec = caller.spec();
        drop(self.reserve_slot(caller_id, &caller_spec)?);
        let installed = agents::installed_presets();
        let known: Vec<&str> = agents::all_presets().into_iter().map(|p| p.id).collect();
        let plan = self.parse_create(params, &caller_spec, &known, &installed)?;
        self.open_child(tx, id, caller_id, &caller, plan)
    }

    /// Answers at once when the policy (or an earlier "allow for this session") lets the child open, else when the user has
    /// decided. The wait is on a thread of its own, so nothing else of the daemon waits.
    fn open_child(self: &Arc<Self>, tx: &ClientTx, id: &Value, caller_id: &str, caller: &Entry, plan: SpawnPlan) -> Result<Option<Value>> {
        if self.subs.policy.read().mode == Mode::Allow || self.subs.granted(caller_id) {
            return Ok(Some(self.spawn_child(caller_id, plan)?));
        }
        let request = Request {
            id: uuid::Uuid::new_v4().to_string(),
            session: caller_id.to_string(),
            from_label: self.session_tag(&caller.info()),
            agent: plan.agent.clone(),
            cwd: plan.cwd.clone(),
            worktree: plan.worktree,
            name: plan.name.clone(),
            task: subs::cut(&plan.task, subs::TASK_PREVIEW),
            model: plan.model.clone(),
            skips_prompts: bypasses_permissions(caller) && agents::preset_for(&plan.agent).is_some_and(|p| !p.yolo.is_empty()),
            at: now_millis(),
        };
        let rx = self.subs.pending.lock().open(request.clone()).map_err(|e| anyhow!(e))?;
        self.broadcast("sub.request", serde_json::to_value(&request)?);
        let (me, tx, id, request_id, caller_id) = (self.clone(), tx.clone(), id.clone(), request.id.clone(), caller_id.to_string());
        let started = std::thread::Builder::new().name("kelpie-sub-ask".into()).spawn(move || {
            let outcome = subs::wait_for_answer(&me.subs.pending, &request_id, &rx, subs::ASK_TIMEOUT, &|| tx.is_closed());
            let result = match outcome {
                // Interrupted by its program: the card goes away and nothing is started, whatever the user does next.
                Waited::CallerGone => {
                    me.broadcast("sub.request_done", json!({"id": request_id, "decision": "cancelled"}));
                    return;
                }
                Waited::Decided(Decision::Deny) => Err(anyhow!("the user denied this sub-session in Kelpie")),
                // The caller hung up while waiting: starting a session nobody can learn about would only cost.
                Waited::Decided(_) if tx.is_closed() => Err(anyhow!("the program that asked is gone")),
                Waited::Decided(_) => me.spawn_child(&caller_id, plan),
                Waited::TimedOut => {
                    me.broadcast("sub.request_done", json!({"id": request_id, "decision": "timeout"}));
                    Err(anyhow!("nobody answered in Kelpie; ask the user to open Kelpie and allow it, then try again"))
                }
                Waited::Cancelled => Err(anyhow!("the session that asked was closed before the user answered")),
            };
            // A call sent without an id wants no answer (the connection code sends none either).
            if !id.is_null() {
                let _ = tx.send(json_out(&rpc_answer(&id, result)));
            }
        });
        if let Err(e) = started {
            self.subs.pending.lock().cancel(&request.id);
            self.broadcast("sub.request_done", json!({"id": request.id, "decision": "cancelled"}));
            bail!("could not wait for the user's answer: {e}");
        }
        Ok(None)
    }

    /// Opens the child: checks the caps again (the world moved while the user decided), makes the worktree when asked,
    /// and starts the agent. The child never has more permission than the caller (`child_params`).
    fn spawn_child(&self, caller_id: &str, plan: SpawnPlan) -> Result<Value> {
        let caller = self.by_id(caller_id).ok_or_else(|| anyhow!("the calling session was closed"))?;
        let caller_spec = caller.spec();
        let _slot = self.reserve_slot(caller_id, &caller_spec)?;
        let name = plan.name.clone().or_else(|| (!plan.task.is_empty()).then(|| name_from_task(&plan.task)));
        let worktree = if plan.worktree {
            let slug = branch_slug(name.as_deref().unwrap_or("task"));
            let mut tries = 0;
            loop {
                let tag = uuid::Uuid::new_v4().simple().to_string();
                let leaf = format!("sub-{slug}-{}", &tag[..4]);
                match create_worktree(&plan.cwd, &leaf, None, "a sub-session worktree needs the folder") {
                    Ok(w) => break Some(w),
                    // The same four letters twice: another try with others.
                    Err(e) if tries < 2 && branch_exists(&plan.cwd, &leaf) => {
                        let _ = e;
                        tries += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
        } else {
            None
        };
        let cwd = worktree.as_ref().map_or_else(|| plan.cwd.clone(), |w| w.path.clone());
        let (params, depth) = child_params(&plan, &caller_spec, bypasses_permissions(&caller), &self.session_tag(&caller.info()), &cwd, name.as_deref());
        let info = match self.create_inner(params, Some((caller_id.to_string(), depth))) {
            Ok(info) => info,
            Err(e) => {
                // The worktree was made for this session only and nothing is in it: it goes, or the error says it stayed.
                return Err(match worktree.as_ref().and_then(|w| discard_worktree(&plan.cwd, w)) {
                    Some(left) => e.context(left),
                    None => e,
                });
            }
        };
        if plan.shape == Shape::Create {
            let mut out = serde_json::to_value(&info)?;
            if let Some(o) = out.as_object_mut() {
                o.remove("env");
            }
            return Ok(out);
        }
        let mut out = json!({"id": info.spec.id, "handle": info.handle, "name": info.spec.name, "kind": info.spec.kind, "cwd": info.spec.cwd});
        if let Some(w) = worktree {
            out["worktree"] = json!({"path": w.path, "branch": w.branch, "base": w.base});
        }
        Ok(out)
    }

    // ---------- the other sub.* methods ----------

    pub(super) fn sub_rpc(&self, method: &str, params: &Value) -> Result<Value> {
        match method {
            "sub.pending" => Ok(json!(self.subs.pending.lock().list())),
            "sub.answer" => self.sub_answer(params),
            "sub.message" => self.sub_message(params),
            "sub.report" => self.sub_report(params),
            "sub.inbox" => self.sub_inbox(params),
            "sub.close" => self.sub_close(params),
            other => bail!("unknown method {other}"),
        }
    }

    /// The user's answer to a request (the user's connections only: `auth`).
    fn sub_answer(&self, params: &Value) -> Result<Value> {
        let id = param(params, "id")?.as_str().unwrap_or_default();
        let decision = param(params, "decision")?.as_str().and_then(Decision::parse).ok_or_else(|| anyhow!("decision must be once, session, always or deny"))?;
        let request = self.subs.pending.lock().answer(id, decision).map_err(|e| anyhow!(e))?;
        match decision {
            Decision::Session => self.subs.grant(&request.session),
            Decision::Always => {
                self.subs.policy.write().mode = Mode::Allow;
                self.persist_policy();
            }
            Decision::Once | Decision::Deny => {}
        }
        self.broadcast("sub.request_done", json!({"id": id, "decision": decision.as_str()}));
        Ok(json!(true))
    }

    /// The row of a session (handle, id, name) to pick one by what a program wrote.
    fn pick_session(&self, target: &str) -> Result<SessionInfo> {
        let list = self.list();
        let named: Vec<Named> = list.iter().map(|i| Named { handle: i.handle, id: &i.spec.id, name: i.spec.name.as_deref().unwrap_or("") }).collect();
        let at = subs::find_session(&named, target).map_err(|e| anyhow!(e))?;
        Ok(list[at].clone())
    }

    /// `sub.message {to, text}`: mail for another session's agent, or from the user (no `session_id`).
    fn sub_message(&self, params: &Value) -> Result<Value> {
        let target = self.pick_session(param(params, "to")?.as_str().unwrap_or_default())?;
        anyhow::ensure!(target.alive, "{} is not running", self.session_tag(&target));
        // A shell would run the text as a command; only an agent reads it as a message.
        anyhow::ensure!(agents::preset_for(&target.spec.kind).is_some(), "{} is not an agent; messages are for agent sessions", self.session_tag(&target));
        let text = param(params, "text")?.as_str().unwrap_or_default();
        let (from_id, from_label) = match params.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            Some(sender) => {
                anyhow::ensure!(sender != target.spec.id, "a session cannot message itself");
                let sender_entry = self.by_id(sender).ok_or_else(|| anyhow!("the calling session no longer exists"))?;
                let info = sender_entry.info();
                // A session that asks before it acts cannot make one that does not ask act for it. (The user always may.)
                let target_entry = self.by_id(&target.spec.id).ok_or_else(|| anyhow!("{} is not running", self.session_tag(&target)))?;
                anyhow::ensure!(
                    !skips_permissions(&target_entry) || skips_permissions(&sender_entry),
                    "{} runs with permission checks off and this session does not; a message from here could make it act with nobody asked. Ask the user to tell it instead",
                    self.session_tag(&target)
                );
                (sender.to_string(), self.session_tag(&info))
            }
            None => ("user".to_string(), "a program run by the user".to_string()),
        };
        let mail = NewMail { from_id, from_label, kind: MailKind::Message, status: None, text: text.to_string() };
        let added = self.subs.mail.lock().add(&target.spec.id, mail, Instant::now(), now_millis(), true).map_err(|e| anyhow!(e))?;
        Ok(json!({"queued": added == Added::Queued, "to": {"id": target.spec.id, "handle": target.handle, "label": self.session_tag(&target)}}))
    }

    /// `sub.report {status, summary}`: what the caller says about its own work; its row shows it and its parent is mailed.
    fn sub_report(&self, params: &Value) -> Result<Value> {
        let sid = caller_of("sub.report", params)?;
        let status = param(params, "status")?.as_str().unwrap_or_default();
        anyhow::ensure!(matches!(status, "progress" | "blocked" | "needs_user" | "done" | "failed"), "status must be progress, blocked, needs_user, done or failed");
        let summary = subs::clean_text(params.get("summary").and_then(Value::as_str).unwrap_or_default(), subs::MAX_MAIL_TEXT);
        let entry = self.by_id(&sid).ok_or_else(|| anyhow!("the calling session no longer exists"))?;
        let info = entry.info();
        // The parent's mail first: a report refused for going too fast changes nothing, not even the row.
        let parent_info = info.spec.parent.as_deref().and_then(|p| self.by_id(p)).map(|p| p.info());
        if let Some(parent_info) = &parent_info {
            let mail = NewMail { from_id: sid.clone(), from_label: self.session_tag(&info), kind: MailKind::Report, status: Some(status.to_string()), text: summary.clone() };
            self.subs.mail.lock().add(&parent_info.spec.id, mail, Instant::now(), now_millis(), true).map_err(|e| anyhow!(e))?;
        }
        self.set_report(&entry, SubReport { status: status.to_string(), summary: subs::cut(&summary, subs::MAX_ROW_SUMMARY), at: now_millis(), auto: false });
        if matches!(status, "done" | "failed") {
            self.record_sub_outcome(&info.spec, status, true);
        }
        self.subs.watch.lock().explicit(&sid);
        Ok(json!({"delivered_to": parent_info.map(|p| self.session_tag(&p))}))
    }

    /// `sub.inbox {take?}`: the caller's mail, which it may remove.
    fn sub_inbox(&self, params: &Value) -> Result<Value> {
        let sid = caller_of("sub.inbox", params)?;
        let mut mail = self.subs.mail.lock();
        if params.get("take").and_then(Value::as_bool) == Some(true) {
            let (mails, dropped) = mail.take(&sid);
            return Ok(json!({"mails": mails, "dropped": dropped}));
        }
        let (mails, dropped) = mail.peek(&sid);
        Ok(json!({"mails": mails, "dropped": dropped}))
    }

    /// `sub.close {session}`: a session closes its own descendants, nobody else's.
    fn sub_close(&self, params: &Value) -> Result<Value> {
        let sid = caller_of("sub.close", params)?;
        let target = self.pick_session(param(params, "session")?.as_str().unwrap_or_default())?;
        anyhow::ensure!(self.is_descendant(&sid, &target.spec), "{} is not one of your sub-sessions; a session can close only the sub-sessions it opened", self.session_tag(&target));
        self.close(target.handle)?;
        Ok(json!(true))
    }

    /// `spec` is a child, grandchild… of `ancestor`, through sessions that are still open: once a middle session is closed the
    /// chain is broken, and the user alone closes what is below it. A chain is as long as the depth cap allows, and no longer.
    fn is_descendant(&self, ancestor: &str, spec: &SessionSpec) -> bool {
        let mut parent = spec.parent.clone();
        for _ in 0..=subs::MAX_DEPTH {
            let Some(p) = parent else { return false };
            if p == ancestor {
                return true;
            }
            parent = self.by_id(&p).and_then(|e| e.spec().parent);
        }
        false
    }

    /// Records what a session reported (the next tick tells the apps: the spec changed).
    fn set_report(&self, entry: &Entry, report: SubReport) {
        match entry {
            Entry::Term(s) => s.spec.lock().report = Some(report),
            Entry::Chat(c) => c.spec.lock().report = Some(report),
        }
        self.meta_dirty.store(true, Ordering::Relaxed);
    }

    /// Whether a new session gets the sub-session tools: while the policy allows sub-sessions at all.
    pub(super) fn sub_tools_on(&self) -> bool {
        self.subs.policy.read().mode != Mode::Off
    }

    /// How a sub-session ended, for the routing hints. The first end counts (a session closed after it reported done is `done`);
    /// a session no one opened is not a sub-session. `judged`: the work itself gave a verdict (a report of done or failed, a run
    /// task's end). Its time is what the daemon watched it work; a restart's downtime is not in it.
    pub(super) fn record_sub_outcome(&self, spec: &SessionSpec, status: &str, judged: bool) {
        if spec.parent.is_some() {
            let minutes = self.subs.births.lock().get(&spec.id).map(|born| born.elapsed().as_secs_f64() / 60.0);
            self.subs.stats.record(&spec.id, &spec.kind, status, judged, minutes);
        }
    }

    /// A sub-session is being closed. Its agent having exited or the user closing it says nothing about the work, so it has no
    /// verdict unless it gave one before. An exit alone is not recorded: a restart or a daemon update makes one too.
    pub(super) fn record_sub_close(&self, entry: &Entry) {
        let info = entry.info();
        let status = if info.status == crate::status::Status::Exited { "exited" } else { "closed" };
        self.record_sub_outcome(&info.spec, status, false);
    }

    /// A run task ended: done or failed is a verdict on the sub-session it ran in.
    pub(super) fn record_task_outcome(&self, run_id: &str, task_id: &str, session: &str) {
        let status = self.runs.lock().get(run_id).and_then(|r| r.tasks.iter().find(|t| t.id == task_id).map(|t| t.status));
        let status = match status {
            Some(TaskStatus::Done) => "done",
            Some(TaskStatus::Failed) => "failed",
            _ => return,
        };
        if let Some(entry) = self.by_id(session) {
            self.record_sub_outcome(&entry.spec(), status, true);
        }
    }

    // ---------- reports the daemon works out ----------

    /// A sub-session's status changed (from the ticker): waiting for the user, idle after work, or exited
    /// goes to its row and, while the parent is there, to the parent's mail. No model is asked.
    pub(super) fn sub_status_changed(&self, entry: &Entry, info: &SessionInfo, now: Instant) {
        let sid = &info.spec.id;
        self.subs.births.lock().entry(sid.clone()).or_insert(now);
        let Some(auto) = self.subs.watch.lock().observe(sid, info.status, now) else { return };
        // Stopped by the app to continue in the other view: the new session says what it is, and nothing it does on the way out
        // (going idle as it stops, exiting) is reported. If the switch does not happen, `flush_handovers` reports the exit later.
        if info.spec.switching {
            return;
        }
        self.report_auto(entry, info, auto, now);
    }

    /// Puts an automatic report on the row of `info` and, while the parent is there, in the parent's mail.
    fn report_auto(&self, entry: &Entry, info: &SessionInfo, auto: Auto, now: Instant) {
        let sid = &info.spec.id;
        let detail = match (&auto, entry) {
            (Auto::NeedsUser, Entry::Term(s)) => s.screen_prompt().and_then(|p| p.question).filter(|q| !q.trim().is_empty()).unwrap_or_else(|| subs::tail_lines(&s.text_tail(40), 3)),
            (Auto::Idle, Entry::Term(s)) => subs::tail_lines(&s.text_tail(40), subs::TAIL_LINES),
            _ => String::new(),
        };
        let text = subs::clean_text(&subs::auto_text(&auto, &detail, info.exit_code), subs::MAX_MAIL_TEXT);
        // What the agent said itself in this turn stays on the row when it only went idle; a wait for the user or an exit is news.
        if auto != Auto::Idle || !self.subs.watch.lock().explicit_this_turn(sid) {
            self.set_report(entry, SubReport { status: auto.status().to_string(), summary: subs::cut(&text, subs::MAX_ROW_SUMMARY), at: now_millis(), auto: true });
        }
        let Some(parent) = info.spec.parent.as_deref().and_then(|p| self.by_id(p)) else { return };
        let mail = NewMail { from_id: sid.clone(), from_label: self.session_tag(info), kind: MailKind::Report, status: Some(auto.status().to_string()), text };
        // The daemon's own reports are not held to the rate a program is: they follow the child's status, which changes slowly.
        if let Err(e) = self.subs.mail.lock().add(&parent.spec().id, mail, now, now_millis(), false) {
            tracing::debug!("sub-session report not queued: {e}");
        }
    }

    /// Switches between views that were not completed in time (called every second by the ticker): the session stays as it is,
    /// and if it did stop, its exit is reported now, as it would have been.
    pub(super) fn flush_handovers(&self) {
        for id in self.subs.due_handovers(Instant::now()) {
            let Some(entry) = self.by_id(&id) else { continue };
            match &entry {
                Entry::Term(s) => s.spec.lock().switching = false,
                Entry::Chat(c) => c.spec.lock().switching = false,
            }
            let info = entry.info();
            if !info.alive && info.spec.parent.is_some() {
                self.report_auto(&entry, &info, Auto::Exited, Instant::now());
            }
        }
    }

    // ---------- runs made by a program in a session ----------

    /// The session that made a run, if it can still have sub-sessions at all: sub-sessions on, and not nested too deep.
    fn run_creator(&self, creator: &str) -> Result<(Entry, SessionSpec), runs::Gate> {
        if self.subs.policy.read().mode == Mode::Off {
            return Err(runs::Gate::Fail("sub-sessions were turned off after this run was made".into()));
        }
        let Some(entry) = self.by_id(creator) else {
            return Err(runs::Gate::Fail("the session that made this run was closed".into()));
        };
        let spec = entry.spec();
        if spec.depth >= subs::MAX_DEPTH {
            return Err(runs::Gate::Fail(format!("sub-sessions nest at most {} levels deep", subs::MAX_DEPTH)));
        }
        Ok((entry, spec))
    }

    /// The run engine asks before it starts a task of a run made by a session (the check that holds nothing). `finished` are
    /// the sessions of tasks that ended: they no longer count against the limits.
    pub(super) fn run_gate(&self, creator: &str, finished: &HashSet<String>) -> runs::Gate {
        match self.run_creator(creator) {
            Err(gate) => gate,
            Ok((_, spec)) => match self.reserve_slot_with(creator, &spec, finished) {
                Ok(_) => runs::Gate::Open,
                Err(_) => runs::Gate::Wait(self.wait_reason(creator, finished)),
            },
        }
    }

    /// What is shown on a task that waits for room.
    fn wait_reason(&self, creator: &str, finished: &HashSet<String>) -> String {
        let (children, total) = self.live_sub_counts(creator, finished);
        if self.mobile.load.level() >= crate::load::Level::L3 {
            "waiting: this computer is under heavy load".into()
        } else if children >= subs::MAX_CHILDREN {
            format!("waiting for a free sub-session slot ({children} of {} of this session's busy)", subs::MAX_CHILDREN)
        } else {
            format!("waiting for a free sub-session slot ({total} of {} busy)", self.subs.cap())
        }
    }

    /// A task of a session's run began to wait for room: the session is told once (not at every tick).
    pub(super) fn announce_waiting(&self, run_id: &str, task_id: &str, reason: &str) {
        let (creator, run_name, title) = {
            let runs = self.runs.lock();
            let Some(run) = runs.get(run_id) else { return };
            let Some(creator) = run.creator.clone() else { return };
            let title = run.tasks.iter().find(|t| t.id == task_id).map_or_else(|| task_id.to_string(), |t| t.title.clone());
            (creator, run.name.clone(), title)
        };
        if self.by_id(&creator).is_none() {
            return;
        }
        let mail = NewMail {
            from_id: format!("run:{run_id}"),
            from_label: format!("run \"{}\"", subs::label_text(&run_name)),
            kind: MailKind::Report,
            status: Some("waiting".into()),
            text: format!("task \"{}\": {reason}; it starts by itself when a sub-session finishes", subs::label_text(&title)),
        };
        if let Err(e) = self.subs.mail.lock().add(&creator, mail, Instant::now(), now_millis(), false) {
            tracing::debug!("run notice not queued: {e}");
        }
    }

    /// The user retries a run whose session is gone: the run is the user's from then on (no parent, no sub-session limits).
    pub(super) fn adopt_orphaned_run(&self, params: &Value) {
        let id = params.get("id").and_then(Value::as_str).unwrap_or_default();
        let mut runs = self.runs.lock();
        if let Some(run) = runs.get_mut(id) {
            if run.creator.as_deref().is_some_and(|c| self.by_id(c).is_none()) {
                run.creator = None;
            }
        }
    }

    /// What a task of a run made by a session starts with: its sub-session depth, its arguments (the model it asked for, and the
    /// agent's own skip-prompts flags only when the session that made the run skips them) and the place held in the limits.
    /// `Err(Wait)` when the limits are full: the task waits.
    pub(super) fn task_lineage(&self, creator: &str, task: &runs::Task) -> Result<(u8, Vec<String>, subs::Slot<'_>), runs::Gate> {
        let (entry, spec) = self.run_creator(creator)?;
        let slot = self.reserve_slot(creator, &spec).map_err(|_| runs::Gate::Wait(self.wait_reason(creator, &self.finished_task_sessions())))?;
        let yolo = agents::preset_for(&task.kind).map_or(&[][..], |p| p.yolo);
        Ok((spec.depth.saturating_add(1), subs::child_args(yolo, bypasses_permissions(&entry), task.args.clone()), slot))
    }

    /// `run.create` from a program in a session. Refused when sub-sessions are off; when they ask, every task waits for the user's
    /// approval on the Runs page; the tasks start as sub-sessions of the session, within the limits.
    pub(super) fn run_create_from_session(&self, caller: &str, params: &Value) -> Result<Value> {
        let spec = self.by_id(caller).ok_or_else(|| anyhow!("the calling session no longer exists"))?.spec();
        let installed = agents::installed_presets();
        let known: Vec<&str> = agents::all_presets().into_iter().map(|p| p.id).collect();
        let new: NewRun = serde_json::from_value(params.clone()).context("run.create params")?;
        let new = self.prepare_session_run(caller, &spec, new, &known, &installed)?;
        let mut run = runs::build_run(new, uuid::Uuid::new_v4().to_string(), now_millis())?;
        run.creator = Some(caller.to_string());
        Ok(self.insert_run(run))
    }

    /// A run as a program in a session may ask for it: a folder that exists, agents Kelpie can start and that are installed,
    /// a model and nothing else for arguments, no environment, no host.
    fn prepare_session_run(&self, caller: &str, spec: &SessionSpec, mut new: NewRun, known: &[&str], installed: &[&str]) -> Result<NewRun> {
        let mode = self.subs.policy.read().mode;
        anyhow::ensure!(mode != Mode::Off, "sub-sessions are turned off; the user can allow them in Kelpie → Settings → Agents");
        anyhow::ensure!(spec.remote.is_none(), "a session on another machine cannot make runs");
        anyhow::ensure!(spec.depth < subs::MAX_DEPTH, "sub-sessions nest at most {} levels deep and this session is already at level {}; its tasks could not start", subs::MAX_DEPTH, spec.depth);
        let ask = mode == Mode::Ask && !self.subs.granted(caller);
        new.cwd = existing_folder(&spec.cwd, &new.cwd)?;
        new.project_id = spec.project_id.clone();
        new.materialize = None;
        for task in &mut new.tasks {
            anyhow::ensure!(task.host.is_none(), "a program cannot start a task on an SSH host");
            anyhow::ensure!(task.env.is_empty(), "a task made by a program in a session cannot set `env`");
            let kind = task.kind.clone().unwrap_or_else(|| "claude".into());
            subs::check_agent(&kind, known, installed).map_err(|e| anyhow!(e))?;
            task.args = subs::only_model(&kind, &task.args).map_err(|e| anyhow!(e))?;
            task.kind = Some(kind);
            // The user approves on the Runs page what they would have been asked about when the session opened it directly.
            task.approve |= ask;
        }
        Ok(new)
    }

    // ---------- delivery ----------

    /// Types the waiting mail into the sessions that sit idle (called every second by the ticker). A recipient that
    /// waits on a prompt, works, was typed in lately or is gone is left alone: the mail stays queued.
    pub(super) fn deliver_mail(&self) {
        let recipients = self.subs.mail.lock().recipients();
        for to in recipients {
            let Some(entry) = self.by_id(&to) else {
                self.subs.mail.lock().forget(&to);
                continue;
            };
            let info = entry.info();
            // Only an agent reads a paste as a message: a shell would run it. Its mail waits for `sub.inbox`.
            if !info.alive || info.status != Status::Idle || agents::preset_for(&info.spec.kind).is_none() {
                continue;
            }
            let Some(delivery) = self.subs.mail.lock().plan_delivery(&to) else { continue };
            let delivered = match &entry {
                Entry::Term(s) => match s.type_mail_when_idle(&delivery.text, subs::QUIET) {
                    Some(written) => {
                        // Enter is a separate write, and only if nothing was typed in between (as for a continue timer).
                        let s = s.clone();
                        let _ = std::thread::Builder::new().name("kelpie-sub-enter".into()).spawn(move || {
                            std::thread::sleep(subs::ENTER_AFTER);
                            s.input_if_last_typed_at(written, b"\r".to_vec());
                        });
                        true
                    }
                    None => false,
                },
                // A chat has a message queue the daemon owns, and builds a line from its own origin without any window. Mail
                // waits behind the user's messages and never ends the wait a Stop asked for (the queue refuses it while
                // paused), is not a phone's, and is not dropped when phone access goes off.
                Entry::Chat(c) => c
                    .enqueue(NewMessage {
                        cid: format!("kelpie-mail-{}", delivery.through),
                        by: By::kelpie(),
                        text: delivery.text.clone(),
                        images: Vec::new(),
                        origin: MessageOrigin::Kelpie,
                    })
                    .is_ok(),
            };
            if delivered {
                self.subs.mail.lock().commit(&to, &delivery);
            }
        }
    }
}
