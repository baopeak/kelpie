# ACP agents: launch argv, capabilities, auth, quirks (as of 2026-10-24)

Method: ACP registry JSON (`cdn.agentclientprotocol.com/registry/v1/latest/registry.json`, parsed), ACP schema JSON on GitHub (`schema/v1`, `schema/v2`), and agent source read via `gh api` at HEAD. "src" = read from source; "docs" = vendor docs; "inf" = my inference, unverified.

## 1. Spec surface a client must implement (stable protocolVersion = 1)

- Current stable: `protocolVersion: 1` (integer, bumps only on breaking change). Schema release `schema-v1.26.0` (2026-10-16). A **v2 is in alpha** (`2.0.0-alpha.9`, 2026-10-16: `auth/login`, `auth/logout`, no `session/load`/`set_mode`/fs/terminal methods in v2 meta). Claude (`CLAUDE_AGENT_ACP_PROTOCOL=auto`) and Codex adapters serve v1 or v2 depending on what the client sends. **Send `protocolVersion: 1`**; if agent answers with something else, close and report.
- Transport: JSON-RPC 2.0, newline-delimited JSON on stdio. stdout = protocol only; logs on stderr.
- Registry/page: 46 agents on https://agentclientprotocol.com/overview/agents ; registry repo https://github.com/agentclientprotocol/registry (44 folders).

### Methods (from schema/v1/meta.json)

| Direction | Method | Gate / note |
|---|---|---|
| C->A | `initialize` | required |
| C->A | `authenticate` `{methodId}` | required; only for `agent`-type methods, never send a `terminal` method id |
| C->A | `logout` | needs `agentCapabilities.auth.logout` |
| C->A | `session/new` `{cwd (absolute), mcpServers[], additionalDirectories?}` | required |
| C->A | `session/load` `{sessionId,cwd,mcpServers}` | needs `loadSession`; agent replays full history as `session/update` BEFORE responding `{}` |
| C->A | `session/resume` | needs `sessionCapabilities.resume`; no replay |
| C->A | `session/list` / `session/close` / `session/delete` | need `sessionCapabilities.list/close/delete`; close = cancel + free |
| C->A | `session/prompt` | required; returns `stopReason` |
| C->A (notif) | `session/cancel` | agent must finish prompt with `stopReason: cancelled` |
| C->A | `session/set_mode` | optional (modes still exist; also exposed as a config option) |
| C->A | `session/set_config_option` | optional; categories `mode`, `model`, `model_config`, `thought_level`, custom `_x`; boolean options need client `session.configOptions.boolean` |
| A->C | `session/request_permission` | required baseline; options kinds `allow_once/allow_always/reject_once/reject_always`; outcome selected/cancelled |
| A->C (notif) | `session/update` | see below |
| A->C | `fs/read_text_file`, `fs/write_text_file` | only if client advertises `fs.readTextFile/writeTextFile` |
| A->C | `terminal/create,output,wait_for_exit,kill,release` | only if client advertises `terminal: true` |
| A->C | `elicitation/create` (+ `elicitation/complete` notif) | **stable in schema v1.26**; client caps `elicitation.form` / `.url` must be explicit non-null; modes `form` (flat JSON schema, no secrets) and `url` (OAuth, out of band); actions accept/decline/cancel |
| both | `$/cancel_request`, `_`-prefixed ext methods/notifications, `_meta` on everything | |

**`session/set_model` is NOT in the v1 schema (stable or unstable meta).** Model selection = config option with `category: "model"`. Gemini, Qwen, OpenCode SDK handlers still implement `unstable_setSessionModel` (`session/set_model`) and some return a legacy `models` block in `session/new` (Kimi, Gemini). Client plan: prefer `configOptions`, fall back to `models`+`session/set_model` if no config options returned.

### `session/update` kinds (schema v1.26 stable)

