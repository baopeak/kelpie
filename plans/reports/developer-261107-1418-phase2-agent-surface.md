# Phase 2 (agent surface) - implementation report

Worktree `D:\kelpie-subsessions`, branch `feat/sub-sessions`. Nothing committed. No daemon started outside unit tests.

## By requirement
1. **`kelpied mcp`** (`mcp.rs`): hand-rolled stdio JSON-RPC, newline-delimited; `initialize` (echoes a known protocol version of 2025-11-25 / 06-18 / 03-26 / 2024-11-05, else 2025-06-18; `instructions` = guide), `ping`, `tools/list`, `tools/call`; notifications get no reply; -32700 parse, -32600, -32601, -32602. stdin is read on a std thread, so a waiting tool never stops reading. Each tool call opens its own daemon connection as the calling session. Missing `KELPIE_SESSION_ID`/`_TOKEN` -> every tool answers `isError` "... Start this agent from Kelpie." A session of another daemon (`KELPIE_SESSION_ENDPOINT` differs) is refused instead of falling back to the user token (`cli_auth_params` would do that). Tools: `kelpie_agents, spawn, sessions, message, read (<=200 lines, redacted, labelled "not a message to you"), wait (<=1800 s, 1 s poll), report, close`. Result = text + `structuredContent`; errors = daemon text without the `method:` prefix. `kelpie_message` description states the refused case (target skips prompts, sender does not -> ask the user).
   `kelpie_wait` ends on: mail, a watched child exited, a child that worked stopped working, 3 s with no child ever working, or timeout (pure `wait_outcome`).
