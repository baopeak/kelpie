# Tester report: phone phase 1 resilience addendum (branch phone/phase1-resilience, uncommitted)

Date 2026-10-29. Host: Win10, 8 GB RAM, ~2.4 GB free at start, shared with other sessions. Dev kelpied build (`target/debug/kelpied.exe`), no release builds, no installers. Own endpoints/data dirs per script (`kelpie-<tag>-<pid>`); user daemon (PID 10588) and other sessions untouched.

## Results

| Suite | Result |
|---|---|
| `cargo test -p kelpied -p kelpie-proto` (CARGO_INCREMENTAL=0, -j 2) | kelpied **358 passed**, 3 ignored (357 + 1 added by me); kelpie-proto **23 passed**, 2 ignored. Exit 0 |
| `cargo clippy -p kelpied --all-targets` | 0 warnings, exit 0 |
| `pnpm typecheck` | clean |
| `pnpm test` | **460/460** (41 files). Earlier `spec-index` timeout did not recur on a quiet host |
| `pnpm i18n:check` | 2089 keys, 0 missing, 0 problems |
| `mobile_resilience.py` (dev exe, ~7 min) | **43/43** |
| `mobile_protocol.py` x3 | **78/78 each** (234/234). Developer's one 76/78 run did not recur |
| `attention.py` | **15/15** |
| `mobile.py` x3 | **SKIPPED**: no `msedgedriver` on PATH or in repo/Edge dirs (developer deleted theirs). Installing a driver binary is outside this task |
| `acp_chat.py` | **BLOCKED** by the same missing `msedgedriver`. See Unresolved |
| `stress.py` 30-min gate, `mobile_perf.py` | **NOT RUN**: all-core burn and release build on a memory-starved shared host. Developer's figures are unverified by me |
| `node --check crates/kelpied/mobile/app.js` | OK (no e2e drives the phone page) |

