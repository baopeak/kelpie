# Code Review: Phase 3 (attention queue & quick answer)

## Scope
- Files: choices.rs, status.rs, session.rs, chat.rs, daemon.rs, auth.rs, kelpie-proto lib.rs; app attention.ts(+test), quick-answer.tsx, dashboard.tsx, inbox.tsx, status-bar.tsx, commands.ts, store.ts, types.ts, app.tsx, styles.css, vi locales; scripts/e2e/attention.py
- Diff: about 190 lines changed across 17 files, plus about 330 lines in new files (choices.rs predates this phase; its diff here is only the `line` flag)
- Spec: plans/261023-0048-multi-agent-ide-roadmap/phase-03-attention-queue.md, plan.md "Contract from phase 2"

## Verification run
| Check | Result |
|---|---|
| `cargo test -p kelpied` | 77/77 pass |
| `cargo clippy -p kelpied -- -D warnings` | 3 errors, none in phase-3 hunks (cli.rs:73, daemon.rs:492, runs.rs:343 are older code) |
| `pnpm typecheck` | 3 errors in `src/chat/*`. Those files are being changed by another agent right now and are not in the phase-3 list. No phase-3 errors |
| `pnpm test` | 104/104 pass |
| `pnpm i18n:check` | 0 missing |
| `scripts/e2e/attention.py` (fresh debug kelpied) | 11/11 pass |
| **Ad-hoc probe** (isolated daemon, two scenarios) | **both reproduced, see H1 and H2** |

