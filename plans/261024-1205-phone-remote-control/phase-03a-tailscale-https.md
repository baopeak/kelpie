---
phase: 3a
title: "Tailscale mode: detection, HTTPS origin, guided PWA install"
status: in-progress
priority: P1
effort: "4d"
dependencies: [3]
---

# Phase 3a: Tailscale mode

## State of this phase — 2026-11-01
Code, unit tests and an e2e run against a fake CLI are done on `phone/phase3a-tailscale`. The desktop half of the spike ran on the owner's real tailnet, after they enabled HTTPS. **The kill rule is not triggered: `tailscale serve` is the path.** Nothing has run on a real phone yet. `scripts/e2e/mobile.py` covers the install step, the paste fallback, a secure origin typing and this mode's Settings in the real app.

### Verified on the real tailnet (Tailscale 1.98.10, Windows 10, 2026-11-01)
An isolated debug daemon using the real CLI, a Python client on the same machine standing in for the phone, through `https://<fqdn>:8443`. Its traffic comes from this machine's own Tailscale address, so it took the tailnet path but no phone's network.

| Question | Result |
|---|---|
| **Serve from a normal-user daemon on Windows** | Allowed, no elevation. `mobile.set_tailscale {on}` took 2.16 s in all, within the 5 s limit per CLI call. |
| **Certificate** | Let's Encrypt, TLS 1.3, validated by the system's trust store. `serve --bg` returns at once; the certificate comes on the **first request, which took 38.13 s**. Later requests take about 0.1 s. Settings says so at Step 1. |
| **WebSocket through serve** | Passes: upgrade and handshake 0.70 s, pairing completed, a session ran, and the socket still carried a message after 25 s idle. |
| **`Host`** | Forwarded as `<fqdn>:8443`: `same_origin` passes with the page's `Origin`, and another site's `Origin` gets 403 through serve. |
| **`X-Forwarded-For`** | Serve **replaces** it. The client sent `100.99.99.99` and the daemon saw the machine's real Tailscale address, so a phone cannot choose its limiter bucket through serve. A program on this machine connecting to the port directly can, within a budget for the whole listener: see Resilience. The shared bucket stays as the fallback. |
| **Serve entry's JSON** | `{"TCP":{"8443":{"HTTPS":true}},"Web":{"<fqdn>:8443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:<port>"}}}}}`, exactly what `parse_serve` and its fixtures expect. Nothing served prints `{}`. |
| **Off** | `serve --https=8443 off` restored `{}` both runs, compared with what was first read. |
| **Daemon killed behind the entry** | Serve answers **502** in 0.08 s with an empty body, and the entry is unchanged. A new daemon takes it up in about a second with no serve command, and the page answers again. |
| **Entry removed by hand** | Told as `missing`, not put back; the verified call restores the same entry on the same port. |
| **Does `serve status` hang?** | **Not reproduced**: 0.04 to 0.68 s over about 25 calls. A coordinator's pipeline had not returned after 45 s just after HTTPS was enabled, cause unknown. If it does hang, each call is killed at 5 s (`a_program_past_its_time_is_killed_and_holds_nothing_up`); the person then reads "could not read what Tailscale Serve is doing", and turning on changes nothing. |

### Not yet verified
Each still fails closed or falls back as described. Only a person with real phones can settle the first group.

