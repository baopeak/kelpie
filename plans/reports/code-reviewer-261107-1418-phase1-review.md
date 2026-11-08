# Phase 1 (daemon core) review: sub-sessions

Worktree `D:\kelpie-subsessions`, branch `feat/sub-sessions`. The changes are uncommitted. This review is read-only.

## Scope
- Files: `crates/kelpied/src/{subs.rs, daemon/sub_rpc.rs, daemon/sub_rpc/tests.rs}` (new), `auth.rs`, `daemon.rs`, `session.rs`, `persist.rs`, `main.rs`, `kelpie-proto/src/lib.rs`
- About 1,935 new lines plus about 160 changed lines.
- Also read: `chat.rs` (enqueue, pump, stop), `chat/queue.rs`, `status.rs`, `session.rs` (type_when_idle, write_input, tick), `agents.rs` (presets, launch, Windows resolve), `server.rs` (worker and deferred replies), `runs::slug`

## Checks run
- `cargo test -p kelpied -j 2 -- sub auth persist`: 74 passed, 0 failed.
- The full `cargo test -p kelpied -j 2 -- --skip notify::destinations` run gave 807 passed and 1 failed. The failure was `mobile::tests::one_address_holds_its_share_and_silence_counts_against_it`, which timed out under load. It passes when run alone in 10.6 s.
- The `notify::destinations` tests fail when run inside an agent session. This is a known issue unrelated to this change: the installed kelpie.exe sends page-detection GETs to the fake servers.
- `cargo clippy -p kelpied -j 2 -- -D warnings`: clean.

## Overall
The pure core (`subs.rs`) is well bounded and well tested. Auth injection of the caller id is correct for every `sub.*` method a session can call. No locks are held across one another, so there are no deadlocks. The problems are at the delivery edge, where typing into another session's input meets real terminals and the chat queue. There are also two design gaps where the new gates can be bypassed through older surfaces.

| Severity | Count |
|---|---|
| blocker | 1 |
| major | 4 |
| minor | 8 |
| nit | 7 |

---

## Blocker

### B1. Mail is typed into a shell parent and runs as commands
- **Where:** `sub_rpc.rs:369-412` (`deliver_mail`, no check on the recipient's kind) and `sub_rpc.rs:123-139` (`parse_spawn`, no check on the caller's kind).
- **Evidence:**
  - Every PTY session gets `KELPIE_SESSION_ID/TOKEN` (`session.rs:510-511`), including `shell` sessions. `sub.spawn` is in `SCRIPTING`.
  - So a script in a plain shell session can call `sub.spawn`. So can an agent the user started by hand inside a shell tab, because it inherits the shell's id.
  - A shell with no output counts as `Idle` (`status.rs:164-172`).
  - The child's reports then go to the shell parent. These are automatic idle reports (the last 12 lines of the child's screen) and explicit `sub.report` summaries. `deliver_mail` types them with `type_when_idle` and sends Enter 120 ms later.
  - `sub.message` refuses non-agent targets (`sub_rpc.rs:264`). Reports to a parent do not.
- **Failure scenario:**
  - A user runs `claude` in a shell tab, Claude spawns a Codex child, and the user exits Claude.
  - The child's next report is pasted into bash or zsh at the prompt. The header line gives a syntax error.
  - Each indented continuation line (`mail_line` indents with two spaces) runs as its own command. Those lines are child-controlled text: its summary, or whatever was on its screen, which may come from untrusted repository content.
  - Result: a non-yolo child gets arbitrary command execution in the parent's shell with no approval.
  - Not run live. Whether PowerShell executes the block depends on parsing. Bash and zsh read a multi-line paste line by line.
- **Fix (both parts):**
  1. In `deliver_mail`, skip recipients where `agents::preset_for(&spec.kind)` is `None`. Leave their mail for `sub.inbox`.
  2. In `parse_spawn`, refuse callers whose kind is not an agent preset: "sub-sessions are opened from an agent session".
  - Add a test: a shell parent with mail queued gets nothing typed.

## Major

### M1. A user's unsent draft is submitted together with the mail
- **Where:** `sub_rpc.rs:382` uses `type_when_idle(text, QUIET)`. That checks only `last_typed` against 30 s (`session.rs:910-920`).
- **Failure scenario:**
  - The user types half an instruction into the parent's Claude or Codex prompt and walks away for more than 30 s.
  - A child report is pasted after the draft, and Enter submits both as one message.
  - The agent receives the user's unfinished instruction joined to text whose header says "not from the user". The user's intent is corrupted, and the merged message is ambiguous about who said what.
- **Comparison:** the continue timer avoids this with `last_draft` (`status.rs:127-129`, `limit_untouched`: no draft keystroke since the turn began).
- **Fix:** deliver only when no prompt-line keystroke has been typed since the last submitted turn. One way: track the last Enter, or use `last_draft <= turn_started` as `limit_untouched` does. Otherwise leave the mail queued. Add a test with a draft typed 31 s ago.

### M2. Chat delivery cancels the user's Stop and flushes their paused queue
- **Where:**
  - `sub_rpc.rs:398-406` delivers through `ChatSession::enqueue`.
  - `chat.rs:1127` (`enqueue_if`) sets `g.q.paused = false` unconditionally ("A new message ends the wait a stop asked for").
  - `chat.rs:1370`: Stop pauses a non-empty queue ("Stopping means wait").
