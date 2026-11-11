# File links / copy-path menu: code review

Scope: diff `file-links.diff` vs origin/master; worktree `D:\kelpie-file-links\app\src`. Read-only review (Bash unavailable: typecheck/tests not run).

## Verdict
No trust-boundary escape found. The `http(s)`-only opener, the viewer-only file path, the UNC refusal and the DOM-built (not markup) spans all hold. I found two medium correctness bugs: the wrong machine for multi-host remote projects, and a stale async context menu. The rest are low.

## Verified safe (risk calibration)
- `underFolder` + `projectRelative` (chat-open-file.ts:14-30): `..` is resolved by name and the result is re-checked against the root. `D:\proj\sub` + `../../etc/x` → `D:/etc/x` → null; `D:foo` and rooted paths are rejected by the final rooted check. A UNC cwd collapses to `/server/...`, which is outside any root.
- `findFileRefs` (chat-links.ts:90): it does not match `\\host`, `//host`, `\\?\` or `scheme://…/a.py`, because the lookbehind rejects every inner start. Path segments cannot contain `/`, and the lookbehind allows about one start per token, so backtracking stays linear (no ReDoS).
- xterm coordinates (terminal-file-links.ts:606-615): 1-based inclusive x/y match the `Linkifier._linkAtPosition` and underline math in xterm 6. Both match ends are ASCII, so a wide character can only appear in the interior or before the match, where `at[]` maps it. The wrapped-line test values are correct.
- `openFileRefMenu` (path-menu.ts:50-53): `preventDefault`/`stopPropagation` run synchronously before the first `await`. Solid's delegated handler honours `cancelBubble`.
- `MarkdownHtml` effect: it resets `innerHTML` each run, so `markProseFiles` never wraps the same text twice. Walker cost scales with what `toHtml` already does.

## Medium

### M1. Remote project: a session on another linked host resolves against the project's host
`path-menu.ts:68`: `!!s.remote !== remoteRoot.test(project.path)` compares only booleans. A remote project can run panes on other linked hosts (`host-target.tsx` RunOn; `pane-view.tsx:86` already computes `elsewhere` as `target.dest !== projectTarget(project)?.dest`).

Failure: a terminal on host B (cwd `/srv/app`) prints `src/config.ts`, and Ctrl+click opens host A's `src/config.ts`. When the cwd path differs, the fallback to the project root and then the unique-name search still finds A's file, so the user edits a different machine's file without noticing. "Copy Path" also hands out A's path.

Fix: in `sessionFiles`, return null unless `sessionTarget(s)?.dest === projectTarget(project)?.dest` (and the ports match). Reuse the `elsewhere` logic.

### M2. The async right-click menu can show up stale or replace a newer menu
`path-menu.ts:53-60`: the menu opens only after up to two `readFile` calls plus a `quickOpen`. On an SSH project that takes hundreds of ms, or seconds when the host is slow. The `ContextMenuHost` close-on-`pointerdown` (context-menu.tsx:26) has nothing to close during the await.

Failure: the user right-clicks a path, then right-clicks elsewhere or clicks away (or presses Escape) before it resolves. The older file menu then appears at the old coordinates, possibly overwriting the newer menu. In the terminal, the plain Copy/Paste menu is also delayed for every right-click over a path.

Fix: use a module-level generation counter (increment it on each `openFileRefMenu` and on window `pointerdown`/`keydown`, and drop the result if it changed). An alternative is to open the menu at once with Open + copy-as-written items and resolve on click.

## Low

### L1. Non-ASCII letters split words: `tệp.py` links `p.py`
chat-links.ts:90: `\w` is ASCII-only (no `u` flag), so `ệ` is not in the lookbehind. Vietnamese or accented names produce a partial link (underline on `p.py`, click searches `p.py`). This matters given the vi locale.

Fix: use the `u` flag with `\p{L}\p{N}` in the lookbehind and lookahead. Note the tradeoff: this would also stop CJK-adjacent matches like `修改了src/a.ts`, so it is a product call.

### L2. Ctrl+right-click both opens the file and shows the menu
xterm 6 `Linkifier._handleMouseUp` calls `activate` for any button. `modified(e)` (terminal-host.ts:131) checks only `ctrlKey`/`metaKey`. This was already true for web links and now also applies to files.

Fix: add `e.button === 0` to `modified`.

### L3. Silent no-op for a local project's session running on a host
`sessionFiles` returns null, so Ctrl+click does nothing and gives no toast. Consider notifying ("files of this session are on {host}").

### L4. A terminal cwd outside the project still falls back to the project root and the name search
chat-open-file.ts:27-29: when `fromCwd` is null because the cwd is outside the project, a relative path is still resolved against the project root and then the unique-name search. That can open a same-named but unrelated file. This is acceptable for chat. For a terminal with a known cwd outside the project, consider refusing relative refs instead.

### L5. Missing tests
There are no unit tests for `absolutePath` (local, `ssh://host`, `ssh://host/`), `sessionFiles` (the M1 case), or `markProseFiles` (DOM; could use happy-dom or a browser e2e check).

### L6. Docs
`docs/user-guide.md` around line 701 describes terminal links and Ctrl/Cmd+click URLs. It does not mention file-path links, the right-click link menu, Copy Name, or that prose file names in chat are now clickable.

### L7. Inherent limits (info)
- TUIs that hard-wrap (no `isWrapped`) give a tail-fragment link (`nents/x.tsx`), which resolves by name search.
- `C:\My Project\a.py` links only `Project\a.py`.
- `__init__.py` in prose is turned into `<strong>` by marked and is not linked.

## Unresolved questions
- `fs_reveal` passes `/select,{path}` to explorer unquoted except for spaces. Is a project file with a comma in its name (now reachable via Reveal from agent-named links, not only the tree) parsed safely by explorer? This predates the change and I did not verify it.
- Typecheck, vitest and i18n:check were not run (Bash broken).

## Follow-up (main session, 2026-11-11 14:30)
- M1 fixed: `sessionFiles` requires the session's host (dest + port) to be the project's (`sessionTarget`/`projectTarget`); tested.
- M2 fixed: the file menu opens at once; the file is resolved only when Copy Relative Path / Copy Path / Reveal is picked (`fileRefMenuItems`).
- L2 fixed: link activation needs button 0. L5: `path-menu.test.ts` (absolutePath, menus, sessionFiles). L6: user guide updated.
- L1, L3, L4, L7 left as is (trade-offs / rare). `fs_reveal` comma question predates this change, not checked.
- Real app (debug build, isolated daemon, WebDriver): 20/20 checks — terminal hover/Ctrl+click/plain click/line 42, bare name via name search, file and URL menus, header menu copy, chat prose + code refs, Copy Path, click opens.
