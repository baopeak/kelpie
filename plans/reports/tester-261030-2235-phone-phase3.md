# Independent test run: phone link (phase 3)

Branch `phone/phase3-link`, uncommitted working tree, HEAD `d6e5883`. Run 2026-10-30 22:44 to 2026-10-31 00:52 on Windows, with the host at 32-100 % CPU almost throughout. Nothing was committed, staged, stashed or pushed. No installer was built. No product code or test was changed.

## Verdict

- **Functionally the link holds.** On the final scripts: `mobile.py` 107/107 on 3 runs in a row, `mobile_protocol` 180/180, `mobile_resilience` 55/55, `acp_chat` 56/56.
- **The `mobile_perf` gate failed on both runs and cannot be judged here** (F11): it can only run on a debug daemon, and the host was at 100 %. It neither shows a regression nor shows the 5 % budget is met.
- **One real product bug, low severity, not new in phase 3** (P1): the page throws away a screen the daemon may still build on, in `screen-sync.ts:116`. The page recovers by itself. Left unpatched, as instructed.
- **The full `cargo test` never ran clean in one go.** Two tests failed in the full run and passed alone, neither in phone code (F1, F2).
- `mobile.py` and `mobile_perf.py` had never run: `mobile_perf.py` needed no edit, `mobile.py` needed several (below). None of them weakens a check.

## Results

Load is `Win32_Processor.LoadPercentage` where a single number is given. `cpu=` values in the e2e logs are 3 s `psutil` samples from my runner, one per output line.

| # | Command | Result | Counts | Time | Load |
|---|---|---|---|---|---|
| 1 | `pnpm typecheck` | PASS | exit 0 | 47 s | 49 at start |
| 1 | `pnpm test` | PASS | 55 files, 670/670 | 36 s | 99 at end |
| 1 | `pnpm i18n:check` | PASS | 2276 used, 2439 translated, 0 missing, 0 problems, 0 untranslated-looking | 4 s | |
| 1 | `pnpm build:phone` | PASS | 70 modules; main chunk 191.92 kB (64.88 kB gzip) | 60 s | 80 at end |
| 2 | `cargo test -p kelpie-link -p kelpie-proto -p kelpied -p kelpie` | **FAIL** exit 101, see F1, F2 | kelpie 169 pass, **1 fail**, 5 ignored; kelpie-link 12 pass; kelpie-proto 23 pass, 2 ignored; kelpied 480 pass, **1 fail**, 3 ignored | 11 m 34 s | 88 start, 45 end |
| 2 | Each failed test alone | PASS | 1/1 and 1/1 | | 63, 62 |
| 2 | `cargo clippy -p kelpie-link -p kelpied -p kelpie --all-targets` | PASS | 0 warnings, 0 errors | 1 m 13 s | 68 start |
| 2 | Ring check (`cargo tree -p kelpie-link -e normal`, grep `\bring v`) | PASS | 0 hits in 66 lines | | |
| 3 | `python -X utf8 scripts/e2e/link_noise.py` | PASS | Rust vectors and published Noise vectors ok | 2 s | |
| 4 | `cargo build -p kelpied` | PASS | binary newer than every daemon source | 1 m 52 s | 32 start |
| 4 | `mobile_protocol.py`, run 1 | PASS | 180/180 | 100 s | 100 start |
| 4 | `mobile_protocol.py`, run 2, final scripts | PASS | 180/180 | 128 s | 88 start |
| 4 | `mobile_resilience.py`, 5 runs | **PASS only on run 5**, see below | 48/1, 48/1, 53/2, 54/1, **55/0** | 246-507 s | |
| 4 | `acp_chat.py` (fake agents only) | PASS | 56/56 | 62 s | 93 start |
| 5 | `pnpm tauri build --debug --no-bundle` | PASS | `target/debug/kelpie.exe`; version stayed 0.1.7 | 10 m 42 s | 94 start |
| 6 | `mobile.py`, 8 runs | **PASS on the last 3, on the first 5 not always** | see below | 97-199 s | |
| 7 | `perf_gate.py --metrics bundle --compare baseline.json` | PASS | entry 325.77 KB gzip, baseline 323.6, limit 338.60 | 58 s | 99 start |
| 7 | `mobile_perf.py target/debug/kelpied.exe`, run 1 | **GATE FAIL**, see F11 | phones add +29.3 points (limit 5); 0 frames for unfollowed sessions | 375 s | 100 |
| 7 | Same, retry | **GATE FAIL** | +42.5 points; 0 frames for unfollowed sessions | 310 s | 100 |

