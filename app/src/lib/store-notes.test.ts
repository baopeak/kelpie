// The review notes in the real store: kept in the app's state for a daemon older than API 13,
// moved to the daemon (after a backup of the state) by a newer one, then read from it.
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { ReviewNote } from "./types";

vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), requestPermission: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("./ipc");
vi.mock("./terminal-host", () => ({ disposeView: vi.fn(), peekView: vi.fn(), resetStreams: vi.fn(), setInputFanout: vi.fn() }));
vi.mock("./browser", () => ({ closeBrowser: vi.fn() }));

const note = (id: string, line: number): ReviewNote => ({ id, path: "src/a.ts", line, side: "new", text: "code", note: `note ${id}`, at: 5 });
const project = (id: string, path: string) => ({ id, name: id, path, color: "#fff" });

let daemonApi = 13;
const calls: { method: string; params: any }[] = [];
const handlers: Record<string, (p: any) => void> = {};
let saved: any;

async function boot(backup: string | null = "C:/data/app-state.notes-1.bak.json") {
  vi.resetModules();
  (globalThis as any).window = globalThis;
  const ipc = await import("./ipc");
  const m = vi.mocked(ipc);
  m.loadState.mockResolvedValue(saved);
  m.saveState.mockResolvedValue(undefined);
  m.stateBackup.mockResolvedValue(backup);
  m.listSessions.mockResolvedValue([]);
  m.platformInfo.mockResolvedValue({ api: 13, version: "0.1.5", security: 1, os: "windows", home: "", data_dir: "", e2e: false } as any);
  m.canonicalPath.mockImplementation(async (p: string) => p);
  m.gitBranchInfo.mockResolvedValue({ is_repo: true, branch: "main", dirty: false, ahead: 0, behind: 0 } as any);
  m.onEvent.mockImplementation(async (name: string, cb: any) => {
    handlers[name] = cb;
    return () => {};
  });
  m.daemon.mockImplementation((async (method: string, params: any) => {
    calls.push({ method, params });
    if (method === "daemon.info") return { api: daemonApi, version: "0.1.5", security: 1, exe: "C:/k/kelpied.exe" };
    if (method === "note.list") return {};
    if (method === "note.keys") return Object.fromEntries((params.paths as string[]).map((x) => [x, x.toLowerCase()]));
    if (method === "note.add") return { path: String(params.path).toLowerCase(), notes: [{ ...note(params.id ?? "new", params.line), by: undefined }] };
    if (method === "note.clear") return { path: String(params.path).toLowerCase(), notes: [] };
    return [];
  }) as any);
  const { store } = await import("./store");
  await store.init();
  return store;
}

beforeEach(() => {
  calls.length = 0;
  daemonApi = 13;
  saved = {
    version: 3, projects: [project("p1", "C:/w/a"), project("p2", "C:/w/b"), project("nested", "C:/w/a/.claude/worktrees/x")], activeProject: "p1", workspaces: {}, settings: {}, recent: [], collapsed: {}, keymap: {}, approvedSetups: {},
    hibernated: [], inbox: [], floating: { sessions: [], active: null, open: false },
    reviewNotes: { p1: [note("n1", 3), note("n2", 8)], p2: [note("n3", 1)], gone: [note("n9", 1)] },
  };
});

const adds = () => calls.filter((c) => c.method === "note.add");

