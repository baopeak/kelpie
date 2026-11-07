// Seeing the computer's screen from the phone (the daemon's `screen` broker; the app's helper captures and encodes). What is here is plain
// logic, tested without a browser: the media messages the computer sends (crates/kelpie-screen/src/wire.rs), H.264 made ready for WebCodecs
// (`avc` format, an avcC description from the keyframe's SPS and PPS, no frame reordering), the crisp tiles painted over the video and dropped
// when their region changes, where a finger lands on the picture under zoom, the two touch modes, input merged and stamped with the computer's
// clock, frames acknowledged once decoded, and a stream counted lost after a few silent seconds. screen-view.tsx draws it.

export const MEDIA = 0x01;
export const KIND_VIDEO = 1;
export const KIND_TILE = 2;
export const KIND_CURSOR = 3;
const FLAG_KEY = 1;
const FLAG_UNPAINT_ALL = 2;
const VIDEO_HEADER = 20;
const TILE_HEADER = 20;

/** No media or heartbeat for this long: the picture is dimmed and input stops. */
export const FREEZE_MS = 3000;
/** Frames arriving but none decoded for this long: ask for a keyframe. */
export const STUCK_MS = 1000;
/** The computer says it sends frames and none was shown for this long: the picture is called stuck. */
export const STUCK_SHOW_MS = 2000;
/** Moves and scrolls are merged and sent at most this often. */
export const FLUSH_MS = 33;
/** A press held this long without moving is a long press. */
export const LONG_PRESS_MS = 450;
/** A touch that moves less than this (screen pixels) is a tap. */
export const TAP_SLOP = 10;
/** Trackpad mode: the cursor moves this many times as far on the picture as the finger moves on the glass. */
export const TRACKPAD_GAIN = 1.2;
export const MAX_ZOOM = 6;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type Media =
  | { kind: "video"; key: boolean; unpaintAll: boolean; unpaint: Rect[]; seq: number; captureMs: number; srcW: number; srcH: number; codedW: number; codedH: number; au: Uint8Array }
  | { kind: "tile"; format: "png" | "jpeg"; seq: number; id: number; rect: Rect; image: Uint8Array }
  | { kind: "cursor"; shape: number; visible: boolean; x: number; y: number };

/** A media message, or null when it is not one this page reads. */
export function parseMedia(b: Uint8Array): Media | null {
  if (b.length < 4 || b[0] !== MEDIA) return null;
  const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
  const u16 = (o: number) => v.getUint16(o, true);
  const rect = (o: number): Rect => ({ x: u16(o), y: u16(o + 2), w: u16(o + 4), h: u16(o + 6) });
  switch (b[1]) {
    case KIND_VIDEO: {
      const n = b[3];
      if (b.length <= VIDEO_HEADER + n * 8) return null;
      const unpaint: Rect[] = [];
      for (let i = 0; i < n; i++) unpaint.push(rect(VIDEO_HEADER + i * 8));
      return { kind: "video", key: (b[2] & FLAG_KEY) !== 0, unpaintAll: (b[2] & FLAG_UNPAINT_ALL) !== 0, unpaint, seq: v.getUint32(4, true), captureMs: v.getUint32(8, true), srcW: u16(12), srcH: u16(14), codedW: u16(16), codedH: u16(18), au: b.subarray(VIDEO_HEADER + n * 8) };
    }
    case KIND_TILE:
      if (b.length <= TILE_HEADER || (b[2] !== 1 && b[2] !== 2)) return null;
      return { kind: "tile", format: b[2] === 1 ? "png" : "jpeg", seq: v.getUint32(4, true), id: v.getUint32(8, true), rect: rect(12), image: b.subarray(TILE_HEADER) };
    case KIND_CURSOR:
      if (b.length !== 12) return null;
      return { kind: "cursor", shape: b[2], visible: b[3] === 1, x: v.getInt32(4, true), y: v.getInt32(8, true) };
    default:
      return null;
  }
}

// ---------- H.264 for WebCodecs ----------

/** The NAL units of an Annex B access unit, without their start codes. */
export function nalUnits(au: Uint8Array): Uint8Array[] {
  const starts: number[] = [];
  for (let i = 0; i + 2 < au.length; i++) {
    if (au[i] === 0 && au[i + 1] === 0 && au[i + 2] === 1) {
      starts.push(i + 3);
      i += 2;
    }
  }
  const out: Uint8Array[] = [];
  starts.forEach((s, k) => {
    let end = k + 1 < starts.length ? starts[k + 1] - 3 : au.length;
    while (end > s && au[end - 1] === 0) end--;
    if (end > s) out.push(au.subarray(s, end));
  });
  return out;
}

export const nalType = (n: Uint8Array) => (n.length ? n[0] & 0x1f : 0);

const hex2 = (n: number) => n.toString(16).padStart(2, "0");

/** `avc1.PPCCLL` from an SPS NAL unit. */
export function codecString(sps: Uint8Array): string {
  return `avc1.${hex2(sps[1])}${hex2(sps[2])}${hex2(sps[3])}`;
}

/** An AVCDecoderConfigurationRecord (ISO 14496-15) for one SPS and one PPS, 4-byte lengths. */
export function avcC(sps: Uint8Array, pps: Uint8Array): Uint8Array {
  const out = new Uint8Array(11 + sps.length + pps.length);
  out.set([1, sps[1], sps[2], sps[3], 0xff, 0xe1, sps.length >> 8, sps.length & 0xff], 0);
  out.set(sps, 8);
  out.set([1, pps.length >> 8, pps.length & 0xff], 8 + sps.length);
  out.set(pps, 11 + sps.length);
  return out;
}

export interface DecoderSetup {
  codec: string;
  description: Uint8Array;
  codedWidth: number;
  codedHeight: number;
}

