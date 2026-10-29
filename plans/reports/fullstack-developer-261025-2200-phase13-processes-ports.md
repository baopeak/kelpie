# Phase 13: Processes, ports & preview (developer report)

Status: DONE_WITH_CONCERNS (see Concerns). Committed on `phase13/processes-ports`; nothing pushed. Debug builds for e2e, one
release `kelpied` (no installer) to measure the scan budget the perf README defines against the release build.

## What the phase delivers
- Daemon (API 16): `process.list|ports|port_owner|kill|kill_children`, user role only (`auth.rs`). `procs.rs` (trees, identity,
  guarded stop), `ports.rs` (listening sockets per OS), `daemon/process.rs` (RPC), `kelpie_proto::procs` (shared `child_map` /
  `descendants`, also used by the app's memory meter in `sys_stats.rs`), job PID list in `proc_tree.rs`.
- App: `:port` chips on tab and sidebar row (click: browser pane; menu: *Open preview beside*, browser tab, copy), Processes tab
  in the right panel (tree with name, PID, RAM, CPU, ports; Kill; Kill children; *Left the session* orphans; "port still in use
  by …"), palette *Processes and Ports*, gated on API 16. Looks are triggered by the app only.
- Docs: `docs/architecture.md` (RPC list, "Processes and ports"), `docs/user-guide.md` ("Processes, ports and preview"), README one line.

## Changes in this session (on top of the previous developer's work)
1. **Leftovers of the parent-walk change**: a remaining `scan(.., false)` in a test fixed; app `listProcesses(session)` without
   `orphans`; `ORPHAN_EVERY` and the deep-look logic removed from `processes-panel.tsx`; `orphans` argument removed from
   `processes()` in `processes.py`.
2. **Why the tree needs the parent walk (measured)**: a real ConPTY `pwsh` adopted into a job like a session: the job held only
   `pwsh.exe`; both a foreground `python -m http.server` and a `Start-Process` one were outside it (walk found them). This pwsh
   is the Store (MSIX) build, whose children are placed outside the job. So descendants-by-parent + job members stays.
3. **Scan cost** (the walk made it miss the budget): per scan, Toolhelp snapshot 5.8 ms + sysinfo refresh 6.6 ms (release,
   243 processes; both enumerate every process). Raw `NtQuerySystemInformation(SystemProcessInformation)` = 1.9 ms for the same
   data. Windows now reads the table once per scan with that call (parent, create time, name, working set, CPU time of every
   process; CPU % from the delta between scans) and reads only the exe path from the handle of each listed process. Unix keeps
   sysinfo. The parent walk is the shared `child_map`/`descendants` (start-time-checked links) on every OS. Removed
   `proc_tree::parent_table` and the Windows-only walk. New test: the table has each process with its parent and the start time
   its handle reports. Features added: `Win32_System_WindowsProgramming`, `Wdk_System_SystemInformation`.
4. **Kill children kept the launcher but stopped the agent** under a non-npm `claude.cmd` (`cmd.exe` root, agent = its direct
   child): `keep_set` now also keeps what a batch launcher (`cmd`) starts directly, for agent sessions only (shell sessions keep
   only the shell). Unit test extended.
5. **e2e fixes** (`processes.py`): the four extra servers were typed into the shell whose foreground ran the first server, so
   they only started after Kill ended it (the earlier "not in the tree" symptom); now a second shell starts them and the scan
   is measured over both sessions. Guard step uses that shell's servers. Precedence bug in the "PID, memory" check. Waits for the
   panel to show the agent's port before Kill children (2 s panel look race). Right-click and pick in one call (the menu closes on
   window blur; the new browser pane can take focus). Teardown runs `process.kill_children` per session before
   `daemon.shutdown` (a `Start-Process` server is outside the job). Budget check: asserted for a release daemon, reported for a
   `debug` one (see Concerns).
6. **Chip latency**: output now brings a planned look forward (never sooner than 2 s after the previous one), so a server's
   "Serving HTTP…" line shows its chip without waiting for the 3 s poll (3.9 s -> 0.9–3.0 s).

## Verification (numbers)
- `cargo test -p kelpied -p kelpie -p kelpie-proto`: kelpied 346 passed / 3 ignored, kelpie-proto 25 / 2 ignored, kelpie 169 /
  5 ignored (kelpie run before the procs rewrite; it does not depend on kelpied, kelpie-proto unchanged since).
- `cargo clippy -p kelpied -p kelpie -p kelpie-proto --all-targets -- -D warnings`: only master's pre-existing
  `too_many_arguments` at `crates/kelpied/src/mobile.rs:892` (not phase 13; not touched). With that one lint allowed on the
  command line the run is clean.
- `pnpm typecheck` clean; `pnpm test` 41 files / 458 tests; `pnpm i18n:check` 0 missing / 0 problems.
- Builds: `pnpm tauri build --debug --no-bundle`, `cargo build -p kelpied` (target `D:\kelpie-p9-target`). e2e checks
  `daemon.info.api == 16`.
- `scripts/e2e/processes.py`: two consecutive green runs, **44/44** and **44/44** (runs 7 and 8; earlier runs found the issues
  above). Covered: idle visible session 0 scans in 10 s; chip <= 6 s (0.94 s and 2.98 s); click opens a browser pane on the port;
  *Open preview beside* splits; panel lists 5 servers; Kill asks, frees the port, shell stays, others untouched; Kill children
  frees the agent's port and keeps the agent process and session; refused: wrong start time, start time 1, wrong program, a PID
  outside the tree, the session's own process, the daemon, unknown session; PID reuse (listed program ended, stale identity
  refused; old identity on an unrelated live PID refused; that process survives); 90 probe calls from inside a session (token and
  claimed, 9 shapes x 5 methods) all refused. No test process left running afterwards (checked by command line).
