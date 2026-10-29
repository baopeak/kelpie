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

## Review fixes (code review `code-reviewer-261029-phase13-processes-ports.md`)

### Blocking
- **Session restart (H1)**: `procs.rs` no longer pins a session's root identity. The root is whatever the session reports
  now (the session holds its process until it records the exit: a handle on Windows, an unreaped child on Unix), so after a
  restart under the same session id the new process is the root. The reported root PID is removed from the members (job
  list included), always added to `keep`, and refused by `kill` even when its identity could not be read. What the
  previous run left stays in `seen` (orphans while it runs). New test `a_restarted_session_s_new_process_is_its_own_and_is_never_stopped`
  (kill old root, new root in a new job under the same id: listed as root, never a child, in `keep`, `kill` refused,
  `kill_children` stops only its children). Checked that it fails on the old logic ("the new process is the session's").
- **Linux threads (H2)**: the Unix reader uses `without_tasks()` and skips entries with `thread_kind()` set (table and
  identity reads). New `cfg(target_os = "linux")` test `threads_are_not_listed_as_processes` (TIDs of the test process are
  not in the table and have no identity). Not runnable here (no Linux toolchain/C cross compiler: `ring` needs one for
  `cargo check --target x86_64-unknown-linux-gnu`); the reader code was type-checked and run against sysinfo 0.39.6 in a
  throwaway crate on Windows, then removed. Runs in CI on Linux later.

### Medium (controller decisions)
- **Kill children keeps the agent's own programs (M3)**: `keep_set` keeps the root and, in an agent session, the agent's
  process (the root when named like the agent; under a launcher the child named like the agent or whatever `cmd` starts
  directly; a launcher with neither is taken as the agent) plus the agent's direct children (MCP servers, tool shells).
  Grandchildren (a dev server a tool shell started) are stopped. Per-process Kill stays available for kept ones. Panel
  confirmation and button tooltip say so for agent sessions; user guide and architecture updated.
- **Windows stop (M2)**: no Ctrl+Break. Verified behaviour: `WM_CLOSE` to the process's top-level windows; a console
  program is never asked and is terminated at once (or after the grace when a windowed program in the same stop was asked).
  On Windows the confirmation now says "A program with a window is asked to close it and forced after 3 seconds; a console
  program (a dev server, for one) is ended at once."; other systems keep "asked to end first and forced after 3 seconds".
  User guide and architecture describe both.
- **macOS lsof (M1)**: it was already one `lsof -a -p pid1,pid2,…` per look (not per PID). `process.ports` now asks for
  ports only when some session has children or orphans, so a shell/agent with nothing under it (looked at after output)
  costs no `lsof`. No new scan. Follow-up: libproc (`PROC_PIDLISTFDS` + `PROC_PIDFDSOCKETINFO`) for session PIDs and a
  measurement on macOS. Trade-off: a session whose own process (not a child) listens gets no chip until something runs
  under it (the panel still shows that port).
- **Final-check test (M4)**: `the_final_check_refuses_a_live_process_with_another_identity` calls `os::terminate` directly
  with a live stranger's identity altered (start ±1, other exe, no exe): all 4 refused, nothing closed/forced/gone, stranger
  alive. The macOS orphan test now also waits for the orphan to end.

### Low
- L2 fixed: Unix executable path is read fresh (one `UpdateKind::Always` refresh of that PID) both when listing and before
  each signal, through the same function, so a launcher that `exec`s is listed as what it runs and can be stopped; the
  table refresh no longer reads exe paths at all.
- L3 fixed: names are lowercased before `.exe` is trimmed (test: `Codex.Exe`).
- L4: behaviour kept (output-triggered looks are what make a chip appear within ~1 s); module comment, user guide and
  architecture now say looks follow output at most every 2 s. Their cost is reduced by the M1 change.
- L5 fixed: chips of a session that left the screen are kept but marked `stale` (dimmed, dashed, tooltip "As of when this
  session was last on screen"), fresh again on the next look. Vitest added.
- L6 fixed: `ImageName.Buffer` is read only when it lies whole, 2-byte aligned, inside the filled buffer.
- L7 fixed: the panel uses `Promise.allSettled`; a session that closed meanwhile is not an error, others keep their lists.
- L8 fixed: the panel drops lists of sessions no longer shown; `process.ports` also forgets closed sessions.
- L9 fixed: shared `uniquePorts` (processes.ts) used by the tab/sidebar chips and the panel.
- L1 skipped: the Unix check→signal window needs a PID wrap between two adjacent syscalls; the Linux fix (`pidfd_open` +
  `pidfd_send_signal`) is raw-syscall code that cannot be compiled or tested on this machine, and macOS has no equivalent.
  Start-time granularity (1 s) is left for the same reason. Worth doing with the phase that adds Linux.

### e2e (`scripts/e2e/processes.py`)
The stand-in agent now runs a helper directly (as an MCP server) and a tool shell that starts the server. Step 5 checks
that Kill children frees the port and keeps the agent and its helper; new step 5b kills and restarts the agent session
(`session.kill` + `session.restart`, same id): the new root is listed as root, not a child, is in `keep`, `process.kill`
on it is refused, and Kill children from the panel names only the new server, frees its port and keeps the new root, its
helper and the session.

### Verification (numbers)
- `cargo test -p kelpie-proto -p kelpied -p kelpie` (target `D:\kelpie-p9-target`, `CARGO_INCREMENTAL=0`): kelpied
  **348 passed / 3 ignored**, kelpie-proto 25 / 2 ignored, kelpie 169 / 5 ignored. (The first combined run hit rustc OOM on
  this 8 GB machine compiling `kelpie` lib and lib-test in parallel; rerun with `-j 1` for `kelpie`.)
- `cargo clippy -p kelpied -p kelpie-proto --all-targets -- -D warnings`: only master's `too_many_arguments` at
  `mobile.rs:892`; clean with that lint allowed. (An MSRV 1.82 lint caught `is_multiple_of`; replaced.)
- `pnpm typecheck` clean; `pnpm test` **460 passed** (was 458; +2); `pnpm i18n:check` 0 missing / 0 problems.
- Rebuilt debug `kelpied` and `pnpm tauri build --debug --no-bundle`.
- `processes.py`: two consecutive green runs, **53/53** and **53/53**. Chip 0.96 s / 0.86 s; idle 0 scans in 10 s; debug
  daemon scan with 5 servers median 5.36 / 5.06 ms (release budget, reported only). No test process left afterwards.
- Regressions: `attention.py` **15/15**, `finish.py` **72/72**.
