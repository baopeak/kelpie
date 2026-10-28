# Code review: Phase 12 "Off-machine notify & limit actions"

Date 2026-10-24 · branch `rebrand/kelpie` @ d7c3fde (uncommitted tree) · reviewer: code-reviewer (read-only)

## Scope
- Daemon: `crates/kelpied/src/notify.rs` (+ `notify/tests.rs`), `scheduled_input.rs`, phase 12 hunks in `daemon.rs`, `session.rs`, `status.rs`, `auth.rs`, `chat.rs`, `mobile.rs`, `mobile/app.js`, `main.rs` (mods only), `Cargo.toml`, `kelpie-proto/src/lib.rs` (API 12; `secret_*` read for namespacing).
- App: `src-tauri/src/accounts.rs`, `lib.rs`, `lib/{store,types,accounts,runs,usage-alerts,settings-catalog,notify-events,notify-sync,limits,limit-time,push-config}.ts`, `components/{limit-banner,pane-view,app,settings/push-settings,settings/interface-settings}.tsx`, locales, docs, `scripts/e2e/notify.py`.
- Other sessions' work (root-user, SSH hosts, usage-meter countdown, proc_tree, plans/research) not reviewed.
- Checks run: `cargo test -p kelpied notify` (24 pass), `cargo test -p kelpied -- continue_timer scheduled_input auth:: limit what_the_user account_switch closed_window` (15 pass), `pnpm typecheck` (clean), `pnpm i18n:check` (0 missing), vitest on the 5 new test files (20 pass). A scratch Node probe confirmed how Solid's root-level `setStore` merges (H3). Not re-run: clippy, the full workspace tests, the e2e scripts, the perf gate.

## Overall assessment
The trust boundaries hold. All 11 new RPCs are user-only because the session roles use an allowlist (`auth.rs:89-97`). The secrets are scoped to the endpoint's service (`kelpie-proto/src/lib.rs:257-296`). TLS verification is on (ureq 3, `rustls` feature: webpki roots, `disable_verification: false`). Lock order is consistent (inner then spec, in `continue_if_limited` and `info_locked`). Errors carry no addresses. The defects are in the semantics of the guards and filters:
- the continue guard can still submit a draft the user never sent;
- the focus suppression mutes the phone exactly when the user has walked away;
- timers vanish on daemon restart;
- switching an account back to the original one is refused.

## Critical
None.

## High

### H1. "Continue at reset" can submit a half-typed draft (blind typing)
- **Where:** `status.rs:102` (`limit_untouched`) and `session.rs:554-576`.
- **What's wrong:** the guard is "no input since the failure was reported" (`last_input <= failed_at`). Claude Code lets the user type into its prompt box while a turn is running. Text typed then, without Enter, stays in the box when the turn fails on `rate_limit`. That text came before `failed_at`, so the guard passes.
- **Failure:** at the reset the daemon types `continue` and, 120 ms later, Enter. That submits `<user's unsent draft>continue`, possibly into a Yolo session. The e2e only covers typing after the failure.
- **Fix:** record when the failed turn started (the first `Working` hook of the turn, i.e. `UserPromptSubmit`) in `Activity`. Require `last_input <= turn_started_at` (the submit Enter is the last allowed keystroke), not `<= failed_at`.
- **Alternative:** refuse when the agent's input line on screen is not empty. This is fragile, but could be added as a second check.
- **Verify:** with real Claude Code. Type while it works, let the turn fail, schedule, and fire.

### H2. A focused window that shows the session mutes the phone even when the user is away
- **Where:** `daemon.rs:277-296` (`raise`).
- **What's wrong:** `looking_at(session)` drops Waiting, Finished and Error events before any push decision. It never checks `os_idle()`.
- **Failure:** the user leaves the desk with Kelpie in front, showing the agent's pane (or the display sleeps without a lock). "Needs you" is never pushed and never reaches the inbox. This is the core use case of the phase. The spec's filter is "window not focused / computer idle", and `IDLE_AFTER` already exists, but it is only used by the optional `away_only` filter.
- **Fix:** treat the user as not looking when `os_idle() >= IDLE_AFTER`:
  ```rust
  let seen = ev.session.as_deref().is_some_and(|s| self.notifier.looking_at(s)) && !self.notifier.idle();
  ```
  Desktop and push still agree, because both then fire. Add a unit test with an injected idle source.

