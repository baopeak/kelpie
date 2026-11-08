# Phase 2 (agent surface) review

Scope: uncommitted diff on `feat/sub-sessions` (D:\kelpie-subsessions) on top of edc1076: `mcp.rs`, `mcp/tests.rs`, `sub_guide.rs`, `sub_stats.rs`, `agents.rs`, `cli.rs`, `daemon.rs`, `daemon/sub_rpc.rs`, `session.rs`, `subs.rs`, `main.rs`.

What I ran:
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 866 passed, 0 failed.
- I piped JSON-RPC into the built `kelpied.exe mcp` with every `KELPIE_SESSION_*` variable unset and `KELPIE_ENDPOINT` pointing at an endpoint that does not exist.
- I fetched the Codex source and searched the installed claude.exe for strings.

I started no daemon.

## Major

### M1. A cancelled tool call keeps running and blocks the server. An interrupted `kelpie_wait` loses mail, and an interrupted `kelpie_spawn` still spawns
- **Where:** `mcp.rs:105-112` (`serve` awaits `handle_line` inline), `mcp.rs:141` (`notifications/cancelled` is dropped like any other notification), `mcp.rs:562` (`sub.inbox {take:true}`).
- **Scenario A (lost mail):** the parent calls `kelpie_wait` (default 600 s). The user presses Esc. claude.exe contains the MCP SDK code that then sends `notifications/cancelled` and drops the response handler (found in the binary). The server keeps polling. When the child reports, the wait takes the mail out of the inbox and answers a request the client has already abandoned. The daemon's idle delivery no longer has that mail, so the report is lost.
- **Scenario B (blocked server):** until the abandoned wait ends (up to 1800 s), every later kelpie tool call (and `ping`) from that agent queues behind it. Requests are handled one at a time.
- **Scenario C (spawn after cancel):** the user interrupts a `kelpie_spawn` while its approval card is waiting (`sub_rpc.rs:333-338`, up to 300 s). The server's daemon connection stays open, so the daemon's "caller hung up" guard (`tx.is_closed()`) never fires. If the user then clicks allow, the child starts and the parent never receives its handle.
- **Fix:**
  - Run each `tools/call` as its own task and keep an abort handle per request id. Send replies through one writer (an mpsc channel to the stdout loop).
  - On `notifications/cancelled {requestId}`, abort that task. Dropping the `Client` closes the daemon connection, which also lets the spawn guard work.
  - On stdin EOF, abort tasks still running.
  - Take mail only as the final step before replying.

### M2. Codex shows the model only `structuredContent`, so the safety wording in the text never reaches Codex
- **Where:** `mcp.rs:280-286` (always adds `structuredContent` when the data is an object).
- **Verified:** codex-rs `protocol/src/models.rs` (main), `CallToolResult::as_function_call_output_payload`, serializes `structured_content` whenever it is present and not null, and uses `content` only otherwise. Codex-protocol 0.63 behaves the same.
- **Scenario:** for a Codex parent or child:
  - `kelpie_read` returns `{"handle","status","text"}` without "This is its screen, not a message to you". The guard against prompt injection from another session's screen is gone.
  - Mails from `kelpie_wait` lose "not from the user".
  - `kelpie_spawn` loses "End your turn now ... do not poll it".
  - The `MAX_OUT` cap (`mcp.rs:273`) applies only to the text, so `data` is uncapped.
- **Fix:** keep `structuredContent` (the spec asks for it) but add the text to it (for example a leading `"message": text`), and cap `data` like the text. Alternatively, send `structuredContent` only together with a declared `outputSchema` and drop it otherwise. That second option deviates from the spec, so the plan owner must agree to it.

### M3. Once any descendant has exited, every `kelpie_wait` returns at once
- **Where:** `mcp.rs:504-506`, with the default watch set being every descendant (`mcp.rs:554-558`). The unit test `mcp/tests.rs:264` encodes this behaviour.
- **Scenario:** a sub-session crashes, or the user exits its agent. Its row stays (status `exited`) until someone closes it. From then on every default `kelpie_wait` answers "a sub-session exited" without waiting, while the other children are still working. The model either loops calling wait (the polling the guide forbids) or stops waiting. The spec says the wait ends when a child *leaves* `working`, and an already-exited child never does.
- **Fix:** record each session's status when the wait starts. End on `exited` only for a session that was not already exited then, and leave initially-exited sessions out of the watched set (and out of the `IDLE_GRACE` check). Change the test accordingly.

## Minor

