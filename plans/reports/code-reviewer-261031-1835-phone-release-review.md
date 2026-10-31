# Release review of the last five phone commits

Reviewed `7252558..f49c699` on `phone/link-on-master`, worktree `D:\kelpie-phone-rebase`.

**Score: 8.5/10.**

**Verdict: hold, for one small fix (W1).** After the first launch, a Linux AppImage cannot pair a phone at all, whatever polkit says. The polkit commit does not reach the person on the package Linux auto-updates through. Everything else can ship.

If the owner prefers not to fix W1 tonight, the alternative is to accept it and say so in the release notes. Then this is a ship. Nothing here is a security hole or data loss on any system.

Nothing was built or run, as instructed. Findings come from reading the code at `f49c699`. For `pkcheck` I read the current man page. My memory of polkit's source is marked as memory where it matters. Agent memory was not updated, because the task allows only this report to be written.

## Critical

None.

## Warning

### W1. An AppImage loses the right to pair as soon as it is reopened with the daemon still running

- **Where:**
  - `app/src-tauri/src/daemon_client.rs:109-110` and `:306`: `spawn_daemon` is the only caller of `daemon_runtime::runnable`, and it runs only when connecting failed.
  - `app/src-tauri/src/daemon_runtime.rs:133`, `:169`, `:176`: `source.txt` is written only from there.
  - `crates/kelpied/src/peer_identity.rs:29-39`: trust comes from `source.txt`.
  - `crates/kelpied/src/auth.rs:60-61`: the role is decided at `auth`.
- **Scenario:**
  1. First launch. The mount is `/tmp/.mount_AAA`, no daemon runs, so the app spawns one and writes `source.txt` naming `/tmp/.mount_AAA/usr/bin`. Pairing is allowed.
  2. The person closes the window. The daemon stays, by design, and the mount goes.
  3. Second launch, or an AppImage update. The mount is `/tmp/.mount_BBB` (`daemon_runtime.rs:27` says it moves on every launch). The connection succeeds at once, so nothing rewrites `source.txt`.
  4. `/proc/<pid>/exe` is under `.mount_BBB`. `trusted_apps` names `.mount_AAA`, and `same_file` fails on the missing path. The role is `Role::User`.
  5. `mobile.approve`, `mobile.set_permission` and `mobile.reset` are refused before polkit is asked. Settings → Mobile says "This window is not the installed Kelpie app" (`mobile-settings.tsx:161`), which is false.
- **Who it hits:**
  - Every AppImage user whose daemon outlived the window. That is the normal case, and `latest.json` updates Linux through the AppImage (`docs/architecture.md:70`).
  - After the update their token devices read "pair again", as in the earlier report's C1, and they have no way to do that short of stopping the daemon and every session.
  - `.deb`, Windows and macOS are not affected: their app path is stable.
- **Why this is not covered by the owner's decision:** "never run on real Linux" accepts that polkit may misbehave. This never reaches polkit, and the traced behaviour does not depend on anything about a real desktop.
- **Why tests miss it:** `copy_runtime` is tested alone. No test covers the mount moving while a daemon lives.
- **Security:** it fails closed, so nothing is exposed.
- **Fix (about 10 lines):**
  - At app start, before the first connection, and only when `APPIMAGE` is set, rewrite `source.txt` in each slot that has one, using `name_source(slot, daemon_exe().parent())`. This must run before the `auth` frame.
  - It lowers nothing: the file is the user's to write already (`docs/architecture.md:139`).
  - Add one unit test: a second `src` updates the file with no copy.
- **Alternative:** the release notes say "on the AppImage, pairing works only in the run that started the background service; use the `.deb` to pair".

## Suggestion

### S1. Oversized lines: the daemon moves the queue on before it answers, so a rollback can undo the next message's state

- **Where:** `crates/kelpied/src/chat.rs:1202` and `:1339-1341`, `app/src/chat/chat-model.ts:811`, `:828`, `:836`.
- **Scenario:** a turn is running, and the person has queued a 15 MB picture and then a second message. When the turn ends, this happens in order:
  1. The window builds the picture, so `busy` is true.
  2. The daemon refuses it. `turn_too_large` pumps inside the call, so `chat.turn_ready` for the second message goes out ahead of the error reply.
  3. On the usual order, `turnReady(q2)` runs first and sees `wasBusy = true`.
  4. The rejection for the picture then calls `rollback`, which sets `busy` to false while the second message is running.
