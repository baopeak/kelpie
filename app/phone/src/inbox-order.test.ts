import { describe, expect, it } from "vitest";
import { groupOf, groupSessions, labelOf, needsCount, whereOf } from "./inbox-order";
import type { SessionRow } from "./link";

const row = (over: Partial<SessionRow>): SessionRow => ({
  id: "s", name: null, kind: "shell", title: null, status: "idle", unread: false, alive: true, chat: false, project_id: null, cwd: "/work/app", last_activity: 0, waiting_since: null, ...over,
});

describe("inbox", () => {
  it("groups by what the session asks of the person, in the order needs / working / done / idle / stopped", () => {
    const rows = [
      row({ id: "idle" }),
      row({ id: "stopped", alive: false, status: "waiting" }),
      row({ id: "done", unread: true }),
      row({ id: "working", status: "working" }),
      row({ id: "needs", status: "waiting" }),
    ];
    expect(groupSessions(rows).map((g) => [g.key, g.rows.map((r) => r.id)])).toEqual([
      ["needs", ["needs"]], ["working", ["working"]], ["done", ["done"]], ["idle", ["idle"]], ["stopped", ["stopped"]],
    ]);
  });

  it("puts the longest wait first, and sessions that do not know their wait last", () => {
    const rows = [
      row({ id: "new", status: "waiting", waiting_since: 900 }),
      row({ id: "unknown", status: "waiting", waiting_since: null }),
      row({ id: "old", status: "waiting", waiting_since: 100 }),
    ];
    expect(groupSessions(rows)[0].rows.map((r) => r.id)).toEqual(["old", "new", "unknown"]);
  });

  it("orders the other groups by recent activity and counts only those that need you", () => {
    const rows = [row({ id: "a", last_activity: 1 }), row({ id: "b", last_activity: 5 }), row({ id: "c", status: "waiting" }), row({ id: "d", status: "waiting", alive: false })];
    expect(groupSessions(rows).find((g) => g.key === "idle")!.rows.map((r) => r.id)).toEqual(["b", "a"]);
    expect(needsCount(rows)).toBe(1);
    expect(groupOf(row({ chat: true, status: "waiting" }))).toBe("needs");
  });

  it("names a session by its name, a short title, or its kind, and finds its project", () => {
    expect(labelOf(row({ name: "build" }))).toBe("build");
    expect(labelOf(row({ title: "✳ Fix the login bug" }))).toBe("Fix the login bug");
    expect(labelOf(row({ title: "x".repeat(60), kind: "claude" }))).toBe("claude");
    expect(whereOf(row({ project_id: "p1" }), { p1: "Kelpie" })).toBe("Kelpie");
    expect(whereOf(row({ cwd: "C:\\Users\\me\\proj" }), {})).toBe("proj");
  });
});
