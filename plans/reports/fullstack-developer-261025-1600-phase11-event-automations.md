# Phase 11 – event-triggered automations: implementation report

Worktree `D:\kelpie-par`, branch `phase11/event-automations` (from rebrand/kelpie @ 6c635a1). Not committed. plan.md / phase status untouched.
API level: `kelpie-proto` `API_LEVEL = 15` (daemon) and `AUTOMATION_API = 15` in `app/src/lib/automation-events.ts` (app): one constant per side, renumber there.

## What was built

**Daemon** (`crates/kelpied`)
- `automations.rs` (rewritten around the old model, schedules unchanged): `trigger {event: file|turn|commit|pr_checks_failed|finished, glob, daily_cap}`, `action agent|quick|shell`, `shell {program, argv, approved_hash}`, `dry_run`, `last_note`, per-day cap counters. Storage `{version:2, paused, automations}`; v1 list migrates after a `.bak` copy (never over an earlier `.bak`); unparsable file -> renamed `automations.json.corrupt-<ts>`, every write refused (`save/upsert/remove/approve/set_paused`), error kept for the inbox; newer-version file left untouched and also refuses writes. Glob matcher (no backtracking, linear), `Automations::on_event` (pure: enabled, not paused, not busy, 60 s gap, daily cap with one notice, approval), `shell_plan` (no shell; whole-argument `{file}` `{files}` `{commit}` `{branch}` only; `-x` file names given as `./-x`; `KELPIE_*` env), `Heads` (commit = HEAD moved on the same branch via `rev-parse HEAD --abbrev-ref HEAD`).
- `daemon/automation.rs` (new child module of `daemon.rs`): RPCs `automation.list|state|save|delete|run|approve|set_paused|event`; event pipeline; busy tracking (sessions an automation started/typed into, running commands, 5 s quiet after); command runner (spawned, 5 min limit, output tail redacted into the run note); quick action (`Session::type_when_idle`: bracketed paste when the agent asked for it, ESC/control chars dropped, Enter as a separate guarded write); commit poll every 10 s plus probes at turn start/end (only while a commit automation exists); inbox announcement of a damaged file once the app is connected; `run_due_automations` (each due run on its own thread, ticker never awaits).
- `daemon.rs`: wiring only; ticker no longer `.await`s each due automation; turn start/end hooks; `automation.*` dispatch -> `automation_rpc`.
- `auth.rs`: `automation.*` (all 8) added to the admin matrix test; `norm`/`within_norm` extracted (no behaviour change). Session/claimed/anonymous never reach them (none is in SCRIPTING/OWN/CLAIMED lists); `save` of a command, `approve`, `event` also need the connection to be the app (`auth` `client:"app"`, sent by the window's connection only, cleared on any re-auth); `event` accepts only `file|pr_checks_failed|finished`.
- `server.rs`, `main.rs`, `session.rs` (`type_when_idle`, test-only `fake_running`/`NoKill`), `vt.rs` (`bracketed_paste`), `kelpie-proto` (API 15).

**App** (`app/src-tauri`, `app/src`)
- `watcher.rs`: each existing 150 ms batch now also carries the changed files (max 200/batch) to the daemon (`automation.event`, fire and forget). No new watcher roots. `daemon_client.rs`: auth says `client:"app"`.
- `lib/automation-events.ts` (new, tiny, in entry): list/state, global pause, `report()`, 5-min `gh`/MR look for projects that have a PR automation. `pull-requests.ts` reports a failing PR once per PR; `finish.ts` reports `finished` (after the merge is recorded, root = base project); `status-bar.tsx`: bolt chip = global switch (shown once any automation exists, "paused" text when off); `settings/automation-settings.tsx`: trigger/event/glob/cap, honest scope note per trigger, action picker (agent / saved prompt / command), program + arguments, command hash + **Approve command**, Dry run checkbox and button, pause toggle, error banner; options disabled below API 15.
- Locales `app/src/locales/vi/automations.json` (+ `backend.json` for daemon messages), catalog/command label "Automations", CSS, `main.tsx` (e2e handle).
- Docs: `README.md`, `docs/user-guide.md` (Automations), `docs/architecture.md` (daemon list, RPCs, events, storage, API 15).

## Numbers

| Check | Result |
|---|---|
| `cargo test -p kelpied -p kelpie -p kelpie-proto` | kelpied 223 (31 new: 20 `automations::tests`, 11 `daemon::automation::tests`), kelpie 113, kelpie-proto 19: all pass |
| `cargo clippy -p kelpied -p kelpie -p kelpie-proto --all-targets -- -D warnings` | clean |
| `pnpm typecheck` / `pnpm test` / `pnpm i18n:check` | clean / 360 pass / 1898 keys, 0 missing |
| Perf: match 100 triggers x 6 files (`on_event`) | best 0.44 ms, mean ~0.6 ms in the **debug** test build (assert 10 ms debug, 1 ms release); release run not done (disk) |
| Perf: ticker with 10 due automations (`run_due_automations`) | 2.0 ms vs 0.05 ms with none due; before, each due run was awaited in turn (a session start each) |
| Perf: watcher roots | unchanged (`watch()`/`create()` untouched; files piggyback on the existing batch) |
| Bundle (`perf_gate.py --label phase11 --metrics bundle`) | entry 321.97 KB gz (limit 338.60), report in scratch `p11/perf.json` |
| `scripts/e2e/event_automations.py` | **69/69, green three times** on the final code (two earlier runs fixed script mistakes: snapshots are off by default; wait past the quiet seconds after a command) |
| Regression | best_of_n 91/91, finish 72/72 (see concern 5), checkpoints 55/55, notify 73/73, context_cost 26/26, attention 12/12 |

Unit tests cover: corrupt file never overwritten (all write paths, no new file created, bytes kept), newer-version file untouched, 7 damaged shapes, migration keeps schedules incl. `deliver` + `.bak` never replaced, round trip with triggers/pause, validation, glob semantics + hostile pattern, matching scope (sibling prefix, disabled, paused), gap/cap/busy/day rollover/restart, approval (forged hash ignored, edits drop it), literal event data (`$(…)`, backticks, `;`, quotes, `%PATH%`, leading `-`), event sanitising, `Heads`, `refs/kelpie` never moves HEAD (real git), session/claimed/anonymous refused for every method x 14 param shapes x scripting on/off, app-only gates, file event -> approved command runs once (a real process), dry run runs nothing, pause (persisted), self-edit guard, turn scope, quick action (idle only, one line, ESC dropped, Enter follows), damaged file reaches the inbox once, commit via real git.

e2e (real watcher, real daemon, stand-in `amp` agent, fixture repo, no network): unapproved command does not run and says why; approve in settings; saving `*.md` runs once (hostile file name arrives literally in argv and env, nothing executed, no session variables); again after the gap; non-matching file nothing; dry run; an automation-started agent editing matching files twice over 9 s is not started again, a later user save starts it again; quick action types into an idle agent; turn trigger; checkpoint refs exist and do not fire commit; checkout does not; hand commit and agent commit do; cap 2; status-bar switch pauses/resumes; PR/finished events from the window; plain-token script cannot save a command/event/approve; claimed and session callers refused for every method/shape; editor text and save; damaged file and v1 migration on throw-away daemons.

## Concerns / deviations

1. **Old daemons cannot reject `trigger` automations themselves** (they ignore unknown fields). Safety is the API bump plus the app offering triggers/commands only at API >= 15. An old daemon after a downgrade reads the v2 object as damaged (its loader returns empty and its next save overwrites): the `.bak` holds the pre-migration v1 list only.
2. `github.rs` is untouched: PR state lives in `pull-requests.ts`, so the failing-checks report is there. There was no background `gh` polling (only on Git panel open/refresh); I added a 5-min look limited to projects that have a PR automation.
3. "Quick command" = text snapshot of an agent-type quick command (stored in `prompt`, label in `quick_label`); terminal-type quick commands are not supported (that would be a hidden shell command; use *Run a command*).
4. Global switch is a status-bar chip (there is no status-bar menu); it pauses clock runs too. *Run now* still works.
5. `finish.py` failed once at "Request changes" (`Send to` menu not found in 10 s) and passed 72/72 on the immediate rerun: timing flake under load, nothing automation-related in that path.
6. Events dropped by a guard (60 s gap, busy, cap) are not replayed, including a commit seen while the automation was quiet (as specified: max 1 run per minute). `KELPIE_AUTOMATION_GAP_MS` is a test hook (default 60 000) so e2e does not wait minutes; the real minute is unit-tested.
7. Commits made by hand in a *linked* worktree are seen only by the 10 s poll of an automation whose folder is that worktree (the app watcher does not see a linked worktree's refs). File/PR/finish triggers fire only while the window runs; the settings page says so.
8. Agent prompts get a short "Triggered by…" note with file names (untrusted text, control chars stripped); that is prompt text, not a shell, but a hostile file name can still be read by the agent.
9. e2e/unit stand-ins only; no real vendor was run. Release-mode matching time not measured (disk).
10. No commit/push made; `D:\kelpie-par` has uncommitted changes plus new files (`daemon/automation*`, `automations/`, `automation-events.ts`, `locales/vi/automations.json`, `scripts/e2e/event_automations.py`).

## Review round (code-reviewer-261025-1700): all High, Medium and Low items addressed

**High (Windows commands)**
- `resolve_program`: a path with a separator is joined onto the automation's folder (never the daemon's), a bare name is searched in `PATH` with `PATHEXT` (`npm` -> `npm.cmd`; only .com/.exe/.bat/.cmd). Started as the resolved absolute file; "not found" says where it looked. The approval hash now covers event kind, folder, the resolved file and every argument.
- `code_risk` (daemon refuses at save **and** again when planning/running): an event placeholder after `powershell`/`pwsh` `-Command|-EncodedCommand|-ec|-c` (any abbreviation; `-File script args` is fine), `bash|sh|zsh|dash|ksh|fish -c` (incl. `-lc`), `python -c`, `node -e/-p/--eval`, `ruby|perl|lua|Rscript|osascript -e`, `php -r`, `deno eval`, through wrappers (`env`, `sudo`, `xargs`, `start`…), and any placeholder at all on a `cmd`/`wsl` command line. Message names a script file that reads `KELPIE_*`; UI text and docs say the same.
- Tests: `event_data_is_refused_where_a_program_would_run_it_as_code` (26 refused, 14 allowed forms), `an_unsafe_command_cannot_be_saved_or_planned`, `a_program_is_looked_for_where_the_user_would_expect`, and the Windows test `a_real_batch_file_and_a_real_powershell_script_get_hostile_file_names_literally` (a real `.cmd` by relative path and a real `.ps1` through `powershell -File`, hostile name with `$(New-Item …)`, backticks, `&`, `^`, `%PATH%`, `;`, `'`: arrives literally in argv and `KELPIE_FILE`, nothing created). e2e section 1b does the same through the real watcher (relative `.cmd`, bare `evhook` via PATHEXT, `.ps1`), plus refusal of `powershell -Command`, `cmd /c`, `bash -c` on save.

**Medium**
- M1 downgrade: every save also writes `automations.json.v2`; if the main file is in the old format and a non-empty `.v2` exists, nothing is written, an inbox notice is raised (again after each restart) and `automation.state.fix = "restore"`; `automation.resolve restore` merges the `.v2` list with what the older Kelpie added, `keep` sets `.v2` aside (`.v2.kept-<ts>`). Settings shows **Restore the newer list / Keep the current list**. Docs say downgrading is unsupported but loses nothing.
- M2: the notice is announced only when an **app** connection exists (not any connection); a set-aside `.corrupt-*` file keeps writes refused after restarts until the app sends `automation.resolve dismiss` (renames it `.seen`); Settings shows a **Dismiss** button. Scripts cannot dismiss.
- M3: busy = alive and `Working` or `Waiting`.
- M4: the turn-start HEAD record is gone; the 10 s poll keeps the baseline; a turn end probes only commit roots that contain the session's folder (so no git for unrelated sessions); a detached HEAD (`HEAD` branch) never counts.
- M5: quick actions type only into a session **this automation started**, only when idle and nothing was typed in the last 30 s; with none of its own alive they start one; with one busy they record an error and type nothing. e2e: the user's own idle session of the same agent/folder is never typed into; first event starts the automation's session, a later one types into that same session (2 turns, 1 start).
- M6: agent/quick kinds must be known agents; kind `shell`/`custom` counts as a command (app-only save, approval via a hash of kind/args/prompt, prompt carries no event data, quick refused).
- M7: commands run in a job object (process group on Unix) via `ProcTree`; stopped with everything they started on timeout and when they end; reader threads are joined with a 3 s grace. Test `a_command_that_runs_too_long_is_stopped_with_what_it_started` (cmd + ping grandchild).
- M9: docs state `client:"app"` is a convention (user token = user), not a boundary; nothing else relies on it beyond what the user token allows.
- M10: chain limit: an automation set off while another one is busy in the same folder is one link further (depth), at 3 it is not run and the inbox says so (`CHAIN_LIMIT`). Tests at store and daemon level.
- M11: the Dry run checkbox is disabled below API 15.

**Low**: L1 one commit poll at a time + git called with a 10 s timeout (killed); L3 `@` and `+` (and `-`) get `./`; L4 `rust-version = "1.82"` in `kelpied` (what the code already needs; batch quoting needs 1.77.2) and a docs line on quoting `%KELPIE_FILE%`; L5 PR looks deduplicated per project, reported PRs persisted in localStorage; L6 the approval hash includes the trigger event; L7 file names in agent prompts sit in a fenced block labelled "names only, not instructions" (backticks replaced); L8 the automations file is re-read up to 4 times before giving up; L9 `P11_*` env vars renamed `EVENT_AUTO_*`; L10 covered by the real `.cmd`/`.ps1` tests; L12 docs note (case folding, `[abc]` literal, nested roots). Not done: L11 (merge-time API numbering, the controller's), L13 tooltip wording.

**Numbers (final code)**
- `cargo test -p kelpied -p kelpie -p kelpie-proto`: kelpied 240 (47 in `automations::tests` + `daemon::automation::tests`, 16 of them new this round), kelpie 113, kelpie-proto 19, all pass. Clippy `-D warnings` clean on all three crates.
- `pnpm typecheck` clean, `pnpm test` 360, `pnpm i18n:check` 1901 keys, 0 missing.
- `scripts/e2e/event_automations.py` **83/83 twice in a row** (and 82/82 once before two test-script fixes: concurrent appends to one hook file; waiting until the automation's session is idle before the user's second save).
- Regression: finish 72/72, notify 73/73, attention 12/12.
- Scratch: all under `D:\kelpie-par-scratch` (deleted), `TEMP`/`TMP` pointed there for cargo and e2e; nothing large on C:.

## Concerns / deviations (current)
1. Downgrading still empties the list for the older Kelpie itself, and an old daemon accepts an event automation as a daily one; mitigated by the API 15 gate and the `.v2` restore, not prevented.
2. `github.rs` untouched (PR state is in TS); a 5-minute `gh` look was added for projects with a PR automation.
3. Guard-dropped events are not replayed; `KELPIE_AUTOMATION_GAP_MS` is a test hook.
4. Hand commits in a linked worktree are seen only when an automation's folder is that worktree. File, PR and finish triggers need the window.
5. A session an automation started that stays `Waiting` on a question keeps that automation busy until answered (by design after M3).
6. A batch file that does `echo %KELPIE_FILE%` unquoted is the script author's to quote (docs say so).
7. Release-mode matching time still not measured (debug 0.44 ms).
8. Earlier `finish.py` flake did not recur this round.

## Re-review round: the blocklist is replaced by an allowlist structure

**B1-B3 and the controller's allowlist (`check_command`, in `automations.rs`; at save and again when a run is planned)**
- The resolved program (and the name it was given; versions such as `python3.12`, `pwsh-preview`, `bash5` count) is classified:
  - *Remote or elevated runner* (cmd, ssh, plink, wsl, wslg, su, sudo, doas, pkexec, runas, schtasks, at, start, conhost, wt, watch, script, forfiles, xargs, env, nohup, time, timeout, nice, ionice, stdbuf, command, exec, call, setsid, chroot, nsenter, psexec, mosh, npx, bunx, uvx, pipx, cross-env, conda…): event data (a `{placeholder}` **or** `KELPIE_*` anywhere in the arguments) refuses the command. Because they are refused outright, nesting them cannot get around anything (the old 4-level loop is gone).
  - *PowerShell* (powershell, pwsh, powershell_ise): only `[-NoProfile|-NonInteractive|-NoLogo|-ExecutionPolicy <value>]… -File <x.ps1>`, the script must exist inside the project (or on PATH) and not be a folder; so `powershell -NoProfile lint.ps1 {file}` (B1) is refused.
  - *POSIX shells* (bash, sh, zsh, fish, dash, ksh, nu, git-bash…): only `[-e|-u|-eu|-o <name>] [--] script args`, never `-c` in any form or `-` (stdin).
  - *Script interpreters* (python/py/pythonw, node, deno, bun, ruby, perl, php, lua, osascript, cscript, wscript, mshta…): the first argument is a script file, no flags.
  - A **batch file called directly** keeps its safe quoting; `%` or `!` written into its arguments are refused. Anything else (git, `npm.cmd`, a real exe) gets whole-argument placeholders; `-`, `@`, `+` names get `./`.
  - `cmd /c x %KELPIE_FILE%` (B2), `ssh host ./x {file}` (B3) and the reviewer's probes are cargo tests (`the_reviewers_probes_cannot_be_saved`, plus a 56-form refused list and a 22-form allowed list in `event_data_reaches_interpreters_only_as_the_data_of_a_script_file`). The message says: script file inside the project, run with `powershell -File`, `bash script`, `python script`, or a `.cmd` called directly, reading `KELPIE_*` from inside; it never suggests `cmd /c` or `%VAR%`.
- **Approval hash** now includes the contents of the program when it sits in the project, of the script an interpreter runs and of every argument that names a file (4 MB cap), plus the typed-shell environment. An agent editing an approved script gives "the command or a script it runs changed since it was approved: approve it again": the automation does not run, the inbox says so once, until approved again (`editing_an_approved_script_needs_approval_again`; the program inside the project too). The dialog shows the resolved program (or "not found"), the covered files and the hash (N3, N7).
- Real-process tests: a real `.ps1` through `powershell -File`, a real `.cmd` called directly (relative path) and a real bash script (Git for Windows bash; skipped where absent), each given a name with `$(…)`, backticks, `;`, `'`, `&`, `^`, `%PATH%`: it arrives literally in argv and `KELPIE_FILE`, nothing is created. e2e does the relative `.cmd`, the bare `evhook` (PATHEXT), the `.ps1`, and the refusal of six forms (powershell -Command, powershell without -File, cmd /c, cmd /c %KELPIE_FILE%, ssh, bash -c).

**Mediums and Lows**
- N2: bare-name lookup is cached per (PATH, program) (a hit forever, a miss for 30 s) and bounded to 2 s on a helper thread; the app save path warms it before taking the automations lock.
- N4: a command that ends normally keeps what it launched (`ProcTree::release` clears kill-on-close; the Unix group is not hung up); only a timeout kills the whole tree.
- N5: `check_head` holds one lock from asking git to recording the answer, and a baseline is taken only where there is none (a re-save no longer swallows a commit): `a_commit_made_while_a_baseline_is_taken_is_not_swallowed`.
- N6: chain depth no longer depends on something still being busy: an automation that fired in the last two minutes in the folder counts as a link (kept per automation with its depth); a finished turn can only be caused by an automation that starts agents, window events by none. `the_chain_follows_recent_runs_even_when_nothing_is_busy_any_more`, `a_command_cannot_have_caused_a_finished_turn`.
- N8: gone with the allowlist. N9: `due()` does not run the older list while a restore choice or a refusal is pending (tested). N10: `head_of` has `core.hooksPath=/dev/null` and `core.fsmonitor=false` again (and still its 10 s timeout).

**Numbers (final code)**
- `cargo test -p kelpied -p kelpie -p kelpie-proto`: kelpied 248 (56 automation tests; 8 of them are new this round), kelpie 113, kelpie-proto 19; clippy `-D warnings` clean. One earlier full run had `five_run_worktrees_with_links_are_ready_within_three_seconds` (an existing git-worktree test) fail twice under load; it passes alone and in the final full run.
- `pnpm typecheck` clean, `pnpm test` 360, `pnpm i18n:check` 1902 keys, 0 missing.
- `scripts/e2e/event_automations.py`: **86/86 on two consecutive runs** (earlier runs this round showed: scripts must live in the project now (the e2e's recorder and `.ps1` moved there), a chain false positive between a turn automation and a commit automation that the "can cause" rule fixed, and a 40 s wait for cold PowerShell raised to 90 s).
- Regression: finish 72/72, notify 74/74, attention 12/12 (11/12 once right after the heavy runs, 12/12 on the immediate rerun).
- Scratch was `D:\kelpie-par-scratch` (TEMP/TMP pointed there); the safety check blocks me from deleting that folder, so please delete it.

## Concerns / deviations (current)
1. Interpreters get event data only in the strict script-file forms above; `deno run x.ts`, `python -u x.py`, `bash -x x.sh` are refused with the message (a deliberately small allowlist). Scripts found only through PATH are checked but their contents are not part of the approval.
2. Downgrading is mitigated by the `.v2` restore, not prevented; an old daemon still treats an event automation as a daily one (API 15 gate).
3. Guard-dropped events are not replayed; `KELPIE_AUTOMATION_GAP_MS` is a test hook.
4. File, PR and finish triggers need the window; hand commits in a linked worktree need an automation on that worktree.
5. A session an automation started that stays `Waiting` keeps it busy until answered.
6. The chain window (2 minutes) can cut off a fourth automation that only coincidentally follows three others in the same folder; the inbox says so.
7. A batch file that does `echo %KELPIE_FILE%` unquoted is the script author's to quote (docs say so). Release-mode matching time still not measured.

## Final check round (F1, F3, pins)

- **F1:** `RUNNERS` now also holds the build tools and task runners (make, gmake, nmake, mingw32-make, cmake, ctest, cpack, gradle, gradlew, mvn, mvnw, ant, sbt, lein, msbuild, dotnet-msbuild, ninja, samu, meson, scons, xmake, bazel, bazelisk, buck, buck2, pants, just, rake, task, invoke, inv, nox, tox, doit, mage, fab, paver, grunt, gulp, fastlane, tup, redo) and the package managers (npm, pnpm, yarn, corepack; pnpm/yarn `exec`/`dlx` included). They get no event data: a placeholder or `KELPIE_*` in their arguments refuses the command (test: 35 more refused forms).
- **Probe of `npm run` / `exec` (real npm 11 / pnpm, hostile name with `$(…)`, backticks, `;`, `'`, `&`, `>`, `^`, `%PATH%`, `!PATH!`):** nothing was executed, but **`%PATH%` was expanded by cmd.exe inside `npm run`, `npm run-script`, `npm exec` and `pnpm run`/`exec`** (the script received the expanded PATH instead of the name; `pnpm run lint -- name` also passed a literal `--`). The name is therefore not delivered literally, and an expanded value is parsed by cmd again. So npm, pnpm, yarn and corepack are runners too, as you specified for that case; the earlier "npm.cmd run lint {file} is safe" claim is withdrawn (docs and tests changed). A `.cmd` file called directly stays literal (real-process test).
- **F3:** `program_stem` ignores trailing dots and spaces (`cmd.`, `cmd `, `CMD.EXE.`, `powershell .exe`); the classifier looks at the given name, the resolved name and the **canonical** path (`std::fs::canonicalize`, `\\?\` stripped), so an 8.3 name or a link cannot hide a runner or PowerShell. Tests: `trailing_dots_spaces_and_case_do_not_hide_a_program`, 10 refused name forms in the list, and the Windows test `an_8_3_short_name_is_the_program_it_names` (real 8.3 aliases such as `POWERS~1.EXE` and `MINGW3~1.EXE`, created under the system drive's temp folder because D: has no short names; it skips where a volume has none).
- **Pin:** `scripting_host_files_are_never_programs` (Windows): `.js .vbs .vbe .wsf .hta .lnk .url .ps1 .msc .jse .wsh` never resolve, by relative path, absolute path, bare name on PATH, or by the extension-less name; a `.cmd` still does.
- **TOCTOU:** a comment at `start_shell` states the same-user window between the approval read and `CreateProcess` and that the hash guards against drift, not against a deliberate same-user swap.
- Docs updated (runners, package managers, canonical classification, resolvable extensions).

**Numbers:** `cargo test -p kelpied` 251 pass (39 in `automations::tests` for this area plus the daemon tests), clippy `-D warnings` clean on kelpied/kelpie/kelpie-proto; `pnpm typecheck` clean, `pnpm test` 360, `pnpm i18n:check` 1902 keys, 0 missing; `event_automations.py` 86/86 once on the rebuilt daemon.

## Concerns / deviations (current)
1. The allowlist is deliberately small: `deno run x.ts`, `python -u x.py`, `bash -x x.sh`, `npm run …` and every build tool are refused when they would be given event data (write a script and call it in the allowed form).
2. Downgrading is mitigated by the `.v2` restore, not prevented; an old daemon treats an event automation as a daily one (API 15 gate).
3. Guard-dropped events are not replayed; `KELPIE_AUTOMATION_GAP_MS` is a test hook.
4. File, PR and finish triggers need the window; hand commits in a linked worktree need an automation on that worktree.
5. A session an automation started that stays `Waiting` keeps it busy; the chain window (2 min) can cut off a coincidental fourth automation.
6. Author-written flags that run commands (`git -c`, `find -exec`, `vim -c`, …) are not agent-reachable and are not refused.

## F4 round and the warning for unvetted programs

- **F4:** wrappers that run another program are runners now, so they get no event data: uv (any form), poetry, pdm, hatch, rye, pipenv, pixi, bundle, bundler, mise, rtx, asdf, volta, fnm, nvm, nodenv, pyenv, rbenv, goenv, jenv, sdkman, direnv, nix, nix-shell, nix-env, guix, dotenv, dotenvx, devbox, flox, earthly, act, doppler, op, aws-vault, infisical, chamber, envchain, with-contenv, firejail, bwrap, proot, plus dagger, docker, podman, nerdctl, kubectl, toolbox, distrobox, nsjail, unshare, chpst, gosu, dumb-init, tini, fakeroot, faketime, taskset, numactl. Refused-form tests include `uv run bash -c {file}`, `poetry run python -c {file}`, `bundle exec ruby -e {file}`, `direnv exec . sh -c {file}` and more.
- **Warning for programs with no rule:** `check_command` returns `unvetted` (event data goes to a program that is neither an interpreter running a script nor a batch file called directly); `automation.list` carries it as `command_unvetted`; the approval dialog then starts with the translated warning "This program will receive file names chosen by agents as its arguments. Approve only if it never runs them as commands." (en + vi). Test `only_programs_with_no_rule_ask_the_user_to_take_the_risk`: unknown programs and `git` flag it; `python lint.py`, `powershell -File`, `bash lint.sh`, a `.cmd` called directly, and commands without event data do not.
- **Build note for the controller:** the shared `D:\kelpie-p9-target` can hold artifacts of another tree built with the same package paths: my `kelpied.exe` once reported API 14 (a stale `kelpie-proto` rlib reported "Fresh"); touching `crates/kelpie-proto/src/lib.rs` and rebuilding fixed it (daemon.info 15). Everything below ran on the corrected binaries.

**Numbers:** `cargo test -p kelpied` 252 pass; clippy `-D warnings` clean (kelpied, kelpie, kelpie-proto); `pnpm typecheck` clean, `pnpm test` 360, `pnpm i18n:check` 1903 keys, 0 missing; `event_automations.py` 86/86 on the rebuilt daemon and app.

Committed on `phase11/event-automations` (code, tests, e2e, docs, locales; and this report).

## Concerns (current)
1. The allowlist is deliberately small: scripts only in the strict forms, runners and tools refused; programs with no rule are allowed with a warning that the user must accept.
2. Downgrading is mitigated by the `.v2` restore, not prevented; an old daemon treats an event automation as a daily one (API 15 gate).
3. Guard-dropped events are not replayed; `KELPIE_AUTOMATION_GAP_MS` is a test hook. File, PR and finish triggers need the window.
4. A `Waiting` session keeps its automation busy; the 2-minute chain window can cut off a coincidental fourth automation.
5. API number: `API_LEVEL = 15` and `AUTOMATION_API = 15`; renumber both at merge.

Status: DONE_WITH_CONCERNS
Summary: F4 wrappers are runners (with the four named refused cases tested), programs with no rule now carry a translated approval warning while known-safe forms do not, and every check is green; the work is committed on `phase11/event-automations`.
Concerns/Blockers: see the list above; no blockers.
