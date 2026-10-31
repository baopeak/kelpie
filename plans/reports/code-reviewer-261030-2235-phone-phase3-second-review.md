# Second code review: phone phase 3 (kelpie-link, gate in the daemon, store v2)

Branch `phone/phase3-link`, HEAD d6e5883, uncommitted working tree.

**Method.** Read only: the code, `git diff`, and the Rust Reference on disk. Nothing was built or run, so "verified" means proven by reading. `plans/reports/fullstack-developer-261030-2105-phone-phase3-fixes.md` was read only after the findings were written. `mobile.py` was read as saved at 21:39 and `mobile.rs` as saved at 22:11. A tester may have changed `scripts/e2e/` since.

## Score: 7.5/10. Verdict: do not merge yet

The first review's security failures are really fixed. I tried to break each one and found no way past the gate that the owner has not already accepted. There are no Critical findings.

Four things block the merge:
1. The one code path the whole phase rests on, the real OS prompt, has never run on any machine (W1).
2. A Hello face pass may not need a deliberate act (W2, needs a run).
3. A prompt that never ends locks out phone-access decisions (W3).
4. `mobile.py` cannot pass as written (W4).

W5 to W8 are small and should go in the same round.

---

## Critical

None.

---

## Warning

### W1. The real prompt has never run, and three of its behaviours are unknown (merge blocker)
`crates/kelpied/src/user_presence.rs:100-124`, `:224-277`, `app/src-tauri/src/user_presence.rs:51-54`

Every test goes around `os::verify`: unit tests use `inject`, e2e uses `KELPIE_TEST_USER_PRESENCE`. The developer's report confirms no real prompt has been raised. A failure here is closed, so it is not a security hole, but phones could not be approved at all. What can only be learned by running it:
- **Hello with another process's window.** The daemon passes the app's HWND to `RequestVerificationForWindowAsync`. If Windows refuses that, every Hello user silently falls to the password dialog (`:120-123`).
- **PIN-only and Microsoft accounts on that fallback.** The password dialog may offer a PIN tile or need a password the person no longer remembers. `CRED_PACK_FLAGS(0)` may also return a protected password. If these fail, those users can never pair.
- **The dialog parented to Kelpie's window (`:236`).** A modal credential dialog normally disables its owner. Across processes that means Kelpie's window takes no input while the prompt is up, which goes against decision 1 ("a prompt must not block"). If the daemon dies during the prompt, the window may stay disabled.

**Fix.** Do the developer's manual list before merging. If the owner window is disabled, pass no parent and rely on `AllowSetForegroundWindow`.

### W2. A Hello face pass may not be a decision, and prompts have no limit or trace
`crates/kelpied/src/user_presence.rs:107-115`, `crates/kelpied/src/daemon.rs:538-556`

The first half is suspected and needs a run; the second is verified by reading.

**Scenario.** A same-user program holds the token and copies itself to where `peer_identity` looks, which docs say is enough for `Role::App`. It calls `mobile.pair_start`, does the Noise handshake itself so it knows the six digits, and calls `mobile.approve {permission:"write"}`. On a machine with Hello Face, the person only has to be looking at the screen. If the consent dialog passes on recognition with no click, the program has a rogue `write` phone, and the daemon's text was on screen for about a second. The accepted risk in decision 1 assumes the person can refuse.

Separately, **nothing limits or records prompts.** `after_verifying` writes no audit line or event when verification fails, and a refused prompt can be raised again at once. A program can keep prompting until someone gives in, and leaves no trace.

**Fix.**
- Try Hello Face once.
- If it passes without a click, show a daemon-owned confirmation before Hello (a message box with the same text, default No).
- In every case, audit `verify_failed` with the prompt's subject, and refuse new prompts for a while after three refusals in a few minutes.

