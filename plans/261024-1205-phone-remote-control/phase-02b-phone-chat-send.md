---
phase: 2b
title: "Send chat messages from the phone (single writer)"
status: completed
priority: P1
effort: "6d"
dependencies: [2]
---

# Phase 2b: Send chat messages from the phone (single writer)

## Overview
Phase 2 lets the phone watch a chat and answer the agent's own questions (`chat_answer`). It cannot send a message: no verb exists,
and `input` on a chat is refused with code `chat` (`mobile.rs:846-851`). Today the desktop's `ChatController` owns the turn queue
and writes turns itself (`chat-model.ts:410-435`), so a phone that writes turns would be a second writer.

This phase moves the turn queue into the daemon: **one bounded queue per chat**, written when the agent is idle, fed by the desktop
and the phone alike. The phone stays an observer adapter, phone sends work while the window is closed, and desktop chat behaves as
before. It closes review W5 (`plans/reports/code-reviewer-261029-2130-phone-phase2.md:57-67`) with variant (a), refined.

Start after phase 2 is merged, because 2b edits the same files (`chat-model.ts`, `app/phone/src/*`). Branch `phone/phase2b-chat-send`,
stacked like phase 2. 2b blocks phase 3's chat-write features, so kelpie-link (3) and the native shell (5) carry the new verbs from the start.

## Requirements
- **Phone verbs** (all *write*, closed table `mobile/verbs.rs:14-57`):
  - `chat_send {id, cid, text}` → `ok {qid, state: queued|sent, ahead}` | `dup {qid, state}` | refusal. Refusal codes: `read_only`, `epoch`, `not_chat`, `exited`, `switching`, `empty`, `too_long`, `rate_limited`, `queue_full`, `command_needs_desktop`.
  - `chat_cancel {id, qid}`, `chat_stop {id}`, `chat_resume {id}`.
  - Phone message `chat_queue {id, items, paused, in_flight, dropped?}`.
- **Desktop RPCs** (user role only, like `session.input`, `daemon.rs:1684-1688`; listed in `auth.rs`):
  - `chat.send {handle, cid, text, rich}` → `{qid, state}`.
  - `chat.write_turn {handle, qid, lines|null, turn}`.
  - `chat.queue_op {handle, op: cancel|front|take|resume|hold, qid?}`.
  - Events: `chat.queue {handle, id, items, paused, held, in_flight, dropped?}` to every attached client, and `chat.turn_ready {handle, qid, cid}` to the queuing client only. Attach and observe replies carry the current queue (`chat.rs:478-519`).
- **Queue** (daemon, per chat):
  - FIFO in daemon arrival order across origins. At most 20 entries per chat and 5 per device.
  - Entry: `{qid, cid, by: By, text, kind: Phone | Desktop{client, rich}, at}`. The desktop keeps a rich entry's images in its own memory, keyed by `cid`.
- **Gate** (pump): write the head entry only when the agent is alive, the queue is not `held` or `paused`, no turn is in flight, and status is `Idle` (never `Working` or `Waiting`, from `classify`, `chat.rs:726-772`). Codex and ACP turns that the daemon builds also need the conversation id.
- **In flight**:
  - Set by every turn the daemon writes. Also set when a turn-shaped line arrives through `session.input`: from an older app, from an adapter's pre-thread queue (`codex-adapter.ts:370-373`, `acp-adapter.ts:297-301`), or from a desktop steer. Turn-shaped means Claude `type:user`, Codex `turn/start|thread/compact/start|review/start`, or ACP `session/prompt`.
  - These lines are **observed, never blocked**.
  - Cleared by: an `Idle` line after the write, an error response to a request id the turn wrote, or the process exiting.
  - Watchdog: in flight for more than 30 s while `Idle` with no output ⇒ cleared and logged.
- **Busy or waiting agent**: the message is queued, and chips show on both the desktop and the phone. **Ordering**: one queue, so desktop and phone messages go out in the order the daemon received them.
- **Stop**:
  - Phone `chat_stop` makes the daemon build the dialect's interrupt:
    - Claude: `control_request interrupt` with a `kelpie-interrupt-` id, which the adapter already ignores on error (`claude-adapter.ts:303-304`).
    - Codex: `turn/interrupt {threadId, turnId}`.
    - ACP: `session/cancel`, plus a cancelled answer to every open permission or elicitation request, sent through `resolve` so every card closes (mirrors `acp-adapter.ts:369-390`).
  - The desktop keeps its adapter interrupt. The daemon recognises an interrupt line from either side.
  - Either way, a non-empty queue becomes `paused`: "stopping means wait" (`chat-model.ts:476-480`). A new send or `resume` unpauses it.
  - Stop with nothing running is a no-op ack. A second stop within 1 s is coalesced.
