---
phase: 9
title: "Desktop / window screen control — macOS"
status: pending
priority: P2
effort: "6d"
dependencies: [7, 8]
---

# Phase 9: Desktop / window screen control — macOS

> **Updated 2026-10-30 (path D).** Inherits phase 8's transport: Tailscale from the installed PWA; relay-mode screen, TURN and native app deferred. Same "`control` for the PWA" decision as phases 7 and 8. No new boundary expected; if one appears, take the next free API level.

## Overview
macOS backends for `kelpie-screen` behind the phase-8 signaling, transport, safety, grant lifetime and phone UI. Browser-pane takeover on macOS
is the Kelpie-window target with the pane rect as crop hint — no separate provider (one non-CDP path). [RT-13]

## Requirements
- Functional
  - Capture: ScreenCaptureKit (`screencapturekit` crate, Apache-2.0) for displays and windows; macOS 12.3+ (12.0–12.2 show "needs macOS 12.3").
  - Encode: VideoToolbox H.264, real-time + low-latency rate control, no B-frames, IDR on PLI, live bitrate.
  - Input: `CGEventPost` (pointer, keys), `CGEventKeyboardSetUnicodeString` for text; Accessibility permission.
  - Permissions onboarding: Screen Recording + Accessibility status, buttons to the right System Settings pane, note on Sequoia's monthly re-consent; missing permission reported to the phone.
  - Helper inside the bundle (`Kelpie.app/Contents/MacOS/kelpie-screen`), spawned by the app (phase 8 model), signed with the app so TCC attributes it correctly.
  - Kelpie-window target accepts a crop rect (pane bounds) for pane takeover; window brought to front for input.
  - Same safety model and grant lifetime as phase 8 (hotkey Ctrl+Option+Shift+K).
- Non-functional: phase-8 latency/CPU targets on Apple Silicon; Intel best effort.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
- Same network ladder, freeze detection, keyframe and degradation rules as phase 8.
- Load inputs for `load.rs` on macOS: `ProcessInfo.thermalState` (+ change notification), `isLowPowerModeEnabled`, `DISPATCH_SOURCE_TYPE_MEMORYPRESSURE`; helper capture/encode threads at QoS `USER_INTERACTIVE`, daemon link/sync at `USER_INITIATED`; App Nap held off (`beginActivity`) while a viewer is connected.
- SCK frame pacing via `minimumFrameInterval`; VideoToolbox `kVTCompressionPropertyKey_RealTime` + overuse detector from phase 8; same `host_busy` admission and gates.

## Architecture
- `crates/kelpie-screen/src/{capture_sck.rs,encode_vt.rs,input_mac.rs}` behind phase-8 traits; `str0m` with `apple-crypto` or `rust-crypto`.

## Related Code Files
- Create: `crates/kelpie-screen/src/capture_sck.rs`, `encode_vt.rs`, `input_mac.rs`
- Modify: `crates/kelpie-screen/Cargo.toml`, `crates/kelpie-screen/src/targets.rs` (crop hint), `app/src-tauri/tauri.bundle.json`, `app/src-tauri/src/screen.rs`, `scripts/build-release.sh`, `app/src/components/settings/mobile-settings.tsx` (permissions panel), locales, docs

## Implementation Steps
1. Spike: SCK window capture + VT encode + str0m to iPhone; measure; verify TCC listing of the bundled, app-spawned helper.
2. Capture/encode/input backends; permission detection + onboarding.
3. Kelpie-window crop for pane takeover.
4. Packaging/signing; docs.

## Tests
- `cargo test` (macOS CI): Retina/multi-display mapping, permission-state mapping, encoder config, crop math.
- Manual matrix per release (CI cannot grant TCC): Sonoma, Sequoia, macOS 26; Apple Silicon + Intel; window, display, pane takeover.

## Perf gate
- Same as phase 8; helper not spawned while off.

## Success Criteria
- [ ] Display, IDE window and Kelpie browser pane controllable from the phone on macOS.
- [ ] Missing permissions explained with one-click fixes; never a black screen without a reason.

## Risk Assessment
- Sequoia re-consent interrupts unattended use; response: clear phone state + desktop reminder; evaluate `persistent-content-capture` entitlement (Apple form).
- TCC attribution of helper differs from expectation; signal: spike step 1; response: capture in-process in the app as fallback.
