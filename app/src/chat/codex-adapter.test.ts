import { describe, expect, it } from "vitest";
import { createStore, produce } from "solid-js/store";
import { codexArgs, createCodexAdapter } from "./codex-adapter";
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

/** Walks the handshake the app-server expects, up to an open thread. */
function openThread(a: ReturnType<typeof createCodexAdapter>) {
  a.start();
  a.onMessage({ id: "kelpie-init", result: {} }, "");
  a.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-1", turns: [] }, model: "gpt-5", sandbox: { type: "workspaceWrite" } } }, "");
}

describe("pictures", () => {
  const PIC = { mime: "image/png", data: "iVBORw0KGgo=", path: "C:\\Temp\\kelpie-composer\\image-1.png" };

  it("starts a turn with the picture as a local image next to the text", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.send("what is this?", true, [PIC]);
    const turn = h.written.at(-1);
    expect(turn.method).toBe("turn/start");
    expect(turn.params.input).toEqual([{ type: "text", text: "what is this?", text_elements: [] }, { type: "localImage", path: PIC.path }]);
  });

  it("keeps the picture of a prompt typed before the thread exists", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.start();
    a.send("early", true, [PIC]);
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    a.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-1", turns: [] } } }, "");
    expect(h.written.find((w) => w.method === "turn/start").params.input).toContainEqual({ type: "localImage", path: PIC.path });
  });
});

describe("codexArgs", () => {
  it("runs app-server and translates the TUI's model flags into -c overrides", () => {
    expect(codexArgs()).toEqual(["codex", "app-server"]);
    expect(codexArgs(["--model", "gpt-5", "-m", "o3", "--model=mini", "-c", "a=1", "--config", "b=2", "--yolo"])).toEqual([
      "codex", "app-server", "-c", 'model="gpt-5"', "-c", 'model="o3"', "-c", 'model="mini"', "-c", "a=1", "-c", "b=2",
    ]);
  });

  it("ignores a trailing flag without a value", () => {
    expect(codexArgs(["--model"])).toEqual(["codex", "app-server"]);
  });
});

