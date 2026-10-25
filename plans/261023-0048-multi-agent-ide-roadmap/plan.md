---
title: "Multi-agent IDE roadmap"
description: "Bring the best of 2026 AI IDEs to Kelpie in a vendor-neutral way for many agent CLIs, each feature gated by tests and a performance budget."
status: pending
priority: P1
effort: "~52d"
tags: [roadmap, multi-agent, acp, performance, ux, security]
created: 2026-10-23
supersedes: [261023-0036-vibe-coder-gaps]
blockedBy: []
blocks: [261024-1205-phone-remote-control]  # phase 12 (notify.rs, mobile.rs) must land first
---

# Multi-agent IDE roadmap

## Overview
Kelpie already runs and keeps many agent CLIs alive. This roadmap adds what modern AI IDEs (Cursor 3,
VS Code agents window, Antigravity, Kiro, Zed/ACP, Codex app, Claude Code Agent View, Warp Oz) offer,
**only where it works across agents** (PTY, hooks, transcripts, ACP, git), and gates every feature with
tests and a performance budget. Supersedes [261023-0036-vibe-coder-gaps](../261023-0036-vibe-coder-gaps/plan.md).

- Feature list & per-feature decisions: [brainstorm-261023-0048-ai-ide-feature-decisions](../reports/brainstorm-261023-0048-ai-ide-feature-decisions.md)
- Research: [gap analysis](../reports/research-261023-0036-vibe-coder-gap-analysis.md) · [AI IDE survey](../reports/researcher-261023-0048-ai-ide-features.md) · [workbenches + ACP](../reports/researcher-261023-0048-multi-agent-workbenches.md)

## Brainstorm contract
- **Outcome**: someone running 5–20 different agents can see who needs them, feed context in,
  see each agent's changes and cost, review and merge them from one queue, and stay informed off-machine —
  while Kelpie stays as fast as today.
- **Constraints**: vendor-neutral first; reuse existing machinery (Board status, Runs, `sendToSession`/`typeInto`, `__kelpie`, `sys_stats`, `chat_argv`, keyring helpers);
  no UI-thread work; no always-on network service; secrets in OS keyring; never touch user index / working tree / agent config without explicit action; EN + VI; Windows first, macOS parity.
- **Non-goals**: tab/next-edit completion, LSP, codebase indexing, memories store, cloud agents, containers, PR bot, agent-driven browser,
  PTY replay, multiplayer, `store.ts` refactor (user decision). SSH close/reconnect: separate plan from [research-261023-0003](../reports/research-261023-0003-ssh-session-persist-resume.md).

## Phases

| # | Phase | Prio | Effort | Depends | Status |
|---|---|---|---|---|---|
| 1 | [Foundation: perf gate, tests, daemon capabilities, outbound text safety](phase-01-perf-gate-foundation.md) | P1 | 5d | — | completed |
| 2 | [Auto-update](phase-02-auto-update.md) | P1 | 3d | 1 | completed |
| 3 | [Attention queue & quick answer](phase-03-attention-queue.md) | P1 | 3d | 1 | completed |
| 4 | [Universal ACP chat](phase-04-universal-acp-chat.md) | P1 | 4d | 1 | completed |
| 5 | [Context, tokens & cost](phase-05-context-cost.md) | P1 | 3d | 4 | completed |
| 6 | [Turn checkpoints & artifacts](phase-06-turn-checkpoints.md) | P1 | 5d | 1, 5 | completed |
| 7 | [Review queue, Finish, overlap, auto-worktree](phase-07-review-queue-finish.md) | P1 | 5d | 6 | completed |
| 8 | [Cross-review & best-of-N](phase-08-cross-review-best-of-n.md) | P2 | 4d | 5, 7 | completed |
| 9 | [Composer & context attach](phase-09-composer-context.md) | P1 | 3d | 1 | completed |
| 10 | [Spec workspace](phase-10-spec-workspace.md) | P2 | 3d | 8, 9 | pending |
| 11 | [Event-triggered automations](phase-11-event-automations.md) | P2 | 3d | 1, 7 | pending |
| 12 | [Off-machine notify & limit actions](phase-12-notify-limits-redaction.md) | P1 | 3d | 1, 3, 5 | completed |
| 13 | [Processes, ports & preview](phase-13-processes-ports.md) | P2 | 3d | 1 | pending |
| 14 | [Agent setup: MCP, instructions, skills](phase-14-agent-setup.md) | P2 | 3d | 1 | pending |
| 15 | [Platform & hygiene](phase-15-platform-hygiene.md) | P3 | 3d | 1, 2 | pending |

