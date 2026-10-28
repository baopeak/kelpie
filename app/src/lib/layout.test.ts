import { describe, expect, it } from "vitest";
import {
  allPanes, autoGrid, buildGrid, cellCount, cyclePane, equalize, fromTemplate, gridLabel, insertBeside, leaf, migrateLayout, movePane,
  moveGutter, readingOrder, removePane, splitPane, swapPanes, templateKinds, toTemplate, validGrid, MAX_GRID,
} from "./layout";
import type { LayoutNode, Pane } from "./types";

const pane = (id: string): Pane => ({ id, kind: "terminal", sessionId: `s-${id}` });
const ids = (n: LayoutNode) => allPanes(n).map((p) => p.id);
const sum = (xs: number[]) => xs.reduce((a, b) => a + b, 0);

/** Per-line pane counts of a grid built by `buildGrid`: "3,2" = a row of 3 over a row of 2. */
function counts(n: LayoutNode): string {
  if (n.type === "leaf") return "1";
  const lines = n.children.map((c) => (c.type === "split" ? c.children.length : 1));
  // A lone line of cells is the split itself.
  return n.children.every((c) => c.type === "leaf") ? String(n.children.length) : lines.join(",");
}

/** Every split keeps sizes that sum to 1 and never nests a same-direction split. */
function expectWellFormed(n: LayoutNode) {
  if (n.type === "leaf") return;
  expect(n.children.length).toBe(n.sizes.length);
  expect(n.children.length).toBeGreaterThanOrEqual(2);
  expect(sum(n.sizes)).toBeCloseTo(1, 9);
  for (const c of n.children) {
    if (c.type === "split") expect(c.dir).not.toBe(n.dir);
    expectWellFormed(c);
  }
}

describe("autoGrid", () => {
  it.each([
    [1, [1]],
    [2, [2]],
    [3, [2, 1]],
    [4, [2, 2]],
    [5, [3, 2]],
    [6, [3, 3]],
    [7, [3, 2, 2]],
    [9, [3, 3, 3]],
  ])("%i panes -> rows %j", (n, expected) => {
    const spec = autoGrid(n);
    expect(spec.mode).toBe("rows");
    expect(spec.counts).toEqual(expected);
    expect(cellCount(spec)).toBe(n);
  });

  it("never exceeds MAX_GRID per line and clamps huge counts", () => {
    const spec = autoGrid(500);
    expect(spec.counts.length).toBeLessThanOrEqual(MAX_GRID);
    expect(Math.max(...spec.counts)).toBeLessThanOrEqual(MAX_GRID);
    expect(validGrid(spec)).toBe(true);
    expect(cellCount(autoGrid(0))).toBe(1);
  });
});

describe("gridLabel / validGrid", () => {
  it("labels even and uneven grids", () => {
    expect(gridLabel({ mode: "rows", counts: [2, 2] })).toBe("2 × 2");
    expect(gridLabel({ mode: "cols", counts: [1, 2] })).toBe("Columns 1 + 2");
    expect(gridLabel({ mode: "rows", counts: [3, 2] })).toBe("Rows 3 + 2");
  });

  it("rejects empty, oversized and fractional specs", () => {
    expect(validGrid({ mode: "rows", counts: [] })).toBe(false);
    expect(validGrid({ mode: "rows", counts: [0, 2] })).toBe(false);
    expect(validGrid({ mode: "rows", counts: [1.5] })).toBe(false);
    expect(validGrid({ mode: "rows", counts: [MAX_GRID + 1] })).toBe(false);
    expect(validGrid({ mode: "rows", counts: Array(MAX_GRID + 1).fill(1) })).toBe(false);
    expect(validGrid({ mode: "cols", counts: [1, 2] })).toBe(true);
  });
});

