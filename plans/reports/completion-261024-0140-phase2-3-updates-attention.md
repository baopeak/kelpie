# Completion — Phase 2 (auto-update) and Phase 3 (attention queue)

Date: 2026-10-24 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md)

## Phase 2 — updates keep sessions running (commit 978699e)
- Signed in-app updates (GitHub Releases, minisign key in repo secret `TAURI_SIGNING_PRIVATE_KEY`), status-bar chip, install now / when idle / skip.
- Design changed by the user mid-phase: the daemon runs from a copy in `<data dir>/daemon/a|b`, so installs replace the app only; the older daemon serves its sessions until none runs (`handOverIfIdle`, `daemon.shutdown {if_idle}`) or "Switch now"; `DAEMON_SECURITY_LEVEL` keeps a status-bar reminder.
- Installer hooks stop only programs directly in the install folder (0.1.x layout); uninstall stops and removes the copies.
- Release workflow: universal macOS + Windows, draft releases.
- Verified live on the user's machine: 0.1.5 → 0.1.6 (one-time stop of the legacy daemon, sessions resumed), 0.1.6 → 0.1.7 with sessions running: same daemon pid, every session alive with the same pid, a ticking shell's longest gap 0.7 s, app reopened and reattached. e2e `updater.py` 24/24.
- Reviews: [phase 2](code-reviewer-261023-1930-phase2-auto-update.md), [keep-sessions design](code-reviewer-261023-2215-update-keeps-sessions.md) — all High/Medium fixed.

## Phase 3 — attention queue
- Daemon: prompt-on-screen detection (`choices.rs`: menus, `(y/n)`, `[Y/n]`, `(Y)es/(N)o`, `Yes (y)`, PowerShell `[Y] Yes … [N] No`, `git add -p`, OpenCode `Allow (a)/Deny (d)`), only at the cursor (not under a shell prompt or input box); one wait per prompt across repaints; an answered prompt still on screen is not a new wait; focus reports are not input. `waiting_since`, `session.read` prompt + `waiting_since`, `session.answer` (atomic check+write, user-only). API 7.
- App: dashboard Needs you (oldest wait first, time, Answer) + Failed column; quick-answer popover locked to the prompt it shows; `Ctrl+Shift+.` next waiting; bell dot; inbox "Needs you"; hidden on daemons below API 7.
- Tests: cargo 83 (18 detection fixtures), vitest 110, e2e `attention.py` 12/12 (incl. repaint flicker), GUI `attention_gui.py` 31/31 (dashboard, popover, inbox, shortcut, stale lock).
- Review: [phase 3](code-reviewer-261024-0100-phase3-attention.md) — 3 blockers fixed (repaint flicker, popover guard, idle-shell false positive); tester found 2 more in the GUI (focus reports, answered menu re-detected) — fixed. [tester report](tester-261024-0100-phase3-attention.md).

## Unresolved
- Vendor fixtures are written from documentation, not captured from every real CLI (only Claude, Codex, OMP are installed here).
