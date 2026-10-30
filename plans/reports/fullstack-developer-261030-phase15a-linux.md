# Linux: data folder, login shell, keep-awake, keyring, packages and CI smoke

Branch `phase15/linux`, from `phase15/session-export` (95c7585). No daemon RPC or API_LEVEL change.

## What was built

- **Data folder** (`crates/kelpie-proto/src/lib.rs`): `$XDG_DATA_HOME/Kelpie`, else `~/.local/share/Kelpie`.
  - Resolution is a pure function, `xdg_data_home`. An empty or relative `XDG_DATA_HOME` is ignored, as the XDG spec says.
  - The socket (`kelpied-v1.sock`), logs, settings, the auth token folder and the daemon's runtime copies all derive from `data_dir()` / `data_base()`, so they follow it.
  - Windows and macOS paths are unchanged.
  - Tests: XDG set, unset, empty, relative and none (pure, run everywhere). A Linux-only test checks that `data_dir()` and the socket sit under the XDG base.
- **Login-shell PATH** (`crates/kelpied/src/agents.rs`).
  - macOS has no timed PATH import. Every agent starts as `$SHELL -l -c 'exec …'` and every shell as `$SHELL -l`; that code was already `cfg(unix)`, so Linux shares it.
  - The Linux gap was the fallback when `SHELL` is unset (some desktop launchers): it was `/bin/zsh`, often missing on Linux.
  - New `login_shell()`: `$SHELL`, else on Linux the account's shell from `getpwuid_r` (skipping `nologin` / `false`), else `/bin/sh`. macOS keeps `/bin/zsh`.
  - It is used by the agent wrapper, the default shell and the installed-agent probe. Unit test on Unix.
- **Keep awake** (`crates/kelpied/src/awake.rs`).
  - Linux runs a `systemd-inhibit --what=idle:sleep --who=Kelpie --why="An agent is working" --mode=block sleep infinity` child.
  - It is spawned with `PR_SET_PDEATHSIG(SIGTERM)` and a `getppid` check, so it dies with the daemon even after a crash. `systemd-inhibit` takes its `sleep` down with it (FORK_DEATHSIG).
  - It is killed on release and when the daemon exits.
  - Missing `systemd-inhibit`, or logind refusing the lock (it exits within 300 ms; its stderr is captured), is logged once as a warning and never panics.
  - Tests: the argument list, and a Linux hold/release cycle that must survive a runner without a session.
- **Keyring**: `keyring` 3.6 with `sync-secret-service` + `crypto-rust` under `[target.'cfg(target_os = "linux")'.dependencies]` in kelpie-proto, kelpied and the app.
  - Before this, Linux builds had no keyring backend, so keyring 3 fell back to its in-memory mock store and secrets were silently lost.
  - `secret_set` / `secret_delete` now return an actionable `String` error. On Linux, `PlatformFailure` / `NoStorageAccess` say to install and unlock GNOME Keyring, KWallet or another `org.freedesktop.secrets` provider, and keep the cause.
  - There is no fallback store. Unit test added.
- **Notifications**: `tauri-plugin-notification` on Linux uses notify-rust over zbus (pure Rust, no extra system package). It compiles as part of the app crate in the Linux CI job.
- **AppImage runtime** (`app/src-tauri/src/daemon_runtime.rs`).
  - When `APPIMAGE` is set, the app starts the daemon from its copy in `<data>/daemon/a|b`, as Windows does.
  - Reason: the AppImage mount disappears when the app quits and its path changes every launch, while the daemon outlives the app and agents' hook settings name its path.
  - The update dialog treats Linux like macOS: replacing the AppImage or the package never stops the daemon (`updater.ts`, vitest case).
- **Packaging** (`scripts/build-release.sh`).
  - `--bundles appimage,deb` works on Linux with the existing sidecar staging (`kelpied`, `kelpie-stt` → `usr/bin`).
  - A `python` shim falls back to `python3`; Debian and Ubuntu ship no `python`.
