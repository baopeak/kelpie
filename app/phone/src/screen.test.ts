import { describe, expect, it } from "vitest";
import {
  avcC,
  avcChunk,
  canDecode,
  clampView,
  coalesce,
  codecString,
  endedText,
  fitView,
  FREEZE_MS,
  Gestures,
  LONG_PRESS_MS,
  nalType,
  nalUnits,
  Overlay,
  parseMedia,
  qualityText,
  remoteText,
  sameSetup,
  ScreenSession,
  setupFor,
  STUCK_MS,
  toClient,
  toSource,
  zoomAt,
  type Frame,
  type Reply,
  type ScOp,
  type ScreenPort,
  type ScreenState,
} from "./screen";

const bytes = (...b: number[]) => new Uint8Array(b);

describe("the computer's media messages", () => {
  it("reads a video message as the helper writes it", () => {
    // The fixture of crates/kelpie-screen/src/wire.rs (a_video_message_has_its_header_where_the_phone_reads_it).
    const m = parseMedia(bytes(1, 1, 2, 0, 9, 0, 0, 0, 0xe8, 3, 0, 0, 100, 0, 50, 0, 100, 0, 50, 0, 0, 0, 0, 1, 0x41, 0x9a));
    expect(m).toMatchObject({ kind: "video", key: false, unpaintAll: true, unpaint: [], seq: 9, captureMs: 1000, srcW: 100, srcH: 50, codedW: 100, codedH: 50 });
    expect(m?.kind === "video" && [...m.au]).toEqual([0, 0, 0, 1, 0x41, 0x9a]);
    const withRects = parseMedia(bytes(1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 10, 0xa0, 5, 0, 5, 0xd0, 2, 64, 0, 128, 0, 192, 0, 64, 0, 0, 0, 0, 1, 0x65));
    expect(withRects).toMatchObject({ key: true, unpaint: [{ x: 64, y: 128, w: 192, h: 64 }], srcW: 2560, srcH: 1440, codedW: 1280, codedH: 720 });
  });

  it("reads tiles and the cursor, and nothing that is not one", () => {
    const tile = parseMedia(bytes(1, 2, 1, 0, 5, 0, 0, 0, 77, 0, 0, 0, 0, 2, 0, 0, 0, 2, 0, 1, 0x89, 0x50));
    expect(tile).toMatchObject({ kind: "tile", format: "png", seq: 5, id: 77, rect: { x: 512, y: 0, w: 512, h: 256 } });
    expect(parseMedia(bytes(1, 3, 1, 1, 0xfd, 0xff, 0xff, 0xff, 0x90, 1, 0, 0))).toEqual({ kind: "cursor", shape: 1, visible: true, x: -3, y: 400 });
    for (const bad of [bytes(0x7b, 0x22), bytes(1), bytes(1, 1, 0, 0, 1, 0, 0, 0), bytes(1, 9, 0, 0, 0, 0), bytes(1, 3, 0, 0), bytes(1, 2, 7, 0, ...new Array(20).fill(0))]) {
      expect(parseMedia(bad)).toBeNull();
    }
  });
});

describe("H.264 made ready for WebCodecs", () => {
  const sps = bytes(0x67, 0x4d, 0x00, 0x28, 0xaa, 0xbb);
  const pps = bytes(0x68, 0xee, 0x3c, 0x80);
  const idr = bytes(0x65, 0x88, 0x84, 0x00, 0x10);
  const au = bytes(0, 0, 0, 1, ...sps, 0, 0, 1, ...pps, 0, 0, 0, 1, ...idr, 0);

  it("splits NAL units on both start codes and keeps the codec string of the SPS", () => {
    const nals = nalUnits(au);
    expect(nals.map(nalType)).toEqual([7, 8, 5]);
    expect([...nals[2]]).toEqual([...idr], "the trailing zero is not the picture's, the zero inside is");
    expect(codecString(sps)).toBe("avc1.4d0028");
  });

  it("builds the avcC description and length-prefixed chunks without the parameter sets", () => {
    const d = avcC(sps, pps);
    expect([...d.subarray(0, 8)]).toEqual([1, 0x4d, 0x00, 0x28, 0xff, 0xe1, 0, 6]);
    expect([...d.subarray(8, 14)]).toEqual([...sps]);
    expect([...d.subarray(14, 17)]).toEqual([1, 0, 4]);
    expect([...d.subarray(17)]).toEqual([...pps]);
    expect([...avcChunk(au)]).toEqual([0, 0, 0, 5, ...idr]);
    const s = setupFor(au, 1280, 720)!;
    expect(s.codec).toBe("avc1.4d0028");
    expect(sameSetup(s, setupFor(au, 1280, 720))).toBe(true);
    expect(sameSetup(s, setupFor(au, 1920, 1080)), "a new size reconfigures").toBe(false);
    expect(setupFor(bytes(0, 0, 0, 1, ...idr), 10, 10), "no parameter sets: no decoder").toBeNull();
  });
});

