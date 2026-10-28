# Code Review: Phase 2 (auto-update + installer safety)

## Scope
- Files: crates/kelpied/src/main.rs (stop --wait), app/src-tauri/{installer/hooks.nsh, tauri.conf.json, tauri.updater.json, Cargo.toml, src/updater.rs, src/lib.rs, src/daemon_client.rs}, app/src/{lib/updater.ts, lib/updater.test.ts, components/update-dialog.tsx, components/status-bar.tsx, components/app.tsx, components/settings/system-settings.tsx, lib/store.ts, lib/types.ts, lib/ipc.ts, lib/settings-catalog.ts, locales/vi}, .github/workflows/release.yml, scripts/release/latest_json.py, scripts/build-release.sh, scripts/e2e/updater.py, docs.
- Focus: phase hunks only (hunks for CPU priority, chat, mascot, echo_bench and typing latency ignored).
- Checks run: `pnpm typecheck` passes. `pnpm test` passes (92/92). `pnpm i18n:check` reports 0 missing. `cargo test -p kelpied waits_for_a_process_to_exit` passes. `cargo clippy -p kelpie --features e2e-updater` reports nothing in updater.rs. `cargo clippy -p kelpied -p kelpie -- -D warnings` **fails**, but only in files outside this phase (choices.rs, cli.rs:73, daemon.rs:492, runs.rs:343, agent_history.rs, and lib.rs:60, which already fails at HEAD). No phase file has a warning.
- Evidence sources: the installer Tauri generated (`target/release/nsis/x64/installer.nsi`, still 0.1.1), tauri-plugin-updater 2.14.0 source, tauri-cli 2.12.3 `build --help`, and the upload-artifact README.

## Overall assessment
The daemon handover itself is sound:
- **Persist order:** `serve()` stops the runtime before calling `persist(true)` (main.rs:234-236).
- **Chat sessions keep `alive`:** chat uses `kill_on_drop(false)`, and `alive` only flips in the read task, which the shutdown drops (chat.rs:123,188).
- **Writes are atomic:** persist.rs:71-77.
- **No respawn race on the in-app path:** `hold_spawn` is set before `daemon.shutdown` (updater.rs:92-94), and the connection loop checks it on every retry (daemon_client.rs:111).
- **Signature checked before anything stops:** `download()` verifies the minisign signature before the daemon is stopped (plugin updater.rs:1893-1917).
- **Overrides compiled out:** they exist only under `cfg(feature = "e2e-updater")`.
- **Manifest is correct:** `latest.json` matches the v2 static format (`windows-x86_64` / `darwin-aarch64`, `url`, `signature`, RFC 3339 `pub_date`).
- **Multiple `--config` flags merge:** tauri-cli 2.12.3 accepts several and merges them in order.

However, two defects block shipping. The release workflow cannot publish. A manual install over a *running app* still hits the "file in use" error this phase exists to fix. The in-app path also leaves the app without a daemon whenever an install fails.

## Critical (blocks shipping)

### C1. The release workflow cannot publish: artifact paths are nested, but publish globs a flat folder
- **Where:** .github/workflows/release.yml:52-57 uploads `nsis/*`, `macos/*` and `dmg/*`. upload-artifact v4 uses "the least common ancestor of all the search paths" as the artifact root (README), which is `target/release/bundle` on both runners.
- **What happens:** after `download-artifact` with `merge-multiple`, the files sit at `dist/nsis/…`, `dist/macos/…` and `dist/dmg/…`.
  - `exe=$(ls dist/*-setup.exe | head -1)` (line 77) fails under the runner's `bash -eo pipefail`, so the job aborts.
  - Even with that fixed, `gh release create … dist/*` (line 89) would be handed directories.
- **Fix:** flatten in the publish job before anything else:
  ```bash
  mkdir -p out && find dist -type f -exec mv {} out/ \;
  ```
  Then use `out/*-setup.exe`, `out/*.app.tar.gz` and `gh release create … out/*`. Alternatively, upload one artifact per file group so each has its own root.

