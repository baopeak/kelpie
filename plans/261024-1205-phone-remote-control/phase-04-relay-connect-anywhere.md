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
  - **Client** (`kelpie-link::client`): LAN addresses (QR + last known, 1.5 s timeout) in parallel with the relay; first completed IK handshake wins; on relay, probe LAN every 30 s and migrate
    (new handshake on direct, rebind subscriptions with per-device resume, then close relay socket). Control grants re-bind to the new connection only after the same device's handshake succeeds. [RT-12]
  - Settings → Mobile: toggle, relay URL (default / custom), "What the relay can see" (ciphertext, IPs, timing, sizes, room id), status.
- Non-functional: ≤ 30 ms median added in-region; zero cost when off; Worker CPU ≤ 1 ms per frame.

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
- Off: zero outbound connections. On, idle: ≤ 1 keepalive / 30 s.

## Success Criteria
- [ ] Test client on a different network reaches a desktop behind NAT with only the QR (native app UX lands in phase 5).
- [ ] Relay operator sees no content; revoke enforced at the relay even if it happened offline.
- [ ] Self-host doc reproduces the relay with one command.

## Risk Assessment
- DO limits/pricing change; response: same code on a `workerd` VPS.
- `tokio-tungstenite` features pull aws-lc; signal: NASM error; response: explicit ring provider (as `mobile.rs` does).
- Relay abuse as a tunnel; signal: rooms near caps; response: tighter caps, admission only with recent pairing.
