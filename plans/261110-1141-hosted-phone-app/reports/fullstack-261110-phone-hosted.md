# Phone: hosted build and behaviour, SW versioning, manifest: implementation report

Date: 2026-11-10. Branch `phone/hosted-phone-app` (worktree `D:\kelpie-hosted-p3`, on top of `phone/hosted-daemon`), not pushed. Feature commit
`45fd9a9`. No daemon was started, nothing reached Cloudflare or the real relay.

## What changed
- **Build** (`app/vite.phone.config.ts`, `app/package.json`, `.gitignore`): `--mode hosted` (`pnpm build:phone:hosted`) defines `__HOSTED__ = true` and
  writes `app/dist-phone-hosted/`; the default mode defines `__HOSTED__ = false` and writes `dist-phone` as before. Both define `__APP_VERSION__`
  (`KELPIE_APP_VERSION`, else `app/package.json`). The hosted build emits `_headers` and `404.html`; the worker's precache list skips both. The default
  build fails if `/v1/health` (the hosted probe) reaches any chunk: Rolldown inlines `HOSTED` and drops the hosted branches (checked in the output).
- **Headers** (`app/phone/security-headers.ts`): the daemon's `ONLY_SELF` directive by directive, `hostedCsp(origin)` = same with
  `connect-src 'self' wss://<host>`, `nosniff`, `no-referrer`, `DENY` on `/*`; `no-store` on the six shell files; `immutable` on `/assets/*`;
  `hostedOrigin()` accepts only `https://<name>[:port]` (default `https://kelpie-link-relay.baopeak.workers.dev`), the build throws otherwise.
- **`hosted.ts`**: `HOSTED`, `APP_VERSION`, `HEALTH_PATH`, `servedVersion()` (`/version.json`, `cache: no-store`, checks `app` and a 64-hex hash),
  `trustText(t)`, `replaceQuestion(t, a, b)`, `onHost()`.
- **Link** (`link.ts`, `conn-state.ts`, `conn-text.ts`): `Env.url()` may be null (no direct route; the race is the relay alone, no direct probe or
  migration); `Env.relayHost()` names the only relay host allowed. A relay elsewhere is not dialled: `facts.relay = "elsewhere"` ⇒ state `relay_moved`
  ("Pair this phone again"). Hosted reach = `GET /v1/health` expecting `text/plain`. `no_tailnet`/`no_path` are shown as "relay unreachable" on
  the hosted app, so it never says "Turn on Tailscale".
