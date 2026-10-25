# Code review: Phase 10 "Spec workspace" (Plan tab, Run with…, host runs)

Date: 2026-10-25 · Branch rebrand/kelpie @ 6c635a1 (uncommitted) · Report-only (no source edits)
Spec: [phase-10](../261023-0048-multi-agent-ide-roadmap/phase-10-spec-workspace.md) · Developer report: [fullstack-developer-261025-1515](fullstack-developer-261025-1515-phase10-spec-workspace.md)

## Scope
- Daemon: `crates/kelpied/src/runs.rs` (TaskHost, build_run, task_prompt), `auth.rs` (run.create refusal), `daemon.rs` `start_task` hunk, `kelpie-proto` API line.
- App: `lib/spec-index.ts`, `components/plan-panel.tsx/.css`, `lib/runs.ts` hunks, `right-panel.tsx`, `quick-commands.ts`, `commands.ts`, `types.ts`, `main.tsx`, `styles.css` (.panel-tabs only), `locales/vi/spec.json`.
- Tests/e2e: `spec-index.test.ts`, `runs-host.test.ts`, `spec_panel.py`, `fake_host_agent.py`, `ssh_test_server.py` change.
- Read for context: `agents::build_remote_launch`, `app/src-tauri/src/remote.rs` + `fs_ops.rs` (fs over ssh / local), `mentions.ts`, `session.rs` typed prompt.
- Other sessions' work (phone remote control, kelpie-85 test hunks, usage-meter countdown, agent-memory) not reviewed.

## Verification run by the reviewer
| Check | Result |
|---|---|
| `cargo test -p kelpied` | 242 passed, 3 ignored |
| `vitest` spec-index + runs-host | 55 passed |
| `pnpm typecheck`, `pnpm i18n:check` | clean; 0 missing |
| `spec_panel.py` (own isolated daemon/driver, `--native-port` free) | **109/109** |
| `remote.py` (isolated wrapper) | 26/26 |
| `ssh_hosts.py` (isolated wrapper) | 24/25: "Claude Code starts on the host" fails, see L6 (not caused by this phase) |
| `ssh_panes.py` (isolated wrapper) | **43/43**. A first attempt failed 11 checks because the reviewer's wrapper created `KELPIE_SSH_DIR` with `mkdtemp`: Windows OpenSSH reported "Bad owner or permissions". The pre-change test server failed identically, and the run passed once the wrapper was fixed. |
| Isolated-daemon probe: a session program calls `run.create` / `session.create` | see H1, M1 |
| Node probes of `spec-index.ts` (copied to the scratchpad): 28 malformed / edge markdown fixtures, perf | 0 mislocated lines; perf see M4 |

All e2e ran through a scratchpad wrapper that sets `KELPIE_ENDPOINT` + `KELPIE_DATA_DIR` + `KELPIE_SSH_DIR` in its own `os.environ` before starting kelpied and tauri-driver, and shuts the daemon down through that endpoint. The real daemon (pid 14260) and app were not touched. `~/.claude.json` has no `fullscreenAutoDisabled` afterwards.

## Overall assessment
The local Plan tab is solid. The parser held on every malformed fixture I tried: reference definitions, front matter, setext headings, fences and HTML inside items, lazy continuation, tabs, blockquotes, CR-only, BOM. Ticks are byte-exact, with the mtime + digest + line checks. The remote launch reuses `build_remote_launch`, whose quoting is sound. Host validation is strict on the destination.

There are two problems with the trust boundary:
- The new session refusal can be bypassed with an array-shaped task (H1, reproduced end to end).
- The refusal is moot anyway, because `session.create` already lets a session start an ssh session with any `remote` (M1).

There is also one correctness problem: with tmux on, a host task's ssh exit status says nothing about the agent, so "exit 0 = done" is wrong (H2).

## Critical
None.

## High

### H1. A session program can still create a run whose task names an SSH host (array-shaped task bypass)
- **Where:** `crates/kelpied/src/auth.rs:211`
- **Problem:** The check is `x.get("host")` on each element of `tasks`. A task sent as a JSON array has no `"host"` key, so the check passes. Then `serde_json::from_value::<NewRun>` deserializes `NewTask` from the array field by field, and the 10th element becomes `host`. `object_params` refuses only a top-level array, not nested ones. This is the same trap as the phase 8 finding; see the agent memory note on auth param scoping.
- **Reproduced:** isolated kelpied, a `custom` session running `kelpied call run.create` with the session's own token:
  - `{"tasks":[{"title":"t","prompt":"p","approve":true,"host":{...}}]}` → refused: `a program cannot start a task on an SSH host`.
  - `{"tasks":[[null,"t","p","shell",[],{},[],true,false,{"dest":"127.0.0.1","port":1,"path":"/tmp"}]]}` → **accepted**. `run.list` shows the task `waiting_approval` with `host: {dest: 127.0.0.1, port: 1, path: /tmp}`.

  I used `approve:true` so nothing was launched. Without it, the daemon starts ssh with the user's ssh setup and askpass env.
- **Impact:** The documented contract ("refused, not dropped") is false. `auth.rs`'s unit test and the e2e cover only object tasks, so the suite is green while the boundary is open. The practical uplift is limited by M1.
- **Fix:** Refuse non-object tasks for session callers, before the host check:
  ```rust
  let tasks = obj.get("tasks").and_then(Value::as_array);
  if tasks.is_some_and(|t| t.iter().any(|x| !x.is_object())) { bail!("run.create: each task must be an object"); }
  ```
  A stricter alternative is to deserialize `NewRun` here and check `t.host.is_some()` on the typed value. Both close every shape at once. Add the array case to `programs_cannot_point_a_run_at_a_host`.

