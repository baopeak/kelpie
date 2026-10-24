---
phase: 12
title: "Off-machine notify & limit actions"
status: completed
priority: P1
effort: "3d"
dependencies: [1, 3, 5]
---

# Phase 12: Off-machine notify & limit actions

## Overview
Reach the user away from the computer and act on usage limits. Redaction now ships in phase 1 and is consumed here.

## Requirements
- **One event source** [RT-11]: move the "is this a notification event" classification out of `maybeNotify` (`store.ts:253-283`) into the daemon (`notify_events`);
  the app consumes the same events for inbox + desktop notifications; the app reports window focus/visibility to the daemon (`ui.focus`, capability `app`).
- Channels (existing `notifications` section in `interface-settings.tsx`) [RT-11]: ntfy (server, **random 128-bit topic by default**, optional access token),
  Telegram (bot token + chat id, test), webhook. All secrets and the topic via `kelpie_proto::secret_*`; config changeable only through the app connection; never log request URLs.
- Payload: project, session title (redacted), status, duration; screen text off by default (explicit warning to enable). Deep links never contain pairing codes or tokens —
  only a URL that still needs an existing paired device.
- Events/filters: needs-you, done, failed, automation finished, run approval, cost cap; "turn > N s"; "window not focused / computer idle"; rate limit 1 msg / session / 30 s.
- **Continue at reset** [RT-11]: timer stores `{session_id, generation, agent_session_id}`; reset time only from usage meter `resets_at` (or user-confirmed);
  at fire time types `continue` via the `session.answer` guard (phase 3) only if same generation, same agent session, still failed by usage limit and no input since — else cancel + inbox note.
- **Switch account** [RT-11]: copy the transcript into the target account home when the vendor layout allows, then new RPC `session.set_env_restart`; refuse with explanation otherwise.
- **Hand off to…** (existing).

## Related Code Files
- Create: `crates/kelpied/src/notify.rs`, `crates/kelpied/src/scheduled_input.rs`.
- Modify: `crates/kelpied/src/daemon.rs` (events, ui.focus, set_env_restart, API_LEVEL), `crates/kelpied/src/persist.rs`, `crates/kelpie-proto/src/lib.rs`, `app/src/lib/store.ts`
  (consume daemon events), `app/src/components/settings/interface-settings.tsx`, `app/src/components/pane-view.tsx` (limit banner), `app/src/components/inbox.tsx`,
  `app/src-tauri/src/accounts.rs` (transcript copy), `app/src/lib/accounts.ts`, locales, docs.

## Implementation Steps
1. Daemon event classification + app migration (desktop notifications unchanged in behaviour). 2. Channels + secrets + filters + rate limit. 3. Limit banner + timer guard + switch account.

## Tests
- `cargo test`: event parity with old `maybeNotify` cases; payload redaction; rate limiter; timer cancels on generation change; switch account refuses when copy impossible.
- e2e `notify.py`: local sink as ntfy + webhook; needs-you with window closed → 1 POST; desktop + push agree on suppression; continue fires only when still limited.
- Manual once: real Telegram bot.

## Perf gate
- No sockets/timers with no channel configured; event classification adds ≤ 0.1 ms per status change.

## Success Criteria
- [x] Phone notified with the window closed; desktop and push consistent; no blind typing.

## Risk Assessment
- Public ntfy topic guessing → random topic + optional token; signal: unexpected subscribers (ntfy stats); response: rotate topic button.
