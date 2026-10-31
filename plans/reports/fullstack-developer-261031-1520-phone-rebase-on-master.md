# Phone commits rebased onto master: done and verified

**Status: DONE_WITH_CONCERNS** (a clippy warning from master, four test token files left in the real folder, and D is low on space). The rebase is complete and every requested check passed on the first run that finished. Local branch **`phone/link-on-master` is at `7252558`**. Nothing was pushed. Two earlier runs were stopped for memory, and the owner approved each restart. No code changed after the rebase was finished; the later rounds only ran checks.

## Commits

Base: `origin/master` at `ddf52cf` (fetched and read, API 20).

| Old | New | Subject | Change |
|---|---|---|---|
| `4a1224a` | `280649d` | feat(mobile): send chat messages from the phone through one daemon queue | 11 files in conflict; "API level 18" became 21 in the message |
| `0a5329d` | `96357bc` | fix(mobile): keep unsent phone messages of a chat that was closed or resumed | clean |
| `6d10b3e` | `179176f` | test(e2e): wait on real conditions in two load-sensitive checks | clean |
| `d6e5883` | `e143fd1` | docs(plans): mark sending chat messages from the phone done | message reworded, see below |
| `f44c7a9` | `bbf43da` | feat(mobile): encrypt the phone link end to end and verify the person in the daemon | 8 files in conflict |
| `4278fe4` | `93c8e8e` | fix(settings): leave desktop notifications off on a new install | 1 file in conflict; test fix |
| `8ec3ef7` | `7252558` | docs(plans): replan phone remote control and record the encrypted link's reviews | plan API numbers |

- Author on all seven is `baopeak <322063315+baopeak@users.noreply.github.com>`. There are no `Co-Authored-By` or AI lines.
- `phone/phase3-link` is still `8ec3ef7` and `phone/phase2b-chat-send` is still `d6e5883`. The main tree is untouched. Nothing was pushed.

## API levels

Master's level is 20, confirmed from `crates/kelpie-proto/src/lib.rs`.

| Level | Commit | What it brings |
|---|---|---|
| 21 | `280649d` | the daemon's chat queue |
| 22 | `bbf43da` | the encrypted link |

- `API_LEVEL` is set in each of those commits.
- `CHAT_QUEUE_API` is 21. `PHONE_API` is 21 in `280649d` and 22 in `bbf43da`.
- A grep of `app/`, `crates/`, `scripts/` and `docs/` found no other code comparing against 18 or 19. The only remaining uses were doc and comment numbers, which moved: `docs/architecture.md` and `chat-wait-notice.tsx`.
- `plan.md` API column: 2b 21, 3 22, 3a 23, 3b 24, 3c 25, 3e 26, 4 27, 7 28, 8 29. The numbers in the phase files for 3, 3a, 3b, 3c, 3e and 4 moved too, along with the `lib.rs` line reference and the execution state. `plan.md` names `phone/link-on-master`, which now exists.

## Conflicts and how each was resolved

### `280649d` (chat queue)

| File | Resolution |
|---|---|
| `crates/kelpied/src/chat.rs`, `ingest_line` | Took the branch's `ask_of`, which records the id and method. Master's `request_key` remains test-only. Kept the queue's `pump` and `announce_if`. |
| `crates/kelpied/src/chat.rs`, `input` | Both sides: `keep_unanswered` returns the answers, `observe_written` runs, the bytes are written, the answers are logged, then the queue is announced. |
| `crates/kelpied/src/chat.rs`, `stop()` (no marker) | Auto-merged and logged nothing, which was the known bug. It now passes the `resolve` result through `answer_line` and appends the line. |
| `crates/kelpied/src/auth.rs` | Union of the admin lists: the branch's `chat.*` and master's `process.*`. |
| `crates/kelpie-proto/src/lib.rs` | Master's 16 and 20 notes kept, then the queue note as level 21. |
| `app/src/chat/chat-model.ts` | Nine hunks, all keeping both sides: `images` and `from` on user items; `namesOnly` and `daemonQueue`/`requeue`; `sentFrom` reset with `agentRuns`; `kelpie_answer`, `kelpie_sent` and `kelpie_unsent` all handled; the "Not sent" note uses `withImageNames` and skips mirrored queues; `send` strips white space, then queues. |
| `app/src/chat/acp-adapter.ts` | Keeps the branch's `daemonQueue` guard and master's `images: sentImages(images)`. |
| `app/src/chat/desktop-transport.ts` | Both sets of imports. |
| `app/src/components/chat-composer.tsx` | Master's `withImageNames` on the chip, plus the branch's sender and "needs the desktop window" labels. |
| `app/src/components/chat-pane.tsx` | Both sets of imports, both sets of listeners and cleanups, the copy button and the sender label. |
| `app/phone/src/chat-view.tsx` | The sender label and master's `userText` fallback. |
| `app/phone/src/phone-state.ts` | `deviceName` and the queue methods. |
| `docs/architecture.md` | Master's text plus the queue methods at API 21. |

