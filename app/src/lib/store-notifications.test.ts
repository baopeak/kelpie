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
  let daemonEvent: (msg: { event: string; params: unknown }) => void = () => {};
  ipc.onEvent.mockImplementation((async (name: string, cb: any) => {
    if (name === "daemon-event") daemonEvent = cb;
    return () => {};
  }) as any);
  ipc.daemon.mockImplementation((async (method: string) => (method === "daemon.info" ? { api: 13, version: "0.1.5", security: 1, exe: "C:/k/kelpied.exe" } : [])) as any);
  const os = vi.mocked(await import("@tauri-apps/plugin-notification"));
  os.isPermissionGranted.mockResolvedValue(true);
  const { store } = await import("./store");
  await store.init();
  return { store, os, emit: (msg: { event: string; params: unknown }) => daemonEvent(msg) };
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

  it("tell of an agent a phone started while the window is not looking, beside its toast and Activity entry", async () => {
    const { store, os, emit } = await boot({ version: 3, settings: { notifications: true } });
    (globalThis as any).document = { hasFocus: () => false };
    emit({ event: "mobile.session_started", params: { id: "s1", name: "Pixel", kind: "claude", project_id: null } });
    await vi.waitFor(() => expect(os.sendNotification).toHaveBeenCalledOnce());
    const shown = os.sendNotification.mock.calls[0][0] as { title: string; body: string };
    expect(shown.title).toContain("Pixel");
    expect(shown.body).toBe("Started from a phone with this computer's settings for the agent.");
    expect(store.state.inbox?.[0]).toMatchObject({ kind: "phone", title: shown.title, sessionId: "s1" });
  });

  it("leave a phone's start to the toast while the window is looking, and to Activity alone when they are off", async () => {
    const looking = await boot({ version: 3, settings: { notifications: true } });
    (globalThis as any).document = { hasFocus: () => true };
    looking.emit({ event: "mobile.session_started", params: { id: "s1", name: "Pixel", kind: "claude", project_id: null } });
    const off = await boot(null);
    (globalThis as any).document = { hasFocus: () => false };
    off.emit({ event: "mobile.session_started", params: { id: "s2", name: "Pixel", kind: "claude", project_id: null } });
    await new Promise((r) => setTimeout(r, 0));
    expect(off.os.sendNotification).not.toHaveBeenCalled();
    expect(off.os.isPermissionGranted).not.toHaveBeenCalled();
    expect(off.store.state.inbox?.[0]).toMatchObject({ kind: "phone", sessionId: "s2" });
    expect(looking.store.state.inbox?.[0]).toMatchObject({ kind: "phone", sessionId: "s1" });
    expect(looking.os.sendNotification).not.toHaveBeenCalled();
  });

  it("stay on for someone who already had them on", async () => {
    const { store, os } = await boot({ version: 3, settings: { notifications: true } });
    expect(store.state.settings.notifications).toBe(true);
    await store.desktopNotify("Claude", "Needs your input", "waiting");
    expect(os.sendNotification).toHaveBeenCalledOnce();
  });
});
