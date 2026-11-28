# Code review: resource monitor phase 1 (commit b4a4c065, feat/resource-monitor)

Scope: crates/kelpied (resources.rs, resources/{windows,unix,ledger,tests}.rs, daemon/resource_rpc.rs, procs.rs, proc_tree.rs, sub_rpc.rs, auth.rs, daemon.rs, main.rs), kelpie-proto API 51, app/src/lib/{resources,processes}.ts, docs/architecture.md, scripts/e2e/resources_monitor.py. Roughly 2.2k LOC added. plans/ ignored.

Checks run: `cargo test -p kelpied -j 2 resources` (isolated target dir, debug off): 12 passed, 0 failed. Its one warning is in screen/tests.rs, which this commit does not touch. I also timed `Get-Counter '\GPU Engine(*)\Utilization Percentage'` on this machine: 186 instances.

## Verified as correct (no action)
- **FFI layout.** In windows-sys 0.61.2, `SYSTEM_PROCESS_INFORMATION` ends `PrivatePageCount: usize, Reserved7: [i64; 6]`. That matches NT's Read/Write/Other OperationCount followed by Read/Write/Other TransferCount, so `Reserved7[3] + Reserved7[4]` is read plus written bytes. `PrivatePageCount` is in bytes (it equals PagefileUsage), so it is commit, as the comment says.
- **PDH.**
  - The buffer is sized by a first call with a null buffer, which returns `PDH_MORE_DATA`.
  - The buffer is a `Vec<u64>`, so it is 8-aligned. `PDH_FMT_COUNTERVALUE_ITEM_W` is pointer + u32 + f64 union, so its alignment is 8.
  - No collect happens between the two calls, so the size stays valid.
  - Counters are closed with the query (`PdhCloseQuery`), the low-memory handle is closed in `Drop`, and the Probe never leaves the sampler thread.
- **Locks.**
  - `Resources.inner`, `ledger` and `wake` are never held together. `command_ended` and `turn_ended` drop the `inner` guard at the end of the `let` before they take `ledger`. `command_started` drops `inner` before it touches `wake`.
  - `wait_timeout_while` checks the flag first, so a wake cannot be lost.
  - The sampler upgrades its Weak per loop and drops the Arc before waiting.
  - In `hook()`, `spec.lock()` is released before `resource_hook`. `machine_load()` takes only `inner`, briefly.
- **Roles.**
  - `resources.get` and `resources.ledger` are in neither OWN nor SCRIPTING. Sessions and claimed callers are refused, and User/App are allowed.
  - `event_visibility` falls to `Hidden` for `resources.changed` on Session, Claimed and Anonymous roles.
  - `daemon.info.resource_samples` is a counter only.
  - `resource_hook` runs after `accepts_report`, and `session_id` is rewritten by auth, so a session can feed only its own track and its own project's ledger.
- **Math.**
  - EWMA variance `(1-a)(var + a d^2)` is correct.
  - Linux diskstats busy% (`io_ms delta / (secs*10)`) is correct, and so is sectors*512.
  - GetSystemTimes: kernel time includes idle, and the code handles that.
  - Commit on Windows is `ullTotalPageFile`/`ullAvailPageFile`, which are the commit limit and what is left of it.
- **E2E isolation.** The e2e script sets its own `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR` in-process.

## High

### H1. A sampler wake triggers an immediate sample, so an agent's Bash bursts break the rates and the hysteresis
Where: `resources.rs:394-415` (`command_started` + `wait`), `resource_rpc.rs:92`, `resources.rs:332`.

Every Claude Bash call that fingerprints sets the wake flag. That includes `ls`, `git status` and `cat`: only cd/echo/export-like segments are skipped. The sampler then samples at once, however recently the last sample was.

