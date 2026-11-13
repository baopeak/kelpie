# Code review: screen flow quick wins (worktree D:\kelpie-rd-0, branch screen/flow-quick-wins, base 6f2aea2)

Scope: the uncommitted diff (26 files, about +1200/-480) plus the untracked `scripts/e2e/screen_net.py` and `scripts/e2e/screen_support.py`. Spec: `plans/261112-1405-phone-remote-desktop/phase-00-quick-wins.md`.
Checks run: `pnpm exec vitest run phone/src/screen.test.ts` (38/38 pass), `pnpm i18n:check` (0 problems), `py_compile` + `pyflakes` on the four scripts (clean). I did not run cargo, typecheck or the e2e scripts, as instructed.

## High

### H1. The frozen screen's 2 s kick aborts every connection attempt that is still in progress
- `app/phone/src/screen.ts:1081-1084` calls `port.kick()` every 2 s while the screen is frozen. `Link.kick()` (`app/phone/src/link.ts:522-549`) does not check `facts.connecting`. Once 2 s have passed since `lastKickAt` it calls `open()`, and `open()` calls `detach()` (`link.ts:839`, `:1085-1095`), which closes every socket that is still being set up.
- Failure scenario: Wi-Fi to cellular, with a handshake (TCP+TLS+WS+Noise, Tailscale or DERP) that takes more than 2 s.
  - t=3 s: the probe fails and the backoff retry starts an attempt.
  - t=4 s: the kick sees the link not ready, with `lastKickAt` still at -inf, so it calls `open()` and kills that attempt.
  - t=6, 8, 10 … s: each kick kills the attempt the previous kick started.
  - The link never completes, the 45 s grace runs out and the screen ends `link_lost`. Without the screen's kick it would have reconnected. `CONNECT_MS = 8000` (`link.ts:369`) shows that connects slower than 2 s are expected.
- Before this change, `kick()` ran only on pageshow or online, so this was not reachable.
- Fix:
  - In `ScreenSession`, kick only when the link is ready (to probe) or idle (not connecting).
  - Or make `Link.kick()` return early when `facts.connecting` and the attempt is younger than `CONNECT_MS`.
  - Add a link test: a kick every 2 s during a 5 s handshake still connects.

### H2. Input queued behind a text bypasses the 1 s freshness rule and the stale-picture rule
- `crates/kelpied/src/screen.rs:557-569` queues `sc_input` while `typing`. The flush at `:632-641` sends the queue with a new `"t": now_millis()`.
- The helper drops input stamped more than 1 s ago (`input_loop`, `win/stream.rs:951-957`, `TTL_MS`). The daemon's `fresh(st)` and `Self::stale` checks ran only when the input arrived. Restamping defeats both.
- Failure scenario: the app or helper is slow to answer `screen.text`. Examples are a hung foreground program under `SendInput`, or a 4096-char text, up to `provider::TIMEOUT` = 10 s.
  - The phone keeps showing a live picture, because acks now flow.
  - The user taps or drags meanwhile.
  - Up to 32 of those ops are then replayed seconds later, at places the user may no longer be looking at.
- Before this change, those `sc_input` waited on the blocked read loop and then failed `fresh(st)`, so they were dropped.
- `docs/architecture.md:1018-1019` still promises "Input older than 1 s by the computer's clock is dropped by the daemon and by the helper".
- Fix:
  - Queue `(ops, arrival_ms)` and forward the original stamp, or drop entries older than `INPUT_TTL_MS` at flush time.
  - At flush time, re-check `Self::stale`.
  - Keep the release-if-held rule for entries that are dropped.

## Medium

### M1. The flush races new input, so input order can flip
- In the text future (`screen.rs:632-641`), `typing = false` is set and the queue is taken under the lock, but the notices are sent after the lock is released.
- A concurrent `sc_input` on the connection task (`:557-569`) can see `typing == false` in that gap. Its `screen.input` notice then reaches the app before the flushed ops (multi-thread runtime).
- Result: a queued `down` can arrive after a newer `up` (button left pressed), or a queued `Enter` after later keys. Requirement 3 (input order) is violated.
- Fix: send the flushed notices while still holding `hold.state` (`providers.notify` does not block). Alternatively, clear `typing` only after the flush, under the lock.

