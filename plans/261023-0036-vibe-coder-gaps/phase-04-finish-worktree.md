# Phase 4 — Finish worktree, overlap watch, cross-review, best-of-N

## Context
Worktree lifecycle stops at "Create pull request" ([git_sync.rs](../../app/src-tauri/src/git_sync.rs));
no local merge-back, no checks, no warning when two agents edit the same files. Runs
([runs.rs](../../crates/kelpied/src/runs.rs)) already create a worktree per task; review notes and
`chooseAgentAndSend` ([send-to-agent.ts:25](../../app/src/lib/send-to-agent.ts#L25)) already send text to agents.

## Requirements
1. **Checks**: `.kelpie.yaml` `check:` (list of commands). "Run checks" in the worktree; result chip.
2. **Finish**: dialog — strategy Squash (default) / Merge / Rebase into base; conflict preview via
   `git merge-tree --write-tree --name-only`; refuse if main checkout dirty; run checks on the
   merged result (in the main checkout, before the user pushes); then remove worktree (+ branch
   optional), stop its sessions (confirm). AI squash message reuses the commit-message helper.
3. **Overlap watch**: with ≥ 2 active worktrees of a repo, every 30 s (and on `Stop` hook)
   compare `diff --name-only base...branch`; chip on both worktrees "Also changed in <wt>: file".
4. **Cross-review**: worktree / Changes menu "Review with…" → agent picker → sends
   `git diff base...HEAD` (bounded like hand-off) + review prompt (editable quick command).
5. **Best-of-N**: new-session menu "Try with N agents": one prompt, pick agents, N worktrees via the
   Runs engine; compare page (files, +/-, checks, diff side by side) → "Use this one" → Finish;
   others removed after confirm.

## Files
- Create: `app/src-tauri/src/git_finish.rs` (checks runner, merge-tree preview, finish, overlap),
  `app/src/components/finish-dialog.tsx`, `app/src/components/compare-page.tsx`.
- Modify: `app/src-tauri/src/worktree_files.rs` (parse `check:`), `app/src-tauri/src/lib.rs`,
  `app/src/components/git-panel.tsx`, `app/src/components/sidebar.tsx` (overlap chip, menu items),
  `app/src/components/new-session-menu.tsx`, `app/src/lib/worktrees.ts`, `app/src/lib/runs.ts`,
  `crates/kelpied/src/runs.rs` (best-of-N template: no deps, no approval), `app/src/lib/quick-commands.ts`
  (default review prompt), locales, docs.

## Validation
- `cargo test`: merge-tree preview on clean / conflicting fixture repos; finish refuses dirty main;
  squash produces one commit; overlap detection.
- e2e `scripts/e2e/finish.py`: two worktrees edit the same file → chip; checks fail → finish blocked
  unless "finish anyway"; squash finish removes worktree and branch; best-of-N with 2 shell "agents"
  (scripted edits) → compare page → pick one → others removed.

## Risk / rollback
- Data loss on remove → reuse existing orphan-commit refusal ([git.rs:304](../../app/src-tauri/src/git.rs#L304)).
- Never force-push or reset; finish only merges locally. Feature is additive; rollback = hide menu items.