- **Pairing** (`pair.ts`, `phone-state.ts`, `push.ts` comment): `pair(..., hosted)` opens only the relay socket on the page's host, adds `page:
  location.origin` to the sealed hello, refuses a code for another relay without opening anything. When the held pairing's `dpk` differs it asks
  "This app is paired with A. Pair with B instead? A loses this phone."; yes ⇒ `lock.forget()`, `alerts.forget()` (ends the browser's push
  subscription), then pairs; no ⇒ nothing opened, back to A with "Nothing changed". After a successful replacement the old computer's cached list,
  screens, drafts and kept messages are cleared, and the trust sheet opens.
- **Install first** (`install-state.ts`, `install-guide.tsx`): `PageFacts.hosted`; `installFirst = hosted || tailnet`; `readCode` unchanged.
  `relayMovedTo()` drives the "moved" banner on the hosted app (relay host instead of the Tailscale address).
- **Trust sheet** (`app.tsx`): "What this app trusts" in the connection sheet (hosted only) and once after each pairing: version, the relay's stated
  version and bundle hash (network only), and the five statements of requirement 7.
- **Service worker** (`sw.ts`, both builds): activation keeps this version's cache and the previous one by an order list in the
  `kelpie-sw-versions` cache (first run after an older worker: the cache it left counts as previous), deletes older ones; `/assets/*` via
  `caches.match` across kept caches, then network into this version's cache; `/version.json` bypassed. `shell-reload.ts` `watchWorker()`
  (`main.tsx`): `controllerchange` on a page that already had a worker ⇒ `shellChanged()` ⇒ reload at the next foreground, at most once a minute.
- **Record frozen** (`pairing-store.ts`): `readPairing()` exported (used by `loadPairing`); fixture of a 0.1.13 record loads unchanged.
- **Manifest** (`scripts/hosted/bundle_manifest.py`, stdlib): lines `path sha256 size` over every file but `version.json`, `_headers` required
  and included; `bundle_sha256` = SHA-256 of the lines (each ending `\n`); writes `<dir>/version.json` (`app, version=<tag>, commit, api_min =
  HOSTED_API read from attention.ts, bundle_sha256`) and `hosted-phone-manifest.json` beside the dir (also holds `files` and the `_headers` text
  for `verify_live.py`). No timestamps. Importable functions: `bundle_files`, `bundle_hash`, `build`, `write`, `hosted_api`.
- **Strings**: 17 new EN keys, VI in `app/src/locales/vi/phone.json`.

## Tests
- `pnpm test`: **94 files, 1750 passed** (before: 92 files, 1717). `pnpm typecheck` clean. `pnpm i18n:check`: 0 missing.
- `python -I scripts/hosted/test_bundle_manifest.py`: 10 passed.
- New: `link.test.ts` (relay alone, never `/link`, no direct probe/migration; retries relay only; relay on another host not dialled ⇒ `relay_moved`;
  hosted `browserEnv` probes `/v1/health` and has no direct URL; daemon `browserEnv` unchanged; 0.1.13 record fixture + first-version record);
  `pair.test.ts` (relay-only with `page`, other relay refused with no socket, replace asks then leaves then dials, declining opens nothing and
  keeps the record, same computer asks nothing); `install-state.test.ts` (hosted install first, `relayMovedTo`); `sw.test.ts` (A's chunk from A's
  cache under B; A survives B, goes at C; order by record not name; first run after an older worker; network fallback kept; `version.json`
  bypassed; `controllerchange` ⇒ one reload at foreground, none for the first worker, once a minute); `hosted.test.ts`; `security-headers.test.ts`
  (CSP equals `ONLY_SELF` read from `mobile.rs`, differs only in `connect-src`, `_headers` rules, origin validation, 404 page has no script/style).

## Validation
- `pnpm build:phone` and `pnpm build:phone:hosted` succeed. Two clean hosted builds of `45fd9a9` into scratch dirs (deleted) +
  `bundle_manifest.py --tag v0.1.15 --commit 45fd9a9…`: both `bundle_sha256 5263a297e80c2208436012966fce6121d0407fb1390e22d57791ba53fc67f46b`.
  `KELPIE_APP_VERSION=9.9.9-test` gives a different hash; `KELPIE_HOSTED_ORIGIN=http://…` fails the build.
- **`wrangler dev --local`** (optional criterion, done): relay deps installed offline from the pnpm store, bundle copied into `relay/public`
  (deleted afterwards, with `.wrangler/`), `WRANGLER_SEND_METRICS=false`, 127.0.0.1:8787 only. `/`, `/sw.js`, `/version.json`,
  `/manifest.webmanifest` came with the CSP, `X-Frame-Options: DENY` and `no-store`; the hashed script with `immutable`; `/no-such-file` 404;
  `/v1/health` 200 text/plain. Headless Edge loaded `/` and rendered the app with no CSP violation in its console log. Over plain
  `http://127.0.0.1` the page shows the pairing screen, so the install/scan screens were not exercised there (phase 6, HTTPS).
  Processes started for this (wrangler, headless Edge) were stopped.

## Departures from the spec
1. `security-headers.test.ts` lives in `app/phone/src/` (imports `../security-headers`), not `app/phone/`: Vitest only collects
   `phone/src/**/*.test.ts` and `app/vitest.config.ts` is outside this phase's files. Same reason `__HOSTED__`/`__APP_VERSION__` are read through
   `typeof` guards in `hosted.ts` (tests have no `define`); the build still drops the hosted branches.
2. Extra files: `app/phone/src/shell-reload.ts` (`watchWorker`, the testable part of the `controllerchange` rule; `app/phone/**` is this phase's
   in plan.md) and `scripts/hosted/test_bundle_manifest.py` (the fixture test the steps ask for). `.gitignore` also ignores
   `hosted-phone-manifest.json`, which the script writes beside `app/dist-phone-hosted/`, i.e. into `app/`.
3. `hosted.ts` takes the translator as an argument (`trustText(t)`, `replaceQuestion(t, …)`) so that `link.ts` can import it without the phone
   dictionary (the link tests load no `virtual:phone-vi`).
4. The leak guard is a build-time check in `vite.phone.config.ts` (fails the default build on `/v1/health` in any chunk) rather than a unit test
   that reads `dist-phone`.
5. The trust sheet opens after **every** successful hosted pairing (not once ever), and on replacing a computer the app also clears the old
   computer's cached list, screens, drafts and kept messages after the new pairing succeeds.
6. `version.json`'s `version` is the tag as given (e.g. `v0.1.15`); the sheet shows both the build's `APP_VERSION` (`0.1.15`) and that.

## Open questions
- The daemon bundle still carries the hosted-only **English keys** with their VI text in its dictionary (the dictionary is built from every
  `t("…")` in `app/phone/src`, whatever the mode): data only, no code. Filtering it per mode needs `vite-dictionary.ts`; worth it?
- A hosted pairing whose relay token expired (or relay turned off) has no route at all and shows "Reconnecting" indefinitely; a dedicated state
  ("pair again") could follow if the daemon's long hosted token life (90 d+) ever proves too short.
- If a person accepts replacing computer A and the pairing with B then fails, the app returns to A with alerts off and the lock state forgotten
  (as the spec orders: leave first, then pair).
- Reproducibility was checked on Windows only; CRLF checkouts (`core.autocrlf=true`) could make a Windows bundle differ from CI's Linux one
  (phase 5 compares on Linux).
