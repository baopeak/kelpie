# Phase 3 attention queue: GUI e2e and regression (rerun on review-fix build)

Date: 2026-10-24 (local ~00:40 to 01:20). Spec: `plans/261023-0048-multi-agent-ide-roadmap/phase-03-attention-queue.md`.
Verdict: GUI e2e still FAILS. Product bugs B1 and B2 open; review fixes did not touch them. Popover lock (stale answer) works. Product code not modified.

## Build and environment

- Rebuilt after the coordinator's note: `cargo build -p kelpied` OK; `cd app && pnpm tauri build --debug --no-bundle` OK (`target/debug/kelpie.exe`, `target/debug/kelpied.exe`). No installer.
- Lock note: no `kelpied`/`kelpie` from `target\debug` was running from my side at rebuild time (my runs shut theirs down). Only the user's installed `%LOCALAPPDATA%\Kelpie` processes (PIDs 7200, 3704) are running; untouched.
- msedgedriver 155 = WebView2 155. Isolated endpoint and data dir per run.

## Results

| Suite | Result |
|---|---|
| `pnpm typecheck` | PASS, 0 errors (earlier chat WIP errors gone) |
| `pnpm test` | PASS 110/110 (11 files) |
| `scripts/e2e/attention.py` (daemon) | PASS 12/12 |
| `scripts/e2e/smoke.py` (GUI, regression) | PASS 16/16 |
| `scripts/e2e/attention_gui.py`, real run | FAIL: 5 pass / 6 fail; aborts at step 3 |
| Same script, focus reports dropped in page (scratch diagnostic, not a verdict) | 28 pass / 3 fail |

### attention_gui.py, real run (no workaround)

- PASS: API 7; Read-Host waits; menu waits; both carry `waiting_since`, first < menu; dashboard column titles.
- FAIL: "opening the dashboard keeps both prompts waiting" (only `first` left in Needs you); Needs you order; "waiting N s" labels; bell dot (`1 need you`, not 2); Ctrl+Shift+. cycle (`first, first, first`); scenario aborts at the Answer button (B1).

### Diagnostic, focus reports dropped (scratch only)

- PASS: dashboard order and labels, bell dot 2, Ctrl+Shift+. cycle and wrap, Answer popover (screen, y/n keys), `y` reaches Read-Host (exactly one `y`), inbox opens menu popover, keys 1/2/3, free text `2` + Enter reaches menu, outside answer locks the popover ("The prompt changed since this was shown.", "Show the new prompt", every key disabled), a click on a disabled key types nothing.
- FAIL: B2 (menu waits again after answer, 3 checks: "nothing waits once the menu is answered", "nothing is waiting any more", and the nobody-waiting toast which follows from it).

## Product bugs

### B1 (high, still open): terminal focus reports count as user input

- Evidence: view `onData` log while the dashboard opens (`scratchpad/diag7.py`): view sends `ESC[I`, `ESC[O`. Daemon `seq` unchanged, no new PTY bytes; session status `waiting` → `idle`.
- Path: `app/src/lib/terminal-host.ts:112,178` (`onData` → `sendInput`) → `crates/kelpied/src/server.rs:129-131` (`Frame::Input` → `entry.input`) → `crates/kelpied/src/session.rs:487-489` (`last_input = now`) → `crates/kelpied/src/status.rs` (screen prompt holds only while `last_input < found`).
- Impact: any focus change off a terminal drops its waiting prompt: dashboard, inbox, palette, quick answer, Ctrl+Shift+. (focuses the target). The Answer button disappears before it can be clicked; "Needs you" empties.
- Fix direction: focus reports (`ESC[I`/`ESC[O`) must not set `last_input`, and should not reach a program that did not request DECSET 1004.

### B2 (high, still open): an answered menu waits again

