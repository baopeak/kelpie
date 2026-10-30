---
phase: 6
title: "Response bookmarks"
status: pending
priority: P2
effort: "7h"
dependencies: [1, 3]
---

# Phase 6: Response bookmarks

## Overview

Bookmark any reply with one click next to Copy, and find it again in a "Saved" tab of the right panel, even after the session is gone.

## Requirements

- Functional:
  - A bookmark button next to Copy on finished replies; filled when saved; click toggles.
  - Right panel tab "Saved", per project, newest first: excerpt, session name, time, remove.
  - Clicking a saved item: if its session is open and the reply is in the transcript, reveal the session and scroll to it. Otherwise show the saved text in a dialog (rendered markdown, Copy).
  - The filled state survives a window reopen and re-attach.
- Non-functional:
  - Own file `chat-bookmarks.json` in the data directory, atomic write (`.tmp` then rename, as `state_save_blocking`).
  - **A failed read is not permission to overwrite** (repository rule): if the read fails, keep the feature read-only for the session and tell the user once.
  - Limits: 64 KiB of text per bookmark (longer is cut, and the item says so), 500 per project (at the limit, refuse with a toast; never drop silently).
  - No daemon or protocol change.

## Architecture

```ts
export interface ChatBookmark {
  id: string;
  projectId: string;
  sessionId: string;
  sessionName: string; // the label when saved: the session may be gone later
  anchor: string;      // hash of the reply's trimmed text
  text: string;
  truncated?: boolean;
  at: number;
}
```

Anchor by text hash (FNV-1a over trimmed text) because item ids are not stable: Claude's completed-message id includes `state.items.length`, and adapters differ. Identical replies share an anchor: `findAnchor` picks the first match, and saving a duplicate in the same session toggles the same bookmark, which is acceptable for identical text.

Store: `lib/chat-bookmarks.ts`, a Solid store, loaded once at start, saved immediately on each toggle or removal (they are rare, and `invoke` in `beforeunload` is unreliable). The app has one window and one state writer, so no compare-before-write across windows.

## Related Code Files

- Create: `app/src/lib/chat-bookmarks.ts`, `app/src/lib/chat-bookmarks.test.ts`, `app/src/components/bookmarks-panel.tsx`
- Modify:
  - `app/src-tauri/src/lib.rs` (`bookmarks_load`, `bookmarks_save`, registered like `state_load`/`state_save`, with a Rust test)
  - `app/src/lib/ipc.ts`
  - `app/src/lib/types.ts` (`RightPanel`), `app/src/components/chat-pane.tsx`, `app/src/components/chat-copy.tsx`, `app/src/components/right-panel.tsx`, `app/src/components/icons.tsx`, `app/src/styles.css`
  - `app/src/locales/vi/tools.json`

## Implementation Steps

1. **Native.** Copy the shape of `state_load_blocking`/`state_save_blocking`, in `spawn_blocking` via `fs_ops::blocking`. Load: missing file gives an empty list; a corrupt file gives an **error** (never defaults). Save: write `.tmp`, then rename. Keep the corrupt file untouched.
2. **Store.** `load()`, `toggle(item, session, project)`, `remove(id)`, `anchorOf(text)`, `isSaved(sessionId, text)` (memo-backed `Set` of `sessionId + anchor`, so a long transcript does not rescan per row). Failed load sets `readOnly`; `toggle` then notifies and does nothing.
3. **Icon.** Add `bookmark` and `bookmarkFilled` to `icons.tsx` (stroke style, 16px grid, as the rest).
4. **Button.** `BookmarkButton` in `chat-copy.tsx` (uses `.msg-action`). Show on `.msg-reply` when not streaming and text exists. Filled stays at full opacity. Label: `t("Save this reply")` / `t("Remove from saved")`.
5. **Panel.** Add `"saved"` to the `RightPanel` type in `lib/types.ts` and to the `tab()` id union in `right-panel.tsx`, add `tab("saved", t("Saved"))` with a count like Review, and its `<Match>`. `BookmarksPanel` lists items by `.list-row`; excerpt is 2 lines from `text` with markdown syntax left as is; remove `icon-btn`.
6. **Open.**
   - Session exists (`store.sessions[sessionId]`): set a module-level pending reveal `{sessionId, anchor}` in `chat-bookmarks.ts`, then `store.revealSession`. A window event would be lost if `ChatBody` is not mounted yet.
   - `ChatBody` consumes the pending reveal for its session once `ctrl.state.ready` is true (and when it changes while mounted). It finds the first assistant item whose `anchorOf(text)` matches and calls `scrollToItem`. If there is no match, it marks the reveal failed so the panel shows the dialog.
   - Otherwise: dialog with `Markdown` (no `project` file links needed if the project is gone; pass it when it exists), Copy, Close. Rendered through the same sanitizer.
7. Keep bookmarks out of the app state export/backup paths unless they copy the data dir anyway (check).

## Tests

- `chat-bookmarks.test.ts` (injected IPC, in-memory): toggle on and off, truncation sets `truncated`, the 501st is refused, `isSaved` after reload, a failed load makes `toggle` a no-op and never calls save.
- Rust: round trip; a corrupt file errors and stays byte-identical.

## Success Criteria

- [ ] Save a reply: icon fills, it appears in Saved with the session name.
- [ ] Reopen the app: still saved, icon still filled on the same reply.
- [ ] Click it: session comes forward and the reply scrolls into view, below the pinned bar, also when beyond the 300-item window.
- [ ] Close the session, click it: dialog shows the full text; Copy works.
- [ ] Corrupt `chat-bookmarks.json` by hand: the app says so, the file is unchanged, nothing overwrites it.

## Risk Assessment

- **Streaming text changes the hash.** Only finished replies get the button.
- **An adapter rewrites text on replay** (trimming, different chunks), so the anchor misses. Signal: icon not filled after reopen. Response: normalize whitespace before hashing; the saved text still opens via the dialog.
- **Secrets in saved replies.** The text sits in the user's data dir, same as app state and logs, with the same owner. It stays local and is never sent to the phone.
- **Multiple panels.** Each tab is project-scoped, so a worktree project has its own list. If the user finds this surprising, group by the root project later.
