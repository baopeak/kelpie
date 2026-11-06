---
phase: 8
title: "Desktop / window screen control — Windows"
status: in-progress
priority: P2
effort: "10d"
dependencies: [3, 3a, 3c, 7]
---

# Phase 8: Desktop / window screen control — Windows

> **Owner decisions, final (2026-11-06). They replace the transport of the 2026-10-30 text below; everything else stands where it fits.**
> - **Transport: H.264 over the existing end-to-end encrypted phone link.** Binary messages on the Noise link (`kelpie-link`) over Tailscale HTTPS or LAN HTTPS; the PWA decodes with WebCodecs `VideoDecoder`. **No `str0m`, no WebRTC, no UDP socket, no STUN/TURN in this phase.** WebRTC is a later stage, added only if measurements show stalls above 250 ms p95.
> - The SPS is written with no frame reordering (VUI `bitstream_restriction` with `max_num_reorder_frames = 0`), so a decoder (Safari above all) has no reason to hold frames. H.264 only.
> - Kept as specified: WGC for windows, DXGI Desktop Duplication for monitors; Media Foundation hardware H.264 with the inbox software MFT as fallback (no openh264); `SendInput`; the helper `kelpie-screen` spawned by the app; the safety rules (`control`, border and badge, stop hotkey, idle stop, audit, stop within 1 s on revoke, lock or link close); states reported and never a silent freeze (secure desktop, locked, elevated/UIPI, minimized); UAC and the lock screen cannot be controlled, and the phone says so.
> - **Crisp text ("paint-over") ships here:** when regions stop changing, lossless (PNG) or high-quality (JPEG q90) tiles of them go on top of the video, and the phone draws them over the video until those regions change again.
> - **iOS version unknown:** the PWA probes WebCodecs H.264 (`VideoDecoder.isConfigSupported`) and says plainly when it is missing.
> - **Controls as phase 7 where they apply:** `control` for a PWA device needs `secure_origin` and an app lock that is on, granted on the desktop with OS verification (that grant is the confirmation, no extra prompt); turning the lock off drops the device to `write`. **No screen over the relay: refused.**
> - **Flow control and budgets:** the resilience section's degradation ladder, encoder overuse detector, bounded queues and keyframe limits, fitted to the new transport (below).
> - API level **29** (26 is 3e, 27 is phase 4, 28 is phase 7).

## Execution state — 2026-11-06

