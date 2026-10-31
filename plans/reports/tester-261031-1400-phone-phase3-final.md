# Final independent test: phone link (phase 3)

Branch `phone/phase3-link`, uncommitted working tree, HEAD `d6e5883`. Run 2026-10-31 14:00 to 14:57 on Windows, 8 GB RAM. Nothing was committed, staged, stashed or pushed. No product code, test or script was changed by me.

## Verdict

- **The tree changed during my run, and I did not change it.** `crates/kelpied/src/mobile.rs` was edited at 14:44:33, after `mobile.py` had finished. It is test code only, and it targets the one phone failure I found (F1). So the main run below is of the tree before that edit, and I reran what the edit could affect.
- **On the tree before the edit, a phone test failed 7 of 12 runs:** `the_handshake_budget_drops_silently_and_garbage_does_not_touch_it`: in the full run, in its one retry alone, and in 5 of 10 extra runs alone. A time window in the test, not a product bug.
- **On the current tree that test passes:** `cargo test -p kelpied --bin kelpied` 523 pass, 0 fail, and the test alone 12/12 at load 65-100, a heavier load than where it had failed half the time.
- **Everything else in the phone work passed:** `mobile.py` 107/107, `mobile_protocol` 182/182, `mobile_resilience` 55/55, `acp_chat` 56/56, `link_noise`, clippy with 0 warnings, frontend 695/695. Each e2e ran once and passed the first time.
- **One failure outside phone code:** `twenty_cached_summaries_are_fast` took 51.49 ms against a 50 ms limit, and passed alone (F2).
- **`mobile_perf.py --seconds 6` refuses as asked:** exit 1, a usage message, nothing started.
- **The real daemon and app were not touched:** same PIDs and start times at all 18 checks.

## Results

Load is `Win32_Processor.LoadPercentage` at the start/end of each command. Times include compilation. The `step.ps1` and `wait.ps1` scripts were copied from `t1240` with only their log folder changed.

### Main run, on the tree before the 14:44:33 edit

| Step | Command | Result | Counts | Time | Load |
|---|---|---|---|---|---|
| 1 | `pnpm typecheck` | PASS | exit 0 | 51 s | 100/79 |
| 1 | `pnpm test` | PASS | 56 files, 695/695 | 33 s | 54/74 |
| 1 | `pnpm i18n:check` | PASS | 2277 used, 2454 translated, 0 missing, 0 problems, 0 untranslated-looking | 4 s | 59/72 |
| 1 | `pnpm build:phone` | PASS | main chunk 193.37 kB (65.23 kB gzip) | 57 s | 73/77 |
| 2 | `cargo test -p kelpie-link -p kelpie-proto -p kelpied -p kelpie --no-fail-fast` | **FAIL** exit 101, F1, F2 | 728 run, **2 failed**, 10 ignored. kelpie 169 pass, **1 fail**, 5 ignored; kelpie-link 12 pass; kelpie-proto 23 pass, 2 ignored; kelpied 522 pass, **1 fail**, 3 ignored | 771 s | 61/74; 100 at my one sample while kelpied's tests ran (14:14:50) |
| 2 | F1 test alone, `-p kelpied --bin kelpied` | **FAIL** exit 101 | 0 pass, 1 fail, 0.34 s in the test | 139 s, recompiled | 100/79 |
| 2 | F1 test binary alone, 10 more runs (extra, see F1) | **5 of 10 FAIL** | 5 pass, 5 fail, 0.22-0.36 s each | 13 s | 29-63 |
| 2 | F2 test alone, `-p kelpie --lib` | PASS | 1/1, 15.99 s | 20 s | 91/85 |
| 2 | `cargo clippy -p kelpie-link -p kelpied -p kelpie --all-targets` | PASS | 0 warnings, 0 errors | 63 s | 50/51 |
| 3 | `python -X utf8 scripts/e2e/link_noise.py` | PASS | Rust vectors and published Noise IK / IKpsk1 vectors ok | 2 s | 53/62 |
| 3 | `cargo build -p kelpied` | PASS | exit 0, binary 14:24:21 | 91 s | 93/53 |
| 3 | `mobile_protocol.py target/debug/kelpied.exe` | PASS | 182/182 | 104 s | 77/47 |
| 3 | `mobile_resilience.py target/debug/kelpied.exe` | PASS | 55/55 | 489 s | 87/39 |
| 4 | `pnpm tauri build --debug --no-bundle` | PASS | `target/debug/kelpie.exe`, 14:39:44; version stayed 0.1.7 | 305 s | 100/71 |
| 4 | `acp_chat.py`, fake agents only | PASS | 56/56 | 43 s | 63/56 |
| 4 | `scripts/e2e/mobile.py`, once | PASS | 107/107; endpoint OK, data dir OK, seam `approve` | 164 s | 58/100 |

### After the edit, on the current tree

