# Code review: tabs and panes UX

Date: 2026-10-22 · Reviewer: code-reviewer agent (report saved by the controller session)

| # | Sev | Finding | Resolution |
|---|---|---|---|
| 1 | High | Tile all kept the tab's broadcast and extended it to every gathered pane (keystrokes could reach agents) | `tileTab` turns broadcast off when tabs join; `spreadTab` clears it |
| 2 | Medium | 36-cell cap also cut the tab's own panes (they vanished from the layout) | cap only on gathered tabs; own ≥ 36 → `arrangeTab` spill; toast for tabs left out |
| 3 | Medium | Tile gathered file / diff / browser tabs | candidates = one pane with a session (terminal / chat) |
| 4 | Medium | Menu and palette disagreed on when Tile is available | shared `canTile` / `canSpread` in the store |
| 5 | Low | Lone remote pane lost the host path | host chip tooltip shows the path |
| 6 | Low | `remote.py` chat-view check could no longer fail | selector widened to `.pane` |
| 7 | Low | Spread kept a group's custom tab name | name cleared on the kept tab |
| 8 | Low | Stale maximize gave a lone pane a header | `zoomedHere` requires 2+ panes |
| 9 | Low | Launcher start racing Tile opens a new tab | accepted (same as `arrangeTab`) |

Verified clean: autoGrid math, store mutation patterns (same as `mergeTab` / `movePaneToTab`),
e2e script logic against the app API.
