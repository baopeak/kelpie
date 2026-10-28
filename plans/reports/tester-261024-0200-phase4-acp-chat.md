# Tester report: phase 4 universal ACP chat (GUI e2e, dev debug build)

Scope: `scripts/e2e/acp_chat.py` (new), `scripts/e2e/fake_acp_agent.py` (test code, edited: `elicitation/create`, `--state`, `--log`, turn pauses). No product code changed. Isolated daemon and data folder; user's installed Kelpie (PID 7200/3704 at start) untouched.

Build: `cargo build -p kelpied` and `pnpm tauri build --debug --no-bundle` (02:10–02:16, after coordinator's review-fix message). No installer built.

## Results

| Suite | Result |
|---|---|
| `acp_chat.py` run 4 (final) | 54 pass, 2 fail (both product findings below) |
| `attention.py target/debug/kelpied.exe` | 12/12 |
| `smoke.py` (isolated daemon + tauri-driver :4444) | 16/16 |
| `cargo test -p kelpied chat` | 4/4 |
| `pnpm test` (vitest) | 133/133 (one earlier failure in untracked phase 5 test, fixed by its owner) |
| `pnpm typecheck` | FAIL: `app/src/components/app.tsx:45` lazy-imports `./cost-page`, file missing (phase 5 WIP, not phase 4) |

## acp_chat.py coverage (run 4)

1. Menu: plugin agents listed ("Fake ACP", "Fake ACP sign-in"); "Chat view" offered; session `chat: true`; chat opens and initialises. PASS.
2. Turn: thinking item kept; tool card "Read notes.txt"; permission card "Allow: Read notes.txt?" with Allow/Reject; status waiting while open; Allow → tool done with output; answer "Done: the file says hello." shown; status `working, waiting, working, idle`; stays idle 5 s after trailing usage_update; title "Fake task: Read notes.txt and report back" in daemon and pane label; composer meter "2,401 of 200,000 tokens". PASS.
3. Pickers from configOptions (Model, Mode; Ask/Auto); choosing Deep → picker shows deep; `session/set_config_option {configId: model, value: deep}` seen in client log. PASS.
4. "ask me" → card "Which colour?" with field "Colour"; waiting while open; "teal" submitted → content.answer reached agent; "You said: teal" shown; idle after. PASS.
5. Kill process → Resume → new live chat session (new id; old closed by design, mode-switch.ts:247); client sent `session/load`; replay shows both prior turns with identical user prompts; status idle immediately and for 5 s; new prompt works. PASS.
6. Sign-in: "Sign in to continue" card with "Fake login"; click → `authenticate {methodId: fake-login}`; session opens; prompt works; idle. PASS.

## Product findings

1. **Trust review omits ACP launch args (medium).** Chat mode starts a plugin agent with `bin + acp` args. The enable review lists only `bin + launch` (`app/src/components/settings/plugin-settings.tsx:32`). Evidence: review text for "Fake ACP" shows only `python`; the args `fake_acp_agent.py --state … --log …` are not shown (check "review shows the acp arguments…" FAIL, run 3 and 4). The reviewer approves a command line that differs from what runs. Fix: include `a.acp` in that `code` line.
2. **Cost lost after trailing usage_update (low).** The fake sends `usage_update` with `cost` then a second `usage_update` without it. Adapter sets `usage` with `cost: undefined` (`acp-adapter.ts`, `usage_update` case), so the meter's "Cost so far" disappears (check FAIL, run 3 and 4). Open question: ACP `usage_update.cost` semantics (see unresolved).
3. **Resumed chat stuck Working (fixed).** Before the review fix, after Resume the replay left status `working` (run 3: `['working']` for 5 s+). Coordinator's `chat.rs` change (answer to `kelpie-session*` sets Idle) fixed it: run 4 PASS.

## Observations (not failing)

- Chat Resume creates a new session id and takes over the pane; a test has to follow the new id.
- Code read, not exercised: `new-session-menu.tsx` `start()` drops the "Extra arguments" (`argv`) for chat sessions (`newChat` is called without it). Confirm with owner.
- Status is observable only with pauses: the fake now waits 0.8 s between steps. Without them, `working` lasts under the 150 ms poll.

## Not covered

Stop/interrupt with open permission or elicitation; legacy `modes`/`models` blocks; `--no-load` (fresh session instead of load); failed `authenticate` retry; multi-field and required/optional elicitation; URL-mode elicitation; daemon restart resume; Claude/Codex regression (not in this run).

## Cleanup

Every process started by the tests was stopped (daemon shutdown with kill_sessions, tauri-driver tree, msedgedriver). Per-run auth tokens removed; isolated data and project folders deleted. Screenshots and logs kept in the scratchpad (`acp-chat-shots`, `acp-chat-run4.log`).

Status: DONE_WITH_CONCERNS

## Unresolved questions

- ACP `usage_update.cost`: cumulative per update (so omitting it means "not reported", keep last value) or per-turn? Decides whether finding 2 is a bug.
- Typecheck is red on `app.tsx` importing `./cost-page` (phase 5 WIP). Who owns it, and should phase 4 wait on it?
- Is the review omitting `acp` args intended? Looks like an oversight, since the review's stated purpose is "every program it can run".
- Extra-arguments field silently ignored in chat mode: intended?
