# Phone link fixes on `phone/link-on-master`

**Status: DONE_WITH_CONCERNS.** All seven items are done and every check passed on the first finished run. `phone/link-on-master` moved from `7252558` to `f49c699` with five new commits. Nothing was pushed. The main concern is that the Linux verifier has not run on Linux, and the whole daemon could not be type-checked for Linux here.

## Commits

| Commit | Subject |
|---|---|
| `b0412ba` | fix(mobile): never leave the app's window locked after a verification prompt |
| `973d745` | fix(mobile): pair again from the same browser under the same key |
| `71b681f` | feat(mobile): verify the person through polkit on Linux |
| `567d055` | fix(chat): say why a queued message was not sent and keep the queue moving |
| `f49c699` | docs(mobile): describe the locked window and polkit, and correct stale API numbers |

- Author is `baopeak <322063315+baopeak@users.noreply.github.com>`, with no AI lines, phase numbers or finding codes.
- API levels stay at 21 and 22. Nothing was folded into earlier commits, because no fix belongs to an API bump.
- `phone/phase3-link` (`8ec3ef7`) and `phone/phase2b-chat-send` (`d6e5883`) did not move. The main tree was not touched apart from this report.
- W1 and W2 share one commit because they change the same lines in the same functions.

**How the tests relate to the commits.** All checks ran on the working tree. A script then split it into commits and confirmed that the last commit's tree equals it, after one final docs edit made once the checks had run (a queue paragraph and a 4409 sentence in `architecture.md`, a blocker line in `plan.md`). The intermediate commits were not built on their own.

## What changed

### 1. Window stuck disabled

- **Cause:** nothing re-enabled the window. The client did notice the dead daemon, since the pipe ending drains every pending call, so no change was needed there.
- **Fix:** `mobile_secure_call` holds a guard (`Unlock`) that calls `EnableWindow` when the call ends, whichever way it ends. The parent stays.
- **Dependency:** `Win32_UI_Input_KeyboardAndMouse` is added to the app's `windows` crate features. Another dependency already enabled it, so nothing else rebuilt.

**Proof,** through the app's own command on an isolated endpoint, with no `KELPIE_TEST_USER_PRESENCE` and `seam: None`:

| Moment | `IsWindowEnabled` |
|---|---|
| Before the prompt | True |
| 0.3 s after the call | False, 2 s later still False, and the call had not ended |
| Daemon killed (PID 2768) | |
| First enabled | **0.05 s after the kill** |
| +0.1 s through +15 s, sampled 9 times | True |

The call ended with `daemon disconnected`. The earlier part B test stayed disabled for 20 s without the fix.

- **Limit of this proof:** my script's search for the dialog's window found none at any point, including while the app's window was disabled. It looks for visible titled windows of `CredentialUIBroker`, `CredentialEnrollmentManager`, `WWAHost` and `consent` that started after the prompt. So I know the window was locked and came back, but I did not see the dialog itself. If the owner saw the dialog at 18:22:43 to 18:23:14, that closes the gap.
- Nothing typed into it. No `CredentialUIBroker` process remained afterwards.
- The script stopped its daemon by PID, then found no build process left: "this build's processes still running: 0".

### 2. Pairing again from the same browser

- `pair()` takes the pairing the browser holds and reuses its key when the computer's public key matches. Another computer, or nothing stored, gets a new key.
- **Beyond what you listed:**
  - **The old record's connection now closes as replaced (4409), not revoked.** Without this, another tab of the same browser would get the sealed `revoked`, forget the shared stored pairing and wipe the one just approved. Replacing a record now sends `change: "replaced"`. Other phones still see "a device was removed".
  - **A failed attempt now goes back to the held pairing.** A phone that already holds a pairing returns to it and shows what went wrong as a banner. Before, it stayed on the pairing card until a reload.

### 3. Linux: polkit

