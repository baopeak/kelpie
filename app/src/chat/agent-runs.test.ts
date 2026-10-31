import { describe, expect, it } from "vitest";
import { createMemo, createRoot } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { type AgentRun, applyRunResult, applyRunTaskEvent, applySubagentMessage, formatDuration, formatTokens, mergeRuns, runDuration, runResult, runTree, startRun } from "./agent-runs";
import { createClaudeAdapter } from "./claude-adapter";
import { type AdapterContext, ChatController, type ChatItem, type ChatState, emptyChatState } from "./chat-model";

// Recorded from claude 2.1.314 with two parallel agents, one of them nesting another, and one in the
// background: paths, session ids and long tool results scrubbed, the ids shortened.
const STREAM: any[] = [
  {"type": "assistant", "message": {"id": "msg_0", "content": [{"type": "tool_use", "id": "toolu_notes", "name": "Agent", "input": {"description": "Read notes", "subagent_type": "general-purpose", "prompt": "Read notes.txt with the Read tool and reply with its first word."}}]}, "parent_tool_use_id": null},
  {"type": "assistant", "message": {"id": "msg_1", "content": [{"type": "tool_use", "id": "toolu_outer", "name": "Agent", "input": {"description": "Nested helper", "subagent_type": "general-purpose", "prompt": "Use the Agent tool once with model haiku, description Inner helper, subagent_type general-purpose, prompt: Reply with the word inner. Then reply with what it said."}}]}, "parent_tool_use_id": null},
  {"type": "system", "subtype": "task_started", "task_id": "task_notes", "tool_use_id": "toolu_notes", "description": "Read notes", "subagent_type": "general-purpose", "is_backgrounded": true, "spawn_depth": 1, "task_type": "local_agent", "prompt": "Read notes.txt with the Read tool and reply with its first word."},
  {"type": "system", "subtype": "task_started", "task_id": "task_outer", "tool_use_id": "toolu_outer", "description": "Nested helper", "subagent_type": "general-purpose", "is_backgrounded": true, "spawn_depth": 1, "task_type": "local_agent", "prompt": "Use the Agent tool once with model haiku, description Inner helper, subagent_type general-purpose, prompt: Reply with the word inner. Then reply with what it said."},
  {"type": "user", "message": {"content": [{"tool_use_id": "toolu_notes", "type": "tool_result", "content": [{"type": "text", "text": "Async agent launched successfully."}]}]}, "parent_tool_use_id": null, "tool_use_result": {"isAsync": true, "status": "async_launched", "agentId": "task_notes", "description": "Read notes", "prompt": "Read notes.txt with the Read tool and reply with its first word."}},
  {"type": "user", "message": {"content": [{"tool_use_id": "toolu_outer", "type": "tool_result", "content": [{"type": "text", "text": "Async agent launched successfully."}]}]}, "parent_tool_use_id": null, "tool_use_result": {"isAsync": true, "status": "async_launched", "agentId": "task_outer", "description": "Nested helper", "prompt": "Use the Agent tool once with model haiku, description Inner helper, subagent_type general-purpose, prompt: Reply with the word inner. Then reply with what it said."}},
  {"type": "assistant", "message": {"id": "msg_6", "content": [{"type": "tool_use", "id": "toolu_background", "name": "Agent", "input": {"description": "Background count", "subagent_type": "general-purpose", "run_in_background": true, "prompt": "Reply with the word done."}}]}, "parent_tool_use_id": null},
  {"type": "assistant", "message": {"id": "msg_7", "content": [{"type": "tool_use", "id": "toolu_read", "name": "Read", "input": {"file_path": "C:/work/notes.txt"}}]}, "parent_tool_use_id": "toolu_notes", "agent_id": "task_notes", "subagent_type": "general-purpose", "task_description": "Read notes"},
  {"type": "system", "subtype": "task_progress", "task_id": "task_notes", "tool_use_id": "toolu_notes", "description": "Reading notes.txt", "subagent_type": "general-purpose", "usage": {"total_tokens": 43630, "tool_uses": 1, "duration_ms": 8787}, "last_tool_name": "Read"},
  {"type": "system", "subtype": "task_started", "task_id": "task_background", "tool_use_id": "toolu_background", "description": "Background count", "subagent_type": "general-purpose", "is_backgrounded": true, "spawn_depth": 1, "task_type": "local_agent", "prompt": "Reply with the word done."},
  {"type": "assistant", "message": {"id": "msg_10", "content": [{"type": "tool_use", "id": "toolu_inner", "name": "Agent", "input": {"description": "Inner helper", "prompt": "Reply with the word inner. Then reply with what it said.", "subagent_type": "general-purpose", "run_in_background": false}}]}, "parent_tool_use_id": "toolu_outer", "agent_id": "task_outer", "subagent_type": "general-purpose", "task_description": "Nested helper"},
  {"type": "system", "subtype": "task_progress", "task_id": "task_outer", "tool_use_id": "toolu_outer", "description": "Inner helper", "subagent_type": "general-purpose", "usage": {"total_tokens": 43738, "tool_uses": 1, "duration_ms": 8913}, "last_tool_name": "Agent"},
  {"type": "system", "subtype": "task_started", "task_id": "task_inner", "tool_use_id": "toolu_inner", "description": "Inner helper", "subagent_type": "general-purpose", "is_backgrounded": false, "spawn_depth": 2, "parent_task_id": "task_outer", "task_type": "local_agent", "prompt": "Reply with the word inner. Then reply with what it said."},
  {"type": "user", "message": {"content": [{"tool_use_id": "toolu_background", "type": "tool_result", "content": [{"type": "text", "text": "Async agent launched successfully."}]}]}, "parent_tool_use_id": null, "tool_use_result": {"isAsync": true, "status": "async_launched", "agentId": "task_background", "description": "Background count", "prompt": "Reply with the word done."}},
  {"type": "user", "message": {"content": [{"tool_use_id": "toolu_read", "type": "tool_result", "content": "1\talpha\n2\tbeta\n3\t"}]}, "parent_tool_use_id": "toolu_notes", "agent_id": "task_notes", "subagent_type": "general-purpose", "task_description": "Read notes"},
  {"type": "assistant", "message": {"id": "msg_15", "content": [{"type": "text", "text": "done"}]}, "parent_tool_use_id": "toolu_background", "agent_id": "task_background", "subagent_type": "general-purpose", "task_description": "Background count"},
  {"type": "assistant", "message": {"id": "msg_16", "content": [{"type": "text", "text": "alpha"}]}, "parent_tool_use_id": "toolu_notes", "agent_id": "task_notes", "subagent_type": "general-purpose", "task_description": "Read notes"},
  {"type": "system", "subtype": "task_updated", "task_id": "task_background", "patch": {"status": "completed", "end_time": 1793380950260}},
  {"type": "system", "subtype": "task_notification", "task_id": "task_background", "tool_use_id": "toolu_background", "status": "completed", "summary": "done", "usage": {"total_tokens": 43377, "tool_uses": 0, "duration_ms": 11536}},
  {"type": "system", "subtype": "task_updated", "task_id": "task_notes", "patch": {"status": "completed", "end_time": 1793380950693}},
  {"type": "system", "subtype": "task_notification", "task_id": "task_notes", "tool_use_id": "toolu_notes", "status": "completed", "summary": "alpha", "usage": {"total_tokens": 52583, "tool_uses": 1, "duration_ms": 21619}},
  {"type": "system", "subtype": "task_updated", "task_id": "task_inner", "patch": {"status": "completed", "end_time": 1793380951163}},
  {"type": "system", "subtype": "task_notification", "task_id": "task_inner", "tool_use_id": "toolu_inner", "status": "completed", "summary": "inner\n\nThe word I replied with was \"inner.\"", "usage": {"total_tokens": 44935, "tool_uses": 0, "duration_ms": 9563}},
  {"type": "user", "message": {"content": [{"tool_use_id": "toolu_inner", "type": "tool_result", "content": [{"type": "text", "text": "[Subagent hand-back] inner"}]}]}, "parent_tool_use_id": "toolu_outer", "agent_id": "task_outer", "subagent_type": "general-purpose", "task_description": "Nested helper"},
  {"type": "assistant", "message": {"id": "msg_24", "content": [{"type": "text", "text": "The subagent replied \"inner\" and reported that the word it replied with was \"inner.\""}]}, "parent_tool_use_id": "toolu_outer", "agent_id": "task_outer", "subagent_type": "general-purpose", "task_description": "Nested helper"},
  {"type": "system", "subtype": "task_updated", "task_id": "task_outer", "patch": {"status": "completed", "end_time": 1793380962079}},
  {"type": "system", "subtype": "task_notification", "task_id": "task_outer", "tool_use_id": "toolu_outer", "status": "completed", "summary": "The subagent replied \"inner\" and reported that the word it replied with was \"inner.\"", "usage": {"total_tokens": 52831, "tool_uses": 1, "duration_ms": 31914}},
];

