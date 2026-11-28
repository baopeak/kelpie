// The person's ranking of work (daemon API 52, `policy.resources.*`): each project's tier and its place in it, a session's
// own tier, boosts and reservations. The daemon keeps the policy and works out each session's effective priority; the
// window keeps a copy, read when it connects and replaced on every `policy.resources.changed`.
import { createSignal } from "solid-js";
import { daemon } from "./ipc";

/** Daemon API level with `policy.resources.get|set`, `session.priority.set` and the `policy.resources.changed` event. */
export const PRIORITY_API = 52;

export type Tier = "critical" | "normal" | "background";
export const TIERS: readonly Tier[] = ["critical", "normal", "background"];
/** How long a boost lasts: `turn` is until the session's current turn ends. */
export type BoostLength = "30m" | "1h" | "2h" | "turn";

export interface Reserve {
  cores: number;
  ram_mb: number;
  vram_mb: number;
}

export interface ProjectPolicy {
  tier: Tier;
  rank: number;
  reserve?: Reserve | null;
}

export interface SessionPolicy {
  tier?: Tier | null;
  /** `{until: ms}` or `"turn"`. */
  boost?: { until: number } | "turn" | null;
  boosted_at?: number;
  reserve?: Reserve | null;
}

export interface Effective {
  tier: Tier;
  weight: number;
  boosted: boolean;
  /** When the boost ends; null for "until the turn ends". */
  boost_until: number | null;
  source: "boost" | "session" | "project" | "parent" | "default";
  reserve: Reserve | null;
}

export interface Priorities {
  policy: {
    projects: Record<string, ProjectPolicy>;
    sessions: Record<string, SessionPolicy>;
    max_agents: number;
    max_agents_per_project: number | null;
  };
  effective: Record<string, Effective>;
  boosted: string[];
  reserved: Reserve;
  reservable: Reserve;
  max_boosts: number;
}

const [priorities, setPriorities] = createSignal<Priorities | null>(null);
export { priorities };

/** The window's copy, from `policy.resources.get` or a `policy.resources.changed` event (null: an older daemon). */
export function applyPriorities(p: Priorities | null) {
  setPriorities(p);
}

export const loadPriorities = () => daemon<Priorities>("policy.resources.get");
export const setSessionPriority = (id: string, change: { tier?: Tier | "project"; boost?: BoostLength | "off"; replace?: string; reserve?: Reserve | null }) =>
  daemon<Priorities>("session.priority.set", { id, ...change });
export const setProjectPolicy = (id: string, change: { tier?: Tier; rank?: number; reserve?: Reserve | null }) =>
  daemon<Priorities>("policy.resources.set", { projects: { [id]: change } });
/** Puts the projects of `tier` in this order, the top first (dragging in the Priorities page). */
export const orderTier = (tier: Tier, ids: string[]) => daemon<Priorities>("policy.resources.set", { order: { tier, ids } });

/** A session's weight in the ranking (0 for one the daemon does not rank yet). */
export function weightOf(p: Priorities | null, sessionId: string): number {
  return p?.effective[sessionId]?.weight ?? 0;
}

/** A project's tier (normal when it has none of its own). */
export function projectTier(p: Priorities | null, projectId: string): Tier {
  return p?.policy.projects[projectId]?.tier ?? "normal";
}

/** "1 h 5 min", "12 min", "<1 min": what is left of a boost ending at `until`. */
export function boostLeft(until: number, now: number): string {
  const min = Math.max(0, Math.floor((until - now) / 60_000));
  if (until - now < 60_000) return "<1 min";
  const h = Math.floor(min / 60);
  return h ? (min % 60 ? `${h} h ${min % 60} min` : `${h} h`) : `${min} min`;
}

/** Projects grouped by tier, each tier in its rank order (projects without a policy are normal, after the ranked ones, in
 *  the sidebar's order). */
export function projectsByTier<P extends { id: string }>(p: Priorities | null, projects: P[]): Record<Tier, P[]> {
  const out: Record<Tier, P[]> = { critical: [], normal: [], background: [] };
  const order = new Map(projects.map((project, i) => [project.id, i]));
  const place = (id: string) => p?.policy.projects[id]?.rank ?? Number.MAX_SAFE_INTEGER;
  for (const project of projects) out[projectTier(p, project.id)].push(project);
  for (const tier of TIERS) out[tier].sort((a, b) => place(a.id) - place(b.id) || order.get(a.id)! - order.get(b.id)!);
  return out;
}

/** `ids` with `moved` placed before `before` (or at the end): the order a drop in a tier's list makes. */
export function reorder(ids: string[], moved: string, before: string | null): string[] {
  const rest = ids.filter((id) => id !== moved);
  const at = before === null ? rest.length : rest.indexOf(before);
  rest.splice(at < 0 ? rest.length : at, 0, moved);
  return rest;
}
