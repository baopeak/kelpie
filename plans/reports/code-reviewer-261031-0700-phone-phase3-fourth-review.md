# Fourth code review: phone phase 3 (last fix round)

Branch `phone/phase3-link`, HEAD d6e5883, uncommitted working tree. Files as saved up to 04:57 on 2026-10-31.

**Method.** Read only. Nothing was built or run, so "traced" means by reading the regexes and code by hand. One read-only scan pulled 83 real `label:"..."` strings out of the installed `claude.exe` and sorted them with copies of the three regexes, from a script in the scratchpad. `plans/reports/fullstack-developer-261031-0550-*.md` was read only after the findings were written. It changed no finding. It added the developer's four perf runs to W4, and a disagreement noted in W5 and W6. Its test counts are its own; I did not repeat them.

## Score: 8/10. Verdict: do not commit yet

The fix round is real. Every Suggestion and most Warnings of the third review are properly fixed. I found no way for a phone or LAN host to gain access, and no late or failed verification that acts.

The blockers are small. Two of them work against owner decisions, and W1 was introduced by this round:
- **W1:** ordinary menus lose their Yes button whenever the question or nearby output contains a common word such as "all".
- **W2:** `answer` still answers a lasting choice if the text is not exactly the key.
- **W3:** one sniffed first message lets 4 LAN addresses take all 16 handshake places, with no trace. The docs say this cannot happen.
- **W4:** a `GATE PASS` from `mobile_perf.py` would say nothing. Its rules pass a daemon that costs about 10 points against a limit of 5. The current tree fails, and the developer reports that plainly, so nothing false has been claimed yet.

The real OS prompt has still never run on any machine. That blocks a merge, not a commit.

---

## Critical

None.

---

## Warning

### W1. Ordinary menus lose their buttons on common words (new in this round)
`crates/kelpied/src/choices.rs:70`, `:115-117`, `:188`, `:108`, `:95`; `docs/architecture.md:427`

Owner decision: "Ordinary question menus stay usable." These traces say otherwise.

`LASTING` now has `\ball\b`, `\bsessions?\b`, `\bblock\b` and `\bnever\b`. `question_lasts` tests it against all three lines above a menu, and those lines may be plain output rather than the question.

| Screen | What the phone offers | Why |
|---|---|---|
| `Should I run all the tests?` / `1. Yes` / `2. No` | `2` only | `all` in the question, so `yes_lasts` hides every consent label |
| `Fixed all three warnings.` / `Should I commit?` / `1. Yes` / `2. No` | `2` only | `all` in the output line above |
| `Ran the session cleanup.` / `Ship it?` / `1. Yes, ship it` / `2. No` | `2` only | `session` in the output line above |
| `Deleted all logs.` / `Continue? (y/n)` | `n` only | same, on the y/n path (`:234-238`) |
| `Which tests?` / `1. All` / `2. Changed only` | `2` only | the label alone matches `LASTING`, on any question |
| `Which style?` / `1. Block` / `2. Inline` | `2` only | same |
| `Session cookies or JWT?` / `1. Session cookies` / `2. JWT` | `2` only | same |
| `How should I proceed?` / `1. Run the tests first` + description / `2. Use the old API` + description / `3. Type something.` | `3` only | `proceed` plus a consent word makes it a permission question (`:108`). The escape for choices with descriptions covers only the `ACTION` path (`:111`), so this common agent form is not helped. |

The person also reads "A choice that lasts ... can only be made on the computer" (`prompt-card.tsx:56`), which is false here. The `key` and `input` routes refuse the digit too (`mobile.rs:1982`, `:2003`), so only arrow keys and Enter remain. It errs safe, but it fails the owner's decision and `architecture.md:427` ("an agent's own `Yes` / `No` question stays answerable"). "All" within three lines of a question is an everyday occurrence.

**Fix.**
- Split `LASTING`. Strong: `don't ask again`, `always`, `remember`, `from now on`, `in the future`, `for this project`, `bypass`, `auto-accept`, `forever`, `permanently`, `without asking`. Weak: `all`, `sessions?`, `block`, `never`, `rest of`.
- On a label, count a weak word only when the label starts with a consent or refusal word, or the menu is a permission question. This keeps the real labels `Yes, run hooks for cloud sessions` and `No, and block this host in the future` hidden, and frees `All`, `Block` and `Session cookies`.
- In `question_lasts`, read the strong words only, and only from the line that is the question (the one ending in `?`, as `menu()` finds it at `:265`), plus `TRUST`. `Remember this for all projects.` / `Continue? (y/n)` still hides, because `remember` is strong. Accept that a strong word in a line right above a question also hides.
- Apply the description escape to `GOES_ON` too. A tool line still makes it a permission question.
- Pin all eight rows above.