Order: 1 → 2 → 3 → 4 → 5 → 6 → 7 → 9 → 12 → 8 → 10 → 11 → 13 → 14 → 15.
Parallel-safe once 1 lands: (2, 3), (9, 13), (13, 14). Phases 6/7/8 share git + diff view → sequential.

## Definition of done for EVERY phase
1. **Unit**: `cargo test --workspace` + `pnpm test` cover new pure logic and parsers.
2. **E2E**: a new `scripts/e2e/<feature>.py` (PowerShell-safe on Windows) against an isolated daemon; touched suites stay green.
3. **Static**: `pnpm typecheck`, `pnpm i18n:check` (0 missing), `cargo clippy -- -D warnings` on touched crates.
4. **Contracts**: every new daemon RPC declares its capability (`app` / `session`) and bumps `API_LEVEL`; new outbound text goes through `redact`; new typed text through `sendToSession`/`typeInto`.
5. **Perf gate**: `python scripts/perf/perf_gate.py --label phase-NN` passes global budgets (medians, ratio-based) + the phase budget; JSON in `plans/reports/perf-phase-NN.json`.
   Over budget → profile (WebView2 DevTools, `tracing` spans, `cargo flamegraph`) and fix before closing; never raise the baseline without a written reason.
6. **UX check**: keyboard + palette reachable; `ui_tour.py` screenshots dark / light / compact.
7. **Docs**: README features + `docs/user-guide.md`; `docs/architecture.md` for daemon services / protocol.
8. **Review**: code-review pass; findings fixed or rejected with evidence.

## Global performance budgets (vs `scripts/perf/baseline.json`, medians)
| Metric | Budget |
|---|---|
| Project switch median (≥ 200 samples) | ≤ baseline × 1.10 (p95 reported, warn > 52 ms) |
| Keystroke echo median (in-page) | ≤ baseline × 1.10 |
| Cold start → sidebar interactive | ≤ baseline × 1.05 |
| Idle CPU app + daemon, 20 sessions, 60 s | ≤ baseline + 1 pt |
| RSS app + WebView2 + daemon, 20 sessions (PTY children reported separately) | ≤ baseline + 30 MB |
| Entry JS chunk | ≤ baseline + 15 KB gz (new UI lazy-loaded) |
| Long tasks > 50 ms in standard scenario | 0 new |
| Daemon bench 20 × 2 MiB | ≤ baseline × 1.05 |

## UX rules
No new permanent bars: chips on existing rows/tabs, right-panel tabs, Board columns, palette commands, context-menu items, settings sections.
Status keeps colour + shape. Destructive actions preview and confirm. Shortcuts pass the conflict checker. Bell keeps its unread count; "needs you" is a separate dot.

## Success criteria
- [ ] All 15 phases meet the definition of done.
- [ ] Global budgets hold at the end (final perf report).
- [ ] Scripted "day in the life" e2e (5 agents, 3 vendors: plan → compose → run → review → finish) passes.

## Red Team Review
2026-10-23 · 4 reviewers (Security Adversary, Failure Mode Analyst, Assumption Destroyer, Scope & Complexity Critic) · 40 findings → 15 after dedup · all with file:line evidence · user: apply all.

| # | Finding | Sev | Disposition | Applied to |
|---|---|---|---|---|
| RT-1 | Daemon socket has no caller identity / per-method authorization; new agent verbs become escalation paths | Critical | Accept | Phase 1 (1b), DoD #4, phases 3, 6, 8, 11–14 |
| RT-2 | Update cannot replace a running detached daemon; restart path kills sessions and loses resume; no version check; overrides/keys | Critical | Accept | Phase 2 |
| RT-3 | Turn boundaries (Codex no start hook, UI-only markers, chat/remote), start-snapshot race, turn persistence, ref GC, watcher noise | Critical | Accept | Phase 6, phase 11 |
| RT-4 | Undo reverts concurrent sessions' work in shared folders; pathspec injection | Critical | Accept | Phase 6 |
| RT-5 | Finish loses uncommitted work / needs force; `check:` bypasses `.kelpie.yaml` approval and runs candidate-chosen commands | Critical | Accept | Phase 7 |
| RT-6 | Review queue duplicates Board status; no daemon review-notes store; Run `group` duplicates Run; run worktrees lack `link:`; dashboard grouping exists | High | Accept | Phases 3, 7, 8 |
| RT-7 | ACP sessions stuck Working; elicitation never Waiting; launch args duplicated in daemon | High | Accept | Phase 4 |
| RT-8 | Phase 5 sources missing (Codex lookup, hook events not broadcast, wrong-session fallback) | High | Accept | Phase 5 |
| RT-9 | Waiting only from Claude hooks/bell; quick answer bypasses Enter-delay helper; PowerShell e2e | High | Accept | Phase 3 |
| RT-10 | Composer paste break-out; duplicate paste path; `@folder` unsupported; redaction arrives after `@terminal` | High | Accept | Phases 1 (1c, 1d), 9 |
| RT-11 | Second notification engine; ntfy topic/deep-link leaks; blind "continue" timer; switch account loses conversation; Rust-only redact | High | Accept | Phases 1 (1d), 12 |
| RT-12 | Perf gate stability vs recorded spread; duplicate test hook | High | Accept | Phase 1 (1a), global budgets |
| RT-13 | Shell actions = injection; forged events; silent empty automations on parse failure; old-daemon misfire; ticker stalls | High | Accept | Phase 11 |
| RT-14 | `proc_tree` cannot list PIDs; PID reuse; duplicate tree walker; orphan semantics on Windows | Medium | Accept | Phase 13 |
| RT-15 | MCP copy bypasses vendor trust prompts / leaks env; second markdown parser, second TOML writer, `task_ref` duplicate of Runs | Medium | Accept | Phases 10, 14 |

