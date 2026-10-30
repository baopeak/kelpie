# Tester report: phase 2b (phone chat send, daemon-owned queue)

Branch `phone/phase2b-chat-send`, HEAD `1db592d`, nothing committed. Tested the working tree as it stands: 97 dirty/untracked paths, including other sessions' edits (`app/src/components/app.tsx`, `store.ts`, settings, others). Results are for the tree, not the 2b diff alone.

Overall: **green on every automated suite the dev listed.** Open: one composer drag-drop flake (F1, failed 1 of 2 runs), three product observations (O1–O3), and the manual Telex/VNI pass not done. No phase 2b blocker found.

## Build status

| Step | Result |
|---|---|
| `pnpm typecheck` | pass |
| `pnpm test` | 51 files, 619 tests pass (13.5 s) |
| `pnpm i18n:check` | 2202 keys, 0 missing |
| `pnpm build:phone` | ok, `index` 117.85 kB (gzip 38.66 kB) |
| `pnpm tauri build --debug --no-bundle` (for GUI e2e) | exit 0 |
| `cargo build -p kelpied` (after `cargo test`) | ok |
| clippy `-p kelpie-proto -p kelpied --all-targets` | 0 warnings |

## Test results: Rust

| Suite | Result |
|---|---|
| `cargo test -p kelpie-proto -p kelpied` (`CARGO_INCREMENTAL=0`) | proto 23 pass, 2 ignored; kelpied 418 pass, 0 fail, 3 ignored |
| `agents::tests::concurrent_hook_settings_are_published_as_complete_json` (dev saw 1 failure) | full run pass; alone 3/3; loop of 20 solo runs 20/20. **Not reproduced.** |

## Test results: e2e (isolated endpoint/data, own daemon or tauri-driver)

| Script | Result | Log |
|---|---|---|
| `mobile.py` run 1 | 70/70, isolation check OK | `scratchpad/e2e-mobile-1.log` |
| `mobile.py` run 2 (back to back) | 70/70, isolation check OK | `scratchpad/e2e-mobile-2.log` |
| `mobile_protocol.py` | 124 PASS, exit 0 | `scratchpad/e2e-mobile-protocol.log` |
| `mobile_resilience.py` (full) | 55/55, exit 0 | `scratchpad/e2e-mobile-resilience.log` |
| `acp_chat.py` | 56/56, exit 0 | `scratchpad/e2e-acp-chat.log` |
| `composer.py` run 1 | **81/85, 4 FAIL** (drag-drop block, see F1) | `scratchpad/e2e-composer.log` |
| `composer.py` run 2 | 85/85, exit 0 | `scratchpad/e2e-composer-rerun.log` |

## Adversarial probes (own probe harness, isolated daemon per group)

Harness: `phone_support` daemon and phone client, a fake desktop window that answers `chat.turn_ready`, a stalled window that never answers, and three chat agents. The agents are the `TURN_AGENT` from `phone_support`, a SILENT agent that logs its stdin, and a SPONT agent that does work on its own (a Working line, optionally a `can_use_tool` permission). No real agents or model calls. Probe results: `scratchpad/probe-results-{A,B,C,D,E}.json`.

