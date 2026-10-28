// Review queue and overlap watch. The queue is the Board's status (todo / progress / review /
// done, shared with agents through `kelpied worktree set`); this adds what each worktree changed
// against the branch it will be merged into, and the "Also changed in …" chips for worktrees that
// edit the same files. The git work is in app/src-tauri/src/git_finish.rs.
import { createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { store } from "./store";
import * as ipc from "./ipc";
import { isAgentKind } from "./agents";
import { confirmDialog } from "./confirm-dialog";
import { t } from "./i18n";
import type { Project, WorkStatus } from "./types";

/** First daemon API level where an automation can deliver to review. */
export const REVIEW_API = 11;
const OVERLAP_EVERY_MS = 30_000;
const TURN_DEBOUNCE_MS = 1500;

const [changes, setChanges] = createStore<Record<string, ipc.ReviewChanges>>({});
const [overlaps, setOverlaps] = createStore<Record<string, { id: string; name: string; files: string[] }[]>>({});
const [finishTarget, setFinishTarget] = createSignal<string | null>(null);
const [reviewTarget, setReviewTarget] = createSignal<string | null>(null);
const [tryTarget, setTryTarget] = createSignal<{ projectId: string; prompt: string } | null>(null);
/** What runs when the Finish of a worktree ends with the work merged and the worktree gone. */
const afterFinish = new Map<string, () => void>();

/** Overlap scans run so far (nothing is scanned while fewer than two worktrees of a repository are active). */
export let overlapScans = 0;

export const changesOf = (projectId: string): ipc.ReviewChanges | undefined => changes[projectId];
export const overlapsOf = (projectId: string) => overlaps[projectId] ?? [];
export const finishing = finishTarget;
/** The project whose "Review with…" dialog is open. */
export const reviewing = reviewTarget;
/** The project and prompt of the open "Try with several agents" dialog. */
export const trying = tryTarget;

/** `after` runs once if this Finish ends with the work merged and the worktree gone (the other candidates of a comparison go then). */
export function openFinish(projectId: string, after?: () => void) {
  if (after) afterFinish.set(projectId, after);
  else afterFinish.delete(projectId);
  setFinishTarget(projectId);
  store.setOverlay("finish");
}

/** The Finish of `projectId` ended with the work merged and the worktree gone. */
export function finishSucceeded(projectId: string) {
  const after = afterFinish.get(projectId);
  afterFinish.delete(projectId);
  after?.();
}

export function openReviewWith(projectId: string) {
  setReviewTarget(projectId);
  store.setOverlay("review");
}

export function openTryAgents(projectId: string, prompt = "") {
  setTryTarget({ projectId, prompt });
  store.setOverlay("try");
}

const repoOf = (p: Project) => (p.parentId ? store.state.projects.find((x) => x.id === p.parentId) : undefined);

/** The repository's current branch (null off a repository or on a detached HEAD). */
export function currentBranchOf(p: Project): string | null {
  if (p.remote) return null;
  const info = store.projectInfo[(repoOf(p) ?? p).id];
  return info?.is_repo ? info.branch : null;
}

/** The branch a project's work is compared with and merged into: the one the worktree was created from,
 *  else (older worktrees) the repository's current branch. A repository's own folder is compared with
 *  itself, which leaves just its uncommitted files. */
export function baseOf(p: Project): string | null {
  if (p.remote) return null;
  return p.parentId && p.baseBranch ? p.baseBranch : currentBranchOf(p);
}

/** Projects that are worktrees whose repository is in the sidebar. */
export const worktrees = () => store.state.projects.filter((p) => p.parentId && repoOf(p) && !p.remote);

export const inReview = (p: Project) => (p.status ?? "todo") === "review";

/** Reads the changes of `projects` (answered from git_finish.rs's cache unless `force`). */
export async function refreshChanges(projects: Project[], force = false) {
  const jobs = projects.flatMap((p) => {
    const base = baseOf(p);
    return base ? [{ p, base }] : [];
  });
  if (!jobs.length) return;
  try {
    const out = await ipc.reviewChanges(jobs.map((j) => ({ path: j.p.path, base: j.base })), force);
    out.forEach((c, i) => setChanges(jobs[i].p.id, reconcile(c)));
  } catch {
    /* git unavailable: the cards keep what they showed */
  }
}

const hasLiveAgent = (projectId: string) => Object.values(store.sessions).some((s) => s.project_id === projectId && s.alive && isAgentKind(s.kind));

/** The repository and its worktrees that have an agent running; fewer than two means nothing to compare. */
export function activeMembers(repo: Project): Project[] {
  const members = [repo, ...store.childrenOf(repo.id)].filter((p) => !p.remote);
  const live = members.filter((p) => hasLiveAgent(p.id));
  return live.length >= 2 ? live : [];
}

/** Pure: which of `members` also changed the files `files[i]` lists, per member. */
export function intersect(members: { id: string; name: string; files: string[] }[]): Record<string, { id: string; name: string; files: string[] }[]> {
  const out: Record<string, { id: string; name: string; files: string[] }[]> = {};
  for (const a of members) {
    const mine = new Set(a.files);
    const list = members
      .filter((b) => b.id !== a.id)
      .map((b) => ({ id: b.id, name: b.name, files: b.files.filter((f) => mine.has(f)) }))
      .filter((o) => o.files.length);
    out[a.id] = list;
  }
  return out;
}

/** Compares the files the active worktrees of one repository changed (committed, staged, edited, untracked). */
export async function scanOverlap(repo: Project) {
  const active = activeMembers(repo);
  const family = [repo, ...store.childrenOf(repo.id)];
  if (!active.length) {
    // Nothing to compare any more: stale chips go.
    for (const p of family) if (overlaps[p.id]?.length) setOverlaps(p.id, []);
    return;
  }
  overlapScans++;
  const jobs = active.flatMap((p) => {
    const base = baseOf(p);
    return base ? [{ p, base }] : [];
  });
  try {
    const out = await ipc.reviewChanges(jobs.map((j) => ({ path: j.p.path, base: j.base, names_only: true })), true);
    const members = jobs.flatMap((j, i) => (out[i]?.ok ? [{ id: j.p.id, name: j.p.name, files: out[i].files }] : []));
    const map = intersect(members);
    for (const p of family) setOverlaps(p.id, map[p.id] ?? []);
  } catch {
    /* git unavailable: keep the chips */
  }
}

let started = false;

/** Scans every 30 s while the window is visible, and a moment after an agent's turn ends. */
export function initReviewWatch() {
  if (started) return;
  started = true;
  const repos = () => store.state.projects.filter((p) => !p.parentId && !p.remote && store.childrenOf(p.id).length);
  const scanAll = () => {
    if (document.hidden) return;
    for (const r of repos()) void scanOverlap(r);
  };
  setInterval(scanAll, OVERLAP_EVERY_MS);
  document.addEventListener("visibilitychange", () => !document.hidden && scanAll());
  const pending = new Map<string, ReturnType<typeof setTimeout>>();
  window.addEventListener("kelpie:session-turn", (e) => {
    const s = store.sessions[(e as CustomEvent).detail?.session];
    const project = s?.project_id ? store.state.projects.find((p) => p.id === s.project_id) : undefined;
    const repo = project && (repoOf(project) ?? project);
    if (!repo || pending.has(repo.id)) return;
    pending.set(repo.id, setTimeout(() => {
      pending.delete(repo.id);
      void scanOverlap(repo);
      // A card in review shows the turn's edits at once.
      if (project && inReview(project)) void refreshChanges([project], true);
    }, TURN_DEBOUNCE_MS));
  });
}

/**
 * Moves a card to a column. Marking a worktree Done while it still has changes nobody merged
 * offers Review instead: Done would hide work that is only on its branch.
 */
export async function moveToStatus(p: Project, status: WorkStatus) {
  if (status === "done" && p.parentId && (p.status ?? "todo") !== "done" && baseOf(p)) {
    await refreshChanges([p], true);
    const c = changes[p.id];
    if (c?.ok && (c.commits > 0 || c.files.length > 0)) {
      const message = c.files.length === 1
        ? t("“{name}” has 1 changed file that is not merged. Mark it Done anyway? (Cancel moves it to Review so it can be finished.)", { name: p.name })
        : t("“{name}” has {n} changed files that are not merged. Mark it Done anyway? (Cancel moves it to Review so it can be finished.)", { name: p.name, n: c.files.length });
      // Esc and Cancel take the safe way out: the card goes to Review.
      if (!(await confirmDialog(message))) status = "review";
    }
  }
  store.setProjectStatus(p.id, status);
}
