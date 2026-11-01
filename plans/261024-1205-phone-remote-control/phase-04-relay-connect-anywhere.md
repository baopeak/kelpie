---
phase: 4
title: "Relay: one Cloudflare relay, connect from anywhere"
status: pending
priority: P1
effort: "6d"
dependencies: [3, 3a, 3c, 4a]
---

# Phase 4: Relay — connect from anywhere

## Overview
An opt-in, untrusted relay so the installed phone app reaches the desktop when Tailscale is off or blocked. **One** relay on Cloudflare Workers + Durable Objects, personal use first (user decision 2026-10-30; a public service is a later, separate decision). The desktop dials out; the relay forwards opaque `kelpie-link` frames. Same TypeScript runs on `workerd`, so self-host stays possible. Noise stays end to end: the relay stores no content and cannot read it, on Cloudflare or anywhere else; it sees IPs, timing and sizes.

Not two relays: Tailscale stays in the race as the fallback (and LAN on a plain `http://` page, see the limit below). The pairing record holds a **list** with one relay entry, so adding one later is config only.

Client is the installed PWA (native app deferred, phase 5), so the race lives in `app/phone/src/link.ts`. `kelpie-link::client` (Rust) is deferred with the native app.

## Measured in phase 4a (2026-11-01, this PC's network only, one night; no carrier, no soak result yet)
Full tables: `plans/reports/fullstack-developer-261031-2150-phone-phase4a.md` (kept locally). Where these differ from the text below, these are the measured values; the design decisions they touch are still the owner's.
- **Placement**: 20 of 20 new rooms landed in HKG, the same colo as the entry POP.
- **Deploys**: the room class in its own script works. Router deploys dropped 0 of 40 sockets. A room-script deploy dropped no idle-room socket (45 of 45 stayed and ran the new code on their next event); active rooms dropped 5 of 5 each time, about 5 minutes after a default deferred deploy, or at once with `immediate`.
- **Close code on a deploy**: clients saw **1006 with no close frame**, so a "distinct restart close code" cannot be sent for a code update.
- **Hibernation**: with 8 idle sockets the object restarted after every idle gap of 10 s or more (15 of 15) and after none of 8 s or less (0 of 6); waking added no visible delay. Auto-responses did not appear as object requests.
- **Ticket check**: 3,000 forged tickets at about 980/s made 0 Durable Object calls. Worker CPU was about 0.42 ms per forged request in total, against this phase's "< 0.1 ms" for the check alone, which was not measured separately.
- **Plan**: the Free plan and `*.workers.dev` replace Workers Paid, the custom domain and the budget alert until usage grows (see phase 4a and plan.md, Session 3).

