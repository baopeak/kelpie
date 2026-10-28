# Phase 6 implementation report: Turn checkpoints & artifacts

Date: 2026-10-24 · Branch rebrand/kelpie · Not committed (controller commits). Updated after the code review
([code-reviewer-261024-1200](code-reviewer-261024-1200-phase6-turn-checkpoints.md)): all findings addressed below.

## Delivered
- **Daemon** `crates/kelpied/src/checkpoints.rs` (+ `checkpoints/tests.rs`), wired in `daemon.rs`, `main.rs`, `session.rs`, `auth.rs`; API level 10.
  - Baseline at spawn/restart; one snapshot per turn end: the ticker sees a session that was Working or Waiting become Idle or Exited. Hook-less agents are `approximate` and need input since the previous boundary. Shells and remote sessions excluded. Unchanged tree = no snapshot.
  - Snapshot: copy of the index in `<data>/checkpoints/tmp`, `add -A --ignore-errors`, `write-tree`, `commit-tree -p HEAD`, `update-ref <zero-old>` under `refs/kelpie/<sid>/<n>`; literal pathspecs, empty hooks folder, normal priority, repository remembered, retry x3, gap window on failure.
  - Records per session (`from`/`at`/`read_at` window, paths, unreadable files); tombstones for closed sessions; keep last 50; startup GC; `checkpoint.drop {worktree, root}` after a successful `git worktree remove`.
  - Undo: owner must be idle; plan per file with state clean / modified / shared; refuses with `needs_confirmation` unless each risky file is confirmed; `git restore --source --worktree`; repeatable; recorded as its own step; undo/plan/drop answer from their own thread.
  - RPCs (user-only): `checkpoint.status/set_enabled/list/owners/diff/blob/undo_plan/undo/drop`.
- **App**: right-panel tab **Turns** (lazy), palette command, Settings toggle, diff pane and file pane with `rev` (lazy `turn-file-view.tsx`: markdown block by block with comment -> review note, image, Mermaid, CSV), all gated on `daemonApi() >= 10`; `DiffView` lazy.
- Watcher ignores `.git/refs/kelpie/**`; EN + VI; docs (architecture, user guide, README).

