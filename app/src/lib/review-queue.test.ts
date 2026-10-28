import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Project } from "./types";

const project = (id: string, extra: Partial<Project> = {}) => ({ id, name: id, path: `/w/${id}`, ...extra }) as Project;
const projects: Project[] = [project("repo"), project("a", { parentId: "repo" }), project("b", { parentId: "repo" }), project("c", { parentId: "repo" })];
const store = {
  state: { projects },
  sessions: {} as Record<string, { project_id: string; alive: boolean; kind: string }>,
  projectInfo: { repo: { is_repo: true, branch: "main" } } as Record<string, { is_repo: boolean; branch: string | null }>,
  childrenOf: (id: string) => projects.filter((p) => p.parentId === id),
  setProjectStatus: vi.fn(),
  setOverlay: vi.fn(),
};
const reviewChanges = vi.fn();
const confirmDialog = vi.fn();
vi.mock("./store", () => ({ store }));
vi.mock("./ipc", () => ({ reviewChanges }));
vi.mock("./confirm-dialog", () => ({ confirmDialog }));

const q = await import("./review-queue");

const result = (path: string, files: string[]) => ({ path, ok: true, error: null, branch: null, files, truncated: false, commits: files.length, uncommitted: 0, added: 1, deleted: 0, config_changed: false });
const agent = (project_id: string) => ({ project_id, alive: true, kind: "claude" });

beforeEach(() => {
  vi.clearAllMocks();
  store.sessions = {};
  projects.forEach((p) => delete p.status);
});

describe("intersect", () => {
  it("names who else changed each file", () => {
    const out = q.intersect([
      { id: "a", name: "a", files: ["x.ts", "y.ts"] },
      { id: "b", name: "b", files: ["y.ts", "z.ts"] },
      { id: "c", name: "c", files: ["z.ts"] },
    ]);
    expect(out.a).toEqual([{ id: "b", name: "b", files: ["y.ts"] }]);
    expect(out.b).toEqual([{ id: "a", name: "a", files: ["y.ts"] }, { id: "c", name: "c", files: ["z.ts"] }]);
    expect(out.c).toEqual([{ id: "b", name: "b", files: ["z.ts"] }]);
  });
  it("is empty when nothing is shared", () => {
    expect(q.intersect([{ id: "a", name: "a", files: ["1"] }, { id: "b", name: "b", files: ["2"] }])).toEqual({ a: [], b: [] });
  });
});

describe("the overlap watch", () => {
  const repo = projects[0];

  it("scans nothing while fewer than two worktrees have an agent running", async () => {
    await q.scanOverlap(repo);
    store.sessions = { s1: agent("a"), s2: { project_id: "b", alive: false, kind: "claude" }, s3: { project_id: "c", alive: true, kind: "shell" } };
    await q.scanOverlap(repo);
    expect(reviewChanges).not.toHaveBeenCalled();
    expect(q.overlapScans).toBe(0);
  });

  it("compares the active ones and shows the chip on both", async () => {
    store.sessions = { s1: agent("a"), s2: agent("b") };
    reviewChanges.mockResolvedValue([result("/w/a", ["src/x.ts", "a.txt"]), result("/w/b", ["src/x.ts"])]);
    await q.scanOverlap(repo);
    expect(reviewChanges).toHaveBeenCalledTimes(1);
    expect(reviewChanges).toHaveBeenCalledWith([{ path: "/w/a", base: "main", names_only: true }, { path: "/w/b", base: "main", names_only: true }], true);
    expect(q.overlapScans).toBe(1);
    expect(q.overlapsOf("a")).toEqual([{ id: "b", name: "b", files: ["src/x.ts"] }]);
    expect(q.overlapsOf("b")).toEqual([{ id: "a", name: "a", files: ["src/x.ts"] }]);
    expect(q.overlapsOf("c")).toEqual([]);
  });

  it("clears the chips when a worktree's agent goes away", async () => {
    store.sessions = { s2: agent("b") };
    await q.scanOverlap(repo);
    expect(q.overlapsOf("a")).toEqual([]);
    expect(q.overlapsOf("b")).toEqual([]);
  });
});

describe("moving a card to Done", () => {
  const wt = projects[1];

  it("sends it to Review by default (Cancel, Esc) when the worktree has changes nobody merged", async () => {
    reviewChanges.mockResolvedValue([result("/w/a", ["x.ts", "y.ts"])]);
    confirmDialog.mockResolvedValue(false);
    await q.moveToStatus(wt, "done");
    expect(confirmDialog).toHaveBeenCalledTimes(1);
    expect(store.setProjectStatus).toHaveBeenCalledWith("a", "review");
  });

  it("marks it Done only when the user says so", async () => {
    reviewChanges.mockResolvedValue([result("/w/a", ["x.ts"])]);
    confirmDialog.mockResolvedValue(true);
    await q.moveToStatus(wt, "done");
    expect(store.setProjectStatus).toHaveBeenCalledWith("a", "done");
  });

  it("does not ask for a clean worktree, a repository or another column", async () => {
    reviewChanges.mockResolvedValue([{ ...result("/w/a", []), commits: 0 }]);
    await q.moveToStatus(wt, "done");
    await q.moveToStatus(projects[0], "done");
    await q.moveToStatus(wt, "progress");
    expect(confirmDialog).not.toHaveBeenCalled();
    expect(store.setProjectStatus.mock.calls.map((c) => c[1])).toEqual(["done", "done", "progress"]);
  });
});

describe("baseOf", () => {
  it("is the branch the worktree was created from, even when the repository moved on", () => {
    const forked = project("f", { parentId: "repo", baseBranch: "release" });
    expect(q.baseOf(forked)).toBe("release");
    expect(q.currentBranchOf(forked)).toBe("main");
  });

  it("is the repository's current branch for a worktree, and null off a repository or on a detached HEAD", () => {
    expect(q.baseOf(projects[1])).toBe("main");
    expect(q.baseOf(projects[0])).toBe("main");
    store.projectInfo.repo = { is_repo: true, branch: null };
    expect(q.baseOf(projects[1])).toBeNull();
    store.projectInfo.repo = { is_repo: true, branch: "main" };
  });
});
