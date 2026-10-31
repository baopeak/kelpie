---
phase: 3d
title: "Settings → Mobile: every option a plain-language switch"
status: pending
priority: P1
effort: "2d"
dependencies: [3a, 3b, 3c]
---

# Phase 3d: Settings toggles

## Overview
User decision 2026-10-30: everything is a user-facing toggle with a plain explanation. Settings → Mobile gets one switch row per option, each saying what it does, the trade-off, and how safe it is. Defaults are safe; the user picks what is most convenient. Later phases add their row through the same component (Telegram in 3e, relay in 4).

## Data flow
```
mobile.status / notify.get → option rows (state, why unavailable) → change: daemon() when it adds safety, mobileSecure() (Role::App + OS verification) when it weakens or widens reach → mobile.updated → rows
```

## Requirements
- Functional
  - One component `option-row.tsx`: title, switch or select, three short lines (**What it does**, **Trade-off**, **How safe**), disabled state with the reason and the next step. No row invents its own layout.
  - Rows and defaults:

    | Row | Default | Backed by | Change needs presence |
    |---|---|---|---|
    | Tailscale mode | off | `mobile.set_tailscale` (3a) | turning on |
    | Web Push on this computer | off | device subscriptions + `notify` (3b) | no |
    | App lock | on | `mobile.set_lock` (3c) | turning off |
    | Lock after idle | see open questions | `mobile.set_lock` | lengthening |
    | Ask Face ID again for dangerous commands | off | `mobile.set_lock` | turning off |
    | Allow reading over plain HTTP | on (read-only) | `mobile.set_options` | turning on |
    | ntfy | off | existing `notify.set` | no |
    | Telegram | off | added in 3e | see 3e |

  - **No second source of truth**: ntfy and Telegram rows write through `notify.set`, the same call `push-settings.tsx` uses; server, topic and tokens stay in Settings → Notifications with a link across. Only the switch appears twice.
  - Each row lists which phones it affects where that differs (plain-HTTP devices cannot use lock or push).
  - A short summary at the top: how a phone reaches this computer now, and whether alerts and lock are on.
  - All strings in EN + VI (`pnpm i18n:check`); searchable through `settings-catalog.ts`. Written for someone who does not know what VAPID or WebAuthn is; those words do not appear.
  - The phone gets no settings verb: the table stays closed (`verbs.rs`).
- Non-functional: no new polling; state comes from `mobile.updated`.

## Backwards compatibility
Existing Mobile settings (address, port, cert/key, devices, reset) keep their place under an "Advanced" group. Boundary changes belong to 3a–3c; this phase adds no RPC unless a gap appears, in which case it takes the next free API level at commit time.

## Resilience requirements
- A daemon below the needed API level: row disabled with "restart Kelpie's background service" (existing level check), never a silent no-op.
- A failed change restores the switch and shows the error; the UI never assumes success.
- A failed presence check leaves the setting as it was.
- Busy machine: same async calls as today; no UI-thread work added.

## Related Code Files
- Create: `app/src/components/settings/option-row.tsx`
- Modify: `app/src/components/settings/mobile-settings.tsx`, `app/src/components/settings/push-settings.tsx` (link only), `app/src/lib/settings-catalog.ts`, `app/src/lib/ipc.ts`, `app/src/lib/types.ts`, `app/src/styles.css`, `app/src/locales/*`, `docs/user-guide.md`

## Implementation Steps
1. Write the wording table in EN and VI; check each line against what the daemon really enforces (`transport_cap`, lock choke point, push payload).
2. `option-row.tsx`; move existing advanced fields under the group.
3. Wire each row; presence rule per row; level checks.
4. Search catalog; guide section with the same wording.

## Tests
- `pnpm exec vitest run`: pure row-state function (enabled, disabled reason, needs presence) for each row and daemon level; wording keys present in both locales.
- `pnpm typecheck`, `pnpm i18n:check`.
- `cargo test -p kelpied`: weakening methods refused for `Role::User` (one test each, shared with 3a, 3c).
- Real-app pass through `scripts/e2e/` on an isolated daemon: toggle each row, confirm the daemon state via `kelpied call mobile.status` on that endpoint.

## Perf gate
None beyond the existing bundle check.

## Success Criteria
- [ ] Every option in the 2026-10-30 list has a switch with the three lines, in EN and VI.
- [ ] A fresh install has the safe defaults in the table above.
- [ ] Turning a weakening option on asks for OS verification; a token-only program cannot do it.
- [ ] A reader who has never heard of Tailscale can tell from the row what to do next.

## Risk Assessment
| Risk | L × I | Mitigation |
|---|---|---|
| Wording promises more than the code does | M × H | Step 1 check against code; reviewer reads it against the threat model |
| Settings page grows too long | M × L | Advanced group; summary at the top |
| Two places change ntfy/Telegram | L × M | Same `notify.set`, config fields in one place |

**Rollback**: UI only; revert restores the old page; settings stored by 3a–3c stay valid.