### W3. A prompt that never ends holds the only slot
`crates/kelpied/src/user_presence.rs:45-50`, `:112`, `:245`, `crates/kelpied/src/daemon.rs:546`, `app/src-tauri/src/daemon_client.rs:27`

`verify` blocks with no time limit. The app gives up after 300 s, but the daemon does not.

**Scenario.** The dialog comes up behind other windows, or the app closes while it is up. For example, the Hello broker does not come to the front because the daemon is not the foreground process. From then on every approve, raise and reset is told "already asking you to verify: answer that prompt first", and there may be no prompt to find. The only way out is restarting the daemon. The kill switch still works, since it needs no prompt.

The deferred reply itself is otherwise sound. Every normal path replies once, a gone client is harmless, `then` needs no Tokio context (nothing it reaches spawns), and a process exit ends the thread. The only hole is a panic in `verify` or `then`: the slot is freed, but the app waits out the 300 s with no reply (S2).

**Fix.** Give the prompt a limit a little under 300 s. Hello can use `IAsyncOperation::Cancel`. CredUI cannot be cancelled, so close its window, or at least let the next caller take the slot and ignore the stale result. Add a test with `inject` that a held prompt past the limit answers with an error and frees the slot.

### W4. `mobile.py` cannot pass: its menu has a lasting choice and the page now hides it
`scripts/e2e/mobile.py:345-347`, `:373`, `:379-381`, `app/phone/src/prompt-card.tsx:48`, `crates/kelpied/src/choices.rs:64`

The test menu prints `2. Yes, and don't ask again`. `lasting_keys` classifies key 2 as lasting, the frame carries `lasting:["2"]`, and `PromptCard` filters it out. What follows:
- `keys == ["1","2","3"]` fails.
- `keys2` fails the same way.
- `tap("2")` finds no button and returns None, so `ANSWERED2=2` never comes.
- Menu 2 stays open, so `card(3)` times out and "the third prompt came up" fails.
- `:404` fails too.

These lines are unchanged from HEAD. They predate this phase's one-shot rule and were never run against it. The rest of the script, including its new checks, reads correctly.

**Fix.** Keep `2. Yes, and don't ask again` in the menu. Assert that the card offers `["1","3"]` and shows `[data-lasting-hidden]`, answer rounds 1 and 2 with key `1`, and keep key `3` for the desktop-first case at `:391`.

### W5. Yes-led menus that are not about permission lose their buttons on the phone
`crates/kelpied/src/choices.rs:80`, `:84-87`, `:130`

`permission` is decided from labels only: any item starting with `Yes`, `Allow`, `Accept` and so on makes the whole menu a permission question. Then anything not plainly once or no counts as lasting.

**Scenario** (traced through the regexes):
- `Ship it?` with `1. Yes, ship it` and `2. No`: `lasting` is `["1"]`. The phone can only say no.
- Claude Code's question form, with a description line under each option: `1. Yes` / `Apply the fix`, `2. No` / `Leave it`, `3. Type something.` The first label reads `Yes Apply the fix`, which `ONCE` does not match, so 1 and 3 are hidden and refused with `one_shot`. The person sees "a choice that lasts" for something that does not last.

The tests pin only `Which approach?`, which has no consent word. The error is on the safe side, but it blocks ordinary phone use, which is exactly what the brief asked to check.

**Fix.** Require a permission signal in the question or the menu as a whole, not only a leading `Yes`: a tool line, or a question with allow/permission/proceed/run/edit/trust. Add the two menus above as tests, plus a capture from a real AskUserQuestion. Owner's call if unknowns should stay refused.

### W6. The per-address handshake limit is evaded by sending one byte at the end
`crates/kelpied/src/mobile/link.rs:165-175`, `:190`, `crates/kelpied/src/mobile.rs:704-717`

Only pure silence counts against an address (`Err(_) => held_silently`). Any binary frame arriving before 5 s gets past that, fails `prefilter`, and returns with no count.

