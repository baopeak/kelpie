import { describe, expect, it } from "vitest";
import { createAcpAdapter } from "./acp-adapter";
import { type Adapter, type AdapterContext, ChatController } from "./chat-model";
import { createClaudeAdapter } from "./claude-adapter";
import { createCodexAdapter } from "./codex-adapter";
import { fromDaemonEvent, fromPhoneMessage } from "./chat-queue";
import { daemonOwnsQueue } from "../lib/attention";
import type { ChatTransport } from "./transport";

const tick = () => new Promise((r) => setTimeout(r, 0));

/** A controller over the daemon's queue: a transport that records what the window asks of the daemon. */
function setup(make?: (ctx: AdapterContext) => Adapter, opts: { daemon?: boolean } = {}) {
  const asked = { enqueue: [] as { cid: string; text: string; rich: boolean; images: string[] }[], write: [] as { qid: string; lines: string[] | null; turn: boolean; retry?: boolean; tooLarge?: boolean }[], ops: [] as { op: string; qid?: string; opts?: object }[], sent: [] as string[] };
  /** What the daemon answers `chat.write_turn` with when it does not take the lines (null: it takes them). */
  const daemonSays = { error: null as string | null };
  const transport: ChatTransport = {
    send: (line) => void asked.sent.push(line),
    enqueue: async (cid, text, rich, images) => {
      asked.enqueue.push({ cid, text, rich, images });
      return { qid: `q${asked.enqueue.length}`, state: "queued", ahead: 0 };
    },
    writeTurn: async (qid, lines, turn, retry, tooLarge) => {
      asked.write.push({ qid, lines, turn, ...(retry ? { retry: true } : {}), ...(tooLarge ? { tooLarge: true } : {}) });
      if (daemonSays.error && lines) throw new Error(daemonSays.error);
    },
    queueOp: async (op, qid, o) => void asked.ops.push({ op, qid, opts: o }),
  };
  let ctx!: AdapterContext;
  const calls: string[] = [];
  const fake = (c: AdapterContext): Adapter => {
    ctx = c;
    return {
      start() {},
      onMessage() {},
      reset() {},
      send(text, _typed, images) {
        calls.push(`send ${text}${images?.length ? ` +${images.length}` : ""}`);
        ctx.write({ type: "user", message: { role: "user", content: text } });
        ctx.set("busy", true);
      },
      steer(text) {
        calls.push(`steer ${text}`);
        return true;
      },
      interrupt() {
        calls.push("interrupt");
      },
      respond() {},
      reset_() {},
      immediate: (text: string) => text === "/model x",
    } as Adapter;
  };
  const ctrl = new ChatController("s1", "/work", null, make ?? fake, { transport, daemonQueue: () => opts.daemon ?? true });
  ctrl.set("ready", true);
  (ctrl as unknown as { live: boolean }).live = true;
  const feed = (...lines: object[]) => (ctrl as unknown as { feedText(t: string): void }).feedText(lines.map((l) => JSON.stringify(l)).join("\n") + "\n");
  return { ctrl, asked, calls, feed, ctx: () => ctx, daemonSays };
}

const view = (items: { qid: string; cid: string; text: string; by?: { id: string; name: string } }[], extra: object = {}) =>
  fromDaemonEvent({ items: items.map((i) => ({ kind: i.by?.id === "desktop" || !i.by ? "desktop" : "phone", by: i.by ?? { id: "desktop", name: "desktop" }, ...i })), paused: false, held: false, in_flight: false, ...extra });

