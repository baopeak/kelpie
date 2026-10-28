# Performance gate

`perf_gate.py` measures the release build and compares the medians with `baseline.json`.
Over budget means exit status 1 and a `FAIL` row; the numbers go to `plans/reports/perf-<label>.json`.

```bash
python scripts/perf/perf_gate.py --label phase-02 --compare scripts/perf/baseline.json
python scripts/perf/perf_gate.py --label ci --metrics bundle --compare scripts/perf/baseline.json   # no GUI
python scripts/perf/perf_gate.py --label rebase --update-baseline --reason "why the numbers moved"
```

## What it measures

| Metric | How | Budget (vs baseline) |
|---|---|---|
| `cold_start_ms` | app process creation time to the first frame with the sidebar drawn and state loaded (`__kelpie.perf.interactiveAt`) | x 1.05 |
| `switch_median_ms` (+ p95) | 210 project switches over 3 projects; `performance.now()` before `switchProject` to the second animation frame, in the page, 120 ms apart | x 1.10 (p95 warns above 52 ms) |
| `echo_median_ms` (+ p95) | 210 keys typed through xterm (`term.input`) into a visible shell; time until `onWriteParsed` and the screen shows the key | x 1.10 |
| `long_tasks` | Long Tasks API entries over 50 ms during the switches and typing | + 0 |
| `idle_cpu_pct` | app + its WebView2 processes + `kelpied`, 60 s with 20 shells open, as percent of one core (CPU-time delta over wall time) | + 1 |
| `rss_mb` | same processes, median of the working sets sampled every 2 s | + 30 |
| `entry_gz_kb` | gzip size of the entry chunk in `vite build`'s own report (lazy chunks are listed in the report) | + 15 |
| `bench_wall_s` | wall time of `kelpied bench --sessions 20 --mb 2` against an isolated daemon | x 1.05 |

Shells, `OpenConsole` / `conhost` and the agents under `kelpied` are the PTY children: reported as
`pty_idle_cpu_pct` / `pty_rss_mb`, never budgeted.

Timings are taken inside the page through `window.__kelpie.perf` (exposed only in dev builds and when
the app runs with `KELPIE_E2E`, like the rest of `__kelpie`), so WebDriver round trips (about 20 ms
each) never enter a sample. The gate runs the app `--runs` times (default 3; each run is a fresh
launch with its own daemon and data folder) and compares the median of the runs.

## Running the GUI metrics

1. Release app without installer: build the sidecars as `scripts/build-release.sh` does, then
   `cd app && pnpm tauri build --no-bundle --config src-tauri/tauri.bundle.json`
   (`build-release.sh --no-bundle` does both, and bumps the version).
2. `tauri-driver` on `PATH` (`cargo install tauri-driver`) and a `msedgedriver` matching the installed
   WebView2 runtime (`--msedgedriver <path>` or on `PATH`).
3. `pip install psutil`.

The gate starts and stops `tauri-driver`, the app and the daemon itself, with `KELPIE_ENDPOINT` /
`KELPIE_DATA_DIR` pointing at throw-away values, and refuses to start while port 4444
(`--driver-port`) is taken. Close other heavy programs first: 20 shells plus a WebView take a
few GB and the idle phase is sensitive to background work.

## Baseline

`baseline.json` holds the medians, the machine they came from, the spread of the runs and a history.
A baseline moves only with `--update-baseline --reason "..."`, never to make a regression pass.
`null` means the metric has not been baselined on any machine yet and is skipped in comparisons.

Numbers are only comparable on the same machine: CI compares the bundle size alone (and checks the
daemon bench's stream integrity); the timing metrics are for local runs on the baseline machine.

If the spread over the runs is above 10 % the gate prints a note: re-run on a quieter machine
before trusting a verdict.

## Checking that the gate catches a regression

`--inject-busy-loop 100` blocks the page for 100 ms once, from the test run itself (a `setTimeout`
started through `__kelpie`; product code is untouched). `long_tasks` must then exceed the baseline
and the gate must exit with status 1.
