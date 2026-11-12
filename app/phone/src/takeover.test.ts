import { describe, expect, it } from "vitest";
import { coalesce, drawnBox, FREEZE_MS, Gesture, LONG_PRESS_MS, TakeoverSession, toPage, type Reply, type TakeoverPort, type TakeoverState, type TkOp } from "./takeover";

describe("where a touch lands on the page", () => {
  const meta = { deviceWidth: 800, deviceHeight: 600 };

  it("maps the drawn picture to the page's CSS pixels, letterboxing left out", () => {
    // A 1600×1200 frame in a 400×600 element: drawn 400×300, 150 px from the top.
    const box = drawnBox({ left: 0, top: 0, width: 400, height: 600 }, 1600, 1200);
    expect(box).toEqual({ left: 0, top: 150, width: 400, height: 300 });
    expect(toPage(200, 300, box, meta)).toEqual({ x: 400, y: 300 });
    expect(toPage(0, 150, box, meta)).toEqual({ x: 0, y: 0 });
    expect(toPage(200, 100, box, meta), "above the picture").toBeNull();
    expect(toPage(1, 1, { left: 0, top: 0, width: 0, height: 0 }, meta)).toBeNull();
    expect(toPage(1, 1, box, {}), "a frame that says nothing of the page").toBeNull();
  });
});

describe("what a finger does", () => {
  it("a short touch is a tap where it went down", () => {
    const g = new Gesture();
    g.down(1, { x: 10, y: 10 }, 0);
    expect(g.move(1, { x: 12, y: 11 }, 50)).toEqual([]);
    expect(g.up(1, { x: 12, y: 11 })).toEqual([{ k: "tap", x: 10, y: 10 }]);
  });

  it("a quick move scrolls the page along with the finger", () => {
    const g = new Gesture();
    g.down(1, { x: 100, y: 300 }, 0);
    expect(g.move(1, { x: 100, y: 250 }, 100)).toEqual([{ k: "wheel", x: 100, y: 300, dx: 0, dy: 50 }]);
    expect(g.move(1, { x: 100, y: 240 }, 120)).toEqual([{ k: "wheel", x: 100, y: 300, dx: 0, dy: 10 }]);
    expect(g.up(1, { x: 100, y: 240 })).toEqual([]);
  });

  it("a long press then a move drags, and lets go where it ends", () => {
    const g = new Gesture();
    g.down(1, { x: 10, y: 10 }, 0);
    expect(g.move(1, { x: 40, y: 10 }, LONG_PRESS_MS + 10)).toEqual([{ k: "down", x: 10, y: 10 }, { k: "move", x: 40, y: 10 }]);
    expect(g.move(1, { x: 60, y: 10 }, LONG_PRESS_MS + 20)).toEqual([{ k: "move", x: 60, y: 10 }]);
    expect(g.up(1, { x: 60, y: 10 })).toEqual([{ k: "up", x: 60, y: 10 }]);
    g.down(2, { x: 1, y: 1 }, 0);
    g.move(2, { x: 30, y: 1 }, LONG_PRESS_MS + 1);
    expect(g.cancel(2)).toEqual([{ k: "up", x: 30, y: 1 }]);
  });

  it("a drag released outside the picture lets go where the finger last was on it, not where it started", () => {
    const g = new Gesture();
    g.down(1, { x: 10, y: 10 }, 0);
    g.move(1, { x: 70, y: 10 }, LONG_PRESS_MS + 5);
    expect(g.up(1, null)).toEqual([{ k: "up", x: 70, y: 10 }]);
  });

  it("a pinch sends nothing: the second finger gives the gesture up until every finger is off", () => {
    const g = new Gesture();
    const sent: TkOp[] = [];
    sent.push(...g.down(1, { x: 100, y: 100 }, 0));
    sent.push(...g.down(2, { x: 200, y: 200 }, 10));
    // Both fingers spread, long enough for a drag, interleaved as a browser delivers them.
    for (let i = 1; i <= 10; i++) {
      sent.push(...g.move(1, { x: 100 - i * 5, y: 100 - i * 5 }, 50 * i + LONG_PRESS_MS));
      sent.push(...g.move(2, { x: 200 + i * 5, y: 200 + i * 5 }, 50 * i + LONG_PRESS_MS));
    }
    sent.push(...g.up(2, { x: 250, y: 250 }));
    sent.push(...g.move(1, { x: 40, y: 40 }, 2000));
    sent.push(...g.up(1, { x: 40, y: 40 }));
    expect(sent).toEqual([]);
    // With every finger off, the next touch is a gesture again.
    g.down(3, { x: 5, y: 5 }, 3000);
    expect(g.up(3, { x: 5, y: 5 })).toEqual([{ k: "tap", x: 5, y: 5 }]);
  });

  it("a second finger during a drag lets the drag go where it is, and nothing more", () => {
    const g = new Gesture();
    g.down(1, { x: 10, y: 10 }, 0);
    g.move(1, { x: 50, y: 10 }, LONG_PRESS_MS + 5);
    expect(g.down(2, { x: 200, y: 200 }, LONG_PRESS_MS + 10)).toEqual([{ k: "up", x: 50, y: 10 }]);
    expect(g.move(1, { x: 90, y: 10 }, LONG_PRESS_MS + 20)).toEqual([]);
    expect(g.up(1, { x: 90, y: 10 })).toEqual([]);
    expect(g.up(2, null)).toEqual([]);
  });

  it("merges moves and scrolls in a row, and nothing else", () => {
    const ops: TkOp[] = [{ k: "move", x: 1, y: 1 }, { k: "move", x: 2, y: 2 }, { k: "up", x: 2, y: 2 }, { k: "wheel", x: 5, y: 5, dx: 0, dy: 10 }, { k: "wheel", x: 6, y: 6, dx: 1, dy: 5 }, { k: "key", key: "Tab" }];
    expect(coalesce(ops)).toEqual([{ k: "move", x: 2, y: 2 }, { k: "up", x: 2, y: 2 }, { k: "wheel", x: 5, y: 5, dx: 1, dy: 15 }, { k: "key", key: "Tab" }]);
  });
});

