import { describe, expect, it } from "vitest";
import { bitsMode, changedBits, clickSelect, filterEntries, formatSize, joinPath, listNames, modeBits, modeString, octal, parentPath, parseOctal, pruneSelection, selectAll, sharedBits, sortEntries, withChange } from "./file-pane";
import type { RemoteEntry } from "./types";

const e = (name: string, over: Partial<RemoteEntry> = {}): RemoteEntry => ({
  name, kind: "file", size: 0, mtime: 0, mode: 0o644, uid: null, gid: null, owner: null, group: null, link_target: null, readable_name: true, hidden: false, ...over,
});

describe("sorting and filtering", () => {
  const list = [e("file10"), e("b", { kind: "dir" }), e("file2", { size: 5 }), e("A", { kind: "dir" }), e("big", { size: 100, mtime: 9 })];
  it("puts folders first and names in natural order", () => {
    expect(sortEntries(list, "name").map((x) => x.name)).toEqual(["A", "b", "big", "file2", "file10"]);
  });
  it("sorts by size, folders still first, and reverses within groups", () => {
    expect(sortEntries(list, "size").map((x) => x.name)).toEqual(["A", "b", "file10", "file2", "big"]);
    expect(sortEntries(list, "size", false).map((x) => x.name)).toEqual(["b", "A", "big", "file2", "file10"]);
  });
  it("sorts by time", () => {
    expect(sortEntries(list, "mtime").at(-1)?.name).toBe("big");
  });
  it("filters by name, ignoring case", () => {
    expect(filterEntries(list, "FILE").map((x) => x.name)).toEqual(["file10", "file2"]);
    expect(filterEntries(list, "  ")).toHaveLength(5);
  });
});

describe("selection", () => {
  const order = ["a", "b", "c", "d", "e"];
  it("picks one, toggles with Ctrl, ranges with Shift", () => {
    let s = clickSelect({ picked: [], anchor: null }, order, "b", {});
    expect(s).toEqual({ picked: ["b"], anchor: "b" });
    s = clickSelect(s, order, "d", { range: true });
    expect(s.picked).toEqual(["b", "c", "d"]);
    s = clickSelect(s, order, "c", { toggle: true });
    expect(s.picked).toEqual(["b", "d"]);
    s = clickSelect({ picked: ["a"], anchor: "d" }, order, "b", { range: true, toggle: true });
    expect(s.picked).toEqual(["a", "b", "c", "d"]);
    expect(selectAll(order).picked).toEqual(order);
  });
});

describe("permissions", () => {
  it("mode <-> rwx <-> grid", () => {
    expect(modeString(0o755, "dir")).toBe("drwxr-xr-x");
    expect(modeString(0o640)).toBe("-rw-r-----");
    expect(modeString(0o4755)).toBe("-rwsr-xr-x");
    expect(modeString(0o1777, "dir")).toBe("drwxrwxrwt");
    expect(modeString(0o2644)).toBe("-rw-r-Sr--");
    expect(modeString(null)).toBe("");
    expect(modeBits(0o750)).toEqual([true, true, true, true, false, true, false, false, false]);
    expect(bitsMode(modeBits(0o640))).toBe(0o640);
    expect(bitsMode(modeBits(0o755), 0o4000)).toBe(0o4755);
    expect(octal(0o755)).toBe("755");
    expect(octal(0o4755)).toBe("4755");
    expect(parseOctal("0640")).toBe(0o640);
    expect(parseOctal("755")).toBe(0o755);
    for (const bad of ["", "8", "75", "07555", "rwx"]) expect(parseOctal(bad)).toBeNull();
  });
});

describe("paths", () => {
  it("joins and climbs on a host", () => {
    expect(joinPath("box", "/", "etc")).toBe("/etc");
    expect(joinPath("box", "/home/me/", "a b")).toBe("/home/me/a b");
    expect(parentPath("box", "/home/me")).toBe("/home");
    expect(parentPath("box", "/etc")).toBe("/");
    expect(parentPath("box", "/")).toBe("/");
  });
  it("joins and climbs on Windows, up to the list of drives", () => {
    expect(joinPath(null, "", "C:\\")).toBe("C:\\");
    expect(joinPath(null, "C:\\", "Users")).toBe("C:\\Users");
    expect(joinPath(null, "C:\\Users", "me")).toBe("C:\\Users\\me");
    expect(parentPath(null, "C:\\Users\\me")).toBe("C:\\Users");
    expect(parentPath(null, "C:\\Users")).toBe("C:\\");
    expect(parentPath(null, "C:\\")).toBe("");
  });
  it("joins and climbs on macOS and Linux", () => {
    expect(joinPath(null, "/Users/me", "x")).toBe("/Users/me/x");
    expect(parentPath(null, "/Users")).toBe("/");
  });
});

it("formats sizes", () => {
  expect(formatSize(0)).toBe("0 B");
  expect(formatSize(1536)).toBe("1.5 KB");
  expect(formatSize(5 << 20)).toBe("5.0 MB");
  expect(formatSize(300 * 2 ** 30)).toBe("300 GB");
});

describe("permissions of several items", () => {
  it("start from the bits they share, never their special bits", () => {
    expect(sharedBits([0o2775, 0o755, 0o750])).toBe(0o750);
    expect(sharedBits([0o4755])).toBe(0o755);
    expect(sharedBits([null, 0o644])).toBe(0);
  });
  it("apply only what was changed, over each item's own mode", () => {
    const changed = changedBits(0o750, 0o754); // others: read
    expect(changed).toBe(0o004);
    expect(withChange(0o2770, 0o754, changed)).toBe(0o2774); // a setgid folder keeps setgid
    expect(withChange(0o700, 0o754, changed)).toBe(0o704); // other bits of the item stay
    expect(withChange(0o4755, 0o754, changed)).toBe(0o4755);
  });
});

describe("deleting what is shown", () => {
  it("keeps the selection to the shown items", () => {
    const sel = { picked: ["a", "b", "c", "d"], anchor: "a" };
    expect(pruneSelection(sel, ["b", "d", "x"])).toEqual({ picked: ["b", "d"], anchor: "b" });
    expect(pruneSelection(sel, [])).toEqual({ picked: [], anchor: null });
  });
  it("lists the first names and how many more", () => {
    expect(listNames(["a", "b"])).toEqual({ shown: ["a", "b"], more: 0 });
    expect(listNames(["1", "2", "3", "4", "5", "6", "7"])).toEqual({ shown: ["1", "2", "3", "4", "5"], more: 2 });
  });
});