/** An adapter context over a real store, as in the adapter's own tests, with a clock to set. */
function harness() {
  const [state, set] = createStore<ChatState>(emptyChatState());
  const written: unknown[] = [];
  const clock = { at: 1000 as number | null };
  const ctx: AdapterContext = {
    state, set,
    write: (obj) => written.push(obj),
    upsert: (item) => {
      const i = state.items.findIndex((x) => x.id === item.id);
      set("items", i < 0 ? state.items.length : i, item);
    },
    patch: (id, fn) => {
      const i = state.items.findIndex((x) => x.id === id);
      if (i >= 0) set("items", i, produce(fn as never));
    },
    find: (id) => state.items.find((x) => x.id === id),
    clear: () => set({ items: [], plan: null, usage: null }),
    setConversation: (id) => set("conversationId", id),
    setModel: (m) => set("model", m),
    now: () => clock.at,
    cwd: "/work", resumeId: null, replaying: () => false,
  };
  const adapter = createClaudeAdapter(ctx);
  return { state, adapter, written, clock, feed: (lines: any[]) => lines.forEach((m) => adapter.onMessage(m, "")), runs: () => JSON.parse(JSON.stringify(state.agentRuns)) as AgentRun[] };
}

/** One run for tests about what surrounds it. */
const run = (id: string, extra: Partial<AgentRun> = {}): AgentRun => ({ id, description: id, status: "done", activity: [], ...extra });
const shape = (nodes: ReturnType<typeof runTree>): unknown[] => nodes.map((n) => (n.children.length ? [n.run.id, shape(n.children)] : n.run.id));