/** A computer that answers as told, and a clock this test moves. */
function rig(replies: Partial<Record<"panes" | "start" | "stop" | "text" | "rebind", Reply>> = {}) {
  let now = 1000;
  const timers: { at: number; fn: () => void; id: number }[] = [];
  let nextId = 0;
  const sent: { tk: string; ops: TkOp[]; st: number; w: number }[] = [];
  const acks: number[] = [];
  const texts: { text: string; w: number }[] = [];
  const states: (TakeoverState | null)[] = [];
  const stopped: string[] = [];
  const port: TakeoverPort = {
    panes: async () => replies.panes ?? { t: "ack", panes: [{ pane: "browser-b", title: "Other", host: "x.example", project: "p2", project_name: "B", held: false }, { pane: "browser-a", title: "Sign in", host: "accounts.example", project: "p1", project_name: "A", held: false }] },
    start: async () => replies.start ?? { t: "ack", result: "ok", tk: "tk1", title: "Sign in", host: "accounts.example" },
    stop: async (tk) => {
      stopped.push(tk);
      return replies.stop ?? { t: "ack" };
    },
    text: async (_tk, text, _st, w) => {
      texts.push({ text, w });
      return replies.text ?? { t: "ack", result: "sent" };
    },
    rebind: async () => replies.rebind ?? { t: "ack" },
    input: (tk, ops, st, w) => sent.push({ tk, ops, st, w }),
    ack: (_tk, seq) => void acks.push(seq),
    serverNow: () => now + 5,
    now: () => now,
    setTimeout: (fn, ms) => {
      const id = ++nextId;
      timers.push({ at: now + ms, fn, id });
      return id;
    },
    clearTimeout: (id) => {
      const i = timers.findIndex((x) => x.id === id);
      if (i >= 0) timers.splice(i, 1);
    },
  };
  const advance = (ms: number) => {
    const end = now + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at);
      const next = timers[0];
      if (!next || next.at > end) break;
      timers.shift();
      now = next.at;
      next.fn();
    }
    now = end;
  };
  const session = new TakeoverSession(port, (s) => states.push(s));
  return { session, sent, acks, texts, stopped, advance, state: () => session.state };
}