describe("a window on the daemon's queue", () => {
  it("queues what is sent without writing it", async () => {
    const { ctrl, asked, calls } = setup();
    ctrl.send("one", true);
    await tick();
    expect(asked.enqueue).toHaveLength(1);
    expect(asked.enqueue[0]).toMatchObject({ text: "one", rich: false, images: [] });
    expect(asked.enqueue[0].cid.length).toBeGreaterThan(8);
    expect(calls).toEqual([]);
    expect(asked.sent).toEqual([]);
    // Pictures are what only this window can build.
    const pic = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
    ctrl.send("look", true, [pic]);
    expect(asked.enqueue[1]).toMatchObject({ rich: true, images: ["a.png"] });
    // A picker command starts no turn: it runs at once.
    ctrl.send("/model x", true);
    expect(calls).toEqual(["send /model x"]);
    expect(asked.enqueue).toHaveLength(2);
  });

  it("builds the offered message's lines under capture and hands them to the daemon with the turn flag", async () => {
    const { ctrl, asked, calls } = setup();
    ctrl.send("hello", true);
    await tick();
    const { cid } = asked.enqueue[0];
    ctrl.turnReady("q1", cid);
    await tick();
    expect(calls).toEqual(["send hello"]);
    expect(asked.sent).toEqual([]);
    expect(asked.write).toHaveLength(1);
    expect(asked.write[0].qid).toBe("q1");
    expect(asked.write[0].turn).toBe(true);
    expect(JSON.parse(asked.write[0].lines![0])).toEqual({ type: "user", message: { role: "user", content: "hello" } });
    // What the adapter writes afterwards goes straight to the agent again.
    ctrl.interrupt();
    expect(calls.at(-1)).toBe("interrupt");
  });

  it("declines an offer it cannot build: while replaying, for a message it does not know, before it is attached", async () => {
    const { ctrl, asked, calls } = setup();
    ctrl.send("hello", true);
    await tick();
    const { cid } = asked.enqueue[0];
    ctrl.turnReady("q9", "someone-elses");
    (ctrl as unknown as { isReplaying: boolean }).isReplaying = true;
    ctrl.turnReady("q1", cid);
    (ctrl as unknown as { isReplaying: boolean }).isReplaying = false;
    (ctrl as unknown as { live: boolean }).live = false;
    ctrl.turnReady("q1", cid);
    ctrl.set("ready", false);
    (ctrl as unknown as { live: boolean }).live = true;
    ctrl.turnReady("q1", cid);
    await tick();
    // A message it does not have is a plain no; the rest are "not now": the daemon asks again.
    expect(asked.write).toEqual([
      { qid: "q9", lines: null, turn: false },
      { qid: "q1", lines: null, turn: false, retry: true },
      { qid: "q1", lines: null, turn: false, retry: true },
      { qid: "q1", lines: null, turn: false, retry: true },
    ]);
    expect(calls, "no adapter was asked to send").toEqual([]);
  });

  it("shows the daemon's queue as chips, with who queued each, and its pause", async () => {
    const { ctrl, asked } = setup();
    ctrl.send("mine", true);
    await tick();
    const cid = asked.enqueue[0].cid;
    ctrl.applyQueue(view([{ qid: "q1", cid, text: "mine" }, { qid: "q2", cid: "p1", text: "from a phone", by: { id: "dev", name: "Pixel" } }], { paused: true }));
    expect(ctrl.state.queue.map((q) => [q.id, q.text, q.from])).toEqual([["q1", "mine", undefined], ["q2", "from a phone", "Pixel"]]);
    expect(ctrl.state.queuePaused).toBe(true);
    // Chips act on the daemon's queue, and go when it says so.
    ctrl.unqueue("q2");
    ctrl.resumeQueue();
    await tick();
    expect(asked.ops).toEqual([{ op: "cancel", qid: "q2", opts: undefined }, { op: "resume", qid: undefined, opts: undefined }]);
    expect(ctrl.state.queue).toHaveLength(2);
    ctrl.applyQueue(view([{ qid: "q1", cid, text: "mine" }]));
    expect(ctrl.state.queue.map((q) => q.id)).toEqual(["q1"]);
    expect(ctrl.state.queuePaused).toBe(false);
  });

  it("reports what the daemon dropped as not sent, and keeps nothing for it", async () => {
    const { ctrl, asked } = setup();
    ctrl.send("lost message", true);
    await tick();
    const cid = asked.enqueue[0].cid;
    ctrl.applyQueue(view([], { dropped: [{ qid: "q1", cid, text: "x", reason: "needs_window", by: { id: "desktop", name: "desktop" } }] }));
    const notes = ctrl.state.items.filter((i) => i.kind === "system");
    expect(notes).toHaveLength(1);
    expect((notes[0] as { text: string }).text).toContain("lost message");
    expect(ctrl.state.queue).toEqual([]);
    // It cannot be built later: the daemon would not have it.
    ctrl.turnReady("q1", cid);
    await tick();
    expect(asked.write.at(-1)).toEqual({ qid: "q1", lines: null, turn: false });
  });

  it("does not show a not-sent note of its own when the agent exits: the daemon says what it dropped", async () => {
    const { ctrl, asked, feed } = setup();
    ctrl.send("waiting", true);
    await tick();
    ctrl.applyQueue(view([{ qid: "q1", cid: asked.enqueue[0].cid, text: "waiting" }]));
    feed({ type: "kelpie_exit", code: 1 });
    expect(ctrl.state.items.filter((i) => i.kind === "system" && i.text.includes("Not sent"))).toHaveLength(0);
    expect(ctrl.state.queue).toHaveLength(1);
  });

  it("sends now: the front when idle, a steer when it can, the front and an interrupt otherwise", async () => {
    const { ctrl, asked, calls, ctx } = setup();
    ctrl.send("a", true);
    ctrl.send("b", true);
    ctrl.send("c", true);
    await tick();
    const [a, b, c] = asked.enqueue.map((e) => e.cid);
    ctrl.applyQueue(view([{ qid: "q1", cid: a, text: "a" }, { qid: "q2", cid: b, text: "b" }, { qid: "q3", cid: c, text: "c" }]));
    ctrl.sendNow("q2");
    await tick();
    expect(asked.ops.at(-1)).toEqual({ op: "front", qid: "q2", opts: undefined });
    // Busy: the adapter steers (the queue gives the message up first).
    ctx().set("busy", true);
    ctrl.sendNow("q3");
    await tick();
    expect(asked.ops.at(-1)).toEqual({ op: "take", qid: "q3", opts: undefined });
    expect(calls).toEqual(["steer c"]);
    // A command never rides in a turn: front, and the interrupt.
    ctrl.send("/compact", true);
    await tick();
    ctrl.applyQueue(view([{ qid: "q1", cid: a, text: "a" }, { qid: "q4", cid: asked.enqueue[3].cid, text: "/compact" }]));
    ctrl.sendNow("q4");
    await tick();
    expect(asked.ops.at(-1)).toEqual({ op: "front", qid: "q4", opts: { interrupt: true } });
    expect(calls.at(-1)).toBe("interrupt");
  });

  it("queues a message the agent refused to steer at the front of the daemon's queue", async () => {
    const { ctrl, asked, ctx } = setup();
    ctx().requeue!("also check b.ts");
    await tick();
    expect(asked.enqueue.map((e) => e.text)).toEqual(["also check b.ts"]);
    expect(asked.ops.at(-1)).toEqual({ op: "front", qid: "q1", opts: undefined });
    expect(ctrl.state.queue).toEqual([]);
  });

  it("tells an older daemon from a new one: below the queue's API the window keeps its own queue", () => {
    const { ctrl, asked, calls } = setup(undefined, { daemon: false });
    ctrl.send("one");
    ctrl.send("two");
    expect(calls).toEqual(["send one"]);
    expect(ctrl.state.queue.map((q) => q.text)).toEqual(["two"]);
    expect(asked.enqueue).toEqual([]);
    expect(asked.sent).toHaveLength(1);
  });
});