`user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`, `tool_call`, `tool_call_update`, `plan`, `available_commands_update`, `current_mode_update`, `config_option_update`, `session_info_update` (title, updatedAt), `usage_update` (`used`, `size`, optional `cost`), `notice` (needs client `session.notices`), `compaction_update` + `compaction_summary_chunk` (need client `session.compaction`). Chunks carry `messageId`.
Unstable-only (do not rely): `plan_update`, `plan_removed`, `subagent_update`, `session_message(_chunk)`, `session/fork`, `providers/*`, `mcp/message`.
- StopReason: `end_turn, max_tokens, max_turn_requests, refusal, cancelled`.
- ToolKind: `read, edit, delete, move, search, execute, think, fetch, switch_mode, other`. ToolCallStatus: `pending, in_progress, completed, failed`.
- Agent caps fields: `loadSession`, `promptCapabilities{image,audio,embeddedContext}`, `mcpCapabilities{http,sse}` (stdio MCP always mandatory; SSE deprecated), `sessionCapabilities{list,delete,resume,close,additionalDirectories}`, `auth{logout}`. Every agent must accept `text` + `resource_link` prompt blocks.

### Auth model
- `authMethods[]` entries: `{id,name,description,type?}`; `type` omitted = `agent` -> call `authenticate{methodId}`; `type:"terminal"` (+`args`,`env`) -> client launches the **same agent command** interactively with those extra args appended, waits for exit 0, then reconnects and re-`initialize`. Agents may only offer terminal methods if client sent `clientCapabilities.auth.terminal: true`.
- Legacy variant (Zed era): client cap `_meta["terminal-auth"]: true`, method carries `_meta["terminal-auth"] = {command,args,label,env}` (Claude, OpenCode, Kimi, Copilot still emit it). Support both to be safe.
- Unauthenticated agents answer `session/new` with JSON-RPC error code `-32000` (`auth_required`), data may carry `authMethods` (Kimi does).
- Sources: https://agentclientprotocol.com/protocol/v1/authentication.md , /initialization , /session-setup , /elicitation (.md suffix works), https://github.com/agentclientprotocol/agent-client-protocol/tree/main/schema/v1

## 2. Per-agent launch matrix

Latest versions from registry 2026-10-24. Spawn from `cwd` = project dir where useful (Cursor needs this for .cursor/mcp.json); always still pass absolute `cwd` in `session/new`.

