---
phase: 3b
title: "Web Push sent directly by kelpied"
status: in-progress
priority: P1
effort: "4d"
dependencies: [3a, "roadmap phase 12"]
---

# Phase 3b: Web Push from kelpied

## State of this phase
**Crate gate (step 1), 2026-11-01:** `cargo tree -i openssl` on a scratch project: `web-push 0.11.0` → `ece 2.4.2` → `openssl 0.10.81`, whatever the features. So `notify/web_push.rs` on RustCrypto (`p256` 0.13, `hkdf` 0.12, `aes-gcm` 0.10, `sha2`), checked byte for byte against the example of RFC 8291 appendix A, and against the signature in RFC 8292 section 2.4.

**Sending one real push to a real iPhone and Android from step 1 is not done:** no person with phones was at hand, and the rule for this run is that nothing goes to a real push service. What a person still needs to check is in [the report](../reports/fullstack-developer-261101-0920-phone-phase3b.md).

**Decisions after the reviews, 2026-11-01** (for the owner, keeping master's behaviour for existing users): a webhook's 5xx other than 502 and 503 is tried twice at most, which narrows "retry on 5xx" under Resilience, since a flow can act before it fails. Under load only a phone's alerts coalesce: a channel keeps each message with its title, text and link, which narrows "other events coalesce" for channels. A later alert of the same kind for a session replaces the last in silence: left as it is.

## Overview
Reach a locked phone when a session needs the user. The daemon signs with its own VAPID key and POSTs an RFC 8291-encrypted, content-free alert with a deep link to the phone's push service. No server of ours, no Apple or Google account. Replaces the "no Web Push" position in old phase 6 and RT-14 (user decision 2026-10-30). Includes the `notify.rs` delivery refactor and durable outbox moved from phase 6.

## Data flow
```
phone (installed PWA, gesture) → pushManager.subscribe(VAPID public key) → verb push_sub {endpoint, p256dh, auth, lang} over Noise → device record
Notifier::observe → wants_push → per-destination queue → outbox → encrypt (aes128gcm) + VAPID JWT → POST endpoint (outbound HTTPS only)
push service → phone → notification (title, generic text, navigate https://<fqdn>:8443/#s=<id>) → tap → app opens → lock (3c) → session
```

## Requirements
- Functional
  - **Keys**: VAPID P-256 key made on first enable, kept with `kelpie_proto::secret_*` under the endpoint's own service (`K_*` pattern in `mobile.rs`, `notify.rs:38`); the public key reaches the phone in `hello` and on `push_info`.
  - **Encryption and signing**: payload encrypted to the subscription's `p256dh` + `auth`; JWT `aud` = endpoint origin, `exp` ≤ 24 h, `sub` a project URL (no personal data; Apple's acceptance to be checked in step 1).
  - **Crate gate** (step 1): the user chose the `web-push` crate. It depends on `ece` 2.2 without turning off default features, and `ece`'s default and only real backend is OpenSSL (docs.rs manifests read 2026-10-30), while this repo keeps OpenSSL/aws-lc/NASM out of Windows builds (phase 4 `relay.rs`, phase 8 `str0m`). Check `cargo tree -i openssl -p kelpied`. If OpenSSL is pulled, use a ~150-line `notify/web_push.rs` on RustCrypto (`p256`, `hkdf`, `aes-gcm`, `sha2`), checked against the RFC 8291 test vector, keeping the same wire result. Reported to the user as an open question.
  - **Sending**: over the daemon's existing `ureq` client (`post_json`, `notify.rs:512`, no redirects). Headers `TTL: 600`, `Urgency: high` for needs-you, `Topic` = short hash of session + wait id, so a late delivery replaces rather than stacks.
  - **Payload**: Declarative Web Push JSON `{web_push: 8030, notification: {title, body, navigate, tag}}`, read natively on iOS 18.4+ and by the service worker elsewhere. Title "Kelpie", body a fixed localized string per kind (EN/VI from `lang`), no project, session or command text. ≤ 2 KiB. The session id in `navigate` is encrypted in transit and opaque.
  - **Service worker** (`app/phone/src/sw.ts`): adds `push`, `notificationclick` (focus or open `navigate`), `pushsubscriptionchange` (marks a re-subscribe for the next open).
  - **Phone UI**: "Turn on alerts" button (permission needs a user gesture), only in a secure context and on iOS only when installed; "Send a test alert"; re-check of the subscription on every open. Plain `http://` shows why not and points at Tailscale or ntfy.
  - **SSRF guard** [RT-14]: the endpoint comes from a phone. Accept `https` only, hosts on a closed list (`*.push.apple.com`, `fcm.googleapis.com`, `*.push.services.mozilla.com`, `*.notify.windows.com` [last two UNVERIFIED against current browsers]), length-capped, no redirects, errors never echo the address.
  - **Lifecycle**: 404/410 removes the subscription; revoke, reset, expiry and kill switch remove it synchronously; `push_sub` / `push_unsub` / `push_info` / `push_test` join the closed verb table (`verbs.rs`), `read` permission, secure transport only, refused while locked (3c).
  - **Delivery refactor** (`notify.rs`): today one thread walks three channels serially with a 10 s timeout and a 3 s retry (`:751-760`), so one dead channel delays the rest. Change to a list of destinations (global channels + one per subscribed device), each with its own bounded queue and worker; `any_enabled` (`:147`) and `active` (`:712`) count subscribed devices. Classification, filters, rate limit and focus suppression (`wants_push`, `:718`) unchanged. [RT-14]
  - `Channel` gains `WebPush`; `notify.test`, `describe` and the status list include it.
