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

---

## Round 2 (`git diff 00596b0 fc831c8`)

Reviewed the fix commits:
- `51f2b90`: AppImage env;
- `9195f57`: private data dir;
- `7454b2b`: keep-awake;
- `659ab1e`: pgid guard;
- `466fdd9`: key scoping;
- `da1b258`: macOS procs test;
- `96ece38`: smoke prompt and baseline;
- `45bae79`: docs.

CI run 40994932186 is reported green on all three OSes, including the `.deb` and AppImage checks and the Xvfb
smoke. I did not re-run CI.

Local checks (Windows, `D:\kelpie-p9-target`, `-j 1`):
- `cargo test -p kelpie-proto`: 29 passed, including `the_data_folder_is_created_for_this_user_only`. Only its non-Unix part runs here.
- `cargo test -p kelpied awake`: 1 passed.
- `cargo test -p kelpie --lib daemon_runtime`: 3 passed, including `the_daemon_never_inherits_the_appimage_mount_or_its_gtk_setup`.
- `vitest run src/lib/updater.test.ts`: 5 passed.

### Disposition of round-1 findings

| Round 1 | Status | Notes |
|---|---|---|
| H1 AppImage env leak | **Fixed (daemon side)** | See R2-1 and R2-2 |
| M1 data folder 0700 | **Fixed** | See R2-3 |
| M2 signing key exposure | **Reduced, documented** | See R2-4: the claim of containment is too strong |
| L1 slot `a` always rewritten | **Fixed (documented)** | Module docs and the architecture table now describe the Linux behaviour |
| L1/L2 copy failure → mount daemon | **Fixed** | See R2-5 |
| L2 keep-awake edges | **Fixed, new risk** | See R2-6 |
| L3 `xauth`, `.deb` line, fail-fast, `stop()` | **Fixed** | `xauth` added; `|| true`; `fail-fast: false`; `stop()` signals the group even after its leader exits, tolerates ESRCH and escalates to SIGKILL |
| L4 `/tmp` fallback | Open (pre-existing) | Folded into R2-3 |
| L5 proc_tree pgid guard | **Fixed** | See R2-7 |
| Nits | **Fixed** | `isWindows` in `platform.ts`. The redundant keyring entries stay, which is harmless |

### R2-1: the `appimage_env_fixes` filter (`app/src-tauri/src/daemon_runtime.rs:115-166`), verified

- **Gating:** the filter returns nothing unless `APPIMAGE` is in the app's environment (`:142`). `spawn_daemon`
  calls it only under `cfg!(target_os = "linux")` (`daemon_client.rs:294`). So `.deb`, Windows and macOS are
  untouched, and the test covers the "not an AppImage" case (`:219`).
- **Complete against the GTK hook I fetched:**
  - The base hook's `APPDIR`, `GTK_DATA_PREFIX`, `GTK_THEME` and `GDK_BACKEND` are handled. The two forced values are removed only when they equal what the hook writes.
  - The base hook's `XDG_DATA_DIRS` has its `$APPDIR` entries dropped.
  - The conditional exports `GSETTINGS_SCHEMA_DIR`, `GI_TYPELIB_PATH`, `GTK_EXE_PREFIX`, `GTK_PATH`, `GTK_IM_MODULE_FILE`, `GDK_PIXBUF_MODULE_FILE` and `GIO_EXTRA_MODULES` are all handled.
- **Complete against the type-2 runtime:** `APPIMAGE`, `APPDIR`, `ARGV0` and `OWD` are removed.
- **Catch-all:** any other variable whose whole value is `$APPDIR` or under `$APPDIR/` is removed. That covers the hook's `$APPDIR//usr/...` double slash and plugins such as gstreamer's.
  - `$APPDIR` is trimmed, and `/` (`len > 1`) is refused, so a degenerate `APPDIR=/` cannot wipe the whole environment.
  - Prefix matching is on `{d}/`, so `/tmp/.mount_X-other` is kept (tested).
- **User values are kept:**
  - `PATH`, `LD_LIBRARY_PATH`, `PYTHONPATH` and `PERL5LIB` lose only the `$APPDIR` entries; an empty result unsets the variable.
  - A user's `GDK_BACKEND=wayland` or `GTK_THEME=Yaru` is kept (tested).