- Regressions (debug pair, isolated endpoints/data dirs): `event_automations.py` 86/86, `spec_panel.py` 124/124,
  `context_cost.py` 26/26, `attention.py <kelpied.exe>` 15/15, `finish.py` 72/72 on the second run (first run: the stand-in
  agent did not report its third turn within 60 s, at the step before any process code is involved; passed unchanged on rerun).
- Bundle gate (`perf_gate.py --metrics bundle --runs 1`): entry **324.99 kB gz** (limit 338.60), 254 lazy chunks (1781.1 kB).

## Perf (step 7)
| Scan, 5 servers, 2 sessions (`process.ports`, daemon `took_us`) | median | p95 |
|---|---|---|
| before (Toolhelp + sysinfo), debug, e2e | 14.74 ms | 15.60 ms |
| before, release, unit probe (one session) | 12.89 ms | – |
| **after, release daemon** (`measure_scan.py`, 100 calls) | **2.58 ms** | 3.29 ms |
| after, debug daemon, no GUI | 3.97 ms | 5.16 ms |
| after, debug daemon, GUI e2e (runs 3, 5, 7, 8) | 5.5–8.1 ms | 6.7–12.3 ms |

`process.list` (one session, 4 servers): release 2.44 ms median. Idle: 0 scans in 10 s with a visible idle session and the panel
closed (e2e), and 0 in 10 s with nothing asking (release and debug daemons).

## Concerns
- The e2e's 5 ms check is now asserted only against a release daemon; on the debug pair the scripts are built with it prints
  the median (5.5–8.1 ms under the GUI). The perf README defines budgets on the release build, where the median is 2.58 ms.
- The tree on Windows depends on the parent walk because a Store pwsh starts programs outside the session's job; such programs
  also escape the job's kill-on-close when a session is closed (pre-existing behaviour, not changed here).
- `keep_set` treats a `cmd` root of an agent session as a batch launcher whose direct children are the agent.
- macOS orphan test is `cfg(target_os = "macos")` and was not run here (Windows machine).