- **Cancel and edit**:
  - Any client with write permission may cancel any queued entry (one user; the origin is shown). An entry already offered or written returns `too_late`.
  - Edit = cancel, then put the text back in the composer. Only for your own entries: the phone keeps its text by `cid`. There is no in-place edit.
- **Desktop "Send now"**: same branches as `sendNow` (`chat-model.ts:455-470`):
  - agent idle ⇒ `front`;
  - agent busy and the adapter can steer ⇒ `take`, then the adapter's steer line goes through `session.input`, observed;
  - otherwise ⇒ `front` + interrupt.
  - The phone has no send-now in this phase.
- **Desktop view**:
  - `state.queue` becomes a mirror of `chat.queue`, and `queuePaused` follows it.
  - Chips show the origin ("from {device}") (`chat-composer.tsx:214-226`).
  - Dropped entries become today's "Not sent: …" item (`chat-model.ts:328-332`).
- **Exit, `chat.close_stdin` and mode switch**: the queue is dropped, with `dropped` listed. `switchToTerminal` sends `hold` **before** `ctrl.interrupt()` and `chat.close_stdin` (`mode-switch.ts:218-222`). While held, new sends are refused with `switching`, so no turn slips in during the hand-off.
- **Visibility**:
  - Before writing a turn, the daemon appends `{"type":"kelpie_sent","qid","cid","by","name","text","images":[names]}` to the log. The text is capped at 64 KiB.
  - ACP agents do not echo a live prompt (`user_message_chunk` only on load, `acp-adapter.ts:178`), so the user item is rendered from `kelpie_sent`, live and on replay.
  - Claude (`--replay-user-messages`, `claude-adapter.ts:29`) and Codex (`userMessage` items) render their own echo. `kelpie_sent` only adds the "from {device}" label.
- **Replay safety**:
  - Only the daemon writes queued turns.
  - `chat.turn_ready` is a live event for one client. It is declined (`lines: null`) while replaying, on a stale attach generation, or for an unknown `cid`.
  - `kelpie_sent` is display-only.
  - Phone adapters keep the observer gate (`chat-model.ts:260-269`) and never get a turn write path.
- **Window closed**:
  - Phone sends still go out.
  - The daemon answers an ACP agent's non-question client request with `-32601` when no desktop client is attached. Today only the desktop adapter does that (`acp-adapter.ts:355-358`), so without it a turn could hang.
  - Claude and Codex ask the client only for approvals, which the phone can answer.
- **Phone input rules**:
  - Text only: no attachments, because file transfer is a plan non-goal (`plan.md:45`).
  - Claude: slash commands are sent as text, because the CLI runs them. TUI-only ones are refused locally, using `terminal_slash_commands` (`claude-adapter.ts:221,353-356`).
  - ACP: agent commands are sent as text. `/new`, `/clear` and `/reset` are refused, because the client handles them (`acp-adapter.ts:73-79`).
  - Codex: any text starting with `/` or `!` is refused, because the client runs those (`codex-adapter.ts:172-211`). The daemon checks this too and answers `command_needs_desktop`.
- **Limits**: text ≤ 16 KiB (`MAX_TYPED`, `mobile.rs:59`) and non-empty after trimming. Per device: ≤ 1 send/s sustained, burst 3. A desktop entry's display text ≤ 1 MiB.
- **Dedupe**:
  - `cid` (random, ≤ 64 chars) per (device, chat), kept 10 min, like answers. A retry gets `dup` with the entry's real state (`queued|sent|cancelled|dropped`) and never makes a second entry.
  - Phase 3's per-device dedupe table adopts `chat_send` at 10 min (phase 3 spec line 35 lists answers and input only).