- **Residual (Low, non-blocking):** `APPIMAGE_ONLY` removes `GTK_PATH`, `GI_TYPELIB_PATH`, `GIO_EXTRA_MODULES` and `GSETTINGS_SCHEMA_DIR` *unconditionally*.
  - The hook exports these only when the bundled GTK has them (build-time detection).
  - So a user's own value survives in the app's environment whenever the hook did not overwrite it, and the filter then drops it anyway.
  - Treating them as `APPIMAGE_LISTS` (drop only `$APPDIR` entries) keeps the user's value and still removes the hook's.
- **Negligible:** non-UTF-8 variables are skipped by the scan (`daemon_client.rs:296`), so one pointing into the mount would survive.
- **Upgrade note:** a daemon started by an earlier AppImage build keeps its polluted environment until it is restarted. It only applies to pre-release AppImage testers.

### R2-2 (Medium, non-blocking, new): programs the *app* starts still inherit the AppImage environment

- The H1 fix covers the daemon and therefore sessions. The app itself also starts long-lived GUI programs with
  its own environment, which still has the hook's GTK setup:
  - `external_editor.rs` (`code` and the other editors);
  - `xdg-open` at `agent_config.rs:1228` and `fs_ops.rs:354`.
- Scenario:
  1. "Open in VS Code" or "reveal in folder" from the AppImage starts VS Code or the file manager with `GDK_BACKEND=x11`, `GTK_PATH`/`GIO_EXTRA_MODULES`/`GDK_PIXBUF_MODULE_FILE` pointing into `/tmp/.mount_…`, and `APPIMAGE` set.
  2. They outlive Kelpie.
  3. Once Kelpie quits, their GTK file dialogs, icons and input methods hit a dead mount.
- Fix: a shared helper, for example `daemon_runtime::clean_appimage_env(&mut Command)`, applied to those spawn sites.

### R2-3: `ensure_private_dir` (`crates/kelpie-proto/src/lib.rs:345-362`), verified with two Low notes

- **Creation:** `DirBuilder::recursive(true).mode(0o700)` creates the folder and any missing parents as 0700 (the XDG spec asks the same for `$XDG_DATA_HOME`).
- **Narrowing:**
  - Only when `uid == geteuid()` and `mode & 0o077 != 0`, so another user's folder is never chmodded.
  - `metadata` follows a symlink, so a symlinked data folder narrows its target, and only when we own the target. That is correct.
- **Call sites:**
  - app start (`app/src-tauri/src/lib.rs:233-236`, logs only);
  - daemon start (`kelpied/src/main.rs:223`, fatal);
  - token write (`kelpie-proto/src/lib.rs:223-226`: the folder above `auth/` and `auth/` itself).
- **macOS:** `~/Library/Application Support/Kelpie` goes from 0755 to 0700. Every consumer (app, daemon, phone server, CLI) runs as the same user, so I see no regression. Windows is a plain `create_dir_all`, as before.
- **Low (non-blocking): a failed chmod is fatal for the daemon.**
  - On a filesystem without POSIX modes (vfat/exFAT/CIFS mounted with `uid=` the user, without `quiet`), the folder reports 0755 and `chmod` returns EPERM.
  - That error goes up through `main.rs:223` and through `write_user_token` → `auth::init`, so `kelpied` refuses to start.
  - It needs `XDG_DATA_HOME` or `KELPIE_DATA_DIR` on such a mount, so it is rare.
  - Fix: log a warning when the narrowing `set_permissions` fails, and only fail when the folder cannot be created.
- **Low (pre-existing L4): a folder owned by someone else is "left alone" and then used.**
  - With the `/tmp` fallback, or an `XDG_DATA_HOME` pointing at a shared place, a pre-created foreign `Kelpie` folder is accepted silently.
  - Refusing such a folder (or one that is group- or world-writable and not ours) would close L4 at almost no cost.

### R2-4 (Medium, non-blocking): key scoping: correct mechanics, overstated isolation

