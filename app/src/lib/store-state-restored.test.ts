// The app state after a sudden shutdown, in the real store: when the backend used the previous save because the file was damaged,
// the person is told it was restored (not that Kelpie starts fresh), the restored state is used, and nothing is set aside.
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), requestPermission: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("./ipc");
vi.mock("./terminal-host", () => ({ disposeView: vi.fn(), peekView: vi.fn(), resetStreams: vi.fn(), setInputFanout: vi.fn() }));
vi.mock("./browser", () => ({ closeBrowser: vi.fn() }));

async function boot(loaded: { state: unknown; restored: boolean; damaged_copy: string | null }) {
  vi.resetModules();
  (globalThis as any).window = globalThis;
  const ipc = vi.mocked(await import("./ipc"));
  ipc.loadState.mockResolvedValue(loaded);
  ipc.saveState.mockResolvedValue(undefined);
  ipc.stateBackup.mockResolvedValue("C:/data/app-state.unreadable-1.bak.json");
  ipc.listSessions.mockResolvedValue([]);
  ipc.platformInfo.mockResolvedValue({ api: 13, version: "0.1.5", security: 1, os: "windows", home: "", data_dir: "", e2e: false } as any);
  ipc.onEvent.mockImplementation((async () => () => {}) as any);
  ipc.daemon.mockImplementation((async (method: string) => (method === "daemon.info" ? { api: 13, version: "0.1.5", security: 1, exe: "C:/k/kelpied.exe" } : [])) as any);
  const { store } = await import("./store");
  await store.init();
  return { store, ipc };
}

const saved = { version: 3, settings: { fontSize: 17 } };

beforeEach(() => vi.clearAllMocks());

describe("an app state restored from the previous save", () => {
  it("is used, and the person is told where the damaged file's copy is", async () => {
    const { store, ipc } = await boot({ state: saved, restored: true, damaged_copy: "C:/data/app-state.json.damaged-1" });
    expect(store.state.settings.fontSize).toBe(17);
    const text = store.toast()?.text ?? "";
    expect(text).toContain("restored the app state from the previous save");
    expect(text).toContain("C:/data/app-state.json.damaged-1");
    expect(text).not.toContain("Starting fresh");
    expect(ipc.stateBackup).not.toHaveBeenCalled();
  });

  it("says so without a path when no copy could be kept", async () => {
    const { store } = await boot({ state: saved, restored: true, damaged_copy: null });
    expect(store.toast()?.text).toBe("Kelpie restored the app state from the previous save because the saved file was missing or damaged.");
  });

  it("says nothing when the file itself was used", async () => {
    const { store } = await boot({ state: saved, restored: false, damaged_copy: null });
    expect(store.toast()).toBeNull();
  });
});
