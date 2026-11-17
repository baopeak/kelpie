import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Project, Worktree } from "./types";

// A small stand-in for the app store: projects, sessions, workspaces, the toast.
const fake = vi.hoisted(() => {
  const state = { projects: [] as any[] };
  const sessions: Record<string, any> = {};
  const workspaces: Record<string, any> = {};
  const toasts: { text: string; action?: { label: string; run: () => void } }[] = [];
  let connected = true;
  let sessionsKnown = true;
  const noteKeys: Record<string, string> = {};
  const store = {
    state,
    sessions,
    connected: () => connected,
    sessionsKnown: () => sessionsKnown,
    workspace: (id: string) => workspaces[id] ?? { tabs: [], activeTab: null, rightPanel: null },
    childrenOf: (id: string) => state.projects.filter((p) => p.parentId === id),
    removeProject: (id: string) => void (state.projects = state.projects.filter((p) => p.id !== id)),
    restoreProjects: (entries: { project: any; index: number }[]) => {
      for (const e of entries) state.projects.splice(e.index, 0, e.project);
    },
    noteKeyFor: async (p: any) => noteKeys[p.id] ?? p.path.replace(/\\/g, "/").toLowerCase(),
    clearNotesAt: vi.fn(async () => {}),
    notify: (text: string, action?: { label: string; run: () => void }) => void toasts.push({ text, action }),
  };
  return {
    store, state, sessions, workspaces, toasts, noteKeys,
    setConnected: (v: boolean) => (connected = v),
    setSessionsKnown: (v: boolean) => (sessionsKnown = v),
  };
});
const invoke = vi.hoisted(() => vi.fn());
vi.mock("./store", () => ({ store: fake.store, metaKey: (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { ancestors, checkRepo, removeFromSidebar, syncWorktrees, worktreeHealth, type SyncProbe } from "./worktree-sync";

const repo: Project = { id: "r", name: "kelpie", path: "D:\\Works\\Kelpie", color: "#000" };
const wt = (id: string, extra: Partial<Project> = {}): Project => ({ id, name: id, path: `D:\\Works\\Kelpie-worktrees\\${id}`, color: "#000", parentId: "r", ...extra });
const entry = (path: string, extra: Partial<Worktree> = {}): Worktree => ({ path, head: "abc", branch: null, bare: false, detached: false, prunable: false, ...extra });

/** A computer where `dirs` exist and git lists `listed` (or fails with `gitError`). */
function probe(opts: { dirs: string[]; listed?: string[]; prunable?: string[]; gitError?: string }): SyncProbe & { calls: string[] } {
  const norm = (p: string) => p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase();
  const dirs = new Set(opts.dirs.map(norm));
  const calls: string[] = [];
  return {
    calls,
    listWorktrees: async (p) => {
      calls.push(`list ${p}`);
      if (opts.gitError) throw new Error(opts.gitError);
      return [entry("D:/Works/Kelpie"), ...(opts.listed ?? []).map((x) => entry(x)), ...(opts.prunable ?? []).map((x) => entry(x, { prunable: true }))];
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

  it("stops at a network share: the server and the current drive's root are not above it", () => {
    expect(ancestors("\\\\server\\share\\wts\\a")).toEqual(["\\\\server\\share\\wts", "\\\\server\\share"]);
    expect(ancestors("\\\\server\\share\\a")).toEqual(["\\\\server\\share"]);
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

  it("keeps a missing worktree git still records without the prunable mark (locked) and marks it missing", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: ROOTS, listed: ["D:/Works/Kelpie-worktrees/a"] }));
    expect(r).toEqual({ remove: [], marks: { a: "missing" } });
  });

  it("removes a missing worktree git lists only as prunable when it has no session", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: ROOTS, prunable: ["D:/Works/Kelpie-worktrees/a"] }));
    expect(r).toEqual({ remove: [wt("a")], marks: {} });
  });

  it("keeps a missing prunable worktree that still has a session and marks it missing", async () => {
    const r = await checkRepo(repo, [wt("a")], (w) => w.id === "a", probe({ dirs: ROOTS, prunable: ["D:/Works/Kelpie-worktrees/a"] }));
    expect(r).toEqual({ remove: [], marks: { a: "missing" } });
  });

  it("leaves a prunable worktree alone when nothing above its folder can be reached", async () => {
    const other = wt("e", { path: "E:\\wt\\e" });
    const r = await checkRepo(repo, [other], noSessions, probe({ dirs: ROOTS, prunable: ["E:/wt/e"] }));
    expect(r).toEqual({ remove: [], marks: {} });
  });

  it("leaves a prunable worktree on an unplugged drive whose mount point's folder above it exists", async () => {
    const vol = wt("v", { path: "/Volumes/Ext/wt/v" });
    const r = await checkRepo(repo, [vol], noSessions, probe({ dirs: [...ROOTS, "/Volumes"], prunable: ["/Volumes/Ext/wt/v"] }));
    expect(r).toEqual({ remove: [], marks: { v: "missing" } });
  });

  it("keeps a prunable worktree marked missing when the folder right above it is gone too", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: ["D:\\", "D:\\Works", "D:\\Works\\Kelpie"], prunable: ["D:/Works/Kelpie-worktrees/a"] }));
    expect(r).toEqual({ remove: [], marks: { a: "missing" } });
  });

  it("marks a prunable entry whose folder still exists (its .git file is gone) as not a worktree", async () => {
    const r = await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: [...ROOTS, "D:\\Works\\Kelpie-worktrees\\a"], prunable: ["D:/Works/Kelpie-worktrees/a"] }));
    expect(r).toEqual({ remove: [], marks: { a: "unlisted" } });
  });

  it("says nothing about a prunable worktree when git fails", async () => {
    expect(await checkRepo(repo, [wt("a")], noSessions, probe({ dirs: ROOTS, prunable: ["D:/Works/Kelpie-worktrees/a"], gitError: "timed out" }))).toBeNull();
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

  it("leaves a worktree on a network share that is down", async () => {
    const r = await checkRepo(repo, [wt("n", { path: "\\\\server\\share\\wt\\n" })], noSessions, probe({ dirs: ROOTS }));
    expect(r).toEqual({ remove: [], marks: {} });
  });

  it("matches git's spelling of a missing folder through the canonical form of the folder above it", async () => {
    // The row was stored with an 8.3 short name; git recorded the long one. The folder is gone, so only
    // the folder above it can be resolved.
    const p = probe({ dirs: [...ROOTS, "D:\\Works\\KELPIE~1"], listed: ["D:/Works/Kelpie-worktrees/a"] });
    p.canonical = async (x) => (x.toLowerCase() === "d:\\works\\kelpie~1" ? "D:\\Works\\Kelpie-worktrees" : x);
    const r = await checkRepo(repo, [wt("a", { path: "D:\\Works\\KELPIE~1\\a" })], noSessions, p);
    expect(r).toEqual({ remove: [], marks: { a: "missing" } });
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
    fake.setSessionsKnown(true);
    for (const k of Object.keys(fake.noteKeys)) delete fake.noteKeys[k];
    fake.store.clearNotesAt.mockClear();
    invoke.mockReset();
  });

  const computer = () => probe({ dirs: [...ROOTS, "D:\\Works\\Kelpie-worktrees\\c"], listed: ["D:/Works/Kelpie-worktrees/c"] });

  it("does nothing before the daemon is connected (sessions not known yet)", async () => {
    fake.setConnected(false);
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
  });

  it("does nothing while the session list could not be read", async () => {
    fake.setSessionsKnown(false);
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
  });

  it("two checks at once remove each row once, with one toast", async () => {
    await Promise.all([syncWorktrees(undefined, computer()), syncWorktrees("r", computer())]);
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "c"]);
    expect(fake.toasts.map((x) => x.text)).toEqual(["Removed 2 worktrees that no longer exist"]);
  });

  it("one toast for the gone worktrees of every repository", async () => {
    const repo2: Project = { id: "r2", name: "other", path: "D:\\Works\\Other", color: "#000" };
    fake.state.projects = [repo, wt("m"), repo2, { ...wt("z"), parentId: "r2", path: "D:\\Works\\Other-worktrees\\z" }];
    const p = probe({ dirs: [...ROOTS, "D:\\Works\\Other"] });
    await syncWorktrees(undefined, p);
    expect(fake.state.projects.map((x) => x.id)).toEqual(["r", "r2"]);
    expect(fake.toasts.map((x) => x.text)).toEqual(["Removed 2 worktrees that no longer exist"]);
    fake.toasts[0].action!.run();
    expect(fake.state.projects.map((x) => x.id)).toEqual(["r", "m", "r2", "z"]);
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

  it("removes a folder deleted by hand (git lists it as prunable) with the same toast and Undo", async () => {
    // Its own row: one brought back with Undo is only marked for the rest of the run.
    fake.state.projects = [repo, wt("p")];
    await syncWorktrees(undefined, probe({ dirs: ROOTS, prunable: ["D:/Works/Kelpie-worktrees/p"] }));
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r"]);
    expect(fake.toasts.map((x) => x.text)).toEqual(["Removed 1 worktree that no longer exists"]);
    fake.toasts[0].action!.run();
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "p"]);
    await new Promise((r) => setTimeout(r, 0));
  });

  it("keeps a worktree whose session runs inside its folder", async () => {
    fake.sessions.s = { id: "s", project_id: "r", cwd: "D:\\Works\\Kelpie-worktrees\\a\\src" };
    fake.workspaces.b = { tabs: [{ id: "t", focus: "p", root: { type: "leaf", id: "l", pane: { id: "p", kind: "chat", sessionId: "s2" } } }], activeTab: "t", rightPanel: null };
    await syncWorktrees(undefined, computer());
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "a", "b", "c"]);
    expect(fake.toasts).toEqual([]);
  });
});

describe("notes of a removed row", () => {
  beforeEach(() => {
    fake.state.projects = [repo, wt("a"), wt("b")];
    for (const k of Object.keys(fake.noteKeys)) delete fake.noteKeys[k];
    fake.store.clearNotesAt.mockClear();
    invoke.mockReset();
  });

  it("Remove from sidebar keeps the notes of a folder that still exists", async () => {
    invoke.mockImplementation(async (cmd: string, a: any) => (cmd === "fs_is_dir" ? true : a?.path));
    await removeFromSidebar(wt("a"));
    expect(fake.state.projects.map((p) => p.id)).toEqual(["r", "b"]);
    expect(fake.store.clearNotesAt).not.toHaveBeenCalled();
  });

  it("clears a gone folder's notes under its own key only, never the repository's above it", async () => {
    invoke.mockImplementation(async (cmd: string, a: any) => (cmd === "fs_is_dir" ? false : a?.path));
    fake.noteKeys.b = "d:/works/kelpie";
    await removeFromSidebar(wt("a"));
    await removeFromSidebar(wt("b"));
    expect(fake.store.clearNotesAt.mock.calls).toEqual([["a", "d:/works/kelpie-worktrees/a"]]);
  });
});