### C2. A manual install over a running Kelpie still fails on `kelpied.exe`: the app restarts the daemon right after the hook stops it
- **Order inside the installer:** the generated `installer.nsi:644-651` runs `NSIS_HOOK_PREINSTALL` first and only then `CheckIfAppIsRunning` (the same order at 767-773 for uninstall).
- **The race:** while the hook runs, the app (Kelpie.exe) is still alive. After a disconnect `spawned_at` is `None` (daemon_client.rs:82), so the connection loop respawns `$INSTDIR\kelpied.exe` within 150 ms (daemon_client.rs:109-118). `hold_spawn` is only set by `update_install`, never for a manual install.
  - The PowerShell step kills that respawn at about 3 s.
  - The app then respawns again 5 s after its previous spawn.
- **Interactive installer:** `CheckIfAppIsRunning` shows a MessageBox. While the user reads it, a fresh daemon is running (detached, so it survives the app being killed). `File "kelpied.exe"` (installer.nsi:663) then fails with "Error opening file for writing".
- **Passive installer:** whether it fails depends on timing.
- **Uninstall:** `Delete "$INSTDIR\kelpied.exe"` fails silently and leaves the file behind.
- **Impact:** success criterion 1 fails, and so does the planned manual test ("install 0.1.x over a running 0.1.x with sessions open"). The docs claim this works: user-guide ("Running an installer by hand over a running Kelpie does the same") and README Install.
- **Fix:** close the app first, inside the hook.
  ```nsis
  !macro KELPIE_STOP_DAEMON
    !insertmacro CheckIfAppIsRunning "$INSTDIR\${MAINBINARYNAME}.exe" "${PRODUCTNAME}"
    ...
  ```
  The macro comes from Tauri's utils.nsh, which is already included, and works in both the install and uninstall sections. Tauri's own later call then finds nothing. Add an e2e or manual step that covers "app running".

## High

### H1. A failed install leaves the app permanently without a daemon
- **Where:** updater.rs:92 calls `client.hold_spawn()`, and nothing ever clears it ("until the app exits", daemon_client.rs:253-256).
- **Failure paths after the hold:**
  - `stop_daemon` times out (updater.rs:97-98).
  - `daemon.shutdown` errors (line 94).
  - `update.install(bytes)` fails: on Windows, `ShellExecuteW` ≤ 32 makes the plugin return Err (plugin updater.rs:1047-1049); on macOS, the bundle replace can fail (permissions, translocation).
  - The dry-install branch also returns with the hold still set.
- **Result:** every session disconnects and nothing respawns the daemon. The dialog only shows the raw error and does not tell the user to restart Kelpie.
- **Fix:** add `DaemonClient::release_spawn()` and call it on every error path after `hold_spawn()`, for example with a scope guard around `stop_daemon` + `install`. The connection loop then restarts the (still old) daemon, and sessions come back interrupted. Also word the error to say that sessions resume.

### H2. On macOS the exited daemon is likely still "alive" as an unreaped zombie, so the install times out
- **Where:** `spawn_daemon()` on Unix drops the `Child` without waiting (daemon_client.rs:316-322). A daemon started by this app is the app's child, so after a graceful exit it stays a zombie until reaped.
- **Why it matters:** `pid_alive` (updater.rs:105-110) uses sysinfo, and sysinfo 0.39.6 lists zombies (it maps `SZOMB` to `ProcessStatus::Zombie`, src/unix/apple/process.rs:16). The loop then waits the full 15 s, returns "did not stop", and H1 strands the app.
- **Not verified on macOS**, but the common case (the app started the daemon in the same run) hits it.
- **Fix:** treat `ProcessStatus::Zombie` as gone, or call `libc::waitpid(pid, null, WNOHANG)` first on Unix. The Unix branch of `wait_for_exit` in main.rs:117-129 (`kill(pid, 0)`) has the same blind spot when the app is the parent. That only matters for a manual `kelpied stop --wait` on macOS (Low).

### H3. The hook was never compiled into an installer, and the spec's signed-update e2e is missing
- **Hook not compiled:** step 1 says "verify the hook compiles in a bundle build". The newest generated script is `target/release/nsis/x64/installer.nsi` with `!define VERSION "0.1.1"` and no hook include. There is no `Kelpie_0.1.2_x64-setup.exe`. The e2e test only runs the extracted PowerShell line (updater.py:120-130), not NSIS.
- **Signed-update e2e missing:** the spec's e2e part 2 does not exist in scripts/e2e/updater.py. That part was: an `e2e-updater` build, a local `latest.json`, a throwaway key, bad-signature rejection, and a dry install that stops the test daemon. So the override code paths (updater.rs:35-43,78-82) ship with no test that uses them.
- **Result:** success criteria 2 ("a bad signature never installs") and 3 are unverified, and the 12/12 pass result covers neither.
- **Fix:**
  1. Run one `scripts/build-release.sh --bundles nsis` locally (no updater config needed) and confirm makensis accepts the hook.
  2. Implement e2e part 2. It needs no real installer: dry install plus a `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR`-isolated daemon.