describe("a message too large to write", () => {
  const pic = { mime: "image/png", data: "AA==", path: "/tmp/big.png" };
  const noteOf = (c: ChatController) => c.state.items.find((i) => i.kind === "system") as { id: string; text: string } | undefined;

  it("does not stay offered when the daemon says so: the bubble goes, and the note comes with its word", async () => {
    const { ctrl, asked, daemonSays, feed } = setup((c) => createClaudeAdapter(c));
    daemonSays.error = "too_large";
    ctrl.send("look", true, [pic]);
    await tick();
    const { cid } = asked.enqueue[0];
    ctrl.applyQueue(view([{ qid: "q1", cid, text: "look" }]));
    ctrl.turnReady("q1", cid);
    await tick();
    await tick();
    expect(ctrl.state.items.filter((i) => i.kind === "user"), "the pending bubble is gone").toHaveLength(0);
    expect(ctrl.state.busy).toBe(false);
    expect(asked.write.filter((w) => w.lines === null), "it already dealt with it: nothing more is asked").toEqual([]);
    ctrl.applyQueue(fromDaemonEvent({ items: [], paused: false, held: false, in_flight: false, dropped: [{ qid: "q1", cid, by: { id: "desktop", name: "desktop" }, text: "look", images: ["big.png"], reason: "too_large" }] }));
    expect(noteOf(ctrl)?.text).toBe("Not sent (too large to send to the agent):\nlook\n[big.png]");
    // The log line that outlives it says the same, under the same id.
    feed({ type: "kelpie_unsent", qid: "q1", cid, by: "desktop", name: "desktop", text: "look", images: ["big.png"], reason: "too_large" });
    expect(ctrl.state.items.filter((i) => i.kind === "system")).toHaveLength(1);
    expect(noteOf(ctrl)?.text).toBe("Not sent (too large to send to the agent):\nlook\n[big.png]");
  });

  it("tells the daemon when the connection refused it before it got there", async () => {
    const { ctrl, asked, daemonSays } = setup((c) => createClaudeAdapter(c));
    daemonSays.error = "too large to send to the daemon: 33 MiB, the limit is 32 MiB";
    ctrl.send("look", true, [pic]);
    await tick();
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    await tick();
    expect(asked.write.at(-1)).toMatchObject({ qid: "q1", lines: null, turn: false, tooLarge: true });
  });

  it("gives a final no for any other failure, so what waits behind it moves on", async () => {
    const { ctrl, asked, daemonSays } = setup((c) => createClaudeAdapter(c));
    daemonSays.error = "lines must be a few strings";
    ctrl.send("look", true, [pic]);
    await tick();
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    await tick();
    expect(asked.write.at(-1)).toMatchObject({ qid: "q1", lines: null, turn: false });
    expect(asked.write.at(-1)).not.toHaveProperty("retry");
    expect(asked.write.at(-1)?.tooLarge).toBeFalsy();
  });

  it.each(["too_late", "busy", "switching"])("leaves it to the daemon to ask again after %s", async (word) => {
    const { ctrl, asked, daemonSays } = setup((c) => createClaudeAdapter(c));
    daemonSays.error = word;
    ctrl.send("look", true, [pic]);
    await tick();
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    await tick();
    expect(asked.write.filter((w) => w.lines === null)).toEqual([]);
  });

  it("is told on a phone too, as its own reason", () => {
    const ctrl = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { observer: true, transport: { queueOp: async () => {} } });
    ctrl.applyQueue(fromPhoneMessage({ items: [], paused: false, held: false, in_flight: false, dropped: [{ qid: "q1", cid: "c1", by: "desktop", kind: "desktop", text: "look", images: ["big.png"], reason: "too_large" }] }));
    const note = () => ctrl.state.items.find((i) => i.kind === "system") as { text: string };
    expect(note().text).toBe("Not sent (too large to send to the agent):\nlook\n[big.png]");
    ctrl.applyLog("reset", [JSON.stringify({ type: "kelpie_unsent", qid: "q1", cid: "c1", by: "desktop", name: "desktop", text: "look", images: ["big.png"], reason: "too_large" })]);
    expect(note().text).toBe("Not sent (too large to send to the agent):\nlook\n[big.png]");
  });
});