/** What a decoder needs for a keyframe's stream, or null without both parameter sets. */
export function setupFor(au: Uint8Array, codedW: number, codedH: number): DecoderSetup | null {
  const nals = nalUnits(au);
  const sps = nals.find((n) => nalType(n) === 7);
  const pps = nals.find((n) => nalType(n) === 8);
  if (!sps || !pps || sps.length < 4) return null;
  return { codec: codecString(sps), description: avcC(sps, pps), codedWidth: codedW, codedHeight: codedH };
}

export function sameSetup(a: DecoderSetup | null, b: DecoderSetup | null): boolean {
  if (!a || !b) return a === b;
  return a.codec === b.codec && a.codedWidth === b.codedWidth && a.codedHeight === b.codedHeight && a.description.length === b.description.length && a.description.every((x, i) => x === b.description[i]);
}

/** An access unit as an `avc` chunk: each NAL unit with a 4-byte length, parameter sets and delimiters left out (they are in the description). */
export function avcChunk(au: Uint8Array): Uint8Array {
  const nals = nalUnits(au).filter((n) => ![7, 8, 9].includes(nalType(n)));
  const out = new Uint8Array(nals.reduce((s, n) => s + 4 + n.length, 0));
  let o = 0;
  for (const n of nals) {
    new DataView(out.buffer).setUint32(o, n.length);
    out.set(n, o + 4);
    o += 4 + n.length;
  }
  return out;
}

// ---------- paint-over tiles ----------

const overlaps = (a: Rect, b: Rect) => a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;

export interface Tile<T> {
  id: number;
  seq: number;
  rect: Rect;
  image: T;
}

/** How many frames' unpaints are remembered, to refuse a tile that arrives after a frame that changed its region. */
const UNPAINT_HISTORY = 64;

/** The crisp tiles over the video: a tile is drawn once the video frame it matches is shown, and dropped as the frame that changes its region
 *  is drawn, or skipped. A tile whose image is decoded only after a later frame changed its region (decoding is asynchronous) is refused: the
 *  unpaints of the last frames are remembered for that. `T` is what the page decoded the image to (an ImageBitmap), with `close` to free it. */
export class Overlay<T> {
  private tiles = new Map<number, Tile<T>>();
  private size = { w: 0, h: 0 };
  private history: { seq: number; all: boolean; rects: Rect[] }[] = [];

  constructor(private close: (image: T) => void = () => {}) {}

  /** False when the tile came too late: a frame after the one it matches changed its region. */
  add(t: Tile<T>): boolean {
    if (this.history.some((h) => h.seq > t.seq && (h.all || h.rects.some((r) => overlaps(r, t.rect))))) {
      this.close(t.image);
      return false;
    }
    const old = this.tiles.get(t.id);
    if (old) this.close(old.image);
    // A newer tile over the same place replaces the ones under it.
    for (const [id, x] of this.tiles) {
      if (overlaps(x.rect, t.rect) && x.rect.x >= t.rect.x && x.rect.y >= t.rect.y && x.rect.x + x.rect.w <= t.rect.x + t.rect.w && x.rect.y + x.rect.h <= t.rect.y + t.rect.h) {
        this.close(x.image);
        this.tiles.delete(id);
      }
    }
    this.tiles.set(t.id, t);
    return true;
  }

  /** A video frame is drawn, or skipped: what changed under tiles drops them, and a new source size drops all. */
  onFrame(f: { seq: number; unpaint: Rect[]; unpaintAll: boolean; srcW: number; srcH: number }) {
    const resized = f.srcW !== this.size.w || f.srcH !== this.size.h;
    this.history.push({ seq: f.seq, all: f.unpaintAll || resized, rects: f.unpaint });
    if (this.history.length > UNPAINT_HISTORY) this.history.shift();
    if (f.unpaintAll || resized) {
      this.clear();
      this.size = { w: f.srcW, h: f.srcH };
      return;
    }
    if (!f.unpaint.length) return;
    for (const [id, t] of this.tiles) {
      if (f.unpaint.some((r) => overlaps(r, t.rect))) {
        this.close(t.image);
        this.tiles.delete(id);
      }
    }
  }

  /** The tiles to draw over frame `shown`. */
  drawable(shown: number): Tile<T>[] {
    return [...this.tiles.values()].filter((t) => t.seq <= shown);
  }

  clear() {
    for (const t of this.tiles.values()) this.close(t.image);
    this.tiles.clear();
  }

  get count() {
    return this.tiles.size;
  }
}

// ---------- where things are ----------

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** How the picture is shown: zoom 1 fits it in the element; `x`/`y` is the source point at the element's centre. */
export interface View {
  zoom: number;
  cx: number;
  cy: number;
}

export const fitView = (srcW: number, srcH: number): View => ({ zoom: 1, cx: srcW / 2, cy: srcH / 2 });

/** Source pixels per screen pixel at a view. */
export function scaleOf(el: Box, srcW: number, srcH: number, v: View): number {
  if (!srcW || !srcH || !el.width || !el.height) return 0;
  return Math.min(el.width / srcW, el.height / srcH) * v.zoom;
}

/** A point on the phone's screen → source pixels (clamped to the picture), or null when there is no picture. */
export function toSource(clientX: number, clientY: number, el: Box, srcW: number, srcH: number, v: View): { x: number; y: number } | null {
  const s = scaleOf(el, srcW, srcH, v);
  if (!s) return null;
  const x = v.cx + (clientX - (el.left + el.width / 2)) / s;
  const y = v.cy + (clientY - (el.top + el.height / 2)) / s;
  return { x: Math.round(Math.min(Math.max(x, 0), srcW - 1)), y: Math.round(Math.min(Math.max(y, 0), srcH - 1)) };
}

