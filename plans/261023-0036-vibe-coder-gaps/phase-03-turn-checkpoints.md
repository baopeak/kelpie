# Phase 3 — Turn checkpoints: diff & undo per agent turn

## Context
Changes panel diffs the whole worktree; with several agents in one folder nobody can tell which
turn of which agent changed what. Only Claude has `/rewind`. Daemon already receives
`UserPromptSubmit` / `Stop` hooks ([status.rs:111](../../crates/kelpied/src/status.rs#L111)) and
knows when a prompt was typed for hook-less agents (prompt jump markers).

## Requirements
- A checkpoint at the start of each turn and at its end, for git folders only.
- Pane "Turns" timeline: turn N prompt (first line), files changed, +/-; click → diff (existing
  diff view); "Undo this turn" restores only files that turn changed, with a confirm showing
  which files were changed since by another session (refuse those by default).
- Folder view "Who touched what": file → sessions/turns.
- Never touch the user's index, HEAD, stash or working tree when snapshotting.
- Refs `refs/kelpie/cp/<session>/<turn>`; keep last 50 per session; delete on session close
  (setting: keep until worktree removed).

## Design
- Snapshot in the daemon on a blocking worker: temp `GIT_INDEX_FILE`, `read-tree HEAD`,
  `add -A` (honours .gitignore), `write-tree`, `commit-tree -p HEAD`, `update-ref`.
  Skip when the tree id equals the previous checkpoint's (no-op turn).
- Debounce: one snapshot per hook event; queue per repo (serialised — git index lock).
- Large repos: if `add -A` > 1 s twice, auto-disable for that repo and say so once.
- Undo: `git restore --source=<cp_start> --worktree -- <files>`; files created in the turn → delete
  (to trash, like the explorer).

## Files
- Create: `crates/kelpied/src/checkpoints.rs`, `app/src/components/turn-timeline.tsx`.
- Modify: `crates/kelpied/src/daemon.rs` (hook → snapshot; new calls `checkpoint.list/diff/undo`),
  `crates/kelpie-proto/src/lib.rs` (messages), `crates/kelpied/src/session.rs` (turn counter,
  cleanup on close), `app/src/components/pane-view.tsx` (Turns button in toolbar),
  `app/src/components/diff-view.tsx` (accept two refs), `app/src/lib/ipc.ts`, `app/src/lib/types.ts`,
  `app/src/lib/settings-catalog.ts`, locales, `docs/architecture.md`, `docs/user-guide.md`.

## Validation
- `cargo test`: snapshot leaves `git status` / index byte-identical; untracked files captured;
  ignored files skipped; undo restores only listed files.
- e2e `scripts/e2e/checkpoints.py`: shell session simulating 3 turns (hook events via
  `kelpied hook`), timeline shows 3 turns, diff correct, undo turn 2 keeps turn 3 changes on other files,
  conflict refusal when another session edited the same file.
- Perf: snapshot p95 < 150 ms on a 10k-file repo; measured and written to the phase report.

## Risk / rollback
- Hidden refs grow the object store → cap + `git gc` friendly (loose objects); setting to turn off.
  Rollback: disable setting; refs removable with `git for-each-ref refs/kelpie/cp | xargs update-ref -d`.