## Requirements
- Functional
  - **Room**: random `room_key` (32 B, keyring); ticket = random room id + HMAC, checked in the Worker **before any DO call** (< 0.1 ms). Relay stores only `room_id` and `H(room_key)`. Unused rooms expire after 90 days. [RT-15]
  - **Registration (personal use)**: creating a room needs an operator key (`wrangler secret`, pasted once in Settings). Strangers cannot create rooms, which is also the first billing guard.
  - **Placement**: no `apac` hint and no `locationHint`; the desktop's own first `get()` places the object (`apac` lands in Japan/Korea ~39 %, `apac-se` cannot be forced [T]). A DO is pinned for life, so if the desktop's measured relay RTT stays above the threshold from phase 4a for 24 h, it offers to **re-create the room**; the new room id reaches phones as an `addrs` update inside Noise, and the old room is dropped.
  - **Pairing record**: `Endpoint {t:"relay", a:"wss://…", r:"<room>"}` (`kelpie-link/src/lib.rs:209`); one entry; clients skip unknown kinds.
  - **Phone admission**: desktop pushes an allow-list of `H(device_relay_token)`; tokens issued over `kelpie-link` after pairing, **expire in 30 days, rotated inside Noise**. Issued to native devices and to PWA devices with `secure_origin` (3a). Plain-`http://` devices never get one. Pairing via relay: desktop registers `H(pair_token)` (TTL 120 s, single use), `pair_token = HKDF(psk, "kelpie-relay-pair")`.
  - **Desktop admission**: signature with the room key, not a bearer value [synthesis: should].
  - **`Origin`** (synthesis said reject it; a browser client always sends one, so refined): desktop path rejects any `Origin`; phone path accepts only the origin the desktop registered for the room, and only with a valid token. Non-WS requests refused. **No HTML served.**
  - **The relay never serves code** (D5 as amended): the phone page comes only from the daemon, over Tailscale HTTPS; the installed app's kept shell (`sw.ts`) opens off the tailnet and dials the relay. So first install needs Tailscale. The hosted-PWA option for non-Tailscale users belongs to the going-public decision (open question).
  - **CSP**: `secure_headers` (`mobile.rs`) is static with `connect-src 'self'`; becomes per-config, adding exactly the relay origin while the relay is on.
  - **Effective permission over relay**: `transport_cap` (`mobile.rs`) reads `peer.tls`/loopback/Tailscale, none of which apply to a slot: a slot's cap = `Write` when the record is `secure_origin` (`Control` per the phase 7 rule), else refused.
  - **Pairing independent of the LAN listener**: with the relay on, `pair_start` works when no LAN address binds. [RT-15]
  - **Forwarding**: binary frames ≤ 64 KiB; slot id per phone socket; ≤ 8 phones per room; `desktop_offline` frame; nothing queued or stored.
  - **Limits**: per-room 4 MB/s burst, **~2 GB/month** soft cap (lowered from 20 GB; confirm after 4a); **per-socket message cap** (close above ~200 msg/s); violations close with explained codes.
  - **Relay admission requires `caps`** ack + hb: the daemon's legacy mode has no application-level liveness, and a relay keeps its own TCP alive, so a dead legacy client would never be noticed.
  - **Revoke**: the daemon's allow-list is authoritative; a revoke kills live sessions on all carriers; while offline, removals are persisted on the desktop and applied first on reconnect. [RT-12]
  - **Logging**: counters and error codes only; no payloads; no persisted IPs (Cloudflare edge logs documented); 24 h hashed-IP abuse state on unauthenticated paths only.
  - **Limits by credential**: per-credential limits; IP limits only on unauthenticated paths, because CGNAT makes per-IP limits unfair.
  - **Daemon** (`relay.rs`): outbound WSS only when the relay row is on (user presence, in `APP_ONLY`); jittered backoff 1 s → 60 s; `TCP_NODELAY`; each slot feeds the same link handler as `/link`.
  - **Daemon handshake budget**: global 20/s, 5/s per slot, 16 pending, silent drop, no side effects before the first valid initiator frame. mac1 prefilter already in `kelpie-link` (`prefilter`, `lib.rs:186`), run on every slot's first message before any Diffie-Hellman.
  - **Traffic shape**: pad control and chat frames to size buckets. No compression (CRIME-style risk; ciphertext does not compress), no permessage-deflate. Raw-terminal chaff (20 ms tick, ~10 ms added latency, ~7 KB/s while typing) only as an off-by-default switch.
  - **Crypto hygiene already in place, kept**: implicit-counter nonces fail closed, `v` in the prologue, no downgrade retry, counter rekey. Add a full re-handshake daily.
  - **Client race** (`link.ts`): candidates from the pairing record, `ts` first, relay after a 250 ms stagger, earlier attempts kept, first completed IK handshake wins. On relay, re-probe direct on interface-change triggers; migrate (new handshake, rebind subscriptions with per-device resume, then close the relay socket). Control grants re-bind only after the same device's handshake. [RT-12]
  - **Limit**: an HTTPS page cannot dial `ws://<LAN IP>` (mixed content), so the installed app's direct path is `ts`. A plain `http://` page keeps its same-origin LAN socket; it is a separate read-only pairing.
  - Settings → Mobile: 3d row, relay URL (default / custom), operator key, "What the relay can see" (ciphertext, IPs, timing, sizes, room id), status, re-create room.