const started = (task: string, tool: string, extra: object = {}) => ({ type: "system", subtype: "task_started", task_id: task, tool_use_id: tool, description: `Name of ${task}`, subagent_type: "general-purpose", task_type: "local_agent", prompt: "Do it", ...extra });
const progress = (task: string, tool: string, extra: object = {}) => ({ type: "system", subtype: "task_progress", task_id: task, tool_use_id: tool, description: "Reading a.ts", subagent_type: "general-purpose", usage: { total_tokens: 10, tool_uses: 1, duration_ms: 500 }, ...extra });
const notification = (task: string, tool: string, extra: object = {}) => ({ type: "system", subtype: "task_notification", task_id: task, tool_use_id: tool, status: "completed", summary: "The answer", usage: { total_tokens: 90900, tool_uses: 3, duration_ms: 229000 }, ...extra });
const agentCall = (id: string, parent: string | null = null) => ({ type: "assistant", parent_tool_use_id: parent, message: { id: `m-${id}`, content: [{ type: "tool_use", id, name: "Agent", input: { description: `Call ${id}`, subagent_type: "explore", prompt: `Prompt of ${id}` } }] } });

describe("the recorded stream", () => {
  it("rebuilds the tree, with each agent's final count and answer", () => {
    const h = harness();
    h.feed(STREAM);
    expect(shape(runTree(h.state.agentRuns))).toEqual(["toolu_background", ["toolu_outer", ["toolu_inner"]], "toolu_notes"]);
    const byName = Object.fromEntries(h.runs().map((r) => [r.description, r]));
    expect(byName["Read notes"]).toMatchObject({ status: "done", type: "general-purpose", tokens: 52583, toolUses: 1, durationMs: 21619, result: "alpha", prompt: "Read notes.txt with the Read tool and reply with its first word." });
    expect(byName["Read notes"].activity).toEqual([{ id: "toolu_read", name: "Read", target: "notes.txt", status: "done" }]);
    expect(byName["Inner helper"]).toMatchObject({ parent: "toolu_outer", parentTask: "task_outer", tokens: 44935, toolUses: 0 });
    expect(byName["Inner helper"].result).toContain("inner");
    expect(byName["Nested helper"].activity).toEqual([{ id: "toolu_inner", name: "Agent", target: "Inner helper", status: "done" }]);
  });

  it("keeps an agent running after its tool call returned: the launch says nothing about it", () => {
    const h = harness();
    const launched = STREAM.findIndex((m) => m.tool_use_result?.status === "async_launched");
    h.feed(STREAM.slice(0, launched + 1));
    const notes = h.runs().find((r) => r.id === "toolu_notes")!;
    expect(notes.status).toBe("running");
    expect(h.state.items.find((i) => i.id === "toolu_notes")).toMatchObject({ kind: "tool", status: "done" });
    // Nor does the merged list go by the row.
    expect(mergeRuns(h.state.agentRuns, h.state.items).find((r) => r.id === "toolu_notes")?.status).toBe("running");
  });

  it("names an agent by its start and shows its progress as what it is doing now", () => {
    const h = harness();
    h.feed(STREAM.slice(0, STREAM.findIndex((m) => m.subtype === "task_progress") + 1));
    expect(h.runs().find((r) => r.id === "toolu_notes")).toMatchObject({ description: "Read notes", summary: "Reading notes.txt", tokens: 43630 });
  });

  it("builds the same map again from the same lines, and never writes to the agent", () => {
    const a = harness();
    a.feed(STREAM);
    const b = harness();
    b.feed(STREAM);
    expect(b.runs()).toEqual(a.runs());
    expect(a.written).toEqual([]);
  });
});

