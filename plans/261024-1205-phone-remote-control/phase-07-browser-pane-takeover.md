---
phase: 7
title: "Browser pane takeover (CDP screencast) + app provider channel"
status: completed
priority: P2
effort: "5d"
dependencies: [3, 3a, 3c]
---

# Phase 7: Browser pane takeover

> **Updated 2026-10-30 (path D).** Runs over Tailscale HTTPS (or LAN HTTPS) from the installed PWA. **Relay-mode takeover is deferred**; the relay figures below stand as reference and do not gate this phase. API level: next free, expected 25.
>
> **`control` for the PWA — owner decision, final (2026-11-05):** a PWA device may hold `control` only when it paired over HTTPS/Tailscale (`secure_origin`), its app lock is enrolled and on, and it was granted on the desktop with OS verification (presence). Other browser devices stay capped at `write`. An app-lock auto-lock releases the hold like a revoke. Relay-mode takeover stays deferred: takeover is refused over the relay. API level **28** (26 is 3e, 27 is phase 4).

## Execution state — 2026-11-05

**Built and tested 2026-11-06** (branch `phone/phase7-takeover`, rebased onto phase 4's `832e966`): `cargo test -p kelpie-proto -p kelpied -p kelpie -p kelpie-link` all green, clippy clean, pnpm typecheck/test/i18n green, `scripts/e2e/takeover.py` 39/0 and `browser.py` 37/0 on a debug app with an isolated daemon. Report: `plans/reports/fullstack-developer-261105-1900-phone-phase7.md`.

**Spike results (Windows 10, WebView2 156, debug app, `takeover.py --spike`):**
- Frames arrive without `Page.enable`; clicks are `isTrusted`. A login page (948 × 784 CSS px) at q70/1280: JPEG ~12.8 KB, newest frame 11 ms old p50 / 17 ms p95 on loopback, ~8 fps with a CSS animation.
- Tap → the page's click handler: 23 ms p50 / 58 ms p95 (earlier runs 26/90 and 51/128), inside the 150 ms budget before the frame back.
- `NewWindowResponse::Create` inside the handler works, no deadlock; `window.opener` and `postMessage` hold. A new popup sends no screencast frames (its window is behind the main one, and its DevTools state starts over on load): the stream restarts on its load, and a 1 s silent stream falls back to `Page.captureScreenshot` every 500 ms. `window.close()` left the window open (wry destroys only the view): now closed with it.
- A pane hidden behind another tab sends **0** frames in 4 s: kill rule applied, the held pane's tab is brought back to the front (28 frames in the same 4 s after the fix), plus the capture fallback.
- WebView2 CPU on that page: 34 % of a core without a stream, 85–107 % with it at L0. **Not measured:** the L2 cap's CPU cut, window-thread time per frame, coordinates under browser zoom, JPEG sizes at q40–q55 (the ladder did not step down on loopback).

**Owner answers (2026-11-05), applied:**
1. The desktop grant of `control`, with OS verification, is the first-takeover confirmation: no extra prompt.
2. Turning the app lock off lowers every browser device's `control` to `write` in its record; getting it back takes a new grant on the desktop with verification (`Mobile::set_lock`, test `turning_the_lock_off_takes_a_pages_control_back_to_typing_until_it_is_granted_again`).
3. A `window.open` with a size or position becomes its own window; every other new window opens in the pane.
4. Gestures: a tap clicks, a drag scrolls, a 400 ms hold then a drag drags, pinch is not passed through.

**Independent review (2026-11-06, `plans/reports/code-reviewer-261106-1645-phone-phase7-review.md`), applied, still unbuilt:** command registration by `browser::takeover::…` path; first finger only, a second gives the gesture up (no pinch); one ordered app-side queue for input and text; window ids on frames and input; app-side TTL and a 64-item bound, 60 input messages a second at the broker; a failed or timed-out start tells the app to stop and the app refuses it later; no wake loop while paused, a `paused` heartbeat and decode failures acknowledged; the same app process re-registering takes over without an alert, release before the old connection drains, registration retried; an app record from a browser held to the page rule; reservations against concurrent starts; same-connection rebind; button released on stop and retarget; drag released outside the picture at the last point; `control_held` re-read by the hold; dropped requests forgotten; own hold not listed as taken; a non-app re-auth gives the role up. Left: popups still have only a title (no overlay or Stop of their own), the pane is brought to the front at start only (a spike item), no byte budget (relay takeover deferred).

**Research (docs and the crates' sources, tauri 2.12.4 / tauri-runtime 2.12.2 / wry 0.57 / webview2-com 0.39.1), design chosen from it:**
- Popups: `WebviewBuilder::on_new_window` may return `NewWindowResponse::Create { window }`; on Windows the new webview must use the opener's environment, which `WebviewWindowBuilder::window_features(features)` sets (`with_environment(features.opener().environment)`). The docs' own example builds the window inside the handler. Only a `window.open` with a size or position (`NewWindowFeatures::size()/position()`, an OAuth window) is made a window; any other new window still opens in the pane, which keeps `browser.py`'s target=_blank check as it was.
- Frames: `ICoreWebView2::GetDevToolsProtocolEventReceiver("Page.screencastFrame")` + `add_DevToolsProtocolEventReceived` (`DevToolsProtocolEventReceivedEventHandler`, `ParameterObjectAsJson`), attached for a held pane only and removed with its token; `Page.startScreencast {format: jpeg, quality, maxWidth, maxHeight, everyNthFrame}` and `Page.screencastFrameAck {sessionId}`, sent without waiting. The handler only hands the JSON to a task (window thread kept free).
- Input reuses `devtools()` (`Input.dispatchMouseEvent`, `Input.dispatchKeyEvent`, `Input.insertText`) behind the daemon's takeover id; `browser_test_input` stays as it was, debug and e2e only.

**Spike checks still owed (need a built app):** frames arrive and `isTrusted` holds on a real login page; tap → visible change ≤ 150 ms p95 on LAN/Tailscale; frames while the pane is hidden or covered (the window brings its tab to the front at start; a dialog over it may still stop them); `Create` from inside the handler works (no deadlock) and `window.opener`/`postMessage` survive; coordinates under page zoom and DPI (`deviceWidth`/`pageScaleFactor`); JPEG sizes q40–q70 at 800–1280 px for text-heavy pages; `msedgewebview2` CPU with and without a screencast, and the L2 cap cutting it ≥ 50 %; `Page.startScreencast` without `Page.enable`; ≤ 2 ms window-thread time per frame.

## Overview
The cheapest takeover for the most common blocker — a login, OAuth consent, 2FA or captcha in a Kelpie browser pane — with no OS capture
permission: stream the pane with WebView2 `Page.startScreencast` and drive it with `Input.*`. Windows only (macOS uses phase 9 window capture of
the Kelpie window). This phase also builds the **daemon → app provider channel** that phase 8 reuses.

## Requirements
- Functional
  - **Provider channel** [RT-13]: the daemon has only fire-and-forget `broadcast` today (`daemon.rs:676-682`). Add request/response to the app: request id, 10 s timeout,
    pending map cleaned in `unregister_client`. Only a `Role::App` client (phase 3 peer verification) may register as provider; a second registration is refused and raises a desktop alert.
  - Phone lists controllable panes (title, host) per project; "Take over" from a session or attention card.
  - Frames: JPEG, only on change, rate and quality set by the byte-budget ladder and flow control in "Resilience requirements" below. The phase-4 takeover budget (200 MB/h) applies only if relay-mode takeover returns (deferred).
  - Input: tap/drag → `Input.dispatchMouseEvent`, scroll → wheel, text → `Input.insertText`, keys → `Input.dispatchKeyEvent`; coordinates from frame metadata (`deviceScaleFactor`, `pageScaleFactor`).
  - **Reuse** the existing `devtools()` helper (`app/src-tauri/src/browser.rs:263-290`) and promote `test_input` (`:299-323`, debug-only today) behind a daemon-issued takeover id; add `add_DevToolsProtocolEventReceived` for frames. [RT-13]
  - **OAuth popups** [RT-13]: today denied and reopened in the same pane (`browser.rs:140-147`), breaking `window.opener`. Use `NewWindowResponse::Create` with a top-level `WebviewWindow`
    on the pane's WebView2 environment (tauri 2.12.4); pane + its popups form a **pane group** — the hold unit; overlay and indicator shown on popup windows too; popup close returns to opener.
  - Permission `control`; first takeover per device needs desktop confirmation (user presence, phase 3); overlay "Controlled from <device> · Stop"; idle auto-stop 10 min; Stop from either side. App lock (3c): an auto-lock releases the hold like a revoke.
  - **Holder lifetime** [RT-12]: holder keyed `(device key, link connection, provider client)`; released synchronously on revoke/downgrade, lock, link close (unless re-bound after the same device's handshake), provider disconnect.
  - Takeover input never logged or written to transcripts; any future session-facing browser RPC must refuse while a pane group is held (none exist today in `auth.rs`/`daemon.rs`).
  - Hidden pane: takeover brings the pane's tab to front for its duration if frames stop while hidden.
- Non-functional: tap → visible change ≤ 150 ms p95 LAN or direct Tailscale; no UI-thread jank. (≤ 400 ms via relay stands only if relay-mode takeover returns.)

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research; supersedes the "≤ 2 fps q50" relay figure above -->
- **Flow control**: ack Chrome's `screencastFrameAck` as soon as a frame is in the broker; the broker holds exactly **one** pending frame (newest wins) and sends when the phone has credit (window 2 unacked frames; phone acks `{seq}` after decode). Never queue frames.
- **Byte-budget ladder, not fps**: driven by ack RTT (EWMA) and a token bucket — relay 55 KB/s average (= the 200 MB/h budget), burst 500 KB. Direct: q70/1280 → q55/1024 → q45/800, fps 10 → 5 → 2; fps drops first, then scale, quality floor q40. Down when ack RTT p50 > 400 ms over 3 frames or the bucket is empty; up when < 150 ms over 10 frames + 5 s dwell. Load levels add caps: L1 q60/1280 ≤ 8 fps, L2 q50/960 ≤ 4 fps, L3 q40/800 ≤ 2 fps; **new takeovers refused at L2+** (`host_busy`).
- **Input TTL 1 s**: clicks/keys/scroll older than 1 s are dropped (a tap delivered after a gap must never click a changed page); mouse moves coalesced; `Input.insertText` (passwords) has an explicit per-op ack shown as sent / not sent and is never retried automatically. No input while not `live`.
- **Freeze detection**: broker sends a 1 Hz `fhb {seq, t}`; 3 s without ⇒ canvas dimmed "connection lost", input disabled; reconnect ⇒ a fresh frame (restart screencast or `Page.captureScreenshot`), never a replay. No phone ack for 2 s ⇒ pause the screencast.
- **Hold re-bind window 15 s**: a link reconnect by the same device (after its handshake) within 15 s keeps the hold; beyond that the hold is released and the overlay removed.
- Spike additions: measure real JPEG sizes for text-heavy pages at q40–q70 / 800–1280 px (may force a lower relay fps) and `msedgewebview2` CPU with/without screencast; L2 caps must cut that CPU ≥ 50 % vs L0 on a busy page.
- Tests (TCP fault proxy, 300 ms RTT, 256 kbps): fps adapts, bytes/s ≤ bucket, no growing queue, newest-frame latency ≤ 2× RTT + encode; freeze link 5 s while tapping ⇒ zero stale clicks executed.

## Architecture
```
phone ──link──▶ kelpied takeover broker ──provider channel (Role::App, req/resp)──▶ app browser.rs (devtools())
          ◀──────────────── frames (binary chunks) ◀──────────── Page.screencastFrame events
```

## Related Code Files
- Create: `crates/kelpied/src/takeover.rs`, `crates/kelpied/src/provider.rs`, `app/src/lib/takeover.ts`, `app/phone/src/pane-takeover.tsx`
- Modify: `app/src-tauri/src/browser.rs`, `app/src-tauri/src/lib.rs`, `crates/kelpied/src/daemon.rs` (provider registration, `unregister_client` cleanup, `API_LEVEL`), `crates/kelpied/src/auth.rs`, `crates/kelpied/src/mobile/verbs.rs` (control verbs), `app/src/components/*browser*` (overlay), locales, docs

## Implementation Steps
1. Spike (½ d): screencast + event receiver + insertText on a real login page; fps, latency, `isTrusted`, hidden pane, popup as top-level window. Kill: popups impossible ⇒ ship without popup support and document; no frames while hidden ⇒ keep pane visible during takeover.
2. Provider channel with timeouts/cleanup; `Role::App` registration.
3. Broker (pane groups, holders, idle stop) + control verbs.
4. App: promote devtools/test_input, frames, popup windows, overlay.
5. Phone UI: pane picker, canvas, gestures, keyboard, Stop. Docs + e2e.

## Tests
- `cargo test`: provider timeout + cleanup on disconnect; second provider refused; `Role::User` provider refused; holder released on revoke within 1 s; idle stop; chunk limits.
- e2e `scripts/e2e/takeover.py` (Windows, isolated daemon): local page with form + `window.open` OAuth mock; phone-emulated login; page sees `isTrusted`; Stop both sides; read/write devices refused; `browser.py` stays green (design-mode picker path `browser.rs:141`).

## Perf gate
- No CDP subscription/timers without an active takeover; ≤ 2 ms main-thread per frame.

## Success Criteria
- [ ] Real OAuth/2FA login completed from the phone in a browser pane, popups included.
- [ ] Only the verified Kelpie app can provide panes; revoke stops control at once.

## Risk Assessment
- WebView2 screencast quirks (DPI, hidden panes); response: spike kill rules.
- Popup environment mismatch makes `Create` fail; response: same-environment creation per tauri docs, else documented limitation.
