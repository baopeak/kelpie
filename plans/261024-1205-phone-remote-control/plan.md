---
title: "Phone remote control: sessions anywhere, screen when needed"
description: "Control every Kelpie session from a phone with end-to-end encryption: a PWA over Tailscale HTTPS with Web Push from the daemon and an app lock first, then one Cloudflare relay, plus browser-pane and desktop/IDE screen takeover."
status: pending
priority: P1
effort: "~74d + 7-day soak (deferred, outside the total: ~10d)"
tags: [mobile, remote, e2e, noise, tailscale, web-push, webauthn, relay, webrtc, screen, security]
created: 2026-10-24
blockedBy: [261023-0048-multi-agent-ide-roadmap]
blocks: []
---

# Phone remote control

## Overview
Today the phone page (`crates/kelpied/mobile/app.js`, 258 lines) polls a session's text every 1.2 s, sends fixed keys, cannot answer chat
sessions, ignores the daemon's prompt detection and stale-answer guard, and only works on one LAN/Tailscale IP. This plan turns the phone
into a full Kelpie controller that works anywhere with **no Kelpie account**, keeps everything end-to-end encrypted, and
adds **screen takeover** when an agent is stuck on a login, a GUI or a dialog.

**Path D (user decision 2026-10-30, see Validation Session 2):** first a PWA over Tailscale (detection, `*.ts.net` HTTPS in the QR, Web Push sent by `kelpied`, app lock, plain-language switches, Telegram options), then one Cloudflare relay. iroh and a native store app are deferred with no phase commitment. This replaces "zero setup, no Tailscale" as the first target.

Blocked by roadmap [261023-0048 phase 12](../261023-0048-multi-agent-ide-roadmap/phase-12-notify-limits-redaction.md), in progress in this
worktree (uncommitted `crates/kelpied/src/notify.rs`, `notify/`, `mobile.rs`). Start phase 1 only after roadmap phases 1–12 are all completed and committed (user decision 2026-10-25, replaces "right after phase 12"). Phase 2's composer core
is shared with roadmap [phase 9](../261023-0048-multi-agent-ide-roadmap/phase-09-composer-context.md) (whichever lands first creates it).

Research (`plans/reports/`): [synthesis](../reports/research-261024-1832-phone-control-architecture-synthesis.md) ·
[Orca + zero-setup](../reports/researcher-261024-1851-orca-zero-setup-competitors.md) ·
[zero-trust relay + native shell](../reports/researcher-261024-1851-zero-trust-relay-native-shell.md) ·
[connectivity](../reports/researcher-261024-1820-phone-connectivity-pairing.md) ·
[landscape](../reports/researcher-261024-1820-mobile-agent-control-landscape.md) ·
[session protocol + UX](../reports/researcher-261024-1820-mobile-session-protocol-ux.md) ·
[app shell + push](../reports/researcher-261024-1820-mobile-app-shell-push.md) ·
[remote desktop](../reports/research-261024-1820-remote-desktop-view-control.md) ·
[Windows host](../reports/researcher-261024-1820-windows-host-capture-input.md) ·
[phone transport](../reports/researcher-261024-1820-mobile-client-transport.md) ·
[takeover + macOS](../reports/researcher-261024-1820-takeover-patterns-macos.md) ·
[screen spikes](../reports/researcher-261024-1820-screen-control-gaps-spike.md) ·
2026-10-30: [options synthesis](../reports/research-261030-1530-phone-remote-access-options-synthesis.md) ·
[relay synthesis](../reports/research-261030-1600-relay-speed-reliability-security-synthesis.md) ·
[re-plan report](../reports/planner-261030-1600-phone-path-d-replan.md)

## Brainstorm contract
- **Outcome**: from a phone, anywhere, the user sees every session across agents (who needs them first), reads and answers terminal and chat
  sessions with one tap or a real keyboard (Telex/VNI safe), gets a push when one needs them, and can take over a browser pane or the
  desktop / an IDE window to type a password, pass 2FA or click a dialog.
