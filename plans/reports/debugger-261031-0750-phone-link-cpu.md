# Phone link: daemon CPU

Branch `phone/phase3-link`, uncommitted working tree. **The work stopped partway.** The instrumented release build was stopped by the harness because the system was critically low on memory, and I was told not to restart it unless asked. The seal measurement and the A/B are done. The per-section breakdown, any reduction and the re-measure are not.

**No code changed.** All probes are removed: `sync.rs`, `link.rs` and `mobile.rs` match their starting hashes and timestamps.

## Verdict: (b), on the tree as it stands

**The crypto overhead against the pre-encryption daemon is at most 5 points, but the script's total still exceeds its limit.**

- The overhead is between about 0.1 and 4.6 points, depending on the estimator. The cycle-counted ones say about 0.1 to 1.3 (table below).
- The script's gate failed on all three runs of the current daemon, and on one of three of the pre-encryption daemon.
- **Reason:** the cost is almost all in work that predates encryption, about 10 points of tokio worker time on both daemons.
- This is the verdict before any reduction. Whether the avoidable part brings the gate under 5 is untested.

## Seal measurement

The benchmark called `Session::seal` in a loop, on one thread, in a build using the workspace release profile. The session came from a real `Initiator::reconnect` / `read_first` / `accept` handshake. Load was 76 before and 35 after.

| Operation | Time | At the phones' rate | Points of a core |
|---|---|---|---|
| Seal 18.4 kB (5 rounds of 20,000) | 30.5 to 34.5 µs (534 to 603 MB/s), one chunk, 18,417 bytes out | 113.6 frames/s (8 x 14.2) | **0.35 to 0.39** |
| Open one ack | 2.97 µs | 56.8/s | 0.017 |
| `to_vec` of 18.4 kB | 1.13 µs | | scale only |
| `with_capacity(65520)` plus copy | 1.08 µs | | scale only |

This is a tight loop with warm caches, so real calls will cost somewhat more.

- **Release profile:** `opt-level = 3`, thin LTO, one codegen unit. Nothing is aimed at size.
- **Cipher path:** `snow` 0.10 calls RustCrypto `chacha20poly1305`. `chacha20` 0.9.1 selects AVX2 at run time, and `poly1305` 0.8.0 has an AVX2 backend. This CPU (i5-6440HQ, 4 cores, no hyper-threading) reports AVX2. There is nothing faster worth enabling for a cost this small.

The previous estimate of "well under 2 points" holds, and it is lower than that. The older +0.4 does not argue against it, as the A/B shows.

## A/B on this machine

- **A:** `d6e5883`, with that commit's own `mobile_perf.py` and `phone_support.py`.
- **B:** the current tree, with `--pair-with target/debug/kelpied.exe`.
- **Pin:** `KELPIE_LOAD_FORCE` exists at `d6e5883` and has no `cfg` around it. The old script starts its daemon with the parent's environment, so I set `L0:calm` in its process. The new script sets it itself. Only B prints a level, and every B round reported level 0. A prints none, but its frame rates match B's, so it was not shedding.
- **Rounds:** 3 rounds per run. Frame rates and sizes match, so the comparison is valid.
- **Extra counters:** a wrapper in the scratchpad ran each script unchanged and added counters. `QueryProcessCycleTime` gives cycle counts; the script itself uses `GetProcessTimes`, which advances in clock ticks. A1 and B1 ran before I added the cycle counters.

| Run, in order | Load before / after | Host busy, whole run | Script: points per round | Script median | Gate | Frames/s/phone | Frame kB | Cycles: added, no ingest correction | Cycles: worker threads, with phones |
|---|---|---|---|---|---|---|---|---|---|
| A1 | 81 / 47 | 96.4 % | +4.2, +4.1, +2.7 | +4.1 | PASS | 14.3 | 18.4 | not recorded | not recorded |
| B1 | 65 / 45 | 95.4 % | +11.1, +10.5, +8.8 | +10.5 | FAIL | 14.27 to 14.33 | 18.4 | not recorded | not recorded |
| B2 | 45 / 42 | 95.2 % | +10.7, +4.5, +5.0 | +5.0 | FAIL | 14.31 to 14.36 | 18.4 | +10.30, +10.12, +10.58 | 13.01, 11.23, 12.14 |
| A2 | 53 / 42 | 95.5 % | +9.5, +2.3, +10.4 | +9.5 | FAIL | 14.25 to 14.44 | 18.4 | +9.03, +8.37, +9.34 | 11.94, 8.12, 11.37 |
| B3 | 43 / 45 | 95.2 % | +9.4, +1.7, +5.0 | +5.0 | FAIL | 14.27 to 14.32 | 18.3 to 18.4 | +10.02, +9.01, +9.79 | 12.76, 9.71, 11.20 |
| A3 | 53 / 59 | 95.6 % | +10.2, +4.5, -8.1 | +4.5 | PASS | 14.26 to 14.40 | 18.4 | +9.52, +10.19, +10.15 | 11.70, 10.44, 8.90 |