- Non-functional: **no added-latency promise**; the earlier "≤ 30 ms in-region" claim is removed (no DO in Vietnam; desktop-to-HKG alone was 47–68 ms [M]). Targets come from phase 4a. Foreground-to-live p50 < 1.2 s, p95 < 3 s (to verify). Zero cost when off. Worker CPU ≤ 1 ms per frame.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research; 2026-10-30 - single Cloudflare relay -->
- **Relay signals**: `desktop_offline` / `desktop_online` (treated as hints, a relay can forge them), `peer_reset` to the surviving side when either socket drops, a distinct **restart** close code on which clients add 0–10 s jitter; desktop sockets carry a generation counter, new replaces old (last wins).
- **Deploy drops are accepted and limited**: every deploy, runtime update and eviction restart drops sockets [D]. Mitigations: resume from phase 1, rare deploys, DO class in its own script so a routine router deploy leaves it alone (**unverified; phase 4a answers it**), Tailscale stays in the race.
- **Keepalive**: relay↔desktop and relay↔phone edge ping/pong every 20 s via `setWebSocketAutoResponse` (no wake, no duration billing); may relax to 30 s when stable; end-to-end liveness from the phase-1 `hb` inside Noise, sent only while a phone is attached, so an idle desktop-only room costs ~0. Desktop declares the relay dead after 45 s (90 s at L2+, lag-aware).
- **Backoff and triggers** (desktop → relay): full-jitter 1 s → 60 s, reset on a network-change event (debounced 2 s) and after 30 s stable. Test: kill/restart `workerd` 20× with 8 phones ⇒ all reconnect ≤ 15 s, ≤ 3 attempts per client per 10 s.
- **Race refined**: direct candidates first, relay after 250 ms; Tailscale always eligible; direct → relay fallback immediate, relay → direct only after direct has been stable 10 s (no flapping).
- **Kept shell opens at once**: `sw.ts` today waits 2.5 s for the network on navigation; for a paired installed app, serve the kept shell first and update behind it, or foreground-to-live cannot meet the target off the tailnet.
- **Budgets survive eviction**: counters persisted to DO storage on message arrival, batched every 5 s or 1 MB, never by a timer.
- `relay unreachable` reported distinctly from `computer offline`; direct retried every 30 s while the relay is down.
- **Deferred, pending the lost-contact decision** (open question): the 2026-10-25 lease + DO alarm. It needs a push from the relay when the desktop is gone, and Web Push is sent by `kelpied` only. Design kept in phase 6.
- Settings → Mobile suggests the existing keep-awake mode "while agents are working" when phone access is on (no new behaviour; user decision 2026-10-25).
- Host overload: relay pairing admission follows the phase-1 load level; overloaded desktop answers new takeover requests with `host_busy`.
- Tests with the TCP fault proxy (phase 1 harness): blackhole 10/30/90 s, half-open, RST storm, NAT rebinding, relay deploy.

