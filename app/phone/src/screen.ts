// Seeing the computer's screen from the phone (the daemon's `screen` broker; the app's helper captures and encodes). What is here is plain
// logic, tested without a browser: the media messages the computer sends (crates/kelpie-screen/src/wire.rs), H.264 made ready for WebCodecs
// (`avc` format, an avcC description from the keyframe's SPS and PPS, no frame reordering), the crisp tiles painted over the video and dropped
// when their region changes, where a finger lands on the picture under zoom, the two touch modes, input merged and stamped with the computer's
// clock, frames acknowledged once decoded, and a stream counted lost after a few silent seconds. screen-view.tsx draws it.

import { chooseVia, needsProof, pathRows, peerFor, startRefusal, viaText, type PathInfo, type RelayInfo, type ViaId } from "./screen-paths";
import { P2P_RETRY_MS, STUN_ONLY, VideoOrder, type PathStats, type PeerEvents, type PeerSetup } from "./screen-rtc";
import { BUSY_RETRY_MS, busyText, type BusyInfo } from "./screen-busy";

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
/** Trackpad: a finger that, once down this long (ms), moves this far (screen pixels) from where it was then, short of the slop, is a slow
 *  nudge of the cursor, not a tap: a quick tap stays a click however the finger rolls as it lands or lifts, a long press keeps the drift a
 *  finger has as it flattens, and a small, deliberate correction moves the cursor. */
export const NUDGE_MS = 120;
export const NUDGE_PX = 4;
/** Trackpad mode: the cursor moves this many times as far on the picture as the finger moves on the glass, at an ordinary speed. */
export const TRACKPAD_GAIN = 1.2;
/** Pointer acceleration: the gain's factor for the finger's speed (screen pixels a millisecond), straight between these points and flat past
 *  the ends. A slow finger points finely at a small target; a quick flick crosses the screen. */
export const TRACKPAD_CURVE: readonly (readonly [number, number])[] = [[0.05, 0.5], [0.3, 1], [1.5, 2]];
/** Two taps this close in time (ms), and in direct mode in place (screen pixels), are a double click, at the first one's point. */
export const DOUBLE_TAP_MS = 350;
export const DOUBLE_TAP_PX = 32;
export const MAX_ZOOM = 6;
/** How long the computer keeps a screen whose connection dropped, for this phone's next connection: counted down while the phone reconnects. */
export const REBIND_GRACE_MS = 45_000;
/** While the picture is frozen, the connection is checked (and made again when it is gone) at most this often. */
export const KICK_EVERY_MS = 2000;
/** One Sharpen at most this often (the computer holds to it too). */
export const SHARPEN_GAP_MS = 3000;
/** Pictures' ages kept between two of the computer's stats. */
const AGES_KEPT = 120;

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

/** Zoomed in, the view pans just enough to keep source point `p` this far (screen pixels) inside its edges: the cursor never leaves the
 *  picture shown (a trackpad move, a pinch, the keyboard taking half the screen). At zoom 1 the whole picture is shown already. */
export const KEEP_IN_VIEW_PX = 40;

export function keepInView(el: Box, srcW: number, srcH: number, v: View, p: { x: number; y: number }, marginPx = KEEP_IN_VIEW_PX): View {
  const s = scaleOf(el, srcW, srcH, v);
  if (!s || v.zoom <= 1) return v;
  // At most half the visible size, so a small box still has a place to put the point.
  const mx = Math.min(marginPx / s, el.width / s / 2);
  const my = Math.min(marginPx / s, el.height / s / 2);
  const halfW = el.width / s / 2;
  const halfH = el.height / s / 2;
  const cx = Math.min(Math.max(v.cx, p.x + mx - halfW), p.x - mx + halfW);
  const cy = Math.min(Math.max(v.cy, p.y + my - halfH), p.y - my + halfH);
  if (cx === v.cx && cy === v.cy) return v;
  return clampView(el, srcW, srcH, { ...v, cx, cy });
}

/** Zoomed in, the inner part of the picture box the trackpad's cursor moves in freely (this far from each edge, screen pixels): a fifth of
 *  the box's smaller side, between `KEEP_IN_VIEW_PX` and 120. Past it the view follows the cursor. */
export function followMargin(el: Box): number {
  return Math.min(Math.max(Math.round(Math.min(el.width, el.height) / 5), KEEP_IN_VIEW_PX), 120);
}

/** Zoomed in, the view after the cursor moved from source point `from` to `to`: it pans only by how far the cursor went on toward an edge
 *  past the inner area (`marginPx` inside the box), so the cursor glides to that line and the picture then moves under it, never jumping.
 *  A move back inward, or anywhere inside the inner area, leaves the view alone. At the picture's own edge the view stops and the cursor
 *  goes on to it. */
export function followView(el: Box, srcW: number, srcH: number, v: View, from: { x: number; y: number }, to: { x: number; y: number }, marginPx: number): View {
  const s = scaleOf(el, srcW, srcH, v);
  if (!s || v.zoom <= 1) return v;
  const a = toClient(from.x, from.y, el, srcW, srcH, v);
  const b = toClient(to.x, to.y, el, srcW, srcH, v);
  const mx = Math.min(marginPx, el.width / 2);
  const my = Math.min(marginPx, el.height / 2);
  // How far (screen pixels) the picture moves on one axis: the part of the move past the inner line, outward only.
  const shift = (a: number, b: number, lo: number, hi: number) => (b < lo && b < a ? b - Math.min(a, lo) : b > hi && b > a ? b - Math.max(a, hi) : 0);
  const dx = shift(a.x, b.x, el.left + mx, el.left + el.width - mx);
  const dy = shift(a.y, b.y, el.top + my, el.top + el.height - my);
  if (!dx && !dy) return v;
  const next = clampView(el, srcW, srcH, { ...v, cx: v.cx + dx / s, cy: v.cy + dy / s });
  return next.cx === v.cx && next.cy === v.cy ? v : next;
}

/** Zoomed in, the cursor brought into the picture shown when it is out of it (the view was moved away from it, or the computer moved it
 *  there): to the nearest point of the inner area, so a trackpad move starts where the person is looking instead of the view jumping
 *  to the cursor. Inside the picture shown (or at zoom 1), the cursor as it is. */
export function cursorIntoView(el: Box, srcW: number, srcH: number, v: View, p: { x: number; y: number }, marginPx: number): { x: number; y: number } {
  const s = scaleOf(el, srcW, srcH, v);
  if (!s || v.zoom <= 1) return p;
  const c = toClient(p.x, p.y, el, srcW, srcH, v);
  if (c.x >= el.left && c.x <= el.left + el.width && c.y >= el.top && c.y <= el.top + el.height) return p;
  const mx = Math.min(marginPx, el.width / 2);
  const my = Math.min(marginPx, el.height / 2);
  const x = Math.min(Math.max(c.x, el.left + mx), el.left + el.width - mx);
  const y = Math.min(Math.max(c.y, el.top + my), el.top + el.height - my);
  return toSource(x, y, el, srcW, srcH, v) ?? p;
}

