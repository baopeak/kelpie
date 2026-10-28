---
phase: 5
title: "Context, tokens & cost"
status: completed
priority: P1
effort: "3d"
dependencies: [4]
---

# Phase 5: Context, tokens & cost

## Overview
Per session: context fill, tokens, estimated cost; aggregates per day/project/agent; optional per-session cost cap.
Sources: ACP `usage_update` (chat), transcripts for Claude/Codex/OMP terminals. **No number without an exact source.**

## Verified against real transcripts (2026-10-24)
- **Claude Code** `~/.claude/projects/<slug>/<session>.jsonl`: `type:"assistant"` lines carry `message.model` and `message.usage`
  `{input_tokens, cache_creation_input_tokens, cache_read_input_tokens, output_tokens, cache_creation:{ephemeral_5m_input_tokens, ephemeral_1h_input_tokens}}`.
  One API message is written as several lines (one per content block) repeating the same usage → **dedupe by `message.id`** (or `requestId`)
  before summing cost. Context in use after a turn = input + cache_creation + cache_read + output of the last message.
- **Codex** rollout `~/.codex/sessions/YYYY/MM/DD/rollout-…-<thread-id>.jsonl`: `event_msg` / `payload.type:"token_count"` with
  `info.last_token_usage {input_tokens, cached_input_tokens, output_tokens, reasoning_output_tokens, total_tokens}` and
  `info.model_context_window` (exact window). `info` can be null (rate-limit-only records): skip. No price source for OpenAI models
  (and most users are on a ChatGPT plan) → tokens and context only, never a cost.
- **OMP** `~/.omp/agent/sessions/…jsonl`: assistant `message.usage {input, output, cacheRead, cacheWrite, totalTokens, cost:{total}}` —
  **exact cost**; `message.contextSnapshot.promptTokens`; no window in the file → no ring for OMP terminals (tokens + cost only).
- **Chat sessions** already carry `state.usage {used, size, cost?}` from their adapters (ACP `usage_update`, Claude/Codex adapters) → ring from it.
- **Claude prices** (USD / MTok, Claude API skill, cached 2026-10-06; cache write = 1.25× input for 5 m, 2× for 1 h):
  Fable 5.1 / Mythos 5.1 10 / 50, cache read 0.25 · Fable 5 / Mythos 5 10 / 50, read 1.00 · Opus 5.5 4 / 20, read 0.20 ·
  Opus 5, 4.8, 4.7, 4.6 5 / 25, read 0.50 · Sonnet 5.5, Sonnet 5 2 / 10, read 0.20 · Sonnet 4.6 3 / 15, read 0.30 ·
  Haiku 5.5 0.10 / 0.50, read 0.01 (prompt ≤ 100 K tokens; above: 0.50 / 2.50, read 0.05) · Haiku 4.5 1 / 5, read 0.10.
  Context: 1 M for all of them except Haiku 4.5 (200 K). Unknown model id → no cost (tokens only).
  Claude cost is an **API-price estimate** ("≈ $"), labelled as such: subscription users are not billed per token. OMP and ACP costs
  are the agent's own numbers (shown without "≈").

## Requirements
- Ring on tab + sidebar (amber ≥ 70 %, red ≥ 85 %); tooltip numbers; one-time hint at 85 %: Compact / Hand off.
- Usage page: cost by day / week / project / agent / model from the history scan cache.
- Cost cap (off by default): over cap → red chip + inbox; optional Esc only when the transcript belongs to that exact session.
- Mapping strictly by identity [RT-8]: Claude via `transcript_path` from hook payload (or `spec.agent_session_id`); Codex via stored `thread-id` (`daemon.rs:648`) → rollout file;
  ACP via adapter. No "newest file in folder" fallback; no id → no ring, no actions.

## Architecture
- Daemon broadcasts `session.turn {session, event, transcript_path?, agent_session_id?}` from `hook()` and on status turn-end [RT-8] (capability: app subscribers; API_LEVEL bump).
- App core `session_usage.rs` tails from the **end** of the file (reverse scan for the last usage record; `.zst` excluded — live transcripts are plain), debounced 1 s,
  only for visible or working sessions.
- Codex rollout resolver keyed by thread id. Prices: bundled `pricing.json`. Aggregates cached per file by (mtime, len).

## Related Code Files
- Create: `app/src-tauri/src/session_usage.rs`, `app/src-tauri/src/pricing.json`, `app/src/components/context-ring.tsx`, `app/src/components/cost-page.tsx`.
- Modify: `crates/kelpied/src/daemon.rs` (`session.turn` broadcast), `crates/kelpie-proto/src/lib.rs`, `app/src-tauri/src/agent_history.rs` (Codex rollout resolver, per-file usage cache),
  `app/src-tauri/src/lib.rs`, `app/src/lib/store.ts`, `app/src/components/sidebar.tsx`, `app/src/components/split-view.tsx`, `app/src/components/usage-meter.tsx`,
  `app/src/chat/acp-adapter.ts`, `app/src/lib/types.ts`, settings catalog, locales, docs.

## Implementation Steps
1. `session.turn` broadcast. 2. Parsers + reverse tail + fixtures. 3. Codex resolver. 4. Ring + hint. 5. Aggregation page. 6. Cost cap.

## Tests
- `cargo test`: parsers (Claude cache tokens, Codex token_count, malformed, truncated last line), reverse tail, two Claude sessions in one folder map to their own transcripts.
- e2e `context_cost.py`: fixture transcripts appended live → ring/$ match; session without id shows no ring; cap → inbox.

## Perf gate
- 20 visible sessions: app core CPU +≤ 0.5 pt; ≤ 64 KB read per update; warm aggregation ≤ 100 ms; cost page lazy.

## Success Criteria
- [x] Numbers equal fixture truth; never shown or acted on without exact mapping.

## Risk Assessment
- Format drift → parser returns None → ring hidden; parse-failure counter per agent in logs.
