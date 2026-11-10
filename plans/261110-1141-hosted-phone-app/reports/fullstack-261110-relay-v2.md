# Relay v2: open rooms, abuse limits, many origins, hosted page — implementation report

Date: 2026-11-10. Branch `relay/v2-open-rooms` (worktree `D:\kelpie-relay-v2`), not pushed. Only `relay/**` and this plan's files changed.

## What changed
- `relay/src/registrar.ts` (new): `Registrar` Durable Object, one instance (`idFromName("registrar")`). SQLite table `rooms(day, key, count)`; `admit(networkHash, now, perNetwork, perDay)` → `ok` / `network` / `full`. Only 64-hex keys accepted (plus the day's `total` row). Rows before yesterday deleted on the next counted admission. No timers, no alarms.
- `relay/src/worker.ts`: registration modes. Any `Origin` gets 403. If `REGISTRATION_KEY` is set, the key is required (401, 429 after 10 wrong, as before). Otherwise `OPEN_REGISTRATION = "1"` opens registration and `Authorization` is ignored. With neither, 503. Limits apply only to open registration, after body validation, in order: `ROOM_RATE` binding (if bound) → Registrar → room `init()`. The 429/503 answers carry `x-kelpie-reason` `rate` / `network` / `full`. Every router answer carries `x-content-type-options: nosniff` and `cache-control: no-store`, and no answer repeats any part of the request.
- `relay/src/room.ts`:
  - New `origins` control: up to 4 `https://` origins, using the same rule as before. Bad input closes the socket with 4003 `origins`. The legacy `origin` control stores `[o]`, and a room stored with `origin` still reads as `[origin]`.
  - Phone admission: no `Origin`, or an `Origin` in `origins`.
  - `open` now carries `o` (absent when the phone sent none). `o` is kept in the attachment, so a desktop that reconnects hears it again.
  - The desktop's 101 carries `x-kelpie-relay-version: 2` (constant `PROTOCOL_VERSION`).
  - Usage now flushes every 60 s or 1 MB, instead of every 5 s.
  - Room refusals also send `nosniff`.
- `relay/src/entry-router.ts`, `relay/src/entry-room.ts` (new): handler-only main modules for the two Cloudflare scripts. See deviations.
- Config:
  - `wrangler.toml`: new mains; `[assets]` (`./public`, `run_worker_first = ["/v1/*"]`, `404-page`); `[vars]` `OPEN_REGISTRATION=1`, `ROOMS_PER_NETWORK_DAY=20`, `ROOMS_PER_DAY=1000`; `[[ratelimits]] ROOM_RATE` 5/60 s; `REGISTRAR` binding to the room script.
  - `wrangler.room.toml`: `RELAY_VERSION = "2"`, migration `v2` adding `Registrar`.
  - `workerd/config.capnp`: Registrar namespace; `OPEN_REGISTRATION` and both limits read from the environment; version text `2`.
  - `workerd/wrangler.toml`: migration v2 and the `REGISTRAR` binding.
  - `workerd/entry.ts`, `test/entry.ts`: export `Registrar`.
  - `vitest.config.ts`: `REGISTRAR` namespace.
  - `package.json`: new `dev:page` script.
  - `.gitignore`: `public/`.
  - `Dockerfile`: comments on registration modes.
- `relay/scripts/smoke.mjs`:
  - Open mode when `REGISTRATION_KEY` is unset, key mode when it is set.
  - New checks: a page cannot make a room; `nosniff` on the answer; the version header, read through a raw upgrade made with `node:http`; listed origin admitted and unlisted origin refused; `o` present in `open`, and absent for a phone without an origin.
  - `PAGE=1` adds: `/` is HTML with CSP, `nosniff`, `no-referrer` and `X-Frame-Options: DENY`; the referenced `/assets/*` file is `immutable`; `/version.json` parses; `/no-such-file` returns 404.

## Tests
`pnpm install --frozen-lockfile && pnpm typecheck && pnpm test`: typecheck clean, **69 passed** (baseline 46; 23 new). Coverage of the phase's Tests list:
- **Registration modes:** open mode, key mode (with or without `OPEN_REGISTRATION`), neither → 503, `Origin` → 403 in every mode.
- **Limits:** network cap → 429 `network`, overall cap → 503 `full`, rate binding → 429 `rate` with the Registrar and room untouched. Refused attempts never call `ROOM.idFromName`. Another network is still admitted. The key holder is not limited.
- **Registrar:** counts per day, prunes days before yesterday, stores only 64-hex keys (or `total`), and rejects a raw address.
- **Origins:** multiple origins admitted, others 403. An empty list admits no page. Legacy stored `origin` still works. Closed with `origins` when the list has more than 4 entries, an `http://` origin, a path, or is not a list.
- **`open` and `o`:** `o` present for a browser phone and absent for an app; a reconnected desktop hears `o` again.
- **Version header:** the desktop 101 carries version `2`.
- **v1-style desktop:** sends `allow` + `origin` + `pair`, reads no headers. It pairs over `p=` with its Origin, then reconnects with `k=`, and frames pass both ways.
- **Router hygiene:** for 15 fuzzed paths, methods and headers, every answer has `nosniff` and `no-store`, is never HTML, and never echoes the marker. The 201 JSON has `nosniff`.
- **Usage flush:** with a fake `Date.now`, 180 s of 1 frame/s flushes exactly 3 times.
- **No timers:** the source grep now covers `registrar.ts` too.
- **Entry modules:** export only handlers and classes.

The fake limiter is injected through `env`. The pool could emulate the binding, but a fake keeps the tests deterministic.

## Smoke results (all local, 127.0.0.1)
- **workerd, open mode** (`OPEN_REGISTRATION=1`, no key, `--socket-addr http=127.0.0.1:8787`): 15/15 ok. Round trip about 3.4 ms.
- **workerd, neither mode set:** `POST /v1/rooms` → 503. This confirms workerd leaves an unset `fromEnvironment` binding falsy.
- **workerd, key mode** (throwaway key): 16/16 ok, including the wrong key → 401.
- **`wrangler dev --local`** with both configs and a throwaway bundle in `public/` (`index.html`, `404.html`, `version.json`, `_headers`, `assets/app-3f2a9c1e.js`), with `PAGE=1`: 24/24 ok.
  - The headers on `/` came from the bundle's `_headers`: wrangler logged "Parsed 2 valid header rules", and the CSP was the test bundle's own string.
  - `/no-such-file` returned 404 `text/html`, so it was served by the 404 page, not the Worker (whose answers are plain text).
  - The local rate binding refused the 6th room in a minute with `429 rate`.
- All servers were stopped. The throwaway `public/`, `.dev.vars`, `.wrangler/`, `workerd/rooms`, `workerd/dist` and `dist/` were removed. `CLOUDFLARE_CF_FETCH_ENABLED=false` and `WRANGLER_SEND_METRICS=false` were set for wrangler, so nothing was fetched from Cloudflare. No deploy and no login took place.

## Deviations from the spec
1. **Burst limit is not a var.** The 5/min limit is the rate binding's `simple.limit` in `wrangler.toml`, not a `ROOMS_PER_MINUTE` var, because the binding fixes it at deploy. Changing it means a router deploy, not a var edit.
2. **Limits only apply to open registration**, as the spec says ("unauthenticated path"). Key-mode registration is unlimited, as before.
3. **Burst refusal reason is `rate`.** The spec named only `network` and `full`, so I added `rate`.
4. **The version header comes from a code constant**, not `RELAY_VERSION`. The header describes what the code speaks, so a misconfigured var cannot lie about it. `RELAY_VERSION` stays the label returned by `info`, and is set to `2` in both configs.
5. **New handler-only entry modules.** Current workerd (1.20261023, wrangler 4.156) refuses a script whose main module exports plain values ("Incorrect type for map entry 'ADMISSION_REFUSALS'"). `worker.ts` and `room.ts` already exported such constants for the tests, so `pnpm dev` was broken on master before this change. Cloudflare may reject a deploy for the same reason (unverified). The new modules fix this, and a test guards it. The self-hosted bundle already used such an entry.
6. **The Registrar lives in the room script.** The router reaches it with `script_name = "kelpie-link-relay-room"`. Deploy order is unchanged: room first, then router.

## Unresolved / for later phases
- Phase 2 should handle `429 rate`, which old daemons read as "too many wrong keys today". Only daemons that turn on the relay without a key can hit it, and those are new daemons.
- **Phase 5:**
  - `relay.yml` should run the workerd smoke twice (open and key mode) and add the `wrangler dev` page smoke. I may not edit workflows.
  - Deploys need `public/` to exist. Checked with a local `wrangler deploy --dry-run -c wrangler.toml`: without `public/` it fails with "The directory specified by the assets.directory field ... does not exist". A router deploy therefore cannot go out without a bundle, which is a useful guard. The flip side is that `pnpm dev` / `pnpm dev:page` also need `public/`: for relay-only work, create an empty one (it is gitignored). Phase 7 docs should say so.
- **Self-hosted relays in open mode behind a proxy** see no `cf-connecting-ip`, so every client counts as one network and the cap is 20 rooms/day in total. This is documented in the `Dockerfile` and `config.capnp` comments. Docs belong to phase 7.
- A room-script deploy is needed for v2 (`origins`, version header, Registrar class), and it drops active room sockets (measured in phase 4a). The router must be deployed after the room script.