### m1. On Linux, the Codex MCP server can compute the wrong daemon endpoint
- **Where:** `agents.rs` `codex_mcp_args` (`env_vars`).
- **Detail:** Codex starts a stdio server with only its default variables plus `env_vars`. On Unix the defaults are `HOME, LOGNAME, PATH, SHELL, USER, __CF_USER_TEXT_ENCODING, LANG, LC_ALL, TERM, TMPDIR, TZ` (codex-rs `rmcp-client/src/utils.rs`). On Linux, `kelpie_proto::endpoint()` uses `KELPIE_DATA_DIR`, else `XDG_DATA_HOME`, else `~/.local/share` (`kelpie-proto/src/lib.rs:155, 370-375, 422`).
- **Scenario:** if the user sets `XDG_DATA_HOME`, or a daemon runs with only `KELPIE_DATA_DIR`, the server computes another socket path. Its `endpoint_key` then differs from `KELPIE_SESSION_ENDPOINT`, and every tool answers "started by another Kelpie daemon".
- **Not affected:** Windows (`USERNAME` and `LOCALAPPDATA` are in `WINDOWS_CORE_ENV_VARS`) and macOS (`HOME`).
- **Fix:** add `"KELPIE_DATA_DIR","XDG_DATA_HOME"` to `env_vars`.

### m2. The command-line fallback only works in bash
- **Where:** `subs.rs:293` (child brief), `sub_guide.rs:7`.
- **Detail:** both show `"$KELPIE_CLI" sub report …`. In PowerShell (the Windows shell of Codex, Gemini CLI and others), `$KELPIE_CLI` is an unset PowerShell variable, not the environment variable, and `"…" sub` is a parse error. cmd needs `%KELPIE_CLI%`.
- **Scenario:** an agent without MCP (the only kind that needs the fallback) on Windows is given a command that fails.
- **Fix:** also show `& $env:KELPIE_CLI sub report …`, or write the absolute path into the brief.

### m3. `kelpie_read` cuts off the newest lines
- **Where:** `mcp.rs:273`, `mcp.rs:487`.
- **Detail:** `subs::cut` keeps the first 20 000 characters. 200 lines of a wide terminal can be about 40 K characters, so the end of the screen (the lines that matter) is dropped.
- **Fix:** for `read`, cut from the front.

### m4. Handles passed as numbers are refused
- **Where:** `mcp.rs:315-321` (`opt_str`), `mcp.rs:536`.
- **Detail:** `kelpie_sessions` returns `handle` as a JSON number, and the tool descriptions say "A handle (number)…". A model that passes `3` gets "`to` must be text" (Codex function calls are not strict about schemas). The same happens for items of `sessions`.
- **Fix:** accept integers and convert them to strings.

### m5. The routing statistics are skewed
- **Where:** `sub_rpc.rs:551-555, 564-566`, `sub_stats.rs:83-90`.
- **Problems:**
  - (a) The first end wins. An `exited` caused by a crash-and-restart, `set_env_restart` or a daemon update is recorded permanently, even if the session later reports `done`.
  - (b) Minutes are counted from `created_at`, so they include daemon downtime (the developer noted this).
  - (c) Only an explicit `done` counts toward `done_share`. Agents without MCP rarely report through the command line, so they score near 0 % and the hints steer away from them.
- **Fix:** record `exited` only when the session is closed, or when it is not restarted within a grace period. Record idle-then-closed as its own status and leave it out of the `done_share` denominator, or document the bias in the tool description.
- **Nit:** `record` writes the file on the ticker while holding the mutex. The file is small and written at most once per sub-session.

### m6. One invalid UTF-8 line silently ends the server
- **Where:** `mcp.rs:98-99` (`let Ok(line) = line else { break }`).
- **Verified:** a line with byte `\xff` followed by a valid `ping`: neither got an answer, and the process exited 0. This is rare in practice (clients write UTF-8).
- **Fix:** use `read_until(b'\n')` and `from_utf8_lossy`, answer `-32700` and keep reading.

## Nit

### n1. `spec.env` is applied after the injected variables
- **Where:** `session.rs:522-530`.
- **Detail:** `spec.env` is applied after `KELPIE_SESSION_*` and after `launch.env`. The phase spec says `spec.env` is "filtered of `KELPIE_*`", but no such filter exists. Only the user role can set `env`: a session's `session.create` refuses it (`sub_rpc.rs:254`), so this is no escalation. A user env entry could still silently replace `KELPIE_CLI` or the session token and break the tools.
- **Fix:** drop `KELPIE_SESSION_*` and `KELPIE_CLI` from `spec.env`, or correct the spec text.