### W2. `answer` answers a lasting choice unless the text is exactly the key
`crates/kelpied/src/mobile.rs:2018`, `:1982`; `crates/kelpied/src/session.rs:1028`, `:758-763`; test comment at `mobile.rs:4785`

Owner decision: a lasting choice can never be answered from a phone "with a button or an `answer`". `choice_lasts` compares the whole trimmed text with each lasting key, and `answer_by` then types whatever it was given.

A write-permitted phone with a hand-built message (stolen, or a modified page) can send these:
- **`{"type":"answer","text":"2x"}`** on `2. Yes, and don't ask again`. Passes the check, types `2` then `x`. Single-key menus act on the `2`. `"2​"` works too, since `trim` keeps a zero-width space and `typed_text` does not filter it.
- **`{"text":"yes","enter":true}`** on `Always allow this command? (y/n)`. `lasting` is `["y"]`, and `yes` is not `y`. The same applies to `don't` on Aider's `(D)on't ask again`.
- **`{"key":"enter"}`** when the `❯` marker sits on a lasting item, for example a menu that opens focused there or that someone left there at the computer. `{"text":"","enter":true}` does the same.

The accepted residual (arrow keys through `input` and `key`) reaches the same choice for the same attacker, so this adds no new power. The defect is against the stated rule, and the test comment "refused by every way of pressing it" is untrue.

**Fix.** In `Verb::Answer`, when `prompt.lasting` is not empty:
- Accept only `esc`, or text that is exactly a key the prompt lists and that is not lasting. The phone's own page never sends anything else (`phone-state.ts:398-401`).
- Have `menu()` record which key the marker is on. Refuse `enter` and empty text when that key is lasting.
- Add `2x`, `yes` and `enter` to the test at `mobile.rs:4788`.

### W3. A sniffed first message holds every handshake place, with no trace
`crates/kelpied/src/mobile/link.rs:195`, `:325-334`; `crates/kelpied/src/mobile.rs:876-878`, `:882`, `:905-911`; `docs/architecture.md:450`, `:453`

Owner decisions: "holding slots gains nothing; flooding leaves an audit trace". This new path breaks both, because a slot marked `answered` can no longer be evicted, and any valid first message earns that mark.

**Scenario.** On plain HTTP, which is the normal browser pairing, a LAN host sees one first message from a paired phone. It keeps working until the device is removed.
1. The attacker uses 4 addresses. Each opens 4 WebSockets and replays the message.
2. `prefilter` passes and `slot.answered()` marks the place (`link.rs:195`). The message opens to a paired device, the reply goes out, and `recv_text` holds the place for 5 s (`link.rs:325`).
3. Keeping 16 places full takes 3.2 upgrades a second in total, under every per-address limit (5 upgrades a second, 5 handshakes a second, 4 places).
4. Every newcomer then gets `None` from `handshake_slot` (`mobile.rs:882`) and is closed with nothing said.

**Nothing records it:**
- Neither `None` path calls `note_flood` (`:876-878`, `:882`).
- The upgrades stay inside their bucket.
- A replay that never finishes counts no failure (`link.rs:334` just returns).

**What the person sees:** after 3 such closes the phone shows "phone access may have been reset" and retries once a minute. That invites them to tap Forget. No access is gained, and the key stays.

**Same route, budget.** `architecture.md:453` says replaying "cannot drain them for others". Four addresses at 5 a second are the whole budget of 20. Exhausting it is also unrecorded (`mobile.rs:909`).

The socket test at `mobile.rs:3588` proves the phone keeps its place. Nothing tests newcomers against a table full of answered places.

**Fix.**
- Keep a small bounded set of recently seen first messages, by hash, for 2 minutes, and drop a repeat before `slot.answered()`. A real phone's first message is unique because its ephemeral key is fresh, so this removes the replay route for both places and budget.
- Call `note_flood` on both `None` paths and when the budget runs out.
- Count a connection that passed `prefilter` and never sent a sealed frame against its address.
- Add the test, and correct `:450` and `:453`.

If the owner accepts this as a LAN denial of service with sniffing as a precondition, the two doc sentences must still change.

### W4. `mobile_perf.py` cannot pass by measuring nothing, but its PASS still hides a real cost
`scripts/e2e/mobile_perf.py:67`, `:69`, `:203`, `:245-248`, `:315-317`, `:341`, `:346`, docstring `:17-21`

The third review's complaints are fixed: the frame floor, the reader checks, the restored `quiet_id` and the pin at L0. It no longer passes having measured nothing. The result is still not honest.

**Where the current tree stands.** All seven recorded runs of it fail: the developer's four (medians +6.0 to +8.9) and the debugger's three (+10.5, +5.0, +5.0). The developer's report states that failure and does not soften it. The risk is the next run that happens to pass, and the next person who cites it.

**Using `debugger-261031-0750-phone-link-cpu.md`:**
- **It passes a daemon over the limit.** Run A1, on the pre-encryption daemon with that commit's script, printed `GATE PASS` (+4.2, +4.1, +2.7), and those rounds also pass today's rules. The same binary's cycle-counted runs cost +8.4 to +10.2 points. The current tree costs about +10. The stable estimators put the true cost at roughly twice the limit of 5.
- **Noise is larger than the limit.** Identical rounds span -8.1 to +10.7. `EXTRA_SPREAD_POINTS` is 10 against a limit of 5, so a spread of 9.9 counts as agreement. Median and first round then decide a question the data cannot answer.
- **It can fail on noise too.** The first-round rule (`:346`) fails B2 on +10.7, while the debugger found that high first rounds track low ingest and not extra cycles.
- **Its method has a measured flaw.** The docstring presents ingest-corrected CPU as the phones' cost. Over a 14 s window, CPU did not follow `seq` at this rate, and that swing alone moves the result by about 14 points. With phones, ticks also read 27.7 to 33.5 % where cycles read 34.7 to 36.6 %. Neither fact is stated in the script.
- **The advice is wrong.** "Run it again on a quiet machine" (`:341`) cannot help: host busy was 99.8 to 100 % in all 36 measured rounds, and the workload itself may keep it there. The debugger could not separate the workload from the other programs.
- **Negative costs pass.** A median of -9.9 passes (`:247`), though a negative cost is impossible and that far off shows a useless baseline.
- **The frame floor is low.** `MIN_FRAMES_PER_PHONE = 2.0` against a real 14.3 and a stated 5 per session. A run with 2 frames a second, about a seventh of the load, passes it. It also ignores `--sessions`.
- **A dead check.** The shedding check at `:252` can never fire while `KELPIE_LOAD_FORCE` pins the level. This is harmless if the pin works, but it does not prove the pin worked.

**Fix.**
- Report `QueryProcessCycleTime` or worker-thread cycles with no ingest correction, as the debugger's question 2 proposes, and keep the tick figure as information only.
- Report unstable when the spread exceeds the limit, not twice it.
- Report unstable below about -2 points.
- Set the floor to about 70 % of `SESSIONS * 5`.
- State in the docstring what the figure can and cannot show.

Until that is done, no one should cite a `GATE PASS` from this script. **The owner should treat the 5 % budget as not met, at about 10 points, most of it older than encryption** (`A` about 9.4, `B` about 10 by cycles). Reduce it, or change the limit.

### W5. Lasting choices still answerable by a button
`crates/kelpied/src/choices.rs:116`, `:128`, `:111`, `:78`, `:80`

Every row of the third review's table now traces correctly, and the scan found nothing else in Claude's real consent-led labels. For example, `Yes, set auto mode as my default permission mode` is caught by `permission` in `PERMISSION_CHOICE`, and `Continue automatically at usage limit` by `^continue`. What remains is below. All are synthetic or from older layouts, and all are cheap to close.

| Menu | Result | Why |
|---|---|---|
| `Do you trust this folder?` then 3 lines of explanation, then `1. Yes, proceed` / `2. No, exit` | `1` answerable | `question_lasts` reads only 3 lines (`:116`). From memory, older Claude Code trust dialogs had this shape; I did not confirm it. Claude 2.1.314's own says `Yes, I trust this folder`, which `trust` in the label catches. |
| `Always allow this tool?` / `1. OK` / `2. Cancel`, and `Do you trust this folder?` / `1. OK` / `2. Cancel` | `1` answerable | `ONCE` has `ok`, `okay` and `y`, and `CONSENT` does not, so `yes_lasts && CONSENT` misses them (`:188`). |
| `Allow this?` / `1. Yes` / `2. Allow` wrapped onto `for this chat` / `3. No` | `2` answerable | `item_outlasts` trusts a plain first line (`:128`). Unwrapped, the same label is hidden. It needs a wrap exactly after the once-word, so a very narrow pane. |
| `Allow this?` / `1. Yes` plus the line `Applies to every chat` / `2. No` | `1` answerable | A real description cannot be told from a term like this without a `LASTING` word. `every` and `future` alone are not in the list. |
| `May I run it?` / `1. Yes` / `2. Yes, keep running these` wrapped onto `commands here` / `3. No` | `2` answerable | One wrapped item counts as a form with descriptions, so the action no longer makes a permission question (`:111`). Unwrapped it is hidden (`:376`). Needs no tool line within 6 lines. |
| `Proceed?` / `1. Yes` / `2. Don't ask for this tool` or `Skip permission checks` or `No more prompts` | `2` answerable | `REFUSES` accepts anything after its first word (`:78`), and `LASTING` wants `again`. None of these is in Claude's label list. |

