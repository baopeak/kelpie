---
phase: 3
title: "Phone: hosted build and behaviour, SW versioning, manifest"
status: pending
priority: P1
effort: "3d"
dependencies: [2]
---

# Phase 3: Phone hosted build

## Context
- Build: `app/vite.phone.config.ts` (outDir `dist-phone` `:54`, service-worker plugin fills `__PRECACHE__`/`__VERSION__` `:29-41`, entry names `:61`); `app/package.json:9` `build:phone`; the daemon embeds `app/dist-phone` (`crates/kelpied/build.rs:57`); `.gitignore:10`.
- Routes: direct route is the page's own `/link` (`app/phone/src/link.ts:284`), reach probe on the page origin (`:290-303`), race in `open()` (`:760-781`); relay URL from the pairing (`pairing-store.ts:28-32`), one pairing per origin (`:34-36`, `savePairing` `:95-111`).
- Pairing: direct then relay (`pair.ts:63-72`), relay pair URL from the QR (`:55-58`), hello asks `read` (`:75`); `pairDevice` opens `wss://<host>/link` first (`phone-state.ts:676-704`); code read from the fragment and stripped (`:747-755`).
- Install-first only on `*.ts.net` (`install-guide.tsx:35`, `install-state.ts:31-35`); scanned link must be this host (`readCode` `:48-64`).
- Service worker: cache per version (`sw.ts:49`), install precaches and `skipWaiting` (`:95-97`), activate deletes every other version's cache (`:99-106`), `/assets/` from this version's cache then network (`:184-194`), bypass list (`:163`). Reload machinery: `ShellReload` (`shell-reload.ts:55-77`); registration (`main.tsx:34-36`).
- Lazy chunks exist: `chat-adapters.ts:8-10`, `qr-scan.ts:49`.
- Daemon page CSP: `ONLY_SELF` (`crates/kelpied/src/mobile.rs:2082`).

## Overview
Build the same app a second time for the hosted origin: relay-only connections, pairing over the relay, install first, a "What this trusts" sheet, headers for Cloudflare, and a manifest with one bundle hash. Fix version skew for both builds in the service worker.

