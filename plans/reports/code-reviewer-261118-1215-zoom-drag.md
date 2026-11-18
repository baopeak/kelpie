# Code review: phone screen zoomed one-finger drag (fix/phone-screen-zoom-drag, uncommitted)

## Scope
- Files: app/phone/src/screen.ts, screen-view.tsx, screen-page-guard.ts (new), screen-page-guard.test.ts (new), screen.test.ts, phone.css,
  app/src/locales/vi/screen.json, docs/user-guide.md, docs/user-guide.vi.md, scripts/e2e/screen_phone.py
- About 490 added and 30 removed lines. The focus was the uncommitted diff plus its callers (ResizeObserver, followHostCursor, up/tap paths).
- Checks run: `pnpm exec vitest run phone/src/screen.test.ts phone/src/screen-page-guard.test.ts` gave 87/87 passing, and `pnpm typecheck`
  is clean. No e2e or real-device run was done (read-only review).

## Overall
The followView/cursorIntoView/tick math is correct: signs, the clamp at the picture's edge, margins larger than half the box, and the depth
clamp when the finger is past the box under pointer capture all check out. The rAF loop stops and is cleaned up correctly. Owner decisions
are intact: instant taps and the 350 ms double click, the text field with Hide, long-press drag, the cursor layer, and pinch around the
cursor. One behavioural regression should be fixed before merge: removing `keepInView` from the pinch pan, together with two fingers now
panning while zoomed, makes an off-screen trackpad cursor routine, and a tap then clicks somewhere the person cannot see.

## BLOCKING

### B1. Trackpad tap, double tap, long press and two-finger tap click at a cursor the person cannot see after a two-finger pan
- screen.ts:759 (zoomed, two fingers moving together now go to `pinch`), screen.ts:766-768 (the pinch pan no longer calls `keepInView`),
  screen.ts:886 (a trackpad tap clicks at `this.cursor`), screen.ts:727 (a trackpad drag starts at `this.cursor`), screen.ts:900 (a
  two-finger tap right-clicks at `this.cursor`).
- `cursorIntoView` runs only on the press to point transition (screen.ts:734-740), which needs a slide. A tap, a long press, a long
  press plus move, or a two-finger tap never runs it.
- Failure scenario: in Trackpad, zoomed x3, the cursor is over a dialog's *Delete* button. Two fingers pan the view to read text elsewhere,
  and the cursor is now outside the box (the `.screen-cursor` layer is clipped by `overflow: hidden`). The person taps to click what they
  are looking at, and the computer clicks *Delete*, unseen. Before this change that path was rare: zoomed two-finger moves scrolled the
  computer, and the trackpad pinch-pan kept the cursor in view (`pinch0.follow`). Now it is the documented way to move the view.
- Fix (keeps instant taps and the click point the person sees): in trackpad mode, bring the cursor in before any press outcome. At the
  start of `up()` for `press`/`two`/`two_up` and at press to drag, compute `at = cursorIntoView(...)`. If it changed, set `this.cursor = at`
  and put `{k:"move"}` before the click or down op in the same batch. The double click's second tap still uses `prev.at`, so it is
  unaffected. A smaller alternative: when a zoomed pan in trackpad ends (the last finger lifts from `pinch`), move the cursor into view
  and send one `move`. That covers this path but not a cursor the computer moved away.
- Add a unit test: zoomed trackpad, two-finger pan that leaves the cursor out, then a tap. The click must be inside the shown picture.

## NON-BLOCKING

### N1. The ResizeObserver still snaps the view back to the cursor, which is now commonly off view on purpose (High)
- screen-view.tsx:375-377 runs `keepInView` on every box resize, not only for the keyboard. The box height changes whenever the lines
  under it rewrap: `.screen-quality` gains *Sharpen* or *software encoder*, `.screen-details` updates with live stats, `SwitchWay`
  appears or disappears.
- Scenario: the person pans away from the cursor with two fingers. A second later the details line wraps to two lines, the box shrinks
  by about 20 px, and the view jumps back to the cursor. The doc promises "the picture never jumps".
- Fix: limit this to keyboard transitions (as in the comment: "where the person is typing"), or apply it only if the cursor was inside
  the shown picture before the resize. Fixing B1 by moving the cursor into view at the end of a pan also makes this mostly harmless.

