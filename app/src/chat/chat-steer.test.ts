// Saying something while the agent works, as in its own terminal. The Claude lines follow what `claude -p` 2.1.316 printed
// for a message written to its stdin mid-turn: the echo comes when the running tools finish; a turn that ends first gets its
// `result`, and then the message runs as a turn of its own.
import { describe, expect, it } from "vitest";
import { ChatController, type ChatImage } from "./chat-model";
import { createClaudeAdapter } from "./claude-adapter";
import { createAcpAdapter } from "./acp-adapter";
import { createCodexAdapter } from "./codex-adapter";
import type { ChatTransport } from "./transport";

const line = (o: unknown) => `${JSON.stringify(o)}\n`;
const encode = (s: string) => new TextEncoder().encode(s);
const toolUse = (id: string) => line({ type: "assistant", message: { id: `m-${id}`, role: "assistant", content: [{ type: "tool_use", id, name: "Bash", input: { command: "sleep 5" } }] } });
const toolResult = (id: string) => line({ type: "user", message: { role: "user", content: [{ type: "tool_result", tool_use_id: id, content: "ok" }] } });
const echo = (text: string) => line({ type: "user", isReplay: true, uuid: `u-${text}`, message: { role: "user", content: [{ type: "text", text }] } });
const reply = (id: string, text: string) => line({ type: "assistant", message: { id, role: "assistant", content: [{ type: "text", text }] } });
const result = (uuid: string) => line({ type: "result", subtype: "success", uuid, num_turns: 1, result: "" });

async function chat(make: typeof createClaudeAdapter = createClaudeAdapter, opts: { daemonQueue?: boolean; log?: string } = {}) {
  const written: string[] = [];
  const queued: string[] = [];
  let live: (b: Uint8Array) => void = () => {};
  const transport: ChatTransport = {
    attach: async (_h, onLive) => ((live = onLive), encode(opts.log ?? "")),
    send: (l) => void written.push(l),
    ...(opts.daemonQueue
      ? {
          enqueue: async (_cid: string, text: string) => (queued.push(text), { qid: `q${queued.length}` }),
          queueOp: async () => {},
        }
      : {}),
  } as ChatTransport;
  const ctrl = new ChatController("s", "/w", null, make, { transport, daemonQueue: () => !!opts.daemonQueue });
  await ctrl.attach(1);
  written.length = 0;
  const said = () => written.flatMap((l) => {
    const m = JSON.parse(l);
    return m.type === "user" ? [m.message.content.map((b: { text?: string }) => b.text ?? "").join("")] : [];
  });
  return { ctrl, said, written, queued, print: (s: string) => live(encode(s)) };
}

const pending = (c: ChatController) => c.state.items.filter((i) => i.kind === "user" && i.pending).map((i) => (i as { text: string }).text);

