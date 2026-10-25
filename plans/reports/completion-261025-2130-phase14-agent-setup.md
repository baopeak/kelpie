# Completion — Phase 14: Agent setup (MCP, instructions, skills)

Date: 2026-10-25 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-14](../261023-0048-multi-agent-ide-roadmap/phase-14-agent-setup.md)

## Delivered
- **Setup page** (lazy `agent-setup.tsx`; Settings section, palette, project start link) over `agent_config.rs`: MCP servers of Claude Code (`~/.claude.json`, `.mcp.json`), Codex (`config.toml`), Gemini (`settings.json`), OpenCode (`opencode.json(c)`), Cursor (`.cursor/mcp.json`), user and project scope, tagged `user` / `project · untrusted`; on/off where the agent has one; Copy to another agent or scope with format conversion (Codex `env_vars` / `env_http_headers` / `bearer_token_env_var`; references rewritten per agent).
- **Trust**: copying a project server to a user file shows everything it would run (argv, folder, every env / header entry as written, references to the user's variables named), needs a confirm enforced in Rust, carries no environment unless opted in, pins relative and bare in-project paths to absolute ones. User secrets never reach the UI or a project file; diffs masked by parsed position. Project paths that lead out of the repository through a link or junction are refused, also re-checked at write time.
- **Instructions**: CLAUDE.md / AGENTS.md / GEMINI.md / `.kiro/steering` / Copilot; "one file for all agents" (AGENTS.md source), already-unified links detected. Skills, commands and subagents listed read-only.
- **Find more agents**: one ACP registry request per click, exact version pinned (tags / ranges / URL specs resolved or refused), confirm, saved as a chat-only custom agent through `agent.set_custom`; never run.
- **Writes**: preview → Claude's own `~/.claude.json.lock` → compare with the previewed text → private backup (newest 20) → unique temp file + fsync + rename (never in place) → parse; a broken write of Kelpie's own is put back only while the file still holds what Kelpie wrote. Same TOML writer for `accounts.rs`.

## Verification
- cargo: kelpie 169 (46 in `agent_config`: round-trips keep comments / order / unknown keys / CRLF / BOM / big ints, conversions, env never in project files, lock and stale-base refusal, rollback only of own writes, links out of the project, unified links, untrusted copy contents, registry pinning), kelpied 208, kelpie-proto 19; clippy `-D warnings` clean.
- vitest 451, typecheck, i18n 0 missing; entry bundle 321 kB gz (page lazy, 8.9 kB).
- GUI e2e `agent_setup.py` on fixture homes (real configs checked unchanged by size / mtime): 84/84 (twice by the developer; again on the merged code); regression `spec_panel` 124, `best_of_n` 91, `finish` 72, `context_cost` 26, `attention` 12/12. Cold first read in a release build 12.7 ms.
- [Review](code-reviewer-261025-1300-phase14-agent-setup.md): H1–H4, eight Mediums, R1, R2 and cheap Lows fixed; Claude confirmed to expand `${VAR}` in user scope; no blocking items.

## Not done / follow-ups
- File-symlink tests are unix-only and were not run (no symlink privilege on this Windows account; junctions and hard links covered).
- Debug-build e2e cold-read check can trip under machine load; `ui_tour.py` screenshots not run.
- Left out by design: env values to the keyring, Claude local-scope MCP inside `.claude.json`, Codex project config, registry entries that ship as downloaded binaries.