describe("events out of order", () => {
  it("takes progress before the tool call, and the call then names the run", () => {
    const h = harness();
    h.feed([progress("t1", "c1")]);
    expect(h.runs()).toMatchObject([{ id: "c1", taskId: "t1", description: "", type: "general-purpose", status: "running", tokens: 10, summary: "Reading a.ts" }]);
    h.feed([agentCall("c1")]);
    expect(h.runs()).toMatchObject([{ id: "c1", description: "Call c1", type: "explore", prompt: "Prompt of c1", summary: "Reading a.ts" }]);
  });

  it("ignores a notification for a task it knows nothing of: a shell's looks the same", () => {
    const h = harness();
    h.feed([notification("nobody", "c9")]);
    expect(h.runs()).toEqual([]);
  });

  it("does not repeat a run on a duplicate start", () => {
    const h = harness();
    h.feed([started("t1", "c1"), started("t1", "c1")]);
    expect(h.runs()).toHaveLength(1);
    expect(h.runs()[0].status).toBe("running");
  });

  it("takes the name from progress only when it also says what the agent is doing", () => {
    const list: AgentRun[] = [];
    applyRunTaskEvent(list, progress("t1", "c1", { description: "Old style name", summary: "Compiling" }));
    expect(list[0]).toMatchObject({ description: "Old style name", summary: "Compiling" });
  });

  it("finds a run by its tool call when the start that linked its task is gone", () => {
    const h = harness();
    h.feed([agentCall("c1"), notification("t1", "c1")]);
    expect(h.runs()).toMatchObject([{ id: "c1", taskId: "t1", status: "done", tokens: 90900 }]);
  });

  it("keeps the final count against progress that comes after the notification", () => {
    const h = harness();
    h.feed([started("t1", "c1"), notification("t1", "c1"), progress("t1", "c1")]);
    expect(h.runs()[0]).toMatchObject({ tokens: 90900, status: "done" });
  });

  it("lets the agent's last word stand over the stop its exit caused", () => {
    const list = [run("c1", { taskId: "t1", status: "stopped", endedAt: 5 })];
    applyRunTaskEvent(list, notification("t1", "c1"), 9);
    expect(list[0]).toMatchObject({ status: "done", endedAt: 5, result: "The answer" });
  });
});

describe("a continued agent", () => {
  it("runs again on a new start under the same id, without the old end or answer showing", () => {
    const list: AgentRun[] = [];
    applyRunTaskEvent(list, started("t1", "c1"), 1000);
    applyRunTaskEvent(list, notification("t1", "c1", { status: "failed" }), 2000);
    applyRunTaskEvent(list, started("t1", "c1"), 5000);
    expect(list).toHaveLength(1);
    expect(list[0]).toMatchObject({ status: "running", at: 5000 });
    expect(list[0].endedAt).toBeUndefined();
    expect(list[0].durationMs).toBeUndefined();
    expect(runResult(list[0])).toBeUndefined();
    expect(runDuration(list[0], 6000)).toBe(1000);
    applyRunTaskEvent(list, notification("t1", "c1", { summary: "Second answer" }), 7000);
    expect(list[0]).toMatchObject({ status: "done", result: "Second answer", endedAt: 7000 });
  });
});

describe("what is not an agent", () => {
  it("leaves out background shells", () => {
    const h = harness();
    h.feed([started("b1", "c1", { task_type: "local_bash", subagent_type: undefined }), { type: "system", subtype: "task_progress", task_id: "b1", tool_use_id: "c1", description: "Run b1", usage: { total_tokens: 0, tool_uses: 1, duration_ms: 10 }, summary: "Compiling" }, notification("b1", "c1")]);
    expect(h.runs()).toEqual([]);
  });

  it("leaves out housekeeping tasks and tasks with no tool call to show them under", () => {
    const h = harness();
    h.feed([started("h1", "c1", { skip_transcript: true }), started("h2", "c2", { ambient: true }), started("h3", "")]);
    expect(h.runs()).toEqual([]);
  });

  it("leaves the transcript alone: a subagent's lines add no rows", () => {
    const h = harness();
    h.feed(STREAM.filter((m) => m.parent_tool_use_id));
    expect(h.state.items).toEqual([]);
  });
});

