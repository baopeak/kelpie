// Which session's composer is open, and where `@selection` comes from. Kept apart from the
// composer itself (components/composer.tsx, loaded the first time it opens) so the panes and the
// menus that open it stay in the main bundle at a few lines.
import { createSignal } from "solid-js";
import { store } from "./store";
import type { Selection } from "./mentions";

export interface ComposerRequest {
  sessionId: string;
  /** Text the composer starts with (a prompt from the history, `@selection `). */
  draft?: string;
  /** The selection `@selection` stands for, when it was taken before the composer opened. */
  selection?: Selection;
}

const [request, setRequest] = createSignal<ComposerRequest | null>(null);
export const composerRequest = request;

export function openComposer(sessionId: string, opts: Omit<ComposerRequest, "sessionId"> = {}) {
  store.revealSession(sessionId);
  setRequest({ sessionId, ...opts });
}

export const closeComposer = () => setRequest(null);

/** The pane toolbar's Compose button and Ctrl+Shift+I: open this session's composer, or close it. */
export function toggleComposer(sessionId: string) {
  if (request()?.sessionId === sessionId) closeComposer();
  else openComposer(sessionId);
}

// ---------- selections ----------

interface Source {
  get: () => Selection | null;
}
const sources = new Set<Source>();
let latest: Source | null = null;

/** An editor, diff or terminal registers how to read its selection; `changed()` marks it as
 *  the one the user selected in last. */
export function selectionSource(get: () => Selection | null) {
  const src: Source = { get };
  sources.add(src);
  return {
    changed: () => (latest = src),
    dispose: () => {
      sources.delete(src);
      if (latest === src) latest = null;
    },
  };
}

/** The selection last made in a pane, else any pane's. */
export function currentSelection(): Selection | null {
  const usable = (s: Selection | null | undefined) => (s?.text.trim() ? s : null);
  const last = latest && usable(latest.get());
  if (last) return last;
  for (const src of sources) {
    const s = usable(src.get());
    if (s) return s;
  }
  return null;
}
