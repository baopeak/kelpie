# Code review: phase 13, processes and ports

Branch `phase13/processes-ports`, commit 5fcd65b on top of master cdc0294 (34 files, +2854/-38).
Review only. No source files were edited.

## Verdict

**Changes requested.** One High issue is blocking on Windows, the primary platform: after a session restart, "Kill children" (and "Kill") can stop the session's own process. A second High issue affects only Linux and has a one-line fix. Everything else is Medium or Low.

What I checked and found sound:

- **Auth.** The `process.*` methods are allowed for `Role::User` only. `auth.rs:100-107` is an allow-list, and the test covers Session, Claimed and Anonymous roles with 11 parameter shapes × 6 methods. A phone connection is `Role::User`, but it can only reach the closed verb table in `mobile/verbs.rs`, so it cannot call `process.*`.
- **Windows kill guard.** Each kill opens a handle, re-reads (start time, exe) through that handle, and holds it until `TerminateProcess`. The PID cannot be reused in between.
- **Process table parsing.** The `NtQuerySystemInformation` code matches the `SYSTEM_PROCESS_INFORMATION` layout in windows-sys 0.61.2:
  - `Reserved1[48]` holds WorkingSetPrivateSize, HardFaultCount/HighWatermark, CycleTime, CreateTime, UserTime and KernelTime, so indices 3, 4 and 5 are correct.
  - `Reserved2` is InheritedFromUniqueProcessId.
  - The buffer is made of `u64` words, so it is 8-byte aligned. The loop bound `at + size_of <= end` holds, and the grow loop is capped at 4 tries.
- **TCP table offsets.** IPv4: 24-byte row with port at +8 and pid at +20. IPv6: 56-byte row with port at +20 and pid at +52. The `from_be` port conversion is correct.
- **Parent links.** `child_map` rejects a "parent" that started after its child.
- **Threading.** Scans run in `spawn_blocking` (`server.rs:94`) and kills run on their own thread. The only session lock taken is the short `proc_root` lock, so the PTY reader is never blocked.

## Findings

### H1 (High, blocking): after a session restart, the session's own process becomes a "child" that Kill / Kill children will stop

- **Where:** `crates/kelpied/src/procs.rs:212-223`, `:239`, `:159-160`, `:260`.
- **Cause:**
  - `Tracked.root` is set once (`get_or_insert_with`, line 214) and never replaced.
  - `Daemon::restart` (`daemon.rs:987-1010`) keeps the same session id and calls `session.spawn` again. That spawn creates a new root process and a new Job Object, and the new root is assigned to the job (`session.rs:385`, `proc_tree.rs:98`).

**What happens on the next scan, on Windows:**
1. `proc_of(new_root)` succeeds, but the filter `known.same(&r.identity())` rejects it, so `root` is `None`.
2. The job loop (lines 218-222) compares `Some(pid) != root.map(..)`, which is `Some(pid) != None`. So the new root PID is pushed into `member_pids` and listed in `children`.
3. `keep_set(None, ..)` returns an empty set.
4. `kill_children` therefore puts the session's shell or agent into `victims`. `terminate` re-checks its identity, which matches because it is the real current process, and stops it.
5. `process.kill` with the root PID also passes: the "own process" guard at line 260 only looks at `scan.root`, which is `None`.

The panel shows the "Kill children" button, because `kids()` = children not in `keep` = everything. The confirmation lists the shell or agent by name, but it calls it "everything {session} started… The session itself keeps running", which is false here.

- **Scenario:** a user restarts or resumes an exited agent session (Restart / `restartKeepingConversation`), opens Processes and clicks **Kill children** to stop a dev server. The agent is terminated mid-conversation.
- **On Unix:** there is no job, so the same stale identity makes the tree permanently empty after a restart. Chips and the panel show nothing for that session until the daemon restarts.
- **Fix:**
  1. When `t.root` resolves to a live process whose identity differs from `tr.root`, adopt it as the new root. Keep the old `seen` entries so that survivors of the previous run still show as orphans.
  2. Independently, never let `t.root`'s PID into `member_pids`.
  3. Always put `t.root` into `keep`, and refuse it in `kill`, even when the root identity is unverified.
  4. Add a regression test: scan, kill the root, start a new root in a new job under the same session id, scan again. Assert that the root is reported as root and that `kill_children` / `kill(root)` leave it alive.