describe("an older stream", () => {
  it("lists a call with no task events, its numbers and status from the tool's result", () => {
    const h = harness();
    h.feed([agentCall("c1"), { type: "user", parent_tool_use_id: null, message: { content: [{ type: "tool_result", tool_use_id: "c1", content: "Found it" }] }, tool_use_result: { status: "completed", totalTokens: 1200, totalDurationMs: 3000, totalToolUseCount: 4 } }]);
    expect(h.runs()).toMatchObject([{ id: "c1", status: "done", tokens: 1200, durationMs: 3000, toolUses: 4 }]);
    expect(runResult(h.runs()[0], "Found it")).toBe("Found it");
  });

  it("links a task with no type to the call it names, and shows nothing for a missing number", () => {
    const h = harness();
    h.feed([agentCall("c1"), started("t1", "c1", { task_type: undefined }), { type: "system", subtype: "task_progress", task_id: "t1", summary: "Thinking", usage: "soon" }, notification("t1", "c1", { usage: undefined })]);
    const [one] = h.runs();
    expect(one).toMatchObject({ taskId: "t1", status: "done", summary: "Thinking" });
    expect(one.tokens).toBeUndefined();
    expect(one.toolUses).toBeUndefined();
    expect(one.durationMs).toBeUndefined();
  });

  it("ends a run on a task update when no notification follows", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunTaskEvent(list, { subtype: "task_updated", task_id: "t1", patch: { status: "paused" } }, 5);
    expect(list[0].status).toBe("running");
    applyRunTaskEvent(list, { subtype: "task_updated", task_id: "t1", patch: { status: "killed" } }, 5);
    expect(list[0]).toMatchObject({ status: "stopped", endedAt: 5 });
  });

  it("reads a tool error as the end of a run, whatever the task events said", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunResult(list, { tool_use_id: "c1", is_error: true }, undefined, 7);
    expect(list[0]).toMatchObject({ status: "error", endedAt: 7 });
  });

  it("ends a run whose result has totals and no status", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunResult(list, { tool_use_id: "c1" }, { message: { content: [{}] }, tool_use_result: { totalTokens: 5 } }, 7);
    expect(list[0]).toMatchObject({ status: "done", tokens: 5 });
  });

  it("does not take a nested call's plain result for the end of a linked run", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunResult(list, { tool_use_id: "c1" }, undefined, 7);
    expect(list[0].status).toBe("running");
  });
});

describe("statuses", () => {
  it.each([["completed", "done"], ["failed", "error"], ["stopped", "stopped"], ["killed", "stopped"]])("maps %s", (status, expected) => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunTaskEvent(list, notification("t1", "c1", { status }));
    expect(list[0].status).toBe(expected);
    expect(list[0].statusText).toBeUndefined();
  });

  const launched = (status: string) => ({ message: { content: [{}] }, tool_use_result: { status } });

  it.each(["async_launched", "remote_launched", "teammate_spawned", "forked"])("leaves a run going on the launch status %s", (status) => {
    for (const list of [[run("c1", { status: "running" })], [run("c1", { taskId: "t1", status: "running" })]]) {
      applyRunResult(list, { tool_use_id: "c1" }, launched(status), 7);
      expect(list[0].status).toBe("running");
      expect(list[0].statusText).toBeUndefined();
      expect(list[0].endedAt).toBeUndefined();
    }
  });

  it("ends a run from a tool result only on a status that means an end", () => {
    const list = [run("c1", { status: "running" })];
    applyRunResult(list, { tool_use_id: "c1" }, launched("failed"), 7);
    expect(list[0]).toMatchObject({ status: "error", endedAt: 7 });
  });

  it("does not stop a live run on a task update it cannot read", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunTaskEvent(list, { subtype: "task_updated", task_id: "t1", patch: { status: "forked" } }, 5);
    expect(list[0].status).toBe("running");
    expect(list[0].statusText).toBeUndefined();
  });

  it("reads the launch of a nested call by the same rule, from its own line", () => {
    const list: AgentRun[] = [run("p", { status: "running" })];
    const nested = (extra: object) => ({ parent_tool_use_id: "p", message: { content: [{ type: "tool_result", tool_use_id: "n", content: "x" }] }, ...extra });
    applySubagentMessage(list, { parent_tool_use_id: "p", message: { content: [{ type: "tool_use", id: "n", name: "Agent", input: { description: "Inner" } }] } });
    applySubagentMessage(list, nested({ tool_use_result: { isAsync: true, status: "async_launched" } }));
    expect(list[1].status).toBe("running");
    applySubagentMessage(list, nested({ tool_use_result: { status: "completed", totalTokens: 9 } }));
    expect(list[1]).toMatchObject({ status: "done", tokens: 9 });
  });

  it("keeps a status it does not know as its text", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunTaskEvent(list, notification("t1", "c1", { status: "rate_limited" }));
    expect(list[0]).toMatchObject({ status: "stopped", statusText: "rate_limited" });
  });

  it("is not fooled by a status named like an object's own property", () => {
    const list = [run("c1", { taskId: "t1", status: "running" })];
    applyRunTaskEvent(list, notification("t1", "c1", { status: "constructor" }));
    expect(list[0]).toMatchObject({ status: "stopped", statusText: "constructor" });
  });
});

