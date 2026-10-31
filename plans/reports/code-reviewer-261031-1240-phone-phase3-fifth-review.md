# Fifth code review: phone phase 3 (last fix round)

Branch `phone/phase3-link`, HEAD d6e5883, uncommitted working tree. `choices.rs` saved 07:41, `mobile.rs` 07:29, `link.rs`, `architecture.md` and `mobile_perf.py` 07:25, all on 2026-10-31.

**Method.** Read only. Nothing was built or run, so "traced" means by reading the regexes and code by hand. Test results are the separate tester's. `plans/reports/fullstack-developer-261031-0720-*.md` was opened only after the findings below were written. It changed no finding. It added the alternative in W1, since it records `Disable all LSP recommendations` as a deliberate choice, and its printed sample (`42792752748 / 8.01 / 2592 MHz x 100 = 206.23`) agrees with the formula by hand, on a debug daemon. Its list of "false hides the table accepts" does not include the W1 rows. It also states that `mobile_perf` was not run on release and that S3 and S7 have no automated test. I did not repeat its test counts.

## Score: 9/10. Verdict: do not commit yet, for one small fix

Every Warning and Suggestion of the fourth review is fixed, apart from one part of S7. I found no way for a phone or LAN host to gain access. The replay set, the failure counts, `plain_answer`, the saving code and the cycle conversion all hold up.

The one blocker is W1. It is the fourth review's W1 again in a narrower form, partly because of the word lists that review proposed. It errs safe and takes two regex edits and a few table rows. It does not need another review round.

The real OS prompt has still never run on any machine, and `mobile_perf` has not been run on a release daemon built from this tree. Both block a merge, not a commit.

---

## Critical

None.

---

## Warning

### W1. Ordinary menus still lose their buttons on everyday wording
`crates/kelpied/src/choices.rs:93`, `:97`, `:154`, `:173`, `:243`; `crates/kelpied/src/mobile.rs:2062`; `app/phone/src/prompt-card.tsx:58`

Owner decision: recognise real labels, keep ordinary menus usable. This also answers check 2: `plain_answer` itself is right, but `lasting` is wrong on these menus, which contain no lasting choice, so a normal answer is refused.

| Screen | Phone offers | Why |
|---|---|---|
| `Should I set up CI for this project?` / `1. Yes` / `2. No` | `2` only | `for this project` is in `STRONG`, so `question_lasts` is true (`:154`) and `Yes` is hidden (`:243`) |
| `Updated the README for this project.` / `Should I commit?` / `1. Yes` / `2. No` | `2` only | same, from the line right over the question |
| `Add a config for this repo? (y/n)` | `n` only | same, on the y/n path (`:295`) |
| `Which test runner?` / `1. Vitest` + `Fits the setup for this project` / `2. Jest` + `Already in the repo` | `2` only | `STRONG` reads the description (`:173`) |
| `Apply to the other files?` / `1. Yes, all of them` / `2. Just this one` / `3. No` | `2`, `3` | `all` in `WEAK`, and `Yes` opens the label (`:173`) |
| `Which tests should I run?` / `1. Run all tests` + `Takes five minutes` / `2. Run the changed ones` + `Faster` | `2` only | same, `Run` opens the label |

`for this project`, `for the repo` and `Yes, all of them` are normal phrases for a coding agent, so this will happen often. Tapping is refused, and so are the `key` and `input` routes for that digit (`mobile.rs:2025`, `:2046`). Only Enter or the arrow keys remain, and the card says "can only be made on the computer", which is false here. The guide's "an ordinary option may lose one" covers a rare miss, not these.

No safety is lost by either fix below. On a permission question `outlasts` still hides anything that is not plainly once or no.

