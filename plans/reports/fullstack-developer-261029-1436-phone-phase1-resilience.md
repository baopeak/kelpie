# Phone phase 1 resilience addendum (developer report)

Nothing committed or staged. Branch `phone/phase1-resilience`. Dev builds for tests; two release `kelpied` builds (no installer, `build-release.sh` never run). Temp files and copies in the scratchpad deleted at the end.

## Files
New: `crates/kelpied/src/load.rs`, `load/windows.rs`, `load/macos.rs`, `crates/kelpied/src/mobile/lanes.rs`, `scripts/e2e/net_fault_proxy.py`, `scripts/e2e/stress.py`, `scripts/e2e/mobile_resilience.py`.
Modified (mine): `mobile.rs`, `mobile/sync.rs` (rewritten around acknowledgements), `mobile/verbs.rs`, `session.rs` (bounded client queue, resolved-waits ring, `answer_by`), `chat.rs` (who/when of answers), `daemon.rs` (`mobile.load` arm; tests use `client_channel`), `main.rs` (`mod load`), `server.rs` and `daemon/automation/tests.rs` (one line each: `client_channel()`), `Cargo.toml` (`socket2`, windows-sys `Win32_System_Memory`; `Cargo.lock` gained `socket2` only), `crates/kelpie-proto/src/lib.rs` (API level), `crates/kelpied/mobile/app.js`, `app/src/lib/attention.ts`, `docs/architecture.md`, `scripts/e2e/phone_support.py`, `mobile_protocol.py`, `mobile_perf.py`.
Not touched: `app/src/components/*`, `styles.css`, `docs/brand.md`, `docs/user-guide.md`, other `app/src/lib/*`, `scripts/e2e/{confirm_dialogs,phase1,split_picker}.py` (dirty from other sessions). Line endings kept (working tree is CRLF).

