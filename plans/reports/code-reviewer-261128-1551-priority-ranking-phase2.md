# Review: commit 20968ed7, priority ranking, boosts and reservations (API 52)

Worktree: `D:/Works/Kelpie-worktrees/resource-monitor` (branch `feat/priority-ranking`). Scope: `crates`, `app`, `docs` (not `plans/`).
Line numbers refer to the files as they are at 20968ed7.

## Checks run
- `cargo test -p kelpied -- priority resource_policy boost verbs`: 16 passed.
- `pnpm exec vitest run src/lib/priorities.test.ts`: 4 passed.
- `pnpm typecheck` (covers `src` and `phone/src`): clean.
- I checked the i18n keys used by the new files against `src/locales/vi/*.json` with a grep. I did not run `i18n:check` because it was not in the allowed commands.

## Verified as safe
- **Trust boundary:** `auth::allowed` uses allowlists per role. `Role::Session` and `Role::Claimed` lists do not include `policy.resources.*` or `session.priority.set`, and anonymous callers only reach `daemon.info`. `event_visibility` falls through to `Hidden` for sessions, so no agent sees `policy.resources.changed`. The phone `boost` verb needs `Write`. A read-only device, or the lock's ReadOnly gate, gets `read_only` (tested).
- **Lock order:** no path holds `priorities` while it takes `sessions`, `spec` or `by_id`. `effective_priority` and `priorities_json` collect from `sessions` first and release it before they lock the policy. `priority_rpc` drops its guard before `priorities_changed` locks again, and nothing awaits while the parking_lot mutex is held. The ticker calls `priority_turn_ended` on a cloned `entries` Vec, so no session-map guard is held when it broadcasts.
- **Refused-change rollback:** both RPCs clone `file` and restore it on any error, including the replace-then-fail path in `set_session`. Tested.
- **Parent cap:** tier weight ranges do not overlap (B 1–1.5, N 2–3, C 4–6, boost 8), so comparing `(weight, tier)` is consistent.
- **Startup:** `restore()` runs synchronously before `ticker()` is spawned (main.rs:266/272). The first sweep therefore cannot drop the policies of sessions that are still being restored.

## High

### H1. The phone's boost button never shows a change in policy, so the toggle cannot end a boost
- **Where:** mobile.rs:4013 (`Translator::translate`), phone-state.ts:102.
- **Problem:** the phone's session list is republished only when `session.updated` or `session.closed` wakes `dirty`. `policy.resources.changed` does not wake it, so `boosted` and `boost_until` stay stale until some unrelated session update.
- **Scenario:** the person taps the bolt on an idle session and gets `ack ok`, but the row still says `boosted: false`. A second tap sends `"1h"` again instead of `"off"`, so the boost cannot be ended from the phone. A boost set in the window, an expiry, or a turn end never shows on the phone either.
- **Why the test passes:** `a_phone_boosts_a_session_and_its_row_says_until_when` calls `sessions(&daemon)` directly and never goes through the publish path.
- **Fix:** treat `is("policy.resources.changed")` as dirty next to `session.updated`. Add a test that waits on `dirty.notified()` after a boost, like the existing test at mobile.rs ~4944.

### H2. Boosts that do nothing, or that the UI hides, still take one of the two slots and cannot be ended from the UI
- **Where:** resource_policy.rs ~291 (parent cap), resource_policy.rs:413 (`sweep`), sidebar.tsx:181, priorities-page.tsx (`SessionLine`).
- **Capped child:** boosting a sub-session or run task whose parent is not boosted succeeds and enters `boosted()`. But `effective()` caps it to the parent (`boosted: false`, `source: "parent"`). The sidebar, the Priorities page and the phone all decide from `effective[id].boosted`, so they keep offering "Boost" and never offer "End". The slot stays taken until the boost expires. A "turn" boost holds it until the child's turn ends. The summary shows "Boosted: 1 of 2" with no boosted row anywhere.
- **Dead or restored session:** `sweep` keeps the policy of any session still in the map, dead ones included. The menus and the Priorities page show boost controls only when `s.alive`.
  - An `Until` boost on a session that exited holds a hidden slot until it expires.
  - A `Turn` boost survives in `resource-policy.json` across a daemon restart. Restored sessions are dead and `last_status` is empty, so no turn end ever fires. That boost holds a slot until the session is closed.
