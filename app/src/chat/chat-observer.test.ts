import { describe, expect, it } from "vitest";
import { createClaudeAdapter } from "./claude-adapter";
import { createCodexAdapter } from "./codex-adapter";
import { createAcpAdapter } from "./acp-adapter";
import { answeredBeforeText, answeredRequest, ChatController } from "./chat-model";
import type { AnswerOutcome, ChatTransport } from "./transport";

const INIT = JSON.stringify({ type: "system", subtype: "init", session_id: "sess-1", model: "claude-opus-5-5", permissionMode: "default" });
const ASK = JSON.stringify({ type: "control_request", request_id: "req-1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "ls" } } });

/** A transport that records everything an observer could send. */
function recorder(outcome: AnswerOutcome | (() => Promise<AnswerOutcome>) = { result: "ok" }) {
  const calls: string[] = [];
  const transport: ChatTransport = {
    send: (line) => void calls.push(`send ${line}`),
    update: (patch) => void calls.push(`update ${JSON.stringify(patch)}`),
    attach: async () => (calls.push("attach"), new Uint8Array()),
    answer: (id, line) => {
      calls.push(`answer ${id} ${line}`);
      return typeof outcome === "function" ? outcome() : Promise.resolve(outcome);
    },
  };
  return { calls, transport };
}

function observer(kind: "claude" | "codex" = "claude", outcome?: AnswerOutcome | (() => Promise<AnswerOutcome>)) {
  const { calls, transport } = recorder(outcome);
  const ctrl = new ChatController("c1", "/work", null, (ctx) => (kind === "claude" ? createClaudeAdapter(ctx) : createCodexAdapter(ctx)), { transport, observer: true });
  return { ctrl, calls };
}

describe("ChatController observer mode", () => {
  it("never writes to the agent or the session when it is given a log, replayed or live", () => {
    const { ctrl, calls } = observer();
    ctrl.applyLog("reset", [INIT, ASK]);
    ctrl.applyLog("live", [JSON.stringify({ type: "control_request", request_id: "req-2", request: { subtype: "can_use_tool", tool_name: "Edit", input: {} } })]);
    expect(ctrl.state.ready).toBe(true);
    expect(ctrl.state.conversationId).toBe("sess-1");
    expect(ctrl.state.items.filter((i) => i.kind === "permission")).toHaveLength(2);
    expect(calls).toEqual([]);
  });

  it("does not attach, send, queue or flush a queue", async () => {
    const { ctrl, calls } = observer();
    await ctrl.attach(7);
    ctrl.applyLog("reset", [INIT]);
    ctrl.send("hello");
    ctrl.set("queue", [{ id: "q", text: "x", typed: true }]);
    ctrl.set("busy", false);
    await Promise.resolve();
    expect(ctrl.state.queue).toHaveLength(1);
    expect(calls).toEqual([]);
  });

  it("keeps no picture bytes from the log: it shows only their names", () => {
    const { ctrl } = observer();
    ctrl.applyLog("reset", [INIT, JSON.stringify({ type: "user", uuid: "u1", message: { role: "user", content: [{ type: "image", source: { type: "base64", media_type: "image/png", data: "QUJD" } }, { type: "text", text: "look" }] } })]);
    expect(JSON.stringify(ctrl.state.items)).not.toContain("QUJD");
    expect(ctrl.state.items.at(-1)).toMatchObject({ kind: "user", text: "look", images: [{ name: "Picture 1" }] });
  });

  it("applies a delta on top of what is shown instead of starting over", () => {
    const { ctrl } = observer();
    ctrl.applyLog("reset", [INIT, JSON.stringify({ type: "user", message: { role: "user", content: "first" } })]);
    const before = ctrl.state.items.length;
    ctrl.applyLog("delta", [JSON.stringify({ type: "user", message: { role: "user", content: "second" } })]);
    expect(ctrl.state.items.length).toBe(before + 1);
    ctrl.applyLog("reset", [INIT]);
    expect(ctrl.state.items.some((i) => i.kind === "user")).toBe(false);
  });

  it("a person's answer goes out as chat_answer for that request, once", async () => {
    const { ctrl, calls } = observer();
    ctrl.applyLog("reset", [INIT, ASK]);
    ctrl.respond("req-1", "allow", "Allowed");
    ctrl.respond("req-1", "allow", "Allowed");
    expect(calls).toHaveLength(1);
    const [, id, line] = /^answer (\S+) (.*)$/.exec(calls[0])!;
    expect(id).toBe("req-1");
    expect(JSON.parse(line)).toMatchObject({ type: "control_response", response: { request_id: "req-1", response: { behavior: "allow" } } });
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ answering: true });
    await new Promise((r) => setTimeout(r, 0));
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ answering: false, resolved: "Allowed" });
  });

  it("a Codex approval is answered by its JSON-RPC id", () => {
    const { ctrl, calls } = observer("codex");
    ctrl.applyLog("reset", [JSON.stringify({ id: 7, method: "item/commandExecution/requestApproval", params: { threadId: "t", turnId: "u", itemId: "i", command: "ls" } })]);
    const card = ctrl.state.items.find((i) => i.kind === "permission");
    expect(card).toBeTruthy();
    ctrl.respond(card!.id, "accept", "Allowed");
    expect(calls).toEqual([`answer 7 ${JSON.stringify({ id: 7, result: { decision: "accept" } })}`]);
  });

  it("shows who answered first when the daemon says the answer is stale", async () => {
    const { ctrl } = observer("claude", { result: "stale", answeredBy: "desktop", at: Date.now() - 125_000 });
    ctrl.applyLog("reset", [INIT, ASK]);
    ctrl.respond("req-1", "allow", "Allowed");
    await new Promise((r) => setTimeout(r, 0));
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Already answered on the desktop 2 min ago" });
  });

  it("an answer that may not have gone through stays open", async () => {
    const { ctrl } = observer("claude", { result: "unknown" });
    ctrl.applyLog("reset", [INIT, ASK]);
    ctrl.respond("req-1", "allow", "Allowed");
    await new Promise((r) => setTimeout(r, 0));
    const card = ctrl.state.items.find((i) => i.id === "req-1");
    expect(card).toMatchObject({ answering: false, retry: "Allowed" });
    expect((card as { resolved?: string }).resolved).toBeUndefined();
    expect((card as { answerError?: string }).answerError).toMatch(/Not sure/);
  });

  it("an answer that is not a request the daemon tracks says so instead of doing nothing", async () => {
    const { ctrl, calls } = observer("codex");
    ctrl.upsert({ kind: "permission", id: "auth-card", title: "Sign in", options: [{ id: "retry", label: "Retry" }] });
    ctrl.respond("auth-card", "retry", "Retry");
    await new Promise((r) => setTimeout(r, 0));
    expect(calls).toEqual([]);
    expect((ctrl.state.items.find((i) => i.id === "auth-card") as { answerError?: string }).answerError).toMatch(/computer/);
  });

  describe("asking again after an answer whose fate is unknown or refused", () => {
    const cases: [string, "claude" | "codex" | "acp", string[], string, string][] = [
      ["claude", "claude", [INIT, ASK], "req-1", "allow"],
      ["codex", "codex", [JSON.stringify({ id: 7, method: "item/commandExecution/requestApproval", params: { threadId: "t", turnId: "u", itemId: "i", command: "ls" } })], "approval-7", "accept"],
      ["acp", "acp", [JSON.stringify({ jsonrpc: "2.0", id: 5, method: "session/request_permission", params: { toolCall: { title: "Run ls" }, options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }] } })], "perm-5", "allow"],
    ];
    for (const outcome of ["unknown", "refused"] as const) {
      for (const [name, kind, log, card, option] of cases) {
        it(`${name}: the same bytes go out again after ${outcome}, and the adapter is asked once`, async () => {
          const { calls, transport } = recorder(outcome === "unknown" ? { result: "unknown" } : { result: "refused", message: "offline" });
          const make = kind === "claude" ? createClaudeAdapter : kind === "codex" ? createCodexAdapter : createAcpAdapter;
          let asked = 0;
          const ctrl = new ChatController("c", "/work", null, (ctx) => {
            const adapter = make(ctx);
            const respond = adapter.respond.bind(adapter);
            adapter.respond = (...args) => (asked++, respond(...args));
            return adapter;
          }, { transport, observer: true });
          ctrl.applyLog("reset", log);
          ctrl.respond(card, option, "Allowed");
          await new Promise((r) => setTimeout(r, 0));
          expect(calls).toHaveLength(1);
          // An ordinary button never stands in for the saved line: it is ignored while "Send again" is pending.
          ctrl.respond(card, "deny", "Denied");
          await new Promise((r) => setTimeout(r, 0));
          expect(calls).toHaveLength(1);
          // "Send again" is the one way to send it once more, and it sends the same bytes each time.
          ctrl.retryAnswer(card);
          await new Promise((r) => setTimeout(r, 0));
          ctrl.retryAnswer(card);
          await new Promise((r) => setTimeout(r, 0));
          expect(calls).toHaveLength(3);
          expect(calls[1]).toBe(calls[0]);
          expect(calls[2]).toBe(calls[0]);
          expect(asked).toBe(1);
          if (kind === "claude") expect(JSON.parse(calls[1].replace(/^answer \S+ /, ""))).toMatchObject({ response: { response: { behavior: "allow", updatedInput: { command: "ls" } } } });
        });
      }
    }

    it("the card settles on what was first sent once the repeat gets through", async () => {
      let n = 0;
      const { ctrl } = observer("claude", () => Promise.resolve<AnswerOutcome>(++n === 1 ? { result: "unknown" } : { result: "dup" }));
      ctrl.applyLog("reset", [INIT, ASK]);
      ctrl.respond("req-1", "allow", "Allowed");
      await new Promise((r) => setTimeout(r, 0));
      ctrl.retryAnswer("req-1");
      await new Promise((r) => setTimeout(r, 0));
      const card = ctrl.state.items.find((i) => i.id === "req-1") as { resolved?: string; retry?: string };
      expect(card.resolved).toBe("Allowed");
      expect(card.retry).toBeUndefined();
    });
  });

  it("a resolved event with who and when says so", () => {
    const { ctrl } = observer("claude");
    ctrl.applyLog("reset", [INIT, ASK]);
    ctrl.resolvedElsewhere("req-1", false, { answeredBy: "desktop", at: Date.now() - 125_000 });
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Already answered on the desktop 2 min ago" });
  });

  it("an answer in flight is not overwritten by the daemon's resolved event", async () => {
    let settle!: (o: AnswerOutcome) => void;
    const { ctrl } = observer("claude", () => new Promise<AnswerOutcome>((r) => (settle = r)));
    ctrl.applyLog("reset", [INIT, ASK]);
    ctrl.respond("req-1", "allow", "Allowed");
    ctrl.resolvedElsewhere("req-1");
    expect((ctrl.state.items.find((i) => i.id === "req-1") as { resolved?: string }).resolved).toBeUndefined();
    settle({ result: "dup" });
    await new Promise((r) => setTimeout(r, 0));
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Allowed" });
  });

  describe("an answer kept in the log", () => {
    const logged = (by: string, answer: unknown = { behavior: "allow", lasting: false }) => JSON.stringify({ type: "kelpie_answer", request_id: "req-1", by: by === "desktop" ? "desktop" : "phone", answered_by: by, at: Date.now() - 125_000, answer });
    /** A phone named Pixel whose answer's reply was lost with the connection. */
    const lostReply = async () => {
      const { calls, transport } = recorder({ result: "unknown" });
      const ctrl = new ChatController("c1", "/work", null, createClaudeAdapter, { transport: { ...transport, deviceName: () => "Pixel" }, observer: true });
      ctrl.applyLog("reset", [INIT, ASK]);
      ctrl.respond("req-1", "allow", "Allowed");
      await new Promise((r) => setTimeout(r, 0));
      expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ retry: "Allowed", answerError: expect.stringMatching(/Not sure/) });
      return { ctrl, calls, card: () => ctrl.state.items.find((i) => i.id === "req-1") as { resolved?: string; retry?: string; answerError?: string; answered?: { by?: string } } };
    };

    it("a lost reply's own answer, back on reconnect, settles the card in the words it was given", async () => {
      const { ctrl, calls, card } = await lostReply();
      ctrl.applyLog("delta", [logged("Pixel")]);
      expect(card().resolved).toBe("Allowed");
      expect(card().retry).toBeUndefined();
      expect(card().answerError).toBeUndefined();
      expect(card().answered?.by).toBeUndefined();
      // Nothing is left to send again.
      ctrl.retryAnswer("req-1");
      expect(calls).toHaveLength(1);
    });

    it("a lost reply and somebody else's answer: says who, and nothing is left to send again", async () => {
      const { ctrl, calls, card } = await lostReply();
      ctrl.applyLog("delta", [logged("Tablet")]);
      expect(card()).toMatchObject({ resolved: "Already answered on Tablet 2 min ago", answered: { by: "Tablet" } });
      expect(card().retry).toBeUndefined();
      expect(card().answerError).toBeUndefined();
      ctrl.retryAnswer("req-1");
      expect(calls).toHaveLength(1);
    });

    it("a page reloaded on this device reads its own answer as its own", () => {
      const { transport } = recorder();
      const ctrl = new ChatController("c1", "/work", null, createClaudeAdapter, { transport: { ...transport, deviceName: () => "Pixel" }, observer: true });
      ctrl.applyLog("reset", [INIT, ASK, logged("Pixel")]);
      const card = ctrl.state.items.find((i) => i.id === "req-1") as { resolved?: string; answered?: { by?: string; label: string } };
      expect(card.resolved).toBe("Allow");
      expect(card.answered).toMatchObject({ label: "Allow" });
      expect(card.answered?.by).toBeUndefined();
    });

    it("settles a card on a log that is loaded, and passes nothing on", () => {
      const { ctrl, calls } = observer();
      ctrl.applyLog("reset", [INIT, ASK, logged("Pixel")]);
      expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Already answered on Pixel 2 min ago", answered: { label: "Allow", by: "Pixel" } });
      expect(calls).toEqual([]);
    });

    it("a window's answer is another device's, to a phone", () => {
      const { ctrl } = observer();
      ctrl.applyLog("reset", [INIT, ASK, logged("desktop")]);
      expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Already answered on the desktop 2 min ago", answered: { by: "desktop" } });
    });

    it("the phone's own answer stays its own: on the way, once settled and when the log comes again", async () => {
      let settle!: (o: AnswerOutcome) => void;
      const { ctrl, calls } = observer("claude", () => new Promise<AnswerOutcome>((r) => (settle = r)));
      ctrl.applyLog("reset", [INIT, ASK]);
      ctrl.respond("req-1", "allow", "Allowed");
      ctrl.applyLog("live", [logged("Pixel")]);
      const card = () => ctrl.state.items.find((i) => i.id === "req-1") as { resolved?: string; answering?: boolean };
      expect(card().resolved).toBeUndefined();
      expect(card().answering).toBe(true);
      settle({ result: "ok" });
      await new Promise((r) => setTimeout(r, 0));
      expect(card().resolved).toBe("Allowed");
      ctrl.applyLog("live", [logged("Pixel")]);
      expect(card().resolved).toBe("Allowed");
      ctrl.applyLog("reset", [INIT, ASK, logged("Pixel")]);
      expect(card().resolved).toBe("Allowed");
      expect(calls).toHaveLength(1);
    });
  });

  it("the desktop controller still writes the answer as a line to the agent, and no chat_answer", () => {
    const { calls, transport } = recorder();
    const ctrl = new ChatController("c2", "/work", null, (ctx) => createClaudeAdapter(ctx), { transport });
    ctrl.upsert({ kind: "permission", id: "req-1", title: "Allow?", options: [{ id: "allow", label: "Allow" }] });
    ctrl.respond("req-1", "allow", "Allowed");
    expect(calls).toHaveLength(1);
    expect(calls[0].startsWith('send {"type":"control_response"')).toBe(true);
    expect(calls[0].endsWith("\n")).toBe(true);
    expect(ctrl.state.items.find((i) => i.id === "req-1")).toMatchObject({ resolved: "Allowed" });
  });
});

