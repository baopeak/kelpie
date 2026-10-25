---
phase: 14
title: "Agent setup: MCP, instructions, skills"
status: completed
priority: P2
effort: "3d"
dependencies: [1]
---

# Phase 14: Agent setup: MCP, instructions, skills

## Overview
One page for MCP servers, instruction files and skills across agents, with trust and secrets handled explicitly.

## Requirements
- **MCP servers** per agent and scope (Claude `.mcp.json` / `~/.claude.json`, Codex `config.toml`, Gemini `settings.json`, OpenCode `opencode.json`, Cursor `.cursor/mcp.json`):
  list, enable/disable, **Copy to…** with provenance [RT-15]:
  - each server tagged `project` (from the repo, untrusted) or `user`;
  - project → user copy shows the exact command + warning and needs explicit confirm;
  - env values never written into project-scope files; copying env to user scope or keyring is opt-in; values masked in UI and never logged.
- **Instructions**: CLAUDE.md, AGENTS.md, GEMINI.md, `.kiro/steering`, `.github/copilot-instructions.md`; "One file for all agents" (AGENTS.md source; CLAUDE.md `@AGENTS.md`; GEMINI.md via `contextFileName` or copy).
- **Skills / commands / subagents**: read-only list, open file.
- **Find more agents** (ACP registry, one request on click): shows full command, pins version, confirm before saving the custom agent (via `agent.set_custom`, capability `app`); never auto-runs.
- Every write: diff preview → backup → write; restore from backup; auto-restore if the file fails to parse after write.

## Architecture
App core `agent_config.rs` with a neutral `McpServer`. One writer per file [RT-15]: `accounts.rs` `codex_file_store` switches to the same `toml_edit` writer in this phase;
JSON keeps unknown keys/order (serde_json `preserve_order`).

## Related Code Files
- Create: `app/src-tauri/src/agent_config.rs`, `app/src/components/agent-setup.tsx`.
- Modify: `app/src-tauri/Cargo.toml`, `app/src-tauri/src/lib.rs`, `app/src-tauri/src/accounts.rs` (shared toml writer), `app/src/components/settings/settings-page.tsx`,
  `app/src/components/welcome.tsx` (project start link — coordinate with the mascot work in the same file), locales, docs.

## Implementation Steps
1. Readers + provenance + round-trip tests. 2. Shared toml writer (migrate `codex_file_store`). 3. Copy with conversion + trust/secrets rules. 4. UI, instructions sync, skills list, registry.

## Tests
- `cargo test`: round-trips keep comments/unknown keys; conversions; env never in project files; `accounts.rs` re-seed after an MCP copy keeps the server.
- e2e `agent_setup.py`: fixture homes → list, toggle, copy project→user warns, diff preview, backup, auto-restore on bad write.

## Perf gate
- Reads only on open (≤ 50 ms); zero background work.

## Success Criteria
- [x] Add an MCP server once, usable in Claude, Codex and Gemini; no untrusted server promoted silently.

## Risk Assessment
- Config corruption → preview + backup + auto-restore.
