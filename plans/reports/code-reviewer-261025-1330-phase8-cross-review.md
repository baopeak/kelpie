# Code review: Phase 8, cross-review and best-of-N

Date 2026-10-25 · branch rebrand/kelpie @ efff847 (uncommitted) · reviewer: code-reviewer · report only, no source edits.

## Scope
- Phase 8 files and hunks from the developer report: `kelpie-proto/src/materialize.rs`, `kelpied/src/{notes,auth,cli,runs,daemon(notes/run hunks),main(3 hunks)}.rs`, `app/src-tauri/src/worktree_files.rs`, app `lib/{store,runs,worktrees,review-notes,review-with,try-agents,run-compare,review-facts,review-queue,send-to-agent,quick-commands}.ts`, components `{run-compare,try-dialog,review-with-dialog,review-card,runs-page,finish-dialog,diff-view}.tsx`, docs hunks.
- Not reviewed: kelpie-46 (mobile/session/chat, the `session.answer` arm), kelpie-85 test hunks, the usage-meter work, `.claude/agent-memory`.
- What I checked by running it:
  - `pnpm typecheck` clean.
  - vitest for the 6 new test files: 40/40.
  - `i18n:check`: 0 missing.
  - `cargo test -p kelpie-proto materialize`: 3/3.
  - `cargo test -p kelpied -- notes auth run_worktree five_run`: 14/14.
  - Two scratch probes, both deleted afterwards:
    - serde: does `NewRun` deserialize from a JSON array? Yes.
    - `materialize.rs` on overlapping `link:`/`copy:` entries.

## Overall
The basic design holds up:
- Tasks start without double-starting: `Runs::step` flips Pending to Starting while it holds the runs lock.
- Notes git calls go through the hardened `checkpoints::command`: hooks off, `GIT_LITERAL_PATHSPECS=1`, `LC_ALL=C`, fsmonitor off.
- A session's `path` is overwritten with its own folder.
- Review-with goes through `startWithPrompt`, which runs `redact` and then the daemon's first-prompt path. A prompt longer than 8 KB is typed through the daemon's sanitiser.
- Try-with-N sends N `worktree: true` tasks with no dependencies and adds no `group` field.

The problems are at the edges:
- The new session scoping is skipped when `params` is not a JSON object.
- Removing the other candidates uses `--force` and `branch -D`, and runs an unbounded time after the confirmation.
- Notes are cleared after the folder is gone, so the key can resolve to a different worktree.
- Moving `materialize` changed its behaviour when a copied path and a linked path overlap.

## Critical
None.

## High

### H1. Non-object `params` skip `scope_session_params`: a session can read every worktree's notes and keep `materialize` on `run.create`
- **Where:** `crates/kelpied/src/auth.rs:174`, plus `crates/kelpied/src/daemon.rs:284-286` and `daemon.rs:639`.
- **The bypass:**
  - `let Some(obj) = params.as_object_mut() else { return Ok(()) };` lets anything that is not an object through untouched.
  - `server.rs:162` turns missing `params` into `Value::Null`.
- **Failure 1, notes from every worktree:**
  - A session program (scripting or restricted) sends `{"method":"note.list"}` with no params, or with `[]`.
  - `note_rpc` reads `path` as empty and returns `self.notes.lock().all()`: every worktree's notes, with file paths and quoted lines.
- **Failure 2, `materialize` kept on `run.create`:**
  - A scripting session sends `run.create` with params as an array. I verified that serde_json deserializes a derived struct from a JSON array: `["r","", "C:/repo", null, [[…task…]], null, [[".env"],["node_modules"]]]` gives `materialize: Some(...)`.
  - The strip never runs, so a run made by an agent gets copies and links that were never approved.
- **Impact:** this is the exact boundary the phase claims. auth.rs says same-user code is not a hard boundary, but here no deliberate escape is needed: one malformed request is enough.
- **Fix:**
  - For `Role::Session`, treat `Null` params as `{}`.
  - Reject any other non-object params for the scoped methods before `scope_session_params`, e.g. `bail!("{method}: params must be an object")`.
  - Defence in depth: `note_rpc` returns `all()` only to callers that are not sessions. Pass the role or a `by_session` flag, or make `note.list` without a path a separate method that only the user may call.
  - Add an auth test with `Value::Null` and an array for `note.list` and `run.create`.

