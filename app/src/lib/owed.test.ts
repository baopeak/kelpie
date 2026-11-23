import { describe, expect, it } from "vitest";
import { applyOwedChanged, kindLabel, openEntries, openOwed, owedRows, replaceOwed, toOwed, type Owed } from "./owed";

const entry = (over: Partial<Owed>): Owed => ({ id: "a", project: "p1", kind: "phone", what: "w", closes_by: "c", opened_by: "user", opened_ms: 1000, ...over });
const name = (id: string) => (id === "s1" ? "Claude · login" : undefined);

describe("owedRows", () => {
  it("lists the open entries oldest first and leaves the closed ones out", () => {
    const rows = owedRows(
      [entry({ id: "late", opened_ms: 5000 }), entry({ id: "done", opened_ms: 100, closed_ms: 900 }), entry({ id: "early", opened_ms: 2000 }), entry({ id: "same-b", opened_ms: 3000 }), entry({ id: "same-a", opened_ms: 3000 })],
      6000,
      name,
    );
    expect(rows.map((r) => r.id)).toEqual(["early", "same-a", "same-b", "late"]);
    expect(rows[0].ageMs).toBe(4000);
  });

  it("says who opened an entry and never shows a negative age", () => {
    const rows = owedRows([entry({ id: "u", opened_by: "user", opened_ms: 10_000 }), entry({ id: "s", opened_by: "session:s1", opened_ms: 11_000 }), entry({ id: "gone", opened_by: "session:zz", opened_ms: 12_000 })], 9000, name);
    expect(rows.map((r) => r.by)).toEqual(["you", "Claude · login", "an agent"]);
    expect(rows.every((r) => r.ageMs === 0)).toBe(true);
  });

  it("labels each kind briefly", () => {
    expect(["real_app", "phone", "clean_machine", "soak", "owner"].map(kindLabel)).toEqual(["Real app", "Phone", "Clean machine", "Over time", "Owner's check"]);
    expect(kindLabel("later")).toBe("later");
  });
});

describe("the window's copy", () => {
  const raw = (id: string, project: string, extra: object = {}) => ({ id, project, kind: "soak", what: "w", closes_by: "c", opened_by: "user", opened_ms: 1, ...extra });

  it("keeps only entries that parse and are open", () => {
    expect(toOwed({ id: "x" })).toBeNull();
    expect(toOwed(null)).toBeNull();
    expect(toOwed(raw("a", "p"))?.kind).toBe("soak");
    expect(openEntries([raw("a", "p"), raw("b", "p", { closed_ms: 5 }), { nope: 1 }]).map((e) => e.id)).toEqual(["a"]);
    expect(openEntries("not a list")).toEqual([]);
  });

  it("replaces everything on a read and one project on an event", () => {
    replaceOwed([raw("a", "p1"), raw("b", "p2")]);
    expect(Object.keys(openOwed).sort()).toEqual(["p1", "p2"]);
    applyOwedChanged("p1", [raw("a", "p1", { closed_ms: 9 }), raw("c", "p1")]);
    expect(openOwed.p1.map((e) => e.id)).toEqual(["c"]);
    expect(openOwed.p2.map((e) => e.id)).toEqual(["b"]);
    applyOwedChanged("p2", [raw("b", "p2", { closed_ms: 9 })]);
    expect(openOwed.p2).toBeUndefined();
    replaceOwed([]);
    expect(Object.keys(openOwed)).toEqual([]);
  });
});
