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
| `Kelpie` | while the window is open | layout, settings (`app-state.json`), saved chat replies (`chat-bookmarks.json`), views |
| `kelpied` | until stopped explicitly (or logoff) | PTYs, agent processes, terminal state, `sessions.json`, snapshots |
| `kelpie-stt` | while dictation is used (exits after 10 idle minutes) | the loaded speech model, the microphone stream |
| agent CLIs | per session | — |

`app-state.json` carries a format `version` (currently 3: n-ary pane splits). A file with an older version, an
unknown format or one that cannot be read is first copied to
`app-state.<label>-<time>.bak.json`; if that copy fails, or the file comes from a newer Kelpie,
the app runs without saving rather than overwrite it.

`chat-bookmarks.json` holds saved replies with their full text, so they outlast their session. Every
change is applied by a native command to the file as it is at that moment and written atomically, so
another running Kelpie's changes, a restored copy and entries this build does not understand all stay.
A file that cannot be read is an error: it stays byte-identical, the feature is read-only until the next
start, and the user is told once.

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

Data: `%LOCALAPPDATA%\Kelpie` (Windows), `~/Library/Application Support/Kelpie` (macOS),
`$XDG_DATA_HOME/Kelpie`, else `~/.local/share/Kelpie` (Linux; an empty or relative `XDG_DATA_HOME`
is ignored, as the XDG spec says). `KELPIE_DATA_DIR` and `KELPIE_ENDPOINT` override both (used by
the dev scripts and tests).

### Platforms

| | Windows | macOS | Linux |
|---|---|---|---|
| Data folder | `%LOCALAPPDATA%\Kelpie` | `~/Library/Application Support/Kelpie`, 0700 | `$XDG_DATA_HOME/Kelpie` or `~/.local/share/Kelpie`, 0700 (`ensure_private_dir`; an existing folder of ours is narrowed) |
| Daemon endpoint | named pipe `\\.\pipe\kelpie-<user>-v1` | `kelpied-v1.sock` (0600) in the data folder | same as macOS |
| Daemon runs from | a copy in `<data>/daemon/a` or `b` (release builds) | the app bundle | the install (`/usr/bin`, .deb) or, from an AppImage (whose mount ends with the app), a copy in `<data>/daemon/a`, rewritten for each new build (a running daemon keeps its unlinked file; `b` only when `a` cannot be written). The daemon, and every program the app starts (editors, `xdg-open`, git, ssh, user commands), gets the app's environment without the AppImage runtime and GTK hook variables (`daemon_runtime::clean_env`), so sessions and those programs never see the mount |
| Session processes | Job Object per session, sideloaded ConPTY | process groups, `killpg` | same as macOS |
| Verifying the person (phone access) | Windows Hello, else the password prompt; text is Kelpie's | Touch ID or the login password; text is Kelpie's | polkit (`pkcheck`); text is polkit's own. Refused with no polkit or no agent. Not yet run on a real desktop |
| Agent PATH | PATH + PATHEXT, npm shims unwrapped | login shell: `$SHELL -l -c 'exec …'` | same as macOS; without `SHELL`, the account's shell from the user database, else `/bin/sh` |
| Keep awake | `SetThreadExecutionState` | `caffeinate -i -w <daemon pid>` | `systemd-inhibit --what=idle:sleep --mode=block sleep infinity` (whichever lock a desktop's automatic suspend honours; a menu suspend waits, the lid follows logind's `LidSwitchIgnoreInhibited`), `PR_SET_PDEATHSIG` ties it to the daemon; none without systemd (logged, also when logind refuses later) |
| Secrets (`keyring`) | Credential Manager | Keychain | Secret Service over D-Bus (`sync-secret-service`, `crypto-rust`); no service = an error naming what to install, never a fallback |
| Notifications | `tauri-plugin-notification` | same | same (freedesktop notifications over D-Bus) |
| Packages | NSIS (`-setup.exe`) | `.app` + `.dmg`, universal | AppImage + `.deb`, built on Ubuntu 22.04 |
| Updater entry (`latest.json`) | `windows-x86_64` | `darwin-aarch64`, `darwin-x86_64` | `linux-x86_64` (the `.AppImage` + `.sig`); `linux-x86_64-deb` when the bundler signed the `.deb` |
| CI | `windows-latest` | `macos-latest` | `ubuntu-22.04`: tests, package checks (`dpkg-deb`, AppImage contents), `scripts/e2e/boot_smoke.py` under Xvfb with WebKitWebDriver |

Daemon state is published through unique same-directory temporary files. Windows uses
`FileRenameInfoEx` with POSIX replacement semantics: existing readers keep their old file
while a newly opened reader sees the replacement
([Windows contract](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)).
Unix keeps atomic rename. A publication error is returned without deleting the destination;
only the failed write's own temporary is cleaned. Alternate filesystems and redirected or
network data directories require separate validation of the native operation.

## Transport

Local socket only — no TCP port. Windows: named pipe `\\.\pipe\kelpie-<user>-v1` with a
protected DACL (SYSTEM + owner). macOS and Linux: `kelpied-v1.sock` (mode 0600) in the data dir.
Single instance is enforced with an exclusive lock file.

**Callers** (`crates/kelpied/src/auth.rs`). A connection names itself with `auth` before
anything but `daemon.info` answers:
- the **user token**, written at start to `<data base>/Kelpie/auth/<endpoint>.token` (owner-only,
  new per daemon run, keyed by endpoint so a dev app and dev daemon agree): the app, `kelpied`
  outside a session, `scripts/kelpie-call.py`. Full access; raw Input frames are accepted from it only.
