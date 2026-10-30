# Code review: phase 15a Linux (`phase15/linux` @ 00596b0 vs origin/master d22d6f3)

Scope: `git diff origin/master...HEAD` (23 files). Read against phase-15 spec (15a), plan.md "Contract from
phase 2", AGENTS.md, docs/architecture.md, the developer report, and tauri-plugin-updater 2.14.0 source.

Local checks (Windows, `CARGO_TARGET_DIR=D:\kelpie-p9-target`, `-j 1`):
- `cargo test -p kelpie-proto`: 28 passed, 2 ignored.
- `cargo test -p kelpied awake`: 1 passed.
- `pnpm exec vitest run src/lib/updater.test.ts`: 5 passed.

Linux-only code (`cfg(target_os = "linux")` / `cfg(unix)`) cannot run here. It was reviewed by reading.

## Verdict

The process-safety fix is correct, and no other `kill -<n> -<pid>` remains. Windows and macOS behaviour is
unchanged. The updater keys are valid.

One High finding blocks shipping the AppImage: the AppImage runtime's environment leaks into the daemon and
from there into every terminal and agent session. The rest are Medium or Low and do not block. CI's install
check, AppImage check and Xvfb smoke are plausible, but they have never run.

## Findings

### H1 (High, blocking for an AppImage release): AppImage/GTK environment leaks into the daemon and every session

- `app/src-tauri/src/daemon_client.rs:284-314`: `spawn_daemon` starts `kelpied serve` with the app's full
  environment.
- `daemon_runtime.rs:27-30` only changes *which binary* runs; nothing strips the environment.
- What the app inherits under an AppImage:
  - The type-2 runtime sets `APPIMAGE`, `APPDIR`, `ARGV0` and `OWD`.
  - Tauri's linuxdeploy GTK hook (`apprun-hooks/linuxdeploy-plugin-gtk.sh`, confirmed from tauri-apps/linuxdeploy-plugin-gtk) exports:
    - `GDK_BACKEND=x11` and `GTK_THEME=Adwaita:*`;
    - `XDG_DATA_DIRS=$APPDIR/usr/share:…`;
    - `GSETTINGS_SCHEMA_DIR`, `GTK_EXE_PREFIX`, `GTK_PATH`, `GTK_IM_MODULE_FILE`, `GDK_PIXBUF_MODULE_FILE`, `GIO_EXTRA_MODULES` and `GI_TYPELIB_PATH`, all under `$APPDIR`.
- The daemon passes all of this to every shell and agent. It outlives the app, and `$APPDIR` (`/tmp/.mount_Kelpie…`)
  disappears when the app quits and is different on every launch.
- Failure scenarios:
  - **After the app quits:** a GUI program started from a Kelpie terminal (`meld`, `gedit`, `xdg-open` → GTK app, an editor) points at loaders, modules and an immodules cache that no longer exist. Pixbuf/icon loading fails (GTK3 can abort on a missing `image-missing` icon) and input methods (IBus/fcitx) are lost.
  - **On Wayland:** `GDK_BACKEND=x11` forces XWayland.
  - **While the app runs:** host GTK programs load the AppImage's Ubuntu 22.04 loader cache and modules, which is an ABI mismatch on newer distros.
  - **`APPIMAGE` leaks:** tools that key off `APPIMAGE` misbehave. One example is a `.deb` `kelpie` started from such a terminal: `daemon_runtime::runnable` would treat it as an AppImage.
