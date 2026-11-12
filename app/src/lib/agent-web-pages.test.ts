import { beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import type { Port } from "./processes";

// A window that carries events: the daemon's `page.open` reaches the module as `kelpie:open-page`.
vi.stubGlobal("window", new EventTarget());

const h = vi.hoisted(() => ({
  sessions: {} as Record<string, { id: string; kind: string; project_id: string | null }>,
  settings: { agentWebTabs: true as boolean },
  projects: [{ id: "p" }] as { id: string }[],
  tabs: [] as { id: string; root: { paneId: string } }[],
  probe: vi.fn(),
  addTab: vi.fn(),
  closePane: vi.fn(async () => {}),
  focusPane: vi.fn(),
  notify: vi.fn(),
  paneOnPort: vi.fn(() => null as { tabId: string; paneId: string } | null),
  paneShowing: vi.fn(() => null as { tabId: string; paneId: string } | null),
  setBrowserState: vi.fn(),
}));

const [ports, setPorts] = createRoot(() => createStore<Record<string, { children: number; ports: Port[]; stale?: boolean }>>({}));
const [fresh, setFresh] = createRoot(() => createSignal<ReadonlySet<string>>(new Set()));

vi.mock("./store", () => ({
  sessionLabel: (s: { kind: string }) => s.kind,
  store: {
    sessions: h.sessions,
    state: { settings: h.settings, projects: h.projects },
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
  browserPaneShowing: h.paneShowing,
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
    h.paneShowing.mockReset();
    h.paneShowing.mockReturnValue(null);
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

describe("web pages an agent asks for", () => {
  const ask = (detail: Record<string, unknown>) => window.dispatchEvent(new CustomEvent("kelpie:open-page", { detail }));

  beforeEach(() => {
    h.settings.agentWebTabs = true;
    h.tabs.length = 0;
    for (const f of [h.probe, h.addTab, h.closePane, h.focusPane, h.notify, h.setBrowserState, h.paneOnPort, h.paneShowing]) f.mockReset();
    h.closePane.mockImplementation(async () => {});
    h.paneOnPort.mockReturnValue(null);
    h.paneShowing.mockReturnValue(null);
    h.addTab.mockImplementation((_p: string, pane: { id: string }) => {
      const tab = { id: `tab-${pane.id}`, root: { paneId: pane.id } };
      h.tabs.push(tab);
      return tab;
    });
  });

  it("opens the page in a background tab of the session's project, even with automatic tabs off, and says so", () => {
    h.settings.agentWebTabs = false;
    const s = session();
    ask({ session_id: s, project_id: "p", url: "http://localhost:5173", title: "Studio" });
    expect(h.addTab).toHaveBeenCalledTimes(1);
    const [projectId, pane, opts] = h.addTab.mock.calls[0];
    expect(projectId).toBe("p");
    expect(pane).toMatchObject({ kind: "browser", url: "http://localhost:5173/" });
    expect(opts).toEqual({ background: true });
    expect(h.setBrowserState).toHaveBeenCalledWith(pane.id, { title: "Studio", url: "http://localhost:5173/", design: false, notes: [] });
    expect(fresh().has(pane.id)).toBe(true);
    expect(h.notify.mock.calls[0][0]).toBe("claude opened a web page: Studio");
    expect(h.focusPane).not.toHaveBeenCalled();
    h.notify.mock.calls[0][1].run();
    expect(h.focusPane).toHaveBeenCalledWith("p", `tab-${pane.id}`, pane.id);
  });

  it("names the page by its address when the agent gave no title", () => {
    ask({ session_id: session(), project_id: "p", url: "https://example.com/docs?x=1", title: null });
    expect(h.addTab.mock.calls[0][1]).toMatchObject({ url: "https://example.com/docs?x=1" });
    expect(h.notify.mock.calls[0][0]).toBe("claude opened a web page: https://example.com/docs?x=1");
  });

  it("does not open the same address twice: it says the page is already open, and nothing takes the focus", () => {
    h.paneShowing.mockReturnValue({ tabId: "t2", paneId: "q2" });
    ask({ session_id: session(), project_id: "p", url: "https://example.com", title: null });
    expect(h.paneShowing).toHaveBeenCalledWith("p", "https://example.com/");
    expect(h.addTab).not.toHaveBeenCalled();
    expect(h.notify.mock.calls.map((c) => c[0])).toEqual(["claude asked to show https://example.com/, which is already open in a tab"]);
    expect(h.focusPane).not.toHaveBeenCalled();
    h.notify.mock.calls[0][1].run();
    expect(h.focusPane).toHaveBeenCalledWith("p", "t2", "q2");
  });

  it("opens another page of a local server that already has a tab: only the same address counts as already open", () => {
    h.paneOnPort.mockReturnValue({ tabId: "t", paneId: "q" });
    ask({ session_id: session(), project_id: "p", url: "http://127.0.0.1:3000/admin", title: "Admin" });
    expect(h.paneShowing).toHaveBeenCalledWith("p", "http://127.0.0.1:3000/admin");
    expect(h.paneOnPort).not.toHaveBeenCalled();
    expect(h.addTab).toHaveBeenCalledTimes(1);
    expect(h.addTab.mock.calls[0][1]).toMatchObject({ url: "http://127.0.0.1:3000/admin" });
    expect(h.notify.mock.calls[0][0]).toBe("claude opened a web page: Admin");
  });

  it("keeps an automatic tab that an agent then asked for, when its server stops", async () => {
    h.probe.mockResolvedValue({ title: "Dev" });
    const s = session();
    setPorts(s, { children: 1, ports: [port(8010)] });
    await settle();
    const paneId = h.addTab.mock.calls[0][1].id;
    h.paneShowing.mockReturnValue({ tabId: `tab-${paneId}`, paneId });
    ask({ session_id: s, project_id: "p", url: "http://localhost:8010/", title: null });
    expect(h.addTab).toHaveBeenCalledTimes(1);
    expect(h.notify.mock.calls[1][0]).toBe("claude asked to show http://localhost:8010/, which is already open in a tab");
    setPorts(s, { children: 0, ports: [] });
    await settle();
    expect(h.closePane).not.toHaveBeenCalled();
  });

  it("drops the characters of a title that turn or hide text", () => {
    ask({ session_id: session(), project_id: "p", url: "https://example.com/", title: "GitHub\u202Emoc.live\u202C sign\u200B-in\uFEFF" });
    expect(h.notify.mock.calls[0][0]).toBe("claude opened a web page: GitHubmoc.live sign-in");
    expect(h.setBrowserState.mock.calls[0][1]).toMatchObject({ title: "GitHubmoc.live sign-in" });
  });

  it("opens nothing that is not an http(s) page without credentials, nor in a project this window does not have, and says why", () => {
    const s = session();
    for (const url of ["javascript:alert(1)", "file:///C:/Windows/win.ini", "http://user:pass@example.com/", "not a url"]) {
      ask({ session_id: s, project_id: "p", url, title: null });
    }
    ask({ session_id: s, project_id: "gone", url: "http://localhost:5173/", title: null });
    expect(h.addTab).not.toHaveBeenCalled();
    expect(h.notify.mock.calls.map((c) => c[0])).toEqual([
      "claude asked to open javascript:alert(1), but only http and https pages open in Kelpie",
      "claude asked to open file:///C:/Windows/win.ini, but only http and https pages open in Kelpie",
      "claude asked to open http://user:pass@example.com/, but addresses with a user name or password are not opened",
      "claude asked to open not a url, but it is not a web address",
      "claude asked to open http://localhost:5173/, but its project is no longer in this window",
    ]);
  });
});