Each run sent 0 frames for the session nobody follows. Host busy over the measured windows themselves was 99.8 to 100 % in all 36 steady rounds, with and without phones.

**How much B costs over A, by estimator:**

| Estimator | A | B | B minus A |
|---|---|---|---|
| Cycles added, no ingest correction, mean of 6 rounds | 9.43 | 9.97 | **+0.5** |
| Cycles, ingest-corrected as the script does, mean of 6 / median of 6 | 8.92 / 10.27 | 10.23 / 10.34 | +1.3 / +0.1 |
| Worker-thread cycles, no subtraction at all, mean of 6 / median of 6 | 10.41 / 10.90 | 11.68 / 11.69 | **+1.3** / +0.8 |
| Script's figure, the same 6 rounds, mean | 4.8 | 6.0 | +1.2 |
| Script's figure, all 9 rounds, mean / median | 4.4 / 4.2 | 7.4 / 8.8 | +3.0 / +4.6 |

Every estimator is under 5 points. The widest figures depend most on A1 and B1, the one pair with no cycle data, and they carry the most noise.

## What the script's number is doing

Three measured observations:

1. **The gate does not separate these two daemons reliably.** The same pre-encryption binary gave run medians of +4.1, +9.5 and +4.5, and rounds from -8.1 to +10.4. Total cycles in the with-phones rounds of A2 and A3 stayed between 34.70 and 35.67 %.
2. **The ingest correction is a large noise source.** Ingest in the with-phones rounds ranged from 1.03 to 1.79 MB/s while total cycles barely moved. A3's rounds are the example: 1.03 MB/s at 34.73 % and 1.78 MB/s at 35.36 %. At about 19 % per MB/s, that swing alone moves the result by about 14 points. CPU does not appear to scale with `seq` over a 14 s window at this rate, which the correction assumes.
3. **Tick accounting reads low.** With-phones rounds read 27.7 to 33.5 % by ticks against 34.7 to 36.6 % by cycles. Short bursts on worker threads falling between clock ticks would explain that, but I did not prove the mechanism.

This also speaks to the earlier first-round question. In all four cycle-counted runs, the first with-phones round had the lowest ingest of its run (1.03 to 1.22 MB/s), while its total cycles matched the later rounds. On those counts the high first round looks like the correction, not extra cost. Why first-round ingest reads low is unknown.

**The +0.4 from the older report** was not reproduced at `d6e5883`: no run median here came near it. That report's rounds were +0.4, +0.7 and -1.6 at about 15 frames/s/phone, on an older tree and using the same tick-based subtraction. Its frame rate rules out shedding. Three rounds of a measure that spans -8.1 to +10.4 here are weak evidence of a lower cost, so I do not treat it as a real baseline. I cannot say how much of the gap comes from the older tree.

I did not change the script, the limit or `baseline.json`.

## Where the cost goes

**Measured:**

- Phones' cost lands on the tokio worker threads: about 0.4 points without phones, about 11 with. On the current daemon that is about **1 ms of CPU per frame** (11.68 points / 114 frames/s), against 32 µs for the seal. On the pre-encryption daemon it is about 0.9 ms.
- PTY reader threads do not grow with phones. They use about 25 points either way, 23.0 to 26.5 with phones and 24.6 to 27.3 without.
- Page faults stayed at 0 to 10 per second with phones, no higher than without (0 to 26), so Windows heap decommit is not the cost.
- Other I/O operations ran at 132 to 179 per second with phones and 0 to 4 without. Frames and acks together come to about 170 per second, so this is roughly one operation per message. It is a rough reading, because I did not establish exactly what that counter includes. A frame of this size is one chunk, per the seal measurement, so chunking adds no writes here.