### H2. With tmux, a host task's end is decided by the tmux client's exit status, not the agent's
- **Where:** `crates/kelpied/src/runs.rs:431`; the launch is built at `agents.rs:365`.
- **Problem:** `step` marks a dead task session `Done` on exit 0 and `Failed` otherwise. A host task launched with `tmux: true` runs `exec tmux new-session -A …`. Saved hosts default to tmux, and the e2e host has no tmux, so this path is untested. The tmux client exits 0 both when the user detaches (prefix-d) and when the session ends, whatever the agent's exit status was. The local ssh process mirrors that.
- **Scenarios:**
  - **Detach:** the user detaches in the pane. The task becomes `Done` and the row hides its live state. Because the box is still unticked, Run with… comes back. Starting again launches a second agent on the same files, in a new `kelpie-<newid>` tmux session, while the first one keeps working.
  - **Crash:** the agent crashes inside tmux. The task is still `Done`.
  - **Network drop:** ssh exits 255 and the task becomes `Failed` while the agent keeps running in tmux. `run.retry` then starts a duplicate.
- **Fix (pick one):**
  1. Make host tasks start with `tmux: false`, so ssh's exit status is the agent's. That is simplest and matches "the task ends with its ssh session".
  2. Keep tmux, but for host tasks treat any session exit as "stopped". That means a new non-terminal state, or leaving the task `Running` until *Mark done*, idle, or the user closes it. Also never auto-retry a task whose tmux session may still exist.

  Document the choice in the user guide. Verify on a real host with tmux, because the Git Bash test host has none.

## Medium

### M1. `session.create` already accepts `remote` from session callers, so the new run.create refusal guards a door next to an open one
- **Where:** `auth.rs` `SCRIPTING` includes `session.create`; `scope_session_params` has no `session.create` arm; `daemon.rs:884` `create` takes `p.remote` (any `dest`/`config`/`port`).
- **Reproduced:** same isolated probe. `kelpied call session.create {"kind":"shell","cwd":"/tmp","remote":{"dest":"127.0.0.1","port":1}}` from inside a session → created (`alive: true`).
- **Note:** This is pre-existing, not introduced by this phase. It is also one more instance of "same-user code is not bounded" (`auth.rs` module doc).
- **Fix:** Decide the policy once.
  - If programs must not reach hosts, refuse `remote` (and `chat_argv`, which is also arbitrary argv) for `Role::Session` in `scope_session_params("session.create")`.
  - Otherwise, drop the claim in `NewTask.host`'s doc and in architecture.md that "only the user's own app may ask for that".

  In both cases, keep H1's fix so `run.create` has no shape-dependent behaviour.

### M2. A newly created root folder never shows up live
- **Where:** `app/src/lib/spec-index.ts:505` (`onFs` → `inRoots`), `:465` (`reloadDir` ignores folders not in `known`).
- **Problem:** When an agent writes `specs/feature/tasks.md` in a repo with no `specs/` yet, the watcher reports `""`, `specs` and `specs/feature`:
  - `""` is not in the roots;
  - `specs` and `specs/feature` were never listed, so they are not `known`;
  - so nothing is scanned.

  The same applies to a new `docs/plans/` (event dir `docs`) and `.kiro/specs/`. Reproduced with an in-memory `SpecIo`: the docs list stays `[]` after the events and appears only after `refresh()`. New `plans/<name>/` folders are fine, because `plans` is known.
- **Impact:** The phase's own success criterion is "Plan by agent A, tasks done by B and C, progress in one panel", and the first plan an agent writes in a fresh repo is exactly this case. The README also says "live as any agent edits the files".
- **Fix:** In `onFs`, treat an event for a parent of a root as a reason to check that root once. That means `""`, `"docs"` or `".kiro"` while the corresponding root is not known: re-list the parent and `scanDir` the root if it is now a plain folder. It is cheap, because it runs only when the root is absent.

### M3. New spec writes through a dangling symlink at the file level
- **Where:** `spec-index.ts:583-589`
- **Problem:** `createSpec` checks only `specs` and `specs/<slug>` for links. For the three files it uses `io.read(f)` as the existence test, and that read fails for a dangling link. `io.create(f)` then runs:
  - locally, `fs_create_blocking`: `full.exists()` is false for a dangling link, and `std::fs::write` follows it;
  - remotely: `[ -e "$f" ]` is false, then `: > "$f"`.

  Either way the link's target is created, and `fs_write_file` (which canonicalizes) or `cat >` writes the template there, outside the project.
- **Scenario:** a cloned repo, especially on a Linux host where links are free, ships `specs/notes/requirements.md -> ~/.config/<somewhere>/x`. The user clicks New spec "notes". The template text is fixed, so impact is limited. Still, it breaks the module's stated rule that links are never followed. Probe: `create()` is reached on the link path.
- **Fix:** List `specs/<slug>` (both listings include dangling links: `read_dir` locally, and `[ -L ]` in the remote loop). Refuse when any of the three names exists as an entry of any kind. After `create`, re-check with `plainPath` before writing.

