# Third code review: phone phase 3 (kelpie-link, gate in the daemon, fix round 2)

Branch `phone/phase3-link`, HEAD d6e5883, uncommitted working tree. Files as saved 01:05 to 01:27 on 2026-10-31.

**Method.** Read only, plus `git diff` and read-only string scans of the installed `claude.exe` (2.1.314) and `codex.exe`, to build menus from their real labels. Nothing was built or run, so "verified" means proven by reading. Regex results were traced by hand. `plans/reports/fullstack-developer-261031-0100-*.md` was read only after the findings were written. It changed one suggested fix in W1, added the shedding point in W4 and settled the prompt row of the table. Its test counts are its own; I did not repeat them.

## Score: 8/10. Verdict: do not commit yet

The fixes to the second review are mostly real. I found no way for a phone or a LAN host to gain access, and no way for a late or failed verification to run an action. There are no Critical findings.

Fix before committing:
- **W1:** the owner's "a choice that lasts" rule still has holes, one of them on a real Codex label.
- **W3:** the kill switch can report success and not survive a restart.
- **W4:** the perf gate can pass having measured nothing.
- **W5:** two doc statements are wrong or missing.

Decide:
- **W2:** the new eviction lets a LAN attacker starve a phone with no trace. Fix it, or accept it in writing.