## Medium

### M1. NSIS relative jump `+2` lands inside the plugin call
- **Where:** hooks.nsh:11-12, `IfFileExists "$INSTDIR\kelpied.exe" 0 +2` followed by `nsExec::Exec …`.
- **Problem:** a plugin call compiles to several instructions (`Call Initialize_____Plugins`, extract the DLL, `SetDetailsPrint`, `Push`, `CallInstDLL`). So `+2` does not skip the call; it jumps into its middle. When kelpied.exe is missing, nsExec still runs (and fails) with plugin initialisation skipped. Today the outcome is harmless; the logic is not what the line says.
- **Fix:** use LogicLib, which Tauri's installer already includes:
  ```nsis
  ${If} ${FileExists} "$INSTDIR\kelpied.exe"
    nsExec::Exec '"$INSTDIR\kelpied.exe" stop --wait --timeout-ms 15000'
    Pop $0
  ${EndIf}
  ```
  Also `Pop` after the PowerShell `nsExec::Exec`. nsExec pushes its exit code, so the hook currently leaves two stray stack entries.

### M2. The dialog's "continues its conversation" claim is wrong for some sessions
- **Where:** update-dialog.tsx:11 passes `isAgentKind` (agents.ts:64: anything but shell or custom) as `canResume`.
- **Problem:** the store already has the real rule, `store.resumable(s)` (store.ts:1117-1121). It requires an `agent_session_id` or a preset `resume_latest`, and returns false for chat sessions, which do resume (daemon.rs:551-557). Core agents without a captured id, and plugin agents without `resume_latest`, are shown as "continues" when they start fresh. That breaks the phase's "honest contract".
- **Fix:** `interrupted(sessions, (s) => s.chat || store.resumable(s))`, changing the signature to take the session.

### M3. Running the installer from inside a Kelpie terminal kills the installer
- **What happens:**
  1. The installer inherits `KELPIE_SESSION_ID`/token, so `kelpied stop` authenticates as that session (`cli_auth_params`, kelpie-proto lib.rs:230-240).
  2. `daemon.shutdown` is an admin method that sessions are refused (auth.rs:247), so the graceful stop fails.
  3. PowerShell force-kills kelpied.exe 3 s later, without a final persist.
  4. The installer process is inside that session's job (`KILL_ON_JOB_CLOSE`, proc_tree.rs:83), so it dies with the daemon.
- **When it bites:** an agent session, or a user in a Kelpie shell, running a freshly built setup.exe is a realistic workflow in this repo.
- **Fix:**
  - Have `stop` use `Client::connect_user()`, as the benches do. The installer acts for the user, and the user token is on disk.
  - In the hook, detect `ReadEnvStr $0 KELPIE_SESSION_ID`, then show a MessageBox ("Run the installer outside a Kelpie terminal") and `Abort`.

### M4. The release tag check only looks at Cargo.toml
- **Where:** release.yml:36-37. With `KELPIE_KEEP_VERSION` set, build-release.sh skips the bump script and its equal-versions assertion.
- **Problem:** the updater's `current_version` and the installer version come from tauri.conf.json. If tauri.conf.json drifts from the tag, installed apps either re-offer the same release forever or never see it.
- **Fix:** in that step, assert that Cargo.toml, app/package.json and app/src-tauri/tauri.conf.json all equal the tag.

### M5. The "no test hooks in release" CI check proves nothing
- **Where:** release.yml:41. `cargo tree -p kelpie -e features 2>/dev/null | grep -q e2e-updater` only inspects default features, before the build. A `cargo tree` failure is silenced and passes. A `--features e2e-updater` passed through `build-release.sh "$@"` would not be seen.
- **Fix:** after the build, fail if the shipped binary contains the override name:
  ```bash
  ! grep -aq KELPIE_UPDATE_ENDPOINT target/release/kelpie(.exe)
  ```
  (on macOS, check the binary inside the .app.)