### H2 (High, Linux only): threads are listed as processes, so "Kill children" signals the root's thread group

- **Where:** `crates/kelpied/src/procs.rs:524-526` and `:543-561` (Unix `Reader`).
- **Cause:**
  - `ProcessRefreshKind::nothing()` leaves `tasks: true` (sysinfo-0.39.6 `src/common/system.rs:2423-2439`).
  - With `ProcessesToUpdate::All`, the Linux backend adds every `/proc/<pid>/task/<tid>` as a `Process` whose parent is the tgid (`src/unix/linux/process.rs:931-961`, `966-985`).
  - The reader does not filter `thread_kind()`, so every thread of the root becomes a "child" of the root in `child_map`.
- **Effect:**
  - `kill_children` sends `kill(tid, SIGTERM)` and then `SIGKILL` to the root's thread IDs. On Linux, `kill(2)` with a non-leader TID signals the whole thread group, so a multi-threaded root (node/claude, codex) is killed.
  - The `children` counts in chips include threads, which keeps the 3 s poll running for every agent session.
  - The panel lists threads.
  - Reading `/proc/*/task/*` for every process is also what makes a Linux scan expensive.
- **Severity:** Linux is not a release target (`release.yml` builds NSIS and macOS only). But the daemon has Linux code paths, and `docs/architecture.md` documents Linux `/proc/net/tcp`. It is blocking if Linux daemon builds are supported; either way I recommend fixing it now.
- **Fix:** use `.without_tasks()` in `refresh_kind()`, and filter `p.thread_kind().is_none()` in `read`.
- **Related, pre-existing and not new here:** the app's `sys_stats.rs` uses the same `nothing().with_memory()`, so on Linux the memory meter counts each thread's (shared) RSS again.

### M1 (Medium): macOS port lookup forks `lsof` on every look

- **Where:** `crates/kelpied/src/ports.rs:199-210`.
- **Problem:** every `process.ports` / `process.list` call spawns `/usr/sbin/lsof`. That happens every 2 s while the panel is open, and every 3 s, or ~0.8 s after output, for visible sessions with children. `lsof -iTCP` typically takes tens to hundreds of ms.
  - The call runs on the client's serialized request worker, so the app's other RPCs wait behind it.
  - The phase's "scan ≤ 5 ms with 5 servers" budget was measured on Windows only.
- **Fix:** measure it on macOS. Preferably, use libproc (`proc_pidinfo(PROC_PIDLISTFDS)` + `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)`) for the session PIDs only, and keep `lsof` for `port_owner`.

### M2 (Medium): Windows "graceful then forced after 3 s" is not graceful for console programs; the docs say it is

- **Where:** `procs.rs:495-513`.
- **Problem:** only programs with top-level windows get `WM_CLOSE`. Console programs (dev servers, `python -m http.server`, node) get `TerminateProcess` immediately, with no chance to clean up. `docs/architecture.md` states this accurately. But `docs/user-guide.md` ("a program is asked to close and forced after 3 seconds") and the confirmation text ("Each is asked to end first and forced after 3 seconds.") promise more than happens. The phase spec asks for "graceful → force after 3 s".
- **Fix options:**
  - (a) Send `CTRL_BREAK`/`CTRL_C` through a small helper that runs `AttachConsole(pid)` + `GenerateConsoleCtrlEvent`.
  - (b) Correct the user guide and dialog text for Windows console programs.
- **Decision needed:** this is a product decision for the user.

### M3 (Medium, product decision): "Kill children (keep agent)" stops the agent's own helpers

- **Where:** `keep_set` (`procs.rs:159-172`).
- **Problem:** when the agent is the root (native `claude.exe`, `/usr/bin/claude`), the only thing kept is the root. Its MCP servers, language servers and tool shells are all children, and they get stopped. The agent survives, but its tools break.
- **Related:** on Windows, GUI applications that a session's shell opened and that inherited the job (for example, a browser first started by `start <url>` or `vite --open`) are job members and are stopped too. Session close already does this through kill-on-close, so this is consistent but still surprising.
- **Mitigation already present:** the confirmation names the processes, up to 8.
- **Decision needed:** whether to keep MCP and other agent-spawned stdio children, or to mark them in the dialog. Flag this to the user rather than change it silently.