/** A source point → the phone's screen (for the cursor and the tiles). */
export function toClient(x: number, y: number, el: Box, srcW: number, srcH: number, v: View): { x: number; y: number } {
  const s = scaleOf(el, srcW, srcH, v);
  return { x: el.left + el.width / 2 + (x - v.cx) * s, y: el.top + el.height / 2 + (y - v.cy) * s };
}

/** Keeps the view inside the picture: at zoom 1 centred, zoomed never showing past an edge. */
export function clampView(el: Box, srcW: number, srcH: number, v: View): View {
  const zoom = Math.min(Math.max(v.zoom, 1), MAX_ZOOM);
  const s = scaleOf(el, srcW, srcH, { ...v, zoom });
  if (!s) return { zoom, cx: v.cx, cy: v.cy };
  const halfW = el.width / s / 2;
  const halfH = el.height / s / 2;
  const clamp = (c: number, half: number, size: number) => (half * 2 >= size ? size / 2 : Math.min(Math.max(c, half), size - half));
  return { zoom, cx: clamp(v.cx, halfW, srcW), cy: clamp(v.cy, halfH, srcH) };
}

/** Pinch: zoom by `factor` around the screen point (keeping the source point under the fingers where it is). */
export function zoomAt(el: Box, srcW: number, srcH: number, v: View, factor: number, clientX: number, clientY: number): View {
  const before = toSourceRaw(clientX, clientY, el, srcW, srcH, v);
  const zoom = Math.min(Math.max(v.zoom * factor, 1), MAX_ZOOM);
  const s = scaleOf(el, srcW, srcH, { ...v, zoom });
  if (!before || !s) return v;
  const cx = before.x - (clientX - (el.left + el.width / 2)) / s;
  const cy = before.y - (clientY - (el.top + el.height / 2)) / s;
  return clampView(el, srcW, srcH, { zoom, cx, cy });
}

function toSourceRaw(clientX: number, clientY: number, el: Box, srcW: number, srcH: number, v: View) {
  const s = scaleOf(el, srcW, srcH, v);
  if (!s) return null;
  return { x: v.cx + (clientX - (el.left + el.width / 2)) / s, y: v.cy + (clientY - (el.top + el.height / 2)) / s };
}

// ---------- input ----------

export type Button = "left" | "right" | "middle";
export type ScOp =
  | { k: "move"; x: number; y: number }
  | { k: "down" | "up"; x: number; y: number; b: Button }
  | { k: "click"; x: number; y: number; b: Button; n?: 1 | 2 }
  | { k: "wheel"; x: number; y: number; dx: number; dy: number }
  | { k: "key"; key: string; mods: Mod[] };
export type Mod = "ctrl" | "shift" | "alt" | "meta";

/** The key bar: what a dialog, an IDE or a sign-in needs, the Windows key last. Letters and digits are typed as text, or sent as keys with a modifier held. */
export const KEYS = ["Escape", "Tab", "Enter", "Backspace", "Delete", "ArrowLeft", "ArrowUp", "ArrowDown", "ArrowRight", "Home", "End", "PageUp", "PageDown", "F5", "ContextMenu", "Meta"] as const;
/** Ctrl, Shift, Alt and the Windows key (`meta`): tapped, a modifier is held for the next key. The Windows key alone is `Meta` (Start). */
export const MODS: Mod[] = ["ctrl", "shift", "alt", "meta"];

/** Merges what waits to be sent: of moves in a row only the last, wheels in a row added up. Presses, releases, clicks and keys stay in order. */
export function coalesce(ops: ScOp[]): ScOp[] {
  const out: ScOp[] = [];
  for (const op of ops) {
    const prev = out[out.length - 1];
    if (op.k === "move" && prev?.k === "move") out[out.length - 1] = op;
    else if (op.k === "wheel" && prev?.k === "wheel") out[out.length - 1] = { ...prev, dx: prev.dx + op.dx, dy: prev.dy + op.dy };
    else out.push(op);
  }
  return out;
}

/** After the phone moved its own cursor, the computer's cursor messages (a round trip old) leave its position alone for this long. */
export const CURSOR_ECHO_MS = 400;
/** Past the hold, a computer position this close (source pixels) to the drawn one is taken as the same, so the cursor does not jitter. */
export const CURSOR_SNAP_PX = 3;

export interface CursorPos {
  shape: number;
  visible: boolean;
  x: number;
  y: number;
}

/** Where to draw the cursor when the computer says where it is: the shape and visibility always follow it; the position does too, unless the
 *  phone moved the cursor itself within `CURSOR_ECHO_MS` (its own echo is newer than this message), or it is within `CURSOR_SNAP_PX`. */
export function reconcileCursor(drawn: CursorPos, host: CursorPos, now: number, echoedAt: number): CursorPos {
  const echoing = now - echoedAt < CURSOR_ECHO_MS;
  const close = Math.abs(host.x - drawn.x) <= CURSOR_SNAP_PX && Math.abs(host.y - drawn.y) <= CURSOR_SNAP_PX;
  return echoing || close ? { ...host, x: drawn.x, y: drawn.y } : host;
}

/** A computer cursor message, as the page takes it: the cursor drawn (`reconcileCursor`), and, with no finger down, the point a trackpad tap
 *  clicks, which is always where the cursor is drawn (never the message's own position, which a local echo may be ahead of). */
export function followHostCursor(drawn: CursorPos, tapAt: { x: number; y: number }, host: CursorPos, now: number, echoedAt: number, idle: boolean) {
  const next = reconcileCursor(drawn, host, now, echoedAt);
  return { drawn: next, tapAt: idle ? { x: next.x, y: next.y } : tapAt };
}
/** A decoder this many chunks behind its own usual queue is behind. */
export const BACKLOG_OVER = 2;
/** Its usual queue: the least seen over this long (some Android decoders always keep two or three). */
export const BACKLOG_WINDOW_MS = 2000;
/** Behind for this many deltas in a row: skipped to the next keyframe. */
export const BACKLOG_RUN = 3;

