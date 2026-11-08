# Phase 4 e2e and real-agent check: sub-session orchestration

Worktree `D:\kelpie-subsessions`, branch `feat/sub-sessions`. Nothing committed or pushed. No code fix was needed in `crates/` or `app/`; the only repo file added is `scripts/e2e/sub_sessions.py`.

Build: `CARGO_TARGET_DIR=/d/kelpie-subsessions-target CARGO_INCREMENTAL=0 cargo build -p kelpied -j 2` (59 s, ok). Binary: `/d/kelpie-subsessions-target/debug/kelpied.exe`.

## 1. Daemon-level script: `scripts/e2e/sub_sessions.py`, 42/42 pass

Command: `python scripts/e2e/sub_sessions.py /d/kelpie-subsessions-target/debug/kelpied.exe` (exit 0). Log: `/d/kelpie-subsessions-target/sub-e2e-run3.log`. Isolated daemon on `kelpie-sub-e2e`, temp data folder, own PID stopped. Agents are a generated fake agent (custom kind, counts as installed). It writes its own id and token to a file, logs typed input, and runs `RUN <kelpied sub …>` through `$KELPIE_CLI`.

| Area | Checks | Result |
|---|---|---|
| Policy allow | spawn returns handle; child parent and depth 1; first prompt typed in | pass |
| Ask | sub.request to user with parent and task; listed in sub.pending; deny refused with the user's words, no child; sub.request_done; once spawns; allow-for-session skips the second ask | pass |
| Caps | sub_max 1 refuses "4 sub-sessions are running … the limit on this computer", asks nothing; policy off refuses "turned off … Settings" | pass |
| Close | sub.close of own child; sub.close of a non-descendant refused | pass |
| Boundaries | sub.message to a non-agent refused; session.create shell and custom from a session refused; session.create from a session becomes a linked child | pass |
| Chains | child opens grandchild at depth 2; parent closes it through the chain | pass |
| Mail | parent message queued for child; child sub.report names parent; report typed into parent and child with the "not from the user" header; kelpied sub list inside a session (KELPIE_CLI, own token) | pass |
| lineage_from | new session takes parent and depth; old session has no parent; children follow the new session | pass |
| Shell parent (Windows) | `kelpied sub spawn` from PowerShell links child; report not typed into shell; report waits in inbox (`kelpied sub inbox`) | pass |

Runs before green: run1 failed on a test bug (stale request id; fixed); run2 passed all 42 checks but crashed in cleanup (`struct.error` on shutdown reply; fixed, and it left a token and temp folders, removed by hand). Run3 clean. Leftover check after run3: no token, no temp folders, no extra process.

Not in this script: the 30 s quiet timer was exercised (mail arrived after waits), not timed.

## 2. Real agents: Claude parent, Codex child

Driver: `D:\kelpie-subsessions-target\e2e-real\real_run.py` (outside the repo; not for commit as-is). Isolated daemon `kelpie-sub-real`, temp git folder. The user approval was given through the user connection (`sub.answer once`). Evidence folders: `D:\kelpie-subsessions-target\e2e-real\evidence-run3` and `…\evidence-run6` (screens, session.list rows, redacted).

Prompt to the parent: use kelpie_spawn to open a codex child in this folder that creates hello.txt with `hi` and calls kelpie_report done, then end the turn.

| Requirement | Result | Evidence |
|---|---|---|
| Claude parent created by user token with injected MCP | pass | parent row kind claude; its kelpie_spawn call raised the approval card |
| Approval card for the spawn | pass | agent codex, session = parent, worktree false, skips_prompts false |
| Child appears with parent | pass | `{"name":"codex-child","kind":"codex","parent":"<parent id>","depth":1}` |
| Codex child reports through MCP | pass | child screen run3 and run6: `Called kelpie.kelpie_report` → `Reported done to session "claude-parent" (claude, #1). It arrives there as a new message.` |
| Report on the child row | pass | `"report":{"status":"done","summary":"Created only hello.txt containing hi; verified exactly 2 bytes (hex 68 69), with no trailing newline; nothing remains.","auto":null}` |
| Report typed into idle parent as a new turn | pass, partly | run6 parent screen shows the report line and Claude acting on it ("Verifying the child's claim…", "Read 1 file, ran 2 shell commands"). The header line was NOT in the saved screen capture |
| "not from the user" header on the parent | not verified on the Claude screen | header format checked at daemon level by sub_sessions.py (fake agent typed log) |
| kelpie_sessions shows the report | not checked | session.list row shows it; the MCP tool output was not captured |
| Clean close | pass in runs 3 and 6 | `exit codex-child clean`, `exit claude-parent clean` |