### M4 (Medium): missing tests for the behaviours that matter most

- No test changes a root's identity under the same session id; this would have caught H1.
- No direct test of the last line of defense: `os::terminate(&[Identity { pid: <live stranger>, start_time: <wrong>, .. }])` must return `refused` and leave the stranger alive. Every current refusal test is stopped earlier, in `kill()`.
- No Linux coverage, which would have caught H2. Even a unit test that `Reader::read` returns no thread entries for the test process would be enough.
- The macOS orphan test (`procs.rs:925-940`) never checks that the orphan ended. On assertion failure it leaves `sleep 60` behind (harmless).

### Low

- **L1. Unix stop has a check→kill race and 1 s start-time granularity** (`procs.rs:578-607`). The PID can be reused between `check()` and `libc::kill`, and two processes with the same exe started in the same second are indistinguishable. In practice this needs PID wrap within that window. On Linux, `pidfd_open` + `pidfd_send_signal` closes the race; `/proc/<pid>/stat` starttime gives clock-tick precision.
- **L2. Unix exe is cached in the long-lived reader, but read fresh in `terminate`** (`procs.rs:525`, `UpdateKind::OnlyIfNotSet`). A process first seen before it `exec`s (fork-to-exec window, or `#!/bin/sh … exec node` launchers) keeps the stale exe in the panel. Every later stop of it is refused with "the process changed". It fails safe, but the process cannot be stopped.
- **L3. `keep_set` trims `.exe`/`.EXE` before lowercasing** (`procs.rs:163`). A mixed-case `Codex.Exe` is not trimmed, so the agent would not be kept. Lowercase first.
- **L4. Output triggers looks for visible sessions with no children** (`processes.ts:147-150`). A working agent that streams output gets a scan every 2 s. That goes beyond the spec ("visible and had child processes at last check"), and the user guide sentence "looks … only while this tab is open or a visible session has programs running under it" is inaccurate. Cheap on Windows (~3 ms); see M1 for macOS. Update the doc or restrict the trigger.
- **L5. Stale sidebar chips** (`processes.ts:111-129`). Only visible sessions are looked at, so a hidden session's sidebar chip stays after its server stops. Clicking it opens a dead address. Consider dropping chips of sessions that leave the screen, or dimming them.
- **L6. `ImageName.Buffer` is not bounds-checked against the filled buffer** (`procs.rs:417-421`). The code relies on the kernel's layout guarantee. Optional hardening: check that `Buffer` lies within `[base, base+end)` and that `Length` is even.
- **L7. One closed session blanks the panel** (`processes-panel.tsx:91`). `Promise.all` makes one session closed between the memo and the request fail the whole look and show the error banner. Use `allSettled`, or skip "no session".
- **L8. Panel `lists` and daemon `tracked` are not pruned promptly.** The panel store is never pruned. Daemon `tracked` is pruned only on `process.list`, not on `process.ports`. Bounded; nit.
- **L9. Duplicated chip dedupe.** `processes-panel.tsx` `Chips` repeats the dedupe in `PortChips`. DRY nit.

## Focus-area notes

- **Scan scheduling:** the daemon has no timer. `daemon.info.process_scans` makes this observable. `process.ports` skips the scan when no target resolves. The app gates everything on `connected() && daemonApi() >= 16` (`processes.ts:108`, `right-panel.tsx`, `commands.ts`).
- **Port attribution:** dedupe is by (port, pid). Wildcard and loopback map to `localhost`; a server bound to one specific address is reached by that address.
- **Protocol:** additive only. These are new methods plus `daemon.info.process_scans`. No existing serialized struct changed, and `API_LEVEL` is 16 with an explanation.
- **Frontend:** timers are cleaned up (`onCleanup`, `gone` flag). The module-level poller lives as long as the app, by design. i18n check: 0 missing.

## Verification run here

