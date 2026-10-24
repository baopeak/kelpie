import { describe, expect, it } from "vitest";
import { createStore, produce } from "solid-js/store";
import { applyClaudeMessage, claudeArgs, createClaudeAdapter } from "./claude-adapter";
import { type AdapterContext, type ChatItem, type ChatState, emptyChatState } from "./chat-model";

/** An adapter context over a real store; `written` collects what the adapter sends to the agent. */
function harness(replaying = false) {
  const [state, set] = createStore<ChatState>(emptyChatState());
  const written: any[] = [];
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
    cwd: "/work", resumeId: null, replaying: () => replaying,
  };
  return { state, ctx, written, items: () => JSON.parse(JSON.stringify(state.items)) as ChatItem[] };
}

// Lines as recorded from `claude -p --output-format stream-json --verbose --include-partial-messages`.
const INIT = { type: "system", subtype: "init", session_id: "sess-1", model: "claude-opus-5-5", permissionMode: "default", terminal_slash_commands: ["login"] };
const STREAM = [
  { type: "stream_event", event: { type: "message_start", message: { id: "msg_1" } } },
  { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } },
  { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hel" } } },
  { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "lo" } } },
  { type: "stream_event", event: { type: "content_block_start", index: 1, content_block: { type: "tool_use", id: "toolu_1", name: "Bash", input: {} } } },
  { type: "stream_event", event: { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '{"command":"ls ' } } },
  { type: "stream_event", event: { type: "content_block_delta", index: 1, delta: { type: "input_json_delta", partial_json: '-la"}' } } },
];

describe("claudeArgs", () => {
  it("runs stream-json with permission prompts over stdio", () => {
    const a = claudeArgs(null);
    expect(a.slice(0, 2)).toEqual(["claude", "-p"]);
    expect(a).toContain("--include-partial-messages");
    expect(a.join(" ")).toContain("--permission-prompt-tool stdio");
    expect(a).not.toContain("--resume");
  });

  it("adds resume and extra flags in order", () => {
    const a = claudeArgs("abc", ["--model", "opus"]);
    expect(a.slice(-4)).toEqual(["--resume", "abc", "--model", "opus"]);
  });
});

describe("applyClaudeMessage", () => {
  it("maps tool_use then its tool_result onto one tool item", () => {
    const { ctx, items } = harness();
    const live = { msgId: null as string | null };
    applyClaudeMessage(ctx, { type: "assistant", uuid: "u1", message: { id: "msg_1", content: [{ type: "tool_use", id: "toolu_1", name: "Bash", input: { command: "ls" } }] } }, live);
    applyClaudeMessage(ctx, { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "toolu_1", content: [{ type: "text", text: "a.txt" }] }] } }, live);
    expect(items()).toEqual([{ kind: "tool", id: "toolu_1", name: "Bash", input: { command: "ls" }, summary: "ls", status: "done", output: "a.txt" }]);
    expect(live.msgId).toBe("msg_1");
  });

  it("marks an erroring tool_result as error", () => {
    const { ctx, items } = harness();
    const live = { msgId: null };
    applyClaudeMessage(ctx, { type: "assistant", message: { id: "m", content: [{ type: "tool_use", id: "t", name: "Bash", input: { command: "false" } }] } }, live);
    applyClaudeMessage(ctx, { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t", content: "boom", is_error: true }] } }, live);
    expect(items()[0]).toMatchObject({ status: "error", output: "boom" });
  });

  it("builds an Edit diff from old and new strings", () => {
    const { ctx, items } = harness();
    applyClaudeMessage(ctx, { type: "assistant", message: { id: "m", content: [{ type: "tool_use", id: "t", name: "Edit", input: { file_path: "a.ts", old_string: "x\ny", new_string: "z" } }] } }, { msgId: null });
    expect((items()[0] as any).diff).toEqual([{ path: "a.ts", text: "-x\n-y\n+z" }]);
  });

  it("finalizes the streaming item of the same message instead of duplicating it", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    for (const m of STREAM.slice(0, 4)) a.onMessage(m, "");
    expect(h.items()).toEqual([{ kind: "assistant", id: "msg_1:assistant:0", text: "Hello", streaming: true, group: "msg_1" }]);
    a.onMessage({ type: "assistant", message: { id: "msg_1", content: [{ type: "text", text: "Hello" }] } }, "");
    expect(h.items()).toEqual([{ kind: "assistant", id: "msg_1:assistant:0", text: "Hello", streaming: false, group: "msg_1" }]);
  });

  it("drops meta, summary and tag-wrapped user messages but keeps real prompts", () => {
    const { ctx, items } = harness();
    const live = { msgId: null };
    applyClaudeMessage(ctx, { type: "user", isMeta: true, message: { content: "meta" } }, live);
    applyClaudeMessage(ctx, { type: "user", isCompactSummary: true, message: { content: "summary" } }, live);
    applyClaudeMessage(ctx, { type: "user", message: { content: "<command-name>/clear</command-name>" } }, live);
    applyClaudeMessage(ctx, { type: "user", message: { content: "[Request interrupted by user]" } }, live);
    applyClaudeMessage(ctx, { type: "user", uuid: "u9", message: { content: "fix the bug" } }, live);
    expect(items()).toEqual([{ kind: "user", id: "u9", text: "fix the bug" }]);
  });

  it("confirms our optimistic prompt when the echo arrives", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("hello there");
    expect(h.items()[0]).toMatchObject({ kind: "user", text: "hello there", pending: true });
    expect(h.state.busy).toBe(true);
    a.onMessage({ type: "user", uuid: "u1", message: { content: [{ type: "text", text: "hello there" }] } }, "");
    expect(h.items()).toHaveLength(1);
    expect(h.items()[0]).toMatchObject({ pending: false });
  });
});