/** Whether the decoder is far enough behind to skip deltas up to the next keyframe (a newer picture is better than a late one). The threshold
 *  follows the decoder: more than `BACKLOG_OVER` over the least queue it kept lately, and for `BACKLOG_RUN` deltas in a row, so a decoder that
 *  always holds a few chunks is not made to ask for a keyframe every second. */
export class Backlog {
  private seen: { at: number; size: number }[] = [];
  private run = 0;

  /** A delta arrives at `at` with `size` chunks waiting in the decoder: true to skip it. */
  skip(size: number, at: number): boolean {
    this.seen.push({ at, size });
    while (this.seen.length && at - this.seen[0].at > BACKLOG_WINDOW_MS) this.seen.shift();
    const usual = Math.min(...this.seen.map((s) => s.size));
    this.run = size > Math.max(BACKLOG_OVER, usual + BACKLOG_OVER) ? this.run + 1 : 0;
    return this.run >= BACKLOG_RUN;
  }

  /** A keyframe: the run starts again. */
  reset() {
    this.run = 0;
  }
}

/** Intervals longer than these count as gaps (the second is the threshold that decides on another transport). */
export const GAP_MS = [100, 250] as const;
/** How many of the latest frames the timings keep. */
export const TIMES_KEPT = 2000;

/** The `q` quantile of `v` (sorted ascending), null for none. */
function quantile(v: number[], q: number): number | null {
  if (!v.length) return null;
  return v[Math.min(v.length - 1, Math.round((v.length - 1) * q))];
}

function spread(v: number[]) {
  const s = [...v].sort((a, b) => a - b);
  const r = (x: number | null) => (x === null ? null : Math.round(x * 10) / 10);
  return { n: s.length, p50: r(quantile(s, 0.5)), p95: r(quantile(s, 0.95)), p99: r(quantile(s, 0.99)), max: r(s.length ? s[s.length - 1] : null) };
}

/** Where each video frame's time went on the phone, for the measurement plan: arrival, the `decode()` call, the decoder's output and the
 *  picture drawn, as `performance.now()` milliseconds; from them the inter-arrival and inter-present intervals (p50/p95/p99 and the gaps
 *  over `GAP_MS`), the receive → present time, and the frames that were never shown. */
export class FrameTimes {
  private arrived = new Map<number, { at: number; decodeAt: number | null }>();
  private arrivals: number[] = [];
  private presents: number[] = [];
  private decodeMs: number[] = [];
  private toPresentMs: number[] = [];
  private lastArrival: number | null = null;
  private lastPresent: number | null = null;
  dropped = 0;
  /** Deltas skipped because the decoder was behind (`Backlog`). */
  backlogSkips = 0;

  private keep(list: number[], v: number) {
    list.push(v);
    if (list.length > TIMES_KEPT) list.shift();
  }

  /** Frame `seq` came off the link at `at`. */
  arrival(seq: number, at: number) {
    if (this.lastArrival !== null) this.keep(this.arrivals, at - this.lastArrival);
    this.lastArrival = at;
    this.arrived.set(seq, { at, decodeAt: null });
    // Frames that never came out of the decoder are counted once they are far behind.
    for (const [s] of this.arrived) {
      if (s >= seq - 60) break;
      this.arrived.delete(s);
      this.dropped++;
    }
  }

  /** Frame `seq` was handed to the decoder at `at`. */
  decoding(seq: number, at: number) {
    const a = this.arrived.get(seq);
    if (a) a.decodeAt = at;
  }

  /** Frame `seq` was skipped (no decoder, or waiting for a keyframe). */
  skipped(seq: number) {
    if (this.arrived.delete(seq)) this.dropped++;
  }

  /** Frame `seq` came out of the decoder at `out` and was drawn at `drawn`. */
  presented(seq: number, out: number, drawn: number) {
    const a = this.arrived.get(seq);
    this.arrived.delete(seq);
    if (a?.decodeAt != null) this.keep(this.decodeMs, out - a.decodeAt);
    if (a) this.keep(this.toPresentMs, drawn - a.at);
    if (this.lastPresent !== null) this.keep(this.presents, drawn - this.lastPresent);
    this.lastPresent = drawn;
  }

  /** What was measured so far. */
  snapshot() {
    const gaps = (v: number[]) => GAP_MS.map((g) => v.filter((x) => x > g).length);
    return {
      arrivalInterval: spread(this.arrivals),
      presentInterval: spread(this.presents),
      decodeMs: spread(this.decodeMs),
      receiveToPresentMs: spread(this.toPresentMs),
      arrivalGaps: gaps(this.arrivals),
      presentGaps: gaps(this.presents),
      gapThresholdsMs: [...GAP_MS],
      dropped: this.dropped,
      backlogSkips: this.backlogSkips,
    };
  }
}

export interface Touch {
  id: number;
  x: number;
  y: number;
}

/** Where the picture is and how it is shown, as a gesture needs it. */
export interface Frame {
  el: Box;
  srcW: number;
  srcH: number;
  view: View;
}

export type GestureOut = { ops: ScOp[]; view?: View };

const NONE: GestureOut = { ops: [] };

/**
 * Fingers on the picture.
 * - **Trackpad** (the default): one finger moves the computer's cursor by how far it moves (finer when zoomed in); a tap clicks where the
 *   cursor is, a long press without moving right-clicks there; a long press then a move drags.
 * - **Direct**: a tap clicks under the finger, a long press without moving right-clicks there; a long press then a move drags; a quick
 *   one-finger drag scrolls.
 * - Both: two fingers moving together scroll; two fingers pinching zoom the phone's view (nothing is sent) and pan it; two fingers tapped
 *   together right-click.
 */
