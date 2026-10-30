---
phase: 2
title: "Open files from links"
status: pending
priority: P1
effort: "6h"
dependencies: [1]
---

# Phase 2: Open files from links

## Overview

Everything in the transcript that names a project file opens it, at the right line. Today `onMarkdownClick` drops any link that is not `http(s)`, so a file link in a reply does nothing.

## Requirements

- Functional:
  - Markdown links to files open in the viewer: `src/a.ts`, `src/a.ts#L42`, `src/a.ts#L42-L51`, `src/a.ts:42`, `src/a.ts:42:7`, `./a.ts`, percent-encoded names, absolute paths inside the project, `file:///D:/proj/a.ts`.
  - Inline code naming an existing project file is clickable.
  - A tool row's target opens its file: Read at its offset, Edit and Write as the diff.
  - The file name on a permission card's diff opens it.
  - URLs in user messages are links.
- Non-functional:
  - **Security.** Agent output is untrusted. Anything outside the project is not opened; a toast says so. Never call the opener's `openPath`, or anything else that launches a file with the OS. UNC and `//host` paths are rejected without touching the file system (Windows would sign in to the named host). Only `http(s)` goes to the browser. Other schemes are ignored.
  - Do not splice anything into sanitized markup: mark nodes in the DOM after `innerHTML`, as `MarkdownHtml` does for code blocks.
  - No file-system call while rendering; existence checks happen on click.

## Architecture

New pure module `chat/chat-links.ts`, with no Tauri imports:

```ts
export type ChatLink =
  | { kind: "web"; url: string }
  | { kind: "file"; path: string; line?: number }
  | { kind: "none" };
export function parseChatLink(href: string): ChatLink;
export function fileRefInCode(text: string): { path: string; line?: number } | null;
export function projectRelative(project: Pick<Project, "path">, path: string): string | null; // moved from chat-rows.tsx
export function linkifyText(text: string): ({ text: string } | { url: string })[];
```

Click flow in a new `openChatFile(project, ref)` in `chat-pane.tsx` or a small `components/chat-open-file.ts`:

1. `projectRelative` null: toast `"{path} is outside this project"`.
2. Link with a slash: `ipc.readFile(project.path, rel, 1)` to check existence (the same call and path guard the viewer relies on). Found: `store.openFile(project.id, rel, { line })`.
3. Not found, or a bare name such as `foo.ts`: `ipc.quickOpen(project.path, basename, 50)`, keep exact basename matches. One match: open it. Several: toast `"{n} files named {name}"`. None: toast `"File not found: {path}"`.

## Related Code Files

- Create: `app/src/chat/chat-links.ts`, `app/src/chat/chat-links.test.ts`
- Modify: `app/src/components/chat-pane.tsx` (`MarkdownHtml`, `onMarkdownClick`, `Markdown`, user bubble), `app/src/components/chat-rows.tsx` (`ToolRow`, `ToolRun`, `PermissionCard`, `TurnFiles`), `app/src/chat/tool-view.ts`, `app/src/styles.css`, `app/src/locales/vi/tools.json`

## Implementation Steps