**Fix: two edits, fewer rules rather than more.**
- Move the `for (this|the) (project|repo|repository|folder|directory|workspace)` alternative from `STRONG` to `WEAK`. `Yes, for this project` stays hidden, because it opens with consent. A question or a description no longer counts.
- Replace `\ball\b` in `WEAK` with `\bfuture\b`. `Use this and all future MCP servers in this project` stays hidden. On a permission question `Allow all edits` stays hidden by the not-plainly-once rule.
- One real row moves: `Disable all LSP recommendations` becomes `false, true`, so it gets a button on an ordinary question. That is a Claude Code suggestion setting and not a grant of permission. I would accept it, but it is the owner's call, and the developer chose to keep it hidden. To keep it hidden, count `all` only on a label that opens with `REFUSAL_WORD`. No row then moves, and `Yes, all of them` and `Run all tests` are still freed, at the cost of one more condition.
- One thing changes on a permission question: `Allow this for this project?` / `1. Yes` / `2. No` leaves `Yes` answerable. The label scan covered labels, not question text, so I cannot say whether a shipping agent asks this. If that worries you, have `question_lasts` also read the scope words on a line that matches `NAMES_PERMISSION`. That is one clause.
- Add the six rows above to `ordinary_menus_keep_their_buttons_on_common_words`.

---

## Suggestion

| # | Where | What | Fix |
|---|---|---|---|
| S1 | `scripts/e2e/mobile_perf.py:115-119`, `:76` | Three ways it can still pass having measured nothing, none of them likely. `cycles()` ignores the return values, so a failed call reads 0 in every round: spread 0, cost +0.00, `GATE PASS`. The frame floor does not catch it, because frames are counted separately. `--rounds 1` has a spread of 0 by definition. `--sessions 0` makes the floor 0. On its own child process the call should not fail, and the printed conversion would show 0. | Raise on a failed call or zero cycles. Treat fewer than 2 rounds or 0 sessions as a usage error. |
| S2 | `mobile_perf.py:268-271`, `:369` | Rounds of, say, +6 to +12 report `UNSTABLE` and "neither met nor missed", though every round is over the limit. Every round is printed, so nothing is hidden, but that sentence would be false. The brief asks for an honest result. | `min(extra) > LIMIT_POINTS` is a `GATE FAIL` whatever the spread. |
| S3 | `crates/kelpied/src/mobile.rs:625`, `:662`, `:708` | `unsaved_text` is one slot, and only `retry_save` clears it. (a) A lowering that also fails replaces "Removed for now": the person then reads that one phone keeps its old permission, not that another comes back. They were told at the time. (b) If another `save` succeeds first, the text stays. A later routine failure then shows "Removed for now" for a removal already on disk, until the next retry, at most 10 s later. | Clear the text in `save` on success. For (a), do not let `NOT_SAVED_LOWERED` replace `NOT_SAVED_REMOVED` or `NOT_SAVED_OFF`. |
| S4 | `app/phone/src/key-bar.tsx:33`, `composer.tsx:76`, `:108` | The rest of the fourth review's S7. The card now translates `one_shot`, but the key bar and the reply box show `ONE_SHOT` in English on a Vietnamese phone, and those are the routes a typed `2` takes. `composer.tsx:108` says "Offline: the key was not sent." for any refusal of a Ctrl key. | Map `one_shot` to `lastingText` in `sendText` and `sendKey` (`phone-state.ts:383`, `:391`). Use the key bar's three-way message at `:108`. |
| S5 | `docs/architecture.md:470`, `:423` | `:470` still lists only "mac-valid failures and connections that stayed silent until their time was up"; `:453` has the longer, correct list. `:423` does not state the rule for `answer` while something lasts: only `esc`, a listed non-lasting key, or Enter on a marker known to be on one. | Point `:470` at `:453`. Add one clause to `:423`. |

---

## Fourth-review items

