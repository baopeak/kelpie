# Code review: phone phase 2, the SolidJS phone client

Branch `phone/phase2-client`, uncommitted on top of fb0ec7f. Read-only review. Scope is the developer's diff only; other sessions' files were ignored.

## Scope and evidence
- Files read: `app/phone/**`, `app/vite.phone.config.ts`, `app/phone/vite-dictionary.ts`, `app/src/chat/{transport,desktop-transport,chat-model,mode-switch}.ts`, `chat-observer.test.ts`, the `respond` and replay paths of the three adapters, `app/src/lib/composer-core.ts`, the config and CI diffs, `crates/kelpied/build.rs`, the `mobile.rs` diff, `scripts/check-phone-guard.sh`, the `scripts/e2e/mobile.py` diff, the `docs/architecture.md` phone sections, the phase file and plan.md.
- Commands run: `pnpm typecheck` passes. `vitest run phone/src`, plus the observer, composer-core and chat-model tests: 92/92 pass. I also ran a scratch vitest probe outside the repo to confirm C1. I did not run `pnpm build:phone`, because the tester uses `app/dist-phone`. I did not run cargo, and I started no daemon or Vite.

## Overall
The architecture is sound. There is one wire boundary (`link.ts`), the transport seam is narrow, and observer mode is enforced centrally in `ChatController.write`. The desktop path behaves the same as before. The build guard and the placeholder grep are correct, and every view renders text only. The answer retry path has one real defect: a second answer after a failed one sends a wrong payload to Claude, and does nothing on Codex or ACP. There are also several state and UX bugs in the client.

## Critical

**C1. Answering a chat card again after `unknown` or `refused` sends a broken answer (Claude) or does nothing (Codex and ACP).**
- Where: `app/src/chat/chat-model.ts:516-553` (`respondObserved` / `respond`), `claude-adapter.ts:387-399`, `codex-adapter.ts:496-503`, `acp-adapter.ts:402-433`.
- Cause: every adapter's `respond()` deletes its pending entry (`pendingInputs`, `approvals`, `requests`, `elicitations`) on the first call. On `unknown` or `refused`, the card stays open and shows "answer again" (`answerError`, buttons enabled through `chat-view.tsx`'s `enabled()`).
- Failure scenario:
  1. On 4G, the link drops right after the user taps "Allow" on `Bash(rm -rf build)`.
  2. The reconnect takes more than 60 s, or the reply times out after 30 s. The result is `unknown`.
  3. The user taps "Allow" again.
  4. Claude then gets `{"behavior":"allow","updatedInput":{}}`. The tool runs with empty input: either it errors, or, for AskUserQuestion, the questions are lost.
  5. On Codex or ACP the second tap writes nothing. `outcome` is `false`, so `respondObserved` returns silently, and the card stays stuck with live buttons that do nothing.
- Verified: the scratch probe showed the second Claude line is `...{"behavior":"allow","updatedInput":{}}`, and the Codex second tap made 0 calls with the card left on `answerError:"offline"`.
- The test `chat-observer.test.ts:96-107` claims "can be given again". It is a phantom test: it only counts calls and never checks the payload.
- Fix options:
  - Have `ChatController` cache the first answer line per request (`requestId → line`) in observer mode, and resend that same line on a re-tap. This is also what makes the daemon's dedupe give `dup`.
  - Or make the adapters delete their pending entry only once the answer has settled.
- Related: the ACP `auth` card (`acp-adapter.ts:157`, created on live `initialize`) is clickable on the phone but its writes are always dropped. Disable cards for which `answeredRequest` will be null.

## Warning

**W1. A late chat line after leaving a chat corrupts the next open: the history is missing.**
- Where: `phone-state.ts:85-88` sets `chatSeq` for every `chat` message, even when there is no controller. `leave()` (`:221-227`) deletes `chatSeq` before the unsubscribe has taken effect.
- Scenario: the agent is streaming and the user taps Back. A `live` chunk that was already queued in the bulk lane arrives and sets `chatSeq`. The user reopens the chat. A new controller sends `chat_sub {since_seq}` and the daemon answers with a `delta`, which is applied on an empty view with `ready=true`. The phone shows only the tail and says nothing about the rest.
- Fix: in `chat()`, ignore a message when `!chats.has(m.id)`. Better still, read `since_seq` from the controller itself.

**W2. Composer double send.**
- Where: `composer.tsx:71` calls `submit()` without checking `sending()`. Only the button is disabled (`:45`).
- Scenario: the `input` reply waits for the daemon's 250 ms Enter delay plus the round trip. A second Enter on a slow link types the same command twice into the terminal, for example `git push` twice, or a duplicate prompt to Claude.
- A second problem: `box.value = ""` (`:52`) wipes any text typed while the first send was in flight.
- Fix: `if (sending()) return;` at the top of `submit`. Clear the box only if `box.value === check.text`.