describe("a note of what was not sent", () => {
  const unsent = (reason: string, text: string, extra: object = {}) => ({ type: "kelpie_unsent", qid: "q1", cid: "c1", by: "desktop", name: "desktop", text, images: ["a.png"], reason, ...extra });
  const noteOf = (c: ChatController) => (c.state.items.find((i) => i.kind === "system") as { text: string } | undefined)?.text;

  it("keeps the names of the pictures, when the log line comes after the event and on a replay", async () => {
    const { ctrl, asked, feed } = setup();
    ctrl.send("", true, [{ mime: "image/png", data: "AA==", path: "/tmp/a.png" }]);
    await tick();
    const { cid } = asked.enqueue[0];
    ctrl.applyQueue(view([{ qid: "q1", cid, text: "" }]));
    ctrl.applyQueue(fromDaemonEvent({ items: [], paused: false, held: false, in_flight: false, dropped: [{ qid: "q1", cid, by: { id: "desktop", name: "desktop" }, text: "", images: ["a.png"], reason: "cancelled", cancelled_by: { id: "p1", name: "Pixel" } }] }));
    expect(noteOf(ctrl)).toBe("Not sent (cancelled from Pixel):\n[a.png]");
    feed(unsent("cancelled", "", { cid, cancelled_by: { id: "p1", name: "Pixel" } }));
    expect(ctrl.state.items.filter((i) => i.kind === "system")).toHaveLength(1);
    expect(noteOf(ctrl)).toBe("Not sent (cancelled from Pixel):\n[a.png]");
  });

  it.each(["needs_window", "declined"])("says the names after a reload (%s), under any text", (reason) => {
    const { ctrl, feed } = setup();
    feed(unsent(reason, "see this"));
    expect(noteOf(ctrl)).toBe("Not sent (this window could not prepare it):\nsee this\n[a.png]");
  });

  it("reads an older log line, which names none", () => {
    const { ctrl, feed } = setup();
    feed({ type: "kelpie_unsent", qid: "q1", cid: "c1", by: "desktop", name: "desktop", text: "words", reason: "declined" });
    expect(noteOf(ctrl)).toBe("Not sent (this window could not prepare it):\nwords");
  });

  it("names them from the daemon's word when this window does not have the message", () => {
    const { ctrl } = setup();
    ctrl.applyQueue(fromDaemonEvent({ items: [], paused: false, held: false, in_flight: false, dropped: [{ qid: "q1", cid: "gone", by: { id: "desktop", name: "desktop" }, text: "", images: ["a.png"], reason: "declined" }] }));
    expect(noteOf(ctrl)).toBe("Not sent (this window could not prepare it):\n[a.png]");
  });
});