describe("a takeover from the phone", () => {
  it("a pane the computer hands over after the phone closed the takeover is let go at once", async () => {
    const r = rig();
    await r.session.openPicker(null);
    const starting = r.session.pick("browser-a");
    r.session.close();
    await starting;
    expect(r.stopped).toEqual(["tk1"]);
    expect(r.state()).toBeNull();
  });

  it("lists the panes of the session's project first, and starts live", async () => {
    const r = rig();
    await r.session.openPicker("p1");
    expect(r.state()?.panes?.map((p) => p.pane)).toEqual(["browser-a", "browser-b"]);
    await r.session.pick("browser-a");
    expect(r.state()).toMatchObject({ phase: "live", tk: "tk1", title: "Sign in", host: "accounts.example" });
  });

  it("shows a refusal and stays on the list", async () => {
    const r = rig({ start: { t: "error", code: "host_busy", message: "The computer is too busy" } });
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    expect(r.state()).toMatchObject({ phase: "pick", message: "The computer is too busy", tk: null });
  });

  it("acknowledges a frame once it is shown, and sends taps at once and moves merged", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 4, data: "jpeg", meta: { deviceWidth: 800, deviceHeight: 600 } });
    expect(r.state()?.frame).toEqual({ seq: 4, data: "jpeg", meta: { deviceWidth: 800, deviceHeight: 600 } });
    r.session.onMessage({ type: "tk_frame", tk: "other", seq: 9, data: "not ours" });
    expect(r.state()?.frame?.seq).toBe(4);
    r.session.shown(4);
    expect(r.acks).toEqual([4]);
    r.session.input([{ k: "tap", x: 1, y: 2 }]);
    expect(r.sent).toEqual([{ tk: "tk1", ops: [{ k: "tap", x: 1, y: 2 }], st: 1005, w: 0 }]);
    r.session.input([{ k: "wheel", x: 1, y: 1, dx: 0, dy: 5 }]);
    r.session.input([{ k: "wheel", x: 1, y: 1, dx: 0, dy: 7 }]);
    expect(r.sent.length).toBe(1);
    r.advance(40);
    expect(r.sent[1].ops).toEqual([{ k: "wheel", x: 1, y: 1, dx: 0, dy: 12 }]);
  });

  it("dims after a few silent seconds, drops input meanwhile, and goes live again when the computer is heard", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.advance(FREEZE_MS + 600);
    expect(r.state()?.phase).toBe("frozen");
    r.session.input([{ k: "tap", x: 1, y: 1 }]);
    expect(r.sent, "no tap is kept for later").toEqual([]);
    r.session.onMessage({ type: "tk_fhb", tk: "tk1", seq: 0 });
    expect(r.state()?.phase).toBe("live");
  });

  it("says text was sent or not, and never sends it twice", async () => {
    const r = rig({ text: { t: "unknown" } });
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    expect(await r.session.type("hunter2")).toBe(false);
    expect(r.state()?.text).toBe("not_sent");
    expect(r.texts).toEqual([{ text: "hunter2", w: 0 }]);
  });

  it("ends when the computer says so, and after a reconnect the computer let go of", async () => {
    const r = rig({ rebind: { t: "error", code: "tk_gone", message: "That takeover has ended." } });
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    await r.session.reconnected();
    expect(r.state()?.phase).toBe("ended");
    const again = rig();
    await again.session.openPicker(null);
    await again.session.pick("browser-a");
    again.session.onMessage({ type: "tk_end", tk: "tk1", reason: "desktop" });
    expect(again.state()).toMatchObject({ phase: "ended", message: "Stopped on the computer.", frame: null });
    again.session.input([{ k: "tap", x: 1, y: 1 }]);
    expect(again.sent).toEqual([]);
  });

  it("names the window each input was mapped against, and keeps input for two windows apart", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 1, data: "pane", meta: { deviceWidth: 800, deviceHeight: 600, window_id: 4 } });
    r.session.input([{ k: "wheel", x: 1, y: 1, dx: 0, dy: 5 }]);
    // A popup opened: its frame comes before the scroll above went.
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 2, data: "popup", meta: { deviceWidth: 480, deviceHeight: 520, window: "popup", window_id: 5 } });
    r.session.input([{ k: "wheel", x: 2, y: 2, dx: 0, dy: 7 }]);
    r.advance(40);
    expect(r.sent.map((s) => [s.w, s.ops])).toEqual([[4, [{ k: "wheel", x: 1, y: 1, dx: 0, dy: 5 }]], [5, [{ k: "wheel", x: 2, y: 2, dx: 0, dy: 7 }]]]);
    await r.session.type("code");
    expect(r.texts).toEqual([{ text: "code", w: 5 }]);
  });

  it("acknowledges a frame it could not show, and says the picture is behind until one shows", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 3, data: "garbage", meta: {} });
    r.session.failed(3);
    expect(r.acks).toEqual([3]);
    expect(r.state()?.broken).toBe(true);
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 4, data: "jpeg", meta: {} });
    r.session.shown(4);
    expect(r.state()?.broken).toBe(false);
  });

  it("shows itself stuck when the computer says it is paused, and asks it to go on from the last frame", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 6, data: "jpeg", meta: {} });
    r.session.shown(6);
    r.session.onMessage({ type: "tk_fhb", tk: "tk1", seq: 7, paused: true });
    expect(r.state()?.phase).toBe("frozen");
    expect(r.acks).toEqual([6, 6]);
    r.session.input([{ k: "tap", x: 1, y: 1 }]);
    expect(r.sent).toEqual([]);
    r.session.onMessage({ type: "tk_fhb", tk: "tk1", seq: 7, paused: false });
    expect(r.state()?.phase).toBe("live");
  });

  it("acknowledges at once a frame identical to the one shown, which the page will not load again", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 1, data: "still", meta: {} });
    r.session.shown(1);
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 2, data: "still", meta: {} });
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 3, data: "still", meta: {} });
    expect(r.acks).toEqual([1, 2, 3]);
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 4, data: "changed", meta: {} });
    expect(r.acks, "a new picture waits until it is shown").toEqual([1, 2, 3]);
  });

  it("asks a paused computer to go on even when it never acknowledged a frame", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_fhb", tk: "tk1", seq: 2, paused: true });
    expect(r.acks).toEqual([0]);
  });

  it("says the pane is hidden on the computer while it is, sends nothing then, and is live again once it shows", async () => {
    const r = rig();
    await r.session.openPicker(null);
    await r.session.pick("browser-a");
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 1, data: "jpeg", meta: {} });
    r.session.onMessage({ type: "tk_fhb", tk: "tk1", seq: 1, paused: false, hidden: true });
    expect(r.state()?.phase).toBe("hidden");
    r.session.input([{ k: "tap", x: 1, y: 1 }]);
    expect(await r.session.type("pw"), "no text either").toBe(false);
    expect(r.sent).toEqual([]);
    r.session.onMessage({ type: "tk_frame", tk: "tk1", seq: 2, data: "late", meta: {} });
    expect(r.state()?.phase, "a frame still on its way does not make it look live").toBe("hidden");
    r.session.onMessage({ type: "tk_fhb", tk: "tk1", seq: 2, paused: false, hidden: false });
    expect(r.state()?.phase).toBe("live");
  });
});