### H3. Timers are lost silently on daemon restart, and the app keeps showing them
- **Where:** `scheduled_input.rs` (in memory only), `persist.rs` (untouched, although the spec lists it), `app/src/lib/limits.ts:42`.
- **What's wrong:** the developer's reason ("a restart changes every generation anyway") covers safety only. The spec requires "survive or cancel + inbox note". Neither happens. "Switch now" after an update restarts the daemon, and the timer disappears with no `scheduled.updated` and no note.
- **App side:** `loadScheduled` calls `setScheduled(Object.fromEntries(list))`. Solid merges objects at the root, so keys the daemon no longer has are never removed. Probe: setting `{a:1,b:2}` and then `{}` leaves `{"a":1,"b":2}`. When that session later hits the limit again, the banner shows "Will type “continue” at <old time>" with a Cancel button, for a timer that does not exist.
- **Fix (minimum):**
  - `setScheduled(reconcile(map))`;
  - on reconnect, for each timer the app knew that the daemon no longer lists, add an inbox note ("the session daemon restarted").
- **Fix (better):** also persist the timers in `notify.json` or `sessions.json`. On restore, emit `scheduled.updated {result: cancelled, reason: "restarted"}` for each, so the outcome reaches the inbox (and, optionally, the channels) instead of vanishing.

### H4. Switching back to the original account is refused
- **Where:** `app/src-tauri/src/accounts.rs:285-290`.
- **What's wrong:**
  1. A→B copies `<id>.jsonl` into B.
  2. The conversation continues in B (Claude `--resume` appends to the same file), so B's file grows.
  3. Switching back B→A finds A's older copy. It is a byte prefix of B's file, so `same_content` is false.
  4. Result: "That account already has a different conversation with the same id." The message is false, and the switch is one-way.
- **Why it matters:** alternating between two subscriptions is the main reason to use this feature. The same happens for the files under `<id>/subagents`.
- **Fix:** if `dst` is a byte prefix of `src` (or its first record carries the same `sessionId`), it is an older copy of the same conversation. Replace it through `.part` + rename, optionally keeping `dst.bak`. Refuse only when the contents really diverge. Add a round-trip test (A→B, append, B→A).

## Medium

### M1. The send queue is unbounded and sends are serial
- **Where:** `notify.rs:685-712`.
- **What's wrong:** `mpsc::channel` has no bound. Each job can take 3 channels × (10 s + 3 s + 10 s) = 69 s during an outage, while the limiter still admits about 1 event per session every 30 s.
- **Failure:** with 10 busy sessions and no network, the backlog grows by about 19 jobs a minute. When the network returns, the phone gets a burst of stale events, and new events wait behind them. A slow Telegram also delays ntfy.
- **Fix:** use `sync_channel(32)` with `try_send` (drop and count when full), or drop jobs older than about 5 minutes in the worker. Optionally use one worker per channel.

### M2. The cost-cap message is swallowed by the per-session rate limit
- **Where:** `notify.rs:296-302` (the `key()` is the session id), `daemon.rs:1319` and `usage-alerts.ts:83`.
- **What's wrong:** the app detects a cost-cap crossing when it reads usage at the end of a turn, seconds after the daemon has pushed `finished` for that session.
- **Failure:** `notify.report` is then inside the same 30 s window and is dropped. With "done" enabled, the cost-cap event almost never leaves the computer. This follows the spec literally but defeats the cost-cap switch.
- **Fix:** give `Cost` (and possibly `Error`) its own limiter key, e.g. `format!("{}:{}", kind, session)`, or let it bypass the limiter.

### M3. Logging can expose secrets, and the e2e's "debug log" check proves nothing
- **ureq at trace:** ureq 3 logs `DebugUri` with the full path when trace is on. That is `/bot<TOKEN>/sendMessage` for Telegram, and the secret path of a webhook. Kelpie's subscriber bridges `log` records (tracing-subscriber's default `tracing-log` feature, `fmt().init()`). So `KELPIE_LOG=trace` writes the bot token to `<data>/logs/kelpied.log`. At debug, ureq logs the webhook host (the path is masked).
- **Fix:** always add `ureq=warn` to the filter, e.g. `EnvFilter::try_from_env("KELPIE_LOG")...add_directive("ureq=warn".parse()?)` in `main.rs:279`.
- **The check:** `notify.py:787` starts the daemon with `RUST_LOG=debug`. The daemon reads `KELPIE_LOG` (`main.rs:279`) and writes to `<data>/logs/kelpied.log`, not to stdout. So the "no secret in the daemon's debug log" check scanned an info-level log and an empty stdout capture. The developer report's claim "RUST_LOG=debug log (asserted)" is therefore not established.
- **Fix:** set `KELPIE_LOG=trace` in the script, and scan `<DATA>/logs/kelpied.log` explicitly.

