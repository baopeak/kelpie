# Code review: phone phase 2b (chat send through the daemon's single-writer queue)

Branch `phone/phase2b-chat-send` (uncommitted, stacked on 1db592d). Spec: `phase-02b-phone-chat-send.md` + Validation log; `plan.md` D14; developer report `fullstack-developer-261030-0422-phone-phase2b-chat-send.md`.

## Scope
- Daemon: `crates/kelpied/src/chat.rs`, `chat/{queue.rs,turns.rs,turn-fixtures.json}`, `daemon.rs`, `auth.rs`, `mobile.rs`, `mobile/verbs.rs`, `kelpie-proto/src/lib.rs`.
- Desktop: `app/src/chat/{chat-model,chat-queue,transport,desktop-transport,mode-switch,acp-adapter,codex-adapter}.ts`, `chat-composer.tsx`, `chat-pane.tsx`, the `store.ts` `chat.queue`/`chat.turn_ready` hunk, `attention.ts`.
- Phone: `app/phone/src/{chat-send,composer,chat-view,phone-state,link}.ts(x)`.
- Docs and e2e: `docs/architecture.md`, `scripts/e2e/{mobile_protocol,mobile,mobile_resilience,phone_support}.py`.
- Out of scope, as the brief excluded: the other session's leftovers.
- Checks run:
  - `pnpm typecheck`: clean.
  - Focused vitest (`chat-queue`, `turn-lines`, `chat-send`, `link`, `chat-model`, `chat-observer`): 6 files, 122 tests pass.
  - `pnpm i18n:check`: 0 missing.
  - Not run: cargo tests, because the brief allowed only read-only commands, typecheck and vitest. I relied on the developer's 417/418 result for those.

## Score: 8 / 10

The design matches D14 and the code is careful.
- **Single writer:** only the daemon's pump writes queued turns. Raw turn lines are observed and never blocked. Replay never writes, because `write()` returns while replaying and `turnReady` declines.
- **Line building:** the daemon builds phone turns with `serde_json` and puts text only into a fixed shape. One golden fixture is checked from both Rust and vitest.
- **Limits:** they are in bytes. `chat_queue` is coalesced.
- **Verbs:** all four are *write*, and phones get no `front`, `take` or `hold`. The L3 test checks that the verbs are never shed.
- **API:** level 18 is consistent across proto, `PHONE_API`, `CHAT_QUEUE_API` and the docs. No other branch has claimed 18 (checked every local and remote ref).
- **Tests:** most of the new tests check behaviour, not just that code runs.

I found no Critical defect. The Warnings are edge-path problems: text loss or misleading state, and one defensive gap in the single-writer rule. Each fix is small.

## Critical
None.

## Warning