### H2. "Use this one" and "Discard…" remove candidates with `--force` and `git branch -D`, possibly long after the confirmation
- **Where:** `app/src/lib/run-compare.ts:164-173` and `run-compare.ts:180-199`. The docs record this as intended: `docs/architecture.md:733`.
- **How the timing works:**
  - The confirmation lists the losers from the last facts read, which may say "changes not read".
  - The losers keep running. `task_skip` is only sent inside the after-Finish callback.
  - Finish can take minutes (WIP commit, merge, approved checks). It can also stop on a conflict, be resolved with an agent and be re-run later from the same dialog. The callback stays in `afterFinish` for all of that.
- **Failure:**
  - A loser's agent keeps working: it commits or writes files after the user confirmed.
  - Or the user opens a loser to borrow code from it.
  - When the winner's Finish succeeds, `removeWorktree(wt, { force: true })` deletes the uncommitted work and `gitBranchDelete(..., true)` drops commits the user never saw.
  - Phase 7's rule was "No `--force`" (completion-261024-1800, line 8). The brief asks for the phase 7 safe removal.
- **Fix:**
  - At confirm time, stop the losers' tasks right away (`task_skip`), so nothing changes after the snapshot.
  - Record each loser's `HEAD` sha and its `git status` file list.
  - In the callback, run `checkRemoval` and `rev-parse HEAD` again. Remove with force, and delete the branch with `-D`, only if both still match the snapshot. Otherwise keep that candidate and say why.
  - Simpler and safer alternative: remove the worktrees but keep the branches. Unmerged commits then stay reachable, and branch deletion becomes its own explicit "Discard…".
  - Also make the end notice honest: it says "removed {n}" even when some removals failed (`run-compare.ts:198`). Count only the ones that succeeded.

### H3. `removeWorktree` clears notes after the folder is deleted: the key resolves upward and can wipe another worktree's notes
- **Where:** `app/src/lib/worktrees.ts:259-260`, plus `crates/kelpied/src/notes.rs:48-51` and `checkpoints::find_top`.
- **The cause:**
  - `store.removeNotes(wt.id, "all")` runs after `gitWorktreeRemove`.
  - By then `worktree_key(path)` finds no `.git` in the deleted folder.
  - `find_top` walks `ancestors()` up to the first folder that has a `.git`.
- **Failure:**
  - A worktree nested in its repository, such as Claude Code's `.claude/worktrees/<name>`, is adopted through `addExistingWorktrees`.
  - Finishing or removing it then sends `note.clear` with no ids for the main repository's key, which wipes the main checkout's notes.
  - The same happens with the sibling `<repo>-worktrees/` layout when any ancestor is a git repo, such as a dotfiles home directory.
  - When no ancestor is a repo, the canonical key and the raw key can differ: 8.3 names like `BAOPEA~1`, subst drives, symlinks. The notes are then left in `review-notes.json` for good.
- **Fix:**
  - Clear notes before `gitWorktreeRemove`, while the folder still exists, or send the daemon key the app already holds (`canonKey(wt)`).
  - In the daemon, `note.clear` and `note.add` should not walk ancestors when `path` does not exist. Use `meta::key(path)` and match the key exactly.

### H4. `materialize` changed behaviour when it moved: an overlapping link and copy now copy a file onto itself (data loss on macOS and Linux)
- **Where:** `crates/kelpie-proto/src/materialize.rs:146-182`, line 164 (`plan` runs the "already in the worktree" check before anything is linked). `docs/architecture.md:709` says "behaviour unchanged".
- **What changed:**
  - Before: each entry was checked right before it was brought in. After `link: [deps]`, a copy of `deps/x` found `dst` already there (through the junction) and was skipped. A copy of `deps` with `link: [deps/inner]` found `wt/deps` created by `link_dir` and was skipped too.
  - Now: every check runs against the fresh checkout before anything is linked. So `copy_tree(repo/deps/x, wt/deps/x)` writes through the junction onto the source file.
