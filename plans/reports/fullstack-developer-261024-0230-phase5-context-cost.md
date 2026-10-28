# Phase 5 report: context, tokens and cost

Date 2026-10-24. Scope: whole of phase-05-context-cost.md. Nothing committed. The installed Kelpie, its daemon and the real data folder were not touched (the e2e runs use an isolated endpoint, data dir, Claude / Codex homes, and a stand-in `claude` batch shim; no real Claude session was started).

## What was built

**Daemon (API_LEVEL 8 -> 9)**
- `session.turn {session, event, transcript_path?, agent_session_id?}` is broadcast from `hook()` (every hook event) and from the ticker when a non-chat session leaves `working` for `idle`/`waiting` (`event: "turn_end"`, so agents without hooks work too).
- The event is built by `status::turn_event` from the hook payload only (nothing else of the payload is forwarded; `event` capped at 64 chars, conversation id at 200, path at 4096). No RPC was added: no program can publish an event, `hook.report` is already forced to the caller's own session in `auth.rs`, so `auth.rs` needed no change.
- Documented in `docs/architecture.md` (events list plus a new "Context, tokens and cost" section and the module table).

**App core**
- `session_usage.rs`: Claude / Codex / OMP parsers, normalised-model price lookup (`pricing.json`, exact ids only; unknown model = tokens, no cost, no window), reverse tail (<= 64 KB, cut first line dropped, half-written last line skipped), per-file aggregate (resumes from the last complete line, message ids hashed for dedupe, hourly buckets per model, persisted in `usage-cache.json`, big gaps counted on a background thread), strict transcript resolver, Tauri command `session_usage`.
- `usage_report.rs`: `usage_report` (rows by local day / agent / project / model from every transcript; warm = stat only; background pass of two workers, small files first; emits `usage-scanned`).
- `agent_history.rs` / `usage.rs`: only visibility changes (`pub(crate)`) so the locator, `Agent`, `open_lines`, `parse_iso_ms` are reused.
- Identity mapping: Claude = hook `transcript_path` (accepted only as `.../projects/<slug>/<id>.jsonl` named after the claimed id) else `<home>/projects/<slug(cwd)>/<id>.jsonl` else `projects/*/<id>.jsonl`; Codex = `rollout-*-<thread id>.jsonl` found by walking the date folders newest first (cached; `latest` is not a thread); OMP = `*_<session id>.jsonl`. No id, no numbers; no newest-file fallback anywhere.

**UI**
- `context-ring.tsx` (+ css) on the sidebar row, pane header and tab: grey / amber >= 70 % / red >= 85 % (red also heavier stroke), tooltip with context, tokens, cost ("≈ $" for Claude, plain "$" for OMP and ACP), model, "still counting". OMP (no window) shows a cost chip instead of a ring. Click = menu: Compact, Hand Off to <agent>, Interrupt (Esc, only over cap and working), Usage & cost.
- `lib/session-usage.ts`: reads on `session.turn` (1 s debounce, only visible/working sessions or a turn end), first read when a session comes into view, retry while the totals are still counting, polling (5 s, visible + working) against a daemon below API 9; chat sessions use `getController(s).state.usage` (read-only import of `mode-switch.ts`).
- One-time 85 % hint (toast, once per session, remembered in localStorage): "Compact / Hand off..." opens the ring menu at the button (`Toast.action.run` now receives the click).
- Hand-off core moved from `terminal-pane.tsx` to `agent-actions.ts` (`handOffSession`, `sessionTranscript`) so the ring and the terminal menu share it.
- `cost-page.tsx` (lazy chunk, 5.6 kB / 2.4 kB gz + css): day / week / project / agent / model, 7 / 30 / 90 days / all; reachable from the palette ("Usage & Cost..."), the usage meter (right-click menu and a "Cost" item beside it), Settings -> Usage & Resources.
- Cost cap (Settings -> Usage & Resources, 0 = off): red chip on the session, one inbox entry (kind `cost`, deduped across restarts by session id), optional "Press Esc when a session passes the cap" (off by default; only while working and only when `readings[s.id].conv === s.agent_session_id`, i.e. numbers from that exact transcript).
- Settings catalog entries, Vietnamese strings in the new `locales/vi/usage.json`, README, user guide, architecture docs.

## Files

New: `app/src-tauri/src/{session_usage.rs,usage_report.rs,pricing.json}`, `app/src/lib/{session-usage.ts,usage-format.ts,usage-format.test.ts}`, `app/src/components/{context-ring.tsx,context-ring.css,cost-page.tsx,cost-page.css}`, `app/src/locales/vi/usage.json`, `scripts/e2e/context_cost.py`.

Modified: `crates/kelpie-proto/src/lib.rs`, `crates/kelpied/src/{daemon.rs,status.rs}`, `app/src-tauri/src/{lib.rs,agent_history.rs,usage.rs}`, `app/src/lib/{types.ts,store.ts,ipc.ts,agent-actions.ts,commands.ts,settings-catalog.ts}`, `app/src/components/{app,sidebar,pane-view,workspace-view,terminal-pane,usage-meter,inbox}.tsx`, `app/src/components/settings/interface-settings.tsx`, `README.md`, `docs/architecture.md`, `docs/user-guide.md`. None of the files on the do-not-edit list was touched. Styles live in the two new css files (imported by their components) so `styles.css` stayed alone.

