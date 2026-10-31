# Code review: phone phase 3 (kelpie-link, Role::App, store v2)

Branch `phone/phase3-link`, uncommitted working tree. Spec: `plans/261024-1205-phone-remote-control/phase-03-kelpie-link-e2e.md`, plan D1, D2, D5, D11.

**Method.** Findings come from reading the code, plus the installed Codex schema (`codex app-server generate-json-schema`, scratchpad, deleted after). No build, test, daemon or e2e script was run. "Verified" below means proven by reading, not by running.

**The tree moved during the review.** `crates/kelpied/src/mobile.rs` and `scripts/e2e/mobile_protocol.py` were saved at 18:47:56 by another writer. Line numbers are from 19:05. W1 comes from that edit.

**Verdict: do not merge.** Spec success criterion 2 ("no program holding only the user token can add a device or raise permissions") is not met.

---

## Critical (verified)

### C1. `Role::App` trusts paths that any same-user program can write
`crates/kelpied/src/peer_identity.rs:30-38`, `:61-64`

The daemon trusts `<own dir>\kelpie.exe` and `<folder in source.txt>\kelpie.exe`. In release the own dir is `%LOCALAPPDATA%\Kelpie\daemon\a|b`, and `source.txt` sits there too. It is re-read on every `auth`. There is no signature check, the repo has no signing config, and the NSIS install is `currentUser` (`tauri.conf.json:51`), so the install folder is also user-writable.

**Failure scenario** (any same-user program, including any agent session or a prompt-injected agent):
1. Read the user token, then call `mobile.enable` and `mobile.pair_start {app:true}`.
2. Do the Noise handshake itself, so it knows the six digits, and send sealed `{type:"pair",want:"write"}`.
3. Copy itself to `%LOCALAPPDATA%\Kelpie\daemon\a\kelpie.exe`, or write any folder into `source.txt`.
4. Run that copy, send `auth {token, client:"app"}`, and get `Role::App`.
5. Call `mobile.approve {id, allow:true, code, permission:"write"}`.

Result: a phone with `write` access, no OS prompt, and no injection into the app. The OS check lives only in the app (`user_presence.rs`), so a fake app skips it. The person sees only a toast after the fact.

`docs/architecture.md:91-93` ("from reading a file to running code in the app's process") is false.

**Fix.** Removing `source.txt` is necessary but not enough, because the slot dir and the install folder stay writable.
- Run the OS verification in `kelpied` for raising `APP_ONLY` calls, so forging the app gains nothing. Keep `Role::App` as a second layer.
- Do not trust `own_dir` when running from a slot.
- Add an Authenticode publisher check if releases become signed.
- If the user accepts the residual instead, rewrite the docs to "the bar is writing a file and running it", and mark criterion 2 unmet. This is a user decision (see Unresolved questions).

### C2. `mobile.set_write {permission}` raises access with no OS verification
`app/src-tauri/src/user_presence.rs:26`, `crates/kelpied/src/daemon.rs:1637-1643`

`needs_presence` looks only at `can_write` for this method. The daemon reads `permission` first.

**Failure scenario:** JS in the main webview (an XSS in rendered agent output, for example) calls
`invoke("mobile_secure_call", {method:"mobile.set_write", params:{id, permission:"write"}, reason:""})`.
There is no prompt, and the device goes from `read` to `write`. `permission:"control"` works the same way on a `Kind::App` device.

`docs/architecture.md:73` ("the webview cannot skip the check") is false. The test at `user_presence.rs:188` misses this case.

**Fix.** Drop `mobile.set_write` from `mobile_secure_call` (the UI does not use it, and `daemon_call` still refuses it). Better still, decide `needs_presence` with the same parser the daemon uses, and have the daemon reject `permission` on `set_write`. Add a regression test.

### C3. The password fallback accepts any valid account, not the signed-in user
`app/src-tauri/src/user_presence.rs:133-145`

