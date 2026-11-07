# Phase 3 review: app UI and the carried daemon items

Scope: the uncommitted changes in `D:\kelpie-subsessions` (`git diff` plus untracked files) on top of edc1076 and 63a68d1. That is 19 modified files and 5 new ones (about 650 lines changed, plus about 560 new lines in `sub-sessions.ts`, `sub-session-actions.ts`, `sub-request-card.tsx` and `sub-sessions-strip.tsx`).

Checks run in this review:
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 884 passed, 0 failed, 3 ignored.
- `pnpm typecheck`: clean.
- `pnpm test`: 1523 passed.
- `pnpm i18n:check`: 0 missing.

Nothing was run against a real daemon or the real app. Every finding below comes from reading the code and is cited by file and line.

## Overall

The UI parts are careful. The tree is defended against loops, list identity is kept stable, every piece of request text is rendered as a text node only, and the confirm uses `window.confirm`. The `lineage_from` key is refused for session callers (`parse_create`) and cannot be reached from a phone (the verb table has no `session.create`).

Two of the carried daemon items have gaps that will show up in real use:
- The ACP injection misses the path that `/new` and `/clear` take.
- The view-switch hand-over carries the old process's automatic `exited` report into the new, live session.

---

## High

### H1. ACP `/new` and `/clear` open a conversation without Kelpie's tools

- **Where:** `crates/kelpied/src/chat.rs:1691-1694` (the rewrite exists only in `ChatSession::input`) and `chat.rs:1155-1158` (`commit_turn` writes straight to stdin).
- **Scenario:**
  1. In an ACP chat, the user types `/new` or `/clear`.
  2. The ACP adapter builds `session/new` (`app/src/chat/acp-adapter.ts:83`) inside a queued turn.
  3. The window sends it with `chat.write_turn {lines}` (`chat.rs:1514-1520`). This is the existing test `chat-queue.test.ts:609-617`, which asserts that `asked.write[0].lines` holds `session/new`.
  4. `commit_turn` sends each line with `stdin.send(format!("{line}\n"))` and never calls `with_mcp_server`.
- **Result:** the new conversation has no `kelpie` server, so the agent loses `kelpie_spawn` and the other tools in the middle of the session. The developer report says `input` is "the single path every app-written line takes". That is not true.
- **Fix:** add one helper, for example `fn for_agent(g: &Inner, line: &[u8]) -> Cow<[u8]>`, that applies `with_mcp_server` when `g.acp_mcp` is set. Use it in both `input` and `commit_turn` (the loop at 1156), and check any other writer of app-built lines (`chat.rs:1460`, `1479`). Add a test that pushes `session/new` through `queue_rpc("chat.write_turn")` and asserts the server was added.

### H2. A switched sub-session takes over the old process's automatic `exited` report

- **Where:**
  - `crates/kelpied/src/daemon.rs:1122-1123` and `place_among_subs` at `daemon.rs:200-206` (copies `old.report` as it is).
  - `crates/kelpied/src/daemon/sub_rpc.rs:600-601` (the ticker sets `{status:"exited", auto:true}` on the old session).
  - `app/src/lib/sub-sessions.ts:623-625` (`finishedChildren`).
- **Scenario:** this happens every time a sub-session is switched:
  1. `stopProcess` (`mode-switch.ts:42-58`) waits until the store shows `alive:false`.
  2. That state arrives in the `session.updated` broadcast at `daemon.rs:1641`. In the same synchronous loop iteration, `sub_status_changed` (`daemon.rs:1681-1682`) sets the old session's report to an automatic `exited`.
  3. Only after that does the app send `session.create {lineage_from}`. `place_among_subs(None, Some(old))` then gives the new, live session `report = {status:"exited", auto:true}`.
- **Effects:**
  - The parent's chip and row (`sub-sessions-strip.tsx`) and the parent agent's `kelpie_list`/`session_line` show the live child as "exited". This lasts until the new session's first real turn ends.
  - While the new session is idle, `finishedChildren` (`!alive || idle` together with `report.status === "exited"`) selects it. *Close finished sub-sessions* then offers to close the conversation that was just switched, and lists it in the confirm.
- **Fix:** in `place_among_subs`, take over the report only when it is the agent's own: `old.report.clone().filter(|r| !r.auto)`. Extend `a_session_that_replaces_another_takes_its_place_among_the_sub_sessions` with an `auto: true` exited report and assert that the new session has `None`.

---

## Medium

### M1. Focus item 2: the switch mails the parent a spurious "exited" and ends its `kelpie_wait`

**Assessment:** this is not a "maybe". For a sub-session that is switched, it happens every time, and it has three effects:

- **(a) Mail.** `sub_status_changed` (`sub_rpc.rs:603-607`) queues "exited with code N" for the parent. `deliver_mail` runs every second and types it into an idle parent as a new turn (`sub_rpc.rs:738-780`), usually before `session.create` arrives. `Mailboxes::rename` then re-labels that mail as coming from the new id. An orchestrating agent is told that its child died, and may re-spawn the work or give up.
- **(b) `kelpie_wait`.** A parent blocked in `kelpie_wait` returns "a sub-session exited" (`mcp.rs:619-620`). Until `hand_over_lineage` runs, the old id is still a child with status `exited`, and `exited_before` only excludes sessions that had already exited when the wait began.
- **(c) Stale references.** The parent agent knows the child by the handle/id that `kelpie_spawn` returned. After the switch the old one is closed, so `kelpie_message`/`kelpie_close` to it fail in `resolve` (`mcp.rs:570-575`). Only the name still resolves.

**Cleanest suppression:** the app says "this stop is a switch" before it stops the process. This mirrors what `lineage_from` does after the stop.

1. **Daemon.** Add a user-only flag `session.update {handle, handover: true}`. It sets a transient, unpersisted `handover_until = now + 15 s` on the old session.
   - In `sub_status_changed`, an `Auto::Exited` for a session under hand-over sets no row report and queues no mail.
   - `session_rows` (or `wait_outcome`) treats that session as `switching`, not `exited`. A parent's wait then does not end on it.
   - `hand_over_lineage` clears the flag. If no `lineage_from` arrives before the deadline (the switch failed), the ticker sends the normal exit report then. A failed switch is still reported, only later.
   - In the same `hand_over_lineage`, queue one Kelpie notice to the parent: "sub-session `<label>` continues as #`<new handle>` (`<view>`)". This fixes (c).
2. **App.** In `doSwitchToChat`, `doSwitchToTerminal` and `restartInBypass`, call it just before `stopProcess`. `replaceChat` from `resumeChat` needs nothing, because a restored chat has no process to exit.

