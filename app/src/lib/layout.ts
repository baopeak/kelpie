// Pure split-tree operations. Every function returns a new tree (immutable), so the store
// can swap a tab's root in one write and Solid only re-renders the changed subtree.
//
// A split has any number of children laid out in one direction with `sizes` (fractions that
// sum to 1). Splits never nest a child split of the same direction (it is flattened), so a
// grid of R rows × C columns is a `col` split of R `row` splits.
import type { LayoutNode, LayoutTemplate, LeafNode, Pane, SessionKind, SessionTarget, SplitNode } from "./types";
import { uid } from "./platform";

export const leaf = (pane: Pane): LeafNode => ({ type: "leaf", id: uid(), pane });

/** Smallest share a pane can be resized to. */
const MIN_SIZE = 0.04;

export function allPanes(node: LayoutNode): Pane[] {
  return node.type === "leaf" ? [node.pane] : node.children.flatMap(allPanes);
}

/** Panes as the eye reads them: top to bottom, then left to right on the same line — whatever
 *  way the tree nests (a grid can be rows of columns or columns of rows). */
export function readingOrder(node: LayoutNode): Pane[] {
  const placed: { pane: Pane; x: number; y: number }[] = [];
  const walk = (n: LayoutNode, x: number, y: number, w: number, h: number) => {
    if (n.type === "leaf") return void placed.push({ pane: n.pane, x, y });
    let at = 0;
    n.children.forEach((child, i) => {
      const share = n.sizes[i] ?? 1 / n.children.length;
      if (n.dir === "row") walk(child, x + at * w, y, share * w, h);
      else walk(child, x, y + at * h, w, share * h);
      at += share;
    });
  };
  walk(node, 0, 0, 1, 1);
  // Tops within a hair of each other are one line (sizes are fractions that rarely add up exactly).
  return placed.sort((a, b) => (Math.abs(a.y - b.y) > 1e-6 ? a.y - b.y : a.x - b.x)).map((p) => p.pane);
}

export function findPane(node: LayoutNode, paneId: string): Pane | undefined {
  return allPanes(node).find((p) => p.id === paneId);
}

const equal = (n: number) => Array.from({ length: n }, () => 1 / n);

/** Builds a split, flattening children that split the same way (their sizes scale into ours). */
function makeSplit(id: string, dir: "row" | "col", children: LayoutNode[], sizes: number[]): LayoutNode {
  const outKids: LayoutNode[] = [];
  const outSizes: number[] = [];
  children.forEach((c, i) => {
    if (c.type === "split" && c.dir === dir) {
      c.children.forEach((g, j) => {
        outKids.push(g);
        outSizes.push(sizes[i] * c.sizes[j]);
      });
    } else {
      outKids.push(c);
      outSizes.push(sizes[i]);
    }
  });
  if (outKids.length === 1) return outKids[0];
  return { type: "split", id, dir, children: outKids, sizes: normalize(outSizes) };
}

function normalize(sizes: number[]): number[] {
  const total = sizes.reduce((a, b) => a + b, 0);
  return total > 0 ? sizes.map((s) => s / total) : equal(sizes.length);
}

/** Splits the leaf holding `paneId`; the new pane goes after (right / below) it and takes half
 *  of its space. Inside a split of the same direction it becomes a sibling. */
export function splitPane(node: LayoutNode, paneId: string, pane: Pane, dir: "row" | "col"): LayoutNode {
  if (node.type === "leaf") {
    if (node.pane.id !== paneId) return node;
    return { type: "split", id: uid(), dir, children: [node, leaf(pane)], sizes: [0.5, 0.5] };
  }
  const i = node.children.findIndex((c) => c.type === "leaf" && c.pane.id === paneId);
  if (i >= 0 && node.dir === dir) {
    const children = [...node.children];
    const sizes = [...node.sizes];
    children.splice(i + 1, 0, leaf(pane));
    sizes.splice(i, 1, sizes[i] / 2, sizes[i] / 2);
    return { ...node, children, sizes };
  }
  let changed = false;
  const children = node.children.map((c) => {
    const n = changed ? c : splitPane(c, paneId, pane, dir);
    if (n !== c) changed = true;
    return n;
  });
  return changed ? makeSplit(node.id, node.dir, children, node.sizes) : node;
}