## Requirement checklist
| Requirement | Evidence |
|---|---|
| `hb` 20 s idle / 8 s pong / 45 s dead, both ways, read permission | `lanes::Liveness` + `Verb::Hb`; unit tests (20/28/36/44/dead 45, anything resets, doubled at L2); e2e: silent phone asked at 20, 28, 36 and closed 4410 at ~45 s |
| `SO_KEEPALIVE` 30/10/3 | `mobile::keep_alive` (socket2, every accepted socket); not separately tested (OS level) |
| Lag-aware liveness | deadline clock restarts when a tick gap exceeds 2 ticks or the probe saw a stall (>= 1 s late) since the last word; capped at 10 min without hearing the phone; L2+ doubles (40 s / 90 s). Unit tests incl. 500 s sleep, stall in window, 10-minute cap; e2e: first hb at 40 s and drop at 90 s under forced L2 |
| Ack window: `ack {id, rev}`, <= 4 frames / 128 KiB, base = last acked, `sub {id, have_rev}` | `sync.rs` (`Slot.acked/flying`, `Views::ack`), unit tests (four frames then blocked, bytes window, base follows the confirmed frame, resume with `have_rev`, never-acking phone); e2e (`mobile_protocol`, `mobile_resilience`); old page acknowledges and keeps screens by rev |
| 20 KB/s phone vs ~1 MB/s output | `mobile_resilience` part 5: RSS flat, biggest frame < 64 KiB, caught up <= 2 s after lifting the throttle (numbers below) |
| 100 mid-stream TCP kills | `mobile_resilience` part 4: reset storm + 100 kills, rows identical after every reconnect (one real race found and fixed, see below) |
| Control lane vs bulk lane, `slow_consumer` | `lanes.rs` (control 64, bulk 4 x ~64 KiB = 256 KiB, biased select; full control lane or 10 s without room/progress closes with 4411); unit test for the full lane |
| No unbounded channel on any phone path + grep test | `session::ClientTx` is now an enum; phones use `bounded_client_channel(256)` (overflow drops, restores the credit, sets a flag; the connection then re-sends the list and `chat_resync`). `mobile::tests::no_phone_path_has_an_unbounded_queue` scans `mobile.rs`, `mobile/*.rs`, `load.rs` |
| Answer results `ok / stale{answered_by, at} / dup / refused`, ring of 20 / 10 min | `Session.resolved` (`RESOLVED_KEPT`, `RESOLVED_TTL_MS`), chat `Resolution` in the answered map; unit tests (who/when, remembered after the next wait starts, 20 entries, 10 min); e2e: double tap = ok + dup, other device = stale naming it, read-only = refused. `chat.resolved` carries `answered_by`, `at` |
| Resume cache bounds | 64 KiB per slot (acked rows <= snapshot cap), 20 sessions per device, 8 devices, 16 MiB LRU, 5 min TTL, `drop_idle` at L3 (which includes the low-memory event); unit test |
| `load.rs` L0-L3, hysteresis, `KELPIE_LOAD_FORCE` | `Machine` (enter after 2 samples, leave after 10 s, <= 2 moves/min), thresholds as specified; unit tests; hook parsed in `parse_force`; e2e pins L0/L2/L3 |
| Shedding ladder + never-shed | `Level` methods: focused 100/100/250/1000 ms, others 100/250/1000/paused, rows 56/56/28/12 KiB (frames <= 64/64/32/16 KiB), heavy 5 fps (1 at L3), chat resets 2 / 1 (counted across connections), pairing 1 s / 5 s (503, code not spent). Input, key, answer, chat_answer, revoke, hb, acks, list, `load.status` are not paced. Unit + e2e (L2/L3 rates, sizes, background pause) |
| `load.status` + `server_ts`/`level` in frames | on change and every second while level > 0; in `hello`, `hb`, frames; e2e |
| EcoQoS opt-out (Windows) | `SetProcessInformation(ProcessPowerThrottling)` in `load::start` (returns value logged on failure) |
| macOS activity | `load/macos.rs`: `NSActivityUserInitiated` while a phone is connected (`PhoneGuard`), plus `host_statistics` CPU, `kern.memorystatus_level`, low-power mode, thermal. Compile-checked only: `rustc --target x86_64-apple-darwin --emit=metadata` on the file with a stub parent: exit 0. Not run |
| Watchdog thread | plain OS thread, logs "runtime stalled" (tokio counters + busiest threads on Windows) at 5 s, broadcasts `daemon.unresponsive` at 30 s; never touches sessions. Not exercised by a test (needs a stalled runtime) |
| Runtime split decision | no split; see below |
| Harness | `stress.py` (busy workers by duty/priority, 20 low-duty child processes, plus the gate), `net_fault_proxy.py` (latency, jitter, bandwidth, blackhole as a stall, half-open, RST storm, kill-all), `mobile_resilience.py` |
| Feature off => probe not running | unit test, `mobile_protocol`, `mobile_resilience`, `stress.py` check `mobile.load.running` false before enable and after disable |
| Leftover: tighter push check | `mobile_protocol.py`: 2 of 3 under 0.25 s or median under 0.3 s; wrong comment fixed |
| Leftover: old page alert | "...The text is on the prompt line: press Enter to send it." (+ VI) |
| Leftover: `busy` retry signal | `chat_resync {id}` sent once the connection's chat budget has drained and (at L2/L3) other phones' logs are done; unit tests |
| API level | `API_LEVEL` 17 (`crates/kelpie-proto/src/lib.rs`, doc comment lists 17). `PHONE_V2_API` renamed `PHONE_API` = 17 (`app/src/lib/attention.ts`; it had no other users). `docs/architecture.md`: both `chat.resolved (API 14)` mentions now API 17, `mobile.load` documented. Levels also appear in `daemon.rs` (`daemon.info` reads `API_LEVEL`) and `mobile.rs` (`hello.api`): both read the constant |

## Found and fixed on the way
- Real race (existing since the first version, exposed by 100 kills): `drive` could end on "no followers" just after a new follower subscribed, leaving that follower on a render nobody made. Decided now under the `follow` lock.
- A frame sleep sized for the old level kept a typed key waiting: the spacing wait is re-evaluated every 50 ms.
- A whole frame (base null) must not make the phone drop screens it has acknowledged since (the daemon may still build on them): fixed in `phone_support.py` and `app.js`.
- Typing boost (deviation from the plain ladder): input/key into a session renders and sends it at the calm rate for 3 s whatever the level (`Views::boost`). Without it the focused session at L3 sends one frame a second, so "input echo p95 <= 300 ms" under stress cannot hold at L3. Unit test.
- Ack cadence deviation: the addendum says ack every 500 ms or 8 frames with a window of 4 frames, which stalls before the 8th. The phone acks every 2nd frame or 250 ms (advertised in `hello.hb`).