| Step | Command | Result | Counts | Time | Load |
|---|---|---|---|---|---|
| 5 | `mobile_perf.py target/debug/kelpied.exe --seconds 6` (about 14:44:43; `mobile_perf.py` was not edited) | PASS, refuses | exit 1 with `--seconds must be more than 6: ...` plus the usage text. Python and kelpied PIDs the same before and after; `kelpie-phone-perf*` temp folders 46 before and after | 2.6 s | 82 |
| Extra | `cargo test -p kelpied --bin kelpied --no-fail-fast` | PASS | 523 pass, 0 fail, 3 ignored, 171.92 s in the tests | 395 s, recompiled | 66/100 |
| Extra | F1 test binary alone, 12 runs | PASS | 12/12 | under 1 min | 65-100 |
| Extra | `mobile::tests::running_out_of_the_budget_is_recorded` alone, 12 runs | PASS | 12/12 | under 1 min | 80-100 |
| Extra | `cargo clippy -p kelpied --all-targets` | PASS | 0 warnings, 0 errors | 32 s | 79/70 |

## The edit made during my run

- **What I saw:** at 14:17 I read lines 4772-4773 of `mobile.rs` as `tokens = 0.0` and `at = Instant::now()`. At 14:47 a grep showed `at = Instant::now() + Duration::from_secs(60)`. The file time is 14:44:33; it was 13:46:35 before. `mobile.py` had ended at 14:44:14.
- **Who made it:** not me. No process of mine writes source. I do not know who did. `git status --short` cannot show it, because the file was already modified.
- **What changed:** the size went from 290148 to 290324 bytes, so +176. That fits exactly a 124-byte comment plus ` + Duration::from_secs(60)` twice, at lines 3755 and 4774. I have no earlier copy, so this fits the numbers but is not a diff. No other tracked or untracked file changed after 13:59 apart from this report.
- **Effect on the daemon:** both lines are inside `mod tests`, which starts at line 2618, so they are not in `target/debug/kelpied.exe`. The e2e results therefore carry over to the current tree, as far as this one edit goes.

## Failures

### F1. `mobile::tests::the_handshake_budget_drops_silently_and_garbage_does_not_touch_it`: phone code, intermittent, on the tree before the edit

The test is in the uncommitted diff of `mobile.rs`. Every failure had the same output:

```
panicked at crates\kelpied\src\mobile.rs:4775:9:
assertion failed: p.reconnect().await.is_none()
```

| Run | Load | Result |
|---|---|---|
| Full `cargo test` | 100 at 14:14:50, 74 at the end (14:16:46) | FAIL |
| Retry alone | 100 at the start of compiling, 79 at the end | FAIL |
| 10 more alone, 14:21:38-14:21:51 | 29, 57, 50, 62, 36, 57, 57, 40, 54, 63 | ok, FAIL, ok, FAIL, FAIL, ok, ok, FAIL, FAIL, ok |

**Those 10 extra runs go beyond the one retry you asked for.** The retry failed too, and they cost only seconds. They used the retry's compiled binary, unchanged.

**Why it reads as the test, not the product:**
- The test set the handshake budget to 0 tokens and `at` to now. `handshake_allowed` (line 948) adds `elapsed × 20` tokens and refuses below 1.
- So the handshake could only be accepted if **at least 50 ms** had passed. Only tests write that budget from outside `handshake_allowed`. A refill after 50 ms is the intended behaviour.
- In those 50 ms the test opened a TCP connection, did the HTTP upgrade and built a Noise first message, in an unoptimised build. I did not measure each part.
- The edit's own comment gives the same cause, and the results agree: 7 failures in 12 before, 0 in 13 after (12 alone plus the full kelpied run), at a heavier load.

**What the earlier failures hid:** in the 7 failing runs the assertions after that line never ran. They ran in all the passing runs.

**What I cannot explain:** the test passed in the three earlier full runs whose logs I checked (the scratchpad root, `t0250`, `t1240`), one of them at load 100, then failed half the time alone at moderate load here. With the edit this no longer decides anything, but the reason is unknown.

### F2. `git_finish::tests::twenty_cached_summaries_are_fast`: not phone code, time-dependent

```
20 worktrees: cold 12.8044433s, cached 51.4852ms
panicked at app\src-tauri\src\git_finish\tests.rs:432:5:
cached refresh took 51.4852ms
```

It missed the 50 ms limit by 1.5 ms in the full run. Alone at load 91/85: **pass**. `git_finish` has no uncommitted change. It also failed in the report before `tester-261031-0250-phone-phase3.md`, which notes that and says it passed in its own run. The two failures from that report (`concurrent_hook_settings...`, `only_the_shown_agent_is_raised...`) passed this time.

## Suspected product bugs

**None.**

**One thing to look at in the edited test, not a failure.** `handshake_allowed` sets `at = now` on its first call, so the clock set ahead only protects that first call. This does not matter for F1's test, which makes one call before refilling the budget. `running_out_of_the_budget_is_recorded` loops `FLOOD_EVENTS` (20) calls, so those calls must still finish within 50 ms in total. It passed 12/12 alone at load 80-100 and in the full run, so I saw no failure. This comes from reading the code only.

