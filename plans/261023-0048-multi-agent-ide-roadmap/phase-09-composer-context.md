---
phase: 9
title: "Composer & context attach"
status: completed
priority: P1
effort: "3d"
dependencies: [1]
---

# Phase 9: Composer & context attach

## Overview
One composer for every pane — TUI or chat, any vendor — with @-mentions and images.

## Requirements
- `Ctrl+Shift+I` (pane toolbar *Compose*): floating multi-line composer; Enter sends, Shift+Enter newline.
- Mentions: files and folders (directory entries added to the search index [RT-10]), `@selection`, `@diff`, `@terminal:<session>`, `@issue`, `@note`.
- Images: paste/drop → temp file → path for TUI, image block for chat/ACP.
- Delivery [RT-10]: through existing `sendToSession` / `typeInto` (xterm adds bracketed-paste markers only when the TUI enabled them; Codex Enter delay kept);
  daemon sanitiser from phase 1 applies (`paste:true`). Agents accepting `@path` (Claude, Codex, Gemini) get `@path` instead of content.
- Outbound text from `@terminal` and `@issue` passes `redact` (phase 1) before insertion; inlined content fenced and labelled with its source.
- "Send selection to agent" from editor, diff and terminal menus; per-project prompt history (↑), searchable in palette.

## Architecture
Fuzzy search in Rust core (`search.rs`, nucleo) incl. directories. Pure TS `expandMentions(text, ctx, agentKind)`; caps: snippet ≤ 200 lines, total ≤ 64 KB, else path reference.

## Related Code Files
- Create: `app/src/components/composer.tsx`, `app/src/lib/mentions.ts`, `app/src/lib/mentions.test.ts`.
- Modify: `app/src-tauri/src/search.rs` (directories), `app/src/components/pane-view.tsx`, `app/src/components/chat-pane.tsx`, `app/src/components/file-viewer.tsx`,
  `app/src/components/diff-view.tsx`, `app/src/components/context-menu.tsx`, `app/src/lib/send-to-agent.ts`, `app/src-tauri/src/fs_ops.rs` (temp image), `app/src/lib/commands.ts`, locales, docs.

## Implementation Steps
1. `mentions.ts` + tests. 2. Directory index. 3. Composer UI + images. 4. Delivery via `sendToSession`. 5. Send-selection + history.

## Tests
- vitest: expansion per agent, caps, Windows paths with spaces, redaction applied to `@terminal`.
- e2e `composer.py`: @file into a shell; a file containing `\x1b[201~` + command is inserted inert; image paste → path; chat gets block; Codex submits (not newline).

## Perf gate
- Fuzzy ≤ 30 ms p95 on 50k files; composer lazy; echo unchanged when closed.

## Success Criteria
- [x] Same composer works for Claude, Codex, Gemini, a shell and an ACP chat; no break-out from any fixture.

## Risk Assessment
- Large pastes slow TUIs → caps + path references.