## Tests (all green)

- `cargo test -p kelpied -p kelpie -p kelpie-proto`: kelpie 53 passed / 4 ignored, kelpie-proto 14 / 2 ignored, kelpied 84 passed. New: 14 Rust tests in `session_usage.rs` (Claude cache tokens at 5 m / 1 h prices + dedupe by message id, sidechain billed but not context, synthetic skipped, unknown model, model-id normalisation, Haiku 5.5 long-prompt tier, Codex `info: null` + total differences + model from `turn_context`, OMP exact cost, truncated last line and malformed usage line, reverse tail <= 64 KB with a record outside the window ignored, aggregate resuming past a half-written line, two Claude sessions in one folder mapping to their own transcripts and a foreign `transcript_path` rejected, a transcript appearing after a first miss, Codex resolve by thread id, OMP resolve by session id), 1 in `usage_report.rs` (local-day bucketing), 1 in `status.rs` (event carries identity only, bounds).
- `cargo check -p kelpie` clean. `cargo clippy` is clean for every new/changed line of mine; `-D warnings` on the workspace still fails on code that is not mine (`kelpied` `runs.rs`, `daemon.rs:492`, existing `agent_history.rs` lints).
- `cd app && pnpm typecheck` clean; `pnpm test` 13 files / 133 tests (new `usage-format.test.ts`: ring thresholds 70 / 85, formatting, "≈" labelling, cap, week start, report grouping); `pnpm i18n:check` 1433 keys, 0 missing.
- `scripts/e2e/context_cost.py` (debug build, isolated daemon / app): **22/22**. Covers: API 9; two Claude sessions in one folder with their own ids; `--continue` session has no id; the daemon event carries exactly the ids and path (a `prompt` field in the hook payload is not forwarded); rings 71 % amber / 86 % red from their own transcripts (cc2's hook named cc1's transcript and was still read from its own); numbers equal fixture truth (tokens 701000 / 9000, cost 0.924 at Opus 5.5 prices, 5 m / 1 h cache writes, repeated message counted once, sidechain billed); live append moves the ring to 88 % red, a repeated message adds nothing, totals 1573000 / 17000 / 1.266; no ring for the id-less session though a newer transcript sits in the folder; unknown id and no id give no numbers; `turn_end` broadcast without a hook; cap 0.70 puts a red "≈ $1.27" chip and exactly one inbox entry on cc1 only, none on cc2 (0.60), cap off removes it; Usage page shows the fixture project with 2.4M / 27k / "≈ $1.87"; Esc closes it. Screenshots in the scratchpad (`shots-cc`).
- `scripts/e2e/attention.py <kelpied.exe>` 12/12; `scripts/e2e/attention_gui.py` 31/31 (regression for the touched sidebar / inbox).
- Real-data check (`cargo test -p kelpie --lib --release -- --ignored --nocapture report_real`, 714 transcripts on this machine): Claude output tokens 11,339,927 equal an independent Python tally (dedupe by message id) exactly; cold count 9.7 s (release, 2 workers not used by the test: single thread), warm report 38-42 ms (budget 100 ms; one noisy run on a loaded machine read 116 ms).
- Bug found by the e2e and fixed: a miss on a transcript lookup was cached for 10 s, so a transcript that appeared right after the id was announced stayed hidden (a forged hook path then had no fallback); the Claude folder lookup is now one uncached stat, other misses are remembered for 3 s.

## Perf notes