- `crates/kelpied/src/user_presence.rs` runs `pkcheck --action-id org.freedesktop.policykit.exec --process <own pid> --allow-user-interaction`.
- **Why `pkcheck` and not `pkexec`:** it runs nothing as root, and its exit code says what happened. `pkexec` would have to start a program as root just to prove the person.
- **Fails closed:** only exit 0 passes.
  - No `pkcheck`: says to install polkit and an agent.
  - Exit 2: no agent.
  - Exit 1: account not allowed.
  - Exit 3: cancelled.
  - Anything else, including a signal: could not check.
  - All four new messages are in the translation table and in `phone-backend-messages.test.ts`.
- **Time limit:** the wait polls and ends the child when the prompt's time runs out.
- **Approval dialog:** no change was needed. It already shows the device's name, key and what it asks for before it asks.
- **Tests:** arguments, exit codes, a missing program, and the time limit ending a real child (`ping` or `sleep`). No test reaches polkit.

**Linux type-check:** `cargo check -p kelpied --target x86_64-unknown-linux-gnu` **does not work here**. The target is installed, but a dependency's build script stops looking for `x86_64-linux-gnu-gcc`. So I ran `rustc --target x86_64-unknown-linux-gnu --emit=metadata` on the `polkit` and Linux `os` modules alone, with a stand-in for `Cancel`: exit 0, no messages. The rest of the Linux build has not compiled on this tree. The `polkit` module is also built, clippy-checked and tested on Windows because it has no platform code.

**Reasoned about, not observed**, and stated in the plan:

- What polkit's agent actually shows.
- Whether a daemon outside the logged-in session finds the agent.
- A non-administrator account failing.
- Ending `pkcheck` taking the dialog off the screen.

That action's default asks for an administrator's password, so a non-administrator cannot pair a phone on Linux. The docs say so. A distribution's polkit rules can also change what it asks, or pass with no prompt.

### 4. W1: message too large

- Lines over 16 MiB now remove the message from the queue with reason `too_large`, so `chat.write_turn` no longer leaves it offered. The window, the phones and the log are all told, and what waits behind it moves on. The limit is unchanged.
- A window whose own connection refuses the lines (over the 32 MiB frame limit) says so with `{lines: null, too_large: true}`.
- Any other failure except `too_late`, `busy` and `switching` is a final no.
- A new note, translated: "Not sent (too large to send to the agent)". Tests cover the desktop, a phone and the daemon.

### 5. W2: picture names in notes

`Dropped` and `kelpie_unsent` now carry `images`, and the phone's `chat_queue` passes them on. Live notes, later log lines and replays keep the names, and older log lines still read. Side effect: the log path now uses the same reason mapping as the live path, so a replayed `revoked` no longer reads as "could not prepare it".

### 6. W3 and S1 to S4

- `docs/architecture.md`, `daemon.rs` and `mobile/store.rs` are corrected. A fresh sweep of code, docs and scripts for API 18 and 19 finds none.
- S1 done: only `by === "desktop"` reads this window's pictures, with a test.
- S4 done.
- **S2 not done:** it is only worth doing if the branch is rewritten, and this round added commits instead.
- **S3 not done:** it needs a push, which is not allowed. CI still has not seen this tree.

### 7. Docs and plan

`docs/architecture.md` covers the locked window and that requests and sessions keep going, plus polkit in the text and the platform table. The guide says the same in plain words. `phase-03-kelpie-link-e2e.md` records the manual results and what has not been run, and that Linux needs a real desktop run before release. `plan.md`'s blocker line is updated too.

## Results

Load is CPU % at start/end. Everything is on the fixed tree with `CARGO_BUILD_JOBS=1`.

