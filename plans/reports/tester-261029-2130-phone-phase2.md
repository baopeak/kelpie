# Tester report — phone phase 2 (client app)

Saved by the controller from the tester's final message (agent did not write the file).

## Results
- Frontend: typecheck pass; vitest 540/540; i18n 2174 keys, 0 missing; `pnpm build` pass, desktop entry gz 322.50 KB vs 323.6 baseline (−0.34 %); total desktop ±2 % vs master not verified (no same-day master build). `pnpm build:phone` 48.1 KB gz, no Tauri refs.
- Rust: kelpie-proto 23/23, kelpied 371/371; 2 automation tests flaky under host load (pass isolated + rerun), unrelated. Clippy 0 warnings. `check-phone-guard.sh` ok both ways.
- e2e (isolated daemon):
  - `mobile.py`: 41/41 pinned `L0:calm`; unpinned 39/41, 38/41, 39/41, 41/41. Failing: "approved phone connects and lists sessions" (page shows `busy`, not `live`) and "repeated bad codes are rate limited" (pairing pacing 503 at L2 comes before the failure counter, so no 429). Load-sensitive assumption, not regression.
  - `mobile_protocol.py` 78/78; `mobile_resilience.py` 48/48 ×2 (dev's jitter failure not reproduced); `attention.py` 15/15.
  - `acp_chat.py`: FAIL 8/12 in two runs — test's `agent.presets` lacks `acp-lab-fake` although plugin menu lists "Fake ACP". One run wrote `plugins/acp-lab` into the mobile test data dir. Cause unresolved; dev saw 56/56.
- Browser spot checks (headless Edge + fault proxy) 22/23: pairing, Needs-you group, `#s=` deep link, prompt buttons, XSS rendered as text, reconnecting at 0.11 s, inputs disabled while reconnecting, reconnect without reload, hidden page 0 requests / 0 frames over 8 s, light/dark, VI, read-only device has no composer.
  - FAIL: "already answered" never shown after desktop-first answer — the screen frame removes the card before the stale reply arrives (3/3).
  - Weak: "cached cards stay visible" checked a view with 0 rows.
  - Not checked: service worker, first load ≤ 1 s, real phones.

## Spec deviations
- `conn-state.ts:98-100` `canAct` allows input/keys/answers in `degraded`, `busy`, `restarted`; spec says disabled while not `live`.
- Composer textarea stays editable offline, send disabled (drafts kept).
- `focus`/`visibility` replaced by sub/unsub.

## Other observations
- `daemon.shutdown` returned `true` but a test kelpied (PID 7740) kept running and answering for > 10 min. Not investigated.

## Unresolved questions
1. Input/answers in `degraded`/`busy`/`restarted`?
2. Where to show a late "already answered" once the card unmounts?
3. `acp_chat` presets missing on test daemon; stray `acp-lab` in the mobile data dir.
4. `daemon.shutdown` not exiting.
5. Pin `mobile.py` to `L0:calm`?
6. Desktop total dist ±2 % vs master unverified.

## Retest after fix round 1
Branch `phone/phase2-client`, uncommitted, 2026-10-30. Fix round 1 per `plans/reports/fullstack-developer-261029-2000-phone-phase2-client.md`. Isolated endpoints and data dirs, ports 47310-47330, Vite on 1427 (free before start, stopped after), tauri-driver on 47320, `L0:calm` pin for the app's daemon.

| Check | Result |
|---|---|
| `pnpm typecheck` | pass |
| `pnpm test` | 564/564 (48 files) |
| `pnpm i18n:check` | 2178 keys, 0 missing |
| `pnpm build:phone` | pass; index 33.4 KB gz, about 49 KB total (budget 250) |
| `cargo test -p kelpie-proto -p kelpied --no-fail-fast` | kelpie-proto 23/23; kelpied 372/372 (3 ignored). No failures this time (the earlier automation and agents flakes did not recur) |
| `mobile.py` | 48/48 on run 1, 48/48 on run 2 (back to back). Run 3 failed at the approve dialog right after run 2. Cause: the script's own ten-plus bad pairing codes trip the 60 s per-IP limiter, and run 3 started within that window. Rerun after more than 60 s: 48/48. Harness caveat: leave 60 s between `mobile.py` runs |
| Stale notice (headless Edge, in `mobile.py`) | pass: "Already answered on the desktop just now" appears after the card is gone, stays at least 5 s, dismisses. Screenshot `scratchpad\m1\07b-phone-stale.png` shows it |
| Input policy (`mobile.py` plus unit) | pass: typing and keys disabled while disconnected; chat opened offline says it is not available offline. `conn-state.ts:103-106` matches the decision: live, degraded, busy allowed; restarted only when `resyncing` is false. Unit tests pin every state (`conn-state.test.ts`, `link.test.ts`) |
| C1 chat retry bytes (unit) | pass: `chat-observer.test.ts` "asking again after an answer whose fate is unknown or refused": Claude, Codex and ACP, unknown and refused. Second and third sends byte-identical to the first; adapter `respond` called once. Claude's line keeps `updatedInput.command`. Not done end to end: no real agent produces an unknown result on demand |
| `mobile_protocol.py` | 78/78 |
| `mobile_resilience.py` | 48/48, 48/48 |
| `acp_chat.py` run A (normal) | 56/56 (was 8/12 before the fix) |
| `acp_chat.py` run B (own msedgedriver held 4445 throughout) | 56/56 |
| `daemon.shutdown`, isolated, no app | exits in every case: bare x3 0.05 s each; `kill_sessions` with a session open 1.75 s (matches the code's 1.7 s sleep); phone access on 0.05 s |

Notes on the shutdown check:
- Not reproduced without an app. The earlier PID 7740 observation had an app attached. A respawned daemon would get a new PID, but 7740 kept its PID and kept answering on its endpoint. So respawn explains it only if the old process was never the one the app had connected to. Still open: rerun with a desktop app attached and the daemon's stop path logged.

Gaps in this retest:
- The headless-Edge spot-check script I wrote earlier is gone: the scratchpad was emptied mid-session (cause unknown; runner recreated). The stale scenario was taken from `mobile.py`, which now covers it.
- The C1 retry is covered by unit tests only. The `restarted`/resync gate is covered by unit tests only.
- Not run: first load on LAN, service worker, real iPhone or Android.

Cleanup: my Vite, tauri-driver (47320) and its msedgedriver, msedgedriver on 4445, the mobile test daemon, and the daemon from the shutdown check are all stopped. My temp dirs and tokens removed. The user's app (PID 8836) and daemon (PID 10588) were not touched. Ports 1427, 4445, 47310-47340 verified clear.

Unresolved questions:
1. `daemon.shutdown` with a desktop app attached: does the old process exit when the app is connected? Rerun needed.
2. `mobile.py` needs 60 s between runs (own bad-code checks). Should the script wait or clear its state first?
3. Chat retry end to end: is a deterministic "unknown" hook worth adding for `acp_chat.py`?

Status: DONE_WITH_CONCERNS
Summary: Fix round 1 holds: frontend, Rust, `mobile_protocol`, `mobile_resilience` (x2), stale notice in headless Edge, and `acp_chat` 56/56 both normally and with 4445 held. `mobile.py` 48/48 twice (third run failed on the limiter window, passed after a wait). The C1 identical-bytes retry is covered by unit tests. `daemon.shutdown` exits in all isolated cases.
Concerns: `mobile.py` needs a 60 s gap between runs; the C1 retry and the resync gate have no end-to-end test; the app-attached shutdown case is unverified; the earlier spot-check script was lost with the scratchpad.

## Retest after fix round 2
Branch `phone/phase2-client`, uncommitted, 2026-10-30. Fix round 2 per `plans/reports/fullstack-developer-261029-2000-phone-phase2-client.md`. Isolated endpoints and data dirs; ports 47310 (phone), 47320 and 47330 (tauri-driver); Vite on 1427 (free before start, stopped after). User daemon PID 10588 never touched.

| Check | Result |
|---|---|
| `pnpm typecheck` | pass |
| `pnpm test` | 581/581 (48 files) |
| `pnpm i18n:check` | 2178 keys, 0 missing |
| `pnpm build:phone` | pass; about 49.6 KB gzip total (budget 250) |
| `cargo test -p kelpie-proto -p kelpied --no-fail-fast` | kelpie-proto 23/23; kelpied 370/372 on the full run: 2 failed in `mobile::tests` (`a_page_that_says_nothing_of_acknowledgements_keeps_getting_frames`, `a_request_resolved_is_never_told_ahead_of_the_chat_lines_it_is_about`) with `http_head` 2 s timeouts under host load. Rerun of the mobile module alone: 41/41 pass. Classified as load flake in the test helper, not a regression |
| `mobile.py` x3 back to back, no wait | 49/49 each (exit 0). Each run preceded by a W7 precheck (see below); the "daemon of its own" check passes each time. The 60 s limiter carry-over did not recur |
| `mobile_protocol.py` | 78/78 |
| `mobile_resilience.py` | 48/48 |
| `acp_chat.py` | 56/56 |
| C2 end to end (reload or restart, then phone Deny sends Deny) | not run end to end. It needs a forced "unknown" answer through a stalled link, then a restart and a headless browser. Unit coverage for Claude, Codex and ACP is in `chat-observer.test.ts` ("after the log is loaded again, Deny sends Deny" and "a new request that reuses the id after a restart is answered as itself"). Not feasible in this retest without a new browser harness |
| `daemon.shutdown` with the app attached | Old PID 16896 exited 0.84 s after the call (call returned True). A new PID 13184 answered on the same endpoint 8 s later, so the app respawned a daemon. Confirms the developer's account. The respawned daemon was stopped in cleanup and the endpoint stopped answering |
| `daemon.shutdown` with no app (earlier retest) | exits 0.05-1.75 s, as before |

### W7 safety check (before each `mobile.py` run)
The reviewer's W7 concern was that `restart_daemon` shuts down whatever daemon the app uses. Before each run I checked, in order:
1. `KELPIE_ENDPOINT=kelpie-r3-mobile` and `KELPIE_DATA_DIR` (a temp dir) set in mobile.py's own process environment.
2. tauri-driver started with the same isolated endpoint and data dir (`KELPIE_LOAD_FORCE=L0:calm`).
3. `w7_precheck.py` reads `daemon.info` from the app's own daemon connection: endpoint `\\.\pipe\kelpie-r3-mobile` and the isolated data dir both match. (Run before each of the three runs; all three printed "W7 CHECK endpoint OK | data dir OK".)

All three conditions held, so the runs were not blocked.

Cleanup: my Vite, tauri-drivers (47320, 47330) and their msedgedriver children, the mobile test daemon, and the shutdown-check daemon are stopped. Test ports verified clear. My temp data dirs and tokens removed. Removed 115 of 116 `kelpie-p2-*` / `kelpie-shutdown*` folders in `%TEMP%`. One `kelpie-p2-acp-scratch-6u2_xg7q` folder is still in use by `python.exe` (PID 16936), a child of the user's daemon (PID 10588). Not mine; left in place, to be deleted once that process is gone. User app PID 8836 and daemon PID 10588 still running, created 10/29/2026 15:20:30 and 15:20:32 (unchanged).

Unresolved questions:
1. C2 end to end: worth building a forced-unknown browser harness, or is the unit coverage enough?
2. The `kelpie-p2-acp-scratch` folder: confirm PID 16936 is expected under the user's daemon.
3. The 2 `mobile::tests` 2 s timeouts under load: should the helper timeout be raised?

Status: DONE_WITH_CONCERNS
Summary: Fix round 2 holds on the e2e side: `mobile.py` 49/49 three times back to back with no wait (each behind a W7 precheck), `mobile_protocol` 78/78, `mobile_resilience` 48/48, `acp_chat` 56/56. Frontend gates pass. `daemon.shutdown` with the app attached: the old PID exits in under 1 s, and the app respawns a new daemon on the same endpoint.
Concerns: kelpied mobile tests flaky under load (2 timeouts, pass alone); C2 has unit coverage only, no end-to-end run; one temp folder is held by a process under the user's daemon and was left in place.