**Scenario.** Hold 4 slots for 4.9 s, send a garbage byte on each, reconnect at once, repeat. That address is never locked out and never appears in the audit log. Four LAN addresses, which a rogue host can take by aliasing, hold all 16 slots forever. Every phone is dropped with 1006 and sees only "reconnecting". This is first-review W7 again with a slightly higher bar.

The test `an_address_cannot_take_every_place_among_the_handshakes` calls `held_silently` directly, so it would not notice either this evasion or the removal of that branch from `accept`.

**Fix.** Count a connection that held a slot for more than about 1 s and then failed the prefilter. Better, when the table is full, drop the oldest idle slot and admit the new one, so holding gains nothing. Add a socket-level test.

### W7. After `mobile.reset` an offline phone never sees "not paired here"
`crates/kelpied/src/mobile/link.rs:190`, `crates/kelpie-link/src/lib.rs:197-199`, `app/phone/src/link.ts:616-630`, `docs/architecture.md:449-450`

`mac1` is keyed from the desktop's public key. After a reset, or if the link key is ever lost from the credential store, a phone holding the old key fails the prefilter and is dropped with no code. `closed(1006)` then reconnects with backoff forever. The page says "reconnecting" with the server plainly reachable, and the Forget button appears only in the Sheet. The docs say "A phone removed while it was away sees that state", which is false for this case. `mobile_protocol.py:848` even pins that it is dropped, but not what the person sees.

**Fix.** On the phone: after a few closes before any handshake reply while `reach` is `reachable`, show a third `not_paired_here` reason ("this computer may have been reset"). The key stays until Forget, as decision 2 requires. Correct the docs either way.

### W8. Docs: the user guide is stale and the same-user residual is understated
- `docs/user-guide.md:836-842` still says "scan it, name the phone and approve it on the computer", and that a device you allow to type can type. Missing: the six digits, the OS prompt, plain HTTP browsers being read-only, "not paired here", off versus revoke all. The developer left suggested text because the file was off limits. It needs applying.
- `docs/architecture.md:100-102` and the suggested user-guide text give "replacing `kelpied.exe`" as the limit, with "it can also read the keyring" in passing. There is an easier route with no binary change:
  1. Read `mobile-device-key` from Credential Manager (no ACL within the user).
  2. Write a sealed `write` record into `mobile.v2.json`.
  3. Call `daemon.shutdown` with the user token and let the next start load it.

  That adds a phone with no prompt and no audit line. The class is the one the owner accepted, but the sentence "The verification stops a program that uses the daemon as it is" is not true of it. Say so plainly.
- `docs/architecture.md:442` says the limit is "mac-valid only", while `:429-430` says silence counts. One of them must change.
- `docs/architecture.md:410-412`: the residual names arrow keys and `/permissions`. `shift-tab` (`verbs.rs:115`) is a one-tap lasting mode change in Claude Code and belongs in that list.

The rest of the architecture text checks out against the code: `after_verifying`, one prompt at a time, the seams, sealed revocation, the record states, and the no-rekey decision.

---

## Suggestion

