// While the computer's screen is open, the page itself never zooms or scrolls: only the app's own view does, with the app's own pinch
// (screen.ts `Gestures`). `touch-action: none` on the picture is all Chromium needs; Safari on the iPhone ignores `user-scalable=no`, may
// still start its own pinch from a finger that landed outside the picture (the header, the key bar), and a page it zoomed then pans under a
// one-finger drag instead of the cursor moving. So the touches that would scroll or zoom the page are cancelled here as well, with
// listeners that are never passive (a passive listener cannot cancel anything, and document-level touch listeners are passive by default).

type Listens = Pick<EventTarget, "addEventListener" | "removeEventListener">;

const cancel = (e: Event) => {
  if (e.cancelable) e.preventDefault();
};

/** Cancels the page's own scroll and zoom while the screen is shown; returns what undoes it. On the picture: every one-finger move (never
 *  a page or `main` scroll) and a second finger landing (never the page's pinch). Anywhere: a two-finger move, and Safari's gesture events,
 *  which are its pinch. A single touch elsewhere (the key bar's own scroll, the help sheet) is left alone. `pageScale` is the page's own
 *  zoom (`visualViewport.scale`): a page already zoomed before the screen opened can still be pinched back out, outside the picture. */
export function holdPageStill(box: Listens, doc: Listens, pageScale: () => number = () => 1): () => void {
  const zoomed = () => pageScale() > 1.01;
  /** Two fingers or more: a pinch the page would take. */
  const pinch = (e: Event) => {
    if (((e as TouchEvent).touches?.length ?? 0) > 1) cancel(e);
  };
  const opts: AddEventListenerOptions = { passive: false };
  const on: [Listens, string, (e: Event) => void][] = [
    [box, "touchstart", pinch],
    [box, "touchmove", cancel],
    [doc, "touchmove", (e) => !zoomed() && pinch(e)],
    [doc, "gesturestart", (e) => !zoomed() && cancel(e)],
    [doc, "gesturechange", (e) => !zoomed() && cancel(e)],
    [doc, "gestureend", (e) => !zoomed() && cancel(e)],
  ];
  for (const [target, type, fn] of on) target.addEventListener(type, fn, opts);
  return () => {
    for (const [target, type, fn] of on) target.removeEventListener(type, fn, opts);
  };
}
