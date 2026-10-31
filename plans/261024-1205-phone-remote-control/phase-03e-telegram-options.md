---
phase: 3e
title: "Telegram: alert + deep link, opt-in low-risk approvals"
status: pending
priority: P2
effort: "3d"
dependencies: [3b, 3c, 3d]
---

# Phase 3e: Telegram options

## Overview
The existing one-way alerts stay. Add alert + deep link (the default when Telegram is on) and an **opt-in** "approve low-risk requests from Telegram", with a clear warning that Telegram can read the content and that there is no Face ID. Outbound long polling only, bound to `user_id` + `chat_id`, per-request nonce and expiry.

## Data flow
```
alert: Raised → Telegram destination → sendMessage (title, text, link) [link when mode ≥ alert_link]
approve: low-risk request → sendMessage + inline keyboard {callback_data: a|d + nonce} → daemon getUpdates (outbound) → callback_query
         → check user_id, chat_id, nonce, expiry, request still waiting → Session::answer_by / Chat::answer (By "telegram", first wins) → audit → edit message, remove buttons
```

## Requirements
- Functional
  - **Mode** on `notify::Telegram` (`notify.rs:94`), serde default for existing users: `alert` (today's text, no link unless `phone_link`), `alert_link` (default when switched on from now on), `approve_low` (opt-in). Existing configurations keep exactly what they send today.
  - **Deep link**: `running_url` + `#s=` (3a prefers the `ts` origin). `disable_web_page_preview` stays on (`notify.rs:926`), so Telegram does not fetch the address. The row warns that Telegram sees the machine name in the link.
  - **Binding**: `chat_id` exists; `user_id` is new. Turning approve on needs `Role::App` + presence and shows a code to send as `/bind <code>` to the bot; the first matching message within 5 min stores both. Change or clear only from the desktop.
  - **Polling**: `getUpdates` long poll on its own thread with its own agent (the 10 s `post_json` timeout, `notify.rs:514`, is too short), started only in `approve_low`, stopped on any mode change. Stored `offset`. A webhook set on the token makes `getUpdates` fail: shown with a plain instruction. Outbound only; no listener.
  - **Filter**: accept a callback only when `from.id == user_id` and `message.chat.id == chat_id`. Anything else is dropped, counted and audited once per minute.
  - **Nonce**: 128-bit random in `callback_data` (limit 64 B [U]), in memory only, single use, expires in 5 min or with the request. Restart invalidates all.
  - **What may be approved**: only requests with level `low` from `mobile/risk.rs` (3c); `unknown`, `medium`, `dangerous` get alert + deep link and no buttons. Only the one-time option: `choice_lasts` (`session.rs`) and `NotAnswered::Lasting` (`chat.rs:210`) rules apply. Deny is offered for the same requests.
  - **Answering**: `Session::answer_by` with `waiting_since` (`session.rs`), `Chat::answer` (`chat.rs:1301`) with `By {id: "telegram", name: "Telegram"}`, first answer wins; a late tap gets "already answered". Chat answers need a line built per dialect: the phone builds these in its adapters today, so the daemon needs an allow-once builder per dialect, or chat approval from Telegram ships only for dialects where one exists [UNVERIFIED, trace in step 1].
  - **Message text**: passes `kelpie_proto::redact` (`notify.rs` already does), command shown with the daemon's label; never the screen tail unless `include_screen` is on.
  - **Warning** (switch and confirm dialog, EN/VI): Telegram can read these messages; a bot chat is not end-to-end encrypted; a tap here has no Face ID; anyone with this Telegram account can approve.
  - **Kill switch**: `mobile.revoke_all`, `mobile.disable`, `mobile.reset` stop polling, burn nonces and remove buttons. Each approval and each refusal goes to the audit log (`mobile/audit.rs`) as device `telegram`.
  - Rate cap: 6 approvals per minute, then alert-only until the next minute.
  - Settings row (3d component): three-position select + warning.
- Non-functional: no thread or socket unless `approve_low` (alerts keep today's on-demand thread).

## Backwards compatibility
`Telegram` fields default so old `notify.json` loads and behaves the same. Bot token stays in the credential store (`K_TELEGRAM_TOKEN`, `notify.rs:38`). `chat.resolved.by` gains `telegram` as a free name. API 26 (for `notify.telegram_bind` and the event field).

## Resilience requirements
- Poll errors: full-jitter backoff 1 s → 60 s; `401` stops polling and shows "bot token rejected"; `409` means another poller or webhook, shown once.
- Never logs the request address: the daemon's filter keeps the HTTP client at `warn` (`docs/architecture.md:225`), which holds for the new agent; test covers it.
- Network down: buttons simply expire; alerts go through the 3b outbox.
- Busy machine: polling is off the event loop; approvals use the same never-shed answer path.

## Related Code Files
- Create: `crates/kelpied/src/notify/telegram_poll.rs`
- Modify: `crates/kelpied/src/notify.rs`, `crates/kelpied/src/notify/destinations.rs`, `crates/kelpied/src/daemon.rs`, `crates/kelpied/src/auth.rs` (`APP_ONLY`), `crates/kelpied/src/chat.rs`, `crates/kelpied/src/mobile.rs` (kill-switch hooks), `crates/kelpie-proto/src/lib.rs`, `app/src/components/settings/mobile-settings.tsx`, `app/src/components/settings/push-settings.tsx`, `app/src/lib/ipc.ts`, `app/src/lib/types.ts`, locales, `docs/architecture.md`, `docs/user-guide.md`, `README.md` (privacy)

## Implementation Steps
1. Trace the answer line per dialect; decide scope; record it here.
2. Mode field, deep link, parity tests for existing users.
3. Binding, poll thread, filter, nonces.
4. Approval flow through the existing answer paths; audit; kill switch.
5. Settings row, warning. Docs. Level bump.

## Tests
- `cargo test -p kelpied` against the local fake (`telegram_base_for`, `notify.rs:499`): old config sends the same text; wrong user, wrong chat, stale, reused or forged nonce ⇒ nothing answered; `dangerous`, `unknown` get no buttons; second tap gets "already answered"; lasting option refused; revoke-all burns nonces; poll stops on mode change; no poll thread in `alert` modes; token absent from logs.
- e2e `scripts/e2e/notify.py` with the fake API, isolated daemon: bind, low-risk approve answers once, desktop answer first ⇒ Telegram told.
- Manual: one real bot from a phone.

## Perf gate
Off or alert modes: no added thread; `approve_low` idle ≤ 1 outbound request per poll timeout.

## Success Criteria
- [ ] Existing Telegram users see no change until they choose one.
- [ ] Turning approve on needs OS verification, shows the warning and binds to one user and chat.
- [ ] Only the bound user can approve, only low-risk, only once, only before expiry; every decision is in the audit log.
- [ ] The kill switch stops it at once.

## Risk Assessment
| Risk | L × I | Mitigation |
|---|---|---|
| Telegram account takeover or SIM swap approves requests | M × H | Opt-in, low-risk only, one-shot, rate cap, audit, warning; off by default |
| Risk label calls a harmful command low | M × H | Allow-list thinking in `risk.rs`; unknown never low; user accepted this on opt-in |
| Bot token leak | L × H | Credential store; binding still required; rotate instruction |
| Bot API drift | L × M | Fake-based tests; adapter is small |

**Rollback**: select back to `alert_link`; revert ignores the new fields.