describe("pictures", () => {
  const PIC = { mime: "image/png", data: "iVBORw0KGgo=", path: "C:\\Temp\\kelpie-composer\\image-1.png" };

  it("sends each picture as an image block ahead of the text and names it in the transcript", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("what is this?", true, [PIC]);
    expect(h.written[0]).toEqual({
      type: "user",
      message: { role: "user", content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: PIC.data } }, { type: "text", text: "what is this?" }] },
    });
    expect(h.items()[0]).toMatchObject({ kind: "user", text: "what is this?\n[image-1.png]" });
  });

  it("sends a picture alone without an empty text block", () => {
    const h = harness();
    createClaudeAdapter(h.ctx).send("", true, [PIC]);
    expect(h.written[0].message.content).toEqual([{ type: "image", source: { type: "base64", media_type: "image/png", data: PIC.data } }]);
  });
});

describe("/clear", () => {
  it("empties the transcript for the new conversation and shows no result for it", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(INIT, "");
    for (const m of STREAM) a.onMessage(m, "");
    h.ctx.set("plan", { entries: [{ text: "x", status: "active" }] });
    a.send("/clear");
    // As recorded from Claude Code 2.1.309.
    a.onMessage({ type: "conversation_reset", new_conversation_id: "sess-2", trigger: "clear" }, "");
    a.onMessage({ ...INIT, session_id: "sess-2" }, "");
    a.onMessage({ type: "result", is_error: false, num_turns: 0, duration_ms: 3, total_cost_usd: 0, modelUsage: {} }, "");
    expect(h.items()).toEqual([]);
    expect(h.state.plan).toBeNull();
    expect(h.state.conversationId).toBe("sess-2");
    expect(h.state.busy).toBe(false);
    // The next turn shows its result again.
    a.onMessage({ type: "result", num_turns: 1, duration_ms: 1000 }, "");
    expect(h.items()).toHaveLength(1);
  });

  it("ignores other conversation resets", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("hello");
    a.onMessage({ type: "conversation_reset", trigger: "something-else" }, "");
    expect(h.items()).toHaveLength(1);
  });
});