- `cargo test -p kelpied -- procs:: ports:: proc_tree:: processes_ programs_in_a_session` (target `D:\kelpie-p9-target`): **22 passed**.
- `pnpm exec vitest run src/lib/processes.test.ts`: **6 passed**.
- `pnpm i18n:check`: 0 missing, 0 problems.
- Checked against dependency sources: sysinfo 0.39.6 (task listing) and windows-sys 0.61.2 (`SYSTEM_PROCESS_INFORMATION` layout).
- Not run: e2e (real GUI and daemon) and anything on macOS or Linux.

## Unresolved questions

1. Is a Linux `kelpied` in scope (for example, for SSH hosts later)? This decides whether H2 blocks.
2. Should "Kill children" keep the agent's MCP and stdio helpers (M3)?
3. Should Windows implement console Ctrl+Break for the grace period, or should the docs and dialog say console programs are stopped at once (M2)?

---

# Round 2: fixes in 0e327b5 (diff `5fcd65b..0e327b5`)

Review only; no source edited. The controller's decisions (M2 docs-only, M3 keeps the agent's direct children, Linux in scope) are taken as given.

## Verdict

**One blocking item left: R2-M1.** The new "look up ports only when something runs under a session" gate applies on every platform, not only macOS. A session whose own process is the server, for example a custom session running `python -m http.server`, no longer gets a chip. The fix is one line. Everything else from round 1 is fixed correctly, and the safety properties hold: every stop is identity-checked, and the session's root can never be stopped. The remaining new findings are Low.

## Round-1 findings re-checked

| Finding | Status | Evidence |
|---|---|---|
| H1 restart/root | **Fixed** | `procs.rs:229` uses whatever root the session reports now. `:238` removes `t.root` from members, including the job list. `:256` always keeps `t.root`, even when its identity is unreadable. `:277` makes `kill` refuse `t.root` by PID. Regression test `a_restarted_session_s_new_process_is_its_own_and_is_never_stopped` passes; it uses a real new job under the same id. See R2-L1 for the guard that was dropped. |
| H2 Linux threads | **Fixed** | `refresh_kind()` uses `.without_tasks()` (`:546`). `read` and `now` both filter `thread_kind().is_none()` (`:558`, `:584`). |
| L2 stale exe | **Fixed** | The table no longer reads exe paths. `now()` (`:555-560`) reads with `UpdateKind::Always` both when listing and before each signal. |
| L3 case | **Fixed** | `:166-169` lowercases before stripping `.exe`, with a test for `Codex.Exe`. |
| L5 chips | **Fixed** | Off-screen chips are marked `stale`, dimmed and dashed, with a tooltip. They become fresh on the next look and are deleted when the session dies (`processes.ts:126-129`). There is a Vitest for it. |
| L6 buffer bounds | **Fixed** | `:435-440`: `wrapping_sub` rejects a buffer below base. The parity check is valid because base is 8-aligned. `name_at > end \|\| name_len > end - name_at` cannot overflow. |
| M2 / M3 / M4 / L7 / L8 / L9 | Fixed as decided | The Windows dialog text is OS-specific. `keep_set` keeps the agent's direct children. There is a direct `os::terminate` refusal test. The panel uses `allSettled` plus pruning. `forget_closed_sessions` is now also called on `process.ports`. `uniquePorts` is shared. |
| M1 macOS lsof | Partly addressed | It was already one `lsof` per look. The new gate causes R2-M1. |
| L1 | Skipped, accepted | — |

## Safety questions asked for this round

**Can anything untrusted choose the root PID?** No.
- `Target.root` comes only from the daemon's own spawn bookkeeping: `Session::proc_root` → `pty.pid` from `child.process_id()` (`session.rs:383,404,816-820`), and `ChatSession::proc_root` → `g.pid` (`chat.rs:371,636-642`).
- A session program cannot reach `process.*` (the auth allow-list and its test still pass), and it cannot change these fields.
- The agent name comes from the user's own preset or custom agent config.
- The only remaining way to get a wrong root is a stale snapshot; see R2-L1.

**Is every stop still identity-checked? Can a stop reach a non-descendant?** Yes, every stop is checked. A non-descendant can be reached only through R2-L1.
- `kill` refuses `t.root` and the scanned root (`:277`).
- It requires the PID to be one of this scan's children or orphans, compares `start_time`, and compares `exe` when one is given (`:280-287`).
- Victims below it come only from this scan's `children`.
- `kill_children` stops only `children` that are not in `keep`.
- `os::terminate` re-checks every victim:
  - On Windows, through the handle it holds until `TerminateProcess` (`:494-512`).
  - On Unix, by a fresh read before `SIGTERM` and again before `SIGKILL` (`:603-635`).