describe("a message sent while the agent works", () => {
  it("goes into the running turn at once, and stays faded until the agent reads it", async () => {
    const { ctrl, said, print } = await chat();
    ctrl.send("do the thing");
    print(echo("do the thing") + toolUse("t1"));
    ctrl.send("use zod instead");
    expect(said()).toEqual(["do the thing", "use zod instead"]);
    expect(ctrl.state.queue).toEqual([]);
    expect(pending(ctrl)).toEqual(["use zod instead"]);
    print(toolResult("t1") + echo("use zod instead"));
    expect(pending(ctrl)).toEqual([]);
    print(reply("m2", "done") + result("r1"));
    expect(ctrl.state.busy).toBe(false);
  });

  it("stays working when the turn ends without reading it, until the turn that reads it ends", async () => {
    const { ctrl, print } = await chat();
    ctrl.send("describe a river");
    print(echo("describe a river"));
    ctrl.send("now say pineapple");
    print(reply("m1", "A river.") + result("r1"));
    expect(ctrl.state.busy).toBe(true);
    print(echo("now say pineapple") + reply("m2", "pineapple"));
    expect(ctrl.state.busy).toBe(true);
    print(result("r2"));
    expect(ctrl.state.busy).toBe(false);
    expect(pending(ctrl)).toEqual([]);
  });

  it("does not stay working for ever when the echo never comes", async () => {
    const { ctrl, print } = await chat();
    ctrl.send("one");
    print(echo("one"));
    ctrl.send("never echoed");
    print(result("r1"));
    expect(ctrl.state.busy).toBe(true);
    print(result("r2"));
    expect(ctrl.state.busy).toBe(false);
  });

  it("keeps working through two unread messages, one turn each", async () => {
    const { ctrl, print } = await chat();
    ctrl.send("one");
    print(echo("one"));
    ctrl.send("two");
    ctrl.send("three");
    print(result("r1"));
    expect(ctrl.state.busy).toBe(true);
    print(echo("two") + result("r2"));
    expect(ctrl.state.busy).toBe(true);
    print(echo("three") + result("r3"));
    expect(ctrl.state.busy).toBe(false);
  });

  it("does not hold back a normal turn that has nothing unread", async () => {
    const { ctrl, print } = await chat();
    ctrl.send("one");
    print(echo("one") + result("r1"));
    expect(ctrl.state.busy).toBe(false);
  });

  for (const [name, text] of [["a command", "/compact"], ["a shell line", "!ls"]] as const) {
    it(`waits for the turn to end when it is ${name}`, async () => {
      const { ctrl, said, print } = await chat();
      ctrl.send("one");
      print(echo("one"));
      ctrl.send(text, true);
      expect(said()).toEqual(["one"]);
      expect(ctrl.state.queue.map((q) => q.text)).toEqual([text]);
    });
  }

  const picture: ChatImage = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
  const pictureBlock = { type: "image", source: { type: "base64", media_type: "image/png", data: "AA==" } };

  it("goes in with its pictures, which Claude takes mid-turn", async () => {
    const { ctrl, written, print } = await chat();
    ctrl.send("one");
    print(echo("one") + toolUse("t1"));
    ctrl.send("look", true, [picture]);
    expect(ctrl.state.queue).toEqual([]);
    expect(JSON.parse(written.at(-1)!).message.content).toEqual([pictureBlock, { type: "text", text: "look" }]);
    expect(pending(ctrl)).toEqual(["look"]);
    // 2.1.325 echoes it, picture and all, after the running tool's result, and one `success` result covers both.
    print(toolResult("t1") + line({ type: "user", isReplay: true, uuid: "u-look", message: { role: "user", content: [pictureBlock, { type: "text", text: "look" }] } }));
    expect(pending(ctrl)).toEqual([]);
    print(reply("m2", "FINISHED. The picture is red.") + result("r1"));
    expect(ctrl.state.busy).toBe(false);
  });

  it("sends a picture waiting behind a command now by steering it, with no stop", async () => {
    const { ctrl, written, print } = await chat();
    ctrl.send("one");
    print(echo("one") + toolUse("t1"));
    ctrl.send("/compact");
    ctrl.send("look", true, [picture]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["/compact", "look"]);
    ctrl.sendNow(ctrl.state.queue[1].id);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["/compact"]);
    const lines = written.map((l) => JSON.parse(l));
    expect(lines.some((l) => l.type === "control_request" && l.request?.subtype === "interrupt")).toBe(false);
    expect(lines.at(-1).message.content).toEqual([pictureBlock, { type: "text", text: "look" }]);
  });

  it("goes in with its pictures past the daemon's queue too", async () => {
    const { ctrl, written, queued, print } = await chat(createClaudeAdapter, { daemonQueue: true });
    print(toolUse("t1"));
    ctrl.set("busy", true);
    ctrl.send("look", true, [picture]);
    expect(queued).toEqual([]);
    expect(JSON.parse(written.at(-1)!).message.content).toEqual([pictureBlock, { type: "text", text: "look" }]);
  });

  it("queues a picture for an agent that takes only text mid-turn", async () => {
    const { ctrl, said } = await chat(createCodexAdapter as typeof createClaudeAdapter);
    ctrl.set("busy", true);
    ctrl.send("look", true, [picture]);
    expect(said()).toEqual([]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["look"]);
  });

  it("stays behind what already waits, so that messages keep their order", async () => {
    const { ctrl, said, print } = await chat();
    ctrl.send("one");
    print(echo("one"));
    ctrl.send("/compact");
    ctrl.send("after it");
    expect(said()).toEqual(["one"]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["/compact", "after it"]);
  });

  it("goes the same way past the daemon's queue, which only holds what cannot go in", async () => {
    const { ctrl, said, queued, print } = await chat(createClaudeAdapter, { daemonQueue: true });
    print(toolUse("t1"));
    ctrl.set("busy", true);
    ctrl.send("use zod");
    expect(said()).toEqual(["use zod"]);
    expect(queued).toEqual([]);
    ctrl.send("/compact");
    await Promise.resolve();
    expect(queued).toEqual(["/compact"]);
  });

  it("queues for an agent that cannot take a message mid-turn", async () => {
    const { ctrl, said } = await chat(createAcpAdapter as typeof createClaudeAdapter);
    ctrl.set("busy", true);
    ctrl.send("later");
    expect(said()).toEqual([]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["later"]);
  });

  it("is queued, not written, before the chat is attached", () => {
    const written: string[] = [];
    const ctrl = new ChatController("s", "/w", null, createClaudeAdapter, { transport: { send: (l) => void written.push(l) } });
    ctrl.set({ ready: true, busy: true });
    ctrl.send("too early");
    expect(written).toEqual([]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["too early"]);
  });

  it("says nothing to the agent when a replayed log held such a turn", async () => {
    const log = echo("one") + toolUse("t1") + toolResult("t1") + echo("two") + result("r1");
    const { ctrl, said } = await chat(createClaudeAdapter, { log });
    expect(said()).toEqual([]);
    expect(ctrl.state.busy).toBe(false);
  });
});

