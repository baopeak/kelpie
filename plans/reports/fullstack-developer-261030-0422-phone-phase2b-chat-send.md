# Phase 2b report: send chat messages from the phone (single writer)

Branch `phone/phase2b-chat-send`. Nothing staged or committed. API_LEVEL 17 -> **18** (`PHONE_API` and the new `CHAT_QUEUE_API` in `app/src/lib/attention.ts`).

## What changed

**Daemon (`crates/kelpied`, `crates/kelpie-proto`)**
- `chat/queue.rs` (new): per-chat FIFO (20 entries, 5 per phone, 16 KiB phone / 1 MiB desktop text), `(device, cid)` memory for 10 min (bounded 2000), cancel / front / take / drop, stop coalescing, `view()` for events.
- `chat/turns.rs` + `chat/turn-fixtures.json` (new): dialect from `spec.kind`, thread/session/turn id tracking (`kelpie-thread`, `kelpie-session*`, `turn/started|completed`, `thread/start|resume`, `session/new|load` invalidate), the three user-turn builders, interrupt builders, turn/interrupt detection, command refusal.
- `chat.rs`: queue + pump (gate: alive, not held/paused, no turn in flight, agent idle, conversation known), in-flight rules (Idle line, error to the turn's request id, exit, 30 s watchdog in `tick`), observed raw turn lines and interrupts (never blocked), `kelpie_sent` logged before the stdin write, offer / `write_turn` / fallback (plain text built by daemon, else `needs_window`), stop (Claude / Codex / ACP incl. cancelled answers through `resolve`), `-32601` for ACP client requests with no window, queue dropped on exit and `close_stdin`, `chat.queue` on attach / observe / every change, `queue_rpc` for the desktop methods.
- `daemon.rs`: `chat.send|write_turn|queue_op` arms, `drop_device_queues` on `mobile.revoke` (before the socket closes) and on `mobile.set_write` to read-only. `auth.rs`: no list change needed (session roles never had `chat.*`); added to the user-only test.
- `mobile/verbs.rs`: `chat_send|chat_cancel|chat_stop|chat_resume` (write). `mobile.rs`: verb arms, token-bucket rate limit (1/s, burst 3), `chat_queue` translation with `mine`, per-batch coalescing of `chat_queue` in `adapt`.

**Desktop (`app/src`)**
- `chat/chat-queue.ts` (new): queue view types + converters. `transport.ts` / `desktop-transport.ts`: `enqueue`, `writeTurn`, `queueOp`, `chatSend`, `stop`.
- `chat-model.ts`: daemon path behind `daemonQueue()` (`send` -> `enqueue`; `turnReady` -> capture -> `writeTurn`; `applyQueue` mirror; `unqueue` / `sendNow` / `resumeQueue` -> `queueOp`; `kelpie_sent` handling and the "from {device}" label). Legacy path untouched below API 18.
- `acp-adapter.ts` (`showsSent`, no local item on the daemon path), `codex-adapter.ts` (`ctx.requeue`), `mode-switch.ts` (`hold` before interrupt / `close_stdin`, released if the stop fails), `chat-composer.tsx` and `chat-pane.tsx` (origin labels, event listeners), `store.ts` (two event cases).
- Tauri bridge: no Rust change. `daemon_call` and the `daemon-event` forwarder are method / event agnostic.

**Phone (`app/phone`)**: `chat-send.ts` state machine, `ChatComposer` (Enter = newline, Send / Ctrl+Enter, Stop, Resume, chips with Edit / x, Retry / Discard / Put back), `link.ts` verbs + `chat_queue`, `phone-state.ts`, EN + VI strings, CSS.

**Docs / plans**: `docs/architecture.md` (RPC and event lists, verb table, never-shed list, phone chat, new "The message queue (API 18)" section). Plan: phase 1 never-shed list, `plan.md` D13 line, phase 3 dedupe windows (`chat_send` 10 min).

## Verification

| Check | Result |
|---|---|
| `cargo test -p kelpie-proto -p kelpied` | proto 23 pass; kelpied 417 pass, 1 fail: `agents::tests::concurrent_hook_settings_are_published_as_complete_json` (not touched here; passes 3/3 alone, passed in the earlier full run: file-replace race) |
| new Rust tests | queue.rs 14, turns.rs 5, chat.rs 22, mobile.rs 6 (verbs / limits / L3 never-shed / revoke / `chat_queue` translation), verbs.rs table updated |
| clippy `-p kelpied -p kelpie-proto --all-targets` | clean |
| `pnpm typecheck` | clean |
| `pnpm test` | 51 files, 619 tests pass (new: `turn-lines` 3, `chat-queue` 14, phone `chat-send` 12, link +3) |
| `pnpm i18n:check` | 2202 keys, 0 missing |
| `pnpm build:phone` | ok (index 117.85 kB) |
| `mobile.py` x2 back to back | 70/70, 70/70 |
| `mobile_protocol.py` | 124/124 |
| `mobile_resilience.py` (full, not `--quick`) | 55/55 |
| `acp_chat.py` | 56/56 (replay of user prompts from `kelpie_sent` included) |
| `composer.py` (pictures: rich entries) | 85/85 |

New e2e coverage: `mobile_protocol.py` section 8b (idle send once, busy send waits, FIFO phone + desktop with a window answering `turn_ready`, dup, cross-cancel, stop pauses / resume, revoke drops, limits, read-only, window detached / closed, plain text vs picture, ACP prompt via `kelpie_sent`, ACP stop cancels a permission), `mobile.py` (box, Enter newline, send, desktop chip with origin, transcript with origin, Stop, desktop and phone cancel, read-only chat has no box), `mobile_resilience.py` (lost reply -> `dup`, blackhole -> lands once, one turn at the agent). Shared fake agent: `phone_support.write_turn_agent`.

## Deviations from the spec

1. `docs/user-guide.md` **not edited**: it has another session's uncommitted hunks and is on the do-not-touch list. Suggested text for the Phone access section: "In a chat, type a message and tap Send (Enter adds a line). It waits in the chat's queue, shown on the computer and the phone with who sent it, and goes out when the agent is idle, also with the Kelpie window closed. Stop interrupts the agent and holds the queue until you tap Resume. Codex commands (`/compact`, `!ls`...) and `/new` must be run on the computer."
2. Idle gate = the agent's own lines (`hook` none or `Idle`), not `Activity::status`, which reports `Working` for 1.5 s after any unclassified output (a fresh agent's handshake).
3. Added to `chat.queue_op`: `hold` takes `held:false` to release (a failed hand-off would otherwise block sends for good); `front` takes `interrupt:true` so the adapter's interrupt after "send now" does not pause the queue. `kelpie_sent` is written only for lines that start a turn: a command the window runs itself (`/new`) is not a message.
4. Daemon also refuses ACP `/new`, `/clear`, `/reset` from the phone (`command_needs_desktop`), not just the phone UI.
5. Phone state `lost` (computer restarted): the message stays on screen with its text (Put back / Discard) instead of vanishing, so text is never dropped silently. A cancelled / dropped `dup` is a refusal that puts the text back.
6. `types.ts` untouched: queue types live in `transport.ts` / `chat-queue.ts`.
7. Phone bundle is embedded in the daemon: any phone change needs `pnpm build:phone` then a daemon rebuild before e2e (I hit this once).
8. Not run: `chat.py`, `chat_ui.py`, `chat_parity.py`, `chat_settings_sync.py`, `agent_resume.py` (real signed-in agents, model calls, TUI sessions); `stress.py` (30 min) and `perf_gate.py`; manual iOS / Android Telex / VNI pass. The legacy desktop queue path is covered by the existing vitest files, all green below API 18.