### M4. Keystrokes the terminal makes itself count as "typed since"
- **Where:** `session.rs:523-533`.
- **What's wrong:** only focus reports are excluded. When the window is attached, xterm sends mouse reports (SGR `ESC [ < … M/m`) and query replies (DA, DSR, DECRQM) through `session.input`, and these set `last_input`. Claude Code's fullscreen TUI turns mouse tracking on.
- **Failure:** moving or scrolling the mouse over the pane cancels the timer with "you typed in the session after the limit hit". The error is in the safe direction, but the feature becomes unreliable with the window open, and the inbox reason is false.
- **Fix:** classify terminal-generated sequences (mouse reports, CSI replies) as not-user-input, like `is_focus_report`, or have the app flag them.

### M5. The Definition of done is not met
- Perf gate: run with `--metrics bundle --runs 1` only. The daemon bench and idle CPU/RSS were not measured, although the ticker now runs `observe` and ureq/rustls were added to the daemon.
- UX check: no `ui_tour.py` screenshots (dark / light / compact) of the banner and settings section.
- The manual real Telegram test is not done (the spec requires it once).
- Real ntfy.sh and Codex resume after the rollout copy are not verified either. Codex may index threads outside the dated rollout file.

## Low

- **L1. A session program can re-arm a cancelled timer.** `daemon.rs:910-915`. A program in the session (Claimed role is enough, no token) can call `hook.report StopFailure {error:"rate_limit"}`. That moves `failed_at` past the user's input, so a timer cancelled by "input since" fires again. It can also set `limited`, which shows the banner. No privilege escalation for Session roles (they already have `session.input` on their own session). For Claimed it gains "type continue+Enter". H1's turn-start guard also closes this.
- **L2. Enter is sent unchecked.** `daemon.rs:340-345`. Enter goes 120 ms after `continue` with no re-check. Re-check `pty`, generation and "no input since the text was written" before sending it.
- **L3. Leftover timers produce late, confusing notes.** Closing the session (`close`), switching its account (`set_env_restart`) or restarting it does not cancel the timer. Hours later an inbox note says "continue was not sent because the session was restarted". Call `self.scheduled.cancel(&sid)` in those paths.
- **L4. Partial transcript copies.** `accounts.rs:277`. `files_under(.., 2000)` with depth ≤ 4 truncates silently, so a big `<id>/` folder (sub-agents, tool results) is copied in part. Refuse (or report) when the limit is hit.
- **L5. The screen-text warning comes too late.** `push-settings.tsx`. The toggle saves `include_screen` at once, and the warning appears only afterwards. The next event can already carry screen text. Show the warning in a `confirmDialog` before saving it as on.
- **L6. Fast edits can undo each other.** `push-settings.tsx` `edit()` clones the last fetched config. Two changes made quickly (before the refetch) make the second save revert the first. Serialize the saves, or patch the local state optimistically.
- **L7. The ntfy token can go out in clear.** `notify.rs:146-149, 837`. An `http://` ntfy server receives the bearer token unencrypted. Refuse a token with an http server, or show a warning.
- **L8. No notifications when the API level is unknown.** `store.ts:340`. `legacyNotify()` is false when `daemonApi() == 0`, which happens when `daemon.info` failed or the daemon predates API levels. Such a connection gets no notifications at all. Use `daemonApi() < NOTIFY_API` once connected, matching `(info.api ?? 1)` in `checkDaemonApi`.
- **L9. A failed worker start drops every message silently.** `notify.rs:687-711`. If `spawn` fails, the `OnceLock` keeps a sender whose receiver is gone, and every later send is dropped with no further warning. Return without storing, or record the error in the channel status.
- **L10. "Usage limit" also fires on ordinary 429s.** Claude's `rate_limit` covers plain API 429s too (docs: `"error_details": "429 Too Many Requests"`), not only subscription windows. API-key users then see "hit the usage limit". It is harmless thanks to the guard; the banner could word it as "rate limit".
- **L11. Some daemon errors cannot be translated.** `set_env_restart` messages with placeholders (`"only {var} can change"`, `"{other} sessions have no account to switch"`) cannot match `vi/backend.json` keys. Minor, because the app's own checks catch these cases first.
- **L12. A small memory leak.** `waiting_tasks` (`notify.rs:552`) is never pruned when a run is deleted.

