# Code review: Phase 6, turn checkpoints and artifacts

Date: 2026-10-24 · Branch rebrand/kelpie (uncommitted) · Reviewer: code-reviewer · Report only, no source edits.

## Scope
- Daemon: `crates/kelpied/src/checkpoints.rs` (1126 LOC), `checkpoints/tests.rs` (532), `daemon.rs` (+189), `auth.rs`, `main.rs`, `kelpie-proto` API 10.
- App: `turn-timeline.tsx`, `turn-file-view.tsx`, `lib/checkpoints.ts`, `checkpoint-api.ts`, `diff-view.tsx`, `rich-viewers.tsx`, `pane-view.tsx`, `right-panel.tsx`, `store.ts`, `types.ts`, `commands.ts`, settings, locales; `src-tauri/git.rs`, `watcher.rs`; `scripts/e2e/checkpoints.py`; docs.
- Read against: `phase-06-turn-checkpoints.md`, `plan.md` (definition of done, UX rules, RT-3/RT-4), developer report.

## How the findings were checked
- Re-ran `cargo test -p kelpied checkpoints` (21 passed, 1 ignored), `vitest checkpoints.test.ts` (7 passed), `pnpm i18n:check` (0 missing).
- Copied the daemon crates to a scratch workspace and added four probe tests (not in the repo) that call the real `Checkpoints` code. All four reproduced the failures they target (output quoted below).
- Ran plain git probes in scratch repos (git 2.55.0.windows.2) for hooks, sparse checkout, split index, a locked file, CRLF round trips, and timed each snapshot step on a generated 10k-file repo.
- No daemon was started. No real repository was touched.

## Overall assessment
The core mechanics are sound. The snapshot really never writes the user's index, HEAD, branch or stash, refs are written with a zero-old guard, every git call uses literal pathspecs, `safe_rel` guards deletes, `checkpoint.*` is user-only (mobile does not route through `dispatch`), the watcher ignores `refs/kelpie`, and the API-level gate is consistent across the app.

The undo safety guarantee does not hold, though: **undo can silently revert another session's work**, which is the phase's main success criterion and RT-4. The overlap check compares the wrong time windows, it forgets closed sessions, and it cannot see work that left no record. Several robustness gaps (sparse checkout, locked files, user hooks, lock contention counted as slowness) mean the feature either silently stops working or runs user hooks in common Windows setups. Perf has easy wins. **Not ready to land without fixing C1, H1 and H2.**

---

## Critical

