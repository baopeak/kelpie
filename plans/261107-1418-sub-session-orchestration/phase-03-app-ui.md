# Phase 3 — App UI

## Context
`store.ts` keeps sessions flat; `sidebar.tsx` groups them by `project_id`. Add lineage rendering, the
approval card and settings. Solid signals/stores, typed IPC wrappers, EN keys + `locales/vi/*.json`.

## Requirements
1. **Types/IPC** — `SessionInfo.parent?`, `depth?`, `report?`; `ipc.subAnswer(id, decision)`, `ipc.subPending()`,
   policy wrappers for `sub_sessions` / `sub_max` / `sub_auto_max`; API gate constant `SUB_SESSIONS_API` (= phase-1 level).
2. **Sidebar tree** — a session whose `parent` is a live session renders nested under the parent's row
   (indent + thin guide line, any depth), not in its own project's list; parent row shows a compact count badge
   with the worst child state colour (waiting > working > idle). Collapsible per parent (state kept in UI store only).
   A child whose parent is gone renders where it does today.
3. **Children strip** in the parent's pane header area (only when it has children): chips = status dot, agent
   icon, name, last report status; tooltip = report summary + time; click opens/focuses the child the same way
   a sidebar click does; overflow "+N". Context action *Close finished sub-sessions* (children idle/exited with a
   `done`/`exited` report) with a confirm listing them.
4. **Child header link** — a child's pane shows "Sub-session of <parent>" (click → parent).
5. **Approval card** — on `sub.request` (and pending on connect): non-blocking card stack (bottom-right, like
   existing toasts/inbox) "<parent> wants to open a sub-session: <agent> in <folder>[, new worktree] — <task…>",
   buttons Allow once / Allow for this session / Always allow / Deny; also an inbox entry so it survives a hidden
   window. Disappears when answered anywhere or expired.
6. **Settings → Agents → Sub-sessions** (settings-catalog entry, searchable): mode Off / Ask (default) / Allow;
   Max running: Auto (shows N for this computer) or 1–16; note "Agents started before this change get the tools
   after a restart"; link to user-guide section. Hidden on daemons below the API gate (with the existing
   "restart the background service" offer).
7. i18n: every new string in `locales/vi/*.json`; `pnpm i18n:check` 0 missing.

## Files
- modify `app/src/lib/types.ts`, `ipc.ts`, `store.ts` (pure helpers `childrenOfSession`, `sessionDepthTree`),
  `components/sidebar.tsx`, `pane-view.tsx` (strip + link), `settings/agent-settings.tsx`, `lib/settings-catalog.ts`, `styles.css`
- create `app/src/components/sub-sessions-strip.tsx`, `app/src/components/sub-request-card.tsx`, `app/src/lib/sub-sessions.ts` (+ `.test.ts`)

## Tests
vitest for tree building (orphans, cycles defended, depth), worst-state badge, close-finished selection, request queue
reducer. `pnpm typecheck && pnpm test && pnpm i18n:check`.

## Note
The shared worktree `D:\Works\Kelpie` has other sessions' uncommitted UI edits; this phase works only in
`D:\kelpie-subsessions`.
