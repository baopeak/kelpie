// The order of a project's sessions in the sidebar: the order they are on screen. Tabs left to right, and in each tab its
// panes in the layout tree's order: a split's first child before its second, so a side-by-side split lists left before right
// and a stacked one top before bottom, each half whole before the next. Derived from the tree alone (never from pane sizes or
// the page), so only a change of layout (a pane split, moved or closed, a tab dragged) moves a row; dragging a divider never does.
import { allPanes } from "./layout";
import type { Tab } from "./types";

/** Each session shown in `tabs` and its place on screen (0 first): tab by tab, each tab's panes in tree order. A session shown
 *  in two panes takes the first. */
export function screenPlaces(tabs: readonly Tab[]): Map<string, number> {
  const place = new Map<string, number>();
  for (const tab of tabs) {
    for (const p of allPanes(tab.root)) if (p.sessionId && !place.has(p.sessionId)) place.set(p.sessionId, place.size);
  }
  return place;
}

/** `places` with each session that is taking over another's conversation (`takeovers`: new id → replaced id) and the session it
 *  replaces at the earlier of their two places: during a switch between terminal and chat the pane goes from one to the other,
 *  and neither row should drop below the sessions on screen meanwhile. */
export function shareTakeoverPlaces(places: ReadonlyMap<string, number>, takeovers: Readonly<Record<string, string>>): Map<string, number> {
  const out = new Map(places);
  for (const [to, from] of Object.entries(takeovers)) {
    const a = places.get(to);
    const b = places.get(from);
    const at = a === undefined ? b : b === undefined ? a : Math.min(a, b);
    if (at === undefined) continue;
    out.set(to, at);
    out.set(from, at);
  }
  return out;
}

/** `rows` (the top of each family, in their stable creation order) in screen order. A family moves as one block: its row sits at
 *  the earliest place of any session in it (`topOf` gives the row a session is listed under; an id that is none of `rows`,
 *  or undefined, for one listed elsewhere), so showing a child in its parent's pane keeps the family where it was. Rows on screen nowhere come after, and
 *  they, like any tie, keep their order in `rows`. */
export function orderByScreen<T extends { id: string }>(
  rows: readonly T[],
  places: ReadonlyMap<string, number>,
  topOf: (sessionId: string) => string | undefined,
): T[] {
  const best = new Map<string, number>();
  for (const [id, at] of places) {
    const top = topOf(id);
    if (top === undefined) continue;
    const was = best.get(top);
    if (was === undefined || at < was) best.set(top, at);
  }
  if (!best.size) return [...rows];
  const index = new Map(rows.map((r, i) => [r.id, i]));
  const rank = (r: T) => best.get(r.id) ?? Number.POSITIVE_INFINITY;
  return [...rows].sort((a, b) => {
    const ra = rank(a);
    const rb = rank(b);
    if (ra !== rb) return ra < rb ? -1 : 1;
    return index.get(a.id)! - index.get(b.id)!;
  });
}