### Judgement calls asked for
- **Phone link with the LAN address, sent by default.** Acceptable. The link holds `scheme://<LAN ip>:<port>/#s=<uuid>`, with no code or token (`deep_link` strips any query or fragment, and the base from `mobile.rs:323` has none). The phone page still needs a paired device. The exposure is the ntfy/Telegram operator learning a private IP and port. Recommendation: a visible "Include a link to the phone page" switch in the channel section, and a sentence saying the address goes to the chosen server.
- **SSRF via the webhook URL.** Not an issue. Only the user role can set it, and the daemon runs as the same user, so no boundary is crossed. Redirects are off, and the response body is never returned (only the status code). Document it and move on.
- **TLS.** Verified on: ureq 3.4.2 `rustls` feature = `rustls-no-provider + _ring + rustls-webpki-roots`; `TlsConfig::default()` uses `RootCerts::WebPki` with `disable_verification: false`. Side effect: TLS-intercepting corporate proxies with a private CA will fail ("network error").

## Edge cases found while scouting
- After a usage-limit failure, Claude's `Notification idle_prompt` maps to Idle and keeps the failure (`status.rs:162`). Other notification types map to Waiting and clear `limited`, which cancels the timer as "not limited" (safe).
- `/clear` gives `SessionStart` with a new `session_id`. `agent_session_id` is updated, so the timer is refused as "conversation". The typed `/clear` is also input. Correct.
- `notify.event` can reach the app before the `session.updated` that caused it (tick path in `update_spec`). The label is then built from the previous state. Cosmetic.
- Approvals are raised while `runs_changed` holds the `runs` lock. Safe today because approval events carry no session (`push_context` does not touch `sessions`). Keep it that way, or collect the events and raise them after unlocking.
- `notify.report` takes any `session` string. It is user-only, and the limiter map is pruned past 512 entries. Fine.
- Two app windows each add the inbox entry and desktop notification for one `notify.event`. That matches the old per-window `maybeNotify`.

## Positive observations (risk calibration only)
- The capability matrix test covers all 11 new methods, and the session allowlists make them user-only by construction.
- Under `KELPIE_E2E`, the Telegram address override accepts only `http://` loopback. The reset-margin override is also E2E-gated.
- Every cargo test uses the in-memory `SecretStore`; only `main.rs:230` builds the keyring-backed daemon.
- `continue_if_limited` checks and writes under the same session lock as `answer`.

## Recommended actions (priority order)
1. H1: turn-start based input guard, plus a real-Claude check of the unsent-draft case.
2. H2: idle overrides `looking_at`.
3. H3: `reconcile` in `loadScheduled` and a restart note; ideally persist the timers.
4. H4: allow replacing an older prefix copy; add a round-trip test.
5. M2 limiter key for cost; M1 bounded queue; M3 `ureq=warn` and fix the e2e log check (`KELPIE_LOG`, logs folder).
6. M4: filter terminal-generated input.
7. M5: full perf gate, ui_tour screenshots, the manual Telegram test.
8. Lows as convenient (L2, L3 and L5 are cheap).

## Metrics
- Type coverage: TS strict typecheck clean; one `as NotifyEvent` cast on daemon input (acceptable).
- Tests: phase Rust tests 39 (sampled 39 pass), vitest 20/20; e2e not re-run.
- Lint: i18n 0 missing; clippy not re-run here (the developer reports it clean).
- No plan IDs in code or test names (grep-verified).

## Unresolved questions
- Does Claude Code keep text typed during a turn in the prompt after a `rate_limit` failure? (H1 assumes yes; verify.)
- Does Codex `resume <id>` find a rollout copied into a new `CODEX_HOME` without its state index?
- Should failure notes for timers (H3 and fire-time cancellations) also go to the phone channels? With the window closed, the inbox note is only seen later.

