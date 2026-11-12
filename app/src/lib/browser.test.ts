import { beforeEach, describe, expect, it, vi } from "vitest";

// The order the backend receives commands in, and each call's pending promise.
const { calls, invoke } = vi.hoisted(() => {
  const calls: { cmd: string; args: Record<string, unknown>; finish: () => void; fail: (e: string) => void }[] = [];
  const invoke = vi.fn((cmd: string, args: Record<string, unknown>) =>
    new Promise<void>((finish, fail) => calls.push({ cmd, args, finish: () => finish(), fail })));
  return { calls, invoke };
});
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));
vi.mock("./store", () => ({ store: {} }));

import { browserClose, browserOpen, browserPaneOnPort, browserPaneShowing, browserVisible, setBrowserState } from "./browser";
import { store } from "./store";

const rect = { x: 0, y: 0, width: 100, height: 100 };
const settle = () => new Promise((r) => setTimeout(r));
const sent = () => calls.map((c) => (c.cmd === "browser_visible" ? `visible:${c.args.visible}` : c.cmd));

describe("browser webview lifecycle order", () => {
  beforeEach(() => {
    calls.length = 0;
  });

  it("holds a hide until the webview being created exists", async () => {
    void browserOpen("browser-a", "http://localhost:1", rect);
    void browserVisible("browser-a", false);
    await settle();
    expect(sent()).toEqual(["browser_open"]);

    calls[0].finish();
    await settle();
    expect(sent()).toEqual(["browser_open", "visible:false"]);
    calls[1].finish();
  });

  it("keeps a close behind the open, so no webview outlives its pane", async () => {
    void browserOpen("browser-b", "http://localhost:1", rect);
    void browserClose("browser-b");
    await settle();
    expect(sent()).toEqual(["browser_open"]);

    calls[0].finish();
    await settle();
    expect(sent()).toEqual(["browser_open", "browser_close"]);
    calls[1].finish();
  });

  it("applies quick switches in the order they were made", async () => {
    void browserOpen("browser-c", "http://localhost:1", rect);
    void browserVisible("browser-c", false);
    void browserOpen("browser-c", "http://localhost:1", rect);
    void browserVisible("browser-c", false);
    for (let i = 0; i < 4; i++) {
      await settle();
      calls[i].finish();
    }
    expect(sent()).toEqual(["browser_open", "visible:false", "browser_open", "visible:false"]);
  });

  it("carries on after a failed open, and reports the failure to its caller", async () => {
    const open = browserOpen("browser-d", "nope:", rect);
    void browserVisible("browser-d", false);
    await settle();
    calls[0].fail("only http and https pages open in the browser pane");
    await expect(open).rejects.toBe("only http and https pages open in the browser pane");
    await settle();
    expect(sent()).toEqual(["browser_open", "visible:false"]);
    calls[1].finish();
  });

  it("does not make one pane wait for another", async () => {
    void browserOpen("browser-e", "http://localhost:1", rect);
    void browserVisible("browser-f", false);
    await settle();
    expect(sent()).toEqual(["browser_open", "visible:false"]);
    calls[0].finish();
    calls[1].finish();
  });
});

describe("the browser pane that shows a page", () => {
  const leaf = (pane: Record<string, unknown>) => ({ type: "leaf", id: `l-${pane.id}`, pane });
  // One project: a tab with a terminal beside a pane opened on a dev server, another opened on a site that moved on to another page.
  (store as any).workspace = (projectId: string) => ({
    tabs: projectId !== "p" ? [] : [
      { id: "t1", root: { type: "split", id: "s", dir: "row", sizes: [0.5, 0.5], children: [leaf({ id: "term", kind: "terminal", url: "http://localhost:3000/" }), leaf({ id: "dev", kind: "browser", url: "http://localhost:3000/" })] } },
      { id: "t2", root: leaf({ id: "docs", kind: "browser", url: "https://example.com/start" }) },
    ],
  });
  setBrowserState("docs", { title: "", url: "https://example.com/docs?x=1", design: false, notes: [] });

  it("is the one opened on that address or on it now, and only a browser pane", () => {
    expect(browserPaneShowing("p", "http://localhost:3000/")).toEqual({ tabId: "t1", paneId: "dev" });
    expect(browserPaneShowing("p", "https://example.com/start")).toEqual({ tabId: "t2", paneId: "docs" });
    expect(browserPaneShowing("p", "https://example.com/docs?x=1")).toEqual({ tabId: "t2", paneId: "docs" });
  });

  it("is none for another page of the same server, a different query or another project", () => {
    expect(browserPaneShowing("p", "http://localhost:3000/admin")).toBeNull();
    expect(browserPaneShowing("p", "https://example.com/docs?x=2")).toBeNull();
    expect(browserPaneShowing("q", "http://localhost:3000/")).toBeNull();
  });

  it("matches a local server by its port alone for automatic tabs", () => {
    expect(browserPaneOnPort("p", 3000)).toEqual({ tabId: "t1", paneId: "dev" });
    expect(browserPaneOnPort("p", 3001)).toBeNull();
  });
});
