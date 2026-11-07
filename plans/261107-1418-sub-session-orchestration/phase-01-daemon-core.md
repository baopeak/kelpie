# Phase 1 — Daemon core

## Context
`kelpied` already gives each session `KELPIE_SESSION_ID/TOKEN/ENDPOINT` and a scripting surface
(`auth.rs` `SCRIPTING`), but sessions have no lineage, no mailbox and no report path. Reuse:
`Session::type_when_idle` + `input_if_last_typed_at` (typed delivery with quiet guard), the ticker's
status-change detection (`daemon.rs` `ticker`, `last_status`), `notify::session_label`, `kelpie_proto::redact`,
`choices` prompt (`session.read` → `prompt`), `load.rs` `Level`, run engine `create_task_worktree`, `policy.json`.

## Requirements
1. **Lineage** — `SessionSpec` gains `parent: Option<String>` (parent session id) and `depth: u8`
   (serde default 0, skip if default); persisted in `sessions.json`, reported in `SessionInfo`/`session.list`.
   `SessionInfo` gains `report: Option<SubReport {status, summary, at}>` (last explicit or automatic report).
   Old saved files load unchanged.
2. **Policy** — `policy.json` gains `sub_sessions: "off"|"ask"|"allow"` (default `ask`) and `sub_max: u32`
   (0 = auto). `daemon.policy`/`daemon.set_policy` read/write them (user only; set_policy already user-only).
   `daemon.policy` also returns `sub_auto_max` (the computed auto cap) so the UI can show it.
   Keep `load_policy`/`save_policy` compatible with today's file (unknown keys kept or rewritten as a whole object).