### M4. Parsing a large spec blocks the UI thread far past the long-task budget
- **Measured** (node 26, same code):
  - a 256 KB file (the `MAX_BYTES` cap, 3906 tasks) parses in **116–121 ms warm and 213–221 ms cold** per change;
  - this repo's 56 plan files (228 KB) take 30 ms warm and about 160 ms on the first pass.
- **Context:** The e2e figure (24 files / 44 KB, best of 7 = 10.6 ms, cold 20.1 ms) and the vitest gate (best of 9, warm only) both pass. Neither covers the per-file worst case, and the global budget is "Long tasks > 50 ms: 0 new". An agent ticking boxes in a big tasks file re-parses it on every debounced change. The panel then re-creates every row of that doc, because `<For>` is keyed by the new task objects.
- **Fix:** Lower `MAX_BYTES` to about 64 KB, which stays under 50 ms. Otherwise parse off the main thread (a worker) or yield between blocks. Keep task object identity for unchanged lines, or use `<Index>`, so a tick doesn't rebuild the whole list. Add a cold single-file case to the perf check.

### M5. A remote tick can silently overwrite a concurrent edit and can truncate the file on a dropped connection
- **Where:** `app/src-tauri/src/remote.rs:224-238`, which the remote tick and New spec now rely on.
- **Problem:**
  - The tick reads (one ssh round trip), then writes (another) with `expected_mtime` compared in **whole seconds**. An agent on the host that ticks the same file between those round trips, within the same second, leaves the size and second unchanged. The write replaces its edit. Running agents on the host is exactly what this phase adds.
  - `cat > "$f"` truncates in place, so a connection that drops mid-transfer leaves a truncated spec.
- **Fix:** In the remote script:
  - compare content, not only time: pass the expected `cksum` (or size + sha) and check it on the host right before writing;
  - write to `"$f.kelpie-tmp"` then `mv -f`.

  This helper is pre-existing, but the claim "byte-exact, refused if changed" for remote ticks depends on it.

### M6. The run-task chip searches every run for every row on every run update
- **Where:** `plan-panel.tsx:195`; `runOfSpecTask` in `lib/runs.ts`
- **Problem:** `found()` is not memoized. It is read three times per row (`Show`, `live()` twice), and each read scans all runs and their tasks. Every `run.updated` broadcast re-runs it for every visible row.
- **Impact:** Fine for tens of rows. With hundreds of tasks and a long run history it becomes O(rows × runs × tasks) per event.
- **Fix:** One `createMemo` in the panel that builds `Map<taskId, {run, task}>` for the project's runs, then `const found = () => byTask().get(rp.task.id)`.

## Low
- **L1:** `safeLabel`/`scrub` (`spec-index.ts:122`) miss some invisible or bidi code points: U+061C ARABIC LETTER MARK, U+180E, U+2061–U+2064, U+FE00–FE0F, U+FFF9–FFFB, and the Tag block U+E0000–E007F, which is used to hide text from humans while LLMs still read it. Probe: `safeLabel("x؜y … ⁤ ᠎")` keeps them. Fix: extend `INVISIBLE` with these (Tag block via `\u{E0000}-\u{E007F}` and the `u` flag).
- **L2:** File *content* inlined for non-@path agents is not scrubbed of ESC/C1. The typed path is protected (`session.rs` `typed_text`), but a prompt of 8000 bytes or less goes to the agent as argv, where a TUI that echoes it could pass terminal sequences through. This is pre-existing composer behaviour, but untrusted spec files make it more reachable. Option: strip C0 except `\t\n\r`, plus C1, from inlined content in `expandMentions`.
- **L3:** For unlocated tasks (`line = -1`), Run with… is still offered and the prompt says `Task (line 0)` (`spec-index.ts:314`). Omit the line when it is unknown.
- **L4:** Ticking a second box in the same file while the first write is in flight returns `"busy"` with no toast (`spec-index.ts:540`, `plan-panel.tsx:108-110`). The click is silently lost. Either notify or queue the tick.
- **L5:** On a remote project, `load` skips re-parsing when mtime (whole seconds) and size are unchanged (`spec-index.ts:408`). A tick never changes the size, so an agent's tick in the same second as the last read is not seen until the next change. On remote roots, also compare the digest, or drop the shortcut.
- **L6:** `ssh_hosts.py` "Claude Code starts on the host" fails because the app now refuses agents in a host's login folder; the red notice is in the screenshot. This is a stale e2e expectation, not caused by this phase or by the test-server change. Update the script to set a start folder first.
- **L7:** Two pieces of noise:
  - `specTemplates`: `.replace(/@/g, "")` after `safeLabel`, which already turns `@` into `(at)`, is dead code (`spec-index.ts:338`).
  - The error text is doubled as `run.create: run.create: a program cannot…`, because the bail already carries the method name.
- **L8:** `runs-host.test.ts:34` `expect(RUN_HOST_API).toBe(16)` pins the placeholder and must change with the renumbering. Assert `RUN_HOST_API === API level that introduced hosts` from one shared constant, or drop the test.
- **L9:** `.panel-tabs { scrollbar-width: none }` (`styles.css:355`) hides the only cue that more tabs exist. The vertical wheel works, but there is no visual hint. Consider a fade at the edge or a thin scrollbar.
- **L10:** A task's id depends on its text and its occurrence index. Editing a task's words, or inserting an identical task above it, detaches the row from its run. The run stays on the Runs page but the chip disappears. This is acceptable by design; mention it in the user guide.

## Focus checklist, answered

