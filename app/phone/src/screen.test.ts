import { describe, expect, it } from "vitest";
import {
  avcC,
  avcChunk,
  Backlog,
  BACKLOG_RUN,
  followHostCursor,
  CURSOR_ECHO_MS,
  CURSOR_SNAP_PX,
  FrameTimes,
  reconcileCursor,
  canDecode,
  clampView,
  coalesce,
  codecString,
  detailsText,
  endedText,
  fitView,
  FREEZE_MS,
  Gestures,
  KEYS,
  KICK_EVERY_MS,
  LONG_PRESS_MS,
  MODS,
  nalType,
  nalUnits,
  Overlay,
  parseMedia,
  qualityText,
  REBIND_GRACE_MS,
  remoteText,
  sameSetup,
  ScreenSession,
  setupFor,
  SHARPEN_GAP_MS,
  STUCK_MS,
  toClient,
  toSource,
  visibleRect,
  zoomAt,
  type Frame,
  type Reply,
  type ScOp,
  type ScreenPort,
  type ScreenState,
} from "./screen";
import { pathRows, type ViaId } from "./screen-paths";
import type { PeerEvents } from "./screen-rtc";

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
    expect([...nals[2]], "the trailing zero is not the picture's, the zero inside is").toEqual([...idr]);
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
  const frame = (over: Partial<{ seq: number; unpaint: { x: number; y: number; w: number; h: number }[]; unpaintAll: boolean }> = {}) => ({ seq: 0, unpaint: [], unpaintAll: false, srcW: 1000, srcH: 500, ...over });

  it("shows a tile from its frame on, and drops it as the frame that changes its region is drawn", () => {
    const closed: string[] = [];
    const o = new Overlay<string>((img) => closed.push(img));
    o.onFrame(frame());
    o.add({ id: 1, seq: 5, rect: { x: 0, y: 0, w: 512, h: 256 }, image: "a" });
    o.add({ id: 2, seq: 5, rect: { x: 512, y: 0, w: 488, h: 256 }, image: "b" });
    expect(o.drawable(4), "not over an older frame").toEqual([]);
    expect(o.drawable(5).map((t) => t.id)).toEqual([1, 2]);
    o.onFrame(frame({ unpaint: [{ x: 600, y: 100, w: 64, h: 64 }] }));
    expect(o.drawable(9).map((t) => t.id)).toEqual([1]);
    expect(closed, "its image is freed").toEqual(["b"]);
    o.onFrame(frame({ unpaintAll: true }));
    expect(o.count).toBe(0);
    o.add({ id: 3, seq: 6, rect: { x: 0, y: 0, w: 64, h: 64 }, image: "c" });
    o.onFrame({ ...frame(), srcW: 800 });
    expect(o.count, "a new size: no tile fits").toBe(0);
  });

  it("a tile decoded only after a later frame changed its region is refused, and one elsewhere is not", () => {
    const closed: string[] = [];
    const o = new Overlay<string>((img) => closed.push(img));
    o.onFrame(frame());
    // Frame 6 changes the top-left while the image of tile 5 (matching frame 5) was still being decoded.
    o.onFrame(frame({ seq: 6, unpaint: [{ x: 0, y: 0, w: 64, h: 64 }] }));
    expect(o.add({ id: 1, seq: 5, rect: { x: 0, y: 0, w: 128, h: 64 }, image: "late" }), "it would cover the change").toBe(false);
    expect(closed).toEqual(["late"]);
    expect(o.add({ id: 2, seq: 5, rect: { x: 512, y: 0, w: 64, h: 64 }, image: "fine" })).toBe(true);
    expect(o.add({ id: 3, seq: 6, rect: { x: 0, y: 0, w: 64, h: 64 }, image: "new" }), "a tile of that frame or later is fine").toBe(true);
    o.onFrame(frame({ seq: 9, unpaintAll: true }));
    expect(o.add({ id: 4, seq: 8, rect: { x: 900, y: 400, w: 64, h: 64 }, image: "x" }), "unpaint-all covers everything").toBe(false);
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

  it("a second finger during a one-finger gesture is ignored, and the first finger ends it", () => {
    const g = new Gestures();
    g.cursor = { x: 100, y: 100 };
    g.down({ id: 1, x: 50, y: 50 }, 0, f());
    g.move({ id: 1, x: 70, y: 50 }, 30, f());
    expect(g.idle()).toBe(false);
    g.down({ id: 2, x: 300, y: 200 }, 40, f());
    expect(g.move({ id: 2, x: 390, y: 200 }, 50, f()).ops, "the second finger moves nothing").toEqual([]);
    expect(g.move({ id: 1, x: 80, y: 50 }, 60, f()).ops).toEqual([{ k: "move", x: 244, y: 100 }]);
    expect(g.up({ id: 1, x: 80, y: 50 }, 70, f()).ops).toEqual([]);
    expect(g.move({ id: 2, x: 395, y: 200 }, 80, f()).ops, "nor after the first lifted").toEqual([]);
    expect(g.up({ id: 2, x: 395, y: 200 }, 90, f()).ops).toEqual([]);
    expect(g.idle()).toBe(true);
  });

  it("merges moves and wheels in a row, and nothing else", () => {
    const ops: ScOp[] = [{ k: "move", x: 1, y: 1 }, { k: "move", x: 2, y: 2 }, { k: "click", x: 2, y: 2, b: "left" }, { k: "wheel", x: 5, y: 5, dx: 0, dy: 120 }, { k: "wheel", x: 6, y: 6, dx: 120, dy: 120 }, { k: "key", key: "Tab", mods: [] }];
    expect(coalesce(ops)).toEqual([{ k: "move", x: 2, y: 2 }, { k: "click", x: 2, y: 2, b: "left" }, { k: "wheel", x: 5, y: 5, dx: 120, dy: 240 }, { k: "key", key: "Tab", mods: [] }]);
  });
});

