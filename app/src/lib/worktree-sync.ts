// Keeps a repository's worktree rows in step with `git worktree list`. A worktree removed outside
// Kelpie (git CLI, an agent) leaves its row behind; this takes such rows away once it is certain the
// worktree is gone, and marks the doubtful ones instead. It never adds rows (new worktrees are added
// when the repository is added, or from "Add existing worktrees") and never touches files or branches.
import { createStore, produce } from "solid-js/store";
import { createEffect, createRoot } from "solid-js";
import { metaKey, store } from "./store";
import * as ipc from "./ipc";
import { t } from "./i18n";
import { normPath } from "./platform";
import { allPanes } from "./layout";
import type { Project, Worktree } from "./types";

/** Why a worktree row is shown differently:
 *  - `missing`: its folder is gone, but it keeps sessions (or git still records it), so it stays;
 *  - `unlisted`: its folder exists, but git no longer lists it as a worktree of the repository. */
export type WorktreeHealth = "missing" | "unlisted";

/** What the check reads from the computer; injected by tests. */
export interface SyncProbe {
  listWorktrees(repoPath: string): Promise<Worktree[]>;
  isDir(path: string): Promise<boolean>;
  canonical(path: string): Promise<string>;
}

export interface RepoCheck {
  /** Worktree rows that can go: folder gone, git no longer lists it, no sessions. */
  remove: Project[];
  /** The rows that stay but are shown as `missing` or `unlisted`. */
  marks: Record<string, WorktreeHealth>;
}

/** The folders above `path`, nearest first, down to a drive root (`D:\`) or a share (`\\server\share`), but not `/`. */
export function ancestors(path: string): string[] {
  const out: string[] = [];
  let p = path.replace(/[\\/]+$/, "");
  // A network path's first folder is its share: `\\server` and `\` above it are not folders of that share.
  const share = p.match(/^[\\/]{2}[^\\/]+[\\/][^\\/]+/)?.[0];
  for (let i = 0; i < 64; i++) {
    const m = p.match(/^(.*?)[\\/][^\\/]+$/);
    if (!m || !m[1] || (share && m[1].length < share.length)) break;
    let parent = m[1];
    if (/^[A-Za-z]:$/.test(parent)) parent += p.charAt(2);
    out.push(parent);
    if (/^[A-Za-z]:[\\/]$/.test(parent)) break;
    p = parent;
  }
  return out;
}

/**
 * Decides, for one repository, which of its worktree rows go and which are marked. Returns null when
 * nothing can be said safely: a remote repository, a repository folder that cannot be reached, or a git
 * error. A row is removed only on a clear result: its folder does not exist while the place it was in can
 * be reached (so not an unplugged drive), git does not list it, and it has no session.
 */
export async function checkRepo(repo: Project, worktrees: Project[], hasSessions: (wt: Project) => boolean, probe: SyncProbe): Promise<RepoCheck | null> {
  if (repo.remote) return null;
  if (!(await probe.isDir(repo.path).catch(() => false))) return null;
  let list: Worktree[];
  try {
    list = await probe.listWorktrees(repo.path);
  } catch {
    return null;
  }
  if (!Array.isArray(list) || !list.length) return null;
  const keys = async (p: string) => [normPath(p), normPath(await probe.canonical(p).catch(() => p))];
  const listed = new Set((await Promise.all(list.map((w) => keys(w.path)))).flat());
  const remove: Project[] = [];
  const marks: Record<string, WorktreeHealth> = {};
  for (const wt of worktrees) {
    if (wt.remote) continue;
    const own = await keys(wt.path);
    if (await probe.isDir(wt.path).catch(() => true)) {
      if (!own.some((k) => listed.has(k))) marks[wt.id] = "unlisted";
      continue;
    }
    // Gone, or only out of reach (an unplugged drive, a network share that is down): with nothing above
    // it reachable, nothing is said about it.
    const above = await nearestFolder(wt.path, probe);
    if (above === null) continue;
    // A missing folder has no canonical form of its own: spell it from the canonical form of the folder
    // above (long names, resolved links), as git may have recorded it.
    const canonAbove = await probe.canonical(above).catch(() => above);
    own.push(normPath(canonAbove.replace(/[\\/]+$/, "") + "/" + wt.path.slice(above.length).replace(/^[\\/]+/, "")));
    if (own.some((k) => listed.has(k)) || hasSessions(wt)) marks[wt.id] = "missing";
    else remove.push(wt);
  }
  return { remove, marks };
}

