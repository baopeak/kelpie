# Code review: Claude Yolo on root SSH hosts + regular-user guide

Scope: agents.rs (`ROOT_BYPASS_NOTE`, `without_claude_bypass`, `build_remote_launch` agent branch + tests), remote.rs (`HostCheck.user/root`, `ADD_USER`, `remote_add_user`), ssh-hosts.ts (`agentFolder`, `connectHost` guards), host-target.tsx, welcome/pane-view/new-session-menu callers, store.ts (`confirmRootStart`, `applyLayout` guard), root-user.ts(+test), root-user-dialog.tsx, hosts-page.tsx, styles, vi locale, user-guide. Unrelated in-progress work was ignored.

Checked: the generated shell parses under `sh -n` and runs both branches. Quoting nests correctly for the tmux and direct paths. For codex and other non-Claude agents the only change is a harmless `exec` prefix in the tmux command. Shell, custom and local sessions don't change: `confirmRootStart` returns early when there is no target.

## High

**H1. The guide resets itself whenever the vault changes, so steps 3 and 4 can't be reached.** `app/src/components/root-user-dialog.tsx:24`
`<Show when={hostById(d.hostId)} keyed>` re-renders by object identity (Solid `Show`: keyed means the default `===` memo). `saveVault` (ssh-hosts.ts:120) and `checkHost` → `ssh_host_touch(...).then(setVault)` (ssh-hosts.ts:170) both replace every host object with freshly deserialised ones.
Scenario: step 1 Create succeeds. Step 2 "Add host" calls `saveHost` → `setVault` → Guide remounts, so `added`, `userHost`, `check` and `name` are lost and the Add-host button is disabled again (`!added()`). Running Create again ("already existed") and Add host again ends in `checkHost(saved)` → touch → another remount. The sign-in and Start buttons stay disabled for good. The same thing happens when `copyProject`/`installClaude` reach `hostProject` → `checkHost`, or when any background prefetch runs. Warn also remounts when the touch finishes after `show()` (an uncached host in `confirmRootStart`), which resets the "Don't explain again" checkbox.
Fix: don't key on the host object, e.g. `<Show when={hostById(d.hostId)}>{(host) => d.mode === "warn" ? <Warn host={host()} /> : <Guide host={host()} dialog={d} />}</Show>`. Then add a component test: run Add host while the vault changes and assert the state survives. The real-host verification exercised the shell, not this dialog.

## Medium

**M1. On an existing account, ADD_USER follows symlinks as root.** `app/src-tauri/src/remote.rs:417,420,422`
When `$n` already exists (the "existed" path, which the UI offers on purpose), root works inside a home folder that another user controls:
- If `~n/.ssh/authorized_keys` → `/etc/shadow`, then `cat ... "$k"` (l.420) reads shadow as root. `install -m 600 -o n` then writes it into a file that user owns, so the user can read the password hashes.
- If `~n/.ssh` → `/root/.ssh`, then `install -d -o n` (l.417) chowns root's .ssh to n, and `install` then writes root's authorized_keys owned by n. User n can now add a key and become root. Also, sshd StrictModes then rejects root's own keys and root is locked out.
- System accounts pass `valid_user_name` (`daemon`, `www-data`, `bin`). For example `daemon` (home `/usr/sbin`) gets `/usr/sbin/.ssh` created and owned by daemon. The dialog reports success even though the account's nologin shell means it can never sign in.

