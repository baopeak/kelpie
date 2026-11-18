# Code review: phone screen lock freeze (fix/phone-screen-lock-freeze)

## Scope
- Diff: `git diff origin/master`, which is commit 73c1639 plus the uncommitted COST edit in `scripts/e2e/screen_phone.py`.
- Files: `app/phone/src/{page-sleep.ts,page-sleep.test.ts,phone-state.ts,lock.ts,lock.test.ts,screen.ts,screen.test.ts,screen-view.tsx}` and `scripts/e2e/screen_phone.py`.
- The `.claude/agent-memory/code-reviewer/*` working-tree edits are left over from an earlier review and do not belong to this change. Do not commit them with it.
- Checks run: `pnpm exec vitest run phone/src/page-sleep.test.ts phone/src/lock.test.ts phone/src/screen.test.ts` passed 138/138. `pnpm typecheck` (which includes `phone/src`) is clean. No e2e, cargo or browser runs, as instructed.

## Overall
The visibilitychange refactor keeps the old behaviour and its order exactly. The sleep detector is small and covered by tests, and the hiddenAt clamp is correct. Part (b), the wedged-decoder check, does not reliably do its job. It only runs when a keyframe arrives. A wedged decoder stops the acknowledgements, and once those stop the phone's existing triggers usually never ask for a keyframe, so the picture can stay stuck. The e2e does not show which path brought the picture back, and its "frozen with no hiding" check exercises Chrome's `resume` event rather than the late tick that iOS depends on.

## Refactor equivalence (verified)
- Hide, before and after: `lock.left`, then close screens and takeovers when grace > 0, then `link.setHidden(true)`, then dispose the syncs. Same order.
- Show, before and after: `lock.back`, then open the wanted session, then `link.setHidden(false)`, then `recompute`, then `shellReload.visible`. Same order.
- The only addition is `sleep.reset()`, which runs first and changes nothing else.
- `ScreenSync.dispose()` only cancels a pending acknowledgement, so calling it on a visible page from `slept()` is harmless. `setHidden(true)` followed by `setHidden(false)` unsubscribes, then kicks and resubscribes. After a sleep that is what we want, because the kick probes the connection that died.

## BLOCKING

### B1. The wedge check only runs when a keyframe arrives, and a wedged decoder usually stops keyframes from being requested
Files: `app/phone/src/screen-view.tsx:351-354` (check), `:377` (keyFedAt); `app/phone/src/screen.ts:1643,1652-1655` (keyframe triggers); `crates/kelpie-screen/src/control.rs:872-875` (`may_encode`).

`decoderWedged` runs only inside `onMedia` for a keyframe. Keyframes come only on request, because the GOP is effectively infinite (`encode_mf.rs:5,198`). A wedged decoder never outputs, so the phone never sends `sc_ack`. The helper's flight window (`MIN_WINDOW` = 3 frames) closes after three unacknowledged frames, and the helper stops encoding. After that, the phone's two keyframe triggers are unlikely to fire:
- `STUCK_MS`: `mediaAt - decodedAt > 1000`. The three frames arrive within about 100 ms of the last decode at 30 fps, then media stops. The difference stays near 100 ms, so this never fires.
- `STUCK_SHOW_MS`: needs `stats.fps > 0`. `Stats::frame` counts frames the helper actually sent (`control.rs:983-985,1046`). The report after the window closes says fps 0. The rule therefore fires only if, at decodedAt + 2 s, the phone has not yet received that zero-fps report. That is a timing race, roughly one chance in three or four, and it gets one attempt.
- `FREEZE_MS` does not fire either, because `sc_stats` keeps `heardAt` fresh (`screen.ts:1354`). The daemon also drops input while the picture is stale (`screen.rs:681-683`), so a tap cannot produce new frames.

**Scenario: wedge in the middle of a stream.** The decoder was working and wedges on deltas. `keyFedAt` is still null, because it is set only for keyframes. Three deltas arrive, the helper is gated, no keyframe is requested, and `decoderWedged` is never evaluated. The picture stays stuck for good, which is the owner's symptom. Even when one keyframe does arrive, it only sets `keyFedAt`. A second keyframe 1.5 s or more later is needed before anything is rebuilt.

The e2e reports the picture coming back. Nothing in it proves the recovery came from `decoderWedged` rather than the STUCK_SHOW race or a setup change (see N3). Run section 8 several times. If B1 is right, some runs will hit the 12 s timeout.

