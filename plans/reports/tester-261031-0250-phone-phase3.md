# Independent test run: phone link (phase 3), third pass

Branch `phone/phase3-link`, uncommitted working tree, HEAD `d6e5883`. Run 2026-10-31 02:38 to 04:32 on Windows, 8 GB RAM. The host was at 90-100 % CPU for most of the run, partly from other people's programs (`ProtoLion`, `Antigravity IDE`, `claude`, `chrome`). Nothing was committed, staged, stashed or pushed. No installer was built. No product code or test was changed.

## Verdict

- **Functionally the link holds on this tree.** `mobile.py` 107/107 twice, `mobile_protocol` 182/182 twice, `mobile_resilience` 55/55 first time (no retry needed), `acp_chat` 56/56.
- **`mobile_perf`, first real measurement on a release daemon: PASS, PASS, UNSTABLE.** Medians +3.4, +3.7, +3.6 points against the limit of 5. This is good evidence, not a clean pass: the machine was at about 100 %, the daemon was at L2/L3 and shedding during every measurement, and the first round with phones cost more than 5 points every time (P1).
- **`cargo test` in one go still fails** with 2 of 704 run: neither is in phone code, both pass alone at load 100.
- **`stress.py`, first run on the new `phone_support`: 10/1, then a short retry 11/0.** The one failure and the retry's pass both come from the host, which never reached L0. Whether the level falls to L0 within 25 s remains **untested**, not passed (F3).
- **The earlier product bug (`screen-sync.ts`) is changed in the code,** and the check that caught it passed once. One pass does not prove it, because it was intermittent.

## Results

Load is `Win32_Processor.LoadPercentage` at start/end of each command. Times include compilation.

| # | Command | Result | Counts | Time | Load |
|---|---|---|---|---|---|
| 1 | `pnpm typecheck` | PASS | exit 0 | 44 s | 97/63 |
| 1 | `pnpm test` | PASS | 55 files, 674/674 | 34 s | 82/70 |
| 1 | `pnpm i18n:check` | PASS | 2277 used, 2440 translated, 0 missing, 0 problems, 0 untranslated-looking | 4 s | 91/65 |
| 1 | `pnpm build:phone` | PASS | main chunk 193.13 kB (65.10 kB gzip) | 59 s | 76/100 |
| 2 | `cargo test -p kelpie-link -p kelpie-proto -p kelpied -p kelpie --no-fail-fast` | **FAIL** exit 101, F1, F2 | kelpie 170 pass, 5 ignored; kelpie-link 12 pass; kelpie-proto 23 pass, 2 ignored; kelpied 497 pass, **2 fail**, 3 ignored | 19 m 15 s | 100/100 |
| 2 | Failed test 1 alone (hook settings) | PASS | 1/1, 7.77 s | 233 s, recompiled | 100/100 |
| 2 | Failed test 2 alone (proc_tree) | PASS | 1/1, 1.95 s | 7 s | 100/100 |
| 2 | `cargo clippy -p kelpie-link -p kelpied -p kelpie --all-targets` | PASS | 0 warnings, 0 errors | 110 s | 100/100 |
| 2 | Ring check (`cargo tree -p kelpie-link -e normal`, grep `\bring v`) | PASS | 0 hits in 66 lines | 2 s | 100/99 |
| 3 | `python -X utf8 scripts/e2e/link_noise.py` | PASS | Rust vectors and published Noise IK / IKpsk1 vectors ok | 2 s | 100/100 |
| 4 | `cargo build -p kelpied` | PASS | exit 0 | 145 s | 100/99 |
| 4 | `mobile_protocol.py target/debug/kelpied.exe`, run 1 | PASS | 182/182 | 111 s | 99/77 |
| 4 | Same, run 2 | PASS | 182/182 | 99 s | 85/100 |
| 4 | `mobile_resilience.py target/debug/kelpied.exe` | PASS | 55/55 | 492 s | 100/53 |
| 4 | `stress.py target/debug/kelpied.exe` (30 min default) | **FAIL** exit 1, F3 | 10 pass, 1 fail | 1880 s | 100/54 |
| 4 | Same, `--minutes 6`, retry alone | PASS, see F3 | 11/11 | 438 s | 94/87 |
| 5 | `pnpm tauri build --debug --no-bundle` | PASS | `target/debug/kelpie.exe`; version stayed 0.1.7 | 478 s | 99/100 |
| 4 | `acp_chat.py` (fake agents only) | PASS | 56/56 | 66 s | 100/100 |
| 6 | `mobile.py`, run 1 | PASS | 107/107; endpoint OK, data dir OK, seam `approve` | 187 s | 100/57 |
| 6 | `mobile.py`, run 2 | PASS | 107/107; endpoint OK, data dir OK, seam `approve` | 97 s | 71/47 |
| 7 | `perf_gate.py --metrics bundle --compare baseline.json` | PASS | entry 325.76 KB gzip, baseline 323.6, limit 338.60 | 29 s | 71/60 |
| 8 | `mobile_perf.py` release + `--pair-with` debug, run 1 | **UNSTABLE** exit 2 | +3.4 points; 0 quiet frames | 173 s | 90/54 |
| 8 | Same, run 2 | **PASS** | +3.7 points; 0 quiet frames | 187 s | 74/100 |
| 8 | Same, run 3 | **PASS** | +3.6 points; 0 quiet frames | 205 s | 100/96 |

