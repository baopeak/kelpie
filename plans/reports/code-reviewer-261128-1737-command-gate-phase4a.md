# Code review: command gate (commit b33209d7, branch feat/command-gate)

Scope: crates/kelpied/src/{gate.rs, daemon/gate_rpc.rs, main.rs, agents.rs, auth.rs, daemon.rs, daemon/priority_rpc.rs, daemon/resource_rpc.rs, resources.rs, resources/ledger.rs}, docs/{architecture,user-guide}.md, scripts/e2e/command_gate.py. About 860 LOC added. plans/ was not reviewed.

Checks run: `cargo test -j 2 -p kelpied -- gate hook claude_is_told` passed 35 of 35 (shared target, debug off). No files were changed. The worktree also has an uncommitted change to `scripts/e2e/resources_monitor.py` that is not part of this commit.

Outside evidence:
- Claude Code docs (hooks, permissions) were fetched on 2026-11-28.
- The safe-environment-variable list was read from the installed `~/.local/bin/claude.exe` with an mmap search. It contains GOOS, RUST_LOG, NODE_ENV, CI and similar names. **CARGO_BUILD_JOBS is not in it.**

## Overall

The pure decision math is tidy and the hook never prints `permissionDecision`. Lock use is safe: the gate, priorities, sessions and resources locks are each taken alone, at statement level, and are never nested. Deadlock risk was not found.

The defects are in three places:
- the trust boundary: `gate.acquire` is not pinned to the caller's session;
- the string rewrite, which can break or silently not apply to commands;
- how long grants live.

The rewrite also interacts badly with Claude's permission engine, because Claude evaluates permission rules against the rewritten input.

## Critical / High

### H1. `gate.acquire` trusts the caller's `session_id` (High, trust boundary)
- **Where:** auth.rs:192. The method is not in the list whose `session_id` is overwritten with the caller's own id (`hook.report`, `run.report`, `sub.*`, ...). `scope_session_params` does not handle it either. gate_rpc.rs:66 takes `session_id` from the params as given.
- **Failure scenario:** a program in any session, including a restricted one (`OWN` now lists `gate.acquire`), calls `gate.acquire {session_id: "<critical session>", call: "x1", command: "cargo build"}`. It is then judged with that session's weight, its critical floor (only half the system floor) and its reservation; `owners` is built from the claimed id. It can also add any number of grants under another session's name or its own (`call` is free text). Each grant adds weight and pending memory, so for up to 90 s every other session's heavy command is held and then cut to one job.
- **Docs:** architecture.md says "A session may ask for itself only", and the commit and module doc repeat the claim. That is false.
- **Fix:**
  - Add `"gate.acquire"` to the overwrite list at auth.rs:192.
  - Cap the open grants per session (for example 8). Refuse new grants beyond the cap, and refuse duplicates of a `call` that already holds a grant.
  - Add an auth test in which a session asks with another session's id.

### H2. The `CARGO_BUILD_JOBS=N` prefix breaks the person's allow rules: prompts in default mode, denials in dontAsk (High)
- **Where:** gate.rs:224, together with how Claude matches permission rules.
- **Evidence:**
  - Claude's hooks doc: "Claude Code evaluates permission rules ... against the input your hook returns, not the input Claude sent."
  - The permissions doc: "An allow rule won't match past an assignment of any other variable", meaning any variable outside the safe list.
  - The safe list in claude.exe has no CARGO_* entry.
- **Failure scenario:**
  - A person who has `Bash(cargo build:*)` or `Bash(cargo test *)` allowed runs an unattended session in default mode. A throttled build now waits on a permission prompt nobody answers. The grant is held the whole time (see H5).
  - In `dontAsk` mode the changed call is denied outright, so the agent's build never runs.
  - The `-j 8` → `-j 2` and `make all` → `make -j2 all` changes break exact-match rules the same way.
- **Note:** the doc line "your permission prompts and rules apply as always" is therefore wrong for rewritten commands.
- **Fix:**
  - For cargo, pass `-j N` / `--jobs N` right after the subcommand, before any ` -- `. cargo build, check, test, run, bench, doc, install and clippy all accept it, and a `Bash(cargo build *)` prefix rule still matches.
  - Do not rewrite when `permission_mode == "dontAsk"`.
  - State in the docs that the rewritten command is what the rules see.

