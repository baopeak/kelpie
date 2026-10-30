---
phase: 5
title: "Paste and zoom images"
status: pending
priority: P1
effort: "9h"
dependencies: [1, 3]
---

# Phase 5: Paste and zoom images

## Overview

Paste or drop a picture directly into the chat's own input, see it as a thumbnail in the transcript, and click any picture to enlarge it.

Today:

- `chat-composer.tsx` has no paste or drop handling. Only the separate floating `Composer` (Ctrl+Shift+I, `composer.tsx`) takes pictures.
- The transcript shows `[image-1.png]` as text (`withImageNames`).
- No lightbox exists.

Bug visible in the code, to reproduce first: `pushUser` matches a pending message by comparing text. A pending picture message holds `"text\n[image-1.png]"`; the echo (`--replay-user-messages`) holds `"text"`. They never match, so the message should duplicate and stay at 70% opacity. An image-only message has no text block in its echo, so it should stay pending for ever. Existing tests do not cover the echo.

## Requirements

- Functional:
  - The chat input accepts paste and drop of pictures, shows removable thumbnails, and sends them, including while the agent is busy (queue) and with no text.
  - User messages in the transcript show thumbnails instead of `[name]`. Text and pictures are separate.
  - Clicking a thumbnail opens a lightbox: fits the window, click toggles actual size (scrolling when larger), arrow keys move between a message's pictures, Esc or a click on the backdrop closes it, and focus returns.
  - The same lightbox opens for composer thumbnails (both composers) and for `<img>` in agent replies.
  - A picture no longer available (temp file swept, not in the replay) shows a named placeholder instead of a broken image.
- Non-functional:
  - Limits unchanged: `CHAT_IMAGE_MAX` 5 MB, `MAX_IMAGES` 8; refused on remote sessions; `IMAGE_EXT` types only.
  - **Security.** A path from the log or from history is agent output. `imageUrl` loads `path` only when it was attached in this window or lies under the `kelpie-composer` temp folder; anything else is a name-only placeholder, with no file-system call at render. Harden `fs_read_image`'s guard, which checks only `\\` and `//` (`/\host\x.png` passes): normalise slashes first, with a Rust test. `mime` for a `data` URL must be in `IMAGE_MIME`; otherwise a placeholder.
  - Loading goes through `fs_read_image`, keeping its other guards (no symlinks, picture types only).
  - Base64 is not kept in the store for live messages; they keep `path` and load on demand with `loading="lazy"`.
  - CSP is unchanged (`img-src 'self' data: blob:`). Remote images in replies stay blocked, and the lightbox only opens images that actually loaded.

## Architecture

- Item: `{ kind: "user"; …; images?: { name: string; mime?: string; path?: string; data?: string }[] }`. Live messages carry `path`; replayed ones carry `data` when the log has it.
- URL: a new `chat/chat-images.ts` with `imageUrl(img): Promise<string | null>`: `data` gives a data URL; `path` goes through `readImage`; errors give null. Cache by path, at most 32 entries (LRU).
- Shared picture logic: `attach`, `drop` and their guards move from `composer.tsx` to a new `lib/composer-pictures.ts` (`createPictures(session)`), and both composers use it. Move, do not copy.
- Lightbox: `components/image-lightbox.tsx`, rendered once via `Portal` from `app.tsx`, using a module-level signal: `openLightbox(images: { url: string; name: string }[], index)`. Uses `.overlay`.

## Related Code Files

- Create: `app/src/chat/chat-images.ts`, `app/src/chat/chat-images.test.ts`, `app/src/lib/composer-pictures.ts`, `app/src/components/image-lightbox.tsx`
- Modify:
  - `app/src/components/chat-composer.tsx`, `app/src/components/composer.tsx`, `app/src/components/composer.css`, `app/src/components/chat-pane.tsx`, `app/src/components/app.tsx`, `app/src/styles.css`
  - `app/src-tauri/src/fs_ops.rs` (guard), `app/phone/src/chat-view.tsx` (minimal fallback)
  - `app/src/chat/chat-model.ts`, `app/src/chat/claude-adapter.ts`, `app/src/chat/codex-adapter.ts`, `app/src/chat/acp-adapter.ts` and their tests
  - `app/src/locales/vi/composer.json`, `app/src/locales/vi/tools.json`

## Implementation Steps