### M2. A button can be left pressed through the queue's overflow or the late-release path
- **Overflow:**
  - `:560` updates `st.held` for every op, but `:562` drops ops once 32 are queued.
  - Example: a drag during typing sends `down`, then moves (30 messages/s), then `up`. The `up` past 32 is dropped, yet `held` says released.
  - The helper presses and never releases, and the end-of-hold release is skipped.
- **Late `up` during typing:**
  - `:548-551` sends `release` at once.
  - The queued `down` is flushed later and stays pressed.
- Fix:
  - On overflow, drop moves but keep `down`/`up`/`click` (or collapse to the latest move).
  - Route the late-path `release` through the queue while `typing`.

### M3. The helper's byte gate and the daemon's byte drop disagree at the boundary
- The helper allows a frame while `inflight_bytes < bytes`, before the frame's size is known (`crates/kelpie-screen/src/control.rs:840-843`).
- The daemon drops video when `unacked + len > bytes` (`screen.rs:456`).
- Near the limit, the next frame the helper allows is dropped by the daemon. That makes a keyframe owed and a stall of at least 1 s (`KEY_GAP`), and the bigger keyframe makes the next overshoot more likely.
- With the deeper window this is now reachable. At 4 Mbps and 300 ms, 12 frames of about 16.7 KB is about 200 KB, against a 200 KB budget. Previously 3 frames were far below 192 KiB.
- Fix: give the daemon headroom (drop only when `unacked` alone exceeds the window, or at window + one frame cap), or make the helper gate on `inflight_bytes + last_frame_bytes <= bytes`.

