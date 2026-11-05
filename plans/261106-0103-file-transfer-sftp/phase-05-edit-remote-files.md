---
phase: 5
title: "Edit remote files: Kelpie editor or an external one, saved back"
status: pending
priority: P2
effort: "2d"
dependencies: [4]
---

# Phase 5: Edit remote files

## Overview
- **Edit in Kelpie**: opens the remote file in Kelpie's own editor, using the remote-project path that already compares before writing.
- **Open with external editor**: downloads to a private temp copy, opens it with the OS default app (or one the person picks), watches it, and uploads on save. The upload checks the server copy has not changed meanwhile.

## Context
- **The editor**:
  - reads through `readFile(project.path, path)` and saves with an mtime check and a conflict prompt (`app/src/components/file-viewer.tsx:157-173,175-189`);
  - for `ssh://` roots the write is the cksum/mtime script with temp and `mv` (`app/src-tauri/src/remote.rs:244-331`);
  - it opens from a project: `store.openFile(projectId, path)` (`app/src/lib/store.ts:1192`);
  - a host's own project is made by `hostProject` (`app/src/lib/ssh-hosts.ts:189-196`).
- **The external route**: the app already has `notify` 8 and `tauri-plugin-opener` 2 (`app/src-tauri/Cargo.toml:19,32`).
- **Termius** downloads to temp, watches, and prompts to upload (report sec 1).

## Requirements
### Edit in Kelpie
1. `hostProject(host)` (made on first use, as *Open as project* does).
2. `store.openFile(projectId, relPath)`. Existing save and conflict behaviour.

This route needs a POSIX shell on the host, like remote projects do. When the host check found none (Windows OpenSSH server, sftp-only account), the item is disabled with "this host has no shell: use an external editor".

### Open with external editor
- **Session** `{id, dest, remote_path, local_copy, base: {size, mtime}, auto}`. The local copy sits in `<data>/sftp-edit/<id>/<name>`: per user, not world-readable, with the name kept so the right app opens.
- **Download** is a normal `xfer.start` job (progress in the queue).
- **Open** with the opener's default, or "Open with…" (the person picks an executable; remembered per extension in the app's settings).
- **Watch** with `notify`, debounced 500 ms, ignoring the editor's own temp and swap files. A save triggers:
  - with `auto` (owner question 2, recommended default on): an upload job with `expect: base`;
  - without `auto`: a toast "Upload changes to <host>?".
- **Upload with `expect`**: the daemon re-checks the server file's `(size, mtime)` against `base` before the rename (phase 3 re-check). On a mismatch the job waits with the question "changed on the server since you opened it", offering *Overwrite*, *Save as a copy* (`name (edited).ext`) or *Discard my upload*. After a successful upload `base` becomes the new stat.
- **Sessions panel** (in the Files tab, above the queue): open edits with host, path and last upload, plus *Stop* (stops watching and deletes the local copy).
- **Cleanup**: sessions stop when the app quits. Copies older than 7 days are removed at app start. Nothing is uploaded at quit without a save event.
- **Binary and huge files**: there is no limit on the external route. "Edit in Kelpie" keeps the editor's limits (`EDIT_LIMIT`).

### Daemon
- `xfer.start` accepts `expect {size, mtime}` for a single-file upload (small additive param, same API level as phase 3 if not yet committed, else the next free level).

## Files
- Create: `app/src-tauri/src/sftp_edit.rs` (sessions, watcher, opener, cleanup); `app/src/lib/remote-edit.ts` and `app/src/lib/remote-edit.test.ts`; `app/src/components/files/edit-sessions.tsx`.
- Modify:
  - `app/src-tauri/src/lib.rs` (commands `sftp_edit_open`, `sftp_edit_stop`, `sftp_edit_list`; managed state; cleanup at start);
  - `crates/kelpied/src/files/queue.rs` and `files/copy.rs` (`expect`);
  - `app/src/components/files/file-pane.tsx` (menu items);
  - `app/src/lib/ipc.ts`, `app/src/locales/vi/*.json`, `docs/user-guide.md`.

## Steps
1. *Edit in Kelpie*: menu item, `hostProject`, `openFile`, and the no-shell check.
2. `expect` in the daemon copy re-check, with tests.
3. `sftp_edit.rs`: download, open, watch, upload, cleanup.
4. Sessions panel and conflict wording. Guide.

## Tests
- `cargo test -p kelpied files::copy`: with `expect`, a matching target is replaced; a changed one (size or mtime) waits; *Save as a copy* names it right.
- `cargo test -p kelpie` (app crate): watcher debounce collapses 3 writes in 200 ms into 1 upload; editor swap files are ignored (`.swp`, `~`, `.tmp`, `4913`); cleanup removes copies older than 7 days and keeps open sessions.
- Vitest `remote-edit.test.ts`: session state and toast choices.
- e2e `sftp_files.py`:
  - **external route**: start an edit session, append to the local copy from the script (standing in for an editor), and check the server file changed; change the server file through `sftp` first, then save locally, and the question appears;
  - **Kelpie route**: open in Kelpie, edit, save, and the server copy changes.

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| An editor saves by rename, so the watcher misses it | M × M | Watch the folder, not the file; match by name |
| Auto-upload pushes a half-written save | L × M | 500 ms debounce plus a size-stable check before the upload |
| Temp copies of secrets stay on disk | M × M | Data dir is per user; deleted on *Stop*, at quit, and after 7 days. Stated in the guide |
| Edit in Kelpie on a host without `sh` | M × L | Disabled with the reason |

**Rollback**: revert. Copies left under `<data>/sftp-edit/` are harmless and can be removed by hand.
