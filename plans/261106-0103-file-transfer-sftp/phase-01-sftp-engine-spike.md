---
phase: 1
title: "SFTP engine spike and gate"
status: done
priority: P1
effort: "1d"
dependencies: []
---

# Phase 1: SFTP engine spike

## Overview
Decide F1 with measurements before any product code. The spike code is thrown away: a scratch binary on a throwaway branch, never merged. The result is a report in `plans/reports/`.

## Context
- How `ssh` is started today: `program()`, `tool()` and `askpass_env()` in `app/src-tauri/src/ssh_hosts.rs:152-194`. `ssh_base()` in `app/src-tauri/src/remote.rs:86-95` adds `-F`, `NumberOfPasswordPrompts=1`, `ConnectTimeout=15` and `LogLevel=ERROR`.
- `russh_sftp::client::SftpSession::new(stream)` takes any `AsyncRead + AsyncWrite` stream. Its `File` implements AsyncRead, AsyncWrite and AsyncSeek, and needs an explicit `close().await` (report sec 2).
- Unknowns flagged in the report:
  - whether stdio is binary-clean on Windows;
  - how well russh-sftp pipelines;
  - whether `openssh-sftp-client` works on Windows;
  - which OpenSSH version Windows ships inbox;
  - how non-UTF-8 names are handled.

## Requirements
Measure on Windows 10 (inbox OpenSSH), Windows 11 (inbox) and macOS, against a LAN Linux host and the paramiko fixture:
1. Spawn `ssh -F <cfg> -s <alias> sftp` with piped stdin/stdout. Run a 1 GB upload and download, comparing SHA-256 on both ends.
2. Throughput of A1 (`russh-sftp`), A2 (`openssh-sftp-client`, `Sftp::new` on the same child, **no** `openssh` feature) and `sftp` CLI (`-b`, default 64 requests × 32 KB). Report MB/s, with RTT 1 ms and 50 ms (through `scripts/e2e/net_fault_proxy.py`).
3. Resume: open with write flags (no truncate), seek to N, write. Read at offset.
4. A ProxyJump host, a saved-password host (askpass via `SSH_ASKPASS_REQUIRE=force`), an agent-only key (the Windows `ssh-agent` service), and an unknown host key (must fail fast with ssh's message).
5. Extensions offered by the server (`posix-rename@openssh.com`, `statvfs@openssh.com`, `fsync@openssh.com`). Also the rename-over-existing behaviour without posix-rename (paramiko).
6. Names: non-UTF-8 bytes, names with `\`, a name with a newline. Does the crate round-trip them?
7. `ssh -V` on each machine, and whether `scp -3` exists (for the record only).
8. Kill the child mid-transfer. How fast does the session report it? Does `kill_on_drop` leave an orphan `ssh.exe` (Task Manager / `ps`)?
9. Binary size added to `kelpied` (release, stripped) for A1 and A2.

## Gate (decides F1)
- **A1** if it passes 1, 3, 4 and 8, and reaches ≥ 70 % of `sftp` CLI throughput at 50 ms RTT.
- Else **A2** if it passes all of these on Windows.
- Else **B** (`sftp -b` with `reget`/`reput`, progress by polling `stat`), with owner question 1 asked first.
- Non-UTF-8 names that do not round-trip are listed as "name not readable" and skipped with a count. They are not a gate.

## Files
- Create: `plans/reports/spike-<date>-sftp-engine.md` (results table, `ssh -V` per machine, decision).
- Throwaway: `scratch/sftp-spike/` on a branch that is never merged. No repo file changes.

## Steps
1. Scratch crate with three modes (a1, a2, cli). Same child launch as `remote::ssh_base`.
2. Run the matrix and write the report.
3. Record the decision in `plan.md` (F1 row) and in phase 2's "Engine" line.

## Tests
None kept. The scratch measurements are the result.

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| Windows pipes corrupt binary or stall | L × H | A2 and B use the same child, so try them in the same session |
| russh-sftp is too slow (no pipelining) | M × M | A2 is designed after `sftp-client.c`, and the trait keeps the swap cheap |
| Machine is busy with another agent's build | H × L | Run it when the owner says the machine is free. Measurements are relative (same run, same machine) |

**Rollback**: nothing merged.
