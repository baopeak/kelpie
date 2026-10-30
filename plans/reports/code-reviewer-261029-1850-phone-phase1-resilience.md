# Code review: phone phase 1, resilience addendum

Branch `phone/phase1-resilience` (uncommitted), base cdc0294. Read-only review; no builds run (findings come from reading the code and following the call paths; none depends on runtime timing).

## Scope
- Rust: `load.rs`, `load/{windows,macos}.rs`, `mobile.rs`, `mobile/{sync,lanes,verbs}.rs`, `session.rs`, `chat.rs`, `daemon.rs`, `server.rs`, `main.rs`, `Cargo.toml`, `Cargo.lock` (adds `socket2` only), `kelpie-proto/src/lib.rs`
- Page and app: `mobile/app.js`, `app/src/lib/attention.ts`; docs: `docs/architecture.md`
- e2e: `phone_support.py`, `mobile_protocol.py`, `mobile_perf.py`, `mobile_resilience.py`, `net_fault_proxy.py`, `stress.py`
- About 1.6k changed lines in tracked files plus about 2.2k new lines.

## Overall
The work is solid. The ack window, the lanes, the bounded event queue, the resolved-waits ring and the load ladder all match the plan. Verb permissions have not changed: `hb` and `ack` need read, and the write verbs still need write. `mobile.load` is user-only (it falls through the `Role::User => true` default in `auth::allowed`, and sessions and anonymous callers are refused). Revoke is the first arm of the biased select, and every request still re-checks `permission()`. The `drive` race fix is correct: `follow()` subscribes under the `feeds` lock, and `drive` re-checks `receiver_count()` under that same lock before it removes itself. If a newer feed has replaced it, the `ptr_eq` check makes it leave that feed alone.

No critical findings. Six warnings, listed below. The most important one is that, with the page that actually ships, `answered_by` is wrong.

## Critical
None.

## Warnings

**W1. Answers from the shipped phone page are recorded as answered by "desktop"**
- Code: `session.rs:669-675` (`write_input`); `mobile.rs:943-975` (`Verb::Input` and `Verb::Key`).
- Only `answer_bytes` sets `typing_by`. `input` (`type_message`) and `key` (`session.input`) both reach `write_input` with `typing_by = None`, so the code falls back to `By::desktop()`.
- The served page never sends `answer`. It uses `input` and `key` only (`app.js:341`, `app.js:347`). So every prompt answered from the page today is recorded as answered by the desktop.
- What the user sees: a second device that taps later is told "stale, answered_by: desktop". The same phone tapping again gets `stale`/desktop instead of `dup`.
- Fix: give `type_message` and the key path a `By` (for example `type_message_by` and `input_by`) and set `typing_by` under the same lock.
- Test gap: the unit and e2e tests only cover the `answer` verb, so they miss this.

**W2. `chat_resolved` and `chat_resync` can arrive before the chat lines they refer to**
- Code: `mobile.rs` `adapt` (~1214-1230).
- Chat lines go on the bulk lane. `chat_resolved` and `chat_resync` go on the control lane, which is written first.
- Failure scenario: a permission request line is queued behind frames in bulk, the desktop answers it at once, and the phone receives `chat_resolved` for a request it has not seen yet. A client that ignores resolutions for unknown requests then shows a card that is already resolved, as if it were still open.
- `app.js` does not render chats, so this hits the phase-2 client and the e2e clients only.
- Fix: send `chat_resolved` through bulk after the lines (it is not latency-critical), or document that clients must remember resolutions for ids they have not seen yet. The same reasoning applies to `chat_resync` overtaking queued deltas; per-message `seq` lets a client de-duplicate, but this is not documented.

**W3. On macOS, a connected phone stops the Mac from going to sleep, which the code comment says it does not do**
- Code: `load/macos.rs:28-29` and the `Activity` doc comment at `:107-108`.
- `0x00FF_FFFF` is `NSActivityUserInitiated`, and that value includes `NSActivityIdleSystemSleepDisabled` (1<<20). The `Activity` doc says "the idle system sleep setting is not touched", which is wrong.
- Effect: a page left open and connected (it reconnects even while hidden, `app.js` onclose) keeps a laptop awake indefinitely.
- The intent is to prevent App Nap. For that, use `NSActivityUserInitiatedAllowingIdleSystemSleep` (`0x00EF_FFFF`). The plan text names `NSActivityUserInitiated`, so ask the lead whether a Mac should stay awake while a phone watches.

