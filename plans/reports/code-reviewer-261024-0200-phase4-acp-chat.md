# Code review: roadmap phase 4 "Universal ACP chat"

Date: 2026-10-24. Branch `rebrand/kelpie`, uncommitted on top of `1b9da5a`.
Spec: `plans/261023-0048-multi-agent-ide-roadmap/phase-04-universal-acp-chat.md`. Interop baseline: `plans/reports/researcher-261024-0110-acp-agents-launch.md`.
Protocol checked against ACP `schema/v1/schema.json` (main, 2026-10-24), `protocol/v1/elicitation.md`, and the TS SDK `src/v1/schema/zod.gen.ts`.

## Scope
- Phase-4 hunks only: `app/src/chat/acp-adapter.ts` (header, legacy/auth/elicitation state, applyLegacy, askSignIn, initialize params, auth_required branch, version check, `kelpie-auth-*`, elicitation handler, respond/setSetting branches), `app/src/chat/mode-switch.ts`, `app/src/lib/agents.ts`, `app/src/lib/plugins.ts`, `app/src/chat/acp-protocol.test.ts`, `crates/kelpied/src/{chat,agents,daemon}.rs`, `crates/kelpie-proto/src/lib.rs`, `docs/{plugins,architecture}.md`, `scripts/e2e/fake_acp_agent.py`.
- The concurrent `/new` / `/clear` hunks are reviewed only where they touch phase 4.
- None of the third-party ACP CLIs is installed on this machine (only `omp`), so per-agent interop is judged against the spec and the research, not a live handshake.

## Checks run
| Check | Result |
|---|---|
| `cargo test -p kelpied` | pass (incl. `acp_updates_mean_work_only_for_the_turns_content`, `acp_titles_name_the_session`) |
| `pnpm typecheck` | pass |
| `pnpm test` | pass, 12 files / 118 tests |
| `pnpm i18n:check` | **FAIL**: 10 missing Vietnamese strings, all in `acp-adapter.ts` (122, 127, 128, 131, 245, 285, 293, 297 x2, 365) |
| Handshake piped into `python scripts/e2e/fake_acp_agent.py` | **crashes** on the first `session/new` (see C1) |

## Verdict: NOT READY TO SHIP
The protocol shapes are mostly right. Field names, result shapes and error codes match ACP v1. The blocking problems are a broken test agent, a daemon status bug that shows every resumed ACP chat as Working, Stop not reaching open elicitations, and a chat-to-terminal hand-off that phase 4 newly exposes to 8 CLIs with an unverified `--resume <id>` form. Fix C1, H1 to H4, M1 and M8 before merge. The rest can follow.

---

## Critical

