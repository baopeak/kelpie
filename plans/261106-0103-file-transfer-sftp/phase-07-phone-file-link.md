---
phase: 7
title: "Phone files: daemon verbs, allowed roots, binary data over kelpie-link"
status: pending
priority: P2
effort: "4d"
dependencies: [3, 6, "phone 3, 3a, 3c, 3d, 4 merged", "after phone 7 (shared files, API order)"]
---

# Phase 7: Phone file link (daemon side)

## Overview
New closed-table verbs let an unlocked phone on a secure origin list allowed folders on this computer and allowed SSH hosts, download (*read*) and upload (*write*). File data travels as binary app messages inside the existing encrypted link, with credit-based flow control, resume, a relay budget and an audit line for every file. The daemon serves it with the window closed (F6). **Do not start until phone phases 3, 3a, 3c, 3d and 4 are on master.** Rebase after phone phase 7, which shares `mobile.rs`, `verbs.rs` and the API level.

## Context
- **Verb table**: closed, least permission each (`crates/kelpied/src/mobile/verbs.rs:111-116`); only `hb`, `ack` and `lock_*` pass the lock (`:119-121`); permission is read per message from the record (`docs/architecture.md:412-419`).
- **Link**:
  - JSON messages up to 1 MiB, cut into chunks of ≤ 65 518 bytes, one Noise message each (`crates/kelpie-link/src/lib.rs:55-60`);
  - flags allow only `FLAG_MORE` (`:68,519`);
  - `Wire::open` rejects non-UTF-8 (`crates/kelpied/src/mobile/link.rs:160-171`).
- **Lanes**:
  - the writer serves its own replies first, then the bulk lane (4 × ~64 KiB);
  - 10 s without room closes with `slow_consumer`;
  - no unbounded queue (a test greps for one) (`docs/architecture.md:640-648`).
- **Relay** (phone phase 4): frames ≤ 64 KiB, about 200 messages/s per socket before a close, 4 MB/s burst per room, about 2 GB/month, and padding of control and chat frames to buckets (`plans/261024-1205-phone-remote-control/phase-04-relay-connect-anywhere.md:32-40`). Cloudflare DO messages may be up to 32 MiB (report sec 5), so 64 KiB is our limit, not theirs.
- **Projects**: the daemon knows them as id→name only (`crates/kelpied/src/mobile.rs:381,1344-1346`), so roots need their own setting.
- **Guard**: the app's guard is lexical (`app/src-tauri/src/fs_ops.rs:17-28`), so links can lead out.
- **Audit**: hash-chained; typed text is kept as length plus a truncated SHA-256 (`docs/architecture.md:621-624`).
- **Options and presence**: `APP_ONLY` and `user_presence` (`crates/kelpied/src/auth.rs:21-23,57`); settings through `mobileSecure` (`app/src/lib/ipc.ts:14`).

## Requirements
### Settings (desktop only, no phone verb)
- `mobile.set_files {on, roots: [{id, name, path}], upload_root, hosts: [host_id], relay_file_cap_mb, relay_month_mb, phone_cap_mb: {ios, other}}`, stored in `mobile.v2.json` options, MAC'd.
- **Defaults**: off; `upload_root` = `~/Downloads/Kelpie from phone` (made on first use); no other roots; no hosts; relay 100 MB per file and 1 GB a month; phone caps 200 MB iOS and 1 GB other (owner questions 7, 8, 10, 11).
- **Turning on, adding a root or host, or raising a cap** is in `APP_ONLY` and needs the person. Turning off, removing or lowering does not.
- A root is stored canonicalised. A root that is a drive root, the home folder itself, `<data>` or anything under the Kelpie data dir is refused.

### Who
- `fs_*` verbs need `secure_origin` on the device record and an unlocked connection.
- `read` for `fs_roots`, `fs_list`, `fs_get` and `fs_cancel`; `write` for `fs_put`.
- Refused with `files_off`, `insecure` (plain HTTP) or `locked`.
- None of them pass the lock. Revoke, a downgrade or the lock stop that device's transfers at once; partials are kept for resume unless revoked.