// Lines as `claude -p` 2.1.325 printed them (C:\Temp\steer-probe): an interrupt is acknowledged at once, the run then writes a
// `[Request interrupted by user…]` user line and ends with `error_during_execution`; a user line it had not read yet starts a
// new run at once (`system/init`), whose echo comes a few seconds later.
const init = line({ type: "system", subtype: "init", session_id: "c1", model: "claude-haiku-4-5" });
const ack = (n: number) => line({ type: "control_response", response: { subtype: "success", request_id: `kelpie-interrupt-${n}` } });
const marker = line({ type: "user", message: { role: "user", content: [{ type: "text", text: "[Request interrupted by user for tool use]" }] } });
const stoppedResult = (uuid: string) =>
  line({ type: "result", subtype: "error_during_execution", is_error: true, uuid, result: null, num_turns: 3, stop_reason: "tool_use", duration_ms: 1300, errors: ["[ede_diagnostic] result_type=user last_content_type=n/a stop_reason=tool_use"] });
const results = (c: ChatController) => c.state.items.filter((i) => i.kind === "result").map((i) => (i as { text: string }).text);

describe("a result the CLI follows with more work", () => {
  it("does not end the turn when a message it had not read runs next", async () => {
    const { ctrl, print } = await chat();
    ctrl.send("describe a river");
    print(init + echo("describe a river") + reply("m1", "Rivers carve…"));
    ctrl.send("also say BANANA");
    print(result("r1") + init);
    expect(ctrl.state.busy).toBe(true);
    expect(pending(ctrl)).toEqual(["also say BANANA"]);
    print(echo("also say BANANA") + reply("m2", "BANANA") + result("r2"));
    expect(ctrl.state.busy).toBe(false);
    expect(results(ctrl).some((r) => r.startsWith("Error"))).toBe(false);
  });

  it("says Stopped, not Error, for a stop the person asked for", async () => {
    const { ctrl, written, print } = await chat();
    ctrl.send("sleep three times");
    print(init + echo("sleep three times") + toolUse("t1"));
    ctrl.interrupt();
    expect(JSON.parse(written.at(-1)!).request).toEqual({ subtype: "interrupt" });
    print(ack(1) + toolResult("t1") + marker + stoppedResult("r1"));
    expect(ctrl.state.busy).toBe(false);
    expect(results(ctrl)).toEqual(["Stopped · 1.3s · 3 turns"]);
  });

  it("stays working, with no error, when a message written before the stop runs after it", async () => {
    const { ctrl, said, print } = await chat();
    ctrl.send("sleep three times");
    print(init + echo("sleep three times") + toolUse("t1"));
    ctrl.send("also say BANANA");
    ctrl.interrupt();
    print(ack(1) + toolResult("t1") + marker + stoppedResult("r1") + init);
    expect(ctrl.state.busy).toBe(true);
    expect(results(ctrl)).toEqual(["Stopped · 1.3s · 3 turns"]);
    print(echo("also say BANANA") + toolUse("t2") + toolResult("t2") + reply("m3", "FINISHED BANANA") + result("r2"));
    expect(ctrl.state.busy).toBe(false);
    expect(pending(ctrl)).toEqual([]);
    expect(said()).toEqual(["sleep three times", "also say BANANA"]);
  });

  it("still shows a run that failed by itself as an error", async () => {
    const { ctrl, print } = await chat();
    ctrl.send("go");
    print(init + echo("go") + toolUse("t1") + toolResult("t1") + stoppedResult("r1"));
    expect(results(ctrl)[0]).toMatch(/^Error: \[ede_diagnostic\]/);
  });

  it("does not carry an answered stop into the next run", async () => {
    const { ctrl, print } = await chat();
    // A stop with nothing running is acknowledged too; the next run starts afresh.
    ctrl.interrupt();
    print(ack(1));
    ctrl.send("go");
    print(init + echo("go") + stoppedResult("r1"));
    expect(results(ctrl)[0]).toMatch(/^Error:/);
  });

  it("shows a replayed stop the same way, and writes nothing", async () => {
    const log = init + echo("one") + toolUse("t1") + ack(1) + toolResult("t1") + marker + stoppedResult("r1") + init + echo("two") + result("r2");
    const { ctrl, said } = await chat(createClaudeAdapter, { log });
    expect(said()).toEqual([]);
    expect(results(ctrl)[0]).toMatch(/^Stopped/);
    expect(ctrl.state.busy).toBe(false);
  });
});