## Files for staging (`git add -p` needed)

- `app/src/lib/store.ts`: **only** the hunk `@@ -424,0 +431,7 @@` (the `chat.queue` and `chat.turn_ready` cases). The other hunks are another session's.
- Everything else I touched had no foreign hunks: `chat-composer.tsx`, `chat-pane.tsx`, `attention.ts`, `locales/vi/composer.json` and `phone.json`. Do not stage `types.ts`, `styles.css`, `settings.json`, `shell.json`, `user-guide.md`.
- Suggested split: commit 1 daemon (`crates/**`, `docs/architecture.md`, plan edits, `scripts/e2e/{mobile_protocol,mobile_resilience,phone_support}.py`); commit 2 desktop (`app/src/chat/**`, `chat-composer.tsx`, `chat-pane.tsx`, `store.ts` hunk, `attention.ts`, `composer.json`); commit 3 phone (`app/phone/**` including `link.ts` / `link.test.ts`, `phone.json`, `mobile.py`). `attention.ts` (`PHONE_API = 18`) goes with commit 2; `link.test.ts` reads it, so commits 1 and 2 stay green on their own.

## Unresolved questions

- A rich desktop entry whose chat pane is closed (detached) before its turn is dropped as `needs_window`; the "Not sent" note goes only to attached clients, so it is lost if the pane is closed. Wanted, or should the daemon keep it until the pane returns?
- `chat_send` on a Claude TUI-only slash command is refused only by the phone (the daemon cannot know the list). Fine?

Status (round 0): DONE_WITH_CONCERNS

## Fix round 1

**Read this first: the code is written but only partly verified.** The Bash tool stopped returning any output in the resumed session (even `echo hi`, also from a delegated agent), so after the daemon edits I could not build or run anything. What ran green before that: `cargo test -p kelpied chat::` (63 pass) with all of the Rust daemon changes except the tests listed as "not compiled" below. A later `cargo test` died in rustc itself (`STATUS_STACK_BUFFER_OVERRUN`, "invalid metadata for serde", i.e. a toolchain crash, not a code error). Nothing in the frontend, the e2e scripts, or the extra Rust tests was typechecked or run. Before committing, run exactly: `pnpm typecheck && pnpm test && pnpm i18n:check && pnpm build:phone`, `cargo build -p kelpied`, `cargo test -p kelpie-proto -p kelpied`, clippy, then `mobile.py` x2, `mobile_protocol`, `mobile_resilience`, `acp_chat`, `composer`. I started no processes in the resumed session; none of mine were left from before (the last ones were stopped in round 0; I could not re-list them, so check for `kelpie-p2b*` endpoints and ports 1427 / 4444 / 9515).

