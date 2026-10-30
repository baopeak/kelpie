---
phase: 1
title: "Transcript polish"
status: completed
priority: P2
effort: "4h"
dependencies: []
---

# Phase 1: Transcript polish

## Overview

Small visible fixes: a live dot on the streaming reply, copy buttons you can see and reach everywhere, readable "Explored" rows, tables that render while streaming.

## Requirements

- Functional:
  - The dot of a streaming reply pulses, like a running tool.
  - Reply actions are visible without hovering (dimmed), full strength on hover or focus.
  - User messages have a copy button. Tool IN and OUT blocks have one, which copies the full text, not the clamped view.
  - "Explored" and read/search rows show paths relative to the project.
  - A table still being streamed renders as a table once its delimiter row has arrived.
- Non-functional: no re-parse of finished blocks per delta (keep `stableEnd`'s purpose); only the open table is re-parsed.

## Architecture

- `copyText` and `CopyButton` move from `chat-pane.tsx` to a new `components/chat-copy.tsx`, because `chat-rows.tsx` needs them and `chat-pane.tsx` imports `chat-rows.tsx`.
- `stableEnd` moves from `chat-pane.tsx` to a new `chat/streaming-markdown.ts` next to a new `openTable(tail)`, so both can be tested without importing Tauri plugins.
- `stripProjectRoot(text, root)` joins `chat/tool-view.ts`.

## Related Code Files

- Create: `app/src/components/chat-copy.tsx`, `app/src/chat/streaming-markdown.ts`, `app/src/chat/streaming-markdown.test.ts`
- Modify: `app/src/components/chat-pane.tsx`, `app/src/components/chat-rows.tsx`, `app/src/chat/tool-view.ts`, `app/src/chat/tool-view.test.ts`, `app/src/styles.css`, `app/src/locales/vi/tools.json`

## Implementation Steps

1. **Streaming dot.** In `ChatBody`, give `.msg-reply` the class `streaming` when `it().streaming`. Add `.msg-reply.streaming::before { background: var(--working); animation: pulse 1.2s ease-in-out infinite; }`.
2. **Visible actions.** Change `.msg-action` from `opacity: 0` to about `.55`; keep `1` on `.msg-user-wrap:hover`, `.msg-reply:hover`, `:focus-visible` and `.copied`. Check both themes for contrast. Keep the code-block copy button hover-only (it sits on top of code).
3. **Copy buttons.** Move `copyText`/`CopyButton` to `chat-copy.tsx`; `onMarkdownClick` imports `copyText`. Add `CopyButton` to the user message's `.msg-meta`. In `ToolRow`, add it to each `.io` row, with `view().command` for IN and `props.item.output` for OUT. Stop the click reaching `.tool-head`. Do not add copy to the permission card.
4. **Relative paths.** Add `stripProjectRoot(text, root)`: removes the project root from path-looking parts of a string, accepting both slash styles, case-insensitive when the root has a drive letter. Apply it to `.tsum` in `ToolRun` and to `view().target` in `ToolRow` when the category is `read` or `search`. Keep the full string in `title`.
5. **Streaming tables.** `openTable(tail)` returns true when the first line of `tail` contains `|` and the second is a GFM delimiter row (`/^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/`). In `StreamingMarkdown`, render the tail with `MarkdownHtml` when true; otherwise keep the plain `pre-wrap` div.
6. Add tests, then run the phase's checks.

## Tests

- `streaming-markdown.test.ts`: `stableEnd` keeps its current behaviour (blank lines, fences, trailing text). `openTable`: true for pipe tables with and without outer pipes and with alignment colons; false for a lone `|` line, a header without a delimiter yet, prose containing `|`, and a code fence.
- `tool-view.test.ts`: `stripProjectRoot` for Windows and POSIX roots, mixed slashes, different drive-letter case, a path outside the root (unchanged), and a sibling folder sharing the prefix (`D:\a\bc` with root `D:\a\b` is unchanged).

## Success Criteria

- [ ] In the real app, a streaming reply's dot pulses and stops when it finishes.
- [ ] Copy on a user message, on reply text and on a long tool output (longer than the clamp) pastes the full text.
- [ ] Clicking a tool's copy button does not fold the row.
- [ ] A table streamed from a real agent turns into a table as its rows arrive.
- [ ] An ACP "Explored" row no longer shows the project root.

## Risk Assessment

- **Partial table cells flicker while re-parsing.** Accepted: it only lasts for the open table. If a very wide table visibly stutters, parse the tail at most every 100 ms.
- **Dimmed icons add noise on many short replies.** If they look busy in the real app, lower the opacity; do not return to hover-only.
- **`stripProjectRoot` alters a non-path string.** It matches only the exact root followed by a separator, so a false hit needs the literal root path in the text, where stripping is still correct.
