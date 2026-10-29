# Kelpie architecture

Kelpie is a desktop workbench for running many coding-agent CLIs (Claude Code, Codex, OMP,
shells) across many projects at once. Performance comes from one rule: **the UI owns
nothing that must keep running**. Every PTY and agent process lives in a background daemon,
and the UI renders only what is on screen.

## Processes

```
Kelpie (Tauri app)                        kelpied (session daemon, one per user)
├─ WebView: SolidJS UI, xterm.js, CM6     ├─ Session × N  (PTY + VT state + ring buffer)
├─ child webviews (browser panes)         ├─ ChatSession × N (agent stdio JSON relay)
├─ Rust core: daemon client, fs, git,     ├─ status ticker (250 ms) + persistence (30 s)
│  search, file watcher, transcript       ├─ runs engine, automations
│  history, usage, accounts, plugins,     ├─ phone server (opt-in, HTTP/WS on the LAN)
│  integrations, remote (ssh), voice      │
├─ local socket ◄──────────────────────►  └─ local socket server
└─ stdin/stdout ◄──► kelpie-stt (speech)  kelpied hook  ◄── agent hooks (Claude/Codex)
```

| Process | Lifetime | Owns |
|---|---|---|
| `Kelpie` | while the window is open | layout, settings (`app-state.json`), views |
| `kelpied` | until stopped explicitly (or logoff) | PTYs, agent processes, terminal state, `sessions.json`, snapshots |
| `kelpie-stt` | while dictation is used (exits after 10 idle minutes) | the loaded speech model, the microphone stream |
| agent CLIs | per session | — |

`app-state.json` carries a format `version` (currently 3: n-ary pane splits). A file with an older version, an
unknown format or one that cannot be read is first copied to
`app-state.<label>-<time>.bak.json`; if that copy fails, or the file comes from a newer Kelpie,
the app runs without saving rather than overwrite it.

Closing or crashing the app never stops a session. If `kelpied` itself stops, sessions that
were running are restored *interrupted* with their last screen; the app resumes them when they
are shown, all at once, or on request (Settings → Agents). Agents resume their conversation
(`--resume <id>` / `resume <id>`); shells restart below their old output. A new Claude Code
terminal session gets its id from Kelpie (`claude --session-id <uuid>`, unless the user's own
arguments pick the conversation), so the id is known before any hook runs — also on an SSH
host, where hooks never run. Hooks still follow it when it changes (`/clear`, `/resume` inside
Claude). An id never saved to a transcript (nothing sent yet) starts again under the same id;
on an SSH host the remote shell checks for the transcript and runs `--resume` or
`--session-id`. Other agents' ids come from hooks or are recovered from transcripts.

Data: `%LOCALAPPDATA%\Kelpie` (Windows), `~/Library/Application Support/Kelpie` (macOS).
`KELPIE_DATA_DIR` and `KELPIE_ENDPOINT` override both (used by the dev scripts and tests).

Daemon state is published through unique same-directory temporary files. Windows uses
`FileRenameInfoEx` with POSIX replacement semantics: existing readers keep their old file
while a newly opened reader sees the replacement
([Windows contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)).
Unix keeps atomic rename. A publication error is returned without deleting the destination;
only the failed write's own temporary is cleaned. Alternate filesystems and redirected or
network data directories require separate validation of the native operation.

## Transport

Local socket only — no TCP port. Windows: named pipe `\\.\pipe\kelpie-<user>-v1` with a
protected DACL (SYSTEM + owner). macOS: `kelpied-v1.sock` (mode 0600) in the data dir.
Single instance is enforced with an exclusive lock file.

**Callers** (`crates/kelpied/src/auth.rs`). A connection names itself with `auth` before
anything but `daemon.info` answers:
- the **user token**, written at start to `<data base>/Kelpie/auth/<endpoint>.token` (owner-only,
  new per daemon run, keyed by endpoint so a dev app and dev daemon agree): the app, `kelpied`
  outside a session, `scripts/kelpie-call.py`. Full access; raw Input frames are accepted from it only.