/** Removes a pane; its space goes to its neighbours. Returns null when the tree empties. */
export function removePane(node: LayoutNode, paneId: string): LayoutNode | null {
  if (node.type === "leaf") return node.pane.id === paneId ? null : node;
  const kids: LayoutNode[] = [];
  const sizes: number[] = [];
  let changed = false;
  node.children.forEach((c, i) => {
    const n = removePane(c, paneId);
    if (n !== c) changed = true;
    if (n) {
      kids.push(n);
      sizes.push(node.sizes[i]);
    }
  });
  if (!changed) return node;
  if (!kids.length) return null;
  return makeSplit(node.id, node.dir, kids, sizes);
}

export function replacePane(node: LayoutNode, paneId: string, pane: Pane): LayoutNode {
  if (node.type === "leaf") return node.pane.id === paneId ? { ...node, pane } : node;
  let changed = false;
  const children = node.children.map((c) => {
    const n = replacePane(c, paneId, pane);
    if (n !== c) changed = true;
    return n;
  });
  return changed ? { ...node, children } : node;
}

/** Moves the gutter after child `index` of split `splitId` to `pos` (0–1 along the split). */
export function moveGutter(node: LayoutNode, splitId: string, index: number, pos: number): LayoutNode {
  if (node.type === "leaf") return node;
  if (node.id === splitId) {
    const sizes = [...node.sizes];
    if (index < 0 || index >= sizes.length - 1) return node;
    const start = sizes.slice(0, index).reduce((a, b) => a + b, 0);
    const pair = sizes[index] + sizes[index + 1];
    const a = Math.min(pair - MIN_SIZE, Math.max(MIN_SIZE, pos - start));
    if (a <= 0 || pair - a <= 0) return node;
    sizes[index] = a;
    sizes[index + 1] = pair - a;
    return { ...node, sizes };
  }
  let changed = false;
  const children = node.children.map((c) => {
    const n = moveGutter(c, splitId, index, pos);
    if (n !== c) changed = true;
    return n;
  });
  return changed ? { ...node, children } : node;
}

/** Gives every pane of every split an equal share. */
export function equalize(node: LayoutNode): LayoutNode {
  if (node.type === "leaf") return node;
  return { ...node, children: node.children.map(equalize), sizes: equal(node.children.length) };
}

/** Next/previous pane in reading order (wraps). */
export function cyclePane(node: LayoutNode, paneId: string, delta: 1 | -1): string {
  const panes = allPanes(node);
  const i = panes.findIndex((p) => p.id === paneId);
  return panes[(i + delta + panes.length) % panes.length]?.id ?? paneId;
}

/** Swaps two panes' places (their contents move, the layout keeps its shape). */
export function swapPanes(node: LayoutNode, a: string, b: string): LayoutNode {
  const pa = findPane(node, a);
  const pb = findPane(node, b);
  if (!pa || !pb || a === b) return node;
  const swap = (n: LayoutNode): LayoutNode => {
    if (n.type === "leaf") return n.pane.id === a ? { ...n, pane: pb } : n.pane.id === b ? { ...n, pane: pa } : n;
    return { ...n, children: n.children.map(swap) };
  };
  return swap(node);
}

export type Side = "left" | "right" | "top" | "bottom";

/** Puts `sub` (a pane or a whole layout) next to pane `targetId` on `side`, sharing the
 *  target's space half and half. */
