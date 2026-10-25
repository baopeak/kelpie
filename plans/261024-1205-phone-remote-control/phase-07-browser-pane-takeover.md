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
  - Frames: JPEG q70 / max width 1280 direct; via relay ≤ 2 fps q50 within the phase-4 takeover budget (200 MB/h); only on change; phone ack → `Page.screencastFrameAck`.
  - Input: tap/drag → `Input.dispatchMouseEvent`, scroll → wheel, text → `Input.insertText`, keys → `Input.dispatchKeyEvent`; coordinates from frame metadata (`deviceScaleFactor`, `pageScaleFactor`).
  - **Reuse** the existing `devtools()` helper (`app/src-tauri/src/browser.rs:263-290`) and promote `test_input` (`:299-323`, debug-only today) behind a daemon-issued takeover id; add `add_DevToolsProtocolEventReceived` for frames. [RT-13]
  - **OAuth popups** [RT-13]: today denied and reopened in the same pane (`browser.rs:140-147`), breaking `window.opener`. Use `NewWindowResponse::Create` with a top-level `WebviewWindow`
    on the pane's WebView2 environment (tauri 2.12.4); pane + its popups form a **pane group** — the hold unit; overlay and indicator shown on popup windows too; popup close returns to opener.
  - Permission `control`; first takeover per device needs desktop confirmation (user presence, phase 3); overlay "Controlled from <device> · Stop"; idle auto-stop 10 min; Stop from either side.
  - **Holder lifetime** [RT-12]: holder keyed `(device key, link connection, provider client)`; released synchronously on revoke/downgrade, link close (unless re-bound after the same device's handshake, phase 4), provider disconnect.
  - Takeover input never logged or written to transcripts; any future session-facing browser RPC must refuse while a pane group is held (none exist today in `auth.rs`/`daemon.rs`).
  - Hidden pane: takeover brings the pane's tab to front for its duration if frames stop while hidden.
- Non-functional: tap → visible change ≤ 150 ms p95 LAN, ≤ 400 ms via relay; no UI-thread jank.

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
