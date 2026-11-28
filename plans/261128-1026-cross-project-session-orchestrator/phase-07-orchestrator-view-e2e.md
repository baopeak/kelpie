# Phase 7: Orchestrator view, docs, real-app e2e

## Requirements

1. **Orchestrator view** (an overlay next to the cost page; also on the phone at reduced size):
   - machine: available RAM, pressure state and the reserve line;
   - per project, sorted by tier: sessions, RAM now and peak, held commands with their reason, queue entries, tokens and cost today and in this window against budget;
   - a **priority ranking**: projects grouped by tier, drag to reorder; sessions under each with their override, boost (time left) and reservation;
   - actions:
     - set tier, rank or session priority;
     - boost or unboost;
     - reserve resources;
     - Run now (skip a hold);
     - Park or Resume;
     - Stop child;
     - Deliver held mail now.
2. Status bar: a small RAM and pressure meter, plus a "n held" badge.
3. Docs:
   - `docs/user-guide.md`: a new "Resources and budgets" section;
   - `docs/architecture.md`: Admission, Gate, Pressure, Ledger, and hook blocking for PreToolUse;
   - `sub_guide.rs`: queue and hold wording for agents.
4. **E2E** (`scripts/e2e/orchestrator_resources.py`), on an isolated `KELPIE_ENDPOINT` and data dir, never the real daemon:
   - (a) tier set → policy persisted;
   - (b) run with 4 tasks and `max_agents=2` → queued and then admitted;
   - (c) fake-hook agent heavy command held while background, admitted when critical;
   - (d) forced `tight` → child demoted and restored;
   - (e) mail to an idle-past-TTL session held, then delivered;
   - (f) cost page totals from fixture transcripts;
   - (g) boost a background session during two builds: its command starts at once with the larger share, and on expiry the shares return to normal;
   - (h) a reservation that does not fit is refused with the numbers; one that fits keeps its RAM and cores free from others' new heavy commands.
5. Perf gate run with the monitor on; the baseline is not updated to hide a cost.

## Validation

All of the above passes, plus `cargo test --workspace`, `pnpm typecheck && pnpm test && pnpm i18n:check` and the echo bench.
