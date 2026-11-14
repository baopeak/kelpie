import { describe, expect, it } from "vitest";
import {
  buildSessionTree,
  childrenOfSession,
  dispatchRequest,
  emptyRequests,
  familyOf,
  familyRoot,
  finishedChildren,
  folderName,
  hasLiveDescendant,
  parseSubMax,
  postponeRequest,
  showPostponed,
  shownRequests,
  reduceRequests,
  stateWithReport,
  sessionDepthTree,
  subRequests,
  subtreeSummary,
  toSubPolicy,
  toSubRequest,
  type ChildState,
  type SubRequest,
} from "./sub-sessions";
import type { SessionInfo } from "./types";

let handle = 0;
const session = (id: string, extra: Partial<SessionInfo> = {}): SessionInfo => ({
  handle: ++handle, id, project_id: "p", name: id, kind: "claude", cwd: "/w", args: [], env: {}, agent_session_id: null, model: null,
  created_at: handle, title: null, status: "idle", unread: false, alive: true, exit_code: null, pid: 1, cols: 80, rows: 24, seq: 0, chat: false,
  last_activity: 0, interrupted: false, ...extra,
});
const ids = (list: readonly SessionInfo[]) => list.map((s) => s.id);

describe("the session tree", () => {
  it("nests a session under its parent, to any depth, oldest first", () => {
    const list = [session("root"), session("b", { parent: "root" }), session("a", { parent: "root" }), session("leaf", { parent: "a" })];
    const tree = buildSessionTree(list);
    expect(ids(childrenOfSession(tree, "root"))).toEqual(["b", "a"]);
    expect(ids(childrenOfSession(tree, "a"))).toEqual(["leaf"]);
    expect(childrenOfSession(tree, "leaf")).toEqual([]);
    expect([...tree.nested.keys()].sort()).toEqual(["a", "b", "leaf"]);
    expect(["root", "a", "leaf"].map((id) => sessionDepthTree(tree, id))).toEqual([0, 1, 2]);
  });

  it("leaves a session where it was when its parent is gone", () => {
    const tree = buildSessionTree([session("orphan", { parent: "closed-long-ago" }), session("kid", { parent: "orphan" })]);
    expect(tree.nested.has("orphan")).toBe(false);
    expect(tree.nested.get("kid")).toBe("orphan");
    expect(sessionDepthTree(tree, "orphan")).toBe(0);
  });

  it("never loses a session to a loop, a self-parent or a parent that is not a string", () => {
    const tree = buildSessionTree([
      session("a", { parent: "b" }),
      session("b", { parent: "a" }),
      session("hangs-off-the-loop", { parent: "a" }),
      session("self", { parent: "self" }),
      session("odd", { parent: "" }),
      session("fine-root"),
      session("fine-kid", { parent: "fine-root" }),
    ]);
    for (const id of ["a", "b", "hangs-off-the-loop", "self", "odd", "fine-root"]) expect(tree.nested.has(id), id).toBe(false);
    expect(tree.nested.get("fine-kid")).toBe("fine-root");
    // Whatever is listed as a child is also marked nested, and nothing is listed twice.
    const listed = [...tree.children.values()].flat();
    expect(listed.every((s) => tree.nested.has(s.id))).toBe(true);
    expect(new Set(ids(listed)).size).toBe(listed.length);
    // Walks over a loop end.
    expect(sessionDepthTree(tree, "a")).toBe(0);
    expect(subtreeSummary(tree, "a", () => "idle")).toEqual({ count: 0, worst: "exited" });
  });

  it("is cheap on a long chain and a wide fan", () => {
    const list: SessionInfo[] = [session("n0")];
    for (let i = 1; i < 300; i++) list.push(session(`n${i}`, { parent: `n${i - 1}` }));
    for (let i = 0; i < 300; i++) list.push(session(`w${i}`, { parent: "n0" }));
    const tree = buildSessionTree(list);
    expect(sessionDepthTree(tree, "n299")).toBe(299);
    expect(childrenOfSession(tree, "n0")).toHaveLength(301);
  });
});

describe("a family of sessions in one tab", () => {
  it("finds the root from any depth, and lists the root, then each parent before its children, oldest first", () => {
    const list = [session("root"), session("b", { parent: "root" }), session("a", { parent: "root" }), session("leaf", { parent: "b" }), session("alone")];
    const tree = buildSessionTree(list);
    expect(["root", "b", "leaf", "alone"].map((id) => familyRoot(tree, id))).toEqual(["root", "root", "root", "alone"]);
    expect(familyOf(tree, list[0]).map((m) => `${m.s.id}:${m.depth}`)).toEqual(["root:0", "b:1", "leaf:2", "a:1"]);
    expect(familyOf(tree, list[4]).map((m) => m.s.id)).toEqual(["alone"]);
  });

  it("treats a session whose parent is gone as its own root", () => {
    const tree = buildSessionTree([session("orphan", { parent: "closed" }), session("kid", { parent: "orphan" })]);
    expect(familyRoot(tree, "kid")).toBe("orphan");
    expect(familyRoot(tree, "orphan")).toBe("orphan");
  });
});

