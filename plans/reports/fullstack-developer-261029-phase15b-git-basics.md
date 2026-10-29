# Git basics in the Changes panel: branch switcher, compact log, stash list

Branch `phase15/git-basics` (based on the processes/ports branch, PR #5). Everything runs in the desktop
bridge. No daemon change, no API level change, no polling.

## What was built

**Native side: `app/src-tauri/src/git_basics.rs`** (new; registered in `lib.rs`)
- `git_branches`: local branches, sorted by most recent commit (`for-each-ref`, limit 500). Each has
  `current` and `worktree` (the folder of another worktree that has the branch checked out, taken from
  `git worktree list`).
- `git_switch_branch` returns `switched`, `dirty {files}`, `in_worktree {path}` or `operation {operation}`.
  - It refuses before git runs, and changes nothing, when:
    - a merge, rebase, cherry-pick or revert is in progress;
    - there are uncommitted changes (staged, unstaged or untracked);
    - the branch is checked out in another worktree.
  - Otherwise it runs `git switch --no-guess --quiet <branch>` under the repository's existing
    cross-process worktree lock.
- `git_log`: the last 100 commits of HEAD (`%H %h %an %at %s`, `--no-show-signature`). A repository with
  no commits returns an empty list, not an error.
- `git_stashes`: `stash list` with `%gd %H %ct %gs`.
- `git_stash_action` (`apply` | `drop`): only accepts `stash@{N}` together with the commit id that the
  list showed. It is refused when `stash@{N}` now points somewhere else, which can happen because the
  stash is shared across worktrees and the indexes shift.
- Input guards:
  - Branch names must pass `valid_branch`, a strict `check-ref-format` subset: no leading `-`, no `..`,
    `@{`, `~^:?*[\`, whitespace or control characters, and no `.`-prefixed or `.lock` components.
  - The branch must also exist under `refs/heads/` (`show-ref --verify`). A tag or a revision
    expression is rejected.
  - Stash refs must match `^stash@\{\d{1,6}\}$`. The commit id must be 40 or 64 hex characters.
  - The project root goes through the existing `fs_ops::guard`.
  - Every call goes through `git::git` / `git_worktree_env`: concurrency slots, `CREATE_NO_WINDOW`,
    `kill_on_drop`, and remote projects run over SSH.
- `git_worktree_add` takes a new optional `existing` flag. With it, the worktree checks out an existing
  local branch (`worktree add -- <path> <branch>`, after the `refs/heads` check) instead of creating one
  with `-b`. Callers that leave the flag out behave as before. `git::git_ok` is now `pub(crate)`.

**UI**
- `git-panel.tsx`:
  - The branch name at the top is now a button that opens the switcher.
  - "History" and "Stashes" are folded sections at the bottom; one is open at a time.
  - The three views are `lazy()` imports of `components/git-basics.tsx` and its CSS, which load in a
    separate chunk.
  - The worktree offer is passed in as a callback. This keeps the lazy chunk from importing
    `worktree-dialog`, which had made Rollup split `worktrees` and `yaml` out of the entry (details under
    Bundle).
- `git-basics.tsx`:
  - The switcher has a filter box, the current branch first, a *worktree* chip on held branches, and Enter
    picks the first match.
  - A refusal shows a note:
    - uncommitted changes: *Open in a new worktree…*;
    - branch held by another worktree: that folder, plus *Go to that worktree* when it is in the sidebar;
    - operation in progress: finish or abort it first.
  - The log shows short hash, subject, author and relative age, with the full hash and date in the
    tooltip. It loads when opened and has a Refresh button.
  - The stash list shows the warning "shared by every worktree", *Apply* (keeps the stash) and *Drop*
    (`confirmDialog` first).
- `worktree-dialog.tsx` has an existing-branch mode, `openWorktreeDialog(repo, branch, then, { existing: true })`:
  - the title becomes "Open {branch} in a new worktree of {repo}";
  - the Branch field is read-only;
  - there is no "From" row, and `.kelpie.yaml` is read at that branch.
- `createWorktree` passes `existing` through. In that mode it skips the strict new-branch name regex,
  because the native `refs/heads` check validates instead.
- Pure helpers are in `lib/git-basics.ts`: `filterBranches` and `parseStashSubject`.
- i18n: 24 keys in the new `locales/vi/git.json`, plus 3 native error messages in `backend.json`.
- Docs:
  - `docs/user-guide.md`: three new bullets on the Changes panel under "Files".
  - `docs/architecture.md`: a `git_basics.rs` row, and the `git.rs` row now mentions existing-branch add.

## Tests

| Check | Result |
|---|---|
| `cargo test -p kelpie -j 2` | 181 passed, 0 failed, 5 ignored (12 new in `git_basics`; they take 9 s, using `fast-import` for the 105-commit fixture) |
| `cargo clippy -p kelpie --all-targets -j 2` | clean, no warnings (`kelpied` not touched, so not run) |
| `pnpm typecheck` | ok |
| `pnpm test` | 42 files, 468 tests passed (new: `git-basics.test.ts` 5 tests, `worktrees-create.test.ts` +3 for existing-branch mode) |
| `pnpm i18n:check` (script) | 0 missing, 0 problems, 0 untranslated-looking |
| `pnpm build` | ok; entry `index-*.js` 325.67 kB gz |
| `perf_gate.py --metrics bundle` | entry 325.67 kB vs 325.05 before (+0.62); limit 338.60; `plans/reports/perf-git-basics.json` |
| GUI e2e `scripts/e2e/git_basics.py` | 27/27 on two consecutive runs of the final build (earlier builds: 26/26 three times) |

What the Rust tests cover:
- Parsing: branches with worktree mapping, log including empty subjects, stashes including junk lines.
- Name guards: 24 bad branch names, 8 bad stash refs, commit ids.
- Against temp repos:
  - a clean switch, including "already on it";
  - dirty refusal for modified, staged and untracked files, with HEAD and the file content unchanged;
  - refusal for a branch in another worktree, plus `git_branches` marking it;
  - refusal during a merge;
  - unknown names, option-like names and tags rejected;
  - log capped at 100, and an empty repository;
  - stash apply keeps the stash, drop removes the right one, a stale commit id is refused, `--all` and
    `pop` are refused;
  - existing-branch `git_worktree_add`: an unknown branch is refused, a real one is checked out.

What the e2e covers:
- clean switch there and back;
- dirty → refused and nothing moves → *Open in a new worktree…* → dialog in existing mode → the
  worktree is created on `feature` and the main checkout is untouched;
- held branch → refused with the `held-wt` folder;
- Escape closes the switcher;
- History shows exactly 100 rows (121 commits in the repository), newest first, with hash, author and age;
- Stashes: warning shown, apply keeps the stash, drop asks with the stash named, Cancel keeps it, OK
  drops only that one.

How the e2e is isolated:
- It runs its own `kelpied serve` with endpoint `kelpie-git-basics-<pid>` and a temporary
  `KELPIE_DATA_DIR`, both set in the script's own process.
- It starts its own `tauri-driver` on a free `--port` and a free `--native-port`.
- The repositories are local, with no remote and no network.
- Teardown quits the app, kills the driver tree it started, shuts its own daemon down, removes the
  endpoint token and deletes its folders. Git's read-only object files are handled.
- After each run, the only Kelpie processes left were the user's own (installed app and daemon, plus a
  `D:\Works` dev daemon); none were touched.

How the e2e was run:
- It was not added to `parity.py`, because that suite needs the release daemon and real agents. It is a
  focused script following the `processes.py` pattern.
- Debug binaries built from this branch were copied to a scratch folder so that a parallel build into the
  shared target folder could not swap them mid-run.
- Command:
  `python -X utf8 scripts/e2e/git_basics.py <shots> <scratch> <dir>\kelpie.exe`, with
  `MSEDGEDRIVER=D:/Works/edgedriver/msedgedriver.exe` and `~/.cargo/bin` on PATH.

## Bundle note (worth knowing for later lazy chunks)

The first version of the lazy module imported `worktree-dialog` and `icons`. Rollup then moved
`worktrees` (with the `yaml` parser), `key-hints` and `icons` out of `index-*.js` into separate chunks
that still load at startup.
- The gate's `entry_gz_kb` fell to 287 kB, a false win: the JS loaded at startup actually grew from
  598.5 to 600.6 kB gz.
- After the callback change, no chunk loaded at startup is new or missing compared with before, and
  the startup JS total is +1.4 kB gz.
- Follow-up: the gate measures only `index-*.js`. It could add up every chunk `index.html` preloads so
  that such splits cannot hide growth. Not changed here.

## Follow-ups and limits

- Remote (SSH) projects: switch, log and stash work through the existing remote git path, but the
  worktree offer is hidden there, as New Worktree is. Not exercised by e2e (no SSH fixture was used).
- Untracked files count as "dirty", which is stricter than git itself: git would carry them over.
  This was chosen so an agent's new files never follow onto another branch. Revisit if users find it
  too strict.
- Relative ages come from the existing `formatRelative`, which shows old commits as "1,080 days ago",
  not in years. That is existing helper behaviour.
- The log does not refresh by itself after a commit while it is open (no polling, by design); Refresh
  or reopening reads it again.

Status: DONE
Summary: Branch switcher (clean-only, with worktree offer and held-branch refusal), 100-commit log and stash apply/drop shipped as desktop-only commands with input guards, unit, vitest and isolated GUI e2e coverage (27/27 twice); entry bundle +0.62 kB.