describe("review notes on a current daemon", () => {
  it("moves the notes in the app's state to the daemon, keeping their ids, after a backup", async () => {
    const store = await boot();
    await vi.waitFor(() => expect(adds()).toHaveLength(3));
    expect(adds().map((c) => [c.params.path, c.params.id, c.params.file, c.params.line, c.params.side, c.params.text, c.params.note, c.params.at])).toEqual([
      ["C:/w/a", "n1", "src/a.ts", 3, "new", "code", "note n1", 5],
      ["C:/w/a", "n2", "src/a.ts", 8, "new", "code", "note n2", 5],
      ["C:/w/b", "n3", "src/a.ts", 1, "new", "code", "note n3", 5],
    ]);
    const ipc = await import("./ipc");
    expect(vi.mocked(ipc).stateBackup).toHaveBeenCalledWith("notes");
    await vi.waitFor(() => expect(Object.keys(store.state.reviewNotes)).toEqual(["gone"]));
  });

  it("keeps the notes in the app, and says so, when the state could not be backed up", async () => {
    const store = await boot(null);
    await vi.waitFor(() => expect(store.toast()?.text).toMatch(/could not be backed up/));
    expect(adds()).toEqual([]);
    expect(Object.keys(store.state.reviewNotes).sort()).toEqual(["gone", "p1", "p2"]);
    expect(store.notesFor("p1").map((n) => n.id)).toEqual(["n1", "n2"]);
  });

  it("reads the notes from the daemon: an event carries a worktree's whole list", async () => {
    const store = await boot();
    await vi.waitFor(() => expect(adds()).toHaveLength(3));
    handlers["daemon-event"]({ event: "notes.changed", params: { path: "c:/w/a", notes: [note("d1", 2), note("d2", 4)] } });
    expect(store.notesFor("p1").map((n) => n.id).sort()).toEqual(["d1", "d2"]);
    expect(store.notesFor("p2").map((n) => n.id)).not.toContain("d1");
    handlers["daemon-event"]({ event: "notes.changed", params: { path: "c:/w/a", notes: [] } });
    expect(store.notesFor("p1")).toEqual([]);
  });

  it("keeps a worktree inside another folder apart: it never shows or sends the notes of the folder around it", async () => {
    const store = await boot();
    await vi.waitFor(() => expect(adds()).toHaveLength(3));
    expect(calls.find((c) => c.method === "note.keys")?.params.paths).toContain("C:/w/a/.claude/worktrees/x");
    handlers["daemon-event"]({ event: "notes.changed", params: { path: "c:/w/a", notes: [note("outer", 2)] } });
    expect(store.notesFor("p1").map((n) => n.id)).toEqual(["outer"]);
    expect(store.notesFor("nested")).toEqual([]);
    handlers["daemon-event"]({ event: "notes.changed", params: { path: "c:/w/a/.claude/worktrees/x", notes: [note("inner", 1)] } });
    expect(store.notesFor("nested").map((n) => n.id)).toEqual(["inner"]);
    expect(store.notesFor("p1").map((n) => n.id)).toEqual(["outer"]);
  });

  it("adds and resolves notes through the daemon, not the app's state", async () => {
    const store = await boot();
    await vi.waitFor(() => expect(adds()).toHaveLength(3));
    calls.length = 0;
    await store.addNote("p2", { path: "src/b.ts", line: 7, side: "old", text: "gone line", note: "why removed?" });
    expect(calls.map((c) => c.method)).toEqual(["note.add"]);
    expect(calls[0].params).toMatchObject({ path: "C:/w/b", file: "src/b.ts", line: 7, side: "old", text: "gone line", note: "why removed?" });
    await store.removeNotes("p2", ["x1"]);
    await store.removeNotes("p2", "all");
    expect(calls.slice(1).map((c) => [c.method, c.params.ids])).toEqual([["note.clear", ["x1"]], ["note.clear", undefined]]);
    expect(store.state.reviewNotes.p2).toBeUndefined();
  });
});

describe("review notes on an older daemon", () => {
  it("stay in the app's state and are not sent anywhere", async () => {
    daemonApi = 12;
    const store = await boot();
    await vi.waitFor(() => expect(calls.some((c) => c.method === "daemon.info")).toBe(true));
    expect(store.daemonApi()).toBe(12);
    expect(store.notesFor("p1").map((n) => n.id)).toEqual(["n1", "n2"]);
    await store.addNote("p1", { path: "src/c.ts", line: 1, side: "new", text: "t", note: "local" });
    expect(store.notesFor("p1").map((n) => n.note)).toEqual(["note n1", "note n2", "local"]);
    await store.removeNotes("p1", ["n1"]);
    expect(store.notesFor("p1").map((n) => n.id)).toHaveLength(2);
    expect(calls.filter((c) => c.method.startsWith("note."))).toEqual([]);
  });
});
