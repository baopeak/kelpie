---
title: "File transfer at Termius level: two-pane SFTP, server-to-server, phone"
description: "A daemon-owned SFTP engine over the user's own OpenSSH, with a transfer queue (progress, speed, cancel, resume), a two-pane Files tab, remote editing, host-to-host copy, and phone browse/upload/download over the encrypted link."
status: pending
priority: P2
effort: "24d"
branch: "one branch per phase off master (files/<name>)"
tags: [sftp, ssh, transfer, files, daemon, phone, relay, security]
created: 2026-11-06
blockedBy: []
blocks: []
---

# File transfer (Termius parity and beyond)

## Overview
Today the Files tab (`app/src/components/hosts-page.tsx:591-703`) lists through a POSIX shell loop over ssh
(`app/src-tauri/src/remote.rs:178-201`), and copies with one blocking `scp -r -p` (`app/src-tauri/src/ssh_hosts.rs:974-1027`, `.output()`
with a 1 h timeout). That gives no progress, no cancel, no queue and no resume. It dies with the window. It needs `sh` on the host.
This plan adds the following:
- one SFTP engine in `kelpied`, spoken over `ssh -F <generated config> -s <alias> sftp`, so the user's config, ProxyJump, agent and saved passwords apply exactly as in terminals;
- a transfer queue that survives the window closing;
- a two-pane Files tab with permissions and remote editing;
- server-to-server copy;
- phone browse, download and upload over kelpie-link.

Research: [Termius SFTP, engines, server-to-server, PWA files, Cloudflare limits](../reports/researcher-261106-0103-termius-sftp-and-engines.md).

## Brainstorm contract
- **Outcome**:
  - The desktop Files tab does what Termius SFTP does: two panes (this computer ↔ host, or host ↔ host), a queue with per-file progress, speed and ETA, pause/cancel/retry, drag and drop, conflict choices, hidden files, sort and filter, and permissions.
  - It also resumes after a drop or a restart, and edits a remote file in Kelpie's editor or an external one with save-back.
  - The phone browses allowed folders on the computer and allowed SSH hosts, downloads to the phone (Save to Files or Photos) and uploads photos and files.
- **Constraints**:
  - Keep the system OpenSSH, the generated `-F` config and the askpass (`ssh_hosts.rs:1-7`, `crates/kelpied/src/askpass.rs:1-19`).
  - Windows first, macOS parity. MIT-compatible dependencies. EN and VI.
  - Persistent work lives in the daemon (AGENTS.md "Ownership").
  - Boundary changes go through protocol, dispatcher, bridge and TS together.
  - Compare before every overwrite. Never log file contents.
  - e2e runs with an isolated `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR`/`KELPIE_SSH_DIR` set in the script's own process.
- **Non-goals**:
  - chown;
  - creating symlinks;
  - sync or mirroring (rsync-like two-way);
  - FTP, S3 and cloud drives;
  - transfers to hosts that are not saved or in the user's SSH config;
  - agent forwarding set by Kelpie;
  - deleting, renaming or making folders from the phone (not asked for);
  - dragging out of Kelpie to Explorer or Finder (no Tauri support in the tree).

