# Test report: tabs and panes UX (Tile all / Spread / header-less lone pane)

Date: 2026-10-22 · Plan: `plans/261022-2252-tabs-panes-ux/plan.md` · Branch: rebrand/kelpie (uncommitted)

## Gates

| Gate | Result |
|---|---|
| `pnpm typecheck` | PASS |
| `pnpm i18n:check` | PASS: 1247 keys used, 1305 translated, 0 missing, 0 problems |
| `pnpm build` (vite) | PASS |
| `cargo build -p kelpie --features tauri/custom-protocol` (debug, UI embedded) + `kelpied` release | PASS |
| e2e `tabs_panes.py` (new) | 28/28 PASS |
| e2e `layouts.py` | 44/44 PASS (after fixing a stale test call, see below) |
| e2e `split_picker.py` | 46/46 PASS |
| e2e `smoke.py` | 16/16 PASS |
| e2e `phase1.py` (project = repo root) | 10/10 PASS |

Total 144 e2e checks, 144 passed.

## Covered by tabs_panes.py

- autoGrid 1..10, 12, cap 6 × 6.
- Lone shell pane: no header, hover toolbar has split / close, tab names the pane.
- 4 CLIs → 4 tabs → Tile all (menu, "+3 tabs" hint) → one 2 × 2 tab, tab order kept, sessions alive, 4 headers, 4 tab dots.
- 5th CLI → new tab, grid unchanged → Tile all (palette) → 3 + 2.
- Split a cell → lopsided 3 + 3 → Tile all → equal 3 + 3.
- Spread → 6 tabs, focused pane stays in the original tab, no headers.
- Split tab is left alone by Tile all; file tab and empty launcher tab are not gathered; broadcast off after tabs join.
- Palette `when` for Tile / Spread.
- Claude Code alone in a tab: no header, "Open in chat view" in hover toolbar (claude on PATH).

## Fixed test-side issue

`layouts.py` called `toTemplate(g, p => 'shell' | null)`; since commit 9268c3e (SSH panes) `leafOf`
returns `{ kind, target? }`. Pre-existing breakage, not from this change. Test call updated.
`remote.py` chat-view absence check now looks in any `.pane` (lone panes have no header).

## Process hygiene

Isolated: endpoint `kelpie-e2e-tabs`, data dir and cargo target dir under the session scratchpad.
tauri-driver 4448 / msedgedriver 4449 / kelpied (scratch build) — all stopped after the runs.
Not touched: another session's listeners on 4446 / 4447.

## Not covered

- remote.py, ssh_panes.py (need a local SSH test host); chat.py (real model calls).
- Real mouse hover over the toolbar (checked via DOM presence; CSS shows it on `:hover`).

## Unresolved questions

- None blocking.
