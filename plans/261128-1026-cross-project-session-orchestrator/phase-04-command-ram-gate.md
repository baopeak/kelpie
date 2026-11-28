# Phase 4: Command gate and parallelism optimizer

Status: **4a implemented** on branch `feat/command-gate`: the gate (`gate.rs`, `daemon/gate_rpc.rs`) and Claude's blocking hook,
reviewed and fixed ([review](../reports/code-reviewer-261128-1737-command-gate-phase4a.md)), proven by
`scripts/e2e/command_gate.py` (8/8). On this 8 GB machine a background build was held 30 s for memory, then given one
job while a critical build ran.

As built, differing from below:
- **No permission decision:** the hook never sends one. It changes the command (`updatedInput`) or adds a note
  (`additionalContext`).
  - A change keeps the command's first words (flags go after the subcommand, never an env prefix), so prefix allow rules still
    match.
  - There are no rewrites in Claude's auto mode (it refuses changed calls) or dontAsk mode.
- **Verified with real Claude Code** (`claude -p`, default mode, a test hook returning only `updatedInput` and
  `additionalContext`):
  - The changed command ran (`echo before` became `echo REWRITTEN`), and the note reached the model.
  - A changed write command with no allow rule (`touch …`) stopped at the approval step and did not run.
  - So the hook never approves anything.
- **Shares:** they use the effective weight without the rank factor of waiting commands; the gate does not queue holds by priority
  among themselves (each re-checks every second).
- **Next (4b):**
  - Codex (its own hook config), and Gemini and Devin through user-level hook entries inert outside Kelpie;
  - chat (ACP) permission steps;
  - OOM signatures bumping the ledger.

## Context

Spikes come from commands *inside* sessions: cargo, pnpm/next build, tests, Playwright/Chrome, docker, PDF, raster and ML jobs, and ffmpeg. One of these is the sign/PDF job that was killed for low memory.

Today only Claude's `PreToolUse` reaches the daemon, and `kelpied hook` (`main.rs:335`) is fire-and-forget: 5 s, no reply. Verified (see the [verification report](../reports/verification-261128-1038-hooks-cache-ttl-fusion.md)): all four CLIs Kelpie runs locally have a blocking pre-tool hook that can deny or rewrite.

| CLI | Event and tool | Rewrite | How Kelpie installs it |
|---|---|---|---|
| Claude | `PreToolUse` / `Bash` | `hookSpecificOutput.updatedInput` | per run, `--settings` (exists) |
| Codex | `PreToolUse` | per its hooks.json | existing Codex hook wiring |
| Gemini | `BeforeTool` / `run_shell_command` | `hookSpecificOutput.tool_input` | user-level `~/.gemini/settings.json` entry with a marker |
| Devin | `PreToolUse` / `exec` | `hookSpecificOutput.updatedInput`; allow = `decision:"approve"` | user-level `%APPDATA%\devin\config.json` entry with a marker |
| Chat (ACP) | `tool_call` `kind:"execute"` | — (gate only) | adapter permission step |

None of them has a per-run config flag for Gemini or Devin. Their entry runs `kelpied hook <agent>`, which exits at once without output when `KELPIE_SESSION_ID` is unset, so it is inert outside Kelpie. The entry is merged without touching the user's other hooks and removed when the setting is turned off. All four fail open on hook error or timeout.

## Requirements

1. **Blocking hook path:**
   - Register a separate pre-tool entry for the shell tool only, with a timeout of 120 s (in seconds or ms per CLI). Other events keep the 5 s fire-and-forget.
   - The hook calls `gate.acquire {session, tool_use_id, command, cwd, tool_timeout}` and blocks on the reply. It prints the decision in that CLI's schema, and prints nothing on any daemon error (fail open).
   - `PostToolUse`/`AfterTool`/command end calls `gate.release` and gives the measured vector to the ledger.
2. **Classify and fingerprint:**
   - Normalize: strip env prefixes, `cd x &&` and pipes.
   - Classify by the dominant resources:

     | Class | Examples |
     |---|---|
     | CPU+RAM | cargo build/test/check/clippy, rustc, tsc -b, next/vite/webpack build, gradle, xcodebuild, go build |
     | Disk | pnpm/npm/yarn install, cargo fetch, docker pull/build, git clone, large copies |
     | Browser | playwright, puppeteer, chrome |
     | GPU | torch/whisper/ocr/diffusion python, `ffmpeg -hwaccel`, blender |
     | Raster/PDF | magick, inkscape, ghostscript, PDF export, pipelines like the sign job |
     | Light | everything else; returns at once with no reservation |

   - The fingerprint is `(project, tool, subcommand, key flags)`.
3. **Estimate a demand vector:**
   - Per fingerprint: EWMA (α 0.3) plus variance of each dimension from the phase-1 ledger. Plan with `est + 1.5·sd`, or the maximum of the last 5.
   - Also record **how the demand scales with parallelism** (RAM and CPU per job), learned from runs at different `-j`.
   - Cold start uses a prior table scaled by repo size (Cargo.lock entries, node_modules size), for example:
     - cargo build: 1 core per job, about 400 MB per job, plus 1 GB to link;
     - next build: 1.5 GB and 2 cores;
     - playwright: 0.8 GB per worker;
     - pnpm install: disk heavy;
     - torch: VRAM.

     The priors are placeholders until measured on the 8 GB machine; the PR records the measurements.
   - An OOM signature in the output bumps that dimension by 20 % for next time. Signatures: `out of memory`, `JavaScript heap out of memory`, `signal: 9`/`SIGKILL`, `memory allocation of`, `LLVM ERROR: out of memory`, exit `0xC0000017`, CUDA `out of memory`.