/** A computer that answers as told, and a clock this test moves. */
interface RigOptions {
  route?: "direct" | "relay";
  hosted?: boolean;
  remembered?: ViaId | null;
  /** What the person's Face ID gives: an assertion, or null when they did not give it. */
  proof?: Record<string, unknown> | null;
  /** This browser has WebRTC (the session may open peers). */
  webrtc?: boolean;
}

/** A peer as the session sees it: what it sent on "i", whether it was started or closed, and its events to fire. */
interface FakePeer {
  sc: string;
  sent: object[];
  started: boolean;
  closed: boolean;
  /** "i" takes messages. */
  open: boolean;
  ev: PeerEvents;
}

function rig(replies: Partial<Record<"targets" | "start" | "stop" | "text" | "rebind" | "via", Reply>> = {}, opts: RigOptions = {}) {
  let now = 1000;
  const timers: { at: number; fn: () => void; id: number }[] = [];
  let nextId = 0;
  const sent: { sc: string; ops: ScOp[]; st: number }[] = [];
  const acks: number[] = [];
  const keyframes: number[] = [];
  const resumed: string[] = [];
  const texts: string[] = [];
  const states: (ScreenState | null)[] = [];
  const stopped: string[] = [];
  const kicks: number[] = [];
  const sharpened: { sc: string; rect: { x: number; y: number; w: number; h: number } }[] = [];
  const starts: { target: string; via: ViaId | null; recheck?: Record<string, unknown> }[] = [];
  const remembered: ViaId[] = [];
  const proofs: string[] = [];
  const peers: FakePeer[] = [];
  const vias: ViaId[] = [];
  const delivered: Uint8Array[] = [];
  const port: ScreenPort = {
    webrtc: () => opts.webrtc ?? false,
    peer: (sc, ev) => {
      const p: FakePeer = { sc, sent: [], started: false, closed: false, open: false, ev };
      peers.push(p);
      return {
        start: async () => void (p.started = true),
        send: (msg) => p.open && !p.closed && p.sent.push(msg) > 0,
        stats: async () => ({ path: "p2p", local: "host", remote: "prflx", relayProtocol: null, rttMs: 12, bytesReceived: 1 }),
        close: () => void (p.closed = true),
      };
    },
    via: async (_sc, via) => {
      vias.push(via);
      return replies.via ?? { t: "ack" };
    },
    deliver: (b) => delivered.push(b),
    targets: async () => replies.targets ?? { t: "ack", targets: [{ id: "m:\\\\.\\DISPLAY1", kind: "monitor", title: "Screen 1", app: "", shortcut: null, w: 1920, h: 1080, minimized: false, primary: true }] },
    start: async (target, via, recheck) => {
      starts.push({ target, via, recheck });
      return replies.start ?? { t: "ack", result: "ok", sc: "sc1", title: "main.rs — kelpie", kind: "window", encoder: "hw" };
    },
    stop: async (sc) => {
      stopped.push(sc);
      return replies.stop ?? { t: "ack" };
    },
    text: async (_sc, text) => {
      texts.push(text);
      return replies.text ?? { t: "ack", result: "sent" };
    },
    rebind: async () => replies.rebind ?? { t: "ack" },
    resume: (sc) => resumed.push(sc),
    input: (sc, ops, st) => sent.push({ sc, ops, st }),
    ack: (_sc, seq) => acks.push(seq),
    keyframe: () => keyframes.push(now),
    sharpen: (sc, rect) => sharpened.push({ sc, rect }),
    kick: () => kicks.push(now),
    route: () => opts.route ?? "direct",
    hosted: () => opts.hosted ?? false,
    remembered: () => opts.remembered ?? null,
    remember: (via) => remembered.push(via),
    proof: async (target) => {
      proofs.push(target);
      return opts.proof === undefined ? { expiry: 1, auth_data: "a", client_data: "c", signature: "s" } : opts.proof;
    },
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
  return { s, sent, acks, keyframes, resumed, texts, states, stopped, kicks, sharpened, starts, remembered, proofs, peers, vias, delivered, advance, now: () => now };
}

describe("one screen from the phone", () => {
  it("a screen the computer starts after the phone closed it is stopped at once", async () => {
    const r = rig();
    await r.s.openPicker();
    const starting = r.s.pick("w:1:2");
    r.s.close();
    await starting;
    expect(r.stopped).toEqual(["sc1"]);
    expect(r.s.state).toBeNull();
  });

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
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 30, h: 1080, crisp: false });
    expect(qualityText(r.s.state!.stats, (x) => x), "the computer turned crisp text off on a slow link").toBe("1080p · 30 fps · crisp text off: slow connection");
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 30, h: 1080, crisp: null });
    expect(qualityText(r.s.state!.stats, (x) => x), "switched off on the computer: nothing to say").toBe("1080p · 30 fps");
    r.s.onMessage({ type: "sc_state", sc: "other", state: "locked" });
    expect(r.s.state?.remote, "another screen's word changes nothing").toBe("live");
    r.s.onMessage({ type: "sc_state", sc: "sc1", state: "secure_desktop" });
    expect(remoteText(r.s.state!.remote, (x) => x)).toContain("UAC");
    r.s.onMessage({ type: "sc_state", sc: "sc1", state: "covered_dialog" });
    expect(remoteText(r.s.state!.remote, (x) => x), "another program's dialog over the shared window").toContain("Share the whole monitor to type into this dialog");
    await r.s.stop();
    expect(r.s.state?.phase).toBe("ended");
  });

  it("a frame that overtakes the start's answer is acknowledged once the screen is known", async () => {
    const r = rig();
    await r.s.openPicker();
    const starting = r.s.pick("w:1:2");
    // The picture comes on its own lane and can arrive first: nothing to acknowledge it with yet.
    r.s.decoded(1);
    r.s.skipped(2);
    expect(r.acks).toEqual([]);
    expect(r.keyframes, "the skipped frame asks for a keyframe: nothing to ask it of yet").toEqual([]);
    await starting;
    expect(r.acks, "the last of them, so the computer does not count them in flight").toEqual([2]);
    expect(r.keyframes.length, "and the keyframe it asked for goes with it").toBe(1);
    r.s.decoded(3);
    expect(r.acks).toEqual([2, 3]);
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
    expect(r.sent, "moves wait a moment for the next").toEqual([]);
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

  it("the cursor moving alone does not count as frames that fail to decode", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    for (let i = 0; i < 12; i++) {
      r.advance(250);
      r.s.media(false);
    }
    expect(r.keyframes).toEqual([]);
    expect(r.s.state?.phase).toBe("live");
  });

  it("frames that will not be shown are acknowledged all the same, and a keyframe is asked for", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.skipped(7);
    expect(r.acks).toEqual([7]);
    expect(r.keyframes.length).toBe(1);
  });

  it("a picture that stops while the computer says it sends and the heartbeat goes on is called stuck", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.decoded(1);
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 15, h: 720 });
    for (let i = 0; i < 10; i++) {
      r.advance(250);
      r.s.onMessage({ type: "sc_fhb", sc: "sc1" });
    }
    expect(r.s.state).toMatchObject({ phase: "live", stuck: true });
    expect(r.keyframes.length).toBeGreaterThan(0);
    r.s.decoded(2);
    expect(r.s.state?.stuck).toBe(false);
    // A still screen sends nothing, and says so: not stuck.
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 0, h: 720 });
    r.advance(3000);
    r.s.onMessage({ type: "sc_fhb", sc: "sc1" });
    r.advance(250);
    expect(r.s.state?.stuck).toBe(false);
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

  it("this phone locking ends the screen at once, and a picker is left alone", async () => {
    const r = rig();
    await r.s.openPicker();
    r.s.locked();
    expect(r.s.state?.phase, "nothing was being seen").toBe("pick");
    await r.s.pick("w:1:2");
    r.s.locked();
    expect(r.s.state).toMatchObject({ phase: "ended", message: "Ended when this phone locked." });
    r.s.input([{ k: "click", x: 1, y: 1, b: "left" }]);
    expect(r.sent).toEqual([]);
  });

  it("the key bar has the Windows key, alone and as a modifier", () => {
    expect(KEYS).toContain("Meta");
    expect(MODS).toEqual(["ctrl", "shift", "alt", "meta"]);
  });

  it("text is said as sent or not, and never sent twice", async () => {
    const r = rig({ text: { t: "error", code: "not_sent", message: "late" } });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(await r.s.type("hunter2")).toBe(false);
    expect(r.s.state?.text).toBe("not_sent");
    expect(r.texts).toEqual(["hunter2"]);
  });

  it("a frozen picture checks the connection every two seconds and counts down the time the computer keeps the screen", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.advance(FREEZE_MS + 250);
    expect(r.s.state).toMatchObject({ phase: "frozen", graceLeft: REBIND_GRACE_MS / 1000 });
    expect(r.kicks.length, "checked at once").toBe(1);
    r.advance(KICK_EVERY_MS - 250);
    expect(r.kicks.length, "not again within two seconds").toBe(1);
    r.advance(250);
    expect(r.kicks.length).toBe(2);
    r.advance(10_000);
    expect(r.kicks.length, "no more than one every two seconds").toBe(7);
    expect(r.s.state?.graceLeft).toBe(Math.ceil((REBIND_GRACE_MS - 12_000) / 1000));
    r.advance(60_000);
    expect(r.s.state?.graceLeft, "never below nothing").toBe(0);
    await r.s.reconnected();
    expect(r.s.state).toMatchObject({ phase: "live", graceLeft: null });
    const kicked = r.kicks.length;
    r.advance(1000);
    expect(r.kicks.length, "a live picture is not checked").toBe(kicked);
  });

  it("each shown picture's age is taken by the computer's clock from the stream's start, and its median is shown each second", async () => {
    const r = rig({ start: { t: "ack", result: "ok", sc: "sc1", title: "Screen 1", kind: "monitor", encoder: "hw", t0: 500 } });
    await r.s.openPicker();
    await r.s.pick("m:1");
    expect(r.s.state?.t0).toBe(500);
    // serverNow is now + 5 = 1005: captured at 400 ms after 500, 105 ms old; at 300, 205; at 450, 55.
    for (const c of [400, 300, 450]) r.s.presented(c);
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 30, h: 1080, kbps: 1200, stages: { ack_rtt_ms_p50: 84.6 } });
    expect(r.s.state?.age).toBe(105);
    expect(detailsText(r.s.state!.stats, "Tailscale", r.s.state!.age, (x, v) => x.replace("{ms}", String(v?.ms)))).toBe("Tailscale · 85 ms round trip · 1200 kbps · 30 fps · 1080p · 105 ms old");
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 30, h: 1080 });
    expect(r.s.state?.age, "a second with no picture shown has no age").toBeNull();
    // A computer that does not say its clock: no age at all.
    const old = rig();
    await old.s.openPicker();
    await old.s.pick("m:1");
    old.s.presented(400);
    old.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 30, h: 1080 });
    expect(old.s.state).toMatchObject({ t0: null, age: null });
    expect(detailsText(null, null, null, (x) => x)).toBe("");
  });

  it("Sharpen asks for the part shown once, only while crisp text is off for a slow link, and at most every three seconds", async () => {
    const r = rig();
    await r.s.openPicker();
    await r.s.pick("m:1");
    const rect = { x: 0, y: 0, w: 960, h: 540 };
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 30, h: 1080, crisp: true });
    expect(r.s.sharpen(rect), "crisp text is on: nothing to sharpen").toBe(false);
    r.s.onMessage({ type: "sc_stats", sc: "sc1", fps: 8, h: 540, crisp: false });
    expect(r.s.sharpen(rect)).toBe(true);
    expect(r.s.sharpen(rect), "not again within three seconds").toBe(false);
    r.advance(SHARPEN_GAP_MS);
    r.s.media();
    expect(r.s.sharpen(null), "no picture shown: nothing").toBe(false);
    expect(r.s.sharpen(rect)).toBe(true);
    expect(r.sharpened).toEqual([{ sc: "sc1", rect }, { sc: "sc1", rect }]);
  });

  it("the part shown is the whole picture at zoom 1 and the zoomed part otherwise, in whole source pixels", () => {
    const el = { left: 0, top: 0, width: 400, height: 225 };
    expect(visibleRect(el, 1920, 1080, fitView(1920, 1080))).toEqual({ x: 0, y: 0, w: 1920, h: 1080 });
    expect(visibleRect(el, 1920, 1080, { zoom: 4, cx: 480, cy: 270 })).toEqual({ x: 240, y: 135, w: 480, h: 270 });
    expect(visibleRect(el, 0, 0, fitView(0, 0))).toBeNull();
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

describe("the cursor the finger moves", () => {
  it("is drawn where the finger put it until the computer's positions catch up, and then follows them", () => {
    const drawn = { shape: 0, visible: true, x: 500, y: 300 };
    // A round trip old: the computer still says where the cursor was before the finger moved it.
    const old = { shape: 1, visible: true, x: 420, y: 300 };
    const held = reconcileCursor(drawn, old, 1000, 900);
    expect(held, "no jump back during the echo, but the shape is the computer's").toEqual({ shape: 1, visible: true, x: 500, y: 300 });
    // Past the hold, the computer's place wins: it is where the cursor really is (a program may have moved it).
    expect(reconcileCursor(drawn, old, 900 + CURSOR_ECHO_MS, 900)).toEqual(old);
    // Close enough is the same place: no jitter of a pixel or two.
    const near = { shape: 0, visible: true, x: 500 + CURSOR_SNAP_PX, y: 300 - CURSOR_SNAP_PX };
    expect(reconcileCursor(drawn, near, 10_000, 0)).toEqual({ ...near, x: 500, y: 300 });
    expect(reconcileCursor(drawn, { ...near, x: 500 + CURSOR_SNAP_PX + 1 }, 10_000, 0).x).toBe(500 + CURSOR_SNAP_PX + 1);
    expect(reconcileCursor(drawn, { ...drawn, visible: false }, 1000, 999).visible, "hidden by the computer is hidden").toBe(false);
  });
});

describe("what the phone measures of each frame", () => {
  it("counts the intervals between frames received and shown, their gaps, the decoder's time and what was never shown", () => {
    const t = new FrameTimes();
    let at = 0;
    for (let seq = 1; seq <= 20; seq++) {
      at += seq === 10 ? 300 : 33;
      t.arrival(seq, at);
      t.decoding(seq, at + 1);
      if (seq === 15) t.skipped(seq);
      else t.presented(seq, at + 6, at + 7);
    }
    const s = t.snapshot();
    expect(s.arrivalInterval.n).toBe(19);
    expect(s.arrivalInterval.p50).toBe(33);
    expect(s.arrivalInterval.max).toBe(300);
    expect(s.arrivalGaps, "one gap over 100 ms and over 250 ms").toEqual([1, 1]);
    expect(s.gapThresholdsMs).toEqual([100, 250]);
    expect(s.presentGaps).toEqual([1, 1]);
    expect(s.decodeMs.p50).toBe(5);
    expect(s.receiveToPresentMs.p95).toBe(7);
    expect(s.dropped, "the skipped one").toBe(1);
    // A frame that never comes out of the decoder counts once it is far behind.
    t.arrival(21, 2000);
    t.arrival(100, 2100);
    expect(t.snapshot().dropped).toBe(2);
  });
});
describe("a trackpad tap after the finger moved the cursor", () => {
  it("clicks where the cursor is drawn, not where a message a round trip old says it was", () => {
    // The finger moved the cursor to 500 and lifted: the echo draws it there.
    let drawn = { shape: 0, visible: true, x: 500, y: 300 };
    let tapAt = { x: 500, y: 300 };
    // The computer's message, a round trip old, still says 440.
    ({ drawn, tapAt } = followHostCursor(drawn, tapAt, { shape: 0, visible: true, x: 440, y: 300 }, 1100, 1000, true));
    expect(drawn.x).toBe(500);
    expect(tapAt, "the tap goes where the cursor is drawn").toEqual({ x: 500, y: 300 });
    // Once the echo is over, the computer's place is drawn, and the tap follows it there.
    ({ drawn, tapAt } = followHostCursor(drawn, tapAt, { shape: 0, visible: true, x: 520, y: 310 }, 2000, 1000, true));
    expect([drawn.x, tapAt.x]).toEqual([520, 520]);
    // With a finger down the gesture's own point is not touched.
    ({ tapAt } = followHostCursor(drawn, { x: 7, y: 8 }, { shape: 0, visible: true, x: 600, y: 600 }, 5000, 1000, false));
    expect(tapAt).toEqual({ x: 7, y: 8 });
  });
});

describe("a decoder that is behind", () => {
  it("is skipped to the next keyframe only past its own usual queue, for several deltas in a row", () => {
    const b = new Backlog();
    // An Android decoder that always keeps three chunks: never behind.
    let skips = 0;
    for (let i = 0; i < 100; i++) if (b.skip(3, i * 33)) skips++;
    expect(skips).toBe(0);
    // It falls behind for real: skipped once the run is long enough.
    const run = [6, 7, 8].map((n, i) => b.skip(n, 4000 + i * 33));
    expect(run).toEqual(Array.from({ length: BACKLOG_RUN }, (_, i) => i === BACKLOG_RUN - 1));
    b.reset();
    expect(b.skip(8, 4200), "a keyframe starts the run again").toBe(false);
    // A decoder that keeps none: three waiting for three deltas is behind.
    const c = new Backlog();
    c.skip(0, 0);
    expect([3, 3, 3].map((n, i) => c.skip(n, 10 + i * 33))).toEqual([false, false, true]);
  });
});
describe("choosing how the screen connects", () => {
  const paths = [{ id: "auto", ok: true }, { id: "tailscale", ok: false, why: "no_direct_link" }, { id: "relay", ok: true }] as const;
  const targets: Reply = { t: "ack", targets: [{ id: "w:1:2", kind: "window", title: "Code", app: "Code.exe", shortcut: "ide", w: 100, h: 100, minimized: false, primary: false }], paths: [...paths], relay: { ours: true, host: "relay.example" } };

  it("preselects Automatic, or the way chosen last when it can be used, and keeps a new choice", async () => {
    const plain = rig({ targets });
    await plain.s.openPicker();
    expect(plain.s.state).toMatchObject({ via: "auto", relay: { ours: true, host: "relay.example" } });
    const kept = rig({ targets }, { remembered: "relay" });
    await kept.s.openPicker();
    expect(kept.s.state?.via).toBe("relay");
    const gone = rig({ targets }, { remembered: "tailscale" });
    await gone.s.openPicker();
    expect(gone.s.state?.via, "a remembered way that cannot be used now is not preselected").toBe("auto");
    kept.s.choosePath("auto");
    expect(kept.s.state?.via).toBe("auto");
    expect(kept.remembered).toEqual(["auto"]);
    kept.s.choosePath("webrtc" as ViaId);
    expect(kept.remembered, "a way the computer did not list is ignored").toEqual(["auto"]);
  });

  it("starts through Tailscale without Face ID, and says which way was used", async () => {
    const r = rig({ targets, start: { t: "ack", result: "ok", sc: "sc1", title: "Code", kind: "window", via: "tailscale" } }, { route: "direct" });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.proofs).toEqual([]);
    expect(r.starts).toEqual([{ target: "w:1:2", via: "auto", recheck: undefined }]);
    expect(r.s.state).toMatchObject({ phase: "live", usedVia: "tailscale" });
  });

  it("asks for Face ID over the screen before a start through the relay, and sends it with the start", async () => {
    const r = rig({ targets, start: { t: "ack", result: "ok", sc: "sc1", title: "Code", kind: "window", via: "relay" } }, { route: "relay", hosted: true });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.proofs).toEqual(["w:1:2"]);
    expect(r.starts[0]).toMatchObject({ via: "auto", recheck: { auth_data: "a", signature: "s" } });
    expect(r.s.state?.usedVia).toBe("relay");
    const chosen = rig({ targets }, { route: "direct" });
    await chosen.s.openPicker();
    chosen.s.choosePath("relay");
    await chosen.s.pick("w:1:2");
    expect(chosen.proofs, "the relay chosen on a direct link needs it too").toEqual(["w:1:2"]);
  });

  it("does not start when Face ID was not given, and the person can try again", async () => {
    const r = rig({ targets }, { route: "relay", proof: null });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.starts).toEqual([]);
    expect(r.s.state).toMatchObject({ phase: "pick" });
    expect(r.s.state?.message).toMatch(/Not verified/);
  });

  it("says Face ID was needed, and shows the computer's words for the other refusals", async () => {
    const face = rig({ targets, start: { t: "error", code: "needs_face_id", message: "Needs Face ID." } }, { route: "relay" });
    await face.s.openPicker();
    await face.s.pick("w:1:2");
    expect(face.s.state).toMatchObject({ phase: "pick" });
    expect(face.s.state?.message).toMatch(/Face ID did not reach your computer in time/);
    for (const code of ["relay_budget_spent", "screen_remote_off"]) {
      const r = rig({ targets, start: { t: "error", code, message: `computer says ${code}` } }, { route: "relay" });
      await r.s.openPicker();
      await r.s.pick("w:1:2");
      expect(r.s.state?.message).toBe(`computer says ${code}`);
    }
  });

  it("a computer that lists no ways is asked as before: no choice, no Face ID", async () => {
    const r = rig({}, { route: "relay" });
    await r.s.openPicker();
    expect(r.s.state?.paths).toBeNull();
    await r.s.pick("w:1:2");
    expect(r.proofs).toEqual([]);
    expect(r.starts).toEqual([{ target: "w:1:2", via: null, recheck: undefined }]);
  });

  it("a screen closed while Face ID is asked is not started", async () => {
    const r = rig({ targets }, { route: "relay" });
    await r.s.openPicker();
    const starting = r.s.pick("w:1:2");
    r.s.close();
    await starting;
    expect(r.starts).toEqual([]);
  });

  it("the details line names the way the computer used, else the phone's own", () => {
    const t = (x: string) => x;
    expect(detailsText({ kbps: 400 }, "this network", null, t, "relay")).toBe("the relay · 400 kbps");
    expect(detailsText({ kbps: 400 }, "the relay", null, t, "tailscale")).toBe("Tailscale · 400 kbps");
    expect(detailsText({ kbps: 400 }, "Tailscale", null, t, "webrtc")).toBe("Tailscale · 400 kbps");
    expect(detailsText({ kbps: 400 }, "Tailscale", null, t)).toBe("Tailscale · 400 kbps");
  });
});

