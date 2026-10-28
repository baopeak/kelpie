# Completion — Phase 6: Turn checkpoints & artifacts

Date: 2026-10-24 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-06](../261023-0048-multi-agent-ide-roadmap/phase-06-turn-checkpoints.md)

## Delivered
- **Daemon** (`checkpoints.rs` + `checkpoints/`): baseline at spawn, end-of-turn snapshots chained end N-1 → end N (Claude `Stop`, Codex `agent-turn-complete`, ACP turn end, Waiting/exit, hook-less "typing then quiet" labelled approximate); temp index copied from `.git/index`, `add -A --ignore-errors`, `write-tree`, `commit-tree`, `update-ref` under `refs/kelpie/`; `GIT_LITERAL_PATHSPECS=1`, `LC_ALL=C`, hooks pointed at an empty folder, normal priority. One queue per repo; records in `<data_dir>/checkpoints/`, numbering survives restart, GC of own sessions only, keep last 50, `checkpoint.drop` on worktree removal. `checkpoint.*` is user-role only. API level 10.
- **Undo safety**: every record carries its full diff stretch (`from..at`); overlap with any other session's stretch, closed sessions (tombstones), failed/skipped snapshots ("may have touched any file" windows), files older than the turn, and a detection-only turn-start look (`between`: edits made between turns, by the user or a shell) all need per-file confirmation, naming the other session. A turn with no start look after its base flags every file. Preview of every restore/delete; nothing changes until confirmed. Undo is its own recorded step.
- **UI**: Turns tab (timeline, per-turn files, before/after diff, artifacts rendered, comment → review note), undo dialog bound to the session and turn it was opened for; Settings → Agents toggle. Lazy chunks; `DiffView` made lazy (entry 338.48 → 327.08 kB gz).
- **Watcher** ignores `refs/kelpie` and the temp index (0 `fs-changed` from ref writes).
- **Default off**: changed turn p95 0.35 s + turn-start look p95 0.15 s on 10k files (target 150 ms not reachable with the git CLI on Windows); a repo over 1 s of git CPU twice auto-disables for the session and says so once. `.git` growth ≈ 870 KiB / 100 turns.

## Verification
- cargo: kelpied 125 (41 checkpoint tests incl. byte-identical index/HEAD, untracked, ignored, literal pathspecs, restart numbering, GC, hooks not run, locked file, lock wait not "slow", idle-gap overlap, closed session, close-right-after-turn, between-turn edits, stalled queue), kelpie 58, kelpie-proto 14; clippy `-D warnings` clean.
- vitest 140, typecheck, i18n 0 missing.
- GUI e2e `checkpoints.py` on an isolated daemon: 55/55, quiet and under full CPU load (controller re-ran 55/55). Regression: `context_cost.py` 26/26, `attention.py` 12/12. Bundle 327.08 kB gz.
- [Review](code-reviewer-261024-1200-phase6-turn-checkpoints.md): C1, H1–H7, M1–M8 (M5 disclosed), B1, B2, N1–N5 and the stalled-queue hole fixed; final check: no blocking items. The first e2e flake was traced to below-normal git priority starving under load (fixed).

## Not done / follow-ups
- 100k-file numbers (fixture build did not finish), keystroke echo during snapshots, `ui_tour.py` screenshots.
- M5: restored files follow the repo's line-ending settings, not byte-exact (disclosed in the dialog and user guide).
- CPU-time slowness accounting is Windows-only; trimming old refs in the same `update-ref` call.
- Faster snapshots would need in-process git (`gix`).
