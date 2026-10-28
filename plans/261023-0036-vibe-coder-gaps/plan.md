# Vibe-coder gaps: review, cost, finish, reach, update

Status: superseded by [261023-0048-multi-agent-ide-roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) (all phases folded in) · 2026-10-23
Research: [gap analysis](../reports/research-261023-0036-vibe-coder-gap-analysis.md)

## Brainstorm contract

- **Outcome**: a vibe coder running 5–20 agents across projects can, without leaving Kelpie,
  see what each agent changed per turn and undo it, see context/cost before it hurts, take a
  worktree from "agent done" to "merged and cleaned up", get pinged off-machine, survive usage
  limits, and always run the latest Kelpie.
- **Constraints**: no UI-thread work; no new always-on network service; secrets in OS keyring;
  never touch the user's index/working tree/statusLine without an explicit action; Windows first,
  macOS parity; EN + VI strings; every phase has an e2e script.
- **Non-goals**: own LLM chat, MCP marketplace, cloud sync server, telemetry, container sandbox,
  remote Allow/Deny via Telegram (round 2). SSH close/reconnect lives in its own plan
  ([research](../reports/research-261023-0003-ssh-session-persist-resume.md)).
- **Acceptance**: see each phase; overall — project switch p95 stays ≤ 52 ms, idle CPU of
  app + daemon with 20 sessions unchanged (±1 %), `cargo test`, `pnpm typecheck`,
  `pnpm i18n:check` and existing e2e stay green.

## Phases

| # | Phase | Gap | Prio | Size | Depends | Status |
|---|---|---|---|---|---|---|
| 1 | [Auto-update & release channel](phase-01-auto-update.md) | G6 | P0 | M | — | todo |
| 2 | [Context, tokens & cost per session](phase-02-context-cost.md) | G2 | P0 | M | — | todo |
| 3 | [Turn checkpoints: diff & undo per agent turn](phase-03-turn-checkpoints.md) | G1 | P0 | L | — | todo |
| 4 | [Finish worktree, overlap watch, cross-review, best-of-N](phase-04-finish-worktree.md) | G3, G7 | P0/P1 | L | 3 (diff view reuse) | todo |
| 5 | [Off-machine notifications & usage-limit actions](phase-05-notify-limits.md) | G4, G5 | P0 | M | 2 (limit reason) | todo |
| 6 | [Processes & ports per session](phase-06-processes-ports.md) | G9 | P1 | M | — | todo |
| 7 | [Agent setup: MCP servers & instruction files](phase-07-agent-setup.md) | G8 | P1 | M | — | todo |
| 8 | [Platform & hygiene: Linux, git basics, config export, store split](phase-08-platform-hygiene.md) | G10–G13 | P1/P2 | L | 1 (Linux bundles in release) | todo |

Order: 1 → 2 → 3 → 4 → 5, then 6/7 in parallel (no shared files), 8 last.
Parallel-safe pairs: (1, 2), (6, 7). Phases 3 and 4 both touch `git-panel.tsx` / diff view → sequential.

## Shared rules for every phase
- New Rust modules: daemon work in `crates/kelpied/src/`, desktop-only work in `app/src-tauri/src/`.
- New UI strings go through `t()` with VI entries; run `pnpm i18n:check`.
- New settings go into `app/src/lib/settings-catalog.ts` (searchable) with a safe default.
- Docs: README feature list + `docs/user-guide.md` section per user-visible feature;
  `docs/architecture.md` when a daemon service or protocol message is added.

## Unresolved questions
See the research report's list (checkpoint default, price table updates, release host and
signing key, Linux format, remote approval scope).