## Deployment & operations
<!-- Updated: 2026-10-25 - hosted + self-host design; 2026-10-30 - one relay, personal use first -->
- **One codebase, two targets**: `relay/` (TypeScript Worker + DO). Hosted on Cloudflare via `wrangler`; self-host via `workerd` in a Docker image (`ghcr.io/baopeak/kelpie-relay`, persistent volume, TLS by Caddy/Let's Encrypt in front). Same protocol version, same tests.
- **Environments**: `staging` and `production` Workers on separate routes; dev builds and e2e use a local `workerd`.
- **Hibernation guard** (a defeated hibernation costs ~$4/room-month against about $0.06): `acceptWebSocket` only, no plain `accept()`; no `setInterval`/`setTimeout`; **no per-socket timers, no looping alarms**. Enforced by a source-grep test and by an assertion that the DO hibernates ≤ ~15 s after the last event with 8 idle sockets.
- **Billing guards** (no hard spend cap exists): own Cloudflare account, Workers Paid, **budget alert + runbook** (what to do at each threshold: tighten caps, disable registration, `wrangler rollback`), custom domain, WAF rule, per-socket message cap, ticket check before any DO, registration key. Alerts at 50 % of the expected monthly figure.
- **Secrets**: only via `wrangler secret put` per environment (env vars for self-host); never in the repo or CI logs. **Push and TURN secrets are not in the relay data path**: if APNs/FCM or TURN credentials ever exist, they live in separate Workers and separate accounts (deferred, phases 6, 8).
- **CI/CD** (`.github/workflows/relay.yml`): on PR, typecheck + vitest (`@cloudflare/vitest-pool-workers`) + `workerd` smoke; on tag `relay-v*`, deploy staging → automated smoke (pair a test client through the relay, send 100 frames, revoke) → deploy production. Docker image on the same tag. Deploys are rare, off-peak; rollback = `wrangler rollback`. Protocol `v` keeps clients compatible across a deploy.
- **Monitoring**: Workers Analytics + `/v1/health`; counters only; alerts on error rate and bandwidth spikes.
- **Cost model** [E, confirm in 4a]: ~$6/month at 100 users, ~$19 at 1 k, ~$188 at 10 k with hibernation holding. Personal use stays near the $5 minimum.
- **Public abuse contact and self-host doc** kept; legal notes (no monitoring duty, conduit) live in the going-public decision, not in this phase. `docs/relay-self-host.md`.
- **Maintainer go-live checklist** (blocks production only): domain, Cloudflare account + Workers Paid, budget alert, secrets, DNS routes. No Apple or Firebase accounts (deferred).
- **Published**: protocol spec and test vectors in `docs/`.

## Architecture
```
relay/
  src/worker.ts   // /v1/desktop, /v1/phone, /v1/health; ticket check before any DO call; no HTML
  src/room.ts     // Durable Object per room, WebSocket Hibernation, allow-lists, limits; own script if 4a says it helps
  wrangler.toml · workerd/ (config.capnp + Dockerfile) · test/ (vitest + @cloudflare/vitest-pool-workers)
```
- Daemon `crates/kelpied/src/relay.rs`: `tokio-tungstenite` without default features + rustls **ring** provider + webpki roots (no aws-lc/NASM).

## Related Code Files
- Create: `relay/**` (beyond the 4a skeleton), `.github/workflows/relay.yml`, `relay/Dockerfile`, `crates/kelpied/src/relay.rs`, `docs/relay-self-host.md`, `docs/link-protocol.md`
- Modify: `crates/kelpied/Cargo.toml`, `crates/kelpied/src/mobile.rs`, `mobile/link.rs`, `mobile/store.rs`, `crates/kelpied/src/auth.rs`, `crates/kelpied/src/daemon.rs` (`mobile.set_relay`), `crates/kelpie-proto/src/lib.rs` (`API_LEVEL` 27), `app/phone/src/link.ts`, `app/phone/src/pairing-store.ts`, `app/phone/src/sw.ts`, `app/phone/src/conn-state.ts`, `app/src/components/settings/mobile-settings.tsx`, locales, `README.md`, `docs/architecture.md`, `.github/workflows/ci.yml`

## Implementation Steps
1. Extend the 4a skeleton: admission, signature, tokens, limits, expiry, padding-agnostic forwarding; vitest; hibernation and billing guards.
2. `workerd` self-host config + Dockerfile + doc.
3. Daemon relay client, slots, handshake budget, allow-list sync incl. persisted removals, pairing without LAN bind, slot cap, room re-creation.
4. `link.ts` race + migration; pairing record list; dynamic CSP; shell-first in `sw.ts`.
5. Padding, optional chaff, daily re-handshake, token rotation.
6. Settings row + explainer; maintainer deploy; spec and vectors.

## Tests
- Relay vitest: forged ticket rejected before DO; wrong `room_key`; unknown or expired device token; expired/reused pair token; oversize frame; caps; message cap; foreign `Origin`; HTML/non-WS refused; desktop_offline; hibernation resume; room expiry; source-grep: no timers.
- `cargo test`: backoff; persisted removals applied first; relay off ⇒ no socket; handshake budget drops silently; no side effect before a valid first frame; garbage msg1 costs no DH; `http://`-paired device refused on a slot; control grant not re-bound before handshake; `Role::User` refused on `mobile.set_relay`.
- `pnpm exec vitest run`: race picks first handshake; migration keeps subscriptions; no flapping; unknown endpoint kinds skipped.
- e2e with local `workerd` (isolated `KELPIE_ENDPOINT`): page pairs and works over the relay with direct blocked, revoke while the relay socket is down is enforced on reconnect, direct appears ⇒ migration; relay capture has no plaintext.

## Perf gate
Off: zero outbound connections. On, idle: ≤ 1 keepalive per 20 s per socket; hibernation assertion above.

## Success Criteria
- [ ] Installed app with Tailscale off reaches a desktop behind NAT with only the relay, on all three carriers measured in 4a.
- [ ] Relay outage: with Tailscale on, sessions continue without user action.
- [ ] Relay operator sees no content; a forged ticket costs no DO call; revoke enforced even if it happened offline.
- [ ] Self-host doc reproduces the relay with one command.
- [ ] Personal monthly bill matches the 4a projection; budget alert tested once.

## Risk Assessment
- DO limits or pricing change; response: same code on `workerd`.
- Hibernation silently defeated; signal: GB-s analytics; response: source-grep test, budget alert, runbook.
- DO incidents (several in HKG/SIN in 2026 [T]); accepted; response: Tailscale in the race, `relay unreachable` state.
- `tokio-tungstenite` features pull aws-lc; signal: NASM error; response: explicit ring provider (as `mobile.rs` does).
- Relay abuse as a tunnel; response: registration key, caps; tighter admission if it goes public.
- **Rollback**: relay row off (no socket); revert the daemon commit; `wrangler rollback`; pairing records ignore an unused relay entry.