- **Revoke or downgrade**: the device's queued entries are dropped (`revoked`) before its socket closes. A turn already written runs on.
- **Never shed**: `chat_send`, `chat_cancel`, `chat_stop`, `chat_resume` and their replies, which go on the writer's own path (`lanes.rs:4-6`). Add them to D13's list. `chat_queue` is state: it goes in the chat's FIFO on the bulk lane and is coalesced to the latest.
- **API**: take the next free `API_LEVEL` at commit time (`kelpie-proto/src/lib.rs:29`). 17 is phone phase 1 and 16 is master, so expect 18; recheck open branches first.
  - App constant `CHAT_QUEUE_API`. Against an older daemon, the controller keeps today's local queue and `session.input` path; that code stays.
  - An older app against a new daemon still works: its raw turn lines are observed.
  - The phone bundle ships inside the daemon, so the two always match. The native app (phase 5) gates its composer on the `hello` API level.
- **Phone UX**:
  - Chat composer: native `<textarea>`, `autocapitalize="sentences"`, spellcheck on, grows to 6 lines.
  - **Enter inserts a newline and Send sends**; Ctrl/Cmd+Enter sends from a hardware keyboard. This sidesteps the Telex/VNI Enter-commit problem (review S4).
  - The IME guard comes from `composer-core`.
  - Double-send guard: `if (sending()) return`. The box is cleared only when its value is unchanged (review W2).
  - Message states: `sending…` → chip `queued (n ahead)` → `sent` (own `cid` seen in a `kelpie_sent` line) → transcript item.
  - `unknown`: "Not sure it was sent", with Retry (same `cid`) and Discard.
  - Refused: the text goes back into the box with the reason.
  - Queue chips for every origin, each with ✕. Edit appears on your own chips.
  - **Stop button**: shown while the agent is busy, separate from Send, and large. It shows "Stopping…" until idle.
  - Paused queue: "Queue paused: Resume".
  - Read-only devices and stopped chats show no composer. When not `live`, the composer is disabled and the draft is kept. EN + VI.

## Resilience notes
- **Drop mid-send** ⇒ `unknown`. One automatic retry with the same `cid` within 60 s; `dup` reports the truth. After 60 s, ask the user. The text is never lost.
- **Reconnect**: the `chat_sub` reply carries a `chat_queue` snapshot. Your own `cid` seen in a `kelpie_sent` line ⇒ `sent`, even if the reply was lost.
- **No hidden offline queue**: the daemon queue is visible and cancellable on every device, and the phone never holds sends while offline.
- **Epoch change** ⇒ pending sends are discarded, never retried. The queue is in memory and dies with the daemon run, by design.
- **L2/L3**: the verbs are never shed and the `chat_queue` mirror is coalesced. The pump runs with the chat session and not on a phone task. `chat_send` ack p95 ≤ 300 ms under the phase-1 stress gate.
- **Slow or stalled window**: a `turn_ready` offer times out after 15 s, then falls back (see Architecture), so a stuck desktop cannot block phone messages.

## Architecture
```
phone composer ─ chat_send{id,cid,text} ─▶ mobile.rs verb (perm, epoch, rate, size, dedupe) ─┐
desktop ChatController.send ─ chat.send{cid,text,rich} ─▶ daemon.rs ─────────────────────────┤
                                                                                             ▼
                ChatSession.turns (chat/queue.rs): FIFO ≤ 20, paused, held, in_flight, cid map
                   pump: alive ∧ ¬held ∧ ¬paused ∧ ¬in_flight ∧ Idle (classify)
          ┌──────────────────────────┴─────────────────────────────┐
  phone entry / plain fallback                              desktop entry
  chat/turns.rs builds the line                   chat.turn_ready → adapter.send under capture
  (conversation ids tracked from stdout)          → chat.write_turn{qid, lines, turn}
          └──────────────────────────┬─────────────────────────────┘
        log ← kelpie_sent{qid,cid,by,text} · stdin ← line(s) · in_flight ← qid (if turn)
        chat.queue → desktop chips / phone chat_queue · transcript ← agent echo or kelpie_sent
```
**Who builds the line: split by who holds the state** (D14).
- **Phone turns: the daemon builds them (option A, text only).** `chat/turns.rs` holds three small user-turn builders, the interrupt builders and turn-line detection. It tracks the Codex thread id, the ACP session id and the Codex turn id from lines the daemon already reads, including the pinned `kelpie-thread` and `kelpie-session` answers (`chat.rs:41-49`). Why this way:
  - The phone stays a pure observer (D6, RT-4).
  - The daemon serialises text it built itself, so a phone cannot slip in `turn/start` sandbox or approval overrides, or Claude `set_permission_mode`. This is the same principle as `exact_answer` (`chat.rs:248-274`).
  - Lines are built at write time, so a thread id is never stale.