The three checks you named as load candidates in `mobile_protocol` ("frames still add up", "carries on from the last frame", "its screen is the terminal's again") passed on both runs, once at 97 %/65 % and once at 100 %/32 %.

### `mobile_resilience.py`, run by run

| Run | Result | Failed | Script state |
|---|---|---|---|
| 1 | 48/1, exit 1 | F3, which also skipped the rest of part 4 and parts 5-6 | as found |
| 2 | 48/1, exit 1 | F3, now with its close code | edit 1 |
| 3 | 53/2, exit 1 | F4, F5 | edits 1, 2 |
| 4 | 54/1, exit 1 | **F6, the product bug** | edits 1-4 |
| 5 | **55/0, exit 0** | none | final |

### `mobile.py`, run by run

| Run | Result | Failed | Script state |
|---|---|---|---|
| 1 | 99/6 | F7, and F8 with its 3 follow-on failures | as found |
| 2 | 106/1 | F9: my own first filter, caught by the check's positive control | edits 5, 6 |
| 3 | 106/1 | F10 | edit 5 corrected |
| 4 | **107/0** | none | + diagnostic text |
| 5 | 106/1 | F10 again, this time with the screen text | same |
| 6 | **107/0** | none | edit 7 |
| 7 | **107/0** | none | same |
| 8 | **107/0** | none | exact final, edit 8 |

All 8 reported `daemon check: endpoint OK | data dir OK` and `verification seam: approve` before any approval.

## Failures

**F1. `git_finish::tests::twenty_cached_summaries_are_fast`: time-dependent, not phone code.**
```
20 worktrees: cold 22.534375s, cached 61.2227ms
panicked at app\src-tauri\src\git_finish\tests.rs:432:5: cached refresh took 61.2227ms
```
Budget 50 ms. Load 88 at the start of the suite and 45 at its end; the test's own cold time shows how busy the host was. Retried alone at load 62 falling to 37: **pass**, cold 6.9 s, cached 45.98 ms. So the margin is thin even when it passes. `app/src-tauri/src/git_finish` is unmodified on this branch.

**F2. `agents::tests::concurrent_hook_settings_are_published_as_complete_json`: a race, not a timing budget, not phone code.**
```
panicked at crates\kelpied\src\agents.rs:654:50:
called `Result::unwrap()` on an `Err` value: Os { code: 5, kind: PermissionDenied, message: "Access is denied." }
```
The test's reader hit `std::fs::read` while two writers replaced the file. Retried alone at load 63: **pass** in 3.66 s. `agents.rs` is unmodified here; the code is from `3eb6b92` (2026-10-28). Load may make it likelier, but I have no evidence it is only load. See P2.

**F3. `mobile_resilience`, 100 kills: script predates a designed limit.** Runs 1 and 2, deterministic. Run 1 crashed with `RuntimeError: coroutine raised StopIteration`, because `Phone.hello()` ran `next()` on no hello, which lost the close code and skipped the rest of part 4 and parts 5-6. With edit 1, run 2 said: `no hello from the daemon: closed 4429, reason 'too_many'`. This is `RECONNECT_BURST`, 5 handshakes per 10 s per device, which the spec asks for (`phase-03-kelpie-link-e2e.md:50`). The page treats 4429 as any other close and retries with backoff (`link.ts:627-629`). The loop now waits on the 4429 (edit 2), and the 100 kills and the rows check are unchanged. They pass: `100 connections killed mid-stream: the rows are identical after every reconnect`.

**F4. `mobile_resilience`, "another device's late answer is stale": stale script value.** Run 3. Alice answered `"2"`, "Yes, and don't ask again", which is now correctly refused with `one_shot`, so Bob's `"1"` was the first answer. Alice now answers `"1"` (edit 4).

