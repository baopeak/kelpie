# Tabs and panes: one clear model, tile / spread flows

Status: done (uncommitted) · Branch: rebrand/kelpie

## Result

- Acceptance 1–3 met. e2e: tabs_panes 28/28, layouts 44/44, split_picker 46/46, smoke 16/16,
  phase1 10/10. Report: `plans/reports/tester-261022-2252-tabs-panes-ux.md`.
- Review fixes: Tile gathers CLI tabs only (terminal/chat), broadcast off when tabs join, Spread
  clears tab name/broadcast, 36-cell cap only on gathered tabs (own overflow → arrange spill),
  one shared `canTile` / `canSpread` for menu and palette, host path in the host chip tooltip,
  stale maximize ignored for a lone pane.
  Review: `plans/reports/code-reviewer-261022-2252-tabs-panes-ux.md`.

## Brainstorm contract

- **Outcome**: tabs and split panes never repeat each other, and opening many CLIs has one
  predictable path: open them (tabs) → tile them into one grid → spread them back.
- **Model** (Zellij-style "one tab = one context, panes = the work inside it"):
  - Tab = identity (glyph, title of the focused pane, host chip, pane dots when split).
  - Pane header only when it tells panes apart: 2+ panes, a maximized pane, file / diff viewers
    (they need a bar). A lone CLI pane keeps its controls in the hover toolbar, incl. the
    chat ↔ TUI switch.
  - New session / terminal → new tab (unchanged). Split → halves the focused pane (unchanged).
  - New: **Tile all** puts this tab's panes plus every one-pane tab into an even grid
    (5 → 3 + 2, 7 → 3 + 2 + 2); multi-pane tabs are deliberate layouts and stay.
  - New: **Spread into tabs** gives every pane of this tab its own tab (undo of Tile).
- **Scenario** "4 CLIs, 2×2, then a 5th": 4 tabs → Tile all → one 2×2 tab. A 5th CLI opens in a
  new tab (tab bar shows the grid tab with 4 dots + the new one). Tile all again → 3 + 2 grid.
  Split instead → the focused cell halves; Tile all evens it out.
- **Constraints**: frontend only; reuse `buildGrid` / store patterns; vi strings for every new
  label (`pnpm i18n:check`); keep existing command ids, picker and arrange behavior.
- **Non-goals**: auto-retiling on every new session; changing what grid presets do.

## Acceptance

1. Single-pane tab with Claude (chatable) shows no `.pane-header`; hover toolbar has
   "Open in chat view"; 2+ panes show headers as before.
2. Layout menu + palette: "Tile all" and "Spread into tabs" with the behavior above; tabs whose
   pane joined the grid close; sessions keep running.
3. typecheck, i18n:check, vite build, release build; e2e: new `tabs_panes.py` + layouts.py,
   split_picker.py, smoke.py pass.

## Files

- `app/src/lib/layout.ts` (`autoGrid`), `app/src/lib/store.ts` (`tileTab`, `spreadTab`),
  `app/src/lib/commands.ts`, `app/src/components/layout-menu.tsx`, `app/src/components/pane-view.tsx`,
  `app/src/locales/vi/*.json`, `scripts/e2e/tabs_panes.py`, README e2e table, user guide.

## Rollback

Revert the touched frontend files; no state format change (tabs/panes shapes unchanged).