describe("what a parent's row shows", () => {
  const states: Record<string, ChildState> = { w: "working", i: "idle", n: "waiting", e: "error", x: "exited", d: "done" };
  const stateOf = (s: SessionInfo) => states[s.id.slice(0, 1)];

  it("counts everything below and takes the worst state: waiting, then failed, working, finished, idle", () => {
    const tree = buildSessionTree([session("root"), session("w1", { parent: "root" }), session("i1", { parent: "root" }), session("n1", { parent: "w1" })]);
    expect(subtreeSummary(tree, "root", stateOf)).toEqual({ count: 3, worst: "waiting" });
    expect(subtreeSummary(tree, "w1", stateOf)).toEqual({ count: 1, worst: "waiting" });
    expect(subtreeSummary(tree, "n1", stateOf)).toEqual({ count: 0, worst: "exited" });
    const rank = (letters: string) => {
      const kids = letters.split("").map((c, i) => session(`${c}${i}`, { parent: "root" }));
      return subtreeSummary(buildSessionTree([session("root"), ...kids]), "root", stateOf).worst;
    };
    expect(rank("iw")).toBe("working");
    expect(rank("iwe")).toBe("error");
    expect(rank("id")).toBe("done");
    expect(rank("xi")).toBe("idle");
    expect(rank("x")).toBe("exited");
    expect(rank("iwen")).toBe("waiting");
  });
});

describe("Close finished sub-sessions", () => {
  const report = (status: string) => ({ status, summary: "s", at: 1 });

  it("picks the ones that stopped or idle after saying done or exited, and nothing else", () => {
    const kids = [
      session("done-idle", { status: "idle", report: report("done") }),
      session("done-exited", { alive: false, status: "exited", report: report("done") }),
      session("exited", { alive: false, status: "exited", report: { ...report("exited"), auto: true } }),
      session("still-working", { status: "working", report: report("done") }),
      session("waiting", { status: "waiting", report: report("done") }),
      session("failed", { status: "idle", report: report("failed") }),
      session("went-quiet", { status: "idle", report: { ...report("idle"), auto: true } }),
      session("no-report", { status: "idle" }),
      session("exited-no-report", { alive: false, status: "exited" }),
    ];
    expect(ids(finishedChildren(kids))).toEqual(["done-idle", "done-exited", "exited"]);
    expect(finishedChildren([])).toEqual([]);
  });

  it("leaves out a child that still has a running sub-session below it", () => {
    const done = { status: "done", summary: "s", at: 1 };
    const list = [
      session("root"),
      session("busy-parent", { parent: "root", status: "idle", report: done }),
      session("its-worker", { parent: "busy-parent", status: "working" }),
      session("clean-parent", { parent: "root", status: "idle", report: done }),
      session("its-dead-worker", { parent: "clean-parent", alive: false, status: "exited" }),
    ];
    const tree = buildSessionTree(list);
    expect(hasLiveDescendant(tree, "busy-parent")).toBe(true);
    expect(hasLiveDescendant(tree, "clean-parent")).toBe(false);
    expect(ids(finishedChildren(childrenOfSession(tree, "root"), tree))).toEqual(["clean-parent"]);
    // Without the tree only the child itself is looked at.
    expect(ids(finishedChildren(childrenOfSession(tree, "root")))).toEqual(["busy-parent", "clean-parent"]);
  });
});

