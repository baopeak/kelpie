# Relay abuse review after opening registration (v0.1.16)

Date 2026-11-11 13:50 (+07). Source: `origin/master` d9b7255, `relay/src/{worker,registrar,room}.ts`, `relay/wrangler.toml`; Cloudflare analytics (GraphQL) for today.

## Current state
- Production `kelpie-link-relay`: v0.1.16 router + app + room script; `REGISTRATION_KEY` deleted 13:3x, `OPEN_REGISTRATION=1`. Smoke (keyless room) passes.
- Usage today: router 16 requests, staging 9; DO active time ~0. No sign of abuse.

## What already holds (per room / per socket)
- Room creation: no `Origin` (pages refused), P-256 key checked, `ROOM_RATE` 5/min per network+location, Registrar 20/network/day, 1000/day total (IPv6 counted per /64).
- Sockets: HMAC ticket checked before any DO call; desktop signs a fresh timestamp; phones need a desktop-allowed or pair token (256-bit); 30 refusals/10 min per network then refused at the router.
- Traffic: binary only, 64 KiB frames, 200 msg/s, 4 MiB burst bucket, 2 GiB/room/month, max 8 phones; hibernation + runtime ping/pong so idle rooms cost nothing.

## Gaps (Free plan — limits are shared and hard: past one, the relay fails for everyone until 00:00 UTC)
| # | Vector | Cost to attacker | Effect |
|---|---|---|---|
| 1 | Hit `/v1/*` ~100k times/day (Worker requests) | ~1.2 req/s from anywhere, no Kelpie needed | Router down for all |
| 2 | Own room (any P-256 key) + own "phone"; trickle 1 frame every few s keeps the DO awake | 2 rooms/24 h ≈ the ~29 room-hours/day of DO duration (13,000 GB-s at 128 MB) | Rooms down for all |
| 3 | Same rooms flushing usage every 60 s → rows written | ~70 busy rooms ≈ 100k rows/day | Storage writes fail for all |
| 4 | Fill 1000 rooms/day from ~50 IPv6 /64s (or IPv4s) | trivial | No new pairings that day (existing rooms fine) |

Code limits only slow 2–4; nothing in code can stop 1 (each request counts once the Worker runs).

## Options
- **A. Workers Paid ($5/mo)**: no hard daily stop; attacks then cost money instead of availability → pair with billing/usage notifications and the code caps below. Prices to confirm on Cloudflare's page (plan's cost table).
- **B. Custom domain + WAF rate-limit rule on `/v1/*`** (Free zone has rate-limiting rules): stops 1 before the Worker runs. Needs a domain; changes the default relay URL (daemon default, docs, hosted origin, passkey rpId → re-pair hosted phones).
- **C. Code caps (next release)**: awake-time budget per room per UTC day; lower `ROOMS_PER_NETWORK_DAY` (20 → 3: a desktop reuses its room 90 days) and `ROOMS_PER_DAY`; proof-of-work on keyless `POST /v1/rooms`.
- **D. Watch**: scheduled CI job reading analytics, alert at e.g. 50 % of any daily limit; close switch = set `REGISTRATION_KEY` again.
- **E. Close now**: set the key again until A/B/C land (new installs then can't make rooms).

## Owner decision (2026-11-11 13:49)
Stay on Free, registration open, keep watching (owner is the only user, building and testing). A/B/C deferred; close switch E if usage looks wrong.