| Item | State | Evidence |
|---|---|---|
| W1 buttons lost on common words | **Fixed as listed; class remains** | All eight rows traced and pinned at `choices.rs:421-429`. `STRONG` and `WEAK` are split (`:92-97`), `question_lasts` reads the question and the line over it (`:149-155`), and the description escape covers `GOES_ON` (`:141-144`). New rows: W1 above. |
| W2 `answer` on a lasting prompt | **Fixed** | `plain_answer` (`choices.rs:34-43`), called at `mobile.rs:2062`. It reads the same `key` and `text` fields as `Answer` at `:2056`, so the two cannot disagree. `2x`, `2` + zero-width space, `yes`, `enter`, empty + enter are all refused at `mobile.rs:4943`, with nothing typed. Marker recorded in `selected` (`:329`), not sent to phones (`#[serde(skip)]`). |
| W3 sniffed message holds every place | **Fixed** | `first_message_seen` runs before `slot.answered()` (`link.rs:196-200`). `note_flood` is on both `None` paths (`mobile.rs:914`, `:921`), the budget (`:952`), and a repeat. Never sealing counts (`link.rs:258`, `:339`). Tests at `mobile.rs:3688`, `:3725`, `:3744`. `architecture.md:450-457` corrected. |
| W4 perf gate's PASS | **Fixed as asked; no run yet** | Cycles with no intake correction are the only gate figure (`mobile_perf.py:230`, `:344`). Ticks are information only (`:341`, `:349`). Unstable at a spread over 5 and a median below -2 (`:268-273`). Floor is 70 % of `SESSIONS * 5` (`:76`). Docstring and advice corrected. Details and residuals: below, and S1, S2. |
| W5 lasting choices with a button | **Fixed** | Traced each row: trust over all of `above` (`:150`), `ONCE` in the yes test (`:243`), lowercase wrap (`:188`), `REFUSES` bounded (`:107`). Pinned at `:444-455`. The `every chat` row is a stated residual, pinned at `:461`. |
| W6 stuck prompt | **Fixed** | Message extended (`user_presence.rs:34`), with the Vietnamese line and the test entry. `verify_stuck` audit line at `daemon.rs:1414`, once per prompt. macOS `invalidate` is wired in (`user_presence.rs:603-606`), which needs the manual run to confirm. `rx.recv()` has no timeout, which is now only a fallback, and the message covers it. |
| S1 lowering not saved | **Fixed** | `daemon.rs:1730`, tested at `mobile.rs:3840`. See S3 for the shared slot. |
| S2 warning lost on enable | **Fixed** | `status` reads `unsaved_text` first (`mobile.rs:708`), tested at `:3827-3830`. |
| S3 false alarm in settings | **Fixed** | `shownError` defers to the daemon's status (`mobile-settings.tsx:122`). `errText` does not translate, so the English `includes` works. |
| S4 retries queue on a hung disk | **Fixed** | `claim_retry` and `retry_ended` (`mobile.rs:631-637`, `daemon.rs:1404-1408`). `retry_save` has no panic path. |
| S5 "may have been reset" | **Fixed** | `conn-text.ts:64`, `architecture.md:480`. |
| S6 stale comments | **Fixed** | `mobile.rs:77-79`, `:4934`. None of the old phrases remain. |
| S7 `ONE_SHOT` in English | **Partly** | The card maps the code (`prompt-card.tsx:37`). Key bar and reply box do not: S4 above. |
| S8 macOS in `architecture.md:77` | **Fixed** | States that macOS's prompt is cancelled, which matches `invalidate`. |

---

## Checks asked for

**`plain_answer` and the page (check 2).** The page sends only `prompt.keys` minus `lasting` (`prompt-card.tsx:50`), verbatim as `text`, with `enter` from `prompt.line` (`attention.ts:48`, `phone-state.ts:400`). It never sends `enter` or `esc` from a button. So every button passes `plain_answer`: on a plain menu `lasting` is empty and anything goes, and otherwise it is an exact listed non-lasting key. Letters match too: `keys` and `lasting` are built the same way, and `a` / `A` stay apart. A stale tap on a new menu may read `one_shot` where it would have read `moved`, which is safe. `selected` is part of `Prompt`'s equality (`session.rs:1055`), but a marker that moves needs input, and input already starts a new wait, so waits behave as before.

**Replay set (check 3).**
- **Honest reconnect, retried pairing, two windows:** none can repeat a message. `linkCodec.open` (`link.ts:178`) and `pair.ts:78` make a new `Initiator` per socket, and each draws a new ephemeral key (`noise.ts:201`). `mac1` covers the version, mode and whole message (`kelpie-link/src/lib.rs:194-195`), so nobody without the key can vary a captured message to get past the set.
- **Bound and lock:** at most 256 entries, added only after `prefilter`. The lock is held for one scan with no await or nested lock. Entries are time-ordered, so popping the front is right. Filling it takes 256 mac-valid messages in 2 minutes, which needs the desktop's public key.
- **After 2 minutes** an old message earns one place for 5 s again. The docs say "in the last 2 minutes", so that is true. Holding all 16 places that way takes 384 distinct sniffed messages every 2 minutes, each a failure for its address.
- **Failure counts on honest use:** reachable, not a lockout in practice. A phone that loses the network mid-handshake counts once per attempt. Reaching 10 in a minute needs a failure every 6 s, and each attempt waits out its timeout and the backoff (`conn-state.ts:124`). So only a network where handshakes are not completing anyway gets close. It then self-heals as entries age out, with a false "too many failed connections" notice as the only cost. A phone let go under a flood can be counted too, which falls within the accepted LAN denial of service. `4429` backs off and does not show "not paired" (`link.ts:634`).

