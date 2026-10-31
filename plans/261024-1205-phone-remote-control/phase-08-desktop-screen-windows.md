---
phase: 8
title: "Desktop / window screen control — Windows"
status: pending
priority: P2
effort: "10d"
dependencies: [3, 3a, 3c, 7]
---

# Phase 8: Desktop / window screen control — Windows

> **Updated 2026-10-30 (path D).** Screen goes over Tailscale from the installed PWA: WebRTC host candidates on the Tailscale address, no relay, no TURN. **Deferred with relay-mode screen and the native app**: `relay/src/turn.ts` and `POST /v1/turn`, TURN credentials and cost, the phase-5 WebView spike, native network and thermal hints. Text on those below stands as reference and does not gate this phase. Direct STUN `srflx` candidates are optional, tested in the spike. Needs the same "`control` for the PWA" decision as phase 7. API level: next free, expected 26.

## Overview
View and control a monitor or a single window (an IDE, the app under test, a dialog) from the phone with low latency. A helper
`kelpie-screen`, **spawned by the Kelpie app** like `kelpie-stt` (`app/src-tauri/src/voice.rs:40-61`), captures, hardware-encodes H.264 and
sends it P2P over WebRTC; input comes back on a DataChannel. Signaling: phone → `kelpie-link` → daemon → phase-7 provider channel → app → helper.
Screen control therefore needs the Kelpie app running; the phone says "open Kelpie on the computer" otherwise. [RT-13]
Go/no-go input: this phase's step 1, run against the installed PWA on iPhone and Android.

## Requirements
- Functional
  - Targets: each monitor; visible top-level windows (title, process); shortcuts for IDEs (Code, Cursor, Zed, JetBrains) and the Kelpie window.
  - Video: WGC for windows (yellow border stays on Win10 19045 — marks what is shared), DXGI Desktop Duplication for monitors; cursor composited; frames only on change.
  - Encode: Media Foundation H.264 hardware MFT (D3D11 NV12, `CODECAPI_AVLowLatencyMode`, CBR, no B-frames, long GOP + IDR on PLI, live bitrate from BWE); inbox software MFT fallback. Profile 42e01f unless the offer allows higher.
  - Transport: `str0m` (`wincrypto` or `rust-crypto`, no aws-lc/OpenSSL); phone offerer (recvonly video + DataChannel `input`); candidates: host on the Tailscale address (srflx via `stun.cloudflare.com:3478` optional; TURN deferred);
    RTP payload ≤ 1100 B (Tailscale MTU 1280); NACK, PLI, TWCC; `playout-delay` 0; DTLS fingerprints exchanged inside `kelpie-link`.
  - Input: unordered channel for moves, reliable for keys/clicks/text; `SendInput` (`KEYEVENTF_UNICODE` text, VK keys, `MOUSEEVENTF_ABSOLUTE|VIRTUALDESK`); per-monitor DPI v2; window targets via `DWMWA_EXTENDED_FRAME_BOUNDS`, brought to front on first click; optional clipboard-paste mode marked "not for passwords".
  - States reported, never a silent freeze: secure desktop (UAC/lock/login), session locked, elevated foreground (UIPI drops input), target minimized/closed.
  - Safety: `control` permission; desktop confirmation per device with user presence (phase 3), "always allow this device" also user-presence gated; always-on-top border + status-bar badge; global stop hotkey (default Ctrl+Alt+Shift+K); idle auto-stop 10 min; audit start/stop/target (never input).
  - **Grant lifetime** [RT-12]: WebRTC peer keyed `(device key, link connection, app provider client)`; revoke/downgrade/provider loss ⇒ helper `stop` and DataChannel closed in < 1 s.
  - Phone: `<video>`, trackpad (default) / direct-touch modes, pinch zoom/pan, long-press right click, two-finger scroll, shared key bar, text box, quality indicator, debug `getStats` overlay.
  - Helper lifecycle: app spawns on first viewer, stdio JSON lines, exits 30 s after the last viewer; version handshake with the app; above-normal priority while streaming; shipped as `externalBin` next to the app (`app/src-tauri/tauri.bundle.json:3`).
