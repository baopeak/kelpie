# Phone phase 3: fix round for the security review

Branch `phone/phase3-link`, uncommitted working tree. Findings: `plans/reports/code-reviewer-261030-1841-phone-phase3.md`. Nothing staged, committed, pushed or stashed. No installer built, no real OS prompt raised, the user's daemon not touched.

**Accepted decision recorded:** deviation 3 (no 30-minute rekey; rekey by count, re-handshake after 24 h) is written into `docs/architecture.md` as accepted.

## Findings

| Finding | What changed | Test |
|---|---|---|
| **C1** OS verification in the daemon | New `crates/kelpied/src/user_presence.rs` (Hello, else CredUI + `LogonUserW`; macOS `LAContext`). `Daemon::after_verifying`: refuses unless `Role::App`, takes the one prompt slot, runs the prompt on its own thread, runs the action only after `Ok`, replies through `tx`; `dispatch` returns `Ok(None)` so the client's worker is free. `mobile.approve` order: decline with no prompt; else `check_approval` (exists, code, tries, ceiling), prompt, `commit_approval` (request must still exist). Prompt text is built by the daemon; `reason` is gone. App's `user_presence.rs` holds only `mobile_secure_call` (allow-list, `AllowSetForegroundWindow(daemon pid)`, `window` hint); `DaemonClient` waits 300 s for these calls only. Deps moved to `crates/kelpied/Cargo.toml`, removed from the app (which gains only `Win32_UI_WindowsAndMessaging`) | `mobile::tests::the_person_verifies_only_after_every_other_check_and_nothing_is_added_before_or_without_it` (no prompt on anything wrong; call returns while the prompt is held; second request refused; nothing added until it passes; quoted name and key in the text), `a_failed_verification_adds_nothing_and_a_request_gone_meanwhile_is_not_brought_back`, `who_is_not_the_verified_app_never_raises_a_prompt`, `user_presence::tests::*`, e2e deny scenario |
| **C2** one parser | `mobile::requested_permission` is the only reading of the params; `raise_prompt` compares it with the record; the same value is applied | `one_reading_of_the_parameters_says_what_raises_access_and_what_is_applied` (includes `set_write {permission:"write"}`, and `{permission:"write", can_write:false}`), `a_reset_always_asks_the_person`, e2e |
| **C3** / S2 / password wipe | `TokenUser` of the logon token and the process token must be `EqualSid`; `CredUIParseUserNameW`; `.` for a plain local name; `zeroize` for the password and CredUI's buffer; a Hello API error falls to the password prompt, a Hello cancel does not | `the_password_must_be_the_signed_in_users` (own SID vs own, vs `WinLocalSystemSid`), `the_account_name_is_taken_apart_for_the_logon` (real `CredUIParseUserNameW`, no prompt) |
| **W1** | `Mobile::removed` tests the record, not `permission()` | `turning_phone_access_off_keeps_the_pairing_and_the_kill_switch_and_a_reset_end_it`, e2e |
| **W2** | `Wire::close` seals `{type:"revoked"}` before every post-session `NOT_PAIRED`, so no call site can forget it; `no_access` sends 4408 when the record exists. `link.ts`: only the sealed message runs `unpaired()`; post-session 4401 reconnects; pre-session 4401 (unknown) or new 4402 (known, unconfirmed) shows "not paired here" with Forget, keeps the key, asks again once a minute (under the 10 a minute limiter) and on `kick()`. `lookup_key` separates paired, unconfirmed, unknown; `verify_devices` re-checks when the key is readable (on `status` and on such a phone's handshake); desktop state `unchecked` | `told_revoked` used in the removal tests, `a_stranger_hears_not_paired_and_a_record_that_cannot_be_trusted_is_told_apart`, `a_failed_or_garbled_key_read_never_makes_a_new_key`, `link.test.ts` (sealed word, in-session 4401, pre-session 4401/4402, retry timing, kick), `conn-state.test.ts`, e2e |
| **W3** / S4 | `chat::answer_is_one_shot`: allow-list by request method, anything else `one_shot`. Checked against `claude-adapter`, `codex-adapter`, `acp-adapter`. ACP options: only `allow_once` / `reject_once`. `acp-adapter.ts` hides `reject_always` too | `only_answers_known_to_hold_for_one_request_pass` (20 accepted shapes, 22 refused) |
| **W4** | `LASTING` gains the missing labels; `menu_labels` reads whole items with wrapped lines; on a question of permission a label that is not plainly once or no counts as lasting; `[A] Yes to All` and key lists are classified too | `the_labels_of_lasting_modes_are_known`, `an_item_wrapped_in_a_narrow_pane_is_read_whole`, `on_a_question_of_permission_what_cannot_be_told_counts_as_lasting` |
| **W5** | `kelpie_proto::secret_read` (`Result<Option<_>>`), `SecretStore::read`; `mac_key` and `link_key` create only on "none"; garbled or failed is an error, never written over. `secret_get` unchanged | `a_failed_or_garbled_key_read_never_makes_a_new_key` |
| **W6** | `phone_line`: a line too long once escaped becomes a `kelpie_stderr` notice at the same `seq`. `Wire::send`: `TooLarge` is logged and dropped, the session goes on; another seal error logs and closes 4408 | `a_line_too_long_for_a_phone_becomes_a_notice_measured_as_it_goes_out`, `a_message_too_long_to_seal_is_dropped_and_the_session_carries_on` |
| **W7** | 4 pending per address, none for loopback (checked: the e2e scripts connect from loopback; `HANDSHAKES_PENDING` still bounds it); a slot that times out silent counts via `failed` | `an_address_cannot_take_every_place_among_the_handshakes` |
| **W8** | Raising `dormant_days` or setting 0: refused for `Role::User`, prompt for `Role::App`; lowering free. `pair_start` takes no params, browser kind only | `keeping_unused_phones_longer_is_for_the_person_and_shortening_is_for_anyone`, e2e |
| **W9** | Deny scenario in `mobile_protocol.py security()`. `mobile.py`: browser log via `ms:loggingPrefs` for CSP, with a positive control (a script from elsewhere must show in that log). **Caps signal chosen:** the page writes the daemon's verdict `hello.hb.legacy` to `<html data-served-legacy>`; the daemon derives it from the page's real first sealed message, so a page that stopped sending `caps` reads `true` and the check fails | `mobile_protocol.py`; the `mobile.py` checks are written, not run |
| **S5** | `send_lock` in `mobile_perf.py` `Light` | not run |
| Info | Approval dialog in-flight guard, pending loaded on mount (`pending` has `address`), `control` wording; `drop_device_queues` on re-pair and expiry; stale text in `noise.ts`, `session.rs`, `architecture.md` (bundle re-measured: 68 KB gzip page + script + style, 81 KB with lazy adapters and worker); audit torn line (`a_line_cut_short_while_writing_is_no_false_alarm_and_the_chain_carries_on`); `pair.ts` overall timeout, 8 s then 135 s (test in `pair.test.ts`) | as named |

`mobile.py` also refuses to go on unless the daemon reports `test_user_presence == "approve"`. I added that field to `daemon.info` (debug builds only) so "nothing in that script may pop a real prompt" is checked, not assumed. This is one small addition beyond the literal brief.

## What I ran

Final results, all on the finished tree:

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 55 files, 670 / 670 |
| `pnpm i18n:check` | 0 missing, 0 problems |
| `pnpm build:phone` | built |
| `cargo build -p kelpied` | clean |
| `cargo check -p kelpie` | clean |
| `cargo test -p kelpie-proto -p kelpie-link -p kelpied -p kelpie` | kelpie 170 pass, 5 ignored; kelpie-link 12; kelpie-proto 23 pass, 2 ignored; kelpied 481 pass, 3 ignored; 0 failed |
| `cargo clippy -p kelpie-link -p kelpied -p kelpie --all-targets` | 0 warnings |
| `python -X utf8 scripts/e2e/link_noise.py` | ok |
| `python -X utf8 scripts/e2e/mobile_protocol.py target/debug/kelpied.exe` | 180 passed, 0 failed, on each of two runs |
| `cmdkey /list` | no `mobile-link-key` or `mobile-device-key` entries left |
| `cargo check --target x86_64-apple-darwin` of the macOS verification code, alone in a scratch crate | clean |

Failures as they came, before those results:

- **`cargo test -p kelpied`, first run: 478 passed, 2 failed.**
  - `a_line_cut_short_while_writing…`: my new test damaged the log's first line, which the chain cannot notice (cutting the head is a known gap, left below). It now damages the second line.
  - `a_phone_cannot_answer_what_the_client_answers`: **an existing test changed.** It answered ACP's `session/request_permission` with `result: {}`, which no adapter writes and the allow-list now refuses. It now sends a real answer, `{"outcome": {"outcome": "cancelled"}}`, and checks the same thing.
- **`pnpm test`, first run: 668 passed, 2 failed** (`store-notes`, `store-notifications`): each hit the 5 s limit in its first test while `cargo build` was running. Both passed alone in 2.6 s, and the full suite passed with nothing else building. I did not touch these tests or `store.ts`.
- **clippy, first run: 3 warnings.** Two complex types got aliases. `peer_identity::peer_pid` got an `allow` because a pid's type differs per OS.
- **The three load-sensitive checks** you warned about passed on both `mobile_protocol.py` runs. I changed nothing in them, so that reflects a quieter machine, not a fix.

Other existing tests changed: the removal tests now require the sealed word (`told_revoked`), which is stricter. The close-code table in `link.test.ts` now expects an in-session 4401 to reconnect, as you decided. `device_by_key` became `lookup_key`, and `set_options` and `pair_start` have new signatures.

**Not run:** `mobile.py` (left to the independent tester), `mobile_perf.py`, `mobile_resilience.py`, `stress.py`, `pnpm tauri build --debug`, and the perf gate. `mobile.py` and `mobile_perf.py` were edited and only syntax-checked. The macOS check left a small `x86_64-apple-darwin` folder in the normal build directory.

## What I left

- **S1** PID reuse, **S3** `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. With the gate in the daemon both now gain only a prompt with the daemon's text.
- **Info:** audit chain unkeyed and head/tail cuts unnoticed, audit hash not keyed, `addrs.seq` restarting with each daemon, a new `mobile.v2.unreadable-*` copy on every start, the 114-byte first-message check.
- `plans/261024-1205-phone-remote-control/phase-03-kelpie-link-e2e.md`: not on my list, so its success criteria and the deviation 3 note are not marked there.
- The dormant-expiry queue drop lives in the sweeper task and has no direct test; the re-pair path's `replaced` list does.

## What a person must try by hand

Never yet run: any real prompt, on any machine. Use a dev pair so the real daemon stays out of it, and do **not** set `KELPIE_TEST_USER_PRESENCE`:

```
bash scripts/dev-daemon.sh run                         # shell 1
cd app && KELPIE_ENDPOINT=kelpie-dev pnpm tauri dev    # shell 2
```

**Windows Hello (a machine with a PIN or biometric set up)**
1. Settings → Mobile, turn phone access on, "Pair a browser on this network". Open the link in a browser, name the device, note the six digits.
2. Type the digits, click "Allow reading". **Record: does the Windows Security dialog appear, is it in front of Kelpie, does it take focus, and does its text read** `Kelpie: allow the phone “<name>” (key XXXX XXXX XXXX XXXX) to read your sessions on this computer.`
3. If instead the password dialog appears, Hello refused the app's window handle: look for `Windows Hello:` in the daemon's log. That is the fallback working, and the Hello path needs another look.
4. While it is up: switch sessions and type in a terminal (must stay responsive); click Allow again (the button is disabled); change a device's permission in Settings (must say "already asking").
5. Cancel: the dialog shows "Verification was cancelled.", the request stays, no device. Allow again and pass: device listed, phone connects.
6. Raise read to write: prompt. Lower it: none. Dormant "Never remove" or a longer period: prompt, and on cancel the select returns to the old value. Shorter: none. Reset: prompt.
7. Leave a prompt open past 2 minutes, then pass: "no such request", nothing added. Past 5 minutes the app shows a timeout; answering later must still add nothing unexpected.

**Password prompt (a user with no Hello, or an RDP session)**
8. Same flow; the credential dialog must appear with the same text. **Record whether it comes to the front**, and whether Kelpie's window behaves normally afterwards: a parent in another process may be disabled while the dialog is up.
9. Wrong password: "That is not the password of the account signed in to this computer." Another valid local account's password: the same, and the daemon log says `that is not the account signed in here`. The right one: paired.
10. **Try each that applies, this is S2:** local account, domain account, Microsoft account (`MicrosoftAccount\me@outlook.com`, and the plain email), PIN-only account, blank password. All fail closed if they fail; a Microsoft account that cannot pass here cannot pair without Hello, and needs a decision. If a correct password is always refused, try `CRED_PACK_PROTECTED_CREDENTIALS` in `CredUnPackAuthenticationBufferW`.

**macOS:** the verification code was type-checked alone for `x86_64-apple-darwin` and has never run. Check that `LAContext` shows a prompt from the daemon, which is not an app bundle.

**`mobile.py`** (independent tester): `KELPIE_TEST_USER_PRESENCE=approve` must now reach the **daemon**: it stays in tauri-driver's environment, and the script passes it to the daemon it starts.

## Suggested text for `docs/user-guide.md` (off-limits to me)

> **Approving a phone.** Scan the code, then type the six digits the phone shows into Kelpie. Windows then asks you to confirm with Windows Hello or your password (Touch ID or your password on a Mac). Read that prompt: it names the phone and what it would be allowed to do. Raising what a phone may do, resetting phone access, and keeping unused phones paired for longer ask the same way. Declining a phone, lowering a permission and removing a phone do not.
>
> **"Not paired here" on the phone.** The computer did not recognise this phone. It may have been removed there, or the answer may not have come from your computer. The phone keeps its pairing and tries again by itself. If it was removed on purpose, tap *Forget this device*, then pair again. A phone only forgets its pairing by itself when your computer tells it so over the encrypted connection.
>
> **Turning phone access off** disconnects phones but keeps them paired: they reconnect when you turn it back on. *Turn off phone access and revoke all* removes them.
>
> **Limits.** Kelpie's check stops a program that merely has your user account's Kelpie token from adding a phone. It does not stop a program determined to replace Kelpie's own files, since those belong to your account.

## Unresolved questions

1. Does Hello accept a window handle from another process, and do both dialogs come to the front? Only the manual run can say. Should Hello fail there, the password prompt is used; a dialog stuck behind Kelpie would need a small owner window in the daemon.
2. The prompt text is English: the daemon has no translations and the caller may not supply text. Is that acceptable for a Vietnamese desktop?
3. A Microsoft account without Hello may be unable to pass the password check (see step 10).
4. W7 exempts loopback, which matters again when phase 3a proxies `tailscale serve` to loopback: every remote phone would then look local. Judge it there.
5. A hint-less wrapped last menu item at column 0 is read as a hint, not as part of the label. It is still caught when its first line is not plainly once or no.
