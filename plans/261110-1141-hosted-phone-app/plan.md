---
title: "Hosted phone app: pair and connect without Tailscale"
description: "Serve the phone PWA from the relay worker's fixed HTTPS origin, open the relay to every install with abuse limits, pair over the relay, deploy only from CI with approval and a published bundle hash; Tailscale mode stays."
status: pending
priority: P1
effort: "17d (+ owner time for real phones and go-live)"
branch: master
tags: [mobile, relay, cloudflare, pwa, hosted, ci, security, webauthn]
created: 2026-11-10
blockedBy: []
blocks: []
---

# Hosted phone app (no Tailscale needed)

## Overview
Today a phone needs Tailscale on both ends, MagicDNS/HTTPS, two QR scans and a relay registration key, because the page comes only from
the daemon over `*.ts.net` (D5 of [261024-1205](../261024-1205-phone-remote-control/plan.md)) and the relay admits phones only from that
origin (`relay/src/room.ts:154-155`) and makes rooms only for the key holder (`relay/src/worker.ts:143-166`). This plan serves the same phone
app from the relay worker's own origin, opens room creation to every install with abuse limits, pairs over the relay, and deploys that
origin only from CI. Tailscale mode, the daemon-served page and every existing pairing keep working.

## Owner decisions — 2026-11-10 (fixed, not reopened)
1. The phone app is hosted **on the relay worker itself** at `https://kelpie-link-relay.baopeak.workers.dev` (one trust root, no new domain). Accepted: whoever controls that Cloudflare account or its deploy can serve code that reads keys and session content. **Amends D5** of the phone plan ("the relay never serves code").
2. The relay is **open to every Kelpie install**: no registration key; abuse limits per network and per room; Cloudflare cost considered.
3. The hosted app is deployed **only by CI from a release tag**, gated by a GitHub environment approval; each release publishes the deployed bundle's hash. Relay deploys move to CI too (H6).
4. **Tailscale mode stays** as an option. Default for new users = hosted app over the relay. Screen view/control and browser-pane takeover stay Tailscale/own-HTTPS only.