## Requirements
1. **Two builds, one source.** `vite build --config vite.phone.config.ts --mode hosted` defines `__HOSTED__ = true`, `__APP_VERSION__` (from `app/package.json`, or `KELPIE_APP_VERSION` when set, which phase 6 uses to make a second, different bundle), writes `app/dist-phone-hosted/`; the default mode defines `__HOSTED__ = false` and writes `dist-phone` as today. `build:phone:hosted` script. `.gitignore` adds `app/dist-phone-hosted/`. Dead-code elimination removes hosted-only branches from the daemon bundle.
2. **Headers, one source.** `app/phone/security-headers.ts` holds the CSP parts and header set; the hosted build emits `_headers`:
   - `/*`: CSP = the daemon's `ONLY_SELF` with `connect-src 'self' wss://<hosted host>` (explicit `wss:` so no browser's reading of `'self'` for WebSockets matters), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`.
   - `/`, `/index.html`, `/sw.js`, `/manifest.webmanifest`, `/version.json`, `/404.html`: `Cache-Control: no-store`. `/assets/*`: `public, max-age=31536000, immutable`.
   - Hosted origin from `KELPIE_HOSTED_ORIGIN`, default `https://kelpie-link-relay.baopeak.workers.dev`; must be `https://<name>[:port]`.
   - A vitest test reads `ONLY_SELF` from `crates/kelpied/src/mobile.rs` and asserts the hosted CSP equals it apart from `connect-src`.
   - Emits a static `404.html` (no script).
3. **Manifest and hash.** `scripts/hosted/bundle_manifest.py <dir> --tag <v> --commit <sha>` (stdlib): sorted lines `path sha256 size` over every file except `version.json`, plus `_headers`' hash; `bundle_sha256` = SHA-256 of those lines; writes `<dir>/version.json` (`{app: "kelpie-phone", version, commit, api_min: HOSTED_API, bundle_sha256}`) and `hosted-phone-manifest.json` beside the dir (never deployed). Functions importable by `verify_live.py` (phase 5). No timestamps anywhere in the output.
4. **Hosted routes** (`__HOSTED__`): no direct route (`browserEnv().url()` gives none; `open()` then races the relay alone); reach probe = `GET /v1/health` on the origin; a relay endpoint is used only when its host is this page's host, else the app says the computer moved to another relay and must be paired again. Never shows "Turn on Tailscale".
5. **Hosted pairing**: only the relay attempt (no `/link` socket), pair URL on this host; the sealed `pair` hello adds `page: location.origin` (information; the daemon binds to the code). If this app already holds a pairing with another computer (`dpk` differs): confirm "This app is paired with <A>. Pair with <B> instead? <A> loses this phone." On yes: unsubscribe alerts (`pushManager.getSubscription()?.unsubscribe()`, which voids the old computer's endpoint at the push service), forget the lock state, then pair. [Q2]
6. **Install first** on the hosted origin as on `*.ts.net` (`installFirst = hosted || tailnet`); `readCode` unchanged.
7. **"What this trusts" sheet** (hosted only, from the app's menu and once after pairing): app version, the bundle hash from `/version.json` (network only), and plain text: the app comes from Kelpie's relay on Cloudflare; whoever controls that account or Kelpie's release deploy could send different code that reads and types while the app is unlocked; each release publishes the app's fingerprint and how to check it; this screen cannot prove what this phone runs; for an app served only by your computer, use Tailscale mode.
8. **Service worker versioning** (both builds):
   - Activate keeps the newest previous `kelpie-phone-*` cache and deletes older ones (order kept in a small entry, not by name).
   - `/assets/*` is looked up with `caches.match` across kept caches before the network (names are content hashes).
   - `/version.json` bypasses the worker like `/manifest.webmanifest`.
   - Page: `navigator.serviceWorker` `controllerchange` ⇒ `shellReload.shellChanged()` ⇒ reload at the next foreground, at most once a minute; typed drafts survive (`kept-messages.ts`).
9. **Pairing record frozen**: additive fields only; a record saved by the 0.1.13 phone loads (fixture test).
10. **Strings**: EN keys, VI in `app/src/locales/vi/phone.json`; `pnpm i18n:check`.

## Related code files
- Modify: `app/vite.phone.config.ts`, `app/package.json`, `.gitignore`, `app/phone/src/env.d.ts`, `link.ts`, `conn-state.ts`, `conn-text.ts`, `pair.ts`, `pairing-store.ts`, `install-state.ts`, `install-guide.tsx`, `phone-state.ts`, `push.ts`, `sw.ts`, `main.tsx`, `app.tsx`, tests `link.test.ts`, `pair.test.ts`, `install-state.test.ts`, `sw.test.ts`; `app/src/locales/vi/phone.json`.
- Create: `app/phone/security-headers.ts`, `app/phone/src/hosted.ts` (flag, version fetch, trust text), `app/phone/src/hosted.test.ts`, `app/phone/security-headers.test.ts`, `scripts/hosted/bundle_manifest.py`.
- Not touched: `crates/**` (phase 2), `app/src/lib/attention.ts` (read only: `HOSTED_API`).

## Implementation steps
1. Build mode, defines, outDir, `.gitignore`; confirm `dist-phone` output is unchanged in behaviour (existing tests).
2. `security-headers.ts`, `_headers` + `404.html` emission, CSP equality test.
3. `hosted.ts`; link/reach/relay URL rules; pairing path and replace confirmation; install-first.
4. Trust sheet and strings.
5. Service-worker retention and reload on a new controller.
6. `bundle_manifest.py` with a fixture test (`python -I -m unittest` style, stdlib only).
7. `cd app && pnpm typecheck && pnpm test && pnpm i18n:check && pnpm build:phone && pnpm build:phone:hosted`.

## Tests
- Hosted link opens only a relay socket, never `/link`; a relay endpoint on another host is ignored and reported; reach uses `/v1/health`.
- Hosted pairing never opens `/link`; replacing another computer's pairing needs the confirmation and unsubscribes first; declining keeps the old pairing.
- `firstScreen` is `install` on the hosted origin when not installed.
- SW: with versions A then B installed, an A asset is served from A's cache; A's cache survives B's activation and goes at C's; `version.json` never cached; `controllerchange` schedules one reload at foreground.
- `_headers` CSP ≡ daemon `ONLY_SELF` except `connect-src`; `/assets/*` immutable; `no-store` on the shell files.
- Manifest: same input ⇒ same hash; changing one byte or one header line changes it; `version.json` excluded; no timestamps.
- 0.1.13 pairing record fixture loads.

## Validation
Two consecutive `pnpm build:phone:hosted` + `bundle_manifest.py` runs in clean output dirs give one `bundle_sha256` (on this machine and later in CI). The daemon bundle's tests and `scripts/e2e/mobile.py` stay green (run in phase 6).

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| Hosted-only code leaks into the daemon bundle | L×M | compile-time flag; a test greps `dist-phone` for hosted-only strings |
| Old page requests a chunk removed by a deploy | H×M | cross-cache lookup + reload at foreground; e2e in phase 6 |
| CRLF or platform differences break reproducibility | M×M | reproduce on Linux/CI; `--rebuild` documented for Linux/WSL; live-vs-published check does not depend on it |
| Replace confirmation skipped by a crafted link | L×M | the check runs on the stored `dpk`, whatever the link says |
| iOS reads storage per Home Screen app | — | unchanged from the Tailscale flow (install first, then scan inside the app) |

## Rollback
Revert the commit; the hosted bundle is not deployed before phase 5/8, and the daemon bundle reverts with it.

## Success criteria
Tests green; both builds succeed; the two-build hash check matches; a hosted build served by `wrangler dev --local` (phase 1 `dev:page`) loads with no CSP violation in a desktop Chromium.