### N2. `main { overflow: hidden }` can make the composer unreachable on short viewports (Medium)
- phone.css:443. Below the picture, `.screen-live` stacks the head, quality, details, way select and composer (key bar plus text field).
  The picture box has `min-height: 0`, so when those fixed parts exceed `main`'s height (iPhone landscape is about 330 px of viewport
  after the app header, or a large text size), the bottom of the composer used to be reachable by scrolling `main` and is now clipped.
- The box's non-passive `touchmove` cancel (screen-page-guard.ts:24) already stops a finger on the picture from scrolling `main`. Keeping
  `overflow-y: auto` with `overscroll-behavior: none` would keep the fix and leave the composer reachable. If `hidden` stays, check
  landscape on a real iPhone.

### N3. A page that is already zoomed cannot be pinched back out while the screen is open (Low-Medium)
- screen-page-guard.ts:25-28 cancel every multi-touch `touchmove` and every `gesture*` on the document. The viewport meta
  (app/phone/index.html:5) allows page zoom, so the page can be zoomed in the inbox (or by a double tap on non-button text, if iOS
  still does that at `width=device-width`) before the screen opens.
- Scenario: the page is zoomed when the screen opens. The person can no longer pinch the page out; only a one-finger pan outside the
  picture moves the visual viewport. *Stop* may sit off screen, reachable only by panning.
- Options: skip the document-level cancel when `visualViewport.scale > 1.01` (let them pinch back), or document it. Also, the
  document-level non-passive `touchmove` makes every key-bar or help-sheet scroll wait on the main thread, which is busy decoding and
  drawing frames. This is minor jank, worth a look on a slow phone.

### N4. The e2e edge-pan check reads the view before lifting the finger while the rAF loop is still panning (likely flaky)
- scripts/e2e/screen_phone.py:667 samples `v6 = view(phone)`, then sends `touchEnd`. Between the two CDP round trips the edge pan
  continues at up to `0.6 px/ms`, about 8 screen px a frame (about 16 source px at zoom x3 on a 390 px box). The check
  `near(ss.cursor(), pic.under(*at_edge, v6), tol + 4)` allows about 10 source px, so one or two late frames fail it.
- Fix: read `v6` after `touchEnd`. The view stops once `edgePanning()` turns false. The `moved > 5` check works with either value.

### N5. The e2e trackpad short-slide check depends on the cursor-centring loop succeeding
- screen_phone.py:601-607: after three tries the code continues even if the cursor is not centred. If it rests within roughly 30 px
  times the gain of the inner line, the 30 px slide pans and `v2 == v1` fails. Fail loudly or skip when the centring did not converge.

### N6. Vietnamese doc overclaims (Low)
- docs/user-guide.vi.md:252 says "nên con trỏ luôn nằm trong tầm mắt" (the cursor is always in sight). After a two-finger pan it is not
  until the next slide. The English text qualifies this; the Vietnamese does not. Align it with the English (and with whatever B1 does).

### N7. Stale i18n key (Low)
- app/src/locales/vi/screen.json:80 `"Scrolls"` is no longer referenced (both help rows now use the longer strings). `i18n:check` only
  fails on missing keys, so it passes silently. Remove the key.

### N8. `panFrame` is declared after the cleanup that reads it (Low, style)
- screen-view.tsx:403 reads `panFrame`, declared with `let` at line 498. This is safe at runtime because cleanup runs after the body.
  If the body ever threw in between, Solid's disposal would raise a TDZ `ReferenceError` that masks the real error. `holdTimer`
  already follows the same pattern. Declare it next to `drawFrame` (line 210) for clarity.

### N9. The edge-pan rAF loop runs every frame for any direct-mode drag, including at zoom 1 (Low)
- screen-view.tsx:509 together with screen.ts:800-802: `edgePanning()` does not check the zoom, so a direct-mode drag at zoom 1 runs
  `getBoundingClientRect` plus `tick` every frame and returns NONE. This is harmless, but `edgePanning()` could require
  `view.zoom > 1` (pass the frame) to keep the loop idle.

## Verified OK (risk calibration)
- followView: the shift is the part of the move past the inner line, outward only. After `cx += dx/s` the cursor lands exactly on the
  line (b - (b - lo) = lo). The clamp at the picture's edge returns `v` unchanged, so the cursor continues to the edge.
  `min(margin, size/2)` handles small boxes.
- cursorIntoView: it does nothing when shown, and otherwise clamps to the inner area. Integer rounding stays inside the box because the
  margin is at least 40 px.
