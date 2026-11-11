// The path menus: full paths on this computer and on a remote project's host, which sessions' files resolve in their project, and that a
// named file is looked for only when an item needs it.
import { beforeEach, describe, expect, it, vi } from "vitest";

const { notify, openFile, readFile, quickOpen, writeText, revealEntry, state, sessions } = vi.hoisted(() => ({
  notify: vi.fn(),
  openFile: vi.fn(),
  readFile: vi.fn(),
  quickOpen: vi.fn(),
  writeText: vi.fn(async () => {}),
  revealEntry: vi.fn(async () => {}),
  state: { projects: [] as any[], settings: {} },
  sessions: {} as Record<string, any>,
}));
vi.mock("../lib/store", () => ({ store: { notify, openFile, state, sessions } }));
vi.mock("../lib/ipc", () => ({ readFile, quickOpen, revealEntry }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

import { absolutePath, fileRefMenuItems, pathMenuItems, sessionFiles } from "./path-menu";

const local = { id: "p1", name: "proj", path: "D:\\proj", color: "#fff" } as any;
const remote = { id: "p2", name: "srv", path: "ssh://box/srv/app", color: "#fff", remote: { dest: "box", path: "/srv/app", tmux: false } } as any;
const settled = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
};
const labels = (items: ReturnType<typeof pathMenuItems>) => items.map((i) => (i ? i.label : "-"));
const run = (items: ReturnType<typeof pathMenuItems>, label: string) => items.find((i) => i?.label === label)!.run();

beforeEach(() => {
  vi.clearAllMocks();
  readFile.mockResolvedValue({ size: 1 });
  quickOpen.mockResolvedValue([]);
  state.projects = [local, remote];
  for (const k of Object.keys(sessions)) delete sessions[k];
});

describe("absolutePath", () => {
  it("joins in the system's own style on this computer", () => {
    expect(absolutePath(local, "src/a.ts")).toBe("D:\\proj\\src\\a.ts");
    expect(absolutePath({ path: "/home/u/proj" }, "src/a.ts")).toBe("/home/u/proj/src/a.ts");
  });

  it("gives a remote project's path as its host writes it, without the ssh:// part", () => {
    expect(absolutePath(remote, "src\\a.ts")).toBe("/srv/app/src/a.ts");
    expect(absolutePath({ path: "ssh://box/" }, "a.ts")).toBe("/a.ts");
    expect(absolutePath({ path: "ssh://box/" }, "")).toBe("/");
  });
});

describe("pathMenuItems", () => {
  it("copies the relative path, the full path and the name", async () => {
    const items = pathMenuItems(local, "src/a.ts");
    expect(labels(items)).toEqual(["Copy Relative Path", "Copy Path", "Copy Name", "Reveal in Explorer"]);
    run(items, "Copy Relative Path");
    run(items, "Copy Path");
    run(items, "Copy Name");
    await settled();
    expect(writeText.mock.calls.map((c: any[]) => c[0]).sort()).toEqual(["D:\\proj\\src\\a.ts", "a.ts", "src/a.ts"]);
  });

  it("offers no reveal for a remote project: its files are not in this computer's file manager", () => {
    expect(labels(pathMenuItems(remote, "a.ts"))).toEqual(["Copy Relative Path", "Copy Path", "Copy Name"]);
  });
});

describe("fileRefMenuItems", () => {
  it("looks for the file only when an item needs it, then copies where it really is", async () => {
    readFile.mockImplementation(async (_root: string, path: string) => {
      if (path !== "sub/x.py") throw new Error("not found");
      return { size: 1 };
    });
    quickOpen.mockResolvedValue([{ path: "sub/x.py", score: 1, indices: [], is_dir: false }]);
    const items = fileRefMenuItems(local, { path: "x.py" });
    expect(labels(items)).toEqual(["Open", "-", "Copy Relative Path", "Copy Path", "Copy Name", "Reveal in Explorer"]);
    expect(readFile).not.toHaveBeenCalled();
    run(items, "Copy Name");
    run(items, "Copy Relative Path");
    await settled();
    expect(writeText.mock.calls.map((c: any[]) => c[0])).toEqual(["x.py", "sub/x.py"]);
  });

  it("says why when the file cannot be found, and copies nothing", async () => {
    readFile.mockRejectedValue(new Error("not found"));
    run(fileRefMenuItems(local, { path: "gone.py" }), "Copy Path");
    await settled();
    expect(notify).toHaveBeenCalledWith("File not found: gone.py");
    expect(writeText).not.toHaveBeenCalled();
  });

  it("opens at the line, from the folder the text was written in", async () => {
    run(fileRefMenuItems(local, { path: "a.ts", line: 7 }, undefined, "D:\\proj\\sub"), "Open");
    await settled();
    expect(openFile).toHaveBeenCalledWith("p1", "sub/a.ts", { line: 7 });
  });
});

describe("sessionFiles", () => {
  it("gives a session's project and folder", () => {
    sessions.s1 = { id: "s1", project_id: "p1", cwd: "D:\\proj\\sub", remote: null };
    expect(sessionFiles("s1")).toEqual({ project: local, cwd: "D:\\proj\\sub" });
  });

  it("gives none for a session with no project, or on another machine than its project's files", () => {
    sessions.a = { id: "a", project_id: null, cwd: "D:\\x" };
    sessions.b = { id: "b", project_id: "p1", cwd: "/srv", remote: { dest: "box", tmux: false } };
    sessions.c = { id: "c", project_id: "p2", cwd: "/srv/app", remote: { dest: "other", tmux: false } };
    sessions.d = { id: "d", project_id: "p2", cwd: "/srv/app", remote: { dest: "box", port: 2222, tmux: false } };
    sessions.e = { id: "e", project_id: "p2", cwd: "D:\\x", remote: null };
    for (const id of ["a", "b", "c", "d", "e", "missing"]) expect(sessionFiles(id)).toBeNull();
    sessions.f = { id: "f", project_id: "p2", cwd: "/srv/app/x", remote: { dest: "box", tmux: false } };
    expect(sessionFiles("f")?.cwd).toBe("/srv/app/x");
  });
});