- a **session token** (`KELPIE_SESSION_ID` + `KELPIE_SESSION_TOKEN`, derived from a per-daemon
  secret, set in each session's environment with `KELPIE_SESSION_ENDPOINT`): `kelpied` inside a
  session of that daemon (against another endpoint it uses that daemon's user token). The scripting
  surface (`session.list|read|input|create`, `meta.*`, `run.*` except `approve` — approval gates wait
  for the user — and `delete`, `hook.report`, and from API 13 `note.add|list`, kept to the session's own
  worktree even when scripting is off); `hook.report` and `run.report` always speak for the
  caller's own session, and `session.list` leaves other sessions' `env` out. With *Agents can control
  other sessions* off (`daemon.set_policy`, saved in `policy.json`) or a session created `restricted`,
  only its own session (`meta.set` compared on canonical paths, `..` refused). Everything else
  (settings, custom agents, automations, phone access, closing sessions) is the user's.
- a **claimed** session (`KELPIE_SESSION_ID` without its token, which some agents strip from the
  programs they run): hook reports for that session only. `kelpied` never falls back to the user
  token inside a session.

Programs run as the same OS user and could read the user token if not sandboxed: this scopes
what agents do through Kelpie by default; it is not a boundary against deliberate same-user code.

Broadcast events are scoped before they enter a client's outgoing queue. Anonymous and
claimed connections receive none; restricted sessions receive their own session lifecycle
events and scoped metadata. Scripting sessions can observe other sessions only while policy
allows it, with their `env` omitted. Administrative and unknown broadcast events are user-only.
The close event keeps its handle-only wire shape; its removed session's identity is passed
privately to the authorization check so the owner can still receive it.

Text Kelpie types into a session as a prompt, and every paste into a terminal view (clipboard,
drops, inserted paths, quick commands), drops control characters first (`session::typed_text`,
`typedText` in `typed-text.ts`, applied in `TerminalHost.paste`), so quoted content cannot end a
bracketed paste early. Text sent out of a
session (hand-off, Copy Context, notes or issues sent to an agent) passes `kelpie_proto::redact`.

Frames (`crates/kelpie-proto`): `u32 LE length | u8 kind | payload`

| kind | direction | payload |
|---|---|---|
| 1 JSON | both | `{"id","method","params"}` / `{"id","result"\|"error"}` / `{"event","params"}` |
| 2 Output | daemon → app | `u32 handle \| u64 seq \| bytes` (`seq` = stream offset after the bytes) |
| 3 Input | app → daemon | `u32 handle \| bytes` |

RPC methods: `auth`, `daemon.info|set_theme|set_awake|set_priority|policy|set_policy|shutdown`, `session.list|create|attach|detach|input|answer|resize|focus|kill|close|restart|update|read|set_unread`, `chat.close_stdin`, `hook.report`, `agent.presets|run|set_custom`, `meta.list|get|set`, `automation.list|state|save|delete|run|approve|set_paused|event` (API 15 for the last four and `state`), `run.list|get|create|approve|retry|task_done|task_skip|cancel|delete|report`, `note.add|list` (sessions: own worktree only) and `note.clear` (the user's) since API 13, `mobile.status|enable|disable|pair_start|approve|revoke|set_write|set_projects`, `mobile.load` (API 17: the machine-load level, lag percentiles; `{reset: true}` restarts the percentiles), and since API 12 `ui.focus`, `notify.get|set|test|rotate_topic|clear|report`, `session.schedule_continue|cancel_scheduled|scheduled|set_env_restart` (all the user's: no session token reaches them).
`daemon.info` reports an `api` feature level (`kelpie_proto::API_LEVEL`); an app built for a
higher level offers to restart an older daemon still running from a previous version.
Events: `session.updated` (coalesced, 250 ms), `session.turn` (API 9: `{session, event, transcript_path?, agent_session_id?}`, from every agent hook and when a session leaves `working`; built by the daemon from the hook payload, so no program can publish one — the app reads a session's usage on it, see "Context, tokens and cost"), `session.closed`, `session.resync`, `session.clipboard`, `meta.updated`, `notes.changed` (API 13: `{path, notes}`, a worktree's whole list), `automation.ran` (API 15 adds `dry_run` and `note`), `automation.changed`, `run.updated`, `mobile.pair_request`, `daemon.unresponsive` (API 17: `{seconds}`, the event loop has been silent for 30 s; sent by the watchdog, delivered when the loop runs again), `chat.resolved` (API 17: `{handle, id, request_id, by, dropped, answered_by, at}`, to the clients attached to a chat when a request of its agent is answered; see "Phone protocol (v2)"), and since API 12 `notify.event` and `scheduled.updated` (see "Notifications and limit actions").

## Session threading (daemon)

Per terminal session:
- **reader thread** — blocking PTY read (64 KiB) → `Session::ingest`: feed `alacritty_terminal`
  (the only VT parser in the system), append to a 1 MiB ring, fan out to subscribers.
  It never blocks on consumers.
- **writer thread** — owns the PTY master; input and resizes are serialised here, so a large
  paste can never deadlock against the reader (ConPTY requirement).
- **waiter thread** — waits for exit, closes the pseudoconsole, records the exit code.

Each subscriber has a 4 MiB unsent-byte budget. A slow view is marked stalled and told to
`resync`; the PTY keeps draining and the other sessions are unaffected.

Terminal queries (DA, DSR, OSC 10/11 colours) are answered by the daemon only while no
interactive view is attached; an attached xterm answers itself. The daemon answers colour
queries with the palette of the app's current theme (`daemon.set_theme`).

Windows: each session runs in its own Job Object (kill-on-close, owned by the daemon) so a
kill reaches the whole process tree; the daemon (started by the app in a new process group,
which Windows marks as ignoring Ctrl+C) clears that mark at start so sessions still get Ctrl+C; a modern `conpty.dll`/`OpenConsole.exe` is sideloaded
(see `third_party/conpty`). macOS: process groups (`killpg`) and a login shell (`$SHELL -l -c
exec …`) so agents see the user's PATH.

## Attach semantics

`session.attach {handle, since_seq?}` replies (inline, under the session lock, so no output
can slip between the reply and the live stream) with one of:

| mode | when | client action |
|---|---|---|
| `delta` raw | `since_seq` still in the ring, ≤ 256 KiB behind, no resize since | append bytes |
| `reset` raw | whole stream ≤ 256 KiB and never resized | reset, write bytes |
| `reset` snapshot | otherwise | reset, write ANSI snapshot (scrollback ≤ 2000 lines, SGR, wide chars, cursor, modes, title) |

Raw replays may contain terminal queries, so the view suppresses its own replies while
writing them.

## UI rendering policy

- Only the active project's active tab is mounted. Hidden terminals are *parked*: daemon
  subscription dropped, xterm buffer kept, element moved to a hidden holder that stays in the
  document (detaching a live WebGL canvas costs Chromium 100–200 ms). The 6 most recently
  hidden views keep their WebGL context (creating one costs 100–400 ms); older ones drop it,
  since browsers cap live contexts at ~16. Re-showing is a delta replay of what was missed. At
  most 16 parked views are kept; older ones are rebuilt from a snapshot.
- Output reaches the webview through one binary `Channel` per visible terminal, coalesced
  every 4 ms (`daemon_client.rs`).
- Solid's fine-grained stores mean a status change on one session re-renders only its dot.
- The file watcher and git status only run for the active project; git commands are capped
  at 3 concurrent processes and debounced (300 ms).

## Agent status

Priority: agent hooks → bell → prompt on screen → output activity.
- Claude Code: launched with `--settings <kelpie hooks file>` (and `--session-id`, see above);
  every hook runs `kelpied hook claude`, which reports the event (and the Claude session id,
  which changes on `/clear`) over the socket.
  Its generated settings use compare-before-write and atomic replacement, so concurrent
  launches never expose truncated JSON to a newly-starting agent.
- Codex: `-c notify=[kelpied, hook, codex]` (turn completion + thread id). Codex also runs
  unsaved background threads of its own that notify too; `kelpied hook` keeps a thread id only
  when Codex saved that thread (`$CODEX_HOME/sessions/YYYY/MM/DD/rollout-…-<id>.jsonl`, the day
  read from the UUIDv7 id), so a restart resumes the conversation, not a helper thread.
- Others: output activity (≥ 1.5 s of output after the echo window = working) and BEL.
- Any terminal session (agents without hooks above all — Codex approvals, Gemini, Aider,
  OpenCode — and shells): once output has been quiet 700 ms and the screen changed since the
  last look, `choices::detect` reads its last 14 lines for a prompt waiting at the cursor
  (a numbered menu starting at 1 with a marker or a question above it, `(y/n)` / `[Y/n]`,
  `(Y)es/(N)o`, `Yes (y)`…). Found → `waiting` until input or new output; `waiting_since`
  (wall-clock ms) is kept while it waits. `session.read` returns the prompt (`keys`,
  `question`, `line` = needs Enter). `session.answer {handle, waiting_since, data|key, enter}`
  types only while the session still waits on that same prompt, text through `typed_text`,
  Enter as a separate write; programs in a session may not call it. The app hides quick
  answers on daemons below API 7.

States: `working`, `waiting` (needs you), `idle` (+ `unread` after ≥ 3 s of work while not
visible), `exited`. What a status change turns into (inbox entry, desktop notification, phone
message) is decided by the daemon: see "Notifications and limit actions".

## Notifications and limit actions

One source decides "this is worth telling the user" (`notify.rs`, API 12). The ticker hands every
changed session to `Notifier::observe`, which compares it with the previous state of that session
(seeded when a session is created or restored, so a first sighting is not an event):

| Event (`notify.event.kind`) | When | Inbox kind |
|---|---|---|
| `waiting` | working → waiting | waiting |
| `finished` | working → idle | finished |
| `error` | a turn or process newly failed, whatever it was doing (once) | error |
| `automation` | an automation run started or failed to start | automation / error |
| `approval` | a run task began waiting for approval | waiting |
| `cost` | the app reports a session over its cost cap (`notify.report`; only the app reads usage) | cost (the app adds it itself) |

The app no longer classifies anything from API 12 on: it shows `notify.event` in its inbox and as a
desktop notification, worded in the interface language (`lib/notify-events.ts`). Against an older
daemon it keeps the old window-side rule (`store.legacyNotify()`), so notifications still work.

**What is in front of the user is not an event.** The app reports its window with `ui.focus
{focused, visible: [session ids]}` (per connection; dropped when the connection ends, so a closed
window shows nothing). A status event for a session that a focused window shows is neither broadcast
nor sent to the phone; automation and approval events are never suppressed. A computer idle for 5 minutes
is not being looked at, whatever the window shows. The desktop and the phone therefore agree by
construction.

**Channels** (Settings → Notifications → *Off this computer*, all through the app's connection,
user-only): ntfy (server, a random 128-bit topic made when ntfy is first switched on, optional access
token; the message goes as JSON to the server's root, so the topic is never in an address), Telegram
(bot token, chat id; `sendMessage`) and a webhook (JSON `{source, title, text, event}`). Their secrets
(topic, tokens, webhook address) are kept with `kelpie_proto::secret_*` under the service
`Kelpie (<endpoint>)` — a daemon with its own `KELPIE_ENDPOINT` (every dev and test daemon) has its own
service and never reads or writes the installed app's entries; `notify.json` holds only what is safe to
read, and `notify.get` says whether each secret is set, never its value (the topic only with
`reveal_topic`). Nothing runs until a channel is switched on: no thread, socket or timer (the sending
thread starts with the first message). Send errors say what failed (`HTTP 500`, `timed out`…), never
where; request addresses are not logged (the daemon's log filter keeps the HTTP client at `warn`, whatever
`KELPIE_LOG` says: its trace output contains the request path, a Telegram bot token among it). The sending
queue holds 32 messages and drops one that waited over 5 minutes; dropped ones are counted and shown in the
settings. A cost-cap report has its own 30 s allowance (it arrives seconds after the turn's message). An ntfy
access token is refused for an `http://` server on another computer. Filters: per-event switches (needs-you, done, failed,
automation, approval, cost cap), a finished turn must last N s (default 30), *only while the window is
not focused or the computer is idle* (idle: no keyboard or mouse for 5 min: the system's clock on Windows, elsewhere the window's own report of its last key or pointer use in `ui.focus`), and one
message per session per 30 s. The Telegram address can be pointed at a loopback fake only when
`KELPIE_E2E` is set.

**Payload.** Project, session label, status, turn length; every text passes `redact`. The end of the
screen is included only when switched on (a warning is shown), last 14 lines, 1000 characters, redacted.
The phone link is `<phone address>/#s=<session id>` while phone access is on and *Include a link to the
phone page* is (default on; the link holds this computer's LAN address and goes to the chosen server): no pairing code and no
token, so it opens only on a device that is already paired (the phone page strips the fragment).

**Continue at reset.** Claude Code's `StopFailure` hook with error `rate_limit` marks the session
`limited` (a `limited` field of `session.list`, cleared when the agent works again); other agents report
no structured limit, so they never show the banner. The pane shows a banner with *Continue at reset*,
*Switch account…* and *Hand off to…*. The reset time comes only from the usage meter (a full window's
`resets_at`, read for the session's own account) or is typed in and confirmed by the user;
`session.schedule_continue {handle, at, source: meter|confirmed}` stores `{session, generation,
agent_session_id, turn}` (`scheduled_input.rs`, written to `scheduled.json`). When it fires (30 s after the
reset, shortened for tests) `Session::continue_if_limited` checks under the session's lock — same run of
the process (generation), same conversation, same turn, still failed by the limit, idle, and **nothing typed
since the failed turn began** (a draft typed while the agent worked would be submitted with the text; the
Enter that sent the turn's prompt is the last keystroke allowed; answers to a permission prompt or question move the mark up, since they went to the dialog) — and only then types `continue` and, as a
separate write, Enter, which is dropped if anything was typed in between. Focus, mouse and query-reply
sequences from the terminal view are not typing. A refused timer marks the limit dead until the next genuine turn begins, so a bare report of it from a
program cannot re-arm it; once the agent has sent a report with the session token, reports without one are
ignored (until then the hook path stays open to agents that strip the token). Otherwise the timer is dropped and
`scheduled.updated {result: cancelled, reason}` gives the app an inbox note; closing or restarting the
session cancels its timer at once. No process outlives its daemon, so timers found in `scheduled.json` at
start can never fire: `session.scheduled` returns `{timers, lost}` and reports each lost one once, which
the app turns into an inbox note; the app replaces its timer list with `timers` on every load. A timer set for a session that is not limited, or for a time that has passed, is refused.

**Switch account.** `account_transcript` (Rust core) copies the conversation into the target account's
home (Claude `projects/<folder>/<id>.jsonl` and its sub-agents; Codex the dated `rollout-…-<id>.jsonl`),
an older copy of the same conversation in the target (the start of the file, left by an earlier switch) is
replaced, a newer or different one never; it refuses (with the reason, before anything is stopped) for other agents, the same
account, a home Kelpie did not create, an account that is not signed in, a different conversation under
that id in the target, more than 1 GB, or more than 2000 files. The app then stops the agent and calls
`session.set_env_restart {handle, env: {CLAUDE_CONFIG_DIR|CODEX_HOME}}`, which sets only that variable on
a stopped terminal session and restarts it on the same conversation (undone if the restart fails).

## Phone protocol (v2)

The phone page is a client of the daemon's phone server (`mobile.rs`), not of the RPC pipe: a connection
(`/ws`, JSON text messages, first message `{token}`) is registered as a daemon client through an adapter
and may use only the verbs below. `session.update`, `session.attach`, closing, phone settings and the
other admin methods are not verbs, so naming them reaches nothing.

The internal adapter is explicitly registered as a trusted user client; the phone itself
still has only the closed verb table below. Each server generation owns a sticky shutdown
signal shared by the listener, accepted HTTP/TLS connections and upgraded WebSockets,
including authentication. Disabling access revokes current permissions and cancels that
generation's connections; enabling it again cannot revive an old socket. Paired device
records remain available for a new connection.

- **Verb table** (`mobile/verbs.rs`, closed; least permission each): `list`, `hb`, `ack`, `sub`, `unsub`, `chat_sub` need
  *read*; `input`, `key`, `answer`, `chat_answer` need *write*; anything else is `unknown_verb`. The
  permission is read from the device's record on every message, so a revoked or downgraded device is
  cut off at once. A request may carry `req` (echoed in its `ack` or `error {code, reason?, message}`)
  and, for write verbs, the `epoch` it was made under (refused as `epoch` after a daemon restart).
- **Addressing and epoch**: sessions are addressed by `spec.id`. Handles restart at 1 and are reassigned
  by a restart, so a write by handle could land in another session. `hello {v: 2, api, daemon_epoch, rev,
  device, sessions, projects}`; `daemon_epoch` is random per daemon start, and a phone that sees it
  change drops every position and frame number it holds and re-resolves its ids.
- **The list is pushed**: the connection's adapter turns `session.updated|closed` events into a wake-up;
  it then rebuilds the list and sends `sessions {rev, sessions, projects}` only if it changed. Nothing
  polls. The adapter never blocks on the socket for events (only for chat lines, below) and a socket
  that takes 10 s to accept a message is cut (`slow_consumer`, see "Bad networks and busy machines").
- **Screens are state, not a stream** (`mobile/sync.rs`): `sub {id, height?, have_rev?, focus?}` follows a terminal
  session. One task per followed session, shared by all its phones, waits on the session's `watch` (bumped
  by `ingest`, a resize, and a tick that changes the prompt or the wait) and, at most once per 100 ms,
  takes `Session::screen_view` (the last 200 logical lines, cursor, prompt, `waiting_since`, title) under
  the session lock and hashes its rows; it ends with its last follower, and a session nobody follows costs
  nothing. A task per phone and session diffs the latest render against the rows it last sent that device,
  outside the lock. Frames: `screen {id, rev, base, shift, rows: [[index, text]], height, cursor, prompt,
  waiting_since, title}`: scroll the phone's lines up by `shift`, resize to `height` (the phone's own, the
  last rows of the render), replace the rows named. `base` is the frame it follows (`null`: the whole
  view). `base` is the last frame the phone *acknowledged* (`ack {id, rev}`), not the last one sent, and
  the frame says everything that changed since it; a phone keeps the screens from `base` on and drops
  older ones when a frame names a later `base`. At most 4 frames / 128 KiB wait for an acknowledgement per
  (device, session): then nothing is sent until one arrives, and what goes next is the latest state, after
  the acknowledged frame. Until the first acknowledgement frames are whole. A frame is reserved a place in the
  connection's bulk lane (4) before the state is read, so a slow socket gets the latest state, never a
  queue of old ones. A session whose frames average over 8 KiB (a full-screen redraw, a flood of output) is
  followed at five frames a second. Frames carry `server_ts` and the load `level`. Rows are cut to 300
  characters and, together, to 56 KiB, so a frame stays under 64 KiB.
- **Resume**: what was last sent is kept per (device, session) for 5 minutes after the connection
  ends. `sub` with the `have_rev` the phone holds (`rev` is read too) carries on from it when it is the last
  frame the daemon had acknowledged (the new connection takes the slot over, the old one's task ends),
  anything else gets a whole first frame. The cache keeps at most 64 KiB per (device, session), 20 sessions
  per device, 8 devices and 16 MiB together (least recently used first), drops everything nobody follows when L3 is caused by memory (not when it is CPU or lag),
  and forgets after 5 minutes; this is what makes a relay↔direct
  migration or a flaky connection cheap. A device has one connection: a newer one closes the older with
  4409 (the page then waits until it is looked at again). Close codes: 4401 the device was removed (the
  page forgets its token); 4408 connect again, the device stays paired (its permission changed, or the
  connection fell behind the daemon's events and may have missed a removal, which the next connect finds
  out); 4409 replaced. A close frame that cannot leave within 5 s is given up on. Rows are compared by text.
- **Typing and answering** share one path with the desktop's quick answer (`Session::answer`,
  `Session::type_message`): text goes through `typed_text` (control characters and with them a
  `\x1b[201~` paste break-out are dropped), several lines are one bracketed paste when the program asked
  for those, and Enter is a separate write 250 ms later (`ENTER_DELAY`: Codex takes an Enter within
  120 ms of input as a newline). The Enter is sent only if nothing was typed meanwhile and no new
  prompt came up (`Session::enter_if`); a phone's `input` reply waits for it, so that its next message
  starts after this Enter instead of merging with it. `input` is refused with `prompt` while a prompt is
  shown unless `force`; when the Enter was skipped the ack says `entered: false` (the text is on the
  prompt line, unsent).
  `answer {id, waiting_since, key | text, enter?}` is typed only while the session still waits on that
  prompt, checked and written under one lock. The page itself answers with `input` and `key`, which are credited to
  its device the same way (a later `answer` from another device is `stale` naming it, from the same one `dup`). A wait is answered once: `answered_wait` is set by `answer`
  and by any typing (the desktop's keys included: a hook's `waiting` outlives the key press), and cleared
  when the session stops waiting or a hook reports anything but `waiting` (a second prompt within one
  tick is a new wait). The daemon keeps the last 20 answered waits per session for 10 minutes, with who
  answered and when. The result of `answer` and `chat_answer` is `result`: `ok` (an `ack`), `stale` (somebody
  else answered it, or the prompt is gone or moved: `answered_by` and `at` when known), `dup` (this device
  answered it already, so a retry after a lost reply is harmless) or `refused`; `code` stays `stale`/`dup`/the
  refusal's own, `reason` is `not_waiting`, `moved` or `answered`.
- **Chat**: `chat_sub {id, since_seq?}` attaches as an observer (`ChatSession::observe`): it neither focuses
  the chat nor marks it read, and a chat may be asked for once a second per connection. The reply is
  small and the log follows as one output frame (nothing is encoded under the chat's lock). A
  connection has one budget for all the chat output it has not yet taken (4 MiB); while 256 KiB of it
  is waiting, a new `chat_sub` is refused as `busy` (so is one while other phones' logs are on their way:
  2 at L2, 1 at L3), so a client that does not read cannot make the daemon queue logs; the phone is sent
  `chat_resync {id}` when there is room, as its signal to ask again. The log after
  `since_seq` arrives as a delta when that is a line boundary still in the log; otherwise the last 2 000
  lines (2 MiB) behind the handshake answers the adapters need (`PINNED_IDS`, also those still in the
  log). Messages: `chat {id, mode: reset|delta|more|live, seq, lines}` (chunks of about 64 KiB, each
  saying where it ends; a longer line goes alone), `chat_resolved {id, request_id, by, dropped}`,
  `chat_resync {id}` (the daemon stopped feeding a phone that fell behind: ask again).
  `chat_answer {id, request_id, line}` answers a question the agent put to the person and nobody has
  answered: the daemon tracks only those from its output (Claude `control_request` `can_use_tool` and
  `elicitation`; JSON-RPC approvals, `requestUserInput`, `session/request_permission`, elicitation) and
  never the requests the client answers (hook callbacks, MCP messages, `fs/*`, `terminal/*`, tool calls). It
  accepts `line` only if it is exactly that request's answer in that dialect (a `control_response` with
  keys `type` and `response` only; a JSON-RPC response with `jsonrpc`, `id` and `result` or `error` only).
  What reaches the agent is the parsed line written again, so a phone cannot send another kind of
  message through this verb. Ids are compared as numbers (`7` and `7.0` are one id), the two dialects'
  never meet. Every line written to a chat agent, the desktop's too, is checked for being an answer: the
  first answer to a request wins, a second is dropped (`stale` for a phone), and every attached client
  gets `chat.resolved {handle, id, request_id, by, dropped, answered_by, at}` (API 17): `by` is who answered first
  (`phone` or `desktop`); `dropped` means this event is about an answer that lost, so a chat view whose
  user clicked it shows what the agent got instead. The sets of answered and pending requests are per
  process run and forget the oldest first.
- **Pairing** (`POST /pair`): failed attempts are counted per source address (10 a minute); at L2/L3 a
  handshake within 1 s / 5 s of the last is refused (503) without spending its code.

### Bad networks and busy machines

- **Heartbeat** (`mobile/lanes.rs`, verb `hb`, both ways): the daemon sends `hb {n, server_ts, level}` after
  20 s of silence, again every 8 s, and drops the phone (4410) after 45 s; a phone answers with
  `hb {echo: true}` (anything it sends counts) and may send `hb` itself, which is answered. Server
  `SO_KEEPALIVE` (30 s / 10 s / 3) is the backstop. A deadline only counts if the daemon's own timer was on time
  in that window; a gap of more than two ticks (stall, sleep) starts the clocks again (at most for 10 minutes
  without hearing the phone). At L2+ the times double (40 s / 90 s).
- **Legacy pages**: the first message is `{token, caps}`; a page that does not list both `ack` and `hb`
  (one opened before this protocol and still running its old script) is served as before: each frame counts as
  acknowledged when sent, so no window holds it back, and no silence is held against it (the OS keep-alive and
  the write timeout still end a dead connection). `hello.hb.legacy` says which. Reloading the page ends it.
- **Writer and lane**: one task owns the socket. Heartbeats, replies to the phone's own requests (answer results,
  errors), the session list and `load.status` it writes itself, ahead of the bulk lane, so none of them (nor revoke)
  waits for more than the send in progress. The bulk lane (4 messages of about 64 KiB) carries what other tasks
  produce: screen frames and everything about a chat, in order, so `chat_resolved` and `chat_resync` are queued
  after the lines they are about and never overtake the request. No room in the bulk lane for 10 s closes the
  connection with 4411 `slow_consumer` (a socket that takes no write for 10 s is simply dropped). The daemon's events for a
  phone use a bounded queue (`session::bounded_client_channel`, 256); when it overflows the phone is sent the
  list again and `chat_resync` for each chat it watches. No phone path has an unbounded queue
  (`mobile::tests::no_phone_path_has_an_unbounded_queue` greps for it).
- **Load levels** (`load.rs`, started with phone access, stopped with it): the event loop's lag (a 100 ms timer,
  p95 over 5 s), CPU and free memory (Windows `GetSystemTimes`, `GlobalMemoryStatusEx`, the low-memory
  notification; macOS `host_statistics`, `kern.memorystatus_level`), battery saver (L1) and thermal (L2) only raise.
  L1: lag > 30 ms or CPU > 85 %; L2: lag > 75 ms, CPU > 95 % for 3 s or free memory < 15 %; L3: lag > 200 ms or
  free memory < 8 %. A higher level is entered after 2 samples in a row; a level is left after 10 s below it, at most
  twice a minute (the limit holds back falls only).
  `KELPIE_LOAD_FORCE=L2:thermal` pins it for tests. Shedding (`Level`): the focused session's frames 100/100/250/1000 ms
  apart, others 100/250/1000 ms/paused, rows per frame 56/56/28/12 KiB (bottom rows first, the rest follows), heavy
  sessions 5 frames a second (1 at L3). A session someone is typing into (`input`, `key`) is rendered and sent at the
  calm rate for 3 s whatever the level, to every phone that follows it. Never shed: `input`, `key`, `answer`, `chat_answer`, revoke, heartbeats,
  acknowledgements, the session list, `load.status`. Phones get `load.status {level, reason, since}` on change and every second
  while the level is above 0. `mobile.load` reports it.
- **Keeping the daemon schedulable**: it opts out of EcoQoS power throttling when phone access starts (Windows),
  holds an activity against App Nap while a phone is connected (macOS: `NSActivityUserInitiatedAllowingIdleSystemSleep`,
  so the Mac may still sleep; keeping it awake stays with the keep-awake setting; compiled, not run, in this
  repository's checks) and never raises its priority class. The EcoQoS opt-out is not undone when phone access
  is turned off. A watchdog thread logs "runtime stalled" (runtime
  counters, the busiest threads on Windows) after 5 s without the timer running and broadcasts
  `daemon.unresponsive {seconds}` after 30 s (an event to the app's connection).

Checks: `cargo test -p kelpied` (verbs, diff, sync, answers, chat, load, lanes) and, against a real daemon,
`scripts/e2e/mobile_protocol.py`; the cost of eight phones is measured by `scripts/e2e/mobile_perf.py`; bad networks
by `scripts/e2e/mobile_resilience.py` (through `net_fault_proxy.py`); a busy machine by `scripts/e2e/stress.py`.

## CPU priority (Windows)

A keystroke crosses daemon → console host → agent → console host → daemon → webview, and
Windows' foreground boost goes to Kelpie's window only. On a busy machine every hop queues
behind other work, so with `daemon.set_priority {enabled}` (the app's *Prioritize visible
terminals*, on by default) the 250 ms ticker applies `proc_tree::Priority` per session:

| Session | Agent process + its ConPTY host | Rest of the tree |
|---|---|---|
| has an interactive view (on screen) | above normal | normal |
| no view | normal | normal |
| chat session | normal (its input is the app's own field) | normal |

Nothing is ever lowered: measured, a shell or agent at below-normal priority printed nothing for
30 s and more while other programs kept a 4-core CPU busy, which would freeze every hidden
session.

"Agent process" is the session's root, or — when the root only launches the agent (an npm shim
runs `node codex.js`, which starts `codex.exe`) — the root's child named after the agent's
binary too. The daemon runs above normal while the policy is on (children still start
normal); a hidden session's output-reader thread drops to the lowest thread priority, so
parsing its output does not run ahead of other programs. The console host is the
`OpenConsole.exe`/`conhost.exe` child that appears while the pseudoconsole is created
(serialised). A raised session is raised again every 5 s (a launcher starts the agent's own
process a moment later); other states are applied once. A `custom` session on screen stays
normal (its root is usually the workload itself). The Win32 calls run in a blocking task, off
the session locks and the async ticker. Measured with `kelpied bench-echo` (keystroke → daemon screen, optional
`--load N` busy sessions / `--external N` busy processes) and `scripts/e2e/typing_latency.py`
(→ rendered frame). Unix: no-op, an unprivileged process cannot raise niceness back.

## Chat mode

A chat session is a daemon `ChatSession`: the agent runs with piped stdio and the daemon
relays JSON lines (logged for replay on re-attach). The agents' lines carry no time, so the
daemon puts a `{"type":"kelpie_time","at":<unix ms>}` line ahead of the first line of each
second; a replay dates every item from it (Claude history read from its transcript uses the
entries' own `timestamp`). Adapters in `app/src/chat` map each
dialect to one item model (user, assistant, thinking, tool card with diff, permission, result):

| Agent | Process | Protocol |
|---|---|---|
| Claude Code | `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --replay-user-messages --permission-prompt-tool stdio [--resume id]` | stream-json + `control_request`/`control_response`: `initialize` (commands, models + effort levels, permission mode), `set_model`, `set_permission_mode`, `apply_flag_settings{effortLevel}`, `interrupt`; slash commands are sent as text and run by the CLI |
| Codex | `codex app-server` | JSON-RPC: `initialize`, `thread/start|resume|list`, `turn/start|interrupt` (model, effort, approval/sandbox ride on `turn/start`), `model/list`, `skills/list`, `thread/compact/start`, `review/start`, `thread/shellCommand` (`!cmd` typed in the composer — it runs outside the sandbox, as in the TUI; never for text Kelpie sends for the user), approvals; `$skill` adds a `skill` input item |
| ACP agents: OMP `omp acp`, Gemini `gemini --acp`, OpenCode `opencode acp`, Copilot `copilot --acp`, Goose `goose acp`, Qwen `qwen --acp`, Kimi `kimi acp`, Auggie `auggie --acp`, Cursor `cursor-agent acp`; plugin agents with an `acp` field | `ACP_ARGS` (`lib/agents.ts`) | ACP v1 (`protocolVersion: 1`; another answer stops with a message): `initialize` (no fs/terminal — the agent uses its own tools; `auth.terminal`, form `elicitation`), `session/new|load` (load only when `loadSession`; the ACP session id is the session's conversation id), `prompt`, `cancel`, `session/update` (chunks, tool calls, `plan`, `usage_update`, `available_commands_update`, `config_option_update`, `current_mode_update`), `set_config_option` (older `modes` / `models` blocks: `set_mode` / `set_model`), `request_permission` (the agent's option ids echoed), `elicitation/create` (form → a card with questions; url → declined), sign-in: `auth_required` → `agent` methods via `authenticate`, `terminal` methods named for the user to run, then retry. The daemon counts only message, thought and tool-call updates as work (usage, title, plan, mode and command updates arrive between turns), elicitation as waiting, and takes `session_info_update` titles as the session's title. |

The composer (`components/chat-composer.tsx`) completes `/` from the adapter's command list,
`$` from Codex skills and `@` from Kelpie's file index (`quick_open`), and shows the adapter's
settings (model / effort / mode) as pickers. `/clear`, `/new` and `/reset` start a new
conversation with an empty transcript in every agent. Claude Code runs them itself
(`conversation_reset`); for Codex and ACP agents chat view runs them as `thread/start` /
`session/new`, whatever the agent lists: in ACP mode most agents pass `/clear` to the model as a
prompt (Gemini, OpenCode, OMP, Copilot) and the rest only empty the context of the same session
(Goose, Qwen, Kimi), so an agent's own `clear` is not listed. Those lines and answers sit in the
replay log, so a replay clears at the same point. The model picker keeps the model a Codex
thread was started with even when Codex's catalog does not list it (one set in `config.toml`).
A command chat view cannot run (Claude's
`terminal_slash_commands`, a Codex command without an app-server request) offers *Run it in
the terminal*: the hand-off below, with the command as the new session's prompt. A resumed
launch passes it as an argument (`claude --resume <id> "/cmd"`, `codex resume <id> "/cmd"`), so
the agent runs it in that conversation itself; a typed prompt (too long for a command line) waits
for the program's first output before the quiet period counts. The daemon keeps the
command/model/skill answers when it trims a chat's replay log. Streaming replies render
finished markdown blocks once and only the open block as text; off-screen messages use
`content-visibility: auto`.

Beyond the items, adapters fill shared state the UI renders the same way for every agent:

| State | Claude | Codex | ACP |
|---|---|---|---|
| plan | `TodoWrite` input | `turn/plan/updated` | `plan` update |
| usage (context used / window, cost) | `assistant.message.usage` (input + cache + output) / `result.modelUsage[model].contextWindow`, `total_cost_usd` | `thread/tokenUsage/updated` (`last.totalTokens` / `modelContextWindow`) | `usage_update` when sent |
| turn diff | from Edit/Write inputs | `turn/diff/updated` | from `diff` contents |
| tool category | tool name | `commandActions` (read / listFiles / search) | tool `kind` |

`chat/tool-view.ts` turns a tool item into a row (verb, target, detail, IN/OUT) and diffs
edit strings line by line. `ChatController` queues what is sent while the agent is busy and
sends it when the turn ends (adapters set `busy = false` last); *Send now* uses the adapter's
`steer` (Claude: written mid-turn, the CLI reads it at its next step; Codex: `turn/steer`,
queued again if refused) or stops the turn; stopping holds the queue. Approvals carry what they
are about (command, edit diff, plan) and Claude's `AskUserQuestion` / `ExitPlanMode` are
answered in their card (`updatedInput.answers`, `updatedPermissions: setMode acceptEdits`).

Terminal ↔ chat switching hands off the *conversation*: find the conversation id (hooks,
transcript directory, or `thread/list`), stop the old front-end (Ctrl+C twice, then kill)
so there is never a second writer, and start the other front-end resuming the same id in
the same pane. Claude history from before the switch is read from its transcript file.
The conversation's settings come along (`chat/agent-settings.ts`): Claude Code restores none of
them on `--resume`, so its transcript's last `permission-mode` entry and latest reply's `effort` /
requested model become `--permission-mode` / `--effort` / `--model` for the chat process (and seed
its pickers until a turn reports the model); back in the terminal, chat view's picks become those
flags (Codex: `-s`/`-a` or `--dangerously-bypass-approvals-and-sandbox`, `-m`,
`-c model_reasoning_effort`), replacing the profile's own. Codex's app-server and OMP restore a
conversation's own settings themselves.

## Composer

One floating composer (`components/composer.tsx`, loaded the first time it opens; `Ctrl+Shift+I`
or the pane's Compose button) writes a prompt for any session: a shell, an agent's terminal UI or a
chat view. `lib/composer-state.ts` (in the main bundle, a few lines) says which session's composer is
open and holds the registry of selections (editor, diff and terminal register how to read theirs;
`@selection` is the one made last).

`@` mentions are expanded by the pure `expandMentions(text, ctx, kind)` (`lib/mentions.ts`, all
app access through the `MentionContext`). Claude, Codex and Gemini read `@path` themselves, so a file
or folder stays `@path` (`@"a path with spaces"` is quoted). Every other program gets the file
fenced and labelled with its path, in a fence longer than any backtick run in the content. A
snippet has at most 200 lines and all snippets together 64 KB (counted in bytes); past that the
agent gets a path: the file's own, `path:lines` for a selection of a file, or a temporary file
(`fs_write_temp`) for text without one (a diff, terminal output). `@selection`, `@diff` (`git_context`),
`@note` (the review notes of the project), `@terminal:<session>` (`session.read`, last 400 lines)
and `@issue:<key>` (Linear, Jira, GitLab; a bare `@issue` is the issue the session was started on,
named by its key) always inline, since they have no path; `@terminal` and `@issue` text passes
`redact` first, and so does what is written to a temporary file. A mention that finds nothing
(no selection, no changes, unknown session or issue) sends nothing and says so. GitHub issues are
not offered: `github.rs` only knows pull requests.

Delivery is the existing one: `sendToSession` → `typeInto` → `TerminalHost.paste`, so `typedText`
drops control characters (an `Esc [201~` in a file cannot end the paste early; xterm adds the paste
markers only when the program asked for them) and the Enter follows after the delay that keeps
Codex from reading it as a newline. The daemon's own `typed_text` (`paste: true`) only covers a
prompt typed at launch; composer text is cleaned by this front-end choke point, which the
detached-view fallback of `typeInto` also goes through (`pasteInput` keeps the markers there). A
chat view gets the text through its controller; pictures go along as `ChatImage`s (Claude: `image`
content blocks; ACP: `image` prompt blocks when the agent reports `promptCapabilities.image`, else
the file's path in the text; Codex: `localImage` items), at most 5 MB each (the API's limit).

What reaches a terminal is decided by whether the program asked for bracketed paste (`term.modes`):
without it every line break is an Enter key, so content is never inlined and a multi-line text is
refused (an agent still starting gets paths until it is ready). A shell or other non-agent program
(`shell`, `custom`) never gets content at all: only paths, written as one word for its shell
(`shellQuote`: single quotes, `'` doubled in PowerShell and `'\''` in POSIX shells, refused for
control or bidi characters and, for cmd, `% ^ ! & | < > ( ) "`), so `$(…)`, `;` or a backtick in a
file name stay text; and the text is left at the prompt, never submitted, once it holds a mention
or a picture (a name can be a script). Gemini's `@` form follows Gemini CLI's own escaping. On a
session on another machine long text goes in as its last lines (no local temp file), pictures are
refused. A terminal selection is masked like `@terminal`. A picture
pasted into a terminal's composer is written to the temporary folder (`fs_write_temp`, known
extensions, 25 MiB, files older than three days are removed) and mentioned by its path; a picture
dropped on the composer keeps its own path and is read (`fs_read_image`, picture types only) only
for a chat.

The file index of `search.rs` holds folders too (`dir/`); `quick_open` returns them only with
`dirs: true` (the composer), so file pickers are unchanged. Ranking (`search/rank.rs`, no app
access) scores an index of 8 000 paths or more on several workers (half the cores, at most 4, so a
search does not take the machine from other work; the result does not depend on the number) and gives the filename bonus, a second match per hit, only to the best 400
path matches: over 51 000 entries (a generated tree, release build, 4-core laptop) the median
query takes 6 to 8 ms and the 95th percentile 16 to 29 ms when the machine is not busy (one core:
14 to 18 ms and 36 to 65 ms; scoring every hit twice, one core: 15 to 20 ms and about 90 ms). The
ignored test `fuzzy_p95_on_fifty_thousand_files` builds such a tree and holds a release build to
30 ms. Sent prompts are kept per project
(`AppState.promptHistory`, newest first, 100 at most): Up / Down in the composer walk them and the
search palette (`!`) finds them.

## Context, tokens and cost

Per session: context fill (a ring on the sidebar row, pane header and tab: amber from 70 %, red
from 85 %, one-time Compact / Hand off hint at 85 %), tokens and cost. **No number without an
exact source**: a terminal session maps to its transcript only by the conversation id the daemon
recorded (`spec.agent_session_id`) — never "the newest file in the folder". No id, no numbers.

| Agent | File | Context | Cost |
|---|---|---|---|
| Claude Code | hook `transcript_path`, else `<config home>/projects/<slug>/<id>.jsonl`; plus its sub-agents `<slug>/<id>/subagents/agent-*.jsonl` (cost and tokens only) | last `assistant` line of the **main** file: input + cache read + cache write + output (none after a `compact_boundary`); window from `pricing.json` | estimate at API prices (shown "≈ $"), once per `message.id` at its highest output (a message is written as several lines; sub-agent files repeat it with a growing `output_tokens`) |
| Codex | `rollout-*-<thread id>.jsonl` under `sessions/YYYY/MM/DD`, found by thread id (cached) | `token_count.info.last_token_usage` and `model_context_window`; `info: null` records skipped | none: tokens only |
| OMP | `<ts>_<session id>.jsonl` | `contextSnapshot.promptTokens`, no window → no ring | the agent's own `usage.cost.total` (shown without "≈") |
| Chat view | the chat controller's `state.usage` | from the adapter (ACP `usage_update`, Claude / Codex) | as reported |

`session_usage.rs` keeps a per-file aggregate (`usage-cache.json`, resumed from the last complete
line, message ids hashed with the output counted so far) that also remembers the main file's last context,
so an update reads only what is new (at most 64 KB in all, sub-agent files included); a transcript without
an aggregate, or with a bigger gap, is counted on one background thread (context then comes from the
last 64 KB of the file, reverse scan, a cut or half-written line skipped) and the totals follow. A hook's
`transcript_path` is used only when it is exactly `<this session's config home>/projects/<slug>/<conversation id>.jsonl`,
local (UNC / `//` / `..` are refused by their text before any file access) and still inside `projects`
after symlinks resolve; otherwise the lookup by id applies. `pricing.json` holds the Claude prices; a model not in it
has tokens but no cost and no window. A line that looks like a usage record but does not parse is
counted and logged per agent (format drift hides numbers, it never invents them).

The app reads on `session.turn` (debounced 1 s, only for a visible or working session, or at a turn
end) and, against a daemon older than API 9, polls visible working sessions. The Usage & cost page
(`cost-page.tsx`, lazy) folds the same per-file aggregates by day / week / project / agent / model
(`usage_report`; a warm report only stats the files, ≤ 100 ms) and listens for `usage-scanned`.
An optional per-session cost cap (Settings → Usage & Resources, off by default) counts a Kelpie
session's conversations together (a `/clear` keeps the sum) and puts a red chip on the session and an
entry in the inbox; "Press Esc" interrupts the session once, the first time it is over the cap and
working, only when its numbers came from its own exact transcript. The hint, the cap's reactions and
the ring menu load lazily (`usage-alerts.ts`); the page's folding is in `usage-report.ts`.
Day boundaries on the page use today's UTC offset for all history (hourly buckets: no :30 / :45
splits, DST shifts an hour); Codex context uses `last_token_usage.total_tokens` (reasoning included).

## Turn checkpoints

Per agent turn, a snapshot of the session's working folder (`checkpoints.rs`; API 10, `checkpoint.*`,
all user-only: no session token reaches them). **Off by default**: a snapshot of a 10k-file repository
costs far more than the 150 ms budget (below), so Settings -> Agents -> *Turn snapshots* turns it on
(`checkpoint.set_enabled`, remembered in `checkpoints/config.json`; the app pushes its setting on every
connect, and an older daemon without API 10 never sees it and the app hides every turn view).

- **Boundaries.** One baseline at spawn (and at restart; a snapshot with an unchanged tree is skipped),
  then one snapshot when the ticker sees a session that was Working or Waiting become Idle or Exited:
  Claude `Stop`, Codex `agent-turn-complete` (shown "turn-complete to turn-complete"), a chat session's
  result / ACP `stopReason` all arrive as that status change, and so does a turn that ends on a prompt or
  with the agent exiting. An agent that has sent no hook (or is neither Claude nor Codex) is labelled
  *approximate* and only counts a boundary when something was typed since the previous one, so a TUI
  repainting a quiet screen does not snapshot; two turns may still merge. A snapshot read more than 3 s
  after its turn ended, or following one that failed, is approximate too. Shells and remote (SSH)
  sessions have none. A hook report alone never starts a snapshot, and `checkpoint.*` is not in the
  session role's method lists (an agent cannot undo anything). Turns whose tree did not change take no number.
  When a turn *starts* (Idle -> Working: a typed prompt, a chat message; for a guessed boundary only after
  typing) a detection-only **look** is queued in the same per-repository queue: the folder is read like a
  snapshot, but nothing is committed, referenced or recorded. If its tree differs from the session's previous
  snapshot, the differing paths (`between` on that turn's record, capped, all files when the look failed) were
  changed between turns by the user's editor, a shell or another session; undo flags them even when the agent
  edited them too. A look that runs after the agent's first edits only flags more.
- **Snapshot.** A copy of the repository's index in `<data>/checkpoints/tmp` (the stat cache is kept; a
  stale index lock there is removed first) is filled with `add -A --ignore-errors`, written as a tree,
  committed with `commit-tree -p HEAD` (git resolves HEAD; none in a repository without a commit) and
  stored as `refs/kelpie/<session>/<n>` (`update-ref` with an all-zero old value: a number is never
  written twice). The user's HEAD, branch, files and the index file are never written by a snapshot
  (undo's `git restore` takes the index lock for a moment, as any git command does; one retry when the
  user's own git holds it). Every git call runs with `GIT_LITERAL_PATHSPECS=1`, `GIT_OPTIONAL_LOCKS=0`
  and `core.hooksPath` pointing at an empty folder: the repository's `reference-transaction`,
  `post-checkout` and other hook scripts never run for these calls. Files another program holds open are
  skipped and listed in the record (`unreadable`); paths outside a sparse checkout do not fail it. Git
  runs at normal priority (below normal was starved for minutes under a build). The repository is opened
  once and remembered. Snapshots of one repository run one after another on a worker thread; a queued
  snapshot of a session absorbs a newer request by widening its window; a git failure is retried three
  times (0.15 s, 0.45 s) and then skipped, leaving a *gap window* for the session (see Undo). An existing
  `.git/index.lock` is no reason to wait: the copy reads the last complete index.
- **Records** in `<data>/checkpoints/<session>.json`: number, commit, base commit, `from` (when the previous
  snapshot began reading) and `at` (when this one finished), work start, source, approximate, counts, the
  first 400 paths, unreadable files. The next number is the highest of the records and the
  `refs/kelpie/<session>/` refs plus one, so numbering survives a restart and a lost record file. The last
  50 turns are kept (plus the base of the oldest). A baseline made after earlier work (a restart) covers that
  work as a stretch too. When a session closes it first waits for a snapshot of it that is running, turns any
  still queued into stretches (and a snapshot job already taken by the worker into one when it finds the
  session closed), deletes its refs, then keeps a **tombstone** (windows and file lists only) so other
  sessions' undo still sees what it touched; a tombstone is dropped when no live session's history reaches
  back to it. After
  `git worktree remove` succeeds the app calls `checkpoint.drop {worktree, root}`: records and refs of
  the sessions that worked there go (refs are deleted through `root`, the worktree folder being gone). The
  startup GC closes sessions this daemon recorded that no longer exist; it never touches other refs, so
  another daemon's snapshots in the same repository stay.
- **Auto-off.** A repository whose snapshot (the successful attempt only) takes over a second twice in a
  row stops being snapshotted until the daemon restarts, with one `checkpoint.disabled` event; a slow
  snapshot on a busy machine says nothing about the repository (a five-file repository took 2.3 s of wall time
  but 0.2 s of git CPU time with every core busy), so the verdict is on git's **CPU time**, counted through a job
  object around each git process (it includes the real git behind the Git for Windows launcher; measured: cpu
  171 ms for 115 ms of wall time on 10k files): slow means over a second of it. Only when no CPU time was counted
  (under 30 ms: non-Windows, or a counting that missed the real git) the wall time is judged, and then only for a
  repository whose index is at least 256 KiB and while a trivial `git --version` starts within 300 ms. Wall time,
  CPU time and the probe of a long snapshot are logged. Git messages are read in English (`LC_ALL=C`).
  `checkpoint.set_enabled {repo}` turns it back on. Nothing about it is saved to disk.
- **Undo** (`checkpoint.undo_plan`, `checkpoint.undo`; both, and `drop`, answer from their own thread so the
  app's other requests are not held up). Allowed only when the owning session is idle: its status is not
  Working or Waiting and it has printed nothing for 1.5 s (a report without the session's token cannot make a
  busy agent look idle). A turn's diff covers everything since the session's previous snapshot, so the
  stretch compared with other sessions is `from`..`at`, not just the time the agent worked; another session's
  edits during the owner's idle time are in the diff too. The plan lists every file of the turn (restore, delete,
  or skip for links and submodules) with a state: *shared* when a stretch of another session overlaps and
  its record lists the file, or may list it (a gap window, a record with too many files or unreadable ones),
  or that session is working in the folder right now; *modified* when the folder no longer matches the end of
  the turn, the file differed between turns (the look at the start), could not be read by this or the previous
  snapshot, was written before the turn's start (file mtime) or removed before it (its folder's mtime);
  else *clean*. Because every
  session's snapshot holds the others' concurrent edits, files in a shared folder are often shared. `undo` changes nothing and returns
  `needs_confirmation` unless every shared or modified file is named in `confirm`. Restores are `git restore
  --source=<base> --worktree` with literal pathspecs from stdin; deletes remove only paths that pass `safe_rel`
  and are not behind a link, then folders they leave empty. A file already equal to its state before the turn
  is skipped, so an undo can be repeated. The result is recorded as its own `undo` step. Restores follow the
  user's git settings (CRLF with `core.autocrlf`): a file can come back with other line endings, and the dialog
  says so.
- **Reading.** `checkpoint.list`, `diff` (files of a turn with counts), `blob` (a file at the end of a turn or,
  with `before`, as the turn found it; 8 MiB cap), `owners` (who touched which files in the folder, closed
  sessions included).
- **App.** Right panel tab *Turns* (`turn-timeline.tsx`, lazy): turns, expanded files, the files a turn produced,
  the owners view and the undo preview. The panel stays on the session chosen (or the focused one when it
  opened) and the dialog is bound to the session and turn it was opened for. A file opens a `diff` pane or a
  `file` pane with `rev` (the diff view and `turn-file-view.tsx` read the snapshot instead of the folder; paths
  are made project-relative with the repository prefix); markdown renders block by block and a comment becomes a
  review note on that block's source line. The watcher ignores `.git/refs/kelpie/**`. `DiffView` is lazy too
  (it freed more entry-chunk space than the new views use).

Measured, Windows 10, release build, quiet machine, `cargo test -p kelpied --release snapshot_cost -- --ignored
--nocapture` (`KELPIE_CP_FILES`, `KELPIE_CP_RUNS`): 10,000 files, a turn that edits 3 files and adds 1. First
version (12 turns, below-normal priority, 8 git starts): baseline 0.53 s, changed turn p50 0.74 s / p95 0.80 s,
unchanged turn p50 0.25 s / p95 0.28 s. Now (10 turns, normal priority, repository remembered, HEAD resolved by
`commit-tree`): baseline 0.38 s, changed turn **p50 0.38 s / p95 0.39 s**, unchanged turn p50 0.15 s / p95 0.16 s;
repository growth about 870 KiB per 100 turns. After the second review a detection-only look runs at each
turn's start (p50 133 ms / p95 145 ms on 10k files, like an unchanged snapshot) and the later measurement reads
baseline 0.34 s, changed turn p50 0.33 s / p95 0.35 s, unchanged p50 0.14 s: a turn now costs about 0.46 s of git
work. Most of the time is git walking 10k files and starting processes; it still does not meet the 150 ms budget for a changed turn, hence the default.
A 100,000-file repository was not measured (building the fixture did not finish in 90 minutes); expect several seconds per snapshot there, so such a repository switches itself off after two.

## App-side services (Rust core)

| Module | Does |
|---|---|
| `agent_history.rs` | Lists past Claude / Codex / OMP conversations from their transcripts (`~/.claude/projects`, `~/.codex/sessions` incl. `.jsonl.zst`, `~/.omp/agent/sessions`). Only the first 512 KB of a file (plus Claude's tail for its latest title) is read; results are cached by mtime/size in `history-cache.json`. Prompt counts run on two background threads, resume from the last counted line of append-only files, and emit `agent-history-counted`. |
| `session_usage.rs`, `usage_report.rs` | Context / tokens / cost of a conversation from its transcript (Claude, Codex, OMP parsers, reverse tail, per-file aggregate cache, prices from `pricing.json`) and the cost report for the Usage page. See "Context, tokens and cost". |
| `usage.rs` | Claude (`api.anthropic.com/api/oauth/usage`) and Codex (`chatgpt.com/backend-api/wham/usage`) plan windows, using the agents' own OAuth sign-in. Tokens never leave this module; the webview gets percentages and reset times only. |
| `sys_stats.rs` | Memory of the app + WebView tree, the daemon and its descendants (PID-reuse safe). |
| `git.rs` | Status, branch summary, diffs, staging, commits, worktrees (`add` / `remove` / `prune`, file at a revision, commits only a detached HEAD reaches). |
| `updater.rs` | Updates through `tauri-plugin-updater`: `latest.json` from GitHub Releases, verified with the minisign public key in `tauri.conf.json`. Install = download (signature checked) → installer (Windows, passive with restart; the app exits) or in-place bundle swap and restart (macOS). The daemon is never stopped. Dev builds and `KELPIE_E2E` runs never check. Endpoint/key overrides and a dry install exist only with the `e2e-updater` feature. |

**Updates keep sessions running.** Windows cannot replace a running program, so the app
(`daemon_runtime.rs`, release builds) starts the daemon from a copy in one of two fixed slots,
`<data dir>/daemon/a` or `…/b` (`kelpied.exe`, `conpty.dll`, `OpenConsole.exe` — the sideloaded ConPTY
starts its console hosts from its own folder; `build.txt`, written last, names the build). A new
build goes into the slot no daemon runs from; two fixed paths keep firewall rules (phone access)
and paths an agent was told valid across updates. The installer replaces the install folder while that daemon
and every session keep running; the installer restarts the app, which reconnects to them.
`checkDaemonApi` (run before interrupted agents resume) sees the older daemon and keeps it until no
session runs on it — checked again from the daemon's own list after 10 s of quiet, since a session can
be down for a moment (restart keeping the conversation, terminal↔chat switch); newer daemons also
refuse `daemon.shutdown {if_idle}` while anything runs — then it shuts it down and the app starts
the new copy; or the user picks *Switch now* (graceful stop: sessions come
back interrupted, agents resume). An updated app therefore has to work with an older daemon (API level
6 or newer): features that need newer RPCs check `daemon.info.api` and stay hidden until the
switch. A daemon started from a slot
removes anything else in `daemon/` no daemon runs from (`runtimes.rs`). A release that fixes a
daemon security problem raises `DAEMON_SECURITY_LEVEL`; against an older daemon the app then keeps a
*Switch session daemon* chip in the status bar until the user switches — it never stops sessions
by itself. macOS replaces
the bundle in place while the daemon keeps running from the old inode.

The NSIS hooks (`installer/hooks.nsh` → `installer/stop-daemon.ps1`) close the app first (Tauri's
`CheckIfAppIsRunning`), then stop only programs **directly in `$INSTDIR`** — a daemon started by
Kelpie 0.1.x before copies existed, gracefully with `kelpied stop --wait` and a wait for its exit —
compared in 8.3 form. The uninstaller an upgrade runs in place ("uninstall before installing",
`$EXEDIR == $INSTDIR`) does the same; only a real uninstall also stops and removes the copies in
`$LOCALAPPDATA\Kelpie\daemon`. `daemon.info.exe` tells the update dialog whether this update still has
to stop sessions (a daemon from the install folder: the one-time move from 0.1.x). A legacy daemon whose session runs the installer makes it refuse
(exit 3) rather than end that session. A graceful stop keeps sessions resumable: `persist(true)`
saves them as running before the process exits, and `restore()` marks them interrupted. Releases: tag `v<version>` → `.github/workflows/release.yml` builds and signs
(`tauri.updater.json` turns on updater artifacts; macOS as one `universal-apple-darwin` build whose
sidecars `build-release.sh` stages for each architecture before joining them with `lipo`
when `KELPIE_TARGET` says so), and
`scripts/release/latest_json.py` writes the manifest (`darwin-aarch64` and `darwin-x86_64` both point
at the universal bundle) into a **draft** release; apps see it once the draft is published.
The release job fetches the remote tag object into a separate ref before reading its
description; a checkout-local lightweight ref must not turn annotated release notes into
the merge commit message.
Each runner checks only its own platform's app binary before uploading artifacts. A missing
binary or a compiled updater test override stops the build before a draft can be published.

## Worktrees

A git worktree is a project with `parentId` (its repository) and `branch`; it has its own
tabs, layouts and sessions and switches like any project. Removal checks for changes and
commits only the worktree holds, leaves the worktree, stops sessions in its folder and waits
for their processes (Windows cannot delete a folder in use), stops watching the folder (an
unwatch that arrives while the watcher is still being created cancels it), then runs `git
worktree remove` — never deleting the branch; a deleted folder that something still holds open
lingers until the handle closes, so the leftover check waits up to 3 s. `.kelpie.yaml` (setup commands, default tabs) is read from the base
commit, shown in full, and runs only when the user opts in; approval is remembered per
repository by the file's hash.

Kelpie's local worktree commands share an OS file lock in the common Git directory
(`kelpie-proto/src/worktree_lock.rs`). Daemon run creation, desktop add/list/remove/prune,
and Finish scratch-worktree commands use the same lock across processes and linked
worktrees. This prevents Git from scanning another creation's incomplete administrative
files. The desktop waits on a blocking thread; copying/linking ignored files and running
checks happen after the lock is released. The lock file is kept permanently so all openers
lock the same inode. Remote Git and independently invoked external Git commands do not
participate in this local coordination.

Folder links use filesystem APIs, not a command interpreter: Unix symlinks and Windows
mount-point reparse points (junctions). Windows targets are canonicalized and encoded as
UTF-16; shell metacharacters in a directory name remain literal. A failed junction creation
removes only its newly-created empty destination, never source contents or an existing target.

### Review queue and Finish

The queue is the board status (`meta.rs`: todo / progress / review / done, shared with agents through
`kelpied worktree set --status`; `meta.updated` moves the card). Nothing else stores "in review".
`app/src-tauri/src/git_finish.rs` holds the git side; `lib/review-queue.ts` and `lib/finish.ts` decide
and sequence.

- **Summaries** (`review_changes`): per worktree, the files changed against the merge base with its
  repository's current branch (committed, staged, edited and untracked, once each), +/- lines, commit and
  uncommitted counts, and whether the change touches `.kelpie.yaml`. Answered from a cache keyed by the
  mtimes of the worktree's `HEAD` and index, the branch's and the base's references and `packed-refs`
  (read without running git), for at most 30 s because edits of tracked files do not touch the index;
  `force` skips it. 20 cached summaries answer in about 10 ms; cold, one costs ~130 ms of git on Windows.
- **Overlap watch**: for a repository with two or more worktrees (the repository's own folder counts)
  that have a live agent, every 30 s while the window is visible and 1.5 s after a `session.turn`, the
  summaries are intersected and each row shows "Also changed in <worktree>". With fewer than two active,
  no scan runs.
- **`.kelpie.yaml` `check:`** (a list of commands, one program and its arguments each, no shell) is read
  from the base branch's committed file (`configAtRef`), never from the working copy or the branch being
  merged, and runs only when that file's hash is the approved one (`approveSetup`, which also remembers
  the approved list so a change is shown as a diff). A change that touches `.kelpie.yaml` is flagged on the
  card and blocks the checks unless the user allows the base's list explicitly.
- **Finish** order (`finish_integrate` does 3-7, the app does 1-2 and the removal):
  1. the app stops the worktree's sessions and waits for their processes, then `finish_inspect` lists the
     worktree again; its HEAD must be on the branch the dialog named (`symbolic-ref`; the backend refuses a
     detached HEAD or another branch in the WIP commit and in `finish_integrate`);
  2. `finish_wip_commit(paths)`: only the confirmed files, as literal pathspecs; files that appeared since
     are returned in `extra` and nothing is committed; embedded repositories are refused; one commit
     "WIP from <agent>". `finish_integrate` itself refuses a worktree that still has uncommitted files;
  3. `merge-tree --write-tree` of base and branch (squash: `commit-tree` with the base as parent; merge:
     both parents; rebase: each commit replayed as a three-way merge against its parent, authors kept,
     merge commits refused). A conflict stops here;
  4. the approved commands run in a scratch worktree (`git worktree add --detach`) at that commit, with the
     base's `copy:` / `link:` brought in; the program is looked up on PATH, never in the checked-out
     folder; the scratch folder is deleted by Kelpie (`unlink_shared`, then `remove_dir_all`), not with
     `--force`; the first failure stops;
  5. `update-ref <base> <new> <old>` (compare-and-swap; a base that moved is `base_moved`, nothing changed).
     Where the base is checked out (the main checkout or another worktree) must have no change to tracked
     files, and no path the merge adds may already exist there (`diff --diff-filter=A old new`, covering
     untracked and ignored files), checked before the checks and again before the swap; afterwards that
     folder is brought up to date with `read-tree -m -u <old> <new>`, and if that fails the reference is put
     back. The `.kelpie.yaml` rule is enforced here too: a candidate that edits `.kelpie.yaml` / `.erka.yaml`
     with checks to run is `config_blocked` unless `allow_config_change` is passed;
  6. the branch is deleted only after `merge-base --is-ancestor`, or when merging it into the new base
     would change nothing (tree equality, for squash and rebase). A worktree that has it checked out is
     detached at the tip first (files unchanged), otherwise the worktree would read as full of staged files
     and `git worktree remove` would need `--force`;
  7. the worktree is removed last by the existing removal (its shared links are taken out first and put back
     if the removal fails; the branch is restored and the worktree re-attached to it).
  The card is set to Done after step 5, and errors after it are reported with the merge (`merged` with the
  branch note), never as "stopped". Nothing uses `--force` or `reset`. The sequence's own git runs with
  `core.hooksPath=/dev/null` (user hooks do not run for Kelpie's bookkeeping commits and reference updates;
  git resolves it itself on Windows, and nothing can be planted in a path that is not a folder; the daemon's
  snapshot git calls use the same), `LC_ALL=C` and no prompts; the user's commit identity is used, or
  "Kelpie". `commit-tree` ignores `commit.gpgSign`, so
  when it is true (`config --type=bool`) every `commit-tree` gets `-S` and git applies the key and format;
  signing, like every other wait, is bounded by two minutes and Cancel and fails before
  anything moves. Cancel belongs to one Finish (`finish_cancel(worktree)`, cleared by `finish_reset` when a
  Finish begins and ends and when `finish_integrate` returns): the app checks it between steps (stopping
  sessions, listing, WIP commit, merge), the backend ends a running signing or check; before the base moves it
  means nothing changed, after it the run only skips the cleanup and reports the merge as it is. Approvals are
  stored as content, not a hash: the canonical text of `setup`/`tabs`/`copy`/`link`, and the exact `check:` list.
  The check folder gets the base's `copy:`/`link:` only when the first is approved. Stale `kelpie-finish-<pid>-*`
  worktrees of dead processes are removed (links first) before each check run. Checks run in a Windows Job object with kill-on-close (a process group elsewhere), so a
  timeout or Cancel ends the whole tree; output is kept as a tail ring, not buffered. Summaries and overlap
  scans run on their own two-slot git pool (`git_review`) so they never queue interactive git calls; the
  overlap watch asks for names only. A worktree records the branch it was created from (`Project.baseBranch`)
  and Finish merges into that; for older worktrees it is the repository's current branch, confirmed in the
  dialog. Before removing a worktree Finish asks the daemon to `checkpoint.drop` it, so a turn-end snapshot
  does not hold files open; a removal that fails relinks the shared folders only while git still lists the
  worktree.
- **Automations** carry `deliver` (`review` or empty, API 11): when the run's first turn ends (or the
  session exits) the daemon does `meta.set status=review` on the automation's folder (the existing record
  above it, like `kelpied worktree set`). An older daemon ignores the field; the app hides the option below
  API 11. The mapping lives in memory, so a daemon restart during a run drops the delivery.
- **Event-triggered automations** (API 15) are described under "Automations" in the daemon list above. The
  window reports what only it can see: `app/src-tauri/src/watcher.rs` sends each batch's changed files
  (at most 200) to the daemon, `lib/pull-requests.ts` reports a failing pull request once, `lib/finish.ts`
  reports a finished worktree, and `lib/automation-events.ts` holds the global switch and the five-minute
  pull request look. File, pull request and finish triggers therefore fire only while the window runs; the
  settings page says so next to each.

### Cross-review and best-of-N

Review notes, run worktrees that bring ignored files, and the compare view (API 13). An app that
finds an older daemon keeps everything it did before: notes stay in `app-state.json`, run
worktrees start as clean checkouts, and the new buttons that need the daemon say so.

- **Review notes in the daemon** (`notes.rs`, `review-notes.json`): `note.add|list|clear`, keyed by the
  worktree folder (the folder holding `.git` at or above the path given, spelled like `meta.rs` keys
  folders; a path that does not exist is its own key, never the repository around it), broadcast as
  `notes.changed {path, notes}` to the user's connections and to the session of that worktree only (the
  daemon records each connection's role at `auth`; unauthenticated ones are sent nothing). A note is `{id, path, line, side, text, note, at,
  by?}`; text is stripped of control and bidi characters, 500 notes per worktree, 4000 characters each.
  One store feeds the diff gutter, the review card's *Request changes*, `@note` of the composer and an
  agent's `kelpied review add <file>:<line> "<text>" [--old] [--base REF]`. The app gives `side` and
  `text` (it showed the line); for an agent the daemon works them out: `git diff -U0` of the file
  against `--base`, else the uncommitted changes, then the default branch's merge base (`--no-ext-diff
  --no-textconv`, literal pathspecs, the ref checked not to be an option); a line outside every diff is
  quoted from the file (a regular file whose real path is inside the worktree), a removed line must be in
  a diff. Capability: `note.add|list` are in the session surface and also in the restricted one, but
  `auth.rs` rewrites their params for a session — `path` becomes its own worktree (another worktree is
  refused), `id`, `at`, `side`, `text` and `by` are dropped, `by_session` names the agent — so a program
  cannot write a note elsewhere or in the user's name, and `note.list` without a folder (every worktree's
  notes) answers the user only. For every method a session (or claimed id) calls, missing params are `{}`
  and any other non-object is refused before the method sees it (a struct deserialises from an array).
  `note.clear` and `note.keys` (which key a folder's notes are filed under) are the user's. `removeWorktree`
  clears a worktree's notes before it removes the folder.
- **Migration**: when the app meets a daemon of API 13 or newer it first asks for `note.list`, then — once
  per run of the app — takes `ipc.stateBackup("notes")` (the existing versioned copy of `app-state.json`),
  sends every note of `AppState.reviewNotes` with its own id, time, side and text, and drops a project's
  notes from the state only after the daemon has all of them (a note id the daemon already has is kept
  once, so a stop in the middle repeats safely). Without a backup nothing moves. `store.notesFor` reads
  the notes filed under the project's own key (`note.keys`; exact, so a worktree inside another folder
  never shows or sends that folder's notes) plus whatever is still local. *Request changes* clears the
  notes it sent, by id.
- **Run worktrees bring `copy:` / `link:`** (`kelpie-proto/src/materialize.rs`, shared with the app's
  `worktree_files.rs`; same order of checks as before, each entry looked at again just before it is brought,
  and an entry inside or around one already brought is skipped, so a copy never writes through a link onto
  the repository's own file; one `git check-ignore` run per worktree, one per entry if it fails):
  `create_task_worktree` calls `materialize` after `git worktree add` when the run has a
  `materialize: {copy, link}`. The daemon cannot see the app's approvals, so the app sends the field only
  for a repository whose `.kelpie.yaml` the user approved (then `.worktreeinclude` entries are read too,
  as for any new worktree; nothing approved, nothing sent, nothing brought) — from Try with N agents and
  from the Runs page's New run editor, which shows the same size line; a session's `run.create` has the
  field removed whatever shape its params have. A task records `base` (the branch the run folder was on,
  for Finish) and `brought` (structured: linked, copied, left out with the reason; the app words it). `worktree_materialize_estimate` measures the same selection
  for the size shown before a run starts. Tasks start on blocking threads; worktree Git metadata
  operations are serialized per repository, while materialization and independent repositories
  remain parallel. The five-worktree regression starts all creators together and checks that
  each receives its approved dependency link.
- **Review with…** (`lib/review-with.ts`, lazy dialog): a new session in the worktree with the review
  prompt (the quick command called *Review prompt*, else a built-in text; editable in the dialog), the
  instruction for `kelpied review add` (when the daemon takes notes) and the diff against the worktree's
  base — `git_context` branch + worktree parts, 48 KB at most, in fences longer than any backtick run
  inside — all passed through `redact` and `startWithPrompt`/`store.newSession(prompt)`, i.e. the daemon's
  first-prompt path (an argument up to 8 kB, else a bracketed paste, sanitised there).
- **Try with N agents** (`lib/try-agents.ts`, lazy dialog): one prompt, 2–5 agents, creates an ordinary
  Run of N tasks with `worktree: true`, no dependencies and no new field. The prompt is redacted once
  and capped at 64 KB.
- **Compare** (`run-compare.tsx`, lazy; the model is `lib/run-compare.ts`): a tab of the Runs page for a run
  with two or more worktree tasks. It reads each candidate's worktree once (`review_changes` cache, the
  base's checks, turns from `checkpoint.list`, cost from the usage readings) and again when a task
  changes state; only the elapsed time of a candidate still running ticks. *Use this one* confirms,
  listing what goes, stops the other candidates at once (`run.task_skip`, their sessions closed and waited
  for) and records each one's commit and uncommitted files, then opens the winner's Finish with a hook
  (`finishSucceeded`). When Finish has merged the work and removed the worktree, a candidate that still holds
  exactly what was recorded has its uncommitted files saved in a WIP commit (`finish_wip_commit`, the files
  that were recorded), its worktree removed the ordinary way (never `--force`) and its branch deleted with
  `branch -d`, which refuses an unmerged one: the branch stays with the candidate's work and the Activity
  entry says so. A candidate that changed after the confirmation (a commit, a file), could not be stopped
  or cannot be removed without force is left as it is, with the reason. *Discard…* does the same for one
  candidate. The run engine's adoption of worktree tasks as sidebar projects skips folders that no longer
  exist and worktrees removed in this session.

### Spec workspace

The Plan tab (`plan-panel.tsx`, lazy; the model is `lib/spec-index.ts`) reads the repository's markdown plans
as phases and tasks. It has no daemon part and no store of its own: checkbox state is the file's, and a
task's run status is the Runs engine's.

- **Detection**: `plans/*/plan.md` + `phase-*.md` (one folder level), `specs/**`, `.kiro/specs/**` and
  `docs/plans/**` (to six levels, 300 files, 256 KB each). Only paths found by walking the project are used,
  never a path named inside a file; symlinks and junctions are not entered (a root that is one is skipped),
  and names with control, bidirectional or quote characters are not listed. A remote project has the tab too: the same
  calls go through `fs_list_dir` / `fs_read_file` / `fs_write_file` / `fs_create`, which speak ssh for an `ssh://` root
  (the link check uses the listing's symlink flag; times are whole seconds, content is compared by digest). There
  is no watcher on a host: the panel reads again on demand.
- **Parsing**: marked's block lexer over the whole file, inline tokens only for headings and the first block
  of a task (most of the cost); line numbers come from the raw text of the tokens, which is checked against
  the checkbox pattern on that line (a task whose line cannot be confirmed is shown but not tickable).
  `parseSpec` never throws. A task's id is `spec-` plus a hash of the file path, its text and its occurrence.
- **Live**: the index listens to the existing `fs-changed` events only while the tab is open. Events for
  folders outside the four roots are ignored; the rest are debounced for 300 ms and reconcile only the
  folders named (a new folder is found through its parent's listing, which also tells whether it is a link).
  A root created later (`specs/`, `docs/plans/`, `.kiro/specs/`, found from the project root's, `docs`' or `.kiro`'s own event and the new root's) is
  looked for in its parent's listing, only while one of the four is unknown; no deeper watcher root is added. `stats` counts scans, reads
  and parses (the e2e checks they stay still while the tab is closed). A file is always parsed whole (any cut changes what is a task: html
  comments and blocks, nested fences and reference definitions reach across lines). Files over 8 KB are parsed in a worker
  (`spec-worker.ts` runs `parseSpec`; `spec-parse-client.ts` is the page's side, with a parse in place when there is no worker or it
  fails), because marked's list tokenizer slows with every item of one list (a 256 KB list takes a few hundred milliseconds): the
  page only posts the text and receives the plain result. Rows are keyed by position, so an edit does not rebuild them. `docs` and
  `.kiro` are checked as plain folders too, not only the root inside them.
- **Tick**: `index.tick` re-reads the file, requires its time and content digest to be the ones parsed and
  the task's line to be unchanged, checks that the file and every folder above it are still plain entries,
  then writes through `fs_write_file` with the time it just read as `expectedMtime` and the POSIX `cksum` of the bytes it read as
  `expectedCksum` (on a host both are checked right before the write, and the content goes to a temporary file made by `mktemp "<target>.kelpie-tmp.XXXXXX"` in the target's own folder (exclusive, mode 0600,
  removed by a trap on every failure; no `mktemp` on the host means no write), whose size and checksum are checked before it replaces the
  file; the values reach the script as positional parameters (`set -- <quoted values>` in front of a constant script), never pasted into
  its text, so a file name cannot reach it, and a project at `/` confines nothing out of itself; a link is followed to its target (confined to the project for the index, not for the editor's save), the mode is copied with
  `chmod`, never `cp -p`, the owner is not touched, and a folder that cannot be written to gets an in-place write after the content
  is verified in a temporary file; a conditional write never recreates a file that is gone); it changes the one
  character and nothing else. A mismatch writes nothing and reads the file again. Text is decoded strictly as
  UTF-8 (a byte order mark is kept), so what is written back is exactly what was read.
- **Run with…** (`createSpecRun` in `lib/runs.ts`): a run with one task (`worktree` as chosen, the profile's
  flags, `materialize` only from an approved `.kelpie.yaml`). The prompt is the scrubbed instruction text
  followed by the file's mention expanded by `expandMentions` for the agent's kind; the heading and task text
  are added after the expansion so they cannot become mentions. `runOfSpecTask` finds a row's latest run by
  its task id, so there is no `task_ref` store.
- **Tasks on a host** (API level 14): `runs::Task.host` (`TaskHost`: ssh destination, port, `-F` config, absolute
  folder there) next to `worktree`. `build_run` validates it (destination of letters, digits and `._@:%[]-` that does not
  start with `-`; port other than 0; an absolute folder without control characters) and refuses a worktree on a host.
  `start_task` hands it to the ordinary session path as `remote` and `cwd`, so the launch is `agents::build_remote_launch`
  (ssh -t, the host's login shell) with `tmux: false` always: the task's end is the ssh session's exit (0 = done; any other code, including
  ssh's 255 for a dropped connection, = failed with "the agent on <host> stopped with the connection"), which tmux would make
  meaningless (a detach and a crash both exit 0, a dropped connection leaves the agent running). `Runs::step` also skips idle completion for
  such a task, and a task whose session came back `interrupted` after a daemon restart is failed and released (`Action::Release`
  clears the flag) so the app does not resume it as an agent no task follows. The same rule runs once at the end of `restore()` (`settle_host_tasks`),
  before any client is served, so an app set to resume sessions at once cannot restart the agent first; `release_session` marks the session
  data dirty, so a second stop does not bring the session back as interrupted. A worker that does not answer in 15 s is stopped: the file
  is parsed in place when it is at most 64 KB, and shown as too slow to read otherwise. The prompt of such a task leaves out the report command: it is a program of
  this computer. `auth::scope_session_params` refuses a `run.create` whose tasks name a host when the caller is a session
  (it does not drop it, which would run the task here), and refuses any `tasks` that is not a list of objects (a task sent as an array is
  deserialised field by field, and its tenth element is the host); claimed sessions cannot create runs at all. This does not stop a
  session from starting an ssh session through `session.create` with a `remote` (a separate, older door, noted as a follow-up). The app asks for a host
  only when `daemonApi() >= RUN_HOST_API` (an older daemon ignores the field and would run the task here), takes the folder from
  the remote project or from the folder linked to the project on a saved host, and adds that host's askpass environment
  (`ssh_session_env`) like any remote session.
- **New spec**: `createSpec` slugifies the name (ASCII, at most 48 characters, no Windows device names),
  refuses when `specs/` or the spec's folder is a link or any of the three files exists, and creates each
  file with `fs_create` (which refuses an existing one) before writing it.

## Daemon services beyond sessions

- **Agent presets** (`agents.rs` `PRESETS`): binary, launch args, skip-permission flags,
  "continue latest" flags and how a first prompt is passed (argument, flag, or typed). The UI
  reads them with `agent.presets` (with an installed check) and adds the Yolo flags itself.
  Agents without hooks restart with their "continue latest" flags.
- **Typed prompts** (`session.rs` `PendingPrompt`): a prompt for an agent that takes none at
  launch, or a shell startup command, is written once the program has run ≥ 2.5 s and been
  quiet for 1.2 s (30 s at most): a bracketed paste for agents, keystrokes for shells, then
  Enter as a separate write.
- **One-shot runs** (`oneshot.rs`, `agent.run`): `claude -p` / `codex exec -` with the prompt on
  stdin, on its own thread; the reply is sent later on the client's channel, so a slow model
  never blocks that client's ordered request queue.
- **Workspace notes** (`meta.rs`, `meta.*`, `workspace-meta.json`): comment + board status
  per canonical folder path; `meta.updated` is broadcast. `kelpied worktree get|set` defaults to
  the git top level of the current folder.
- **Automations** (`automations.rs`, `automation.*`, `automations.json`): hourly / daily /
  weekdays / weekly in local time (chrono). The ticker checks every 10 s and starts each due run on its own
  thread (never awaited: ten due runs cost the ticker about 2 ms); a run creates an ordinary session with
  the prompt and is broadcast as `automation.ran`. Runs missed by more than 30 min are skipped.
  Since API 15 an automation can instead carry a `trigger` (`file`, `turn`, `commit`, `pr_checks_failed`,
  `finished`) and an `action` (`agent`, `quick`, `shell`); the runtime is `daemon/automation.rs`:
  - **Events.** `turn` is raised by the ticker when a non-shell session leaves working; `commit` by comparing
    `git rev-parse HEAD --abbrev-ref HEAD` of the folder with the last look (at turn start and end, and every
    10 s for folders of commit automations): only HEAD moving on the same branch counts, so checkpoint refs
    under `refs/kelpie` and checkouts never fire it. `file`, `pr_checks_failed` and `finished` arrive as
    `automation.event` from the app connection only (the file watcher sends the changed files of each 150 ms
    batch of the roots it already watches; no new roots). An automation reacts to events whose folder is its
    folder or inside it.
  - **Guards** (`Automations::on_event`, pure and unit-tested): enabled and not paused, not busy (a session it
    started is working, its command is running, or either stopped less than 5 s ago), at most one run per 60 s
    (`KELPIE_AUTOMATION_GAP_MS` shortens it for end-to-end tests), a per-trigger daily cap (counted per local
    day, saved), and a command only when its hash is approved. Matching 100 triggers takes well under a
    millisecond.
  - **Commands** are spawned without a shell (`std::process::Command` with the program and argument list),
    `stdin` closed, `KELPIE_*` event variables added, a 5-minute limit, the end of the output redacted into
    the run's note. Whole-argument placeholders only (`{file}`, `{files}`, `{commit}`, `{branch}`); a file
    name starting with `-`, `@` or `+` is given as `./name`. The program is resolved the way a user expects
    (`resolve_program`): a path with a separator against the automation's folder, never the daemon's; a bare
    name through `PATH` and, on Windows, `PATHEXT` (`npm` is `npm.cmd`, which `std` starts with safe
    quoting, which needs Rust 1.77.2; the crate pins 1.82). The approval is the SHA-256 of the event, folder, the **resolved file** and
    the arguments **and the contents of the files it runs from the project** (the program when it is inside
    the folder, the script an interpreter runs, every argument naming a file); `automation.approve` stores it
    (an editor's copy is ignored). An agent that edits an approved script makes the automation "changed since
    approved": it does not run and the inbox says so until the user approves again.
    Event data is allowed by an **allowlist** (`check_command`, at save and again at run). The program, resolved
    (`PATH`, `PATHEXT`, the file's real name, versions such as `python3.12` or `pwsh-preview` included), is
    classified. A *remote or elevated runner* (cmd, ssh, plink, wsl, su, sudo, doas, runas, schtasks, at, start,
    conhost, wt, watch, script, forfiles, xargs, env, nohup, time, timeout, nice, npx…), a *package manager*
    (npm, pnpm, yarn: `npm run` expands `%VAR%` in a name through cmd.exe, probed) and a *build tool or task
    runner* (make, cmake, gradle, mvn, msbuild, ninja, bazel, just, rake, task, tox…), or an environment, version, secret or sandbox
    *wrapper* (uv, poetry, pdm, hatch, rye, pipenv, pixi, bundle, mise, asdf, volta, fnm, nvm, pyenv, rbenv, direnv,
    nix, nix-shell, guix, dotenv(x), devbox, flox, earthly, act, doppler, op, aws-vault, infisical, chamber,
    firejail, bwrap, proot, docker, podman, kubectl…) never get event data:
    a `{placeholder}` or `KELPIE_*` anywhere in its arguments refuses the command. A *command interpreter*
    gets it only as `interpreter [safe flags] script args` with a script file that exists inside the project
    (or in a `PATH` folder): `powershell`/`pwsh` need `-File x.ps1` after only -NoProfile, -NonInteractive,
    -NoLogo, -ExecutionPolicy <value>; `bash`/`sh`/`zsh`/`fish`… the script as first non-flag argument after
    only -e, -u, -o <name> (never -c in any form); `python`/`node`/`ruby`/`perl`/`php`/`deno`/… the script as
    first argument, no flags. A batch file called directly gets arguments quoted by `std`, but `%` and `!`
    written into its arguments are refused. Anything else (git, a real executable) gets whole-argument
    placeholders. The program is classified by its given name and by its real file name (canonical path: an 8.3
    short name or a link does not hide it), with trailing dots and spaces ignored as Windows does. Only
    `.com/.exe/.bat/.cmd` ever resolve as programs (never `.js`, `.vbs`, `.wsf`, `.hta`, `.lnk`, `.url`). A program with no rule gets whole-argument placeholders, and `shown()` flags it (`command_unvetted`) so the
    approval dialog warns that agent-chosen names will be its arguments; the safe forms (a script run by its
    interpreter, a batch file called directly) carry no warning. The refusal message points to the safe form. A command is started in a job object (a process group on Unix): after 5 minutes everything it started is
    stopped, so its output pipes close and "running" clears; at a normal end what it launched (an editor it
    opened) is left alone and its output is not waited for beyond 3 s. An "agent" of
    kind `shell` or `custom` types what it is given into a shell, so it counts as a command: app-only save,
    approval, and the prompt carries no event data. Only agents Kelpie knows can be agents.
  - **Quick** actions type the saved prompt, as one bracketed paste with ESC and other control characters
    dropped (`typed_text`), only into a session **this automation started** (never one the user opened), only
    while it is idle and nothing was typed into it in the last 30 s (checked and written under one lock), then
    Enter as a separate write if nothing else was typed. With no session of its own alive it starts one.
  - **Loops.** An automation set off while another one is busy, or fired in the last two minutes, in the same folder is one step further
    down a chain; at 3 it is not run and the inbox says so. Daily caps and the per-automation busy guard
    (sessions `Working` or `Waiting`, running commands, 5 s after) bound the rest.
  - **Storage.** `{"version": 2, "paused": bool, "automations": [...]}`. A bare list (version 1) is copied to
    `automations.json.bak` (never over an earlier one) and rewritten. A file that does not parse is renamed
    `automations.json.corrupt-<time>`; a file with a newer version is left alone; in both cases every write is
    refused, the app is told through `notify.event` (inbox, only once an app connection is there to read it)
    and `automation.state.error`, and nothing replaces the file. A set-aside file stays a problem across
    restarts until the user dismisses it (`automation.resolve dismiss` renames it `.seen`). Every save also
    writes the same bytes to `automations.json.v2`, a name older Kelpies never read: an older Kelpie reads the
    object as damaged, starts empty and replaces the file with its own list. Downgrading is not supported, but
    nothing is lost: at the next start of this version the list found beside an existing `.v2` raises a notice
    and `state.fix = "restore"`; `automation.resolve restore` merges the `.v2` list with what the older one
    added, `keep` sets the `.v2` aside (`.v2.kept-<time>`). Until then nothing is written. The app offers the
    new options only from API 15.
  - **RPCs.** `automation.list|state|save|delete|run|approve|set_paused|resolve|event` are the user's: no
    session token reaches them (none is in the session or claimed method lists). `save` of a command,
    `approve`, `resolve` and `event` also need the connection to say it is the app (`auth` with
    `client: "app"`, which the window's connection sends and `kelpied` does not). That is a convention, not a
    boundary: whoever holds the user token can send it, and the user token already allows everything these do
    (it can create sessions). The boundary against agents is the session token, which these methods refuse.
    `event` takes only `file`, `pr_checks_failed` and `finished`.
    `automation.list` adds `command_hash` to commands.
- **Keep awake** (`awake.rs`, `daemon.set_awake`): Windows `SetThreadExecutionState` from one
  dedicated thread, macOS a `caffeinate -i -w <pid>` child; mode *agents* holds it while any
  non-shell session is working.
- **Terminal palette**: `daemon.set_theme` takes an optional palette (16 ANSI + fg/bg/cursor)
  so colour-query answers match the chosen terminal theme.
- **CLI** (`cli.rs`): `kelpied terminal list|read|send|wait`, `kelpied worktree get|set` and
  `kelpied review add|list` over the same RPCs.

## Features beyond sessions

| Area | Where | How |
|---|---|---|
| Layouts | `app/src/lib/layout.ts` | A tab is a tree of n-ary splits (`children`, `sizes`) with pane leaves; grids are built from `{mode: rows\|cols, counts}`; saved layouts are templates (tree shape + session kind per leaf). Version-2 binary trees migrate on load. |
| Accounts | `app/src-tauri/src/accounts.rs`, `lib/accounts.ts` | One config home per account under `<data>/accounts/<agent>/<id>`, passed as `CLAUDE_CONFIG_DIR` / `CODEX_HOME` to every launch (terminal, chat, automations, runs, one-shot AI actions); history and usage read each home. A stopped terminal session can move to another account (`account_transcript` copies the conversation, `session.set_env_restart` restarts it there; see "Notifications and limit actions"). |
| Orchestration | `crates/kelpied/src/runs.rs` | A run is a task DAG (cycle-checked) persisted in `runs.json`. `Runs::step` starts ready tasks (deps done, approval given) as ordinary sessions, optionally in a new worktree; tasks end on `kelpied run report`, session exit, or idle after working (default 300 s). A run may carry `materialize` (ignored files its worktrees get) and a task records its `base` branch and what it `brought`; see "Cross-review and best-of-N". |
| Integrations | `app/src-tauri/src/integrations.rs` | Linear GraphQL, Jira REST (Cloud v3 `search/jql`, Server v2), GitLab REST v4. Tokens live in the OS keyring (`keyring`, service `Kelpie (<endpoint>)`) and never reach the webview. |
| Browser panes | `app/src-tauri/src/browser.rs`, `browser_picker.js` | Tauri child webviews positioned over the pane (hidden while an overlay covers them). Design mode injects a picker; picks travel through a cancelled navigation to a sentinel URL, are redacted and size-capped, and the element screenshot uses the WebView2 DevTools protocol (end-to-end tests also send trusted page input through it, `browser_test_input`, dev / e2e builds only). |
| Remote (SSH) | `app/src-tauri/src/remote.rs`, `agents.rs` `build_remote_launch` | The system OpenSSH client (on Windows `System32\OpenSSH`, never a Git Bash one from PATH): sessions run `ssh -t host` with a login shell, inside `tmux new-session -A` when available; files, search and git run as remote commands with quoted arguments. Commands never prompt: secrets come from askpass, anything else fails. |
| SSH hosts | `app/src-tauri/src/ssh_hosts.rs`, `crates/kelpied/src/askpass.rs`, `app/src/components/hosts-page.tsx` | Saved hosts and forward rules in `<data>/ssh/hosts.json`, rendered into `<data>/ssh/config` (one `Host` per saved host, then `Match all` + `Include` of the user's config) that every Kelpie `ssh` / `scp` reads with `-F`. Passwords and key passphrases live in the OS keyring (service `Kelpie (<endpoint>)`); ssh gets them through `SSH_ASKPASS=kelpied` + `SSH_ASKPASS_REQUIRE=force` + `KELPIE_ASKPASS=<host id>` — `kelpied` answers password / passphrase prompts and declines host-key questions, which the page settles with `ssh-keyscan` fingerprints first. Connecting to a host opens a remote project on it. Port forwards are `ssh -N` children of the app (stopped with it; auto-start rules restart on launch); transfers are `scp -r -p` (SFTP). Keys, agent and known hosts go through `ssh-keygen` / `ssh-add` and the known_hosts file. `KELPIE_SSH_DIR` replaces `~/.ssh` for the tests. |
| Phone server | `crates/kelpied/src/mobile.rs`, `crates/kelpied/mobile/` | axum on a chosen address (default port 4817), optional rustls (ring). Pairing: a one-time code shown as a QR (2 min) and approved in the app; device tokens are stored as SHA-256 and compared in constant time; Origin checks, CSP, pairing attempts limited per source address; revoking closes the device's sockets. Protocol v2 (verbs, state sync, answers, chat): `mobile/verbs.rs`, `mobile/sync.rs`, `mobile/lanes.rs`, machine load in `load.rs`, and "Phone protocol (v2)". |
| Plugins | `app/src-tauri/src/plugins.rs`, `lib/plugins.ts` | Declarative manifests (see [plugins.md](plugins.md)); approval bound to the manifest SHA-256; agents are sent to the daemon (`agent.set_custom`, kept in `custom-agents.json`). |
| Agent setup | `app/src-tauri/src/agent_config.rs` (+ `agent_config/`), `lib/agent-setup.ts`, `components/agent-setup.tsx` | MCP servers, instruction files and skills of the agents, from their own files: Claude `~/.claude.json` / `.mcp.json`, Codex `config.toml`, Gemini `settings.json`, OpenCode `opencode.json(c)`, Cursor `.cursor/mcp.json`. One neutral `McpServer` with a codec per agent; reads happen only when the page opens. **One writer per file**: JSON and JSONC through a syntax tree (`jsonc-parser`: comments, order, indentation and unknown keys survive), TOML through `toml_edit` — the same writer `accounts.rs` uses to keep a Codex login in a file, so an MCP server copied into `config.toml` survives an account re-seed. **Trust**: a server found in a repository is `project` scope (untrusted); copying it to a user-level file needs an explicit confirmation, enforced by the app on apply, of everything it would run (argv per element, working folder, every env / header entry as the repository wrote it; relative paths are made absolute inside the project); none of its environment is carried unless opted in, and no reference to the user's variables is invented for it. A user's own environment and header values never reach the webview (names only), never a project file (they become `${VAR}`-style references in the destination's syntax, rewritten inside values too; Codex uses `env_vars`, `env_http_headers`, `bearer_token_env_var`), and are masked in every diff by position in the parsed tree. Project-scope paths (and `AGENTS.md` / `CLAUDE.md` / `GEMINI.md`, `.gemini/settings.json`, `.claude/settings.local.json`) are refused when a link or junction leads out of the project. **Writes** (one apply at a time): preview (a token over the files as read and as they would become, refused when stale) → per file, with Claude's own lock (`~/.claude.json.lock`, `mkdir`, stale after 10 s) held for `~/.claude.json`: compare with the previewed text → backup (`<data>/agent-config-backups`, private, newest 20 per file) → unique private temp file in the same folder (owner-only on unix, fsync) renamed over the target, retried and then refused, never rewritten in place → read back and parse. A write that is Kelpie's own and broken is put back (with the earlier files of the edit), but only while the file still holds what Kelpie wrote; a file another program saved since is left as it is and reported. The ACP registry (`agent_registry_fetch`) is one https GET per click (size- and time-capped); an entry becomes a custom agent (`npx` / `uvx`, an exact version; tags, ranges and URL or git specs are replaced by the registry's version or refused) that is stored in the settings and sent to the daemon with the plugin agents (`agent.set_custom`, the user's capability); it has no terminal interface, so the app starts it as a chat only. For tests only (`KELPIE_E2E`): `KELPIE_AGENT_CONFIG_HOME` replaces the user's home for every lookup (without it an end-to-end run cannot write), `KELPIE_ACP_REGISTRY_URL` moves the registry (plain http to 127.0.0.1 / localhost only), and a marker file `agent-setup-corrupt-next` corrupts the next write. |
| Voice | `crates/kelpie-stt`, `app/src-tauri/src/voice.rs`, `lib/voice.ts` | A separate helper process: sherpa-onnx (static, offline) with a Vietnamese zipformer or Whisper; `cpal` microphone capture mixed to mono; JSON lines over stdin/stdout; models downloaded on request and verified by pinned SHA-256. Kept out of the app and daemon so the native engine can never take them down. |
| Language | `app/src/lib/i18n.ts`, `app/src/locales/vi/*.json` | `t("English text")` looks the text up in the Vietnamese dictionaries (English is the key and the fallback); reactive on the language setting; dates through `Intl`. `pnpm i18n:check` reports missing translations. |

## Measured (Windows 10, see `plans/reports/`)

- 20 sessions × 2 MiB output: 40 MiB in 3.9 s through the daemon, 0 sequence gaps.
- Attach (snapshot) p50/p95: 4.4 / 5.8 ms.
- Project switch (3 projects with terminals): release p50 ≈ 30 ms, p95 40–52 ms; dev build p95 35–105 ms.
- Session history, 421 transcripts / 1.2 GB (release build): first scan 0.5 s, later launches
  (disk cache) 34 ms; the first full prompt count runs in the background (~15–30 s).
- Event classification (`Notifier::observe`, 20 sessions cycling working → waiting → idle,
  `cargo test -p kelpied classification_costs -- --nocapture`): 1.2 µs per status change in a debug
  build (budget 100 µs). With no channel switched on nothing else runs for a status change.
