# Cross-project session orchestrator: CPU, RAM, disk, GPU and tokens

Status: **approved direction, ready to start phase 1** (owner answers 2026-11-28 recorded below)
Date: 2026-11-28

## Outcome

On one small machine (the owner's has 8 GB), as many agent streams as possible run **in parallel without slowing each other down**, and important projects never lose to background work. The daemon:

1. knows which projects matter;
2. measures **CPU, RAM, disk I/O and GPU** per session and per command, and names the resource that is the actual bottleneck;
3. reads the command an agent is about to run (via hooks), estimates its demand on each resource, and then:
   - **re-shapes it to fit**, for example by lowering parallelism;
   - or holds it briefly;
   - or lets it run at once;
4. spends tokens deliberately:
   - budgets per project;
   - idling that is aware of the cache TTL;
   - model tiers and lead/sidekick pairs, after Devin Fusion.

## Evidence

- Kelpie baseline and gaps: [scout](../reports/scout-261128-orchestrator-resource-baseline.md)
- Other orchestrators: [orchestrators](../reports/research-261128-agent-orchestrators.md)
- OS APIs, hook gating, prediction and cache economics: [resource/token](../reports/research-261128-resource-token-control.md)
- Owner questions 3 and 4 verified, plus Devin Fusion: [verification](../reports/verification-261128-1038-hooks-cache-ttl-fusion.md)

## Owner decisions (2026-11-28)

| # | Question | Answer |
|---|---|---|
| Q1 | A held command times out | **Run it automatically**, with a note to the agent and on the row. |
| Q2 | Rewriting commands (parallelism) | **On by default, for every tier.** Always optimize for the most parallel streams without interference (phase 4 optimizer). |
| Q3 | Gemini/Devin hooks | Checked. Both have a blocking pre-tool hook that can deny or rewrite (`BeforeTool`/`run_shell_command`; `PreToolUse`/`exec`). There is no per-run config flag, so a user-level entry is used that is inert outside Kelpie. |
| Q4 | Claude cache TTL | Measured. Main conversations use **1 hour** (5-minute writes = 0); subagents use 5 minutes. Thresholds are per agent. |
| — | Scope | Measure and schedule **CPU, disk, GPU** as well as RAM: congestion often comes from elsewhere. |

## Design decisions

| # | Decision | Why |
|---|---|---|
| D1 | **The user ranks priority by project and by session**:
- a tier (critical, normal, background) plus a drag rank per project;
- a per-session override;
- a temporary **Boost** (30 min, 1 h, 2 h or until the turn ends);
- an optional **guaranteed reservation** (cores, RAM, VRAM) that is checked for feasibility.

One `effective()` priority feeds admission, the optimizer, the ladder and the token router. "Needs you" still sorts first. | The owner asked to guarantee resources for a chosen project or session. It follows the Devin/Claude layering and the Orca attention queue. |
| D2 | Never lower an agent CLI's CPU priority. Demote only child build and test trees, and only when that child's resource is contended: CPU via EcoQoS or a Job CPU weight, RAM via memory priority, disk via I/O priority. | `proc_tree.rs` measured shells starved at below-normal priority. |
| D3 | Every demand is a **vector** (cpu cores, RAM MB, disk MB/s, GPU VRAM MB and GPU %). Admission and the gate pack vectors against capacity minus reserve; the bottleneck resource decides. | The Kubernetes requests, Bazel `resource_set` and Slurm pattern; a single RAM number misses CPU and disk congestion. |
| D4 | The optimizer prefers **re-shaping over waiting over refusing**. A hold is bounded (90 s, or less than the tool's timeout), then the command runs (Q1). It always runs when nothing else heavy runs. | The owner wants maximum parallelism, and no deadlock. |
| D5 | Never auto-kill. Parking is only for an idle session at its prompt, and is opt-in per tier or by click. | Streams, cache and locks. |
| D6 | The token ledger lives in the daemon; no hosted proxy. | Budgets act with the window closed; the cloud is user-owned. |

## Phases

| # | Phase | Depends | API |
|---|---|---|---|
| 1 | [Resource monitor: CPU, RAM, disk, GPU per machine, session and command](phase-01-resource-monitor.md) — **implemented** (API 51) | — | +1 |
| 2 | [Priority ranking (projects, sessions, boost, reservation) and resource policy](phase-02-project-tiers-policy.md) — **implemented** (API 52) | 1 | +1 |
| 3 | [One admission queue for sessions, run tasks, sub-sessions and automations](phase-03-admission-queue.md) — **implemented** | 2 | +1 |
| 4 | [Command gate and parallelism optimizer (Claude, Codex, Gemini, Devin, ACP)](phase-04-command-ram-gate.md) — **4a implemented** (core + Claude); 4b (Codex, Gemini, Devin, ACP) next | 1, 3 | +1 |
| 5 | [Contention ladder per resource](phase-05-pressure-ladder.md) | 2, 4 | +1 |
| 6 | [Token ledger, budgets, cache-aware idling, lead/sidekick routing](phase-06-token-ledger-budgets.md) | 2 | +1 |
| 7 | [Orchestrator view, docs, real-app e2e](phase-07-orchestrator-view-e2e.md) | 1–6 | — |

Order: 1 → 2 → 3 → 4 → 5; phase 6 may start after 2. Each phase gets its own branch off master, its own PR and its own API level, and passes a full review-and-fix loop.

## Acceptance (whole plan)

- On the 8 GB machine, run one critical session plus three or more others, with two Rust builds and one Next build started together:
  - the optimizer splits CPU jobs so the sum of jobs ≤ cores and the RAM sum fits, instead of oversubscribing;
  - the critical session's build starts at once;
  - nothing hits a low-memory kill;
  - total wall time is no worse than running them one after another.

  This is proven by real-app e2e on an isolated endpoint, with the numbers recorded in the PR.
- The orchestrator view shows, per project and session:
  - CPU, RAM, disk and GPU now and at peak;
  - the bottleneck resource;
  - held or re-shaped commands with the reason;
  - tokens and cost today and in the 5-hour window against budget.
- A boosted or reserved session keeps its reserved cores and RAM free while others build. Its heavy command starts at once. When the boost ends, the shares return to normal. The e2e proves both.
- Every hold, rewrite and demotion is visible to the agent and the person, with its cause.
- Agent CLIs keep their priority, and the echo bench shows no regression.
- `cargo test --workspace`, the pnpm checks and the perf gate are green.

## Non-goals

- A hosted scheduler or budget proxy.
- Automatic killing.
- Switching a conversation's model mid-turn.
- Gating remote (SSH/tmux) sessions. These are measured on the remote side later, if ever.

## Open questions

None blocking. Items to measure during the phases:
- the priors on the 8 GB machine;
- whether a Job's commit peak is a good enough proxy for RSS;
- the GPU counters on this machine's adapter.
