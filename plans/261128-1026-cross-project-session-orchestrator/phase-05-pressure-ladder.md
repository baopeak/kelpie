# Phase 5: Contention ladder per resource

## Context

Contention still arrives from ungated sources: remote sessions, commands an agent started in the background, apps outside Kelpie, and builds already running when the gate decided. `proc_tree.rs` measured that lowering a shell's CPU priority starves it, so the ladder acts only on **child** trees, and only on the resource that is contended.

## Requirements

Each action names its cause, holds only while that resource stays saturated (with phase-1 hysteresis), and is undone on relief. The agent CLI process, the focused session, and boosted or reserved sessions are never touched. Demotion starts from the lowest effective priority (phase 2) and moves up only while the resource stays saturated.

| Resource saturated | Background-tier children | Normal-tier children (only if still saturated after 30 s) |
|---|---|---|
| CPU | Windows: EcoQoS (`ProcessPowerThrottling`) plus a Job CPU-rate weight of 1–3 on the child's job; Linux: `cpu.weight` / nice +10; macOS: `PRIO_DARWIN_BG` | same, lighter weight |
| RAM | Windows: `ProcessMemoryPriority` LOW; Linux: `memory.high` and `oom_score_adj +500`; macOS: BG | same |
| Disk I/O | Windows: I/O priority low (`ProcessIoPriority`, to verify); Linux: `io.weight` / ionice idle; macOS: BG | same |
| GPU | Nothing can be throttled portably: only cards and holding new GPU commands | — |

At the same time:
- new heavy commands of the matching class are held or re-shaped by phase 4;
- background automations defer (phase 3).

1. **Card at saturation of any resource.** Shown in the inbox, plus a phone push when that is on. It names:
   - the bottleneck;
   - the top users: Kelpie sessions by project and tier, and outside processes by name.

   Actions:
   - **Stop** a child that a session started (a dev server or headless Chrome), using the identity-safe `process.kill` path;
   - **Park** an idle session;
   - **Ignore**.
2. **Disk space low** (under 10 % or 5 GB on a project volume): a card listing the largest Kelpie-made target and worktree folders, with **Clean** for those Kelpie created. A Clean always asks, and never runs automatically.
3. **Park:**
   - Allowed for a session at its prompt (idle, no turn by hooks, no child running), opt-in per tier or by click.
   - Parking ends the agent cleanly. The buffer and conversation id are kept, and Resume uses `--resume <id>` or the preset's resume.
   - **The cache TTL is per agent** (measured: Claude main 1 h, subagents 5 min). When an idle session nears its TTL, the card suggests Compact while the cache is warm. Parking right after the TTL costs nothing extra.
4. Never auto-kill. Never suspend (`NtSuspendProcess`/SIGSTOP) by default.

## Files

- New `crates/kelpied/src/pressure.rs`: the ladder state machine per resource, pure.
- `proc_tree.rs`: EcoQoS, CPU weight, memory and I/O priority setters on child pids, with an identity check.
- `procs.rs`: top users.
- `session.rs`: park and resume.
- `notify.rs`: cards.
- App inbox cards and `locales/*`.

## Validation

- Unit tests:
  - the ladder per resource, with hysteresis and undo;
  - the agent and focused processes are never selected;
  - the GPU path only notifies.
- Echo bench: typing latency in the focused session is unchanged under forced CPU and disk saturation (`KELPIE_TEST_PRESSURE=cpu|ram|io`).
- Real-app e2e on an isolated endpoint:
  - a `hog` child in a background session is demoted on its resource and restored on relief;
  - the card lists it;
  - park → resume restores the conversation.

## Risk / rollback

- Risk: EcoQoS slows a background build. That is intended, only under saturation, and visible.
- Rollback: `pressure.actions=notify_only`.
