# Agent desktop managers: UI/UX ideas for Kelpie chat view

Scope: Conductor, opcode, Crystal/Nimbalyst, Sculptor, Warp, Claude Desktop Code tab / claude.ai/code, Jules. Date 2026-10-23.
Method note: most sources are product changelogs/docs (first-party, high credibility for "feature exists", zero for "feature is good"). Conductor changelog pages give titles + short bodies; user complaints are thin (HN threads for Sculptor only; Conductor review via rywalker.com). Not verified by running any product.

## 1. Feature inventory (evidence)

| Product | Chat-view-relevant features | Source |
|---|---|---|
| Conductor (Mac, Claude+Codex) | Message queue (0.1.0), "Steering" setting: how follow-ups affect a running agent (0.50.0); checkpoints = revert chat to earlier turn, wiping history+changes (0.19.0), Codex checkpoints (0.44.0); table of contents for long chats; chat/conversation summaries + "insta-summarize"; chat search; chat links; per-workspace Todos that block merge until checked (0.28.4); diff viewer in main panel, historical diffs, edit files in diffs (0.84.0); code review + Claude comments on code, PR comment sync; sidebar shows GitHub status (merged/failing CI/conflicts), status groups backlog/active/review/done (0.35.0), "needs permission" in sidebar, click workspace jumps to oldest unread chat (0.44.0); context-usage indicator updates mid-turn; AI response metadata (0.25.0); file pills, clickable file mentions, line-ref links; Re-run actions; permission prompt can "always allow" MCP/web tools; error toasts with copy; completion sounds | [changelog index](https://www.conductor.build/changelog.md), [0.44.0](https://www.conductor.build/changelog/0.44.0-new-sidebar-rebuilt-composer-codex-checkpoints), [0.50.0](https://www.conductor.build/changelog/0.50.0-steering), [0.35.0](https://www.conductor.build/changelog/0.35.0-workspace-status) |
| Claude Desktop Code tab | Transcript view modes Normal/Thinking/Verbose cycled with Ctrl+O; Changes card with `+12 -1` stats opening diff pane; click diff line to comment, Ctrl+Enter submits all comments to Claude; "Review code" button, `/code-review` card with "Walk through in diff" / "Apply fixes"; CI status bar, Auto-fix, Merge-when-ready; Background tasks pane (subagents, bg shells); Context card (% + per-category breakdown); usage ring by model picker; side chat (`/btw`, Ctrl+;) not saved to thread; OS notification when session done and not viewed; Enter while running = steer after current action; shortcuts for permission/model/effort menus; session rail with Environment/Changes/Context cards; auto-archive on PR merge; drag-able pane layout (chat/diff/terminal/plan/tasks/subagent) | [desktop docs](https://code.claude.com/docs/en/desktop) |
| claude.ai/code web | Review diff, inline comments, create PR, keep chatting in same session after push; session continues with app closed | [desktop/web quickstart](https://code.claude.com/docs/en/web-quickstart.md) |
| opcode (ex-Claudia, OSS) | Visual session timeline with branching, checkpoint create/restore/fork, diff between checkpoints; usage analytics dashboard (cost by model/project/time, export); session list with first message + timestamps; CLAUDE.md editor | [repo](https://github.com/winfunc/opcode) |
| Crystal -> Nimbalyst (OSS Electron) | Parallel Claude/Codex sessions in worktrees; Crystal deprecated Feb 2026 for Nimbalyst (agents stream edits into open editors live, mobile companion) | [repo](https://github.com/stravu/crystal) (detail page thin) |
| Sculptor (Imbue) | Container per agent; Pairing Mode (mirror agent branch to local IDE, bidirectional sync); Merge Review UI; fork agent; suggestions (beta). HN praise: diffs + internal to-do list + summaries aid review. Complaints: parallel overhead/cognitive load, idle CPU/mem (Electron), no mobile status | [docs](https://docs.imbue.com/features/containers), [HN](https://news.ycombinator.com/item?id=45427697) |
| Warp | Agent management panel (local + cloud runs, Personal/All tabs); notifications "Request" category (command approval, permission, idle prompt) -> unread tab badge + native alert when app backgrounded; works for Claude/Codex via plugin; diff review with arrow-key hunk navigation | [agent notifications](https://docs.warp.dev/agent-platform/capabilities/agent-notifications.md), [management](https://docs.warp.dev/platform/managing-cloud-agents/) |
| Jules | Plan shown first with per-step feedback + "Approve plan" (auto-approves if ignored); activity feed of steps with inline mini diffs, right-pane full diff, "Publish branch" | [docs](https://jules.google/docs/code/) |

Cross-product convergence (strongest signal): (a) diff stats chip per turn opening a diff pane, (b) inline diff comments batch-sent to agent, (c) steering/queue while running, (d) checkpoints/rewind, (e) context % indicator, (f) "needs you" surfaced at sidebar + OS level, (g) collapsible transcript detail levels, (h) todo/plan panel, (i) session -> PR flow with CI status.

## 2. Kelpie current state (read-only scan)

- Transcript: item kinds user/assistant/thinking/tool/permission/system/result only (`app/src/chat/chat-model.ts:7-15`). No plan/todo, usage, turn-boundary, or checkpoint kinds.
- Rendering: flat `For` over last 300 items, "Show N earlier" button (`chat-pane.tsx:15,123,147`); no virtualization; auto-stick scroll with no "jump to latest" pill or unread marker (`chat-pane.tsx:125-131,145`). Streaming markdown split at stable block (`chat-pane.tsx:34-58`) - good.
- Tool cards: one line (icon, name, summary), diff shown only if open or not done, capped 400 lines, output tail 20k chars (`chat-pane.tsx:60-93`). No per-file +/- stats, no consecutive-tool grouping, thinking is plain text always visible (`chat-pane.tsx:164`).
- Turn summary: single `result-line` "2.5s · 2 turns · $0.012" (Claude only, `claude-adapter.ts:276`). No files-changed list, no diff stats, no "review changes" action.
- Running state: `ctrl.state.busy` -> "Thinking…/Working…" line (`chat-pane.tsx:207`). Sidebar has StatusDot, "Needs you" label, per-project waiting badge, attention filter (`sidebar.tsx:34-53,341,367`); inbox panel for finished/needs-you (`inbox.tsx:1-2`). Good base - parity with Conductor/Warp sidebar already; gaps are in-chat (no pinned approval banner, no unread-turn marker).
- Queue/steer: Codex and ACP queue while busy with a dimmed pending bubble (`codex-adapter.ts:67,391-393`, `acp-adapter.ts:23,176`); Claude writes directly (`claude-adapter.ts:290`). Queue is invisible to user as a list: no edit/cancel/"send now (steer)" per item, no turn/steer use.
- Composer: `/` `$` `@` with ranking, setting pickers (model/effort/mode), mic, Esc interrupt, Stop (`chat-composer.tsx:14-266`). Missing: attachments/paste image, prompt history (Up arrow), draft persistence per session, context/usage in the row (only conversation id shown, `:200`), file pills.
- Context/cost: none in chat; status-bar usage meters are account-level only (`usage-meter.tsx:1`). Codex `contextCompaction` notice only (`codex-adapter.ts:246`). No grep hits for tokenUsage/usage/TodoWrite/turn/plan/turn/diff in adapters.
- Diff/review: `diff-view.tsx` exists with `notesPrompt(ReviewNote[])` (`:64`) = review-notes-to-prompt already built for git panel; not linked from chat. `git-panel.tsx` exists.
- Chat<->terminal: sidebar menu + header chat icon + "Run it in the terminal" for TUI-only slash commands (`sidebar.tsx:124-126`, `chat-pane.tsx:194`) - strong, comparable to nothing in competitors (most have no real TUI).
- History: `history-panel.tsx` resumes past transcripts in chat/terminal.

## 3. Prioritized gap list

Legend: data = what protocol gives. C=Claude stream-json, X=Codex app-server, A=ACP. Effort S/M/L.

1. **Turn summary card: files changed + `+N -M` + "Review changes"** (end of each turn)
   - Look: card under final assistant message: "3 files · +42 -7", per-file rows, click opens diff pane (Kelpie `DiffView`) scoped to those files; Claude Desktop "Changes" card / Jules mini diff.
   - Why: the #1 convergent feature; answers "what did it do" without scrolling tool cards.
   - Data: X: `turn/diff/updated` {diff} aggregated unified diff + `fileChange` items {path, kind, diff}. C: derive from Edit/Write/MultiEdit tool_use inputs (old/new string) in-transcript, or `git diff --numstat` in cwd at turn end (works for all three incl. A, robust to Bash edits). Recommend git-based baseline: record `git stash create`/HEAD+status at turn start, numstat at `result`.
   - Effort M. Files: `chat-model.ts` (new `turn` item kind with files/stats), the three adapters (turn start/end hooks), `chat-pane.tsx` (new `TurnSummary`), `diff-view.tsx`/`right-panel.tsx` (open from chat), `ipc.ts` (git numstat call if daemon lacks one).

2. **Inline diff comments -> batch send to agent**
   - Look: click a diff line, write note, "Send N comments" (Ctrl+Enter) becomes one prompt. Claude Desktop, Conductor.
   - Why: closes review loop inside chat; Kelpie already has `ReviewNote`/`notesPrompt`.
   - Data: none from agent; pure client. Effort S (reuse) -> M if wiring to chat ctrl. Files: `diff-view.tsx:64`, `send-to-agent.ts`, `chat-pane.tsx` entry from #1.

3. **Context % + token/cost chip in composer row**
   - Look: small ring/"34% context" next to model picker, hover breakdown; Conductor "Claude's Context", Desktop Context card; per-turn cost already in result line.
   - Why: users decide `/compact` or new session off it; Kelpie shows only conversation id there.
   - Data: C: `message_delta`/assistant `message.usage` (input+cache tokens) + model context window (init/model table); `/context` exists as slash. X: `thread/tokenUsage/updated`. A: usage not standardized (hide). Effort M. Files: `chat-model.ts` (`usage` in ChatState), `claude-adapter.ts`, `codex-adapter.ts`, `chat-composer.tsx:196-203`.

4. **Plan/todo panel pinned above composer**
   - Look: collapsible "3/7 steps" list from agent's todos; Jules plan, Sculptor to-do, Conductor todos, Desktop plan/tasks pane. Also a "Plan approval" card for plan mode.
   - Data: C: `TodoWrite` tool_use input `todos[{content,status}]` (currently rendered as raw JSON tool card). X: `turn/plan/updated` {plan[{step,status}]} + `plan` item. A: `plan` session update (ACP spec). All three provide it. Effort M. Files: `chat-model.ts` (state.plan), adapters, `chat-pane.tsx` (`PlanBar`), hide TodoWrite card.

5. **Queue/steer as first-class: visible queue chips, edit/cancel, "send now"**
   - Look: pending messages above composer as removable chips; Enter while busy = steer (Codex `turn/steer`) vs Alt+Enter = queue after turn; Conductor 0.50 setting, Desktop Enter-steers.
   - Data: X: `turn/steer` (confirmed in docs; no turn/started). C: stream-json accepts user lines mid-turn (CLI queues them) - already what Kelpie does, can't cancel once written. A: no steer; client-side queue. Effort M. Files: `codex-adapter.ts:67,139-158,391` (swap queue for steer, expose `queue` in state), `acp-adapter.ts:23,176`, `chat-composer.tsx` (chips, key bindings), `chat-model.ts` (`ChatState.queue`).

6. **Checkpoints / rewind to a user turn (chat + files)**
   - Look: hover user message -> "Rewind here" (files / files+conversation); Conductor checkpoints, opcode timeline, Desktop rewind.
   - Data: C: needs `--replay-user-messages` + `CLAUDE_CODE_ENABLE_SDK_FILE_CHECKPOINTING`; user msg uuid = checkpoint; rewind via control request or `--rewind-files` flag (hidden, undocumented in --help). Only Write/Edit tracked, not Bash. Conversation truncation = resume at uuid / fork. X: `thread/fork`, `thread/revert` (rollback removed - verify against installed Codex version). A: none. Alternative agent-agnostic: git snapshot per turn (`git stash create` ref under refs/kelpie/) - works for all, Kelpie owns worktrees. Effort L. Files: adapters, `chat-model.ts` (checkpoint ref on user item), `chat-pane.tsx`, daemon (`crates/kelpied`) for snapshot refs.

7. **Transcript density modes (Normal / Thinking / Verbose) + tool-run grouping**
   - Look: Ctrl+O cycles; Normal collapses runs of tool cards into "Ran 6 commands, edited 2 files" and hides thinking; Verbose = today. Desktop Code tab.
   - Why: long sessions are dominated by tool cards; also a perf lever (fewer DOM nodes).
   - Data: all local. Effort S-M. Files: `chat-pane.tsx` (group consecutive `tool` items; thinking collapsed by default, `msg-thinking`), `styles.css`, setting in `settings-catalog.ts`.

8. **In-chat "needs you" banner + unread-turn marker + jump-to-latest**
   - Look: sticky banner over composer when a permission is pending (Approve / Deny without scrolling); "N new messages" pill when scrolled up; sidebar click opens chat at oldest unread (Conductor 0.44.0); OS notification on done/needs-input when window unfocused (Desktop, Warp).
   - Data: local; permission items already exist. Effort S. Files: `chat-pane.tsx:125-145,170-187`, `store.ts` (last-seen item per session), `inbox.tsx`/OS notify (check existing).

9. **Long-session performance: virtualize list, lazy markdown, cap DOM**
   - Look: windowed rendering by height instead of "last 300 + button"; markdown parsed only when item enters viewport; collapsed tool bodies not rendered. No competitor evidence of complaint specifics (Sculptor HN: idle CPU/mem on Electron) - Kelpie's Tauri base is an advantage, protect it.
   - Effort M. Files: `chat-pane.tsx:15,123`, perf gate in repo (`scripts/e2e`, vitest baseline).

10. **Chat search + table of contents/turn navigator** (Conductor chat search, ToC). Data local. Effort M. Files: `chat-pane.tsx` (turn anchors from user items), new small `chat-outline`.

11. **Prompt history (Up), per-session draft persistence, image paste/attachments, file pills** (Conductor rebuilt composer). Data: C accepts image content blocks in stream-json; X `UserInput` image; A prompt image capability-gated. Effort M. Files: `chat-composer.tsx`, `store.ts`.

12. **Side chat (`/btw`)**: ephemeral Q using session context. Data: Claude CLI `/btw` is TUI feature; in stream-json uncertain. Effort M-L, low priority.

13. **Background tasks/subagent pane**: C emits `parent_tool_use_id` on subagent messages and task events; X collab/agent items. Nest subagent activity under its Task tool card (S) before building a pane (L). Files: `claude-adapter.ts:34`, `chat-pane.tsx` ToolCard.

14. **Session -> PR flow with CI status + auto-fix** (Desktop, Conductor). Kelpie has `pull-requests.ts`, `git-panel.tsx`; add a PR/CI chip in chat header. Effort M. Out of core chat scope; reuse existing modules.

15. **Usage dashboard (opcode analytics)**. Kelpie has account meters; per-project/model cost report is nice-to-have. L, defer.

Skip / anti-picks: containers per agent (Sculptor; heavy, Kelpie uses worktrees); Pairing Mode; merge-blocking todos (policy not UX); agent builder (opcode CC Agents); cloud sessions; editing files inside diff (Conductor 0.84, big scope).

## 4. Trade-off matrix (top picks)

| # | Idea | Value | Effort | Cross-agent data | Risk |
|---|---|---|---|---|---|
| 1 | Turn summary card | very high | M | git-based = all agents | git baseline wrong in shared worktrees/dirty trees |
| 2 | Diff comments -> agent | high | S | none needed | low (reuse) |
| 3 | Context/cost chip | high | M | C+X yes, A no | token math for cache tokens, window sizes drift |
| 4 | Plan/todo bar | high | M | all three | schema drift per agent |
| 5 | Queue/steer | high | M | X native, C implicit, A client | steer semantics differ; unsent-edit impossible on Claude |
| 6 | Checkpoints | high | L | C partial, X yes, A no | Claude hidden flag; Bash not tracked; truncation semantics |
| 7 | Density modes | medium-high | S-M | local | low |
| 8 | Needs-you banner/unread | high | S | local | low |

## 5. Top 8 picks, ranked

1. **Turn summary card (#1)**. Every product has it; biggest "is this good" lift; unlocks #2 and #6 entry points. Use git numstat baseline for agent independence, enrich with X `turn/diff/updated`.
2. **Needs-you banner + unread/jump-to-latest + notify (#8)**. S effort, directly addresses the many-sessions use case Kelpie is built for; sidebar half already done.
3. **Plan/todo bar (#4)**. All three protocols supply data; today TodoWrite shows as raw JSON.
4. **Context/cost chip (#3)**. Cheap decision aid; fills empty composer row.
5. **Visible queue + steer (#5)**. Kelpie already queues invisibly; make it controllable, adopt `turn/steer` for Codex.
6. **Density modes + tool grouping (#7)**. S effort; fixes long-session noise and reduces DOM.
7. **Diff comments -> agent (#2)**. Reuse `notesPrompt`; small code, closes the review loop.
8. **Checkpoints/rewind (#6)**, via Kelpie-owned git snapshots rather than agent flags. Highest effort, most differentiated (Conductor, opcode, Desktop all prize it); ship after #1 since the summary card is its UI anchor. Perf virtualization (#9) is a close ninth; do it if the perf gate regresses once #1/#7 add DOM.

Suggested order: 8 -> 1 -> 7 -> 4 -> 3 -> 5 -> 2 -> 6.

## 6. Limitations

- No hands-on use of any product; Conductor changelog bodies mostly titles; Crystal/Nimbalyst and Sculptor UI detail thin; claude.ai/code web UI layout unverified; Cursor/Windsurf/Codex app/VS Code ext covered by other researchers.
- Codex `thread/revert` vs removed `thread/rollback` read from one doc page; verify against the installed Codex's generated JSON schema (`codex app-server generate-json-schema`).
- Claude `--rewind-files`/checkpoint env var documented but flag hidden from --help: treat as unstable.
- Did not inspect daemon (`crates/kelpied`) for existing git-stat endpoints.

## Unresolved questions

1. Does the daemon or `ipc.ts` already expose git numstat / per-session baseline refs? Decides #1 and #6 cost.
2. Should steer be default on Enter (Desktop) or opt-in (Conductor setting)?
3. Is ACP agent (OMP) usage/plan data actually emitted? Affects #3/#4 parity claims.
4. Does Claude stream-json `assistant.message.usage` arrive with cache fields in the CLI version Kelpie pins (needed for context %)?
5. Is OS-level notification for chat sessions already wired (inbox exists; native toast unchecked)?

Status: DONE_WITH_CONCERNS
Summary: Report written with feature inventory, Kelpie gap map (file:line), 15 ranked ideas and top-8 picks. Concerns: mostly doc/changelog evidence with no hands-on testing, thin detail for Crystal/Nimbalyst and Sculptor UI, and Codex revert and Claude rewind APIs need version verification.
