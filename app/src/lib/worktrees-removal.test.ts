import { beforeEach, describe, expect, it, vi } from "vitest";

const order: string[] = [];
const wt = { id: "wt", name: "x", path: "C:/w/repo/.claude/worktrees/x", parentId: "repo" };
let removeFails = false;

vi.mock("./store", () => ({
  store: {
    state: { projects: [{ id: "repo", path: "C:/w/repo" }, wt], activeProject: "repo" },
    sessions: {},
    notify: vi.fn(),
    switchProject: vi.fn(),
    noteKeyFor: async () => (order.push("key"), "c:/w/repo/.claude/worktrees/x"),
    clearNotesAt: async (id: string, key: string) => void order.push(`notes ${id} ${key}`),
    removeProject: (id: string) => void order.push(`project ${id}`),
  },
}));
vi.mock("./ipc", () => ({
  unwatchRoot: async () => void order.push("unwatch"),
  gitWorktreeRemove: async (_root: string, _path: string, force: boolean) => {
    order.push(`git remove force=${force}`);
    if (removeFails) throw new Error("the folder is in use");
  },
  gitWorktreePrune: async () => {},
  isDir: async () => false,
  processesAlive: async () => [],
  daemon: async () => [],
}));
vi.mock("./terminal-host", () => ({ disposeView: vi.fn() }));
vi.mock("./runs", () => ({ markWorktreeRemoved: (p: string) => void order.push(`forget ${p}`) }));

const { removeWorktree } = await import("./worktrees");

beforeEach(() => {
  order.length = 0;
  removeFails = false;
});

describe("removeWorktree and its review notes", () => {
  it("asks where the notes are filed while the folder exists and clears exactly that key once it is gone", async () => {
    await removeWorktree(wt as never, { force: false });
    const at = (s: string) => order.indexOf(s);
    expect(at("key")).toBeGreaterThanOrEqual(0);
    expect(at("key")).toBeLessThan(at("git remove force=false"));
    expect(at("git remove force=false")).toBeLessThan(at("notes wt c:/w/repo/.claude/worktrees/x"));
    expect(at("notes wt c:/w/repo/.claude/worktrees/x")).toBeLessThan(at("project wt"));
    expect(order).toContain("forget C:/w/repo/.claude/worktrees/x");
  });

  it("keeps the notes (and the project) when the removal fails", async () => {
    removeFails = true;
    await expect(removeWorktree(wt as never, { force: false })).rejects.toThrow(/in use/);
    expect(order.some((o) => o.startsWith("notes"))).toBe(false);
    expect(order).not.toContain("project wt");
  }, 15000);
});