### C1. Another session's work done while the owner was idle is reverted with no confirmation
- **Where:** `checkpoints.rs:863` (overlap filter `r.started <= rec.at && r.at >= rec.started`), `checkpoints.rs:604-607` and `daemon.rs:796` (`started` = when the owner started *working*), `checkpoints.rs:617-623` (the turn's diff runs from the previous snapshot, `end N-1`).
- **Problem:** A turn's diff covers `(end N-1, end N]`, which includes the owner's idle gap. The window used to look for other sessions only covers `[work start N, end N]`. Anything another session (or the user, or a shell) changed during the gap lands in turn N, counts as "clean", and is restored without asking.
- **Probe (scratch copy, real code):** A does turn 1 [100,200]. B edits `b.txt` during [300,400] while A is idle. A's turn 2 [500,600] edits only `d.txt`.
  ```
  PLAN {"items":[{"action":"restore","path":"b.txt","state":"clean","with":[]},{"action":"restore","path":"d.txt","state":"clean","with":[]}],...}
  DONE {"applied":true,"files":["b.txt","d.txt"]}
  b.txt now = "b.txt v0\n"      <- B's work silently reverted
  ```
  The existing test `turns_that_do_not_overlap_in_time_are_not_shared` only covers the opposite order (B after A).
- **The same path hits the user's own edits:** the user edits a file in an editor between turns, asks the agent to continue, then undoes that turn. The user's edit is listed as a plain "Restore", pre-ticked, and reverted.
- **Fix:**
  1. Compare diff windows, not work windows. Store `from = prev.at` on each record (or derive it from the `base_n` record) and test overlap as `r.from < rec.at && r.at > rec.from`. Apply this on both sides: the record being undone and the other sessions' records.
  2. Cover editors, shells and anything else that has no record: when the snapshot is taken, stat each changed path (cheap: the diff is at most 400 paths). If a path's mtime is before `rec.started`, mark it `state: "modified"` with the reason "changed before this turn started". It then needs confirmation. Deleted paths carry no mtime; flag them when the idle gap is longer than a few seconds.
  3. Add a test for the B-in-the-gap order shown above.

## High

### H1. Closing a session erases its turns from the overlap check, so its concurrent work becomes "clean"
- **Where:** `daemon.rs:665-667` (`close` calls `drop_session`) → `checkpoints.rs:1009-1018` (`forget` removes the log from memory and disk). `plan()` (`:855-868`) only looks at logs that are still loaded. `session_label` (`daemon.rs:822`) already has an "a closed session" branch, which suggests closed sessions were expected to keep showing up.
- **Probe:**
  ```
  PLAN before close {"items":[{"path":"f.txt","state":"shared",...},{"path":"g.txt","state":"shared","with":[{"session":"b"}]}]}
  PLAN after close  {"items":[{"path":"f.txt","state":"clean","with":[]},{"path":"g.txt","state":"clean","with":[]}]}
  DONE {"applied":true,"files":["f.txt","g.txt"]}; g.txt now = "<missing>"   <- B's file deleted
  ```
  This is a normal sequence: two agents work, the user closes one tab, then undoes the other agent's turn.
- **Fix:** On close, delete the refs but keep a metadata-only tombstone (`session`, `key`, and each record's `from`/`at`/`paths`/`truncated`, no commits) in memory and in `<sid>.json` with `closed: true`. `gc()` drops a tombstone only once no live log in the same repo has a record overlapping it, or after the 50-turn horizon of every session there. `owners()` can keep showing it as "a closed session".

### H2. Work that left no record is invisible to undo
These all produce a working window that `plan()` never sees, so the files involved fall into another turn as "clean":
- **Snapshot failed or was skipped** after 3 attempts (`checkpoints.rs:580-595`). Nothing is recorded, and the next turn of that session silently contains two turns without the *approximate* label.
- **Coalesced jobs** (`checkpoints.rs:533`). A second Turn job for a session that already has one queued is dropped. The surviving job keeps the *older* `started`/`at`, so the newer window disappears even though its changes are in the snapshot.
- **Missed boundaries** (`daemon.rs:997`). Only `Working -> Idle` triggers a snapshot. `Working -> Waiting -> Idle` (a permission prompt that ends the turn, a chat elicitation) never does, and neither does `Working -> Exited`. An agent that exits after its last turn (`claude -p`, `codex exec`, run tasks) never has that turn recorded; it is absorbed into the next session's or the next restart's baseline.
- **Fix:**
  - Record the window even when no snapshot is written. Append it to the session's log as `{kind: "window", from, at, paths: null}`; `paths: null` is already treated as "may be any file" in `plan()` (`:883`).
  - On coalesce, widen the queued job (`at = max`, `started = min`).
  - Trigger on `prev ∈ {Working, Waiting}` and `status ∈ {Idle, Exited}`, taking `cwd` from the `info` that is already cloned.
  - Mark a record `approximate` when the previous attempt for that session failed.

### H3. Undo can target a different session's turn
- **Where:** `turn-timeline.tsx:37-40`, `:133-136`, `:285`.
- **Problem:** `session()` is reactive. With no explicit choice and no agent pane focused, it falls back to `list[0]`, which is sorted by `last_activity` and changes whenever any agent prints. `UndoDialog` receives `session={session()!}` and reads `props.session.id` only when **Undo** is clicked. If another agent becomes the most recently active one while the dialog is open (the plan alone takes about 1 s on a 10k repo), the request goes to **the other session's turn #n**, with the path list and `confirm` list from the first plan. The daemon only rejects paths that are not part of that turn, and turns often share files. The dialog title does not name the session either.
- **Fix:** Freeze the target when the dialog opens: `setUndoing({ session: s.id, n })` and pass plain values, not the memo. Show the session name in the heading ("Undo turn #3 of Claude 1a2b3c?"). Also pin the timeline itself to the session it was showing until the user changes the selector; today the list jumps between agents as they print.

### H4. User hooks run during snapshots and undo
- **Where:** `command()` (`checkpoints.rs:167-191`). The spec's concern was "commit-tree must not run hooks". `commit-tree` indeed runs none, but `update-ref` and `restore` do.
- **Probe output** (with `core.hooksPath` pointing at logging hooks):
  ```
  == after commit-tree + update-ref:  reference-transaction preparing / prepared / committed
  == after restore:                   post-checkout <head> <head> 0
  == after delete (update-ref --stdin): reference-transaction ... committed
  == with -c core.hooksPath=<empty dir>: (nothing ran)
  ```
- **Impact:**
  - Every snapshot and every trim runs the user's `reference-transaction` hook. git-branchless records every ref change in its event log, and ref-mirroring hooks would copy `refs/kelpie/*`.
  - Every undo runs `post-checkout`. Husky and git-lfs install one, and many husky setups run `npm install` / codegen there without checking the "file checkout" flag.
- **Fix:** Add `-c core.hooksPath=<data_dir>/checkpoints/no-hooks` (an empty folder the daemon creates) to `command()`. That disables every hook for these internal calls. Add a test with a logging `reference-transaction` hook.

### H5. A file another program holds open fails the whole snapshot, every time
- **Where:** `working_tree()` (`checkpoints.rs:262`).
- **Probe** (a file opened by PowerShell with `FileShare.None`, a common case on Windows: a dev server's SQLite DB, an open Office document, a running `.exe` in an unignored `bin/`):
  ```
  add -A                  -> error: open("locked.db"): Permission denied / fatal: adding files failed / exit=128
  add -A --ignore-errors  -> exit=1, index written, tree has every other file
  ```
- **Impact:** Retries cannot help while the lock persists, so every turn in that repo is lost and turns merge (see H2). With 3 attempts plus backoff on a 10k repo, each failure takes more than 1 s, so after two turns the repo is switched off with the misleading message "took over a second" (see H6).
- **Fix:**
  - Use `add -A --ignore-errors`. Treat exit 1 as success when `write-tree` succeeds.
  - Parse the `unable to index file '<p>'` lines into `rec.unreadable`. In `plan()`, give those paths `state: "modified"` with the reason "could not be read at snapshot time".

### H6. Lock contention and failed attempts count as "slow" and permanently switch the repo off; the `index.lock` wait is not needed
- **Where:** `checkpoints.rs:579-594` (`started` is taken before the retry loop, so `took` includes the 150 ms and 450 ms sleeps and the failed attempts) and `:584` (skips the attempt when `.git/index.lock` exists).
- **Probe:** an external `index.lock` held for 400 ms (one IDE `git status` refresh) during each of two turns on a 5-file repo:
  ```
  turn 0: Ok(()) took 2.0s
  turn 1: Ok(()) took 1.96s
  EVENTS ... ("checkpoint.disabled", {"reason": "A snapshot of this repository took 1.8 s; turn snapshots are off for it"})
  ```
- **Why the wait is not needed:** the snapshot never needs the real index lock. git replaces `index` by renaming `index.lock`, so copying `index` while a lock exists reads the last complete index. The wait only adds latency and false "slow" counts. VS Code and other IDEs create `index.lock` many times a minute.
- **Fix:**
  - Remove the `index.lock` precheck.
  - Time only the successful attempt (`let t = Instant::now()` inside the loop) and call `note_duration` only on success.
  - Keep the retry for real failures.
  - `disabled.json` should store the *measured snapshot* time, not wall time including sleeps.

### H7. Under CPU load, below-normal git is starved: snapshots take minutes, capture the wrong content, and switch checkpoints off for good (root cause of the e2e flake)
- **Where:** `command()` `creation_flags(... | 0x0000_4000)` (BELOW_NORMAL_PRIORITY_CLASS, `checkpoints.rs:188`); `note_duration` measures wall time (`:579`, `:593-594`); the auto-off is persisted in `disabled.json` (`:701-704`).
- **Probe** (scratch copy, real `run_job`, 5-file fixture; normal-priority busy threads on all 4 cores stand in for `cargo build`/rustc):
  ```
  idle machine, turn 0..2:     435 ms, 421 ms, 423 ms
  loaded machine, turn 0:      Ok(()) 305.4 s      <- one snapshot of a 5-file repo
  loaded machine, turn 1:      Ok(()) 326.6 s
  ```
  I stopped the probe before its third loaded turn so it would not hold the machine's cores for another 5 minutes, so its `checkpoint.disabled` print was not reached. Two consecutive runs of at least 1 s meet `SLOW_LIMIT` in `note_duration` (`:699-706`). The H6 probe shows that exact path emitting `checkpoint.disabled` and leaving the repo `disabled` in `status()`.
  On Windows, a below-normal thread gets no CPU while normal-priority threads keep every core busy. It only progresses through the balance-set manager's starvation boost, roughly one quantum every few seconds. Agents run builds and tests, so this situation is normal during agent work, not exotic.
- **Consequences:**
  1. **Auto-off from load, not repo size.** Two such snapshots in a row trip the ">1 s twice" rule. The repository is switched off and stays off across restarts (`disabled.json`), with the message "A snapshot of this repository took N s". A busy machine silently turns the feature off; the coordinator's suspicion is correct.
  2. **Wrong content for the turn.** The snapshot reads files when git finally runs, possibly minutes after `job.at`. By then the agent may be in its next turn, so those edits land in turn N, whose recorded window still ends at `job.at`. That breaks attribution and the overlap check (it adds to C1).
  3. **Queue stalls.** The per-repo queue is serial, so every later job in that repo (other sessions' baselines included) waits behind a starved snapshot.
- **Fix:**
  - Bound the starvation. Spawn below normal, but if the child is still running after about 300 ms, raise it with `SetPriorityClass(child_handle, NORMAL_PRIORITY_CLASS)` (windows-sys is already a dependency). Alternatively, run git at normal priority: a snapshot is short and serial per repo, and typing latency is protected by the existing visible-terminal prioritization.
  - Decide slowness on git's own cost, not wall time. Sum the children's user+kernel time (`GetProcessTimes` on Windows, `getrusage(RUSAGE_CHILDREN)`/`wait4` on unix). Failing that, skip counting when a reference `git --version` in the same attempt took over about 200 ms (the machine is loaded).
  - Do not persist an automatic switch-off across restarts. Keep `disabled.json` for user decisions only.
  - Record the time the snapshot actually read the tree and use it as the record's window end (`at = max(job.at, snapshot_time)`).

---

## E2E flake (coordinator request): `checkpoints.py` "both sessions recorded their overlapping turns"
- **Symptom:** 19 PASS, then `wait_records(s2, 2, 60)` / `wait_records(s3, 2, 60)` returned nothing. Then "scenario ran to completion" FAILED, because with `r2` empty the script falls back to `n2 = 1` and `checkpoint.undo` throws "no checkpoints for this session". So **s2 and s3 had no records at all, not even their baselines**: the two earlier `wait_records(s2, 1)` / `(s3, 1)` calls (lines 282 and 284) are not checked and must also have timed out. Run 2 on a quiet machine passed 38/38.
- **Root cause (high confidence): H7.** The run happened while a concurrent `cargo test` and a fresh debug build kept the cores busy at normal priority, and the daemon's snapshots run git below normal. Both possible outcomes give exactly "zero records for s2/s3 within 45 s + 60 s":
  - (a) **Auto-off.** Two consecutive s1 snapshots took over 1 s of wall time (most likely turn 3 and the watcher turn; undo snapshots are not timed). `note_duration` disabled the fixture repo after s1's record was saved, which is why every s1 check still passed. `submit()` then drops every job for that repo (`checkpoints.rs:526`), so s2/s3 never got a baseline.
  - (b) **Starvation.** A single s1 job took minutes (305 s measured above for a 5-file repo), and s2's baseline waited behind it in the serial per-repo queue.
  The test cannot tell which one happened: it never reads `checkpoint.status`, and teardown deletes `DATA` (including `disabled.json` and the daemon log) whatever the outcome.
- **Ruled out:**
  - Queue design starving one session in favour of another: there is one FIFO per repo, coalescing only drops duplicates of the *same* session, and each job is bounded on an idle machine (about 0.4 s here).
  - A test timing assumption on a quiet machine: 45 s + 60 s is ample, and run 2 passed.
  - `index.lock` from the app: `app/src-tauri/src/git.rs:33` sets `GIT_OPTIONAL_LOCKS=0`. The test's own `git()` helper also passes `GIT_OPTIONAL_LOCKS=0`, so H6's lock path is unlikely here.
- **Fix:**
  - **Product:** H7 (bounded priority, slowness measured on git's own CPU time, auto-off not persisted) and H6 (time only the successful attempt). This is a product bug: a user's machine running a build behaves exactly like the flaky run.
  - **Test:**
    - Check the results of `wait_records(s2, 1)` / `(s3, 1)`.
    - On any `wait_records` failure, add `daemon(d, "checkpoint.status")` to the failure detail, so a `disabled` entry shows up.
    - Assert `checkpoint.status()["disabled"] == []` once after section 1.
    - Keep `DATA` when a check failed (print its path) instead of removing it in `teardown`.
    - Do not raise the timeouts to hide it.

---

## Medium

### M1. A stale `<sid>.index.lock` in `checkpoints/tmp` blocks that session's snapshots forever
- **Where:** `working_tree()` removes `tmp_index` but never the lock file. If the daemon or a git child is killed during `add -A`, the lock stays. Session ids survive restarts, so that session never snapshots again and only a log line says why.
- **Probe:**
  ```
  RESULT Err(git add: fatal: Unable to create '...\checkpoints\tmp\s1.index.lock': File exists.
  ```
- **Fix:** Before copying, remove `tmp_index.with_extension("index.lock")`. This is safe: the path is per session and every use is serialized by the repo lock. Also empty `checkpoints/tmp` in `Checkpoints::new`.

### M2. Sparse-checkout repos can never be snapshotted (and are then switched off as "slow")
- **Probe** (`git sparse-checkout set in`, agent creates `out/created.txt`):
  ```
  The following paths ... outside of your sparse-checkout definition, so will not be updated in the index: out/created.txt
  add -A exit=1          (the index is still written; the tree has every other file)
  ```
  `git_raw` treats exit 1 as failure, so every snapshot in such a repo fails, is retried, counts toward "slow" and the repo gets switched off.
- **Fix:** Pass `-c advice.updateSparsePath=false`. When `core.sparseCheckout` is on, accept exit 1 if `write-tree` succeeds. List out-of-cone files as not captured (or use `add -A --sparse` after checking that skip-worktree entries are not recorded as deletions).

### M3. Undo, plan and drop block the app's whole request queue
- **Where:** `server.rs:84-110`. Each client's requests run strictly in order on one worker. `checkpoint.undo_plan` runs a full `add -A`. `checkpoint.undo` runs plan, `restore` and a new snapshot. Both first wait for the repo lock behind any in-flight snapshot, including its backoff sleeps. On a 10k repo that is about 1.5-3 s during which `session.create`, `attach`, `resize` and `list` from the app all wait. Keystrokes are separate frames and are not affected.
- **Fix:** Run `checkpoint.undo_plan`, `checkpoint.undo` and `checkpoint.drop` (and `diff`/`blob` for large repos) on their own `spawn_blocking`. Return `Ok(None)` and send the reply through `tx` when done; `attach` already uses this pattern.

### M4. Spec departure: token-less hook reports decide turn boundaries and the undo "idle" gate
- **Where:** `auth.rs:68-71`. Any caller that names an existing session id without a token gets `Claimed` and may send `hook.report` for that id. Session ids are visible to scripting sessions through `session.list`.
- **Judgement:**
  - *For boundaries, the departure is acceptable.* Real Claude hook reports arrive without the token (it is stripped), and chained snapshots keep undo correct even if a boundary is forged: a forged boundary only splits or merges turns. Worst case otherwise is a snapshot every tick, coalesced to one per session.
  - *For the undo gate, it is not acceptable.* A forged `Stop` makes `undo_ctx.owner_working` false while the agent is still writing (`daemon.rs:829-831`).
- **Fix:** In `undo_ctx`, also treat the owner as working when it produced output within the last few seconds (`Activity.last_output`), whatever the hook says. Mark records whose boundary came from a claimed report as `approximate`.

### M5. Undo is not byte-exact under `core.autocrlf=true` (the Git for Windows default)
- **Where:** `command()` sets `core.safecrlf=false`, which hides git's warnings about lossy conversion. `add -A` normalizes line endings, and `git restore` converts them back.
- **Probe** (`autocrlf=true`, a file `mixed\r\nendings\nhere\r\n` captured by a snapshot, then restored from it):
  ```
  before: m i x e d \r \n e n d i n g s \n h e r e \r \n
  after : m i x e d \r \n e n d i n g s \r \n h e r e \r \n
  ```
  LF-only files an agent wrote (shell scripts, files under `.editorconfig` with `end_of_line=lf`) come back as CRLF after an undo, which breaks `bash` scripts.
- **Fix:** Use `core.safecrlf=warn` and collect the `CRLF will be replaced by LF` / `LF will be replaced by CRLF` paths from `add -A` stderr into the record. Show those files in the plan as "line endings will change", or restore them with `-c core.autocrlf=false` from a blob added with `git -c core.autocrlf=false add -- <those paths>`. At minimum, state the limitation in the undo dialog. The developer notes it, but users are not told.

### M6. Output-driven boundaries for hook-less agents fire on every repaint burst
- **Problem:** The spec's boundary for agents without hooks is "input after idle → next idle". The implementation uses any `Working -> Idle` transition (`status.rs:121-129`: output within 1.5 s). A TUI agent that repaints a clock or spinner while idle flips Working/Idle and triggers a snapshot each time. On a 10k repo that is about 0.25 s of git work per flip even when nothing changed.
- **Not listed** as a departure in the developer report.
- **Fix:** For `approximate` sources, only submit a turn end when there was user input since the last boundary (`Activity.last_input > last boundary`). That is the spec's rule.

### M7. Removing a worktree drops checkpoints before knowing the removal succeeded
- **Where:** `git.rs:354-356`. `checkpoint.drop` runs before `git worktree remove`. If removal fails (a dirty worktree without `force`), the refs and records are already gone, and the sessions there are marked `dropped` (`checkpoints.rs:1012`), so they never snapshot again until the daemon restarts, while still running.
- **Fix:** Call `checkpoint.drop` after a successful remove, and have `drop_session` run git in a folder that still exists: the refs live in the common git dir, so `git -C <root>` works. Alternatively, pass `root` along with `worktree`.

### M8. Review notes from turn files use repo-relative paths
- **Where:** `turn-file-view.tsx:103`, `turn-timeline.tsx:160-161`.
- **Problem:** When the project is a subfolder of the repository, `pane.path` (documented as project-relative in `types.ts:92`) holds a repo-relative path. Notes then name the wrong file when sent to an agent, and **Open** from the owners view opens a non-existent path. The developer notes it, but it ships as a silent wrong-path bug.
- **Fix:** `checkpoint.list` already returns `repo`. Strip `relative(repo, project.path)` from daemon paths in the app, and hide files outside the project, or show them with a `../` prefix.

## Low
- **L1. Ref leak on crash** (`checkpoints.rs:1016-1026`). `forget` deletes `<sid>.json` before the refs are deleted. A crash in between leaks those refs permanently, because GC only knows record files. Delete the refs first, then the file.
- **L2. Turn start race** (`daemon.rs:998-1000`). `take_work_start` runs inside `spawn_blocking`. If the blocking pool is saturated and the session starts working again first, it takes the *next* turn's start. Take it synchronously on the ticker (a mutex op) and move the value into the closure.
- **L3. File I/O under the global lock.** `push_record` writes the log while holding `inner` (`:688`). `drop_worktree` reads every stored log under `inner` (`:1032-1036`). The async ticker locks `inner` in `work_started` (`daemon.rs:995`), so a slow disk or antivirus stalls status updates for every session. Serialize under the lock, write after dropping it.
- **L4. Turns panel on an older daemon** (`right-panel.tsx:65`). A workspace saved with `rightPanel: "turns"` renders `TurnTimeline` against an older daemon: the tab is hidden but the `Match` is not gated, so the panel shows "Turns could not be read". Gate the `Match` like the tab.
- **L5. Docs overstate.** `docs/architecture.md` says the index is "never opened for writing". `git restore` takes and rolls back `index.lock`, so a concurrent `git commit` by the user can fail with "index.lock exists" during an undo. Reword it, and retry the restore once on that error.
- **L6. Snapshots are visible to other tools.** `refs/kelpie/*` shows up in `git log --all`, graph GUIs and `git for-each-ref`, and is published by `git push --mirror` (snapshots include untracked, non-ignored files). Mention this in the settings description and user guide.
- **L7. Wrong label when hooks are missing.** `source = "hook"`, `approximate = false` for every `claude`/`codex` session (`daemon.rs:789-795`), even when hooks are not installed and status is output-driven. Use `Activity.hook.is_some()`.
- **L8. Deletes follow junctions.** `remove_file(top.join(p))` follows a junction or symlink in a parent folder. The case needs a confirmed "modified" item, but checking that no ancestor is a reparse point (`symlink_metadata`) costs nothing.
- **L9. Unknown split-index behaviour.** With `core.splitIndex=true`, no new `sharedindex.*` file appeared in `.git` (probe). Still, add `-c core.splitIndex=false` so the temporary index is always self-contained and git's shared-index cleanup can never run against the user's git dir.
- **L10. Style nits.** `rich-viewers.tsx:53` is missing a space (`"",props.path`). `undo_ctx` (`daemon.rs:839`) builds `(name, id)` and then swaps it; build `(id, name)` directly. The `KEEP` trim counts undo and restart-baseline records as "turns", so the UI text "The last 50 turns are kept" is slightly off.

---

## Performance

The developer measured 0.74 s p50 / 0.80 s p95 per changed snapshot on a 10k-file repo (0.25 s when nothing changed). Each snapshot runs `rev-parse` (`Repo::open`), `for-each-ref` (first time only), copy, `add -A`, `write-tree`, `rev-parse HEAD`, `commit-tree`, `update-ref`, `diff-tree`, plus `update-ref --stdin` when trimming. That is 7-8 git processes at below-normal priority.

**Measured here** (`perf_probe.py` in the scratchpad: the shipped sequence step by step through `subprocess` with the daemon's creation flags, a generated 10k-file / 200-folder committed repo, 3 edits + 1 new file per turn, 6 turns per variant, idle i5-6440HQ):

| Step | p50 |
|---|---|
| bare `git --version` start (below normal / normal) | 59 / 51 ms |
| `rev-parse --absolute-git-dir --git-path index` | 57 ms |
| copy `.git/index` | 4 ms |
| `add -A` (temporary index) | 183 ms |
| `write-tree` | 174 ms |
| `rev-parse HEAD` | 58 ms |
| `commit-tree` | 96 ms |
| `update-ref` | 64 ms |
| `diff-tree --raw --numstat` | 67 ms |
| **total** | **711 ms** (max 780), matching the developer's 0.74 s |

| Variant | total p50 |
|---|---|
| kept temporary index (no re-copy) | 938 ms |
| kept index + `core.untrackedCache=true` | 692 ms |
| kept index + untracked cache + builtin fsmonitor | 827 ms (the first call starts the daemon) |

**Reading:**
- About 400 of the 711 ms is **process start-up** (7 git processes × about 55 ms). The real work is about 130 ms in `add -A` (lstat plus directory walk), about 115 ms in `write-tree`, and about 40 ms in `commit-tree`.
- On a fixture whose real index is fresh, keeping the temporary index, the untracked cache and fsmonitor gave **no measurable gain** (differences are within run-to-run noise). I withdraw them as headline wins.
- The 150 ms target cannot be reached with the git CLI on Windows. Even with every avoidable process removed, 4 starts (`add`, `write-tree`, `commit-tree`, `update-ref`) cost about 220 ms before any work. **The developer's default-off decision is justified.**

**Concrete improvements, in order of value:**
1. **Remove avoidable processes: about 120 ms (17 %).**
   - Cache `Repo::open` per repo key (−57 ms).
   - Drop `rev-parse HEAD`: `commit-tree -p HEAD` resolves the revision itself. Fall back to no `-p` on the unborn-HEAD error (−58 ms).
   - Fold the trim deletes into the same `update-ref --stdin` transaction as the new ref (`create <ref> <oid>` + `delete <old>`), which saves one more start whenever a trim happens.
   Expected result: about 590 ms per changed snapshot and about 140 ms less per unchanged one.
2. **Measure only the successful attempt, drop the `index.lock` precheck (H6), and fix the priority starvation (H7).** These do not change the quiet-machine number, but they remove the 0.6 s of backoff sleeps and the multi-second to multi-minute stalls under load that set p95 (the developer's noisy 30-turn run had p95 4.5 s).
3. **Keep the temporary index between snapshots only for the cases it helps.** When the user's real index is stale, or many untracked, non-ignored files exist (each one is re-hashed in full on every snapshot today, because untracked files are never in the copied index), a kept index avoids that. The fixture does not show this; measure on a repo with a stale index or a large untracked folder before adopting it.
4. **The only path to about 150 ms is in-process:** `gix` status + tree + commit + ref writing in one process, with no process starts. If the 150 ms default-on gate matters, schedule that as its own task.

**Blocking:** snapshots run on a per-repo worker thread with git at below-normal priority, so session I/O threads are not blocked. Under CPU load the snapshot itself is the one starved (H7). Two exceptions remain: the app's RPC queue during undo/plan (M3), and the ticker briefly contending on `inner` (L3). The keystroke-echo-during-snapshot metric and `.git` growth at 100k files were not measured (developer report).

---

## Spec departures
| Departure | Verdict |
|---|---|
| Hook token rule replaced by ticker status transitions, `checkpoint.*` user-only | **Safe for boundaries, unsafe for the undo gate** (M4). User-only RPCs verified: not in `SCRIPTING`/`OWN`/`CLAIMED`, mobile does not route through `dispatch`, test at `auth.rs:247`. |
| Concurrent-edit attribution blur, compensated by flagging overlapping files as shared | **Unsafe as implemented.** The compensation depends on complete overlap detection, which fails in three proven ways (C1, H1, H2). |
| GC limited to sessions this daemon has record files for | **Safe.** It only ever deletes `refs/kelpie/<own sid>/`, the trailing-slash prefix cannot match another id, and the `stranger` test covers foreign refs. The leak on a lost record is acceptable; fix the delete order (L1). |
| Hook-less boundary is "output went quiet", not "input after idle → next idle" (not listed) | **Unsafe for cost** (M6). Reinstate the input condition. |
| `session.rs` / `chat.rs` boundaries replaced by a single ticker transition | Fine in principle, but it misses `Waiting -> Idle` and `Exited` (H2). |

## Verified OK (relevant for risk calibration)
- The real index, HEAD, symbolic HEAD, `status` and stash are byte-identical after a snapshot (test plus code review). `GIT_OPTIONAL_LOCKS=0` prevents index refreshes by the read-only calls.
- `GIT_LITERAL_PATHSPECS=1` everywhere. Restores go through `--pathspec-from-file` with NUL separators. The `[ab].txt` test proves the restore does not widen.
- `restore --source --worktree` works for a path the real index does not track (probe), and with `--worktree` alone the index is unchanged.
- Undo paths must belong to the turn's own diff, and `blob` checks `safe_rel`. There is no path traversal from the app.
- Watcher: `refs/kelpie/**` is ignored. `logs/`, `packed-refs`, `reftable/` and `index.lock` were already ignored, and `refs/heads` is still a git event.
- API gate: tab, palette command, settings push and `checkpoint.drop` are all tolerant of a daemon below API 10. Remote sessions and shells are excluded both in `checkpoint_eligible` and in the UI filter.
- Submodules, links and nested repos are skipped in undo. An unborn HEAD gives a root snapshot commit.

## Recommended actions (in order)
1. C1: diff-window overlap plus the mtime check for edits before the turn started, with a test for B working in A's idle gap.
2. H1: tombstones for closed sessions.
3. H2: record windows without snapshots, widen windows on coalesce, trigger on Waiting/Exited.
4. H3: freeze the session and turn in the undo dialog and name the session.
5. H4: `core.hooksPath` pointing at an empty folder for every internal git call.
6. H7: bound the below-normal starvation, measure slowness on git CPU time, do not persist auto-off, use the real snapshot time as the window end. Harden `checkpoints.py` as described in the flake section.
7. H5, H6, M1, M2: `--ignore-errors`, sparse handling, no lock precheck, time only the successful attempt, clear stale tmp locks.
8. M3: async replies for undo, plan and drop.
9. Perf 1 (cache the repo lookup, `-p HEAD`, one `update-ref` transaction: about −120 ms), then re-measure. The 150 ms gate needs in-process `gix` (perf 4).
10. M4-M8 and the Low items.

## Plan follow-ups (report only, plan files not edited)
- Definition-of-done items still open according to the developer report and not run here: `ui_tour.py` screenshots (dark/light/compact) for the Turns panel, the keystroke-echo-during-snapshot metric, and 100k-file numbers.
- Phase success criterion "undo never silently reverts another session's work" is **not met** (C1, H1, H2).

## Unresolved questions
- Should the user's own edits between turns (C1, second scenario) be protected the same way as another session's? The spec only names sessions; I recommend yes, through the mtime flag.
- Are real Claude/Codex hook reports always token-less (claimed)? If some carry the token, M4's undo gate could rely on token-verified reports alone.

Status: DONE_WITH_CONCERNS
Summary: Snapshots are safe for the user's index, HEAD and refs, but undo can silently revert another session's work in three reproduced ways: idle-gap windows, closed sessions forgotten, and work that left no record. In addition the undo dialog can target the wrong session, and user hooks run. Below-normal git is starved under CPU load (305 s for one snapshot of a 5-file repo), which switches checkpoints off for good and is the most likely cause of the e2e flake. Perf is dominated by process start-up (about 400 of 711 ms): about −120 ms is available cheaply, and 150 ms needs in-process git.

---

## Re-review (2026-10-24, after the developer's fixes)

The status block above belongs to the first pass. The current verdict is at the end of this section.

### How it was checked
- Copied the current `crates/kelpie-proto`, `crates/kelpied` and `third_party/conpty` to a fresh scratch workspace and appended adapted probes (`scratchpad\p6-review\probes2.rs`). All probes call the real `Checkpoints` code. No daemon was started and no real repository was touched. Every process I started was stopped.
- On the real tree:
  - `cargo test -p kelpied`: 118 passed, 1 ignored.
  - `cargo clippy -p kelpied --all-targets -D warnings`: clean.
  - `pnpm typecheck`: clean.
  - `vitest checkpoints.test.ts`: 7 passed.
  - `i18n:check`: 0 missing.

### 1. Original findings: verified
| Finding | Result | Evidence |
|---|---|---|
| C1 idle-gap work of another session | **Fixed** | Probe: `b.txt` is now `state: "shared"` with B named, the undo returns `needs_confirmation`, and `b.txt` keeps "work of B". Overlap compares `from..at` diff stretches (`checkpoints.rs:1035-1043`). The mtime check flags user/shell edits to files the turn did not touch (probe: user-only `b.txt` → `modified`, "changed before this turn started"). |
| H1 closed session forgotten | **Fixed for the common order, hole remains** (see B1) | Probe (B's turn already recorded, then closed): both files `shared` with B, undo refused, `g.txt` intact. The tombstone survives a restart (developer test). |
| H2 work without a record | **Fixed** | A failed snapshot adds a gap window with `paths: None` and makes the next record approximate (developer test with a corrupt index; code `:754-760`, `:782-800`). The queue widens instead of dropping (`coalesce`, `:1351-1360`, test). `Working|Waiting → Idle|Exited` triggers a snapshot (`daemon.rs:1010`). |
| H3 undo targets the wrong session | **Fixed** | The target `{session, name, n}` is fixed when the dialog opens (`turn-timeline.tsx:121`, `:137-146`); the selection is pinned (`:41-43`). |
| H4 user hooks | **Fixed** | `-c core.hooksPath=<data>/checkpoints/no-hooks` on every call (`:210-214`). A `-c` override beats the repo's own `core.hooksPath`. Developer test with `reference-transaction` / `post-checkout` / `pre-commit` passes. |
| H5 locked file | **Fixed** | `add -A --ignore-errors`, exit 0/1 accepted, `unreadable` recorded (`:374-386`). Developer test holds a file with share mode 0. |
| H6 lock contention counted as slow | **Fixed** | Probe: an external `index.lock` held 400 ms on each of two turns. Turns took 0.50 s and 0.48 s, with **0** `checkpoint.disabled` events (before: 2.0 s each and the repo switched off). |
| H7 starvation / permanent auto-off | **Fixed** | Probe with normal-priority burners on every core: 1.38 s / 1.87 s / 2.01 s wall (before: 305 s and 326 s), **0** disabled events. Git runs at normal priority, slowness is measured on job-object CPU time, auto-off lives in memory only, and records use `at = max(turn end, read end)` with an approximate label after 3 s. |
| M1-M4, M6-M8, L1-L10 | Fixed as described | Spot-checked: `:368`/`:561` (stale tmp locks), `:381` (sparse), `daemon.rs:1182-1191` (async replies), `undo_ctx` output check, the M6 `typed` gate, `git.rs` drop after a successful remove with `root`, `right-panel.tsx:65` gating. |
| M5 CRLF | Disclosed only | Acceptable as a disclosed limitation (the dialog and guide say so). LF-only scripts still come back as CRLF under `autocrlf=true`. Not blocking. |
| Perf | Repository lookup cached, `rev-parse HEAD` dropped | Consistent with my estimate of about −115 ms. The developer reports p95 0.39 s. Default off remains correct. |
| E2E hardening | Done | Both baselines are checked, `status_note()` adds `checkpoint.status` to failure details, a "nothing was switched off" check is in place, and `DATA` is kept on failure (`checkpoints.py:179-181`, `:245`, `:296-298`, `:494-495`). |

### 2. Regressions and new holes

**B1. BLOCKING: closing a session right after its turn ends loses that turn's work from undo**
- **Where:** `daemon.rs:1015` takes the work start on the ticker and queues the job. `close_session` (`checkpoints.rs:1192-1215`) only adds a window when `work_start` is still set, which it no longer is. It marks the session `dropped`. The queued job then returns early on `dropped` (`:730-732`), or `push_record` discards the record (`:842-843`). Finally, `prune_tombstones` can delete the tombstone at once, because without that window its last activity is just its baseline.
- **Probe:**
  ```
  NEW-close: queued B job result Ok(()); B log = None          <- tombstone pruned, B's turn never recorded
  NEW-close PLAN {"items":[{"path":"f.txt","state":"clean"},{"path":"g.txt","state":"clean","with":[]}]}
  NEW-close UNDO {"applied":true,"files":["f.txt","g.txt"]}; g.txt = "<missing>"   <- B's output deleted, no confirmation
  ```
- **Realistic:** the agent finishes, the user closes the tab within the snapshot latency (0.4-0.7 s, longer when the repo queue is busy), and later undoes another agent's turn in the same folder.
- **Fix:** In `close_session`, while holding the repo lock, take that session's jobs out of the queue and either run them inline (the snapshot becomes a record, then the refs are deleted with the rest) or turn each into a `Window { from: last record's read_at, at: job.at }`. Also make the `dropped` early returns in `run_job`/`push_record` add the same window to the tombstone instead of discarding it. Add a test with this exact order: `take_work_start` → `close_session` → `run_job`.

**B2. BLOCKING for shell sessions, strongly recommended for the user's editor: an edit made between turns to a file the agent then also edits is reverted unflagged**
- **Where:** the mtime check (`:1046-1050`, `:1081`) only catches files whose *last* write was before the turn started. Once the agent writes the file too, its mtime is fresh. The restore goes back to the previous turn's end, which predates the edit made in the gap.
- **Probe** (user adds "user fix" to `a.txt` in the gap; the agent then appends "agent line" in its turn):
  ```
  NEW-gap PLAN {"items":[{"path":"a.txt","state":"clean"}, {"path":"b.txt","state":"modified","reason":"changed before this turn started"}, {"path":"d.txt","state":"clean"}]}
  ```
  `a.txt` is pre-ticked as clean. Undoing it restores `a.txt v0`, and "user fix" is gone from the folder (it survives only inside the turn's snapshot until it is trimmed).
- **Why it blocks:** shells are sessions. `git pull`, a formatter or a codegen run in a terminal tab between two agent turns is an ordinary workflow, and shells leave no records or windows. That violates "undo never silently reverts another session's work".
- **Fix:**
  - Minimum, to unblock: give local shell sessions in a git folder "may have touched any file" windows. On a shell's `Working → Idle` after user input (the same `typed` rule as M6), push a `Window { from: work start, at: now }` into a log for that shell (no snapshot, no git). `plan()` already treats `paths: None` windows as shared, so the file shows "shared with Terminal 2" and needs a tick.
  - Comprehensive, also covering the user's editor: when a turn starts (`Idle → Working`), queue a gap snapshot. If the tree differs from the previous record, store it as an unattributed "between turns" record and use it as the turn's base. The race only moves the agent's first writes into the gap record, which is the safe direction (undo does less). Cost: one extra snapshot per turn while the feature is opt-in.

**N1. Medium, not blocking: a restart baseline in mid-history hides the work before it**
- **Problem:** A `Baseline` record created with a previous record present (daemon restart after a crash, or a resume) is excluded from `windows_of` (`:1343`) and has no `from` or paths. Work the session did between its last snapshot and the restart (for example the turn it was in when the daemon died) is in no window, so another session's overlapping turn shows those files as clean.
- **Fix:** For a `Baseline` with a previous record, compute `from = prev.read_at` and the diff paths like a turn, and expose it to `windows_of` (it stays un-undoable for its own session).

**N2. Low: a coalesced job's wider window weakens the mtime check.** `rec.started = min(...)` moves the stale threshold back to the first coalesced turn. A user edit made between the two merged turns is then not flagged. This only happens when the queue is backed up. Keep the latest `started` for the mtime test and the earliest for windows.

**N3. Low: stderr parsing assumes English.** `unreadable_files` (`"unable to index file"`), the unborn-HEAD fallback (`contains("HEAD")`) and the restore retry (`contains("index.lock")`) all depend on English git messages. A localized git (`LANG=de`, Git for Windows ships translations) silently loses the `unreadable` list. Set `LC_ALL=C` and `LANGUAGE=` in `command()`.

**N4. Low: job-object CPU accounting may undercount.** `AssignProcessToJobObject` runs after `spawn`. If git is the Git for Windows launcher (`C:\Program Files\Git\cmd\git.exe`, the usual PATH entry), it can start `mingw64\bin\git.exe` before the assignment, and then the real git's CPU time is not counted. The failure direction is benign (auto-off never fires), but the "includes the mingw child" claim is racy. Spawning suspended, or resolving `mingw64\bin\git.exe` directly, makes it exact.

**N5. Low: unreadable paths are not forced risky in their own plan.** A tracked file that was locked keeps its index version in the snapshot. The plan shows the `unreadable` list as text but does not set those paths (or the same paths in the next record) to `modified`. The mtime check usually catches the next record. Setting it explicitly costs one line.

**Checked and fine:**
- Tombstone pruning is sound once windows are complete: it prunes only when `ended <= min(live from)`, and no live stretch can overlap then.
- "Any file" windows are noisy (a repo whose snapshots keep failing makes other sessions' files shared) but fail safe, and are capped at 100.
- Widened queue jobs: `started` min, `at` max, `approximate` OR'd.
- Waiting→Idle and exit snapshots submit correctly for exited sessions.
- Async RPC threads: undo, plan and drop are serialized by the repo lock. A double click finds the files "already as before".
- `checkpoint.drop {worktree, root}` deletes only `refs/kelpie/<sid>/` through `root`. The plain-spelling match is limited to the removed folder.

### 3. The developer's remaining gap: "a user deleting a file during an idle gap cannot be told apart from an agent deleting it"
- **Effect:** undoing the turn re-creates the file, listed as a pre-ticked, clean **Restore** row (probe: `d.txt` restore, `clean`).
- **Judgement: acceptable as a disclosed limitation, not a silent destructive revert.**
  - The preview lists the file by name.
  - Re-creating a deleted file destroys no content; deleting it again is the remedy.
  - A restore never overwrites something present: a re-created path differs from the turn's end and shows as `modified`.
  - This differs from B2, where content the user wrote disappears.
- **Cheap improvement (non-blocking):** for a `D` entry, check the parent folder's mtime. NTFS and ext4 update it when an entry is removed. If it is older than `rec.started - slack`, nothing in that folder changed during the turn, so the deletion happened before it. Flag it `modified`, "removed before this turn started". If the folder changed during the turn, it is ambiguous; leave it as is. A turn-start gap snapshot (B2, comprehensive fix) removes this gap entirely.

### Re-review verdict
- **Still blocking:**
  - **B1:** close right after a turn's end loses its window and prunes the tombstone. Reproduced, with silent deletion of the closed session's file.
  - **B2:** a shell's (or the user's) gap edit to a file the agent then edits is reverted unflagged. Reproduced.
- **Not blocking:** N1-N5, M5 (disclosed), the deletion-in-gap limitation (acceptable with the preview), and the trim batching Low.
- Everything else from the first review is verified fixed. The H7 fix also removes the cause of the e2e flake (0 disables and about 2 s snapshots under full CPU load).

Status: DONE_WITH_CONCERNS
Summary: C1 and H2-H7 are verified fixed by re-run probes and tests, and H1 holds when the closed session's turn was already recorded. Two undo holes still silently revert another session's work: B1 (closing a session right after its turn ends drops that turn and its tombstone) and B2 (a gap edit by a shell or the user, followed by an agent edit to the same file, shows as clean). The deletion-in-gap limitation is acceptable with the preview.

---

## Final check (2026-10-24, B1/B2 fixes)

The status blocks above belong to earlier passes. This section's verdict is the current one.

I re-ran the probes against a fresh scratch copy of the current tree (`scratchpad\p6-review\probes3.rs`, real `Checkpoints` code, no daemon, no real repo). On the real tree, `cargo test -p kelpied` passes (126 passed, 1 ignored) and `clippy -D warnings` is clean. No test process of mine is left running.

### B1 (close right after a turn ends): fixed
```
B1: queued B job Ok(()); B log = Some((true, 0, 1))          <- tombstone kept, with one window
B1 PLAN  f.txt shared(b), g.txt shared(b)  ->  UNDO needs_confirmation; g.txt intact
B1q (job still in the real queue, repo lock held by another snapshot, close on its own thread):
     B log = Some((true, 2, []))  ->  the queued snapshot ran and was recorded before the tombstone; g.txt shared(b)
```
- `close_session` now takes the repo lock, turns the session's queued jobs into windows, and adds the open turn.
- The early returns for a dropped session (`run_job`, `push_record`) add a window instead of discarding the work.
- **Locking reviewed, no cycle.** `close_session` takes `queues` (briefly, then released), then `repo_lock`, then `queues`, then `inner`. `drain` releases `queues` before it takes `repo_lock`. `submit` takes `inner` and then `queues`, never `repo_lock`. `undo` takes `repo_lock` and then `inner`. Nothing takes `repo_lock` while holding `queues` or `inner`. The `if self.inner.lock()...` conditions drop their guard before the block (an `if`, not an `if let`), so `add_window` re-locking `inner` cannot self-deadlock.
- **Residual (Low, not blocking).** The ticker takes the work start, then `spawn_blocking(checkpoint_turn_end)`. If the close lands in the few milliseconds before that task calls `submit`, `submit`'s `dropped` check discards the job without a window. A human click comes at least a tick (250 ms) after the idle broadcast, so this is practically unreachable. Making `submit` call `add_window` for a dropped Turn job closes it fully.

### B2 (edit between turns, then the agent edits the same file): fixed
```
B2 PLAN  a.txt modified "changed between turns, before this turn started"
         d.txt modified "changed between turns, before this turn started"   <- the user's deletion is now caught too
B2 UNDO  needs_confirmation; a.txt keeps "user fix"
RESTART  between = ["a.txt"], truncated = false   <- the start look survives a daemon restart before the turn's end
```
- The start look's result is stored in `Log.start` (serde default, saved immediately) and moved onto the Turn record as `between` / `between_truncated` (also persisted).
- A failed look sets `truncated`, meaning "flag everything". A late look only flags more.
- The deletion-in-gap limitation I accepted in the re-review is now closed as well.
- **Hook-less boundary.** The start look needs typing since the last boundary, the same rule as the end. Prompts typed before the agent starts printing satisfy it. Repaints of an idle screen are correctly skipped.

### New hole found (Medium, not blocking): a backed-up queue merges turns across a start look and loses `between`
- **Where:** `coalesce` (`checkpoints.rs:1492-1501`) widens the *first* waiting Turn job of the session, even when that session's Start job is queued after it.
- **Probe:**
  ```
  QUEUE ORDER [(Turn, 300, 400), (Start, 300, 300)]        <- turn 2's end merged ahead of turn 2's start look
  COALESCED records [(1, Turn, ["a.txt","b.txt"], between [], approximate false)]
  COALESCED PLAN a.txt clean                                 <- user's between-turns edit, then agent edit: unflagged
  ```
  The late start look then attaches to the *next* turn.
- **When:** turn 1's snapshot must still be waiting when turn 2 *ends*. The repo queue has to be stalled for a whole gap plus a turn (a hung git or filter, a long undo, or many sessions in a slow repo). Rare, hence not blocking.
- **Fix (one line in `coalesce`, or in `push_record`):** do not widen a Turn job across a later Start job of the same session; append it instead. Better still, apply the principle the code already uses for a failed look ("not knowing is not nothing changed"): remember after which record number the start look ran. When a Turn record is pushed and no look ran after its base record, set `between_truncated = true`. That one rule also covers a skipped look for a hook-less turn and a look made before a daemon restart that followed a new baseline.

### Verdict
- **Blocking items remaining: none.** B1 and B2 are verified fixed by reproduction. N1-N5 were spot-checked in code: restart baselines count as windows (`windows_of` `r.from > 0`), `coalesce` keeps the latest `started`, `LC_ALL=C`/`LANGUAGE=` are set, CPU time below wall time falls back to the wall-time and busy-machine check, and unreadable paths are forced `modified`.
- **Recommended before or soon after landing (not blocking):** the coalesce/start-look ordering fix above, and the `submit` dropped-job window.
- **Still disclosed:** M5 (CRLF), trim batching (Low), and the definition-of-done items the developer lists as not run (`ui_tour.py` screenshots, typing latency during snapshots, 100k-file numbers).

Status: DONE
Summary: B1 and B2 are fixed and verified by reproduction (closed sessions keep their windows, and edits or deletions made between turns are flagged even when the agent edits the same file). The close-path locking has no lock-order cycle and the `between` set survives a restart. No blocking items remain; one rare, non-blocking hole is left: a backed-up queue merges a turn end ahead of its start look and loses `between`, fixed with a one-line coalesce rule.
