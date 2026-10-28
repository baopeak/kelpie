# Phase 5 — Off-machine notifications & usage-limit actions

## Context
Notifications are desktop-only and come from the UI ([store.ts:250](../../app/src/lib/store.ts#L250));
phone access needs an open page. Usage limits are detected
([usage-meter.tsx:144](../../app/src/components/usage-meter.tsx#L144)) and failed turns are flagged
([status.rs:45](../../crates/kelpied/src/status.rs#L45)) but nothing acts on them.

## Requirements
- **Channels** (Settings → Notifications): ntfy (server URL default ntfy.sh, topic), Telegram (bot
  token + chat id, "send test"), generic webhook (POST JSON). Secrets in keyring.
- **Sent by the daemon** so they work with the window closed; events: needs-you, done, failed,
  automation finished, run approval needed. Filters: per event, "only if the turn ran > N s",
  "only when the computer is idle / window not focused".
- Payload: project, session title, status, duration; terminal text off by default.
  ntfy click URL = phone-access deep link when phone access is on.
- **Limit actions** on a session failed by a usage limit (inline banner + inbox item):
  "Continue at <reset time>" (daemon timer types `continue` + Enter, cancelable),
  "Switch to account <B>" (same vendor, resume the conversation under B's home),
  "Hand off to <agent>" (existing hand-off).

## Files
- Create: `crates/kelpied/src/notify.rs` (channels, rate limit 1 msg / session / 30 s, retries),
  `crates/kelpied/src/scheduled_input.rs` (one-shot timers, persisted with sessions).
- Modify: `crates/kelpied/src/daemon.rs` (status transitions → notify), `crates/kelpied/src/persist.rs`,
  `crates/kelpie-proto/src/lib.rs`, `app/src-tauri/src/integrations.rs` (keyring helpers reuse),
  `app/src/components/settings/system-settings.tsx` (channels UI), `app/src/components/pane-view.tsx`
  (limit banner), `app/src/components/inbox.tsx`, `app/src/lib/accounts.ts`, locales, docs.
- Reset time: from the usage meter's `resets_at` (Claude/Codex); fallback: parse the agent's
  "resets at" text in the last screen lines.

## Validation
- `cargo test`: payload builders, filters, rate limiter, timer persistence across daemon restart.
- e2e `scripts/e2e/notify.py`: local HTTP sink as ntfy + webhook; session goes needs-you with the
  window closed → one POST; filter "> 30 s" suppresses short turns; scheduled continue fires
  (short fake reset) and is cancelable.
- Manual: real Telegram bot once.

## Risk / rollback
- Leaking code to third parties → terminal text off by default, explicit toggle with warning.
- Spam → rate limit + per-event toggles. Rollback: channels off by default.