## Design decisions (this plan)
| # | Decision | Why / evidence |
|---|---|---|
| H1 | Hosted origin = the relay URL's own HTTPS origin (`http_url`, `crates/kelpied/src/mobile/relay.rs:157-162`), `wss://` relays only. Not a separate setting | One trust root; the page's relay socket is same-origin, so its CSP is `connect-src 'self' wss://<host>` |
| H2 | One source (`app/phone`), two builds: daemon-embedded `app/dist-phone` (unchanged, `crates/kelpied/build.rs:57`) and hosted `app/dist-phone-hosted` (`__HOSTED__` compile flag) | No second UI; the hosted bundle can never be embedded in the daemon |
| H3 | A device's page is a sealed record field (`page` + `page_kind`: `ts` / `own` / `hosted` / `plain`), bound to **the pairing code the desktop issued**, not to what the relay claims | The relay is untrusted; the code is the desktop's own word (`link.rs:519-597`) |
| H4 | Hosted devices: ceiling `write`; `control`, takeover and screen never (relay never carries control, `mobile.rs:1997-2009`); passkey rpId = hosted host; lock mandatory for hosted **[Q1]** | Bounds what a bad deploy can do silently |
| H5 | Relay v2: open registration (key still enforced when set), registration caps (rate-limit binding + one `Registrar` DO), rooms with an `origins` list, `o` (phone's Origin) in `open`, `x-kelpie-relay-version: 2` on the desktop's 101, static assets with `_headers`, `run_worker_first = ["/v1/*"]`, 404 page, usage flush 60 s | Phase 1; keeps v1 daemons and old self-hosted relays working |
| H6 | Router + assets deploy only from a `v*` tag through `release.yml`: staging, then production, each behind the owner's approval in its environment (one Cloudflare token cannot be limited to one script); room script by a dispatch from a tag with approval, only when room code changed | Assets are part of the router's version, so the router must move to CI; a room deploy drops active sockets (phase 4a: 5/5 active rooms) |
| H7 | Transparency: `hosted-phone-manifest.json` + bundle hash as release asset and in notes, `version.json` on the origin, `scripts/hosted/verify_live.py`, two-build determinism check. **No SRI** (hashed immutable names already bind content; SRI cannot guard against the deployer, who also serves `index.html`) | Owner asked for a way to check what is live |
| H8 | Service worker keeps the previous version's hashed assets and reloads the page at the next foreground after a new worker takes over; pairing record format additive only | Workers static assets keep only the current version's files; lazy chunks exist (`app/phone/src/chat-adapters.ts:8-10`, `qr-scan.ts:49`) |
| H9 | New setups: phone access on ⇒ relay on (one OS verification) and LAN listener off **[Q4]**; existing stores unchanged | Owner decision 4; no new open port for a user who only wants the hosted app |
| H10 | A self-hosted (workerd) relay serves no page; its computers' phones use Tailscale. Deploying the router with assets to one's own Cloudflare account works, and makes that account the trust root | KISS; Cloudflare applies `_headers`, plain workerd does not |

## Data flow
```
Desktop app ──mobile.pair_start {hosted}──▶ kelpied: Code{psk, page: hosted O}; relay `pair` {H(pair_token)}
   QR = https://O/#p=<dpk, psk, [relay wss://O + ticket]>   (fragment: never sent to any server)
Phone (installed PWA at O = https://kelpie-link-relay.baopeak.workers.dev)
   GET O/*            → Cloudflare static assets + _headers (no Worker run, free)
   WSS O/v1/phone?t=&p=|k=   Origin: O ──▶ router (ticket HMAC) ──▶ Room DO (O ∈ origins; pair/token hash) ──open{s,via,k,o:O}──▶
kelpied relay.rs ──▶ link::accept(Peer::relayed(slot, o)) ──▶ pair: o must equal Code.page ──▶ person types 6 digits + OS verification
   ──▶ Device{secure_origin, page O, page_kind hosted} sealed ──▶ relay token (inside Noise) ──▶ passkey enrol (rpId = O host)
Reconnect: phone ──token──▶ relay ──▶ kelpied: device.page == o, passkey, verbs ≤ write
CI: tag v* → build ×2 → manifest/hash → draft release assets+notes → [approval] staging deploy+verify → [approval] production deploy → verify_live → publish draft
```

## Phases
Execution: strictly sequential (owner preference: full review/test/fix loop per phase), one branch per phase off master, own commits, no push without the owner. Phases 1 and 2 own disjoint files (`relay/**` vs `crates/**`; phase 2 tests use a fake relay) and could run in parallel if the owner ever asks.

| # | Phase | Effort | Depends on | Owns | Status |
|---|---|---|---|---|---|
| 1 | [Relay v2: open rooms, abuse limits, many origins, hosted page](phase-01-relay-v2-open-rooms-hosting.md) | 2.5d | — | `relay/**` | done |
| 2 | [Daemon: hosted pairing, device page, relay v2 client](phase-02-daemon-hosted-pairing.md) | 4d | 1 | `crates/**`, `app/src/lib/attention.ts`, `app/src/lib/types.ts` | done |
| 3 | [Phone: hosted build and behaviour, SW versioning, manifest](phase-03-phone-hosted-build.md) | 3d | 2 | `app/phone/**`, `app/vite.phone.config.ts`, `app/package.json`, `scripts/hosted/bundle_manifest.py`, `.gitignore`, `app/src/locales/vi/phone.json` | done |
| 4 | [Desktop Settings → Mobile: hosted default, trust text](phase-04-desktop-settings.md) | 2d | 2, 3 | `app/src/components/settings/mobile-*.tsx`, `app/src/lib/mobile-options.ts`, `settings-catalog.ts`, `app/src/locales/vi/settings.json` | done |
| 5 | [CI deploy, approval, published hash, live verification](phase-05-ci-deploy-verification.md) | 2d | 1, 3 | `.github/workflows/*`, `scripts/hosted/verify_live.py`, `[env.staging]` in `relay/wrangler*.toml` | pending |
| 6 | [E2E on an emulated phone, existing e2e updated](phase-06-e2e-emulated-phone.md) | 2d | 1–5 | `scripts/e2e/mobile_hosted.py`, `mobile_relay.py`, `mobile.py` | pending |
| 7 | [Docs](phase-07-docs.md) | 1d | 1–6 | `docs/*.md`, `README.md` | pending |
| 8 | [Go-live (owner/maintainer)](phase-08-go-live.md) | 0.5d + owner time | 1–7 | none (settings, secrets, real phones) | pending |

API level: phase 2 takes the next free `API_LEVEL` at commit time (34 expected; `crates/kelpie-proto/src/lib.rs:57` is 33). Coordinate with the file-transfer phone phases (upload/download), which also touch `app/phone` and `mobile.rs` [from memory 2026-11-08, verify branch state first]: whichever lands second renumbers and moves its gates.

## Backwards compatibility (summary; detail in phases 1–3)
- **Tailscale users**: records without `page` behave exactly as now; the room's `origins` keeps the Tailscale origin; daemon-served page, CSP and `/link` unchanged.
- **Old daemons (≤ 33) on relay v2**: `origin` message still accepted; Authorization ignored in open mode; no header read. Old daemons still demand a key before turning the relay on (client-side, cannot change): existing rooms keep working, new users update Kelpie.
- **New daemon on an old (v1) relay** (self-hosted): no version header ⇒ v1 messages only, no hosted option, key flow as today.
- **Store**: additive fields with their own tags (pattern of `secure_mac`, `store.rs:139-142`); `VERSION` stays 2; an older Kelpie reads it and its relay refuses hosted phones by origin (fails closed).
- **Hosted bundle vs daemon**: bundle serves daemons with API ≥ `HOSTED_API`; newer features by `caps` only; production deploy goes live before the desktop release is published.

## Trust trade-off: what the mitigations give, and what they cannot
| Measure | Gives | Cannot give |
|---|---|---|
| CI-only deploy from a tag + environment approval; deploy token only in environments | Every intended change is reviewed, tagged, logged | The account owner, or whoever steals the account or a session, can still deploy from the dashboard or API |
| Manifest + hash in release, `version.json`, `verify_live.py`, determinism check | Anyone can detect a broad swap, after the fact, and rebuild from the tag | Code served only to one phone or between checks; what a given phone ran |
| Immutable hashed asset names | A cached file is the file its name says | Anything against the deployer |
| Daemon-enforced passkey lock (UV), mandatory for hosted [Q1] | A key copied by changed code (e.g. a service worker at push time) reaches nothing; changed code acts only while the person has the app unlocked | While unlocked, changed code sees and sends what the device may |
| Hosted ceiling `write`, no control/screen | A changed page cannot drive the desktop screen or browser panes | It can type into terminals/chats the device may write to, i.e. run commands through a session |
| "What this trusts" in Settings and on the phone; Tailscale alternative | An informed choice | — |
| In-app version/hash display | Support and skew diagnosis | Proof: changed code can claim any version |

## Cost (detail in phase 1)
Static asset requests are free and unlimited on Workers. The binding Free-plan constraint is Durable Object duration: an active room is billed at 128 MB, so 13,000 GB-s/day ≈ **29 active room-hours per day for all users together** [E]; rows written (100k/day) next. Past any Free limit the relay fails for everyone until 00:00 UTC. Hence Q5.

## Test matrix (summary)
| Layer | What | Where |
|---|---|---|
| Unit (relay vitest, workers pool) | registration modes, caps, origins, `o`, version header, nosniff, no reflected input, flush cadence, no timers | phase 1 |
| Unit (cargo) | hosted code binding, page tags, ceilings, lock path, token life, v1/v2 relay negotiation, store up/down compat, LAN off, test-CA seam gating | phase 2 |
| Unit (vitest app) | hosted routes, pairing over relay only, replace confirm, install-first, SW retention, headers ≡ daemon CSP, record format frozen | phase 3, 4 |
| Integration | `pnpm smoke` (open + key modes), `wrangler dev --local` page smoke, two-build determinism | phases 1, 5 |
| E2E (isolated `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR`) | `mobile_hosted.py` (Edge mobile emulation, virtual authenticator, push stand-in, SW update), `mobile_relay.py`, `mobile.py` | phase 6 |
| Live | `verify_live.py` staging + production; real iPhone + Android | phases 5, 8 |

## Acceptance criteria
- [ ] With no Tailscale anywhere: turn on phone access (one OS verification, no registration key) → install → scan → type 6 digits + verify → sessions on mobile data. Green in `mobile_hosted.py`; done once on a real iPhone and a real Android phone.
- [ ] Hosted device: passkey (rpId = hosted host) required; typing works after unlock; `control`, takeover and screen refused by the daemon and never offered in Settings.
- [ ] Tailscale-paired phones work after the upgrade without re-pairing (direct and via relay); `mobile.py` and `mobile_relay.py` pass; a 0.1.13 store loads; an older reader accepts the new store.
- [ ] Relay: rooms without a key; per-network and global caps answer 429/503 with reasons; setting `REGISTRATION_KEY` closes it; v1 daemons connect; a phone from an unregistered origin is refused.
- [ ] Production deploy only from a `v*` tag after `hosted-production` approval; Cloudflare token only in environments (`gh api` check); release notes and assets carry the bundle hash; `verify_live.py` exits 0 on production and non-zero on a tampered copy.
- [ ] Two clean builds of one tag give one bundle hash.
- [ ] Deploying version B while A is open: no failed chunk load; A reloads into B at its next foreground.
- [ ] Docs updated (user guide, relay-self-host, architecture, link-protocol, README); EN + VI; `pnpm typecheck && pnpm test && pnpm i18n:check`, `cargo test -p kelpied -p kelpie-proto -p kelpie-link`, relay `pnpm typecheck && pnpm test` green; phone access off ⇒ no sockets (perf gate unchanged).
- [ ] 7 days after go-live, Cloudflare analytics within the chosen plan's limits.

## Top risks (cross-phase)
| Risk | L×I | Mitigation |
|---|---|---|
| Open rooms exhaust shared Free limits ⇒ relay down for all | H×H | Registrar caps, 404 page without Worker, flush 60 s, close switch, Q5 |
| Bad or mistaken deploy reaches every hosted phone | M×H | Staging + approval + verify; mandatory lock; `write` ceiling; rollback = redeploy previous tag |
| Origin confusion lets a page pair or unlock as another kind | L×H | Code-bound `page_kind`, sealed `page`, relay `o` cross-check, tests |
| Old page loads a deleted chunk after a deploy | H×M | SW keeps previous cache; reload at foreground; e2e |
| Relay version skew (new daemon, old room; or reverse) | M×H | Version header gating; v1 messages kept; router deploy fails without the new room class |

## Rollback (summary; per phase in each file)
Production: redeploy the previous tag's bundle through the same pipeline (or `wrangler versions rollback` once phase 5 proves it restores assets); close registration by setting `REGISTRATION_KEY`. Code: revert the phase's commits; store fields are additive, so older builds read the store and fail closed for hosted phones.

## Links
Phone plan: [plan](../261024-1205-phone-remote-control/plan.md) · [phase 3](../261024-1205-phone-remote-control/phase-03-kelpie-link-e2e.md) · [3a Tailscale](../261024-1205-phone-remote-control/phase-03a-tailscale-https.md) · [3c lock](../261024-1205-phone-remote-control/phase-03c-app-lock.md) · [4 relay](../261024-1205-phone-remote-control/phase-04-relay-connect-anywhere.md) · [4a validation](../261024-1205-phone-remote-control/phase-04a-relay-cloudflare-validation.md). Docs: `docs/user-guide.md:1071-1253`, `docs/relay-self-host.md`, `docs/link-protocol.md`, `docs/architecture.md:559-660`.

## Owner answers — 2026-11-10
The owner took every recommendation below: 1a, 2a, 3a, 4a, 5a, 6a, 7a, 8 (defaults), 9a. Kept as asked for the record.

## Questions answered (owner)
1. **Lock for hosted devices.** (a) Always required for hosted devices, even with *Lock Kelpie on phones* off; (b) follow the switch like Tailscale devices. **Recommend (a)**: the daemon-checked passkey is the one control that limits a changed deploy to the moments the person unlocks the app.
2. **Several computers in one hosted app.** (a) One computer per app; pairing another asks to replace it and unsubscribes its alerts (in phase 3); (b) multi-computer store and switcher (+~3d phone, alerts per computer). **Recommend (a)** for v1, (b) on demand.
3. **Relay token life for hosted devices.** (a) As long as the dormancy limit (90 d default; "never" → 365 d); (b) keep 30 d, so a hosted phone unused for a month must pair again (the relay is its only way). **Recommend (a)**; a token alone admits to the room, never to a session.
4. **LAN listener for new setups.** (a) Off for new hosted setups, on for existing ones, switch in Advanced; (b) on as today. **Recommend (a)**.
5. **Cloudflare plan before hosted is the default.** (a) Workers Paid ($5/month base) with usage notifications before go-live; (b) stay Free, accepting a shared outage when daily limits are hit (~29 active room-hours/day [E]). **Recommend (a)**.
6. **Hand access after CI** (reverses your 2026-10-31 "keep and reuse the token" only if you choose so). (a) Keep the personal token for break-glass only: `wrangler versions rollback` to a CI-made version and secret changes (closing registration), never a code/asset deploy, each use noted; (b) revoke it, CI only; (c) unchanged. **Recommend (a)**.
7. **Room-script deploys.** (a) Dispatch from a release tag with approval, only when room code changed; (b) every release (drops active phone sockets each time). **Recommend (a)**.
8. **Registration limits.** Defaults 5/min burst, 20/day per network, 1,000/day overall, tunable vars without code change. Per-IPv4 limits hit CGNAT households together. **Recommend** these defaults, revisit with analytics.
9. **A "Check the hosted app" button on the desktop.** (a) Not in v1: script + docs; (b) add (+0.5d), same blind spot for targeted serving. **Recommend (a)**.