| # | Check (your list) | Result | Evidence / note |
|---|---|---|---|
| 1 | Phone send while agent busy waits, then writes once idle | PASS | `[busy]`: queued with `ahead` 0 and 1, nothing written while held; out after END in order; never two turns at once |
| 1b | Send while agent works on its own (Working line, no daemon turn) or waits on a permission | PASS | `[gate:work]`, `[gate:perm]`: written only after the agent's `result`. The gate sees the agent's own classified lines (`chat.rs:524`). |
| 2 | Phone and desktop cancel each other's entries | PASS | `[cross]`: phone cancels a desktop entry, desktop cancels a phone entry, neither written |
| 3 | Limit 20 per chat, 5 per device | PASS | `[limits]`: 6th from one device → `queue_full`; 21st from a fresh device → `queue_full`; exactly 20 accepted; all 20 went out in acceptance order; the phone mirror showed 20 |
| 4 | 16 KiB measured in bytes, multi-byte Vietnamese | PASS | `[bytes]`: 16384 bytes Vietnamese (3-byte chars) accepted; 5462 Vietnamese chars (16386 bytes, under 16384 chars) → `too_long`; 16384 ASCII accepted; 16385 ASCII → `too_long`; blank → `empty`. `[bytes-utf8]`: the 16384-byte Vietnamese message reaches the agent byte-exact, and `kelpie_sent` carries the same text. (First run's failure was harness, see F2.) |
| 5 | Burst rate | PASS | `[burst]`: 10 at once → 3 accepted, 7 `rate_limited`; after 3.2 s idle → 3 accepted, 1 `rate_limited` |
| 6 | Duplicate cid within 10 min | PASS (to 10 min: unit only) | `[dup]`: retry while queued → `dup` with same qid and `queued`; cancelled → `dup` `cancelled`, nothing sent; after it went out → `dup` `sent`; same cid from another device → a new message; agent got each cid once |
| 7 | Codex/ACP refusal: leading whitespace, uppercase, multi-line | PASS | `[refuse:codex]`: refused `/compact`, `  /compact`, `\n/compact`, `\t/review`, `/COMPACT`, `!ls`, `   !ls -la`, `/compact\nand more`, `/Compact now`, ` /new` (all `command_needs_desktop`); `hello /compact` and `Hello\n/compact` accepted. `[refuse:acp]`: refused ` /new  `, `/clear`, `/reset`, `/clear\n`, `/new`, `\t/reset  `; `/NEW`, `/new\nfoo`, `/new project`, `/newer`, `hello /new` sent as text (same rule the desktop applies, `acp-adapter.ts:44-52`) |
| 8 | Revoke during running turn: queued entries go, turn finishes | PASS | `[revoke]`: desktop told, dropped reason `revoked`; the running turn ENDs; the others go out in order. `[revoke-e]`: revoked socket closes with 4401; its waiting message never sent |
| 9 | Forced L3 (`KELPIE_LOAD_FORCE=L3:probe`): chat verbs served | PASS | `[L3]`: `mobile.load` level 3; `chat_send`, `chat_cancel`, `chat_stop`, `chat_resume` all `ack`; queue still went out; 5 sequential sends each under 300 ms (sample of 5, max not recorded) |
| 10 | Daemon restart with a queued entry: phone shows `lost`, text kept | PARTIAL | Daemon side `[restart]`: new epoch; a send with the old epoch → `epoch`; the queued message never went out; old qid → `too_late`. **Phone `lost` UI not exercised in a browser.** Covered only by `chat-send.ts` `epochChanged()` and vitest. |
| 11 | Window closed: phone send still writes | PASS | `[closed]`: phone send written with no window; desktop plain text built by the daemon and written; `mobile_protocol.py` section 8b agrees |
| x1 | Stalled window (never answers `turn_ready`) | PASS | `[stall]`: desktop plain message went out via fallback at about 15 s (window 14–22 s); phone entry behind it went out; stalled rich entry dropped as `needs_window`, never sent |
| x2 | Codex with no thread; ACP with no session | PASS | `[codex]`, `[acp]`: nothing written (no `turn/start`, no `session/prompt`) |
| x3 | 200 KB desktop plain text | PASS | `[big]`: goes out whole; the log copy is at most 64 KiB |
| x4 | Desktop RPCs user-only, 30 s watchdog, stop coalescing, `switching` on hold | not run e2e | Unit tests exist (`auth.rs`, `chat.rs:2150` watchdog, `queue.rs` stop and hold tests) and pass |

Probe counts: group A 36/38 (both fails were harness bugs, fixed and re-run in D/E: 5/5 and 2/2); group B 8/8; group C 7/7. Total final: all adversarial checks pass.

## Failures

**F1. `composer.py` run 1: 4 FAIL in the drag-drop block (not reproduced).**
- Checks: `a dropped picture is attached by its own path` (`None`); `…and a dropped file becomes a mention` (`''`); `…and both reach the program` (`''`); `Up brings back the last prompt` (`'describe the picture'`). The last three follow from the first: the drop never attached.
- Run 2: all of these pass; 85/85.
- Repro: `python -X utf8 scripts/e2e/composer.py <shots> <scratch>` with `MSEDGEDRIVER` set. The drop is emitted as `tauri://drag-drop` at `composer.py:604-610`, handled at `composer.tsx:300-311`.
- Evidence against a phase 2b cause: `composer.tsx`, `app.tsx` drop listener, `lib/mentions.ts`, `Cargo.lock` and `pnpm-lock.yaml` have no diff against HEAD; `@tauri-apps/api` is pinned at 2.12.4 and its `onDragDropEvent` sets `type: 'drop'` itself (`webview.js:687-696`).
- Status: flaky, cause not established. It ran right after `mobile.py`/`acp_chat.py` on the same machine.

**F2. Harness, not product: first `[bytes]` run delivered 38228 bytes for 16384.**
- The agent was started without `-X utf8`, so its stdin decoded UTF-8 as cp1252 (5461 × 7 + 1 = 38228). In the UTF-8 rerun (`[bytes-utf8]`) the agent receives the text byte-exact and the same daemon's `kelpie_sent` carries it intact.
- Recommendation: any VI test should start the agent with `-X utf8`. `phone_support.write_turn_agent` callers do not do that today.

## Observations (product, not failures)

- **O1 (low, code reading, not exercised).** Rate-limit tokens are spent by refusals other than `rate_limited`. `mobile.rs:1038` calls `allow_send` before `enqueue`, so `queue_full`, `empty`, `command_needs_desktop` and `switching` cost a token too. The comment at `mobile.rs:227-228` ("A request that is refused costs nothing") is true only for `rate_limited`.
- **O2 (product question).** A chat restored after a daemon restart is not running (`daemon.rs:902-915` builds `ChatSession::new` without starting the agent). Sends get `exited` ("This chat has stopped.", `queue.rs:137`) until the desktop starts it. `session.restart` refuses chats ("chat sessions cannot be restarted"). The phone shows Put back and Discard, so no text is lost, but nothing on the phone says how to resume.
- **O3 (dev's open question 2).** The daemon forwards Claude TUI-only slash commands as text. Only the phone refuses them, from its own list. A device whose client is not the phone UI can send them. Not tested.

## Coverage and performance

- Coverage: not measured. `AGENTS.md` configures no coverage tool or threshold.
- Test timings: kelpied unit suite 92 s; vitest 13.5 s; `mobile_resilience.py` about 6 min wall time (it has built-in waits of 30–45 s).
- Latency: `chat_send` ack under 300 ms in 5 sequential samples at L3; the 30-minute stress gate and the perf gate were not run.

## Gaps (not run or not covered)

- Manual Telex/VNI on iPhone Safari and Android Chrome (spec "Manual once"). Not done.
- Phone UI states in a real browser: `queued`, `sent`, `unknown`, `lost`, Stop/Resume buttons, read-only and offline. Only vitest (`chat-send.test.ts`) and the `mobile.py` box checks cover this.
- Real agents: `chat.py`, `chat_ui.py`, `chat_parity.py`, `chat_settings_sync.py`, `agent_resume.py` (need signed-in agents). `stress.py` (30 min, 8 phones) and `perf_gate.py` not run.
- The 10-minute dedupe expiry, the 30 s in-flight watchdog and stop coalescing are covered only by unit tests.
- Desktop send-to-stdin p95 ≤15 ms: not measured.

## Recommendations

1. Make `cargo build -p kelpied` part of the e2e preflight. `cargo test` does not relink `target/debug/kelpied.exe`, and I ran a daemon built before the latest `chat.rs` edit until I caught it.
2. Start test agents with `-X utf8` in `phone_support.write_turn_agent` callers, and add a VI byte-exact case to `mobile_protocol.py`.
3. Decide O1: either refund tokens on every refusal, or fix the comment at `mobile.rs:227-228`.
4. Decide O2 before the native shell (phase 5) gates its composer: a restored chat should say how to resume, or the phone should say so.
5. Investigate F1 (run `composer.py` 3 more times; if it stays flaky, add a settle wait before the drop).
6. Investigate `concurrent_hook_settings_are_published_as_complete_json`: the dev's one failure is consistent with a Windows replace-under-reader race in the hook settings write.

## Next steps (priority)

1. F1: 3 more composer runs. Owner: whoever owns `composer.py`.
2. Manual Telex/VNI pass on phones (spec requirement; blocks the phase 2b success criteria).
3. O2 decision, then O1 decision.
4. Stress run (`stress.py`) and perf gate before merge.

## Unresolved questions

- Is there a phone-visible way to resume a restored chat, or is it desktop-only by design (O2)?
- Are the rate-limit tokens for refusals intended (O1)?
- Should the daemon refuse Claude TUI-only commands too, or is the phone's list enough (O3)?
- What made the first `composer.py` run fail its drop checks, and does it recur (F1)?

## Process and environment hygiene

- Started and stopped by me: tauri-driver (PID 1820) and its msedgedriver (PID 6872), stopped with `taskkill /T /F`; the GUI-test daemon (PID 5720, endpoint `kelpie-p2b-gui-probe`), shut down with `daemon.shutdown`. Probe daemons were stopped by their harness. `mobile.py` ran against my tauri-driver (stopped above). `composer.py` and `acp_chat.py` tore down their own drivers and apps.
- User's processes at the end: app `kelpie.exe` PID 8836 started 2026-10-29 15:20:31; daemon `kelpied.exe` PID 10588 started 15:20:33. Both unchanged.
- Temp: probe folders `kelpie-p2b-probe-*` and `kelpie-p2b-gui-*` removed; probe scripts deleted. Protected `%TEMP%\kelpie-p2-acp-scratch-6u2_xg7q` not touched. Logs and probe result JSON kept in the session scratchpad.
- Disk: D: 7.8 GB free after the builds (was 8.3 GB). No extra target dirs. `build-release.sh` not run.

Status: DONE_WITH_CONCERNS

Summary: phase 2b passes every listed suite and all adversarial checks; `mobile.py` 70/70 ×2, `mobile_protocol` 124/124, `mobile_resilience` 55/55, `acp_chat` 56/56, `composer` 85/85 on rerun (81/85 on the first run, F1). Open: F1 flake, O1/O2/O3 product questions, manual IME pass not done.

Concerns/Blockers: F1 is unexplained and only reproduced once; the phone's `lost` UI state and the manual Telex/VNI pass are not verified end-to-end.

## Retest after fix round 1

**Result: BLOCKED. Nothing in this section was run.** The Bash tool returns no output and runs nothing in this session. Checked: `echo`, `date`, a background command that writes a file (exit 0, file not created), a fresh delegated agent (same failure), and a retry after a 2-minute wait. Read, Grep, Glob and Write still work, so only static checks were possible. Not run, so no pass or fail is claimed for any item below.

### Static presence check (Grep only, not a behaviour test)

| Item | Found in code |
|---|---|
| W6 `enqueue_if` re-checks permission under the chat lock | `crates/kelpied/src/chat.rs:847` |
| W7 stuck-turn rule (60 s silence tick test) | `chat.rs:2164` (test calls `tick` at +60 s) |
| W2 `client_gone` and `kelpie_unsent` line | `chat.rs:1003`, `chat.rs:765` (lasting note test `chat.rs:2226`) |
| O1 `refund_send` after a refusal | `crates/kelpied/src/mobile.rs:230` |
| W4 `cancelled_by` and phone "Cancelled on desktop/phone" | `app/phone/src/composer.tsx`, `chat-send.ts`, `phone-state.ts`, `locales/vi/phone.json` |
| W5 Retry keeps text (`putBack`) | `app/phone/src/` (grep hit) |
| O2 "Open this chat on the desktop" | `app/phone/src/composer.tsx`, `chat-send.ts`, `locales/vi/phone.json` |

Presence is not correctness. The dev's own hand walk-through is the only evidence for behaviour.

### Still to run, in this order, once the shell works

1. Prechecks: `pnpm typecheck && pnpm test && pnpm i18n:check && pnpm build:phone`; `cargo build -p kelpied`, confirm `kelpied.exe` is newer than `chat.rs`, `queue.rs`, `mobile.rs`. The desktop app `pnpm tauri build --debug --no-bundle` also has to be redone, since the frontend changed.
2. Process check first: list `kelpie-p2b*` endpoints and ports 1427, 4444, 9515 and leftover `kelpied` or `tauri-driver` from round 0. Round 0 said none were left, but that was not verified. Do not touch PID 10588, PID 8836, Python PID 16936 and its scratch folder, or any `cargo` or `rustc` not started by me (another session runs `cargo test -p kelpie`, parent pwsh 14512).
3. E2E: `mobile.py` run twice back to back (isolated endpoint and data dir in its own process, tauri-driver started with the same values); `mobile_protocol.py`; `mobile_resilience.py` (full); `acp_chat.py`; `composer.py`. If the drag-drop block fails again, rerun once and say whether the same checks also fail on master code paths.
4. Adversarial re-runs: W6 (revoke racing a send leaves nothing queued), W7 (a stuck `Working` turn with no output for more than 60 s releases the queue; a real in-flight turn or a pending question is never released), W4 (a cancel by the other side shows on the phone with its text kept), W5 (Retry refused keeps the text), O1 (a refused send spends no rate token), O2 (a send to a non-running restored chat shows the "open on desktop" hint), and `needs_window` (a rich entry is held while the app is connected, then on disconnect is dropped with a lasting `kelpie_unsent` line).

### Process hygiene for this section

I started no process in this session: the background shell calls created no processes, and the blocked calls ran nothing. I stopped nothing and killed nothing. The round-0 leftovers were not checked (process listing is blocked).

Status: BLOCKED
Summary: fix round 1 is only statically checked (item names are in code and tests). None of the e2e, probes, or Rust/frontend reruns could run because the Bash tool produces no output in this session.
Concerns/Blockers: need a working shell to run anything. Every result for fix round 1 is still unverified, and the round-0 leftover processes are unchecked.

## Retest after fix round 1 (PowerShell)

Run 30/10 09:00–10:00 through the PowerShell tool. Same dirty tree on `phone/phase2b-chat-send`, nothing committed, no product code edited. Every daemon, app and probe ran on its own `KELPIE_ENDPOINT` (`kelpie-p2b-r1-*` and the scripts' own names) and its own temp `KELPIE_DATA_DIR`, set inside its own process. No real agents and no model calls.

**Result: the daemon fix items all pass. Two phone UI bugs found:**
- **B1:** a chip on screen never changes state, so Retry never appears.
- **B2:** the "lost after restart" chip can never be shown.

`mobile.py` was not clean in any of its 3 runs.

Host conditions: the host was loaded during this retest.
- A user `du` process held most of the CPU, which was at 84%.
- Free memory was about 6%.
- A daemon that was not pinned measured itself at L3 (`reason: memory`). This slowed pairing to one every 5 s.

### Preflight

| Step | Result |
|---|---|
| Leftovers from earlier rounds | None. No `kelpied` other than 10588, and no `tauri-driver`, `msedgedriver` or vite/node on 1427, 4444, 4445, 9515 or 47310–47340. The ports in 49xxx/51xxx belong to the Antigravity IDE (not touched). |
| `kelpied.exe` freshness | **Stale.** The binary was built at 08:47:57 but `chat.rs` changed at 08:54:46. Rebuilt with `cargo build -p kelpied` (`CARGO_INCREMENTAL=0`) at 09:03. The phone bundle in `app/dist-phone` (08:45) is newer than every phone source file (newest 08:30), so the embedded phone client is current. |
| Desktop app | `pnpm tauri build --debug --no-bundle`, exit 0, `kelpie.exe` built 09:09. The old `app/dist` was from 06:23. |
| `cargo test -p kelpied` on the current source | 432 passed, 0 failed, 3 ignored (130 s). This confirms the reported count against the 08:54 `chat.rs`. |

### E2E results

| Script | Result | Log (scratchpad) |
|---|---|---|
| `mobile.py` run 1 | **53 pass, 1 fail.** It stopped at line 456: "to cancel on the phone" never reached the agent (trace: `…STOP, END a long job, ARRIVE to be cancelled here`), and the chip lookup then threw. See F3. Isolation check OK. | `e2e-mobile-1.log` |
| `mobile.py` run 2 (back to back) | **2 checks failed** (`quick keys (Ctrl+C) reach the terminal`, F4; `after the computer restarted … lost`, B2), plus `scenario ran to completion`. Cancel note passed, as did "Cancelled on desktop" and Discard. Isolation check OK. | `e2e-mobile-2.log` |
| `mobile.py` run 3 (extra, to characterise the flakes) | **71 pass, 1 fail.** Every check before the restart scenario passed, including the Ctrl+C keys and the cancel block. The restart scenario could not pair: HTTP 429 (T1). | `e2e-mobile-3.log` |
| `mobile_protocol.py` | **126/126.** Includes the picture held with `needs_window` and cancelled by the phone, and the Vietnamese message arriving byte for byte. | `e2e-mobile-protocol.log` |
| `mobile_resilience.py` (full) | **55/55** | `e2e-mobile-resilience.log` |
| `acp_chat.py` | **56/56** | `e2e-acp_chat.log` |
| `composer.py` | **85/85 on the first run.** The drag-drop block passed, so no rerun was needed. F1 did not recur. | `e2e-composer.log` |

### Adversarial probes

**Daemon probes (39/39):** own daemon `kelpie-p2b-r1-probe-<pid>`, phone clients from `phone_support`, and a desktop client over the pipe that answers or ignores `chat.turn_ready`. Results are in `probe-results-r1.json` and `probe-r1.log`.

**Browser probes:** headless Edge with phone emulation, msedgedriver on 9531, against their own daemon pinned at `L0:calm`. Results are in `probe-results-r1-browser.json` and `probe-r1-browser.log`; the screenshot is `browser-after-restart.png`.

| Item | Result | Evidence |
|---|---|---|
| **W6** revoke racing `chat_send` | PASS | 20 rounds; in each, 3 sends were fired while `mobile.revoke` ran 0–8 ms apart.<br>- Nothing from a revoked device was left queued, and none reached the agent.<br>- Of the 60 sends, 14 were acked `ok` and 46 got no reply (the socket closed with 4401).<br>- 20 accepted entries were dropped with reason `revoked`.<br>- The exact window (permission checked, then flipped before the lock) was not hit on purpose. The unit test covers it. |
| **W7** stuck "working" | PASS | - A spontaneous `assistant` line with no flight: the queued send went out 60.1 s after the last output.<br>- Each of these stayed queued for all 628 s: our in-flight turn, a raw turn line written via `session.input`, and an open `can_use_tool` question (status `waiting`).<br>- Both released turns' messages went out as soon as those turns ended. |
| **W4** cancel by the other side | PASS (daemon); the phone UI is in `mobile.py` | - Desktop cancels a phone's message: the phone's `chat_queue.dropped` has `reason: cancelled`, `cancelled_by: desktop`, the text and `mine: true`.<br>- Phone cancels a desktop message: the desktop's `chat.queue.dropped` has `cancelled_by.name` set to the phone's name, and the text.<br>- A self-cancel from either side is silent.<br>- `mobile.py` runs 1–3: "Cancelled on desktop" with the text kept, then Discard. The desktop's own "Not sent (cancelled from …)" note is checked by vitest only. |
| **W5** a refused Retry keeps the text | **Logic PASS; UI FAIL (B1)** | In Edge, a page-side patch dropped the `chat_send` frame, so the send resolved `unknown` after 30.0 s. **The chip on screen still read "Sending…", with no Retry.** After leaving and reopening the chat, it showed "Not sure it was sent" with Retry. With 5 messages from that phone waiting, Retry was refused with `queue_full`: the text went back into the box with "Too many messages are waiting…", and the agent never got it. |
| **O1** refusals spend no token | PASS | - A full bucket survived 4 refusals in a row (empty ×2, `command_needs_desktop` on ACP `/new`, `too_long`): 3 valid sends were accepted and the 4th was `rate_limited`.<br>- 3 `queue_full` refusals did the same: after cancelling 3, a burst of 3 was accepted and the 4th was `rate_limited`.<br>- A fresh device sending 10 at once got 3 accepted and 7 `rate_limited`. |
| **O2** restored chat that is not running | PASS | - After a daemon restart the row is `alive: false`, `status: exited`.<br>- `chat_send` returns `exited` with "This chat is not running. Open this chat on the desktop to resume it."<br>- The phone (Edge) shows "This chat is not running. Open it on the desktop to resume it." in place of the box. |
| **needs_window** | PASS | - A rich desktop entry whose client detached stays at the head with `needs_window: true`, visible in the phone's `chat_queue`. The phone message behind it waits, and the phone can cancel it, after which the queue goes on.<br>- Closing the client's connection drops it with reason `needs_window` and writes a `kelpie_unsent` line. The phone sees that line live, and a later `session.attach` replays it (it lasts).<br>- A client that is attached but never answers: `needs_window` shows 15 s after the offer, the entry is offered again 30.4 s after the first offer, and answering then writes it. |
| **"Not now" retry** | PASS (bounded) | Over 30 s with `{lines: null, retry: true}`:<br>- 24 offers, gaps 1.01–1.25 s.<br>- Nothing was written to the agent or the chat log, and the entry was neither sent nor dropped.<br>- The data folder grew by 0 bytes.<br>- It was written as soon as the window answered.<br>- Plain text declined with retry was written by the daemon 15 s after the first offer.<br>- Event traffic: 26 `chat.queue` events to the desktop and 27 `chat_queue` messages to the phone in 30 s (O5). |
| Replay of the run-1 sequence (Stop, desktop cancel, Discard, send) | PASS | In Edge the send right after Discard reached the agent, so F3 was not reproduced. |

### Failures with repro

**B1 (product, phone UI, medium): a chip on screen never updates its state, so Retry never appears.**
- **Code:**
  - `ChatSender` changes `PendingSend` objects in place (`chat-send.ts:123, 142-144, 153, 163, 171, 188-202, 231`).
  - `chatPending` is a plain `createSignal` over those objects (`phone-state.ts:34, 256`).
  - `ChatComposer` renders them with `<For each={onTheWay()}>` (`composer.tsx:291-305`). `<For>` keeps rows by object identity.
  - `stateText(p)`, `classList={{warn: p.state…}}` and `<Show when={p.state === "unknown"}>` read no signal, so a row drawn as "Sending…" keeps that text and those buttons.
- **Effects:**
  - `sending` to `unknown`: no "Not sure it was sent", no Retry and no Discard until the chat view is mounted again. This makes the W5 fix unreachable in normal use.
  - `sending` to `sent`: shows "Sending…" until the log line removes it (cosmetic).
  - A Retry going from `unknown` to `sending` does not show "Sending…".
  - Rows that leave the list and come back (queued, then cancelled or lost) are new rows, which is why `mobile.py`'s "Cancelled on desktop" passes.
- vitest tests `ChatSender` only, not the component.
- **Repro (headless Edge, own daemon):**
  1. Pair a phone and open a chat whose agent is busy.
  2. In the page, run `WebSocket.prototype.send` patched to drop strings containing `"chat_send"`.
  3. Type a message and tap Send.
  4. After 30 s Send is enabled again and the box is empty, but the chip still reads "Sending…".
  5. Tap back, then open the chat again: the chip now reads "Not sure it was sent", with Retry and Discard.
- **Fix direction:** replace the object (`this.list = this.list.map(x => x.cid === cid ? {...x, state} : x)`) or use a store, and test it with a component-level check.

**B2 (product, phone UI, medium): after a daemon restart, "what waited is shown as lost, with its text" can never be shown.**
- `epochChanged()` marks the messages `lost` (`phone-state.ts:85`, `chat-send.ts:229-232`), but every chat comes back `alive: false` after a restart (O2).
- `chat-view.tsx:176-181` mounts `ChatComposer`, which holds the chips, only when `props.row.alive`. Otherwise it shows the read-only line.
- So the lost chip, with Put back and Discard, is never on screen. The text lives only in the page's memory, and a reload drops it without a word. That breaks "text is never dropped silently".
- **Repro:**
  1. In a chat, keep the agent busy and queue a message from the phone.
  2. Restart the daemon (`daemon.shutdown {kill_sessions: true}`, then start it).
  3. The phone reconnects and shows "This chat is not running. Open it on the desktop to resume it."
  4. There is no composer and no chips (`browser-after-restart.png`; the row reads `alive: false, status: exited`).
- **Fix direction:** render the pending chips (lost, unknown, cancelled) outside the `alive` gate, or above the read-only line.
- `mobile.py` lines 563-568 expect the chip and will fail every time they are reached.

**T1 (test, `mobile.py`): the restart scenario pairs too soon after the deliberate bad-code lockout.**
- Line 546 calls `ps.pair` a few seconds after "repeated bad codes are rate limited" (10 per minute per address). Whether the 429 hits depends on how long the steps in between take. Run 2 got through; run 3 failed with `HTTP Error 429: Too Many Requests`.
- Even when pairing works, B2 makes the next check fail.
- **Fix direction:** move the scenario before the bad-code checks, or wait until the limiter window has passed.

**F3 (flake, not reproduced): `mobile.py` run 1, line 449-456.**
- "to cancel on the phone" never reached the daemon. The next message went out at once, and the run then threw at line 456.
- The cancel block passed in runs 2 and 3, and the exact sequence passed in the Edge replay.
- Most likely cause: the Send tap was ignored, because Send was disabled (connection blip, or an earlier send still in flight) or a notice was shown. The script does not record the notice or whether the button was disabled. Not established.
- The script does not wait for Send to be enabled again before the next tap, unlike the probe's `settled()`.

**F4 (flake, unrelated to 2b): `mobile.py` run 2, `quick keys (Ctrl+C) reach the terminal`.** It passed in runs 1 and 3, and it is a terminal path. The host was loaded.

### Observations (no failure)

- **O4, re-offer cadence.** An attached window that does not answer sees `needs_window` 15 s after the offer, and the next offer comes 15 s after that, 30 s after the first. The fix report says "every 15 s". Either the docs or the constants should say 30 s.
- **O5, chatty "not now".** `decline_turn` announces `chat.queue` on every decline (`chat.rs:996-997`), even though the shape does not change. That is about one event a second to every attached client, and one coalesced `chat_queue` a second to each phone, for as long as the window keeps declining. It is bounded and writes nothing to disk, but `announce_if(before, …)` would make it silent.
- **W6 coverage.** Every revoke closed the socket before most replies, so the `read_only` refusal from `enqueue_if` was never seen end to end. The invariant held. The precise interleaving is covered only by the Rust test.

### Gaps

- The desktop's own "Not sent (cancelled from {device})" note and its `kelpie_unsent` note: vitest only. No GUI check was added.
- W5 in normal UI use is blocked by B1. The refusal logic was verified only after re-mounting the view.
- The manual Telex/VNI pass on real phones, `stress.py`, the perf gate, and the real-agent scripts (`chat.py`, `chat_ui.py`, `chat_parity.py`, `chat_settings_sync.py`, `agent_resume.py`) were not run.
- The 10-minute dedupe expiry, the 30 s in-flight watchdog and stop coalescing: unit tests only.

### Cleanup

- **Started and stopped by me:**
  - `mobile.py` runs 1–3: tauri-driver killed as a tree, isolated daemon shut down by its endpoint, no leftovers listed after each run.
  - The probe daemons and the browser probe's msedgedriver and Edge, stopped by their harnesses.
  - The suites stopped their own daemons and drivers.
  - At the end no `kelpied` other than 10588 was running, and no `tauri-driver`, `msedgedriver` or test Python.
- **Not touched:** PID 10588 `kelpied` (still running, started 29/10/2026 15:20:32), app 8836 (started 15:20:30), Python 16936 and `%TEMP%\kelpie-p2-acp-scratch-6u2_xg7q`, the user's `du`, the IDE processes, and `http.server` 2836.
- **Temp:**
  - Removed my 11 `%TEMP%\kelpie-p2b-r1-*` folders and the two `kelpie-phone-protocol-*` folders from my run.
  - Left alone: `kelpie-cp-*`, `kelpie-auto-*`, `kelpie-notes-other-*` and `kelpie-phone-<uuid>` (created 09:24–09:25). Their names look like another session's unit tests and I cannot attribute them to my runs.
  - Deleted my probe and runner scripts and the scratch work folders, including the Edge profiles.
  - Logs, result JSON and screenshots are kept in the session scratchpad.
- **Disk:** C: 3.94 GB free, D: 7.66 GB free. No extra target dirs. `build-release.sh` not run.

Status: DONE_WITH_CONCERNS
Summary: every daemon-side fix from round 1 holds in e2e and adversarial probes (W4, W6, W7, O1, O2, needs_window, bounded "not now"). `mobile_protocol` 126/126, `mobile_resilience` 55/55, `acp_chat` 56/56, `composer` 85/85, `cargo test -p kelpied` 432 pass. `mobile.py` was not clean in any of its 3 runs.
Concerns/Blockers: B1 (phone chips never re-render their state, so Retry and W5 are unreachable without a re-mount) and B2 (the post-restart lost chip is never shown, because a restored chat has no composer) are product bugs that need fixing. `mobile.py`'s new restart scenario also has a pairing-429 ordering problem (T1). F3 is not explained.
