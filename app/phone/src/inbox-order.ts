// The inbox: sessions across projects, grouped by what they ask of the person.
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

/** Needs-you sessions: the longest wait first, unknown start last. Everything else: most recent activity first. */
export function compare(group: GroupKey): (a: SessionRow, b: SessionRow) => number {
  if (group === "needs") {
    return (a, b) => (a.waiting_since ?? Number.MAX_SAFE_INTEGER) - (b.waiting_since ?? Number.MAX_SAFE_INTEGER) || b.last_activity - a.last_activity;
  }
  return (a, b) => b.last_activity - a.last_activity;
}

export function groupSessions(rows: readonly SessionRow[]): { key: GroupKey; rows: SessionRow[] }[] {
  return GROUP_ORDER.map((key) => ({ key, rows: rows.filter((s) => groupOf(s) === key).sort(compare(key)) })).filter((g) => g.rows.length);
}

/** How many sessions need the person now (the badge). */
export const needsCount = (rows: readonly SessionRow[]) => rows.filter((s) => groupOf(s) === "needs").length;

/** What a session is called: its name, else a short title, else its kind. */
export const labelOf = (s: SessionRow): string => s.name || (s.title && s.title.length < 48 ? s.title.replace(/^[^\w(]+\s*/u, "") : "") || s.kind;

export const whereOf = (s: SessionRow, projects: Record<string, string>): string =>
  (s.project_id && projects[s.project_id]) || (s.cwd || "").split(/[\\/]/).filter(Boolean).pop() || "";