1. Write `chat-links.ts` and its tests first. Move `projectRelative` there and update its imports. Remote project roots can have the form `ssh://host/abs`: strip `^ssh://[^/]+` from the root before comparing, or absolute remote paths get a false "outside" toast. `readFile` and `quickOpen` already branch on the root, so they need no remote case.
1b. **DOMPurify removes the hrefs this phase needs.** `toHtml` uses its default `IS_ALLOWED_URI`, which strips `file:///…`, `D:/x` and `a.ts:42` (`src/a.ts` survives), so the click sees an empty `href`. Vitest runs in Node and will not catch this. Use a chat-only `new Marked()` instance with a link renderer that copies the raw href to `data-href`; `onMarkdownClick` reads `data-href` and passes it through `parseChatLink` only. `data-href` is agent-controlled, like any other link text, so nothing but `parseChatLink` may interpret it. Check all forms in the real app.
2. `parseChatLink`: reject empty, `#anchor`-only and non-web schemes (`javascript:`, `data:`, `vbscript:`, and so on; a single-letter "scheme" is a drive letter). Strip `#L…`/`:line[:col]`; `decodeURIComponent` in a try/catch; turn `file:///D:/x` into `D:/x` and `file:///x` into `/x`; reject `file://host/…` and any `\\` or `//` prefix.
3. `fileRefInCode`: true only for a single token with no whitespace that contains `/` or `\`, or ends in a known-looking extension (`/\.[a-zA-Z][a-zA-Z0-9]{0,7}$/`). Reject URLs, flags (`-x`), `1.2.3`, `e.g.`, `foo.bar()`, `@scope/pkg` and anything over 260 characters. Accept a trailing `:line[:col]`.
4. `Markdown` takes an optional `project`. `ChatBody` passes it; so does `PermissionCard`'s `markdown` prop. Without a project, file links stay inert (phone is unaffected: it has its own view).
5. In `MarkdownHtml`'s effect, after the code-block pass: for each `:not(pre) > code` whose text passes `fileRefInCode`, set `data-file-ref` and class `file-ref`. `onMarkdownClick` becomes a closure over `project`, handling `.code-copy`, then `a`, then `[data-file-ref]`.
6. `ToolView` gains `path?: string; line?: number`: set them for `Read`, `Write`, `Edit`, `MultiEdit`, `NotebookEdit`, and from `item.diff[0].path` for generic edits. In `ToolRow`, when `projectRelative` succeeds, render the target as `<button class="link-btn tool-target">` with `stopPropagation`; edits open `{ diff: true }`. Keep the row toggling when clicked elsewhere. In `ToolRun`, keep the folded row as text (items are clickable once opened).
7. `PermissionCard`'s `.diff-file`: same button as `ToolRow`, showing `rel` when known, otherwise `basename`. It needs a `project` prop.
8. User bubble: render `linkifyText(u.text)` with `<For>`; each URL is an `<a>` built by Solid (never `innerHTML`) whose click calls `openUrl`. Trim trailing `.,;:!?)` from URLs, except a `)` that closes a `(` inside the URL. Keep `white-space: pre-wrap` and user text selection.
9. CSS: `.file-ref { cursor: pointer; text-decoration: underline dotted; text-underline-offset: 2px; }`, hover colour `var(--accent)`; `.tool-target` inherits `.tsum` font and truncation.

## Tests

`chat-links.test.ts`, as tables:

- `parseChatLink`: every form in Requirements; `HTTP://X` is web; `javascript:alert(1)`, `data:…`, `#top` and an empty string are `none`; `\\host\share\a`, `//host/a` and `file://host/a` are `none`; a bad `%` sequence does not throw.
- `fileRefInCode`: accepts `src/a.ts`, `a.ts:12`, `C:\x\a.rs`, `README.md`; rejects `npm install`, `foo.bar()`, `1.2.3`, `--flag`, `https://x.y/z`, `e.g.`, `@types/node`.
- `projectRelative`: existing cases (`..`, outside, Windows case-insensitive, trailing slash).
- `linkifyText`: URL at the start, middle and end; trailing punctuation; balanced parens; no URL.

## Success Criteria

- [ ] In the real app, each form in Requirements opens the file, and `#L42` lands on line 42.
- [ ] `` `README.md` `` in a reply opens it; `` `npm install` `` is not clickable.
- [ ] A link to a file outside the project, to `C:\Windows\system32\calc.exe` and to `\\host\x` launches nothing; the first two show a toast.
- [ ] Clicking a tool target opens the file without folding the row; clicking elsewhere still folds.
- [ ] A URL in a user message opens in the browser.
- [ ] `store.openFile` is called with a project-relative path only.

## Risk Assessment

- **`readFile` semantics.** `fs_read_file` may reject binaries or limit sizes. It is only an existence check: treat "too large" and "binary" as found. If the error text cannot separate those from "not found", add a `kind` to the error or fall back to `quickOpen`.
- **False positives in inline code.** The click check means the worst outcome is a toast. Tune the regex with real replies before widening it.
