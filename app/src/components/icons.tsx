// Small inline SVG icon set (stroke icons, 16px grid) — no icon font, no network.
import { createUniqueId, For, Show, type JSX } from "solid-js";
import { AGENT_LOGOS, GEMINI_WASHES } from "../lib/agent-logos";
import { kindInfo } from "../lib/agents";

// Factories, not elements: a Solid JSX element is a real DOM node and can live in one place only.
const paths: Record<string, () => JSX.Element> = {
  plus: () => <path d="M8 3v10M3 8h10" />,
  mic: () => <><rect x="6" y="2" width="4" height="7.5" rx="2" /><path d="M3.8 7.5a4.2 4.2 0 0 0 8.4 0M8 11.7V14M5.8 14h4.4" /></>,
  close: () => <path d="M4 4l8 8M12 4l-8 8" />,
  terminal: () => <><path d="M3 4.5l3.5 3.5L3 11.5" /><path d="M8.5 12h4.5" /></>,
  files: () => <><path d="M3 2.5h6l3 3v8H3z" /><path d="M9 2.5v3h3" /></>,
  git: () => <><circle cx="4.5" cy="4" r="1.5" /><circle cx="4.5" cy="12" r="1.5" /><circle cx="11.5" cy="6" r="1.5" /><path d="M4.5 5.5v5M11.5 7.5c0 2.5-3 2.5-6.2 3.6" /></>,
  search: () => <><circle cx="7" cy="7" r="4" /><path d="M10 10l3.5 3.5" /></>,
  dashboard: () => <><rect x="2.5" y="2.5" width="4.5" height="5.5" rx="1" /><rect x="9" y="2.5" width="4.5" height="3.5" rx="1" /><rect x="9" y="8" width="4.5" height="5.5" rx="1" /><rect x="2.5" y="10" width="4.5" height="3.5" rx="1" /></>,
  settings: () => <><circle cx="8" cy="8" r="2" /><path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6L5 5M11 11l1.4 1.4M3.6 12.4L5 11M11 5l1.4-1.4" /></>,
  splitRight: () => <><rect x="2.5" y="3" width="11" height="10" rx="1.5" /><path d="M8 3v10" /></>,
  splitDown: () => <><rect x="2.5" y="3" width="11" height="10" rx="1.5" /><path d="M2.5 8h11" /></>,
  chevronRight: () => <path d="M6 4l4 4-4 4" />,
  chevronDown: () => <path d="M4 6l4 4 4-4" />,
  folder: () => <path d="M2 4.5h4l1.5 1.5H14v6.5H2z" />,
  sidebar: () => <><rect x="2.5" y="3" width="11" height="10" rx="1.5" /><path d="M6 3v10" /></>,
  refresh: () => <><path d="M13 8a5 5 0 1 1-1.5-3.6" /><path d="M13 2.5v3h-3" /></>,
  chat: () => <path d="M3 3.5h10v7H7l-3 2.5v-2.5H3z" />,
  check: () => <path d="M3.5 8.5l3 3 6-7" />,
  alert: () => <path d="M8 3.5v5.2M8 12.3v.2" />,
  minus: () => <path d="M4 8h8" />,
  undo: () => <><path d="M5 6.5H10a3 3 0 0 1 0 6H6" /><path d="M7 4L4.5 6.5 7 9" /></>,
  restart: () => <><path d="M3 8a5 5 0 1 0 1.5-3.6" /><path d="M3 2.5v3h3" /></>,
  stop: () => <rect x="4" y="4" width="8" height="8" rx="1.5" />,
  more: () => <><circle cx="4" cy="8" r=".8" /><circle cx="8" cy="8" r=".8" /><circle cx="12" cy="8" r=".8" /></>,
  up: () => <path d="M4 10l4-4 4 4" />,
  down: () => <path d="M4 6l4 4 4-4" />,
  maximize: () => <path d="M3 6V3h3M10 3h3v3M13 10v3h-3M6 13H3v-3" />,
  minimize: () => <path d="M6 3v3H3M10 3v3h3M13 10h-3v3M3 10h3v3" />,
  pin: () => <><path d="M6 2.5h4l-.5 4 2 2v1.5h-7V8.5l2-2z" /><path d="M8 10v4" /></>,
  copy: () => <><rect x="5.5" y="5.5" width="8" height="8" rx="1.5" /><path d="M3.5 10.5h-1v-8h8v1" /></>,
  bookmark: () => <path d="M4.5 2.5h7v11L8 10.8l-3.5 2.7z" />,
  bookmarkFilled: () => <path d="M4.5 2.5h7v11L8 10.8l-3.5 2.7z" fill="currentColor" />,
  history: () => <><path d="M3 8a5 5 0 1 0 1.5-3.6" /><path d="M3 2.5v3h3" /><path d="M8 5.5V8l2 1.5" /></>,
  back: () => <path d="M9.5 4L5.5 8l4 4" />,
  forward: () => <path d="M6.5 4l4 4-4 4" />,
  bolt: () => <path d="M9 2L4 9h4l-1 5 5-7H8z" />,
  board: () => <><rect x="2.5" y="3" width="3" height="10" rx="1" /><rect x="6.5" y="3" width="3" height="7" rx="1" /><rect x="10.5" y="3" width="3" height="5" rx="1" /></>,
  branch: () => <><circle cx="5" cy="4" r="1.5" /><circle cx="5" cy="12" r="1.5" /><circle cx="11" cy="5" r="1.5" /><path d="M5 5.5v5M11 6.5c0 3-6 2-6 4" /></>,
  filter: () => <path d="M2.5 3.5h11l-4.5 5v4l-2 1v-5z" />,
  sparkle: () => <><path d="M8 2.5l1.3 3.2L12.5 7l-3.2 1.3L8 11.5 6.7 8.3 3.5 7l3.2-1.3z" /><path d="M12.5 11v3M11 12.5h3" /></>,
  bell: () => <><path d="M4.5 11V7.5a3.5 3.5 0 0 1 7 0V11l1 1.5h-9z" /><path d="M7 14h2" /></>,
  coffee: () => <><path d="M3 6h8v4a3 3 0 0 1-3 3H6a3 3 0 0 1-3-3z" /><path d="M11 7h1a1.5 1.5 0 0 1 0 3h-1" /><path d="M5.5 2.5v1.5M8 2v2" /></>,
  pr: () => <><circle cx="4.5" cy="4" r="1.5" /><circle cx="4.5" cy="12" r="1.5" /><circle cx="11.5" cy="12" r="1.5" /><path d="M4.5 5.5v5M11.5 10.5V6.5a2 2 0 0 0-2-2H7.5" /><path d="M9 3l-1.5 1.5L9 6" /></>,
  push: () => <><path d="M8 11V3M4.5 6.5L8 3l3.5 3.5" /><path d="M3 13h10" /></>,
  pull: () => <><path d="M8 3v8M4.5 7.5L8 11l3.5-3.5" /><path d="M3 13h10" /></>,
  sync: () => <><path d="M12.5 6.5A4.5 4.5 0 0 0 4 5" /><path d="M3.5 9.5A4.5 4.5 0 0 0 12 11" /><path d="M4 2.5V5h2.5M12 13.5V11H9.5" /></>,
  compose: () => <><path d="M3 13l.8-3.2L10.6 3l2.4 2.4-6.8 6.8z" /><path d="M9.4 4.2l2.4 2.4" /></>,
  note: () => <><path d="M3 3h10v7l-3 3H3z" /><path d="M10 13v-3h3M5.5 6h5M5.5 8.5h3" /></>,
  moon: () => <path d="M12.5 10A5 5 0 0 1 6 3.5 5 5 0 1 0 12.5 10z" />,
  external: () => <><path d="M9 3h4v4M13 3L7.5 8.5" /><path d="M11 9.5V13H3V5h3.5" /></>,
  float: () => <><rect x="2.5" y="3" width="11" height="10" rx="1.5" /><rect x="7.5" y="8" width="5" height="4" rx="1" /></>,
  clock: () => <><circle cx="8" cy="8" r="5.5" /><path d="M8 5v3.2l2 1.3" /></>,
  grid: () => <><rect x="2.5" y="3" width="11" height="10" rx="1.5" /><path d="M8 3v10M2.5 8h11" /></>,
  globe: () => <><circle cx="8" cy="8" r="5.5" /><path d="M2.5 8h11M8 2.5c1.7 1.8 2.5 3.6 2.5 5.5S9.7 11.7 8 13.5M8 2.5C6.3 4.3 5.5 6.1 5.5 8s.8 3.7 2.5 5.5" /></>,
  target: () => <><circle cx="8" cy="8" r="5" /><circle cx="8" cy="8" r="1.5" /><path d="M8 1.5v2.5M8 12v2.5M1.5 8H4M12 8h2.5" /></>,
  camera: () => <><path d="M2.5 5.5h2.5L6.2 4h3.6L11 5.5h2.5v7h-11z" /><circle cx="8" cy="8.8" r="2.2" /></>,
  code: () => <path d="M6 4.5L2.5 8 6 11.5M10 4.5L13.5 8 10 11.5" />,
  server: () => <><rect x="2.5" y="2.5" width="11" height="4.5" rx="1" /><rect x="2.5" y="9" width="11" height="4.5" rx="1" /><path d="M5 4.75h.01M5 11.25h.01" /></>,
};

