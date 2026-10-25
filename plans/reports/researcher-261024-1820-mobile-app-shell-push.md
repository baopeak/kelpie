# Mobile app shell + push + background (2026-10-24)

Scope: PWA vs Tauri 2 mobile vs Capacitor vs RN; push w/o server; background limits; store policy. ~12 searches/fetches. Search tooling returned thin 2026 data -> many claims tagged [U] (from prior knowledge, not re-verified this session). Verified = fetched source this session.

## TL;DR
- Notifications on locked phone cannot be done by a socket (iOS kills it in seconds). Only OS push can. Two ways: Web Push (daemon -> Apple/Google/Mozilla push svc, no Kelpie server) or APNs/FCM (needs Kelpie-run relay).
- Web Push fits privacy promise (payload E2E-encrypted per RFC 8291 [U]; push svc sees metadata only). Cost: iOS install friction + HTTPS mandatory + no Live Activities.
- Tauri mobile adds little here: phone UI talks to remote daemon over WS, needs no local Rust. Its push story is the weakest of the native options. If native, Capacitor > Tauri mobile for push/Live Activity ecosystem [U].
- Ship PWA + Web Push now. Native shell later only with content-free wake-up relay.

## 1. App shell comparison

| Option | Reuse SolidJS/xterm | Push | Live Activity/widgets | Build/ship cost | Risk |
|---|---|---|---|---|---|
| PWA (served by kelpied) | 100% (already there) | Web Push, no relay | none (iOS), ongoing-notif no | $0, no store, no Mac | iOS manual "Add to Home Screen"; HTTPS required |
| Tauri 2 mobile | ~100% UI, WebView | official `notification` = local only; remote push = 3rd-party `tauri-plugin-mobile-push` (v0.1, 11 stars, iOS event callbacks do NOT deliver yet - verified [GitHub](https://github.com/yanqianglu/tauri-plugin-mobile-push)) -> write own Swift/Kotlin | DIY Swift (WidgetKit/ActivityKit) | Mac+Xcode for iOS, $99/yr, $25 Play | youngest mobile stack; few public shipped apps found; WKWebView quirks (CORS/cookies/CSP) [dev.to guide, generic] |
| Capacitor | ~100% | mature `@capacitor/push-notifications` [U] | community Live Activity plugins [U] | same Apple/Google cost | extra JS-bridge dep; but most proven web->native path [U] |
| Expo/RN | ~0% for xterm/DOM comps (rewrite) | best | best | highest | throws away "shared SolidJS" goal |

Tauri 2 official plugins (docs [v2.tauri.app/plugin](https://v2.tauri.app/plugin/)): biometric, haptics, nfc confirmed iOS+Android; notification, barcode-scanner, deep-link, stronghold, websocket exist, per-platform table not extractable -> [U] mobile support per plugin. Tauri 2.0 stable since Oct 2024 ([blog](https://v2.tauri.app/blog/tauri-20/)); iOS panic bugs in 2.2.x/wry fixed ([release notes](https://v2.tauri.app/release/tauri/v2.2.5/)). No first-hand shipped-app postmortem found -> adoption risk [U].
WebRTC/WS in WKWebView + Android WebView: both work in-app [U]; WKWebView same engine as iOS PWA, no gain.
Honest read: Tauri mobile's pitch (Rust core + shared commands) is unused; Kelpie phone = thin remote client. Keychain/biometric/QR scanner are the only native wins and Capacitor does those too.

## 2. Push without a server

### Web Push (PWA)
- iOS/iPadOS 16.4+, only for Home-Screen-installed web app, permission on user gesture, VAPID, `userVisibleOnly`, every push must show a notification (silent pushes get revoked) ([WebKit](https://webkit.org/?p=13878); Apple doc page extraction failed -> details [U]).
- Safari 18.4 declarative web push: system renders notification from JSON without waking SW -> better reliability [U, from Apple doc summary unverified].
- iOS 26: any site added to Home Screen now opens as web app by default ([heise](https://heise.de/-10749652)) -> eases install but still manual Share > Add.
- EU: Apple reversed the 17.4 removal; PWAs work in EU ([Register](https://www.theregister.com/2024/02/08/apple_web_apps_eu/), [Notificare](https://notificare.com/blog/2024/02/16/apple-giveth-apple-taketh-away/)). One vendor blog claims otherwise for 2026 (MagicBell) - low credibility, test on EU-SIM device [U].
- Daemon: VAPID keypair self-generated, POST to subscription endpoint (web.push.apple.com / fcm.googleapis.com / Mozilla autopush). Needs host internet + Rust crate (`web-push`) [U]. Phone must be able to reach push svc (any internet). Host offline -> no push (inherent).
- HARD CONSTRAINT: service worker + Push need secure context. Kelpie "HTTPS optional" -> push requires HTTPS. LAN HTTP w/ self-signed = no push. Tailscale HTTPS certs (`tailscale cert`/serve) give browser-trusted *.ts.net certs [U] -> that is the clean path; LAN-only users get no push unless they accept a CA/cert hassle.
- Reliability: iOS Web Push generally OK when installed; failures: user removes/reinstalls PWA (subscription lost, storage not shared with Safari tab), Focus modes [U]. Android Chrome: reliable, install prompt exists, notification action buttons supported; iOS Safari: no action buttons [U].
- Pairing gotcha: iOS installed PWA storage is separate from Safari -> scan QR/open link in Safari, then Add to Home Screen = token lost. Pair inside installed PWA (in-app QR scan via getUserMedia + JS decoder; BarcodeDetector missing on Safari [U]) or 6-digit code.

### ntfy / UnifiedPush (alt for Android; precedent for iOS)
- ntfy self-hosted iOS instant delivery: server sends `poll_request` (message id + hashed topic, fixed text "New message") via ntfy.sh -> Firebase -> APNs; device then fetches actual content from your own server. Content never touches upstream ([ntfy docs](https://docs.ntfy.sh/config/#ios-instant-notifications), verified). This is the template for a minimal Kelpie relay.
- UnifiedPush (Android, no Google): works w/ ntfy distributor; niche audience [U]. Don't build for it v1.

### Native relay design (if APNs/FCM)
- App holds device token; sends it to the user's daemon at pairing (not to relay).
- Daemon -> relay: `{device_token, opaque_id}` only. Relay (Cloudflare Worker-class, stateless, holds APNs .p8 + FCM key) sends content-free push (`mutable-content`, generic "Kelpie needs you"). App's Notification Service Extension / FCM data handler wakes, fetches real text from daemon over LAN/Tailscale, or daemon includes payload encrypted with a key shared at pairing (decrypted in NSE). Relay sees: token, timing, IP of daemon. No content.
- Auth relay: daemon signs request w/ per-pair key registered at first pairing; rate-limit; no accounts.
- Cost: Apple $99/yr, Play $25 once, relay ~$0-5/mo on free-tier serverless [U]. Real cost = Kelpie now operates a service (uptime, abuse, key custody for ALL users, APNs key leak = spam all users) and break "nothing leaves the machine" unless opt-in + disclosed. Apple 4.5.4: push must not be required and not carry sensitive info ([guidelines](https://developer.apple.com/app-store/review/guidelines/)) -> content-free design is also compliance-aligned.
- tauri-plugin-mobile-push would need FCM `google-services.json` + Xcode capability; iOS callbacks broken -> own Swift.

## 3. Background, Live Activities, actions

- iOS: app suspended soon after backgrounding; sockets killed; no general background socket (VoIP/PushKit is restricted to calls, abuse = rejection) [U]. Only push wakes. => "keep session socket alive" is unrealistic; on foreground, reconnect + replay (VT state snapshot) is the design.
- Android: FGS can hold a socket but needs persistent notification, can't start from background (Android 12+ exemptions: user visible activity/notification interaction/etc., [Android docs](https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start) verified). `dataSync` ~6h/24h cap on Android 15, `specialUse` needs Play declaration [U]. Battery-optimization exemption allows start. Prefer FCM/Web Push wake over FGS.
- Live Activities (iOS 16.2+): update via per-activity APNs push token; push-to-start iOS 17.2+; locally can only start in foreground; ~8h life; ~4KB payload; update budget throttled (ActivityKit summary, doc extraction failed -> [U]). So "3 working, 1 needs you" on lock screen/Dynamic Island = native app + APNs relay, NOT possible via PWA. Android analog: ongoing notification updated through FCM data message or FGS.
- Widgets: iOS WidgetKit (native extension, needs Swift; refresh is OS-budgeted, not live) [U]; Android widgets via native. Low value vs cost; skip v1.
- Actionable notifications (approve/deny from lock screen):
  - Native: UNNotificationAction; mark Approve `.authenticationRequired` (Face ID/unlock) [U]. Web Push: Android Chrome only; iOS none.
  - Risk: remote approval of agent tool-use = remote code exec primitive. Mitigate: action carries single-use nonce + request id bound to daemon-side pending prompt, TTL ~2 min, daemon verifies device key signature; Deny allowed unauthenticated, Approve requires auth; never approve destructive/high-risk class from lock screen (open app + biometric); payload contains no command text/secrets (lock-screen readable). Reuse existing `can_write` permission; separate `can_approve`.

## 4. Store policy
- Apple 4.2 (not a repackaged website): thin WebView of a URL is the rejection pattern. Shell with bundled UI + native pairing/biometric/push passes [U on practice]. 4.2.7 Remote Desktop: for apps mirroring specific software; host must be user-owned PC AND "connected on a local and LAN-based network"; software runs on host; no store-like UI; account creation from host ([guidelines](https://developer.apple.com/app-store/review/guidelines/), verified). Tailscale/remote = arguably not LAN -> grey zone; Kelpie's text sessions are not pixel mirror, but the desktop-view feature (WebRTC) would fall under 4.2.7. Mitigation: describe as companion to user's own host, LAN/tailnet only, no accounts. Precedent apps (Termius, Blink, Moonlight, Parsec, Screens) exist [U].
- 2.5.2: no downloading code that changes features. Do NOT load UI from daemon in the store app (remote-served bundle = reviewer flag + daemon/app version skew). Bundle UI, version the WS protocol, show "update Kelpie desktop" on mismatch. Terminal output is data, fine.
- Also: privacy policy + nutrition label required (5.1.1) even w/ no collection; reviewer needs a demo path (they have no Kelpie desktop) -> provide demo mode/video; this is the #1 realistic rejection cause (2.1 "can't test") [U].
- Play: remote-control/terminal apps allowed [U]; needs FGS-type declaration if used, data-safety form, 20-tester/14-day closed test for new personal accounts [U].
- Review risk summary: Apple medium (4.2.7 grey, demo access); Play low.

## 5. Ranking (weighted to modern / performance / user-friendly, privacy as hard constraint)

1. PWA + Web Push now (+ Tailscale HTTPS), native later. Zero cloud, zero cost, ships in days; iOS install is the UX tax.
2. Native thin shell (Capacitor first; Tauri 2 mobile acceptable if team wants one toolchain) + content-free relay + Live Activity. Best UX; costs relay ops, Apple/Mac, review risk, native Swift.
3. Tauri 2 mobile shell without relay: no benefit over PWA (local notif only); worst of both.
4. Expo/RN: loses code share.

## Limitations
Search tool returned little 2026-specific data; no primary data on iOS 18/26 Web Push reliability, Tauri mobile production cases, Play policy text. Prior-knowledge items tagged [U]. Not covered: Windows host capture (other reports), pricing quotes of relay hosts.

## Unresolved questions
- Does Kelpie accept being a service operator (relay) for opt-in users? Decides option 2.
- Is HTTPS-by-Tailscale acceptable as push prerequisite, or must LAN-only users get push (needs local CA flow)?
- Does Tailscale-only access satisfy Apple 4.2.7 "LAN-based network" reading? Ask Apple pre-submission or avoid mirror feature in iOS build.
- Real-device test: iOS 26 declarative push in EU-SIM device; subscription survives PWA update?
- Tauri mobile plugin table per-platform (notification actions, stronghold on iOS) not confirmed.

## Pick

**Recommended: installed PWA + self-hosted Web Push (VAPID in kelpied), delivered over Tailscale HTTPS; declarative push payload on iOS 18.4+; in-PWA QR/code pairing; lock-screen = Deny only on Android, Approve via deep-link + biometric/PIN.**

| Criterion | Score /5 | Note |
|---|---|---|
| Modern | 3 | declarative push, iOS 26 default web-app; no Live Activity/widgets |
| Performance | 4 | same WebView engine as native shell; text transport, WebRTC hw decode |
| User-friendliness | 3 | iOS manual Add-to-Home-Screen + HTTPS setup is the weak spot; Android 4 |
| Privacy | 5 | no Kelpie server; payload E2E to push svc |
| Effort (5 = cheapest) | 5 | extends existing PWA |

**Runner-up: Capacitor (or Tauri 2 mobile) native shell + content-free APNs/FCM relay (ntfy poll-request pattern) + Live Activities.** Scores: modern 5, perf 4, friendly 5, privacy 3 (relay sees tokens/timing), effort 1-2. Lost because: requires Kelpie to run a service (contradicts no-cloud principle, ops/abuse/key-custody burden), Mac + $99 + review risk (4.2.7 grey, demo-access), Swift work for Live Activities, and Tauri's mobile push plugin is not production-ready. Graduate to it when iOS install friction measurably blocks adoption; the shared SolidJS client keeps that migration cheap.

Status: DONE_WITH_CONCERNS
Summary: Report written; recommends PWA + self-hosted Web Push now, native shell with content-free relay as the later upgrade.
Concerns/Blockers: Search returned little 2026-specific data; many claims tagged [U] (Tauri plugin mobile support table, iOS 18/26 push reliability, Play policy, Live Activity budgets) and should be device-tested.
