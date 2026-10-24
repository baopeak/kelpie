# Completion — Phase 12: Off-machine notify & limit actions

Date: 2026-10-25 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-12](../261023-0048-multi-agent-ide-roadmap/phase-12-notify-limits-redaction.md)

## Delivered
- **One event source**: the daemon classifies notification events (`notify.rs`); the app consumes them for the inbox and desktop notifications (behaviour unchanged) and reports focus, visibility and last input (`ui.focus`). Away = window not focused, or the computer idle ≥ 5 min (Windows system clock; elsewhere the app's last input).
- **Channels**: ntfy (random 128-bit topic, optional token, rotate), Telegram (bot token + chat id, test), webhook. Secrets via `kelpie_proto::secret_*`, keyring service scoped per endpoint; config only from the app connection; TLS on; the HTTP client never logs above `warn` (no token or topic in logs at any `KELPIE_LOG`). Payload redacted, screen text off by default; phone link carries no pairing code or token (LAN-address link has its own switch). Filters, 1 message / session / 30 s (cost cap has its own allowance), bounded send queue with drops counted.
- **Continue at reset** (`scheduled_input.rs`): timer `{session, generation, agent session, turn}`, reset time from the usage meter or user-confirmed; types `continue` through the `session.answer` guard only if the turn still failed on the limit, no draft was typed since the turn began (answers to dialogs excepted) and nothing since the failure; else cancels with an inbox note. Persisted (`scheduled.json`); timers lost to a restart become notes. Forged limit reports cannot re-arm a cancelled timer; once a session proves its token on `hook.report`, unproven reports are ignored.
- **Switch account**: copies the transcript (sub-agents included) into the target account home, replacing only an older copy of the same conversation, then `session.set_env_restart`; refuses with a reason otherwise.
- Limit banner for Claude terminal sessions; settings and banner lazy (entry 319 kB gz). API level 12.

## Verification
- cargo: kelpied 183 (event parity with every old `maybeNotify` case, redaction, rate limiter, timer guard incl. draft-before-dialog matrix, persistence, switch-account refusals, no sockets/timers without a channel), kelpie 107, kelpie-proto 14; clippy `-D warnings` clean. Classification 1.19 µs per status change (budget 100 µs).
- vitest 228, typecheck, i18n 0 missing; daemon bench 3.75 s (limit 3.91).
- GUI e2e `notify.py` on an isolated daemon with a local sink as ntfy/webhook/Telegram: 73/73 (twice, then once after each later fix); real trace log checked for secrets. Regression: `attention.py`, `context_cost.py`, `checkpoints.py`, `finish.py` green.
- Staged commit built and tested alone in a clean worktree (other sessions' uncommitted work excluded).
- [Review](code-reviewer-261024-1930-phase12-notify-limits.md): H1–H4, M1–M4, RR-M1–M3, FC-1, FC-L1/L2 and the cheap Lows fixed; no safety blocker left.

## Not done / follow-ups
- Manual: a real Telegram bot, real ntfy.sh, a real Claude usage limit, a copied Codex conversation resumed in Codex, a real phone opening the link.
- GUI perf group and `ui_tour.py` not run; the non-Windows idle path is unit-tested only.
- No limit signal (banner, continue) for Codex or chat sessions; L11 and RR-L2 accepted.
