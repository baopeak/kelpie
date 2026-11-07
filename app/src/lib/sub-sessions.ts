// Sub-sessions in the window: a session an agent opened with Kelpie's tools has a `parent` (daemon API 30+). This is the pure part,
// which the sidebar tree, the children strip and the approval cards are built on: the tree (defended against cycles and parents
// that are gone), the summary a parent's row shows, the sub-sessions *Close finished sub-sessions* offers, and the queue of
// requests waiting for the user. The only state here is what the window itself keeps (which parents are folded, the policy as the
// daemon last said it, the open requests); nothing is persisted.
import { createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import type { SessionInfo } from "./types";

/** Daemon API level that has sub-sessions (`parent` on a session, `sub.*`, the policy keys `sub_sessions`, `sub_max`). */
export const SUB_SESSIONS_API = 30;

/** The most sub-sessions the Settings number accepts (the daemon takes up to 64; 16 is already far more than a computer runs). */
export const SUB_MAX_LIMIT = 16;

// ---------- the tree ----------

export interface SessionTree {
  /** child id -> parent id, for a session that renders under its parent: the parent is a session this window knows, and the
   *  chain above it ends at a session with no (known) parent. */
  nested: Map<string, string>;
  /** parent id -> its nested children, oldest first. */
  children: Map<string, SessionInfo[]>;
}

/** Which sessions nest under which. A session whose parent is gone (closed, not listed) stays where it was; so does one whose
 *  parent chain loops back on itself, or points at itself: the daemon never makes one, but a damaged file or a future version
 *  must not make a session disappear from the sidebar. */
export function buildSessionTree(sessions: readonly SessionInfo[]): SessionTree {
  const byId = new Map<string, SessionInfo>();
  for (const s of sessions) byId.set(s.id, s);
  const nested = new Map<string, string>();
  for (const s of sessions) {
    const parent = s.parent;
    if (!parent || parent === s.id || !byId.has(parent)) continue;
    // Walk up: a chain that reaches a root is fine, one that meets a session twice is a loop.
    const seen = new Set<string>([s.id]);
    let at: string | undefined = parent;
    let ok = true;
    while (at) {
      if (seen.has(at)) {
        ok = false;
        break;
      }
      seen.add(at);
      const up: string | undefined = byId.get(at)?.parent || undefined;
      at = up && byId.has(up) ? up : undefined;
    }
    if (ok) nested.set(s.id, parent);
  }
  const children = new Map<string, SessionInfo[]>();
  for (const s of sessions) {
    const parent = nested.get(s.id);
    if (!parent) continue;
    const list = children.get(parent);
    if (list) list.push(s);
    else children.set(parent, [s]);
  }
  for (const list of children.values()) list.sort((a, b) => a.created_at - b.created_at || a.handle - b.handle);
  return { nested, children };
}

const NONE: SessionInfo[] = [];

/** The sessions nested directly under `id`. */
export const childrenOfSession = (tree: SessionTree, id: string): SessionInfo[] => tree.children.get(id) ?? NONE;

/** How deep `id` is nested (0: not under anything). */
export function sessionDepthTree(tree: SessionTree, id: string): number {
  let depth = 0;
  for (let at = tree.nested.get(id); at; at = tree.nested.get(at)) depth++;
  return depth;
}

/** What a parent's row shows about everything below it. */
export type ChildState = "waiting" | "error" | "working" | "done" | "idle" | "exited";

/** Worst first: a child that needs the user beats one that failed, one that works, one finished and unread, one that idles. */
const RANK: Record<ChildState, number> = { waiting: 5, error: 4, working: 3, done: 2, idle: 1, exited: 0 };

export interface SubtreeSummary {
  /** Everything below the session (children, their children…). */
  count: number;
  worst: ChildState;
}

/** The number of sessions below `id` and the worst state among them. `stateOf` is the sidebar's own `sessionState`. */
export function subtreeSummary(tree: SessionTree, id: string, stateOf: (s: SessionInfo) => ChildState): SubtreeSummary {
  let count = 0;
  let worst: ChildState = "exited";
  const stack = [...childrenOfSession(tree, id)];
  // The tree has no loops, but a bound costs nothing.
  for (let guard = 0; stack.length && guard < 10_000; guard++) {
    const s = stack.pop()!;
    count++;
    const state = stateOf(s);
    if (RANK[state] > RANK[worst]) worst = state;
    stack.push(...childrenOfSession(tree, s.id));
  }
  return { count, worst };
}

/** The sub-sessions *Close finished sub-sessions* closes: stopped or idle, and having said they are done or have exited. A session
 *  that works, waits for the user, or only went quiet without saying so stays. */
export function finishedChildren(children: readonly SessionInfo[], tree?: SessionTree): SessionInfo[] {
  return children.filter((c) => (!c.alive || c.status === "idle") && (c.report?.status === "done" || c.report?.status === "exited") && !(tree && hasLiveDescendant(tree, c.id)));
}

/** Something below `id` is still running: closing `id` would leave it without its parent. */
export function hasLiveDescendant(tree: SessionTree, id: string): boolean {
  const stack = [...childrenOfSession(tree, id)];
  for (let guard = 0; stack.length && guard < 10_000; guard++) {
    const s = stack.pop()!;
    if (s.alive) return true;
    stack.push(...childrenOfSession(tree, s.id));
  }
  return false;
}

/** The last path part of a folder, for a short label (the whole path stays in the tooltip). */
export function folderName(path: string): string {
  const parts = path.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

// ---------- requests waiting for the user ----------

export type SubDecision = "once" | "session" | "always" | "deny";

/** A spawn the user is asked to allow (`sub.request`, `sub.pending`). */
export interface SubRequest {
  id: string;
  /** The session that asks. */
  session: string;
  from_label: string;
  agent: string;
  cwd: string;
  worktree: boolean;
  name: string | null;
  task: string;
  model: string | null;
  /** The child will run without asking before it acts, because the session that opens it does. */
  skips_prompts: boolean;
  at: number;
}

const text = (v: unknown, max: number) => (typeof v === "string" ? v.slice(0, max) : "");

/** A request from the daemon, or null when it has no id (anything else missing is filled in). */
export function toSubRequest(raw: unknown): SubRequest | null {
  const r = raw as Record<string, unknown> | null;
  if (!r || typeof r !== "object" || typeof r.id !== "string" || !r.id) return null;
  return {
    id: r.id,
    session: text(r.session ?? r.from, 200),
    from_label: text(r.from_label, 200),
    agent: text(r.agent, 100),
    cwd: text(r.cwd, 1000),
    worktree: r.worktree === true,
    name: typeof r.name === "string" && r.name ? r.name.slice(0, 200) : null,
    task: text(r.task, 4000),
    model: typeof r.model === "string" && r.model ? r.model.slice(0, 100) : null,
    skips_prompts: r.skips_prompts === true,
    at: typeof r.at === "number" ? r.at : 0,
  };
}

export interface RequestsState {
  list: SubRequest[];
  /** Ids answered or expired lately: a list read from the daemon a moment before must not bring one back. */
  closed: string[];
}

export const emptyRequests: RequestsState = { list: [], closed: [] };

export type RequestEvent =
  | { type: "request"; request: SubRequest }
  | { type: "done"; id: string }
  /** The daemon's list of open requests (`sub.pending`), read when the window connects. A request this window learned of at or after
   *  `keepFrom` (wall-clock ms, when the question was sent) may be newer than the list and is kept. */
  | { type: "replace"; list: SubRequest[]; keepFrom?: number };

const MAX_CLOSED = 100;

/** The queue of open requests after `event`. A request already listed, or already closed, changes nothing. */
export function reduceRequests(state: RequestsState, event: RequestEvent): RequestsState {
  switch (event.type) {
    case "request": {
      if (state.list.some((r) => r.id === event.request.id) || state.closed.includes(event.request.id)) return state;
      return { ...state, list: [...state.list, event.request].sort((a, b) => a.at - b.at) };
    }
    case "done": {
      const closed = state.closed.includes(event.id) ? state.closed : [...state.closed, event.id].slice(-MAX_CLOSED);
      const list = state.list.filter((r) => r.id !== event.id);
      return list.length === state.list.length && closed === state.closed ? state : { list, closed };
    }
    case "replace": {
      const fresh = event.list.filter((r) => !state.closed.includes(r.id));
      // What the daemon no longer lists is closed (answered elsewhere, or timed out while the window was away), unless it was
      // asked after the list was made.
      const newer = state.list.filter((r) => event.keepFrom !== undefined && r.at >= event.keepFrom && !fresh.some((f) => f.id === r.id));
      const next = [...fresh, ...newer].sort((a, b) => a.at - b.at);
      const same = next.length === state.list.length && next.every((r, i) => r.id === state.list[i].id);
      return same ? state : { ...state, list: next };
    }
  }
}

const [requests, setRequests] = createSignal<RequestsState>(emptyRequests);

/** Requests waiting for the user, oldest first. */
export const subRequests = () => requests().list;

/** Requests the user chose to answer later: their cards are hidden; they stay in the Activity panel. */
const [later, setLater] = createSignal<string[]>([]);

/** The requests whose cards are shown. */
export const shownRequests = () => requests().list.filter((r) => !later().includes(r.id));
export const postponeRequest = (id: string) => setLater((l) => (l.includes(id) ? l : [...l, id]));
export const showPostponed = () => setLater([]);

/** Applies `event` to the window's queue; true when it changed. */
export function dispatchRequest(event: RequestEvent): boolean {
  const next = reduceRequests(requests(), event);
  if (next === requests()) return false;
  setRequests(next);
  // A request that is gone needs no "later" any more.
  setLater((l) => (l.length ? l.filter((id) => next.list.some((r) => r.id === id)) : l));
  return true;
}

// ---------- the daemon's policy ----------

export interface SubPolicy {
  sub_sessions: "off" | "ask" | "allow";
  /** 0: automatic. */
  sub_max: number;
  /** What automatic means on this computer. */
  sub_auto_max: number;
}

/** The policy as the daemon last said it (null: not read yet, or a daemon without sub-sessions). */
export const [subPolicy, setSubPolicy] = createSignal<SubPolicy | null>(null);

/** The policy from a `daemon.policy` / `daemon.set_policy` reply, or null when the reply has no sub-session keys. */
export function toSubPolicy(raw: unknown): SubPolicy | null {
  const r = raw as Record<string, unknown> | null;
  if (!r || typeof r !== "object") return null;
  const mode = r.sub_sessions;
  if (mode !== "off" && mode !== "ask" && mode !== "allow") return null;
  const num = (v: unknown, fallback: number) => (typeof v === "number" && Number.isFinite(v) ? Math.max(0, Math.round(v)) : fallback);
  return { sub_sessions: mode, sub_max: num(r.sub_max, 0), sub_auto_max: num(r.sub_auto_max, 2) };
}

/** The number typed for "Max running": 1 to 16, else null (not a number we send). */
export function parseSubMax(value: string): number | null {
  const digits = value.trim();
  const n = /^\d{1,3}$/.test(digits) ? Number(digits) : 0;
  return n >= 1 && n <= SUB_MAX_LIMIT ? n : null;
}

// ---------- which parents are folded in the sidebar ----------

/** Parents whose children are hidden (this window only; everything starts open). */
export const [foldedParents, setFoldedParents] = createStore<Record<string, true | undefined>>({});

export const toggleParentFold = (id: string) => setFoldedParents(id, foldedParents[id] ? undefined : true);
