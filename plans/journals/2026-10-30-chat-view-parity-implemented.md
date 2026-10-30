---
title: Chat view parity implemented
date: 2026-10-30
summary: All eight phases of chat view parity implemented and checked in the real app; the terminal to chat switch fixed and released as 0.1.9
---

# Chat view parity implemented

## What happened
Implemented plans/261030-0830-chat-view-parity phase by phase in worktree D:\kelpie-chat: fix, tests, independent review, fixes, real-app scenario, commit.

Bug reported on the way: switching a new Claude terminal session to chat failed with error_during_execution. Claude saves a conversation only with its first turn, so `--resume` of a preset id exits 1. Fixed in the daemon, released as 0.1.9 with the transcript polish.

## Surprises worth keeping
- Two worktrees sharing one CARGO_TARGET_DIR run each other's binaries: Cargo fingerprints are workspace-relative. It made a fixed bug look unfixed. The plan had told the implementer to share one.
- Claude 2.1.314's Agent tool returns `async_launched` at once, so a tool row says done while the agent runs. A capture changed the agent map's design before it was built.
- Claude shrinks a picture before echoing it, so the log holds far less than feared; the byte strip is only a safety net.
- Vitest ran Solid's server build, so no memo or effect reacted in tests.
- Replay fed line by line ran every whole-list memo per line. Batching took a 4000 row probe from 65 s to 0.6 s.
- Review found what the scenarios did not: focus lost from the composer, a CSS class collision with Settings, an entry bundle over its limit caused by one eager import.

## Decisions
- Stop answers a card still open even if only a replay showed it: it is the person's action, and the agent is waiting on it.
- Bookmarks change through a read-modify-write in Rust, since two processes can share the file.
- API_LEVEL 20 on this branch; the phone branches must renumber on rebase.

## Not done
- Perf gate GUI metrics not run: they need a release build with the speech helper, on a nearly full disk. Bench was noisy on a loaded machine (0.1.8 and this branch both over twice the baseline); baseline untouched.
- `noteSent` part of the picture work waits for the phone branches.
- A Codex first prompt disappears until its echo, up to 28 s with slow hooks: older than this work.
- Default DOMPurify still used in file-viewer, turn-file-view and rich-viewers.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