**W1. The desktop can flush the daemon's mirrored queue itself: a second writer.** At `app/src/chat/mode-switch.ts:100` and `app/src/chat/chat-model.ts:535`.
- **What happens:**
  - `queued()` depends on `store.connected() && store.daemonApi() >= 18`, re-read on every call.
  - `applyQueue` fills `state.queue` with daemon entries, including phone ones.
  - Every `set` runs `flushQueue`, which is only skipped when `queued()` is true.
  - If `queued()` is ever false while the mirror is non-empty, the legacy flush calls `adapter.send` for the head (possibly a phone's message) through `session.input`. The daemon observes it as a raw turn, keeps the entry, and writes it again after the turn: **the same message reaches the agent twice**.
- **Typed messages are lost too:** a message typed while `queued()` is false is put in the local queue. It is never flushed once `queued()` is true again, and the next `applyQueue` overwrites it without a word.
- **When it can happen:**
  - at startup, `daemonApi` is 0 until `checkDaemonApi` resolves (`store.ts:1554`);
  - during any `connected()` blip.
- **Likelihood:** low today. A non-empty queue on an idle, unpaused agent is rare at attach, because the pump drains it. Still, this is exactly the invariant this phase exists to protect.
- **Fix:**
  - Once `mirrored` is true, the controller is on the daemon path for good: `queued()` returns `this.mirrored || …`.
  - `flushQueue` returns early when `mirrored`.
  - Drop `store.connected()` from the predicate; `daemonApi` survives reconnects. A send while disconnected then fails in `enqueue` and shows "Not sent (…)" instead of vanishing.

**W2. A "not now" decline is treated as "never", so rich messages are dropped on a transient re-attach.** At `chat-model.ts:649-651` and `chat.rs:887`, then the fallback at `chat.rs:726`.
- **What happens:** `turnReady` declines (`lines: null`) while replaying, while `!live`, while `loadingHistory`, or before `ready`. The daemon sets `declined = true`, and the next pump falls back for good: a rich entry is dropped as `needs_window`, and a plain one is built by the daemon without the Codex picks, skills or `typed` flag.
- **When:** the tick runs every 250 ms. Any re-attach is a window for this: a resync (`kelpie:chat-resync`, which happens under heavy streaming), a remount, or a Claude history load. If the agent's turn ends inside that window, a queued picture message becomes "Not sent".
- **Fix:** send a decline reason.
  - `chat.write_turn {lines: null, retry: true}` for replaying, not live, or loading. The daemon then clears `offered` without setting `declined`, so it offers again on the next pump, and `OFFER_TIMEOUT` still counts from the first offer.
  - Keep the hard decline only for an unknown `cid`.
  - Also call `pump` after `attach`, so a re-attached window is offered its entry at once.

**W3. A `write_turn` refusal leaves the adapter's changes in place.** At `chat-model.ts:653-668` and `chat.rs:892-898`.
- **What happens:** under capture, `adapter.send` has already changed local state before the daemon answers:
  - Claude: the `local-…` pending bubble and `busy = true`;
  - Codex `/new`: `threadId = null` and `threadReady = false`;
  - Codex turns: `busy = true`.
- If the daemon then answers `busy` (a phone's Stop or a raw turn arrived between offer and write), `switching` or `too_late`, nothing is rolled back.
- **Result:**
  - `state.busy` stays true while the agent is idle, so the composer shows Working and Stop until some later turn.
  - The re-offer runs `adapter.send` again, which leaves a duplicate pending bubble.
  - With Codex `/new`, the adapter waits for a thread answer that never comes.
- **Fix:** take a snapshot of `state.items.length` and `busy` before capture. On rejection, remove the items added during capture and restore `busy`.
  - Also consider having the daemon accept the lines when only `paused` changed since the offer: the offer was already a commitment. Keep refusing when a flight started.
  - For Codex `/new`, an adapter `reset` hook or a "dry" flag would be needed. At least document it.

**W4. A message cancelled by the other side is reported as sent on the phone and vanishes on the desktop.** At `chat.rs:822` (cancel announces with an empty `dropped` list), `app/phone/src/chat-send.ts:184`, and `chat-model.ts:672+`.
- **What happens:**
  - The user decision lets the desktop and the phone cancel each other's queued messages. But `cancel` emits `chat.queue` without listing the cancelled entry.
  - On the phone, `snapshot` turns any `queued` entry that is no longer listed into `sent`, and `prune` drops it after 5 s. The phone shows "Sent" for a message that was never sent, and its text is gone.
  - On the desktop, `applyQueue` deletes `mine[cid]` with no note.
- This breaks the spec's "text is never dropped silently".
- **Fix:** list cancelled entries in `dropped` with `reason: "cancelled"` and who cancelled them, at least when the canceller is not the sender. The phone maps that to `lost`/"Put back"; the desktop shows "Not sent (cancelled from {device})". `take` is not affected, because it really is sent.

**W5. The phone's manual Retry throws the text away on a refusal.** At `app/phone/src/composer.tsx:252` and `chat-send.ts:142-155`.
- **What happens:** `deliver` removes the entry on `refused`, and on `dup` with `cancelled` or `dropped`. The submit path is safe, because the box still holds the text. But the chip's Retry handler only sets a notice. The box was cleared when the first try came back `unknown`, so the text is lost.
- This contradicts deviation 5 of the developer report ("a cancelled / dropped `dup` is a refusal that puts the text back").
- **Fix:** in `retry`, call `putBack(p.text)` when the result is a refusal. Better still, return the text inside `SendResult` so that no caller can forget it.

**W6. Race between revoke and send.** At `daemon.rs:1609` and `1616`, and `mobile.rs:1025-1041`.
- **What happens:**
  - `drop_device_queues` runs **before** `mobile.revoke` or `set_write`.
  - The phone's serve task reads `permission()` once per request (`mobile.rs:1268`), then enqueues.
  - A `chat_send` that passes the permission check before the revoke, but enqueues after the drop, stays queued and is written after the device was revoked. Reordering the two calls alone does not close the window.
- **Fix:** in `Verb::ChatSend`, after `enqueue` succeeds, check `self.daemon.mobile.permission(&self.device)` again. If it is no longer `Write`, `cancel` the entry and refuse with `read_only`. Also run the drop **after** `revoke` and `set_write`. The socket close is asynchronous anyway, so the phone still gets the `dropped` event if it is connected.

**W7. A stuck busy state has no way out for the queue.** At `chat.rs:581`, the `idle()` gate.
- **What happens:**
  - The gate trusts the last classified line.
  - Sometimes the last line is a `Working` one (a late ACP `agent_message_chunk` after the stop reason, a Codex `item/*` after `turn/completed`, or a stalled agent). Then `Activity::status` returns `Working` forever, because `output_after_hook` needs output *after* that line.
  - With no flight, the 30 s watchdog does not apply. "Send now" (`front`) and `resume` go through the same gate, and an ACP `session/cancel` with no prompt running draws no reply.
  - The queue, phone messages included, waits until the agent prints something again.
- **Fix:** add an explicit escape. For example, the desktop's "Send now" on an entry while `in_flight == false` passes `force: true`, and the daemon then skips the hook gate. Or treat `hook == Working` with no flight and no output for N minutes as idle *for the queue only*. At least make the phone show "waiting for the agent" (spec risk table) and document it.

## Suggestion

- **S1. `enqueue` takes messages that can never be written** (`chat.rs:799`). After `close_stdin`, if the stop fails, `hold(false)` releases the queue, but `stdin` is `None`, so new sends are queued and the pump never writes them. Refuse with `Exited` when `g.stdin.is_none()`.
- **S2. Dedupe memory forgets messages that are still queued** (`queue.rs:183`). `forget_old` expires by `at`, which is the push time. A message queued for more than 10 minutes behind a long turn, then retried from an old `unknown` chip, becomes a second entry. Skip expiry while the state is `Queued`.
- **S3. A Codex Stop before `turn/started` does nothing and does not pause** (`chat.rs:927`). It returns `Nothing` before `paused` is set. The spec says "either way, a non-empty queue becomes paused", so set `paused` even when no line can be built.
- **S4. A spare interrupt can stay armed** (`queue.rs:301`). `front {interrupt: true}` arms it for 5 s even when the adapter's interrupt writes nothing (Codex without a `turnId`). A real Stop within those 5 s then does not pause the queue. Clear it when nothing was written, or arm it only after the interrupt line is observed.
- **S5. Rate limit:**
  - The comment on `allow_send` (`mobile.rs:231`) says "a request that is refused costs nothing", but `queue_full`, `empty` and `command_needs_desktop` come after the token is taken.
  - Either fix the comment, or check the queue limits first, which would need a dry-run `push`.
- **S6. Some access changes leave phone messages queued.** `mobile.disable` and `mobile.set_projects` narrowing leave the affected phones' messages in the queue, and they are still written after the phone has lost access. Drop entries whose `by` is no longer allowed: all phone entries on disable, out-of-scope chats on a narrowing.
- **S7. A restart during a send ends as `unknown` instead of `lost`** (`chat-send.ts:160`). `epochChanged()` runs before `resendLost` resolves the in-flight send as `unknown`, which overwrites `lost`. The user then sees Retry, which sends a fresh message to the new run. Keep `lost` when the state became `lost` while the send was awaited.
- **S8. The phone's `onSent` is suppressed during replay.** A reconnect that replays a `kelpie_sent` line does not clear the matching `unknown` chip; only the automatic or manual `dup` retry does. Allowing `onSent` during replay would be harmless, because it only clears local chips.
- **S9. Large lines are parsed under the chat lock.** `observe_written` and `commit_turn` parse whole lines while holding `inner`. A Claude turn with pictures is several MB, which blocks `ingest_line` for that chat for milliseconds. Parse outside the lock, or classify from the first 300 bytes when the line is over about 1 MiB.
- **S10. Batching holds credit longer** (`mobile.rs:1295-1319`). `adapt` releases the credit for the whole batch of up to 64 frames only after all of it has been sent, so a busy chat reaches `SUB_BUDGET` and resyncs sooner under load. Release each frame's credit after its own messages are queued, as before; keep only the `chat_queue` coalescing across the batch.
- **S11. The desktop's waiting text is lost when the daemon dies.** If the daemon dies (no `kelpie_exit`), the desktop's mirrored chips and its `mine` texts disappear when the pane is replaced, with no "Not sent" note. Before 2b this was much the same, so it is not a regression. When `connected` goes false with `mine` non-empty, consider adding a "Not sent" item.

## Non-issues (verified)

- **Command refusal bypass** (leading Unicode whitespace or BOM, `／` U+FF0F, zero-width characters, case, multi-line):
  - `needs_desktop` can be bypassed by a hand-made client. Rust `trim` does not strip U+FEFF, where JS does.
  - The result is harmless. The daemon writes only `{"type":"text","text":…}` inside a fixed `turn/start`/`session/prompt`/`user` shape (`turns.rs:108-121`).
  - Codex app-server does not run `/` or `!` from `turn/start` input; those are TUI and adapter features, and the adapter's `!` runs only when `typed`. So the text reaches the model as a prompt: the same trust as any phone prompt, which the risk table accepts.
  - The refusal is user experience, not a security boundary. No change needed. Optionally, have Rust also trim `\u{FEFF}` so both sides agree.
- **Protocol or JSON framing injection:** not possible. Text is serialised with `serde_json`, which escapes control characters (newline included), and `cid` and `name` go only into JSON values. The desktop shows `from {device}` as a Solid text node.
- **Sizes:** checked in bytes (`text.len()` in Rust, `TextEncoder` on the phone). The phone limit is checked before rate limiting and again in `push`.
- **Memory under abuse:**
  - at most 20 entries per chat, with 16 KiB phone and 1 MiB desktop text;
  - `seen` at most 2000;
  - the rate-limit buckets are keyed by paired devices;
  - `sentFrom` at most 50;
  - phone delivery uses a bounded `ClientTx`, and an overflow resyncs, after which the `observe` reply carries a fresh `chat_queue`.
- **Stale Idle clearing a new flight:** every dialect has exactly one `Idle`-classified line per turn (`result`, `turn/completed`, `stopReason`). The next turn is written under the same lock right after it, so no late Idle line from the old turn can clear the new flight.
- **Older apps:**
  - An API-17 app against the new daemon ignores `chat.queue`, never gets `turn_ready`, and its raw turn lines are observed.
  - The new app against a 17 daemon keeps the legacy path.
  - The phone bundle ships with the daemon. `restorePicks` safely ignores daemon-built `kelpie-turn-d…` ids, which have no `~`.
- **Who may call the new methods:** `chat.send`, `chat.write_turn` and `chat.queue_op` are user-role only (`auth.rs` test). The phone's `queueOp` maps only `cancel` and `resume`.

## Open questions from the developer

1. **A rich desktop entry whose pane is detached is dropped as `needs_window`.** I agree with the controller, with one boundary.
   - **Hold it** at the head as a visible "waiting for the desktop window" state while the **queuing client's daemon connection is still alive**: the pane is closed, remounting or resyncing. Make it cancellable from both sides, and offer it again when that client re-attaches (call `pump` on `attach`).
   - **Do not hold it across a client disconnect:** app quit or crash (`daemon.rs:807` detaches every handle). Holding then would block phone messages behind it indefinitely, which defeats "the phone works with the window closed". On disconnect, keep today's fallback: plain text is built by the daemon, and a rich entry is dropped. Also write a durable log line (e.g. `kelpie_unsent {qid, cid, reason}`) so the next attach shows "Not sent". That closes the "note can be lost" gap.
   - W2 is the same problem in a smaller form and should be fixed with it.
2. **Claude TUI-only slash commands are refused only by the phone UI.** **Accept.**
   - The daemon cannot know the list.
   - What it writes is a text-only `type:user` line, and the Claude CLI answers or handles the command itself in stream-json.
   - No protocol or permission escalation is possible.
   - Note that the phone's check depends on `state.commands` having been replayed; before then, the text simply passes through, which is harmless.

## Test quality

- **Strong, behavioural:**
  - in `chat.rs`, the tests checking stdin contents and ordering, the error-reply and exit endings of a turn, the watchdog, ACP stop closing the cards, and the `kelpie_sent` order;
  - in `mobile.rs`, the dedupe, limits, L3 never-shed, revoke and `set_write` tests;
  - `turn-lines.test.ts`, which runs the real adapters against the fixture;
  - `chat-queue.test.ts`, which covers capture, decline and the ACP single render.
- Not tautological.
- **Gaps that match the Warnings above:**
  - a decline for a transient reason (W2);
  - a `write_turn` rejection rolling back (W3);
  - a cancel by another device, as seen by the phone and the desktop (W4);
  - Retry, then refused, puts the text back (W5);
  - a mirrored queue never flushed by the legacy path (W1);
  - a `chat_send` racing a revoke (W6).

## Verdict

**Ready to commit after fixes.** Fix W1–W5 before committing; each is small and local. Fix W6 in the same pass, or track it explicitly. W7 and the suggestions can follow, but W7 should at least be documented in the user guide text the developer drafted. Commit split as the developer proposed: daemon, then desktop, then phone, staging only the `store.ts` hunk `@@ -424,0 +431,7 @@`.

## Unresolved questions

- Does any remount path, such as a project-tab switch or a pane split or move, call `ChatController.detach()` for longer than one 250 ms tick? If so, Q1 and W2 are hit in everyday use, not only on a resync.
- Do the ACP agents in use (e.g. Gemini, OMP) ever send `agent_message_chunk` after the `session/prompt` result? If so, W7 is real for them.

---

# Re-review 1 (fix round 1)

## What I checked and how

- **Inputs:** the "Fix round 1" section of the developer report and the current working tree.
- **Code read:**
  - `chat.rs`: `idle`, `pump`, `enqueue_if`, `cancel_by`, `write_turn`, `decline_turn`, `client_gone`, `attach`, `tick`, `stop`;
  - `queue.rs`: the new constants and `forget_old`;
  - `mobile.rs`: the `ChatSend` arm, `allow_send` and `refund_send`;
  - `daemon.rs`: revoke, `set_write`, disable and `unregister_client`;
  - `server.rs:187`;
  - `chat-model.ts`: `queued`, `turnReady`, `rollback`, `applyQueue`, `kelpie_unsent`;
  - the Codex and ACP `snapshot()`;
  - the phone's `chat-send.ts` and `composer.tsx`.
- **Not run by me:** my Bash tool returned no output this session, the same failure the developer hit. I could run nothing myself and relied on the controller's green results: typecheck, vitest 634/634, i18n, cargo 23 + 432, clippy. The e2e retest is still pending.

## Score: 7 / 10

Most fixes are correct. W1, W4 (phone side), W5 and W6 are properly closed, and so are S1, S2, S3, S7 and S8. But one fix introduced a new defect in the hold path, another fabricates an agent status, and the hold makes a cross-device cancel the normal way out while that cancel still leaves no lasting trace on the desktop.

## Status of W1–W7

| Item | Verdict | Evidence |
|---|---|---|
| W1 | **Closed** | `chat-model.ts:322`: `queued()` is `mirrored \|\| daemonQueueOn()`. `mode-switch.ts:100` no longer reads `connected()`. Left over: at startup, with `daemonApi` still 0 and no mirror yet, a send takes the local path and the first `applyQueue` overwrites it. The window is milliseconds; see Suggestion R-S3. |
| W2 | **Closed for "not now"**, but see **R1** | `decline_turn(retry)` sets `retry_at = now + 1 s` and never `declined` (`chat.rs:980-999`). `attach` pumps. Plain text stops waiting after `OFFER_TIMEOUT` counted from the first offer. |
| W3 | **Closed** | Codex `snapshot()` restores `threadId`, `turnId`, `threadReady`, `shown`, `conversationId` and the pre-thread queue length. ACP restores `sessionId`, `lastKind` and the queue length. That covers everything `/new` changes (`codex-adapter.ts:183-195` and `225-229`, `acp-adapter.ts:74-78`). The controller removes the items the build added and restores `busy`, except on a `busy` refusal. The daemon now accepts lines when only `paused` changed (`chat.rs:967-971`). |
| W4 | **Phone closed; desktop only half closed** (R3) | `cancel_by` lists the entry in `dropped` with `cancelled_by`. The phone keeps the text as `cancelled`. The desktop note appears only if the pane is attached at that moment. |
| W5 | **Closed** | `SendResult` refusals carry `text`, and `composer.tsx:253-259` calls `putBack(r.text)`. |
| W6 | **Closed** | `enqueue_if` re-reads `permission()` under the chat lock (`mobile.rs:1051-1058`, `chat.rs:853`). Revoke and `set_write` change the permission first and drop the queue after (`daemon.rs:1617-1629`). Lock order is chat `inner` then `mobile.config`; I found no reverse nesting (`mobile.rs:262` and `284` take `config` alone). |
| W7 | **The 60 s rule is sound; the stop rule is not** (R2) | See below. |

## New findings

**R1 (Warning, regression). A head the window has *declined* is held for ever.** At `chat.rs:721` and `770-774`, together with `attach` at `chat.rs:1178-1181`.
- **What happens:**
  - `turnReady` answers a hard decline (`retry: false`) when it does not have the `cid`. The pump then skips the offer branch (`!declined` is false); the entry is neither plain nor orphaned, so it lands in the `else` hold: `waiting_window = true`, `break`.
  - The window has said it can never build this message, yet the entry stays at the head as long as the app's connection lives.
  - `attach` clears `declined`, so every re-attach offers it again and gets the same decline.
- **When it bites:**
  - A webview reload keeps the same daemon client id (`DaemonClient` lives in Tauri's Rust side) but loses `mine`.
  - An adapter error hits the same path.
  - The window's own picture message, and every phone message behind it, then wait until someone presses ✕. In round 0 this case was dropped and the phone messages went out.
- **Fix:** treat `declined` like `orphaned` for a message that is not plain text: drop it with `kelpie_unsent` and do not clear `declined` in `attach`.
- **Also:**
  - In `turnReady`, an adapter that throws should decline with `retry: false`. Today (`chat-model.ts:677-681`) it declines with `retry: true`, so a deterministic throw is retried every second for ever, each time with a `console.warn` and a `chat.queue` event.
  - `decline_turn` announces unconditionally; use `announce_if`.

**R2 (Warning, regression). The stop grace makes up an Idle status for the whole session.** At `chat.rs:1384-1393`.
- **What happens:**
  - After any stop, the phone's or an observed desktop interrupt including Send now's (`observe_written` sets `stopped_at`), 5 s with no classified line clears the flight and calls `g.activity.on_hook(Status::Idle, …)`.
  - That is not limited to the queue. It changes the session's reported status, and with it the tick's Working→Idle edge that drives `unread`, `working_since`, the app's status and the daemon's "a session leaves `working`" events and notifications.
  - Then, when the agent really ends with its `result` or `turn/completed`, or prints another Working line, `on_hook` counts a new turn (`turn_seq += 1`, `turn_started`).
- **Failure scenarios:**
  - Stop Claude while a slow subagent or a tool that ignores SIGINT is running. The agent's control response is not classified, so 5 s later the session shows Idle and may notify "finished", although the agent has not stopped.
  - If the queue was empty, so not paused, a message sent in that window goes straight into a turn that is still winding down. For Codex that is a `turn/start` during an active turn, which the agent answers with an error; the message is marked sent and lost. For ACP it is a `session/prompt` while a cancel is still pending.
- **Fix:** keep this rule inside the queue.
  - Do not touch `Activity`. Instead set a queue-only `released_after_stop` that `idle()` reads, cleared by the next classified line.
  - Raise `STOP_GRACE` to about 30 s.
  - Require no agent output at all since the stop (`last_output <= stopped`), not only no *classified* line.
- **The 60 s rule itself is sound** (`idle()` at `chat.rs:590-596`):
  - It applies only when `flight.is_none()`.
  - "In flight" is set by every turn the daemon writes (`commit_turn`) and by every observed raw turn line (`observe_written`): Claude `type:user`, Codex `turn/start`, `thread/compact/start` and `review/start`, ACP `session/prompt`.
  - It is cleared only by an Idle line, an error answer to the turn's request id, exit, the 30 s no-output watchdog (which needs `!output`), or the R2 rule.
  - So a long silent tool run inside a tracked turn is never released, and `Waiting` never counts as idle.
  - One Claude-only gap remains: a desktop steer (`type:user`) written while a flight exists is ignored. If Claude runs it as a separate turn after the first `result`, that turn has no flight, and after 60 s of silence a queued message is written into it. Claude's CLI queues stdin input, so the effect is a late steer rather than a protocol error. Document it, or count observed turn lines during a flight and keep the flight until as many Idle lines have come.

**R3 (Warning). The hold makes ✕ the normal way out, but the desktop gets no lasting note when it was cancelled from the phone.** At `chat.rs:886-898` and `chat-model.ts:727-731`.
- **What happens:**
  - Switching project or tab unmounts `ChatBody` (developer's finding), and `chat.queue` goes only to attached clients.
  - A held picture message therefore blocks the chat's phone messages while the user is elsewhere on the desktop. The only way out is the phone's ✕ on the "needs the desktop window" chip.
  - A cancel from the phone emits `dropped {reason: cancelled}` only as a live event. The desktop, which is not attached at that moment, never sees it: `mine` is deleted on the next `applyQueue`, and the picture message disappears without a note.
- **Fix:** for a cancel by someone other than the sender, also append `kelpie_unsent {reason: "cancelled", cancelled_by}` to the log, as the `needs_window` drop already does. The desktop's existing `unsent-{qid}` id already merges the live note and the log note into one.

**R4 (product decision for the user). A held head can block phone messages for ever.** The pump at `chat.rs:770-774` has no time limit while the app's connection lives.
- **What already works:**
  - The phone shows the head chip with "needs the desktop window" and ✕ (`composer.tsx:283`).
  - The hold ends correctly when the app closes or crashes: `server.rs:187` runs `unregister_client` on any end of the connection, including a crash, and that calls `client_gone`.
  - A hung app is offered the message again every 15 s.
- **What is missing:** the phone user's own chip still says only "Queued (1 ahead)", with nothing that says it is blocked behind the desktop's message. The desktop user gets no sign anywhere outside that chat's pane.
- **Options:**
  - **(a) Keep strict FIFO,** and add a phone hint ("waiting behind a message only the computer can send; ✕ to skip it") plus an app-level notice. The daemon can tell the client through the `clients` registry even when the chat is not attached.
  - **(b) Time-bounded hold:** after N minutes (e.g. 2), entries behind a held head may go out while the head stays visible. This is a documented ordering exception.
  - **(c) Drop after N minutes,** with `kelpie_unsent`.
- **Recommendation:** (a), plus R3, as the minimum. Choose (b) if the phone has to work unattended. This is a trade-off between ordering and the phone always working, which the Validation log does not cover, so the user should decide.

## Answers to the controller's questions

1. **needs_window hold:**
   - Yes, it can block phone messages for ever while the app is connected (R4), and for ever even when the window can never build the message (R1).
   - The only way out on the phone is ✕ on the head chip, which is visible. It is not explained on the phone's own blocked chip, and it is silent on the desktop (R3).
   - A live connection is detected correctly, crashes included: `server.rs:187` runs `unregister_client` on every end of the connection, and that calls `client_gone`.
2. **"Not now" retry every second:**
   - It is bounded: one head per chat, at most one offer, one decline call and one `chat.queue` per second, and nothing is written to the agent. Plain text gives up waiting after 15 s and is written by the daemon.
   - It is unbounded in time while a pane stays attached but never becomes ready (e.g. an ACP chat waiting for sign-in), and for an adapter that throws (R1).
   - Log spam: a `console.warn` once a second only on the throw path. The daemon logs nothing.
   - No write storm.
3. **W7:**
   - The 60 s rule never releases a turn that is in flight; how flight is tracked per dialect is described under R2. The Claude steer gap remains.
   - The 5 s stop rule (R2) is the part that can release a turn that is still winding down. It also makes up an Idle status for the whole session.
4. **W3:** complete for Codex `/new` and ACP `/new`. Claude has no adapter state to restore beyond the items and `busy` the controller handles. Codex `/compact` and `/review` change only `busy`. `seq` is not restored, which is harmless.
5. **W6:** correct: the re-check runs under the chat lock, revoke and `set_write` order is fixed, and there is no lock inversion.
6. **O1:**
   - Unchanged for what gets in: every accepted message spends one token, the refund is capped at the burst, and a `dup` is refunded.
   - Side effect: refused sends (`queue_full`, `command_needs_desktop`, `read_only`, `exited`, `switching`, `empty`) are no longer rate limited. Each costs one chat lock and no stdin write, so I accept it.
7. **Skipped suggestions:** all acceptable.
   - S4 is bounded to 5 s.
   - Narrowing projects: phone verbs have no project scope (`mobile.rs:878-881`), so there is nothing to enforce.
   - S9 affects speed only.
   - The rest of S10: batches are now 16.
   - S11 is not a regression.

## Suggestions (new)

- **R-S1:** in `decline_turn`, use `announce_if` (see R1).
- **R-S2:** on the phone, show "waiting behind a message only the computer can send" on the device's own chips while the head has `needsWindow` (part of R4 option a).
- **R-S3:** the startup gap in W1. Until `daemonApi` is known, refuse or hold a send instead of putting it in the local queue, or decide the path on the first `chat.queue` or `daemon.info`, whichever comes first.

## Verdict

**Not ready to commit yet.** Before committing:
1. Fix R1 (drop when the window has declined; decline with `retry: false` when the adapter throws).
2. Fix R2 (keep the stop rule inside the queue, never call `on_hook`, use a longer grace that requires no output at all).
3. Fix R3 (log a cross-device cancel).
4. Get the user's decision on R4 and implement the chosen option, at least the phone hint.
5. Get a green e2e retest (`mobile.py` x2, `mobile_protocol`, `mobile_resilience`, `acp_chat`, `composer`).

R1–R3 are small and local. After them, I expect this to be ready, pending e2e.

---

# Re-review 2 (fix round 2)

## What I checked and how

- **Inputs:**
  - the "Fix round 2" section of the developer report;
  - the new Validation log line in the phase plan (strict order, a one-tap Skip on the phone, a desktop notice outside the pane);
  - the current working tree.
- **Code read:**
  - `chat.rs`: `idle`, `note_agent_line`, `observe_written`, `pump`, `unsent`, `cancel_by`, `write_turn`, `decline_turn`, `client_gone`, `attach`, `info_locked`, `tick`;
  - `queue.rs`: `stopped_at` and `released`;
  - `session.rs:117-120`;
  - `chat-model.ts`: `turnReady` and the `kelpie_unsent` handling;
  - `chat-wait-notice.tsx`;
  - the phone's `chat-send.ts`, `phone-state.ts:234-264`, `storage.ts` and the `composer.tsx` Skip.
- **Not run by me:** my Bash tool still returns no output, so this round is read-only too. Test results are the controller's and the developer's: typecheck, vitest 638/638, i18n, cargo 432 with one HEAD timing flake that passes alone, clippy, `mobile.py` ×2 84/84, `mobile_protocol` 130/130, `mobile_resilience` 55/55, `acp_chat` 56/56.

## Score: 8.5 / 10

R1–R4 are closed. No new single-writer gap. Two non-blocking leftovers are listed below.

## R1–R4

| Item | Verdict | Evidence |
|---|---|---|
| R1 | **Closed** | A final decline sets `declined`. The pump drops a non-plain head with `unsent(…, "declined")` (`chat.rs:781-784`). `attach` no longer clears `declined` (`chat.rs:1211-1217`). An adapter that throws gives `retry: false` after rolling back (`chat-model.ts:684-690`). `decline_turn` uses `announce_if`. |
| R2 | **Closed** | The after-stop rule sets only the queue's `released` (`chat.rs:1421-1430`). Nothing touches `Activity`, so the hook, status and notifications are as the agent's lines made them. The grace is 30 s and needs no output at all since the stop. `released` is cleared by the next classified line (`chat.rs:636-639`). |
| R3 | **Closed for desktop messages** | `cancel_by` writes `kelpie_unsent {reason: cancelled, cancelled_by}` for a desktop message cancelled by someone else (`chat.rs:922-923`). The desktop shows it with the same `unsent-{qid}` id as the live note, so it appears once (`chat-model.ts:434-436`). |
| R4 | **Closed per the user's decision** | Strict order is kept. Phone messages behind a held head show the reason and a Skip button (`composer.tsx:321-341`). The desktop notice lists held chats whose pane is not visible, with Open chat (`chat-wait-notice.tsx`). The controller's wording change, "only the computer can send", is right: a held head can be a `/compact` command. |

## Questions from the controller

**1. `waits_for_window` in session info.**
- **Wire compatibility:**
  - `SessionInfo` is `Serialize` only (`session.rs:89`). Nothing in the daemon reads it back, and it is not part of persisted state.
  - The field is left out when false (`skip_serializing_if = Not::not`). Old apps, the CLI and `kelpie-call.py` see no change when nothing is held, and an old TypeScript client ignores the field when it is there.
  - It is new in API 18, which has not been released, so no new API level is needed.
- **Cleared on every path:**
  - It is not separate state. It is computed at each tick from the head (`chat.rs:1391`: `items.front().waiting_window`), and `tick` runs every 250 ms and broadcasts when the info changes.
  - Cancel and Skip (`cancel_by`), a declined entry and `client_gone` all remove the head. The next tick reports false.
  - On attach the head is offered again, which sets `waiting_window = false`.
  - A daemon restart starts with an empty queue.
- **One small inaccuracy, not a defect:**
  - If the pane comes back while the agent is busy or the queue is paused, the pump stops before it reaches the head. `waiting_window` then stays true until the gate opens.
  - The desktop notice hides visible panes, so only the phone's Skip hint lingers a little longer than needed.

**2. B2 persisted text.**
- **Wiped on forget and unpair:**
  - The key is `kelpie-draft:chat-pending:<chatId>`, which `wipeTypedText()` removes at `phone-state.ts:128` and `418`.
  - `forgetSenders()` runs right after and calls `dispose()`, which drops `save`, so a late reply cannot write the text back.
- **No leak across devices or projects:**
  - `localStorage` belongs to the phone page's own origin, and the key is per chat id. Chat ids are random daemon session ids, so they do not collide across projects.
  - Pairing with another computer means forget/unpair first, which wipes the keys.
- **Bounds:**
  - Size is bounded per message: phone sends are at most 16 KiB (`chat-send.ts:45`).
  - The number of messages is not bounded, per chat or overall:
    - `lost`, `unknown` and `cancelled` pile up until the person discards them;
    - keys for chats that no longer exist are never cleaned up;
    - when `localStorage` is full, `store.set` silently falls back to memory (`storage.ts:13-18`), so the copy is lost on the next reload without a word.
  - Realistic volumes are small (5 queued per device, rate limited), so this is a Suggestion (R2-S2), not a blocker.

**3. Can R1 or R2 drop a real picture message too eagerly?**
- No, as far as I can see.
- A hard decline happens only when `!own`, on an observer, without `writeTurn`, when the agent has exited, or when the adapter throws (`chat-model.ts:673`). `!own` is checked first, but it cannot happen just because history is still loading:
  - `mine` lives on the controller, which is cached per session (`mode-switch.ts`) and disposed only on a mode switch or a resume, not when the pane unmounts;
  - `mine` is filled before `chat.send` is called.
- So a remount, a resync or a history load finds the message and answers "not now" (`retry: true`), which never drops.
- Only a real loss gives a final decline: a webview reload, a new controller, or an adapter error. That is now dropped with a lasting note, which is right because the images are gone.
- R2's new gate (nothing counts as idle while a stop is pending) only adds waiting, never a drop. An interrupt seen while nothing runs does not arm it (`chat.rs:702`).

**4. Single-writer gaps still open.** None new. Three known and accepted ones remain:
- **Claude steer:** a steer written during a flight that Claude runs as its own turn has no flight. After 60 s of silence a queued message can go into it, which Claude's CLI queues like a late steer.
- **30 s release:** a stopped turn that prints nothing for 30 s is released for the queue. Codex answers a `turn/start` into a still-running turn with an error, so that message is marked sent but fails.
- **API-17 desktop app:** its raw turn lines are observed, not blocked.

A related loss of text, not a second writer: at startup, until `daemonApi` is known and no queue update has arrived, a desktop send goes to the local queue and the first `applyQueue` overwrites it (R-S3 from re-review 1, still open).

## Remaining items (none blocking)

**R2-W1 (Warning, follow-up). A phone message cancelled by the desktop while the phone is offline shows as "Sent".**
- **What happens:**
  - R3 logs `kelpie_unsent` only for *desktop* messages (`chat.rs:922-925`).
  - A phone message cancelled by the desktop is reported only in the live `dropped`, which a disconnected phone misses.
  - On reconnect, `ChatSender.snapshot` turns a `queued` entry that is no longer listed into `sent`, and prunes it after 5 s. The phone user's text is gone, and the screen said it was sent. This is the W4 problem again, narrowed to an offline phone.
- **Fix, small:**
  - Log `kelpie_unsent {reason: cancelled}` for every cross-device cancel.
  - The phone's controller passes it on through `onUnsent` (as it does with `onSent`), and the sender marks the message `cancelled`.
  - Alternatively, `snapshot` could move a disappeared message to `unknown` instead of `sent` unless its `kelpie_sent` was seen.

**R2-S1.** The phone shows the `kelpie_unsent` notes for `declined` and `needs_window` as "Not sent (this window could not prepare it)" (`chat-model.ts:438`). On a phone, "this window" means the phone. Use "the computer could not prepare it" when `observer`.

**R2-S2.** Cap the kept chips per chat (for example 20, oldest first after a warning). Drop `chat-pending` keys whose chat is not in the session list when the list arrives. Tell the person when the browser cannot store them.

**R2-S3.** `chat-wait-notice.tsx` shows the notice in every app connected to the daemon. When a dev app and the installed app share one daemon, the app that did not queue the message cannot help, because only the queuing client can build it. Low priority. If wanted, add the queuing client to the field, or show the notice only in the app that queued it.

**Carried over:**
- R-S3 (startup gap).
- The Claude steer gap. Document it in `docs/user-guide.md` with the suggested text when that file is free.

## Verdict

**Ready to commit.** Use the developer's three-commit split and stage only the listed hunks:
- `app.tsx`: `@@ -25,6 +26,7 @@` and `@@ -157,6 +159,7 @@`;
- `store.ts`: `@@ -424,0 +431,7 @@`.

Do R2-W1 next, together with R2-S1 and R2-S2, in a small follow-up before phase 3. It needs no further review loop beyond its own tests.

---

# Re-review 3 (fix round 3), final

## What I checked and how

- **Inputs:** the "Fix round 3" section of the developer report and the code it names.
- **Code read:**
  - `chat.rs` `cancel_by`;
  - `chat-model.ts` (`onUnsent`, the observer wording);
  - `chat-send.ts` `unsentSeen`;
  - `kept-messages.ts`;
  - `phone-state.ts:93`, `108` and `229-233`;
  - `mobile.rs` `publish` and `sessions` (`675-686`, `810-819`);
  - `mode-switch.ts` `resumeChat` (`253-271`);
  - `store.ts` `resumeInterrupted` (`1480-1492`);
  - `mobile.py:262-274`.
- **Not run by me:** my Bash tool still returns no output. Test results are the developer's.

## Score: 9 / 10

## The five items

1. **R2-W1: closed.**
   - **Exact matching:**
     - `unsentSeen` matches on the message's `cid`, a per-message `randomUUID` from the phone, and acts only on a message in this phone's own sender list. Lines for other `cid`s, desktop messages included, do nothing.
     - A message that is already `cancelled` or `lost` is left alone, so a replayed line changes nothing (vitest covers both cases).
   - **A cancel never applies to a written message:**
     - The daemon's `cancel` takes only entries still queued and not offered (`queue.rs` `cancel`), and phone entries are never offered. So an entry logged as `cancelled` was never written.
     - On replay, a written message's `kelpie_sent` removes it from the list (`sentSeen`), so a later line cannot find it.
     - The only theoretical false match is another device reusing this phone's `cid`. `cid`s are visible in `chat_queue`, but all paired devices belong to one user. Suggestion R3-S1 removes even that.
2. **R2-S1: fine.** The phone (observer) says "the computer could not prepare it"; the desktop keeps "this window".
3. **R2-S2: the bound is correct.**
   - **Caps:** 20 per chat and 60 in all, newest first. The total also counts every other chat's key.
   - **Failed writes:** a failed write deletes the chat's key and flags every message as `unsaved`, and the chip shows Copy with the alert. Nothing stale is left that could bring back a message the person let go.
   - **No pruning during a transient empty list:** `forgetGoneChats([])` returns at once. The list it gets is always the daemon's full, unfiltered session list (`mobile.rs` `sessions()` / `publish`), never a partial or project-scoped one. So a reconnect, or the time before the first state arrives, cannot delete a live chat's copies.
   - **One real gap, see R3-W1:** a chat that is *resumed* is replaced by a new session id.
4. **F4: stricter, confirmed.**
   - The old check slept 1.5 s and looked for the echo.
   - The new one first waits, up to 15 s, for "Control-C" (Windows) or "^C" after the last `ping` on screen. That proves the phone's key reached the PTY and stopped the command, which the old check never proved. Only then does it type the next command and check that it shows. That second check is as strong as before.
   - What it gives up: a fixed 1.5 s latency budget for the phone key. That budget was never the point of the check.
   - On non-Windows hosts the command is `sleep 30` and there is no `ping`, so `split("ping")[-1]` is the whole screen and "^C" still matches.
5. **`mobile_protocol.py`: right.** "FIFO across origins" means the order the daemon *received* the messages in. Waiting until the daemon lists the desktop message before the phone sends sets up that order on purpose, instead of leaving it to two threads. The FIFO assertion itself is unchanged.

## Findings

**R3-W1 (Warning, not blocking). Resuming a stopped chat deletes the phone's lost messages from it.**
- **What happens:**
  - The main use of B2 is a phone message lost in a daemon restart. The phone shows it as `lost` in the stopped chat, and its O2 hint says "Open it on the desktop to resume it".
  - Resuming creates a **new** session and closes the old one (`mode-switch.ts:257-267`). With the default `resumeSessions: "shown"`, a chat in a visible pane is resumed as soon as the app reconnects (`store.ts:1480-1492`, `wakeVisible`).
  - The old id then leaves the list, and the next `sessions` message runs `forgetGoneChats`, which deletes that chat's kept copies.
  - Even before this round the chips were unreachable once the chat left the list, because no screen shows a chat that is gone. Pruning makes the loss permanent, and silent.
- **How often:** rare. It needs a phone message waiting or in flight at the moment of a daemon restart.
- **Fix, small:**
  - Prune a gone chat's copies only once they are old (for example older than 7 days, by `at`). The 60-message total cap already bounds storage.
  - List the kept messages of chats that are gone under a "Not sent" section on the phone's session list, with Copy and Discard.

**R3-S1.** In `unsentSeen`, when the phone knows the message's `qid` (from the `ok` or `dup` reply), require the line's `qid` to match as well as the `cid`. A message in doubt (no `qid` yet) falls back to the `cid` alone.

**Carried over, accepted:**
- R-S3 (startup gap);
- the Claude steer gap (document it in the user guide);
- R2-S3 (the desktop notice in two apps that share one daemon);
- the two load-sensitive test flakes the developer listed: harden them in a separate test-only change, as proposed.

## Verdict

**Phase 2b is ready to commit**, with the developer's three-commit split and the hunk-only staging listed in round 2. Do R3-W1 as the first follow-up, before phone access is used in production.