Fix: refuse when `$home`, `$home/.ssh` or `$k` is a symlink (`[ -L ]`). Refuse existing accounts with uid below `UID_MIN` (from login.defs, default 1000) or a nologin/false shell. Better still, do the key write as the user (`runuser -u "$n" -- sh -c 'umask 077; cat >> ...'`, feeding root's keys on stdin).

**M2. The key merge can corrupt keys and changes their order.** `remote.rs:420`
- `cat root_keys "$k"` puts no newline between the files. If root's file has no trailing newline, its last key is glued to the existing user's first key, and both stop working. This affects the existing-user path, and I reproduced the gluing locally.
- `sort -u` reorders lines, and sshd uses the first line that matches a key. Lines that start with options (`no-port-forwarding,...,command="echo Please login as ubuntu..."`, as cloud images ship) sort before plain `ssh-...` lines. If the same key appears both restricted and unrestricted, the restricted line now wins for the new user.
- Fix: `{ cat "$HOME/.ssh/authorized_keys"; echo; cat "$k"; echo; } 2>/dev/null | grep -v '^[[:space:]]*$' | awk '!seen[$0]++'`.

**M3. A warning for one host silently cancels a waiting start on another.** `app/src/lib/root-user.ts:25-29,57-61`
`show()` resolves the current `answer(false)` before it replaces the dialog. `applyLayout` (store.ts:967) and the grid start (store.ts:903) start panes with `Promise.all`.
Scenario: a layout has Claude panes on two root hosts. vps1's warning appears and is replaced at once by vps2's. vps1's panes come up as empty panes without the user deciding anything. Fix: queue the warnings (one per host, shown in turn), and only cancel on an explicit user action (guide, Cancel, host removed).

**M4. Hosts page: an agent button on a host with no start folder does nothing visible.** `app/src/components/hosts-page.tsx:269-270`
`open()` calls `store.setOverlay(null)` and then `connectHost`, which now throws "Set a start folder for ...". `guarded` puts that error into the HostDetail that just unmounted, so the user sees the page close and nothing start. Fix: check `isAgentKind(kind) && !host.folder && !linked` before closing the overlay, or show the error with `store.notify`.

**M5. The app ignores IS_SANDBOX, the daemon honours it.** `remote.rs:356`, root-user.ts:56, hosts-page.tsx:329
The daemon keeps the Yolo flags when `IS_SANDBOX=1`, and the docs say so, but the probe only reports uid.
Scenario: on a sandbox host that sets IS_SANDBOX=1 in its profile, the dialog says "This session starts in the normal mode instead: Claude asks before…" while Claude actually runs in Yolo mode. The "logs in as root" chip and its tooltip are equally wrong. That is a misleading security message. Fix: have the probe report `sandbox=${IS_SANDBOX:-}` and set `root` (or a separate `refusesYolo`) only when uid is 0 and sandbox ≠ 1.

## Low

- **L1** `root-user-dialog.tsx:137,151`: `signIn` and `startClaude` close the dialog before awaiting. A later throw (offline, host key) goes to `setError` on a disposed component and the user sees nothing. Close after success, or use `store.notify`.
- **L2** `docs/user-guide.md` step 3 says Kelpie "copies the project … and links the copy" and that each step is "checked". In fact linking is a separate manual button. The copy result is never checked, so linking can point at a copy that failed or at a folder that already existed. `linkProject` (dialog.tsx:131) can also link the user's bare home. Either fix the docs, or enable linking only after the folder is confirmed to exist (e.g. with `remote_folders`).
- **L3** `host-target.tsx:33`: the folder link is saved before the session starts. If the start is cancelled in the root warning or fails, the typed folder still stays linked to the project for good.
- **L4** `rootYoloOk` has no UI to turn it back on, and ids of removed hosts stay in it. `cachedCheck` isn't cleared when a host is edited (`saveHost`), so changing a host's user from root to dev keeps the warning until the host is re-checked.
- **L5** ADD_USER assumes `~/.ssh/authorized_keys`. With a non-default `AuthorizedKeysFile` it fails with "root has no keys", or copies keys to a file sshd doesn't read. `sshd -T` already runs, so `authorizedkeysfile` could be reported as well.
- **L6** `store.ts:952`: a layout with N agent panes on a host with no folder shows N identical toasts.

## Unresolved questions

1. Remote projects (`project.remote` on the root host, the default "vps · root" project from `hostProject`) hide the link and Start steps, which leaves no project hand-off for the most common root flow. Is that intended?
2. If root's `~/.claude/settings.json` has `permissions.defaultMode: "bypassPermissions"`, does Claude still exit as root? Stripping the flags would not help there.
3. Does CodeBuddy (a Claude Code fork, with the same `--dangerously-skip-permissions`) also refuse to run as root?
4. Hosts imported from ssh_config whose `hostname` is a config alias: does `addHost` (dialog.tsx:110-114) produce a host that connects?

Status: DONE_WITH_CONCERNS
Summary: The shell generation and the regressions checked are fine, but the guide dialog's keyed `Show` remounts it on every vault update, which makes the 4-step flow impossible to finish (H1). ADD_USER's existing-user path follows symlinks as root (M1) and its key merge can corrupt keys (M2).
