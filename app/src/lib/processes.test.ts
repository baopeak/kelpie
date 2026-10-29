import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ProcInfo, ProcessList } from "./processes";

const visible = new Set<string>();
const sessions: Record<string, { id: string; alive: boolean; project_id: string | null }> = { a: { id: "a", alive: true, project_id: null }, b: { id: "b", alive: true, project_id: null } };
const calls: { method: string; params: any }[] = [];
const answer = { children: 0 };
let clock = Date.now();

vi.stubGlobal("document", { hidden: false });
vi.mock("./store", () => ({
  store: { sessions, connected: () => true, daemonApi: () => 16, isSessionVisible: (id: string) => visible.has(id), state: { activeProject: null } },
}));
vi.mock("./ipc", () => ({
  daemon: vi.fn(async (method: string, params: any) => {
    calls.push({ method, params });
    return { sessions: Object.fromEntries((params.sessions as string[]).map((id) => [id, { children: answer.children, ports: answer.children ? [{ port: 5173, host: "localhost", addr: "127.0.0.1" }] : [] }])) };
  }),
}));
vi.mock("./browser", () => ({ openBrowser: vi.fn() }));
vi.mock("./terminal-host", () => ({ onTerminalOutput: vi.fn() }));
vi.mock("./i18n", () => ({ errorText: (s: string) => s, t: (s: string, v?: Record<string, string>) => s.replace(/\{(\w+)\}/g, (_: string, k: string) => v?.[k] ?? "") }));

const { chips, noteOutput, killSummary, formatBytes } = await import("./processes");
const { rows, subtree } = await import("./process-tree");

const p = (pid: number, ppid: number | null, name = `p${pid}`): ProcInfo => ({ pid, ppid, name, start_time: 1, exe: null, mem: 0, cpu: 0, ports: [] });
const list = (root: ProcInfo | null, children: ProcInfo[]): ProcessList => ({ session: "s", root, children, orphans: [], keep: root ? [root.pid] : [], dropped: 0, took_us: 0 });

describe("looking for ports", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime((clock += 10_000_000));
    calls.length = 0;
    visible.clear();
    answer.children = 0;
  });
  afterEach(() => {
    vi.runAllTimers();
    vi.useRealTimers();
  });

  it("does not look for a session that is not on screen, or when nothing printed", async () => {
    noteOutput("a");
    await vi.advanceTimersByTimeAsync(10_000);
    expect(calls).toHaveLength(0);
  });

  it("looks once for a burst of output, asks only about what is on screen, and stops when nothing runs under it", async () => {
    visible.add("a");
    for (let i = 0; i < 50; i++) noteOutput("a");
    await vi.advanceTimersByTimeAsync(5_000);
    expect(calls).toHaveLength(1);
    expect(calls[0]).toEqual({ method: "process.ports", params: { sessions: ["a"] } });
    // Nothing runs under it: no timer of its own keeps looking.
    await vi.advanceTimersByTimeAsync(60_000);
    expect(calls).toHaveLength(1);
    expect(chips.a?.children).toBe(0);
  });

  it("keeps looking every few seconds while programs run, shows the port, and stops when they are gone", async () => {
    visible.add("a");
    answer.children = 1;
    noteOutput("a");
    await vi.advanceTimersByTimeAsync(1_000);
    expect(chips.a?.ports.map((x) => x.port)).toEqual([5173]);
    await vi.advanceTimersByTimeAsync(9_000);
    expect(calls.length).toBeGreaterThanOrEqual(3);
    answer.children = 0;
    await vi.advanceTimersByTimeAsync(4_000);
    const seen = calls.length;
    await vi.advanceTimersByTimeAsync(30_000);
    expect(calls).toHaveLength(seen);
    expect(chips.a?.ports).toEqual([]);
  });
});

describe("the tree", () => {
  it("lists programs under their parents, and one whose parent is gone under the session's process", () => {
    const l = list(p(1, null), [p(2, 1), p(3, 2), p(4, 99), p(5, 1)]);
    expect(rows(l).map((r) => [r.p.pid, r.depth])).toEqual([[1, 0], [2, 1], [3, 2], [4, 1], [5, 1]]);
    expect(subtree(l, p(2, 1)).map((x) => x.pid)).toEqual([2, 3]);
    expect(subtree(l, p(5, 1)).map((x) => x.pid)).toEqual([5]);
  });

  it("survives a parent cycle and shows every program once", () => {
    const l = list(p(1, null), [p(2, 3), p(3, 2)]);
    expect(rows(l).map((r) => r.p.pid).sort()).toEqual([1, 2, 3]);
  });
});

describe("messages", () => {
  it("says why a stop was refused, once per reason", () => {
    expect(killSummary({ closed: [1], forced: [], gone: [], refused: [] })).toBeNull();
    expect(killSummary({ closed: [], forced: [], gone: [], refused: [{ pid: 1, reason: "x" }, { pid: 2, reason: "x" }] })).toBe("Not stopped: x");
    expect(formatBytes(5 * 1024 * 1024)).toBe("5 MB");
  });
});