describe("crisp tiles over the video", () => {
  const frame = (over: Partial<{ unpaint: { x: number; y: number; w: number; h: number }[]; unpaintAll: boolean }> = {}) => ({ unpaint: [], unpaintAll: false, srcW: 1000, srcH: 500, ...over });

  it("shows a tile from its frame on, and drops it as the frame that changes its region is drawn", () => {
    const closed: string[] = [];
    const o = new Overlay<string>((img) => closed.push(img));
    o.onFrame(frame());
    o.add({ id: 1, seq: 5, rect: { x: 0, y: 0, w: 512, h: 256 }, image: "a" });
    o.add({ id: 2, seq: 5, rect: { x: 512, y: 0, w: 488, h: 256 }, image: "b" });
    expect(o.drawable(4)).toEqual([], "not over an older frame");
    expect(o.drawable(5).map((t) => t.id)).toEqual([1, 2]);
    o.onFrame(frame({ unpaint: [{ x: 600, y: 100, w: 64, h: 64 }] }));
    expect(o.drawable(9).map((t) => t.id)).toEqual([1]);
    expect(closed).toEqual(["b"], "its image is freed");
    o.onFrame(frame({ unpaintAll: true }));
    expect(o.count).toBe(0);
    o.add({ id: 3, seq: 6, rect: { x: 0, y: 0, w: 64, h: 64 }, image: "c" });
    o.onFrame({ ...frame(), srcW: 800 });
    expect(o.count, "a new size: no tile fits").toBe(0);
  });

  it("a larger tile over the same place replaces the smaller ones under it", () => {
    const o = new Overlay<string>();
    o.onFrame(frame());
    o.add({ id: 1, seq: 1, rect: { x: 0, y: 0, w: 64, h: 64 }, image: "a" });
    o.add({ id: 2, seq: 2, rect: { x: 0, y: 0, w: 128, h: 64 }, image: "b" });
    expect(o.drawable(9).map((t) => t.id)).toEqual([2]);
  });
});

describe("where a finger lands on the picture", () => {
  const el = { left: 0, top: 100, width: 400, height: 300 };

  it("maps the screen to the source at zoom 1 and when zoomed, and back", () => {
    const v = fitView(1600, 1200);
    expect(toSource(200, 250, el, 1600, 1200, v)).toEqual({ x: 800, y: 600 });
    expect(toSource(0, 100, el, 1600, 1200, v)).toEqual({ x: 0, y: 0 });
    expect(toSource(-50, 0, el, 1600, 1200, v), "outside: the nearest edge").toEqual({ x: 0, y: 0 });
    expect(toClient(800, 600, el, 1600, 1200, v)).toEqual({ x: 200, y: 250 });
    const zoomed = zoomAt(el, 1600, 1200, v, 2, 100, 175);
    expect(zoomed.zoom).toBe(2);
    expect(toSource(100, 175, el, 1600, 1200, zoomed), "the point under the fingers stays there").toEqual({ x: 400, y: 300 });
    expect(toSource(1, 1, { left: 0, top: 0, width: 0, height: 0 }, 1600, 1200, v)).toBeNull();
  });

  it("never shows past an edge and never zooms out past the whole picture", () => {
    expect(clampView(el, 1600, 1200, { zoom: 0.5, cx: 0, cy: 0 })).toEqual({ zoom: 1, cx: 800, cy: 600 });
    const v = clampView(el, 1600, 1200, { zoom: 4, cx: 0, cy: 99_999 });
    expect(v.cx).toBe(200);
    expect(v.cy).toBe(1050);
    expect(clampView(el, 1600, 1200, { zoom: 99, cx: 800, cy: 600 }).zoom).toBe(6);
  });
});

