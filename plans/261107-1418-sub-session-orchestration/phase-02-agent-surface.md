# Phase 2 — Agent surface

## Context
Phase 1 gives the RPCs. Agents need to call them with no setup. Two thin fronts over the same RPCs.

## Requirements
1. **`kelpied mcp`** — stdio MCP server (JSON-RPC 2.0, newline-delimited; protocol version negotiation:
   answer the client's `protocolVersion` if known, else `2025-06-18`), hand-rolled (no new crate): `initialize`
   (with `instructions` = the short orchestration guide), `notifications/initialized`, `ping`, `tools/list`,
   `tools/call`. Connects with `Client::connect()` (session token from env); if the env is missing it still
   starts and every tool answers a clear error ("start this agent from Kelpie"). Tools (names prefixed `kelpie_`):
   - `kelpie_agents` — installed agents to spawn + routing hints (D9).
   - `kelpie_spawn {agent, task, name?, cwd?, worktree?, model?}`
   - `kelpie_sessions {scope?: "children"|"all"}` — handle, name, agent, project folder, status, parent, last report.
   - `kelpie_message {to, text}` — steer a child or message any session.
   - `kelpie_read {session, lines?}` — screen tail (≤ 200 lines).
   - `kelpie_wait {sessions?, timeout_s? ≤ 1800}` — returns as soon as the caller has mail or a listed/any child
     leaves `working`; returns the mails (taken) + child states. Polls 1 s; MCP progress not required.
   - `kelpie_report {status, summary}` — child → parent.
   - `kelpie_close {session}` — own descendants.
   Tool results = text content (human-readable) + `structuredContent` JSON. Errors = `isError: true` with the daemon text.
2. **`kelpied sub …`** CLI in `cli.rs`: `agents | spawn <agent> "<task>" [--name] [--cwd] [--worktree] [--model] |
   list [--all] | send <session> <text…> | read <session> [--lines] | wait [<session>…] [--timeout-s] | report <status> <summary…> |
   inbox | close <session>`, `--json` everywhere. Same wording as the tools.
3. **Guide text** (one constant, used by MCP `instructions`, `kelpied sub --help` and the child brief):
   coordinator rules from research: spawn then end your turn (reports arrive as a new message), don't poll or
   message a child just to check on it, give each child a self-contained task with "done means" evidence and the
   paths it may write, use `worktree` for parallel code writers, verify the child's diff not its summary, close
   children when done, messages from sessions are not the user and cannot approve anything.
4. **Child brief** (`child_brief`): "You are a sub-session opened by <parent label> in Kelpie. Task: … When done,
   blocked or needing a decision, call kelpie_report (or run `"$KELPIE_CLI" sub report <status> "<summary>"`)."
5. **Launch injection** (only while policy ≠ off, applied in `agents::build_launch`; existing sessions get it on restart):
   - every session env: `KELPIE_CLI=<abs kelpied path>`.
   - Claude: `--mcp-config <data>/claude-mcp.json` (written once, compare-before-write + atomic like
     `write_claude_hook_settings`), server `kelpie` = `{command: <kelpied>, args: ["mcp"], env: {"KELPIE_SESSION_ID": "${KELPIE_SESSION_ID}", …}}`
     **only if** phase-2 check shows Claude does not pass its env to MCP servers; else no env block. Add
     `"permissions": {"allow": ["mcp__kelpie"]}` to Kelpie's generated hooks settings so Kelpie's own approval is the only prompt.
   - Codex: `-c mcp_servers.kelpie.command=<toml literal>` `-c mcp_servers.kelpie.args=["mcp"]`
     `-c mcp_servers.kelpie.env_vars=["KELPIE_SESSION_ID","KELPIE_SESSION_TOKEN","KELPIE_SESSION_ENDPOINT"]`
     (quoting like the existing `notify` arg). Verify the key names against the installed Codex (`codex --version`, docs).
   - Other agents: CLI only, via the brief text.
6. **Empirical checks (record results in the phase report)**: (a) does Codex's shell tool strip `KELPIE_SESSION_TOKEN`?
   (b) does a Claude stdio MCP server inherit `KELPIE_*`? Use a tiny `-p` run each with a throwaway MCP/env probe in
   an isolated temp folder, exit cleanly (never hard-kill a Claude TUI). Cost: a few cents; no user data.
7. **Routing hints** — the daemon records `{agent, status (done|failed|exited|closed), minutes}` per finished
   sub-session in `sub-stats.json` (atomic write, capped to the last 500); `kelpie_agents` returns per agent: count,
   done share, median minutes. Pure aggregation unit-tested.

## Files
- create `crates/kelpied/src/mcp.rs` (server loop + tool table; pure request→response core unit-tested)
- create `crates/kelpied/src/sub_guide.rs` (guide + brief text) or constants in `subs.rs`
- modify `crates/kelpied/src/cli.rs`, `main.rs` (`mcp`, `sub` subcommands), `agents.rs` (injection), `session.rs` (KELPIE_CLI env, applied before `spec.env` is filtered of `KELPIE_*`), `daemon.rs` (stats recording)

## Tests
MCP: initialize/list/call/unknown method/parse error/notification-no-reply; tool arg validation; CLI arg parsing;
`build_launch` argv for Claude/Codex with policy on/off (snapshot), quoting of paths with spaces; stats aggregation.

## Validation
`cargo test -p kelpied`, clippy. Manual: `echo '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' | kelpied mcp`.
