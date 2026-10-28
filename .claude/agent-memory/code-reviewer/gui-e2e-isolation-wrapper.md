---
name: gui-e2e-isolation-wrapper
description: Kelpie ssh_hosts/ssh_panes/remote e2e do not isolate themselves; a scratchpad runpy wrapper sets env, starts kelpied + tauri-driver; KELPIE_SSH_DIR must not come from mkdtemp
metadata:
  type: feedback
---

`scripts/e2e/ssh_hosts.py`, `ssh_panes.py`, `remote.py` expect the caller to start tauri-driver with an isolated `KELPIE_ENDPOINT` / `KELPIE_DATA_DIR` / `KELPIE_SSH_DIR` (only `spec_panel.py`-style newer scripts start their own daemon + driver). Safe recipe used 2026-10-25: a scratchpad wrapper sets those in its own `os.environ` (plus `KELPIE_E2E=1`, `KELPIE_DAEMON_PATH` and `KELPIE_E2E_APP` = debug exes from `cargo metadata`, `KELPIE_E2E_DRIVER` = free port), pops `KELPIE_SESSION_*` / `KELPIE_ALLOW_REAL_DAEMON`, starts `kelpied serve` and `tauri-driver --port P --native-port Q`, then `runpy.run_path(script)`; teardown taskkills the driver tree and shuts the daemon down through the same endpoint (`kelpie-call.py` Conn).

Trap: creating `KELPIE_SSH_DIR` with Python `tempfile.mkdtemp` on Windows adds an `OWNER RIGHTS:(OI)(CI)(F)` ACE that the scripts' `icacls /grant:r` leaves in place; Windows OpenSSH then fails with "Bad owner or permissions on …/config" and ssh_panes cascades to 11 failures. Use plain `os.makedirs`. `ssh_hosts.py` rmtree's and recreates the dir itself, so it is immune.

Also seen 2026-10-25: `ssh_hosts.py` "Claude Code starts on the host" fails because the app refuses agents in a host's login folder (stale expectation, not a regression). After any run that may start real `claude`, check `~/.claude.json` for `fullscreenAutoDisabled`.

**Why:** Phase 10 review needed these suites re-run after a `ssh_test_server.py` change without touching the real daemon.
**How to apply:** Reuse the wrapper pattern for any e2e script whose docstring says "Start tauri-driver with …". Related: [[shell-hook-blocked-words]], [[e2e-overlay-selector-collisions]].