### Done, by item

1. **W1** `chat-model.ts`: `queued()` is `mirrored || daemonQueueOn()`, so once a queue update arrived the view never flushes locally; `mode-switch.ts` no longer reads `connected()`. A send that cannot reach the daemon shows "Not sent (…)" with its text. Tests: head of a mirrored queue is not sent by the legacy path (agent calls stay empty), typed text goes to `enqueue`, a failing `enqueue` leaves a note with the text.
2. **W2 and needs_window.** Daemon (`chat.rs`, `queue.rs`): a rich entry (or a command) whose window is detached, declined, or slow stays at the head with `needs_window: true` (in `chat.queue` and the phone's `chat_queue`), cancellable by anyone, re-offered when the window attaches (`attach` pumps) and every 15 s while attached. It is held only while the queuing client's connection lives: `unregister_client` calls the new `client_gone`, after which plain text is written by the daemon and the rest is dropped as `needs_window` with a `kelpie_unsent {qid,cid,by,name,text,reason}` line in the log. `chat.write_turn {lines:null, retry:true}` is "not now" (replaying, loading history, not attached): asked again after 1 s, never dropped (`decline_turn`); a plain `null` is "I do not have it". Desktop: `turnReady` declines with `retry` while not live / replaying / loading / not ready; `kelpie_unsent` shows one note (same id as the live `dropped` note). Phone and desktop chips show "needs the desktop window".
   - *Does a common action detach a chat pane for more than a tick?* **Yes.** `app.tsx:150` mounts only the active project (`Show … keyed`) and `workspace-view.tsx:153` only its active tab, so a project or tab switch unmounts `ChatBody`, whose cleanup calls `ctrl.detach()` (session.detach) for as long as the user is elsewhere. Splits and moves remount panes briefly (an attach round trip is longer than 250 ms). So the hold (instead of dropping) matters in everyday use, and so does re-offering on attach.
   - *Do shipped ACP agents send chunks after a turn's final result?* **No evidence of it in the repo**: `classify` documents only usage / title / plan / mode / command-list updates after the stop reason, and `fake_acp_agent.py` sends only a `usage_update` there (classified as no change). I cannot check real Gemini / OMP builds. W7's 60 s rule is the safety net if one does.
3. **W3**: `Adapter.snapshot()` (Codex: thread / turn ids, `threadReady`, `shown`, conversation id, pre-thread queue; ACP: session id, last chunk kind, pre-session queue); the controller also removes the items the build added and restores `busy` (not when the refusal is `busy`: a real turn started). The daemon now accepts lines when only `paused` changed since the offer (an offer is a commitment); a started flight still refuses. Tests: Claude (bubble + busy, re-offer builds one bubble, `busy` refusal), Codex `/new` (conversation id and thread restored, next turn uses `T1`), ACP `/new` (next prompt uses `S1`).
4. **W4**: `cancel_by` lists the entry in `dropped` with `reason: cancelled` and `cancelled_by` when the canceller is not the sender (a self-cancel is silent). Phone: new state `cancelled`, "Cancelled on desktop" / "Cancelled on phone", Put back / Discard, text kept; its own ✕ forgets the chip (`forgetQueued`). Desktop: "Not sent (cancelled from {device})" for its own message cancelled by a phone; none when it cancelled it itself; none on a phone's controller. Rust tests (daemon event, phone translation) and vitest.
5. **W5**: `SendResult` refusals carry `text`; the Retry handler calls `putBack(r.text)`; a retry with no connection leaves the chip as it was. Tests added.
6. **W6**: `enqueue_if(new, allowed)` re-reads the permission under the chat's lock (`Refused::ReadOnly`, `read_only`); the daemon now revokes / downgrades first and drops the device's entries after. Test: a connection granted Write whose record says Read, and a revoked one, leave nothing queued or written (`mobile.rs`, not compiled yet).
7. **W7**, chosen rule (documented in the code and in `architecture.md`): (a) a turn that is **not in flight** and whose last classified line is `Working`, silent for 60 s, is idle for the queue only; a turn in flight (ours or a raw one) and `Waiting` are never taken as idle, so a real long turn is not touched and no second turn goes into it; (b) after a stop (own or observed interrupt), an agent that says nothing classified for 5 s is not running anything: the flight is cleared. A Stop that cannot build a line (Codex before `turn/started`) still pauses a non-empty queue (S3). Tests: late chunk, flight and question not stuck, stop answered / unanswered.
8. **O1**: `refund_send` after any refusal by the chat; the limiter is unchanged for what gets in. Test: 8 refusals leave the full burst, then the 4th valid send is `rate_limited`; a full queue costs nothing.
9. **O2**: "Open this chat on the desktop to resume it." (phone composer refusal) and "This chat is not running. Open it on the desktop to resume it." (no composer), EN + VI; the daemon's `exited` message says the same.
10. **Suggestions applied**: S1 (no stdin = `exited`), S2 (a queued message is not forgotten after 10 minutes), S3, S5 (= O1), S7 (a restart seen during a send stays `lost`), S8 (`onSent` also on replay, test updated), `mobile.disable` drops every phone's waiting messages (part of S6), Rust also trims U+FEFF, `adapt` batches 16 frames not 64 (part of S10), the test agent reads / writes UTF-8 (tester F2) with a Vietnamese case in `mobile_protocol.py`. **Skipped**: S4 (the spare interrupt is bounded to 5 s and needs adapter feedback); S6 narrowing of projects (no per-project rule exists for phone sends); S9 (parsing outside the lock is a larger refactor; lines over a few MiB are rare); the rest of S10 (credit is released per batch of 16, backpressure order is unchanged); S11 (not a regression).
11. **E2E**: `mobile.py` now checks "Cancelled on desktop" and Discard on the phone, and ends with a self-contained scenario (re-enable, pair, queue two messages, restart the daemon, expect a lost chip with the text, Put back refills the box). `mobile_protocol.py`: the picture now waits visibly with `needs_window` and the phone cancels it; a Vietnamese message arrives intact. **Not run.**

### Not compiled / not run (needs a working shell)
Rust: `mobile.rs` new tests (4), `queue.rs` stale-queued test, and the two small edits to the cancel test. Frontend: every TS / TSX change above, `chat-queue.test.ts`, `chat-send.test.ts`. Python: the three e2e scripts. Expect small type fixes at worst; the logic was walked through by hand for each test.

Status: DONE_WITH_CONCERNS
Summary: All eleven fix items are implemented in code, tests and docs; the daemon part was compiled and its chat tests passed (63) before the shell stopped answering, and the frontend, remaining Rust tests and e2e were not run.
Concerns/Blockers: Verification is outstanding (list above); with `Bash` dead I also could not re-list processes, so check for leftover `kelpie-p2b*` daemons, vite (1427) and tauri-driver (4444). Never touched PID 10588, app 8836 or python 16936.

## Fix round 2

Inputs: re-review 1 (R1–R4), the tester's PowerShell retest (B1, B2, T1, O4, O5, F3) and the user's 2026-10-30 decision on R4 (strict order). All commands ran through PowerShell. Nothing staged or committed.

### What changed, by item

1. **R1, declined message held forever** (`chat.rs`).
   - A plain `null` from the window is final. A non-plain head the window declined is dropped right away: `dropped` with `reason: "declined"`, plus a lasting `kelpie_unsent` line with the same reason. The new `unsent()` helper writes both the log line and the event entry; `needs_window` and `cancelled` use it too.
   - `attach` no longer clears `declined`, so the message is never offered again.
   - Plain text the window declines is still written by the daemon.
   - `write_turn` with no lines now goes through `decline_turn`.
   - Desktop (`chat-model.ts`): an adapter that throws gives a final `retry: false` with a single `console.warn`, and its partial build is rolled back. The "not now" cases (replay, history load, not live or not ready) keep `retry: true`.
2. **R2, fake Idle after a stop** (`chat.rs`, `queue.rs`).
   - The after-stop rule no longer calls `activity.on_hook` and touches nothing outside the queue. It sets a queue-only `q.released`, which `idle()` reads and the next classified agent line clears.
   - `STOP_GRACE` went from 5 s to 30 s. Release now needs no output at all since the stop (`last_output <= stopped`).
   - New gate: while a stop is pending (`stopped_at`), `idle()` is false until the stopped turn's `Idle` line. No message goes into a turn that is still stopping (Codex errors on that).
   - An interrupt seen while nothing runs no longer sets `stopped_at`, so a stray interrupt does not hold the queue for 30 s.
   - Tests:
     - A silent stop is released only after 30 s. The hook stays `Working` and `turn_seq` is unchanged.
     - A stop the agent acknowledged with output is not released. The queued message goes out only after the turn's `result`.
3. **R3, cancel note for a detached pane.**
   - Daemon: when another device cancels a desktop entry, a `kelpie_unsent {reason: "cancelled", cancelled_by: {id, name}}` line is logged.
   - Desktop: the log line produces the same note as the live event, with the same `unsent-{qid}` id, so the two never show twice.
   - The note shows only on a non-observer window and only for `by: "desktop"`. A phone's own messages are told live, as before.
4. **R4, held message in strict order (user decision).**
   - Phone (`composer.tsx`):
     - Every queued phone message behind a `needsWindow` head gets a `.chip.held` style and the note "A desktop message with an attachment is waiting for the desktop window".
     - The note has a **Skip** button (`data-skip`) that cancels the held head, right on the phone's own chip.
   - Desktop:
     - The daemon adds `SessionInfo.waits_for_window` (`session.rs`; set in `chat.rs` `info_locked`, skipped when false). `tick` re-broadcasts it through `session.updated`.
     - New `app/src/components/chat-wait-notice.tsx` is mounted in `app.tsx`. It reuses the `.toast` look with inline styles, because `styles.css` is foreign. It lists every chat with that flag whose pane is not visible, with an **Open chat** button (`store.revealSession`).
     - `types.ts` is untouched; the component reads the field through a local type.
   - i18n:
     - Phone strings in `vi/phone.json`.
     - Desktop strings in `vi/composer.json`, which has no foreign hunks. `shell.json` and `settings.json` are not touched.
     - "Skip", "Copy" and "Copied" reuse existing keys.
5. **B1, status lines never update.**
   - `ChatSender` never changes a message in place: every change makes a new object (`patch()`), so `<For>` redraws the row.
   - `chipView(p)` (pure) decides warn, Retry, Put back and Discard.
   - New vitest test: it maps the sender's list with Solid's reactive `mapArray`, exactly as `<For>` keys rows, and checks Sending→unknown shows Retry, then Retry→Sending.
   - The test imports `solid-js/dist/solid.js` explicitly, because the Node test environment resolves `solid-js` to the server build, which has no reactivity.
   - I checked that the test fails against an in-place mutation, then reverted that probe.
6. **B2, lost message after a daemon restart.**
   - New `PendingChips` in `composer.tsx`, used by the composer and by a new `NotRunning` block in `chat-view.tsx`.
   - A chat that is not running, or a read-only device, now shows the lost, unknown or cancelled messages with their full text (selectable, wrapping), **Copy** and **Discard**. They sit above the O2 line "This chat is not running. Open it on the desktop to resume it."
   - Retry and Put back appear once the chat runs again (the composer is back).
   - Persistence:
     - `ChatSender` saves the messages that need a decision through `deps.save` to `kelpie-draft:chat-pending:<chatId>` (`phone-state.ts`) and restores them when the chat's sender is created.
     - The `kelpie-draft:` prefix means `wipeTypedText()` clears them on forget or unpair. `forgetSenders()` also disposes the senders in memory, so a late reply cannot save text again.
7. **T1, pairing 429 in `mobile.py`.**
   - The bad-pairing-code checks moved to the very end, after the restart scenario. The limiter is unchanged.
   - The restart scenario now checks:
     - the lost chip and its hint in the stopped chat;
     - Copy;
     - that a page reload keeps the chip;
     - that Discard removes it, including the stored copy.
8. **Small items.**
   - Re-offer cadence: the new constant `REOFFER_EVERY = 30 s` is counted from the offer: 15 s to answer, then 15 s before the next offer. Code, `docs/architecture.md` and this report all say **every 30 s**; the "every 15 s" in fix round 1 was wrong.
   - O5: `decline_turn` uses `announce_if`, so a "not now" that changes nothing visible announces nothing. Covered by a Rust test: 3 declines, 0 events to a watching phone.
9. **F3, the `mobile.py` flake** (found while running `mobile.py` ×2).
   - Root cause: the daemon answered `rate_limited`. On an idle host the script sent 4–5 messages from one phone within a few seconds. The per-device limiter allows a burst of 3, then 1 a second, and it was right.
   - Fix: `phone_send` now spaces sends by at least 1.1 s, waits for Send to be enabled, and prints a `NOTE` with the notice, the button state and the `chat_send` frames if a send does not take.
10. **Docs**: `docs/architecture.md` (message queue section: decline is final, the 30 s cadence, strict order with the phone Skip and the desktop notice, `waits_for_window`, the `declined` and `cancelled` log lines, and the stop gate with the 30 s queue-only release).

### Verification

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 51 files, **638 / 638** (new: `chat-send` redraw and persistence tests, `chat-queue` adapter-throw and cancelled-note tests) |
| `pnpm i18n:check` | 2213 keys used, 0 missing |
| `pnpm build:phone` | ok, then `cargo build -p kelpied` (`kelpied.exe` 10:37, after the bundle) |
| `cargo test -p kelpie-proto -p kelpied` | proto 23 / 23; kelpied 432 pass, 1 fail: `mobile::tests::a_message_whose_enter_was_skipped_says_so`, a terminal-input timing test (80 ms typist against a 250 ms Enter delay) that this round did not touch. It passed 3 / 3 when run alone. `chat::` is 65 / 65. |
| `cargo clippy -p kelpie-proto -p kelpied --all-targets` | 0 warnings |
| Desktop debug app | `pnpm tauri build --debug --no-bundle`, `kelpie.exe` 11:12 |
| `mobile.py` ×2 back to back (runs 8 and 9) | **84 / 84, 84 / 84**, restart lost-chip scenario included. Earlier runs: 84 / 84 (runs 1 and 3); runs 2, 4, 5, 6 and 7 hit F3 (rate limit, diagnosed above) before the pacing fix. |
| `mobile_protocol.py` | **130 / 130** (new: the cancel note in the log; a declined picture dropped as `declined`, with its note, and never reaching the agent) |
| `mobile_resilience.py` (full) | **55 / 55** |
| `acp_chat.py` | **56 / 56** |

Isolation: every GUI run went through a scratchpad runner that sets `KELPIE_ENDPOINT=kelpie-p2b-r2-gui-<n>-<pid>`, a temp `KELPIE_DATA_DIR`, `KELPIE_LOAD_FORCE=L0:calm` and `KELPIE_DAEMON_PATH` only in the environments of tauri-driver and the script. Afterwards it kills tauri-driver as a tree, shuts the isolated daemon down through its own endpoint, and deletes the temp folder. `mobile_protocol`, `mobile_resilience` and `acp_chat` isolate themselves. No real agents and no model calls.

### Files with foreign hunks (stage with `git add -p`)

- `app/src/components/app.tsx`: mine are **only** the hunks `@@ -25,6 +26,7 @@` (`import { ChatWaitNotice }`) and `@@ -157,6 +159,7 @@` (`<ChatWaitNotice />` after `<StatusBar />`'s parent `div`). The `HoverHints` hunks are another session's.
- `app/src/lib/store.ts`: unchanged this round. Still only the `chat.queue` / `chat.turn_ready` hunk from round 0.
- These have no foreign hunks: `crates/kelpied/src/session.rs` and `notify/tests.rs` (new field only), `chat-wait-notice.tsx` (new), `locales/vi/composer.json`, `locales/vi/phone.json`, `app/phone/**`, `app/src/chat/**`, `docs/architecture.md`, `scripts/e2e/mobile.py`, `scripts/e2e/mobile_protocol.py`.
- Suggested commit split: `session.rs` and `notify/tests.rs` go with commit 1 (daemon). `chat-wait-notice.tsx`, the `app.tsx` hunks and `composer.json` go with commit 2 (desktop).

### Unresolved questions

- On a phone, the desktop notice and the held note say "attachment". A held head can also be a desktop *command* (`/compact`), which is non-plain but has no picture, so the wording is slightly off in that case. Keep the user's wording, or say "a desktop message only the computer can send"?
- `docs/user-guide.md` is still not updated (foreign hunks). Suggested addition: "A desktop message with a picture waits for its chat to be open on the computer; messages behind it wait too. Skip on the phone, or Open chat on the computer's notice, lets them go."
- The Claude-only steer gap from re-review 1 (a steer written during a flight that Claude runs as a separate turn) is unchanged, as the reviewer allowed (document it, or count observed turn lines).

## Fix round 3

Inputs: re-review 2 (R2-W1, R2-S1, R2-S2) and the controller's F4 request. The controller's held-queue wording is kept as is ("only the computer can send" on the phone, "can only be sent with its chat open here" on the desktop). PowerShell only. Nothing staged or committed.

### What changed, by item

1. **R2-W1: a phone message cancelled on the desktop while the phone was offline.**
   - Daemon (`chat.rs` `cancel_by`): *every* cross-device cancel is now logged as `kelpie_unsent {reason: "cancelled", cancelled_by}`, phone messages included. A self-cancel is still silent.
   - Shared chat model (`chat-model.ts`): a new `onUnsent` option is called for every `kelpie_unsent` line, live and on replay, with `cid`, `reason` and `cancelledBy`.
   - Phone (`phone-state.ts` → new `ChatSender.unsentSeen`): a message of this phone that the log says was cancelled becomes `cancelled` ("Cancelled on desktop" or "on phone") with its text, also when the queue snapshot had already marked it `sent`. Any other reason makes it `lost`. A settled message is left alone, so a replayed line changes nothing.
   - The desktop's own view still shows a note only for its own (`by: "desktop"`) messages.
   - Tests:
     - Rust: `a_phone_that_was_offline_when_its_message_was_cancelled_finds_out_from_the_log` (a delta re-subscribe carries the note with the phone's `cid`), and the cross-cancel test now expects two notes.
     - vitest: the sender goes from sent to cancelled from the log, survives `prune`, and ignores replays and other `cid`s. The controller passes `onUnsent` on replay.
     - `mobile.py`: the phone goes offline through the browser's `offline` event, the desktop cancels the queued message, and after reconnecting the chip says "Cancelled on desktop" with the text. It is still there 6 s later (not taken for sent), and it never reached the agent.
2. **R2-S1:** a phone's view now says "Not sent (the computer could not prepare it)" for `declined` and `needs_window`. The desktop keeps "this window". The vi string is in `vi/composer.json`; the phone bundle picks it up from there.
3. **R2-S2: bounded kept copies.**
   - New `app/phone/src/kept-messages.ts`:
     - keeps at most 20 per chat and 60 in all, the newest first;
     - drops the keys of chats missing from the computer's list on `hello` and `sessions` (an empty list drops nothing);
     - leaves no stale copy behind when a write fails.
   - `storage.ts` gets `keep()`, which reports whether the browser took the value instead of quietly keeping it in memory, and `keys()`.
   - `ChatSender.save` returns the `cid`s it could not keep. Those messages get `unsaved` (never stored), and their chip shows a Copy button and the alert "This phone could not save a copy: copy the text now, or it is lost if the page reloads." The vi string is in `vi/phone.json`.
   - Tests: `kept-messages.test.ts` (per-chat cap, total cap, full storage, cleanup) uses a fake `localStorage`; `chat-send.test.ts` checks the `unsaved` flag.
4. **F4: the `mobile.py` Ctrl+C quick-key check. Cause: the test's ordering, not a product bug.**
   - What the check did: the phone tapped Ctrl+C, the script slept 1.5 s, then typed `echo after-ctrl-c` through the *desktop's* `session.input`, and expected it on screen. The two writes reach the PTY by different paths (the phone link, and a desktop RPC), and nothing orders them.
   - Evidence, from my own probe against an isolated daemon, PowerShell in ConPTY, writing the bytes directly:
     - Ctrl+C written, then `echo` 0.3 s or 1.5 s later: the echo ran **18/18** times. Ctrl+C stops `ping` in under 0.3 s.
     - `echo` written **0.3 s before** the Ctrl+C: it failed **4/4**, with exactly the reported signature. The screen shows `ping` stopped ("Control-C", so the byte arrived), but the echo never runs, because the console throws away typed-ahead input when it handles Ctrl+C. The same happens with a real keyboard.
     - So when the phone's round trip takes longer than 1.5 s (a busy host, or a WebView2 timer delay), the echo lands first and is dropped, although the phone's Ctrl+C worked.
   - The phone path itself is plain: the `key` verb → `input_by` → the PTY write channel, the same channel the desktop uses. Nothing coalesces or drops it.
   - Fix (`mobile.py`): the check now waits until the screen shows the running command stopped ("Control-C" on Windows, "^C" elsewhere), which proves the phone's Ctrl+C reached the PTY. Only then does it type the next command, and it checks the shell runs it. That is now two checks instead of one.
   - Result: **3/3** green in this round's `mobile.py` runs.
5. **Another test race found while verifying.** It is fixed in the test script; no product change was needed.
   - `mobile_protocol.py`, "in the order the daemon got them": the desktop's `chat.send` goes out from the Window thread's outbox, and the phone's next send was not ordered after it. In one run the phone message reached the daemon 270 ms earlier. The script now waits until the daemon lists the desktop message before the phone sends again.

### Verification

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 52 files, **645 / 645** |
| `pnpm i18n:check` | 2215 keys, 0 missing |
| `pnpm build:phone` → `cargo build -p kelpied` | ok (`kelpied.exe` 12:03, after the bundle) |
| `cargo test -p kelpie-proto -p kelpied` | proto **23 / 23**, kelpied **434 / 434** (`chat::` 66 / 66) |
| `cargo clippy -p kelpie-proto -p kelpied --all-targets` | 0 warnings |
| Desktop debug app | `pnpm tauri build --debug --no-bundle`, `kelpie.exe` 12:13 |
| `mobile.py` ×3 back to back | **88 / 88, 88 / 88, 88 / 88** (tauri-driver started with `--native-driver D:\Works\edgedriver\msedgedriver.exe`) |
| `mobile_protocol.py` | **130 / 130** on the last run (see below) |
| `acp_chat.py` | **56 / 56** on the last run (see below) |

Earlier runs of `mobile_protocol` and `acp_chat` this round, all while the host sat at 59–100% CPU with 0.3 GB free memory (the user's IDE and Edge, not my processes):
- `mobile_protocol` run 1: 126 / 130, the FIFO race fixed in item 5.
- `mobile_protocol` run 2: 129 / 130, "a write by id lands in the right session after handles moved". That is a terminal restart check: it types as soon as "PS " shows, before PowerShell is ready.
- `acp_chat`, three earlier runs: 55 / 56 each.
  - Twice the status sampler (a 0.25 s poll through WebDriver) missed the short first `working` before the permission request.
  - Once the permission card was read mid-render (an empty title).

None of these is in chat-queue code this round touched, and the session status code was not touched. They passed on rerun, and in round 2.

Isolation and cleanup:
- Every GUI run went through the scratchpad runner: its own `KELPIE_ENDPOINT` and temp `KELPIE_DATA_DIR` in the child environments only, tauri-driver killed as a tree, and the isolated daemon shut down through its own endpoint.
- The Ctrl+C probe used `phone_support.isolate`, so it had its own daemon.
- Afterwards only PIDs 8836 and 10588 remain. My temp folders (`kelpie-p2b-r3-ctrlc-*`, `kelpie-phone-protocol-*`) are removed.

### Staging notes (in addition to round 2's)

- No new foreign hunks. `app.tsx` is unchanged this round: still only `@@ -25` and `@@ -157` are mine.
- New files: `app/phone/src/kept-messages.ts` and `kept-messages.test.ts` (commit 3, phone).
- `vi/composer.json` gains one key (commit 2). `vi/phone.json` gains one key (commit 3).

### Unresolved questions

- Test flakes under heavy load that are not this phase's: the `acp_chat` status sampler, which can miss a short `working`, and `mobile_protocol`'s "handles moved" check, which types right after "PS " shows. Harden them in a separate, test-only change?
- The R2-S3 desktop notice showing in every app connected to one daemon is still open (low priority, per the reviewer).

## Follow-up after commit

Phase 2b is committed as `4a1224a`. Two separate follow-ups, neither staged nor committed. Rust and the desktop app are unchanged in both.

### 1. R3-W1: the lost messages of a resumed or closed chat are kept and shown (product)

- **Cause** (re-review 3): resuming a stopped chat starts a new session and closes the old one. The next session list then made `forgetGoneChats` delete that chat's kept copies at once, and no screen showed a chat that was gone anyway.
- **`kept-messages.ts`:**
  - **Stored form:** each chat now stores `{v: 2, name, savedAt, items}`. A plain list from the first build is still read; its `savedAt` is the newest `at`. `name` is the chat's label when it was last saved, and a save that no longer knows the name keeps the stored one.
  - **Pruning:** `forgetGoneChats(chats, now)` drops a gone chat's copies only once they have not changed for **7 days** (`GONE_KEEP_MS`). An empty list still deletes nothing. The caps (20 per chat, 60 in all) still bound storage.
  - **Listing:** new `goneChats(chats)` lists the chats with kept messages that the computer's list no longer has. It takes the daemon's list as given, even an empty one, because listing deletes nothing.
  - **Discarding:** new `discardKept(chat, cid)` removes one message. The rest keep their week.
- **`phone-state.ts`:**
  - It saves with the chat's name (`labelOf(row)`).
  - A `notSent` signal is refreshed on `hello` and `sessions`.
  - `discardNotSent` also tells the sender that holds the message on this page, so it is not saved again.
  - Unpair and forget clear the list.
- **Inbox (`inbox.tsx`):** a **"Not sent"** section under the session groups shows each gone chat's name (or "A chat that was closed"), each message's full selectable text, and **Copy** and **Discard**, with the hint "From chats that are no longer running here (closed, or resumed as a new chat). Copy the text to send it again." CSS is in `phone.css`; en and vi strings are in `vi/phone.json`.
- **Successor link: not added.** The daemon exposes no predecessor or successor id: a resumed session gets a fresh id, and `SessionSpec` and the phone's `SessionRow` carry nothing that links the two. The agent's own conversation id (`agent_session_id`) is not sent to phones. A link would need a protocol change, so it is left out.
- **Tests:**
  - vitest (`kept-messages.test.ts`):
    - a gone chat's copies stay and are listed with their name;
    - an empty list deletes nothing but lists both;
    - a save without a name keeps the old name;
    - one discard keeps the rest;
    - pruning happens at exactly 7 days after the last change, never for a listed chat;
    - the first build's plain-list format is read and pruned by its newest `at`;
    - the full-storage case still holds with the new form.
  - `mobile.py` (restart scenario): after the lost message survived a reload, the chat is closed on the desktop. The inbox then lists the message under Not sent with the chat's name, also after another reload, and Discard empties both the section and storage.

**Files (commit 1):** `app/phone/src/kept-messages.ts`, `app/phone/src/kept-messages.test.ts`, `app/phone/src/phone-state.ts`, `app/phone/src/inbox.tsx`, `app/phone/src/phone.css`, `app/src/locales/vi/phone.json`, `scripts/e2e/mobile.py`. None has foreign hunks. The daemon embeds the new phone bundle (`pnpm build:phone`, then `cargo build -p kelpied`); no Rust source changed.

### 2. Two load-sensitive checks waiting on real conditions (test only)

- **`acp_chat.py`, "the status went working, waiting, working, idle":**
  - *Why it flaked:* the timeline was built by polling `session.list` between WebDriver calls. On a busy host one round took longer than the fake agent's 0.8 s of work before it asks for a permission, so the first `working` was missed (`['idle', 'waiting', …]`) although the daemon reported it.
  - *Change:* a `StatusEvents` thread reads the daemon's own `session.updated` events on a connection of its own. The daemon broadcasts every status change from its 250 ms tick, so a 0.8 s phase always appears. `watch_status(sid)` starts the timeline where `TL.clear()` used to, and `track()` only waits for its condition.
  - *What it asserts:* unchanged (working before waiting, working again after it, idle last). It now reads the daemon's account rather than a sample of it.
- **`mobile_protocol.py`, "a write by id lands in the right session after handles moved":**
  - *Why it flaked:* it typed through the phone as soon as "PS " showed after `session.restart`. On a busy host PowerShell prints its prompt before it reads keys, so the line could be lost: `entered: true`, but nothing was on screen within 15 s.
  - *Change:* before the phone's write, the script waits until the restarted shell has evaluated a command of its own, `echo restarted-ready-$((40+2))`, whose output "restarted-ready-42" exists only once the line ran. It is resent every 5 s, for up to 120 s.
  - *What it asserts:* unchanged. The phone's write by id, its `ack`, the text landing in that session, and its absence from every other live terminal.

**Files (commit 2):** `scripts/e2e/acp_chat.py`, `scripts/e2e/mobile_protocol.py`.

### Verification

| Check | Result |
|---|---|
| `pnpm typecheck` | clean |
| `pnpm test` | 52 files, **646 / 646** (`kept-messages` 6) |
| `pnpm i18n:check` | 2218 keys, 0 missing |
| `pnpm build:phone` → `cargo build -p kelpied` | ok (`kelpied.exe` 13:18) |
| cargo test / clippy | not needed: no Rust source changed |
| Desktop rebuild | not needed: no desktop source changed (`kelpie.exe` 12:13 is current) |
| `mobile.py` ×2 | **90 / 90, 90 / 90** (new: Not sent with the chat's name, after a reload, Discard) |
| `mobile_protocol.py` ×2 | **130 / 130, 130 / 130** ("handles moved" passed both times) |
| `acp_chat.py` ×2 | **56 / 56, 56 / 56** (timeline `['working', 'waiting', 'working', 'idle']` both times) |

Isolation as before: the GUI runs went through the scratchpad runner (own endpoint, temp data folder, tauri-driver with `--native-driver`, everything stopped after), and the daemon-only scripts isolated themselves. Afterwards only PIDs 8836 and 10588 run. My temp folders are removed.

### Unresolved

- A link from a gone chat to the chat that resumed it needs the daemon to expose the predecessor id (or `agent_session_id`) to phones: a small protocol addition, if wanted.
