# Review: updates keep sessions running (daemon copies)

Date: 2026-10-23 · Branch `rebrand/kelpie` · Read-only review; nothing was installed and the user's installed Kelpie was not touched.

## Verdict

**It holds only for the main path.** On Windows, an in-app update (or a passive `/UPDATE` install) from a release that already runs its daemon from a copy keeps the daemon and its sessions running. I confirmed the ConPTY sideload on a live process. macOS also holds.

**Paths where sessions still stop.** They come back interrupted and resumable; they do not keep running:
- the first upgrade from 0.1.x, which is the user's current install;
- a manual GUI install that keeps the default choice "Uninstall before installing";
- any install where the copy could not be made.

**The handover can also stop live sessions.** `handOverIfIdle` fails open: if the session list fails, it shuts the daemon down anyway. It also fires during short gaps where no session is alive: restart-keeping-conversation, the terminal/chat mode switch, and resuming sessions at startup.

**What to do before release.** Fix H1–H3 and correct the docs. None of the fixes needs a new architecture.

## Evidence run

| Check | Result |
|---|---|
| `pnpm typecheck && pnpm test` | pass (99/99) |
| `pnpm i18n:check` | 0 missing |
| `cargo test -p kelpie daemon_runtime` | 2/2 |
| `cargo test -p kelpied runtimes` | 2/2 |
| `python scripts/e2e/updater.py target\debug\kelpied.exe` | **21/21** |
| Live probe (my own script, isolated endpoint and data dir): copied daemon + shell session | the session's `OpenConsole.exe` ran from `<copy>\OpenConsole.exe`; the daemon loaded `<copy>\conpty.dll` (checked with `Get-Process ...Modules`). No orphaned processes afterwards. |
| kelpied build warnings | 12, all in `choices.rs` (another session's work, out of scope) |

## Q1. ConPTY sideload: verified

- **The daemon loads the copied `conpty.dll`.** portable-pty 0.9.0 `src/win/psuedocon.rs:45-58` checks `kernel32.dll` and then calls `ConPtyFuncs::open("conpty.dll")`. That goes to `shared_library-0.1.9/src/dynamic_library.rs:340`, a bare-name `LoadLibraryW`. Windows searches the application (exe) folder first, and `conpty.dll` is not a KnownDLL. Nothing in `crates/` calls `SetDefaultDllDirectories` or `SetDllDirectory` (grep found none). So the copied daemon loads the copied `conpty.dll`, and the live probe confirms it.
- **`conpty.dll` starts `OpenConsole.exe` from its own folder.** The vendored `conpty.dll` is version 1.25.2609.30003. It contains the strings `OpenConsole.exe`, `x64`/`arm64` (it also tries an arch subfolder) and `conhost.exe` (a fallback). In the probe it started `OpenConsole.exe` from its own folder.
- **Fallback.** If `OpenConsole.exe` is missing, this conpty build falls back to the inbox `conhost.exe`. Sessions still work but with the slow, old Windows 10 host. `proc_tree.rs` already treats `OpenConsole.exe` and `conhost.exe` alike, so nothing breaks functionally.
- **Missing files are copied silently.** `copy_runtime` skips a missing file without an error (`daemon_runtime.rs:54`). That only degrades performance.

## Critical

None.

## High

### H1. Manual GUI upgrade stops every session, but the docs say it doesn't

**Where:** generated `installer.nsi:238-241` and `288-289`. When upgrading, the first radio button is "Uninstall before installing" and it is checked by default. `PageLeaveReinstall` (`:329-356`) then runs the installed version's `uninstall.exe _?=$INSTDIR`. From 0.1.5 on, that uninstaller runs `NSIS_HOOK_PREUNINSTALL` with `-Mode uninstall` (`hooks.nsh:29-32`), which stops every `kelpied` under `$INSTDIR`, the daemon copy included (`stop-daemon.ps1:58`). Sessions come back interrupted.

**Docs that are wrong as a result:**
- README:206: "Installing over a running Kelpie restarts only the app"
- user-guide:22: "running an installer by hand ... works the same way"

**Fix.** An uninstaller started by an installer runs in place with `_?=`, so `$EXEDIR == $INSTDIR`. A normal uninstall first copies itself to `%TEMP%\~nsu*.tmp` and runs from there. In `NSIS_HOOK_PREUNINSTALL`:

```nsis
!macro NSIS_HOOK_PREUNINSTALL
  ${If} "$EXEDIR" == "$INSTDIR"   ; run in place by an installer (reinstall/upgrade)
    !insertmacro KELPIE_STOP_DAEMON install
  ${Else}
    !insertmacro KELPIE_STOP_DAEMON uninstall
  ${EndIf}
!macroend
```

Ship this in the first release that has copies (nothing with copies is published yet: no git tags). Verify it once in Windows Sandbox. Until then, the docs should tell users to pick "Do not uninstall".

### H2. `handOverIfIdle` stops the daemon even when it could not list sessions

**Where:** `store.ts:227-229` runs `await refreshSessions().catch(() => {})` and then `handOverIfIdle()`.

**What goes wrong.** If `session.list` fails (30 s request timeout on a busy daemon, or a disconnect), the error is swallowed. The `sessions` map may then be stale or still `{}` from app start. `daemon.shutdown` is sent, and every live session is interrupted. This is the one failure that defeats the requirement outright.

**Fix.** Decide from a list call that succeeded, and return without handing over if it fails:

```ts
let list: SessionInfo[];
try { list = await ipc.listSessions(); } catch { return; }
setSessions(reconcile(Object.fromEntries(list.map((s) => [s.id, s]))));
if (!list.some((s) => s.alive) && handOverIfIdle()) return;
```

### H3. Handover fires during short "no session alive" gaps

`handOverIfIdle` runs on every alive→dead change and on every `session.closed` (`store.ts:321`, `:336`). Three flows briefly have no live session even though the user is still working:

- `restartKeepingConversation`: kill, wait until dead, then `resumeSession` (`agent-actions.ts:41-48`).
- Terminal↔chat mode switch: `session.kill`, then create (`mode-switch.ts:36-41`).
- Startup: `onConnected` fires `checkDaemonApi` and `resumeInterrupted` together, unawaited (`store.ts:1320-1328`). The first `resumeSession` runs at `setTimeout 0` and can race the shutdown.

If that session is the only one, the shutdown hits the old daemon in the middle of the flow:
- the restart/create call fails, or lands on the old daemon and is interrupted;
- a stale handle reaches the new daemon;
- for about 5 s or more the app shows "not reachable". The new copy loses the lock race while the old one is still persisting (`main.rs:214-220`), and `daemon_client.rs:102-112` only respawns every 5 s.

**Fix:**
1. Debounce the handover: hand over only after about 10 s with no live session and no create/restart/resume in flight, and re-list just before the shutdown.
2. In `onConnected`, `await checkDaemonApi()` first. If it hands over, skip `resumeInterrupted`; the new daemon's connect will resume the sessions.
3. Add a daemon-side atomic `daemon.shutdown {if_idle: true}` that refuses when any session is alive. Today's old daemons won't have it, but every future handover then targets a daemon that does (this release onward), so the race is closed for good.

## Medium

### M1. The first upgrade from 0.1.x does not keep sessions; the docs promise it does

The user's installed Kelpie is legacy: `kelpied.exe` (pid 9900) runs from `%LOCALAPPDATA%\Kelpie\kelpied.exe` and there is no `daemon\` folder. The install hook stops that daemon gracefully, so sessions come back interrupted and agents resume.

That is unavoidable for one transition, but README:206/209 and the user guide should say "from 0.1.5 on". Separately, the ps1 runs the legacy daemon's own CLI (`stop-daemon.ps1:65`). A 0.1.x CLI ignores `--wait` and returns right after the RPC. The script then waits only 3 s (`:67-73`) before `Stop-Process -Force`. On a loaded machine, `rt.shutdown_timeout(2s)` plus `persist(true)` can take longer than that. Writes are atomic, so nothing is torn, but sessions may not be saved as running, which loses their resume. **Fix:** before the 3 s sweep, add `Wait-Process -Id $daemon[0].ProcessId -Timeout 15`.

### M2. The copy path changes on every build, which breaks Windows Firewall rules

Mobile access binds a LAN or Tailscale IP (`mobile.rs:308`). Windows Firewall stores allow-rules per program path. Every update gives `kelpied.exe` a new path (`daemon\<ver>-<mtime>-<len>\`), so after each handover a mobile user gets a new firewall prompt (allowing it on private networks needs admin), and access is blocked until then. Old rules pile up. The same applies to user-set Defender or AV path exclusions; the installed path used to be stable.

**Fix:** use two fixed slots, `daemon\a` and `daemon\b`. Pick the slot that is not running, copy into it, and record the build in a `build.txt` inside it. Paths stay stable, there are at most two firewall rules, and pruning becomes trivial.

### M3. The compatibility contract cannot be enforced, and its window is unbounded

- `plan.md:123-126` and `architecture.md:264-266` say features "check `daemon.info.api` and stay hidden". The app has no such mechanism: `olderDaemon` is a private variable in the store (`store.ts:217`), nothing exposes the daemon's API level, and grep found no feature gate.
- `API_LEVEL` is still 6 even though this change set adds `daemon.set_priority` (diffed against HEAD). The version comparison hides that here, but the contract as written would not.
- A long-lived session (a floating shell, an always-on agent) means handover never happens. App vN may therefore run against a daemon several releases older, possibly indefinitely. That includes daemon-side security fixes (auth, redaction), which do not take effect until the user clicks "Switch now", and that notice is a 15 s toast (`store.ts:230-239`).

**Fix:**
- Expose `daemonApi()` from the store.
- Require an `API_LEVEL` bump for every new RPC, event or field.
- Add a test that every app RPC name exists at the minimum supported API.
- Keep a persistent indicator (status-bar chip) while an older daemon runs.
- Add a manifest flag (e.g. `critical`) that escalates "Switch now".

### M4. Changing `PROTOCOL_VERSION` would leave the app unable to connect

The endpoint name includes `PROTOCOL_VERSION` (`kelpie-proto/src/lib.rs:128-135`), but the lock is per data dir (`main.rs:214-220`). With the old daemon kept running, a future app with a new protocol would find no endpoint, spawn its daemon, and that daemon would exit "already running". The app would retry every 5 s and never connect (no UI) until the old daemon exits. Before, the updater stopped the daemon, so this could not happen.

**Fix:** write it into the contract: never change `PROTOCOL_VERSION`. Or, if it must change, the app also probes the previous endpoint and hands that daemon over. Fix the comment at `lib.rs:21-23` accordingly.

### M5. A real uninstall leaves the copies behind, and the hook comment is wrong

- Tauri's uninstaller deletes only its listed files and a non-recursive `RMDir $INSTDIR` (`installer.nsi:776-796`). "Delete the application data" removes only `%APPDATA%`/`%LOCALAPPDATA%\dev.kelpie.desktop` (`:844-860`).
- Nothing touches `%LOCALAPPDATA%\Kelpie`, so user data survives (good, unchanged), but up to three or more daemon copies (about 9 MB each: kelpied 7.9 MB + OpenConsole 1.1 MB + conpty 0.1 MB) stay on disk.
- `hooks.nsh:30` ("a real uninstall removes the daemon copies too") is false.
- If the user installed somewhere other than `%LOCALAPPDATA%\Kelpie`, the copies are not under `$INSTDIR`. Uninstall mode then misses them, and the copy daemon keeps running after uninstall.

**Fix:** pass the data dir to the ps1 as well (`-DataDir "$LOCALAPPDATA\Kelpie"`). In the real-uninstall branch, add `RMDir /r "$LOCALAPPDATA\Kelpie\daemon"` after the stop, and correct the comment.

### M6. A failed copy silently loses the guarantee

`runnable()` falls back to the installed binary and only logs a warning (`daemon_runtime.rs:26-29`). Causes include disk full, AV quarantine, or a partially pruned folder of the same build (see L5). The next update then stops that daemon. Meanwhile the update dialog still says "Your sessions keep running" (`update-dialog.tsx:34`).

**Fix:** add `exe` to `daemon.info`. When the daemon runs from the install folder, the dialog says sessions will be interrupted, and the next idle handover respawns it from a copy.

## Low

- **L1. `hooks.nsh:17` runs `powershell` from PATH.** Use `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"`; NSIS is 32-bit, so this resolves to SysWOW64, which is fine. Two cases make the ps1 silently do nothing (exit 0 or 1, and only 3 is checked):
  - Constrained Language Mode (AppLocker/WDAC) blocks `New-Object -ComObject`. `$fs` becomes null, `-not $null` is true, and the script exits 0.
  - A GPO `AllSigned` policy overrides `-ExecutionPolicy Bypass`.

  Either is harmless for copies but breaks the legacy path. Fail loudly on unexpected exit codes in install mode.
- **L2. A trailing backslash in `$INSTDIR` breaks the argument quoting.** For a drive-root install (`D:\`), `-InstallDir "$INSTDIR"` becomes `"D:\"` and the `\"` escapes the closing quote. Use `-InstallDir "$INSTDIR."`, or trim in NSIS.
- **L3. Exit 3 strands the user.** It shows a MessageBox (also in passive mode) and then `Abort`s inside the section, so `.onInstSuccess` and its `/R` relaunch never run. After an in-app update the user is left with no app. Exit 3 is only possible with a legacy daemon plus `KELPIE_SESSION_ID` in the installer's environment. The same message ("Run this installer...") is also shown by the uninstaller, where it is misleading.
- **L4. `copy_runtime` reuses an existing copy after checking only its length** (`daemon_runtime.rs:43`). That adds no new trust boundary: same user, and `$INSTDIR` is equally writable. However, `KELPIE_DATA_DIR` is honoured in release builds, so pointing it at a shared folder would let another user plant `kelpied.exe`. Compare the copy byte-for-byte with the installed file before reusing it (about 9 MB, only when spawning).
- **L5. Pruning can fail partway, and a failed folder then looks newest.** `remove_unused` deletes `kelpied.exe` first, then `remove_dir_all`. A partial failure (for example an orphaned `OpenConsole.exe`) leaves a folder without the exe, and its mtime becomes "now", so it counts among the two newest. If that build is needed again, `rename` fails, `exe.is_file()` is false, and M6 applies. Fix: treat a folder without `kelpied.exe` as removable regardless of age, and in `copy_runtime` remove such a folder before copying.
- **L6. `spawn_daemon` copies about 9 MB synchronously on an async runtime task** (`daemon_client.rs:105`). It is rare, but wrap it in `spawn_blocking`.
- **L7. Older copies are still named in transcripts.** Task prompts (`runs.rs:444-447`) put the copy's absolute path into agent transcripts. An agent resumed after two or more later builds will call a pruned path for `run report`. `KEEP=2` mitigates this; the message says so. Acceptable.
- **L8. `pnpm tauri dev` without `KELPIE_ENDPOINT=kelpie-dev` hands over the installed daemon.** The dev version is usually newer than the installed one, so the handover now happens automatically when idle. The installed app then runs on a `target\debug` daemon, which locks the next cargo build. The documented dev setup avoids this; add a guard so handover only targets a daemon running from a `daemon\` copy or the install folder.
- **L9. Tests do not cover all of it.** The e2e does not assert that the session's `OpenConsole.exe` and `conpty.dll` come from the copy (I verified it manually), and does not replace `inst\conpty.dll` or `inst\OpenConsole.exe`. No real NSIS run is covered, by design. Add a Windows Sandbox manual check to the phase file: in-app update, GUI default upgrade, real uninstall.
- **L10. `updater.rs:114`: on Windows the plugin calls `std::process::exit(0)`** (plugin 2.14 `updater.rs:1050`). The store's 400 ms debounced `saveState` may be lost. Flush state before `update_install`.

## Q2. What else runs from the install folder

| Thing | Path at runtime | During/after an install |
|---|---|---|
| Daemon, `conpty.dll`, `OpenConsole.exe` | the copy | unaffected (verified) |
| Claude hooks (`agents.rs:437-441`, shared `claude-hooks.json`) | the copy's `kelpied` (`self.exe`) | unaffected. A new daemon rewrites the file only after the old one is gone (one daemon per lock). |
| Codex `notify` (`agents.rs:384-387`) | the copy, in argv | unaffected |
| Run task prompt (`runs.rs:444`) | the copy | see L7 |
| `SSH_ASKPASS` (`ssh_hosts.rs:128-130`, app-side `daemon_exe()`) | `$INSTDIR\kelpied.exe` | replaced by `File` in place (a tiny window). `askpass.rs` is standalone: it reads `hosts.json` and the keyring and makes no daemon RPC, so a new binary with an old daemon is fine. |
| `kelpie-stt` (`voice.rs:46`) | `$INSTDIR` | owned by the app; the app is closed first, and orphans are force-stopped by the ps1 |
| PATH / app exe in sessions | none (no hits in `crates/kelpied/src`) | n/a |

## Q3. Old daemon with the new app

- **Auth.** The token file is keyed by endpoint (`lib.rs:203`), and the old daemon wrote it, so the user token works.
- **Pre-API-6 daemons.** These are only legacy daemons in `$INSTDIR`, which the hook stops. A copy-era daemon is always API 6 or newer.
- **Attach and frame format.** Unchanged in this diff.
- **The only new RPC, `daemon.set_priority`.** Called with `.catch` (`store.ts:1325`), so harmless.
- **Future risk.** See M3 and M4.

## Q4. The NSIS flow (checked in the generated `installer.nsi`)

- **Hooks are included correctly.** `!include hooks.nsh` is at `:32`, before `MAINBINARYNAME` is defined, which is fine because the macro expands at insertion. `${__FILEDIR__}` is captured in a define, so `File` resolves at compile time. A 0.1.5 setup exe exists, so the hooks compiled. I could not confirm it was built from the latest `hooks.nsh`.
- **Update mode skips the uninstall.** `$UpdateMode = 1` goes to `reinst_done` (`:314-317`).
- **Hook placement.** PREINSTALL runs before Tauri's `CheckIfAppIsRunning` (`:648-652`). Our own call to it runs first, through Restart Manager with `RmShutdown` on `kelpie.exe` only, so the detached daemon is not affected.
- **Section `Abort`.** Ends the install and skips `.onInstSuccess` (see L3).
- **Quoting.** Paths with spaces are fine (the e2e uses `"kelpie-hook inst "`); a trailing backslash is not (L2).
- **Plugin args.** `tauri-plugin-updater` 2.14 passes `/P /UPDATE /R /ARGS` (`updater.rs:1056-1080`), and `.onInstSuccess` relaunches through `RunAsUser` (`:735-746`). Verified.

## Q5. Uninstall

Tauri never recursively deletes `$INSTDIR`, which is the data dir, so user data and copies survive (M5). Hook ordering is right: the hook runs before Tauri's file deletes. The real problem is the reinstall path (H1).

## Q6. Handover races

- Covered in H2 and H3.
- Two app instances: there is no single-instance plugin, so both may call shutdown and both spawn. The same-build `rename` race is handled (`daemon_runtime.rs:63-69`), and the lock leaves one daemon. Harmless.
- SSH and chat sessions count as alive, which is correct.

## Q7. Disk and cleanup

- Copies are about 9 MB each. Pruning keeps the current copy plus the two newest, so roughly 27 MB.
- Local builds bump the version every time, which churns copies but pruning bounds them.
- Folder mtime on NTFS is the time the copy was made (`rename` keeps it), so the ordering is sound except for L5.
- `.tmp-*` folders older than 1 h are removed.
- Copies are only made by release builds. Debug builds and runs with `KELPIE_DAEMON_PATH` don't copy, and e2e runs with `KELPIE_DATA_DIR` copy into their own temporary folder.

## Q8. Security

- **No new privilege boundary.** `%LOCALAPPDATA%\Kelpie` is already the per-user, user-writable install folder. DLL planting next to the copy is the same exposure as `$INSTDIR`.
- **Residual risks.** L4 (`KELPIE_DATA_DIR` pointed at a shared folder), and per-path policy effects such as firewall rules and AV exclusions (M2).

## Q9. macOS

- **The daemon keeps running.** Plugin 2.14 swaps the bundles with an atomic rename (`updater.rs:1486-1560`); its fallback is two renames. The old bundle is unlinked when the temp dir is dropped. The running daemon keeps its inode, nothing is overwritten in place, and there is no code-signature kill.
- **The daemon's exe path now points at the new binary.** `current_exe()` returns the path string, which now resolves to the new binary. So hooks and new sessions on the old daemon run the new `kelpied hook`, a new client talking to an old daemon. That is fine while the hook RPC stays compatible; add it to the M3 contract.
- **Brief missing-path window.** It exists only on the two-rename fallback; a missed hook event there is harmless.

## Phase file `phase-02-auto-update.md`: what to change

| Lines | Currently says | Change to |
|---|---|---|
| 14-16 | Overview: "updating stops running processes" | "updates keep sessions running; only the 0.1.x→0.1.5 transition and *Switch now* interrupt" |
| 38 | "There is no daemon version check" | outdated: `platform_info.version` plus `checkDaemonApi` now compare versions |
| 49 | "inside `$INSTDIR`" | "directly in `$INSTDIR` (install) / under it (uninstall)" |
| 52-54 | Req 2: "Restart daemon" toast | replace with handover-when-idle plus *Switch now* (`kill_sessions:false`) |
| 56-58 | Req 3: `update_install` "stops the daemon ... + wait for its pid" | remove that part |
| 62 | UI: "list of working sessions that will be interrupted" | "sessions keep running" (`update-dialog.tsx`) |
| 102-103 | e2e item 2: "dry install stops the test daemon" | "dry install leaves the daemon running" |
| 104-105 | Manual check: "sessions resume" | "same daemon pid, sessions keep answering" |

Add to the manual checks: a GUI install with default choices (H1) and a real uninstall (M5).

**Success criteria (`:111-112`):** "resumable agents resume" becomes "running sessions keep running through an update (same daemon pid)", and add "a manual upgrade with default choices keeps sessions".

**Risk section:**
- Replace "Daemon fails to stop within the timeout → install aborted" with: copy failure, which falls back to the install folder (M6).
- Add the firewall-per-path risk (M2) and the PROTOCOL_VERSION bump wedge (M4).

**`plan.md` contract (`:123-126`).** Add:
- bump `API_LEVEL` for every RPC, event or field;
- never change `PROTOCOL_VERSION` or the endpoint name without handing over the previous endpoint;
- new `kelpied` CLI/hook binaries must talk to older daemons (macOS);
- expose the daemon API level to features.

## Recommended actions (in order)

1. H1: the `$EXEDIR == $INSTDIR` branch in `NSIS_HOOK_PREUNINSTALL`, before the first copy-era release ships.
2. H2: hand over only after a successful `session.list`.
3. H3: debounced and in-flight-aware handover; await `checkDaemonApi` before `resumeInterrupted`; add `daemon.shutdown {if_idle}`.
4. M1: `Wait-Process` for the legacy daemon; docs say "from 0.1.5 on" and "choose *Do not uninstall* when upgrading by hand" until H1 ships.
5. M2: A/B slots instead of per-build folders.
6. M3, M4: put the contract in code (`daemonApi()`, an `API_LEVEL` bump rule, a persistent older-daemon indicator); document that `PROTOCOL_VERSION` is frozen.
7. M5, M6: clean up copies on a real uninstall and pass the data dir; report the daemon's exe in `daemon.info` and warn in the update dialog.
8. Update the phase-02 and plan files as listed above.

## Unresolved questions

1. Is unbounded old-daemon lifetime acceptable for daemon security fixes, or should a release be able to force (or strongly push) "Switch now"? This is a product decision.
2. Should a real uninstall stop running sessions, or leave the copy daemon running until logoff? Tauri keeps user data either way.
3. Are A/B slots acceptable instead of per-build folders? They affect the `KEEP=2` rationale (older copies named in agent transcripts).

Status: DONE_WITH_CONCERNS
Summary: Updates keep sessions running only on the main path (in-app update on Windows from a release that already runs its daemon from a copy, and macOS); the ConPTY sideload is verified on a live process. Three High issues remain before release: a manual GUI upgrade with the default choice interrupts sessions (H1); `handOverIfIdle` shuts the daemon down even when it could not list sessions (H2); and it also fires during restart, mode switch and startup resume (H3).