`target/release/kelpied.exe` (02:27:18) is newer than every file in `crates/kelpied/src` and `crates/kelpie-link/src` (newest 01:27:17), so it was not rebuilt. That is a comparison of file times only. `git status` shows the same source tree, but I did not verify the binary by any other means.

### `mobile_perf` in detail

| Run | Points added, per round | Median | Without phones, % of a core per MB/s | Daemon's level, per round | Frames/s/phone, per round | Verdict |
|---|---|---|---|---|---|---|
| 1 | **+9.1**, +1.7, +3.4 | +3.4 | 16.6 to 23.2 | 2, 2, 2 | 5.85, 10.03, 5.85 | UNSTABLE (baseline spread over 25 %) |
| 2 | **+12.1**, +3.7, +2.5 | +3.7 | median 19.8 | 2, 2, 2 | 5.85, 5.86, 7.29 | PASS |
| 3 | **+6.1**, +3.6, +0.1 | +3.6 | median 20.6 | 2, 3, 3 | 5.85, 5.85, 1.00 | PASS |

Event-loop lag p99 20-30 ms, p99.9 20-30 ms, all at or under the 30 ms decision line for the runtime split. Flat out (not gated): with phones -4.8 %, -56.0 %, -34.8 % against no phones, which is noise (without-phones time ranged 2.98-7.86 s). 0 frames for unfollowed sessions on all three.

**How far this can be trusted:**
- The runner's CPU sample at the end of each steady measurement was 96-100 % in runs 1 and 2 and 82-100 % in run 3. Some of that is the printers, so the load is not all other people's.
- The daemon was at L2 or L3 in all nine measurements, and `mobile_resilience.py`'s header says those levels shed frame rate and size. **The cost at L0 and full frame rate was not measured.** A quiet machine could show a higher number.
- Run 3 is the weakest: in round 3 the phones got 1 frame/s, and intake with phones (0.63-0.89 MB/s) was well below intake without (1.10-1.49 MB/s).
- The verdict's stability filter says run 1 is unstable. I did not rerun beyond the three allowed. No limit or baseline was changed.

## Failures

**F1. `agents::tests::concurrent_hook_settings_are_published_as_complete_json`: a race, not phone code.**
```
panicked at crates\kelpied\src\agents.rs:654:50:
called `Result::unwrap()` on an `Err` value: Os { code: 5, kind: PermissionDenied, message: "Access is denied." }
```
Full run at load 100. Alone at load 100: **pass**, 7.77 s. `agents.rs` has no uncommitted change. Same failure as in the previous report, so it has now failed in both full runs I know of. Parallel tests appear to provoke it; load alone does not, since it passed alone at 100. See P2.

**F2. `proc_tree::tests::only_the_shown_agent_is_raised_and_nothing_is_lowered`: time-dependent, not phone code. New in this run.**
```
panicked at crates\kelpied\src\proc_tree.rs:333:9:
ping runs under cmd
```
Full run at load 100. Alone at load 100: **pass**, 1.95 s. `proc_tree.rs` has no uncommitted change. The test sleeps a fixed 1800 ms and then expects a `ping.exe` child (line 331). Its neighbour `a_launcher_child_counts_as_the_agent` polls instead, with the comment "On a busy machine cmd may take a while". **Inference, not proof:** the fixed wait lost the race with 500 other tests running. I did not capture the process list at that moment.

