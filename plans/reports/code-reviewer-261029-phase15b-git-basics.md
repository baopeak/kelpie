# Code review: phase 15b Git basics

- Branch: `phase15/git-basics`, commit 7d4978b.
- Diff reviewed: `git diff 31b5c06 7d4978b` (20 files, +1420/−33).
- Reviewer actions: read-only review. I ran focused tests and probed git behaviour in a scratch repository (git 2.55.0.windows.2). I did not run the e2e or any daemon.

## Verdict

The code is well guarded against injection, and it is safe for uncommitted work. **One blocking finding:** a branch switch from a detached HEAD silently orphans commits (H1). The stash compare-before-act check is weaker than the docs claim (M1). Everything else is Low.

| ID | Severity | Blocking | Finding |
|---|---|---|---|
| H1 | High | **Yes** | Switching from a detached HEAD silently abandons commits reachable only from HEAD |
| M1 | Medium | No (fix recommended) | Stash check-then-act is two processes; `apply` can be made race-free, `drop` can verify and restore |
| L1 | Low | No | History (and the switcher's refusal choice) go stale or misleading after a switch |
| L2 | Low | No | A dirty check that runs before the held-branch check offers "Open in a new worktree…" for a held branch |
| L3 | Low | No | Ignored files are overwritten silently by a switch (git parity) |
| L4 | Low | No | Accessibility: click-only section toggles, rows that cannot be focused, Enter with an empty filter switches |
| L5 | Low | No | Non-UTF-8 `i18n.logOutputEncoding` garbles subjects; `%an` ignores the mailmap |
| L6 | Low | No | Remote (SSH): an old host git gives raw errors; "Go to that worktree" never matches; no test coverage |
| L7 | Info | No | A switch runs post-checkout hooks, including one versioned by the target branch through `core.hooksPath` |
| L8 | Info | No | Spec asked to extend `parity.py`; a standalone `git_basics.py` was delivered instead |

---

## H1 (High, blocking): detached-HEAD switch orphans commits

**Where:**
- `app/src-tauri/src/git_basics.rs:186-200`
- `app/src/components/git-panel.tsx:152`: the switcher button is enabled when `st()?.branch` is undefined and shows "…".

**What happens:**
- `git_switch_branch` checks for an operation in progress, a dirty tree and a held branch. It never checks for a detached HEAD.
- `git switch --quiet` suppresses git's own "you are leaving N commits behind" warning. The command succeeds, so stderr is discarded anyway.

**Verified:** in a scratch repository I ran `switch --detach`, made a commit, then ran `git switch --no-guess --quiet main`. The exit code was 0 and there was no output. Afterwards `git branch --contains <orphan>` was empty.

**Failure scenario:**
1. An agent or the user commits on a detached HEAD in a worktree.
2. The user clicks a branch in the switcher.
3. The switch "succeeds" and those commits now live only in that worktree's HEAD reflog.
4. If the worktree is later removed (Finish or Remove), `worktrees/<id>/logs/HEAD` goes with it, and gc eventually prunes the commits.

The codebase already treats this as data loss elsewhere:
- `finish-dialog.tsx:163`: "commits made there would be lost with it".
- `git.rs:362-369`: `git_orphan_commits`.

**Fix (small):** when `status.branch` is `None`, run `rev-list --count HEAD --not --branches --tags --remotes`, the same query as `git_orphan_commits`. If the count is above 0, return a new refusal such as `Orphans { commits }`, with a note telling the user to create a branch first. Add a Rust test with the detached-plus-commit fixture.

## M1 (Medium): stash compare-before-act is not atomic

**Where:**
- `git_basics.rs:239-243`
- `docs/user-guide.md` (Stashes bullet): it says the action "is refused when the list changed since it was shown".

**What happens:**
- `rev-parse stash@{N}` and `git stash <action> stash@{N}` are two separate git processes.
- On Windows the gap between them is tens of milliseconds.
- If another worktree runs `git stash push` in that gap, `stash@{N}` moves to the older neighbour.
- `drop` then deletes a stash the user never confirmed. The confirm dialog came earlier, so it does not narrow this window.
- The window is narrow, and the dropped commit can be recovered from the "Dropped … (<oid>)" line or with `git fsck`. The docs still overstate the guarantee.

**Verified:**
- `git stash apply <oid>` works on git 2.55: it accepts any stash-like commit.
- `git stash drop <oid>` fails with "is not a stash reference", so `drop` needs `stash@{N}`.

**Fixes:**
- **apply:** pass the validated `oid` instead of `stash@{N}`. Apply then has no race and no longer needs the pre-check at all, although keeping it is harmless.
- **drop:** keep the pre-check, then parse the oid out of git's `Dropped stash@{N} (<oid>)` output.
  - If it differs from `oid`, restore it with `git stash store -m <subject> <dropped-oid>` and return an error.
  - Alternatively, reword the docs to "very likely refused" and accept the residual risk.

## L1 (Low): stale views after an action

**History after a switch:** `CommitLog`'s resource is keyed only on `props.project.path` (`git-basics.tsx:98`).
- With History open, a successful switch leaves the log showing the old branch until the user presses Refresh or reopens it.
- The developer report notes the "after a commit" case, but not the switch case.
- Fix: add `gitState.status()?.branch` to the resource source, or bump a version signal from `onSwitched`.

**Sidebar after stash apply:** `act` refreshes `gitState` but not `store.refreshProjectInfo` (`git-basics.tsx:138`).
- The sidebar's dirty marker relies on the fs watcher instead.
- This is minor. `refreshAll` in `git-panel.tsx:63` already exists and could be passed down.

## L2 (Low): refusal order

`refusal()` (`git_basics.rs:167-175`) checks dirty before held-by-another-worktree. Scenario:
1. The tree is dirty and the user clicks `held`.
2. The result is `Dirty`, so the panel offers "Open in a new worktree…".
3. `git worktree add` then fails with git's raw "already used by worktree" error.

Fix: check the held-branch case before dirty, or hide the offer when the branch row has `worktree`.

## L3 (Low): ignored files overwritten silently

`status --untracked-files=normal` does not report ignored files. If an ignored file at the current path (for example `.env` excluded locally) is tracked on the target branch, the switch overwrites it with no prompt.

**Verified:** `secret.env` "MY LOCAL SECRET" became "tracked-on-feature", with exit code 0.

This is standard git behaviour, the same as in a terminal. Accept it and document it, or check `ls-files -o -i --exclude-standard` against `ls-tree <target>`. I would not block on this.

## L4 (Low): accessibility of the switcher and sections

- `SectionToggle` (`git-panel.tsx:244-249`) is a `div` with `onClick`: no `role="button"`, no `tabindex`, no Enter/Space handling. History and Stashes cannot be opened from the keyboard.
- Branch rows (`git-basics.tsx:80-87`) cannot receive focus, there is no arrow-key navigation and there are no listbox/option roles. The only keyboard path is typing a filter and pressing Enter.
- **Enter with an empty filter switches to the most recently committed non-current branch** (`git-basics.tsx:54`). One accidental keypress after opening the switcher changes the branch. This is safe, because only a clean tree can switch, but it is surprising. Consider requiring a non-empty query, or moving a visible highlight with the arrow keys.
- The toggle button has no `aria-expanded`. Focus does not return to it when the switcher closes.
- Escape is handled on the switcher wrapper, so it works only while focus is inside the switcher. `pick` re-focuses the input, which keeps the e2e path working. Nothing else listens for Escape there (checked `app.tsx`, `right-panel.tsx`, `git-panel.tsx`), so the event does not double-fire.

## L5 (Low): encoding and display

- `git_log` and `git_stashes` read stdout with `from_utf8_lossy`. If a user sets `i18n.logOutputEncoding` (for example to latin1), subjects come back garbled. Fix: add `--encoding=UTF-8` to `log`. `stash list` passes through to log, so it takes the same option.
- `%an` ignores `.mailmap`, while `git log` in a terminal shows `%aN`. This is cosmetic.
- **Checked and fine:**
  - The other config probes all came back clean: `log.showSignature=true`, `format.pretty=oneline`, `color.ui=always` (overridden by the `-c color.ui=false` in `run_git`), `color.branch=always` on `for-each-ref`, and `log.date=iso` on `%gd` (it still prints `stash@{0}`).
  - Non-ASCII subjects and branch names survive.
  - CRLF is trimmed (`trim_start_matches(['\r','\n'])` and `trim_end`).
  - Nothing parses git's human-readable messages, so a localized git is safe.
  - A subject that contains `\x1e` would split into a bogus row. This is very unlikely. NUL with `-z` would avoid it.

## L6 (Low): remote (SSH) projects

**Correct as written:**
- Every argument goes through `remote::q` (single-quoted), and `stash@{N}` is not expanded.
- `operation()` has a remote branch.
- `git_worktree_env` skips the lock for remote projects.
- The worktree offer is hidden through `canWorktree`.

**Gaps:**
- A host with git older than 2.23 has no `switch`. One older than 2.10 has no `log --no-show-signature`, and some very old versions lack `committerdate:unix`. Each of these surfaces a raw git error.
- `projectAt(path)` compares a host path with `ssh://…` project roots, so "Go to that worktree" never appears for remote projects.
- None of these paths is exercised by a unit test or the e2e. The developer report says so.

## L7 (Info): hooks

`git switch` and `git worktree add` (existing mode) run `post-checkout`. With `core.hooksPath` pointing into the tree (husky), that is the target branch's own script. This matches `git.rs`'s stated policy ("same behaviour as the user's terminal, hooks included"), but differs from the Finish path's deliberate no-hooks stance (`git_finish` tests `no_hook_runs_wherever_it_is_configured` and `the_branchs_own_config_and_programs_never_run`). This needs a product decision, not a code fix.

## L8 (Info): spec deviation

The phase file (line 29) says "15b: extend `parity.py` git section (PowerShell-safe)". A focused `scripts/e2e/git_basics.py` was delivered instead. The reason given is that `parity.py` needs the release daemon and real agents, which is reasonable, but the plan owner should accept it explicitly.

**Isolation check of `git_basics.py`:**
- It sets `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR` in its own process before `kelpie-call` is loaded (lines 46 and 69).
- It drops the inherited session variables.
- Teardown only kills processes it started.

---

## Verified OK

- **Option and revision injection:**
  - `valid_branch` (`git_basics.rs:26-37`) rejects a leading `-`, `..`, `@{`, `~^:?*[\`, whitespace, control characters, `.`-prefixed components, `.lock` components, and `@`.
  - `require_local_branch` then requires `show-ref --verify refs/heads/<name>`.
  - Stash refs must be exactly `stash@{\d{1,6}}`, oids must be 40 or 64 hex characters, and the action is limited to `apply`/`drop`.
  - Tests cover `--orphan=x`, `-`, `main..feature`, `HEAD~1`, tags, `--all` and `pop`.
- **Race between the dirty check and the switch (TOCTOU), uncommitted work:**
  - The dirty check runs outside the lock, but `git switch` runs without `-f` or `--discard-changes`.
  - It refuses when a tracked change or a conflicting untracked file would be overwritten, and carries over non-conflicting changes.
  - So no path loses uncommitted work: git itself is the backstop. The worst case is a "switched" result with changes carried along, which breaks the clean-only rule but loses nothing. H1 is about committed work.
- **Root guard:** the same lexical `fs_ops::guard(&root, "")` as every existing git command, so there is no new exposure.
- **`git_worktree_add` existing callers:**
  - `existing: Option<bool>` deserializes to `None` when absent.
  - The JS side passes `opts.existing || undefined`.
  - The new-branch path is byte-identical: `requested || undefined` equals the old `opts.base?.trim() || undefined`.
  - `baseBranch === opts.branch → undefined` is only reachable in existing mode, because `-b` would fail otherwise.
  - The only other caller, `parity.py:244`, is unaffected.
  - vitest asserts both call shapes.
- **Credentials:** none of these commands touch a remote, and errors are git's own stderr or fixed messages.
- **i18n:** every new `t()` key is in `locales/vi/git.json`. "Refresh", "Loading…" and "Apply" already existed. The three backend messages are in `backend.json`, and "invalid branch name" was already there.
- **Solid:**
  - Resources are owned by their components and are not under any `Suspense`; the only `Suspense` blocks are in settings, so loading the lazy views never blanks an ancestor.
  - There are no timers or listeners, so nothing leaks.
  - After a switch, `refreshAll` updates status, repo state and the sidebar's project info, and the PR status is refreshed too.
- **Lazy loading is real:**
  - The `dist/` built at 20:05 for this commit has `git-basics-*.js` (7.5 kB) and `.css` as separate chunks. Neither is in `index.html`'s modulepreload list.
  - The chunk imports only chunks that already load at startup: index, i18n, ipc, platform, store, web.
- **Bundle:**
  - Startup growth is the small `git-panel` and `worktree-dialog` changes, plus `vi/git.json` in the eagerly loaded i18n chunk.
  - I found no hidden split. The preload list holds the same 17 module chunks the developer describes.
  - The gate's entry-only measurement is a weakness that predates this change. Follow-up: sum the chunks that `index.html` preloads.

## Tests run

| Check | Result |
|---|---|
| `cargo test -p kelpie -j 2 git` (CARGO_TARGET_DIR=D:\kelpie-p9-target, CARGO_INCREMENTAL=0) | 54 passed, 0 failed, 1 ignored (includes the 12 `git_basics` tests) |
| `pnpm exec vitest run src/lib/git-basics.test.ts src/lib/worktrees-create.test.ts` | 10 passed |
| Scratch git probes | detached-HEAD orphaning (H1), `stash apply <oid>` works and `drop <oid>` fails (M1), ignored-file overwrite (L3), hostile config parsing (L5) |

## Test gaps worth adding

- Rust: switch from a detached HEAD with an orphan commit is refused (after the H1 fix).
- Rust: drop when the oid printed by git differs from the expected one, restored (if M1 is fixed that way).
- Rust: dirty tree plus held target gives `InWorktree` (after the L2 fix).
- Rust: `git_worktree_add(existing)` on a branch held elsewhere gives a clear error.

## Unresolved questions

1. Should a switch from a detached HEAD with no orphan commits stay silent? I suggest yes; refuse only when the orphan count is above 0.
2. Should Kelpie-initiated switches suppress hooks, like Finish does (L7)? This is a product call.
3. Does the plan owner accept the standalone e2e instead of the `parity.py` extension (L8)?