**F5. `mobile_resilience`, "a second retry is a duplicate and the agent got one turn": time-dependent, fixed wait.** Run 3 only, **100 % CPU**:
```
({'qid': 'q1', 'result': 'dup', 'state': 'sent', 'type': 'ack', ...}, [])
```
`dup` was right, but the fake agent had not read its input one second after the write. Passed at 90 % and 86 % in runs 1 and 2. The wait now watches for the arrival, then still holds 1 s to catch a second turn (edit 3). Passed at 100 % and 96 % in runs 4 and 5.

**F6. `mobile_resilience`, "300 ms latency with jitter ... loses no frame": product bug P1.** Run 4 only, `(None, 1)`: one lost frame. The check reported at 44 %, and CPU over its 58 s ranged 44-100 %. Retried as run 5: pass, reported at 97 %. Runs 1-3 passed, at 48 %, 66 % and 99 %. Timing of the network path decides this, not host load, see P1.

**F7. `mobile.py`, CSP check: script mistake.** Run 1. The violation it found was from Edge's own new tab page, before navigation: `https://ntp.msn.com/edge/ntp?... - Executing inline event handler violates...`. Edit 5.

**F8. `mobile.py`, prompt keys: stale expectation.** Run 1. `the prompt card offers the prompt's own keys [['1', '3']]`, expected `1, 2, 3`. The phone hides the lasting option by design (`architecture.md:410`). The tap on `"2"` then found no button, which caused three follow-on failures ("one tap answers the second prompt too", "the third prompt came up", "the card goes away once the prompts are answered"). Edit 6.

**F9. `mobile.py`, "…and that log would show one": my own mistake, caught by the check.** Run 2. My first filter for F7 kept only lines starting with the page's URL. A small probe showed the real line starts `security 2 Loading the script ...`. Corrected so that only a line naming *another* page is left out.

**F10. `mobile.py`, "…and the shell takes the next command": time-dependent, waited on the wrong thing.** Run 3 at 92-99 %, run 5 at 89-99 %. The same step flaked in the phase 2b report (F4 there). Passed in run 4 at 89 %. With the screen text added, run 5 showed:
```
Control-C
PS C:\...\mobile-proj>
```
The prompt returned and `echo after-ctrl-c` was never echoed. `ping` prints `Control-C` before the shell is back, and the console drops what is typed in between; the comment at the same place in the script already describes that drop for an earlier moment. The text goes through `session.input`, not through the phone. The script now waits for the prompt (edit 7). Passed at 93 %, 100 % and in run 8. **Inference, not proof:** I did not capture ConPTY's behaviour directly. The evidence is the screen text, and 3 of 3 passes after the change against 2 of 5 failures before it.

**F11. `mobile_perf` gate: fails, and is inconclusive.**
```
run 1: 8 phones add +29.3 points (runs: +29.3, -9.5, +48.9); lag p99 20/20/20 ms, levels 3/3/2; GATE FAIL
run 2: 8 phones add +42.5 points (runs: +51.3, +2.0, +42.5); lag p99 30/20/50 ms, levels 2/3/2; GATE FAIL
```
Load 100 on both, in every sample during the measurements. Why this does not settle the budget either way:
- **It must run on debug.** The script's header says "use a release build", but `phone_support.Daemon.start` refuses anything but debug, because only debug trusts a script as the app, which approving a phone needs. Unoptimized Rust is much slower, and that includes the new encryption (I did not measure by how much). There is no way to run this gate on release today.
- **The subtraction it relies on did not hold.** With no phones, intake ranged 0.02-0.71 MB/s, so "% of a core per MB/s" came out 77 and 102, and results for identical runs ranged -9.5 to +51.3.
- **The daemon was not pinned**, so it reached L2/L3 and shed work during the measurements.

I am not calling this "just load": the numbers could also hide a real cost of encrypting per phone. The part of the gate that is not about CPU holds: 0 frames for sessions nobody follows, on both runs.

## Product bugs suspected

**P1. The phone throws away a screen the daemon may still build on. Low severity, self-healing, predates phase 3 (`1db592d`).**

