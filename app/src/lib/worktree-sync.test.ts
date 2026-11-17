import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Project, Worktree } from "./types";

// A small stand-in for the app store: projects, sessions, workspaces, the toast.
const fake = vi.hoisted(() => {
  const state = { projects: [] as any[] };
  const sessions: Record<string, any> = {};
  const workspaces: Record<string, any> = {};
  const toasts: { text: string; action?: { label: string; run: () => void } }[] = [];
  let connected = true;
  const store = {
    state,
    sessions,
    connected: () => connected,
    workspace: (id: string) => workspaces[id] ?? { tabs: [], activeTab: null, rightPanel: null },
    childrenOf: (id: string) => state.projects.filter((p) => p.parentId === id),
    removeProject: (id: string) => void (state.projects = state.projects.filter((p) => p.id !== id)),
    restoreProjects: (entries: { project: any; index: number }[]) => {
      for (const e of entries) state.projects.splice(e.index, 0, e.project);
    },
    noteKeyFor: async (p: any) => p.path,
    clearNotesAt: vi.fn(async () => {}),
    notify: (text: string, action?: { label: string; run: () => void }) => void toasts.push({ text, action }),
  };
  return { store, state, sessions, workspaces, toasts, setConnected: (v: boolean) => (connected = v) };
});
vi.mock("./store", () => ({ store: fake.store }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { ancestors, checkRepo, syncWorktrees, worktreeHealth, type SyncProbe } from "./worktree-sync";

const repo: Project = { id: "r", name: "kelpie", path: "D:\\Works\\Kelpie", color: "#000" };
const wt = (id: string, extra: Partial<Project> = {}): Project => ({ id, name: id, path: `D:\\Works\\Kelpie-worktrees\\${id}`, color: "#000", parentId: "r", ...extra });
const entry = (path: string, extra: Partial<Worktree> = {}): Worktree => ({ path, head: "abc", branch: null, bare: false, detached: false, prunable: false, ...extra });

/** A computer where `dirs` exist and git lists `listed` (or fails with `gitError`). */
function probe(opts: { dirs: string[]; listed?: string[]; gitError?: string }): SyncProbe & { calls: string[] } {
  const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  const dirs = new Set(opts.dirs.map(norm));
  const calls: string[] = [];
  return {
    calls,
    listWorktrees: async (p) => {
      calls.push(`list ${p}`);
      if (opts.gitError) throw new Error(opts.gitError);
      return [entry("D:/Works/Kelpie"), ...(opts.listed ?? []).map((x) => entry(x))];
    },
    isDir: async (p) => dirs.has(norm(p)),
    canonical: async (p) => p,
  };
}
const ROOTS = ["D:\\", "D:\\Works", "D:\\Works\\Kelpie", "D:\\Works\\Kelpie-worktrees"];
const noSessions = () => false;

describe("ancestors", () => {
  it("walks up to the drive root on Windows and stops below / elsewhere", () => {
    expect(ancestors("D:\\Works\\Kelpie-worktrees\\a")).toEqual(["D:\\Works\\Kelpie-worktrees", "D:\\Works", "D:\\"]);
    expect(ancestors("D:/Works/x/")).toEqual(["D:/Works", "D:/"]);
    expect(ancestors("/home/me/wt")).toEqual(["/home/me", "/home"]);
  });
});

describe("checkRepo", () => {
  it("removes a worktree whose folder is gone, that git no longer lists, and that has no session", async () => {
    const r = await checkRepo(repo, [wt("a"), wt("b")], noSessions, probe({ dirs: [...ROOTS, "D:\\Works\\Kelpie-worktrees\\b"], listed: ["D:/Works/Kelpie-worktrees/b"] }));
    expect(r).toEqual({ remove: [wt("a")], marks: {} });
  });

  it("keeps a missing worktree that still has a session and marks it missing", async () => {
    const r = await checkRepo(repo, [wt("a")], (w) => w.id === "a", probe({ dirs: ROOTS }));
    expect(r).toEqual({ remove: [], marks: { a: "missing" } });
  });

  it("keeps a missing worktree git still records (prunable) and marks it missing", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: ROOTS, listed: ["D:/Works/Kelpie-worktrees/a"] }));
    expect(r).toEqual({ remove: [], marks: { a: "missing" } });
  });

  it("keeps a folder that exists but git no longer lists, marked as not a worktree any more", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: [...ROOTS, "D:\\Works\\Kelpie-worktrees\\a"] }));
    expect(r).toEqual({ remove: [], marks: { a: "unlisted" } });
  });

  it("says nothing when git fails", async () => {
    expect(await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: ROOTS, gitError: "fatal: not a git repository" }))).toBeNull();
  });

  it("says nothing when git answers an empty list", async () => {
    const p = probe({ dirs: ROOTS });
    p.listWorktrees = async () => [];
    expect(await checkRepo(repo, [wt("a")], noSessions, p)).toBeNull();
  });

  it("says nothing when the repository's drive cannot be reached", async () => {
    expect(await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: [] }))).toBeNull();
  });

  it("leaves a worktree on another drive that cannot be reached", async () => {
    const other = wt("e", { path: "E:\\wt\\e" });
    const r = await checkRepo(repo, [other], noSessions, probe({ dirs: ROOTS }));
    expect(r).toEqual({ remove: [], marks: {} });
  });

  it("treats a failed folder check as present, never as gone", async () => {
    const p = probe({ dirs: ROOTS });
    const isDir = p.isDir;
    p.isDir = async (path) => (path.endsWith("\\a") ? Promise.reject(new Error("access denied")) : isDir(path));
    expect(await checkRepo(repo, [wt("a")], noSessions, p)).toEqual({ remove: [], marks: { a: "unlisted" } });
  });

  it("skips a remote repository without running git", async () => {
    const p = probe({ dirs: ROOTS });
    const remote = { ...repo, remote: { dest: "box", path: "/srv/kelpie", tmux: false } };
    expect(await checkRepo(remote, [wt("a")], noSessions, p)).toBeNull();
    expect(p.calls).toEqual([]);
  });

  it("matches git's spelling of a path regardless of slashes and case", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: [...ROOTS, "D:\\Works\\Kelpie-worktrees\\a"], listed: ["d:/works/kelpie-worktrees/A"] }));
    expect(r).toEqual({ remove: [], marks: {} });
  });
});

