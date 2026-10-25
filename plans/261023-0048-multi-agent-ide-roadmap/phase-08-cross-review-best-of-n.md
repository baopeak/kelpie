---
phase: 8
title: "Cross-review & best-of-N"
status: completed
priority: P2
effort: "4d"
dependencies: [5, 7]
---

# Phase 8: Cross-review & best-of-N

## Overview
Compare and review across different agent CLIs — the thing single-vendor IDEs cannot do.

## Requirements
- **Review notes in the daemon** [RT-6]: `note.add/list/clear` keyed by worktree path, broadcast `notes.changed`; the app store's `reviewNotes` migrates to it
  (one store, diff gutter and `notesPrompt` read it). Agent verb `kelpied review add <file>:<line> "<text>"` (capability `session`, own worktree only);
  daemon fills `side`/`text` from `git diff` for that line.
- **Review with…**: picks an agent → new session in the same worktree with the bounded, redacted diff (phase 1 `redact`) + editable review prompt (quick command default).
- **Try with N agents**: one prompt, 2–5 agents of any vendor → a normal Run with N independent tasks, each `worktree: true` (no new `group` field) [RT-6].
- Run worktrees materialise `.kelpie.yaml` `copy:`/`link:` [RT-6]: move `materialize` from `worktree_files.rs` into `kelpie-proto` (shared) and call it from `create_task_worktree`;
  size estimate shown before start.
- **Compare view** inside `runs-page.tsx` for N-candidate runs: status, duration, cost (phase 5), files, +/−, checks (phase 7), turns; side-by-side diff; "Use this one" → Finish; others removed after confirm.

## Related Code Files
- Create: `crates/kelpied/src/notes.rs`, `crates/kelpie-proto/src/materialize.rs`.
- Modify: `crates/kelpied/src/daemon.rs` (note.*, create_task_worktree materialize, API_LEVEL), `crates/kelpied/src/cli.rs`, `app/src-tauri/src/worktree_files.rs` (use shared),
  `app/src/lib/store.ts` (notes from daemon + migration), `app/src/lib/send-to-agent.ts`, `app/src/components/runs-page.tsx`, `app/src/components/new-session-menu.tsx`,
  `app/src/lib/runs.ts`, `app/src/lib/quick-commands.ts`, locales, docs.

## Implementation Steps
1. Shared materialize + run worktrees use it. 2. Daemon notes + migration + CLI verb. 3. Review-with. 4. Try-with-N launcher. 5. Compare view.

## Tests
- `cargo test`: notes scoped to own worktree; migration keeps existing notes; run worktree gets `link:` dirs.
- e2e `best_of_n.py`: 3 scripted agents → compare → pick → finish → others removed; reviewer agent adds notes via CLI → visible in diff gutter.

## Perf gate
- 5 worktrees created in parallel ≤ 3 s (with links); compare view lazy; no idle cost after the run.

## Success Criteria
- [x] One prompt → 3 vendors → pick best → merged, in one flow; cross-review notes land in the normal diff view.

## Risk Assessment
- Notes migration loss → back up app-state before migration (existing versioned backup).