First-round status (superseded by the re-review below): DONE_WITH_CONCERNS. There were four High findings: the unsent draft, the focused-window mute, timers lost on restart, and the refused switch back.

---

## Re-review (2026-10-25)

Checked against the current tree: the developer's report section "Review round", and the phase 12 hunks in `status.rs`, `session.rs`, `daemon.rs`, `notify.rs`, `scheduled_input.rs`, `main.rs`, `accounts.rs`, `limits.ts`, `push-settings.tsx` and `store.ts`. I left alone the usage-meter countdown, kelpie-85's three test hunks, and `D:\kelpie-p9`.

Checks run:
- `cargo test -p kelpied -- notify scheduled continue limit terminal_report what_the_terminal draft cancelled_limit account_switch looking`: 44 pass.
- `cargo test -p kelpie accounts::`: 10 pass, including the round-trip and too-many-files tests.
- `pnpm typecheck`: clean. `pnpm i18n:check`: 0 missing. Vitest on the 6 phase files: 22 pass.

Two throwaway probes (both deleted):
- **Solid:** `setStore(reconcile({b:5}))` over `{a,b}` leaves only `b`, and `reconcile({})` empties the store.
- **Log filter:** a small Rust crate with the exact `main.rs` filter (tracing-subscriber 0.3.23 and log 0.4.34, the same versions as the lock file).
  - With `KELPIE_LOG=trace`, `ureq=trace,trace` or `ureq_proto=trace,ureq=debug`, ureq's debug and trace lines (bot path) are dropped and only ureq warnings pass.
  - `log_enabled!(target: "ureq::util", Trace)` is false, so ureq's `DebugUri` never writes the path.
  - Kelpie's own trace lines still appear.

I did not start a live daemon: `notify.set` would write credential-store entries (scoped to the test endpoint, but still the real store). The developer's e2e covers that path and now uses `KELPIE_LOG=trace` and scans `<data>/logs/kelpied.log`.

### Status of the first-round findings

| Finding | Verdict | Evidence |
|---|---|---|
| H1 unsent draft | **Fixed for the reported case**, two new Mediums below | `status.rs:90-115`: `turn_started` is set on the first `Working` after a non-working report; the guard is `last_input <= turn_started`. `session.rs:554-578` adds the `turn_seq` match. Test `a_draft_typed_while_the_turn_ran_is_not_a_clean_stop`. |
| H2 focused window mutes the phone | **Fixed on Windows** | `notify.rs:613-624`: `looking_at` is false after 5 minutes idle (`GetLastInputInfo`), so the inbox and the phone both get the event. **Other OSes:** `os_idle()` returns `None` (`notify.rs:937`), so the old behaviour stays: a focused window showing the session mutes the phone indefinitely, and `away_only` reduces to "window not focused". See RR-M3. |
| H3 timers lost on restart | **Fixed** | `scheduled_input.rs`: written on every change (`write_atomic`), loaded at start as `lost`, reported once through `session.scheduled {timers, lost}`. `limits.ts:42` replaces the list with `reconcile` (probe-verified) and adds a note for each lost timer ("the session daemon restarted"). `close` and `restart` cancel at once with their own reason (`daemon.rs` `drop_timer`). |
| H4 switch back refused | **Fixed** | `accounts.rs` `transcript_move`: a target that is the byte prefix of the source is replaced, checked per file including sub-agents. A longer target is refused ("newer copy"), a diverging one is "different conversation". Every refusal is decided before any write. More than 2000 files is refused (`MAX_FILES + 1` walk). Round-trip test passes. |
| M1 queue | **Fixed** | `sync_channel(32)` + `try_send`; a full queue drops the newest; a message older than 5 minutes is dropped at dequeue; all drops are counted in `dropped`, shown in settings and logged. |
| M2 cost cap | **Fixed** | `notify.rs:309-316`: the `cost:<session>` key has its own allowance. |
| M3 logging | **Fixed, probe-verified** | `main.rs` filter forces `ureq=warn` and `ureq_proto=warn`; an explicit `ureq=trace` in `KELPIE_LOG` is overridden (probe). |
| M4 terminal reports | **Fixed**, side effects noted in RR-L3 | `session.rs` `is_terminal_report` covers focus, SGR/X10 mouse, DA, CPR, DSR, DECRPM and OSC replies; partial sequences, arrows and paste still count as typing (unit test). |
| M5 Definition of done | **Partly** | Release daemon bench 3.75 s vs 3.91 s limit; bundle 319.0 kB gz. The GUI perf group, `ui_tour.py`, and real Telegram / ntfy / Codex resume remain unverified, with reasons given. The lead should accept or schedule these; they don't block the code. |
| Lows | L2, L3, L4, L5, L6, L7, L8, L9, L10, L12 done; L11 open (accepted) | Spot-checked L2 (`input_if_last_typed_at`), L5 (confirm before screen text), L6 (saves chained), L7 (`is_loopback` gate), L8 (`info ? api ?? 1 : 0`). |