### C1. `fake_acp_agent.py` calls an undefined `save()`, so every session crashes
`scripts/e2e/fake_acp_agent.py:76,136,141,149` call `save()`. No `def save` exists in the file. Live output:
```
File "scripts\e2e\fake_acp_agent.py", line 136, in <module>
    save()
NameError: name 'save' is not defined
```
`initialize` answers and then the agent dies on `session/new`. No e2e that depends on it can pass, including the `--state FILE` resume scenario.
Fix: add the function the `--state` docstring describes.
```python
def save():
    if STATE:
        with open(STATE, "w", encoding="utf-8") as f:
            json.dump(sessions, f)
```
(The tester's `acp_chat.py` run log shows `{"passed": 6, "failed": 3}`, failing earlier on a `.launch-card` selector. It may never have reached this.)

## High

### H1. Resuming via `session/load` reports the chat as Working for about 2 minutes, then marks it unread
- `chat.rs:459-463` classifies `agent_message_chunk` as Working. ACP agents replay the whole history as `agent_message_chunk` / `user_message_chunk` before answering `session/load` (research §1; the fake agent does the same at lines 142-143).
- The load answer `{"id":"kelpie-session","result":{...}}` has no `stopReason`, so `classify` returns `None` (`chat.rs:471`).
- The Working hook then sticks. `status.rs` `Activity::status` only lets output override a Working hook after `HOOK_STALE` (120 s), and only if more output arrived.
- When it finally drops to Idle, `tick` (`chat.rs:387-391`) sets `unread` for every unfocused pane, because `working_since` is more than 3 s back.
- Who hits it: every ACP chat resumed after a daemon restart or hibernation (`resumeChat`), every History "Open in chat view", and every terminal-to-chat switch with a known conversation. This breaks the success criterion "status returns to Idle".

Fix (minimal): treat the session answer as the end of activity.
```rust
// classify(), before the stopReason check
if v.get("result").is_some()
    && v.get("id").and_then(Value::as_str).is_some_and(|id| id.starts_with("kelpie-session")) {
    return Some(Status::Idle);
}
```
Better: in `ChatSession::input` (chat.rs:296), note a client `"method":"session/load"` and suppress Working until the `kelpie-session` answer. Long replays then never count as work toward unread. Add a Rust test that runs a load replay through `Activity` and asserts Idle.

### H2. Stop (`interrupt`) leaves open elicitations unanswered
- `acp-adapter.ts:329-341` answers only `requests` (permissions) with `cancelled`. `elicitations` (line 52) is untouched.
- An agent awaiting `elicitation/create` while it handles `session/cancel` may never send `stopReason: cancelled`. In SDK-based agents the client request promise stays pending.
- Result: `busy` stays true and the card stays live, so Stop looks broken. The daemon status also stays Waiting (`chat.rs:454`).

Fix:
```ts
for (const [id, e] of elicitations) {
  ctx.write({ jsonrpc: "2.0", id: e.rid, result: { action: "cancel" } });
  ctx.patch(id, (i: any) => { if (i.kind === "permission" && !i.resolved) i.resolved = t("Cancelled"); });
}
elicitations.clear();
```
Also clear `elicitations` in `reset()` (line 204).

### H3. Chat → terminal hand-off sends the ACP session id as `--resume <id>` to every ACP CLI
- Phase 4 makes `supportsChat` true for 8 more kinds. The sidebar therefore offers "Continue in terminal (TUI)" for them (`sidebar.tsx:133`).
- `doSwitchToTerminal` (`mode-switch.ts:215,232`) passes `ctrl.state.conversationId` (the ACP `sessionId`) as `agent_session_id`.
- `resume_args` (`agents.rs:425`) then appends `--resume <id>` for every kind except codex.
- Known CLI forms: Gemini, Copilot and Cursor accept `--resume <id>`. OpenCode uses `--session/-s` and Kimi `--session/-S`; to my knowledge neither has `--resume`. Goose's `session --resume` is a boolean that takes `--session-id`/`--name`. Qwen and Auggie are unverified.
- It is also unverified for most agents that the ACP `sessionId` equals the CLI conversation id.
- Effect: the chat process is stopped first, then the TUI fails to start or resumes the wrong conversation.

Fix: add a per-preset resume-by-id form (e.g. `resume_id: &["--session", "{id}"]`). Until it is verified, pass `agent_session_id` only for kinds known to accept it, and use `Resume::Latest` (`resume_latest`) otherwise.

The reverse direction is lossy too. `doSwitchToChat` (`mode-switch.ts:166-169`) looks up the conversation only for claude/omp. Switching a Gemini/OpenCode TUI to chat kills it and starts a new conversation every time. The toast says so, but users lose continuity. Options: `session/list` when `sessionCapabilities.list` is advertised, or don't offer the switch for those kinds.

### H4. `initialize.clientInfo` lacks the required `version`
`acp-adapter.ts:218` sends `clientInfo: { name: "kelpie", title: "Kelpie" }`. The schema's `Implementation` has `"required": ["name", "version"]`.
- The current TS SDK wraps `clientInfo` in `defaultOnError`, so it would just drop the field.
- Agents on older SDKs, or a strict Python/pydantic or Rust/serde deserializer (Kimi, Goose), can reject `initialize` with -32602, and then the agent cannot be used at all.

Fix (one line): `clientInfo: { name: "kelpie", title: "Kelpie", version: APP_VERSION }`, using the app's version (e.g. from Tauri `getVersion()` or the build constant).

## Medium

### M1. A failed `authenticate` leaves no way to retry
- `respond` deletes the auth card from `requests` (line 371) and `ChatController.respond` marks it resolved.
- If `authenticate` errors, the error branch (`acp-adapter.ts:225-240`) only prints "Agent: …". `needsAuth` is false for `kelpie-auth-*`.
- Real cases from the research:
  - Goose `goose-provider`: cannot be completed over ACP.
  - Gemini `gemini-api-key`: needs `_meta["api-key"]`.
  - Cursor `cursor_login`: fails before `agent login` has been run.
- The user is stuck until they restart the chat.

Fix: in the error branch:
```ts
if (String(m.id).startsWith("kelpie-auth-") && !ctx.replaying()) {
  ctx.upsert({ kind: "system", id: `e-${String(m.id)}`, text: t("Agent: {error}", { error: String(m.error.message ?? "") }), tone: "error" });
  askSignIn(authMethods);
  return;
}
```

### M2. `auth.terminal: true` is advertised, but Kelpie cannot run the terminal method
- Schema `AuthCapabilities.terminal`: "set this to `true` only when it can reproduce the configured agent invocation in an interactive terminal."
- The adapter only prints the command (`askSignIn`, lines 118-128). For standard `type: "terminal"` methods it has no command and prints the placeholder `t("the agent's command") + args`. OpenCode, for example, would show "the agent's command --login".
- It also drops `env`.
- Kimi (`_meta["terminal-auth"].command`) and Copilot do get a real command. Qwen (`_meta.type`) gets the placeholder.

Fix:
1. Give the adapter the launch argv (add `argv` to `AdapterContext`, from `chatArgv`).
2. Display `[bin, ...args]`.
3. Add a "Run in a terminal" option that reuses the existing hand-off (`chat-pane.tsx:177` `openInTerminal`, or a terminal session with `env`).
4. Re-initialize when it exits with code 0.

### M3. Cursor's blocking extension methods and Copilot's interactive-only commands
- The `cursor/ask_question` and `cursor/create_plan` requests fall through to `acp-adapter.ts:315-318` and get -32601. That does unblock Cursor, which is good.
- But the user never sees the question or the plan, and Cursor may fail the tool or give up silently.
- At minimum, upsert a system line naming the method and `params` summary so the user knows. Better: map `cursor/ask_question` onto the existing `ask` card once its params shape is verified.
- Copilot lists interactive-only commands (`/diff`, `/login`) in `available_commands_update`. They are offered unfiltered (lines 190-195) and silently do nothing. Either filter them (verify the list against a live Copilot first) or attach the "Run it in the terminal" `tui` hand-off the Claude adapter already uses.

### M4. Any -32000 error on `session/load` becomes a sign-in card instead of the fallback to `session/new`
- The `needsAuth` check (line 227) runs before the load fallback (lines 236-239).
- -32000 is the ACP `auth_required` code, but it is also the generic "server error" that several JSON-RPC stacks use.
- The `/auth/i` message test is broad too: "OAuth token for MCP server … failed" matches it.
- Fix: for a failed load (`m.id === SESSION_ID && ctx.state.conversationId`), fall back to `session/new` first. A real auth problem will then show up as `auth_required` on the new session. Alternatively require `m.error.data?.authMethods` or `/auth(entication)? required/i` there.

### M5. Remote sessions: "Continue in chat view" is not blocked (existing gap, now wider)
- `pane-view.tsx:76` and `new-session-menu.tsx:75,136` exclude remote sessions, but `sidebar.tsx:133` (`s.alive && supportsChat(s.kind)`) does not, and neither does `doSwitchToChat`.
- For a remote session, the daemon spawns the local CLI. `chat.rs:133-135` skips `current_dir` because the remote path is not a local directory, so the agent runs in the daemon's cwd and gets `session/new { cwd: <remote path> }`. That points a local agent with tool access at the wrong tree.
- This existed for claude/codex/omp. Phase 4 extends it to 8 more agents.
- Fix: add `&& !s.remote` at `sidebar.tsx:133`, and guard `doSwitchToChat` / `newChat` on `project.remote`.

### M6. The status classifier has no notion of a turn
- **No Working before the first chunk.** Working starts only at the first chunk. An agent that thinks silently (no thought chunks) shows Idle for the whole wait.
- **Late updates stick as Working.** A `tool_call_update` or `agent_message_chunk` after `stopReason` (cancel races) sets Working, and with no later output that never clears (`status.rs` HOOK_STALE needs output after the hook).
- **Missing `cancelled` hangs.** Agents that never send `stopReason: cancelled` stay Working forever. The adapter's `busy` stays true as well.
- **Fix:** in `ChatSession::input`, mark the turn open on a client `"method":"session/prompt"` (Working) and closed on `stopReason`. Count updates as Working only inside an open turn. Optionally fall back to Idle N seconds after a client `session/cancel` with no output.
- **Missing test:** the spec's "trailing `usage_update` after `stopReason` keeps Idle" test exists only per line (`chat.rs:500-512`). No test runs the sequence through `Activity`.

### M7. Gaps in elicitation form handling (`acp-adapter.ts:280-302, 354-369`, `chat-rows.tsx:186-227`)
- **No Accept on a field-less form.** A form with no fields (a valid "Proceed?" request) renders no Submit button, because `p().ask?.length` is 0. It can only be declined. Add `{ id: "answer", label: t("Accept"), tone: "allow" }` when `fields` is empty.
- **Optional fields are forced.** `required` is ignored and `complete()` (`chat-rows.tsx:194`) demands every field. Pass the required set and only gate on it.
- **Multi-select returns the wrong type.** `type: "array"` (multi-select) shows no options (they live in `items.enum` / `items.anyOf`) and returns a string. ACP expects `string[]`.
- **Numbers are not validated.** `Number("abc")` is NaN and serialises as `null`. Validate it, or keep the card open.
- **Duplicate titles collide.** Answers are keyed by question text, so two fields with the same `title` get the same answer. Disambiguate the question string with the key.
- **Smaller deviations:** schema `default` values are not prefilled (the spec says SHOULD), and `oneOf` options show `const` instead of `title`.
- **URL mode is not rejected per spec.** An unadvertised `url` mode should get JSON-RPC -32602 per the elicitation doc, but receives `{action:"decline"}`. Agents must handle decline anyway, so this is low impact.

### M8. The i18n gate fails
`pnpm i18n:check` exits 1 with the 10 strings listed above. Add them to `app/src/locales/vi/*.json`. Note that the boolean answer is compared by translated label (`v === t("Yes")`, line 365). That works only while the language does not change between rendering the card and answering it. Carrying a value next to the label would be safer.

## Low
1. **Elicitation substring match.** `chat.rs:454` `method.contains("elicitation")` also matches the agent's `elicitation/complete` notification and would set Waiting. Use `method == "elicitation/create"`.
2. **Title handling** (`chat.rs:216-221, 419-431`):
   - `title: null`, which the schema defines as "clear", is ignored.
   - The title is not reset on `/new`.
   - It is lost on every daemon restart or resume: `restore` builds `ChatSession::new` with `title: None` (`daemon.rs:554`), `resumeChat` creates a fresh session, and agents may not re-send `session_info_update` on load.
   - Control and bidi characters are not stripped.
   - `sessionLabel` (`store.ts:1479`) ignores titles of 48 characters or more, so the 120-character cap is mostly invisible.
   - Fix: strip `char::is_control`, persist the title with the saved session (or pass it on resume), and truncate with an ellipsis in the UI instead of dropping it.
   - Injection into the UI: none. Solid renders the title as text, and system lines with URLs are text too (`chat-pane.tsx:244-245`).
3. **Cursor bin fallback.** `acpArgv` (`agents.ts:36-41`) falls back to `p?.bin ?? kind`. For `cursor` that gives `cursor`, the Cursor IDE shim on Windows, when presets are not loaded yet (e.g. `resumeChat` at startup racing `agent.presets`). Store the bin in the table (`cursor: { bin: "cursor-agent", args: ["acp"] }`), or return null when the preset is unknown and the kind is not the bin.
4. **Fragile auth sentinel.** The auth card is stored in `requests` with the sentinel value `"auth"` (line 134). `interrupt()` would write `{id:"auth", result:{outcome:cancelled}}` to the agent if it ever ran while an auth card is open. Today it cannot, because it returns early with no `sessionId`. Keep auth cards in their own set. `reset()` also does not clear `legacy`, `authMethods` or `modeOption`.
5. **Legacy modes/models not refreshed.** `applyLegacy` (lines 101, 107) skips when the setting already exists, so after `/new` the new session's `currentModeId` / `currentModelId` are not applied.
6. **No `session/close`.** The spec lists it, but it is never sent (on `/new` or close). Agents with `sessionCapabilities.close` keep the old session in memory until the process dies.
7. **Version mismatch leaves the agent running.** On a protocol version mismatch (line 244) the agent process keeps running and queued prompts keep `busy` true. Close stdin or mark the chat as exited.
8. **Older daemons (API 7).** The contract holds: `AgentPreset.acp?` is optional, the old `AgentDef` has no `deny_unknown_fields` so the extra `acp` key is ignored, and built-in agents still chat. But the old classifier counts the trailing `usage_update` as Working, so Gemini/Copilot chats on an old daemon show Working until the next turn. Acceptable under the contract, because the app already prompts for the daemon switch on API 8. Worth one line in docs.
9. **Profile flags dropped.** `chatArgv` ignores `profileLaunch(kind).args` for ACP kinds (only env is passed). User profile flags such as a model are silently dropped in chat. Document this or append the safe ones.
10. **Spec drift.** The spec says "unknown kinds render as text". The adapter drops unknown `sessionUpdate` kinds silently. The daemon side is correct (`None`).

## Interop matrix (from code paths, not live runs)
| Agent | Launch | Auth path | Notes |
|---|---|---|---|
| Gemini `gemini --acp` | ok (.cmd shim unwrapped by `platform_argv`, daemon.rs:600) | `oauth-personal` via authenticate ok; `gemini-api-key` fails with no retry (M1) | legacy `models` + `set_model` handled; resume to TUI ok |
| OpenCode `opencode acp` | ok | terminal method shows placeholder command (M2) | TUI resume flag wrong (H3) |
| Copilot `copilot --acp` | ok | `_meta["terminal-auth"]` shows `copilot login` | interactive-only commands listed (M3) |
| Goose `goose acp` | ok | `goose-provider` cannot be completed, no retry (M1) | TUI resume flag wrong (H3); `_goose/*` requests get -32601 |
| Qwen `qwen --acp` | ok | `_meta.type=terminal` detected, but command shows the placeholder (M2) | resume form unverified |
| Kimi `kimi acp` | ok | `auth_required` + `data.authMethods` + `_meta["terminal-auth"]` gives `kimi login`; good | strict SDK risk from missing `clientInfo.version` (H4); TUI resume flag wrong (H3) |
| Cursor `cursor-agent acp` | ok via preset bin; IDE shim risk (Low 3) | `cursor_login` via authenticate; fails before `agent login`, no retry (M1) | dashed option ids echoed (tested); blocking ext methods get -32601, user never sees them (M3) |
| Auggie `auggie --acp` | ok | terminal auth per changelog shows the placeholder (M2) | `loadSession` unverified |
| Claude via adapter | not routed (native adapter kept, per spec) | n/a | n/a |

## Verified correct (for calibration)
- `session/set_mode {sessionId, modeId}` matches `SetSessionModeRequest`. `session/set_model {sessionId, modelId}` matches the unstable form the research names. `set_config_option {sessionId, configId, value}` matches.
- The elicitation result `{action: accept|decline|cancel, content}` matches `CreateElicitationResponse` / `ElicitationAcceptAction`. The client capability `elicitation: { form: {} }` correctly advertises form only.
- `authenticate {methodId}` is right, and terminal methods are never sent to `authenticate` (filtered at line 130). `auth_required` is -32000 per `ErrorCode`. The `cancelled` / `selected` permission outcome shapes are correct, and agent option ids are echoed (Cursor dashed ids are covered by a test).
- Elicitation answers are not persisted. `ChatSession::input` (`chat.rs:296-303`) never writes client lines to the replay log, and the card label lives only in controller memory. Kelpie collects no secrets itself, and form mode can only be policed by the agent (the spec forbids secrets there).
- `chat_argv` goes through `agents::platform_argv` (`daemon.rs:600`), so npm `.cmd` shims resolve on Windows. A CLI that is not installed fails cleanly with "`x` not found on PATH" and a "Could not start … chat" toast (`mode-switch.ts:143`).
- Narrowing the classifier (`chat.rs:459-463`) to message, thought and tool-call updates is the right model, and it fixes the old "any `session/update` means Working" bug for OMP too.

## Test coverage gaps
- **acp-protocol.test.ts misses these cases:**
  - an `authenticate` error followed by a retry;
  - `auth_required` carrying `data.authMethods` (Kimi);
  - command display for `_meta["terminal-auth"]` and Qwen's `_meta.type`;
  - interrupt with an open elicitation;
  - a failed load answered with -32000;
  - -32601 for an unknown request (`cursor/ask_question`);
  - field-less, optional and multi-select forms.
- **Rust is missing** a sequence test of a load replay and of a trailing update after `stopReason` through `Activity`.
- **e2e:** the fake agent is broken (C1). Real Gemini/OpenCode runs are not possible on this box.

## Plan follow-ups (no plan edits made)
- Steps 2 (`acp` args + `supportsChat`), 3 (classifier + tests) and most of 4 (adapter, elicitation, title, resume) are present.
- Step 5 (settings UI for custom `acp`) has no diff in `agent-settings.tsx`. Today the only `agent.set_custom` caller is `plugins.ts:167`, so the plugin manifest `acp` field covers "custom agents". Confirm that is the intended scope.
- `chat-pane.tsx`, `new-session-menu.tsx` and the locales listed in the spec are untouched apart from the i18n gap.
- The perf gate (2k items at 60 fps, lazy adapter chunk) is not evidenced in this diff.
- Spike report (step 1): only the researcher's desk research exists. There is no on-box `initialize` capture per installed CLI.

## Unresolved questions
1. Does the ACP `sessionId` equal the CLI's resumable conversation id for Gemini, Copilot, Cursor, Qwen and Auggie? This decides H3's fix per agent.
2. What are the params and result shapes of `cursor/ask_question` / `cursor/create_plan`? Needed to render them instead of returning -32601.
3. Which commands does Copilot actually list in `available_commands_update` that it refuses to run over ACP?

Status: DONE_WITH_CONCERNS
Summary: Phase 4's protocol shapes match ACP v1, but the fake ACP agent crashes on `session/new`, resumed chats show as Working for ~2 minutes, Stop doesn't cancel open elicitations, and chat→terminal sends an unverified `--resume <id>` to 8 newly enabled CLIs; i18n check fails. Not ready to ship until C1, H1–H4, M1 and M8 are fixed.