describe("buildGrid", () => {
  const filler = () => pane("filler");

  it("lays panes out in reading order with the per-row counts", () => {
    const panes = ["a", "b", "c", "d", "e"].map(pane);
    const g = buildGrid({ mode: "rows", counts: [3, 2] }, panes, filler);
    expect(counts(g)).toBe("3,2");
    expect(ids(g)).toEqual(["a", "b", "c", "d", "e"]);
    expect(g.type === "split" && g.dir).toBe("col");
    expectWellFormed(g);
  });

  it("cols mode stacks panes inside columns", () => {
    const g = buildGrid({ mode: "cols", counts: [1, 2] }, ["a", "b", "c"].map(pane), filler);
    expect(g.type === "split" && g.dir).toBe("row");
    expect(counts(g)).toBe("1,2");
    expectWellFormed(g);
  });

  it("fills missing cells and ignores surplus panes' cells", () => {
    const g = buildGrid({ mode: "rows", counts: [2, 2] }, [pane("a")], filler);
    expect(ids(g)).toEqual(["a", "filler", "filler", "filler"]);
  });

  it("a single cell is a leaf, a single line is one split", () => {
    expect(buildGrid({ mode: "rows", counts: [1] }, [pane("a")], filler).type).toBe("leaf");
    const line = buildGrid({ mode: "rows", counts: [3] }, ["a", "b", "c"].map(pane), filler);
    expect(line.type === "split" && line.dir).toBe("row");
    expect(line.type === "split" && line.sizes).toEqual([1 / 3, 1 / 3, 1 / 3]);
  });

  it("tiling N panes with autoGrid keeps every pane once (tile helper)", () => {
    for (const n of [2, 3, 4, 5, 7, 9]) {
      const panes = Array.from({ length: n }, (_, i) => pane(`p${i}`));
      const g = buildGrid(autoGrid(n), panes, filler);
      expect(ids(g)).toEqual(panes.map((p) => p.id));
      expectWellFormed(g);
    }
    expect(counts(buildGrid(autoGrid(5), Array.from({ length: 5 }, (_, i) => pane(`p${i}`)), filler))).toBe("3,2");
  });
});

describe("splitPane / removePane", () => {
  it("splits a leaf in half, and a second split the same way flattens", () => {
    let t: LayoutNode = leaf(pane("a"));
    t = splitPane(t, "a", pane("b"), "row");
    expect(t.type === "split" && t.sizes).toEqual([0.5, 0.5]);
    t = splitPane(t, "b", pane("c"), "row");
    expect(t.type === "split" && t.children.length).toBe(3);
    expect(t.type === "split" && t.sizes).toEqual([0.5, 0.25, 0.25]);
    expectWellFormed(t);
  });

  it("splitting across keeps the old pane order", () => {
    let t: LayoutNode = leaf(pane("a"));
    t = splitPane(t, "a", pane("b"), "row");
    t = splitPane(t, "a", pane("c"), "col");
    expect(ids(t)).toEqual(["a", "c", "b"]);
    expectWellFormed(t);
  });

  it("unknown pane id leaves the same tree object", () => {
    const t = splitPane(leaf(pane("a")), "a", pane("b"), "row");
    expect(splitPane(t, "zzz", pane("c"), "row")).toBe(t);
  });

  it("removing gives the space to the neighbours and collapses to a leaf", () => {
    let t = splitPane(leaf(pane("a")), "a", pane("b"), "row");
    t = splitPane(t, "b", pane("c"), "row");
    const r = removePane(t, "a")!;
    expect(ids(r)).toEqual(["b", "c"]);
    expect(r.type === "split" && sum(r.sizes)).toBeCloseTo(1, 9);
    const last = removePane(r, "b")!;
    expect(last.type).toBe("leaf");
    expect(removePane(last, "c")).toBeNull();
    expect(removePane(last, "missing")).toBe(last);
  });
});

describe("rearranging", () => {
  const grid = () => buildGrid({ mode: "rows", counts: [2, 2] }, ["a", "b", "c", "d"].map(pane), () => pane("x"));

  it("swapPanes exchanges contents and keeps the shape", () => {
    const g = grid();
    const s = swapPanes(g, "a", "d");
    expect(ids(s)).toEqual(["d", "b", "c", "a"]);
    expect(counts(s)).toBe("2,2");
    expect(swapPanes(g, "a", "a")).toBe(g);
    expect(swapPanes(g, "a", "nope")).toBe(g);
  });

  it("movePane puts a pane beside another and removes it from its old place", () => {
    const g = grid();
    const m = movePane(g, "a", "d", "right");
    expect(ids(m)).toEqual(["b", "c", "d", "a"]);
    expectWellFormed(m);
    expect(movePane(g, "a", "a", "right")).toBe(g);
    expect(movePane(g, "a", "nope", "right")).toBe(g);
  });

  it("insertBeside flattens a same-direction sub layout", () => {
    const row = splitPane(leaf(pane("a")), "a", pane("b"), "row");
    const sub = splitPane(leaf(pane("c")), "c", pane("d"), "row");
    const t = insertBeside(row, "a", sub, "right");
    expect(ids(t)).toEqual(["a", "c", "d", "b"]);
    expect(t.type === "split" && t.children.length).toBe(4);
    expectWellFormed(t);
  });

  it("equalize gives every split equal shares", () => {
    let t = splitPane(leaf(pane("a")), "a", pane("b"), "row");
    t = splitPane(t, "b", pane("c"), "row");
    const e = equalize(t);
    expect(e.type === "split" && e.sizes.every((s) => Math.abs(s - 1 / 3) < 1e-9)).toBe(true);
  });

  it("cyclePane wraps in both directions", () => {
    const g = grid();
    expect(cyclePane(g, "a", 1)).toBe("b");
    expect(cyclePane(g, "d", 1)).toBe("a");
    expect(cyclePane(g, "a", -1)).toBe("d");
  });

  it("moveGutter clamps to the minimum size and keeps the pair's total", () => {
    const t = splitPane(leaf(pane("a")), "a", pane("b"), "row");
    if (t.type !== "split") throw new Error("expected a split");
    const moved = moveGutter(t, t.id, 0, 0.7);
    expect(moved.type === "split" && moved.sizes[0]).toBeCloseTo(0.7, 9);
    const clamped = moveGutter(t, t.id, 0, 0.999);
    expect(clamped.type === "split" && clamped.sizes[1]).toBeCloseTo(0.04, 9);
    expect(moveGutter(t, t.id, 5, 0.5)).toBe(t);
  });
});