### 1. Host runs
- **Destination:** letters, digits and `._@:%[]-`, no leading `-`, at most 255. Option injection, spaces, quotes, `;` and newlines are all refused (unit-tested, and refused in e2e). `user@-o…` is not parsed as an option because ssh stops option parsing at the destination.
- **Port:** not 0.
- **Config:** not empty, no leading `-`, no control characters. It is passed as its own argv, so there is no shell.
- **Folder:** absolute, no control characters, and `sh_quote`d into `cd` and tmux `-c`.
- **tmux session name:** alphanumeric session-id prefix only.
- **Remote command:** double-quoted (`sh_quote(body)` inside `-lc`), which is POSIX-safe. Windows spawns `ssh.exe` directly, with no `cmd /c`.
- **Same launch path as panes:** yes, via `create({remote})` → `launch_for` → `build_remote_launch`.
- **Session refusal:** bypassable (H1), and moot while `session.create` accepts `remote` (M1). Null and top-level-array params are handled by `object_params`; claimed sessions cannot call `run.create`.
- **Old daemon:** correctly gated in the app. `hostsKnown()` hides the host options and Run with… on remote projects, and `createSpecRun` throws before sending (tested).
- **End conditions:** correct without tmux; wrong with tmux (H2).

### 2. Remote Plan tab
- **Traversal:** `remote::guard` rejects `..` escapes lexically. The remote listing loop never yields `.` or `..`, and links are flagged with `[ -L ]` and skipped.
- **Command injection through file names:** none found. Every path is `q()`-quoted, the listing is NUL-separated, and `plainName` also drops control characters, quotes and backticks.
- **Tick over ssh:** see M5 and L5.
- **`ssh_test_server.py`:** passing stdin raw on non-pty channels is what a real sshd does, so the change does not hide bugs. `remote.py` (26/26), `ssh_panes.py` (43/43) and `ssh_hosts.py` (24/25, L6 unrelated) pass with it.

### 3. Local Plan tab
- **Roots:** symlinks and junctions are skipped at every level (`is_symlink` covers junctions in Rust std), and only known folders are reconciled.
- **Parser:** robust (0 mislocations over 28 fixtures).
- **Tick:** byte-exact for CRLF, CR and BOM, refused on an mtime or digest change.
- **Watcher:** gated by `active` and by the roots, so no work is done while the tab is closed (e2e counters unchanged).
- **Remaining gaps:** new roots (M2), dangling file links in New spec (M3), and the large-file cost (M4).

### 4. Run with…
- **Prompt:** only `Spec file: @path` is expanded. Instructions are added afterwards, `redact` is applied, and the typed path goes through `typed_text`.
- **Heading and task text:** scrubbed of C0/C1, bidi, LS/PS and zero-width characters. `@` becomes `(at)`, and quotes and backticks become `'`. Gaps are listed in L1.
- **Worktree:** only `planBringing().approved` lists are used.
- **Task id:** a 32-bit FNV over path, text and occurrence index. It stays stable across ticks, and collisions are negligible at this scale (L10).
- **Row status:** read from the Runs store, with no `task_ref` store (M6 for cost).

### 5. Perf and style
- **Panel and index:** lazy. The entry chunk is 320.56 KB gz, within budget, per the developer's report and `perf-phase-10.json`.
- **i18n:** clean.
- **Plan IDs:** none in the code. "Phase 10" appears only as fixture content.
- **Parse budget:** met warm for 20 small files, exceeded per large file (M4).

## Recommended actions (order)
1. H1: refuse non-object tasks for session callers in `run.create`, and add the array case to the test.
2. H2: run host tasks without tmux, or stop mapping a host session's exit to done/failed; verify on a real tmux host.
3. M1: decide the `session.create` `remote` policy for session callers and make docs and code agree.
4. M2: discover new roots from parent-folder events.
5. M3: use the folder listing as the existence test in New spec, and re-check before writing.
6. M4: lower `MAX_BYTES` or move parsing off the main thread; keep row identity.
7. M5: content check plus temp-and-rename in the remote write script.
8. M6, L1–L10 as convenient.

## Metrics
- Type coverage: `tsc --noEmit` clean; no `any` in the new production code. `main.tsx` uses an existing `(window as any)` e2e hook.
- Tests: kelpied 242; vitest for the phase-10 suites 55. E2E: spec_panel 109/109, remote 26/26, ssh_panes 43/43, ssh_hosts 24/25 (L6).
- Lint: clippy not re-run by the reviewer (the developer reports it clean).

## Unresolved questions
- Is `session.create` with `remote` meant to be available to session programs (M1)? That is a product decision and it decides how H1 should be framed.
- Host tasks and tmux (H2): should a host task outlive its connection, which needs tmux and a different end rule, or end with it (no tmux)?

Status: DONE_WITH_CONCERNS
Summary: The local Plan tab, the parser, the byte-exact tick and the remote launch quoting hold up (spec_panel 109/109, remote 26/26). But the session refusal for host tasks can be bypassed with an array-shaped task (reproduced), and with tmux, host tasks are marked done or failed from the tmux client's exit status rather than the agent's.

---

## Re-review (2026-10-25, after the developer's review round)

**Scope:** the hunks listed under "Review round" in the developer report, checked against the current tree.

### What I ran