| Unverified | Code assumes | If wrong |
|---|---|---|
| **All of the above from a real phone, over 4G** | Same as from this machine | See the Success Criteria below. |
| **iOS storage, tab vs installed** | Separate, so install comes first | If shared, the install step is needless but harmless. |
| **Camera in standalone on iOS** | `getUserMedia` works | Falls to paste, which is always offered. |
| **`qr-decode.ts` on a real camera** | Synthetic frames only: noise, tilt, shadow, glare. Reads a 69-module code up to a tilt of about 0.12, and fails at some angles past 0.15 | Paste. Tune `binarize` and the finder tolerance. |
| **Off-tailnet tell** (`no_tailnet`: a `*.ts.net` name failing within 1.5 s) | The name does not resolve with Tailscale off; a computer that is off makes the phone wait | The label is wrong the other way; the text covers both. Does not change what the phone may do. |
| **`isSecureContext`, install prompt, icons on real phones** | As documented | See the Success Criteria below. |
| **macOS** | Same CLI at its fixed path | Refusal, or `DENIED` shown. |
| **Certificate auto-renewal** | tailscaled renews; this one runs to 2027-01-29 | HTTPS lapses. Tailscale's to fix; kill rule if it does not. |
| **`serve --bg` on a tailnet where Serve is not enabled** | Fails or blocks | A block is killed at 5 s and reads as `NOT_PUBLISHED`. Not seen: this tailnet allowed it. |
| **A MagicDNS rename** | Serve keeps entries by name, so the new name gets no answer and `off` may not find the old entry. Not tried: renaming the machine was ruled out, and no other way settles it | Such an entry is `renamed`: no origin is advertised and Restore publishes under the new name. If `off` cannot find the old one, Settings keeps saying so and it must be removed by hand. If serve does answer the new name, Restore is an extra click and nothing worse. |

### Choices the spec left open
- **Phone access off ends the mode.** `mobile.disable`, `revoke_all` and `reset` set it off and remove the entry. `mobile.enable` is open to the plain user token, and the plan's criteria say such a program cannot enable Tailscale mode; keeping the mode across a disable would let it.
- **A daemon that starts again never republishes**, so a by-hand removal holds until the person restores it. If its loopback port was taken, it removes its entry, which would lead to that program.
- **`X-Forwarded-For` counts only when it is a Tailscale address**, and the rightmost value. Serve replaces it, so this guards only against that changing.
- **`secure_origin` has a second tag** rather than a field in the first MAC, so that the record still verifies for an older daemon (the rollback note below) and setting it in the file does not.
- **The scanner is a decoder of our own**, loaded only without `BarcodeDetector` (4.0 KB gzip): jsQR is about 60 KB gzip unminified against a limit of 15.

## Overview
Gives the phone client a trusted HTTPS origin that works away from home, which Web Push (3b) and WebAuthn (3c) both need: Kelpie detects Tailscale, publishes the phone page at `https://<machine>.<tailnet>.ts.net:8443` with `tailscale serve`, and puts that address in the QR. Plain LAN stays as it is. Off by default. No Kelpie server, no account of ours.

Cites are to the phase-3 working tree, uncommitted on 2026-10-30; `mobile.rs` is still moving, so symbols are named and lines must be re-grepped.

## Data flow
```
Settings toggle → mobile.set_tailscale (Role::App + presence) → mobile/tailscale.rs → `tailscale status --json`, `serve status --json`, `serve --bg`
phone (Tailscale on) ── HTTPS :8443 ──▶ tailscaled (TLS, cert) ── http://127.0.0.1:<port> ──▶ kelpied loopback listener → page, /link (Noise inside)
QR: https://<fqdn>:8443/#p=<payload>, payload.ep = [{t:"ts", a:"<fqdn>:8443"}, {t:"lan", a:"ip:port"}...]
```