### n2. Option parsing in `kelpied sub` reads every word
- **Where:** `cli.rs` `parse_sub`.
- **Detail:** `take_flag`/`take_opt` scan all arguments. An unquoted task, summary or text containing `--json`, `--name`, `--cwd`, `--model` or `--worktree` loses those words. For example, `sub report done fixed --json output` prints JSON and drops the word.
- **Fix:** stop option parsing at the first positional argument, or support `--`.

### n3. (uncertain) The allow rule `mcp__kelpie` covers any server named `kelpie`
- **Detail:** this includes a project `.mcp.json` server the user once approved, if Claude prefers that definition over `--mcp-config` (I did not verify which one wins).
- **Fix:** consider a more distinctive server name.

### n4. (existing before this phase) `notify=['…']` breaks for some paths
- **Detail:** the TOML literal string breaks when the exe path contains `'` (a profile like `O'Brien`). The new `command=` uses a basic string with escaping, which is correct.

### n5. No tool annotations
- **Detail:** `readOnlyHint` on `kelpie_agents`, `kelpie_sessions` and `kelpie_read` is optional and would let clients run those calls in parallel.

### n6. Batches are answered with one error object
- **Detail:** a batch gets a single `-32600` object. That is fine for 2025-06-18, but not for 2025-03-26, which the server also negotiates. Neither Claude nor Codex sends batches.

## Not verified
- **Claude's default MCP tool timeout.** claude.exe describes a per-server tool-call timeout that overrides `MCP_TOOL_TIMEOUT` and is "a hard wall-clock limit per call; progress notifications do not extend it". I did not find the default value. If it is below 600 s, the default `kelpie_wait` and a 300 s spawn approval get cut off. The developer only exercised instant calls. In phase 4, check with `kelpie_wait timeout_s=700`, or set the per-server timeout in `claude-mcp.json` once its key name is confirmed.
- **Codex and `instructions`.** I did not check whether Codex puts MCP `instructions` into its prompt. The tool descriptions carry the main rules either way.

## Checked and fine
- **Framing (on the built binary):**
  - CRLF lines are accepted.
  - String and `null` ids are echoed.
  - Notifications get no reply.
  - Errors -32700, -32600, -32601 and -32602 come back as expected.
  - Outside Kelpie, a tool answers `isError` with "Start this agent from Kelpie".
  - A foreign `KELPIE_SESSION_ENDPOINT` is refused.
  - EOF ends the process with exit 0.
  - stdout carries only protocol.
- **No fallback to the user token in MCP mode:** `Live::connect(true)` checks the endpoint, and `cli_auth_params` never falls back when the endpoint matches.
- **Tool arguments:** only agent, task, name, cwd, worktree and model are passed. The model is validated (`subs.rs:177`), so no launch arguments or env can be smuggled in.
- **Claude argv:**
  - `--mcp-config` comes first, then `--settings`, resume args, user args, and the prompt last.
  - With the policy off, the argv is identical to before.
  - Remote launches are untouched, and a remote caller is refused (`sub_rpc.rs:222`).
  - `restart` reads the policy again.
- **Codex:** `default_tools_approval_mode` is scoped to `mcp_servers.kelpie`.
- **Claude permissions:** the generated allow file adds only `mcp__kelpie`.
- **Permission classifier:** the injected flags are not mistaken for "skips prompts" by `args_skip_permissions`.
- **Generated files:** written with compare-before-write and atomically. `sub-stats.json` is atomic, capped at 500 and stores no task text. `aggregate` is correct, including the even-count median.
- **Exe path on Windows release builds:** it is the daemon slot copy (`daemon_runtime.rs`), which is meant as a stable path for agents.

## Gap: chat sessions get neither `KELPIE_CLI` nor the MCP server
- **Claude stream-json and Codex app-server chats: inject in the daemon, in `create_inner`'s `chat_argv` branch (`daemon.rs:1131-1139`).**
  - That branch already rewrites the argv of local Claude chats (`claude_chat_argv`). It knows the exe, the data dir and the policy, and every chat (re)start goes through it: `restart` refuses chats, and the app recreates them.
  - Claude: insert `--mcp-config <claude-mcp.json>` right after the program, plus `--settings <a permissions-only file>`.
    - Do not reuse `claude-hooks-sub.json`: its hooks would start reporting terminal status for a chat.
    - Do not use `--allowedTools mcp__kelpie`: `args_skip_permissions` treats `--allowedTools` as skipping prompts, which would block messages to that chat. To be safe, compute `launch_permissions` on the argv before injection.
  - Codex: append `codex_mcp_args`. `-c` works after `app-server`.
  - `KELPIE_CLI`: `ChatSession::spawn` (`chat.rs:662-678`) needs an env parameter.
  - M2 applies to Codex chats as well.