`LogonUserW` succeeding is the whole check. `user` and `domain` are never compared with the current user, and the token is closed unread. `CREDUIWIN_ENUMERATE_CURRENT_USER` only controls what the prompt lists.

**Failure scenario:** on a machine without Windows Hello, a colleague at an unlocked PC enters their own domain or local credentials and approves their own phone onto this user's sessions. Any account with logon rights passes.

**Fix.** After `LogonUserW`, read `TokenUser` from the returned token and from the process token, and require `EqualSid`. Add a test for the compare helper.

The Hello path does verify the current user. Every failure path in both flows returns `Err`, so cancel is handled correctly.

---

## Warning (verified)

### W1. Turning phone access off un-pairs every connected phone
`crates/kelpied/src/mobile.rs:1785-1787` (the 18:47 edit), `:443-452`

The new close condition is `generation.is_cancelled() && permission(id).is_none()`. `permission()` also returns `None` while disabled, because `disable()` sets `enabled=false` before it cancels.

**Failure scenario:** the person toggles phone access off. Each connected phone gets 4401 and runs `unpaired()`, which deletes its private key, cache and drafts (`phone-state.ts:160-161`). The desktop still lists the device as `ok`.

This also breaks `mobile.py:579-597` ("the page reconnects by itself"), and the comment on the new block is wrong.

**Fix.** Test for the record itself: `!config.devices.iter().any(|d| d.id == id)`.

### W2. An unauthenticated close code destroys the phone's key
`app/phone/src/link.ts:621-624`, `crates/kelpied/src/mobile/link.rs:271-274`

WebSocket close frames are outside Noise. 4401 wipes the key with no proof of who sent it.

**Failure scenarios:**
- (a) An on-path host on plain `http://` LAN injects a close frame with 4401, or a rogue host answers at the old IP. The pairing is destroyed.
- (b) A credential-store read fails at daemon start, so `verify_devices` (`mobile.rs:315-327`) marks all records unverified and they never re-verify. Every phone that reconnects gets 4401 and wipes its key, permanently.
- (c) A heartbeat racing `disable()` hits `permission()` returning `None` at `mobile.rs:1991`.

**Fix.**
- Wipe only on a sealed `{type:"revoked"}` inside the session.
- A pre-session 4401 should show "not paired here" and keep the key until the person chooses to forget it.
- Separate "known but unverified" from "unknown".
- Re-verify when the key becomes readable.

### W3. The one-shot rule is a deny-list, and it misses a Codex session grant
`crates/kelpied/src/chat.rs:306-322`

The installed Codex schema has `PermissionsRequestApprovalResponse {permissions, scope: turn|session}`. `asks_the_user` accepts `item/permissions/requestApproval`, and `answer_lasts` checks only `updatedPermissions`, `decision` and `optionId`.

**Failure scenario:** a `write` phone sends `chat_answer` with `{"result":{"permissions":{...},"scope":"session"}}`. It is accepted, and a lasting grant has been made from the phone. ACP `reject_always` also passes (it lasts, though it is not a raise).

**Fix.** Use an allow-list per request method: accept only the known one-shot shapes and refuse the rest as `one_shot`.

### W4. Terminal "lasting" detection misses real labels and wrapped lines
`crates/kelpied/src/choices.rs:69`, `:76-79`, `crates/kelpied/src/mobile.rs:1637`

The regex misses "auto-accept edits", "bypass permissions", "for this project" and "from now on". It also matches one line at a time, so `2. Yes, allow all edits during this` / `session (shift+tab)` is not caught when a narrow pane wraps it.

**Failure scenario:** the phone taps that key on that menu, and a lasting mode is set. Arrow keys and non-exact text such as `2.` are the documented residual; these misses are not.

**Fix.** Add those labels. Match the whole item, including its continuation lines. When a menu cannot be classified, treat any option beyond "once / no" as lasting.

