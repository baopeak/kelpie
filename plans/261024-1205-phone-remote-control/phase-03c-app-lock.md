---
phase: 3c
title: "App lock (WebAuthn) and daemon risk labels"
status: in-progress
priority: P1
effort: "6d"
dependencies: [3a]
---

# Phase 3c: App lock and risk labels

## State of this phase, 2026-11-01
Code, daemon tests with real signatures, and e2e with real WebAuthn ceremonies through a virtual authenticator in Blink are on `phone/phase3c-app-lock`. **Step 1, the spike on real phones, has not run, and so neither has its kill rule:** no person with phones was at hand, and this run was told to go as far as can be emulated. WebKit has run none of this, so it may yet trigger the kill rule; what a person must check, and what differs on WebKit, is in [the report](../reports/fullstack-developer-261101-1800-phone-phase3c.md). Until they do it stays *not yet verified*.

### Choices the spec left open
- **Idle time default: 5 minutes,** with 1, 5, 15, 30 and 60 on offer. **No grace on leaving.** Both are for the owner to change: see the report.
- **Plain `http://`, D17 as proposed:** read-only, no PIN, closed by a switch, on by default. Still the owner's to confirm. It also covers a plain `http://` page on a Tailscale address: such a page has no secure context either, so while the lock is on it may read and no more, though `transport_cap` lets it type with the lock off.
- **A plain connection from this machine itself is not locked** (`LockPath::Local`): nothing stands between but this user's own programs, and the e2e scripts' plain pages and protocol clients are such connections.
- **A counter that does not grow is refused unless it stays 0,** which is the WebAuthn rule and a little stricter than "a decrease refuses".
- **The unlock is a button.** A tap on it starts the ceremony: iOS may refuse a ceremony with no tap, and a refused one costs the person more than a tap does. The re-check is a second tap on a sheet for the same reason. A real iPhone may show that one tap can go.
- **The settings carry their own tag** (`LockOptions.mac`): a program that edits the file must not be able to turn the lock off, which is this plan's own criterion.
- **`hello` keeps `sessions: []` and `projects: {}` while locked**, empty rather than absent: a shell cached from before the lock then gets a list with nothing in it and `locked` refusals, and not a page that fails to start. It cannot be made to reload, having no code for that. The next load gets the new shell, as the worker's install takes over at once.
- **The window belongs to the page that opened it**, by the owner's decision: a secret given at unlock and held in the page's memory alone, so a copied key inside the window stays locked. Stated in `docs/architecture.md`.
- **Files beyond the list:** `mobile/lock.rs` (the lock's own logic, out of a file of 7,000 lines), `mobile/tests/lock.rs`, `app/phone/src/command-text.ts` and `command-block.tsx` (the card shared by terminal and chat).

## Overview
User decision 2026-10-30: lock the app, not each command. Opening the phone app requires biometric/device verification (WebAuthn platform authenticator, `userVerification: "required"`). Once unlocked the device has full rights for its granted permission. It re-locks on leaving the app and after a configurable idle time. The daemon still shows the full command and a daemon-computed risk label, as information only. An optional "ask Face ID again for dangerous commands" toggle is off by default. Pairing and approving devices stay on the desktop.

The daemon enforces the lock, not the page: a copied Noise key alone reaches nothing.

## Data flow
```
enrol: navigator.credentials.create (rp = <fqdn>) → verb lock_enrol {credId, spki, authData, clientData} over Noise → verified → device record (MAC'd)
open:  IK handshake → hello {locked: true, challenge} (no sessions, no projects) → navigator.credentials.get → verb lock_unlock {assertion}
       daemon verifies → unlocked_until = now + idle → fresh hello → normal verbs
leave: visibilitychange hidden → verb lock_now; idle expiry on the daemon → `locked` → follows stopped, grants stopped
```

## Note from the Web Push phase
`push_info`, `push_sub`, `push_unsub` and `push_test` must be refused while locked. `push_test` does not pass through `Conn::run`: `Conn::reply` hands it to `push_test` first, so a gate placed in `run` would miss it.

## Requirements
- Functional
  - **Verification** (`mobile/webauthn.rs`, ES256 only, `p256` crate; `webauthn-rs` not used, it brings OpenSSL [U]): RP ID hash = SHA-256(fqdn); flags UP and UV set; `clientDataJSON.type`, `challenge` (single use, 60 s), `origin` exact; signature over authData ‖ SHA-256(clientData); sign count may stay 0, a decrease refuses. Enrol takes `getPublicKey()` SPKI, attestation `none`, so no CBOR parser.
  - **Enrol**: first open after pairing on a secure origin, when the lock is on. A device has one credential. Replacing it needs the desktop ("Reset this phone's lock", `Role::App` + presence, opens a 5-minute window), so a stolen Noise key cannot enrol its own passkey.
  - **Choke point**: the per-message permission check in `mobile.rs` also checks lock state. While locked only `hb`, `ack`, `lock_*` pass; anything else gets `locked`. `hello` withholds `sessions` and `projects`. `load.status` and `addrs` still flow (no content).
  - **Re-lock**: `lock_now` on hidden; daemon idle timer from the last user action (phone sends `lock_touch` at most every 30 s). On lock: `followed` tasks and chat watches stop, control grants stop (`stop_grants`), `locked` sent.
  - **Flaky network**: a reconnect by the same device inside the unlocked window does not ask again, so a Wi-Fi ↔ 4G flap is not a Face ID prompt. The window is in memory, per device, and gone on daemon restart.
  - **Phone**: lock screen before any cached paint; on hidden, an opaque overlay at once (app-switcher snapshot [U]); cache and kept messages stay in IndexedDB unencrypted (residual, documented); deny or cancel leaves it locked, retry button; a push tap (3b) lands here first.
  - **Settings** (app-only, `mobile.set_options` splits off `mobile.set_lock`, in `APP_ONLY`, `auth.rs:56`): lock on/off (default on), idle timeout (default in open questions), dangerous re-check (default off). Weakening needs presence. Applied on the next message.
  - **Plain `http://` LAN** (no secure context, no WebAuthn): **proposed fallback: read-only until HTTPS, no PIN**. `transport_cap` already caps it at `read`; a PIN typed into a page that can be swapped on the network gives comfort, not protection. A separate switch "Allow reading over plain HTTP" lets the user close it. Decision point recorded in the plan and report.
  - **Risk labels** (`mobile/risk.rs`, pure, table-tested): input = tool + full command + cwd + worktree. Tags: `network`, `delete`, `outside_worktree`, `git_push`, `secret_access`, `install`, `sudo`, `pipe_to_shell`, `edits_config` (hooks, CI, agent config). Level: `low` (read-only known tool, or edit inside the worktree), `medium`, `dangerous`, `unknown`. Unparseable or unknown is never `low`. Agent text is never an input to the level.
  - **Display**: prompt and permission cards show the daemon's label first, then the full command: monospaced, no truncation, non-printables, bidi and zero-width characters escaped, scroll to the end before Approve for long commands. Label is informational unless the re-check toggle is on.
  - **Delivery of labels**: terminal prompts on the `sync` frame's waiting info; chat requests on a new `chat_request_info {id, request_id, command, risk}` from `chat.rs`, which holds the pending request (`chat.rs:1301`). Exact fields for each adapter dialect to be traced in step 3 [UNVERIFIED].
  - **Dangerous re-check** (toggle, default off): an `answer` / `chat_answer` approving a `dangerous` request needs an assertion whose challenge = SHA-256(request id ‖ SHA-256(command) ‖ expiry); deny never needs one; refusal `recheck`. Raw `input` / `key` cannot be classified and are not covered; the toggle's text says so plainly.
- Non-functional: unlock verify ≤ 5 ms; no timer per device when none is unlocked; lock screen first paint ≤ 300 ms.

## Backwards compatibility
Device records gain optional credential fields (serde default, inside the MAC). Devices without a credential are prompted to enrol on the next secure open. `DAEMON_SECURITY_LEVEL` 3 (a level-2 daemon has no lock), `API_LEVEL` 25; phone bundle ships with the daemon, so both sides match. A shell cached by the service worker that predates the lock gets `locked` refusals and reloads.

## Resilience requirements
- Assertion lost with the connection: new challenge after the next hello; no retry of a used challenge.
- Passkey gone (Face ID reset, phone migrated): "Reset this phone's lock on the computer"; never a bypass.
- `lock_*` verbs and the choke point run on the phase-1 priority path, never shed at L3; `grant_alive` stops input < 1 s on lock.
- Daemon restart: every device locked; phone paints the lock screen, not an error.

## Related Code Files
- Create: `crates/kelpied/src/mobile/webauthn.rs`, `crates/kelpied/src/mobile/risk.rs`, `app/phone/src/lock.ts`, `app/phone/src/lock-screen.tsx`
- Modify: `crates/kelpied/src/mobile.rs`, `mobile/verbs.rs`, `mobile/store.rs`, `mobile/sync.rs`, `crates/kelpied/src/chat.rs`, `crates/kelpied/src/auth.rs`, `crates/kelpied/src/daemon.rs`, `crates/kelpie-proto/src/lib.rs`, `app/phone/src/{app,link,prompt-card,chat-view,cache,conn-text}.ts*`, `app/src/components/settings/mobile-settings.tsx`, `app/src/lib/types.ts`, locales, `docs/architecture.md`, `docs/user-guide.md`, `README.md`

## Implementation Steps
1. **Spike (½ d, real phones)**: `credentials.create/get` with UV inside iOS standalone and Android installed; `getPublicKey()` available; behaviour after Face ID failure. **Kill rule**: WebAuthn unusable in iOS standalone ⇒ stop, report to the user with options (Safari tab; PIN; read-only). Do not improvise.
2. `webauthn.rs` with recorded fixtures from step 1; enrol/unlock verbs; choke point; hello gating.
3. `risk.rs` and label delivery for terminal, then each chat adapter.
4. Re-lock rules, idle timer, reconnect window.
5. Phone lock screen, overlay, card display.
6. Re-check toggle. Settings methods. Docs. Level bumps.

## Tests
- `cargo test -p kelpied`: tampered authData, wrong origin, wrong RP hash, reused or expired challenge, no UV flag, lower sign count ⇒ refused; every verb but the allowed set refused while locked (extend `every_verb_is_refused_below_its_permission`, and `the_table_is_closed`); hello holds no sessions while locked; idle expiry stops follows and grants; reconnect inside the window stays unlocked, after it locks; `Role::User` refused on `mobile.set_lock`; replacing a credential refused without the desktop window.
- Risk table: `rm -rf`, `curl | sh`, `git push`, `sudo`, `npm install`, path outside worktree, bidi-padded command, 50-subcommand chain, unknown tool ⇒ never `low`.
- `pnpm exec vitest run`: lock state machine; no cached paint before unlock; display escapes hostile characters.
- e2e `mobile.py` with a Chromium virtual authenticator (WebDriver), isolated daemon: enrol, unlock, hide ⇒ locked, idle ⇒ locked, read refused while locked.
- Manual: iPhone Face ID and Android biometric/passcode, installed app, per release.

## Perf gate
Choke-point check adds ≤ 1 µs per message; none of this runs while phone access is off.

## Success Criteria
- [ ] Holding the paired phone and its Noise key without passing verification shows no session data, confirmed at the daemon.
- [ ] Leaving the app and coming back asks again; a network flap in the foreground does not.
- [ ] Every approval card shows a daemon label and the full command, including hostile strings.
- [ ] Re-check off: no prompt on approvals. On: a dangerous approval without a fresh assertion is refused by the daemon.

## Risk Assessment
| Risk | L × I | Mitigation |
|---|---|---|
| WebAuthn broken in iOS standalone | M × H | Step 1 kill rule, user decision |
| Prompt fatigue on every return to the app | M × M | Idle and grace settings; user chooses |
| Malicious page can still misreport what it shows | L × H | Page comes from the daemon; only `*.ts.net` over TLS; label is daemon-computed but rendered by the page (documented limit) |
| Risk parser gives false confidence | M × H | `unknown` never low; label is information; agents' own guards unchanged |
| Lock off leaves a lost phone open | M × H | Default on; revoke and kill switch; dormant expiry |

**Rollback**: lock toggle off on the desktop; revert: credential fields ignored; security level stays higher only if released.
