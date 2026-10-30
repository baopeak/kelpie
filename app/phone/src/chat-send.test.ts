import { describe, expect, it } from "vitest";
import type { ChatCommand } from "../../src/chat/chat-model";
import type { QueueView } from "../../src/chat/chat-queue";
import type { SendOutcome } from "../../src/chat/transport";
import { ChatSender, chipView, MAX_SEND_BYTES, refuseLocally, SENT_LINGER_MS, type PendingSend } from "./chat-send";

function rig(kind = "claude", commands: ChatCommand[] = []) {
  let now = 1_000;
  let n = 0;
  let online = true;
  const sent: { cid: string; text: string }[] = [];
  const cancelled: string[] = [];
  let next: (cid: string, text: string) => Promise<SendOutcome> = async () => ({ result: "ok", qid: "q1", state: "queued", ahead: 0 });
  let cancelResult: "ok" | "too_late" | "failed" = "ok";
  const sender = new ChatSender(
    {
      send: (cid, text) => {
        sent.push({ cid, text });
        return next(cid, text);
      },
      cancel: async (qid) => (cancelled.push(qid), cancelResult),
      connected: () => online,
      now: () => now,
      newCid: () => `cid-${++n}`,
    },
    kind,
    () => commands,
  );
  return { sender, sent, cancelled, reply: (f: typeof next) => (next = f), tick: (ms: number) => (now += ms), setOnline: (v: boolean) => (online = v), cancelResult: (r: typeof cancelResult) => (cancelResult = r) };
}

const view = (items: string[], extra: Partial<QueueView> = {}): QueueView => ({
  items: items.map((cid, i) => ({ qid: `q${i + 1}`, cid, text: cid, mine: true, needsWindow: false })),
  paused: false, held: false, inFlight: false, dropped: [], ...extra,
});

