---
phase: 1
title: "Relay v2: open rooms, abuse limits, many origins, hosted page"
status: done
priority: P1
effort: "2.5d"
dependencies: []
---

# Phase 1: Relay v2

## Context
- Router: `relay/src/worker.ts` — `register()` needs the key (`:143-166`, `REGISTRATION_KEY` check `:146`, refuses any `Origin` `:145`), in-isolate `Strikes` (`:28-57`), socket paths `:20`, `fetch` `:177-207`.
- Room: `relay/src/room.ts` — one stored origin, phone refused otherwise (`:154-155`), `origin` control (`:305-312`), `open` message (`:200-202`), desktop 101 (`:126-151`), usage flush every 5 s (`USAGE_FLUSH_MS` `:65`, `count()` `:407-419`), `info` with `RELAY_VERSION` (`:440-448`).
- Config: `relay/wrangler.toml` (router, DO bound to `kelpie-link-relay-room`), `relay/wrangler.room.toml` (`RELAY_VERSION = "1"` `:11-12`, migration `v1` `:19-21`), `relay/workerd/config.capnp` (`:8-27`), `relay/workerd/entry.ts`, `relay/Dockerfile`.
- Measured (phase 4a): rooms hibernate after ≥ 10 s idle; a room-script deploy drops active rooms' sockets (1006), a router deploy drops none.

## Overview
Make the relay usable by every install without a key, safe to leave open, able to admit phones from both the Tailscale origin and the hosted origin, able to tell a daemon what it speaks, and able to serve the hosted bundle as static assets from the router's own origin.

