# Phone phase 3: second fix round

Branch `phone/phase3-link`, uncommitted working tree. Findings: `plans/reports/code-reviewer-261030-2235-phone-phase3-second-review.md` (W, S) and `plans/reports/tester-261030-2235-phone-phase3.md` (P, F). Nothing staged, committed, pushed or stashed. No installer built, no real OS prompt raised, the user's daemon not touched.

## Items

| # | What changed | Test |
|---|---|---|
| 1 W3 time limit | `Presence::verify` runs the prompt on its own thread and waits at most `PROMPT_LIMIT` (280 s; the app waits 300 s). On expiry: an error reply, the slot is free, `Cancel::fire` calls `IAsyncOperation::Cancel` for Hello and nothing falls through to the password dialog. CredUI cannot be cancelled: its answer goes to a closed channel, so a late pass cannot run the action. Limit injectable with `set_limit` | `a_prompt_past_its_time_answers_an_error_frees_the_screen_and_its_late_pass_adds_nothing`, `a_prompt_past_its_time_is_an_error_and_its_late_pass_goes_nowhere`, `taking_a_prompt_back_works_at_any_moment` |
| 2 W2 audit and pause | Each refused or failed verification writes `verify_failed` with the prompt's English text and the reason. Three within 5 minutes stop new prompts for 5 minutes (`verify_paused`, a clear message naming the kill switch). A pass clears the count. Declining, lowering and the kill switch are unaffected. Kept in memory, so a daemon restart ends a pause (stated in the docs). No daemon-owned confirmation dialog | `a_refused_verification_leaves_a_line_and_three_stop_the_prompts_for_a_while`, `three_refusals_in_a_few_minutes_stop_new_prompts_for_a_while`, e2e in `mobile_protocol.py` |
| 3 S2 panics | A panic in `verify` ends the channel; a panic in `then` is caught. Both produce one error reply, and the slot is free | `a_panic_around_the_prompt_still_answers_and_frees_the_screen`, `a_panic_in_the_prompt_still_ends_the_wait` |
| 4 S3 | The kill switch button is never disabled | typecheck only; no UI test |
| 5 W1 parent | Nothing added. The one line to change is commented in `os::verify`, at `valid_window` | none needed |
| 6 language | `Prompt` + `Lang`: the daemon's own English and Vietnamese table. `ipc.ts` adds `lang: locale()`; unknown means English. A caller's `reason`, `text` or `prompt` is ignored | `a_prompt_is_in_the_language_asked_for_and_otherwise_in_english`, `a_prompt_is_in_the_language_the_app_names_and_a_caller_supplies_no_text` |
| 7 W5, S1, S10 menus | `asks_permission`: a word of consent among the choices **and** a signal (question wording, a tool line above, or a choice that speaks of allowing). Without one, only `LASTING` words hide a choice. `CONSENT` gains proceed, run, continue, confirm; `LASTING` gains "without asking". A y/n question that itself lasts marks `y`. `a` and `A` stay two keys where the prompt lists both, and `choice_lasts` honours that | `a_question_that_is_not_about_permission_keeps_its_buttons` (both W5 menus), `a_tool_line_or_a_question_of_permission_keeps_unknown_labels_refused`, `the_question_of_a_yes_or_no_can_be_the_lasting_part`, `keys_are_told_apart_by_case_only_where_the_prompt_does`. **No real AskUserQuestion capture exists in the repo**, so the form test uses the reviewer's traced shape |
| 8 W6 slots | `Handshakes` table: when all 16 are taken the oldest gets `given_way` and its connection ends; the newcomer is admitted. The 4-per-address limit stays. Later: the loopback exemption must be judged again when a proxy on loopback arrives | Socket level through `accept`: `holding_every_place_gains_nothing_however_late_the_garbage_comes`, `one_address_holds_its_share_and_silence_counts_against_it` (both with garbage at 4.9 s; the second uses a test-only flag that holds loopback to the address limit, since loopback is all a test can bind) |
| 9 W7 | `link.ts`: 3 connections in a row that sent their first message and were closed with no reply and no stated code, while the page's server answers, show `not_paired_here` with reason `reset`. The key stays until Forget; retry every 60 s and on `kick()`. Docs corrected. Vietnamese added | `link.test.ts`: the timing, the key staying, recovery, and not firing when unreachable, never opened, or on a stated close |
| 10 P1 | `screen-sync.ts`: a whole frame prunes nothing; a frame with `base` prunes below it; `HELD_FRAMES = 8`, twice the daemon's `WINDOW_FRAMES`, drops the oldest beyond that. `phone_support.py` `_apply` changed the same way | `screen-sync.test.ts`: the tester's sequence, then 100 whole frames holding exactly 8 |
| 11 S4, S9 | The given domain is used when the parser yields none. **UPN pinned:** the OS returns `me@corp.example` whole with no domain, which goes to the logon with a null domain. `logon_refusal` words a wrong password apart from an account Windows refuses and from a check that failed | `the_account_name_is_taken_apart_for_the_logon`, `a_refused_logon_says_what_is_wrong` |
| 12 S5 | `save`: copy under the config lock, release, then write. A separate `saving` lock keeps saves in order, so an older copy cannot overwrite a newer one (the lock alone provided that before) | `a_slow_disk_does_not_hold_up_what_reads_the_settings` |
| 13 S7 | `phone_access_on` and `phone_access_off` audit lines, only on a real change. Permission unchanged | `turning_phone_access_on_and_off_is_in_the_audit_log` |
| 14 F11 perf | `mobile_perf.py --pair-with <debug kelpied>`: pairs there, stops it, starts the named daemon on the same endpoint and data folder as the plain user (`Daemon(as_app=False)`), which cannot raise a prompt. Header corrected. New verdict `GATE UNSTABLE`, exit 2, when no-phones cost per MB/s spreads over 25 %, or the phones' cost ranges over 10 points between identical runs. The 5 % limit and `baseline.json` are untouched | Run for real on a new release build, see below |
| 15 docs | `architecture.md`: the file route and `dormant_days` stated plainly, failed-attempt wording fixed, `shift-tab` added, time limit, pause, language, eviction, held screens, the Hello Face open question. `user-guide.md`: only the phone section; the foreign hunks at 351 and 422 are unchanged (checked with `git diff -U0`) | read against the code |