- **Desktop turns: the desktop adapter builds them at dequeue (option B).**
  - The daemon offers the head entry to the client that queued it.
  - `ChatController` runs `adapter.send` exactly as `flushQueue` does today (`chat-model.ts:424-435`), with `write()` captured.
  - It returns the lines with `turn = state.busy became true`.
  - This keeps images (Claude base64, Codex `localImage` paths, ACP `canImage`), Codex `$skill` items, picks encoded in the turn id (`codex-adapter.ts:116-123`) and client-side commands in TypeScript, with today's timing.
- **Fallback**: a desktop entry whose builder is gone (window closed or reloaded, or no `write_turn` within 15 s) is handled by type:
  - plain text (no images, no leading `/` or `!`) ⇒ the daemon builds it;
  - anything else ⇒ `dropped: needs_window`, shown as "Not sent".
  - Codex picks not yet sent then apply on the next desktop-built turn.
- **Rejected**:
  - Option A for every message: it ports the rich adapter logic to Rust.
  - Option B for the phone: it still needs a validator for each dialect, gives the observer a write path, and lines built at enqueue go stale after `/new`.
  - Forwarding to the desktop `ChatController`: it does nothing while the window is closed.
- **Drift guard**: one golden fixture, `crates/kelpied/src/chat/turn-fixtures.json`. A cargo test checks it against the Rust builders, and a vitest test checks it against the TS adapters' lines for the same text, ignoring ids.
- **Single writer**:
  - Turns are written only by the daemon's pump (or observed when written raw).
  - Answers keep first-wins (`chat.rs:532-594`).
  - Control lines (settings, handshakes, steer) remain the desktop's own, unchanged.
  - Everything goes through the one stdin channel (`chat.rs:351-373`).

## Related Code Files
- **Create**:
  - `crates/kelpied/src/chat/queue.rs`: queue, gate, in-flight, cid map, bounds.
  - `crates/kelpied/src/chat/turns.rs`: dialect from `spec.kind` (`claude`, `codex`, else ACP, as `mode-switch.ts:27`), conversation tracking, builders, turn and interrupt detection.
  - `crates/kelpied/src/chat/turn-fixtures.json`.
  - `app/src/chat/turn-lines.test.ts`, `app/src/chat/chat-queue.test.ts`, `app/phone/src/chat-send.ts` (+ `.test.ts`).
- **Modify**:
  - `crates/kelpied/src/chat.rs`: `Inner` gets the queue; `input` observes turn and interrupt lines; `ingest_line` updates the conversation and turn ids and the in-flight state; `kelpie_sent`; `pending` keeps the raw id and kind for ACP cancel; `-32601` when no desktop is attached; exit and `close_stdin` drop the queue.
  - `crates/kelpied/src/daemon.rs` (methods and events), `crates/kelpied/src/auth.rs` (user-only methods).
  - `crates/kelpied/src/mobile/verbs.rs` (4 verbs and table tests), `crates/kelpied/src/mobile.rs` (verb arms, rate limit, revoke drop, `chat_queue` relay).
  - `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`).
  - `app/src/chat/transport.ts` (`enqueue`, `writeTurn`, `queueOp`, `chatSend`, `stop`, events), `desktop-transport.ts`.
  - `app/src/chat/chat-model.ts`: daemon-queue path, capture, `turn_ready`, mirror, `kelpie_sent`, `from` label.
  - `app/src/chat/acp-adapter.ts`: no local user item on the daemon path. Other sessions have uncommitted edits in this file; coordinate before editing.
  - `app/src/chat/codex-adapter.ts`: a refused steer is queued again through a context hook, not through `ctx.set("queue")` (`:317-321`).
  - `app/src/chat/mode-switch.ts` (`hold`), `app/src/components/chat-composer.tsx` (origin label), `app/src/lib/{ipc,types}.ts`.
  - `app/phone/src/{chat-view.tsx,composer.tsx,phone-state.ts,link.ts}`, `app/src/locales/vi/*.json`.
  - `scripts/e2e/{mobile_protocol.py,mobile.py,mobile_resilience.py}`.
  - `docs/architecture.md` (RPC and event lists at `:105,:108`, verb table at `:284-285`, chat at `:345-368`, never-shed list at `:404`), `docs/user-guide.md`.
