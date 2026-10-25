---
phase: 4
title: "Relay: connect from anywhere"
status: pending
priority: P1
effort: "4d"
dependencies: [3]
---

# Phase 4: Relay — connect from anywhere

## Overview
An opt-in, untrusted relay so the phone reaches the desktop from any network with no setup and no account. The desktop dials out;
the relay forwards opaque `kelpie-link` frames. One TypeScript implementation runs on Cloudflare Workers + Durable Objects (hosted default)
and on open-source `workerd` (self-host). The client side (connect race, migration) is written once in Rust (`kelpie-link` feature `client`) and used by the native app (phase 5) and tests.

## Requirements
- Functional
  - **Room**: random `room_key` (32 B, keyring); `room_id = H(room_key)` — random, not derived from `dpk`, carried in the QR; relay stores only `room_id` and `H(room_key)`.
    Unused rooms expire after 90 days; room creation rate-limited per IP. [RT-15]
  - **Phone admission**: desktop pushes an allow-list of `H(device_relay_token)`; tokens issued over `kelpie-link` after pairing (native devices only — PWA devices never get one, phase 3).
    Pairing via relay: desktop registers `H(pair_token)` (TTL 120 s, single use), `pair_token = HKDF(psk, "kelpie-relay-pair")`.
  - **Pairing independent of the LAN listener**: with the relay on, `pair_start` works even when no LAN address binds (today it bails, `mobile.rs:192-193`). [RT-15]
  - **Forwarding**: binary frames ≤ 64 KiB; slot id per phone socket; ≤ 8 phones per room; `desktop_offline` frame; nothing queued or stored.
  - **Limits**: per-room 4 MB/s burst and 20 GB/month soft cap for session traffic; browser-takeover frames (phase 7) have their own 200 MB/h budget outside the room cap; violations close with explained codes.
  - **Revoke while offline**: pending allow-list removals persisted on the desktop and applied first on relay reconnect. [RT-12]
  - **Logging**: counters and error codes only; no payloads; no persisted IPs (Cloudflare edge logs documented).
  - **Daemon**: outbound WSS only when "Connect from anywhere" is on (user presence required, phase 3); jittered backoff 1 s → 60 s; each slot feeds the same link handler as `/link`.
  - **Client** (`kelpie-link::client`): LAN addresses (QR + last known + `addrs` updates) first, relay after a 250 ms stagger (details in Resilience requirements); first completed IK handshake wins; on relay, re-probe LAN on interface change and migrate
    (new handshake on direct, rebind subscriptions with per-device resume, then close relay socket). Control grants re-bind to the new connection only after the same device's handshake succeeds. [RT-12]
  - Settings → Mobile: toggle, relay URL (default / custom), "What the relay can see" (ciphertext, IPs, timing, sizes, room id), status.