export function insertBeside(node: LayoutNode, targetId: string, sub: LayoutNode, side: Side): LayoutNode {
  const dir = side === "left" || side === "right" ? "row" : "col";
  const before = side === "left" || side === "top";
  const place = (n: LayoutNode): LayoutNode => {
    if (n.type === "leaf") {
      if (n.pane.id !== targetId) return n;
      return makeSplit(uid(), dir, before ? [sub, n] : [n, sub], [0.5, 0.5]);
    }
    const i = n.children.findIndex((c) => c.type === "leaf" && c.pane.id === targetId);
    if (i >= 0 && n.dir === dir) {
      const children = [...n.children];
      const sizes = [...n.sizes];
      children.splice(before ? i : i + 1, 0, sub);
      sizes.splice(i, 1, sizes[i] / 2, sizes[i] / 2);
      // `sub` may itself split this way: flatten it in.
      return makeSplit(n.id, n.dir, children, sizes);
    }
    let changed = false;
    const children = n.children.map((c) => {
      const x = changed ? c : place(c);
      if (x !== c) changed = true;
      return x;
    });
    return changed ? makeSplit(n.id, n.dir, children, n.sizes) : n;
  };
  return place(node);
}

/** Moves pane `paneId` next to pane `targetId` on `side` (it leaves its old place). */
export function movePane(node: LayoutNode, paneId: string, targetId: string, side: Side): LayoutNode {
  const pane = findPane(node, paneId);
  if (!pane || paneId === targetId || !findPane(node, targetId)) return node;
  const without = removePane(node, paneId);
  if (!without) return node;
  return insertBeside(without, targetId, leaf(pane), side);
}

// ---------- grids ----------

/** A grid: `rows` mode lists the columns of each row (top to bottom); `cols` mode lists the
 *  panes stacked in each column (left to right). [2, 2] in rows mode is 2 × 2; [1, 2] in cols
 *  mode is one tall pane on the left and two stacked on the right. */
export interface GridSpec {
  mode: "rows" | "cols";
  counts: number[];
}

export const MAX_GRID = 6;

export const cellCount = (spec: GridSpec) => spec.counts.reduce((a, b) => a + b, 0);

export function gridLabel(spec: GridSpec): string {
  const c = spec.counts;
  if (c.every((n) => n === c[0])) return spec.mode === "rows" ? `${c.length} × ${c[0]}` : `${c[0]} × ${c.length}`;
  return spec.mode === "rows" ? `Rows ${c.join(" + ")}` : `Columns ${c.join(" + ")}`;
}

/** Built-in arrangements offered in the layout menu. */
export const GRID_PRESETS: GridSpec[] = [
  { mode: "rows", counts: [2] },
  { mode: "rows", counts: [1, 1] },
  { mode: "rows", counts: [3] },
  { mode: "cols", counts: [1, 2] },
  { mode: "cols", counts: [2, 1] },
  { mode: "rows", counts: [1, 2] },
  { mode: "rows", counts: [2, 2] },
  { mode: "rows", counts: [3, 3] },
  { mode: "rows", counts: [4, 4] },
  { mode: "rows", counts: [3, 3, 3] },
];

export function validGrid(spec: GridSpec): boolean {
  return spec.counts.length >= 1 && spec.counts.length <= MAX_GRID && spec.counts.every((n) => Number.isInteger(n) && n >= 1 && n <= MAX_GRID);
}

/** The most even grid for `n` panes: about as many columns as rows, longer rows first
 *  (4 → 2 + 2, 5 → 3 + 2, 7 → 3 + 2 + 2). Never more than MAX_GRID per line. */
export function autoGrid(n: number): GridSpec {
  const count = Math.max(1, Math.min(n, MAX_GRID * MAX_GRID));
  const cols = Math.min(MAX_GRID, Math.ceil(Math.sqrt(count)));
  const rows = Math.ceil(count / cols);
  const base = Math.floor(count / rows);
  const extra = count % rows;
  return { mode: "rows", counts: Array.from({ length: rows }, (_, i) => base + (i < extra ? 1 : 0)) };
}