- Phase 2b owns these files alone. It runs after phase 2 and does not overlap with phases 3–9; phase 3 starts after 2b, because both touch `mobile.rs` and `verbs.rs`.

## Implementation Steps
Three commits. Each passes `cargo test -p kelpied` and `pnpm typecheck && pnpm test`. Use `CARGO_INCREMENTAL=0`.
1. **Daemon** (commit 1):
   - `turns.rs` and the fixture; `queue.rs`; `chat.rs` integration (observe, `kelpie_sent`, conversation ids, in flight, watchdog, drop on exit, ACP `-32601`).
   - Desktop RPCs and events, auth table, phone verbs, rate limit, cid dedupe, revoke drop, `API_LEVEL`.
   - Rust tests. An unchanged app must still pass `chat_ui.py`.
2. **Desktop** (commit 2):
   - Transport methods; the `ChatController` daemon path behind `CHAT_QUEUE_API` (`send` → `enqueue`; `turn_ready` → capture → `writeTurn`; mirror; `unqueue`, `sendNow` and `resumeQueue` → `queueOp`; `kelpie_sent` handling).
   - ACP local-item gate, Codex steer requeue hook, `hold` in mode-switch, chip origin label. The legacy path stays as it is.
3. **Phone** (commit 3): `chat-send.ts` state machine, chat composer mode, Stop/Resume, queue chips, refusals, VI strings. Then docs, and the e2e extensions.

**Rollback**: revert per commit.
- Reverting commit 3 hides the phone composer; the verbs go unused.
- Reverting commit 2 puts the desktop back on its legacy queue, which still works against the new daemon because raw turns are observed.
- Reverting commit 1 needs commits 2 and 3 reverted first.
- No persisted data changes: the queue lives in memory only.

## Tests
- **Rust** (`cargo test -p kelpied`):
  - Builders equal the fixture (three dialects plus interrupts).
  - Gate: busy, waiting, exited, in flight, paused, held, conversation unknown.
  - FIFO across origins; offer timeout and fallback (plain text ⇒ built, rich ⇒ `needs_window`).
  - A `cid` retry gives `dup` with the same `qid` and state.
  - Cancel, `too_late`, `front`, `take`, `resume`.
  - Revoke drops the device's entries; exit drops everything with `dropped`.
  - `hold` refuses with `switching`.
  - Conversation ids from pinned lines, including Codex `/new`; Codex stop without a turn id ⇒ no-op.
  - ACP stop cancels open permissions through `chat.resolved` (by the phone).
  - A raw desktop turn line sets in flight; an observed interrupt pauses the queue.
  - `kelpie_sent` comes before the stdin write and survives into the observer reset.
  - Bounds (20/5/16 KiB) and the rate limit.
  - Verb table closed and the permission matrix (the 4 verbs refused for read); `input` on a chat is still `chat`.
  - The no-unbounded-queue grep stays green.
- **Vitest**:
  - `turn-lines.test.ts`: each adapter's line for "hello" equals the fixture.
  - `chat-queue.test.ts`:
    - `send` enqueues without writing;
    - `turn_ready` captures the lines and the `turn` flag;
    - a `turn_ready` during replay or for an unknown `cid` is declined;
    - the mirror drives chips and `queuePaused`;
    - an ACP `kelpie_sent` renders once (no duplicate with a local item);
    - the Claude echo carries the `from` label;
    - below `CHAT_QUEUE_API`, existing `chat-model.test.ts` and `chat-observer.test.ts` stay green.
  - Phone `chat-send.test.ts`: states queued, sent, `unknown` → exactly one retry with the same `cid`, refused → text back; double-send guard; Codex `/` refused locally; disabled offline with the draft kept.
- **e2e, daemon only** (`mobile_protocol.py`, own `KELPIE_ENDPOINT`, fake agents):
  - Idle send is written once.
  - A send while busy is queued and written after the turn ends.
  - Desktop RPC and phone sends interleaved stay in FIFO order.
  - A `dup` retry; stop pauses the queue; revoke drops entries.
  - ACP through `fake_acp_agent.py --log`: the prompt is visible through `kelpie_sent`, and stop cancels a permission.
  - The window-closed case: no desktop client attached.
