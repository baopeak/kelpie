import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: { method: string; params: any }[] = [];
let api = 14;
vi.mock("./ipc", () => ({
  daemon: vi.fn(async (method: string, params: any) => {
    calls.push({ method, params });
    return { id: "r1", ...params };
  }),
  agentRun: vi.fn(),
  isDir: vi.fn(),
  onEvent: vi.fn(async () => () => {}),
}));
vi.mock("./platform", () => ({ normPath: (p: string) => p }));
vi.mock("./store", () => ({
  store: { daemonApi: () => api, profileLaunch: (kind: string) => ({ args: [`--${kind}`], env: { PROFILE: "1" } }), notify: vi.fn() },
}));
vi.mock("./remote", () => ({
  targetEnv: async (dest: string) => ({ SSH_ASKPASS: `askpass-for-${dest}` }),
  targetRemote: (t: { dest: string; port?: number; tmux: boolean }) => ({ dest: t.dest, port: t.port, config: "/cfg/ssh_config", tmux: t.tmux }),
}));

const { RUN_HOST_API, createSpecRun } = await import("./runs");
const project = { id: "p1", name: "app", path: "C:/work/app", color: "" } as any;
const task = { id: "spec-0123abcd", title: "do it", prompt: "prompt" };

describe("a spec task on a host", () => {
  beforeEach(() => {
    calls.length = 0;
    api = RUN_HOST_API;
  });

  it("goes to the daemon with the host, its folder and the host's askpass, and no worktree", async () => {
    await createSpecRun(project, task, "claude", false, undefined, { dest: "devbox", port: 2222, path: "/srv/app", tmux: true });
    expect(calls).toHaveLength(1);
    const run = calls[0].params;
    expect(calls[0].method).toBe("run.create");
    expect(run.cwd).toBe("C:/work/app");
    const t = run.tasks[0];
    expect(t.host).toEqual({ dest: "devbox", port: 2222, config: "/cfg/ssh_config", path: "/srv/app" });
    // Never in tmux: the task ends with its ssh session.
    expect(JSON.stringify(t.host)).not.toMatch(/tmux/);
    expect(t.worktree).toBe(false);
    expect(t.env).toEqual({ PROFILE: "1", SSH_ASKPASS: "askpass-for-devbox" });
    expect(t.args).toEqual(["--claude"]);
    expect(run.materialize).toBeUndefined();
  });

  it("is refused by the app, before anything is sent, when the daemon is older (it would run the task here)", async () => {
    api = RUN_HOST_API - 1;
    await expect(createSpecRun(project, task, "claude", false, undefined, { dest: "devbox", path: "/srv/app", tmux: false })).rejects.toThrow(/too old/);
    expect(calls).toHaveLength(0);
  });

  it("cannot have a worktree of its own", async () => {
    await expect(createSpecRun(project, task, "claude", true, { copy: [".env"], link: [] }, { dest: "devbox", path: "/srv/app", tmux: false })).rejects.toThrow(/worktree/);
    expect(calls).toHaveLength(0);
  });

  it("leaves local tasks as they were, on any daemon", async () => {
    api = 13;
    await createSpecRun(project, task, "codex", true, { copy: [".env"], link: ["deps"] });
    const run = calls[0].params;
    expect(run.tasks[0].host).toBeUndefined();
    expect(run.tasks[0].worktree).toBe(true);
    expect(run.materialize).toEqual({ copy: [".env"], link: ["deps"] });
  });

  it("starts a remote project's task in the folder on its host", async () => {
    const remote = { ...project, path: "ssh://devbox/srv/app", remote: { dest: "devbox", path: "/srv/app", tmux: false } };
    await createSpecRun(remote, task, "claude", false, undefined, { dest: "devbox", path: "/srv/app", tmux: false });
    expect(calls[0].params.cwd).toBe("/srv/app");
    expect(calls[0].params.project_id).toBe("p1");
  });
});