export function Icon(props: { name: keyof typeof paths | string; size?: number }) {
  const s = () => props.size ?? 16;
  return (
    <svg width={s()} height={s()} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">
      {paths[props.name]?.()}
    </svg>
  );
}

/** An agent's official mark, unaltered, on a neutral tile; kinds without one get a lettered colour tile. */
export function KindGlyph(props: { kind: string; size?: number }) {
  const meta = () => kindInfo(props.kind);
  const logo = () => AGENT_LOGOS[props.kind];
  const size = () => props.size ?? 18;
  return (
    <Show
      when={logo()}
      fallback={
        <span
          class="kind-glyph"
          style={{ "--kc": meta().color, width: `${size()}px`, height: `${size()}px`, "font-size": `${Math.round(size() * 0.5)}px` }}
        >
          {meta().glyph}
        </span>
      }
    >
      {(l) => (
        <span class="kind-glyph logo" role="img" aria-label={l().title} style={{ width: `${size()}px`, height: `${size()}px` }}>
          <svg viewBox="0 0 24 24" width="66%" height="66%" fill={l().fill ?? "currentColor"} fill-rule="evenodd" aria-hidden="true">
            <Show when={l().gemini} fallback={<path d={l().path} />}>
              <GeminiMark path={l().path} />
            </Show>
          </svg>
        </span>
      )}
    </Show>
  );
}

// The washes need gradient ids unique to this instance: duplicate ids resolve to the first in the
// document, which renders nothing once that copy sits in a hidden subtree.
function GeminiMark(props: { path: string }) {
  const id = createUniqueId();
  return (
    <>
      <defs>
        <For each={GEMINI_WASHES}>
          {([color, x1, y1, x2, y2, end], i) => (
            <linearGradient id={`gw${id}-${i()}`} gradientUnits="userSpaceOnUse" x1={x1} y1={y1} x2={x2} y2={y2}>
              <stop stop-color={color} />
              <stop offset={end} stop-color={color} stop-opacity="0" />
            </linearGradient>
          )}
        </For>
      </defs>
      <path d={props.path} fill="#3186FF" />
      <For each={GEMINI_WASHES}>{(_, i) => <path d={props.path} fill={`url(#gw${id}-${i()})`} />}</For>
    </>
  );
}

