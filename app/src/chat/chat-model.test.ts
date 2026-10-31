import { afterEach, describe, expect, it, vi } from "vitest";
import { createEffect, createMemo, createRoot } from "solid-js";
import { createAcpAdapter } from "./acp-adapter";
import { createClaudeAdapter } from "./claude-adapter";
import { createCodexAdapter } from "./codex-adapter";
import { type Adapter, type AdapterContext, ChatController, matchAnswer } from "./chat-model";

/** A controller over an adapter that records what it is asked to do. */
function setup(opts: { steer?: boolean; command?: string } = {}) {
  const calls: string[] = [];
  let ctx!: AdapterContext;
  const adapter: Adapter = {
    start() {},
    onMessage() {},
    reset() {},
    send(text, _typed, images) {
      calls.push(`send ${text}${images?.length ? ` +${images.length}` : ""}`);
      ctx.set("busy", true);
    },
    interrupt() {
      calls.push("interrupt");
    },
    respond() {},
    immediate: (text) => text === "/model x",
    ...(opts.steer ? { steer: (text: string) => (calls.push(`steer ${text}`), true) } : {}),
    ...(opts.command ? { isCommand: (text: string) => text === opts.command } : {}),
  };
  const ctrl = new ChatController("s1", "/work", null, (c) => ((ctx = c), adapter));
  ctrl.set("ready", true);
  const idle = async () => {
    ctx.set("busy", false);
    await Promise.resolve();
  };
  return { ctrl, calls, idle };
}