- **Failure scenario:**
  - The user presses Stop on a chat parent that has queued messages, so the queue pauses and the agent goes idle.
  - A child's report arrives, and `deliver_mail` enqueues it while the chat is `Idle`. That unpauses the queue, and `pump` writes the user's held messages followed by the mail.
  - Mail, not the user, restarts the agent and sends what the user had deliberately held.
- **Fix:** for a chat recipient, skip delivery while the queue is paused or held. This needs a read accessor, or an `enqueue_if` variant that refuses instead of unpausing for daemon mail. It ties in with the dedicated origin in m3.

### M3. `sub.message` lets any scripting session start turns in a yolo session (bypasses D6)
- **Where:** `sub_rpc.rs:260-277`. Any live agent session is a valid target, and the sender's and target's permission levels are not compared.
- **Failure scenario:**
  - A non-yolo session is prompt-injected. In phase 2 it calls `kelpie_message`, which Claude pre-allows by D4, so there is no prompt.
  - The target is an unrelated Claude session started with `--dangerously-skip-permissions`. The mail is typed and submitted, and the yolo agent acts on it with no approval anywhere.
  - D6 ("a child never gets more than its parent") holds for `sub.spawn` but not for messaging.
  - Before this change, `session.input` had the same reach, but from Claude it needed a Bash tool approval. The pre-allowed MCP tool removes that gate.
- **Fix (needs a decision from the lead or user):**
  - Option 1: refuse when the target has its preset's yolo flags (`subs::args_have_yolo`) and the sender does not.
  - Option 2: limit messages to the sender's own lineage and project, and ask the user for anything else.
  - Option 3: do not pre-allow `kelpie_message` in phase 2.

### M4. `session.create` from a session gets around every sub-session gate (no forgery, but a full bypass)
- **Where:** `daemon.rs:2022` (`session.create` → `create_inner(params, None)`) and `daemon.rs:162-193` (`CreateParams`).
- **What was verified:**
  - `parent`, `depth` and `report` cannot be forged. They are not in `CreateParams`, serde drops unknown keys, and lineage comes only from `spawn_child`.
  - However, `session.create` from a scripting session ignores `sub_sessions: off`, the ask prompt, the depth cap, the 10-children cap, the global cap, L3 load and D6. The caller can pass any `args`, yolo flags included, and the new session is depth 0.
- **Failure scenario:**
  - A depth-3 child is refused with "do the work in this session". It falls back to `kelpied call session.create {...}` and gets an unlinked, uncapped session.
  - On the 8 GB machine (auto cap 2) this is exactly the runaway the caps exist to stop.
  - The "off" setting therefore does not stop a determined agent while `agents_control_others` is on.
- **Fix (needs a decision; behaviour visible to users):** for `Role::Session` callers, treat `session.create` as a sub-session.
  - It records `parent`/`depth` and goes through `reserve_slot`. Under `off` it is refused.
  - Yolo args are stripped unless the caller has them.
  - At minimum, document in `docs/architecture.md` that `sub_sessions` is not a boundary while scripting is on.

## Minor

### m1. Reusing a closed parent's id adopts its children (affects `sub.close` and reports)
- **Where:** `CreateParams.id` (`daemon.rs:164`) is accepted from sessions, and `is_descendant` (`sub_rpc.rs:319-329`) matches on the id string.
- **Scenario:**
  - P spawns C, then P is closed.
  - Scripting session X calls `session.create {id: "P"}`, then uses `session.input` to run `kelpied call sub.close {session: C}` inside it.
  - The new "P" closes C and receives C's reports, including C's screen tails.
- **Impact:** X's reach already covers C through `session.input` and `session.read`, so this is an invariant violation rather than a new capability.
- **Fix:** refuse a session-supplied `id` that appears as any live session's `parent`, or record a parent instance nonce such as `created_at` alongside the id.

### m2. Chat mail reuses `MessageOrigin::Phone`; a dedicated origin is recommended
- **Where:** `sub_rpc.rs:404`.
- **Verified consequences:**
  1. **Text limit:** `MAX_PHONE_TEXT` is 16 KiB, counted in bytes (`queue.rs:19, 245`), but `MAX_DELIVERY` is 6000 characters. A CJK or emoji-heavy delivery (6000 × 3 bytes) is refused `TooLong` every second, and `plan_delivery` keeps choosing the same head. Delivery stalls until queue overflow drops the head.
  2. **Per-device cap:** `MAX_PER_DEVICE = 5` per `by.id` applies to `"kelpie-sub"` (`queue.rs:248`), so it is another possible stall.
  3. **Phone access off:** `drop_phones` (`daemon.rs:1777, 1881, 1951`) drops mail that is already queued. That mail was already committed out of the mailbox, so it is lost. The window is small but real: an Idle chat whose `conv` is not ready, or whose queue is held, keeps the entry queued.
  4. **Labelling:** the queue view reports it as `kind: "phone"` (`queue.rs:348`). Phase 3 UI and the phone page will show it as a phone message.
- **Fix:** add `Origin::Daemon` (or `Mail`) that is written by `write_plain` like Phone but has the desktop text limit, is not counted per device, is not dropped by `drop_phones` and is labelled `"kelpie"`. Until then, budget `MAX_DELIVERY` in bytes for chat recipients.