### New or remaining findings

None of these is a safety blocker: each fails toward not typing, or needs code running as the same user. RR-M1 and RR-M2 make the timer feature unreliable in common cases, so they should be fixed before calling the feature done.

**RR-M1 (Medium). A permission or question answer during the turn blocks the timer.**
- **Where:** `status.rs:90-94`.
- **What's wrong:** a `Waiting → Working` transition keeps `turn_started` (by design: "work after a question goes on with it"). The keys the user presses to answer a permission dialog or `AskUserQuestion` set `last_input` after `turn_started`.
- **Failure:** any turn that asked the user something and then hit the limit can never be continued by the timer. It is cancelled with the false reason "you typed in the session after the limit hit". In Claude's default permission mode that covers most turns with tool use. Safe direction, but it defeats the feature.
- **Why the e2e misses it:** the unit test feeds `Waiting → Working` without the answer's keystroke.
- **Fix:** on `Waiting → Working`, move the clean mark to `now` and keep `turn_seq`:
  ```rust
  if status == Status::Working && matches!(self.hook, Some((Status::Waiting, _))) { self.turn_started = Some(now); }
  ```
  While a dialog is shown, keystrokes go to the dialog, not the prompt, and a draft typed during the following working phase is still caught. Add a test where the answer key is typed during `Waiting`.

**RR-M2 (Medium). `limit_dead` is permanent for the whole run of the agent.**
- **Where:** `session.rs:571`, `status.rs:108`.
- **What's wrong:** it is set once, when a timer is refused for input, and reset only by a new spawn.
- **Failure:** after one such cancellation, every later genuine usage limit in that Claude session (sessions run for hours or days) shows no banner and cannot be scheduled.
- **What it buys:** a forged bare `StopFailure` after the user typed is already harmless, because `limit_untouched` fails (input after `turn_started`). The flag only hides the banner, and a forged full turn (the accepted residual) gets past it anyway.
- **Fix:** clear `limit_dead` when a new turn begins (`turn_seq += 1` in `on_hook`), or drop the flag. Update the e2e step "forged report, schedule refused" to expect "fire refused (input)" instead.

**RR-M3 (Medium, non-Windows). The focused-window mute is still unconditional off Windows.**
- **Where:** `notify.rs:937`.
- **Failure:** on macOS (a supported target: `isMac`, keyring `apple-native`), a user who walks away with Kelpie focused on the session gets no phone message. This was H2's failure.
- **Fix, without OS-specific code:** the app already reports `ui.focus`. Add `last_activity` (the latest keydown or pointer move in the window, debounced) and treat "focused but no activity for 5 minutes" as idle when `os_idle()` is `None`. Optionally use CGEventSource idle time on macOS.
- **Short term:** at least state "Windows only" next to the presence filter in the UI. The docs already say it.

**RR-L1 (Low). The remaining forgery, and the decision not to require the token.**
- **The forgery:** a program in the session (Claimed role is enough) fakes `UserPromptSubmit` + `StopFailure rate_limit` after the user typed a draft. `turn_started` moves past the draft. If the user then clicks "Continue at reset", the fire submits draft + "continue".
- **Why it is acceptable:** it needs the user's click. A Session-role program can already type into its own session (`session.input` is in `OWN`). The architecture states same-user code is not a boundary (`docs/architecture.md`, auth section). The turn check voids any timer set before such a forgery. Requiring the token on the hook path would turn the feature off for agents that strip `*TOKEN*` variables.
- **Optional hardening:** a sticky upgrade. Once a session has delivered a token-verified `hook.report`, ignore limit and turn-start reports from the Claimed role for it. That needs the role passed into `Daemon::hook`.