describe("syncWorktrees", () => {
  beforeEach(() => {
    fake.state.projects = [repo, wt("a"), wt("b"), wt("c")];
    for (const k of Object.keys(fake.sessions)) delete fake.sessions[k];
    for (const k of Object.keys(fake.workspaces)) delete fake.workspaces[k];
    fake.toasts.length = 0;
    fake.setConnected(true);
  });

  const computer = () => probe({ dirs: [...ROOTS, "D:\\Works\\Kelpie-worktrees\\c"], listed: ["D:/Works/Kelpie-worktrees/c"] });

  it("does nothing before the daemon is connected (sessions not known yet)", async () => {
    fake.setConnected(false);
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
  });

  it("removes the gone worktrees with one toast, keeps one with a pane, and Undo brings them back", async () => {
    fake.workspaces.b = { tabs: [{ id: "t", focus: "p", root: { type: "leaf", id: "l", pane: { id: "p", kind: "terminal", sessionId: "s1" } } }], activeTab: "t", rightPanel: null };
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "b", "c"]);
    expect(worktreeHealth("b")).toBe("missing");
    expect(worktreeHealth("c")).toBeUndefined();
    expect(fake.toasts.map((x) => x.text)).toEqual(["Removed 1 worktree that no longer exists"]);
    fake.toasts[0].action!.run();
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
    // Undo checks again (with the real probe, unavailable here); let that finish.
    await new Promise((r) => setTimeout(r, 0));
    // Brought back: in this run it is only marked, not removed again.
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
    expect(worktreeHealth("a")).toBe("missing");
  });

  it("keeps a worktree whose session runs inside its folder", async () => {
    fake.sessions.s = { id: "s", project_id: "r", cwd: "D:\\Works\\Kelpie-worktrees\\a\\src" };
    fake.workspaces.b = { tabs: [{ id: "t", focus: "p", root: { type: "leaf", id: "l", pane: { id: "p", kind: "chat", sessionId: "s2" } } }], activeTab: "t", rightPanel: null };
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
    expect(fake.toasts).toEqual([]);
  });
});