4. **Optimizer** (on by default for every tier, per Q2): fit the most commands at once without interference. Capacity per resource = machine capacity − reserve for critical work − current use by others.
   - **Shares.** CPU cores and RAM are shared by tier weight among the heavy commands running or waiting, using the effective priority from phase 2: critical 4, normal 2, background 1, times a rank factor of 1.0–1.5; a boosted session counts 8. Guaranteed reservations are taken out of capacity first, and only their owner draws on them. A new command's share is `free × w / Σw`, with at least 1 core.
   - **Re-shape to fit the share.** Return the rewritten command when the tool has a parallelism or memory knob:

     | Tool | Knob |
     |---|---|
     | cargo, make, ninja | `-j N` |
     | cargo (environment) | `CARGO_BUILD_JOBS` |
     | go | `-p N` |
     | gradle | `--max-workers=N` |
     | turbo | `--concurrency=N` |
     | jest | `--maxWorkers=N` |
     | vitest | `--maxWorkers=N` |
     | playwright | `--workers=N` |
     | pytest-xdist | `-n N` |
     | Node | `NODE_OPTIONS=--max-old-space-size=…`, kept above the learned peak |

     An explicit knob the agent already set is respected when it fits its share and lowered only when it does not.
   - **Serialize** on a resource that cannot be shared without harm:
     - one heavy disk writer per physical disk while that disk is saturated;
     - GPU jobs while the sum of VRAM would exceed the adapter;
     - only one link-heavy step of very high RAM at a time.
   - **Order** the waiting commands by effective priority, then by "the person is watching this session", then FIFO, with aging of +1 tier per 10 minutes. Aging never lifts anything above a boosted session.
   - **Hold limit.** A hold lasts at most 90 s and never more than half the command's own timeout. After that the command **runs automatically** (Q1), re-shaped to the smallest sensible parallelism, with a note.
   - **No deadlock.** A command always runs when no other heavy command runs. A critical session may use the reserve.
   - **No rewrite at all** for a command without a knob: it is only held or allowed.
5. **Tell the agent and the person:**
   - **The agent** sees the decision note:
     - Claude: `permissionDecisionReason`/`systemMessage`;
     - Gemini: `additionalContext` (AfterTool);
     - Devin: PostToolUse `additionalContext`.

     Example: "Kelpie ran `cargo build -j 2` instead of `cargo build`: 2 of 4 cores are free and session X (critical) is building."
   - **The session row** shows the hold or rewrite, with **Run as written** and **Run now**.
6. Settings:
   - `gate.enabled` (default on);
   - `gate.rewrite` (default on);
   - a per-project "never rewrite" switch.

## Files

- `crates/kelpied/src/main.rs`: `hook` gains the blocking path and an output schema per CLI.
- `agents.rs`: separate Claude entry; Codex; new user-level Gemini and Devin installers with marker, merge and remove.
- New `crates/kelpied/src/gate.rs`, pure and unit-tested:
  - `gate/classify.rs`;
  - `gate/estimate.rs`;
  - `gate/optimize.rs`: shares, knobs, serialization;
  - `gate/priors.rs`.
- `daemon.rs`: RPC `gate.acquire` and `gate.release`.
- `app/src/chat/` ACP permission path.
- Proto and the session row UI.

## Validation

- Unit tests:
  - classifier table, with noisy shapes such as `cd a && FOO=1 cargo test -p x | tee log`;
  - knob rewriting per tool, preserving quoting, existing flags and `&&` chains;
  - shares by tier;
  - serialization on disk and GPU;
  - aging and the hold-timeout fallback;
  - "always run when alone".
- Hook contract tests: per CLI, the printed JSON parses as the documented schema, and nothing is printed on daemon error. The Gemini and Devin installers are idempotent and keep the user's own hooks byte for byte.
- Real-app e2e on an isolated endpoint, with a fake agent that emits real hook payloads and runs a `hog --cpu N --mem M --disk D` helper:
  - two background builds and one critical build: the critical build starts at once;
  - the background builds are re-shaped so that the sum of jobs ≤ cores and RAM fits;
  - the wall time is recorded against running one after another.
- One real Claude session and one real Devin or Gemini session in a scratch repo confirm that the hook is honored, which proves each CLI's schema live.
- Perf: a light command's hook round trip is under 30 ms p95.

## Risk / rollback

- Risk: a rewrite changes behaviour. Only parallelism and heap knobs are changed, every rewrite is logged on the row, and "Run as written" is one click.
- Risk: user-level config files of Gemini and Devin. The entries are marker-scoped, merged atomically and backed up, and removed by the setting.
- Rollback: `gate.enabled=false` returns every hook to fire-and-forget and removes the user-level entries.