- **Mechanics are correct:**
  - `scripts/build-release.sh:14-18` copies the key into *unexported* shell variables and unsets the environment variables before any other command. `cargo build` of the sidecars, the version bump and `pnpm install` cannot inherit it.
  - `:83-88` passes it as a command-prefix assignment only to `pnpm tauri build`.
  - Nothing echoes it. The script never enables `set -x`.
  - The empty-key branch keeps local and CI unsigned builds working.
- **Trace caveat:** under a manual `bash -x scripts/build-release.sh`, the `signing_key=…` assignment and the prefixed command would print the key. Actions masks registered secrets in logs, and the workflow does not use `-x`. Acceptable; a one-line comment would help.
- **The claim is too strong.** Comments at `release.yml:62-65` and `build-release.sh:14-15` say the earlier commands "never see" the key, but the step's own shell still has it.
  - The release step's shell (`bash -e {0}` started with `env: TAURI_SIGNING_PRIVATE_KEY`) and the script's starting environment both still hold the key.
  - On Linux, any same-uid process (a dependency's `build.rs`, a pnpm lifecycle script) can read `/proc/<ancestor pid>/environ`. On macOS, `KERN_PROCARGS2` gives the same for a same-uid process.
  - So the unset stops *accidental* inheritance (tools that dump their environment), not a malicious build step.
  - `pnpm tauri build` itself (frontend build, the app crate's dependency build scripts, and the linuxdeploy and GTK plugin downloads) still runs with the key. The comment acknowledges that.
- Real containment: bundle with no key in the job's environment, then sign in a separate step (`tauri signer sign` on each updater artifact). That is the decision left open in round 1 (unresolved question 2).
- Until then, reword the two comments to "not passed on to …" rather than "never see".

### R2-5: copy-failure path, verified

- `daemon_runtime.rs:37-42`: a failed copy under an AppImage logs that the daemon runs from the mount and stops when Kelpie quits.
- `updater.ts:304`: `daemonSurvivesUpdate` returns `false` for an `exe` matching `/\.mount_[^/]*\//`.
  - The AppImage runtime always mounts at `<tmp>/.mount_<name><random>`, whatever `TMPDIR` is, so the pattern matches.
  - A slot copy (`…/Kelpie/daemon/a/kelpied`) or `/usr/bin/kelpied` still survives. All three cases are tested.
  - So the update dialog now warns correctly.
- Slot rewrite on Linux is documented: the module docs (`:11-14`) and the architecture table say `a` is rewritten each build, and `b` is used only when `a` cannot be written.

### R2-6 (Medium, non-blocking; verify on GNOME before release): keep-awake `--what=idle` may not stop GNOME's automatic suspend

- **Mechanics are fine:**
  - `recv_timeout(5 s)` runs only while a lock is held (`awake.rs:430-443`); with no lock it is a plain blocking `recv`, so there are no wakeups at idle. While held, 0.2 Hz is negligible.
  - A late refusal is caught by `exited()`, reaped and logged once (`:444-451`), so there is no zombie and no leak.
  - Remaining minor point: after a late refusal, `Awake::held()` stays `true`, and `set(true)` deduplicates, so nothing retries until the mode toggles. Acceptable; the log says why.
- **The concern is behavioural:**
  - A logind `idle` lock is honoured by logind's own `IdleAction`, which defaults to `ignore`.
  - On GNOME (Ubuntu's default), automatic suspend is triggered by gnome-settings-daemon, which follows gnome-session's own inhibitors. Whether it consults logind `idle` inhibitors is unclear, and I could not confirm it. Public reports are mixed:
    - [gnome-session#110](https://gitlab.gnome.org/GNOME/gnome-session/-/issues/110);
    - [a GNOME Discourse thread on inhibitors](https://discourse.gnome.org/t/why-does-gnome-shell-doesnt-notify-user-about-suspend-being-blocked-by-inhibitor/34077).
  - One forum report says GNOME's menu suspend went through *despite* a logind sleep block lock ([Zorin forum](https://forum.zorin.com/t/caffeine-in-zorin-lite/69157?page=2)). If that holds, round 1's `idle:sleep` did not actually block menu suspend on GNOME, and the narrowing may have removed the part that worked.
  - My round-1 L2 prompted this change, so I flag the risk rather than assert it.
- **Verify on a real GNOME and KDE desktop before the Linux release:**
  1. Set a 1-minute automatic suspend.
  2. Hold keep-awake with an agent working.
  3. Check with `systemd-inhibit --list`.
- If GNOME ignores it, either restore `idle:sleep`, or add `org.gnome.SessionManager.Inhibit` / `org.freedesktop.PowerManagement.Inhibit` (flags idle and suspend) on D-Bus.
- The docs currently promise "stops automatic sleep", which is unverified.

### R2-7: proc_tree pgid guard (`crates/kelpied/src/proc_tree.rs:280-288`), verified

- `adopt` refuses a pid that does not fit an `i32`, a pgid ≤ 1, and the daemon's own `getpgrp()`, with `InvalidInput`.
- Every caller already handles an `Err`:
  - `session.rs:389` (`.ok()` + warn);
  - `chat.rs:366` (`.ok()`, then the `kill_pid` fallback);
  - `automation.rs:557` (`.ok()`);
  - `procs.rs:724` (`.ok()`).
- Real children are never rejected: they are new group leaders, so their pid can never equal the daemon's pgid. Windows is untouched.

### R2-8: the macOS `procs` test change (`crates/kelpied/src/procs.rs:1090-1094`), correct; no real bug masked

- The process table refreshes with `refresh_kind()` = memory + CPU only (`:567-569`), and sysinfo keeps the name it read when it first saw a pid. So a forked `sh` (bash on macOS) seen before its `exec` keeps the name `bash` after it becomes `/bin/sleep`.
- The identity used for listing and killing comes from `now()` with `with_exe(UpdateKind::Always)` (`:583-589`), applied in `proc_of` (`:153-157`). So `exe` is always current.
- The test now accepts a fresh `exe` ending in `/sleep`. It still asserts:
  - orphan detection with a matching start time (`:1103-1104`);
  - a kill with no refusal (`:1106`);
  - that the process is gone (`:1111`).

  Kill safety (pid + start time + fresh exe) is still fully exercised, so nothing is masked.
- Production consequence (Low, cosmetic): the Processes panel can show a stale name (`bash`) for a process that `exec`'d into another program, which phase 13 also touched. Optional: display the fresh `exe` file name when it differs from the table name.

### Other round-2 changes

- `boot_smoke.py`:
  - Prompt detection is now per line with trailing spaces trimmed (`$`, `%`, `#`, `>`). That matches bash's `user@host:~$`. `PS` is dropped, which is fine because the script is Unix/CI-oriented.
  - The 1 s wait before the screenshot is harmless.
- `scripts/perf/linux-smoke-baseline.json`:
  - It records cold start 634 ms, idle CPU 4.53 % and RSS 505.7 MB from run 40992930356, with provenance, and is not gated.
  - 4.5 % idle CPU under Xvfb software rendering is worth watching against the spec's "no idle cost". Under Xvfb it is likely WebKit software compositing, but a later gate should compare like with like.
- Docs (architecture table, keep-awake bullet, user guide) match the code, except the unverified keep-awake promise (R2-6).

### Round 2 blocking summary

- **Blocking: none.** H1 is fixed for the daemon and sessions.
- **Before the first Linux release (non-blocking for merge):**
  - **R2-6:** verify keep-awake on GNOME and KDE (it may silently not work on GNOME).
  - **R2-2:** clean the AppImage environment for the app's own external launches.
  - **R2-4:** a separate signing step, or at least accurate comments.
- **Low:**
  - R2-1 (list treatment for the 4 GTK variables);
  - R2-3 (EPERM on a non-POSIX filesystem is fatal; accept or refuse a foreign data folder);
  - R2-8 (stale display name).

Status: DONE_WITH_CONCERNS
Summary: The round-1 findings are fixed or documented, and the focused tests pass. H1 is resolved for the daemon and every session, M1 and the pgid guard are correct, and the macOS procs test change is sound and masks no kill-safety bug.
Concerns/Blockers: No blocking findings. Before the Linux release: R2-6 (keep-awake `--what=idle` may not stop GNOME's automatic suspend; verify on a desktop), R2-2 (app-launched editors and `xdg-open` still get the AppImage environment), R2-4 (the step shell still holds the signing key, readable through `/proc`; the comments overstate the isolation).