**Answer to the brief.** The descriptions rule and the wrapped case are not safe against a lasting label with no lasting word. Both rest on the shape of the text. The real labels I could check do carry a lasting word or a permission word, so the exposure is small.

The developer's report accepts the wrap risk "only under an action verb" and says a named permission "still hides it". Row 3 shows otherwise: the first-line rule in `item_outlasts` applies under a named permission too.

**Fix.**
- Treat a continuation that starts lowercase as a wrap: use the whole label for `ONCE` and for the `first_line != label` test. In a form, descriptions start with a capital, as in the tests. The rest of the description case is a residual to accept.
- Add `ok`, `okay` and `y` to the yes-lasts test by using `CONSENT || ONCE`.
- Read `TRUST` over all of `above`, not 3 lines.
- Limit `REFUSES`: the word must end the label or be followed by `,`, `—`, `(` or `.`, and `don't ask` / `don't prompt` go in `LASTING` with or without `again`.

### W6. A stuck prompt thread blocks new prompts for good, and says to close a dialog that may not exist
`crates/kelpied/src/user_presence.rs:34`, `:223-225`, `:356`, `:568-583`

`prompt_up` clears only when the thread ends. If it never ends, every prompt is refused until the daemon restarts. That includes `mobile.approve`, raising a permission, and `mobile.reset`, which is also the way out of an unreadable store. Three ways it could happen:
- `CheckAvailabilityAsync().join()` hangs (`:356`). It runs before any `on_fire`, so the time limit cannot cancel it.
- macOS `rx.recv()` has no timeout, and `_cancel` is ignored (`:568`, `:579`). A reply that never comes is the same.
- A password dialog that opened behind other windows or on a state the person cannot reach.

