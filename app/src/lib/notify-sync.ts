// Tells the daemon what this window shows (`ui.focus`, API 12): whether it has focus, and which
// sessions are on screen. The daemon decides from that whether a status change is worth an event,
// for the desktop notification and the phone alike: a session in front of the user is not.
import { createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "./store";
import { daemon } from "./ipc";
import { NOTIFY_API } from "./notify-events";

export interface FocusReport {
  focused: boolean;
  visible: string[];
  /** When the person last pressed a key or moved the pointer in this window (ms): where the system has
   *  no idle clock the daemon tells "away" from it. */
  active_at: number;
}

/** Activity is sent at most this often: the daemon only needs it to the minute. */
const ACTIVITY_BUCKET = 60_000;

/** Sends a report only when it differs from the last one that went out; `reset` after a reconnect
 *  (a new connection starts with no report). */
export function makeReporter(send: (r: FocusReport) => void) {
  let last = "";
  return {
    report(focused: boolean, visible: string[], activeAt: number) {
      const r: FocusReport = { focused, visible: focused ? [...visible].sort() : [], active_at: activeAt };
      const key = JSON.stringify({ ...r, active_at: Math.floor(activeAt / ACTIVITY_BUCKET) });
      if (key === last) return;
      last = key;
      send(r);
    },
    reset() {
      last = "";
    },
  };
}

/** Call once from the root component. */
export function initNotifySync() {
  const [focused, setFocused] = createSignal(document.hasFocus() && !document.hidden);
  const reporter = makeReporter((r) => void daemon("ui.focus", r).catch(() => {}));
  let timer = 0;
  const [activeAt, setActiveAt] = createSignal(Date.now());
  onMount(() => {
    const update = () => setFocused(document.hasFocus() && !document.hidden);
    // Keys and the pointer, noted at most every 20 s (the report to the daemon is per minute).
    let lastNoted = 0;
    const touch = () => {
      const now = Date.now();
      if (now - lastNoted > 20_000) {
        lastNoted = now;
        setActiveAt(now);
      }
    };
    for (const e of ["keydown", "pointerdown", "pointermove", "wheel"]) window.addEventListener(e, touch, { capture: true, passive: true });
    window.addEventListener("focus", update);
    window.addEventListener("blur", update);
    document.addEventListener("visibilitychange", update);
    onCleanup(() => {
      window.removeEventListener("focus", update);
      window.removeEventListener("blur", update);
      document.removeEventListener("visibilitychange", update);
      for (const e of ["keydown", "pointerdown", "pointermove", "wheel"]) window.removeEventListener(e, touch, { capture: true });
      clearTimeout(timer);
    });
  });
  createEffect(() => {
    if (!store.connected()) reporter.reset();
  });
  createEffect(() => {
    if (!store.connected() || store.daemonApi() < NOTIFY_API) return;
    const f = focused();
    const active = activeAt();
    const visible = f ? Object.keys(store.sessions).filter((id) => store.isSessionVisible(id)) : [];
    clearTimeout(timer);
    timer = window.setTimeout(() => reporter.report(f, visible, active), 150);
  });
}