describe("nesting", () => {
  it("puts a child under its parent, by the call or by the task", () => {
    const list: AgentRun[] = [];
    startRun(list, { id: "p" });
    startRun(list, { id: "by-call" }, "p");
    applyRunTaskEvent(list, started("tp", "p"));
    applyRunTaskEvent(list, started("tc", "by-task", { parent_task_id: "tp" }));
    expect(shape(runTree(list))).toEqual([["p", ["by-task", "by-call"]]]);
  });

  it("shows a child whose parent is not known yet at the top, then moves it under", () => {
    const list: AgentRun[] = [];
    applyRunTaskEvent(list, started("tc", "child", { parent_task_id: "tp" }));
    expect(shape(runTree(list))).toEqual(["child"]);
    applyRunTaskEvent(list, started("tp", "parent"));
    expect(shape(runTree(list))).toEqual([["parent", ["child"]]]);
  });

  it("survives a cycle and a self-parent, keeping every run", () => {
    const self = runTree([run("a", { parent: "a" })]);
    expect(shape(self)).toEqual(["a"]);
    const cycle = runTree([run("a", { parent: "b", at: 1 }), run("b", { parent: "a", at: 2 }), run("c", { parent: "a" })]);
    const ids = JSON.stringify(shape(cycle));
    for (const id of ["a", "b", "c"]) expect(ids.split(`"${id}"`)).toHaveLength(2);
  });
});

describe("activity", () => {
  const call = (id: string) => ({ parent_tool_use_id: "p", message: { content: [{ type: "tool_use", id, name: "Bash", input: { command: `echo ${id}` } }] } });

  it("keeps the last 200 calls of an agent, the oldest dropped", () => {
    const list: AgentRun[] = [run("p", { status: "running" })];
    for (let i = 0; i < 205; i++) applySubagentMessage(list, call(`c${i}`));
    expect(list[0].activity).toHaveLength(200);
    expect(list[0].activity[0].id).toBe("c5");
    expect(list[0].activity[199].id).toBe("c204");
  });

  it("marks a call done or failed by its result, and stores a short target and no output", () => {
    const list: AgentRun[] = [run("p", { status: "running" })];
    applySubagentMessage(list, { parent_tool_use_id: "p", message: { content: [{ type: "tool_use", id: "c", name: "Bash", input: { command: "x".repeat(5000) } }, { type: "tool_use", id: "d", name: "Bash", input: { command: "y" } }] } });
    applySubagentMessage(list, { parent_tool_use_id: "p", message: { content: [{ type: "tool_result", tool_use_id: "c", content: "o".repeat(5000), is_error: true }, { type: "tool_result", tool_use_id: "d", content: "fine" }] } });
    expect(list[0].activity.map((a) => a.status)).toEqual(["error", "done"]);
    expect(list[0].activity[0].target.length).toBeLessThanOrEqual(121);
    expect(JSON.stringify(list[0])).not.toContain("ooo");
  });

  it("does not list a call twice when its message comes again", () => {
    const list: AgentRun[] = [run("p", { status: "running" })];
    applySubagentMessage(list, call("c"));
    applySubagentMessage(list, call("c"));
    expect(list[0].activity).toHaveLength(1);
  });

  it("brings in the agent of a line whose start the log lost", () => {
    const list: AgentRun[] = [];
    applySubagentMessage(list, { parent_tool_use_id: "p", task_description: "Lost", subagent_type: "explore", message: { content: [{ type: "text", text: "hm" }] } }, 3);
    expect(list).toMatchObject([{ id: "p", description: "Lost", type: "explore", status: "running", at: 3 }]);
  });

  it("names a run that came back with no name by its next line", () => {
    const list: AgentRun[] = [];
    applyRunTaskEvent(list, progress("t1", "p"));
    expect(list[0].description).toBe("");
    applySubagentMessage(list, { parent_tool_use_id: "p", task_description: "Found again", message: { content: [] } });
    expect(list[0].description).toBe("Found again");
  });

  it("copies neither the list of runs nor a run's calls per message", () => {
    const h = harness();
    h.feed([agentCall("p")]);
    const before = h.state.agentRuns;
    const calls = h.state.agentRuns[0].activity;
    h.feed([{ type: "assistant", ...call("c1") }]);
    expect(h.state.agentRuns).toBe(before);
    expect(h.state.agentRuns[0].activity).toBe(calls);
  });
});