The real OS prompt has still never run on any machine (the developer's report confirms it). That blocks a merge, not a commit. The 5 % perf budget has also not been shown to be met: the developer's two runs on a release daemon gave GATE FAIL (+5.3) and GATE UNSTABLE (+8.1).

---

## Critical

None.

---

## Warning

### W1. Lasting choices are still answerable from a phone
`crates/kelpied/src/choices.rs:70`, `:74`, `:78`, `:90-93`, `:97-100`, `:142-149`, `:186-192`, `:288`; `crates/kelpied/src/session.rs:1028`

**Answer to the primary question: yes.** Each case below was traced through the regexes. The first two use labels found in the installed binaries.

| Menu | What happens | Why |
|---|---|---|
| Codex: `No, and block this host in the future` | Key answerable. A phone writes a lasting deny rule. | `in the future` is not in `LASTING`, and `REFUSES` matches `No`, so `outlasts` is false even on a permission question. The comment at `:76-77` ("changes nothing that lasts") is false for this label. |
| Claude 2.1.314: `Don't ask me again` (resume dialog), `Don't show again` (auto-mode onboarding) | Key answerable. The preference is saved. | `LASTING` has only `don't ask again`, and `REFUSES` takes `don't`. |
| `Always allow this tool?` then `1. Yes`, `2. No` | Key 1 answerable. | The menu branch never reads the question. The y/n fix at `:186-192` has no menu twin. |
| `Do you trust the files in this folder?` then `1. Yes, proceed`, `2. No, exit` | Key 1 answerable, and **a test pins that** (`:288`). | Folder trust is saved, and this is the opposite of the owner's rule. Claude 2.1.314 now uses `hideIndexes` on this dialog, so it is not detected there. Older Claude Code versions and any agent with a numbered trust menu still hit it. |
| `Allow this tool?` then `1. Enable`, `2. Enable for this chat`, `3. Decline` | Key 2 answerable. | `asks_permission` needs a `CONSENT` word among the labels, even when the question itself asks for permission. `enable`, `grant`, `use` and `turn on` are not `CONSENT` words. |
| `Allow (this chat)`, `Yes (until you restart)` on a permission question | Answerable. | `ONCE` accepts any parenthetical (`\([^)]*\)`). It was meant for `(esc)` and `(y)`. |
| `Remember this for all projects.` then `Continue? (y/n)` | `y` answerable. | `:187-188` reads the previous line only when the `(y/n)` stands alone. |
| Non-permission menu: `Yes, run hooks for cloud sessions`, `Yes, use my enabled plugins in cloud sessions` (real labels) | Answerable unless the question has a permission word. | `\bsession\b` does not match `sessions`. |
| Numbered menu with a letter on each item: `2. Yes, and don't ask again (a)` | `2` refused, `a` is not: `answer {text:"a"}`, or `input` with `force`. | The menu branch returns digits only. The label shape is what I recall of Codex's menu and is not confirmed on this version; the gap in `choices.rs` is verified. |

**Impact.** The accepted residual already lets a phone reach any choice with arrow keys and Enter, so a stolen phone gains nothing new. The cost is to the honest person and to the promise: the card offers a button for something that lasts, and the docs say that cannot happen.

**Fix.**
- Add to `LASTING`: `\bsessions?\b`, `\bin the future\b`, `\bblock\b`, `\bnever\b` (this also hides `Never mind`, which is safe), `don['’]?t (?:ask|show|prompt|remind)( me)?( .*)? again`.
- Test `LASTING`, plus a word for trust, against the three lines above a menu, as the y/n path does for its question. Apply it to the "yes" keys.
- Let a question that names permission (`allow`, `permission`, `permit`, `grant`, `trust`) stand on its own, and add `enable`, `grant`, `use`, `turn on` to `CONSENT`. Action verbs (`edit`, `run`, `write`) should still need a consent word: the developer's report is right that `Which file should I edit?` with `1. a.rs` / `2. b.rs` must stay answerable.
- Limit `ONCE`'s parenthetical to hints: a single letter, `esc`, `enter`, `shift+tab`.
- Read the last `\(([a-z])\)` of a lasting item's label into `lasting`.
- Reverse the assertion at `:288`.
- Keep `No, and ask again next time` answerable, and pin it.

**Ordinary menus (second question).** These work: Codex `Yes, just this once (y)`, `No, continue without running it`, `No, and tell Codex what to do differently (esc)`; Claude `Yes`, `Yes, run it`, `Yes, continue`. See S1 for what is over-hidden.

**ACP and chat answers are sound.** `answer_is_one_shot` (`chat.rs:308-337`) only lets through `allow_once` and `reject_once` option ids, and I could not break it.

### W2. Eviction lets a LAN host starve a phone mid-handshake, with no trace
`crates/kelpied/src/mobile.rs:745-767` (`:754-757`), `crates/kelpied/src/mobile/link.rs:168`, `:246`, `:325`, `:296`, `crates/kelpied/src/mobile.rs:770-780`

**Answer to the brief's question: yes.** The code evicts the *oldest*, not the oldest idle. A phone that has sent a valid first message and is waiting on its own crypto goes before any newer silent socket.

**Scenario.**
1. A rogue host with 4 addresses (aliases on a LAN) keeps 15 places taken.
2. The phone connects, and its slot is 16th.
3. The attacker opens 16 more WebSockets. Eviction frees places as fast as it takes them, so its per-address count never blocks it.
4. A phone needs about 50 to 150 ms from upgrade to first sealed frame, which comes to roughly 100 to 300 upgrades a second. That is cheap on a LAN, and there is no limit on upgrades.
5. The phone is dropped with 1006 (`link.rs:246`, `:325`).

Nothing records it. `given_way` returns with no failure counted, the 5 s silence rule never fires, and there is no audit line and no `mobile.lockout`. After 3 such closes the phone shows "phone access may have been reset" and retries only once a minute, so the attacker then only has to be active at those retries. It also invites the person to tap Forget. The key itself stays, as decision 2 requires.

**A cheaper route.** On plain HTTP a first message can be sniffed. Replaying one passes `mac1` and takes one of the 20 Diffie-Hellman handshakes a second, with no share per address (`link.rs:296`). One address can drain the budget, and real phones then get a silent drop.

The socket tests cover holding places and late garbage. None evicts a phone mid-handshake.

**Fix.**
- Take a slot out of the evictable set once its first message passes `prefilter`.
- Count an eviction that comes before any first message against its address.
- Add a per-address token bucket on upgrades in `link_socket`, before `on_upgrade`, and a per-address share of the handshake budget.
- Add a socket test: phone mid-handshake, then 16 newcomers.
- If the owner accepts this (a LAN attacker has other denial-of-service routes), say so in the docs. `architecture.md:443` currently says "holding places gains nothing", which is true only of holding.

The developer's report notes this flood and says it "only delays", within "a few milliseconds". I disagree on both counts: the phone's handshake takes far longer than that, and a sustained flood is a denial of service for as long as it runs, with nothing recorded.

### W3. The kill switch can report success and not survive a restart
`crates/kelpied/src/mobile.rs:515-526`, `:1040-1053`, `:1097-1116`, `crates/kelpied/src/daemon.rs:1723-1735`

`save` only logs a failure, and `revoke_all`, `revoke` and `disable` reply `true` either way.

**Scenario.** A phone is stolen and offline. The person hits *Turn off phone access and revoke all* while the store cannot be written. Disk full is the likely cause; this machine's C: was at 100 % on 2026-10-23, and a sharing violation on the rename would do the same. The UI confirms. At the next daemon start `mobile.v2.json` still says `enabled: true` with the device's valid record, `autostart` listens again, and the phone is back in. Phones that were online forgot their key from the sealed message, so only the absent one, the dangerous one, returns.

**Fix.** Make `save` return `Result`. On failure, report it from `revoke_all`, `revoke`, `disable` and `reset` ("removed for now, but could not be saved: it will come back after a restart unless this is fixed"), set `last_error`, and retry from the sweeper while a dirty flag is set.

### W4. `mobile_perf.py` can print GATE PASS having measured nothing
`scripts/e2e/mobile_perf.py:124-149`, `:209`, `:293-305`, `:62-63`

- **No floor on frames.** `_read` catches only `ConnectionClosed`. Any other exception (a decrypt error, `ValueError` from `raw.index`) ends the task silently, and acks stop. The daemon then sends 4 frames per session and blocks, so its CPU matches the no-phones runs, `extra` is about 0 and the gate passes. The same happens if `sub` is refused, because `follow` never reads a reply. `frames_per_phone_per_s` is printed and never checked.
- **Blind quiet check.** `flat_out` overwrites `p.quiet_id` with the ghost's id and never restores it (`:209`). From round 2 on, the steady runs count frames of a closed session, so the "0 frames for it" gate does not look at the quiet session there. This predates the phase.
- **Noise as large as the limit.** `EXTRA_SPREAD_POINTS` is 10 against a limit of 5, and negative `extra` always passes. Three runs at -9, 0, +1 are not "unstable" and pass, though a baseline that far off says nothing.
- **Shedding is not checked.** Under load the daemon sends fewer frames, so a busy machine lowers the very cost being measured. The developer's runs were at levels 1 to 2. The script prints the level and never tests it, so a pass at L2 means little. The developer asks whether to pin the run calm with `KELPIE_LOAD_FORCE`; pinning would measure the full work, so I would do that or report unstable when the level is above 0.

`--pair-with` itself is sound: a release `exe` without it raises instead of passing, and pairing refuses a non-debug daemon. No check confirms the measured daemon is a release build, but a debug build could only fail.

**Fix.**
- Fail or report unstable when any with-phones run is below a small frame rate, or when any `p.reader.done()`.
- Restore `quiet_id` in `flat_out`.
- Report unstable when `median(extra)` is below minus the spread limit, or when the spread alone could hide the limit.

### W5. Docs: one false promise, one owner decision not stated
- `docs/user-guide.md:850-851`: "a choice that lasts ... can only be made on the computer". With *write*, arrow keys and Enter, `shift-tab` (one key: Claude Code's permission mode) and `/permissions` all work from the phone, as `architecture.md:422-425` says. The guide states it as a guarantee to the person deciding whether to give a phone *write*. Say it plainly, for example: "the phone offers no button for a choice that lasts; a device allowed to type can still reach one with the arrow keys, Shift+Tab or a command, so allow typing only on phones you would trust at your keyboard."
- `docs/user-guide.md:864-866` and `docs/architecture.md`: decision "turning access back on stays open to the token holder, audited" appears nowhere. `mobile.enable` is not mentioned in either file. After the person turns access off, any program with the token can turn it back on, and every paired phone regains access with no prompt. The person needs to know that turning off is not how to cut off a phone. Add it, and point to *Remove* and *revoke all*.

The rest of the phone docs check out against the code. That covers `after_verifying`, the 280 s limit, the refusal pause, the file route and `dormant_days` (`architecture.md:102-111`), the phone link section, the sealed revocation, and the guide's "Not paired here" and same-user paragraphs.

### W6. Ignored prompts never pause, and abandoned password dialogs pile up
`crates/kelpied/src/user_presence.rs:31`, `:34`, `:201-211`, `:247-250`, `crates/kelpied/src/daemon.rs:550-558`

A timeout counts as a refusal, but with a 280 s limit and a 300 s window three can never fall in one window: at 840 s only the ones at 560 s and 840 s are still in it. The brief's "after three refusals" therefore never fires for a person who is away.

**Scenario, on a machine with no Hello.** A same-user program holding the token that defeats `Role::App` calls `mobile.approve` or `mobile.reset` in a loop. The password dialog cannot be cancelled (`:337`), so each timeout leaves a dialog on screen and a blocked `kelpie-verify-prompt` thread, about 300 a day. The person returns to a stack of identical-looking prompts. Those are the ones the gate relies on them reading, and only the newest is live. Passing a stale one does nothing, but the person has no way to tell them apart, so a rogue phone's prompt can sit among them.

The same happens without an attacker, more gently: someone comes back after 5 minutes, types the password into the old dialog, it passes, nothing happens, and only the app's error tells them why.

**Bound.** Prompts are one per 280 s and every one is audited, so this is fatigue and a slow leak, not a bypass. Late results can never act; I tried.

**Fix.** Count timeouts over a longer window (three in 20 minutes pause). While an abandoned password dialog is still up, refuse a new prompt with "close the earlier dialog first", or close it. CredUI may run in `CredentialUIBroker.exe`, so closing it needs a run to find out how.

---

## Suggestion

| # | Where | What | Fix |
|---|---|---|---|
| S1 | `choices.rs:82`, `:97-100` | **Second review's W5, the rest.** A consent-led option on a question with `run`, `edit`, `create`, `write` or `continue` is still hidden: `Should I create a PR?` with `1. Yes` + description, `2. No` + description, `3. Type something.` leaves only `2`. Real Claude: `Allow this read outside the working directories?` with `Yes, but ask again next time` hides the one-shot yes, which is the opposite of the intent. Errs safe. | Add `but ask again next time` to `ONCE`. Read a label's first line alone for `ONCE` when the rest is a description. Owner's call on the rest (question 3). |
| S2 | `link.ts:614-617`, `:620` | `drop()` nulls the codec before `closed()`, so `inSession` is always false there. A session whose hello timed out (`link.ts:567`), a decrypt failure and a failed probe all count as "no reply". After three, a computer whose handshake *succeeded* is called reset. | Read `codec.ready()` in `drop` and pass it on. |
| S3 | `link.ts:632-638`, `:685` | The "reset" state is sticky. It returns before `failed()`, so `findOutWhy` no longer runs and `reach` stays "reachable"; a refused TCP connection does not clear it. If the computer then sleeps, the phone keeps saying "may have been reset", not "computer offline". | Call `findOutWhy` in that branch. |
| S4 | `link.ts:330-338` | `start()` does not reset `noReplyCloses`. After Forget and a new pairing, one failed connection goes straight to "reset" with a 60 s retry. | `this.noReplyCloses = 0` in `start`. |
| S5 | `screen-sync.ts:98` | `resume` keeps the newest frame's prompt, cursor and title with the acknowledged frame's rows. If that frame's prompt has since gone and the screen equals the acknowledged one, the daemon says `Same` (`sync.rs:386`), so the card stays. Tapping it is safe: `waiting_since` makes it stale. | Now that `held` stores whole views: `this.view = kept`. |
| S6 | `sync.rs:150-152`, `:236`, `:266-272`, `mobile.rs:1810` | "Numbers are never reused" is false: a hidden page sends `unsub`, which removes the slot, and the next one starts at 0. **Wrong screen, rare:** two windows of one phone, where window A holds rev 3, B hides and shows and reaches acked rev 3, and A returns with `have_rev: 3`. The daemon builds on A's old screen, and those rows stay wrong until they change. | A per-daemon or per-device counter that survives the slot. |
| S7 | `screen-sync.ts:123`, `phone_support.py:345-346` | Memory is bounded and the view is always newest, but `min` evicts by number. After a hide and show, the old high revs outrank the new low ones and the bound can evict the frame the daemon builds on. That costs one needless whole frame per show. Slow, never wrong. | Evict by insertion order (`held.keys().next()`), or drop revs above a whole frame's. |
| S8 | `daemon.rs:550-558` | `drop(turn)` comes before `refused()`, so a caller in that gap gets a prompt after the third refusal. One extra at most. | Record the refusal first. |
| S9 | `user_presence.rs:295` | `quoted` misses U+2028, U+2029, U+061C, U+FEFF and U+2060. A name can force a line break inside the prompt, and the permission text still follows it. | Filter these too. |
| S10 | `user_presence.rs:29`, `:184`, `:326`, `:462` | The prompt is Vietnamese, but what comes back ("Verification was cancelled", the pause message) is English, shown as is by `mobile-settings.tsx:89`. | Send a code and translate in the app, or use the same table. |
| S11 | `link.rs:5-7` | The module comment says what fails is "nothing counted"; `held_silently` counts. | Reword. |

---

## Second-review items

| Item | State | Evidence |
|---|---|---|
| W1 real prompt never run | **Not fixed** | Confirmed by the developer's report ("A real OS prompt, by rule"), which gives a good manual list, steps 1 to 12. `user_presence.rs:309` leaves the parent-window question open in a comment. macOS has never run either. |
| W2 Hello face pass; no limit or trace | **Partly** | Audit and pause are real (`daemon.rs:554-557`, `user_presence.rs:201-211`). Face pass still untried, stated as **Open** at `architecture.md:81`. No daemon-owned confirmation. |
| W3 prompt that never ends | **Fixed** | One reply on every path: timeout, inner panic (`Disconnected`), `then` panic (`catch_unwind`), spawn failure (closure drops the `Turn`, `?` replies), client gone (`tx.send` ignored). No `panic = "abort"` in the profiles. A late pass goes to a channel nobody reads, and `then` is only reached through `verify`'s return. Residual: W6. |
| W4 `mobile.py` | **Fixed** | `mobile.py:352`, `:379-387`. Read only, not run. |
| W5 Yes-led menus | **Partly** | `Ship it?` and the form pass. S1 and W1 remain. |
| W6 limit evaded by a late byte | **Fixed, new hole** | Holding gains nothing, with a socket test (`mobile.rs:3246`). W2 above replaces it. |
| W7 reset never shows "not paired here" | **Fixed** | `link.ts:632-638`, 60 s retry, key kept, tests at `link.test.ts:314-366`. Gaps: S2 to S4. |
| W8 docs | **Mostly fixed** | Guide rewritten, file route and `dormant_days` stated, `shift-tab` listed, `:442` agrees with `:456`. W5 above. |
| S1 `CONSENT` words, y/n question | **Partly** | Words added and the y/n case fixed. Menu question, W1. |
| S2 panic in verify or then | **Fixed** | `daemon.rs:560`, test at `:2021`. |
| S3 kill switch disabled while busy | **Fixed** | `mobile-settings.tsx:222` has no `disabled`. |
| S4 logon domain | **Fixed** | `user_presence.rs:429`, tests at `:694-698`. |
| S5 config lock over the write | **Fixed** | Below. |
| S6 `dormant_days` outside the MAC | **Fixed in docs** | `architecture.md:109`. |
| S7 enable and disable not audited | **Fixed** | `daemon.rs:1656-1658`, `mobile.rs:1186-1188`, test at `:3336`. The docs half is W5. |
| S8 transport cap e2e | **Not fixed** | No e2e on a private address; unit-tested. Acceptable. |
| S9 password failure wording | **Fixed** | `logon_refusal`, `:504-510`. |
| S10 `a` versus `A` | **Fixed** | `choices.rs:158`, `:248`, `session.rs:1028`, test at `:327-335`. |

## Checked and sound

- **Refusal counter.** A plain token holder cannot touch it: `is_verified_app` comes before `begin`, and `APP_ONLY` before that. `passed()` needs an OS yes. Only a daemon restart resets it, which is documented and within the accepted residual. A normal person who mistypes three times waits 5 minutes with a message that says so, and the kill switch, removing and lowering all still work. Hello's own retries do not count.
- **Prompt language.** `lang` is read in one place (`daemon.rs:545`). Anything but `vi` or `vi-*` is English, including non-strings. The audit line is always English. A caller's `reason`, `text` and `prompt` are ignored, with a test (`mobile.rs:3660`). The app adds the code at `ipc.ts:15`. `from_code` on `vietnam` gives English, as pinned.
- **Config save.** `saving` then `broken` (released at once) then `config` (held only to serialize). No path holds `config` or `broken` and calls `save`: I checked each `config.lock()` block, and `enable`, `revoke*`, `reset`, `seen_at`, `set_options` and `commit_approval` all end the block first. Serializing inside `saving` makes the last writer newest. No deadlock. `status` takes config then running, pending and codes, with no reverse path.
- **Slots.** Evicted places are forgotten once. `HandshakeSlot::drop` finds nothing and does not decrement again. `notify_one` keeps a permit for a task not yet waiting. No slot is held past the first sealed frame.
- **`link.ts`.** The only path to `h.unpaired()` is `case "revoked"` after `codec.receive`, which decrypts. No forget without a sealed message (`forgetPairing` has two callers: that handler and the person's Forget). No retry storm: "reset" and 4401/4402 wait 60 s, `kick` is limited to one per 2 s, the rest use capped backoff, and timers are cleared in `open`.
- **`screen-sync.ts`.** Bound holds (8). A frame on a missing base clears and refollows, and in-flight deltas each do the same and then converge on a whole frame. Stale acks cannot produce a wrong base, because frames arrive in order and overwrite. The only wrong-screen path is S6. `phone_support.py:329-347` matches the page except that it records `lost` and does not refollow, which suits a test client.
- **`--pair-with`.** See W4.
- **Second-review "Checked and sound" items** in the areas that changed were re-read and still hold: order around the prompt, `Role::App`, test seams, `Wire::close`.

## Unresolved questions

1. W2: fix the eviction, or accept a LAN denial of service with a doc line?
2. Does any shipping agent still show a numbered trust menu (W1)? Whichever the answer, the test at `choices.rs:288` should change, since it pins the opposite of decision 4.
3. S1: should consent-led options with a description, on a question with a word like `run` or `create`, stay refused?
4. W6: can an abandoned CredUI dialog be closed, and does a parented one leave Kelpie's window disabled after the timeout? Only a manual run can say.
5. Hello face pass: still untried (`architecture.md:81`).
6. Phase 3a: phones behind `tailscale serve` look like this machine, so they have no address share and are never counted (`mobile.rs:746`, `:1381`). Judge the loopback exemption again then.
