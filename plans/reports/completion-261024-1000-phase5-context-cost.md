# Completion — Phase 5: Context, tokens & cost

Date: 2026-10-24 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-05](../261023-0048-multi-agent-ide-roadmap/phase-05-context-cost.md)

## Delivered
- **Daemon**: `session.turn` broadcast (hook turn events with conversation id and `transcript_path`, `turn_end` when a working session goes quiet); status map pruned for closed sessions. API level 9; the app degrades against older daemons.
- **Transcript readers** (`session_usage.rs`): Claude `message.usage` deduped per `message.id` at its highest output (sub-agent files repeat a message with growing `output_tokens`), sub-agent transcripts `<slug>/<id>/subagents/agent-*.jsonl` counted for tokens and cost (context from the main file only, none after a `compact_boundary`); Codex `token_count` (`last_token_usage`, `model_context_window`), rollouts found from the UUIDv7 date; OMP exact `usage.cost.total`. Per-file aggregate cache; an update reads at most 64 KB. Prices and windows in `pricing.json` (Claude costs shown as "≈" estimates).
- **Hook path safety**: `transcript_path` accepted only as `<session config home>/projects/<slug>/<id>.jsonl`; UNC / `//` / `..` refused by text before file access; must stay inside `projects` after symlinks resolve.
- **UI**: context ring per session (amber / red), 85 % hint with Compact / Hand off only where the session supports them; Usage & cost page (day / week / project / agent / model, 7 / 30 / 90 days / all) from every transcript on the machine; optional per-session cost cap summed over a Kelpie session's conversations (survives `/clear`), red chip + inbox entry, optional Esc once (raw ESC; the old `session.input {key}` path never worked). Alerts and page folding are lazy chunks.

## Verification
- cargo: kelpie 58 (+4 ignored), kelpied 84, kelpie-proto 14; clippy `-D warnings` clean on all three crates (pre-existing lints fixed alongside).
- vitest 133 incl. `usage-format.test.ts`, `usage-report.test.ts`; typecheck, i18n 0 missing.
- GUI e2e `scripts/e2e/context_cost.py` on an isolated daemon: 26/26 (fixtures for both Claude layouts, sub-agent dedupe, wrong hook path ignored, per-session cap across a conversation change, Esc exactly once, page totals). Regression: `attention.py` 12/12.
- Perf: entry bundle 338.48 kB gz (limit 338.60); warm Usage report on real transcripts 86 ms (budget 100 ms).
- [Review](code-reviewer-261024-0910-phase5-context-cost.md): H1, H2, M1–M4, L1–L4, L6 fixed; L7, L8 documented in `docs/architecture.md`; L9 accepted.

## Not done / follow-ups
- GUI perf metric "20 visible sessions, core CPU +≤ 0.5 pt" not run (machine under load; fold into the quiet-machine re-baseline).
- Entry bundle margin is 0.12 kB: the next phase that adds to the entry chunk must move something lazy or re-budget.
- Codex context % includes reasoning tokens (L7); not checked against codex-rs.