- Job members are kernel-assigned, so on Windows they are descendants. The `child_map` start-time rule still rejects reused parent PIDs.

**Is the "launcher with nothing agent-like under it is the agent" rule sound?** It is sound for safety:
- The rule only adds to `keep`, so it can only cause something to be kept that should have been stopped. It can never cause something extra to be stopped.
- The root is never a victim either way.
- In the configurations that actually ship, it is what makes M3 work:
  - Windows npm shims are unwrapped to `node cli.js` (`agents.rs:552-555`), so for npm-installed agents the root is `node`.
  - On Unix, `$SHELL -l -c "exec claude"` (`agents.rs:527-531`) leaves `node` as the root.

  In both cases no child is named like the agent, so the root is correctly taken as the agent.
- Two kinds of mistake are possible, both Low:
  - A wrong keep: a launcher's direct child that is not an agent helper is kept and can still be stopped with Kill. No shipped agent session keeps a shell around after the agent exits (Windows runs the agent as the root; Unix `exec`s it), so I found no realistic case.
  - A wrong kill when the real agent sits one level deeper; see R2-L3.

## New findings

### R2-M1 (Medium, **blocking**): the port-lookup gate removed chips for sessions whose own process is the server, on every platform

- **Where:** `crates/kelpied/src/daemon/process.rs:106`. The condition `scans.iter().all(|s| s.children.is_empty() && s.orphans.is_empty())` has no `cfg`. At 5fcd65b it also checked `s.root.is_none()`, so the root's ports were looked up.
- **Scenario (Windows):**
  1. Create a custom session (New session → Custom, `python -m http.server 8000`; worktree tab commands start the same way, `worktrees.ts:93`). `resolve` runs `python.exe` as the PTY root (`agents.rs:559`).
  2. It has no children, so `listeners` is empty and `session_ports` returns nothing. No chip appears.
  3. On Unix, `wrap_for_platform` `exec`s, so the root is again the server and the result is the same.
  4. Many single-process servers are affected: `python -m http.server`, `uvicorn`, `deno run`, `php -S`, Go or Rust binaries.
- **Inconsistency:** because `process.ports` batches every visible session, the same root-only server *does* get a chip whenever some other visible session happens to have children.
- **Spec and docs:** "Port chips `:5173` on tab/sidebar → browser pane" with success criterion "Ports visible". The spec's own example server is `python -m http.server`. The e2e still passes only because it types the command into a shell, which makes the server a child. `docs/user-guide.md` still says "When a program in a session listens on a port (… `python -m http.server 8000`…), a `:8000` chip appears", which is now false for custom sessions.
- **Why blocking:** the gate saves nothing on Windows, the primary platform (`GetExtendedTcpTable` is about 1 ms). It regresses the spec's main preview path there, and the fix is trivial.
- **Fix (either):**
  - (a) Apply the gate only on macOS: `cfg!(target_os = "macos") && …`.
  - (b) Also look up ports when a non-agent, non-shell (custom) session has a root, because a custom root is often the server itself. This needs the kind on `Target`; `agent` alone cannot tell shell from custom.

  Either way, add a daemon test: a target whose root itself listens appears in `process.ports`.

### R2-L1 (Low): the root's identity is no longer checked, so a stale `Target` with a reused PID would expose a stranger's descendants to Kill children

- **Where:** `procs.rs:229` uses `proc_of(&table, pid)` with no comparison against the process the session started. `process.rs:128-138` snapshots the `Target` before the kill thread scans.
- **Comment accuracy:** the comment at `:225-228` says "the session holds its process until it records the exit". That is true on Windows: the `Child` and the killer's duplicated handle live until after `g.pty = None` (`session.rs:449-453`, `:467`). On Unix it is not true: `child.wait()` reaps before `on_exit` takes the lock (`session.rs:450`, `chat.rs:411-414`).
- **Scenario:**
  1. The `Target` is built while the session is alive.
  2. The session's process exits, is reaped, and its PID is reused by an unrelated process before the scan reads the table.
  3. `root` is now the stranger, and `member_pids` are the stranger's descendants that started after it.
  4. `kill_children` stops them. The identity checks pass because those identities come from the same scan.

  `process.kill` is not exposed, because the request identity must match a process that was listed earlier.