### Verbs (JSON)
- `fs_roots` → `{roots: [{id, name, kind: local|host}], upload_root}`. Hosts show the label only.
- `fs_list {root, path, cursor?}`:
  - at most 500 entries per reply (stays well under 1 MiB, and padded by phase-4 buckets), with `next` as the cursor;
  - entries carry `name`, `kind`, `size` and `mtime` only (no owner, mode or link target);
  - hidden files are listed, as on the desktop.
- `fs_get {root, path, xid, offset?}`:
  - reply `fs_meta {xid, name, size, mtime}`;
  - then data messages;
  - then `fs_done {xid, sha256}`, a SHA-256 of the whole file (for a resumed download the daemon hashes the prefix again from the source);
  - a source changed since (`size`/`mtime`) refuses the resume with `changed`.
- `fs_put {root, dir, name, size, xid, conflict: ask|keep_both|replace}`:
  - reply `{xid, offset}` (offset > 0 resumes this device's own partial of the same `(root, dir, name, size)`);
  - `ask` with an existing target → `exists` (the phone asks the person and sends again with `keep_both` or `replace`);
  - then data messages, then `fs_put_done {xid, sha256}`;
  - the daemon checks the SHA-256 of what it has, the re-check and the rename (phase 3 rules). A mismatch → `corrupt`, and the partial is removed.
- `fs_ack {xid, upto}` grants download credit.
- `fs_cancel {xid}` cancels the device's own transfer only.

### Data message
- Encoding: `0x00 · kind(1) · xid(u32 BE) · offset(u64 BE) · bytes`, with `kind` 1 = data.
- Payload is at most 64 000 bytes, so one message is one Noise message and one relay frame.
- Only to and from a connection whose `caps` include `files`.
- `Wire::open` returns `Opened::Bytes` for a leading `0x00`. Anything else that is not UTF-8 still ends the connection.
- Offsets must be contiguous per `xid`. A gap or overlap is `bad_request` and ends that transfer, not the connection.

### Flow control
- **Download**: at most 16 chunks beyond the last `fs_ack`. The connection gets a **file lane** of 2 messages, served alternately with the bulk lane, so screens stay live and files are not starved. The file lane has no 10 s close. A transfer with no credit for 60 s is paused (resumable).
- **Upload**: the phone sends at most 16 chunks beyond the daemon's `fs_ack`. The read loop hands chunks to the transfer's bounded channel of 16; overflow is a protocol error for that `xid`.
- **Over the relay** (`Peer` relay flag from phone phase 4): data paced to at most 100 messages/s per socket (≈ 6.4 MB/s), below the close threshold, leaving room for screens.
- **Load** (`load.rs` levels): file data paced to half at L2 and paused at L3. Input, answers, heartbeats and revoke are never behind it (they already bypass lanes).
- **Memory**: at most 2 transfers per device and 4 per daemon, so the buffers stay ≤ 8 MiB in all.

### Budgets
- Over the relay: refuse when the size exceeds `relay_file_cap_mb` (`too_big_for_relay`, with the hint to use Tailscale), and count data bytes per calendar month (stored with the options, written at most every 5 s or 1 MB). Past `relay_month_mb` → `relay_budget`.
- `phone_cap_mb` is sent in `fs_roots`, and the phone applies its own platform's cap to downloads (the iOS blob limit, phase 8). The daemon cannot tell iOS from Android, so it enforces only the larger of the two caps.

### Paths
- `root` is an id from `fs_roots`.
- Local: `path` is relative. Refused:
  - absolute paths;
  - drive letters;
  - UNC;
  - `\\?\`;
  - `..`;
  - `:` (alternate data streams);
  - control characters;
  - Windows reserved names.
  Then canonicalise the deepest existing ancestor and require the canonical root prefix, so a link or junction leading out is refused. An upload's `name` passes the same name rules (phase 3 `names.rs`).
- Host: the path is absolute on the host. The server's permissions are the boundary (owner question 8). Uses phase-2 `SftpFs` and phase-3 copy.

### Audit
One line per `fs_get`, `fs_put` (start and outcome) and per first `fs_list` of a root on a connection: device, verb, root id, path, size, SHA-256, result. Never contents.

### Hello
`hello.files {on}`, so the phone shows or hides the tab. `caps` gains `files`.

### API
The phone API level and `API_LEVEL` take the next free values at commit time.

## Files
- Create: `crates/kelpied/src/mobile/files.rs` (verbs, roots guard, budgets, file lane task, audit); `scripts/e2e/mobile_files.py`.
- Modify:
  - `crates/kelpied/src/mobile/verbs.rs` (verbs and tests);
  - `crates/kelpied/src/mobile.rs` (dispatch, file lane in the writer, caps, hello, revoke and lock stop transfers);
  - `crates/kelpied/src/mobile/link.rs` (`Opened::Bytes`);
  - `crates/kelpied/src/mobile/store.rs` (file options with MAC);
  - `crates/kelpied/src/mobile/audit.rs` (line kinds);
  - `crates/kelpied/src/auth.rs` (`mobile.set_files` in `APP_ONLY`);
  - `crates/kelpied/src/daemon.rs`;
  - `crates/kelpie-proto/src/lib.rs`;
  - `scripts/e2e/link_noise.py` (binary data messages);
  - `docs/architecture.md` ("Phone protocol": verbs, data message, lane, budgets).

## Steps
1. Roots guard and name rules, with tests (Windows junction and symlink cases included).
2. Verbs and permissions table, lock, `secure_origin`, and their tests.
3. `Opened::Bytes`, data message codec, file lane in the writer, credit window, pacing, load.
4. Download and upload on top of phase-3 copy (local and host), SHA-256, resume, conflicts.
5. Budgets, options, `mobile.set_files` with presence, audit. Docs.

## Tests
- `cargo test -p kelpied mobile::`:
  - **verb table** still closed: new names parse, other names don't, and permissions are `read`/`write` as listed;
  - **lock**: none pass it;
  - **refusals**: an `insecure` device and `files_off` are refused;
  - **roots guard**: `..`, `C:\x`, `\\srv\s`, `\\?\C:\`, `a:stream`, `CON`, a symlink and a junction leading out, the root itself, deep paths and odd Unicode;
  - **data codec**: round trip; a non-`0x00` non-UTF-8 message still ends the connection;
  - **contiguity**: a gap or overlap ends only that `xid`;
  - **flow**: credit never exceeded; the file lane alternates with bulk; no 4411 from the file lane; pacing over the relay ≤ 100/s;
  - **pause**: L3 pauses;
  - **memory**: limits per device and daemon;
  - **revoke**: stops transfers in < 1 s;
  - **relay limits**: the file cap and month budget, with persisted counters;
  - **SHA-256**: a mismatch is refused and the partial removed;
  - **resume**: a download resume is refused when the source changed; an upload resume returns the partial offset for the same device only;
  - **audit**: lines present and with no content;
  - **presence**: `mobile.set_files` widening refused for `Role::User`;
  - **queues**: `no_phone_path_has_an_unbounded_queue` still passes.
- e2e `scripts/e2e/mobile_files.py` (real daemon, isolated `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR`/`KELPIE_SSH_DIR` set in-process; `link_noise.py` as the phone; paramiko host):
  - a read device lists and gets a 20 MB file (hash equal) and is refused `fs_put`;
  - a write device uploads 10 files to the upload root;
  - path escapes are refused and audited;
  - a plain-HTTP device is refused;
  - files off → `files_off`;
  - drop the socket mid-download through `net_fault_proxy.py`, reconnect, resume from the offset, hash equal; the same for upload;
  - a host root through the paramiko server;
  - while a 100 MB download runs, a followed session's screen frames keep arriving (no starvation);
  - revoke mid-transfer.

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| A path escape on Windows (junction, 8.3 short names, ADS) | M × H | Canonicalise plus prefix; refuse `:` and `~` short-name forms in input; tests for each; root canonicalised when saved |
| File data starves screens or input | M × M | Alternating lanes; input, answers and hb bypass lanes; pacing; L3 pause |
| The relay closes the socket for message rate | M × M | 100/s pacing, under the ~200/s cap, measured in phone 4a |
| Relay budget spent by files | M × M | File cap plus monthly counter; "use Tailscale" hint |
| A copied Noise key downloads files | L × H | Lock required (needs the passkey); secure origin only; audit |
| Merge conflicts with phone branches | H × M | Start only after they are merged; rebase after phone 7 |

**Rollback**: revert. Options fields are dropped by an older daemon on save (MAC'd like other options). Partials in the upload root are hidden dot files.
