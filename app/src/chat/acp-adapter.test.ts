import { describe, expect, it } from "vitest";
import { createStore, produce } from "solid-js/store";
import { createAcpAdapter } from "./acp-adapter";
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

/** Walks the handshake up to an open session with one answered prompt on screen. */
function openSession(a: ReturnType<typeof createAcpAdapter>) {
  a.start();
  a.onMessage({ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: {} } }, "");
  a.onMessage({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "s-1" } }, "");
  a.send("hi");
  a.onMessage({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hello" } } } }, "");
}

describe("a logged answer", () => {
  const LINE = { type: "kelpie_answer", request_id: 5, by: "desktop", answered_by: "desktop", at: 1, answer: { result: { outcome: { outcome: "selected", optionId: "once" } } } };
  const permission = () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    a.onMessage({ jsonrpc: "2.0", id: 5, method: "session/request_permission", params: { toolCall: { title: "Run ls" }, options: [{ optionId: "once", name: "Allow once", kind: "allow_once" }, { optionId: "no", name: "Reject", kind: "reject_once" }] } }, "");
    return { a, h, card: h.items()[0] as Extract<ChatItem, { kind: "permission" }> };
  };
  const form = (properties: Record<string, unknown>) => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    a.onMessage({ jsonrpc: "2.0", id: 6, method: "elicitation/create", params: { message: "Tell me", requestedSchema: { properties } } }, "");
    return { a, h, card: h.items()[0] as Extract<ChatItem, { kind: "permission" }> };
  };

  it("is not the adapter's to read: the controller takes it first", () => {
    const { a, h } = permission();
    const before = h.items();
    a.onMessage(LINE, JSON.stringify(LINE));
    expect(h.items()).toEqual(before);
    expect(h.written).toEqual([]);
  });

  it("names the option the agent offered", () => {
    const { a, card } = permission();
    expect(a.describeAnswer!(card, LINE.answer)).toEqual({ label: "Allow once" });
    expect(a.describeAnswer!(card, { result: { outcome: { outcome: "selected", optionId: "no" } } })).toEqual({ label: "Reject" });
    expect(a.describeAnswer!(card, { result: { outcome: { outcome: "selected", optionId: "gone" } } })).toBeNull();
    expect(a.describeAnswer!(card, { error: true })).toBeNull();
  });

  it("gives a form's fields back under their questions", () => {
    const { a, card } = form({ db: { type: "string", title: "Which database?", enum: ["Postgres", "Mongo"] }, ok: { type: "boolean", title: "Sure?" }, many: { type: "array", title: "Which?", items: { enum: ["A", "B"] } } });
    const said = a.describeAnswer!(card, { result: { action: "accept", content: { db: "Mongo", ok: false, many: ["A", "B"] } } });
    expect(said?.answers).toEqual({ "Which database?": "Mongo", "Sure?": "No", "Which?": "A, B" });
    expect(said?.label).toBe("Mongo · No · A, B");
  });

  it("names how a form was turned down, and does not make up answers", () => {
    const { a, card } = form({ db: { type: "string", title: "Which database?" } });
    expect(a.describeAnswer!(card, { result: { action: "decline" } })).toEqual({ label: "Decline" });
    expect(a.describeAnswer!(card, { result: { action: "cancel" } })).toEqual({ label: "Cancel" });
    // Only the verdict was kept (the answer was too long for the log).
    expect(a.describeAnswer!(card, { result: { action: "accept" } })).toBeNull();
    const nothingToFill = form({});
    expect(nothingToFill.a.describeAnswer!(nothingToFill.card, { result: { action: "accept", content: {} } })).toEqual({ label: "Accept" });
  });
});