export class Gestures {
  mode: "trackpad" | "direct" = "trackpad";
  /** Where the computer's cursor is, in source pixels (the computer says it; trackpad mode moves it). */
  cursor = { x: 0, y: 0 };

  /** No finger is down: the computer's own cursor (moved at the desk, or by a program) is where a tap clicks. */
  idle(): boolean {
    return this.touches.size === 0;
  }
  private touches = new Map<number, { x: number; y: number; x0: number; y0: number; t0: number }>();
  /** The finger a one-finger gesture follows: another one that lands during it is ignored. */
  private first: number | null = null;
  private state: "idle" | "press" | "point" | "drag" | "scroll1" | "two" | "two_up" | "scroll2" | "pinch" = "idle";
  private pinch0 = { d: 0, view: null as View | null, mid: { x: 0, y: 0 } };
  private carry = { x: 0, y: 0 };

  private src(f: Frame, x: number, y: number) {
    return toSource(x, y, f.el, f.srcW, f.srcH, f.view);
  }

  private pair() {
    const [a, b] = [...this.touches.values()];
    return a && b ? { d: Math.hypot(a.x - b.x, a.y - b.y), mid: { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 } } : null;
  }

  down(t: Touch, time: number, f: Frame): GestureOut {
    this.touches.set(t.id, { x: t.x, y: t.y, x0: t.x, y0: t.y, t0: time });
    if (this.touches.size === 1) {
      this.state = "press";
      this.first = t.id;
    }
    else if (this.touches.size === 2 && (this.state === "press" || this.state === "idle")) {
      this.state = "two";
      const p = this.pair()!;
      this.pinch0 = { d: p.d, view: f.view, mid: p.mid };
    }
    return NONE;
  }

  move(t: Touch, time: number, f: Frame): GestureOut {
    const was = this.touches.get(t.id);
    if (!was) return NONE;
    if ((this.state === "point" || this.state === "drag" || this.state === "scroll1") && t.id !== this.first) return NONE;
    let dx = t.x - was.x;
    let dy = t.y - was.y;
    was.x = t.x;
    was.y = t.y;
    const s = scaleOf(f.el, f.srcW, f.srcH, f.view) || 1;
    if (this.state === "press") {
      if (Math.hypot(t.x - was.x0, t.y - was.y0) <= TAP_SLOP) return NONE;
      if (time - was.t0 >= LONG_PRESS_MS) {
        // Held, then moved: a drag from where it was held.
        this.state = "drag";
        const at = this.mode === "trackpad" ? { ...this.cursor } : this.src(f, was.x0, was.y0);
        if (!at) return NONE;
        return { ops: [{ k: "down", x: at.x, y: at.y, b: "left" }, ...this.follow(f, t, t.x - was.x0, t.y - was.y0, s).ops] };
      }
      this.state = this.mode === "trackpad" ? "point" : "scroll1";
      // The move that went past the slop counts from where the finger started.
      dx = t.x - was.x0;
      dy = t.y - was.y0;
    }
    switch (this.state) {
      case "point":
      case "drag":
        return this.follow(f, t, dx, dy, s);
      case "scroll1":
        return this.wheel(f, t.x, t.y, -dx / s, -dy / s);
      case "two":
      case "scroll2":
      case "pinch": {
        const p = this.pair();
        if (!p) return NONE;
        if (this.state === "two") {
          if (Math.abs(p.d - this.pinch0.d) > 24) this.state = "pinch";
          else if (Math.hypot(p.mid.x - this.pinch0.mid.x, p.mid.y - this.pinch0.mid.y) > TAP_SLOP) this.state = "scroll2";
          else return NONE;
        }
        if (this.state === "pinch") {
          if (!this.pinch0.view || this.pinch0.d <= 0) return NONE;
          const zoomed = zoomAt(f.el, f.srcW, f.srcH, this.pinch0.view, p.d / this.pinch0.d, this.pinch0.mid.x, this.pinch0.mid.y);
          const zs = scaleOf(f.el, f.srcW, f.srcH, zoomed) || 1;
          return { ops: [], view: clampView(f.el, f.srcW, f.srcH, { ...zoomed, cx: zoomed.cx - (p.mid.x - this.pinch0.mid.x) / zs, cy: zoomed.cy - (p.mid.y - this.pinch0.mid.y) / zs }) };
        }
        // Both fingers move the page with them: each finger's move is half the pair's.
        return this.wheel(f, p.mid.x, p.mid.y, -dx / s / 2, -dy / s / 2);
      }
      default:
        return NONE;
    }
  }

  /** The cursor follows the finger: under it in direct mode, by the finger's move in trackpad mode. */
  private follow(f: Frame, t: Touch, dx: number, dy: number, s: number): GestureOut {
    if (this.mode === "direct") {
      const p = this.src(f, t.x, t.y);
      if (!p) return NONE;
      this.cursor = p;
      return { ops: [{ k: "move", x: p.x, y: p.y }] };
    }
    const k = TRACKPAD_GAIN / s;
    const x = Math.round(Math.min(Math.max(this.cursor.x + dx * k, 0), f.srcW - 1));
    const y = Math.round(Math.min(Math.max(this.cursor.y + dy * k, 0), f.srcH - 1));
    this.cursor = { x, y };
    return { ops: [{ k: "move", x, y }] };
  }

  /** Scroll by a distance in source pixels: whole notches of 120 go, the rest waits for the next move. */
  private wheel(f: Frame, clientX: number, clientY: number, sx: number, sy: number): GestureOut {
    const at = this.mode === "trackpad" ? this.cursor : this.src(f, clientX, clientY);
    if (!at) return NONE;
    // About 40 source pixels a notch: what a mouse wheel scrolls in most programs.
    this.carry.x += (sx / 40) * 120;
    this.carry.y += (sy / 40) * 120;
    const dx = Math.trunc(this.carry.x / 120) * 120;
    const dy = Math.trunc(this.carry.y / 120) * 120;
    this.carry.x -= dx;
    this.carry.y -= dy;
    if (!dx && !dy) return NONE;
    return { ops: [{ k: "wheel", x: at.x, y: at.y, dx, dy }] };
  }