### H3. The rewrite breaks commands that run behind a wrapper (High, the command fails)
- **Where:** gate.rs:224. `ledger::fingerprint` skips the wrappers `timeout`, `nice`, `nohup`, `command` and `exec` (ledger.rs PREFIXES and the `timeout` branch), but the rewrite inserts the assignment right before the program word.
- **Failure scenario:**
  - `timeout 900 cargo test` becomes `timeout 900 CARGO_BUILD_JOBS=1 cargo test`. GNU timeout then tries to exec a program called `CARGO_BUILD_JOBS=1` and fails: "failed to run command ... No such file or directory", exit 127.
  - `nice cargo build`, `nohup cargo build` and `command cargo build` fail the same way.
  - `exec cargo` is also broken.
  - Only `env`, `sudo` and bash's `time` keyword accept an assignment there.
- **Fix:** the `-j N` argument from H2 avoids this. If the environment form stays, put `CARGO_BUILD_JOBS=N` at the start of the whole simple command, before the wrapper. Add tests for `timeout 600 cargo test` and `nice cargo build`.

### H4. The rewrite lands on the first text match of the program, not on the segment the fingerprint chose (High)
- **Where:** gate.rs:246-268 (`find_word`) and gate.rs:208-214.
- **Root cause:** the fingerprint skips leading trivial segments (`cd`, `echo`, `export`, `set`, `source`), but `find_word` searches the whole string. It accepts the word after `/`, `\` or a quote, it ignores case, and it does not treat a newline as a boundary.
- **Failure scenarios:**
  - `cd vendor/cargo && cargo build` and `cd "D:/x/make" && make` become `cd CARGO_BUILD_JOBS=2 vendor/cargo && ...`. Bash fails with "cd: too many arguments", and `&&` then skips the build.
  - `echo "build with cargo" && cargo build`: the inserted text lands inside the echo string. The real build runs at full parallelism, but the gate charged it at the reduced estimate.
  - `"C:/Program Files/rust/cargo.exe" build`: the walk back stops at the space inside the quotes, giving `"C:/Program CARGO_BUILD_JOBS=2 Files/..."`. The path is broken and the command fails.
  - A multi-line command (`cd foo` then a newline, then `cargo build`), which Claude writes often: `\n` is not a valid "before" byte, so `find_word` returns None. `rewrite` then returns None, but gate_rpc.rs:113-117 still records the grant at the lowered estimate and answers plain `run`. The build runs as written while the gate believes it is small.
- **Fix:**
  - Have `fingerprint` return the byte range of the segment it chose and its program word, using the same quote-aware splitter that includes `\n`. Rewrite only inside that range.
  - Never insert inside quotes.
  - When `rewrite` returns None, charge `as_written` and do not answer as if rewritten (fall back to Run or Hold).
  - Add tests for each case above.

### H5. Grants live on with no time limit while the command is not running (High: other sessions starve)
- **Where:** gate_rpc.rs:117. A grant is removed only by PostToolUse, a turn boundary or the session ending (resource_rpc.rs:157-163, priority_rpc.rs:131).
- **Failure scenarios:**
  - **Permission prompt:** PreToolUse runs before the prompt. A `cargo build` waiting for approval, possibly for hours while the person is away (made more likely by H2), counts as running all that time. Every other session's heavy commands are held 90 s and then cut to one job.
  - **Denial or block:** a denied call, a call blocked by another hook, or one the person interrupted with Esc gets no PostToolUse. Stop does not fire on Esc, so the grant stays until the next UserPromptSubmit.
  - **Hook killed during a hold:** if Esc or Claude's timeout kills the hook, the daemon thread does not notice. It keeps holding for up to 90 s and then inserts a grant for a command that never ran. If the person typed in the meantime, the UserPromptSubmit release has already happened, and the grant waits for the following turn boundary.
- **Fix:**
  - Give each grant an expiry: the Bash tool timeout (default 2 min, `timeout_ms` when given) plus a margin. Drop expired grants in `priority_sweep`.
  - Better still, tie a grant to the measured command: release it when the resources tracker shows no process for that call after N s.
  - Stop the hold loop when the client connection closes (check `tx.is_closed()` or the equivalent each iteration).

## Medium

### M1. Two asks at the same moment can both be let through (race between check and insert)
- **Where:** gate_rpc.rs:100-117. Each loop iteration snapshots `running` under the gate lock, releases the lock, decides, and only then inserts.
- **Failure scenario:** two sessions, or a run that starts N tasks, ask within the same second. Each sees the other as not running, and both get the full share and memory. `pending_mb` only counts grants already inserted.
- **Fix:** decide and insert under one gate lock. Read the policy, effective priority and machine first, then take the gate lock, rebuild `running` and `pending`, decide and insert. Holders recheck each second anyway.

### M2. The ledger learns a throttled run under the as-written fingerprint
- **Where:** resources.rs:434. `command_started` is fed by the status hook, which receives the original input; Claude runs matching hooks in parallel.
- **Failure scenario:** `cargo build` is cut to CARGO_BUILD_JOBS=1 and measured at about 1 GB, but the measurement is stored under `jobs: default`. Next time, `estimate()` at gate.rs:117 uses that learned value for the full-parallel build, so it "fits" and runs at all cores. This can overcommit memory: the case the gate exists to prevent.
- **Fix:**
  - At PostToolUse, re-fingerprint from `payload.tool_input.command`, which is the updated input, before learning.
  - Alternatively, have `gate.acquire` tell `Resources` the jobs it granted for `session/call`.

### M3. Holding for cores only delays commands that cannot be rewritten, and inverts priority
- **Where:** gate.rs:180. `cores_fit` fails whenever `jobs > core_share` and the command has a knob. In auto mode, or with a tier's `rewrite` switch off, `may_rewrite` is false, so the result is a Hold.
- **Failure scenario:** in auto mode, every `cargo build` that overlaps another session's heavy command waits the full 90 s. It then runs exactly as written (gate_rpc.rs:158), so the wait bought nothing. A critical session in auto mode waits behind a background build.
- **Fix:** when a command cannot be rewritten, hold only for memory, the disk or the GPU, never for core share.
- **Note:** Claude's hooks doc says a settings command hook can change Bash calls in auto mode without the denial on direct-API sessions. The blanket auto-mode exclusion may therefore be broader than needed. That is the developer's call.

### M4. One unbounded OS thread per `gate.acquire`, each recomputing everything once a second
- **Where:** daemon.rs:2565, gate_rpc.rs:94-108.
- **Failure scenario:** each held request keeps a thread alive for up to 90 s. Once a second it runs `effective_map()` (all sessions), `resources.snapshot()` (JSON) and the policy lock. A looping program, or H1's abuse, creates thousands of threads.
- **Fix:**
  - Cap concurrent holders per session and overall. Answer `run` at the cap, since the gate fails open by design.
  - Or move holders onto a single waiter that is re-evaluated on grant release.

### M5. A session's own grants never count against its own asks
- **Where:** gate_rpc.rs:100 filters out every `"{session}/"` key.
- **Failure scenario:** Claude subagents (Task) share `KELPIE_SESSION_ID`, so two subagents' `cargo build`s in one session both get the full machine share. Pending memory counts them, but the core share and the hold logic do not.
- **Fix:** exclude only the same `call`, not the whole session.

### M6. The flag search crosses `--` and newlines; go and workers append a flag that the existing one then overrides
- **Where:** gate.rs:276-297, gate.rs:233.
- **Failure scenarios:**
  - `cargo run --bin tool -- -j 16`: the program's own `-j 16` is rewritten to `-j 2`. The fingerprint also misreads it as jobs 16. This changes what the agent's program does.
  - `segment_end` ignores `\n`, so a later line's `-j` can be rewritten instead of the target's.
  - `go test -p 4 ./...` becomes `go test -p 2 -p 4 ./...`, where the last flag wins. The rewrite has no effect, but the note says it did. The same happens for `--workers` given only in a later segment.
- **Fix:** stop the search at ` -- ` and at newlines. For GoP, replace an existing `-p N` as the other knobs do.

## Low

- **L1. A no-op rewrite after a timed-out hold** (gate_rpc.rs:155). `decide_or_give_up` returns `Rewrite(jobs 1)` even when the command already runs one job, such as `cargo build -j 1` held for memory, or `make` with no `-j`. The agent is told "Kelpie ran `cargo build -j 1` instead of `cargo build -j 1`". The changed input (`make -j1 all`) also triggers H2. Fix: return Run when `jobs_as_written <= 1`.
- **L2. The hook's doc comment moved** (main.rs:338-344). "Called by agents' hook machinery. Must be fast, print nothing to stdout..." now sits on top of `gate_hook`, which blocks for up to 110 s and prints. `hook()` lost its doc. Move it back.
- **L3. Every Bash call costs a daemon round trip.** Each call spawns the hook, builds a runtime, connects and authenticates, even for `ls`. It runs alongside the status hook, so the added delay is small. Still, the hook is the same binary and could run `ledger::fingerprint` plus `gate::profile(..).heavy` locally and exit for light commands. Building the per-call current-thread runtime is not the cost; process start (and Defender) is.
- **L4. A tokenless caller can release grants.** A Claimed (tokenless) `hook.report` for a session that is not strict can release that session's grants through Stop or PostToolUse (resource_rpc.rs:157-163). This only makes the gate more optimistic, so the impact is low. Consider releasing only on verified reports.
- **L5. Background commands are not covered.** `run_in_background` and in-shell `&` backgrounding are ungated, or released at once on PostToolUse. Claude's foreground-to-background moves end the tool call early too, and eligibility is evaluated on the rewritten input. The docs should say background commands are not covered.
- **L6. Docs accuracy:**
  - "A session may ask for itself only" is false (H1).
  - "rules apply as always" is false for rewritten commands (H2).
  - "then runs with one job" (both docs) holds only with a knob and rewriting allowed; in auto mode it runs as written.
  - `KELPIE_COMMAND_GATE=0` is read from the daemon's environment, not the agent's; the hook still runs.
- **L7. The e2e only checks what the hook prints.** command_gate.py drives the hook with a hand-made payload in PowerShell sessions. It never shows Claude applying `updatedInput` without a `permissionDecision`; the docs only say auto mode can deny it. It also does not cover prompts or allow rules. The commit's "then ran as `CARGO_BUILD_JOBS=1 cargo build`" is the hook's output, not an observed run. A real-Claude check with an allow rule on `cargo build` would have caught H2.

## Checklist

- Concurrency: M1 (race between check and insert), H5 (holder continues after the hook is killed). Lock order is fine.
- Error boundaries: the hook fails open (it prints nothing) on every error path, and the 110 s client timeout is under the 120 s hook timeout. Fine.
- API contracts: H2, H4 and L1 are cases where a command may be changed or charged in a way the caller does not expect.
- Auth/authz: H1.
- Data leaks: notes contain only the command and figures. None found.
- Backwards compatibility: the PreToolUse array gains a second entry; tested in agents.rs. Fine.

## Recommended order

1. H1 (one-line auth fix, plus a cap on grants).
2. Use `cargo <sub> -j N` instead of the environment prefix (fixes H2 and H3 for cargo), and skip the rewrite in dontAsk mode.
3. H4: rewrite only inside the fingerprinted segment's range, and charge as written when the rewrite fails.
4. H5: grant expiry and an abandoned-holder check.
5. M1 through M6, then the Low items and the docs.

## Unresolved questions

- Does Claude Code apply `updatedInput` with no `permissionDecision` in default and acceptEdits modes? The docs imply yes but do not say it. This needs a real-Claude check.
- How do Kelpie's own `updatedInput` and a user's PreToolUse hook that also returns `updatedInput` combine? The hooks run in parallel and the docs define no merge order.

Status: DONE_WITH_CONCERNS
Summary: Five High findings: `gate.acquire` is not pinned to the caller's session; the CARGO_BUILD_JOBS prefix breaks Claude allow rules (prompts in default mode, denials in dontAsk) and commands run behind `timeout`/`nice`/`nohup`; the rewrite can hit the wrong spot (`cd .../cargo &&` fails, newlines silently undercharge); and grants have no expiry, so a command waiting on a permission prompt starves other sessions. The focused tests pass, 35 of 35.
