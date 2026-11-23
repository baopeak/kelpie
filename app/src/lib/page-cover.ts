import type { Overlay } from "./store";

// Pages that fill the whole window with an opaque background: the sidebar, workspace and status
// bar stay mounted underneath (terminals keep running) but cannot be seen or used meanwhile.
const FULL_PAGES: ReadonlySet<Overlay> = new Set<Overlay>(["dashboard", "settings", "board", "runs", "hosts", "cost"]);

/** True while a full-window page hides the main view, which must then leave the accessibility tree and the tab order. */
export function mainViewCovered(overlay: Overlay): boolean {
  return FULL_PAGES.has(overlay);
}
