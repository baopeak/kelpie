# Completion — Phase 7: Review queue, Finish, overlap, auto-worktree

Date: 2026-10-24 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-07](../261023-0048-multi-agent-ide-roadmap/phase-07-review-queue-finish.md)

## Delivered
- **Review queue = Board data**: worktrees with status `review` (user, `kelpied worktree set --status review`, automation `deliver: review`, or the Done-with-unmerged prompt) appear in the Board's Review column and a right-panel Review tab. Card: files, +/− committed and uncommitted, checks, cost, turns; Open diff · Request changes (review notes → agent, redacted) · Approve → Finish. Board and review UI are lazy (entry 311 kB gz).
- **`check:` in `.kelpie.yaml`**: read only from the base branch's committed file; approvals stored per section as exact content (setup / tabs / checks), never one approval for another; a candidate branch editing `.kelpie.yaml` is flagged and auto-checks blocked, re-checked in Rust at Finish time; `copy:`/`link:` reach the check worktree only with setup approved; the dialog says checks run the branch's own scripts.
- **Finish** (`git_finish.rs`, `finish.ts`, `finish-dialog.tsx`): stop sessions and wait → re-list and commit exactly the confirmed files as "WIP from <agent>" (new files, embedded repos, detached HEAD stop it) → `merge-tree --write-tree` (conflict → "resolve with agent") → approved checks in a temporary worktree (Job object, cancellable, whole-tree kill; stale temp worktrees pruned links-first) → squash / merge / rebase via `commit-tree` (signed with `-S` when `commit.gpgSign`, 2-min timeout) → CAS `update-ref`; main checkout updated only when clean and no ignored/untracked path collides → detach and delete branch only with ancestry/tree proof → `checkpoint.drop`, remove worktree last (links restored or truthfully reported). No `--force`, no `reset`, hooks off (`core.hooksPath=/dev/null`). Cancel is per run: before the base moves it changes nothing; after, it says what is merged.
- **Overlap watch**: ≥ 2 active worktrees, every 30 s and on turn end → "Also changed in <wt>" chip; 0 scans below two.
- **Auto-worktree**: ask / always / never for new agent sessions in a git repo, name from the first prompt; "always" runs only approved setup.
- Phase 6 follow-up shipped with it: checkpoints judge slowness on git CPU time (a busy machine no longer switches a small repo off); `/dev/null` hooks in the daemon too.

## Verification
- cargo: kelpie 97 (39 `git_finish` incl. lost-work matrix, CAS, branch proof, candidate `check:` never run, detached HEAD, ignored-file collision, planted hooks, confirmed-files WIP, signing flag, Cancel, stale cleanup), kelpied 130, kelpie-proto 14; clippy `-D warnings` clean.
- vitest 185, typecheck, i18n 0 missing; bundle 311 kB gz.
- GUI e2e on isolated daemons: `finish.py` 72/72 (twice by the developer, again by the controller), `checkpoints.py` 55/55 (also under a CPU hog), `context_cost.py`, `attention.py` (controller final run).
- [Review](code-reviewer-261024-1530-phase7-review-finish.md): C1, H1–H4, signing, Mediums, re-review blockers 1–4 and the final Low fixed; final check: ready to ship.

## Not done / follow-ups
- Cold summary of 20 worktrees ≈ 2 s (cached refresh < 50 ms gate passes).
- Checks are added to the Job object after they start (small escape window); `checkpoint.drop` does not cover a session with no snapshot yet; no resume state if the app crashes after the branch was deleted.
- `ui_tour.py` screenshots and macOS not run. Users re-approve `.kelpie.yaml` once (approval format changed).
- Hooks never run for Finish's commits (Gerrit Change-Id / DCO commit-msg hooks won't apply; documented).