| # | Where | What | Fix |
|---|---|---|---|
| S1 | `choices.rs:80`, `:201-205` | **Fail-open gaps in W4's fix.** `CONSENT` lacks `proceed/run/continue/confirm`, which `ONCE` has. So `1. Proceed` / `2. Keep going without asking` is not flagged. `Always allow this command? (y/n)`: the `y` is lasting and nothing looks at the question. | Add the words. Test the question against `LASTING` for y/n prompts. |
| S2 | `daemon.rs:545-554` | A panic in `verify` or `then` sends no reply; the app waits 300 s. The slot does free. | `catch_unwind`, or a guard that replies on drop. |
| S3 | `mobile-settings.tsx:222`, `:83-94` | `busy()` is true for the whole prompt, up to 300 s, and it disables the kill switch and reset. | Never disable the kill switch. |
| S4 | `user_presence.rs:214`, `:375` | If `CredUIParseUserNameW` accepts a plain name, the supplied `domain` is dropped and `.` is used. The UPN assertion accepts almost anything, and the null-domain path has no test. | Use the given domain when the parser yields none, and pin one UPN result. |
| S5 | `mobile.rs:495` | The `if let` keeps the config lock through `write_atomic`, so a slow disk stalls every phone's `permission()` on a Tokio worker. It predates this work, but this phase calls it far more often. | Serialize, release the lock, then write. |
| S6 | `store.rs:98-99` | `dormant_days` is outside any MAC, so the new gate can be skipped by editing the file. Same class as W8. | Mention it in the same docs paragraph. |
| S7 | `daemon.rs:1630`, `auth.rs:479` | The user token can turn phone access back on after the person turned it off, and paired phones regain access. There is no audit line for enable or disable. | Audit both. See question 3. |
| S8 | `scripts/e2e/` | `transport_cap` is still unit-tested only: e2e binds to loopback, which is exempt. | A test on a private address, or accept this. |
| S9 | `user_presence.rs:273-276` | Every password failure, including a blank-password restriction or an unpack error, reads "That is not the password". | Word the non-password failures apart. |
| S10 | `choices.rs:39`, `:141` | `Allow (a)   Allow always (A)` marks `a` as lasting because keys are lowercased, so the once key is refused too. Safe, but it blocks a real menu shape. | Compare keys case-sensitively where the menu distinguishes them. |

---

## First-review findings

| Finding | State | Evidence |
|---|---|---|
| C1 same-user program approves a phone | **Fixed** (accepted residual) | The prompt runs in the daemon. `then` runs only after `Ok`. Text comes from the daemon's records with no caller `reason`. `Role::App` only decides who may ask. Residual: W2, W8. |
| C2 raise without prompt | **Fixed** | One `requested_permission` decides and applies. `pair_start` takes no params. The only callers of `set_permission` outside tests are `apply_permission`. I found no other path that adds a device or raises a permission, apart from the file route in W8. |
| C3 any account's password | **Fixed** (never run) | `EqualSid` of the logon token and the process token. W1 covers running it. |
| W1 turning off un-pairs phones | **Fixed** | `removed()` tests the record. `no_access` sends 4408. Tested at socket level. |
| W2 unauthenticated close wipes key | **Fixed** | Only sealed `revoked` reaches `unpaired()`. Pre-session 4401/4402 keep the key and retry after 60 s. Keys are re-verified when readable. Gap: W7. |
| W3 one-shot deny-list | **Fixed** | Allow-list by method. It matches all three adapters; the only other shapes it accepts are harmless. Lasting options are hidden on the phone and refused by the daemon. |
| W4 lasting labels, wrapped lines | **Mostly fixed** | Labels and wraps work. False refusals: W5. Fail-open gaps: S1. |
| W5 failed read overwrites keys | **Fixed** | `secret_read` plus create only on none. A garbled value is an error. The unit test is strong. |
| W6 reconnect loop over 1 MiB | **Fixed** | `phone_line` measures after escaping. `TooLarge` is raised before any counter moves, so the session really does carry on. |
| W7 LAN host silences pairing | **Partly** | 4 per address and silence counted, but it is evaded by one garbage frame: W6. |
| W8 weakening settings open | **Fixed** | Lengthening needs `Role::App` and the prompt. `pair_start` makes only browser pairings. File route: S6. |
| W9 missing proof | **Mostly fixed** | Deny scenario in `mobile_protocol.py:874-904`. CSP read from the browser log, with a positive control. Caps via `data-served-legacy`. Transport cap: S8. |
| S1 PID reuse | **Not fixed, moot** | Still read at `auth`. It now only buys a prompt. |
| S2 account name parsing | **Addressed, unproved** | `CredUIParseUserNameW` is in. W1, S4. |
| S3 WebView2 debug port | **Not fixed, moot** | The page gets only a prompt with the daemon's text. |
| S4 Codex `_meta` | **Fixed** | Refused, and tested. |
| S5 perf `send` lock | **Fixed** | `mobile_perf.py:97`, `:109-111`. |
| Password wipe | **Fixed** | `zeroize` on the password and the CredUI buffer. |
| Audit log | **Partly** | Torn line fixed. Unkeyed chain, head and tail cuts, and the unsalted text hash remain; the hash is documented at `architecture.md:466`. |
| `addrs.seq` | **Not fixed** | Restarts per daemon (`mobile.rs:1250`). Harmless until a client dials other addresses. |
| Store copy per start, "a copy was kept" | **Not fixed** | `store.rs:145`, `mobile-settings.tsx:132`, `mobile.rs:1110`. |
| Settings: reload, double click, `control` label | **Fixed** | `mobile-settings.tsx:277`, `:300`, `:323`. |
| Queues on re-pair and expiry | **Fixed** | `approved.replaced` and the sweeper. |
| Stale text, `pair.ts` timeout | **Fixed** | `noise.ts:2`, `architecture.md:525`, `pair.ts:22-24`. |
| 114-byte first message | **Not fixed** | Minor: bounded by 65 KB and 4 codes. |

