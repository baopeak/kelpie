---
phase: 3
title: "kelpie-link: E2E pairing, trusted approval, device store v2"
status: in-progress
priority: P1
effort: "6d"
dependencies: [2]
---

# Phase 3: kelpie-link

> **Addendum 2026-10-30.** Absorbed during implementation: SAS from the handshake hash with 3 tries, remote-forbidden operations (one-shot approvals), audit log, kill switch, dormant-device expiry, noble JS crypto (`app/phone/src/noise.ts`) instead of the wasm build. Path D later changes lines below: relay tokens are no longer native-only (phase 4), `control` for the PWA awaits the user (phase 7), and `src/client.rs` is deferred with the native app. Phase 3 itself stays as implemented; those changes land in 3a, 3c and 4. API 22.

## Overview
One Rust crate that pairs a phone by QR and carries every phone message end-to-end encrypted (Noise), independent of the pipe.
Device approval and permission upgrades require **user presence in the Kelpie app**, not just an RPC, because any same-user program can read
the user token (`docs/architecture.md:72`, `auth.rs:19-21`). Replaces bearer tokens with device public keys.

## Requirements
- Functional
  - **QR** (default): `kelpie://pair#p=<payload>` read by the native app's scanner. Payload v1: `{v, dpk, psk (32 B, single use, 120 s), lan: [bound "ip:port"...], room?, relay?, name}`.
    Until phase 5 ships the app, Settings offers only the browser form below.
    LAN **browser** pairing is a separate explicit desktop action "Pair a browser on this network" that shows an `http(s)://<ip>:4817/#p=…` QR with a warning on the desktop dialog. [RT-2]
  - **Handshake** `Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s` (psk mixed at the end of msg1): the desktop trial-decrypts msg1 against each live code, spends a code only when msg1 decrypts
    under it, and raises approval only then; wrong psk ⇒ no event, nothing spent. Reconnect `Noise_IK_…`; known device keys only; no app data before msg2. [RT-3]
  - **Approval**: phone shows a 6-digit code from the handshake hash; the user **types** it into the desktop dialog (not compare). [RT-2]
  - **User presence** (app-only): approve, permission upgrades (`write`, `control`), "always allow", relay enable and "Reset phone access" need (a) the call to come from the
    **trusted app client** and (b) OS user verification in the app (Windows Hello `UserConsentVerifier`, macOS LocalAuthentication; fallback the OS credential prompt).
    Trusted app client = new `Role::App`, granted when the pipe peer's process image is the installed Kelpie app (`GetNamedPipeClientProcessId` + `QueryFullProcessImageNameW`;
    macOS `LOCAL_PEERPID` + `proc_pidpath` + code-signature check); dev builds accept the dev exe. Bare `Role::User` RPCs for these methods are refused. [RT-1]
  - Every device add / permission change: desktop toast + event to the other paired phones; device records MAC'd with a key in the OS keyring, unverifiable records refused. [RT-1]
  - Default permission for a new device: `read`; PWA devices never get `control` or a relay token. [RT-2]
    <!-- Updated: Validation Session 1 - PWA write only on trusted transports -->
    Effective permission is capped by the transport, enforced by the daemon per connection: a PWA link over TLS or from a Tailscale address (100.64.0.0/10, fd7a:115c:a1e0::/48) may use `write`;
    a PWA link over plain `http://` from any other address is capped at `read` (the page could have been swapped). Native-app links are not capped.
  - Framing: one WS binary message = one Noise transport message (≤ 65 535 B); app messages chunked to ≤ 1 MiB; JSON body = phase-1 protocol; rekey every 2^16 messages or 30 min.
  - `client_id` dedupe per **device** across connections (answers and `chat_send` 10 min, input/key 60 s — see Resilience requirements), cleared on `daemon_epoch` change. [RT-8]
  - **Live permission checks**: every received message checks the device's current record by key; `broadcast::RecvError::Lagged` ⇒ re-check all; revoke/downgrade synchronously
    stops that device's control grants (phase 7/8 hooks) before closing sockets. [RT-12]
  - **Listeners**: bind `/link` on every `usable()` address (private + Tailscale, never 0.0.0.0) and rebind on interface change (`if-addrs`); QR lists bound addresses. [RT-15]
  - **Device store v2**: new file `mobile.v2.json` (`version` field); old `mobile.json` read once for settings, never rewritten; parse failure ⇒ back up, log, keep running with phone access off — never overwrite.
    Old token devices shown as "re-pair needed". `mobile.set_write` kept as alias for one release. [RT-9]
  - `/ws` + tokens removed; `API_LEVEL` and `DAEMON_SECURITY_LEVEL` bumped so the app keeps asking to restart an old plaintext-token daemon. [RT-9]
  - PWA: wasm build; key in IndexedDB; banner on plain `http://` non-Tailscale origins; CSP adds exactly `'wasm-unsafe-eval'` to `script-src` (`mobile.rs:384`). [RT-11]
