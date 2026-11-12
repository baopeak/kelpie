// The daemon's `page.open` event in the real store: handed to lib/agent-web-pages.ts as the window event `kelpie:open-page`
// (the store cannot import that module, which imports the store).
import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-notification", () => ({ isPermissionGranted: vi.fn(), requestPermission: vi.fn(), sendNotification: vi.fn() }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("./ipc");
vi.mock("./terminal-host", () => ({ disposeView: vi.fn(), peekView: vi.fn(), resetStreams: vi.fn(), setInputFanout: vi.fn() }));
vi.mock("./browser", () => ({ closeBrowser: vi.fn() }));

const handlers: Record<string, (p: any) => void> = {};

async function boot() {
  vi.resetModules();
  (globalThis as any).window = globalThis;
  const ipc = vi.mocked(await import("./ipc"));
  ipc.loadState.mockResolvedValue(null as any);
  ipc.saveState.mockResolvedValue(undefined);
  ipc.listSessions.mockResolvedValue([]);
  ipc.platformInfo.mockResolvedValue({ api: 13, version: "0.1.5", security: 1, os: "windows", home: "", data_dir: "", e2e: false } as any);
  ipc.onEvent.mockImplementation(async (name: string, cb: any) => {
    handlers[name] = cb;
    return () => {};
  });
  ipc.daemon.mockImplementation((async (method: string) => (method === "daemon.info" ? { api: 13, version: "0.1.5", security: 1, exe: "C:/k/kelpied.exe" } : [])) as any);
  const { store } = await import("./store");
  await store.init();
}

describe("a page an agent asks for", () => {
  it("reaches the window as `kelpie:open-page` with the daemon's request as it came", async () => {
    await boot();
    const sent: Event[] = [];
    const target = globalThis as any;
    const had = target.dispatchEvent;
    target.dispatchEvent = (e: Event) => sent.push(e) > 0;
    try {
      const params = { session_id: "s1", project_id: "p", url: "http://localhost:5173/", title: "Studio" };
      handlers["daemon-event"]({ event: "page.open", params });
      const opened = sent.filter((e) => e.type === "kelpie:open-page");
      expect(opened).toHaveLength(1);
      expect((opened[0] as CustomEvent).detail).toEqual(params);
    } finally {
      target.dispatchEvent = had;
    }
  });
});