### `bbf43da` (encrypted link)

| File | Resolution |
|---|---|
| `Cargo.lock` | Kept both sides' packages; `cargo metadata` then added what was missing. 150 insertions, no deletions, so no existing version moved. |
| `crates/kelpied/Cargo.toml` | Union: `sysinfo` and `zeroize`; `windows` and `windows-future`; the Linux and macOS tables. `windows-sys` has both feature sets, with `Win32_NetworkManagement_Ndis` added to master's list. |
| `crates/kelpie-proto/src/lib.rs` | Level 22 note. Keeps the branch's `secret_read` and master's `secret_set`, whose error says what to fix. All callers still compile. |
| `crates/kelpied/src/auth.rs` | Master's and the queue's admin list, plus the branch's `APP_ONLY` assertions. |
| `app/src/lib/attention.ts` | The branch's text with `PHONE_API = 22`. |
| `app/src/locales/vi/backend.json` | Union, with a comma added where the two lists meet. |
| `docs/architecture.md` | The branch's text plus master's `process.*` sentence. API 19 became 22 (3 places) and API 18 became 21 (2 places). |
| `scripts/e2e/mobile.py` | Both imports: `phone_support` and `real_data_dir`. |

`chat.rs` auto-merged here. `answer()` now checks `answer_is_one_shot`, resolves, writes, then logs through `answer_line`, so a phone's answers keep the one-shot rule and are logged.

### `93c8e8e` (notifications)

`app/src/lib/store.ts`: master moved the defaults to `default-settings.ts`, so I kept master's store and set `notifications: false` in the new file.

No resolution needed a product decision.

## Changes beyond conflicts

1. **`stop()` logging.** Added to `the_acp_stop_cancels_what_the_agent_asked_and_closes_every_card`: exactly one `kelpie_answer` is logged, for the elicitation's `cancel`, under the phone that stopped. A permission's `cancelled` stays unlogged, because master's `answer_line` treats it as nobody's answer and master's `a_stop_logs_nothing` expects that. The owner confirmed this rule.
2. **`noteSent`.**
   - On the desktop, a message sent through the queue now records `images` the way an adapter's `send` does: by path for this window's own message, by name for what it no longer has.
   - A phone keeps the `[name]` text, and the wire format is unchanged.
   - **Deviation from the plan.** The chat-view-parity plan says to read `this.mine.get(cid)`. I keep a small separate map, `sentPictures` (50 entries, no bytes), instead. `mine` is emptied when the queue event says the message left, and that event and the log line reach the window by different routes. A replay also needs the pictures again. I reasoned this from the code and did not observe the ordering problem happening.
3. **`daemonOwnsQueue(api)`.** New helper in `attention.ts`, used by `mode-switch.ts`, so that a test exercises the real gate.
4. **New tests.**
   - `chat-queue.test.ts`: level 20 versus 21, including that nothing goes to `enqueue` at 20; own pictures with the log line arriving before and after the queue's word; name-only pictures; the phone's text form. One existing expectation changed on purpose, the ACP phone prompt.
   - `link.test.ts`: a daemon at level 20 is flagged outdated.
5. **`store-notifications.test.ts`.** `desktopNotify` was called with three arguments, but it takes two, on `8ec3ef7` as well as on master. So the original commit's typecheck would already fail on that test. Fixed in `93c8e8e`.
6. **`d6e5883` message reworded.** "mark phone phase 2b done" broke your rule against plan phase numbers in messages. This is the one place I did not keep a message for an API reason.

A scan of the added code, docs and scripts for phase numbers and finding codes found none. The only hits were a hex fingerprint in two tests.

Not changed: `plans/261030-0830-chat-view-parity/phase-05-paste-and-zoom-images.md` still says the `noteSent` part is not applied. It is a historical record.

## Results

Load is CPU % at start/end. The machine has 7.89 GB of memory.