/** Direct touch, zoomed in: a finger this close (screen pixels) to an edge of the picture box pans the view that way while it moves the
 *  cursor or drags, after `EDGE_PAN_DELAY_MS` there (a finger only passing by, or reaching for a target near the edge, pans nothing), at up
 *  to `EDGE_PAN_SPEED` screen pixels a millisecond at the very edge. */
export const EDGE_PAN_PX = 36;
export const EDGE_PAN_DELAY_MS = 250;
export const EDGE_PAN_SPEED = 0.6;

/** The part of the picture the phone shows now, in source pixels (whole pixels, inside the picture): what Sharpen sends crisp. */
export function visibleRect(el: Box, srcW: number, srcH: number, v: View): Rect | null {
  const a = toSourceRaw(el.left, el.top, el, srcW, srcH, v);
  const b = toSourceRaw(el.left + el.width, el.top + el.height, el, srcW, srcH, v);
  if (!a || !b) return null;
  const x = Math.max(0, Math.floor(a.x));
  const y = Math.max(0, Math.floor(a.y));
  const w = Math.min(srcW, Math.ceil(b.x)) - x;
  const h = Math.min(srcH, Math.ceil(b.y)) - y;
  return w > 0 && h > 0 ? { x, y, w, h } : null;
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
export const KEYS = ["Escape", "Tab", "Enter", "Backspace", "Delete", "ArrowLeft", "ArrowUp", "ArrowDown", "ArrowRight", "Home", "End", "PageUp", "PageDown", "ContextMenu", "Meta", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"] as const;
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

/** After the phone moved its own cursor, the computer's cursor messages (a round trip old) leave its position alone for this long: while the
 *  finger moves, every message is older than the place it put the cursor. */
export const CURSOR_ECHO_MS = 400;
/** After that, for this long from the finger's last move, a message at a place the phone itself sent is taken as late (it would pull the
 *  cursor back along its path: behind the picture on the relay, the computer's messages come well after a round trip). A place the phone did
 *  not send (the mouse at the desk, a program, an edge that held the cursor) is followed at once; after this long, any place is. */
export const CURSOR_CATCHUP_MS = 2000;
/** A computer position this close (source pixels) to another is taken as the same, so the cursor does not jitter. */
export const CURSOR_SNAP_PX = 3;

export interface CursorPos {
  shape: number;
  visible: boolean;
  x: number;
  y: number;
}

/** Where the phone last put the cursor (source pixels) and when, and every place it sent within `CURSOR_CATCHUP_MS` of that (oldest first,
 *  the last one included): what the computer may still say late. */
export interface CursorEcho {
  x: number;
  y: number;
  at: number;
  trail: { x: number; y: number; at: number }[];
}

/** The echo once the phone put the cursor at `p` at `now`: the places sent before it kept for as long as they may still come back. */
export function echoAt(prev: CursorEcho | null, p: { x: number; y: number }, now: number): CursorEcho {
  const trail = (prev?.trail ?? []).filter((t) => now - t.at < CURSOR_CATCHUP_MS);
  trail.push({ x: p.x, y: p.y, at: now });
  return { x: p.x, y: p.y, at: now, trail };
}

/** Where ops sent put the computer's cursor: the last one with a place (a move, or a click, press, release or wheel, which the computer
 *  moves the cursor to first), or null for keys alone. */
export function echoPoint(ops: readonly ScOp[]): { x: number; y: number } | null {
  const at = ops.findLast((op): op is Exclude<ScOp, { k: "key" }> => op.k !== "key");
  return at ? { x: at.x, y: at.y } : null;
}

/** Where to draw the cursor when the computer says where it is, and the echo still kept (null once its time is over). The shape and
 *  visibility always follow the computer; the position does too, unless the phone put the cursor somewhere within `CURSOR_ECHO_MS`, or the
 *  computer names a place the phone sent within `CURSOR_CATCHUP_MS` (a late message), or it is within `CURSOR_SNAP_PX` of the drawn one. */
export function reconcileCursor(drawn: CursorPos, host: CursorPos, now: number, echo: CursorEcho | null): { drawn: CursorPos; echo: CursorEcho | null } {
  const near = (p: { x: number; y: number }) => Math.abs(host.x - p.x) <= CURSOR_SNAP_PX && Math.abs(host.y - p.y) <= CURSOR_SNAP_PX;
  const kept = { ...host, x: drawn.x, y: drawn.y };
  const since = echo ? now - echo.at : Infinity;
  if (since < CURSOR_ECHO_MS || (since < CURSOR_CATCHUP_MS && echo!.trail.some(near))) return { drawn: kept, echo };
  return { drawn: near(drawn) ? kept : host, echo: since < CURSOR_CATCHUP_MS ? echo : null };
}

/** A computer cursor message, as the page takes it: the cursor drawn and the echo still kept (`reconcileCursor`), and, with no finger down,
 *  the point a trackpad tap clicks, which is always where the cursor is drawn (never the message's own position, which a local echo may be
 *  ahead of). */
export function followHostCursor(drawn: CursorPos, tapAt: { x: number; y: number }, host: CursorPos, now: number, echo: CursorEcho | null, idle: boolean) {
  const next = reconcileCursor(drawn, host, now, echo);
  return { drawn: next.drawn, echo: next.echo, tapAt: idle ? { x: next.drawn.x, y: next.drawn.y } : tapAt };
}
/** A decoder given a frame this long ago (ms) that has put out no picture since is wedged: it neither decodes nor fails (an iPhone's
 *  hardware decoder after the phone slept), so its own error never comes. Checked on a timer, not on the next frame: a decoder that never
 *  answers leaves frames unacknowledged, and the computer then stops sending. */
export const DECODER_WEDGED_MS = 1500;

/** Whether a decoder is wedged: the first frame given to it since its last picture at `fedAt` (null: none), its last picture out at
 *  `outputAt` (null: none yet), and now `now`. */
export function decoderWedged(fedAt: number | null, outputAt: number | null, now: number): boolean {
  return fedAt !== null && (outputAt === null || outputAt < fedAt) && now - fedAt >= DECODER_WEDGED_MS;
}

/** When a frame given to a decoder now starts the wait for its next picture: the first frame since the last picture (an earlier one
 *  still unanswered keeps its time). */
export function fedSince(fedAt: number | null, outputAt: number | null, now: number): number {
  return fedAt === null || (outputAt !== null && outputAt >= fedAt) ? now : fedAt;
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

/** What a gesture did, for the page to show and buzz: a click, a double click, a right click, a drag picked up. */
export type Feel = "click" | "double" | "right" | "drag";
export type GestureOut = { ops: ScOp[]; view?: View; feel?: Feel };

/** The gain's factor for a finger moving at `speed` (screen pixels a millisecond), along `TRACKPAD_CURVE`. */
export function accel(speed: number): number {
  const c = TRACKPAD_CURVE;
  if (!(speed > c[0][0])) return c[0][1];
  for (let i = 1; i < c.length; i++) {
    const [x1, y1] = c[i];
    if (speed <= x1) {
      const [x0, y0] = c[i - 1];
      return y0 + ((y1 - y0) * (speed - x0)) / (x1 - x0);
    }
  }
  return c[c.length - 1][1];
}

const NONE: GestureOut = { ops: [] };

/**
 * Fingers on the picture.
 * - **Trackpad** (the default): one finger moves the computer's cursor by how far it moves, more for a quick move and less for a slow one
 *   (`accel`; finer when zoomed in). Zoomed in, the cursor moves freely in the inner part of the picture shown and the view follows it only
 *   past that (`followView`); a cursor out of the picture shown is first brought into it (`cursorIntoView`), so the view never jumps. A tap
 *   clicks where the cursor is, a long press without moving right-clicks there; a long press then a move drags. A pinch zooms around the
 *   cursor.
 * - **Direct**: a tap clicks under the finger, a long press without moving right-clicks there; a long press then a move drags; a quick
 *   one-finger drag scrolls, or zoomed in moves the cursor under the finger, the view panning while the finger is held near an edge
 *   (`tick`). A pinch zooms around the fingers.
 * - Both: a second tap soon after a tap, near it, clicks again at the first tap's point (a double click, never two places); two fingers
 *   moving together scroll, or zoomed in move the phone's view (nothing is sent); two fingers pinching zoom the phone's view (nothing is
 *   sent) and pan it; two fingers tapped together right-click.
 */
export class Gestures {
  mode: "trackpad" | "direct" = "trackpad";
  /** Where the computer's cursor is, in source pixels (the computer says it; trackpad mode moves it). */
  cursor = { x: 0, y: 0 };

  /** No finger is down: the computer's own cursor (moved at the desk, or by a program) is where a tap clicks. */
  idle(): boolean {
    return this.touches.size === 0;
  }
  /** Each finger: where it is, where it landed and when, and where it was once `NUDGE_MS` had passed (a nudge is measured from there). */
  private touches = new Map<number, { x: number; y: number; x0: number; y0: number; t0: number; xn?: number; yn?: number }>();
  /** The finger a one-finger gesture follows: another one that lands during it is ignored. */
  private first: number | null = null;
  private state: "idle" | "press" | "point" | "drag" | "scroll1" | "two" | "two_up" | "scroll2" | "pinch" = "idle";
  private pinch0 = { d: 0, view: null as View | null, mid: { x: 0, y: 0 }, anchor: { x: 0, y: 0 } };
  /** Direct touch, zoomed in: since when the finger is near an edge, and the last edge pan's time (null: not near one). */
  private edge: { since: number; last: number } | null = null;
  private carry = { x: 0, y: 0 };
  /** What trackpad moves leave over of a source pixel, so that a slow finger still moves the cursor. */
  private rest = { x: 0, y: 0 };
  /** The finger's last move (for its speed). */
  private lastMove: { time: number; speed: number } | null = null;
  /** The last tap: when, where on the glass, and the source point it clicked (a second one near it clicks there again). */
  private lastTap: { time: number; x: number; y: number; at: { x: number; y: number }; b: Button; mode: "trackpad" | "direct" } | null = null;

  /** Trackpad: the finger, short of the slop, has moved `NUDGE_PX` since `NUDGE_MS` passed and before the long press: a slow nudge. */
  private nudging(was: { x: number; y: number; t0: number; xn?: number; yn?: number }, time: number): boolean {
    const held = time - was.t0;
    return this.mode === "trackpad" && was.xn !== undefined && was.yn !== undefined && held < LONG_PRESS_MS && Math.hypot(was.x - was.xn, was.y - was.yn) >= NUDGE_PX;
  }

  /** A long press is held now (one finger, not moved, `LONG_PRESS_MS` or more): where its right click or drag would be (source pixels),
   *  for the page to show; null otherwise. */
  held(time: number, f: Frame): { x: number; y: number } | null {
    if (this.state !== "press" || this.touches.size !== 1 || this.first === null) return null;
    const t = this.touches.get(this.first)!;
    if (time - t.t0 < LONG_PRESS_MS) return null;
    // A trackpad cursor out of the picture shown: the press only brings it back, nothing to show held.
    if (this.bringIn(f)) return null;
    return this.mode === "trackpad" ? { ...this.cursor } : this.src(f, t.x0, t.y0);
  }

  /** Trackpad, zoomed in, the cursor out of the picture shown (the view was moved away from it, or the computer moved it): where it comes
   *  back into view (`cursorIntoView`); null when it is in view, or in direct mode, or at zoom 1. Nothing is ever clicked, pressed or dragged
   *  at a cursor the person cannot see. */
  private bringIn(f: Frame): { x: number; y: number } | null {
    if (this.mode !== "trackpad") return null;
    const at = cursorIntoView(f.el, f.srcW, f.srcH, f.view, this.cursor, followMargin(f.el));
    return at.x !== this.cursor.x || at.y !== this.cursor.y ? at : null;
  }

  /** Before an on-screen mouse button acts at the cursor: the cursor brought back into view when it is out of sight (and nothing else done),
   *  or null when it is in view and the button may act there. */
  bringBack(f: Frame): GestureOut | null {
    // Whatever the mode: the buttons always act at the cursor, so a cursor out of sight is brought back in Direct touch too.
    const at = cursorIntoView(f.el, f.srcW, f.srcH, f.view, this.cursor, followMargin(f.el));
    return at.x !== this.cursor.x || at.y !== this.cursor.y ? this.broughtIn(at) : null;
  }

  /** The cursor brought back into view (`bringIn`): a move there, and no click. */
  private broughtIn(at: { x: number; y: number }): GestureOut {
    this.cursor = at;
    this.rest = { x: 0, y: 0 };
    return { ops: [{ k: "move", x: at.x, y: at.y }] };
  }

  /** A drag is going on (the button is held on the computer). */
  dragging(): boolean {
    return this.state === "drag";
  }

  /** Where the one finger is now (screen pixels), while one finger presses or drags: for the direct mode's magnifier. */
  finger(): { x: number; y: number } | null {
    if ((this.state !== "press" && this.state !== "drag") || this.first === null) return null;
    const t = this.touches.get(this.first);
    return t ? { x: t.x, y: t.y } : null;
  }

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
      this.lastMove = null;
      this.edge = null;
    }
    else if (this.touches.size === 2 && (this.state === "press" || this.state === "idle")) {
      this.state = "two";
      const p = this.pair()!;
      // Trackpad mode zooms around the cursor (where the person is working), direct mode around the fingers.
      const c = toClient(this.cursor.x, this.cursor.y, f.el, f.srcW, f.srcH, f.view);
      const shown = c.x >= f.el.left && c.x <= f.el.left + f.el.width && c.y >= f.el.top && c.y <= f.el.top + f.el.height;
      // A cursor out of the picture shown (zoomed away from it) is not chased: the pinch goes around the fingers.
      this.pinch0 = { d: p.d, view: f.view, mid: p.mid, anchor: this.mode === "trackpad" && shown ? c : p.mid };
    }
    return NONE;
  }

  move(t: Touch, time: number, f: Frame): GestureOut {
    const was = this.touches.get(t.id);
    if (!was) return NONE;
    // The finger's speed, smoothed over a few moves (a single event's time is coarse).
    if (t.id === this.first) {
      const dt = this.lastMove ? time - this.lastMove.time : 0;
      const v = dt > 0 ? Math.hypot(t.x - was.x, t.y - was.y) / dt : (this.lastMove?.speed ?? 0.3);
      this.lastMove = { time, speed: this.lastMove && dt > 0 && dt < 100 ? this.lastMove.speed * 0.5 + v * 0.5 : v };
    }
    if ((this.state === "point" || this.state === "drag" || this.state === "scroll1") && t.id !== this.first) return NONE;
    let dx = t.x - was.x;
    let dy = t.y - was.y;
    // Where the finger was when `NUDGE_MS` passed (its last place before this move): the landing's roll is behind it.
    if (time - was.t0 >= NUDGE_MS && was.xn === undefined) {
      was.xn = was.x;
      was.yn = was.y;
    }
    was.x = t.x;
    was.y = t.y;
    const s = scaleOf(f.el, f.srcW, f.srcH, f.view) || 1;
    if (this.state === "press") {
      const far = Math.hypot(t.x - was.x0, t.y - was.y0);
      if (far <= TAP_SLOP && !this.nudging(was, time)) return NONE;
      // Held, then moved: a drag from where it was held; but never from a trackpad cursor out of sight (then it is a plain slide).
      if (time - was.t0 >= LONG_PRESS_MS && !this.bringIn(f)) {
        this.state = "drag";
        const at = this.mode === "trackpad" ? { ...this.cursor } : this.src(f, was.x0, was.y0);
        if (!at) return NONE;
        const then = this.follow(f, t, t.x - was.x0, t.y - was.y0, s);
        return { ops: [{ k: "down", x: at.x, y: at.y, b: "left" }, ...then.ops], view: then.view, feel: "drag" };
      }
      // Zoomed in, a direct finger moves the cursor too (the person is pointing at something small), not the computer's page.
      this.state = this.mode === "trackpad" || f.view.zoom > 1 ? "point" : "scroll1";
      const at = this.bringIn(f);
      if (at) this.broughtIn(at);
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
          // Moving together: zoomed in, the view moves with the fingers (a pinch's pan); otherwise the computer scrolls.
          else if (Math.hypot(p.mid.x - this.pinch0.mid.x, p.mid.y - this.pinch0.mid.y) > TAP_SLOP) this.state = (this.pinch0.view?.zoom ?? 1) > 1 ? "pinch" : "scroll2";
          else return NONE;
        }
        if (this.state === "pinch") {
          if (!this.pinch0.view || this.pinch0.d <= 0) return NONE;
          const zoomed = zoomAt(f.el, f.srcW, f.srcH, this.pinch0.view, p.d / this.pinch0.d, this.pinch0.anchor.x, this.pinch0.anchor.y);
          const zs = scaleOf(f.el, f.srcW, f.srcH, zoomed) || 1;
          // The view goes where the fingers take it, the cursor's place included: moving the view on purpose. The next trackpad move
          // brings a cursor left out of it back in (`cursorIntoView`).
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
    const k = (TRACKPAD_GAIN * accel(this.lastMove?.speed ?? 0.3)) / s;
    const fx = Math.min(Math.max(this.cursor.x + this.rest.x + dx * k, 0), f.srcW - 1);
    const fy = Math.min(Math.max(this.cursor.y + this.rest.y + dy * k, 0), f.srcH - 1);
    const x = Math.round(fx);
    const y = Math.round(fy);
    this.rest = { x: fx - x, y: fy - y };
    const from = this.cursor;
    this.cursor = { x, y };
    // Zoomed in, the view goes with the cursor once it leaves the inner area, by as much as it went on past it.
    const view = followView(f.el, f.srcW, f.srcH, f.view, from, this.cursor, followMargin(f.el));
    return view === f.view ? { ops: [{ k: "move", x, y }] } : { ops: [{ k: "move", x, y }], view };
  }

  /** Direct touch, zoomed in, one finger moving the cursor or dragging: the view can pan while it is held near an edge (`tick`). */
  edgePanning(): boolean {
    return this.mode === "direct" && (this.state === "point" || this.state === "drag");
  }

  /** Called each frame while `edgePanning()`: a finger held near an edge for `EDGE_PAN_DELAY_MS` pans the view that way, faster the nearer
   *  it is, and the cursor (under the finger) goes with the picture. Away from the edges, or at the picture's own edge, nothing. */
  tick(time: number, f: Frame): GestureOut {
    const t = this.first === null ? undefined : this.touches.get(this.first);
    const s = scaleOf(f.el, f.srcW, f.srcH, f.view);
    if (!this.edgePanning() || !t || !s || f.view.zoom <= 1) {
      this.edge = null;
      return NONE;
    }
    const z = Math.min(EDGE_PAN_PX, f.el.width / 4, f.el.height / 4);
    // -1..1: how deep into the left (top) or the right (bottom) band the finger is.
    const depth = (p: number, lo: number, size: number) => (p < lo + z ? -Math.min(1, (lo + z - p) / z) : p > lo + size - z ? Math.min(1, (p - (lo + size - z)) / z) : 0);
    const ux = depth(t.x, f.el.left, f.el.width);
    const uy = depth(t.y, f.el.top, f.el.height);
    if (!ux && !uy) {
      this.edge = null;
      return NONE;
    }
    if (!this.edge) {
      this.edge = { since: time, last: time };
      return NONE;
    }
    // A late frame (a busy page) is not a jump.
    const dt = Math.min(Math.max(time - this.edge.last, 0), 50);
    this.edge.last = time;
    if (time - this.edge.since < EDGE_PAN_DELAY_MS) return NONE;
    const step = (EDGE_PAN_SPEED * dt) / s;
    const view = clampView(f.el, f.srcW, f.srcH, { ...f.view, cx: f.view.cx + ux * step, cy: f.view.cy + uy * step });
    if (view.cx === f.view.cx && view.cy === f.view.cy) return NONE;
    const p = toSource(t.x, t.y, f.el, f.srcW, f.srcH, view);
    if (!p) return { ops: [], view };
    this.cursor = p;
    return { ops: [{ k: "move", x: p.x, y: p.y }], view };
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
      this.edge = null;
    }
    const oneFinger = state === "point" || state === "drag" || state === "scroll1";
    // A finger that landed during a one-finger gesture lifts: the gesture goes on.
    if (oneFinger && t.id !== this.first) return NONE;
    // The first finger ends it, whatever other finger is still down.
    if (oneFinger) {
      this.state = "idle";
      this.first = null;
    }
    // Anything but a tap in between: the next tap is a click of its own.
    const prev = this.lastTap;
    this.lastTap = null;
    if (state === "drag") {
      const at = this.mode === "trackpad" ? this.cursor : this.src(f, t.x, t.y);
      return at ? { ops: [{ k: "up", x: at.x, y: at.y, b: "left" }] } : NONE;
    }
    if (state === "press") {
      // A tap or a long press with the trackpad's cursor out of sight brings it back first, and clicks nothing: the next tap clicks there.
      const hidden = this.bringIn(f);
      if (hidden) return this.broughtIn(hidden);
      // Trackpad: a slow small slide lifts as a nudge of the cursor, never as a click (its last move may not have said so yet).
      was.x = t.x;
      was.y = t.y;
      if (this.nudging(was, time)) return this.follow(f, t, t.x - was.x0, t.y - was.y0, scaleOf(f.el, f.srcW, f.srcH, f.view) || 1);
      const b: Button = time - was.t0 >= LONG_PRESS_MS ? "right" : "left";
      // A second left tap soon after one (near it, in direct mode; anywhere on the glass in trackpad mode, which clicks at the cursor): a
      // double click, at exactly the first one's point, so that the computer counts the two as one (Windows wants them within a few pixels
      // and its double-click time).
      const near = this.mode === "trackpad" || (prev !== null && Math.hypot(was.x0 - prev.x, was.y0 - prev.y) <= DOUBLE_TAP_PX);
      const again = b === "left" && prev !== null && prev.b === "left" && prev.mode === this.mode && time - prev.time <= DOUBLE_TAP_MS && near;
      const at = again ? prev.at : this.mode === "trackpad" ? { ...this.cursor } : this.src(f, was.x0, was.y0);
      if (!at) return NONE;
      if (this.mode === "direct") this.cursor = at;
      // A third tap starts again: a double click, then a click.
      this.lastTap = again ? null : { time, x: was.x0, y: was.y0, at, b, mode: this.mode };
      return { ops: [{ k: "click", x: at.x, y: at.y, b }], feel: b === "right" ? "right" : again ? "double" : "click" };
    }
    if (state === "two") {
      // One of two fingers lifted without moving: a right click once the other one is lifted too.
      this.state = this.touches.size ? "two_up" : "idle";
      if (this.touches.size) return NONE;
    }
    if (state === "two" || state === "two_up") {
      if (this.touches.size) return NONE;
      const hidden = this.bringIn(f);
      if (hidden) return this.broughtIn(hidden);
      const at = this.mode === "trackpad" ? this.cursor : this.src(f, this.pinch0.mid.x, this.pinch0.mid.y);
      return at ? { ops: [{ k: "click", x: at.x, y: at.y, b: "right" }], feel: "right" } : NONE;
    }
    return NONE;
  }

  /** The system took the touches (a notification, an edge swipe): a drag lets go where it is, nothing else is sent. */
  cancel(): GestureOut {
    const dragging = this.state === "drag";
    this.touches.clear();
    this.state = "idle";
    this.first = null;
    this.lastTap = null;
    this.carry = { x: 0, y: 0 };
    this.edge = null;
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
  | { t: "ack"; result?: string; sc?: string; title?: string; kind?: string; w?: number; h?: number; encoder?: string; t0?: number; targets?: TargetRow[]; paths?: PathInfo[]; relay?: RelayInfo | null; via?: string; ice_servers?: RTCIceServer[]; turn?: string | null }
  | { t: "error"; code: string; message: string; reason?: string | null; busy?: BusyInfo | null }
  | { t: "offline" }
  | { t: "unknown" };

type Translate = (text: string, vars?: Record<string, string | number>) => string;
const fillIn: Translate = (s, vars) => (vars ? s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : s);

export interface ScreenPort {
  targets(): Promise<Reply>;
  /** `via`: the way chosen (null: the computer does not offer a choice). `recheck`: the passkey assertion a start through the relay needs. */
  start(target: string, via: ViaId | null, recheck?: Record<string, unknown>): Promise<Reply>;
  stop(sc: string): Promise<Reply>;
  text(sc: string, text: string, st: number): Promise<Reply>;
  rebind(sc: string): Promise<Reply>;
  resume(sc: string): void;
  input(sc: string, ops: ScOp[], st: number): void;
  ack(sc: string, seq: number): void;
  keyframe(sc: string): void;
  /** The crisp tiles of `rect` (source pixels) once, while crisp text is off for a slow connection. */
  sharpen(sc: string, rect: Rect): void;
  /** The connection is checked now, and made again when it is gone, leaving one being made to finish (`Link.nudge`). */
  kick(): void;
  /** The way this phone's connection takes now, and whether this app came from the relay (the only way it can reach the computer). */
  route(): "direct" | "relay";
  hosted(): boolean;
  /** The way chosen last for this pairing, and keeping a new choice. */
  remembered(): ViaId | null;
  remember(via: ViaId): void;
  /** The person's Face ID over the screen's challenge: the assertion `sc_start` takes as `recheck`; null when they did not give it. */
  proof(target: string): Promise<Record<string, unknown> | null>;
  /** Peer to peer, at a computer that has it: whether this browser can (WebRTC), a peer for screen `sc` (its media go to `deliver`), the way from now
   *  on (`sc_via`), and a media message to the view (as the link's are). */
  webrtc?(): boolean;
  peer?(sc: string, ev: PeerEvents, setup: PeerSetup): PeerLike | null;
  /** The ICE servers for a peer of screen `sc` (`sc_rtc_servers`, from a computer that has TURN): null from one before. */
  servers?(sc: string, relayOnly: boolean): Promise<Reply> | null;
  via?(sc: string, via: ViaId): Promise<Reply>;
  deliver?(bytes: Uint8Array): void;
  serverNow(): number;
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(t: unknown): void;
}

/** What the session needs of a peer (`screen-rtc.ts` `ScreenPeer`). */
export interface PeerLike {
  start(): Promise<void>;
  /** A message on "i"; false when it cannot go there now (it goes on the link instead). */
  send(msg: object): boolean;
  stats(): Promise<PathStats | null>;
  close(): void;
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
  /** What this screen sent through TURN so far (MB), once anything did. */
  turn_mb?: number | null;
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
  /** The computer's clock when the stream started (ms): a frame's capture time is counted from it. Null from a computer that does not say. */
  t0: number | null;
  /** How old the pictures shown over the last second were when shown, the median (ms): from their capture on the computer. */
  age: number | null;
  /** Frozen: the seconds the computer still keeps the screen for this phone while it reconnects. */
  graceLeft: number | null;
  /** The ways the computer lists for the screen, the relay's name, and the one chosen (`pick`; null paths: a computer that offers no choice). */
  paths: PathInfo[] | null;
  relay: RelayInfo | null;
  via: ViaId;
  /** The way the computer used for the screen that started (`ack.via`). */
  usedVia: string | null;
  /** Where the picture goes now, as the computer last said (`sc_path`): peer to peer, or the link; null before it said. And the browser's reading of the
   *  peer's way (P2P or TURN, the round trip). */
  path: "p2p" | "link" | null;
  pathStats: PathStats | null;
  /** Peer to peer failed from here a moment ago (`P2P_RETRY_MS`): Automatic does not try it on its own meanwhile. */
  p2pFailed: boolean;
  /** The computer was too busy for `target`: the phone asks again at `at` (`port.now()`), try `attempt` of `BUSY_RETRY_MS`. */
  retry: { target: string; at: number; attempt: number } | null;
}

const fresh = (): ScreenState => ({ phase: "pick", targets: null, sc: null, title: "", kind: "", remote: "live", stats: null, encoder: "hw", message: null, text: null, stuck: false, t0: null, age: null, graceLeft: null, paths: null, relay: null, via: "auto", usedVia: null, path: null, pathStats: null, p2pFailed: false, retry: null });

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
    case "route_changed":
      return t("The connection now goes another way: see the screen again.");
    case "relay_budget_spent":
      return t("This month's screen allowance through the relay is used up. It starts again next month, or use Tailscale.");
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
    case "turn_budget_spent":
      return t("This computer's TURN allowance is used up, and the screen could not go back to the relay.");
    case "p2p_failed":
      return t("The direct (peer to peer) connection failed. Choose Automatic to fall back to the relay.");
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

/** The details line: the way, the round trip, the rate, the frames, the size and the pictures' age: `Tailscale · 85 ms · 1200 kbps · 30 fps · 1080p · 120 ms old`. */
export function detailsText(s: Stats | null, path: string | null, age: number | null, t: (s: string, vars?: Record<string, string | number>) => string, via: string | null = null, p2p: PathStats | null = null): string {
  const parts: string[] = [];
  // Peer to peer: P2P or TURN as the browser sees it. Otherwise the way the computer says it used, else the phone's own reading of its connection.
  const way = p2p ? (p2p.path === "turn" ? t("TURN") : t("P2P")) : (viaText(via, t) ?? path);
  if (way) parts.push(way);
  const rtt = p2p?.rttMs ?? s?.stages?.ack_rtt_ms_p50;
  if (typeof rtt === "number") parts.push(t("{ms} ms round trip", { ms: Math.round(rtt) }));
  if (typeof s?.kbps === "number") parts.push(`${Math.round(s.kbps)} kbps`);
  if (s?.h) parts.push(`${Math.round(s.fps ?? 0)} fps`, `${Math.round(s.h)}p`);
  if (age !== null) parts.push(t("{ms} ms old", { ms: Math.round(age) }));
  if (p2p?.path === "turn" && typeof s?.turn_mb === "number") parts.push(t("{mb} MB through TURN", { mb: s.turn_mb }));
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
  private frozenAt = 0;
  private kickedAt = -Infinity;
  private sharpenedAt = -Infinity;
  private ages: number[] = [];
  /** Peer to peer: the peer the computer moved the picture onto, one being opened, the way the person chose, when it last failed from here, and the
   *  order video comes in from both ways. */
  private peer: PeerLike | null = null;
  private nextPeer: PeerLike | null = null;
  private chosen: ViaId = "auto";
  private p2pFailedAt = -Infinity;
  private statsAt = 0;
  private order = new VideoOrder();
  private peerGen = 0;
  /** The kind of peer opened last (TURN alone or not), which a reconnect or renewed credentials open again. */
  private peerRelayOnly = false;

  /** The next try after a busy refusal (`busy`). */
  private retryTimer: unknown = null;

  constructor(private port: ScreenPort, private changed: (s: ScreenState | null) => void, private t: Translate = fillIn) {}

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
    this.stopRetry();
    this.set(fresh());
    const r = await this.port.targets();
    if (this.phase() !== "pick") return;
    if (r.t === "ack") {
      const paths = r.paths ?? null;
      const relay = r.relay ?? null;
      const p2pFailed = this.p2pFailedNow();
      const via = chooseVia(pathRows(paths, relay, this.side(p2pFailed), this.t), this.port.remembered());
      this.patch({ targets: r.targets ?? [], paths, relay, via, p2pFailed });
    } else this.patch({ targets: [], message: r.t === "error" ? r.message : this.t("Not connected.") });
  }

  /** What this phone knows of its own ways (`pathRows`). */
  side(p2pFailed = this.p2pFailedNow()) {
    return { hosted: this.port.hosted(), webrtc: this.port.webrtc?.() ?? false, p2pFailed };
  }

  private p2pFailedNow(): boolean {
    return this.port.now() - this.p2pFailedAt < P2P_RETRY_MS;
  }

  /** The way to connect, chosen in the list; kept for the next time on this pairing. */
  choosePath(via: ViaId) {
    if (this.state?.phase !== "pick" || !this.state.paths?.some((p) => p.id === via)) return;
    this.patch({ via });
    this.port.remember(via);
  }

  async pick(target: string) {
    if (this.state?.phase !== "pick") return;
    this.stopRetry();
    const via = this.state.paths ? this.state.via : null;
    this.patch({ phase: "starting", message: null, retry: null });
    this.early = 0;
    this.earlyKey = false;
    // Through the relay the computer wants the person's Face ID for this very screen, asked for now, while the tap that started it is fresh.
    let recheck: Record<string, unknown> | undefined;
    if (via && needsProof(via, this.port.route())) {
      const proof = await this.port.proof(target);
      if (this.phase() !== "starting") return;
      if (!proof) {
        this.patch({ phase: "pick", message: this.t("Not verified, so the screen was not started. Try again when you are ready.") });
        return;
      }
      recheck = proof;
    }
    await this.begin(target, via, recheck, 0, false);
  }

  /** The person stops the tries after a busy refusal; what the computer said stays. */
  cancelRetry() {
    this.stopRetry();
    if (this.state?.retry) this.patch({ retry: null });
  }

  private stopRetry() {
    if (this.retryTimer !== null) this.port.clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }

  /** The computer was too busy (`host_busy`): its reason on the picker, and one more try after the next wait, until they run out. */
  private busy(target: string, r: { message: string; reason?: string | null; busy?: BusyInfo | null }, attempt: number) {
    this.stopRetry();
    const text = busyText(r.reason, r.busy, r.message, this.t);
    const wait = BUSY_RETRY_MS[attempt];
    if (wait === undefined) {
      this.patch({ phase: "pick", message: `${text} ${this.t("Still busy. Tap See it to try again.")}`, retry: null });
      return;
    }
    this.patch({ phase: "pick", message: text, retry: { target, at: this.port.now() + wait, attempt } });
    this.retryTimer = this.port.setTimeout(() => {
      this.retryTimer = null;
      void this.retry(target, attempt + 1);
    }, wait);
  }

  /** A try on its own after a busy refusal, the way chosen. Never with Face ID, which only a tap may ask for: the computer looks at its load
   *  before the passkey, so a start that needs one is refused for the load while it is still busy, and for the missing Face ID once it is not. */
  private async retry(target: string, attempt: number) {
    if (this.state?.phase !== "pick" || this.state.retry?.target !== target) return;
    const via = this.state.paths ? this.state.via : null;
    this.patch({ phase: "starting", retry: null });
    this.early = 0;
    this.earlyKey = false;
    await this.begin(target, via, undefined, attempt, true);
  }

  private async begin(target: string, via: ViaId | null, recheck: Record<string, unknown> | undefined, attempt: number, retrying: boolean) {
    const r = await this.port.start(target, via, recheck);
    if (this.phase() !== "starting") {
      // Closed while the computer was starting it (the person left the app): it would go on capturing with nobody watching.
      if (r.t === "ack" && r.sc) void this.port.stop(r.sc);
      return;
    }
    if (r.t !== "ack" || !r.sc) {
      if (r.t === "error" && r.code === "host_busy") return this.busy(target, r, attempt);
      if (retrying && r.t === "error" && r.code === "needs_face_id") {
        this.patch({ phase: "pick", message: this.t("The computer is less busy now. Tap See it to start.") });
        return;
      }
      this.patch({ phase: "pick", message: r.t === "error" ? startRefusal(r.code, r.message, this.t) : this.t("Not connected.") });
      return;
    }
    const now = this.port.now();
    this.heardAt = this.mediaAt = this.decodedAt = now;
    this.ages = [];
    this.chosen = via ?? "auto";
    this.patch({ phase: "live", sc: r.sc, title: r.title ?? "", kind: r.kind ?? "", encoder: r.encoder ?? "hw", t0: typeof r.t0 === "number" ? r.t0 : null, usedVia: r.via ?? null, path: null, pathStats: null });
    // Peer to peer from the start (the computer waits for it), or Automatic moving onto it from the relay once it opens.
    const kind = peerFor(this.chosen, r.via, pathRows(this.state?.paths, this.state?.relay, this.side(), this.t));
    if (kind) void this.openPeer(kind === "turn");
    if (this.early) this.port.ack(r.sc, this.early);
    this.early = 0;
    if (this.earlyKey) {
      this.earlyKey = false;
      this.keyAskedAt = now;
      this.port.keyframe(r.sc);
    }
    this.watch();
  }

  /** `sc_state`, `sc_stats`, `sc_fhb`, `sc_path`, `sc_end`. */
  onMessage(m: { type: string; sc?: string; state?: string; reason?: string; encoder?: string; path?: string; why?: string; stages?: Record<string, number | null>; ice_servers?: RTCIceServer[] } & Stats) {
    const s = this.state;
    if (!s?.sc || m.sc !== s.sc || s.phase === "ended") return;
    if (m.type === "sc_end") return this.ended(endedText(m.reason ?? "", this.t));
    this.heardAt = this.port.now();
    if (m.type === "sc_path") return this.onPath(m.path ?? "", m.state ?? "", m.why ?? null);
    // TURN's renewed credentials: a new peer with them (the computer swaps to it once it opens, and the old one's credentials end on their own).
    if (m.type === "sc_rtc_servers") {
      if (m.ice_servers?.length && this.state?.path === "p2p") void this.openPeer(this.peerRelayOnly, m.ice_servers);
      return;
    }
    if (m.type === "sc_state" && m.state === "encoder") this.patch({ encoder: m.encoder === "sw" ? "sw" : "hw" });
    else if (m.type === "sc_state" && m.state) this.patch({ remote: m.state });
    else if (m.type === "sc_stats") {
      this.patch({ stats: { fps: m.fps, w: m.w, h: m.h, kbps: m.kbps, encode_ms_p95: m.encode_ms_p95, dropped: m.dropped, reason: m.reason, degraded: m.degraded, level: m.level, encoder: m.encoder, crisp: m.crisp, stages: m.stages, turn_mb: m.turn_mb }, age: quantile([...this.ages].sort((a, b) => a - b), 0.5) });
      this.ages = [];
    }
    this.thawed();
  }

  /** The computer moved the picture (`sc_path`): onto the peer opened last, back onto the link, or a peer failed. */
  private onPath(path: string, state: string, why: string | null) {
    if (state === "live" && path === "p2p") {
      if (this.nextPeer) {
        this.peer?.close();
        this.peer = this.nextPeer;
        this.nextPeer = null;
      }
      this.patch({ path: "p2p" });
    } else if (state === "live") {
      this.closePeers();
      this.patch({ path: "link", pathStats: null });
    } else if (state === "failed") {
      // A peer that never opened from here says this network does not let one through; one that went live and then dropped (the phone moved
      // networks) does not: a new one is offered once the link is back.
      const liveDropped = this.state?.path === "p2p" && !this.nextPeer;
      if (why !== "chosen" && !liveDropped) this.p2pFailedAt = this.port.now();
      // A new peer failed while the picture still goes on another way: only that one goes.
      this.nextPeer?.close();
      this.nextPeer = null;
      if (path !== "p2p") this.closePeers();
      this.patch({ p2pFailed: this.p2pFailedNow() });
    }
  }

  /** A peer for this screen, offered now: the computer moves the picture onto it once it opens, and keeps the one before until then. Its servers
   *  come from the computer first (TURN through the relay); `relayOnly`: through TURN alone, which needs them. */
  private async openPeer(relayOnly: boolean, given?: RTCIceServer[]) {
    const sc = this.state?.sc;
    if (!sc || !this.port.peer || !(this.port.webrtc?.() ?? false)) return;
    this.nextPeer?.close();
    this.nextPeer = null;
    // Only the newest ask opens a peer: an older one still waiting for its servers gives up.
    const gen = ++this.peerGen;
    // *Direct (peer to peer)* chosen: STUN only, never TURN (that is the relay's way, with its own choice).
    const directOnly = this.chosen === "p2p" && !relayOnly;
    const asked = given ? Promise.resolve<Reply>({ t: "ack", ice_servers: given, turn: null }) : directOnly ? null : (this.port.servers?.(sc, relayOnly) ?? null);
    let setup: PeerSetup = { ...STUN_ONLY, relayOnly };
    if (asked) {
      const r = await asked;
      if (gen !== this.peerGen || this.state?.sc !== sc || this.state.phase === "ended") return;
      if (r.t === "ack" && r.ice_servers?.length) setup = { servers: r.ice_servers, relayOnly };
      // TURN alone with no TURN given: there is nothing to open; the picture stays where it is, and the row says why.
      if (relayOnly && (r.t !== "ack" || r.turn)) {
        this.patch({ message: r.t === "ack" && r.turn ? this.t("TURN is not available now: the picture stays on the relay.") : this.state.message });
        return;
      }
    } else if (relayOnly) return;
    const peer: PeerLike | null = this.port.peer(sc, {
      media: (bytes) => this.port.deliver?.(bytes),
      open: () => {},
      failed: () => {
        if (this.nextPeer === peer) {
          this.nextPeer = null;
          this.p2pFailedAt = this.port.now();
          if (this.state && this.state.phase !== "ended") this.patch({ p2pFailed: true });
        }
        if (this.peer === peer) this.peer = null;
      },
    }, setup);
    if (!peer) return;
    // One opened meanwhile by another call (TypeScript keeps the null set above).
    (this.nextPeer as PeerLike | null)?.close();
    this.nextPeer = peer;
    this.peerRelayOnly = relayOnly;
    void peer.start();
  }

  private closePeers() {
    this.peer?.close();
    this.nextPeer?.close();
    this.peer = this.nextPeer = null;
  }

  /** The way from now on, chosen in the live view: the computer switches, and a peer is opened for a way that needs one. */
  async switchVia(via: ViaId) {
    const sc = this.state?.sc;
    if (!sc || !this.port.via || !this.live()) return;
    const r = await this.port.via(sc, via);
    if (this.state?.sc !== sc) return;
    if (r.t !== "ack") {
      this.patch({ message: r.t === "error" ? r.message : this.t("Not connected.") });
      return;
    }
    this.chosen = via;
    this.port.remember(via);
    this.patch({ via, message: null });
    if (via === "relay_turn") void this.openPeer(true);
    else if ((via === "p2p" || (via === "auto" && this.port.route() === "relay")) && this.state?.path !== "p2p") void this.openPeer(false);
  }

  /** A media message, from the link or peer to peer: whether it goes to the view. A video frame older than one already taken does not (in a switch
   *  the old way's last frames can come after the new way's first, which is a keyframe). */
  incoming(bytes: Uint8Array): boolean {
    // Only a screen starting or shown takes media. A closed one's last frames still come (a page that slept reads all that was sent
    // meanwhile, after it closed the screen): taken, they would put the order past the next screen's frames, which count from 0
    // again, and that screen would show nothing and ask for keyframes for good.
    const phase = this.state?.phase;
    if (phase !== "starting" && phase !== "live" && phase !== "frozen") return false;
    const video = bytes[1] === KIND_VIDEO;
    if (video) {
      const m = parseMedia(bytes);
      if (m?.kind === "video" && !this.order.take(m.seq)) return false;
    }
    this.media(video);
    return true;
  }

  /** A media message arrived (the view decodes it). Only video says frames are coming: the cursor moves on its own. */
  media(video = true) {
    const now = this.port.now();
    this.heardAt = now;
    if (video) this.mediaAt = now;
    this.thawed();
  }

  private thawed() {
    if (this.state?.phase !== "frozen") return;
    this.patch({ phase: "live", graceLeft: null });
    // Heard again on the same connection (it never dropped, so there was no rebind): what the computer sent meanwhile may never have
    // arrived (a phone asleep behind the relay), and it would wait for those frames' acknowledgements for good, sending nothing and
    // dropping every touch as onto a stale picture. A keyframe makes the helper give them up, and its acknowledgement clears them all.
    this.needKeyframe();
  }

  /** A video frame captured `captureMs` after the stream started was shown: how old it was, by the computer's clock. */
  presented(captureMs: number) {
    const t0 = this.state?.t0;
    if (t0 == null) return;
    const age = this.port.serverNow() - (t0 + captureMs);
    if (Number.isFinite(age) && age >= 0 && this.ages.length < AGES_KEPT) this.ages.push(age);
  }

  /** Sharpen: the part of the picture shown now (`rect`, source pixels) sent crisp once, at most every `SHARPEN_GAP_MS`, and only while
   *  crisp text is off for a slow connection. False when nothing was asked. */
  sharpen(rect: Rect | null): boolean {
    const now = this.port.now();
    if (!rect || !this.live() || !this.state?.sc || this.state.stats?.crisp !== false || now - this.sharpenedAt < SHARPEN_GAP_MS) return false;
    this.sharpenedAt = now;
    this.port.sharpen(this.state.sc, rect);
    return true;
  }

  /** Video frame `seq` came out of the decoder: the computer may send more. */
  decoded(seq: number) {
    this.decodedAt = this.port.now();
    if (this.state?.stuck) this.patch({ stuck: false });
    this.acknowledge(seq);
  }

  private acknowledge(seq: number) {
    if (this.state?.sc && this.state.phase !== "ended") {
      // Peer to peer, on "i"; on the link when that cannot take it.
      if (!(this.state.path === "p2p" && this.peer?.send({ type: "sc_ack", seq }))) this.port.ack(this.state.sc, seq);
    }
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
    if (ops.length && this.state?.sc && this.live()) {
      const st = this.port.serverNow();
      if (!(this.state.path === "p2p" && this.peer?.send({ type: "sc_input", ops, st }))) this.port.input(this.state.sc, ops, st);
    }
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
    // A try waiting after a busy refusal ends too: it would start a screen with no tap once the phone is unlocked.
    this.cancelRetry();
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
      this.patch({ phase: "live", graceLeft: null });
      // The phone's network may have changed under its peer: a new one is opened, and the computer keeps the old one until it opens. Automatic
      // whose peer dropped meanwhile (the picture went back to the relay) tries again too.
      const kind = peerFor(this.chosen, this.state?.usedVia, pathRows(this.state?.paths, this.state?.relay, this.side(), this.t));
      if (this.state?.path === "p2p" || this.peer || this.nextPeer) void this.openPeer(this.peerRelayOnly);
      else if (kind) void this.openPeer(kind === "turn");
    } else if (r.t === "error") this.ended(endedText(r.code === "route_changed" ? "route_changed" : "link_lost", this.t));
  }

  close() {
    void this.stop();
    this.stopRetry();
    this.unwatch();
    this.closePeers();
    this.order.reset();
    this.set(null);
  }

  private ended(message: string) {
    this.queue = [];
    this.unwatch();
    this.closePeers();
    this.order.reset();
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
        this.frozenAt = now;
        this.patch({ phase: "frozen", graceLeft: Math.ceil(REBIND_GRACE_MS / 1000) });
      }
      // Frozen: the connection is checked, and made again when it is gone (then the screen is asked for again on it), while the computer
      // keeps it; the person sees how long that is.
      if (this.state?.phase === "frozen") {
        if (now - this.kickedAt >= KICK_EVERY_MS) {
          this.kickedAt = now;
          this.port.kick();
        }
        const left = Math.max(0, Math.ceil((REBIND_GRACE_MS - (now - this.frozenAt)) / 1000));
        if (left !== this.state.graceLeft) this.patch({ graceLeft: left });
      }
      if (this.state?.phase === "live" && this.mediaAt - this.decodedAt > STUCK_MS) this.needKeyframe();
      // The peer's way and round trip, once a second, for the details line.
      if (this.peer && this.state?.path === "p2p" && now - this.statsAt >= 1000) {
        this.statsAt = now;
        const peer = this.peer;
        void peer.stats().then((p) => {
          if (p && this.peer === peer && this.state?.path === "p2p") this.patch({ pathStats: p });
        });
      }
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