describe("a saved answer belongs to one request", () => {
  const claude = (command: string) => JSON.stringify({ type: "control_request", request_id: "req-1", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command } } });
  const codex = (command: string) => JSON.stringify({ id: 7, method: "item/commandExecution/requestApproval", params: { threadId: "t", turnId: "u", itemId: "i", command } });
  const acp = (title: string) => JSON.stringify({ jsonrpc: "2.0", id: 5, method: "session/request_permission", params: { toolCall: { title }, options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }, { optionId: "reject", name: "Reject", kind: "reject_once" }] } });
  type Kind = "claude" | "codex" | "acp";
  const dialects: { kind: Kind; first: string[]; second: string[]; card: string; allow: string; deny: string; says: (line: string) => string }[] = [
    { kind: "claude", first: [INIT, claude("ls")], second: [INIT, claude("rm -rf ~")], card: "req-1", allow: "allow", deny: "deny", says: (l) => JSON.parse(l).response.response.behavior },
    { kind: "codex", first: [codex("ls")], second: [codex("rm -rf ~")], card: "approval-7", allow: "accept", deny: "decline", says: (l) => JSON.parse(l).result.decision },
    { kind: "acp", first: [acp("Run ls")], second: [acp("Run rm -rf ~")], card: "perm-5", allow: "allow", deny: "reject", says: (l) => JSON.parse(l).result.outcome.optionId },
  ];
  const lineOf = (call: string) => call.replace(/^answer \S+ /, "");
  const make = (kind: Kind, outcome: AnswerOutcome) => {
    const { calls, transport } = recorder(outcome);
    const adapter = kind === "claude" ? createClaudeAdapter : kind === "codex" ? createCodexAdapter : createAcpAdapter;
    const ctrl = new ChatController("c", "/work", null, adapter, { transport, observer: true });
    return { ctrl, calls };
  };
  const tick = () => new Promise((r) => setTimeout(r, 0));

  for (const d of dialects) {
    it(`${d.kind}: after the log is loaded again, Deny sends Deny and not the earlier answer`, async () => {
      const { ctrl, calls } = make(d.kind, { result: "unknown" });
      ctrl.applyLog("reset", d.first);
      ctrl.respond(d.card, d.allow, "Allowed");
      await tick();
      expect(d.says(lineOf(calls[0]))).toBe(d.allow);
      ctrl.applyLog("reset", d.first);
      expect((ctrl.state.items.find((i) => i.id === d.card) as { retry?: string }).retry).toBeUndefined();
      ctrl.respond(d.card, d.deny, "Denied");
      await tick();
      expect(calls).toHaveLength(2);
      expect(d.says(lineOf(calls[1]))).toBe(d.deny);
    });

    it(`${d.kind}: a new request that reuses the id after a restart is answered as itself`, async () => {
      const { ctrl, calls } = make(d.kind, { result: "unknown" });
      ctrl.applyLog("reset", d.first);
      ctrl.respond(d.card, d.allow, "Allowed");
      await tick();
      // The daemon restarted: same id, another request. (What ChatFeed.forgetPositions does on an epoch change.)
      ctrl.forgetSentAnswers();
      ctrl.applyLog("reset", d.second);
      const card = ctrl.state.items.find((i) => i.id === d.card) as { retry?: string; resolved?: string };
      expect(card.retry).toBeUndefined();
      ctrl.respond(d.card, d.deny, "Denied");
      await tick();
      expect(calls).toHaveLength(2);
      expect(d.says(lineOf(calls[1]))).toBe(d.deny);
      if (d.kind === "claude") expect(JSON.parse(lineOf(calls[1])).response.response.message).toMatch(/denied/i);
    });

    it(`${d.kind}: an answer that was accepted does not mark the next request with the same id as answered`, async () => {
      const { ctrl, calls } = make(d.kind, { result: "ok" });
      ctrl.applyLog("reset", d.first);
      ctrl.respond(d.card, d.allow, "Allowed");
      await tick();
      expect(ctrl.state.items.find((i) => i.id === d.card)).toMatchObject({ resolved: "Allowed" });
      ctrl.forgetSentAnswers();
      ctrl.applyLog("reset", d.second);
      expect((ctrl.state.items.find((i) => i.id === d.card) as { resolved?: string }).resolved).toBeUndefined();
      ctrl.respond(d.card, d.allow, "Allowed");
      await tick();
      expect(calls).toHaveLength(2);
    });

    it(`${d.kind}: "Send again" still resends the identical bytes, also after the same log came again as a delta`, async () => {
      const { ctrl, calls } = make(d.kind, { result: "unknown" });
      ctrl.applyLog("reset", d.first);
      ctrl.respond(d.card, d.allow, "Allowed");
      await tick();
      expect(ctrl.state.items.find((i) => i.id === d.card)).toMatchObject({ retry: "Allowed" });
      // The same request arrives again without a reset: the card keeps its "Send again".
      ctrl.applyLog("delta", d.first);
      expect(ctrl.state.items.find((i) => i.id === d.card)).toMatchObject({ retry: "Allowed" });
      ctrl.retryAnswer(d.card);
      await tick();
      expect(calls).toHaveLength(2);
      expect(calls[1]).toBe(calls[0]);
    });
  }

  for (const d of dialects) {
    it(`${d.kind}: a log loaded again while the answer is on its way keeps the card locked, and it settles on the answer that was sent`, async () => {
      let reply!: (o: AnswerOutcome) => void;
      const { calls, transport } = recorder(() => new Promise<AnswerOutcome>((r) => (reply = r)));
      const adapter = d.kind === "claude" ? createClaudeAdapter : d.kind === "codex" ? createCodexAdapter : createAcpAdapter;
      const ctrl = new ChatController("c", "/work", null, adapter, { transport, observer: true });
      ctrl.applyLog("reset", d.first);
      ctrl.respond(d.card, d.allow, "Allowed");
      await tick();
      expect(calls).toHaveLength(1);
      // The log comes again (a stale position, a resync) while the reply is awaited: the card is rebuilt.
      ctrl.applyLog("reset", d.first);
      expect(ctrl.state.items.find((i) => i.id === d.card)).toMatchObject({ answering: true });
      ctrl.respond(d.card, d.deny, "Denied");
      await tick();
      expect(calls).toHaveLength(1);
      reply({ result: "ok" });
      await tick();
      expect(ctrl.state.items.find((i) => i.id === d.card)).toMatchObject({ resolved: "Allowed", answering: false });
      expect(d.says(lineOf(calls[0]))).toBe(d.allow);
    });
  }

  it("a request that came after a restart is not locked by the answer to the old one, and its reply is ignored", async () => {
    let reply!: (o: AnswerOutcome) => void;
    const { calls, transport } = recorder(() => new Promise<AnswerOutcome>((r) => (reply = r)));
    const ctrl = new ChatController("c", "/work", null, createCodexAdapter, { transport, observer: true });
    ctrl.applyLog("reset", [codex("ls")]);
    ctrl.respond("approval-7", "accept", "Allowed");
    await tick();
    ctrl.forgetSentAnswers();
    ctrl.applyLog("reset", [codex("rm -rf ~")]);
    expect((ctrl.state.items.find((i) => i.id === "approval-7") as { answering?: boolean }).answering).toBeFalsy();
    reply({ result: "ok" });
    await tick();
    expect((ctrl.state.items.find((i) => i.id === "approval-7") as { resolved?: string }).resolved).toBeUndefined();
    ctrl.respond("approval-7", "decline", "Denied");
    await tick();
    expect(calls).toHaveLength(2);
  });

  it("a request that reuses an id does not keep the old 'not sure' message", async () => {
    const { ctrl } = make("codex", { result: "unknown" });
    ctrl.applyLog("reset", [codex("ls")]);
    ctrl.respond("approval-7", "accept", "Allowed");
    await tick();
    expect((ctrl.state.items.find((i) => i.id === "approval-7") as { answerError?: string }).answerError).toMatch(/Not sure/);
    // The same request shown again keeps its message...
    ctrl.applyLog("delta", [codex("ls")]);
    expect((ctrl.state.items.find((i) => i.id === "approval-7") as { answerError?: string }).answerError).toMatch(/Not sure/);
    // ...a different one under the same id (live, no reset in between) does not.
    ctrl.forgetSentAnswers();
    ctrl.applyLog("live", [codex("rm -rf ~")]);
    const card = ctrl.state.items.find((i) => i.id === "approval-7") as { answerError?: string; retry?: string; answering?: boolean };
    expect(card.answerError).toBeUndefined();
    expect(card.retry).toBeUndefined();
    expect(card.answering).toBeFalsy();
  });

  it("a card that stopped being the request that was answered is not given the saved line", async () => {
    const { ctrl, calls } = make("codex", { result: "unknown" });
    ctrl.applyLog("reset", [codex("ls")]);
    ctrl.respond("approval-7", "accept", "Allowed");
    await tick();
    // A different request with the same id arrives without a reset or an epoch change.
    ctrl.upsert({ kind: "permission", id: "approval-7", title: "Run rm -rf ~?", options: [{ id: "accept", label: "Allow" }] });
    expect((ctrl.state.items.find((i) => i.id === "approval-7") as { retry?: string }).retry).toBeUndefined();
    ctrl.retryAnswer("approval-7");
    await tick();
    expect(calls).toHaveLength(1);
  });
});

