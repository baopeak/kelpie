// A session's programs laid out as the Processes panel shows them.
import type { ProcInfo, ProcessList } from "./processes";

export interface Row {
  p: ProcInfo;
  depth: number;
}

/** The session's programs in tree order: the root, then each program under its parent (one whose parent is not in the list under the root). */
export function rows(list: ProcessList): Row[] {
  const all = list.children;
  const known = new Set([...all.map((c) => c.pid), ...(list.root ? [list.root.pid] : [])]);
  const under = new Map<number | null, ProcInfo[]>();
  for (const c of all) {
    const parent = c.ppid !== null && known.has(c.ppid) && c.ppid !== c.pid ? c.ppid : list.root?.pid ?? null;
    under.set(parent, [...(under.get(parent) ?? []), c]);
  }
  const out: Row[] = [];
  const seen = new Set<number>();
  const walk = (p: ProcInfo, depth: number) => {
    if (seen.has(p.pid)) return;
    seen.add(p.pid);
    out.push({ p, depth });
    for (const c of under.get(p.pid) ?? []) walk(c, depth + 1);
  };
  if (list.root) walk(list.root, 0);
  for (const c of under.get(null) ?? []) walk(c, 0);
  // Anything a parent cycle left out.
  for (const c of all) walk(c, 1);
  return out;
}

/** `p` and everything under it, as the rows show them. */
export const subtree = (list: ProcessList, p: ProcInfo): ProcInfo[] => {
  const all = rows(list);
  const at = all.findIndex((r) => r.p.pid === p.pid);
  if (at < 0) return [p];
  const out = [all[at].p];
  for (const r of all.slice(at + 1)) {
    if (r.depth <= all[at].depth) break;
    out.push(r.p);
  }
  return out;
};
