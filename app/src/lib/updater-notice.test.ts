import { beforeEach, describe, expect, it, vi } from "vitest";

const settings: Record<string, unknown> = {};
const notify = vi.fn();
const setOverlay = vi.fn();
const invoke = vi.fn();

vi.mock("./store", () => ({
  store: {
    state: { settings },
    sessions: {},
    setSettings: (patch: Record<string, unknown>) => Object.assign(settings, patch),
    notify,
    setOverlay,
  },
}));
vi.mock("./i18n", () => ({ t: (text: string, vars: Record<string, string> = {}) => text.replace(/\{(\w+)\}/g, (_, k) => vars[k]) }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

const { available, checkForUpdate } = await import("./updater");

const release = (version: string) => ({ version, current_version: "0.1.8" });

describe("the notice of a new release", () => {
  beforeEach(() => {
    for (const key of Object.keys(settings)) delete settings[key];
    notify.mockClear();
    setOverlay.mockClear();
  });

  it("comes once per release, and its button opens the update dialog", async () => {
    invoke.mockResolvedValue(release("0.1.10"));
    await checkForUpdate();
    await checkForUpdate();
    expect(notify).toHaveBeenCalledTimes(1);
    const [text, action] = notify.mock.calls[0];
    expect(text).toBe("Kelpie 0.1.10 is available");
    action.run();
    expect(setOverlay).toHaveBeenCalledWith("update");
    expect(available()?.version).toBe("0.1.10");

    invoke.mockResolvedValue(release("0.1.11"));
    await checkForUpdate();
    expect(notify).toHaveBeenCalledTimes(2);
  });

  it("does not come back after the app restarts", async () => {
    settings.announcedUpdate = "0.1.10";
    invoke.mockResolvedValue(release("0.1.10"));
    await checkForUpdate();
    expect(notify).not.toHaveBeenCalled();
    expect(available()?.version).toBe("0.1.10");
  });

  it("stays quiet for a skipped release, when there is none, and when the check fails", async () => {
    settings.skippedUpdate = "0.1.10";
    invoke.mockResolvedValue(release("0.1.10"));
    await checkForUpdate();
    invoke.mockResolvedValue(null);
    await checkForUpdate();
    invoke.mockRejectedValue(new Error("offline"));
    await checkForUpdate();
    expect(notify).not.toHaveBeenCalled();
    expect(settings.announcedUpdate).toBeUndefined();
  });

  it("is not needed after a manual check, which opens the dialog itself", async () => {
    invoke.mockResolvedValue(release("0.1.10"));
    await checkForUpdate(true);
    await checkForUpdate();
    expect(notify).not.toHaveBeenCalled();
  });
});