- Non-functional: ≤ 30 ms median added in-region; zero cost when off; Worker CPU ≤ 1 ms per frame.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
- **Relay signals**: `desktop_offline` / `desktop_online`, `peer_reset` to the surviving side when either socket drops, a distinct **restart** close code (code deploys drop every DO socket) on which clients add 0–10 s jitter; desktop sockets carry a generation counter — a new desktop socket replaces the old one (last wins) even before the old one is noticed dead.
- **Keepalive**: relay↔desktop and relay↔phone edge ping/pong every 20 s via DO `setWebSocketAutoResponse` (no wake, no duration billing); end-to-end liveness of the desktop comes from the phase-1 `hb` inside Noise plus `desktop_offline`. Desktop declares the relay dead after 45 s (90 s at L2+, lag-aware). Perf gate changes from "≤ 1 keepalive / 30 s" to **≤ 1 per 20 s per socket**.
- **Backoff and triggers** (desktop → relay): full-jitter 1 s → 60 s, reset to attempt 0 on a network-change event (debounced 2 s) and after 30 s stable. Test: kill/restart `workerd` 20× with 8 phones ⇒ all reconnect ≤ 15 s, ≤ 3 attempts per client per 10 s.
- **Connect race refined**: LAN candidates first, relay after 250 ms (happy-eyeballs stagger) instead of a flat 1.5 s LAN timeout; skip RFC1918 candidates that are not on-link when the phone is on cellular (Tailscale 100.64/10 always eligible); "probe LAN every 30 s" replaced by interface-change triggers (+30 s only on Wi-Fi with an unknown subnet); direct → relay fallback immediate, relay → direct only after direct has been stable 10 s (no flapping).
- **Budgets survive eviction**: rate and byte counters (20 GB/month, takeover budget) persisted to DO storage, batched every 5 s or 1 MB.
- `relay unreachable` reported distinctly from `computer offline`; direct retried every 30 s while the relay is down; room created with `locationHint` from the desktop; "≤ 30 ms added in-region" stated as in-region only.
- **Lost-contact lease** for phase 6 (user decision 2026-10-25): instead of an on/off bit, the desktop holds a lease on its room only while ≥ 1 session is Working/Waiting **and** ≥ 1 device opted in; the lease is renewed by the existing 20 s heartbeat (no extra traffic) and carries one sealed blob per opted-in device (encrypted to that device's key — the relay forwards it unread). On desktop socket loss the room's Durable Object sets `storage.setAlarm(now + 90 s)` (works on `workerd`); desktop back before it fires ⇒ alarm cancelled. A `going_to_sleep` frame sent on OS suspend (`PBT_APMSUSPEND` / `NSWorkspaceWillSleepNotification`) marks the loss as sleep. Relay learns only *when* a lease exists (documented); default off.
- Settings → Mobile suggests the existing keep-awake mode "while agents are working" when phone access is turned on (no new keep-awake behaviour — user decision 2026-10-25).
- Host overload: relay pairing admission follows the phase-1 load level; overloaded desktop answers new screen/takeover requests with `host_busy` (retry-after 10 s).
- Tests with the TCP fault proxy (phase 1 harness): blackhole 10/30/90 s, half-open, RST storm, NAT rebinding (proxy restart on a new port), relay deploy.

## Architecture
```
relay/
  src/worker.ts   // /v1/desktop, /v1/phone, /v1/health (+ /v1/push phase 6, /v1/turn phase 8)
  src/room.ts     // Durable Object per room, WebSocket Hibernation, allow-lists, limits
  wrangler.toml · workerd/ (config.capnp + Dockerfile) · test/ (vitest + @cloudflare/vitest-pool-workers)
```
- Daemon `crates/kelpied/src/relay.rs`: `tokio-tungstenite` without default features + rustls **ring** provider + webpki roots (no aws-lc/NASM).
- PWA stays LAN/Tailscale (no service worker on `http://`, origin unreachable away from home); the relay never serves code.

## Related Code Files
- Create: `relay/**`, `crates/kelpied/src/relay.rs`, `crates/kelpie-link/src/client.rs`, `docs/relay-self-host.md`
- Modify: `crates/kelpied/Cargo.toml`, `crates/kelpie-link/Cargo.toml` (feature `client`), `crates/kelpied/src/mobile.rs`, `crates/kelpied/src/daemon.rs` (`mobile.set_relay`, `API_LEVEL`), `app/src/components/settings/mobile-settings.tsx`, locales, `README.md`, `docs/architecture.md`, `.github/workflows/ci.yml`

## Implementation Steps
1. Relay Worker + DO: admission, forwarding, limits, expiry; vitest.
2. `workerd` self-host config + Dockerfile + doc.
3. Daemon relay client, slots, allow-list sync incl. persisted removals, pairing without LAN bind.
4. `kelpie-link::client` connect race + migration; test harness client.
5. Settings UI + explainer; hosted deploy (maintainer, open question 1).

## Tests
- Relay vitest: wrong `room_key`, unknown device token, expired/reused pair token, oversize frame, caps, takeover budget separate, desktop_offline, hibernation resume, room expiry.
- `cargo test`: backoff; persisted removals applied first; relay off ⇒ no socket; client race picks first handshake; migration keeps subscriptions; control grant not re-bound before handshake.
- e2e with local `workerd`: Rust test client pairs over relay only (LAN bind failing), sessions work, revoke while relay socket down is enforced on reconnect, LAN appears ⇒ migration; relay capture has no plaintext.

## Perf gate
- Off: zero outbound connections. On, idle: ≤ 1 keepalive per 20 s per socket (carrier NAT idle timeouts can be 30 s).

## Success Criteria
- [ ] Test client on a different network reaches a desktop behind NAT with only the QR (native app UX lands in phase 5).
- [ ] Relay operator sees no content; revoke enforced at the relay even if it happened offline.
- [ ] Self-host doc reproduces the relay with one command.

## Risk Assessment
- DO limits/pricing change; response: same code on a `workerd` VPS.
- `tokio-tungstenite` features pull aws-lc; signal: NASM error; response: explicit ring provider (as `mobile.rs` does).
- Relay abuse as a tunnel; signal: rooms near caps; response: tighter caps, admission only with recent pairing.
