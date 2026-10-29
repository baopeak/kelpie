# Planner report: phone phase 2b, send chat messages from the phone (single writer)

Plan: `plans/261024-1205-phone-remote-control/phase-02b-phone-chat-send.md` (new). `plan.md` changes:
- a 2b row: P1, 6d, depends on 2;
- the phase 3 row now depends on "2; 2b for chat-write verbs";
- decision D14;
- effort ~59d → ~65d;
- one sentence in the Order note.

No code and no other plan file was touched.

## Evidence that shaped the design
- **Desktop owns the queue**: the desktop `ChatController` queues and flushes turns itself (`chat-model.ts:410-435`). The phone is an observer whose only write is an answer (`chat-model.ts:260-269`).
- **Turn lines depend on adapter state**: Codex `turn/start` needs the thread id, the user's picks encoded in the request id, and skill items (`codex-adapter.ts:107-123`). Codex client-side commands change adapter state (`:172-211`). ACP needs `sessionId`, `canImage`, and handles `/new` itself (`acp-adapter.ts:73-85`).
- **The daemon is a neutral relay today**:
  - It already classifies busy/idle (`chat.rs:726-772`).
  - It pins the `kelpie-thread` and `kelpie-session` answers (`chat.rs:41-49`).
  - It parses and re-serialises phone answers (`exact_answer`, `chat.rs:248-274`).
  - It logs only stdout (`chat.rs:410-476`).
- **ACP never echoes a live prompt** (only on `session/load`, `acp-adapter.ts:178`). A message written by the daemon would therefore be invisible unless the daemon logs it.
- **A hand-off race exists**: `switchToTerminal` interrupts and then closes stdin (`mode-switch.ts:218-222`), so a phone turn could slip in between.

## Key decisions
1. **Who builds the line depends on who holds the state.**
   - Phone turns: the daemon builds them (option A), text only. Three small builders, plus interrupts, in `chat/turns.rs`, with conversation ids taken from stdout.
   - Desktop turns: the desktop adapter builds them at dequeue (option B). The daemon offers the head of the queue with `chat.turn_ready`, the controller runs `adapter.send` as `flushQueue` does today, and returns the lines with `chat.write_turn`.
   - This keeps the phone a pure observer and keeps rich desktop features in TypeScript (images, skills, picks, commands). No line is built ahead of time and goes stale. A phone cannot inject sandbox or permission overrides.
   - A shared golden fixture is tested by both cargo and vitest, so the Rust and TS formats cannot drift apart.
2. **One FIFO per chat in the daemon.**
   - Limits: 20 entries per chat, 5 per device.
   - The gate opens only when the agent is alive and `Idle`, with no turn in flight, not paused and not held.
   - Turn lines written raw (an older app, adapter pre-thread queues, steer) are observed, never blocked.
   - Stop pauses a queue that is not empty, as on the desktop today. Any client with write permission may cancel any entry. Edit means cancel plus refill the composer, for your own entries only.
   - Desktop chips mirror `chat.queue` and show who queued each message.
3. **Every written turn is logged as `kelpie_sent`.** This gives one record for replay, shows who sent each message, renders ACP prompts, and lets the phone confirm "sent" even after a lost reply.
4. **When the window is closed, phone sends still go out.**
   - A desktop entry whose builder is gone: plain text is built by the daemon; anything else is dropped as `needs_window` and shown "Not sent".
   - The daemon answers `-32601` to ACP client requests when no desktop is attached, so a turn cannot hang.
5. **Safety.**
   - All four verbs need write permission. They are never shed, and their replies travel on the writer's own path.
   - `cid` dedupe lasts 10 min. On `unknown`, exactly one retry within 60 s.
   - Rate limit: 1 send/s per device, burst 3. Text is at most 16 KiB.
   - Revoke drops that device's queued entries.
   - Mode switch sends `hold` first, so no turn slips into a hand-off.
6. **Phone UX.**
   - The chat composer uses Enter for a newline and the Send button to send. This avoids the Telex/VNI Enter-commit problem (review S4).
   - A separate Stop button.
   - States shown: queued (n ahead), sent, unknown with Retry/Discard, refused with the text restored.
   - Text only, no attachments, because file transfer is a plan non-goal.
   - Codex `/` and `!` commands are refused from the phone.
7. **API and compatibility.**
   - `API_LEVEL` takes the next free level at commit time; 18 is expected.
   - The app gates the new path on `CHAT_QUEUE_API`. The legacy desktop queue code stays for older daemons.
   - Three commits, each revertable (daemon / desktop / phone). Nothing is persisted.

## Plan inconsistencies noticed (not fixed: outside my file scope)
- `phase-02-phone-client-app.md` frontmatter says `effort: "6d"`, but the plan.md table says 7d. The ~65d total uses the table.
- Phase 3 (`phase-03-kelpie-link-e2e.md:35,52`) lists dedupe windows only for answers and input/key. When it is next edited, it should add `chat_send` at 10 min, as 2b requires.
- The phase-1 never-shed list (`phase-01…:57`) and `docs/architecture.md:404` should gain the four chat verbs. The 2b docs step covers the architecture doc.

## Decisions that need the user
1. **Accept the daemon-owned queue (D14).** It changes how desktop chat sends (it goes through the daemon, gated by API level). Review W5 already flagged this as a user decision.
2. **Cross-device cancel.** May the phone cancel desktop-queued messages, and the desktop cancel phone ones? The plan says yes, because there is one user.
3. **Phone command scope.** Codex `/compact`, `/review`, `/new` and `!cmd` are refused on the phone in this phase. Is "use the desktop" acceptable, or should the daemon learn those commands?
4. **Revoke.** Revoking a device drops its queued messages but lets a turn already running continue. Should revoke also stop that running turn?

Status: DONE_WITH_CONCERNS
Summary: Phase 2b is planned as a daemon-owned, bounded turn queue per chat. Phone turns are built by the daemon (text only) and desktop turns by its adapters at dequeue, with every turn logged as `kelpie_sent`. The phone stays an observer and works with the window closed. `plan.md` now has the 2b row, D14 and the ~65d total.
Concerns/Blockers: D14 changes the desktop send path and needs the user's approval before implementation. `acp-adapter.ts` and the locales have uncommitted edits from other sessions, so coordinate before editing. 2b must start after phase 2 is merged, because it edits the same files.
