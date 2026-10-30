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

describe("a failed result", () => {
  it("shows the reason Claude gave, not only its subtype", () => {
    const { ctx, items } = harness();
    createClaudeAdapter(ctx).onMessage({ type: "result", subtype: "error_during_execution", is_error: true, errors: ["No conversation found with session ID: abc"], total_cost_usd: 0 }, "");
    expect(items()[0]).toMatchObject({ kind: "result", text: "Error: No conversation found with session ID: abc · $0.000" });
  });

  it("falls back to the subtype without a reason", () => {
    const { ctx, items } = harness();
    createClaudeAdapter(ctx).onMessage({ type: "result", subtype: "error_during_execution", is_error: true, errors: [] }, "");
    expect(items()[0]).toMatchObject({ kind: "result", text: "Error: error_during_execution" });
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
    // The picture is not part of the words: it travels beside them, by path, with no bytes in the store.
    expect(h.items()[0]).toMatchObject({ kind: "user", text: "what is this?", pending: true, images: [{ name: "image-1.png", mime: "image/png", path: PIC.path }] });
    expect((h.items()[0] as any).images[0].data).toBeUndefined();
  });

  it("sends a picture alone without an empty text block", () => {
    const h = harness();
    createClaudeAdapter(h.ctx).send("", true, [PIC]);
    expect(h.written[0].message.content).toEqual([{ type: "image", source: { type: "base64", media_type: "image/png", data: PIC.data } }]);
  });

  // The echo as recorded from Claude Code 2.1.314: picture blocks ahead of the text, or alone.
  const IMAGE = { type: "image", source: { type: "base64", media_type: "image/png", data: "QUJD" } };
  const echo = (...content: unknown[]) => ({ type: "user", uuid: "u1", message: { role: "user", content }, session_id: "s" });

  it("confirms a message with a picture when its echo comes, without a second copy", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("what is this?", true, [PIC]);
    a.onMessage(echo(IMAGE, { type: "text", text: "what is this?" }), "");
    expect(h.items()).toHaveLength(1);
    expect(h.items()[0]).toMatchObject({ text: "what is this?", pending: false, images: [{ name: "image-1.png", path: PIC.path }] });
    // The echo's bytes are not kept: this message has its path.
    expect((h.items()[0] as any).images[0].data).toBeUndefined();
  });

  it("confirms a picture alone: its echo has no text", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("", true, [PIC]);
    a.onMessage(echo(IMAGE), "");
    expect(h.items()).toHaveLength(1);
    expect(h.items()[0]).toMatchObject({ text: "", pending: false, images: [{ path: PIC.path }] });
  });

  it("confirms the oldest waiting picture first", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    h.ctx.upsert({ kind: "user", id: "first", text: "", pending: true, images: [{ name: "image-1.png", path: PIC.path }] });
    h.ctx.upsert({ kind: "user", id: "second", text: "", pending: true, images: [{ name: "image-2.png", path: PIC.path }] });
    a.onMessage(echo(IMAGE), "");
    expect(h.items().map((i: any) => i.pending)).toEqual([false, true]);
  });

  it("leaves plain text as it was", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("hello");
    a.onMessage(echo({ type: "text", text: "hello" }), "");
    expect(h.items()).toHaveLength(1);
    expect(h.items()[0]).toMatchObject({ text: "hello", pending: false });
    expect((h.items()[0] as any).images).toBeUndefined();
  });

  it("a replayed message keeps the picture bytes the log has, apart from its text", () => {
    const h = harness(true);
    createClaudeAdapter(h.ctx).onMessage(echo(IMAGE, { type: "text", text: "what is this?" }), "");
    expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "what is this?", images: [{ name: "Picture 1", mime: "image/png", data: "QUJD" }] }]);
    expect(h.written).toEqual([]);
  });

  it("a replayed picture alone shows as a message", () => {
    const h = harness(true);
    createClaudeAdapter(h.ctx).onMessage(echo(IMAGE, IMAGE), "");
    expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "", images: [{ name: "Picture 1", mime: "image/png", data: "QUJD" }, { name: "Picture 2", mime: "image/png", data: "QUJD" }] }]);
  });

  it("history from Claude's own transcript keeps only the names of its pictures", () => {
    const h = harness();
    applyClaudeMessage(h.ctx, echo(IMAGE, { type: "text", text: "old" }), { msgId: null }, true);
    expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "old", images: [{ name: "Picture 1" }] }]);
  });

  describe("a message that starts with <", () => {
    const TAGGED = "<b>bold</b> what is this?";

    it("is confirmed by its echo, with its picture: not dim for ever, no second copy", () => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      a.send(TAGGED, true, [PIC]);
      a.onMessage(echo(IMAGE, { type: "text", text: TAGGED }), "");
      expect(h.items()).toHaveLength(1);
      expect(h.items()[0]).toMatchObject({ text: TAGGED, pending: false, images: [{ name: "image-1.png", path: PIC.path }] });
    });

    it("is confirmed without a picture too", () => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      a.send(TAGGED);
      a.onMessage(echo({ type: "text", text: TAGGED }), "");
      expect(h.items()).toHaveLength(1);
      expect(h.items()[0]).toMatchObject({ text: TAGGED, pending: false });
    });

    it("does not clear a picture waiting alone", () => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      h.ctx.upsert({ kind: "user", id: "alone", text: "", pending: true, images: [{ name: "image-1.png", path: PIC.path }] });
      a.send(TAGGED, true, [PIC]);
      a.onMessage(echo(IMAGE, { type: "text", text: TAGGED }), "");
      expect(h.items().map((i: any) => [i.text, i.pending])).toEqual([["", true], [TAGGED, false]]);
    });

    it("is Claude's own when nothing is waiting with that text: no bubble, and nothing cleared", () => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      a.send("", true, [PIC]);
      a.onMessage(echo(IMAGE, { type: "text", text: "<system-reminder>x</system-reminder>" }), "");
      expect(h.items()).toHaveLength(1);
      expect(h.items()[0]).toMatchObject({ text: "", pending: true });
    });

    it("gives a picture to the words beside Claude's own text", () => {
      const h = harness();
      createClaudeAdapter(h.ctx).onMessage(echo({ type: "text", text: "<system-reminder>x</system-reminder>" }, IMAGE, { type: "text", text: "real" }), "");
      expect(h.items()).toMatchObject([{ text: "real", images: [{ name: "Picture 1" }] }]);
    });
  });

  describe("an echo the daemon took the bytes out of", () => {
    const STRIPPED = { type: "image", source: { type: "base64", media_type: "image/png", data: "", kelpie_stripped: PIC.data.length } };

    it("confirms the message, which keeps its own picture", () => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      a.send("what is this?", true, [PIC]);
      a.onMessage(echo(STRIPPED, { type: "text", text: "what is this?" }), "");
      expect(h.items()).toHaveLength(1);
      expect(h.items()[0]).toMatchObject({ pending: false, images: [{ path: PIC.path }] });
    });

    it("finds a picture sent from this window again when the log is replayed", () => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      a.send("", true, [PIC]);
      h.ctx.clear();
      a.reset();
      a.onMessage(echo(STRIPPED), "");
      expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "", images: [{ name: "image-1.png", mime: "image/png", path: PIC.path }] }]);
    });

    it("is only a name when it was not sent from this window", () => {
      const h = harness(true);
      createClaudeAdapter(h.ctx).onMessage(echo(STRIPPED, { type: "text", text: "old" }), "");
      expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "old", images: [{ name: "Picture 1" }] }]);
    });
  });

  it("a watcher keeps only the names of replayed pictures", () => {
    const h = harness(true);
    h.ctx.namesOnly = true;
    createClaudeAdapter(h.ctx).onMessage(echo(IMAGE, { type: "text", text: "look" }), "");
    expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "look", images: [{ name: "Picture 1" }] }]);
  });

  it("a command takes no pictures", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(INIT, "");
    expect(a.isCommand!("/login", true)).toBe(true);
    expect(a.isCommand!("what is /login?", true)).toBe(false);
    expect(a.isCommand!("/compact", true)).toBe(false);
  });

  it("a picture block with something other than bytes in it is only a name", () => {
    const h = harness();
    applyClaudeMessage(h.ctx, echo({ type: "image", source: { type: "url", url: "https://example.com/x.png" } }), { msgId: null });
    expect(h.items()).toEqual([{ kind: "user", id: "u1", text: "", images: [{ name: "Picture 1" }] }]);
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

  describe("a logged answer", () => {
    const LINE = { type: "kelpie_answer", request_id: "req-1", by: "desktop", answered_by: "desktop", at: 1, answer: { behavior: "allow", lasting: false } };
    const card = (tool: string, input: unknown) => {
      const h = harness();
      const a = createClaudeAdapter(h.ctx);
      a.onMessage({ type: "control_request", request_id: "req-1", request: { subtype: "can_use_tool", tool_name: tool, input } }, "");
      return { a, h, card: h.items()[0] as Extract<ChatItem, { kind: "permission" }> };
    };

    it("is not the adapter's to read: the controller takes it first", () => {
      const { a, h } = card("Bash", { command: "ls" });
      const before = h.items();
      a.onMessage(LINE, JSON.stringify(LINE));
      expect(h.items()).toEqual(before);
      expect(h.written).toEqual([]);
    });

    it("is described in the card's own words", () => {
      const bash = card("Bash", { command: "ls" });
      const describe = (c: typeof bash, answer: unknown) => c.a.describeAnswer!(c.card, answer);
      expect(describe(bash, { behavior: "allow", lasting: false })).toEqual({ label: "Allow" });
      expect(describe(bash, { behavior: "allow", lasting: true })).toEqual({ label: "Allow for session" });
      // A suggestion can set a mode on an ordinary card: the card has no such option.
      expect(describe(bash, { behavior: "allow", lasting: true, mode: "acceptEdits" })).toEqual({ label: "Allow for session" });
      expect(describe(bash, { behavior: "deny", message: "use dir", lasting: false })).toEqual({ label: "Deny", message: "use dir" });
      const plan = card("ExitPlanMode", { plan: "x" });
      expect(describe(plan, { behavior: "allow", lasting: true, mode: "acceptEdits" })).toEqual({ label: "Yes, and accept edits" });
      expect(describe(plan, { behavior: "allow", lasting: false })).toEqual({ label: "Yes, ask before edits" });
      expect(describe(plan, { behavior: "deny", lasting: false })).toMatchObject({ label: "Keep planning" });
    });

    it("says what the person answered, not the agent's words for a denial with no message", () => {
      const ask = card("AskUserQuestion", { questions: [{ question: "Which?", options: [{ label: "A" }] }, { question: "Where?", options: [{ label: "B" }] }] });
      expect(ask.a.describeAnswer!(ask.card, { behavior: "allow", answers: { "Which?": "A", "Where?": "there" }, lasting: false })).toEqual({ label: "A · there", answers: { "Which?": "A", "Where?": "there" } });
      expect(ask.a.describeAnswer!(ask.card, { behavior: "deny", message: "The user denied this action.", lasting: false })).toEqual({ label: "Skip", message: undefined });
    });

    it("does not take a tool's own `answers` input for answers to a question", () => {
      const tool = card("mcp__survey__submit", { answers: { "Q?": "the tool's input" } });
      expect(tool.a.describeAnswer!(tool.card, { behavior: "allow", answers: { "Q?": "the tool's input" }, lasting: false })).toEqual({ label: "Allow" });
    });

    it("is not known when it is not one, or only the verdict of a question is left", () => {
      const ask = card("AskUserQuestion", { questions: [{ question: "Which?", options: [{ label: "A" }] }] });
      expect(ask.a.describeAnswer!(ask.card, { behavior: "allow", lasting: false })).toBeNull();
      expect(ask.a.describeAnswer!(ask.card, { behavior: "allow", answers: { "Which?": 3 }, lasting: false })).toBeNull();
      expect(ask.a.describeAnswer!(ask.card, { error: true })).toBeNull();
      expect(ask.a.describeAnswer!(ask.card, {})).toBeNull();
    });
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

describe("the agent map's runs", () => {
  const started = (id: string, extra: object = {}) => ({ type: "system", subtype: "task_started", task_id: id, tool_use_id: `toolu_${id}`, description: `Run ${id}`, task_type: "local_agent", is_backgrounded: true, ...extra });
  const subagent = (type: string, content: unknown[]) => ({ type, parent_tool_use_id: "toolu_a1", message: { id: "m", content } });

  it("adds no rows and writes nothing for a subagent's lines, while the map follows them", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(started("a1"), "");
    a.onMessage(subagent("assistant", [{ type: "tool_use", id: "toolu_r", name: "Read", input: { file_path: "a.ts" } }, { type: "text", text: "Reading" }]), "");
    a.onMessage(subagent("user", [{ type: "tool_result", tool_use_id: "toolu_r", content: "secret output" }]), "");
    a.onMessage({ type: "stream_event", parent_tool_use_id: "toolu_a1", event: { type: "message_start", message: { id: "m2" } } }, "");
    expect(h.items()).toEqual([]);
    expect(h.written).toEqual([]);
    expect(h.state.busy).toBe(false);
    expect(h.state.agentRuns[0].activity.map((c) => [c.name, c.target, c.status])).toEqual([["Read", "a.ts", "done"]]);
    expect(JSON.stringify(h.state.agentRuns)).not.toContain("secret output");
  });

  it("leaves the strip as it is: it drops what ended and skips what is nested, and the map keeps both", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage(started("a1"), "");
    a.onMessage(started("n1", { parent_task_id: "a1" }), "");
    expect(h.state.tasks.map((x) => x.id)).toEqual(["a1"]);
    a.onMessage({ type: "system", subtype: "task_notification", task_id: "a1", tool_use_id: "toolu_a1", status: "completed", summary: "done" }, "");
    expect(h.state.tasks).toEqual([]);
    expect(h.state.agentRuns.map((r) => [r.id, r.status])).toEqual([["toolu_a1", "done"], ["toolu_n1", "running"]]);
  });

  it("starts a run when an Agent call begins to stream, and names it when the call is complete", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "c1", name: "Task", input: {} } } }, "");
    expect(h.state.agentRuns).toMatchObject([{ id: "c1", description: "", status: "running" }]);
    a.onMessage({ type: "assistant", message: { id: "m", content: [{ type: "tool_use", id: "c1", name: "Task", input: { description: "Look", subagent_type: "explore", prompt: "Go" } }] } }, "");
    expect(h.state.agentRuns).toMatchObject([{ id: "c1", description: "Look", type: "explore", prompt: "Go" }]);
    expect(h.items()).toHaveLength(1);
  });

  it("makes no run of other tools, and none from a conversation's history, which lists by its rows", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.onMessage({ type: "assistant", message: { id: "m", content: [{ type: "tool_use", id: "c1", name: "Bash", input: { command: "ls" } }] } }, "");
    applyClaudeMessage(h.ctx, { type: "assistant", message: { id: "m2", content: [{ type: "tool_use", id: "c2", name: "Agent", input: { description: "Old" } }] } }, { msgId: null }, true);
    expect(h.state.agentRuns).toEqual([]);
    expect(h.items().map((i) => i.id)).toEqual(["c1", "c2"]);
  });
});
