// Quick hints for icon buttons. The webview's native `title` tooltip comes late (about a second),
// small and at the pointer, so an icon-only button reads as unexplained. Inside a container marked
// `data-hints`, hovering anything with a `title` shows that text in a styled hint under it after a
// short delay, with a trailing "(Ctrl+…)" drawn as a key. Moving to the next button while one is
// up shows the next at once. Elements keep their `title` (tests and screen readers rely on it):
// it is only blanked while the hint is up, so the native tooltip (or an ancestor's) stays away.
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { splitHint } from "../lib/hover-hint";

const DELAY_MS = 350;
/** After a hint closes, the next one within this window opens without the delay. */
const WARM_MS = 600;
const GAP = 6;

type Hint = { label: string; keys: string; left: number; top: number; below: boolean };

export function HoverHints() {
  const [hint, setHint] = createSignal<Hint | null>(null);
  let el: HTMLDivElement | undefined;
  let target: HTMLElement | null = null;
  let timer = 0;
  let closedAt = 0;

  const restore = () => {
    if (target?.dataset.hintTitle != null) {
      target.setAttribute("title", target.dataset.hintTitle);
      delete target.dataset.hintTitle;
    }
  };
  const hide = () => {
    clearTimeout(timer);
    if (hint()) closedAt = Date.now();
    restore();
    target = null;
    setHint(null);
  };
  const show = (t: HTMLElement) => {
    const text = t.getAttribute("title");
    if (!text || !t.isConnected) return;
    if (!t.hasAttribute("aria-label")) t.setAttribute("aria-label", text);
    t.dataset.hintTitle = text;
    t.setAttribute("title", "");
    const r = t.getBoundingClientRect();
    // Under the button unless that runs off the window; centred, kept inside it (see place()).
    const below = r.bottom + GAP + 32 < window.innerHeight;
    setHint({ ...splitHint(text), left: r.left + r.width / 2, top: below ? r.bottom + GAP : r.top - GAP, below });
    queueMicrotask(place);
  };
  /** Keeps the hint inside the window horizontally once its width is known. */
  const place = () => {
    const h = hint();
    if (!h || !el) return;
    const w = el.offsetWidth;
    const left = Math.min(Math.max(h.left, w / 2 + 4), window.innerWidth - w / 2 - 4);
    if (left !== h.left) setHint({ ...h, left });
  };

  const over = (e: PointerEvent) => {
    // Moving onto the icon inside the hinted button is still the same button.
    if (target?.contains(e.target as Node | null)) return;
    const t = (e.target as Element | null)?.closest<HTMLElement>("[data-hints] [title]:not([title=''])");
    if (!t || t === target) return;
    hide();
    target = t;
    if (Date.now() - closedAt < WARM_MS) show(t);
    else timer = window.setTimeout(() => target === t && show(t), DELAY_MS);
  };
  const out = (e: PointerEvent) => {
    if (target && !target.contains(e.relatedTarget as Node | null)) hide();
  };

  onMount(() => {
    document.addEventListener("pointerover", over);
    document.addEventListener("pointerout", out);
    // A click, a key, a scroll or the window losing focus ends the hint.
    for (const ev of ["pointerdown", "keydown", "wheel"] as const) document.addEventListener(ev, hide, true);
    window.addEventListener("blur", hide);
  });
  onCleanup(() => {
    hide();
    document.removeEventListener("pointerover", over);
    document.removeEventListener("pointerout", out);
    for (const ev of ["pointerdown", "keydown", "wheel"] as const) document.removeEventListener(ev, hide, true);
    window.removeEventListener("blur", hide);
  });

  return (
    <Show when={hint()}>
      {(h) => (
        <div ref={el} class="hover-hint" classList={{ above: !h().below }} role="tooltip" style={{ left: `${h().left}px`, top: `${h().top}px` }}>
          <span>{h().label}</span>
          <Show when={h().keys}><kbd>{h().keys}</kbd></Show>
        </div>
      )}
    </Show>
  );
}
