---
phase: 5
title: "Native mobile shell (Tauri 2 iOS/Android)"
status: pending
priority: P1
effort: "6d"
dependencies: [3, 4]
---

# Phase 5: Native mobile shell

## Overview
A store-signed Kelpie phone app that bundles the phase-2 client (trust root for relay mode, D5), runs `kelpie-link` (incl. its `client`
connect race from phase 4) natively in Rust, scans the pairing QR, keeps keys in the OS keychain and connects LAN-or-relay automatically.
After this phase the default desktop QR becomes the app form (`kelpie://pair#p=…`, phase 3).

## Requirements
- Functional
  - App `dev.kelpie.phone`, iOS 16+ / Android 10+; UI = `app/dist-phone` bundled, never loaded from the daemon or relay (store rule 2.5.2, D5).
  - In-app QR scanner (`kelpie://pair` and the browser LAN form).
  - Keys: Keychain (`AfterFirstUnlockThisDeviceOnly`) / Android Keystore-wrapped storage, no backups; the secure-store plugin is **not** exposed to WebView capabilities — only Rust uses it. [RT-2]
  - **Biometric/device-passcode gate enforced in Rust**: `link_send` refuses `control` verbs and risky approvals without a fresh (≤ 60 s) Rust-side verification result. [RT-2]
  - Multiple desktops; last-used opens directly.
  - Foreground reconnect ≤ 1 s; background socket closed after 30 s; push (phase 6) takes over.
  - Version negotiation: a 404 on `/link` or an old `v` ⇒ "update Kelpie on the computer"; newer daemon ⇒ "update the app". [RT-9]
- Non-functional: cold start ≤ 1.5 s mid Android; ≤ 15 MB; no analytics SDKs; store privacy labels "no data collected".

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
- Native network events forwarded to the client: `NWPathMonitor` (iOS), `ConnectivityManager.registerDefaultNetworkCallback` (Android); path change ⇒ kill the socket proactively and reconnect (do not wait for a socket error — half-open sockets take 60 s–3 min to fail); first state frame ≤ 3 s p95 after Wi-Fi → 4G on a good 4G profile.
- Captive portal (iOS captive assist / Android `NET_CAPABILITY_VALIDATED` absent) ⇒ `sign-in needed`, backoff paused; constrained / expensive network (`NWPath.isConstrained`/`isExpensive`, Android `getRestrictBackgroundStatus`) ⇒ visible-session-only, 500 ms min frame interval, screen streams start at the lowest rung and need an explicit tap.
- Background: no socket, no timer; foreground resume sequence = paint cache → 3 s probe → reconnect → resubscribe the visible session first.
- mDNS `_kelpie._tcp` browsing for the phase-3 hostname fallback (iOS `NSLocalNetworkUsageDescription` + `NSBonjourServices`; Android `NsdManager`).
- Phone hints to the desktop every 2 s and on change: thermal state (`ProcessInfo.thermalState`, `PowerManager.getCurrentThermalStatus`), low-power mode, battery %, decode time, dropped frames; the desktop uses `min(host caps, phone caps)`.

## Architecture
```
apps/phone/
  src-tauri/            // Rust: kelpie-link (+client), commands link_connect/link_send/link_close, events via tauri::ipc::Channel
    Cargo.toml          // workspace member, NOT a default-member
    tauri.conf.json     // frontendDist ../../../app/dist-phone
  plugins/secure-store/ // Swift Keychain + Kotlin Keystore, Rust-side API only
```
- Official plugins: `barcode-scanner`, `biometric` (called from Rust), `deep-link`, `haptics`.
- `app/phone/src/link.ts`: Tauri commands when running in the shell, wasm otherwise.

## Related Code Files
- Create: `apps/phone/**`, `scripts/build-mobile.sh`, `.github/workflows/mobile.yml` (Android on ubuntu, iOS on macos, unsigned in CI; signed release only with secrets), `docs/phone-app.md`
- Modify: `Cargo.toml`, `app/phone/src/link.ts`, `app/phone/src/*` (scanner, desktops, gate prompts), `app/src/components/settings/mobile-settings.tsx` (app QR default), locales, `README.md`, `docs/user-guide.md`

## Implementation Steps
1. `tauri android init` / `tauri ios init`; bundle the phone build; CI builds.
2. Native link commands over `kelpie-link::client`; event channel.
3. Secure store (Rust-only); scanner + deep links; desktops list.
4. Rust-side biometric gate; version screens; switch desktop QR default to the app form.
5. **Spike (gate for phase 8)**: `RTCPeerConnection` recvonly H.264 + DataChannel inside WKWebView and Android WebView against a str0m test sender; record result.
6. Store prep: privacy labels, Play data safety, 4.2.7 notes (controls the user's own computer with user-installed host software), reviewer notes + video; TestFlight + Play internal testing.

## Tests
- Rust unit tests: native link client, gate refuses `control` without fresh verification.
- Android emulator e2e in CI against an isolated test daemon (`KELPIE_ENDPOINT`) + local `workerd`; iOS simulator build + launch smoke.
- Manual per release: iPhone + Android, LAN / 4G relay / migration, biometric gate.

## Perf gate
- Foreground idle ≤ 1 wake / 30 s; ≤ 2 %/h battery foreground idle.

## Success Criteria
- [ ] Install → scan → type code on desktop → sessions, anywhere, no settings.
- [ ] Keys never reachable from the WebView; reinstall requires re-pairing.
- [ ] WebView WebRTC spike recorded (go/no-go input for phase 8).

## Risk Assessment
- Tauri mobile plugin gaps; response: small own plugin (secure-store pattern); Capacitor documented fallback shell for the same bundle.
- App Review 4.2.7 rejection; response: reviewer video + user-owned-host wording; Android + TestFlight meanwhile.
- iOS builds need macOS; `macos-latest` already in CI (`.github/workflows/ci.yml:12`).