Runs: run1 parent quit at once (Enter on "No, exit"; driver bug). Run2 got approval, child done, then the driver crashed on reading hello.txt and its shutdown hard-killed the live Claude parent and Codex child (see Incidents). Run3: approval, child done via MCP, both closed by /exit; the header check matched the brief text, so it proves nothing. Run4: parent quit again on "No, exit"; stopped by endpoint. Run5: listener missed the approval, so I answered it by hand (sub.pending path); driver ended on a trust-dialog exception; parent /exit clean; Codex child killed by the final shutdown. Run6: approval via sub.pending; child done via MCP; parent got and acted on the report; both closed by /exit; header check did not match (see above).

Manual steps, all logged in the run output: run3 Enter on the Codex trust prompt (driver window too short, then driver answered too); run5 approval answered by hand; run6 Enter on the Claude trust dialog (driver's selection check missed the leading space; fixed afterwards).

### Findings (product)

**F1. Automatic `idle` reports fire during child startup and quiet mid-turn; the parent acts on them.**
- Evidence: run3 parent screen: three `idle` notices while the child was still working; the parent then wrote "The three 'idle' notices came while it was still busy … the copy I sent will probably arrive as a second turn", i.e. it re-sent the task. Duplicate work.
- Cause: `crates/kelpied/src/subs.rs` `ChildWatch::observe`: Working→Idle after 3 s (`MIN_WORK`) reports `Auto::Idle`. Codex goes Waiting (trust) → Working (startup) → Idle (ready for the brief), and its screen goes quiet between steps.
- Not fixed: a design choice. Options: (a) no idle report while the first prompt is still pending (`pending_prompt` in `session.rs`); covers startup only. (b) Debounce idle (about 20–30 s); covers quiet steps, adds delay. (c) For Codex, use its notify hook (`agent-turn-complete`) as the idle source.
- Do not gate on `turn_seq() > 0`: it moves only on hook-reported Working, and Codex sends none, so Codex automatic reports would go silent.

**F2. Files a Codex child writes are not readable by the user's account on Windows.**
- Evidence: run3 and run6, `hello.txt` in the child's folder: `Permission denied` on open; `Get-Acl` "unauthorized operation"; `icacls` fails. The parent Claude also reported it could not confirm the file's content.
- Not Kelpie code: Kelpie does not touch ACLs. Codex's Windows sandbox log shows write ACEs on the workspace and deny ACEs on `.git`; the exact cause was not reproduced.
- Impact: Finish and Review and the user cannot read child output on Windows. Needs a decision: which Codex sandbox mode children get (Kelpie passes none today).

**F3. Trust dialogs need the user.** The first run in a new folder shows Claude's trust dialog (starts on "No, exit"; Enter quits Claude) and Codex's (Enter works). Kelpie surfaces neither. Not a bug in the daemon; a usability note for the user guide.

### Incidents (test hygiene)
- Run2 hard-killed a healthy parent Claude TUI and the Codex child (driver's `daemon.shutdown {kill_sessions: true}` after a crash). Checked afterwards: `fullscreenAutoDisabled` absent from `~/.claude.json`, `"tui": "fullscreen"` still in `settings.json` (checked again at the end). Driver now ends agents with /exit first; the final shutdown still kills anything left alive (the Codex child in run5).
- Leftover temp folders and tokens from crashed runs were removed by name; the user's daemon (PID 8704) and user `claude.exe` processes were not touched.

## 3. GUI: skipped
Reasons: no desktop app binary in the isolated target (only `kelpied.exe`); a cold Tauri debug build on 2 jobs with about 2.7 GB free RAM (shared with other sessions) was not justified; `msedgedriver` is not on PATH (`tauri-driver` is present). No screenshots taken. Sidebar tree, strip and approval card rendering are untested in the real app.

## 4. Not done in this pass
- Docs (phase 4 items 1–2: `docs/architecture.md`, `docs/user-guide.md`) and plan status: not in this task's scope.
- Rust unit tests not re-run here (developer reports: kelpied 887 pass, clippy clean). Frontend tests not re-run.
- ACP and Claude/Codex chat injection not exercised. Ask-mode approvals only; policy allow not exercised with real agents.

## Resources
- Disk D: 7.4 GB free at start, 6.8 GB at end (build plus runs). RAM free about 2.7 GB during real runs (shared machine).
- Temp folders: all `kelpie-sub-*` from this session removed. Files outside the repo kept for evidence: `D:\kelpie-subsessions-target\e2e-real\` and `D:\kelpie-subsessions-target\sub-e2e-run*.log`.

## Unresolved questions
1. F1: which idle-report fix, or keep as is? (Needs owner decision; a design call.)
2. F2: should Kelpie pass a sandbox or permission mode to Codex children so their output is readable? Is this a Codex default we accept?
3. Was the approval event missed in run5 because the listener died? `scripts/kelpie-call.py` `frame()` reads the 5-byte header with one `read(5)` and does not loop on short reads, and the listener swallows exceptions. Unverified; low likelihood.
4. Does the "not from the user" header appear on the Claude screen for mail? Needs a run that keeps the screen (not captured in run6).

Status: DONE_WITH_CONCERNS
Summary: Daemon-level script green (42/42) and real Claude-to-Codex run reached child link, MCP done report, and parent receipt; no code bugs fixed. Concerns: automatic idle reports cause duplicate work (F1); child output unreadable on Windows (F2); GUI and docs not covered.
Concerns: F1 and F2 need owner decisions; header not seen on Claude screen; two hard kills in test runs (run2, run5 Codex child).

## Follow-up A: F2 isolation (Codex outside Kelpie)

Config (key names and enum values only; no secrets read): `[windows] sandbox = "elevated"`, top-level `approvals_reviewer = "user"`. `codex-cli 0.166.0`. No user config changed.

| Case | Command | Result |
|---|---|---|
| Git temp folder, no sandbox flag | `codex exec` (defaults to `sandbox: read-only`) | no write: "filesystem is read-only" |
| Non-git temp folder, no sandbox flag | same | same |
| `D:\kelpie-subsessions-target\f2-check` | same | same |
| Git temp, non-git temp, f2-check | `codex exec -s workspace-write --skip-git-repo-check` | hello.txt written, 2 bytes, readable by this account (`hi`) |
| f2-check, child's exact PowerShell write | same, prompt asks for `[System.IO.File]::WriteAllText(...)` | readable (`hi`) |

ACEs seen on the isolated files: the inherited `DESKTOP-1IVG152\CodexSandboxUsers:(I)(M)`, `BUILTIN\Users` or `Authenticated Users` (from the parent), `SYSTEM`, and the Administrators group on the projects folder. Owner: `CodexSandboxOffline`, the sandbox account. This account is `BaoPeakHub`.

Conclusion: the Kelpie child's unreadable file did not reproduce outside Kelpie. Isolated `workspace-write` writes were readable by the user in every case tried. The owner is the sandbox account in both cases, so ownership alone does not explain F2. The difference is the readable inherited ACE, and the Kelpie child's file denied even `Get-Acl`. The remaining difference is Kelpie's own launch: the interactive TUI on the daemon's ConPTY with notify, MCP and env set. This was not reproduced because no PTY library is installed, and I did not install one. F2 is therefore not shown to be Codex-only. Next check: after the next Kelpie child writes a file, read its owner and ACL right away and compare with an isolated interactive run.

## Follow-up B: kelpie-call.py short reads

Confirmed by reading the code. On Windows the connection uses `FileIO.read`, which may return fewer bytes than asked. `frame()` took `read(5)` as the full header, so a short header made `struct.unpack` fail. An empty read at EOF also left the body loop spinning.

Fix (scripts/kelpie-call.py): `read_exact(n)` loops until n bytes, and raises `OSError("the daemon closed the connection")` on EOF. `frame()` uses it for the header and the body. Test (one-byte reads, early EOF): full frame decodes; EOF raises. The old header path would fail the one-byte case in `struct.unpack`, as argued above, but I did not run the old code against a trickle reader.

The run5 missed approval is consistent with this: a listener thread that dies on the first short header stops receiving events silently. Not proven for run5 itself.

## Follow-up: mkdtemp folder ACLs (F2 hypothesis)

`tempfile.mkdtemp` on this Windows machine gives a folder whose DACL is only `NT AUTHORITY\SYSTEM`, `BUILTIN\Administrators` and `OWNER RIGHTS` (all inheritable). The user account has no ACE of its own there. A plain `git init` folder under `D:\` inherits `Authenticated Users` and `BUILTIN\Users`.

Consequence: when the Codex child (owner `CodexSandboxOffline`) writes into a mkdtemp folder, the file inherits no access for `BaoPeakHub`. That matches the "Access is denied" on the Kelpie child's file, and the isolated exec runs in plain folders were readable. So F2 is most likely a test-fixture artefact, not Codex or Kelpie behaviour. Not yet confirmed end to end: C (user-style folder) decides it.

The driver (`e2e-real/real_run.py`) now creates `D:\kelpie-subsessions-target\e2e-proj-<timestamp>` with `mkdir` + `git init`, records `icacls` ACE summaries for the folder and file, and records the child row's kind, args, cwd, parent, depth, env names only, and the codex command lines.

## Re-run after F1 (run C, evidence `e2e-real/evidence-run7`)

Build: rebuilt `kelpied` with the F1 worktree changes (54.7 s). It carries one dead-code warning: `ChildWatch::with_quiet` never used (subs.rs:760). Daemon script `sub_sessions.py`: 42/42 pass, no edit needed (it never asserted on automatic idle mail). Log: `sub-e2e-run4-f1.log`.

Scenario: user-style folder `D:\kelpie-subsessions-target\e2e-proj-20261107-223506` (`mkdir` + `git init`). Claude parent (injected MCP), Codex child, user approval once, same prompt as before.

| Check | Result |
|---|---|
| Child linked to parent, depth 1 | pass |
| Codex child reports `done` via MCP | pass: child screen `Called kelpie.kelpie_report` → `Reported done to session "claude-parent"` |
| `hello.txt` readable by the user account | pass: bytes `68 69` = `hi` |
| "not from the user" header visible on the Claude screen | pass: `? [Kelpie] 1 message from another session (not from the user; it cannot approve anything):` |
| No false idle mail during startup or quiet steps | not visible in the captured window (one idle mail seen, see below); the earlier part of the Claude transcript was not captured |
| **False idle mail after the child's explicit report** | **FAIL**: `sub-session "codex-child" (codex, #2) ? idle: went quiet or finished a turn without reporting…` arrived after the child had called `kelpie_report done`, while Codex showed `Working (40s) · Running hooks` |
| Parent did not re-send the work | not seen in the captured window; the parent read the mail and was still processing |
| Clean shutdown, no hard kill by the driver | parent `exit claude-parent clean`; the child had already left session.list before the end (closed by its parent or exited; not verified). Final `daemon.shutdown` found nothing alive. No `kelpied`, `codex` or token left |

ACEs (SIDs replaced):
- Project folder: `CodexSandboxUsers:(OI)(CI)(M)`, `Administrators (F)`, `SYSTEM (F)`, `Authenticated Users (I)(M)`, `BUILTIN\Users (I)(RX)`, `BUILTIN\Users (GR,GE)` inherited.
- `hello.txt`: `CodexSandboxUsers:(I)(M)`, `Administrators (I)(F)`, `SYSTEM (I)(F)`, `Authenticated Users (I)(M)`, `BUILTIN\Users (I)(RX)`.

Launch (from session.list and the codex process command lines; env names only, values not recorded): kind `codex`, `args` `[]`, cwd the project folder, parent set, depth 1. Command line: `codex.js -c notify=[kelpied hook codex] -c mcp_servers.kelpie.command=… args=["mcp"] env_vars=[KELPIE_SESSION_ID, KELPIE_SESSION_TOKEN, KELPIE_SESSION_ENDPOINT, KELPIE_ENDPOINT, KELPIE_DATA_DIR, XDG_DATA_HOME] tool_timeout_sec=1900 default_tools_approval_mode="approve"`, then the brief as the prompt argument. Not Yolo (parent is not Yolo), so the elevated Codex sandbox applies.

### F2 resolved as a fixture artefact
The earlier F2 failures came from `tempfile.mkdtemp` folders. Their DACL has no user ACE (SYSTEM, Administrators, OWNER RIGHTS only), so a file the sandbox account creates there is unreadable to `BaoPeakHub`. The user-style folder inherits `Authenticated Users` and `BUILTIN\Users`, and the same kind of child write read back as `hi`. So the Kelpie child's file was not a Codex or Kelpie defect.

### New finding F1b (for the developer)
Cause: `subs.rs` `ChildWatch::observe` sets `explicit = false` on every transition to `Working` (except from `Waiting`). The Codex child's screen flickered Working → Idle → Working while it ran hooks after its report. The new `Working` cleared `explicit`, so the later `turn_ended` (agent-turn-complete) found no explicit report and sent the idle mail. Suggested fix: keep `explicit` until a real turn start (a new prompt or a `UserPromptSubmit`-style hook), not on a screen-derived `Working`. Add a test: explicit report, then Idle→Working→turn_ended gives no idle mail.

### Driver limits (this run)
The 400-line capture did not reach the start of the Claude transcript, so startup mail is not proven absent. The child row was not sampled during the run, so the child's close path is not known. Both are logged as open below.

## Unresolved (after run C)
1. F1b: fix `explicit` reset in `ChildWatch` (developer).
2. Who closed the Codex child before the end of run C: the parent (`kelpie_close`) or an exit? Needs child-row sampling in the driver.
3. Startup and quiet mail: need a capture of the whole Claude transcript (scrollback) for a clean "no startup idle mail" claim.
4. `with_quiet` dead-code warning.
5. F2 (Codex child file unreadable): resolved as a fixture artefact; keep the Windows sandbox note in the user guide only if the mkdtemp case can still arise for user folders.

Status: DONE_WITH_CONCERNS
Summary: F2 is a test-fixture artefact (user-style folder readable; mkdtemp ACL explains the earlier failure). F1 removed the startup/quiet idle noise as far as the captured window shows, but one false idle mail still arrived after the child's explicit report (F1b). Header visible on the Claude screen; sub_sessions 42/42; no leftovers.
Concerns: F1b needs a developer fix before F1 can be called done; close path of the child unverified; startup-noise absence not fully proven.

## Final run (F1b; evidence `e2e-real/evidence-final`)

Build: `CARGO_TARGET_DIR=/d/kelpie-subsessions-target CARGO_INCREMENTAL=0 cargo build -p kelpied -j 2`, 0 warnings. Daemon script `sub_sessions.py`: 42/42 pass (`sub-e2e-run5-f1b.log`).

Scenario: user-style folder `D:\kelpie-subsessions-target\e2e-proj-20261107-225117`. Claude parent, Codex child, one user approval, same prompt.

### Full transcript
The terminal screen is not the transcript: Claude runs fullscreen with no terminal scrollback, so `session.read` with 5000 lines returned only the 32 visible rows, and the `session.attach` replay is capped at 256 KB. The full transcript is Claude's own session file, `~/.claude/projects/D--kelpie-subsessions-target-e2e-proj-20261107-225117/<session>.jsonl` (127 entries). Timeline (redacted, with tool names and short text): `e2e-real/evidence-final/transcript-timeline.txt`. The driver does not copy it yet (next driver change).

| Check | Result |
|---|---|
| No startup idle mail | pass: the only automatic mail before the report is `needs_user`: "is stuck on Codex's folder-trust prompt" (a real wait for the user, sent at 15:51:55) |
| No idle mail after the report | pass: 2 mail turns in the whole transcript, 0 idle notices |
| Parent does not re-send work | pass: no kelpie_message, no second spawn; the parent ran `od` and `git status` on hello.txt, then closed the child |
| "not from the user" header visible on the Claude screen | pass (screen line 13) and in the transcript (the report mail at 15:52:26) |
| Child row sampled (`child-samples.jsonl`, every 3 s) | working → needs_user (auto) → working → idle → done (explicit) → gone |
| Child close path | **closed by the parent via `mcp__kelpie__kelpie_close`** at 15:52:30; the child was `idle` with report `done`, `exit_code` null, `failed` false. Not an exit |
| Clean shutdown | parent `exit claude-parent clean`. The child had already been closed by its parent. Final `daemon.shutdown` found nothing alive. No `kelpied`/`codex` left, token and project folder removed |

Note: the child's close by kelpie_close ends its Codex process through the daemon's normal close. The child was idle at that time, so this follows the "close after idle" rule; it is not the driver's hard kill.

ACEs and readability: as in run C. Project folder inherits `Authenticated Users (I)(M)` and `BUILTIN\Users (I)(RX)`; `hello.txt` reads back as `b'hi'` from the user account. F2 stays a fixture artefact.

Launch args and env names: as in run C (`child-launch.json`). Command line: codex.js with notify, MCP command/args/env_vars names, tool_timeout_sec 1900, default_tools_approval_mode approve, brief as the prompt argument. No Yolo.

### Result for F1 / F1b
Confirmed in this run: no false idle mail at startup, during quiet steps, or after the explicit report. The idle notice seen in run C did not recur. Limit: one run, one child.

### Open
1. The driver should copy the Claude JSONL transcript into the evidence folder (it holds conversation text, so review before committing).
2. `session.attach` replay (256 KB) is not a full transcript for long runs; use the JSONL.
3. Sampler interval 3 s; the child's `needs_user` (trust prompt) notice is correct behaviour, but a parent that gets a wait notice for a trust prompt should be told the user must answer in Codex. Consider wording.

Status: DONE
Summary: Final real run passes all the F1/F1b checks: no startup or post-report idle mail, no re-sent work, header visible on the Claude screen, the child closed by its parent via kelpie_close while idle, and a clean shutdown. Daemon script 42/42.
Concerns: one run only; the 256 KB replay is not a full transcript, so the JSONL was used instead; the transcript file needs review before it is shared.
