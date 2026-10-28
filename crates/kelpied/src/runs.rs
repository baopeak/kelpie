//! Orchestration runs: a set of agent tasks with dependencies, run by the daemon.
//!
//! A task starts once every task it depends on is done (and, with an approval gate, once the
//! user approves it). It runs as an ordinary agent session — in the run's folder or its own git
//! worktree — with a prompt that carries the run's objective, the results of the tasks it
//! depends on and how to report back: `kelpied run report done|failed "<summary>"` (the session's
//! `KELPIE_SESSION_ID` identifies the task). A task also finishes when its session exits, and,
//! after it worked and then sat idle for `idle_done_secs`, as done. A task that fails or is
//! skipped skips the tasks depending on it; the run ends when every task has.
//!
//! This module is the state machine and storage; the daemon executes its `Action`s.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::persist::write_atomic;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Waiting for the tasks it depends on.
    Pending,
    /// Ready, waiting for the user to approve the start.
    WaitingApproval,
    /// Being started (worktree, session).
    Starting,
    Running,
    Done,
    Failed,
    /// Not run: a task it depends on failed or was skipped.
    Skipped,
    Cancelled,
}

impl TaskStatus {
    pub fn finished(self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Skipped | TaskStatus::Cancelled)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Task {
    /// Short id unique in the run (`t1`, `tests`…); dependencies refer to it.
    pub id: String,
    pub title: String,
    pub prompt: String,
    /// Agent session kind (claude, codex, …).
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub deps: Vec<String>,
    /// Ask the user before starting.
    #[serde(default)]
    pub approve: bool,
    /// Run in its own git worktree (a new branch from the run folder's HEAD).
    #[serde(default)]
    pub worktree: bool,
    /// Run on a saved SSH host, in a folder there, instead of on this computer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<TaskHost>,
    #[serde(default = "pending")]
    pub status: TaskStatus,
    #[serde(default)]
    pub approved: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    /// Folder the task ran in (its worktree, or the run folder).
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    /// The branch the worktree was created from (what Finish merges into).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default)]
    pub started_at: u64,
    #[serde(default)]
    pub finished_at: u64,
    /// What the agent reported (or the end of its output).
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    /// The session has been seen working (idle afterwards may mean done).
    #[serde(default)]
    pub worked: bool,
    /// What its worktree was given from the repository's ignored files (see `Materialize`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brought: Option<kelpie_proto::materialize::Materialized>,
}

/// Ignored files and folders of the repository a run's worktrees get (`.kelpie.yaml` `copy:` and
/// `link:`). The app sends the lists the user approved for that repository; none are brought
/// without them.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Materialize {
    #[serde(default)]
    pub copy: Vec<String>,
    #[serde(default)]
    pub link: Vec<String>,
}

/// Where a task's agent runs when not on this computer: an ssh destination (a saved host's alias or
/// `user@host`) and the project's folder there. The session is started the way any remote session is
/// (`agents::build_remote_launch`), but never in tmux: the task ends when the ssh session does, and with
/// tmux that exit says nothing about the agent (a detach and a crash both exit 0, a dropped connection
/// leaves the agent running while the task would be retried). Without it the agent ends with its connection once the
/// host notices that it is gone, which can take minutes: a task that failed with its connection may still have an agent
/// running there for a while (the app asks before starting it again).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskHost {
    pub dest: String,
    #[serde(default)]
    pub port: Option<u16>,
    /// `ssh -F` config file (Kelpie's generated one, which holds the saved hosts).
    #[serde(default)]
    pub config: Option<String>,
    /// The absolute folder on the host the agent starts in.
    pub path: String,
}

impl TaskHost {
    /// What `ssh` will be handed: nothing that could be read as an option, a second word or a quote.
    pub fn validate(&self) -> Result<()> {
        let d = &self.dest;
        anyhow::ensure!(
            !d.is_empty() && d.len() <= 255 && !d.starts_with('-') && d.chars().all(|c| c.is_ascii_alphanumeric() || "._@:%[]-".contains(c)),
            "invalid ssh destination `{d}`"
        );
        anyhow::ensure!(self.port != Some(0), "invalid ssh port");
        anyhow::ensure!(self.path.starts_with('/') && !self.path.chars().any(char::is_control), "a task on a host needs the absolute folder there");
        if let Some(c) = &self.config {
            anyhow::ensure!(!c.is_empty() && !c.starts_with('-') && !c.chars().any(char::is_control), "invalid ssh config path");
        }
        Ok(())
    }

