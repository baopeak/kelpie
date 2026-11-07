# Phase 1 (daemon core) - implementation report

Worktree `D:\kelpie-subsessions`, branch `feat/sub-sessions`. Nothing committed.

## Implemented, by requirement
1. **Lineage**: `SessionSpec` gets `parent`, `depth`, `report` (all serde-default, skipped when empty/0). `SubReport {status, summary, at, auto}` in `session.rs`. They reach `session.list` / `session.updated` through the flattened spec. Persisted in `sessions.json`; old files load unchanged (test in `persist.rs`).
2. **Policy**: `policy.json` keys `sub_sessions` (off|ask|allow, default ask) and `sub_max` (0 = auto, max 64). `daemon.policy` returns `agents_control_others, sub_sessions, sub_max, sub_auto_max`. `daemon.set_policy` validates everything first (bad value changes nothing), writes atomically, keeps unknown keys.
3. **`sub.spawn`**: caps (off, restricted, depth 3, 10 children, global cap, L3) checked before asking and again after approval, with a reserved "slot" so concurrent spawns cannot overshoot. Agent must be an installed preset/custom agent (`shell`/`custom` are not presets, so refused). cwd resolved relative to the parent, canonicalised, `\\?\` stripped. `worktree` uses `create_worktree` (refactored out of `create_task_worktree`, same behaviour), branch `kelpie/sub-<slug>-<4hex>`. Yolo flags only when the parent's args had its own preset's yolo sequence. `model` becomes `--model X` for agents that have it (claude, codex, gemini, cursor, copilot, opencode, omp, qwen, aider), else an error. First prompt = `subs::child_brief` placeholder through the normal prompt path. Ask mode: pending request, `sub.request` event (user only), caller's reply deferred on a short-lived `kelpie-sub-ask` thread (5 min, std channel; reply on the caller's `ClientTx`); `sub.answer` once|session|always|deny; `sub.pending`. Result `{id, handle, name, kind, cwd, worktree?}`.
4. **Mailbox** (`subs.rs`): bounded as specified (4000 chars with "... (cut)", 50 queued with dropped counter, 60 s dedupe, 20/min per sender per recipient). `sub.message`, `sub.report`, `sub.inbox {take}`, `sub.close` (descendants only).
5. **Automatic reports**: `ChildWatch` (pure) fed from the ticker's status changes for sessions with a parent: waiting -> `needs_user` (prompt question, else last 3 lines), idle after >= 3 s work with no explicit report this turn -> `idle` (last 12 lines, redacted), exit -> `exited` (code). Row updated with `auto: true`, except an explicit report of the same turn stays. No mail when the parent is gone.
6. **Delivery**: ticker, once a second, `deliver_mail` -> only recipients with mail (one mutex check when none). Terminal: `type_when_idle(text, 30 s)` then Enter after 120 ms via `input_if_last_typed_at`. Only when the session is alive and `Idle` (never waiting/working/exited). Text format and 6000-char cap as specified; the rest stays queued.
7. **Events**: `session.updated` carries the new fields (same scoping). `sub.request` and the extra `sub.request_done {id, decision}` are user-only (explicit arm in `event_visibility`).
8. **Close**: `close()` drops the closed session's mailbox, grant, watch state and cancels its pending requests (`sub.request_done` cancelled). Children keep `parent`.
9. **Auth**: SCRIPTING += `sub.spawn|message|report|inbox|close`; OWN += `sub.report|inbox` (OWN must stay a subset of SCRIPTING, so report/inbox are in both); CLAIMED none; `sub.answer|pending` user-only. `authorize` injects the caller id as `session_id` for the five session methods (overwrites a forged one). `capability_matrix` extended.
10. **API_LEVEL = 30** with a note line.

## Chat sessions as recipients
`ChatSession::enqueue` with `MessageOrigin::Phone` is a daemon-side enqueue that builds the line itself and needs no window, so mail IS delivered to chat recipients (only when the chat is alive and idle; `By {id: "kelpie-sub"}`, cid `kelpie-mail-<id>`). Caveat, documented in a code comment: turning phone access off calls `drop_phones`, which would also drop a queued message of this origin; mail is only enqueued while idle so it is written at once and the window is negligible.

## Files
- modified: `crates/kelpie-proto/src/lib.rs`, `crates/kelpied/src/{session.rs, persist.rs, daemon.rs, auth.rs, main.rs}`
- new: `crates/kelpied/src/subs.rs` (pure rules + state + 25 unit tests), `crates/kelpied/src/daemon/sub_rpc.rs` (RPCs, spawn, delivery, auto reports), `crates/kelpied/src/daemon/sub_rpc/tests.rs` (13 daemon-level tests)
- `runs.rs` not touched: the worktree helper lives in `daemon.rs`.

## Tests
- `cargo test -p kelpie-proto -p kelpied -j 2`: kelpie-proto 29 passed / 2 ignored; kelpied 862 passed / 3 ignored / 0 failed (about 36 new: 25 in `subs`, 13 in `sub_rpc`, 1 in `persist`, `capability_matrix` extended).
- `cargo clippy -p kelpied -j 2 -- -D warnings`: clean.
- Not covered by a unit test (needs a real agent process): the actual start of a child in `spawn_child` and worktree creation for subs (the git logic is the existing, tested helper). Phase 4 e2e covers it.

## Deviations
- `report` is a field of `SessionSpec`, not `SessionInfo` (SessionInfo is flattened over the spec, so the wire shape is identical). Adding a field to `SessionInfo` would have forced edits in `chat.rs` and `notify/tests.rs`, which are outside my ownership. Side effect: the report is persisted with the session.
- Extra event `sub.request_done {id, decision}` (answered, timeout, cancelled) so the UI can clear a card answered elsewhere or timed out.
- `sub.message` to a non-agent session (shell/custom) is refused: delivered text plus Enter would run as a shell command.
- `sub.inbox` returns `{mails: [...], dropped: n}`; `sub.message` returns `{queued, to: {id, handle, label}}`; `sub.report` returns `{delivered_to}`. Phase 2 tools should use these shapes.
- Automatic reports are not rate limited (they follow status changes); explicit ones are (20/min).
- A user's `sub.message` is labelled `you` per the spec, but the delivery header still says "not from the user" (any same-user program can use the user token, so the header must not vouch for it).
- Tests that spawn nothing: an early draft of a cap test reached a real `codex` start; it was changed to assert the live counts instead. No process was left running.

## Open questions
- Startup noise: a child that draws its TUI for >= 3 s before its first turn would produce one automatic `idle` report with the banner. Left as is; could require `turn_seq() > 0`.
- If the Enter after a delivered paste is skipped because the user typed in between, the text stays typed unsent (same behaviour as the continue timer); the mail is already counted as delivered.
- Settings UI (phase 3) must send `sub_sessions` / `sub_max` through `daemon.set_policy` and read `sub_auto_max`; `store.ts` currently sends only `agents_control_others` (still works, other keys untouched).

## Fix round 1

Tests: `cargo test -p kelpie-proto -p kelpied -j 2` = 29 + 873 passed, 0 failed. `cargo clippy -p kelpied [--tests] -j 2 -- -D warnings` clean. Also touched: `chat.rs`, `chat/queue.rs`, `status.rs` (chat.rs/queue.rs as allowed).

| Item | Done |
|---|---|
| B1 | `deliver_mail` skips any recipient whose kind is not an agent preset; its mail waits for `sub.inbox`. A shell caller may still `sub.spawn` (no caller-kind check). Tests: shell parent gets nothing typed, reads mail from inbox, shell can pass the gates. |
| M1 | `Session::type_mail_when_idle` (also used for nothing else) refuses while `Activity::draft_waiting()` (a prompt-line keystroke after the turn began, same rule as the continue timer) or while the first prompt is pending (m6). Test with a draft typed 31 s ago. Without hooks no turn start is known, so only the 30 s quiet time protects there. |
| M2 + m2 | New `MessageOrigin::Kelpie` + `By::kelpie()`. Written like a phone's line by `write_plain`, desktop text limit (bytes), not per-device counted, not dropped by `drop_phones`, labelled `kind: "kelpie"` in the queue view and `kelpie` in `chat.resolved`; refused with `Refused::Paused` while the user's Stop holds the queue (never unpauses). Test in `chat.rs` (6 x 6000 CJK chars accepted, survives `drop_phones`, paused refusal). |
| M3 | `sub.message` from a session that does not skip prompts to one that does is refused ("... Ask the user to tell it instead"); the user token is never held back. Note: the rule is uniform, so a non-yolo child (possible via `session.create` from a yolo parent) cannot `sub.message` a yolo parent; `sub.report`/auto reports to the parent are unaffected. |
| M4 (owner decision) | See below. |
| m1 | `create_inner` refuses an id that appears as any session's `parent` ("belongs to the history of a sub-session"). Test. |
| m3 | App-side (`mode-switch.ts` creates a new session with a new id), so NOT done in the daemon. Phase 3 needs a user-only way to carry lineage, e.g. `session.create {lineage_from: <old id>}` honoured for User/App only (copy parent/depth/report) AND re-pointing the old session's children to the new id; otherwise the parent loses the child on a terminal/chat switch. |
| m4 | `discard_worktree` (git worktree remove + branch -D) when the session fails to start after its worktree was made; the error says if something was left. Test with a real temp repo. |
| m5 | Wait-for-user and exit always replace the row; "explicit wins" only holds against the automatic idle report. Test updated. |
| m6 | See M1. |
| m7 | `parse_spawn`/`parse_create` take the known/installed lists, and `child_params` is a pure builder of the child's `create` parameters, so lineage depth, args (yolo/model), cwd, prompt and project are tested without a process. The real ask test still needs an installed agent (deny path only). |
| m8 | Accepted and documented on `is_descendant` (chain breaks when a middle session is closed; the user closes what is below). |
| n1 | `\?\UNC\server\share` becomes `\server\share`. n2: `branch_slug` (no `sub-run-xxxx` for names without usable letters) and 3 tries on a branch-name collision. n3: user mail label is "a program run by the user". n4: no deferred reply for a call without id. n5: kept as documentation. n6: an unreadable `policy.json` is copied to `policy.json.unreadable` before being replaced, and policy writes are serialized (`Subs::policy_file`). n7: `sub.inbox` without `take` reports `dropped`; `sub.report` mails the parent first so a rate-limited report changes nothing. Tests for each. |

### M4 contract change (docs, phase 4)
`session.create` called by a program in a session (Role::Session) is now a sub-session of the caller, through the same gate function as `sub.spawn` (`reserve_slot`: policy off refuses, caps depth/per-parent/global/L3; ask mode prompts the user and replies later; `allow` and "allow for this session" pass). `parent`/`depth` are set by the daemon; `project_id` is the caller's. The reply keeps the old wire shape (the session info; the deferred reply has `env` removed as `filter_reply` does).
- Allowed kinds: installed agent presets only. `shell` (the default kind when `kind` is omitted) and `custom` are refused from a session: a shell can start any program with any permission, so D6 cannot hold for it. A script that needs a shell session must be run by the user (user token) or use `session.input` on an existing shell.
- Refused when set by a session: `id`, `env`, `chat_argv`, `remote`, `agent_session_id`. `restricted`, `cols`, `rows`, `name`, `prompt`, `args` pass.
- D6: permission-skipping flags are removed from `args` (the preset's yolo flags plus the other spellings, e.g. Claude `--permission-mode bypassPermissions`, Codex `--full-auto`/`-s danger-full-access`/`-a never`, Gemini `-y`/`--approval-mode yolo`, also `--flag=value`) unless the caller itself runs that way.
- User/App callers (and the run engine, automations) are unchanged and still call `create` directly. `run.create` keeps its own gates.
- Existing e2e scripts that create shells from a session token (not found in `daemon_auth.py`, which only checks refusal under policy off) must use the user token.
- The gate is `Daemon::reserve_slot` + `session_create_from_session`; `parse_create`/`child_params` hold the rules.

Status: DONE_WITH_CONCERNS

## Fix round 2

Tests: `cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations` = 29 + 828 passed, 0 failed. The 54 `notify::destinations` tests fail inside an agent session (an extra request reaches the fake servers, `left: 3 right: 2`), the known environment issue the reviewer noted; this change does not touch them. `cargo clippy -p kelpied [--tests] -j 2 -- -D warnings` clean. Files touched this round besides the sub-session ones: `agents.rs` (omp preset), `runs.rs`, `status.rs`, `chat.rs`, `chat/queue.rs`.

| Item | Done |
|---|---|
| R2-M1 allow-list | `subs::only_model`: a program in a session passes, in `args` of `session.create` and of a task, only `--model <name>` / `--model=<name>` / `-m <name>` (validated by `model_args`: plain name, <= 100 chars, no leading `-`, no space or control character; agent must have a model flag). Anything else is refused ("may pass only a model ..."). The child's arguments are now always `child_args(preset yolo if the caller skips prompts, model)`; the deny-list `strip_permission_skips` is gone. Daemon-injected flags (hooks settings etc.) are unchanged. |
| R2-M3 one classifier | `subs::args_skip_permissions(kind, yolo, args)`: preset yolo sequence plus `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--permission-mode bypassPermissions\|bypass`, `--allowedTools` (any value), `--settings` holding bypassPermissions, `--full-auto`, `-a/--ask-for-approval never`, `-s/--sandbox danger-full-access`, `-c approval_policy=never`, `-c sandbox_mode=danger-full-access`, `--yolo`, `-y` (gemini, qwen), `--approval-mode yolo`, `--auto-approve`, `--auto high`, `--yes-always`, `--trust-all-tools`, `--allow-all-tools`, `--skip-permissions-unsafe`, `-f/--force` (cursor), all with `=` forms. `skips_permissions(entry)` in `sub_rpc.rs` adds a chat's runtime mode: `ChatSession::skips_permissions()` follows what the daemon saw written (Claude `set_permission_mode` bypassPermissions, Codex `turn/start` `approvalPolicy: never`, ACP `session/set_mode` bypass/yolo). Used by D6 (child inheritance, runs, tasks) and by the `sub.message` rule. Best effort and documented as such in the code; it cannot see a chat's launch argv beyond the `args` the app passed. omp preset now has `yolo: ["--auto-approve"]` (`agents.rs`; it also shows as omp's Yolo flag in `agent.presets`). |
| R2-M4 | `Queue::mail_hold`, set by any applied Stop (also with an empty queue), cleared by the next non-mail message, a turn line someone else writes, or `resume`. Mail meanwhile gets `Refused::Paused`. Test. |
| r1 | The draft guard now uses the last prompt-line edit vs the last Enter and the last turn start (`Activity::last_edit/last_enter`): an Enter clears a draft with or without hooks; cursor/escape keys are not edits; a bracketed paste is an edit and not an Enter. |
| r2 | At most one `Origin::Kelpie` entry per chat queue (`QueueFull` otherwise). |
| r3 | `Request` (approval card) carries `model` and `skips_prompts` (the child will run without asking because its opener does). |
| r4 | Contract change noted for phase 4 docs (below). r5: carried, phase 3. |
| nits | Branch retry checks `git show-ref` for the branch instead of git's English text; `discard_worktree` takes the repo's worktree lock. |

### R2-M2: runs made by a program in a session
- `run.create` from a Session role (`dispatch` knows the role): sub-sessions off -> refused; ask -> every task gets `approve: true` (user approves on the Runs page; skipped after "allow for this session"); allow -> tasks as asked. cwd must exist (relative to the caller), `project_id` is the caller's, `materialize` dropped, `host` and `env` refused, `kind` must be an installed agent (shell/custom refused), `args` only a model. `Run.creator` (serde-default, skipped when empty) is set by the daemon only. User-created runs: unchanged (tested: shell task, no creator).
- Start: `Runs::step_gated` asks `Daemon::run_gate(creator)` before a task of such a run starts. Limits full (per-parent/global/L3) -> `Gate::Wait`: the task stays ready and is asked again every tick (waiting is practical here; it does not fail). Sub-sessions turned off after creation, the creating session closed, or its depth already at the limit -> `Gate::Fail`: the task fails with the reason (waiting would never end). `start_task` re-checks with a held slot (`task_lineage`); if the limits filled in between, the task goes back to Pending.
- Task sessions are created with `parent` = the creating session and `depth + 1`; args = the model plus the agent's own skip-prompts flags only when the creator skips prompts.
- `Runs::step` kept as a test-only wrapper of `step_gated`.

### Contract changes for docs (phase 4), all of round 1 and 2
- `session.create`, `sub.spawn` and `run.create` from a program in a session are one door: sub-session policy, caps, D6; agents only; arguments limited to a model; `run.create` tasks wait for approval in ask mode.
- `sub.message` to a session that runs without asking is refused unless the sender does too (user token always may).
- `sub.request` carries `model` and `skips_prompts`.

Status: DONE_WITH_CONCERNS

## Fix round 3

Tests: `cargo test -p kelpie-proto -p kelpied -j 2 -- --skip notify::destinations` = 29 + 836 passed, 0 failed (the 54 `notify::destinations` tests still fail inside an agent session, the known environment issue). `cargo clippy -p kelpied [--tests] -j 2 -- -D warnings` clean.

| Item | Done |
|---|---|
| R3-M1 | Two checks. Strict `subs::args_full_bypass` (preset yolo flags; claude `--dangerously-skip-permissions` / `--permission-mode bypassPermissions`; codex `--dangerously-bypass-approvals-and-sandbox` / `--yolo`; gemini `--yolo` / `--approval-mode yolo`; omp `--auto-approve` / `--approval-mode yolo`; chat runtime bypass) decides what a child inherits: `bypasses_permissions(entry)` is used by `child_params`, `task_lineage` and the approval card's `skips_prompts`. The broad `args_skip_permissions` / `skips_permissions` stays only for the `sub.message` target rule. Chats keep two runtime flags (`perm_skip`, `perm_bypass`): Claude bypass mode and ACP bypass/yolo set both; Codex `approvalPolicy: never` sets only the broad one (both when `sandboxPolicy.type` is `dangerFullAccess`). Tests: the reviewer's examples (`--allowedTools Read`, `--full-auto`, `-a never`, `--allow *`, `--auto high`, `-y`) are broad and not strict; a parent with them gives children no yolo flags; real bypass parents give the child agent's own flags. |
| R3-M2 | Caps count busy children only: alive, not finished. Finished = its run task is done/failed/skipped/cancelled (`Runs::finished_sessions`, handed to the gate by `step_gated` so no lock is retaken) or it last reported itself done/failed (explicit report). A waiting task gets `Task.note` ("waiting for a free sub-session slot (2 of 2 busy)", or the per-session / heavy-load variant), cleared when it starts or stops being pending; the first time a task starts waiting `Action::Waiting` makes the daemon mail the creating session once (`run "R"` ... `task "x": waiting ...; it starts by itself when a sub-session finishes`), a changed reason updates the note without another mail. Tests: a -> b -> c with cap 2 (a, b done and idle) starts c; two busy children make c wait with the reason, one `Action::Waiting`, one mail, no repeat at later ticks; a place freeing starts it and clears the note. |
| R3-M3 | `subs::launch_permissions` classifies the chat process's command line (`chat_argv` after Claude's own mode-restoring flags are added) when the chat is created, and sets both flags (`ChatSession::set_launch_permissions`). Every chat spawn goes through `create_inner`, so a chat started again in bypass is covered. A chat restored from `sessions.json` without a process has no flags until the app creates it again; the command line is not persisted. Test with `--permission-mode bypassPermissions` and a sandboxed Codex line. |
| r1 | Alt+Enter (`ESC CR`) is an edit and not a submit; Enter right after a backslash (same write or the next) does not submit; cursor keys edit nothing. Backspace / Ctrl+U still count as edits (an erased draft holds mail until the next Enter; this errs towards waiting) - noted in a code comment. Tests. |
| r3 | `run.create` from a session at the deepest level is refused at creation; the user (not a session) retrying a task of a run whose creating session is gone clears `creator` (the run is theirs, no parent). Tests. |
| nit | Mail already queued in a stopped chat is moved behind the user's next message (`Queue::mail_to_back`). Test. |
| r2 | omp `--auto-approve` kept as decided (for the phase-4 docs: users with Permissions: Yolo now launch omp with it, and omp children inherit it from a bypass parent). |

Status: DONE_WITH_CONCERNS
