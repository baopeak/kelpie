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
