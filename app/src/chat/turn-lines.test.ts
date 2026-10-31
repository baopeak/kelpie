// The daemon builds a plain-text turn itself (a phone's message, or a desktop one whose window is gone). These lines
// must be the ones the adapters would have written: one shared fixture, checked here against the adapters and in
// `crates/kelpied/src/chat/turns.rs` against the Rust builders.
import { describe, expect, it } from "vitest";
import { createStore, produce } from "solid-js/store";
import fixtures from "../../../crates/kelpied/src/chat/turn-fixtures.json";
import { createAcpAdapter } from "./acp-adapter";
import { type AdapterContext, type ChatState, emptyChatState } from "./chat-model";
import { createClaudeAdapter } from "./claude-adapter";
import { createCodexAdapter } from "./codex-adapter";

function harness() {
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
    cwd: "/work", resumeId: null, replaying: () => false,
  };
  return { ctx, written };
}

/** Request ids carry a counter and a time: they are not part of the format. */
function withoutIds(line: any): any {
  const copy = JSON.parse(JSON.stringify(line));
  for (const key of ["id", "request_id"]) if (key in copy) copy[key] = "*";
  return copy;
}

describe("the lines the daemon builds for a plain message", () => {
  it("are the ones the Claude adapter writes", () => {
    const h = harness();
    const a = createClaudeAdapter(h.ctx);
    a.send("hello");
    expect(withoutIds(h.written.at(-1))).toEqual(withoutIds(fixtures.claude.turn));
    a.interrupt();
    expect(withoutIds(h.written.at(-1))).toEqual(withoutIds(fixtures.claude.interrupt));
    expect(h.written.at(-1).request_id).toMatch(/^kelpie-interrupt-/);
  });

  it("are the ones the Codex adapter writes", () => {
    const h = harness();
    const a = createCodexAdapter(h.ctx);
    a.start();
    a.onMessage({ id: "kelpie-init", result: {} }, "");
    a.onMessage({ id: "kelpie-thread", result: { thread: { id: "T1", turns: [] } } }, "");
    a.send("hello");
    const turn = h.written.at(-1);
    expect(withoutIds({ ...turn, id: "x" })).toEqual(withoutIds(fixtures.codex.turn));
    expect(turn.id).toMatch(/^kelpie-turn-/);
    a.onMessage({ method: "turn/started", params: { turn: { id: "TURN1" } } }, "");
    a.interrupt();
    expect(withoutIds(h.written.at(-1))).toEqual(withoutIds(fixtures.codex.interrupt));
  });

  it("are the ones the ACP adapter writes", () => {
    const h = harness();
    const a = createAcpAdapter(h.ctx);
    a.start();
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: {} } }, "");
    a.onMessage({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "S1" } }, "");
    a.send("hello");
    expect(withoutIds(h.written.at(-1))).toEqual(withoutIds(fixtures.acp.turn));
    expect(h.written.at(-1).id).toMatch(/^kelpie-prompt-/);
    a.interrupt();
    expect(h.written.at(-1)).toEqual(fixtures.acp.interrupt);
  });
});