## Requirements
- Functional
  - **Detection** (`mobile/tailscale.rs`): CLI at the fixed install paths (`C:\Program Files\Tailscale\tailscale.exe`, `/Applications/Tailscale.app/Contents/MacOS/Tailscale`), then `PATH`. `status --json` parsed tolerantly (Tailscale documents the format as subject to change). States: `not_installed`, `stopped` / `needs_login`, `https_off` (no `CertDomains`), `ready {fqdn}`. Each maps to one plain instruction in Settings. Run only while the toggle is on or Settings → Mobile is open; no polling when off.
  - **HTTPS**: `tailscale serve --bg --https=8443 http://127.0.0.1:<port>`. Tailscale documents `http://127.0.0.1` as the only supported proxy target, and `listen::wanted` (`mobile/listen.rs:58`) never binds loopback in normal use, so this mode adds one loopback listener, started and stopped with the mode. Certificate issue and renewal stay with tailscaled [UNVERIFIED: the serve page does not describe renewal; the synthesis and CLI page imply it].
  - **Fixed port 8443**: the origin holds the phone's keys, passkey and push subscription, so it must not change. Port 443 is left to whatever the user serves there.
  - **Never clobber**: read `serve status --json` first. 8443 used by something else ⇒ actionable refusal. Off runs `tailscale serve --https=8443 off` only when the entry is ours. `serve reset` is never used.
  - **Transport**: the loopback listener's `Peer` is secure for `transport_cap` (`mobile.rs`, already true for loopback peers). `same_origin` (`mobile.rs`) must pass with the `Host` serve forwards [UNVERIFIED]. The pairing limiter (`pairing_blocked`, `pairing_failed`) keys on the client address from `X-Forwarded-For`, trusted on this listener only [UNVERIFIED]; if serve does not send it, one shared bucket for all serve traffic.
  - **QR**: `pair_start` (`mobile.rs`) uses the `ts` origin and adds `Endpoint {t:"ts", a}` (`kelpie-link/src/lib.rs:209`; unknown kinds are already skipped). `running_url` (`mobile.rs`) prefers the `ts` origin, so `#s=` links from every channel (`daemon.rs:405`) open away from home. `addrs` carries endpoints, not only `ip:port`.
  - **Device origin**: the device record stores whether it paired on a secure origin (`secure_origin`, serde default false). The relay (phase 4) and `control` (phases 7, 8) rely on it.
  - **Guided install** (a required step, not a hint):
    1. Desktop shows **Step 1: Install**, a QR with the bare `https://<fqdn>:8443/` and no secret.
    2. The phone page detects `display-mode: standalone` / `navigator.standalone`. Not installed: iOS gets Share → Add to Home Screen steps, Android the `beforeinstallprompt` button.
    3. Desktop **Step 2: Pair**, a QR with the secret. iOS keeps Safari-tab storage separate from the installed app [U, must test], and a camera scan opens Safari rather than the app, so the installed app scans it itself: `getUserMedia` + `BarcodeDetector`, else a small pure-JS decoder. Fallback: paste the link.
  - **Icons**: `manifest.webmanifest` lists only `icon.svg`; add PNG 192/512 and a 180 px `apple-touch-icon` (iOS does not use the SVG [U]).
  - **Phone off the tailnet**: conn-state says "Turn on Tailscale on this phone", distinct from "computer offline". The kept shell opens through `sw.ts`.
  - **Honest limit**: from an HTTPS page the browser cannot dial `ws://<LAN IP>` (mixed content) and the `*.ts.net` certificate does not match a LAN IP, so the installed app's candidates are `ts` (later relay), never raw LAN. On the same Wi-Fi with Tailscale on, Tailscale connects directly. Plain `http://` stays a separate origin with its own pairing, read-only.
  - Privacy note in Settings: the machine name appears in public certificate logs; Tailscale sees metadata, not content; Tailnet Lock suggested.
- Non-functional: no process, listener or timer when off; CLI calls `spawn_blocking`, 5 s timeout, child killed on timeout; page bundle growth ≤ 15 KB gzip for the scanner.

## Backwards compatibility
Plain LAN and manual cert/key fields (`mobile-settings.tsx`) keep working. Devices paired on `http://` stay valid and read-only there; moving to the HTTPS origin means pairing again (browser storage is per origin), and the UI says so. New record field defaults false. API 23.

## Resilience requirements
- tailscaled down or logged out while on: state shown, retried on `listen::changed` and on opening Settings, never in a tight loop. Sessions unaffected.
- Daemon crash leaves the serve entry behind: a 502 from tailscaled **while nobody holds the port**; re-checked at start; toggle-off still removes it. An isolated test daemon never issues `serve` (see Tests).
- **That is not harmless on a machine with more than one OS user:** during that window another local user can bind the port and answer at the phone's trusted origin, read the phone's key from IndexedDB and later use it. Same-user programs are out of scope (`auth.rs`). Open, and the owner's decision: see `docs/architecture.md`, "Limit: a machine with more than one OS user". A program on this machine can also name any address through the listener: no session, within a budget for the whole listener, and it can lock out an address it names for a minute. Accepted as local denial of service.
- Serve entry removed by hand: detected on the next check, shown, one-click restore.
- An entry Tailscale will not remove when the mode ends: kept in the store, shown even with phone access off, removed the next time Tailscale is looked at or phone access or the daemon starts.
- MagicDNS rename changes the origin: `addrs` tells the phone, which offers re-pairing.
- The loopback listener follows the same `Generation` stop as the others. Load levels and heartbeats unchanged.

