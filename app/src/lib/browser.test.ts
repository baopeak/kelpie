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

import { browserClose, browserOpen, browserVisible } from "./browser";

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
