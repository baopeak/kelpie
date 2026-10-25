# Completion — Phase 8: Cross-review & best-of-N

Date: 2026-10-25 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-08](../261023-0048-multi-agent-ide-roadmap/phase-08-cross-review-best-of-n.md)

## Delivered
- **Review notes in the daemon** (`notes.rs`): `note.add/list/clear/keys` keyed by the worktree's exact path, `notes.changed` only to the user's connections and the matching session; `kelpied review add <file>:<line> "<text>"` for agents, own worktree only, `side`/`text` filled from `git diff` (hooks off, literal pathspecs, `LC_ALL=C`). The app's notes migrate once after a backup (newest 3 kept); older daemons keep notes in the app. The review card, Request changes (clears only what it sent), diff gutter and `@note` read the daemon store.
- **Session scoping hardened** (`auth.rs`): for session callers missing params are `{}` and any other non-object is refused, on every scoped method; a session's `run.create` never keeps `materialize`.
- **Review with…**: an agent of any vendor in the same worktree with the bounded (48 KB), redacted diff and an editable prompt, through the safe-send path.
- **Try with N agents**: 2–5 agents → one Run of independent `worktree: true` tasks; tasks start on create / approve / retry (no tick wait); five worktrees with links ready ≈ 1–1.9 s.
- **Shared `materialize`** (`kelpie-proto/src/materialize.rs`): used by the worktree dialog and run worktrees, only for an approved `.kelpie.yaml` setup (`.worktreeinclude` included, every worktree kind); overlapping link / copy entries never copy onto themselves; size estimate shown only for what will be brought.
- **Compare view** (lazy): status, time, cost, files, +/−, checks, turns, side-by-side diff; *Use this one* → Finish; losers are stopped at confirm (in parallel), and after Finish an unchanged loser gets a WIP commit, a non-forced removal and `branch -d` (unmerged branches stay); a changed loser is kept with the reason. Notes are cleared only after a removal succeeds. API level 13.

## Verification
- cargo: kelpied 196, kelpie 113, kelpie-proto 19 (scoping with null / array / string / number params, notes scoped and exact-key, migration, materialize overlap and approval, backup pruning); clippy `-D warnings` clean.
- vitest 360, typecheck, i18n 0 missing; staged commit's frontend checked alone in a clean worktree. Entry bundle 320 kB gz.
- GUI e2e `best_of_n.py` (3 scripted agents → compare → pick → Finish → losers handled, a loser that commits after confirm is kept, CLI notes in the gutter, nested-worktree notes, Review with…): 91/91 twice, again on the final build; regression finish, checkpoints, composer, notify, context_cost, attention green.
- [Review](code-reviewer-261025-1330-phase8-cross-review.md): H1–H4, M1–M4, R1–R5, L6 and cheap Lows fixed; reviewer's isolated-daemon probe (30 param shapes) leaked nothing.

## Not done / follow-ups
- "Unchanged" loser check compares file sizes, not content (no loss possible: WIP commit on a branch `-d` keeps).
- `ui_tour.py` and macOS not run; e2e cost / turns cells show "–" with stand-in agents.
