# Phone phase 3, fourth fix round

All items are done in the uncommitted tree, and every check you listed passes except `mobile_perf`. **`mobile_perf` was not run on a release daemon**: `target/release/kelpied.exe` dates from 06:21 and the newest daemon source from 07:41, so I did not rebuild. The new gate has therefore not yet produced its expected steady fail at about 10 points on the current tree. The macOS `invalidate` change is type-checked only.

## Items

| # | What changed | Test |
|---|---|---|
| 1 W1 | `LASTING` is split into `STRONG` and `WEAK`. A weak word counts only when the label starts with consent or refusal, or the menu is a permission question. `question_lasts` reads strong words only from the question line and the line above it, plus `TRUST`. The description escape now covers `GOES_ON`. A consent-led option with a description under `How should I proceed?` and no tool line stays answerable; without a description it stays hidden | `ordinary_menus_keep_their_buttons_on_common_words` (all eight rows, the no-description form, the tool-line form, a strong word over the question) |
| 2 Corpus | Table of 83 Claude labels and 7 others: the 3 Codex labels from the third review, plus `No, and block this host in the future`, `Yes, and don't ask again for this command (a)`, `Yes, continue` and `Yes, but ask again next time`. Each has an expected result on `Which one?` and on `Allow this?`. No binary was re-scanned | `real_labels` in `crates/kelpied/src/choices.rs` |
| 3 W5 | Lowercase continuation counts as a wrap. `CONSENT \|\| ONCE` for yes-lasts. `TRUST` over all of `above`. `REFUSES` limited to the word alone or followed by `,` `.` `(` or a dash. `don't ask` / `don't prompt` with or without `again`. Remaining rows are one sentence at `docs/architecture.md:428`, and the guide says best effort | `what_is_cheap_to_tell_is_told`, `a_description_with_no_lasting_word_is_read_as_a_description` |
| 4 W2 | `Prompt::plain_answer`, used by `Verb::Answer`: while something lasts, only `esc`, exactly a listed non-lasting key, or Enter/empty text with the marker known to be on a non-lasting key. `menu()` records the marked key in `selected`, which is not serialized. With no marker (y/n lines), Enter is refused. Test comment corrected | `a_phones_answer_to_a_prompt_with_a_lasting_choice_is_one_of_its_plain_choices`; the socket test now also sends `2x`, `2` plus zero-width space, `yes`, `enter` and empty text |
| 5 W3 | Recent first messages by SHA-256 (256 kept, 2 minutes). A repeat is dropped before `slot.answered()` and counts against its address. `note_flood` on both `None` paths and when the budget runs out. A connection past `prefilter` with no sealed frame counts against its address. `architecture.md:450` and `:453` corrected, including that several addresses can still deny service, which leaves a trace | `a_sniffed_first_message_holds_no_place_and_counts_against_its_address`, `a_table_full_of_answered_places_turns_newcomers_away_and_says_so`, `running_out_of_the_budget_is_recorded`; two replay tests rewritten |
| 5 Retries | An honest retry is a fresh first message: `linkCodec.open` and `pair.ts` make a new `Initiator`, and so a new ephemeral key, for each connection | New Vitest in `app/phone/src/link.test.ts` (4 connections, 4 different messages). `mobile_resilience.py`'s reconnect storms also passed with the set active |
| 6 W4 | `mobile_perf.py`: gate figure is `QueryProcessCycleTime` with no ingest correction; ticks and ticks corrected by intake are information only. Unstable above 5 points of spread or below -2. Frame floor is 70 % of `SESSIONS * 5`. Docstring and advice corrected. Conversion printed. Limit and `baseline.json` unchanged | See "`mobile_perf`" below |
| 7 W6 | Message extended in English and Vietnamese, with a test row. `Presence::newly_stuck` gives one `verify_stuck` audit line past 10 minutes, from the 10 s ticker. macOS `Cancel` calls `LAContext::invalidate` | `a_prompt_stuck_for_long_is_said_once`, `phone-backend-messages.test.ts`; macOS code type-checked for `x86_64-apple-darwin` in a scratch crate. The ticker line itself has no test |
| 8 S1 | Lowering a permission that cannot be saved reports `NOT_SAVED_LOWERED`, in both languages | Extended the existing dispatch test; message row added |
| 8 S2 | `status` reports the unsaved text from its own field while `save_pending`, so `enable` or a failed `start` cannot remove it | Extended the existing retry test |
| 8 S3 | Settings shows the daemon's status for "could not be saved", so it goes when the save succeeds | **None:** typecheck only |
| 8 S4 | A `retrying` flag allows one retry task at a time | `a_retry_that_hangs_holds_up_no_other` (the flag, not the ticker) |
| 8 S5 | "or something on this network is interfering" added to the phone text, vi, guide and `architecture.md:480` | `i18n:check` |
| 8 S6 | Three stale comments reworded, and a test comment that repeated one | None needed |
| 8 S7 | The `one_shot` code shows the card's translated line | **None:** typecheck only |
| 8 S8 | One clause in `architecture.md`, and it now reads "Hello and macOS's prompt are cancelled" | None needed |