/** The nearest folder above `path` that exists, or null when none can be reached. */
async function nearestFolder(path: string, probe: SyncProbe): Promise<string | null> {
  for (const a of ancestors(path)) if (await probe.isDir(a).catch(() => false)) return a;
  return null;
}

// ---------- live state ----------

const [health, setHealth] = createStore<Record<string, WorktreeHealth>>({});
/** How a worktree row is shown: `missing`, `unlisted` or normally (undefined). */
export const worktreeHealth = (projectId: string): WorktreeHealth | undefined => health[projectId];

/** Worktrees Kelpie itself is removing right now: the check leaves them to that removal. */
const busy = new Set<string>();
export function markWorktreeBusy(id: string, on: boolean) {
  if (on) busy.add(id);
  else busy.delete(id);
}

/** Rows the user brought back with Undo: in this run of the app they are only marked, not removed again. */
const keptPaths = new Set<string>();

/** A worktree has a session when a session (running, waiting or stopped) belongs to it or runs in its
 *  folder, or one of its panes holds a session (a sleeping agent too). */
export function worktreeHasSessions(wt: Project): boolean {
  const root = normPath(wt.path);
  const inside = (cwd: string) => !!cwd && (normPath(cwd) === root || normPath(cwd).startsWith(root + "/"));
  if (Object.values(store.sessions).some((s) => s.project_id === wt.id || inside(s.cwd))) return true;
  return store.workspace(wt.id).tabs.some((tab) => allPanes(tab.root).some((p) => !!p.sessionId));
}

/** Sessions are known only once read from the connected daemon: before that a worktree with sessions looks empty. */
const ready = () => store.connected() && store.sessionsKnown();

const GIT_TIMEOUT_MS = 20_000;
/** A git call that never answers (a hung network drive) counts as an error, so later checks still run. */
function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timed out")), ms);
    p.then(
      (v) => { clearTimeout(timer); resolve(v); },
      (e) => { clearTimeout(timer); reject(e); },
    );
  });
}

const liveProbe: SyncProbe = {
  listWorktrees: (repoPath) => withTimeout(ipc.gitWorktrees(repoPath), GIT_TIMEOUT_MS),
  isDir: (path) => ipc.isDir(path),
  canonical: (path) => ipc.canonicalPath(path),
};

const UNDO_MS = 10_000;

/** Clears the review notes filed under the worktree's own folder. Never those of a folder above it: the
 *  daemon files notes of a folder without `.git` under the repository holding it. */
async function clearOwnNotes(wt: Project) {
  const key = await store.noteKeyFor(wt).catch(() => "");
  const own = [metaKey(wt.path), metaKey(await ipc.canonicalPath(wt.path).catch(() => wt.path))];
  if (key && own.includes(key)) await store.clearNotesAt(wt.id, key);
}

const clone = <T>(v: T): T => (v === undefined ? v : JSON.parse(JSON.stringify(v)));

/** What `removeProject` drops of a row, kept so Undo can put it back. */
function snapshot(wt: Project, index: number) {
  return {
    project: clone(wt),
    workspace: clone(store.workspace(wt.id)),
    index,
    collapsed: !!store.state.collapsed?.[wt.id],
    worktreesExpanded: !!store.state.worktreesExpanded?.[wt.id],
    promptHistory: clone(store.state.promptHistory?.[wt.id]),
  };
}

/** Takes the rows away as Remove from Kelpie does (layouts, fold state, history) at once, and their review
 *  notes once Undo can no longer bring them back. */
