# Phase 6: Token ledger, budgets, cache-aware idling, lead/sidekick routing

## Context

The app reads tokens and cost per session from the agent's transcript (`session-usage.ts`). It has a per-session cost cap with Interrupt, Compact and Hand off. The daemon parses usage-limit resets (`limit_reset.rs`) and can continue at reset (`scheduled_input.rs`). Nothing totals per project or per window, and nothing acts with the window closed.

Facts from the research:

- Cache reads cost 0.1× and writes 1.25× (5-minute TTL) or 2× (1-hour TTL).
- A session that idles past its TTL pays a full re-read on its next turn.
- A message from another session or a timer that wakes an idle session pays it too.
- Claude transcript `message.usage` records are additive and need dedupe of streaming placeholders. Codex `token_count` is cumulative.

## Requirements

1. **Daemon ledger:**
   - A Rust reader of the transcripts the daemon already matches by conversation id: Claude `~/.claude/projects/**.jsonl` via the hook's `transcript_path`, Codex `~/.codex/sessions/**/rollout-*.jsonl`, and OMP as today.
   - It reads incrementally (offset per file) on `Stop`/`PostCompact` hooks and on turn end.
   - It records input, output, cache read and cache write tokens, cost by the existing price table, and the time of the last cached turn.
   - Totals are per session, per project and per day.
   - It runs only when the ledger is on; it is the source the app's cost page reads (`usage.get`). The app's own reader stays as the fallback for older daemons.
2. **Budgets:**
   - Per project, per day (cost or tokens) and per 5-hour window (share of the window), set in `resource-policy.json`.
   - At 80 %: warn on the row.
   - At 100 %:
     - a background project's queued and automation work waits for the next window (phase 3 queue, "resume at reset");
     - its sub-session spawns are refused with the reason;
     - a running session is never stopped mid-turn. The existing cost cap actions apply.
3. **5-hour window pace:**
   - Compute the block start from transcript timestamps (the ccusage block logic), the burn rate and the predicted exhaustion.
   - When it is predicted before reset, defer background and automation work, and show "at this rate the limit is reached at 15:40".
4. **Cache-aware idling.** The TTL is per agent. Measured on this machine: Claude main conversations 1 h, Claude subagents 5 min; unknown agents are assumed to use 5 min.
   - Mail between sessions (`subs::Mailboxes`) and scheduled inputs addressed to a session idle past its cache TTL are **held** while the sender is not critical. The sender is told "delivered when X next works, or now with Deliver now". This follows Claude's `crossSessionInbound: hold`.
   - Before parking (phase 5) or a planned long idle, offer Compact while the cache is still warm.
   - No keep-alive pings: they cost more than they save.
5. **Lead / sidekick pairs (after Devin Fusion: a frontier lead plans and reviews, a cheaper sidekick implements, they exchange only briefs and results):**
   - A policy per project: `pair: {lead: {agent, model}, sidekick: {agent, model}}`. A run task or sub-session marked `implement` starts with the sidekick, and one marked `plan`/`review` starts with the lead. The lead session gets a guide line (in `sub_guide.rs`) saying it should hand mechanical work to a sidekick child and review its diff, sending a brief, not the conversation.
   - A run template "Plan → Implement → Review" built on existing runs: lead plans, sidekick implements in a worktree, lead (or another agent, e.g. Codex reviewing Claude) reviews; a failed review sends feedback to the sidekick once, then to the person.
   - `sub_stats.rs` records cost and outcome per (role, agent, model), so the view shows what a pair saved compared with the lead doing it all, as Fusion's `/session-stats` does. Defaults stay off until the owner picks a pair; Devin's own `/fusion` keeps working inside a Devin session and is simply measured.
6. **Model tiers:**
   - `resource-policy.json` maps tier to the default model for sub-sessions and run tasks per agent, for example background → a smaller model.
   - It applies only at spawn (`subs::model_args`), never mid-conversation, because switching loses the cache.
   - `sub_stats.rs` gains tokens and cost per agent and model, so `kelpie_agents` can show the cost per finished task.

## Files

New `crates/kelpied/src/usage_ledger.rs` (+ `usage_ledger/claude.rs`, `codex.rs`, fixture tests), `resource_policy.rs`, `subs.rs` (mail hold), `scheduled_input.rs`, `sub_stats.rs`, `daemon.rs`, proto, `app/src/lib/session-usage.ts` (source switch), `cost-page.tsx` (project/day/window view), `locales/*`.

## Validation

- Unit, with fixture transcripts (redacted): dedupe of Claude streaming records, Codex cumulative deltas, block computation, budget thresholds and the mail hold.
- The cost page totals match the app's per-session readings on the same transcripts.
- No transcript text is logged. Only numbers are stored.

## Risk / rollback

- Risk: transcript formats change. The parsers are fixture-tested, and an unknown record is skipped and counted.
- Rollback: `usage.ledger=false` returns the app reader.