### M6. Signing-key exposure in release.yml
- **Problems:**
  - `permissions: contents: write` is workflow-wide (lines 10-11), but only `publish` needs it.
  - Third-party actions are pinned by mutable tag or branch (`dtolnay/rust-toolchain@stable` is a branch).
  - `TAURI_SIGNING_PRIVATE_KEY` is in the environment for the whole build step. That step includes `pnpm install` and every crate's build.rs, so any compromised dependency can read the key, which signs updates for every install.
- **Fix:**
  - Give `build` `contents: read` and `publish` `contents: write`.
  - Pin actions to commit SHAs.
  - Ideally build without the key and sign in a separate step (`pnpm tauri signer sign` on setup.exe and the .app.tar.gz). At minimum, accept and document the residual risk.

### M7. Update checks also run in `pnpm tauri dev` and GUI e2e runs
- **Where:** `startUpdater()` (app.tsx:54, updater.ts:77-84) has no dev or e2e gate.
- **Problem:** dev and e2e apps call GitHub, and the status-bar chip can appear mid-test (selector and screenshot noise). "Install now" from a dev build runs the real NSIS installer over the user's installed Kelpie in %LOCALAPPDATA%. The constraint says e2e must never touch the installed Kelpie.
- **Fix:** skip when `import.meta.env.DEV`, when `platformInfo().e2e` is set, or in debug builds on the Rust side (`cfg!(debug_assertions)` in `update_check`/`update_install`, unless `e2e-updater` is enabled).

### M8. The pending update is consumed before the download and installs are not single-flight
- **Consumed early:** updater.rs:65 `take()`s the update before downloading. After a network failure the dialog still shows the update (`available()` stays set), but "Install now" returns "no update to install; check again".
- **Two installs at once:** an auto "when idle" install (updater.ts:87) and a manual click can run together. The second call fails with the same message, and the dialog's `busy` state does not reflect a background install.
- **Fix:** `clone()` the `Update` (it is `Clone`) and clear it only on success. Add a module-level `installing` signal in updater.ts that disables the dialog buttons and the chip.

## Low
- **L1. Daemon version toast fires in either direction.** store.ts:218 now also fires when the daemon is *newer* than the app. Dev and installed apps share the default endpoint (kelpie-proto lib.rs:128-136), so running both makes them offer to restart each other's daemon. Compare semver (`daemon < app`) or only offer when the app's bundled daemon is newer.
- **L2. Duplicate doc comment.** store.ts:212 keeps the old JSDoc line above the new one.
- **L3. Apostrophes in the install path break PowerShell.** hooks.nsh:15 puts `'$INSTDIR'` inside PowerShell single quotes, so a profile path containing an apostrophe (`C:\Users\O'Brien\…`) is a parse error and the force-stop never runs. Pass the path through an environment variable instead: `System::Call 'kernel32::SetEnvironmentVariable(t "KELPIE_INSTDIR", t "$INSTDIR")'`, then `$$env:KELPIE_INSTDIR`. The command is about 740 characters expanded, within NSIS's 1024 limit.
- **L4. If PowerShell is missing or blocked by AppLocker, nsExec just fails.** The graceful `stop --wait` has already run, and Constrained Language Mode still allows `Scripting.FileSystemObject`. Leftover OpenConsole.exe then surfaces as NSIS's Retry dialog. This is an acceptable degradation.
- **L5. No back-off for repeated offline errors.** The spec asks for it. Errors are swallowed, and with a 6 h interval this is harmless; note it as a spec deviation.
- **L6. Intel Macs never get updates.** Only `darwin-aarch64` is published. A manual check there gets a plugin "platform not found" error. Document this, or build universal and add `darwin-x86_64`.
- **L7. Release notes may come out as the commit message.** actions/checkout can turn an annotated tag into a lightweight one, and `%(contents)` then gives the commit message. Run `git fetch --force --tags` before reading the notes.
- **L8. `wait_for_exit` on Windows treats `OpenProcess` returning NULL as "gone".** That includes access denied (main.rs:108-110). Waiting on a pid read before the shutdown also carries a theoretical pid-reuse window. Both are negligible for a per-user daemon.
- **L9. "Install when idle" ignores `waiting` agents and active typing.** It fires on `waiting` agents (blocked on a user question) and while the user is typing in a shell, then exits the app without warning. Consider a short countdown notice with Cancel.

