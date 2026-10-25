# Zero-setup phone control: untrusted relay + native shell

2026-10-24 · ~15 searches/fetches · [U] = unverified (from memory or inference, no source fetched). Builds on `research-261024-1832-phone-control-architecture-synthesis.md` (its tier 3 "relay Noise opt-in" becomes the default path here).

## 0. Verdict on hypothesis

"Zero setup => outbound-only relay; PWA from relay can swap JS => trust root must be store-signed app bundling UI => relay untrusted." **Mostly verified, 4 amendments.**

| Claim | Verdict | Evidence |
|---|---|---|
| Zero setup needs a rendezvous server | True. Daemon behind NAT, phone on 4G: nothing connects without a third party. | Inherent. But relay can be *signaling + push + TURN creds* only; payload relay is the fallback, not the core. |
| PWA served by relay can swap JS, breaking E2E | True, and unfixable on iOS today. | SW updates are server-forced on every load / <=24h; old SW cannot veto the new one ([W3C thread, 2026-01](https://lists.w3.org/Archives/Public/public-webapps-github/2026Jan/0256.html)). No SRI for the entry HTML. |
| Signed bundles / IWA can fix it | Refuted for this use. Chrome-only; launch = enterprise-managed ChromeOS install by admin; unmanaged/cross-platform "planned" ([Chrome docs](https://developer.chrome.com/docs/iwa/introduction)). Mozilla has an open position request, no decision seen ([#799](https://github.com/mozilla/standards-positions/issues/799)); WebKit: no entry found [U]. iPhone: nothing. |
| WAICT (transparency-logged JS) | Not shippable: Firefox Nightly prototype behind a pref, May 2026 ([Mozilla](https://hacks.mozilla.org/2026/05/trustworthy-javascript-for-the-open-web/)). Safari/Chrome nothing seen. |
| Passkeys/WebAuthn as trust fix | Refuted. Authenticates user to an origin; does not authenticate the JS that runs. Malicious swapped JS can still call `navigator.credentials` / use PRF output. Useful only as at-rest unlock, not as trust root. |
| iOS PWA can be pinned | No mechanism. Home-screen install pins nothing. |
| Native app bundling UI => relay untrusted | True, with caveats below. |

Amendments:
1. **Trust root = code-distribution channel, not "store" per se.** Self-host case: user runs the relay and serves the PWA from it => relay operator = user = fine. Hosted default relay (Kelpie-run) is the only case needing the native shell. So the PWA tier stays valid for LAN/Tailscale/self-hosted.
2. **Brutal framing:** Kelpie maintainer already ships the desktop binary, so trust in Kelpie is a given. What the shell removes is a *live, silent* swap channel (CF account/DNS takeover, coerced operator, bad deploy): attacker must instead compromise signing keys + pass store review/update. Real, but only worth the cost if you actually run a hosted relay.
3. **APNs key custody breaks "relay untrusted/self-hostable" for push:** a store app's APNs topic is bound to Kelpie's bundle id, so only Kelpie's key can push to it. Self-hosted relays must forward a content-free wake to a Kelpie-run push gateway (ntfy's upstream-server model [U]). Gateway sees: device token, timestamp, opaque id. Document it; it breaks "nothing leaves the machine" unless the user opts in to push.
4. Untrusted relay != harmless: it can drop/delay/replay-attempt (Noise rejects replay), DoS, and do traffic analysis.

## 1. Native shell: pick Tauri 2 mobile

| | Tauri 2 mobile | Capacitor | Expo/RN |
|---|---|---|---|
| Reuse SolidJS UI | 100% (same `app/src`) | 100% | 0% (rewrite) |
| Share Rust (`snow`, `kelpie-proto`) | In-process, same crate as daemon | No; JS Noise lib or WASM | No (needs uniffi) |
| Remote push (APNs/FCM) | Official notification plugin = local only (v2.0.0 page, Oct 2024) ([release](https://v2.tauri.app/release/notification/v2.0.0/)). Community [Choochmeque](https://github.com/Choochmeque/tauri-plugin-notifications) v0.5: MIT, 641 commits, 86 stars, `push-notifications` feature, FCM+APNs token via `registerForPushNotifications()`; [yanqianglu](https://github.com/yanqianglu/tauri-plugin-mobile-push) 11 commits. Thin bus-factor. Fallback: own Swift/Kotlin plugin (token registration only; AppDelegate hook in generated Xcode project is the awkward bit [U]). | Official `@capacitor/push-notifications` (APNs on iOS, FCM Android) mature, de facto standard ([forum](https://forum.ionicframework.com/t/capacitor-push-notifications/239316)); Capacitor 8 specifics not verified | Best (expo-notifications), irrelevant |
| QR | Official `barcode-scanner` plugin iOS/Android (`windowed` mode; needs Rust >=1.90) ([doc](https://v2.tauri.app/plugin/barcode-scanner/)) | Mature plugins | Mature |
| Key storage | Official `biometric` = gate only, stores nothing ([doc](https://v2.tauri.app/plugin/biometric/)). Secure Enclave/StrongBox: community `crypto-hw` 0.1.0 only. Likely write ~100-line Keychain/Keystore plugin [U]. | Mature secure-storage plugins [U] | Mature (expo-secure-store) |
| WebRTC in WebView | Same WKWebView/Android WebView as Capacitor. RTCPeerConnection works in WKWebView; getUserMedia was the gap (fixed iOS 14.3) ([WebKit 183201](https://bugs.webkit.org/show_bug.cgi?id=183201), [Apple forum](https://developer.apple.com/forums/thread/88052)). Kelpie only *receives* video + datachannel: no getUserMedia needed. No Tauri-specific report found [U] => spike. | Same | Needs react-native-webrtc |
| Build | Needs Mac+Xcode for iOS (GH macOS runners) | Same | EAS cloud build avoids Mac |
| Maturity risk | Mobile plugins vary per-plugin; weakest link = push + keystore | Lowest | Lowest |

**Pick: Tauri 2 mobile.** Decisive: Noise + protocol types compile once and run identically in daemon and phone (no second crypto implementation to audit), and UI is literally the existing SolidJS. Cost: you own ~2 small native plugins (push token, keystore). **Runner-up shell: Capacitor** (lower plugin risk, but second crypto impl in JS and no Rust sharing). Hedge: put the link layer (Noise session, framing, resume) in a plain Rust crate `kelpie-link` so the shell stays swappable (Capacitor would call it via wasm/uniffi; extra work, but contained).

Sidestep: run Noise + WebSocket in the Rust core of the shell, not in JS. Benefits: key never in JS heap; Rust sockets bypass WebView mixed-content/ATS => LAN direct `ws://` is safe under Noise, **no tailscale cert / HTTPS needed**. iOS still shows Local Network permission prompt + needs `NSLocalNetworkUsageDescription` [U].

Store risk: Apple 2.5.2 forbids downloading code that changes features ([forum case](https://developer.apple.com/forums/thread/821927)); bundled UI is compliant, which is also exactly the security property. Cost: UI updates ride app releases => UI/daemon version skew; negotiate protocol version inside Noise handshake payload and keep the phone UI thin. Fees: Apple $99/yr, Google $25 once [U].

## 2. E2E channel

| Option | Verdict |
|---|---|
| **Noise via `snow`** | Pick. `snow` tracks Noise rev 34, last release I could confirm 0.9.6 (Jan 2024) via [lib.rs](https://lib.rs/crates/snow) snapshot; **no formal audit**; a nonce-DoS fixed in 0.9.5. Newer release status [U] => check crates.io before adopting. Alternative impl if stalled: `noise-protocol` / `clatter` [U]. |
| MLS | Overkill: group semantics, big dep, for 2 parties. |
| age | One-shot file encryption, no forward secrecy, no session. |
| libsodium secretstream | Needs your own handshake = reinventing Noise. |

Pattern (design, not sourced): QR carries `{daemon static pubkey, one-time PSK, relay URL, room id}`. Pairing = **Noise_IKpsk2** (phone knows daemon key from QR, PSK proves physical QR access; daemon accepts unknown phone static only while a pending pairing PSK exists, then stores phone key in allowlist). Later sessions = plain **Noise_IK** (1 round trip, forward secrecy via ephemerals, phone identity hidden). XX unnecessary since the daemon key is known out-of-band. Revoke = delete key from allowlist; per-device `can_write`/`can_control` stays daemon-side.

WebRTC: exchange SDP/DTLS fingerprints *inside* the Noise channel => relay and TURN cannot MITM media; SRTP stays E2E. Relay-signaled WebRTC becomes untrusted-safe for free.

Key storage:
- Phone: iOS Secure Enclave is P-256 only, Noise uses X25519 => store X25519 key in Keychain (`WhenUnlockedThisDeviceOnly`), or wrap with an SE/StrongBox key [U]. Android Keystore: wrap key with hardware AES key (native X25519 support version-dependent [U]). Gate Approve with biometric plugin.
- Desktop: OS keyring (Windows Credential Manager / macOS Keychain) via `keyring` crate [U]; file fallback with DPAPI.

## 3. Relay hosting

| | CF Workers + Durable Objects | fly.io / small VPS (axum) | Self-host |
|---|---|---|---|
| Cost | Free plan allows SQLite DOs: 100k req/day, 13k GB-s/day. Paid: 1M req/mo then $0.15/M; incoming WS msgs billed **20:1**; outgoing + pings free; hibernating idle DOs **not billed for duration** ([pricing](https://developers.cloudflare.com/durable-objects/platform/pricing/)). WS msg limit 32 MiB (was 1 MiB, Oct 2025) ([changelog](https://developers.cloudflare.com/changelog/product/durable-objects/index.md)). Realistic cost for thousands of idle-paired users: low single-digit $/mo [U, my arithmetic]. | ~$3-6/mo fixed, one VM, you patch it | $0 |
| Ops | ~0; global anycast; 1 DO per room | On-call | User's problem |
| Code | Separate small TS worker (~150 LOC) | Rust axum, can share `kelpie-proto` | Same axum binary |
| Risk | CF vendor lock, ToS on heavy media proxying [U]; soft ~1k req/s per DO (3rd-party claim) | SPOF, DDoS | |

**Pick: CF Worker default + axum `kelpie-relay` for self-host**, one tiny wire protocol, shared conformance test (violates "same code" slightly; workers-rs could unify but hibernation support unverified [U]). Design: room = DO keyed by hash of daemon pubkey; both ends dial **outbound WSS**; relay forwards opaque binary frames; auth by signed challenge (Ed25519/HMAC from pairing) so strangers can't squat/spam; rate caps per room.

Relay unavoidably sees: both IPs, connect/disconnect times (= when the user is active), message sizes/timing (traffic-analysis: can tell video burst vs typing), room id, push tokens + platform, approximate user count. Mitigate: pad frames to buckets, hash room ids, rotate tokens on unpair. Cannot see: content, agent names, keys.

## 4. Screen streaming when P2P fails

- **Cloudflare Realtime TURN: $0.05/GB egress, 1,000 GB/mo free (shared with SFU), ingress free, STUN free** ([FAQ](https://developers.cloudflare.com/realtime/turn/faq/)). 1080p screen at 1.5-2 Mbps ~ 0.7-0.9 GB/hr => ~1,100 viewing hours/mo free; every user relayed through *Kelpie's* account, so Kelpie pays beyond that. Mint short-lived creds from the relay Worker; cap bitrate (e.g. 1.5 Mbps) + session timer + per-device monthly quota.
- coturn on a VPS: bandwidth often bundled; self-hosters set their own TURN URL.
- **WS-over-relay video fallback: skip.** TCP head-of-line stalls, per-message billing, duplicates the pipeline. TURN over TCP/TLS 443 [U] covers UDP-blocked networks with the same WebRTC code path.
- DTLS-SRTP keeps media E2E through TURN; TURN sees IPs + volume.

## 5. Push

- Relay sends **content-free alert push**: generic text ("Kelpie needs you") + opaque id; app opens, connects, fetches real state over Noise. Payload limit 4096 B ([overview](https://developer.apple.com/forums/thread/709671)); keep < 200 B.
- Prefer visible alert over `content-available` silent push: silent is throttled by Apple (AWS guidance ~3/hr, unconfirmed) and not delivered after force-quit ([AWS](https://aws.amazon.com/blogs/messaging-and-targeting/sending-push-notifications-to-ios-13-and-watchos-6-devices/), [forum](https://developer.apple.com/forums/thread/835645)). NSE (`mutable-content`) could decrypt a ciphertext payload, but Tauri has no easy extension target => skip.
- Android: FCM high-priority data/notification message. Needs `google-services.json` (Firebase project) ([plugin doc](https://github.com/Choochmeque/tauri-plugin-notifications)). Optional UnifiedPush/ntfy for de-Googled [U].
- Cost: APNs and FCM free. Privacy: Apple/Google see token + timing + generic text. Push is an opt-in toggle; off = phone must be foreground.
- Approve from lock screen: not via notification action (no content); open app + biometric. Matches prior rule "Deny without auth, Approve with auth".

## 6. Recommended architecture + scores (5 = best; ops cost 5 = cheapest; effort 5 = least)

**A (recommend): Tauri 2 mobile shell (bundled SolidJS, Rust core does Noise/WS) + Noise_IKpsk2/IK over CF DO relay (self-host axum option) + QR pairing + content-free APNs/FCM via Kelpie push gateway + WebRTC signaled inside Noise + CF TURN fallback.**
**B (runner-up): Keep PWA (served by kelpied) + LAN/Tailscale + Web Push + optional *self-hosted* relay that serves the PWA; no hosted relay, no store apps.**
C: A with Capacitor shell.

| | User-friendly | Security | Perf | Ops cost | Effort |
|---|---|---|---|---|---|
| A Tauri shell + untrusted relay | 4 (install app, scan QR, done; no Tailscale/HTTPS) | 5 | 4 | 3 (Kelpie runs relay+push gw+TURN; $124 dev fees) | 2 |
| B PWA + self-host relay | 3 (needs Tailscale or own relay/domain; iOS PWA push quirks) | 3 (4 on LAN/Tailscale; hosted relay would break E2E, so none) | 4 | 5 | 4 |
| C Capacitor variant of A | 4 | 4 (JS Noise, key handling in plugin) | 4 | 3 | 3 |

Sequence: (1) `kelpie-link` crate + relay + daemon outbound client, test with the *existing* PWA over self-host relay (this is B, ships value early); (2) Tauri mobile shell spike: WebRTC recv in WKWebView, push plugin, keystore; (3) push gateway; (4) TURN. Go/no-go for A: iOS push + WKWebView H.264 recv in Tauri spike works in <= 3 days; otherwise fall to C.

## Limitations
- Search tool returned mostly secondary results; Capacitor 8 push, Tauri iOS 2026 stability, snow post-2024 releases, WebKit stance on IWA not confirmed.
- No hands-on test of Tauri mobile + WebRTC; cost figures are arithmetic from list prices, not load-tested.
- Did not research legal/GDPR duties of running a push gateway holding device tokens, or Cloudflare ToS for media proxying.

## Unresolved questions
1. Will Kelpie (the project) operate a public relay + push gateway + TURN account, and who pays/abuse-handles? Without it, A collapses to self-host only (no store-app push).
2. Is a Mac + Apple Developer account available for iOS builds/CI?
3. Does Tauri mobile expose the AppDelegate hook needed for `didRegisterForRemoteNotifications` without forking the generated Xcode project?
4. Latest `snow` release/maintainer activity in 2026; need crates.io check.
5. X25519 key custody: wrap-with-SE vs plain Keychain acceptable to the user?
6. Should the existing PWA remain as the "no-install" mode permanently (B), accepting its weaker hosted-relay story?