### m3. The user switching a sub-session between terminal and chat loses its lineage
- **Where:** `app/src/chat/mode-switch.ts:193, 249, 273` creates a new session with a new id.
- **Effect:** `parent`, `depth` and `report` are gone. The parent stops getting reports and cannot `sub.close` the child, and closing the old id cancels its pending requests.
- **Fix:** before phase 3, add a user-only way to carry lineage. For example, `session.create {lineage_from: <old id>}`, honoured only for `Role::User`/`App`, or `session.switch_mode` done in the daemon.

### m4. A worktree is left behind when the spawn fails after creating it
- **Where:** `sub_rpc.rs:200-212`.
- **Effect:** if `create_inner` fails after `create_worktree` succeeds (missing binary, PTY error), the branch `kelpie/sub-…` and its folder remain, and the error does not mention them.
- **Fix:** on error, run `git worktree remove` and `git branch -D`, or put the path in the error text.

### m5. The child's row keeps a stale explicit report through a later wait or exit
- **Where:** `sub_rpc.rs:354` combined with `ChildWatch::explicit` (`subs.rs:636`). The flag resets only on a new Working.
- **Effect:** a child that reported `progress` in its last turn and then hits a permission prompt, or exits or crashes hours later, still shows `progress` on its row. The parent's mail is correct.
- **Fix:** let `NeedsUser` and `Exited` always overwrite the row. Keep the "explicit wins" rule only for `Idle`.

### m6. Mail can race a typed child's first prompt
- **Where:** `deliver_mail` does not check `pending_prompt`.
- **Effect:** for typed-prompt presets (auggie, kiro, cline and other `PromptMode::Typed` agents), a parent's `sub.message` sent right after spawn finds the child `Idle` with no `last_typed`. It is typed before or alongside the brief (`session.rs:1198-1230`).
- **Status:** uncertain on the exact interleaving.
- **Fix:** skip delivery while `pending_prompt.is_some()`.

### m7. The only ask-mode end-to-end test passes vacuously in CI
- **Where:** `sub_rpc/tests.rs:266`. It returns early when `installed_presets()` is empty, which is the case on CI runners.
- **Coverage gap:** `spawn_child` and the ask-to-allow path that ends in a spawn have no test anywhere.
- **Fix:** inject the installed list or the launcher, for example `sub_spawn_with(installed: &[&str])`. Alternatively, put the post-decision part of `spawn_child` behind a seam that returns the `create_inner` params, so the test can assert args (yolo and model), cwd, prompt and lineage without starting a process.

### m8. A grandparent cannot close a grandchild once the middle session is closed
- **Where:** `is_descendant` (`sub_rpc.rs:326`) follows only live sessions.
- **Effect:** the chain breaks, and the grandchild becomes closable by nobody except the user. This contradicts D8's "its own descendants".
- **Fix:** accept it and document it, or keep a closed-session parent map (id → parent) in memory until restart.