- **Effect:**
  - Claude and Codex recover at the next stream event.
  - ACP has no such event (`acp-adapter.ts:87`, `:406` are its only `busy = true`), so that window shows idle for the whole second turn.
  - The daemon's queue stays right, because `commit_turn` recognises the turn line whatever `turn` says, and no message is lost.
  - It takes a rare trigger and clears itself, so I would not hold for it. The event-before-reply order is traced, not observed.
- **Fix, which also removes a path:** over `TURN_MAX`, refuse with `too_large` and change nothing. The window rolls back, then sends `{lines: null, too_large: true}`, the route it already uses for the frame limit. The daemon then pumps after the rollback. No test has a desktop message behind the picture, so add one.

### S2. `pkcheck` is found through `PATH`, and stderr is thrown away

- **Where:** `crates/kelpied/src/user_presence.rs:626`, `:665-670`.
- **Threat model:** a file named `pkcheck` in `~/.local/bin` that exits 0 passes the gate with no prompt and no restart. `docs/architecture.md:141-146` already accepts deliberate same-user code going around this gate (the file route, replacing the daemon), so this is not a new class. It is the quietest member of that class, though, because the other routes need a daemon restart.
- **Fix:**
  - Try fixed paths (`/usr/bin/pkcheck`, `/bin/pkcheck`, `/run/current-system/sw/bin/pkcheck`) with `env_clear()`, as `awake.rs:73` does for `caffeinate`. Otherwise answer `NO_POLKIT`.
  - Log `pkcheck`'s stderr with `tracing`. This code has never run on Linux, and today the first bug report will only say "polkit could not check this".

### S3. Window lock: a second call that ends can enable the window under a live prompt

- **Where:** `app/src-tauri/src/user_presence.rs:33`, `:51`; `crates/kelpied/src/daemon.rs:571-577`; `app/src/components/settings/mobile-settings.tsx:293-296`.
- **Scenario:** Deny is not guarded by `allowing()`, and there is a gap between the click on Allow and the dialog appearing. A Deny, or a click on a second request, that lands in that gap ends fast. Its `Unlock` then enables the window if the dialog has just come up. `drop(turn)` before the reply gives the mirror case for a prompt that follows immediately.
- **Impact:** only the "stays locked" half of the decision, for that one prompt, and only in that gap. Once the dialog is up the window takes no clicks, so no second caller exists. It can never leave the window stuck, which is the half that matters.
- **Fix if wanted:** an `AtomicUsize` of calls in progress, enabling only when it reaches zero. Also disable Deny while `allowing()`.

### S4. Failures of the environment count as refusals

- **Where:** `crates/kelpied/src/daemon.rs:564-569`.
- **Scenario:** with no polkit or no agent, three clicks get "You refused or did not answer the last 3 verifications" and a five-minute pause, with advice to revoke everything. The refusal message is wrong on a machine that simply lacks an agent.
- **Fix:** give `Unverified` a third kind for "could not ask". The same wording problem exists for the Windows setup errors, so this is not Linux-only.

### S5. Sealed `revoked` can still reach a replaced record when phone access turns off

- **Where:** `crates/kelpied/src/mobile.rs:2190`.
- **Scenario:** phone access is turned off in the milliseconds between a record being replaced and its old connection closing. The phone then gets the sealed `revoked` for a pairing that stands, and forgets its key. This is the one close path `key_paired` was not added to.
- **Fix:** `removed(&device.id) && !key_paired(&device.pk)`.

### S6. Pictures of about 12 to 24 MB that master sends are refused here

- **Where:** `crates/kelpied/src/chat.rs:39`.
- **What it does:** the person is now told, the queue moves and nothing is lost, so this no longer blocks. It is still a change from master for anyone who sends such a picture. Commit `567d055` keeps the limit on purpose.
- **Action:** one line in the release notes, or raise the limit towards `MAX_FRAME_LEN` in a later release.