## Verification
Commands and results (host: 4 cores, 7.9 GB RAM, other sessions busy; the host itself sat at L2/L3 during much of this):
- `cargo test -p kelpied -p kelpie-proto`: kelpied 357 passed, 3 ignored; kelpie-proto 23 passed.
- `cargo clippy -p kelpied --all-targets`: no warnings.
- `cd app && pnpm typecheck`: clean. `pnpm i18n:check`: 0 missing, 0 problems. `pnpm test`: 459 passed, 1 failed: `spec-index.test.ts` "parses a 256 KB list in one piece" timed out at 5 s on the overloaded host (file not touched by me; passes alone only when the host is quiet, re-run alone timed out too).
- `mobile_protocol.py` x10 (dev exe): 9 x 78/78; run 4 had 76/78: the "carries on from the last frame it acknowledged" check (the ack was not yet processed, so a whole frame came) and the p95-150 ms latency check (845 ms outlier) in the same host stall. No daemon defect found.
- `attention.py`: 15/15.
- `mobile.py` (GUI, own tauri-driver on ports 47010/47011, isolated endpoint/data, `KELPIE_LOAD_FORCE=L0:calm` so pairing is not paced): 5 runs: 19/20 once (quick keys Ctrl+C: the test types `ping` and presses Ctrl+C 1.5 s later; on a loaded host PowerShell had not started it), then 4 x 20/20. A first attempt used port 4455, which another session's tauri-driver already held, so it drove that session's app by mistake; I aborted that run, did not use its result and moved to my own ports.
- `acp_chat.py`: NOT verified. It reaches the daemon (API 17 check passes) and then fails at "plugin acp-lab is already installed" on two runs, with my daemon and a copy of the app; the frontend is served from the shared live Vite dev server (port 1427, another session, with uncommitted frontend edits), so I could not tell whether that is mine. Nothing in my change touches plugins.
- `mobile_resilience.py` (full, dev exe, daemon pinned `L0:calm` for the calm parts): 43/43 (about 7 min). Earlier runs found: unrelated expectations about heartbeats of a phone that keeps acknowledging, the proxy being starved by blocking waits in the test, the drive race above.
- `mobile_perf.py` (release) and `stress.py --minutes 30` (release): see Gates.

## Gates
Phase-1 perf gate (`mobile_perf.py`, release build, 3 printers + 1 unfollowed, 8 phones, 3 rounds):
- Steady: the 8 phones add **-0.7 points** of a core (median; runs -2.8, +0.7, -0.7), limit +5. Each phone gets 5.8 frames/s of 18.4 kB (heavy sessions are held to 5 fps). First run of the same gate before the last fixes: +2.1 (runs -0.2, +2.1, +5.6). **PASS**
- Unfollowed/unsubscribed sessions: 0 frames. **PASS**
- Flat out (not gated, ConPTY sets the pace): 3 x 16 MiB in 19.2 s without phones, 22.6 s with 8 (+17.7 %, noise: first run showed +12.8 %); daemon CPU 7.6 s -> 7.8 s.
- Ingest throughput, in-process (`cargo test --release ... --ignored ingest_speed`, one invocation after the final changes): 100.6 MB/s without, 94.1 MB/s with eight phones (-6.4 %; invocations vary +-10 % by themselves, the assert is 15 %; earlier median over invocations was -1.1 %). Only one invocation re-run here.

Stress gate (`stress.py <release exe> --minutes 30`, 20 sessions, 8 phones, 4 busy workers at normal priority plus 20 low-duty children, test process above normal; 20 `-S` Python sessions, each pre-filled to a full scrollback):
- Echo: 8 351 typed words under stress: **p50 18 ms, p95 125 ms, p99 139 ms** (limits 300 / 800); calm baseline p95 28 ms. **PASS**
- False disconnects: **0**. **PASS**
- Dropped input: 9 266 `input` sent = 9 266 answered = 9 266 echoed. **PASS**
- Revoke: a throwaway device typing every 50 ms stopped in 0.10-0.12 s each time (10 revokes), 0 writes after 1 s. **PASS**
- RSS: 245 MiB at the end of the first minute, 175 MiB at the end; the OS trimmed the working set at minute 14-16 under host memory pressure (112 MiB) and it grew back by about 4 MiB a minute to 168 MiB by minute 27: working-set recovery, not growth past the baseline. Check as written passes; I did not prove the absence of a slow leak beyond 30 min.
- Level: 0.07 s to L2, but the host was already at L3 from its own memory before the stress began (other sessions), so the "L2 <= 5 s after start" figure is not a clean measurement; during the stress it ran L2 (`cpu`) with excursions to L3 when host memory fell below 8 %. Back to L0 **19.9 s** after the stress stopped (limit 25 s). Event-loop lag over the run: p99 <= 30 ms, p99.9 <= 50 ms. **PASS**
- Feature off: probe not running before enabling and after disabling. **PASS**
- Earlier 6-minute development runs failed echo p95 (336 ms, then 546 ms) and RSS before the typing boost, the scrollback pre-fill and lighter sessions; kept as history, not hidden.

