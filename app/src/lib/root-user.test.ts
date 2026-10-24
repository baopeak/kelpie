import { beforeEach, describe, expect, it, vi } from "vitest";

const settings: { rootYoloOk?: string[] } = {};
const host = { id: "h1", alias: "vps", label: "vps" };
const check = { root: true } as { root: boolean } | undefined;
const ssh = { cached: check as typeof check, checkHost: vi.fn(async () => ({ root: true })) };
vi.mock("./store", () => ({ store: { state: { settings } } }));
vi.mock("./ssh-hosts", () => ({
  hostOf: (t: { dest: string }) => (t.dest === "vps" ? host : t.dest === "vps2" ? { ...host, id: "h2", alias: "vps2" } : undefined),
  cachedCheck: () => ssh.cached,
  checkHost: () => ssh.checkHost(),
}));

const { closeRootUserDialog, confirmRootStart, rootUserDialog, skipsPermissions, validUserName, warnToGuide } = await import("./root-user");
const target = { dest: "vps", path: "/srv/app", tmux: true };
const yolo = ["--dangerously-skip-permissions"];

describe("skipsPermissions", () => {
  it("sees every flag Claude refuses as root", () => {
    expect(skipsPermissions(["--dangerously-skip-permissions"])).toBe(true);
    expect(skipsPermissions(["--model", "opus", "--permission-mode", "bypassPermissions"])).toBe(true);
    expect(skipsPermissions(["--permission-mode=bypassPermissions"])).toBe(true);
    expect(skipsPermissions(["--allow-dangerously-skip-permissions"])).toBe(true);
    expect(skipsPermissions(["--permission-mode", "plan", "bypassPermissions"])).toBe(false);
    expect(skipsPermissions([])).toBe(false);
  });
});

describe("validUserName", () => {
  it("takes what useradd takes, never root", () => {
    for (const ok of ["dev", "_svc", "web-1"]) expect(validUserName(ok)).toBe(true);
    for (const bad of ["", "root", "Dev", "1dev", "-x", "a b", "a;b", "a".repeat(33)]) expect(validUserName(bad)).toBe(false);
  });
});

describe("confirmRootStart", () => {
  beforeEach(() => {
    closeRootUserDialog();
    settings.rootYoloOk = undefined;
    ssh.cached = { root: true };
  });

  it("starts at once unless it is Claude in Yolo mode on a saved root host", async () => {
    expect(await confirmRootStart("claude", ["--model", "opus"], target, "p")).toBe(true);
    expect(await confirmRootStart("codex", yolo, target, "p")).toBe(true);
    expect(await confirmRootStart("claude", yolo, undefined, "p")).toBe(true);
    expect(await confirmRootStart("claude", yolo, { ...target, dest: "unsaved" }, "p")).toBe(true);
    ssh.cached = { root: false };
    expect(await confirmRootStart("claude", yolo, target, "p")).toBe(true);
    ssh.cached = { root: true };
    settings.rootYoloOk = ["h1"];
    expect(await confirmRootStart("claude", yolo, target, "p")).toBe(true);
    expect(rootUserDialog()).toBeNull();
  });

  it("waits for the answer, asked once for starts together", async () => {
    const a = confirmRootStart("claude", yolo, target, "p");
    const b = confirmRootStart("claude", yolo, target, "p");
    await vi.waitFor(() => expect(rootUserDialog()?.mode).toBe("warn"));
    expect(rootUserDialog()).toMatchObject({ hostId: "h1", projectId: "p" });
    closeRootUserDialog(true);
    expect(await a).toBe(true);
    expect(await b).toBe(true);
  });

  it("asks for each root host in turn, never cancelling another host's start", async () => {
    const a = confirmRootStart("claude", yolo, target, "p");
    const b = confirmRootStart("claude", yolo, { ...target, dest: "vps2" }, "p");
    await vi.waitFor(() => expect(rootUserDialog()?.hostId).toBe("h1"));
    closeRootUserDialog(true);
    expect(await a).toBe(true);
    await vi.waitFor(() => expect(rootUserDialog()?.hostId).toBe("h2"));
    closeRootUserDialog(false);
    expect(await b).toBe(false);
  });

  it("cancels the start when the user goes to the guide", async () => {
    const a = confirmRootStart("claude", yolo, target, "p");
    await vi.waitFor(() => expect(rootUserDialog()).not.toBeNull());
    warnToGuide();
    expect(await a).toBe(false);
    expect(rootUserDialog()).toMatchObject({ mode: "guide", hostId: "h1", projectId: "p" });
  });

  it("checks a host not checked yet; one that cannot be reached starts as usual", async () => {
    ssh.cached = undefined;
    ssh.checkHost.mockRejectedValueOnce(new Error("offline"));
    expect(await confirmRootStart("claude", yolo, target, "p")).toBe(true);
    ssh.checkHost.mockResolvedValueOnce({ root: true });
    const a = confirmRootStart("claude", yolo, target, "p");
    await vi.waitFor(() => expect(rootUserDialog()).not.toBeNull());
    closeRootUserDialog(false);
    expect(await a).toBe(false);
  });
});
