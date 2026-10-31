---
title: "Chat view parity with Claude Code"
description: "Close the gaps between Kelpie's desktop chat view and the Claude Code reference: live status, file links, pinned prompt, kept answers, pictures, bookmarks and an agent map."
status: completed
priority: P2
effort: 56h
branch: feat/chat-view-parity
tags: [feature, frontend, backend]
blockedBy: []
blocks: []
created: 2026-10-30
---

# Chat view parity with Claude Code

## Overview

Audit of 2026-10-30 compared the desktop chat view (`app/src/components/chat-pane.tsx`, `chat-rows.tsx`, `chat-composer.tsx`) with Claude Code screenshots. This plan closes every gap found. Execute phases in order, in a fresh session; each phase file is self-contained.

Already good, do not rebuild:

- Running tool dots pulse (`.trow.running`); done is green, error red.
- Tables render clearly: checked 2026-10-30 by rendering `.msg-assistant` markup with the real `styles.css` in headless Edge. Only the streaming state is missing (phase 1).
- Links `http(s)` open in the browser; diff file names and "N files changed" open the diff.

## Goals

| # | Gap today | Phase |
|---|---|---|
| 1 | Streaming reply dot is static; copy is hover-only and missing on user messages and tool IN/OUT; "Explored" row shows absolute paths; tables show raw pipes while streaming | 1 |
| 2 | File links in replies do nothing; inline-code paths, tool targets and URLs in user messages are not clickable | 2 |
| 3 | Latest prompt scrolls away; nothing pins it | 3 |
| 4 | Answered question cards drop the questions; after a replay they say "No longer needed" | 4 |
| 5 | Chat's own input takes no pasted picture; the transcript shows only `[name.png]`; no way to enlarge | 5 |
| 6 | No bookmarks | 6 |
| 7 | No agent map | 7 |

## Phases

| # | Phase | Status |
|---|-------|--------|
| 1 | [Transcript polish](./phase-01-transcript-polish.md) | Completed |
| 2 | [Open files from links](./phase-02-open-files-from-links.md) | Completed |
| 3 | [Pinned prompt](./phase-03-pinned-prompt.md) | Completed |
| 4 | [Persist question answers](./phase-04-persist-question-answers.md) | Completed |
| 5 | [Paste and zoom images](./phase-05-paste-and-zoom-images.md) | Completed |
| 6 | [Response bookmarks](./phase-06-response-bookmarks.md) | Completed |
| 7 | [Agent map](./phase-07-agent-map.md) | Completed |
| 8 | [Docs and final verification](./phase-08-docs-and-final-verification.md) | Completed |

Phases 6 and 7 use `scrollToItem` from phase 3. Phase 5 uses the user-message action row from phase 1. Phase 4 is the only one that changes the daemon.

## Decisions

- **Chat column width stays 1200px.** `--chat-max` was set on purpose in b35e67b ("wide … so more fits"). Not changed here.
- **Pinned prompt is an overlay, not CSS `position: sticky`:** it needs a compact clamp, must work when the prompt is outside `RENDER_WINDOW`, and must not move the scroll height.
- **Answers are kept by the daemon, in its replay log,** as a `kelpie_answer` line next to the existing `kelpie_time` lines. It covers all three agent dialects and every device.
- **Files outside the project are never launched.** Agent output is untrusted: no `openPath` on anything it names (phase 2).
- **Bookmarks live in their own file,** with the full reply text, so they outlast the session. `app-state.json` is rewritten whole every 400 ms of activity and must stay small.
- **Agent details come from the event stream,** not from reading `subagents/*.jsonl`: no new native command, and it behaves the same on replay.

## Non-goals

Phone client UI (`app/phone/`, which must still build and pass tests), terminal view, chat width, pictures on remote sessions (already refused), and a full per-subagent transcript viewer.

## Execution rules

- **Start in your own worktree.** `D:\Works\Kelpie` is on `phone/phase3-link` with about 130 uncommitted paths (phone phase 3, kelpie-link, has no other copy; on 2026-10-30 they included `styles.css`, `icons.tsx`, `store.ts`, `types.ts`). Never switch branches, stash, reset or clean there. Run `git worktree list`, then create a worktree on a new branch `feat/chat-view-parity` from `origin/master`. Do not wait for phone phase 3.
- **This plan's files are untracked in `D:\Works\Kelpie`,** so they are not on `origin/master`. Copy `plans/261030-0830-chat-view-parity/` into the new worktree and commit it there first. Do not remove the original until the phone session has finished.
- **Expect overlap at merge time.** Phone phases 2b and 3 touch `app/src/chat/*`, `app/src/lib/store.ts` and `mobile-settings.tsx`; 2b is committed locally on `phone/phase2b-chat-send`. Before each phase, check `git log origin/master` for what has landed, and rebase rather than copy phone code across.
- **Commits:** one commit per phase, conventional format, stage by explicit path, no push or PR unless asked. No plan or phase numbers in code, tests or commit messages.
- **Review:** each phase ends with tests, an independent code review and fixes, then the commit. Do not trim this loop to go faster.
- **Disk is tight:** `CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`, no installer builds; test in dev. This worktree builds into its own `CARGO_TARGET_DIR=C:\kelpie-chat-target`: two worktrees sharing a target get the same Cargo fingerprints and silently run each other's binaries. Delete it when done.
- **Daemon safety:** use only the isolated dev daemon (`bash scripts/dev-daemon.sh run`, endpoint `kelpie-dev`). Never stop or mutate the default daemon. Set `KELPIE_ENDPOINT` inside any test script's own process.
- **Claude test sessions:** exit them cleanly. Hard-killing them turns off the user's fullscreen TUI.
- **Strings:** every visible string goes through `t()`, with a Vietnamese entry (chat strings in `app/src/locales/vi/tools.json`, composer strings in `composer.json`).
- **Checks each phase:** focused Vitest file, then `cd app && pnpm typecheck && pnpm test && pnpm i18n:check`; then the real desktop app, since unit tests do not cover WebView behaviour.

## Success Criteria

- [ ] Every row of the Goals table is fixed and checked in the real app.
- [ ] Replay never sends anything to an agent (existing boundary, covered by new tests).
- [ ] `pnpm typecheck`, `pnpm test`, `pnpm i18n:check` and `cargo test -p kelpie-proto -p kelpied -p kelpie` pass.
- [ ] Perf gate shows no regression against `scripts/perf/baseline.json`, and the baseline is unchanged.
- [ ] `docs/user-guide.md` and `docs/architecture.md` describe the new behaviour.

<!-- slug: chat-view-parity -->
