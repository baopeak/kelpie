---
phase: 7
title: "Review queue, Finish, overlap, auto-worktree"
status: completed
priority: P1
effort: "5d"
dependencies: [6]
---

# Phase 7: Review queue, Finish, overlap, auto-worktree

## Overview
Close the worktree lifecycle on top of what exists: the Board's per-worktree status (todo/progress/review/done, `meta.rs:14`) is the single source for "in review" [RT-6].

## Requirements
1. **Auto-worktree**: setting ask / always / never for new agent sessions in a git repo; name from first prompt.
2. **Checks** [RT-5]: `.kelpie.yaml` `check:` parsed in `app/src/lib/worktrees.ts`, read **only from the base branch's committed file**, run only after the existing hash approval
   (`approveSetup`); if `check:` changed vs approved hash → diff + re-approval. If the candidate branch modifies `.kelpie.yaml`, the review card flags it and auto-checks are blocked.
   Rust receives an already-approved command list; commands run without a shell where possible.
3. **Review queue** = Board data [RT-6]: worktrees with status `review` (set by user, by agent via `kelpied worktree set --status review`, or by Done + unmerged changes prompt)
   shown as cards in the Board's Review column and as a filtered right-panel tab. Card: files, +/− (committed **and** uncommitted), checks, cost, turns.
   Actions: Open diff · Request changes (review notes → agent) · Approve → Finish. Automations `deliver: review` = `meta.set status=review`.
4. **Finish** [RT-5] in this order:
   1. stop the worktree's sessions and **wait for exit**;
   2. list uncommitted files → confirm → commit "WIP from <agent>";
   3. build merge with `git merge-tree --write-tree` (conflict → stop, offer "resolve with agent");
   4. run approved checks in a **temporary worktree** checked out at that tree;
   5. strategy: squash (commit-tree with base as parent) / merge / rebase; advance base with `git update-ref <base> <new> <old>` (compare-and-swap); main checkout updated only if clean;
   6. delete branch only after `merge-base --is-ancestor` or tree-equality proof;
   7. remove worktree last; if removal fails, re-create shared links and report.
   Never `--force`, never reset. Finish sets status `done`.
5. **Overlap watch**: ≥ 2 active worktrees → every 30 s and on turn end: intersect `git status --porcelain` + `diff --name-only base...HEAD` sets → chip "Also changed in <wt>".

## Related Code Files
- Create: `app/src-tauri/src/git_finish.rs`, `app/src/components/finish-dialog.tsx`.
- Modify: `app/src/lib/worktrees.ts` (`check:`), `app/src/lib/store.ts` (approval reuse), `app/src-tauri/src/git.rs`, `app/src-tauri/src/lib.rs`, `app/src/components/board.tsx`,
  `app/src/components/right-panel.tsx`, `app/src/components/sidebar.tsx`, `app/src/components/git-panel.tsx`, `app/src/components/new-session-menu.tsx`,
  `crates/kelpied/src/automations.rs` (`deliver`), settings catalog, locales, docs.

## Implementation Steps
1. `check:` parse + approval + runner. 2. Finish sequence with tests per step. 3. Overlap watcher. 4. Board Review cards + right-panel view. 5. Auto-worktree.

## Tests
- `cargo test`: finish with uncommitted work keeps it (WIP commit); failing check leaves base untouched; CAS update-ref refuses when base moved; branch deletion only with proof;
  `check:` from candidate branch never executed.
- e2e `finish.py`: two worktrees edit same file → chip; agent sets `review` via CLI → card appears; squash finish; candidate edits `.kelpie.yaml` → flagged, checks blocked.

## Perf gate
- 0 overlap scans with < 2 active worktrees; queue refresh ≤ 50 ms for 20 worktrees (cached by HEAD + index mtime); panel lazy.

## Success Criteria
- [x] Agent done → merged → cleaned up without typing git, with zero lost work in the test matrix.

## Risk Assessment
- Windows open handles block removal → sessions awaited first; removal failure leaves a consistent, reported state.