describe("pictures", () => {
  const PIC = { mime: "image/png", data: "iVBORw0KGgo=", path: "C:\\Temp\\kelpie-composer\\image-1.png" };
  const open = (image: boolean) => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    a.start();
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: { promptCapabilities: { image } } } }, "");
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "s-1" } }, "");
    return { h, a };
  };

  it("sends an image block when the agent takes pictures", () => {
    const { h, a } = open(true);
    a.send("what is this?", true, [PIC]);
    const prompt = h.written.find((w) => w.method === "session/prompt");
    expect(prompt.params.prompt).toEqual([{ type: "image", mimeType: "image/png", data: PIC.data }, { type: "text", text: "what is this?" }]);
  });

  it("tells an agent that cannot take pictures where the file is", () => {
    const { h, a } = open(false);
    a.send("what is this?", true, [PIC]);
    const prompt = h.written.find((w) => w.method === "session/prompt");
    expect(prompt.params.prompt).toEqual([{ type: "text", text: `what is this?\n${PIC.path}` }]);
  });

  it("keeps the pictures of a prompt typed before the session opened", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    a.start();
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: { promptCapabilities: { image: true } } } }, "");
    a.send("early", true, [PIC]);
    expect(h.written.some((w) => w.method === "session/prompt")).toBe(false);
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "s-1" } }, "");
    const prompt = h.written.find((w) => w.method === "session/prompt");
    expect(prompt.params.prompt[0]).toMatchObject({ type: "image", data: PIC.data });
  });

  it("shows the picture beside the text, without its bytes", () => {
    const { h, a } = open(true);
    a.send("what is this?", true, [PIC]);
    expect(h.items().at(-1)).toMatchObject({ kind: "user", text: "what is this?", images: [{ name: "image-1.png", mime: "image/png", path: PIC.path }] });
    expect((h.items().at(-1) as any).images[0].data).toBeUndefined();
  });

  it("says when the agent cannot take the picture itself", () => {
    expect(open(true).a.takesPictures!()).toBe(true);
    expect(open(false).a.takesPictures!()).toBe(false);
    // Not known yet: nothing to warn about.
    expect(createAcpAdapter(harness().ctx).takesPictures!()).toBe(true);
  });

  it("reads the pictures of a loaded conversation's user message", () => {
    const { h, a } = open(true);
    const chunk = (content: unknown) => ({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "user_message_chunk", content } } });
    a.onMessage(chunk({ type: "image", mimeType: "image/png", data: "QUJD" }), "");
    a.onMessage(chunk({ type: "text", text: "look" }), "");
    a.onMessage(chunk({ type: "image", uri: "file:///etc/x.png" }), "");
    expect(h.items().at(-1)).toMatchObject({ kind: "user", text: "look", images: [{ name: "Picture 1", mime: "image/png", data: "QUJD" }, { name: "Picture 2" }] });
    // A location the agent names is not kept: it could point anywhere.
    expect(JSON.stringify(h.items().at(-1))).not.toContain("file:");
  });

  const imageChunk = (content: unknown) => ({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "user_message_chunk", content } } });

  it("is only a name when the daemon took the picture's bytes out", () => {
    const { h, a } = open(true);
    a.onMessage(imageChunk({ type: "image", mimeType: "image/png", data: "", kelpie_stripped: 9 }), "");
    expect(h.items().at(-1)).toEqual({ kind: "user", id: expect.any(String), text: "", streaming: false, images: [{ name: "Picture 1" }] });
  });

  it("a watcher keeps only the names of replayed pictures", () => {
    const { h, a } = open(true);
    h.ctx.namesOnly = true;
    a.onMessage(imageChunk({ type: "image", mimeType: "image/png", data: "QUJD" }), "");
    expect((h.items().at(-1) as any).images).toEqual([{ name: "Picture 1" }]);
  });

  it("a new conversation takes no pictures", () => {
    const { a } = open(true);
    expect(a.isCommand!("/new", true)).toBe(true);
    expect(a.isCommand!("look", true)).toBe(false);
  });
});

describe("what is still streaming", () => {
  const chunk = (sessionUpdate: string, text: string) => ({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate, content: { type: "text", text } } } });
  const toolUpdate = (sessionUpdate: string) => ({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate, toolCallId: "t1", title: "Run", kind: "execute", status: "in_progress" } } });
  const streaming = (h: ReturnType<typeof harness>) => h.items().filter((i: any) => i.streaming).map((i: any) => i.text);
  const promptId = (h: ReturnType<typeof harness>) => h.written.filter((w) => w.method === "session/prompt").at(-1).id;

  it("ends every reply a session/load replayed, live or from the log", () => {
    for (const replaying of [false, true]) {
      const h = harness(replaying);
      const a = createAcpAdapter(h.ctx);
      h.ctx.set("conversationId", "s-old");
      a.start();
      a.onMessage({ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: { loadSession: true } } }, "");
      for (const m of [chunk("user_message_chunk", "q1"), chunk("agent_thought_chunk", "hm"), chunk("agent_message_chunk", "a1"), chunk("user_message_chunk", "q2"), chunk("agent_message_chunk", "a2")]) a.onMessage(m, "");
      expect(streaming(h)).toEqual(["a2"]);
      a.onMessage({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "s-old" } }, "");
      expect(streaming(h)).toEqual([]);
      expect(h.state.busy).toBe(false);
    }
  });

  it("ends the reply a session/load left unfinished when the load fails", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    h.ctx.set("conversationId", "s-old");
    a.start();
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: { loadSession: true } } }, "");
    a.onMessage(chunk("agent_message_chunk", "part"), "");
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-session", error: { code: -32000, message: "no such session" } }, "");
    expect(streaming(h)).toEqual([]);
  });

  it("ends a text segment when a tool call interrupts it, and the next one at the turn's end", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    openSession(a);
    expect(streaming(h)).toEqual(["hello"]);
    a.onMessage(toolUpdate("tool_call"), "");
    expect(streaming(h)).toEqual([]);
    a.onMessage(chunk("agent_message_chunk", "after"), "");
    a.onMessage(chunk("agent_message_chunk", " the tool"), "");
    expect(streaming(h)).toEqual(["after the tool"]);
    a.onMessage(toolUpdate("tool_call_update"), "");
    expect(streaming(h)).toEqual([]);
    a.onMessage(chunk("agent_message_chunk", "last"), "");
    expect(streaming(h)).toEqual(["last"]);
    a.onMessage({ jsonrpc: "2.0", id: promptId(h), result: { stopReason: "end_turn" } }, "");
    expect(streaming(h)).toEqual([]);
  });

  it("ends a thought when the reply starts", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    openSession(a);
    a.onMessage(chunk("agent_thought_chunk", "hm"), "");
    expect(streaming(h)).toEqual(["hm"]);
    a.onMessage(chunk("agent_message_chunk", "reply"), "");
    expect(streaming(h)).toEqual(["reply"]);
  });

  it("ends the reply when the prompt fails or the next prompt goes out", () => {
    const failed = harness();
    const a = createAcpAdapter(failed.ctx);
    openSession(a);
    a.onMessage({ jsonrpc: "2.0", id: promptId(failed), error: { message: "boom" } }, "");
    expect(streaming(failed)).toEqual([]);
    expect(failed.state.busy).toBe(false);

    const next = harness();
    const b = createAcpAdapter(next.ctx);
    openSession(b);
    b.send("again");
    expect(streaming(next)).toEqual([]);
  });

  it("leaves user messages without a streaming mark", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    openSession(a);
    a.onMessage(chunk("user_message_chunk", "q"), "");
    a.onMessage(chunk("agent_message_chunk", "a"), "");
    expect(h.items().find((i: any) => i.text === "q")).toMatchObject({ streaming: false });
    expect(streaming(h)).toEqual(["a"]);
  });
});

