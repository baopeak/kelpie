---
phase: 2
title: "Phone client app (SolidJS, PWA)"
status: completed
priority: P1
effort: "6d"
dependencies: [1]
---

# Phase 2: Phone client app

## Overview
Replace the vanilla `crates/kelpied/mobile/app.js` with one SolidJS phone client (`app/phone/`) that the daemon serves as an installable
PWA and the native shell bundles in phase 5. Chat reuses the desktop adapters in **observer mode** through a narrow transport.

## Requirements
- Functional
  - **Inbox**: sessions across projects: Needs you (longest wait first) / Working / Done-unread / Idle / Stopped; badge; tap → session.
  - **Terminal session**: text view of phase-1 `screen` frames (monospace, pinch zoom). No xterm.js view — phase 1 carries text rows only (D8). [RT-7]
  - **Chat session**: `ChatController` in observer mode — no `adapter.start()` (`chat-model.ts:313-321`), no automatic replies (`acp-adapter.ts:296,334`), no queue flush,
    no `session.update` (`chat-model.ts:200-208`), explicit delta-apply path instead of `resetState()` on every attach (`chat-model.ts:305-311`); answers via phase-1 `chat_answer`;
    cards resolve on `chat.resolved`. [RT-4]
  - **Prompt card**: buttons from `prompt.keys` using the existing `keyAnswer` mapping (`app/src/lib/attention.ts:42`), sent via `answer`; "stale" shows "already answered". [RT-6]
  - **Composer**: native `<textarea>` (OS IME and dictation), Enter guard for composition, Shift+Enter newline, history, long-paste confirm. Built as a transport-agnostic
    module `app/src/lib/composer-core.ts` shared with roadmap phase 9's desktop composer — whichever lands first creates it. [RT-6]
  - **Key bar**: Esc, Tab, Shift+Tab, Ctrl (sticky one-shot), arrows, PgUp/PgDn, Ctrl+C, Enter; hidden for chat sessions.
  - Reconnect banner, backoff with jitter, resume via `base`/`since_seq`/`daemon_epoch`; pause subscriptions when hidden; `#s=<session>` deep links open the session. [RT-9]
  - Read-only devices — and, from phase 3, PWA links over plain `http://` outside Tailscale — see no composer/keys, with a hint "use the Kelpie app or Tailscale/HTTPS to type". EN + VI from `app/src/locales`; light/dark. <!-- Updated: Validation Session 1 - PWA write only on trusted transports -->
  - Service worker for the app shell only on secure origins (HTTPS/Tailscale); plain `http://` works without it.
- Non-functional: ≤ 250 KB gzip; iOS Safari 16.4+, Android Chrome 120+.

## Resilience requirements — unstable networks and host overload
<!-- Updated: 2026-10-25 - resilience research -->
- **One connection-state enum + always-visible indicator**: `live`, `degraded` (RTT > 800 ms or heartbeat late), `reconnecting (attempt n, next in s)`, `no network`, `sign-in needed` (captive portal), `computer offline (since hh:mm)`, `relay unreachable`, `computer restarted` (epoch change), `computer busy` (load level). Distinct wording for "your connection is slow" vs "the computer is busy".
- **Never block reading**: last state cached (IndexedDB on the PWA, native secure store in phase 5) and painted first (0 ms), dimmed with "as of 14:03:22 (2 min ago)"; data-age chip from `server_ts` (amber > 2 s "updating slowly", red > 5 s "computer not responding").
- **Offline interaction policy** (no hidden queue): decided in fix round 1: `input`, `key`, `answer` and `chat_answer` are allowed in `live`, `degraded` and `busy` (socket open and authenticated; a busy computer must not lock the person out) and in `restarted` only after the resync completed (what the phone follows has been sent again, or 5 s passed); refused in every other state. The reply box stays editable offline (drafts kept locally), only sending is disabled. A drop with an answer in flight ⇒ `unknown`; one automatic retry within 60 s on the same daemon run (safe: first-wins + dedupe; a chat answer is sent again byte for byte, never rebuilt); later ask the user; takeover/screen input refused offline.
- **Conflict messages** from phase-1 answer results (`stale {answered_by, at}`): "Already answered on the desktop 2 min ago".
- **Triggers**: `visibilitychange`, `online`/`offline`, failed heartbeat ⇒ immediate 3 s probe then reconnect, bypassing backoff once per 2 s; backoff full-jitter: first attempt 0–200 ms, then 1 s ×2 up to 30 s; nothing runs in the background.
- **Load-aware**: shows `load.status` reason banners (busy / low memory / hot or battery saver) in EN + VI; sends `focus` (session on screen) and `visibility`; hidden page ⇒ unsubscribe everything except the attention list; rendering rAF-batched, ≤ 10 Hz when hidden or Save-Data (`navigator.connection.saveData`) is on; Save-Data ⇒ subscribe only the visible session.
- Tests: fake-link Playwright/WebDriver: drop link ⇒ "reconnecting" ≤ 1 s after detection, cached cards stay visible, composer disabled; reconnect resumes without reload; answer during drop ⇒ `unknown` then exactly one retry.