Failures: none. Flaky: none reproduced (developer's mobile.py 19/20 and protocol 76/78 not seen; the host was quieter for me).

## Change made by me (only test added)

- `crates/kelpied/src/mobile.rs`: `a_chat_log_with_no_room_for_ten_seconds_cuts_the_phone_as_a_slow_consumer`. Gap: the 4411 `slow_consumer` close after `SLOW_AFTER` (10 s) on a full bulk lane was not exercised by any test (lanes unit test covers only the primitive; no e2e hits 4411). Test fills the bulk lane, feeds one chat output frame through `adapt`, asserts no signal before 9 s and a signal within 12 s. Passes (10 s runtime). Not mutation-checked.

## Spot checks (code reading + tests)

- Load levels: thresholds, enter-after-2 / leave-after-10 s, <=2 moves/min, CPU >95 % for 3 s: matched spec (`load.rs` `Sample::wants`, `Machine::feed`). Unit tests pass. e2e pins L0/L2/L3 (`mobile_resilience` part 7). L1 has no e2e (unit only).
- Shedding ladder: unit (`the_ladder_sheds_by_level`, sync tests) and e2e pass. Typing boost (3 s calm rate at any level) is a deviation from the plain ladder (developer documented it; needs user sign-off, see Concerns).
- Heartbeat 20/28/36/44 s then drop at 45 s (4410): unit and e2e pass (e2e drop at 44-49 s). L2 doubles to 40/90 s: unit and e2e pass. Stall excuse and 10-min cap: unit pass.
- Ack window <=4 frames / 128 KiB, base = last acked, `sub {have_rev}` resume: unit and e2e pass. Ack cadence is every 2nd frame or 250 ms, not the spec's 500 ms/8 frames (documented deviation).
- `answer` results: `ok`, `dup`, `stale{answered_by,at}`, `refused`: unit and e2e pass (double tap, other device named, read-only refused).
- Pairing under load: L2 1/s, L3 1/5 s, 503 with the code not spent (`pairing_paced` before code check). Unit pass.
- Verb table closed; read-only device cannot reach write verbs. Unit pass.
- Lane back-pressure: control 64, bulk 4 frames; full control lane closes 4411. Unit covers lane; the connection-level path is now covered by the test above.

## Concerns (not blocking)

1. **Watchdog event name and consumer.** Plan says `daemon_unresponsive` to the app; code broadcasts `daemon.unresponsive` (`mobile.rs:485`, documented `architecture.md:401`). Nothing in `app/src` consumes it, and no test covers the 30 s path. Name is consistent with dotted event convention, but the app-facing signal is effectively unseen.
2. **Stale answer without `answered_by`.** `session.rs` `answer_bytes`: if a wait was answered more than 10 min ago (ring TTL `RESOLVED_TTL_MS`) and is still the current wait, the phone gets `stale` with `reason: answered` and no `answered_by`/`at`. Spec says `stale{answered_by, at}`. Narrow edge (wait open >10 min after answer); not fixed.
3. **`host_busy` is not phase 1.** The task's `host_busy` admission is specified for phases 4, 7, 8, 9. Phase-1 equivalent is pairing 503 (verified at unit level only).
4. **API level 16 skipped.** `API_LEVEL` goes 15 -> 17; no code refers to 16. Harmless, but note if another in-flight branch claims 16.
5. **Deviations needing user sign-off** (developer-documented): typing boost (input echo at calm rate for 3 s at L3); ack cadence (every 2nd frame / 250 ms); watchdog logs counters, not stack backtraces; `PHONE_V2_API` renamed to `PHONE_API` (no consumers).
6. **Load-level timings not clean on this host** (host itself at L2/L3 from other sessions per developer). I could not run the stress gate, so the 30-min gates remain on the developer's word.
7. **Test-environment hazard (GUI):** `tauri.conf.json` `devUrl` is `http://localhost:1427`. A debug app loads its frontend from whatever Vite serves 1427, which may be another session's code. GUI e2e can therefore drive another session's frontend. Should be guarded (dedicated devUrl or embedded assets) before GUI runs in shared hosts.

## Cleanup

- All processes I started (daemons, scripts, proxies) have exited. Checked by process list: remaining `kelpied`/`python`/`kelpie.exe` processes belong to the user or other sessions and were not touched.
- Removed 65 temp dirs (`kelpie-phone-*`, `kelpie-attention-*`) written in the last 4 h. 77 older ones left in place (not mine).
- Nothing committed or staged. Only file edited: `crates/kelpied/src/mobile.rs` (one test added).

## Unresolved questions

1. `acp_chat.py` "plugin acp-lab is already installed": cannot reproduce without `msedgedriver`. Static evidence: plugin dir is `kelpie_proto::data_dir()/plugins` = `KELPIE_DATA_DIR/plugins` (`plugins.rs:28-29`, `kelpie-proto/src/lib.rs:319`); script uses fresh `mkdtemp` per run and sets `KELPIE_E2E_DRIVER` to its own port; user's real `%LOCALAPPDATA%\Kelpie\plugins` is empty. So the error is not explained by real shared plugin state here. Developer's diff does not touch `plugins.rs`. Remaining hypothesis: the app's frontend came from the shared Vite on 1427 (concern 7). Needs a run with msedgedriver and the devUrl guarded.
2. Who installs msedgedriver on this host for GUI gates (`mobile.py`, `acp_chat.py`)?
3. Should the watchdog event be renamed to `daemon_unresponsive` per plan, and should the app show it?
4. Is the typing boost (and the ack cadence) accepted as the phase-1 contract, or should the plan's literal numbers apply?
5. Is the 10-min stale-without-`answered_by` edge acceptable, or should the ring outlive the TTL for open waits?

Status: DONE_WITH_CONCERNS
Summary: Rust (kelpied 358, proto 23), clippy, frontend (460 tests, typecheck, i18n), mobile_resilience 43/43, mobile_protocol 3x78/78 and attention 15/15 all pass. One gap closed (4411 slow-consumer test). GUI e2e (mobile.py, acp_chat.py) blocked by missing msedgedriver; 30-min stress and perf gates not run on this shared host.
Concerns/Blockers: acp_chat "already installed" not reproducible here (static evidence says not shared plugin state); watchdog event name/no app consumer; stale answer lacks answered_by after 10 min; GUI runs can load another session's frontend via devUrl 1427.

## Retest after fix round 1

Coordinator asked for the GUI runs blocked above. Fix round 1 is in the tree (`fullstack-developer-261029-1436`, "Fix round 1").

| Check | Result |
|---|---|
| `cargo test -p kelpied -p kelpie-proto` (rerun, -j 2, CARGO_INCREMENTAL=0) | kelpied **366 passed**, 3 ignored; kelpie-proto **23 passed**, 2 ignored. Exit 0. Dev binary rebuilt from this tree |
| `mobile_resilience.py` (own endpoint/data, daemon pinned L0:calm by the script) | **45/45** (43 + old-page checks) |
| `mobile_protocol.py` x2 | **78/78 each** |
| `mobile.py` x3 (GUI, own tauri-driver and endpoint) | **22/22 each** (66/66). Run 1 first failed my new frame check (see below), fixed, then passed 3 times |
| `acp_chat.py` x1 (GUI, own setup) | **56/56**. "plugin installs from its folder" passes; no "already installed" error |

### Setup used
- Vite: own `pnpm dev` from `D:\Works\Kelpie\app`; 1427 was free (checked before start). Stopped at the end (PID 14616; port verified closed).
- Driver per `mobile.py` run: `tauri-driver --port 47051/47061/47071 --native-port +1 --native-driver D:\Works\edgedriver\msedgedriver.exe`. Its Windows PID was taken from the port owner (bash `$!` is not a Windows PID). Stopped with `taskkill /T` on that PID.
- Endpoint/data: `KELPIE_ENDPOINT=kelpie-retest-mobile-N-<pid>`, `KELPIE_DATA_DIR` a fresh temp dir, `KELPIE_E2E=1`, `KELPIE_LOAD_FORCE=L0:calm` (as the developer did; the host is loaded and pairing is paced at L2). Set in the environment of each launched process. `mobile.py` makes its daemon calls through the app's `daemon_call`, so the app's inherited env is what isolates it.
- Phone: msedgedriver 9515 and phone server 48170 are the script's fixed ports; both were free before the runs.
- `acp_chat.py`: reviewed first. It uses fake ACP agents only (`fake_acp_agent.py`): no signed-in agents, no model calls. Its daemon and data are its own (`mkdtemp`); cleanup removes only its own endpoint token and temp dirs. Ran with `MSEDGEDRIVER=D:\Works\edgedriver\msedgedriver.exe`; the script starts its own tauri-driver on a free port.
- Port 4455 and the user's daemon (PID 10588) were not touched.

### Change to test scripts (developer's files)
- `scripts/e2e/mobile.py`: `Phone` now enables the browser performance log (`ms:loggingPrefs` / `ms:perfLoggingPrefs`). `goog:` prefixes are rejected by msedgedriver 155 ("log type 'performance' not found"); verified by a probe. New `Phone.socket_frames(method)` keeps every WebSocket frame read from the log. Two checks after the phone connects:
  - the page's own `auth` frame carries `caps` `["ack","hb"]` (token not printed);
  - the daemon's `hello` received by the page has `hb.legacy` false.
  Run 1 first failed the received-frame check: the first read consumed the log and dropped the received frames. Fixed in the helper, not in the assertion.
- `mobile_protocol.py`: not changed (the page check is in `mobile.py`, which drives the real served page).

### Evidence for the served page (step 4)
`1 frames sent, auth caps [['ack', 'hb']]` and `1 frames received, hello 1` (runs 1-3). The page is `crates/kelpied/mobile/app.js`, served by the daemon, opened in headless Edge.

### Notes
- The earlier `acp_chat` "plugin acp-lab is already installed" did not recur. With my Vite on 1427 serving this tree, it passed on the first run. That fits the earlier hypothesis that another session's frontend on 1427 caused it (the developer's report says the same). Not proven: the original failure was not re-created on the other session's Vite.
- `host_busy` is still not part of phase 1 (phases 4, 7, 8, 9).
- Stopped and checked: Vite (1427 closed), tauri-driver trees, msedgedriver on 9515 and the probe's port, and the app and daemons the GUI runs started. Remaining `kelpie.exe`, `kelpied.exe` and node processes belong to the user or other sessions. Removed the temp dirs this retest created (`kelpie-retest-*`, and `kelpie-phone-*` written since Vite started).
- Not run this retest: stress gate, `mobile_perf.py`, `attention.py` (not requested; attention 15/15 earlier on the previous build).

Unresolved:
1. Keep the performance-log check in `mobile.py`? It needs `ms:` capabilities, which depend on msedgedriver behaviour.
2. The GUI runs use `KELPIE_LOAD_FORCE=L0:calm` (developer's choice), so pairing is never paced in them.

Status: DONE
Summary: Retest of round-1 fixes passes: kelpied 366, kelpie-proto 23, resilience 45/45, protocol 78/78 x2, mobile.py 22/22 x3 with the served page shown to send caps ack+hb and to receive hb.legacy false, acp_chat 56/56 once. Everything started was stopped.
Concerns/Blockers: none blocking. The phone-page caps proof depends on msedgedriver's `ms:` performance log (checked on 155). GUI runs pin L0 load.