**Saving (check 4).** `save` stores its result each time and stays in order. `broken` is set only at load and cleared only by `reset` (`mobile.rs:1321`), so its early `Ok` cannot leave a retry loop reporting "saved". `unsaved` reads the flag after the write, and a later save that succeeds has written the removal too, so no false "fine" comes from a race. `newly_stuck` fires once per prompt, with a test at `user_presence.rs:685-693`. The status fields match the docs. Remaining: S3.

**`mobile_perf.py` (check 5).**
- **Formula:** `cycles / seconds / Hz * 100` is correct on a multi-core machine. `QueryProcessCycleTime` sums all the process's threads, user and kernel, including threads that have ended, and not child processes (ConPTY, the printers). The result is points of one core, which can exceed 100, the same unit as the old tick figure and as the docstring says.
- **The phase criterion in those units:** "≤ 5 % CPU" (`phase-03-kelpie-link-e2e.md:86`) does not say whether that means one core or the whole machine, and it speaks of crypto overhead. Whichever the owner decides, the script's 5 points is the stricter reading.
- **The divisor:** the `~MHz` registry value is fair. On invariant-counter CPUs the cycle count runs at about that rate whatever the current clock, so a turbo does not inflate it. Any error is proportional, about 1-2 % of a 10-point difference, or 0.2 points. Microsoft warns against converting these counts to time on CPUs whose counter varies. The docstring states the assumption, and no CPU this script is likely to meet breaks it. On a busy SMT host a thread takes more scheduled time for the same work, which raises both arms; the docstring says so.
- **Verdict logic:** a dead phone and frames for an unfollowed session fail first. Then unstable, exit 2. Then pass only if both the median and the first round are within 5. The level check (`:277`) can now fire, since `level` is a number from `mobile.load`.
- **The result as stated:** the docstring says what the figure is and is not, and that sealing was about 0.4 points by a separate measurement. It does not claim to answer the phase's crypto criterion.

**Docs (check 6).** The phone sections match the code: the first-message costs, 256 by hash, the failure list at `:453`, the flood line, 4429 and 4503, the 10 s retry, `verify_stuck`, the restart advice, and the guide's prompt and "Not paired here" paragraphs. Exceptions are in S5. The guide's "an ordinary option lose one" is true, but W1 makes it more common than it reads.

**Probes and seams (check 7).** No `eprintln!` or `dbg!` in the phone files, and no `QueryThreadCycleTime`, `QueryProcessCycleTime` or `seal_bench` anywhere in `crates/`. `loopback_is_remote` is `#[cfg(test)]` at all three places (`mobile.rs:384`, `:489`, `:900`). `KELPIE_TEST_USER_PRESENCE` and `KELPIE_TRUSTED_APP` are `debug_assertions` only. `Presence::inject`, `set_limit`, `forget_refusals` and `newly_stuck_at`'s callers in tests are test-only or harmless. `KELPIE_LOAD_FORCE` is honoured in release, as before this phase. `scripts/perf/baseline.json` is unchanged.

---

## Unresolved questions

1. W1: is a button on `Disable all LSP recommendations` on an ordinary question acceptable? I recommend yes.
2. W1: should `Allow this for this project?` keep `Yes` hidden, at the cost of one clause? I have no evidence of a shipping agent asking it.
3. The 5-point budget stays the owner's open decision. The phase's criterion, crypto overhead against the pre-encryption daemon, is a different number, and nothing in the tree records it as met.
4. Still owed to the manual run: Hello's face pass, the parented CredUI window, and whether macOS's `invalidate` ends the wait.