`app/phone/src/screen-sync.ts:116`:
```ts
const keep = m.base === null ? this.acked ?? 0 : m.base;
```
On a whole frame the page discards everything below its own last acknowledgement. But acknowledgements take time to arrive. The daemon may have heard only an earlier one and build on that.

Evidence, from an isolated probe with 8 phones behind the same 300 ms + 100 ms jitter proxy, 2 rounds of 58 s (2 lost frames, on 1 of 16 phone-runs, CPU 70-92 %):
```
frame 310 whole, ack 310       held [309, 310]
frame 311 whole                held [310, 311]
ack 311 (still on its way)
frame 312 whole                held [311, 312]   <- 310 gone
frame 313 base=310  LOST                         <- the daemon had only heard ack 310
frame 314 base=310  LOST
frame 315 base=311  applied
```
Replayed against the page's real `ScreenSync` class:
```
frame rev=312 base=null: applied=true  refollows=0 acks=[310,311] haveRev=311
frame rev=313 base=310:  applied=false refollows=1 acks=[310,311] haveRev=null
```
Effect: over a slow link at the start of following a session, the page drops what it holds and asks for the whole screen again, costing an extra whole frame and a round trip. The screen does not end up wrong. It will likely matter more once a relay is in use, since higher latency widens the window.

Likely fix, not applied: do not prune on a whole frame, or prune only below the highest `base` the daemon has used. `scripts/e2e/phone_support.py` `_apply` has the same rule on purpose, to mirror the page, so **both must change together**, and I left both as they are so the check keeps catching it. Expect `mobile_resilience` to fail this check now and then until it is fixed.

**P2. Reading the hook settings can fail while they are replaced (F2). Pre-existing, not phone.** Replacing a file on Windows can make a concurrent read fail with Access denied. If a real consumer reads this file during concurrent launches, the same could happen to it. I did not check what a real reader does on this error. The test itself treats any read error as fatal, so at minimum it is flaky under load.

**Not bugs:** the 4429 limit (F3), hiding lasting options (F8) and the `one_shot` refusal (F4) are all designed behaviour.

## Script edits

All in the allowed files. `mobile_perf.py` was not edited. No budget, assertion or check was relaxed.

| # | File | Change | Reason |
|---|---|---|---|
| 1 | `phone_support.py`, `Phone.open` | Raises `ConnectionError` naming the close code when no hello comes | Was a `StopIteration` inside a coroutine, which lost the code. Callers checked: `stress.py:228` guards `ps.pair` only |
| 2 | `mobile_resilience.py`, `connect` | On 4429, waits 1 s and tries again, up to 20 times | F3. Any other close still raises |
| 3 | `mobile_resilience.py`, `chat_send_in_blackhole` | Waits for the message to arrive, up to 20 s, then the same 1 s and the same check | F5. A message that never arrives still fails |
| 4 | `mobile_resilience.py`, part 6 | Alice answers `"1"`, not `"2"` | F4 |
| 5 | `mobile.py`, `csp_violations` | Leaves out only lines naming another page | F7, F9. The positive control still passes |
| 6 | `mobile.py`, prompt section | Expects `["1", "3"]`; taps `"3"`, expects `ANSWERED2=3` | F8. Stricter than before about what the phone offers |
| 7 | `mobile.py`, after Ctrl+C | Waits for the prompt before typing; the check shows the screen when it fails | F10 |
| 8 | `mobile.py`, two check details | No longer prints the pairing link or the six-digit code | The link holds the PSK, see Disclosures |

## Rules and isolation

