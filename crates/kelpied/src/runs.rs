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
}

/// A task's session as the engine needs it.
#[derive(Debug, Clone)]
pub struct SessionView {
    pub alive: bool,
    pub working: bool,
    pub idle: bool,
    pub exit_code: Option<i32>,
    pub last_activity: u64,
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
                    Some(s) if !s.alive => {
                        if s.exit_code == Some(0) {
                            t.status = TaskStatus::Done;
                        } else {
                            t.status = TaskStatus::Failed;
                            t.error = Some(match s.exit_code {
                                Some(c) => format!("the agent exited with code {c}"),
                                None => "the agent stopped".into(),
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
                        if run.idle_done_secs > 0 && t.worked && s.idle && quiet >= run.idle_done_secs * 1000 {
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
        NewTask { id: Some(id.into()), title: id.into(), prompt: format!("do {id}"), kind: None, args: vec![], env: BTreeMap::new(), deps: deps.iter().map(|s| s.to_string()).collect(), approve: false, worktree: false }
    }

    fn run_of(tasks: Vec<NewTask>) -> Run {
        build_run(NewRun { name: "R".into(), objective: "obj".into(), cwd: ".".into(), project_id: None, tasks, idle_done_secs: Some(60), materialize: None }, "r1".into(), 0).unwrap()
    }

    fn view(alive: bool, working: bool, exit: Option<i32>, last: u64) -> Option<SessionView> {
        Some(SessionView { alive, working, idle: !working && alive, exit_code: exit, last_activity: last })
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
}
