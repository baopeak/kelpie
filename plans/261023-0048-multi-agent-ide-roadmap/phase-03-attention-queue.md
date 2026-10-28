---
phase: 3
title: "Attention queue & quick answer"
status: completed
priority: P1
effort: "3d"
dependencies: [1]
---

# Phase 3: Attention queue & quick answer

## Overview
With 10+ agents the question is "who needs me, in what order". Make "needs you" reliable for agents
without hooks, order it by how long each has waited, and let the user answer simple prompts without
switching panes. The dashboard already groups Needs you / Working / Done·unread / Idle
(`dashboard.tsx`); this phase adds what is missing.

## Verified against the code (2026-10-23)
- `Waiting` comes only from hooks (Claude: PermissionRequest / Notification / AskUserQuestion) or a bell
  not preceded by input (`status.rs` `Activity::status`). Codex `notify` reports turn completion only;
  Gemini, OpenCode, Cursor, Copilot, Aider have no hooks — their approval prompts show as Idle.
- Status is recomputed in `Session::tick` (250 ms ticker) from `Activity`; `SessionInfo` has no waiting time.
- The phone's replies are a fixed key table (`mobile.rs` `key_bytes`): nothing to reuse for detection.
- Typing: `sendToSession` / `typeInto` (Codex Enter delay, `typedText`); raw `session.input` has no guard.
- Bell badge = unread inbox count (`status-bar.tsx`); user decision: keep it, add a separate "needs you" dot.

## Requirements
1. **Screen-detected prompts** (daemon, `crates/kelpied/src/choices.rs`): `detect(tail) -> Option<Prompt { keys, question }>`
   looking only at the lines just above the cursor (last ~12 non-empty lines, prompt within the last 3):
   numbered menus (`❯ 1. Yes`, `● 1.`, `1)` … consecutive from 1), `(y/n)` / `[Y/n]` / `(Y)es/(N)o` (Aider),
   `Yes (y)` / `No (n)` style (Codex). Run in `tick` once per quiet period (output stopped ≥ 700 ms and the
   screen changed since the last check) for agent sessions **and** shells; a match sets `Waiting` until
   input or a changed screen without a match. Hooks still win when fresh.
2. `waiting_since` (ms) in `SessionInfo`, set when a session enters `Waiting`, cleared when it leaves.
3. **Dashboard**: Needs you sorted oldest-waiting first with "waiting 3 min"; a **Failed** column
   (`failed` flag); Working; Done·unread; Idle/exited.
4. **Next waiting**: `Ctrl+Shift+.` / `⌘⇧.` and a palette command jump to the session that has waited
   longest, cycling; toast when none. Shortcut passes the conflict checker.
5. **Quick answer** (dashboard card button, inbox "needs you" item, palette "Answer waiting session…"):
   last 12 screen lines (ANSI stripped) + buttons for the detected keys + Enter / Esc + a free-text field.
   Sends through the daemon's `session.answer {handle, waiting_since, data, enter}`, which refuses unless the
   session is still `Waiting` with the same `waiting_since` (no stray keys after a phone or desktop answer
   or a changed prompt). Text goes through `typed_text`; Enter is a separate write after 120 ms (Codex).
6. **Bell**: unread count as today + an accent dot when ≥ 1 session needs you; tooltip shows both counts.
7. `session.answer` is user-only in `auth.rs` (default-deny for sessions); `API_LEVEL` 7.

## Architecture
```
PTY output → Activity (last_output) ──tick──► quiet ≥ 700 ms & screen changed?
                                              └─► choices::detect(text_tail(12)) → screen_prompt / clear
Activity::status: hook (fresh) > bell > screen_prompt (no input since) > output activity
```
`session.read` gains `choices: true` → `{text, prompt?: {keys, question}}` for the popover.

## Related Code Files
- Create: `crates/kelpied/src/choices.rs`, `app/src/components/quick-answer.tsx`.
- Modify: `crates/kelpied/src/status.rs` (screen_prompt in Activity), `crates/kelpied/src/session.rs` (detect in tick,
  waiting_since, `answer`), `crates/kelpied/src/daemon.rs` (`session.answer`, `session.read` choices), `crates/kelpied/src/auth.rs`
  (no change: default-deny covers it; test asserts), `crates/kelpie-proto/src/lib.rs` (API_LEVEL 7), `app/src/lib/types.ts`,
  `app/src/components/dashboard.tsx`, `app/src/components/inbox.tsx`, `app/src/components/status-bar.tsx`, `app/src/lib/commands.ts`,
  `app/src/lib/ipc.ts`, styles, locales, docs. (`sidebar.tsx` is left alone: the mascot work owns it.)

## Implementation Steps
1. `choices.rs` + ≥ 20 fixtures (Claude permission menu, Codex approval, Gemini, Aider, OpenCode, generic `Read-Host (y/n)`,
   and negatives: shell prompt after `cat` of a file containing "(y/n)", numbered lists in prose, diff output).
2. Activity/tick integration, `waiting_since`, `session.answer`, `session.read` choices; API_LEVEL 7.
3. Dashboard ordering + Failed column; next-waiting command + shortcut.
4. Quick answer popover (dashboard, inbox, palette); bell dot.

## Tests
- `cargo test`: fixtures (0 false positives on negatives); status precedence; `session.answer` refuses a changed `waiting_since`
  or a non-waiting session; auth denies it to sessions.
- e2e `scripts/e2e/attention.py` (daemon + GUI): PowerShell `Read-Host "Continue (y/n)"` and a scripted fake agent printing a
  numbered menu → both Waiting with `waiting_since`; ordering; next-waiting cycles; quick answer `y` reaches the right session;
  a second answer with the old `waiting_since` is refused; bell shows unread count + dot.

## Perf gate
- Detection only on quiet-after-output with a changed screen (≤ 0.2 ms per check, measured in a cargo test); no new timers;
  popover lazy; dashboard with 50 sessions renders within one frame; global budgets.

## Success Criteria
- [x] Codex / Gemini / Aider approval prompts appear under Needs you without hooks.
- [x] Answer from anywhere in ≤ 2 keystrokes; stale answers never land.
- [x] No false "needs you" on the negative fixtures.

## Risk Assessment
- False Waiting on output that looks like a menu → prompt must sit at the cursor, fixtures per vendor; signal: users dismissing;
  response: tighten patterns per vendor.
- TUIs that animate while waiting never go quiet → no detection (same as today), documented.