    /// The `remote` of the task's session.
    pub fn remote(&self) -> crate::session::RemoteSpec {
        crate::session::RemoteSpec { dest: self.dest.clone(), port: self.port, config: self.config.clone(), tmux: false }
    }
}

fn default_kind() -> String {
    "claude".into()
}
fn pending() -> TaskStatus {
    TaskStatus::Pending
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Run {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub objective: String,
    pub cwd: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub tasks: Vec<Task>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub materialize: Option<Materialize>,
    pub status: RunStatus,
    pub created_at: u64,
    #[serde(default)]
    pub finished_at: u64,
    /// A running task whose agent worked and then stayed idle this long counts as done
    /// (0 = only a report or the session exiting ends it).
    #[serde(default = "default_idle")]
    pub idle_done_secs: u64,
}

fn default_idle() -> u64 {
    300
}

/// What the daemon must do for the run engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Prepare the folder and start the task's session.
    Start { run: String, task: String },
    /// The task ended: keep the end of its output as its summary if it reported none.
    Capture { run: String, task: String, session: String },
    /// The session is not to be resumed by the app: it belonged to a task on a host that was already marked failed.
    Release { session: String },
}

/// A task's session as the engine needs it.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub alive: bool,
    pub working: bool,
    pub idle: bool,
    pub exit_code: Option<i32>,
    pub last_activity: u64,
    /// Restored after the daemon stopped while it ran (the app resumes such sessions).
    pub interrupted: bool,
}

pub struct Runs {
    path: PathBuf,
    pub list: Vec<Run>,
}

/// A run as created by the user, a coordinator agent or `kelpied run create`.
#[derive(Debug, Deserialize)]
pub struct NewRun {
    pub name: String,
    #[serde(default)]
    pub objective: String,
    pub cwd: String,
    #[serde(default)]
    pub project_id: Option<String>,
    pub tasks: Vec<NewTask>,
    #[serde(default)]
    pub idle_done_secs: Option<u64>,
    #[serde(default)]
    pub materialize: Option<Materialize>,
}

#[derive(Debug, Deserialize)]
pub struct NewTask {
    #[serde(default)]
    pub id: Option<String>,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub deps: Vec<String>,
    #[serde(default)]
    pub approve: bool,
    #[serde(default)]
    pub worktree: bool,
    /// Run on an SSH host (API level 14). Programs in a session cannot create runs that name one (see `auth`);
    /// they can still start an ssh session of their own through `session.create`.
    #[serde(default)]
    pub host: Option<TaskHost>,
}

/// Validates ids and dependencies (known, no cycles) of a new run.
pub fn build_run(new: NewRun, id: String, now: u64) -> Result<Run> {
    if new.name.trim().is_empty() || new.cwd.trim().is_empty() {
        bail!("a run needs a name and a folder");
    }
    if new.tasks.is_empty() {
        bail!("a run needs at least one task");
    }
    let mut tasks = Vec::new();
    let mut seen = HashSet::new();
    for (i, t) in new.tasks.into_iter().enumerate() {
        let tid = t.id.filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("t{}", i + 1));
        if !tid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || tid.len() > 40 {
            bail!("task id `{tid}`: letters, digits, - and _ only");
        }
        if !seen.insert(tid.clone()) {
            bail!("two tasks have the id `{tid}`");
        }
        if t.title.trim().is_empty() || t.prompt.trim().is_empty() {
            bail!("task `{tid}` needs a title and a prompt");
        }
        if let Some(h) = &t.host {
            h.validate().map_err(|e| anyhow::anyhow!("task `{tid}`: {e}"))?;
            if t.worktree {
                bail!("task `{tid}`: a worktree is made on this computer, so a task on a host cannot have one of its own");
            }
        }
        tasks.push(Task {
            id: tid,
            title: t.title.trim().to_string(),
            prompt: t.prompt,
            kind: t.kind.filter(|k| !k.is_empty()).unwrap_or_else(default_kind),
            args: t.args,
            env: t.env,
            deps: t.deps,
            approve: t.approve,
            worktree: t.worktree,
            host: t.host,
            status: TaskStatus::Pending,
            approved: false,
            session_id: None,
            cwd: None,
            branch: None,
            base: None,
            started_at: 0,
            finished_at: 0,
            summary: None,
            error: None,
            worked: false,
            brought: None,
        });
    }
    for t in &tasks {
        for d in &t.deps {
            if !seen.contains(d) {
                bail!("task `{}` depends on unknown task `{d}`", t.id);
            }
            if d == &t.id {
                bail!("task `{}` depends on itself", t.id);
            }
        }
    }
    if let Some(cycle) = find_cycle(&tasks) {
        bail!("tasks depend on each other in a circle: {}", cycle.join(" → "));
    }
    Ok(Run {
        id,
        name: new.name.trim().to_string(),
        objective: new.objective,
        cwd: new.cwd,
        project_id: new.project_id,
        tasks,
        materialize: new.materialize,
        status: RunStatus::Running,
        created_at: now,
        finished_at: 0,
        idle_done_secs: new.idle_done_secs.unwrap_or_else(default_idle),
    })
}