- **Constraints**: no Kelpie account; first target needs Tailscale on both ends (guided), every option a plain-language switch with safe defaults; nothing readable leaves the machine (relay sees ciphertext only; the opt-in Telegram approvals are the stated exception); off by default;
  daemon stays per-user and per-user-installed; MIT-compatible dependencies; EN + VI; Windows first, macOS parity; no work or sockets when off;
  e2e never touches the real daemon (`KELPIE_ENDPOINT` in the test's own process).
- **Non-goals**: UAC secure desktop, lock/login screen, elevated windows (need a SYSTEM service — reported clearly instead); audio; file transfer; multi-user sharing; any Kelpie cloud storage.

## Why this beats Orca
Orca (v1.4.239, mobile 0.0.58 beta, Expo app): LAN `ws://` or `relay.onorca.dev` **with an Orca account on both ends**, NaCl app-layer encryption
(forward secrecy unconfirmed), cloud push gateway, **no screen control**. Kelpie: no account (keys in the QR), Noise with forward secrecy, self-hostable
relay, daemon-enforced app lock, one inbox across 23 agents, browser-pane and desktop/IDE takeover, and the PWA from the daemon itself (no store/version skew).

## Architecture decisions (final, after red team; amended 2026-10-30)
| # | Decision | Why |
|---|---|---|
| D1 | **One E2E protocol `kelpie-link`**: Noise `IKpsk1` pairing (psk checked in msg1), `IK` reconnect, via `snow` 0.10 (RustCrypto resolver, no ring) in the daemon, JS on noble in the page, over any pipe (Tailscale HTTPS, LAN WS, relay WS) | One protocol for daemon and page; desktop verifies the QR secret before any prompt |
| D2 | **Pair by QR keys, no accounts**; phone shows a 6-digit code the user **types** on the desktop. *Amended 2026-10-30*: with no native app, the default QR is the two-step install + pair for the `*.ts.net` origin (3a); plain-`http://` browser pairing stays an explicit separate action; `kelpie://pair` waits for the native app | Beats account-based Orca/Claude Remote Control; a swapped `http://` page cannot ride the default path |
| D3 | **Connect race**. *Amended 2026-10-30*: in the PWA's `link.ts`, not Rust (`kelpie-link::client` deferred with native). Candidates from the pairing record: `ts`, then the single relay after 250 ms; first IK handshake wins; migrate to direct after 10 s stable. An HTTPS page cannot dial `ws://<LAN IP>` (mixed content), so a raw LAN address is only used by a plain-`http://` page, on its own origin | Reach without losing a direct path; Tailscale connects directly on the same Wi-Fi |
| D4 | **Untrusted relay**, opt-in, *amended 2026-10-30*: **one** Cloudflare Workers + Durable Object relay, personal use first (public service a later decision); pairing record holds a list with one entry; no `apac` hint, the desktop's first `get()` places the object, room re-created if RTT stays high; hibernation and billing guards; hardening from relay synthesis section 3; push and TURN secrets not in the data path; same code self-hosts on `workerd`. Replaces the 2026-10-25 "hosted default" detail only where listed; validated on a real deployment first (4a). Sees ciphertext, IPs, timing, sizes, room id | Nothing to patch or keep alive, cost follows usage, DDoS absorbed; accepted: ~0–25 ms over a same-city VPS [E], drops on deploys and runtime updates, DO incidents |
| D5 | **Trust root**, *amended 2026-10-30*: the page is served only by the daemon, over Tailscale HTTPS (daemon-origin PWA); the relay never serves code. A device with `secure_origin` may use the relay (frames only) and, **pending user confirmation before phase 7**, hold `control` once app-locked. Plain `http://` stays capped at `read`, no relay token, no `control`. Signed native app deferred | No code origin but the user's own machine; PWAs cannot be pinned on iOS |
| D6 | **One phone client** (`app/phone`, own Vite config), chat adapters reused in **observer mode** over a narrow transport | No three UIs; phone never initializes or reconfigures an agent |
| D7 | **Native shell = Tauri 2 mobile**, keys + biometric gate enforced in Rust. **Deferred 2026-10-30**, no phase commitment; iroh only with it and only after a spike | Shares Rust crates; Capacitor documented fallback |
| D8 | **Sessions travel as state**: line-diff frames by session id with per-device resume; first-wins answers for terminal and chat prompts | 4G bandwidth, Telex-safe input, no double answers |
| D9 | **Push**, *amended 2026-10-30*: **Web Push sent directly by `kelpied`** (VAPID, RFC 8291, content-free alert + deep link) for HTTPS PWA devices; ntfy and Telegram stay optional channels; native APNs/FCM gateway, sealed details and the relay-lease lost-contact push deferred. Supersedes "no Web Push" and RT-14's disposition (SSRF guard kept) | No server of ours, no Apple/Google account; HTTPS comes from Tailscale (3a), so the old objection falls away |
| D10 | **Screen, two tiers**: browser pane via WebView2 CDP screencast (Windows); desktop/window via app-spawned `kelpie-screen` (WGC/DXGI + MF H.264 + `str0m` WebRTC + `SendInput`; macOS SCK + VideoToolbox + `CGEventPost`). Desktop video never goes through the relay. *Amended 2026-10-30*: both tiers run over Tailscale from the installed PWA; relay-mode takeover and TURN deferred | P2P UDP for video; cheap takeover for logins |
| D12 | **Built for flaky networks**: encrypted heartbeat 20 s / pong 8 s / dead 45 s (lag-aware), ack window with latest-state-only, `peer_reset` + IK re-handshake on any pipe drop, address advertisement after IP changes, no hidden offline queue (input disabled offline, answers retried once with first-wins), durable push outbox, WebRTC restart ladder | Carrier NAT idle timeouts 30–120 s; half-open sockets take minutes to fail; Noise transport desyncs on lost frames ([research](../reports/researcher-261025-1744-unstable-network-resilience.md)) |
| D13 | **Built for a busy computer**: one `load.rs` level machine (L0–L3, hysteresis) fed by the daemon's own event-loop lag, sink queue age and encoder time; every phase sheds rate/size by level; input, answers, chat sends/cancels/stops/resumes (2b), revoke, stop and heartbeats are never shed; new screen sessions refused at L2+; EcoQoS opt-out, MMCSS for capture/encode, never HIGH/REALTIME | 5–20 agents with builds peg the CPU; a windowless daemon gets throttled ([research](../reports/researcher-261025-1744-cpu-overload-resilience.md)) |
| D11 | **Trust boundary**: sensitive settings (approve, upgrade to write/control, always-allow, relay on, reset) need the verified Kelpie app process (`Role::App`) + OS user verification; per-device `read` < `write` < `control`; grants keyed to device + connection + provider and stopped on revoke | Same-user programs can read the user token (`docs/architecture.md:72`) |
| D14 | **Daemon-owned chat turn queue (single writer)**: one bounded FIFO per chat in `kelpied`, written only when the agent is idle; phone `chat_send` (write) is text the daemon builds per dialect at write time; desktop entries are built by its adapter at dequeue (`chat.turn_ready` → `chat.write_turn`), plain text falls back to the daemon when the window is gone; every turn logged as `kelpie_sent`; `chat_send/cancel/stop/resume` never shed, `cid` dedupe 10 min | One writer per conversation, phone works with the window closed, phone stays an observer, rich desktop features stay in TS ([phase 2b](phase-02b-phone-chat-send.md), review W5) |
| D15 | **Tailscale mode** (3a): daemon detects the CLI, publishes `https://<fqdn>:8443` with `tailscale serve` to a loopback listener, QR holds that origin; `tailscale cert` + the existing TLS listener is the fallback if the spike fails; fixed port because the origin holds keys, passkey and push subscription | Secure context with no server of ours; Web Push and WebAuthn both need it |
| D16 | **App lock, not per-command step-up** (3c, user decision 2026-10-30): WebAuthn platform authenticator with UV, enforced by the daemon (locked connections get no session data); once unlocked, full rights for the granted permission; auto-lock on leaving and after a configurable idle time; daemon risk label + full command as information; "ask Face ID again for dangerous commands" off by default; pairing and approving stay on the desktop | Convenience the user chose; daemon check means a copied key alone reaches nothing |
| D17 | **Plain `http://` LAN fallback**: *proposed, awaiting the user*: read-only until HTTPS, no PIN; switch to close reading too | A PIN typed into a swappable page protects little; `transport_cap` already enforces `read` |
| D18 | **Every option a plain switch** (3d): what it does / trade-off / how safe, safe defaults; weakening needs `Role::App` + OS verification | User decision 2026-10-30 |
| D19 | **Telegram** (3e): one-way alerts stay; alert + deep link default when on; opt-in low-risk approvals with a clear warning, outbound long polling, bound to `user_id` + `chat_id`, per-request nonce + expiry, one-shot, audited | User decision 2026-10-30; not E2E and no Face ID, so opt-in and `low` only |

```
Phone: installed PWA from the daemon (app/phone), locked until WebAuthn passes
   │  kelpie-link (Noise) frames
   ├── Tailscale: wss://<fqdn>:8443/link (tailscale serve → 127.0.0.1) ─┐
   ├── later, one relay: wss://<relay>/v1/phone (ciphertext only) ──────┤
   └── plain http:// page on the LAN (own origin, read-only) ───────────┤
                                                                        ▼
          kelpied: lock, verbs (read/write/control), devices v2, state-sync, chat observer + first-wins answers, risk labels,
                   notify destinations (Web Push, ntfy, Telegram, webhook), takeover broker
                     │ provider channel (Role::App, request/response)      │ outbound: push services, Telegram getUpdates
                     ▼                                                     ▼
          Kelpie app: browser panes (CDP screencast) · spawns kelpie-screen ══ WebRTC over Tailscale ══▶ phone
```

## Execution state — 2026-10-31
- **On master** (ddf52cf, past v0.1.10, API 20 taken by chat view parity): phases 1 and 2.
- **Committed locally, not pushed** (`phone/link-on-master`, on master, API 21): phase 2b. `CHAT_QUEUE_API` and `PHONE_API` moved with the levels.
- **Committed locally, not pushed** (`phone/link-on-master`, stacked on 2b, API 22): phase 3. Going out as a release with two known limits, both the owner's decision on 2026-10-31:
  1. The real OS prompt has run only on the Windows Hello PIN path (Windows 10, a real iPhone). The password dialog, Hello Face, Microsoft or PIN-only accounts on the fallback, macOS and Linux have not. **Linux's polkit is released untested on a real Linux desktop, and the release notes say so:** [manual steps](../reports/fullstack-developer-261031-0100-phone-phase3-fixes-2.md), with [changes](../reports/fullstack-developer-261031-0550-phone-phase3-fixes-3.md) and [more](../reports/fullstack-developer-261031-0720-phone-phase3-fixes-4.md).
  2. Eight phones following three printing sessions cost about 10 points of a core at L0 against the script's limit of 5. Encryption adds 0.1 to 1.3 of them, so this phase's own criterion is met; the rest predates it ([measurements](../reports/debugger-261031-0750-phone-link-cpu.md)). **User decision 2026-10-31: release first, and reduce phones' CPU in a later release.**
- **Next, in order:** 3a → 3b → 3c → 3d → 3e → 4a → 4 → 7 → 8 → 9.
- **Handoff:** [plans/handoffs/phone-remote-control-phase3-20261030-1550.md](../handoffs/phone-remote-control-phase3-20261030-1550.md).

## Phases
Numbers are stable names, not an order (no rename: other phases and reports refer to them; 2b set the precedent). **Execution order: 3 → 3a → 3b → 3c → 3d → 3e → 4a → 4 → 7 → 8 → 9.**

| # | Phase | Priority | Effort | Depends on | API | Status |
|---|---|---|---|---|---|---|
| 1 | [Phone protocol v2: state-sync, prompts, safe answers](phase-01-phone-protocol-v2.md) (+ resilience addendum, second commit) | P1 | 4d + 3d | roadmap phase 12 | 17 | done |
| 2 | [Phone client app (SolidJS, PWA)](phase-02-phone-client-app.md) | P1 | 7d | 1 | | done |
| 2b | [Send chat messages from the phone (single writer)](phase-02b-phone-chat-send.md) | P1 | 6d | 2 | 21 | done |
| 3 | [kelpie-link: E2E pairing, trusted approval, device store v2](phase-03-kelpie-link-e2e.md) (+ 2026-10-30 extras) | P1 | 7d | 2; 2b for chat-write verbs | 22 | committed; merge blocked, see Execution state |
| 3a | [Tailscale mode: detection, HTTPS origin, guided PWA install](phase-03a-tailscale-https.md) | P1 | 4d | 3 | 23 | code on `phone/phase3a-tailscale`; desktop half verified on a real tailnet, real phones pending |
| 3b | [Web Push sent directly by kelpied](phase-03b-web-push.md) | P1 | 4d | 3a, roadmap phase 12 | 24 | pending |
| 3c | [App lock (WebAuthn) and daemon risk labels](phase-03c-app-lock.md) | P1 | 6d | 3a | 25 + security level 3 | pending |
| 3d | [Settings → Mobile: plain-language switches](phase-03d-mobile-settings-toggles.md) | P1 | 2d | 3a, 3b, 3c | none expected | pending |
| 3e | [Telegram: alert + deep link, opt-in low-risk approvals](phase-03e-telegram-options.md) | P2 | 3d | 3b, 3c, 3d | 26 | pending |
| 4a | [Relay validation on a real Cloudflare deployment](phase-04a-relay-cloudflare-validation.md) (run ahead of the order at the owner's request, Free plan on `*.workers.dev`, see the phase) | P1 | 1d + 7-day soak | 3e | none | in-progress |
| 4 | [Relay: one Cloudflare relay, connect from anywhere](phase-04-relay-connect-anywhere.md) | P1 | 6d | 3, 3a, 3c, 4a | 27 | pending |
| 7 | [Browser pane takeover + app provider channel](phase-07-browser-pane-takeover.md) (over Tailscale; relay mode deferred) | P2 | 5d | 3, 3a, 3c | 28 | pending |
| 8 | [Desktop / window screen control — Windows](phase-08-desktop-screen-windows.md) (over Tailscale; TURN deferred) | P2 | 10d | 3, 3a, 3c, 7 | 29 | pending |
| 9 | [Desktop / window screen control — macOS](phase-09-desktop-screen-macos.md) | P2 | 6d | 7, 8 | none expected | pending |

**Deferred, no phase commitment** (content kept; not in the total):

| # | Phase | Priority | Effort | Returns when | Status |
|---|---|---|---|---|---|
| 5 | [Native mobile shell (Tauri 2 iOS/Android)](phase-05-native-mobile-shell.md) | P3 | 7d | Real use and demand justify a store app; iroh only after a hands-on spike | pending (deferred) |
| 6 | [Native push: APNs/FCM gateway, sealed details, lost-contact](phase-06-push-notifications.md) | P3 | 3d | With phase 5; lost-contact on the user's decision | pending (deferred) |

API levels are expected values: each phase takes the next free `API_LEVEL` at commit time (`kelpie-proto/src/lib.rs:34`, 22 today).

Order: daily value (1–2, 2b), security foundation (3), Tailscale reach with its secure context (3a) because push and lock need it, push and lock, the switches once there is something to switch, Telegram last because it reuses risk labels and the switch row. Relay after all of that (user decision 2026-10-30), with validation first. Screen last with spike gates. Phases 3a–9 all touch `crates/kelpied/src/mobile.rs` and `daemon.rs`, and 3a–3e share `mobile-settings.tsx`: **strictly sequential, no parallel work** (matches the existing execution rule: one branch per phase off master). Only the 4a soak runs alongside other phases; it touches no product file.

## Security model (applies to every phase)
- Off by default; no listener, outbound socket, timer or helper when off.
- **Boundary stated honestly**: any same-user program can read the user token. Sensitive mobile settings therefore require `Role::App` (verified Kelpie process) + OS user verification;
  residual risk = code injection into the app process (same user), documented.
- Devices = Noise static public keys in `mobile.v2.json` (MAC'd); desktop private key in the OS keyring; device add/permission change ⇒ desktop toast + notice to other phones.
- **App lock (2026-10-30)**: a connection is locked until a WebAuthn assertion passes at the daemon; locked means no session data. Once unlocked, the device has full rights for its permission; approvals show the daemon's risk label and the full command as information. Optional re-check off by default. Cache at rest on the phone is not encrypted (documented).
- Permissions: new device `read`; PWA `write` only over TLS/Tailscale, `read` over plain `http://`. `control`: native only as decided 2026-10-24; with the native app deferred, proposed for app-locked `secure_origin` PWA devices, **to be confirmed before phase 7**. Every message checked against the live record; revoke/downgrade/lock stops control grants before closing sockets.
- Phone verbs are a closed table; no generic daemon-method proxy; phone typing goes through `typed_text` and the agent's Enter delay. No settings verb: pairing, approving and settings stay on the desktop. Phone approvals are one-shot.
- Relay: no accounts for phones, registration key for rooms while personal, ticket checked before any DO, per-device relay tokens (hashes only, 30 days), caps, no payload logging, persisted pending revocations; never serves code; self-host documented.
- Web Push payloads carry no session text; push services see endpoint, size and timing. Endpoint host allow-list against SSRF.
- Telegram approvals are the one path that shows command text to a third party and skip the lock: opt-in, `low` only, one-shot, bound, audited, stopped by the kill switch.
- Tailscale sees metadata, and the machine name appears in public certificate logs (stated in Settings).
- Screen control: user-presence confirmation per device, visible indicator, stop hotkey, idle auto-stop 10 min, audit start/stop (never keystrokes), revoke ⇒ input stops < 1 s.

## Success Criteria
- [ ] Tailscale on both ends: toggle on → scan → install → scan → type the code on the desktop + OS verification → sessions on 4G, no Kelpie account.
- [ ] Opening the app asks for Face ID/biometrics; without it the daemon gives out no session data; leaving the app re-locks; a network flap in the foreground does not.
- [ ] Same on LAN via a plain `http://` browser pairing, read-only.
- [ ] Terminal prompt and chat permission answered with one tap, label and full command shown; first answer wins across devices; Telex/VNI never sends mid-composition.
- [ ] A program holding only the user token cannot add a device, raise permissions, enable Tailscale mode or the relay, weaken the lock or turn on Telegram approvals.
- [ ] Needs-you Web Push accepted by the push service ≤ 2 s p95 (device delivery measured on real phones); payload has no session text; no push lost or duplicated across a desktop outage; 7-day iOS soak reported.
- [ ] Every option is a switch in Settings → Mobile with its three plain lines, EN + VI, safe defaults.
- [ ] Telegram: alert + deep link works; approvals only for the bound user, low-risk, once, before expiry.
- [ ] Flaky network: Wi-Fi ↔ 4G handover ⇒ sessions back ≤ 3 s p95 and screen stream ≤ 5 s p95 without user action; 90 s blackhole ⇒ clean reconnect, cached view stays readable, no stale input executed, no duplicate answer (fault-proxy + netem matrix).
- [ ] Busy computer: 30 min all-core stress, 20 sessions, 8 phones ⇒ 0 false disconnects, input echo p95 ≤ 300 ms, revoke < 1 s, quality reduced visibly (never a silent freeze).
- [ ] Relay: installed app works with Tailscale off on the carriers measured in 4a; relay outage with Tailscale on ⇒ sessions continue; operator (logs + storage) cannot read any session content (test harness inspects relay traffic); hibernation holds and the bill matches the projection.
- [ ] Browser-pane OAuth/2FA login (popups included) completed from the phone on Windows.
- [ ] Desktop/IDE window viewed and controlled over Tailscale: ≤ 150 ms p95 LAN, ≤ 250 ms direct P2P; secure-desktop/elevated states reported.
- [ ] Feature off: no sockets, no helper, no measurable CPU (perf gate).
- [ ] EN + VI; user guide, architecture, README privacy updated; e2e green on Windows and macOS CI.

## Resilience update — 2026-10-25
Trigger: user asked to research and add unstable-network and CPU-overload cases. Research: [unstable networks](../reports/researcher-261025-1744-unstable-network-resilience.md) · [host overload](../reports/researcher-261025-1744-cpu-overload-resilience.md); code scout of existing mechanisms (no phone heartbeat, unbounded per-connection event channel, no network-change detection, no load sampler; reusable: visible-terminal priority, `SUB_BUDGET`/resync, `7a28b74` busy-probe pattern).
Added D12, D13, a "Resilience" section to every phase, the phase-1 resilience addendum (second commit), success criteria for handover/blackhole/stress, and effort +12 d.
User decisions (2026-10-25): (1) never lower hidden agents' priority under load — only stream quality/rates are shed; (2) no new keep-awake behaviour — Settings suggests the existing "while agents are working" mode; (3) lost-contact push via a relay lease + Durable Object alarm (90 s), sealed per-device details decrypted on the phone (iOS Notification Service Extension), sleep vs network loss distinguished, default off, opt-in per device.
Superseded figures: phase 7 relay "≤ 2 fps q50" → byte budget 55 KB/s; phase 4 keepalive "≤ 1 / 30 s" → "≤ 1 / 20 s"; phase 4 "1.5 s LAN timeout / probe every 30 s" → 250 ms stagger + interface-change triggers; push "≤ 10 s p95" → gateway ≤ 2 s p95 + measured delivery; answer dedupe 60 s → 10 min.

## Red Team Review

### Session — 2026-10-24
**Findings:** 15 after dedupe of 40 (15 accepted, 0 rejected) · **Severity:** 5 Critical, 9 High, 1 Medium
Reviewers: Security Adversary (Fact Checker), Failure Mode Analyst (Flow Tracer), Assumption Destroyer (Scope Auditor), Scope & Complexity Critic (Contract Verifier). All findings cited `file:line`.
Applied directly under the user's grant of full architectural authority. One sub-suggestion rejected: dropping the CDP tier (kept — no OS permission, lands before phase 8; the macOS duplicate provider was removed instead).

| # | Finding | Severity | Disposition | Applied to |
|---|---|---|---|---|
| RT-1 | Same-user token can pair/approve/grant control (`auth.rs:19-21`, `daemon.rs:1395-1408`) | Critical | Accept — `Role::App` + OS user verification, MAC'd devices, change toasts | Plan D11, Phase 3, 4, 8 |
| RT-2 | Default `http://` QR hands psk to a swappable page; PWA key theft; JS-reachable gate | Critical | Accept — app-only QR default, typed code, PWA max `write` (refined in Validation 1: transport-capped), Rust-side keys + biometric gate | D2, D5, Phase 3, 5 |
| RT-3 | `IKpsk2` cannot verify psk before the prompt; codes spent by junk | Critical | Accept — `IKpsk1`, trial decrypt, spend only on success | D1, Phase 3 |
| RT-4 | Phone ChatModel = second protocol client (re-`initialize`, double answers, `session.update`) | Critical | Accept — observer mode, pinned lines, delta apply, daemon first-wins `chat_answer`, non-focusing attach | Phase 1, 2 |
| RT-5 | Answer guard passes for hook waits already answered (`status.rs:110-121`) | Critical | Accept — `answered_wait` set by answer and input | Phase 1 |
| RT-6 | No phone verb table; phone typing skips `typed_text`/Enter delay; duplicate key mapping/composer | High | Accept — closed verb table, shared typed-input fn, `keyAnswer`, shared composer core | Phase 1, 2 |
| RT-7 | No wake source for sync; resume impossible per-connection; lock contention; xterm toggle has no data | High | Accept — per-session watch, `register_client` adapter, per-device resume cache, snapshot outside lock, 8-phone perf gate, xterm view dropped | Phase 1, 2 |
| RT-8 | Handles reassigned after restart ⇒ writes hit the wrong session | High | Accept — address by session id, per-device dedupe, drop writes on epoch change | Phase 1, 3 |
| RT-9 | No contract versioning; `mobile.json` wiped on schema change; `#s=` links break | High | Accept — `API_LEVEL`/`DAEMON_SECURITY_LEVEL` bumps, `mobile.v2.json`, never overwrite on parse failure, alias, deep link kept, 404 ⇒ update desktop | Phase 1, 2, 3, 5 |
| RT-10 | Build pipeline: `build.rs` exists, order, caching, placeholder in release, shared Vite outDir, typecheck, wasm in release | High | Accept — modify `build.rs`, `rerun-if-changed`, `KELPIE_REQUIRE_PHONE`, release grep, `vite.phone.config.ts`, CI/release order | Phase 2, 3 |
| RT-11 | `snow` defaults pull ring; `getrandom` `js` gone; CSP blocks wasm | High | Accept — pinned features, `wasm_js`, ring gate, `'wasm-unsafe-eval'` only | Phase 3 |
| RT-12 | Revoke/downgrade misses lagged broadcasts and leaves screen/takeover control alive | High | Accept — live per-message checks, Lagged ⇒ recheck, grants keyed and stopped < 1 s, persisted relay removals | Phase 3, 4, 7, 8 |
| RT-13 | Takeover provider spoofable; no daemon→app RPC; popups not child webviews; helper owner wrong (`kelpie-stt` is app-spawned) | High | Accept — `Role::App` provider channel with timeouts, reuse `devtools()`, popup pane groups, app-spawned helper, macOS uses window target | Phase 7, 8, 9 |
| RT-14 | `notify.rs` closed enum + serial worker; Web Push SSRF/HTTPS setup; gateway auth + metadata | High | Accept — per-destination queues, `active` incl. push devices, Web Push dropped (ntfy for PWA), gateway credential, metadata documented | D9, Phase 6 |
| RT-15 | Single-IP bind breaks the race and relay pairing; derivable room ids; wrong dependency graph; connect race built twice | Medium | Accept — bind all usable addresses + rebind, pairing without LAN bind, random room id, deps fixed (8: 3,4,5,7; 9: 7,8), race once in Rust | D3, D4, Phase 3, 4, 5, 8, 9 |

### Whole-Plan Consistency Sweep
- Searched all plan files for superseded terms: `IKpsk2` (none left), `Web Push`/`webpush` (only as "not built" in D9/phase 6), `handle` addressing (phase 1 uses session id; `handle` only as history),
  `Create: crates/kelpied/build.rs` (now Modify), `like kelpie-stt` spawned by kelpied (now app-spawned), `channel trait` (removed), `xterm` toggle (removed from phase 2), `disjoint files` (replaced),
  `npm run` (now `pnpm`), `macos-window` provider (removed), single-IP listener (now all usable addresses).
- Dependencies reconciled between frontmatter and the phases table (7: [3] + relay mode needs 4; 8: [3,4,5,7]; 9: [7,8]); effort total 47 d.
- Unresolved contradictions: none.

## Validation Log

### Session 1 — 2026-10-24
**Trigger:** post-red-team validation requested by the user. **Questions asked:** 6

#### Verification Results
- Claims checked: 12 (new claims added by the red-team rewrite; older claims verified by the red team)
- Verified: 12 | Failed: 0 | Unverified: 0 · Tier: Full (spot check, red-team evidence reused)

#### Questions & Answers
1. **[Architecture]** Who runs the "connect from anywhere" relay (plus push gateway and TURN)?
   - Options: Hosted + self-host | Self-host only | No relay yet
   - **Answer:** Hosted + self-host — maintainer's Cloudflare account by default, `workerd` self-host supported.
2. **[Risk]** App Store reviewers have no Kelpie desktop — how to pass review?
   - Options: Notes + video | Real always-on demo desktop | Android first
   - **Answer:** Notes + video (no fake data).
3. **[Assumptions]** After the native app ships, what may the LAN PWA do?
   - Options: Write on HTTPS/Tailscale, read on plain http | Write everywhere | Read only
   - **Answer:** Write on HTTPS/Tailscale; plain `http://` read-only; `control` native only.
4. **[Scope]** Priority versus the remaining roadmap phases (8–11, 13–15)?
   - Options: Right after roadmap phase 12 | After the whole roadmap | Parallel worktree
   - **Answer:** Right after roadmap phase 12, before its remaining phases.
   - **Amended 2026-10-25 by the user:** wait until roadmap phases 1–12 are all done (8–11 included), then implement this plan.
5. **[Tradeoffs]** Screen control needs the Kelpie app running — accept?
   - Options: Needs the app open | Works with the app closed (+3–4 d)
   - **Answer:** Needs the app open (sessions remain controllable with the app closed).
6. **[Risk]** User verification when Windows Hello / Touch ID is unavailable?
   - Options: OS password prompt | 6-digit code only | Require biometrics
   - **Answer:** OS password/PIN prompt.

#### Confirmed Decisions
- Relay: hosted default + self-host — zero setup without lock-in.
- App Review: reviewer notes + video.
- PWA permission capped by transport (daemon-enforced).
- Order: this plan starts after roadmap phases 1–12 are completed and committed (amended 2026-10-25).
- Screen control requires the app; sessions do not.
- User presence fallback: OS credential prompt.

#### Action Items
- [ ] Maintainer: pick the relay domain (placeholder `relay.kelpie.dev`), set up Cloudflare, Apple (APNs key) and Google (FCM) accounts before phases 4–6 deploy.

#### Impact on Phases
- Phase 2: read-only UI for plain-`http://` PWA links.
- Phase 3: transport-capped effective permission.
- Phases 4–6: hosted deployment confirmed; self-host doc stays in scope.
- Phase 5: reviewer notes + video in store prep (already planned).
- Phases 3, 8: OS credential fallback and app-required screen control (already planned).

### Whole-Plan Consistency Sweep
- Searched for `max \`write\``, "PWA max", "make it read-only", "Open questions": all updated to the transport-capped rule; open questions resolved into decisions + one maintainer action item.
- Unresolved contradictions: none.

### Session 2 — 2026-10-30
**Trigger:** user chose how the phone reaches the desktop after reviewing the [options](../reports/research-261030-1530-phone-remote-access-options-synthesis.md) and [relay](../reports/research-261030-1600-relay-speed-reliability-security-synthesis.md) research. Recorded by the planner from the coordinator's brief; the wording is the coordinator's relay of the user's intent. Details: [re-plan report](../reports/planner-261030-1600-phone-path-d-replan.md).

#### Decisions
1. **Path D.** First a PWA over Tailscale: Kelpie detects Tailscale and puts the `*.ts.net` address in the QR; HTTPS from `tailscale serve`/`cert`; Web Push sent directly by `kelpied`; Home Screen install is a guided step. Then **one relay on Cloudflare Workers + Durable Objects**, personal use first, a public service a later decision, validated on a real deployment (about 1 day + a 7-day soak). The same day a Hong Kong VPS was chosen, then replaced by Cloudflare; earlier "spike Cloudflare vs iroh" and two-relay options are also dropped. iroh and a native store app are deferred with no phase commitment. APNs/FCM deferred with the native app.
2. **Security model: app lock instead of per-command step-up.** WebAuthn platform authenticator, UV required, needs the Tailscale secure context. Once unlocked, full rights for the granted permission. Auto-lock on leaving and after a configurable idle time. Full command and daemon risk label shown as information. Optional "ask Face ID again for dangerous commands", off by default. Pairing and approving stay on the desktop. Plain-`http://` LAN needs a defined fallback (open).
3. **Everything is a user-facing toggle with plain explanations.** Settings → Mobile: Tailscale mode, Web Push, app lock and timeout, dangerous re-check, Telegram, ntfy. Safe defaults.
4. **Telegram.** One-way alerts stay; alert + deep link is the default when on; opt-in "approve low-risk requests" with a clear warning; outbound long polling, bound to `user_id` + `chat_id`, per-request nonce + expiry.

#### Earlier decisions this changes
| Earlier | Now | Status |
|---|---|---|
| Relay first (phase order 3 → 4–6), "no Tailscale" zero setup | Tailscale work first, relay after | User decision |
| D9 "no Web Push", RT-14 | Web Push from `kelpied` | User decision |
| D5 "trust root = signed native app", phase-5 store app | Daemon-origin PWA; native deferred | User decision; D5 detail derived |
| 2026-10-24 Q3 "`control` native only" | Needed for phases 7–9 now the native app is deferred | **Derived, unconfirmed: ask the user** |
| 2026-10-25 (3) lost-contact push via relay lease + alarm | No delivery path: a dead desktop cannot send Web Push | **Deferred, unconfirmed: ask the user** |
| 2026-10-24 Q1 hosted + self-host, phase 4 "20 GB/month", "≤ 30 ms in-region" | One Cloudflare relay, personal first; ~2 GB/month; claim removed | User decision + synthesis |

#### Open decision points (see the report)
Plain-`http://` fallback · idle-timeout default and grace on leaving · `control` for the PWA · lost-contact push · `web-push` crate vs a RustCrypto module · first install needs Tailscale, so a hosted PWA waits for going public · 4a thresholds · relay domain and account.

#### Action Items
- [ ] Maintainer: Cloudflare account, Workers Paid, relay domain and budget alert before 4a. Apple and Firebase accounts no longer needed.
- [ ] `ak plan`: Bash was unavailable when this was edited, so status cells were written by hand: new rows `pending`, phases 5 and 6 left `pending` under a Deferred heading, phase 3 not moved to in-progress. Run `ak plan reindex` and set statuses through the CLI.

#### Impact on Phases
- New: 3a, 3b, 3c, 3d, 3e, 4a. Reworked: 4 (keeps Cloudflare and `workerd`). Reworked and deferred: 6. Deferred: 5. Dependencies fixed: 7, 8, 9.

### Whole-Plan Consistency Sweep — 2026-10-30
See the report. Older red-team tables and validation answers above stay as history and are overridden by this session where the table says so.

### Session 3 — 2026-10-31
**Trigger:** the owner asked for the relay validation (4a) on their real Cloudflare account and supplied the token.

#### Decisions
1. **Free plan, not Workers Paid**, while there are no users; revisit when usage grows. Durable Objects use the SQLite backend.
2. **`*.workers.dev` first, no custom domain.** No budget alert is needed, because the Free plan fails rather than bills. The WAF measure cannot run without a domain and is marked not run.
3. **Cost threshold**: projected use stays within the Free limits (100,000 requests/day, 13,000 GB-s/day, incoming WebSocket messages count 20:1). Latency and disconnect thresholds are unchanged; a failed threshold goes to the owner, never an automatic design change.
4. **No SIMs**: this PC's network only, with the carrier gap reported plainly and no estimates.

These override phase 4a's setup and phase 4's billing guards and go-live checklist until usage grows. Phase 4 still has its text for the later paid setup; revisit it with the 4a results.
