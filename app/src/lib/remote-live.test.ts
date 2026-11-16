import { beforeEach, describe, expect, it, vi } from "vitest";

const statMany = vi.fn();
vi.mock("./ipc", () => ({ LIVE_API: 45, sftpStatMany: (...a: unknown[]) => statMany(...a) }));
const api = { level: 45 };
vi.mock("./store", () => ({ store: { daemonApi: () => api.level, activeProject: () => null } }));
vi.mock("./git-state", () => ({ gitState: { schedule: vi.fn() } }));
const refreshed: [string, string | null][] = [];
vi.mock("./file-refresh", () => ({ refreshFiles: (root: string, path: string | null) => refreshed.push([root, path]) }));

import { backoff, changed, hostPath, liveFor, plan, pollProject, resumeLive, stamp, watchFile, watchTree } from "./remote-live";
import type { Project, RemoteMeta } from "./types";

const project = (over: Partial<Project> = {}): Project =>
  ({ id: "p", name: "app", path: "ssh://box/srv/app", remote: { dest: "box", path: "/srv/app", tmux: true }, ...over }) as Project;
const meta = (mtime: number, size = 1, kind: RemoteMeta["kind"] = "file") => ({ kind, size, mtime, mode: null, uid: null, gid: null }) as RemoteMeta;

describe("what a poll compares", () => {
  it("maps project paths onto the host", () => {
    expect(hostPath("/srv/app", "")).toBe("/srv/app");
    expect(hostPath("/srv/app/", "src/a.ts")).toBe("/srv/app/src/a.ts");
    expect(hostPath("/", "etc")).toBe("/etc");
  });
  it("tells kind, time and size apart, and a path that went away", () => {
    expect(stamp(meta(5))).not.toBe(stamp(meta(6)));
    expect(stamp(meta(5, 1))).not.toBe(stamp(meta(5, 2)));
    expect(stamp(null)).toBe("gone");
  });
  it("reports only keys seen before with another stamp", () => {
    const prev = new Map([["d:", "dir:1:0"], ["f:a", "file:1:3"]]);
    const next = new Map([["d:", "dir:2:0"], ["f:a", "file:1:3"], ["f:new", "file:9:9"]]);
    expect(changed(prev, next)).toEqual(["d:"]);
  });
  it("reads a path once more on the poll after it changed or was first seen (same-second writes, the first look)", () => {
    const first = plan(new Map(), new Map([["d:", "dir:1:0"]]), new Set());
    expect(first.reload).toEqual([]);
    const again = plan(new Map([["d:", "dir:1:0"]]), new Map([["d:", "dir:1:0"]]), first.pending);
    expect(again.reload).toEqual(["d:"]); // the first look is checked once more
    const quiet = plan(new Map([["d:", "dir:1:0"]]), new Map([["d:", "dir:1:0"]]), again.pending);
    expect(quiet.reload).toEqual([]);
    const moved = plan(new Map([["f:a", "file:1:3"]]), new Map([["f:a", "file:2:3"]]), new Set());
    expect(moved.reload).toEqual(["f:a"]);
    // More written in that same second leaves the stamp as it was: the follow-up read still catches it.
    expect(plan(new Map([["f:a", "file:2:3"]]), new Map([["f:a", "file:2:3"]]), moved.pending).reload).toEqual(["f:a"]);
    // A path no longer shown is not read.
    expect(plan(new Map(), new Map(), new Set(["f:gone"])).reload).toEqual([]);
  });
  it("waits longer after each failure, and long at once after a refused login", () => {
    expect(backoff(1, false)).toBe(30_000);
    expect(backoff(2, false)).toBe(60_000);
    expect(backoff(9, false)).toBe(600_000);
    expect(backoff(1, true)).toBe(600_000);
  });
  it("covers projects without a typed port, on a daemon that can", () => {
    expect(liveFor(project())).toBe(true);
    expect(liveFor(project({ remote: { dest: "box", port: 2222, path: "/srv/app", tmux: true } }))).toBe(false);
    expect(liveFor(project({ remote: undefined }))).toBe(false);
    api.level = 44;
    expect(liveFor(project())).toBe(false);
    api.level = 45;
  });
});