function forget(list: Project[]) {
  const order = store.state.projects.map((p) => p.id);
  const saved = list.map((wt) => snapshot(wt, order.indexOf(wt.id)));
  for (const wt of list) store.removeProject(wt.id);
  setHealth(produce((h) => list.forEach((wt) => delete h[wt.id])));
  let undone = false;
  const n = list.length;
  store.notify(
    n === 1 ? t("Removed {n} worktree that no longer exists", { n }) : t("Removed {n} worktrees that no longer exist", { n }),
    {
      label: t("Undo"),
      run: () => {
        if (undone) return;
        undone = true;
        for (const s of saved) keptPaths.add(normPath(s.project.path));
        store.restoreProjects(saved);
        void syncWorktrees();
      },
    },
    UNDO_MS,
  );
  setTimeout(() => {
    if (undone) return;
    for (const wt of list) if (!store.state.projects.some((p) => p.id === wt.id)) void clearOwnNotes(wt);
  }, UNDO_MS + 1000);
}

/** Removes a marked worktree's row on the user's request (its sessions keep running; nothing on disk changes).
 *  A folder that still exists keeps its notes: it may be added again. */
export async function removeFromSidebar(wt: Project) {
  const gone = !(await ipc.isDir(wt.path).catch(() => true));
  store.removeProject(wt.id);
  setHealth(produce((h) => void delete h[wt.id]));
  if (gone) await clearOwnNotes(wt);
}

const running = new Map<string, Promise<void>>();

async function syncRepo(repo: Project, probe: SyncProbe): Promise<Project[]> {
  const kids = store.childrenOf(repo.id).filter((wt) => !busy.has(wt.id));
  if (!kids.length) return [];
  const result = await checkRepo(repo, kids, worktreeHasSessions, probe);
  if (!result) return [];
  // What changed while git answered (a session opened, a removal started, the row went) wins.
  const remove: Project[] = [];
  for (const wt of result.remove) {
    if (!store.state.projects.some((p) => p.id === wt.id) || busy.has(wt.id)) continue;
    if (keptPaths.has(normPath(wt.path)) || worktreeHasSessions(wt)) result.marks[wt.id] = "missing";
    else remove.push(wt);
  }
  setHealth(produce((h) => {
    for (const wt of kids) {
      if (result.marks[wt.id]) h[wt.id] = result.marks[wt.id];
      else delete h[wt.id];
    }
  }));
  return remove;
}

/**
 * Checks every local repository with worktree rows (or only `repoId`'s). Runs only while the daemon is
 * connected: before that the sessions are not known, and a worktree with sessions must never go.
 */
export async function syncWorktrees(repoId?: string, probe: SyncProbe = liveProbe) {
  if (!ready()) return;
  const repos = store.state.projects.filter((p) => !p.parentId && !p.remote && (!repoId || p.id === repoId) && store.childrenOf(p.id).length);
  for (const repo of repos) {
    // One check per repository at a time; one asked for meanwhile runs after it (it may know more).
    const job: Promise<void> = (running.get(repo.id) ?? Promise.resolve())
      .then(() => (ready() ? syncRepo(repo, probe) : []))
      .then((r) => {
        // Removed at once, so a check that follows sees them gone.
        if (r.length) forget(r);
      }, () => {})
      .finally(() => {
        if (running.get(repo.id) === job) running.delete(repo.id);
      });
    running.set(repo.id, job);
    await job;
  }
}

/** Checks a repository again a moment after Kelpie created, added or removed one of its worktrees. */
export function syncRepoSoon(repoId: string | undefined) {
  if (repoId) setTimeout(() => void syncWorktrees(repoId), 500);
}

const FOCUS_EVERY_MS = 15_000;
let started = false;

/** Checks once the daemon is connected (sessions known), then when the window gains focus, at most every 15 s. */
export function initWorktreeSync() {
  if (started) return;
  started = true;
  let last = 0;
  const run = () => {
    if (!ready() || Date.now() - last < FOCUS_EVERY_MS) return;
    last = Date.now();
    void syncWorktrees();
  };
  createRoot(() => {
    let first = true;
    createEffect(() => {
      if (!ready() || !first) return;
      first = false;
      last = Date.now();
      void syncWorktrees();
    });
  });
  window.addEventListener("focus", run);
}
