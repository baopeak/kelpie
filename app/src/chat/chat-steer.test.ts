// Saying something while the agent works, as in its own terminal. The Claude lines follow what `claude -p` 2.1.316 printed
// for a message written to its stdin mid-turn: the echo comes when the running tools finish; a turn that ends first gets its
// `result`, and then the message runs as a turn of its own.
import { describe, expect, it } from "vitest";
import { ChatController, type ChatImage } from "./chat-model";
import { createClaudeAdapter } from "./claude-adapter";
import { createAcpAdapter } from "./acp-adapter";
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
  return { ctrl, said, queued, print: (s: string) => live(encode(s)) };
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

  const picture: ChatImage = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
  for (const [name, text, images] of [["a command", "/compact", undefined], ["a shell line", "!ls", undefined], ["a picture", "look", [picture]]] as const) {
    it(`waits for the turn to end when it is ${name}`, async () => {
      const { ctrl, said, print } = await chat();
      ctrl.send("one");
      print(echo("one"));
      ctrl.send(text, true, images ? [...images] : undefined);
      expect(said()).toEqual(["one"]);
      expect(ctrl.state.queue.map((q) => q.text)).toEqual([text]);
    });
  }

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
