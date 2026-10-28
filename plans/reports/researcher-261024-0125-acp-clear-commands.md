# ACP `/clear` `/new` `/reset` behaviour per agent (as of 2026-10-24)

Method: source read at default-branch HEAD via `gh api` (src), vendor docs (docs), my inference (inf = unverified). Claude/Codex out of scope.

## Table

| Agent | Listed in available_commands_update | `/clear` sent as prompt text in ACP | Effect on sessionId | TUI name / aliases |
|---|---|---|---|---|
| Gemini CLI | NO. Only `memory, extensions, init, restore, about, help` (src) | Not matched -> falls through to model as plain text (src) | n/a, no reset | `clear`, alt `new` (src) |
| OpenCode | NO. Server/user commands + builtin `compact` only (src) | Not a known command -> `session.prompt` with raw text to model (src) | n/a, no reset | `/clear`, `/new` (separate entries; both navigate to a fresh session) |
| Copilot CLI | Not documented as listed; docs say only a subset works over ACP (docs) | Docs: text not in advertised list is "an ordinary prompt" to the model (docs) | n/a | `/clear [PROMPT]`, `/new`, `/reset` = "Start a new conversation" (docs) |
| Goose | YES `clear` (+ `compact, prompts, prompt, skills, doctor, goal, ...`); no `new`/`reset` (src) | Handled as command: conversation emptied in SAME session, assistant msg "Conversation cleared" (src) | unchanged (inf: op runs inside session state machine) | `/clear`, `/new` (src) |
| Qwen Code | YES `clear` (supportedModes includes `acp`) (src) | Handled: internally rotates config session id but ACP Session object/id stays; returns info msg "Context cleared. Previous messages are no longer in context." (src) | ACP sessionId unchanged (inf); no session_info_update on clear found | `clear`, alt `reset`, `new` (src) |
| Kimi CLI | YES `clear` (soul registry: init, compact, clear, yolo, afk, plan, add-dir, export, import) ; NOT `new`, NOT alias `reset` (src) | Handled in `KimiSoul.run`: clears context in SAME session, text "The context has been cleared." + status update (src) | unchanged | `/clear` alias `reset`; separate `/new` (shell-only, creates a new Session) (src) |
| Cursor CLI | Unknown (closed source, docs silent) | Unknown | Unknown | `/clear`, aliases `/new`, `/new-chat`, `/newchat` (docs) |
| Oh-My-Pi | NO. `new`/`clear` have only `handleTui`, no ACP `handle`; list is filtered to commands with `handle` (src) | `executeAcpBuiltinSlashCommand` returns false -> "forward the input as a prompt" i.e. model sees text (src) | n/a | `/new` = new session, `/clear` = "Clear the conversation context in place, keeping the session"; no aliases (src) |

## Evidence per agent

**Gemini** (google-gemini/gemini-cli)
- Registry only registers Memory/Extensions/Init/Restore/About/Help: `packages/cli/src/acp/acpCommandHandler.ts` L24-30.
- Advertised via `sendAvailableCommands` -> `available_commands_update`: `packages/cli/src/acp/acpSession.ts` ~L232-240.
- Prompt starting with `/` or `$` -> `commandHandler.handleCommand`; returns true only if a registered command matched, else normal model turn: `acpSession.ts` L349-372; `acpCommandHandler.ts` L45-54.
- TUI: `packages/cli/src/ui/commands/clearCommand.ts` L19-20 (`name:'clear'`, `altNames:['new']`).
- Client action: send `session/new` itself (docs `docs/cli/acp-mode.md` L83).

**OpenCode** (anomalyco/opencode, dev @755bbc7)
- `packages/cli/src/acp/catalog.ts` L9-11: `builtinCommands` = only `compact`; L168 server commands filtered. `config-option.ts` L70-75 `availableCommands` = server commands + builtins.
- `prompt.ts` L30-41, `turn.ts` L235-257: `compact` -> `session.compact`; command in catalog -> `session.command`; else `session.prompt` with raw text. So `/clear` => model text (unless user defines a custom command named `clear`).
- TUI: `packages/tui/src/app.tsx` L774 `slash:{name:"new"}`, L800 `slash:{name:"clear"}`; both navigate to home/new session.

**Copilot CLI** (closed source; github/copilot-cli has only changelog)
- Docs https://docs.github.com/en/copilot/reference/cli-command-reference : row "`/clear [PROMPT]`, `/new [PROMPT]`, `/reset [PROMPT]` - Start a new conversation"; "a subset ... available to clients that use the CLI via its ACP server".
- ACP page https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server : commands sent as a single text block; ACP-supported examples `/compact /env /model /mcp /research /session /rename /context /usage` + skills; interactive-only (`/diff /resume /settings /login /help /tasks /undo`) do not run; "If a command isn't in this list, the server treats the text as an ordinary prompt". Page never mentions clear/new/reset.
- Changelog confirms ACP slash set grew by /compact /context /usage /env (changelog.md ~L1870). No ACP clear entry (grep `/clear|/new|/reset`). Verdict: inf not supported; verify empirically via the advertised list.