describe("what the log says was sent", () => {
  it("shows an ACP prompt once, from the log, with nothing local ahead of it", async () => {
    const { ctrl, asked, feed } = setup((c) => createAcpAdapter(c));
    feed({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "S1" } });
    ctrl.send("hi there", true);
    await tick();
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    expect(asked.write[0].turn).toBe(true);
    expect(JSON.parse(asked.write[0].lines![0]).method).toBe("session/prompt");
    expect(ctrl.state.items.filter((i) => i.kind === "user"), "no local copy").toHaveLength(0);
    const sent = { type: "kelpie_sent", qid: "q1", cid: asked.enqueue[0].cid, by: "desktop", name: "desktop", text: "hi there", images: [] };
    feed(sent);
    expect(ctrl.state.items.filter((i) => i.kind === "user").map((i) => (i as { text: string }).text)).toEqual(["hi there"]);
    // A replay of the same log shows it once as well.
    feed(sent);
    expect(ctrl.state.items.filter((i) => i.kind === "user")).toHaveLength(1);
  });

  it("shows a phone's ACP prompt with its sender", () => {
    const { ctrl, feed } = setup((c) => createAcpAdapter(c));
    feed({ type: "kelpie_sent", qid: "q2", cid: "c2", by: "phone", name: "Pixel", text: "from the couch", images: ["a.png"] });
    const user = ctrl.state.items.find((i) => i.kind === "user") as { text: string; from?: string; images?: object[] };
    expect([user.text, user.from, user.images]).toEqual(["from the couch", "Pixel", [{ name: "a.png" }]]);
  });

  it("records this window's pictures as sent from here, whichever of the log line and the queue's word comes first", async () => {
    const pic = { mime: "image/png", data: "AA==", path: "/tmp/a.png" };
    const shown = [{ name: "a.png", mime: "image/png", path: "/tmp/a.png" }];
    const userOf = (c: ChatController) => c.state.items.find((i) => i.kind === "user") as { text: string; images?: object[] };
    const line = (cid: string) => ({ type: "kelpie_sent", qid: "q1", cid, by: "desktop", name: "desktop", text: "", images: ["a.png"] });

    const first = setup((c) => createAcpAdapter(c));
    first.ctrl.send("", true, [pic]);
    await tick();
    first.feed(line(first.asked.enqueue[0].cid));
    expect([userOf(first.ctrl).text, userOf(first.ctrl).images]).toEqual(["", shown]);

    const late = setup((c) => createAcpAdapter(c));
    late.ctrl.send("", true, [pic]);
    await tick();
    const { cid } = late.asked.enqueue[0];
    late.ctrl.applyQueue(view([{ qid: "q1", cid, text: "" }]));
    late.ctrl.applyQueue(view([]));
    late.feed(line(cid));
    expect(userOf(late.ctrl).images).toEqual(shown);
    // No bytes in the transcript, and a replay of the same log finds them again.
    late.feed(line(cid));
    expect(late.ctrl.state.items.filter((i) => i.kind === "user")).toHaveLength(1);
    expect(userOf(late.ctrl).images).toEqual(shown);
  });

  it("does not take another device's message for this window's, even under a cid it knows", async () => {
    const { ctrl, asked, feed } = setup((c) => createAcpAdapter(c));
    ctrl.send("mine", true, [{ mime: "image/png", data: "AA==", path: "/tmp/private.png" }]);
    await tick();
    feed({ type: "kelpie_sent", qid: "q9", cid: asked.enqueue[0].cid, by: "phone", name: "Pixel", text: "not mine", images: [] });
    const user = ctrl.state.items.find((i) => i.kind === "user") as { images?: object[]; from?: string };
    expect([user.images, user.from]).toEqual([undefined, "Pixel"]);
  });

  it("names the pictures of a message this window no longer knows", () => {
    const { ctrl, feed } = setup((c) => createAcpAdapter(c));
    feed({ type: "kelpie_sent", qid: "q1", cid: "from-before", by: "desktop", name: "desktop", text: "see", images: ["a.png", "b.png"] });
    const user = ctrl.state.items.find((i) => i.kind === "user") as { text: string; images?: object[]; from?: string };
    expect([user.text, user.images, user.from]).toEqual(["see", [{ name: "a.png" }, { name: "b.png" }], undefined]);
  });

  it("keeps a phone's view of pictures as their names in the text", () => {
    const ctrl = new ChatController("s1", "/work", null, (c) => createAcpAdapter(c), { observer: true });
    const sent = (qid: string, text: string) => JSON.stringify({ type: "kelpie_sent", qid, cid: qid, by: "desktop", name: "desktop", text, images: ["a.png"] });
    ctrl.applyLog("reset", [sent("q1", "see"), sent("q2", "")]);
    const users = ctrl.state.items.filter((i) => i.kind === "user") as { text: string; images?: object[] }[];
    expect(users.map((u) => [u.text, u.images])).toEqual([["see\n[a.png]", undefined], ["[a.png]", undefined]]);
  });

  it("labels the echo of a phone's message for the agents that echo it", () => {
    const { ctrl, feed } = setup((c) => createClaudeAdapter(c));
    feed({ type: "kelpie_sent", qid: "q1", cid: "c1", by: "phone", name: "Pixel", text: "run the tests", images: [] });
    expect(ctrl.state.items.filter((i) => i.kind === "user")).toHaveLength(0);
    feed({ type: "user", uuid: "u1", message: { role: "user", content: [{ type: "text", text: "run the tests" }] } });
    const user = ctrl.state.items.filter((i) => i.kind === "user") as { text: string; from?: string }[];
    expect(user).toHaveLength(1);
    expect([user[0].text, user[0].from]).toEqual(["run the tests", "Pixel"]);
    // The desktop's own messages carry no label, and the label is used once.
    feed({ type: "kelpie_sent", qid: "q2", cid: "c2", by: "desktop", name: "desktop", text: "mine", images: [] });
    feed({ type: "user", uuid: "u2", message: { role: "user", content: "mine" } });
    feed({ type: "user", uuid: "u3", message: { role: "user", content: "run the tests" } });
    const all = ctrl.state.items.filter((i) => i.kind === "user") as { id: string; from?: string }[];
    expect(all.map((i) => [i.id, i.from])).toEqual([["u1", "Pixel"], ["u2", undefined], ["u3", undefined]]);
  });

  it("tells the one who sent it when its own message shows up", () => {
    const seen: string[] = [];
    const ctrl = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { observer: true, onSent: (m) => void seen.push(m.cid) });
    ctrl.applyLog("reset", [JSON.stringify({ type: "kelpie_sent", qid: "q1", cid: "old", by: "phone", name: "Pixel", text: "x", images: [] })]);
    expect(seen, "a replayed line clears the chip too: the reply to the send may have been lost").toEqual(["old"]);
    ctrl.applyLog("live", [JSON.stringify({ type: "kelpie_sent", qid: "q2", cid: "mine", by: "phone", name: "Pixel", text: "y", images: [] })]);
    expect(seen).toEqual(["old", "mine"]);
  });
});