- Evidence on rebuilt daemon (`scratchpad/diag9.py`, `session.answer` as the popover sends it, `data "2"`, `enter`): `+0.1s waiting`; `+0.5s working` (waiting_since cleared); `+1.3s waiting` (new waiting_since). Screen after answer: `2. Yes, and don't ask again` / `3. No (esc)` / `2` / `ANSWERED=2`.
- Cause: after the answer, program output sets Working; on quiet, `choices::detect` (`crates/kelpied/src/choices.rs:46`) matches the stale menu lines still above the cursor. The new rule "unmarked question+list must end the screen" does not stop this: the menu sits above `ANSWERED=2`, not at the end.
- Impact: false "needs you" after every menu answer; bell dot stays; the nobody-waiting toast never shows.
- Fix direction: an answered prompt must not re-match the same lines; require the prompt at the cursor line, or clear the match on answer until the screen changes.

### B3 (low, superseded): stale-answer error wording

- Previous build: refusal showed `missing param 'waiting_since'`, not "no longer waits" (`quick-answer.tsx`, `daemon.rs:1042` `param()?` before the check at 1044).
- Rebuilt build: the popover locks as soon as the prompt changes, so the click path is not reached from the UI. The daemon-side message is still wrong for any caller that omits `waiting_since`. Low priority.

## Test-side notes

- `scripts/e2e/attention_gui.py` (new, untracked, not committed). Stale-answer check (#5) now expects the lock: "prompt changed" banner, every key button disabled, clicks on them type nothing. Starts and stops its own kelpied and tauri-driver, removes its token and dirs.
- Page checks use DOM `.dash-col`, `.quick-answer`, `.inbox-item`, `.bell-needs-you`. Answer and key buttons use DOM `click()`; the inbox anchor and text field use WebDriver input.
- Pre-existing hygiene gap: `scripts/e2e/attention.py` never deletes its data and cwd folders (`kelpie-attention-*`). Not changed. My runs' folders were removed.
- Cleanup verified: no `kelpie-attention-gui-*` tokens or data dirs left; no `kelpie-regress-*` leftovers; no driver/webview processes of mine. Other sessions' `kelpie-attention-*` folders left alone.

## Recommendations (priority)

1. Fix B1 in the daemon (focus reports are not input). Add a daemon-level case to `attention.py`: a waiting prompt stays waiting after `ESC[O` / `ESC[I` writes.
2. Fix B2. Add a `choices` fixture: an answered menu followed by `ANSWERED=` output must not match. Add an `attention.py` case: after `session.answer`, the session stays not-waiting for 5 s.
3. Re-run `attention_gui.py` after 1-2. The bypass run passes 28 of 31; the rest should follow.
4. Then add `attention_gui.py` to the README e2e table (not done here).

## Unresolved questions

- Why the view sends focus reports: ConPTY or the shell enabling DECSET 1004, or app default? Not checked. Affects only the fix location for B1.
- Does a real keyboard focus change produce the same bytes? Reproduced only through WebDriver. Likely the same, not confirmed by hand.
- Chat views not covered: no focus check there.
- `cargo test` (choices fixtures, `session.answer` unit tests) not run in this pass.
- Results are for the working tree at ~01:08 (rebuild time); other sessions keep editing.

## Artifacts

- Script: `D:/Works/Kelpie/scripts/e2e/attention_gui.py`
- Screenshots (rebuilt real run): `C:/Users/BAOPEA~1/AppData/Local/Temp/claude/d--Works-Kelpie/44e62a07-f61e-4e6f-b8ce-e8b06cca80e6/scratchpad/shots-attention2/` (only checked before the failure point)
- Diagnostics (scratch, not committed): `.../scratchpad/diag7.py` (focus bytes), `diag9.py` (B2), `diag8.py` (bypass run)

Status: DONE_WITH_CONCERNS
Summary: Rebuilt on the review-fix tree. Popover lock works (bypass run). GUI e2e still fails on B1 (focus reports clear "needs you") and B2 (answered menu waits again). Regression: attention.py 12/12, smoke.py 16/16, typecheck 0 errors, vitest 110/110.
Concerns/Blockers: B1 and B2 block the GUI flow until the daemon is fixed.
