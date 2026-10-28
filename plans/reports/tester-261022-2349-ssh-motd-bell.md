# Test report: SSH login banner (MOTD) and bell-after-input status

Date: 2026-10-22 (run ended 2026-10-23 00:10, TZ Asia/Saigon) · Branch: rebrand/kelpie (uncommitted) · No product or repo test-script edits.

Changes under test:
1. `crates/kelpied/src/agents.rs` `build_remote_launch`: plain remote shell prints MOTD (`~/.hushlogin` silences) before `exec $SHELL -l`; tmux new-session command too. Agents unchanged.
2. `crates/kelpied/src/status.rs` `Activity::on_bell` (+ `session.rs`): bell within 1s of input ignored.

## Results

| Check | Result |
|---|---|
| `cargo test -p kelpied` | 45/45 PASS (incl. `remote_shell_shows_the_login_banner_once`, `bell_answering_a_key_is_not_waiting`) |
| `pnpm build` (app: tsc + vite) | PASS |
| `cargo build -p kelpied --release` | PASS, no warnings |
| `cargo build -p kelpie --features tauri/custom-protocol` | PASS, no warnings |
| `scripts/e2e/remote.py` | 26/26 PASS |
| `scripts/e2e/ssh_panes.py` | 43/43 PASS |
| `scripts/e2e/ssh_hosts.py` | 25/25 PASS on clean data dir (first run 23/25, see below) |
| Bell check (scratch copy of remote.py flow, not committed) | 3/3 PASS |

Bell check detail (remote shell on test host, via daemon `session.input`):
- PASS bell 4s after input is ignored: status `idle`.
- PASS control, bell 3s after input: status `waiting`. Shows the BEL path reaches the daemon.
- PASS typing again clears it: `idle`.

## Failures

`ssh_hosts.py` first run: 2 FAIL (`generated ssh config has the host…`, `hosts import from the SSH config`). Not caused by the changes. Cause: `remote.py` persists `sshConfig` (Settings → Remote) into `data/app-state.json` pointing at its own scratch config. `ssh_hosts.py` shares the data dir and does not reset it, so the generated `Include` and import read the wrong file. Evidence: `app-state.json` holds `remote-scratch\ssh-remote-test\config`. Rerun on fresh `KELPIE_DATA_DIR` + endpoint + `KELPIE_SSH_DIR`: 25/25.

## Coverage gaps / caveats

- Real Tab not tested. Test server pipes stdin (no tty, no readline), so Tab never rings a bell there. The bell was emitted by `printf '\a'` sent via `session.input`. Same `Session::input` path that sets `last_input` as UI typing, so the timing rule is exercised. Optional UI Tab check skipped.
- MOTD not visibly exercised. Test host is Git Bash: no `/etc/motd`, no `/run/motd.dynamic`, so the `cat` is silent. Verified in Git Bash only that the snippet does not break `exec`, with and without `~/.hushlogin`. Banner text printing is covered by the unit string assert only.
- tmux branch not exercised: host has no tmux ("tmux not installed"), so every remote shell took the no-tmux path.
- No pre-fix run: the bell check is not shown red on the old code. Discrimination rests on the unit test and on `status()` logic (old `bell = Some(now)` → waiting).
- Latency: bell window is 1s. A high-latency link where the shell's reaction arrives >1s after the key still shows "needs you". By design; not measured.
- Tested UI build (debug, 00:02) predates `app/src/components/icons.tsx` and `sidebar.tsx` edits (00:12). Daemon binaries (23:58 release) include the two changed daemon files. Those UI edits are not covered by this run.

## Process hygiene

Started by me, all stopped (`taskkill /T /F` after graceful refused):
- tauri-driver: 13788, 15096, 13712 (stray, env-less start, no app spawned), 1192. Port 4446.
- msedgedriver 155.0.4283.62 (downloaded to scratch): 4000, 1164, 5192, 8984. Port 4447.
- Scratch `kelpied.exe` daemons (repo release build): 12436, 13408, 14972 (+ children 11960, 2204, 3200).
- SSH test servers stopped by scripts. Ports 47823, 47824, 47844 not listening at end.

Not touched: user's `kelpie.exe` (PID 1700) and `kelpied.exe` (PID 1692) under `AppData\Local\Kelpie`; Python processes 11552, 10356 (not mine).

Scratch: `…\scratchpad\ssh-motd-bell\` (data, ssh, data-hosts, ssh-hosts, logs, msedgedriver, bell-check copy).

## Unresolved questions

- Should `remote.py` restore `sshConfig` on exit, or should scripts that share a data dir reset it? Current state leaks into `ssh_hosts.py`.
- Is a real MOTD check wanted? Needs `/etc/motd` on the test host (Git install root, not touched here) or a pty-backed bash in `ssh_test_server.py`.
- Is a real-Tab e2e wanted? Needs readline on the test host (pty-backed shell).

Status: DONE_WITH_CONCERNS
Summary: rebuilt app and daemon; `remote.py` 26/26, `ssh_panes.py` 43/43, `ssh_hosts.py` 25/25 (clean data dir), bell checks 3/3, `cargo test -p kelpied` 45/45. No failures caused by the MOTD or bell changes.
Concerns/Blockers: MOTD banner text and real Tab bell not exercised (test host has no MOTD files and no readline tty); no pre-fix red run; tested UI predates `icons.tsx`/`sidebar.tsx` edits made at 00:12.