describe("createCodexAdapter", () => {
  it("initializes, then opens a thread and asks for models and skills", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.start();
    expect(h.written[0]).toMatchObject({ id: "kelpie-init", method: "initialize" });
    expect(h.state.commands.map((c) => c.name)).toEqual(["compact", "review", "new", "model"]);
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    expect(h.written.slice(1).map((w) => w.method)).toEqual(["initialized", "thread/start", "model/list", "skills/list"]);
  });

  it("a replayed handshake sends nothing", () => {
    const h = harness(true);
    const a = createCodexAdapter(h.ctx);
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    expect(h.written).toEqual([]);
  });

  it("thread result sets the conversation, model and sandbox mode, and renders history", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.onMessage({ id: "kelpie-thread", result: { model: "gpt-5", sandbox: { type: "readOnly" }, thread: { id: "thr-9", turns: [{ items: [{ type: "userMessage", id: "i1", content: [{ type: "text", text: "hi" }, { type: "image" }] }, { type: "agentMessage", id: "i2", text: "hello" }] }] } } }, "");
    expect(h.state.conversationId).toBe("thr-9");
    expect(h.state.model).toBe("gpt-5");
    expect(h.state.settings.find((s) => s.id === "mode")!.value).toBe("read-only");
    expect(h.items()).toEqual([{ kind: "user", id: "i1", text: "hi" }, { kind: "assistant", id: "i2", text: "hello", streaming: false }]);
  });

  it("maps command, file change, MCP and web search items to tool cards", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.onMessage({ method: "item/started", params: { item: { type: "commandExecution", id: "c1", command: ["git", "status"] } } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "tool", name: "Shell", summary: "git status", status: "running" });
    a.onMessage({ method: "item/commandExecution/outputDelta", params: { itemId: "c1", delta: "clean" } }, "");
    a.onMessage({ method: "item/completed", params: { item: { type: "commandExecution", id: "c1", command: ["git", "status"], exitCode: 1 } } }, "");
    // The streamed output survives the completion that carries no aggregated output.
    expect(h.items()[0]).toMatchObject({ status: "error", output: "clean" });
    a.onMessage({ method: "item/completed", params: { item: { type: "fileChange", id: "f1", status: "completed", changes: [{ path: "a.ts", diff: "+x" }, { path: "b.ts", unified_diff: "-y" }] } } }, "");
    expect(h.items()[1]).toMatchObject({ name: "Edit", summary: "a.ts, b.ts", status: "done", diff: [{ path: "a.ts", text: "+x" }, { path: "b.ts", text: "-y" }] });
    a.onMessage({ method: "item/completed", params: { item: { type: "mcpToolCall", id: "m1", server: "gh", tool: "pr", arguments: { n: 1 }, error: { message: "nope" } } } }, "");
    expect(h.items()[2]).toMatchObject({ name: "gh.pr", summary: '{"n":1}', status: "error", output: "nope" });
    a.onMessage({ method: "item/started", params: { item: { type: "webSearch", id: "w1", query: "rust" } } }, "");
    expect(h.items()[3]).toMatchObject({ name: "WebSearch", summary: "rust", status: "running" });
  });

  it("accumulates agent and reasoning deltas, and turn/completed ends streaming", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.onMessage({ method: "turn/started", params: { turn: { id: "t1" } } }, "");
    expect(h.state.busy).toBe(true);
    for (const d of ["Hel", "lo"]) a.onMessage({ method: "item/agentMessage/delta", params: { itemId: "a1", delta: d } }, "");
    a.onMessage({ method: "item/reasoning/textDelta", params: { itemId: "r1", delta: "think" } }, "");
    a.onMessage({ method: "turn/completed", params: { turn: { id: "t1", status: "completed", durationMs: 1500 } } }, "");
    expect(h.state.busy).toBe(false);
    expect(h.items()).toEqual([
      { kind: "assistant", id: "a1", text: "Hello", streaming: false },
      { kind: "thinking", id: "r1", text: "think", streaming: false },
      { kind: "result", id: "turn-t1", text: "completed · 1.5s" },
    ]);
  });

  it("surfaces a failed turn and RPC errors, clearing busy for turn requests", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.onMessage({ method: "turn/completed", params: { turn: { id: "t2", status: "failed", error: { message: "rate limited" } } } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "system", tone: "error", text: "rate limited" });
    h.ctx.set("busy", true);
    a.onMessage({ id: "kelpie-turn-1-0", error: { message: "bad request" } }, "");
    expect(h.state.busy).toBe(false);
    expect(h.items().at(-1)).toMatchObject({ kind: "system", tone: "error", text: "Codex: bad request" });
  });

  it("a prompt typed before the thread exists is queued and sent once it is open", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.start();
    a.send("do the thing");
    expect(h.items()[0]).toMatchObject({ kind: "user", pending: true, text: "do the thing" });
    expect(h.written.some((w) => w.method === "turn/start")).toBe(false);
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    a.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-1", turns: [] } } }, "");
    const turn = h.written.find((w) => w.method === "turn/start");
    expect(turn.params).toMatchObject({ threadId: "thr-1", input: [{ type: "text", text: "do the thing", text_elements: [] }] });
    expect(h.items()[0]).toMatchObject({ text: "" });
  });

  it("starts turns with skill items for known $skills only", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.onMessage({ id: "kelpie-skills", result: { data: [{ skills: [{ name: "lint", path: "/s/lint", shortDescription: "Lint" }, { name: "off", path: "/s/off", enabled: false }] }] } }, "");
    expect(h.state.skills.map((s) => s.name)).toEqual(["lint"]);
    a.send("run $lint please, not $other");
    const turn = h.written.at(-1);
    expect(turn.method).toBe("turn/start");
    expect(turn.params.input).toEqual([{ type: "text", text: "run $lint please, not $other", text_elements: [] }, { type: "skill", name: "lint", path: "/s/lint" }]);
  });

  it("slash commands map to requests; TUI-only ones are offered in the terminal; others are prompts", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.send("/compact");
    expect(h.written.at(-1)).toMatchObject({ method: "thread/compact/start", params: { threadId: "thr-1" } });
    a.send("/review focus on auth");
    expect(h.written.at(-1)).toMatchObject({ method: "review/start", params: { target: { type: "custom", instructions: "focus on auth" } } });
    // `!cmd` runs outside the sandbox: only when the user typed it in the composer.
    a.send("!ls -l", true);
    expect(h.written.at(-1)).toMatchObject({ method: "thread/shellCommand", params: { command: "ls -l" } });
    a.send("!ls -l");
    expect(h.written.at(-1)).toMatchObject({ method: "turn/start" });
    a.send("/status");
    expect(h.items().at(-1)).toMatchObject({ kind: "system", tui: { command: "/status" } });
    a.send("/tmp is full");
    expect(h.written.at(-1)).toMatchObject({ method: "turn/start" });
  });

  it("mode and model picks travel with the next turn", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.setSetting!("mode", "full-access");
    a.setSetting!("model", "o3");
    a.send("go");
    const turn = h.written.at(-1);
    expect(turn.params).toMatchObject({ model: "o3", approvalPolicy: "never", sandboxPolicy: { type: "dangerFullAccess" } });
    expect(String(turn.id)).toContain("~");
  });

  it("never overrides the thread's own sandbox and approvals unless the user picks a mode", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.start();
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    a.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-1", turns: [] }, model: "gpt-5", approvalPolicy: "untrusted", sandbox: { type: "workspaceWrite", networkAccess: true, writableRoots: ["/extra"] } } }, "");
    a.send("one");
    // The live answer to a turn must not turn the shown mode into an override.
    a.onMessage({ id: h.written.at(-1).id, result: { turn: { id: "t1" } } }, "");
    a.send("two");
    expect(h.written.at(-1).params).not.toHaveProperty("sandboxPolicy");
    expect(h.written.at(-1).params).not.toHaveProperty("approvalPolicy");
    // Picking the same kind of sandbox keeps the thread's network access and extra roots.
    a.setSetting!("mode", "auto");
    a.send("three");
    expect(h.written.at(-1).params.sandboxPolicy).toEqual({ type: "workspaceWrite", networkAccess: true, writableRoots: ["/extra"] });
  });

  it("keeps a model the catalog does not list (from config.toml) pickable after switching away", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.start();
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    a.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-1", turns: [] }, model: "gpt-custom" } }, "");
    a.onMessage({ id: "kelpie-models", result: { data: [{ id: "gpt-6", model: "gpt-6", isDefault: true }, { id: "gpt-mini", model: "gpt-mini" }] } }, "");
    const picker = () => h.state.settings.find((s) => s.id === "model")!;
    expect(picker().value).toBe("gpt-custom");
    a.setSetting!("model", "gpt-mini");
    expect(picker().value).toBe("gpt-mini");
    expect(picker().options.map((o) => o.value)).toEqual(["gpt-custom", "gpt-6", "gpt-mini"]);
    a.send("/model gpt-custom");
    expect(picker().value).toBe("gpt-custom");
    a.send("go");
    expect(h.written.at(-1).params).toMatchObject({ model: "gpt-custom" });
  });

  it("/new, /clear and /reset open a new thread with an empty transcript, live and replayed", () => {
    for (const cmd of ["/new", "/clear", "/reset"]) {
      const h = harness();
      const a = createCodexAdapter(h.ctx);
      openThread(a);
      a.onMessage({ method: "item/completed", params: { item: { type: "agentMessage", id: "i1", text: "old" } } }, "");
      a.send(cmd);
      expect(h.written.at(-1)).toMatchObject({ method: "thread/start" });
      a.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-2", turns: [] } } }, "");
      expect(h.items()).toEqual([]);
      expect(h.state.conversationId).toBe("thr-2");
    }
    // The daemon log holds both thread answers: replaying it ends on the new thread alone.
    const r = harness(true);
    const b = createCodexAdapter(r.ctx);
    b.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-1", turns: [{ items: [{ type: "agentMessage", id: "i1", text: "old" }] }] } } }, "");
    b.onMessage({ id: "kelpie-thread", result: { thread: { id: "thr-2", turns: [] } } }, "");
    expect(r.items()).toEqual([]);
  });

  it("keeps the words after /new and refuses an unknown /model", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.send("/new add a login page");
    expect(h.written.at(-1)).toMatchObject({ method: "turn/start", params: { input: [{ text: "/new add a login page" }] } });
    const before = h.written.length;
    a.send("/model gpt-5 but faster");
    expect(h.written).toHaveLength(before);
    expect(h.items().at(-1)).toMatchObject({ kind: "system" });
    a.send("next");
    expect(h.written.at(-1).params).not.toHaveProperty("model");
  });

  it("approval requests become permission cards and are answered with the request's own id", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.onMessage({ id: 7, method: "item/commandExecution/requestApproval", params: { command: ["rm", "-rf", "x"] } }, "");
    expect(h.items()[0]).toMatchObject({ kind: "permission", id: "approval-7", command: "rm -rf x", title: "Run command?" });
    a.respond("approval-7", "acceptForSession");
    expect(h.written.at(-1)).toEqual({ id: 7, result: { decision: "acceptForSession" } });
    a.onMessage({ id: 8, method: "applyPatchApproval", params: { fileChanges: { "a.ts": {} } } }, "");
    expect(h.items()[1]).toMatchObject({ title: "Apply file changes?", detail: "a.ts" });
    a.respond("approval-8", "decline");
    expect(h.written.at(-1)).toEqual({ id: 8, result: { decision: "denied" } });
    const before = h.written.length;
    a.respond("approval-unknown", "accept");
    expect(h.written).toHaveLength(before);
  });

  it("keeps the plan, context use and the turn's diff", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.onMessage({ method: "turn/started", params: { turn: { id: "turn-1" } } }, "");
    a.onMessage({ method: "turn/plan/updated", params: { threadId: "thr-1", turnId: "turn-1", explanation: "why", plan: [{ step: "Read", status: "completed" }, { step: "Fix", status: "inProgress" }] } }, "");
    expect(h.state.plan).toEqual({ entries: [{ text: "Read", status: "done" }, { text: "Fix", status: "active" }], explanation: "why" });
    a.onMessage({ method: "thread/tokenUsage/updated", params: { threadId: "thr-1", turnId: "turn-1", tokenUsage: { last: { totalTokens: 1200 }, total: { totalTokens: 9000 }, modelContextWindow: 272000 } } }, "");
    expect(h.state.usage).toEqual({ used: 1200, size: 272000 });
    a.onMessage({ method: "turn/diff/updated", params: { threadId: "thr-1", turnId: "turn-1", diff: "diff --git a/x b/x\n+a\n" } }, "");
    a.onMessage({ method: "turn/completed", params: { turn: { id: "turn-1", status: "completed" } } }, "");
    expect(h.items().find((i) => i.kind === "result")).toMatchObject({ diff: "diff --git a/x b/x\n+a\n" });
  });

  it("reads a parsed shell command as what it does", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    a.onMessage({ method: "item/started", params: { item: { type: "commandExecution", id: "c1", command: "cat src/a.ts", commandActions: [{ type: "read", command: "cat src/a.ts", name: "a.ts", path: "src/a.ts" }] } } }, "");
    a.onMessage({ method: "item/started", params: { item: { type: "commandExecution", id: "c2", command: "rg foo", commandActions: [{ type: "search", command: "rg foo", query: "foo", path: null }] } } }, "");
    a.onMessage({ method: "item/started", params: { item: { type: "commandExecution", id: "c3", command: "cargo test", commandActions: [{ type: "unknown", command: "cargo test" }] } } }, "");
    expect(h.items().map((i: any) => [i.category, i.summary])).toEqual([["read", "a.ts"], ["search", "foo"], ["command", "cargo test"]]);
  });

  it("steers the running turn, and queues the message again when Codex refuses", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    openThread(a);
    expect(a.steer!("more")).toBe(false);
    a.onMessage({ method: "turn/started", params: { turn: { id: "turn-1" } } }, "");
    expect(a.steer!("also check b.ts")).toBe(true);
    const req = h.written.at(-1);
    expect(req).toMatchObject({ method: "turn/steer", params: { threadId: "thr-1", expectedTurnId: "turn-1", input: [{ type: "text", text: "also check b.ts" }] } });
    a.onMessage({ id: req.id, error: { message: "no active turn" } }, "");
    expect(h.state.queue.map((q) => q.text)).toEqual(["also check b.ts"]);
    expect(h.items().filter((i) => i.kind === "system")).toEqual([]);
  });
});