The person reads "close the earlier dialog first" every time, and nothing tells them what to do when there is none. Restarting the daemon also interrupts their sessions. The refusals leave no audit line.

**Bound.** The kill switch, removing and lowering all still work (decision kept). There is no sign this happens, but the real prompt has never run, so that is no evidence either. The developer's report covers a dialog nobody closes, where the instruction is true. It does not cover a thread with no dialog at all, where the instruction cannot be followed.

**Fix.**
- Add to the message: "If there is no such dialog, restart Kelpie's background service." Add the Vietnamese line and the test entry.
- Write one audit line when a prompt has been stuck past, say, 10 minutes.
- On macOS, wire `Cancel` to `LAContext::invalidate`, which should close the prompt and remove this case there. It needs the manual run to confirm.

---

## Suggestion

| # | Where | What | Fix |
|---|---|---|---|
| S1 | `daemon.rs:1724`, `mobile.rs:1211` | Lowering a permission that cannot be saved replies `true`. A restart restores the old permission from the old file. The decision names removal only, but this is the same failure, for a phone someone meant to limit. | Call `unsaved` after a lowering, with its own message. |
| S2 | `mobile.rs:1324` | A successful `enable` sets `last_error` to None while `save_pending` is still true, so changing the port removes the warning about an unsaved removal. A failed `start` overwrites it (`:1326`), and `retry_save` then leaves the other text. `architecture.md:485` says it stays "until it could be written". | Have `status` report the unsaved text from its own field while `save_pending`. |
| S3 | `mobile-settings.tsx:89`, `:129` | The local `error()` holds "Removed for now..." after the retry succeeds, until the next action. `mobile.updated` refetches but does not clear it. A false alarm, in the safe direction. | Show `s()?.error` for these messages, or clear `error()` when `save_pending` ends. |
| S4 | `daemon.rs:1404-1410` | With a hung disk, a new blocking task queues on `saving` every 10 s. That takes 85 minutes to fill the 512-thread pool and affects the whole daemon. A plain failure is fast and leaves no temp file (`persist.rs:86`). | An in-flight flag. |
| S5 | `link.rs:334`, `conn-text.ts:64`, `architecture.md:476` | Any flood that closes a phone 3 times shows "may have been reset", and the doc gives only the reset cause. | One clause in the doc, and "or something on this network is interfering" in the text. |
| S6 | `mobile.rs:77-78`, `screen-sync.ts:125`, `mobile.rs:4785` | Stale comments: "holding places gains nothing" (W3); "a whole frame on a new follow may number lower" is no longer true within a run; "every way of pressing it" (W2). | Reword. |
| S7 | `mobile.rs:1700`, `prompt-card.tsx:35` | `ONE_SHOT` reaches a Vietnamese phone in English: `o.message` is shown as is, and no vi entry has this longer text. Only a typed `2` reaches it, since the button is hidden. | Map the `one_shot` code to the card's existing translated line. |
| S8 | `architecture.md:77` | Says Hello is cancelled and a password dialog ignored, and does not mention macOS, whose prompt stays up too (`user_presence.rs:567`). | One clause. |