- **Fix:**
  - Refuse a boost (or report it as ineffective) when the parent chain would cap it, or let a boost lift the cap explicitly.
  - In `sweep`, drop boosts of sessions that are not alive. Drop `Turn` boosts when the policy is loaded.
  - In the UI, base "End boost" on membership in `priorities().boosted`, not on `effective.boosted`.

## Medium

### M1. `effective` is stale for sessions created after the last policy change
- **Where:** priorities.ts:76 (`weightOf`), store.ts event handler.
- **Problem:** the window's `effective` map changes only on `policy.resources.changed`, and creating a session never broadcasts one. A new session has no entry, so `weightOf` returns 0, which is below background (1.0).
- **Scenario:** the person marks project `web` critical, then opens a new agent in `web`. When it waits, Activity and "Next waiting" put it after every background session. The reverse also happens: a session whose policy was swept keeps its old entry.
- **Fix:** have the daemon broadcast `policy.resources.changed` (or a per-session effective value) on session create, close and restart. Alternatively, fall back on the client to the project's tier weight when an id has no entry.

### M2. Ranking is lost or tied when a project's policy changes
- **Where:** resource_policy.rs:359 and resource_policy.rs:350.
- **Pruning:** `set_project` deletes the entry when it equals `ProjectPolicy::default()`, which is Normal, rank 0, no reserve. That is exactly the top of the Normal tier after a drag. Removing that project's reservation (or setting the tier select to Normal on a rank-0 project) deletes it. It then becomes unranked: factor 1.0 in the daemon, sorted last in `projectsByTier`.
- **Tier change keeps the old rank:** moving a rank-0 project into a tier that already has a rank 0 gives two "top" projects (both 1.5). The page then orders them by sidebar order.
- **Fix:** do not prune an entry whose tier has other ranked entries, or store rank as an `Option`. When the tier changes, set `rank = max(rank in the new tier) + 1`, or put it at the top.

### M3. Cost: quadratic `SessionSpec` clones on every phone publish and every broadcast
- **Where:** priority_rpc.rs:35 (`by_id` per chain step), priority_rpc.rs:65, mobile.rs:2433.
- **Problem:** `by_id` scans `sessions` linearly and calls `e.spec()` for each entry, which takes the entry's mutex and clones the whole `SessionSpec` (env, args…). `effective_priority` does that once per level of the parent chain. Callers then run it once per session:
  - phone `sessions()`: every `session.updated`, for every connected phone;
  - `priorities_json`: twice per change, for the event and for the RPC reply.
  - Each call also takes the policy lock again and re-sorts the tier's ranks in `project()`.
- **Size:** with 40 sessions, one phone publish costs about 1,600 or more spec clones and 40 lock round-trips.
- **Fix:** add one `effective_all()` that reads `(id, parent, project)` for every session in one pass under one `sessions.read()`. Then lock the policy once and walk the chains from that map. Use it in both callers, and precompute the rank factors per tier once.

### M4. Disk writes on the ticker and on async workers
- **Where:** priority_rpc.rs:79 (`priorities_changed`).
- **Problem:** `priorities_changed` calls `persist::save_state` (atomic write plus `.prev`), and does it while holding the policy mutex. It runs on:
  - the 1 s ticker, through `priority_sweep` and through `priority_turn_ended` inside the per-entry loop;
  - the RPC dispatcher and the phone `Conn` task, both on tokio workers.
- **Conflicts with:** AGENTS.md ("spawn_blocking/fs_ops::blocking for substantial filesystem/config work") and the ticker's own comment two lines below ("touches the disk: not on the ticker"). A slow disk stalls the ticker, and every phone publish blocks on the policy lock while the write runs.
- **Fix:** snapshot `file` under the lock, then write it from `spawn_blocking`, keeping writes in order (for example a dirty flag and a single writer).

### M5. Docs and UI promise enforcement that does not exist yet
- **Where:** architecture.md:2410, user-guide.md:245, the Priorities page header, priority_rpc.rs:60.
- **Consumers:** architecture.md says "Admission, the command gate, the contention ladder and the token router read it". The user guide and the page say "Kelpie gives the most to the work you rank highest". In this commit nothing reads `effective_priority`, `max_agents`, `max_agents_per_project` or `switches` except the phone list and the JSON (grep-verified).
- **GPU:** the commit message and docs advertise GPU-memory reservations, but `reservation_machine` hard-codes `vram_mb: None`. `GpuReading` has only current `dedicated_mb`, no total. Every non-zero `vram_mb` is therefore refused.
- **Fix:** describe the policy as recorded and say which later phase enforces it, or hold back the user-facing claim. Drop GPU memory from the docs until a total is sampled.

