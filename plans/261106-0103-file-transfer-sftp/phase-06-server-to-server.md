---
phase: 6
title: "Server ↔ server copy: direct when A can reach B, else through this computer"
status: pending
priority: P2
effort: "3d"
dependencies: [3, 4]
---

# Phase 6: Server ↔ server

## Overview
With a host in each pane, a copy goes **direct** (A pushes to B with A's own login to B) when that works without lending A the person's keys. Otherwise it **streams through this computer** over two SFTP sessions, with nothing written to this computer's disk. The queue says which route was used and why.

## Context
- **Options** (report sec 3):
  - `scp -3` relays through the client;
  - `ssh A scp … B:` needs A to reach and log in to B;
  - `-A` agent forwarding lets anyone with root (or the same user) on A use the person's loaded keys for as long as the connection lasts (`man ssh`);
  - Termius uses two SFTP sessions (inferred).
- **Kelpie's hosts** may carry `ForwardAgent` (`app/src-tauri/src/ssh_hosts.rs:38`, written into the generated config as `ForwardAgent yes`; see the test at `:1088`). The outer ssh to A must override it.
- **B's address as A sees it** is not the local alias. It comes from the vault (`hostname`, `port`, `user`; `ssh_hosts.rs:23-56`) or the user's config. A host reached through ProxyJump locally is usually not reachable from A, and the probe finds that out.
- **The test fixture** runs commands through Git Bash with its own `KELPIE_TEST_HOME` (`scripts/e2e/ssh_test_server.py:5-12`), so a second instance can play B, and A's home can hold a key for B.

## Requirements
### Route choice per job
`route: auto|relay` (owner question 6; recommended default `auto`).

### Direct probe (`auto` only; cached per (A, B) for 10 min)
- The outer ssh to A always has `-o ForwardAgent=no -o ForwardX11=no -o ClearAllForwardings=yes`.
- On A, run a fixed script with quoted positional values (the `set --` pattern of `remote.rs:244-299`). The script checks that `sftp` and `ssh` exist, then runs `ssh -o BatchMode=yes -o ConnectTimeout=5 -p <port> -l <user> <hostname> true`.
- `<user>` is the user Kelpie logs in to B as: the vault user, else B's `whoami`, read through Kelpie's own session.
- `BatchMode` means A's own known hosts must already trust B's key. An unknown key fails, and nothing is accepted on A.
- A POSIX shell on A is required. Without one (Windows server), the job uses relay.
- If the probe fails, the job uses relay, with a one-line reason:
  - "A has no SSH client";
  - "A cannot reach B (timeout)";
  - "A has no login of its own on B";
  - "A does not know B's host key".

### Planning is the same for both routes
The daemon walks A and stats B through its own SFTP sessions, and resolves conflicts and names with phase 3's rules. Then each file is copied by the chosen route. Conflicts, re-check and rename stay in the daemon, through the B session.

### Direct copy
- Per batch of up to 50 files, run on A: `echo $$; exec sftp -b - -o BatchMode=yes -P <port> <user>@<hostname>`, feeding it `reput "<src>" "<dst>.kelpie-part"` lines. Names are quoted for sftp's batch syntax, and a name with a newline or `"` uses relay for that file.
- `reput` appends, which is the resume.
- Progress: the daemon stats B's partial once a second through its own session.
- Then the re-check, and the rename into place through Kelpie's B session.
- Cancel: `ssh A kill <pid>` (the echoed PID), then remove the partial.
- A dropped ssh to A follows phase 3's backoff and resumes with `reput`.

### Relay copy
- Source = `SftpFs(A)` and sink = `SftpFs(B)` in phase 3's copy, with an 8 MiB buffer. Resume compares tails on both.
- Speed is bounded by this computer's links. The queue shows "through this computer".

### Never
- agent forwarding set by Kelpie;
- writing to B as a different user than Kelpie uses;
- accepting a host key on A;
- staging on this computer's disk.

### UI
- With a host in each pane, the copy menu offers *Copy (Auto)* and *Copy through this computer*. The job line shows "direct A → B" or "through this computer: <reason>".
- `xfer.start` takes `route`. Jobs report `route_used` and `route_reason`.

### API
Next free API level. Older app: the param is ignored and `relay` is the behaviour; the app checks the level before offering Auto.

## Files
- Create: `crates/kelpied/src/files/route.rs` (probe, script, cache, direct batch runner); `scripts/e2e/sftp_host_to_host.py`.
- Modify:
  - `crates/kelpied/src/files/queue.rs`, `files/copy.rs`, `files/pool.rs` (exec on a host: reuse the remote `exec` pattern through `kelpie-proto::ssh` helpers);
  - `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`);
  - `app/src/components/files/files-panes.tsx`, `transfer-queue.tsx`, `app/src/lib/transfers.ts`;
  - `app/src/locales/vi/*.json`, `docs/user-guide.md`, `docs/architecture.md`.

## Steps
1. `route.rs`: the probe script and the parsing of its exit codes into reasons. Unit tests on the generated script (quoting) and on the parse.
2. Relay route (pure reuse of phase-3 copy with two `SftpFs`).
3. Direct batch runner: PID, `reput`, progress by stat, cancel.
4. UI route choice and reasons. Docs, including a plain warning in the guide on why Kelpie does not forward the agent.

## Tests
- `cargo test -p kelpied files::route`:
  - the script quotes `$'…'`-style names safely (property test with odd names);
  - outer args always contain `ForwardAgent=no`, whatever the host's config;
  - probe exit codes map to the reasons;
  - the cache expires;
  - a name with a newline or `"` uses relay;
  - B's user comes from the vault, else `whoami`.
- `cargo test -p kelpied files::copy`: the relay route with two `MemFs` resumes from B's partial.
- e2e `scripts/e2e/sftp_host_to_host.py` (isolated dirs; two `ssh_test_server.py` instances on two ports with separate `KELPIE_TEST_HOME`):
  1. A's home has a key and a known-hosts entry for B, so direct is used and the file arrives with the same hash;
  2. A's home has neither, so relay is used, the reason is "has no login of its own", and the hash is equal;
  3. kill B mid-way and restart it, and the job resumes (both routes);
  4. cancel a direct job: the remote `sftp` is gone (`ps` through A) and there is no partial on B;
  5. a host with `forwardAgent: true` in the vault still gets `ForwardAgent=no` on the outer ssh (check the command line the test seam logs, `KELPIE_E2E` only).

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| Direct writes as another user, or to another host with the same name | L × H | Same user is forced (`-l`). Hostname and port come from the vault. A's known hosts must already trust B (BatchMode) |
| sftp batch quoting bug runs a different command | L × H | Strict quoting with a fallback to relay for any name needing more; property tests |
| Killing the local ssh leaves `sftp` running on A | M × L | PID kill, then a check that it is gone |
| The probe adds latency to each copy | M × L | 10 min cache; at most 5 s timeout; a relay start never waits for a probe on retry |
| The person expected direct to be faster but it is relay | M × L | The reason is shown in one line |

**Rollback**: revert. Jobs fall back to relay only (phase 3 already supports host ↔ host as two `SftpFs` if the UI offers it).