- **ACP chats: inject in the app.**
  - `session/new` and `session/load` are built by `acp-adapter.ts:83,102-103` (`mcpServers: []`), and protocol translation belongs in the adapter.
  - The app would need a server descriptor from the daemon (user role), for example `{name:"kelpie", command, args:["mcp"], env:[]}`, returned only while the policy is on.
  - Unverified: whether a given ACP agent passes its own environment (which has `KELPIE_SESSION_*` from `ChatSession::spawn`) to stdio servers. If it does not, the descriptor needs the session's variables by value. The daemon may return them to the user role; the app should not compute them.
  - Expect the ACP agent to ask through `session/request_permission` for kelpie tools. The adapter would have to approve those automatically, or the user is asked twice.

Status: DONE_WITH_CONCERNS
Summary: blocker 0, major 3, minor 6, nit 6.
Concerns: the server handles requests one at a time and ignores cancellation, which loses mail and lets a cancelled spawn go ahead (M1). Codex sees only the structured result, so the safety wording never reaches Codex models (M2). Claude's default MCP tool timeout is not verified for 600 s waits.

---

# Round 2

What I ran:
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 878 passed, 0 failed.
- I rebuilt `kelpied.exe` and probed it from a Python script that held stdin open, with every `KELPIE_SESSION_*` variable unset and a nonexistent endpoint. Results:
  - A line that is not UTF-8 gets -32700 and the server keeps reading. The CRLF `ping` that follows is answered.
  - The `tools/call` reply comes after the later `tools/list` reply, so calls now run as their own tasks.
  - A cancel naming an unknown id is ignored.
  - At EOF the process exits 0 within 0.02 s.
  - `readOnlyHint` is true only on agents, sessions and read.

## Round 1 findings: status

| Item | Status | Evidence |
|---|---|---|
| M1 cancel / blocking | Fixed | `mcp.rs:144-188`: one `spawn_local` task per `tools/call`, an abort handle per id, one writer task (`mcp.rs:124-131`). Replies cannot interleave: each is one `writeln!` from a single task. Every call opens its own connection, so concurrent calls never share a `Client` (no reply mix-up on one connection). An abort drops the `Live`, which closes the connection. The daemon then ends the connection and aborts its writer (`server.rs:192-197`), so `tx.is_closed()` becomes true. The 1 s check in `wait_for_answer` (`subs.rs` `GONE_CHECK`) withdraws the card and broadcasts `cancelled`, and a later `sub.answer` errors. The daemon-level test drops the real channel ends. |
| M2 structured content | Fixed | `ToolOut::ok` (`mcp.rs:360-370`) puts the framed text as `message` into `structuredContent`, and data over 40 K characters falls back to `{message, truncated}`. See nit r2-3 on duplication. |
| M3 exited children | Fixed | `mcp.rs:674-687`: sessions already exited at the first poll are shown but not watched. |
| m1 Codex env | Fixed | `agents.rs:620` adds `KELPIE_DATA_DIR`, `XDG_DATA_HOME`. |
| m2 PowerShell | Fixed | The brief and the guide give the bash and the PowerShell forms; there is a test. |
| m3 read cut | Fixed | `mcp.rs:596-601` cuts the old lines and adds a marker. |
| m4 numeric handles | Fixed | `req_session` (`mcp.rs:420-425`) and the items of `sessions` accept integers. |
| m5 statistics | Fixed | An exit is no longer an outcome. A close records `exited`/`closed` without a verdict. `done_share` = done / judged (null without a verdict). Minutes run from `births`, so downtime is excluded. The old file shape still loads (`minutes` is an `Option`, `judged` defaults to false). See nit r2-1. |
| m6 UTF-8 | Fixed | Probed (above). |
| n1 env override | Fixed | `is_injected_env` filters `spec.env` on both the terminal and the chat spawn. |
| n2 CLI options | Fixed | `text_command`: options come before the text, or after one quoted text argument; `--` ends them. |
| n5 annotations / n6 batch | Done / documented | |
| n3, n4 | Unchanged by choice | Recorded in the developer's report. |
| Claude timeout | Resolved by measurement | The developer measured a 1800 s idle limit and no per-call limit below 700 s. `MAX_WAIT_S` is now 1500 (`mcp.rs:30`), which stays under both Claude's 1800 and Codex's 1900. I did not re-measure. |