describe("createClaudeAdapter", () => {
  it("start() asks for the commands and models", () => {
    const h = harness();
    createClaudeAdapter(h.ctx).start();
    expect(h.written).toEqual([{ type: "control_request", request_id: "kelpie-init", request: { subtype: "initialize" } }]);
  });

  it("system init sets the conversation, model and TUI-only commands", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "control_response", response: { subtype: "success", request_id: "kelpie-init", response: { commands: [{ name: "login", description: "Sign in" }, { name: "compact", argumentHint: "[focus]" }], models: [{ value: "default", displayName: "Default", resolvedModel: "claude-opus-5-5", supportedEffortLevels: ["low", "high"] }], current_permission_mode: "default" } } }, "");
    a.onMessage(INIT, "");
    expect(h.state.conversationId).toBe("sess-1");
    expect(h.state.model).toBe("claude-opus-5-5");
    expect(h.state.commands.map((c) => [c.name, !!c.tuiOnly, c.hint])).toEqual([["login", true, undefined], ["compact", false, "[focus]"]]);
    expect(h.state.settings.map((s) => s.id)).toEqual(["model", "effort", "mode"]);
    expect(h.state.settings.find((s) => s.id === "model")!.value).toBe("default");
  });

  it("streams text and tool input, then result closes the turn", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    for (const m of STREAM) a.onMessage(m, "");
    expect(h.state.busy).toBe(true);
    const tool = h.items().find((i) => i.kind === "tool")!;
    expect(tool).toMatchObject({ id: "toolu_1", name: "Bash", status: "running", summary: "ls -la" });
    a.onMessage({ type: "result", uuid: "r1", duration_ms: 2500, num_turns: 2, total_cost_usd: 0.0123 }, "");
    expect(h.state.busy).toBe(false);
    expect(h.items().some((i) => (i as any).streaming)).toBe(false);
    expect(h.items().at(-1)).toEqual({ kind: "result", id: "result-r1", text: "2.5s · 2 turns · $0.012" });
  });

  it("ignores subagent internals", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ ...STREAM[0], parent_tool_use_id: "toolu_x" }, "");
    a.onMessage({ type: "assistant", parent_tool_use_id: "toolu_x", message: { id: "m", content: [{ type: "text", text: "inner" }] } }, "");
    expect(h.items()).toEqual([]);
    expect(h.state.busy).toBe(false);
  });

  it("turns can_use_tool into a permission card and answers it", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "control_request", request_id: "req-1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "rm -rf x" }, permission_suggestions: [{ type: "addRules" }] } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "permission", id: "req-1", command: "rm -rf x", feedback: true });
    expect((h.items()[0] as any).options.map((o: any) => o.id)).toEqual(["allow", "allow-session", "deny"]);
    a.respond("req-1", "allow-session");
    expect(h.written.at(-1)).toEqual({ type: "control_response", response: { subtype: "success", request_id: "req-1", response: { behavior: "allow", updatedInput: { command: "rm -rf x" }, updatedPermissions: [{ type: "addRules" }] } } });
    a.respond("req-1", "deny");
    expect(h.written.at(-1).response.response).toEqual({ behavior: "deny", message: "The user denied this action." });
  });

  it("denies with the user's message, and shows the edit it asks to make", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "control_request", request_id: "req-2", request: { subtype: "can_use_tool", tool_name: "Edit", input: { file_path: "a.ts", old_string: "a\nb\nc", new_string: "a\nB\nc" } } }, "");
    expect((h.items()[0] as any).diff).toEqual([{ path: "a.ts", text: " a\n-b\n+B\n c" }]);
    a.respond("req-2", "deny", { message: "Use a constant instead" });
    expect(h.written.at(-1).response.response).toEqual({ behavior: "deny", message: "Use a constant instead" });
  });

  it("answers AskUserQuestion in the card", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    const input = { questions: [{ question: "Which database?", header: "DB", multiSelect: false, options: [{ label: "Postgres", description: "SQL" }, { label: "Mongo" }] }] };
    a.onMessage({ type: "control_request", request_id: "req-3", request: { subtype: "can_use_tool", tool_name: "AskUserQuestion", input } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "permission", ask: [{ question: "Which database?", options: [{ label: "Postgres" }, { label: "Mongo" }] }] });
    a.respond("req-3", "answer", { answers: { "Which database?": "Postgres" } });
    expect(h.written.at(-1).response.response).toEqual({ behavior: "allow", updatedInput: { ...input, answers: { "Which database?": "Postgres" } } });
  });

  it("approves a plan and can switch to accepting edits", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "control_request", request_id: "req-4", request: { subtype: "can_use_tool", tool_name: "ExitPlanMode", input: { plan: "1. Do it" } } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "permission", plan: "1. Do it" });
    a.respond("req-4", "plan-accept-edits");
    expect(h.written.at(-1).response.response).toEqual({ behavior: "allow", updatedInput: { plan: "1. Do it" }, updatedPermissions: [{ type: "setMode", mode: "acceptEdits", destination: "session" }] });
  });

  it("keeps the todo list as the plan and reports context use", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(INIT, "");
    a.onMessage({ type: "assistant", message: { id: "m1", usage: { input_tokens: 10, cache_creation_input_tokens: 1000, cache_read_input_tokens: 5000, output_tokens: 90 }, content: [{ type: "tool_use", id: "t1", name: "TodoWrite", input: { todos: [{ content: "Read", status: "completed" }, { content: "Fix", activeForm: "Fixing", status: "in_progress" }, { content: "Test", status: "pending" }] } }] } }, "");
    expect(h.state.plan?.entries).toEqual([{ text: "Read", status: "done" }, { text: "Fixing", status: "active" }, { text: "Test", status: "pending" }]);
    expect(h.state.usage).toEqual({ used: 6100, size: null });
    a.onMessage({ type: "result", uuid: "r1", total_cost_usd: 0.5, modelUsage: { "claude-haiku-5-5": { contextWindow: 1, costUSD: 0.01 }, "claude-opus-5-5": { contextWindow: 1000000, costUSD: 0.49 } } }, "");
    expect(h.state.usage).toEqual({ used: 6100, size: 1000000, cost: 0.5 });
  });

  it("shows a control error but stays silent for a stale interrupt", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "control_response", response: { subtype: "error", request_id: "kelpie-interrupt-1", error: "nothing running" } }, "");
    expect(h.items()).toEqual([]);
    a.onMessage({ type: "control_response", response: { subtype: "error", request_id: "kelpie-req-3", error: "bad model" } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "system", tone: "error", text: "Claude: bad model" });
  });

  it("offers TUI-only slash commands in the terminal instead of sending them", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(INIT, "");
    a.send("/login now");
    expect(h.written).toEqual([]);
    expect(h.items()[0]).toMatchObject({ kind: "system", tui: { command: "/login now" } });
    a.send("/compact");
    expect(h.written.at(-1)).toEqual({ type: "user", message: { role: "user", content: [{ type: "text", text: "/compact" }] } });
  });

  it("setSetting sends the matching control request", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.setSetting!("model", "opus");
    a.setSetting!("mode", "plan");
    a.setSetting!("effort", "high");
    expect(h.written.map((w) => w.request)).toEqual([
      { subtype: "set_model", model: "opus" },
      { subtype: "set_permission_mode", mode: "plan" },
      { subtype: "apply_flag_settings", settings: { effortLevel: "high" } },
    ]);
    expect(h.written[2].request_id).toMatch(/^kelpie-effort:high:\d+$/);
  });
});