- **Likelihood:** exit, PID reuse and new descendants must all happen within the milliseconds between snapshot and table read. On Windows the snapshot must also outlive the ≥50 ms handle hold. That is practically nil, which is why this is Low.
- **Fix:** record the root's start time at spawn (in `Pty` or the chat `Inner`), carry it in `Target`, and require `root.start_time == target.root_start` in `scan`. If it differs, treat `root` as `None`, but still keep `t.root` in `keep`. This restores the invariant the old `tr.root` pin gave, without the H1 bug. Also correct the comment.

### R2-L2 (Low, Linux only): after a wall-clock step, every process fails `proc_of` until the daemon restarts

- **Cause:** On Linux, sysinfo's `start_time` is boot-relative ticks plus `SystemInfo.boot_time`, where `boot_time` is `/proc/stat` `btime`. sysinfo reads it once per `System` (sysinfo `unix/linux/system.rs:116-158`; `process.rs:581-587`).
  - The long-lived `Reader` keeps the `btime` from daemon start.
  - `now()` builds a fresh `System` per call (`procs.rs:562-563`) and gets the current `btime`.
  - `btime` moves when the wall clock is stepped: an NTP step after start, or WSL2 or a VM after resume.
  - After that, `proc_of` (`:141-145`) compares `table.start_time` with the fresh `start_time` and they never match. Every Linux tree and chip goes empty. This fails safe; nothing is killed.
- **Regression:** before 0e327b5, Unix `identity()` came from the table, so listing kept working.
- **Fix:** use one clock source for both reads. Either reuse the `Reader`'s `System` in `now()`, or key Linux identity on `/proc/<pid>/stat` starttime ticks, which also removes L1's 1 s granularity. This can go with the Linux phase.
- **macOS is unaffected:** `pbi_start_tvsec` is absolute (`apple/macos/process.rs:421`).

### R2-L3 (Low): the agent's helpers are still stopped when the real agent is one level below the process taken as the agent

- **Where:** `keep_set` (`procs.rs:173-184`).
- **Examples:**
  - Gemini CLI relaunches itself as a child `node` (outer `node` → inner `node`).
  - A Volta shim named `claude` is the root and spawns `node cli.js`.
- **What happens:** the outer process is taken as the agent. Its direct child, the real agent, is kept, but the real agent's MCP servers and tool shells are grandchildren of the root, so they are stopped. This happens despite the dialog promising "the programs the agent runs directly (such as MCP servers) keep running" (`processes-panel.tsx:163-165`). The agent itself always survives.
- **Optional fix:** when the chosen agent has exactly one direct child with the same name or exe (`node` → `node`), descend to that child. Otherwise, soften the dialog wording.

### R2-L4 (Low, Unix performance): one `System::new()` per PID per scan

- **Where:** `procs.rs:562-563`. `identity()` is called for the root, every member and every remembered PID on each scan: every 2 s while the panel is open, and on chip polls.
- **macOS cost:** `System::new()` calls `mach_host_self()` and `host_processor_info` (`SystemTimeInfo::new`) each time (sysinfo `apple/system.rs:133-152`, `apple/macos/system.rs:69-96`). sysinfo never deallocates the host-port send right it takes.
- **Fix:** keep one `System` (for example in `Reader`, or one per scan) and refresh only `Some(&[pid])` with exe `Always`. This also fixes R2-L2.

## Linux test: compile check by inspection against sysinfo 0.39.6

- **APIs exist:**
  - `ProcessRefreshKind::without_tasks` and `with_exe` (`common/system.rs:2532-2533`, `impl_get_set!`).
  - `UpdateKind::Always` (`:2324`).
  - `Process::thread_kind() -> Option<ThreadKind>` (`:2144`; it returns `None` off Linux, so the Windows and macOS filters are no-ops).
