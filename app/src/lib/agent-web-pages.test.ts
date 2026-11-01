import { beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import type { Port } from "./processes";

vi.stubGlobal("window", {});

const h = vi.hoisted(() => ({
  sessions: {} as Record<string, { id: string; kind: string; project_id: string | null }>,
  settings: { agentWebTabs: true as boolean },
  tabs: [] as { id: string; root: { paneId: string } }[],
  probe: vi.fn(),
  addTab: vi.fn(),
  closePane: vi.fn(async () => {}),
  focusPane: vi.fn(),
  notify: vi.fn(),
  paneOnPort: vi.fn(() => null as { tabId: string; paneId: string } | null),
  setBrowserState: vi.fn(),
}));

const [ports, setPorts] = createRoot(() => createStore<Record<string, { children: number; ports: Port[]; stale?: boolean }>>({}));
const [fresh, setFresh] = createRoot(() => createSignal<ReadonlySet<string>>(new Set()));

vi.mock("./store", () => ({
  sessionLabel: (s: { kind: string }) => s.kind,
  store: {
    sessions: h.sessions,
    state: { settings: h.settings },
    workspace: () => ({ tabs: h.tabs }),
    addTab: h.addTab,
    closePane: h.closePane,
    focusPane: h.focusPane,
    notify: h.notify,
  },
}));
vi.mock("./layout", () => ({ findPane: (root: { paneId: string }, id: string) => (root.paneId === id ? root : undefined) }));
vi.mock("./agents", () => ({ isAgentKind: (k: string) => k !== "shell" && k !== "custom" }));
vi.mock("./platform", () => ({ uid: () => `pane${h.addTab.mock.calls.length}` }));
vi.mock("./i18n", () => ({ t: (s: string, v?: Record<string, string>) => s.replace(/\{(\w+)\}/g, (_: string, k: string) => v?.[k] ?? "") }));
vi.mock("./processes", () => ({
  get sessionPorts() { return ports; },
  portUrl: (p: Port) => `http://${p.host}:${p.port}/`,
  uniquePorts: (x: Port[]) => x,
}));
vi.mock("./browser", () => ({
  browserProbe: h.probe,
  browserPaneOnPort: h.paneOnPort,
  setBrowserState: h.setBrowserState,
  freshPages: () => fresh(),
  markPageFresh: (id: string) => setFresh((s) => new Set(s).add(id)),
  markPageSeen: (id: string) => setFresh((s) => { const n = new Set(s); n.delete(id); return n; }),
}));

await import("./agent-web-pages");

const port = (n: number): Port => ({ port: n, host: "localhost", addr: "127.0.0.1" });
const settle = async () => { for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r)); };
let sessionNo = 0;
/** A new session each time: ports already considered stay considered for the module's life. */
const session = (kind = "claude", project: string | null = "p") => {
  const id = `s${++sessionNo}`;
  h.sessions[id] = { id, kind, project_id: project };
  return id;
};

