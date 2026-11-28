# Research: how agent orchestrators manage many sessions / many projects

Date 2026-11-28. Scope: concurrency, queueing, idle suspend, cost/token budgets, model routing, caching/compaction, worktree isolation, pressure signals.
Legend: [P] = primary/vendor doc fetched and read. [S] = search snippet or third-party only (lower trust). [U] = unverified / not found.

## 1. Verdict up front

- No desktop/local orchestrator found does real resource scheduling (queues, priorities, suspend idle). Local tools (Claude Squad, Conductor, Vibe Kanban, Crystal/Nimbalyst, Orca) = worktree + terminal multiplexer + status badges. Budget control is delegated to the underlying vendor account.
- The real controls live in cloud/enterprise products: Devin (ACU caps, auto-sleep), OpenHands (idle pause, deletion TTL, 12h cap), Jules (daily + concurrent caps, disabled "new task"), Cursor (spend limit, headroom check), Claude Code (TPM/RPM guidance, spend limits, cache TTL).
- Biggest hidden cost lever for a many-session tool is prompt cache lifetime (5m vs 1h): an idle session that wakes after TTL re-reads full context uncached. Suspend policy and cache TTL must be designed together. [P] code.claude.com prompt-caching.
- Kelpie is already ahead of local peers on persistence (daemon outlives window); the gap is policy: caps, priorities, idle policy, cost/limit surfacing.

## 2. Per-tool findings

### Orca (stablyai/orca) — exists, MIT, ~15.5k stars [S]
- README features: parallel worktrees per task; coordinator agent that splits a task and starts worker agents in own workspaces; "Needs you" state for bots; mobile app ping when an agent needs input; Usage & Accounts view (limits used + reset time, account switch); run worktrees on remote Orca server / SSH / cloud VM; schedules/triggers (PR opened, failed check). [P] https://github.com/stablyai/orca
- Concurrency/queue/idle-suspend/cost($)/model routing: not documented on README. [U]
- Note: "Orca" design assets are referenced in Kelpie's own repo as historical context; do not confuse forks (many mirrors exist). Official: https://github.com/stablyai/orca , site onorca.dev [S].

