---
phase: 1
title: "Phone protocol v2: state-sync, prompts, safe answers"
status: completed
priority: P1
effort: "4d"
dependencies: []
---

# Phase 1: Phone protocol v2

## Overview
Replace the phone's poll-and-replace protocol with push-based state-sync that survives flaky 4G, a closed verb table with a
minimum permission per verb, prompt answers that cannot double-fire, and chat sessions the phone can watch and answer without
becoming a second protocol client. Daemon side, plus the IME fix in today's page (replaced in phase 2).
Starts after roadmap phase 12 is committed (it edits `mobile.rs`, `notify.rs`).

## Requirements
- Functional
  - **Addressing by session id** (`spec.id`), not `handle`: handles restart at 1 and are reassigned on daemon restart (`daemon.rs:182`, `:720-721`, `:767`). [RT-8]
  - `hello` carries `v: 2`, `daemon_epoch` (random per daemon start), session list `rev`. List changes are pushed: each phone connection registers as a
    daemon client (`register_client`, `daemon.rs:664`) through an adapter that decodes the broadcast, replacing the 1 s poll (`mobile.rs:492-508`). [RT-7]
  - **Verb table** in `mobile.rs` — closed list, minimum permission each, unknown verbs refused: `list, sub, unsub, chat_sub` = read; `input, key, answer, chat_answer` = write;
    screen/takeover verbs (phases 7–8) = control. No generic daemon-method proxy; `session.update`/`session.attach`/admin methods are never reachable (`auth.rs:248`). [RT-6]
  - `sub {id, height}` → `screen` frames `{id, rev, base, rows: [[index, text]], height, cursor, prompt, waiting_since, title}`; ≤ 1 per 100 ms per session; only changed rows.
    **Wake source**: a per-`Session` `tokio::sync::watch<u64>` of output `seq`, created in `Session::new`, bumped in `ingest` (`session.rs:432`); rows rendered from a
    snapshot taken under the lock, diffed outside it. Slow sockets get latest-state-only (never an unbounded queue). [RT-7]
  - **Resume**: last-sent rows kept per `(device, session id)` for 5 min so `base` works across reconnects and relay↔direct migration; `daemon_epoch` change ⇒ client drops `rev`/`seq`,
    re-resolves ids, discards pending writes (never retries them). [RT-7][RT-8]
  - **Answer guard fix**: `Session::answer` also refuses when the wait was already answered — new `answered_wait` set under the same lock by `answer` *and* by any `input`
    (desktop keypress included). Today hook-driven `Waiting` survives input (`status.rs:110-121`) and `waiting_since` updates only on the 250 ms tick (`session.rs:797-802`). [RT-5]
  - **One typed-input path**: phone `input`/`answer` go through a shared daemon function extracted from the `session.answer` arm (`daemon.rs:1498-1520`): `typed_text` sanitising
    (`session.rs:856`) and the agent's Enter delay (Codex needs > 120 ms; desktop uses 250 ms, `app/src/lib/quick-commands.ts:31-43`). `input` refused while a prompt is shown unless `force`. [RT-6]
  - `key` table extended: pgup, pgdn, home, end, left, right, shift-tab, ctrl-d, ctrl-z, ctrl-l, ctrl-r, backspace, space, `ctrl+<letter>`.
  - **Chat**: `chat_sub {id, since_seq?}` = observer attach that does **not** set `focused`/clear `unread` (`chat.rs:283-284`); delta mode when `since_seq` is still in the log, else reset capped
    at the last 2 000 lines / 2 MB **plus all pinned lines** (`chat.rs:28-39`, `:274-276`). [RT-4]
  - **Chat answers**: `chat_answer {id, request_id, line}` — the phone's adapter builds the reply line; `chat.rs` records answered request ids from every outgoing stdin line
    (JSON-RPC response `id`; Claude `control_response.response.request_id`), refuses a second answer for the same id (desktop or phone, first wins), broadcasts `chat.resolved {id, request_id}`
    to all attached clients. Desktop writes go through the same check. [RT-4]
  - Pairing limiter per source IP via axum `ConnectInfo` (peer address is discarded today, `mobile.rs:341`).
  - Deep link `#s=<session>` from `notify.rs:364` keeps working (phase 2 carries it). [RT-9]
  - Old page: Enter ignores IME composition (`e.isComposing || e.keyCode === 229`) at `crates/kelpied/mobile/app.js:231`.
  - `API_LEVEL` bump (`crates/kelpie-proto/src/lib.rs:24`); app gates the new UI on `daemon.info.api` like `ANSWER_API`. [RT-9]
- Non-functional: no timers/diffs for unsubscribed sessions; nothing when phone access is off; frames ≤ 64 KiB; WS cap 256 KiB unchanged.