Failure scenario: an agent fires several Bash calls within a second, or several parallel ones. Samples are then taken tens of ms apart, and four things break:
- `secs` approaches 0 and is clamped to 0.001. `io_mbps = delta / secs` inflates by up to 1000x, and that feeds `peak_io_mbps` in the ledger and the disk users list.
- Windows CPU times are tick-quantized (15.6 ms). Per-process `cpu` over a 50 ms window swings by about ±30% of a core, which feeds `peak_cores`.
- GetSystemTimes and PDH rate counters over a tiny window are noise.
- `ENTER_AFTER = 2` "samples in a row" then means about 100 ms rather than 4 s, so one spike raises a gauge. That includes `ram` → `caps_level` L3, which blocks sub-session spawns for at least 10 s.

There is also no rate limit: with a steady stream of command starts, the sampler loops back-to-back (PDH collect plus a full process table per loop).

Fix: a wake should only switch an idle pause to the working pace. For example, in the loop compute `next = last + BUSY_EVERY` and wait until then (or until `pause`), never less than about 1 s after the previous sample. Also consider counting `ENTER_AFTER` by elapsed time, not by sample count.

### H2. CPU for a newly seen process is unbounded and tick-quantized
Where: `procs.rs:514-517`.

`cpu_time / alive_ms` for a process that is a few ms old divides a 15.6 ms CPU tick by, say, 3 ms, giving ~500% of a core. Cargo, pnpm and test runners spawn many short processes between 2 s samples.

Where this lands:
- Session `cores`.
- `Demand.peak_cores`, which is a max, so one outlier sticks in the EWMA.
- The cpu `users` list.
- The existing `process.list` panel, since it shares `os::Reader`. That is the behaviour change the task asked about. It is better than the old 0 for processes that have lived a while, but shows bogus spikes for very young ones.

Fix:
- Use the lifetime average only when `alive_ms >= ~500`, otherwise report 0.
- Clamp to `cores * 100`.
- Optionally also require `start_time >= previous reading's wall ms`. On Windows every process is read every time, so this is equivalent today, but it makes the intent explicit.

### H3. Commands that never get a PostToolUse are learned as successful runs, charged with later work
Where: `resource_rpc.rs:126` (`Stop | StopFailure | SessionEnd` → `turn_ended`), `resources.rs:431-437`, `resource_rpc.rs:120-125`.

Three Claude cases fire PreToolUse with no matching PostToolUse/PostToolUseFailure:
1. A permission prompt the user denies.
2. A PreToolUse hook (another one) that blocks the tool.
3. A user interrupt (Esc). Claude also does not fire `Stop` on an interrupt.

In cases 1 and 2 the open command is charged every sample until `Stop`, then learned with `failed=false`. Its `ram_mb` ≈ 0 and its `secs` covers the rest of the turn, so the estimate is pulled low. That contradicts the module doc's "errs high, never low".

In case 3 it stays open into the next prompt. It is charged the next turn's commands, and is learned at that turn's Stop with inflated ram, cores and secs.

Related: PreToolUse fires before the permission prompt, so time spent waiting for approval dilutes `cores` (a mean over samples) and inflates `secs`.

Fix:
- On `UserPromptSubmit` (already installed in `agents.rs:572-584`), drop open commands without learning them.
- On `Stop`, drop rather than learn commands still open: a normal Bash always gets its Post before Stop.
- Optionally restart `began`, `samples` and `base_mb` on the matching `PermissionRequest` resolution.

## Medium

### M1. The ledger can still be overwritten after an unreadable load
Where: `ledger.rs:303-309`, `ledger.rs:334-341`.