describe("which daemon owns the queue", () => {
  it("is decided by the level that brought chat.send, not by one close to it", async () => {
    // Level 20 is the chat log's `kelpie_answer`: that daemon has no `chat.send`.
    expect(daemonOwnsQueue(20)).toBe(false);
    expect(daemonOwnsQueue(21)).toBe(true);
    const { ctrl, asked, calls } = setup(undefined, { daemon: daemonOwnsQueue(20) });
    ctrl.send("hello", true);
    await tick();
    expect(asked.enqueue, "nothing goes to a verb the daemon does not have").toEqual([]);
    expect(calls).toEqual(["send hello"]);
  });
});

describe("a phone on the daemon's queue", () => {
  it("mirrors the queue it is told, and passes its stops and cancels on", async () => {
    const ops: string[] = [];
    const transport: ChatTransport = {
      queueOp: async (op, qid) => void ops.push(`${op} ${qid ?? ""}`.trim()),
      stop: async () => void ops.push("stop"),
    };
    const ctrl = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { observer: true, transport });
    ctrl.applyQueue(fromPhoneMessage({ items: [{ qid: "q1", cid: "c1", by: "Pixel", mine: true, kind: "phone", text: "hi" }, { qid: "q2", cid: "c2", by: "desktop", mine: false, kind: "desktop", text: "there" }], paused: true, held: false, in_flight: true }));
    expect(ctrl.state.queue.map((q) => [q.id, q.mine, q.from])).toEqual([["q1", true, "Pixel"], ["q2", false, undefined]]);
    expect(ctrl.state.queuePaused).toBe(true);
    ctrl.unqueue("q2");
    ctrl.resumeQueue();
    ctrl.interrupt();
    ctrl.send("never from a phone's controller", true);
    await tick();
    expect(ops).toEqual(["cancel q2", "resume", "stop"]);
  });
});

describe("a view that has been told the daemon's queue stays on it", () => {
  it("never flushes the mirrored queue itself, and sends what is typed to the daemon, when the daemon level reads as old for a moment", async () => {
    const level = { daemon: true };
    const { ctrl, asked, calls, ctx } = setup(undefined, level);
    ctrl.applyQueue(view([{ qid: "q1", cid: "p1", text: "from a phone", by: { id: "dev", name: "Pixel" } }]));
    level.daemon = false; // not read yet, or a blip in the connection
    ctx().set("busy", true);
    ctx().set("busy", false);
    await tick();
    expect(calls, "the head is the daemon's to write").toEqual([]);
    expect(asked.sent).toEqual([]);
    ctrl.send("typed meanwhile", true);
    await tick();
    expect(asked.enqueue.map((e) => e.text)).toEqual(["typed meanwhile"]);
    expect(ctrl.state.queue.map((q) => q.id)).toEqual(["q1"]);
    expect(calls).toEqual([]);
  });

  it("shows a message it could not hand to the daemon as not sent, with its text", async () => {
    const transport: ChatTransport = { send: () => {}, enqueue: async () => { throw new Error("not attached"); }, queueOp: async () => {} };
    const ctrl = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { transport, daemonQueue: () => true });
    ctrl.set("ready", true);
    ctrl.send("keep this text", true);
    await tick();
    const note = ctrl.state.items.find((i) => i.kind === "system") as { text: string } | undefined;
    expect(note?.text).toContain("keep this text");
  });
});