| Check | Result |
|---|---|
| `cargo test -p kelpied -- runs:: auth::` | 20 pass |
| vitest spec-index + runs-host | 67 pass |
| `pnpm typecheck`, `pnpm i18n:check` | clean; 0 missing |
| `spec_panel.py` (own isolated daemon/driver) | **113/113** |
| Isolated-daemon probe: a `custom` session calls `kelpied call run.create` with its own token, 15 shapes | see H1 |
| Node probes on scratch copies of `spec-index.ts` | slicing equivalence, turn lengths, live roots, New spec, cksum vectors vs Git Bash `cksum` |
| The new remote write script run under Git Bash with stdin closed early | see N2 |

Kelpie-46's 3 failing `mobile::` tests were not run, as instructed. Only processes I started were stopped; the other kelpied processes (pid 14260 = the real daemon; 20252 and 6968 belong to other sessions) were left alone.

### Status of each finding

| Finding | Status | Evidence |
|---|---|---|
| **H1** | **Fixed** | All 15 shapes refused except the legitimate one. Details below. |
| **H2** | **Fixed as decided, with residual risk (R1)** | Details below. |
| **M1** | Open by decision | `NewTask.host` doc and architecture.md no longer claim more than the code enforces. |
| **M2** | **Fixed**, but with a **pre-existing gap the fix copies (N3)** | Details below. |
| **M3** | **Fixed** | Details below. |
| **M4** | **Partially fixed**, and the slicing **introduced N1 (blocking)** | Details below. |
| **M5** | **Partially fixed: truncation still possible (N2), and a new symlink regression (N4, blocking)** | Details below. |
| **M6** | **Fixed** | Details below. |
| **Lows** | L1–L5, L7–L9 fixed; L6 and L10 deferred by decision | Details below. |

**H1 (fixed).** Of the 15 shapes, every one is refused except `[{…, host: null}]`, which is accepted as a local task waiting for approval:
- a task given as an array (the host as the 10th element, or as an array itself);
- a string, `null` or number task;
- `tasks` given as a map or a string, or as `null` (refused by serde);
- `host` given as an object, array, string or `false`;
- a host on the second task;
- duplicate `"host": null, "host": {…}` keys (serde_json keeps the last one, and the check and the deserializer see the same `Value`).

`materialize` was dropped in every case. No other privileged field is reachable: `NewRun` is an object, `TaskHost` comes only from `host`, and `env`/`args`/`kind` were already the session's to set. The error is no longer doubled.

**H2 (fixed as decided).**
- `TaskHost` has no `tmux` field, and `remote()` hard-codes `tmux: false`. The app sends no tmux, and an old app's `tmux` field is ignored by serde.
- The launch takes the direct `cd … && exec …` path.
- The end rules in `step` are right: exit 0 is done; 255 or no code is failed ("stopped with the connection"); any other code is failed with that code.
- The idle completion is skipped for host tasks (`t.host.is_none()`).
- Two agents are still possible in some cases; see R1.

**M2 (fixed).**
- In the probe, a new `specs/` (events `""` + `specs/feature`) and a new `docs/plans/` are picked up live.
- A root that is itself a link is refused.
- A root-level event while a root is still unknown costs one listing per debounced batch.
- Gap: see N3.

**M3 (fixed).**
- The spec folder's listing now decides; a dangling `requirements.md` gave `symlink specs/evil/requirements.md`, and nothing was written through it.
- `fs_create` refuses dangling links both locally (`symlink_metadata`) and over ssh (`[ -L ]`).
- `plainPath` is re-checked after create.
- Outside the project: `guard` is lexical, and `specs`/`specs/<slug>` links are refused before `mkdir -p`.

**M4 (partial).**
- One long list of 256 KB: 15–18 turns, the longest 9.6 ms. The result is identical to the synchronous parse.
- 64–256 KB documents of short multi-item sections: longest turn 8–16 ms.
- Still long: a document whose lists have a single top-level item each (`## Step` / `- [ ] task` / `  - [x] sub` / paragraph) has no cut point at all. At 256 KB that gives 2 turns, the longest **129–192 ms**.
- Fix: also cut before column-0 headings outside fences and HTML.

**M5 (partial).**
- The `cksum` check is correct: `cksumOf` matches Git Bash `cksum` for `""` and `"a\n"`, and the value is validated to digits and spaces before it goes into the script.
- The temp file `<file>.kelpie-tmp.$$` sits in the same folder, so the rename stays on one filesystem; there is no cross-filesystem move. No temp file was left behind in my runs.
- Gaps: N2 and N4.

**M6 (fixed).** One `createMemo(specTaskRuns)` serves the whole panel, and rows are keyed with `<Index>`.

**Lows.**
- L1: the Tags block, U+061C and the rest are dropped; probe: `safeLabel("x؜y\u{e0041}z")` gives `"xyz"`.
- L2: `stripControls` removes ESC/C1/bidi/invisible characters and keeps tab and newline.
- L3: no "line 0" in the prompt or the run title.
- L4: a second tick while a write is in flight now shows a toast.
- L5: whole-second times are never treated as "unchanged".
- L7 and L8 done; L9 uses a thin scrollbar.
- L6 and L10 deferred, as decided.

### New or remaining findings