  up(t: Touch, time: number, f: Frame): GestureOut {
    const was = this.touches.get(t.id);
    this.touches.delete(t.id);
    if (!was) return NONE;
    const state = this.state;
    if (!this.touches.size) {
      this.state = "idle";
      this.carry = { x: 0, y: 0 };
    }
    const oneFinger = state === "point" || state === "drag" || state === "scroll1";
    // A finger that landed during a one-finger gesture lifts: the gesture goes on.
    if (oneFinger && t.id !== this.first) return NONE;
    // The first finger ends it, whatever other finger is still down.
    if (oneFinger) {
      this.state = "idle";
      this.first = null;
    }
    if (state === "drag") {
      const at = this.mode === "trackpad" ? this.cursor : this.src(f, t.x, t.y);
      return at ? { ops: [{ k: "up", x: at.x, y: at.y, b: "left" }] } : NONE;
    }
    if (state === "press") {
      const at = this.mode === "trackpad" ? { ...this.cursor } : this.src(f, was.x0, was.y0);
      if (!at) return NONE;
      if (this.mode === "direct") this.cursor = at;
      return { ops: [{ k: "click", x: at.x, y: at.y, b: time - was.t0 >= LONG_PRESS_MS ? "right" : "left" }] };
    }
    if (state === "two") {
      // One of two fingers lifted without moving: a right click once the other one is lifted too.
      this.state = this.touches.size ? "two_up" : "idle";
      if (this.touches.size) return NONE;
    }
    if (state === "two" || state === "two_up") {
      if (this.touches.size) return NONE;
      const at = this.mode === "trackpad" ? this.cursor : this.src(f, this.pinch0.mid.x, this.pinch0.mid.y);
      return at ? { ops: [{ k: "click", x: at.x, y: at.y, b: "right" }] } : NONE;
    }
    return NONE;
  }

  /** The system took the touches (a notification, an edge swipe): a drag lets go where it is, nothing else is sent. */
  cancel(): GestureOut {
    const dragging = this.state === "drag";
    this.touches.clear();
    this.state = "idle";
    return dragging ? { ops: [{ k: "up", x: this.cursor.x, y: this.cursor.y, b: "left" }] } : NONE;
  }
}
// ---------- one screen, from the phone's side ----------

export interface TargetRow {
  id: string;
  kind: "monitor" | "window";
  title: string;
  app: string;
  shortcut: "ide" | "kelpie" | null;
  w: number;
  h: number;
  minimized: boolean;
  primary: boolean;
}

export type Reply =
  | { t: "ack"; result?: string; sc?: string; title?: string; kind?: string; w?: number; h?: number; encoder?: string; targets?: TargetRow[] }
  | { t: "error"; code: string; message: string }
  | { t: "offline" }
  | { t: "unknown" };

export interface ScreenPort {
  targets(): Promise<Reply>;
  start(target: string): Promise<Reply>;
  stop(sc: string): Promise<Reply>;
  text(sc: string, text: string, st: number): Promise<Reply>;
  rebind(sc: string): Promise<Reply>;
  resume(sc: string): void;
  input(sc: string, ops: ScOp[], st: number): void;
  ack(sc: string, seq: number): void;
  keyframe(sc: string): void;
  serverNow(): number;
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(t: unknown): void;
}

export interface Stats {
  fps?: number;
  w?: number;
  h?: number;
  kbps?: number;
  encode_ms_p95?: number;
  dropped?: number;
  reason?: string;
  degraded?: boolean;
  level?: number;
  encoder?: string;
  /** Crisp text: on, off for now because the link is slow (false), or switched off on the computer (null). */
  crisp?: boolean | null;
  /** The computer's stage timings over the last second, in ms (`kelpie_screen::control::Stages`): for the measurement plan, shown in the debug view. */
  stages?: Record<string, number | null>;
}

export interface ScreenState {
  /** `pick`: choosing what to see; `starting`; `live`; `frozen`: nothing for a while, input off; `ended`. */
  phase: "pick" | "starting" | "live" | "frozen" | "ended";
  targets: TargetRow[] | null;
  sc: string | null;
  title: string;
  kind: string;
  /** What the computer says about the picture: live, locked, secure_desktop, minimized, elevated, covered, blocked, paused_load. */
  remote: string;
  stats: Stats | null;
  encoder: string;
  message: string | null;
  text: "sending" | "sent" | "not_sent" | null;
  /** The computer says it sends frames, the heartbeat goes on, and none has been shown for a while: the picture is stuck. */
  stuck: boolean;
}

const fresh = (): ScreenState => ({ phase: "pick", targets: null, sc: null, title: "", kind: "", remote: "live", stats: null, encoder: "hw", message: null, text: null, stuck: false });

/** Why a screen ended, in the phone's words; the reason codes are the daemon's and the helper's. */
export function endedText(reason: string, t: (s: string) => string): string {
  switch (reason) {
    case "desktop":
      return t("Stopped on the computer.");
    case "hotkey":
      return t("Stopped with the hotkey on the computer.");
    case "closed":
      return t("The window was closed on the computer.");
    case "revoked":
      return t("Control was taken away, or this phone locked.");
    case "idle":
      return t("Stopped after 10 minutes with nothing done.");
    case "link_lost":
      return t("The connection was lost for too long.");
    case "replaced":
      return t("Another screen was opened instead.");
    case "display_changed":
      return t("The screen's size or orientation changed on the computer. See it again.");
    case "failed":
      return t("The screen could not be started.");
    case "device_lost":
      return t("The computer's graphics driver restarted, which ended the screen. See it again.");
    case "encoder_failed":
      return t("The computer could not encode its screen.");
    case "app_closed":
    case "app":
      return t("The Kelpie app on the computer closed.");
    default:
      return t("Stopped.");
  }
}