**W3. The prompt card's phase carries over to the next prompt.**
- Where: `session-view.tsx:41` is a non-keyed `<Show>`, and `PromptCard`'s `phase` (`prompt-card.tsx:13`) is never reset when `waitingSince` or the prompt changes.
- Scenario: two prompts come back to back. If no frame with `prompt: null` is rendered between them (frames are batched per animation frame, and the daemon renders every 100 ms or more at load), the card stays on "Sent" with its buttons disabled. "One tap for prompts" fails until the user leaves the session and opens it again.
- Fix: key the card on `waitingSince` (`<Show keyed>`), or reset the phase in `createEffect(on(() => props.waitingSince, …))`.

**W4. "Forget this device" and remote unpairing leave session data behind.**
- `cache.ts:66-83`: throttled writes are already scheduled when `clearCache()` runs (`:89`). They fire up to 1.5 s later, reopen the database and write the last list and screen back.
- `forgetDevice` and `unpaired` (`phone-state.ts:93-96`, `:287-290`) also never remove `kelpie-history` or the `kelpie-draft:*` keys in localStorage.
- The composer history records everything sent to a terminal, so a password typed at a `sudo` or `ssh` prompt from the phone stays in plain text and comes back with the up arrow. The code's own comment says "what it saw is no longer its to keep".
- Fix:
  - Cancel the throttle timers in `clearCache`.
  - Remove the history and draft keys on forget and on unpair.
  - Consider not recording history for terminal sessions, or limiting it to sends that were entered and are longer than a few characters.