- **The assertions will hold on Linux:**
  - `get_proc_and_tasks` checks `Tgid` from `/proc/<pid>/status` in every mode, including `ProcessesToUpdate::Some` (`unix/linux/process.rs:940-953`), and marks a TID `ThreadKind::Userland` (`:594-595`, `:650-651`).
  - So `now(tid)` is filtered out, and `os::identity(&table, tids[0]).is_none()` holds.
  - `without_tasks` skips `get_proc_tasks`, so `read(None)` does not contain TIDs.
- **The test code compiles by inspection:** `os::Reader::default()`, `read(None)` and `os::identity(&Table, u32)` are all `pub` in the `cfg(unix)` `os` module, and `std::sync::mpsc` is used correctly.

## Verification run (this round)

- `cargo test -p kelpied -j 2 -- procs:: ports:: proc_tree:: processes_ programs_in_a_session` (target `D:\kelpie-p9-target`, `CARGO_INCREMENTAL=0`): **24 passed**, including the two new tests and the auth test.
- `pnpm exec vitest run src/lib/processes.test.ts`: **8 passed**.
- `pnpm i18n:check`: 0 missing, 0 problems.
- Not run: e2e, macOS, Linux.

## Round 2 unresolved questions

1. R2-M1: should the gate be macOS-only (simplest), or should custom sessions' roots always be included?

---

# Round 3: fixes in b44d807 (diff `0e327b5..b44d807`)

Review only; no source edited.

## Verdict

**Approve. Nothing is blocking.** R2-M1 is fixed. R2-L1, R2-L2 and R2-L4 are fixed without opening a new hole. The reason for deferring R2-L3 is sound. The only new observations are informational.

## 1. R2-M1, chips for a session that is itself the server: **fixed**

- **Gate:** `daemon/process.rs:63-65`, `wants_ports`, is called with `cfg!(target_os = "macos")` (`:115`).
  - On Windows and Linux, ports are looked up whenever a scanned session has a root, children or orphans.
  - On macOS, the root of a custom session also counts. `custom = agent.is_none() && kind != "shell"` (`:76-78`).
- **Ordering:** the `zip(targets, scans)` is sound because `Procs::scan` maps `targets` in order, one scan per target.
- **Chat sessions:** their kind is always an agent kind, so `custom` is false. A kind whose custom agent preset was deleted counts as custom, which only means one extra lookup.
- **Tests:** the unit test covers both platform settings, and a daemon test uses a custom session whose own process listens. Both pass here. e2e step 5c covers the user path (not run by me).
- **Docs:** the architecture text matches. The user-guide promise of a chip for `python -m http.server` is true again.

## 2. R2-L1, root start time recorded at spawn: **fixed, no new hole**

- **Where it is recorded:**
  - Terminal sessions: `session.rs:386-387` reads `start_time_of(pid)` right after `spawn_command`, before the waiter thread exists. The child is unreaped, so the PID cannot have been reused.
  - Chat sessions: `chat.rs:365` does the same, right after `cmd.spawn()`. This is the only place `g.pid` is assigned (`:376`), and `pid_start` is overwritten on every spawn. After exit, `proc_root` returns `None` because `alive` is false, so a stale `pid_start` is never used.
  - `Session::proc_root` and `ChatSession::proc_root` both return `procs::Root { pid, start, tree }`. Fakes record a start (`fake_process`) or have no PID.
- **No false mismatch** (a mismatch would hide a live root):
  - Windows: the recorded start comes from `GetProcessTimes` → `epoch_ms`. The scan's root start comes from the table (`NtQuerySystemInformation` CreateTime → `epoch_ms`), and `proc_of` already requires the table value to equal the handle read (`procs.rs:141-145`). Both are the same kernel value through the same conversion, so they are equal.
  - Linux: every reading goes through the one shared `System` with one boot time, so they are equal even after a clock step.
  - macOS: `pbi_start_tvsec` is absolute, so they are equal.
  - If the start could not be read at spawn (`None`), `is_none_or` accepts the root, which is the round-2 behaviour, so this fails open only to the previous state.
  - On Unix, `std::process::Command::spawn` returns after `exec`, and the start time (fork time) does not change across `exec`.
