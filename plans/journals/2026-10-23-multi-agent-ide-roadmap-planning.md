# 2026-10-23 — Multi-agent IDE roadmap: research, plan, red-team

- Researched 2026 AI IDEs (Cursor 3, VS Code agents window, Antigravity, Kiro, Zed/ACP, Codex app, Claude Code Agent View, Warp Oz) and scored every feature against four rules: generic across agents, real pain, free when unused, no new permanent chrome. Decision record: `plans/reports/brainstorm-261023-0048-ai-ide-feature-decisions.md`.
- Wrote a 15-phase roadmap (`plans/261023-0048-multi-agent-ide-roadmap/`) that supersedes `261023-0036-vibe-coder-gaps`. Every phase ends with unit + e2e tests and a perf gate against a committed baseline.
- Red-team (4 lenses, 40 findings → 15, all with code evidence) changed the plan materially:
  - The daemon socket has no per-caller authorization, so any agent-callable verb would be an escalation path. Session tokens + capability table moved into phase 1.
  - Several "reuse existing X" claims were false: no daemon review-notes store, no Codex transcript lookup, no PID enumeration in `proc_tree`, no daemon version restart, `chat.rs` classifier would keep ACP sessions Working.
  - Data-loss paths fixed in the design: undo in shared folders, Finish with uncommitted work, update restart that saved sessions as dead.
  - `.kelpie.yaml` `check:` must stay behind the existing hash approval, read from the base branch only.
- Lesson: grep every helper a plan says it reuses before writing the phase; three reviewers independently caught the same invented reuse paths.
- User decisions: drop `store.ts` split; bell keeps unread count plus a separate "needs you" dot; checkpoints on by default only if p95 < 150 ms on 10k files.
- Next: implement phase 1 (perf gate, vitest, daemon capabilities, paste sanitiser, redaction).