**Not measured:** how the 1 ms divides. The probes were written but never compiled into a binary, so everything in the next list comes from reading the code, in the order I would test it.

1. **`Views::frame` (`sync.rs`) diffs twice per frame, under the global `slots` lock.** Under a flood no shift matches, so each `diff` compares up to 65 shifts x 200 rows, about 13,000 string compares. `Arc::ptr_eq` never hits across snapshots, because `Snapshot::of` allocates new rows each time. The first diff only decides "nothing changed", which a shift-0 equality check with an early exit can answer with the same result. A per-row hash computed once per render would cut the second diff, verifying the text whenever hashes match.
2. **`drive` renders every 100 ms while heavy frames go out every 200 ms.** Frames run at about 4.8/s per session, so about half of these renders are never sent. Each one reads 200 rows under the session lock, which also holds up the PTY reader.
3. **`run` rechecks every 50 ms while waiting out its gap.** That is 24 tasks at up to 20 wakes a second, and each wake takes the global `feeds` lock in `typing`.
4. **JSON is encoded per phone.** Sharing the row encoding is allowed by the brief and looks like a modest saving.

None of these touches encryption, the frame rate or what phones receive. Nothing here is proven to be the main cost until the probes run.

**Not worth doing:** the 65 kB scratch buffer and the `to_vec` in `seal` cost about 1 µs each.

## What I changed, and why it is safe

No product code. Nothing to verify, so the verify commands were not run.

Side effects in `target/`:

- `cargo clean --release -p kelpie-proto -p kelpied`, needed after the worktree build (next section).
- `target/release/examples/seal_bench.exe` remains there. Its source is deleted.
- `target/release/kelpied.exe` is the uninstrumented main-tree build. Its hash matches the copy B2 and B3 ran on (`E61F4F67...`).
- The killed build may have left the `kelpied` fingerprint dirty, so expect the next release build to recompile it (about 11 minutes).

## A hazard in the build recipe

After the worktree build, `cargo build --release -p kelpied` in the main tree failed with E0425: `kelpie_proto::secret_read` not found. Cargo had kept the worktree's `kelpie-proto` rlib as fresh and linked it. Clearing those two crates' release artifacts fixed it. If that crate had not differed, the same staleness could have silently put the old `kelpied` in place. The rebuilt binary has the same size as before, 12,732,928 bytes, though its hash differs from the first build.

## After numbers

None, because no reduction was made. The current figures are the B rows above.

## What was not run

- The instrumented daemon, and therefore any per-section numbers.
- Any reduction, its tests, or a re-measure.
- `cargo test`, clippy, `link_noise.py` and `mobile_protocol.py`, since no code changed.
- `daemon.info` on the rebuilt binary. Its identity rests on the hash match and on B2 and B3 running on the copy.
- A run on a quiet machine. The workload itself keeps the host near 100 %, and other applications were running too, so I could not separate the two.
- `stress.py`, real agents and a real OS prompt, by rule.

## State at the end

- **Disk:** C 2.17 GB, D 3.35 GB.
- **Memory:** 1.6 GB free of 8 GB.
- **Worktree:** `D:\kelpie-perf-ab` removed.
- **Daemon:** the real one is still PID 12168 and untouched. No test daemons remain.
- **Build processes:** none running.
- **Credentials:** no phone-key entries left in the credential store.
- **Scratchpad:** the A binary, the logs and the wrapper are there, in `C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad`. Redoing the A/B would need the worktree added back for the old scripts, with no compile.

## Unresolved questions

1. Should I restart the instrumented build and carry on with the reductions, once memory allows? The probes would have to go back in. The edits are simple to repeat, but a release build takes about 11 minutes and this machine has 8 GB.
2. If reductions do not reach 5 points, should the gate change how it measures? Cycle counts, or worker-thread cycles with no subtraction, are far more stable than ticks with an ingest correction. That is your decision, as is the limit.
3. Why does ingest read lowest in the first with-phones round of a run, while total cycles do not change?
