// Checks still owed (daemon API 47): the real-world checks a change could not get (on the real app, a real phone, a clean machine,
// over time, or the owner's own) that an agent or the user recorded with `owed.open`. This is the pure part, which the Activity
// panel's group is built on, and the window's copy of the open entries by project. Nothing is persisted: the daemon keeps the record.
import { createStore, reconcile } from "solid-js/store";
import { t } from "./i18n";

/** Daemon API level that has `owed.list|open|close` and the `owed.changed` event. */
export const OWED_API = 47;

export type OwedKind = "real_app" | "phone" | "clean_machine" | "soak" | "owner";

/** An entry as the daemon sends it (`owed.rs`); only the fields the window shows. */
export interface Owed {
  id: string;
  project: string;
  pr?: number;
  kind: OwedKind;
  what: string;
  closes_by: string;
  /** `user`, or `session:<id>`. */
  opened_by: string;
  opened_ms: number;
  closed_ms?: number;
}

const KINDS: readonly string[] = ["real_app", "phone", "clean_machine", "soak", "owner"];

/** The short label of a kind. An unknown kind (a newer daemon) shows as it is written. */
export function kindLabel(kind: string): string {
  switch (kind) {
    case "real_app":
      return t("Real app");
    case "phone":
      return t("Phone");
    case "clean_machine":
      return t("Clean machine");
    case "soak":
      return t("Over time");
    case "owner":
      return t("Owner's check");
    default:
      return kind;
  }
}

/** An entry from the daemon, or null when it lacks what the window needs (id, project, texts, a time). */
export function toOwed(raw: unknown): Owed | null {
  const r = raw as Record<string, unknown> | null;
  if (!r || typeof r !== "object") return null;
  if (typeof r.id !== "string" || !r.id || typeof r.project !== "string" || !r.project) return null;
  if (typeof r.kind !== "string" || typeof r.what !== "string" || typeof r.closes_by !== "string") return null;
  if (typeof r.opened_ms !== "number" || !Number.isFinite(r.opened_ms)) return null;
  return {
    id: r.id,
    project: r.project,
    pr: typeof r.pr === "number" ? r.pr : undefined,
    kind: (KINDS.includes(r.kind) ? r.kind : "owner") as OwedKind,
    what: r.what,
    closes_by: r.closes_by,
    opened_by: typeof r.opened_by === "string" ? r.opened_by : "",
    opened_ms: r.opened_ms,
    closed_ms: typeof r.closed_ms === "number" ? r.closed_ms : undefined,
  };
}

/** What the group shows for one entry. */
export interface OwedRow {
  id: string;
  /** Short label of the kind. */
  kind: string;
  what: string;
  closesBy: string;
  /** Who opened it: "you", or the agent session's name. */
  by: string;
  /** Milliseconds since it was opened (the panel turns it into text with its clock). */
  ageMs: number;
  pr?: number;
}

/** The open entries of one project as rows, oldest first. `sessionName` gives the name of a session that is still known. */
export function owedRows(entries: readonly Owed[], now: number, sessionName: (id: string) => string | undefined): OwedRow[] {
  return entries
    .filter((e) => e.closed_ms === undefined)
    .sort((a, b) => a.opened_ms - b.opened_ms || a.id.localeCompare(b.id))
    .map((e) => {
      const by = e.opened_by.startsWith("session:") ? (sessionName(e.opened_by.slice("session:".length)) ?? t("an agent")) : e.opened_by === "user" ? t("you") : e.opened_by || t("an agent");
      return { id: e.id, kind: kindLabel(e.kind), what: e.what, closesBy: e.closes_by, by, ageMs: Math.max(0, now - e.opened_ms), pr: e.pr };
    });
}

// ---------- the window's copy ----------

/** The open entries by project id, as the daemon last said. */
export const [openOwed, setOpenOwed] = createStore<Record<string, Owed[]>>({});

/** The open entries of a list the daemon sent (closed ones and entries that do not parse are dropped). */
export function openEntries(raw: unknown): Owed[] {
  return (Array.isArray(raw) ? raw : []).map(toOwed).filter((e): e is Owed => !!e && e.closed_ms === undefined);
}

/** All projects' entries from `owed.list`: replaces the window's copy. */
export function replaceOwed(raw: unknown) {
  const next: Record<string, Owed[]> = {};
  for (const e of openEntries(raw)) (next[e.project] ??= []).push(e);
  setOpenOwed(reconcile(next));
}

/** One project's whole list from an `owed.changed` event. */
export function applyOwedChanged(project: string, raw: unknown) {
  const list = openEntries(raw).filter((e) => e.project === project);
  setOpenOwed(project, list.length ? reconcile(list) : undefined!);
}
