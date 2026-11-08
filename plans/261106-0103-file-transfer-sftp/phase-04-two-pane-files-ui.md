---
phase: 4
title: "Two-pane Files tab, transfer queue panel, permissions"
status: pending
priority: P1
effort: "4d"
dependencies: [3]
---

# Phase 4: Two-pane Files tab

## Overview
Replace the single-list `FileBrowser` (`app/src/components/hosts-page.tsx:591-703`) with Termius-style panes over the daemon's `sftp.*`, `local.list` and `xfer.*`, plus a queue panel, a permissions dialog and a conflict dialog. Then remove `ssh_transfer`.

## Context
Today's tab:
- `run()` blocks every button while one action runs (`hosts-page.tsx:620-631`);
- upload and download are one blocking `transfer()` (`:632-643`);
- drop from Explorer/Finder uses `onDragDropEvent` and a hit test (`:654-664`);
- rename and delete go through remote-project fs commands (`:644-652`);
- a host-key error triggers `onHostKey` (`:613`).

Termius parity list: report sec 1. Local delete goes to the OS trash through the existing `ipc.deleteEntry` (`app/src-tauri/src/fs_ops.rs:325-339`).

## Requirements
### Panes
- Two side by side. Each is *This computer* or any saved host (a picker at the top of the pane). Default: left is this computer (Downloads), right is the host whose tab is open.
- The narrow layout stacks them, with a swap button.

### Pane toolbar
- path bar (editable, Enter to go);
- up, refresh, home (login folder / user home);
- drives on Windows (`local.list ""`);
- show hidden (remembered per pane kind);
- filter box (type to filter);
- new folder.

### List
- Columns: name, size, modified, permissions (`rwxr-xr-x`; remote and macOS local only), owner (remote, read-only).
- Sort by any column, folders first.
- Links show an arrow and their target in the tooltip. A link to a folder opens through `sftp.realpath`.
- Names that cannot be read are shown dimmed and cannot be selected.

### Selection and keys
- Click, Ctrl/Cmd+click, Shift+click range, Ctrl/Cmd+A.
- Enter opens a folder or edits a file (phase 5).
- Backspace goes up, F2 renames, Delete deletes, Tab switches panes, F5 copies to the other pane.

### Copy
- Drag a selection to the other pane, the *Copy →* / *← Copy* buttons, or the context menu. Each starts one `xfer.start` job.
- Dropping from Explorer/Finder onto a host pane uploads; onto a local pane it copies locally. Local → local is out of scope: the drop is refused with "use Explorer/Finder".

### Context menu
- Open; Edit in Kelpie; Open with external editor (both phase 5); Copy to other pane; Download to…; Rename; Delete; Permissions…; Copy path.
- **Delete**: remote asks "no trash on the host" (existing wording, `hosts-page.tsx:650`) and runs as a delete job. Local goes to the trash.

### Permissions dialog (remote; macOS local read-only view)
- rwx grid for owner, group and others, with an octal field kept in sync.
- "Apply to everything inside" for folders, with "x on folders only".
- Shows owner and group. No chown (owner question 12).

### Conflict dialog
- Opens for a job in `waiting`.
- Shows both sides' size and date.
- Choices: Replace / Skip / Keep both / Replace if newer / Skip identical, plus *Apply to all in this transfer*.
- Several waiting jobs are shown one after another, never stacked.

### Queue panel
- At the bottom of the Files tab.
- Each job shows: name (or "12 files"), direction (this computer → host), a progress bar, bytes/total, speed, ETA, and the state in words (`reconnecting in 5 s`, `interrupted`, the failure reason).
- Actions: pause/resume, cancel, retry, reveal (local result), and clear finished.

### Status bar (owner question 13)
A chip "↑↓ 2 · 45 %" while jobs run, which opens the Hosts page Files tab with the queue.

### Too-old daemon
Below `XFER_API` the tab shows "Restart Kelpie's background service to use the file manager" (the `runs.ts:158` pattern).

### Removal
- `ssh_transfer`: `ssh_hosts.rs:974-1027`, its registration in `lib.rs:697`, and the `transfer` wrapper in `ssh-hosts.ts:102`.
- `e2e ssh_hosts.py:200-205` moves to the new methods.

### Text
Every string in EN and VI, and searchable where settings are (not needed: the tab is not a setting).

## Files
- Create:
  - `app/src/components/files/files-panes.tsx`, `file-pane.tsx`, `transfer-queue.tsx`, `permissions-dialog.tsx`, `conflict-dialog.tsx`;
  - `app/src/lib/transfers.ts` (job store fed by `xfer.updated`/`xfer.removed`; formatting of speed and ETA);
  - `app/src/lib/file-pane.ts` (sort, filter, selection, mode ↔ octal);
  - `app/src/lib/transfers.test.ts`, `app/src/lib/file-pane.test.ts`.
- Modify:
  - `app/src/components/hosts-page.tsx` (remove `FileBrowser`, mount `FilesPanes`);
  - `app/src/components/status-bar.tsx`;
  - `app/src/lib/ssh-hosts.ts`;
  - `app/src-tauri/src/ssh_hosts.rs`, `app/src-tauri/src/lib.rs`;
  - `app/src/styles.css`, `app/src/locales/vi/*.json`;
  - `scripts/e2e/ssh_hosts.py`;
  - `docs/user-guide.md` (Files bullet `:862-863` becomes a short subsection).

## Steps
1. `file-pane.ts` and `transfers.ts`, with tests.
2. `file-pane.tsx` over `localList`/`sftpList`. `files-panes.tsx`, with drag between panes and the OS drop.
3. Queue panel, conflict dialog, permissions dialog.
4. Status-bar chip. Too-old daemon message.
5. Remove `ssh_transfer`. Update `ssh_hosts.py`. Guide.

## Tests
- Vitest:
  - sort by each column with folders first, and natural name order;
  - filter;
  - Shift range and Ctrl toggles;
  - mode `0o755` ↔ `rwxr-xr-x` ↔ grid;
  - speed and ETA formatting (0, unknown total, > 1 day);
  - job store applies coalesced events out of order by `updated`;
  - the conflict queue shows one at a time;
  - the chip text.
- `pnpm typecheck && pnpm test && pnpm i18n:check`.
- e2e `scripts/e2e/sftp_files.py` (GUI, `tauri-driver`, isolated dirs):
  - open the Files tab;
  - left local, right host;
  - drag a file to the host and see progress, then done;
  - double-click a folder;
  - toggle hidden;
  - sort by size;
  - a conflict → Keep both;
  - permissions 640 applied (verified by `sftp.stat`);
  - delete remote, then confirm;
  - cancel a throttled job;
  - status-bar chip visible while running.
- `scripts/e2e/ssh_hosts.py` still green with the new calls.

## Risk
| Risk | L × I | Mitigation |
|---|---|---|
| Large folders make the list slow | M × M | Virtualised list past 2 000 rows (as the explorer does; check `explorer.tsx`) |
| The drop hit test is wrong with two panes | M × L | Hit test per pane `ref` (same technique as `:657-661`) |
| Removing `ssh_transfer` breaks an older daemon setup | L × M | The too-old-daemon message; the old tab is not kept (KISS) |
| Uncommitted edits to `hosts-page.tsx` in the main worktree | H × L | Rebase first; this phase replaces only the `FileBrowser` block |

**Rollback**: revert the commit. `ssh_transfer` and the old tab come back. The daemon methods stay, unused.