## Checked and sound

- **Order around the prompt:** code, tries and ceiling come first. A wrong code gets no prompt. Decline, timeout, revoke, kill switch and disable during a prompt all leave nothing behind (`commit_approval` needs the pending entry, `set_permission` needs the record).
- **`Role::App`:** only from `auth` plus the peer process. Phone connections are `Role::User`. Session roles cannot reach `APP_ONLY`. No `dispatch` caller bypasses `user_may` outside tests.
- **Test seams:** `cfg(debug_assertions)` or `cfg(test)`. No release profile enables assertions. `daemon.info` reports null in release.
- **`Wire::close`:** seals before every in-session 4401. A send cut off mid-frame by cancellation loses the sealed word, and the phone then sees "not paired here". That is the right direction.
- **`lookup_key` with an unreadable key:** Unconfirmed, no failure counted, nothing written, re-verified on the next look. Tested with a flaky store.
- **`link.ts` state machine:** no loop. Revoked stops all retries. 4401/4402 retry after 60 s, or at most every 2 s on page views, which stays under the 10 a minute limit for one phone. `loadPairing` failures keep the key in IndexedDB.
- **No lock inversion in `removed()`:** each operand of `&&` ends its own temporary scope (Rust Reference, destructors).

## Tests

**No weakened test found.** The two changed tests are justified: the ACP `result: {}` answer, and in-session 4401 per decision 2.

The prompt-order tests in `mobile.rs:3287-3361` are real behaviour tests. They hold the prompt open, prove the call returned, prove nothing was added, and check the text.

Gaps: W4 and W6, the panic path, and any test of the real prompt.

`the_account_name_is_taken_apart_for_the_logon` is fine for what it can do, with S4. It tests the OS parser and the fallback, not a logon, and it depends on Windows refusing plain names.

## Unresolved questions

1. W1: do Hello with a foreign HWND, the parent window, and PIN-only and Microsoft accounts work? Only a manual run can say.
2. W2: does Hello Face pass with no click on this Windows version? If yes, is a daemon-owned confirmation wanted?
3. S7: should turning phone access back on be app-only? Today any holder of the token can do it, and the owner's list says only that turning off keeps pairings.
4. W5: should unknown labels on Yes-led menus stay refused (safe, blocks use) or be allowed when the question shows no sign of permission?
5. The prompt text is English only, because the daemon has no translations. Is that acceptable on a Vietnamese desktop? (Also raised by the developer.)
6. Phase 3a: the loopback exemption and `transport_cap` must be judged again when `tailscale serve` proxies through loopback, since every remote phone would then look local.
