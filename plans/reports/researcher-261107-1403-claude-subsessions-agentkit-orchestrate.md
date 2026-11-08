# Claude Code sub-sessions + AgentKit orchestrate/atoz: research for Kelpie

Date 2026-11-07. Read-only research. Marks: [V]=verified in official doc/changelog, [S]=secondary/third-party, [U]=unverified/not found.

## Source access
- agentkit.best blog: fetched OK (fetch tool summarises, not raw).
- Facebook post (mrgoonie): FAILED. Page returns only a truncated title ("Duy Nguyen - Co the ban biet roi: Claude Code nay dieu..."), no body. Used the user-supplied summary only.
- No official Anthropic page uses the words "sub-session". Official terms: sessions, cross-session messaging, agent view/background sessions, agent teams, environments. "Nested sessions" appears only in a third-party article [S].

## Key finding (contradicts the task premise)
The AgentKit blog does NOT describe (a) a cached benchmark data store for model routing, (b) device-aware spawn limits, or (c) automated cleanup. It says explicitly: no device-aware limits, cap parallel sessions by remaining usage; model routing is guidance only (cheap model for mechanical, strong for design); cleanup is a warning (orphaned dev servers/watchers per worktree eat RAM), not a feature. https://agentkit.best/vi/blog/orchestrate-va-atoz
The benchmark-cache/device-awareness claims are likely only in the Facebook post (unreadable) or in the `ak` CLI itself. [U] Web search found no public repo/doc for `ak orchestrate`. Treat as unverified; Kelpie would need to design these itself.

---
## (a) Claude Code sub-session mechanics, messaging, UX

### Surfaces (do not conflate)
| Surface | What | Source |
|---|---|---|
| Subagents | in-session workers, own context, return summary; not separate rows | https://code.claude.com/docs/en/agents [V] |
| Agent view (`claude agents`, research preview) | background sessions run by a supervisor process; each its own Claude process | https://code.claude.com/docs/en/agent-view [V] |
| Agent teams (experimental, env `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`) | lead + teammates, shared task list, mailbox | https://code.claude.com/docs/en/agent-teams [V] |
| Desktop Code tab sessions + saved environments | sidebar sessions; Claude can list/read/message them | https://code.claude.com/docs/en/desktop [V] |
| "Nested sessions" (desktop, announced 2026-10-14 by ClaudeDevs) | main thread opens child session per parallel task; child sends result back | https://pasqualepillitteri.it/en/news/24032/claude-code-desktop-nested-sessions-environments [S, single article, no app version] |

"Sub-sessions" in mrgoonie's post = desktop nested sessions + cross-session messaging. AgentKit's blog confirms desktop UX: asks "Allow once" when main session wants to open a new session; main panel lists sub-sessions (example 8), each can be opened, chatted with, stopped, redirected. https://agentkit.best/vi/blog/orchestrate-va-atoz [S]

### Environments
- Saved environment = entry in selector recording where Claude runs (Local, Cloud, SSH, WSL); local/SSH env also holds the folders Claude works in. Desktop auto-creates up to 8 from recent folders. Optional worktree per folder. [V] https://code.claude.com/docs/en/desktop ; [S] pasqualepillitteri link above.
- So "environment with several folders" = multi-root workspace record. Matches mrgoonie's description.

### Spawning / steering
- Desktop suggested-task chips: Start (new session; "sidebar lists the started session under the current session"), Do it here, Start all. [V] desktop doc. This is the parent-child sidebar nesting.
- Dispatch can also spawn Code sessions itself. Approvals last 30 min in Dispatch-spawned sessions. [V]
- Agent view dispatch: `claude --bg "<prompt>" --name --agent`, `--exec 'cmd'` (shell job no model), `/bg`, `/fork`. Dispatched session moves into its own git worktree under `.claude/worktrees/` before editing; commits/pushes/draft PR; never pushes to main, force-pushes or merges. Deleting a session removes its worktree only if clean. [V] https://code.claude.com/docs/en/agent-view
- Exact name of the tool the main session uses to open a desktop sub-session and the permission prompt text: [U] not documented anywhere I could reach. AgentKit says "Allow once". Tool index (tools-reference) not read.