**W5. Gap against the user's goal: chat sessions cannot be typed into from the phone.** This is not a defect in this diff; it is a gap in the plan.
- The phase file deliberately scopes chat to `chat_sub` plus `chat_answer`. The daemon refuses `input` on a chat with code `chat` (`mobile.rs` test at :1711).
- So the phase success criterion "readable and answerable" is met for agent questions. The plan's own outcome, "reads and answers terminal and chat sessions with one tap or a real keyboard", is met by no phase. Chat mode is the default way of using Claude Code and Codex in Kelpie, so this blocks the user's stated goal.
- Correct design, keeping a single writer per conversation:
  - Today the per-dialect user-turn format and the queue/busy/steer logic live in the desktop's `ChatController`, and they depend on adapter state (Codex `threadId`/`turnId`, ACP `sessionId`). A phone-side adapter writing directly would be a second writer: Codex `turn/start` and ACP `session/prompt` during a running turn fail or race the desktop's queue flush.
  - **Recommended:** a daemon-owned chat input queue. Add a write verb `chat_send {id, text, client_id}` and an equivalent desktop RPC. The daemon serializes the user turns for each chat. The daemon already classifies busy and idle from the agent output (`chat.rs classify`).
  - Formatting stays where the protocol knowledge is, so there are two variants:
    - (a) Port only the three "user turn" builders to `chat.rs`. The daemon writes the next turn when the status is idle. The desktop `ChatController.send/flushQueue` becomes a call to that RPC, so the desktop stops writing turns itself.
    - (b) Keep the builders in TS, and let the desktop app be the only one that formats. The daemon forwards `chat_send` to the attached desktop controller over the phase-7 provider channel (`Role::App`). This works only while the window is open, which goes against "sessions keep running when the window closes".
  - Prefer (a). Either way, dedupe `client_id` like `input` (phase 3's 60 s window). The phone sees its message come back through the log, and nothing is queued offline.
- Placement: not in phase 2. Add a new **phase 2b, "Chat compose from the phone"**, sequenced before phase 3, so that kelpie-link (phase 3) and the native shell (phase 5) carry the verb from the start. It touches `chat.rs`, `mobile/verbs.rs`, `chat-model.ts` and `mode-switch` hand-off semantics. It needs a user decision, because it changes the desktop's send path.

## Suggestions
- **S1. No chat-log cache.** Opening a chat cold while offline shows "Loading the conversation…" forever, which goes against "never block reading". Cache the last 200 `ChatItem`s for each chat (plain JSON; `SHOWN` already caps it) with an "as of" line. Acceptable as a tracked follow-up; it is not a blocker.
- **S2. `chat_resolved` text.** It always says "Answered on another device". `ChatResolved.by` and `answered_by` are available, so reuse `answeredBeforeText` for consistency with the stale text.
- **S3. Sticky Ctrl.** `composer.tsx` `onBeforeInput` returns early on `isComposing` or `inputType !== "insertText"` without disarming Ctrl. On Android Gboard, where letters arrive as composition text, Ctrl stays armed and the letter goes into the box. Add this to the manual IME pass.
- **S4. Enter during composition.** An Enter the guard treats as a composition confirm is not `preventDefault`ed, so a misclassified send leaves a `\n` in the draft. Worth checking during the manual Laban/Gboard pass (`enterkeyhint="send"`).
- **S5. Write gating.** `canAct` allows writes in `degraded`, `busy` and `restarted`, while the plan says "disabled while not `live`". This is a reasonable reading (the socket is up and nothing is retried), but record the deviation.
- **S6. Who sets `KELPIE_REQUIRE_PHONE` in release.** `release.yml` gets it only through `build-release.sh`'s `export`. That works, but the architecture doc says all three set it. Either set it in the workflow env as well or reword the doc.
- **S7. Hidden page.** ScreenSync ack timers and `syncs` entries are never disposed, so up to one ack goes out after the page is hidden. This is minor against "0 requests while hidden".

## Focus areas that check out
- **Observer mode** has no write on attach, replay or live; no `start`, no queue flush, no `session.update`; no `setSetting` or `interrupt` writes, for all three adapters. Each adapter's `reset()` writes nothing. The tests include a desktop control case.
- **Desktop chat path** is equivalent to before:
  - the `handle` is set before the await;
  - `send` is skipped without a handle or during replay;
  - `setConversation` still updates during replay, as before;
  - `answering` is never set on the desktop.
- **Link:**
  - The handshake sends `{token, caps:[ack,hb]}`, answers the daemon's heartbeat with `hb echo`, and probes after 15 s of silence with a 3 s limit.
  - The backoff is 0–200 ms, then 1 s doubling to 30 s, with full jitter.
  - Close code 4401 forgets the token, 4409 waits until the page is looked at, and 4408/4410/4411 reconnect.
  - Resume sends `have_rev` and `since_seq`. A daemon restart (new epoch) resets positions and refuses retries.
  - Retry is exactly once, within 60 s, for answers only. `load.status` reasons match `load.rs`.
- **Security:**
  - No `innerHTML` anywhere; the terminal uses `textContent` and the chat is JSX text.
  - The token is in localStorage and sent only in the first socket message. It never appears in a URL or a log, and `mobile.py` does not print it.
  - The service worker registers only when `isSecureContext`, skips `/ws`, `/pair` and the manifest, and caches only the shell and `/assets`.
  - Embedded files are served by exact table lookup, so there is no path traversal. They get correct MIME types, the strict CSP, `no-store` except immutable `/assets`, and GET/HEAD only.
  - Read-only devices get no composer or key bar, and their cards are disabled.
- **Build:**
  - The guard panics before embedding.
  - The marker is in the binary only in the placeholder case; the test strings are behind `cfg(test)`.
  - CI builds the phone client before cargo, and the release workflow greps both macOS archs.
  - `pnpm build` is unchanged.
  - The import guard catches `@tauri-apps/*` even when it comes in transitively.

## Deviations
| Deviation | Verdict |
|---|---|
| `focus` sent as `sub {focus}`, hidden ⇒ unsubscribe everything | Accept. The verb table is closed. The daemon's `sub` defaults `focus` to true and clears focus on the device's other sessions (`mobile.rs:908-911`). Unsubscribing is stronger than a `visibility` hint. |
| No chat-log cache | Partial gap (S1). Track it as a follow-up. |
| No `en/phone.json` | Accept. `app/src/locales/` has only `vi/`, because English is the key. The spec's `{en,vi}` wording is out of date. |
| Observer enforced centrally, `acp-adapter.ts` untouched | Accept. It is enforced in one place, but fix C1 there too. |

## Test quality
- The link, screen-sync and composer-core tests are behavioural and use a fake clock and socket. They are good.
- `chat-observer.test.ts:96-107` is a phantom test (C1).
- There is no test for:
  - a chat message arriving with no controller (W1);
  - a double Enter (W2);
  - two prompts in a row (W3);
  - the cache after forget (W4).
- End to end, only a Claude chat answer is covered. Codex and ACP answers are unit-tested only. Answer-during-drop and service-worker caching are not covered end to end, as the developer reported.

## Score: 7/10
The structure and the boundaries are solid. C1 must be fixed before merge. W1 to W4 are small, local fixes.

## Recommended actions
1. Fix C1 by caching and resending the first answer line, and replace the phantom test with one that checks the payload of the second send for all three adapters.
2. Fix W1 to W4 and add a regression test for each.
3. User decision on W5: create phase 2b (daemon-owned `chat_send`) before phase 3.
4. Track S1 and the manual IME/PWA pass (S3, S4).

## Unresolved questions
- W5: does the user accept a daemon-owned chat turn queue (variant a), which changes how the desktop's chat send works, or prefer the desktop-forwarded variant (b), which works only while the window is open?
- When a chat `reset` arrives mid-turn, the phone shows cards already answered on the desktop as open, because the log has no answers; tapping one gets "Already answered". Should `chat_sub` also return the daemon's set of answered requests, so those cards arrive already resolved?

Status: DONE_WITH_CONCERNS
Summary: Phase 2 is well structured: observer mode is enforced centrally, the desktop chat path is unchanged, the link protocol and build guard are correct, and there are no XSS sinks. One critical defect: answering a chat card again after `unknown` or `refused` sends Claude an empty `updatedInput`, and does nothing on Codex and ACP. There are also four warnings: missing history after reopening a chat, a composer double send, a prompt card stuck on "Sent", and data left behind after forget.
Concerns/Blockers: C1 blocks merge. W5 (no way to send chat messages from the phone) is a plan gap that needs a user decision and a new phase 2b before phase 3.

---

## Re-review 1 (fix round 1), 2026-10-30

Read-only. I read the fix-round section of the developer report, the tester report and the coordinator's decisions, then the current code. Decisions the coordinator already made were checked against the code, not reopened.
- Ran: `vitest run phone/src src/chat src/lib/composer-core.test.ts`, 14 files and 196 tests, all pass. `pnpm typecheck` is clean. `pnpm i18n:check` reports 0 missing.
- Probes: a scratch vitest probe outside the repo, run twice.
- Not run: cargo, Vite, daemons.

### Earlier findings
| # | Verdict | Evidence |
|---|---|---|
| C1 | **Fixed for the case reported, with a real test.** The resend is byte-identical for Claude, Codex and ACP, and the adapter is asked once. | `chat-model.ts:528-577`: `unsettled` keeps the first line, and `respondObserved` resends it without calling `adapter.respond`. `chat-observer.test.ts:116-150` covers 3 dialects × (unknown, refused) and checks that calls 2 and 3 are `toBe(calls[0])`, that `asked === 1`, and that Claude's `updatedInput.command` is still there. The phantom test is gone. The untracked ACP sign-in card now says "This can only be answered on the computer" (test at `:107`). **But see C2 below.** |
| W1 | Fixed, with a real test | `chat-feed.ts`: `apply()` drops a message when no controller exists, so the position is not moved. `since()` is null without a controller. `fixes.test.ts:13-39`. |
| W2 | Fixed, with a real test | `SendLatch` (`composer-core.ts:68-85`) sets `busy` synchronously. Enter and the button share it (`composer.tsx:41,53`). `unsentRest` keeps text typed during the send. `fixes.test.ts:41-69`. |
| W3 | Fixed | `session-view.tsx:51`: a keyed `<Show>` on `promptKey(prompt, waiting_since)`. Solid's ancestor-first update stops the inner key from reading a null prompt. Unit test for the key identity, plus the back-to-back menu case in `mobile.py`. |
| W4 | Fixed | `cache.ts`: `clearCache` cancels pending throttled writes first (`fixes.test.ts:114`). `wipeTypedText()` runs on forget and on 4401. Typing history is held in memory only, per the decision. The old `kelpie-history` key is removed at start. Nothing typed at a prompt that looks like a password is recorded. |
| S2 | Applied | `resolvedElsewhere(…, who)` uses `answeredBeforeText` (`phone-state.ts:106`, test at `chat-observer.test.ts:164`). |
| S3 | Applied | `composer.tsx:99` disarms Ctrl when the input is a composition or not a plain letter. Gboard is still a manual check. |
| S6 | Applied | `KELPIE_REQUIRE_PHONE: "1"` is now in the build step env of `release.yml`. |
| S7 | Applied | When the page is hidden, ack timers are disposed (`phone-state.ts:345`). Resume uses the last frame acknowledged. |

### Other items checked
- **Stale/dup notice:** shown at session level. `answerPrompt` raises a `NoticeBoard` notice for 8 s with a dismiss button, scoped to its session (`notice.ts`, `session-view.tsx` notice bar). It is built from the reply's `answered_by` and `at`. Tests are at `fixes.test.ts:90-112`. Matches the tester's finding.
- **Conn-state policy:** matches the decision. `canAct` (`conn-state.ts:103-106`) allows `live`, `degraded` and `busy`. `restarted` is allowed only when `!resyncing`. Everything else refuses. The resync ends on the first frame or chat message of every followed session, a `no_session` error, or a 5 s cap (`link.ts:596-625`). The composer box stays editable and only sending is gated. `conn-state.test.ts` pins every state. The plan line is rewritten.
- **`mobile.rs` pairing 503:** correct. The handler itself is unchanged since phase 1: `pairing_paced()` returns 503 before `code.take()`. The new test `a_pairing_attempt_turned_away_for_load_never_looks_at_the_code` proves three things at L3, for both a right and a wrong code: the reply is 503, the code is not spent, and no failure is counted. This is real behaviour and gives no guessing oracle, since codes are 64 hex characters.
- **`acp_chat.py`:** correct. `tauri-driver --native-port <free port>` gives each run its own msedgedriver, where the default 4445 was shared. The developer's root cause (one run's driver driving the other run's app) explains the stray `plugins/acp-lab` the tester found in the mobile data dir.
- **Plan:** D14 and phase 2b are in `plan.md`, sequenced before phase 3.

### New findings

**C2 (Critical, introduced by the C1 fix, narrow trigger). After a chat `reset`, any button on the re-rendered card resends the earlier answer, even when the person chose the opposite.**
- Where: `chat-model.ts:530-534` looks up `unsettled` by card id only, and ignores `optionId` ("whatever button was pressed"). Nothing clears `unsettled`: `applyLog("reset")` (`:406-409`) does not, `resetState` does not, and an epoch change does not (`ChatFeed.forgetPositions` keeps the controllers, `phone-state.ts:71-74`). Meanwhile `upsert` recreates the card without `retry`, so the normal Allow and Deny buttons come back.
- Probe results, run twice:
  - **Claude:** Allow ⇒ `unknown` ⇒ `applyLog("reset", same request)`. The card shows `retry: undefined` with its normal buttons. A tap on **Deny** sends the earlier `allow` line, and the card settles as "Allow".
  - **Codex:** id 0 accept ⇒ `unknown` ⇒ a reset that brings a **different** request with the same JSON-RPC id 0 (`command: "rm -rf ~"`), which is what a restored session's new agent process can produce after a daemon restart. A tap on **Decline** sends `{"id":0,"result":{"decision":"accept"}}`.
- When it happens: an answer whose result was `unknown` or `refused`, followed by a `reset` in the same controller. The most likely `reset` is the epoch change after a daemon restart. Chat sessions persist in `sessions.json` with the same id, and JSON-RPC request ids restart per process. A `since_seq` that has fallen off the log is the other way to get a `reset`.
- The test at `chat-observer.test.ts:138-139` locks in the "other button sends the same line" behaviour, without the guarantee that the UI shows only "Send again".
- Fix (small):
  - In `applyLog` when `mode === "reset"`, clear `unsettled`, and also any `answering` state. The replay rebuilds the adapter's pending entries (`pendingInputs.set`, `approvals.set` and `requests.set` all run during replay), so a fresh tap builds a correct line for whatever request is pending now.
  - Also make `respondObserved` refuse to substitute: when `again` exists and the call did not come from `retryAnswer`, ignore the tap or show "Send again" instead of silently resending.
  - Add tests for reset-then-Deny (Claude) and a reused id after reset (Codex/ACP), asserting that the line sent matches the button pressed.

**W6 (Warning). An epoch change while the page is hidden skips the resync gate.**
- Where: `link.ts:602` arms `awaiting` only when `!this.hideSubs`. `setHidden(false)` (`:289-297`) resubscribes without arming it.
- Effect: during the 6 s `restarted` window, `canAct` is true while the screen view was just reset to empty. This goes against the "restarted only after the resync" decision.
- Fix: arm `awaiting` in `setHidden(false)` whenever `restartedAt` is newer than the last resync. Alternatively, keep a `needsResync` flag set in `onHello` while hidden.

**S8 (Suggestion).** The in-memory typing `history` is module-level (`composer.tsx:15`). It survives "Forget this device" for the rest of the page's life, so after pairing again in the same tab, the up arrow recalls the earlier text. Clear it inside `wipeTypedText`'s callers. Example: an exported `forgetHistory()`.

**S9 (Suggestion).** `answerPrompt` raises the session notice even while the card is still visible and itself shows the stale text, so the message appears twice. Raise the notice only when the card has gone. Cosmetic.

### Regressions
I found none on the desktop path:
- `respond`, `write` and `attach` for a non-observer are unchanged.
- `unsettled`, `written` and `retry` are reached only in observer mode.
- `resolvedElsewhere` gained an optional third argument, and the existing desktop caller (`chat-pane.tsx:198`) still passes two.

C2 is the only regression the fixes introduced.

### Score: 8/10
C1 and W1 to W4 are fixed properly, with behavioural tests, and the decided policies match the code. C2 is a small, contained fix, but it must land before merge, because it can send an approval the person explicitly declined.

### Unresolved questions
- On a C2 re-tap after an `unknown`, should the phone always resend the first answer (current design) or allow a different choice once the card was re-rendered? I recommend clearing `unsettled` on `reset`, so that a re-rendered card is a fresh question.

Status: DONE_WITH_CONCERNS
Summary: Fix round 1 resolves C1 (byte-identical resend, adapter asked once, for all three dialects) and W1 to W4 with real tests. S2, S3, S6 and S7 are applied correctly; the notice, the conn-state policy, the pairing 503 test and the acp_chat port fix check out. One new critical regression from the C1 fix (C2): after a chat `reset`, a tap on any button resends the earlier answer, which can turn a Deny into an Allow, or approve a different request that reuses the same JSON-RPC id after a daemon restart.
Concerns/Blockers (Re-review 1): C2 blocks merge (fix: clear `unsettled` on `applyLog("reset")`, plus tests). W6 (resync gate skipped when the restart happens while the page is hidden) should be fixed in the same round.

---

## Re-review 2 (fix round 2), 2026-10-30

Read-only. I read the "Fix round 2" section of the developer report and the changed code. I ignored kelpie-bc's leftovers, as instructed.
- Ran: `vitest run phone/src src/chat src/lib/composer-core.test.ts`, 14 files and 213 tests, all pass. `pnpm typecheck` is clean. `pnpm i18n:check` reports 0 missing.
- Probes: six scratch vitest probes outside the repo, against the real `ChatController` and adapters.
- Not run: cargo, Vite, daemons.

### C2: closed
- **Code** (`chat-model.ts`):
  - `unsettled` is cleared on `applyLog("reset")` (`:420`).
  - It is cleared again, together with `resolved`, on an epoch change: `ChatFeed.forgetPositions` → `forgetSentAnswers` (`:578`), which runs in the hello handler before any resubscription.
  - It is bound to `cardIdentity`: title, detail, command, plan, questions and option ids. `upsert` restores `retry` only when the identity matches, and drops the saved line otherwise (`:285-292`).
  - An ordinary `respond` is ignored while a saved line exists (`:622`). Only `retryAnswer` resends it, after checking the identity again (`:560-571`).
- **Probes:**
  - **(P1) Claude:** Allow ⇒ `unknown` ⇒ reset ⇒ Deny sends `deny`.
  - **(P2)** After a `delta` re-upsert, the card keeps "Send again". An ordinary Deny is ignored (0 extra calls), and `retryAnswer` sends identical bytes.
  - **(P4) Codex:** id 0 accepted ⇒ epoch change ⇒ a new request reusing id 0 gets no `retry`, and Decline sends `{"id":0,"result":{"decision":"decline"}}`, built fresh.
  - **(P5)** The same id with identical content and no epoch change keeps "Send again". Within one agent process that is the same request: chat sessions cannot be restarted (`daemon.rs:977`), and every way a new agent process can appear goes through an epoch change or a new session id. So I found no identity collision that does not also pass through `forgetSentAnswers`.
- **Tests:** `chat-observer.test.ts` covers reload-then-Deny, a reused id after a restart, an identical resend after a delta, an ordinary button ignored while a resend is pending, and a changed card, for all three dialects. The old assertion that pinned the wrong behaviour is gone.

### W6: closed, and the gate cannot stay stuck
- **Arming:** `link.ts:609-614` arms `awaiting` on every epoch change. The 5 s timer starts only when the page is visible, and `setHidden(false)` arms it if `awaiting` is non-empty (`:295`).
- **The gate opens on any of:**
  - the first frame or chat message of each followed session;
  - a sub/chat_sub error other than `too_fast` (`:329,336`);
  - `unfollow` (`:313`);
  - the 5 s timer once the page is shown;
  - `closed()`, which clears `awaiting` (`:485-487`); the next hello then re-derives it.
- No path leaves `awaiting` non-empty without either the timer or a close. `link.test.ts:334-370` covers 60 s hidden, then show, then the first frame.
- Minor (S11): if the page is hidden again within 5 s of being shown, the timer keeps running and can open the gate while hidden, so the gate fails open early rather than staying stuck. The fix is to clear the resync timer in `setHidden(true)`, since showing the page re-arms it.

### S8, S9: done
- **S8:** `history.ts` `forgetHistory()` runs on forget and on 4401 (`phone-state.ts:114,310`).
- **S9:** `PromptCard` raises the session notice only when the card has already unmounted (`noticeFor(o, !gone)`, `prompt-card.tsx:18-28`); a card still shown says it itself. The unit test covers the "card still shown" case; headless Edge cannot force it.

### Shutdown with the app attached
I accept the developer's explanation: `daemon_client.rs` respawns on a lost connection (`:105-113`, at most once per 5 s). Nothing to fix in phase 2.

### New findings

**W7 (Warning, test harness). `mobile.py`'s daemon restart is not proven isolated and can reach the default daemon or the real data folder.**
- Where: `mobile.py:147-152` calls `smoke_support.restart_daemon(d, kelpied, kill=True)` on every run. `daemon.shutdown {kill_sessions: true}` goes to whatever daemon the app under test is connected to.
- **Gap 1.** The only guard is `Run.boot()` (`smoke_support.py:67-71`), and it checks the app's `platform_info.data_dir`, not the daemon. It is skipped entirely when the script's own env has `KELPIE_DATA_DIR`.
  - Scenario: tauri-driver is started with `KELPIE_DATA_DIR` but without `KELPIE_ENDPOINT`. The app connects to the default endpoint, the user's real daemon, and the guard passes.
  - The first action is then shutting down the real daemon and killing all its sessions. This is exactly the incident in the project memory "E2E never touches the real daemon".
  - The script's earlier `session.close`-all loop had the same exposure; the restart makes it a whole-daemon kill.
- **Gap 2.** If the app has not respawned within 8 s (a loaded host), the fallback `subprocess.Popen([kelpied, "serve"], env=os.environ.copy())` (`smoke_support.py:50-52`) inherits the script's env, not the app's.
  - The docstring tells people to set the isolation variables on tauri-driver, so the script process may well have no `KELPIE_ENDPOINT` or `KELPIE_DATA_DIR`.
  - The fallback then starts a dev `kelpied` on the default endpoint `kelpie-<user>-v<proto>` (`kelpie-proto/src/lib.rs:141-142`), with the real `%LOCALAPPDATA%\Kelpie` data folder.
- Fix, before the next e2e run:
  1. Before restarting, read `daemon.info` and refuse unless `endpoint` is not the default (it does not match `kelpie-*-v<n>`) and `data_dir` is not `%LOCALAPPDATA%\Kelpie`.
  2. Require `KELPIE_ENDPOINT` in the script's own env, matching `daemon.info.endpoint`.
  3. Start the fallback with an explicit env built from `daemon.info`: `KELPIE_ENDPOINT` set to the namespaced name and `KELPIE_DATA_DIR` set to `info.data_dir`.
  - The same guard belongs in `smoke_support.restart_daemon` itself, which `mobile.py` now calls on every run.

**W8 (Warning; existed before round 1, not introduced by round 2). A `reset` that arrives while an answer is in flight lets a second, different answer go out, and the card ends up labelled with the losing answer.**
- Where: `resetState` rebuilds the card without `answering`. `respond` (`:618-622`) then allows a new tap, because the rebuilt card is not `answering` and `unsettled` was cleared.
- Probe P6, with Claude:
  1. Allow is sent and still in flight.
  2. A `reset` arrives, and the user taps Deny, which sends `deny`.
  3. The daemon answers the first `ok` and the second `dup` (same device). Both replies arrive in that order.
  4. **The agent got `allow`, but the card says "Deny".**
- When it happens: a non-epoch `reset` (a stale `since_seq`, or `chat_resync`) during the reply window of an answer, for example right after a reconnect while `resendLost` retransmits it.
- Fix: keep a set of in-flight permission ids in the controller. `upsert` restores `answering: true` for those ids, and `respond` refuses while one is in flight. Clear the set in `settleAnswer` and on an epoch change. Add a test for the P6 ordering.

**S10 (Suggestion).** A card re-upserted under the same index keeps its old `answerError` ("Not sure this was sent…"). This happens with a live re-upsert of a reused id after `forgetSentAnswers` (probe P4): the store merge keeps fields the new item does not mention. `upsert` already clears `retry` for observers; clear `answerError` and `answering` there too.

**S11 (Suggestion).** Clear the resync timer on hide (see W6 above).

### Score: 8.5/10
C2 and W6 are closed properly, with behavioural tests for all three dialects, and S8/S9 are done. Nothing in the product code blocks merge. W8 is a narrow mislabel race that existed before these fix rounds. W7 is in the test harness, but it is a real risk to the user's live sessions, so fix it before anyone runs `mobile.py` again.

### Unresolved questions
- W7: should `smoke_support.restart_daemon` refuse outright unless `KELPIE_ENDPOINT` is set in the script's own process? This would match the project rule. It is also used by `layouts.py` and `notify.py`, which have helpers of their own.

Status: DONE_WITH_CONCERNS
Summary: Fix round 2 closes C2: the saved answer is cleared on reset and on an epoch change, bound to card identity and used only by "Send again"; probes confirm Deny after a reload sends Deny, a reused id after a restart is answered fresh, and the resend stays byte-identical. W6 is closed and cannot stick shut, and S8/S9 are done. Two warnings remain: W7, where `mobile.py`'s per-run daemon restart is not proven isolated (the app-side data-dir guard does not cover the daemon endpoint, and the fallback spawn inherits the script env, which may reach the default endpoint or real data folder), and W8, a narrow race that existed before these fix rounds where a reset during an in-flight answer lets a second answer out and mislabels the card.
Concerns/Blockers (Re-review 2): No product-code blocker. Fix W7 before the next e2e run against this machine. Fix W8 in a small follow-up, or before merge if cheap. S10 and S11 are optional.

---

## Re-review 3 (fix round 3), 2026-10-30

Read-only, focused on W7, W8, S10 and S11. I ignored kelpie-bc's leftovers.
- Ran: `vitest run phone/src src/chat src/lib/composer-core.test.ts`, 14 files and 219 tests, all pass. `pnpm typecheck` is clean. `pnpm i18n:check` reports 0 missing.
- Probes: I re-ran my Re-review 2 scratch probes against the new code.
  - P3: across a reload the card is still `answering`, only `allow` goes out, and it settles as "Allow".
  - P4: a new request reusing the id after a restart has no stale `answerError` and no `retry`.
  - P6: the second tap is now refused, so the probe's own second-reply handle is undefined. That is the expected outcome.
- Not run: cargo, Vite, daemons, e2e.

### W7: closed for the daemon. One gap remains before the check (W9)
- **`isolated_env()`** (`mobile.py:37-50`) runs at import time, before `Run()` creates the driver.
  - It refuses unless the script's own `KELPIE_ENDPOINT` is set and does not fully match the default `kelpie-[A-Za-z0-9_-]*-v\d+` (the real name is `kelpie-<USERNAME filtered>-v<proto>`, `kelpie-proto/src/lib.rs:141-164`).
  - It also refuses unless `KELPIE_DATA_DIR` is set and is not `%LOCALAPPDATA%\Kelpie`, which matches `data_dir()` on Windows (`lib.rs:319-328`).
- **`check_daemon()`** runs before `restart_daemon` and again after it. It requires the daemon's own `daemon.info.endpoint` to end with the isolated name **and** its `data_dir` to equal the script's.
  - `endswith` alone is weak: a name such as `v1` would also match the default pipe.
  - But the data-dir equality cannot hold for the real daemon, whose folder is the real one and which `isolated_env` has already excluded. The two checks together are fail-safe.
- **On refusal,** `SystemExit` bypasses `except Exception`, and `finally` exits 2. Nothing destructive (`daemon.shutdown`, the `session.close` loop, revokes) runs before `check_daemon` (`mobile.py:183-187`).
- **The fallback `kelpied serve`** gets only the checked env (`restart_daemon(..., env=ISOLATED)`), so it can no longer reach the default endpoint or the real data folder.
  - Minor: the allow-list leaves out `USERNAME`, `ComSpec`, `PATHEXT`, `WINDIR` and `PSModulePath`. Sessions started by a fallback daemon get a stripped env, and shells may misbehave in that rare path. This affects the test only, not safety (S12).
- **`smoke_support.restart_daemon(env=None)`** is backward compatible.
  - `layouts.py:72` calls it positionally as `(d, kelpied, kill)` and gets the old behaviour.
  - `notify.py` has its own `restart_daemon` and is self-isolating: it writes its own `KELPIE_ENDPOINT` and `KELPIE_DATA_DIR` into `os.environ` at `notify.py:85-90`, so its spawns are isolated.
- **Other callers (recommendation, not a blocker):** `layouts.py` has the old risk in full. It relies on externally set variables and only on `Run.boot()`'s app-folder guard, then closes all sessions, removes all projects, overwrites settings and restarts the daemon. Move `isolated_env` and `check_daemon` into `smoke_support` and call them from `layouts.py`, and from any script that closes sessions or restarts the daemon.

**W9 (Warning, test harness, minor). `Run.boot()` runs before `check_daemon` and, for `mobile.py`, now never guards.**
- Where: `smoke_support.py:67-71` skips its real-folder check whenever the script's env has `KELPIE_DATA_DIR`. `mobile.py` now *requires* that variable, so for `mobile.py` the boot guard is always off.
- Effect: `boot()` then calls `setSettings({tips:false})` and `setSettings({language:'en'})` on whatever app tauri-driver launched. If tauri-driver was started without the isolation variables, that rewrites the real app state: tips off and the language switched to English. It happens before `check_daemon` refuses the run.
- Not destructive to sessions or the daemon, but it does touch the real data folder, which the guard was meant to exclude.
- Fix (backward compatible): in `Run.boot()`, when `KELPIE_DATA_DIR` is set, require `platform_info.data_dir` to equal it, instead of skipping the check. Alternatively, have `mobile.py` call `check_daemon` before the `setSettings` lines.
- Launching the app itself under a misconfigured tauri-driver cannot be prevented from the script. Document it as an operator rule.

### W8: closed; no leaks or stuck locks
- **How it works** (`chat-model.ts`):
  - The `inFlight` set (`:219`) is added in `settleAnswer` only when something was written (`:604`), and removed when the reply settles (`:614`).
  - `upsert` restores `answering: true` for ids in the set (`:301`).
  - `respond` and `retryAnswer` refuse while an id is in the set (`:573,639`).
  - On an epoch change, `forgetSentAnswers` clears the set and bumps `generation`, and a reply that arrives after that is dropped (`:589-592,611`).
- **Lock release:** a stuck lock would need an answer promise that never resolves. `Link.request` always resolves:
  - `offline` at once;
  - `unknown` after the 30 s `REPLY_MS` timeout;
  - a lost answer is resolved by `giveUp` at the 60 s mark, or by `resendLost` on the next hello;
  - `stop()` and `failPending()` resolve everything.
  - So every lock is released within about 90 s at most, and `stop()`/forget releases it at once.
- **No leak:** entries are deleted on settle. An epoch change clears the set. A closed controller is dropped together with its set.
- **Cosmetic only:** if an epoch change happens and no `reset` follows (the chat was never resubscribed), the old card keeps the spinner, because the dropped reply does not patch `answering:false`. A resubscription after a restart always gets a `reset`, which rebuilds the card, so this is cosmetic (S13: patch `answering:false` on the early return).
- **Tests:** `chat-observer.test.ts` covers reload with an in-flight answer, and an old reply after an epoch change, for Claude, Codex and ACP.

### S10, S11: done
- **S10:** in observer mode, `upsert` now states `retry`, `answerError` and `answering` explicitly (`:294-301`). The error is kept only for the same request; P4 shows a reused id starts clean.
- **S11:** `setHidden(true)` clears the resync timer (`link.ts:293`), and showing the page re-arms it. `link.test.ts` covers the case.

### Score: 9/10
The product code is ready. W9 and S12/S13 are small test-harness or cosmetic items.

### Ready to commit?
**Yes: phase 2 is ready to commit.**
- All Critical findings (C1, C2) and product Warnings (W1–W4, W6, W8) are closed with behavioural tests.
- W7 is closed for every path that reaches a daemon.
- W9 should be fixed before the next `mobile.py` run on this machine. It can go in the same commit, as a one-line change in `Run.boot()`, or as an immediate follow-up. It does not affect shipped code.
- Still pending and not blocking a commit, per earlier rounds: the manual iPhone and Android IME, PWA install and offline-start checks.

### Unresolved questions
- Should the `isolated_env` and `check_daemon` guard become shared `smoke_support` policy, used by `layouts.py` and every script that closes sessions or restarts the daemon? Recommended; this is a pre-existing risk outside phase 2.

Status: DONE_WITH_CONCERNS
Summary: Fix round 3 closes W7 for every daemon path (the script's own env is checked at import, the daemon's endpoint and data dir are verified before shutdown, and the fallback spawn gets only the checked env), W8 (the in-flight lock survives reloads, is always released by the link's timeouts, and is cleared and generation-dropped on epoch change), S10 and S11, with tests passing. Phase 2 is ready to commit.
Concerns/Blockers: No blocker. W9: `Run.boot()` skips its folder guard when `KELPIE_DATA_DIR` is set, which `mobile.py` now always requires, so a misconfigured tauri-driver gets the real app's tips and language settings rewritten before `check_daemon` refuses; fix before the next e2e run. `layouts.py` should adopt the same guard (recommendation, not a blocker). Manual device checks are still pending.