## Isolation and rules

**Real daemon and app.** Recorded by path before anything started, after every e2e script and every other group of commands, 18 records in all. All identical, no change at any time:

| Process | PID | Started | Path under `%LOCALAPPDATA%\Kelpie` |
|---|---|---|---|
| kelpied | 268 | 13:38:51 | `daemon\b\kelpied.exe` |
| kelpie | 4368 | 12:42:03 | `kelpie.exe` |

- **Endpoints:** `mobile_protocol`, `mobile_resilience` and `mobile_perf` call `ps.isolate()` in their own process, and `mobile_perf` refuses before that call. `acp_chat` sets its own endpoint and data folder. For `mobile.py`, `run-with-load.py --gui` set them in its own process (endpoint `kelpie-m-3576`), and `mobile.py` printed endpoint OK and data dir OK. `link_noise.py` sets no Kelpie endpoint; its result is a vector check.
- **No real OS prompt:** `mobile.py` printed `verification seam: approve` before any approval. The protocol and resilience daemons get the seam from `phone_support.py`.
- **No real agents or model calls.** No Claude session was started or killed.
- **Secrets:** none printed. The environment was never listed. The 47 `.log` and `.txt` files in my folder were scanned for `#p=`, `"psk"` and `KELPIE_SESSION_TOKEN`: 0 hits.
- **Keyring:** `cmdkey /list`, entries matching `kelpie-`: 0 before, 0 after.
- **Memory and builds:** no build was stopped. Lowest free memory I sampled was 323 MB, during the F1 retry's compile.

**Leftovers.** No test daemon, `msedgedriver`, `tauri-driver`, headless Edge, e2e Python process, `cargo` or `rustc`. No listener on 4444, 4445, 9515 or 48170. All `msedgewebview2` processes belong to the real app: PID 4368 or its child 7332. `mobile.py`'s runner stopped its driver tree (PID 13160) and test daemon (PID 11016), and reported the daemon gone.

**`git status --short`.** 173 lines at the start. At the end the only difference is `?? plans/reports/tester-261031-1400-phone-phase3-final.md`, this report. HEAD unchanged, 0 stashes, 0 staged. Version 0.1.7 in `app/package.json` and `tauri.conf.json`.

## Departures from the brief

- **Extra runs:** the 10 repeats of F1, and the whole "After the edit" table apart from step 5. I added them because the tree changed under the run.
- **`MSEDGEDRIVER` set for `acp_chat`** to `D:\Works\edgedriver\msedgedriver.exe`, which is not on PATH. It is not a Kelpie setting.
- **`acp_chat` got the app path explicitly,** `target\debug\kelpie.exe`, the same binary its default would find.

## Not run, and why

- **The full step 2 command on the current tree:** kelpied ran in full. `kelpie`, `kelpie-link` and `kelpie-proto` did not rerun, because the edit is in kelpied's test module. So one clean `cargo test` across all four has **not** been seen, and F2 can still fail it under load.
- **The e2e scripts on the current tree:** see "Effect on the daemon". I did not rebuild `target/debug/kelpied.exe` (14:24:21), so it is older than the source's file time.
- **Full perf gate, `mobile_perf` measurement, release build:** settled separately, by your instruction.
- **`stress.py`, installer, `scripts/build-release.sh`:** by rule.
- **`kelpie-stt` tests, `scripts/check-phone-guard.sh`:** not in the brief. CI's `cargo test` line does include `-p kelpie-stt`.
- **A real OS prompt, a real phone, macOS:** by rule or not available.

## Other observations

- **`mobile.py` took 36 s from its last check to its result** (14:43:30 to 14:44:06) at load 100. The earlier run in `t1240` took 2 s there. The checks had all passed by then, and I did not find out what took the time.
- **`%LOCALAPPDATA%\Kelpie\auth`:** 407 token files at 14:45, 384 at the end of the `t0250` report. **`%TEMP%`:** 9,197 `kelpie-*` folders, 8,331 then. I did not count at my start, so I cannot say how many are mine. I removed none. These tests keep growing both.

## Disk at the end

C: 11.07 GB free (12.29 at the start, lowest seen 8.56 during a build). D: 7.37 GB free (7.56 at the start, lowest seen 7.24). Never near the 1.5 GB stop line. Logs: `C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad\t-final\`.

## Unresolved questions

1. **Who made the 14:44:33 edit, and is it meant to be part of this commit?** If it was not yours, it needs a look. The rerun results depend on it.
2. **Why did F1 fail so often only in this run?** It passed in all earlier runs I checked, and I have no earlier copy of `mobile.rs` to compare.
3. **Is `running_out_of_the_budget_is_recorded`'s remaining 50 ms worth tightening?** It has not failed.
4. **Should `twenty_cached_summaries_are_fast` keep a 50 ms limit under parallel tests?** It is not phone code, and it has now missed that limit in two reports I know of.
