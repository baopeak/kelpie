---
phase: 8
title: "Desktop / window screen control — Windows"
status: pending
priority: P2
effort: "8d"
dependencies: [3, 4, 5, 7]
---

# Phase 8: Desktop / window screen control — Windows

## Overview
View and control a monitor or a single window (an IDE, the app under test, a dialog) from the phone with low latency. A helper
`kelpie-screen`, **spawned by the Kelpie app** like `kelpie-stt` (`app/src-tauri/src/voice.rs:40-61`), captures, hardware-encodes H.264 and
sends it P2P over WebRTC; input comes back on a DataChannel. Signaling: phone → `kelpie-link` → daemon → phase-7 provider channel → app → helper.
Screen control therefore needs the Kelpie app running; the phone says "open Kelpie on the computer" otherwise. [RT-13]
Go/no-go inputs: phase-5 WebView WebRTC spike and this phase's step 1.

## Requirements
- Functional
  - Targets: each monitor; visible top-level windows (title, process); shortcuts for IDEs (Code, Cursor, Zed, JetBrains) and the Kelpie window.
  - Video: WGC for windows (yellow border stays on Win10 19045 — marks what is shared), DXGI Desktop Duplication for monitors; cursor composited; frames only on change.
  - Encode: Media Foundation H.264 hardware MFT (D3D11 NV12, `CODECAPI_AVLowLatencyMode`, CBR, no B-frames, long GOP + IDR on PLI, live bitrate from BWE); inbox software MFT fallback. Profile 42e01f unless the offer allows higher.
  - Transport: `str0m` (`wincrypto` or `rust-crypto`, no aws-lc/OpenSSL); phone offerer (recvonly video + DataChannel `input`); candidates host + srflx (`stun.cloudflare.com:3478`) + TURN (relay `POST /v1/turn`, 10-min credentials, relay on only);
    RTP payload ≤ 1100 B (Tailscale MTU 1280); NACK, PLI, TWCC; `playout-delay` 0; DTLS fingerprints exchanged inside `kelpie-link`.
  - Input: unordered channel for moves, reliable for keys/clicks/text; `SendInput` (`KEYEVENTF_UNICODE` text, VK keys, `MOUSEEVENTF_ABSOLUTE|VIRTUALDESK`); per-monitor DPI v2; window targets via `DWMWA_EXTENDED_FRAME_BOUNDS`, brought to front on first click; optional clipboard-paste mode marked "not for passwords".
  - States reported, never a silent freeze: secure desktop (UAC/lock/login), session locked, elevated foreground (UIPI drops input), target minimized/closed.
  - Safety: `control` permission; desktop confirmation per device with user presence (phase 3), "always allow this device" also user-presence gated; always-on-top border + status-bar badge; global stop hotkey (default Ctrl+Alt+Shift+K); idle auto-stop 10 min; audit start/stop/target (never input).
  - **Grant lifetime** [RT-12]: WebRTC peer keyed `(device key, link connection, app provider client)`; revoke/downgrade/provider loss ⇒ helper `stop` and DataChannel closed in < 1 s.
  - Phone: `<video>`, trackpad (default) / direct-touch modes, pinch zoom/pan, long-press right click, two-finger scroll, shared key bar, text box, quality indicator, debug `getStats` overlay.
  - Helper lifecycle: app spawns on first viewer, stdio JSON lines, exits 30 s after the last viewer; version handshake with the app; above-normal priority while streaming; shipped as `externalBin` next to the app (`app/src-tauri/tauri.bundle.json:3`).
- Non-functional: glass-to-glass ≤ 150 ms p95 LAN, ≤ 250 ms Tailscale/direct P2P, ≤ 400 ms via TURN on 4G; idle ≤ 50 kbps; helper ≤ 10 % of one core at 1080p30 HW.

## Architecture
```
phone ─link: screen.offer/answer/ice─▶ kelpied (verbs, grants) ─provider channel─▶ app ─stdio─▶ kelpie-screen
phone ◀════════ WebRTC: SRTP H.264 + DataChannel (UDP, P2P or TURN) ════════▶ kelpie-screen
                  capture (windows-capture 2.x) → MF H.264 → str0m → UDP;  DataChannel → SendInput
```

## Related Code Files
- Create: `crates/kelpie-screen/**` (`capture.rs`, `encode_mf.rs`, `rtc.rs`, `input_win.rs`, `targets.rs`, `states.rs`), `app/src-tauri/src/screen.rs`, `relay/src/turn.ts`, `app/phone/src/screen-view.tsx`, `app/src/components/screen-indicator.tsx`
- Modify: `Cargo.toml` (member, not default-member), `app/src-tauri/tauri.bundle.json`, `app/src-tauri/src/lib.rs`, `crates/kelpied/src/takeover.rs` (screen grants), `crates/kelpied/src/mobile/verbs.rs`, `app/src/components/settings/mobile-settings.tsx` (hotkey, always-allow list), `scripts/build-release.sh`, locales, docs

## Implementation Steps
1. **Spike gate (1–2 d)**: str0m build on MSVC without NASM; DXGI → MF HW → str0m → iPhone Safari, Android Chrome and the phase-5 WebViews; LAN, Tailscale direct, 4G+TURN; ≥ 2 GPU vendors. Pass = targets above. Fail ⇒ webrtc-rs 0.20 once; fail again ⇒ stop the phase and document RustDesk-over-Tailscale.
2. Targets + capture + change detection.
3. MF encoder with BWE-driven bitrate/fps/scale and SW fallback.
4. App screen module (spawn, version handshake) + signaling through the provider channel; TURN endpoint.
5. Input mapping, DPI, window targets, state detection.
6. Safety: confirm, border, badge, hotkey, idle stop, audit, grant lifetime.
7. Phone screen view + gestures. Packaging; docs; e2e.

## Tests
- `cargo test -p kelpie-screen`: coordinate mapping (multi-monitor, negative origins, 100–200 % DPI), target filters, state logic, bitrate controller.
- Integration: in-process str0m receiver decodes ≥ 30 frames from the helper (software MFT on `windows-latest`).
- e2e `scripts/e2e/screen.py`: phone-emulated browser receives a test window, clicks a button in it (verified), Stop via hotkey and phone, revoke stops input < 1 s, read/write devices refused, app closed ⇒ "open Kelpie" state.

## Perf gate
- Off: helper not spawned, no UDP socket. Idle desktop ≤ 50 kbps; CPU budgets above.

## Success Criteria
- [ ] IDE window and a native test app controllable from the phone within latency targets.
- [ ] Secure desktop / elevated / minimized states shown, never a silent freeze.
- [ ] Desktop always shows sharing + stop; revoke ends control within 1 s.

## Risk Assessment
- No production str0m desktop-streaming users; response: spike stop rule.
- AMD/Intel MFT ignore low-latency props; signal: encode > 15 ms; response: per-vendor table or SW MFT at lower resolution.
- Antivirus flags the input helper; response: signed builds, clear name, docs.
