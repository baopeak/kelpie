# Phone release: independent test

Branch `phone/link-on-master`, HEAD `f49c699`, worktree `D:\kelpie-phone-rebase`, build directory `D:\kelpie-phone-rebase-target`. Run 2026-10-31, 18:36 to 19:05, on Windows with 8 GB RAM.

**Result: every command passed on its first run, with no failures and no retries.** One item needs the owner's reading: `ring` is absent from `kelpie-link` and `kelpie-proto` but present in `kelpied` and `kelpie` (section 3). The real daemon did not change. Hygiene findings and one older anomaly in the real data folder are listed below.

Runner `run-with-load.py`: read first; `REPO` and `DEBUG` point at this worktree and build directory.

## 1. Commands

Load: for e2e scripts, the runner's per-line CPU samples (average / maximum). For other commands, one sample before and one after. Free RAM is measured after the command.

| # | Command | Exact counts | Result | Time | CPU | Free RAM |
|---|---|---|---|---|---|---|
| 1 | `git status --short`, HEAD | 0 lines, `f49c699` | PASS | | | 2333 MB |
| 2a | `pnpm typecheck` | no errors | PASS | 33 s | 100 / 44 | 2421 MB |
| 2b | `pnpm test` | 68 files, 1328 / 1328 tests | PASS | 26 s | 79 / 96 | 2518 MB |
| 2c | `pnpm i18n:check` | 2463 keys used, 2677 translated, 0 missing, 0 problems, 0 untranslated-looking | PASS | 3 s | 23 / 17 | 2523 MB |
| 2d | `pnpm build:phone` | built | PASS | 27 s | 42 / 21 | 2517 MB |
| 2e | `pnpm build` | built | PASS | 31 s | 31 / 28 | 2561 MB |
| 3a | `cargo test -p kelpie-link -p kelpie-proto -p kelpied -p kelpie --no-fail-fast` | 875 passed, 0 failed, 11 ignored (below) | PASS | 357 s | 6 / 27 | 2236 MB |
| 3b | `cargo clippy -p kelpie-link -p kelpie-proto -p kelpied --all-targets` | 0 warnings, 0 errors | PASS | 54 s | 85 / 31 | 2176 MB |
| 3c | CI ring check: `cargo tree -p kelpie-link -e normal`, grep `\bring v` | 0 matches in 66 lines | PASS | 3 s | 15 / 21 | 2172 MB |
| 3d | `cargo tree -i ring --workspace` | `ring v0.17.14` **present** (section 3) | see section 3 | 3 s | 7 / 14 | 2183 MB |
| 4a | `link_noise.py` | Rust vectors and published Noise IK / IKpsk1 vectors ok | PASS | 1 s | 0 / 0 | 2312 MB |
| 4b | `cargo build -p kelpied` | built | PASS | 47 s | 94 / 59 | 2075 MB |
| 4c | `daemon.info` on isolated endpoint | `api` = 22, `protocol` = 1, seam `approve`, endpoint confirmed | PASS | 8 s | 36 / 79 | 1962 MB |
| 4d | `mobile_protocol.py` | 188 / 188 | PASS | 86 s | 49 / 76 | 2137 MB |
| 4e | `mobile_resilience.py` (full, no `--quick`) | 55 / 55 | PASS | 487 s | 39 / 72 | 1616 MB |
| 5a | `pnpm tauri build --debug --no-bundle` | built `debug\kelpie.exe` | PASS | 113 s | 51 / 77 | 1717 MB |
| 5b | `acp_chat.py` | 56 / 56 | PASS | 33 s | 44 / 99 | 1941 MB |
| 5c | `mobile.py`, once | 107 / 107 | PASS | 80 s | 45 / 75 | 1751 MB |
| 6 | `updater.py <debug kelpied>` | 24 / 24 | PASS | 40 s | 29 / 71 | 1936 MB |

`cargo test` by crate:

| Crate | Passed | Failed | Ignored |
|---|---|---|---|
| `kelpie` (lib) | 257 | 0 | 6 |
| `kelpie` (main) | 0 | 0 | 0 |
| `kelpie-link` | 12 | 0 | 0 |
| `kelpie-proto` | 29 | 0 | 2 |
| `kelpied` | 577 | 0 | 3 |
| Doc-tests (3 crates) | 0 | 0 | 0 |

Ignored tests:
- `kelpie`: `agent_history::tests::scan_real_transcripts`, `integrations::tests::live_gitlab_public_project`, `search::tests::fuzzy_p95_on_fifty_thousand_files`, `session_export::tests::bless`, `usage::tests::live_usage`, `usage_report::tests::report_real_transcripts`
- `kelpie-proto`: `redact::tests::redact_speed`, `tests::legacy_secret_moves_to_the_current_service`
- `kelpied`: `checkpoints::tests::snapshot_cost`, `checkpoints::tests::snapshots_on_a_loaded_machine`, `mobile::sync::tests::ingest_speed_with_eight_phones`

Crates for the ignored tests are inferred from their order in the log and the per-binary ignored counts (6, 2, 3).

## 2. Failures and retries

None. No time-dependent failure, so no retry was needed.

## 3. Suspected bugs and items for the owner

No product bug was found.

**`ring` is still in the shipped binaries.** The brief asks `cargo tree` to confirm no `ring`. That holds only per crate:

| Crate | `ring` lines (`-e normal`) |
|---|---|
| `kelpie-link` | 0 |
| `kelpie-proto` | 0 |
| `kelpied` | 2 |
| `kelpie` | 2 |

`ring v0.17.14` arrives through `rustls v0.23.46` and `rustls-webpki v0.103.15`. `rustls` is reached from `kelpied` directly and through `tokio-rustls` and `ureq`, and from `kelpie` through `tauri-plugin-updater`/`reqwest` and `ureq`. The CI step is named "The phone link's crypto stays free of ring" and checks only `kelpie-link`, so it passes. That matches its name. It does not mean the daemon is free of `ring`.

**Test hygiene (scripts, not product):**
- `phone_support.py` never removes the endpoint's token from the real `%LOCALAPPDATA%\Kelpie\auth`. It deletes only the keyring entries. This run added 3 files: `kelpie-phone-protocol-12580.token`, `kelpie-phone-resilience-9816.token`, `kelpie-mobile-release-9096.token`. The folder already held 417 files, 185 of them `kelpie-phone-*`. `updater.py` and `acp_chat.py` do remove theirs; no token from those runs remained.
- `mobile_protocol.py` has no `rmtree`: its folders `kelpie-phone-protocol-cwd-nip685oz` and `kelpie-phone-protocol-n660xxgv` stay in `%TEMP%`.
- `updater.py` leaves its two session working folders (`kelpie-updater-cwd-1_wrnu6t`, `kelpie-upd-cwd-7por2ifk`).
- The Rust tests appear to leave most of the rest. `%TEMP%` gained 135 `kelpie-*` folders, 1.4 MB in total: 74 `kelpie-cp-*`, 55 `kelpie-phone-*` (2 are the protocol folders above), 2 `kelpie-auto-*`, 1 `kelpie-finish-*`, 1 `kelpie-notes-*`, and the 2 `updater.py` folders. This is by name and creation time; I did not trace each folder to a test.

I left all of these in place.

**Older anomaly in the real data folder, not from this run.** Real `sessions.json` holds 12 sessions, one of kind `acp-lab-fake` (alive = true). Real `plugins\acp-lab\kelpie-plugin.json` exists, created and written 2026-10-30 00:14:59. `acp-lab` is the plugin `acp_chat.py` installs in its isolated data folder.
- Not this run: the plugin file dates from two days ago, and `sessions.json` was last written at 18:48:19, before `acp_chat.py` started at 19:01.
- I found no marker of this run's endpoints or sessions in `sessions.json` or `app-state.json`, and both parse as valid JSON.
- I could not tell whether a past test leaked into the real folder or the owner installed it by hand. I read these files without changing them, and did not call the daemon.

