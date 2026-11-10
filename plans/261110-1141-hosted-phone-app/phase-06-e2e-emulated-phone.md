---
phase: 6
title: "E2E on an emulated phone; existing e2e updated"
status: done
priority: P1
effort: "2d"
dependencies: [1, 2, 3, 4, 5]
---

# Phase 6: E2E on an emulated phone

## Context
- Isolation helper: `scripts/e2e/phone_support.py` `isolate()` sets `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR` in the script's own process and refuses foreign endpoints (`:44-55`, `:58-64`).
- Relay e2e today: `scripts/e2e/mobile_relay.py` (docstring `:1-26`): debug daemon, fake Tailscale, local workerd with secrets in its own process (`start_workerd` `:137-163`), TCP taps that look for plaintext, no browser, nothing reaches Cloudflare.
- Phone-in-browser e2e today: `scripts/e2e/mobile.py` (`:1-35`): tauri-driver app + headless Edge with mobile emulation (msedgedriver), HTTPS at `localhost` with a run certificate the browser accepts, CDP virtual authenticator, push stand-in (`KELPIE_PUSH_TEST_BASE`, `ServiceWorker.deliverPushMessage`).
- Phase 2 test seams: `KELPIE_RELAY_TEST_CA`, `KELPIE_RELAY_TEST_URL`, loopback-only relay under `KELPIE_E2E`.
- House rules: e2e never touches the real daemon; never loop-launch the installed `kelpied.exe` (use a debug build); hard-killing real agent sessions disables the owner's TUI setting (none are started here).

## Overview
Prove the whole hosted path on this machine with an emulated phone: new-user setup, pairing through the relay only, lock, typing, refusals, alerts, a deploy of a new version while the app is open, and limits. Keep the Tailscale and relay e2e green with the new relay.

