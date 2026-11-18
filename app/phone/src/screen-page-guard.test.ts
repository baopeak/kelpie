import { describe, expect, it } from "vitest";
import { holdPageStill } from "./screen-page-guard";

/** An event target that keeps what is added and removed, with the options given. */
function target() {
  const added: { type: string; fn: (e: Event) => void; opts: AddEventListenerOptions }[] = [];
  const removed: string[] = [];
  return {
    added,
    removed,
    addEventListener: (type: string, fn: EventListenerOrEventListenerObject | null, opts?: boolean | AddEventListenerOptions) => {
      added.push({ type, fn: fn as (e: Event) => void, opts: opts as AddEventListenerOptions });
    },
    removeEventListener: (type: string) => {
      removed.push(type);
    },
  };
}

const event = (touches: number, cancelable = true) => {
  const e = { cancelable, touches: { length: touches }, prevented: false, preventDefault: () => (e.prevented = true) };
  return e;
};
const fire = (t: ReturnType<typeof target>, type: string, e: ReturnType<typeof event>) => t.added.filter((a) => a.type === type).forEach((a) => a.fn(e as unknown as Event));

describe("the page held still while the screen is open", () => {
  it("listens on the picture and the document, never passively, and lets go of all of it", () => {
    const box = target();
    const doc = target();
    const undo = holdPageStill(box, doc);
    expect(box.added.map((a) => a.type).sort()).toEqual(["touchmove", "touchstart"]);
    expect(doc.added.map((a) => a.type).sort()).toEqual(["gesturechange", "gestureend", "gesturestart", "touchmove"]);
    // A passive listener could not cancel the page's scroll or pinch (and the document's touch listeners are passive unless told).
    expect([...box.added, ...doc.added].every((a) => a.opts?.passive === false)).toBe(true);
    undo();
    expect(box.removed.sort()).toEqual(["touchmove", "touchstart"]);
    expect(doc.removed.sort()).toEqual(["gesturechange", "gestureend", "gesturestart", "touchmove"]);
  });

  it("cancels a finger moving on the picture, a pinch anywhere and Safari's own pinch; leaves one finger elsewhere alone", () => {
    const box = target();
    const doc = target();
    holdPageStill(box, doc);
    const slide = event(1);
    fire(box, "touchmove", slide);
    expect(slide.prevented, "one finger on the picture never scrolls the page").toBe(true);
    const land = event(1);
    fire(box, "touchstart", land);
    expect(land.prevented, "one finger landing is left alone (focus, the browser's own tap)").toBe(false);
    const second = event(2);
    fire(box, "touchstart", second);
    expect(second.prevented, "a second finger never starts the page's pinch").toBe(true);
    const keyBar = event(1);
    fire(doc, "touchmove", keyBar);
    expect(keyBar.prevented, "the key bar still scrolls").toBe(false);
    const pinch = event(2);
    fire(doc, "touchmove", pinch);
    expect(pinch.prevented).toBe(true);
    for (const type of ["gesturestart", "gesturechange", "gestureend"]) {
      const g = event(0);
      fire(doc, type, g);
      expect(g.prevented, type).toBe(true);
    }
    // A page Safari zoomed before the screen opened can be pinched back out (outside the picture); the picture itself still never lets it.
    let scale = 2;
    const zb = target();
    const zd = target();
    holdPageStill(zb, zd, () => scale);
    const out = event(2);
    fire(zd, "touchmove", out);
    const outGesture = event(0);
    fire(zd, "gesturestart", outGesture);
    expect([out.prevented, outGesture.prevented]).toEqual([false, false]);
    const onPicture = event(2);
    fire(zb, "touchstart", onPicture);
    expect(onPicture.prevented).toBe(true);
    scale = 1;
    const again = event(0);
    fire(zd, "gesturestart", again);
    expect(again.prevented, "back at its own size: held still again").toBe(true);
    const late = event(1, false);
    fire(box, "touchmove", late);
    expect(late.prevented, "a move the browser already scrolls with cannot be cancelled").toBe(false);
  });
});
