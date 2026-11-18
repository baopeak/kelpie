# Phone remote screen: zoomed drag moves the cursor, not the screen

Owner report (v0.1.22, real iPhone, Safari, 2026-11-18): zoomed in, a one-finger drag moved the whole screen instead of the cursor.

## Root cause (evidence)

1. **Direct touch, zoomed (reproduced, Chrome + Chromium).** A quick one-finger slide in direct mode was a *scroll of the computer* at any
   zoom (`Gestures.move` → `scroll1` → wheel). Master in installed Chrome: zoomed direct slide sent `wheel:-120` and left the cursor ~33 px
   from the finger: "the screen moves, the cursor does not".
2. **Two fingers, zoomed (reproduced).** Always scrolled the computer (`wheel:-120` in the master run); no way to move the view on purpose.
3. **iOS page zoom (not reproducible here, by code inspection).** The page relied only on CSS `touch-action: none` on the picture: no
   non-passive `touchmove`/`gesturestart` cancel anywhere, the picture inside a scrolling `<main>`. iOS ignores `user-scalable=no`; a pinch
   that starts or spreads outside the picture zooms the page, and a one-finger drag then pans the page (pointercancel, cursor still) —
   exactly the report in trackpad mode. Chromium honours touch-action, so no emulator shows it; master's e2e check of the guards: `[False, False]`.
4. **Trackpad follow (by math).** `keepInView` pinned the cursor 40 px from the edge and snapped the view when the cursor started inside
   that band: the view jumped and then scrolled under a cursor stuck near the edge.

## Fix

- `screen-page-guard.ts`: non-passive listeners cancel page scroll/pinch while the screen is open (picture: any move, a 2nd finger; document:
  2-finger moves, Safari `gesture*`; skipped while the page is already zoomed so it can be pinched back out). CSS `overscroll-behavior: none`.
- `followView`/`followMargin`: trackpad cursor moves freely in an inner area (1/5 of the box's smaller side, 40–120 px); the view pans only
  by how far the cursor goes past it, outward only — no jumps.
- `cursorIntoView` + `bringIn`: a trackpad cursor out of the picture shown is brought into view at the next slide; a tap / hold /
  two-finger tap then only brings it back (no click at a cursor nobody sees — review finding B1).
- Direct, zoomed: one-finger slide moves the cursor under the finger; a finger held ≥250 ms in a 36 px edge band pans the view
  (`Gestures.tick`, rAF while `edgePanning()`); taps never pan.
- Two fingers moving together: zoomed → pan the phone's view (nothing sent); zoom 1 → scroll the computer as before.
- Unchanged owner decisions: instant taps/double click at 350 ms, visible text field + Hide, long-press drag, cursor SVG layer, pinch
  around the cursor.

## Verification

- Vitest: 2208/2208 (11 new zoomed-gesture tests + 2 guard tests); typecheck, i18n:check, `pnpm build`, `pnpm build:phone` green.
- e2e `scripts/e2e/screen_phone.py` (new `zoomed_fingers` section, `--chrome` for installed Chrome), isolated daemon/data, debug app +
  kelpied + kelpie-screen:
  - master page, installed Chrome 157: **57/69, 12 FAIL** (direct slide scrolls, two fingers scroll, no guards, …);
  - fixed page, Playwright Chromium: **69/69**; installed Chrome: 69/69 (one earlier run 30/69 — picture never decoded; one 68/69 — real
    cursor jumped 600 px down during a 30 px right slide: the shared desktop's mouse; both are the known flakiness of this script).
- WebKit: not run — Playwright's WebKit on Windows has no WebCodecs, so no picture and no gestures; and no emulator has iOS page zoom.
- Independent code review: 1 blocking (B1) + 9 non-blocking, all fixed; re-review clean (`code-reviewer-261118-1215-zoom-drag.md`).

## Real-iPhone checks for the owner (after release)

1. Open a screen, pinch on the picture: only the picture zooms (the header and key bar stay their size). Then pinch with one finger on the
   header: nothing zooms.
2. Zoomed, Trackpad: slide one finger slowly toward an edge: the cursor moves, then the picture follows it smoothly, no jump; the page
   never slides.
3. Zoomed, Direct touch: slide a finger: the cursor stays under it; hold at an edge for a moment: the picture moves that way; tap a button
   near the edge: it clicks and the picture stays.
4. Zoomed, two fingers together: the picture moves, the computer does not scroll; Fit, then two fingers: the computer scrolls.

## Open questions

- Scrolling the computer while zoomed now needs Fit (or PgUp/PgDn/arrows). Recommendation: keep it; add an explicit scroll toggle only if
  the owner misses it.
- Direct touch at zoom 1 still scrolls with one finger (only zoomed changes). Recommendation: keep; it matches a touchscreen.
