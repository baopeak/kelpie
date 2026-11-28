# Phase 1: Resource monitor (CPU, RAM, disk, GPU)

Status: **implemented** on branch `feat/resource-monitor` (API 51). Reviewed, and every finding fixed.

## As built (differences from the requirements below)

- **`Load` stays phone-only.** A separate always-on sampler (`resources.rs`, thread `kelpie-resources`) does the work instead. Starting
  `Load` always would have run its 10 Hz lag probe and its throttling opt-out on every machine. The sub-session caps read the larger of
  the two levels (`Daemon::machine_load`).
- **Rollback switch:** `KELPIE_RESOURCE_MONITOR=0`, not a setting.
- **Unknown readings:** disks and GPUs on macOS, and GPUs on Linux, read as unknown (IOKit and NVML are not wired). Windows reads all
  four resources.
- **How a command is charged:** a command's demand is what the processes started after it began take. A baseline subtraction would let
  the agent's own memory or an earlier dev server count.
- **Commands without an end report** (denied, blocked, Esc) are dropped at the turn's boundary, unlearned. `run_in_background` commands
  are not measured. Only Claude terminal sessions report commands; Codex, Gemini, Devin and chat (ACP) come with the gate (phase 4).
- **Measured on the 8 GB machine** (`scripts/e2e/resources_monitor.py`, 18/18): a 600 MiB / two-process / 256 MiB-write program was
  learned at 634 MiB, 1.44 peak cores (its workers reported 1.28 on a busy machine), 260 MiB of I/O and 13 s.
- **Cost:** a debug (unoptimized) build sampling every 2 s used 0.92 % of one core and 14 MiB, against 0.17 % and 9 MiB with the
  monitor off. That is about +0.75 % of one core and +5 MiB, an upper bound for the optimized build. The release profile
  (LTO, one codegen unit) took over 80 minutes to build here, so it was not measured.
- **Review:** [code-reviewer report](../reports/code-reviewer-261128-1142-resource-monitor-phase1.md), and every finding was fixed:
  - a minimum 1 s between samples;
  - bounded CPU for new processes;
  - unfinished commands dropped;
  - an unreadable ledger never overwritten;
  - the ledger saved on the way out;
  - finer fingerprints;
  - background commands skipped;
  - gauges holding through two missed readings;
  - volumes read per drive, with network drives skipped;
  - per-project ledger cap.

## Context

`load.rs` computes L0–L3 from event-loop lag, CPU, free memory, the Windows low-memory notification and power saver. It starts only with phone access (`mobile.rs:2078`), so `check_caps`' L3 refusal is inert for most users. `procs.rs` reads a tree's RSS and CPU only on request. Every session has a Job Object (`proc_tree.rs`), which gives per-tree accounting for free.

Congestion is often not RAM: CPU oversubscription from builds, a disk saturated by `pnpm install` or `cargo` linking, a full disk from target folders, or the GPU taken by rendering or ML jobs.

## Requirements

1. Start `Load` at daemon start, independent of phone access; the phone keeps reading the same level.
2. **Machine view.** Each resource gets a state `ok | busy | saturated` with hysteresis.

   | Resource | Windows | Linux | macOS |
   |---|---|---|---|
   | CPU | busy %, processor queue length (PDH `\System\Processor Queue Length`) | PSI `/proc/pressure/cpu` | host load, `processor_info` |
   | RAM | available, commit, `CreateMemoryResourceNotification` | PSI memory | dispatch memory-pressure source |
   | Disk I/O | per physical disk `% Idle Time`, `Avg. Disk Queue Length` (PDH) | PSI io, `/proc/diskstats` | IOKit block storage stats |
   | Disk space | free bytes on the volumes holding project folders and targets | same | same |
   | GPU | per adapter `\GPU Engine(*)\Utilization Percentage`, `\GPU Adapter Memory(*)\Dedicated Usage` (PDH) | NVML when present, else amdgpu sysfs | IOAccelerator `PerformanceStatistics` |

   - The PDH counters are queried once per sample, with one open query.
   - Any counter missing on a machine is reported as "unknown", never as zero.
   - The GPU counter names are marked to verify on this machine's adapter.
3. **Session sampler**, every 2 s while any session works (10 s otherwise). Per live session, from its Job Object:
   - CPU time delta (`JobObjectBasicAccountingInformation`);
   - I/O bytes (`JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION`);
   - commit now and peak (`PeakJobMemoryUsed`);
   - private working set from the one `NtQuerySystemInformation` scan;
   - GPU: per-process `\GPU Process Memory(*)\Dedicated Usage` and engine utilization, summed over the job's pids.

   Unix sums the process group (cgroup `cpu.stat`/`io.stat`/`memory.peak` when Kelpie runs under a delegated cgroup). Each session keeps a 10-minute ring.
4. **Bottleneck attribution.** For each saturated resource, list the sessions (by project and tier) and the outside processes by name that use most of it. This feeds the cards and the view.
5. **Command ledger:**
   - Between a command's pre-tool hook and its post-tool hook (or its end), record the session tree's demand vector: peak RSS, CPU core-seconds and mean cores, disk read and write MB plus peak MB/s, GPU VRAM peak and mean %, and duration.
   - Key: project, fingerprint and the parallelism it ran with.
   - Store: `command-ledger.json`, written atomically with compare-before-write and `.prev` restore.
   - Phase 4 learns from it.
6. RPC `resources.get` returns the machine and session view. The broadcast `resources.changed` fires at most every 2 s.

## Files

- `crates/kelpied/src/load.rs` and `load/*.rs`;
- `mobile.rs`: no longer owns start;
- new `crates/kelpied/src/resources.rs`, with submodules `resources/windows.rs` (PDH, job accounting), `linux.rs` and `macos.rs`;
- `proc_tree.rs`: job accounting queries;
- `daemon.rs`, `crates/kelpie-proto`;
- `app/src/lib/types.ts`, `ipc.ts`.

## Validation

- Unit tests:
  - hysteresis per resource;
  - "unknown" vs zero;
  - ledger keying and atomic write;
  - ring bounds.
- Bench on the 8 GB machine with 6 sessions: the sampler and PDH use under 0.5 % of one core, and `kelpied bench-echo` is unchanged.
- Real check, on an isolated endpoint, against Task Manager / Resource Monitor:
  - `cargo build`: CPU and RAM peak within 15 %;
  - `pnpm install`: disk MB/s within 20 %;
  - a GPU workload (for example `ffmpeg -hwaccel` or a WebGL page): GPU % and VRAM visible.

## Risk / rollback

- Risk: PDH on a few machines is slow or broken. A query that times out disables that counter and logs it once.
- Rollback: `resources.monitor=false` restores phone-only behaviour.