/** Lays `panes` out in the grid, in reading order. Missing cells get `filler()`. */
export function buildGrid(spec: GridSpec, panes: Pane[], filler: () => Pane): LayoutNode {
  const queue = [...panes];
  const lines = spec.counts.map((n) => {
    const cells = Array.from({ length: n }, () => leaf(queue.shift() ?? filler()));
    return cells.length === 1 ? cells[0] : ({ type: "split", id: uid(), dir: spec.mode === "rows" ? "row" : "col", children: cells, sizes: equal(n) } as SplitNode);
  });
  if (lines.length === 1) return lines[0];
  return { type: "split", id: uid(), dir: spec.mode === "rows" ? "col" : "row", children: lines, sizes: equal(lines.length) };
}

// ---------- templates (saved layouts) ----------

/** The shape of a layout with what each pane runs, without any session ids. */
export function toTemplate(node: LayoutNode, leafOf: (pane: Pane) => { kind: SessionKind | null; target?: SessionTarget }): LayoutTemplate {
  if (node.type === "leaf") {
    const { kind, target } = leafOf(node.pane);
    return { type: "leaf", kind, ...(target ? { target } : {}) };
  }
  return { type: "split", dir: node.dir, sizes: [...node.sizes], children: node.children.map((c) => toTemplate(c, leafOf)) };
}

export function templateKinds(t: LayoutTemplate): (SessionKind | null)[] {
  return templateLeaves(t).map((l) => l.kind);
}

/** The panes of a template in reading order. */
export function templateLeaves(t: LayoutTemplate): Extract<LayoutTemplate, { type: "leaf" }>[] {
  return t.type === "leaf" ? [t] : t.children.flatMap(templateLeaves);
}

/** Rebuilds a template, asking `make` for each pane (in reading order). */
export function fromTemplate(t: LayoutTemplate, make: (kind: SessionKind | null) => Pane): LayoutNode {
  if (t.type === "leaf") return leaf(make(t.kind));
  const children = t.children.map((c) => fromTemplate(c, make));
  return makeSplit(uid(), t.dir, children, t.sizes.length === children.length ? t.sizes : equal(children.length));
}

/** Converts layouts saved before n-ary splits (`a`, `b`, `ratio`). */
export function migrateLayout(node: any): LayoutNode {
  if (!node || node.type === "leaf") return node;
  if (Array.isArray(node.children)) {
    const kids = node.children.map(migrateLayout);
    return makeSplit(node.id ?? uid(), node.dir, kids, Array.isArray(node.sizes) && node.sizes.length === kids.length ? node.sizes : equal(kids.length));
  }
  const ratio = typeof node.ratio === "number" ? node.ratio : 0.5;
  return makeSplit(node.id ?? uid(), node.dir, [migrateLayout(node.a), migrateLayout(node.b)], [ratio, 1 - ratio]);
}

/** The pane nearest to `from` in a direction, by on-screen rectangles. */
export function paneInDirection(rects: { id: string; r: DOMRect }[], from: string, dir: "left" | "right" | "up" | "down"): string | null {
  const me = rects.find((x) => x.id === from)?.r;
  if (!me) return null;
  const cx = me.left + me.width / 2;
  const cy = me.top + me.height / 2;
  let best: { id: string; d: number } | null = null;
  for (const { id, r } of rects) {
    if (id === from) continue;
    const ok =
      dir === "left" ? r.right <= me.left + 2 : dir === "right" ? r.left >= me.right - 2 : dir === "up" ? r.bottom <= me.top + 2 : r.top >= me.bottom - 2;
    if (!ok) continue;
    // Overlap along the other axis first, then distance.
    const overlap = dir === "left" || dir === "right" ? Math.min(r.bottom, me.bottom) - Math.max(r.top, me.top) : Math.min(r.right, me.right) - Math.max(r.left, me.left);
    const ox = r.left + r.width / 2 - cx;
    const oy = r.top + r.height / 2 - cy;
    const d = Math.hypot(ox, oy) - (overlap > 0 ? 10000 : 0);
    if (!best || d < best.d) best = { id, d };
  }
  return best?.id ?? null;
}
