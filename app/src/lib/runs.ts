// Orchestration runs, kept in sync with the daemon (crates/kelpied/src/runs.rs): the list, the
// actions on runs and tasks, activity entries for what needs the user, worktree projects for
// tasks that run in their own worktree, and planning a run's tasks with an AI agent.
import { createStore, produce, reconcile } from "solid-js/store";
import { agentRun, daemon, isDir, onEvent } from "./ipc";
import { normPath } from "./platform";
import { store } from "./store";
import { msg, t } from "./i18n";
import type { SessionKind } from "./types";

export type RunStatus = "running" | "done" | "failed" | "cancelled";
export type TaskStatus = "pending" | "waiting_approval" | "starting" | "running" | "done" | "failed" | "skipped" | "cancelled";

/** What a worktree was given from the repository's ignored files (the daemon's `Materialized`). */
export interface Brought {
  linked: string[];
  copied: string[];
  skipped: { path: string; reason: string }[];
}

export const STATUS_LABEL: Record<string, string> = {
  pending: msg("Waiting"), waiting_approval: msg("Needs approval"), starting: msg("Starting"), running: msg("Running"), done: msg("Done"), failed: msg("Failed"), skipped: msg("Skipped"), cancelled: msg("Cancelled"),
};

/** The reasons the daemon gives for leaving an entry out, in the user's language. */
const REASONS: Record<string, string> = {
  "not a plain relative path": msg("not a plain relative path"),
  "missing in the repository": msg("missing in the repository"),
  "already in the worktree": msg("already in the worktree"),
  "not ignored by git (tracked files are already checked out)": msg("not ignored by git (tracked files are already checked out)"),
  "only folders can be linked": msg("only folders can be linked"),
};

/** One line about what a worktree was given (the full text, with the reasons, is the tooltip). */
export function broughtText(b: Brought): { short: string; full: string } {
  const parts = [b.linked.length ? t("linked {files}", { files: b.linked.join(", ") }) : "", b.copied.length ? t("copied {files}", { files: b.copied.join(", ") }) : ""].filter(Boolean);
  const left = b.skipped.map((s) => `${s.path} (${REASONS[s.reason] ? t(REASONS[s.reason]) : s.reason})`);
  return { short: parts.join(" · ") || t("nothing brought"), full: [...parts, ...(left.length ? [t("not brought: {files}", { files: left.join("; ") })] : [])].join(" · ") };
}

export interface RunTask {
  id: string;
  title: string;
  prompt: string;
  kind: SessionKind;
  args: string[];
  env: Record<string, string>;
  deps: string[];
  approve: boolean;
  worktree: boolean;
  status: TaskStatus;
  approved: boolean;
  session_id: string | null;
  cwd: string | null;
  branch: string | null;
  /** The branch the worktree was created from. */
  base?: string;
  started_at: number;
  finished_at: number;
  summary: string | null;
  error: string | null;
  worked: boolean;
  /** What its worktree was given from the repository's ignored files (copies, links, what was left out). */
  brought?: Brought;
}

export interface Run {
  id: string;
  name: string;
  objective: string;
  cwd: string;
  project_id: string | null;
  tasks: RunTask[];
  /** The ignored files and folders each task worktree gets: what the user approved in `.kelpie.yaml`. */
  materialize?: { copy: string[]; link: string[] };
  status: RunStatus;
  created_at: number;
  finished_at: number;
  idle_done_secs: number;
}

/** A task as written in the editor or by a planning agent. */
export interface TaskDraft {
  id: string;
  title: string;
  prompt: string;
  kind: SessionKind;
  deps: string[];
  approve: boolean;
  worktree: boolean;
}

const [runs, setRuns] = createStore<Record<string, Run>>({});
export const runsById = runs;

export const runList = () => Object.values(runs).sort((a, b) => b.created_at - a.created_at);
export const activeRuns = () => runList().filter((r) => r.status === "running");
export const finishedTask = (s: TaskStatus) => s === "done" || s === "failed" || s === "skipped" || s === "cancelled";

/** Tasks grouped by depth: a task sits one column right of the deepest task it waits for. */
export function lanes(run: Run): RunTask[][] {
  const depth = new Map<string, number>();
  const of = (t: RunTask, seen = new Set<string>()): number => {
    if (depth.has(t.id)) return depth.get(t.id)!;
    if (seen.has(t.id)) return 0;
    seen.add(t.id);
    const d = Math.max(-1, ...t.deps.map((id) => run.tasks.find((x) => x.id === id)).filter((x): x is RunTask => !!x).map((x) => of(x, seen))) + 1;
    depth.set(t.id, d);
    return d;
  };
  const out: RunTask[][] = [];
  for (const t of run.tasks) (out[of(t)] ??= []).push(t);
  return out.filter(Boolean);
}

export async function loadRuns() {
  const list = await daemon<Run[]>("run.list").catch(() => null);
  if (!list) return;
  const map: Record<string, Run> = {};
  for (const r of list) map[r.id] = r;
  setRuns(reconcile(map));
}

export const createRun = (spec: { name: string; objective: string; cwd: string; project_id: string | null; tasks: (TaskDraft & { env?: Record<string, string>; args?: string[] })[]; idle_done_secs?: number; materialize?: { copy: string[]; link: string[] } }) =>
  daemon<Run>("run.create", spec);

