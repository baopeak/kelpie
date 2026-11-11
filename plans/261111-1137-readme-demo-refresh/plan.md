---
title: "README, demo kit, demo scripts and guides refresh"
description: "Inventory every 0.1.15 feature, extend the demo fixtures and screenshot tour, write demo scripts, and rewrite README (EN + VI), user guide (EN fixes + new VI guide) and AGENTS.md."
status: in-progress
priority: P2
branch: docs/readme-demo (worktree D:\kelpie-docs-demo, from origin/master af17880)
created: 2026-11-11
---

# README, demo kit and guides refresh

Inventory: [scout-261111-1137-feature-inventory.md](../reports/scout-261111-1137-feature-inventory.md).

## Brainstorm contract
- **Outcome**: a reader of README wants to try Kelpie; every 0.1.15 feature is findable from README → user guide; a presenter can run the demo kit and follow written demo scripts.
- **Decisions (owner, 2026-11-11)**: bilingual README (`README.md` EN + `README.vi.md`); update `docs/user-guide.md`, add `docs/user-guide.vi.md`, update `AGENTS.md`; extend fixtures **and** recapture screenshots; work in a separate worktree. Positioning: the daemon's value is *updates never interrupt running agents*, not "close the window and walk away" (background mode is rarely used).
- **Style**: complete sentences with subject and predicate; clear, plain claims; no invented results; sample data labelled as samples.
- **Constraints**: disk is tight (build target on `C:\kelpie-docs-demo-target`, debug, no debuginfo); screenshot tour uses its own data dir and endpoint, never the real daemon; real model calls only for the existing `--chat omp --agents` shots; Claude test session exits cleanly (fullscreen strike rule).
- **Non-goals**: no product code changes; no installer/release build; no push or PR unless asked.

## Phases

| # | Phase | Files | Done when | Status |
|---|---|---|---|---|
| 1 | Extend demo fixtures | `scripts/e2e/readme_demo.py` | seed into an empty dir succeeds; every `smoke.py` passes; new fixtures: `.kelpie.yaml` setup/tabs/copy, `plans/` plan with phases, run JSON, notebook, Mermaid `.mmd`, demo plugin | done |
| 2 | Extend screenshot tour | `scripts/e2e/readme_shots.py` | new shots: welcome (Kip), runs, processes, finish, hosts, try, plugins, notebook; `capture.json` written from the run (version, platform, worktrees, shots, checks); Claude exits with `/exit` | done (Full HD, four agents, review fixes) |
| 3 | Capture | `docs/screenshots/*` | tour passes all checks on the debug build; orphan `history.png` removed; images viewed | done (22/22 checks, 38 shots at 1920 × 1080) |
| 4 | Demo scripts | `docs/demo-scripts.vi.md` | 3-minute pitch, 10-minute tour and themed scenes; each scene: goal, setup, steps, narration, expected result, reset | done |
| 5 | README EN + VI | `README.md`, `README.vi.md` | every feature area linked to the guide; accurate close/update behaviour; Linux listed; language switch links | done |
| 6 | Guides | `docs/user-guide.md`, `docs/user-guide.vi.md`, `AGENTS.md` | inventory gaps fixed; VI guide ≤ 800 lines, task-oriented, links to EN sections for detail | done |
| 7 | Verify & review | — | links and anchors resolve; fixture/tour scripts compile; reviewer pass on scripts and docs; findings fixed | done (reviewer findings fixed) |

## Risks
- Capture flakiness or a UI change since 0.1.5 breaks selectors → fix selectors from current components, rerun.
- Disk: build ≈ 3 GB on C:; delete `C:\kelpie-docs-demo-target` when done (with the user's OK).

## Findings for the owner
- Welcome text said closing the window never stops sessions; fixed in `welcome.tsx` and `locales/vi/shell.json` to "switching projects or updating Kelpie".
- OMP has no hooks; Kelpie's screen heuristics marked an idle OMP as *Needs you* because its random tip line ended in "?" ("Spaghetti code? Try complaining with /omfg"). Product false positive, not fixed here.
- Codex receives the terminal's OSC 10/11 colour reply (`]10;rgb:…]11;rgb:…`) as typed text in its prompt; seen in 2 of 4 runs. The tour clears it with Ctrl+U before shooting; the defect itself is not fixed here.
- A pane moved into a tab that already holds a browser pane leaves the page unpainted (navigation buttons greyed, Kelpie's placeholder shows); reproduced twice in an isolated probe. The tour opens the browser beside OMP instead.
