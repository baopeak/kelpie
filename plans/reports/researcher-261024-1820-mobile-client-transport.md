# Phone client + transport: findings (2026-10-24)

Scope: phone browser + transport for Kelpie remote view/control. Complements `research-261024-1820-remote-desktop-view-control.md`.
Tags: **[V]** verified this session (source linked) · **[KT]** background knowledge, unverified · **[?]** open.
Budget used: ~12 searches/fetches. Search tool snippets are model-summarised; only [V] items below were checked against primary data (spec text / MDN BCD JSON / Tailscale docs).

## TL;DR
- WebRTC recv-only on plain `http://` LAN/Tailscale origin: **works** [V spec]. Draft's "no HTTPS needed" stands. Do NOT use WebCodecs/WebTransport as primary (both secure-context only).
- Biggest un-drafted risks: (a) **mDNS-obfuscated phone ICE candidates** vs str0m, (b) **WireGuard MTU 1280 vs RTP packet size**, (c) Tailscale 4G may fall to DERP -> TCP/HTTPS relay -> latency + HOL, (d) Safari lacks the main latency knobs (`jitterBufferTarget` only from Safari 27).
- Recommendation unchanged: WebRTC/H.264/DataChannel via str0m. Add: constrain RTP payload ~1100 B, h264 constrained-baseline, `contentHint="detail"`-equivalent via encoder tuning, `tailscale ping`-based path indicator in UI.

## 1. Secure context

| Item | Finding | Evidence |
|---|---|---|
| `RTCPeerConnection` | **Not** `[SecureContext]`. grep of W3C webrtc-pc spec (851 KB) finds zero `SecureContext` annotations; `interface RTCPeerConnection : EventTarget` unannotated. Only `getUserMedia`/`enumerateDevices` (mediacapture-main) are gated. [V] | https://w3c.github.io/webrtc-pc/ (fetched + grep'd) |
| iOS Safari / Android Chrome on `http://192.168.x.x` or `http://100.x.x.x` receive-only | Should work: no gUM, no gated API. Actual device test not done [KT -> must spike on real iPhone + Android] | - |
| Note | An MDN-fetch helper claimed "WebRTC needs HTTPS" - that is **wrong** (conflates gUM). Ignore. | spec |
| `VideoDecoder` | **Secure context only** [V] | https://developer.mozilla.org/en-US/docs/Web/API/VideoDecoder |
| Safari VideoDecoder | `version_added: 16.4` desktop (iOS "mirror" = same engine, 16.4) [V, MDN BCD]. H.264 decode in Safari: yes in practice [KT]; always gate with `VideoDecoder.isConfigSupported()` | https://github.com/mdn/browser-compat-data (api/VideoDecoder.json) |
| `WebTransport` | BCD: Safari `26.4` [V BCD]; secondary press (9to5mac, iphonesoft) + W3C meeting record quote WebKit "Safari 26.4 adds support for WebTransport" [V secondary]. Primary WebKit 26.4 post not retrieved (blog fetch 404/not on page). Also secure-context-only + needs HTTP/3 server (QUIC TLS cert) -> useless on Kelpie's optional-HTTPS LAN. Draft's "chưa xác minh" -> now **confirmed enough**, irrelevant anyway. | BCD api/WebTransport.json |
| Side effects of http origin | No service worker, no PWA install-with-offline, no Clipboard async API, no WebCodecs, no `getUserMedia`. Standalone display via Add-to-Home-Screen manifest on iOS over http: [KT] works but verify. | - |
| `tailscale cert` HTTPS | Only path to secure context on LAN/Tailscale (MagicDNS name + LE cert). Optional upgrade for: WebCodecs fallback, clipboard API, SW. [KT] | - |

### mDNS ICE gotcha [KT, high impact, verify in spike]
Chrome/Safari replace the phone's host candidate IP with `<uuid>.local` unless gUM permission is granted. Server (str0m) cannot resolve mDNS from Windows reliably -> remote candidate ignored. Mitigation: ICE with server as full agent receives phone's STUN binding request from the real source addr -> **peer-reflexive candidate** learned automatically; str0m supports this [KT, confirm]. Test: don't add remote candidates at all on server; rely on prflx. Server side: only 1 host candidate (bound IP:port) sent in answer, with trickle disabled -> simplest signaling (single offer/answer).

### MTU gotcha [KT]
Tailscale WireGuard MTU = 1280. Typical RTP/SRTP packets ~1200 B + UDP/IP + WG overhead (~60-80 B) => fragmentation/drop on Tailscale path. Set max RTP payload (H.264 FU-A) ~1000-1100 B in str0m packetizer. Cheap, high value. Verify with 4G + Tailscale in spike.

## 2. WebRTC latency tuning (desktop/text content)

