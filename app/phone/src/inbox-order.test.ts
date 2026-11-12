import { describe, expect, it } from "vitest";
import { groupByWorkspace, groupOf, groupSessions, labelOf, needsCount, shownRows, whereOf, workspaceOf } from "./inbox-order";
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

  it("orders by name A–Z in the phone's language, while needs-you keeps the longest wait first", () => {
    const rows = [
      row({ id: "zeta", name: "Zeta", last_activity: 9 }),
      row({ id: "alpha", name: "alpha", last_activity: 1 }),
      row({ id: "build10", name: "build 10" }),
      row({ id: "build2", name: "build 2" }),
      row({ id: "dang", name: "Đăng" }),
      row({ id: "late", name: "A late wait", status: "waiting", waiting_since: 500 }),
      row({ id: "early", name: "Z early wait", status: "waiting", waiting_since: 100 }),
    ];
    const groups = groupSessions(rows, "name", "vi-VN");
    expect(groups.find((g) => g.key === "needs")!.rows.map((r) => r.id)).toEqual(["early", "late"]);
    // In Vietnamese "Đ" comes after "D" and before "E"; numbers count as numbers.
    expect(groups.find((g) => g.key === "idle")!.rows.map((r) => r.id)).toEqual(["alpha", "build2", "build10", "dang", "zeta"]);
    expect(groupSessions(rows, "recent").find((g) => g.key === "idle")!.rows[0].id).toBe("zeta");
  });

  it("leaves stopped sessions out when asked, and says how many", () => {
    const rows = [row({ id: "a" }), row({ id: "b", alive: false }), row({ id: "c", alive: false, status: "waiting" })];
    expect(shownRows(rows, true)).toEqual({ rows, hidden: 0 });
    const off = shownRows(rows, false);
    expect(off.rows.map((r) => r.id)).toEqual(["a"]);
    expect(off.hidden).toBe(2);
    expect(needsCount(off.rows)).toBe(0);
  });
});

describe("the inbox by workspace", () => {
  const projects = { p1: "Kelpie", p2: "Website" };

  it("lists a session under its project, else the folder it runs in, else Other folders", () => {
    expect(workspaceOf(row({ project_id: "p1" }), projects)).toEqual({ key: "p1", name: "Kelpie" });
    expect(workspaceOf(row({ project_id: "gone", cwd: "C:\\code\\tools\\" }), projects)).toEqual({ key: "folder:tools", name: "tools" });
    expect(workspaceOf(row({ project_id: null, cwd: "" }), projects)).toEqual({ key: "other", name: null });
  });

  it("puts sections with a session that needs you first, then the most recently active", () => {
    const rows = [
      row({ id: "web", project_id: "p2", last_activity: 50 }),
      row({ id: "kelpie", project_id: "p1", last_activity: 90 }),
      row({ id: "loose", cwd: "/tmp/scratch", last_activity: 10, status: "waiting", waiting_since: 5 }),
      row({ id: "nowhere", cwd: "", last_activity: 70 }),
    ];
    expect(groupByWorkspace(rows, projects).map((w) => [w.key, w.name])).toEqual([
      ["folder:scratch", "scratch"], ["p1", "Kelpie"], ["other", null], ["p2", "Website"],
    ]);
  });

  it("orders a section's sessions: needs you, working, done, idle, stopped, each in the chosen order", () => {
    const rows = [
      row({ id: "stopped", project_id: "p1", alive: false, last_activity: 99 }),
      row({ id: "idle-b", name: "b", project_id: "p1", last_activity: 5 }),
      row({ id: "idle-a", name: "a", project_id: "p1", last_activity: 1 }),
      row({ id: "done", project_id: "p1", unread: true }),
      row({ id: "working", project_id: "p1", status: "working" }),
      row({ id: "needs", project_id: "p1", status: "waiting" }),
    ];
    expect(groupByWorkspace(rows, projects)[0].rows.map((r) => r.id)).toEqual(["needs", "working", "done", "idle-b", "idle-a", "stopped"]);
    expect(groupByWorkspace(rows, projects, "name", "en-US")[0].rows.map((r) => r.id)).toEqual(["needs", "working", "done", "idle-a", "idle-b", "stopped"]);
  });

  it("drops a section whose sessions are all hidden", () => {
    const rows = [row({ id: "live", project_id: "p1" }), row({ id: "ended", project_id: "p2", alive: false })];
    expect(groupByWorkspace(shownRows(rows, false).rows, projects).map((w) => w.key)).toEqual(["p1"]);
  });
});
