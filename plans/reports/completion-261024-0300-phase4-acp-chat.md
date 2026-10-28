# Completion — Phase 4: Universal ACP chat

Date: 2026-10-24 · Plan: [multi-agent IDE roadmap](../261023-0048-multi-agent-ide-roadmap/plan.md) · Phase: [phase-04](../261023-0048-multi-agent-ide-roadmap/phase-04-universal-acp-chat.md)

## Delivered
- **Which agents chat**: `ACP_ARGS` in `app/src/lib/agents.ts` (OMP `omp acp`, Gemini `gemini --acp`, OpenCode `opencode acp`, Copilot `copilot --acp`, Goose `goose acp`, Qwen `qwen --acp`, Kimi `kimi acp`, Auggie `auggie --acp`, Cursor `cursor-agent acp`; flags from the [research](researcher-261024-0110-acp-agents-launch.md)); plugin agents declare `acp` (manifest → `agent.set_custom` → `AgentDef.acp` → `agent.presets`). `supportsChat` = Claude, Codex, or an ACP argv. Windows npm shims resolve through `agents::platform_argv`.
- **Generic adapter** (`acp-adapter.ts`): protocol v1 check; `initialize` with `auth.terminal`, form `elicitation`, `clientInfo` + version; sign-in (`auth_required` → card; `agent` methods → `authenticate`, `terminal` methods named, retry; failed sign-in offers it again; a failed `session/load` falls back to `session/new`); older `modes` / `models` blocks with `session/set_mode` / `set_model`; `elicitation/create` form → question card (required fields, enums, booleans, arrays; Accept for field-less forms; URL mode declined), Stop cancels open questions; agents' permission option ids echoed; usage cost kept across updates without one.
- **Daemon** (`chat.rs`): ACP `session/update` counts as work only for message / thought / tool-call updates; elicitation → waiting; the answer to `session/new|load` → idle (no 2-minute "working" after a resumed chat's replay); `session_info_update` titles become the session title. API level 8.
- **Safety**: chat→terminal resumes by id only for Claude / Codex / OMP; "Continue in chat view" hidden for remote sessions; the plugin trust review lists the chat-view command line too.
- Coexists with another session's `/new` / `/clear` / `/reset` handling in the same adapter (not part of this phase's commit).

## Verification
- cargo: kelpied incl. classifier, titles, `kelpie-session` idle; vitest `acp-protocol.test.ts` 12/12 (sign-in, version, legacy settings, elicitation incl. optional/array/empty forms, Stop, failed sign-in, failed resume, option ids); typecheck, i18n clean.
- GUI e2e [`acp_chat.py`](tester-261024-0200-phase4-acp-chat.md) with the fake ACP agent as a real plugin agent: 54/56, both failures fixed afterwards (trust review, cost kept). Regression: attention 12/12, smoke 16/16.
- [Review](code-reviewer-261024-0200-phase4-acp-chat.md): C1 (fake agent, test code), H1–H4, M1, M4, M5, M7, M8 fixed.

## Not done / follow-ups
- M2: terminal sign-in methods are shown as a command to run, not started for the user.
- M3: Cursor's own `cursor/ask_question` / `cursor/create_plan` are answered "method not found" (unblocks the agent; the question is not shown).
- M6: the daemon does not track turns; a late update after a turn or an agent that never confirms a cancel can leave "working" until output stops.
- Real third-party agents not installed here: Gemini, OpenCode, Copilot, Goose, Qwen, Kimi, Auggie, Cursor were checked against the schema and research only.
- Extra launch arguments from the new-session menu are not passed in chat mode (preexisting).
