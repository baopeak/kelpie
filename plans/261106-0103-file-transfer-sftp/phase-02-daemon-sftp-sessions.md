---
phase: 2
title: "Daemon SFTP sessions, local listing, shared ssh helpers"
status: pending
priority: P1
effort: "3d"
dependencies: [1]
---

# Phase 2: Daemon SFTP sessions

## Overview
Give `kelpied` an SFTP client per host, spoken over the user's OpenSSH, with browse operations, plus a local listing. Move the ssh launch helpers to `kelpie-proto`, so the app and the daemon start ssh the same way.

## Context
- Helpers to share:
  - `config_path` (`app/src-tauri/src/ssh_hosts.rs:106-109`);
  - `askpass_env` (`:158-166`, whose exe today is `daemon_exe()`, `:152-154`);
  - `program` (`:176-184`, which on Windows prefers `System32\OpenSSH` over Git's ssh);
  - `host_by_alias` (`:169-171`).
  The daemon's askpass reads `hosts.json` itself (`crates/kelpied/src/askpass.rs:10`).
- Destination validation exists twice: `TaskHost::validate` (`crates/kelpied/src/runs.rs:140-151`) and `remote::parse_root` (`app/src-tauri/src/remote.rs:45-58`).
- The `Daemon` struct is one per process (`crates/kelpied/src/daemon.rs:184-203`, built once in `Daemon::new`, `:302`). That is the right lifetime for a session pool.
- The async-reply pattern is in `daemon.rs:1808-1819` (spawn, reply with `{id, result|error}`, `Ok(None)`).
- Calls from the frontend need no new Tauri command: `daemon()` goes through `daemon_call` (`app/src/lib/ipc.ts:7-8`).
- The test fixture already serves SFTP (`scripts/e2e/ssh_test_server.py:153-302`).

## Requirements
- `kelpie-proto::ssh`:
  - `program(name)`;
  - `askpass_env(exe, host_id)`;
  - `config_path()`;
  - `host_id_for_alias(alias)` (reads `hosts.json`, never writes);
  - `valid_dest(dest)`, one rule for `TaskHost::validate` and the app.
  The app's `ssh_hosts` and `remote` call these. No behaviour changes.
- `crates/kelpied/src/files/mod.rs`, with:
  - **`RemoteFs` trait**: `list(dir) -> Vec<Entry>`, `stat`, `lstat`, `readlink`, `realpath`, `mkdir`, `rename(from, to, overwrite)`, `remove_file`, `remove_dir`, `chmod`, `set_times`, `open_read(path, offset)`, `open_write(path, offset, create)`, `statvfs?`.
  - **`Entry`**: `{name, kind: file|dir|link|other, size, mtime, mode, uid, gid, owner?, group?, link_target?, readable_name: bool}`. `owner` and `group` come from the long name, best effort.
  - **Impls**: `SftpFs` (the engine chosen in phase 1), `LocalFs` (tokio fs / `spawn_blocking`) and `MemFs` (tests).
  - **`files/pool.rs`**: one SFTP session per destination for browsing, started on first use.
    - Child: `ssh -F <cfg> -o NumberOfPasswordPrompts=1 -o ConnectTimeout=15 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -o LogLevel=ERROR -s <dest> sftp` with the askpass environment.
    - Idle close after 5 min, and closed at daemon shutdown.
    - stderr is kept (16 KiB) for the error text.
    - Children are `kill_on_drop` and inside the daemon's process-tree handling (`crates/kelpied/src/proc_tree.rs`; check its API).
    - A session that died is restarted on the next call, never mid-call.
  - **Methods** (all async):
    - `sftp.list {dest, path, hidden}`, `sftp.stat`, `sftp.realpath {dest, path}` (`~` and `.` give the login folder);
    - `sftp.mkdir`, `sftp.rename {dest, from, to}` (refuses an existing target), `sftp.chmod {dest, paths, mode, recursive}` (a recursive chmod runs as a queue job once phase 3 exists; here only single paths);
    - `local.list {path, hidden}` (Windows: `""` lists the drives), `local.home`.
  - **Errors**:
    - ssh exit 255 is reported as a connection error (same wording as `remote::exec`, `remote.rs:134-137`);
    - a host-key failure keeps ssh's text, so `isHostKeyError` (`app/src/lib/ssh-hosts.ts:157`) still triggers the fingerprint flow;
    - SFTP status codes become plain messages.
  - **Roles**: `Role::User` and `Role::App` only. Add the methods to the auth tests that keep session roles out (`auth.rs:489-493`).
- `API_LEVEL`: next free at commit time. TS constant `FILES_API`.
- `ipc.ts`/`types.ts`: typed wrappers `sftpList`, `sftpStat`, `sftpRealpath`, `sftpMkdir`, `sftpRename`, `sftpChmod`, `localList`, `localHome`, and the `RemoteEntry` type.
- Paths: remote paths must be absolute after `realpath` and free of control characters; the error says why. A local path must be absolute.

## Backwards compatibility
Additive methods. The app still lists through `remote::list_dir` until phase 4 switches the Files tab. Remote projects are unchanged.

## Files
- Create:
  - `crates/kelpie-proto/src/ssh.rs`;
  - `crates/kelpied/src/files/mod.rs`, `files/pool.rs`, `files/sftp_fs.rs`, `files/local_fs.rs`, `files/mem_fs.rs` (`#[cfg(test)]`).
- Modify:
  - `crates/kelpie-proto/src/lib.rs` (`mod ssh`, `API_LEVEL`);
  - `crates/kelpied/Cargo.toml` (engine crate);
  - `crates/kelpied/src/daemon.rs` (field `files`, dispatch, shutdown);
  - `crates/kelpied/src/auth.rs` (tests only);
  - `crates/kelpied/src/runs.rs` (`validate` uses `valid_dest`);
  - `crates/kelpied/src/main.rs` (`mod files`);
  - `app/src-tauri/src/ssh_hosts.rs`, `app/src-tauri/src/remote.rs` (use the shared helpers);
  - `app/src/lib/ipc.ts`, `app/src/lib/types.ts`;
  - `docs/architecture.md` (new "File transfer" section: engine, pool, roles).
- `Cargo.lock` gains the engine crate.

## Steps
1. `kelpie-proto::ssh` with the moved helpers and tests. Switch the app to it (pure move) and run the existing tests.
2. `RemoteFs`, `LocalFs`, `MemFs`, `Entry`. Unit tests on `MemFs`.
3. `SftpFs` on the chosen engine. Pool with idle close and restart.
4. Dispatcher methods and roles. TS wrappers.
5. Architecture doc section.

## Tests
- `cargo test -p kelpie-proto`: `valid_dest` accepts `alias`, `user@host`, `[::1]` and IPv6 forms, and refuses `-oProxyCommand=x`, spaces, quotes, newlines and an empty value. `askpass_env` keys. `program` prefers System32 on Windows (cfg test).
- `cargo test -p kelpied`:
  - `files::` `Entry` mapping from SFTP attributes (mode → kind, link target), long-name owner parsing (GNU and BSD `ls -l` forms, garbage gives `None`);
  - pool: idle close, restart after child exit (fake child with `MemFs` behind a duplex stream if the engine allows, else trait-level);
  - roles: `sftp.list` refused for `Role::Session`/`Claimed`.
- `pnpm typecheck`.
- e2e (`scripts/e2e/sftp_files.py`, created here, extended later): isolated `KELPIE_ENDPOINT`, `KELPIE_DATA_DIR` and `KELPIE_SSH_DIR` set in the script's process. Start `ssh_test_server.py` with a password, save the host, and call `kelpied call sftp.list` / `sftp.realpath` / `sftp.mkdir` / `sftp.rename` on that endpoint. An unknown host key gives ssh's host-key text.

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| Moving helpers changes which ssh runs (Git Bash vs System32) | L × H | Pure move with the same tests. e2e `ssh_hosts.py` still green |
| Orphan `ssh.exe` after a daemon crash | M × M | `kill_on_drop` plus `proc_tree` handling; checked in phase-1 step 8; e2e checks the process list after cleanup |
| An askpass prompt hangs a session | L × M | `SSH_ASKPASS_REQUIRE=force` plus `NumberOfPasswordPrompts=1`, and an unknown prompt is declined (`askpass.rs:4-6`) |
| Many hosts open many children | L × L | Idle close 5 min; at most 1 browse session per destination |

**Rollback**: revert the commit. The app helpers go back to `ssh_hosts.rs`, and the new methods disappear (the app does not use them yet).