- Non-functional: glass-to-glass ≤ 150 ms p95 LAN, ≤ 250 ms Tailscale/direct P2P (≤ 400 ms via TURN only if relay-mode screen returns); idle ≤ 50 kbps; helper ≤ 10 % of one core at 1080p30 HW.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
**Network**
- **Spike gate additions**: ICE restart on a live str0m `Rtc` (or a full re-offer/new session ≤ 5 s as fallback); a UDP TURN client for the helper (str0m has none; e.g. the `turn` crate) while only the phone uses TURN-over-TLS 443 (Cloudflare TURN has TLS but no RFC 6062 TCP allocations; a desktop with UDP fully blocked is unsupported ⇒ Tailscale/RustDesk); Wi-Fi → 4G mid-stream; 5 % and 10 % loss (netem/clumsy); keyframe storm.
- **Restart ladder** (phone is offerer and owns it): path-change event ⇒ `restartIce()` at once; `iceConnectionState` disconnected 3 s or failed ⇒ restart; no `framesDecoded` and no DataChannel ping reply for 5 s ⇒ restart. Attempts at 0, 2, 5, 10 s (max 3 per 30 s), then tear down with "Screen lost — tap to reconnect". Signaling waits for the link re-handshake; fresh TURN credentials (1 h, allocation refreshed every 5 min) fetched over the link on every restart.
- **Helper side**: no STUN/RTCP response for 5 s ⇒ pause the encoder (capture stays warm); consent expiry at 30 s (RFC 7675) ⇒ stop sending, keep ICE state 60 s awaiting a restart, then stop the stream.
- **Freeze detection**: helper sends a DataChannel ping 1 Hz `{t, seq}`; 2 missed ⇒ "poor connection" badge, 5 missed ⇒ restart ladder; packets flowing but `framesDecoded` flat ≥ 1 s ⇒ keyframe request.
- **Keyframes**: ≤ 1 IDR per second per peer (PLI/FIR coalesced), IDR size ≤ 3× the mean frame, NACK preferred when RTT < 150 ms, long GOP kept, no FEC/RED in v1 (usable to ~5 % loss, degraded at 5–20 %). Test at 10 % loss: ≤ 30 IDR/min, ≤ 3 bitrate down-up cycles/min.
- **Degradation ladder for text-heavy screens** (fps first, keep resolution): 1080p30 → 1080p15 → 1080p8 → 720p8 → 720p4; BWE 150 kbps–4 Mbps, start 1.5 Mbps; < 300 kbps ⇒ 720p/5 fps; down-step immediate, up-step after 10 s stable; "degraded" badge when fps < 8 or scaled.
- Input TTL 1 s on the input DataChannel (keys carry seq; the helper drops anything older); input refused while the DataChannel is unhealthy.
- TURN cost note: 1.5 Mbps ≈ 675 MB/h per viewer; Cloudflare 1 000 GB/month free then $0.05/GB.

