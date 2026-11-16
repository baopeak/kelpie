// Live updates for remote projects, which have no file watcher (`fs_watch` skips `ssh://` roots). While the window is
// shown (not minimised: you may well watch an agent's edits beside another window), every few seconds the daemon stats the folders a remote project's tree shows and the files it has open,
// over its one SFTP session to the host (`sftp.stat_many`, daemon API 45: no new ssh per poll). Only what changed is
// read again: a folder through the tree, a file through file-refresh, and git status once. A project added with a port
// typed in its address is not covered (the daemon's sessions go by host name or alias): it refreshes on demand.
// Tested in remote-live.test.ts.
import { store } from "./store";
import * as ipc from "./ipc";
import { refreshFiles } from "./file-refresh";
import { gitState } from "./git-state";
import type { Project, RemoteMeta } from "./types";

export const POLL_MS = 4000;
/** After a failed poll a project waits 30 s, doubling up to 10 min; a refused login or host key waits 10 min at once
 *  (a host that bans repeated failed logins never sees one every few seconds). Refresh asks again at once. */
const BACKOFF_MS = 30_000;
const BACKOFF_MAX_MS = 10 * 60_000;
/** Git status of the active live project is read again every this many polls (an edit in place changes no folder). */
const GIT_EVERY = 8;
/** What one `sftp.stat_many` may ask about. */
const MAX_PATHS = 512;

/** The project gets live updates (else Refresh / Reload only). */
export const liveFor = (p: Project | null | undefined) => !!p?.remote && !p.remote.port && store.daemonApi() >= ipc.LIVE_API;

/** A project-relative path on the host, under the project's folder `base`. */
export const hostPath = (base: string, rel: string) => (rel ? `${base.replace(/\/+$/, "")}/${rel}` : base);

/** What a poll compares: kind, time and size (a folder's time changes when a name in it is added, removed or renamed). */
export const stamp = (m: RemoteMeta | null) => (m ? `${m.kind}:${m.mtime}:${m.size}` : "gone");

/** Keys whose stamp differs from the one seen before; a key seen for the first time is not a change. */
export function changed(prev: Map<string, string>, next: Map<string, string>): string[] {
  return [...next].filter(([k, v]) => prev.has(k) && prev.get(k) !== v).map(([k]) => k);
}

/**
 * What a poll reads again. Times are whole seconds and a folder's size may never change, so one stat cannot tell that
 * more changed later in the same second; and a view read its folder or file a moment before the first stat, so a
 * change in between would be taken as the starting point. Hence a path that changed, or was seen for the first time,
 * is read again once more on the next poll (`pending`).
 */
export function plan(prev: Map<string, string>, next: Map<string, string>, pending: Set<string>): { reload: string[]; pending: Set<string> } {
  const diff = changed(prev, next);
  const first = [...next.keys()].filter((k) => !prev.has(k));
  return { reload: [...new Set([...diff, ...[...pending].filter((k) => next.has(k))])], pending: new Set([...diff, ...first]) };
}

/** How long a project waits after its `failures`-th failed poll in a row; `login`: the host refused the login or key. */
export const backoff = (failures: number, login: boolean) => (login ? BACKOFF_MAX_MS : Math.min(BACKOFF_MS * 2 ** Math.max(0, failures - 1), BACKOFF_MAX_MS));
const loginRefused = (e: unknown) => /host key|permission denied|authentication|too many|password/i.test(String(e));

interface TreeWatch {
  project: Project;
  dirs: () => string[];
  onDirs: (dirs: string[]) => void;
}
const trees = new Map<string, TreeWatch>();
const files = new Map<string, { project: Project; paths: Map<string, number> }>();
const seen = new Map<string, Map<string, string>>();
const pendingOf = new Map<string, Set<string>>();
const busy = new Set<string>();
const waitUntil = new Map<string, number>();
const failuresOf = new Map<string, number>();
const pollsOf = new Map<string, number>();
let timer: ReturnType<typeof setInterval> | null = null;

function schedule() {
  const any = trees.size > 0 || files.size > 0;
  if (any && !timer) timer = setInterval(tick, POLL_MS);
  if (!any && timer) {
    clearInterval(timer);
    timer = null;
  }
}