| Check | Result | Load | Notes |
|---|---|---|---|
| `pnpm typecheck` | exit 0 | 31/34 | |
| `pnpm test` | **1328 passed, 68 files** | 46/25 | was 1308 |
| `pnpm i18n:check` | 0 missing, 0 problems (2463 used, 2677 translated) | 24/32 | |
| `pnpm build:phone` | exit 0 | 26/35 | |
| `cargo test`, four crates, `--no-fail-fast` | **875 passed, 0 failed, 11 ignored** | 34/16 | 334 s. `kelpie` 257, `kelpie-link` 12, `kelpie-proto` 29, `kelpied` 577 |
| clippy, three crates, `--all-targets` | exit 0, 1 warning from master | 34/24 | `lib.rs:370`, unchanged, not this work's |
| `link_noise.py` | vectors ok | 31/38 | |
| `cargo build -p kelpied` | exit 0 | 74/17 | |
| `daemon.info` | `api=22`, `security=2`, isolated | 24/64 | |
| `mobile_protocol.py` | **188 passed, 0 failed** | 21 at end | 85 s; was 182, adds the six same-key checks |
| `mobile_resilience.py` | **55 passed, 0 failed** | 4 at end | 487 s |
| `pnpm tauri build --debug --no-bundle` | exit 0 | 4 at end | 76 s |
| `acp_chat.py`, fake agents | **56 passed, 0 failed** | 7/5 | |
| `mobile.py`, once | **107 passed, 0 failed** | 4/10 | endpoint OK, data dir OK, seam `approve` |
| Item 1's proof | pass, first enabled 0.05 s after the kill | 4/3 | see above |

`twenty_cached_summaries_are_fast` passed first time. No time-dependent failure and no retry. Nothing was stopped for memory.

## Failures as they came

1. **`phone/src/chat-send.test.ts` did not typecheck,** because a required `images` field was missing on `DroppedItem`. I made the field optional, which also fits older events.
2. **`cargo check` failed:** a third `no_access` call and a test's `Dropped` initializer were not updated. Both fixed.
3. **`pairing_again_with_the_same_key_says_which_record_it_replaced` failed** on my new assertion, because the test setup also has a phone called "phone". I narrowed it to this phone's records.
4. **Linux type-check** failed as described above.

## Safety notes