describe("mergeRuns", () => {
  const tool = (id: string, extra: object = {}): ChatItem => ({ kind: "tool", id, name: "Spawn", summary: `Summary of ${id}`, status: "running", category: "agent", ...extra });

  it("adds a tool item of another agent, with no tokens", () => {
    const [one] = mergeRuns([], [tool("x", { at: 5, status: "done", input: { prompt: "Go" } }), tool("y", { category: "command" })], true);
    expect(one).toMatchObject({ id: "x", description: "Summary of x", status: "done", prompt: "Go", at: 5 });
    expect(one.tokens).toBeUndefined();
  });

  it("adds a conversation's history by its Agent calls, and only those", () => {
    const merged = mergeRuns([], [{ kind: "tool", id: "a", name: "Agent", summary: "", status: "done", input: { description: "Look", subagent_type: "explore" } }, { kind: "tool", id: "b", name: "Read", summary: "x", status: "done" }, { kind: "tool", id: "c", name: "TaskOutput", summary: "", status: "done" }]);
    expect(merged).toMatchObject([{ id: "a", description: "Look", type: "explore" }]);
  });

  it("does not repeat a run it has, and keeps the run's status over the row's", () => {
    const runs = [run("a", { status: "running" })];
    expect(mergeRuns(runs, [tool("a", { status: "done" })])).toBe(runs);
  });

  it("counts a row as running only while the chat is busy", () => {
    expect(mergeRuns([], [tool("x", { at: 5 })], true)[0].status).toBe("running");
    const idle = mergeRuns([], [tool("x", { at: 5 })], false)[0];
    expect(idle.status).toBe("stopped");
    // A day-old row with no result has no counter to show.
    expect(runDuration(idle, 86_400_000)).toBeUndefined();
  });

  it("shows no time for a running row with no known start", () => {
    expect(runDuration(mergeRuns([], [tool("x")], true)[0], 9000)).toBeUndefined();
  });

  it("reuses the objects of rows that did not change, so their cards stay", () => {
    const first = mergeRuns([], [tool("x")], true);
    const same = mergeRuns([], [tool("x"), { kind: "tool", id: "r", name: "Read", summary: "", status: "done" }], true, first);
    expect(same).toBe(first);
    const more = mergeRuns([], [tool("x"), tool("y")], true, first);
    expect(more[0]).toBe(first[0]);
    expect(more).toHaveLength(2);
    const changed = mergeRuns([], [tool("x", { status: "done" }), tool("y")], true, more);
    expect(changed[0]).not.toBe(more[0]);
    expect(changed[0].status).toBe("done");
    expect(changed[1]).toBe(more[1]);
  });

  describe("in a memo over a store", () => {
    const setup = (rows: ChatItem[]) => {
      const [state, set] = createStore({ items: rows });
      let computed = 0;
      const { merged, dispose } = createRoot((dispose) => ({
        dispose,
        merged: createMemo(() => {
          computed++;
          return mergeRuns([], state.items, true);
        }),
      }));
      merged();
      return { set, merged, dispose, computed: () => computed };
    };
    const bash: ChatItem = { kind: "tool", id: "b", name: "Bash", summary: "", status: "running" };

    it("does not recompute for a change to a row that is not an agent's", () => {
      const m = setup([bash, tool("x")]);
      m.set("items", 0, "output" as never, "out" as never);
      m.set("items", 0, "status" as never, "done" as never);
      // Not even to what the scan looks at: only the rows that matched are watched.
      m.set("items", 0, "category" as never, "read" as never);
      m.merged();
      expect(m.computed()).toBe(1);
      m.dispose();
    });

    it("recomputes for a new row, and for a change to an agent's row", () => {
      const m = setup([bash, tool("x")]);
      m.set("items", 1, "status" as never, "done" as never);
      expect(m.merged()[0].status).toBe("done");
      expect(m.computed()).toBe(2);
      m.set("items", 2, tool("y"));
      expect(m.merged()).toHaveLength(2);
      expect(m.computed()).toBe(3);
      m.dispose();
    });
  });
});

describe("runTree order", () => {
  it("has running first, then errors, then the finished, the newest first within each", () => {
    const list = [run("old-done", { at: 1 }), run("new-done", { at: 5 }), run("old-run", { at: 2, status: "running" }), run("new-run", { at: 6, status: "running" }), run("err", { at: 3, status: "error" }), run("stop", { at: 4, status: "stopped" })];
    expect(shape(runTree(list))).toEqual(["new-run", "old-run", "err", "new-done", "stop", "old-done"]);
  });

  it("goes by arrival when there are no times", () => {
    expect(shape(runTree([run("first"), run("second")]))).toEqual(["second", "first"]);
  });
});

describe("runResult", () => {
  it("prefers the agent's own answer", () => {
    expect(runResult(run("a", { result: "Answer" }), "Tool output")).toBe("Answer");
  });

  it("has none while the agent runs, and takes no launch notice for one", () => {
    expect(runResult(run("a", { status: "running" }), "Partial")).toBeUndefined();
    expect(runResult(run("a", { status: "running", result: "From before" }))).toBeUndefined();
    expect(runResult(run("a"), "Async agent launched successfully. (internal)")).toBeUndefined();
  });
});

