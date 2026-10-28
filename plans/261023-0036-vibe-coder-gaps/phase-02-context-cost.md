# Phase 2 — Context, tokens & cost per session

## Context
Cost shows only at the end of a chat-view turn ([claude-adapter.ts:192](../../app/src/chat/claude-adapter.ts#L192)).
TUI sessions show nothing. The transcript reader already supports offset reads
([agent_history.rs:334](../../app/src-tauri/src/agent_history.rs#L334)) and knows each agent's
transcript folder per account.

## Requirements
- Per session: context used / window (%), tokens in/out this session, est. cost (API pricing).
- Ring on the tab + sidebar row; amber ≥ 70 %, red ≥ 85 %; tooltip with numbers.
- At ≥ 85 %: one-time hint "Compact" (types `/compact` for Claude) or "Hand off" (existing).
- Usage page: cost per day / week / project / model from all transcripts (ccusage-like), cached.
- Claude + Codex first; OMP if its transcript has usage; others show nothing (no fake numbers).
- No statusLine hijack; transcript tail only.

## Design
- Session → transcript file: Claude session id is known (resume flow) → file path; Codex via
  `agent_latest_session`. Tail from last offset only when the session is visible or working;
  1 s debounce after a hook event (`Stop`, `PostToolUse`) instead of polling.
- Claude: last assistant `message.usage` → context = input + cache_read + cache_creation (+ output);
  window from model id (200k / 1M table). Codex: last `token_count` event (`info.total_token_usage`,
  `model_context_window`).
- Prices: `app/src-tauri/src/pricing.json` bundled per release (no network).
- Aggregation for the Usage page reuses `scan_history` cache; per-file totals cached by
  (mtime, len) like turn counts.

## Files
- Create: `app/src-tauri/src/session_usage.rs` (tail + parse + aggregate), `app/src-tauri/src/pricing.json`,
  `app/src/components/context-ring.tsx`.
- Modify: `app/src-tauri/src/lib.rs` (commands), `app/src-tauri/src/agent_history.rs` (share path
  helpers, per-file usage cache), `app/src/lib/store.ts` (usage map keyed by session),
  `app/src/components/sidebar.tsx`, `app/src/components/split-view.tsx` (tab ring),
  `app/src/components/usage-meter.tsx` (link to cost page), `app/src/lib/types.ts`, locales.

## Validation
- `cargo test`: parsers on fixture lines (Claude with cache tokens, Codex token_count, malformed).
- e2e `scripts/e2e/context_cost.py`: fixture transcripts → ring % and cost match fixtures;
  hint appears once at 85 %.
- Perf: 20 visible sessions, CPU of app core unchanged (±1 %); 1.2 GB history aggregation warm < 100 ms.

## Risk / rollback
- Transcript format drift → parser returns None, ring hidden (never wrong numbers). Feature flag
  setting "Show context and cost".