describe("answers", () => {
  it("names the request an agent's answer is for, and nothing else", () => {
    expect(answeredRequest({ type: "control_response", response: { subtype: "success", request_id: "r", response: {} } })).toBe("r");
    expect(answeredRequest({ jsonrpc: "2.0", id: 5, result: {} })).toBe(5);
    expect(answeredRequest({ id: "a", result: { decision: "accept" } })).toBe("a");
    expect(answeredRequest({ jsonrpc: "2.0", id: 5, method: "initialize", params: {} })).toBeNull();
    expect(answeredRequest({ type: "user", message: {} })).toBeNull();
    expect(answeredRequest(null)).toBeNull();
  });

  it("says when and by whom an answer was given first", () => {
    const now = 1_000_000_000;
    expect(answeredBeforeText("desktop", now - 120_000, now)).toBe("Already answered on the desktop 2 min ago");
    expect(answeredBeforeText("Pixel", now - 5_000, now)).toBe("Already answered on Pixel just now");
    expect(answeredBeforeText(undefined, undefined, now)).toBe("Already answered on the desktop");
    expect(answeredBeforeText("Pixel", now - 7_300_000, now)).toBe("Already answered on Pixel 2 h ago");
  });
});

describe("observer mode drops every automatic write", () => {
  const acp = () => {
    const { calls, transport } = recorder();
    const ctrl = new ChatController("a1", "/work", null, (ctx) => createAcpAdapter(ctx), { transport, observer: true });
    return { ctrl, calls };
  };

  it("ACP: the decline for a URL-mode elicitation and the error for an unsupported client method, live", () => {
    const { ctrl, calls } = acp();
    ctrl.applyLog("live", [
      JSON.stringify({ jsonrpc: "2.0", id: 9, method: "elicitation/create", params: { mode: "url", url: "https://example.com", message: "open" } }),
      JSON.stringify({ jsonrpc: "2.0", id: 3, method: "fs/read_text_file", params: { path: "x" } }),
      JSON.stringify({ jsonrpc: "2.0", id: 4, method: "terminal/create", params: {} }),
    ]);
    expect(calls).toEqual([]);
  });

  it("ACP: the same line on a desktop controller is answered, so the drop above is the observer's doing", () => {
    const { calls, transport } = recorder();
    const ctrl = new ChatController("a2", "/work", null, (ctx) => createAcpAdapter(ctx), { transport });
    (ctrl as unknown as { feedText(t: string): void }).feedText(JSON.stringify({ jsonrpc: "2.0", id: 3, method: "fs/read_text_file", params: {} }) + "\n");
    expect(calls.filter((c) => c.startsWith("send"))).toHaveLength(1);
  });

  it("ACP: a person's answer to a permission still goes out as chat_answer", () => {
    const { ctrl, calls } = acp();
    ctrl.applyLog("reset", [JSON.stringify({ jsonrpc: "2.0", id: 5, method: "session/request_permission", params: { toolCall: { title: "Run ls" }, options: [{ optionId: "allow", name: "Allow", kind: "allow_once" }] } })]);
    ctrl.respond("perm-5", "allow", "Allow");
    expect(calls).toHaveLength(1);
    expect(calls[0].startsWith("answer 5 ")).toBe(true);
    expect(JSON.parse(calls[0].slice("answer 5 ".length))).toMatchObject({ id: 5, result: { outcome: { optionId: "allow" } } });
  });

  it("nothing goes out on attach, on queue flush, for a setting change or an interrupt, whichever adapter", async () => {
    for (const make of [createClaudeAdapter, createCodexAdapter, createAcpAdapter]) {
      const { calls, transport } = recorder();
      const ctrl = new ChatController("x", "/work", null, make, { transport, observer: true });
      await ctrl.attach(1);
      ctrl.applyLog("reset", [INIT]);
      ctrl.applyLog("live", [JSON.stringify({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "s-1" } })]);
      ctrl.set("queue", [{ id: "q", text: "hi", typed: true }]);
      ctrl.set("busy", true);
      ctrl.set("busy", false);
      ctrl.setSetting("model", "other");
      ctrl.interrupt();
      await Promise.resolve();
      expect(calls).toEqual([]);
      expect(ctrl.state.queue).toHaveLength(1);
    }
  });
});