## Requirements
1. **Registration modes** (`POST /v1/rooms`): `REGISTRATION_KEY` set ⇒ key required (today's behaviour, 401/429 kept); else `OPEN_REGISTRATION = "1"` ⇒ open, `Authorization` ignored; else 503 "relay not configured" (today). Any `Origin` still 403 (a page can never make rooms).
2. **Registration limits** (unauthenticated path; responses static text, `x-kelpie-reason` says which):
   - Burst: Workers Rate Limiting binding `ROOM_RATE`, key = hashed network (`failureKey`, `worker.ts:135-137`), `ROOMS_PER_MINUTE` (default 5, period 60 s). Per location and approximate: a first shed, not the cap. Missing binding (workerd) ⇒ skipped.
   - Daily per network and overall: new `Registrar` Durable Object, single instance (`idFromName("registrar")`), SQLite rows `(day, key, count)`; `ROOMS_PER_NETWORK_DAY` (20), `ROOMS_PER_DAY` (1,000). Over ⇒ 429 `network` / 503 `full`. Keys are hashes, never addresses; rows older than yesterday deleted on the next write; no alarms, no timers.
   - Order: rate binding → Registrar `admit()` → room `init()` (the room call happens only after admission).
3. **Many origins per room**: control `{"t":"origins","o":[...]}`, ≤ 4 entries, each matching today's origin rule (`room.ts:307`, `https://` only). Stored as `origins`. Legacy `{"t":"origin"}` sets `origins = [o]`; a room stored with `origin` reads as `[origin]`. Phone admission: no `Origin` (non-browser) or `Origin ∈ origins`. Origin is a browser-page guard only; admission still rests on the token or single-use pair token.
4. **`open` carries the phone's origin**: `{"t":"open","s","via","k","o"}`, `o` = the `Origin` header the room checked (absent when none). Kept in the socket attachment so a desktop that reconnects hears it again (`room.ts:286-289`).
5. **Version**: room script `RELAY_VERSION = "2"`; the desktop's 101 carries `x-kelpie-relay-version: 2`. Daemons read it (phase 2) before sending `origins`, so a new daemon never sends a v1 room a message that closes its socket (`room.ts:321-322`).
6. **Hosted page from the router** (Cloudflare only, H10):
   - `wrangler.toml`: `[assets] directory = "./public"`, `run_worker_first = ["/v1/*"]`, `not_found_handling = "404-page"`. `public/` is the hosted bundle copied in by CI or `pnpm` script; gitignored; never committed.
   - Asset requests never run the Worker (free, unlimited); unknown paths get `404.html` without the Worker, so scanners do not spend the request quota.
   - `_headers` comes from the bundle (phase 3). The router itself still serves no HTML: every `worker.ts` response is static plain text or JSON with `x-content-type-options: nosniff` and `cache-control: no-store`; no response echoes any part of the request (the same origin now runs code, so a reflected byte would be script-able).
   - workerd self-host: no page (no `ASSETS`); `/v1/*` unchanged.
7. **Cost guards**: usage flush 5 s → 60 s or 1 MB (`room.ts:65-66`), cutting row writes about 12× while active; a lost minute of counting on eviction is acceptable for a soft monthly cap. Hibernation rules unchanged (`acceptWebSocket`, auto-response, no timers).
8. **Smoke** (`relay/scripts/smoke.mjs`): works with no key in open mode and with a key in key mode; checks the version header, `origins` admission (listed origin in, unlisted out), `o` in `open`; with `PAGE=1`, `GET /` is HTML with a CSP header, `GET /version.json` parses, `GET /no-such-file` is 404.

## Abuse limits after this phase
| Where | Limit | Mechanism |
|---|---|---|
| Room creation | 5/min, 20/day per network, 1,000/day overall (vars) | rate binding + `Registrar` DO |
| Close registration | set `REGISTRATION_KEY` | secret, no code deploy |
| Refused phone/desktop sockets | 30 per network+room per 10 min | `Strikes` (existing, `worker.ts:79-80`) |
| Per room | 8 phones, 64 KiB frames, 200 msg/s/socket, 4 MB/s burst, 2 GB/month, 90-day TTL | existing (`room.ts:50-68`) |
| Unknown paths | 404 page, no Worker run | `not_found_handling` |
| Phones from other origins | refused | `origins` |
Replaces "admit phones only from the computer's Tailscale address": the room admits phones from the origins its desktop registered (the Tailscale origin and/or the hosted origin); the token stays the real gate.

## Cost model [E unless marked]
- Free: 100k Worker requests/day, DO 100k requests/day, 13,000 GB-s/day, 100k rows written/day, 5 GB stored [docs, third-party summaries; confirm on the pricing page in phase 8]. Static assets free and unlimited [docs].
- An active room is billed at 128 MB wall-clock: 450 GB-s per active hour ⇒ ~29 active room-hours/day for everyone on Free. A phone idle in the foreground (hb every 20 s, hibernation after ≥ 10 s [M, 4a]) keeps it active about half the time.
- Incoming WebSocket messages bill 20:1 as DO requests [docs, unconfirmed]; a streaming terminal at ~10 frames/s ≈ 1,800 billed requests per active hour.
- Rows written: ~60 per active room-hour after the 60 s flush, plus a few per connection.
- Paid ($5/month base) example, 1,000 users × 1 active hour/day: ~13.5M GB-s/month ≈ $165, DO requests ≈ $8, rows within the included amount. Measure, do not trust this: phase 8 reads analytics after 7 days.

## Related code files
- Modify: `relay/src/worker.ts`, `relay/src/room.ts`, `relay/wrangler.toml`, `relay/wrangler.room.toml` (`RELAY_VERSION` 2, `[[migrations]] tag = "v2" new_sqlite_classes = ["Registrar"]`), `relay/workerd/entry.ts` (export `Registrar`), `relay/workerd/config.capnp` (Registrar namespace, `OPEN_REGISTRATION` and limits from the environment), `relay/workerd/wrangler.toml` (migration v2), `relay/scripts/smoke.mjs`, `relay/test/relay.test.ts`, `relay/test/entry.ts`, `relay/vitest.config.ts`, `relay/package.json` (`dev:page`: `wrangler dev --local` with `public/`), `relay/.gitignore` (`public/`), `relay/Dockerfile` (comments: registration modes).
- Create: `relay/src/registrar.ts`.
- Delete: none.

## Implementation steps
1. `registrar.ts`: `admit(networkKey, now) → "ok" | "network" | "full"`, one SQLite table, prune on write. Unit tests first.
2. `worker.ts`: registration modes, limits in order, `x-kelpie-reason`, `nosniff` on `plain()` and the JSON reply; `Env` gains `REGISTRAR`, `ROOM_RATE?`, `OPEN_REGISTRATION?`, limit vars.
3. `room.ts`: `origins` storage + legacy read, admission check, `o` in `open` and attachment, version header on the desktop 101, flush cadence.
4. Config files and migrations; workerd parity (no rate binding there).
5. `[assets]` config, `.gitignore`, `dev:page` script; smoke extensions.
6. Tests; `pnpm typecheck && pnpm test`; smoke on local workerd (open and key modes) and on `wrangler dev --local` with a test bundle.

## Tests (relay vitest, `@cloudflare/vitest-plugin`)
- Open mode makes a room without a key; key mode as before (401, 429 after 10 wrong); neither ⇒ 503; `Origin` ⇒ 403 in every mode.
- Per-network daily cap ⇒ 429 `network`; overall cap ⇒ 503 `full`; another network still admitted; refused attempts make no room `init`.
- Registrar prunes old days; never stores an address (grep the stored keys are 64-hex).
- `origins`: listed origins admitted, others 403; legacy `origin` message and legacy stored `origin` still work; > 4 entries or `http://` ⇒ closed with `origins`.
- `open` has `o` for a browser phone, none for a non-browser one; a reconnected desktop hears `o` again.
- Desktop 101 has `x-kelpie-relay-version: 2`.
- Every router response has `nosniff`; fuzzed paths/queries never appear in a body; `/v1/*` never `text/html`.
- Usage flushes at most once per 60 s under steady traffic (fake clock); source-grep: no `setInterval`/`setTimeout`/looping alarms in `room.ts` and `registrar.ts`.
- If the pool cannot emulate the rate binding [UNVERIFIED], inject a fake limiter through `env`.

## Validation
- `cd relay && pnpm install --frozen-lockfile && pnpm typecheck && pnpm test`.
- workerd smoke as in `.github/workflows/relay.yml:30-39`, once with `OPEN_REGISTRATION=1` and no key, once with a key.
- `wrangler dev --local` with a test bundle in `public/`: `PAGE=1 pnpm smoke`; response headers come from `_headers`.
- Nothing reaches Cloudflare in this phase.

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| Open rooms used as a free tunnel | M×M | per-room caps unchanged; per-network room cap; close switch |
| Flood from many networks exhausts Free limits | M×H | overall daily cap; Registrar only after the rate binding; Q5 |
| CGNAT users share a per-network count | M×M | 20/day default, clear refusal text, tunable var (Q8) |
| `_headers` not applied ⇒ page without CSP | L×H | `wrangler dev` smoke checks headers; phase 5 `verify_live.py` checks them live |
| Rate Limiting binding unavailable on Free [UNVERIFIED] | M×L | treated as optional; Registrar is the cap |
| Registrar single instance is a hot spot | L×M | only room creation reaches it, after the rate binding |

## Rollback
Revert the commit. Deployed: redeploy the previous router (assets go with it) and room versions; a room stored with `origins` is read by v1 code as "no `origin`" ⇒ browser phones refused until v2 returns (fails closed). Close registration any time by setting `REGISTRATION_KEY`.

## Success criteria
- All tests above green; both smoke modes pass; the page smoke shows CSP, `nosniff`, `no-referrer`, `X-Frame-Options: DENY` on `/` and immutable caching on `/assets/*`.
- A v1-style desktop (sends `origin`, ignores headers) still pairs and relays in the vitest suite.

## Completion note (2026-11-10)
Done on branch `relay/v2-open-rooms`; report: [reports/fullstack-261110-relay-v2.md](reports/fullstack-261110-relay-v2.md). Where the code differed from this spec:
- The burst limit (5/min) is the `[[ratelimits]]` binding's `simple.limit` in `wrangler.toml`, not a `ROOMS_PER_MINUTE` var: the binding's limit is fixed at deploy. The daily limits are vars as specified.
- The limits apply only without a key (open registration); a relay with `REGISTRATION_KEY` set behaves as before. The burst refusal is `429` with `x-kelpie-reason: rate` (besides `network` / `full`); phase 2 should map it to a "try again in a minute" text.
- `x-kelpie-relay-version` is a protocol constant in `room.ts` (`2`), not `RELAY_VERSION`, which stays the deployment label shown by `info` (set to `2`).
- The Cloudflare scripts now have handler-only main modules (`src/entry-router.ts`, `src/entry-room.ts`): current workerd refuses a main module that exports plain constants (`worker.ts`/`room.ts` do, for tests), which broke `wrangler dev` already before this phase.
- With `[assets]`, `wrangler dev` and `wrangler deploy` refuse to start without `public/` (checked with a local dry run): a router deploy cannot ship without a bundle; for relay-only dev, create an empty `public/`.
- `.github/workflows/relay.yml` still smokes key mode only; adding the open-mode run belongs to phase 5 (workflows).

Sources: [static assets billing](https://docs-durable-objects-instance-replaced-errors.previews.developers.cloudflare.com/workers/static-assets/billing-and-limitations/index.md) · [rate limit binding](https://developers.cloudflare.com/workers/runtime-apis/bindings/rate-limit/) · [DO pricing](https://developers.cloudflare.com/durable-objects/platform/pricing)
