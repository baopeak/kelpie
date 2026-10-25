---
phase: 10
title: "Spec workspace"
status: completed
priority: P2
effort: "3d"
dependencies: [8, 9]
---

# Phase 10: Spec workspace

## Overview
Kiro-style specs without a proprietary format: markdown in the repo, written by any agent, executed by any other, shown as a task board.

## Requirements
- Right-panel tab **Plan**: detect `plans/*/plan.md` + `phase-*.md`, `specs/**`, `.kiro/specs/**`, `docs/plans/**`.
- Render phases/tasks with checkbox state and progress.
- **Run with…** per task [RT-15]: creates a **Run task** (existing Runs engine; agent, here / worktree / host) whose prompt references the file + section via composer expansion;
  the task row shows the run's status (no separate `task_ref` store). Agents can report back with `kelpied run report`.
- Checkbox progress from watcher events; manual tick writes the file with mtime check.
- "New spec" quick command (plan-first template: requirements → design → tasks into `specs/<name>/`).

## Architecture
Parse in TS with `marked.lexer` (already shipped; task-list items) [RT-15], triggered by the existing watcher, debounced 300 ms, only for files under detected roots.

## Related Code Files
- Create: `app/src/components/plan-panel.tsx`, `app/src/lib/spec-index.ts`, `app/src/lib/spec-index.test.ts`.
- Modify: `app/src/components/right-panel.tsx`, `app/src/lib/runs.ts`, `app/src/lib/quick-commands.ts`, locales, docs.

## Implementation Steps
1. `spec-index.ts` + tests (this repo's `plans/`, a Kiro sample). 2. Panel. 3. Run-with via Runs. 4. Tick write-back.

## Tests
- vitest: parser fixtures, malformed markdown. e2e `spec_panel.py`: tasks listed; Run with shell agent → run status on row; editing file ticks box live.

## Perf gate
- Parse ≥ 20 plan files ≤ 20 ms; panel lazy; no work when tab closed.

## Success Criteria
- [x] Plan by agent A, tasks done by B and C, progress in one panel.

## Risk Assessment
- Unknown formats show as plain markdown lists, never hidden.
