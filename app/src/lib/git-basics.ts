// Pure helpers for the Changes panel's branch switcher and stash list.
import type { GitBranch } from "./ipc";

/** Branches whose name contains `query` (any case), the current one first, otherwise in the given (recency) order. */
export function filterBranches(list: GitBranch[], query: string): GitBranch[] {
  const q = query.trim().toLowerCase();
  const hits = q ? list.filter((b) => b.name.toLowerCase().includes(q)) : list;
  return [...hits.filter((b) => b.current), ...hits.filter((b) => !b.current)];
}

/**
 * A stash's reflog subject split for display: `WIP on main: abc1234 fix login` (made without a
 * message: the commit it was made on) or `On main: my message`.
 */
export function parseStashSubject(subject: string): { branch: string | null; message: string; wip: boolean } {
  const m = /^(WIP on|On) ([^:]+): ([\s\S]*)$/.exec(subject);
  if (!m) return { branch: null, message: subject, wip: false };
  const wip = m[1] === "WIP on";
  return { branch: m[2] === "(no branch)" ? null : m[2], message: wip ? m[3].replace(/^[0-9a-f]{4,64} /, "") : m[3], wip };
}
