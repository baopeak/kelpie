// Removing the candidates that were not picked: only if they still hold what was recorded when the user
// confirmed, never forced, and the branch only when it is merged.
import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
const projects = [
  { id: "repo", path: "C:/w/repo", name: "repo" },
  { id: "wt", path: "C:/w/repo-worktrees/a", name: "a", parentId: "repo", baseBranch: "main" },
];
let head = "c1";
let uncommitted: { path: string; status: string; size: number }[] = [];
let branchDelete: "ok" | "unmerged" = "unmerged";
let removal: "ok" | "dirty" = "ok";
let wip: { extra: string[] } = { extra: [] };

vi.mock("./store", () => ({ store: { state: { projects }, sessions: {}, notify: vi.fn(), addInbox: vi.fn(), removeNotes: vi.fn() } }));
vi.mock("./ipc", () => ({
  gitHead: async () => head,
  finishInspect: async () => ({ branch: "kelpie/a", uncommitted }),
  finishWipCommit: async (_wt: string, branch: string, message: string, paths: string[]) => (calls.push(`wip ${branch} ${message} ${paths.join(",")}`), { commit: "w1", ...wip }),
  gitBranchDelete: async (_root: string, branch: string, force: boolean) => {
    calls.push(`branch-d ${branch} force=${force}`);
    if (branchDelete === "unmerged") throw new Error("not fully merged");
  },
  daemon: async (method: string) => void calls.push(method),
}));
vi.mock("./review-queue", () => ({ baseOf: () => "main", changesOf: () => undefined, openFinish: vi.fn(), refreshChanges: vi.fn() }));
vi.mock("./finish", () => ({}));
vi.mock("./review-facts", () => ({}));
vi.mock("./worktrees", () => ({
  removeWorktree: async (_wt: unknown, opts: { force: boolean }) => {
    calls.push(`remove force=${opts.force}`);
    if (removal === "dirty") throw new Error("contains modified or untracked files");
    return { leftover: false };
  },
  stopSessions: async () => true,
}));
vi.mock("./confirm-dialog", () => ({}));
vi.mock("./usage-format", () => ({ formatCost: () => "" }));
vi.mock("./agents", () => ({ kindInfo: () => ({ label: "Codex" }) }));
vi.mock("./runs", () => ({ finishedTask: (s: string) => s === "done", taskAction: vi.fn() }));

const { discardSafely, sameSnapshot, snapshotOf } = await import("./run-compare");
import type { Candidate } from "./run-compare";

const cand = { id: "a", title: "Codex", kind: "codex", branch: "kelpie/a", projectId: "wt" } as Candidate;
const snap = (over: object = {}) => ({ head: "c1", branch: "kelpie/a", files: [], paths: [], ...over });

beforeEach(() => {
  calls.length = 0;
  head = "c1";
  uncommitted = [];
  branchDelete = "unmerged";
  removal = "ok";
  wip = { extra: [] };
});

describe("snapshots", () => {
  it("are equal only for the same commit, branch and uncommitted files", async () => {
    uncommitted = [{ path: "b.txt", status: "M", size: 3 }, { path: "a.txt", status: "?", size: 1 }];
    const s = await snapshotOf(projects[1] as never, projects[0] as never, "main");
    expect(s).toMatchObject({ head: "c1", branch: "kelpie/a", paths: ["b.txt", "a.txt"] });
    expect(s.files).toEqual(["? a.txt 1", "M b.txt 3"]);
    expect(sameSnapshot(s, { ...s })).toBe(true);
    expect(sameSnapshot(s, { ...s, head: "c2" })).toBe(false);
    expect(sameSnapshot(s, { ...s, files: ["? a.txt 1", "M b.txt 4"] })).toBe(false);
    expect(sameSnapshot(s, { ...s, branch: "other" })).toBe(false);
  });
});

describe("discarding a candidate that was stopped", () => {
  it("removes the worktree without force and keeps a branch that is not merged", async () => {
    const fate = await discardSafely(cand, snap());
    expect(fate).toMatchObject({ kept: null, branchDeleted: false, saved: 0 });
    expect(calls).toEqual(["checkpoint.drop", "remove force=false", "branch-d kelpie/a force=false"]);
  });

  it("deletes the branch only when git says it is merged", async () => {
    branchDelete = "ok";
    expect((await discardSafely(cand, snap())).branchDeleted).toBe(true);
  });

  it("saves the uncommitted files in a commit first, so removal needs no force", async () => {
    uncommitted = [{ path: "a.txt", status: "?", size: 1 }];
    const fate = await discardSafely(cand, snap({ files: ["? a.txt 1"], paths: ["a.txt"] }));
    expect(fate.saved).toBe(1);
    expect(calls[0]).toBe("wip kelpie/a WIP from Codex a.txt");
    expect(calls).toContain("remove force=false");
  });

  it("is left alone when it committed after the confirmation", async () => {
    head = "c2";
    const fate = await discardSafely(cand, snap());
    expect(fate.kept).toMatch(/changed after you confirmed/);
    expect(calls).toEqual([]);
  });

  it("is left alone when a file changed after the confirmation, or one more appeared", async () => {
    uncommitted = [{ path: "a.txt", status: "?", size: 99 }];
    expect((await discardSafely(cand, snap({ files: ["? a.txt 1"], paths: ["a.txt"] }))).kept).toMatch(/changed/);
    expect((await discardSafely(cand, snap())).kept).toMatch(/changed/);
    expect(calls).toEqual([]);
  });

  it("is left alone when its agent could not be stopped (no record)", async () => {
    expect((await discardSafely(cand, null)).kept).toMatch(/could not be stopped/);
    expect(calls).toEqual([]);
  });

  it("is left alone when files appeared while their commit was made", async () => {
    uncommitted = [{ path: "a.txt", status: "?", size: 1 }];
    wip = { extra: ["late.txt"] };
    const fate = await discardSafely(cand, snap({ files: ["? a.txt 1"], paths: ["a.txt"] }));
    expect(fate.kept).toMatch(/more files appeared/);
    expect(calls.some((c) => c.startsWith("remove"))).toBe(false);
  });

  it("is kept, with the reason, when git refuses to remove it; its branch is not touched", async () => {
    removal = "dirty";
    const fate = await discardSafely(cand, snap());
    expect(fate.kept).toMatch(/modified or untracked/);
    expect(calls.some((c) => c.startsWith("branch-d"))).toBe(false);
  });

  it("never asks for a forced removal or a forced branch delete", async () => {
    branchDelete = "ok";
    uncommitted = [{ path: "a.txt", status: "?", size: 1 }];
    await discardSafely(cand, snap({ files: ["? a.txt 1"], paths: ["a.txt"] }));
    expect(calls.filter((c) => c.includes("force=true"))).toEqual([]);
  });
});