## Architecture
- **Separate build** `app/vite.phone.config.ts` (`root: app/phone`, `outDir: app/dist-phone`), script `pnpm build:phone`; `pnpm build` stays desktop-only (Tauri `frontendDist ../dist`, `tauri.conf.json:9-10`); `tsconfig` includes `phone`. [RT-10]
- Transport `app/src/chat/transport.ts` = only `attach(observer)`, `send(line)`, `answer(request_id, line)`, events; desktop wraps `lib/ipc.ts`, phone wraps the phone socket. No generic `daemon()` on the phone. [RT-6]
- `app/phone/src/link.ts`: phase-1 JSON protocol over WS today; phase 3 swaps framing to `kelpie-link` without UI changes.
- **Embedding**: modify the existing `crates/kelpied/build.rs` (keep its ConPTY sideload copy, `build.rs:1-33`): embed `app/dist-phone/` with `cargo:rerun-if-changed=../../app/dist-phone`;
  missing bundle ⇒ placeholder page in dev, **hard error when `KELPIE_REQUIRE_PHONE=1`** (set by `scripts/build-release.sh` and both workflows). [RT-10]
- Build order: `scripts/build-release.sh` and `ci.yml` run `pnpm install && pnpm build:phone` **before** any `cargo build/test -p kelpied`; `release.yml` greps the daemon binary for the placeholder string (like its `KELPIE_UPDATE_ENDPOINT` check). [RT-10]

## Related Code Files
- Create: `app/phone/index.html`, `app/phone/src/{main.tsx,app.tsx,link.ts,inbox.tsx,session-view.tsx,terminal-text.tsx,prompt-card.tsx,composer.tsx,key-bar.tsx,sw.ts}`, `app/vite.phone.config.ts`, `app/src/chat/transport.ts`, `app/src/lib/composer-core.ts` (if roadmap phase 9 has not created it)
- Modify: `crates/kelpied/build.rs`, `app/package.json`, `app/tsconfig.json`, `app/src/chat/chat-model.ts` + adapters (transport, observer mode), `app/src/locales/{en,vi}/*.json`, `crates/kelpied/src/mobile.rs` (serve embedded files), `scripts/build-release.sh`, `.github/workflows/{ci,release}.yml`, `scripts/e2e/mobile.py`, `docs/user-guide.md`
- Delete: `crates/kelpied/mobile/app.js`, `app.css` (after parity)

## Implementation Steps
1. Transport + observer mode in `ChatController`; desktop wiring; desktop chat e2e green.
2. Phone Vite config, build scripts, CI/release order, `build.rs` embedding with guard.
3. Inbox, terminal text view, prompt card, composer core, key bar, link module.
4. Chat view (observer adapters, `chat_answer`, `chat.resolved`).
5. Service worker (secure origins), i18n, themes, deep links.
6. Replace old page; docs; e2e.

## Tests
- Unit (node test runner / vitest): frame apply (diff/full/epoch), composer IME guard, backoff, observer adapter never writes on attach.
- e2e `mobile.py`: existing checks on the new client + inbox order, prompt tap, chat answer resolves on desktop, `#s=` deep link, reconnect resume.
- Desktop chat e2e unchanged; CI check that `cargo build -p kelpied` with `KELPIE_REQUIRE_PHONE=1` fails without the bundle.
- Manual once: iPhone Safari + Android Chrome, Telex (Laban/Gboard) and VNI.

## Perf gate
- First load ≤ 1 s on LAN; 0 requests while hidden; desktop `dist/` size unchanged ±2 %.

## Success Criteria
- [ ] Every terminal and chat session readable and answerable from the phone; one tap for prompts.
- [ ] Vietnamese IME correct on iOS and Android.
- [ ] Desktop chat unchanged; phone never initializes or reconfigures an agent.

## Risk Assessment
- Adapters entangled with desktop store; signal: phone bundle imports `@tauri-apps/*`; response: move behind `transport.ts`.
- Composer collision with roadmap phase 9; response: shared core module, whichever phase lands first owns it.
