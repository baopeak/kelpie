// Tabs for the web pages an agent starts, in the real store: off on a new install, so a port an agent opens gets no tab unless
// the person turned this on. A choice already saved is kept.
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), requestPermission: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("./ipc");
vi.mock("./terminal-host", () => ({ disposeView: vi.fn(), peekView: vi.fn(), resetStreams: vi.fn(), setInputFanout: vi.fn() }));
vi.mock("./browser", () => ({ closeBrowser: vi.fn() }));

async function boot(saved: unknown) {
  vi.resetModules();
  (globalThis as any).window = globalThis;
  const ipc = vi.mocked(await import("./ipc"));
  ipc.loadState.mockResolvedValue(saved as any);
  ipc.saveState.mockResolvedValue(undefined);
  ipc.listSessions.mockResolvedValue([]);
  ipc.platformInfo.mockResolvedValue({ api: 13, version: "0.1.5", security: 1, os: "windows", home: "", data_dir: "", e2e: false } as any);
  ipc.onEvent.mockImplementation(async () => () => {});
  ipc.daemon.mockImplementation((async (method: string) => (method === "daemon.info" ? { api: 13, version: "0.1.5", security: 1, exe: "C:/k/kelpied.exe" } : [])) as any);
  const { store } = await import("./store");
  await store.init();
  return store;
}

describe("tabs for an agent's web pages", () => {
  it("are off on a new install", async () => {
    expect((await boot(null)).state.settings.agentWebTabs).toBe(false);
  });

  it("stay on for someone who already had them on", async () => {
    expect((await boot({ version: 3, settings: { agentWebTabs: true } })).state.settings.agentWebTabs).toBe(true);
  });
});
