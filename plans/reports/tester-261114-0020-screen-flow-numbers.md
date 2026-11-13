# Screen flow: before/after numbers (emulated phone)

Branch `screen/flow-quick-wins` (API 40) against master `6f2aea2` (API 38), measured with `scripts/e2e/screen_net.py` on 2026-11-13/14.

## Setup and caveats

- Machine: Windows 10, Intel HD 530 (QSV hardware H.264), one 2560×1440 monitor, 8 GB RAM; another agent session compiled Rust on the same machine at times.
- Both runs: debug `kelpied` and app, **release** `kelpie-screen` (a debug helper encodes at about 5 fps and measures nothing useful), isolated endpoint and data dir, emulated phone through `net_fault_proxy.py` (acks 8 ms after arrival, standing in for decoding), primary monitor shared, a topmost tkinter window scrolling text at 30 fps, 60 s per condition.
- **This encoder is the ceiling.** It takes 30–65 ms p95 per 1080p frame, so it caps the stream at about 15–20 fps whatever the network. The absolute targets ≥ 28 fps, ≥ 27 fps and ≥ 24 fps cannot be judged here; the before/after differences can. A scrolling page is cheap for H.264 (motion vectors), so the rates stay low (13–70 kbps) and the 300 kbps cap is rarely the binding limit.
- The matrix ran in chunks (Vite and the app started for each). Rows taken while the other session compiled were run again (conditions 2, 3, 5 both builds; 7, 8 after a fix below); the table has the quiet runs.
- Picture age needs `t0`, which only the new daemon sends: "before" has none. "queued" is the age above the run's best frame, for both.

## Results (before → after)

| # | One-way delay, fault | fps | age p50 / p95 ms | click p50 / p95 ms | cursor p50 / p95 ms | size | cuts | in-flight window | other |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 25 ms | 14.9 → 14.8 | – → 105 / 143 | 46 / 82 → 53 / 114 | 102 / 156 → 102 / 148 | 1080p → 1080p | 1 → 0 | 3 → 5 | encoder-capped |
| 2 | 50 ms | 13.8 → 14.7 | – → 96 / 200 | 68 / 74 → 61 / 80 | 153 / 191 → 141 / 176 | 1080p → 1080p | 0 → 0 | 3 → 7 | before: 2 freezes > 2 s; after: none |
| 3 | 100 ms | **9.8 → 19.7** | – → 137 / 152 | 116 / 132 → 114 / 120 | 249 / 286 → 239 / 266 | 1080p → 1080p | 0 → 0 | 3 → 10 | |
| 4 | 50 ms, 1 % loss model | 15.2 → 13.8 | – → 99 / 219 | 69 / 81 → 68 / 88 | 153 / 318 → 146 / 290 | 1080p → 1080p | 1 → 0 | 3 → 7 | noisy (encoder p95 17 → 30) |
| 5 | 50 ms, 5 % loss model | **8.1 → 13.9** | – → 100 / 285 | 68 / 170 → 67 / 163 | 162 / 365 → 157 / 346 | 722p → 1080p | 10 → 5 | 3 → 7 | |
| 6 | 50 ms, 300 kbps down | 15.6 → **21.2** | – → 104 / 128 | 63 / 72 → 65 / 75 | 152 / 190 → 146 / 176 | 1080p → 1080p | 0 → 0 | 3 → 7 | ≤ 300 kbps at once both; keyframes ≤ 10 KB |
| 7 | 50 ms, 10 s blackhole | 7.1 → 9.1 | – → 86 / 118 | – | – | 810p → 722p | 11 → 9 | 3 → 7 | same `sc` resumes 0.05 s after the path returns, both |
| 8 | 50 ms, 60 s blackhole | 2.8 → 7.4 | – → 94 / 123 | – | – | 722p → 356p | 15 → 15 | 3 → 7 | before: `link_lost` 60.5 s after the drop; after: the screen survives (rebind on the new connection) |

No `slow_consumer` disconnect in any run. `screen.py` (regression): 45/45 PASS on the new build.

## Against the phase's acceptance

| Target | Result |
|---|---|
| 100 ms RTT (row 2): ≥ 27 fps and ≥ before + 5 | 14.7 vs 13.8: encoder-capped here; at 200 ms RTT (row 3) +9.9 fps |
| 200 ms RTT (row 3): ≥ 24 fps | 19.7 (encoder cap ~20) |
| age p95 ≤ RTT + 120 ms at 0 % loss | 143 ≤ 170, 200 ≤ 220, 152 ≤ 320: met |
| 100 ms RTT: cursor p95 ≤ RTT + 50; click p95 ≤ RTT/2 + 40 | cursor 176 vs 150: **not met** (Python proxy and phone, 33 ms cursor poll); click 80 ≤ 90: met |
| 1 % model: ≥ 20 fps, no freeze > 2 s, no disconnect | 13.8 fps (encoder cap); no freeze, no disconnect |
| 5 % model: ≥ 10 fps, no disconnect | 13.9 fps, none: met |
| 300 kbps: ≤ 300 kbps in 10 s, ≥ 5 fps, age p95 ≤ 1.5 s, keyframe ≤ 40 KB, no `slow_consumer` | met |
| 10 s blackhole: same `sc` resumes ≤ 5 s, keyframe first | resumes in 0.05 s on the same connection (TCP held the bytes, so no keyframe is needed) |
| 60 s blackhole: `link_lost` at 45 ± 2 s | **differs**: a TCP blackhole is noticed only after the link's 45 s of silence (`lanes::DEAD`), so the 45 s grace starts then; the screen came back when the path did at 60 s |

## Fix made from these numbers

The first "after" run of row 7 cut 19 times (to the lowest rung): a window-full spell was cut every 500 ms once the round trip was short. Only the first cut of a spell now comes after twice the round trip; each later one waits a second more, as before. Row 7 then cut 9 times.

## Owner's devices

The owner's phone on 4G over Tailscale (debug view numbers) is not a gate and was not measured here.