### M4. Sharpen sends its 256 KiB budget unpaced
- `win/stream.rs:860-874` sends up to `TILE_CELLS_AT_ONCE` tiles every loop iteration until the 256 KiB budget is spent. It ignores `tile_tokens` and the link rate.
- On the links where Sharpen appears (crisp off, about 220-500 kbps), 256 KiB takes about 4-10 s on the wire, ahead of the video in the same FIFO media lane. The picture's age spikes to seconds and the queueing rule cuts the bitrate further.
- This works against the 300 kbps acceptance row (age p95 ≤ 1.5 s) whenever Sharpen is used.
- Fix: pace Sharpen tiles with their own token bucket (for example the link's rate share), or cap the budget at about `kbps/8 × 2 s`.

### M5. `screen_support.start` leaks the test daemon when setup fails after it started
- `ss.start` starts `ps.Daemon` (`scripts/e2e/screen_support.py:270-285`), then can raise. Examples: `SystemExit("tauri-driver did not open its port")`, a `Run(...)` failure.
- `main()` in `screen.py` and `screen_net.py` then still has `d = None`. `finish` stops the daemon only `if d is not None` (`:299-300`), and `procs["daemon"]` is never stopped. `ps.Daemon` has no atexit cleanup.
- The old `screen.py` started the daemon before `try`, so its `finally` always stopped it. This is a regression and leaves an orphan isolated daemon process.
- Fix: in `finish`, stop `procs.get("daemon")` (or set `s.daemon` before anything can fail).

### M6. Test gaps for contracts the spec names
- No daemon test for step 9: a newer connection of the same device `sc_rebind`s while the older connection is still open (the new `connection_opened` → `link_closed`, `mobile.rs:3439-3445`). The existing `connection_opened` test (`mobile.rs:~4103`) was not extended.
- The `sc_text` test (`screen/tests.rs`, `input_that_comes_while_text_is_typed…`) calls `Broker::text_begin` directly. Nothing covers `Conn::reply("sc_text")`:
  - that the reply comes later on `ctl` with the `req` tag;
  - that the error shape is the same as before;
  - that the read loop really does not wait.
- No test for the forced keyframe dip at rung ≥ 7 (`stream.rs:737`). It is Windows-only, but the predicate could be a `Controller` method with a unit test.

## Low

- **L1. The typing flag can stick on a path that is not used in production.** The `typing` flag is set before the future is returned (`screen.rs:627`); only the future clears it. The `phone_verb("sc_text")` path (`:572`) awaits it inline. If that caller's future is dropped (only tests use this path now), `typing` stays true for the hold: every later `sc_input` is queued or dropped and every `sc_text` is refused. Also, `provider::Asked` dropped before `answer` is polled leaves a `waiting` entry, because the `Forget` guard is created only in `answer`. Use a drop guard that clears `typing`, and create `Forget` in `begin`. `typing` is also checked (`:619`) and set (`:627`) under two separate lock acquisitions; one scope would be cleaner.
- **L2. The cursor is now never dropped, which costs up to about 64 kbps.** Up to 30 messages/s (`CURSOR_EVERY` 33 ms), each padded to 256 B. On a 220 kbps rung that is about 25-30 % of the link while the pointer moves. It is counted in `raw`/`wire`, so the encoder adapts, but consider a lower cursor rate at rungs ≥ 7.
- **L3. The countdown is approximate.** The phone counts down from the freeze (`screen.ts:1076-1086`), while the daemon's 45 s starts when it sees the link close (or at replacement), so the countdown is pessimistic. At 0 it keeps showing "0 s more" until a rebind fails.
- **L4. The picture age leaves out the base one-way delay.** It is `serverNow() − (t0 + capture)`, and `serverNow` subtracts the minimum observed lag (offset + minimum one-way delay). This matches the spec formula, but the details line slightly understates how old the picture is. `screen_net.py` measures the true age, on the same machine.
- **L5. The proxy's loss model under-models head-of-line blocking.** `scripts/e2e/net_fault_proxy.py:150`: `wait += self.rto` adds less than `rto` when the piece is already late (`wait < 0`), so a backlog makes later losses lighter. Use `wait = max(wait, 0) + self.rto`. Also, "pieces" are socket reads, not packets, so 1 % of reads is not 1 % packet loss. It is labelled a model; worth saying in the report.
- **L6. `screen_net.py` measurement limits.**
  - `link_lost_after_s` guesses audit timestamp keys (`at/ts/time/when`). Its fallback reads `sc_end`, which the disconnected phone never receives.
  - The emulated phone does not kick, so resume times do not reflect the page's behaviour.
  - `prepare` reads `argv[1:3]` positionally, so options placed before the positionals break it.
- **L7. Docs and locale nits.**
  - `docs/architecture.md:811` (pane takeover) does not say that a newer connection now releases the older connection's takeover holds at once; the same `connection_opened` change applies to takeover.
  - `app/src/locales/vi/screen.json` has mixed line endings (one CRLF). `eol=lf` normalizes it on commit.
- **L8. Sharpen is also offered when crisp text is off because of machine load.** It works whenever `crisp_ok` is false, including L3 or paused for load, where the extra top-off passes and tiles add load. The spec only intends the slow-link case.

## Requirements vs spec

| # | Status | Evidence / note |
|---|---|---|
| 1 Window | Met (see M3) | `control.rs` `Controller::window` formula, clamps, `win` gating, `may_encode(inflight, inflight_bytes, …)`, `lagging`/`tick` use `bandwidth.window`; `Flight` sizes cleared on `ask_key`/`tick`/`restart`, freed via `acked`/`dropped_through`; helper emits `window` on change only when `win` (`stream.rs:553-558`); app passthrough (`app/src-tauri/src/screen.rs:338,387`); daemon clamps to [3,12]×[192 KiB,2 MiB] (`screen.rs` `"window"` arm), `note_window` and the byte rule use `st.window`; `win: true` in flow. |
| 2 Cursor lane | Met (see L2) | `lanes.rs` `CursorSlot` (latest-only, Notify keeps a wake-up); writer order ctl → bulk → cursor → media (`link.rs:404-425`), dropped while locked; `screen.rs:446-452` bypasses `ROOM_FOR_VIDEO`; `MEDIA_CAPACITY` 12; test of a cursor ahead of 6 queued frames. |
| 3 sc_text off the read loop | Met with H2, M1, M2 | `Conn::screen_text` spawns; `Providers::begin/answer`; queue of 32. Ordering of text then input is correct at the helper (app requests are spawned, `app/src-tauri/src/screen.rs:373-380`, so the daemon-side queue is justified). |
| 4 Ladder | Met | `RUNGS`, `rung()` thresholds exact; `fit` relative to `min(src_h,1080)` via `geom::fitted`; the window's 540 floor is kept; tests for 4K → 540, window, 250/180 kbps; forced dip at rung ≥ 7 (`stream.rs:737`, `KEY_DIP_RUNG` = 7 = the 6/50 rung). |
| 5 Sharpen | Met (see M4, L8) | Verb plus permission Control (`verbs.rs`); daemon 3 s gap plus rect validation; helper `tiles::Sharpen` 256 KiB, ignored when `crisp_ok`; `Grid::due_within`; `TopOff::again`; phone button only when `crisp === false` and API ≥ 39. |
| 6 Trend | Met | `QUEUE_LAG_MS` 40, `trend_window_ms` = max(500, 3000/fps), `trend_samples` = clamp(fps/4, 2, 3), `full_for_ms` = clamp(2×base, 500, 1000); the 1275/1083 cut values kept (test moved to a 400 ms RTT). |
| 7 Grace | Met (see H1, L3) | `SCREEN_REBIND_WINDOW` 45 s, takeover keeps 15 s; `connection_opened` releases the older connection's holds; phone kick plus countdown. |
| 8 t0 / details | Met (see L4) | `t0` in the helper start answer and the daemon ack (null from an old app); age p50 per stats second; details line; `inflight_limit` whitelisted in `STAGE_FIELDS`; API 39 plus `SCREEN_FLOW_API`. |
| 9 No plan IDs | Met | No phase/plan references in code, tests or scripts. |

## Verified OK

- **Lock order:** `connection_news` is taken only in `connection_opened`/`connection_closed`. Under it, `takeover.link_closed` and `screen.link_closed` take holds → state (the screen collects first, then `providers.notify` without holds). No path takes these in reverse; `providers.notify` and `tx.send` do not block. `media()` takes the cursor slot lock under `hold.state`, and the writer takes it alone.
- **Cursor wake-ups:** `Notify::notify_one` keeps a permit, a `Notified` dropped by the biased `select!` passes its notification on, and `take()` runs in the same poll as `ready()`. No lost or spurious cursor.
- **Spawned text task:** it holds `Arc<Hold>` and a `ctl` sender. The flush checks `hold.live()`, the reply send is bounded by `SLOW_AFTER`, and the answer is bounded by `provider::TIMEOUT`, so `typing` always clears on the production path.
- **Version skew:**
  - Old app/helper with a new daemon: `win` is dropped by the old app whitelist, so 3 frames; `screen.sharpen` hits `_ => {}`; `t0` is null.
  - New app/helper with an old daemon: no `win`, so no `window` events; `inflight_limit` is dropped by the old whitelist; old `on_event` ignores unknown kinds.
  - Old phone with a new daemon: the `sc_text` reply has the same shape and `req`; extra `t0` is ignored.
  - Field names are kept everywhere.
- **Takeover/screen `link_closed` after replacement:** the older connection's later `end()` is a no-op, because it matches by connection id. A new connection's rebind needs `gone_at`, which is now set at once.
- **Unchanged checks:** `held_here` (`sc_elsewhere`) still guards all screen verbs. `sc_sharpen` needs Control and is refused in `run` like the other screen verbs.
- **Phone:** the Sharpen button is guarded (`crisp === false`, live, API ≥ 39) and its own rate limit is tested. `visibleRect` uses source pixels, matching the helper's `cells_in`. The send button is disabled while sending, so a second text is not normally refused.
- **`screen.py`:** keeps all 42 named checks (set compared against HEAD; the app-connected check moved to `screen_support.start`). Isolation: `ps.isolate(tag)` sets `KELPIE_ENDPOINT=kelpie-<tag>-<pid>` and the data dir before `smoke_support` import or any spawn; `finish` asserts the endpoint prefix before stopping; nothing touches the real daemon.
- **`screen_net.py`:** uses the same isolation and its own tkinter window; it types nothing.
- **Docs:** architecture and user guides match the implemented ladder, window formula, cursor lane, 45 s grace, Sharpen and details line, except the 1 s input claim (H2) and the takeover note (L7). Vietnamese strings are present and i18n:check is clean.

## Recommended order

1. H1: guard the kick against attempts that are still connecting.
2. H2, M1 and M2 together: queue entries with arrival stamps, flush under the lock, keep presses and releases paired.
3. M3: byte-rule headroom.
4. M4: pace Sharpen.
5. M5: stop the daemon in `finish`.
6. M6: add the replacement-rebind and `Conn` `sc_text` tests.

## Unresolved questions

- Should input queued behind a long text be replayed at all after about 1 s, or dropped as before? This is a product call between the "text then Enter" order and the 1 s safety rule.
- Is the Sharpen burst size (256 KiB) meant to be unpaced, given the 300 kbps age target?