- **Real daemon:** PID 12168 (`...\Kelpie\daemon\a\kelpied.exe`) still running with its original start time, 5:06:54 PM, checked repeatedly during the run and at the end.
- **Isolation:** I read each script's setup and cleanup first. GUI runs used a scratch runner that sets `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR` in its own process, removes `KELPIE_SESSION_*`, and stops only its own driver tree by PID. It asserts its endpoint before shutting down a daemon.
- **No real OS prompt:** negative control on an isolated debug daemon, calling `daemon.info` only. Without the variable: `test_user_presence = None`, `mobile.py` would refuse. With it: `'approve'`. All real `mobile.py` runs reported `approve` before any approval.
- **No real agents or model calls.** Only `fake_acp_agent.py`, the turn agent and small scripts. No Claude session was started or killed.
- **Keyring:** 0 entries matching `kelpie-` before and after.
- **Leftovers:** no test daemon, `msedgedriver`, `tauri-driver`, app or test WebView of mine remains; ports 4444, 4445, 9515 and 48170 have no listener. WebView windows were closed by their scripts or by stopping the driver tree.
- **`git status`:** one new entry since the start, `plans/reports/code-reviewer-261030-2235-phone-phase3-second-review.md`, which is not mine (a parallel reviewer) and which I did not read or touch. Nothing staged. HEAD unchanged. `scripts/perf/baseline.json` unchanged. The perf report JSON went to the scratchpad, not `plans/`.

## Disclosures

- **A rule I broke:** my first environment listing printed `KELPIE_SESSION_TOKEN` into my tool output. I noticed at once and did not copy it into any log or file, but it is in this session's transcript. It is the token of the session this ran in. Rotate it if that matters to you.
- **PSKs in logs:** `mobile.py` printed the pairing link in a check's detail, so 7 of my scratch logs each held a test PSK. They were one-time codes on throw-away daemons, already spent and gone, and were not shown in my output. I scrubbed the logs (14 replacements, links and codes) and fixed the script (edit 8). Run 8's log has none.

## Not run, and why

- **`mobile_perf` on a release daemon, and the GUI and `bench` perf metrics:** not possible or not asked, see F11. Disk also rules out a fresh release build, and `target/release/kelpied.exe` dates from 2026-10-29.
- **A clean full `cargo test` in one go:** not repeated, at 11+ minutes. The two failures passed alone.
- **`kelpie-stt` tests, `scripts/check-phone-guard.sh`, `KELPIE_REQUIRE_PHONE=1`:** in CI, not in this brief.
- **`stress.py`, `composer.py`:** not in this brief. `stress.py` also uses `phone_support` and was not exercised against edit 1.
- **The real OS verification prompt:** by rule. Only its unit tests ran.
- **A real phone:** the phone is headless Edge with mobile emulation, on 127.0.0.1. No Wi-Fi, Tailscale or iOS/Android browser.
- **macOS.**

## Other observations

- **`acp_chat.py` ran after step 5, not in step 4,** because it needs the debug app.
- **`msedgedriver` is 155 and Edge/WebView2 is 156.** It works today, but expect it to break at some point.
- **Test runs write outside their data folder.** `%LOCALAPPDATA%\Kelpie\auth` gains one token file per test endpoint and nothing removes them: 344 before, 363 now. This is the real Kelpie folder, though the daemon itself was not touched.
- **`%TEMP%` holds 7,932 `kelpie-*` folders,** up from 7,806. `mobile_protocol` and `mobile_perf` leave their data folders, and `cargo test` leaves `kelpie-cp-*` and `kelpie-phone-*`. The folders created during this run total about 1.5 MB, so this is clutter, not the disk problem. I deleted none, since other sessions may own some.
- **`mobile_resilience.py`'s header says about six minutes;** it now takes about eight, because of the 4429 waits.

## Disk at the end

C: 3.12 GB free (3.21 at the start). D: 4.46 GB free (5.24 at the start). Lowest seen on D: 4.36 GB. Never near the 1.5 GB stop line.

Logs: `C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad\`, for example `6-mobile-8.log`, `4-resilience-5.log`, `4-latent-probe-1.log`, `7-mobile-perf-2.log`. Probe scripts are there too.

## Unresolved questions

1. **Should P1 be fixed before phase 3 is committed?** It is low severity and not new, but `mobile_resilience` will fail now and then until it is, and relay latency will make it more frequent.
2. **How should the 5 % phone CPU budget be verified now that phones need a debug daemon to pair?** One option to examine: pair on a debug daemon, then measure a release daemon on the same endpoint and data folder. That needs a release build, and disk is short. Until then, the budget is unverified with encryption in place.
3. **Should `twenty_cached_summaries_are_fast` keep a 50 ms budget?** It passed at 46 ms alone on this machine.
4. **Is a failed read during the hook-settings replace harmful to a real reader (P2), or only to the test?**
