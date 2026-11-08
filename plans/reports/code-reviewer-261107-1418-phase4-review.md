# Phase 4 review: idle reports, e2e script, kelpie-call.py, docs

Scope: uncommitted changes on top of 8de1779:
- `subs.rs` (`ChildWatch`), `daemon/sub_rpc.rs`, `daemon.rs`, `session.rs`, `sub_guide.rs`
- `scripts/kelpie-call.py`, new `scripts/e2e/sub_sessions.py`
- `docs/architecture.md`, `docs/user-guide.md`

What I ran: `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 894 passed, 0 failed. I did not run the e2e script or any real agent; for those I read the tester's evidence (`tester-261107-1418-phase4-e2e.md`, final run: no idle mail at startup or after the report, 42/42).

## Minor

### P4-1. Automatic terminal replies count as new input and undo an explicit report
- **Where:** `session.rs` `write_input` (lines ~741-782); `sub_rpc.rs` `sync_turn` uses `last_input()`.
- **Cause:** `activity.last_input` is set for every write except focus reports. That includes mouse reports and replies to the program's queries (cursor position, device attributes), which `is_terminal_report` recognises but still lets through to `last_input`. The terminal view (xterm.js) sends these by itself when the agent queries, for example after a resize or a redraw.
- **Scenario:** a Codex child calls `kelpie_report done` and keeps writing its final answer. The user resizes the window or opens the child, and Codex's cursor query gets an automatic reply. At `agent-turn-complete`, `sync_turn` sees new input and clears `explicit`; `turn_began` (still set from the start of the turn) is at least 3 s old, so an idle mail follows the done report. This is the F1b symptom by another route. The hookless quiet report is affected the same way.
- **Fix:** use the time of the last submit as the turn boundary: `activity.last_enter` (set only when an Enter submits; Kelpie's typed mail ends with its own Enter), or at least `last_typed`, which skips terminal reports. Add a test: report, a terminal reply, turn end: no idle mail.

### P4-2. A `progress` report now silences the turn-end notice for the rest of the turn
- **Where:** `sub_rpc.rs` `sub_report` (`watch.explicit` for every status); `subs.rs` `turn_ended` and `quiet` (`!w.explicit`).
- **Change in behaviour:** before F1b, a screen-derived `Working` cleared `explicit`. Now only new input clears it.
- **Scenario:** a child reports `progress` (the tool description invites this: "progress worth knowing"), works on, and ends its turn without a final report. Its turn end gives no idle mail, and for a hookless agent the 30 s quiet notice never comes either. The parent waits with nothing until it polls. The same holds after `blocked` / `needs_user` followed by more work, though those mostly end in a wait.
- **Fix:** only `done` and `failed` (the verdicts) should mark the turn reported. Alternatively, let `progress` suppress nothing.

### P4-3. Claude or Codex children whose hooks never arrive get no idle notice
- **Where:** `sub_quiet_check` settles these kinds by kind alone (`has_turn_end_signal`).
- **When it happens:** the user disabled hooks (`disableAllHooks`, managed-hooks-only policy), or the hook process timed out under load.
- **Why it is acceptable for now:**
  - The child's own report, `needs_user` and `exited` reports still work.
  - `kelpie_wait` ends on the child's status, not on mail.
  - The brief tells the child to report.
- **Cheap improvement:** fall back to the quiet path when `!hooks_seen()`. For Claude this is reliable early (SessionStart arrives at start-up). For Codex the first notify comes only at the first turn end, so use a longer quiet there, or none. Either way, document the limit.

### P4-4. Some doc text predates F1/F1b and is now wrong
The diff otherwise matches the code. I checked these against it: depth 3, 10 children, auto cap, 4000/50/60 s/20 per minute mail limits, 6000-character delivery, 30 s quiet before typing, Enter after 120 ms, 5-minute ask, 8 pending and 3 per session, `MAX_SUB_CAP` 64, ACP injection in `chat.rs`, births moved on handover, the 1500 s wait cap, generated files, `is_injected_env`.

Stale:
- **`docs/architecture.md`, the "Automatic reports" bullet.**
  - It says "(`ChildWatch`, fed by status changes) … idle after at least 3 s of work and no explicit report this turn → `idle`".
  - Now: Claude and Codex report idle only on `Stop` / `StopFailure` / `agent-turn-complete`, after a turn of at least 3 s, once per turn. Other agents report after 30 s of unbroken idleness following work, once per idle spell, never while the first prompt is still to be typed.
  - An explicit report holds until new input or a hook turn start (`sync_turn`).
  - The mail now reads "went quiet or finished a turn without reporting … may still be working".
  - A Claude/Codex child whose hooks never arrive gives no idle notice.
- **`docs/user-guide.md`, Sub-sessions:** "A child that stops at a prompt or ends without reporting is reported anyway from what Kelpie sees."
  - True for a prompt.
  - For an ending without a report, it should say this is a hint that arrives at the agent's turn end (Claude, Codex) or after about 30 s of quiet (others). The child may still be working; only its own report means done.
- **`docs/architecture.md`, the API 30 line:** `agent.presets[].outcomes` is given as `{count, done_share, median_minutes}`. The code (`sub_stats::AgentStats`) also has `agent` and `judged`. `done_share` is the share of done among judged outcomes and is null without a verdict; `median_minutes` is null when no time is known.
- **`docs/architecture.md`, Agent surface (smaller):** `kelpie_wait` also ends with "nothing to wait for" and with "no sub-session is working" after 3 s. A child that had already exited when the wait began is shown but not waited for.

## Nit
- **N1. Codex can get mail in the middle of a turn.** Its working status is read from the screen, and a mid-turn flicker to Idle can let `deliver_mail` type a parent's message into it. That counts as new input, so a done report given earlier in the same turn is followed by an idle mail at turn end. This comes from the existing delivery rule, and the wording is now harmless. Using `last_enter` (P4-1) does not change it.
- **N2. Hook order under load.** `kelpied hook` waits at most 2 s for the daemon's reply (`main.rs` ~341-345). Under heavy load, a `UserPromptSubmit` for the next turn can be handled before the previous `Stop`. Then `sync_turn` at `Stop` restarts `turn_began`, so that turn's idle notice is dropped (missed, never false). Acceptable.
- **N3. `sub_sessions.py` details.**
  - The PowerShell lines put temp paths in single quotes (`*> '{spawn_out}'`), which breaks for a profile path containing `'`.
  - If the graceful shutdown fails, `daemon.kill()` stops only the daemon process; the long `sleep 900` custom session may outlive it unless the daemon's job object kills it (not checked).