**Daemon-only fallback**, if no new parameter is wanted: delay the automatic exit mail by a grace period (about 10 s, longer than `stopProcess`'s worst case of 6 s plus the spawn) in a `pending_exits` map. The 1-second delivery tick flushes it. `hand_over_lineage` drops `old`'s entry. `close` flushes it at once if the session still has a parent. This fixes (a) but not (b). In both options, H2's `!auto` filter is still needed.

### M2. Runs made by a switched session stop starting tasks

- **Where:** `crates/kelpied/src/daemon/sub_rpc.rs:614-620` (`run_creator`), `crates/kelpied/src/runs.rs:185` (`Run::creator`), `daemon.rs:1199-1217` (`hand_over_lineage` does not touch runs).
- **Scenario:**
  1. A parent agent makes a run (`run.create` from its session, so `creator = old id`).
  2. The user switches the parent between chat and terminal view, and the app closes the old session.
  3. The next `step_gated` calls `run_gate(old)`, `by_id(old)` is `None`, and every task that has not started fails with "the session that made this run was closed". The task sessions already running were moved to the new parent, but the run was not.
- **Fix:** in `hand_over_lineage`, also re-point `run.creator == old` to `new` under `self.runs.lock()`, and mark the run state dirty. Add a test next to the hand-over test.

### M3. The request card can be answered by accident, and "Always allow" turns the policy on with one click

- **Where:** `app/src/components/sub-request-card.tsx:52-57`, `app/src/styles.css:1561`.
- **What the code shows:**
  - `.sub-cards` is fixed at `right:16px; bottom:36px; z-index:119`, which is above the floating terminal (`styles.css:1075`, z-index 40, `right:18px; bottom:34px`).
  - The cards hold no focus (good), but their buttons work from the instant they mount. They can stay for up to 5 minutes.
  - "Always allow" sets the global policy to `allow`. In Settings, the same change asks for a confirm first (`agent-settings.tsx:142-145`).
- **Scenario:** a card appears under the cursor while the user is about to click near the bottom-right corner, for example in the floating terminal or (likely, but not checked in a running app) a right-hand pane's composer. One stray click can approve the request, or turn on Allow for every session, including requests marked `skips_prompts`.
- **Fix:**
  - Arm the buttons about 500-700 ms after the card mounts, as browsers do for permission prompts.
  - Put "Always allow" behind the same `confirmDialog` as Settings, or move it into a secondary menu.
  - Add a "Later" control that hides the stack and leaves the requests in Activity, so the cards stop covering the composer.

---

## Low

### L1. A connect can drop a live request, and gone requests stay unread in the inbox

- **Where:** `app/src/lib/sub-sessions.ts:703-708` (`replace`) and `app/src/lib/store.ts:431-441`.
- **Scenario:**
  1. The window connects and calls `sub.pending`.
  2. A `sub.request` event is sent after the daemon built the list but before the reply arrives. The event adds the request, then `replace` removes it, because it is not in the older list.
  3. The daemon still waits 5 minutes. The card is gone and only the inbox item remains.
- **Second problem:** requests that `replace` drops (they timed out while the window was away, or the daemon restarted) never reach `requestGone`. Their inbox entries stay unread, and `requestInbox` keeps their ids.
- **Fix:** keep entries whose `at` is at or after the time the call started (or merge rather than replace), and call `requestGone` for every id that `replace` drops.

### L2. The cards cannot be reached from a terminal by keyboard

- **Where:** `app/src/components/sub-request-card.tsx` and `app.tsx:196`.
- **Detail:** xterm takes Tab, and there is no command or shortcut that moves focus to a card (developer report, open question 3). Someone using only the keyboard in a terminal pane cannot answer.
- **Fix:** add a palette command ("Answer sub-session request") and a chord that focuses the first card's primary button. Do not focus it automatically.

### L3. *Close finished* orphans grandchildren that are still running

- **Where:** `app/src/lib/sub-sessions.ts:623-625`, `sub-session-actions.ts:775-798`.
- **Detail:** `finishedChildren` checks only the child. If that child has its own sub-sessions still working, closing it leaves them running with a parent that is gone (`daemon.rs:1251`: "its children stay"). They jump to the project list. The confirm does not say this.
- **Fix:** leave out children that have descendants that are alive, or list those descendants in the confirm.

### L4. The badge shows the worst state by colour only

- **Where:** `app/src/components/sidebar.tsx:246-258`, `styles.css:1538-1543`.
- **Detail:** the sidebar's rule (`sidebar.tsx:33`) is "each state has its own shape so color is never the only cue". The `waiting` state has text only in the tooltip.
- **Fix:** add the state glyph (`GLYPH[worst]`) or an `aria-label`.

### L5. A sub-session in another project disappears from that project's list

- **Where:** `app/src/components/sidebar.tsx:120-127`.
- **Detail:** a nested child is removed from its own project's rows but still counted in that project's badges and filter. This only happens if the child's `project_id` differs from its parent's. `sub.spawn` uses `caller.project_id` (`sub_rpc.rs:119`), so it needs a later project change. If the parent's project is folded or filtered out, the child is shown nowhere.
- **Fix:** nest only when the parent shares the child's `project_id`, or when the parent is in "Other sessions".

### L6. Vietnamese wording (`locales/vi/shell.json`)

- "Allow for this session" is "Cho phép cho phiên này", with "cho" twice. Suggest "Cho phép trong phiên này".
- "blocked" (the report status) is "bị chặn", which reads as banned or blocked by someone. Suggest "đang bị kẹt".
- "It will run without asking before it acts." Suggest "Phiên con sẽ hành động mà không hỏi bạn trước."

Everything else reads naturally and uses the existing terms ("daemon phiên", "Hoạt động").

---

## Checked and found fine (focus item 1, item 4)

- **`lineage_from` trust boundary.**
  - Session and Claimed callers reach `session_create_from_session`, and `parse_create` refuses the key (`sub_rpc.rs:254`, with a test).
  - The run engine's `create_inner` builds its own params, and automations call `create` with fixed keys.
  - A phone cannot name `session.create` (`mobile.rs:5708`).
  - A User-token script can use it, but that role can already call `sub.answer` and `daemon.set_policy`, so it gains nothing.
- **Hand-over consistency and loops.**
  - The new id cannot be an existing parent's id (`daemon.rs:1146`).
  - The new session takes `old.parent`, and `old`'s children move to `new` with depth unchanged. No new loop is possible.
  - An unknown `lineage_from` carries nothing.
  - If the create fails, the hand-over never runs (it happens after the insert).
  - Spec locks are taken one at a time and nothing is held across `subs.transfer`.
  - A `births` entry written by the ticker before `transfer` is overwritten (`insert`).
- **ACP rewrite (on the `input` path).**
  - It only acts on stdin, the client-to-agent direction.
  - It matches only the exact method names after a full parse (prompt text that mentions `session/new` does not count).
  - Answers and batches pass through unchanged.
  - A `kelpie` server the window already sent is never duplicated.
  - With the policy off, nothing is set (`daemon.rs:1158`, `1165`).
  - Only the ACP dialect is affected (`set_acp_mcp`).
  - The token lives only in `Inner.acp_mcp` and on the agent's stdin. It is not logged (no tracing in these paths), not added to the chat log (`append` gets only answers and `kelpie_sent`), and not in `SessionInfo`.
  - Replay cannot trigger it, because the app never writes while replaying (`chat-model.ts:435`).
  - One limit: if the `"method"` key comes after the first 300 bytes, the line is not rewritten (it fails open). The app always writes the method first (`acp-adapter.ts:93`).
- **Security in the UI.**
  - Task, name, label and cwd are rendered as text nodes or attributes only.
  - `confirmDialog` is `window.confirm`.
  - The inbox body is text.
  - `KindGlyph` only looks up a table.
  - No token reaches the webview.
- **Render cost.**
  - `sessionTree` reads only `id`, `parent`, `created_at` and `handle`, so a status tick does not re-run it.
  - Children memos keep the same array when nothing changed (`sameList`).
  - A badge is O(subtree) per change.
  - Building the tree is O(n·depth). There is no O(n²) work per update.
- **Settings.**
  - The policy is read from the daemon (on connect and when the page opens) and set with the user's call.
  - Below API 30 the controls are hidden and the restart is offered.
  - The automatic cap shows `sub_auto_max`, which is at most 8, inside the 1-16 input range.

## Recommended order

1. H1: one write helper for both stdin paths, plus a test.
2. H2: take over only reports that are not `auto`, plus a test.
3. M1: mark the switch before the stop (or the grace-period fallback), and send the "continues as" notice.
4. M2: re-point `Run::creator` in the hand-over.
5. M3: arm delay, a confirm for "Always allow", and "Later".
6. The Low items.

## Open questions

- Should the "continues as #N" notice to the parent (M1 c) be mail (a new turn) or only part of the next `kelpie_wait`/`kelpie_list`? Mail is simpler, but it starts a turn in an idle parent.
- Should "Always allow" stay on the card at all (developer report, open question 2)? M3 assumes it stays, behind a confirm.

Status: DONE_WITH_CONCERNS
Summary: Phase 3 UI is sound. Two carried daemon items are broken in real use: the ACP tools are lost on `/new`/`/clear`, and a switched sub-session takes over an `exited` report that offers the live session for *Close finished*. The view switch also mails the parent a false exit, ends its wait, and strands runs.
Counts: Critical 0, High 2, Medium 3, Low 6.

---

# Round 2 (fix round 1)

Checks rerun on the current tree:
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 887 passed, 0 failed, 3 ignored.
- `pnpm typecheck`: clean.
- `pnpm test`: 1526 passed (82 files).
- `pnpm i18n:check`: 0 missing.

Nothing was run against a real daemon or the real app.

## Round 1 findings: verified fixed

| Item | Verdict | Evidence |
|---|---|---|
| H1 | Fixed | `ChatSession::for_agent` is used by `input` (`chat.rs:1701`) and `commit_turn` (`chat.rs:1157`). The other stdin writers send only daemon-built lines: the refusal at `chat.rs:976`, the interrupt at `1470`, cancel answers at `1489`, and phone answers at `1732`. None of them is an opener. The daemon builds no `session/new`, `session/load` or `initialize` line of its own (the only match outside tests is `turns.rs:84-88`, which only reads what was written). The replay/live boundary is unchanged: the window still writes nothing while replaying (`chat-model.ts:435`), and the rewrite is entirely on the daemon side. |
| H2 | Fixed | `place_among_subs` takes the report only if `!r.auto`. Test added. |
| M1 | Fixed | See the hand-over flag lifecycle below. |
| M2 | Fixed | `hand_over_lineage` re-points `Run::creator` from old to new, then calls `runs_changed`. Test added. |
| M3 | Fixed | Each card's buttons are disabled for `ARM_MS = 500` after it mounts, and the timer is cleared with `onCleanup`. "Always allow" asks the same confirm as Settings. "Later" hides the card through `postponeRequest`, and `dispatchRequest` prunes ids that are gone. |
| L1 | Fixed | `replace` keeps requests with `at >= keepFrom`, where `keepFrom` is when the question was sent; daemon and app share the same wall clock. Activity entries left out of the list are marked read with `requestGone` (`store.ts:309`). |
| L2 | Fixed | `sub.review` (`mod+shift+a`) and the palette entry focus the first card. Its primary button gets focus only once armed, otherwise the card itself (`tabindex=-1`). When run from the palette, the panes' `queueMicrotask` refocus runs before the `setTimeout(0)`, so the card keeps focus. |
| L3 | Fixed | `finishedChildren(children, tree)` excludes any child that has a live descendant. The re-check just before closing uses the tree too. |
| L4 | Fixed | The badge has `role=img`, an `aria-label` and tooltip that name the state, and a glyph. |
| L5 | Fixed | A child nests only when its `project_id` matches its parent's (`sidebar.tsx:120-124`), and `subChildren` uses the same filter. |
| L6 | Fixed | The three Vietnamese strings are changed. |

## Hand-over flag lifecycle (the cases you asked about)

- **Who can set it.** `handover` is read only by `update_spec`, behind `session.update`. That method is in neither `SCRIPTING` nor `OWN` (`auth.rs:127-134`), so a program in a session (with or without its token) is refused. A phone cannot name `session.update` (`mobile.rs:5708`). Only the user's connections can set it.
- **The app crashes mid-switch.**
  - Before the stop: the process is still alive. `flush_handovers` (every second, `daemon.rs:1617`) clears the flag at 15 s and reports nothing.
  - After the stop: the exit is reported at 15 s (`!alive && parent`). This is the same report as before, only later.
  - A daemon restart clears `switching` on restore (`daemon.rs:1087`, `1095`).
  - Closing the session removes its entry (`Subs::forget`).
  - The flag cannot outlive 15 s or a restart.
- **The session is restarted rather than switched.** A terminal `session.restart` within the 15 s keeps the same entry with `switching = true` until the deadline. Meanwhile `kelpie_list`/`kelpie_wait` show a live session as `switching`, and an exit of the restarted process in that window is reported only if it is still stopped at the deadline. At worst this lasts 15 s and loses nothing. Acceptable.
- **The flag is set on a session that is not a sub-session.** The only effects are its status showing `switching` in rows (only under scope `all`) for up to 15 s, and `flush_handovers` doing nothing because `parent` is `None`. Harmless.
- **A failed switch.** All three app paths clear the flag (`handover: false`) when the stop or the create fails. If something throws after the create succeeded, `hand_over_lineage` has already cleared the parent, so the flush reports nothing.

## Alias map

I agree with skipping it.
- A name still resolves after a switch.
- The parent is told the new handle once, by mail (which also ends a `kelpie_wait` that names the old id).
- Its `kelpie_list` shows the new child under it.

An alias would add a new id-resolution rule to the client-side `resolve` in `kelpied mcp`, for a case the note already covers. Revisit only if real use shows agents holding on to old handles.

## New findings this round

### N1 (Low). Only the automatic exit is suppressed during a switch; idle and needs-user reports are not

- **Where:** `crates/kelpied/src/daemon/sub_rpc.rs:595`. Only `Auto::Exited` with `switching` returns early.
- **Scenario:** the user switches a working child after the "Interrupt it and continue?" confirm.
  - Terminal: the Ctrl-C pair, or for a chat the interrupt followed by 200 ms and `close_stdin`, could let the status reach `Idle` before `Exited`.
  - `ChildWatch` then gives `Auto::Idle` (the work lasted at least `MIN_WORK`), and the parent is mailed "finished its turn and is idle" with the end of the screen.
- **Verified?** No. This depends on idle detection firing within the second or so before the process exits, which I did not check against real timings.
- **Fix:** while `info.spec.switching` is set, skip every automatic report (`if info.spec.switching { return; }`). `flush_handovers` already sends the exit after the grace period if the switch fails.

### N2 (Low, informational). `Ctrl+Shift+A` is taken from the terminal while a request is open

- **Where:** `app/src/lib/commands.ts:90`.
- **Behaviour:**
  - `match` honours `when` (`commands.ts:324`), so the key is taken only while `subRequests().length > 0`. The existing one-time "kept from the terminal" hint explains it, and the user can rebind it.
  - With the usual key encoding a terminal reads Ctrl+Shift+A as `^A` (readline beginning-of-line, GNU screen's prefix). That `^A` is lost only while a request is waiting.
  - On macOS it is Cmd+Shift+A, which terminals never receive, so there is no clash. No other Kelpie command uses the chord.
- **Fix:** none needed. If it comes up, a chord with no control-character meaning in terminals would avoid it, for example `mod+alt+a`.

### N3 (Low, cosmetic). The parent's badge counts sub-sessions it does not show

- **Where:** `app/src/components/sidebar.tsx:279`.
- **Detail:** `subtreeSummary` counts every descendant in the tree, including ones that L5 now leaves in another project. The parent's badge and its worst state can include a child that is not listed under it.
- **Fix:** none needed now. If it matters, filter the summary with `isNested` the same way `subChildren` does.

## Round 2 status

- Every High and Medium from round 1 is fixed and verified in the code.
- No regressions found in the hand-over flag lifecycle, in the shared write path or the replay boundary, or in card timers and cleanup.

Remaining: Critical 0, High 0, Medium 0, Low 3 (N1, N2, N3).

**Ready to commit.**

Status: DONE
Summary: Round 1 fixes verified (H1, H2, M1, M2, M3, L1-L6). All suites pass. Three new Low items, none blocking.

---

# Round 3 (the "Real-app check fixes", uncommitted on top of de9d485)

Checks:
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 899 passed, 0 failed, 3 ignored.
- `pnpm typecheck`: clean.
- `pnpm test`: 1527 passed (82 files).
- `pnpm i18n:check`: 0 missing.

Nothing was run against a real daemon or the real app.

Round 2's N1 is fixed (in de9d485): while `switching` is set, every automatic report is skipped (`sub_rpc.rs:599-601`), and so is the hook-driven `sub_turn_ended` (`sub_rpc.rs:627`).

## The five items

| Item | Verdict |
|---|---|
| (1) Explicit reports other than `progress` suppress the idle report | Correct for terminal children: `sub.report` calls `sync_turn` first and then `explicit()` (`sub_rpc.rs:501-505`), and the next submitted input or hook turn resets it (`subs.rs:816-818`). The docs match. **Not correct for children in chat view: see R3-1.** |
| (2) Badge uses the children's reports | `stateWithReport` changes only a quiet child (`idle` or `done`), so a working, waiting, failed or exited child keeps its own state. **Stale reports: see R3-1** (permanent for chat children) **and R3-2** (short terminal turns). |
| (3) Cards hidden while Activity is open | Correct. `status-bar.tsx:17` registers `inbox` in the covering set, and `isCovering` reads it. When the panel closes, the cards come back and wait 500 ms again before their buttons work. The shortcut finds the copy in the panel (`[data-sub-request]`) while the panel is open. |
| (4) Nested rows: no relative time, tighter indent | Correct. The rule is limited to `.sub-list` and to ages that are not state labels, so "needs you" and "error" still show. Every row's tooltip now ends with its age, which changes with the 30 s tick (cheap). **The CSS block is in the file twice: see R3-3.** |
| (5) "Later" as a small text button | Placed absolutely inside the now `position: relative` card, last in the DOM and tab order, with a focus ring. **It is not delayed by 500 ms**, despite what the report says: it has `disabled={busy()}`, not `off()` (`sub-request-card.tsx:66`). Harmless, because "Later" only hides the card and the request stays in Activity. Noted in R3-3. |

## On the earlier P4-2 concern (a child reports `blocked`, then quietly finishes the same turn)

I accept it, for these reasons:
- The parent already has the child's own `blocked` mail, which says why it stopped. An idle hint for the same turn would add nothing.
- A parent in `kelpie_wait` still returns when the child stops working, because `wait_outcome` looks at status, not mail.
- A real wait for the user (`Status::Waiting`) is still reported automatically.

What remains is an agent that reports `blocked`, gets itself unstuck and finishes without reporting `done`. Its row and badge stay on "blocked" until its next turn. The brief tells agents to report `done`, so this is acceptable.

## New findings

### R3-1 (Medium). For a child in chat view, an explicit report never expires: no more idle reports, a stale row, and now a permanently amber badge

- **Where:**
  - `crates/kelpied/src/daemon/sub_rpc.rs:610-616`: `sync_turn` passes `(None, 0)` for `Entry::Chat`.
  - `crates/kelpied/src/subs.rs:810-825`: `explicit` is reset only when `input` or `turn_seq` changes. `subs.rs:817` is the only place it is reset.
- **How it goes:**
  1. A child switched to chat view (the supported switch path) calls `sub.report blocked` (or `needs_user`, `done`, `failed`).
  2. Its markers never change, so `explicit` stays true until the session closes.
  3. `ChildWatch::quiet` returns nothing for every later turn (`subs.rs:866`, `w.worked && !w.explicit`). A chat child is judged by quiet time, because `hooked` is false for `Entry::Chat` (`sub_rpc.rs:652`). So the parent gets no "finished its turn" hint again.
  4. `report_auto` leaves the row as it is for an idle report (`sub_rpc.rs:677`).
  5. With item (2), a chat child whose last report was `blocked` or `needs_user` shows "waiting / one needs you" on its parent's badge every time it is quiet, even after it was steered and did more work.
  6. A chat child whose last report was `done` stays in *Close finished sub-sessions* after it has started new work and gone idle again.
- **History:** this existed for `done`/`failed` since de9d485. This round widens it to `blocked`/`needs_user`, and item (2) makes it show as a false "needs you".
- **Fix:** give chat sessions a turn marker for `sync_turn`. The chat already sees each turn line in `observe_written` (`Written::Turn`) and in `commit_turn`. Add a counter there, and pass it as `turn_seq` for `Entry::Chat`. Do not use `activity.last_input`, which permission answers and interrupts also update. Add a test: chat child, `sub.report blocked`, a turn line, quiet, and the idle report and row come back.

### R3-2 (Low). A stale `blocked`/`needs_user` survives a short terminal turn

- **Where:** `crates/kelpied/src/subs.rs:831-833` and `864-866`. The idle report needs `worked`, meaning the turn lasted at least `MIN_WORK` (3 s).
- **Scenario:**
  1. A terminal child reports `blocked`.
  2. The parent answers it. A new input resets `explicit`.
  3. The child replies in under 3 s and goes idle without reporting.
  4. No automatic report replaces the row, so the badge keeps showing it as waiting for the user.
- **Fix:** when `sync_turn` sees a new turn begin, clear a row report of `blocked` or `needs_user`. Those describe a turn that has now moved on, unlike a verdict, which can stay. Doing it on the daemon side also corrects `kelpie_list` for the parent agent.

### R3-3 (Low). Small leftovers

- `app/src/styles.css`: the 11-line block that starts "Nested rows give the name the room" (with `.sub-later`) appears twice, at lines 1572 and 1583. Delete one copy.
- `sub-request-card.tsx:66`: "Later" is not delayed by 500 ms, unlike what the report says. Either change the report's sentence or use `disabled={off()}`. I would keep it working at once: it is harmless and lets the user get the card out of the way fast.

## Round 3 status

- (1), (3) and (4) are correct as made.
- (2) is correct for terminal children.
- (5) is correct except the arming claim.
- The chat-view gap (R3-1) is a Medium: a false "needs you" badge and lost idle reports after a child is switched to chat view.

Remaining: Critical 0, High 0, Medium 1 (R3-1), Low 2 (R3-2, R3-3), plus round 2's N2 and N3 (Low, informational).

**Not ready to commit until R3-1 is fixed.** Alternatively, the coordinator can accept it as a known limit for chat-view children, since it existed before for `done`/`failed`. If R3-1 is fixed (with the test), the rest is Low and the change is ready to commit.

Status: DONE_WITH_CONCERNS
Summary: The five fixes are correct for terminal children. In chat view, a child's explicit report never expires (pre-existing, now wider), which leaves a stale row, lost idle hints and a false "needs you" badge.

---

# Round 4 (the developer's "Fix round 2", uncommitted on top of de9d485)

Checks:
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 901 passed, 0 failed, 3 ignored.
- `pnpm typecheck`: clean.
- `pnpm test`: 1527 passed (82 files).
- `pnpm i18n:check`: 0 missing.

Nothing was run against a real daemon or the real app.

## Round 3 findings

| Item | Verdict | Evidence |
|---|---|---|
| R3-1 | Fixed | `ChatSession.turn_count` goes up in exactly two places, both only for turn lines: `observe_written` on `Written::Turn` (`chat.rs:1006-1007`, the lines the window writes) and `commit_turn` when `turn \|\| turn_line` (`chat.rs:1153-1154`). `commit_turn` covers queued desktop turns, phone messages and Kelpie mail turns (`write_plain` calls `commit_turn(turn: true)`). Each turn is counted once: `commit_turn` does not call `observe_written`, and `input` does not call `commit_turn`. Permission answers, interrupts and `/new` or `/clear` (when they are not turn lines) do not count. `sync_turn` passes the counter as `turn_seq` for `Entry::Chat`. The test covers a `blocked` report, then a permission answer and an interrupt (no change), then a user turn, then the idle report again. |
| R3-2 | Fixed | When `ChildWatch::sync_turn` sees a new turn begin, it returns true and the daemon clears a row report of `blocked` or `needs_user` (`sub_rpc.rs`). `done` and `failed` stay. `sub.report` now runs `sync_turn` before it records the report, so a report made right after an input is not wiped. This also clears an automatic `needs_user` once the user's answer is submitted, which is correct. Three tests. |
| R3-3 | Fixed | `styles.css` has the block once. "Later" stays usable at once (`disabled={busy()}`), and the developer report now says so. |

## Your questions

### Do the attention counts that now include reported states feed anything else? Can they notify twice?

No. `reportedState` is used only in the window:
- `sidebar.tsx`: the dot, the row, the badge, the project counts and the attention filter.
- `status-bar.tsx:45`: the "N need you" count. That item opens the dashboard; it triggers nothing.

Desktop notifications come from status transitions in `store.ts:374`, which still read `next.status`. Phone sync, the daemon's notifier and destinations all use the daemon's own status. I found no tray or taskbar badge in the app (no `setBadge` or overlay icon). So a reported state never raises a second notification.

The count is per session, and a parent's badge does not add to the project or status-bar totals, so nothing is counted twice.

### Is the chat turn counter bumped only on user turns, including queued and mail turns?

Yes, as shown under R3-1. One more effect: a Codex `turn/steer` while a turn is running counts as a turn line. It resets the turn and clears a `blocked` row, which is right, because a steer is new input from someone.

### Did anything parse the raw id that `session_tag` used to contain?

No. `from_label` is display-only everywhere:
- mail lines: `subs.rs:562`, `mcp.rs:637`;
- the request card and Activity fallback: `sub-request-card.tsx:28`, `store.ts:283`;
- the request's `from_label`.

The app identifies sessions by `request.session` and `mail.from_id`, never by the label. The tests that compared labels were updated.

## New findings

### R4-1 (Low). "N need you" counts sessions that the waiting-session commands skip

- **Where:**
  - `status-bar.tsx:45` and the sidebar now count a quiet child whose last report was `blocked` or `needs_user` as waiting.
  - These still use raw status, through `waitingSessions` (`attention.ts:28`, `alive && status === "waiting"`): `attention.next` (`revealNextWaiting`, `store.ts:234-238`), `attention.answer` (`commands.ts:92`), the palette's attention list (`palette.tsx:180`) and the board (`board.tsx:44`).
- **Effect:** the bar can say "1 need you" while "Next Session Waiting for You" answers "No session is waiting for you".
- **Fix:** have `nextWaiting` and the palette's attention list also include `reportedState === "waiting"` (sorted after real prompts, by `report.at`). `attention.answer` should stay on real prompts, because those are the ones the quick-answer popover can answer.

### R4-2 (Low). Agent labels from plugins reach mail without `label_text`

- **Where:** `crates/kelpied/src/daemon/sub_rpc.rs:166-174`. `kind_label` is a preset's `label`. For plugin agents that is free text from `agent.set_custom`, which checks the id but not the label (`agents.rs:130-141`). The session name goes through `subs::label_text`; `kind_label` does not.
- **Effect:** control characters are removed when the mail is typed (`session::typed_text`). A newline survives, though. A plugin label with a newline can start what looks like a new line, for example a fake "From the user:" line, in the paste typed into a parent agent.
- **Who can do it:** only the user's own plugins can set it, so the risk is low.
- **Fix:** use `subs::label_text(kind_label)` in `session_tag`.

## Round 4 status

- R3-1, R3-2 and R3-3 are fixed and tested.
- The new "needs you" rule raises no extra notifications and has no consumers outside the window.
- The `session_tag` change breaks nothing.

Remaining: Critical 0, High 0, Medium 0, Low 2 (R4-1, R4-2), plus round 2's N2 and N3 (informational).

**Ready to commit.**

Status: DONE
Summary: All Round 3 findings are fixed and verified. Two new Low items: the waiting-session commands disagree with the "need you" count, and plugin labels are not passed through `label_text`. Nothing blocks the commit.

---

# Round 5 (R4-1 and R4-2 only)

Checks:
- `pnpm typecheck`: clean.
- `pnpm test`: 1528 passed (82 files).

I did not rerun the Rust suite. The R4-2 change is one line and covered by the existing label tests, which were run by the developer.

## R4-1: fixed

- `lib/session-state.ts` is pure:
  - `sessionState` is the same rule as before;
  - `reportedState` adds the child's report to it;
  - `needsYou` returns 1 for a real prompt, 2 for a reported `needs_user`/`blocked`, 0 otherwise.
  - It imports only the pure `stateWithReport` and types, so there is no import cycle.
- `waitingSessions` lists real prompts first (longest wait first), then reported ones (oldest report first). So `nextWaiting` and the Activity "Needs you" group now agree with the status bar. The dashboard's "Needs you" column, the palette's attention list and the board's count use the same rule.
- **Quick answer stays on real prompts:**
  - The dashboard's Answer button now also requires `s.status === "waiting"` (`dashboard.tsx:68`).
  - `attention.answer` can now pick a reported child only when no real prompt exists, because of the sort order. Even then `openAnswer` requires `waiting_since` and otherwise only shows the session (`store.ts:225-230`). So no text is ever typed into a session that is only quiet.
- **Lists:** Activity shows the project name where a real prompt would show "waiting N s" (`inbox.tsx:61`). The dashboard falls back to the creation time (`dashboard.tsx:65`). Nothing breaks.

## R4-2: fixed

`session_tag` now passes `kind_label` through `subs::label_text`. That turns newlines and quotes into spaces and cuts the label at 60 characters. The label is only shown, and nothing parses it.

## New finding

### R5-1 (Low). A reported child appears twice on the dashboard

- **Where:** `app/src/components/dashboard.tsx:14-18`.
- **Detail:** the "Needs you" column now lists quiet children that reported `blocked` or `needs_user` (through `waitingSessions`). The "Done · unread" and "Idle / exited" columns still use raw status, so the same child also shows in one of them.
- **Fix:** add `&& needsYou(s) === 0` to the tests of those two columns.

## Round 5 status

R4-1 and R4-2 are fixed, with no regressions in these two changes.

Remaining: Critical 0, High 0, Medium 0, Low 1 (R5-1), plus round 2's N2 and N3 (informational).

**Ready to commit.**

Status: DONE
Summary: R4-1 and R4-2 verified. Quick answer still acts only on real prompts. One cosmetic Low: a reported child shows twice on the dashboard.