**RR-L2 (Low). A short gap between the submit and the turn-start hook.**
- `turn_started` is the time the `UserPromptSubmit` hook arrives (a `kelpied hook` process start after the Enter, roughly 50-300 ms on Windows).
- Keystrokes typed in that gap count as before the turn and are not caught. Accept and document it.

**RR-L3 (Low). Side effects of the wider "terminal report" rule.**
- `last_input` also drives the prompt-answered detection (`answered_prompt`), bell clearing and the checkpoint "typed" heuristic. A prompt answered by a mouse click (TUIs with clickable options, e.g. opencode or crush, hookless) is no longer recognised as answered. The status can stay "needs you" until the screen changes.
- Modified F3 keys (`ESC [ 1 ; 2 R`) look like cursor-position reports and are not counted either. Harmless.
- **Fix if it matters:** use `is_terminal_report` only for the timer guard (a separate `last_typed`) and keep the old focus-only rule for prompt answering.

**RR-L4 (Low). Lost-timer notes can themselves be lost.**
- `ScheduledInputs::open` deletes `scheduled.json` at start, before any app has called `take_lost`. If the daemon stops again before an app connects, the notes are gone.
- **Fix:** delete the file in `take_lost`, not in `open`.
- Also, `noteCancelled` may run before `store.sessions` is loaded after a restart, so the note says "A session" with no link. Cosmetic.

**RR-L5 (Low). "sent" is announced even if the Enter is withheld.**
- `fire_scheduled` broadcasts `result: "sent"` before `input_if_last_typed_at` decides. If the user types in those 120 ms, "continue" sits unsent in the prompt while the app says "Typed “continue”".
- **Fix:** broadcast after the Enter decision (from the spawned thread), and say so when it is withheld.

**RR-L6 (Low). A failed apply can leave part of the set behind.**
- `transcript_move` applies file by file, each atomically. If a later file fails, earlier files are already in the target home, and a `.part` file can remain.
- No harm follows: the app resumes on the old account, and the leftovers are identical or prefix copies that the next switch accepts.
- "Nothing partially copied" holds per file, not for the set. Remove the `.part` on error for tidiness.

### Still blocking
None for safety. Before the feature counts as done, fix:
- **RR-M1:** the timer is unusable after any permission answer;
- **RR-M2:** one cancellation disables the banner for the rest of the session;
- **RR-M3:** decide between the app-activity fallback and a visible "Windows only" note.

Everything else can be fixed later.

Re-review status (superseded by the final check below): DONE_WITH_CONCERNS. RR-M1, RR-M2 and RR-M3 were open.

---

## Final check (2026-10-25)

Checked against the current tree (`status.rs`, `session.rs`, `daemon.rs` hook and `ui.focus`, `notify.rs` presence, `auth.rs`, `notify-sync.ts`). `cargo test -p kelpied -- status:: session:: auth:: notify scheduled`: 66 pass. Ownership rules respected; nothing edited.

### Verdict: one blocking item (FC-1); everything else verified

**FC-1 (High, BLOCKING). The RR-M1 fix opens the H1 hole again for a draft typed before the dialog.**
- **Where:** `status.rs:101-105`. On `Waiting → Working` the clean mark (`turn_started`) moves to the moment the agent resumes.
- **Failure:** a draft typed while the agent worked before the permission dialog appeared now lies before the mark. Claude Code keeps the prompt box text while a dialog is shown. The sequence:
  1. Turn starts at t0.
  2. The user types a draft without Enter (t1).
  3. A permission dialog appears (t2), and the user answers it (t3).
  4. Work resumes (t4: the mark moves here).
  5. The turn fails on the limit.
  6. `limit_untouched` holds, and at the reset `continue` + Enter submits `<draft>continue`.
- The new tests cover the answer typed during `Waiting` and a draft typed after it. None covers a draft typed before the dialog (`answering_a_prompt_during_the_turn…` starts with `last_typed = at(0)`, before the turn).
- **Fix:** don't move the mark. Ignore only what is typed while the dialog is up. In `write_input`, record a second time, `last_draft`, only when the hook status is not `Waiting`:
  ```rust
  if !is_terminal_report(&bytes) {
      g.activity.last_typed = Some(now);
      if !matches!(g.activity.hook, Some((Status::Waiting, _))) { g.activity.last_draft = Some(now); }
  }
  ```
  `limit_untouched` then checks `last_draft <= turn_started` with the original mark. Delete the `else if` branch in `on_hook`.
