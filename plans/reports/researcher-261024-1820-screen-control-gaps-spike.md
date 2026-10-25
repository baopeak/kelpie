# Screen view/control: remaining gaps + spike plans

2026-10-24 · Complements the 4 reports of 261024-1820 (not repeated). Tags: [V] verified this session (link) · [U] unverified/background · ~16 searches/fetches, search tool returns thin model summaries so most WebView2 specifics are [U].

## TL;DR
- CDP screencast takeover in WebView2: **feasible, low risk, Windows only**. Mechanism (`CallDevToolsProtocolMethod` + `DevToolsProtocolEventReceiver`) is documented; `Page.startScreencast`/`screencastFrameAck` is plain Chromium CDP (flagged experimental). Kelpie already uses the same call path in `app/src-tauri/src/browser.rs:264-285` [V, read]. No WebView2-specific screencast doc/sample found [U].
- macOS WKWebView: **no CDP, no screencast**. Only `takeSnapshot` loop (poor) or ScreenCaptureKit window capture of Kelpie's own window. [U]
- Capture: `windows-capture` (MIT, active) on Windows, `screencapturekit` (Apache-2.0, released yesterday) on macOS. scap stale, CrabGrab archived, hwcodec unlicensed.
- Encode: no maintained cross-platform Rust H.264 HW crate fits. Use `windows` crate for MF directly + VideoToolbox bindings; openh264 only as SW fallback.
- Latency techniques from Parsec/Jump/Selkies/moonlight-web mostly confirm the existing design (UDP, HW decode, encoder-coupled congestion control, direct path). Nothing contradicts str0m/WebRTC.

## 1. WebView2 CDP screencast takeover