- tick: `step = speed*dt/s` with `s` = screen px per source px, which is correct (the `scaleOf` doc comment says the inverse, which is
  pre-existing and misleading). The direction is correct, `dt` is capped at 50 ms, there is a 250 ms delay, and `edge` is reset on
  down, on the last up, on cancel, outside the band and when not panning.
- The loop stops: `edgePan` re-checks `edgePanning()` each frame, `up`/`cancel` set the state to idle, and unmount cancels `panFrame`.
  It never starts twice (`!panFrame` guard).
- Taps near an edge stay in `press`, so they never pan. The direct double tap, the two-finger tap right click and long press are
  unchanged.
- The guards do not touch the text field, Hide or Type it (outside `.screen-canvas`), and a one-finger key-bar or help-sheet scroll is
  untouched. preventDefault on touch events does not suppress pointer events (they are dispatched first). The only 2-finger `touchstart`
  cancel is on the box, which holds no buttons.
- The two-finger pan direction is natural (content follows the fingers), and the e2e expectation `v4.cx < v3.cx` for `dx=+80` matches.

## Recommended actions
1. Fix B1 (cursor into view before a trackpad press outcome) and add a test.
2. Limit the ResizeObserver `keepInView` (N1).
3. Restore `overflow-y: auto` on `main` or verify landscape (N2); decide on N3.
4. Sample `v6` after `touchEnd` (N4); fix the vi doc and the stale key (N6, N7).
5. The real iOS Safari page-zoom fix cannot be verified in Chromium (the e2e says so). The owner's iPhone check is still owed.

## Metrics
- Type coverage: strict tsc clean. Tests: 87 passed in the two focused files. Lint: not configured.

## Unresolved questions
- Does iOS Safari still double-tap-zoom non-button text at `width=device-width`? If yes, the page can get zoomed while the screen is open
  and N3 matters more.
- scripts/e2e/screen_phone.py's diffstat changed (119 to 130 lines) between two reads during this review; the content reviewed is the
  12:08 version.

---

## Re-review of the fixes (2026-11-18 12:25)

Checks: `vitest run` on the two files gives 88/88 passing; `pnpm typecheck` is clean. The re-review was read-only and wrote no memory notes.

| Finding | Result |
| --- | --- |
| B1 | Fixed. `Gestures.bringIn`/`broughtIn` (screen.ts:671-682) cover every trackpad action that used to act at the cursor: tap and long press (up, screen.ts:894-895), long press then move, which is now a plain slide (screen.ts:743), the two-finger right click (screen.ts:916-917), and `held()` (screen.ts:664), which shows no ring or buzz. A drag can no longer start hidden, and during a drag `followView` keeps the cursor on the inner line. The two-finger wheel at the cursor happens only at zoom 1. Direct mode is unaffected. The new test (screen.test.ts:938) checks each path, including that the next tap is a click of its own. |
| N1 | Fixed. The ResizeObserver keeps the cursor in view only if it was inside the previous box at the current view (screen-view.tsx). The first callback just records the box. The keyboard transition still keeps a visible cursor in view. |
| N2 | Fixed. `overflow: hidden` on `main` was removed; only `overscroll-behavior: none` remains. |
| N3 | Fixed. The document-level cancels are skipped while `visualViewport.scale > 1.01`. The box's own touchstart/touchmove cancels stay, so a pinch on the picture remains the app's. `scale` is checked on every event, so a pinch-out stops being allowed once the page is back at 1. There is a test. |
| N4/N5 | Fixed. `v6` is read after `touchEnd`, and the centring loop now ends in a check. |
| N6-N9 | Fixed. The vi doc was reworded; the stale "Scrolls" key was removed; `panFrame` is now declared with `drawFrame`; the frame loop starts only when `view().zoom > 1`. |

No BLOCKING issues remain.

Non-blocking notes (no action required):
- screen.ts:894: if the computer moves the cursor out of the shown picture between the two taps of a double tap, the second tap only brings the cursor back, so the double click becomes a single click. This is rare and the safe direction.
- docs/user-guide.md: "never on the page itself, which neither zooms nor scrolls" no longer holds for a page that was already zoomed before the screen opened, since that page can now be pinched back out. This is a minor wording point.
- The real iOS Safari page-zoom behaviour is still only verifiable on the owner's iPhone.

Status: DONE
Summary: All of the earlier findings, B1 and N1-N9, are fixed and covered by tests; no blocking issues remain.