**Fix:** detect the wedge from a timer rather than from keyframe arrival. Every 250 ms, in the Live view or the remoteScreens watch: if any chunk has been fed since the last output and `now - firstFedSinceOutput >= DECODER_WEDGED_MS`, treat it exactly like the decoder's `error` callback. That means `setup = null`, `skipToKey = true`, apply and acknowledge the pending frames through `remoteScreens.skipped(last)` (which also asks for a keyframe), and `redraw()`. Acknowledging the pending frames frees the helper's and the daemon's windows, so the requested keyframe actually gets through. To avoid N5, confirm with `decoder.flush()` raced against a timeout before declaring a wedge. A decoder that is only holding frames back will release them on flush; a wedged one will not resolve.

## NON-BLOCKING

### N1. The `resume` path drops the time the page went to sleep
File: `app/phone/src/phone-state.ts:983-985`.

`slept()` is called with no `since`, so `lock.left()` takes `hiddenAt = now` and the frozen time never counts toward the grace. Calling `slept()` also resets the detector, so the late tick that would have supplied `since` is thrown away. The commit message says "from the time the page went to sleep", which is not true for this path.

Scenario: Chrome freezes a visible page for 10 minutes with a 5 minute grace. On resume the screen ends, but the app does not lock. The idle-time clause still applies.

Fix: `if (!document.hidden) slept(sleep.tick(Date.now()) ?? undefined)`. In practice Chrome freezes a visible page only through CDP, so this listener mostly serves the e2e (see N2).

### N2. The e2e "frozen with no hiding" check tests `resume`, not the iOS late tick
File: `scripts/e2e/screen_phone.py:657-660`.

When `Page.setWebLifecycleState active` runs, Chrome fires `resume`, which calls `slept()` and then `sleep.reset()`. The late `setInterval` tick that follows sees `last === null`. So the tick, the only path Safari has, is never exercised end to end. Its wiring (`tick`, then `sleep.tick(Date.now())`, then `slept(since)`) is covered only by the unit test of `PageSleep`.

Fix: for this check, add a capture listener in the init script that swallows `resume`, for example `window.addEventListener("resume", e => window.__noResume && e.stopImmediatePropagation(), true)`. Or run the check once with each path.

### N3. The e2e wedge check cannot show which mechanism recovered, and only covers the easy case
File: `scripts/e2e/screen_phone.py:166-186,644-652`.

The hook wedges only decoders that already exist. Any decoder made afterwards works normally. A real post-sleep hardware wedge may also affect the next decoder. The check passes whenever frames come back, by any route.

Fix:
- Assert through the hook that a new decoder was made with the same `codedWidth` (`made.size` grew and `sameSetup` held). That shows the rebuild was caused by the wedge and not by a setup change.
- Add a variant in which new decoders stay wedged for N keyframes. That shows rebuilds stay bounded (one per keyframe or slower) and that the picture returns once the wedge clears.
- Record which trigger asked for the keyframe, for example with a counter on `needKeyframe`.

### N4. A wedge rebuild leaves `pending` without acknowledging it or applying its unpaints
File: `app/phone/src/screen-view.tsx:283-288`; compare the error path at `:309-321`.

`configure()` is reached from a wedge with `pending` still full. Those frames are never acknowledged, and their unpaints never apply, so crisp tiles can stay over regions that have changed.

The daemon also drops all video, keyframes included, while `unacked >= window.1` (`crates/kelpied/src/screen.rs:602`). With a large keyframe (around 200 KB at 1080p, `control.rs:360`, against the 192 KiB minimum window), the daemon can drop the very keyframe that would trigger the rebuild. That leaves a permanent stall. The fix proposed in B1 (reusing the error path) also solves this.

### N5. Decoders that hold frames back can be rebuilt over and over on a still screen
File: `app/phone/src/screen-view.tsx:354`.

`Backlog` notes that some Android decoders "always keep two or three" chunks. On such a decoder, a keyframe can stay inside with no output until more input arrives.

Scenario: a new screen on a still desktop. K1 is held. STUCK_SHOW or STUCK_MS asks for a keyframe, and K2 arrives 1.5 s or more later. The check calls it a wedge and rebuilds the decoder, discarding K1. K2 is then held in the new decoder. Keyframes keep coming about once a second (`mediaAt - decodedAt` keeps growing), and the decoder is rebuilt every 2 s or so. The picture never appears. Before this change, the third keyframe pushed K1 out and the picture appeared.

