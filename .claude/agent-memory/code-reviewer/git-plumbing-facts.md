---
name: git-plumbing-facts
description: Empirically verified git behaviours (git 2.55 Windows) relevant to Kelpie's Finish/worktree code - signing, worktree remove, read-tree, hooksPath
metadata:
  type: reference
---

Verified 2026-10-24 with throwaway repos (scripts kept in the session scratchpad `p7-review\`):
- `git commit-tree` ignores `commit.gpgSign` (needs explicit `-S`); `git commit --no-verify` still signs.
- Non-forced `git worktree remove` with a locked file (Windows): deletes most files AND the admin dir anyway; second call says "is not a working tree". Ignored files never block removal; embedded repos/submodules make it refuse.
- `git read-tree -m -u old new` silently overwrites an ignored file at a path the merge adds (status --porcelain shows clean).
- `-c core.hooksPath=<dir>`: post-commit (even with --no-verify), reference-transaction and post-checkout still run from that dir.
- `-c core.hooksPath=/dev/null` (or `NUL`) via a native spawn: rc 0, no hooks, nothing plantable.
- Killing `git commit` / `commit-tree -S` while the signer waits: no index.lock, nothing moves; the signer grandchild survives.
- `read-tree -m -u` refuses (not uptodate) for assume-unchanged/skip-worktree files with local edits.
- Detached worktree + Finish replay: WIP commit lands on detached HEAD, branch ref merged/deleted, worktree removed -> commits unreachable.

**Why:** these drove C1/H1/H2 of the Finish review; re-probing costs time.
**How to apply:** when reviewing Finish/worktree/removal changes, check whether fixes address these; re-verify if git version changes. See [[shell-hook-blocked-words]] for running probes.