### Mechanism [V docs / U details]
- API: `ICoreWebView2::CallDevToolsProtocolMethod`, `GetDevToolsProtocolEventReceiver(name)` + `add_DevToolsProtocolEventReceived`. Doc is thin; says CDP is maintained by Chromium, not the WebView2 team ([MS Learn](https://learn.microsoft.com/en-us/microsoft-edge/webview2/how-to/chromium-devtools-protocol)) [V].
- `Page.screencastFrame` params: base64 `data`, `metadata`, `sessionId`; must call `Page.screencastFrameAck{sessionId}` per frame or stream stalls. Chrome DevTools front end acks before rendering ([search result: ScreenCaptureModel.js](https://cobalt.googlesource.com/cobalt/+/master%5E/third_party/devtools/front_end/sdk/ScreenCaptureModel.js)) [V partial]. `sessionId` is NOT a frame counter: constant per cast, increments per `startScreencast`; method is `experimental: true` ([browser-use PR 124](https://github.com/browser-use/browsercode/pull/124)) [V partial].
- Rust: `webview2_com::CallDevToolsProtocolMethodCompletedHandler` (already used) and `DevToolsProtocolEventReceivedEventHandler` + `GetDevToolsProtocolEventReceiver`, all inside `wv.with_webview(|pv| ...)` (main/UI thread; COM handlers fire on UI thread) [U]. Keep handler cheap: copy base64 string out, push to channel, ack immediately or after WS send (ack-after-send = natural backpressure).
- Input: `Input.dispatchMouseEvent` (type/x/y/button/clickCount/deltaX/deltaY for wheel), `Input.dispatchKeyEvent` (rawKeyDown/keyUp/char), `Input.insertText` (OTP/password, layout-independent, no key events), `Input.dispatchTouchEvent` / `Input.emulateTouchFromMouseEvent` [U, standard CDP]. Events from CDP Input are routed through the browser input pipeline, so `isTrusted === true` and they pass hit-testing like real input [U — Puppeteer/Playwright rely on this; verify in spike]. Caveats: no OS-level IME, no native pickers/prompts, WebAuthn/passkey UI is browser chrome not page [U].
- Coordinates: screencast frame is the viewport scaled by `maxWidth/maxHeight`; map phone tap via `metadata.deviceWidth/deviceHeight` + `pageScaleFactor`/`offsetTop`; send as CSS px. Use signed-int-safe values (agent-browser bug: a bad `maxWidth` rejects the whole call, zero frames — [takeover report](researcher-261024-1820-takeover-patterns-macos.md)).

### Numbers (all [U], to measure)
| Item | Expectation |
|---|---|
| FPS | Frames emitted only on compositor change; practical 15-30 fps, up to 60 if acks prompt. `everyNthFrame` throttles |
| JPEG q | 60-70 for UI text; 80+ for login forms. 1280x720 q70 ≈ 40-100 KB/frame → 5-15 Mbps while animating, ~0 idle |
| Latency | Capture-to-JS-callback ≈ one compositor frame (16 ms) + JPEG encode (3-10 ms, GPU/CPU) + base64+COM hop (few ms) + WS TCP. LAN glass-to-glass ≈ 60-120 ms; input RTT adds one WS hop. Fine for login, not for drag/animation |
| Bandwidth control | Reduce `maxWidth`, quality, `everyNthFrame` by WS `bufferedAmount` |

### Gotchas
1. **Hidden/occluded webview stops producing frames** [U]. Kelpie panes are child webviews; if user switches tab, compositor goes idle. Spike: does `Emulation.setFocusEmulationEnabled` / `Page.bringToFront` / keeping the webview shown keep frames flowing? Else takeover forces pane visible on desktop (acceptable: "being controlled" banner).
2. **OAuth popups**: default `NewWindowRequested` (Handled=false, NewWindow unset) opens a native popup window with "no avenue to control the popup from the app" ([docs via search](https://github.com/MicrosoftDocs/webview2-win32-reference/blob/live/1-0-1418-22/icorewebview2newwindowrequestedeventargs.md)) [V partial] → invisible to screencast. Fix: handle `NewWindowRequested`, give `NewWindow` = a Kelpie-owned child webview (keeps `window.opener` so popup OAuth completes) and attach a second screencast to it; phone UI shows tabs/popup stack. Tauri-level `on_new_window` may expose this [U]. CDP on a new window before first navigation works since Edge 125 ([WebView2Feedback 4181](https://github.com/MicrosoftEdge/WebView2Feedback/issues/4181)) [V partial]. Alt: `Target.setAutoAttach` + `CallDevToolsProtocolMethodForSession` [U].
3. Native dialogs (`alert`, file picker, HTTP auth): `Page.javascriptDialogOpening` + `handleJavaScriptDialog` for JS dialogs; file picker/auth prompts invisible → detect via events and tell phone "native prompt on desktop" [U].
4. Page vs devtools coupling: agent's own CDP usage (element screenshot) and screencast share the session; screenshot during cast OK but pause agent reads during takeover (design rule from prior report).
5. Anti-bot/captcha may detect CDP; WebView2 always has remote-debug-capable pipe, not a flag you can hide [U].

### macOS WKWebView [U]
- No CDP, no screencast API. Options ranked: (1) ScreenCaptureKit single-window capture of the Kelpie window (needs Screen Recording TCC; helper must live inside .app — see takeover report) + input by JS-synthesized events (untrusted → breaks many login forms) or `CGEventPost` to Kelpie pid (Accessibility TCC); (2) `WKWebView.takeSnapshot(with:)` loop: async, tens of ms per call, ≤10 fps realistic, no TCC, no input path; (3) skip macOS for tier 1, fall back to tier 2 desktop path.
- Honest answer: macOS browser-pane takeover = same effort as full tier 2; CDP shortcut is Windows-only.

## 2. Cross-platform Rust capture crates

| Crate | License | Latest | Per-window Win/mac | GPU texture out | Verdict |
|---|---|---|---|---|---|
| `windows-capture` 2.0.1 | MIT | 2026-08-08, 2.25M dl, pushed 2026-10-05 [V crates.io/GitHub API] | Win only; WGC window+monitor, DXGI DD [per main report] | D3D11 texture frames [U verify in docs] | **Use for Windows** (or call `windows` crate WGC directly) |
| `screencapturekit` 11.1.0 (doom-fish/screencapturekit-rs) | Apache-2.0 | 2026-10-23, 2.9M dl, 244 stars [V] | mac only; SCK window/app/display | IOSurface-backed CVPixelBuffer → VideoToolbox zero-copy [U] | **Use for macOS**. Min macOS 12.3 [U] |
| `xcap` 0.9.8 | Apache-2.0 [V LICENSE] | 2026-08-01, 2.56M dl [V] | Yes both (window+monitor) | No: CPU RGBA screenshots/recording, not zero-copy [U] | Good for window picker thumbnails, not for stream |
| `scap` 0.1.0-beta.1 (CapSoftware) | MIT [V] | crates.io 2025-08-04, repo pushed 2025-08-05, 641 stars [V] | Yes both (WGC/SCK/PipeWire) | BGRA CPU frames only (README shows `BGRAFrame`) [V] | Stale 14 months, beta, CPU copies. Skip |
| `crabgrab` 0.4.0 | Apache-2.0 | 2024-06; repo **archived** [V GitHub API] | Yes both; wgpu/Metal/IOSurface outputs (README) [V] | Yes | Best API on paper, dead. Skip; study as reference |
| `dxgi-capture-rs` 1.2.2 (prior report) | MIT? [U] | 2026-04 | monitor only | CPU | Only if DD needed and windows-capture's DD path insufficient |

Rec: **per-OS, not one cross-platform crate.** Capture is ~200 lines per OS; a unifying crate buys little and none that is maintained also yields GPU textures on both. Common interface = own `trait Capture { next_frame() -> GpuFrame }` inside `kelpie-screen`. Per-window on Win10 19045 has yellow border (prior report).

## 3. H.264 encode options

| Option | License | Windows | macOS | Verdict |
|---|---|---|---|---|
| MF H.264 MFT via `windows` crate | MIT/Apache | HW (NVENC/AMF/QSV) + inbox SW; D3D11 NV12 in; low-latency props [V per prior report] | n/a | **Pick Win.** No maintained high-level Rust wrapper found [U], write ~400 lines |
| VideoToolbox via `objc2-video-toolbox` / `core-video`/`core-foundation` bindings, or raw FFI | MIT/Apache [U] | n/a | HW, `EnableLowLatencyRateControl`, RealTime [U, unverified in this session] | **Pick mac.** Check crate maturity at spike [U] |
| `rustdesk-org/hwcodec` | **No LICENSE file, no `license` in Cargo.toml, GitHub reports none** [V]; FFmpeg-based; Windows QSV/NVENC/AMF, mac is FFmpeg *RAM* encode H.265 only per README; no MF/VT [V WebFetch] | partial | partial | **Reject**: unlicensed (= all rights reserved by default), RustDesk is AGPL, no VT |
| `ffmpeg-next` | wrapper MIT/WTFPL-ish [U]; FFmpeg LGPL if no GPL parts | via nvenc/amf/qsv/mf | via videotoolbox | Heavy Windows build + LGPL relink/compliance for MIT product. Fallback only |
| `openh264` 0.9.9 (ralfbiedert) | crate Apache/MIT [U; GitHub API says none, verify]; Cisco source BSD-2 | SW only | SW only | SW fallback. v0.5+ can compile Cisco source in or load Cisco prebuilt via `libloading`; **patent grant only for Cisco's prebuilt binary** ([README](https://raw.githubusercontent.com/ralfbiedert/openh264-rs/master/README.md)) [V] → use libloading + download at runtime if shipping |
| NVENC/AMF/QSV direct | SDK licenses/MIT | vendor-specific | n/a | Later optimization only |

### SDP / profile for phone receive
- Baseline reality: `profile-level-id=42e01f` = Constrained Baseline L3.1 is the interop default in WebRTC offers; paired with `packetization-mode=1` [V: [bloggeek](https://bloggeek.me/webrtcglossary/h-264/) weak source, [WebRTC ObjC source](https://webrtc.googlesource.com/src/+/HEAD/sdk/objc/components/video_codec/RTCH264ProfileLevelId.mm)].
- Phone is the **offerer** (recvonly) → it lists the H.264 payloads it decodes; str0m answer must pick one of those exactly (profile + mode). Encode to the intersection: prefer Constrained High/Main if offered (better text compression at same bitrate), else 42e01f. MF must be told profile (`CODECAPI_AVEncH264Profile`/media type attribute) [U].
- Level: 1080p30 needs ≥ L4.0 (`..28`); 1280x720p30 fits L3.1 (`..1f`). If frame > L3.1 limits, offer/accept higher level or stay ≤720p/1600x900@≤20fps. Chrome and Safari generally decode above-advertised level in HW [U]. Test.
- Known quirks: Safari has sent High profile while negotiating 42e01f ([WebKit bug 195124](https://bugs.webkit.org/show_bug.cgi?id=195124)) [V partial]; some Android devices lack HW H.264 or need `42001f` rewrite ([VDO.Ninja](https://docs.vdo.ninja/advanced-settings/mobile-parameters/and-androidfix)) [V partial]; old Chrome rejected non-CB profiles ([discuss-webrtc](https://groups.google.com/g/discuss-webrtc/c/EKLBlSaiYdU/m/JJhZ1mKtDwAJ)) [V partial, 2017, likely stale].
- Keep VP8 out; if H.264 unavailable on a phone, the JPEG/WS fallback from the transport report is the plan B (not VP8, no HW encode).

## 4. Product latency techniques (documented only)

| Product | Documented | Take-away for Kelpie |
|---|---|---|
| Parsec | BUD: custom UDP protocol, DTLS 1.2, custom congestion control that **talks to the encoder** to adapt settings; claims 97% NAT traversal; HW decode default ([site](https://parsec.app/technology), [blog](https://blog.parsec.app/a-networking-protocol-built-for-the-lowest-latency-interactive-game-streaming-1fd5a03a6007/), [decode](https://support.parsec.app/hc/en-us/articles/32361394388500-Software-Versus-Hardware-Accelerated-Decoding)). Web client uses an in-house host-side WebRTC (not libwebrtc). LAN ~4-8 ms pipeline at 240 fps, vendor test, "not real world" ([blog](https://parsec.app/blog/parsec-game-streaming-total-latency-at-240-frames-per-second-c0818cc0daa5)) | BWE → encoder loop (already planned). Confirms writing own sans-I/O WebRTC host side is viable (str0m ≈ same idea) |
| Jump Desktop Fluid | Proprietary, DTLS, direct vs relay badge, "Encode Time" stats overlay, 30 Mbps cap, recovers faster on low bandwidth ([support](https://support.jumpdesktop.com/hc/en-us/articles/360047204251)). Codec/capture/CC internals **not documented** [V] | Copy the stats overlay (encode ms, direct/relayed, fps, bitrate) |
| Chrome Remote Desktop | Nothing retrieved. WebRTC-based, VP8/VP9/AV1 [U] | no data |
| Selkies | GStreamer WebRTC; Linux only, MPL-2.0; NVENC "typically <50 ms" (blog, weak); separate A/V peer connections cut latency in Neko discussion ([neko #339](https://github.com/m1k1o/neko/issues/339)) | audio is out of scope; skip |
| moonlight-web-stream | GPL-3.0; WebRTC default, WS fallback; **WebCodecs `VideoDecoder` only over HTTPS, otherwise older `<video>` path with extra latency**; fixed UDP port range, `nat_1to1` public IP, optional TURN ([repo](https://github.com/MrCreativ3001/moonlight-web-stream)) [V] | Independent confirmation of secure-context finding; fixed UDP port + advertised IP = what Kelpie plans |

No source gave a measured desktop-WebRTC latency number independent of vendors. Budget real numbers from the spike.

## 5. Spike plans

### Spike A: CDP screencast takeover (Windows, 1 day)
Steps
1. Add Tauri command `browser_cast_start(pane, quality, maxW, everyNth)` in `browser.rs`: `Page.startScreencast`, register `Page.screencastFrame` via `GetDevToolsProtocolEventReceiver`, forward frame bytes on a bounded channel; ack after hand-off.
2. Temp WS route in kelpied (or Tauri event to existing phone WS) → test HTML page draws frames to `<img>/canvas`; send taps/keys back → `Input.dispatchMouseEvent/KeyEvent/insertText`.
3. Test pages: GitHub login (2FA OTP), Google OAuth popup (`window.open`), hCaptcha demo, a page with `alert()` and a file input, scroll-heavy page, video.
4. Add `NewWindowRequested` handler → child webview; attach second cast.
5. Hide the pane (switch tab) and minimize app mid-cast.
Metrics (pass/fail)
- Tap→visible change latency p50/p95 on LAN and via Tailscale: pass ≤150 ms p95 LAN, ≤250 Tailscale.
- Sustained fps during scroll at q70 720p: ≥15; idle bandwidth ≈ 0.
- Frames continue when pane hidden? (yes/no; if no, document "pane shown while controlled").
- `isTrusted` true in page for click/keys (log in `addEventListener`); login to 2 real sites succeeds incl. password manager-free typing via `insertText`.
- OAuth popup flow completes end to end.
- CPU of WebView2 renderer + Kelpie during cast; Kelpie UI jank.
- Kill criteria: popup flow impossible, or hidden-pane frames dead with no workaround.

### Spike B: kelpie-screen WebRTC on Windows (2-3 days; refines prior report spike)
Steps
1. Build gate (hour 1): `str0m` with `default-features=false, features=["wincrypto"]` and alt `rust-crypto` on this MSVC box without NASM/cmake. Fail → webrtc-rs 0.20.
2. Capture: `windows-capture` monitor (DD) and window (WGC) → D3D11 texture; log capture interval jitter.
3. Encode: MF H.264 HW MFT, D3D11 NV12 (VideoProcessor BGRA→NV12 on GPU), `AVLowLatencyMode`, CBR, no B, GOP large, on-demand IDR, live bitrate. Dump profile/level. Compare inbox SW MFT and openh264 (CPU time, ms/frame).
4. Transport: str0m, one host candidate on bound IP, fixed UDP port, RTP payload ≤1100 B, phone as offerer with recvonly H.264 transceiver; `playout-delay` ext; NACK+PLI+TWCC; BWE→encoder.
5. Phone page: `<video>`, `getStats` overlay, DataChannel `input` (unordered mouse, reliable keys) → `SendInput`.
6. Devices: iPhone (Safari 26 and 27 if available), Android Chrome; paths: LAN Wi-Fi, Tailscale direct, 4G + Tailscale (check `tailscale ping`).
7. Repeat on ≥2 GPU vendors (NVIDIA + one of AMD/Intel).
Metrics
| Metric | Method | Pass |
|---|---|---|
| Glass-to-glass | on-screen ms clock + phone camera 240 fps, 30 samples | LAN p95 ≤60 ms; Tailscale direct ≤100; 4G ≤200 |
| Input RTT | key → screen change, same method | ≤ glass-to-glass + 30 |
| Encode ms/frame (MF HW) | QPC around ProcessInput→output | ≤10 ms 1080p |
| Capture→encode CPU/GPU % | Perfmon | <10% CPU, GPU encode engine used |
| Idle bitrate | static desktop 60 s | <100 kbps |
| IDE text quality | scrolling code @ 1600x900, 3 Mbps | legible on phone; no PLI storm on 2% loss (`clumsy` or `netem`-like) |
| Recovery | 5 s Wi-Fi drop | video back ≤3 s, no manual reconnect |
| Decoder acceptance | SDP answer accepted on iOS/Android, HW decode (`decoderImplementation` in getStats) | 100% |
| Build | `cargo build` on clean machine w/o NASM/cmake | OK |
| Stability | 30 min run, 2 resolution changes, lock/UAC event | no crash; helper reports secure-desktop state |
Go/no-go: all pass → plan phases. Fails on iOS decode or build gate → webrtc-rs 0.20 retry once; else JPEG/WS fallback + recommend RustDesk externally.

## Pick

**Recommended stack (two-tier, in this order)**

Tier 1 (ship first, Windows): CDP screencast + `Input.*` over existing phone WS (JPEG) for browser panes.
Tier 2 (desktop/IDE window, Win then mac): per-OS capture → HW H.264 → str0m WebRTC + DataChannel input, in `kelpie-screen`.
- Windows: `windows-capture` (WGC window, DXGI DD monitor, D3D11 texture) → MF H.264 HW MFT (`windows` crate) → str0m `wincrypto` → `SendInput`.
- macOS: `screencapturekit` (SCK window, IOSurface) → VideoToolbox low-latency H.264 → str0m (`apple-crypto`) → `CGEventPost`; helper inside .app.

| Criterion | Tier 2 stack | Tier 1 CDP |
|---|---|---|
| Modern (1-5) | 5 (WGC/SCK/HW codec/sans-I/O WebRTC) | 3 (JPEG over WS is old but proven) |
| Performance | 4 (est. 60/100/200 ms LAN/TS/4G [U]; str0m desktop use unproven) | 3 (15-30 fps, 60-150 ms [U]) |
| User-friendly | 4 (any window or monitor; yellow border on Win10, UAC/lock blind) | 5 (no OS permission, no TCC, tab-level, OAuth popup flow) |
| Effort (5 = low) | 2 (two capture paths, MF/VT encode, input, perms) | 4 (Windows days; mac not covered) |

**Runner-up: JPEG/WebP dirty-rect over WS (no codec, no WebRTC, works on plain http)** as the general desktop path. Lost because 10-20x bandwidth during motion, TCP head-of-line stalls on 4G, no BWE, so worse on stability and latency. Keep as the fallback if spike B fails on iOS decode or str0m build; also what Tier 1 already is, so it is nearly free to generalize. WebRTC via webrtc-rs 0.20 is the second fallback if str0m fails. RustDesk over Tailscale remains the 0-code answer for UAC/login screens.

## Limitations
No code run; WebView2 screencast specifics, isTrusted, hidden-pane behavior, fps/latency figures, VideoToolbox and `windows-capture` texture API, openh264 crate license are [U]. Chrome Remote Desktop internals and Parsec web-client latency not found. scap/crabgrab status from crates.io/GitHub API, not code reading.

## Unresolved questions
1. Does a hidden/background child webview keep emitting `screencastFrame`? Workaround cost?
2. Can Tauri 2 hand a `NewWindowRequested` popup to a Kelpie-owned webview with `window.opener` intact (OAuth), via `on_new_window` or raw webview2-com?
3. `isTrusted` and password-field behavior for CDP `Input.*` in WebView2 (any site blocking CDP-driven input)?
4. Does `windows-capture` expose raw `ID3D11Texture2D` for zero-copy to MF, or only CPU buffers?
5. MF HW encoder profile control (Constrained Baseline vs High) and `AVEncVideoIntraRefresh` on AMD/Intel?
6. `openh264` crate license text and whether to ship Cisco binary download.
7. macOS browser pane: accept "no tier 1 on mac" or build SCK window capture earlier?
8. Is a Tauri 2 child webview on mac ever worth `takeSnapshot`? (probably no)