## Related Code Files
- Create: `crates/kelpied/src/mobile/tailscale.rs`, `app/phone/src/install-guide.tsx`, `app/phone/src/qr-scan.ts`, `app/phone/public/icon-{180,192,512}.png`
- Modify: `crates/kelpied/src/mobile.rs`, `mobile/listen.rs`, `mobile/store.rs`, `crates/kelpied/src/daemon.rs` (`mobile.set_tailscale`, `mobile.tailscale_status`), `crates/kelpied/src/auth.rs` (`APP_ONLY`), `crates/kelpie-proto/src/lib.rs` (`API_LEVEL` 23), `app/src/lib/ipc.ts`, `app/src/lib/types.ts`, `app/src/components/settings/mobile-settings.tsx`, `app/phone/src/{app,pair,conn-state,conn-text}.ts*`, `app/phone/src/link.ts`, `app/phone/public/manifest.webmanifest`, `app/phone/index.html`, locales, `docs/architecture.md`, `docs/user-guide.md`, `README.md`

## Implementation Steps
1. **Spike (½ d, real tailnet, Windows + macOS + iPhone + Android)**: WebSocket through serve; `Host` and `X-Forwarded-For` as seen by the daemon; port 8443 and auto-renewal; permission needs of `serve` from a normal-user daemon; iOS tab vs installed storage; camera in standalone. **Kill rule**: serve fails on WebSocket or Windows ⇒ `tailscale cert` + the existing `tls_acceptor` (`mobile.rs`) on the Tailscale listeners only (per-listener TLS, daemon renews), recorded in this file.
2. Detection module with fixtures; `mobile.tailscale_status`.
3. Loopback listener, serve on/off, ownership check, `Peer` handling, limiter key; `mobile.set_tailscale` in `APP_ONLY`.
4. QR, `running_url`, endpoints in `addrs`, `secure_origin`.
5. Phone: install guide, scanner, off-tailnet state, icons.
6. Desktop two-step dialog. Docs. Level bump.

## Tests
- `cargo test -p kelpied`: status fixtures (ready, stopped, needs login, HTTPS off, missing fields, garbage); port-occupied refusal; off removes only our entry; `Role::User` refused on `mobile.set_tailscale`; QR holds the `ts` endpoint; `running_url` prefers it; no listener or child process when off.
- CLI seam: `KELPIE_TAILSCALE_BIN` honoured only with `KELPIE_E2E` set (same rule as `telegram_base_for`, `notify.rs:499`), so tests drive a fake script and **never touch the user's real `tailscale serve`**.
- `pnpm exec vitest run` for the install-guide state, payload parsing and scanner fallback.
- e2e `scripts/e2e/mobile.py` (isolated `KELPIE_ENDPOINT`, TLS loopback fixture): install flow, pair through the scanner fallback, secure origin may type.
- Manual, recorded in the phase report: real iPhone and Android, install → pair → sessions over 4G with Tailscale.

## Perf gate
Toggle off: no socket, no child, no timer (existing off gate).

## Success Criteria
- [ ] Tailscale on both ends: toggle on, scan, install, scan, type the code on the desktop, sessions over 4G in < 3 min on a first run.
- [ ] Page reports `isSecureContext === true` in the installed app on iOS and Android.
- [ ] Each detection state shows a specific instruction; none shows a raw CLI error.
- [ ] Toggle off leaves `serve status` as it was before the toggle.

## Risk Assessment
| Risk | L × I | Mitigation |
|---|---|---|
| iOS one-VPN limit turns Tailscale off | M × H | Distinct phone state; docs; relay later (phase 4) |
| Serve behaviour differs per platform or version | M × M | Spike kill rule; `tailscale cert` fallback |
| iOS standalone storage, camera or install flow breaks pairing | M × H | Spike first; paste fallback |
| Free Tailscale plan is non-commercial | L × M | Stated in docs; user's own account |
| Machine name in certificate logs | H × L | Stated in Settings |

**Rollback**: toggle off (removes our serve entry); reverting the commit leaves LAN untouched; `secure_origin` is ignored by older daemons.