- **What I measured:** the scratch probe, run with the real file, on Windows:
  - Both overlap cases fail with "os error 32" (sharing violation). The case `copy: deps` + `link: deps/inner` copies part of the tree first.
  - The repository's files survived.
- **On macOS and Linux:** `std::fs::copy` opens the destination with `O_TRUNC`, which truncates the source file, since source and destination are the same file. That means data loss in the main checkout, e.g. `.env` or a local config under a linked folder.
- **Affected paths:** both callers, the app's worktree dialog / auto-worktree and run worktrees.
- **Fix:**
  - In `materialize`, re-check `worktree.join(&rel).exists()` (or `symlink_metadata`) right before each item, as the old code did.
  - Also drop copy entries that sit at or under a linked entry, and the other way round, during `plan`.
  - Add a test for `link: [deps]` + `copy: [deps/x]` and for `copy: [deps]` + `link: [deps/inner]`.

## Medium

### M1. `notesForKey` prefix fallback shows a parent worktree's notes in nested worktrees and sends them to the wrong agent
- **Where:** `app/src/lib/review-notes.ts:13-17`, `store.ts:570-573`.
- **The cause:** the daemon keys notes by the folder holding `.git`. The app takes the longest key that is a prefix of the project key.
- **Failure:**
  - A worktree at `repo/.claude/worktrees/x` has no notes of its own, so it shows the main repo's notes, the user's or an agent's, in its diff and review card.
  - *Request changes* (`review-card.tsx:29`) then sends those notes to the worktree's agent and calls `note.clear` on the worktree key, so the main repo's notes stay.
  - The nested worktree's view switches to its own notes as soon as it gets one.
  - The same applies to a repo nested in a dotfiles home repo.
- **Fix:**
  - Use prefix matching only for a project whose own folder has no `.git`, i.e. a subfolder project. `projectInfo` already knows the repo root.
  - Better: have the app look up the key for the project once (`worktree_key`, e.g. returned by `note.list {path}`) and match it exactly.

### M2. *Request changes* clears all notes, including ones that arrived after the prompt was built
- **Where:** `app/src/components/review-card.tsx:26-33`. Also "Resolve all" at `diff-view.tsx:213`.
- **Why this is new:** notes now have writers working at the same time, such as a reviewing agent running `kelpied review add`.
- **Failure:**
  - The `notes` snapshot is taken when the user clicks.
  - The user then picks a target in the menu while the reviewer adds a note.
  - `removeNotes(p.id, "all")` deletes the new note, which was never sent.
- **Fix:** `removeNotes(p.id, notes.map((n) => n.id))`.

### M3. `notes.changed` goes to every connection, including session and anonymous ones
- **Where:** `crates/kelpied/src/daemon.rs:331-333` → `broadcast` (`daemon.rs:757-763`); `server.rs:55` registers clients before `auth`.
- **Failure:** any program in a session, restricted or not, opens a connection and keeps it, e.g. `kelpied terminal wait`. It receives every worktree's notes as they change, which defeats the own-worktree scoping on `note.list`.
- **Context:** `run.updated` and `meta.updated` already behave this way, but notes are new data that the phase scopes explicitly.
- **Fix:**
  - Tag clients with their role in `register_client`, set when `auth` succeeds.
  - Send `notes.changed` only to `Role::User` clients, or only to sessions whose own key matches.

### M4. The Runs page's "New run" editor still creates run worktrees without `copy:`/`link:` and shows no size estimate
- **Where:** `app/src/components/runs-page.tsx:166-170`. Only `try-agents.ts:88` sends `materialize`.
- **Spec:** "Run worktrees materialise `.kelpie.yaml` `copy:`/`link:` … size estimate shown before start". This covers run worktrees in general, and it is the RT-6 gap "run worktrees lack `link:`".
- **Failure:** a hand-written or AI-planned run with `worktree: true` tasks gets clean checkouts, with no `node_modules`.
- **Fix:**
  - Reuse `planBringing` and `bringingText` in the editor whenever any task has `worktree: true`.
  - Send `materialize` from it.