export type RunAction = "approve" | "retry" | "task_done" | "task_skip";
export const taskAction = (action: RunAction, run: string, task: string) => daemon<Run>(`run.${action}`, { id: run, task }).catch((e) => store.notify(String(e)));
export const cancelRun = (id: string) => daemon("run.cancel", { id }).catch((e) => store.notify(String(e)));
export const deleteRun = (id: string) => daemon("run.delete", { id }).catch((e) => store.notify(String(e)));

/** Activity entries and notifications for what changed in a run. */
function announce(prev: Run | undefined, next: Run) {
  const project = next.project_id ?? undefined;
  for (const task of next.tasks) {
    const before = prev?.tasks.find((x) => x.id === task.id)?.status;
    if (before === task.status) continue;
    const title = t("Run · {name} · {task}", { name: next.name, task: task.title });
    if (task.status === "waiting_approval") {
      // From API 12 the daemon says it (`notify.event`), also for the phone channels.
      if (!store.legacyNotify()) continue;
      const body = t("Waiting for your approval to start");
      store.addInbox({ kind: "waiting", title, body, projectId: project });
      void store.desktopNotify(title, body);
    } else if (task.status === "failed") {
      store.addInbox({ kind: "error", title, body: task.error ?? t("Failed"), sessionId: task.session_id ?? undefined, projectId: project });
    }
  }
  if (prev?.status === "running" && next.status !== "running") {
    const body = next.status === "done" ? t("All tasks done") : next.status === "failed" ? t("Finished with failed tasks") : t("Cancelled");
    const title = t("Run · {name}", { name: next.name });
    store.addInbox({ kind: next.status === "done" ? "finished" : "error", title, body, projectId: project });
    void store.desktopNotify(title, body);
  }
}

/** Worktrees removed in this session (a discarded candidate, a finished task): a run's later updates do not bring them back. */
const removedWorktrees = new Set<string>();
export const markWorktreeRemoved = (path: string) => void removedWorktrees.add(normPath(path));

/** A task running in its own worktree appears as that worktree's project, with its session. */
async function adoptWorktrees(run: Run) {
  const parent = store.state.projects.find((p) => p.id === run.project_id);
  if (!parent) return;
  const repo = parent.parentId ?? parent.id;
  for (const t of run.tasks) {
    if (!t.worktree || !t.cwd || !t.session_id || !t.branch) continue;
    // A worktree that was removed (a discarded candidate, a finished one) does not come back.
    if (removedWorktrees.has(normPath(t.cwd))) continue;
    if (!store.state.projects.some((p) => normPath(p.path) === normPath(t.cwd!)) && !(await isDir(t.cwd).catch(() => false))) continue;
    const s = store.sessions[t.session_id];
    const pid = store.addProject(t.cwd, { parentId: repo, branch: t.branch, baseBranch: t.base, open: false });
    if (s && s.project_id !== pid) void daemon("session.update", { handle: s.handle, project_id: pid }).catch(() => {});
  }
}

let started = false;
/** Keeps the run list in sync with the daemon (call once). */
export function initRuns() {
  if (started) return;
  started = true;
  void onEvent<{ event: string; params: any }>("daemon-event", (msg) => {
    if (msg.event === "run.updated") {
      const r = msg.params as Run;
      announce(runs[r.id], r);
      setRuns(r.id, reconcile(r));
      void adoptWorktrees(r);
    } else if (msg.event === "run.deleted") {
      setRuns(produce((m) => delete m[msg.params.id]));
    }
  });
  void onEvent<{ connected: boolean }>("daemon-status", (s) => s.connected && void loadRuns());
  void loadRuns();
}

/** Asks an agent (one-shot) to split an objective into tasks; the user reviews them. */
export async function planTasks(objective: string, cwd: string, agent: SessionKind, defaultKind: SessionKind): Promise<{ name: string; tasks: TaskDraft[] }> {
  const prompt = `You are planning work for a small team of coding agents that work in parallel on the repository in the current folder.
Objective: ${objective}

Split the objective into 2 to 6 tasks. Each task is done by one agent with no other context than its prompt and the summaries of the tasks it depends on, so write each prompt as complete instructions. Tasks that do not depend on each other run at the same time; tasks that edit code at the same time should each use their own git worktree ("worktree": true). A final task can integrate or review the others.

Reply with ONLY a JSON object, no prose, no code fences:
{"name": "<short run name>", "tasks": [{"id": "<short-id>", "title": "<title>", "prompt": "<instructions>", "deps": ["<ids of earlier tasks it needs>"], "worktree": true}]}`;
  const text = await agentRun(agent, prompt, cwd, store.profileLaunch(agent).env, 300);
  const json = text.slice(text.indexOf("{"), text.lastIndexOf("}") + 1);
  let plan: any;
  try {
    plan = JSON.parse(json);
  } catch {
    throw new Error(t("the agent did not answer with a task list; try again or write the tasks yourself"));
  }
  const tasks: TaskDraft[] = (Array.isArray(plan.tasks) ? plan.tasks : []).map((tk: any, i: number) => ({
    id: String(tk.id ?? `t${i + 1}`).replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 40) || `t${i + 1}`,
    title: String(tk.title ?? t("Task {n}", { n: i + 1 })),
    prompt: String(tk.prompt ?? ""),
    kind: defaultKind,
    deps: Array.isArray(tk.deps) ? tk.deps.map(String) : [],
    approve: false,
    worktree: !!tk.worktree,
  }));
  if (!tasks.length) throw new Error(t("the agent returned no tasks"));
  const ids = new Set(tasks.map((t) => t.id));
  for (const t of tasks) t.deps = t.deps.filter((d) => ids.has(d) && d !== t.id);
  return { name: String(plan.name ?? "").slice(0, 80), tasks };
}