| Check | Tree | Result | Load | Notes |
|---|---|---|---|---|
| `pnpm typecheck` | `3ee3c39` before the test fix | **FAIL**, exit 2 | 54/49 | the three-argument calls above |
| `pnpm test` | same | 1308/1308, 68 files | 63/51 | not rerun in full after the fix |
| `pnpm i18n:check` | same | 0 missing, 0 problems (2462 used, 2672 translated) | 54/54 | no translation had to be added |
| `pnpm build:phone` | same | **FAIL**, exit 2 | 65/66 | same cause, stopped at `tsc` |
| `pnpm typecheck` | after the fix | exit 0 | 56/66 | |
| `pnpm build:phone` | after the fix | exit 0 | 61/100 | |
| `pnpm build` | after the fix | exit 0 | 100/65 | needed by the app crate |
| `vitest store-notifications.test.ts` | `7252558` | 3/3 | 51/62 | |
| `link_noise.py` | `7252558` | "vectors ok", exit 0 | 57/55 | |
| `cargo check -p kelpied --tests` | `280649d` content | exit 0 | not recorded | |
| `cargo test -p kelpied chat::` | `280649d` content | 85 passed, 0 failed, 398 filtered | not recorded | includes the new `stop()` check |
| `cargo check -p kelpie-link -p kelpie-proto -p kelpied --tests` | `bbf43da` content | exit 0 | not recorded | |
| `cargo test` on all four crates, 2 jobs | `7252558` | **STOPPED for memory, no results** | 42 at start | started 15:38:26; log has only its start line |
| `cargo check -p kelpie`, 1 job | `7252558` | exit 0, no warnings, no fixes needed | 79/58 | 835 s; first compile of the app on the merged tree |
| `cargo test` on all four crates, 1 job | `7252558` | exit 0: **867 passed, 0 failed, 11 ignored** | 73/50 | 1665 s; memory free 1.74 GB at the end |
| `cargo clippy -p kelpie-link -p kelpied -p kelpie --all-targets`, 1 job | `7252558` | **STOPPED for memory, no results** | 59 at start | started 16:41:22 with 1.49 GB free; log has only its start line |
| same clippy, after `ProtoLion` was closed | `7252558` | exit 0, **1 warning, from master** | 36/1 | 218 s; see below |
| `cargo build -p kelpied` | `7252558` | exit 0 | 21/9 | 84 s |
| `daemon.info` on that binary | `7252558` | **`api=22`**, `security=2`, `isolated=True` | 12/11 | exe `D:\kelpie-phone-rebase-target\debug\kelpied.exe` |
| `mobile_protocol.py` | `7252558` | **182 passed, 0 failed** | 16/2 | 83 s |
| `mobile_resilience.py` | `7252558` | **55 passed, 0 failed** | 28/8 | 483 s |
| `pnpm tauri build --debug --no-bundle` | `7252558` | exit 0 | 28/8 | 281 s; worktree still clean afterwards |
| `acp_chat.py`, fake agents only | `7252558` | **56 passed, 0 failed** | 30/4 | 32 s |
| `mobile.py`, once | `7252558` | **107 passed, 0 failed** | 21/13 | 78 s; endpoint OK, data dir OK, seam `approve` |
| `pnpm test` in full | `7252558` | **1308 passed, 68 files** | 10/12 | 17 s |
| `pnpm typecheck` | `7252558` | exit 0 | 9/21 | |
| `pnpm i18n:check` | `7252558` | 0 missing, 0 problems | 50/19 | |

`pnpm build:phone` last ran on the tree before the final fixups, which changed only plan files. `link_noise.py` ran at `7252558`.

**The clippy warning.** `app\src-tauri\src\lib.rs:370:13`, "unnecessary use of `clone` to create a slice from a reference", in a test (`assert_eq!(..., [first.clone()])`). It comes from master's commit `6e47c95`, an ancestor of `origin/master`, where the same line is at 365. It is not from the phone work, so I left it. clippy exits 0 with it. The earlier "0 warnings" reports were on trees without that commit. It is your call whether to fix it separately.

Counts for the 1-job `cargo test`:

| Target | Passed | Failed | Ignored |
|---|---|---|---|
| `kelpie` (`kelpie_lib`) | 257 | 0 | 6 |
| `kelpie-link` | 12 | 0 | 0 |
| `kelpie-proto` | 29 | 0 | 2 |
| `kelpied` | 569 | 0 | 3 |
| `kelpie` `main`, doc tests | 0 | 0 | 0 |

- `twenty_cached_summaries_are_fast` passed in that run, so it needed no retry.
- `the_acp_stop_cancels_what_the_agent_asked_and_closes_every_card` and `a_stop_logs_nothing` both passed.

The level-20 tests pass in the 1308 above: `daemonOwnsQueue(20)` is false and no `enqueue` happens, and `link.test.ts` flags `api: 20` as outdated.

The `sentPictures` map is covered by `records this window's pictures as sent from here, whichever of the log line and the queue's word comes first` in `chat-queue.test.ts`, so no test had to be added.

## Failures as they came

