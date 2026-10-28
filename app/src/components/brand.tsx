// Kelpie logo and mascot artwork (docs/brand.md). Each logo file is drawn for one ground, so
// both variants are rendered and CSS (`.on-dark` / `.on-light`) hides the one the theme does not use.
import { Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import markOnDark from "../assets/brand/kelpie-mark-dark.svg";
import markOnLight from "../assets/brand/kelpie-mark.svg";
import wordOnDark from "../assets/brand/kelpie-wordmark-light.svg";
import wordOnLight from "../assets/brand/kelpie-wordmark.svg";
import kipPeek from "../assets/brand/kip-peek.svg?raw";
import kipWorking from "../assets/brand/kip-working.svg?raw";
import kipNeeds from "../assets/brand/kip-needs.svg?raw";
import kipDone from "../assets/brand/kip-done.svg?raw";
import kipSleep from "../assets/brand/kip-sleep.svg?raw";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import "./kip.css";

/** The mark: two kelpie ears behind a `>_` terminal window. Decorative next to the wordmark. */
export function BrandMark() {
  return (
    <>
      <img class="brand-logo on-dark" src={markOnDark} alt="" draggable={false} />
      <img class="brand-logo on-light" src={markOnLight} alt="" draggable={false} />
    </>
  );
}

/** "kelpie" set in the brand typeface (outlined, so it needs no font). */
export function BrandWordmark(props: { height?: number }) {
  const style = () => (props.height ? { height: `${props.height}px` } : undefined);
  return (
    <>
      <img class="brand-word on-dark" src={wordOnDark} alt="Kelpie" style={style()} draggable={false} />
      <img class="brand-word on-light" src={wordOnLight} alt="Kelpie" style={style()} draggable={false} />
    </>
  );
}

// ---------- Kip, the mascot ----------

export type KipMood = "peek" | "working" | "needs" | "done" | "sleep";

/** Inline SVG (so CSS can move its ears and eyes), decorative: the text beside Kip says what
 *  its mood means, so the file's own title and label are dropped. */
const decorative = (svg: string) => svg.replace(/<title>[^<]*<\/title>\s*/, "").replace(/ role="img" aria-label="[^"]*"/, ' aria-hidden="true" focusable="false"');
const KIP: Record<KipMood, string> = { peek: decorative(kipPeek), working: decorative(kipWorking), needs: decorative(kipNeeds), done: decorative(kipDone), sleep: decorative(kipSleep) };

/** Whether Kip is shown at all (Settings → Appearance). */
export const kipShown = () => store.state.settings.mascot !== false;

// Motion runs only while the window is visible: a hidden window has no Kip animations and no
// micro-action timers at all.
const [pageVisible, setPageVisible] = createSignal(!document.hidden);
document.addEventListener("visibilitychange", () => setPageVisible(!document.hidden));
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
const [osAllowsMotion, setOsAllowsMotion] = createSignal(!reducedMotion.matches);
reducedMotion.addEventListener("change", () => setOsAllowsMotion(!reducedMotion.matches));
const moving = () => {
  const m = store.state.settings.mascotMotion ?? "system";
  return pageVisible() && (m === "on" || (m === "system" && osAllowsMotion()));
};
// Gestures only while Kelpie is the window in use: when you work in another app, Kip waits.
const [windowFocused, setWindowFocused] = createSignal(document.hasFocus());
window.addEventListener("focus", () => setWindowFocused(true));
window.addEventListener("blur", () => setWindowFocused(false));

type Gesture = "blink" | "twitch" | "bob" | "nudge" | "hop" | "breathe";

/** How long each gesture's animation runs (kip.css), in ms. */
const GESTURE_MS: Record<Gesture, number> = { blink: 200, twitch: 500, bob: 1000, nudge: 900, hop: 1000, breathe: 2400 };

/** What each mood does, and how often: a pause of `every` ms (min, max) between gestures. */
const GESTURES: Record<KipMood, { every: [number, number]; pick: () => Gesture }> = {
  peek: { every: [7000, 14000], pick: () => (Math.random() < 0.7 ? "blink" : "twitch") },
  working: { every: [5000, 8000], pick: () => (Math.random() < 0.6 ? "bob" : "blink") },
  needs: { every: [3000, 4500], pick: () => "nudge" },
  done: { every: [8000, 12000], pick: () => "hop" },
  sleep: { every: [9000, 14000], pick: () => "breathe" },
};

/**
 * Kip in one of the agent states. Light by design: nothing loops. Every movement is a short,
 * one-shot gesture (a blink, a bob, a nudge…) that a single timer adds every few seconds, so
 * between gestures the page has nothing to redraw. A looping animation, however small, makes
 * the webview redraw the window every frame (measured: ~18% of a core while agents work).
 */
export function Kip(props: { mood?: KipMood; size?: number; interactive?: boolean }) {
  const mood = () => props.mood ?? "peek";
  const [gesture, setGesture] = createSignal<Gesture | "">("");
  // The timer exists only while Kip moves and Kelpie has the focus; motion off, a hidden or a
  // background window clears it. A new mood starts its own rhythm.
  createEffect(() => {
    if (!moving() || !windowFocused()) return;
    const { every, pick } = GESTURES[mood()];
    let timer = 0;
    const next = () => {
      timer = window.setTimeout(() => {
        const g = pick();
        setGesture(g);
        timer = window.setTimeout(() => {
          setGesture("");
          next();
        }, GESTURE_MS[g]);
      }, every[0] + Math.random() * (every[1] - every[0]));
    };
    next();
    onCleanup(() => {
      window.clearTimeout(timer);
      setGesture("");
    });
  });
  return (
    <span
      class={`kip kip-${mood()}${moving() ? " moving" : ""}${gesture() ? ` ${gesture()}` : ""}${props.interactive ? " interactive" : ""}`}
      style={{ width: `${props.size ?? 168}px` }}
      aria-hidden="true"
    >
      {/* Keyed: a new mood is a new element, so its entry animation (a pop or a hop) plays once.
          Gestures animate the inner element, so ending one never restarts the entry. */}
      <Show when={mood()} keyed>
        {(m) => (
          <span class="kip-body">
            <span class="kip-act" innerHTML={KIP[m]} />
          </span>
        )}
      </Show>
    </span>
  );
}

/** After this long without agent activity, Kip naps. */
const NAP_AFTER_MS = 10 * 60 * 1000;

/**
 * Kip in the sidebar footer, peeking over its edge: it shows how the agents are doing (one
 * needs you, they are working, one finished, all quiet, or a nap after a quiet while) and
 * takes you to the agent it is reacting to.
 */
export function KipCompanion() {
  const summary = createMemo(() => {
    let working = 0, waiting = 0, done = 0;
    let firstWaiting: string | null = null, firstDone: string | null = null;
    for (const s of Object.values(store.sessions)) {
      if (!s.alive) continue;
      if (s.status === "waiting") {
        waiting++;
        firstWaiting ??= s.id;
      } else if (s.status === "working") working++;
      else if (s.unread) {
        done++;
        firstDone ??= s.id;
      }
    }
    return { working, waiting, done, firstWaiting, firstDone };
  });
  const busy = () => summary().working + summary().waiting + summary().done > 0;
  // The nap clock: one tick a minute, and only while everything is quiet.
  const [quietSince, setQuietSince] = createSignal(Date.now());
  const [now, setNow] = createSignal(Date.now());
  createEffect(() => {
    if (busy()) {
      setQuietSince(Date.now());
      return;
    }
    const tick = window.setInterval(() => setNow(Date.now()), 60_000);
    onCleanup(() => window.clearInterval(tick));
  });

  const mood = createMemo<KipMood>(() => {
    const s = summary();
    if (s.waiting) return "needs";
    if (s.working) return "working";
    if (s.done) return "done";
    return now() - quietSince() > NAP_AFTER_MS ? "sleep" : "peek";
  });
  const label = () => {
    const s = summary();
    const parts = [
      s.waiting && t("{n} need you", { n: s.waiting }),
      s.working && t("{n} working", { n: s.working }),
      s.done && t("{n} finished", { n: s.done }),
    ].filter(Boolean) as string[];
    if (!parts.length) return mood() === "sleep" ? t("All quiet. Kip is napping.") : t("All quiet");
    const go = s.firstWaiting ? t("Open the agent that needs you") : s.firstDone ? t("Open the agent that finished") : t("Open the agent dashboard");
    return `${parts.join(" · ")}. ${go}`;
  };
  const open = () => {
    const id = summary().firstWaiting ?? summary().firstDone;
    if (id) store.revealSession(id);
    else store.setOverlay("dashboard");
  };
  return (
    <button class="kip-companion" title={label()} aria-label={label()} onClick={open}>
      <Kip mood={mood()} size={40} interactive />
    </button>
  );
}
