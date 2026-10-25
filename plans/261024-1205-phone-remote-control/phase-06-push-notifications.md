---
phase: 6
title: "Push notifications"
status: pending
priority: P1
effort: "4d"
dependencies: [3, 5]
---

# Phase 6: Push notifications

## Overview
Reach a locked phone when a session needs the user, with no session content readable by Apple, Google or the relay.
The native app gets **relay push** (content-free APNs/FCM alert); PWA users keep roadmap phase 12's **ntfy** channel, whose `#s=` deep link
opens the session in the phone client (phase 2). Web Push is not built: it needs HTTPS + Home-Screen install (setup the contract rules out)
and would duplicate ntfy. [RT-14] Includes the `notify.rs` delivery refactor this needs.

## Requirements
- Functional
  - **Delivery refactor** (`crates/kelpied/src/notify.rs`): today channels are a closed enum of three global singletons (`:451-475`), delivered serially by one thread with a 10 s timeout and a 3 s retry each (`:683-707`),
    and `active` = ntfy || telegram || webhook (`:141-143`, `:664-667`). Change to a list of **destinations** (global channels + per-device push targets), each with its own bounded queue
    and worker so one broken channel cannot delay another; `active` includes push-enabled devices. Classification, filters, rate limit (1 / session / 30 s) and focus/idle suppression stay as they are. [RT-14]
  - **Relay push** (native): the app's push plugin returns an APNs/FCM token, sent to the desktop over `kelpie-link`. The desktop registers with the push gateway using its **own gateway credential**
    (separate from `room_key`, issued at registration, per-credential rate limits), then calls `POST /v1/push {gateway-auth, token, kind, collapse_id}`. The gateway sends a localized generic alert (`loc-key`, EN/VI in the app) + random nonce — no project or session text. [RT-14]
  - Tap → app opens the attention queue and the session's prompt card; lock-screen actions limited to "Open".
  - Per-device toggle; desktop "Send test notification".
  - Privacy doc: the gateway sees device push tokens and the timing of attention events; self-hosted relays still use the hosted gateway for the store app (its APNs/FCM keys are Kelpie's) or turn native push off. [RT-14]
- Non-functional: needs-you → phone ≤ 10 s p95 even with another channel failing; nothing runs with no destination enabled.

## Architecture
- `crates/kelpied/src/notify/` gains `destinations.rs` (queue per destination) and `relay_push.rs`.
- `relay/src/push.ts` holds the APNs `.p8` key and FCM service account as Worker secrets; fixed payload templates only.
- `apps/phone/plugins/push/` — Swift (`UNUserNotificationCenter`, APNs token) + Kotlin (FCM); tap → deep link.

## Related Code Files
- Create: `crates/kelpied/src/notify/destinations.rs`, `crates/kelpied/src/notify/relay_push.rs`, `relay/src/push.ts`, `apps/phone/plugins/push/**`
- Modify: `crates/kelpied/src/notify.rs`, `crates/kelpied/src/mobile.rs` (token messages), `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`), `relay/src/worker.ts`, `apps/phone/src-tauri/**`, `app/phone/src/*` (permission ask after first pairing), `app/src/components/settings/push-settings.tsx` (destination list, per-device push), locales, `README.md`, `docs/user-guide.md`

## Implementation Steps
1. Confirm roadmap phase 12 committed; refactor delivery into per-destination queues with parity tests.
2. Gateway registration + `/v1/push` + secrets; native push plugin; token exchange; relay-push destination.
3. UX: ask for notification permission after the first successful pairing; test button; docs.

## Tests
- `cargo test`: parity of existing ntfy/Telegram/webhook behaviour; a hung destination does not delay others (fake slow sink); `active` true with only a push device; payload never contains screen text.
- Relay vitest: gateway auth, per-credential rate limit, fixed template (caller text never forwarded).
- e2e `notify.py`: local sinks; needs-you with window closed ⇒ one call per destination; slow ntfy sink does not delay the relay-push sink.
- Manual: real iPhone (APNs) and Android (FCM) locked-phone delivery.

## Perf gate
- Event → enqueue ≤ 1 ms; zero threads/sockets with no destination.

## Success Criteria
- [ ] Locked native-app phone notified ≤ 10 s; PWA users notified through ntfy with a working deep link.
- [ ] Apple/Google/gateway receive no session content (asserted in tests).

## Risk Assessment
- Roadmap phase 12 lands with a different structure; response: adapt the refactor here, never fork the classifier.
- APNs/FCM credential ownership (open question 1); response: native push off until configured.
