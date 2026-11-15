// What the measurement reads, the same way whichever way the page draws the cursor: installed before the screen view loads.
// - Like Safari on iOS, the canvas is never `desynchronized` (unless ?desync=1).
// - Each video picture drawn (drawImage of a VideoFrame) is counted and timed.
// - Where the cursor is shown, in CSS pixels from the picture box's corner: a cursor drawn into the canvas is read from the transform its
//   drawing scales (canvas pixels / devicePixelRatio); a cursor element ([data-screen-cursor]) from its own transform.
// - Each pointermove: its wait before handling, the handling's time, and the next animation frame after it.
// - Each animation frame: its time and where the cursor was shown.

const desync = new URLSearchParams(location.search).get("desync") === "1";
const getContext = HTMLCanvasElement.prototype.getContext;
HTMLCanvasElement.prototype.getContext = function (this: HTMLCanvasElement, kind: string, opts?: Record<string, unknown>) {
  return (getContext as (k: string, o?: unknown) => RenderingContext | null).call(this, kind, opts && !desync ? { ...opts, desynchronized: false } : opts);
} as typeof HTMLCanvasElement.prototype.getContext;

export const rec = {
  videoDraws: [] as { at: number; ms: number }[],
  moves: [] as { ts: number; start: number; end: number; frame: number | null }[],
  frames: [] as { at: number; x: number | null; y: number | null }[],
  canvasCursor: null as { x: number; y: number } | null,
};

const drawImage = CanvasRenderingContext2D.prototype.drawImage;
CanvasRenderingContext2D.prototype.drawImage = function (this: CanvasRenderingContext2D, ...args: unknown[]) {
  if (typeof VideoFrame !== "undefined" && args[0] instanceof VideoFrame) {
    const t0 = performance.now();
    (drawImage as (...a: unknown[]) => void).apply(this, args);
    rec.videoDraws.push({ at: t0, ms: performance.now() - t0 });
    return;
  }
  (drawImage as (...a: unknown[]) => void).apply(this, args);
} as typeof CanvasRenderingContext2D.prototype.drawImage;

// Only the cursor's drawing scales the canvas (by the device pixel ratio, after moving to the cursor).
const scale = CanvasRenderingContext2D.prototype.scale;
CanvasRenderingContext2D.prototype.scale = function (this: CanvasRenderingContext2D, x: number, y: number) {
  const m = this.getTransform();
  const dpr = window.devicePixelRatio || 1;
  rec.canvasCursor = { x: m.e / dpr, y: m.f / dpr };
  scale.call(this, x, y);
};

/** Where the cursor is shown now (CSS pixels from the box's corner), or null when it is not. */
export function shownCursor(): { x: number; y: number } | null {
  const el = document.querySelector<HTMLElement>("[data-screen-cursor]");
  if (el) {
    if (el.style.display === "none" || el.hidden) return null;
    const m = /translate(?:3d)?\(\s*(-?[\d.]+)px,\s*(-?[\d.]+)px/.exec(el.style.transform);
    return m ? { x: Number(m[1]), y: Number(m[2]) } : null;
  }
  return rec.canvasCursor;
}

let pendingFrame: number[] = [];
window.addEventListener("pointermove", (e) => {
  rec.moves.push({ ts: e.timeStamp, start: performance.now(), end: 0, frame: null });
}, { capture: true });
window.addEventListener("pointermove", () => {
  const m = rec.moves[rec.moves.length - 1];
  if (m) {
    m.end = performance.now();
    pendingFrame.push(rec.moves.length - 1);
  }
});
const tick = () => {
  const at = performance.now();
  for (const i of pendingFrame) rec.moves[i].frame = at;
  pendingFrame = [];
  const c = shownCursor();
  rec.frames.push({ at, x: c?.x ?? null, y: c?.y ?? null });
  requestAnimationFrame(tick);
};
requestAnimationFrame(tick);
