import { describe, expect, it } from "vitest";
import { orderByScreen, screenPlaces, shareTakeoverPlaces } from "./session-order";
import type { LayoutNode, Pane, Tab } from "./types";

const pane = (session?: string, kind: Pane["kind"] = "terminal"): LayoutNode =>
  ({ type: "leaf", id: `l-${session ?? Math.random()}`, pane: { id: `p-${session ?? Math.random()}`, kind, sessionId: session } });
const split = (dir: "row" | "col", children: LayoutNode[], sizes?: number[]): LayoutNode =>
  ({ type: "split", id: `${dir}${children.length}`, dir, children, sizes: sizes ?? children.map(() => 1 / children.length) });
const tab = (root: LayoutNode): Tab => ({ id: `t-${Math.random()}`, root, focus: "" });

/** Rows given oldest first; no family unless `families` says which row a session is listed under. Like the sidebar, a session
 *  that is no one's child maps to itself, also when it is another project's (no row here). */
const order = (oldestFirst: string[], tabs: Tab[], families: Record<string, string> = {}) =>
  orderByScreen(oldestFirst.map((id) => ({ id })), screenPlaces(tabs), (id) => families[id] ?? id)
    .map((r) => r.id);

describe("screen order of a project's sessions", () => {
  it("lists the left pane before the right one, whichever is older", () => {
    // The owner's report: the newer session sits on the left, and used to be listed second.
    expect(order(["old", "new"], [tab(split("row", [pane("new"), pane("old")]))])).toEqual(["new", "old"]);
  });

  it("lists the top pane before the bottom one", () => {
    expect(order(["a", "b"], [tab(split("col", [pane("b"), pane("a")]))])).toEqual(["b", "a"]);
  });

  it("reads a split's first half whole before its second: a grid of columns column by column, a grid of rows row by row", () => {
    const columns = split("row", [split("col", [pane("tl"), pane("bl")]), split("col", [pane("tr"), pane("br")])]);
    expect(order(["br", "bl", "tr", "tl"], [tab(columns)])).toEqual(["tl", "bl", "tr", "br"]);
    const lines = split("col", [split("row", [pane("tl"), pane("tr")]), split("row", [pane("bl"), pane("br")])]);
    expect(order(["br", "bl", "tr", "tl"], [tab(lines)])).toEqual(["tl", "tr", "bl", "br"]);
  });

  it("never reorders when a divider is dragged: pane sizes play no part", () => {
    // The left column's divider dragged far below the right column's: the tops cross, the order stays.
    const grid = (left: number, right: number) =>
      split("row", [split("col", [pane("tl"), pane("bl")], [left, 1 - left]), split("col", [pane("tr"), pane("br")], [right, 1 - right])]);
    expect(order(["tl", "bl", "tr", "br"], [tab(grid(0.2, 0.8))])).toEqual(order(["tl", "bl", "tr", "br"], [tab(grid(0.8, 0.2))]));
  });

  it("lists tab 1 before tab 2, and follows a tab dragged before another", () => {
    const one = tab(pane("a"));
    const two = tab(split("row", [pane("b"), pane("c")]));
    expect(order(["c", "b", "a"], [one, two])).toEqual(["a", "b", "c"]);
    expect(order(["c", "b", "a"], [two, one])).toEqual(["b", "c", "a"]);
  });

  it("keeps a family in one block at its first member's place, so a child shown in the parent's pane moves nothing", () => {
    const families = { child: "parent" };
    const rows = ["parent", "other"];
    const withParent = [tab(pane("other")), tab(pane("parent"))];
    const withChild = [tab(pane("other")), tab(pane("child"))];
    expect(order(rows, withParent, families)).toEqual(["other", "parent"]);
    expect(order(rows, withChild, families)).toEqual(["other", "parent"]);
    // A child in a pane before the parent's brings the family up to it.
    expect(order(rows, [tab(pane("child")), tab(pane("other")), tab(pane("parent"))], families)).toEqual(["parent", "other"]);
  });

  it("puts sessions on screen nowhere after the placed ones, oldest first", () => {
    expect(order(["a", "b", "c", "d"], [tab(split("row", [pane("d"), pane("b")]))])).toEqual(["d", "b", "a", "c"]);
  });

  it("keeps creation order when nothing is on screen, and ignores panes without a session or of other projects", () => {
    expect(order(["a", "b"], [])).toEqual(["a", "b"]);
    expect(order(["a", "b"], [tab(split("row", [pane(undefined, "file"), pane("elsewhere"), pane("b")]))])).toEqual(["b", "a"]);
  });

  it("places a session shown in two panes at the first", () => {
    expect(order(["a", "b"], [tab(split("row", [pane("a"), pane("b")])), tab(pane("a"))])).toEqual(["a", "b"]);
    expect(screenPlaces([tab(split("row", [pane("b"), pane("a")])), tab(pane("b"))])).toEqual(new Map([["b", 0], ["a", 1]]));
  });

  it("keeps a conversation switched between terminal and chat in its place while the pane passes to the new session", () => {
    const rows = ["old", "other", "new"];
    // The new session is listed while the old one still has the pane, then the old one is listed while the new one has it.
    const before = shareTakeoverPlaces(screenPlaces([tab(split("row", [pane("other"), pane("old")]))]), { new: "old" });
    expect(orderByScreen(rows.map((id) => ({ id })), before, (id) => id).map((r) => r.id)).toEqual(["other", "old", "new"]);
    const after = shareTakeoverPlaces(screenPlaces([tab(split("row", [pane("other"), pane("new")]))]), { new: "old" });
    expect(orderByScreen(rows.map((id) => ({ id })), after, (id) => id).map((r) => r.id)).toEqual(["other", "old", "new"]);
    // Neither on screen: nothing to share.
    expect(shareTakeoverPlaces(new Map([["other", 0]]), { new: "old" })).toEqual(new Map([["other", 0]]));
  });
});