**Host overload**
- **Pipeline**: capture → 1-slot mailbox (drop-oldest) → encoder; ≤ 2 frames in flight in the MFT; `MF_E_NOTACCEPTING` or a full pipeline for > 3 frames ⇒ drop incoming frames, count `dropped_encoder`.
- **Overuse detector** (libwebrtc model): `encode_usage = EMA(encode_ms) / EMA(target frame interval)`; > 0.85 for 2 s ⇒ step down; < 0.42 for 10 s ⇒ step up one rung (needs ≥ 60 frames of data). Rung order: fps 30 → 20 → 15 → 10, then scale 100 → 75 → 50 % (windows never below 540p), then bitrate cap. Load-level caps: L1 1080p20, L2 720p15, L3 540p8 + banner; **new screen sessions refused at L2+** (`host_busy`, retry-after 10 s) and when free memory < 20 %.
- **HW ↔ SW**: stay on hardware and shed load; switch to the software MFT only if the HW MFT is absent/failed or HW encode p95 > 33 ms for 5 s at the lowest rung; SW capped at 720p15; every switch emits `encoder_changed` and a visible badge (no silent fallback). Spike enumerates `MFTEnumEx(HARDWARE|SORTANDFILTER)` per GPU vendor and logs the chosen MFT.
- **Scheduling**: capture/encode threads `AvSetMmThreadCharacteristics("Capture")` and MF work queues via `MFLockSharedWorkQueue("Capture")`; helper opted out of EcoQoS; never HIGH/REALTIME. No frame for > 500 ms while the desktop is active (3D saturation) ⇒ 1 fps keepalive repeat frame.
- **Bounds**: RTP queue ≤ 200 ms of media or 512 KiB (overflow ⇒ drop to the next IDR boundary + one IDR request); input queue ≤ 256 events; fixed texture pool 4; helper RSS alarm 300 MB ⇒ restart the stream once.
- **Stats** over the DataChannel 1 Hz: `{fps, w, h, bitrate, encode_ms_p95, dropped, reason: cpu|gpu|bandwidth|thermal|battery|other, level}` shown on the phone (`720p · 15 fps · 180 ms · reduced: computer busy`); a long L3 (> 5 min) auto-pauses with "Paused to protect your computer — tap to resume".
- Test hook `KELPIE_SCREEN_TEST_ENCODE_DELAY_MS` drives the ladder deterministically in CI (no GPU on runners); GPU-load gates run manually/self-hosted: 3D shader load + all-core stress ⇒ stream stays up, fps ≥ 10, encode p95 ≤ 33 ms at the lowest rung, glass-to-glass p95 ≤ 400 ms LAN, no silent HW→SW switch.
- Success criterion added: Wi-Fi ↔ 4G handover ⇒ stream resumes ≤ 5 s p95 without user action; no stuck black frame > 5 s.

## Architecture
```
phone ─link: screen.offer/answer/ice─▶ kelpied (verbs, grants) ─provider channel─▶ app ─stdio─▶ kelpie-screen
phone ◀════════ WebRTC: SRTP H.264 + DataChannel (UDP, P2P or TURN) ════════▶ kelpie-screen
                  capture (windows-capture 2.x) → MF H.264 → str0m → UDP;  DataChannel → SendInput
```

## Related Code Files
- Create: `crates/kelpie-screen/**` (`capture.rs`, `encode_mf.rs`, `rtc.rs`, `input_win.rs`, `targets.rs`, `states.rs`), `app/src-tauri/src/screen.rs`, `app/phone/src/screen-view.tsx`, `app/src/components/screen-indicator.tsx`
- Modify: `Cargo.toml` (member, not default-member), `app/src-tauri/tauri.bundle.json`, `app/src-tauri/src/lib.rs`, `crates/kelpied/src/takeover.rs` (screen grants), `crates/kelpied/src/mobile/verbs.rs`, `app/src/components/settings/mobile-settings.tsx` (hotkey, always-allow list), `scripts/build-release.sh`, locales, docs

## Implementation Steps
1. **Spike gate (1–2 d)**: str0m build on MSVC without NASM; DXGI → MF HW → str0m → iPhone Safari and Android Chrome as installed PWAs; LAN, Tailscale direct, 4G over Tailscale; ≥ 2 GPU vendors. Pass = targets above. Fail ⇒ webrtc-rs 0.20 once; fail again ⇒ stop the phase and document RustDesk-over-Tailscale.
2. Targets + capture + change detection.
3. MF encoder with BWE-driven bitrate/fps/scale and SW fallback.
4. App screen module (spawn, version handshake) + signaling through the provider channel.
5. Input mapping, DPI, window targets, state detection.
6. Safety: confirm, border, badge, hotkey, idle stop, audit, grant lifetime (an app auto-lock, 3c, stops the stream like a revoke).
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
