// The inbox: sessions across projects, grouped by what they ask of the person, or by the workspace they run in.
import type { SessionRow } from "./link";

export type GroupKey = "needs" | "working" | "done" | "idle" | "stopped";

/** Which group a session belongs to. A session that finished and was not looked at yet is "done". */
export function groupOf(s: SessionRow): GroupKey {
  if (!s.alive) return "stopped";
  if (s.status === "waiting") return "needs";
  if (s.status === "working") return "working";
  return s.unread ? "done" : "idle";
}

export const GROUP_ORDER: GroupKey[] = ["needs", "working", "done", "idle", "stopped"];

/** How sessions are ordered inside a group: the most recent activity first, or by name A–Z. */
export type InboxOrder = "recent" | "name";

/** Needs-you sessions: the longest wait first, unknown start last, whatever the order. Everything else: the most recent activity
 *  first, or by name in the alphabetical order of `locale` (the phone's language), the most recent first among equal names. */
export function compare(group: GroupKey, order: InboxOrder = "recent", locale?: string): (a: SessionRow, b: SessionRow) => number {
  const recent = (a: SessionRow, b: SessionRow) => b.last_activity - a.last_activity;
  if (group === "needs") {
    return (a, b) => (a.waiting_since ?? Number.MAX_SAFE_INTEGER) - (b.waiting_since ?? Number.MAX_SAFE_INTEGER) || recent(a, b);
  }
  if (order === "name") {
    const collator = new Intl.Collator(locale, { numeric: true });
    return (a, b) => collator.compare(labelOf(a), labelOf(b)) || recent(a, b);
  }
  return recent;
}

export function groupSessions(rows: readonly SessionRow[], order: InboxOrder = "recent", locale?: string): { key: GroupKey; rows: SessionRow[] }[] {
  return GROUP_ORDER.map((key) => ({ key, rows: rows.filter((s) => groupOf(s) === key).sort(compare(key, order, locale)) })).filter((g) => g.rows.length);
}

/** The rows the inbox lists, and how many stopped ones it leaves out when stopped sessions are hidden. */
export function shownRows(rows: readonly SessionRow[], showStopped: boolean): { rows: SessionRow[]; hidden: number } {
  const shown = showStopped ? [...rows] : rows.filter((s) => s.alive);
  return { rows: shown, hidden: rows.length - shown.length };
}

/** How many sessions need the person now (the badge). */
export const needsCount = (rows: readonly SessionRow[]) => rows.filter((s) => groupOf(s) === "needs").length;

/** What a session is called: its name, else a short title, else its kind. */
export const labelOf = (s: SessionRow): string => s.name || (s.title && s.title.length < 48 ? s.title.replace(/^[^\w(]+\s*/u, "") : "") || s.kind;

/** Where a session runs, for the inbox's sections: its project when the computer named it, else the last folder of its working
 *  directory, else nothing (the inbox says "Other folders"). `key` tells the sections apart. */
export function workspaceOf(s: SessionRow, projects: Record<string, string>): { key: string; name: string | null } {
  if (s.project_id && projects[s.project_id]) return { key: s.project_id, name: projects[s.project_id] };
  const folder = (s.cwd || "").split(/[\\/]/).filter(Boolean).pop();
  return folder ? { key: `folder:${folder}`, name: folder } : { key: "other", name: null };
}

export const whereOf = (s: SessionRow, projects: Record<string, string>): string => workspaceOf(s, projects).name ?? "";

export interface Workspace {
  key: string;
  name: string | null;
  rows: SessionRow[];
}

/** One section per workspace: those with a session that needs the person first, then the most recently active. Inside one, the
 *  sessions that need the person, then working, done, idle and stopped, each kind in the chosen order. */
export function groupByWorkspace(rows: readonly SessionRow[], projects: Record<string, string>, order: InboxOrder = "recent", locale?: string): Workspace[] {
  const byKey = new Map<string, Workspace>();
  for (const s of rows) {
    const w = workspaceOf(s, projects);
    const section: Workspace = byKey.get(w.key) ?? { ...w, rows: [] };
    section.rows.push(s);
    byKey.set(w.key, section);
  }
  const sections = [...byKey.values()].map((w) => ({ ...w, rows: groupSessions(w.rows, order, locale).flatMap((g) => g.rows) }));
  const needs = (w: Workspace) => (w.rows.some((s) => groupOf(s) === "needs") ? 1 : 0);
  const latest = (w: Workspace) => Math.max(...w.rows.map((s) => s.last_activity));
  return sections.sort((a, b) => needs(b) - needs(a) || latest(b) - latest(a));
}
