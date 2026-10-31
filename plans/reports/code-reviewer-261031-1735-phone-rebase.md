# Review of the phone rebase onto master

Reviewed `phone/link-on-master` (`7252558`) on `origin/master` (`ddf52cf`), against `phone/phase3-link` (`8ec3ef7`) and the fork point `1db592d`.

**Score: 8/10.**

**Verdict: the rebase is faithful, but do not land it on master until the owner decides about Linux.** I found nothing lost or doubled. The problems are places where the two sides were combined correctly line by line and are wrong together. Master gained Linux after the phone work forked, and the merged tree cannot pair a phone there (C1).

Nothing was built or run, as instructed. The findings come from reading the code and from read-only line audits of the git trees.

## How it was checked

A script compared the lines each side added since the fork with the final tree, as multisets, for all 145 phone files and 191 master files. It also listed lines that now occur more often than on either side.

| Check | Result |
|---|---|
| Master-only files that differ from master | Only `app/src/lib/default-settings.ts`, the one intended change |
| Phone-only files that differ from `8ec3ef7` | 5: `link.test.ts`, `chat-queue.test.ts`, `chat-wait-notice.tsx`, `attention.ts`, `store-notifications.test.ts`. All are the author's declared edits |
| Files neither side touched | None differ |
| Lines missing from the final tree | Each one is an intended rewrite: the `from` + `images` type, the sender label through `User`, the API numbers, the admin list, the `windows-sys` line, `resolve` returning who answered |
| Lines more frequent than on either side | Each one is declared new code or a common token (`#[test]`, `continue;`) |

The line counts cannot detect wrong placement. I read the conflict sites by hand for that.

## Critical

### C1. A phone can no longer be paired on Linux, a platform master now ships

- **Where:** `crates/kelpied/src/user_presence.rs:619-625`, reached from `crates/kelpied/src/daemon.rs:550-560`. Docs: `docs/user-guide.md:930-931` and the platform table at `docs/architecture.md:58`.
- **What changed:** on master, `mobile.approve` runs with no OS check (`origin/master:crates/kelpied/src/daemon.rs:1608`) and Linux is a release target (AppImage and `.deb`). The phone work forked before Linux existed. Its Linux branch of `os::verify` always returns `Err("Verifying the person is not available on this system, so phone access cannot be changed here.")`.
- **Scenario:** a Linux user with paired phones updates.
  1. Their token devices become `legacy`, "pair again", because device tokens no longer exist.
  2. They scan the QR code and type the six digits. The daemon refuses at verification.
  3. `mobile.reset` always asks too, so a store that cannot be read stays off for good on Linux.

  The result is no phone access and no way back, from a default update.
- **Why tests miss it:**
  - Windows runs used the `approve` seam.
  - Unit tests inject the decision.
  - `boot_smoke.py` never pairs.
- **Nothing warns about it:**
  - No doc says so. The guide names only Windows and Mac.
  - The phone plan folder has no mention of Linux at all.
  - The author's "No resolution needed a product decision" does not hold here.
  - The string has no Vietnamese translation and is not listed in `phone-backend-messages.test.ts`.