### Devin (Cognition) — most complete cost model
- ACU = consumption unit: accrues from number/complexity of actions plus small VM time. [P] https://docs.devin.ai/admin/billing/usage.md
- Auto-sleep: default 5 min inactivity (Windows/GPU 30 min; enterprise can request 5–120 min). Sleeping = zero usage; awake-idle = few units. Any message wakes it. [P] same URL.
- Plan-dependent concurrency, no fixed number published. [P] same URL. Search snippet claims "no concurrent session limits for Teams" [S] https://docs.devin.ai/billing (page itself 404 on direct fetch).
- ACU limits hierarchy: account limit -> shared pools (teams/projects) -> per-user limits inside pool; managed by V3 API with separate permissions (View/Manage). [P] https://docs.devin.ai/admin/billing/acu-limits.md
- At org cap: new sessions cannot start, running sessions sleep; resets monthly or admin raises/sets temp limit. [S] search summary of docs (cap-behavior text not on the page I fetched).
- Dashboards: per-session Session Insights, Settings > Usage & limits (quota remaining), enterprise Consumption + Consumption Analytics. [P] usage.md
- Managed Devins (launched 2026-03-19): coordinator session scopes, launches child sessions (own VM each) with prompt/playbook/tags/**ACU limit per child**, sends follow-ups, monitors child ACU, sleeps/terminates children; coordinator is woken on child finish/needs-input (no polling); "Auto-approve child sessions" toggle; schedule messages to itself. No platform cap on child count stated. [P] https://cognition.com/blog/devin-can-now-manage-devins ; [S] https://docs.devin.ai/work-with-devin/advanced-capabilities
- "Devin Fusion": not found in searches. [U] Skipped.
- Model routing: not published. [U]

### OpenHands — best-documented suspend lifecycle
- Idle pause: no agent/user activity for configurable Idle Time -> sandbox paused, CPU/RAM released; activity resets timer; auto-resume on next access or explicit `POST /api/v1/sandboxes/{id}/resume`. [P] https://docs.openhands.dev/enterprise/conversations-and-sandboxes
- Deletion Time: paused conversation deleted with storage after N time. Hard 12h max running session -> force-paused even if active (not configurable); resume starts new 12h window. [P] same.
- Placement: one sandbox per conversation (isolated, costly) or shared via strategy (none / newest / LRU / fewest conversations / first available). Grouping is placement only, NOT a scheduler; "applications must limit admission based on tested sandbox capacity". [P] same.
- SDK: `conversation.pause()` from another thread; `conversation.run()` resumes with state preserved. [P] https://docs.openhands.dev/sdk/guides/convo-pause-and-resume.md
- Error/missing sandbox -> conversation read-only (transcript viewable, no new messages). [S]
- Default idle value / open-source numeric concurrency cap: [U].

### Claude Code (subagents, agent teams, worktrees) — best token tooling
- Cost: avg ~$13/dev/active day; <$30 for 90% [P] https://code.claude.com/docs/en/costs
- `/usage`: per-session tokens, per-model, prompt-cache hit line (hits, misses, expected rebuilds, warm/cold + TTL), attribution to skills/subagents/MCP/loops, recommendations ("compact before you step away"). Status line + OTel expose same. [P] costs
- Team limits: spend limits at org/group/member; seat allowance rolling 5h + weekly windows; TPM/RPM per-user table (e.g. 5-20 users: 100-150k TPM, 2.5-3.5 RPM) applied org-wide so individuals burst. [P] costs
- Usage-limit hit: `autoContinueAtUsageLimit` waits for reset and continues task automatically (v2.1.234+). [P] costs
- Model routing: subagent `model:` frontmatter, `CLAUDE_CODE_SUBAGENT_MODEL`, `_FORCE`; guidance "Sonnet default, Opus for architecture, Haiku for simple subagents; Sonnet for teammates". [P] costs, https://code.claude.com/docs/en/agent-teams.md
- Agent teams: experimental, no hard teammate cap, advice start 3-5, ~7x tokens when teammates in plan mode, "shut down teammates when done, each active one keeps consuming"; shared task list with dependency blocking and file-lock claiming; TeammateIdle/TaskCreated/TaskCompleted hooks; idle rows auto-hide after 30s, >3 idle collapse to "N idle agents". [P] agent-teams.md
- Caching: prefix cache, per-model, TTL 5m or 1h; subscription main conversation gets 1h, subagents/teammates/compaction get 5m by default; API-key users get 5m unless set (`promptCacheTtl`, `subagentPromptCacheTtl`); model/effort switch, MCP tool-set change, plugin toggle, compaction invalidate; cache scoped per machine+directory; parallel sessions in same dir can share cache; resume after TTL = full uncached re-read, and `/compact` after idle is the most expensive. `idleCompaction` setting compacts long idle 1h-TTL sessions. [P] https://code.claude.com/docs/en/prompt-caching.md
- Hidden background spend: scheduled tasks, cross-session messages delivered to idle sessions, goal check-ins (max 3 idle), session recap. `crossSessionInbound: hold` exists to hold inbound messages. [P] costs "Why usage climbs"
- Worktrees: documented manual parallel pattern. [P] agent-teams.md "Next steps"

### Claude Squad (smtg-ai) — exists, Go, AGPL-3.0, ~6-8k stars [S]
- tmux session + git worktree per instance; `c` = commit + pause, `r` = resume; `-y/--autoyes` experimental; profiles in `~/.claude-squad/config.json`. [P] https://raw.githubusercontent.com/smtg-ai/claude-squad/main/README.md
- Pause = commit changes and (per snippets) remove worktree but keep branch [S, unverified detail]. No limits, queue, cost view documented. [P]
- Known weakness: killed tmux leaves orphan worktrees/branches. [S] https://dev.to/stevengonsalvez/claude-squad-run-multiple-ai-agents-in-parallel-without-the-mess-1hfl

### Conductor (conductor.build) — exists, Mac app, free [S]
- Isolated workspace per agent (Claude Code, Codex; Cursor claimed on site, conflicting) using the user's own login; limits are the vendor's. Changelog: shows when Claude Code is usable again after limit hit; context-usage popover shows account limits + reset times. [S] https://www.conductor.build/changelog/page/2 , https://thenewstack.io/a-hands-on-review-of-conductor-an-ai-parallel-runner-app/
- Conductor Cloud exists (site title) [S]; details not read. Concurrency/idle: [U].

### Vibe Kanban (BloopAI) — README says "sunsetting" [P]
- Kanban board, worktree per workspace/task, many agent CLIs; automatic cleanup of orphaned/expired workspaces (`DISABLE_WORKTREE_CLEANUP` to disable). Issues can be prioritized/assigned (board-level priority only, not scheduler). [P] https://github.com/BloopAI/vibe-kanban
- Adoption risk: sunset => do not copy dependencies; patterns OK.

### Crystal -> Nimbalyst (stravu)
- Crystal retired (last v0.3.5, 2026-02-26), successor Nimbalyst: worktree per session, kanban cards auto-move columns, Claude Code + Codex. [S] https://nimbalyst.com/crystal , https://github.com/stravu/crystal
- Resource/cost features: [U].

### Sculptor (Imbue) — exists
- One Docker container per Claude Code agent (permission prompts avoided, host isolation); Pairing Mode mirrors container state to local checkout (stash support v0.4.0). [S] https://imbue.com/sculptor/ , https://docs.imbue.com/changelog
- License conflicting (MIT vs ELv2) [S]. Resource caps/idle: [U]. Mac/Linux only [S].

### Cursor Cloud (background) agents
- Spend limit prompted on first use; run won't start without ~$2 headroom under hard limit (staff reply). [S] https://forum.cursor.com/t/what-is-the-pricing-structure-for-using-cloud-agents/156843
- Billing = tokens at API price + env setup, not idle time. [S] same.
- Concurrency: docs say no fixed cap; forum reports plan-based caps (e.g. 10 Pro) and "Upgrade to Ultra for more Cloud Agents" error. Conflicting, official numbers [U]. https://forum.cursor.com/t/cloud-agents-simultaneous-limit-what-are-the-actual-numbers-per-plan/154013
- VM idle TTL: [U]. Pricing page fetch 404.
- Forum complaint: one session can cost $600+ without guardrails. [S] https://forum.cursor.com/t/cursor-needs-better-pricing-guardrails-when-one-session-can-cost-600/156172

### OpenAI Codex cloud
- Container state cached up to 12h to speed new tasks/follow-ups; setup script run then cached; env changes apply to new tasks only. [S] https://developers.openai.com/codex/cloud/environments
- Usage: local messages and cloud tasks share one allowance in rolling 5h windows (+ weekly may apply); cloud tasks can cost more than local messages; credits extend past limit; smaller model has far higher estimated message allowance (Plus: small model 350-3000 vs large 15-150 per 5h, per page figures). [P] https://learn.chatgpt.com/docs/pricing.md
- Concurrent cloud task limit: [U] not documented.

### Amp (Sourcegraph)
- Current docs: "dial" of 4 modes replacing smart/rush/deep/large; each subagent chooses own model independently; read-only "oracle" subagent (stronger, slower) for planning/review, pattern "cheap fast main + oracle". Mappings change over time. [S] https://ampcode.com/news/the-dial , https://ampcode.com/docs/models-and-subagents , https://ampcode.com/news/rush-2.0
- Original rush claim: 67% cheaper, 50% faster than smart (old lineup, vendor claim). [S] https://ampcode.com/news/rush-mode
- Concurrency/idle/budget: [U].

### Jules (Google) — only tool with clean published queue-ish limits
- Free 15 tasks/day, 3 concurrent; Pro 100/15; Ultra 300/60. Rolling 24h window. At limit "new task" button disabled with tooltip; existing tasks still manageable. Behavior on exceeding concurrent cap not described. [P] https://jules.google/docs/usage-limits/

## 3. Comparison matrix

| Tool | Concurrency cap | Queue | Idle suspend | Budget/cost view | Model routing | Isolation | Pressure signal |
|---|---|---|---|---|---|---|---|
| Devin | plan-based, no number | no (sessions blocked at cap) | auto-sleep 5m, 0 cost asleep [P] | ACU caps acct/pool/user/child, dashboards [P] | n/p | VM per session | cap -> start refused, running sleep [S] |
| OpenHands | operator admission control [P] | none built-in [P] | idle pause + delete TTL + 12h cap [P] | n/p | n/p | sandbox (shared or solo) | read-only conv on error [S] |
| Claude Code | none; advice 3-5 teammates [P] | task list w/ deps [P] | idle teammate hide, idleCompaction [P] | /usage, OTel, spend limits, TPM/RPM [P] | per-subagent model [P] | worktree manual | limit msg + auto-continue [P] |
| Jules | 3/15/60 + daily 15/100/300 [P] | no | n/p | task counter | n/p | VM | disabled button + tooltip [P] |
| Cursor cloud | forum-only [S] | no | n/p [U] | spend limit + $2 headroom [S] | user pick | VM | spend-limit block [S] |
| Codex cloud | n/p [U] | n/p | container cache 12h [S] | 5h/weekly shared, credits [P] | small vs large model [P] | container | dashboard + reset time [P] |
| Amp | n/p | n/p | n/p | n/p | per-subagent + oracle [S] | n/p | n/p |
| Orca | none seen | none | remote host keeps running [P] | account limit + reset display [P] | n/p | worktree | "Needs you" + mobile ping [P] |
| Conductor | none seen | none | n/p | vendor limit + reset popover [S] | n/p | workspace/worktree | limit-reset notice [S] |
| Claude Squad | none | none | manual pause (commit + remove worktree) [P/S] | none | profiles per program [P] | tmux+worktree | none |
| Vibe Kanban | none | board only | n/p | none | per-agent | worktree + auto cleanup [P] | n/p (sunset) |
| Sculptor | n/p | n/p | n/p | n/p | n/p | Docker | n/p |

## 4. Transferable patterns (ranked for Kelpie fit)

Kelpie context (from AGENTS.md): daemon owns processes/buffers/lifecycle; agents are external CLIs; no hosted cloud; user pays vendor directly.

| # | Pattern | Used by | Kelpie fit / note |
|---|---|---|---|
| 1 | Hard admission cap with explicit refusal message (global + per-project), not silent queue | Jules (disable button+tooltip), OpenHands (admit by tested capacity), Kelpie sub-session spawn already refuses | High. Cheap. Show "n/N running" in UI. |
| 2 | Idle auto-suspend policy: no-activity timer -> pause, wake on message; configurable; deletion TTL separate from pause; hard max runtime | Devin (5m sleep), OpenHands (idle/deletion/12h) | High for terminal sessions: suspend = stop PTY child only when agent is at prompt + resumable (Claude `--resume`), keep buffer. Never suspend mid-turn (DigitalOcean also: no auto-pause mid-run [S]). |
| 3 | Align suspend timer with prompt-cache TTL; warn that resume after TTL costs a full uncached read; compact BEFORE parking | Claude Code prompt-caching + /usage recommendation | High, unique cost lever. Suspend threshold <5m is cache-hostile; 1h TTL only on subscription main convo. |
| 4 | Per-child budget cap passed at spawn (ACU limit analogue = tokens/$/turns) + coordinator can sleep/terminate child | Devin managed devins, Claude `--max-budget-usd` flag exists [P costs] | Medium. Kelpie sub-sessions could pass a budget/turn cap. |
| 5 | Hierarchical limits: account -> pool (project) -> user/session | Devin ACU limits, Claude org/group/member spend limits | Medium. Kelpie analogue: global -> project -> session caps. |
| 6 | Event-driven wake of coordinator on child finish/needs-input (no polling) | Devin, Claude idle notifications, Kelpie report already | Already have; keep. |
| 7 | "Needs you" attention queue = prioritization by blocked-on-human, not by project rank | Orca (Needs you + mobile), Claude Squad status, Nimbalyst auto-columns | High value, low cost: sort/badge sessions waiting for input first; mute/park others. |
| 8 | Surface vendor limit + reset time in UI; auto-continue after reset | Orca Usage & Accounts, Conductor popover, Claude `autoContinueAtUsageLimit` | High: read-only display; scheduling a resume at reset = natural "queue". |
| 9 | Model tiering per role: cheap model for explore/background, strong for plan/review, set at spawn not mid-session (switch invalidates cache) | Claude subagent model/teammates Sonnet, Amp subagent model + oracle, Codex small model allowance | Medium: expose model per sub-session spawn; do not auto-switch mid-conversation. |
| 10 | Attribution of spend to source (subagent/MCP/loop/skill) + actionable recommendations | Claude `/usage` | Medium; only for Claude-adapter sessions Kelpie already parses. |
| 11 | Block hidden idle spend: hold cross-session/scheduled messages to parked sessions; cap idle check-ins | Claude `crossSessionInbound: hold`, goal check-in cap 3 | High: Kelpie's own session-to-session messaging wakes idle agents (full-context turn each). Deliver only when session active or user-opt-in. |
| 12 | Team size guidance 3-5; tokens scale linearly; shut down finished agents promptly | Claude agent-teams, costs | Defaults: recommend soft warning at ~N running agents. |
| 13 | Worktree lifecycle hygiene: auto-clean orphaned/expired worktrees; pause = commit + free worktree, keep branch | Vibe Kanban, Claude Squad | Medium; Squad shows orphan failure when processes killed externally. Daemon ownership avoids this. |
| 14 | Shared sandbox grouping strategies (LRU / fewest) as placement only | OpenHands | Low; Kelpie uses local FS. Skip. |
| 15 | Spend-limit pre-flight (refuse start if < headroom) | Cursor ($2 headroom) | Low; Kelpie lacks billing access. Skip unless vendor usage is readable. |
| 16 | Container/state caching for fast restart | Codex (12h), OpenHands pause | Not applicable locally; PTY/buffer persistence already equivalent. |

## 5. Recommended order for Kelpie (ranked)

1. Visible caps + refusal text (global running agents, per project) — pattern 1, 12. Default soft warn ~5, hard configurable.
2. "Needs you" attention ordering + notification as the prioritization mechanism — 7. Avoid manual project ranking; blocked-on-human is the real priority signal in every tool that has one.
3. Idle park policy tied to cache TTL (suggest >= 1h for subscription users, offer 5m only for API-key/no-cache awareness), only at idle prompt, with pre-park compact suggestion — 2, 3.
4. Hold inbound inter-session messages for parked sessions — 11.
5. Show vendor limits + reset time; optional auto-resume at reset — 8.
6. Per-sub-session model + optional budget/turn cap at spawn — 4, 9.
Defer: attribution analytics (10), spend pre-flight (15).

## 6. Limitations / source quality

- Claude Code docs, OpenHands Enterprise docs, Devin usage/ACU/blog, Jules limits, Claude Squad README, Orca README, Codex pricing = fetched primary. Everything marked [S] came from search-result summaries and was not read at source.
- Several tool docs returned 404 or redirect on direct fetch (Devin billing page, Cursor pricing/cloud-agent pages); Cursor numbers rest on forum posts (unofficial, contradictory).
- Search outputs reference model/version names I could not cross-check (some pages post-date my training knowledge); I avoided relying on specific model names.
- OpenHands numbers are for Enterprise; OSS runtime defaults not checked.
- Not covered: Conductor Cloud, Nimbalyst internals, Sculptor resource limits, Amp pricing, Codex concurrency, Cursor 3 agent window internals, Windsurf/Cline/Goose/Zed, Devin "Fusion".

## Unresolved questions

1. Does Devin enforce a hard concurrent-session cap per plan? (docs give none; snippet says none for Teams)
2. What is Cursor's actual cloud-agent concurrency and idle VM TTL per plan (official)?
3. Does Claude Squad pause remove the worktree (detail not verified in README text read)?
4. Can Kelpie read subscription usage windows (5h/weekly) for non-Claude agents, or only Claude via local files/`/usage`?
5. Do Codex/ACP agents expose cache-TTL/idle semantics comparable to Claude's, to make pattern 3 agent-agnostic?

Status: DONE_WITH_CONCERNS
Summary: Report written with per-tool matrix, 16 patterns, ranked Kelpie recommendations; several tools (Cursor, Amp, Conductor, Sculptor, Nimbalyst, Codex concurrency, Devin Fusion) rest on search snippets or are unverified and are marked as such.