### S7. Stale text

- `plans/261024-1205-phone-remote-control/plan.md:97-99`: "Merge is blocked by two things", with "before release" and "before merge". Both contradict the owner's decisions for this release.
- `plans/261024-1205-phone-remote-control/phase-03-kelpie-link-e2e.md:14`: "needs a run on a real Linux desktop before release". Same contradiction.
- `crates/kelpied/src/user_presence.rs:376-377`: "Should a parented dialog turn out to disable Kelpie's window". It does, and that was decided.
- `app/src/chat/transport.ts:37`: does not list `too_large` or explain `tooLarge`.
- `docs/architecture.md`, the 4409 sentence: lost its list indentation. Cosmetic.

## The five checks

**1. `EnableWindow` after `mobile_secure_call`: correct, apart from S3.**
- The hint is `get_webview_window("main")`, this process's own top-level window.
- `Unlock` is created only once the hint exists and lives across `client.call`, so every return path enables the window, including a dropped future.
- A dead daemon's disconnect is prompt on every path:
  - `read_loop` returns and `on_disconnect` drains `pending`.
  - `pending` is inserted before `send_frame` and `out` is cleared before the drain, so no request can be missed.
  - An unreachable daemon errors within 10 s, and a hung one within 300 s.
- If the app's call timed out or lost the connection while a dialog was still up, that dialog enables its parent when it closes.
- The abandoned password dialog at 280 s stays up with the window enabled, as documented.
- `Win32_UI_Input_KeyboardAndMouse` is the right feature for `EnableWindow`.

**2. Reuse in `pair.ts`: sound.**

| Case | Result |
|---|---|
| Revoked while connected | Sealed `revoked` runs `forgetPairing`, so `held` is null and a new key is made |
| Revoked while offline | The key stays and is reused. A new record still takes a live code, six digits and verification |
| Bad stored key | `valid()` rejects a wrong shape, giving a new key. A wrong `priv` of the right length just gives another fingerprint and a new record |
| `reset`, or another computer | A different `dpk`, so a new key |
| Same `dpk` from a forged QR code | IKpsk1 sends the static key encrypted to `dpk`, so only the real computer can read it. A wrong `psk` fails the handshake and `held` is untouched |
| Page served by something else | Only the same origin reads this IndexedDB, and such a page could read the key before this change too. Reuse adds nothing |
| Stranger who knows a public key | `ss` needs the private key, so the first message does not open. The approval also needs a live code, the digits and the person |
| Raising or lowering by replacing | `check_approval` then `after_verifying` always runs, with the permission chosen in the dialog. A re-pair can lower only by the person's own choice |
| Dormant data | The clock restarts only after verification |
| Audit data | Nothing removed. The `pair` line carries the same fingerprint |
| Grants and queues of the old record | Stopped and dropped |
| `replaced` vs `revoked` | The sealed message is decided in the daemon, by event or by `key_paired`. A forged 4409 only makes a page wait, as before. 4409 never forgets a key. See S5 for the one gap |
| Other consumers of the event | `mobile.rs:1691` is the only reader of `change`. Desktop clients hear only `added`. Phones handle `replaced` |
| Failure path | `link.stop` ran when the code was read, so `link.start` opens one socket. No code path leaves a tab with a false "paired" |

**3. `pkcheck`: correct against the man page, apart from S2.**
- **Arguments:** valid.
- **`--process <pid>`:**
  - The man page warns against the bare form because of races. Those are about a subject that can exit or exec a setuid program.
  - Here the subject is the daemon itself, alive and waiting on its child, so I see no real race.
  - `pid,start,uid` would follow the man page's advice, but this is not worth holding for.