### W5. A failed credential read regenerates and overwrites the keys
`crates/kelpied/src/mobile.rs:335-344`, `:357-365`, `crates/kelpie-proto/src/lib.rs:281`

`secret_get` turns every error into `None`, and a garbled value becomes `None` too. `mac_key(true)` and `link_key()` then create and store a new key.

**Failure scenario:** a transient read error, or a truncated entry, followed by a successful write. All device records fail their MAC after the next restart, and all phones are locked out, because their `dpk` no longer matches and mac1 fails silently. This breaks the AGENTS.md rule that a failed state read is not permission to overwrite.

**Fix.** Make `secret_get` return `Result<Option<_>>`, and create a key only on `NoEntry`. Treat a garbled value as an error, not as absent.

### W6. Any phone message over 1 MiB causes a reconnect loop (regression)
`crates/kelpied/src/chat.rs:35` (4 MiB lines allowed), `crates/kelpied/src/mobile.rs:2171`, `:2001`, `crates/kelpied/src/mobile/link.rs:77`

"A longer line goes alone", so a chat line from about 0.6 MiB can exceed `MAX_APP_MESSAGE` once JSON-escaped (a line with base64 image output, for example). `seal` then fails, `Wire::send` returns false, and the socket drops with no code.

**Failure scenario:** the phone re-subscribes from the same `since_seq`, gets the same line, and drops again. The whole connection flaps while that chat is watched.

**Fix.** In `chat_messages`, replace over-long lines with a notice (as `ingest_line` does), measured after escaping. Add a test. A bare `seal` failure should never end a session silently.

### W7. Any LAN host can silence all pairing and reconnects
`crates/kelpied/src/mobile/link.rs:142`, `crates/kelpied/src/mobile.rs:589-593`

The 16 pending handshake slots are global, each is held for 5 s, there is no per-address cap, and nothing is counted.

**Failure scenario:** 16 idle WebSockets every 5 s. All phones get 1006 forever, and no event or audit line records it.

**Fix.** Add a per-IP pending cap (2 to 4), and count timed-out slots against the address.

### W8. Two settings that weaken security are open to the plain user token
`crates/kelpied/src/daemon.rs:1667`, `:1611`, `crates/kelpied/src/mobile.rs:750`, `crates/kelpied/src/mobile/link.rs:228`

- `mobile.set_options {dormant_days:0}` lets a program keep a rogue device forever. D18 says weakening needs `Role::App` plus verification.
- `mobile.pair_start {app:true}` mints `Kind::App`, which has a `Control` ceiling and no transport cap (`mobile.rs:1173`). The spec says Settings offers only the browser form until the native app ships. The approval dialog does not show `kind`, so a native client gets `write` on plain LAN without the person knowing.

**Fix.** Make lowering or disabling the expiry app-only. Refuse `app:true` from `Role::User`, or remove it until the native app exists. Show `kind` in the dialog.

### W9. Missing proof for security claims
- `KELPIE_TEST_USER_PRESENCE=deny` is never used. No test proves a cancelled verification stops the decision.
- The transport cap is unit-tested only (known from the checkpoint).
- `mobile.py` dropped the only real-page check that the page sends `caps` and is not served as legacy. Nothing replaced it, because the frames are now opaque.
- The CSP listener in `mobile.py:250` is attached after page load, so load-time violations are not caught.

---

## Warning (suspected, needs a run)