describe("an offer that cannot be answered now", () => {
  it("keeps the message and has the daemon ask again, and asks only for a message it does not have to be given up on", async () => {
    const { ctrl, asked } = setup();
    ctrl.send("look", true);
    await tick();
    const { cid } = asked.enqueue[0];
    (ctrl as unknown as { loadingHistory: boolean }).loadingHistory = true;
    ctrl.turnReady("q1", cid);
    await tick();
    expect(asked.write.at(-1)).toEqual({ qid: "q1", lines: null, turn: false, retry: true });
    (ctrl as unknown as { loadingHistory: boolean }).loadingHistory = false;
    ctrl.turnReady("q1", cid);
    await tick();
    expect(asked.write.at(-1)!.lines).not.toBeNull();
  });

  it("shows a message that waits for its window, to every viewer", () => {
    const { ctrl } = setup();
    ctrl.applyQueue(fromDaemonEvent({ items: [{ qid: "q1", cid: "c1", text: "a picture", by: { id: "desktop", name: "desktop" }, kind: "desktop", needs_window: true }] }));
    expect(ctrl.state.queue[0].needsWindow).toBe(true);
    const phone = fromPhoneMessage({ items: [{ qid: "q1", cid: "c1", text: "a picture", by: "desktop", kind: "desktop", mine: false, needs_window: true }] });
    expect(phone.items[0].needsWindow).toBe(true);
  });

  it("keeps a lasting note of what the daemon gave up on, once, however often the log is read", () => {
    const { ctrl, feed } = setup();
    const line = { type: "kelpie_unsent", qid: "q7", cid: "c7", by: "desktop", name: "desktop", text: "a picture", reason: "needs_window" };
    feed(line);
    feed(line);
    const notes = ctrl.state.items.filter((i) => i.kind === "system" && i.text.includes("a picture"));
    expect(notes).toHaveLength(1);
  });

  it("says no for good when the adapter cannot build the message, once, without asking to be asked again", async () => {
    const warn = console.warn;
    const warned: unknown[] = [];
    console.warn = (...a: unknown[]) => void warned.push(a);
    try {
      const { ctrl, asked } = setup((c) => ({ start() {}, onMessage() {}, reset() {}, interrupt() {}, respond() {}, send() { c.set("busy", true); throw new Error("broken picture"); } }) as unknown as Adapter);
      ctrl.send("look", true);
      await tick();
      ctrl.turnReady("q1", asked.enqueue[0].cid);
      await tick();
      expect(asked.write).toEqual([{ qid: "q1", lines: null, turn: false }]);
      expect(ctrl.state.busy, "what the failed build did is undone").toBe(false);
      expect(warned).toHaveLength(1);
    } finally {
      console.warn = warn;
    }
  });

  it("tells a phone's view what the log says left the queue unsent, also on replay, and words a dropped note for the phone", () => {
    const told: { cid: string; reason: string; cancelledBy?: string }[] = [];
    const phone = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { observer: true, onUnsent: (m) => void told.push(m) });
    phone.applyLog("reset", [
      JSON.stringify({ type: "kelpie_unsent", qid: "q1", cid: "mine-1", by: "phone", name: "Pixel", text: "cancelled while offline", reason: "cancelled", cancelled_by: { id: "desktop", name: "desktop" } }),
      JSON.stringify({ type: "kelpie_unsent", qid: "q2", cid: "pic", by: "desktop", name: "desktop", text: "a picture", reason: "declined" }),
    ]);
    expect(told).toEqual([{ cid: "mine-1", reason: "cancelled", cancelledBy: "desktop" }, { cid: "pic", reason: "declined", cancelledBy: undefined }]);
    const notes = phone.state.items.filter((i) => i.kind === "system") as { text: string }[];
    expect(notes.map((n) => n.text)).toEqual(["Not sent (the computer could not prepare it):\na picture"]);
  });

  it("notes this window's message cancelled from a phone also from the log, for a pane that was closed when it happened", () => {
    const { ctrl, feed } = setup();
    const line = { type: "kelpie_unsent", qid: "q8", cid: "c8", by: "desktop", name: "desktop", text: "with a picture", reason: "cancelled", cancelled_by: { id: "dev1", name: "Pixel" } };
    feed(line);
    // The live event for the same message, when the pane was there for it too: one note.
    ctrl.applyQueue(view([], { dropped: [{ qid: "q8", cid: "c8", text: "with a picture", reason: "cancelled", by: { id: "desktop", name: "desktop" }, cancelled_by: { id: "dev1", name: "Pixel" } }] }));
    const notes = ctrl.state.items.filter((i) => i.kind === "system" && i.text.includes("with a picture")) as { text: string }[];
    expect(notes).toHaveLength(1);
    expect(notes[0].text).toContain("Pixel");
    // A phone's own message cancelled elsewhere is the phone's to show; a phone's view shows no desktop note.
    feed({ ...line, qid: "q9", by: "phone", name: "Pixel", text: "phone text" });
    expect(ctrl.state.items.some((i) => i.kind === "system" && i.text.includes("phone text"))).toBe(false);
    const phone = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { observer: true });
    phone.applyLog("reset", [JSON.stringify(line)]);
    expect(phone.state.items.some((i) => i.kind === "system")).toBe(false);
  });
});