describe("requests waiting for the user", () => {
  const request = (id: string, at = 1, extra: Partial<SubRequest> = {}): SubRequest => ({
    id, session: "s1", from_label: "x", agent: "codex", cwd: "/w", worktree: false, name: null, task: "t", model: null, skips_prompts: false, at, ...extra,
  });

  it("keeps requests oldest first, once each, and drops one that is done", () => {
    let state = reduceRequests(emptyRequests, { type: "request", request: request("b", 2) });
    state = reduceRequests(state, { type: "request", request: request("a", 1) });
    expect(state.list.map((r) => r.id)).toEqual(["a", "b"]);
    expect(reduceRequests(state, { type: "request", request: request("a", 1) })).toBe(state);
    const after = reduceRequests(state, { type: "done", id: "a" });
    expect(after.list.map((r) => r.id)).toEqual(["b"]);
    expect(reduceRequests(after, { type: "done", id: "a" })).toBe(after);
  });

  it("does not bring back a request that was answered before a late list arrives", () => {
    let state = reduceRequests(emptyRequests, { type: "request", request: request("a", 1) });
    state = reduceRequests(state, { type: "done", id: "a" });
    // The event for it arrives again, and so does a list read from the daemon just before the answer.
    expect(reduceRequests(state, { type: "request", request: request("a", 1) })).toBe(state);
    const replaced = reduceRequests(state, { type: "replace", list: [request("a", 1), request("c", 3)] });
    expect(replaced.list.map((r) => r.id)).toEqual(["c"]);
  });

  it("a list from the daemon replaces what the window had: what is gone from it was answered or timed out", () => {
    let state = reduceRequests(emptyRequests, { type: "request", request: request("old", 1) });
    state = reduceRequests(state, { type: "replace", list: [request("new", 2)] });
    expect(state.list.map((r) => r.id)).toEqual(["new"]);
    expect(reduceRequests(state, { type: "replace", list: [request("new", 2)] })).toBe(state);
    expect(reduceRequests(state, { type: "replace", list: [] }).list).toEqual([]);
  });

  it("keeps a request that is newer than the list the daemon sent, and drops older ones it does not list", () => {
    let state = reduceRequests(emptyRequests, { type: "request", request: request("older", 10) });
    state = reduceRequests(state, { type: "request", request: request("newer", 200) });
    const next = reduceRequests(state, { type: "replace", list: [request("listed", 50)], keepFrom: 100 });
    expect(next.list.map((r) => r.id)).toEqual(["listed", "newer"]);
  });

  it("Later hides a card without closing the request", () => {
    dispatchRequest({ type: "request", request: request("l1", 1) });
    dispatchRequest({ type: "request", request: request("l2", 2) });
    postponeRequest("l1");
    expect(shownRequests().map((r) => r.id)).toEqual(["l2"]);
    expect(subRequests().map((r) => r.id)).toEqual(["l1", "l2"]);
    showPostponed();
    expect(shownRequests().map((r) => r.id)).toEqual(["l1", "l2"]);
    dispatchRequest({ type: "done", id: "l1" });
    dispatchRequest({ type: "done", id: "l2" });
  });

  it("the shared queue follows the events", () => {
    expect(dispatchRequest({ type: "request", request: request("q1", 5) })).toBe(true);
    expect(dispatchRequest({ type: "request", request: request("q1", 5) })).toBe(false);
    expect(subRequests().map((r) => r.id)).toEqual(["q1"]);
    expect(dispatchRequest({ type: "done", id: "q1" })).toBe(true);
    expect(subRequests()).toEqual([]);
  });

  it("reads what the daemon sends and refuses what has no id", () => {
    const wire = { id: "r1", session: "s9", from_label: "session \"main\"", agent: "codex", cwd: "C:/w", worktree: true, name: "fix #1", task: "do it", model: "gpt", skips_prompts: true, at: 7 };
    expect(toSubRequest(wire)).toEqual({ ...wire, session: "s9", from_label: "session \"main\"", name: "fix #1" });
    expect(toSubRequest({ id: "r2" })).toEqual({ id: "r2", session: "", from_label: "", agent: "", cwd: "", worktree: false, name: null, task: "", model: null, skips_prompts: false, at: 0 });
    expect(toSubRequest({ id: "r3", from: "s2" })?.session).toBe("s2");
    for (const bad of [null, undefined, 5, "x", {}, { id: "" }, { id: 4 }]) expect(toSubRequest(bad), String(bad)).toBeNull();
    expect(toSubRequest({ id: "r4", task: "x".repeat(10_000) })?.task).toHaveLength(4000);
  });
});

describe("policy and small helpers", () => {
  it("reads the policy keys and ignores a reply without them (an older daemon)", () => {
    expect(toSubPolicy({ agents_control_others: true, sub_sessions: "ask", sub_max: 0, sub_auto_max: 2 })).toEqual({ sub_sessions: "ask", sub_max: 0, sub_auto_max: 2 });
    expect(toSubPolicy({ sub_sessions: "allow", sub_max: 3.4, sub_auto_max: 4 })).toEqual({ sub_sessions: "allow", sub_max: 3, sub_auto_max: 4 });
    for (const bad of [null, {}, { agents_control_others: true }, { sub_sessions: "yes" }, { sub_sessions: 1 }]) expect(toSubPolicy(bad)).toBeNull();
  });

  it("takes a maximum of 1 to 16 and nothing else", () => {
    expect([" 3 ", "1", "16"].map(parseSubMax)).toEqual([3, 1, 16]);
    for (const bad of ["0", "17", "-1", "2.5", "", "abc", "1e1"]) expect(parseSubMax(bad), bad).toBeNull();
  });

  it("shortens a folder to its last part, on either kind of path", () => {
    expect(folderName("C:\\Works\\Kelpie\\app")).toBe("app");
    expect(folderName("/home/me/proj/")).toBe("proj");
    expect(folderName("/")).toBe("/");
    expect(folderName("repo")).toBe("repo");
  });
});

describe("a quiet child's state with its report", () => {
  it("counts needs_user and blocked as waiting and failed as an error, only while it is quiet", () => {
    for (const quiet of ["idle", "done"] as const) {
      expect(stateWithReport(quiet, "needs_user")).toBe("waiting");
      expect(stateWithReport(quiet, "blocked")).toBe("waiting");
      expect(stateWithReport(quiet, "failed")).toBe("error");
      expect(stateWithReport(quiet, "done")).toBe(quiet);
      expect(stateWithReport(quiet, undefined)).toBe(quiet);
    }
    for (const live of ["working", "waiting", "error", "exited"] as const) expect(stateWithReport(live, "needs_user")).toBe(live);
  });
});
