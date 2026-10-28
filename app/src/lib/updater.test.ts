import { describe, expect, it, vi } from "vitest";

// updater.ts reads the store and Tauri APIs at import time only through these modules.
vi.mock("./store", () => ({ store: { state: { settings: {} }, sessions: {}, setSettings: vi.fn(), notify: vi.fn() } }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const { daemonSurvivesUpdate, idle, offered, olderVersion, CHECK_EVERY_MS } = await import("./updater");
import type { SessionInfo } from "./types";

const session = (id: string, kind: string, status: string, alive = true) => ({ id, kind, status, alive }) as unknown as SessionInfo;

describe("updater", () => {
  it("offers a newer release unless that version was skipped", () => {
    const info = { version: "0.2.0", current_version: "0.1.2" };
    expect(offered(info)).toBe(info);
    expect(offered(info, "0.1.9")).toBe(info);
    expect(offered(info, "0.2.0")).toBeNull();
    expect(offered(null)).toBeNull();
  });

  it("is idle only when no live session works", () => {
    expect(idle([])).toBe(true);
    expect(idle([session("a", "claude", "idle"), session("b", "shell", "waiting")])).toBe(true);
    expect(idle([session("a", "claude", "working", false)])).toBe(true);
    expect(idle([session("a", "claude", "working")])).toBe(false);
  });

  it("compares release versions", () => {
    expect(olderVersion("0.1.2", "0.1.10")).toBe(true);
    expect(olderVersion("0.2.0", "0.1.9")).toBe(false);
    expect(olderVersion("0.1.2", "0.1.2")).toBe(false);
    expect(olderVersion("dev", "0.1.2")).toBe(false);
  });

  it("knows when the daemon keeps running through an install", () => {
    expect(daemonSurvivesUpdate(String.raw`C:\Users\u\AppData\Local\Kelpie\daemon\0.1.5-6a1b-2f00\kelpied.exe`, false)).toBe(true);
    expect(daemonSurvivesUpdate(String.raw`C:\Users\u\AppData\Local\Kelpie\kelpied.exe`, false)).toBe(false);
    expect(daemonSurvivesUpdate("/Users/u/Library/Application Support/Kelpie/daemon/b/kelpied", false)).toBe(true);
    expect(daemonSurvivesUpdate(undefined, false)).toBe(false);
    expect(daemonSurvivesUpdate(undefined, true)).toBe(true);
  });

  it("checks every six hours", () => {
    expect(CHECK_EVERY_MS).toBe(6 * 3600 * 1000);
  });
});
