import { beforeEach, describe, expect, it, vi } from "vitest";

const materialize = vi.fn(async () => ({ copied: [".env"], linked: ["deps"], skipped: [] }));
const worktreeAdd = vi.fn(async (..._args: unknown[]) => {});

vi.mock("./store", () => ({
  store: {
    state: { projects: [{ id: "repo", path: "C:/w/repo", name: "repo" }, { id: "wt", path: "C:/w/repo-worktrees/a", name: "a", parentId: "repo" }], approvedSetups: {} },
    projectInfo: {},
    sessions: {},
    notify: vi.fn(),
    addInbox: vi.fn(),
    addProject: () => "wt",
    newSession: vi.fn(),
  },
}));
vi.mock("./ipc", () => ({ gitRefs: async () => [], gitWorktreeAdd: worktreeAdd, worktreeMaterialize: materialize }));
vi.mock("./terminal-host", () => ({ disposeView: vi.fn() }));
vi.mock("./runs", () => ({ markWorktreeRemoved: vi.fn() }));

const { createWorktree, parseKelpieConfig } = await import("./worktrees");
const repo = { id: "repo", path: "C:/w/repo", name: "repo" } as never;

beforeEach(() => (materialize.mockClear(), worktreeAdd.mockClear()));

describe("createWorktree brings ignored files only for an approved .kelpie.yaml", () => {
  it("brings the approved copies and links", async () => {
    await createWorktree(repo, { branch: "feat", path: "C:/w/repo-worktrees/a", config: { ...parseKelpieConfig("copy: [.env]\nlink: [deps]\n") } });
    expect(materialize).toHaveBeenCalledWith("C:/w/repo", "C:/w/repo-worktrees/a", [".env"], ["deps"]);
  });

  it("leaves a clean checkout, .worktreeinclude included, when the setup is not approved", async () => {
    await createWorktree(repo, { branch: "feat", path: "C:/w/repo-worktrees/a", config: null });
    expect(materialize).not.toHaveBeenCalled();
  });
});

describe("createWorktree for an existing branch", () => {
  it("checks the branch out as it is: no start point, no new branch", async () => {
    await createWorktree(repo, { branch: "feature", base: "main", path: "C:/w/repo-worktrees/feature", config: null, existing: true });
    expect(worktreeAdd).toHaveBeenCalledWith("C:/w/repo", "C:/w/repo-worktrees/feature", "feature", undefined, true);
  });

  it("leaves names git accepts to the native check, while a new branch keeps the strict name rule", async () => {
    await createWorktree(repo, { branch: "über", path: "C:/w/repo-worktrees/uber", config: null, existing: true });
    expect(worktreeAdd).toHaveBeenCalledTimes(1);
    await expect(createWorktree(repo, { branch: "über", path: "C:/w/repo-worktrees/uber", config: null })).rejects.toThrow();
  });

  it("a new branch still passes its start point and no existing flag", async () => {
    await createWorktree(repo, { branch: "feat", base: "main", path: "C:/w/repo-worktrees/a", config: null });
    expect(worktreeAdd).toHaveBeenCalledWith("C:/w/repo", "C:/w/repo-worktrees/a", "feat", "main", undefined);
  });
});