## 4. Real daemon and app

Found by path under `%LOCALAPPDATA%\Kelpie\`. Snapshots were taken after the frontend checks, after the cargo checks, and after each e2e script. From step 4 on, the daemon's PID 268 was also polled every 5 to 10 s while commands ran. Steps 2 and 3 had snapshots only.

| | PID | Started | Before (18:36:23) | After (19:05:49) |
|---|---|---|---|---|
| `kelpied` (`daemon\b\kelpied.exe`) | 268 | 13:38:51 | running | running, same PID and start time |
| `kelpie` (`kelpie.exe`) | 4368 | 12:42:03 | running | running, same PID and start time |

No change at any snapshot. Real data files written during the run, presumably by the running real app and daemon: `sessions.json` 18:48:19, `usage-cache.json` 18:51:14, `app-state.json` 19:04:11. Isolation evidence: `api-check` reported a temp `data_dir`, and `mobile.py` printed `endpoint OK | data dir OK` three times.

`kelpie-` keyring entries: 2 before, 2 after, 2 at every snapshot. No real OS prompt: the seam was `approve` on the daemons that report it (`api-check`, `mobile.py`), and `mobile_protocol.py` / `mobile_resilience.py` get it from `phone_support`.

## 5. How the scripts were run

- All e2e scripts went through the runner. `mobile.py` used `--gui mobile-release` (pinned `L0:calm`).
- `acp_chat.py` and `updater.py` ran without `--gui`, because each starts its own daemon and endpoint; `--gui` would have added an idle driver. I set `KELPIE_TEST_USER_PRESENCE=approve` in their process. `acp_chat.py` also got `MSEDGEDRIVER`, since `msedgedriver` is not on PATH. Neither was pinned calm, as they don't ask for it.
- The shell carried `KELPIE_SESSION_*` from the real session. The step wrapper and runner removed them for every command. I checked variable names only and printed no values.
- Clippy "three crates" read as `kelpie-link`, `kelpie-proto`, `kelpied`.

## 6. Not run

- Excluded by the brief: release build, `scripts/build-release.sh`, installer, `stress.py`, `mobile_perf.py`.
- In CI but not in the brief:
  - `cargo test -p kelpie-stt`
  - `KELPIE_REQUIRE_PHONE=1` on `cargo test`
  - `scripts/check-phone-guard.sh`
  - the performance gate
- Clippy on the `kelpie` app crate.
- The 11 ignored tests.
- A real updater run: `updater.py` covers `kelpied stop --wait`, `latest_json.py` and `stop-daemon.ps1` with temp folders. No signed update or real installer was exercised.
- Screenshots from `mobile.py` (12) and `acp_chat.py` (9) were produced but not looked at.

## 7. End state

| Check | Result |
|---|---|
| Worktree `status --short` | 0 lines, HEAD `f49c699` |
| Test daemons, `msedgedriver`, `tauri-driver` | none |
| WebView windows | none added; only the real Kelpie window remains |
| Browser, script and console process list vs baseline | identical |
| Listeners on 4444, 4445, 9515, 48170 | none (`TIME_WAIT` only) |
| Disk at end | C 9.84 GB, D 6.22 GB (started 9.91 / 6.22) |
| Free RAM at end | 1800 MB |

No run was stopped for memory, and no disk came near 1.5 GB. Logs and screenshots are in `C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad\release-test`.

## 8. Unresolved questions

1. Should "no `ring`" cover the shipped binaries, or only the phone link's crypto? If the binaries, this fails: `kelpied` and `kelpie` both carry it through `rustls`.
2. Was the real `acp-lab` plugin and the live `acp-lab-fake` session put there on purpose on 2026-10-30, or did a test leak into the real data folder?
3. Should `phone_support.py` remove its token files? The real `auth` folder now holds 420, most of them from test endpoints.
