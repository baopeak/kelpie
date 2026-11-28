# Verification: agent hooks (Gemini, Devin), Claude cache TTL, Devin Fusion

Date: 2026-11-28. Installed on the owner's machine: gemini 0.71.1, devin 3000.15.14, codex (npm).

## Q4: which prompt-cache TTL Claude Code uses (measured locally, numbers only)

- Main conversations: 40 newest transcripts, 29,873 usage records.
  - `ephemeral_1h_input_tokens` 209M;
  - `ephemeral_5m_input_tokens` **0**;
  - cache reads 12.2B.

  **Main conversations write the 1-hour cache.**
- Subagent transcripts (`*/subagents/*.jsonl`, 60 newest): 1h **0**, 5m 48M. **Subagents write the 5-minute cache.**
- Consequences:
  - A Kelpie sub-session is its own main Claude process, so it gets 1 hour.
  - A session idle under ~55 minutes resumes warm; past 60 minutes its next turn re-writes the whole context at 2× the input price.
  - The thresholds for parking and for holding mail are therefore per agent: Claude main 55 minutes, unknown agents 5 minutes (the conservative default).

## Q3: pre-execution hooks of Gemini CLI and Devin CLI

| | Gemini CLI | Devin CLI |
|---|---|---|
| Event / shell tool | `BeforeTool`, tool `run_shell_command`, args in `tool_input` | `PreToolUse`, tool `exec`, `tool_input.command` |
| Holds the tool | Yes, "waits for all matching hooks" | Implied (blocking semantics); not stated for PreToolUse |
| Timeout | `timeout` ms, default 60000 | `timeout` seconds, default not documented |
| Allow / deny | `decision: allow \| deny` + `reason` (sent to the model as a tool error); exit 2 blocks | `decision: approve \| block` + `reason`; exit 2 blocks |
| Rewrite command | `hookSpecificOutput.tool_input` (merged) | `hookSpecificOutput.updatedInput` (merged) |
| Message to model | `additionalContext` (AfterTool, BeforeAgent); `systemMessage` shows to the user only | `additionalContext` on PostToolUse/UserPromptSubmit/SessionStart |
| Turn start / end | `BeforeAgent` / `AfterAgent`; `SessionStart`/`End` | `UserPromptSubmit` / `Stop`; `SessionStart`/`End` |
| Config | `.gemini/settings.json` (project), `~/.gemini/settings.json`, system file; **no per-run flag documented** | `.devin/hooks.v1.json`, `.devin/config.json`, `%APPDATA%\devin\config.json`, and it also reads `.claude/settings*.json` hooks by default; **no per-run flag documented** |
| On error or timeout | Fails open | Fails open |

Today Kelpie wires hooks only for Claude (`--settings`, per run) and Codex. Gemini and Devin are status-detected from the screen (`choices.rs`).

Integration decision: install **one user-level hook entry** for each of the two CLIs.
- The entry runs `kelpied hook gemini|devin` and exits at once when `KELPIE_SESSION_ID` is unset, so it is inert outside Kelpie.
- It is installed with a marker key, merged without touching the user's other hooks, and removed when the setting is turned off.
- Devin's reading of `~/.claude/settings.json` does not double up, because Kelpie never writes that file (Claude gets `--settings`).
- Remote (SSH/tmux) sessions stay ungated.
- The Gemini docs carry a banner: unpaid and Google One users were moved to "Antigravity CLI" on 2026-06-18. Re-check that CLI's hooks when it is added.

## Devin Fusion (the owner's clarification, confirmed by the docs)

`/fusion` in Devin CLI 3000.10.20+ (paid plans) pairs a **lead** frontier model and a **sidekick** cost-effective model:
- the lead plans, makes design decisions, investigates and **reviews**;
- the sidekick does the mechanical implementation;
- the two exchange only briefs, results and feedback, not whole conversations;
- `/session-stats` shows cost by model and the Fusion savings.

The recommended pair is Fable 5.1 + SWE-2. For Kelpie this is a routing pattern, applied in phase 6 of the plan.

## Sources

- https://geminicli.com/docs/hooks/reference/ , https://geminicli.com/docs/hooks/
- https://docs.devin.ai/cli/extensibility/hooks/overview , https://docs.devin.ai/cli/extensibility/hooks/lifecycle-hooks
- https://docs.devin.ai/cli/fusion , https://cognition.com/blog/fusion-in-devin-cli
