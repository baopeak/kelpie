# Phone phase 3: third fix round

Branch `phone/phase3-link`, uncommitted working tree. Findings: `plans/reports/code-reviewer-261031-0250-phone-phase3-third-review.md` (W, S) and `plans/reports/tester-261031-0250-phone-phase3.md`. Nothing staged, committed, pushed or stashed. No installer built, no real OS prompt raised, the user's daemon not touched, `stress.py` not run.

## Items

| # | What changed | Test |
|---|---|---|
| 1 W1 lasting choices | `LASTING` gains `sessions?`, `in the future`, `block`, `never`, and `don't/do not ask/show/prompt/remind (me) … again`. The three lines above a menu, and above a `(y/n)`, are tested against `LASTING` and a trust word; a hit marks the consent-led keys, or `y`. A question naming `allow`, `permission`, `permit`, `grant` or `trust` is a permission question on its own. `enable`, `grant`, `use`, `turn on` are consent words. `ONCE`'s parenthetical is limited to a letter, `esc`, `enter`, `tab`, `shift+tab`. A lasting item's letter goes into `lasting`. The assertion on the trust menu is reversed | `a_lasting_choice_has_no_way_to_be_answered_from_a_phone` (every row of the review's table, with the real labels), `a_no_that_comes_back_next_time_stays_answerable`, `choices_that_outlast_the_prompt_are_named` |
| 2 S1 | `ONCE` accepts `but ask again next time`. A label's first line alone counts for `ONCE` when the rest is a description. An action verb with choices that carry descriptions is not a permission question | `an_action_alone_does_not_make_a_question_of_permission` (`Should I create a PR?`, `Which file should I edit?`), `a_real_one_time_yes_with_a_condition_or_a_description_is_answerable` (`Allow this read outside the working directories?`) |
| 3 W2 handshakes | A slot leaves the evictable set once its first message passes `prefilter`. With every slot like that a newcomer is refused and nobody is let go. An eviction before any first message counts against its address. Per-address upgrade bucket before `on_upgrade` (5 a second, burst 20, answered 429). Per-address share of the handshake budget (5 a second). `flood` audit line at most once in 5 minutes. Loopback keeps its exemption | `a_phone_in_the_middle_of_its_handshake_outlasts_any_number_of_newcomers`, `a_connection_let_go_before_its_first_message_counts_against_its_address`, `a_replayed_first_message_from_one_address_leaves_the_budget_to_the_others`, `upgrades_are_limited_per_address_before_any_task_exists`, `a_flood_that_goes_on_is_in_the_audit_log_once_in_a_while` |
| 4 W3 | `save` returns `Result`, `keep` logs and leaves it for the retry. `revoke`, `revoke_all`, `disable`, `reset` reply with `NOT_SAVED_REMOVED` or `NOT_SAVED_OFF`, set `last_error` and audit `save_failed`. **One change from your wording:** the retry is in the daemon's ticker, every 10 s, not the sweeper, because the sweeper stops with phone access, which is exactly when this matters. English and Vietnamese | `a_removal_that_cannot_be_saved_says_so_and_is_saved_when_it_can_be` (a real failing write: a folder where the file goes; then recovery, and the file a restart would load) |
| 5 W6, S8, S9, S10 | Timeouts count over 20 minutes (3 pause). While a prompt's thread outlives its time, `begin` says "close the earlier dialog first"; its end clears that. The refusal is noted before the turn is freed. `quoted` also filters U+2028, U+2029, U+061C, U+FEFF, U+2060. S10: the simpler of your two options, fixed daemon messages translated by the app through the existing `errorText` and `locales/vi/backend.json`; the pause message lost its minute count so it stays one fixed sentence | `prompts_left_unanswered_stop_new_ones_over_a_longer_window`, `a_prompt_past_its_time_is_an_error_and_its_late_pass_goes_nowhere`, `a_name_cannot_end_its_quote_or_turn_the_text_around`, `app/src/lib/phone-backend-messages.test.ts` (each message still in the Rust source, and translated) |
| 6 phone | S2 `drop` reads `codec.ready()` first. S3 `findOutWhy` in the reset branch. S4 `start` resets the count. S5 `resume` restores the whole acknowledged view. S7 eviction by arrival order, mirrored in `phone_support.py` | `link.test.ts` and `screen-sync.test.ts`, one test each |
| 7 S6 | `Views::last_rev`, one counter for the daemon run, replaces the per-slot counter | `a_frame_number_outlives_its_slot_so_another_window_is_not_mistaken_for_it` |
| 8 S11 | Module comment in `link.rs` reworded | none needed |
| 9 W4, perf | See below | Run for real |
| 10 W5 docs | `user-guide.md` phone section: no button for a lasting choice, but typing can reach one; turning off is not cutting off. Same in `architecture.md`, plus every behaviour above. **One word differs from your brief:** the guide says *Unpair*, because that is what the button for a device is called; *Remove* is only the button for token-era entries | read against the code |

**A reading of items 1 and 2 to check.** They pull apart on `Should I create a PR?`: item 1 says an action verb with a consent word is a permission question, item 2 says this one must stay answerable. I separated them by descriptions: `May I run it?` with `Yes, ship it` still hides that option; choices with descriptions under them do not. A narrow pane that wraps an unknown label therefore looks like a description, but only under an action verb: a named permission, a tool line or a lasting word still hides it. `Type something` and `Other` count as saying no.

## `mobile_perf`: at full rate the budget is not met

**On a release daemon pinned at L0, 8 phones cost about 6 to 9 points of a core against the limit of 5, on a stable baseline. All four runs: GATE FAIL.** This replaces the tester's +3.4 to +3.7, which were measured with the daemon shedding at L2/L3.

Changes: frame-rate floor (2 frames/s/phone), a dead reader fails the run, `quiet_id` restored after `flat_out`, `median(extra)` below minus the spread limit reports unstable, every round printed with its level, `--warm-up`, both daemons pinned with `KELPIE_LOAD_FORCE=L0:calm`. **The pin holds in a release build:** `load.rs` `forced()` is not behind any `cfg`, and every round reported level 0. The limit and `baseline.json` are untouched.

| Run | Load before / after | Per round, points added | Median | No phones, % of a core per MB/s | Frames/s/phone | Level | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | 100 / 45 | +6.0, +11.6, +8.9 | **+8.9** | 15.2 to 16.2 | 14.2 to 14.4 of 18.4 kB | 0 | GATE FAIL |
| 2, `--warm-up` | 51 / 50 | warm-up +12.0; +7.8, +7.9, +6.4 | **+7.8** | 15.3 to 17.4 | 14.3 to 14.35 | 0 | GATE FAIL |
| 3 | 56 / 43 | +8.9, +5.4, +8.9 | **+8.9** | 16.5 to 19.1 | 14.1 to 14.4 | 0 | GATE FAIL |
| 4, final tree | 38 / 43 | +4.2, +7.0, +6.0 | **+6.0** | 15.0 to 16.9 | 14.3 to 14.35 | 0 | GATE FAIL |

0 frames for the session nobody follows on every run. Event-loop lag p99 20-30 ms.

**Is a result at this load meaningful?** Yes, for this. The pin does not make it meaningless: the daemon did the full work (14 frames/s/phone, more than twice the tester's 5.85), the baseline agreed within the 25 % limit on every run, and the load samples at the start and end of the runs were 38-56 % apart from one 100. A busy host can inflate CPU time through cache contention, and the lowest median came from the quietest run, so a quiet machine may read lower: +6.0 is not far off the limit. That is one run, not proof. It stays a failure until a run says otherwise.

**The first round.** I could not find a one-time cause, so it counts: the gate now fails if the first round with phones is over 5 points, as well as on the median.
- The pattern the tester saw, highest in every run, does not hold at L0: the first round was the lowest of runs 1 and 4 and tied for highest in run 3. So most of what he saw came from shedding: later rounds shed more, not the first costing more. At L0 every round costs about what his first rounds did.
- Whole frames on follow and handshakes are ruled out by the timing: they come before the measured window opens, 3 s after the printers start.
- A cold process is neither shown nor ruled out: the first round without phones, the first printing in a default run, was in the middle in runs 1 and 4 (15.4 and 16.5 % per MB/s) and the highest in run 3 (19.1).
- One thing remains: in `--warm-up`, the very first printing round with phones cost +12.0 against +6.4 to +7.9 after it. That is one observation, within the +5.4 to +11.6 seen in other rounds, and I cannot explain it.

**Where the cost goes: not measured.** Phones receive about 2.1 MB/s of sealed frames (8 x 14.2 x 18.4 kB). A software ChaCha20-Poly1305 should spend well under 2 points on that, which would leave most of the cost in rendering, diffing and JSON, but that is an estimate, not a profile. Whether encryption is what broke the budget would need the same run on a daemon from before encryption; there is no such comparison yet.

**This is a decision for you, not a failure I can fix within the limit.** Options: find the cost with a profile and reduce it; reduce what is sent at L0; or decide that 8 phones each following 3 flooding sessions is beyond the case the budget is for. I changed nothing to bring the number down.

## What I ran

Final results. Load is `Win32_Processor.LoadPercentage`, sampled where given.

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 56 files, 691 / 691 |
| `pnpm i18n:check` | 2277 used, 2453 translated, 0 missing, 0 problems |
| `pnpm build:phone` | built |
| `cargo test --no-fail-fast` on the four crates | kelpie 170 pass, 5 ignored; kelpie-link 12; kelpie-proto 23 pass, 2 ignored; kelpied 511 pass, 3 ignored; 0 failed. Load 48 before, 42 after |
| `concurrent_hook_settings_are_published_as_complete_json`, `only_the_shown_agent_is_raised_and_nothing_is_lowered` | **neither failed**, in either full `kelpied` run. Not changed |
| clippy on the three crates, all targets | 0 warnings |
| `link_noise.py` | ok |
| `mobile_protocol.py` | 182 / 182, load 50 after |
| `mobile_resilience.py` | 55 / 55, load 42 after |
| `pnpm tauri build --debug --no-bundle` | built in 3 m 23 s |
| `mobile.py` once, through `run-with-load.py` | 107 / 107, seam `approve`, CPU 65-77 %, test daemon gone afterwards |
| `cargo build --release -p kelpied` | built in 11 m 02 s |
| `mobile_perf.py` on it | GATE FAIL on all 4 runs, table above |
| `cmdkey /list` | no `mobile-link-key` or `mobile-device-key` entries |
| Real daemon | still `...\Kelpie\daemon\a\kelpied.exe`, untouched |
| Disk at the end | D 3.53 GB, C 2.24 GB |

`mobile_protocol`, `mobile_resilience` and `link_noise` ran one small edit before the final tree: after them I removed a redundant `map_err` in `save` that clippy flagged, which changes no behaviour. Everything after them, including the release daemon and the four crates' tests, is the final code.

Failures as they came:

- **`pnpm typecheck`:** my new test imported `node:fs`, which the project's types do not have. It now uses `import.meta.glob` with `?raw`, as `i18n.ts` does for the locales.
- **`cargo test -p kelpied`, first run, 510 passed, 1 failed:** `a_lasting_choice_has_no_way_to_be_answered_from_a_phone`, on my row `Use this tool?` with `Grant`. `grant` named permission in a question but not in a choice. Added to `PERMISSION_CHOICE`; the test was not changed.
- **`cargo check`, 1 warning:** a `save` result unused in a test. **clippy, 1 warning:** the redundant conversion above. Both fixed.
- **`mobile_perf`:** GATE FAIL each time, as above. I gave it more runs than the one retry because the first-round question needed them. No time-dependent check failed otherwise.

## What was not run

- `stress.py`, by your instruction. A real OS prompt, by rule. macOS.
- `mobile_perf` on a quiet machine, which is the measurement that would settle the budget.

## Changes to the manual steps

Steps in `plans/reports/fullstack-developer-261031-0100-phone-phase3-fixes-2.md` stand, with these changes:
- **Step 7:** after 280 s the app says "not answered in time". With Hello, its dialog goes and a new request can ask. With a password dialog still up, a new request now says "close the earlier dialog first"; close it, and the next can ask. Passing the stale dialog must add nothing.
- **Add, Windows:** with the password dialog past its time, check whether closing it works and whether Kelpie's window is usable meanwhile.
- **Add, failing save:** optional. Make `mobile.v2.json` unwritable in a dev data folder, press *Turn off phone access and revoke all*: the message says it will come back after a restart; fix it, and within 10 s the warning goes.
- **Add:** take a real AskUserQuestion capture from a Claude session and compare it with `a_question_that_is_not_about_permission_keeps_its_buttons`.

## Unresolved questions

1. Whether an abandoned CredUI dialog can be closed from the daemon: only a manual run can say. Until then a dialog nobody closes blocks new prompts (not the kill switch) until the daemon restarts.
2. Hello Face with no click, and a window disabled by a parented dialog: still untried.
3. A pause, and the record of timeouts, end when the daemon restarts.
4. The loopback exemption must be judged again when a proxy on loopback arrives.