## Low
- **L1. `.worktreeinclude` is copied even when `.kelpie.yaml` is unapproved.**
  - Where: `try-agents.ts:95` always sends `materialize: {copy: [], link: []}` (`Some`), so the daemon's `materialize` still copies `.worktreeinclude` entries.
  - This matches the worktree dialog, but the e2e/docs claim "unapproved → clean checkout" is only true without that file.
  - Agent runs get nothing, not even `.worktreeinclude`.
  - Either document it, or send `materialize: undefined` when there is nothing approved.
- **L2. One bad entry fails the whole batched `check-ignore`.**
  - Where: `materialize.rs:56-80`. Example: an entry past a tracked symlink makes git `die`, so every entry is skipped as "not ignored". This fails closed, but it is worse than the old per-entry runs.
  - The command also does not use the hardened environment: `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE` are inherited from the daemon.
  - Clear those variables, as `checkpoints::command` does.
- **L3. Quoting from the file is looser than the docs say.**
  - Where: `notes.rs:127-130`. Only the last path component is checked with `symlink_metadata`; a junction or symlink folder in the middle is followed. `docs/architecture.md:696` says "never a symlink".
  - `clean_file` (`notes.rs:64`) rejects `.git` but not `.GIT` on Windows.
  - Impact is small, since the agent can read those files anyway.
  - Fix: canonicalize and check the result with `starts_with(top)`, and compare `.git` without case.
- **L4. i18n and duplication.**
  - The daemon's `brought` text ("linked …, not brought: … (reason)") is shown raw on task cards (`runs-page.tsx:57`), English only.
  - `STATUS_LABEL` is copied from `runs-page.tsx` into `run-compare.tsx:21`.
  - Send structured `brought` data, or translate it in the app; export one `STATUS_LABEL`.
- **L5. `adoptWorktrees` can re-add a removed project.**
  - Where: `runs.ts:138`. A folder that is still there after removal (`removeWorktree` returned `leftover: true`) passes `isDir`, so the next `run.updated`, e.g. the winner's `task_done`, adds a project for a half-deleted folder whose branch is gone.
  - It also calls `isDir` over IPC for every removed task on every `run.updated`.
  - Keep a set of the paths removed in this session, or mark discarded tasks in the run.
- **L6. Migration edge cases.**
  - A note the daemon refuses forever (e.g. an old file path it rejects) makes a new `app-state.notes-*.bak.json` on every app start, and nothing prunes them.
  - Notes of a project whose folder is gone are filed under an ancestor key (same cause as H3).
  - Skip folders that do not exist, and back up at most once per state version.
- **L7. A timing assertion in the unit suite.**
  - Where: `daemon.rs:1904` asserts that 5 worktrees take less than 3 s, inside `cargo test`.
  - It is flaky under machine load (builds, Defender). Keep the measurement in the perf gate or e2e, and make the unit test check correctness only.
- **L8. Agents that strip `*TOKEN*` variables cannot add notes.**
  - Such an agent (opt-in hardening in Codex, whose current default no longer filters) runs as `Claimed`, so `review add` is refused with a clear message. The review prompt still tells it to use the command.
  - Mention this in the user guide.
- **L9. A misleading error on an unborn branch.**
  - Where: `create_task_worktree` (`daemon.rs` near line 1742). Adding `--abbrev-ref HEAD` makes `rev-parse` fail on an unborn branch, which is then reported as "needs the run folder to be a git repository".
  - The outcome is the same as before (the worktree add would fail anyway); only the message is wrong.

