---
phase: 6
title: "DEFERRED: Native push (APNs/FCM gateway), sealed details, lost-contact"
status: pending
priority: P3
effort: "3d (deferred, not in the total)"
dependencies: [3, 5]
---

# Phase 6: Native push

> **REWORKED AND DEFERRED (user decision 2026-10-30).** Push is now Web Push sent by `kelpied`: see [phase 3b](phase-03b-web-push.md). **Moved to 3b**: the `notify.rs` delivery refactor, the durable outbox, overload rules. **Superseded**: "Web Push is not built" and RT-14's dropping of Web Push (SSRF kept as a 3b requirement). **Deferred here, with the native app (phase 5)**: APNs/FCM gateway, `relay/src/push.ts`, NSE sealed details, and the 2026-10-25 lost-contact push. If this returns, gateway secrets live in a separate Worker and account, not in the relay data path (phase 4). The text below is kept as reference.
>
> **Lost-contact needs a user decision**: a dead desktop cannot send Web Push, so that 2026-10-25 decision has no delivery path now (open question in the plan).

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
- Non-functional: needs-you → gateway accepted ≤ 2 s p95 even with another channel failing (device delivery measured, see below); nothing runs with no destination enabled.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
- **Durable outbox** per destination (on disk, ≤ 50 entries) replacing the single 3 s retry: state-based send (re-check "still waiting" right before every attempt, drop if resolved); retry 1, 2, 4, 8, 16, 32, 60 s with full jitter on network errors / 5xx / 429 (honour `Retry-After`), none on other 4xx; max age 10 min; after an outage, **one** summary ("N sessions need you") instead of a burst of stale pushes. Tests: sink 503×3 then 200 ⇒ exactly one delivery ≤ 1 min late; sink down 12 min while the session got answered ⇒ none.
- Gateway: `apns-expiration = now + 600 s`, FCM `ttl = 600s`, collapse key = session id + wait id (a late delivery replaces, never stacks), high priority for needs-you; the app dedupes by nonce/wait id and always resolves a tap against synced state ("already handled" if the wait is gone).
- **Lost-contact push** (opt-in per device, default off): when the phase-4 lease alarm fires (desktop gone 90 s while agents were running), the gateway sends **one** push per opted-in device — `computer_asleep` if a `going_to_sleep` frame preceded the loss, else `lost_contact` — collapse id per room, at most 1 per room per hour until the desktop returns; no "back online" push by default.
- **Sealed push details** (all push kinds): the desktop attaches a blob sealed to each device's key (e.g. "3 agents running in api-server, last seen 14:02"; needs-you: project + redacted session title). iOS Notification Service Extension (`mutable-content: 1`) and the Android app decrypt it locally and show the details; on any failure the generic localized text stays. Apple, Google and the relay see only ciphertext. Payload ≤ 2 KiB after sealing (APNs limit 4 KiB).
- Host overload: attention events are never delayed > 1 s at any load level; other events coalesce 1 s at L1, digest 5 s at L2; delivery work never runs on the classifier thread.
- Success criterion rewritten: the gateway accepts the push ≤ 2 s p95 after the event (controllable); device delivery p50/p95 is **measured** on real iOS + Android over 100 events and reported (APNs/FCM give no latency SLA).

## Architecture
- `crates/kelpied/src/notify/` gains `destinations.rs` (queue per destination) and `relay_push.rs`.
- `relay/src/push.ts` holds the APNs `.p8` key and FCM service account as Worker secrets; fixed payload templates only.
- `apps/phone/plugins/push/` — Swift (`UNUserNotificationCenter`, APNs token) + Kotlin (FCM); tap → deep link.

## Related Code Files
- Create: `crates/kelpied/src/notify/destinations.rs`, `crates/kelpied/src/notify/relay_push.rs`, `crates/kelpied/src/notify/outbox.rs`, `relay/src/push.ts`, `relay/src/lease.ts`, `apps/phone/plugins/push/**`, `apps/phone/ios-notification-service/` (NSE target)
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
- [ ] Gateway accepts ≤ 2 s p95; locked-phone delivery measured and reported (p50/p95 over 100 events, iOS + Android); PWA users notified through ntfy with a working deep link.
- [ ] Offline desktop: no push lost or duplicated across a 12-min outage; one summary after recovery; lost-contact push at 90–100 s while a lease is held; none without one.
- [ ] Apple/Google/gateway receive no session content (asserted in tests).

## Risk Assessment
- Roadmap phase 12 lands with a different structure; response: adapt the refactor here, never fork the classifier.
- APNs/FCM credential ownership (open question 1); response: native push off until configured.
