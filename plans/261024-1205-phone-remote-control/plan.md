---
title: "Phone remote control: sessions anywhere, screen when needed"
description: "Control every Kelpie session from a phone with zero networking setup and end-to-end encryption — LAN or anywhere, no account — plus browser-pane and desktop/IDE screen takeover."
status: pending
priority: P1
effort: "~47d"
tags: [mobile, remote, e2e, noise, relay, webrtc, screen, security]
created: 2026-10-24
blockedBy: [261023-0048-multi-agent-ide-roadmap]
blocks: []
---

# Phone remote control

## Overview
Today the phone page (`crates/kelpied/mobile/app.js`, 258 lines) polls a session's text every 1.2 s, sends fixed keys, cannot answer chat
sessions, ignores the daemon's prompt detection and stale-answer guard, and only works on one LAN/Tailscale IP. This plan turns the phone
into a full Kelpie controller that **works anywhere with one QR scan, no account, no Tailscale**, keeps everything end-to-end encrypted, and
adds **screen takeover** when an agent is stuck on a login, a GUI or a dialog.

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
[screen spikes](../reports/researcher-261024-1820-screen-control-gaps-spike.md)

## Brainstorm contract
- **Outcome**: from a phone, anywhere, the user sees every session across agents (who needs them first), reads and answers terminal and chat
  sessions with one tap or a real keyboard (Telex/VNI safe), gets a push when one needs them, and can take over a browser pane or the
  desktop / an IDE window to type a password, pass 2FA or click a dialog.