## Resilience addendum — unstable networks and host overload (second commit of this phase)
<!-- Updated: 2026-10-25 - resilience research: researcher-261025-1744-unstable-network-resilience.md, researcher-261025-1744-cpu-overload-resilience.md -->
Implemented after the protocol-v2 commit, reviewed and committed separately. Numbers are starting values; the stress/network harness tunes them.
- **Heartbeat** (verb `hb`, read permission, both directions; any inbound frame resets idle): send after 20 s idle, pong deadline 8 s, daemon closes a phone after 45 s of silence; server `SO_KEEPALIVE` 30/10/3 as backstop. Today there is no ping and `ws.recv()` has no read timeout (`mobile.rs`, scout 2026-10-25).
- **Lag-aware liveness**: a peer deadline only counts if the daemon's own lag probe was healthy in that window; a tick gap > 2× interval (stall, sleep) resets all deadlines; at load level L2+ intervals double (40 s / 90 s). Gate: 0 false disconnects in 30 min of all-core stress.
- **Legacy pages** (fix round 1): a client whose first message lacks `caps: ["ack","hb"]` runs without the ack window and without a heartbeat deadline; the bounded queues and the write timeout stay.
- **Ack window**: phone sends `ack {id, rev}` every 2 frames or 250 ms (the original "500 ms or 8 frames" cannot work with a window of 4); daemon keeps ≤ 4 unacked frames / 128 KiB per (device, session), beyond that coalesces to latest-state-only; diff base = last **acked** rev; `sub {id, have_rev}` resumes from it. Test: 20 KB/s throttled socket with 1 MB/s output ⇒ flat daemon RSS, convergence ≤ 2 s after unthrottle, no frame > 64 KiB; 100 mid-stream TCP kills ⇒ rows identical after every reconnect.
- **One writer, one bulk lane** (fix rounds 1-3: a separate control channel had no producer and was removed): the writer sends `hb`, acks, answer results, revoke/stop, errors and `load.status` itself, ahead of the bulk lane (`biased` select); bulk (bounded 4 × 64 KiB) = latest-state frames and everything about a chat in FIFO order (a `chat_resolved` is queued after its lines); bulk without room for 10 s ⇒ close with `slow_consumer`. No `unbounded_channel` anywhere on a phone path (CI grep gate) — the daemon-event channel per connection is unbounded today.
- **Answer results**: `ok | stale {answered_by: desktop|<device name>, at} | dup | refused`; the daemon keeps the last 20 resolved waits per session for 10 min so a late tap from a push says "already answered on the desktop 2 min ago".
- **Resume cache bounds**: ≤ 64 KiB per (device, session), ≤ 8 devices × 20 sessions, global LRU 16 MiB, TTL 5 min; cleared when L3 is caused by memory (free memory < 8 % or the low-memory event); kept when L3 comes from CPU or event-loop lag, since clearing frees almost nothing and forces every reconnecting phone to reload the whole screen.
- **Host load levels** — new `crates/kelpied/src/load.rs`, started only while phone access is on: lag probe (100 ms interval, p95 over 5 s, `MissedTickBehavior::Delay`), system CPU 1 Hz (`GetSystemTimes` / `host_statistics`), available memory + low-memory event (`CreateMemoryResourceNotification` / memory-pressure source), battery saver and thermal as raise-only inputs. Levels L0–L3 with hysteresis (a rise needs 2 consecutive 1 s samples; hysteresis, 10 s below and ≤ 2 changes/min hold back falls only): L1 lag p95 > 30 ms or CPU > 85 %; L2 lag > 75 ms or CPU > 95 % for 3 s or free memory < 15 %; L3 lag > 200 ms or free memory < 8 %. Exposed as `watch<Level>` + reason set; every later phase only reads it. Test hook `KELPIE_LOAD_FORCE=L2:thermal`.
- **Shedding ladder** (rates per phone): focused session 100/100/250/1000 ms (L0/L1/L2/L3); other subscribed sessions 100/250/1000 ms/paused; max diff frame 64/64/32/16 KiB; heavy-TUI cap 5 fps → 1 fps at L3; chat resets ≤ 2 concurrent at L2, 1 at L3; pairing handshakes 1/s at L2, 1/5 s at L3. **Never shed**: `input`, `key`, `answer`, `chat_answer`, revoke/downgrade, stop, `hb`/acks, attention events, `load.status`, device-change toasts — a full control lane is an explicit error, never a silent drop. A single slow phone goes latest-only without raising the global level.
- **`load.status {level, reason[], since}`** on change and every 1 s while level > 0; every `screen` frame carries `server_ts` and `level` so the phone can show data age.
- **Keep the daemon schedulable**: opt `kelpied` out of Windows EcoQoS power throttling (`SetProcessInformation(ProcessPowerThrottling)`) and hold a macOS `NSActivityUserInitiatedAllowingIdleSystemSleep` activity while a phone is connected (no App Nap; user decision 2026-10-25: keeping the computer awake stays with the existing keep-awake setting, so idle sleep is allowed); never raise the process class (children would inherit); never HIGH/REALTIME.
- **Watchdog**: a plain OS thread checks the lag probe's heartbeat atomic; runtime silent > 5 s ⇒ log "runtime stalled" (runtime counters and the busiest threads; not backtraces); > 30 s ⇒ the `daemon.unresponsive` event to the app (dotted like the daemon's other events; the code's name wins over `daemon_unresponsive`); never kills agent PTYs.
- **Runtime split decision**: if lag p99 > 30 ms at the phase-1 perf-gate load without external stress, move WS accept / Noise / heartbeats / sync send to a 2-thread `kelpie-link` runtime at `THREAD_PRIORITY_ABOVE_NORMAL`; otherwise keep one runtime.
- **Stress and network harness** (reused by later phases): `scripts/e2e/stress.py` (N busy threads at configurable duty and priority, 20 child processes profile), a TCP fault proxy (toxiproxy or an in-repo Python proxy: latency, bandwidth, blackhole 10/30/90 s, half-open, RST storm) between test phone and daemon; always an isolated daemon (`KELPIE_ENDPOINT` in the script's own process).
- **Gates**: under all-core 100 % stress with 20 sessions and 8 phones for 30 min — 0 false disconnects; `input` echo p95 ≤ 300 ms, p99 ≤ 800 ms; revoke stops writes < 1 s; 0 dropped `answer/input/key`; RSS growth ≤ 10 %; level reaches L2 ≤ 5 s after stress starts and L0 ≤ 25 s after it stops; feature off ⇒ probe not running.

## Related Code Files
- Create: `crates/kelpied/src/mobile/sync.rs`, `crates/kelpied/src/mobile/verbs.rs`; addendum: `crates/kelpied/src/load.rs`, `scripts/e2e/stress.py`, `scripts/e2e/net_fault_proxy.py` (or toxiproxy wrapper), `scripts/e2e/mobile_resilience.py`
- Modify: `crates/kelpied/src/mobile.rs`, `crates/kelpied/src/session.rs` (watch, `answered_wait`, snapshot helper, shared typed-input fn), `crates/kelpied/src/daemon.rs` (use shared fn in `session.answer`), `crates/kelpied/src/chat.rs` (observer attach, delta, pinned cap, answered ids, `chat.resolved`), `crates/kelpie-proto/src/lib.rs` (`API_LEVEL`), `app/src/chat/chat-model.ts` (handle `chat.resolved`), `crates/kelpied/mobile/app.js` (IME guard), `scripts/e2e/mobile.py`, `scripts/e2e/attention.py`, `docs/architecture.md`

## Implementation Steps
1. IME guard (`app.js:231`) + e2e composition check.
2. `answered_wait` + shared typed-input function; desktop `session.answer` and phone use it.
3. Verb table + permission tests.
4. Session watch, list adapter via `register_client`, `sync.rs` (snapshot, diff, latest-only, per-device resume cache), id addressing, epoch.
5. Chat observer attach, delta, pinned cap, answered-id tracking, `chat.resolved`.
6. Per-IP limiter; `API_LEVEL`; docs.

## Tests
- `cargo test -p kelpied`: row diff; coalescing bound; epoch reset; resume across connections; `answered_wait` (hook wait answered by desktop input ⇒ phone answer stale; double tap ⇒ second stale);
  verb matrix (every verb refused below its permission; unknown verb refused); `typed_text` applied to phone input (bracketed-paste break-out stripped); chat delta/reset keeps pinned lines; duplicate chat answer refused.
- e2e `mobile.py` (own `KELPIE_ENDPOINT`): push without polling; prompt answer ok then stale; hook-reporting fake agent case; chat watched from phone does not clear desktop unread; phone answers chat permission, desktop card resolves; daemon restart ⇒ new epoch, ids re-resolved.

## Perf gate
- 3 sessions at 1 MB/s with **8 phones** subscribed: daemon CPU ≤ 5 % of one core; PTY ingest throughput within 5 % of no-phone baseline; 0 frames for unsubscribed sessions.

## Success Criteria
- [ ] Phone view updates within 150 ms on LAN without polling.
- [ ] Prompt and chat permission answers are first-wins across phone and desktop; stale answers refused for hook and screen prompts.
- [ ] A phone can never reach admin daemon methods.

## Risk Assessment
- Full-redraw TUIs make large diffs; signal: avg frame > 8 KiB; response: 5 fps cap for that session.
- Generic answered-id extraction misses an adapter's reply shape; signal: e2e duplicate-answer test per adapter (claude, codex, acp); response: per-adapter extractor table.