`Ledger::open` maps every `Err` to an empty ledger.
- For `LoadError::Damaged` a copy is kept (`keep_damaged`), so this is acceptable.
- For `LoadError::Io` (a scanner or AV holding the file past `durable::load`'s retries) the next learned command calls `save_state`. `write_with_prev` cannot read the current file, so it leaves `.prev` alone and replaces the good-but-locked `command-ledger.json` with a nearly empty one.

AGENTS.md says: "A failed state read is not permission to overwrite the state with defaults." The ledger test covers only the restored-from-`.prev` path.

Fix: on `Err(LoadError::Io)`, set a `blocked` flag and refuse `save`, or retry the load on the first save and merge. Mirror `mobile/store.rs:484` (`broken`).

Related hardening:
- Add `#[serde(default)]` on `Entry`, `Avg` and `File`. Today any added field makes an older file fail to parse, which routes it to the Damaged path, and the file is then overwritten after a copy is kept.
- A non-finite f64 in an `Entry` serializes as `null` and makes the whole file unparseable. Guard `learn` with `is_finite`.

### M2. Learned commands are lost on daemon shutdown and update hand-over
Where: `resource_rpc.rs:85-88`, `main.rs:276-279`.

The ledger is saved only every 30 s from the sampler loop. `daemon.persist(true)` at shutdown does not call `resources.save()`, and the thread's `return` path does not save either. The doc comment at `resource_rpc.rs:17` ("and when the monitor stops") is false.

Daemon hand-overs on update are routine, so up to 30 s of learned runs are dropped each time.

Fix: call `self.resources.save()` from `persist(true)`, or right after `server::serve` returns in `main.rs`.

### M3. Fingerprints merge different scripts and split by path
Where: `ledger.rs:129-148`.

Merges (one entry for different work):
- `npm run build`, `pnpm run test` and `yarn run lint` all fingerprint as `npm run` / `pnpm run`, because the subcommand is `run` and the script name is dropped.
- Interpreters with a script collapse into a single entry: `python scripts/e2e/x.py` → `python`, `node build.js` → `node` (the test asserts this), and `bash x.sh` → `bash`. In this repo the heaviest commands, the e2e and perf scripts, merge with trivial ones.
- `timeout 600 cargo test` → `timeout`. `timeout` is not in PREFIXES and needs its numeric argument skipped.

Splits (paths in the key):
- Flags with values before the subcommand put a path into the key: `git -C D:/x status` → `git d:/x`, `cargo --manifest-path a/Cargo.toml build` → `cargo a/cargo.toml`, `pnpm --filter app build` → `pnpm app`, `make -C dir`.
- Wrappers with flags do the same: `nice -n 10 make`, `sudo -u x`, `env -i`. The flag becomes the program.

Minor: `-n` is taken as parallelism for every program (`git log -n 5` → jobs 5).

Fix:
- For `run` (npm/pnpm/yarn/bun) and `run-script`, keep the next word.
- For known interpreters (python, node, bash, sh, pwsh/powershell, ruby, deno), keep the script's file name.
- Skip known value-taking global flags before the subcommand (`-C`, `--manifest-path`, `--filter`, `--prefix`, `--dir`).
- Treat `timeout <n>` as a prefix.
- Limit `-n` to pytest/xdist-like programs.

### M4. Background Bash is learned as a near-zero run
Where: `resource_rpc.rs:120-125`.

With `tool_input.run_in_background: true`, Claude fires PostToolUse as soon as the shell starts. A long build or dev server is learned as ~0 s with almost no samples, which again errs low.

Fix: skip `command_started` when `/tool_input/run_in_background` is true. Background commands can be tracked later via the job tree if wanted.

### M5. A stale or empty `base_mb` overcharges a command
Where: `resources.rs:401`.

`base_mb` is the last ring sample. At idle pace that can be up to 10 s old. Before the session's first sample it is 0, and then the whole tree, including the agent's own 300-500 MiB, is charged to the command.

Fix: take the base from the first sample after the start (mark `base_mb: None` and fill it then), or use `min(last, first-after)`. With H1's fix, the first post-wake sample comes at the working pace.

### M6. One missed PDH reading bypasses the hysteresis
Where: `resources.rs:195-205`.

`feed(None)` resets the gauge, and the next known value "stands" at once. One failed `PdhCollectQueryData`, an empty GPU instance list, or a transient missing disk array therefore makes `disk`/`gpu` flip to null and then straight to any state, skipping `ENTER_AFTER`.

Fix: keep the previous state through a short run of unknowns (for example, unknown only after N consecutive `None`s). Or apply the rise rule after an unknown when the previous state is known.

### M7. Sampling cost: always-on GPU Engine counters and per-command wakes
Where: `windows.rs:35-44`, `resource_rpc.rs:92`.

The `\GPU Engine(*)` and `\GPU Process Memory(*)` wildcard counters are the costliest PDH sets. They have one instance per process × engine (186 here, thousands on a browser-heavy machine) and are re-enumerated on every collect, every 2 s while any session works, which is most of the day.

The commit's real check covers only 13 s.

Recommendation:
- Measure the daemon's CPU time and private bytes over about an hour, with a GPU-using browser open and builds spawning processes (instance churn).
- Consider collecting GPU counters at the idle pace only, or only when an adapter showed use.

(Also see H1 for the unbounded wake rate.)

## Low
- **L1. Volume budget is spent per folder, not per volume** (`resource_rpc.rs:45-46`, `MAX_VOLUMES` = 8). The set holds folders in BTreeSet order, so eight session folders on C: crowd out a nearly full D:, and the `space` gauge misses it. Dedupe by volume root (`GetVolumePathNameW` / `st_dev`).
- **L2. `GetDiskFreeSpaceExW` can stall on a network share.** On a UNC or `\\wsl$` cwd it can block for seconds. That is only the sampler thread, but it stalls every gauge.
- **L3. io first-seen after a failed table read.** The rule counts a process's whole lifetime I/O when it is first seen (`resources.rs:338`). After a failed `NtQuerySystemInformation` (empty table), every process is "new" on the next sample: a multi-GB spike in the disk users list and the ledger's io. Skip the delta when the previous table was empty.
- **L4. A moved doc comment.** `procs.rs:325`: inserting `TreeUsage` separated `/// When \`pid\` started…` from `start_time_of`, so rustdoc now attaches it to `TreeUsage`.
- **L5. Chat-mode Claude sessions are never measured.** `hook()` handles `Entry::Term` only, so chat-mode commands are never measured. Either document this as a phase-1 limitation or route chat hooks.
- **L6. Hook-driven entries can evict the ledger.** `resources.changed` reaches only user/app (verified). However, a session's own hook stream is trusted for ledger size: 500 distinct fingerprints from one session evict every other project's entries (`ledger.rs:325-329`). This is local trust, so it is low. A per-project cap would contain it.
- **L7. Linux disk usage on the panel's scans.** `with_disk_usage()` was added to the shared `refresh_kind` (`procs.rs:619`), so `process.*` scans on Linux now also read `/proc/<pid>/io` for every process. It is cheap but is a behaviour and cost change outside the sampler.

## Recommended order
1. H1 (a floor on the interval between samples).
2. H2 (clamp young-process CPU).
3. H3 (drop rather than learn unended commands; handle UserPromptSubmit).
4. M1 and M2 (ledger safety and the shutdown save).
5. M3 and M4 (fingerprint and background commands).
6. M5 and M6.
7. Measure M7 before turning this on by default for everyone.

## Unresolved questions
- Is `PostToolUseFailure` emitted for permission denials in the Claude version targeted? If it is, H3 case 1 shrinks to the hook-block and interrupt cases.
- Should `caps_level` L3 (it blocks sub-sessions) use a stricter signal than `free < 8%` on 8 GB machines, where ~650 MB available is common? It depends on product intent.

Status: DONE_WITH_CONCERNS
Summary: The FFI layout, PDH buffer handling, lock ordering and role gating are correct, and the 12 resources tests pass. The remaining risk is that an agent's Bash bursts can make the sampler sample back-to-back, which corrupts the rates, the gauges and the sub-session caps, and that bad young-process CPU readings and commands that never ended feed wrong figures into the ledger.
