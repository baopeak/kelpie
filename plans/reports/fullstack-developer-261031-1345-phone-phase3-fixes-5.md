# Phone phase 3, fifth fix round

The review's blocker and all five suggestions are fixed in the uncommitted tree, and every check you listed passes. `mobile_perf.py` was tested on made-up cases only, as you asked, so its new verdict has not seen a real measurement.

## Items

| # | What changed | Test |
|---|---|---|
| W1 | `for (this\|the) (project\|repo\|...)` moved from `STRONG` to `WEAK`; `\ball\b` in `WEAK` replaced with `\bfuture\b`. Nothing else in the menu rules changed | The six rows added to `ordinary_menus_keep_their_buttons_on_common_words`. `real_labels`: `Disable all LSP recommendations` is now `false, true`, and no other row moved |
| W1 residual | `Allow this for this project?` / `1. Yes` is answerable; `Yes, for this project` stays hidden. Stated in `docs/architecture.md`'s best-effort sentence | `a_scope_named_only_in_the_question_leaves_a_plain_yes_answerable` |
| S1 | `cycles()` raises if the process cannot be opened, the call fails, or it reads 0. `steady` raises if a round's cycles did not rise. Fewer than 2 rounds or 0 sessions is a usage error before anything starts | Logic check, below. The `steady` guard runs only in a real measurement, so it was not exercised |
| S2 | `min(extra) > LIMIT_POINTS` is `GATE FAIL` whatever the spread, in a new `verdict()` that `main` prints. The unstable message and docstring now say a busy machine cannot settle it, citing no-phones rounds from 15.2 to 41.1 points with the host at 100 % | Logic check, below |
| S3 | `save` clears `unsaved_text` on success, which replaces the clearing in `retry_save`. `NOT_SAVED_LOWERED` no longer replaces another unsaved message; the reverse still works | New `a_save_that_gets_through_ends_the_warning_of_the_one_before`; two assertions added to `a_removal_that_cannot_be_saved_says_so_and_is_saved_when_it_can_be` |
| S4 | `sendText` and `sendKey` map `one_shot` to the phone's existing translated line. `composer.tsx:108` now uses the key bar's three-way message. `lastingText`, `refusalText` and `keyResultText` live in `app/phone/src/notice.ts`, used by the card, key bar and composer | New `what a refused key or text says` in `app/phone/src/fixes.test.ts` tests those functions; the components themselves have no test |
| S5 | `architecture.md` failure list now points to "What a first message costs". One clause added for what `answer` accepts while a lasting choice is on screen | Read against the source; no automated test |

## `mobile_perf.py` logic check

`C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad\perf-verdict-logic.py` lifts the functions out of the script. No daemon starts. All 26 checks came out as expected:

| Group | Checks | Covers |
|---|---|---|
| Verdicts | 18 | Every round over the limit (close together, far apart, no-phones rounds from 15.2 to 41.1, level 2): `FAIL`. Also the lowest round exactly at the limit, median or first round deciding, the 5.0 and 5.1 spreads, -1.5 and -2.5, 10.4 and 10.5 frames a second. Each `UNSTABLE` also checks for the busy-machine sentence |
| `cycles()` | 5 | Process cannot be opened, call fails, call reads 0 (all raise), a good reading through a stand-in, and this process's real cycles |
| Usage | 3 | `--rounds 1`, `--rounds 0`, `--sessions 0`: exit 1 with the usage message, using a daemon path that does not exist |

## What I ran

| Check | Result |
|---|---|
| `cargo test -p kelpied choices` | 37 passed |
| `cargo test -p kelpied mobile` | 135 passed, 1 ignored |
| `cargo clippy -p kelpied --all-targets` | clean |
| `pnpm typecheck` | clean |
| `pnpm test` | 695 of 695 (after failure 2 below) |
| `pnpm i18n:check` | 0 missing, 0 problems |
| `pnpm build:phone` | ok |

The real daemon (`%LOCALAPPDATA%\Kelpie\daemon\b\kelpied.exe`, PID 268) stayed up, and no test processes are left. No build was stopped for memory; free memory read 1.5 to 2.2 GB at each check.

## Failures, as they came

1. **Logic check, first run:** `AttributeError` on `used._obj` in my stand-in for `QueryProcessCycleTime`. A fault in the check, not the script. Fixed to `used.contents`.
2. **`pnpm test`, first run:** `fixes.test.ts` could not load, because `notice.ts` now imports `./i18n`, which needs a build-only virtual module. The other 680 tests passed. I added the same `vi.mock("./i18n", ...)` that `pair.test.ts` uses, and the rerun passed 695 of 695.
3. **`cargo test -p kelpied choices`:** the command passed the tool's 178 s limit while compiling and moved to the background. It finished with exit 0; I did not restart it.

## What was not run

- **`mobile_perf.py` for real:** by your instruction.
- **The e2e scripts:** left to the separate tester, by your instruction.
- **`cargo build -p kelpied`:** not on your list, so `target/debug/kelpied.exe` predates these edits. **The tester must rebuild it first**; `pnpm tauri build --debug --no-bundle` is needed too before `mobile.py`. `dist-phone` is already current.
- **The rest of the Rust suite:** only the `choices` and `mobile` filters ran, plus clippy.
- **`stress.py`, a real OS prompt, a release build:** by rule.

## Worth knowing

- **A lowering while another save is waiting.** The reply says the permission will revert, but Settings shows the status, which keeps "Removed for now". So on screen the person reads the stronger message, not the lowering one. The audit log has both. This is what the review asked for.
- **`FAIL` whatever the spread can catch noise.** Every round is measured against the median of the no-phones rounds. With those at 15 to 41 points, a low median can put every round over the limit on a busy machine. I applied your rule as written and pinned this case as `FAIL`.
- **The 15.2 to 41.1 figures are fixed text** in the script's message and docstring. They record your last three runs, not the current run.

## Unresolved questions

1. **`--seconds 6` or less:** the script's measuring window is `--seconds` minus 6, so nothing is measured. It used to end as unstable, and it could now print `GATE FAIL` if every noisy round lands over the limit. It is outside your list, so I left it. Do you want it added to the usage errors?
2. **The 5-point budget:** still your decision. `mobile_perf` has still not run on a release daemon built from this tree.

Status: DONE
Summary: The review's blocker and S1 to S5 are fixed and pinned, and every listed check passes: `choices` 37, `mobile` 135, `pnpm test` 695 of 695, clippy clean. The `mobile_perf.py` logic check passed 26 of 26 on made-up cases.
Concerns/Blockers: The tester must rebuild the debug daemon and app before the e2e scripts. Whether `--seconds 6` or less becomes a usage error is your call.