describe("polling a remote project", () => {
  beforeEach(() => {
    statMany.mockReset();
    refreshed.length = 0;
  });

  it("reads again only the folders and files that changed on the host", async () => {
    const p = project();
    const dirsChanged: string[][] = [];
    const stopTree = watchTree(p, () => ["", "src"], (d) => dirsChanged.push(d));
    const stopFile = watchFile(p, "src/a.ts");
    statMany.mockResolvedValueOnce([meta(1, 0, "dir"), meta(1, 0, "dir"), meta(1, 10)]);
    await pollProject(p.path, 0);
    expect(statMany).toHaveBeenCalledWith("box", ["/srv/app", "/srv/app/src", "/srv/app/src/a.ts"]);
    expect(dirsChanged).toEqual([]); // the first look is not a change
    expect(refreshed).toEqual([]);
    statMany.mockResolvedValueOnce([meta(1, 0, "dir"), meta(1, 0, "dir"), meta(1, 10)]);
    await pollProject(p.path, 0);
    // The first look is checked once more, then nothing until something changes.
    expect(dirsChanged).toEqual([["", "src"]]);
    expect(refreshed).toEqual([[p.path, "src/a.ts"]]);
    dirsChanged.length = 0;
    refreshed.length = 0;
    statMany.mockResolvedValueOnce([meta(1, 0, "dir"), meta(1, 0, "dir"), meta(1, 10)]);
    await pollProject(p.path, 0);
    expect(dirsChanged).toEqual([]);
    statMany.mockResolvedValueOnce([meta(1, 0, "dir"), meta(2, 0, "dir"), meta(2, 11)]);
    await pollProject(p.path, 0);
    expect(dirsChanged).toEqual([["src"]]);
    expect(refreshed).toEqual([[p.path, "src/a.ts"]]);
    stopTree();
    stopFile();
  });

  it("goes quiet for a while when the host cannot be reached", async () => {
    const p = project({ path: "ssh://box/srv/other", remote: { dest: "box", path: "/srv/other", tmux: true } });
    const stop = watchFile(p, "x.md");
    statMany.mockRejectedValueOnce(new Error("the connection to the host was lost"));
    await pollProject(p.path, Date.now());
    await pollProject(p.path, Date.now());
    expect(statMany).toHaveBeenCalledTimes(1);
    expect(refreshed).toEqual([]);
    // Refresh asks again at once.
    resumeLive(p.path);
    statMany.mockResolvedValueOnce([meta(1)]);
    await pollProject(p.path, Date.now());
    expect(statMany).toHaveBeenCalledTimes(2);
    stop();
  });

  it("stops asking about a file no view has open", async () => {
    const p = project({ path: "ssh://box/srv/third", remote: { dest: "box", path: "/srv/third", tmux: true } });
    const one = watchFile(p, "a.md");
    const two = watchFile(p, "a.md");
    one();
    statMany.mockResolvedValueOnce([meta(1)]);
    await pollProject(p.path, 0);
    expect(statMany).toHaveBeenLastCalledWith("box", ["/srv/third/a.md"]);
    two();
    await pollProject(p.path, 0);
    expect(statMany).toHaveBeenCalledTimes(1);
  });

  it("brings back nothing for a project closed while its poll ran", async () => {
    const p = project({ path: "ssh://box/srv/fourth", remote: { dest: "box", path: "/srv/fourth", tmux: true } });
    const stop = watchFile(p, "a.md");
    let answer!: (v: unknown) => void;
    statMany.mockReturnValueOnce(new Promise((r) => (answer = r)));
    const poll = pollProject(p.path, 0);
    stop();
    answer([meta(1)]);
    await poll;
    // Opened again: its first look is a first look, not compared with the poll of the closed one.
    const again = watchFile(p, "a.md");
    statMany.mockResolvedValueOnce([meta(9)]);
    await pollProject(p.path, 0);
    expect(refreshed).toEqual([]);
    again();
  });
});