## Answers to the explicit checks
- **(a) Handover:**
  - Persist ordering is correct; resumable state survives a graceful stop (verified as above, and by e2e part 1).
  - In-app Windows path: plugin `install` runs ShellExecute and then `process::exit(0)` (plugin updater.rs:1037-1050), so `app.restart()` is reached only on macOS, which is correct.
  - Failure after the stop leaves the app without a daemon (H1). macOS zombie risk (H2).
  - The manual-install race is C2.
- **(b) Hook:**
  - Quoting (backticks, `$$`, `$INSTDIR`) is correct, and the length is fine.
  - An old daemon without `--wait` gets the shutdown request and then the 3 s wait plus force-stop; persist is atomic, so at worst the latest snapshots are lost.
  - The 8.3 comparison is sound: both sides go through the same FSO `ShortPath`, with a trailing `\`, case-insensitive.
  - It never matches processes outside `$INSTDIR`, and e2e proves this.
  - nsExec ships with Tauri's NSIS distribution, but compilation was never verified (H3). See also M1, L3, L4, M3.
- **(c) Security:**
  - Feature gating is correct, but the CI check is weak (M5).
  - Signature verification is done by plugin 2.14 `download()`, before anything stops.
  - The manifest format is correct.
  - Artifact paths are broken (C1). Permissions and key exposure: M6. Tag check: M4.
  - Merging `--config` works.
- **(d) UX:**
  - The "when idle" effect fires once: `installUpdate` clears `whenIdle` synchronously, and after a failure it does not retry, so no loop.
  - Skip suppresses exactly that version; a manual check re-offers it.
  - i18n is complete and the dialog is lazy.
  - Issues: M2, M7, M8, L9.
- **(e) Regressions:**
  - The `checkDaemonApi` restart no longer kills sessions, as intended (L1 aside).
  - Adding `platform_info.version` is additive; consumers are system-settings, store.ts:1244 and main.tsx:22.
  - Local and CI builds are unaffected: `createUpdaterArtifacts` lives only in tauri.updater.json, and ci.yml does not pass it.
- **(f) Patterns:** the code follows existing conventions (State-managed Rust commands, `fs_ops::err`, module-level Solid signals, catalog-driven settings, e2e isolation through `KELPIE_ENDPOINT`/`KELPIE_DATA_DIR`).

## Recommended actions (in order)
1. C1: flatten the downloaded artifacts in release.yml.
2. C2: run `CheckIfAppIsRunning` at the start of `KELPIE_STOP_DAEMON`.
3. H1: release `hold_spawn` on every failure path.
4. H2: treat zombie processes as gone, or reap them.
5. H3: one local NSIS bundle build, plus e2e part 2 (signed update, bad signature, dry install).
6. M1 (LogicLib + `Pop`), M2 (`store.resumable`), M3 (`connect_user` + terminal guard), M4 (three-way version check), M5 (binary string check), M7 (dev/e2e gate), M8 (clone + single-flight).
7. M6 hardening (job-scoped permissions, SHA pins, separate signing step).

## Plan follow-ups
- Requirement 1 is partly met: `stop --wait` and the hook are done; C2, M1 and M3 are open.
- Requirement 2 is done (L1 aside).
- Requirement 3 is mostly done; H1, H2, M2, M7 and M8 are open, and back-off (L5) is missing.
- Requirement 4 has C1, M4, M5 and M6 open.
- Requirement 5 is done, but the manual-install claim depends on C2.
- Tests: e2e part 2 and the hook compile check are missing (H3).
- All three success criteria are still unchecked: do not mark them done.

## Unresolved questions
- Is `darwin-x86_64` (Intel Mac) in scope for updates?
- Should the release be created as a draft first, so a human checks the assets before `releases/latest/download/latest.json` goes live?

Status: DONE_WITH_CONCERNS
Summary: Handover persistence, signature verification and feature gating are correct, but shipping is blocked by C1 (release publish fails on nested artifact paths) and C2 (a manual install while the app runs still hits "file in use" because the app respawns the daemon after the hook stops it); H1–H3 (no daemon after a failed install, macOS zombie wait, hook never compiled and signed-update e2e missing) should be fixed before the first release.