describe("createAcpAdapter", () => {
  it("words an unusual stop reason and adds nothing to a normal finish", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    openSession(a);
    const id = h.written.find((w) => w.method === "session/prompt").id;
    a.onMessage({ jsonrpc: "2.0", id, result: { stopReason: "end_turn" } }, "");
    expect(h.items().find((i) => i.kind === "result")).toMatchObject({ text: "" });
    a.send("again");
    const id2 = h.written.filter((w) => w.method === "session/prompt")[1].id;
    a.onMessage({ jsonrpc: "2.0", id: id2, result: { stopReason: "max_tokens" } }, "");
    expect(h.items().filter((i) => i.kind === "result").map((i: any) => i.text)).toEqual(["", "Stopped: response length limit"]);
  });

  it("offers /new and /clear ahead of the agent's own commands, without the agent's own clear", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    a.start();
    expect(h.state.commands.map((c) => c.name)).toEqual(["new", "clear"]);
    a.onMessage({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "available_commands_update", availableCommands: [{ name: "clear", description: "Clear context" }, { name: "compact" }] } } }, "");
    expect(h.state.commands.map((c) => c.name)).toEqual(["new", "clear", "compact"]);
  });

  it("/new, /clear and /reset start a new session with an empty transcript, live and replayed", () => {
    for (const cmd of ["/new", "/clear", "/reset"]) {
      const h = harness();
      const a = createAcpAdapter(h.ctx);
      openSession(a);
      expect(h.items()).toHaveLength(2);
      a.send(cmd);
      expect(h.written.at(-1)).toMatchObject({ id: "kelpie-session-new", method: "session/new", params: { cwd: "/work" } });
      a.onMessage({ jsonrpc: "2.0", id: "kelpie-session-new", result: { sessionId: "s-2" } }, "");
      expect(h.items()).toEqual([]);
      expect(h.state.conversationId).toBe("s-2");
      a.send("next");
      expect(h.written.at(-1)).toMatchObject({ method: "session/prompt", params: { sessionId: "s-2" } });
    }
    const r = harness(true);
    const b = createAcpAdapter(r.ctx);
    b.onMessage({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "old" } } } }, "");
    b.onMessage({ jsonrpc: "2.0", id: "kelpie-session-new", result: { sessionId: "s-2" } }, "");
    expect(r.items()).toEqual([]);
  });

  it("words after /new are a prompt, and a failed /new stays in the session shown", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    openSession(a);
    a.send("/new login page");
    expect(h.written.at(-1)).toMatchObject({ method: "session/prompt", params: { prompt: [{ text: "/new login page" }] } });
    a.send("/clear");
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-session-new", error: { message: "nope" } }, "");
    expect(h.items().at(-1)).toMatchObject({ kind: "system", tone: "error" });
    a.send("again");
    expect(h.written.at(-1)).toMatchObject({ method: "session/prompt", params: { sessionId: "s-1" } });
  });
});