### Messaging protocol (cross-session) [V] https://code.claude.com/docs/en/cross-session-messaging
- Tools: `ListAgents` (discover) + `SendMessage` (to by name, optional short id when names collide). Claude calls them itself; user can `@name` a session; `/list-agents` (alias `/peers`).
- Message = plain text only, never history/files. Cap ~1M chars; burst-limited per target (sender refused, told to batch); receiver rate-limits per sender, drops identical repeats, queues max 50 -> loops self-stop.
- Delivery: read between tool calls in an active turn (never interrupts a running tool); idle receiver gets a new turn. Counts as usage like a typed prompt. Rendered as dim one-line preview "Message from @name: ..." with ctrl+o to expand. Desktop: card with sender title + link back; busy target holds until current work finishes; can't deliver to archived sessions.
- Transport: per-session Unix socket (macOS/Linux) or named pipe with token-auth first line (native Windows, v2.1.234+); never via Anthropic servers for same-machine. Remote/cloud via Remote Control through Anthropic servers. Env exported to children: `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`. 30 s to complete a line or connection closed.
- Idle notice: `SendMessage notify_when_idle: true` -> one-shot notice when watched session next idles/exits, with finish time + one-line status; 12 h expiry; main conversation only, local sessions only (v2.1.236+). This is the "done" report primitive. Blocked/needs-permission reporting is not a separate protocol: agent view exposes state `Needs input` and `claude agents --json` gives `state`, `status` (busy/waiting/idle), `waitingFor`. [V]
- Trust model: incoming message is attributed to other session, cannot approve prompts, change config/CLAUDE.md, or run slash commands; receiver's own permission prompts still fire; senders told never to ask peer for an action their own permissions deny (anti permission-laundering).
- Inbound policy `crossSessionInbound` accept/hold/refuse. Default: prompting-mode receiver accepts unless sender is bypass-mode; bypass-mode receiver HOLDS unless sender also bypass. Held msgs expire after `dialogExpiry` (5 min default); desktop/VS Code/`-p` can't show approval dialog so held msgs just wait then drop. `isolatePeerMachines` forces approval for cross-machine sends. Deny `SendMessage`+`ListAgents` to disable.
- Desktop scope: Claude sees only desktop-run sessions (local/SSH/WSL), top 20 most recently active, excludes own session and archived; scheduled-task sessions can't send. [V] desktop doc, [S] pasquale article.
- Agent view sessions report results only to the user, not the parent; teammates in agent teams message each other and get automatic idle notifications containing the final answer. [V] agents + agent-teams docs.

