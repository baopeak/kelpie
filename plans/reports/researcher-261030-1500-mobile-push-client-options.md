# Kelpie phone client + push options (research 2026-10-30)

Legend: [U] = unverified / from memory / secondary source only. "Fetched" = primary page read 2026-10-30.
Limits: 1-2 searches per topic; WebFetch of Apple/MDN pages often returned truncated content; no hands-on iOS 26 testing. Bash broken, so no repo check beyond the brief (phone SolidJS client already served by daemon, per recent commits).

## TL;DR
1. **PWA + direct Web Push from kelpied (VAPID, `web-push` crate)** is the primary path. No relay server, no Apple/Google account, $0, payload E2E-encrypted to the phone.
2. **Notification = alert + deep link only.** Approve/deny happens inside the PWA after a WebAuthn (Face ID) check. Do not rely on notification action buttons (iOS support [U]; lock-screen approvals have no user verification).
3. **ntfy** (self-host + upstream-base-url, or UnifiedPush on Android) = optional "alert-only" fallback for people who won't install a PWA.
4. **Telegram bot** = optional opt-in channel, redacted content + deep link. Not E2E. Skip Matrix/Signal/Discord/Slack/WhatsApp for now.
5. **Native (Tauri mobile) = defer.** Needs $99/yr Apple + APNs-key gateway + immature push plugins + Play verification friction.

## 1. Web Push for PWA