**Code written, not built** (branch `phone/phase8-screen`, from `phone/phase7-takeover` b71fb05). Cargo and pnpm were not run (the machine's one build slot was held); the Rust unit tests, the TS tests and `scripts/e2e/screen.py` are written and unrun. Report: `plans/reports/fullstack-developer-261106-1645-phone-phase8.md`.

- `crates/kelpie-screen`: workspace member, **not** a default member. Pure, GPU-free modules with unit tests: `geom` (coordinates, DPI, virtual desktop), `targets` (window filters, IDE shortcuts, ids), `states` (secure desktop / locked / elevated / minimized / closed, input policy, debounce), `control` (degradation ladder, overuse detector, bandwidth controller, load caps, HW→SW decision, keyframe gate, long-L3 pause), `tiles` (64×64 change detection, paint-over and unpaint), `sps` (VUI rewrite, Annex B, codec string), `wire` (helper framing and the phone's media messages), `input` (closed op set, TTL, bounded queue, keys, hotkey parser). Native Windows code in small modules under `win/` (`d3d`, `capture_dxgi`, `capture_wgc`, `convert`, `encode_mf`, `input_win`, `targets_win`, `states_win`, `border`), uncompiled.
- `windows-capture` is **not** used: its D3D11 texture hand-off is unconfirmed in the research, and the capture texture must stay on the encoder's own device. Raw WGC/DXGI through the `windows` crate (0.62, the app's version) instead.
- Daemon: `crates/kelpied/src/screen.rs` (broker: holds, grants, rebind, idle, media forwarding with an IDR-on-overflow rule, input TTL); small edits to `mobile.rs` (a media lane), `mobile/link.rs` (`Wire::send_bytes`), `mobile/lanes.rs`, `mobile/verbs.rs`, `provider.rs` (`screen` kind), `server.rs`, `daemon.rs`, `kelpie-proto` (API 29).
- App: `app/src-tauri/src/screen.rs` (spawn, version handshake, idle exit 30 s, provider requests), the status-bar indicator, the stop hotkey setting. Phone: `screen.ts` (logic, tested), `screen-view.tsx`.

## Overview
View and control a monitor or a single window (an IDE, the app under test, a dialog) from the phone with low latency. A helper
`kelpie-screen`, **spawned by the Kelpie app** like `kelpie-stt` (`app/src-tauri/src/voice.rs`), captures, hardware-encodes H.264 and
hands each frame to the app, which gives it to the daemon through the phase-7 provider channel; the daemon sends it to the phone as a binary
message on the phone's encrypted link. Input comes back the same way. Screen control therefore needs the Kelpie app running; the phone says
"open Kelpie on the computer" otherwise. [RT-13]

## Architecture
```
phone ──link (Noise): screens / sc_start / sc_input / sc_ack…──▶ kelpied screen broker (grants, holds, TTL) ──provider channel──▶ app screen.rs ──stdin JSON──▶ kelpie-screen
phone ◀── link: binary media (H.264 AU · tile · cursor) + sc_state / sc_stats / sc_fhb / sc_end ◀── kelpied ◀── provider.event ◀── app ◀── stdout frames ◀── kelpie-screen
kelpie-screen: WGC (window) | DXGI duplication (monitor) → texture pool (4) → D3D11 video processor (BGRA→NV12, scale) → MF H.264 (HW, SW fallback) → SPS VUI rewrite
               CPU readback → 64×64 tile hashes → paint-over tiles (PNG / JPEG q90);  stdin input → SendInput;  border + badge window (excluded from capture) + RegisterHotKey
```
- **Media message** (binary, first byte `0x01`, which no JSON message starts with): `kind` 1 video (flags key / unpaint-all, unpaint rects, `seq`, capture ms, source and coded size, Annex B access unit), 2 tile (format, `seq` it matches, id, rect in source pixels, PNG/JPEG bytes), 3 cursor (shape, visible, x, y in source pixels). Defined once in `kelpie-screen/src/wire.rs`; the phone parses it in `app/phone/src/screen.ts`. The local hop app → daemon carries it base64 in `provider.event` (as phase 7's frames); the network hop is binary.
- **Phone decode:** WebCodecs `VideoDecoder`, `avc` format with an `avcC` description built from the keyframe's SPS/PPS (supported by Safari's VideoToolbox path and Chrome alike), `optimizeForLatency: true`, one `decode()` per frame, newest frame drawn on `requestAnimationFrame`; tiles via `createImageBitmap`; the cursor drawn by the phone (capture has the cursor off, so a moving cursor never dirties tiles).
- **Flow control (TCP-friendly):** frames are dropped at capture, never after encode. The phone acknowledges each decoded frame (`sc_ack {seq}`); the daemon forwards `{acked, rtt_ms, inflight}` to the helper, which encodes only while fewer than 3 frames are in flight (newest capture wins in a one-slot mailbox). The daemon's media lane is bounded (6 messages, 512 KiB unacknowledged); overflow drops video until the next IDR and asks for one (≤ 1 per second).

## Requirements
- Functional
  - Targets: each monitor; visible top-level windows (title, process); shortcuts for IDEs (Code, Cursor, Zed, JetBrains, Visual Studio) and the Kelpie window.
  - Video: WGC for windows (yellow border stays on Win10 19045 — marks what is shared), DXGI Desktop Duplication for monitors; cursor drawn by the phone from cursor messages; frames only on change.
  - Encode: Media Foundation H.264 hardware MFT (async, D3D11 NV12 surfaces, `CODECAPI_AVLowLatencyMode`, CBR with a ~2-frame VBV, no B-frames, long GOP + IDR on request), live bitrate (`CODECAPI_AVEncCommonMeanBitRate`); inbox software MFT fallback with CPU NV12. Main profile (High where the MFT offers it), level left to the encoder. SPS VUI rewritten with `bitstream_restriction` (`max_num_reorder_frames = 0`, `max_dec_frame_buffering = max_num_ref_frames`).
  - Transport: binary messages on the phone's kelpie-link session (≤ 1 MiB each; tiles split to ≤ 256 KiB); JSON for state, stats, heartbeat and end.
  - Input: closed op set (`move`, `down`, `up`, `click`, `wheel`, `key` with modifiers, `text`); `SendInput` (`KEYEVENTF_UNICODE` text, VK keys, `MOUSEEVENTF_ABSOLUTE|VIRTUALDESK`); per-monitor DPI v2; window targets via `DWMWA_EXTENDED_FRAME_BOUNDS`, brought to front before the first click, and a click that would land on another window is dropped and reported (`covered`).
  - States reported, never a silent freeze: secure desktop (UAC/lock/login), session locked, elevated foreground (UIPI drops input), target minimized/closed/covered.
  - Safety: `control` permission (phase 7 rule above); always-on-top border + badge excluded from capture, and the status-bar indicator with Stop; global stop hotkey (default Ctrl+Alt+Shift+K, set in Settings → Mobile); idle auto-stop 10 min; audit start/stop/target (never input).
  - **Grant lifetime** [RT-12]: hold keyed `(device key, link connection, app provider client)`; revoke/downgrade/lock/provider loss ⇒ helper `stop` and input refused at once, the hold gone in < 1 s; a dropped link keeps the hold 15 s for the same device's next connection only.
  - Phone: canvas, trackpad (default) / direct-touch modes, pinch zoom/pan, long-press right click, two-finger scroll, shared key bar with modifiers, text box, quality line (`720p · 15 fps · reduced: computer busy`), debug overlay (decode ms, queue, rtt), plain "this phone cannot show the screen" when WebCodecs H.264 is missing.
  - Helper lifecycle: app spawns on first viewer, stdin JSON lines in and framed stdout out, exits 30 s after the last viewer; version handshake (`hello {proto}`); above-normal priority and EcoQoS opt-out while streaming, MMCSS "Capture" on capture/encode threads; shipped as `externalBin` next to the app.
- Non-functional: glass-to-glass ≤ 150 ms p95 LAN, ≤ 250 ms Tailscale direct; idle ≤ 50 kbps; helper ≤ 10 % of one core at 1080p30 HW.

## Resilience requirements — unstable networks and host overload (fitted to the link transport)
**Network** (TCP under the link: no packet loss to recover, only delay to bound)
- **Freeze detection:** the daemon sends `sc_fhb` 1 Hz; the phone dims "connection lost" and stops input after 3 s without media or heartbeat. Frames arriving but nothing decoded for 1 s ⇒ `sc_key` (keyframe request). A link reconnect ⇒ `sc_rebind` within 15 s, then a fresh IDR, never a replay.
- **Keyframes:** ≤ 1 IDR per second (requests coalesced in the helper); IDR on start, rebind, decoder error and daemon overflow only (no loss on TCP ⇒ no IDR storms). IDR size vs mean frame is measured and reported (MF has no direct cap; spike).
- **Degradation ladder** (fps first, keep resolution): rungs 30/20/15/10/8 fps at 100 %, then 8 fps 75 %, 6 fps 67 %, 4 fps 67 %, 4 fps 50 %; windows never below 540 lines. Bandwidth controller: bitrate 150 kbps–4 Mbps, start 1.5 Mbps; lag = more than 3 frames in flight or queueing delay (rtt − min rtt) over 150 ms; lag for 2 s ⇒ −25 % at once; 10 s clean ⇒ +10 %, the wait doubling (to 60 s) after an up-step that was undone within 10 s (≤ 3 down-up cycles a minute); the rung follows the bitrate (≥ 1.2 Mbps 30 fps, ≥ 800 k 15 fps, ≥ 500 k 8 fps, ≥ 300 k 8 fps 75 %, below 4 fps 67 %). "Degraded" badge when fps < 8 or scaled.
- Input TTL 1 s (stamped with the computer's clock, as phase 7); input refused while the stream is not live (no ack for 2 s).
- Relay: none. A stage-2 WebRTC path returns only if the spike measures stalls above 250 ms p95 on the link.

**Host overload** (unchanged where it fits)
- **Pipeline:** capture → 1-slot mailbox (newest wins) → encoder; ≤ 2 frames in the MFT; `MF_E_NOTACCEPTING` or a full pipeline ⇒ drop incoming, count `dropped_encoder`.
- **Overuse detector:** `encode_usage = EMA(encode_ms) / target frame interval`; > 0.85 for 2 s ⇒ one rung down; < 0.42 for 10 s ⇒ one up (≥ 60 frames since the last change). Load caps: L1 ≤ 1080p20, L2 ≤ 720p15, L3 ≤ 540p8 + banner; **new screen sessions refused at L2+** (`host_busy`) and when free memory < 20 %. L3 for more than 5 min ⇒ paused, "Paused to protect your computer — tap to resume".
- **HW ↔ SW:** stay on hardware and shed load; software only if the HW MFT is absent/failed or HW encode p95 > 33 ms for 5 s at the lowest rung; SW capped at 720p15; every switch is an `encoder` event and a visible badge.
- **Scheduling:** MMCSS "Capture" on capture/encode threads, EcoQoS opt-out; never HIGH/REALTIME. No frame for > 500 ms while frames are expected ⇒ 1 fps repeat of the last one.
- **Bounds:** daemon media lane 6 messages / 512 KiB unacknowledged; helper input queue ≤ 256 events; texture pool 4; helper RSS alarm 300 MB ⇒ restart the stream once.
- **Stats** 1 Hz `sc_stats {fps, w, h, kbps, encode_ms_p95, dropped, reason: cpu|gpu|bandwidth|load|encoder|other, level, encoder: hw|sw, idr_ratio}`.
- Test hook `KELPIE_SCREEN_TEST_ENCODE_DELAY_MS` adds encode time to drive the ladder deterministically; the pure controller is unit-tested without it.

## Spike research done from documents (2026-11-06)
- **Media Foundation low latency** ([H.264 Video Encoder](https://learn.microsoft.com/en-us/windows/win32/medfound/h-264-video-encoder)): output type before input type; `MF_MT_MPEG_SEQUENCE_HEADER` appears on the output type once set. HCK-certified HW encoders must support rate control mode, mean/max bitrate, buffer size, GOP size, QP and **force key frame**; `CODECAPI_AVLowLatencyMode`, max ref frames, slice control and CABAC are **optional**, and `AVEncMPVDefaultBPictureCount` is optional and untested. So: set every knob, read back what took, and log the MFT's name and the result (`encoder` event). The inbox software encoder defaults to slice-parallel low-latency encoding and supports CBR, live `MeanBitRate` (adaptive encoding) and force key frame from Windows 8. Settings used: CBR, `MeanBitRate`, `BufferSize` ≈ 2 frames, `GOPSize` large (IDR on request), B-frames 0, `AVLowLatencyMode` on, `MaxNumRefFrame` 1 where offered, `QualityVsSpeed` low-complexity for SW.
- **WebCodecs on Safari:** WebCodecs video is in Safari from 16.4 (iOS "partial" 16.4–18.x, full from 26 per caniuse, as the research report cites); the [AVC registration](https://w3c.github.io/webcodecs/avc_codec_registration.html) defines `annexb` (no description) and `avc` (parameter sets in `description`); implementations need not support H.264 at all. WebKit's exported H.264 decoder tests pass for the `h264_avc` case including low-latency decode; Annex B results are not shown. Decision: always use `avc` with an `avcC` description built on the phone from the keyframe's SPS/PPS, and probe with `isConfigSupported` first.
- **SPS:** without VUI `bitstream_restriction`, a decoder may assume up to `MaxDpbFrames` of reordering and hold that many frames before output; writing `max_num_reorder_frames = 0` and `max_dec_frame_buffering = max_num_ref_frames` lets it output each frame at once (the dev.to measurement in the research: 267 ms → 2.5 ms). The helper rewrites every SPS it emits (bit-exact parse up to the VUI, emulation-prevention aware) rather than relying on each vendor's MFT.

## Spike checks still owed (need a built helper, a GPU and real phones)
1. MF HW encode per GPU vendor (≥ 2): chosen MFT logged, `AVLowLatencyMode` honoured, first-output delay, encode ms p95 at 1080p30, live bitrate change response, IDR size ratio, frames held inside the MFT.
2. The SPS rewrite accepted by Safari (installed PWA, iOS version noted) and Chrome Android; decode latency with and without it.
3. `isConfigSupported` results on the iPhones at hand; the fallback message on one without WebCodecs.
4. Glass-to-glass p95 on LAN and Tailscale direct (and over DERP, reported); idle kbps; helper CPU at 1080p30 HW and SW.
5. Paint-over on an IDE: tile bytes per screen (PNG vs JPEG q90), time to crisp, no visible seam or stale tile after a change.
6. WGC on Win10 19045: frames for a covered window, `IsCursorCaptureEnabled(false)`, the yellow border; DXGI `ACCESS_LOST` on UAC and its recovery; the border/badge window kept out of capture (`WDA_EXCLUDEFROMCAPTURE`).
7. `SendInput` into a normal and an elevated window (UIPI state shown), foreground handling for window targets, 100–200 % DPI on two monitors with a negative origin.
8. Link behaviour: `bufferedAmount`/lane occupancy under 3–10 % loss (clumsy), Wi-Fi ↔ 4G handover ≤ 5 s back, stalls > 250 ms p95 (the WebRTC trigger).
9. Defender/SmartScreen on the unsigned helper.

## Related Code Files
- Create: `crates/kelpie-screen/**`, `crates/kelpied/src/screen.rs`, `app/src-tauri/src/screen.rs`, `app/phone/src/screen.ts`, `app/phone/src/screen.test.ts`, `app/phone/src/screen-view.tsx`, `app/src/components/screen-indicator.tsx`, `app/src/lib/screen.ts`, `app/src/locales/vi/screen.json`, `scripts/e2e/screen.py`
- Modify: `Cargo.toml` (member, not default-member), `app/src-tauri/tauri.bundle.json`, `app/src-tauri/src/lib.rs`, `app/src-tauri/src/daemon_client.rs`, `crates/kelpied/src/{main,daemon,provider,server,mobile}.rs`, `crates/kelpied/src/mobile/{verbs,link,lanes}.rs`, `crates/kelpie-proto/src/lib.rs`, `app/src/components/settings/mobile-settings.tsx`, `app/src/components/status-bar.tsx`, phone `link.ts`/`phone-state.ts`/`app.tsx`/`session-view.tsx`/`phone.css`, `scripts/build-release.sh`, docs

## Implementation Steps
1. Spike: the owed checks above. Pass = targets above. If the link transport measures stalls > 250 ms p95 on real networks, stage 2 (WebRTC, `str0m`) is planned then; if MF HW fails on a vendor, SW at 720p15 for that vendor and a per-vendor note.
2. Targets + capture + change detection. ✔ written
3. MF encoder with ladder-driven bitrate/fps/scale and SW fallback. ✔ written
4. App screen module (spawn, version handshake) + provider channel. ✔ written
5. Input mapping, DPI, window targets, state detection. ✔ written
6. Safety: border, badge, hotkey, idle stop, audit, grant lifetime (an app auto-lock stops the stream like a revoke). ✔ written
7. Phone screen view + gestures; packaging; docs; e2e. ✔ written, unrun

## Tests
- `cargo test -p kelpie-screen` (no GPU): coordinate mapping (multi-monitor, negative origins, 100–200 % DPI), target filters, state logic, ladder/overuse/bandwidth controller, keyframe gate, tile change detection and paint-over, SPS rewrite, media framing, input checks and hotkey parsing.
- `cargo test -p kelpied screen`: hold start/stop, revoke ⇒ input refused at once and the hold gone < 1 s, idle stop, rebind window, provider gone, IDR-on-overflow, L2 refusal, audit without input.
- `pnpm exec vitest run phone/src/screen.test.ts`: media parsing, `avcC` building, codec string, gestures (trackpad and direct), coordinate mapping with zoom, tile overlay life, freeze and keyframe request, unsupported decoder message.
- Integration (owed, needs a built helper): the software MFT encodes ≥ 30 frames that `openh264`-free decode in the test (MF H.264 decoder) round-trips.
- e2e `scripts/e2e/screen.py`: phone-emulated client receives a test window, clicks a button in it (verified by the window), Stop via hotkey and via phone, revoke stops input < 1 s, read/write devices refused, app closed ⇒ "open Kelpie" refusal.

## Perf gate
- Off: helper not spawned, no extra socket. Idle desktop ≤ 50 kbps; CPU budgets above.

## Success Criteria
- [ ] IDE window and a native test app controllable from the phone within latency targets.
- [ ] Secure desktop / elevated / minimized states shown, never a silent freeze.
- [ ] Desktop always shows sharing + stop; revoke ends control within 1 s.
- [ ] Text in an IDE is crisp within a second of it standing still.

## Open product questions (for the owner; not decided here)
See the report. In short: one screen viewer at a time or several; an extra per-session desktop prompt for the whole desktop (more than a pane); the optional clipboard-paste mode; whether Win+L / Ctrl+Esc style system keys are allowed; paint-over on by default on metered links; whether the hotkey may be turned off.

## Risk Assessment
- AMD/Intel MFT ignore low-latency props; signal: encode > 15 ms or frames held; response: per-vendor table or SW MFT at lower resolution.
- TCP head-of-line blocking on lossy cellular; signal: stalls > 250 ms p95; response: stage-2 WebRTC.
- Safari WebCodecs gaps on older iOS; response: the probe and a plain message.
- Antivirus flags the input helper; response: signed builds, clear name, docs.

## History
The 2026-10-30 version of this phase sent video P2P over WebRTC (`str0m`, host candidates on the Tailscale address, DataChannel input, ICE restart ladder, TURN deferred). The owner replaced that transport on 2026-11-06 after the [build-stack](../reports/researcher-261105-2210-remote-screen-build-stack.md) and [existing-solutions](../reports/researcher-261105-2210-remote-screen-existing-solutions.md) research; WebRTC is stage 2, gated on measurements.
