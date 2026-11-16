// Which worktrees of a repository the sidebar lists and which it folds under "N more worktrees":
// a repository with many worktrees shows the ones that mean something now and keeps the quiet rest
// one click away.

/** A repository folds its quiet worktrees when it has more than this many. */
export const WORKTREE_FOLD_LIMIT = 4;

/** What makes a worktree worth a row of its own while the list is folded. */
export interface WorktreeSignals {
  /** It is the open project. */
  active: boolean;
  /** One of its sessions runs, or one asks for the user (waiting, failed, finished unread). */
  sessions: boolean;
  /** Its git state; unknown until first read. */
  info?: { dirty: boolean; ahead: number } | null;
}

/** A worktree stays in view when it is open, has sessions to watch, or holds uncommitted or unpushed work. */
export const isNotableWorktree = (w: WorktreeSignals) => w.active || w.sessions || !!w.info?.dirty || (w.info?.ahead ?? 0) > 0;

/** Splits `list` into the worktrees shown and those folded away. Up to `limit` worktrees are all shown, in
 *  their order. Above it the notable ones are shown (in their order) and the rest are folded, unless that
 *  would fold a single row: a "1 more" row saves nothing, so then everything is shown. */
export function foldWorktrees<T>(list: readonly T[], notable: (w: T) => boolean, limit = WORKTREE_FOLD_LIMIT): { shown: T[]; hidden: T[] } {
  if (list.length <= limit) return { shown: [...list], hidden: [] };
  const shown: T[] = [];
  const hidden: T[] = [];
  for (const w of list) (notable(w) ? shown : hidden).push(w);
  return hidden.length < 2 ? { shown: [...list], hidden: [] } : { shown, hidden };
}
