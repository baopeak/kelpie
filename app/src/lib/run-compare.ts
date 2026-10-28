// Comparing the candidates of a run: the same work done by several agents, each in a worktree of
// its own. The model is pure (a run plus what was read about each worktree); the actions at the
// bottom pick one for Finish and remove the others.
import { store } from "./store";
import * as ipc from "./ipc";
import { baseOf, changesOf, openFinish, refreshChanges } from "./review-queue";
import { loadChecksPlan } from "./finish";
import { checksChip, countTurns, sessionsOf, worktreeCost, type ChecksChip } from "./review-facts";
import { removeWorktree, stopSessions } from "./worktrees";
import { kindInfo } from "./agents";
import { finishedTask, taskAction, type Run, type RunTask, type TaskStatus } from "./runs";
import { confirmDialog } from "./confirm-dialog";
import { formatCost } from "./usage-format";
import { normPath } from "./platform";
import { t } from "./i18n";
import type { Project } from "./types";

/** What was read about one candidate's worktree. */
export interface Facts {
  projectId: string | null;
  changes: ipc.ReviewChanges | null;
  cost: number | null;
  turns: number | null;
  checks: ChecksChip | null;
}

export interface Candidate {
  /** The task's id. */
  id: string;
  title: string;
  kind: string;
  status: TaskStatus;
  branch: string | null;
  cwd: string | null;
  projectId: string | null;
  sessionId: string | null;
  summary: string | null;
  /** Milliseconds from start to end (so far, while it runs); null before it started. */
  durationMs: number | null;
  running: boolean;
  /** Files changed against the base; null until read. */
  files: string[] | null;
  added: number | null;
  deleted: number | null;
  uncommitted: number;
  cost: number | null;
  turns: number | null;
  checks: ChecksChip | null;
  /** The worktree is in the sidebar and its changes were read: it can go to Finish. */
  pickable: boolean;
}

/** The tasks that are candidates: they work in a worktree of their own. */
export const candidateTasks = (run: Run): RunTask[] => run.tasks.filter((x) => x.worktree && x.cwd);

export function durationOf(task: RunTask, now: number): number | null {
  if (!task.started_at) return null;
  const end = task.finished_at > 0 ? task.finished_at : finishedTask(task.status) ? task.started_at : now;
  return Math.max(0, end - task.started_at);
}

export function buildCandidates(run: Run, facts: Record<string, Facts | undefined>, now: number): Candidate[] {
  return candidateTasks(run).map((task) => {
    const f = facts[task.id];
    const c = f?.changes?.ok ? f.changes : null;
    return {
      id: task.id,
      title: task.title,
      kind: task.kind,
      status: task.status,
      branch: task.branch,
      cwd: task.cwd,
      projectId: f?.projectId ?? null,
      sessionId: task.session_id,
      summary: task.summary ?? task.error,
      durationMs: durationOf(task, now),
      running: !finishedTask(task.status),
      files: c ? c.files : null,
      added: c ? c.added : null,
      deleted: c ? c.deleted : null,
      uncommitted: c?.uncommitted ?? 0,
      cost: f?.cost ?? null,
      turns: f?.turns ?? null,
      checks: f?.checks ?? null,
      pickable: !!f?.projectId && !!c,
    };
  });
}

export interface Best {
  fastest?: string;
  cheapest?: string;
  smallest?: string;
}

/** The task ids that win a column among the candidates that finished (a tie names nobody). */
export function bestOf(rows: Candidate[]): Best {
  const done = rows.filter((r) => r.status === "done");
  const pick = (value: (r: Candidate) => number | null): string | undefined => {
    const have = done.flatMap((r) => (value(r) == null ? [] : [{ id: r.id, v: value(r)! }]));
    if (have.length < 2) return undefined;
    const min = Math.min(...have.map((h) => h.v));
    const winners = have.filter((h) => h.v === min);
    return winners.length === 1 ? winners[0].id : undefined;
  };
  return {
    fastest: pick((r) => r.durationMs),
    cheapest: pick((r) => (r.cost && r.cost > 0 ? r.cost : null)),
    smallest: pick((r) => (r.added == null || r.deleted == null ? null : r.added + r.deleted)),
  };
}

/** Every file any candidate changed, with the candidates that changed it (most shared first). */
export function changedFiles(rows: Candidate[]): { path: string; by: string[] }[] {
  const map = new Map<string, string[]>();
  for (const r of rows) for (const f of r.files ?? []) map.set(f, [...(map.get(f) ?? []), r.id]);
  return [...map].map(([path, by]) => ({ path, by })).sort((a, b) => b.by.length - a.by.length || a.path.localeCompare(b.path));
}

/** What picking `winner` removes: the other candidates that still have a worktree. */
export const discardsFor = (rows: Candidate[], winner: string) => rows.filter((r) => r.id !== winner && r.projectId);

