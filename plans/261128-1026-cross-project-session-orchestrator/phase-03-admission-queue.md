# Phase 3: One admission queue for every agent session

Status: **implemented** on branch `feat/admission-queue`, stacked on phase 2. Reviewed, and every finding fixed (see
[review](../reports/code-reviewer-261128-1648-admission-queue-phase3.md)). No API level: no new methods; tasks carry
`waiting_since`, and their existing `note` says `Queued: why (in line: N)`.

As built:
- **Queued state:** no new task state. A ready task that waits stays `pending`, with its reason as the note the Runs page now shows.
- **Start vector:** memory only. CPU and disk enter as "no saturated gauge"; the commands an agent runs are the gate's (phase 4).
- **Agent start cost:** the mean private memory of that agent's running sessions (an estimate that errs high), or 400 MiB.
- **Automatic limit:** the memory beyond the system's share at 400 MiB an agent, from 2 to 32 (17 on this 8 GB machine).
- **Background automations:** they wait a minute at a time, for at most 30 minutes, then are recorded as skipped with the busy
  resource.

## Context

`check_caps` counts only sub-sessions, using a count cap of 2 on an 8 GB machine. Run tasks start all at once (`runs.rs`, which has no concurrency limit). Automations start on their schedule regardless of load. A person's own new session is never refused, and must not be.

## Requirements

1. A single `Admission` in the daemon. It is consulted by `sub.spawn`, run task start, automation fire and a session a person opens (a person's session is admitted always, but it counts).
2. Admit when all of these hold:
   - running agents < `max_agents`;
   - per-project limit not reached;
   - the agent CLI's learned start vector (phase-1 ledger: RSS, CPU, disk; prior 400 MB and 0.5 core) fits every resource's free capacity minus reserve, unless the requester is critical;
   - no resource is `saturated`, for a non-critical requester.
3. Otherwise **queue** with a priority:
   - order: effective priority (`resource_policy::effective`: boost, then the session override, then the project tier and rank), then FIFO;
   - a session or project with a guaranteed reservation is admitted against its own reservation first;
   - aging: +1 tier per 15 minutes waiting, so background work is not starved forever;
   - for a sub-session, the caller gets the existing refusal text plus the queue position and what is running, as today.
   - Run tasks show `queued` with the reason.
4. Dequeue on a session exit, a resource leaving `saturated`, or a policy change.
5. Automations in a background project defer while any resource is `busy` or `saturated` and log which resource in the run history.
6. The default `max_agents` is not a fixed count: it is the number of agents whose learned vectors fit (so a big machine runs many, the 8 GB one fewer), capped by the user's setting.

## Files

New `crates/kelpied/src/admission.rs` (pure, unit-tested like `subs.rs`), `daemon/sub_rpc.rs`, `daemon/automation.rs`, `runs.rs` (new `Queued` task state, which is serialized with a compatibility default), proto, `app/src/components/runs-page.tsx`, `app/src/lib/runs.ts`.

## Validation

- Unit: the ordering, aging, reserve and critical bypass. A person's session is always admitted.
- Daemon test: a run with 4 parallel tasks and `max_agents=2` starts two, then the rest.
- No change for users at the default settings on a 16 GB+ machine.

## Risk / rollback

- Risk: a queued run looks stuck. The reason is always shown.
- Rollback: `max_agents` set to a large number disables queueing.