## Architecture decisions
| # | Decision | Evidence / why |
|---|---|---|
| F1 | **Engine A1**: spawn `ssh -F <cfg> -s <alias> sftp` and speak SFTP v3 over its stdio with `russh-sftp` (Apache-2.0, 3.0.1), behind a small `RemoteFs` trait. A2 (`openssh-sftp-client`, MIT) is the swap if throughput falls below the spike gate. B (`sftp -b`) is the emergency fallback (no progress in batch mode). C (libssh2) and D (full `russh`) are rejected. | `SftpSession::new` takes any AsyncRead+AsyncWrite (report sec 2). C and D re-implement config, ProxyJump, IdentityAgent and askpass, so a host that works in the terminal could fail in Files. The Windows `sftp.exe` itself drives `ssh.exe` over pipes [inference; spike confirms]. **Decided 2026-11-06 by the owner: A1** after the spike and its upload diagnosis ([report](../reports/spike-261106-sftp-engine.md)). Kelpie's side is cleared: 767 MB/s in process and 181–258 MB/s on the daemon's own process pipes. The remaining upload gap of about 2× against the `sftp` CLI was seen only against the Python test server, which is most likely its cause. A2 is out: its streaming download does not pipeline. **Follow-up**: if a real OpenSSH server still shows A1 uploading about 2× slower than the `sftp` CLI, investigate again. |
| F2 | **The daemon owns sessions and transfers** (`crates/kelpied/src/files/`). The desktop calls `sftp.*`, `local.list` and `xfer.*` through `daemon()` (`app/src/lib/ipc.ts:7-8`). Progress comes as `xfer.updated` events (`daemon_client.rs:180-181`). | Survives the window closing (AGENTS.md ownership). The phone reaches hosts with the app closed. One engine serves both clients (DRY). The daemon already runs `ssh -F` (`crates/kelpied/src/agents.rs:422`, `runs.rs:126-157`) and is the askpass (`main.rs:106-107`). |
| F3 | **Shared ssh launch helpers move to `kelpie-proto`**: `program`, `askpass_env(exe, host_id)`, `config_path`, `host_id_for_alias`, destination validation. App and daemon both use them. | Today they live only in the app (`ssh_hosts.rs:106-109,152-194`). Destination rules are duplicated in `runs.rs:140-151` and `remote.rs:45-58`. |
| F4 | **Resume = temp file and rename.** Data goes to `.<name>.kelpie-part` beside the target. A resume continues from its size after a 64 KiB tail comparison. The rename into place happens only after an explicit conflict answer and a re-check that the target's size and mtime are still what was approved. | Same rule as the editor's compare-before-write (`remote.rs:244-331`, `file-viewer.tsx:157-173`). SFTP v3 has no compare-and-swap, so this is best effort, and the docs say so. |
| F5 | **Server ↔ server: Auto.** Copy *direct* (A pushes to B with A's own credentials) when a probe shows A can log in to B **as the same user Kelpie uses for B**, with Kelpie's ssh to A forced to `ForwardAgent=no`. Otherwise stream *through this computer* with two SFTP sessions (nothing staged on disk). Kelpie never turns on agent forwarding. | `scp -3` relays through the client; `ssh A scp … B:` needs A→B reachability and auth; `-A` lets root on A use the user's keys (report sec 3, `man ssh`). The baseline (relay) is always safe. Direct without forwarding exposes nothing new. |
| F6 | **Phone files go through the daemon, not the phase-7 provider channel.** | The provider fails when the window is closed (`crates/kelpied/src/provider.rs:87`, phase7 worktree) and is request/response with a 10 s limit (`provider.rs:22`). F2 already puts SFTP in the daemon. |
| F7 | **Phone data travels as binary app messages**: a leading `0x00` byte (JSON never starts with it), then `xid u32 · offset u64 · bytes`. Each is ≤ 64 000 bytes, so one message is one Noise message (≤ 65 535) and one relay frame (≤ 64 KiB). Sent only to a connection whose `caps` lists `files`. Credit window of 16 chunks (≈1 MiB). The file lane takes turns with the bulk lane. | `Wire::open` accepts only UTF-8 (`crates/kelpied/src/mobile/link.rs:160-171`). The crate's flags allow only `FLAG_MORE` (`crates/kelpie-link/src/lib.rs:68,519`), so this stays out of the crate. Base64 in JSON would cost +33 % of a 2 GB/month relay budget (`phase-04-relay-connect-anywhere.md:33`). |
| F8 | **Phone roots on the computer are explicit.** Default: one upload folder. The owner adds folders in Settings → Mobile, and hosts are opt-in per host. The guard canonicalises the path and checks the prefix (links that lead outside are refused). Everything sits behind a switch that is off by default; turning it on or widening it needs `Role::App` and the person. | The existing guard is lexical only (`app/src-tauri/src/fs_ops.rs:17-28`). The daemon knows projects only as id→name (`crates/kelpied/src/mobile.rs:381,1344-1346`). Widening reach needs presence (D18 in the phone plan, `auth.rs:57`). |
| F9 | **Files need a secure-origin, unlocked device**: *read* to list and download, *write* to upload, never over plain `http://`. | A swappable `http://` page reading files is exfiltration. Lock and verbs follow `verbs.rs:111-121`. |

## Data flows
```
Desktop pane ──daemon("sftp.list"|"local.list")──▶ kelpied files::pool ──ssh -F cfg -s alias sftp──▶ host
Desktop drop/copy ──daemon("xfer.start", {src, dst, items, conflict})──▶ files::queue (journal <data>/transfers.json)
   └─ worker: Local|Sftp RemoteFs read → bounded pipe → RemoteFs write .kelpie-part → re-check → rename → setstat
   └─ broadcast xfer.updated (≤ 4/s per job, latest state) ──daemon-event──▶ transfer-queue.tsx
Host A ↔ host B: plan in daemon (walk A, stat B) → route direct: ssh A 'sftp -b - user@B' (progress = stat of B's .kelpie-part)
                                                    route relay: SFTP(A) read → 8 MiB pipe → SFTP(B) write
Phone ──link {fs_roots|fs_list|fs_get|fs_put} (JSON) + 0x00 data messages──▶ mobile::files (roots guard, audit, budgets)
   └─ same files::RemoteFs / local guard; uploads → .kelpie-part → sha256 check → rename
```

## Phases
Strictly sequential. Desktop first, then server ↔ server, then the phone (owner's order). The phases share `crates/kelpied/src/daemon.rs`, `kelpie-proto/src/lib.rs` (`API_LEVEL`) and `hosts-page.tsx`, so there is no parallel work.

| # | Phase | Effort | Depends on | API | Status |
|---|---|---|---|---|---|
| 1 | [SFTP engine spike and gate](phase-01-sftp-engine-spike.md) | 1d | none | none | done: A1 ([report](../reports/spike-261106-sftp-engine.md)) |
| 2 | [Daemon SFTP sessions, local listing, shared ssh helpers](phase-02-daemon-sftp-sessions.md) | 3d | 1 | next free | written, review fixed for 2–3; not yet built |
| 3 | [Transfer queue: progress, cancel, conflicts, resume](phase-03-transfer-queue-resume.md) | 4d | 2 | next free | written, review fixed for 2–3; not yet built |
| 4 | [Two-pane Files tab, queue panel, permissions](phase-04-two-pane-files-ui.md) | 4d | 3 | none | written, review fixed for 2–3; not yet built |
| 5 | [Edit remote files (Kelpie editor, external editor)](phase-05-edit-remote-files.md) | 2d | 4 | none | written, review fixed for 2–3; not yet built |
| 6 | [Server ↔ server copy (direct or through this computer)](phase-06-server-to-server.md) | 3d | 3, 4 | next free | written, review fixed for 2–3; not yet built |
| 7 | [Phone files: daemon verbs, roots, binary link data](phase-07-phone-file-link.md) | 4d | 3, 6; **phone plan 3–3d, 4 merged**; after phone 7 | next free + phone caps | pending |
| 8 | [Phone files UI and desktop settings](phase-08-phone-files-ui.md) | 3d | 7 | none | pending |

**Phone dependency**:
- Phases 7–8 need these phone-plan phases merged on master:
  - 3: link, device store, permissions;
  - 3a: HTTPS, since `navigator.share` and `crypto` need a secure context;
  - 3c: the app lock, because file verbs must not pass it;
  - 3d: `option-row.tsx`;
  - 4: the relay `Peer`, its limits and its padding.
- Phone phase 7 is not needed functionally (F6). Phases 7–8 still come after it, because both edit `mobile.rs`, `verbs.rs` and the API level.
- None of these phone phases are merged yet (`plans/261024-1205-phone-remote-control/plan.md:94-100`). Phases 1–6 do not touch mobile code and can land first.

## Security model
- **Who may call**:
  - `sftp.*`, `local.list` and `xfer.*` are for `Role::User` and `Role::App`.
  - Session and claimed roles are refused (they have whitelists, `auth.rs:140-144`), so agents cannot use saved passwords through the daemon.
  - A same-user program can already run ssh itself; out of scope (`auth.rs:25-28`).
- **Destinations**: aliases and `user@host` only, validated with one shared rule (F3). Nothing that starts with `-` and no control characters.
- **Paths**:
  - Remote paths are absolute and free of control characters.
  - Local transfer targets are what the person picked.
  - Phone paths are root-relative, canonicalised and prefix-checked (F8).
  - Names a target OS cannot hold are mapped and reported (phase 3).
- **Overwrites**:
  - Never silent. The default policy is *ask*.
  - "Apply to all" covers one job only.
  - The re-check happens before the rename (F4).
  - A failed read of the journal never becomes an empty journal written back (AGENTS.md).
- **Logs**:
  - Job id, alias, byte counts and error class only.
  - Never contents.
  - File names only in the journal and the phone audit.
  - ssh stderr is kept at most 16 KiB per session, shown to the user and never logged at info.
- **Server ↔ server**: `-o ForwardAgent=no -o ForwardX11=no` on the ssh to A. The direct route logs in as the same user and with A's own known hosts in batch mode, so an unknown B key fails instead of being accepted.
- **Phone**:
  - Switch off by default.
  - Roots and hosts are explicit.
  - Secure origin and unlocked only.
  - *read* for list/get, *write* for put.
  - Hash-chained audit (`docs/architecture.md:621-624`) with path, size and SHA-256, never contents.
  - Relay byte budget and per-file cap.

## Backwards compatibility
- `ssh_transfer` (scp) and its TS wrapper are removed in phase 4. The only callers are `hosts-page.tsx:635,640,661` and `scripts/e2e/ssh_hosts.py:200,205`, which move to `xfer.*`.
- A daemon below the files API level gets the existing "restart Kelpie's background service" pattern (`app/src/lib/runs.ts:158`), not a silent fallback.
- Remote projects' tree, editor, search and git keep their shell path (`remote.rs`). Only the Files tab and transfers use SFTP.
- New files: `<data>/transfers.json` (journal; versioned, unknown fields kept), and phone file settings inside `mobile.v2.json` (phase 7, MAC'd like the other options).

## Test matrix (summary; details per phase)
| Layer | What |
|---|---|
| Rust unit (`cargo test -p kelpied`, `-p kelpie-proto`) | destination/path validation, Windows name mapping, conflict policy table, resume decision, journal load/save strict, symlink rules, walk limits, route probe parsing, phone verb table/permissions/lock, roots guard (`..`, absolute, `\\?\`, ADS `:`, junction/symlink out), binary frame encode/decode, credit window, budgets, audit lines. `MemFs` implements `RemoteFs` for engine tests. |
| Vitest (`pnpm exec vitest run …`) | queue view model (speed EMA, ETA, state labels), pane selection/sort/filter, conflict dialog state, permission bits ↔ octal, phone files store (chunk reassembly, resume offsets, save-path choice by platform). |
| e2e (`scripts/e2e/sftp_files.py`, `scripts/e2e/mobile_files.py`) | paramiko fixture (`scripts/e2e/ssh_test_server.py:153,233,302`) on two ports, isolated endpoint/data/ssh dirs set in-process, `net_fault_proxy.py` for drops and throttling. |
| Manual | Windows 10 and 11 inbox OpenSSH, macOS, a real Linux host with ProxyJump, a password host, iOS and Android installed PWA (share sheet, picker, size cap). |

## Success criteria
- [ ] Uploading and downloading 1 GB to a LAN host shows live progress, speed and ETA, and runs at ≥ 70 % of `sftp` CLI throughput (phase-1 baseline).
- [ ] Cancel stops within 1 s and leaves no `.kelpie-part` behind. Pause then resume continues from the partial size.
- [ ] Killing the ssh server mid-transfer and starting it again: the job reconnects and resumes on its own within the retry window. After a daemon restart the job shows *Interrupted* with *Resume*.
- [ ] Closing the window does not stop a transfer. Reopening it shows the queue as it is.
- [ ] Conflicts ask (replace, skip, keep both, replace if newer, apply to all). Nothing is overwritten unasked. A target changed after the answer is asked about again.
- [ ] Permissions view and change (octal and rwx, recursive) on a remote file or folder.
- [ ] Edit in Kelpie or in an external editor saves back. A change made on the server meanwhile is reported, not overwritten.
- [ ] Host ↔ host: direct when A can log in to B by itself as the same user, otherwise through this computer with a plain one-line reason. Never with agent forwarding.
- [ ] Phone: browse an allowed folder and an allowed host. Download a 50 MB file and save it to Files/Photos on iOS and to Downloads on Android. Upload 10 photos. A dropped link resumes. A read-only device cannot upload. Plain HTTP gets nothing. A path outside the roots is refused and audited.
- [ ] A host with ProxyJump and a host with a saved password work in Files exactly as in a terminal.
- [ ] EN and VI. User guide and architecture updated. e2e green on Windows and macOS.

## Rollback
- Each phase is its own commit on its own branch, so reverting a phase reverts its methods and UI.
- The journal and the phone options are additive files and fields. Older code ignores them, and the phase-7 option fields are MAC'd and dropped by an older daemon on save, as other options are.
- Reverting phase 4 alone needs `ssh_transfer` back (a revert brings it back).

## Execution rules (from the owner's standing rules)
- One branch per phase off master. Commits are separate per phase. No push unless asked. Each phase takes the next free API level at commit time.
- Full review and fix loop per phase, sequential.
- Test in dev (`scripts/dev-daemon.sh run`). Build installers only when asked.
- Each worktree has its own `CARGO_TARGET_DIR`.
- e2e never touches the real daemon: tests set `KELPIE_ENDPOINT` in their own process, and never loop-launch the installed `kelpied.exe`.
- The main worktree currently has uncommitted edits in `hosts-page.tsx`, `auth.rs`, `store.ts` and `types.ts` (git status 2026-11-06). Rebase phase 4 onto whatever lands first.

## Open questions for the owner (each with a recommendation)
**Decided 2026-11-06**: the owner accepted all 13 recommendations below as written.

1. **Engine fallback if the spike fails on Windows**: should `sftp -b` (no live progress, size polled once a second) be allowed as the last resort? *Recommend yes*, only if both A1 and A2 fail the gate.
2. **External editor save-back**: upload automatically on every save, or ask each time as Termius does? *Recommend automatic* with the compare-before-write check (a conflict always asks), plus a per-session "ask each time" switch.
3. **Resume after a daemon restart**: automatic or a *Resume* button? *Recommend the button*: the person may have changed the source meanwhile, and a restart is often an update.
4. **Symlinks in folder copies**: follow links to files, skip links to folders (reported), never create links. Or recreate links on remote → remote? *Recommend follow files, skip folders, no creation in v1.*
5. **Windows-invalid names on download** (`a:b`, `CON`, trailing dot): rename with `_` and report, or skip? *Recommend rename and report* ("saved as …").
6. **Server ↔ server default**: Auto (direct when A logs in to B by itself, else through this computer), or always through this computer? *Recommend Auto*, with a per-copy "through this computer" choice. Agent forwarding is never offered (*recommend not at all*, even opt-in).
7. **Phone roots**: which folders by default? *Recommend only `~/Downloads/Kelpie from phone`* (made on first use). The owner adds folders; a button adds the open projects' folders. No home or drive roots by default.
8. **Phone access to SSH hosts**: all saved hosts, or opt-in per host? *Recommend opt-in per host*, with no folder limit on the host (the server's own permissions are the boundary; a lexical limit there is not one).
9. **Plain HTTP**: may a plain-`http://` device list or download? *Recommend no*: files need a secure-origin device.
10. **Relay budget for files**: *recommend* a per-file cap of 100 MB and 1 GB a month of the relay's ~2 GB, adjustable, with "use Tailscale for big files" shown. No cap on the Tailscale path except the phone's own.
11. **Phone download size cap** (iOS holds the blob in memory before the share sheet): *recommend 200 MB on iOS and 1 GB on Android* until measured on devices, then tune.
12. **chown and owner names**: show owner/group (from the listing's long name) read-only, no chown? *Recommend yes*: chown needs root on most servers.
13. **Status-bar transfer indicator** (transfers outlive the Hosts page): add one? *Recommend yes*: one small chip, which opens the queue.