- Non-functional: needs-you → push service accepted ≤ 2 s p95 even with another channel failing; nothing runs with no destination.

## Backwards compatibility
Existing ntfy, Telegram and webhook configuration and behaviour unchanged (parity tests). New record fields default empty. Lost-contact push (2026-10-25) cannot be sent by a dead desktop and is deferred; see phase 6 and the open questions. API 24.

## Resilience requirements
- **Durable outbox** per destination (on disk, ≤ 50 entries, atomic write): re-check "still waiting" right before every attempt, drop if resolved; retry 1, 2, 4, 8, 16, 32, 60 s with full jitter on network errors, 5xx, 429 (honour `Retry-After`), none on other 4xx; max age 10 min; after an outage **one** summary ("N sessions need you"). Sink 503×3 then 200 ⇒ one delivery ≤ 1 min late; sink down 12 min while the session got answered ⇒ none.
- Phone: tap resolves against synced state ("already handled" when the wait is gone); duplicate tags collapse.
- Host overload: attention events never delayed > 1 s at any load level; delivery never runs on the classifier thread; other events coalesce 1 s at L1, digest 5 s at L2.
- Sleeping computer: no alert is possible; Settings says so and suggests the existing keep-awake mode.

## Related Code Files
- Create: `crates/kelpied/src/notify/destinations.rs`, `notify/outbox.rs`, `notify/web_push.rs`, `app/phone/src/push.ts`
- Modify: `crates/kelpied/src/notify.rs`, `crates/kelpied/src/mobile.rs`, `mobile/verbs.rs`, `mobile/store.rs`, `crates/kelpied/src/daemon.rs`, `crates/kelpied/Cargo.toml`, `crates/kelpie-proto/src/lib.rs` (`API_LEVEL` 24), `app/phone/src/sw.ts`, `app/phone/src/app.tsx`, `app/phone/src/link.ts`, `app/src/lib/types.ts`, locales, `docs/architecture.md`, `docs/user-guide.md`, `README.md` (privacy)

## Implementation Steps
1. Crate gate; RFC 8291 vector; send one real push to iPhone and Android from a scratch test. Record the result here.
2. Delivery refactor with parity tests, outbox, then the `WebPush` destination.
3. Verbs, subscription store, SSRF guard, lifecycle.
4. Service worker and phone UI, test alert.
5. Docs. Level bump. Start the iOS soak.

## Tests
- `cargo test -p kelpied`: RFC 8291 vector; JWT claims; endpoint allow-list (private IP, http, redirect, odd host refused); 410 removes; revoke removes; payload never contains session text (asserted over each kind); parity for ntfy/Telegram/webhook; a hung destination does not delay others; `active` true with only a subscribed device; outbox cases above; no thread or socket with no destination.
- `pnpm exec vitest run`: subscribe state machine, re-subscribe on open, click target.
- e2e `scripts/e2e/notify.py`: local fake push service over a test-only base address (`KELPIE_E2E` rule), window closed ⇒ one call per destination; slow ntfy does not delay Web Push.
- Manual: real iPhone + Android, phone locked; p50/p95 over 100 events; **7-day iOS soak** (open question in the research).

## Perf gate
Event → enqueue ≤ 1 ms; zero threads and sockets with no destination.

## Success Criteria
- [ ] A needs-you event reaches a locked iPhone (installed PWA) and Android phone; delivery p50/p95 measured and reported.
- [ ] A capture at the fake push service holds only ciphertext; the decrypted payload has no session text.
- [ ] Tap opens the right session after the lock.
- [ ] No alert lost or duplicated across a 12-min outage; one summary after recovery.

## Risk Assessment
| Risk | L × I | Mitigation |
|---|---|---|
| `web-push` drags in OpenSSL | H × M | Step 1 gate; RustCrypto module |
| iOS push flaky over days | M × H | Soak; re-subscribe on open; test button; ntfy/Telegram stay as second channels |
| Phone-supplied endpoint abused | M × H | Closed host list, https only, no redirects |
| Roadmap phase 12 structure drift | L × M | Adapt the refactor; never fork the classifier |

**Rollback**: per-device off; revert leaves subscriptions unused; a kept outbox file is ignored by older daemons.
