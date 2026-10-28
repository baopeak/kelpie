# Code review: Review queue, Finish, overlap, auto-worktree (uncommitted)

Date: 2026-10-24 · Branch `rebrand/kelpie` · Reviewer: code-reviewer (read-only, no source edits)
Spec: [phase-07](../261023-0048-multi-agent-ide-roadmap/phase-07-review-queue-finish.md), plan.md DoD / UX rules / RT-5 / RT-6.
Developer report: [fullstack-developer-261024-1415](fullstack-developer-261024-1415-phase7-review-finish.md)

## Scope
- Files: `app/src-tauri/src/git_finish.rs` (+tests), `git.rs`, `lib.rs`, `worktree_files.rs`; `app/src/lib/{finish,review-queue,worktrees,store,ipc,types,commands,settings-catalog}.ts`; components `finish-dialog`, `review-card`, `review-panel`, `board`, `right-panel`, `sidebar`, `git-panel`, `new-session-menu`, `app`, `diff-view`, `pane-view`, settings; `crates/kelpied/src/{automations,daemon}.rs`, `kelpie-proto` (API 11); `scripts/e2e/finish.py`; docs.
- LOC: ~477 changed lines in 26 tracked files + ~2,525 lines in new files.
- Verification I ran: `cargo test -p kelpie git_finish` 20/20 pass; `pnpm test` 172/172; `pnpm typecheck` clean; `pnpm i18n:check` 0 missing; clippy (kelpie, kelpied, all targets, `-D warnings`) clean; `kelpied deliver` tests 2/2. GUI e2e (`finish.py`) not re-run (it needs a fresh build).
- Probes I ran on throwaway repos (git 2.55.0.windows.2) under the scratchpad `p7-review\` folder: `probe1.py`, `probe-detached.py`, `probe-planted-hooks.py`. They replay the exact git calls `git_finish.rs` makes.

## Overall assessment
The core plumbing is careful: candidate commits are built before any ref moves, the base moves by CAS only, conflicts and failing checks leave everything as it was, there is no `--force` or `reset`, and branch deletion needs proof. But four paths lose work or break the trust model, and the tests do not cover them: a detached worktree loses its commits; read-tree overwrites ignored files in the main checkout; a shared temp hooks folder runs planted hooks; and "Approve these checks" approves `setup:` commands the user never saw. Not ready to commit until the Critical and High items are fixed.

## Critical

### C1. A detached worktree's commits, including the WIP commit, are lost and Finish reports "Finished"
- Where: `finish-dialog.tsx:35` (`branch = projectInfo.branch ?? wt.branch`), `git_finish.rs:346-368` (WIP commit goes to whatever HEAD is), `git_finish.rs:689-760` (merges `refs/heads/<branch>`), `git_finish.rs:747` (`checkout_of(branch)` is None, so no detach and no guard), `finish.ts:156` (non-forced `git worktree remove`).
- Scenario: the agent detaches HEAD (`git checkout <sha>`, bisect, a botched rebase it aborted onto a sha) and commits. `projectInfo.branch` becomes null, so the dialog falls back to the stored `wt.branch`. Finish commits the confirmed files as a WIP commit on the **detached** HEAD, then merges the stale branch ref. The tree proof passes because the stale branch is in base, so the branch is deleted. `git worktree remove` sees a clean status and deletes the worktree along with its HEAD reflog.
- Evidence: `probe-detached.py` gives `wip: 0`, `update-ref: 0`, `delete branch: 0`, `worktree remove rc 0`, `main has two.txt: False three.txt: False`, `detached work reachable from any ref: NO`. Only `git fsck --lost-found` can recover it, and only until gc. The normal removal flow guards this case with `checkRemoval().orphanCommits`; Finish skips that check.
- Fix: in `finish_inspect`, `finish_wip_commit` and `finish_integrate`, require `git symbolic-ref -q HEAD` in the worktree to equal `refs/heads/<branch>`. Otherwise refuse with "the worktree is not on <branch>" and offer to create a branch at HEAD. Also take `branch` from `finish_inspect` (Rust reads the worktree's HEAD) instead of `projectInfo ?? wt.branch`. Add a cargo test: detached worktree with commits → Finish refuses and the branch tip and HEAD are unchanged.

## High

### H1. `read-tree -m -u` silently overwrites ignored files in the main checkout
- Where: `git_finish.rs:304-312` (`is_clean` uses `status --porcelain`, which hides ignored files) and `git_finish.rs:733`.
- Scenario: the main checkout holds the user's real `secret.env` / `config/local.json` (ignored). The branch adds a tracked file at that path, for example by changing `.gitignore` and committing a template, or with `add -f`. Finish moves the base and `read-tree -m -u old new` replaces the user's file without a word.
- Evidence: `probe1.py` section 4 gives `main checkout clean per status: True`, `read-tree rc 0`, `main secret.env now: AGENT VERSION`. Porcelain `git merge` does the same, but the zero-lost-work promise is stricter than git's.
- Fix: before the CAS (and again just before read-tree), list `git diff --name-only -z --diff-filter=A <old> <new>` and refuse with `CheckoutDirty { path, files }` if any of those paths exists in the checkout. The same check lets `is_clean` stop blocking on harmless untracked files (see M7).

### H2. The shared `%TEMP%\kelpie-no-hooks` folder runs any hook planted in it, including from a sandboxed agent
- Where: `git_finish.rs:29-33`.
- Scenario: the folder has a fixed name in the user's temp dir, and `create_dir_all` accepts it if it already exists. An agent whose sandbox lets it write to TMPDIR (Codex workspace-write, macOS seatbelt profiles), or any other process, can drop `post-commit`, `reference-transaction` or `post-checkout` there. Every later Finish, in any repository, then runs it outside the agent's sandbox. `--no-verify` only skips `pre-commit`/`commit-msg`.
- Evidence: `probe-planted-hooks.py` shows `post-commit` (WIP commit), `reference-transaction` on every ref update (it can also veto the CAS), and `post-checkout` (scratch `worktree add`) all running.
- Fix: never use a fixed shared folder. Create the empty hooks directory under the app's private data dir and check that it is empty before each use, or create a fresh one per Finish with `tempfile::TempDir`. Add a test that plants a hook and asserts it does not run.

### H3. "Approve these checks" approves the whole `.kelpie.yaml`, including `setup:`/`tabs:` the dialog never showed
- Where: `finish-dialog.tsx:103-108` → `store.approveSetup(repo, hash, check)` (`store.ts:546`). Approval is one hash per repo. `createAgentWorktree` (`worktrees.ts:148-157`) then runs approved setup/tabs with no prompt, which is automatic under auto-worktree "always".
- Scenario: a merged agent branch changed base's `setup:` and `check:`. Finish shows only the check diff; the user approves. The next auto-worktree types the new `setup:` commands into a shell and launches the new custom `tabs:` commands, none of which were reviewed. The reverse also holds: `worktree-dialog.tsx:120-127` lists setup/tabs/copy/link but not `check:`, so check commands get approved there unseen (see M4).
- Fix: either show the full file diff (setup, tabs, copy, link, check) in Finish before approving, or store approvals per section (`approvedChecks` hash separate from `approvedSetups`) so approving checks never approves setup. Render `check:` in the New worktree dialog.

### H4. The WIP commit is not the file set the user confirmed
- Where: `finish-dialog.tsx:56-62` (`finish_inspect` runs when the dialog opens, **before** sessions stop), `finish.ts:120-126` (WIP happens only if `input.uncommitted > 0`, then `add -A` of everything), `git_finish.rs:355`.
- Scenarios:
  1. The agent keeps writing until it is stopped. `add -A` commits files never listed (a `.env` that is not ignored, a 2 GB dump, an embedded clone). With squash they land in base history for good.
  2. The list was empty when the dialog opened, so no WIP commit is made. The merge goes in without the late files, and removal then refuses: "merged, worktree stays".
  3. An embedded repo (`vendor/lib/.git`) becomes a gitlink to a commit that exists only in that inner repo and is merged into base as a dangling submodule pointer. `git worktree remove` then refuses because of the "submodule", while the inner repo's uncommitted work stays in the folder (probe1 section 2).
- The spec order is "stop and wait → list → confirm → commit". The code lists first.
- Fix: after `stopSessions`, call `finish_inspect` again. If the set differs from what the user ticked, stop with a "files changed, confirm again" outcome. Have Rust commit exactly the confirmed pathspecs (`git add -- <paths>` via `--pathspec-from-file -z`, with an `:(literal)` prefix) and refuse when gitlinks or embedded repos appear. Show sizes and flag files over ~5 MB in the list.

## Medium

### M1. Checks can be skipped by a timing race or a base `.kelpie.yaml` that does not parse
- `finish-dialog.tsx:83-84`: `canFinish` does not wait for `plan()`. While the plan resource is still loading, `needsChoice()` is false and `start()` passes `checks: []`, so Finish runs with no checks and no warning. If base's file does not parse, `commands = []` and Finish proceeds with only an informational note.
- Fix: `canFinish` requires `plan.state === "ready"`. A plan error needs the same explicit "Finish without running the checks" tick as an unapproved list.

### M2. The `.kelpie.yaml` block is UI-only, racy, and not re-evaluated after the WIP commit
- `finish-dialog.tsx:63-70` reads `config_changed` from a **non-forced** `refreshChanges`. The cache fingerprint (HEAD, index, refs) does not see an unstaged edit of `.kelpie.yaml`, so for up to 30 s it reports `false`. The WIP commit then puts the edit on the branch while the checks run unblocked. Rust never re-checks.
- Fix: in `finish_integrate`, compute `git diff --name-only base_old <candidate> -- .kelpie.yaml .erka.yaml` and refuse to run checks unless an `allow_config_change: true` argument is passed. This keeps the rule enforceable at the boundary.

### M3. Checks run the branch's own code; this is inherent, but the UI implies otherwise
- `resolve_program` (`git_finish.rs:541-545`) resolves names containing a slash (`./gradlew`, `node_modules/.bin/vitest`) inside the candidate tree. `npm test` and `cargo test` run the branch's `package.json` scripts and `build.rs`. A branch can change what a check does without touching `.kelpie.yaml`, so the `.kelpie.yaml` block does not stop it.
- The dialog text ("From the .kelpie.yaml committed on base, never from the branch") is accurate about the command list but suggests the branch cannot influence what runs. Since the agent already had a shell in the worktree, the extra risk is small. Disclose it in `finish-dialog.tsx:178` and the user guide: "the checks run the branch's code (its scripts, build files)".

### M4. The New worktree approval does not show `check:`
- `worktree-dialog.tsx:120-127`: approving there stores `approvedChecks = config.check` (`worktrees.ts:120`), so later Finishes show the checks as already approved with no diff, even though they were never shown in an approval UI. Render the `check:` lines too.

### M5. A failed removal can leave an orphaned folder with a working junction to shared folders
- `git.rs:405-412` relinks on any error. But a removal that fails on Windows because a file is locked **deletes most files and git's admin dir anyway**. probe1 section 3: `remaining files: ['f2.txt','f3.txt','f4.txt']`, `admin dir exists: False`, second remove gives `is not a working tree`.
- The relinked `node_modules` junction then sits in a folder that is no longer a worktree. `removeWorktree` (`worktrees.ts:234`) prunes it and reports "leftover", so the user deletes the folder by hand through a junction into the main checkout's `node_modules`. The developer's claim of "restore branch + re-attach worktree" (`finish.ts:158-161`) does not apply in this case; it only applies when git refuses cleanly.
- Fix: after a failed `git worktree remove`, relink only if `git worktree list --porcelain` still lists the path. Otherwise report the leftover folder without links. Make the message say "partly deleted" when the admin dir is gone.

### M6. The daemon's turn-end snapshot races Finish
- `session.close` while the agent is Working gives Working→Exited, so `checkpoint_turn_end` (`daemon.rs:1053-1064`) queues a snapshot job that runs git in the worktree. `stopSessions` waits for the pid, not for that job. On Windows the job's open handles can make `git worktree remove` fail part-way (M5). A snapshot ref written after the snapshots were deleted is orphaned until the daemon's startup gc runs. This comes from reading the code; I did not reproduce it.
- Fix: a daemon call to wait for or skip pending checkpoint jobs for a cwd before Finish's step 2, or skip turn-end snapshots for sessions closed by Finish.

### M7. `is_clean` treats any untracked file in the main checkout as dirty
- `git_finish.rs:304-312`: many users keep untracked notes or logs in the main checkout, so Finish is permanently blocked. The advice "commit or stash them" does not help, because `git stash` leaves untracked files. With the H1 collision check in place, block only on tracked changes and on untracked or ignored paths that the merge adds.

### M8. Cold summaries take the app-wide git slots
- `GIT_CONCURRENCY = 3` (`git.rs:14`). A cold 20-worktree refresh is about 80 git processes, roughly 2.2 s of queue. `review_changes` does not block the WebView thread (it is an async Tauri command), but every other git call waits behind it on the fair semaphore: sidebar branch info, the git panel, diff view, commit. Overlap scans use `force=true` every 30 s and read up to 500 untracked files × 1 MiB for line counts they do not need (`git_finish.rs:213`).
- Cheaper approach:
  1. One `git diff --numstat -z --merge-base refs/heads/<base>` (git ≥ 2.30) replaces `merge-base` + `diff`. That gives one serial step instead of two, and 3 processes instead of 4.
  2. Give overlap a names-only mode (`status` + `diff --name-only --merge-base`, no line counting) and let it use the cache key.
  3. Load cards in the visible column first.
  4. Run review scans in their own semaphore (or at lower priority) so they cannot starve interactive git calls.

### M9. The base silently follows the main checkout's current branch
- `review-queue.ts:39-43`. If the user switches the main checkout to `develop`, every worktree that forked from `main` is compared against `develop`: inflated files and +/−, false overlap chips. Finish would merge into `develop`; the dialog shows `branch → develop`, but nothing points out the change.
- Fix: record the start branch at creation (`createWorktree` already knows `opts.base`; store it in project meta) and use it as the default base. This also answers the developer's question 2.

### M10. Errors after the base moved are reported as "stopped"
- `finish_integrate` propagates `?` errors from `checkout_of` / `delete_if_merged` (`git_finish.rs:746-754`) after the CAS succeeded and the checkout was updated. JS then shows `stopped/error` with "Try again", and the card is not set to Done. Retrying does recover (it hits Nothing, then deletes and removes), but the user is told nothing was merged.
- Fix: after a successful advance, turn later failures into `Merged { branch_note }` instead of `Err`.

### M11. A check cannot be cancelled, and a timeout kills only the direct child
- `git_finish.rs:573-598`: up to 20 min per command; the dialog blocks Close and Escape while `step()` is set. `kill_on_drop` kills `cmd.exe`, not its `node` grandchild, which keeps running branch code and holding the scratch folder. `output()` also buffers all output in memory before tailing it.
- Fix: put the check in a Job Object with kill-on-close (the daemon has ConPTY / job code to reuse), add a Cancel button that drops the future, and stream into a ring buffer of `OUTPUT_TAIL`.

## Low
- **L1. Crash leftovers.**
  - A crash during the checks leaves `%TEMP%\kelpie-finish-*` registered as a detached worktree. It shows in `git worktree list` and possibly in "add existing worktrees". Prune or remove `kelpie-finish-*` entries at startup.
  - A crash between `update-ref` and `read-tree` (`git_finish.rs:729-733`) leaves the main checkout's index at the old tree. A `git commit` there would revert the merge. The window is milliseconds; the reflog note `kelpie finish:` helps.
  - A crash after branch deletion leaves no resume path: `finish_inspect` fails on the missing branch.
  - Fix for both resume cases: let `finish_inspect` recognise a "branch gone, worktree detached at a commit contained in base" state and offer "remove the worktree".
- **L2. Sessions are stopped even when Finish is bound to stop at a conflict.** `merge-tree` is read-only. Run a conflict pre-check in `finish_inspect` so that running agents are not killed when the merge is going to conflict anyway.
- **L3. Branch naming.**
  - `uniqueName` (`worktrees.ts`) compares case-sensitively, but loose refs on Windows and macOS collide case-insensitively (`Fix-login` vs `fix-login`).
  - The folder is not checked for existence, for example a leftover from M5.
  - Both cases end in a "Could not create the worktree" toast.
  - Otherwise `branchFromPrompt` only produces `[a-z0-9-]`, never starts with `-` and is capped at 40 characters. That is safe.
- **L4. The "Done with unmerged changes" prompt uses Cancel to mean "Done anyway"** (`review-queue.ts:149-152`). Esc commits the destructive-looking choice. Use a three-button dialog (developer's question 3).
- **L5. The cache fingerprint ignores reftable repos** (`git_finish.rs:149-158`). Ref stamps are constant there, so the cache can be up to 30 s stale.
- **L6. `merge.verifySignatures=true` repos.** Porcelain merge would refuse unsigned branch tips; Finish's `commit-tree` bypasses that policy. Honour it or mention it in the docs.
- **L7. Unused fields.** `Integrated::Merged.base_old` is not used by the UI. `Candidate::Ready { commit, .. }` has a needless `..`.
- **L8. Rust error strings are English-only in the vi UI** (`"the branch contains merge commits; use squash..."`). This matches the existing pattern; noted only.
- **L9. Automation delivery.** `deliver: review` on a non-worktree folder moves the repo's card to Review, but the card has no review body or actions, because `board.tsx` requires `parentId`. Either hide the option for non-worktree folders or document it.

## Controller decision: signing on, hooks off
Current state, verified with `probe1.py` section 1 (fake `gpg.program`, `commit.gpgsign=true`):
- `git commit-tree` **ignores** `commit.gpgSign`: `commit-tree rc 0 gpg called: False`. So squash, merge and rebase commits are **never signed** today.
- `git commit --no-verify` (the WIP commit, `git_finish.rs:363`) **does** honour it: `gpg called: True`. When signing fails, the WIP commit fails (`rc 128`). Behaviour is inconsistent today: WIP commits are signed, final commits are not.

Recommendation, which matches the controller's intent:
1. Once per Finish, read `git config --type=bool --get commit.gpgSign` in the repo (this covers global, include and worktree config). If true, pass `-S` to every `commit-tree` (`git_finish.rs:476`, `503-508`). git then picks up `user.signingKey`, `gpg.format` (openpgp, ssh or x509) and `gpg.ssh.program` itself. If false, pass nothing (or `--no-gpg-sign`) so the behaviour is explicit.
2. Keep hooks off, including for the WIP commit. Beyond predictability there is a security reason: with husky-style `core.hooksPath=.husky`, the hooks live in the **branch's tree**, so running hooks in the worktree would run branch-controlled scripts outside the check gate. But fix H2 first, because "off" currently means "whatever is in a shared temp folder".
3. Risks to accept and handle:
   - pinentry or hardware-key touch prompts can block Finish indefinitely. There is no cancel (M11), so add a timeout or cancel around signing too.
   - A rebase of N commits needs N signatures, which can mean N prompts or touches.
   - ssh signing with a passphrase-protected key and no agent fails, because there is no TTY (git runs without a console). That is a clean failure, before any ref moves, since candidates are built before the checks and the CAS.
   - Original signatures on replayed commits are dropped; the committer re-signs.
   - Repos that depend on `commit-msg` / `prepare-commit-msg` hooks (Gerrit Change-Id, DCO sign-off, commitlint) get final commits without them. Document this in the user guide. Today only architecture.md mentions hooks, and nothing mentions signing.
4. Tests:
   - A fake `gpg.program` that records its calls: with `commit.gpgsign=true`, squash, merge and rebase all invoke it.
   - A failing signer leaves base, branch and worktree untouched.
   - With `commit.gpgsign=false`, it is never invoked.

## Edge cases found by scouting
- A detached worktree (C1), ignored-file collisions (H1), a planted hooks dir (H2), an embedded git repo or submodule (H4.3), a locked file during removal (M5), a checkpoint job racing removal (M6), an unparsable base config or a still-loading plan (M1), a stale `config_changed` cache (M2), the main checkout switched to another branch (M9), and crashes between steps (L1).
- Verified safe:
  - `update-ref` CAS on a moved base (test and code).
  - A local tracked edit in the main checkout makes `read-tree -m -u` refuse, which is twoway-merge `verify_uptodate`; the TOCTOU window only matters for ignored files (H1).
  - `remove_dir_all` does not follow junctions.
  - `head_ref` rejects option-like names.
  - The scratch path and the commit ids cannot be read as options.
  - Request changes and "Resolve/Fix with agent" go through `redact`.
  - `deliver` is validated in the daemon; old automation files still load.
  - The overlap watch does 0 scans with fewer than 2 live agent members.
  - The candidate's `pwn` program is not resolved from the checkout.

## Review queue and overlap (focus 4-5)
- Board status is the single source (RT-6). `inReview` reads `project.status`, and `meta.updated` moves cards; there is no duplicate store. The `kelpied worktree set --status review` path is unchanged and valid (`meta.rs:14`). Automation delivery uses `set_meta`. Card contents match the spec: files, +/− (committed and uncommitted), commits, uncommitted, checks chip, `.kelpie.yaml` flag, cost, turns.
- Overlap intersect logic is correct (`review-queue.ts:75-86`). The repo's own folder contributes only its uncommitted files; this is disclosed. The 30 s interval plus a 1.5 s debounce after `session.turn` matches the spec. Cost: see M8.

## Recommended actions (in order)
1. C1: enforce worktree HEAD == `refs/heads/<branch>` in Rust and take the branch from Rust; add a test.
2. H2: private, verified-empty hooks dir; add a planted-hook test.
3. H1 + M7: collision check for paths the merge adds, covering untracked and ignored files; relax `is_clean`.
4. H4: re-inspect after stopping sessions; commit only the confirmed pathspecs; refuse gitlinks and embedded repos; show sizes.
5. H3 + M4: separate approval of checks from setup, or show the full file diff; render `check:` in New worktree.
6. Signing: `-S` when `commit.gpgSign` is true, plus tests and user-guide text (hooks stay off).
7. M1 / M2: gate on plan readiness; enforce the `.kelpie.yaml` block in Rust.
8. M5 / M6 / M10 / M11: removal relink only while the worktree is registered, checkpoint drain, post-CAS errors reported as merged, Job Object plus cancel for checks.
9. M8 / M9: a cheaper scan with its own semaphore; record the fork base.

## Metrics
- Rust: `cargo test -p kelpie git_finish` 20/20 pass. `cargo clippy -p kelpie -p kelpied --all-targets -D warnings` clean. `cargo test -p kelpied deliver` 2/2 pass.
- Frontend: vitest 172/172, `tsc --noEmit` clean, i18n 1603 keys / 0 missing.
- Test gaps: none of C1, H1, H2, H4 or signing is covered; the open-handle test asserts the relink but not the unregistered state that follows (M5).

## Plan follow-ups (no plan files edited)
- Spec items delivered: auto-worktree, `check:` parse and hash approval, review queue as Board data, Finish 7 steps (+ detach step), overlap, automations `deliver`, docs, e2e.
- Not met yet: "zero lost work in the test matrix" (C1, H1, H4); RT-5 approval integrity (H3 / M4); DoD #6 `ui_tour.py` not run (developer report).

## Unresolved questions
1. Should Finish refuse when the main checkout has *any* untracked file (current behaviour) or only colliding ones (recommended)?
2. Is a per-section approval store acceptable (it changes the `approvedSetups` semantics), or should Finish show the full `.kelpie.yaml` diff instead?
3. Should the Finish base be the recorded fork base (recommended) or stay "the repo's current branch" with a picker?

First-pass status (superseded by the re-review below): DONE_WITH_CONCERNS. Probes reproduced C1, H1 and H2; H3 and H4 and the lack of signing were found by reading the code.

---

## Re-review (2026-10-24, after the developer's fixes)

Scope: the current working tree, with the updated developer report. Report only; no source edits.

Verification I ran:
- `cargo test -p kelpie git_finish`: **35/35 pass**.
- `cargo test -p kelpied hook`: 4/4 pass, including `a_hook_planted_in_the_hooks_folder_is_removed_before_git_runs`.
- `pnpm test` 182/182; `tsc` clean; i18n 1623 keys / 0 missing.
- New probes, in the scratchpad `p7-review\` folder: `probe-sign-cancel.py`, `probe-assume-unchanged.py`, `probe-hooks-fallback.py`.
- I deleted my scratch repos (`w1`..`w6`).
- I did **not** delete `D:\kelpie-p3-target\debug\incremental` (2.9 GB, part of it from my test builds). Another `cargo`/`rustc` was running against that shared target dir when I finished, and deleting it under a live build could break that build. D: had 8.9 GB free. The coordinator can delete it once the build is idle; cargo regenerates it.

### 1. The original findings, re-checked
My first probes replayed the old code's git calls. The same scenarios now run through the real functions in the developer's tests. I read each test to confirm it asserts the probe's failure condition, then ran it.

| Finding | Result | Evidence |
|---|---|---|
| C1 detached worktree | **Fails safe** | `finish_inspect` reads `symbolic-ref -q HEAD`. `finish_wip_commit` and `integrate` call `require_on_branch` (`git_finish.rs:127-133`, `571`, `1046`). `runFinish` re-inspects after `stopSessions` (`finish.ts:136-137`). Test `a_detached_worktree_is_never_finished`: main, the branch and HEAD are unchanged, the late file is kept, and the "other branch" case is refused too. |
| H1 ignored file overwritten | **Fails safe** | `collisions()` (`git_finish.rs:474-489`) runs before the checks and again before the CAS. Tests: an ignored `secret.env` is kept, and a file where a folder would go collides. My new probe: a tracked file marked assume-unchanged / skip-worktree with local edits passes the gate, `read-tree` refuses ("not uptodate"), the CAS undo runs and the local file is kept. Safe. |
| H2 planted hooks (Finish) | **Fails safe**, with Low TOCTOU | The hooks dir is now `<data>/no-hooks`, emptied before every call, and an error is raised if it cannot be emptied (`git_finish.rs:36-58`). Test `hooks_planted_in_the_hooks_folder_do_not_run` passes. |
| H3 approvals | **Mostly fixed; two new holes** | `approveChecks` only stores `approvedChecks` (`finish-dialog.tsx:128-133`). `setupHash` covers setup, tabs, copy and link only. See RB-3 and RB-4. |
| H4 WIP file set | **Fails safe** | After the stop: re-list → compare → `finish_wip_commit(paths)` returns `extra` instead of committing, using literal pathspecs. `integrate` refuses any leftover file (`WorktreeChanged`). Tests cover the exact set, a late `.env`, a glob-looking name, an embedded repo, and a late file after an empty list. Race analysis: a file appearing between Rust's status and `git add` is not staged (literal specs), so `integrate` stops on it. No hole. |
| Signing `-S` | **Honoured** | `Maker::commit_tree` adds `-S` when `config --type=bool commit.gpgSign` is true (`git_finish.rs:697-716`). Test `commits_are_signed_when_the_repository_signs` covers squash, merge, rebase and WIP: the signer is invoked and the commit carries `BEGIN PGP SIGNATURE`. With `commit.gpgSign=false` the signer is never called. |
| Failed signature changes nothing | **Yes** | Test `a_failing_signer_changes_nothing` (squash, rebase). My probe killed `git commit` and `git commit-tree -S` while a signer was waiting, the way `kill_on_drop` kills on a timeout or Cancel: HEAD and main unchanged, **no `index.lock` left**, the next `git add` works. |
| 2-minute timeout | **Correct by construction; not tested at 120 s** | `run_signed` = `timeout(120 s)` + `kill_on_drop` (`git_finish.rs:558-563`, `git.rs`). The kill path is safe (probe above). Low: the signer process (`gpg`/pinentry/`ssh-keygen`) is a grandchild and survives the kill; in the probe the stand-in signers kept running. A pinentry window can stay up after "timed out". |
| Cancel | **Broken outside `finish_integrate`**, see RB-1 | |

### 2. New holes introduced by the fixes

#### RB-1 [High, still blocking]: Cancel is ignored before the merge, and stays stuck "on" afterwards
- Where: `CANCEL` is a process-wide flag. It is set by `finish_cancel` (`git_finish.rs:152`) and cleared **only** at the start of `finish_integrate` (`git_finish.rs:1027`). The "Cancel Finish" button shows whenever `step()` is set (`finish-dialog.tsx:264`): during the session stop, the re-list, the WIP commit, the merge and the removal.
- Scenario A (Cancel ignored): the user presses Cancel while sessions stop or during the re-list or an unsigned WIP commit. `finish_integrate` then clears the flag, and Finish **merges into base, deletes the branch and removes the worktree** after the user pressed Cancel.
- Scenario B (stuck): the user presses Cancel during "Removing the worktree…" (or after the last cancel check in `integrate`). The flag stays `true`. The next Finish of *any* worktree with uncommitted files reaches `finish_wip_commit`, where `run_signed` sees `cancelled()` resolve at once and fails with "the WIP commit failed: cancelled". "Try again" repeats the failure until the app restarts, because `finish_integrate` (the only reset) is never reached.
- Fix:
  1. Clear the flag when a Finish *starts*: a `finish_begin` command called first in `runFinish`, or a run id passed with every call.
  2. In `runFinish`, check for cancel between steps and return `cancelled` before `finishIntegrate`.
  3. Once the CAS has succeeded, disable the button or relabel it ("Stops after the merge").
  4. Add a vitest for `runFinish` with Cancel during `stopSessions`, and a cargo test that a stale flag does not fail the next `finish_wip_commit`.

#### RB-2 [blocking, small]: Leftover check worktrees from a crash (L1 still open)
See section 3.

#### RB-3 [Medium, still blocking]: Approving checks still approves the base's `copy:`/`link:` for the check worktree
- Where: `finish-dialog.tsx:112` passes `cfg.copy` / `cfg.link` whenever the checks run. They are gated only by `checksApproved`, never by `configApproved` (setupHash), even though copy and link are exactly the sections setupHash protects.
- Scenario: a merged branch changed base's `copy:` to `secrets/prod.env` (ignored in the main checkout). Approving the new check list makes Finish copy that file into the scratch worktree, where the **branch's own code** runs as the check (M3). That contradicts "approving checks never approves setup".
- Fix (one line): `copy`/`link` = `configApproved(repo, cfg) ? cfg.copy/link : []`. Show "copy/link not approved: checks run without them" in the dialog.

#### RB-4 [Medium, still blocking]: An old setup approval auto-approves a changed `check:` list in New worktree
- Where: `worktree-dialog.tsx:65` ticks "Run .kelpie.yaml" when `configApproved` (setupHash) matches. `create()` then calls `approveChecks(cfg.check)` (`worktree-dialog.tsx:78-81`) whether or not the check list changed.
- Scenario: base's `check:` changed (its `setup:` did not). The box is pre-ticked by the old approval, Enter creates the worktree, and the new checks are now approved with no diff or highlight. This is the "old approval counts for a new section" case the coordinator asked about.
- Fix: pre-tick only when `configApproved && checksApproved`. Otherwise leave the box unticked and mark changed `check:` lines, as Finish's diff does.

#### RB-5 [Medium, non-blocking]: The daemon's hooks fallback points into the folder it could not empty
- Where: `checkpoints.rs:237-245`. When `no-hooks` cannot be emptied, `verified_empty` returns `no-hooks/missing`, and a planted `missing/` folder is exactly the entry that blocks emptying.
- Evidence: `probe-hooks-fallback.py` replays the function's steps. It plants `no-hooks/missing/reference-transaction` and holds that file open without delete sharing. Result: "could not remove missing", "hooksPath chosen: …\no-hooks\missing", "planted hook ran: True".
- This needs write access to the Kelpie data dir, which most agent sandboxes do not grant, hence non-blocking.
- Fix: fall back to `/dev/null`. Better still, use `core.hooksPath=/dev/null` in both `checkpoints.rs` and `git_finish.rs` and drop the folder entirely. I verified on Git for Windows 2.55 that `-c core.hooksPath=/dev/null` (and `NUL`), passed without MSYS path conversion, gives rc 0 and runs no hooks. Nothing can be planted there, there is no TOCTOU window between emptying and use, and there is no filesystem work on every git call.

#### RB-6 [Medium, non-blocking]: Post-CAS `?` and the cancel mapping can still misreport a moved base
- Where: `git_finish.rs:1103`, `plumb(&dir, ["read-tree", …]).await?`. `plumb` can now fail before git runs (`empty_hooks` errors when the folder cannot be emptied). If that happens after the CAS, `integrate` returns `Err` with **no undo**. Base has moved, but the main checkout's index and files are still the old ones. A commit there would revert the merge, and the user is told "stopped".
- Also, `finish_integrate` maps any `Err` to `Cancelled` ("Nothing was changed") when the flag is set (`git_finish.rs:1028-1030`), and that includes this post-CAS error.
- This is unlikely: the hooks folder would have to become un-emptiable in the milliseconds between the CAS and `read-tree`. It would vanish with the `/dev/null` change.
- Fix: handle `Err` from that `plumb` like `!rt.ok` (run the undo), and only map to `Cancelled` before the CAS (track a `moved` flag).

#### RB-7 [Medium, non-blocking]: Approvals rest on 32-bit FNV-1a (`worktrees.ts:35-39`)
- The old whole-file values (`hex:len`) cannot match the new `setupHash` by accident, so the migration itself is safe and forces one re-approval.
- But FNV-1a/32 can be forged at a fixed length. An agent that can read Kelpie's state file (sandboxes usually allow reads) can craft a `setup:` that collides with the approved hash. After the user merges that `.kelpie.yaml` change, an "always" auto-worktree runs it with no prompt.
- This weakness predates Phase 7, but auto-worktree removes the last prompt.
- Fix: store the approved canonical JSON string itself and compare for equality, as `checksApproved` already does. That also means one re-approval and no migration code.

#### Checked and fine
- **Collision check:** case-insensitive on Windows (`symlink_metadata`), D/F both ways, `--no-renames`. Paths the merge modifies are covered by `tracked_clean` plus `read-tree`'s own refusal (assume-unchanged probe).
- **Re-list/confirm race:** no hole; see H4 above.
- **Job object:** the timeout test proves a grandchild is killed. Low: the child is assigned to the job *after* `spawn` (`git_finish.rs:894-898`), so a grandchild started in that window would escape. Spawning suspended and then assigning would close it. Unix kills the process group.
- **Truthful reporting:** `delete_if_merged` errors become `Merged{branch_note}` (test passes). The removal failure says "The work is merged into {base}, but the worktree could not be removed". `remove_linked_worktree` relinks only while git still lists the worktree, and otherwise reports "partly deleted … not linked back" (`git.rs:424-440`). The exceptions are RB-6.
- **`checkpoint.drop` ordering:** it is called before `removeWorktree` (`finish.ts:184`). `remove_session` marks the session dropped and takes the repo lock, so an in-flight snapshot finishes first and queued jobs skip dropped sessions.
  - Low: a session with no snapshot log yet is not marked dropped, so its queued job can still run during the removal.
  - Low: the snapshots are dropped even when the removal then fails and the worktree is re-attached, so its undo history is lost (the work itself is merged).
- **Old daemon:** `.catch(() => {})` keeps an older daemon working.

### 3. L1 (crash leftovers): is it acceptable to ship?
Not as-is, for one of its three parts:
1. **A crash during the checks: not acceptable.** The leftover `%TEMP%\kelpie-finish-<pid>-<nanos>` folder stays registered as a worktree. It holds up to the 2 GiB `copy:` budget and **live junctions to the main checkout's `link:` folders** (`node_modules`, or any ignored folder the config links). Deleting the folder with Windows PowerShell 5.1 `Remove-Item -Recurse -Force` (this user's default shell) or a junction-following temp cleaner can delete what the junctions point to. `git worktree prune` leaves it alone because the folder exists.
   - **Minimum fix**, about 20 lines plus one test: at the start of `run_checks` (optionally also at app start), list the repo's worktrees. For each path under `std::env::temp_dir()` named `kelpie-finish-<pid>-*` where `<pid>` is not the current process, run `unlink_shared`, then `remove_dir_all`, then `worktree prune`. The test plants such a folder with a junction, runs `run_checks`, and asserts the folder is gone and the junction target is intact.
2. **A crash between branch deletion and removal: acceptable.** The content is proven to be in base. The worktree is detached at the tip; re-opening Finish shows the detached-HEAD message, and the user can remove the worktree.
3. **A crash between `update-ref` and `read-tree` (milliseconds): acceptable.** The reflog note `kelpie finish:` makes the state recoverable. A one-line mention in the user guide is enough.

### Still blocking (minimum to ship)
1. **RB-1:** Cancel ignored before the merge, and stuck after it.
2. **RB-2 / L1:** prune stale `kelpie-finish-*` check worktrees, removing their links first.
3. **RB-3:** check worktrees get `copy:`/`link:` only when setupHash is approved.
4. **RB-4:** New worktree does not pre-tick, or silently approve, a changed `check:` list.

Recommended, not blocking: RB-5 and RB-6 (both go away with `core.hooksPath=/dev/null` plus undo on any post-CAS error), RB-7 (store approved content instead of FNV), and the Lows above.

### Re-review metrics
- Rust: `git_finish` 35/35, kelpied hook tests 4/4. Frontend: 182/182, tsc clean, i18n 0 missing. GUI e2e not re-run.
- Not covered by tests: Cancel outside `integrate` (RB-1), the copy/link gating (RB-3), the dialog pre-tick (RB-4), the daemon's hooks fallback (RB-5), stale-scratch pruning (RB-2).

Re-review status (superseded by the final check below): DONE_WITH_CONCERNS, with four blockers: RB-1, RB-2 (L1), RB-3, RB-4.

---

## Final check (2026-10-24)

Scope: the current `d:\Works\Kelpie` working tree. I did not touch `D:\kelpie-p9`. For `checkpoints.rs` I judged only the hooksPath change.

Verification I ran:
- `cargo test -p kelpie git_finish`: **39/39 pass**. New tests: `cancel_belongs_to_one_finish`, `a_stale_cancel_does_not_fail_the_next_wip_commit`, `a_cancel_pressed_while_signing_waits_changes_nothing`, `check_worktrees_left_by_a_crashed_kelpie_are_removed_without_following_their_links`.
- `cargo test -p kelpied hook`: 4/4, including `no_hook_runs_for_snapshot_git_calls`.
- vitest 185/185 (Cancel at each step, after the merge, and the reset calls); `tsc` clean; i18n 1627 keys / 0 missing.
- No `kelpie-finish-*` leftovers in `%TEMP%` after the runs.
- Disk: once no cargo/rustc was running, I deleted `D:\kelpie-p3-target\debug\incremental` (regenerable, 1.4 GB). D: is at 7.9 GB free.

| # | Item | Verdict | Evidence |
|---|---|---|---|
| 1 | Cancel at every boundary; never sticks; never "nothing changed" after the base moved | **Pass, one residual (FC-1, Low)** | The flag is now per worktree (`CANCELLED` set keyed by path). It is cleared by `finish_reset` at the start and in the `finally` of `runFinish` (`finish.ts`), and by `finish_integrate` on return (`git_finish.rs:1047-1058`). JS checks it after the session stop, after the re-list, and before the merge. Rust checks it before the checks and before the CAS, and inside signing and checks. `cancelled_early = !moved && …` maps an `Err` to `Cancelled` only before the move; the undo path sets `moved = !put_back`. After the merge the JS path puts the branch back and reports "merged … worktree and branch stay", and the button is disabled once the merge is done (`pastMerge`). The stale-flag case is covered by a test. |
| 2 | Stale `kelpie-finish-<pid>-*` cleanup | **Pass** | `prune_stale_scratch` (`git_finish.rs:918-940`) only touches worktrees git lists, directly in the canonical temp dir, whose pid is not ours and not alive. An error checking the pid counts as alive, so it skips. It runs `unlink_shared` (uses `symlink_metadata`, never descends into links), then `remove_dir_all` (Rust removes a link without following it), then `worktree prune`. The test shows the leftover is gone, the junction target survives, git's record is gone, and our own pid's folder is kept. Another live Kelpie instance (such as `D:\kelpie-p9`) is skipped because its pid is alive. A reused pid only leaves the folder in place, which is safe. |
| 3 | `copy:`/`link:` in the check worktree only when setup is approved | **Pass** | `finish-dialog.tsx:118-121`: `setupOk = configApproved(r, cfg)` gates both, and the dialog says when they are skipped (`:252-253`). This is enforced in the UI only; Rust takes the list it is given, which matches the "Rust receives an approved list" design. |
| 4 | New worktree never pre-ticks or approves a changed section | **Pass** | Pre-tick requires `configApproved && checksApproved` (`worktree-dialog.tsx:66`). Changed checks are marked "(new since you approved the checks)" (`:136`). Approval happens only in `create()` with the box ticked, so only for what was displayed (`:77-81`). `createWorktree` and `createAgentWorktree` approve nothing. |
| 5 | Content-based approvals migration | **Pass** | `setupKey = JSON.stringify([setup, tabs, copy, link])` is compared by equality (`worktrees.ts:59`, `:73`), and checks by exact list (`:76-79`). Old values (`hex:len`, from the whole-file hash or the FNV `setupHash`) can never equal a key that starts with `[[`, so each repo is re-approved once and nothing is approved implicitly. There is no hash left to forge (RB-7 closed). |
| 6 | `/dev/null` hooks on Git for Windows, every call path | **Pass** | `git_finish.rs:36`: every `plumb*` call carries `-c core.hooksPath=/dev/null` (commit, commit-tree, update-ref, switch, worktree add, read-tree, config, merge-tree). The calls that skip `plumb` (status, diff, rev-list, merge-base, worktree list, worktree remove) run no hooks. In the daemon, `checkpoints.rs:236-240` adds `NO_HOOKS` to its only `Command::new("git")`. On 2026-10-24 I verified that a native spawn on Git for Windows 2.55 with `/dev/null` gives rc 0 and runs no hooks. The tests cover repo hooks and a husky-style `core.hooksPath`. RB-5 and RB-6 (fallback folder, emptying TOCTOU) are gone with the folder. |

#### FC-1 [Low, non-blocking]: one narrow path can still show "Cancelled. Nothing was changed" after the base moved
- Where: the outer `catch` in `runFinish`, `if (cancelled() && !merged) return stopped();`. `merged` is only set after `finishIntegrate` returns OK.
- Scenario: after the CAS, `read-tree` in the main checkout fails, the CAS undo **also** fails (the base was moved again in those milliseconds), and Cancel was pressed during the merge. Rust correctly returns `Err("… the branch could not be put back: check it by hand")` and does not map it, because `moved` is true. JS then replaces that message with "Nothing was changed".
- This needs three rare events at once, but it is the stated invariant.
- Fix (two lines): set `let integrating = true` before `ipc.finishIntegrate(...)`, and only take the `cancelled()` shortcut in `catch` when `!integrating`. Rust has already mapped every pre-move cancel to `Cancelled`.

#### Low notes (no action needed for this phase)
- The live-pid skip in `prune_stale_scratch` is tested with our own pid only, not with another live process. The code path is the same `processes_alive` the removal already relies on.
- Leftovers are pruned only at the next Finish with checks on the same repository; nothing prunes them at app start. That is acceptable as the minimum.
- Creating a worktree (user action or auto-worktree) still runs the repository's own `post-checkout` hook, through `git worktree add` in `git.rs`. This is parity with plain git and outside Finish.

### Verdict
**Ready to ship.** All four re-review blockers (RB-1 to RB-4) and the recommended RB-5, RB-6 and RB-7 are fixed and covered by tests. The only residual is FC-1 (Low), a two-line fix that can land with this change or right after it.

Status: DONE
Summary: Final check passes on all six points: per-worktree Cancel never sticks and the backend never reports "nothing changed" after the move; stale check worktrees are pruned with their links removed first and live runs left alone; copy/link are gated by setup approval; New worktree never pre-ticks or approves a changed section; approvals are stored as content with a safe one-time re-approval; and every Finish and checkpoint git call uses `core.hooksPath=/dev/null` (cargo 39/39 + 4/4, vitest 185). One Low residual, FC-1: a cancelled run whose post-move undo also fails is shown as "nothing changed" (a two-line JS fix).