### Validation decisions (user, 2026-10-23)
- `store.ts` split removed from the roadmap.
- Bell keeps unread-inbox count; separate "needs you" dot.
- Checkpoints on by default when p95 < 150 ms on a 10k-file repo; slow repos auto-disable with a one-time notice.

### Whole-Plan Consistency Sweep
- Decision deltas: redaction + paste sanitiser + daemon auth moved into phase 1; phase 12 consumes redaction; phase 15 drops 15e and depends on 1.
- Checked all phase files for stale terms: `__kelpieTest`/`KELPIE_TEST` (gone), `session.persist_now` (gone), Run `group` (gone), `task_ref` (gone),
  `spec_index.rs` (now TS), "spawn by preset" (gone), "existing daemon-version check" (replaced by version compare), `integrations.rs` keyring (now `kelpie_proto::secret_*`),
  `system-settings.tsx` for notifications (now `interface-settings.tsx`), `review-queue.tsx` (now Board + filtered panel).
- Dependencies updated: 6 ← 1, 5; 10 ← 8, 9; 11 ← 1, 7; 12 ← 1, 3, 5; 15 ← 1, 2. Execution order still satisfies every dependency.
- Unresolved contradictions: none.

## Contract from phase 2 (updates keep sessions running)
After an update the new app runs against the previous daemon until its sessions end. Every later
phase that adds a daemon RPC must check `daemon.info.api` in the app and hide (not break) the
feature on an older daemon; never assume app and daemon are the same release.

## Follow-ups carried from completed phases
- Phase 1: re-baseline cold start and idle CPU on a quiet machine (`perf_gate.py --update-baseline --reason`); the committed values were taken under load.
- Phase 1: e2e evidence for hand-off, quick-command typing and the Copy Context menu click (`phase3.py` needs an agent transcript folder); covered today by unit tests and `ipc.redact` checks.
- Phase 5: GUI perf metric (20 visible sessions, core CPU) not run; entry bundle margin 0.12 kB (338.48 of 338.60 kB gz) — the next entry-chunk addition must go lazy or re-budget.
- Phase 6: snapshots default off (0.35 s + 0.15 s start look on 10k files); 100k-file numbers, echo-during-snapshot and `ui_tour.py` not run; CRLF restore not byte-exact (disclosed).
- Phase 7: cold 20-worktree summary ≈ 2 s; checks join the Job object after start; no resume state after a crash past branch deletion; `ui_tour.py` not run.
- `attention.py` flaked once (11/12) under heavy parallel load, then 12/12 three times (incl. at 100 % CPU): watch it.
- Phase 12: manual checks pending (real Telegram bot, ntfy.sh, a real Claude usage limit, Codex resume after switch account, phone link); no limit signal for Codex/chat; non-Windows idle unit-tested only.
- Phase 9: fuzzy p95 > 30 ms under full CPU load (17–21 ms quiet); remote Windows hosts get POSIX quoting; `chat_ui.py` (real Claude/Codex) not run.
- Perf gate `bench` measures unfocused sessions, which the CPU-priority work (separate change) deliberately slows under load: compare bench with `daemon.set_priority {enabled:false}` or budget it separately.

## Unresolved questions
1. Gemini / OpenCode / Copilot: native ACP or adapter, exact flags — phase 4 spike.
2. Release host (public vs private GitHub Releases) and who holds the signing key — phase 2.
3. Linux format (AppImage vs .deb) and whether Linux users exist — phase 15.
4. Opt-in for scripts outside sessions calling `kelpied call` with the app token — default off, confirm in phase 1 review.
