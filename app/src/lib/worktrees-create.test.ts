import { beforeEach, describe, expect, it, vi } from "vitest";

const materialize = vi.fn(async () => ({ copied: [".env"], linked: ["deps"], skipped: [] }));

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
vi.mock("./ipc", () => ({ gitRefs: async () => [], gitWorktreeAdd: async () => {}, worktreeMaterialize: materialize }));
vi.mock("./terminal-host", () => ({ disposeView: vi.fn() }));
vi.mock("./runs", () => ({ markWorktreeRemoved: vi.fn() }));

const { createWorktree, parseKelpieConfig } = await import("./worktrees");
const repo = { id: "repo", path: "C:/w/repo", name: "repo" } as never;

beforeEach(() => materialize.mockClear());

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