- **If the root were hidden anyway (real reuse):**
  - `root` is `None`, so nothing is walked from the stranger.
  - `t.root` is still removed from members (`:251`), still put in `keep`, and still refused by `kill` (`:290`).
  - Job members still come from the session's own job, which the `Target` holds, so they are the session's leftovers.
  - The new test `a_process_under_the_session_s_pid_with_another_start_is_not_its_root` checks all of this and passes.
- **Windows unchanged otherwise:** the only change on Windows is the extra start-time filter, and the restart test still passes.

## 3. R2-L2 / R2-L4, one shared `System` on Unix: **correct**

- **Lock scope:** `SYS` (`procs.rs:579`) is a leaf lock.
  - It is taken only inside `os::now` (one PID refresh) and `Reader::read` (one refresh plus building the table), and released when they return.
  - Neither function takes any other lock while holding it.
  - `terminate` only takes it inside `check()`. The 50 ms and 25 ms sleeps, the 3 s grace and `libc::kill` all run outside the lock.
- **No deadlock:** the lock order is `Procs.reader` → `Procs.tracked` → `SYS` (transient). `describe` takes `reader`, then `SYS`, then `tracked`, but never holds `SYS` while taking `tracked`, so there is no cycle.
- **Spawn path:** `start_time_of` runs on the caller of `Session::spawn` / `ChatSession::spawn`, which is the RPC request worker or the automation thread. It runs before `inner.lock()` and outside `daemon.sessions` (`daemon.rs:964,975,1010`).
- **PTY threads:** the reader, writer and waiter threads (`session.rs:413-457`) never call `procs`.
- **Worst case:** a spawn waits for an in-flight `ProcessesToUpdate::All` refresh, tens of ms on a busy macOS or Linux machine. That is acceptable.
- **Checked against sysinfo 0.39.6:**
  - Refreshing `ProcessesToUpdate::Some(&[pid])` with `remove_dead = true` only removes that PID if it is dead (`common/system.rs:330-333`, `385-393`), so the shared cache that `read(None)` uses for CPU deltas is kept.
  - On Linux, a cached entry is re-read from `/proc/<pid>/stat`. If the start differs (PID reused), sysinfo rebuilds the whole entry (`unix/linux/process.rs:642-680`), so identity checks remain correct.
  - With `with_exe(UpdateKind::Always)`, `exe` is re-read for a known PID (`:496-518`).
  - Thread entries that `now(tid)` might insert are filtered by `thread_kind`, and the next `All` refresh drops them because they are not updated.
- **Compiles by inspection:**
  - `LazyLock` is stable since Rust 1.80, below the MSRV of 1.82.
  - `System` is `Send`, so the static `Mutex<System>` is `Sync`.
  - The iterator chain in `read`'s tail expression has no `Drop` types, so it does not hit the guard-lifetime error.
  - Callers of the changed `now`/`check` signatures are all updated, and the test module's `os::Reader::default()` still works with the unit struct.
  - The developer also compiled the module verbatim against sysinfo on Windows, and CI will compile it on macOS.
- **Info, not a finding:**
  - `now()` refreshes without CPU, yet on Linux it still shifts a process's `old_utime` (`set_time`, `process.rs:799`, `363-365`). The next scan's CPU % for listed processes then covers a window shortened by the milliseconds between a table read and the identity reads. That error is negligible.
  - sysinfo is built without `multithread` (`crates/kelpied/Cargo.toml:43`), so no rayon pool runs under the lock.

## 4. R2-L3, deferred: **the reason is sound**

For an npm agent (root `node`), MCP servers are often `node` and tool shells are `bash`. A rule like "descend into a sole same-named child" would pick a lone `node` MCP server or dev server as "the agent". It would then keep what that server starts and stop the real agent's tool shells. That is worse than today's behaviour, where the agent always survives and only its helpers may be stopped. Recognising these launchers by command line in a later phase is the right follow-up.

## Verification (round 3)

- `cargo test -p kelpied -j 2 -- procs:: ports:: proc_tree:: processes_ programs_in_a_session a_session_whose_own_process daemon::process::` at b44d807 (target `D:\kelpie-p9-target`, `CARGO_INCREMENTAL=0`): **27 passed**.
- Not run: e2e, macOS, Linux. The Unix-only code was reviewed by reading it; CI compiles it for macOS.

Blocking items: **none**.