| Item | iOS/iPadOS | Android Chrome | Source |
|---|---|---|---|
| Support | 16.4+, only for Home-Screen-installed web app (manifest `display: standalone`/`fullscreen`) | Works in browser or installed, no install needed | [WebKit blog](https://webkit.org/blog/13878/web-push-for-web-apps-on-ios-and-ipados/) fetched |
| Permission | Must be requested from a user gesture; per-app in Settings | Same gesture best-practice | WebKit blog |
| Push service | APNs-backed `*.push.apple.com`; no Apple Developer membership needed | FCM-backed; no Firebase project needed for Web Push w/ VAPID [U] | WebKit blog |
| Declarative Web Push | Safari/iOS 18.4+; JSON with `web_push` key + required `title` + `navigate` URL; works without a service worker; fallback shown if SW fails; survives SW removal by ITP; silent push not allowed | n/a (Chrome uses SW push event) | [WebKit: Declarative Web Push](https://webkit.org/blog/16535/meet-declarative-web-push/) fetched |
| Silent push | No (must show a visible notification; penalties otherwise) | Must be `userVisibleOnly` | WebKit blog |
| iOS 26 | Every Home-Screen site opens as web app by default (toggle available) | - | [mjtsai quoting Jen Simmons](https://mjtsai.com/blog/?p=49472) (secondary) |
| Action buttons | Persistent-notification `actions` is "Limited availability" in MDN; iOS status unconfirmed [U] | Supported (Chrome) [U], `Notification.maxActions` limits | [MDN actions](https://developer.mozilla.org/en-US/docs/Web/API/Notification/actions) fetched (compat table not rendered) |
| Payload | ~4 KB [U] (RFC 8030 min 4096 B) | ~4 KB | Apple page fetch failed; figure from memory |
| Focus modes/DND | Apply | Apply | - |
| EU/DMA | Apple announced removal of Home Screen web apps in EU (iOS 17.4 beta), then reversed before 17.4 release; feature kept. One vendor guide ([MagicBell](https://www.magicbell.com/blog/pwa-ios-limitations-safari-support-complete-guide)) still says no push in EU - conflicts with Apple's reversal ([Engadget/other coverage via search](https://www.engadget.com/apple-confirms-home-screen-web-apps-will-no-longer-work-on-european-ios-devices-112527560.html) is the pre-reversal story). Treat EU push as works, but [U] for 2026 specifics | n/a | search results, secondary |

Reliability: no authoritative iOS 26 reliability data found [U]. Known folklore: iOS drops push permission if user clears Safari data / removes app; Declarative Web Push is Apple's own fix for SW fragility. Mitigation: re-subscribe check on every PWA open + in-app "test push" button + daemon marks subscriptions dead on 404/410.

### Can kelpied push directly (no own server)?
Yes. Daemon holds VAPID private key, POSTs encrypted payload to the subscription endpoint (push.apple.com / fcm.googleapis.com / mozilla). Needs only outbound HTTPS from the PC.
- Rust crate [`web-push` 0.11.0](https://docs.rs/web-push/latest/web_push/) (fetched): VAPID builder, aes128gcm, hyper/isahc clients, Apache-2.0, 3 owners. RFC 8291 not named on page [U] but aes128gcm = RFC 8188 content coding used by 8291.
- Wire: RFC 8291 encryption to the subscription's p256dh + auth secrets; push service sees ciphertext, endpoint, size, timing, VAPID sub. Cannot read content. [RFC 8291](https://www.rfc-editor.org/rfc/rfc8291) [U not fetched, spec known].
- Phone does NOT need to reach the PC to receive the alert. The PC needs outbound only. Phone needs reachability to PC only when opened (LAN/Tailscale).
- Store: subscriptions in daemon state (atomic persistence per AGENTS.md); VAPID key in daemon data dir, 0600.

### Secure context (required for SW + push + WebAuthn)
| Option | Works for iOS PWA push | Effort | Privacy/risk | Verdict |
|---|---|---|---|---|
| `tailscale serve`/`tailscale cert` (`*.ts.net` Let's Encrypt, DNS-01 by Tailscale) | Yes, publicly trusted | Enable MagicDNS + HTTPS in admin console once; renew every 90 d unless using `serve` | Machine FQDN appears in public CT logs; key stays on device ([Tailscale docs](https://tailscale.com/kb/1153/enabling-https) fetched) | **Best default** for Tailscale users |
| Let's Encrypt DNS-01 on user-owned domain, internal A record | Yes | Needs domain + DNS API token + renewal | Hostname in CT (use wildcard to hide names) | Power users |
| Caddy/ACME reverse proxy | Yes | Medium | same | Skip |
| Self-signed / local CA + install profile on iPhone | iOS requires full trust toggle in Settings > About > Certificate Trust; SW/push with user-installed CA [U] reportedly OK but fragile | High, bad UX, MITM-capable root on phone if mishandled | Poor | Avoid |
| Plain HTTP LAN | No (not secure context; `localhost` only exception) | - | - | Not viable |

Note: Tailscale-only means phone reaches daemon only on tailnet (needs Tailscale app on phone, free personal plan [U pricing not fetched]). LAN-only users need domain+DNS-01.

## 2. Native push

| Topic | Finding |
|---|---|
| APNs/FCM | Sender must authenticate with Apple key (.p8, from $99/yr Apple Developer Program, [Apple](https://developer.apple.com/programs/whats-included/) fetched) / Firebase service account. One key per app, owned by publisher, so each user's daemon cannot hold it -> publisher-run gateway required. Self-built = ongoing server + key custody. |
| Content-blind gateway | Pattern: daemon encrypts payload to phone key, gateway forwards opaque blob (+ `mutable-content`), iOS Notification Service Extension decrypts and rewrites text. Signal/Element do variants. Requires native app + NSE; ~2-4 weeks eng [U estimate]. |
| ntfy Android | UnifiedPush or F-Droid build: no Firebase; persistent connection ~0-1%/17h battery per [ntfy FAQ](https://docs.ntfy.sh/faq/) fetched. ntfy.sh default also relays via FCM. |
| ntfy iOS | Instant delivery requires `upstream-base-url: https://ntfy.sh` on self-hosted server; upstream gets only message ID + SHA256 of topic, generic "New message" text; app then polls your server for content ([ntfy config](https://docs.ntfy.sh/config/) fetched). Without upstream: delays of 20-30 min to hours. FAQ describes iOS app as "bare bones and a little buggy," dev paused (page date unknown) [U on current state]. Needs self-host ntfy reachable by phone (again HTTPS/Tailscale). |
| ntfy Web Push | ntfy server itself can Web Push (VAPID, same-server only) - adds nothing over doing it in kelpied. |

Verdict: native push only makes sense once you ship an App Store app. ntfy = cheap alert channel but adds a second server + a second app for the user; keep as optional adapter ("webhook to ntfy URL"), alert + click-URL only.

## 3. Chat-app control surfaces

| Channel | E2E | Inbound path (home PC, no open port) | Approve UX | Setup | Cost | Security notes | Verdict |
|---|---|---|---|---|---|---|---|
| Telegram bot | **No** (only Secret Chats are E2E; bots can't use them) - [Telegram E2E doc](https://core.telegram.org/api/end-to-end) fetched, bot-chat omission = not E2E [U wording] | `getUpdates` long polling, outbound only; incompatible with webhook set ([Bot FAQ](https://core.telegram.org/bots/faq) fetched) | Inline keyboard + callback query = one tap, works on lock screen notification w/ app | Create bot via BotFather, paste token, `/start` to bind chat id (pairing code) | Free (30 msg/s) | Token leak = attacker impersonates bot; mitigate: daemon accepts callbacks only from bound `user_id`+`chat_id`, per-request nonce in callback_data, expire, never put secrets in messages; Telegram sees command text -> send redacted summary. Callback taps have no biometric. | **Opt-in tier 3** |
| Matrix (matrix-rust-sdk + Element X) | Yes (Megolm) if bot device verified | outbound sync | Reactions/polls as buttons (no real inline buttons) | Homeserver account, device verification, crypto store; heavy | Free/self-host | Strong but big dependency + key-store state + push goes via Sygnal/ gateway [U not researched] | No (too heavy) |
| Signal (signal-cli) | Yes | linked device | Text only; no buttons found | Unofficial GPL tool, no ToS statement on page [U] | Free | Fragile unofficial protocol | No |
| Discord | No | Gateway websocket outbound | Buttons | App + bot token + server | Free | Content on Discord | No |
| Slack | No | Socket Mode outbound [U] | Block Kit buttons | Workspace app approval | Free tier | Admin-heavy | No |
| WhatsApp Business | Transport E2E but business API goes via Meta cloud | webhook (inbound server needed) | Reply buttons in templates | Meta business verification | Per-delivered-message pricing since 2025-07-01; service-window utility msgs billable from 2026-10-01; 1000 free service msgs/mo/number ([Meta pricing](https://developers.facebook.com/docs/whatsapp/pricing/) fetched) | Requires public webhook | No |

Is a chat channel worth it? Telegram only: ~1-2 days, zero infra, instant lock-screen alerts with inline buttons, covers users who won't set up HTTPS/Tailscale. Cost: not E2E, bypasses biometric gate -> restrict to low-risk approvals by policy or "deep-link only" mode.

## 4. Phone app delivery

| Question | PWA | Tauri 2 mobile |
|---|---|---|
| Maturity | Existing SolidJS phone client already served by daemon (repo, recent commits) | Official plugins: barcode-scanner (QR), biometric, notification ([Tauri plugins](https://v2.tauri.app/plugin/) via search, mobile coverage per-plugin [U]). Remote push NOT official: 3 community plugins (`tauri-plugin-notifications` 0.4.x w/ FCM/APNs opt-in, `tauri-plugin-remote-push` 1.0.10, `tauri-plugin-mobile-push`) ([docs.rs](https://docs.rs/crate/tauri-plugin-notifications/latest)). Needs manual AppDelegate edits/Firebase. Background work: no official plugin found [U]. |
| Background | iOS: no persistent WebSocket when backgrounded (suspends); push is the only wake path. Fine since alerts are push-driven; reconnect + replay on open (daemon already has replay offsets) | Same OS limits; native can use background tasks but APNs still needed |
| Camera/QR | `getUserMedia` works in iOS standalone PWA [U]; fallback: pairing via typed code / camera app opens URL | barcode-scanner plugin |
| Biometric approval | WebAuthn platform authenticator (Face ID/Touch ID) with `userVerification: "required"`; iOS prompts Face ID/passcode ([passkeys.dev iOS](https://passkeys.dev/docs/reference/ios) via search). Behavior inside standalone Home-Screen mode unconfirmed [U] - must test | Biometric plugin / LocalAuthentication |
| Store friction | none | Apple $99/yr ([Apple](https://developer.apple.com/programs/whats-included/) fetched). Guideline 4.2.7 remote-desktop rules (host = user's PC, **LAN-based network** both ends, executed on host, account creation from host) ([Apple guidelines](https://developer.apple.com/app-store/review/guidelines/) fetched) - Kelpie is a not a screen mirror so 4.2.7 may not apply, but it is a client for desktop software: 4.2.3(i) "work on its own without another app", 4.2 "beyond repackaged website" rejection risk is real [U outcome]. Remote over Tailscale conflicts with "LAN-based" if 4.2.7 applied. Google Play: $25 one-time [U not fetched], personal accounts created after 2023-11-13 need closed test with 12 testers x 14 days ([Play help](https://support.google.com/googleplay/android-developer/answer/14151465) via search); Android developer verification: global enforcement from 2027, hobbyist limited-distribution accounts (<=20 devices, no fee) from Aug 2026, sideload "advanced flow" ([Android](https://developer.android.com/developer-verification) fetched) |
| Cost | $0 | $99/yr + gateway server + review time |

## 5. Approval security (phone)

| Control | How | Fit |
|---|---|---|
| Device pairing | QR/short code from desktop -> phone generates keypair (WebAuthn credential or WebCrypto key); daemon stores public key + role | Reuses daemon auth roles |
| Notification contains no secrets | Title "Claude needs approval - project X"; no command text in push (push service sees metadata anyway; payload is encrypted but lock screen is visible) | Required |
| Transaction confirmation | Daemon issues approval request id + hash of exact command; phone shows full command from daemon (not from push); WebAuthn assertion over challenge = hash(request id, command, expiry); daemon verifies signature, single-use, short TTL | Prevents replay/forged approval, binds consent to command |
| Risk tiers | Low risk (read/edit in worktree): tap approve, no biometric. Dangerous (shell rm, network, git push, outside-worktree): WebAuthn UV required. Deny always one tap | KISS, tune via policy |
| Lock-screen action buttons | Disable for approve; permit "Open" / "Deny" only | Lock-screen tap has no UV |
| Revocation | Desktop UI lists paired phones; revoke deletes sub + key | Needed |
| Quick replies | Short canned replies + free text in PWA; no push inline-reply | iOS support for text-input actions [U] |

## Ranked recommendation

| Rank | Path | Who | Setup | Cost | Security | Risk |
|---|---|---|---|---|---|---|
| 1 | PWA + kelpied direct Web Push (VAPID) + Tailscale HTTPS cert | Default; anyone willing to install Tailscale + Add to Home Screen | 5 min: enable HTTPS in Tailscale, scan QR, Add to Home Screen, allow notifications, passkey | $0 | Best: content encrypted end-to-end to phone, WebAuthn-bound approvals, no third-party server holding keys | iOS push flakiness [U], CT log leaks hostname, EU specifics [U] |
| 2 | Telegram bot (opt-in, redacted + deep link, bound chat id, long polling) | Users who refuse Tailscale/PWA, or want redundancy | 2 min | $0 | Not E2E; no biometric; token custody | Telegram policy/API drift (low) |
| 3 | ntfy self-hosted (upstream-base-url for iOS; UnifiedPush for Android) | Privacy-focused Android users, existing ntfy users | Medium (second server) | $0 | OK: content-blind upstream | iOS app quality [U] |
| 4 | Native Tauri mobile + content-blind gateway | Only if PWA push proves unreliable in real use | High | $99/yr + server + $25 Play | Strong w/ NSE | Review rejection, immature push plugins, maintenance load |
| - | Matrix / Signal / Discord / Slack / WhatsApp | Not recommended | - | - | - | - |

Decision: build #1 now (extend existing phone client: push subscribe, VAPID key mgmt, subscription store, approval screen with WebAuthn). #2 as a small adapter if demand appears. Revisit #4 only on measured iOS push failure rate.

## Unresolved questions
1. Do iOS 26 Home-Screen PWAs deliver Web Push reliably when app closed + locked (test matrix on a real iPhone, 7-day soak)? Does Declarative Web Push remove need for SW push handler?
2. Does WebAuthn with `userVerification: required` work inside standalone Home-Screen mode on iOS 26 (vs Safari tab)? Passkey RP ID must equal the `*.ts.net` host: any breakage if hostname changes?
3. Are notification `actions` rendered on iOS (and with what max)? Not needed for plan, but decides nice-to-have "Deny" button.
4. Self-signed/local-CA SW + push viability on iOS (only matters for non-Tailscale LAN users). Alternative to avoid: kelpied-hosted relay? (rejected: adds server).
5. Tailscale free-plan limits and `tailscale serve` vs `cert` renewal behavior from within kelpied (does daemon invoke CLI or embed tsnet?).
6. Telegram: confirm bots can never use secret chats (docs omit; known behavior [U]); confirm callback_data size limit (64 B [U]).
7. Google Play $25 fee not verified on primary page this session.
8. Push payload size limit on APNs-web (4 KB vs. Apple-specific) not verified from Apple doc (page fetch returned title only).
