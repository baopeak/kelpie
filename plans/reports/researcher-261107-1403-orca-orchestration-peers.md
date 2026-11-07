# Orca orchestration, Claude primitives, peers (condensed)

Date 2026-11-07. Researcher returned the report inline; condensed here by the coordinator. Tags: [S] source, [O] official docs, [T] third party, [U] unverified.

## Orca (stablyai/orca, MIT)
- CLI + skill. `orca orchestration run-create|task-create|task-list|task-update`, `worker-start --task --worktree current|new-child|new-top-level --agent <claude|codex…>`, `worker-list|show|read|stop|release`, `send --to dispatch:<id>|@all|@idle`, `ask`/`reply`, `check [--wait] [--ack]`, `inbox`, `gate-create|gate-resolve`. Separate `orca-cli` skill: `terminal list|read|send|wait --for tui-idle`, `worktree create`, `agent launch`. https://www.onorca.dev/docs/cli/orchestration [O]
- Data: run (namespace + inbox), task (deps, status), dispatch (one attempt; carries taskId+dispatchId so stale retries can't complete the wrong one), messages status|dispatch|worker_done|escalation|question|heartbeat; nesting depth default 4; 3 failed starts → circuit breaker; SQLite in app. [S]
- Auth: `orca-runtime.json` with a plain auth token + socket/pipe; identity env `ORCA_TERMINAL_HANDLE`, `ORCA_PANE_KEY`, `ORCA_WORKTREE_ID`; special-cases Codex stripping env (falls back to pane key + cwd). [S]
- Skill install: thin stub (`npx skills add stablyai/orca --skill orchestration`); full guide bundled in the binary (`orca skills get orchestration`) so it never drifts. [O]
- Delivery to a terminal agent: wait for TUI idle, type a "mailbox pointer" command, the agent runs `check`. Chat agents get each message as a chat message. Coordinator rule: start workers, **end your turn**, never poll. Steering a working worker is delivered at turn end. [S]
- UI: children nested under parent workspace (also across projects); finished workers auto-close unless `--keep-open`; release only with proof of exit. [O/S]

## Claude Code primitives
- Cross-session messaging (v2.1.224+, Windows v2.1.234+): `ListAgents`, `SendMessage`, `notify_when_idle`; per-session socket/pipe + token env; inbound accept|hold|refuse; 50 queued, loop throttling; receiving Claude told the message is from another session and cannot grant consent. https://code.claude.com/docs/en/cross-session-messaging [O]
- Agent teams: JSON inbox per agent, file-locked task list, hooks TeammateIdle/TaskCompleted; no nesting. [O]
- Headless: `claude -p --output-format stream-json --input-format stream-json` allows queued messages + interrupt. [O]
- Hooks: Stop / Notification(permission_prompt, idle_prompt) / PermissionRequest give "done/blocked"; `additionalContext` (≤ 10k chars) and Stop `decision:block` can inject mail at turn boundaries. [O]
- Channels (MCP push into a session): research preview, gated. [O]

## Peers
- agent-deck (Go/tmux): closest analogue — `launch` from inside a session auto-parents; `session send` types keys; daemon transitions land in parent's inbox; `===AGENTDECK_DONE===` sentinel; group `max_concurrent`; worktree finish/cleanup. [S]
- claude-swarm: each connection = MCP server tool the lead calls. claude-code-mcp (steipete): one MCP tool spawning `claude -p`. Vibe Kanban: MCP tools (project shut down 2026-04). [T]
- Claude Squad, ccmanager, Conductor, Crystal/Nimbalyst, Sculptor: human-driven only, worktree per session. [O/T]
- Gemini CLI subagents: in-process only. Codex: `codex exec` no mid-run steering; app-server has `turn/steer`/`turn/interrupt` [O, params U]. ACP: only cancel; queue between turns. [O]

## Recommendation (researcher)
CLI first (token surface exists, Orca/agent-deck precedent), MCP as a thin adapter; push delivery order: chat/ACP structured path → Claude hook injection → typed text after idle; automatic reports from hooks; caps (depth, concurrency, retry breaker); children inherit restrictions; prompt-injection header on delivered mail; never let a message answer a permission prompt.

## Coordinator's decision (see plan)
Build both fronts in one phase (MCP is needed anyway because Codex strips `*TOKEN*` from its shell env, and MCP needs no install when injected at launch); typed full message (not a pointer) at idle as the universal delivery; hook injection left out of v1.

## Unresolved
Orca thread model in current build; Codex `turn/steer` params; Gemini `-p`/`--acp` mid-run input; whether Claude's Stop `additionalContext` wakes an idle TUI.
