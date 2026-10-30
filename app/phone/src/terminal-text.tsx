// A terminal session as text: the daemon's rows in a monospace block that follows the bottom while you
// are there, with pinch zoom. (The phone gets rows of text from the daemon, never a terminal emulator.)
import { createEffect, createSignal, onCleanup } from "solid-js";
import { store } from "./storage";

const ZOOM_KEY = "kelpie-zoom";
const MIN = 8;
const MAX = 24;

const [size, setSize] = createSignal(Math.min(MAX, Math.max(MIN, Number(store.get(ZOOM_KEY)) || 12)));

/** The new font size after a pinch that started at `start` px with fingers `from` apart and is now `to` apart. */
export function pinchSize(start: number, from: number, to: number): number {
  if (!(from > 0) || !(to > 0)) return start;
  return Math.round(Math.min(MAX, Math.max(MIN, start * (to / from))) * 2) / 2;
}

export function TerminalText(props: { rows: string[]; dim?: boolean }) {
  let pre!: HTMLPreElement;
  const pointers = new Map<number, { x: number; y: number }>();
  let pinch: { from: number; start: number } | null = null;
  const distance = () => {
    const [a, b] = [...pointers.values()];
    return Math.hypot(a.x - b.x, a.y - b.y);
  };

  createEffect(() => {
    const text = props.rows.join("\n");
    // Stay at the bottom if that is where the reader is.
    const atEnd = pre.scrollTop + pre.clientHeight >= pre.scrollHeight - 30;
    pre.textContent = text;
    if (atEnd) pre.scrollTop = pre.scrollHeight;
  });

  const down = (e: PointerEvent) => {
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 2) pinch = { from: distance(), start: size() };
  };
  const move = (e: PointerEvent) => {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
    if (pointers.size === 2 && pinch) setSize(pinchSize(pinch.start, pinch.from, distance()));
  };
  const up = (e: PointerEvent) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2 && pinch) {
      pinch = null;
      store.set(ZOOM_KEY, String(size()));
    }
  };
  onCleanup(() => pointers.clear());

  return (
    <pre
      ref={pre}
      class="screen"
      classList={{ dim: props.dim }}
      style={{ "font-size": `${size()}px` }}
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={up}
      onPointerCancel={up}
    />
  );
}