describe("sending to a chat from the phone", () => {
  it("goes from sending to queued with how many are ahead, and to sent when its line is in the log", async () => {
    const r = rig();
    let release!: (o: SendOutcome) => void;
    r.reply(() => new Promise((res) => (release = res)));
    const done = r.sender.send("hello");
    expect(r.sender.pending.map((p) => [p.text, p.state])).toEqual([["hello", "sending"]]);
    expect(r.sender.sending).toBe(true);
    release({ result: "ok", qid: "q7", state: "queued", ahead: 2 });
    expect(await done).toEqual({ accepted: true, cid: "cid-1" });
    expect(r.sender.pending[0]).toMatchObject({ state: "queued", qid: "q7", ahead: 2 });
    expect(r.sender.sending).toBe(false);
    // The daemon says the queue moved on, then wrote it.
    r.sender.snapshot(view(["cid-1"]));
    expect(r.sender.pending[0]).toMatchObject({ state: "queued", ahead: 0 });
    r.sender.snapshot(view([]));
    expect(r.sender.pending[0].state).toBe("sent");
    // Its own cid in a kelpie_sent line: it is on the transcript.
    r.sender.sentSeen("cid-1");
    expect(r.sender.pending).toEqual([]);
  });

  it("says sent at once for a message the daemon wrote, and lets it go if its line never shows", async () => {
    const r = rig();
    r.reply(async () => ({ result: "ok", qid: "q1", state: "sent", ahead: 0 }));
    await r.sender.send("hi");
    expect(r.sender.pending[0].state).toBe("sent");
    r.tick(SENT_LINGER_MS - 1);
    r.sender.prune();
    expect(r.sender.pending).toHaveLength(1);
    r.tick(2);
    r.sender.prune();
    expect(r.sender.pending).toEqual([]);
  });

  it("is not sure about a message whose reply was lost, and a retry goes under the same cid", async () => {
    const r = rig();
    r.reply(async () => ({ result: "unknown" }));
    expect(await r.sender.send("hello")).toEqual({ accepted: true, cid: "cid-1" });
    expect(r.sender.pending[0]).toMatchObject({ state: "unknown", text: "hello" });
    r.reply(async () => ({ result: "dup", qid: "q3", state: "queued" }));
    expect(await r.sender.retry("cid-1")).toEqual({ accepted: true, cid: "cid-1" });
    expect(r.sent.map((s) => s.cid)).toEqual(["cid-1", "cid-1"]);
    expect(r.sender.pending[0]).toMatchObject({ state: "queued", qid: "q3" });
    // A message that is not in doubt has no retry.
    expect(await r.sender.retry("cid-1")).toEqual({ accepted: false, reason: "busy" });
    expect(r.sent).toHaveLength(2);
  });

  it("learns from the queue that a message in doubt got there", async () => {
    const r = rig();
    r.reply(async () => ({ result: "unknown" }));
    await r.sender.send("hello");
    r.sender.snapshot(view(["cid-1"]));
    expect(r.sender.pending[0]).toMatchObject({ state: "queued", ahead: 0 });
  });

  it("gives the text back with the reason when the daemon refuses it", async () => {
    const r = rig();
    r.reply(async () => ({ result: "refused", code: "queue_full", message: "full" }));
    expect(await r.sender.send("hello")).toEqual({ accepted: false, reason: "refused", code: "queue_full", message: "full", text: "hello" });
    expect(r.sender.pending).toEqual([]);
    // A retry that finds the first try cancelled or dropped is a refusal too.
    r.reply(async () => ({ result: "unknown" }));
    await r.sender.send("again");
    r.reply(async () => ({ result: "dup", qid: "q1", state: "cancelled" }));
    expect(await r.sender.retry("cid-2")).toMatchObject({ accepted: false, reason: "refused", code: "cancelled" });
    expect(r.sender.pending).toEqual([]);
  });

  it("does nothing on a second tap while one send is on its way", async () => {
    const r = rig();
    let release!: (o: SendOutcome) => void;
    r.reply(() => new Promise((res) => (release = res)));
    const first = r.sender.send("one");
    expect(await r.sender.send("one")).toEqual({ accepted: false, reason: "busy" });
    release({ result: "ok", qid: "q1", state: "queued", ahead: 0 });
    await first;
    expect(r.sent).toHaveLength(1);
  });

  it("keeps the draft when offline, and sends nothing", async () => {
    const r = rig();
    r.setOnline(false);
    expect(await r.sender.send("hello")).toEqual({ accepted: false, reason: "offline" });
    expect(r.sent).toEqual([]);
    expect(r.sender.pending).toEqual([]);
    // Offline at the reply: nothing was sent either.
    r.setOnline(true);
    r.reply(async () => ({ result: "offline" }));
    expect(await r.sender.send("hello")).toEqual({ accepted: false, reason: "offline" });
    expect(r.sender.pending).toEqual([]);
  });

  it("refuses at once what the daemon would refuse, and what only the computer can run", async () => {
    const r = rig("codex");
    for (const text of ["/compact", "/review now", "!ls", "  /new"]) {
      expect(await r.sender.send(text), text).toEqual({ accepted: false, reason: "command" });
    }
    expect(await r.sender.send("   ")).toEqual({ accepted: false, reason: "empty" });
    expect(await r.sender.send("é".repeat(MAX_SEND_BYTES / 2 + 1))).toEqual({ accepted: false, reason: "too_long" });
    expect(r.sent).toEqual([]);
    expect(refuseLocally("codex", "fix a/b", [])).toBeNull();
    // Claude runs its own slash commands, bar the terminal-only ones; ACP agents too, bar the ones the client handles.
    const commands: ChatCommand[] = [{ name: "login", tuiOnly: true }, { name: "review" }];
    expect(refuseLocally("claude", "/review", commands)).toBeNull();
    expect(refuseLocally("claude", "/login", commands)).toBe("tui");
    expect(refuseLocally("omp", "/review", [])).toBeNull();
    for (const text of ["/new", "/clear", "/reset"]) expect(refuseLocally("omp", text, [])).toBe("command");
    expect(refuseLocally("omp", "/new thing", [])).toBeNull();
  });

  it("lets the computer's restart show what was waiting as lost, and sends nothing again", async () => {
    const r = rig();
    await r.sender.send("waiting");
    r.reply(async () => ({ result: "unknown" }));
    await r.sender.send("in doubt");
    r.reply(async () => ({ result: "ok", qid: "q9", state: "sent", ahead: 0 }));
    await r.sender.send("written");
    r.sender.epochChanged();
    expect(r.sender.pending.map((p) => [p.text, p.state])).toEqual([["waiting", "lost"], ["in doubt", "lost"]]);
    expect(r.sent).toHaveLength(3);
    expect(await r.sender.take("cid-1")).toBe("waiting");
    expect(r.cancelled, "nothing to take out of a queue that is gone").toEqual([]);
    r.sender.discard("cid-2");
    expect(r.sender.pending).toEqual([]);
  });

  it("takes a waiting message out of the queue to edit it, unless it went out meanwhile", async () => {
    const r = rig();
    await r.sender.send("fix this");
    expect(await r.sender.take("cid-1")).toBe("fix this");
    expect(r.cancelled).toEqual(["q1"]);
    expect(r.sender.pending).toEqual([]);
    await r.sender.send("too late");
    r.cancelResult("too_late");
    expect(await r.sender.take("cid-2")).toBeNull();
    expect(r.sender.pending).toHaveLength(1);
    expect(await r.sender.take("nope")).toBeNull();
  });

  it("shows a message dropped by the daemon as lost, with its text", async () => {
    const r = rig();
    await r.sender.send("waiting");
    r.sender.snapshot(view([], { dropped: [{ qid: "q1", cid: "cid-1", text: "waiting", reason: "revoked", mine: true }] }));
    expect(r.sender.pending[0].state).toBe("lost");
  });

  it("hands the text back when a retry is refused, so no caller can lose it", async () => {
    const r = rig();
    r.reply(async () => ({ result: "unknown" }));
    await r.sender.send("precious words");
    r.reply(async () => ({ result: "refused", code: "queue_full", message: "full" }));
    expect(await r.sender.retry("cid-1")).toMatchObject({ accepted: false, reason: "refused", text: "precious words" });
    expect(r.sender.pending).toEqual([]);
    r.reply(async () => ({ result: "unknown" }));
    await r.sender.send("second");
    r.reply(async () => ({ result: "dup", qid: "q1", state: "dropped" }));
    expect(await r.sender.retry("cid-2")).toMatchObject({ accepted: false, code: "dropped", text: "second" });
  });

  it("keeps a message in doubt as it was when the retry finds no connection", async () => {
    const r = rig();
    r.reply(async () => ({ result: "unknown" }));
    await r.sender.send("in doubt");
    r.reply(async () => ({ result: "offline" }));
    expect(await r.sender.retry("cid-1")).toEqual({ accepted: false, reason: "offline" });
    expect(r.sender.pending[0]).toMatchObject({ state: "unknown", text: "in doubt" });
  });

  it("shows a restart that came while a send was out as lost, not as in doubt", async () => {
    const r = rig();
    let release!: (o: SendOutcome) => void;
    r.reply(() => new Promise((res) => (release = res)));
    const done = r.sender.send("on its way");
    r.sender.epochChanged();
    release({ result: "unknown" });
    await done;
    expect(r.sender.pending[0].state).toBe("lost");
  });

  it("says where a message was cancelled when someone else took it out of the queue, and keeps its text", async () => {
    const r = rig();
    await r.sender.send("mine");
    r.sender.snapshot(view([], { dropped: [{ qid: "q1", cid: "cid-1", text: "mine", reason: "cancelled", mine: true, cancelledBy: "desktop" }] }));
    expect(r.sender.pending[0]).toMatchObject({ state: "cancelled", cancelledOn: "desktop", text: "mine" });
    expect(await r.sender.take("cid-1")).toBe("mine");
    expect(r.cancelled, "nothing to take out of the queue").toEqual([]);
    await r.sender.send("also mine");
    r.sender.snapshot(view([], { dropped: [{ qid: "q2", cid: "cid-2", text: "also mine", reason: "cancelled", mine: true, cancelledBy: "phone" }] }));
    expect(r.sender.pending[0].cancelledOn).toBe("phone");
  });

  it("does not report a cancel of its own as sent or cancelled", async () => {
    const r = rig();
    await r.sender.send("changed my mind");
    r.sender.forgetQueued("q1");
    expect(r.sender.pending).toEqual([]);
    r.sender.snapshot(view([]));
    expect(r.sender.pending).toEqual([]);
  });

  it("redraws a message on screen when it changes: Sending… turns into Retry without opening the chat again", async () => {
    const r = rig();
    let release!: (o: SendOutcome) => void;
    r.reply(() => new Promise((res) => (release = res)));
    // Solid's reactive build, as the phone runs it: under Node `solid-js` resolves to the server build, which has no reactivity.
    const { createRoot, createSignal, mapArray } = (await import("solid-js/dist/solid.js" as string)) as typeof import("solid-js");
    await createRoot(async (dispose) => {
      // As the phone shows them: a signal the sender's listener sets, and rows keyed by message as `<For>` keys them.
      const [pending, setPending] = createSignal(r.sender.pending);
      r.sender.subscribe(() => setPending(r.sender.pending));
      const rows = mapArray(() => pending().filter((p) => p.state !== "queued"), (p) => chipView(p));
      const done = r.sender.send("hello");
      expect(rows().map((c) => [c.state, c.retry])).toEqual([["sending", false]]);
      release({ result: "unknown" });
      await done;
      expect(rows().map((c) => [c.state, c.retry, c.discard])).toEqual([["unknown", true, true]]);
      // And back to "Sending…" when it is retried.
      r.reply(() => new Promise((res) => (release = res)));
      const again = r.sender.retry("cid-1");
      expect(rows().map((c) => [c.state, c.retry])).toEqual([["sending", false]]);
      release({ result: "unknown" });
      await again;
      dispose();
    });
  });

  it("keeps what the person has to decide about past a reload of the page, and nothing else", async () => {
    let kept: readonly PendingSend[] = [];
    const deps = { send: async (): Promise<SendOutcome> => ({ result: "ok", qid: "q1", state: "queued", ahead: 0 }), cancel: async () => "ok" as const, connected: () => true, now: () => 1, newCid: () => "c1", save: (k: readonly PendingSend[]) => void (kept = k) };
    const sender = new ChatSender(deps, "claude", () => []);
    await sender.send("waits in the queue");
    expect(kept, "a queued message lives in the daemon's queue").toEqual([]);
    sender.epochChanged();
    expect(kept.map((p) => [p.text, p.state])).toEqual([["waits in the queue", "lost"]]);
    // A new page: the lost message is there, with its text.
    const next = new ChatSender(deps, "claude", () => [], JSON.parse(JSON.stringify(kept)));
    expect(next.pending.map((p) => [p.text, p.state])).toEqual([["waits in the queue", "lost"]]);
    next.discard("c1");
    expect(kept).toEqual([]);
    // Only messages in those states are taken back, whatever was stored.
    const odd = new ChatSender(deps, "claude", () => [], [{ cid: "x", text: "t", state: "sending", at: 0 }, null as unknown as PendingSend]);
    expect(odd.pending).toEqual([]);
  });

  it("learns from the log that a message was cancelled on the desktop while the phone was offline, and keeps its text", async () => {
    const r = rig();
    await r.sender.send("cancelled while I was away");
    // Back online: the queue no longer lists it and its live `dropped` was missed, so it reads as written for now…
    r.sender.snapshot(view([]));
    expect(r.sender.pending[0].state).toBe("sent");
    // …until the log, replayed from where the phone was, says what really happened.
    r.sender.unsentSeen("cid-1", "cancelled", "desktop");
    expect(r.sender.pending[0]).toMatchObject({ state: "cancelled", cancelledOn: "desktop", text: "cancelled while I was away" });
    r.tick(SENT_LINGER_MS * 10);
    r.sender.prune();
    expect(r.sender.pending, "not let go like a sent message").toHaveLength(1);
    // A replay of the same line, or one for a message that is not this phone's, changes nothing.
    r.sender.unsentSeen("cid-1", "cancelled", "phone");
    r.sender.unsentSeen("someone-else", "cancelled", "desktop");
    expect(r.sender.pending.map((p) => [p.state, p.cancelledOn])).toEqual([["cancelled", "desktop"]]);
    // Dropped for another reason: lost, text kept.
    await r.sender.send("dropped while away");
    r.sender.unsentSeen("cid-2", "revoked");
    expect(r.sender.pending[1]).toMatchObject({ state: "lost", text: "dropped while away" });
  });

  it("flags a message it could not keep a copy of, so the screen can say to copy it now", async () => {
    let refuse = true;
    const deps = { send: async (): Promise<SendOutcome> => ({ result: "unknown" }), cancel: async () => "ok" as const, connected: () => true, now: () => 1, newCid: () => "c1", save: (k: readonly PendingSend[]) => (refuse ? k.map((p) => p.cid) : []) };
    const sender = new ChatSender(deps, "claude", () => []);
    await sender.send("only on this page");
    expect(sender.pending[0]).toMatchObject({ state: "unknown", unsaved: true });
    expect(chipView(sender.pending[0]).unsaved).toBe(true);
    refuse = false;
    sender.snapshot(view([]));
    sender.unsentSeen("c1", "cancelled", "desktop");
    expect(sender.pending[0].unsaved, "kept now").toBeUndefined();
  });

  it("tells whoever listens about every change", async () => {
    const r = rig();
    let heard = 0;
    const off = r.sender.subscribe(() => heard++);
    await r.sender.send("x");
    expect(heard).toBeGreaterThanOrEqual(2);
    off();
    const before = heard;
    r.sender.discard("cid-1");
    expect(heard).toBe(before);
  });
});
