---
phase: 3
title: "Transfer queue: progress, speed, cancel, conflicts, resume"
status: pending
priority: P1
effort: "4d"
dependencies: [2]
---

# Phase 3: Transfer queue

## Overview
A daemon-owned queue of transfer jobs between any two `RemoteFs` ends (this computer ↔ host here; host ↔ host in phase 6). Each job has live progress, speed, ETA, pause, cancel and retry, conflict questions, and resume after a dropped connection or a daemon restart. It replaces `scp -r -p` (`app/src-tauri/src/ssh_hosts.rs:974-1027`), which waits on `.output()` with a 1 h cap and has none of these.

## Context
- Compare-before-write today:
  - remote writes check mtime and/or `cksum`, write to a temp file and `mv` (`app/src-tauri/src/remote.rs:244-331`);
  - the editor asks before overwriting a changed file (`app/src/components/file-viewer.tsx:157-173`);
  - the vault refuses to write over a file it could not read (`ssh_hosts.rs:127-149`). The journal follows the same rule.
- Events reach the window as `daemon-event` (`app/src-tauri/src/daemon_client.rs:180-181`), and TS listens the way `runs.ts:226` does.
- Huge folders today: `remote::list_dir` runs `wc -c` per file in one shell loop (`remote.rs:182-185`). The queue walks over SFTP instead.

## Requirements
### Model
- **Job** `{id, kind: copy|delete|chmod, src: End, dst: End?, items: [path], dst_dir, conflict: ask|replace|skip|keep_both|newer, preserve: true, state, totals {files, bytes, done_files, done_bytes, skipped, failed}, speed_bps, eta_s, current?, question?, error?, created, updated}`.
- **End** = `{local}` or `{dest}`.
- **States**: `queued`, `scanning`, `running`, `waiting` (a question), `paused`, `reconnecting {attempt, next_s}`, `interrupted` (daemon restarted), `failed`, `done`, `cancelled`.

### Scheduling
- At most 3 jobs run at once; the rest are FIFO.
- A job copies up to 4 files at once. Files ≥ 8 MiB go one at a time per job.
- Each running job opens its own transfer session per destination (from the pool, separate from the browse session), so a listing never queues behind a bulk transfer.

### Walk
- Streamed: directories are queued and files are fed to the workers as they are found. Totals grow while `scanning` and are final at the end.
- A cancel stops the walk. There is no tree in memory beyond the directory queue.
- A job with more than 10 000 files keeps per-file detail only for failures and skips.

### Copy one file
1. Open the source. `stat` it (size, mtime, mode).
2. Target exists?
   - With `ask`, the item waits for a question. Other items go on.
   - `replace`/`newer` record the target's `(size, mtime)` as approved.
   - `keep_both` picks `name (2).ext`.
   - `skip` counts it as skipped.
3. Write to `.<name>.kelpie-part` in the target folder.
   - If a partial exists and the job owns it (journal), compare the last 64 KiB with the source at the same offset. Equal: continue from its size. Else start from 0.
4. Pipelined read and write with an 8 MiB bounded buffer. Progress is counted on bytes written.
5. Then:
   - `fsync` where the extension exists;
   - size check;
   - **re-check** that the target is still absent or still `(size, mtime)` as approved, else ask again;
   - rename into place (`posix-rename` where offered; otherwise remove and rename, only after an approved replace; the window is documented);
   - `set_times`/`chmod` from the source when `preserve` is on (local Windows gets times only).
6. Cancel removes the partial. Pause keeps it.

### Conflicts
- **Answer**: `xfer.answer {id, choice, all}`. With `all`, the choice is applied to the rest of this job only.
- **Folders** merge.
- **File vs folder** of the same name: never replaced. Only skip or keep both.
- **Identical** (same size and mtime): offered as "skip identical".

### Resume
- **Dropped session** (ssh exit, SFTP EOF, 60 s with no progress):
  - Retry with backoff 2, 5, 15, 30, 60 s for up to 5 min, resuming from the partials. Then `failed` with *Retry*.
  - On resume, a folder job walks again and skips files already complete (same size and mtime, which `preserve` set).
- **Daemon restart**: running and paused jobs load as `interrupted`, and *Resume* continues as above (owner question 3).

### Symlinks (owner question 4)
- An explicitly picked link copies its target.
- Inside a folder: links to files are followed; links to folders are skipped and counted ("links to folders not followed"); a loop is impossible since folder links are never followed.
- Local junctions and reparse points count as folder links.
- Links are never created.

### Names (owner question 5)
- **Download to Windows**: map `<>:"\|?*`, controls, trailing dot or space, and `CON`/`PRN`/`AUX`/`NUL`/`COM1-9`/`LPT1-9` (with any extension) by replacing with `_` or appending `_`, and report "saved as".
- **Case-insensitive targets** (Windows, default macOS): names that differ only in case get keep-both naming.
- **Long paths**: check that std handles > 260 characters on Windows (spike).
- **Unreadable (non-UTF-8) remote names**: skipped and counted.

