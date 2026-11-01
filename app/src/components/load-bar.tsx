// A thin bar along the top edge of a view, dialog or panel while it loads or waits on work, so a
// slow read reads as "running", not "frozen". It costs nearly nothing: it only exists while work
// is under way, shows after a short delay (quick work never flashes it), runs on the compositor
// (transform only) and stops while the window is hidden or another app has the focus.
import { Show } from "solid-js";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { t } from "../lib/i18n";
import "./load-bar.css";

// A looping animation makes the webview redraw every frame (docs/brand.md), so the bars pause
// whenever the user is not looking at Kelpie. Focus is the window's (the OS's), not the
// document's: clicking into a browser pane's page moves focus to another webview of the same
// window, and that pane's own bar must keep running.
const root = document.documentElement;
let windowFocused = document.hasFocus();
const syncFocus = () => root.toggleAttribute("data-unfocused", !windowFocused || document.hidden);
document.addEventListener("visibilitychange", syncFocus);
try {
  void getCurrentWebview().window.onFocusChanged(({ payload }) => { windowFocused = payload; syncFocus(); }).catch(() => {});
} catch {
  // Not in the desktop shell (Vite alone): only visibility pauses the bars.
}
syncFocus();

/** `when`: work is under way. `value` (0..1): known progress, drawn as a filling bar; without it
 *  the bar sweeps. `edge="bottom"` hangs it under a toolbar. The container must be positioned. */
export function LoadBar(props: { when: unknown; value?: number | null; edge?: "top" | "bottom"; label?: string }) {
  const known = () => props.value != null;
  return (
    <Show when={props.when}>
      <div class="load-bar" classList={{ known: known(), bottom: props.edge === "bottom" }} role="progressbar"
        aria-label={props.label ?? t("Loading…")} aria-valuemin={0} aria-valuemax={100}
        aria-valuenow={known() ? Math.round(props.value! * 100) : undefined}>
        <span style={known() ? { transform: `scaleX(${Math.min(1, Math.max(0, props.value!))})` } : undefined} />
      </div>
    </Show>
  );
}