describe("formats", () => {
  it.each([[0, "0"], [999, "999"], [1000, "1.0k"], [90900, "90.9k"], [999949, "999.9k"], [999950, "1.0M"], [1200000, "1.2M"]])("tokens %d", (n, text) => expect(formatTokens(n)).toBe(text));
  it.each([[0, "0s"], [999, "0s"], [59000, "59s"], [60000, "1m 0s"], [229000, "3m 49s"], [5943000, "99m 3s"]])("duration %d", (ms, text) => expect(formatDuration(ms)).toBe(text));

  it("times a running agent by the clock, never below what it reported", () => {
    expect(runDuration(run("a", { status: "running", at: 1000 }), 4000)).toBe(3000);
    expect(runDuration(run("a", { status: "running", at: 3900, durationMs: 9000 }), 4000)).toBe(9000);
    expect(runDuration(run("a", { status: "running", durationMs: 9000 }), 4000)).toBe(9000);
  });

  it("times a finished agent by its report, and a stopped one by when it stopped", () => {
    expect(runDuration(run("a", { at: 1000, endedAt: 2000, durationMs: 700 }), 9000)).toBe(700);
    expect(runDuration(run("a", { at: 1000, endedAt: 2000 }), 9000)).toBe(1000);
    expect(runDuration(run("a", { status: "stopped", at: 1000, endedAt: 2000, durationMs: 700 }), 9000)).toBe(1000);
  });

  it("has no time to show without one", () => {
    expect(runDuration(run("a"), 9000)).toBeUndefined();
    expect(runDuration(run("a", { at: 1000 }), 9000)).toBeUndefined();
  });
});

describe("ChatController and the runs", () => {
  const line = (obj: unknown) => new TextEncoder().encode(JSON.stringify(obj) + "\n");
  const claude = () => {
    const written: string[] = [];
    const ctrl = new ChatController("s1", "/work", null, createClaudeAdapter, { transport: { send: (l: string) => void written.push(l) } as never });
    const feed = (objs: unknown[]) => objs.forEach((o) => (ctrl as any).feed(line(o)));
    return { ctrl, feed, written };
  };

  it("stops what still runs when the agent exits, and keeps what finished", () => {
    const { ctrl, feed } = claude();
    feed([started("t1", "c1"), started("t2", "c2"), notification("t2", "c2"), { type: "kelpie_exit", code: 0 }]);
    expect(ctrl.state.agentRuns.map((r) => [r.id, r.status])).toEqual([["c1", "stopped"], ["c2", "done"]]);
    expect(ctrl.state.agentRuns[0].endedAt).toEqual(expect.any(Number));
  });

  it("stops what the agent's last lines start after its exit, on a replay too", () => {
    const { ctrl, feed } = claude();
    feed([{ type: "kelpie_exit", code: 0 }, agentCall("c1"), started("t1", "c1"), progress("t1", "c1"), agentCall("c2", "c1"), { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "c3", name: "Agent", input: {} } } }]);
    expect(ctrl.state.agentRuns.map((r) => r.status)).toEqual(["stopped", "stopped", "stopped"]);
    expect(ctrl.state.agentRuns[0].endedAt).toEqual(expect.any(Number));
    feed([notification("t1", "c1")]);
    expect(ctrl.state.agentRuns[0]).toMatchObject({ status: "done", result: "The answer" });
  });

  it("brings a live agent back after /clear with a plain name, until its start names it", () => {
    const { ctrl, feed } = claude();
    feed([started("t1", "c1"), { type: "conversation_reset", trigger: "clear" }, progress("t1", "c1")]);
    expect(ctrl.state.agentRuns[0]).toMatchObject({ id: "c1", description: "", type: "general-purpose", summary: "Reading a.ts" });
    feed([started("t1", "c1")]);
    expect(ctrl.state.agentRuns[0].description).toBe("Name of t1");
  });

  it("forgets the runs on /clear", () => {
    const { ctrl, feed } = claude();
    feed([started("t1", "c1"), { type: "conversation_reset", trigger: "clear" }]);
    expect(ctrl.state.agentRuns).toEqual([]);
  });

  it("forgets them on a replay, and rebuilds them with the log's own times", async () => {
    const log = [{ type: "kelpie_time", at: 5000 }, agentCall("c1"), started("t1", "c1"), { type: "kelpie_time", at: 9000 }, notification("t1", "c1")];
    const written: string[] = [];
    const ctrl = new ChatController("s1", "/work", null, createClaudeAdapter, {
      transport: {
        send: (l: string) => void written.push(l),
        attach: async () => new TextEncoder().encode(log.map((o) => JSON.stringify(o)).join("\n") + "\n"),
      } as never,
    });
    ctrl.set("agentRuns", [run("gone")]);
    await ctrl.attach(1);
    expect(JSON.parse(JSON.stringify(ctrl.state.agentRuns))).toMatchObject([{ id: "c1", at: 5000, endedAt: 9000, status: "done" }]);
    // The handshake of the agent that starts now is the only thing written.
    expect(written.filter((l) => !l.includes('"subtype":"initialize"'))).toEqual([]);
  });
});
