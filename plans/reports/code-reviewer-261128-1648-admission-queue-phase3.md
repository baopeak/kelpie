# Review: admission queue (f95107e7, feat/admission-queue)

Scope: crates/kelpied/src/admission.rs, daemon/admission_gate.rs, runs.rs, daemon.rs (run_tick), daemon/sub_rpc.rs, daemon/automation.rs, app runs page, docs. Line numbers are in D:/Works/Kelpie-worktrees/resource-monitor.

Tests: `cargo test -j 2 -p kelpied -- admission runs:: sub`: 145 passed, 0 failed. None of the passing tests cover findings 1 to 4.

## High

### 1. Tasks whose dependency finishes in the same step skip the queue
- **Where:** daemon.rs:873-879 with runs.rs:519 and runs.rs:600-640.
- **Cause:** `admission_for_tasks()` takes `Runs::ready()` before the step. `step_gated` then first moves Running tasks to Done (exit, or idle-done), and its waiting loop finds the dependants ready in the same pass. Those tasks are missing from the `admission` map, so `_ => ... None => Gate::Open` starts them with no check of count, memory or saturation.
- **Effect:** most runs are chains, so the queue misses most of their starts. The same bypass hits a run created, or a task approved, between `admission_for_tasks` and `self.runs.lock()`.
- **Example:** max_agents 2, two running, an older independent task C queued, and B depends on A. A exits, so the plan admits C. B is missing from the map and starts too: 3 running against a limit of 2.
- **Fix:** treat a missing key as `Gate::Wait` with no note, so it is re-planned next tick. Better, run the planner lazily inside the step after the Running phase, so it sees the statuses as they are then.

### 2. Tasks finished by idle-done keep their places forever
- **Where:** admission_gate.rs:33-51 and 73.
- **Cause:** `running` counts every live non-shell session. Tasks finish by `idle_done_secs` (default 300) while their agent session stays alive. The sub-session caps already exclude these sessions (`finished_task_sessions`, see the test "a finished task's agent holds no place"); the admission count does not.
- **Example:** max_agents 2 and a run of 4 parallel tasks. a and b start, work, idle for 5 minutes and are Done, but their sessions stay alive. c and d stay "queued: 2 agents are running" until the person closes sessions by hand.
- **Test gap:** the gate test only passes because it sets Done with no session existing.
- **Fix:** drop sessions in `runs.finished_sessions()` from `kinds` and `per_project`. Their memory is still in `avail`, so the memory check stays honest.

### 3. Memory is over-admitted across ticks
- **Where:** admission.rs:113 (`avail` reduced only within one plan) and admission_gate.rs:77.
- **Cause:** `avail_mb` is the last sample: every 2 s when busy, 10 s when idle, while the tick runs every 1 s. An agent's private memory reaches its ~400 MiB over many seconds. Starting tasks and young sessions add to `running` but take nothing from `avail`.
- **Example:** 1 GiB free beyond the floor. Each tick admits 2 more tasks of 400 MiB against the same reading, until the count limit (17 on 8 GB). That is exactly the "all at once on a full computer" this commit is meant to prevent.
- **Fix:** subtract `cost_mb` for each Starting task. Also subtract `max(0, cost - private_mb)` for each agent session younger than about 60 s, or until the session has two samples.

### 4. The planner budgets for tasks the sub-session gate will refuse, and starves others
- **Where:** daemon.rs:874-879 and admission.rs:103-121.
- **Cause:** `plan()` charges a count slot and memory for a creator-run task, then `run_gate` refuses it because the creator's sub caps are full. Nobody starts, but a lower-ranked candidate was refused for "the limit" in that same plan.
- **Effect:** the high-priority or long-waiting task is at the head of the queue every tick, so a person's run can starve for as long as another session's sub caps stay full.
- **Fix:** evaluate `run_gate` before planning and leave out creator tasks it refuses (Fail or Wait). Alternatively, let `plan` take a predicate and skip a candidate without charging its share.

