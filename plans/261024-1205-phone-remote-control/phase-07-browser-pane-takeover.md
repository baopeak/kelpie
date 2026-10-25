---
phase: 7
title: "Browser pane takeover (CDP screencast) + app provider channel"
status: pending
priority: P2
effort: "4d"
dependencies: [3]
---

# Phase 7: Browser pane takeover

## Overview
The cheapest takeover for the most common blocker — a login, OAuth consent, 2FA or captcha in a Kelpie browser pane — with no OS capture
permission: stream the pane with WebView2 `Page.startScreencast` and drive it with `Input.*`. Windows only (macOS uses phase 9 window capture of
the Kelpie window). This phase also builds the **daemon → app provider channel** that phase 8 reuses. Relay mode needs phase 4.

## Requirements
- Functional
  - **Provider channel** [RT-13]: the daemon has only fire-and-forget `broadcast` today (`daemon.rs:676-682`). Add request/response to the app: request id, 10 s timeout,
    pending map cleaned in `unregister_client`. Only a `Role::App` client (phase 3 peer verification) may register as provider; a second registration is refused and raises a desktop alert.
  - Phone lists controllable panes (title, host) per project; "Take over" from a session or attention card.
  - Frames: JPEG, only on change, rate and quality set by the byte-budget ladder and flow control in "Resilience requirements" below (relay within the phase-4 takeover budget of 200 MB/h).
  - Input: tap/drag → `Input.dispatchMouseEvent`, scroll → wheel, text → `Input.insertText`, keys → `Input.dispatchKeyEvent`; coordinates from frame metadata (`deviceScaleFactor`, `pageScaleFactor`).
  - **Reuse** the existing `devtools()` helper (`app/src-tauri/src/browser.rs:263-290`) and promote `test_input` (`:299-323`, debug-only today) behind a daemon-issued takeover id; add `add_DevToolsProtocolEventReceived` for frames. [RT-13]
  - **OAuth popups** [RT-13]: today denied and reopened in the same pane (`browser.rs:140-147`), breaking `window.opener`. Use `NewWindowResponse::Create` with a top-level `WebviewWindow`
    on the pane's WebView2 environment (tauri 2.12.4); pane + its popups form a **pane group** — the hold unit; overlay and indicator shown on popup windows too; popup close returns to opener.
  - Permission `control`; first takeover per device needs desktop confirmation (user presence, phase 3); overlay "Controlled from <device> · Stop"; idle auto-stop 10 min; Stop from either side.
  - **Holder lifetime** [RT-12]: holder keyed `(device key, link connection, provider client)`; released synchronously on revoke/downgrade, link close (unless re-bound after the same device's handshake, phase 4), provider disconnect.
  - Takeover input never logged or written to transcripts; any future session-facing browser RPC must refuse while a pane group is held (none exist today in `auth.rs`/`daemon.rs`).
  - Hidden pane: takeover brings the pane's tab to front for its duration if frames stop while hidden.
- Non-functional: tap → visible change ≤ 150 ms p95 LAN, ≤ 400 ms via relay; no UI-thread jank.

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