## Decisions for you to check

- **Additions to the review's strong list.** I added `use auto mode` / `set auto mode`, because `Use auto mode during plan` is a real label that lasts and carries no other strong word. `without (asking|prompting|confirm)`, `yolo` and the rest stay as they were.
- **`disable` as a refusal opener.** It applies only to weak words, so `Disable all LSP recommendations` hides and `Disable` stays answerable on an ordinary question.
- **One old assertion removed.** `Never? 1. Never mind` no longer hides, as W1 intends. `Never mind` is now a table row.
- **Baseline spread.** Rounds without phones more than 5 points apart also report unstable. The brief names only the spread in general, and this is the existing baseline check moved to points.

## False hides the table accepts

On a question naming no permission, these real labels lose their button because `^run|continue|accept` or a permission word makes the menu a permission question:
- `Allow carve-outs`, `Allow possible downgrade to stable version`
- `Accept terms ...`
- `Continue with browser tools`, `Continue with usage credits`, `Continue without ...`, `Continue now at lower priority`
- `Run and only show findings here`

Most don't truly last. In that case the other options in the same menu are judged as a permission question too. This errs safe and arrow keys still work, so I left it under your "no more clever rules". The table pins it, so a later change will show up.

## `mobile_perf`

**On debug, its numbers mean nothing.** The run used `--pair-with` on the debug daemon, default phones and sessions, 2 rounds of 14 s, and gave +84.67 and +95.99 points, `GATE UNSTABLE`, exit 2. It confirms that the conversion line prints, that cycles are read, that rounds print and that the unstable path works:

```
cycles to points: cycles / seconds / 2592 MHz x 100; the first round with phones: 42792752748 / 8.01 / 2592 MHz x 100 = 206.23
```

**Verdict logic.** The real run did not reach the pass or fail branch. I lifted `unstable` out of the script and ran it on 10 made-up cases at the boundaries (5.0 and 5.1 spread, -1.5 and -2.5, 10.4 and 10.5 frames a second, level 2, a steady 10): 10 of 10 as expected. The final pass/fail comparison is two lines and was not exercised.

**The script's limit on a measure:** the window is `--seconds` minus 6, so `--seconds 6` measures nothing. It reports unstable rather than a verdict.

## What I ran

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 693 of 693 |
| `pnpm i18n:check` | 0 missing, 0 problems |
| `pnpm build:phone` | ok |
| `cargo test -p kelpie-link -p kelpie-proto -p kelpied -p kelpie --no-fail-fast` | First run: kelpied 520 passed, 1 failed (below). After the fix, `kelpied` alone: 521 passed, 3 ignored. kelpie 170 (5 ignored), kelpie-link 12, kelpie-proto 23 (2 ignored), from the first run; the fix touched only `kelpied` |
| `cargo clippy` on the crates, `--all-targets` | clean |
| `link_noise.py` | ok |
| `cargo build -p kelpied` | ok |
| `mobile_protocol.py` | 182 of 182 |
| `mobile_resilience.py` | 55 of 55 |
| `pnpm tauri build --debug --no-bundle` | ok; lowest free memory 456 MB, not stopped |
| `mobile.py` | 107 of 107 |