### M6. "Until this turn ends" has no upper bound
- **Problem:** the boost ends only when the ticker sees a Working/Waiting → Idle/Exited transition.
  - A shell, an agent left idle, or a dead session never produces one, so the boost holds a slot indefinitely (see H2).
  - For an agent without hooks, the opposite happens: a silent long build can read as Idle and end the boost early.
- **Fix:** cap `Turn` at, for example, 2 h by storing `Turn { until }`. Do not offer it for `kind === "shell"`.

## Low
- **L1. Reservation sum can overflow** (resource_policy.rs:75). `Reserve::add` uses plain `+=` on u64. Sending `{projects: {a: {reserve: {ram_mb: 18446744073709551615}}, b: {reserve: {ram_mb: 2}}}}` in one request panics in debug and wraps to a small sum in release, so `check_reservations` accepts it. The input is the user's own, but later phases will act on these numbers. Fix: `saturating_add`, plus a per-field cap at the machine's total in `parse_reserve`.
- **L2. Events can arrive out of order.** `priorities_changed` saves, releases the lock, then builds the JSON. A ticker sweep and an RPC can interleave so that the older snapshot is broadcast last. The RPC reply can also overwrite a newer event in the window. Fix: build the JSON under the same lock and add a `rev` that the app compares.
- **L3. Editor UI after a refusal.**
  - `ReserveEditor` closes even when the daemon refused (priorities-page.tsx:42-43), because `save` and `setSessionReserve` swallow the error into a notice and resolve; the typed values are lost.
  - The `SessionLine` tier `<select>` (line 56) keeps showing the refused choice.
  - `readReserve` always sends `vram_mb: 0`, which wipes a GPU reservation set through the API.
- **L4. `boostLeft` units are not localized** (priorities.ts:86). It returns English ("1 h 5 min", "<1 min"), which is inserted into Vietnamese templates.
- **L5. Unbounded and surprising inputs in `policy.resources.set`.**
  - `order` skips the `MAX_ENTRIES` check and accepts any number of ids, including unknown ones.
  - `switches` replaces the whole struct, so any tier or switch left out is reset to its default or `false`.
- **L6. Remote sessions and projects get boost and priority items** (sidebar.tsx:181, projectMenu). A boost there only affects the local ssh client, but it takes one of the two slots.
- **L7. Phantom test name.** `the_file_keeps_the_policy_and_an_unreadable_one_is_left_alone` never exercises an unreadable file or the no-overwrite guard. Add a case with a corrupt file (and no `.prev`), call `save()`, and assert the bytes are unchanged.
- **L8. Drag-and-drop quirks.**
  - The whole `.prio-project` card is `draggable` and contains `<select>`s and number inputs. In WebView2, a mouse drag inside an input can start a card drag instead of selecting text.
  - Dropping inserts before the target. Moving a project one place down therefore means dropping it on the item two below; dropping on the next one does nothing. Consider an "after" drop zone on the lower half of a card.
- **L9. Third boost.** The user guide says "boosting a third asks which boost to end", but the app always offers only the oldest (priority-actions.ts). On the phone, the refusal is shown raw and includes internal session ids.

## Plan follow-ups
- The daemon policy model, RPCs, persistence, the phone verb, the app page and menus are all present.
- Nothing consumes the policy yet (M5): admission, the command gate, parking and the token router remain for later phases.

## Unresolved questions
- Is it intended that rank factors make priority dominate waiting time between any two differently ranked projects in the same tier? Any two ranked projects differ in weight, so "longest wait first" effectively applies only within one project.
- Should a boost on a sub-session lift its parent cap, or should it be refused?

Status: DONE_WITH_CONCERNS
Summary: The trust boundary and lock order hold, and the focused tests and typecheck pass. The phone's boost toggle never sees a policy change (H1), and boosts on capped, dead or restored sessions hold hidden slots that no UI can end (H2). Medium issues: stale weights for new sessions, lost ranks, quadratic spec cloning per phone publish, disk writes on the ticker, and docs describing enforcement and GPU reservations that do not exist yet.