The previous report's other failure, `twenty_cached_summaries_are_fast`, passed in this run (170 kelpie tests pass).

**F3. `stress.py`, "…and is L0 within 25 s of it stopping": the host, as far as the evidence goes.**
- 30-minute run: `FAIL`, with no detail printed, because the script passed `None`. No `(still ... after 60 s only because host memory ...)` line either, so at the last reading within the 60 s the level was above 0 and the reason was not memory alone. A later reading, after phone access went off, was level 3, `reason: ['memory']`, memory free 7.2 %, CPU 53 %.
- Retry alone with edit 1, 6 minutes: `PASS`, **through the check's own memory exception, not by reaching L0**:
```
(still 3 after 60 s only because host memory is 6 % free)
PASS ...and is L0 within 25 s of it stopping  ({'cpu': 75.78, 'level': 3, 'memory_free': 5.78, 'reason': ['memory'], ...})
```
  The very next reading, 2 s later, was `reason: ['cpu', 'memory']` at CPU 88 %. **So pass or fail here depends on which reason was showing in the last poll.** I think that explains the first run, but its last reading was not printed, so this is inference.
- Host memory free during the runs: 4-14 % at the once-a-minute readings. CPU at my sampler: 100 % through the stress phase.

Other checks in the stress runs are weaker here than they read:
- **"reaches L2 within 5 s" passed without testing anything:** the level was already L2 and L3 before the stress began, at 0.14 s and 0.01 s.
- **"RSS growth <= 10 %" passed on a shrinking number:** 255 to 155 MiB, 339 to 201 MiB. RSS fell during the same minutes host memory was at 4-7 %, so I read this as Windows trimming the working set. A leak could hide in this.

Checks with real weight: 0 false disconnects in both; echo p95 121 and 123 ms, p99 130 and 142 ms (limits 300, 800), on debug, at L2/L3, though the test process itself runs above normal priority by design; 9319 of 9319 and 1859 of 1859 inputs answered and echoed; 11 revokes in each run, longest 0.12 s and 0.28 s (limit 1 s), 0 writes after.

Stress on a shared machine: this run took host memory down to 4 % free for stretches of half an hour. Anyone else's programs on this host will have felt it.

## Product bugs suspected

**None new in phone code.**

**P1. Not a bug yet, a question: the first round with phones costs more than the limit.** In all 3 `mobile_perf` runs the first steady round with phones was the highest: +9.1, +12.1, +6.1, against 0.1-3.7 for the other rounds. The median, which the gate uses, leaves that out. It could be real start-up cost (first frames for a new follower), or the order of the rounds, or chance: one and the same round being highest in all 3 runs happens by chance about 1 time in 9. Nothing here separates those.

**P2. Reading the hook settings can fail while they are replaced (F1). Pre-existing, not phone.** As in the previous report. I did not check what a real reader does on this error.

**Previous report's P1 (`screen-sync.ts`):** the line that discarded screens on a whole frame is gone. `apply` now lets go only when `m.base !== null` (line 120), with a comment saying why. "300 ms latency with jitter ... loses no frame" passed once here, at 100 %. It failed 1 run in 5 before, so one pass is consistent with a fix but does not prove one.

## Script edits

| # | File | Change | Reason |
|---|---|---|---|
| 1 | `scripts/e2e/stress.py`, "…and is L0" check | Detail is `fell if fell is not None else left_by` instead of `fell` | A failure printed nothing about what held the level (F3). Diagnostic only: the pass condition is unchanged |

This is the file's only uncommitted change: `stress.py` was clean at the start. No other script, budget, assertion or check was changed. `scripts/perf/baseline.json` is unchanged.

## Departures from the brief

- **`cargo test` got `--no-fail-fast`,** so that a failure in one crate would not hide the others. Results are not otherwise affected.
- **The stress retry was 6 minutes, not 30.** The failing check concerns the minutes after the stress stops, so the shorter run tests the same thing. It is not a repeat of the 30-minute gate.
- **`acp_chat.py` ran after step 5,** as you asked.
- **I overwrote `status-before.txt` in the shared scratchpad,** a file left there by the previous tester, with my own snapshot. No project file was involved.

## Rules and isolation