- **Exit codes:** 0 passes. 1, 2, 3, 126, 127, a signal and a missing binary all refuse. The mapping matches the man page. Three wrong passwords give exit 1 and "polkit did not allow this account", which is imprecise but closed.
- **Agent:** `--allow-user-interaction` with no agent exits 2. `--enable-internal-agent` is not passed and stdin is null, so nothing can ask on a terminal.
- **Time limit:** `wait` polls `cancel`, kills and reaps. From memory of polkit's source, its daemon drops an authentication session when the client's bus name goes away. The plan already lists this as not yet seen on a real desktop.
- **Reaping:** no code under `crates/` handles `SIGCHLD` or calls `waitpid`, so `try_wait` sees its child.
- **Environment:**
  - Inherited in full. An AppImage's additions were already removed by `clean_env` before the daemon started.
  - This fails closed if a stray `LD_LIBRARY_PATH` breaks the child. See S2 for `PATH`.
- **`cfg`:**
  - `polkit` is built on every system, so a Windows build has already compiled it.
  - The Linux `os` module is three lines that call it with matching types.
  - The macOS `os` is untouched.
  - `on_fire` is unused on non-test Linux. That is only a warning, and CI has no `-D warnings`.
  - `staying_child` works on Linux and macOS.
  - CI on `ubuntu-22.04` remains the first real compile.

**4. Too-large messages: correct, apart from S1.**
- **Told on both sides:** the `chat.queue` event lists it in `dropped`, and `kelpie_unsent` reaches phones and replay. `whyUnsent` gives the same words live and replayed.
- **Queue:** `turn_too_large` matches the same conditions as `write_turn`, so only an offered desktop message of that client can leave. It removes one entry and pumps.
- **Nothing else dropped, and no re-offer loop:** the entry is gone, and asking again answers `too_late`.
- **Frame limit:** the text is `too large to send to the daemon` (`daemon_client.rs:289`). It matches `/too large/i` and reaches the daemon as `too_large: true`.
- **Other errors:** a final no, which writes plain text or drops with a note. A lost connection cannot reach the daemon, so the second `writeTurn` fails quietly and `client_gone` decides.
- **Replay:** `applyLog` only calls `unsentItem` and `onUnsent`. `turnReady` declines while replaying, and `write` drops anything during a replay. The rejection handler runs only after a live `writeTurn`. So no commands are sent.
- **Across replay:** same id, same text format, and older lines without `images` still read. Clipped text in the log line replacing the full live text is older behaviour.
- **`noteSent`:** the fix to look up by `cid` only for `by === "desktop"` is correct and tested.

**5. Docs and hygiene: true, apart from S7.**
- `docs/architecture.md` and `docs/user-guide.md` match the code on the lock, polkit's text, refusal, the time limit, `too_large`, `images`, level 18 and 19, and API 21 for token access.
- The guide's "Kelpie's own dialog names the phone, its key and what it asks for" matches `mobile-settings.tsx:318-323`.
- **Commits:**
  - All five have author and committer `baopeak` with the noreply address.
  - None has an AI line.
  - None has a phase number or finding code in the message or in added code.
  - The phase and plan mentions are in plan files only.
- **Not in this tree:** I found no release notes file, so the Linux statement the owner decided on still has to be written.

## The author's report, read after the findings above

It changes no finding. `plans/reports/fullstack-developer-261031-1800-phone-link-fixes.md` matches the code on everything I compared. Three points add to the above:

- It lists what polkit may do on a real desktop as unobserved, but it does not mention W1, which comes before polkit.
- Its compile of the Linux code was the `polkit` and `os` modules alone, with a stand-in for `Cancel`. That confirms CI will be the first compile of the whole Linux daemon.
- Checks ran on the working tree before it was split into commits, so the intermediate commits were never built alone, and one docs edit came after the checks. This matters only for bisecting. The final tree is what ships.

## Not checked

- Runtime behaviour on any system. W1 and S1 are traced, not observed.
- Linux and macOS compilation, TypeScript type checks and the test suites.
- Windows crate signatures beyond reading how they are used.

## Unresolved questions

1. W1: fix tonight, or accept it and put the AppImage limit in the release notes? This decides the verdict.
2. Should the 16 MiB limit stay, given that master sends pictures up to about 24 MB (S6)?
3. Once Linux runs for real: does a daemon started by the app find the session's agent on Ubuntu 22.04's older polkit? It fails closed if not, and the plan already lists this as open.