fn find_cycle(tasks: &[Task]) -> Option<Vec<String>> {
    let deps: HashMap<&str, &[String]> = tasks.iter().map(|t| (t.id.as_str(), t.deps.as_slice())).collect();
    // 0 = unvisited, 1 = on the stack, 2 = done
    let mut state: HashMap<&str, u8> = HashMap::new();
    fn visit<'a>(n: &'a str, deps: &HashMap<&'a str, &'a [String]>, state: &mut HashMap<&'a str, u8>, path: &mut Vec<&'a str>) -> Option<Vec<String>> {
        match state.get(n) {
            Some(2) => return None,
            Some(1) => {
                let start = path.iter().position(|p| *p == n).unwrap_or(0);
                let mut c: Vec<String> = path[start..].iter().map(|s| s.to_string()).collect();
                c.push(n.to_string());
                return Some(c);
            }
            _ => {}
        }
        state.insert(n, 1);
        path.push(n);
        for d in deps.get(n).copied().unwrap_or_default() {
            if let Some(c) = visit(d.as_str(), deps, state, path) {
                return Some(c);
            }
        }
        path.pop();
        state.insert(n, 2);
        None
    }
    for t in tasks {
        if let Some(c) = visit(t.id.as_str(), &deps, &mut state, &mut Vec::new()) {
            return Some(c);
        }
    }
    None
}

impl Runs {
    pub fn load(dir: &Path) -> Runs {
        let path = dir.join("runs.json");
        let mut list: Vec<Run> = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        // Tasks caught half-started by a daemon restart start again.
        for r in &mut list {
            for t in &mut r.tasks {
                if t.status == TaskStatus::Starting {
                    t.status = TaskStatus::Pending;
                }
            }
        }
        Runs { path, list }
    }

    pub fn save(&self) -> Result<()> {
        write_atomic(&self.path, &serde_json::to_vec_pretty(&self.list)?)
    }

