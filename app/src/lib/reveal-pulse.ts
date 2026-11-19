// Where a reveal landed, made visible: the session just revealed (a sidebar row, a sub-session chip, the palette, a notice...)
// pulses for a moment in its pane and on its sidebar row, so the eye follows the window to it. The pulse is a short fade, never a
// loop; with reduced motion it is a steady mark for the same moment (styles.css, `.reveal-pulse-*`).
import { createSignal } from "solid-js";

/** How long a pulse lasts: the CSS animation's length. */
export const PULSE_MS = 900;

const [pulse, setPulse] = createSignal<{ id: string; n: number } | null>(null);
let count = 0;
let timer: ReturnType<typeof setTimeout> | undefined;

/** The session pulsing now, or null. */
export const pulsingSession = () => pulse()?.id ?? null;

/** The class that plays the pulse on an element of session `id` (with its leading space), or "". Two names alternate so that a
 *  second reveal of the same session restarts the animation instead of leaving it where it was. */
export function pulseClass(id: string | null | undefined): string {
  const p = pulse();
  return p && id && p.id === id ? (p.n % 2 ? " reveal-pulse-a" : " reveal-pulse-b") : "";
}

export function pulseSession(id: string) {
  setPulse({ id, n: ++count });
  clearTimeout(timer);
  timer = setTimeout(() => setPulse(null), PULSE_MS);
}
