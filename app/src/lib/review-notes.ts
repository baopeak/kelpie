// Review notes live in the daemon from API 13 (crates/kelpied/src/notes.rs), keyed by worktree
// folder, so the diff gutter, the review card, `@note` and a reviewing agent's `kelpied review add`
// share one store. A daemon from before keeps them here, in the app's state. Pure helpers: the
// store wires them to the daemon.
import type { ReviewNote } from "./types";

/** First daemon API level with `note.*` and run worktrees that bring `copy:` / `link:` files. */
export const NOTES_API = 13;

/** The notes filed under `key` and nowhere else: a worktree inside another one (or inside a repository)
 *  never reads or sends the notes of the folder around it. The daemon says which key a project's
 *  folder has (`note.keys`); `key` and the map's keys are `metaKey`s. */
export function notesForKey(byKey: Record<string, ReviewNote[]>, key: string): ReviewNote[] {
  return byKey[key] ?? [];
}

/** `own` first, then the ones of `others` it lacks (by id). */
export function mergeNotes(own: ReviewNote[], others: ReviewNote[]): ReviewNote[] {
  const seen = new Set(own.map((n) => n.id));
  return [...own, ...others.filter((n) => !seen.has(n.id))];
}

export interface PendingNotes {
  projectId: string;
  path: string;
  notes: ReviewNote[];
}

/** The notes still kept in the app's state, per project that is still in the sidebar. */
export function pendingNotes(local: Record<string, ReviewNote[]> | undefined, projects: { id: string; path: string }[]): PendingNotes[] {
  const out: PendingNotes[] = [];
  for (const p of projects) {
    const notes = local?.[p.id];
    if (notes?.length) out.push({ projectId: p.id, path: p.path, notes });
  }
  return out;
}

export interface Migration {
  moved: number;
  /** Projects whose notes stay in the app's state because the daemon refused one. */
  failed: string[];
  /** The copy of the app's state could not be made: nothing was moved. */
  noBackup: boolean;
}

/**
 * Moves the notes kept in the app's state to the daemon. A copy of the state comes first; a
 * project's notes leave the state only once the daemon has every one of them (it keeps a note
 * once per id, so running this again after a stop adds nothing twice).
 */
export async function migrateNotes(
  pending: PendingNotes[],
  io: { backup(): Promise<string | null>; add(path: string, note: ReviewNote): Promise<void>; drop(projectId: string): void },
): Promise<Migration> {
  const result: Migration = { moved: 0, failed: [], noBackup: false };
  if (!pending.length) return result;
  if (!(await io.backup())) return { ...result, noBackup: true };
  for (const p of pending) {
    try {
      for (const n of p.notes) await io.add(p.path, n);
      io.drop(p.projectId);
      result.moved += p.notes.length;
    } catch {
      result.failed.push(p.projectId);
    }
  }
  return result;
}