describe("task events", () => {
  // Shapes from the claude 2.1.310 SDK message schema (system task_started … task_notification).
  const started = (id: string, extra: object = {}) => ({ type: "system", subtype: "task_started", task_id: id, tool_use_id: `toolu_${id}`, description: `Run ${id}`, task_type: "local_bash", is_backgrounded: true, ...extra });

  it("lists a task from start to its notification, with its progress summary", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(started("b1"), "");
    a.onMessage({ type: "system", subtype: "task_progress", task_id: "b1", description: "Run b1", usage: { total_tokens: 0, tool_uses: 1, duration_ms: 10 }, summary: "Compiling" }, "");
    expect(JSON.parse(JSON.stringify(h.state.tasks))).toEqual([{ id: "b1", description: "Run b1", background: true, summary: "Compiling" }]);
    a.onMessage({ type: "system", subtype: "task_notification", task_id: "b1", status: "completed", output_file: "", summary: "done" }, "");
    expect(h.state.tasks).toEqual([]);
  });

  it("follows a move to the background and drops a task that ended through task_updated", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(started("f1", { is_backgrounded: false, task_type: "local_agent" }), "");
    expect(h.state.tasks[0].background).toBe(false);
    a.onMessage({ type: "system", subtype: "task_updated", task_id: "f1", patch: { is_backgrounded: true, description: "Review" } }, "");
    expect(h.state.tasks[0]).toMatchObject({ background: true, description: "Review" });
    a.onMessage({ type: "system", subtype: "task_updated", task_id: "f1", patch: { status: "killed" } }, "");
    expect(h.state.tasks).toEqual([]);
  });

  it("skips housekeeping tasks and a subagent's own tasks", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(started("h1", { skip_transcript: true }), "");
    a.onMessage(started("h2", { ambient: true }), "");
    a.onMessage(started("n1", { parent_task_id: "x" }), "");
    a.onMessage(started("n2", { owned_by_subagent: true }), "");
    expect(h.state.tasks).toEqual([]);
  });

  it("does not count a subagent the turn waits for as background work", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(started("w1", { task_type: "local_agent", awaited: true }), "");
    expect(h.state.tasks[0].background).toBe(false);
  });
});