describe("templates and migration", () => {
  it("toTemplate / fromTemplate round-trip the shape and kinds", () => {
    const g = buildGrid({ mode: "rows", counts: [3, 2] }, ["a", "b", "c", "d", "e"].map(pane), () => pane("x"));
    const t = toTemplate(g, (p) => ({ kind: p.id === "c" ? "claude" : "shell" }));
    expect(templateKinds(t)).toEqual(["shell", "shell", "claude", "shell", "shell"]);
    let n = 0;
    const back = fromTemplate(t, (kind) => ({ id: `n${n++}`, kind: "terminal", sessionId: kind ?? undefined }));
    expect(counts(back)).toBe("3,2");
    expect(ids(back)).toEqual(["n0", "n1", "n2", "n3", "n4"]);
    expectWellFormed(back);
  });

  it("fromTemplate repairs sizes of the wrong length", () => {
    const back = fromTemplate(
      { type: "split", dir: "row", sizes: [1], children: [{ type: "leaf", kind: null }, { type: "leaf", kind: null }] },
      () => pane("e"),
    );
    expect(back.type === "split" && back.sizes).toEqual([0.5, 0.5]);
  });

  it("migrateLayout converts the old a/b/ratio form and flattens it", () => {
    const old = {
      type: "split", id: "s1", dir: "row", ratio: 0.25,
      a: { type: "leaf", id: "l1", pane: pane("a") },
      b: { type: "split", id: "s2", dir: "row", ratio: 0.5, a: { type: "leaf", id: "l2", pane: pane("b") }, b: { type: "leaf", id: "l3", pane: pane("c") } },
    };
    const n = migrateLayout(old);
    expect(ids(n)).toEqual(["a", "b", "c"]);
    expect(n.type === "split" && n.sizes).toEqual([0.25, 0.375, 0.375]);
    expectWellFormed(n);
  });

  it("migrateLayout keeps n-ary splits and passes leaves through", () => {
    const g = buildGrid({ mode: "rows", counts: [2, 1] }, ["a", "b", "c"].map(pane), () => pane("x"));
    expect(ids(migrateLayout(JSON.parse(JSON.stringify(g))))).toEqual(["a", "b", "c"]);
    expect(migrateLayout(null)).toBeNull();
  });
});

describe("readingOrder", () => {
  const split = (dir: "row" | "col", children: LayoutNode[], sizes?: number[]): LayoutNode =>
    ({ type: "split", id: dir + children.length, dir, children, sizes: sizes ?? children.map(() => 1 / children.length) });
  const order = (n: LayoutNode) => readingOrder(n).map((p) => p.id);

  it("reads a 2x2 grid line by line, however the tree nests it", () => {
    // Rows of panes, and columns of panes, drawing the same grid.
    expect(order(split("col", [split("row", [leaf(pane("a")), leaf(pane("b"))]), split("row", [leaf(pane("c")), leaf(pane("d"))])]))).toEqual(["a", "b", "c", "d"]);
    expect(order(split("row", [split("col", [leaf(pane("a")), leaf(pane("c"))]), split("col", [leaf(pane("b")), leaf(pane("d"))])]))).toEqual(["a", "b", "c", "d"]);
  });

  it("puts a tall pane before the panes beside it", () => {
    expect(order(split("row", [leaf(pane("tall")), split("col", [leaf(pane("top")), leaf(pane("bottom"))], [0.3, 0.7])]))).toEqual(["tall", "top", "bottom"]);
  });
});