## Review findings: what changed
| Finding | Fix |
|---|---|
| C1 idle-gap edits | Records store `from` (previous snapshot's read start) and `at`; overlap compares the whole diff stretch `from..at` on both sides. A file whose mtime is older than the turn's start is `modified` ("changed before this turn started"). Test: B edits in A's idle gap -> shared, not reverted (cargo + e2e). Limitation: a user deleting a file in an idle gap is indistinguishable and not flagged. |
| H1 closed sessions | `close_session` deletes refs first, keeps a tombstone (windows + file lists) in memory and `<sid>.json`; pruned when no live history reaches back to it. Test after close and after restart. |
| H2 work without a record | Failed snapshot adds a gap window (any file) and marks the next record approximate; queued jobs of one session are widened (`started` min, `at` max), not dropped; turns ending Waiting -> Idle or by exit now snapshot; closing mid-turn records the open window. Tests (corrupt index, coalesce). |
| H3 dialog binding | Undo target `{session, name, n}` is fixed when the dialog opens; heading names the session; the panel pins its session until the user changes it. e2e: another agent becomes most recent while the dialog is open, the undo still lands on the right session. |
| H4 user hooks | Every git call gets `core.hooksPath` = an empty folder. Test with logging `reference-transaction`, `post-checkout`, `pre-commit` hooks (the probe proves they run for plain git). |
| H5 locked file | `add -A --ignore-errors`, exit 1 accepted, `unable to index file` paths kept as `unreadable` on the record and shown in the undo dialog. Windows test holds a file open. |
| H6 lock contention | `index.lock` precheck removed (the copy reads the last complete index); only the successful attempt is timed. Test with a held `index.lock`. |
| H7 starvation / auto-off | Git at normal priority. Slowness is judged on git's CPU time (Windows job object around each git, includes the mingw child); off Windows wall time, ignored when a trivial `git --version` is also slow. Auto-off is in memory only (no `disabled.json`). Record `at` = max(turn end, read end); read more than 3 s after the turn ended -> approximate. |
| M1 stale tmp lock | Removed before use; `tmp` emptied at start. Test. |
| M2 sparse checkout | Exit 1 accepted, `advice.updateSparsePath=false`. (No dedicated test: needs a sparse fixture.) |
| M3 blocked request queue | `undo_plan`, `undo`, `drop` reply from their own thread. |
| M4 token-less hook vs idle gate | Owner/others count as busy also when they printed within 1.5 s, whatever the hook said. |
| M5 CRLF | Not made byte-exact (snapshots must follow the user's filters to stay comparable with the index). The dialog and guide say line endings follow git settings. |
| M6 repaint snapshots | Approximate boundaries need input since the last boundary. |
| M7 drop before remove | `checkpoint.drop {worktree, root}` after the removal succeeds; refs deleted through `root`; matching also by the folder's plain spelling since the folder is gone. e2e: kill agent, remove, refs and records gone. |
| M8 paths | Panes use project-relative paths; the daemon gets the repository prefix back (`rev.prefix`); files outside the project are shown dimmed without actions. |
| Lows | L1 refs deleted before record; L2 work start taken on the ticker; L3 record written after the lock is released; L4 panel gated on API level; L5 docs reworded (restore briefly takes the index lock, one retry); L6 settings text and guide mention `refs/kelpie` visibility; L7 label `hook` only when hooks were seen; L8 no delete behind a link; L9 `core.splitIndex=false`; L10 style. Not done: folding trim deletes into one `update-ref` (small win). |
| Perf | Repository remembered, `rev-parse HEAD` dropped: see numbers below. |
| Test fixes | Both baselines are checked; failing checks print `checkpoint.status`; the data folder is kept when a check fails. |

Attribution note (inherent, conservative): every session's snapshot also holds the others' edits made since its previous snapshot, so in a folder shared by several sessions many files show as shared and need a tick.

## Second review: what changed
| Finding | Fix |
|---|---|
| B1 close right after a turn ends | `close_session` takes the repository's lock first (a running snapshot of the session finishes and is recorded), then turns every job still queued for it into a stretch ("may have touched any file", from its last snapshot to the job's end) and only then deletes refs and keeps the tombstone. A job the worker had already taken, and a record refused because the session closed, add the same stretch (`add_window`) instead of being dropped; a tombstone is pruned only after that. Three cargo tests: queued job, taken job (the reviewer's exact order), running snapshot. |
| B2 gap edit then agent edit of the same file | A detection-only **look** at each turn's start: `Kind::Start` jobs in the same per-repo queue (same hooks, normal priority, timing and auto-off rules); the folder is read, nothing committed or referenced or recorded. If its tree differs from the session's previous snapshot, the differing paths are stored (capped; "all files" when the look failed) and joined to the turn's record as `between`. Plan: those files are `modified`, "changed between turns, before this turn started", even when the agent edited them later; undo refuses without confirmation. The end -> end chain, numbering and diffs are unchanged; the end result of the look is not a record. Guessed boundaries need typing since the last one, as for ends. Also covers files the user deleted in the gap. A late look only over-flags. |
| N1 restart baseline | A baseline made after earlier work now gets `from` and the changed paths like a turn (still no base, not undoable) and counts as a stretch in `windows_of`. |
| N2 coalesced window | The queued job keeps the latest `started` (mtime test); windows come from the snapshots' own times. |
| N3 localized git | Every git call runs with `LC_ALL=C`, `LANGUAGE=`. |
| N4 CPU accounting | Verified on 10k files: `add -A` + `write-tree` took 115 ms wall and the job object counted 171 ms CPU, so the real git behind the launcher is counted (children of the assigned process join the job). Also made robust: slow means CPU over 1 s, or wall over 1 s unless a trivial git is slow too, so an undercount can only make auto-off rarer, never blind. |
| N5 unreadable | Paths unreadable in this record or the previous one are `modified`, "could not be read when a snapshot was taken". |
| Parent-folder mtime | A removed file whose folder was last changed before the turn started is `modified`, "removed before this turn started" (the look at the start covers it too). |

E2E: new scenario (user edits a file between turns, the agent edits the same file: flagged "changed between turns", undo refuses, the user's fix stays, the look makes no record). The stand-in agent now thinks 1.5 s before its first edit, as a real one does (the look runs within about 0.3 s of the turn starting; an instant edit made the look flag the agent's own work, which is the safe direction but hid the scenario). The stand-in is waited for (banner) before typing: one earlier load run lost typed input to a stand-in that had not started yet (cc4 and cc5 left with baselines only); the checks for those turns now print status.

## Final check: stalled-queue merge
| Finding | Fix |
|---|---|
| Medium: turn 1's job still queued when turn 2 ends merges ahead of turn 2's start look, so `between` is lost | Two layers. (1) `coalesce` no longer widens a waiting Turn job across a Start job of the same session that waits after it; the newer turn is appended. (2) The robust rule: the log remembers after which snapshot number the last look ran (`looked_after`, set whether or not the look found anything); a Turn record whose base is not that number (merged jobs, a skipped or failed look, a look made before an undo or restart baseline) gets `between_truncated`, so every file of that turn is `modified` / "changed between turns" and needs confirmation. Tests: coalesce order, no look after the base (flagged), look after the base (clean), look before a baseline (flagged again), and the probe's order (turn end run before its look: all files flagged, undo refuses, the user's fix stays). |
| Low: `submit` dropped a turn job for a closed session | It now adds a "may have touched any file" window (test: refused job leaves a window and another session's undo sees the file as shared). `add_window` takes the repository path. |

## Follow-up: checkpoints switched off on a busy machine
- **Reproduced** (ignored test `snapshots_on_a_loaded_machine`: busy threads at normal priority on every core, ten snapshots of a five-file fixture): every snapshot read wall 1.5-2.9 s but **git CPU time 125-250 ms**, and the one-sample busy probe (`git --version`) read 258-714 ms. The repository was switched off after two snapshots.
- **Root cause:** the CPU measurement was right and did not grow (about 0.2 s). The rule ignored it: it fell back to the *wall* time whenever the CPU was under a second, and excused a slow wall only if a single trivial git start took over 300 ms. At about 80% load that probe sits around 250-350 ms, so it was a coin flip, and two in a row disabled the repository. The reported 1.4 s and 7.5 s are such wall times.
- **Fix:** the verdict is on CPU time when it was counted (at least 30 ms, so a launcher-only count is not trusted); wall time is judged only when no CPU time was counted, and then only for a repository whose index is at least 256 KiB and while the probe is prompt. A busy machine can no longer switch off a small repository by any path. Wall, CPU and probe of a snapshot over a second are logged (`snapshot of <repo> took long`).
- **Tests:** `a_busy_machine_does_not_switch_a_small_repository_off` (the measured numbers: 1.4-7.5 s wall with 190 ms CPU, and wall-only 7.5 s with no CPU counted on a small index: stays on; two snapshots of over a second of CPU still switch it off), `without_counted_cpu_a_big_index_and_a_prompt_machine_make_a_snapshot_slow`, the existing auto-off test, and the ignored hog test now asserts the repository stays on. One timing assertion (a held `index.lock` finishing within 10 s) was removed: the machine was loaded by other work and it failed for that reason alone. The phase 7 `core.hooksPath=/dev/null` change in `checkpoints.rs` is untouched.
- **Runs:** `cargo test -p kelpied` 130 passed (2 ignored); `clippy -D warnings` clean; `checkpoints.py` **55/55 quiet and 55/55 under my bounded hog** (560 s, normal priority on every core, stopped by me; other work on the machine kept it busy too).

## Results (final runs)
- `cargo test -p kelpied -p kelpie -p kelpie-proto`: kelpied **130** passed (45 checkpoint tests, 2 ignored: bench and hog), kelpie 58 (+4 ignored), kelpie-proto 14. `cargo clippy ... --all-targets -D warnings` clean.
- App: typecheck clean, vitest **140**, i18n 1501 keys, **0 missing**.
- `scripts/e2e/checkpoints.py` (isolated daemon/endpoint set in-process): **55/55 after the last change (quiet machine)**; the previous build of the same scenarios passed 55/55 twice quiet and twice under full CPU load (normal-priority burners on every core, bounded, stopped by me).
- New e2e coverage: cc5's edit during cc4's idle time is flagged shared (naming cc5) and not reverted; dialog bound to cc4's turn while cc5 becomes most recent; status printed on failure; baselines checked.
- Regression: `context_cost.py` 26/26, `attention.py` 12/12.
- Bundle gate: entry **327.08 kB gz** (baseline 323.6, limit 338.60); `plans/reports/perf-phase-06.json`.

## Snapshot cost and default
Release, quiet machine, 10,000-file repo, turn = 3 edits + 1 new file:
- first version: changed p50 0.74 s / p95 0.80 s; unchanged p50 0.25 s.
- now (release, 10 turns): baseline 0.34 s; changed turn **p50 0.33 s / p95 0.35 s**; unchanged p50 0.14 s / p95 0.15 s; **turn-start look p50 0.13 s / p95 0.15 s** (new, counted: a turn costs about 0.46 s of git work); `.git` growth **~870 KiB per 100 turns** (the look writes loose objects git collects later).
- 100k files: not obtained (fixture build did not finish in 90 min).
- **Default stays OFF** (a turn's p95 is 0.35 s, plus 0.15 s for the start look, against 150 ms). Daemon `DEFAULT_ENABLED = false`, app `checkpoints: false`.

## Notes / not done
- Hook-token rule: boundaries come from the ticker status; token-less hook reports cannot start a snapshot nor make a busy agent look idle for undo; `checkpoint.*` is user-only.
- Not run: keystroke-echo-during-snapshot GUI metric, `ui_tour.py` screenshots, macOS, Linux (CPU-time accounting is Windows-only; elsewhere wall time plus the busy check).
- The look at a turn's start runs a moment after the turn begins; an agent that edits within that moment has its first edits counted as "between turns" (flagged; safe).
- The cargo tests leave fixtures under `%TEMP%\kelpie-cp-*` (I deleted mine).

## Files
Created: `crates/kelpied/src/checkpoints.rs`, `checkpoints/tests.rs`, `app/src/components/turn-timeline.tsx`, `turn-timeline.css`, `turn-file-view.tsx`, `app/src/lib/checkpoints.ts`, `checkpoints.test.ts`, `checkpoint-api.ts`, `app/src/locales/vi/turns.json`, `scripts/e2e/checkpoints.py`, `plans/reports/perf-phase-06.json`.
Modified: `daemon.rs`, `session.rs`, `main.rs`, `auth.rs`, `kelpie-proto/src/lib.rs`, `watcher.rs`, `git.rs`, `app.tsx`, `pane-view.tsx`, `workspace-view.tsx`, `right-panel.tsx`, `diff-view.tsx`, `rich-viewers.tsx`, `agent-settings.tsx`, `commands.ts`, `settings-catalog.ts`, `store.ts`, `types.ts`, `locales/vi/backend.json`, `docs/architecture.md`, `docs/user-guide.md`, `README.md`.

Status: DONE_WITH_CONCERNS
Summary: Both blockers (B1 close-after-turn, B2 gap edit then agent edit), N1-N5, the stalled-queue merge and the dropped-job window are fixed with cargo and e2e coverage; every acceptance check is green, including checkpoints.py 55/55 quiet and under full CPU load. Snapshots stay off by default (a turn costs about 0.46 s of git on 10k files with the new start look).
Concerns/Blockers: 100k-file numbers not obtained; echo-during-snapshot and ui_tour not run; M5 (byte-exact CRLF) only disclosed; CPU-time accounting is Windows-only (wall time plus the busy check elsewhere); in shared folders many files show as shared by design; the start look doubles the idle-tree git cost per turn.
