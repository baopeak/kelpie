# Phase 1a: perf gate, TS unit tests, CI

Status: done. Everything ran here, including the GUI metrics on the release build.

## Files
- Created: `scripts/perf/perf_gate.py`, `scripts/perf/baseline.json`, `scripts/perf/README.md`, `app/vitest.config.ts`,
  `app/src/lib/layout.test.ts`, `app/src/chat/claude-adapter.test.ts`, `app/src/chat/codex-adapter.test.ts`.
- Modified: `app/package.json` (+ `test` script, `vitest` devDependency) and `app/pnpm-lock.yaml`, `app/src/main.tsx` (`__kelpie` block only),
  `.github/workflows/ci.yml`, `README.md` (Testing).
- Evidence: `plans/reports/perf-baseline.json` (3-run baseline), `plans/reports/perf-injected-busy-loop.json` (regression check).

## Tests
- `pnpm test`: 3 files, 59 tests (layout 30, Claude 16, Codex 13). Pure modules only; adapters run over a real Solid store context with inline recorded messages. No product code changed.
- `pnpm typecheck` clean; `pnpm i18n:check` 0 missing, 0 untranslated-looking.

## Perf gate
- `__kelpie.perf` (same KELPIE_E2E / dev gate, no new global or flag): `longTasks()` / `clearLongTasks()`, `interactiveAt()`, `switchProjects(ids, n)`, `echo(sessionId, text)`.
  Everything timed with `performance.now()` in the page.
- Metric groups: `bundle`, `bench`, `gui` (`--metrics`). Medians of `--runs` (3) fresh app launches; each launch has its own daemon, data dir and pipe name. Budgets from plan.md; p95 warns only.
- Entry gz is Vite's own number. Baseline moves only with `--update-baseline --reason`.
- Process accounting: the daemon is a child of the app, so its subtree (shells, OpenConsole) is split off as the PTY group (reported, not budgeted). The first baseline run missed this (RSS 2262 MB); fixed and re-run (below).
- The daemon bench starts its own `kelpied serve` (no pipe client written; the bench binary is the client).
- CI: `pnpm test`; bundle compare against baseline (fails over +15 KB gz); `cargo build --release -p kelpied` then `perf_gate.py --metrics bench` (stream-integrity gate, no timing compare since CI hardware differs).

## Baseline (median of 3 launches; Win10, i5-6440HQ 4C, 8 GB, shared with other builds)
| Metric | Median | Range |
|---|---|---|
| cold start | 2985 ms | 2620 - 3736 (spread 37 %) |
| project switch p50 / p95 | 24.8 / 36.0 ms | 24.65 - 24.9 (1 %) |
| echo p50 / p95 | 16.6 / 21.7 ms | 16.6 - 16.6 (0 %) |
| long tasks > 50 ms | 0 | 0 - 0 |
| idle CPU, app+WebView2+daemon, 20 shells, 60 s | 1.30 % of a core | 1.12 - 2.71 (spread 122 %) |
| RSS same group | 509 MB | 501 - 513 (2 %) |
| PTY children (not budgeted) | about 1.7 GB RSS, 0.2 - 0.5 % CPU | |
| entry chunk | 323.6 KB gz (219 lazy chunks, 1627 KB) | |
| `kelpied bench` 20 x 2 MiB | 3.72 s | 3.70 - 3.82 |

Spread target (10 %) met by switch, echo, RSS, bench, bundle. **Not met by cold start (37 %) and idle CPU (absolute range 1.6 pt, wider than its +1 pt budget)**: noisy because the machine was building Rust for another task at the same time. Recorded in `baseline.json` notes; re-baseline on a quiet machine before trusting those two verdicts. (An earlier run, before the process-group fix, gave cold 2537 ms and switch 24.95 ms, similar.)

## Injected regression
`--inject-busy-loop 100` (a `setTimeout` busy loop started through `__kelpie` at runtime) -> `long_task_ms: [100]`, `long_tasks 1 vs 0` -> `FAIL`, exit 1. Other rows ok.

## Notes
- Echo is quantised by frames (16.7 ms steps); a 10 % budget on a 16.6 ms median is about one frame, so a real regression of that size shows up only as the next step. Fine for a gate, noted for interpretation.
- Release build made without `build-release.sh` (it bumps versions in files outside my ownership): same cargo/tauri steps, `--no-bundle`. Built in `target/release` (ignored by git, about 2 GB).
- `msedgedriver` 155.0.4283.62 (matches Edge 155) downloaded to the session scratchpad; not in the repo. Needs `--msedgedriver <path>` or PATH.
- Disk: C: is at 0 bytes free (not from this work); Cargo/linker/Vite temp went to a D: folder (removed). Anyone re-running may need `TEMP`/`TMP` pointed at a roomy drive.
- The user's own installed Kelpie and another session's bench daemon were running; untouched.

## Unresolved
- Idle-CPU budget unit chosen: percent of one core, absolute +1.
- CI runs `python`; relies on the runners' preinstalled Python (psutil not needed for bundle/bench).

Status: DONE_WITH_CONCERNS