- **e2e, GUI** (`mobile.py`): the phone composer sends into a chat; the desktop shows the phone chip, then the transcript item with its origin; the phone Stop button works; a read-only device sees no composer.
- **e2e, resilience** (`mobile_resilience.py`): a link drop during `chat_send` ⇒ `unknown` → one retry → `dup`, and no duplicate turn reaches the agent.
- **Desktop regressions that must stay green**:
  - `chat.py` (claude, codex, omp; includes the hand-off) and `acp_chat.py`;
  - `chat_ui.py` (queue chip while busy);
  - `chat_parity.py` (commands, Codex local commands);
  - `composer.py` (pictures: rich entries);
  - `chat_settings_sync.py` (Codex picks on `turn/start`);
  - `agent_resume.py`, `mobile_protocol.py`.
- **Manual once**: iPhone Safari and Android Chrome with Laban/Gboard Telex and VNI in the chat composer.

## Perf gate
- Desktop chat send to agent stdin: p95 ≤ 15 ms on an idle machine (two local IPC round trips added).
- Phone `chat_send` ack p95 ≤ 300 ms and 0 lost sends in the 30 min stress run (`stress.py`, 8 phones).
- `perf_gate.py --compare scripts/perf/baseline.json`: daemon throughput and bundle size within budget. Do not update the baseline.

## Success Criteria
- [ ] From a phone, a Claude, a Codex and an ACP chat each take a typed message (Telex/VNI safe) and run it as a turn, with the Kelpie window open and with it closed.
- [ ] A message sent while the agent works waits in one queue shown on the desktop and the phone, and goes out after the turn in arrival order. Cancel and Stop work from both.
- [ ] The agent never receives two turns at once, or a duplicate after a dropped link (e2e asserts this from the fake agents' stdin logs).
- [ ] Desktop chat e2e listed above are green and unchanged; against an older daemon the desktop still uses its own queue.
- [ ] A read-only, revoked or plain-`http://` device cannot send. A phone cannot send agent settings or overrides.

## Validation log

- 2026-10-30, user decisions: D14 daemon-owned queue approved; desktop and phone may cancel each other's queued messages (one user); Codex `/compact`, `/review`, `/new` and `!cmd` from the phone are refused with a hint to use the desktop; revoking a device drops its queued messages but lets a running turn finish (Stop stays available on the desktop).
- 2026-10-30, user decision (remote access, see `plans/reports/research-261030-1530-phone-remote-access-options-synthesis.md` and plan.md Validation log): path D, app-level biometric lock with full rights once unlocked, every channel and option a user-facing toggle with plain explanations.
- 2026-10-30, user decision: a rich desktop entry held at the head as `needs_window` keeps strict order (nothing reorders or expires on its own). The phone shows a clear "a desktop message with an attachment is waiting for the desktop window" notice with a one-tap Skip (cancel), and the desktop shows a notice outside the chat pane.

## Risk Assessment
| Risk | L×I | Mitigation |
|---|---|---|
| Desktop send regression (queue, picks, pictures, commands) | M×H | Desktop keeps building its own lines at dequeue; `CHAT_QUEUE_API` gate with the legacy path retained; the chat e2e set above; commit 2 can be reverted on its own |
| Queue stuck because the daemon misses a turn end | M×H | End rules: `Idle` line, error to the turn's id, exit; 30 s idle watchdog; desktop "Send now"; the phone shows "waiting for the agent" |
| Rust and TS line formats drift | M×M | Shared golden fixture tested on both sides |
| A turn slips in during a terminal ↔ chat hand-off | L×H | `hold` before interrupt and `close_stdin`; `chat.py` hand-off e2e |
| ACP user item duplicated or missing | M×L | Rendered from `kelpie_sent` only on the daemon path; vitest |
| Window closed: an ACP client request hangs a phone turn | M×M | Daemon `-32601` when no desktop is attached; a sign-in still needs the window (documented) |
| Phone prompts let the agent run anything its mode allows | H×M | Same trust as terminal `input` (write); agent approvals still go to the phone; documented in the user guide |
| Edits collide with other sessions in `acp-adapter.ts` and locales | M×M | Stage by explicit path and ask peers first (phone-plan execution rule) |