- **Real daemon:** PID 12168 (`%LOCALAPPDATA%\Kelpie\daemon\a\kelpied.exe`) still running with its original start time, 30/10/2026 5:06:54 PM.
- **Isolation:** I read each script's setup and cleanup first. `mobile_protocol`, `mobile_resilience`, `stress` and `mobile_perf` call `ps.isolate()`. `acp_chat` sets its own endpoint and data folder. For `mobile.py`, the previous tester's runner (`run-with-load.py`, read before use, not changed) set them in its own process and stopped only its own driver tree by PID. It removes `KELPIE_SESSION_*`. I never listed the environment.
- **No real OS prompt:** both `mobile.py` runs printed `verification seam: approve` before any approval. For `mobile_perf` on release I read the dispatcher first: it calls only `mobile.enable`, `mobile.load`, `session.*` and `daemon.shutdown`, none of which reach `after_verifying`. Pairing happened on debug with the seam.
- **No real agents or model calls.** Only fake agents and small Python programs. No Claude session was started or killed.
- **Secrets:** none printed. All my logs scanned for `#p=`, `"psk"` and `KELPIE_SESSION_TOKEN`: 0 files.
- **Keyring:** 0 entries matching `kelpie-` before, 0 after.
- **Leftovers of mine:** no test daemon, driver, app, test WebView or e2e script left. Ports 4444, 4445, 9515, 48170: no listener.
- **Not mine, left alone:** `D:\kelpie-chat-e2e\bin\kelpied.exe` (PID 16304), `kelpie.exe` (14580), `msedgedriver` (2768) and `tauri-driver` (11920), all started 04:30:53-56, after my last process ended at 04:30:17. An earlier daemon from the same folder, PID 9016, started 03:49:47, during my 30-minute stress run. So another e2e session shares this machine and may have added to the load at the end of that run. It did not touch my endpoints.
- **`git status`:** compared with my snapshot at the start, two new entries: ` M scripts/e2e/stress.py` (edit 1) and `?? plans/reports/code-reviewer-261031-0250-phone-phase3-third-review.md`, which is not mine, and which I did not read or touch. After this report, this file is a third. Nothing staged, 0 stashes, HEAD unchanged. The perf JSON went to the scratchpad, not `plans/`.

## Not run, and why

- **A quiet-machine `mobile_perf`, and any measurement at L0:** not available on this host.
- **`cargo test --release -p kelpied --bin kelpied -- --ignored ingest_speed`,** which `mobile_perf.py`'s header names as the gate for intake speed: not in the brief, and the disk is short.
- **Perf gate GUI and `bench` metrics:** not asked. They need a release app.
- **A clean full `cargo test`:** not repeated, at 19 minutes.
- **Stress at 30 minutes a second time:** see Departures.
- **`kelpie-stt` tests, `scripts/check-phone-guard.sh`, `KELPIE_REQUIRE_PHONE=1`:** in CI, not in this brief.
- **The real OS verification prompt:** by rule. **A real phone:** headless Edge on 127.0.0.1. **macOS.**

## Other observations

- **`%LOCALAPPDATA%\Kelpie\auth` keeps growing:** 369 token files at my start, 384 now. Some are from the other e2e session. This is the real Kelpie folder, though the daemon was not touched.
- **`%TEMP%` holds 8,331 `kelpie-*` folders,** up from 7,932 in the previous report. I deleted only my own scratch folders in the scratchpad.

## Disk at the end

C: 2.32 GB free (2.39 at the start). D: 3.52 GB free (4.32 at the start). Never near the 1.5 GB stop line. Logs: `C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad\t0250\`.

## Unresolved questions

1. **Is +3.5 points enough to call the 5 % budget met?** Three medians agree, but every measurement was at L2/L3. Only a quiet machine can answer it for L0.
2. **Is the first round's +6 to +12 points real (P1)?** If phones usually connect one at a time, a start-up cost may matter more than the median shows.
3. **Should "…and is L0" accept `['cpu', 'memory']` on a host short of memory?** Its exception accepts `['memory']` only, so the verdict flips on the reason at one poll. I did not change it. Neither run tested the fall to L0.
4. **Should the two checks that passed without testing anything here be guarded,** for example by reporting "not tested" when the level is already L2 before the stress starts?
5. **Should F1 and F2 be fixed so that a full `cargo test` passes under load?** F2 could poll as its neighbour does. F1 has now failed in both full runs and also raises the question of what a real reader does on this error.