**W4. A page loaded before the update stops updating after four frames and reconnects about every 45 s**
- Pages already open keep their old JS across the daemon update (PWAs and kept-alive tabs), and the page never reloads itself when `api` or the epoch changes.
- The old page never sends `ack`, so `frame()` returns `Blocked` after 4 frames (`sync.rs:364`).
- It never answers `hb`, so it is closed with 4410 after 45 s. The old `onclose` handler treats 4410 as "reconnect after 2.5 s". It then gets 4 more frames and freezes again.
- The daemon already reads the legacy `rev` field (`mobile.rs:410`), so this case was anticipated, but it is not handled.
- Options:
  - The page reloads itself when `hello.api` is greater than the API level it was built for. This fixes future updates only.
  - The daemon does not apply the ack window to a `sub` that sent `rev` and no `have_rev` until that connection's first `ack`. This also fixes the pages open now.

**W5. The two-changes-a-minute limit also delays escalation**
- Code: `load.rs:253-257`.
- Under load that ramps up, L0→L1 and then L1→L2 use up the budget. Memory then dropping below 8 % (L3, `drop_idle`) is held back until 60 s after the first change.
- The plan states the limit without a direction, so this follows the plan, but it delays reaction to the low-memory event, which is the one input that cannot wait.
- Fix: rate-limit only descents, or exempt raises to L3 and `low_memory`.

**W6. An e2e heartbeat check passes when no heartbeat arrives at all**
- Code: `mobile_resilience.py:136`.
- The check is `beats[:3] == [20, 28, 36] or all(abs(a - b) <= 1 for a, b in zip(beats[:3], ...))`. With `beats == []`, `zip` is empty and `all()` returns True.
- Line 137 (the 4410 at 44-49 s) still catches a dead daemon, but the "asked at 20/28/36" claim is not proven.
- Fix: also require `len(beats) >= 3`.

## Suggestions
- **S1. The chat path closes without 4411.** When the chat path decides the phone is too slow, `adapt` calls `control.slow()` and then returns. In `serve`'s biased select, the `_ = &mut *adapter => return` arm comes before `wires.slow.notified()`, so the socket is dropped without the 4411 close frame and the page waits 2.5 s instead of 0.3 s. Fix by moving the slow arm above the adapter arm.
- **S2. The docs and the code differ in a few places:**
  - Answer results and errors are written inline from the request arm, not through the control lane.
  - 4411 is sent only when the control lane overflows (a stuck bulk write just times out).
  - The `daemon.unresponsive` event is missing from the Events list.
- **S3. Unsubscribed sessions stay outside the resume-cache bounds.** `Verb::Unsub` aborts the `run` task, so the `f.views.close(...)` at the end of `run` (`sync.rs:530`) never runs. The slot stays `left: None` for the life of the connection, and `trim` never evicts it. This is pre-existing, but it contradicts the new "20 sessions per device" bound for connected devices. Fix: call `views.close(device, id, conn)` in `Unsub`.
- **S4. `chat_room` counts the wrong thing.** It counts other connections with any chat output pending (`waiting > 0`), including live deltas, not resets in flight. At L2/L3 this can bounce between `busy` and `chat_resync` once a second. Count only resets, for example a flag set while a `reset` attach is being drained.
- **S5. One typist lifts the shedding for every phone.** The typing boost is per session (`Views::boost` sets `Feed.fast_until`, and each `run` reads `typing(id)`). One typist puts all 8 phones that follow that session back at the calm rate at L3. Keep the shared render fast, but pace each `run` by (device, session).
- **S6. `mach_host_self()` once a second (`macos.rs:85`) adds a send-right reference each call.** Many projects cache it or deallocate it. This is compile-only code, so check before shipping on macOS.
- **S7. The EcoQoS opt-out is never undone after phone access is turned off.** It stays in force for the life of the process, which costs battery on laptops. Acceptable per the plan; note it in the docs.
- **S8. Very slow links (about 6 KB/s or less) may never get a frame (not measured).** The first frames are whole (up to 64 KiB). On such a link each write can wait over 10 s for room in the socket buffer, the write is cut, the phone reconnects and gets a whole frame again. Consider judging progress by bytes accepted rather than per message, or allowing one frame in flight until the first ack.
- **S9. Timing windows in the e2e scripts.**
  - `mobile_resilience.py:137/147` expect the drop at 44-49 s. A host stall of 1 s or more legitimately restarts the clock (`healthy_since`), so on a loaded host this fails even though the daemon is right. Widen the upper bound or record stalls.
  - The `--quick` flag is still fine.