describe("ChatController queue", () => {
  it("queues while the agent works and sends the next message when it is done", async () => {
    const { ctrl, calls, idle } = setup();
    ctrl.send("one");
    ctrl.send("two");
    ctrl.send("three");
    expect(calls).toEqual(["send one"]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["two", "three"]);
    await idle();
    expect(calls).toEqual(["send one", "send two"]);
    await idle();
    expect(calls).toEqual(["send one", "send two", "send three"]);
    expect(ctrl.state.queue).toEqual([]);
  });

  it("keeps the pictures of a queued message, and sends a picture without text", async () => {
    const { ctrl, calls, idle } = setup();
    const pic = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
    ctrl.send("one");
    ctrl.send("two", true, [pic]);
    ctrl.send("", true, [pic, pic]);
    ctrl.send("  ");
    expect(ctrl.state.queue.map((q) => q.images?.length ?? 0)).toEqual([1, 2]);
    await idle();
    await idle();
    expect(calls).toEqual(["send one", "send two +1", "send  +2"]);
  });

  it("a queued picture message keeps its images when the turn ends", async () => {
    const ctrl = new ChatController("s1", "/work", null, createClaudeAdapter);
    ctrl.set("ready", true);
    const pic = { mime: "image/png", data: "AA==", path: "/tmp/kelpie-composer/a.png" };
    ctrl.send("one");
    ctrl.send("two", true, [pic]);
    ctrl.send("", true, [pic]);
    expect(ctrl.state.items).toHaveLength(1);
    for (const text of ["two", ""]) {
      ctrl.set("busy", false);
      await Promise.resolve();
      expect(ctrl.state.items.at(-1)).toMatchObject({ kind: "user", pending: true, text, images: [{ name: "a.png", path: pic.path }] });
    }
    expect(ctrl.state.queue).toEqual([]);
  });

  it("blanks text that is only white space next to a picture, and leaves other text as it is", () => {
    const { ctrl, calls } = setup();
    const pic = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
    ctrl.send("  \n ", true, [pic]);
    expect(calls).toEqual(["send  +1"]);
    ctrl.send("  \n ", true, [pic]);
    expect(ctrl.state.queue[0].text).toBe("");
    ctrl.send("  kept  ", true, [pic]);
    expect(ctrl.state.queue[1].text).toBe("  kept  ");
  });

  it("says which text takes no pictures", () => {
    const { ctrl } = setup({ command: "/compact" });
    expect(ctrl.takesPicturesWith("/compact")).toBe(false);
    expect(ctrl.takesPicturesWith("look at this")).toBe(true);
    // An adapter that has no commands of its own.
    expect(setup().ctrl.takesPicturesWith("/compact")).toBe(true);
  });

  it("names the pictures of what could not be sent", async () => {
    let feed!: (data: Uint8Array) => void;
    const ctrl = new ChatController("s1", "/work", null, createClaudeAdapter, {
      transport: { attach: async (_handle, onData) => ((feed = onData), new Uint8Array()), send: () => {} },
    });
    await ctrl.attach(1);
    const pic = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
    ctrl.send("one");
    ctrl.send("two", true, [pic]);
    ctrl.send("", true, [pic]);
    feed(new TextEncoder().encode(`${JSON.stringify({ type: "kelpie_exit", code: 0 })}\n`));
    expect(ctrl.state.items.at(-1)).toMatchObject({ kind: "system", text: "Not sent:\ntwo\n[a.png]\n[a.png]" });
    expect(ctrl.state.queue).toEqual([]);
  });

  it("never queues a picker command", () => {
    const { ctrl, calls } = setup();
    ctrl.send("one");
    ctrl.send("/model x");
    expect(calls).toEqual(["send one", "send /model x"]);
  });

  it("holds the queue after a stop until the user sends again", async () => {
    const { ctrl, calls, idle } = setup();
    ctrl.send("one");
    ctrl.send("two");
    ctrl.interrupt();
    await idle();
    expect(calls).toEqual(["send one", "interrupt"]);
    expect(ctrl.state.queuePaused).toBe(true);
    ctrl.send("three");
    await Promise.resolve();
    expect(calls).toEqual(["send one", "interrupt", "send two"]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["three"]);
  });

  it("sends now by steering, or by stopping the turn when the agent cannot be steered", async () => {
    const steering = setup({ steer: true });
    steering.ctrl.send("one");
    steering.ctrl.send("two");
    steering.ctrl.sendNow(steering.ctrl.state.queue[0].id);
    expect(steering.calls).toEqual(["send one", "steer two"]);
    expect(steering.ctrl.state.queue).toEqual([]);

    const plain = setup();
    plain.ctrl.send("one");
    plain.ctrl.send("two");
    plain.ctrl.send("three");
    plain.ctrl.sendNow(plain.ctrl.state.queue[1].id);
    expect(plain.calls).toEqual(["send one", "interrupt"]);
    expect(plain.ctrl.state.queue.map((q) => q.text)).toEqual(["three", "two"]);
    await plain.idle();
    expect(plain.calls.at(-1)).toBe("send three");
  });

  it("never steers a command into a turn: it stops the turn and goes next", () => {
    const { ctrl, calls } = setup({ steer: true });
    ctrl.send("one");
    ctrl.send("/compact");
    ctrl.sendNow(ctrl.state.queue[0].id);
    expect(calls).toEqual(["send one", "interrupt"]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["/compact"]);
  });

  it("an approval left open when the turn ends can no longer be answered", () => {
    const { ctrl, idle } = setup();
    ctrl.send("one");
    ctrl.upsert({ kind: "permission", id: "p1", title: "Allow?", options: [{ id: "allow", label: "Allow" }] });
    return idle().then(() => {
      expect(ctrl.state.items.find((i) => i.id === "p1")).toMatchObject({ resolved: "No longer needed" });
    });
  });

  it("an approval a phone answered first stops asking, under each adapter's card id", () => {
    const { ctrl } = setup();
    const card = (id: string) => ({ kind: "permission" as const, id, title: "Allow?", options: [{ id: "allow", label: "Allow" }] });
    // Claude names the card by the request id, Codex and ACP prefix the JSON-RPC id.
    for (const id of ["req-1", "approval-7", "perm-8", "elicit-9"]) ctrl.upsert(card(id));
    ctrl.resolvedElsewhere("req-1");
    ctrl.resolvedElsewhere(7);
    ctrl.resolvedElsewhere(8);
    ctrl.resolvedElsewhere("9");
    for (const id of ["req-1", "approval-7", "perm-8", "elicit-9"]) expect(ctrl.state.items.find((i) => i.id === id)).toMatchObject({ resolved: "Answered on another device" });
    // A request that was already answered here keeps its answer; one that is not on this screen is ignored.
    ctrl.upsert(card("req-2"));
    ctrl.respond("req-2", "allow", "Allowed");
    ctrl.resolvedElsewhere("req-2");
    ctrl.resolvedElsewhere("unknown");
    expect(ctrl.state.items.find((i) => i.id === "req-2")).toMatchObject({ resolved: "Allowed" });
    // An answer clicked here that the daemon dropped (a phone's counted) is corrected, once.
    ctrl.upsert(card("req-3"));
    ctrl.respond("req-3", "allow", "Allowed");
    ctrl.resolvedElsewhere("req-3");
    expect(ctrl.state.items.find((i) => i.id === "req-3")).toMatchObject({ resolved: "Allowed" });
    ctrl.resolvedElsewhere("req-3", true);
    expect(ctrl.state.items.find((i) => i.id === "req-3")).toMatchObject({ resolved: "Answered on another device" });
    // The agent repeating the card (a replay) does not bring the question back.
    ctrl.upsert(card("req-1"));
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Answered on another device" });
  });

  it("waits for the view to attach before sending", async () => {
    const { ctrl, calls } = setup();
    ctrl.set("ready", false);
    ctrl.send("early");
    expect(calls).toEqual([]);
    ctrl.set("ready", true);
    await Promise.resolve();
    expect(calls).toEqual(["send early"]);
  });

  it("gives replayed items the time the log carried", () => {
    const { ctrl } = setup();
    const feed = (lines: unknown[]) => (ctrl as any).feed(new TextEncoder().encode(lines.map((l) => JSON.stringify(l)).join("\n") + "\n"));
    (ctrl as any).isReplaying = true;
    feed([{ type: "kelpie_time", at: 1_792_000_000_000 }]);
    ctrl.upsert({ kind: "user", id: "u1", text: "hi" });
    (ctrl as any).isReplaying = false;
    ctrl.upsert({ kind: "user", id: "u2", text: "now" });
    expect(ctrl.state.items[0].at).toBe(1_792_000_000_000);
    expect(ctrl.state.items[1].at).toBeGreaterThan(1_792_000_000_000);
  });

  it("times a thought from its first to its last chunk", () => {
    const { ctrl } = setup();
    ctrl.upsert({ kind: "thinking", id: "th", text: "", streaming: true });
    expect(ctrl.state.items[0]).toMatchObject({ at: expect.any(Number) });
    ctrl.patch("th", (i: any) => (i.streaming = false));
    const it = ctrl.state.items[0] as any;
    expect(it.doneAt).toBeGreaterThanOrEqual(it.at);
  });

  it("forgets running tasks when the agent exits: nothing will report them done", () => {
    const { ctrl } = setup();
    ctrl.set("tasks", [{ id: "b1", description: "npm run dev", background: true }]);
    (ctrl as any).feed(new TextEncoder().encode(JSON.stringify({ type: "kelpie_exit", code: 0 }) + "\n"));
    expect(ctrl.state.exited).toBe(true);
    expect(ctrl.state.tasks).toEqual([]);
  });

  it("ends what was streaming when the agent exits: nothing will finish it", () => {
    const { ctrl } = setup();
    ctrl.upsert({ kind: "thinking", id: "th", text: "hm", streaming: true });
    ctrl.upsert({ kind: "assistant", id: "a", text: "hel", streaming: true });
    (ctrl as any).feed(new TextEncoder().encode(JSON.stringify({ type: "kelpie_exit", code: 1 }) + "\n"));
    expect(ctrl.state.items.filter((i: any) => i.streaming)).toEqual([]);
    expect(ctrl.state.items.find((i) => i.id === "a")).toMatchObject({ text: "hel" });
  });

  it("times a replayed thinking row by the log, not by how old the log is", async () => {
    const update = (sessionUpdate: string, text: string) => ({ jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate, content: { type: "text", text } } } });
    const log = [{ type: "kelpie_time", at: 1000 }, update("agent_thought_chunk", "hm"), { type: "kelpie_time", at: 5000 }, update("agent_message_chunk", "done")].map((m) => JSON.stringify(m) + "\n").join("");
    const ctrl = new ChatController("s1", "/work", null, createAcpAdapter, { transport: { attach: async () => new TextEncoder().encode(log) } });
    await ctrl.attach(1);
    expect(ctrl.state.items.find((i) => i.kind === "thinking")).toMatchObject({ at: 1000, doneAt: 5000 });
  });

  it("does not stream what arrives after the agent's exit", async () => {
    const chunk = { jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "last words" } } } };
    const log = [{ type: "kelpie_exit", code: 1 }, chunk].map((m) => JSON.stringify(m) + "\n").join("");
    const ctrl = new ChatController("s1", "/work", null, createAcpAdapter, { transport: { attach: async () => new TextEncoder().encode(log) } });
    await ctrl.attach(1);
    expect(ctrl.state.items.find((i) => i.kind === "assistant")).toMatchObject({ text: "last words", streaming: false });
  });

  it("ends what was streaming when a replayed log shows the agent exited", async () => {
    const chunk = { jsonrpc: "2.0", method: "session/update", params: { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "hel" } } } };
    const log = [chunk, { type: "kelpie_exit", code: 1 }].map((m) => JSON.stringify(m) + "\n").join("");
    const ctrl = new ChatController("s1", "/work", null, createAcpAdapter, { transport: { attach: async () => new TextEncoder().encode(log) } });
    await ctrl.attach(1);
    expect(ctrl.state.exited).toBe(true);
    expect(ctrl.state.items.find((i) => i.kind === "assistant")).toMatchObject({ text: "hel", streaming: false });
  });
});