**Goose** (aaif-goose/goose, formerly block/goose)
- `crates/goose/src/agents/execute_commands.rs` L17-50: COMMANDS incl. `clear` "Clear the conversation history". ACP list built from it: `acp/response_builder.rs` L469-480, `slash_commands/slash_command.rs` `list_acp_commands`.
- Handler: `agents/state_machine/ops_compaction.rs` L110-125 `clear()`: replaces conversation with empty, emits "Conversation cleared".
- TUI: `crates/goose-cli/src/session/input.rs` L232-233, L440-441: `/clear` clears history; `/new` starts fresh session in-process.
- Can be disabled by config `GOOSE_SLASH_COMMANDS_ENABLED=false` (list empty).

**Qwen Code** (QwenLM/qwen-code)
- `packages/cli/src/ui/commands/clearCommand.ts` L28-36: `name:'clear'`, `altNames:['reset','new']`, `supportedModes:['interactive','non_interactive','acp']`; ACP return L151-157 info message. Calls `config.startNewSession()` (new internal id) + `llmClient.resetChat()`.
- ACP dispatch: `packages/cli/src/acp-integration/session/Session.ts` ~L7003-7015 `handleSlashCommand(...startNewSession: rebindGoalRuntimeForNewSession)` - comment says Session is "long-lived" across `/clear`; `buildAvailableCommandsSnapshot` ~L2330-2380 lists built-ins filtered by mode.
- Caveat: `STANDALONE_SLASH_COMMAND_POLICY` (L2301-2315) sets `allowSessionReset:false` and blocks `clear` for "standalone" sessions (daemon/bridge profile); in that policy `/clear` is refused. Normal `qwen --acp` assumed not standalone (inf). Check `available_commands_update` at runtime.
- Whether ACP-visible sessionId stays equal to the internal `config.getSessionId()` after rotation: not verified; session_info_update is only emitted for title changes (L11900-11918).

**Kimi** (MoonshotAI/kimi-cli)
- ACP advertises `soul_slash_registry.list_commands()` primary names only: `src/kimi_cli/acp/server.py` L183-195.
- `src/kimi_cli/soul/slash.py` L80-94: `@registry.command(aliases=["reset"]) clear` -> `soul.context.clear()`, rewrites system prompt, `wire_send("The context has been cleared.")` + StatusUpdate. Alias `reset` still dispatches (`utils/slashcmd.py` alias map) but is not listed.
- Prompt path: `acp/session.py` L155-165 -> `cli.run` -> `soul/kimisoul.py` L725-731 slash parse; unknown slash -> "Unknown slash command".
- Shell-only (not ACP): `src/kimi_cli/ui/shell/slash.py` L538-567: `/new` and `/clear`(alias reset) create new Session (new id); ACP uses the soul version instead (in-place).

**Cursor CLI**: docs only. https://cursor.com/docs/cli/reference/slash-commands : "`/clear` Start a new chat session. Aliases `/new`, `/new-chat`, `/newchat`". https://cursor.com/docs/cli/acp lists only session/new, session/load, prompt, update, request_permission, cancel; no slash-command mention. Unverified: ACP list/behaviour.

**Oh-My-Pi** (can1357/oh-my-pi)
- `packages/coding-agent/src/slash-commands/builtin-lifecycle.ts` L160-200: `new` and `clear` define only `handleTui`.
- `slash-commands/acp-builtins.ts` L38-47: ACP list filtered to `command.handle !== undefined`; L55-66 dispatcher returns `false` when no ACP `handle` -> caller forwards as prompt. `modes/acp/acp-agent.ts` L1195 uses it.
- So `/clear`/`/new` reach the model as text (inf from filter logic; not run).

## Client implications (recommendation)
1. Never rely on text `/clear`. Safest uniform rule: Kelpie keeps intercepting `/clear` `/new` -> `session/new` (current behaviour) for ALL ACP agents. Works everywhere: Gemini, OpenCode, OMP, Copilot (and likely Cursor) would otherwise burn a model turn on "/clear".
2. Only in-place variants (Goose, Qwen, Kimi) reset context without changing sessionId; none emits a new id. If Kelpie forwarded text there, keep its sessionId and just wipe transcript on the "cleared" message - but this is agent-specific and inconsistent (Goose `new`, Kimi `new`/`reset`, Qwen standalone-policy refusal), so not worth it.
3. Trade-off of intercepting: old session persists in agent history (resumable via session/list); acceptable. If the agent supports `session/close`, optionally close the old id (capability-gated) to free resources.
4. If `/clear` is passed through anyway, detect handled-ness by presence of `clear` in the last `available_commands_update` snapshot (Copilot docs state this rule explicitly; same logic holds in Gemini/OMP/OpenCode source).

## Unresolved
- Copilot, Cursor: no source; need runtime probe of `available_commands_update`.
- Qwen: ACP sessionId vs internal rotated id after `/clear` unverified; standalone-policy scope unverified.
- Goose: confirm the clear op executes on the ACP prompt path (strong inference only).

Status: DONE_WITH_CONCERNS
Summary: Source-verified for Gemini, OpenCode, Goose, Qwen, Kimi, OMP; Copilot docs-only; Cursor unknown. Only Goose/Qwen/Kimi list `clear` and reset in place (same sessionId); the rest pass `/clear` to the model, so keep intercepting with `session/new`.
