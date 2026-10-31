# Phase 3 report: kelpie-link (E2E pairing, trusted approval, device store v2)

Branch `phone/phase3-link`, stacked on d6e5883. Nothing staged or committed. API level 19, `DAEMON_SECURITY_LEVEL` 2.

**This is a hand-over checkpoint, not a finished phase.** The daemon, the crate and the phone's crypto are done and unit-tested. The desktop UI is partly done. e2e scripts, docs and CI have not been touched, so **every phone e2e script currently fails**: `phone_support.py` still speaks `/ws` and tokens.

Read first: `plans/261024-1205-phone-remote-control/phase-03-kelpie-link-e2e.md` (spec), then `plans/reports/researcher-261030-1500-remote-control-security.md` sections 1, 2, 5, 8 and `plans/reports/research-261030-1600-relay-speed-reliability-security-synthesis.md` section 3 (the two added hardening lists).

## What was actually run on the tree as it stands

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm i18n:check` | **fails: 47 keys missing** (expected: `vi/mobile.json` not written yet, step 3) |
| `cargo check -p kelpied` | clean, no warnings |
| `cargo check -p kelpie` (first time this crate was compiled since `user_presence.rs`) | **fails, 2 errors, both in `user_presence.rs`:** line 81 `windows::Foundation::IAsyncOperation` does not exist (the type lives in the `windows-future` crate); line 76 `IAsyncOperation` has no method `get`. More errors may appear once these are fixed |
| `cargo test -p kelpie-link` | 12 / 12 (vectors regenerated with `KELPIE_LINK_WRITE_VECTORS=1`) |
| `cargo test -p kelpied -- mobile auth peer_identity choices` | 132 pass, 1 ignored. **Full `cargo test -p kelpied` not run since these changes.** `chat::` last ran in a mid-way run, before the final fixes |
| `pnpm exec vitest run phone/src/noise.test.ts phone/src/link.test.ts` | 45 / 45. All of `phone/src` was 111 / 111 before the mac1 and rekey change. **Full `pnpm test` not run** |
| `python scripts/e2e/link_noise.py` | ok (Rust vectors + published cacophony IK / IKpsk1) |
| Not run | clippy, `pnpm build:phone`, `cargo build -p kelpied` after a phone build, `cargo test -p kelpie`, `pnpm tauri build --debug`, any e2e, perf gate, a real Windows Hello or password prompt |

macOS-only code (`user_presence::mac`, `peer_identity::{image_of, peer_pid_of}`) was type-checked once with `cargo check --target x86_64-apple-darwin` in a scratch crate. It has never run. That check predates the final tree, but this code has not changed since.

## State of each step

| Step | State |
|---|---|
| `crates/kelpie-link` (IKpsk1 / IK, mac1, version, chunks, counter rekey, QR, SAS, vectors) | done, tested |
| Python `scripts/e2e/link_noise.py` | done, checked against vectors |
| Phone `noise.ts`, `pair.ts`, `pairing-store.ts`, `link.ts` codec, `phone-state.ts`, `app.tsx` | written, typechecks. `noise` and `link` tested. **No test for `pair.ts`; never run in a browser** |
| Daemon: store v2, `peer_identity`, `Role::App`, `/link`, listeners and rebind, caps, dedupe, grants, audit, kill switch, dormant expiry, budget | done, unit-tested |
| App: `user_presence.rs`, `mobile_secure_call`, `daemon_call` refusal, slot `source.txt` | written, **does not compile yet** (table above), so no desktop debug build is possible until step 1; no prompt ever exercised |
| Desktop `mobile-settings.tsx` | typed-code dialog, permissions, reset, legacy list, store error done. Missing items in step 2 |
| Vietnamese strings | not done (step 3) |
| e2e scripts | not done (step 4) |
| Docs, CI | not done (steps 5, 6) |

## Added items: done or not

**First list**

| Item | State |
|---|---|
| Noble crypto, no WebCrypto dependency | **Done.** Checked against published cacophony vectors and against Rust. Non-extractable WebCrypto keys **not used**: the Noise state machine is synchronous and the plain-http page cannot have them, so the key sits raw in IndexedDB. Noted in code; **still to document** in `docs/architecture.md` |
| msg1 empty, no side effects before msg2, fixed protocol string, new session per reconnect | **Done** |
| SAS `trunc(HKDF(h, "kelpie-sas"))`, 3 tries | **Done** in all three implementations. The third wrong code declines the request. The code was already spent by then (single use, spent at the first sealed frame) |
| Lockout notice | **Daemon done** (`mobile.lockout` event, audit line). **Desktop does not listen yet** (step 2) |
| Remote-forbidden ops | **Done.** Closed verb table, a test enumerating forbidden names at every permission, and a source check that phone code never calls the dispatcher. One-shot approvals enforced in the daemon for Claude `updatedPermissions`, Codex session and amendment decisions, ACP `allow_always`, and terminal menu keys (`Prompt::lasting`); the phone hides those options. See the residual under Security notes |
| Audit log | **Daemon done** (`mobile/audit.rs`, hash chain, `mobile.audit` RPC, no text stored). **Not shown in Settings yet** (step 2) |
| Kill switch | **Daemon done** (`mobile.revoke_all`, tested). **No button yet** (step 2) |
| Approval dialog: name, fingerprint, requested permission, "Did you just scan a QR?" | **Partly.** The daemon sends `fingerprint`, `want`, `address` in `mobile.pair_request` and `mobile.status.pending`; the phone shows the same fingerprint. **The dialog shows only the name and the typed code** (step 2) |
| Dormant expiry | **Daemon done** (default 90 days, `mobile.set_options {dormant_days}`, 0 = never, hourly sweep). **No setting in the UI** (step 2) |

**Second list**

| Item | State |
|---|---|
| mac1 prefilter | **Done** in Rust, TS, Python. An invalid one is dropped with no reply, close code, event or count |
| Handshake budget | **Done, tested.** 20/s global, 16 pending, 5 s. A WebSocket carries exactly one handshake, which is stricter than 5/s per connection. Nothing happens until the first valid sealed frame |
| Protocol version | **Done.** Header byte and prologue; `MIN_VERSION`; no negotiation and no retry |
| Nonces and rekey | **Done.** Implicit counters, fail closed, no reorder window, `Rekey` every 2^16 messages by counting in each direction with no signalling. **30 min trigger replaced by a re-handshake after 24 h** (see Deviations). No resume token exists |
| Revoke kills live sessions | **Done, tested.** All listeners share one event channel; `serve` re-reads the record after subscribing. The test has one listener only |
| Endpoint list in the pairing record | **Done.** QR `ep: [{t, a, r?}]` and the phone's `Pairing.endpoints`. No relay code |
| `TCP_NODELAY` | **Done** at accept. No direct test (not visible from the client side) |
| No compression | **Done, tested** (an offer of permessage-deflate is declined) |

**Not implemented, by decision:** capability tiers, per-command step-up, deny-by-default remote approvals, passkeys, relay, padding or chaff, user-facing option toggles. **Phone biometric unlock ("security at the app door") is also not in this tree**; see Unresolved questions.

## Files

**New:** `crates/kelpie-link/**` (includes `tests/vectors.json`, `tests/cacophony-ik.json`), `crates/kelpied/src/peer_identity.rs`, `crates/kelpied/src/mobile/{audit,link,listen,store}.rs`, `app/src-tauri/src/user_presence.rs`, `app/phone/src/{noise,noise.test,pair,pairing-store}.ts`, `scripts/e2e/link_noise.py`, this report.

**Modified, no foreign hunks:** `Cargo.toml`, `Cargo.lock`, `crates/kelpied/Cargo.toml`, `crates/kelpied/src/{auth,chat,choices,daemon,main,mobile,server,session}.rs`, `crates/kelpied/src/mobile/verbs.rs`, `crates/kelpie-proto/src/lib.rs`, `app/package.json`, `app/pnpm-lock.yaml` (noble packages), `app/src-tauri/Cargo.toml`, `app/src-tauri/src/{lib,daemon_runtime}.rs`, `app/src/lib/{attention,ipc}.ts`, `app/src/chat/{acp-adapter,chat-model,claude-adapter,codex-adapter}.ts`, `app/phone/src/{app.tsx,chat-view.tsx,link.ts,link.test.ts,phone-state.ts,phone.css,prompt-card.tsx,screen-sync.ts,storage.ts}`.

`app/src-tauri/src/lib.rs` had no foreign hunks when checked at the start (`git diff --stat` was empty). Re-check before staging.

**Foreign hunks: stage with care**
- `app/src/components/settings/mobile-settings.tsx`: I rewrote most of it. These two added lines are **another session's and must not be staged**: `import { LoadBar } from "../load-bar";` and `<LoadBar when={busy()} />`. Because the rewrite is wide they may land inside my hunks, so use `git add -p` with edit, or stage the file and then remove those two lines from the index.
- Do not stage: `app/src-tauri/src/config_transfer.rs`, `crates/kelpied/src/procs.rs`, `app/src/lib/settings-catalog.ts`, `app/src/locales/vi/{settings,shell}.json`, `docs/user-guide.md`, `plans/reports/researcher-261030-1500-mobile-push-client-options.md`, `.claude/agent-memory/*`, and the other UI-polish files in the brief.

**Line endings:** several Rust and TS files were rewritten with LF. Git normalizes them (`git diff --stat` shows only real changes on the ones checked).

## Test seams and how they stay out of release

| Seam | Where | How it is kept out |
|---|---|---|
| `KELPIE_TEST_USER_PRESENCE=approve\|deny` skips the OS prompt | `user_presence::verify` | The block is under `#[cfg(debug_assertions)]`, so it is not compiled in release |
| `KELPIE_TRUSTED_APP=<paths>` extra trusted executables, for the daemon-only e2e scripts | `peer_identity::trusted_apps` | Same |
| `KELPIE_LINK_WRITE_VECTORS` rewrites the vectors | `kelpie-link` tests | Test code only |
| `kelpie_link::testing` (fixed ephemeral keys) and `Session::set_rekey_every` | `kelpie-link/src/lib.rs` | **Not gated: `#[doc(hidden)] pub`, so compiled into release.** Nothing outside tests calls them. Gate them (step 7) |
| Unit tests use `notify::Memory` as the secret store | `Daemon::with_secrets` | Test-only constructor argument; no real keyring in `cargo test` |

e2e daemons will use the real credential store under the service name `Kelpie (<endpoint>)`, with keys `mobile-link-key` and `mobile-device-key`. The scripts must delete them afterwards (step 4).

## Deviations from the spec

1. **No wasm.** The phone uses `@noble/{curves,ciphers,hashes}` and a hand-written Noise state machine (the brief allowed this). `crypto.subtle` does not exist on plain `http://` origins, which is the common LAN case. Consequences: no `src/wasm.rs`, no `scripts/build-link-wasm.sh`, no wasm CI jobs, and the CSP does **not** gain `'wasm-unsafe-eval'`. The spec's "wasm instantiates" e2e check becomes "pairing works with zero CSP violations".
2. **No `src/client.rs`** in `kelpie-link`. The spec ties it to phases 4 and 5.
3. **30-minute rekey dropped.** Rekey is by count only. Time is covered by a re-handshake after 24 h (`REHANDSHAKE_AFTER`, close 4408): a re-handshake every 30 min would interrupt screens for little gain, and every reconnect is already a fresh handshake.
4. **Handshake timeout 5 s, not 8 s**, per the coordinator's budget. The phone's own timeouts stay at 8 s.
5. **Pairing spends the code at the first sealed frame, not at msg1.** A code answers 5 first messages per source address and is not burnt by replays, so a replay cannot destroy someone else's code. This replaces "burn on failed attempt", which would have let a replay burn it.
6. **Wrong pairing codes are silent.** mac1 fails, so there is no `4403` and no per-IP count for them. A phone with an expired code sees only a closed connection and a generic message. Per-IP failures now count only mac-valid failures such as unknown device keys. `BAD_CODE` remains only for a mac-valid msg1 that fails Noise. Lockout events therefore fire only for mac-valid failures and for three wrong typed codes.
7. **Loopback counts as a trusted transport** for the browser cap, because no network sits between. This also lets the e2e page type on `127.0.0.1`. The capped path is covered by unit tests only (`transport_cap`, `device_hello`), not by an e2e.
8. **`ip` setting kept.** Set: bind that address alone (loopback allowed, for tests). Unset (the default): every usable address, following changes.
9. **`NotifyUnicastIpAddressChange`** instead of `NotifyIpInterfaceChange` (more precise for a DHCP change), plus the 5 s poll on all platforms.
10. **Hostname in `addrs` is `<host>.local` only**, no Tailscale MagicDNS lookup.
11. **Minimal snow features** (`use-getrandom`, `use-curve25519`, `use-chacha20poly1305`, `use-blake2`) instead of `default-resolver-crypto`, so no aes-gcm or sha2. Pinned `=0.10.0`.
12. **macOS has no code-signature check**, only the path check (LOCAL_PEERPID + `proc_pidpath`).
13. **Trusted app on Windows** is `kelpie.exe` next to the daemon, or in the folder named by `<slot>/source.txt`, which the app writes when it fills or reuses a runtime slot (the daemon runs from `<data>/daemon/a|b`, not the install folder).
14. **`mobile.revoke_all` (kill switch) is allowed for the plain user token** with no verification, because it only removes access. `mobile.reset` is app-only and verified. App-only methods: `mobile.approve`, `mobile.set_permission`, `mobile.set_write`, `mobile.reset`.
15. **Transport independence** is at the crate level: `kelpie-link` is sans-I/O. The daemon's `Wire` still holds an axum `WebSocket`, and a later carrier needs a small abstraction there.

## Security notes

- **Threat model stated honestly:** same-user code can read the user token, the keyring and the install folder. `Role::App` plus OS verification raises the bar from reading a file to running code in the app. Verification happens in Rust inside `mobile_secure_call`, and `daemon_call` refuses the app-only methods, so the webview cannot skip it.
- **MAC'd device records** defend against file tampering only: same-user malware can read the keyring key.
- **One-shot residual:** a phone with `write` can still type into a terminal, so it could navigate a TUI menu with arrow keys and Enter, or run `/permissions`. The daemon refuses the direct keys and exact text, not arbitrary typing. Typing is a shell, and the user declined the capability-tier redesign. **Say this in the docs.**
- **Menu detection is by label regex** (`LASTING` in `choices.rs`), so a new agent's wording may be missed.
- **Audit** stores length and a truncated SHA-256 of typed text. Very short text is guessable from its hash; the file is the local user's. The queue is bounded (1024): when the disk is stuck, entries are counted and reported as `audit_dropped`, never blocking input.
- **No secrets logged:** `Debug` for `Keypair` and `QrPayload` redacts, with a test.

## Exact remaining steps, in order

1. **Make `app/src-tauri` compile.** `windows_verify` in `user_presence.rs` uses the wrong type path and a method that does not exist. Depend on `windows-future` directly or drop the type annotation, and find its blocking wait (probably `.join()`; not verified, read `windows-future-0.3.2` in the cargo registry). Run `cargo check -p kelpie`, then `cargo test -p kelpie` if disk allows. The `CredUI` fallback below it has not been through the compiler's later passes, so expect more errors.
2. **Desktop UI in `mobile-settings.tsx`** (keep the two `LoadBar` lines as they are; `settings-catalog.ts` is off-limits, so put new controls inside existing rows):
   - `MobileApprovals`: keep `fingerprint`, `want`, `address` from the event; show them and "Did you just scan a QR on your phone?"; preselect the requested permission (default read).
   - Toast on `mobile.lockout`.
   - Button "Turn off phone access and revoke all" calling `daemon("mobile.revoke_all")` (shown also when phone access is off and devices remain; check the current `<Show when={s()?.enabled}>` nesting).
   - Audit list from `daemon("mobile.audit", {limit})`, with a warning when `intact` is false.
   - Dormant setting via `daemon("mobile.set_options", {dormant_days})` (30 / 90 / 180 / never).
   - Move "Reset phone access" out of the enabled-only block if needed: it currently also appears under `store_error`.
3. **Create `app/src/locales/vi/mobile.json`** with Vietnamese for every missing key (`pnpm i18n:check` lists them; 47 now, more after step 2). Do not edit `vi/settings.json` or `shell.json`.
4. **e2e**, with `link_noise.py` as the phone's Noise:
   - `scripts/e2e/phone_support.py`: `pair()` and `Phone` over binary `/link`: send `Initiator.first`, finish with the reply, send sealed `{"type":"pair","name":...}` to pair or `{"caps":["ack","hb"]}` to reconnect, seal and open every message, `compression=None`. Approve with `{id, allow, code: session.pair_code(), permission}`.
   - `Daemon`: pass `KELPIE_TRUSTED_APP=<sys.executable>;<sys._base_executable>` in its env and re-auth its connection with `{"token":..., "client":"app"}` so it gets `Role::App`. The daemon must be the **debug** build.
   - `isolate()` teardown: delete `mobile-link-key.Kelpie (<endpoint>)` and `mobile-device-key.Kelpie (<endpoint>)` with `cmdkey` (verify the target naming with `cmdkey /list` first).
   - Call sites: `mobile_perf.py:100` (token), `mobile_protocol.py:501` (`mobile.set_write`), `mobile_resilience.py:463-473` (`pair_start`, `approve`), and `stress.py` if it pairs. Check that `net_fault_proxy.py` passes binary frames.
   - `mobile.py` (real app, debug build, `KELPIE_TEST_USER_PRESENCE=approve` in the env of tauri-driver only; daemon from `target/debug` next to `kelpie.exe`, so `Role::App`): QR link from `[data-link]`, read `[data-pair-code]` on the phone page, type it into `.mobile-approve-code`, click `[data-allow]`. The old checks at lines 231-238 and 577-578 and the token check must change.
   - New e2e checks from the spec: user-token `mobile.approve` refused; wrong code three times; revoke; re-pair after reset; kill switch; CSP zero violations; captured bytes hold no plaintext; a `one_shot` refusal.
   - Update `app/phone/src/sw.ts:56`, which still names `/ws` and `/pair` (should be `/link`).
   - Add a `pair.ts` vitest with a fake socket.
5. **Docs:**
   - `docs/architecture.md`: callers (`Role::App`), RPC and event lists (`mobile.set_permission|reset|revoke_all|set_options|audit`, `mobile.device_changed`, `mobile.lockout`), Phone protocol (replace `/ws`, `{token}` and `POST /pair` with the `/link` wire, header, close codes, budget, pairing flow), store v2, one-shot rule and its residual, the IndexedDB key note, bundle size line.
   - `README.md` privacy.
   - `docs/user-guide.md` is foreign: leave suggested text in this report.
   - Mark the plan's phase row.
6. **CI** (`.github/workflows/{ci,release}.yml`): add `-p kelpie-link` to the test lines and a gate that `cargo tree -p kelpie-link -e normal` contains no `ring` (today `-i ring` errors with "did not match any packages", which means absent, so do not use that form as the gate). No wasm jobs (deviation 1).
7. **Gate `kelpie_link::testing` and `set_rekey_every`** behind `#[cfg(test)]` or a test feature.
8. **Full verification, as in the brief:**
   - `pnpm typecheck; pnpm test; pnpm i18n:check; pnpm build:phone`.
   - `cargo build -p kelpied` (after `build:phone`).
   - `cargo test -p kelpie-proto -p kelpie-link -p kelpied`, plus `-p kelpie`.
   - Clippy with 0 warnings on the touched crates (`add_pending` and others carry `allow(clippy::too_many_arguments)`; check they are needed).
   - `pnpm tauri build --debug --no-bundle`.
   - e2e: `mobile.py` ×2, `mobile_protocol`, `mobile_resilience` (full), `acp_chat`.
   - `python scripts/perf/perf_gate.py --metrics bundle`.
   - One **manual** run of a real Windows Hello or password prompt: it cannot be automated, and the `CredUI` + `LogonUserW` fallback (including Microsoft accounts) has never run.
9. Finish this report (verification counts, user-guide text) and send for review.

## Unresolved questions

- **Phone biometric unlock:** the user's decision says the phone app is unlocked with biometrics, then has full rights. A browser page on plain `http://` cannot use WebAuthn, and the native app is phase 5. Is the unlock phase 5 only, or wanted for the HTTPS / Tailscale PWA in phase 3? Nothing is implemented.
- **Does the desktop want a toast for mac-invalid floods?** Currently silent by instruction, so an attacker spraying garbage leaves no trace.
- **Is a real-network e2e of the transport cap wanted?** It would mean binding a LAN address in tests, which may raise a Windows Firewall prompt. Unit tests only today.
- **macOS code-signature check:** needed before macOS ships?