- **Real prompt:** your earlier rule was no real OS prompt, and this round's instruction asked for one in item 1's proof. It ran exactly once, on an isolated daemon, with nobody typing.
- The real app (PID 4368, started 12:42:03) and daemon (PID 268, started 13:38:51) kept the same PIDs and start times at the end. I never launched or called them, and left the `omp` processes alone.
- No cargo, rustc, `tauri-driver` or `msedgedriver` is left.
- A scan of my logs for `#p=`, `"psk"` and `KELPIE_SESSION_TOKEN` found nothing. No test keys are in the credential store.
- **A blocked removal:** I tried to delete my Linux check output inside my own build directory, and a guard refused because the path resembles the protected worktree. I did not look for another way, and left the 0.025 GB there.
- **More leftover tokens:** the real `%LOCALAPPDATA%\Kelpie\auth\` now has ten test token files since 15:00. This round's runs added `kelpie-level-11504`, `kelpie-m-9472`, `kelpie-phone-protocol-11768` and `kelpie-phone-resilience-4428`. The others (`kelpie-level-11040`, `kelpie-m-892`, `kelpie-phone-protocol-9560`, `kelpie-phone-resilience-68`, `kelpie-manual`, `kelpie-selftest-12264`) came from earlier runs, the last two not from mine. My proof script removed its own. I removed nothing else there.

## Not run

- A real prompt on any path other than item 1's short run. No password dialog, Hello Face, or Microsoft or PIN-only accounts on the fallback.
- macOS and Linux builds as a whole. Nothing ran on Linux.
- CI.
- `mobile_perf.py` and `stress.py`, which were not on the list.
- A real `too_large` message end to end. It is covered by unit tests at both ends, not with a real 15 MB picture and a real agent.
- Two real browser tabs for the replaced-record case. The daemon's side and the phone's close handling are tested, but not the two together in a browser.

## Disk at the end

| Item | Size |
|---|---|
| `D:\kelpie-phone-rebase-target` | 4.71 GB |
| `D:\kelpie-phone-rebase` | about 0.28 GB as counted earlier; not re-measured |
| Free on C | 9.92 GB |
| Free on D | 2.62 GB |

The worktree stays on `phone/link-on-master`, and both are left in place. Nothing needed freeing.

## Unresolved questions

1. **Did the owner see the Hello dialog during the proof (18:22:43 to 18:23:14), and did it go away by itself after the kill?** My script could not see it.
2. **Non-administrators on Linux cannot pair a phone**, because of that action's default. Is that acceptable for release, or should a dedicated polkit action be shipped? That would also allow Kelpie's own text, but it needs a policy file installed with the package.
3. **Should the test token files be deleted, and should the phone e2e scripts remove their own?**
4. **Master's clippy warning** at `lib.rs:370`: fix it separately, or leave it?
5. **Standing blocker from `plan.md`, unchanged:** eight phones cost about 10 points of a core against a limit of 5.

---

# Last commits before the PR

**Status: DONE_WITH_CONCERNS.** All six items and review S1 are committed, one commit each. `phone/link-on-master` moved from `f49c699` to **`b638e7d`**. Nothing was pushed. Every check on your list passed, and these ran on `b638e7d` itself, so nothing changed after them. Nothing was stopped for memory.

## Commits

| Commit | Subject |
|---|---|
| `ef3d82d` | fix(mobile): let an AppImage pair after it is reopened |
| `64b2ff1` | fix(mobile): do not count a computer without polkit as a person refusing |
| `eaef270` | test(agents): retry a read refused while the hook settings are being replaced |
| `06dc6c8` | ci: run once per pull request and keep artifacts for three days |
| `6d8a8a6` | test(e2e): remove a phone test daemon's token file at exit |
| `8043559` | docs(plans): record what the owner decided for this release |
| `b638e7d` | fix(chat): offer the next message only after a too-large one is undone |

Author is `baopeak` with the noreply address. There are no AI lines, phase numbers or finding codes. API levels stay at 21 and 22. `phone/phase3-link` (`8ec3ef7`) and `phone/phase2b-chat-send` (`d6e5883`) did not move. The intermediate commits were not built on their own.

## What changed

1. **AppImage.** At start, before the first connection, an AppImage rewrites `source.txt` in each slot that already has one. Debug builds and `KELPIE_DAEMON_PATH` are left out, matching `runnable`. The misleading "This window is not the installed Kelpie app" case needed no change: reading the code, this cures the AppImage case, because the daemon reads `source.txt` on each `auth`. **This has not run on Linux.** The unit test proves the file is rewritten with no copy, not that a real AppImage pairs.
2. **polkit missing.** `Unverified` has a new `could_not_ask`, set for no polkit or no agent, and those no longer count towards the pause. The audit line stays. A person's own no still counts, and the test checks both. `pkcheck`'s stderr goes to the debug log, read on a thread and kept to 4 KiB. It carries no secret, since the agent asks for the password, not `pkcheck`.
3. **Flaky test.** The test retries `PermissionDenied` only. Every other read error panics, and the torn-JSON assertion is unchanged. See the finding below.
4. **CI.** `on.push` is limited to `branches: [master]`, and `pull_request` stays. `retention-days: 3` is on both `upload-artifact` steps. `release.yml` is untouched and I did not read it. A pushed branch with no PR no longer runs CI.
5. **Test cleanup.** `phone_support.isolate()` now removes its own endpoint's exact token file at exit. It refuses any endpoint it did not make there. `scripts/kelpie-call.py` gained `token_path()` so the path is worked out in one place.
   - **Checked:** this run's `kelpie-phone-protocol-4568.token` is gone, and the protocol count stayed at 107 before and after.
   - **Not covered:** `mobile.py` does not go through `isolate()`, because its endpoint comes from its runner, so it still leaves one. I did not delete any of the older files.
6. **Plan.** `plan.md` and the phase 3 file now say the owner decided on 2026-10-31 to release first and reduce phones' CPU in a later release, and that polkit is released untested on a real Linux desktop, stated in the release notes. **No release notes file exists in this tree, so that sentence still has to be written.**

**Review S1: applied, as it was small.** Over the limit, the daemon now refuses and changes nothing. The window rolls back and then sends `{lines: null, too_large: true}`, so the next message is offered only after the rollback. This removes the path that could mark a running turn idle. There is a new test with a desktop message behind the picture.

**Left, as told:** S3, S5, S6. Also left, because you scoped S7 to the plan text: the stale comment at `user_presence.rs:376` and the 4409 sentence's indentation in `docs/architecture.md`.

## Can a real reader fail on the hook settings file?

**Yes in principle, but in a very narrow case. No production code changed.** This comes from reading our code. I did not test Claude, and I do not know what it does if the read is refused.

- `write_claude_hook_settings` rewrites the file only when its contents differ, and the contents depend only on the daemon's path. So a daemon rewrites it at most once per path: on its first launch after moving to a new slot or build. The test forces a different path on every one of its 256 writes.
- As far as I know, Node opens files with the same sharing as Rust's `std::fs::read`, so a Claude reading in that same moment could be refused too. I did not verify this.
- So a launch can be hit only if it reads in the moment a launch after a daemon move replaces the file.

## Results

Load is CPU % at start/end, where `step.ps1` recorded it. Everything is at `b638e7d` with `CARGO_BUILD_JOBS=1`.

| Check | Result | Load | Notes |
|---|---|---|---|
| `cargo test -p kelpied --bin kelpied` | **580 passed, 0 failed, 3 ignored** | 3 at end | 169 s |
| `cargo test -p kelpie --lib daemon_runtime` | **5 passed, 0 failed, 259 filtered** | 31 at end | 66 s |
| `cargo test -p kelpied agents`, 20 times | **20 of 20 passed; each 19 passed, 0 failed** | 25/62 | 25 s |
| clippy, three crates, `--all-targets` | exit 0, 1 warning from master | 53 at end | `lib.rs:370`, unchanged |
| `pnpm typecheck` | exit 0 | 34/46 | |
| `vitest src/chat/chat-queue.test.ts` | 44 passed | 43/58 | not on your list; I changed that test |
| `cargo build -p kelpied` | exit 0 | 60/45 | 167 s |
| `mobile_protocol.py` | **188 passed, 0 failed** | 48/29 | 110 s |
| Linux verifier alone, `rustc --target x86_64-unknown-linux-gnu` | exit 0 | | 1 dead-code warning from the check's own stand-in |

- **The 20 passes do not prove the retry fired.** I never saw the `PermissionDenied` here, so this shows the test is not broken, not that the CI failure is cured. CI on Windows is the real check.
- **Not rerun after these commits:** the full `pnpm test`, `cargo test` on the other crates, `mobile_resilience.py`, `pnpm tauri build`, `acp_chat.py`, `mobile.py` and item 1's window proof. They were not on your list.
- **CI will be the first compile of the whole Linux daemon.** The full Linux `cargo check` still cannot run here.

## Safety notes

- The real app (PID 4368) and daemon (PID 268) kept the same PIDs and start times. I did not launch or call them, and I left the `omp` processes alone.
- No cargo, rustc, `tauri-driver` or `msedgedriver` is left. No secrets in the logs: a scan found 0 hits.
- `git restore` on one file in my own worktree, to undo a half-applied edit of my own script.

## Disk at the end

Build directory 5.26 GB. Free on C 9.83 GB, on D 6.15 GB. The worktree stays on `phone/link-on-master`.

## Unresolved questions

1. **Who writes the release notes?** They must state that polkit is untested on a real Linux desktop, and review S6's 16 MiB limit on pictures that master sends is worth one line too.
2. **Old test token files:** 107 protocol files remain in the real `auth` folder, plus others. Should they be deleted? From now on `isolate()` runs clean up after themselves.
3. **Master's clippy warning** at `lib.rs:370`: fix it separately, or leave it?
