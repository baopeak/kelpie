# Phase 2: Priority ranking (projects and sessions) and resource policy

Status: **implemented** on branch `feat/priority-ranking` (API 52), stacked on phase 1.

As built:
- **Sidebar:** the menus give Boost for 1 hour, Boost until this turn ends, End boost, and Priority & reservation… (opens the
  Priorities page on that row).
- **Priorities page:** the drag ranking, the tier selects, boosts and reservations.
- **Phone:** a ⚡ button on an open session (1 h, or end it), backed by the `boost` write verb.
- **Not done:** the sidebar project list is not re-sorted by tier, so the person's own order stays. The tier shows as a chip,
  and only the waiting list (Activity, next waiting session) orders by priority.
- **Defaults:** max_agents, the per-project cap and the per-tier switches are stored and editable over RPC; phases 3 to 5 read
  them.

## Context

Projects live in the app (`types.ts:73`: id, name, path, color, pinned, status), and the daemon knows only `session.project_id`. Policy that acts with the window closed must live in the daemon.

The owner wants to **rank priority by project and by individual session**, so that a chosen project or session always has enough resources to work.

## Requirements

1. **The priority model:**
   - **Tier per project:** critical, normal or background (default normal).
   - **Rank within the tier:** the user drags projects into order. A higher rank means a larger share inside the same tier.
   - **Session override:** a session takes its project's tier and rank by default. The user can raise or lower it alone. Example: one critical session in a normal project, or a background session (a long test run) in a critical project.

     A run task or sub-session inherits from the session that opened it, never above it.
   - **Boost** (temporary): "Prioritize this session" for 30 min, 1 h, 2 h, until its current turn ends, or until turned off. While boosted, the session is treated as the top of critical. Boosts expire on their own so they are not forgotten.

     At most 2 boosts at a time; a third asks which one to drop. Otherwise nothing stands out.
   - **Guaranteed reservation** (optional, per project or session): a minimum of CPU cores, RAM and GPU VRAM held free for it, such as "keep 2 cores and 2 GB for project X".

     When set, Kelpie checks it is achievable: all reservations together must stay under capacity minus a base for the OS and the window (on 8 GB: at most about 4.5 GB and cores − 1). A reservation that does not fit is refused, with the numbers.

     A reservation is only kept free while its owner has a live session. Unused, it is lent to others for **new light work only**, and taken back by holding new heavy commands of others, never by killing.
2. **Effective priority** = boost, otherwise the session override, otherwise the project tier and rank. It is computed in one function (`resource_policy::effective(session)`) used by:
   - the admission queue (phase 3);
   - the gate and optimizer shares (phase 4): tier weights 4/2/1, multiplied by a rank factor from 1.0 at the bottom of a tier to 1.5 at the top; a boost uses weight 8;
   - the contention ladder (phase 5): protected sessions are never demoted, and demotion starts from the lowest effective priority;
   - the token router (phase 6): when a budget is short, lower priority waits first.
3. **`resource-policy.json`** (daemon-owned, atomic, `.prev`, compare-before-write):
   - `projects: { <id>: { tier, rank, reserve?: {cores, ram_mb, vram_mb} } }`;
   - `sessions: { <id>: { tier?, rank?, boost_until?, reserve? } }`. Session entries are dropped when the session is closed;
   - `reserve_mb_default`: the free floor for critical work when no explicit reservation exists, `max(1024, 12 % of RAM)`;
   - `max_agents`: 0 means automatic, from the vectors (phase 3);
   - `max_agents_per_project`: optional;
   - per-tier switches `may_rewrite_parallelism`, `may_demote_children`, `may_park_when_idle`. Rewriting is on for all tiers (owner Q2). Demoting and parking default to off for critical and on for background.
4. **RPC** `policy.resources.get|set`, `session.priority.set {id, tier?, rank?, boost_for?, reserve?}`, and the broadcast `policy.resources.changed`. Roles: the app and the person's phone only. **Never a session**, so an agent cannot promote itself or demote another.
5. **UI:**
   - **Project:** a tier chip in the project's menu and settings; a ranked list (drag to reorder) in the orchestrator view; a reservation field with live "fits / does not fit" feedback.
   - **Session:** row menu → Priority (Critical / Normal / Background / As project), Boost ▸ 30 min · 1 h · 2 h · until this turn ends, and Reserve resources…
   - **Display:** a boosted or reserved session shows a small badge with the time left.
   - **Sort order:** "Needs you" (Waiting) still sorts first in the attention order. Then effective priority, then the existing order.
   - **Phone:** shows the badges and can boost or unboost, which is a person's action.
6. **Explanations everywhere.** A hold or rewrite caused by a priority says so. Example: "held: session X is boosted (1 h left) and reserves 2 GB."

## Files

- New `crates/kelpied/src/resource_policy.rs`: the model, `effective()`, the feasibility check, boost expiry on the daemon ticker.
- `daemon.rs`, proto.
- App: `app/src/lib/store.ts`, `types.ts`, `ipc.ts`, the project settings component, the session row menu, the orchestrator view's ranked list (phase 7 builds the rest of the view), `locales/*`.
- Phone: mobile verbs for boost and unboost (`mobile/verbs.rs`, an existing person-only path).

## Validation

- Unit tests:
  - `effective()` precedence (boost > session > project tier and rank);
  - inheritance never above the parent;
  - boost expiry, and boost at the end of a turn;
  - the at-most-2-boosts rule;
  - the reservation feasibility math on 8 GB and 4 cores;
  - an unreadable file is never overwritten;
  - `Role::Session` is refused on every set.
- Vitest: menu states, the drag-reorder result, the reservation feedback.
- i18n check.
- Real-app e2e (phase 7): boost a background session while two others build. Its next heavy command starts at once with the larger share. When the boost expires, the shares return to normal.

## Risk / rollback

- Risk: the user over-reserves. The feasibility check refuses it, and a reservation is only kept free while its owner is live.
- Risk: a forgotten boost. Boosts always expire and show the time left.
- Rollback: deleting the file restores the defaults (all normal, no boosts, no reservations).
