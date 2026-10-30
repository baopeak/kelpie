---
phase: 3
title: "Pinned prompt"
status: pending
priority: P1
effort: "5h"
dependencies: [1]
---

# Phase 3: Pinned prompt

## Overview

While you read a long answer, the prompt it answers stays pinned at the top of the chat. Clicking it jumps to the original. Also adds `scrollToItem`, which phases 6 and 7 use.

## Requirements

- Functional:
  - When the prompt owning the top of the viewport has scrolled off, a compact bar at the top of the chat shows it (at most 2 lines), with its time.
  - While streaming and stuck to the bottom, the bar shows the latest prompt once it is off-screen.
  - Hidden when that prompt is still visible, or when there is no user message above the viewport.
  - Clicking the bar scrolls to the prompt, showing earlier items if it is outside `RENDER_WINDOW`.
- Non-functional:
  - Does not change scroll height or `stick` behaviour; the "Jump to latest" pill still works.
  - At most one position pass per frame while streaming.
  - Keyboard reachable (a real `<button>`).

## Architecture

Overlay, positioned inside `.chat` (which is `position: absolute`), a sibling of `.chat-scroll`, not inside the scrolled content. Sticky was rejected: it cannot clamp only when stuck, needs the element rendered, and interacts with `content-visibility: auto` on `.chat-inner > *`.

Pure core in `chat/pinned-prompt.ts`:

```ts
/** Index of the last prompt that ended above `scrollTop`; -1 when none did, or when the next
 *  prompt straddles the top (`top <= scrollTop < bottom`), so it is still on screen. Ascending. */
export function pinnedIndex(prompts: { top: number; bottom: number }[], scrollTop: number): number; // binary search
```

Off-screen rows keep a layout box (`contain-intrinsic-size: auto 60px` remembers measured heights), so `offsetTop` stays ordered, though estimated until measured. Good enough to pick a prompt; correct as the user scrolls.

Prompts before the render window have no element. If no rendered prompt qualifies but earlier user items exist in `ctrl.state.items`, use the last of those.

## Related Code Files

- Create: `app/src/chat/pinned-prompt.ts`, `app/src/chat/pinned-prompt.test.ts`
- Modify: `app/src/components/chat-pane.tsx`, `app/src/styles.css`, `app/src/locales/vi/tools.json`

## Implementation Steps

1. Add `data-item={id}` on each row's outer element: `.msg-user-wrap`, `.msg-reply`, `.trow` (`ToolRow`, `ToolRun`, `ThinkingRow`), `.perm`, `.result-line`, `.turn-files`. Add `data-user` on `.msg-user-wrap`.
2. Add `scrollToItem(id)` in `ChatBody`, and expose it through a small object passed where needed:
   - If `id` is a member of a folded run, target the run's first item (`shape().runs`).
   - If it is outside `items()`, `setShowAll(true)`, wait a frame.
   - `el.scrollIntoView({ block: "start", behavior: "smooth" })`, set `stick = false`, `setAway(true)`. Unmeasured rows are 60 px estimates, so a smooth scroll can land off target: on `scrollend`, re-run once without smoothing.
   - Add a brief highlight class for 1.2 s, using opacity or an outline, no layout change. Respect `prefers-reduced-motion` (no smooth scroll).
   - Reuse it for `toPending`, keeping `block: "center"` through an option.
3. In the `onScroll` handler, schedule one `requestAnimationFrame`, coalesced by a flag, calling `updatePinned()`. Also call it after the auto-scroll effect and on `items().length` change. Cancel the frame in `onCleanup`.
4. `updatePinned()`: `scroller.querySelectorAll("[data-user]")` (cache until `items().length` changes); read each `offsetTop` and `offsetHeight`, relative to the scroller; `pinnedIndex`; set a signal with the item id, or null. Writing the same id does nothing, so Solid does not re-render.
5. Render the bar as a sibling of `.chat-scroll`, with the user item looked up by id through a memo (so text updates without layout reads):

   ```tsx
   <button class="chat-pinned" onClick={() => scrollToItem(id)} title={t("Go to this message")}>
     <span class="chat-pinned-text">{text}</span>
     <Stamp at={at} />
   </button>
   ```
6. CSS: `.chat-pinned { position: absolute; top: 0; left: 0; right: 0; z-index: 4; }` (below `.chat-pill-dock`'s 5), background `var(--term-bg)`, bottom hairline `var(--border)`. The inner width lines up with `.chat-inner` (`max-width: var(--chat-max)`, gutter). Text uses `-webkit-line-clamp: 2`, `white-space: pre-wrap`, right-aligned to echo the user bubble.
7. Treat image-only prompts: show the picture count (phase 5 adds `images`; until then such messages have name text and just work).

## Tests

`pinned-prompt.test.ts`: empty list; before the first prompt; exactly on a boundary (a prompt whose bottom equals `scrollTop` counts as off-screen); a later prompt straddling the top gives -1; after the last; many prompts.

## Success Criteria

- [ ] Scroll through a long reply: the bar shows its prompt and changes as you cross into the next turn.
- [ ] Prompt fully visible: no bar. Partly scrolled, with its bottom still visible: no bar.
- [ ] During a live stream, stuck to the bottom: the bar appears once the prompt leaves, and auto-scroll does not stutter.
- [ ] Click jumps to the prompt, including one beyond the 300-item window; "Jump to latest" returns.
- [ ] Perf gate shows no regression (run `python scripts/perf/perf_gate.py --label pinned-prompt --compare scripts/perf/baseline.json` from the root).

## Risk Assessment

- **Reading `offsetTop` per frame while streaming forces layout.** It happens at most once a frame, on a short list. Signal the assumption broke: long-task warnings or a perf-gate regression. Response: update only when `scrollTop` moved more than 24 px, or when the user is not stuck to the bottom plus every 500 ms.
- **Estimated offsets pick the wrong prompt in unmeasured history.** It self-corrects within a scroll. If wrong prompts are visibly common, use an `IntersectionObserver` on prompts instead of reading offsets.
- **The bar covers the top ~44px.** Content scrolled under it is hidden, and `scrollToItem` must not land under it: set `scroll-margin-top: 52px` on `[data-item]`.