## Nits
- **n1.** `existing_folder` (`sub_rpc.rs:32-42`) keeps the `\\?\UNC\…` prefix. On Windows, canonicalising also turns mapped drives and `subst` drives into UNC or real paths, so a child's cwd differs from the parent's text. Strip `\\?\UNC\` to `\\`, or keep the given path once `is_dir()` holds.
- **n2.** `runs::slug` makes non-ASCII names (Vietnamese) unreadable ("Sửa lỗi" becomes `s-a-l-i`), and an empty result becomes `run`, giving `sub-run-xxxx`. Four hex digits give no retry on a branch collision, which surfaces as git's raw error.
- **n3.** A user's mail is labelled `you` (`sub_rpc.rs:272`). From the recipient agent's point of view "you" means the agent itself, and it sits under a header that says "not from the user". Use "a program run by the user".
- **n4.** A deferred reply for an ask with a null request id is still sent (`sub_rpc.rs:180`). `server.rs:111` suppresses replies to notifications. Skip the send when `id.is_null()`.
- **n5.** `auth.rs:342`: the explicit `"sub.request" | "sub.request_done" => Hidden` arm is redundant with `_ => Hidden`. Keep it as documentation or drop it.
- **n6.** `save_policy` overwrites an unparseable `policy.json` with in-memory values that were themselves defaults, so `agents_control_others` comes back as true. This behaviour predates the change, but the rewrite now explicitly reads and discards the file. AGENTS.md says "A failed state read is not permission to overwrite". Also, a concurrent `sub.answer always` and `daemon.set_policy` can write an older snapshot last.
- **n7.** `sub.inbox` without `take` always returns `dropped: 0`, even when some mail was dropped. `sub.report` updates the row and then can fail on the rate limit, so the caller sees an error although the row changed.

## Questions answered from the brief
- **Is the caller id injected for every `sub.*` method?** Yes, for all five methods a session can call (`auth.rs:185-193`). A forged `session_id` is overwritten, which the test at `sub_rpc/tests.rs:88` confirms. `object_params` runs first, so Null or array params cannot skip the injection. Claimed sessions get no `sub.*` methods, and `sub.answer` and `sub.pending` are user and app only.
- **Can a restricted session spawn?** No. It is blocked by auth (`OWN` has only `sub.report` and `sub.inbox`) and again by `check_caps`.
- **Can a session message itself into a loop?** It is refused for itself. Two sessions can ping-pong, limited by the 30 s quiet window per recipient and 20 mails per minute per pair. Each exchange still costs a model turn, which the user may want flagged in phase 4 docs.
- **Who sees `sub.request` events?** Hidden for Session, Claimed and Anonymous (tested).
- **Is a child given more permission than its parent?** Not through `sub.spawn`. `args_have_yolo` is conservative: aliases and `=` forms count as not yolo, and a chat parent switched to bypass with the mode chip yields a non-yolo child. Through M3 and M4, yes.
- **Is typed text safe?** Control characters are stripped (`typed_text`, `clean_text` plus `redact`), quotes in labels are neutralised, and continuation lines are indented. Mail is never typed while waiting, working or exited (`deliver_mail` checks status, `type_when_idle` checks again under the lock). Exceptions: B1 (shells), M1 (drafts), M2 (chat pause).
- **Deadlocks:** none found.
  - `reserve` holds `slots`, then the sessions read lock, then each session's lock. Nothing takes them in the reverse order.
  - `close`, `deliver_mail`, `sub_status_changed`, `sub_answer` and `wait_for_answer` take one lock at a time.
  - Broadcasts happen with no subs lock held.
- **Ask mode:** the reply goes out on a `kelpie-sub-ask` thread, with at most 8 waiting at once. The timeout is settled under the pending lock, so a late answer cannot be lost or applied twice. The caps are checked again after the decision. A caller that hung up gets no spawn.
- **Ticker cost with no mail:** one mutex lock and an empty `Vec` per second. With mail held for a busy recipient: one `by_id` plus `info()` per second each. Fine.
- **Persistence:** old `sessions.json` and old `policy.json` load unchanged (tested). Saving always keeps `agents_control_others`, and unknown keys are kept. A downgrade followed by an upgrade resets `sub_sessions` to `ask` (the safe default).
- **`report` on `SessionSpec` (the deviation):** acceptable. The wire shape is identical because `SessionInfo` flattens the spec. Persisting it is a benefit: the row survives a restart. Cost: every report marks `meta_dirty` and triggers a `sessions.json` save, which is cheap. It also disappears on a mode switch (m3).
- **API_LEVEL 30:** this follows D10. Per the API-level-in-commit-order rule, re-check at merge time. If this lands before the phone branches (26–29), either they renumber above 30, or this takes the next free level and every gate moves with it.

## Recommended order
1. B1 (kind checks for both recipient and caller, plus a test).
2. M1 (draft guard).
3. M2 together with m2 (dedicated chat origin that does not unpause).
4. Lead or user decision on M3 and M4, before phase 2 pre-allows the tools.
5. m7 (a test seam for `spawn_child`), then m4, m5, m6, m3 (the last before phase 3).

## Unresolved questions
- M3 and M4 change what users experience, so the owner should choose.
- Should a child inherit the parent's `env`, such as `CLAUDE_CONFIG_DIR` for another account? Today it does not, so a child of a second-account parent runs on the default account.
- B1 for PowerShell parents was not reproduced live.

Status: DONE_WITH_CONCERNS

---

# Round 2: re-review after "Fix round 1"

## Checks run
- `cargo test -p kelpied -j 2 -- sub auth chat persist`: 192 passed, 0 failed.
- `cargo clippy -p kelpied --tests -j 2 -- -D warnings`: clean.

## Round 1 findings: verified in code
| Finding | State | Evidence |
|---|---|---|
| B1 | Fixed | `deliver_mail` skips recipients that are not agent presets (`sub_rpc.rs:536`). Shell callers may still spawn, and their mail waits in `sub.inbox`. Test at `tests.rs:392`. |
| M1 | Fixed for agents with hooks | `type_mail_when_idle` refuses while `draft_waiting()` is true or a pending prompt waits. For agents without turn-start hooks (Codex in a terminal), only the 30 s quiet time protects a draft (see r1). |
| M2 | Partly fixed | Mail never unpauses the queue and is refused with `Paused` (`chat.rs`, `enqueue_if`). A Stop on an empty queue is still undone (R2-M4). |
| M3 | Rule implemented, detection incomplete | See R2-M3. |
| M4 | Gate implemented; the flag denylist leaks, and `run.create` remains open | See R2-M1 and R2-M2. |
| m1 | Fixed | `create_inner` refuses an id that appears as a live session's `parent`. |
| m2 | Fixed | `Origin::Kelpie`: desktop byte limit, not counted per device, survives `drop_phones`, labelled `kind: "kelpie"`. |
| m3 | Open, deferred to phase 3 | App-side, as noted by the developer. Carried as a minor below. |
| m4 | Fixed | `discard_worktree` runs on a failed start; test against a real repo. |
| m5 | Fixed | `sub_rpc.rs:512`: only the automatic idle report yields to an explicit one. |
| m6 | Fixed | Pending-prompt check in `type_idle`. |
| m7 | Mostly fixed | `child_params` and `parse_create`/`parse_spawn` are now tested without a process. The ask path is still covered only by the deny case. |
| m8 | Accepted | Documented on `is_descendant`. |
| n1 to n7 | Fixed | Read in code. n5 kept on purpose. |

## New and remaining findings

### Major

**R2-M1. Stripping permission flags from `session.create` uses a denylist, and it leaks**
- **Where:** `subs.rs:201-230` (`strip_permission_skips`) and `sub_rpc.rs:100`.
- **Leaks confirmed against the installed CLIs' `--help`:**
  - omp: `--auto-approve` and `--approval-mode=yolo`. The omp preset has an empty `yolo` list and no table entry, so nothing is stripped. A careful session can open `session.create {kind:"omp", args:["--auto-approve"]}`, which is a child that never asks.
  - claude: `--allowedTools Bash` / `--allowed-tools` (unrestricted shell with no prompt), `--settings '{"permissions":{"defaultMode":"bypassPermissions"}}'`, and `--mcp-config <json>` (starts an arbitrary command as an MCP server).
  - codex: `-c approval_policy="never"`, `-c sandbox_mode="danger-full-access"`, `-c mcp_servers.x.command=…`, and `-p/--profile <name>` (any profile the user has).
- **Not installed, so not confirmed:** cursor `--force/-f`, copilot `--allow-all-tools`/`--allow-tool`, droid `--skip-permissions-unsafe`, kiro `--trust-tools`.
- **Why it matters:** per the owner's M4 decision, a careful caller must not open a child that skips prompts. A denylist cannot keep up with every agent's CLI.
- **Fix:** switch to an allowlist. From a careful session, accept only `--model <name>` (validated with `subs::model_args`) and drop or refuse everything else. A caller that does skip prompts keeps its args. Test with omp, Claude `--allowedTools` and Codex `-c`.

**R2-M2. `run.create` still lets a session open sessions without the gate**
- **Where:** `daemon.rs:715-723` (`start_task` calls `self.create` with the task's `kind`, `args` and `env`) and `auth.rs` `scope_session_params` (only `materialize` and `host` are scoped).
- **Failure scenario:**
  - A session calls `run.create {tasks:[{kind:"claude", args:["--dangerously-skip-permissions"], prompt:"…", approve:false}]}`, or uses `kind:"custom"` with any command.
  - Tasks start at once: unlinked, uncapped, with prompts skipped, and without regard to `sub_sessions: off`.
  - The owner's M4 intent (no way past the gate while scripting is on) is not met. The developer kept "run.create keeps its own gates", but `approve:false` is a choice the caller makes for itself.
- **Fix (owner decision):** for `Role::Session` callers, either force `approve: true` on every task (the user approves in the app, an existing gate), or start the tasks through `reserve_slot` and the same allowlist as R2-M1. At minimum, document that the door exists.

**R2-M3. M3 does not detect most targets that skip prompts**
- **Where:** `skips_prompts` (`sub_rpc.rs:76-78`) looks only for the preset's single canonical flag sequence in `spec.args`.
- **Not detected, so mail from a careful session is allowed:**
  - Claude terminal sessions started with `--permission-mode bypassPermissions` or `--allowedTools Bash`.
  - omp started with `--auto-approve` (its preset `yolo` is empty).
  - Codex started with `-a never -s danger-full-access` or `-c approval_policy=never`.
  - Most importantly, a Claude chat switched to bypass with the mode chip. That mode is set at runtime by `set_permission_mode` and never appears in `spec.args` (master 8d153ab/d7cefdc).
- **Fix:** reuse one classifier for both sides (the R2-M1 knowledge, inverted). For chats, read the live permission mode from the chat state; `ChatSession` knows the last mode it ran in. Where it cannot be known, treat the target as skipping prompts.

**R2-M4. A chat Stop with an empty queue is still undone by mail**
- **Where:**
  - `Refused::Paused` only applies while `q.paused`. `stop()` sets `paused` only when the queue has items (`chat.rs:1330, 1370`).
  - `stopped_at` is cleared when the stopped turn ends (`chat.rs:877`, `chat.rs:1759`).
  - The chat delivery path has no quiet time.
- **Failure scenario:** the user presses Stop on a chat parent with nothing queued. About 1 s after the interrupted turn ends, `deliver_mail` sees `Idle`, enqueues, and `pump` writes the mail, so the agent starts again.
- **Fix:** record a user stop (for example `q.last_stop` / a `user_stopped` flag cleared by the next user or phone message), and refuse `Origin::Kelpie` while it is set, or for at least `QUIET` after it. Add a test: Stop on an empty queue, then mail is refused.

### Minor
- **r1. Draft guard liveness and gaps.**
  - `draft_waiting` compares `last_draft` with the last hook-reported turn start.
  - Keystrokes that start no turn (`/clear` and other local slash commands, arrow or PageUp navigation in the prompt) block delivery until the next real prompt. The parent then waits for reports the user never sees.
  - Codex in a terminal reports no turn start, so it has only the 30 s rule.
  - Consider clearing the draft on Enter (submit), not on turn start.
- **r2. `Origin::Kelpie` entries can fill the shared `MAX_ENTRIES = 20`.**
  - Each delivery is committed once queued, even when `pump` cannot write (e.g. a Codex or ACP chat whose conversation is not known yet).
  - Further mail adds entries until the user's own `chat.send` is refused `QueueFull`.
  - Fix: allow at most one `Kelpie` entry queued per chat, and do not enqueue while one waits.
- **r3. The approval card does not show the args of a `session.create` child.**
  - `Request` carries the kind, cwd and prompt preview, but not `user_args`, which a caller that skips prompts passes through unchanged (`--mcp-config`, `-c …`).
  - The user approves without seeing them. Add `args` to `Request`.
- **r4. Contract change not yet in docs or the e2e list.**
  - From a session, `session.create` now refuses `shell`, `custom`, `id`, `env`, `chat_argv`, `remote` and `agent_session_id`, and in ask mode it blocks up to 5 min.
  - `docs/architecture.md:126` ("`session.list|read|input|create`") and `docs/user-guide.md:1265` (`kelpied call session.create …`, which from inside a session now prompts) must change in phase 4.
  - Checked: `bench`/`echo_bench` use `connect_user`, and the e2e scripts create sessions with the user token. `daemon_auth.py:157` still passes, because it is refused by auth under policy off. No in-repo caller breaks.
- **r5. (carried m3)** Mode switch loses lineage; owed in phase 3.

### Nits
- Branch-collision retry matches git's English "already exists" text (`sub_rpc.rs:336`), so a localised git never retries. Check `git show-ref --verify refs/heads/<branch>` first instead.
- `discard_worktree` runs `worktree remove` / `branch -D` without `kelpie_proto::worktree_lock`, unlike `create_worktree`.

## Race check (as asked)
- `reserve_slot` stays atomic: the per-caller slot table lock is held while counting live sessions.
- The only window is between `create_inner` inserting the session and the `Slot` dropping. There a concurrent spawn counts the new child twice (session plus slot) and may be refused spuriously. That is conservative, never an overshoot.
- Pre-checks drop their slot, and the spawn checks again. Ask-mode waits hold no slot.
- No new lock-order inversions: `subs.policy_file`, then `policy.read` only in `persist_policy`.

## Remaining counts (round 2)
| Severity | Count |
|---|---|
| blocker | 0 |
| major | 4 (R2-M1, R2-M2, R2-M3, R2-M4) |
| minor | 5 (r1 to r5) |
| nit | 2 |

R2-M2 and the scope of R2-M3 need owner decisions. R2-M1 and R2-M4 are code fixes.

Status: DONE_WITH_CONCERNS

---

# Round 3: re-review after "Fix round 2"

## Checks run
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 828 passed, 0 failed, 3 ignored.
- `notify::destinations` was skipped. It is the known environment failure inside agent sessions.
- The omp flag was checked against `omp --help`: `--auto-approve  Auto-approve all tool calls (skip approval prompts)` is real. `--approval-mode=…yolo` exists too.

## Round 2 findings: verified in code
| Finding | State | Evidence |
|---|---|---|
| R2-M1 | Fixed | `subs::only_model` accepts nothing, `--model X`, `--model=X` or `-m X`, validated by `model_args`. Everything else is refused. `child_params` now always builds `child_args(preset yolo if the caller skips prompts, model)`, and the denylist is gone. |
| R2-M2 | Fixed as decided, with a liveness regression | Under `off`, creation is refused. Under `ask`, `approve |= ask`. `env` and `host` are refused, kinds are agents only, args are model only. `creator` is set by the daemon only: `NewRun` has no such field, and array tasks are still refused by `scope_session_params`. The run engine gates task starts (`step_gated` → `run_gate`), and `start_task` checks again with a held slot, putting the task back to Pending on `Wait`. See R3-M2 for the regression. |
| R2-M3 | Implemented, but with two new problems | One classifier, `args_skip_permissions`, plus `ChatSession::perm_skip`. See R3-M1 and R3-M3. |
| R2-M4 | Fixed | Every applied Stop sets `q.mail_hold`, even with an empty queue. It is cleared by the next user or phone message, a turn line someone else writes, or `resume`. While it is set, `Origin::Kelpie` gets `Refused::Paused`. |
| r1 | Partly fixed | Enter now clears a draft with or without hooks. See R3 r1. |
| r2 | Fixed | At most one `Origin::Kelpie` entry per chat queue. Further mail stays in the mailbox. |
| r3 | Fixed | `Request` now carries `model` and `skips_prompts`. |
| nits | Fixed | `branch_exists` uses `git show-ref`, and `discard_worktree` takes the worktree lock. |
| r4, r5 | Carried | Phase 4 docs and phase 3 mode switch. |

## Concurrency and the run engine (as asked)
- **Locks:** `run_gate` runs while the `runs` lock is held. The order is runs → slots → sessions (read) → per-session lock. No path takes them in reverse:
  - `Slot` holds no guard.
  - `create_inner` does not take `runs`.
  - The `view` closure releases its sessions read lock before `gate` is called.
  - No deadlock found.
- **Two tasks, one free place:** both can pass `run_gate` in the same step, because the step's check holds no slot. `start_task` checks again atomically with a held slot, and the loser goes back to Pending. No overshoot, only a Starting → Pending flicker on the Runs page.
- **Cancel:** gated Pending tasks cancel like any other.
- **Creator closed:** Pending tasks fail with a clear reason; running tasks continue without a parent. See r3 for retry.

## New findings

### Major

**R3-M1. The single classifier is too broad for D6 inheritance and gives children more permission than their parent**
- **Where:** `skips_permissions(entry)` (`sub_rpc.rs`) uses `args_skip_permissions` for two different questions:
  1. M3: does the target skip prompts? Detecting too much here is the safe direction.
  2. D6: may the child get its agent's full skip-prompts flags (`child_params`, `task_lineage`)? Detecting too much here grants permission.
- **Over-detections confirmed in `subs.rs:204-244`, each giving the child full bypass:**
  - `--allowedTools <anything>` (ANY_VALUE). A Claude parent started with `--allowedTools Read` gets children launched with full bypass, for example Codex with `--dangerously-bypass-approvals-and-sandbox`.
  - Codex `--full-auto` (sandboxed, still asks before escalating) and `-a never` alone (still sandboxed by default). Children get unsandboxed full bypass.
  - `--allow '*'` and `--auto high` from any kind.
- **Why it matters:** D6 says a child never gets more than its parent.
- **Fix:** split the classifier.
  - Inheritance should test strictly: the caller's own preset yolo sequence (`args_have_yolo`) or its exact full-bypass equivalents (Claude `--dangerously-skip-permissions` / `--permission-mode bypassPermissions`, Codex `--dangerously-bypass-approvals-and-sandbox`), or a chat whose `perm_skip` came from bypass mode.
  - Keep the broad `args_skip_permissions` for the `sub.message` target check only.
  - Add tests: a parent with `--allowedTools Read` gets a child with no yolo; a parent with `--full-auto` gets a child with no yolo.

**R3-M2. Tasks of a session-made run stall without explanation, because finished task sessions keep counting against the caps**
- **Where:**
  - `live_sub_counts` counts every alive session that has a `parent`.
  - Task sessions are never closed by the run engine: `Action::Release` only clears `interrupted` (`daemon.rs:799-806`). After `run.report done` the agent stays idle and alive.
  - `run_gate` returns `Gate::Wait` with no reason recorded.
- **Failure scenario:**
  - On this machine (auto cap 2), an agent makes a run `a → b → c`.
  - `a` and `b` finish, and their sessions stay alive.
  - `c` stays "pending" forever. Nothing on the task says why, and the creator gets no mail.
  - Any run with more tasks than the cap that can run in parallel (minus the creator's other children) hangs the same way. User-made runs are unaffected.
- **Fix:**
  - Leave sessions whose task is in a final state (done, failed, skipped) out of the cap counts, or let the engine close a done task's session when the run was made by a session.
  - Also give a waiting task a visible note ("waiting for room: 2 of 2 sub-sessions running").
  - Add a test: a run of 3 sequential tasks under cap 2 completes.

**R3-M3. A chat switched to bypass by restarting is not detected, so M3 misses the main way to bypass**
- **Where:**
  - `ChatSession::perm_skip` is set only from written lines (`permission_skip`), and starts `false` (`chat.rs:632`).
  - The app's bypass switch restarts the chat with `--permission-mode bypassPermissions` in `chat_argv` (`mode-switch.ts:319` → `replaceChat`, flags built by `withSettings` into `chatArgv`). That is not `spec.args`, and no `set_permission_mode` line is ever written.
  - A Claude chat resumed in the mode it last ran in (d7cefdc) behaves the same way.
- **Failure scenario:** the user switches a chat to bypass using the mode chip. A careful session's `sub.message` to it is allowed, because `skips_permissions` is false. The R2-M3 owner decision ("including the chat's runtime mode") is not met for this path.
- **Fix:** at create time, run the classifier on the `chat_argv` the daemon receives (`daemon.rs:1125`) and set `perm_skip` from it. Add a test with `chat_argv` containing `--permission-mode bypassPermissions`.

### Minor
- **r1. The draft guard reads keystrokes too simply.**
  - Backspace and Ctrl+U count as edits. A user who erased their draft blocks mail until the next Enter or turn. The mail stays in the inbox, so this only delays it.
  - Any write containing `\r` counts as Enter. In Claude, `\` followed by Enter, or Alt+Enter (`\x1b\r`), adds a new line rather than submitting. A multi-line draft paused after such a newline gets the mail pasted after it and submitted after 30 s quiet. This is a narrow return of M1.
  - Suggested: for agents with hooks, treat the draft as waiting until a turn starts; use Enter only for agents without hooks.
- **r2. The omp preset change is visible to users and outside this phase's scope.**
  - The flag is real.
  - Users with *Permissions: Yolo* will now launch omp with `--auto-approve` (`store.ts:1310-1311` reads `presetFor(kind).yolo`). It also feeds the `--auto-approve` in `args_have_yolo` and the D6 inheritance for omp.
  - This is consistent with the setting, but the owner should know, and the user guide's Yolo section (line 443) or the release notes should mention it.
- **r3. Runs whose creator is gone, or that are made too deep.**
  - When the creating session closes, a user `run.retry` of its tasks fails again at once ("closed"), so the user cannot adopt the run. Consider clearing `creator` when the user retries or approves.
  - `prepare_session_run` does not check the creator's depth, so a depth-3 session's run is accepted and then every task fails at start. Refuse it at creation.
- **r4 (carried).** Phase 4 docs for the contract changes (`session.create` / `run.create` / `sub.message` from sessions, `sub.request` fields).
- **r5 (carried).** Phase 3: the mode switch loses lineage.

### Nit
- After a Stop, a Kelpie mail entry already queued (with the queue paused) is written ahead of the user's next message once they send it, because the queue is first in, first out. Consider returning a queued mail entry to the mailbox on Stop.

## Remaining counts (round 3)
| Severity | Count |
|---|---|
| blocker | 0 |
| major | 3 (R3-M1, R3-M2, R3-M3) |
| minor | 5 (r1 to r5; r4 and r5 are carried to later phases) |
| nit | 1 |

Not ready to commit. All three majors are contained code fixes, and none needs a new owner decision.

Status: DONE_WITH_CONCERNS

---

# Round 4: re-review after "Fix round 3"

## Checks run
- `cargo test -p kelpied -j 2 -- --skip notify::destinations`: 836 passed, 0 failed, 3 ignored.

## Round 3 findings: verified in code
| Finding | State | Evidence |
|---|---|---|
| R3-M1 | Fixed | See the classifier section below. |
| R3-M2 | Fixed, with one loophole (R4-M1) | Waiting tasks get `Task.note`, and the creator gets one `Action::Waiting` mail per task (only when `note` was empty before). `announce_waiting` runs after `step_gated` has released the `runs` lock. A sequential run under cap 2 completes (test). |
| R3-M3 | Fixed | `create_inner` classifies the final `chat_argv` (after `claude_chat_argv`) with `launch_permissions` and sets both chat flags before spawning. Restored chats (`restore`, `daemon.rs:1071`) have no process and both flags false. They cannot be messaged (the target must be alive) or open children until the app creates them again through `create_inner`, which sets the flags. Chats cannot be restarted (`restart` is terminal only). No gap. |
| r1 | Fixed | `\x1b\r` (Alt+Enter) is an edit and not a submit. Enter right after `\` (same write, or `after_backslash` from the previous write) does not submit. The daemon's own Enter after a paste counts as a submit, and mail text always ends with the footer, never `\`. The erased-draft liveness cost is accepted and commented. |
| r3 | Fixed | `prepare_session_run` refuses at depth ≥ 3. A non-session `run.retry` clears `creator` when the creating session is gone (`adopt_orphaned_run`, `daemon.rs:2035`). |
| nit | Fixed | `Queue::mail_to_back` when a person's message arrives. |

## Strict vs broad classifier: every use checked
- **Strict** (`bypasses_permissions` → `args_full_bypass` plus `perm_bypass`) is used where permission is inherited:
  - `child_params` (`sub_rpc.rs:381`)
  - `task_lineage` (`:644`)
  - the approval card's `skips_prompts` (`:322`)
- **Broad** (`skips_permissions`) is used only by the `sub.message` target rule (`:460`). Using broad on both sides there is as decided: a sender that broadly skips prompts may message a target that broadly skips them.
- `args_full_bypass` lists only full-bypass spellings, keyed by kind:
  - `--allowedTools`, `--full-auto`, `-a never`, `--allow *` and `--auto high` grant nothing.
  - Claude `--allow-dangerously-skip-permissions` correctly does not count. It only unlocks a mode.
  - Codex `--yolo` is not in this machine's `codex --help`, so the entry is inert.
- Chat runtime flags: Claude `set_permission_mode` sets both flags, and any other mode clears both, which is right after a chip switch back. Codex `turn/start` with `approvalPolicy: never` sets bypass only when `sandboxPolicy.type == dangerFullAccess`.

## New finding

### Major

**R4-M1. A child that reports "done" or "failed" stops counting against the caps even while it keeps working**
- **Where:**
  - `live_sub_counts` (`sub_rpc.rs:188-201`): `reported_end` is computed from the last explicit report alone, and `info.status` is ignored.
  - Finished task sessions (`Runs::finished_sessions`) likewise count as finished whatever the session is doing.
- **Failure scenarios:**
  1. **Steering a finished child, the plan's core flow:**
     - A child reports `done`, and the parent messages it with a follow-up ("now also fix the tests").
     - The child works on it for a long time. Its row still holds the explicit `done`: the automatic idle report only replaces it after the next turn ends, and Working writes no report.
     - Meanwhile it is left out of the per-parent and global caps, so the parent can spawn up to the cap again. On this machine (auto cap 2), that means more working agents than the cap allows.
  2. **Reporting done too early:**
     - A child calls `kelpie_report done` early (a planning step, a confused model, or on purpose) and keeps working, without being counted for the rest of that turn.
     - The same applies to a finished run task whose session the user or parent continues.
- **Fix:**
  - Treat a session as finished only while it is not busy:
    `finished = (reported_end || finished.contains(id)) && !matches!(info.status, Status::Working | Status::Waiting)`
  - Optionally also require that the explicit report is newer than the session's last turn start.
  - Add a test: a child reports done, goes back to Working, and counts again; when it is Idle it does not count.

### Minor (carried, owned by later phases)
- **r2.** The omp Yolo flag `--auto-approve` is a change users will see. It needs a phase 4 docs or release-notes line.
- **r4.** Phase 4 docs for the contract changes: `session.create`, `run.create` and `sub.message` from sessions, the `sub.request` fields, and waiting-task notes and mail.
- **r5.** Phase 3: the mode switch loses lineage.

### Nits
- The waiting note includes live counts ("2 of 2 busy"). Every count change rewrites `runs.json` and broadcasts `run.updated`. This is bounded by session events, so it is acceptable. A fixed text with the counts sent only in the event would avoid the disk writes.
- The Codex chat strict flag depends on `turn/start` carrying `sandboxPolicy`. Without it, a chat launched in full bypass drops to "not bypass" after its first turn. That is the safe direction: children get no yolo flags. Not verified against the Codex adapter.

## Remaining counts (round 4)
| Severity | Count |
|---|---|
| blocker | 0 |
| major | 1 (R4-M1) |
| minor | 3 (all carried to phases 3 and 4) |
| nit | 2 |

Not ready to commit until R4-M1 is fixed. It is a one-line status condition plus a test, and no owner decision is needed. With that change and its test, phase 1 is ready to commit.

Status: DONE_WITH_CONCERNS