describe("what fingers do", () => {
  const el = { left: 0, top: 0, width: 400, height: 225 };
  const f = (zoom = 1): Frame => ({ el, srcW: 1600, srcH: 900, view: { ...fitView(1600, 900), zoom } });

  it("trackpad: a finger moves the cursor by its move, a tap clicks where the cursor is", () => {
    const g = new Gestures();
    g.cursor = { x: 800, y: 450 };
    g.down({ id: 1, x: 100, y: 100 }, 0, f());
    expect(g.move({ id: 1, x: 105, y: 100 }, 30, f()).ops, "within the slop: nothing").toEqual([]);
    const moved = g.move({ id: 1, x: 120, y: 100 }, 60, f()).ops;
    // 20 px of finger at 4 source px a screen px and a gain of 1.2: 96 px.
    expect(moved).toEqual([{ k: "move", x: 896, y: 450 }]);
    expect(g.up({ id: 1, x: 120, y: 100 }, 80, f()).ops).toEqual([]);
    g.down({ id: 1, x: 300, y: 10 }, 100, f());
    expect(g.up({ id: 1, x: 301, y: 11 }, 150, f()).ops, "a tap is where the cursor is, not the finger").toEqual([{ k: "click", x: 896, y: 450, b: "left" }]);
    g.down({ id: 1, x: 300, y: 10 }, 200, f());
    expect(g.up({ id: 1, x: 300, y: 10 }, 200 + LONG_PRESS_MS, f()).ops, "a long press is a right click").toEqual([{ k: "click", x: 896, y: 450, b: "right" }]);
    // Zoomed in twice, the same finger move is half as far.
    g.down({ id: 1, x: 100, y: 100 }, 1000, f(2));
    expect(g.move({ id: 1, x: 120, y: 100 }, 1030, f(2)).ops).toEqual([{ k: "move", x: 944, y: 450 }]);
  });

  it("a long press then a move drags, in either mode, and the system taking the touch lets go", () => {
    const g = new Gestures();
    g.cursor = { x: 100, y: 100 };
    g.down({ id: 1, x: 50, y: 50 }, 0, f());
    expect(g.move({ id: 1, x: 70, y: 50 }, LONG_PRESS_MS + 5, f()).ops).toEqual([{ k: "down", x: 100, y: 100, b: "left" }, { k: "move", x: 196, y: 100 }]);
    expect(g.up({ id: 1, x: 70, y: 50 }, LONG_PRESS_MS + 50, f()).ops).toEqual([{ k: "up", x: 196, y: 100, b: "left" }]);
    const d = new Gestures();
    d.mode = "direct";
    d.down({ id: 1, x: 100, y: 100 }, 0, f());
    expect(d.move({ id: 1, x: 150, y: 100 }, LONG_PRESS_MS + 1, f()).ops).toEqual([{ k: "down", x: 400, y: 400, b: "left" }, { k: "move", x: 600, y: 400 }]);
    expect(d.cancel().ops).toEqual([{ k: "up", x: 600, y: 400, b: "left" }]);
  });

  it("direct: a tap clicks under the finger, a quick drag scrolls in whole notches", () => {
    const g = new Gestures();
    g.mode = "direct";
    g.down({ id: 1, x: 200, y: 100 }, 0, f());
    expect(g.up({ id: 1, x: 202, y: 101 }, 60, f()).ops).toEqual([{ k: "click", x: 800, y: 400, b: "left" }]);
    g.down({ id: 1, x: 200, y: 200 }, 100, f());
    // 50 screen px up = 200 source px: the page moves with the finger, so the wheel goes down, 5 notches.
    expect(g.move({ id: 1, x: 200, y: 150 }, 150, f()).ops).toEqual([{ k: "wheel", x: 800, y: 600, dx: 0, dy: 600 }]);
    expect(g.move({ id: 1, x: 200, y: 148 }, 160, f()).ops, "less than a notch waits").toEqual([]);
    expect(g.up({ id: 1, x: 200, y: 148 }, 170, f()).ops).toEqual([]);
  });

  it("two fingers: a tap right-clicks, moving together scrolls, pinching zooms the view and sends nothing", () => {
    const g = new Gestures();
    g.cursor = { x: 10, y: 10 };
    g.down({ id: 1, x: 100, y: 100 }, 0, f());
    g.down({ id: 2, x: 200, y: 100 }, 10, f());
    expect(g.up({ id: 1, x: 100, y: 100 }, 60, f()).ops).toEqual([]);
    expect(g.up({ id: 2, x: 200, y: 100 }, 70, f()).ops).toEqual([{ k: "click", x: 10, y: 10, b: "right" }]);
    g.down({ id: 1, x: 100, y: 100 }, 100, f());
    g.down({ id: 2, x: 200, y: 100 }, 110, f());
    const pinch = g.move({ id: 2, x: 300, y: 100 }, 150, f());
    expect(pinch.ops).toEqual([]);
    expect(pinch.view?.zoom).toBeCloseTo(2);
    g.cancel();
    g.down({ id: 1, x: 100, y: 100 }, 200, f());
    g.down({ id: 2, x: 200, y: 100 }, 210, f());
    g.move({ id: 1, x: 100, y: 60 }, 250, f());
    const scrolled = g.move({ id: 2, x: 200, y: 60 }, 260, f());
    expect(scrolled.ops).toEqual([{ k: "wheel", x: 10, y: 10, dx: 0, dy: 240 }]);
  });

  it("merges moves and wheels in a row, and nothing else", () => {
    const ops: ScOp[] = [{ k: "move", x: 1, y: 1 }, { k: "move", x: 2, y: 2 }, { k: "click", x: 2, y: 2, b: "left" }, { k: "wheel", x: 5, y: 5, dx: 0, dy: 120 }, { k: "wheel", x: 6, y: 6, dx: 120, dy: 120 }, { k: "key", key: "Tab", mods: [] }];
    expect(coalesce(ops)).toEqual([{ k: "move", x: 2, y: 2 }, { k: "click", x: 2, y: 2, b: "left" }, { k: "wheel", x: 5, y: 5, dx: 120, dy: 240 }, { k: "key", key: "Tab", mods: [] }]);
  });
});

