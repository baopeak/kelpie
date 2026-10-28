---
phase: 6
title: "Turn checkpoints & artifacts"
status: completed
priority: P1
effort: "5d"
dependencies: [1, 5]
---

# Phase 6: Turn checkpoints & artifacts

## Overview
Per-turn snapshots for local git folders, per-turn diff, safe undo, "who touched what", and the files a turn produced.

## Requirements
- **Chained snapshots** [RT-3]: one baseline snapshot at session spawn; afterwards only **turn-end** snapshots; turn N = (end N-1 → end N).
  Removes the start-snapshot race and the need for a start hook.
- Turn-end sources: Claude `Stop`; Codex `agent-turn-complete` (labelled "turn-complete to turn-complete"); ACP `stopReason` (seen by daemon classifier, phase 4);
  other agents: daemon "input after idle → next idle" boundary. Remote (SSH) sessions excluded.
- Snapshot: temp index seeded by **copying `.git/index`** (keeps stat cache), `add -A`, `write-tree`, `commit-tree -p HEAD`, `update-ref`; skip when tree unchanged.
  All git calls with `GIT_LITERAL_PATHSPECS=1` [RT-4].
- Persistence [RT-3]: turn records in `<data_dir>/checkpoints/<sid>.json`; next turn number from highest existing ref; startup GC deletes refs of unknown sessions;
  keep last 50; `git_worktree_remove` calls `checkpoint.drop {worktree}`.
- Watcher ignores `.git/refs/kelpie/**` [RT-3].
- **Undo** [RT-4]: allowed only when the owning session is idle and no other session worked in the same working tree during that turn's window
  (per-repo turn windows recorded). Otherwise files touched by overlapping turns are listed as "shared" with the other session named and need per-file confirmation.
  Always shows a preview of every file to be restored/deleted. Hook-driven boundaries require the session token (phase 1).
- Turns timeline, diff (two refs), owners view, artifacts (md/images/mmd/csv rendered, comment → review note).
- Default [user decision]: on when snapshot p95 < 150 ms on a 10k-file repo; a repo exceeding 1 s twice auto-disables and says so once.

## Architecture
```
spawn ─► baseline ─► … Stop/turn-complete/idle ─► snapshot(end N) ─► diff(end N-1, end N) → turn record (persisted)
```
Daemon worker, one queue per repo; retries on index.lock with backoff, skip after 3 (logged).

## Related Code Files
- Create: `crates/kelpied/src/checkpoints.rs`, `app/src/components/turn-timeline.tsx`.
- Modify: `crates/kelpied/src/daemon.rs`, `crates/kelpied/src/session.rs` (input-after-idle boundary), `crates/kelpied/src/chat.rs` (turn end), `crates/kelpie-proto/src/lib.rs`
  (API_LEVEL, `checkpoint.*` capability app), `app/src-tauri/src/watcher.rs` (ignore refs/kelpie), `app/src-tauri/src/git.rs` (`checkpoint.drop` on remove),
  `app/src/components/pane-view.tsx`, `app/src/components/diff-view.tsx`, `app/src/components/rich-viewers.tsx`, `app/src/lib/ipc.ts`, `app/src/lib/types.ts`, settings catalog, locales, docs.

## Implementation Steps
1. `checkpoints.rs` (purity, literal pathspecs, persistence, GC). 2. Boundaries per agent type. 3. Turn windows + undo rules. 4. UI. 5. Watcher + worktree drop. 6. Measure 10k / 100k repos → default.

## Tests
- `cargo test`: snapshot leaves index/status byte-identical; untracked captured; ignored skipped; file named `:(top)*` does not widen undo; numbering survives restart; GC removes unknown refs.
- e2e `checkpoints.py`: 3 turns → timeline/diff; undo turn 2 keeps turn 3; **two concurrent sessions in one folder** → undo flags shared files and refuses without confirmation; checkpoint ref write emits 0 git `fs-changed` events.

## Perf gate
- Snapshot p95 < 150 ms (10k files), off UI thread; keystroke echo unchanged during snapshots; `.git` growth reported per 100 turns.

## Success Criteria
- [x] Any local agent's turn inspectable; undo never silently reverts another session's work.

## Risk Assessment
- Object growth → cap + GC; index.lock contention → backoff/skip; boundary heuristic for hook-less agents can merge two turns → labelled "approximate" in UI.