describe("an answer kept in the log", () => {
  const logged = (requestId: string | number, answer: unknown, by = "desktop", at = 1_793_000_000_000) => ({ type: "kelpie_answer", request_id: requestId, by: by === "desktop" ? "desktop" : "phone", answered_by: by, at, answer });
  const askInput = {
    questions: [
      { question: "Which database?", header: "DB", options: [{ label: "Postgres" }, { label: "Mongo" }] },
      { question: "Which port?", options: [{ label: "5432" }] },
    ],
  };
  const claudeAsk = (id = "req-3") => ({ type: "control_request", request_id: id, request: { subtype: "can_use_tool", tool_name: "AskUserQuestion", input: askInput } });
  const claudeBash = (id = "req-4") => ({ type: "control_request", request_id: id, request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "ls" } } });
  const claudeTurn = { type: "stream_event", event: { type: "message_start", message: { id: "m1" } } };
  const claudeEnd = { type: "result", subtype: "success", uuid: "r1" };
  const codexAsk = (command = "ls") => ({ id: 7, method: "item/commandExecution/requestApproval", params: { threadId: "t", turnId: "u", itemId: "i", command } });
  const codexTurn = { method: "turn/started", params: { turn: { id: "u" } } };
  const codexEnd = { method: "turn/completed", params: { turn: { id: "u", status: "completed" } } };
  const acpAsk = { jsonrpc: "2.0", id: 5, method: "session/request_permission", params: { toolCall: { title: "Run ls" }, options: [{ optionId: "once", name: "Allow once", kind: "allow_once" }, { optionId: "no", name: "Reject", kind: "reject_once" }] } };
  const acpEnd = { jsonrpc: "2.0", id: "kelpie-prompt-1", result: { stopReason: "end_turn" } };

  /** A controller over a real adapter whose `respond` must never run, and what reaches the agent. */
  function make(create: (ctx: AdapterContext) => Adapter, opts: { attachLog?: unknown[][] } = {}) {
    const sent: string[] = [];
    const responded: string[] = [];
    const logs = [...(opts.attachLog ?? [])];
    const ctrl = new ChatController("s1", "/work", null, (ctx) => {
      const adapter = create(ctx);
      const respond = adapter.respond.bind(adapter);
      adapter.respond = (id, option, reply) => {
        responded.push(id);
        respond(id, option, reply);
      };
      return adapter;
    }, { transport: { send: (line) => void sent.push(line), update: () => {}, attach: async () => bytes(logs.shift() ?? []) } });
    const feed = (lines: unknown[]) => (ctrl as any).feed(bytes(lines));
    const replay = (lines: unknown[]) => {
      (ctrl as any).isReplaying = true;
      feed(lines);
      (ctrl as any).isReplaying = false;
    };
    const card = (id: string) => ctrl.state.items.find((i) => i.id === id) as Extract<ReturnType<typeof ctrl.state.items.find>, { kind: "permission" }>;
    return { ctrl, sent, responded, feed, replay, card };
  }
  const bytes = (lines: unknown[]) => new TextEncoder().encode(lines.map((l) => JSON.stringify(l) + "\n").join(""));

  const dialects = [
    { name: "claude", create: createClaudeAdapter, log: [claudeTurn, claudeBash(), logged("req-4", { behavior: "deny", message: "use dir", lasting: false }), claudeEnd], card: "req-4", resolved: "Deny", answered: { label: "Deny", message: "use dir" } },
    { name: "codex", create: createCodexAdapter, log: [codexTurn, codexAsk(), logged(7, { result: { decision: "acceptForSession" } }), codexEnd], card: "approval-7", resolved: "Allow for session", answered: { label: "Allow for session" } },
    { name: "acp", create: createAcpAdapter, log: [acpAsk, logged(5, { result: { outcome: { outcome: "selected", optionId: "once" } } }), acpEnd], card: "perm-5", resolved: "Allow once", answered: { label: "Allow once" } },
  ];

  for (const d of dialects) {
    it(`${d.name}: a replay restores the answer, writes nothing to the agent, and the turn's end leaves it alone`, () => {
      const { sent, responded, replay, card } = make(d.create);
      replay(d.log);
      expect(card(d.card)).toMatchObject({ resolved: d.resolved, answered: { ...d.answered, at: 1_793_000_000_000 } });
      expect(card(d.card).answered!.by).toBeUndefined();
      expect(sent).toEqual([]);
      expect(responded).toEqual([]);
    });

    it(`${d.name}: a request nobody answered still expires when the replayed turn ends`, () => {
      const { replay, card } = make(d.create);
      replay(d.log.filter((l: any) => l.type !== "kelpie_answer"));
      expect(card(d.card).resolved).toBe("No longer needed");
      expect(card(d.card).answered).toBeUndefined();
    });

    it(`${d.name}: the line arriving live writes nothing either`, () => {
      const { sent, responded, feed, card } = make(d.create);
      feed(d.log.slice(0, -2));
      expect(card(d.card).resolved).toBeUndefined();
      feed(d.log.slice(-2, -1));
      expect(card(d.card).resolved).toBe(d.resolved);
      expect(sent).toEqual([]);
      expect(responded).toEqual([]);
    });
  }

  it("a whole attach, handshake included, sends the agent no answer", async () => {
    const { ctrl, sent, card } = make(createClaudeAdapter, { attachLog: [[claudeTurn, claudeAsk(), logged("req-3", { behavior: "allow", answers: { "Which database?": "Postgres", "Which port?": "5432, 9999" }, lasting: false }), claudeEnd]] });
    await ctrl.attach(1);
    expect(card("req-3")).toMatchObject({ resolved: "Postgres · 5432, 9999", answered: { answers: { "Which database?": "Postgres", "Which port?": "5432, 9999" } } });
    expect(sent.map((l) => JSON.parse(l).request?.subtype)).toEqual(["initialize"]);
  });

  it("an answer from another device names it, and says what it said", () => {
    const { replay, card } = make(createClaudeAdapter);
    replay([claudeTurn, claudeBash(), logged("req-4", { behavior: "allow", lasting: true }, "Pixel", Date.now() - 125_000), claudeEnd]);
    expect(card("req-4")).toMatchObject({ resolved: "Already answered on Pixel 2 min ago", answered: { label: "Allow for session", by: "Pixel" } });
  });

  it("a record no card option matches stays truthful", () => {
    const { replay, card } = make(createCodexAdapter);
    replay([codexTurn, codexAsk(), logged(7, { error: true })]);
    expect(card("approval-7")).toMatchObject({ resolved: "Answered", answered: { label: "Answered" } });
  });

  it("an answer whose request was trimmed away, or a line with no id, is ignored", () => {
    const { ctrl, replay } = make(createClaudeAdapter);
    replay([logged("gone", { behavior: "allow" }), { type: "kelpie_answer" }, { type: "kelpie_answer", request_id: {} }]);
    expect(ctrl.state.items).toEqual([]);
  });

  it("an answer given here shows at once, and the log only confirms it", () => {
    const { ctrl, feed, card, sent } = make(createClaudeAdapter);
    feed([claudeTurn, claudeBash()]);
    ctrl.respond("req-4", "deny", "Deny", { message: "use dir" });
    expect(card("req-4")).toMatchObject({ resolved: "Deny", answered: { label: "Deny", message: "use dir" } });
    expect(sent).toHaveLength(1);
    feed([logged("req-4", { behavior: "deny", message: "use dir", lasting: false })]);
    expect(card("req-4")).toMatchObject({ resolved: "Deny", answered: { label: "Deny", message: "use dir", at: 1_793_000_000_000 } });
    expect(sent).toHaveLength(1);
  });

  it("a re-attach to a daemon with no answer lines keeps what was clicked here", async () => {
    const log = [claudeTurn, claudeAsk()];
    const { ctrl, card } = make(createClaudeAdapter, { attachLog: [log, log] });
    await ctrl.attach(1);
    ctrl.respond("req-3", "answer", "Postgres · 5432", { answers: { "Which database?": "Postgres", "Which port?": "5432" } });
    await ctrl.attach(1);
    expect(card("req-3")).toMatchObject({ resolved: "Postgres · 5432", answered: { answers: { "Which database?": "Postgres" } } });
  });

  it("a click here that lost to a phone shows the phone's answer, and the dropped event does not blur it", () => {
    const { ctrl, feed, card } = make(createClaudeAdapter);
    feed([claudeTurn, claudeBash()]);
    ctrl.respond("req-4", "deny", "Deny", { message: "no" });
    feed([logged("req-4", { behavior: "allow", lasting: false }, "Pixel")]);
    ctrl.resolvedElsewhere("req-4", true);
    expect(card("req-4").answered).toMatchObject({ label: "Allow", by: "Pixel" });
    expect(card("req-4").resolved).toMatch(/^Already answered on Pixel/);
    expect(card("req-4").answered!.message).toBeUndefined();
  });

  it("with an older daemon a lost click drops what it claimed", () => {
    const { ctrl, feed, card } = make(createClaudeAdapter);
    feed([claudeTurn, claudeBash()]);
    ctrl.respond("req-4", "deny", "Deny", { message: "no" });
    ctrl.resolvedElsewhere("req-4", true);
    expect(card("req-4").resolved).toBe("Answered on another device");
    expect(card("req-4").answered).toBeUndefined();
  });

  describe("an id a restarted agent uses again", () => {
    it("is a new request, even one that asks the same thing", () => {
      for (const again of ["ls", "rm -rf ~"]) {
        const { replay, card, ctrl, sent } = make(createCodexAdapter);
        replay([codexTurn, codexAsk(), logged(7, { result: { decision: "accept" } }), codexEnd, codexTurn, codexAsk(again)]);
        expect(card("approval-7").resolved).toBeUndefined();
        expect(card("approval-7").answered).toBeUndefined();
        ctrl.respond("approval-7", "decline", "Decline");
        expect(sent.map((l) => JSON.parse(l).result.decision)).toEqual(["decline"]);
      }
    });

    it("takes its own answer", () => {
      const { replay, card } = make(createCodexAdapter);
      replay([codexTurn, codexAsk(), logged(7, { result: { decision: "accept" } }), codexEnd, codexTurn, codexAsk("rm"), logged(7, { result: { decision: "decline" } }), codexEnd]);
      expect(card("approval-7")).toMatchObject({ resolved: "Decline", answered: { label: "Decline" } });
    });

    it("is not closed by an earlier request that expired in the same replay", () => {
      const { replay, card } = make(createCodexAdapter);
      replay([codexTurn, codexAsk(), codexEnd, codexTurn, codexAsk("rm")]);
      expect(card("approval-7").resolved).toBeUndefined();
    });

    it("the same log replayed again ends the same", async () => {
      const log = [codexTurn, codexAsk(), logged(7, { result: { decision: "accept" } }), codexEnd];
      const { ctrl, card } = make(createCodexAdapter, { attachLog: [log, log] });
      await ctrl.attach(1);
      await ctrl.attach(1);
      expect(card("approval-7")).toMatchObject({ resolved: "Allow", answered: { label: "Allow" } });
    });
  });

  describe("an ACP replay, which has no prompt to say a turn runs", () => {
    const open = [{ jsonrpc: "2.0", id: "kelpie-init", result: { agentCapabilities: {} } }, { jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "s-1" } }];
    const form = { jsonrpc: "2.0", id: 6, method: "elicitation/create", params: { message: "Tell me", requestedSchema: { properties: { db: { type: "string", title: "Which?" } } } } };
    const answered = logged(5, { result: { outcome: { outcome: "selected", optionId: "once" } } });
    const exit = { type: "kelpie_exit", code: 0 };
    const stopped = { jsonrpc: "2.0", id: "kelpie-prompt-2", result: { stopReason: "end_turn" } };
    /** Everything that went to the agent, as parsed lines. */
    const lines = (sent: string[]) => sent.map((l) => JSON.parse(l));
    const cancels = (sent: string[]) => lines(sent).filter((l) => "result" in l || "error" in l);
    const prompts = (sent: string[]) => lines(sent).filter((l) => l.method === "session/prompt");
    const attached = async (log: unknown[]) => {
      const m = make(createAcpAdapter, { attachLog: [log] });
      await m.ctrl.attach(1);
      expect(m.sent).toEqual([]);
      return m;
    };

    it("request and answer in a turn that still runs: settled, not busy, and a send and a Stop work", async () => {
      const { ctrl, sent, feed, card } = await attached([...open, acpAsk, answered]);
      expect(card("perm-5")).toMatchObject({ resolved: "Allow once" });
      expect(ctrl.state.busy).toBe(false);
      ctrl.send("next");
      expect(prompts(sent)).toHaveLength(1);
      expect(ctrl.state.busy).toBe(true);
      ctrl.interrupt();
      expect(cancels(sent)).toEqual([]);
      feed([stopped]);
      expect(ctrl.state.busy).toBe(false);
    });

    it("request then exit: closed, not busy, and nothing goes out", async () => {
      const { ctrl, sent, card } = await attached([...open, acpAsk, exit]);
      expect(card("perm-5").resolved).toBe("No longer needed");
      expect(ctrl.state).toMatchObject({ busy: false, exited: true });
      ctrl.send("next");
      ctrl.interrupt();
      expect(prompts(sent)).toEqual([]);
      expect(cancels(sent)).toEqual([]);
      expect(ctrl.state.busy).toBe(false);
    });

    it("exit then request (the reader and the exit watcher race): closed, not busy, and nothing goes out", async () => {
      const { ctrl, sent, card } = await attached([...open, exit, acpAsk]);
      expect(card("perm-5").resolved).toBe("No longer needed");
      expect(ctrl.state).toMatchObject({ busy: false, exited: true });
      ctrl.send("next");
      ctrl.interrupt();
      expect(prompts(sent)).toEqual([]);
      expect(cancels(sent)).toEqual([]);
      expect(ctrl.state.busy).toBe(false);
    });

    it("a stop then a request: the request still asks, nothing is stuck, and it can be answered", async () => {
      const { ctrl, sent, feed, card } = await attached([...open, stopped, acpAsk]);
      expect(card("perm-5").resolved).toBeUndefined();
      expect(ctrl.state.busy).toBe(false);
      ctrl.send("next");
      expect(prompts(sent)).toHaveLength(1);
      expect(ctrl.state.queue).toEqual([]);
      feed([stopped]);
      expect(ctrl.state.busy).toBe(false);
      expect(card("perm-5").resolved).toBe("No longer needed");
    });

    it("a request a replayed turn's end closed stays closed, for a form and when the prompt failed too", async () => {
      const { card } = await attached([...open, acpAsk, form, stopped]);
      expect(card("perm-5").resolved).toBe("No longer needed");
      expect(card("elicit-6").resolved).toBe("No longer needed");
      const failed = await attached([...open, acpAsk, { jsonrpc: "2.0", id: "kelpie-prompt-2", error: { code: -1, message: "no" } }]);
      expect(failed.card("perm-5").resolved).toBe("No longer needed");
    });

    it("a live turn's end closes a replayed card even though this window never saw the turn start", async () => {
      const { ctrl, feed, card } = await attached([...open, acpAsk]);
      expect(ctrl.state.busy).toBe(false);
      feed([stopped]);
      expect(card("perm-5").resolved).toBe("No longer needed");
    });

    describe("Stop", () => {
      it("writes nothing for a request answered or expired, and cancels what is still open: the agent waits on it", async () => {
        const { ctrl, sent, card } = await attached([...open, acpAsk, answered, form, { ...acpAsk, id: 8 }, { ...form, id: 9 }, stopped, { ...acpAsk, id: 10 }, { ...form, id: 11 }]);
        expect(card("perm-10").resolved).toBeUndefined();
        expect(lines(sent)).toEqual([]);
        ctrl.interrupt();
        expect(cancels(sent).map((l) => [l.id, l.result])).toEqual([[10, { outcome: { outcome: "cancelled" } }], [11, { action: "cancel" }]]);
        expect(card("perm-10").resolved).toBe("Cancelled");
        expect(card("elicit-11").resolved).toBe("Cancelled");
      });

      it("a re-attach forgets what the replay before it held", async () => {
        const log = [...open, form];
        const { ctrl, sent } = make(createAcpAdapter, { attachLog: [log, open] });
        await ctrl.attach(1);
        await ctrl.attach(1);
        ctrl.interrupt();
        expect(lines(sent).map((l) => l.method)).toEqual(["session/cancel"]);
      });

      it("still answers a request that came live, once, and not one a phone answered", async () => {
        const { ctrl, sent, feed, card } = await attached(open);
        feed([acpAsk, { ...acpAsk, id: 8 }, form, { ...form, id: 9 }]);
        ctrl.resolvedElsewhere(8);
        ctrl.resolvedElsewhere(9);
        ctrl.interrupt();
        expect(cancels(sent).map((l) => [l.id, l.result])).toEqual([[5, { outcome: { outcome: "cancelled" } }], [6, { action: "cancel" }]]);
        expect(card("perm-5").resolved).toBe("Cancelled");
        expect(card("elicit-6").resolved).toBe("Cancelled");
        expect(card("perm-8").resolved).toBe("Answered on another device");
        ctrl.interrupt();
        expect(cancels(sent)).toHaveLength(2);
      });
    });
  });

  it("forgetting answers (a daemon restart) leaves nothing to stand in for a new request", () => {
    const { ctrl, feed, card } = make(createCodexAdapter);
    feed([codexTurn, codexAsk()]);
    ctrl.respond("approval-7", "accept", "Allow");
    ctrl.forgetSentAnswers();
    feed([codexAsk("rm")]);
    expect(card("approval-7").resolved).toBeUndefined();
    expect(card("approval-7").answered).toBeUndefined();
  });
});