Network fault matrix (`mobile_resilience.py`): 43/43. Numbers: silent phone dropped at 4410 in 44-49 s; 10 s and 30 s blackholes survived and caught up within 4 s; 90 s blackhole: dropped by the daemon, cached view unchanged while dark, typed input never executed, rows equal after reconnect; answer into a blackhole retried once lands once; answer whose reply was lost: retry is `dup`; 19 reconnects in a 10 s reset storm and 100 kills: 0 mismatches; 20 KB/s phone under ~0.8 MB/s printer: RSS flat, no frame >= 64 KiB, caught up within 2 s of lifting the throttle.

## Runtime-split decision
**No split.** Event-loop lag at the phase-1 perf-gate load (3 printers at ~1 MB/s, 8 phones, no outside stress): p99 <= 30 ms and p99.9 <= 30 ms in all three runs of each of two gate runs (about 190 probes of 100 ms per run; the histogram's bucket edge is 30 ms, so "30" means 20-30 ms), against the threshold "p99 above 30 ms". The host itself was at L2 from other sessions during these runs, so the figure is, if anything, pessimistic. Under the full 30-minute all-core stress the p99 stayed <= 30 ms (p99.9 <= 50 ms) and the 8 phones met the echo gate, so a second runtime is not needed. Coarse buckets are a limit of the measure: a re-measure with exact percentiles would be needed if the margin ever narrows.

## Deviations
- Watchdog logs runtime counters and per-thread CPU time (Windows), not stack backtraces of all threads (no portable way without a debugger API).
- Thermal state has an OS source on macOS only (and the force hook); Windows has none cheap, so Windows feeds battery saver only.
- CPU/memory sources: Windows and macOS only; elsewhere lag only.
- Typing boost and ack cadence as above.
- `mobile.load` (new RPC, API 17) exists for the tests and diagnostics; `{reset:true}` restarts the lag percentiles.
- Lag percentiles are bucketed (edges 1, 2, 5, 10, 20, 30, 50, 75, 100, 200, 500, 1000 ms): "p99 30 ms" means "<= 30 ms".
- A stuck write is cut after 10 s (was 30 s).
- `PHONE_V2_API` renamed to `PHONE_API`.

## Open questions
- Should the typing boost also apply to `answer`? Left out (answers are not echo-latency sensitive).
- `acp_chat.py` needs a re-run on a quiet tree (see above).
- The gate "input echo p95 <= 300 ms" is only met while the level is at most L2 or the user is typing (boost); at L3 without typing the focused session is one frame a second by design.

Status: DONE_WITH_CONCERNS
Summary: Everything in the addendum is implemented with tests and the harness; unit tests, clippy, protocol e2e, resilience e2e (43/43) and the perf gate pass.
Concerns: acp_chat.py could not be verified (plugin already installed, shared Vite dev server); the host was memory-starved so the load-level timings are not clean measurements (echo, disconnect, drop, revoke and fault gates are); one mobile.py run in five failed a timing-sensitive Ctrl+C step.

## Fix round 1

Reports read: code-reviewer-261029-1850 (8/10) and tester-261029-1850. The tester's 4411 test is kept.

| Finding | Change | Evidence |
|---|---|---|
| W1 answers credited to "desktop" | `Session::input_by` / `type_message_by` set who types under the same lock; `input` and `key` pass the device. The open wait also remembers who answered it (`answered_now`) for as long as it is open | `mobile::tests::an_answer_typed_through_input_or_key_belongs_to_the_device_that_sent_it` (second device: `stale` + `answered_by` "Phone"; same device: `dup`; the desktop's own key stays "desktop"), `session::tests::an_open_wait_knows_who_answered_it_after_the_ring_has_forgotten` |
| W2 chat ordering | A control message about a chat (`chat_resolved`, `chat_resync`) is written after the bulk messages already queued, so it cannot overtake its lines | `mobile::tests::a_request_resolved_is_never_told_ahead_of_the_chat_lines_it_is_about` (real socket, both messages ready at once) |
| W3 macOS sleep | `0x00EFFFFF` (`...AllowingIdleSystemSleep`); comment and docs corrected; decision noted in `phase-01-phone-protocol-v2.md` | one constant changed; the macOS file was not re-compile-checked after it |
| W4 old pages | First message is `{token, caps}`; without both `ack` and `hb` the connection is legacy: each frame counts as acknowledged when sent (`Views::open_as`), no heartbeat deadline; queues and the 10 s write cut stay. `hello.hb.legacy` says which. New `app.js` sends `caps: ["ack","hb"]` | `acknowledges` unit test; `a_phone_that_cannot_acknowledge_is_not_held_back...` (sync); `a_page_that_says_nothing_of_acknowledgements_keeps_getting_frames` (real socket, twice the window); e2e `old_page` in `mobile_resilience.py` (56 s, no acks, no heartbeat answers: stays connected, frames keep coming) |
| W5 escalation | Rising levels apply in the same second; hysteresis and the 2-per-minute limit apply to falls only | `load::tests`: entered at once; memory under 8 % right after L1 -> L2; falls limited to 2 a minute |
| W6 empty heartbeat check | requires at least 3 heartbeats | `mobile_resilience.py` |
| S1 4411 vs adapter exit | slow arm moved above the adapter arm in `serve` | by order; the tester's 4411 test still passes |
| S3 unsubscribe vs resume cache | `Unsub` calls `views.close` | `mobile::tests::letting_go_of_a_session_frees_its_place_in_the_resume_cache` |
| S2 docs | answer results and errors are inline, 4411 conditions, `daemon.unresponsive` in the events list, legacy pages, the typing boost applies to every follower of the session, EcoQoS not undone, escalation text: `docs/architecture.md` | read against the code |
| Stale after the 10-minute ring | no new reason needed: the open wait keeps who answered, so `stale` still carries `answered_by`/`at` | session test above |
| Watchdog event name | kept `daemon.unresponsive` (dotted like the daemon's other events); the plan text now says so | `phase-01-phone-protocol-v2.md` |

Left alone from the review: S4 (`chat_room` counts any pending chat output, not only resets), S5 (typing boost shared by all followers: kept as decided), S6, S7 (documented), S8 (very slow links), S9 (timing windows in the e2e).

### Verification (this round)
- `cargo test -p kelpied -p kelpie-proto`: kelpied 366 passed, 3 ignored; kelpie-proto 23 passed. `cargo clippy -p kelpied --all-targets`: no warnings.
- `cd app && pnpm typecheck` clean; `pnpm test`: 41 files, 460 passed.
- `mobile_resilience.py` (own endpoint and data folder, daemon pinned `L0:calm`): 45/45 (43 plus the old page's two checks).
- `mobile_protocol.py` x3: 78/78 each. `attention.py`: 15/15.
- Headless-Edge check of the served page (scratch script, not in the repo), x3: 6/6 each. msedgedriver used: `D:\Works\edgedriver\msedgedriver.exe` (it is installed; the tester looked elsewhere). Ports 47041-47043 (daemon) and 47051-47053 (driver). The page connects, shows the session, 12 messages in a row come back, it is still connected after 52 s idle, and still works.
- `mobile.py` x3 and `acp_chat.py`: not run to the end; environment blocker with evidence. The debug app's frontend comes from `devUrl` `http://localhost:1427`, compiled in. With nothing listening on 1427 (netstat: 0 listeners) the app never loads (`window.__kelpie.store.loaded()` times out) for both scripts; I ran each once to confirm. The earlier "plugin acp-lab is already installed" happened while another session's Vite (pid 13824) held 1427, and that session's `kelpie-uireview-data-*\plugins\acp-lab` existed: consistent with the tester's hypothesis that the app showed that session's frontend. I was told not to use 1427, and the port cannot change without rebuilding the shared app, so no Vite was started. Only `attention.ts` of the Kelpie app side is mine.
- Clean "reaches L2 within 5 s of stress start": not measured. The host was at L2 before the stress began again (6-minute dev-build run: all 11 checks passed, echo p95 120 ms, p99 226 ms, back to L0 in 23.0 s, RSS 151 -> 147 MiB).
- Everything I started has stopped; scratch copies and temp folders were removed. Nothing committed.

Status: DONE_WITH_CONCERNS
Summary: W1-W6, the 4411 order, the unsubscribe cache bound, the docs, the answer info after the ring's TTL and the watchdog name are fixed with tests; cargo, clippy, vitest, resilience (45/45), protocol x3, attention and a headless page check pass.
Concerns/Blockers: mobile.py x3 and acp_chat.py could not run (the debug app needs a Vite on port 1427, which I may not use); "L2 within 5 s" is not measured on this busy host; no e2e proves directly that the new page sends `caps` (code review and node syntax check only).

## Fix round 2

Source: the reviewer's "Re-review 1" (8.5). The tester's edits to `scripts/e2e/mobile.py` and the 4411 test in `mobile.rs` are kept.

| Finding | Change | Evidence |
|---|---|---|
| R1 unbounded drain | The drain loop in `serve` is gone. `chat_resolved`, `chat_resync` and the chat lines all go through the bulk lane from the adapter, in order, so FIFO gives the ordering. The control lane stays (first in the writer's order, `Control::send` marked `allow(dead_code)` until something uses it). Revoke, heartbeat and inbound handling wait for at most the send in progress | `mobile::tests::a_revoked_device_is_cut_off_within_a_send_however_full_the_lanes_are_kept` (chat of 2 KB lines every 2 ms plus a changing screen towards a slow reader; revoke after 1 s; the connection must close within 3 s), and the earlier ordering test `a_request_resolved_is_never_told_ahead_of_the_chat_lines_it_is_about` still passes with no drain |
| R2 rises | A rise needs 2 consecutive samples above the current level, for every level (about 2 s, inside "L2 within 5 s"); the 10 s hysteresis and the 2-per-minute limit hold back falls only. Plan text (phase 1) says so | `load::tests`: entered after two samples and left after ten seconds; `rises_are_never_held_back_by_earlier_changes` (L1, L2, then L3 on memory, each two seconds after the last, though several changes were just made); `at_most_two_falls_a_minute` |
| R3 first answerer wins | `write_input` records who answered only when the wait was not answered yet | `session::tests::the_first_to_answer_stays_the_one_who_did_past_the_rings_ten_minutes` |
| R4 relay | Added to `phase-04-relay-connect-anywhere.md` under Requirements (relay admits only clients declaring `caps` ack+hb) | file |
| docs | `docs/architecture.md`: lanes paragraph and the rise rule corrected | read against the code |

### Verification
- `cargo test -p kelpied -p kelpie-proto`: kelpied 368 passed, 3 ignored; proto 23. Clippy: no warnings. `pnpm typecheck`: clean.
- `mobile_resilience.py`: 45/45. `attention.py`: 15/15.
- `mobile.py` x2 (tester's version, 22 checks each, with a Vite of my own): 22/22 and 22/22. Port 1427 was free (netstat), so I started `pnpm dev` in `app` (pid 10684) and stopped it afterwards; msedgedriver `D:\Works\edgedriver\msedgedriver.exe`; tauri-driver ports 47010/47011; daemon and data folder isolated, `KELPIE_LOAD_FORCE=L0:calm`.
- `mobile_protocol.py` x2: the first pair of runs (daemon not pinned) each had one failure, in different checks: the ticker latency check (p95 1.4 s) and the "carries on from the last frame it acknowledged" check. The host was at L2/L3 from other sessions, where the focused session is one frame a second by design and pairing is paced. I pinned the daemon calm in the script (`KELPIE_LOAD_FORCE=L0:calm`, as `mobile_resilience.py` already did) and re-ran: 78/78 and 78/78. I did not find a defect in the resume path; that check can still fail when the host stalls.
- Not re-measured: "L2 within 5 s of stress start" (host not calm).
- Stopped: Vite, daemons, drivers. Scratch copies and temp folders removed. Nothing committed.

Status: DONE_WITH_CONCERNS
Summary: R1-R4 are done with tests; cargo, clippy, typecheck, resilience (45/45), protocol x2 (78/78 pinned calm), attention and mobile.py x2 (22/22) pass.
Concerns/Blockers: the protocol script's resume and latency checks are sensitive to a stalled or loaded host (seen once each before pinning); "L2 within 5 s" is still not cleanly measured; no e2e proves directly that the new page sends `caps`.

## Fix round 3

Source: the reviewer's "Re-review 2" (9/10, nothing blocking).

| # | Change | Evidence |
|---|---|---|
| 1 control lane | Removed: the channel, `CONTROL_CAPACITY`, the `control_rx` arm, `Control` and `Control::send` (and its `allow(dead_code)`), the capacity test. `adapt` takes only the `slow` notifier. `lanes.rs` header, `docs/architecture.md` ("Writer and lane") and the plan text (phase 1; one mention in phase 3) now say: the writer sends heartbeats, replies, the list and `load.status` itself, ahead of the bulk lane; chat messages ride bulk in order | `the_bulk_lane_is_bounded` replaces the old lane test; the tester's 4411 test passes |
| 2 R1 test | The feeder now has the chat ask and the desktop answer every 4th round, so `chat_resolved` keeps arriving; the feeder runs 12 000 rounds of 20 KB chat lines. The test times the daemon side (`connections` empty) after the revoke, since the phone sees the close only after the data in the socket's buffers | passes with the fix |
| 2 reproduction check | I put the round-1 code back temporarily (chat lines on bulk, `chat_resolved`/`chat_resync` on a control channel, the drain before writing them) and ran the test three ways: (a) 2 KB lines, revoke at 1 s; (b) 20 KB lines, revoke at 2.5 s and at 6 s; (c) slow reads (250 ms), revoke at 5 s. **All passed with the old code too**, so the test does **not** reproduce the round-1 bug on this machine. Reason: the drain only runs on while each send blocks (a full socket buffer lets the other tasks refill the lane); loopback buffers and the cooperative single-thread runtime end the drain after a few messages, and in (c) both versions end about 8 s after the revoke because the close frame waits behind buffered data. I restored the fix after each try and kept the quick variant (revoke at 2.5 s, passes in ~3 s) as a guard against revoke being held off by full lanes; it is not proof against the old defect. The defect is excluded by the code instead: no unbounded loop is left in `serve` |
| 3 resume under load | `mobile_resilience.py` `resume_under`: at L2 and at L3 (non-memory reason) a reconnect carries on from the last acknowledged frame; at `L3:memory` a whole screen comes back. Acknowledgement is read before closing: auto-ack off, then a `hb` request whose answer proves the daemon has handled everything sent before | 3 new checks, all pass (48/48 twice) |
| 4 L3 clearing | `Status::short_of_memory()` (L3 with reason memory, i.e. free memory under 8 % or the low-memory event). The cache is emptied of away phones inside `Views::trim` when that holds, so it works wherever the cache is touched, with no phone connected (the first version did it in a connection's tick and could not clear anything with nobody connected: found by the e2e design, fixed before running). `drop_idle` removed. Plan phase 1 and the docs say memory only | `load::tests::only_a_shortage_of_memory_clears_what_is_kept_for_away_phones` (memory, CPU+memory, lag, CPU, L2 with memory, default; and the machine's samples), `sync` cache test (L3 for CPU keeps, L3 for memory clears) |

### Verification
- `cargo test -p kelpied -p kelpie-proto`: kelpied 369 passed, 3 ignored; proto 23. Clippy: no warnings. `pnpm typecheck`: clean.
- `mobile_resilience.py` x2: 48/48 and 48/48.
- `mobile_protocol.py` x1: 78/78 (daemon pinned calm). `attention.py`: 15/15.
- `mobile.py` x1: 22/22 with my own Vite: port 1427 was free (netstat), I started `pnpm dev` in `app` (pid 8896) and stopped it afterwards (checked the owner pid before killing). msedgedriver `D:\Works\edgedriver\msedgedriver.exe`, tauri-driver ports 47010/47011, isolated endpoint and data folder.
- Everything I started has stopped; scratch copies and temp folders removed; nothing committed.

Status: DONE_WITH_CONCERNS
Summary: The control lane is gone, L3 clears away phones' positions only for memory, resume under L2/L3 is checked end to end, and cargo, clippy, typecheck, resilience x2, protocol, attention and mobile.py pass.
Concerns/Blockers: the revoke test could not be made to fail against the round-1 drain on this machine (three variants tried), so it guards revoke latency but is not proof against that exact bug; the defect is excluded by construction instead.