| # | Where | Suspicion | Check or fix |
|---|---|---|---|
| S1 | `server.rs:32`, `:157`, `peer_identity.rs:61` | **PID reuse.** The PID is taken at accept, the image at `auth`. A client connects, passes the duplicated pipe handle to a helper and exits. The attacker then spawns the real `kelpie.exe` suspended until it reuses that PID. The helper's `auth` then gets `Role::App` with no injection. | Evaluate at accept and hold a process handle for the connection, or require process creation time earlier than accept (`GetProcessTimes`). |
| S2 | `user_presence.rs:134`, `:137` | The comment "LogonUser takes them apart" is unsupported. Docs describe UPN with a NULL domain. If CredUI returns `MACHINE\alice` with an empty domain, the fallback may always fail. Microsoft and Azure AD accounts and blank passwords are unknown. Fails closed. | One manual run. Use `CredUIParseUserNameW`. |
| S3 | app start | A same-user program starts the real app with `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=…` and drives the main webview. With C2 that is a silent raise with no file drop. Without C2 it is a real prompt carrying the attacker's `reason`. | Try it. Strip the variable at startup in release builds. |
| S4 | `chat.rs:306` | Codex `mcpServer/elicitation/request` has a free `_meta`, which may carry persistence. | Covered by the W3 allow-list. |
| S5 | `mobile_perf.py:108` | `Light.send` has no lock, unlike `ps.Phone`. An ack from `_read` may overtake a sealed `sub`, which is a nonce error, the session drops, and the perf run flakes. | Add the same `send_lock`. |

---

## Info

- **Password wipe:** `user_presence.rs:146` wipes `pass` with a plain loop right before the Vec is freed. The compiler may remove it. Use `zeroize` or a volatile write.
- **Audit log** (`mobile/audit.rs`):
  - The chain is unkeyed, so same-user code can rewrite it.
  - Cutting the head or tail is not noticed (`:130-142`).
  - A torn last line resets `prev` to genesis, giving a permanent false "edited or damaged" (`:49-53`).
  - The unsalted 48-bit hash of typed text is brute-forceable for short passwords, and it is readable with the user token (`:152-155`). Key it with HMAC under `K_MAC`.
- **`addrs.seq`** restarts with each daemon (`mobile.rs:1102`). The phone ignores `seq <= addrsSeq` (`phone-state.ts:153`), so a restart hides new addresses. Harmless today, but phase 4 depends on it. Add the epoch to the comparison.
- **Store:** a new `mobile.v2.unreadable-*` copy is written on every start while broken (`store.rs:145`). The UI's "a copy was kept" (`mobile-settings.tsx:131`) is false when the read or the copy failed.
- **`mobile-settings.tsx`:**
  - Requests come only from the event, so a window reload loses the dialog even though `status.pending` has them (`:266`).
  - There is no in-flight guard, so a double click on a wrong code burns two of the three tries and raises two prompts (`:315-316`).
  - `control` is labelled "read and type" (`:306`).
