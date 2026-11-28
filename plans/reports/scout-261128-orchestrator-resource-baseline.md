# Scout: what Kelpie's orchestrator already has (origin/master, 2026-11-28)

Read from `git show origin/master:` (the main checkout is a stale branch).

## Already in place

| Capability | Where | Notes |
|---|---|---|
| Machine load level L0–L3 with hysteresis (event-loop lag, CPU, free memory, power saver, thermal) | `crates/kelpied/src/load.rs`, `load/windows.rs`, `load/macos.rs` | **Started only while phone access is on** (`mobile.rs:2078`). Otherwise `level()` stays L0, so every check that reads it is inert. Linux probe is a stub. |
| Sub-session caps: depth 3, 10 children per parent, machine cap `auto_cap` (half the cores or (RAM−4)/2 GiB, clamped 2..8; 2 on an 8 GB box), refuse at L3 | `subs.rs` `check_caps`, `daemon/sub_rpc.rs:279,865` | Counts only sub-sessions. Sessions a person opens, run tasks and automations are not counted. The cap is a count, not memory. |
| Per-session Job Object (`KILL_ON_JOB_CLOSE`), on-screen session raised above normal, nothing lowered | `proc_tree.rs` | Measured: below-normal priority starved shells for 30 s or more on 4 cores, so the plan must not lower priority. Memory limits are not set. |
| Process tree scan with RSS and CPU per process (one `NtQuerySystemInformation` on Windows) | `procs.rs` (`Proc.mem`, `Proc.cpu`) | Runs only when the app asks (`process.*`). No sampler and no history. |
| Agent hooks → daemon (`hook.report`): Claude (SessionStart, UserPromptSubmit, **PreToolUse**, PostToolUse, Stop, StopFailure, PostCompact…), Codex notify | `agents.rs:563` `write_claude_hook_settings`, `main.rs:335` `hook()`, `daemon.rs:1558`, `status.rs:214` | **Fire and forget**: hook timeout 5 s, 2 s connect, prints nothing, so it cannot defer or deny a tool call today. The payload holds `tool_name` and `tool_input` (the Bash command) before it runs. |
| Usage-limit detection and "continue at reset" timer | `limit_reset.rs`, `scheduled_input.rs`, `session.rs:130` | Parses "resets 3:20am (…)" text. |
| Token, context and cost per session from the agent's own transcript (Claude, Codex, OMP), cost cap with Interrupt, Compact, Hand off to another agent | `app/src/lib/session-usage.ts`, `usage-alerts.ts`, `cost-page.tsx` | Lives in the app, not the daemon. A cap per session; no budget per project, per day or for the 5 h window. |
| Sub-session outcome stats per agent (routing hints) | `sub_stats.rs` | No cost or token data per agent. |
| Model choice for children (`--model`) | `subs.rs` `model_args` | Chosen by the caller and not routed by policy. |
| Orchestration runs: task DAG, worktrees, approval gate, idle-done | `runs.rs` | **No concurrency limit**: every ready task starts at once. |
| Keep-awake while agents work | `awake.rs` | — |
| Projects: id, name, path, color, pinned, status, remote | `app/src/lib/types.ts:73` (app-owned) | No priority field. The daemon knows `session.project_id` only. |

## Gaps that matter for the goal

1. Load is not measured unless phone access is on. Admission cannot rely on it.
2. There is no notion of project importance, and nothing reserves memory for important work.
3. Admission counts only sub-sessions. Builds started *inside* sessions (the real memory spikes, e.g. cargo, pnpm build, Playwright, PDF/raster jobs) are invisible until the OS or a low-memory reaper kills something at random. This matches the screenshot: a sign/PDF job was stopped for low memory.
4. The hook path cannot answer, so nothing can hold a heavy command until memory frees up.
5. There is no memory history per command, so nothing can predict the next `cargo build`.
6. Token data lives in the app, per session. There is no per-project budget, no cache-TTL awareness for idle sessions, and no policy for routing models by tier.
7. Runs start every ready task at once.