The rebuild rate is bounded (at most one per keyframe), so this is not a hot loop. The cost is a black picture. Whether it happens depends on the device. A `flush()` probe (see B1) rules it out.

### N6. False positives of the 5 s gap and what they cost
File: `app/phone/src/phone-state.ts:954-958`.

What a false positive costs:
- With grace > 0: the screen or takeover ends, and `hidSinceUnlock` means the next one asks for Face ID. If the gap is at least the grace, the whole app locks.
- With grace = 0: `lock.left` locks the whole app at once (`lockOut`, `lock_now`).

Cases:
- **Real:** `confirm()` blocks the event loop. It is used at `app.tsx:128,196`, `settings-view.tsx:298` and `phone-state.ts:794` (the pairing replace question). Answering one after 5 s or more counts as a sleep. With grace = 0 and the answer "Cancel", the app locks. Low impact, since no screen is open in those views. Fix: call `sleep.reset()` after each `confirm()`.
- **Real but rare:** a forward jump of the wall clock by 4 s or more, from network time correcting after airplane mode or a manual clock change, ends the screen and asks for Face ID. A clock check cannot separate this from a real sleep, because `performance.now` may stop while the phone sleeps. Acceptable.
- **Not an issue:** background tabs and occluded or minimized desktop windows are reported as `hidden`, so the tick returns early and visibilitychange resets the detector. Chrome's intensive timer throttling only applies to hidden pages. When a laptop or desktop sleeps, ending the screen is the right result. A paused debugger only matters during development. A 5 s synchronous task would be a bug of its own. To my knowledge, Low Power Mode throttles requestAnimationFrame rather than 1 s timers (not verified).
- **Passkey sheet:** if iOS stalls timers during the Face ID sheet without a visibilitychange, `lock.asking` is true for all of `go()`, including the round trip, so `left` and `back` do nothing and the pending screen still opens afterwards. `wentAway` still closes any open takeover during a `recheck` ceremony, as the old visibilitychange path already did. Low risk.

### N7. Double handling is benign
- If Chrome ran the late tick before `resume`, `slept()` runs twice. The second `left`/`back` pair finds nothing late, screens are already closed, and the only cost is an extra unsubscribe and resubscribe.
- If iOS delivers a late visibilitychange hide/show after the tick has already handled the sleep, it is handled again with the same idempotent effects.
- If a visibilitychange arrives while the page is asleep, `reset()` stops the gap from being counted twice.

## Bookkeeping checks (verified)
- `configure()` resets `keyFedAt` and `outputAt`. The error path sets `setup = null`, so the next keyframe reconfigures and resets both.
- Keyframes are never skipped by `Backlog`: `backlog.reset()` runs for every keyframe, and `skipToKey` is cleared before the skip test. So `keyFedAt` reflects keyframes that were actually fed.
- `keyFedAt` is set when the keyframe is fed, after it has fully arrived. A slow relay (around 150 kbps) delays when the keyframe arrives, not the measurement. Any output after it was fed, including older deltas, clears suspicion. A slow first decode would need more than 1.5 s with no output at all, which is unlikely.
- The hiddenAt clamp `Math.min(since ?? now, now)` is correct (a time in the future is not believed), and `??=` keeps an earlier real hide.
- `since` is the last tick, up to 1 s before the actual sleep. That errs toward locking, which is acceptable.

## Recommended actions
1. (B1 and N4) Detect the wedge from a timer and recover through the decoder's error path: acknowledge pending frames, ask for a keyframe, `setup = null`, `skipToKey = true`. Optionally confirm with a `flush()` probe first (N5).
2. (N1) Pass `since` from the `resume` handler.
3. (N2, N3) In the e2e, suppress `resume` for the no-hiding check, prove the rebuild came from the wedge check, add a case where new decoders also stay wedged, and run section 8 several times.
4. (N6) Call `sleep.reset()` after `confirm()`.

## Unresolved questions
- Does an iPhone that locks actually suspend Safari without a visibilitychange? The fix is built on that assumption, and nothing has checked it on a device.
- Is the iOS hardware decoder wedge per decoder (so a new decoder recovers) or system-wide after a wake (so new decoders wedge too)? The answer decides whether a rebuild loop like N5 matters in practice.