/** What the computer said about the picture, as a line over it (null: nothing to say). */
export function remoteText(remote: string, t: (s: string) => string): string | null {
  switch (remote) {
    case "locked":
      return t("The computer is locked. Nothing can be seen or done until someone signs in at it.");
    case "secure_desktop":
      return t("A Windows security prompt (UAC) or another secure screen is up. It cannot be seen or answered from the phone: answer it on the computer.");
    case "minimized":
      return t("The window is minimized. Tap to restore it.");
    case "elevated":
      return t("An app running as administrator is in front: Windows does not let Kelpie type into it.");
    case "covered":
      return t("Another window is over this one where you tapped: the tap was not sent.");
    case "covered_dialog":
      return t("A dialog of another program is in front of the shared window, and this phone cannot see it. Share the whole monitor to type into this dialog.");
    case "not_front":
      return t("The shared window is not in front on the computer, so keys and text were not sent. Tap it to bring it to the front.");
    case "blocked":
      return t("Windows did not take that input: an app running as administrator, or a secure screen.");
    case "paused_load":
      return t("Paused to protect your computer. Tap to resume.");
    default:
      return null;
  }
}

/** The quality line: `720p · 15 fps · reduced: computer busy`. */
export function qualityText(s: Stats | null, t: (s: string) => string): string {
  if (!s?.h) return "";
  const parts = [`${Math.round(s.h)}p`, `${Math.round(s.fps ?? 0)} fps`];
  const why: Record<string, string> = { cpu: t("reduced: encoding is slow"), bandwidth: t("reduced: slow connection"), load: t("reduced: computer busy"), encoder: t("reduced: software encoder") };
  if (s.degraded && s.reason && why[s.reason]) parts.push(why[s.reason]);
  if (s.crisp === false) parts.push(t("crisp text off: slow connection"));
  return parts.join(" · ");
}

/** Whether this browser can show the screen: WebCodecs, and an H.264 decoder that takes the low-latency setting. */
export async function canDecode(env: { VideoDecoder?: { isConfigSupported(c: unknown): Promise<{ supported?: boolean }> } }): Promise<boolean> {
  const D = env.VideoDecoder;
  if (!D?.isConfigSupported) return false;
  try {
    const r = await D.isConfigSupported({ codec: "avc1.4d0028", codedWidth: 1920, codedHeight: 1080, optimizeForLatency: true, hardwareAcceleration: "no-preference" });
    return !!r.supported;
  } catch {
    return false;
  }
}

/** One screen from the phone's side, from picking what to see to its end. `changed` is called with each new state. */
export class ScreenSession {
  state: ScreenState | null = null;
  private queue: ScOp[] = [];
  private flushTimer: unknown = null;
  private watchTimer: unknown = null;
  private heardAt = 0;
  private mediaAt = 0;
  private decodedAt = 0;
  /** The last frame that came before the start's answer (the picture can overtake it): acknowledged once the screen's id is known, or the
   *  computer would count it in flight, and hold the phone's input back, until a later frame came. */
  private early = 0;
  private earlyKey = false;
  private keyAskedAt = 0;

  constructor(private port: ScreenPort, private changed: (s: ScreenState | null) => void, private t: (s: string) => string = (s) => s) {}

  private set(next: ScreenState | null) {
    this.state = next;
    this.changed(next);
  }

  private patch(p: Partial<ScreenState>) {
    if (this.state) this.set({ ...this.state, ...p });
  }

  private phase() {
    return this.state?.phase ?? null;
  }

  async openPicker() {
    if (this.state?.sc && this.state.phase !== "ended") return;
    this.set(fresh());
    const r = await this.port.targets();
    if (this.phase() !== "pick") return;
    if (r.t === "ack") this.patch({ targets: r.targets ?? [] });
    else this.patch({ targets: [], message: r.t === "error" ? r.message : this.t("Not connected.") });
  }

  async pick(target: string) {
    if (this.state?.phase !== "pick") return;
    this.patch({ phase: "starting", message: null });
    this.early = 0;
    this.earlyKey = false;
    const r = await this.port.start(target);
    if (this.phase() !== "starting") return;
    if (r.t !== "ack" || !r.sc) {
      this.patch({ phase: "pick", message: r.t === "error" ? r.message : this.t("Not connected.") });
      return;
    }
    const now = this.port.now();
    this.heardAt = this.mediaAt = this.decodedAt = now;
    this.patch({ phase: "live", sc: r.sc, title: r.title ?? "", kind: r.kind ?? "", encoder: r.encoder ?? "hw" });
    if (this.early) this.port.ack(r.sc, this.early);
    this.early = 0;
    if (this.earlyKey) {
      this.earlyKey = false;
      this.keyAskedAt = now;
      this.port.keyframe(r.sc);
    }
    this.watch();
  }

  /** `sc_state`, `sc_stats`, `sc_fhb`, `sc_end`. */
  onMessage(m: { type: string; sc?: string; state?: string; reason?: string; encoder?: string; stages?: Record<string, number | null> } & Stats) {
    const s = this.state;
    if (!s?.sc || m.sc !== s.sc || s.phase === "ended") return;
    if (m.type === "sc_end") return this.ended(endedText(m.reason ?? "", this.t));
    this.heardAt = this.port.now();
    if (m.type === "sc_state" && m.state === "encoder") this.patch({ encoder: m.encoder === "sw" ? "sw" : "hw" });
    else if (m.type === "sc_state" && m.state) this.patch({ remote: m.state });
    else if (m.type === "sc_stats") this.patch({ stats: { fps: m.fps, w: m.w, h: m.h, kbps: m.kbps, encode_ms_p95: m.encode_ms_p95, dropped: m.dropped, reason: m.reason, degraded: m.degraded, level: m.level, encoder: m.encoder, crisp: m.crisp, stages: m.stages } });
    if (this.state?.phase === "frozen") this.patch({ phase: "live" });
  }