- **Release** (`.github/workflows/release.yml`).
  - New `ubuntu-22.04` matrix entry: apt packages, the same signing env, and `APPIMAGE_EXTRACT_AND_RUN=1` for linuxdeploy.
  - The binary check covers `target/release/kelpie`. Uploads: `.AppImage(.sig)`, `.deb(.sig)`.
  - `latest.json` gains `linux-x86_64` → the signed `.AppImage`; Tauri 2 signs the AppImage itself when `createUpdaterArtifacts: true`, not a `.tar.gz`.
  - It also gains `linux-x86_64-deb` when the bundler produced a `.deb.sig`: the updater looks for `<os>-<arch>-<bundle>` first.
  - `latest_json.py` needed no logic change; its doc names the Linux artifact.
- **CI** (`.github/workflows/ci.yml`): `ubuntu-22.04` was added to the matrix. Windows and macOS steps are unchanged.
  - apt packages: `libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev patchelf libasound2-dev xdg-utils libfuse2 file webkit2gtk-driver xvfb dbus-x11`.
  - The same cargo tests (kelpie-proto, kelpied, kelpie, kelpie-stt), the frontend checks and the perf gate (bundle size, bench).
  - The bundle step builds `appimage,deb`.
  - **Install check**:
    - `dpkg-deb --info/--contents`, which must list the app, both sidecars and the desktop file;
    - `dpkg -i`, with every shared library resolved (`ldd`), then `dpkg -r`;
    - the AppImage is an executable ELF and `--appimage-extract` shows the app and both sidecars.
  - The bundles are uploaded as artifacts.
  - **Smoke under Xvfb** (`dbus-run-session -- xvfb-run`):
    - `tauri-driver` + `WebKitWebDriver` drive the release app with `KELPIE_E2E=1`;
    - the result JSON is uploaded as the `linux-smoke` artifact.
- **e2e helpers** (`scripts/e2e/smoke_support.py`).
  - `native_driver()` returns WebKitWebDriver on Linux, msedgedriver elsewhere, as before. `tauri_driver()` is shared too.
  - `real_data_dir()` makes the "never the real data folder" guard work on Linux and macOS as well; it was Windows-only before.
- **Smoke script** `scripts/e2e/boot_smoke.py`. It is new because `smoke.py` needs pre-started services, three projects including `orca-ref`, and Windows prompts.
  - Its own daemon on its own `KELPIE_ENDPOINT` (a short socket path in `/tmp`) and `KELPIE_DATA_DIR`. On Linux the XDG data, config and cache folders also point into scratch.
  - It boots the app, checks the app talks to *that* daemon, opens a shell, clicks the terminal and types `echo hello` with WebDriver key actions. It checks the output line in both the daemon's screen and the xterm buffer, then closes the session.
  - It records `cold_start_ms` (`__kelpie.perf.interactiveAt` − app process creation), `idle_cpu_pct` and `rss_mb`: app + web processes + daemon over 30 s, one shell open.
  - It prints the numbers next to `scripts/perf/linux-smoke-baseline.json` and never gates on them.
  - It stops only what it started and removes its folders.
- **Docs**:
  - user-guide: Linux install (AppImage and .deb), data folder, Secret Service requirement, keep-awake, login shell, build deps and e2e driver;
  - architecture: a new **Platforms** table, plus the Linux transport, process, keep-awake, updater and release lines.

- **Bug found on Linux and fixed** (`app/src-tauri/src/git_finish.rs`). A Finish check that timed out or was cancelled stopped its process group with `kill -KILL -<pid>`.
  - procps' `kill` (Linux) parses `-<pid>` as options. Its "negative digit PID" special case turns `-1234` into `kill(-1, SIGKILL)`, which reaches **every process of the user**.
  - On CI it killed the GitHub runner itself: two runs hung with no logs. On a desktop it would end the whole session.
  - The fix is `libc::killpg(pgid, SIGKILL)` directly, with pid > 1. The app crate gains `libc` for unix targets.
  - Regression test `a_stopped_check_never_reaches_programs_outside_it` (unix): a bystander process in its own group survives three stopped checks.

