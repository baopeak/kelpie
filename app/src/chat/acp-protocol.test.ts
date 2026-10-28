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
  const a = createAcpAdapter(ctx);
  const msg = (m: object) => a.onMessage({ jsonrpc: "2.0", ...m }, "");
  return { state, ctx, a, msg, written, last: () => written.at(-1), items: () => JSON.parse(JSON.stringify(state.items)) as ChatItem[] };
}

describe("ACP protocol handling", () => {
  it("speaks version 1 and offers terminal sign-in and form questions", () => {
    const h = harness();
    h.a.start();
    expect(h.last()).toMatchObject({ method: "initialize", params: { protocolVersion: 1, clientCapabilities: { terminal: false, auth: { terminal: true }, elicitation: { form: {} } } } });
  });

  it("stops with a message when the agent answers another protocol version", () => {
    const h = harness();
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 2, agentCapabilities: {} } });
    expect(h.written.some((w) => w.method === "session/new")).toBe(false);
    expect(h.items().some((i) => i.kind === "system" && i.tone === "error")).toBe(true);
  });

  it("asks to sign in when the session needs it, then opens it", () => {
    const h = harness();
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [
      { id: "oauth", name: "Google login" },
      { id: "cli-login", name: "CLI login", type: "terminal", args: ["--login"] },
    ] } });
    h.msg({ id: "kelpie-session", error: { code: -32000, message: "Authentication required" } });
    const card = h.items().find((i) => i.kind === "permission") as any;
    expect(card.title).toBe("Sign in to continue");
    expect(card.options.map((o: any) => o.id)).toEqual(["method:oauth", "retry"]);
    expect(card.detail).toContain("--login");
    h.a.respond(card.id, "method:oauth");
    expect(h.last()).toMatchObject({ method: "authenticate", params: { methodId: "oauth" } });
    h.msg({ id: h.last().id, result: {} });
    expect(h.last()).toMatchObject({ method: "session/new", params: { cwd: "/work" } });
  });

  it("uses the older modes and models blocks and their setters", () => {
    const h = harness();
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 1, agentCapabilities: {} } });
    h.msg({ id: "kelpie-session", result: { sessionId: "s1",
      modes: { currentModeId: "default", availableModes: [{ id: "default", name: "Default" }, { id: "plan", name: "Plan" }] },
      models: { currentModelId: "m1", availableModels: [{ modelId: "m1", name: "Model one" }, { modelId: "m2", name: "Model two" }] } } });
    expect(h.state.settings.map((s) => [s.id, s.value])).toEqual([["mode", "default"], ["model", "m1"]]);
    expect(h.state.model).toBe("m1");
    h.a.setSetting!("mode", "plan");
    expect(h.last()).toMatchObject({ method: "session/set_mode", params: { sessionId: "s1", modeId: "plan" } });
    h.a.setSetting!("model", "m2");
    expect(h.last()).toMatchObject({ method: "session/set_model", params: { sessionId: "s1", modelId: "m2" } });
    expect(h.state.model).toBe("m2");
  });

  it("turns a question from the agent into a card and answers it", () => {
    const h = harness();
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 1, agentCapabilities: {} } });
    h.msg({ id: "kelpie-session", result: { sessionId: "s1" } });
    h.msg({ id: 41, method: "elicitation/create", params: { sessionId: "s1", mode: "form", message: "Pick a colour", requestedSchema: { type: "object", properties: {
      colour: { type: "string", title: "Colour", enum: ["red", "blue"] },
      count: { type: "integer", title: "How many" },
      sure: { type: "boolean", title: "Sure?" },
    } } } });
    const card = h.items().find((i) => i.kind === "permission") as any;
    expect(card.title).toBe("Pick a colour");
    expect(card.ask.map((q: any) => [q.question, q.options.map((o: any) => o.label)])).toEqual([["Colour", ["red", "blue"]], ["How many", []], ["Sure?", ["Yes", "No"]]]);
    h.a.respond(card.id, "answer", { answers: { Colour: "blue", "How many": "3", "Sure?": "Yes" } });
    expect(h.last()).toEqual({ jsonrpc: "2.0", id: 41, result: { action: "accept", content: { colour: "blue", count: 3, sure: true } } });
  });

  it("declines a question and URL-mode requests", () => {
    const h = harness();
    h.msg({ id: 42, method: "elicitation/create", params: { mode: "form", message: "Name?", requestedSchema: { properties: { name: { type: "string" } } } } });
    const card = h.items().find((i) => i.kind === "permission") as any;
    h.a.respond(card.id, "decline");
    expect(h.last()).toEqual({ jsonrpc: "2.0", id: 42, result: { action: "decline" } });
    h.msg({ id: 43, method: "elicitation/create", params: { mode: "url", url: "https://example.com/login", message: "Sign in" } });
    expect(h.last()).toEqual({ jsonrpc: "2.0", id: 43, result: { action: "decline" } });
  });

  it("names itself with a version", () => {
    const h = harness();
    h.a.start();
    expect(h.last().params.clientInfo).toMatchObject({ name: "kelpie", version: expect.any(String) });
  });

  it("Stop answers open questions, and never the sign-in card", () => {
    const h = harness();
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [{ id: "x", name: "X" }] } });
    h.msg({ id: "kelpie-session", error: { code: -32000, message: "auth_required" } });
    h.a.respond((h.items().find((i) => i.kind === "permission") as any).id, "method:x");
    h.msg({ id: h.last().id, result: {} });
    h.msg({ id: "kelpie-session", result: { sessionId: "s1" } });
    h.msg({ id: 60, method: "elicitation/create", params: { mode: "form", message: "Name?", requestedSchema: { properties: { n: { type: "string" } } } } });
    const before = h.written.length;
    h.a.interrupt!();
    const sent = h.written.slice(before);
    expect(sent).toContainEqual({ jsonrpc: "2.0", id: 60, result: { action: "cancel" } });
    expect(sent.some((w) => w.id === "auth")).toBe(false);
  });

  it("offers sign-in again when it fails", () => {
    const h = harness();
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 1, agentCapabilities: {}, authMethods: [{ id: "x", name: "X" }] } });
    h.msg({ id: "kelpie-session", error: { code: -32000, message: "auth_required" } });
    h.a.respond((h.items().find((i) => i.kind === "permission") as any).id, "method:x");
    h.msg({ id: h.last().id, error: { code: -32603, message: "run `goose configure` first" } });
    const cards = h.items().filter((i) => i.kind === "permission");
    expect(cards.length).toBe(2);
  });

  it("a failed resume opens a new session instead of asking to sign in", () => {
    const h = harness();
    h.ctx.set("conversationId", "old");
    h.a.start();
    h.msg({ id: "kelpie-init", result: { protocolVersion: 1, agentCapabilities: { loadSession: true } } });
    expect(h.last()).toMatchObject({ method: "session/load", params: { sessionId: "old" } });
    h.msg({ id: "kelpie-session", error: { code: -32000, message: "session not found" } });
    expect(h.items().some((i) => i.kind === "permission")).toBe(false);
    expect(h.last()).toMatchObject({ method: "session/new" });
  });

  it("a question with nothing to fill in can be accepted, optional fields are not forced", () => {
    const h = harness();
    h.msg({ id: 70, method: "elicitation/create", params: { mode: "form", message: "Continue?", requestedSchema: { properties: {} } } });
    let card = h.items().find((i) => i.kind === "permission") as any;
    expect(card.options.map((o: any) => o.id)).toEqual(["answer", "decline", "cancel"]);
    h.a.respond(card.id, "answer", { answers: {} });
    expect(h.last()).toEqual({ jsonrpc: "2.0", id: 70, result: { action: "accept", content: {} } });
    h.msg({ id: 71, method: "elicitation/create", params: { mode: "form", message: "Details", requestedSchema: { properties: { name: { type: "string" }, note: { type: "string" }, tags: { type: "array", items: { enum: ["a", "b"] } } }, required: ["name", "tags"] } } });
    card = h.items().filter((i) => i.kind === "permission").at(-1) as any;
    expect(card.ask.map((q: any) => q.question)).toEqual(["name", "tags"]);
    h.a.respond(card.id, "answer", { answers: { name: "kip", tags: "a, b" } });
    expect(h.last()).toEqual({ jsonrpc: "2.0", id: 71, result: { action: "accept", content: { name: "kip", tags: ["a", "b"] } } });
  });

  it("echoes the agent's own permission option ids", () => {
    const h = harness();
    h.msg({ id: 50, method: "session/request_permission", params: { toolCall: { title: "Run ls" }, options: [{ optionId: "allow-once", name: "Allow", kind: "allow_once" }, { optionId: "reject-once", name: "Reject", kind: "reject_once" }] } });
    const card = h.items().find((i) => i.kind === "permission") as any;
    h.a.respond(card.id, "allow-once");
    expect(h.last()).toEqual({ jsonrpc: "2.0", id: 50, result: { outcome: { outcome: "selected", optionId: "allow-once" } } });
  });
});