  /** A media message arrived (the view decodes it). Only video says frames are coming: the cursor moves on its own. */
  media(video = true) {
    const now = this.port.now();
    this.heardAt = now;
    if (video) this.mediaAt = now;
    if (this.state?.phase === "frozen") this.patch({ phase: "live" });
  }

  /** Video frame `seq` came out of the decoder: the computer may send more. */
  decoded(seq: number) {
    this.decodedAt = this.port.now();
    if (this.state?.stuck) this.patch({ stuck: false });
    this.acknowledge(seq);
  }

  private acknowledge(seq: number) {
    if (this.state?.sc && this.state.phase !== "ended") this.port.ack(this.state.sc, seq);
    else if (this.state?.phase === "starting") this.early = Math.max(this.early, seq);
  }

  /** Video frame `seq` will not be shown (skipped while a keyframe is awaited, or the decoder failed on it): it is acknowledged all the same, so
   *  the computer never waits for it, and a keyframe is asked for. */
  skipped(seq: number) {
    this.acknowledge(seq);
    this.needKeyframe();
  }

  /** The decoder failed or was reset: a keyframe, at most once a second. */
  needKeyframe() {
    const now = this.port.now();
    // Asked for before the start's answer: sent with the early acknowledgement once the screen is known.
    if (!this.state?.sc && this.state?.phase === "starting") this.earlyKey = true;
    if (!this.state?.sc || now - this.keyAskedAt < 1000) return;
    this.keyAskedAt = now;
    this.port.keyframe(this.state.sc);
  }

  live() {
    return this.state?.phase === "live";
  }

  /** Input, while the picture is live: anything else is dropped here, never kept for later. */
  input(ops: ScOp[]) {
    if (!this.live() || !ops.length) return;
    if (this.state?.remote === "paused_load" && this.state.sc) {
      this.port.resume(this.state.sc);
      this.patch({ remote: "live" });
      return;
    }
    this.queue.push(...ops);
    if (ops.some((o) => o.k !== "move" && o.k !== "wheel")) this.flush();
    else if (this.flushTimer === null) this.flushTimer = this.port.setTimeout(() => this.flush(), FLUSH_MS);
  }

  private flush() {
    if (this.flushTimer !== null) this.port.clearTimeout(this.flushTimer);
    this.flushTimer = null;
    const ops = coalesce(this.queue.splice(0));
    if (ops.length && this.state?.sc && this.live()) this.port.input(this.state.sc, ops, this.port.serverNow());
  }

  /** Text into the focused field: said as sent or not sent, never sent twice. */
  async type(text: string): Promise<boolean> {
    if (!this.live() || !this.state?.sc || !text) return false;
    this.flush();
    this.patch({ text: "sending" });
    const r = await this.port.text(this.state.sc, text, this.port.serverNow());
    const ok = r.t === "ack" && r.result === "sent";
    this.patch({ text: ok ? "sent" : "not_sent" });
    return ok;
  }

  /** This phone locked (it left the app, or its idle time ran out): the computer has ended the screen, and so does the page, at once. */
  locked() {
    if (this.state?.sc && this.state.phase !== "ended" && this.state.phase !== "pick") this.ended(this.t("Ended when this phone locked."));
  }

  async stop() {
    const sc = this.state?.sc;
    if (sc && this.state?.phase !== "ended") {
      this.ended(this.t("Stopped."));
      await this.port.stop(sc);
    }
  }

  /** The connection is back after a handshake: the screen is asked for again, and is gone if the computer let it go. */
  async reconnected() {
    const sc = this.state?.sc;
    if (!sc || this.state?.phase === "ended" || this.state?.phase === "pick") return;
    const r = await this.port.rebind(sc);
    if (this.state?.sc !== sc) return;
    if (r.t === "ack") {
      this.heardAt = this.mediaAt = this.decodedAt = this.port.now();
      this.patch({ phase: "live" });
    } else if (r.t === "error") this.ended(endedText("link_lost", this.t));
  }

  close() {
    void this.stop();
    this.unwatch();
    this.set(null);
  }

  private ended(message: string) {
    this.queue = [];
    this.unwatch();
    this.patch({ phase: "ended", message });
  }

  /** Dimmed after `FREEZE_MS` without a word; a keyframe asked for when frames come and none decodes. */
  private watch() {
    this.unwatch();
    const look = () => {
      this.watchTimer = this.port.setTimeout(look, 250);
      const now = this.port.now();
      if (this.state?.phase === "live" && now - this.heardAt > FREEZE_MS) {
        this.queue = [];
        this.patch({ phase: "frozen" });
      }
      if (this.state?.phase === "live" && this.mediaAt - this.decodedAt > STUCK_MS) this.needKeyframe();
      // Heartbeats come and the computer says it sends frames, yet nothing has been shown: say so, and ask for a fresh picture.
      const sending = (this.state?.stats?.fps ?? 0) > 0;
      const stuck = this.state?.phase === "live" && sending && now - this.decodedAt > STUCK_SHOW_MS;
      if (stuck) this.needKeyframe();
      if (this.state && this.state.phase !== "ended" && stuck !== this.state.stuck) this.patch({ stuck });
    };
    this.watchTimer = this.port.setTimeout(look, 250);
  }

  private unwatch() {
    if (this.watchTimer !== null) this.port.clearTimeout(this.watchTimer);
    this.watchTimer = null;
    if (this.flushTimer !== null) this.port.clearTimeout(this.flushTimer);
    this.flushTimer = null;
  }
}