#### N1. Slicing changes what is a task — BLOCKING (High)
- **Where:** `spec-index.ts` `cutPoints`/`parseSpec`. Every file over 120 lines is now sliced, not only big ones.
- **Problem:** `cutPoints` only avoids cutting inside a fence, and it tracks only the fence *character*, not its length. It also lexes slices one after another with a shared lexer. Compared with a whole-file parse (my probe; cuts every 1–7 lines, plus the real 120-line default):
  - **HTML comments** (`<!-- … -->`) and HTML blocks (`<details>…`) are split. Commented-out `- [ ]` lines become real, tickable, runnable tasks. At the default chunk size, a 200-line commented-out list yields **84 tasks instead of 1**.
  - **Nested fences** (```` ````md ```` containing ```` ``` ````, the usual way to show markdown examples) close early in `cutPoints`, so example tasks inside become real tasks.
  - **Reference links defined at the end** (`[text][ref]` … `[ref]: url`): tasks lexed before the slice holding the definitions keep the raw `[text][ref]` text. Their **task ids differ** from a whole-file parse, and they move whenever an edit shifts the cut points, so the row loses its run.
- **Impact:** These are ordinary plan files, not 256 KB edge cases. Ticking a phantom task writes into a comment, and Run with… hands an agent a task the author commented out. The "results are identical" claim does not hold; the generated tests did not include HTML comments, nested fences or reference definitions.
- **Fix:**
  - Treat `<!--` up to `-->`, and HTML block starts up to a blank line, like fences in `cutPoints`.
  - Track the fence's char **and length** (a closing fence must be at least as long and carry no info string).
  - Collect reference definitions before walking (a cheap regex pass that seeds `lexer.tokens.links`), or slice only above `SLICE_BYTES`.
  - Add the three cases to the equivalence test.

#### N4. Remote saves now replace a symlink with a plain file — BLOCKING (High, regression)
- **Where:** `remote.rs` `write_file`
- **Problem:** `mv -f "$t" "$f"` renames over the directory entry. When `$f` is a symlink, the link itself is replaced by a regular copy, and the target is left untouched. Before this round, `cat > "$f"` wrote through the link. The local path still writes through links deliberately (`fs_write_file_blocking` canonicalizes: "Write through symlinks to their target").
- **Impact:** The Plan tab refuses links, but this helper is also the **editor's** save for remote projects. `CLAUDE.md -> AGENTS.md` style links are common, and saving one over ssh would silently fork the two files.
- **Fix:** Resolve the target first, for example:
  ```sh
  if [ -L "$f" ]; then r=$(readlink -f -- "$f") || exit 1; f=$r; fi
  ```
  before building `$t`. Alternatively, keep the old in-place write for links. Add a check for a symlinked file to `remote.py`.

#### N2. A dropped connection can still commit a truncated file (Medium; M5 not fully fixed)
- **Where:** `remote.rs` `write_file`
- **Problem:** When the connection drops, sshd closes the channel's stdin. The script is `cat > "$t" && mv -f "$t" "$f"`: `cat` sees EOF, exits 0, and `mv` commits whatever arrived.
- **Reproduced:** I ran the exact script under Git Bash and closed stdin after a third of a 30 894-byte file. Exit 0, and the file was **10 298 bytes** afterwards.
- **Note:** That sshd closes stdin on a dropped channel is my reading of OpenSSH's behaviour; I did not reproduce it against a real sshd here. Not a regression (the old in-place write truncated too), but the developer report's "never a truncated one" is not true.
- **Fix:** Check the temp file before the `mv`, for example:
  ```sh
  [ "$(wc -c < "$t" | tr -d ' ')" = N ] || exit 5
  ```
  with `N = content.len()`, or compare `cksum` of `$t` against the new bytes' cksum, which Rust can pass.

#### N3. A linked `docs/` or `.kiro/` lets the index walk outside the project (Medium; pre-existing, and the M2 fix copies it)
- **Where:** `spec-index.ts` `scanAll` and `findRoots`
- **Problem:** Only the root entry itself (`plans`, `specs`, `docs/plans`, `.kiro/specs`) is checked for `is_symlink`, never its parent. If `.kiro` or `docs` is a junction or symlink to somewhere else, the index lists `docs/plans` / `.kiro/specs` *through it*. Probe: a linked `.kiro` → `.kiro/specs/x/tasks.md` was listed.
- **Impact:** Read-only. Ticks are still refused, because `plainPath` checks every component. I missed this in round 1.
- **Fix:** In both functions, also require the parent (`docs`, `.kiro`) to be a plain folder in the root listing.

#### R1. A second agent is still possible in two corner cases (Medium, not blocking; document or mitigate)
- **Half-open connection:** without tmux, the agent dies with its connection only once the host's sshd notices. When the client's network drops, local ssh exits 255 after about 2 minutes (`ServerAliveInterval=30` × 4) and the task becomes Failed. Unless the host sets `ClientAliveInterval`, its sshd may keep the agent's pty for many minutes, until TCP gives up. A retry in that window runs two agents on the same files. The `TaskHost` doc and the panel note ("the agent goes with it") overstate this.
  - Mitigation: the launch writes the remote shell's pid (`echo $$ > ~/.kelpie/run/<sid>.pid; exec …`), and retrying a host task first kills that pid over ssh. Alternatively, add `-o ClientAlive…` advice to the docs, and change the note to "may keep running for a while".
- **Daemon restart:** when the daemon restarts during a host task, the task is marked Failed. But the app may auto-resume the interrupted session (`resumeSessions`), which restarts the agent, now untracked, while Run with… is offered again. This is shared with local tasks and is pre-existing.

#### N5. Low
- `runs.rs:522`: the comment still says the task ends "or idle after working"; for host tasks that is no longer true.
- `remote.rs` `write_file`: when the file was deleted between read and write, both checks are skipped (`if [ -f "$f" ]`) and the tick recreates it. A conditional write should fail with a conflict when the expected file is missing.
- The new write also needs write permission on the *folder*, to create the temp file and rename. A file that is writable inside a read-only folder can no longer be saved over ssh.
- `cp -p` may change the file's owner to the ssh user for group-writable files owned by someone else.

### Blocking before commit
1. **N1:** slicing must not change parse results (HTML comments and blocks, nested fences, reference definitions). Restrict cut points and add the equivalence cases.
2. **N4:** remote saves must keep writing through symlinks.

### Should fix (not blocking)
- N2: size or cksum check of the temp file before the `mv`.
- N3: check the parent of `docs/plans` and `.kiro/specs`.
- M4 remainder: cut points before headings.
- R1: wording at least, pid cleanup preferably.
- N5.

### Re-review metrics
- Tests: kelpied runs/auth 20 pass; vitest phase-10 suites 67 pass; typecheck and i18n clean.
- E2E: spec_panel 113/113. `remote.py` and `ssh_panes.py` were not re-run by me in this round; the developer reports 26/26 and 43/43, but they do not cover symlinked files or an interrupted write.

Status: DONE_WITH_CONCERNS
Summary: H1, H2, M3 and M6 are fixed, H1 verified against an isolated daemon with 15 task shapes, but two of the fixes introduced blocking regressions. Sliced lexing turns HTML-commented or nested-fence example tasks into real tasks and changes ids when reference links are defined at the end (N1), and the remote atomic write now replaces symlinks with plain copies (N4). A dropped connection can also still commit a truncated file (N2).

---

## Final check (2026-10-25)

### What I ran

| Check | Result |
|---|---|
| `cargo test -p kelpied -- runs:: auth::` | 21 pass |
| `cargo test -p kelpie remote::` | 8 pass, including `a_remote_write_keeps_links_and_never_commits_a_short_file` and `cksum_is_the_posix_one` |
| vitest spec-index + runs-host | 70/70 when the machine is quiet; see F4 |
| `spec_panel.py` (own isolated daemon/driver; debug app 17:33 is newer than every phase-10 source) | **124/124** |
| Node: worker `handle()` + `structuredClone` vs `parseSpec` | identical; see N1 |
| Git Bash: remote.rs's `WRITE_SCRIPT`, extracted verbatim and filled exactly as `write_script` does | see F1 and F2 |

The tauri-driver/msedgedriver processes seen afterwards belong to phase 11's `event_automations.py` run (parent `D:/kelpie-par-scratch`) and were left alone. The scratch folder is deleted.

### 1. N1: fixed
- **Slicing is gone.** `cutPoints`, `CHUNK_LINES` and the sliced `parseSpecAsync` no longer exist, and `parseSpec` lexes the whole file.
- **Worker equals main thread.** Padded above 8 KB, my four cases come back identical from `handle()` after a structured clone and from `parseSpec` on the main thread:
  - HTML comment: 1 task, not 201;
  - `<details>`: 1 task;
  - nested fence: 1 task;
  - reference links defined at the end: `"see doc 0"`, with the same ids.
- **Fallback.** When there is no `Worker`, the file is parsed in place. When the worker throws on construction, errors, has a `messageerror`, or `postMessage` throws, `giveUp` parses everything still waiting in place and never tries the worker again.
- **Untrusted content.**
  - The reply is plain structured-clone data.
  - Task text only ever becomes string *values*; the `seen` map is a `Map`, and `__proto__`/`constructor` task texts leave `Object.prototype` clean.
  - Rendering is Solid text and attribute binding, with no `innerHTML` in the panel or the spec modules, so a `<img onerror>` task shows as text.
  - Replies are matched by numeric id. The worker is our own module (CSP `'self'`).
- Low: see F5.

### 2. N4: fixed for links. Two new holes in the script, one of them BLOCKING (F1, F2)
- **Links kept.** `resolve()` follows a link chain with plain `readlink --` (at most 40 hops), handling relative targets. The temp file goes in the target's folder and is renamed over the target, so the link stays a link. The Rust test and the developer's e2e cover an in-project link, a link leading out, a dangling link and confine.
- **No `readlink` on the host.** `resolve` fails, the script exits 6, and nothing is written. That fallback is safe.
  - macOS before 12.3 and BusyBox both have plain `readlink`; the script doesn't need `-f`.
  - On a host where `[ -O ]` is unsupported, the test errors and the script takes the verified in-place path, which is also safe.
- **`confine`.** Only the Plan tab sets it (`plan-panel.tsx:40`). It compares `pwd -P` of the resolved file's folder with the project's.
  - Edge (Low): a project rooted at `/` gives `R=/`, and the pattern `//*` never matches, so every confined write there is refused.
- **Owner and mode.** `cp -p` is gone. The mode is copied with `chmod` (GNU `stat -c %a`, BSD `stat -f %Lp`). Files owned by someone else, and folders that can't be written to, take the verified in-place path through `mktemp` in `$TMPDIR`.
- **Quoting.** A path with spaces, `'`, `$(…)` and `;` is written correctly; I checked `x'y z $(echo hi) ;.md`. A newline in a name can't be created on NTFS, so I couldn't test it, but `q()` is sound for it.

**F1. Insecure temp file in the target's folder (BLOCKING, High)**
- **Where:** `remote.rs` `WRITE_SCRIPT`
- **Problem:** `t="$f.kelpie-tmp.$$"`, then `( : > "$t" )` and `cat > "$t"`. The name is predictable (the shell's pid), and `>` opens it without `O_EXCL`, following an existing symlink.
- **Scenario:** a multi-user host where another account can write to the project folder, for example a group-shared repo. That account pre-places `tasks.md.kelpie-tmp.<pid>` symlinks for a range of likely pids, pointing at the victim's `~/.bashrc`. The victim's next save or tick writes the file's content into `~/.bashrc`, and the attacker can influence that content through a shared spec file. `mv` then moves the symlink over the target.
- **Shown:** in my run, a pre-existing `t.md.kelpie-tmp.4242` was silently truncated, overwritten and consumed. Real symlinks can't be created on this machine, but `>` following a symlink is standard POSIX behaviour.
- **Second problem:** the temp file is created with the umask (022, so 0644) and holds the content before `chmod`. A file with mode 0600 is briefly readable by other users.
- **Fix:** `t=$(mktemp "$f.kelpie-tmp.XXXXXX") || …`. That creates the file with `O_EXCL` and mode 0600, and works with GNU, BSD and BusyBox. Keep the in-place fallback when `mktemp` in that folder fails.

**F2. Placeholder text in a file name breaks the script's quoting (Medium)**
- **Where:** `remote.rs:282`
- **Problem:** `write_script` substitutes `__F__` (the quoted path) **first**, then replaces `__ROOT__`, `__CONFINE__`, `__CHECKS__` and `__NEW__` across the whole script, including inside the already-quoted path.
- **Reproduced:**
  - `a__NEW__b.md` leaves the cksum digits unquoted, and the shell then tried to run a command named `8b.md` ("8b.md: command not found"): part of the file name ran as a command.
  - `a__CHECKS__b.md` and `a__CONFINE__b.md` simply break the script.
- **Impact:** I found no practical code execution: the command word is digits plus a slash-free name suffix, looked up in `PATH`. But it is an injection-class flaw in a script run on the user's hosts, and such files cannot be saved over ssh.
- **Fix:** substitute the path **last**, or do a single-pass replacement. Better still, pass values as positional parameters: `set -- 'path' 'root' 'sum'` and use `"$1"`.

### 3. N2, N3, host retries
- **N2: fixed.** `cksum` of the temp file, which covers both size and CRC, is compared with the app's `cksum(content)` before the rename and before the in-place copy. A short or broken input exits 5 and leaves the original untouched (Rust test: a stream cut after 10 bytes).
- **N3: fixed.** `plainRoot` requires every component (`docs`, `.kiro`, then the root) to be a plain folder, both in `scanAll` and in `findRoots`.
  - Low, pre-existing: a root that becomes a link *after* it was walked is reloaded through the link by `reloadDir`. Listing is read-only and ticks are still checked by `plainPath`.
- **Retry confirmation.** Runs page *Retry* on any host task, and Run with… after a failed host run, both ask first. The e2e covers it.
- **Restart path.** A running host task whose session comes back `interrupted` is failed with a clear message, gets `Capture`, and then `Release`.
  - `Session::clear_interrupted` (`session.rs:948`, the only method I judged in that file) only takes the lock and clears the flag. That is correct and minimal, and `release_session` broadcasts the change.
  - **F3 (Medium, not blocking):**
    - `release_session` does not set `meta_dirty`. If the daemon stops again before the next persist, the session is saved as `alive || interrupted` and comes back interrupted, while its task is already Failed, so `step` no longer looks at it and the app may resume an untracked agent.
    - The failing happens on the run ticker, about a second after `restore()`. An app set to resume "at once" may restart the session first; the task then stays Running and follows the resumed agent, which is tracked but may be a second agent while the first is still on the host.
    - Fix: set `meta_dirty` in `release_session`, and apply the interrupted-host-task rule once synchronously after `restore()`, before clients are served.

### 4. Other new items
- **F4 (Low): two timing tests flake under load.** "parses a 256 KB list in one piece, in under a second" and "24 plan files … under 20 ms" both failed when other sessions' e2e and builds were running, and passed 70/70 when the machine was quiet. Keep the hard bounds in the perf gate, or allow a retry or a load-relative bound.
- **F5 (Low): no worker timeout.** A file that makes the lexer hang in the worker leaves its `load` promise pending forever. The index's serial queue then stalls until the panel is remounted. A main-thread parse would have frozen the page instead, so this is strictly better, but a timeout that marks the doc as a "problem" and recreates the worker would contain it.

### Verdict
- **Blocking: F1**, the predictable, non-exclusive temp file in the target's folder. It is a one-line `mktemp` template fix, and the same change closes the umask exposure.
- **Should fix before commit (not blocking):**
  - F2 (substitute the path last, or use positional parameters);
  - F3 (`meta_dirty`, and the interrupted rule applied at restore).
- **Low:** F4, F5, the `/`-rooted confine edge, and the late-link reload.
- **Fixed and verified:** H1, H2, M2, M3, M5/N2, M6, N1, N3, N4 (links kept), the host-retry confirmation, and the Lows.
- **Open by decision:** M1, L6, L10.

Status: DONE_WITH_CONCERNS
Summary: N1 (whole-file parse, worker identical on all four regression cases), N2, N3, N4's link handling and the host-retry rules are verified, and spec_panel passes 124/124. One blocking item remains: the remote write script's temp file has a predictable name and is opened without `O_EXCL`, so on a multi-user host another account can redirect the write through a planted symlink (F1). A file name containing a placeholder also breaks the script's quoting (F2, reproduced).