## Edge cases checked and fine
- Double start: an immediate tick from `dispatch` racing the 1 s ticker cannot start a task twice (`runs.rs:409-425`, atomic Pending to Starting). `Handle::try_current()` works inside the `spawn_blocking` worker.
- Path tricks on notes (case, `..`, junctions, UNC): the session's `path` is replaced by its spec cwd, so they cannot reach another worktree, provided H1 is fixed. File paths reject `..`, absolute paths and `:` (alternate data streams).
- Git in `notes::locate`: `--no-ext-diff --no-textconv -U0 <ref> -- <file>`; the ref is limited to safe characters and cannot start with `-`; the environment is hardened.
- Automations and mobile verbs: neither creates orchestration runs, so they cannot reach `materialize`.
- An older daemon: Try works with clean worktrees (the unknown field is ignored); notes stay local; review-with leaves out the `review add` instructions.
- Lazy chunks for compare, try and review-with. The compare timer runs only while a candidate is still running. No plan IDs in code, test names or comments.
- Finish's `finishSucceeded` fires only on `kind === "finished"`, i.e. merged and worktree removed. "Merged, but not removed" does not trigger the discard.

## Recommended actions (in order)
1. H1: normalise or reject non-object params for sessions, and stop returning `note.list` without a path to sessions.
2. H2: snapshot the losers and stop them at confirm; remove only if nothing changed; keep the branches, or delete them only when the tip matches.
3. H4: restore the per-item existence check in `materialize`, drop overlapping entries, and add tests.
4. H3: clear notes before the worktree is removed, using the exact key, and stop walking ancestors for missing paths.
5. M1, M2, M3, M4, then the Lows.

## Metrics
- Typecheck: clean. Lint/clippy: not re-run (the developer reports clean). vitest (new files): 40/40. i18n: 0 missing. Cargo (filtered): 17/17.
- Test gaps: non-object params on scoped methods; overlapping link/copy; nested-worktree note keys; removal after the folder is gone.

## Unresolved questions
- Was force removal of the losers explicitly accepted by the user, or chosen by the developer? The docs record it as intended (`architecture.md:733`). If the user accepted it, H2 comes down to the race (losers still running after confirm) plus keeping the branches.
- Should `.worktreeinclude` count as "approved by the user" for run worktrees (L1)?

Status (first pass): DONE_WITH_CONCERNS
Summary (first pass): four High findings: params-shape bypass of session scoping, forced loser removal, notes cleared after the folder is gone, and `materialize` overlap self-copy.

---

## Re-review (2026-10-25, after the developer's fix round)

Controller decisions applied as the yardstick:
1. Losers are stopped at confirm, WIP-committed only if unchanged, removed without force, and their branch deleted only with `branch -d`.
2. `.worktreeinclude` counts only when setup is approved.