- **Queues:** a re-pair that replaces an old record and a dormant expiry do not call `drop_device_queues` for the old id.
- **Stale text:** `noise.ts:2` ("the sender signals a rekey"), `session.rs:1025` (`screen_prompt`'s doc now sits on `choice_lasts`), `docs/architecture.md:494` ("about 45 KB gzip" not re-measured since noble).
- **`pair.ts`** has no overall timeout, while the checkpoint says the phone's timeouts stay at 8 s.
- **Cheap hardening:** `link.rs:169` could reject any first message that is not exactly 114 bytes before hashing up to 65 KB per live code.

---

## The "look hardest" list

| Area | Verdict |
|---|---|
| 1. Pairing | Code logic is sound: the SAS comes from the handshake hash, is kept only in `Pending`, and is never in status or events. Spend is atomic under the lock, and concurrent attempts get `declined`. The three-tries rule holds. Replay cannot spend a code. **The check on who may approve fails: C1.** |
| 2. `Role::App` | **Fails: C1, C2, S1.** Re-auth is correct: the role is recomputed on each `auth`, and a failed re-auth changes nothing. `daemon_call` refusal and `user_may` are correct, all `Role::User` matches were updated, and `dispatch` has no other entry point. Tauri 2.12.4 rejects app commands from remote origins (`webview/mod.rs:2080`), so browser panes cannot call these. |
| 3. `user_presence.rs` | Hello path is correct. **The fallback fails: C3.** Fail-closed on every path. S2 is open. |
| 4. Verbs and one-shot | The verb table is closed and tested. Live permission recheck per message, Lagged recheck, cap applied per message and again inside `enqueue_if`: correct. **One-shot rule leaks: W3, W4.** |
| 5. Store v2 | No-overwrite holds, including on non-NotFound read errors, and `save()` is guarded. The MAC covers id, name, pk, permission, kind and created_at. v1 is never written. Unverified devices can only be listed, revoked or replaced. **W2(b), W5.** |
| 6. Crypto | All three implementations agree: implicit nonces, `n` not reset on rekey, rekey counts every chunk, mac1, prologue and SAS, with vectors covering them. Reassembly is bounded at 1 MiB and 10 s. mac1 runs before any DH. **W7.** |
| 7. Test seams | **All gated correctly.** `testing` and `set_rekey_every` are `#[cfg(test)]`. `KELPIE_TEST_USER_PRESENCE` and `KELPIE_TRUSTED_APP` are `#[cfg(debug_assertions)]`, and no release profile enables assertions. No other crate uses them. |
| 8. e2e safety | **Cannot touch the real daemon or its keys.** The real keyring service is plain `Kelpie`, while `forget_secrets` targets `<key>.Kelpie (<endpoint>)`; the keyring target format was confirmed in `keyring-3.6.3`. `isolate()` and `isolated_env()` guard the endpoint. Checks: W9. |
| 9. UI and docs | UI: Info items above. Docs are accurate except `architecture.md:73`, `:91-93` and `:494`. |

## Deviations from the spec

| # | Deviation | Judgment |
|---|---|---|
| 1 | No wasm, noble instead | Sound. |
| 2 | No `client.rs` | Sound, deferred. |
| 3 | No 30-min rekey, 24 h re-handshake | Small weakening: the window of past traffic exposed by a stolen session key grows. Acceptable, but needs the user's sign-off. |
| 4 | Handshake 5 s | Sound. |
| 5 | Code spent at first sealed frame | Sound, and stronger than the spec. |
| 6 | Wrong codes silent | Sound. Together with W7, floods leave no trace. |
| 7 | Loopback trusted | Sound today, because `usable()` excludes loopback. Judge again in 3a, when `tailscale serve` proxies to loopback. |
| 8 | `ip` setting kept | Sound. A v1 `ip` carries over and pins one address. |
| 9 | `NotifyUnicastIpAddressChange` | Sound. |
| 10 | `.local` hostname only | Functional gap, no security impact. |
| 11 | Minimal, pinned snow features | Sound. |
| 12 | macOS no code signature | **Real weakening.** Must be fixed or accepted before macOS ships. |
| 13 | `source.txt` | **Real weakening: C1.** |
| 14 | `revoke_all` for the user token | Sound for security: it only removes access. Any program can use it as a nuisance. |
| 15 | Transport independence at crate level | Sound. |

## Score: 5/10

The crypto, store and verb table are solid. The two security controls the phase exists to add, the trusted app and user verification, can both be bypassed, and the 18:47 edit adds a regression that will fail `mobile.py`.

**Required before merge:** C1 to C3, W1 to W8.

## Unresolved questions

1. **C1 is a user decision.** Options:
   - (a) Run the OS verification inside `kelpied`: strongest, moderate work.
   - (b) Sign releases and pin the publisher: needs a certificate and only works for signed builds.
   - (c) Accept "write a file and run it" as the bar: least work, but criterion 2 stays unmet and the docs must say so.
2. Does the user accept the 24 h re-handshake (deviation 3)?
3. Is a plain `http://` LAN device allowed to lose its pairing to a forged close, or must revocation be sealed (W2)? Recommended: sealed.
4. Not verified by this review: the perf gate (≤ 5 % crypto overhead), the downgrade test, the phone bundle size, and whether the current `user_presence.rs` compiles. The checkpoint's compile failure may be stale.