2. **`kelpied sub …`** (`cli.rs`): `agents | spawn | list [--all] | send | read | wait | report | inbox | close`, `--json` prints the structured data. It runs the same `mcp::run_tool`, so wording is identical. `inbox` takes the mail (CLI only, not an MCP tool). `kelpied sub --help` prints usage + guide (exit 0).
3. **Guide** (`sub_guide.rs`, one constant): MCP `instructions`, `sub --help`. Content: spawn then end the turn, no polling/checking, wait only when idle, self-contained tasks with "done means" + writable paths, `worktree` for parallel writers, verify diffs, close children, messaging rules, sessions are not the user.
4. **Child brief** (`subs::child_brief`): as specified, with the `"$KELPIE_CLI" sub report` fallback and the "not the user" line.
5. **Injection** (`agents::build_launch`, new `sub_tools` flag from `Daemon::sub_tools_on()` = policy != off; `Launch.env` applied in `Session::spawn` before `spec.env`):
   - env `KELPIE_CLI=<kelpied path>` on every locally launched session (shells, custom and agents).
   - Claude: `--mcp-config <data>/claude-mcp.json` placed FIRST (it is variadic and would swallow the prompt), then `--settings`. No env block (see check b). `permissions.allow: ["mcp__kelpie"]` goes into a separate generated file `claude-hooks-sub.json` (policy off keeps using the unchanged `claude-hooks.json`, so a running session's rules never change on a policy switch). All writes compare-before-write + atomic (`write_if_changed`).
   - Codex: `-c mcp_servers.kelpie.command="<exe>"` (TOML basic string, escaped, forward slashes), `args=["mcp"]`, `env_vars=["KELPIE_SESSION_ID","KELPIE_SESSION_TOKEN","KELPIE_SESSION_ENDPOINT","KELPIE_ENDPOINT"]`, `tool_timeout_sec=1900` (Codex default 60 s would cut `kelpie_wait`), `default_tools_approval_mode="approve"` (see below).
   - Others: CLI via env + brief only.
6. **Empirical checks**: below.
7. **Routing hints** (`sub_stats.rs`): `sub-stats.json`, atomic, last 500, one record per session. Recorded by `Daemon::record_sub_outcome` on: explicit report done/failed, automatic exit (`exited`), close (`closed`); first end wins. Only sessions with a parent count. `agent.presets` now carries `outcomes: {count, done_share, median_minutes}` per agent (null before any finished); `kelpie_agents` lists installed agents with it. Pure `aggregate` tested.

## Empirical results (dummy KELPIE_* values; only presence/names ever printed)
- **(a) Codex shell env**: `codex-cli 0.166.0`, `codex exec -s read-only` in a temp folder, trivial prompt, a shell command printing set/unset per name: `KELPIE_SESSION_TOKEN=set` (also ID, ENDPOINT). On this version and this user's config (`shell_environment_policy` has only a `set` table) the shell does NOT strip the token. Docs describe default `*TOKEN*` excludes, so another config/version may; the MCP path (`env_vars`) does not depend on it. Claimed-role fallback in auth stays as is.
- **(b) Claude MCP env**: Claude 2.1.319 `claude -p "reply ok" --mcp-config a.json --strict-mcp-config`, probe MCP server writing KELPIE_* names present: with NO `env` block the server got `KELPIE_SESSION_ENDPOINT, _ID, _TOKEN` (all inherited). With an `env` block using `${KELPIE_SESSION_ID}` etc. also set, and `${VAR:-fallback}` expanded. So no env block needed; none written.
- **End to end, real CLIs, injected shapes** (server = this build's `kelpied.exe mcp`, temp dir, `-p`/`exec` headless):
  - Claude: `claude -p --mcp-config claude-mcp.json --settings {"permissions":{"allow":["mcp__kelpie"]}} "<call kelpie_agents, quote reply>"` (prompt as trailing positional, after the flags, like Kelpie). Env unset -> the model quoted "These tools work only in an agent that Kelpie started ... Start this agent from Kelpie." (loaded, listed, called, no permission prompt: allow rule works). Dummy env + nonexistent endpoint -> "cannot reach Kelpie's daemon: ..." (env reached the server). The hooks part of the settings file was omitted in this test (no daemon to receive them).
  - Codex: `codex exec --skip-git-repo-check -s read-only -c mcp_servers.kelpie.command="D:/.../kelpied.exe" -c ...args -c ...env_vars -c ...tool_timeout_sec -c ...default_tools_approval_mode="approve" "<same prompt>"`. Env unset -> same "Start this agent from Kelpie" text. Without `default_tools_approval_mode` the call failed with "MCP tool call requires approval, but approval policy is never" (so the key is needed; the approval then is Kelpie's own). With dummy env but `KELPIE_ENDPOINT` not forwarded the server answered "started by another Kelpie daemon" (Codex gives MCP servers only `env_vars`); adding `KELPIE_ENDPOINT` to `env_vars` -> reached the connect step ("cannot reach Kelpie's daemon"). Key names (`command, args, env_vars, tool_timeout_sec, default_tools_approval_mode`) verified by those runs against 0.166.0.
  - Not run: a real spawn against a live daemon (needs a daemon; phase 4 e2e).

## Files
- new: `crates/kelpied/src/{mcp.rs, mcp/tests.rs, sub_guide.rs, sub_stats.rs}`
- modified: `cli.rs` (sub + tests), `main.rs` (`sub`, `mcp`, mods, docs line), `agents.rs` (injection, `Launch.env`, `build_launch_with`, tests), `session.rs` (applies `launch.env`), `daemon.rs` (`launch_for` flag, stats file, close hook, presets outcomes), `daemon/sub_rpc.rs` (`sub_tools_on`, `record_sub_outcome`, hooks), `daemon/sub_rpc/tests.rs`, `subs.rs` (brief, `stats` field).

## Tests
`cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations`: 29 + 866 passed, 0 failed. `cargo clippy -p kelpied --tests -j 2 -- -D warnings` clean. New: ~16 in `mcp/tests.rs` (initialize/versions, notification, ping, parse/invalid/unknown errors, tools/list schema, outside-Kelpie error for every tool, text+structured, spawn validation, refusal passthrough, message queued/duplicate, session rows, read clamp+redaction, wait outcomes and mail path, timeout, report, close/inbox, output cap), 2 in `cli.rs`, 5 in `agents.rs` (Claude/Codex argv policy on/off with path containing spaces, shell/gemini env, TOML escaping, compare-before-write), 3 in `sub_stats.rs`, 2 daemon-level (outcomes recorded once and shown in presets, policy toggles injection). `build_launch_with` takes the platform wrapper so argv tests need no installed agent.

## Deviations / notes
- Chat sessions (app-supplied `chat_argv`, `chat.rs`) get neither `KELPIE_CLI` nor the MCP server; only terminal launches do. Spec did not cover chats' launch path; flag for phase 4/3 if wanted.
- `agent.presets` gained an `outcomes` field instead of a new RPC (no auth-matrix change); extra JSON field is ignored by the app.
- `kelpie_inbox` exists internally for `sub inbox`, not listed to MCP clients.
- Stats minutes use `created_at`; a session restored from before the daemon restarted counts its whole age.
- Codex per-server `default_tools_approval_mode="approve"` and Claude `mcp__kelpie` allow rule mean Kelpie's sub-session policy is the only prompt; with policy `allow` there is none by design.
- Plan/phase status cells not edited (plan CLI rule).

## Open questions
- Docs (phase 4) must mention `sub-stats.json`, `claude-mcp.json`, `claude-hooks-sub.json`, `KELPIE_CLI`, new `agent.presets.outcomes`.
- Should chat sessions get the tools too? Needs the app to pass an MCP config in `chat_argv`.

Status: DONE_WITH_CONCERNS
Summary: All seven requirements implemented and tested (tests and clippy clean); both empirical checks done and both real CLIs verified to load the injected `kelpie` server, run its tool and return the "start from Kelpie" error.
Concerns/Blockers: Chat sessions are not injected; a real spawn against a live daemon is untested until phase 4; Codex token stripping depends on the user's config (not stripped here).

## Fix round 1

Tests: `cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations` = 29 + 878 passed, 0 failed. `cargo clippy -p kelpied --tests -j 2 -- -D warnings` clean.

| Item | Done |
|---|---|
| M1 | `mcp::serve_io` (testable apart from stdio, runs on a `LocalSet`): every `tools/call` is its own task with an abort handle per request id; one writer task prints replies; `ping` and `tools/list` answer while a call waits. `notifications/cancelled {requestId}` aborts that call, EOF aborts the rest; dropping the task drops its daemon connection. Daemon side: `subs::wait_for_answer` takes a `gone` check (looked at every second) and the ask-mode thread passes `tx.is_closed()`: when the program hung up the request is removed at once, `sub.request_done {cancelled}` is broadcast, nothing is started, and a later `sub.answer` finds nothing. Mail is taken (`sub.inbox take:true`) as the last step before the reply, so a cancel earlier leaves it in the inbox. Residual window: a cancel in the instant between the daemon taking the mail and the reply arriving (one round trip) loses it; closing it needs a two-step take in the daemon, not done. Note: `printf ... \| kelpied mcp` now ends at EOF and aborts a call still running; test by holding stdin open. Tests: a waiting call does not block ping/tools/list; cancel closes the daemon connection and sends no reply; cancel names one call; EOF aborts the rest; bad UTF-8 and broken JSON are answered and reading goes on; `a_request_is_withdrawn_when_the_asker_hangs_up` (subs) and a daemon-level hang-up test (card withdrawn, no child, later answer refused). |
| M2 | `structuredContent` holds `message` = the same text (cautions included) next to the data; if the serialized data exceeds twice the text cap it is replaced by `{message, truncated: true}`. Tests for read, wait and spawn. |
| M3 | `kelpie_wait` notes on its first poll which sessions had already exited; they are shown but not watched (not in `wait_outcome`'s set, not in the idle grace). Only exited children: "nothing to wait for". The old assumption in the tests is replaced. |
| m1 | Codex `env_vars` also lists `KELPIE_DATA_DIR` and `XDG_DATA_HOME` (with `KELPIE_ENDPOINT`). |
| m2 | Brief and guide give bash `"$KELPIE_CLI" sub ...`, PowerShell `& $env:KELPIE_CLI sub ...` (guide also cmd). Test on the brief. |
| m3 | `kelpie_read` cuts the old lines (front) of an over-long screen, with a marker. Test. |
| m4 | `to`, `session` and the items of `sessions` accept whole numbers (only those keys; a numeric `task` is still refused). Tests. |
| m5 | `exited` is no longer recorded at exit (a restart or update makes one too). Outcomes are written for: explicit done/failed (judged), a run task done/failed (judged, in `capture_task`), and a close (`exited` if the agent had exited, else `closed`; not judged). `done_share` = done / judged and is null with no verdict, so agents that never report are not scored as failures; `kelpie_agents` text says what the share counts. Minutes = time since the daemon first saw the session change status (`Subs.births`), so daemon downtime is never in it; unknown minutes are left out of the median. The file is written outside the lock. The older `sub-stats.json` shape still loads. |
| m6 | stdin is read with `read_until` and a UTF-8 check: a bad line gets -32700 and the server keeps reading. |
| n1 | A session's `env` can no longer replace `KELPIE_SESSION_ID`, `_TOKEN`, `_ENDPOINT` or `KELPIE_CLI` (terminal and chat spawn). |
| n2 | `kelpied sub spawn/send/report`: options are read before the text, or after it when the text is one (quoted) argument; `--` ends options; words like `--json` inside an unquoted sentence stay text. `--json` before the command also works. Tests. |
| n3 | Kept the server name `kelpie` (a project `.mcp.json` server of that name would be covered by the allow rule; which definition Claude prefers is not verified). |
| n4 | Not changed: the existing `notify=['...']` literal breaks for an exe path containing a quote; the new `command=` uses an escaped basic string. |
| n5 | `readOnlyHint: true` on `kelpie_agents`, `kelpie_sessions`, `kelpie_read`; false on the others. Test. |
| n6 | A batch gets one -32600 (documented in the code; batching left the protocol after 2025-03-26). |

### Chat injection (owner decision)
`agents::chat_with_sub_tools`, called in `create_inner`'s `chat_argv` branch when the policy is not off and the chat is local:
- Claude stream-json chat (`claude` with `stream-json` in its command line): `--mcp-config <claude-mcp.json> --settings <claude-chat-sub.json>` right after the program. The settings file holds only `{"permissions":{"allow":["mcp__kelpie"]}}` (no hooks, no `--allowedTools`).
- Codex `app-server` chat: the same `-c mcp_servers.kelpie.*` overrides, inserted right after `app-server`.
- Any other command line (ACP agents) is untouched. The permission classifier (`launch_permissions`) runs on the command line as the app sent it; a test shows the injected flags change nothing for it.
- `ChatSession::spawn(argv, env)` sets `KELPIE_CLI`; `spec.env` cannot replace it.
- **ACP chats, note for phase 3**: the app builds `session/new` and `session/load` with `mcpServers: []` (`acp-adapter.ts`). It needs a server descriptor from the daemon (user role only, returned while the policy is not off), e.g. `{name:"kelpie", command:<kelpied>, args:["mcp"], env:[]}`. The session's `KELPIE_SESSION_*` are already in the agent's environment (set by `ChatSession::spawn`), but whether an ACP agent forwards its environment to stdio MCP servers is unverified; if not, the daemon must hand the variables by value to the user role. The adapter must also auto-approve `session/request_permission` for kelpie tools, or the user is asked twice.

### Claude MCP tool timeout (empirical)
Real Claude 2.1.319, `claude -p --mcp-config ... --strict-mcp-config --allowedTools mcp__sleeper`, a throwaway stdio server whose tool sleeps N seconds (prompt through stdin: `--allowedTools` is variadic and swallowed a trailing prompt argument, the same trap as `--mcp-config`).
- 70 s, 130 s, 310 s and 700 s calls all returned `SLEPT_OK N`. So the per-call limit is above 700 s: the default `kelpie_wait` (600 s) and the 300 s ask-mode approval fit with nothing set. No `MCP_TOOL_TIMEOUT` is added to Claude launches (a user's own value is theirs).
- Codex is the one that cuts calls (60 s default); fixed by `tool_timeout_sec=1900`.
- A 1850 s call was ABORTED by Claude after exactly 1800 s of silence: "sent no response or progress for 1800s; aborting ... set a per-server `timeout` (ms) ... otherwise set CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT (ms) globally (0 disables)". So there is an idle limit of 1800 s, which the old maximum of `kelpie_wait` (1800 s) would have hit exactly. Fix: `kelpie_wait` maximum is now 1500 s (deviation from the phase spec's 1800; default stays 600 s). Codex's `tool_timeout_sec=1900` stays (above 1500). The 300 s approval wait and the 600 s default are far below both limits.

Status: DONE_WITH_CONCERNS
Summary: All review items fixed (M1-M3, m1-m6, real nits), chat injection done for Claude stream-json and Codex app-server chats, Claude's tool timeout measured above 700 s.
Concerns/Blockers (fix round 1): ACP chats still need the app side (phase 3); a cancel in the one round trip between the daemon taking mail and the reply can still lose that mail; a real spawn against a live daemon remains for phase 4.

## Phase 4 fix: idle reports

Cause (F1): `ChildWatch::observe` turned any Working to Idle change after 3 s of output into an `idle` report, but an agent's output also goes quiet at start-up and between tool calls.

Fix, as decided (`subs.rs`, `daemon/sub_rpc.rs`, `daemon.rs`, `session.rs`, `sub_guide.rs`):
1. **Agents with a turn-end signal (kind claude or codex)**: `observe` never reports idleness any more. The report comes from `Daemon::hook` when Claude sends `Stop`/`StopFailure` or Codex sends `agent-turn-complete`: `sub_turn_ended` -> `ChildWatch::turn_ended`. It reports only if the turn really had work behind it (turn begun at least 3 s earlier; this also drops a background-thread notify with no work) and the child did not report by itself in the turn; at most once per turn. These kinds never get the quiet report (`settle_quiet`). Consequence: a Claude or Codex child whose hooks never arrive reports no idle at all (its explicit report, waiting and exit reports still work).
2. **Other agents**: `ChildWatch::quiet`, asked once a second from the mail tick (`sub_quiet_check`, one lock when nothing is quiet): after real work, continuously idle for `QUIET_IDLE` = 30 s (field `quiet`, `ChildWatch::with_quiet` for tests), never while the first prompt is still to be typed (`Session::first_prompt_pending`, nothing is used up while held), once per idle spell (more work starts a new one), and not if the child reported by itself.
3. **Wording**: the automatic idle mail and row now say "went quiet or finished a turn without reporting. It may still be working: only its own kelpie_report says it is done. Look at its screen (kelpie_read) before you send it work again". The guide has a new rule: automatic notices are hints; wait for the explicit report or `kelpie_read` first; never re-send a task because of such a notice.
4. Waiting (`needs_user`) and exit reports are unchanged (tested for both kinds).

Tests: `subs.rs` (turn-end signal vs quiet, short burst, background signal without work, explicit report, once per turn; hookless: 29 s not enough, spell broken by more work, hold while start-up, once per spell, next cycle; no work / explicit / exit) and `sub_rpc` (Codex-like session with Waiting -> Working -> Idle -> Working -> Idle start-up pattern: nothing mailed, even after a long quiet, then exactly one report on `agent-turn-complete` through `Daemon::hook`; hookless session: nothing at 29 s, nothing while the first prompt is pending, one report after, none again; needs_user and exited for both kinds). Existing automatic-report test now uses the turn-end signal.

Run: `cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations` = 29 + 895 passed, 0 failed (one test assertion fixed after the first run). `cargo clippy -p kelpied --tests -j 2 -- -D warnings` clean. Real agents were not re-run.

Notes: `scripts/e2e/sub_sessions.py` was not touched; if it expects an automatic idle mail from a stand-in, that now arrives after 30 s of quiet (hookless kinds) or on a hook (claude/codex). The first run hung on a lock taken twice (the temporary guard of `if let ... = watch.lock().quiet(..)` lived through `report_auto`); fixed by binding the result first.

Status: DONE
Summary: Automatic idle reports now come only from a real turn-end signal (Claude/Codex) or after 30 s of unbroken quiet (others, never during start-up), with wording that says the child may still be working; tests and clippy pass.
Concerns/Blockers: a Claude or Codex child whose hooks never arrive gets no automatic idle report; real-agent rerun left to the tester.

### F1b: an explicit report must hold through a flickering screen

Cause: `ChildWatch::observe` cleared `explicit` on every change to Working, so a screen that flickered Working -> Idle -> Working after the child's `kelpie_report done` let the later `agent-turn-complete` send an idle mail.

Fix (`subs.rs`, `daemon/sub_rpc.rs`, `session.rs`):
- `observe` no longer touches `explicit`. A new turn begins only in `ChildWatch::sync_turn(sid, input, turn_seq, now)`: the session's `last_input()` (user keystrokes, a prompt or mail Kelpie typed in; this is the boundary for Codex, which has no turn-start hook) or its hook turn counter `turn_seq()` (Claude's `UserPromptSubmit`) changed since the last look. Then `explicit` and the settled-quiet flag are cleared and the idle clock restarts; a session that was already Working gets its turn start set then.
- The daemon syncs before it decides anything about a session: in `sub_status_changed`, `sub_turn_ended`, `sub_quiet_check` (per candidate) and in `sub.report` before `explicit` is noted (so an input that came earlier cannot wipe the report just made).
- The hookless quiet report follows the same rule: an explicit report suppresses it until new input arrives.
- Chat children give no markers (`None`, 0).
- `ChildWatch::with_quiet` is `#[cfg(test)]` (the dead-code warning is gone).

Tests: `a_report_of_its_own_holds_through_a_flickering_screen_until_new_input` (hook agent: report, Idle/Working/Idle/Working flicker, turn-complete gives nothing; a prompt or a hook turn begins the next turn, which is reported; hookless agent: no quiet report after an explicit one until new input) and the daemon-level `no_idle_mail_follows_a_child_that_reported_done_however_its_screen_flickers` (Codex-like child: `sub.report done`, flicker via `sub_status_changed`, `Daemon::hook` turn-complete: the parent has only the `done` mail, the row keeps the child's words; a delivered prompt then makes the next turn-complete report). The earlier turn-end test now begins its next turn with `sync_turn`.

Run: `cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations` = 29 + 894 passed, 0 failed; `cargo clippy -p kelpied --tests -j 2 -- -D warnings` clean. Real agents not re-run. Note: one build in this round ran without `CARGO_TARGET_DIR` and left a build folder `D:\kelpie-subsessions\target` (git-ignored; a hook here blocks me from deleting a path named `target`, so the owner may delete it).

Status: DONE
Summary: An explicit report now stands until new input (typed text, delivered prompt/mail) or a hook turn start; screen flicker no longer re-arms the automatic idle mail, for hook and hookless agents; `with_quiet` warning gone.
Concerns/Blockers: stray `D:\kelpie-subsessions\target` build folder (see above); real-agent rerun left to the tester.

## Phase 4 review fixes

- **P4-1** The turn boundary is now `Session::last_submit()` = `activity.last_enter` (an Enter that submits: a prompt, or the Enter that ends the mail Kelpie typed), not `last_input`. Terminal replies (cursor position, device attributes), focus and mouse reports, cursor keys and plain typing no longer re-arm an explicit report. Test: report done, those inputs, flicker, turn-complete: only the `done` mail; then `next task\r` begins a new turn which is reported.
- **P4-2** Only `done` and `failed` set the reported-this-turn flag (`sub_report`); `progress`, `blocked` and `needs_user` do not. Test over all five statuses.
- **P4-3** `sub_quiet_check` settles Claude and Codex children only when `hooks_seen()`; otherwise they use the quiet path. Claude: 30 s. Codex: four times as long (120 s, `ChildWatch::quiet` has a `times` argument), because before its first turn ends Codex has sent no hook and may think silently for a while; this is a deviation from "30 s" for Codex, chosen to avoid bringing F1 back on a first turn. Tests: claude 31 s reports / 20 s does not; codex 31 s does not / 121 s does; Claude with SessionStart heard says nothing.
- **Nit 1 (done, cheap)** `Activity::hook_turn_in_progress`: Claude with a Working hook not yet ended, or Codex with an Enter submitted after the last announced end. `type_mail_when_idle` refuses while it holds (cap 10 minutes, so an interrupted turn that never announces its end does not block mail for good); the mail stays queued and `kelpie_wait` still reads it. Tests in `status.rs` and a daemon test (waits mid-turn, goes once the end is announced).
- **Nit 3** `scripts/e2e/sub_sessions.py`: temp paths in the PowerShell lines go through `psq()` (single-quote literal with doubled quotes).
- Earlier F1b test helper now sets `last_enter` (`set_last_submit_for_test`).

Run: `cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations` = 29 + 899 passed, 0 failed; `cargo clippy -p kelpied --tests -j 2 -- -D warnings` clean (all with `CARGO_TARGET_DIR=/d/kelpie-subsessions-target`). `python scripts/e2e/sub_sessions.py <rebuilt kelpied.exe>` (own endpoint `kelpie-sub-e2e`, temp data dir, stops only its own PID): 42/42 passed. Not touched: `docs/`.

Status: DONE
Summary: Turn boundary is a submitted Enter, only done/failed mark a turn reported, hookless-by-fact Claude/Codex children fall back to the quiet path (Codex at 120 s), mail waits mid-turn for hook agents, and the e2e PowerShell quoting is fixed; tests, clippy and the 42-check e2e pass.
Concerns/Blockers: Codex fallback uses 120 s instead of 30 s (deliberate); a Claude Esc-interrupted turn blocks mail for up to 10 minutes.