| Knob | Chrome Android | Safari iOS | Verdict |
|---|---|---|---|
| `playout-delay` RTP hdr ext (min=max=0) | libwebrtc: supported, honoured [KT; spec: https://webrtc.googlesource.com/src/+/main/docs/native-code/rtp-hdrext/playout-delay/README.md, "experimental, not standardized"] | WebKit tests show it **negotiated in SDP offer** (`a=extmap:12 http://www.webrtc.org/experiments/rtp-hdrext/playout-delay`) [V, WebKit LayoutTests]; whether Safari *honours* it [?] -> measure `jitterBufferDelay/jitterBufferEmittedCount` | Send it (str0m ext); harmless if ignored |
| `RTCRtpReceiver.jitterBufferTarget` | yes (Chrome 114+) [KT] | **27+ only** [V BCD]; not on <=26 | Feature-detect; set 0-20 ms where available. Safari 27 shipped ~Sep 2026 [KT]; many iPhones not yet updated |
| `playoutDelayHint` | legacy Chrome alias | no | skip |
| `contentHint = "detail"/"text"` | applies to **sender-side track** (screen share/gUM). Safari >=12.1 BCD [V]. Kelpie sender is str0m (no MediaStreamTrack) -> contentHint doesn't exist; replicate manually: prefer resolution/QP over fps | n/a | encode-side policy: keep fps flexible, hold sharpness (maintain-resolution analog). IDE text: prefer high QP quality, drop fps to 15-20 under BWE squeeze (matches draft) |
| `degradationPreference` | sender param | n/a | same: server-side policy (see above) |
| B-frames | disable; WebRTC H.264 profiles are CB/baseline anyway (no B). Use `profile-level-id=42e01f`, packetization-mode=1 | same | mandatory |
| Keyframe on PLI vs intra-refresh | PLI/FIR works; each IDR for 1080p text is big (100-300 KB [KT]) -> spike over 4G -> loss -> PLI storm. Prefer: periodic intra-refresh (rolling) if encoder supports it (NVENC/MF `CODECAPI_AVEncVideoIntraRefresh`? [KT]); else IDR on PLI rate-limited (>=1 s) and pace with low-bitrate IDR | - | Start: IDR on PLI + rate limit; intra-refresh = v2 |
| NACK vs FEC | Safari 26.0 removed `fec` and `rtx` from `RTCRtpEncodingParameters` [V, WebKit 26.0 notes via fetch] - sender-side params, irrelevant to receive-only, but signals no ULPFEC investment. Use **NACK (+RTX)**; at 30-60 ms RTT NACK is cheap; FEC wastes bandwidth. Add FEC only if 4G loss >2% proves a problem [KT] | | NACK + PLI + TWCC |
| Bitrate for IDE text | 1080p30 text: 3-6 Mbps CBR-capped feels crisp; 720p ok at 1.5-3 [KT]. Content static -> encoder idles. Scale to phone-ish res (draft: 1280x720/1600x900) | | cap + BWE-driven |
| Realistic glass-to-glass | LAN: 30-60 ms (capture 5 + HW encode 5-10 + net 2 + jitterbuf ~10-30 + decode/render 10-20 incl. 60 Hz vsync 0-16). Tailscale direct same city: 40-90. 4G via Tailscale direct: 80-150; via DERP: 150-300+ [all KT, from component budgets; Transitive Robotics/Open Robotics put internet total ~150-180 ms]. **Draft's 30-80 ms is LAN-optimistic; do not promise on 4G.** Measure with on-screen ms clock + phone camera. | |

Source quality: playout-delay README is Google primary [V]; the rest is [KT] — no primary-source benchmark for desktop WebRTC latency found in budget. Selkies / Moonlight-web latency docs not retrieved [?].

## 3. Tailscale on 4G

- Direct = UDP peer-to-peer WireGuard; DERP = relay fallback "limited QoS", slower, often far region (docs example `via DERP(tor)` 152 ms). Hard NAT on **both** ends blocks direct [V https://tailscale.com/kb/1257/connection-types].
- Mobile carriers: CGNAT (often endpoint-dependent mapping = hard NAT) is common. Desktop on home router with UPnP/NAT-PMP is usually "easy" -> hole punching succeeds if only one side is hard [KT]; both hard (e.g. desktop also behind CGNAT/office) -> DERP. No measured 4G success rate found [?].
- Every connection starts via DERP then upgrades; first seconds may be relayed; `tailscale ping` shows `via ip:port` (direct) / `via DERP(x)` / `via peer-relay(...)` [V].
- Best practice for direct path [V]: enable UPnP/NAT-PMP on router or static port-map UDP 41641; ensure UDP not blocked; **Tailscale Peer Relay** (user-run relay, faster than DERP) as fallback [V docs + sitepoint/tailscale blog titles in search results].
- WebRTC inside Tailscale = UDP inside UDP; over DERP it becomes UDP-in-HTTPS/TCP -> RTP gets head-of-line blocking, latency spikes. WebRTC does not know. Kelpie action: helper/daemon can read path via `tailscale status --json` / LocalAPI `CurAddr` vs `Relay` [KT] and show badge "direct / relayed" on phone + auto-lower bitrate/fps when relayed. Do not add own TURN (draft Q2 stands).
- iOS: Safari traffic follows Tailscale VPN route; Android Chrome ICE gathers on VPN `tun` iface - confirm candidate selection [KT].
- Verdict: Tailscale 4G is a **risk to p95 latency but not a blocker**; worst case 150-300 ms still usable for login/OTP-type tasks. Spec acceptance for draft's "<=100 ms p95 on 4G" is probably too tight; propose LAN <=60, Tailscale direct <=100, 4G <=200 [KT targets].

## 4. Mobile UX patterns

Evidence thin (single run per query); RustDesk-specific gesture docs not found. [V] items below from ubuntuhandbook release notes and Splashtop docs.

| Pattern | Evidence / practice |
|---|---|
| Two modes: touch (tap=left click, long-press=right click) and mouse/trackpad (drag moves pointer, actions at pointer) | Splashtop gesture docs [V secondary]; RustDesk 1.4.3 moved "touch mode" to local setting + added virtual mouse/joystick + scroll buttons [V https://ubuntuhandbook.org/index.php/2025/10/rustdesk-released-1-4-3-with-multi-monitor-for-wayland-virtual-mouse/]. Default: **trackpad** for full-monitor (desktop UI targets tiny at phone scale), direct-touch when zoomed. |
| Right-click | two-finger tap and/or long-press (Splashtop); also a toolbar toggle. Cheap to offer all 3. |
| Scroll | two-finger drag -> wheel events; RustDesk had to add virtual scroll buttons (two-finger conflicts with pinch zoom) [V ubuntuhandbook]. |
| Pinch zoom / pan | RustDesk 1.5.0 (09/2026) added "Lock canvas" to stop two-finger scale from panning/resizing the remote screen [V https://ubuntuhandbook.org/index.php/2026/09/rustdesk-1-5-0-is-out-with-screen-zoom-and-pan-lock-for-mobile/] -> pinch/pan conflict is a real user pain; ship lock toggle from day 1. Do zoom client-side only (CSS transform on `<video>`), disable browser page zoom (`touch-action: none`, `user-scalable=no`). |
| Special-key bar | RustDesk 1.4.5 floating key bar, 1.5.0 one-shot sticky Ctrl/Alt row [V ubuntuhandbook 1.5.0]. Copy: sticky one-shot modifiers (Ctrl/Alt/Shift/Win), Esc, Tab, arrows, Fn, Del, Ctrl+Alt+Del disabled (SAS = SYSTEM only). |
| Text/password send-box | Not in cited products as far as verified [?]; keep draft design (single UNICODE string, not stored). Sending whole string avoids per-key IME/autocorrect mangling; set `autocomplete=off autocorrect=off autocapitalize=none spellcheck=false`; for password use `type=password` with Show toggle. |
| iOS Safari soft keyboard | [KT, no source found]: keyboard only opens on `focus()` called *synchronously inside a user tap handler*, on a real `<input>/<textarea>` (not `contenteditable` hidden by `display:none`; use opacity ~0, 1px, `position:fixed`). Typing then arrives via `beforeinput`/`input` events (not reliable `keydown` keycodes on IME/Gboard: `key="Unidentified"`, keyCode 229) -> use `beforeinput.data` / `inputType` and diff-based text, map Backspace via `deleteContentBackward`. Visual viewport shrinks -> use `visualViewport` API to resize video area instead of page scroll. Test-matrix needed. |
| Fullscreen / PWA | iPhone Safari lacks Fullscreen API for non-`<video>` elements [KT]; Add-to-Home-Screen standalone is the practical full-screen route (`display: standalone`/`fullscreen` manifest, `apple-mobile-web-app-capable`). No Pointer Lock on iOS -> relative mouse must be done by deltas from touch. Android Chrome: Fullscreen API works from gesture; orientation lock needs fullscreen. |
| Chrome Remote Desktop / Jump Desktop / Microsoft Windows App | Not retrieved this session [?]; general pattern in all: trackpad default on mobile, toolbar with keyboard/ctrl keys, pinch-zoom, two-finger scroll [KT]. |
| Haptic / feedback | `navigator.vibrate` not on iOS Safari; skip. |

## 5. Fallbacks

| Option | Justified when | Cost |
|---|---|---|
| **WebRTC (primary)** | always | see draft |
| WebCodecs `VideoDecoder` over WebSocket (H.264 AUs) | WebRTC blocked: UDP filtered (corp Wi-Fi/DERP-only), str0m/ICE trouble on device. Needs **HTTPS** [V] -> forces `tailscale cert`. TCP HOL + you build BWE (watch `ws.bufferedAmount`, drop to next IDR). Same encoder, same capture; only packetizer/transport differ -> ~1-2 days extra, **moderate DRY** | Medium. Build only if spike shows WebRTC unusable on some network |
| JPEG/WebP (or PNG tiles) over WS, dirty-rect | Works on plain http, no decoder deps, trivial; text stays pixel-sharp (no chroma-subsampling blur - H.264 4:2:0 blurs coloured text edges [KT]); good for **mostly static IDE** and as "slow safe mode". Bad for scrolling/video; bandwidth 10-20x higher during motion; decode via `createImageBitmap` in worker. | Low code, no new deps. Reasonable *first fallback* because it runs on http |
| WebTransport | no: Safari 26.4+ only [V], needs HTTP/3+TLS cert, no BWE | skip |
| Terminal text streaming (existing) | Already covers PTY/agent view; pixel path only for GUI | exists |

Ranked: 1) WebRTC/H.264 2) JPEG/WebP dirty-rect-tiles over WS (http-safe) 3) WebCodecs over WS (only if HTTPS configured and tiles too heavy). Do 2/3 only after spike evidence; YAGNI otherwise.

## Concrete changes to the draft
1. Sec. 8: replace "30-80 ms" blanket with LAN/Tailscale/4G tiered targets.
2. Sec. 4.3 row WebTransport: drop "chưa xác minh" -> BCD says 26.4; still not needed.
3. Add to spike checklist: mDNS/prflx test, MTU 1100 payload, Safari `getStats` jitterBufferDelay with/without playout-delay ext, Safari 26 vs 27 `jitterBufferTarget`, `tailscale ping` path while on 4G, soft-keyboard `beforeinput` path on iOS.
4. Add "direct/relayed" badge + auto-degrade when relayed.
5. Add Lock-canvas + sticky-modifier bar to UX list.

## Limitations
No hands-on device test; no primary source for iOS keyboard behaviour, Moonlight-web/Selkies/Jump Desktop/Parsec web numbers; WebKit 26.4 primary post unseen; WebKit honouring of playout-delay unknown. Search-tool summaries partially model-generated - only spec/BCD/Tailscale-docs items verified directly.

## Unresolved questions
1. Does Safari iOS (26.x and 27) honour `playout-delay` ext, and does str0m expose it?
2. Does str0m accept prflx peer candidate when remote offer uses only `.local` mDNS candidates, no remote candidates added?
3. Android Chrome candidate selection through Tailscale `tun` iface: reliable?
4. 4G Tailscale direct success rate for user's carrier (Viettel/VNPT/etc.)? Test with `tailscale ping`.
5. iOS standalone PWA over plain http: installable and fullscreen? Keyboard behaviour inside standalone?
6. Is 1080p H.264 4:2:0 text sharp enough on phone at 3-6 Mbps, or is tile/JPEG mode needed as a "sharp" toggle?
7. Intra-refresh support in MF H.264 MFT across NVIDIA/AMD/Intel?

## Sources
- W3C WebRTC spec https://w3c.github.io/webrtc-pc/ (grep: no SecureContext)
- MDN VideoDecoder https://developer.mozilla.org/en-US/docs/Web/API/VideoDecoder ; MDN BCD https://github.com/mdn/browser-compat-data (VideoDecoder, WebTransport, RTCRtpReceiver, MediaStreamTrack)
- Safari 26.4 WebTransport (secondary): https://9to5mac.com/?p=1044958 , https://iphonesoft.fr/2026/03/25/safari-26-4-apple-annonce-44-nouvelles-fonctionnalites-developpeurs , https://www.w3.org/wiki/WebTransport/Meetings2023
- WebKit 26.0 notes https://webkit.org/blog/17333/webkit-features-in-safari-26-4/ (content = 26.0)
- WebKit LayoutTests playout-delay extmap: https://github.com/WebKit/WebKit (RTCPeerConnection-inspect-offer-bundlePolicy-bundle-only-expected.txt)
- playout-delay ext https://webrtc.googlesource.com/src/+/main/docs/native-code/rtp-hdrext/playout-delay/README.md ; jitterBufferTarget https://developer.mozilla.org/en-US/docs/Web/API/RTCRtpReceiver/jitterBufferTarget
- Tailscale https://tailscale.com/kb/1257/connection-types ; https://www.sitepoint.com/tailscale-peer-relays-nat-traversal-derp/
- RustDesk 1.4.3 / 1.4.5 / 1.5.0 notes via ubuntuhandbook.org ; Splashtop Android gestures https://support-splashtopbusiness.splashtop.com/hc/en-us/articles/212724403-Introduction-to-Android-gestures