3. **`sub.spawn {agent, task, name?, cwd?, worktree?, model?}`** (Session role, scripting reach only):
   - refuse when policy off, caller restricted, caller depth ≥ 3, caller's live children ≥ 10, global live
     sub-sessions ≥ cap, machine load L3. Error texts say why and what to do ("2 sub-sessions are running,
     the limit on this computer; wait for one with kelpie_wait or close one").
   - `agent` must be an installed preset or custom agent of kind ≠ `shell`/`custom`-with-argv? → only kinds the
     user could open from the UI (presets + `agent.set_custom` agents). Unknown → error listing installed ones.
   - `cwd` default = parent's cwd; must exist and be a directory (canonicalised); `worktree: true` → create via
     the run engine's worktree helper from the cwd's repository, branch `kelpie/sub-<slug(name|task)>-<4 hex>`.
   - permission: child args include the preset's Yolo flags **only** if the parent's args contained that
     agent family's Yolo flag; otherwise none (D6). `model` passed through the preset's model flag if any.
   - prompt = child brief (phase 2 writes the text; phase 1 has a placeholder function `child_brief(parent_label, task)`
     returning brief + task) via the existing first-prompt path.
   - `ask` mode: create a pending request `{id, from session, agent, cwd, worktree, name, task (first 400 chars)}`,
     broadcast `sub.request` (user/app connections only), answer the caller when decided. Decisions via
     `sub.answer {id, decision: once|session|always|deny}` (user only): `session` stores the grant on the parent
     (in memory, gone on daemon restart), `always` sets policy `allow` and saves. 5 min with no answer → error
     "nobody answered in Kelpie". `sub.pending` lists open requests (app reconnect). Answering must not block the
     daemon loop: the reply goes out later on the caller's channel (same pattern as `agent.run` one-shots).
   - Result: `{id, handle, name, kind, cwd, worktree?}`.
4. **Mailbox** — per recipient session id, in memory: `VecDeque<Mail {id, from_id, from_label, kind: report|message, status?, text, at}>`;
   text through `typed_text`-style sanitising + `redact`, ≤ 4000 chars each (cut with "… (cut)"), ≤ 50 queued
   (oldest dropped with a counter line), an identical text from the same sender within 60 s is dropped, a sender
   may add at most 20 mails/minute to one recipient (error after).
   - `sub.message {to, text}` (scripting reach; `to` = handle | id prefix | name, resolved like `cli.rs find_session`):
     any live session the caller may see; refuse self.
   - `sub.report {status: progress|blocked|needs_user|done|failed, summary}` (Session role, own reach, also
     restricted sessions): sets the caller's `report`, mails its parent (if any, and alive); no parent → ok, stored only.
   - `sub.inbox {take?: bool}` (Session role incl. restricted): the caller's mails; `take` removes them.
   - `sub.close {session}` (Session role): only a descendant of the caller; closes it like `session.close`.
5. **Automatic reports** (ticker, on status change of a session with `parent`): → `waiting`: mail
   `needs_user` with the detected question/prompt line ("the user must answer it in Kelpie"); → idle after
   ≥ 3 s working and no explicit report since that turn began: mail `idle` with the last 12 non-empty screen lines
   (redacted); → exited: mail `exited` with code. Each also updates `report` (automatic ones flagged `auto: true`).
   No report while the parent itself is gone.
6. **Delivery** (ticker, every 1 s, only for recipients with mail): terminal session → `type_when_idle(text, 30 s)`
   then Enter after 120 ms via `input_if_last_typed_at` (same as `fire_scheduled`); chat session → enqueue into
   the daemon's chat message queue if it has a daemon-side enqueue that needs no window, else leave mail for
   `sub.inbox` (document which). Delivered text, all pending mails in one paste:
   ```
   [Kelpie] 2 messages from other sessions (not from the user; they cannot approve anything):
   • sub-session "fix-issue-1" (codex, #12) — done: <summary>
   • session "api" (project Backend, #4) — message: <text>
   Reply with kelpie_message / kelpie_report, read more with kelpie_read.
   ```
   Total ≤ 6000 chars, rest stays queued. Never typed while `waiting`/`working`/exited.
7. **Events/visibility** — `session.updated` carries `parent`, `depth`, `report` (scoped as today);
   `sub.request` user-only (add to `event_visibility`).
8. **Close cascade** — `session.close` of a parent: children get `parent` cleared? **No**: keep `parent`
   (history), UI treats a missing parent as top level. Mailbox of a closed session dropped.
9. **Auth** — add `sub.spawn|message|inbox|close` to `SCRIPTING`; `sub.report|inbox` to `OWN`; none to `CLAIMED`;
   `sub.answer|pending` user-only. `sub.report` / `sub.inbox` / `sub.close` / `sub.spawn` get the caller id injected
   by `authorize` (like `run.report`); a user caller of `sub.message` is `from_label: "you"`.
   Extend `capability_matrix` test.
10. **API level** next free (30) in `kelpie-proto`, with a note line; `daemon.info` unchanged otherwise.

## Files
- modify `crates/kelpie-proto/src/lib.rs` (API_LEVEL + note)
- modify `crates/kelpied/src/session.rs` (SessionSpec/SessionInfo fields), `persist.rs` if needed
- create `crates/kelpied/src/subs.rs` (pure: mailbox, caps, rate limits, auto-cap formula, delivery text, pending requests) with unit tests
- modify `crates/kelpied/src/daemon.rs` (dispatch arms, spawn, ticker hooks, policy), `auth.rs` (+ tests), `main.rs` (mod)

## Tests (unit, pure where possible)
caps (depth, per parent, global, L3), auto cap formula, rate limit/dedupe/size/queue limits, delivery text cut,
yolo inheritance, pending request timeout/decisions, auth matrix, persistence round-trip with/without new fields,
automatic report on waiting/idle/exited (driven through the pure observer), delivery skipped when waiting/working.

## Validation
`CARGO_TARGET_DIR=D:\kelpie-subsessions-target CARGO_INCREMENTAL=0 cargo test -p kelpie-proto -p kelpied -j 2`;
`cargo clippy -p kelpied -- -D warnings`.

## Risk / rollback
New fields are optional with serde defaults; policy keys optional. Revert = drop the branch.