- Reads per update: <= 64 KB tail; totals continue from the last counted line (steady state a few KB); a gap > 64 KB goes to a background thread. 20-session CPU was not measured with the perf gate (`perf_gate.py --label phase-05` not run: it was not in the task's run list).
- Entry JS (debug-build vite output): 338.9 kB gz vs the phase-1 baseline 323.6 (+15.3 kB) and 328.5 at phase 2. That includes other sessions' uncommitted phase-4 code; my share estimated at about 6 kB gz (session-usage + ring + format ~3.8, `usage.json` ~1.3 because vi locales are bundled eagerly, settings / catalog / actions the rest). Cost page is a separate lazy chunk. Worth re-checking with the gate once phase 4 lands.

## Deviations from the phase file

- The 1 s debounce lives in the webview service (`session-usage.ts`), not in the Rust core: the core command is stateless per call and cheap by the (mtime, len) / offset aggregate, the webview already knows visibility and status.
- OMP: the daemon never learns an OMP session id for a fresh terminal session (only chat / resume paths set it), so those show nothing, per "no id, no numbers".
- Claude context windows come from the price table (1 M for all but Haiku 4.5), as the phase file says.
- `acp-adapter.ts` (listed in the phase's file list) was not touched (forbidden); chat sessions simply read `state.usage` as the adapters already fill it. `Claude` chat sessions show their cost as an estimate ("≈"), ACP / Codex chat as the agent's number.
- The 85 % hint is one toast with one button that opens the Compact / Hand off menu, not two buttons (`Toast` has a single action).
- Plan/phase files were not edited (status is owned by `ak plan`).

## Open questions

1. Claude Code only gives Opus / Sonnet a 1 M window with the `[1m]` model tag; without it the real window is 200 K, so the ring understates fill for those sessions. Per phase file it is 1 M; confirm or switch to "1 M only when the model id carries `[1m]`".
2. The ring is read only for visible or working sessions, so a background session that never worked since the app started has no ring until its next turn. Acceptable?
3. Cost cap Esc cannot be exercised by the e2e (needs a genuinely working Claude); covered by logic (`working` + exact conversation match) and the menu item, not by a GUI test.

## Review fixes (code-reviewer-261024-0910, coordinator decisions applied)

- **H1** Claude sub-agent transcripts (`<slug>/<id>/subagents/agent-*.jsonl`) now count towards a session's tokens, cost, cap and the Usage page (`subagent_files`; the report lists them too). Context still comes from the main file only. Real data: Claude output tokens 11.3 M -> 29.7 M, estimated total $2,810 -> $4,420 (the reviewer's ~$1,590 sub-agent share matches).
- **H2** Dedupe keeps the highest output per message id (`seen`: id hash -> output counted; a repeat with more output adds only the extra tokens at that message's own output rate, tier included). `CACHE_VERSION` 2. Tests: 1 -> 40 -> 300 -> 40 gives 300; main-file layout (identical repeats) unchanged; real-layout sub-agent fixture in Rust and in the e2e.
- **M1** A hook `transcript_path` is accepted only as exactly `<this session's config home>/projects/<slug>/<id>.jsonl`, local, with UNC / `//` / `\\?\` / `..` refused by their text before any file access, and still inside `projects` after canonicalisation. Test covers three UNC forms, `..`, another account's `projects`, and the fallback to the lookup by id.
- **M2** The cap's Esc is tracked apart from the inbox report: pressed once, the first time the session is over the cap and working (so also on the turn after the crossing). Found while testing: the old Esc sent `session.input {key}`, which the daemon ignores; it now types the raw ESC through `term_input`. e2e: a stand-in agent that reports received ESC proves one Esc, and only once.
- **Cap per Kelpie session** (decision): costs are summed over a session's conversations (`/clear` keeps the sum), kept in localStorage; the chip shows the sum. e2e: cc2 moves to a new conversation, passes the cap with the sum, gets its own inbox entry.
- **M3** An update reads at most 64 KB in all: the aggregate remembers the main file's last context, so a fully counted file needs no tail read; the inline budget (64 KB) is shared by the main and sub-agent files; anything bigger goes to one background thread (context then from the 64 KB tail). Perf gate (`--metrics bundle --runs 1`): entry **338.48 kB gz, limit 338.60: ok** (338.86 before). Done by moving the page's folding to the lazy `lib/usage-report.ts` (plus tests), the ring menu / 85 % hint / cap reactions to the lazy `lib/usage-alerts.ts`, and dropping the redundant "Usage & cost page" settings row. The margin is 0.12 kB: other work in the entry chunk can tip it again. The GUI perf metrics were not run.
- **M4** The hint's button appears only when the menu holds Compact or Hand off.
- **Lows** L1 one retry timer per session and replies carry a sequence (older ones dropped); L2 a `compact_boundary` after the last usage record hides the old fill; L3 Codex rollouts are found from the UUIDv7 date folder (+-1 day) before any walk, a walk miss is remembered 20 s; L4 tiny estimates read "≈ <$0.01"; L5 done under M3 (`ReportRow` replaced by `UsageRow`); L6 daemon `last_status` is pruned when a session stops; L7 (Codex context includes reasoning tokens) and L8 (day boundaries use today's offset, hourly buckets) are documented in `docs/architecture.md`, not changed; L9 accepted.
- Kept: the 1 M Claude window (evidence in the review).

Re-run after the fixes: cargo test kelpie 58 (+4 ignored) / kelpie-proto 14 / kelpied 84; typecheck clean; vitest 14 files, 133 tests; i18n 1430 keys, 0 missing; `context_cost.py` **26/26** (adds sub-agent layout, per-session sum across a conversation change, one Esc); `attention.py` 12/12. Real-data release check with sub-agents: 926 transcripts, warm report 86 ms (budget 100). The e2e and attention scripts set `KELPIE_ENDPOINT` / `KELPIE_DATA_DIR` in their own `os.environ` before any daemon call; the installed Kelpie and its daemon were not touched.

Status: DONE_WITH_CONCERNS
Summary: All six scope items are built and green (cargo, vitest, typecheck, i18n 0 missing, context_cost.py 22/22, attention.py 12/12, attention_gui 31/31); Claude totals match an independent tally exactly and the warm report is about 40 ms.
Concerns/Blockers: The bundle gate passes with only 0.12 kB margin (338.48 vs 338.60); the GUI perf metrics (20-session CPU) were not run.