---

## Third-review items

| Item | State | Evidence |
|---|---|---|
| W1 lasting choices answerable | **Partly** | All nine table rows now trace right, most pinned at `choices.rs:342-356`, and `:288` is reversed (now `:335`). Holes: W2, W5. New failure in the other direction: W1 above. |
| W2 eviction starves a phone | **Partly** | `answered` is real and tested (`mobile.rs:3588`); upgrade bucket, address share and flood audit line added; evicted connections counted. The answered mark itself opens W3. A phone not yet past its first message can still be evicted, within the owner's decision: at 4 addresses and 20 upgrades a second the oldest place lasts 0.8 s, enough for a first message. |
| W3 kill switch not saved | **Mostly fixed** | `save` returns `Result`. `revoke`, `revoke_all`, `disable`, `reset` report after the effect, with the phones already cut off and the broadcast sent. 10 s retry, audit lines, tested through `dispatch` (`mobile.rs:3684`). Remaining: S1 to S4. Restart mid-retry brings the phone back, which is what the person was told, and nothing says so afterwards (the file cannot record it). |
| W4 perf gate measured nothing | **Fixed as asked; result not honest** | See W4 above. |
| W5 docs | **Fixed** | Guide `:851-853` states the residual, `:869-872` and `architecture.md:314-315` state turning access back on. True against `mobile.rs:1321`. |
| W6 ignored prompts and stacked dialogs | **Fixed** | `TIMEOUT_WINDOW` 1200 s, three at 280 s intervals pause (test `user_presence.rs:649`). `CLOSE_THE_EARLIER` before any new prompt, tested at `:637`. `begin` holds the turn with no gap. Residual: W6 above. |
| S1 consent-led forms | **Partly** | `Yes, but ask again next time` is `ONCE`; the first-line rule and the `ACTION` escape work. The `GOES_ON` path does not: W1. |
| S2 `inSession` always false | **Fixed** | `link.ts:617` reads it before `detach`. |
| S3 sticky "reset" | **Fixed** | `findOutWhy` at `:639`, cleared at `:636-643` once unreachable; test `link.test.ts:363`. |
| S4 `start` keeps `noReplyCloses` | **Fixed** | `link.ts:337`. |
| S5 stale prompt on resume | **Fixed** | `screen-sync.ts:99`. |
| S6 frame numbers reused | **Fixed** | `Views::last_rev`, `sync.rs:205`, `:415`. See below. |
| S7 eviction by number | **Fixed** | Insertion order at `screen-sync.ts:126`, mirrored at `phone_support.py:346-347`. |
| S8 refusal order | **Fixed** | `daemon.rs:555` before `:560`. |
| S9 `quoted` | **Fixed** | `user_presence.rs:339`, test `:735`. |
| S10 English messages | **Fixed** | `backend.json:129-141`. `phone-backend-messages.test.ts` checks the source still says each message, and that the translation exists. `Windows could not verify you` works by substring with no test row, which is fine. |
| S11 module comment | **Fixed** | `link.rs:7-10`. |