### Agent teams mailbox (alt design) [V]
- Per-agent JSON inbox file `~/.claude/teams/{team}/inboxes/{agent}.json`, write success = "sent"; shared task list with file-locked claiming, dependencies auto-unblock; hooks TeammateIdle/TaskCreated/TaskCompleted (exit 2 = push back). No per-teammate permission mode at spawn (inherits lead's incl. bypass). One team per session, no nesting, lead fixed, no resume of in-process teammates. Recommends 3-5 teammates, 5-6 tasks each.

### Subagent progress-summary token change
- Old: ~every 30 s (server flag, clamp 30-240 s) the agent's FULL transcript was sent to the agent's own model (usually Opus) to write a 3-5 word status; on by default for interactive background agents since v2.1.232; reported 8-26% of spend, ~16% typical heavy user. https://www.dsebastien.net/claude-code-agent-progress-summaries/ [S], https://gigazine.net/gsc_news/en/20261102-claude-code-resends-background-agents-transcript/ [S]
- New (reported): v2.1.316+, subagents keep compact running notes; Haiku 5.5 writes the status line every 30 s; ~99% cheaper per update, ~5% less usage-limit use. Source: hermes-ai.net item dated 2026-11-04 citing a ClaudeDevs X post. https://hermes-ai.net/news/anthropic-s-claude-code-slashes-background-agent-status-costs-by-99/ [S, unofficial]. **I could not confirm in the official CHANGELOG**: the 2.1.316 entry I read lists other items; 2.1.318 only mentions progress summaries timing out up to 5 min on cold cache for Bedrock/Vertex/Foundry. Mark UNVERIFIED but plausible.
- Separate: agent view row summaries already Haiku-class, refreshed at turn end + every few minutes; between refreshes row text updates every 15 s from raw output with no model call. [V] agent-view doc.
- Lesson for Kelpie: derive status text locally from PTY output; only call a cheap model sparsely, never with full transcript.

---
## (b) AgentKit orchestrate / atoz (from the blog only)
https://agentkit.best/vi/blog/orchestrate-va-atoz [S, vendor blog, kit `ak` 3.0.0]

### /ak-orchestrate (skill v2.0.0 + CLI `ak orchestrate`)
- Split: skill = judgment (job -> agent, model, risk); CLI = plan state, process monitoring, evidence for re-runs.
- Workers are real sessions (own context/model/lifecycle) or monitored headless processes. Nested subagents do NOT count as workers; fallback to them only when runtime lacks permission to open sessions and must be stated in report.
- Default: only the currently running runtime. `--cross-runtime` or naming a runtime (Codex, Pi...) enables others. Pre-flight per candidate: installed, logged in, quota, cost. Fail -> job `blocked` with evidence, NO silent switch.
- Per-run results dir `plans/reports/orchestrate-<timestamp>`; `--resume <dir>`. Input can be a YAML job file (`plans/orchestrate-jobs.yaml`) or a prose request. Independent arbiter reviews before delivery.
- Plan groups files so jobs don't overlap; dependent tasks wait on prerequisites.

### /ak-atoz (v1.0.0), three gates
1. Lock: `/ak-advise` interview -> brief (Objective, Done means, Out of scope, Constraints); user must Confirm; brief then immutable by agent.
2. Prepare: list everything not auto-obtainable (CLI logins, env vars, repo perms) before touching code; secrets via env/vault never chat; reversible choices logged as overridable defaults; re-check on "ready".
3. Execute: main session = conductor: plan, split into sub-sessions per worktree, monitor, nudge stuck, verify diffs not reports, post progress to GitHub issue. Final review in a fresh-context sub-session (conductor never grades itself).
- Stops to ask only for: destructive/irreversible action outside brief, change breaking a locked "Done means", spending money/sending messages not allowed, unexplained error after advisor.
- Coordination: GitHub Issues = single source of truth. Labels `in progress` -> `in review` -> `done` (PR opened) -> `shipped` (merged). Out-of-scope findings become new issues labelled `ai-handle`. Agent assigns issue to user to claim.
- Model routing / device-awareness / cleanup: see Key finding. Blog only has guidance + one data point: third-party single-run test, subagents on a small task 121k -> 513k tokens (>4x). Anthropic guidance quoted: when in doubt, don't spawn.

---
## (c) Cross-runtime dispatch patterns
Headless one-shots (preferred by orchestration skills) [S] https://dev.to/gabrielanhaia/headless-mode-in-5-agents-claude-p-codex-exec-gemini-p-copilot-p-opencode-run-4816 :
| Agent | Command | Structured output | Pre-approve | Resume |
|---|---|---|---|---|
| Claude | `claude -p` | `--output-format json\|stream-json` (result, session_id, permission_denials) | `--allowedTools`, `--permission-mode` | `--resume <id>` |
| Codex | `codex exec` | `--json` JSONL (thread.started..turn.completed/failed), `-o file`; final msg stdout, progress stderr | `--sandbox workspace-write` (default read-only) | `codex exec resume --last\|<id>` |
| Gemini | `gemini -p` | json / stream-json; exit 42 input err, 53 turn limit | `--approval-mode`, policy file | `--resume latest` |
| Copilot | `copilot -p` | JSONL | `--allow-tool/--deny-tool` | `--continue` |
| OpenCode | `opencode run` | `--format json` (undocumented schema) | config `permission` | `-c`, `-s` |
- Claude docs: all built-in parallelism uses Claude workers only; to involve another tool expose it as an MCP server. https://code.claude.com/docs/en/agents [V]. So cross-runtime in AgentKit = skill shells out to the other CLI (headless) or opens its own interactive session; no Anthropic-native protocol.
- Claude `-p` non-interactive sessions DO bind an inbox socket (unless `--bare`), so a long-running `-p` worker is messageable if `crossSessionInbound: accept` is passed via `--settings`. [V]
- Claude `claude -p` on subscription plans draws from a separate monthly credit since 2026-06-15 [S, inventivehq.com/blog/ai-coding-clis-in-ci-cd-automation]; relevant if Kelpie spawns `-p` workers.
- Gemini CLI reportedly replaced by Antigravity CLI (`agy -p`) for some tiers on 2026-06-18 [S, unconfirmed]; verify before building a Gemini adapter.
- Not covered: Pi / OMP / Grok CLI headless interfaces (no sources found). Kelpie already has ACP adapters; ACP is the structured alternative to scraping PTYs [project knowledge, not researched here].

---
## (d) Pitfalls & lessons
1. Token multiplication: each session independent quota; 4 sessions burn the 5 h limit faster [S]; agent teams "significantly more"; subagent 4x single-run datapoint [S]. Cap by remaining usage, not just CPU/RAM.
2. Hidden background overhead: full-transcript status polling cost 8-26% of spend (above). Never poll models with full context.
3. Orphans: per-worktree dev servers/watchers survive; tmux sessions orphan after team end (official troubleshooting) [V]. Desktop archive deletes worktree; agent view keeps worktree when dirty/unpushed [V].
4. Same-file edits: parallel agents must own disjoint files or separate worktrees; agent teams don't isolate. [V]
5. Permission laundering / loops: messages never grant consent; rate limits stop ping-pong; bypass-mode receivers hold messages from non-bypass senders (surprising default: an unattended bypass worker silently never gets messages; held msgs drop after 5 min). [V]
6. Prompts bubble to the lead; too many prompts -> pre-approve allow rules before spawn. [V]
7. Lead drift: lead starts implementing instead of waiting; teammates stop early after errors; task status lags. [V] agent-teams troubleshooting. AgentKit's answer: conductor verifies diffs, fresh-context reviewer, nudging.
8. Interactive REPL startup race: first task can be silently ignored if sent too early; prefer headless exec. Codex can't commit from worktree when git index outside sandbox -> orchestrator commits. Exit 0 != done: check no `turn.failed` and `git status` non-empty. [S] https://www.claudepluginhub.com/skills/salemaziel-quintet/quintet-headless-worktrees , dev.to link above.
9. Silent runtime fallback is dangerous: AgentKit marks `blocked` with evidence instead.
10. Idle reaping: agent view stops idle unattended sessions after ~1 h (pin to keep); machine shutdown -> sessions <48 h marked failed/restartable. [V]
11. Desktop cannot show held-message approval dialog; unattended desktop sessions just expire messages. [V]

---
## (e) Ideas for a terminal workbench (Kelpie)
Ranked by value/effort, all fit "daemon owns sessions" architecture.
1. **Parent/child session tree in sidebar** (cheap, high UX): add `parent_session_id` + `origin` (user|agent) to session record; sidebar nests children under parent, click to open PTY/chat. Matches Claude desktop "started session listed under current session".
2. **Daemon-side session control API for agents** (core): expose list/spawn/send/stop/state as an MCP server (or CLI `kelpie call`) that any runtime can call. This is runtime-agnostic equivalent of ListAgents/SendMessage and the only way Claude can orchestrate Codex etc. (Claude supports external tools via MCP). Auth via per-session token env var (copy `CLAUDE_..._TOKEN` idea), roles already exist in kelpie-proto.
3. **Spawn consent gate**: agent-initiated spawn needs user "Allow once / Always for this project" prompt in UI; children inherit not-more-than parent's permission; message from agent never counts as approval (copy Anthropic's rules: no consent, no config change, no slash command execution).
4. **Messaging**: plain text, size cap (~64-256 KB for PTY use), per-target burst limit, dedupe identical repeats, max queue (50), sender-attributed prefix. Deliver at idle/turn boundary: for PTY agents, inject only when daemon sees agent idle (use state detection) to avoid the REPL race; for chat-mode agents use adapter's send. One writer per conversation (already an AGENTS.md rule).
5. **Status model**: daemon derives `working | needs_input | idle | done | failed` and `waitingFor` from PTY/adapter events (no model calls); emit one-shot `notify_when_idle` subscription so parent gets "done/blocked/needs permission" events pushed into its inbox. Optionally Haiku-class summary only at turn end, with truncated tail not full transcript.
6. **Worktree-per-child + lifecycle**: auto-create worktree per spawned child; archive removes it only if clean; record child PIDs/ports per worktree so teardown kills the process tree (matches process-management lessons; addresses AgentKit's unaddressed cleanup gap). Reaper for orphans on daemon start.
7. **Concurrency governor** (Kelpie can beat AgentKit here): max parallel = f(free RAM, CPU, per-runtime usage/quota info if available); queue extra spawns. Kelpie already has cost/usage-alerts code (`usage-alerts.ts`, cost page) that could feed quota limits.
8. **Runtime registry + preflight**: per runtime adapter declares: installed?, logged in?, headless cmd template, structured-output parser, resume cmd, permission flags, ACP support. Preflight failure -> job `blocked` with evidence, never silent fallback (AgentKit rule).
9. **Model routing cache**: store per (runtime, model, task-type) outcomes from Kelpie's own runs: tokens, cost, duration, success/arbiter verdict. Use as a hint list only; user-overridable. Do not claim parity with AgentKit's unverified benchmark cache.
10. **Environments**: workspace = named set of folders (+ host) per project; child spawn picks folder from it; cross-folder messaging allowed only within an environment unless user approves.
11. **Run folder + resume**: per-orchestration dir with job list, status, evidence; `--resume`. Fresh-context reviewer session at end; conductor must verify diffs rather than trust reports.
12. Skip initially: shared task list with file locks (agent-teams style), cloud/remote messaging, auto-summary models.

Recommended order: 1 -> 2 -> 3/4 -> 5 -> 6 -> 7 -> 8, then 9-11.

---
## Limitations / unresolved
- Facebook post unread; benchmark cache and device-awareness claims unverified anywhere. Ask mrgoonie/inspect `ak` CLI (npm/GitHub) directly.
- Official name of Claude Desktop's session-spawn tool, its exact permission prompt, and whether children inherit parent's permission mode: not found. Nested-session details rest on one third-party article.
- Haiku 5.5 progress-summary change (v2.1.316): only a secondary report citing an X post; changelog tail (>100k chars) not fully read.
- Desktop doc read only first ~100k of 221k chars; changelog first ~100k of 1.17M chars.
- Pi/OMP/Grok/Gemini-successor headless interfaces not researched; Codex app-server/ACP protocols not researched.
- Q: should Kelpie children be PTY-only, or also structured chat (ACP/`-p` stream-json) for reliable state/done detection?
