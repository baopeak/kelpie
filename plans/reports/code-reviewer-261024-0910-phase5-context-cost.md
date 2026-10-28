# Code review: phase 5 "Context, tokens & cost"

Date 2026-10-24. Reviewer: code-reviewer. Read-only (only this report written). Scope: the phase-5 files listed in the task (uncommitted on `rebrand/kelpie`). Phase-4 ACP and `/clear` work in the same tree was ignored.

## Verdict

**Not ready to ship.** The plumbing, identity mapping, degrade path and UI are solid and every check passes. But on real data the session and Usage-page numbers miss about **47 % of Claude spend**: sub-agent transcripts are never read. And once they are read, the current dedupe would under-count them (see H1/H2). That breaks "Numbers equal fixture truth" for any session that uses sub-agents. The fixtures still pass because they model a `isSidechain`-in-main-file format that this machine's transcripts no longer use. Fix H1, H2, M1 and M2 before landing. The rest can follow.

## Checks run

| Check | Result |
|---|---|
| `cargo test -p kelpie -p kelpied -p kelpie-proto` | kelpie 53 passed / 4 ignored, kelpie-proto 14 / 2 ignored, kelpied 84 passed |
| `cargo clippy -p kelpie -p kelpied` | no warnings in `session_usage.rs`, `usage_report.rs`, `status.rs`, the new `daemon.rs` lines |
| `pnpm typecheck` / `pnpm test` / `pnpm i18n:check` | clean / 13 files 133 tests / 1433 keys 0 missing |
| `pnpm build` | entry `index-*.js` **1,088.61 kB, 338.93 kB gz** (matches the implementer's number); `cost-page` lazy chunk 5.56 kB / 2.35 kB gz + 0.55 kB css |
| `scripts/e2e/context_cost.py` (existing debug `kelpie.exe` 02:50, newer than every source; isolated endpoint/data dir/Claude home) | **22/22**. Afterwards only the user's installed `kelpie.exe` / `kelpied.exe` were running (no leftovers) |
| Real-transcript probes (scratchpad Python; read only `type`, `message.id/model/usage`, `requestId`, `isSidechain`) | 455 Claude files (243 main, 212 `subagents/`), 57 Codex rollouts. Findings below |

## Critical issues

None in the security sense.

## High priority (blocking)

### H1. Sub-agent transcripts are ignored: session cost, cap and Usage page under-count by about 47 %
- Claude Code writes sub-agent (Task) conversations to `~/.claude/projects/<slug>/<session-id>/subagents/agent-*.jsonl`, not into the main file. On this machine there are **0** `isSidechain` lines in main files and **36,002** in sub-agent files. That is 53 % of all assistant usage lines.
- At `pricing.json` prices, main files come to $1,788 and sub-agent files to $1,590 (deduped by max per id, see H2). 24 of 217 sessions have sub-agent spend: median share 15 %, max 100 %.
- `transcript_files` lists only `projects/<slug>/*.jsonl` (`app/src-tauri/src/agent_history.rs:266-269`). `resolve_in` / `usage_of` read only `<id>.jsonl` (`session_usage.rs:688-698, 737-755`). So the ring tooltip, the cost chip, the **cost cap** and the Usage page all leave sub-agent spend out.
- The sidechain branch (`session_usage.rs:170-171`), the `CLAUDE_SIDE` fixture (`:792`), the e2e "sidechain message billed" step and the docs table all describe a layout that real current transcripts do not have.
- Fix:
  - Session totals: also aggregate `<dir of the main file>/<id>/subagents/*.jsonl`. This keeps identity, because the folder is named after the conversation id.
  - Context: read only the main file (as now).
  - Report: add those files to `transcript_files` for Claude. Their aggregates take `cwd` from their own lines, so project attribution works.
  - Add a fixture in the real layout and update `docs/architecture.md`.

### H2. Dedupe keeps the first line of a message. In sub-agent files that line carries a partial `output_tokens`
- Main files: every repeated line of a message has identical usage (23,771 multi-line messages, 0 differ in main files). So today's main-file numbers are right, and the spec's claim holds for them.
- Sub-agent files: **11,876** messages repeat with growing `output_tokens`. Only that field differs; input and cache fields are identical. Sum of the first line is 3.64 M output tokens, sum of the last (= max) line is **18.29 M**. 1,893 messages have non-adjacent repeats.
- `FileUsage::feed` drops any id already in `seen` (`session_usage.rs:462-464`). Once H1 adds these files, sub-agent output would be under-counted about 5x ($1,384 vs $1,590).
- Fix: replace `seen: HashSet<u64>` with `HashMap<u64, u64>` (id hash → output already counted). On a repeat with higher output, add `Δout` to `tokens_out` and `Δout × output rate` to cost. Use the tier chosen from that message's prompt; prompt fields are identical across repeats. Bump `CACHE_VERSION`. Add a test with three lines of one id whose output goes 1 → 40 → 300.

## Medium priority

### M1. Hook `transcript_path` is accepted by shape, not location
- `claude_hook_path` (`session_usage.rs:673-679`) accepts any existing `…/projects/<x>/<id>.jsonl`.
- The id check gives no protection against a forger. The same hook payload also sets `spec.agent_session_id` (`crates/kelpied/src/daemon.rs:705-716`), and `hook.report` is open to Session and **Claimed** (token-less) callers (`auth.rs:94-96, 136-140`).
- So a program in a session can make the app parse any file named that way. Examples: `\\host\share\projects\x\<id>.jsonl`, where `is_file()` makes the app open an SMB connection and send the user's NTLM credentials; or a huge file, which gets counted once on a background thread.
- Impact is bounded: the attacker is same-user code, which `auth.rs:18-20` explicitly does not treat as a boundary, and only numbers on that session result. But the spec asked for "under the agent's transcript dirs", and this is cheap to close.
- Fix: before any filesystem call, reject paths that start with `\\` or `//` or contain `..` components. Then require `path.starts_with(root.join("projects"))`, where `root` is the session's config home already passed in (`session_usage.rs:770`). Compare case-insensitively on Windows. Account homes keep working, because `root` is `sessionHome(s)`.

### M2. Cost-cap Esc almost never fires for a turn that crosses on its last message, and never fires afterwards
- `reportOverCap` adds the session to `capReported` first, then presses Esc only if `s.status === "working"` at that moment (`app/src/lib/session-usage.ts:190-203`).
- A crossing first seen on `Stop` / `turn_end` (status already idle; also every turn without tool hooks) is reported but not interrupted. `capReported` then blocks Esc on every later turn, so the session runs uncapped.
- Fix: keep the inbox entry once, but track Esc separately (`escSent`). Press Esc on the first observation that is both over the cap and `working`, with the exact-conversation check unchanged.

### M3. The perf gate was not run, and the read budget is up to 2x the spec
- Kelpie installs `PreToolUse` / `PostToolUse` hooks (`crates/kelpied/src/agents.rs:450-452`). Every hook broadcasts `session.turn` (`daemon.rs:733-735`), so each working Claude session triggers up to one read per second.
- Each read is a 64 KB tail plus up to 64 KB of aggregate delta counted inline (`session_usage.rs:616-619`). That is up to 128 KB, against the spec's "≤ 64 KB read per update". The phase's perf gate (20 visible sessions, CPU ≤ +0.5 pt) is unverified.
- Fix: run `perf_gate.py --label phase-05`. When the inline delta contains a usage record, take the context from it and skip the tail read, so steady state is a single small read.

### M4. The 85 % hint offers actions that do not exist for some sessions
- ACP chat kinds have no Compact (`agent-actions.ts:15-19`). No chat session gets Hand off (`session-usage.ts:149`, `!s.chat`).
- For an ACP chat at 85 %, "Compact / Hand off…" opens a menu that contains only "Usage & cost…".
- Fix: compute the menu items first. Attach the toast button only when Compact or Hand off is among them; otherwise show a plain toast.

## Low priority

- **L1. Retry chains multiply and responses can arrive out of order.**
  - Every `load` that comes back with `totals:false` starts its own chain of up to 45 retries every 2 s (`session-usage.ts:115`). Each hook event during a background count starts another chain.
  - Concurrent loads have no sequence number, so an older reply can overwrite a newer one.
  - Fix: one retry timer per session id, and drop replies older than the last request.
- **L2. After `/compact` the ring keeps the old fill until the next assistant message.** The tail still finds the pre-compact record, and the `PostCompact`-triggered read returns the same number. Fix: if a `compact_boundary` line follows the last usage record in the tail, report the context as unknown.
- **L3. A Codex resolver miss walks the whole `sessions/` tree, at most once every 3 s per session read** (`session_usage.rs:636, 652-669`). `archived_sessions/` is never searched, which is correct for identity but leaves archived spend out of the report. Thread ids are UUIDv7, so derive the date folder from the id (local ±1 day) before falling back to the walk.
- **L4. Tiny estimates lose their "≈".** `formatCost` returns `<$0.01` without "≈" even when it is an estimate (`usage-format.ts:31-33`).
- **L5. Entry chunk.** 338.93 kB gz is confirmed.
  - Phase-5 code in the entry: `session-usage.ts` (3.7 kB gz source), `context-ring.tsx` (1.3), `usage-format.ts` (1.7), `context-ring.css` (0.5), `vi/usage.json` (1.4; all vi locales are eager-globbed at `i18n.ts:17`). Minified, that is roughly 4–5 kB gz.
  - The rest of the +15 kB is phase 4 (`acp-adapter.ts` +213 lines, `icons.tsx` +52, …). The chat stack was already eager through `sidebar.tsx` / `pane-view.tsx` → `mode-switch` at HEAD, so phase 5 did not newly pull it in.
  - Can be made lazy: `groupRows`, `weekStart`, `folderName` and `ReportRow` are used only by the cost page. Move them into the lazy chunk; `ReportRow` also duplicates `UsageRow` in `types.ts` (DRY).
- **L6.** `last_status` in the daemon ticker is never pruned for closed handles (`daemon.rs:792, 847`).
- **L7. Unverified Codex context detail.** Codex context uses `last_token_usage.total_tokens`, which includes `reasoning_output_tokens` (32 records here). Codex's own indicator may leave reasoning out. Check against the codex-rs source before relying on the exact %.
- **L8. Day boundaries on the Usage page.** `local_day` applies today's UTC offset to all history, so DST zones shift the boundary by an hour. Hourly buckets also cannot split days in :30 / :45 zones (`usage_report.rs:83`). Document it or bucket by the offset at each record.
- **L9. Other connections receive `session.turn` too.** `session.turn` goes to every daemon connection, including session-role clients (`daemon.rs:507-513`). It adds only `transcript_path` beyond what `session.updated` already broadcasts. Acceptable; noted for the exposure inventory.

## Item-by-item verification

1. **Formats**
   - Claude dedupe: `message.id`, else `requestId` (`:173`). Ids are never shared across files: 0 of 30,819 appear in more than one file, so `--resume` forks do not double-count here. Within a file, see H2.
   - Cache-write tiers: 1.25x input for 5 m and 2x for 1 h (`:92`). The 5m/1h split always summed to `cache_creation_input_tokens` (0 mismatches).
   - Haiku 5.5 tier: prompt = input + read + writes > 100 K (`:87-91`, tested).
   - Unknown model → tokens with no cost and no window. `<synthetic>` is skipped.
   - Codex: `info:null` skipped; totals from differences of `total_token_usage`, context from `last_token_usage`. Real rollouts show no carried-over totals and no drops. Window values seen: 258,400 and 475,000.
   - OMP: exact cost and `promptTokens` are used. The "≈" rule is correct apart from L4.
2. **1 M window for Claude.** Keep it, at least for Opus 5.x: open question 1 is resolved by evidence.
   - No `message.model` across 68,432 lines contains `[1m]`.
   - Main-chain context still reaches **968,863** tokens on `claude-opus-5-5` (23,805 messages over 200 K) and 762,361 on `claude-opus-5`.
   - Sonnet 5/5.5 and Haiku 5.5 have no main-chain samples over 200 K here, so they stay unverified.
3. **Identity.**
   - No newest-file fallback anywhere (e2e step 4 passes).
   - Two sessions in one folder are tested in Rust and e2e.
   - Account homes go through `sessionHome(s)` → `home_or`.
   - Remote sessions are excluded (`session-usage.ts:59`). `latest` is rejected.
   - Codex resolver is correct; its cost is L3. The forged-path concern is M1.
4. **Performance.**
   - The tail is bounded at 64 KB and drops the cut line.
   - Reads are debounced in the webview: an accepted deviation, the core is stateless.
   - Only visible or working sessions, or a turn end, trigger reads. Polling happens only against API < 9.
   - Windows handles: `std::fs::File::open` shares read/write/delete and handles are dropped right away, so the agent's transcripts are never locked.
   - Aggregates are cached by (mtime, len), with an atomic tmp+rename save.
   - Open items are M3 and L5.
5. **Privacy.**
   - The serde structs read usage fields only. `cwd` is taken by regex. Logs carry counts only.
   - The daemon forwards only ids and the path (`status.rs:159-165`, tested).
   - See M1.
6. **Cost cap.** Off by default (`costCap: 0`). The inbox entry is created once and deduped across restarts. Esc goes only to that session with an exact conversation match, but see M2.
7. **Old daemons.** API 7 → 9 (8 presumably reserved by phase 4: coordinate before either lands).
   - Against API < 9 the app polls visible working sessions every 5 s.
   - `session.input {key:"esc"}` predates this phase.
   - Unknown events are ignored.
8. **UX.**
   - Thresholds 0.70 / 0.85 are tested. Tooltip lines are correct.
   - The hint shows once per session, kept in localStorage capped at 300 ids.
   - The cost page is lazy. vi strings are complete.
   - See M4.

## Recommended actions (in order)

1. H1 and H2 together: read `subagents/`, dedupe by max output per id, add a real-layout fixture, update the docs table.
2. M1: check the hook path against the root (and reject UNC) before any filesystem call.
3. M2: track Esc separately from the inbox report.
4. M3: run the perf gate; one read per update in steady state.
5. M4, then L1–L5 as follow-ups.

## Unresolved questions

1. Who owns API level 8? This phase sets 9 while HEAD is 7. Confirm that phase 4 takes 8 and that the app's "switch daemon" prompt is acceptable for both bumps.
2. Should the cost cap apply per conversation (current: `/clear` starts the count again) or per Kelpie session? The spec says "per-session".

Status: DONE_WITH_CONCERNS
Summary: Every test, the typecheck, i18n, the build (entry 338.93 kB gz) and the e2e (22/22) pass, but on real transcripts sub-agent files are never read. That misses about 47 % of Claude spend, and they need a max-output dedupe when added. Not ready to ship until H1/H2 plus the hook-path containment (M1) and the cap Esc logic (M2) are fixed.