## Review fixes

From `plans/reports/code-reviewer-261030-phase15a-linux.md`.

- **H1, AppImage environment** (`daemon_runtime::appimage_env_fixes`, used by `daemon_client::spawn_daemon` on Linux).
  - When `APPIMAGE` is set, the daemon starts without the AppImage runtime and GTK hook variables:
    - removed: `APPIMAGE`, `APPDIR`, `ARGV0`, `OWD`, `GTK_PATH`, `GTK_IM_MODULE_FILE`, `GDK_PIXBUF_MODULE_FILE`, `GIO_EXTRA_MODULES`, `GSETTINGS_SCHEMA_DIR`, `GTK_EXE_PREFIX`, `GTK_DATA_PREFIX`, `GI_TYPELIB_PATH`;
    - also removed: any other variable whose value points into `$APPDIR`;
    - removed only when they carry the hook's values: `GDK_BACKEND=x11` and `GTK_THEME=Adwaita:dark|light`.
  - `$APPDIR` entries are dropped from `XDG_DATA_DIRS`, `PATH`, `LD_LIBRARY_PATH`, `PYTHONPATH` and `PERL5LIB`, and an emptied list is removed.
  - Nothing is restored, because neither the type-2 runtime nor linuxdeploy-plugin-gtk keeps the values it overwrites.
  - The filter is pure, and its unit test (runs on every OS) covers:
    - the hook's real variable set, kept user values, and the `-other` prefix trap;
    - a user-chosen backend and theme, which are kept;
    - no change when not an AppImage.
- **M1, private data folder** (`kelpie_proto::ensure_private_dir`).
  - Unix creates the folder (and missing parents) with 0700. An existing folder that is ours and readable by others is narrowed to 0700; one owned by someone else is left alone.
  - It runs at daemon start, at app start, and for the auth-token folder. Windows is a plain create.
  - Test: new folder, its parents, and an existing 0755 folder (mode checks on Unix).
- **M2, signing key.**
  - `build-release.sh` takes `TAURI_SIGNING_PRIVATE_KEY(_PASSWORD)` out of its environment at once and passes them only to `pnpm tauri build`. The sidecar cargo builds and `pnpm install` run without the key.
  - The workflow's dependency install was already a separate step without the key.
  - Known exposure, documented in `release.yml`:
    - the `tauri build` command (frontend build, bundling, signing) has the key;
    - in that command Tauri itself downloads linuxdeploy and its GTK plugin, unpinned, and runs them. The build script downloads nothing, so there is nothing for us to pin.
  - Also, the step's own shell still has the key in its initial environment. Removing that would mean signing in a separate step with our own `.app.tar.gz` packing for macOS; that is a product/ops decision.
- **L1, slots on Linux**: the module doc and architecture now say what Linux does. The AppImage copy goes to slot `a` each new build; the running daemon keeps its unlinked file, and `b` is used only when `a` cannot be written.
- **L2, failed copy**:
  - `runnable` warns that a daemon left running from the AppImage mount stops when Kelpie quits.
  - `daemonSurvivesUpdate` returns false for an `exe` under `/.mount_…/` (vitest case).
  - `isWindows` moved to `lib/platform.ts`.
- **L3.**
  - `xauth` added to the CI apt list.
  - The optional `.deb` line in `release.yml` is `|| true`-safe under `bash -e`.
  - The release matrix has `fail-fast: false`.
  - `boot_smoke.py` `stop()` signals the process group even when its leader already exited, waits for it, then SIGKILLs the group with every `killpg` guarded.
- **Keep-awake.**
  - Round 1 narrowed it to `--what=idle`. Round 2 restored `idle:sleep`, as the controller decided (see Round 2 below).
  - While a lock is held, the daemon looks at the child every 5 s, so a refusal that arrives after the 300 ms probe is reaped and logged. There is no timer while nothing is held.
