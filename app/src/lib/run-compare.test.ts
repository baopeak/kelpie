import { describe, expect, it, vi } from "vitest";

vi.mock("./store", () => ({ store: { state: { projects: [] }, sessions: {}, notify: vi.fn() } }));
vi.mock("./ipc", () => ({}));
vi.mock("./review-queue", () => ({}));
vi.mock("./finish", () => ({}));
vi.mock("./review-facts", () => ({}));
vi.mock("./worktrees", () => ({}));
vi.mock("./confirm-dialog", () => ({}));
vi.mock("./usage-format", () => ({ formatCost: (n: number) => `$${n.toFixed(2)}` }));
vi.mock("./runs", () => ({ finishedTask: (s: string) => ["done", "failed", "skipped", "cancelled"].includes(s), taskAction: vi.fn() }));

const { bestOf, buildCandidates, candidateTasks, changedFiles, discardsFor, durationOf, formatDuration } = await import("./run-compare");
import type { Facts } from "./run-compare";
import type { Run, RunTask } from "./runs";

const task = (id: string, over: Partial<RunTask> = {}): RunTask => ({
  id, title: id, prompt: "p", kind: "claude", args: [], env: {}, deps: [], approve: false, worktree: true, status: "done", approved: false,
  session_id: `s-${id}`, cwd: `C:/w/${id}`, branch: `kelpie/${id}`, started_at: 1000, finished_at: 61_000, summary: null, error: null, worked: true, ...over,
});
const run = (tasks: RunTask[]): Run => ({ id: "r1", name: "Try", objective: "", cwd: "C:/w", project_id: "p", tasks, status: "running", created_at: 0, finished_at: 0, idle_done_secs: 300 });
const changes = (files: string[], added: number, deleted: number, uncommitted = 0) =>
  ({ path: "", ok: true, error: null, branch: "b", files, truncated: false, commits: 1, uncommitted, added, deleted, config_changed: false });
const facts = (id: string, over: Partial<Facts> = {}): Facts => ({ projectId: `proj-${id}`, changes: changes(["a.ts"], 5, 1), cost: null, turns: null, checks: null, ...over });

describe("durationOf", () => {
  it("is the time from start to end, or so far while it runs, and none before it started", () => {
    expect(durationOf(task("a"), 999_999)).toBe(60_000);
    expect(durationOf(task("a", { status: "running", finished_at: 0 }), 31_000)).toBe(30_000);
    expect(durationOf(task("a", { status: "pending", started_at: 0, finished_at: 0 }), 31_000)).toBeNull();
    expect(durationOf(task("a", { status: "skipped", finished_at: 0 }), 31_000)).toBe(0);
  });
});

describe("formatDuration", () => {
  it("reads as seconds, minutes or hours", () => {
    expect([null, 4_000, 61_000, 3_723_000].map(formatDuration)).toEqual(["–", "4s", "1m 01s", "1h 02m"]);
  });
});

describe("candidates", () => {
  it("are the tasks that work in a worktree of their own", () => {
    const r = run([task("a"), task("b", { worktree: false }), task("c", { cwd: null }), task("d")]);
    expect(candidateTasks(r).map((x) => x.id)).toEqual(["a", "d"]);
  });

  it("carry what was read about their worktree", () => {
    const r = run([task("a"), task("b", { status: "running", finished_at: 0, summary: null, error: null })]);
    const rows = buildCandidates(r, { a: facts("a", { cost: 0.5, turns: 4, changes: changes(["x.ts", "y.ts"], 30, 2, 1) }) }, 101_000);
    expect(rows[0]).toMatchObject({ id: "a", status: "done", durationMs: 60_000, running: false, files: ["x.ts", "y.ts"], added: 30, deleted: 2, uncommitted: 1, cost: 0.5, turns: 4, projectId: "proj-a", sessionId: "s-a", pickable: true });
    expect(rows[1]).toMatchObject({ id: "b", running: true, durationMs: 100_000, files: null, projectId: null, pickable: false });
  });

  it("cannot be picked when their changes could not be read", () => {
    const bad = { ...facts("a"), changes: { ...changes([], 0, 0), ok: false, error: "boom" } };
    expect(buildCandidates(run([task("a")]), { a: bad }, 0)[0]).toMatchObject({ files: null, pickable: false });
  });

  it("show a failed task's error where the result goes", () => {
    const rows = buildCandidates(run([task("a", { status: "failed", error: "the agent exited with code 2" })]), {}, 0);
    expect(rows[0].summary).toBe("the agent exited with code 2");
  });
});

describe("bestOf", () => {
  const rows = (...f: [string, Partial<Facts>, Partial<RunTask>?][]) =>
    buildCandidates(run(f.map(([id, , t]) => task(id, t))), Object.fromEntries(f.map(([id, over]) => [id, facts(id, over)])), 0);

  it("names the fastest, cheapest and smallest of the finished candidates", () => {
    const best = bestOf(rows(
      ["a", { cost: 0.9, changes: changes(["a"], 50, 10) }, { finished_at: 31_000 }],
      ["b", { cost: 0.4, changes: changes(["a"], 5, 1) }],
      ["c", { cost: 1.5, changes: changes(["a"], 20, 20) }, { finished_at: 91_000 }],
    ));
    expect(best).toEqual({ fastest: "a", cheapest: "b", smallest: "b" });
  });

  it("ignores candidates that did not finish, and names nobody on a tie or with one entrant", () => {
    const tie = bestOf(rows(["a", { cost: 1 }], ["b", { cost: 1 }]));
    expect(tie.cheapest).toBeUndefined();
    expect(tie.fastest).toBeUndefined();
    const lone = bestOf(rows(["a", { cost: 1 }], ["b", { cost: 0.1 }, { status: "running", finished_at: 0 }]));
    expect(lone).toEqual({});
  });
});

describe("changedFiles", () => {
  it("lists every file with who changed it, shared files first", () => {
    const rows = buildCandidates(run([task("a"), task("b")]), { a: facts("a", { changes: changes(["z.ts", "shared.ts"], 1, 0) }), b: facts("b", { changes: changes(["shared.ts", "b.ts"], 1, 0) }) }, 0);
    expect(changedFiles(rows)).toEqual([{ path: "shared.ts", by: ["a", "b"] }, { path: "b.ts", by: ["b"] }, { path: "z.ts", by: ["a"] }]);
  });
});

describe("discardsFor", () => {
  it("is every other candidate that still has a worktree", () => {
    const rows = buildCandidates(run([task("a"), task("b"), task("c")]), { a: facts("a"), b: facts("b"), c: { ...facts("c"), projectId: null } }, 0);
    expect(discardsFor(rows, "a").map((r) => r.id)).toEqual(["b"]);
    expect(discardsFor(rows, "c").map((r) => r.id)).toEqual(["a", "b"]);
  });
});