| Agent | Executable | Argv for ACP | Ver (registry) | Still experimental/renamed? |
|---|---|---|---|---|
| Gemini CLI | `gemini` (npm `@google/gemini-cli`) | `gemini --acp` | 0.65.0 (latest tag v0.65.0) | `--experimental-acp` is **deprecated alias** ("use --acp") in src config.ts; docs still call the mode stable-ish. Min version for `--acp`: not stated, **inf**: introduced between ~0.3x-0.6x; probe: try `--acp`, fall back to `--experimental-acp` on yargs "unknown option"/exit |
| OpenCode | `opencode` (binary; npm `opencode-ai`) | `opencode acp` | 1.19.4 (and `opencode-v2` 2.1.4 in registry) | stable subcommand; repo moved sst/opencode -> **anomalyco/opencode** |
| GitHub Copilot CLI | `copilot` (npm `@github/copilot`) | `copilot --acp` (stdio default; `--acp --stdio` explicit; `--acp --port N` = TCP on 127.0.0.1) | 1.0.103 | "public preview, subject to change" (GH docs, changelog 2026-01-28); flag stable since |
| Goose | `goose` | `goose acp` | 1.55.0 registry / 1.56.0 latest release | stable subcommand |
| Qwen Code | `qwen` (npm `@qwen-code/qwen-code`) | `qwen --acp` (registry also passes `--experimental-skills`, optional) | 0.26.0 | `--acp` current; (inf) older builds used `--experimental-acp`, same fork lineage as Gemini |
| Kimi CLI | `kimi` (Python, uv tool / registry ships binary) | `kimi acp` | 1.52.0 | stable subcommand |
| Augment Auggie | `auggie` (npm `@augmentcode/auggie`) | `auggie --acp` (registry sets env `AUGMENT_DISABLE_AUTO_UPDATE=1`) | 0.36.0 (GH releases now show v2 prereleases 1.2.0) | GA since changelog "no longer experimental" |
| Cursor | `cursor-agent` (docs alias `agent`) | `agent acp` / `cursor-agent acp`. Root options go BEFORE `acp` (`agent -e URL acp`, `agent -k acp`) | 2026.10.19 | stable subcommand |
| Claude Code (adapter) | npm `@agentclientprotocol/claude-agent-acp` (bin `claude-agent-acp`) | `npx -y @agentclientprotocol/claude-agent-acp@0.94.0` (no args) | 0.94.0 | **Renamed twice**: zed-industries/claude-code-acp -> zed-industries/claude-agent-acp -> agentclientprotocol/claude-agent-acp. Registry id `claude-acp` |
| Codex (adapter) | npm `@agentclientprotocol/codex-acp` (bin `codex-acp`; bundles `@openai/codex`, override with `CODEX_PATH`) | `npx -y @agentclientprotocol/codex-acp@2.2.2` (no args) | 2.2.2 | **Moved** from zed-industries/codex-acp to agentclientprotocol/codex-acp; major 2.x |
| Mistral Vibe | `vibe-acp` | no args | 3.0.1 | dedicated binary |
| Kiro CLI | `kiro-cli` (docs https://kiro.dev/docs/cli/acp/) | see docs (not in registry JSON; **not verified**) | n/a | n/a |
| Cline | `cline` (npm `cline`) | `cline --acp` | 3.0.73 | |
| Junie (JetBrains) | `junie` | `junie --acp=true` | 3813.9.0 | |
| Kilo | `kilo` (npm `@kilocode/cli`) | `kilo acp` | 7.8.17 | |
| Factory Droid | `droid` | `droid exec --output-format acp-daemon` (+env `DROID_DISABLE_AUTO_UPDATE=true`) | 0.248.0 | odd form |
| Devin CLI | `devin` | `devin acp` | 3000.11.205 | |
| Poolside | `pool` | `pool acp` | 1.0.16 | |
| Grok Build | `@xai-official/grok` | `grok agent stdio` | 1.0.69 | |
| Stakpak / VT Code / Crow / Harn | own binaries | `<bin> acp` (Harn: `harn serve acp`) | various | |
| Cortex Code | `cortex` | `cortex acp serve` | 1.0.73 | |
| Bob (IBM) / MiniMax / Dimcode / Nova / Minion | npm/uvx | `<pkg> acp` | various | |
| Qoder / CodeBuddy / Dirac | npm | `<bin> --acp` | various | |
| Kimchi | `kimchi` | `kimchi --mode acp` | 1.19.2 | |
| Pi | npm `pi-acp` | no args (adapter) | 0.0.34 | third-party adapter |
| Amp | `amp-acp` | no args (adapter, tao12345666333) | 0.10.0 | third-party |
| Antigravity | `agy_acp_server` | linux needs `--uid=`; other OSes no args | 1.4.1 | Google's Gemini CLI successor (Gemini docs: replaced for unpaid/Google One users 2026-06-18) |

Source for rows: registry.json (parsed); Gemini/OpenCode/Kimi/Qwen/Goose/Claude/Codex confirmed in source; Copilot via https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server ; Cursor https://cursor.com/docs/cli/acp ; Auggie https://docs.augmentcode.com/cli/acp ; OpenCode https://opencode.ai/docs/acp/ ; Gemini https://geminicli.com/docs/cli/acp-mode/ .

## 3. Initialize behaviour per agent (protocolVersion all answer 1 unless noted)

| Agent | loadSession | sessionCapabilities | promptCapabilities | mcpCapabilities | authMethods (ids) | Modes / model / extras |
|---|---|---|---|---|---|---|
| Gemini (src) | true | none advertised (no list/resume/close) | image, audio, embeddedContext all true | http+sse | `oauth-personal` (Google login), `gemini-api-key` (`_meta["api-key"]` in authenticate request carries key), `vertex-ai`, `gateway` (`_meta.gateway`). All `agent` type: `authenticate` does the work (opens browser for oauth-personal) | `session/set_mode` (modes = approval modes `default/autoEdit/yolo/plan`); `unstable_setSessionModel`; `ask_user` tool disabled in ACP mode; `ide` env detection |
| OpenCode (src v1.19.4) | true | close, fork, list, resume | image, embeddedContext (audio absent) | http+sse | one method `opencode-login`, "Run `opencode auth login`"; gets `type:terminal,args:["--login"]` only if client sent `auth.terminal`; `_meta["terminal-auth"]` if client opted in. Providers configured via `opencode auth login` / config | `set_mode`, `unstable_setSessionModel`, `set_config_option`, `unstable_forkSession`; `/undo` `/redo` unsupported. On main (v2 branch): adds delete, additionalDirectories, elicitation (form), compaction, `_meta["opencode/child-session-updates"]` client opt-in |
| Copilot (changelog) | yes ("ACP server supports loading existing sessions") | close (closeSession) | images; MCP http/stdio/sse accepted | stdio, http, sse | GitHub login; **terminal-auth** via `copilot login` subcommand; `authenticate` returns success only if logged in; or BYOK via `COPILOT_PROVIDER_*` / `COPILOT_PROVIDERS_CONFIG` (no login) | modes agent/plan, config options (model, reasoning effort, `allow_all`, custom agent), `usage_update`, plan updates, slash commands via `available_commands_update` (interactive-only ones like `/diff`,`/login` not run). Tool/effort/sandbox flags are fixed at server start (`--available-tools`, `--effort`, `--sandbox`) |
| Goose (src) | true | list, delete, close | image, embeddedContext; **audio false** | http only | single `agent` method `goose-provider` "Run `goose configure`" (no real login flow: client cannot complete it; user must configure provider/API key beforehand) | form elicitation supported; `_meta` goose-specific capabilities (`client.goose` in initialize `_meta`); custom `_goose/*` methods; fork_session; `ProtocolVersion::LATEST` (check it equals 1 at runtime) |
| Qwen (src) | true | list, resume | image, audio, embeddedContext | http+sse | one method id = `AuthType.USE_OPENAI` ("Requires `OPENAI_API_KEY`"), signalled via `_meta:{type:"terminal",args:["--auth-type=openai"]}` (non-standard, not top-level `type`) | `set_mode`, `unstable_setSessionModel`, `set_config_option`, `_meta.imageCapability` in agentCapabilities, heavy `_meta` use; accepts initialize `_meta` private-parent key |
| Kimi (src) | true | list, resume | image, embeddedContext; audio false | http only (no sse) | one method `login`, **no top-level type**; terminal info only in `_meta["terminal-auth"] {command,args:["login"],label,type:"terminal"}`. Unauthenticated `session/new` -> `auth_required` error with `data.authMethods` (includes terminal type/args/env) | returns `modes` (default) and `models` (legacy `SessionModelState`, thinking variants expanded into model ids) in `session/new`; negotiates version via ACP py SDK |
| Auggie (docs/changelog) | not documented in docs (changelog: "ACP integrations can access CLI session history"); **verify at runtime** | unknown | images + file mentions (changelog) | unknown | terminal authentication (changelog); or pre-auth `auggie login` / env `AUGMENT_SESSION_AUTH` (JSON from `auggie token print`) | docs: "not all interactive features supported in ACP mode"; model selection supported |
| Cursor (docs) | `session/load` supported; capability flag not documented -> read from initialize | unknown | unknown | project/user `.cursor/mcp.json` servers; team-dashboard MCP not supported | `cursor_login` (agent-type; `authenticate{methodId:"cursor_login"}` after `agent login`); also `--api-key`/`CURSOR_API_KEY`, `--auth-token`/`CURSOR_AUTH_TOKEN` as argv/env to the launch | modes `agent`, `plan`, `ask`; **non-standard permission option ids** `allow-once/allow-always/reject-once` (dash, not underscore): echo back the `optionId` the agent sent, don't hardcode; ext methods `cursor/ask_question`, `cursor/create_plan` (blocking, agent waits for reply), notifs `cursor/update_todos`, `cursor/task`, `cursor/generate_image`. Unanswered permission requests block the agent |
| Claude (src v0.94.0) | true | additionalDirectories, close, delete, fork, list, resume, systemPrompt (+subagents for AIR) | image, embeddedContext (audio absent) | http+sse | **Empty `authMethods` unless client advertises `auth.terminal` (or `_meta["terminal-auth"]:true`)**; then terminal methods `claude-ai-login` (args `--cli auth login --claudeai`), `console-login`, or `claude-login` on remote/SSH. `gateway`/`gateway-bedrock` agent methods only if `auth._meta.gateway === true`. Without them, auth is whatever the Claude SDK finds: existing `claude` login credentials or `ANTHROPIC_API_KEY` | `auth.logout`, `providers` (unstable providers/list,set,disable), `_meta.claudeCode.promptQueueing`, `_meta.claudeCode.options`, `_meta.jetbrains.air.*`, `_auth/status_update`, `_session/steering` ext, URL elicitation bridge for MCP OAuth, mode is a config option, `usage_update`, terminal output via client `terminal/*` |
| Codex (src v2.2.2) | true | resume, list, close, delete, fork | image, embeddedContext | stdio + http | `api-key` (env `CODEX_API_KEY`, fallback `OPENAI_API_KEY`), `chat-gpt` (browser; hidden if `NO_BROWSER=1`), `chat-gpt-device-code`, `gateway` (opt-in `_meta`). Already-logged-in `codex login` state (in `~/.codex`) is reused | modes `read-only/workspace-write/agent/agent-full-access` (`INITIAL_AGENT_MODE`), config options model/reasoning/fast/approval/sandbox, `CODEX_CONFIG` JSON env, `MODEL_PROVIDER`, protocol v2 branch chosen if client offers 2 |

## 4. Windows quirks

| Item | Finding |
|---|---|
| npm-distributed agents (Gemini, Copilot, Qwen, Auggie, Claude/Codex adapters, Cline) | Global npm installs create `name.cmd` (+`.ps1`) shims, not `.exe`. `npx` is `npx.cmd`. Spawning `gemini` without extension fails with ENOENT in Node `spawn` and Rust `Command` unless extension resolved. **inf**: resolve full path via `where`/PATHEXT lookup, then launch through `cmd.exe /d /s /c "<path.cmd>" args` (avoid shell injection: quote args; Rust >=1.77.2 hardening rejects unsafe `.cmd` args). Kill process tree on shutdown (`taskkill /T /F /PID`): the `.cmd` shim leaves `node.exe` child alive otherwise. |
| Node dependency | Gemini, Copilot (npm form), Qwen, Auggie, Cline, Claude/Codex adapters need Node (Gemini/Qwen 20+; adapters recent LTS, **inf**). Registry ships native binaries (no Node) for: OpenCode, Goose, Kimi, Cursor (`.cmd` launcher!), Junie, Devin, Poolside, Mistral Vibe, Stakpak, Kilo, VT Code, Cortex, Kimchi, Harn, Amp, Crow. |
| Cursor | Registry Windows entry is `dist-package\cursor-agent.cmd acp` => needs the same `.cmd` handling. Docs do not mention Windows. |
| Goose / OpenCode / Kimi | native `goose.exe` / `opencode.exe` / `kimi.exe` + same argv. OpenCode docs have no Windows ACP notes (WSL page separate). |
| Copilot | winget/npm. `--acp` stdio uses stdout exclusively; TCP mode alternative avoids stdio shim issues. |
| Claude adapter | Needs Claude Code credentials already in `%USERPROFILE%\.claude`; SDK bundles its own claude binary (**inf**), no separate CLI install required. |
| Antigravity | `.exe` registry entry exists, win-x64 and arm64. |
| General | Pass `cwd` as Windows absolute path (`C:\...`); spec requires absolute. Agents emit paths in their own format in `tool_call.locations`, so normalise `/` vs `\` when mapping to fs. Set stdin/stdout to binary UTF-8, no CRLF translation. |

## 5. Recommendations for Kelpie (ranked)

1. **Drive launch from a table** `{id, resolveCmd, argv, env, minVersionProbe}`; seed from the registry JSON (`https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json`) fields `distribution.npx|uvx|binary` rather than hardcoding; hourly updates, pinned versions in it. Use it for the user's "install/detect" UX; local overrides win.
2. Always send `protocolVersion: 1`; capabilities: `fs` + `terminal` only if Kelpie implements them (otherwise agents run their own tools: Gemini/Copilot/OpenCode fall back to local FS), `auth.terminal: true` (needed to get Claude login methods) and the legacy `_meta["terminal-auth"]: true`, `elicitation` only if implemented, `session.configOptions`/`notices`/`compaction` only if rendered.
3. Gate everything on returned capabilities: `loadSession` -> `session/load`; else `sessionCapabilities.resume`; else start new. Treat history replay as a flood of `session/update` before the load response (do not treat as new activity/notifications). Gemini, Qwen, Kimi, Goose, OpenCode, Claude, Codex all advertise `loadSession: true`; Cursor/Auggie/Copilot verify at runtime.
4. Auth handler: (a) none needed -> go; (b) `session/new` returns `auth_required` -> show methods; `type: agent` -> `authenticate`; `type: terminal` or `_meta["terminal-auth"]` -> run interactive terminal w/ same command + args (Kelpie PTY), wait exit 0, restart agent, re-`initialize`. Quirks: Qwen puts type in `_meta`; Kimi has terminal data only in `_meta`; Goose cannot be authed via ACP at all; Cursor needs `agent login` first (or API key env).
5. Model/mode UI: use `configOptions` (categories `mode`, `model`, `thought_level`) first; fallback `session/set_mode` / `session/set_model` (unstable name `unstable_setSessionModel`; still on the wire as `session/set_model` for Gemini/Qwen/OpenCode/Kimi).
6. Permission replies: echo agent-supplied `optionId` (Cursor uses dashed ids).
7. Rank by integration risk: lowest = OpenCode, Goose, Kimi, Codex, Claude adapter, Gemini (open source, read); medium = Copilot, Qwen; highest = Cursor, Auggie (closed, thin docs, custom ext), Factory/Grok (nonstandard argv).

## 6. Limitations / unresolved

- Cursor, Auggie, Copilot are closed source: loadSession/promptCapabilities not verified; run `initialize` smoke test per agent on the actual Windows box (a tiny harness that logs `initialize` results would settle it).
- Min versions: no vendor states the first release with the ACP flag. Gemini `--acp` introduction version unknown; Qwen's older flag name unverified; Copilot ACP preview began 2026-01-28 (changelog), Goose `acp` subcommand version not checked.
- Kiro CLI, Junie, Cline, Mistral Vibe, OpenHands, Hermes, etc. only launch argv from registry, not capabilities.
- Windows .cmd/process-tree behaviour is my inference, not tested.
- Gemini docs say the CLI was replaced by Antigravity for some paid tiers (2026-06-18); check the user's tier before recommending `gemini`.
- Goose `ProtocolVersion::LATEST` value at runtime not confirmed (SDK crate may report 1; verify in handshake).
- Whether `session/set_model` will be re-added upstream is unknown; schema v2 alpha drops `session/load`, `set_mode`, fs, terminal methods (v2 meta lists only: initialize, auth/login, session/new, set_config_option, prompt, cancel, list, delete, resume, close, auth/logout), which signals a coming client rewrite if v2 ships.