- the **verified app** (`Role::App`): the user token, `client: "app"`, *and* the connection's peer process runs the Kelpie
  app's executable (`crates/kelpied/src/peer_identity.rs`: `kelpie` next to the daemon, or in the folder named by a runtime slot's
  `source.txt`, which an AppImage rewrites at every start because its mount moves while the daemon lives; on macOS a path check only, no code-signature check, a known weakening to fix or accept before macOS ships). Only
  it may ask for the phone decisions `mobile.approve`, `mobile.set_permission`, `mobile.set_write`, `mobile.reset`, `mobile.set_tailscale`
  (`auth::APP_ONLY`), and for a `mobile.set_options` that keeps unused phones longer. Those that raise access take effect only
  after **the daemon itself** asked the operating system to verify the person (`crates/kelpied/src/user_presence.rs`: Windows
  Hello, else the Windows password prompt checked with `LogonUserW` and required to be this user's account; macOS Touch ID or the
  login password; Linux polkit, by running `pkcheck --action-id org.freedesktop.policykit.exec --process <own pid>
  --allow-user-interaction`, which runs nothing as root). That is the gate. **On Linux polkit's agent shows polkit's own generic text, not the daemon's: it
  cannot say which phone this is about.** The app's approval dialog names the device, its key and what it asks for before the
  prompt comes up, so that is what the person reads. That action's default asks for an administrator's password, so an account that is
  not an administrator cannot pass, and a distribution's polkit rules can change what it asks. With no polkit, or no agent to ask through
  (a headless login), it is refused with a message saying what to do, which does not count as the person refusing; past the time limit `pkcheck` is ended. What `pkcheck` wrote to stderr goes to the debug log. This has not yet run on a real Linux desktop. Declining a phone and lowering a permission ask nobody; `mobile.reset` always asks. What
  the prompt says is built by the daemon from its own records, with a phone's name in quotes; a caller supplies no text. The
  prompt runs on a thread of its own (`Daemon::after_verifying`), the request is answered when it ends, one prompt at a time, for at
  most 280 s: then the caller gets an error, Hello and macOS's prompt are cancelled, a password dialog (which cannot be) is ignored, and whatever it says
  later can no longer do anything. While such a dialog is still up a new prompt is refused ("close the earlier dialog first"), so a person
  never faces a stack of them (should no such dialog exist, a thread that never ended, the message says to restart the daemon; a prompt up past 10 minutes is an audit line, `verify_stuck`). Each refused or failed verification is an audit line (`verify_failed`, with what it was about);
  three within 5 minutes, or three left unanswered within 20, stop new prompts for 5 minutes (`verify_paused`), in memory only, so a
  restart of the daemon ends the pause. What the daemon tells the person is English, translated by the app (`locales/vi/backend.json`).
  Prompts are written in English and Vietnamese in the daemon's own table (`Prompt::text`); the app sends a language code, nothing else
  of the text. **Open:** whether Windows Hello's face recognition passes with no deliberate act has not been tried; if it does, a
  person looking at the screen would approve whatever is asked. In the
  app `mobile_secure_call` lets the daemon's dialog come to the front (`AllowSetForegroundWindow`) and passes the window as a
  hint for parenting it, which the daemon checks (`IsWindow`). **On Windows the app's window is locked while the dialog is up,** as a window
  behind its dialog is: it takes no click or key, while requests and sessions keep going. The app enables it again as soon as the call
  ends, however it ends (an answer, an error, the time limit, or the daemon going away, which ends the call at once), so it is never left
  locked. `daemon_call` refuses the app-only methods. A debug build also
  trusts `KELPIE_TRUSTED_APP` and answers verifications from `KELPIE_TEST_USER_PRESENCE` (both read by the daemon); a release build has
  neither path, and unit tests answer through an injected decision and refuse when there is none, so they cannot raise a real
  prompt. `mobile.revoke_all` only removes access, so the plain user token may call it with no verification.
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
what agents do through Kelpie by default; it is not a boundary against deliberate same-user code. For phone access the gate is the
OS verification in the daemon. `Role::App` is a speed bump in front of it: writing a file where `peer_identity` looks and running
it defeats the role, and all that gains is the right to raise a prompt with the daemon's text, which the person can refuse.
The verification stops a program that asks the daemon, and nothing more: deliberate same-user code can go around it without
raising a prompt or leaving an audit line. **The file route:** a program of this user can read `mobile-device-key` from the OS
credential store (it has no access control within the user), write a correctly sealed `write` record into `mobile.v2.json`, and
have the daemon restart with the user token, which then loads that record. `dormant_days` in the same file is outside any MAC, so
it can simply be edited. It can also replace `kelpied.exe` on disk (the daemon's runtime slot and the install folder are the
user's to write). These are accepted: the OS gives one user's programs no boundary between them.

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

RPC methods: `auth`, `daemon.info|set_theme|set_awake|set_priority|policy|set_policy|shutdown`, `session.list|create|attach|detach|input|answer|resize|focus|kill|close|restart|update|read|set_unread`, `chat.close_stdin`, `chat.send|write_turn|queue_op` (API 21: the chat's message queue, below), `hook.report`, `agent.presets|run|set_custom`, `meta.list|get|set`, `automation.list|state|save|delete|run|approve|set_paused|event` (API 15 for the last four and `state`), `run.list|get|create|approve|retry|task_done|task_skip|cancel|delete|report`, `note.add|list` (sessions: own worktree only) and `note.clear` (the user's) since API 13, `mobile.status|enable|disable|pair_start|approve|revoke|revoke_all|set_permission|set_write|set_options|audit|reset|set_projects` (API 22 for `revoke_all`, `set_permission`, `set_options`, `audit`, `reset`), `mobile.load` (API 17: the machine-load level, lag percentiles; `{reset: true}` restarts the percentiles), and since API 12 `ui.focus`, `notify.get|set|test|rotate_topic|clear|report`, `session.schedule_continue|cancel_scheduled|scheduled|set_env_restart` (all the user's: no session token reaches them), and since API 16 `process.list|ports|port_owner|kill|kill_children` (the user's only; see "Processes and ports" under "Daemon services beyond sessions").
`daemon.info` reports an `api` feature level (`kelpie_proto::API_LEVEL`); an app built for a
higher level offers to restart an older daemon still running from a previous version.
Events: `session.updated` (coalesced, 250 ms), `session.turn` (API 9: `{session, event, transcript_path?, agent_session_id?}`, from every agent hook and when a session leaves `working`; built by the daemon from the hook payload, so no program can publish one — the app reads a session's usage on it, see "Context, tokens and cost"), `session.closed`, `session.resync`, `session.clipboard`, `meta.updated`, `notes.changed` (API 13: `{path, notes}`, a worktree's whole list), `automation.ran` (API 15 adds `dry_run` and `note`), `automation.changed`, `run.updated`, `mobile.pair_request` (API 22 adds `fingerprint`, `want`, `address`), `mobile.device_changed` and `mobile.lockout` (API 22: a phone was added, changed or removed; too many failed attempts, `{reason, address?}`), `daemon.unresponsive` (API 17: `{seconds}`, the event loop has been silent for 30 s; sent by the watchdog, delivered when the loop runs again), `chat.resolved` (API 17: `{handle, id, request_id, by, dropped, answered_by, at}`, to the clients attached to a chat when a request of its agent is answered; see "Phone protocol (v2)"), `chat.queue` and `chat.turn_ready` (API 21: the chat's message queue and the offer of its next message to the window that queued it; see "Chat mode"), and since API 12 `notify.event` and `scheduled.updated` (see "Notifications and limit actions").

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
(see `third_party/conpty`). macOS and Linux: process groups (`killpg`) and a login shell (`$SHELL -l -c
exec …`) so agents see the user's PATH (GUI-launched apps do not inherit it).

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

**Destinations** (`notify/destinations.rs`). Each channel that is on and each phone with alerts on has its own bounded queue (32), thread and outbox, started by its first message and ended with it, so one that hangs delays nothing else. `Notifier::active` counts subscribed phones too; with no destination there is no thread, socket or timer. The classifier's thread only hands the message over (`Notifier::send`, under a millisecond), and the rest happens on the destination's thread.

**Outbox** (`notify/outbox.rs`, `<data>/notify-outbox/`, written atomically, at most 50 entries, an older daemon ignores the folder). Before every attempt each entry is looked at again: a wait the person answered goes without a word, and an approval is asked of the run. Failures on the way there (name, route, connection or TLS, and a timeout in resolving or connecting, each with 5 s of its own), 5xx and 429 retry after 1, 2, 4, 8, 16, 32, then 60 s with full jitter, and at least `Retry-After`; other 4xx do not retry. A send that ended without a word (any other timeout, a connection cut) may have reached the receiver, which acts on each one: a channel gets it twice at most, the second at least 3 s later, what it did before the outbox, and then it is dropped and counted; a phone goes on, since its tries take each other's place. A webhook's 5xx other than 502 and 503 counts the same: a flow can act and then fail. An entry goes at 10 minutes, unless it says someone still waits. After a real outage (three failures in a row, or what is ready a minute old) several entries leave as one message ("N sessions need you, and M more updates"), and one failed attempt merges nothing. A webhook's `event` carries `id`, the same on every try, and for a message that stands for several, `status` `needs_you` when someone waits, `failed` when something in it failed, else `summary`. At load L1 a phone's entries about what wants no answer (finished, automation, cost) wait 1 s to leave with the others, from L2 5 s; needs-you, failed and approval are never held, and a channel's messages never are, so each keeps its title, text and link. A channel's entry holds its redacted text until it is sent; a phone's holds none. A file it cannot use is moved aside; one it cannot read is not written over.

**Web Push** (`notify/web_push.rs`, API 24). The daemon is its own application server: no server of ours, no Apple or Google account. `push_info` makes the VAPID key on first need, kept with `kelpie_proto::secret_*` (`mobile-push-vapid`; a reset makes a new one; a failed read is an error and makes none), and `hello` carries its public half on a connection that may have alerts. `push_sub {endpoint, p256dh, auth, lang}` stores the subscription on the device record with its own HMAC, and `auth` in the credential store (`mobile-push-auth-<device>`). An alert is `{web_push: 8030, notification: {title: "Kelpie", body, navigate, tag}}`: Declarative Web Push, which iOS 18.4 and later shows by itself and `sw.ts` shows elsewhere. `body` is a fixed sentence per kind (EN or VI from `lang`), `navigate` is `<page origin>/#s=<session id>` (with `&a=1` when someone was waited for: a tap then says when that is over), the origin being the one the page named in `push_sub`, refused unless it is the `Origin` of the connection it came over (which the route has checked against `Host`, and which on a TLS listener the browser has checked against the certificate) and kept under the record's HMAC, since that is where the phone's pairing is. A name counts as well as an address, so a certificate of the person's own works. A subscription from before this has none and uses the running address, and waits rather than naming this network's address while Tailscale mode is on with its origin not back. `tag` and `Topic` name the kind, the session and the wait, so only the same thing replaces itself; a summary and a test alert are each their own, and nothing else from a session is ever an input to it. It is encrypted to the subscription (RFC 8291, `aes128gcm`) and signed (RFC 8292: `aud` the endpoint's origin, 12 h, `sub` the project's address, no person's), and sent with `TTL: 600`, `Urgency: high` for what waits or failed, and `Topic`. The RustCrypto crates do this, since the `web-push` crate needs OpenSSL through `ece`.

The endpoint comes from a phone and the daemon POSTs to it, so it is held to a closed list: `https`, no port, no login, `fcm.googleapis.com`, `*.push.apple.com`, `*.push.services.mozilla.com`, `*.notify.windows.com` (the last two from the browsers' documentation, not yet seen from a real phone), no redirects. It is checked when it arrives and again before every request, and no error says it back. Only with `KELPIE_E2E` set may `KELPIE_PUSH_TEST_BASE` name one loopback address besides. A 404 or 410 ends the subscription; a revoke, reset, expiry or the kill switch ends it before returning, with its secret and what it had left to send; phone access off stops sending and keeps it for when access is back. The browser's side is looked at every time the app opens, and renewed with no tap once permission is given. A failed read of `notify.json`, the device store or its key deletes no outbox file at the start. A phone may change its subscription 10 times a minute.

A computer that is asleep sends nothing, and a computer that lost contact cannot say so: that needs someone else's server, and stays deferred. Real delivery to phones is not yet measured.

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
`KELPIE_LOG` says: its trace output contains the request path, a Telegram bot token among it). What is dropped
(a full queue, an outbox past its limit, an entry too old, one tried twice with no answer) is counted and shown in the
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
(`/link`, kelpie-link binary messages carrying the JSON messages below; see "Phone link") is registered as a daemon client
through an adapter and may use only the verbs below. `session.update`, `session.attach`, closing, phone settings and the
other admin methods are not verbs, so naming them reaches nothing.

The internal adapter is explicitly registered as a trusted user client; the phone itself
still has only the closed verb table below. Each server generation owns a sticky shutdown
signal shared by the listener, accepted HTTP/TLS connections and upgraded WebSockets,
including authentication. Disabling access revokes current permissions and cancels that
generation's connections; enabling it again cannot revive an old socket. Paired device
records remain available for a new connection, and a phone that was connected keeps its pairing and connects again when it can.
Turning access off is therefore not how to cut a phone off: `mobile.enable` stays open to any holder of the user token (audited,
`phone_access_on`), and every paired phone then has its access back with no prompt. `mobile.revoke` and `mobile.revoke_all` remove it.

- **Verb table** (`mobile/verbs.rs`, closed; least permission each): `list`, `hb`, `ack`, `sub`, `unsub`, `chat_sub`, and
  `push_info`, `push_sub`, `push_unsub`, `push_test` (refused `insecure` where `transport_cap` would not let a page type), and
  `lock_challenge`, `lock_enrol`, `lock_unlock`, `lock_now`, `lock_touch` need
  *read*; `input`, `key`, `answer`, `chat_answer`, `chat_send`, `chat_cancel`, `chat_stop`, `chat_resume` need *write*;
  anything else is `unknown_verb`. The
  permission is read from the device's record on every message, so a revoked or downgraded device is
  cut off at once, and the lock is checked before anything else (see "App lock"). A request may carry `req` (echoed in its `ack` or `error {code, reason?, message}`)
  and, for write verbs, the `epoch` it was made under (refused as `epoch` after a daemon restart).
- **Addressing and epoch**: sessions are addressed by `spec.id`. Handles restart at 1 and are reassigned
  by a restart, so a write by handle could land in another session. `hello {v: 2, api, daemon_epoch, rev,
  device, sessions, projects, locked, lock}`; `daemon_epoch` is random per daemon start, and a phone that sees it
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
  older ones when a frame names a later `base` (a whole frame lets nothing go: the daemon may have heard only an earlier
  acknowledgement; at most 8 screens are held, the first to arrive going first, twice what the daemon may send past what it has heard). At most 4 frames / 128 KiB wait for an acknowledgement per
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
  4409 (the page then waits until it is looked at again). Close codes: 4401 the device was removed, always after the sealed
  message `{type: "revoked"}` (see "Phone link": only that message makes a phone forget its pairing); 4408 connect again, the device stays
  paired (its permission changed, phone access was turned off, the connection fell behind the daemon's events and may have
  missed a removal, which the next connect finds out, or it has been up for 24 h); 4409 replaced, also when the same key was paired
again: the record is one per key, and the browser that paired again uses the key it holds for that computer, so nothing still using the old
record (another tab) is told to forget a pairing that stands. A close frame that cannot leave
  within 5 s is given up on. Rows are compared by text.
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
  (A line too long for one phone message once it is JSON-escaped, 1 MiB, arrives as a `kelpie_stderr` notice at the same place in
  the log.) `chat_answer {id, request_id, line}` answers a question the agent put to the person and nobody has
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
- **Sending to a chat** (API 21; the queue itself is under "Chat mode"): `chat_send {id, cid, text}` queues a message of at most
  16 KiB (bytes); the reply is `ack {result: ok, qid, state: queued|sent, ahead}`, or `ack {result: dup, qid, state}` for a `cid`
  the chat remembers (10 minutes, per device; `state` is `queued`, `sent`, `cancelled` or `dropped`), or an `error` with one of
  `read_only`, `epoch`, `not_chat`, `no_session`, `exited`, `switching`, `empty`, `too_long`, `rate_limited` (one a second per
  device, bursts of three), `queue_full` (20 in a chat, 5 from one device) and `command_needs_desktop` (a Codex `/command` or
  `!shell`, or an ACP `/new`, `/clear`, `/reset`: the desktop's client runs them). `chat_cancel {id, qid}` takes a message that still
  waits out of the queue, whoever queued it (`too_late` once it was offered to its builder or written); `chat_stop {id}` stops the turn
  (`result: noop` when nothing runs); `chat_resume {id}` ends the wait a stop asked for. The phone is told the queue as
  `chat_queue {id, items: [{qid, cid, by, mine, kind, text}], paused, held, in_flight, dropped?}`: the whole state each time, once
  after `chat_sub` and on every change. Revoking a device, or taking its write permission away, drops what it has waiting
  (`dropped` with the reason `revoked`); a turn already written runs on.
- **One-shot approvals**: a phone's answer holds for this one request. For a chat the daemon keeps a list of the answer shapes
  known to be one-shot for each kind of request (`chat::answer_is_one_shot`: Claude's allow or deny, Codex's `accept`, `decline`,
  `cancel` or a permission grant for this `turn`, ACP's `allow_once` and `reject_once`, a form's answers) and refuses anything else as
  `one_shot`, so a new field or decision word of an agent is the computer's until it is known. A terminal menu's lasting choice is
  refused by key, by the item's letter or exact text (`Prompt::lasting`, from the whole label in `choices.rs`, wrapped lines included),
  and while one is on screen `answer` takes only `esc`, a listed key that does not last, or Enter on a marker known to be on one (`Prompt::plain_answer`).
  The lines above a menu count too: the yes of `Always allow this tool?` or of a question of trust lasts. A question that names
  permission is one on its own; a tool's line, going on with something, or an action in a plainly asked question (not one whose choices
  carry descriptions) need a word of consent as well. There, whatever is not plainly once or no counts as
  lasting; on any other question only the words that last do, so an agent's own `Yes` / `No` question stays answerable. The phone
  hides those options. This is a convenience on what a person sees, not a boundary, and recognition is best effort (a table of real labels pins it, `choices::real_labels`): a term in a description with no word that lasts, a scope named only in the question (`Allow this for this project?`), a wrap at a capital, or a new agent's wording can still leave a button. **Residual:** a phone
  with *write* can still type in a terminal, so it could reach such a choice with arrow keys and Enter, press `shift-tab` (one key
  that changes Claude Code's permission mode for the session), or run `/permissions`.
  Typing is a shell; the daemon does not try to judge it.

### Phone link

`crates/kelpie-link` is the end-to-end protocol, sans I/O: Noise `IKpsk1` to pair and `IK` to reconnect (25519, ChaChaPoly,
BLAKE2s; `snow` with RustCrypto backends, no `ring`, which CI checks). The phone's half is hand-written on noble
(`app/phone/src/noise.ts`) because `crypto.subtle` does not exist on a plain `http://` page; `scripts/e2e/link_noise.py` is a
third implementation for the tests, and all three are checked against the crate's vectors and the published cacophony ones.

- **Wire** (`GET /link`, `mobile/link.rs`): binary WebSocket messages, no compression (a `permessage-deflate` offer is declined),
  `TCP_NODELAY`. The first message is `version, mode, mac1 (16 bytes), Noise message 1`; the reply is Noise message 2, and carries
  nothing. After that, each message is a sealed chunk with a flags byte (a JSON message may span several; at most 1 MiB).
  Implicit counters, no reorder window: a message that does not decrypt ends the connection. Keys are rekeyed every 2^16 messages
  each way by counting, and a connection handshakes again after 24 h (4408); there is no time-based rekey, an accepted decision: a
  reconnect is a fresh handshake anyway, and a handshake every 30 minutes would interrupt screens for little gain. Every reconnect
  is a new session. A message too long to seal is reported in the daemon's log and dropped, and the session carries on.
  Frame numbers are per daemon run and never reused, however often a slot comes and goes.
- **What a first message costs**: a connection counts against 16 handshakes pending, 4 of them from one address (this machine has
  no such share), from its upgrade until its first valid sealed frame or 5 s, and carries one handshake. When all 16 are taken the
  oldest that has sent no valid first message gives way to the newcomer, and a phone in the middle of its handshake is never let go. A first
  message seen in the last 2 minutes (256 kept, by hash) is dropped before it could hold a place: a phone's is new every time, since
  it carries a fresh ephemeral key, so a repeat is someone replaying what they saw on plain HTTP. What holds a place for nothing counts as a
  failed attempt of its address: saying nothing until those 5 s are up, being let go having said nothing, a repeat, or passing the check
  and never sending a sealed frame. If every place is held by connections past the check, a newcomer is turned away. An address may also make
  only 5 upgrades a second (bursts of 20, refused with 429 before any task
  exists) and take only 5 of the machine's handshakes a second. Several addresses together can still deny service on a LAN, and that leaves a
  trace: a flood that goes on (20 refused or let go in a minute, the budget running out among them) is an audit line (`flood`) at most every 5 minutes. This machine has no share of any of
  these; what comes through Tailscale Serve never counts as this machine (see "Tailscale mode"). `mac1` (keyed from the desktop's public key, or the pairing secret) is checked
  before any Diffie-Hellman; only then does it take one of 20 handshakes a second. What fails these is dropped with no reply, no
  close code, no event and no count, so a wrong or spent pairing code gets silence. A first message can be a replay, so **nothing
  happens until the phone's first valid sealed frame**.
- **Pairing**: the QR link holds the desktop's public key, a one-time secret and the endpoints (`#p=`, never sent in a request).
  A code answers 5 first messages per address and is spent at the first sealed frame (`{type: "pair", name, want}`), so a replay cannot
  burn it. The desktop then gets `mobile.pair_request`; the phone shows six digits derived from the handshake hash
  (`HKDF(h, "kelpie-sas")`) and its key's fingerprint. The person types the digits into the verified app; a wrong code is refused at
  once with no prompt and three of them decline the request. With the right one the daemon asks the OS to verify the person and, when
  that passes and the request is still there, adds the device; a failed verification leaves the request waiting until it times out.
  The phone hears `paired`, `declined` or `timeout` over the encrypted channel, then connects
  again as a device. Close codes before a session: 4403 a mac-valid first message that fails Noise, 4429 too many failures from
  the address (10 a minute: mac-valid failures and what holds a place for nothing, as listed under "What a first message costs"; `mobile.lockout`) or reconnects of the device (5 in 10 s), 4503 busy (at L2/L3 a pairing
  handshake within 1 s / 5 s of the last, without spending its code), 4401 this computer does not know the key, 4402 it knows the key
  and cannot confirm the record now (not counted as a failure).
- **Reconnect**: the phone's static key must be a paired device's, and its first sealed frame is `{caps}`.
- **Being removed**: WebSocket close frames are not authenticated, so a phone forgets its pairing (and its key, cache and drafts) only on
  the sealed `{type: "revoked"}` inside a session. A 4401 in a session with no such message means connect again. A 4401 or 4402 before a
  session shows "not paired here" with a Forget button and keeps the key; the phone asks again once a minute and whenever the page is
  looked at, so a forged answer or a passing failure on the computer costs nothing. A phone removed while it was away sees that state
  and is forgotten by the person. After `mobile.reset` the desktop has a new key, so an old phone's first message fails its `mac1` and
  is dropped with no code at all: after 3 such closes in a row while the computer's page still answers, the phone shows the same state
  with "phone access may have been reset, or something on this network is interfering" (a guess, so the key stays; it asks again once a minute).
- **Device store** (`mobile/store.rs`, `mobile.v2.json`): each device is its Noise public key, name, permission and kind, MAC'd
  with a key from the OS credential store, which also holds the desktop's link key. A record whose MAC fails is `unverified`: it
  gets no access and must be paired again. This defends against file tampering only: same-user code can read the keyring. A key is
  made only when the credential store says it has none (`kelpie_proto::secret_read`): a read that fails or a value that is no key is an error,
  and nothing is written over it. While the key cannot be read records are `unchecked`, with no access and no conclusion, and are
  verified again when it can be (when the app looks and when such a phone connects). A file that
  cannot be read is copied aside and never overwritten; phone access stays off until `mobile.reset`. A change that cannot be written
  (a full disk) holds in memory and is tried again every 10 s; a removal or turning off says so ("removed for now, it will come back after a
  restart unless this is fixed", in `mobile.status`'s `error`, audit line `save_failed`), and so does lowering a permission, until it could be written. Phones paired with a token
  before this are listed as `legacy` and must be paired again. Devices not seen for `dormant_days` (default 90, 0 never) are removed;
  lengthening that needs the verified app and the person, shortening it does not.
- **Permissions**: `read` < `write` < `control`, capped by the transport: a browser page may type only over HTTPS (Tailscale Serve's included), from a Tailscale
  address or from this machine, because a plain `http://` page can be swapped by someone on the network. Until a native app exists
  `mobile.pair_start` makes browser pairings only.
- **Tailscale mode** (`mobile/tailscale.rs`, off by default): `tailscale serve --bg --https=8443 http://127.0.0.1:<port>` publishes the
  page at `https://<fqdn>:8443` to the person's own tailnet, and forwards to one loopback listener that exists only while the mode is on and
  follows the same `Generation` stop as the others, with its own for what came through it, so turning it off cuts those phones and no
  others. The port is fixed because the origin holds what the phone stores. `listen::wanted` still never binds loopback.
  - `mobile.tailscale_status` runs `tailscale status --json` and `serve status --json` and answers `state` (`not_installed`, `stopped`, `needs_login`,
    `https_off`, `unreadable`, `ready`) and `serve` (`off`, `ours`, `missing`, `left_behind`, `port_busy`, `not_listening`, `renamed`, `taken`, `unknown`). The CLI runs off the runtime, 5 s at most,
    killed past that, and only when asked, while the mode is on and the machine's addresses change, or when `mobile.enable` or a daemon start finds an entry still to go: no polling. Output that cannot be read is a
    state of its own, and nothing is changed on it. A failed command's own words are never passed on.
  - `mobile.set_tailscale {on}` is in `auth::APP_ONLY`. On is checked with no prompt, then verified (`Prompt::Tailscale`), then checked again; off asks nobody.
  - **Never clobbers**: only one handler `/` proxying to `http://127.0.0.1:<the port the store remembers>`, not open to the internet, is ours. Off removes only that, with `tailscale
    serve --https=8443 off`. `serve reset` is never used. A daemon that starts again only looks: it republishes nothing, so an entry removed by hand stays removed, is
    reported as `missing`, and comes back through the verified path. If its port has been taken by another program it removes its entry, which would lead there.
  - **`mobile.disable`, `mobile.revoke_all` and `mobile.reset` end the mode and remove the entry**, also when one of them comes while a turn-on is under way or at its prompt: it leaves word under the config lock and does not wait, and the turn-on's last step finds it and undoes what it wrote. So `mobile.enable`, which any holder of the user token may call,
    never publishes anything. An entry Tailscale will not remove yet is kept in the store (`tailscale_remove`), shown in Settings even with phone access off, and removed the next time Tailscale is looked at or phone access or the daemon starts. No timer runs for it.
  - **With the mode on the listener does not depend on this network's**: a busy port, a pinned address or no address at all are reported, and the server goes on.
  - **A MagicDNS rename**: an entry under another name is `renamed`, not `ours`. No origin is advertised and Restore publishes under the new name. That tailscaled keeps entries by name, and that `serve --https=8443 off` acts only on the name of now, are inferred, not seen: a rename was not tried on the real tailnet. If so, Restore leaves the old entry beside the new one, and turning off takes only the new one. An entry counts as removed only once a second read finds the port free, so Kelpie then keeps its port, reports `left_behind` and keeps trying, and it must be removed with Tailscale's own tools. Restore refuses to put a second entry on a second port.
  - **Limit: a machine with more than one OS user.** The serve entry outlives the daemon: `tailscaled` is a service, and it keeps forwarding to `127.0.0.1:<port>` after a crash, a logoff or a shutdown, or while the store cannot be read. While nothing of ours holds that port, **another local user can bind it and answer at the phone's trusted origin**, with Tailscale's valid certificate in front. The installed app's service worker accepts that answer, so that user's script runs at the origin and can read the phone's Noise key from IndexedDB, and with it later reach this daemon as that device, up to its permission. A 502 while nobody holds the port is the good case, not the only one. A daemon that starts and finds the port taken removes its entry, which closes the window if `off` works, and does not undo a key already read; if it fails, Settings warns that phones may be reaching that program (`port_busy`). A daemon that cannot get that far (a bad certificate, a credential store that will not answer) does not look. The window is also open while a removal is still pending after phone access went off. On a machine with one OS user this does not apply: same-user code is out of scope (`auth.rs`). Settings does not state this limit. Decided on 2026-11-01: the limit stays documented, with no further guard, for a machine with one OS user. If shared machines come into scope the ways out are removing the entry on a clean shutdown, or `tailscale cert` with a listener on the Tailscale address, where a squatter has no valid certificate.
  - **Limits**: on this listener alone, `Peer` takes the client from the last value of `X-Forwarded-For`, and only a Tailscale address (serve replaces whatever a client sent; seen on 1.98). Missing, garbled or any other
    address falls into one shared bucket, `listen::BEHIND_SERVE`, which is not loopback, so nothing through serve has this machine's exemption. The header means nothing on any other listener. All upgrades through serve also share a budget of 20 a second, bursts of 40, spent only on what the address's own limit lets through, so one real address, which is all serve gives a tailnet device, cannot empty it. Several devices together can.
  - **The header is only as good as the programs on this machine**, which can connect to that port and say what they like. That gets no session: reconnecting takes a paired key, and pairing takes a live secret, the typed code and the OS prompt. The same is true of "From address" in the approval dialog. **What this comes to, without any key, for as long as the program runs: it can keep phones from connecting.** It can refuse every phone's new connection by keeping the budget spent, naming addresses it makes up (phones already connected stay). It can keep out one phone, whose Tailscale address is no secret, by holding that address's four handshake places (four connections every 5 s, with no lockout at all), or by renewing its lockout, which takes about one failure every 6 s: a lockout lasts a minute from the failures that made it, and they age out one by one. Past 1,024 addresses the table of failures starts over, which also wipes real lockouts. The desktop hears of a lockout, and of a flood only through an audit line every 5 minutes. This is accepted for now because nothing tells `tailscaled` from another program on that port and none of it gives access, but the reason differs by who it is: same-user code is out of scope (`auth.rs`), which does not cover another OS user, who can reach loopback too, or a tailnet device, which is not local at all. For another OS user that falls under the decision on the limit above. Several tailnet devices together were not decided separately.
  - **Transport**: this listener's `Peer` counts as HTTPS for `transport_cap`. `same_origin` relies on serve forwarding `Host` as `<fqdn>:8443`, which it does; if that changed, the page would get a 403, never access.
  - **QR and addresses**: `mobile.pair_start` uses the origin and puts `{t: "ts", a: "<fqdn>:8443"}` first in `ep`, so it is the last to be left out of a code too small; `{plain: true}` asks for this
    network's own address, the separate pairing. `running_url` prefers the origin, so `#s=` links open away from home. `addrs` carries `endpoints`, with `list` kept for pages from before. A page's socket is always to its own origin: from HTTPS a browser cannot dial `ws://<LAN IP>`, and the certificate does not match a LAN IP.
  - **`secure_origin`** on a device record says it paired from a page on an HTTPS origin: this computer's Tailscale origin as published at that moment, or a listener with a certificate of the person's own. A pairing that comes while Tailscale could not be read (no origin published at that moment) records false, and stays so until paired again. A program of this user can still produce it, with the pairing secret and the person's approval, by naming that origin on the loopback port; so it says what the person approved, not that no such program exists. It is what the relay and `control` will rest on, so it has a tag of its own (`secure_mac`), a second one so that a record still verifies for an older daemon, which ignores both and drops them when it saves. Setting the flag in the file makes the record `unverified`.
  - **The phone**: at a Tailscale name and not installed it shows how to install, because iOS keeps a tab's storage apart from the installed app's; so does a page on a person's own certificate opened with no code, while a pairing link there pairs directly, as it did before this mode. In the app the code comes from the camera (`qr-scan.ts`: `BarcodeDetector`, else `qr-decode.ts`, loaded only then) or a pasted link; plain `http://` is unchanged. A `*.ts.net` name failing at once reads as `no_tailnet` ("Turn on Tailscale"), distinct from a computer that does not answer; a 5xx from serve is no sign-in page, and the service worker answers with the kept shell.
  - Test seam: `KELPIE_TAILSCALE_BIN`, only with `KELPIE_E2E`; with `KELPIE_E2E` and no such variable there is no Tailscale. `serve --bg` returns at once and the certificate comes on the first request (38 s when measured), which stays with tailscaled. What real phones may yet change is in the phase file (`plans/261024-1205-phone-remote-control/phase-03a-tailscale-https.md`).
- **Kill switch and audit**: `mobile.revoke_all` turns phone access off, removes every device and stops their connections. What phones
  did is in a hash-chained log (`mobile/audit.rs`, `mobile.audit`, `intact` says whether the chain holds; a line cut short by a failed
  write is skipped, nothing the chain covers is lost with it): the verb, device and time; of typed
  text only its length and a truncated SHA-256, so very short text is guessable by the local user who owns the file.
- **On the phone**: the key sits raw in IndexedDB, not as a non-extractable WebCrypto key: the Noise state machine is synchronous and
  a plain `http://` page has no WebCrypto. App lock is a separate phase.

### Bad networks and busy machines

- **Heartbeat** (`mobile/lanes.rs`, verb `hb`, both ways): the daemon sends `hb {n, server_ts, level}` after
  20 s of silence, again every 8 s, and drops the phone (4410) after 45 s; a phone answers with
  `hb {echo: true}` (anything it sends counts) and may send `hb` itself, which is answered. Server
  `SO_KEEPALIVE` (30 s / 10 s / 3) is the backstop. A deadline only counts if the daemon's own timer was on time
  in that window; a gap of more than two ticks (stall, sleep) starts the clocks again (at most for 10 minutes
  without hearing the phone). At L2+ the times double (40 s / 90 s).
- **Legacy pages**: the first sealed message is `{caps}`; a page that does not list both `ack` and `hb`
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
  calm rate for 3 s whatever the level, to every phone that follows it. Never shed: `input`, `key`, `answer`, `chat_answer`, `chat_send`, `chat_cancel`, `chat_stop`, `chat_resume` (their replies go
  out on the writer's own path; the `chat_queue` mirror they change is coalesced to the latest state), revoke, heartbeats,
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

### App lock

A phone that paired on a secure origin reaches nothing until a passkey's signature passes at the daemon (`mobile/lock.rs`, `mobile/webauthn.rs`, API 25, security level 3). The page only asks the person: a copied Noise key alone gets a lock screen's worth of nothing.

- **Which lock a connection has** (`LockPath`), decided by the device's record and never by the shape of the connection, which the caller chooses: a device with `secure_origin` or a passkey needs the passkey over every transport; a device that never had a secure origin may read at most over the network, plain or Tailscale address alike, since such a page has no secure context (`plain_read`, on by default, closes that: state `closed`). A page at an IP address over HTTPS counts as one: it has a secure context, but an IP has no RP ID, so `secure_origin` is set only at a name, and `pair_start` says so with `passkey: false`. A record from before that rule stays at `no_name`, whose screen says to pair again or turn the lock off; a plain connection from this machine is as it would be with no lock, since nothing stands between but this user's own programs.
- **The choke point** is `Conn::handle`, ahead of its `cid` dedupe and of `Conn::reply`'s short cut for `push_test`: while locked only `hb`, `ack` and `lock_*` pass (`Verb::passes_lock`), anything else gets `error {code: "locked"}`. It reads the clock, so a window that ran out holds from that moment whatever a sweep has got to. `hello {locked: true, lock: {state, challenge, rp_id, cred_id?…}, rev: 0, sessions: [], projects: {}}` holds nothing of any session; so does a locked connection's other traffic: `device_changed` and the bulk lane are dropped, `load.status` and `addrs` flow. Being locked stops the connection's follows and chat watches and the device's control grants, and sends `locked {lock}`. Being unlocked is a fresh `hello` (locked ones do not count for finding a daemon restart). `lock.state` is `none`, `open`, `read_only`, `closed`, `unlock`, `enrol`, `desktop` (a passkey may be set up once the computer's button is pressed), `no_name` (an origin with an IP address: no RP ID) or `broken` (the passkey does not verify).
- **The ceremonies**: ES256 only, with `p256`, no CBOR parser and attestation `none`. Enrolling (`lock_enrol {cred_id, spki, auth_data, client_data}`) takes the SPKI from `getPublicKey()` and reads the credential id at its fixed offset. An assertion (`lock_unlock {auth_data, client_data, signature}`) is checked over `authData ‖ SHA-256(clientData)`. Both check SHA-256(RP ID) with the RP ID the origin's name, flags UP and UV, and in the client data the ceremony, the challenge (32 random bytes, single use, 60 s, `lock_challenge` for a new one) and the exact origin, kept with the passkey when enrolled; `crossOrigin: true` is refused. A counter must grow unless it stays 0, as passkeys' do. Ten failures in a minute and a device waits; each goes in the audit log, with nothing of what was sent.
- **The window** is in memory, per device, gone at a restart, and belongs to the page that opened it. The ack of `lock_unlock` and `lock_enrol` carries a random `resume` secret, kept beside the window; the page holds it in a JS variable only and says it in its first sealed frame, `{caps, resume}`. A connection without it is `locked` even while the window is open, and only a holder's `lock_touch` (at most every 30 s, from the person's pointer or key) restarts the idle time, so a copied Noise key can neither enter a window nor keep one open. A flap asks nothing, a reload asks again, and a new ceremony's secret ends the old one's. `lock_now` (the page was hidden) ends the window; if it cannot reach the daemon, because a copied key's connection replaced the owner's, the page has dropped the secret anyway and nobody holds it until the idle time ends. A copied key still gets nothing, but until it is unpaired it can cost the owner: it can send `lock_now`, take the owner's connection's place, and with ten bad `lock_unlock` calls a minute make the owner's own unlock and re-check wait (`too_many`). `reset_lock` clears those failures, and only the audit log records them. The ticker's `lock_sweep` stops the control of a device whose window ran out with no connection to notice; with none unlocked it costs one look.
- **Who may enrol**: only a device on a secure origin with a name, inside a five-minute window the desktop opened: when `commit_approval` adds it, the person having just verified, or by `mobile.reset_lock` (`Role::App` and the person). With no window the state is `desktop`, whose screen says to press that button, so a phone with a secure origin and no passkey needs one press, and a daemon restart inside a window needs another. No released phone is in that state: `secure_origin` came at API 23 and 0.1.11 is API 22, so every phone paired with it is a plain device. It reads with no lock over every transport and, when remote, loses typing until it is paired again or the lock is turned off. What cannot be saved is not taken. **Limit:** a Noise key stolen inside such a window can enrol first; the desktop's toast and audit line say that one was set up, and pairing and approving stay on the computer. A same-user program editing the store is out of scope as anywhere (`auth.rs`), though what it edits does not verify: a passkey's tag covers the phone's id and key and the passkey's own fields, and is never re-sealed by a permission change.
- **Settings** (`mobile.set_lock {on?, idle_s?, recheck?, plain_read?}`, in `APP_ONLY`; default on, 5 minutes, re-check off, reading over plain HTTP on, which is the owner's choice: by default a page that cannot have a passkey reads sessions, screens, chat logs and pending commands with no lock, on plain `http://`, on Tailscale addresses and at HTTPS at a number, and while the lock is on can never type) apply on the next message, a shorter idle time to the windows open now. What takes protection away needs the person. They are in `mobile.v2.json` with a tag, written with the first passkey if not before. A store from before the lock, with no passkey on record, has the defaults. Settings that do not verify, a key that cannot be read, or settings gone once a passkey is on record give `LockOptions::tightest` (on, 1 minute, re-check on, no reading over plain HTTP), which nothing the person can choose is stricter than, and a failed read never unlocks. That covers settings changed in the file, and settings removed while a passkey is on record. **It does not cover everything an edit can do:** removing the settings and every passkey together reads as a store from before the lock and gets the defaults, removing the settings of a store with no passkey does the same, and an older sealed copy of the settings still verifies. Only something kept outside the file would close those, and a same-user program is out of scope here as anywhere (`auth.rs`).
- **Risk labels** (`mobile/risk.rs`, pure): tool, full command, working directory and worktree to a level (`low`, `medium`, `dangerous`, `unknown`) and tags (`network`, `delete`, `outside_worktree`, `git_push`, `secret_access`, `install`, `sudo`, `pipe_to_shell`, `edits_config`). `low` is earned: a known read-only program, or an edit inside the worktree. A program not listed or named by a path outside the system's own folders, what cannot be read (quotes left open, 17 or more sub-commands, a shell's control words) and any bidi, zero-width or control character make `unknown`, unless something on the table is already `dangerous`. What is inside `$(...)`, backticks, `(...)` or a `{...}` script block is `unknown` and is also read as commands of its own, in double quotes and `${...}` as well, as are what `xargs` and `find -exec` run and a shell's `-c` script ahead of its first operand, up to four levels down and 512 KiB of reading in all per label; past either it says `unknown`, and labels are worked out with the session's lock let go. A backslash inside double quotes is read both as a POSIX shell takes it and as PowerShell does, and what either reading finds counts; a command given as an argument array keeps its words (`command_text`). A reader told to run or write (`rg --pre`, `sort -o`, `uniq` with two operands, `git log --output`) is no longer read-only, with long options cut short counted too, git is read through the options ahead of its subcommand (`-c`, `-C` and the like make it `unknown` at least), a file's name counts for `secret_access` and `edits_config` with or without a slash, and a word the shell works out (`$NAME`, a glob) makes a reader `medium`. This table will always have gaps: what it cannot read with confidence should be `unknown`, not `low`, and `unknown` does not ask for the re-check. Known ones it leaves: `/usr/local/bin/` and `/opt/homebrew/bin/` count as the system's although a person can write there, flags on a wrapper (`env -i`, `nice -n 5`) read `unknown` instead of what the wrapped command is, a `\"` with a `;` in the text can read `unknown` in a POSIX command, a `PostToolUse` of an auto-allowed run takes the entry of an open dialog with the identical command, and a plain string given to cmd.exe itself is read as a POSIX shell would read it (`echo # & del /s x`, `echo ^" & del /s x & echo ^"`), since only a written `cmd /c` says it is cmd. A heredoc and `$'...'` read `unknown`. An agent's description, title or reason is never an input. It reads words; it is not a shell, and someone who wants to hide from it can, which is why it is information and the whole command is shown under it. A terminal's prompt is labelled from what `PermissionRequest` hooks said (`wait {command, risk}` on a screen frame), and is `unknown` with no command when no hook spoke: what the screen says is an agent's text. The session keeps the requests open, not one slot: each stays until that tool's `PostToolUse` or `PostToolUseFailure`, or the end of the turn (`Stop`, `UserPromptSubmit`, an idle report, the agent gone). Another tool's events, a long silence and an answer end none of them: an answer does not say which dialog it was for, so a dialog that was refused stays on the card until the turn ends. A card says every command and the worst level, the re-check is over all of it, and past 16 open requests the mildest goes. All of that errs high, with one exception: the end of a turn clears the list, even when it is reported while a background agent's dialog is still open, and that card then says less than it should. A chat's request is labelled from its own fields, per dialect (`chat::rpc_request`), as `chat_request_info {id, request_id, command, risk}` behind its log line, and at `chat_sub` for those still open; a Codex file change names its files only in the log, so it is `unknown`. A command over 64 KiB is not sent: a notice says to read it on the computer, as a command cut short is not the command.
- **Dangerous re-check** (off by default): an `answer` or `chat_answer` that approves a request labelled `dangerous` needs `recheck {expiry, auth_data, client_data, signature}`, an assertion whose challenge is SHA-256(request ‖ 0 ‖ SHA-256(command) ‖ expiry), with expiry at most 2 minutes ahead by the daemon's clock; else it is refused as `recheck`, and the page then asks with a sheet whose button is the tap a ceremony may need. For a terminal's menu Esc alone is known to refuse. Raw `input` and `key` cannot be classified, so while a dangerous request is open they are refused as `recheck`, except `esc` and `ctrl-c`, whatever the screen or the agent's status says, since a dialog stays up while another tool reports and on a screen that never goes quiet. The refusal lifts when exactly one request is open and something was typed since the list last changed: Esc, a card's answer or the computer's keys, which is how the phone can type again after Esc. A refused key writes nothing, so it cannot lift it. A chat approval's `updatedInput` that differs from the request is labelled for what would run, and the assertion must be over that. **`unknown` does not ask for the re-check**, by the owner's choice, so a command read wrongly gets no second look. Only Claude Code registers `PermissionRequest` hooks (`agents.rs`; Codex's `notify` fires at the end of a turn), so a terminal prompt of any other agent is `unknown` and has no re-check. Chat sessions label each request from its own fields, whichever agent it is.
- **On the phone** (`lock.ts`, `lock-screen.tsx`): the lock screen stands before any cached paint, until the computer has said no lock applies here (and what it said is remembered for an offline start). On `visibilitychange` hidden an opaque cover goes up at once, `lock_now` goes out and the page drops the window's secret, so if the network took that with it the next hello is locked all the same. A hello already on its way that says `open` is answered with `lock_now` before anything shows. A ceremony's own system sheet is not leaving. Deny or cancel leaves it locked with its button. The challenge is kept fresh in the background, so that the tap goes straight to the browser. **Residual:** the page's cache and kept messages stay in IndexedDB unencrypted, a person with the unlocked phone's browser tools can read them, and the label is rendered by the page: what comes from the daemon over TLS.

### Phone client

The page the daemon serves to phones is a SolidJS app of its own, `app/phone` (installable as a PWA), not the
desktop frontend. It has its own build, `pnpm build:phone` (`app/vite.phone.config.ts` → `app/dist-phone/`); `pnpm build`
stays the desktop's. `crates/kelpied/build.rs` embeds `dist-phone` as `PHONE_ASSETS` and `mobile.rs` serves it by path
(hashed `/assets/*` are cacheable, the page, `sw.js` and the manifest are not). A development build without the bundle embeds
a placeholder page; with `KELPIE_REQUIRE_PHONE=1` (`scripts/build-release.sh`, `ci.yml`, `release.yml`) the build script
fails instead, and the release workflow greps the daemon for the placeholder's marker. `pnpm build:phone` must run before any
`cargo build/test -p kelpied`. `scripts/check-phone-guard.sh` proves the guard. The bundle may not import `@tauri-apps/*`
or `lib/ipc` (the build fails), is about 83 KB gzip (page, script and style, with the Noise code; about 100 KB with the agent adapters it loads when needed and the service worker), and carries only the Vietnamese strings its code uses
(`app/phone/vite-dictionary.ts`, from `app/src/locales/vi/*.json`; the phone's own are in `phone.json`).

- **One boundary**: `app/phone/src/link.ts` is the only file that knows the wire (kelpie-link, above; pairing is `pair.ts`). It declares
  `caps: ["ack","hb"]` in its first message, answers and sends heartbeats (it asks after 15 s of silence and drops the socket if
  3 s pass without a word), reconnects with full-jitter backoff (first retry within 200 ms, then 1 s doubling to 30 s) and at once
  on `visibilitychange`, `online` and a failed probe (at most every 2 s), resumes with `sub {have_rev}` / `chat_sub {since_seq}`,
  and refuses writes while not connected: nothing is queued. An `answer` or `chat_answer` whose reply was lost with the
  connection is sent once more after the next hello within a minute (same daemon run only); typing and keys are never retried.
- **Connection state** (`conn-state.ts`): one enum shown by an always-visible indicator: live, degraded (round trip over
  800 ms, a late heartbeat or frames 2 s behind), reconnecting (attempt, seconds to the next), no network, sign-in needed (a
  same-origin probe got another page), computer offline since, relay unreachable (reserved for the relay), computer restarted
  (epoch change), computer busy (`load.status`, worded by reason) and replaced. The last list and last screen are kept in
  IndexedDB and painted first, dimmed, with "as of hh:mm:ss"; a data-age chip goes amber at 2 s and red at 5 s.
- **Chat** reuses the desktop adapters through `chat/transport.ts` in observer mode (`ChatController`): no `start()`, no queue
  of its own, no `session.update`, no write the adapter makes by itself; only the person's answer to a card goes out, as
  `chat_answer`, and the card settles on the daemon's verdict (ok / dup, "already answered on … " for stale, "not sure" for a lost
  reply). `chat_sub` replies are applied with `applyLog` (`reset` replaces, `delta` / `more` carry on, `live` appends).
- **Sending to a chat** (`chat-send.ts`, `ChatComposer`): text only; the daemon builds the turn. A message goes
  `sending…` → `queued (n ahead)` → `sent` (its own `cid` seen in a `kelpie_sent` line) → the transcript. A `chat_send` whose
  reply was lost with the connection is sent once more after the next hello within a minute, under the same `cid` (the daemon
  answers `dup`, never a second message); after that, or after a computer restart, it is shown as *not sure it was sent* or
  *lost*, with its text, and Retry, Put back or Discard. A refusal puts the text back in the box with the reason. Enter is a
  new line; the Send button (or Ctrl/Cmd+Enter) sends, which keeps Telex/VNI composition safe. Stop is a button of its own while
  the agent works; a stopped queue shows *Resume*. The chips list every origin's waiting messages, each with ✕ (Edit on the
  phone's own). Codex `/` and `!` commands, ACP `/new`, `/clear`, `/reset` and Claude's terminal-only commands are refused on the
  phone with a hint to use the computer.
- The phone has no `focus` or `visibility` verb (the table is closed): focus is `sub {focus}`, and a hidden page unsubscribes
  everything and re-subscribes, resuming, when it is shown.
- **Approval cards** (`command-block.tsx`, `command-text.ts`) show the daemon's label first, then the full command, monospaced, nothing cut, with every control, bidi and zero-width character written out as `\u{202E}` and set apart. A long one must be scrolled to its end before it can be approved; a refusal never waits for that.
- A service worker keeps the app shell, only on secure origins; plain `http://` works without it. It also shows alerts where the system does not (`push`), opens their link on a tap, in this origin and nowhere else (`notificationclick`), and marks a subscription the browser changed (`pushsubscriptionchange`) for `push.ts`, which compares the browser's subscription with what the computer's `hello` says on every open and renews it with no tap once permission is given. A tap on an alert about a wait says "Already handled" when the synced list shows the wait is over.

Checks: `cd app && pnpm test` (link, sync, state, inbox, composer, observer), `scripts/e2e/mobile.py` on the real page (inbox
order, prompt tap, chat answer, `#s=` link, reconnect).

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
entries' own `timestamp`). Since API 20 the log also keeps the answers to the agent's questions, so
a reopened window shows what was chosen, who chose it and when, for every dialect:
`{"type":"kelpie_answer","request_id":…,"by":"desktop"|"phone","answered_by":<device name>,"at":<unix ms>,"answer":…}`,
written after the answer went to the agent, once, for the first answer to a request the agent had
open (`pending`) and that can still take it (its stdin is open). `answer` is sanitized: Claude's
`behavior`, `message`, a question's `answers`, `lasting` and `mode`, never the rest of
`updatedInput`, which can hold whole files; JSON-RPC's `result`, or `{"error":true}`.
A line is at most 16 KiB: above that only the verdict stays. A request over 1 MiB is not tracked,
so its answer is not recorded and after a replay its card says "No longer needed". ACP's
`cancelled`, which a stop sends for a permission, writes no line; the `{"action":"cancel"}` it sends
for an elicitation does, as it cannot be told from the person's own Cancel. A stop answers what is
still open, also a card a replay showed, since the agent is waiting on it; one answered or expired is
left alone. `ChatController` applies the line ahead of the adapter
(`Adapter.describeAnswer` puts it in the card's words) and it never reaches an agent: a replayed
answer only changes what a card shows. An older daemon writes none, and its cards say what they
did; an older app's adapters ignore the type. Levels 18 and 19 are unused. Claude 2.1.314 shrinks a picture before it echoes it (a 4 MB picture came
back in a 680 KB line), so a replay usually shows the picture from the log. As a safety net, a line
over 2 MiB (`PICTURE_LINE_MAX`) loses the bytes of the pictures in
the person's message before it is logged or sent to anyone attached, because one 5 MB picture would
evict most of the 12 MiB log, and a line over 4 MiB (`MAX_LINE`) would be replaced by a notice and
take its words with it. That covers a Claude `user` line (the echo, or an image in a tool's result)
and an ACP `user_message_chunk`; Codex logs carry paths, not bytes. Each picture keeps its type, with
`data` empty and `kelpie_stripped` holding the length it had; everything else stays, and a shorter
line is logged byte for byte. The transcript then shows the picture from its path if it was attached
in this window, and otherwise a named placeholder. A path in a log or a history is agent output:
`chat/chat-images.ts` reads one only if it was attached in this window or lies directly in the
composer's temp folder, and `fs_read_image` refuses network paths in every spelling (`\\`, `//`,
`/\`, `\??\`). A replay feeds the log in one Solid `batch`, as loading a history does: what watches
the whole list would otherwise run for every line. Adapters in `app/src/chat` map each
dialect to one item model (user, assistant, thinking, tool card with diff, permission, result).
The agent map (`chat/agent-runs.ts`) is derived from the same stream, so a replay rebuilds it: on
Claude Code a run's status comes from its `task_*` events and never from its tool row, because the
Agent tool returns `async_launched` as soon as the agent is launched. Links in a reply carry their
target in `data-href`, read on click by `parseChatLink` alone (`chat/chat-links.ts`): DOMPurify
empties `file:` and `a.ts:42` hrefs, and `CHAT_SANITIZE` also drops what could navigate the window or
cover its buttons (`form`, `area`, `style`, `class`). Nothing an agent names is ever launched.

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
`content-visibility: auto`. A controller outlives its pane: coming back to a chat feeds only what the
log holds beyond the offset already fed (`seq` of the attach reply, checked against the last bytes
fed), still as a replay. It rebuilds when the log does not carry on from there or after
`session.resync`, reading any history first and swapping in one update, so the old rows stay until then.

Beyond the items, adapters fill shared state the UI renders the same way for every agent:

| State | Claude | Codex | ACP |
|---|---|---|---|
| plan | `TodoWrite` input | `turn/plan/updated` | `plan` update |
| usage (context used / window, cost) | `assistant.message.usage` (input + cache + output) / `result.modelUsage[model].contextWindow`, `total_cost_usd` | `thread/tokenUsage/updated` (`last.totalTokens` / `modelContextWindow`) | `usage_update` when sent |
| turn diff | from Edit/Write inputs | `turn/diff/updated` | from `diff` contents |
| tool category | tool name | `commandActions` (read / listFiles / search) | tool `kind` |

`chat/tool-view.ts` turns a tool item into a row (verb, target, detail, IN/OUT) and diffs
edit strings line by line. A message sent during a turn goes straight to the adapter's `steer` when it
can (Claude: written mid-turn, the CLI reads and echoes it when the running tools finish, or, when the
turn ends first, runs it as a turn of its own after a `result`, during which the adapter stays busy; Codex:
`turn/steer`, queued again if refused): text only, no commands, and nothing else waiting. Below API 21
`ChatController` queues the rest and sends it when the turn ends (adapters set `busy = false` last); *Send
now* uses `steer` or stops the turn; stopping holds the queue. From API 21 the queue is the daemon's, below. Approvals carry what they
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

### The message queue (API 21)

One bounded FIFO per chat lives in the daemon (`chat/queue.rs`; at most 20 messages, 5 per phone), fed by the window and by phones,
in the order the daemon received them. Exactly one writer feeds the agent: the daemon's pump writes the head of the queue
only when the agent is alive, the queue is neither `held` (a hand-off to a terminal is under way) nor `paused` (a stop), no turn
is in flight and the agent's own lines say it is idle (`classify`); for Codex and ACP the conversation must be known too
(`chat/turns.rs` follows the thread and session ids in the handshake answers, and a `thread/start` or `session/new` makes the old id
wrong until the answer). A turn is *in flight* from a write until an `Idle` line, an error answer to the request that started it, or
the process ending; a turn-shaped line that reaches stdin by `session.input` (an older app, an adapter's own pre-thread queue, a
steer) is observed and counts too, never blocked, and an interrupt line seen from either side pauses a queue that has something in
it. A turn that was written, drew no output and left the agent idle for 30 s is given up.

Who builds the line depends on who holds the state. A **phone's** message is plain text and the daemon builds the turn when it
writes it (`turns.rs`: three user-turn builders and the interrupts, checked against one golden fixture,
`chat/turn-fixtures.json`, by cargo and vitest). A **window's** message is built by its adapter at dequeue (pictures, Codex skills and
picks, client-side commands): the daemon sends `chat.turn_ready {handle, qid, cid}` to the window that queued it, which runs
`adapter.send` with the writes captured and answers `chat.write_turn {handle, qid, lines|null, turn, retry?}`. `lines: null` with
`retry` means "not now" (replaying the log, loading history, not attached yet): the daemon asks again after a second, never gives up
on it, and announces nothing while what clients see is unchanged. A plain `null` means the window does not have the message (a reload
lost it) or could not build it (the adapter threw): that is final. Plain text (nothing a picture or a command needs) is built by the
daemon when the window is not attached, says `null`, or does not answer within 15 s. Anything else *waits at the head* (`needs_window:
true` on its item, shown to every viewer, cancellable from either side) for as long as the window's daemon connection is alive and the
window has not said `null`; it is offered again when the window attaches, and a window that is attached but never answers is asked
every 30 s (15 s to answer, then 15 s before the next offer). It holds up what is behind it, in strict order: nothing reorders or
expires on its own. The phone shows "a message only the computer can send is waiting for its chat to be open there" with Skip (a cancel of
that message) on every phone message behind it, and the session's info carries `waits_for_window`, so the app says so outside the chat
pane, which a tab or project switch unmounts. When the connection is gone it is dropped as `needs_window`, when the window said `null`
as `declined`, and a `{"type":"kelpie_unsent",...}` line in the chat log keeps the note for whoever looks next (the daemon's
`client_gone`). If the daemon does not take the lines (`busy`, `switching`,
`too_late`) the window undoes what building them did: the pending bubble, `busy` and the adapter's own state (`Adapter.snapshot`).
Lines over 16 MiB cannot go, whoever builds them: `chat.write_turn` answers `too_large` and changes nothing, the window undoes what building them did and says so with `{lines: null,
too_large: true}`, as it does when its own connection refused the lines (over the frame limit). The message then leaves the queue with that reason, everyone is told
and what waits behind it moves on, so the next offer cannot be undone by that rollback. Any other failure is a final `null`. Both `chat.queue`'s `dropped` and `kelpie_unsent` carry the names of the pictures (`images`), so a note
keeps them after a reload.
The window's `chat.send {handle, cid, text, rich, images?}` answers `{qid, state, ahead}` or `{refused, message}`.
`chat.queue_op {handle, op, qid?}` is `cancel` (any queued message, whoever's; an offered or written one is `too_late`), `front`
("send now"; `interrupt: true` when the adapter's interrupt follows and must not pause the queue), `take` (the window puts the message
in the running turn itself), `resume`, and `hold` (`held: false` releases it; mode switches hold before they interrupt and close
stdin, so no turn slips in). `chat.queue {handle, id, items, paused, held, in_flight, dropped?}` goes to every client attached, on
every change and on attach; a window shows the chips from it and the not-sent notes from `dropped` (`exited`, `closed`, `revoked`,
`needs_window`, `declined`, and `cancelled` with `cancelled_by` when someone other than the sender took the message out: the sender's
phone shows "Cancelled on desktop" or "Cancelled on phone" and keeps the text, the desktop notes it). Every such cancel is also logged
as a `kelpie_unsent` line with `reason: "cancelled"`, so a sender that missed the live event learns it from the log: a desktop pane
that was closed, or a phone that was offline (it matches its own `cid` in the log replayed on reconnect, so the message is not taken
for sent). The phone keeps a copy of what still needs a decision (at most 20 per chat and 60 in all; copies for chats the computer
no longer lists are dropped), and says so when the browser would not keep one. Once a window has been told the queue
it stays on the daemon path (it never flushes the mirrored queue itself, whatever the connection does), and a send that cannot reach
the daemon is shown as not sent with its text. A phone's `chat_send` re-checks the device's permission once the chat is locked
(`enqueue_if`), and a revoke or downgrade takes the permission away before it drops what the device had queued, so nothing is left
behind; turning phone access off drops every phone's waiting messages. A send the chat refuses (too long, a command for the computer,
a full queue, a chat that is not running) does not spend a rate-limit token.

*A stuck "working".* The gate trusts the agent's last classified line. A turn that is not in flight (the queue and every raw turn line
set one) and has printed nothing for 60 s is taken as over for the queue only (a late chunk after a result leaves the last line a
working one); a turn in flight, or a question waiting for the person, is never taken as idle. A stop (the phone's, or an interrupt
line seen from the window) of a running turn makes nothing idle until that turn's `Idle` line, so no message goes into a turn that
is still stopping (Codex answers a `turn/start` during a turn with an error). A stopped turn that printed nothing at all for 30 s is
let go of by the queue only (`released`, until the agent's next classified line): the session's status, turn count and
notifications stay as the agent's lines made them. Neither rule writes a second turn into a live one, and neither interrupts
anything. A Stop with nothing to address it to (Codex before `turn/started`) still pauses the queue.

Every message that starts a turn is logged as `{"type":"kelpie_sent","qid","cid","by","name","text","images"}` ahead of the write
(text cut at 64 KiB): ACP agents do not echo a prompt, so their transcript shows the user item from this line, live and on replay;
Claude and Codex echo their own, and the line only names the sender. It is display-only: history replay never sends a command to an
agent. With no window attached the daemon also answers an ACP agent's request for the client (`fs/*`, `terminal/*`) with `-32601`, so a
turn cannot hang on it. The queue is in memory and dies with the daemon run, by design. Below API 21 the window keeps its own queue.

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
  written twice). The copy is given the real index's modification time. Git reads a file whose size and
  times match its entry only if the file is as new as the index file, so where git compares whole seconds
  that time is all that shows a same-size edit in the second of the entry; a copy stamped later would hide
  it (a Linux copy gets a new time unless it is set; a Windows one keeps it). The time is read back: a
  filesystem that does not keep it fails the snapshot rather than letting git trust the copy. A same-size
  edit that also keeps its modification time is **not** seen by a snapshot, as it is not by `git status`
  (on Linux and macOS a later change time usually gives it away): reading every file would cost too much
  here (numbers below), and undo does read the turn's files. The user's HEAD, branch and index file are
  never written by a snapshot or an undo; a snapshot does not write the user's files either. An undo writes
  the turn's files, and to restore one it replaces what stands in its way: such a file or link where its folder
  was, or a folder with other files where it was, is asked about first (see Undo). Every git call runs with `GIT_LITERAL_PATHSPECS=1`, `GIT_OPTIONAL_LOCKS=0`
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
  the turn, the file differed between turns (the look at the start), could not be read by the plan itself, by this
  or by the previous snapshot, was written before the turn's start (file mtime) or removed before it (its folder's mtime),
  has a file or link in its folder's place, or sits where a folder now holds files the turn did not leave there;
  else *clean*. A path inside a folder the turn made a submodule is skipped like the submodule: it is another repository's. Because every
  session's snapshot holds the others' concurrent edits, files in a shared folder are often shared. `undo` changes nothing and returns
  `needs_confirmation` unless every shared or modified file is named in `confirm`. The plan reads only the
  turn's files, by name, into a temporary index filled from the turn's tree (`read-tree`, then `add -A` with
  literal pathspecs from stdin, `-f` and `core.sparseCheckout=false`): it has no stat data or assume-unchanged
  marks to trust, and leaves out neither a file the turn stopped tracking and ignored nor one outside a sparse
  checkout, so a same-size edit cannot pass for the state the turn left, and nothing else in the folder is read.
  A path outside a sparse checkout is read when it is present and counts as changed when it is absent, as a sparse
  checkout leaves it, which asks more often than it must. This cost is not counted towards auto-off. Just before writing, `undo` compares each
  file's size and modification time with what they were just before the read; a file saved since is reported in
  `needs_confirmation` again, even if it was confirmed. That leaves the moment until `git` reaches the
  file, and a same-size save that also keeps its time: the gap is much smaller, not gone. After any refusal or
  error the dialog reads the plan again and keeps only the safe files ticked, so the next press does not carry a
  tick made on the old plan. A confirmation still names a path, not a version: a file ticked in the dialog and saved
  again while the dialog stays open, with no refusal in between, is overwritten. Deletes run first, so a case-only
  rename or a file and folder that swapped places go back cleanly; they remove only paths that pass `safe_rel`
  and are not behind a link, then folders they leave empty. Restores are `git restore --source=<base> --worktree`
  with literal pathspecs from stdin, on a copy of the index stamped second 1 of 1970, so a file that only looks
  like its index entry is still written and the real index's lock is not involved. The copy is made before anything
  is written. If a write fails, what was done stays done, no `undo` step is recorded, and repeating the undo
  finishes it. A file already equal to its state before the turn
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

Reading every file instead of trusting the stat cache was measured for snapshots and rejected (debug test binary,
same bench, `KELPIE_CP_REPO` reuses one repository, `KELPIE_CP_REPEATS` sets the file size; Windows 10, 10k files,
3.3 MB). Trusting it: unchanged turn p50 153-160 ms, git CPU 156-203 ms; changed turn p50 357-392 ms, git CPU
265-296 ms. Reading every file: unchanged turn p50 1004 ms, git CPU 1000 ms; changed turn p50 1232 ms, git CPU
1156 ms, and two such snapshots switched the repository off. The kept design is as cheap as before, also on 10k files
of 208 MB (unchanged p50 157 ms, changed p50 363 ms). An undo reads only the turn's files: `undo_plan` of a four-file
turn takes p50 411-433 ms on 3.3 MB and 402-410 ms on 208 MB (it took 1.14-1.18 s and 1.97 s reading the whole folder),
on its own thread and outside the auto-off count, but under the repository's lock, so snapshots wait behind it. Turns
with large files cost what reading those files costs, and turns with many files cost more per file: on a repository of
10k small files, `git add` of the named paths took 150 ms for 100 paths, 1.17 s for 1,000 and 9.26 s for 10,000 (an
unrestricted `add` on the same temporary index took about 5 s), and `git restore` 238 ms, 1.40 s and 12.7 s. On 30k
files `add` took 226 ms for 100 paths; larger turns there were not measured. Matching paths costs roughly entries times
paths, so a turn touching most of a much larger repository is slow to preview. No fallback to an unrestricted `add` is
used: it reads the whole folder, and could not take `-f` without adding every ignored file.

A 100,000-file repository was not measured (building the fixture did not finish in 90 minutes); expect several seconds per snapshot there, so such a repository switches itself off after two.

## App-side services (Rust core)

| Module | Does |
|---|---|
| `agent_history.rs` | Lists past Claude / Codex / OMP conversations from their transcripts (`~/.claude/projects`, `~/.codex/sessions` incl. `.jsonl.zst`, `~/.omp/agent/sessions`). Only the first 512 KB of a file (plus Claude's tail for its latest title) is read; results are cached by mtime/size in `history-cache.json`. Prompt counts run on two background threads, resume from the last counted line of append-only files, and emit `agent-history-counted`. |
| `session_export.rs` | Markdown export of one listed conversation (`agent_session_export {agent, id, cwd?, home?, title?, path}` → `{path, skipped, truncated}`; claude / codex / omp only). The transcript is found by id under the agent's own folder (`session_usage::resolve_in`, plus Codex `.jsonl.zst`), never through a path inside a transcript; it is streamed (lines over 32 MiB skipped, output capped at 16 MiB with a note) on a blocking thread. Each text piece passes `kelpie_proto::redact` before it is cut (tool input / output: 40 lines, 4,000 characters; only that much plus a 4 KiB margin is redacted) and fenced; structured tool input and JSON tool output (whole, or one JSON value per line) have every string redacted before serialising (JSON escapes hide line starts from the patterns; every number or string under a secret-named key, at any depth, is masked), then the text again: the ranges `kelpie_proto::redact::redact_ranges` finds in the text as written and as read with JSON escapes undone are merged and masked, then filtered once more. User / assistant text is contained (`contain`): every message (an entry's text) has its own heading, so a list or code block cannot run on into the next; `pulldown-cmark` (parse only, GFM tables / task lists / strikethrough like the app's `marked`) gives the ranges of fenced, indented and inline code, and every tag-like `<` (then a letter, `/`, `!` or `?`) outside them is escaped; the result is parsed again and must hold no raw HTML and leave a following heading a heading (escaping is redone by the new code ranges at most three times). A message left inside an open code fence (an interrupted reply), one with a fence `marked` would end early (its closer rule accepts the opener run followed by any mix of backticks and tildes, CommonMark's does not), one nested more than 100 blockquotes / lists deep (`marked` overflows its stack), or one that does not pass, is written whole as plain text in an outer fence longer than any backtick or tilde run in it. Escaping walks the sorted code ranges with a cursor (linear). The golden files, and `adversarial.md` (every hostile message from the reviews as an export writes it, rewritten by the ignored `bless` test and checked byte for byte), are rendered with `marked` by `app/src/lib/session-export-goldens.test.ts` (part of `pnpm test`; alone: `pnpm exec vitest run src/lib/session-export-goldens.test.ts`): every heading and `<details>` survives and no tag outside an allow-list, comment, processing instruction or declaration appears outside code. Thinking is dropped, images and attachments are named. OMP sessions are trees: only the active branch (leaf, or an off-branch entry's `activeLeafId`, back to the root by `parentId`, a root's parent being none or the session header) is exported; Claude (whose `parentUuid` chain also forks for parallel tool results) and Codex are exported in file order. LF line endings; written beside the chosen file through a uniquely named, newly created temporary file that is renamed over it and removed on any failure (refused for a relative path, a device path, the transcript itself, or a network account home). Conversation ids must be file-safe (no `:`, `..` or separators; the shared resolver checks this for usage too). Golden files: `app/src-tauri/tests/fixtures/session-export/` (synthetic), rewritten by the ignored `session_export::tests::bless` test. With `KELPIE_E2E`, `KELPIE_AGENT_CONFIG_HOME` also replaces the home under which OMP's sessions are read (list, usage, export). |
| `session_usage.rs`, `usage_report.rs` | Context / tokens / cost of a conversation from its transcript (Claude, Codex, OMP parsers, reverse tail, per-file aggregate cache, prices from `pricing.json`) and the cost report for the Usage page. See "Context, tokens and cost". |
| `usage.rs` | Claude (`api.anthropic.com/api/oauth/usage`) and Codex (`chatgpt.com/backend-api/wham/usage`) plan windows, using the agents' own OAuth sign-in. Tokens never leave this module; the webview gets percentages and reset times only. |
| `sys_stats.rs` | Memory of the app + WebView tree, the daemon and its descendants (PID-reuse safe). |
| `git.rs` | Status, branch summary, diffs, staging, commits, worktrees (`add` new or existing branch / `remove` / `prune`, file at a revision, commits only a detached HEAD reaches). |
| `git_basics.rs` | Branch switcher (refused before git when another worktree holds the branch, the tree is dirty, an operation is in progress, or a detached HEAD has commits no ref reaches; hooks run as in a terminal), compact log (last 100), stash list with apply / drop. Branch names must be existing `refs/heads/` names that cannot read as options. A stash action names `stash@{N}` plus the commit the list showed and is refused when they no longer match (the stash is per repository, shared by worktrees); apply then uses the commit itself; after a drop the list is compared with the one taken just before (never git's message, which a remote host may translate), any other stash that went missing is stored back, and anything unconfirmed is an error naming the commit. |
| `updater.rs` | Updates through `tauri-plugin-updater`: `latest.json` from GitHub Releases, verified with the minisign public key in `tauri.conf.json`. Install = download (signature checked) → installer (Windows, passive with restart; the app exits) or in-place bundle swap and restart (macOS; Linux: the AppImage, or a `.deb` through `pkexec dpkg -i`). The daemon is never stopped. Dev builds and `KELPIE_E2E` runs never check. Endpoint/key overrides and a dry install exist only with the `e2e-updater` feature. |

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
at the universal bundle; `linux-x86_64` at the signed AppImage from the `ubuntu-22.04` job) into a
**draft** release; apps see it once the draft is published.
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
- **Processes and ports** (`procs.rs`, `ports.rs`, `daemon/process.rs`, API 16): a session's tree is the
  descendants of its process by parent PID (a parent that started after its child is a reused PID, not a parent;
  shared `kelpie_proto::procs::child_map`/`descendants`, also used by the app's memory meter) plus, on Windows,
  the members of its Job Object (`QueryInformationJobObject(JobObjectBasicProcessIdList)`): a program a shell
  starts is not always in the job (the children of a packaged PowerShell are not), so the job alone is not the
  tree. A scan reads the process table once: on Windows one `NtQuerySystemInformation(SystemProcessInformation)`
  call (parents, start times, names, working set and CPU time of every process, ~2 ms; a Toolhelp snapshot or
  sysinfo cost ~6 ms each for the same), with the executable path read from the handle of each process listed;
  on Unix sysinfo without threads (`without_tasks`; Linux would list each thread as a process), through one
  long-lived `System` for every reading (one boot time, so start times agree after a clock step), the executable
  path read fresh for each process listed and again before each stop. Every process is keyed by its identity
  (pid, start time, exe path); one remembered in a tree that still runs as the same process outside it is listed
  as an orphan, one whose identity changed is dropped. The session's own process is the one the session reports
  now, checked against the start time the session recorded when it started it (a PID reused before the session
  learnt its process ended is not the root, and nothing is walked from it). So after a restart under the same
  session id the new process is the root, never a child, and the reported PID is always in the keep set; what
  the previous run left is remembered as orphans. Ports: Windows `GetExtendedTcpTable` (listeners, both
  families), Linux `/proc/net/tcp*` with socket inodes, macOS one `lsof` for all the PIDs of a look. Nothing
  scans by itself: `process.ports` (for an agent's web pages: `lib/agent-web-pages.ts`, which sends `browser_probe` a port once per session to learn whether it answers with a page) is asked by the app for the visible sessions, once when they
  appear, ~0.8 s after their terminal prints (at most every 2 s), and every 3 s only while the last look found
  children; on macOS it runs `lsof` only when some session has children or orphans or is a custom session (whose
  own process is often the server), elsewhere whenever a session has a process. `process.list` while the Processes panel is open (every 2 s). `daemon.info.process_scans`
  counts scans. `process.kill {session, pid, start_time, exe?}` refuses unless the process is in the session's
  tree (or its orphans) with that identity, never the session's own process or the daemon; it stops the process
  and its descendants deepest first. Each victim is re-checked against its identity through the handle that
  stops it (Windows: an open process handle, so the PID cannot be reused in between; Unix: re-read before
  `SIGTERM` and again before `SIGKILL`). Graceful first (Windows: `WM_CLOSE` to its top-level windows, then
  forced after 3 s; a console program has none and is not asked — no console control event is sent — but
  terminated at once, or after the grace when a program with windows in the same stop was asked;
  Unix: `SIGTERM`, forced after 3 s). `process.kill_children` keeps the session's process and, in an agent
  session, the agent's process (the root when named like the agent; under a launcher the child named like the
  agent, or whatever a batch file run by `cmd` starts directly; a launcher with neither is the agent) and the
  agent's direct children (MCP servers, tool shells); their descendants are stopped. Kills answer from their own
  thread.
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
  dedicated thread, macOS a `caffeinate -i -w <pid>` child, Linux a `systemd-inhibit --what=idle:sleep` child that dies
  with the daemon (`PR_SET_PDEATHSIG`; logged once and skipped without systemd or when logind refuses it,
  even later); a suspend picked from the menu waits while it is held, the lid follows the system setting; mode *agents* holds it while any
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
| Phone server | `crates/kelpied/src/mobile.rs`, `crates/kelpied/mobile/` | axum on a chosen address (default port 4817), optional rustls (ring). Pairing: a one-time secret shown as a QR (2 min), then the phone's six digits typed into the verified app (see "Phone link"); devices are Noise public keys in a MAC'd store; Origin checks, CSP, failed attempts limited per source address; revoking closes the device's sockets. Protocol v2 (verbs, state sync, answers, chat): `mobile/verbs.rs`, `mobile/sync.rs`, `mobile/lanes.rs`, machine load in `load.rs`, and "Phone protocol (v2)". |
| Plugins | `app/src-tauri/src/plugins.rs`, `lib/plugins.ts` | Declarative manifests (see [plugins.md](plugins.md)); approval bound to the manifest SHA-256; agents are sent to the daemon (`agent.set_custom`, kept in `custom-agents.json`). |
| Agent setup | `app/src-tauri/src/agent_config.rs` (+ `agent_config/`), `lib/agent-setup.ts`, `components/agent-setup.tsx` | MCP servers, instruction files and skills of the agents, from their own files: Claude `~/.claude.json` / `.mcp.json`, Codex `config.toml`, Gemini `settings.json`, OpenCode `opencode.json(c)`, Cursor `.cursor/mcp.json`. One neutral `McpServer` with a codec per agent; reads happen only when the page opens. **One writer per file**: JSON and JSONC through a syntax tree (`jsonc-parser`: comments, order, indentation and unknown keys survive), TOML through `toml_edit` — the same writer `accounts.rs` uses to keep a Codex login in a file, so an MCP server copied into `config.toml` survives an account re-seed. **Trust**: a server found in a repository is `project` scope (untrusted); copying it to a user-level file needs an explicit confirmation, enforced by the app on apply, of everything it would run (argv per element, working folder, every env / header entry as the repository wrote it; relative paths are made absolute inside the project); none of its environment is carried unless opted in, and no reference to the user's variables is invented for it. A user's own environment and header values never reach the webview (names only), never a project file (they become `${VAR}`-style references in the destination's syntax, rewritten inside values too; Codex uses `env_vars`, `env_http_headers`, `bearer_token_env_var`), and are masked in every diff by position in the parsed tree. Project-scope paths (and `AGENTS.md` / `CLAUDE.md` / `GEMINI.md`, `.gemini/settings.json`, `.claude/settings.local.json`) are refused when a link or junction leads out of the project. **Writes** (one apply at a time): preview (a token over the files as read and as they would become, refused when stale) → per file, with Claude's own lock (`~/.claude.json.lock`, `mkdir`, stale after 10 s) held for `~/.claude.json`: compare with the previewed text → backup (`<data>/agent-config-backups`, private, newest 20 per file) → unique private temp file in the same folder (owner-only on unix, fsync) renamed over the target, retried and then refused, never rewritten in place → read back and parse. A write that is Kelpie's own and broken is put back (with the earlier files of the edit), but only while the file still holds what Kelpie wrote; a file another program saved since is left as it is and reported. The ACP registry (`agent_registry_fetch`) is one https GET per click (size- and time-capped); an entry becomes a custom agent (`npx` / `uvx`, an exact version; tags, ranges and URL or git specs are replaced by the registry's version or refused) that is stored in the settings and sent to the daemon with the plugin agents (`agent.set_custom`, the user's capability); it has no terminal interface, so the app starts it as a chat only. For tests only (`KELPIE_E2E`): `KELPIE_AGENT_CONFIG_HOME` replaces the user's home for every lookup (without it an end-to-end run cannot write), `KELPIE_ACP_REGISTRY_URL` moves the registry (plain http to 127.0.0.1 / localhost only), and a marker file `agent-setup-corrupt-next` corrupts the next write. |
| Voice | `crates/kelpie-stt`, `app/src-tauri/src/voice.rs`, `lib/voice.ts` | A separate helper process: sherpa-onnx (static, offline) with a Vietnamese zipformer or Whisper; `cpal` microphone capture mixed to mono; JSON lines over stdin/stdout; models downloaded on request and verified by pinned SHA-256. Kept out of the app and daemon so the native engine can never take them down. |
| Settings export / import | `app/src-tauri/src/config_transfer.rs`, `lib/config-transfer.ts`, `lib/settings-transfer.json`, `components/settings/config-transfer.tsx` | A file `{kind: "kelpie-config", version: 1, exportedAt, appVersion, sections}` (sections `settings`, `quickCommands`, `layouts`, `automations`, `sshHosts`, `plugins`, `agents`). The window passes its own sections and the daemon's `automation.list` (only at the API level with event automations); the app core adds hosts (`hosts.json`, read strictly) and the plugin list and owns the file: 2 MB cap (checked on the path's metadata before it is opened), an allowlist per section (unknown fields and sections dropped). Settings keys come from `settings-transfer.json` (`exported` / `excluded`, read by both sides; a unit test makes every default setting be classified); the window also checks each imported value against the settings page's enums and ranges. Credential filter (a heuristic, the same both ways): every string through `kelpie_proto::redact` plus `NAME=value` / `--flag value` / `--flag=value` with a secret-sounding name (a name part PASS, PWD, KEY, AUTH, COOKIE, PAT, or one ending in PASSWORD, SECRET, TOKEN, CREDENTIAL, PASSPHRASE, SSHPASS, DBPASS, PRIVATEKEY, SECRETKEY, ACCESSKEY, APIKEY, AUTHORIZATION, WEBHOOK, less a list of known non-secrets such as BYPASS; parts split at punctuation, camelCase and acronym ends; `…env_key` names a variable and is left alone), secret headers, Bearer tokens, `-u user:pass`, and `-pPASS` for mysql / mariadb / sshpass, `mysql -p…`, `curl -u user:…`; argument lists mask the element after such a flag; environment as `[{name, value?}]` with values only on request and never for such a name; automations without approvals, run history or `enabled`; hosts without credentials, with ids passwords can be saved under; one path classifier (`path_kind`: local, network — `\\server`, `//server`, `\\?\UNC\`, `\??\UNC\`, any mix of separators — or device — `\\.\`, `\\?\`, `\??\`) refuses network and device paths for a key file, an automation folder, program or any argument (the daemon reads those on save) and the terminal program and arguments, naming what was left out on export and import; device paths are never opened as the file itself. Import: plan against the current configuration (preview counts by id, then name; each section read on its own, an unreadable one shown as unavailable), with every value that decides what runs shown old → new (`shellArgs`, `agentPermissions`, `agentsScripting`, profile args / permission / env, quick-command text, automation action / kind / command / args / prompt / cwd / trigger or schedule / deliver / env, host target (new hosts) / `identityFile` / `startup`, layouts with remote panes) behind a separate confirmation; a masked value (`[redacted]`) is never written: it takes this computer's value, and the preview says so (arguments by position only: the same occurrence of the same flag for an exact marker, of `name=`, of the same header after the same flag occurrence, or the same index in a list of the same shape), else it is left empty and listed, or the item is left out when it cannot work without it (a shell command that would run differently included); an argument list with an element it could not restore is never imported in part: an agent profile, an agent automation or the terminal keeps this computer's list whole (`argsNotImported`, named in the preview with the reason), and a new automation is left out. The Rust cleaner never shortens a list either: one over 256 elements or with an element that is not text, over 64 KB or has a NUL is left out whole on export and import (the item carries `argsLeftOut` with the reason, the terminal's is named in `argsLeftOut` of the read and export results); plan again before applying (refused if any of that moved), back up to `<data>/backups/config-<ms>` (newest 5), forget the saved password of every replaced host whose user, host name, port or jump changed (first, so no failure or crash leaves a password bound to a new address; never undone, and a failure says so; a forget that fails stops the import before anything is saved), then hosts (compare-before-write) → app settings (re-planned from the current settings, saved at once) → automations (`automation.save` over the app connection, always `enabled: false`; one the daemon refuses is skipped and reported, while a lost daemon, or one that kept the automation in memory although it reported a failure, compared on every field the import saves, fails the import). A moved host's port forwards are removed and agent forwarding is off; a file never turns agent forwarding on. Applied steps are undone in reverse on failure (a replaced automation's own approval is restored by its hash). No daemon change. |
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