/** A computer that answers as told, and a clock this test moves. */
function rig(replies: Partial<Record<"targets" | "start" | "stop" | "text" | "rebind", Reply>> = {}) {
  let now = 1000;
  const timers: { at: number; fn: () => void; id: number }[] = [];
  let nextId = 0;
  const sent: { sc: string; ops: ScOp[]; st: number }[] = [];
  const acks: number[] = [];
  const keyframes: number[] = [];
  const resumed: string[] = [];
  const texts: string[] = [];
  const states: (ScreenState | null)[] = [];
  const port: ScreenPort = {
    targets: async () => replies.targets ?? { t: "ack", targets: [{ id: "m:\\\\.\\DISPLAY1", kind: "monitor", title: "Screen 1", app: "", shortcut: null, w: 1920, h: 1080, minimized: false, primary: true }] },
    start: async () => replies.start ?? { t: "ack", result: "ok", sc: "sc1", title: "main.rs — kelpie", kind: "window", encoder: "hw" },
    stop: async () => replies.stop ?? { t: "ack" },
    text: async (_sc, text) => {
      texts.push(text);
      return replies.text ?? { t: "ack", result: "sent" };
    },
    rebind: async () => replies.rebind ?? { t: "ack" },
    resume: (sc) => resumed.push(sc),
    input: (sc, ops, st) => sent.push({ sc, ops, st }),
    ack: (_sc, seq) => acks.push(seq),
    keyframe: () => keyframes.push(now),
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
  const s = new ScreenSession(port, (x) => states.push(x));
  return { s, sent, acks, keyframes, resumed, texts, states, advance };
}

describe("one screen from the phone", () => {
  it("lists what can be seen, starts, acknowledges decoded frames and stops", async () => {
    const r = rig();
    await r.s.openPicker();
    expect(r.s.state?.targets?.[0].title).toBe("Screen 1");
    await r.s.pick("m:\\\\.\\DISPLAY1");
    expect(r.s.state).toMatchObject({ phase: "live", sc: "sc1", kind: "window" });
    r.s.media();
    r.s.decoded(1);
    expect(r.acks).toEqual([1]);
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 15, h: 720, reason: "load", degraded: true });
    expect(qualityText(r.s.state!.stats, (x) => x)).toBe("720p · 15 fps · reduced: computer busy");
    r.s.onMessage({ type: "sc_state", sc: "other", state: "locked" });
    expect(r.s.state?.remote, "another screen's word changes nothing").toBe("live");
    r.s.onMessage({ type: "sc_state", sc: "sc1", state: "secure_desktop" });
    expect(remoteText(r.s.state!.remote, (x) => x)).toContain("UAC");
    await r.s.stop();
    expect(r.s.state?.phase).toBe("ended");
  });

  it("a refusal stays on the picker with the computer's words", async () => {
    const r = rig({ start: { t: "error", code: "host_busy", message: "The computer is too busy." } });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.s.state).toMatchObject({ phase: "pick", message: "The computer is too busy." });
  });

  it("input goes merged and stamped while live, and is dropped, not kept, otherwise", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.input([{ k: "move", x: 1, y: 1 }]);
    r.s.input([{ k: "move", x: 2, y: 2 }]);
    expect(r.sent).toEqual([], "moves wait a moment for the next");
    r.s.input([{ k: "click", x: 2, y: 2, b: "left" }]);
    expect(r.sent).toEqual([{ sc: "sc1", ops: [{ k: "move", x: 2, y: 2 }, { k: "click", x: 2, y: 2, b: "left" }], st: 1005 }]);
    r.advance(FREEZE_MS + 300);
    expect(r.s.state?.phase).toBe("frozen");
    r.s.input([{ k: "click", x: 9, y: 9, b: "left" }]);
    expect(r.sent.length, "nothing is sent while frozen").toBe(1);
    r.s.media();
    expect(r.s.state?.phase).toBe("live");
  });

  it("frames that come and never decode ask for a keyframe, at most once a second", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    for (let i = 0; i < 12; i++) {
      r.advance(250);
      r.s.media();
    }
    expect(r.keyframes.length).toBeGreaterThanOrEqual(1);
    expect(r.keyframes.length).toBeLessThanOrEqual(2);
    expect(r.keyframes[0] - 1000).toBeGreaterThan(STUCK_MS);
  });

  it("a pause to protect the computer is resumed by a tap, which is not sent as a click", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.onMessage({ type: "sc_state", sc: "sc1", state: "paused_load" });
    r.s.input([{ k: "click", x: 1, y: 1, b: "left" }]);
    expect(r.sent).toEqual([]);
    expect(r.resumed).toEqual(["sc1"]);
  });

  it("text is said as sent or not, and never sent twice", async () => {
    const r = rig({ text: { t: "error", code: "not_sent", message: "late" } });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(await r.s.type("hunter2")).toBe(false);
    expect(r.s.state?.text).toBe("not_sent");
    expect(r.texts).toEqual(["hunter2"]);
  });

  it("a connection back asks for the screen again, and says so when it is gone", async () => {
    const r = rig({ rebind: { t: "error", code: "sc_gone", message: "gone" } });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    await r.s.reconnected();
    expect(r.s.state).toMatchObject({ phase: "ended", message: endedText("link_lost", (x) => x) });
    r.s.onMessage({ type: "sc_end", sc: "sc1", reason: "hotkey" });
    expect(r.s.state?.message, "an ended screen hears nothing more").toBe(endedText("link_lost", (x) => x));
    expect(endedText("hotkey", (x) => x)).toContain("hotkey");
  });
});

describe("whether this browser can show the screen", () => {
  it("needs WebCodecs with an H.264 decoder", async () => {
    expect(await canDecode({})).toBe(false);
    expect(await canDecode({ VideoDecoder: { isConfigSupported: async () => ({ supported: false }) } })).toBe(false);
    expect(await canDecode({ VideoDecoder: { isConfigSupported: async () => { throw new Error("no"); } } })).toBe(false);
    expect(await canDecode({ VideoDecoder: { isConfigSupported: async (c: unknown) => ({ supported: (c as { optimizeForLatency?: boolean }).optimizeForLatency === true }) } })).toBe(true);
  });
});