    pub fn get(&self, id: &str) -> Option<&Run> {
        self.list.iter().find(|r| r.id == id || (id.len() >= 4 && r.id.starts_with(id)))
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Run> {
        self.list.iter_mut().find(|r| r.id == id || (id.len() >= 4 && r.id.starts_with(id)))
    }

    pub fn task_mut(&mut self, run: &str, task: &str) -> Result<(&mut Task, String)> {
        let r = self.get_mut(run).ok_or_else(|| anyhow::anyhow!("no run {run}"))?;
        let rid = r.id.clone();
        let t = r.tasks.iter_mut().find(|t| t.id == task).ok_or_else(|| anyhow::anyhow!("no task {task} in run {rid}"))?;
        Ok((t, rid))
    }

    /// The running task whose agent session is `session`.
    pub fn by_session(&mut self, session: &str) -> Option<(&mut Run, usize)> {
        for r in &mut self.list {
            if let Some(i) = r.tasks.iter().position(|t| t.session_id.as_deref() == Some(session) && !t.status.finished()) {
                return Some((r, i));
            }
        }
        None
    }

    /// Right after a restart, before any client is served: the running tasks on a host whose session came back interrupted fail
    /// now (see `fail_after_restart`), so that an app resuming sessions at once cannot restart their agents first. Returns the
    /// sessions to release and the ids of the runs that changed.
    pub fn settle_interrupted_hosts(&mut self, now: u64, view: impl Fn(&str) -> Option<SessionView>) -> (Vec<String>, Vec<String>) {
        let (mut sessions, mut changed) = (Vec::new(), Vec::new());
        for run in self.list.iter_mut().filter(|r| r.status == RunStatus::Running) {
            let mut touched = false;
            for t in run.tasks.iter_mut().filter(|t| t.status == TaskStatus::Running && t.host.is_some()) {
                let Some(sid) = t.session_id.clone() else { continue };
                if view(&sid).is_some_and(|s| !s.alive && s.interrupted) {
                    fail_after_restart(t, now);
                    sessions.push(sid);
                    touched = true;
                }
            }
            if touched {
                changed.push(run.id.clone());
            }
        }
        (sessions, changed)
    }

    /// Advances every running run. Returns what to do and the ids of runs that changed.
    pub fn step(&mut self, now: u64, view: impl Fn(&str) -> Option<SessionView>) -> (Vec<Action>, Vec<String>) {
        let mut actions = Vec::new();
        let mut changed = Vec::new();
        for run in self.list.iter_mut().filter(|r| r.status == RunStatus::Running) {
            let before = run.clone();
            for t in run.tasks.iter_mut().filter(|t| t.status == TaskStatus::Running) {
                let Some(sid) = t.session_id.clone() else {
                    t.status = TaskStatus::Failed;
                    t.error = Some("no session".into());
                    t.finished_at = now;
                    continue;
                };
                match view(&sid) {
                    None => {
                        t.status = TaskStatus::Failed;
                        t.error = Some("its session was closed".into());
                        t.finished_at = now;
                    }
                    Some(s) if !s.alive && s.interrupted && t.host.is_some() => {
                        fail_after_restart(t, now);
                        actions.push(Action::Capture { run: run.id.clone(), task: t.id.clone(), session: sid.clone() });
                        actions.push(Action::Release { session: sid });
                    }
                    Some(s) if !s.alive => {
                        if s.exit_code == Some(0) {
                            t.status = TaskStatus::Done;
                        } else {
                            t.status = TaskStatus::Failed;
                            t.error = Some(match (&t.host, s.exit_code) {
                                // ssh itself says 255 when the connection fails or drops; without tmux the agent ends with it, in time.
                                (Some(h), Some(255) | None) => format!("the agent on {} stopped with the connection", h.dest),
                                (Some(h), Some(c)) => format!("the agent on {} exited with code {c}", h.dest),
                                (None, Some(c)) => format!("the agent exited with code {c}"),
                                (None, None) => "the agent stopped".into(),
                            });
                        }
                        t.finished_at = now;
                        actions.push(Action::Capture { run: run.id.clone(), task: t.id.clone(), session: sid });
                    }
                    Some(s) => {
                        if s.working {
                            t.worked = true;
                        }
                        let quiet = now.saturating_sub(s.last_activity.max(t.started_at));
                        // A task on a host ends with its ssh session only: idle says nothing about an agent there, and a task
                        // that is "done" while its agent still runs would invite a second one.
                        if run.idle_done_secs > 0 && t.host.is_none() && t.worked && s.idle && quiet >= run.idle_done_secs * 1000 {
                            t.status = TaskStatus::Done;
                            t.finished_at = now;
                            actions.push(Action::Capture { run: run.id.clone(), task: t.id.clone(), session: sid });
                        }
                    }
                }
            }
            // Waiting tasks, until nothing changes: a failure skips its whole chain at once.
            loop {
                let status_of: HashMap<String, TaskStatus> = run.tasks.iter().map(|t| (t.id.clone(), t.status)).collect();
                let mut moved = false;
                for t in run.tasks.iter_mut().filter(|t| matches!(t.status, TaskStatus::Pending | TaskStatus::WaitingApproval)) {
                    let deps: Vec<TaskStatus> = t.deps.iter().filter_map(|d| status_of.get(d).copied()).collect();
                    if deps.iter().any(|s| matches!(s, TaskStatus::Failed | TaskStatus::Skipped | TaskStatus::Cancelled)) {
                        t.status = TaskStatus::Skipped;
                        t.error = Some("a task it depends on did not finish".into());
                        t.finished_at = now;
                        moved = true;
                    } else if deps.iter().all(|s| *s == TaskStatus::Done) {
                        if t.approve && !t.approved {
                            if t.status != TaskStatus::WaitingApproval {
                                t.status = TaskStatus::WaitingApproval;
                                moved = true;
                            }
                        } else {
                            t.status = TaskStatus::Starting;
                            actions.push(Action::Start { run: run.id.clone(), task: t.id.clone() });
                            moved = true;
                        }
                    }
                }
                if !moved {
                    break;
                }
            }
            if run.tasks.iter().all(|t| t.status.finished()) {
                run.status = if run.tasks.iter().any(|t| t.status == TaskStatus::Failed) { RunStatus::Failed } else { RunStatus::Done };
                run.finished_at = now;
            }
            if *run != before {
                changed.push(run.id.clone());
            }
        }
        (actions, changed)
    }
}

/// The daemon stopped while the agent ran on the host. Resuming it would start a second agent that no task follows, while the
/// first may still run there: the task fails, and a retry is the user's to confirm.
fn fail_after_restart(t: &mut Task, now: u64) {
    let dest = t.host.as_ref().map_or("", |h| h.dest.as_str()).to_string();
    t.status = TaskStatus::Failed;
    t.error = Some(format!("Kelpie restarted while the agent ran on {dest}: it may still be running there for a few minutes; check before starting it again"));
    t.finished_at = now;
}

/// The prompt a task's agent starts with.
pub fn task_prompt(run: &Run, task: &Task, kelpied: &Path) -> String {
    let mut p = task.prompt.trim().to_string();
    p.push_str("\n\n---\n");
    p.push_str(&format!("This is task \"{}\" ({}) of the run \"{}\"", task.title, task.id, run.name));
    if !run.objective.trim().is_empty() {
        p.push_str(&format!(", whose overall objective is: {}", run.objective.trim()));
    }
    p.push_str(".\n");
    let deps: Vec<&Task> = task.deps.iter().filter_map(|d| run.tasks.iter().find(|t| &t.id == d)).collect();
    if !deps.is_empty() {
        p.push_str("\nResults of the tasks this one builds on:\n");
        for d in deps {
            let summary = d.summary.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("done (no summary)");
            let place = d.branch.as_deref().map(|b| format!(" [branch {b}]")).unwrap_or_default();
            p.push_str(&format!("- {} ({}){place}: {}\n", d.title, d.id, summary));
        }
    }
    // The report command is a program of this computer: an agent on a host cannot run it. Its task ends when
    // its session does (or by hand): never on idle.
    if task.host.is_some() {
        return p;
    }
    let exe = kelpied.to_string_lossy().replace('\\', "/");
    p.push_str(&format!(
        "\nWhen the task is finished, report it by running this shell command (PowerShell needs `&` in front):\n  \"{exe}\" run report done \"<one-line summary of the result>\"\nIf you cannot complete it, run:\n  \"{exe}\" run report failed \"<the reason>\"\n"
    ));
    p
}

/// `name` as a short branch / folder slug.
pub fn slug(name: &str) -> String {
    let s: String = name.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let s = s.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
    let s: String = s.chars().take(24).collect();
    if s.is_empty() { "run".into() } else { s.trim_end_matches('-').to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_task(id: &str, deps: &[&str]) -> NewTask {
        NewTask { id: Some(id.into()), title: id.into(), prompt: format!("do {id}"), kind: None, args: vec![], env: BTreeMap::new(), deps: deps.iter().map(|s| s.to_string()).collect(), approve: false, worktree: false, host: None }
    }

    fn run_of(tasks: Vec<NewTask>) -> Run {
        build_run(NewRun { name: "R".into(), objective: "obj".into(), cwd: ".".into(), project_id: None, tasks, idle_done_secs: Some(60), materialize: None }, "r1".into(), 0).unwrap()
    }

    fn view(alive: bool, working: bool, exit: Option<i32>, last: u64) -> Option<SessionView> {
        Some(SessionView { alive, working, idle: !working && alive, exit_code: exit, last_activity: last, interrupted: false })
    }

    #[test]
    fn rejects_bad_graphs() {
        let bad = |tasks| build_run(NewRun { name: "R".into(), objective: String::new(), cwd: ".".into(), project_id: None, tasks, idle_done_secs: None, materialize: None }, "x".into(), 0).unwrap_err().to_string();
        assert!(bad(vec![new_task("a", &["b"]), new_task("b", &["a"])]).contains("circle"));
        assert!(bad(vec![new_task("a", &["zzz"])]).contains("unknown"));
        assert!(bad(vec![new_task("a", &[]), new_task("a", &[])]).contains("two tasks"));
        assert!(bad(vec![]).contains("at least one"));
    }

    #[test]
    fn dependencies_gate_starts_and_failures_skip() {
        let mut runs = Runs { path: PathBuf::new(), list: vec![run_of(vec![new_task("a", &[]), new_task("b", &["a"]), new_task("c", &["b"])])] };
        let (acts, _) = runs.step(1000, |_| None);
        assert_eq!(acts, vec![Action::Start { run: "r1".into(), task: "a".into() }]);
        // The daemon started `a`.
        {
            let (t, _) = runs.task_mut("r1", "a").unwrap();
            t.status = TaskStatus::Running;
            t.session_id = Some("s-a".into());
            t.started_at = 1000;
        }
        let (acts, _) = runs.step(2000, |_| view(true, true, None, 2000));
        assert!(acts.is_empty());
        // `a` exits with an error: `b` and `c` are skipped, the run fails.
        let (acts, changed) = runs.step(3000, |_| view(false, false, Some(2), 2500));
        assert_eq!(acts, vec![Action::Capture { run: "r1".into(), task: "a".into(), session: "s-a".into() }]);
        assert_eq!(changed, vec!["r1".to_string()]);
        let _ = runs.step(3100, |_| None);
        let r = &runs.list[0];
        assert_eq!(r.tasks.iter().map(|t| t.status).collect::<Vec<_>>(), vec![TaskStatus::Failed, TaskStatus::Skipped, TaskStatus::Skipped]);
        assert_eq!(r.status, RunStatus::Failed);
    }

    #[test]
    fn approval_gate_and_idle_completion() {
        let mut t2 = new_task("b", &["a"]);
        t2.approve = true;
        let mut runs = Runs { path: PathBuf::new(), list: vec![run_of(vec![new_task("a", &[]), t2])] };
        let _ = runs.step(0, |_| None);
        {
            let (t, _) = runs.task_mut("r1", "a").unwrap();
            t.status = TaskStatus::Running;
            t.session_id = Some("s".into());
            t.started_at = 0;
        }
        // Idle before it ever worked: not done.
        let _ = runs.step(120_000, |_| view(true, false, None, 0));
        assert_eq!(runs.list[0].tasks[0].status, TaskStatus::Running);
        let _ = runs.step(130_000, |_| view(true, true, None, 130_000));
        // Worked, then idle 59 s: still running; 60 s: done.
        let _ = runs.step(189_000, |_| view(true, false, None, 130_000));
        assert_eq!(runs.list[0].tasks[0].status, TaskStatus::Running);
        let (acts, _) = runs.step(190_000, |_| view(true, false, None, 130_000));
        assert_eq!(runs.list[0].tasks[0].status, TaskStatus::Done);
        assert_eq!(acts.len(), 1);
        // `b` waits for approval, then starts.
        let (acts, _) = runs.step(191_000, |_| None);
        assert!(acts.is_empty());
        assert_eq!(runs.list[0].tasks[1].status, TaskStatus::WaitingApproval);
        runs.task_mut("r1", "b").unwrap().0.approved = true;
        let (acts, _) = runs.step(192_000, |_| None);
        assert_eq!(acts, vec![Action::Start { run: "r1".into(), task: "b".into() }]);
    }

    #[test]
    fn prompt_carries_objective_dependency_results_and_report_command() {
        let mut r = run_of(vec![new_task("a", &[]), new_task("b", &["a"])]);
        r.tasks[0].summary = Some("added the parser".into());
        r.tasks[0].branch = Some("kelpie/r-a".into());
        let p = task_prompt(&r, &r.tasks[1], Path::new("C:\\bin\\kelpied.exe"));
        assert!(p.starts_with("do b"));
        assert!(p.contains("objective is: obj"));
        assert!(p.contains("- a (a) [branch kelpie/r-a]: added the parser"));
        assert!(p.contains("\"C:/bin/kelpied.exe\" run report done"));
    }

    #[test]
    fn slugs() {
        assert_eq!(slug("Fix the Login bug!"), "fix-the-login-bug");
        assert_eq!(slug("***"), "run");
    }

    fn host() -> TaskHost {
        TaskHost { dest: "devbox".into(), port: Some(2222), config: Some("/cfg/ssh_config".into()), path: "/srv/app".into() }
    }

    fn hosted(id: &str) -> NewTask {
        NewTask { host: Some(host()), ..new_task(id, &[]) }
    }

    fn build(tasks: Vec<NewTask>) -> Result<Run> {
        build_run(NewRun { name: "R".into(), objective: String::new(), cwd: "C:/proj".into(), project_id: None, tasks, idle_done_secs: None, materialize: None }, "r1".into(), 0)
    }

    #[test]
    fn a_task_keeps_its_host_and_runs_made_before_hosts_have_none() {
        let run = build(vec![hosted("a"), new_task("b", &[])]).unwrap();
        assert_eq!(run.tasks[0].host, Some(host()));
        assert_eq!(run.tasks[1].host, None);
        let json = serde_json::to_value(&run).unwrap();
        assert_eq!(json["tasks"][0]["host"]["dest"], "devbox");
        assert!(json["tasks"][1].get("host").is_none(), "a local task does not mention hosts");
        // runs.json written before hosts existed.
        let mut old = json.clone();
        old["tasks"][0].as_object_mut().unwrap().remove("host");
        let back: Run = serde_json::from_value(old).unwrap();
        assert!(back.tasks.iter().all(|t| t.host.is_none()));
        // A request that names a host in the app's own wording.
        let new: NewRun = serde_json::from_value(serde_json::json!({"name": "R", "cwd": "/p", "tasks": [{"title": "t", "prompt": "p", "host": {"dest": "h", "path": "/x"}}]})).unwrap();
        assert_eq!(new.tasks[0].host.as_ref().map(|h| (h.port, h.path.as_str())), Some((None, "/x")));
    }

    #[test]
    fn a_host_is_checked_before_ssh_is_handed_anything() {
        let bad = |edit: &dyn Fn(&mut TaskHost)| {
            let mut h = host();
            edit(&mut h);
            build(vec![NewTask { host: Some(h), ..new_task("a", &[]) }]).unwrap_err().to_string()
        };
        assert!(bad(&|h| h.dest = "-oProxyCommand=calc".into()).contains("invalid ssh destination"));
        assert!(bad(&|h| h.dest = String::new()).contains("invalid ssh destination"));
        assert!(bad(&|h| h.dest = "box; rm -rf /".into()).contains("invalid ssh destination"));
        assert!(bad(&|h| h.dest = "a b".into()).contains("invalid ssh destination"));
        assert!(bad(&|h| h.dest = "host\n".into()).contains("invalid ssh destination"));
        assert!(bad(&|h| h.port = Some(0)).contains("port"));
        assert!(bad(&|h| h.path = "relative/dir".into()).contains("absolute folder"));
        assert!(bad(&|h| h.path = "~/x".into()).contains("absolute folder"));
        assert!(bad(&|h| h.path = "/a\nb".into()).contains("absolute folder"));
        assert!(bad(&|h| h.config = Some("-F".into())).contains("config"));
        assert!(bad(&|h| h.config = Some(String::new())).contains("config"));
        for ok in ["devbox", "me@devbox.example.com", "[fe80::1]", "10.0.0.5", "kelpie-host_1"] {
            let mut h = host();
            h.dest = ok.into();
            assert!(build(vec![NewTask { host: Some(h), ..new_task("a", &[]) }]).is_ok(), "{ok}");
        }
    }

    #[test]
    fn a_task_on_a_host_cannot_have_a_worktree() {
        let err = build(vec![NewTask { worktree: true, ..hosted("a") }]).unwrap_err().to_string();
        assert!(err.contains("task `a`") && err.contains("cannot have one of its own"), "{err}");
    }

    #[test]
    fn the_prompt_of_a_task_on_a_host_has_no_report_command_of_this_computer() {
        let run = build(vec![hosted("a"), new_task("b", &[])]).unwrap();
        let exe = Path::new("C:\\bin\\kelpied.exe");
        let on_host = task_prompt(&run, &run.tasks[0], exe);
        assert!(on_host.contains("This is task") && !on_host.contains("run report") && !on_host.contains("kelpied.exe"), "{on_host}");
        assert!(task_prompt(&run, &run.tasks[1], exe).contains("run report done"));
    }

    #[test]
    fn a_task_on_a_host_starts_like_any_remote_session() {
        let h = host();
        let l = crate::agents::build_remote_launch("claude", &[], crate::agents::Resume::None, Some("do it"), &h.remote(), &h.path, "sess-1").unwrap();
        let line = l.argv.join(" ");
        assert!(l.argv.iter().any(|a| a.to_lowercase().contains("ssh")), "{line}");
        assert!(line.contains("-F /cfg/ssh_config") && line.contains("-p 2222") && line.contains("devbox"), "{line}");
        assert!(line.contains("/srv/app") && !line.contains("tmux"), "a task on a host never starts in tmux: {line}");
        assert!(line.contains("do it"), "the prompt goes on the host's command line: {line}");
    }

    #[test]
    fn a_task_on_a_host_ends_with_its_ssh_session() {
        let outcome = |exit: Option<i32>| {
            let mut runs = Runs { path: PathBuf::new(), list: vec![build(vec![hosted("a")]).unwrap()] };
            let _ = runs.step(0, |_| None);
            let (t, _) = runs.task_mut("r1", "a").unwrap();
            t.status = TaskStatus::Running;
            t.session_id = Some("s".into());
            let _ = runs.step(10, |_| view(false, false, exit, 5));
            let t = &runs.list[0].tasks[0];
            (t.status, t.error.clone())
        };
        assert_eq!(outcome(Some(0)), (TaskStatus::Done, None));
        // Idle after working is not an end for a task on a host.
        let mut runs = Runs { path: PathBuf::new(), list: vec![build(vec![hosted("a")]).unwrap()] };
        runs.list[0].idle_done_secs = 60;
        let _ = runs.step(0, |_| None);
        let (t, _) = runs.task_mut("r1", "a").unwrap();
        t.status = TaskStatus::Running;
        t.session_id = Some("s".into());
        let _ = runs.step(130_000, |_| view(true, true, None, 130_000));
        let _ = runs.step(400_000, |_| view(true, false, None, 130_000));
        assert_eq!(runs.list[0].tasks[0].status, TaskStatus::Running);
        let (status, error) = outcome(Some(255));
        assert_eq!(status, TaskStatus::Failed);
        assert!(error.as_deref().unwrap().contains("stopped with the connection") && error.unwrap().contains("devbox"));
        assert!(outcome(None).1.unwrap().contains("stopped with the connection"));
        let (status, error) = outcome(Some(2));
        assert_eq!(status, TaskStatus::Failed);
        assert!(error.unwrap().contains("exited with code 2"));
        // A local task keeps its wording.
        let mut local = Runs { path: PathBuf::new(), list: vec![run_of(vec![new_task("a", &[])])] };
        let _ = local.step(0, |_| None);
        let (t, _) = local.task_mut("r1", "a").unwrap();
        t.status = TaskStatus::Running;
        t.session_id = Some("s".into());
        let _ = local.step(10, |_| view(false, false, Some(255), 5));
        assert_eq!(local.list[0].tasks[0].error.as_deref(), Some("the agent exited with code 255"));
    }

    #[test]
    fn a_task_on_a_host_never_asks_for_tmux() {
        let mut h = host();
        h.config = None;
        assert!(!h.remote().tmux);
        // An older app that still sends the field is understood, and it changes nothing.
        let h: TaskHost = serde_json::from_value(serde_json::json!({"dest": "d", "path": "/x", "tmux": true})).unwrap();
        assert!(!h.remote().tmux);
    }

    #[test]
    fn a_host_task_interrupted_by_a_restart_fails_and_is_not_resumed() {
        let run_with = |host: bool| {
            let mut runs = Runs { path: PathBuf::new(), list: vec![build(vec![if host { hosted("a") } else { new_task("a", &[]) }]).unwrap()] };
            let _ = runs.step(0, |_| None);
            let (t, _) = runs.task_mut("r1", "a").unwrap();
            t.status = TaskStatus::Running;
            t.session_id = Some("s".into());
            let restored = |_: &str| Some(SessionView { alive: false, working: false, idle: false, exit_code: None, last_activity: 5, interrupted: true });
            let (actions, _) = runs.step(10, restored);
            (runs, actions)
        };
        let (runs, actions) = run_with(true);
        let t = &runs.list[0].tasks[0];
        assert_eq!(t.status, TaskStatus::Failed);
        assert!(t.error.as_deref().unwrap().contains("still be running there") && t.error.as_deref().unwrap().contains("devbox"));
        assert!(actions.contains(&Action::Release { session: "s".into() }));
        // A local task keeps the old behaviour: the app may resume its session.
        let (runs, actions) = run_with(false);
        assert_eq!(runs.list[0].tasks[0].status, TaskStatus::Failed);
        assert!(!actions.iter().any(|a| matches!(a, Action::Release { .. })));
    }

    #[test]
    fn settling_after_a_restart_fails_only_the_host_tasks_whose_session_came_back_interrupted() {
        let mut runs = Runs { path: PathBuf::new(), list: vec![build(vec![hosted("a"), hosted("b"), new_task("c", &[])]).unwrap()] };
        let _ = runs.step(0, |_| None);
        for (id, sid) in [("a", "sa"), ("b", "sb"), ("c", "sc")] {
            let (t, _) = runs.task_mut("r1", id).unwrap();
            t.status = TaskStatus::Running;
            t.session_id = Some(sid.into());
        }
        // `a` came back interrupted; `b` is somehow alive; `c` is local and interrupted (the app may resume it).
        let view = |sid: &str| Some(SessionView { alive: sid == "sb", working: false, idle: false, exit_code: None, last_activity: 5, interrupted: sid != "sb" });
        let (sessions, changed) = runs.settle_interrupted_hosts(10, view);
        assert_eq!((sessions, changed), (vec!["sa".to_string()], vec!["r1".to_string()]));
        let status = |id: &str| runs.list[0].tasks.iter().find(|t| t.id == id).map(|t| t.status).unwrap();
        assert_eq!((status("a"), status("b"), status("c")), (TaskStatus::Failed, TaskStatus::Running, TaskStatus::Running));
        assert!(runs.list[0].tasks[0].error.as_deref().unwrap().contains("Kelpie restarted"));
        // Nothing left to settle: the next tick has nothing to release.
        assert_eq!(runs.settle_interrupted_hosts(11, view), (vec![], vec![]));
    }
}