## Requirements
### New `scripts/e2e/mobile_hosted.py`
- **Setup**: `ps.isolate("phone-hosted")`; debug `kelpied` with `KELPIE_E2E`, `KELPIE_TEST_USER_PRESENCE=approve`, `KELPIE_RELAY_TEST_CA`, `KELPIE_RELAY_TEST_URL`; no `KELPIE_TAILSCALE_BIN` (no Tailscale at all). A run certificate for `localhost` (Python `cryptography`, as `mobile.py`). Port deterministic per PID (`47000 + pid % 1000` style).
- **Relay + page**: build the hosted bundle with `KELPIE_HOSTED_ORIGIN=https://localhost:<port>` into a run folder, `bundle_manifest.py` on it, copy into a temporary copy of `relay/` (never the repo's `relay/public`), start `wrangler dev --local` with HTTPS on that certificate [flags: verify `--local-protocol https --https-key-path --https-cert-path`], `--persist-to <run dir>`, `--var TICKET_SECRET:<random> --var OPEN_REGISTRATION:1`, metrics off. Record the PID; stop the whole process tree at the end (Windows `taskkill /T /PID`), even on failure.
- **Phone**: headless Edge, phone viewport, `acceptInsecureCerts`, CDP virtual authenticator (internal, UV) for rpId `localhost`; `pushManager.subscribe` replaced in the page as `mobile.py` does.
- **Checks**:
  1. Daemon: phone access on with `lan: false` + relay on with no key; room made; status `hosted.available`; no LAN listener bound (no socket on port 4817).
  2. Page `https://localhost:<port>/` shows the install step; after the install emulation (as `mobile.py`), the hosted pairing link pairs **through the relay only** (CDP network log: no socket but `wss://localhost:<port>/v1/phone`), digits approved through the daemon; device `page_kind: hosted`, relay token held.
  3. Lock: passkey enrolled in the window; reload ⇒ locked; unlock ⇒ sessions; a `link_noise.py` client with the phone's copied key but no passkey gets no session data (Q1 a: also with the global lock off).
  4. Write: type into a shell session, read the result back. `mobile.set_permission control` refused; `tk_start` and `sc_start` refused.
  5. Alerts: stand-in push service; `navigate` is `https://localhost:<port>/#s=…`; a synthetic `notificationclick` opens the hosted page.
  6. Headers and CSP: response headers match `_headers`; no `securitypolicyviolation`; `verify_live.py --origin https://localhost:<port> --manifest <run manifest>` exits 0, and 1 after one byte of a served file is changed in the run copy.
  7. New version while open: build B (`KELPIE_APP_VERSION=<v>-b`), swap the assets, restart wrangler on the same persist folder; with page A open and a chat not yet opened, open a chat (A's lazy adapter chunk) ⇒ loads from A's kept cache, no 404; background → foreground ⇒ one reload into B; drafts kept.
  8. Replace: a second isolated daemon issues a hosted code; the phone asks before replacing; after yes the first daemon's push endpoint is gone (stand-in sees the unsubscribe or a 410 path).
  9. Limits: a wrangler run with `ROOMS_PER_NETWORK_DAY:2`: the third room ⇒ the daemon's status says "too many rooms today"; with `REGISTRATION_KEY` set and no key ⇒ "needs a registration key".
  10. Nothing a session said is in the persist folder (marker and phone name searched in the DO SQLite files).
- **GUI part** (when run with tauri-driver, prerequisites as `mobile.py`): as a new user, Settings → Mobile → turn on phone access ⇒ one verification ⇒ guide opens; Step 1 install link; Step 2 link (from `data-link`) opened in Edge pairs; the approval dialog for a hosted phone offers no control; device badge *hosted app*; "What this trusts" text present.
- **Never**: Cloudflare (loopback-only guard), the default daemon, the installed `kelpied.exe`, `relay/public` or `relay/.dev.vars` in the repo.

### Updated `scripts/e2e/mobile_relay.py`
- Open registration (no key) and key mode both make rooms; `origins` sent on a v2 relay; `o` arrives in `open`; a phone with a foreign `Origin` refused; a header-suppressed relay (v1 behaviour) still works with `origin` only (via a small proxy that strips `x-kelpie-relay-version`).

### Updated `scripts/e2e/mobile.py`
- Settings rows: hosted rows absent/explained without a relay; plain pairing hidden when LAN is off; existing checks unchanged.

## Related code files
- Create: `scripts/e2e/mobile_hosted.py`.
- Modify: `scripts/e2e/mobile_relay.py`, `scripts/e2e/mobile.py`, `scripts/e2e/phone_support.py` (shared helpers: certificate, wrangler start/stop, only if needed by two scripts).

## Implementation steps
1. Helpers (cert, wrangler lifecycle, process-tree stop) with a dry `--help` smoke.
2. Daemon-only checks 1–6, 9, 10; then 7, 8.
3. GUI part.
4. `mobile_relay.py`, `mobile.py` updates.
5. Run all three; record results in the phase's report (outside this plan, in `plans/reports/`).

## Validation
- `python -X utf8 scripts/e2e/mobile_hosted.py <debug kelpied.exe>`: every check PASS.
- `python -X utf8 scripts/e2e/mobile_relay.py <debug kelpied.exe>`: PASS.
- `python -X utf8 scripts/e2e/mobile.py <shots> <scratch>`: PASS (quiet machine; the owner's 2–3 AM window if timing-sensitive).
- After each run: no `wrangler`/`workerd`/`kelpied` process of the run left (`tasklist` filtered by the recorded PIDs).

## Risk
| Risk | L×I | Mitigation |
|---|---|---|
| A test reaches the real daemon | L×H | `isolate()`; endpoint guard; refuse to start if `KELPIE_ENDPOINT` is the default |
| A test reaches Cloudflare | L×H | loopback-only guard in the daemon; `wrangler dev --local`; metrics off |
| Orphaned wrangler/workerd processes | M×M | PID tracking, process-tree stop in `finally`, check after run |
| Edge's virtual authenticator differs from Safari | H×M | documented; real iPhone in phase 8 |
| `wrangler dev` HTTPS flags differ by version | M×L | pinned wrangler from `relay/pnpm-lock.yaml`; fall back to a TLS front in Python |

## Rollback
Test-only files; revert.

## Success criteria
All three scripts green on Windows; the hosted checks also run on macOS where the GUI part needs no Windows-only piece (or are marked Windows-only with the reason).

## Completion note (2026-11-10)
Done on branch `test/hosted-e2e`; report: [reports/fullstack-261110-hosted-e2e.md](reports/fullstack-261110-hosted-e2e.md). `mobile_hosted.py --gui` 76/76, `mobile_relay.py` 49/49, `mobile.py` 246 passed / 0 failed, no process of any run left. Fixed on the way: room making ignored the test relay's CA (`1e86250`); the phone took a computer away from the relay for one that was reset, showing "Not paired here" on the hosted app during relay restarts (`1194a7d`, with a regression test). Where the code differed from this spec:
- wrangler's HTTPS flags exist (no TLS front); the run's copy of the configs sets `[dev]` https, since wrangler dev's proxy otherwise rewrites a page's `Origin` to `http://` and the room refuses it.
- The push stand-in is installed on every page load through CDP and the phone's Edge resolves nothing off this machine: before that, a reload let the page subscribe with Edge's real push service and a test daemon's alert test reached it (report, "Incident").
- The replace check's 410 comes from daemon A's alert test, not a needs-you event (focus/idle rules); "one verification" is one `relay_on` audit entry; the worker update is triggered with `registration.update()`.
- The GUI part starts tauri-driver itself with an isolated `WEBVIEW2_USER_DATA_FOLDER` (the debug app shares the installed app's identifier).
- Open: the expired-token state (unstable in e2e, not a check), fencing real push hosts in the daemon's e2e seam, the guide opening a moment before the relay connection is up.