- Fix: when `APPIMAGE` is set, have `spawn_daemon` clean the child environment:
  - remove `APPIMAGE`, `APPDIR`, `ARGV0` and `OWD`;
  - remove every variable whose value points into `$APPDIR`;
  - drop the `$APPDIR` entries from `XDG_DATA_DIRS`;
  - unset `GDK_BACKEND` and `GTK_THEME` (the hook overwrites them unconditionally, so the user's originals are lost; unsetting is the correct default).

  Add a unit test of the pure filter. It is cheap, and it is the only way to keep sessions identical between `.deb` and AppImage installs.

### M1 (Medium, non-blocking): the data folder is not created 0700 on Linux

- The brief asks for a 0700 directory and a 0600 socket. The socket is set to 0600 after bind
  (`crates/kelpie-proto/src/lib.rs:857-862`), and the token file is 0600 (`lib.rs:225-231`). So no other user
  can drive the daemon. Same as macOS.
- The data folder itself comes from `create_dir_all` with the umask:
  - `crates/kelpied/src/main.rs:222`;
  - `kelpie-proto/src/lib.rs:841`;
  - `daemon_runtime.rs:75`.

  In practice that is 0755, or 0775 under Ubuntu's UPG umask. On macOS, `~/Library` (0700) hides it. On Linux,
  `~/.local/share` is often 0755, and home folders are 0755 on Debian ≤ 11 and Ubuntu ≤ 20.04.
- Scenario: on such a host, another local account can read:
  - `logs/`;
  - `notify.json`, `custom-agents.json`, `claude-hooks.json`;
  - `checkpoints/` (file snapshots of the user's repos);
  - session state.
- Fix: on Unix, create `data_dir()` with `DirBuilderExt::mode(0o700)` (the XDG spec also asks for 0700 when
  creating `$XDG_DATA_HOME`).
- Not a regression; worth doing before a Linux release.

### M2 (Medium, non-blocking; verify): the release job runs unpinned bundler downloads with the signing key in its environment

- `.github/workflows/release.yml:65-74`: the Linux matrix entry mirrors the other jobs: `TAURI_SIGNING_PRIVATE_KEY`
  is in the environment for the whole `build-release.sh`.
- New on Linux: tauri-bundler's AppImage step downloads `linuxdeploy` and the GTK plugin from GitHub and runs
  them, as I understand the bundler (it is not vendored locally, so please confirm).
- Scenario: a compromised upstream asset can read the signing key, and that key signs every platform's updates.
- Pre-existing in the same job: `scripts/build-release.sh:76` runs `pnpm install --frozen-lockfile` again with
  the key in the environment. That contradicts the comment at `release.yml:61` ("Dependencies install without
  the signing key").
- Options:
  - bundle without the key, then sign in a separate step (`tauri signer sign`) that has the key in its environment;
  - or accept and document.
- Secrets are never echoed. There is no `pull_request_target`. `publish` has `contents: write` and runs only on
  tag push. All of that is fine.

### L1 (Low): the Linux "slot in use" check is a no-op, so slot `a` is always rewritten

- `daemon_runtime.rs:67-73` detects a slot in use only because Windows refuses to delete a running program. On
  Linux, `remove_file` always succeeds.
- `spawn_daemon` runs on *any* connect failure (`daemon_client.rs:105-108`). So after an AppImage update, one
  transient connect error rewrites slot `a` under the running old daemon.
- The old daemon's stored `exe` (`daemon.rs:87`, used for hook and launch commands at `:607/:973/:1000`) then runs
  the new build.
- That is the same cross-version situation macOS and the `.deb` already live with (the bundle or package is
  swapped under the daemon), so it is tolerated. The new spawn exits on the lock.
- Fix: correct the comments and docs that say Linux alternates a/b like Windows:
  - `daemon_runtime.rs:11`;
  - architecture "Daemon runs from" row.

  Or skip the slot whose build the running daemon reports.
- Related:
  - **Fallback:** when the copy fails (`daemon_runtime.rs:34-37`), the daemon runs from the AppImage mount and dies (SIGBUS) once the app quits. That makes the warning "updates will have to stop it" understated.
  - **Update dialog:** `daemonSurvivesUpdate(exe, unix=true)` (`app/src/lib/updater.ts:160-163`) still says such a daemon survives. Consider returning `false` for an `exe` under `/.mount_`.

### L2 (Low): keep-awake edge cases (`crates/kelpied/src/awake.rs`)

The lifetime design is correct:
- `PR_SET_PDEATHSIG(SIGTERM)` is set in `pre_exec` on the dedicated `kelpie-awake` thread, which lives as long as `Awake` and ends with the process.
- The `getppid()` re-check closes the fork race.
- The daemon installs no `SIG_IGN` handlers that would be inherited.
- `systemd-inhibit` forks `sleep` with FORK_DEATHSIG and FORK_CLOSE_ALL_FDS, so the logind fd is released as soon as it dies.
- Release and exit do `kill` + `wait`, so no zombie.

Remaining edges:
- **Late refusal (`:507-518`):** a refusal that arrives after the 300 ms probe (slow D-Bus) leaves an exited child unreaped until the next toggle. During that time `held()` reports true with no lock, and the warning is never logged. Minor.
- **Explicit suspend:** `--what=…sleep --mode=block` also blocks *explicit* user suspend from the desktop menu (logind asks for the polkit ignore-inhibit authorisation). Lid close still suspends, because `LidSwitchIgnoreInhibited=yes` is the default. macOS `caffeinate -i` and Windows `ES_SYSTEM_REQUIRED` do not block an explicit sleep. Document it in the user guide, or accept it.

### L3 (Low): CI and release script details

- **`xauth` (`.github/workflows/ci.yml:30-33`):** `--no-install-recommends` with `xvfb`. On Debian/Ubuntu `xauth` is only a *Recommends* of `xvfb`, yet `xvfb-run` needs it. The runner image very likely already has it, but add `xauth` explicitly so the never-run smoke step does not depend on the image.
- **Optional `.deb` entry (`release.yml:134`):** `deb=$(ls dist/*.deb 2>/dev/null | head -1)` runs under Actions' default `bash -eo pipefail`. A missing `.deb` aborts the step, so the "optional .deb" logic is dead. It is harmless while the `.deb` is always built; `ls … || true` matches the intent.
- **Release fail-fast (`release.yml:18`):** `fail-fast: true` means the first-ever Linux release run can cancel the Windows and macOS builds of the same tag. Consider a release-candidate tag or `fail-fast: false` for the first Linux release.
- **`boot_smoke.py` `stop()` (`:111-127`):**
  - The SIGKILL `os.killpg` is not wrapped in try (ProcessLookupError inside `finally` would mask results).
  - When the group leader has already exited (`p.poll() is not None`), the group is not signalled, so a surviving WebKitWebDriver or app in that group is left behind.

  CI-only.

### L4 (Low, pre-existing): `/tmp` fallback

- `kelpie-proto/src/lib.rs:350` falls back to `std::env::temp_dir()`, which gives `/tmp/Kelpie`, a shared
  directory. This happens when HOME is unset or, new here, relative.
- Another user could pre-create `/tmp/Kelpie` and plant the token, socket or daemon slot (`daemon_runtime.rs:52`
  trusts a predictable `build.txt`).
- Practically unreachable for a desktop app. Consider failing loudly instead.

### L5 (Low, defense in depth): `proc_tree.rs` killpg sites have no pgid guard

- `crates/kelpied/src/proc_tree.rs:281,288-294,330` use `pid as i32` with no `> 1` guard, unlike `git_finish.rs:213`.
- All current callers pass pids from successful spawns in their own groups:
  - `session.rs:385` (portable-pty setsid);
  - `chat.rs:362-366`, `automation.rs:553-557`, `procs.rs:724`, all with `process_group(0)`.

  So 0, 1 and -1 cannot occur today. A guard in `adopt` would make that hold for future callers too.

### Nits

- `app/src/components/update-dialog.tsx:13`: inline `!/Win/.test(navigator.platform)`. An `isWindows` in `lib/platform.ts` next to `isMac` would keep platform detection in one place.
- The Linux `keyring` entries in `crates/kelpied/Cargo.toml` and `app/src-tauri/Cargo.toml` are redundant. Neither crate calls `keyring::` directly, and the features already reach the build through kelpie-proto. Harmless.

## Process safety: verified

- **`git_finish.rs:207-218`:**
  - The check is spawned with `process_group(0)` (`:891`), and a spawn whose `setpgid` fails is a spawn error. So pgid = child pid.
  - `try_from` rejects pids above `i32::MAX`, and `> 1` rejects 0 and 1. The app's own group cannot match a fresh child pid.
  - The timeout and cancel paths killpg *before* `child.kill().await` reaps, so there is no reuse window.
  - The success path signals after `wait()` has reaped. A theoretical pid-reuse window of microseconds needs a pid wrap; same as before; informational.
  - On macOS, `libc::killpg` and `process_group` work as on Linux. BSD `kill` was never affected.
  - The regression test `a_stopped_check_never_reaches_programs_outside_it` would catch a return of `kill(-1)`.
- **No `kill` program calls remain** in `.rs`, `.sh`, `.py`, `.ts` or the workflows:
  - The other signal sites are `libc::kill(pid)` on identity-checked pids (`procs.rs:639,656`; `chat.rs:728`; `main.rs:131` with signal 0) and `proc_tree.rs` killpg (L5).
  - `scripts/dev-daemon.sh:16` uses `kill "$pid"` with a positive pid.
- **Keep-awake lifetime:** see L2. Correct.
- **AppImage copy:**
  - Integrity: a slot is reused only when `build.txt` is "version + mtime + size" and matches, so no stale copy runs after an update, and the copy is not hash-verified. `build.txt` is written last.
  - Permissions: `fs::copy` keeps the source mode (0755 from the squashfs). The slot directory follows the umask (see M1). No other user can swap it unless `XDG_DATA_HOME` or `HOME` points at a shared directory (L4).
  - Phase-2 contract: the updater replaces the AppImage file, and the running daemon (from the slot) keeps running. The new app reconnects to the old daemon. The `.deb` is upgraded by `dpkg` renaming files under a daemon running from `/usr/bin`. Both satisfy "the new app runs against the previous daemon".

## Correctness: verified

- **Windows/macOS unchanged:**
  - `daemon_runtime::runnable`: `!(windows || appimage)` reduces to the old `!windows` off Linux.
  - `data_base`: only the final `else` changed.
  - `default_shell` on Windows: the same code, moved under `#[cfg(windows)]`.
  - `wrap_for_platform` on Windows: untouched.
  - macOS `login_shell`: identical, except that an *empty* `SHELL` now falls back to `/bin/zsh` instead of exec'ing `""`, which is an improvement.
  - `daemonSurvivesUpdate`: unchanged for macOS (true) and Windows (regex).
- **Updater keys** (tauri-plugin-updater 2.14.0 `src/updater.rs:753-761`):
  - Lookup is `{os}-{arch}-{installer}`, then `{os}-{arch}`, with installer names `appimage`, `deb` and `rpm` (`:70-72`).
  - `linux-x86_64-deb` is valid. The AppImage resolves `linux-x86_64-appimage`, then falls back to `linux-x86_64`.
  - A raw ELF `.AppImage` is accepted (`:1235`).
  - A `.deb` build that is offered the AppImage fails `is_deb` (`:1331`) with an error and no damage.
  - `install_deb` uses `pkexec dpkg -i` first (`:1383`), as the docs say.
- **`build-release.sh`:**
  - The non-universal branch stages `kelpied-<triple>` and `kelpie-stt-<triple>` (no extension on Linux).
  - The `python` → `python3` shim also works inside `$(…)` and pipes.
  - The version bump is unchanged; `KELPIE_KEEP_VERSION` is respected.
- **Login shell:**
  - `$SHELL` and the passwd shell are passed as a program path, never through a shell. Agent argv is `shell_quote`d.
  - nologin and false are skipped.
  - A relative `$SHELL` is accepted (PATH lookup). That is the same account's input, so not an injection boundary.
- **Keyring:**
  - Linux gets `sync-secret-service` + `crypto-rust`, so there is no mock or plaintext store.
  - Errors keep the D-Bus cause and never include the value. `BadEncoding` only arises on get, and get is not mapped.
  - The real-store test stays `#[ignore]`.

## CI review

- **Step order:** apt → tests → frontend → perf → bundle → install check → smoke → uploads. Each Linux step
  depends only on artifacts of earlier steps.
- **Bundle contents:** `target/release/kelpie` and `kelpied` exist from `build-release.sh`. The `.deb` file list
  was seen in run 40977107950.
- **Install check:** `dpkg -i … || { apt-get install -f && dpkg -i; }` is safe under `set -e`. `--appimage-extract`
  needs no FUSE. The relative `../$img` resolves correctly from `appimage-check/`.
- **Smoke isolation is good:**
  - `KELPIE_ENDPOINT` is a `/tmp` socket.
  - `KELPIE_DATA_DIR` and the three XDG folders are in scratch, so the token lands in scratch too.
  - `KELPIE_DAEMON_PATH` is set, and the inherited `KELPIE_SESSION_*` and `KELPIE_ALLOW_REAL_DAEMON` are removed.
  - Teardown shuts down only its own endpoint and removes the socket and `WORK`.
  - No secrets are involved.
- **Release build under the driver:** `KELPIE_E2E` exposes `__kelpie` in the release build (`main.tsx:20-25`).
  `interactiveAt` is wall-clock ms, so the cold-start subtraction is sound.
- **Remaining CI risks:** `xauth` (L3), and WebKitWebDriver key actions under Xvfb, which are unverifiable until run.

## Docs and tests

- **Docs:**
  - The architecture Platforms table and the user-guide Linux sections are accurate.
  - Two exceptions: the a/b slot statement for Linux (L1), and no mention that keep-awake blocks menu suspend (L2).
- **Missing tests:**
  - an environment-sanitising filter (H1);
  - the Linux branch of `runnable()` (an `APPIMAGE`-set path) and a 0700 data-dir test (M1).
- **Never run:** the `.deb`/AppImage install check, the Xvfb smoke, and the release job. The spec success
  criterion "Linux build installs and passes smoke" is not met yet.

## Unresolved questions

1. Does tauri-cli 2.12.x sign the `.deb` (`.deb.sig`) under `createUpdaterArtifacts: true`? The workflow handles both cases, so it is not a correctness issue.
2. Should the signing key be isolated into a separate signing step (M2)? This is a product and ops decision.
3. Is blocking explicit suspend (L2) acceptable, or should keep-awake on Linux be idle-only with `--mode=delay` for sleep?

Status: DONE_WITH_CONCERNS
Summary: The process-safety fix is correct, no other negative-pid `kill` remains, Windows and macOS are unchanged, and the updater keys are valid. One High finding: the AppImage/GTK runtime environment leaks through the daemon into every session.
Concerns/Blockers: H1 blocks an AppImage release. M1 (0700 data dir) and M2 (signing key exposed to bundler downloads) should be decided before the first Linux release. The CI install check and smoke have never run.
