---
phase: 11
title: "Event-triggered automations"
status: completed
priority: P2
effort: "3d"
dependencies: [1, 7]
---

# Phase 11: Event-triggered automations

## Overview
Automations run on a clock today. Add event triggers that work for any agent, safely.

## Requirements
- Triggers: file changed matching glob, agent turn finished in project, **commit** (HEAD/branch tip changed — last `rev-parse HEAD` per worktree, not raw ref events) [RT-13],
  PR checks failed (existing `gh` polling), worktree finished.
- Honest scope [RT-13]: file and PR triggers come from the app watcher → fire only while Kelpie's window process runs; agent-turn and commit-by-agent triggers come from the daemon → also with the window closed.
  "User saves vs agent edits" is not claimed; instead "ignore changes while the triggered session (or any session of that trigger) is working".
- Actions: agent prompt, quick command, shell command [RT-13]: shell actions spawned **without a shell**, event data only as env vars / argv (never interpolated);
  created or edited only through the app connection (capability `app`), command hash shown and approved; `automation.event` accepts events from the app connection only.
- Guards: max 1 run per trigger per 60 s, daily cap per trigger, global switch in status-bar menu, dry run.
- Storage [RT-13]: `version` field + daemon-side `.bak` before migration; parse failure → rename to `.corrupt-<ts>`, refuse saves, inbox error (no silent empty list);
  `trigger` automations rejected by older daemons (API_LEVEL bump); due automations run off the ticker (spawned, not awaited).

## Related Code Files
- Modify: `crates/kelpied/src/automations.rs`, `crates/kelpied/src/daemon.rs`, `crates/kelpie-proto/src/lib.rs`, `app/src-tauri/src/watcher.rs`, `app/src-tauri/src/github.rs`,
  `app/src/components/settings/automation-settings.tsx`, locales, docs.

## Implementation Steps
1. Storage hardening + migration. 2. Trigger model + matching + guards. 3. Event sources (daemon, app). 4. Shell action without shell + approval. 5. Settings UI with dry run.

## Tests
- `cargo test`: corrupt file never overwritten; migration keeps schedules; matching/debounce/caps; event data with `$(…)` in a file name is passed literally.
- e2e `event_automations.py`: save `*.md` → action once; triggered session edits don't retrigger; commit trigger fires on HEAD change only; checkpoint refs (phase 6) don't fire it.

## Perf gate
- No new watcher roots; match ≤ 1 ms for 100 triggers; ticker latency unchanged with 10 due automations.

## Success Criteria
- [x] "When a spec changes, ask Gemini to update tasks" works without loops or injection.

## Risk Assessment
- Runaway cost → caps + inbox summary + global switch.
