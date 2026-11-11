# WebView memory target + start-up/memory instrumentation: A/B

Date 2026-11-09. Windows 10, i5-6440HQ 4C, 8 GB, WebView2 157. Release builds, isolated daemon,
`perf_gate.py --metrics gui` (20 shells, 3 runs unless noted). Machine shared with other sessions
(installed Kelpie, another agent session); runs overlapping a foreign build/bench were discarded.

- **A**: origin/master `ffaeb84` + instrumentation only (start-up marks, gate changes). `perf-webview-mem-a.json`.
- **B**: A + low memory target applied at once on hide/minimise. `perf-webview-mem-b.json`.
- **B2** (shipped): B with a 5 s settle before going low; normal again at once. `perf-webview-mem-b2.json`.

## Results (medians)

| Metric | A | B | B2 |
|---|---|---|---|
| idle RSS, 20 shells (MB) | 532 | 531 | 538 |
| idle private (MB) | 360 | 356 | 365 |
| minimised RSS (MB) | 525 (466–575) | 402 | 385 |
| minimised private (MB) | 380 | 381 | 363 |
| page renderer while minimised (MB) | 161 | 28 | 12 |
| first frame after restore (ms) | 60 (60–159) | 135 (103–148) | 84 (83–112) |
| slowest of 30 switches after restore (ms) | 202 | 180 | 40 |
| echo p50 (ms) | 16.8 | 16.8 | 17.7 |
| long tasks | 0 | 0 | 0 |

Browser pane with a 60 MB page (`pane_memory.py`, renderer of the pane only, WS/private MB):

| Step | A | B | B2 |
|---|---|---|---|
| shown | 144 / 112 | 144 / 111 | 148 / 118 |
| hidden 1 s then shown (a menu) | — | — | 140 / 107 |
| hidden 40 s | 134 / 100 | 29 / 100 | 8 / 107 |
| shown again | 134 / 100 | 38 / 100 | 24 / 107 |

Reading: the target trims working sets (physical RAM back to the system), not private bytes; the
renderer pages back in lazily. The settle delay removes the menu case and did not cost restore time.

## Start-up (A, ms from process creation)

webview (page navigation starts) ~990–1140 · html +30 · entry module +300 · font +25–50 ·
saved state +110 · interactive +30. Cold start 1.07–1.80 s; ~60 % is before the page loads.
Loading the saved state in parallel with the font was tried (B) and gave no measurable gain
(script→state 135 vs 122 ms, within noise): dropped.

## Findings beyond the change

- `switch_p95_ms` ≈ 166–198 ms in A and B against the 2026-10-23 baseline of 36 ms: a regression
  already on master, not from this change. Not investigated here.
- `perf_gate.py` reused tauri-driver's default msedgedriver port 4445, shared with e2e runs of other
  worktrees and briefly unbindable after a run: second runs failed to start. Now a free port per run.

## Control: B2's gate FAIL

B2's own gate run failed idle CPU (3.96 %, limit 2.3) and switch p50 (30 ms, limit 27.3) in runs 2–3,
run 1 being 0.96 % / 26.1 ms. Interleaved control right after (2 runs each, A → B2 → A → B2):

| | idle CPU % | switch p50 ms | minimised RSS MB | restore frame ms |
|---|---|---|---|---|
| A | 4.11, 4.11, 4.09, 3.88 | 30.0–30.4 | 565–583 | 59–67 |
| B2 | 4.45, 4.30, 4.43, 3.67 | 29.9–31.2 | 384–394 | 78–79 |

A, without the change, moved the same way: the machine's state changed (other sessions, the
installed Kelpie), not B2. Difference A→B2 is within run-to-run noise (+0.16 pt CPU, switch equal).
The baseline in `scripts/perf/baseline.json` was not moved.

## Unresolved

- Root cause of master's `switch_p95_ms` regression (≈ 170–200 ms vs 36 ms baseline).
- Idle CPU of the whole budget doubled during the evening on both builds; re-check on a quiet machine
  (2–3 AM) before trusting any absolute idle-CPU number from today.