- Keys that reach a dialog before its `Waiting` hook arrives count as a draft. That errs toward not typing.
- **Tests to add:** a draft typed before `Waiting`; an answer typed during `Waiting`; a draft typed after `Working` resumes; typing after the failure.

**What the coordinator asked to verify:**
- **RR-M1:** a draft typed after the answer still blocks (the mark moves only on `Waiting → Working`; later typing is newer). Typing after the failure still blocks. Both are in the test. The case not covered is FC-1's draft typed before the dialog.
- **RR-M2:** verified. `limit_dead` is cleared only where `turn_seq` increments (`status.rs:97-100`). A bare forged `StopFailure` never increments it, so it is still refused (test `a_later_genuine_limit_gets_its_banner_again_but_a_bare_report_does_not`). Even with `limit_dead` false, a bare report cannot re-arm after typing, because `limit_untouched` compares with `turn_started`. A forged full turn remains the accepted residual from RR-L1. It is now further limited: after the agent has proven its token, a token-less caller cannot forge a turn at all.
- **`ui.focus active_at`:** verified.
  - Session-role clients cannot set it: `ui.focus` is not in `SCRIPTING`, `OWN` or `CLAIMED` (`auth.rs:89-97`), and the capability test lists it as admin-only.
  - It is the app's wall-clock time on the same machine. Bucketing it to the minute can make the computer count as idle up to about 1 minute early, in the safe direction (the event is sent).
  - An app that sends no `active_at` is never idle, so the old mute stays only for apps older than API 12.
  - Windows still uses `GetLastInputInfo` first.
- **`looking_at` locking:** verified, no cycle.
  - `looking_at` takes `ui` in a temporary that is dropped at the end of the `let` (`notify.rs:616`), then `idle_at` takes `ui` again, so there is no re-entry on the non-reentrant parking_lot mutex.
  - `wants_push` holds `config` while taking `ui` (`window_focused`, `idle`), so the order is config then ui. No path takes ui then config: `describe` releases `ui` inside its block before calling `config()`. `set_ui` and `drop_client` take only `ui`.
- **Sticky token lockout:** no realistic lockout.
  - `verified_hooks` lives in `Activity`, which every spawn resets (`session.rs:357`). A restart, an agent upgrade (new process) or a daemon restart therefore starts unverified again.
  - Within one process, hooks inherit one environment. Each agent has a single hook path (Claude `--settings`, Codex `notify`, `agents.rs:3`), so a run cannot mix token and token-less reports.
  - `auth` overwrites `verified` for Session and Claimed callers, so a caller cannot claim it.

**Non-blocking notes:**
- **FC-L1:** for User-role callers, `verified` is absent and counts as true (`daemon.rs:930`). One user-token `hook.report` (a debugging script, the e2e) makes the session sticky. An agent whose hooks are genuinely token-less is then locked out until it restarts. Only the user can trigger this, but making only an explicit `verified: true` sticky (Session role) is a one-line change.
- **FC-L2:** the sticky gate (`daemon.rs:931`) runs after the hook has already updated `agent_session_id` and `model`. A token-less caller can still rewrite the conversation id of a proven session. For the timer that errs safe (refused as "conversation"), but it also changes which conversation a later restart or switch resumes. This behaviour existed before phase 12. Moving the gate to the top of `Daemon::hook` makes the hardening consistent.
- The coordinator's list: RR-L3 to RR-L6 are as described in the developer report (spot-checked `last_typed` vs `last_input` in `write_input` and the post-Enter "sent"). Not re-tested in depth.

### Final verdict
One blocking item: **FC-1**. Fix it with a phase-aware "draft" time instead of moving the mark, and add the four-case test. After that, nothing in phase 12 blocks from this review. FC-L1 and FC-L2 are optional one-line hardenings. The remaining unverified items (real Telegram, ntfy.sh, Codex resume, the presence path on macOS) are for the lead to accept.

Status: DONE_WITH_CONCERNS
Summary: RR-M2, the `active_at` trust boundary, the `looking_at` lock order and the sticky token (no lockout across restarts) are verified, and 66 tests pass. One blocker remains: FC-1, the RR-M1 change, again lets the timer submit a draft typed before a permission dialog, and should use a phase-aware draft time instead.