### Space
Check the free space locally (`GetDiskFreeSpaceExW`/`statvfs`), and remotely via `statvfs@openssh.com` when offered. Not enough space fails before the copy starts.

### Delete and chmod jobs
- Recursive delete over SFTP (walk, children first) with progress and cancel. A local delete stays the app's trash (`fs_ops.rs:325-339`), so this is remote only.
- Recursive chmod: files and folders get the same bits, except that the "x for folders only" option keeps folders searchable.

### Methods and events
- **Methods**:
  - `xfer.start {kind, src, dst?, items, dst_dir?, conflict, mode?, recursive?}` returns `{id}`;
  - `xfer.list`;
  - `xfer.control {id, action: pause|resume|cancel|retry|clear}`;
  - `xfer.answer`;
  - `xfer.clear_finished`.
- **Events**:
  - `xfer.updated {job}` at most 4 times a second per job, as the latest state, coalesced. Speed is an EMA over 3 s.
  - `xfer.removed {id}`.

### Journal
- `<data>/transfers.json`: `{version, jobs}` with no per-file lists. Written atomically, at most once a second plus on every state change.
- A file that cannot be read is copied aside and never overwritten with an empty one.
- Finished jobs are kept for 7 days or 200 entries.
- Partials of jobs no longer in the journal are removed at start (local) or on clear (remote).

### Logs
Job id, kind, alias, bytes and error class. Never contents. File names appear only in the journal and in the events.

### API
`API_LEVEL` is the next free one. TS: `XFER_API`, plus wrappers and types.

## Backwards compatibility
Additive. `ssh_transfer` stays until phase 4.

## Files
- Create:
  - `crates/kelpied/src/files/queue.rs` (jobs, scheduler, events);
  - `files/copy.rs` (one file: partial, resume, re-check, rename);
  - `files/walk.rs`;
  - `files/names.rs` (Windows and case mapping);
  - `files/journal.rs`.
- Modify:
  - `crates/kelpied/src/files/mod.rs`;
  - `crates/kelpied/src/daemon.rs` (dispatch, load at start, shutdown pauses jobs);
  - `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`);
  - `app/src/lib/ipc.ts`, `app/src/lib/types.ts`;
  - `docs/architecture.md` (queue, resume, conflict and symlink rules).

## Steps
1. `names.rs` and `walk.rs` with `MemFs` tests.
2. `copy.rs`: partial, tail compare, re-check, rename fallback. Tests on `MemFs` with injected failures.
3. `queue.rs`: scheduler, states, backoff, events, answers.
4. `journal.rs`: strict load, atomic save, restart → `interrupted`.
5. Delete and chmod jobs. Dispatcher. TS wrappers. Docs.

## Tests
- `cargo test -p kelpied files::`, all on `MemFs`/`LocalFs` in a temp dir:
  - **conflict table**: every policy × (absent, file, folder, identical, newer, older);
  - **apply to all**: scoped to one job;
  - **re-check**: a target changed after the answer asks again;
  - **resume**: a matching tail continues; a different tail restarts; a source changed since (size or mtime) restarts;
  - **cancel**: removes the partial; pause keeps it;
  - **backoff**: schedule; failure after 5 min;
  - **restart**: running → `interrupted`;
  - **journal**: a damaged file is kept aside and not overwritten;
  - **names**: `CON.txt`, `a:b`, `x.`, and case pairs;
  - **symlinks**: a folder link is skipped and counted, a file link is followed, a picked folder link is followed once;
  - **walk**: 20 000 files keeps memory bounded (counter assertion);
  - **space**: an insufficient check fails before writing;
  - **events**: throttled to ≤ 4/s per job;
  - **no partial without its journal entry**: removed at start.
- `cargo test -p kelpied auth`: `xfer.*` refused for session roles.
- e2e `scripts/e2e/sftp_files.py` (extended):
  - upload and download a 200 MB file through `net_fault_proxy.py` (throttled), with progress events seen and SHA-256 equal;
  - cancel mid-way leaves no partial;
  - pause and resume;
  - kill the paramiko server mid-transfer and start it again on the same port, so the job resumes by itself and the hash is equal;
  - restart the isolated daemon so the job shows `interrupted`, then resume;
  - conflict ask → keep both;
  - a folder of 2 000 small files plus a folder link (skipped and counted).

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| Resume splices wrong bytes | L × H | Tail compare plus a source `(size, mtime)` check. Phone uploads add a whole-file SHA-256 (phase 7) |
| Overwrite race between the re-check and the rename | L × M | Documented as best effort (no SFTP CAS); the window is milliseconds |
| Remove-and-rename without `posix-rename` loses the old file if the rename fails | L × M | Only after an approved replace. Rename the old file to `.kelpie-old` first and remove it after the new one is in place |
| Event flood to the window | L × M | Coalesced latest state, ≤ 4/s per job |
| Journal grows | L × L | 7 days / 200 jobs, no per-file lists |

**Rollback**: revert. The journal file is ignored by older daemons, and partials are hidden dot files the owner can delete.