describe("matchAnswer", () => {
  const options = (...labels: string[]) => labels.map((label) => ({ label }));

  it("marks the option that was chosen, and leaves what was typed", () => {
    expect(matchAnswer("Postgres", options("Postgres", "Mongo"))).toEqual({ chosen: new Set(["Postgres"]), other: "" });
    expect(matchAnswer("Postgres, Mongo, sqlite", options("Postgres", "Mongo"))).toEqual({ chosen: new Set(["Postgres", "Mongo"]), other: "sqlite" });
    expect(matchAnswer("just this", options("Postgres"))).toEqual({ chosen: new Set(), other: "just this" });
  });

  it("marks a label that contains the separator, not the labels inside it", () => {
    expect(matchAnswer("Yes, but later", options("Yes", "Yes, but later", "No"))).toEqual({ chosen: new Set(["Yes, but later"]), other: "" });
    expect(matchAnswer("Red, green, Blue", options("Red, green", "Blue"))).toEqual({ chosen: new Set(["Red, green", "Blue"]), other: "" });
  });

  it("keeps typed text whole, even around a chosen label", () => {
    expect(matchAnswer("Blue, one, two", options("Blue"))).toEqual({ chosen: new Set(["Blue"]), other: "one, two" });
  });

  it("gets through a long list pasted as Other, with labels of more than one part", () => {
    const typed = Array.from({ length: 5000 }, (_, i) => `item ${i}`);
    const { chosen, other } = matchAnswer(["Red, green", ...typed, "Blue"].join(", "), options("Red, green", "Blue", "a, b, c, d"));
    expect(chosen).toEqual(new Set(["Red, green", "Blue"]));
    expect(other).toBe(typed.join(", "));
  });
});