describe("a message the daemon does not take is undone in the view", () => {
  it("Claude: the pending bubble and busy go, and a turn that is really running keeps it busy", async () => {
    const { ctrl, asked, daemonSays } = setup((c) => createClaudeAdapter(c));
    ctrl.send("hi there", true);
    await tick();
    daemonSays.error = "too_late";
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    expect(ctrl.state.items.filter((i) => i.kind === "user")).toHaveLength(1);
    expect(ctrl.state.busy).toBe(true);
    await tick();
    expect(ctrl.state.items.filter((i) => i.kind === "user")).toHaveLength(0);
    expect(ctrl.state.busy).toBe(false);
    // The offer comes again and is built again: one bubble, not two.
    daemonSays.error = "busy";
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    expect(ctrl.state.items.filter((i) => i.kind === "user")).toHaveLength(0);
    expect(ctrl.state.busy, "another turn started: it is busy because of that").toBe(true);
  });

  it("Codex: a refused /new leaves the thread as it was", async () => {
    const { ctrl, asked, feed, daemonSays } = setup((c) => createCodexAdapter(c));
    feed({ id: "kelpie-init", result: {} }, { id: "kelpie-thread", result: { thread: { id: "T1", turns: [] } } });
    ctrl.send("/new", true);
    await tick();
    daemonSays.error = "switching";
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    expect(asked.write[0].lines!.some((l) => JSON.parse(l).method === "thread/start")).toBe(true);
    expect(ctrl.state.conversationId).toBe("T1");
    daemonSays.error = null;
    ctrl.send("hello", true);
    await tick();
    ctrl.turnReady("q2", asked.enqueue[1].cid);
    await tick();
    const turn = JSON.parse(asked.write.at(-1)!.lines![0]);
    expect([turn.method, turn.params.threadId]).toEqual(["turn/start", "T1"]);
  });

  it("ACP: a refused /new leaves the session as it was", async () => {
    const { ctrl, asked, feed, daemonSays } = setup((c) => createAcpAdapter(c));
    feed({ jsonrpc: "2.0", id: "kelpie-session", result: { sessionId: "S1" } });
    ctrl.send("/new", true);
    await tick();
    daemonSays.error = "too_late";
    ctrl.turnReady("q1", asked.enqueue[0].cid);
    await tick();
    expect(asked.write[0].lines!.some((l) => JSON.parse(l).method === "session/new")).toBe(true);
    daemonSays.error = null;
    ctrl.send("hi", true);
    await tick();
    ctrl.turnReady("q2", asked.enqueue[1].cid);
    await tick();
    const prompt = JSON.parse(asked.write.at(-1)!.lines![0]);
    expect([prompt.method, prompt.params.sessionId]).toEqual(["session/prompt", "S1"]);
  });
});

describe("a message cancelled by the other side", () => {
  const dropped = (by: object, extra: object) => view([], { dropped: [{ qid: "q1", cid: "c1", text: "my words", reason: "cancelled", by: { id: "desktop", name: "desktop" }, cancelled_by: by, ...extra }] });

  it("shows on the desktop as a note naming the phone, and not when the desktop cancelled it itself", () => {
    const { ctrl } = setup();
    ctrl.applyQueue(dropped({ id: "dev", name: "Pixel" }, {}));
    const notes = ctrl.state.items.filter((i) => i.kind === "system") as { text: string }[];
    expect(notes).toHaveLength(1);
    expect(notes[0].text).toContain("Pixel");
    expect(notes[0].text).toContain("my words");
    const again = setup();
    again.ctrl.applyQueue(dropped({ id: "desktop", name: "desktop" }, {}));
    expect(again.ctrl.state.items.filter((i) => i.kind === "system")).toHaveLength(0);
  });

  it("is left to the sender's own chip on a phone", () => {
    const ctrl = new ChatController("s1", "/work", null, (c) => createClaudeAdapter(c), { observer: true, transport: { queueOp: async () => {} } });
    ctrl.applyQueue(fromPhoneMessage({ items: [], dropped: [{ qid: "q1", cid: "c1", text: "my words", reason: "cancelled", mine: true, cancelled_by: "desktop", cancelled_by_name: "desktop" }] }));
    expect(ctrl.state.items.filter((i) => i.kind === "system")).toHaveLength(0);
  });
});