1. **Capture real data.** On the isolated daemon, send a picture from the floating composer to a real Claude chat, and exit it cleanly. Note from the log: does the echo include the image block with its base64? Does the message duplicate or stick at pending? Record the result in the phase file. `LOG_MAX_BYTES` is 12 MiB, so one 5 MB picture (6.7 MB of base64) can evict much of the transcript: if the echo holds base64, keep that in mind before relying on it.
2. **Fix matching, test first.** Pending items store the raw `text` and `images`; `pushUser` matches on raw text. An echo with no text matches the oldest pending image-only item. New tests: text plus picture leaves one item, not pending; image-only leaves one item, not pending; plain text behaves as now.
3. **Transcript model.** `claude-adapter.send` and `acp-adapter.send` (line with `withImageNames`) put `images: [{name, mime, path}]` on the user item. Codex: same; on replay read `localImage` paths from `userMessage` content at `case "userMessage"`. Claude on replay: read `image` blocks in `applyClaudeMessage` (`m.message.content`), with `data` if step 1 showed it, otherwise a name-only placeholder entry. `loadHistory` also goes through `applyClaudeMessage`, capped at 6000 lines with no byte limit, so history images are always name-only. ACP: `user_message_chunk` image content when present. Codex: where `renderTurns` blanks `queued-` items' text so they hide (`i.text = ""`), also clear `images`, or they stay and duplicate the echo. Update the test expecting `"what is this?\n[image-1.png]"`, deliberately.
4. **Keep `withImageNames` where text is the right form:** queue chips and unsent notes (`chat-model.ts` near `enqueueNew`, `applyQueue`). In `noteSent` (`kelpie_sent`), the path ACP uses with the daemon queue: on the desktop take `images` from `this.mine.get(cid)`, otherwise name-only entries from `msg.images`; an observer (the phone) keeps today's `[name]` text. Do not change the wire format. An image-only item must not render an empty bubble on the phone: add a minimal fallback in `chat-view.tsx`.
5. **Render.** `chat-pane.tsx`: change `<Show when={u.text}>` to `u.text || u.images?.length`. Add `.msg-user-images`, a wrapping row of thumbnails (`max-height: 120px`, radius 8px, `object-fit: cover`, `loading="lazy"`, `alt={name}`, `<button>` wrapper). Show the placeholder for a null URL. Pinned prompt: text, or `t("{n} pictures")`.
6. **`composer-pictures.ts`.** Move `Attached`, `attach`, `IMAGE_EXT`, `IMAGE_MIME`, `CHAT_IMAGE_MAX`, `MAX_IMAGES`, and the "read bytes at send" step. `drop` also turns non-pictures into mentions and edits the textarea, so it becomes `createPictures(session, onOther)`, with mention handling left in `composer.tsx` as the `onOther` callback. Move the `.composer-attached` and `.composer-image` rules from `composer.css` to `styles.css`: `composer.css` is imported only by the lazy `composer.tsx`, so they would be unstyled in the chat input until the floating composer opens. Run the composer's behaviour by hand afterwards: terminal sessions, mentions for non-pictures, and the floating composer's drop.
7. **Chat input.**
   - `onPaste`: pictures from `clipboardData.files` are attached and `preventDefault` is called; plain text passes through.
   - Drop: use `getCurrentWebview().onDragDropEvent` with the same hit-test as `composer.tsx` (it checks the drop is inside its root). Check `app.tsx:132` does not also handle it, and that both composers open at once (floating composer over a chat) do not both take the drop: the floating one wins.
   - Thumbnails above the textarea (reuse `.composer-attached`, `.composer-image`).
   - `send`: allow empty text with pictures; `ctrl.send(text, true, attachments)`; clear after. `stops()` (Stop button when busy with no text) and the Send button's `disabled` must both also check attached pictures. Non-picture drops: keep current behaviour.
   - An ACP agent without `canImage` receives the picture's path as text (`acp-adapter.prompt`). Keep that, and say so with a note when attaching, since the agent may not be able to read it.
8. **Lightbox.** `window.addEventListener("keydown", fn, true)` while open (Esc, ArrowLeft, ArrowRight), in the capture phase as `context-menu.tsx` does, with `stopPropagation`. The interrupt lives in the textarea's Solid-delegated `onKeyDown`, so a bubbling listener would be too late and Esc would stop the agent. Image `max-width: 92vw; max-height: 88vh`; class `actual` removes both limits, with the container `overflow: auto`. Name and "n / m" caption. `aria-modal`, initial focus on close button, restore focus.
9. **Replies.** `onMarkdownClick`: `target.closest("img")` that has `naturalWidth > 0` opens the lightbox with that image. Add `cursor: zoom-in` for `.msg-assistant img`, and `max-width: 100%`.
10. Composer thumbnails: clicking the image opens the lightbox; the remove button still removes.

## Tests

- `claude-adapter.test.ts`: step 2 cases; a replayed `image` block produces `images`; `userMessage` with `localImage` in `codex-adapter.test.ts`.
- `chat-images.test.ts`: data URL form, failed read gives null, cache evicts the oldest and does not re-read a cached path (inject the reader); a replayed path outside the temp folder is never read; a `mime` outside `IMAGE_MIME` gives a placeholder.
- `fs_ops.rs`: `/\host\x.png`, `\/host\x.png`, `\\host\x.png` and `//host/x.png` are refused.
- Chat-model: a queued picture message keeps its images when the turn ends.

## Success Criteria

- [ ] Ctrl+V a screenshot in the chat input: thumbnail appears; send; agent answers about it; the transcript shows the thumbnail, with no `[name]` and no duplicate, not stuck dim.
- [ ] Image-only message works. Text paste still pastes text.
- [ ] Sending while the agent is busy queues it, and the thumbnail appears when sent.
- [ ] Click a thumbnail: lightbox opens; click toggles actual size; arrows move; Esc closes it and does **not** interrupt the running agent.
- [ ] Close and reopen the window: pictures still show, or show a named placeholder, never a broken image icon.
- [ ] Oversized, 9th and remote-session pictures are refused with the existing messages.
- [ ] Floating composer unchanged, for terminals too.

## Risk Assessment

- **Log has no image data on replay.** The daemon's `kelpie_sent` line already carries picture names (`msg.images`), but not paths or bytes. Decide from step 1: if the echo has no data, resolve a name against the `kelpie-composer` temp folder only (a pasted picture lives there for a few days, until `sweep_temp`), and show the placeholder otherwise. Report the result to the user: a dropped picture from elsewhere, or an old one, shows as a placeholder after reopening.
- **Moving composer code regresses terminals.** It is the riskiest part: the code handles mentions, shells and quoting nearby. Move only picture code, keep the tests, and run the manual check above.
- **Two drop listeners.** Floating composer and chat input may both be under the pointer; hit-test order is in step 7.
- **Memory.** Replayed `data` stays in the store while the session is open: at most what the 12 MiB log held. Acceptable.
