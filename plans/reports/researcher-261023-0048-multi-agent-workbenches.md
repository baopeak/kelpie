# Multi-agent workbenches & ACP — new findings beyond the gap report (2026-10-23)

Researcher report (5 searches + 3 fetches). Tags: **[V]** vendor/marketing · **[I]** independent · **[O]** official protocol docs · (bg) unverified background.
Builds on [gap analysis](research-261023-0036-vibe-coder-gap-analysis.md) G1–G14. Not covered: Amp, Jules, Devin internals, Orca, A2A, AGENTS.md standard, cross-CLI hooks standards, perf patterns of competitors.

## Verdict
1. Build G2 (context/cost) **ACP-first**: ACP `session/usage_update` stable since 2026-06-05 [O]; transcript tailing = fallback for non-ACP CLIs.
2. Add a **state-grouped attention list** (Claude Code Agent View May 2026 [I], Warp Agent Management Panel [V]).
3. Containers/cloud: keep as non-goal; devcontainer opt-in per worktree is the cheapest safety answer if ever needed.

## Feature matrix

| # | Feature | Products | Note | Generic? |
|---|---|---|---|---|
| A1 | State-grouped agent list (running / needs input / PR ready) | Claude Code desktop & Agent View [I], Warp panel [V] | "needs input" floats up; Warp separates interactive vs cloud, shows blocked/failed + logs | Yes — statuses exist |
| A2 | Review queue: all agent diffs in one approve/merge surface | Codex app [I], Conductor [I] | comment on diff line → back to agent; Kelpie has notes per worktree, no cross-worktree queue | Yes |
| A3 | Automations deliver into review queue | Codex app [I] | scheduled jobs end as queue item | Yes |
| A4 | Per-thread execution mode Local / Worktree / Cloud | Codex app [I] | one selector at creation | Cloud = API |
| A5 | Cross-harness agent memory | Warp Oz (preview) [V] | memories from Claude Code + Codex sessions usable by any agent | Partly (instructions sync / memory MCP) |
| A6 | Multi-harness routing + effectiveness comparison | Warp Oz [V] | compare cost, time, pass rate | Yes if cost captured |
| A7 | Governance: per-agent credit caps, audit log | Warp Oz [V] | solo version = per-session cost cap | Needs metering |
| A8 | Usage/cost as protocol message | ACP `session/usage_update` 2026-06-05 [O] | no scraping | ACP only |
| A9 | Session list/resume/close/delete | ACP (Apr–Jun 2026) [O] | discover/reconnect without replay | ACP only |
| A10 | Elicitation (structured questions/forms) | ACP 2026-07-22 [O] | maps to inbox "needs you" with form | ACP only |
| A11 | Additional workspace roots | ACP `additionalDirectories` 2026-06-01 [O] | multi-repo tasks | ACP only |
| A12 | ACP Registry (discover/install agents) | ACP 2026-03-09 [O] | could feed presets list | ACP only |
| A13 | Phone launch + local/remote handoff | Warp Oz [V], Nimbalyst iOS [I] | oversight from phone | Generic via daemon; push = G4 |
| A14 | Container pairing mode | Sculptor [I] | two-way sync with container | Needs runtime |
| A15 | Session sharing / replay | Warp Oz API [V] | raw conversations + artifacts | Transcripts; PTY recording for others |
| A16 | Self-hosted agents (Docker/K8s/remote dev env) | Warp Oz [V] | Kelpie SSH covers remote dev env | Yes over SSH |
| A17 | Auto worktree per session at creation | Claude Code Agent View [I] | removes "forgot to isolate" | Yes |
| A18 | Panel presets per session type | Claude Code desktop redesign [I] | refactor → diff, UI → preview, tests → terminal | Yes |
| A19 | Agent-pushed session title | ACP `session_info_update` [O] | auto-titles | ACP only |

## ACP adoption (agentclientprotocol.com/overview/agents [O], 46 agents)

| Agent | Status | Kelpie note |
|---|---|---|
| Claude Code | adapter `claude-agent-acp` | not native; adapter hides Claude hooks/status |
| Codex CLI | adapter `codex-acp` | not native |
| Gemini CLI, OpenCode, Cursor, GitHub Copilot (preview) | listed (native vs adapter not stated) | Gemini likely native (bg) |
| Goose, Cline, Kimi, Kiro CLI, Qwen Code, Mistral Vibe, OpenHands, Factory Droid, Augment, Junie, Qoder, Blackbox, Poolside, VT Code, Pi (adapter)… | listed | heavy overlap with Kelpie's 23 presets |
| Clients | Zed, JetBrains | Cursor / VS Code as ACP clients unconfirmed |

Milestones [O]: 03-09 registry, `session/list`, `session_info_update` · 04-22/23 `session/resume`, `session/close`, Transports WG (remote WS/HTTP) · 06-05 `usage_update`, `session/delete` · 06-25 Rust & TS SDK 1.0 · 07-20 ACP v2 draft · 07-22 elicitation.
Implications: track v1 session methods, watch v2; stdio only today (SSH-hosted agents can still be spawned via `ssh host agent acp` over stdio — to verify); Claude via adapter is a trade-off.

## UX / perf patterns
- Attention = list grouped by state, not just badges. Review = queue of finished work. Worktree by default. Layout follows task type. Review comments flow back to the agent.
- No independent data on how competitors keep 10+ live sessions cheap → rely on Kelpie's own measurements.

## Recommended priority changes
G2 ACP-first · new P1 attention list (A1) · new P1 cross-worktree review queue (A2, pairs with Finish) · comparison metrics (A6) into best-of-N, depends on G2.

## Unresolved questions
1. Native vs adapter ACP for Gemini CLI, OpenCode, Cursor, Copilot.
2. Do Claude/Codex ACP adapters expose usage + permission parity with native CLI?
3. Amp/Jules/Devin/Orca review & handoff; A2A; AGENTS.md standard status; cross-CLI hooks standard.
4. Competitor perf for 10+ sessions.

## Sources
- https://agentclientprotocol.com/overview/agents · https://agentclientprotocol.com/updates
- https://www.warp.dev/blog/multi-harness-cloud-agent-orchestration · https://docs.warp.dev/platform/managing-cloud-agents/ · https://itsfoss.com/news/warp-launches-oz/
- https://openai.com/index/introducing-the-codex-app/ · https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/ · https://techinformed.com/openai-ships-codex-macos-app-as-ai-coding-shifts-toward-parallel-agents/
- https://verygood.ventures/blog/claude-code-desktop-hands-on-review/ · https://www.eigent.ai/pl/blog/claude-code-desktop-redesign · https://pasqualepillitteri.it/en/news/2384/claude-code-agent-view-cli-dashboard-sessions-2026
- https://superset.sh/compare/superset-vs-sculptor · https://devtune.ai/verticals/coding-agent-orchestration/compare.md · https://www.morphllm.com/comparisons/superset-vs-conductor-vs-emdash