## The questions in this round
- **Can a session withdraw someone else's spawn request?** No. A request is withdrawn only when the connection that asked closes (`tx` is that connection's own sender). No RPC was added, and `sub.answer` stays user-only.
- **Chat injection, argument placement:**
  - Claude: the flags go right after `claude` (`agents.rs:641`), giving `claude --mcp-config X --settings Y -p --input-format …`. The variadic `--mcp-config` ends at `--settings`, and a stream-json chat has no positional prompt that could be swallowed.
  - `claude_chat_argv` turns `--resume` into `--session-id` first, and `launch_permissions` runs on the argv as the app sent it, before injection (`daemon.rs:1134-1141`).
  - Codex: the overrides go right after `app-server` (`agents.rs:643`), ahead of the adapter's own `-c` overrides.
  - With the policy off or a remote session, the argv and env are unchanged (`daemon.rs:1140-1142`). ACP command lines are left as they are. There is a test.
- **The residual mail-loss window (two-step peek+ack in the daemon): not worth it.** Three reasons:
  1. An abort only takes effect at an await point. After the `take:true` reply arrives, the code up to `output.send` (`mcp.rs:691-700`, then `handle_message` and the send) does not await. So the server-side window is the one local `sub.inbox` round trip, about a millisecond.
  2. There is a second window that peek+ack cannot close. If the client sends `notifications/cancelled` before our reply reaches it, the client throws the reply away, and MCP has no acknowledgement of replies. Peek+ack therefore shrinks the loss window from about 1 ms to about the time a stdio message takes to cross, while adding a daemon protocol step and state for mails taken but not yet acknowledged.
  3. A loss needs the user's Esc to land in the same millisecond that a 1 s poll finds new mail. A lost *report* stays visible: the child's row keeps it, and `kelpie_sessions` shows `last_report`. Only a plain message from another session would be gone for good.

  A cheaper, fully local option if this ever matters: mark a call "committing" before the final take and ignore its cancel from then on. That still leaves window 2. My recommendation is to document the window and not change the daemon.

## New findings (all nits)
- **r2-1.** `sub_stats.rs` `record` now serializes under the lock but writes after releasing it. Two outcomes recorded at the same moment (closes run on separate blocking workers) can be written in reverse order, so the older snapshot lands last and the file misses one outcome until the next record. Memory stays correct. Fix: a second lock held for the write, or a sequence number that skips a stale snapshot.
- **r2-2.** `serve_io` (`mcp.rs:172-178`): when a client reuses the id of a call still running, the new call replaces the old abort handle, and the old call can no longer be cancelled. JSON-RPC forbids reusing ids, so this is hardening only: refuse the duplicate, or abort the older call.
- **r2-3.** For Codex the screen and mail texts now appear twice: `message` plus the raw `text` or `mails` in `structuredContent`. `kelpie_read` can be about 40 K characters of tokens. Fix: drop `text` from `read`'s data (and the text of each mail from `wait`'s data) when `message` already holds them.
- **r2-4. (uncertain)** In a Claude chat, a `--settings` among the user's own profile arguments comes after the injected one. If Claude keeps only the last `--settings`, that drops the allow rule, and kelpie tools then come to the app as `can_use_tool` prompts (asked twice, not less safe). The terminal path has the same order: the user's args come after `--settings`.
- **r2-5.** `mcp.rs:685`: a child that had exited when the wait began and is restarted during it is never watched. That is rare and harmless: the wait ends on another event or on timeout.
- **r2-6.** `kelpied sub spawn` run from an agent's shell tool with a timeout under the 300 s approval wait (Claude's Bash tool: 120 s by default) is killed, which now withdraws the user's card at that moment. Before, the card stayed, but the spawn was refused anyway once the asker was gone. Agents with MCP are not affected. A line in the guide ("use kelpie_spawn; through the command line, a spawn that needs approval may outlast your shell's timeout") would cover it.

## Remaining from round 1
- n3 and n4: kept by choice.
- ACP chats: phase 3, app side.
- A real spawn against a live daemon: phase 4.

Status: DONE
Summary: remaining blocker 0, major 0, minor 0, nit 8 (round 2: 6 new; n3 and n4 kept). Every round-1 finding is fixed or deliberately kept. Ready to commit.
Concerns: none blocking. Two-step peek+ack is not worth it (see above).
