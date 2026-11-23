// The phone's icons: one stroke style (24 px grid, 2 px round strokes, currentColor), drawn inline so the strict CSP and the
// hosted page's trust model need no image or font from anywhere. Decorative: the button around an icon carries its name.
import type { JSX } from "solid-js";

const PATHS: Record<string, () => JSX.Element> = {
  back: () => <path d="M15 18l-6-6 6-6" />,
  forward: () => <path d="M9 18l6-6-6-6" />,
  plus: () => <path d="M12 5v14M5 12h14" />,
  close: () => <path d="M18 6L6 18M6 6l12 12" />,
  gear: () => (
    <>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z" />
    </>
  ),
  lock: () => (
    <>
      <rect x="4" y="11" width="16" height="10" rx="2.5" />
      <path d="M8 11V8a4 4 0 0 1 8 0v3" />
    </>
  ),
  monitor: () => (
    <>
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M8 20h8M12 16v4" />
    </>
  ),
  globe: () => (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18" />
    </>
  ),
  // Session states: an exclamation mark for "needs you", a pulse for "working", a check for "done", a moon for "idle", a square for "stopped".
  needs: () => <path d="M12 7v6M12 17h.01" />,
  working: () => <path d="M3 12h4l3-7 4 14 3-7h4" />,
  done: () => <path d="M6 12.5l4 4 8-9" />,
  idle: () => <path d="M17 14.5A6 6 0 0 1 9.5 7a6 6 0 1 0 7.5 7.5z" />,
  stopped: () => <rect x="8" y="8" width="8" height="8" rx="1.5" />,
  alert: () => (
    <>
      <path d="M10.3 4.2L2.6 18a2 2 0 0 0 1.7 3h15.4a2 2 0 0 0 1.7-3L13.7 4.2a2 2 0 0 0-3.4 0z" />
      <path d="M12 10v4M12 17.5h.01" />
    </>
  ),
  offline: () => (
    <>
      <path d="M2 8.8a15 15 0 0 1 4.2-2.6M10.7 5.1A15 15 0 0 1 22 8.8M5 12.5a10 10 0 0 1 4.3-2.3M15.5 10.6A10 10 0 0 1 19 12.5M8.5 16a5 5 0 0 1 7 0M12 20h.01" />
      <path d="M3 3l18 18" />
    </>
  ),
  refresh: () => <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />,
  info: () => (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v5M12 8h.01" />
    </>
  ),
};

export type IconName = "back" | "forward" | "plus" | "close" | "gear" | "lock" | "monitor" | "globe" | "needs" | "working" | "done" | "idle" | "stopped" | "alert" | "offline" | "info" | "refresh";

export function Icon(props: { name: IconName; size?: number; class?: string }): JSX.Element {
  return (
    <svg class={props.class ? `ic ${props.class}` : "ic"} viewBox="0 0 24 24" width={props.size ?? 22} height={props.size ?? 22} aria-hidden="true" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      {PATHS[props.name]()}
    </svg>
  );
}