describe("the screen peer to peer", () => {
  const paths = [{ id: "auto", ok: true }, { id: "tailscale", ok: false, why: "no_direct_link" }, { id: "p2p", ok: true }, { id: "relay", ok: true }] as const;
  const targets: Reply = { t: "ack", targets: [{ id: "w:1:2", kind: "window", title: "Code", app: "Code.exe", shortcut: "ide", w: 100, h: 100, minimized: false, primary: false }], paths: [...paths], relay: { ours: true, host: "relay.example" } };
  const onRelay = (via: string) => rig({ targets, start: { t: "ack", result: "ok", sc: "sc1", title: "Code", kind: "window", via } }, { route: "relay", hosted: true, webrtc: true });
  const video = (seq: number) => {
    const b = new Uint8Array(40);
    b[0] = 0x01;
    b[1] = 1;
    new DataView(b.buffer).setUint32(4, seq, true);
    return b;
  };

  it("Automatic on the relay starts at once and opens a peer beside it; acknowledgements and input move onto it once the computer says so", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.s.state).toMatchObject({ phase: "live", usedVia: "relay" });
    expect(r.peers).toHaveLength(1);
    expect(r.peers[0]).toMatchObject({ sc: "sc1", started: true });
    // Before the computer moved the picture: still on the link.
    r.s.decoded(1);
    expect(r.acks).toEqual([1]);
    r.peers[0].open = true;
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "live" });
    expect(r.s.state?.path).toBe("p2p");
    r.s.decoded(2);
    r.s.input([{ k: "click", x: 1, y: 2, b: "left" }]);
    expect(r.acks, "the link no more").toEqual([1]);
    expect(r.peers[0].sent).toEqual([{ type: "sc_ack", seq: 2 }, { type: "sc_input", ops: [{ k: "click", x: 1, y: 2, b: "left" }], st: r.now() + 5 }]);
    // A channel that cannot take it now: the link carries it.
    r.peers[0].open = false;
    r.s.decoded(3);
    expect(r.acks).toEqual([1, 3]);
  });

  it("the details line names P2P from the browser's stats", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "live" });
    r.advance(1200);
    await Promise.resolve();
    await Promise.resolve();
    expect(r.s.state?.pathStats).toMatchObject({ path: "p2p", rttMs: 12 });
    expect(detailsText({ kbps: 900 }, "the relay", null, (x) => x, "relay", r.s.state!.pathStats)).toBe("P2P · {ms} ms round trip · 900 kbps");
    expect(detailsText({ kbps: 900 }, null, null, (x) => x, null, { path: "turn", local: "relay", remote: "host", relayProtocol: "udp", rttMs: null, bytesReceived: 0 })).toBe("TURN · 900 kbps");
  });

  it("a peer that fails leaves the picture on the link, and Automatic does not try again on its own for a while", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "relay", state: "failed", why: "not_open" });
    expect(r.peers[0].closed).toBe(true);
    expect(r.s.state?.p2pFailed).toBe(true);
    await r.s.stop();
    r.s.close();
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.peers, "no new peer within the retry time").toHaveLength(1);
    const row = pathRowsOf(r.s);
    expect(row).toMatchObject({ ok: true, why: "A direct connection did not work from this network a moment ago." });
  });

  it("the picture going back to the link closes the peer, and a computer's switch is followed", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "live" });
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "failed", why: "ice_disconnected" });
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "relay", state: "live", why: "ice_disconnected" });
    expect(r.s.state?.path).toBe("link");
    expect(r.peers[0].closed).toBe(true);
    r.s.decoded(4);
    expect(r.acks).toEqual([4]);
  });

  it("peer to peer from the start opens a peer at once; a reconnect opens a new one", async () => {
    const r = onRelay("p2p");
    await r.s.openPicker();
    r.s.choosePath("p2p");
    await r.s.pick("w:1:2");
    expect(r.starts[0].via).toBe("p2p");
    expect(r.peers).toHaveLength(1);
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "live" });
    await r.s.reconnected();
    expect(r.peers).toHaveLength(2);
    expect(r.peers[0].closed, "the one in use stays until the computer moves to the new one").toBe(false);
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "live" });
    expect(r.peers[0].closed).toBe(true);
    expect(r.peers[1].closed).toBe(false);
  });

  it("switching the way mid-screen asks the computer, and opens a peer for peer to peer", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    r.s.choosePath("relay");
    await r.s.pick("w:1:2");
    expect(r.peers, "the relay chosen: no peer").toHaveLength(0);
    await r.s.switchVia("p2p");
    expect(r.vias).toEqual(["p2p"]);
    expect(r.peers).toHaveLength(1);
    expect(r.remembered.at(-1)).toBe("p2p");
    const refused = rig({ targets, start: { t: "ack", result: "ok", sc: "sc1", title: "Code", kind: "window", via: "relay" }, via: { t: "error", code: "method_off", message: "Turned off." } }, { route: "relay", webrtc: true });
    await refused.s.openPicker();
    refused.s.choosePath("relay");
    await refused.s.pick("w:1:2");
    await refused.s.switchVia("p2p");
    expect(refused.peers).toHaveLength(0);
    expect(refused.s.state?.message).toBe("Turned off.");
  });

  it("a live peer that dropped (the phone moved networks) is not a failure of the way: Automatic offers again once the link is back", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "live" });
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "p2p", state: "failed", why: "ice_disconnected" });
    r.s.onMessage({ type: "sc_path", sc: "sc1", path: "relay", state: "live", why: "ice_disconnected" });
    expect(r.s.state?.p2pFailed).toBe(false);
    await r.s.reconnected();
    expect(r.peers).toHaveLength(2);
    expect(r.peers[1].started).toBe(true);
  });

  it("video older than what the view has is not shown again, whichever way it came", async () => {
    const r = onRelay("relay");
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.s.incoming(video(5))).toBe(true);
    expect(r.s.incoming(video(4)), "the link's last frame after the peer's keyframe").toBe(false);
    expect(r.s.incoming(video(6))).toBe(true);
    expect(r.s.incoming(new Uint8Array([0x01, 3, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0])), "a cursor always goes").toBe(true);
  });

  it("no peer without WebRTC in this browser, and the row says why", async () => {
    const r = rig({ targets, start: { t: "ack", result: "ok", sc: "sc1", title: "Code", kind: "window", via: "relay" } }, { route: "relay", webrtc: false });
    await r.s.openPicker();
    await r.s.pick("w:1:2");
    expect(r.peers).toHaveLength(0);
    expect(pathRowsOf(r.s)).toMatchObject({ ok: false, why: "This browser cannot connect peer to peer (no WebRTC)." });
  });
});

function pathRowsOf(s: ScreenSession) {
  return pathRows(s.state?.paths, s.state?.relay, s.side(s.state?.p2pFailed ?? false), (x) => x).find((r) => r.id === "p2p");
}