## Deviations listed by the developer
- **Ack cadence (every 2 frames or 250 ms):** accept. The plan's 8 frames combined with a window of 4 can never work.
- **Typing boost:** accept as an echo-gate necessity, with S5.
- **Watchdog without backtraces, Windows thermal input absent, CPU/memory only on Windows and macOS:** accept.
- **10 s write cut:** accept, with S8.
- **`PHONE_V2_API` renamed to `PHONE_API`:** no other users (grep), so accept.
- **`mobile.load` RPC:** user-only, so accept.

## Edge cases checked and found OK
- The bounded queue reports `Ok` on overflow, so `daemon.broadcast` and `chat.resolve`'s `retain` keep the client. Overflow gives credit back, and `housekeeping` re-sends the list and `chat_resync`.
- A replaced connection's `run` gets `Lost`. `leave` and `close` check the owner. Acks from an old connection are ignored.
- `answer_bytes` checks the resolved ring before the status, under one lock, so the first answer wins. The desktop RPC `session.answer` maps every non-`UnknownKey` error to the same message as before.
- The Windows FFI is fine: out-pointers are valid, `dwLength` is set, every handle is closed, and `SetProcessInformation` gets `ControlMask=EXECUTION_SPEED, StateMask=0` (the correct opt-out). `macOS` minimum version is 12.0, so `isLowPowerModeEnabled` exists. `HOST_CPU_LOAD_INFO` has 4 counters, in user/system/idle/nice order.
- `KELPIE_LOAD_FORCE` comes only from the daemon's own environment. Invalid values are ignored.
- The load measuring runs exactly as long as the server's lifetime. The `Done` guard runs on abort. Each start gets its own watchdog thread, stopped by its flag. The callback holds a `Weak<Daemon>`, so there is no reference cycle.
- No MMCSS code exists. That is correct: the plan puts MMCSS in phases 8/9.

## Recommended actions (in order)
1. W1: record who typed for `input` and `key`, and add an e2e check that uses the page's `key` verb.
2. W3: change the activity flag, or get the lead's decision.
3. W4: the page reloads on a newer API level, and/or the daemon does not apply the window to clients that never ack.
4. W2: move `chat_resolved` behind the chat lines, or document the rule for clients.
5. W5, W6, S1, S3; then the docs fixes in S2.

## Metrics
- Type coverage: Rust strict; TypeScript change is one constant.
- Tests: the developer reports 357 kelpied and 23 proto tests passing, and clippy clean (not re-run here).
- Lint: no lint script is configured.

## Score: 8/10