## The questions in this round
- **Does Kelpie's delivery of a parent's mail into a child re-arm wrongly?** No, in the normal case. Mail is typed only into an Idle child, and Kelpie's own typing is real input, so the next turn is a new turn and an idle notice at its end is right. The exceptions are P4-1 (terminal replies) and N1 (Codex flicker).
- **Do the hook turn counter or the locks race?**
  - Every `sync_turn` reads the session's markers, releases that lock, and then takes the watch lock. Nothing holds one while taking the other.
  - `sub.report` syncs before it marks the report. An input that arrives between those two steps makes the explicit mark apply to the new turn (harmless).
  - The hook ordering case is N2.
- **Lock order in `sub_quiet_check`: fine.**
  - The candidates are collected under the watch lock, which is released at the end of the statement.
  - For each candidate, `by_id` (sessions read lock), `info()` and `first_prompt_pending()` (session lock) and `sync_turn` each take and release their lock in turn.
  - The result of `quiet()` is bound to a variable before `report_auto`, which takes the watch lock (`explicit_this_turn`) and the mail lock. This is the double lock the developer hit and fixed.
  - `turn_ended` in `sub_turn_ended` uses `let … else`, so its watch guard is dropped before `report_auto`.
  - Nothing is nested.
- **Is the e2e script safe? Yes.**
  - It removes every `KELPIE_*` variable and sets `KELPIE_ENDPOINT=kelpie-sub-e2e` and a temporary `KELPIE_DATA_DIR` in its own process before loading `kelpie-call.py` and before it connects.
  - It refuses to run if that endpoint already answers.
  - It starts its own daemon, which inherits that environment.
  - `daemon.shutdown {kill_sessions}` goes over a connection to that endpoint.
  - `kill()` targets only the `Popen` PID.
  - It deletes only that endpoint's token file and its own temp folders.
  - It never touches the default endpoint.
- **`kelpie-call.py`: an improvement.** `read_exact` replaces a loop that spun forever on an empty pipe read, and a header read that crashed with `struct.error`. A closed daemon now raises `OSError`, which the existing e2e handlers already catch (for example `acp_chat.py:399`). The frame layout is unchanged (`length` includes the kind byte), and the destructive-call guard is untouched.

## Summary
Ready to commit. Nothing is above minor. I recommend fixing P4-4 (the doc lines) and P4-2 (a one-line status filter) in the same commit; P4-1 is a small change of which input marker is used.

Status: DONE
Summary: blocker 0, major 0, minor 4 (P4-1 terminal replies re-arm, P4-2 `progress` silences the turn-end notice, P4-3 no notice when hooks never arrive, P4-4 stale doc lines), nit 3. Ready to commit.
Concerns: the docs' automatic-report text predates F1/F1b and should be updated with this commit.
