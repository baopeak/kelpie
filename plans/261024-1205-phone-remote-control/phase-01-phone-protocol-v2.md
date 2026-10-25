---
phase: 1
title: "Phone protocol v2: state-sync, prompts, safe answers"
status: pending
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

## Related Code Files
- Create: `crates/kelpied/src/mobile/sync.rs`, `crates/kelpied/src/mobile/verbs.rs`
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