export function formatDuration(ms: number | null): string {
  if (ms == null) return "–";
  const s = Math.round(ms / 1000);
  return s < 60 ? `${s}s` : s < 3600 ? `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s` : `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}

// ---------- reading ----------

const projectAt = (cwd: string | null): Project | undefined => (cwd ? store.state.projects.find((p) => normPath(p.path) === normPath(cwd)) : undefined);

/** Reads each candidate's changes, cost, turns and checks (one git read per worktree; nothing runs after). */
export async function readFacts(run: Run, force: boolean): Promise<Record<string, Facts>> {
  const tasks = candidateTasks(run);
  const projects = tasks.map((task) => projectAt(task.cwd));
  await refreshChanges(projects.filter((p): p is Project => !!p), force);
  const out: Record<string, Facts> = {};
  await Promise.all(
    tasks.map(async (task, i) => {
      const p = projects[i];
      if (!p) return void (out[task.id] = { projectId: null, changes: null, cost: null, turns: null, checks: null });
      const changes = changesOf(p.id) ?? null;
      const repo = store.state.projects.find((x) => x.id === p.parentId);
      const base = baseOf(p);
      const [turns, plan] = await Promise.all([
        countTurns(sessionsOf(p.id).map((s) => s.id)).catch(() => null),
        repo && base && changes?.ok ? loadChecksPlan(repo, base, !!changes.config_changed).catch(() => undefined) : Promise.resolve(undefined),
      ]);
      const cost = worktreeCost(p.id);
      out[task.id] = { projectId: p.id, changes, cost: cost > 0 ? cost : null, turns, checks: checksChip(plan) };
    }),
  );
  return out;
}

// ---------- picking ----------

const describe = (c: Candidate) => {
  const size = c.files ? t("{n} files, +{added} −{deleted}", { n: c.files.length, added: c.added ?? 0, deleted: c.deleted ?? 0 }) : t("changes not read");
  return `• ${c.title} (${c.branch ?? ""}): ${size}${c.uncommitted ? ` · ${t("{n} uncommitted", { n: c.uncommitted })}` : ""}${c.cost ? ` · ${formatCost(c.cost, false)}` : ""}`;
};

/** What a candidate's worktree holds: its branch and commit, and the files not committed (with their sizes). */
export interface Snapshot {
  head: string;
  /** `status path size`, sorted. */
  files: string[];
  /** The paths, for the commit that saves them. */
  paths: string[];
  branch: string | null;
}

export const sameSnapshot = (a: Snapshot, b: Snapshot) => a.head === b.head && a.branch === b.branch && a.files.length === b.files.length && a.files.every((f, i) => f === b.files[i]);

export async function snapshotOf(wt: Project, repo: Project, base: string): Promise<Snapshot> {
  const [head, inspect] = await Promise.all([ipc.gitHead(wt.path), ipc.finishInspect(repo.path, wt.path, base)]);
  return { head, branch: inspect.branch, files: inspect.uncommitted.map((f) => `${f.status} ${f.path} ${f.size}`).sort(), paths: inspect.uncommitted.map((f) => f.path) };
}

const projectsOf = (c: Candidate) => {
  const wt = store.state.projects.find((p) => p.id === c.projectId);
  const repo = wt && store.state.projects.find((p) => p.id === wt.parentId);
  return wt && repo ? { wt, repo, base: baseOf(wt) } : null;
};

/**
 * Stops a candidate (its task and every session in its worktree, waiting until they have exited) and
 * records what it holds. Done when the user confirms, so nothing changes after the snapshot unless
 * someone makes it change; null when it could not be stopped or read.
 */
export async function stopAndSnapshot(run: Run, c: Candidate): Promise<Snapshot | null> {
  const task = run.tasks.find((x) => x.id === c.id);
  if (task && !finishedTask(task.status)) await taskAction("task_skip", run.id, task.id);
  const p = projectsOf(c);
  if (!p?.base) return null;
  if (!(await stopSessions(p.wt))) return null;
  return snapshotOf(p.wt, p.repo, p.base).catch(() => null);
}

/** What became of a discarded candidate. */
export interface Fate {
  title: string;
  /** Why its worktree was left alone (null: removed). */
  kept: string | null;
  branch: string | null;
  /** The branch went with it (it was merged); otherwise it stays with the work the candidate did. */
  branchDeleted: boolean;
  /** Uncommitted files saved in a commit on its branch first. */
  saved: number;
}

/**
 * Removes a stopped candidate the safe way, only if it still holds exactly what `before` recorded
 * (the same commit, the same uncommitted files): what is not committed is saved in a commit on its
 * branch, the worktree goes through the ordinary non-forced removal, and the branch is deleted only when
 * git agrees it is merged (`branch -d`). Anything that does not fit leaves the candidate as it is.
 */
export async function discardSafely(c: Candidate, before: Snapshot | null): Promise<Fate> {
  const fate: Fate = { title: c.title, kept: null, branch: c.branch, branchDeleted: false, saved: 0 };
  const keep = (why: string): Fate => ({ ...fate, kept: why });
  const p = projectsOf(c);
  if (!p?.base) return keep(t("its worktree is not available"));
  if (!before) return keep(t("its agent could not be stopped"));
  try {
    const now = await snapshotOf(p.wt, p.repo, p.base);
    if (!sameSnapshot(before, now)) return keep(t("it changed after you confirmed"));
    if (!now.branch) return keep(t("it is on a detached HEAD"));
    if (now.paths.length) {
      const wip = await ipc.finishWipCommit(p.wt.path, now.branch, `WIP from ${kindInfo(c.kind).label}`, now.paths);
      if (wip.extra.length) return keep(t("more files appeared while saving its work"));
      fate.saved = now.paths.length;
    }
    await ipc.daemon("checkpoint.drop", { worktree: p.wt.path, root: p.repo.path }).catch(() => {});
    await removeWorktree(p.wt, { force: false });
  } catch (e) {
    return keep(String(e).replace(/^Error: /, ""));
  }
  if (c.branch) fate.branchDeleted = await ipc.gitBranchDelete(p.repo.path, c.branch, false).then(() => true, () => false);
  return fate;
}

export function fateText(f: Fate): string {
  if (f.kept) return t("{name}: kept, {why}", { name: f.title, why: f.kept });
  const saved = f.saved ? t("; its {n} uncommitted files were saved in a commit", { n: f.saved }) : "";
  return f.branchDeleted ? t("{name}: removed with its branch{saved}", { name: f.title, saved }) : t("{name}: worktree removed, branch {branch} kept with its work{saved}", { name: f.title, branch: f.branch ?? "", saved });
}

/** Tells the user what happened to the discarded candidates: a short line, the details in Activity. */
function report(winner: string | null, fates: Fate[]) {
  const kept = fates.filter((f) => f.kept).length;
  const removed = fates.length - kept;
  store.addInbox({ kind: "worktree", title: winner ? t("Compared candidates: kept “{name}”", { name: winner }) : t("Candidate discarded"), body: fates.map(fateText).join("\n") });
  store.notify(
    winner
      ? t("Kept “{name}”; {removed} other candidates removed, {kept} left as they are (details in Activity).", { name: winner, removed, kept })
      : fateText(fates[0]),
    undefined,
    9000,
  );
}

/** "Discard…" on one candidate (after the user confirmed it). */
export async function discardCandidate(run: Run, c: Candidate): Promise<void> {
  report(null, [await discardSafely(c, await stopAndSnapshot(run, c))]);
}

/**
 * "Use this one": after a confirmation that lists what goes, the other candidates are stopped and
 * recorded and the winner's Finish opens; once it has merged the work, each other candidate that still
 * holds exactly what was recorded is removed (worktree safely, branch only if merged) and the winner's
 * task is marked done. Nothing is removed before Finish succeeded.
 */
export async function useCandidate(run: Run, rows: Candidate[], winner: Candidate): Promise<void> {
  if (!winner.projectId) return;
  const others = discardsFor(rows, winner.id);
  const message = others.length
    ? t("Finish “{name}” and remove the other {n} candidates?\n\n{list}\n\nTheir agents are stopped now. After Finish has merged “{name}”, each worktree that is still as it is now goes: uncommitted work is saved in a commit on its branch, and the branch stays unless it is merged. A candidate that changed meanwhile is left alone.", { name: winner.title, n: others.length, list: others.map(describe).join("\n") })
    : t("Finish “{name}”?", { name: winner.title });
  if (!(await confirmDialog(message))) return;
  // Stopped together, with the count while it lasts (each may take a few seconds to exit).
  const before = new Map<string, Snapshot | null>();
  let stopped = 0;
  const progress = () => others.length && store.notify(t("Stopping the other candidates… {done}/{n}", { done: stopped, n: others.length }), undefined, 12000);
  progress();
  await Promise.all(others.map(async (c) => {
    before.set(c.id, await stopAndSnapshot(run, c).catch(() => null));
    stopped++;
    progress();
  }));
  openFinish(winner.projectId, () => {
    void (async () => {
      const fates: Fate[] = [];
      for (const c of others) fates.push(await discardSafely(c, before.get(c.id) ?? null));
      const task = run.tasks.find((x) => x.id === winner.id);
      if (task && !finishedTask(task.status)) await taskAction("task_done", run.id, task.id);
      if (fates.length) report(winner.title, fates);
    })();
  });
}
