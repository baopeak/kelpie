// Folding of the usage report for the Usage & cost page (loaded with the page, not at startup).
import type { UsageRow } from "./types";

/** Days since the epoch → the Monday that starts its week (also days since the epoch). */
export function weekStart(day: number): number {
  // 1970-01-01 was a Thursday: day 4 (Mon = 0) of its week.
  return day - ((day + 3) % 7);
}

export type GroupBy = "day" | "week" | "project" | "agent" | "model";

export interface Group {
  key: string;
  tokensIn: number;
  tokensOut: number;
  /** Sum of what has a price. */
  cost: number;
  /** Tokens without a price (unknown model, Codex). */
  unpriced: number;
  /** Part of the cost is an estimate at API prices (Claude) rather than the agent's own number. */
  estimate: boolean;
}

/** The report folded by one dimension, from local day `minDay` on. Time dimensions come newest
 *  first; the others by cost, then by tokens. */
export function groupRows(rows: UsageRow[], by: GroupBy, minDay: number | null = null): Group[] {
  const groups = new Map<string, Group>();
  for (const r of rows) {
    if (minDay !== null && r.day < minDay) continue;
    const key = by === "day" ? String(r.day) : by === "week" ? String(weekStart(r.day)) : by === "project" ? r.project : by === "agent" ? r.agent : r.model;
    let g = groups.get(key);
    if (!g) groups.set(key, (g = { key, tokensIn: 0, tokensOut: 0, cost: 0, unpriced: 0, estimate: false }));
    g.tokensIn += r.tokens_in;
    g.tokensOut += r.tokens_out;
    g.cost += r.cost;
    g.unpriced += r.unpriced;
    if (r.cost > 0 && r.agent !== "omp") g.estimate = true;
  }
  const list = [...groups.values()];
  if (by === "day" || by === "week") return list.sort((a, b) => Number(b.key) - Number(a.key));
  return list.sort((a, b) => b.cost - a.cost || b.tokensIn + b.tokensOut - (a.tokensIn + a.tokensOut));
}

/** The last path segment of a folder (either separator), for a compact project label. */
export function folderName(path: string): string {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}
