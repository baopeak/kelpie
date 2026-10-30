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
  - `--what=idle` only: automatic idle sleep is blocked, but a suspend the user picks from the menu still happens, like `ES_SYSTEM_REQUIRED` on Windows.
  - While a lock is held, the daemon looks at the child every 5 s, so a refusal that arrives after the 300 ms probe is reaped and logged. There is no timer while nothing is held.
- **L5, `proc_tree`**: `ProcTree::adopt` refuses process group ids ≤ 1 and the daemon's own group, so every `killpg` site is guarded.
- **Not changed (as reviewed):**
  - the redundant Linux `keyring` entries in kelpied and the app (harmless);
  - L4, the `/tmp` fallback when `HOME` is unset (pre-existing, practically unreachable).

## CI

| Run | Result |
|---|---|
| [40962346789](https://github.com/baopeak/kelpie/actions/runs/40962346789) | Windows ✓, macOS ✓. Linux: the runner died during the Rust tests, no log (cancelled). |
| [40968292943](https://github.com/baopeak/kelpie/actions/runs/40968292943), [40972496914](https://github.com/baopeak/kelpie/actions/runs/40972496914) | Linux split into per-crate steps, then disk cleanup. The runner still died in the app tests (cancelled, no log). |
| [40976083881](https://github.com/baopeak/kelpie/actions/runs/40976083881) | The watchdog showed memory and disk fine (≈2 GB used, 105 GB free). Under `timeout` the test process alone was killed mid-run, which pointed at the `kill` call above. |
| [40977107950](https://github.com/baopeak/kelpie/actions/runs/40977107950) | Windows ✓, macOS ✓. Linux: every Rust test passed (proto 27, daemon 345, app 240 including the new regression test, stt 4), plus frontend checks, perf gate and Bundle (`.deb` 32 MB, version 0.1.8). The `.deb` check failed only on its own grep (`dpkg-deb` lists `usr/bin/…` without `./`). |
| [40985738280](https://github.com/baopeak/kelpie/actions/runs/40985738280) | With that grep fixed (7269ff5). Not watched, as the user asked. |

The diagnostics (per-crate steps, `continue-on-error`, watchdog, disk cleanup) were removed again in 332ac5b. Linux now runs the same single test step as Windows and macOS.

**Verified on Linux CI so far:**
- every Rust test, including `kelpie-stt`, the XDG data-folder tests, the login-shell test and the systemd-inhibit hold/release test;
- the frontend checks and the perf gate;
- AppImage + `.deb` bundling. The `.deb` contains `usr/bin/kelpie`, `usr/bin/kelpied`, `usr/bin/kelpie-stt` and `Kelpie.desktop`, and depends on `libwebkit2gtk-4.1-0, libgtk-3-0`.

**Only verifiable on Linux CI, not yet seen passing:**
- `dpkg -i` with every library resolved;
- the AppImage executable check and `--appimage-extract` contents;
- the Xvfb smoke (`boot_smoke.py` with WebKitWebDriver) and its numbers;
- the release job, which runs only on a tag.

**Checked locally (Windows):**
- `cargo test` per crate at 332ac5b: kelpie-proto 28 ✓, kelpied 352 ✓, kelpie 237 ✓ (`-j 1`; `-j 2` ran out of memory on the 8 GB machine); clippy (only the known `mobile.rs:892` warning); `pnpm typecheck`, `pnpm test`, `pnpm i18n:check`;
- the Linux-only daemon code (`awake.rs`, `login_shell`/`passwd_shell`, the `killpg` drop) type-checked for `x86_64-unknown-linux-gnu` and `x86_64-apple-darwin` in a scratch crate;
- `kelpie-proto` (keyring with Secret Service) `cargo check --target x86_64-unknown-linux-gnu`;
- `bash -n` on `build-release.sh` and every workflow `run:` block; both workflows parse as YAML; the Python scripts compile.

**Smoke numbers: none yet.** The smoke step has not run on CI. The first green run uploads `linux-smoke` (`linux-smoke.json`: `cold_start_ms`, `idle_cpu_pct`, `rss_mb`). Commit its `metrics` as `scripts/perf/linux-smoke-baseline.json`; CI then prints the comparison, still without gating.

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

- Run CI once after review and confirm the install check, AppImage check and Xvfb smoke. Commit the first smoke numbers as `scripts/perf/linux-smoke-baseline.json`.
- Run `ui_tour.py` screenshots on Linux before the first Linux release (WebKitGTK rendering differences).
- Gate the Linux smoke numbers once there is a history (budgets relative to `linux-smoke-baseline.json`).
- Manual checks on a real desktop: GNOME Keyring prompt/unlock flow, KWallet, keep-awake under GNOME/KDE (`systemd-inhibit --list`), notifications, AppImage self-update from a published release, `.deb` update through pkexec.
- Wayland: not exercised (Xvfb only).
- arm64 Linux builds are not produced.