- Non-functional: handshake ≤ 50 ms (wasm, mid phone); crate is sans-I/O.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
- **`peer_reset`**: any drop/replacement of the pipe between two peers ⇒ both sides discard Noise transport state and partial chunk buffers and redo IK (1 RTT); a frame from a dead session can never cause a decrypt-error loop in the new one; chunk reassembly timeout 10 s. Test: kill the relay↔desktop socket mid-chunk 100× ⇒ clean re-handshake every time.
- No session-resumption tickets (rejected: complexity, no measurable gain); reconnect = transport + IK ≤ 4 RTT; handshake timeout 8 s; known device keys may burst 5 handshakes / 10 s, pairing stays rate-limited (and by load level, phase 1).
- **Address advertisement**: after every handshake and on every rebind (debounced 2 s) the daemon sends `addrs {seq, list, hostname}` (Tailscale MagicDNS name if present, else `<host>.local`), also over the relay; the phone stores the last-known set. Desktop network-change detection: `NotifyIpInterfaceChange` (Windows) / `SCDynamicStore` or a 5 s `if-addrs` poll (macOS) triggers rebind. Test: change the daemon's IP while the phone is on the relay ⇒ phone dials the new address ≤ 5 s.
- PWA limit: its origin is the old IP after a DHCP change; the browser-pairing QR uses the hostname when available; docs recommend a DHCP reservation or Tailscale; the native app is the recovery path.
- The phase-1 writer's priority path (heartbeat, replies, revoke; there is no separate control lane), heartbeat and lag-aware liveness run **inside** the Noise channel; `grant_alive: Arc<AtomicBool>` is checked per input event in every control path (independent of the async scheduler) so revoke stops input < 1 s even at L3.
- Dedupe windows: `answer`/`chat_answer`/`chat_send` 10 min per device (`chat_send` is deduped by its `cid`: a retry gets `dup` with the message's real state, never a second message; the link retries it once within 60 s, never into another daemon epoch), `input`/`key` 60 s (a phone offline for minutes then reconnecting must not double-answer). 100 fuzzed disconnect/replay trials ⇒ 0 duplicates.

## Architecture
```
crates/kelpie-link/
  src/lib.rs      // QrPayload, Pairing (IKpsk1), Session (IK, rekey), Chunker
  src/wasm.rs     // wasm-bindgen API (cfg wasm32)
  src/client.rs   // feature "client": tokio-tungstenite connect race + migration (phase 4), used by phase 5 and tests
```
- `snow = { version = "0.10", default-features = false, features = ["default-resolver-crypto"] }`; wasm target `getrandom = { version = "0.3", features = ["wasm_js"] }`;
  CI gate: `cargo tree -p kelpie-link --target wasm32-unknown-unknown -i ring` is empty. [RT-11]
- `wasm32-unknown-unknown` + wasm-bindgen added to `ci.yml` **and** `release.yml`. [RT-10]

## Related Code Files
- Create: `crates/kelpie-link/**`, `scripts/build-link-wasm.sh`, `crates/kelpied/src/peer_identity.rs`, `app/src-tauri/src/user_presence.rs`
- Modify: `Cargo.toml`, `crates/kelpied/src/mobile.rs`, `crates/kelpied/src/auth.rs` (`Role::App`, method gating), `crates/kelpied/src/daemon.rs` (`mobile.*`), `crates/kelpie-proto/src/lib.rs` (levels), `app/src/components/settings/mobile-settings.tsx`, `app/phone/src/link.ts`, `app/package.json`, `.github/workflows/{ci,release}.yml`, locales, `docs/architecture.md`, `docs/user-guide.md`, `README.md` (privacy)

## Implementation Steps
1. Crate: QR codec, IKpsk1/IK, chunker, rekey, vectors; wasm binding; CI gates.
2. Peer identity → `Role::App`; user-presence flow in the app; gate the sensitive `mobile.*` methods.
3. Daemon `/link` on all usable addresses, responder with trial decrypt, approval with typed code, device store v2 + MAC, live perm checks, per-device dedupe.
4. Desktop settings: app QR, browser QR (explicit), approval dialog, perms, reset.
5. Phone pairing + reconnect; http banner; CSP change.
6. Remove `/ws`; level bumps; docs.

## Tests
- `cargo test -p kelpie-link`: handshakes; wrong psk ⇒ responder raises nothing and code stays live; expired/reused code; unknown device on IK; tamper/replay/reorder; chunk limits; rekey; QR proptest.
- `cargo test -p kelpied`: `Role::User` approve/upgrade refused, `Role::App` allowed; MAC'd record edit refused; v1 file preserved and never rewritten; parse failure backs up and does not overwrite; revoke ⇒ socket closed and control stopped; lagged broadcast re-check.
- e2e `mobile.py`: pair via emulated phone (wasm, browser QR path), typed code, RPC-only approve fails, revoke, re-pair after reset, CSP: wasm instantiates with zero violations; captured bytes contain no plaintext.
- Downgrade test: previous release daemon starts with `mobile.v2.json` present and old `mobile.json` intact.

## Perf gate
- Crypto overhead ≤ 5 % CPU vs phase 2 workload. **Met:** 0.1 to 1.3 points by cycle counter against the daemon before encryption, seal alone about 0.37 ([measurements](../reports/debugger-261031-0750-phone-link-cpu.md)). `mobile_perf.py`'s total of 5 points for eight phones is not met, at about 10, most of it older than this phase; to be reduced separately before merge (user decision 2026-10-31).

## Success Criteria
- [ ] Pairing = scan + type a 6-digit code + OS verification on the desktop. Built and tested through a seam; the real OS prompt has never run, and that blocks merge.
- [x] No program holding only the user token can add a device or raise permissions through the daemon. Accepted and documented residual: a same-user program can read the keyring and write a sealed record, or replace the daemon's files.
- [x] All phone traffic encrypted on any pipe; revoke cuts access (and control) at once.

## Risk Assessment
- `snow` 0.10 unaudited; signal: RUSTSEC advisory; response: pin, review IK/IKpsk1 paths, alternative crate behind the same API.
- Peer-process check bypassable by code injection into the app (same user); accepted residual — raises the bar from reading a file to injecting code; stated in docs.
- Windows Hello absent; response: OS credential prompt fallback.