### What I ran
- An end-to-end probe against an isolated daemon of my own:
  - It used its own `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR`, removed any `KELPIE_SESSION_*` variables from its environment, used fixture repos in scratch, and shut down with `kill_sessions`.
  - A real session-role caller (its id and token read from a `custom` session's environment) sent `null`, `[]`, `"x"`, `7`, an array, and an object naming another repo to `note.list`, `note.add`, `run.create`, `note.keys` and `note.clear`.
- A scratch crate that includes the current `materialize.rs` directly, covering 4 overlap cases.
- `pnpm typecheck` clean; vitest 355/355 (36 files); i18n 0 missing; `cargo test -p kelpie-proto` 19 pass; `cargo test -p kelpied -- notes auth run_worktree five_run client` 17/17. I did not re-run clippy or the e2e suite.
- All scratch files are deleted. The one `kelpied.exe` still running is your installed daemon (`AppData\Local\Kelpie\daemon\a`, started 2026-10-24). I didn't touch it.

### Verdict per finding

**H1: Fixed (verified end to end).**
- A session with `null` params is treated as `{}`. `note.list` then returns only that session's own worktree (`[]`). The user's note in another repo never leaked in any of the 30 combinations.
- Arrays, strings and numbers are refused with "params must be an object", for every scoped method (`auth.rs:142-145`, `object_params`). The check runs for Claimed callers too.
- `note.list` with no folder is user-only, guarded twice: `by_session` is checked in `note_rpc` (`daemon.rs:288-291`).
- `note.keys` and `note.clear` are not in `SCRIPTING`, `OWN` or `CLAIMED`, so a session gets "not available".
- Session `run.create`: an object with `materialize` gave a run whose `materialize` is `None`; the array form is refused.
- An object naming another worktree's `path` is refused.
- A session's own `note.add` still works: `by: "custom"`, side and text filled from the diff.

**H2: Fixed, and it matches decision 1** (`app/src/lib/run-compare.ts:184-283`).
- **Stop and wait:** `stopAndSnapshot` sends `task_skip`, then `stopSessions(wt)`, which closes and waits up to 10 s for the PIDs to exit. If they don't stop, it returns `null` and the candidate is kept.
- **Record:** the snapshot holds `git_head`, the branch, and `finishInspect` uncommitted entries as sorted `status path size`.
- **Identical detection:** `sameSnapshot` compares the head, the branch and the file list.
- **WIP commit:** done only when the snapshot is unchanged. It uses phase 7's `finish_wip_commit`, which commits exactly the status files with literal pathspecs, uses `--no-verify`, and refuses on extra files, embedded repos or conflicts.
- **Removal:** `removeWorktree(..., { force: false })`, then `gitBranchDelete(..., false)`, i.e. `branch -d`. An unmerged branch, which includes any WIP commit, stays.
- **A changed loser is kept with a reason:** "it changed after you confirmed", "could not be stopped", "detached HEAD", or git's own error. Reasons go to an Activity entry and a toast; the toast counts what was removed and what was kept.
- No `force: true` is left anywhere in the compare code. *Discard…* uses the same path.
- Remaining nit (Low, not blocking): "identical" compares file size, not content. A same-size edit after confirm counts as unchanged. It still lands in a WIP commit on a branch `-d` won't delete, so nothing is lost. A content hash, or including mtime, would make the "left alone if changed" promise exact.

**H3: Fixed (verified by reading the code).**
- `removeWorktree` clears notes before `gitWorktreeRemove` (`worktrees.ts:243-245`).
- `notes::worktree_key` returns `meta::key(dir)` for a path that does not exist, with no upward search (`notes.rs:48-53`).
- There is a Rust test and an e2e step for a nested `.claude/worktrees/x`.
- New side effect (Low, see R2): if the removal then fails, the worktree stays but its notes are already gone.

**H4: Fixed (verified with the real file).**
- The 4 cases, all with source files intact:
  - `link deps` + `copy deps/local.txt`: the copy is skipped as "already in the worktree".
  - `copy deps` + `link deps/inner`: `deps` is skipped.
  - `copy deps/inner, deps`: `deps` is skipped, as the old code did.
  - Disjoint entries: brought normally.
- `estimate` gives the same selection as `materialize` in every case.
- Each item's destination is checked again with `symlink_metadata` right before it is brought.
- The check order is the same as in the old `worktree_files.rs` (clean, dedupe, source exists, destination exists, ignored, folder-only for links, budget).
- The batched `check-ignore` falls back to one run per entry when it fails, and clears `GIT_DIR`, `GIT_WORK_TREE` and `GIT_INDEX_FILE`.
- One edge differs from the old code (Low): an overlap is decided at plan time, so an earlier entry that fails at runtime (a failed `mklink`, or a copy skipped for size) still blocks a later entry inside it. The old code would have brought the later entry. This fails closed and is harmless.

**M1: Fixed.**
- `notesForKey` matches exactly.
- The app asks `note.keys` (user-only, capped at 500 paths) once per project, at load and at `addProject`.
- A nested worktree never shows or sends the notes of the folder around it.
- A subfolder project gets its worktree's key from the daemon.

**M2: Fixed.** *Request changes* (`review-card.tsx:32`) and *Resolve all* (`diff-view.tsx:213`) clear by id.

**M3: Fixed.**
- Roles are recorded at `auth` (`server.rs`, `set_client_role`) and removed on unregister.
- `notes.changed` goes to User connections, and to a Session connection only when the key is that session's own worktree. Claimed and unauthenticated connections get nothing.
- Perf nit (Low, R3): `notes_changed` holds `clients.lock()` while it calls `session_policy` (sessions read lock plus the entry lock) and `worktree_key` (canonicalize plus a `.git` lookup up the folders) for each session connection. I found no path that takes these locks in the opposite order, so I see no deadlock. Still, compute the session keys before taking the clients lock.

**M4: Fixed, with one new mismatch (R1).** The Runs-page editor reads `planBringing` when any task has `worktree: true`, shows the size line, and sends `materialize` only when the config is approved.

**`git_head`: OK.**
- `app/src-tauri/src/git.rs:340-343` runs `guard(&root, "")` then `rev-parse HEAD`.
- It takes no user path inside the root, nothing is passed as an option, and it can only fail closed: an unborn HEAD gives a `null` snapshot, so the candidate is kept.
- It is the same trust level as the other `git_*` commands, which are called by the app's own webview.

**Lows from the first pass:**
- L1 now follows decision 2 for runs: nothing approved, nothing brought, `.worktreeinclude` included.
- L2, L3, L4, L5, L7, L8 and L9 are done as described. L3: canonical path inside the top folder, `.git` compared without case. L4: `brought` is structured and translated, with one shared `STATUS_LABEL`. L5: an in-memory set of removed paths. L7: the unit test's time bound is now 30 s.
- L6 was not done (stale `app-state.notes-*.bak.json`), as stated. It doesn't block landing.

### New findings from the fix round

**R1 (Medium, should fix, not blocking): the estimate line still counts `.worktreeinclude` when nothing is approved.**
- Where: `app/src/lib/try-agents.ts:74-83` and `bringingText` at `try-agents.ts:64-71`; also used by `runs-page.tsx:137-141`.
- **Failure:**
  - A repo has `.worktreeinclude` (e.g. `.env`) but no approved `.kelpie.yaml`: either no `.kelpie.yaml` at all, or one without `copy:`/`link:` that was never approved.
  - `planBringing` still calls `worktreeMaterializeEstimate(path, [], [])`, which reads `.worktreeinclude`. `unapproved` is false, so the dialog says "Each worktree copies .env (…)".
  - The run then sends `materialize: undefined` and brings nothing. The preview contradicts decision 2 and what actually happens.
- **Fix:** when `!approved`, set `estimate = null`, so the text says "starts as a clean checkout". Or have `estimate` take an `include_worktreeinclude` flag.

**R2 (Low): notes are lost when a removal fails after they were cleared.**
- Where: `worktrees.ts:243-245`, now that the clear comes first.
- When the non-forced removal then fails (a file lock on Windows, a dirty tree in Finish's "merged but not removed" outcome, or `discardSafely` keeping the candidate), the worktree stays and its notes are already gone.
- **Fix:** read the worktree's key first (`noteKeys[wt.id]`, or `note.keys`), and clear by that exact key only after `gitWorktreeRemove` succeeds. The daemon's new rule for missing paths makes clearing after removal safe, provided the key is passed exactly.

**R3 (Low): `notes_changed` does filesystem work under the clients mutex.**
- Where: `daemon.rs:345-358`. See M3 above.

**R4 (Low, UX): the confirm can be followed by a long silent wait.**
- Losers are stopped one after another at confirm (`run-compare.ts:270`, up to 10 s each), behind a 4-second toast, before Finish opens. With 4 slow losers that is about 40 s with no progress shown.
- Stop them in parallel (`Promise.all`), or show progress.

**R5 (Info): the worktree dialog treats `.worktreeinclude` differently from runs.**
- `createWorktree` (`worktrees.ts:119`) still copies `.worktreeinclude` without any approval. Decision 2 was asked about run worktrees, so I left this alone.
- If the decision is meant to cover all worktrees, the dialog needs the same gate.

### Still blocking
None. H1 to H4 and M1 to M4 are resolved. R1 is a Medium worth fixing before landing because the preview contradicts decision 2, but it causes no data loss and touches no trust boundary. R2 to R5 and L6 can follow.

Status: DONE_WITH_CONCERNS
Summary: All High and Medium findings are fixed and verified: an isolated-daemon probe of 30 param shapes against the scoped methods, the overlap cases run against the real `materialize.rs`, and 355 vitest and 17 filtered cargo tests pass; nothing is still blocking. One new Medium (R1: the size line still counts `.worktreeinclude` when nothing is approved) and four Lows remain, the main one being that notes cleared before a removal are lost if that removal fails.