- **Fix (owner's decision):**
  - **Implement it:** polkit (`pkcheck`), or a PAM helper, as the Linux verifier.
  - **Accept and say so:**
    - Add a phone row to the platform table.
    - Add a sentence to the user guide.
    - Have Settings → Mobile refuse up front on Linux, with a clear message and before any QR code appears.
    - Translate the string and add it to `MESSAGES`.
  - **Keep it off:** do not let this build reach Linux users until one of the above is done.

## Warning

### W1. A picture message over about 12 MiB blocks the queue, and nobody is told

- **Where:** `crates/kelpied/src/chat.rs:1306`, `app/src/chat/chat-model.ts:825-830`.
- **The two sides disagree:**
  - Master's rule (`desktop-transport.ts:20`) is that a message refused as too large must tell the person.
  - That rule lives only on `send()`, which queued messages no longer use. They go through `chat.write_turn`.
  - `write_turn` refuses more than 16 MiB of lines, while the old route allows `MAX_FRAME_LEN`, 32 MiB.
  - A single attachment may be 25 MiB (`fs_ops.rs:363`), so the range that breaks is roughly 12 to 24 MiB of picture.
- **Scenario:** with a daemon at API 21 or later, a person sends a 15 MB PNG to Claude.
  1. The window builds a line of about 20 MiB. The daemon answers `lines must be a few strings`.
  2. `turnReady` only logs and rolls back. It does not decline, because this error is not one of the listed reasons.
  3. The entry stays `offered`. After 15 s it becomes `waiting_window`, and it is offered again every 30 s for as long as the window stays attached.
  4. Each cycle rebuilds the line, sends it over IPC, and adds then removes the pending bubble.
  5. Every message behind it waits, phones' messages included, under a chip that says "needs the desktop window". That label is wrong: the window is right there.

  On master the same picture is sent.
- **Note:** the 16 MiB limit is older phone code. It becomes a failure only next to master's sending rule. I found no earlier report of it in the reports I searched, but that search was narrow.
- **Fix:**
  - Raise the limit to just under `MAX_FRAME_LEN`, as the old route allows.
  - In the rejection branch, treat anything other than `too_late`, `busy` or `switching` as final: show the "Not sent" note through `unsentItem` and call `writeTurn(qid, null, false)` so the queue moves on.

### W2. "Not sent" notes lose the picture names, and a picture-only note is blank

- **Where:** `crates/kelpied/src/chat.rs:967-969`, `app/src/chat/chat-model.ts:543-547`.
- **What is wrong:** master's rule, which the rebase kept on the exit path, is that the note carries `withImageNames`. `kelpie_sent` carries `images`, but `kelpie_unsent` carries text only. The frontend writes that text under the same id, `unsent-<qid>`, as the live note that has the names.
- **Scenario:** a window queues a picture with no text, and a phone cancels it.
  - The live note reads `Not sent (cancelled from Pixel):\n[a.png]`.
  - The log line can arrive after the event, because the two take different routes, and replaces the note.
  - Any replay gives `Not sent (cancelled from Pixel):\n` with nothing under it.
  - `needs_window` and `declined` notes after a reload always hit this.
- **Fix:**
  - Add `"images": entry.images` to the line.
  - In `chat-model.ts:539-549`, join the names under the text, as `noteSent` does for a watcher.
  - Older log lines stay valid because the field is optional.
  - Add one test that feeds the event and then the line.

### W3. A false statement in the merged docs, and two stale API numbers in comments

- **Where:**
  - `docs/architecture.md:667`: "Level 20 skips 18 and 19, which belong to the phone work on other branches". The phone work is now 21 and 22, so 18 and 19 belong to nothing.
  - `crates/kelpied/src/daemon.rs:1737`: "`mobile.set_write` ... is the API-18 name". It should be the name before API 22.
  - `crates/kelpied/src/mobile/store.rs:7`: "`mobile.json`, API ≤ 18". Token access lasted through 21, so a maintainer reasoning about migration from a level 20 or 21 daemon is told the wrong range.
- **Impact:** comments and docs only, so behaviour does not change. The author's report says these numbers were all moved, and they were not.
- **Fix:** change the doc sentence to "18 and 19 are unused" and correct the two comments.

## Suggestion

- **S1. `noteSent` looks up pictures by `cid` alone** (`app/src/chat/chat-model.ts:905`). Queue memory is per `(device, cid)` (`queue.rs:252`) and phones see every `cid` in `chat_queue`. A phone with write permission can reuse a desktop `cid` that is still in `sentPictures`, and its message then shows the window's picture labelled as the phone's. It takes a deliberate act and exposes only what that window already shows. Fix: read the map only when `msg.by === "desktop"`.
- **S2. Commit `93c8e8e` says "Windows and macOS popups".** The default now covers Linux too. This is only worth changing if the branch is rewritten anyway.
- **S3. CI has not seen this tree.** The merged Linux and macOS code has never compiled, and that covers the Linux keyring table in `crates/kelpied/Cargo.toml`. I read `interprocess` 2.4.6: `peer_creds().pid()` exists on Unix and `peer_identity.rs` is gated correctly, so I expect it to compile, but that is reasoning, not proof. Push it, once the owner allows, before trusting it.
- **S4. Three blank lines** before `## Composer` in `docs/architecture.md`. Cosmetic.

## The seven checks

**1. Master's chat view parity: whole.**
- Pictures, `kelpie_answer` lines, saved replies and the agent map are all present.
- Files only master touched are byte-identical, including `agent-runs.ts`, `chat-images.ts`, `chat-links.ts`, `pinned-prompt.ts`, `agent-map.tsx`, `bookmarks-panel.tsx` and `image-lightbox.tsx`.
- `answer_line`, `answer_record`, `verdict_of`, `answered_key` and `without_picture_bytes` are identical to master's.
- `resetState` clears both `agentRuns` and `sentFrom`.
- The exit path keeps `stopRunning`, the streaming sweep and `withImageNames`.

**2. Phone work: whole.**
- `answer_is_one_shot`, `ask_of`, `plain_answer`, sealed revocation and `exact_answer` are unchanged.
- The admin list is exactly the union of both sides, including all five `process.*` methods, with nothing extra.
- No master code checks `Role::User` alone in a way that would shut out `Role::App`. `server.rs` uses `is_user()`, and `daemon.rs:375` and `event_visibility` name both roles.
- `mark_app` behaves as on master.
- No phone test was dropped. The one changed expectation is intended: a desktop ACP message now has `images` instead of `[name]` in its text.

**3. `stop()`: correct.**
- It resolves, writes, then logs through `answer_line`.
- A permission's `cancelled` writes no line, an elicitation's `cancel` writes one under whoever stopped, and a request already answered is out of `pending`, so there is no second line.
- `a_stop_logs_nothing` is identical to master's.
- The added assertions test the real behaviour.

**4. `noteSent` and `sentPictures`: sound, apart from S1.**
- **Memory:** capped at 50 entries holding name, mime and path, with no bytes. The cap cannot evict anything still queued, because a queue holds at most 20. It lives as long as the controller. A desktop never calls `forgetSentAnswers`, but the cap makes that harmless.
- **Replay:** `noteSent` only calls `upsert` and `onSent`, so it never reaches `write`. `turnReady` declines while replaying, and `write` drops anything during a replay. So a replay sends no pictures and no commands.
- **Doubling:** ids are `sent-<qid>`, so a replay replaces the item instead of adding one, and `qid` is per log.
- **The deviation from the plan is justified:** `applyQueue` deletes from `mine` when the message leaves, and the log line can come after that. A reload empties the map and falls back to names, which is the right result.

**5. API levels: correct in code.**

| Commit | `API_LEVEL` | `PHONE_API` | `CHAT_QUEUE_API` |
|---|---|---|---|
| `280649d`, `96357bc`, `179176f`, `e143fd1` | 21 | 21 | 21 |
| `bbf43da`, `93c8e8e`, `7252558` | 22 | 22 | 21 |

- Each commit's level matches its own features.
- Master's gates are untouched at 7 to 16.
- No comparison uses 18, 19 or 20. The only 20s are the tests that prove level 20 is treated as old.
- The phase 3 plan and `plan.md` have no old numbers left.
- The remaining stale numbers are in W3.

**6. Commit hygiene: clean.**
- All seven commits have the required author and committer.
- There are no AI lines in the messages.
- There are no phase numbers or finding codes in the messages, or in the 17,036 added non-plan lines.
- The diff from master contains nothing from `LoadBar`, `hover-hint`, `config_transfer`, `procs.rs`, `createRepeatGuard`, `worthPopup` or `notifyFinished`.
- `93c8e8e` changes only the one line in `default-settings.ts` and adds its test. The test's two-argument fix is the declared difference.

**7. Docs: coherent, apart from W3 and C1.**
- There are no duplicate headings.
- `/ws`, `/pair` and device tokens are gone from the current text.
- The queue section and master's chat-mode text agree, including the `kelpie_answer` paragraph and the `stop()` behaviour.

## Claims in the author's report that do not hold

- "No resolution needed a product decision." See C1.
- The API-number sweep is reported as complete. See W3. `store.rs` was missed partly because its comment says `API ≤ 18`.

Everything else I could check from the trees matched: the conflict lists, the union of admin lists, `Cargo.lock` (additions only, `kelpie-link` at 0.1.10) and the message rewording.

## Not checked

- Runtime behaviour. C1 and W1 are traced through the code, not observed.
- Linux and macOS compilation.
- The test results in the author's report, which I did not rerun.

## Unresolved questions

1. Linux and phone access: implement a verifier, ship with phones refused and documented, or hold back? This is the owner's call and it decides C1.
2. Should a verifier stay mandatory on Linux desktops with no polkit agent, for example a bare window manager? This only matters if the answer to question 1 is to implement one.