Probe (scratch script, own endpoint and data dir, the user's daemon was not touched):
- A fake agent prints a `❯ 1. Yes / 2. No` menu, then every 2 s writes `ESC 7 ESC 8` (save/restore cursor: 4 bytes, the screen does not change). Observed status: `waiting(…751054) → working → waiting(…753063) → working → waiting(…755060) …`. **Every repaint makes a fresh Working→Waiting transition and a new `waiting_since`.**
- PowerShell `Get-Content q.txt`, where the file holds `Which option do you prefer?\n1. Keep\n2. Delete`, ends with the shell idle at `PS …>`. Result: **`waiting`, prompt `{keys:[1,2], question:"Which option do you prefer?"}`.**

## Overall assessment
The overall design is sound: detection runs only after output goes quiet, `session.answer` is user-only, and the app hides the feature on an older daemon. The tests pass. Two behaviours break the feature's main promises, though. "Stale answers never land" fails in the app, and any periodic repaint causes notification spam. The probe confirmed both on a real PTY. **Do not ship until H1 to H3 are fixed.**

---

## Critical
None. No trust-boundary violation: `session.answer` is denied to sessions (default-deny, asserted in auth.rs:247 and e2e step 4).

## High

### H1. Repaint flicker: Waiting↔Working on every output byte, new `waiting_since` each cycle, notification spam (confirmed)
- status.rs:114-118: Waiting requires `last_output <= found`. Any output after detection drops the session to Working (status.rs:119-127, `recent && !echo_only`). session.rs:587-589 then waits another 700 ms before it looks again. session.rs:677-682 resets `waiting_since` to `None`, then to a new `now_millis()`.
- The spec says "until input or a **changed screen without a match**". The code uses "any output byte", and `seq` (bytes) instead of screen content.
- Effects:
  - Each cycle is a Working→Waiting transition, so `maybeNotify` (store.ts:332-350) adds an inbox entry, sends a desktop notification and plays the chime whenever the window is not focused.
  - session.rs:686-689 sets `unread` again.
  - The phone buzzes again (mobile/app.js:141).
  - The dashboard's oldest-first order and "waiting N" restart from zero.
  - `session.answer` races against a moving `waiting_since`.
- Real triggers:
  - Remote tmux projects: the tmux status clock repaints every 15 s by default.
  - Agent status lines with timers or spinners.
  - Statusline scripts.
  - Focus-report and resize repaints.
- Fix:
  1. In `Activity::status`, drop the `last_output <= found` condition. Waiting ends on input (`last_input < found`) or when the tick clears it.
  2. In `look_for_prompt`, when the re-check finds the *same* prompt (`==` on keys+question) and there has been no input since `found`, keep the old `activity.screen_prompt` instant. Set it to `now` only when the prompt is new or input came in between. Clear it when the check finds nothing.
  3. Optional: compare a hash of `text_tail` instead of `seq`, so byte-only repaints never trigger a check.
  4. Add a status/tick test: prompt found → repaint output → same prompt again ⇒ status stays Waiting and `waiting_since` is unchanged.

### H2. The stale-answer guard is bypassed in the app: the popover sends the *live* `waiting_since`
- quick-answer.tsx:80 sends `waiting_since: s.waiting_since`. `s` is `store.sessions[id]` at click time, not at open time. The screen resource (quick-answer.tsx:65-68) is keyed only by `handle` and is never fetched again.
- Example: the user opens the popover on Codex "run `cargo build`?". Meanwhile the phone answers it. Codex then asks "run `rm -rf dist`?" and the store gets the new `waiting_since`. The user clicks **y** in the popover, which still shows `cargo build`. The daemon accepts it, because the app sent the current value. This is exactly the case requirement 5 exists to prevent. H1 makes it worse: every repaint changes `waiting_since`.
- Fix:
  - Capture `waiting_since` once, when the screen is fetched. Better: have `session.read` return `waiting_since` from the same lock as `text` and `prompt`, and send that value.
  - When the store's `waiting_since` changes, or the status leaves `waiting`, disable the key buttons and show "the prompt changed" with a Refresh action.
  - Never send `undefined`. Today a stopped wait produces the raw error ``missing param `waiting_since` ``.

### H3. Shell false positive: an idle shell after any output ending in "question? + numbered list" is flagged Waiting (confirmed)
- choices.rs:56-66: the menu may end up to 3 non-empty lines above the bottom, and a question line is enough without a marker. The shell prompt line `PS D:\x> ` does not veto it. The negative fixture `shell_prompt_after_text_mentioning_yes_no` (choices.rs:164-167) only passes because its text has no `?` line. The e2e negative (attention.py:75) has the same gap.
- The probe shows `waiting` with keys `1,2` on an idle shell. It stays that way until the user types, and it notifies like H1.
- The same pattern hits agents without hooks (Aider, Gemini, OpenCode) whose final answer ends in "Which do you prefer?\n1. …\n2. …" above a short input area. For Codex it fires while the user types in the composer (hook Idle no longer wins once `last_input > at`, status.rs:97).
- The spec asks for "prompt must sit at the cursor". The code uses the last non-blank row (vt.rs:222-226), not the cursor.
- Fix:
  - Veto detection when the bottom line looks like a shell or agent input prompt (`^PS .*> ?$`, `[$#%>❯›] ?$` with nothing after it) for menus without a selection marker.
  - Or require the cursor row to be at or below the last menu item and the cursor line to contain no shell prompt.
  - Add fixtures: shell prompt after a question+list, a Codex composer under an assistant answer with a numbered question, Aider `> ` under a numbered question.

## Medium

### M1. The `session.answer` check and the write are not atomic, and the status is up to one tick (250 ms) old
- daemon.rs:1043-1046 compares `s.info()`, which is `last_status` from the previous tick (session.rs:640-643). The lock is then released before `s.input`.
- Say the user answers in the terminal and the tick has not run yet. `last_status` still says Waiting with the same `waiting_since`, so the popover's keys are also typed into whatever comes next.
- The Enter after 120 ms (daemon.rs:1053-1057) is sent without any re-check. A `[Y/n]` prompt that accepts the key alone, or a next prompt that appeared, receives a stray Enter (the default answer).
- Fix: add `Session::answer(since, bytes, enter) -> Result<()>`. It takes the lock, recomputes `g.activity.status(Instant::now())`, checks `g.waiting_since == since && last_input < screen_prompt`, and writes. Before the Enter, verify that no other input arrived after our own write.

### M2. The 120 ms `thread::sleep` blocks every later RPC from that client
- server.rs:85-112 runs one client's requests strictly in order on a single worker. daemon.rs:1055 sleeps inside `dispatch`, so `session.read`, `session.list`, resize and similar calls from the app's single connection wait 120 ms per answer. Raw input frames are not affected.
- Fix: send the Enter from a `tokio::spawn` that runs `sleep(120ms)` and then the guarded input (or `std::thread::spawn`). Reply right away.

### M3. Free-text submit always sends Enter, even for menu prompts that act on the key alone
- quick-answer.tsx:104 sends `enter: true`. For a Claude, Gemini or Codex menu (`line: false`), typing `1` and pressing Enter selects the option and then sends an extra Enter 120 ms later into the next screen. Spec success criterion: answer in at most 2 keystrokes. This is the natural keyboard path.
- Fix: `enter: prompt?.line ?? true`, or when the text is exactly one of `prompt.keys`, use `keyAnswer`.

### M4. Chat sessions never get `waiting_since`
- chat.rs:350 hard-codes `waiting_since: None`, but chat sessions do go Waiting (chat.rs:417-426). `waitingSessions` (attention.ts:14) sorts them last, whatever their real wait. They show `relTime(created_at)` instead of "waiting N". This breaks requirement 2 ("set when a session enters Waiting").
- Fix: track `waiting_since` in `ChatSession::tick` the same way as session.rs:677-682.

### M5. Vendor fixtures are synthetic, and some common prompts are missed
- No fixture is a captured screen. Known gaps:
  - OpenCode's Go TUI permission dialog `Allow (a) / Allow for session (s) / Deny (d)` fails `keys.contains("y")` (choices.rs:81). The `1. Allow` fixture (choices.rs:158-161) is likely not a real OpenCode screen.
  - Current Codex (codex-rs) shows a numbered `› 1. Yes, proceed (y)` list. Menu detection catches it, but whether digit keys select there is not verified.
  - Boxed TUIs (Gemini, Claude) draw a right border `│`, so `ends_with('?')` misses the question (choices.rs:63). Detection still works through the marker, but `question` is None.
  - PowerShell `PromptForChoice` (`[Y] Yes  [A] Yes to All  [N] No … (default is "Y"):`, used by Remove-Item and Install-Module) and `git add -p` `[y,n,q,a,d,e,?]?` are not detected. This matters for the spec's "and shells".
- Fix: capture real `text_tail(14)` from each CLI into fixtures. Strip a trailing `│` before the `?` check. Add PromptForChoice `[X] Word` and `[y,n,…]` patterns, with `line: true`.

### M6. A user typing `(y/n)` or a numbered list in an input line is flagged Waiting after a 700 ms pause
- With hook Idle overridden by input (Codex) or no hooks at all, `found > last_input` after a pause in typing. Examples: PS line `PS> Read-Host "Continue (y/n)"` while typing, Codex composer `› 1. fix A` / `2. fix B`.
- Fix: the same input-line veto as H3. Or skip detection while `last_input` is newer than the last output burst start (the user is the one producing the screen changes).

### M7. Dashboard is clipped at narrow widths
- styles.css:441 uses `repeat(5, minmax(190px, 1fr))`, about 1054 px minimum including gaps and padding. The window minWidth is 760 (tauri.conf.json:19), and `.dashboard` (styles.css:436) has no overflow handling, so up to about 1.5 columns are cut off.
- Fix: `overflow-x: auto` on `.dash-cols`, or `repeat(auto-fit, minmax(190px, 1fr))` with wrapping rows.

## Low
- L1. The detection perf test asserts `< 5 ms` per check (choices.rs:212); the spec budget is ≤ 0.2 ms. It also excludes `text_tail`. Assert 0.2 ms in release, or measure `text_tail` and `detect` together.
- L2. `text_tail(14)` walks *logical* lines (vt.rs:228-240). One huge wrapped line (a minified blob) can make it copy most of the scrollback while holding the session mutex, which stalls `ingest` for that session. Cap it at about 60 rows.
- L3. Mapping `Period` in `eventChord` (commands.ts:254) changes Shift+. from `>` to `.`. Custom keymap entries saved earlier as `mod+shift+>` silently stop matching. If any exist, migrate them or accept both.
- L4. Daemon error text (``the session no longer waits on that prompt``, ``missing param `waiting_since` ``) is shown raw and in English in the popover (quick-answer.tsx:83,107). Map known errors to an i18n string.
- L5. Bell tooltip: the spec says "tooltip shows both counts". The dot has its own title (status-bar.tsx:107), but the anchor title is still "Activity and notifications".
- L6. Answering from the dashboard replaces the `dashboard` overlay with `answer`. `close()` sets the overlay to null, so the user loses the dashboard. Return to the previous overlay instead.
- L7. `session.read` returns `prompt` even when the session no longer waits (session.rs:579-581). The `choices: true` opt-in from the spec is not implemented; it always returns the prompt. Harmless, but document it.
- L8. The "waiting N" labels do not update as time passes (no tick). This matches the existing `relTime`, but with H1 they often show "0 s".

## Answers to the brief, item by item
1. **Detection and precedence.**
   - Hook Idle wins for Claude (`hooks_report_start`, status.rs:97). Claude's own input box, suggestions and pickers never show as Waiting while hooks are on. Correct.
   - For Codex, a mid-turn approval overrides hook Idle once the user has typed (`last_input > at`). Correct.
   - Caveat: a prompt delivered by `deliver_prompt` writes to the PTY directly (session.rs:621) and does not set `last_input`. A daemon-queued second turn on Codex therefore keeps hook Idle, so its approval stays Idle. This predates the phase but now limits it.
   - Misfires: H3 and M6 (shell, composer, prose lists).
   - PSReadLine ListView/inline predictions do not match: no digit item and no y/n.
   - Periodic repaint: H1.
2. **Notification spam.** Yes, confirmed (H1). `waiting_since` is not stable across output.
3. **Performance.** When nothing changed, the tick returns early with no allocation (session.rs:586). One `text_tail` plus regex per quiet screen change is negligible for 20+ sessions. Lock hold is fine except L2. Under H1, each repaint adds 2 extra broadcasts per session.
4. **`session.answer`.**
   - Auth is correct.
   - Key handling is fine (`esc` and `enter` raw; text goes through `typed_text`, which keeps `\n` raw. That is acceptable for the user role).
   - Equality on a ms clock is fine.
   - Problems: M1 (non-atomic, tick-stale status, unguarded Enter), M2 (sleep blocks the client's queue), H2 (the app defeats the guard).
5. **Old daemon (API 6).**
   - `daemonApi` is set on every connection (store.ts:236, :1363).
   - The Answer button is hidden (dashboard.tsx:68).
   - `openAnswer` falls back to `revealSession` because there is no `waiting_since` (store.ts:256-260), so the palette's `attention.answer` also falls back.
   - `attention.next` works from status alone.
   - New daemon with an old app: only additive fields. Pass.
6. **UI.**
   - Escape works: the handler sits on the modal and the input inside it has focus.
   - Problems: stale screen and error (H2, L4), narrow dashboard (M7), tooltip (L5), Period remap (L3).
   - i18n check is clean.
   - `mod+shift+.` has no OS conflict on Windows. On macOS, ⌘⇧. only matters in Finder and file dialogs.
7. **Tests.**
   - Missing: the repaint/flicker stability test (H1), the shell-prompt veto fixture (H3), real vendor captures (M5), and precedence tests for hook Idle versus screen prompt (Claude suppressed, Codex after input).
   - `session.answer` refusal is tested only in e2e, not in `cargo test` (the spec asks for cargo).
   - The GUI half of the e2e (ordering, next-waiting cycling, bell dot) is missing.

## Recommended actions (in order)
1. H1: keep the screen prompt and `waiting_since` across same-prompt repaints, and add the test.
2. H2: send the `waiting_since` captured with the screen (return it from `session.read`), and lock the popover when it changes.
3. H3 and M6: veto shell and input prompt lines, and add negative fixtures, including the probe case.
4. M1 and M2: an atomic `Session::answer` with a live status check, and a non-blocking, re-checked Enter.
5. M3, M4, M7, then M5 fixture captures.

## Ship verdict
**NOT READY.** H1 and H2 break the phase's two headline guarantees: "Needs you" is reliable without spam, and stale answers never land. H3 produces visible false "needs you" on plain shells. All three are small, local fixes.

## Unresolved questions
- Do Codex's numbered approval lists accept digit keys, and does Gemini's confirmation indicator animate? Capture both from the real CLIs.
- Is the intended "needs you" behaviour for an agent's final prose question with numbered options (no hooks) Waiting or Idle? This is a product decision; it affects the H3 veto scope.