- **L5, `proc_tree`**: `ProcTree::adopt` refuses process group ids ≤ 1 and the daemon's own group, so every `killpg` site is guarded.
- **Not changed (as reviewed):**
  - the redundant Linux `keyring` entries in kelpied and the app (harmless);
  - L4, the `/tmp` fallback when `HOME` is unset (pre-existing, practically unreachable).

## Round 2 review fixes

From the Round 2 section of `plans/reports/code-reviewer-261030-phase15a-linux.md`.

- **R2-2, programs the app starts.**
  - One helper, `daemon_runtime::clean_env(&mut Command)`, applies `appimage_env_fixes` to the app's own environment, computed once. It is a no-op outside an AppImage and off Linux.
  - Every program the app spawns now goes through it:
    - the daemon;
    - external editors and both `xdg-open` reveal sites (`agent_config.rs`, `fs_ops.rs`);
    - git (`git.rs`, `git_sync.rs`, `integrations.rs`, plugin clone), which can start pinentry or credential GUIs;
    - `gh`;
    - Finish check commands;
    - plugin shell commands;
    - `ssh` / `scp` / `ssh-keygen` / `ssh-add` (`remote.rs`, `ssh_hosts.rs`).
  - The bundled speech helper keeps the app's environment (it is ours and lives inside the mount). URLs opened through `tauri-plugin-opener` are outside our control (follow-up).
  - New test: outside an AppImage a started program's environment is untouched.
- **R2-1, GTK lists.**
  - `GTK_PATH`, `GI_TYPELIB_PATH`, `GIO_EXTRA_MODULES` and `GSETTINGS_SCHEMA_DIR` are now treated like `PATH`: only their `$APPDIR` entries go.
  - Only the runtime's `APPIMAGE`, `APPDIR`, `ARGV0` and `OWD` are removed unconditionally. Every other variable is removed only when its value lies inside the mount (the hook's module and loader files and GTK prefixes).
  - The test covers a user's own `GTK_PATH` entry kept beside the hook's, and user-set `GI_TYPELIB_PATH`, `GSETTINGS_SCHEMA_DIR` and `GDK_PIXBUF_MODULE_FILE` kept.
- **R2-3, data folder.**
  - `ensure_private_dir` returns a warning instead of failing when the folder cannot be narrowed to 0700 (vfat, CIFS…). The daemon logs it after logging starts; the app prints it.
  - A folder owned by another user is refused: "… belongs to another user (uid N, not M): remove it, or point KELPIE_DATA_DIR or XDG_DATA_HOME at a folder of your own". This also closes the pre-created `/tmp/Kelpie` case (L4).
  - Test (Unix, non-root): `/` is refused with that message.
- **R2-4, signing-key wording.**
  - `build-release.sh` and `release.yml` now say the key is *not passed on* to the sidecar builds and `pnpm install`, rather than "never seen".
  - They name the `/proc/<pid>/environ` exposure and say a separate signing step is the real fix. The script warns never to run it with `bash -x` while the key is set.
- **R2-6, keep-awake (controller decision).**
  - Back to `--what=idle:sleep --mode=block`: a long agent run must never be cut off, and GNOME's automatic suspend may not honour an idle lock alone.
  - The 5 s late-refusal check stays.
  - The user guide says what this means: while Kelpie keeps the computer awake, a suspend chosen from the menu waits; closing the lid follows the system setting; keep-awake can be switched off (or set to *while an agent works*). The architecture doc says the same.
## CI