### 5. No limit before the first sample, or with the monitor off
- **Where:** admission_gate.rs:52.
- **Cause:** with `max_agents == 0` (the default) and `mem_total_mb` None, the limit is `usize::MAX`. That happens when `KELPIE_RESOURCE_MONITOR=0`, and right after a daemon restart before the sampler's first reading (`Inner::default()`).
- **Example:** a restarted daemon with restored runs starts every ready task on the first tick. With avail None the memory check is skipped too, so nothing limits it.
- **Contradiction:** docs say "about 17 on 8 GB". Also, `reservation_machine` (priority_rpc.rs:75) already falls back to sysinfo for the total.
- **Fix:** use the same sysinfo fallback for `total`. Or, while the monitor is enabled and `resources.samples() == 0`, return a Wait such as "measuring the computer".

## Medium

### 6. `sub.spawn` / `session.create` jump the queue
- **Where:** admission_gate.rs:129-137.
- **Cause:** `admit_sub_session` plans with only itself as a candidate, not the ready run tasks already waiting.
- **Effect:** a background-tier session's spawn takes the last slot ahead of a critical task that has waited 20 minutes. That contradicts "one queue" in the commit and the docs.
- **Fix:** include `runs.ready()` candidates in this plan and read the result for the `sub:` key.

### 7. Concurrent approved spawns are not counted
- **Cause:** `Capacity.running` = live sessions + Starting tasks. It leaves out `Subs::slots`, which counts spawns under way (approved, worktree being made, session not yet created).
- **Effect:** two `spawn_child` calls, or a spawn and a tick, at limit-1 each pass, so the limit is exceeded by one per concurrent spawn.
- **Fix:** add the total of `subs.slots` to `running`, and its cost to the memory budget.

### 8. The per-project limit is not enforced for runs or sessions without a `project_id`
- **Where:** admission_gate.rs:42 and admission_gate.rs:121.
- **Cause:** `per_project` is keyed by `spec.project_id` only, while candidates use `project_id.or(cwd)`, as `effective_map` does. A cwd key never appears in `per_project`.
- **Effect:** `max_agents_per_project` is never enforced for those runs. Live-project reservations miss cwd-keyed projects the same way.
- **Fix:** key by `project_id.or(Some(cwd))` in both places.

### 9. Reservation owner is one value
- **Where:** admission_gate.rs:93-96.
- **Cause:** when the parent session has a reservation, the owner becomes the parent. The candidate's own project's reservation then counts as "reserved for others" against it.
- **Fix:** make `owner` a list (parent session and project) and exclude every one of them from `others`.

### 10. Automation deferral threads pile up and then burst
- **Where:** automation.rs:440-458.
- **Cause:** `due()` reschedules before the thread sleeps, so each firing of a frequent automation (every 1 to 5 minutes) adds a sleeping thread, up to 30 per automation.
- **Effects:**
  - When the gauges go calm, all of them run within the same minute: N agents of one background automation at once, after the busy period.
  - Each thread holds a strong `Arc<Daemon>` for up to 30 minutes. The sampler deliberately uses a Weak reference for this reason.
  - A disabled or deleted automation still fires once it wakes. `run_automation(..., explicit=false)` should be checked for an `enabled` guard.
- **Fix:** keep one deferred slot per automation id: skip a new firing while one waits, or coalesce into it. Re-check `enabled` before running, and hold a Weak reference.
- **Scope note:** only background-project automations are gated, and only on gauges. Normal-project automations ignore max_agents and memory, so "one queue ... automations" in the commit and the user guide overstates it.

### 11. The note changes every sample, so runs.json is rewritten and broadcast
- **Where:** runs.rs:636 and admission.rs:92-121.
- **Cause:** the reason embeds the ordinal and the live free-MiB figure, so `t.note` changes whenever the order or the memory reading moves. `*run != before` then makes `runs_changed` save runs.json (a durable write) and broadcast `run.updated` to the app and phone.
- **Effect:** with tasks queued on memory, that is one disk write and broadcast per sample (2 to 10 s) for as long as they wait.
- **Fix:** keep the persisted note stable (the category and the limit) and carry the live figures in a non-persisted field. Or skip save and broadcast when only the note changed.