function forget(root: string) {
  if (trees.has(root) || files.has(root)) return;
  for (const m of [seen, pendingOf, waitUntil, failuresOf, pollsOf]) m.delete(root);
}

/** Refresh: a project waiting after failed polls is asked again on the next one. */
export function resumeLive(root: string) {
  waitUntil.delete(root);
  failuresOf.delete(root);
}

/** Live updates cover this project and its last poll worked (else views re-check on their own). */
export const liveOk = (p: Project | null | undefined) => liveFor(p) && !!p && !waitUntil.has(p.path);

/** A remote project's tree: `dirs` are the folders it shows ("" is the top), `onDirs` reads changed ones again. */
export function watchTree(project: Project, dirs: () => string[], onDirs: (dirs: string[]) => void): () => void {
  if (!project.remote) return () => {};
  const root = project.path;
  const mine: TreeWatch = { project, dirs, onDirs };
  trees.set(root, mine);
  schedule();
  return () => {
    if (trees.get(root) === mine) trees.delete(root);
    forget(root);
    schedule();
  };
}

/** A file of a remote project open in a view: re-read (through file-refresh) when it changes on the host. */
export function watchFile(project: Project, path: string): () => void {
  if (!project.remote) return () => {};
  const root = project.path;
  const w = files.get(root) ?? { project, paths: new Map<string, number>() };
  files.set(root, w);
  w.paths.set(path, (w.paths.get(path) ?? 0) + 1);
  schedule();
  return () => {
    const n = (w.paths.get(path) ?? 1) - 1;
    if (n > 0) w.paths.set(path, n);
    else w.paths.delete(path);
    if (!w.paths.size && files.get(root) === w) files.delete(root);
    forget(root);
    schedule();
  };
}

/** One poll of one project; exported for tests. */
export async function pollProject(root: string, now = Date.now()) {
  const tree = trees.get(root);
  const open = files.get(root);
  const project = tree?.project ?? open?.project;
  if (!project?.remote || !liveFor(project) || busy.has(root) || (waitUntil.get(root) ?? 0) > now) return;
  waitUntil.delete(root);
  const keys = [...new Set([...(tree?.dirs() ?? []).map((d) => `d:${d}`), ...[...(open?.paths.keys() ?? [])].map((f) => `f:${f}`)])].slice(0, MAX_PATHS);
  if (!keys.length) return;
  busy.add(root);
  try {
    const stats = await ipc.sftpStatMany(project.remote.dest, keys.map((k) => hostPath(project.remote!.path, k.slice(2))));
    // Nothing of the project is shown any more: its state stays forgotten.
    if (!trees.has(root) && !files.has(root)) return;
    failuresOf.delete(root);
    const prev = seen.get(root) ?? new Map<string, string>();
    const next = new Map(keys.map((k, i) => [k, stamp(stats[i] ?? null)]));
    const { reload, pending } = plan(prev, next, pendingOf.get(root) ?? new Set());
    // Kept across polls: a folder collapsed and opened again is compared with what it was.
    seen.set(root, new Map([...prev, ...next]));
    pendingOf.set(root, pending);
    const dirs = reload.filter((k) => k.startsWith("d:")).map((k) => k.slice(2));
    if (dirs.length) trees.get(root)?.onDirs(dirs);
    for (const k of reload) if (k.startsWith("f:")) refreshFiles(root, k.slice(2));
    const polls = (pollsOf.get(root) ?? 0) + 1;
    pollsOf.set(root, polls);
    if (store.activeProject()?.path === root && (changed(prev, next).length || polls % GIT_EVERY === 0)) gitState.schedule();
  } catch (e) {
    // Unreachable or refused for now: quiet, asked again later (Refresh asks at once).
    const n = (failuresOf.get(root) ?? 0) + 1;
    failuresOf.set(root, n);
    waitUntil.set(root, Date.now() + backoff(n, loginRefused(e)));
  } finally {
    busy.delete(root);
  }
}

function tick() {
  if (document.hidden) return;
  for (const root of new Set([...trees.keys(), ...files.keys()])) void pollProject(root);
}

// Shown again after being minimised: a poll at once, not up to a period later.
if (typeof document !== "undefined") document.addEventListener("visibilitychange", () => timer && tick());