| Run | Result |
|---|---|
| [40962346789](https://github.com/baopeak/kelpie/actions/runs/40962346789) | Windows ✓, macOS ✓. Linux: the runner died during the Rust tests, no log (cancelled). |
| [40968292943](https://github.com/baopeak/kelpie/actions/runs/40968292943), [40972496914](https://github.com/baopeak/kelpie/actions/runs/40972496914) | Linux split into per-crate steps, then disk cleanup. The runner still died in the app tests (cancelled, no log). |
| [40976083881](https://github.com/baopeak/kelpie/actions/runs/40976083881) | The watchdog showed memory and disk fine (≈2 GB used, 105 GB free). Under `timeout` the test process alone was killed mid-run, which pointed at the `kill` call above. |
| [40977107950](https://github.com/baopeak/kelpie/actions/runs/40977107950) | Windows ✓, macOS ✓. Linux: every Rust test passed (proto 27, daemon 345, app 240 including the new regression test, stt 4), plus frontend checks, perf gate and Bundle (`.deb` 32 MB, version 0.1.8). The `.deb` check failed only on its own grep (`dpkg-deb` lists `usr/bin/…` without `./`). |
| [40985738280](https://github.com/baopeak/kelpie/actions/runs/40985738280) | With that grep fixed (7269ff5). Not watched, as the user asked. |
| [40992930356](https://github.com/baopeak/kelpie/actions/runs/40992930356) | After the review fixes (cf97e08). **Windows ✓.**<br>Linux: tests ✓, bundling ✓, `.deb` install check ✓, AppImage check ✓. The smoke passed 7/8 and took its numbers: only the prompt check failed, because the screen text has trailing spaces trimmed, so bash's `$ ` reads as `$`.<br>macOS: a timing-dependent `procs` orphan test failed. It is mac-only and untouched by this branch: the scan still had the forked `sh`'s pre-exec name (`bash`) while its exe was already `/bin/sleep`. |
| [40994932186](https://github.com/baopeak/kelpie/actions/runs/40994932186) | With both fixed (da1b258, 96ece38): **Windows ✓, macOS ✓, ubuntu-22.04 ✓**. Linux: every test (proto 28, daemon 345, app, stt 4), frontend checks, perf gate, bundles, the `.deb` install and AppImage checks, and the Xvfb smoke **8/8**. |

The diagnostics (per-crate steps, `continue-on-error`, watchdog, disk cleanup) were removed again in 332ac5b. Linux now runs the same single test step as Windows and macOS.

**Verified on Linux CI** (run 40994932186):
- every Rust test, including `kelpie-stt`, the XDG data-folder, private-folder, AppImage-environment, login-shell and keep-awake tests;
- the frontend checks and the perf gate;
- AppImage + `.deb` bundling. The `.deb` (version 0.1.8 after CI's bump) contains `usr/bin/kelpie`, `usr/bin/kelpied`, `usr/bin/kelpie-stt` and `Kelpie.desktop`, and depends on `libwebkit2gtk-4.1-0, libgtk-3-0`;
- `dpkg -i` installs it, every shared library of the three programs resolves, and `dpkg -r` removes it;
- the AppImage is an executable ELF, and `--appimage-extract` shows the app and both sidecars;
- the Xvfb smoke under WebKitWebDriver against the release app:
  - the app boots and connects to its own isolated daemon (same pid);
  - a shell session opens and its terminal renders;
  - the prompt appears; typing `echo hello` prints `hello` in the daemon's screen and in the xterm buffer;
  - the session closes.

**Only on a real machine or a tag:**
- the release job, which runs only on a tag;
- the manual desktop checks under Follow-ups.

**Checked locally (Windows), after the round-2 fixes:**
- `cargo test`: kelpie-proto 29 ✓, kelpied 352 ✓, kelpie 239 ✓ (`-j 1`; `-j 2` ran out of memory on the 8 GB machine);
- clippy: only the known `mobile.rs:892` warning;
- `pnpm typecheck`, `pnpm test` (515) and `pnpm i18n:check`;
- type-checked for `x86_64-unknown-linux-gnu` and `x86_64-apple-darwin`:
  - the Linux-only daemon code (`awake.rs`, `login_shell`/`passwd_shell`, the `killpg` drop, the `adopt` guard), in a scratch crate;
  - `kelpie-proto` itself (keyring with Secret Service, `ensure_private_dir`);
- `bash -n` on `build-release.sh` and every workflow `run:` block; both workflows parse as YAML; the Python scripts compile.

**Smoke numbers** (ubuntu-22.04 runner, 4 cores, Xvfb, software rendering; one shell; 30 s idle):

| Run | cold_start_ms | idle_cpu_pct (of one core) | rss_mb |
|---|---|---|---|
| 40992930356 (now the baseline, `scripts/perf/linux-smoke-baseline.json`) | 634.0 | 4.53 | 505.7 |
| 40994932186 (printed next to the baseline, not gated) | 1141.0 (×1.80) | 6.63 (×1.46) | 494.9 (×0.98) |

Shared runners vary a lot between runs: cold start nearly doubled with no code change on that path. Any future gate needs several runs per sample and wide budgets.

## Decisions

- **Plan question 3 (AppImage vs .deb): both, as the spec says.**
  - The AppImage is the update channel (`linux-x86_64`).
  - The `.deb` serves Debian/Ubuntu users who want apt-managed dependencies. It updates through `linux-x86_64-deb` when the bundler signs it.
  - Whether Linux users exist is still unknown; shipping both costs one CI job.
- **kelpie-stt on Linux**: built, tested (4/4 on CI) and bundled; no gate needed. sherpa-onnx ships a `linux-x64-static-lib` prebuilt, and `cpal` needs only `libasound2-dev`.
- **Xvfb smoke runs against the release binary** from the bundle step (`KELPIE_E2E=1` exposes `__kelpie`), not an extra debug build: no second app compile, and the cold start is the one users get. It sets `WEBKIT_DISABLE_DMABUF_RENDERER` and `WEBKIT_DISABLE_COMPOSITING_MODE` because there is no GPU under Xvfb.
- **No new PATH import with a timeout.** macOS resolves the PATH per launch through the login shell rather than importing it once, so "the same way" means sharing that path; only the missing-`SHELL` fallback needed Linux work.
- **Keep-awake lifetime**: `PR_SET_PDEATHSIG` instead of polling the daemon pid (`tail --pid` would be GNU-only). The lock is a logind fd held by `systemd-inhibit`, so killing it releases the lock at once.
- The `.deb` declares no `libasound2` dependency. Every desktop with WebKitGTK already has it, and the package name changed to `libasound2t64` on Ubuntu 24.04, so a hard dependency could break installs.

## Follow-ups

- macOS process scan: a process read just before its `exec` can keep its old name (`bash` for `sleep`) in the Processes list. The test now matches on the program path; the cached name itself is a small display issue to look at.
- **Signing key** (M2/R2-4): the release step's shell keeps the key in its starting environment, which any same-user process of the step can read through `/proc/<pid>/environ` (`KERN_PROCARGS2` on macOS). This includes dependency build scripts, pnpm lifecycle scripts, and the linuxdeploy that Tauri downloads unpinned.
  - The real fix is a separate signing step: bundle with no key anywhere in the job's environment, then `tauri signer sign` each updater artifact in a step of its own.
  - For macOS that means packing the `.app.tar.gz` ourselves. This is an ops decision.
- URLs opened through `tauri-plugin-opener` (`xdg-open` inside the plugin) still get the AppImage environment. A browser started that way outlives the app.
- Keep-awake on a real GNOME and KDE desktop:
  1. Set a 1-minute automatic suspend and hold keep-awake.
  2. Check `systemd-inhibit --list`.
  3. Confirm automatic suspend waits and a menu suspend waits.
- Run `ui_tour.py` screenshots on Linux before the first Linux release (WebKitGTK rendering differences).
- Gate the Linux smoke numbers once there is a history (budgets relative to `linux-smoke-baseline.json`).
- Manual checks on a real desktop: GNOME Keyring prompt/unlock flow, KWallet, keep-awake under GNOME/KDE (`systemd-inhibit --list`), notifications, AppImage self-update from a published release, `.deb` update through pkexec.
- Wayland: not exercised (Xvfb only); the AppImage no longer forces `GDK_BACKEND=x11` on sessions, but the app itself still runs under the hook's X11 setting.
- arm64 Linux builds are not produced.