---

## Checked and sound

- **`Views::last_rev` across a restart.** Slots live in memory, so after a restart `slot.acked` is None. Any `have_rev`, however high, gets a whole frame (`sync.rs:237`). The page also resets on `daemon_epoch` (a fresh uuid, `mobile.rs:451`) before it subscribes (`link.ts:793`, `:801`, `phone-state.ts:105`). A stale screen cannot be taken for current, even by a client that skips the reset. Within a run, a window holding an old rev no longer matches `acked` and gets a whole frame. `u64` numbers stay in JS's safe range. `ack` matches only `flying`, on the owner connection. An ack dropped offline just makes the next `have_rev` miss, which is safe.
- **Slot accounting.** `notify_one` keeps its permit. Eviction removes the entry and its address count once, and `Drop` then finds nothing. An eviction racing `answered` is a no-op, leaving one connection running with no place for at most a moment. No slot is held past the first sealed frame.
- **Table growth.** All per-address tables are capped at 1024 (`take_token`, `pairing_failed`, `code.attempts`). `by_address` is bounded by the 16 places. **IPv6 ranges are not a route:** `listen::usable` allows only Tailscale's `fd7a:115c:a1e0::/48`, one address per node, and everything else is private IPv4. Filling a table clears it, which only helps a host that already has 1024 addresses. Worth a re-check if a plain IPv6 LAN address is ever allowed.
- **Shared NAT.** Honest phones do not lock each other out: 5 phones waking together fit 20 upgrades, and a fifth over 4 places is closed and retried on backoff. Only someone hostile behind the same address can lock it out, for 60 s after the attempts stop.
- **429 before upgrade.** A browser sees 1006 with `onopen` never fired, so `sentFirst` stays false. `noReplyCloses` does not move, `failed()` backs off, and the state reads "reconnecting". `same_origin` runs first, so a page on another site spends no token. 4429 resets the count and backs off.
- **`save`.** Lock order unchanged. `broken` returns Ok without touching `save_failed`, and the kill switch is hidden while `store_error` is set (`mobile-settings.tsx:220`). A token holder calling it then gets `true` while the file is untouched, but memory had no devices and the file is flagged. Retry failures log at debug, so no log spam. `keep` warns only on real changes. A failed write removes its temp file. `audit.add` only queues to its own thread, so `note_flood` under the table lock does no I/O.
- **`link.ts`.** No forget without a sealed message. The reset branch retries at 60 s. `kick` limit unchanged.
- **`phone_support.py`.** Matches `screen-sync.ts` on base, eviction and bound, except that it records `lost` where the page refollows, which suits a test client.
- **Docs.** The rest of the phone sections match the code: the 280 s limit and the pauses, the upgrade and handshake numbers, the 10 s retry, removal and sealed revocation, the guide's prompt paragraph and "Not paired here". Exceptions are W1, W3, S2, S5 and S8.
- **Probes and seams.** No `QueryThreadCycleTime` or debug counter in `crates/`, and no `eprintln!` or `dbg!` in the phone files. `GetThreadTimes` in `load/windows.rs` is the older watchdog and is unchanged. `seal_bench` source is gone. `loopback_is_remote` is `#[cfg(test)]` at all three places. `KELPIE_TEST_USER_PRESENCE` and `KELPIE_TRUSTED_APP` are `debug_assertions` only. `Presence::inject`, `set_limit` and `fake` are `#[cfg(test)]`. `KELPIE_LOAD_FORCE` is honoured in release, as it was before this phase.

---

## Unresolved questions

1. W3: fix the replay route, or accept it as a LAN denial of service that needs sniffing? The docs change either way.
2. W4: the 5 % budget is about 10 points by the stable measures, mostly older than encryption. Reduce, change the limit, or change how it is measured? This is the debugger's open question 2.
3. W1: should a consent-led option under "How should I proceed?" with no tool line stay answerable? I recommend yes when it has a description. Without one, the owner may prefer to keep it hidden.
4. W6: can any of the stuck-thread cases happen? Only the manual run of the real prompt can say. That run also still owes the questions on Hello's face pass and the parented CredUI window.