W4 was already fixed by the tester's edit: `mobile.py` expects keys `["1","3"]` and taps `"3"`. Left as it is.

One decision needing evidence: item 7 adds a condition to yours. Besides a signal, a menu must contain a word of consent. Without that, `Which file should I edit?` with `1. a.rs` / `2. b.rs` would have every label refused because of the word "edit". Lasting words still hide regardless.

## What I ran

Final results on the finished tree. Load is `Win32_Processor.LoadPercentage`, sampled where given.

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 55 files, 674 / 674 |
| `pnpm i18n:check` | 2277 used, 0 missing, 0 problems |
| `pnpm build:phone` | built |
| `cargo test -p kelpie-proto -p kelpie-link -p kelpied -p kelpie` | kelpie 170 pass, 5 ignored; kelpie-link 12; kelpie-proto 23 pass, 2 ignored; kelpied 499 pass, 3 ignored; 0 failed. `twenty_cached_summaries_are_fast` and the `agents.rs` race (tester F1, F2) did not fail this time; I left both alone |
| `cargo clippy -p kelpie-link -p kelpied -p kelpie --all-targets` | 0 warnings |
| `link_noise.py` | ok |
| `mobile_protocol.py` | 182 / 182, load 57 after |
| `mobile_resilience.py` | 55 / 55, load 51 after. No lost frame at 300 ms latency |
| `pnpm tauri build --debug --no-bundle` | built in 5 m 32 s |
| `mobile.py` through the tester's `run-with-load.py` (own endpoint and data folder, seam `approve`) | 107 / 107 on its one run, CPU 78-96 %; daemon check OK, `verification seam: approve`, test daemon gone afterwards |
| `cargo build --release -p kelpied` | built in 18 m. D stayed at 4.3 GB, C ended at 2.39 GB, both above the 1.5 GB line |
| `cmdkey /list` | no `mobile-link-key` or `mobile-device-key` entries |
| Real daemon | still `...\Kelpie\daemon\a\kelpied.exe`; the only other daemons were mine, and they are gone |

**`mobile_perf.py` on the release daemon, paired with `--pair-with target/debug/kelpied.exe`: the 5 % budget is still not shown to be met.**

| Run | Verdict | 8 phones add | Baseline | Load |
|---|---|---|---|---|
| 1 | **GATE FAIL**, exit 1 | **+5.3** points (+1.9, +5.3, +7.2) | agreed within the limit: 20.0 % of a core per MB/s | 100 at end |
| 2 (the one retry) | **GATE UNSTABLE**, exit 2 | +8.1 points (+8.5, +1.0, +8.1) | 18.4 to 38.4 % of a core per MB/s | 56 before, 57 after |

- Zero frames for sessions nobody follows on both runs. Event-loop lag p99 20-30 ms, levels 1-2, so the daemon was shedding some work.
- This replaces the tester's debug result of +29 to +43 points: on release the cost is far lower, about the size of the limit. **I read this as an open result, not a pass:** one run missed by 0.3 points with the machine busy, and identical runs still range from +1 to +8.5. It needs a quiet machine before anyone calls it. I did not touch the limit or `baseline.json`.
- `--rounds 2 --seconds 8 --phones 2` on debug-paired-on-debug was run first to prove the pairing path; its numbers mean nothing.

Failures as they came:

- **`cargo test -p kelpied`, first build:** did not compile, a wrong type name in my new daemon test.
- **`link.test.ts`, 2 runs:** my new test advanced the clock past the 8 s connect timer, so `r.last()` was a dead socket. That also exposed a real flaw: a connection that never opened was resetting the count of no-reply closes. Fixed in `link.ts`, so only a reply or a stated close resets it, and the test now waits for the next connection.
- **`a_message_whose_enter_was_skipped_says_so`, first full `kelpied` run, 498 passed, 1 failed:** the typist's 80 ms sleep overran the 250 ms Enter delay. Time-dependent, not phone-link logic. Passed alone at load 100, and in the later full run. Not loosened.
- **`spec-index.test.ts` "parses a 256 KB list in one piece, whole", `pnpm test`, then alone:** 5.6 s against the 5 s limit, load 100. Not phone code and not changed by me. Passed with `--testTimeout=60000` to prove the logic, and passed unchanged in the later full suite at 674 / 674. Not loosened.
- **clippy, first run: 1 warning** in my test (`is_ascii`), fixed.
- **`the_account_name_is_taken_apart_for_the_logon`:** passed first time, so the pinned UPN result is what the OS returned, not an adjustment.

## What was not run

- A real OS prompt, by rule. `os::verify`, `IAsyncOperation::Cancel`, `logon_refusal` on a real logon and the parent window have never run.
- macOS. The macOS branch changed slightly (the `Cancel` parameter) and I did not re-run the alone-in-a-scratch-crate type check, since the block code did not change. `--pair-with` was not tried with a macOS build.
- `stress.py`, `acp_chat.py`, the perf gate's GUI and bundle metrics: not in this brief. `stress.py` uses `phone_support`, whose `_apply` and `Daemon` changed.
- A real AskUserQuestion capture: none exists in the repo. Worth taking one during the manual run.

## Manual steps for a person

No real prompt has ever been raised, on any machine. Use a dev pair so the real daemon stays out of it, and do **not** set `KELPIE_TEST_USER_PRESENCE`:

```
bash scripts/dev-daemon.sh run                         # shell 1
cd app && KELPIE_ENDPOINT=kelpie-dev pnpm tauri dev    # shell 2
```

Pair from Settings → Mobile → "Pair a browser on this network", open the link in a browser, name the device, type the six digits, click "Allow reading".

**Windows Hello**
1. Record: does the Windows Security dialog appear, is it in front of Kelpie, does it take focus, and does its text read `Kelpie: allow the phone “<name>” (key XXXX XXXX XXXX XXXX) to read your sessions on this computer.`
2. **Hello Face, the open question:** with face recognition set up, look at the screen. Does it pass with no click at all? If so, a person merely looking at the screen approves whatever a same-user program asked, and the daemon-owned confirmation dialog held back in this round is needed.
3. If the password dialog appears instead, Hello refused the app's window handle: look for `Windows Hello:` in the daemon's log.
4. **While the prompt is up, is Kelpie's window disabled?** Switch sessions and type in a terminal. If the window takes no input, pass `HWND::default()` at the one commented line in `os::verify` in `crates/kelpied/src/user_presence.rs`, and retest step 1. Also kill the dev daemon during a prompt and check that the window recovers.
5. While it is up, the kill switch stays clickable. Changing a permission says "already asking".
6. Cancel: "Verification was cancelled.", the request stays, no device, and "What phones did" shows `verify_failed`. Cancel three times: the next attempt says Kelpie will not ask again for 5 minutes, and `verify_paused` is in the log.
7. Leave a prompt unanswered for 280 s: the app shows "not answered in time", Hello's dialog goes away, and a new request can raise a prompt. For a password dialog, it stays on screen; pass it afterwards and check nothing is added.
8. Language: with Kelpie in Vietnamese the prompt is Vietnamese, in English it is English. Check the Vietnamese reads well.
9. Raise read to write: prompt. Lower: none. A longer dormant period: prompt. Shorter: none. Reset: prompt.

**Password dialog (a user with no Hello, or an RDP session)**
10. The credential dialog appears with the same text. Record whether it comes to the front and whether Kelpie's window is disabled while it is up, and normal afterwards.
11. Wrong password: "That is not the password…". Another valid local account's password: "That is not the account signed in to this computer." The right one: paired.
12. Try each that applies: local, domain, Microsoft account (`MicrosoftAccount\me@outlook.com` and the plain email), PIN-only, blank password (should say Windows would not sign that account in). A correct password that is always refused points at `CRED_PACK_FLAGS(0)`: try `CRED_PACK_PROTECTED_CREDENTIALS`.

**macOS:** never run. Check `LAContext` shows a prompt from a daemon that is not an app bundle. An abandoned Touch ID prompt is not cancelled; only its answer is ignored.

## Unresolved questions

1. Hello Face with no click, and a window disabled by the parent: only the manual run can say.
2. A pause ends when the daemon restarts, and any holder of the token can restart it. Accepted for now as the same class as the file route; say if it should persist.
3. The per-address exemption for loopback must be judged again when a proxy on loopback arrives (phase 3a).
4. A flood of connections can give way a real phone's handshake if 16 newer ones arrive within its few milliseconds, and silent drops can also show "may have been reset" until the next try. Both only delay.
5. Should the perf run be pinned calm (`KELPIE_LOAD_FORCE`)? Unpinned, the daemon sheds work at L2/L3 during measurement. I left it, since you did not ask.