## Unresolved questions
1. Should a Mac stay awake while a phone is connected (the plan's flag) or only avoid App Nap?
2. Is API level 16 reserved by another branch, or should this be 16?
3. Must pages loaded before this update keep working until they are reloaded (W4), or is "reload after update" acceptable for phase 1?
4. The stress gate "L2 within 5 s of the stress starting" has no clean measurement yet: the host was already at L3.

Status: DONE_WITH_CONCERNS
Summary: No critical issues. The ack window, lanes, load ladder, first-wins answers and permissions are correct. The main defects are answers from the shipped page being recorded as answered by "desktop", the macOS activity flag stopping sleep, and pages loaded before the update freezing.
Concerns/Blockers: W1 should be fixed before commit; W3 and W4 need a decision from the lead.

## Re-review 1 (fix round 1)

Read-only review: no builds and no daemons. I checked each fix against the code; the test results quoted below are the developer's.

### Fixes confirmed
- **W1 (who answered): fixed. Keys pressed on the desktop are not credited to a phone.**
  - `input_by` and `type_message_by` set `typing_by` while holding the session lock, and `write_input` uses it and clears it inside that same lock (`session.rs:655-660, 773-788`). The phone's `input` and `key` handlers pass the device (`mobile.rs:962, 979`).
  - Every other writer still goes through `input()`, which passes `By::desktop()`: the desktop RPCs, session scripting, `enter_if` and automations. No phone can be credited for a desktop write.
  - Tests cover a second device, the same device, and a desktop key.
- **W2 (chat ordering): fixed, with one new problem (R1 below).**
  - The adapter waits until each chat line is in the bulk lane before it handles the next event, so a `chat_resolved` or `chat_resync` reaches the control lane only after its lines are queued. Draining bulk first therefore sends the lines before the resolution.
  - The real-socket test proves it.
- **W3 (macOS sleep): fixed.** The flag is now `0x00EF_FFFF` (`NSActivityUserInitiatedAllowingIdleSystemSleep`), and the comments are corrected. A one-constant change, so not re-compiling the macOS file is acceptable.
- **W4 (pages from before the update): fixed, and a client cannot use legacy mode to escape revoke or the limits.**
  - Choosing legacy still requires a valid token (checked by `device_for`). Revoke is still the first arm of the loop. Every request still re-checks its permission. The event queue (256), the lanes (64 control / 4 bulk), reading the screen only after a lane place is reserved, and the 10 s write cut all still apply.
  - In legacy mode a frame counts as acknowledged as soon as it is built (`sync.rs:426`), which is the original protocol: frames follow the last one built, and a page that lost one asks again and gets a whole frame.
  - Legacy gives up two things: the ack window, which a client that does not acknowledge could not use anyway, and the silence deadline. A dead legacy connection is still ended by the OS keep-alive (about 60 s) or the 10 s write cut.
  - A page from before the update gets what it was built for. It sends `{type:"auth", token}`, which the daemon reads. It receives `load.status` messages and ignores them; it never receives `hb`; it gets no 4410 loop.
  - The new `app.js` sends `caps: ["ack","hb"]`, as do `phone_support.py` and `mobile_perf.py`.
- **W5 (escalation held back): the rate limit is fixed, but the change goes further than the finding asked (R2).**
- **W6 (empty heartbeat check): fixed.** The check now requires `len(beats) >= 3`.
- **S1 (4411):** fixed by arm order; the slow arm now comes before the adapter arm.
- **S3 (unsubscribed sessions in the cache):** `Unsub` now calls `views.close`, and the owner check stops it from evicting a slot another connection now owns.
- **S7 (EcoQoS left on):** documented.

### New findings
- **R1 – Warning; fix before commit. The W2 drain loop has no end, so it can hold off revoke, heartbeats and the phone's own messages.**
  - Code: `mobile.rs` `serve`, the control arm, `while let Ok(queued) = wires.bulk_rx.try_recv()`.
  - Each `send` frees a place in the bulk lane. A run task waiting on `reserve()`, or the adapter waiting to send a chat line, fills it again, so the loop goes on for as long as the producers keep up with the socket. While it runs, the `revoked`, `tick` and `ws.recv` arms never run.
  - Failure scenario: a phase-2 client watches a chat and follows terminal sessions over a 100 KB/s link, and a `chat_resolved` arrives. The frame producers stop only once each followed session has 128 KiB unacknowledged (acknowledgements are not read during the loop). With 16 sessions that is up to about 2 MiB, roughly 20 s. A chat streaming faster than the link has no limit at all.
  - During that time a revoked device keeps receiving screen and chat data. Revoke used to wait for at most one send.
  - Pages from before the update never attach chats, so they are not affected.
  - Fix, either of:
    - Drain only what was queued at that moment (`for _ in 0..wires.bulk_rx.len()`; the lane holds 4).
    - Have the adapter put `chat_resolved` and `chat_resync` on the bulk lane after their lines, and drop the drain.
  - Add a test: revoke while bulk keeps being refilled.
- **R2 – Suggestion; needs the lead's acknowledgement. W5 also removed the "enter after 2 samples" rule.**
  - Code: `load.rs:232-235`.
  - Any rise now applies in the same second. The finding asked only to exempt rises from the two-a-minute limit.
  - In practice this matters little. Lag is already a p95 over 5 s, CPU needs 3 seconds above 95 % for L2, and free memory does not spike. What remains is that one second of CPU above 85 % (a compile burst on 4 cores) puts the machine at L1 for at least 10 s and sends `load.status` messages to every phone.
  - This departs from the plan text ("enter after 2 consecutive samples"). Either note it in the plan, or keep 2 samples for L1 and rise at once to L2 and L3.
- **R3 – Suggestion. A wait answered more than 10 minutes ago can be credited to a later typist.**
  - Code: `session.rs` `note_resolved`.
  - `note_resolved` runs on every write while the session waits. Once the ring has dropped the wait (10-minute TTL) and someone types into the same open wait again, it pushes a new entry for that later typist and overwrites `answered_now`. Later taps are then told the wrong person answered.
  - This needs a wait left open over 10 minutes after its answer, so it is rare. Fix: record only when `answered_wait` was false before the write.
- **R4 – Note for phase 4. Legacy connections have no application-level liveness check.** Through a relay, which keeps its own TCP connection alive, a dead legacy client would never be noticed. Require `caps` on the relay path.

### Deferred suggestions: none blocks the commit
- **S4 (chat budget counts live output):** not blocking. It only causes `busy`/`chat_resync` churn at L2+ and no data is lost. Fix it with the phase-2 chat client.
- **S6 (`mach_host_self`):** not blocking. It is macOS-only code that the test suite only compiles and never runs; fix it before the first macOS release.
- **S8 (very slow links):** not blocking. It is unmeasured and only applies to very slow links.
- **S9 (e2e timing windows):** not blocking. It affects test flakiness on a loaded host, not the product.

### Score: 8.5/10 (9/10 once R1 is fixed)

### Unresolved questions
1. R2: should the plan record "rises take effect at once", or should L1 keep the two-sample entry?
2. R4: should phase 4 refuse legacy (no-`caps`) clients on the relay path?

Status: DONE_WITH_CONCERNS
Summary: W1–W6, S1, S3 and S7 are fixed correctly. Legacy mode cannot be used to escape revoke or the bounds, and keys pressed on the desktop are not credited to a phone. The W2 ordering holds, but its drain loop has no limit (R1).
Concerns/Blockers: R1 should be fixed before commit, with a regression test. R2 needs the lead to acknowledge the plan deviation. S4, S6, S8 and S9 do not block.

## Re-review 2 (fix round 2)

Read-only review: no builds and no daemons. I checked each fix against the code; the test results are the developer's.

### 1. R1 (unbounded drain): fixed. The regression test does not reproduce the old bug.
- **Fixed.** `adapt` now puts every translated message (`chat`, `chat_resolved`, `chat_resync`) on the bulk lane, in order, with the 10 s timeout leading to `slow` (`mobile.rs:1231-1253`). The drain is gone.
- **Ordering.** The adapter is one task and the lane is first in, first out, so a resolution or resync is always written after its lines. The ordering test still covers this.
- **Waiting.** `serve` now has no unbounded inner loop. Each arm writes at most:
  - one message (control, bulk, the list, `load.status`, a reply); or
  - the tick's bounded set: `hb`, `load.status`, then `housekeeping` (the list plus at most 16 `chat_resync`).
  So revoke, heartbeats and inbound messages wait for at most the send in progress (10 s at worst) and the tick's bounded set.
- **The test (`a_revoked_device_is_cut_off_within_a_send_however_full_the_lanes_are_kept`) does not reproduce R1.** The old drain ran only when a control message containing `"type":"chat_` arrived. The feeder produces `assistant` lines and screen output only.
  - Before the revoke at 1 s it sends about 0.3-1 MB. That is far below the 4 MiB chat budget, so no `session.resync` occurs.
  - An overflow `chat_resync` is written inline from the tick, not through the control lane.
  - So no `chat_*` control message reaches the round-1 writer, the drain is never entered, and the test would have passed on the round-1 code.
  - It is still a useful guard for revoke latency under a full bulk lane, but it is not a regression test for R1.
  - To make it one, have the feeder also make the chat issue a `control_request` and answer it every few iterations, so that `chat_resolved` keeps arriving while bulk is being refilled. Then check by hand once that it fails against the round-1 code.
  - Not blocking: the defective code is gone, and the code paths above show there is no loop left.

### 2. The control lane: remove it (KISS)
- Nothing uses it any more. `Control::send` is marked `allow(dead_code)`. The `control_rx` arm in `serve` waits forever, because the adapter's `Control` keeps the channel open but only calls `slow()`.
- The plan's requirement ("control lane written before bulk; hb, acks, answer results, revoke, errors, `load.status`") is met by the single writer: it writes those inline, in arms that come before bulk.
- Recommendation:
  - Pass the adapter only the `slow: Arc<Notify>`.
  - Remove the `Control` channel, `CONTROL_CAPACITY`, the `control_rx` arm and the lane-capacity test.
  - Update `docs/architecture.md:386-391`, which still describes "the control lane (64) is first in the writer's order".
- Phase 2+ can add a channel when a real producer task appears. An `allow(dead_code)` placeholder in shipped code is speculative abstraction.
- This is a small change and best done in this commit, but it is not a correctness blocker.

### 3. R2 and R3: both correct
- **R2 (`load.rs:231-270`): correct.**
  - Rises need `ENTER_AFTER` (2) samples in a row and go to the lowest level asked for over those samples.
  - Only falls set `moves` and are held back by the two-a-minute limit.
  - Both `above` and `below` are cleared on every change.
  - `rises_are_never_held_back_by_earlier_changes` checks L1 at 1 s, L2 at 3 s and L3 at 5 s; with the old limit, the L3 rise would have been blocked. `at_most_two_falls_a_minute` checks the fall limit.
  - The plan text and the docs match the code.
- **R3 (`session.rs` `write_input`): correct.**
  - Who answered is recorded only on the write that sets `answered_wait`, so later typing into the same open wait never takes the credit or adds an entry to the ring.
  - The test empties the ring, types from a second device and from the desktop, and then asserts that the first device still has the credit and that no new entry was added. That is the failure scenario from R3.
  - One minor leftover:
    - If the first write lands between a hook reporting `waiting` and the next `tick`, `waiting_since` is still `None`, so `note_resolved` records nothing.
    - Later attempts are then refused as `stale` with reason `answered`, without saying who answered.
    - This is less information, not wrong credit, and it is rare; not worth more work now.

### 4. Pinning `mobile_protocol.py` calm: it hides no resume bug; keep it pinned
- **The failure under load is designed behaviour.** At L3, every connection's tick calls `views.drop_idle()` (`mobile.rs:1175`), which drops every resume slot whose connection has ended. This is what the plan asks for ("cleared at L3").
  - The script closes the phone, waits 1 s and reconnects. With the host at L3 and the script's other phones still connected, the slot is gone, so `have_rev` matches nothing and a whole frame comes. That is the "carries on from the last frame it acknowledged" failure.
- **The second cause is a race in the script, not a daemon fault.** The phone counts a frame as acknowledged when it sends the `ack`, and then closes at once. The daemon can stop reading that socket before it reads the queued ack: its writer gives the tick (which sends `load.status` every second at L1 and above) priority over reading. The safe fallback is a whole frame, which is what happened. A loaded host makes this window wider.
- **Nothing else in the resume path loses or skips an acknowledged frame.**
  - The acknowledged rows are exactly what the phone holds after applying the frame (`Held.rows`), even when a frame is cut to the L2/L3 row budget and marked `more`.
  - Shedding and the background pause only delay frames; they never touch `acked`.
  - `open_as` keeps `acked` only when it equals `have_rev`, and `leave` only clears `flying`.
- **The ticker p95 of 1.4 s at L3 is also designed behaviour:** the session being viewed gets one frame a second at L3.
- **Decision: keep `mobile_protocol.py` pinned to `L0:calm`.** It tests protocol correctness. Load behaviour belongs to `mobile_resilience.py` part 7 (L2 and L3 forced) and to `stress.py`.
- **Gap to close (not blocking):** nothing checks resume while loaded. Add to `mobile_resilience.py` part 7:
  - at forced L2: a reconnect within 5 minutes resumes from the acknowledged frame;
  - at forced L3: a whole frame comes, as documented.
- **Optional:** run `drop_idle` only when the L3 reasons include memory. At L3 from lag or CPU, dropping at most 64 KiB slots frees nothing useful, and it makes every phone that loses its network on a busy machine reload the whole screen. The plan text says "cleared at L3 / low-memory", so that would need the lead's agreement.

### Score: 9/10

### Unresolved questions
1. Should the L3 `drop_idle` run only when the L3 reasons include memory (this changes plan wording)?

Status: DONE_WITH_CONCERNS
Summary: R1–R4 are fixed correctly. The writer has no unbounded loop, chat ordering holds through the first-in-first-out bulk lane, and the R2 and R3 logic and their tests are right. Pinning the protocol script hides no resume bug: under load the failures come from the designed L3 cache clear and from the script closing right after its ack.
Concerns/Blockers: Nothing blocks the commit. Recommended:
- Remove the unused control lane (KISS) and correct the docs lines about it.
- Make the R1 test also produce `chat_resolved` so that it actually reproduces the old bug.
- Add L2/L3 resume checks to `mobile_resilience.py`.
