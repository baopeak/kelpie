---
title: Chat view parity plan
date: 2026-10-30
summary: Audited the chat view against Claude Code and wrote an 8-phase plan
---

# Chat view parity plan

## What happened
Compared the desktop chat view with Claude Code screenshots and wrote plans/261030-0830-chat-view-parity (8 phases). No product code changed. Tables already render clearly (real styles.css in headless Edge); pasted pictures show only as [name] and the chat input takes no paste.

## Findings worth keeping
- DOMPurify's default URI check strips file:///, D:/x and a.ts:42 hrefs; src/a.ts survives. Vitest in Node cannot catch this.
- Picture paths from the log or history are agent output. The fs_read_image guard checks only \\ and //, so /\host passes (read from code, not run).
- Esc interrupt lives in the textarea's Solid-delegated onKeyDown; overlays need a capture-phase window listener.
- pushUser compares pending text with [name] against an echo without it, so picture messages should duplicate or stay pending (from code, to reproduce).
- The replay log holds stdout plus kelpie_time and kelpie_sent, not answers, which is why answered cards say "No longer needed" after a replay.

## Decision
Keep chat width at 1200px. Answers go in the daemon log as kelpie_answer. Bookmarks get their own file. Files outside the project are never launched.

## Next steps
Implement in a fresh session, phase by phase, after phone phase 3 has landed.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