The real daemon (PID 12168) kept running, no test processes are left, and no OS prompt appeared. Disk ended at C 2.16 GB and D 3.34 GB.

## Failures, as they came

1. **New Vitest:** `TypeError` on `sent[0]`. Advancing 60 s let the retry's unopened socket be given up on, so the test read a dead socket. The test now stops as soon as the new socket appears. A test fault.
2. **`a_lasting_choice_has_no_way_to_be_answered_from_a_phone`:** `Enable auto mode?` / `OK` gave `["1","2"]`. My first word, `auto mode`, also matched the question and hid `OK`. I narrowed it to `use|set auto mode` and left the test unchanged. Everything else passed in that run, including `real_labels`.
3. **`mobile_protocol.py` exit 1 after 2 s:** I left out its `<kelpied.exe>` argument, so no check ran. The rerun passed.
4. **`mobile_perf` on debug, first try, unstable:** I passed `--seconds 6`, which gives a window of zero. Rerun with 14.

The scratch macOS crate had gone from the scratchpad, so I recreated it.

## What was not run

- **`mobile_perf` on release:** stale binary, and no release build this round.
- **`stress.py`:** by your instruction.
- **A real OS prompt:** by rule.
- **macOS itself:** `invalidate` has never run.
- **S3 and S7 in the UI:** no automated test exercises them.

## Changes to the manual steps

Steps in `plans/reports/fullstack-developer-261031-0100-phone-phase3-fixes-2.md` and the changes in `plans/reports/fullstack-developer-261031-0550-phone-phase3-fixes-3.md` stand, with these additions:
- **macOS, step 7:** after 280 s the prompt should go away and a new request should be able to ask. If it stays up, that is a finding.
- **Failing save:** make `mobile.v2.json` unwritable and lower a phone's permission. The message says it will go back after a restart. Change the port: the warning stays. Fix the file: within 10 s the warning goes, and also goes from Settings without another action.
- **Marker on a lasting choice:** with the marker on `Yes, and don't ask again`, the phone's Enter is refused and the card shows the translated line.
- **Real phone:** drop Wi-Fi during a reconnect. It comes back by itself, with no `flood` line in the audit log from one drop.
- **At the next release build:** run `mobile_perf`. Expect `GATE FAIL` at about 10 points. `GATE UNSTABLE` or a pass needs explaining before anyone cites it.

## Unresolved questions

1. **The 5-point budget:** still not met, by the debugger's cycle figures of about 10 points, most of it older than encryption. Reduce the cost or change the limit? This is yours to decide.
2. **`Enable auto mode?` / `OK`:** still answerable, and enabling that mode arguably lasts. An existing test pins it, and hiding it needs a new rule, which you ruled out. Say if you want it hidden.
3. **macOS `invalidate`:** I relied on it ending the evaluation with a reply, and on calling it from the thread that gave up. Neither is confirmed on a Mac. If no reply comes, the thread stays stuck, and the new message and audit line cover that.
4. **A shared address:** a repeat counts against its address, so a phone behind the same NAT as someone replaying shares the penalty. A different address is not affected.
5. **Replay after expiry:** a first message older than 2 minutes, or pushed out by 256 other valid ones, can earn one place again. Either way it needs sniffed messages and costs the attacker on its own address after the first.
6. **From the third report, still open:** whether an abandoned CredUI dialog can be closed from the daemon; Hello Face with no click; a window disabled by a parented dialog; pauses ending at a daemon restart; the loopback exemption once a proxy arrives.

Status: DONE_WITH_CONCERNS
Summary: All eight items are done, and every listed check passes, including `mobile_protocol` 182/182, `mobile_resilience` 55/55 and `mobile.py` 107/107. The new `mobile_perf` gate has run only on debug, where its numbers mean nothing.
Concerns/Blockers: The expected steady fail at about 10 points still needs a release daemon. macOS `invalidate` is type-checked only. S3 and S7 have no automated test.