- **Constraints**: zero networking knowledge; no Kelpie account; nothing readable leaves the machine (relay sees ciphertext only); off by default;
  daemon stays per-user and per-user-installed; MIT-compatible dependencies; EN + VI; Windows first, macOS parity; no work or sockets when off;
  e2e never touches the real daemon (`KELPIE_ENDPOINT` in the test's own process).
- **Non-goals**: UAC secure desktop, lock/login screen, elevated windows (need a SYSTEM service — reported clearly instead); audio; file transfer; multi-user sharing; any Kelpie cloud storage.

## Why this beats Orca
Orca (v1.4.239, mobile 0.0.58 beta, Expo app): LAN `ws://` or `relay.onorca.dev` **with an Orca account on both ends**, NaCl app-layer encryption
(forward secrecy unconfirmed), cloud push gateway, **no screen control**. Kelpie: no account (keys in the QR), Noise with forward secrecy, self-hostable
relay, user-presence-gated approvals, one inbox across 23 agents, browser-pane and desktop/IDE takeover, and the PWA from the daemon itself on LAN (no store/version skew there).

## Architecture decisions (final, after red team)
| # | Decision | Why |
|---|---|---|
| D1 | **One E2E protocol `kelpie-link`**: Noise `IKpsk1` pairing (psk checked in msg1), `IK` reconnect, via `snow` 0.10 (RustCrypto resolver, no ring), over any pipe (LAN WS, relay WS) | One crate for daemon, native app, wasm PWA; desktop verifies the QR secret before any prompt |
| D2 | **Pair by QR keys, no accounts**; phone shows a 6-digit code the user **types** on the desktop; default QR is app-only (`kelpie://pair`); browser pairing is an explicit separate action | Beats account-based Orca/Claude Remote Control; a swapped `http://` page cannot ride the default path |
| D3 | **Connect race** (Rust, `kelpie-link::client`): all bound LAN/Tailscale addresses ∥ relay; first IK handshake wins; migrate to direct | Zero setup without losing LAN latency; daemon binds every usable address, not one |
| D4 | **Untrusted relay**, opt-in: one TS Worker + Durable Object, self-host on `workerd`; random `room_id`; sees ciphertext, IPs, timing, sizes, room id | Every zero-setup competitor needs a relay; no derivable room ids |
| D5 | **Trust root = signed native app** for relay mode; PWA stays LAN/Tailscale, `write` only over TLS/Tailscale (plain `http://` capped at `read`), no relay token, no `control` | PWAs cannot be pinned on iOS; Isolated Web Apps are Chrome/ChromeOS-only |
| D6 | **One phone client** (`app/phone`, own Vite config), chat adapters reused in **observer mode** over a narrow transport | No three UIs; phone never initializes or reconfigures an agent |
| D7 | **Native shell = Tauri 2 mobile**, keys + biometric gate enforced in Rust | Shares Rust crates; Capacitor documented fallback |
| D8 | **Sessions travel as state**: line-diff frames by session id with per-device resume; first-wins answers for terminal and chat prompts | 4G bandwidth, Telex-safe input, no double answers |
| D9 | **Push**: native app via content-free relay push (own gateway credential); PWA users via existing ntfy deep links; no Web Push | No content to Apple/Google/relay; no HTTPS requirement; no duplicate channel |
| D10 | **Screen, two tiers**: browser pane via WebView2 CDP screencast (Windows); desktop/window via app-spawned `kelpie-screen` (WGC/DXGI + MF H.264 + `str0m` WebRTC + `SendInput`; macOS SCK + VideoToolbox + `CGEventPost`). Desktop video never goes through the relay; browser-takeover frames may, at ≤ 2 fps within their own budget | P2P UDP for video; cheap takeover for logins |
| D11 | **Trust boundary**: sensitive settings (approve, upgrade to write/control, always-allow, relay on, reset) need the verified Kelpie app process (`Role::App`) + OS user verification; per-device `read` < `write` < `control`; grants keyed to device + connection + provider and stopped on revoke | Same-user programs can read the user token (`docs/architecture.md:72`) |

```
Phone (PWA on LAN | native app anywhere) ── one SolidJS client (app/phone)
   │  kelpie-link (Noise) frames
   ├── direct: ws(s)://<every usable LAN/Tailscale IP>:4817/link ─┐
   └── relay:  wss://<relay>/v1/phone  (ciphertext only) ─────────┤
                                                                  ▼
          kelpied: verbs (read/write/control), devices v2, state-sync, chat observer + first-wins answers,
                   notify destinations (ntfy, Telegram, webhook, relay push), takeover broker
                     │ provider channel (Role::App, request/response)
                     ▼
          Kelpie app: browser panes (CDP screencast) · spawns kelpie-screen ══ WebRTC (UDP P2P/TURN) ══▶ phone
```

## Phases
| # | Phase | Priority | Effort | Depends on | Status |
|---|---|---|---|---|---|
| 1 | [Phone protocol v2: state-sync, prompts, safe answers](phase-01-phone-protocol-v2.md) | P1 | 4d | roadmap phase 12 | pending |
| 2 | [Phone client app (SolidJS, PWA)](phase-02-phone-client-app.md) | P1 | 6d | 1 | pending |
| 3 | [kelpie-link: E2E pairing, trusted approval, device store v2](phase-03-kelpie-link-e2e.md) | P1 | 6d | 2 | pending |
| 4 | [Relay: connect from anywhere](phase-04-relay-connect-anywhere.md) | P1 | 4d | 3 | pending |
| 5 | [Native mobile shell (Tauri 2 iOS/Android)](phase-05-native-mobile-shell.md) | P1 | 6d | 3, 4 | pending |
| 6 | [Push notifications](phase-06-push-notifications.md) | P1 | 4d | 3, 5, roadmap phase 12 | pending |
| 7 | [Browser pane takeover + app provider channel](phase-07-browser-pane-takeover.md) | P2 | 4d | 3 (4 for relay mode) | pending |
| 8 | [Desktop / window screen control — Windows](phase-08-desktop-screen-windows.md) | P2 | 8d | 3, 4, 5, 7 | pending |
| 9 | [Desktop / window screen control — macOS](phase-09-desktop-screen-macos.md) | P2 | 5d | 7, 8 | pending |

Order: daily value (1–2), security foundation (3) before reach (4–6), screen last with spike gates (7–9). Phase 7 may run beside 4–6 but
phases 4, 6, 7 and 8 all touch `crates/kelpied/src/mobile.rs` and `daemon.rs`: work them sequentially or with one owner of those files.

## Security model (applies to every phase)
- Off by default; no listener, outbound socket, timer or helper when off.
- **Boundary stated honestly**: any same-user program can read the user token. Sensitive mobile settings therefore require `Role::App` (verified Kelpie process) + OS user verification;
  residual risk = code injection into the app process (same user), documented.
- Devices = Noise static public keys in `mobile.v2.json` (MAC'd); desktop private key in the OS keyring; device add/permission change ⇒ desktop toast + notice to other phones.
- Permissions: new device `read`; PWA `write` only over TLS/Tailscale, `read` over plain `http://`; `control` native only. Every message checked against the live record; revoke/downgrade stops control grants before closing sockets.
- Phone verbs are a closed table; no generic daemon-method proxy; phone typing goes through `typed_text` and the agent's Enter delay.
- Relay: no accounts, random room id, per-device relay tokens (hashes only), caps, no payload logging, persisted pending revocations; self-host documented.
- Push payloads carry no session text; gateway sees push tokens + event timing (documented).
- Screen control: user-presence confirmation per device, visible indicator, stop hotkey, idle auto-stop 10 min, audit start/stop (never keystrokes), revoke ⇒ input stops < 1 s.

## Success Criteria
- [ ] Fresh phone + desktop, relay on: install app → scan QR → type code on desktop + OS verification → sessions in < 30 s on 4G, no account, no network settings.
- [ ] Same on LAN via PWA (browser pairing); direct path preferred automatically when both exist.
- [ ] Terminal prompt and chat permission answered with one tap; first answer wins across devices; Telex/VNI never sends mid-composition.
- [ ] A program holding only the user token cannot add a device, raise permissions or enable the relay.
- [ ] Locked phone (native app) notified ≤ 10 s; payload has no session text; PWA users notified via ntfy deep link.
- [ ] Relay operator (logs + storage) cannot read any session content (test harness inspects relay traffic).
- [ ] Browser-pane OAuth/2FA login (popups included) completed from the phone on Windows.
- [ ] Desktop/IDE window viewed and controlled: ≤ 150 ms p95 LAN, ≤ 250 ms direct P2P, ≤ 400 ms TURN on 4G; secure-desktop/elevated states reported.
- [ ] Feature off: no sockets, no helper, no measurable CPU (perf gate).
- [ ] EN + VI; user guide, architecture, README privacy updated; e2e green on Windows and macOS CI.

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