### 12. User-facing text is English-only
- **Where:**
  - "queued (1st in line): ...", built by `ordinal()` and the memory sentences (admission.rs:92-160);
  - "skipped: ... stayed busy for 30 minutes" in automation history (automation.rs:449).
- **Problem:** the Runs page now renders `tk().note` raw (runs-page.tsx:65) in an app that ships a Vietnamese locale. These strings were previously not shown in the UI.
- **Fix:** send structured fields (`place`, `reason` code, numbers) and format them with `t()` in the app, or key the note text for i18n.

### 13. Remote (host) tasks are held for local resources
- **Where:** runs.rs:519-535.
- **Cause:** `ReadyTask` has no `host`, so a task that runs on another machine is gated by local memory, saturation and max_agents, and counts in `starting` while Starting. Live remote sessions are excluded from `kinds` (admission_gate.rs:39), so the two counts disagree.
- **Fix:** carry `host` in `ReadyTask`. Admit host tasks without the memory and saturation checks and leave them out of `starting`, matching how live remote sessions are excluded.

## Low

### 14. Wait time is lost when `task_lineage` refuses
- **Where:** runs.rs:622 and daemon.rs:913.
- **Cause:** `waiting_since` is reset to 0 on Open. If `start_task` then bounces the task back to Pending because `task_lineage` refused, its aging restarts.
- **Fix:** reset `waiting_since` only once the session exists.

### 15. Per-tick cost while anything waits
- **Work per tick:**
  - one full `resources.snapshot` (JSON for every track with its open commands, plus users);
  - `info()` for every session under `sessions.read()`;
  - two runs locks;
  - for each creator-run candidate, a full `effective_map()` over all sessions;
  - two priorities locks per candidate.
- **Cost:** small at today's sizes, but O(T·S) every second.
- **Fix:** compute `effective_map()` once per plan. Read the gauges and machine from a small accessor instead of building JSON.

### 16. Starting tasks are briefly counted twice
- **Cause:** while `create_inner` runs, the session already exists and the task is still Starting, so it is counted twice (conservative), and the person sees "N agents are running" one too high.

### 17. The skip message names only the last busy gauge
- **Cause:** "{busy} stayed busy for 30 minutes" names whichever gauge was busy at the last check, not one that was busy the whole time.

## Checked, no defect found
- **Lock ordering:** runs, priorities, sessions.read and resources.inner are each taken and released on their own. `admission_candidate` computes `above` before locking priorities. `Slot` holds no lock, so `spawn_child` holding `_slot` while admitting cannot deadlock with `run_tick`'s `run_gate`.
- **A person's own sessions:** `session.create` from the app is not gated; only the session-token paths call `admit_sub_session`.
- **Shell or custom kinds** cannot reach `session_create_from_session` (parse_create refuses them).
- **Aging:** clamped below BOOST_WEIGHT, and critical status comes from tier or boost, never from aging.

## Test gaps
None of the following is covered by a test:
- a dependent task in the same step (finding 1);
- an idle-done task whose session is still alive (finding 2);
- memory across two ticks with an unchanged sample (finding 3);
- a creator task refused by `run_gate` while a lower-ranked candidate waits (finding 4);
- an unknown total (finding 5);
- sub-spawn against queued tasks (finding 6);
- a cwd-keyed project limit (finding 8);
- automation pile-up (finding 10).

Status: DONE_WITH_CONCERNS
Summary: Five high-severity defects. Tasks whose dependency finishes in the same step skip the queue, and finished idle-done sessions block a run's remaining tasks. Memory reservations are lost across ticks, slots are charged to tasks the sub-session gate refuses, and the agent limit becomes unlimited before the first sample or with the monitor off. Medium issues: pile-up of deferred automations, the sub-session path bypassing the queue, per-project key mismatch, note churn into runs.json, and English-only UI strings.