describe("web pages an agent starts", () => {
  beforeEach(() => {
    h.settings.agentWebTabs = true;
    h.tabs.length = 0;
    for (const f of [h.probe, h.addTab, h.closePane, h.focusPane, h.notify, h.setBrowserState]) f.mockReset();
    h.closePane.mockImplementation(async () => {});
    h.paneOnPort.mockReset();
    h.paneOnPort.mockReturnValue(null);
    h.addTab.mockImplementation((_p: string, pane: { id: string }) => {
      const tab = { id: `tab-${pane.id}`, root: { paneId: pane.id } };
      h.tabs.push(tab);
      return tab;
    });
  });

  it("opens a page in a tab in the background, says so, and marks the tab until it is looked at", async () => {
    h.probe.mockResolvedValue({ title: "Studio" });
    const s = session();
    setPorts(s, { children: 1, ports: [port(3002)] });
    await settle();
    expect(h.probe).toHaveBeenCalledWith("localhost", 3002);
    expect(h.addTab).toHaveBeenCalledTimes(1);
    const [projectId, pane, opts] = h.addTab.mock.calls[0];
    expect(projectId).toBe("p");
    expect(pane).toMatchObject({ kind: "browser", url: "http://localhost:3002/" });
    expect(opts).toEqual({ background: true });
    expect(fresh().has(pane.id)).toBe(true);
    expect(h.notify.mock.calls[0][0]).toContain("Studio");
    h.notify.mock.calls[0][1].run();
    expect(h.focusPane).toHaveBeenCalledWith("p", `tab-${pane.id}`, pane.id);
  });

  it("opens no tab for a port that does not answer with a page", async () => {
    vi.useFakeTimers();
    h.probe.mockResolvedValue(null);
    const s = session();
    setPorts(s, { children: 1, ports: [port(56101)] });
    await vi.advanceTimersByTimeAsync(10_000);
    vi.useRealTimers();
    expect(h.probe).toHaveBeenCalledTimes(3);
    expect(h.addTab).not.toHaveBeenCalled();
  });

  it("waits for a server that listens before it answers", async () => {
    vi.useFakeTimers();
    h.probe.mockResolvedValueOnce(null).mockResolvedValue({ title: null });
    const s = session();
    setPorts(s, { children: 1, ports: [port(5173)] });
    await vi.advanceTimersByTimeAsync(5_000);
    vi.useRealTimers();
    expect(h.addTab).toHaveBeenCalledTimes(1);
    expect(h.notify.mock.calls[0][0]).toContain("localhost:5173");
  });

  it("leaves what a person runs in a shell alone", async () => {
    h.probe.mockResolvedValue({ title: "x" });
    for (const kind of ["shell", "custom"]) setPorts(session(kind), { children: 1, ports: [port(8000)] });
    await settle();
    expect(h.probe).not.toHaveBeenCalled();
  });

  it("does nothing when turned off", async () => {
    h.settings.agentWebTabs = false;
    h.probe.mockResolvedValue({ title: "x" });
    setPorts(session(), { children: 1, ports: [port(8001)] });
    await settle();
    expect(h.addTab).not.toHaveBeenCalled();
  });

  it("does not open a second tab for a port that already has one", async () => {
    h.probe.mockResolvedValue({ title: "x" });
    h.paneOnPort.mockReturnValue({ tabId: "t", paneId: "q" });
    setPorts(session(), { children: 1, ports: [port(8002)] });
    await settle();
    expect(h.addTab).not.toHaveBeenCalled();
  });

  it("opens a page once, however often its port is reported, and not again after the tab is closed", async () => {
    h.probe.mockResolvedValue({ title: "x" });
    const s = session();
    setPorts(s, { children: 1, ports: [port(8003)] });
    await settle();
    setPorts(s, { children: 2, ports: [port(8003)] });
    await settle();
    expect(h.addTab).toHaveBeenCalledTimes(1);
  });

  it("removes a tab nobody looked at when its port closes, and opens it again when the server returns", async () => {
    h.probe.mockResolvedValue({ title: "x" });
    const s = session();
    setPorts(s, { children: 1, ports: [port(8004)] });
    await settle();
    const paneId = h.addTab.mock.calls[0][1].id;
    setPorts(s, { children: 0, ports: [] });
    await settle();
    expect(h.closePane).toHaveBeenCalledWith("p", `tab-${paneId}`, paneId);
    setPorts(s, { children: 1, ports: [port(8004)] });
    await settle();
    expect(h.addTab).toHaveBeenCalledTimes(2);
  });

  it("keeps a tab that was looked at when its port closes", async () => {
    h.probe.mockResolvedValue({ title: "x" });
    const s = session();
    setPorts(s, { children: 1, ports: [port(8005)] });
    await settle();
    const paneId = h.addTab.mock.calls[0][1].id;
    setFresh((f) => { const n = new Set(f); n.delete(paneId); return n; });
    await settle();
    setPorts(s, { children: 0, ports: [] });
    await settle();
    expect(h.closePane).not.toHaveBeenCalled();
  });
});