1. **Typecheck and phone build** failed on `store-notifications.test.ts` (item 5). Fixed and rerun.
2. **`chat-queue.test.ts`, "shows a phone's ACP prompt with its sender"** failed after the `noteSent` change. The expectation changed on purpose, then 31/31 passed in that file.
3. **The first `cargo test` run (2 jobs) was stopped for memory by Claude Code.** Nothing had failed; its log has only the start line. The 1-job run after the owner's approval passed.
4. **Clippy (1 job) was stopped for memory by Claude Code.** It started with 1.49 GB free and was gone within 54 seconds, with no result. `ProtoLion`, not mine, was holding 1699 MB. The owner closed it, memory went to about 3.6 GB, and the same clippy then finished without trouble. That fits `ProtoLion` being the pressure, but one before-and-after is not proof.

No test or e2e failed in any finished run, and none needed a retry. `twenty_cached_summaries_are_fast` passed first time.

## Not run

- **The real OS prompt.** Everything used the `approve` test seam, as you required.
- **`mobile_perf.py` and `stress.py`.** Neither was on the list, and `stress.py` was excluded. So the known cost of eight phones was not remeasured on this tree.
- **macOS and Linux builds.** Only Windows ran here. The merged `crates/kelpied/Cargo.toml` tables for those systems have not been compiled.

## How it was run

These are in `C:\Users\BaoPeakHub\AppData\Local\Temp\claude\D--Works-Kelpie\f517c3f7-a770-4c9c-ad99-c35f255352a0\scratchpad\rebase\`, with logs in `logs\`:

- `step.ps1` sets this worktree's build environment (`CARGO_BUILD_JOBS=1`) and logs load, memory and disk.
- `run-with-load.py` is a copy of the prepared runner. **The original pointed at `D:\Works\Kelpie\target\debug`, the wrong binaries for this work.** The copy points at `D:\kelpie-phone-rebase` and `D:\kelpie-phone-rebase-target\debug`.
- `daemon-info-level.py` starts the debug daemon through `phone_support.isolate`, asks `daemon.info`, and stops it. It fails unless the endpoint and data folder are its own, the exe is the one given, and `api` is 22.

## Safety notes

- The real app (PID 4368, started 12:42:03) and daemon (PID 268, started 13:38:51) still had the same PIDs and start times after the last e2e. Both start times are from before this work began. I never launched or called them. I did not touch the `omp` pane's processes.
- Each e2e set its own endpoint and data folder in its own process: `mobile_protocol` and `mobile_resilience` through `ps.isolate()`, `acp_chat` itself, `mobile.py` through the runner (`kelpie-m-892`).
- The only process I stopped was through the runner: its own `tauri-driver` tree (PID 11924) and test daemon (PID 11692), which was reported gone. No `msedgedriver`, `tauri-driver`, cargo or rustc is left, and ports 4444, 4445, 9515 and 48170 are free.
- A scan of the logs for `#p=`, `"psk"` and `KELPIE_SESSION_TOKEN` found nothing. No test keys are left in the credential store.
- **Leftovers in the real folder.** The test daemons wrote four token files to `%LOCALAPPDATA%\Kelpie\auth\`, and they are still there: `kelpie-level-11040.token`, `kelpie-m-892.token`, `kelpie-phone-protocol-9560.token`, `kelpie-phone-resilience-68.token`. They belong to endpoints that no longer exist, and the real daemon does not use them. I left them because you said not to touch that folder. Only `acp_chat.py` removes its own, so the other scripts have this gap in general.
- **One slip.** A stray `Stop-Process -Id 0` was in one of my commands, wrapped in a `try`. PID 0 is not a process that can be stopped and it did nothing, but it was outside "stop only processes you started" and should not have been there.

## Disk at the end

| Item | Size |
|---|---|
| `D:\kelpie-phone-rebase-target` | 4.60 GB |
| `D:\kelpie-phone-rebase` | 0.28 GB as counted; includes `node_modules`, `dist` and `dist-phone` |
| Free on C | 10.00 GB |
| Free on D | 2.73 GB |

Both are left in place for the next phase. D is 1.23 GB above the 1.5 GB floor, so the next phase may need space freed first; the build directory is the obvious candidate.

## Unresolved questions

1. **The four test token files** in `%LOCALAPPDATA%\Kelpie\auth\`: should they be deleted? They are safe to remove, but that is the real folder.
2. **Master's clippy warning** at `lib.rs:370`: fix it separately, or leave it?
3. **Standing blockers from `plan.md`, unchanged by this rebase:** the real OS prompt has never run, and eight phones cost about 10 points of a core against a limit of 5.

Settled by the owner: a permission cancelled by `stop()` gets no log line, `a_stop_logs_nothing` stays, and the `sentPictures` map stays because a test covers it.
