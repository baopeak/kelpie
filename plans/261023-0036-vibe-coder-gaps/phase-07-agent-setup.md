# Phase 7 — Agent setup: MCP servers & instruction files

## Context
Each agent keeps MCP servers and instructions in its own place and format; Kelpie shows none of it.
Multi-agent users copy configs by hand and end up with drift.

## Requirements
- Project start page + Settings → "Agent setup":
  - **MCP servers** per agent and scope (project / user): Claude `.mcp.json`, `~/.claude.json`
    (`mcpServers`, per-project entries); Codex `~/.codex/config.toml` `[mcp_servers]`;
    Gemini `~/.gemini/settings.json` / `.gemini/settings.json`; OpenCode `opencode.json`.
    Show name, command/url, scope; enable/disable; "Copy to…" other agents (format conversion);
    honour the active account's home (accounts feature).
  - **Instruction files**: CLAUDE.md, AGENTS.md, GEMINI.md of the project — open in editor;
    "Use one file for all agents" (AGENTS.md source; CLAUDE.md = `@AGENTS.md` import line,
    GEMINI.md via `contextFileName` or copy) — shown as a diff before writing.
- Every write: preview diff, backup file next to the app data, then write; never touch secrets
  in `env` blocks (masked in UI).

## Files
- Create: `app/src-tauri/src/agent_config.rs` (read/convert/write; `toml_edit` to keep comments),
  `app/src/components/agent-setup.tsx`.
- Modify: `app/src-tauri/Cargo.toml` (`toml_edit`), `app/src-tauri/src/lib.rs`,
  `app/src-tauri/src/accounts.rs` (home paths), `app/src/components/welcome.tsx` (project start page link —
  coordinate: another session edits this file for the mascot), `app/src/components/settings/settings-page.tsx`, locales, docs.

## Validation
- `cargo test`: round-trip each format on fixtures (comments and unknown keys preserved);
  Claude → Codex → Gemini conversion of stdio and http servers; env values never logged.
- e2e `scripts/e2e/agent_setup.py`: fixture homes (`KELPIE_*` home overrides) → list, toggle,
  copy to Codex, diff preview, backup created.

## Risk / rollback
- Corrupting a user config → preview + backup + format-preserving editors; restore button from backup.
