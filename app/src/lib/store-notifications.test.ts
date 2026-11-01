// Desktop notifications in the real store: off on a new install, and the operating system is not even asked until the
// person turns them on. A choice already saved is kept.
import { beforeEach, describe, expect, it, vi } from "vitest";

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
  const os = vi.mocked(await import("@tauri-apps/plugin-notification"));
  os.isPermissionGranted.mockResolvedValue(true);
  const { store } = await import("./store");
  await store.init();
  return { store, os };
}

beforeEach(() => vi.clearAllMocks());

describe("desktop notifications", () => {
  it("are off on a new install: nothing is shown and the system is not asked for permission", async () => {
    const { store, os } = await boot(null);
    expect(store.state.settings.notifications).toBe(false);
    await store.desktopNotify("Claude", "Needs your input", "waiting");
    expect(os.isPermissionGranted).not.toHaveBeenCalled();
    expect(os.requestPermission).not.toHaveBeenCalled();
    expect(os.sendNotification).not.toHaveBeenCalled();
  });

  it("show once the person turns them on", async () => {
    const { store, os } = await boot(null);
    store.setSettings({ notifications: true });
    await store.desktopNotify("Claude", "Needs your input", "waiting");
    expect(os.sendNotification).toHaveBeenCalledWith({ title: "Claude", body: "Needs your input" });
  });

  it("stay on for someone who already had them on", async () => {
    const { store, os } = await boot({ version: 3, settings: { notifications: true } });
    expect(store.state.settings.notifications).toBe(true);
    await store.desktopNotify("Claude", "Needs your input", "waiting");
    expect(os.sendNotification).toHaveBeenCalledOnce();
  });
});
