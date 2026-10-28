---
title: "Phone remote control plan: zero-setup E2E design and red team"
date: 2026-10-24
summary: "Planned account-free, E2E phone control of Kelpie sessions plus screen takeover; red team caught 5 critical design flaws before any code."
---

# Phone remote control plan: zero-setup E2E design and red team

## What happened
- Researched Orca (stablyai/orca v1.4.239, Expo app, relay.onorca.dev needs an Orca account, NaCl, no screen control), Claude Remote Control, VS Code tunnels, Happy, Chrome Remote Desktop.
- Every zero-setup competitor depends on a vendor relay + account. Kelpie can avoid the account: keys travel in the pairing QR.
- Wrote plan `plans/261024-1205-phone-remote-control/` (9 phases, ~47 d).

## Decisions
- `kelpie-link`: Noise IKpsk1 pairing / IK reconnect (`snow` 0.10, RustCrypto, no ring), same crate in daemon, native app and wasm PWA.
- Untrusted relay: one TS Cloudflare Worker + Durable Object, self-host on `workerd`; random room id; ciphertext only.
- Trust root for relay mode = store-signed Tauri 2 app (PWAs cannot be pinned on iOS); PWA stays LAN, max `write`.
- Sessions as state (line diffs by session id), chat adapters in observer mode, first-wins answers for terminal and chat.
- Push: content-free relay push for the native app; PWA users via existing ntfy deep links. Web Push dropped (needs HTTPS, duplicates ntfy).
- Screen: WebView2 CDP screencast for browser panes; app-spawned `kelpie-screen` (WGC/DXGI + MF H.264 + str0m WebRTC + SendInput; macOS SCK/VT/CGEvent).

## Red team (40 findings → 15 accepted)
- Same-user programs can read the user token (`auth.rs:19-21`) → approval/upgrades now need verified app process (`Role::App`) + OS user verification.
- Default `http://` QR leaked the psk to a swappable page → app-only QR default, code typed on desktop.
- IKpsk2 mixes psk in msg2, so the desktop could not verify the QR secret before prompting → IKpsk1.
- Reusing ChatModel on the phone would re-`initialize` live agents and double-answer permissions → observer mode + daemon first-wins `chat_answer`.
- Hook-driven waits survive input, so `session.answer`'s guard allowed double answers → `answered_wait`.
- Also: handle reuse after restart, mobile.json wipe on schema change, build pipeline order, snow/ring + CSP wasm, notify.rs serial worker.

## Next steps
- Blocked until roadmap phase 12 (uncommitted `notify.rs`, `mobile.rs` in this worktree) is committed.
- Owner decisions: relay/push/TURN accounts and domain; App Store reviewer approach; PWA permission after native app ships.
- Then `/ak:cook` phase 1.

> Historical work record — not durable authority. Prefer docs/specs/ADRs for current decisions.
