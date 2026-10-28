import { describe, expect, it } from "vitest";
import { type Adapter, type AdapterContext, ChatController } from "./chat-model";

/** A controller over an adapter that records what it is asked to do. */
function setup(opts: { steer?: boolean } = {}) {
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
});