describe("replaying a log", () => {
  afterEach(() => vi.useRealTimers());

  const bytes = (lines: unknown[]) => new TextEncoder().encode(lines.map((l) => JSON.stringify(l)).join("\n") + "\n");
  const tool = (id: string, name = "Read") => ({ type: "assistant", message: { id: `m-${id}`, content: [{ type: "tool_use", id, name, input: { file_path: "a.ts", description: `Call ${id}` } }] } });
  const TURN = [
    { type: "kelpie_time", at: 1000 },
    { type: "system", subtype: "init", session_id: "c-1", model: "claude-opus-5-5", permissionMode: "default" },
    { type: "user", uuid: "u1", message: { content: "Look" } },
    { type: "stream_event", event: { type: "message_start", message: { id: "m1" } } },
    { type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } },
    { type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hello" } } },
    { type: "assistant", message: { id: "m1", content: [{ type: "text", text: "Hello" }], usage: { input_tokens: 10, output_tokens: 2 } } },
    tool("t1"),
    { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "text" }] } },
    { type: "control_request", request_id: "r1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "ls" } } },
    { type: "kelpie_answer", request_id: "r1", answered_by: "desktop", at: 1500, answer: { behavior: "allow" } },
    { type: "control_request", request_id: "r2", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "rm" } } },
    { type: "kelpie_time", at: 2000 },
    tool("a1", "Agent"),
    { type: "system", subtype: "task_started", task_id: "k1", tool_use_id: "a1", description: "Call a1", task_type: "local_agent", is_backgrounded: true },
    { type: "system", subtype: "task_progress", task_id: "k1", tool_use_id: "a1", description: "Reading", usage: { total_tokens: 7 } },
    { type: "system", subtype: "task_notification", task_id: "k1", tool_use_id: "a1", status: "completed", summary: "Done", usage: { total_tokens: 9 } },
    { type: "result", uuid: "res1", subtype: "success", duration_ms: 1000, num_turns: 1, total_cost_usd: 0.5, modelUsage: {} },
  ];
  const EXITED = [...TURN, tool("t2"), { type: "kelpie_exit", code: 0 }];

  /** A controller over the Claude adapter whose attach brings `log`. `sends`: what reached the agent, and when. */
  function controller(log: Uint8Array) {
    const sends: { line: any; ready: boolean; replaying: boolean }[] = [];
    const ctrl: ChatController = new ChatController("s1", "/work", null, createClaudeAdapter, {
      transport: {
        send: (line: string) => void sends.push({ line: JSON.parse(line), ready: ctrl.state.ready, replaying: (ctrl as any).isReplaying }),
        attach: async () => log,
      } as never,
    });
    return { ctrl, sends };
  }
  const snapshot = (ctrl: ChatController) => JSON.parse(JSON.stringify(ctrl.state));

  it.each([["a turn", TURN], ["an exit", EXITED]])("ends in the same state as feeding it line by line: %s", async (_name, lines) => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(50_000);
    const log = bytes(lines);
    const whole = controller(log);
    await whole.ctrl.attach(1);

    const byLine = controller(log);
    const plain = byLine.ctrl as any;
    plain.isReplaying = true;
    for (const line of lines) plain.feed(bytes([line]));
    plain.isReplaying = false;
    plain.set("ready", true);
    if (!plain.state.exited) plain.adapter.start();

    expect(snapshot(whole.ctrl)).toEqual(snapshot(byLine.ctrl));
    expect(whole.ctrl.state.items.length).toBeGreaterThan(4);
    expect(whole.ctrl.state.agentRuns[0]).toMatchObject({ id: "a1", status: "done", at: 2000, endedAt: 2000, tokens: 9 });
  });

  it("settles what watches the transcript once, at the end of the log, while still replaying", async () => {
    const { ctrl } = controller(bytes(TURN));
    const seen: [number, boolean, boolean][] = [];
    let computed = 0;
    const dispose = createRoot((dispose) => {
      const lengths = createMemo(() => {
        computed++;
        return ctrl.state.items.length;
      });
      createEffect(() => seen.push([lengths(), ctrl.state.ready, (ctrl as any).isReplaying]));
      return dispose;
    });
    computed = 0;
    await ctrl.attach(1);
    const n = ctrl.state.items.length;
    expect(n).toBeGreaterThan(4);
    expect(computed).toBeLessThanOrEqual(2); // The reset, and the log.
    // It saw the whole log before `ready`, and `ready` once the replay was over: nothing in between.
    expect(seen.filter(([len]) => len > 0)).toEqual([[n, false, true], [n, true, false]]);
    dispose();
  });

  it("keeps state current for the adapter during the feed, watched or not", async () => {
    let ctx!: AdapterContext;
    const read: unknown[] = [];
    const adapter: Adapter = {
      start() {},
      reset() {},
      send() {},
      interrupt() {},
      respond() {},
      onMessage(m) {
        ctx.upsert({ kind: "system", id: m.id, text: m.id });
        ctx.set("busy", m.busy);
        read.push([ctx.state.items.length, ctx.find(m.id)?.id, ctx.state.busy, ctx.state.busySince, ctx.replaying()]);
      },
    };
    const ctrl = new ChatController("s1", "/work", null, (c) => ((ctx = c), adapter), {
      transport: { attach: async () => bytes([{ id: "a", busy: true }, { id: "b", busy: false }, { id: "c", busy: true }]) } as never,
    });
    const dispose = createRoot((dispose) => {
      createMemo(() => [ctrl.state.items.length, ctrl.state.busy]);
      return dispose;
    });
    await ctrl.attach(1);
    expect(read).toEqual([[1, "a", true, null, true], [2, "b", false, null, true], [3, "c", true, null, true]]);
    dispose();
  });

  it("sends a queued message only after the replay, once the chat is ready", async () => {
    const { ctrl, sends } = controller(bytes(TURN));
    ctrl.send("Queued while attaching");
    expect(ctrl.state.queue).toHaveLength(1);
    await ctrl.attach(1);
    await Promise.resolve();
    await Promise.resolve();
    expect(ctrl.state.queue).toEqual([]);
    const sent = sends.filter((s) => s.line.type === "user");
    expect(sent).toHaveLength(1);
    expect(sent[0]).toMatchObject({ ready: true, replaying: false });
    expect(sends.every((s) => !s.replaying)).toBe(true);
  });
});
