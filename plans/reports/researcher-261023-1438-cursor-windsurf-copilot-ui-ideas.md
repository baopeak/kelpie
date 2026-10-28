# Cursor / Windsurf / Copilot agent UI -> Kelpie chat view ideas

Date 2026-10-23. Scope: Cursor (2.x agents window), Windsurf Cascade, GitHub Copilot agent mode (VS Code 1.110+). Read-only on code.

## 1. Evidence quality (be honest)
Official docs pages fetched are thin on UI detail (summarizer returned "not covered" for many items). Confidence per claim marked: [D]=official doc, [R]=release notes/blog, [F]=forum/3rd party, [K]=my product knowledge, unverified this session. Nothing here is single-source for the top picks except where flagged.

## 2. Feature inventory

### Cursor
- Agents sidebar + parallel agents (up to 8, git worktrees/remote) [D: cursor.com/changelog/2-0]
- Plan mode: Shift+Tab or mode picker; clarifying questions, editable plan markdown, "Build" button, save-to-workspace [D: cursor.com/docs/agent/planning]
- Checkpoints: auto snapshot before significant changes, preview in chat timeline, restore files only (messages stay), stored outside git [D: cursor.com/docs/agent/overview]
- Queue: Enter queues, drag to reorder, Cmd+Enter / "Send now" steers at next tool call [D: same]
- Side chats `/side` `/btw`; @-mention a side chat to pull context back [D]
- Review: diff shown live while agent works; bar above input with file count + Review / Keep All / Undo All; "Find Issues" post-run review; `@Branch`; Stop = Cmd+Shift+Backspace [D: cursor.com/docs/agent/review; F: forum.cursor.com threads on Keep All/Undo All]
- Forum lessons (anti-patterns): Keep/Undo All buttons swapping position -> accidental Undo All; Undo on agent-created file deletes file; pending review state lost on restart; button hidden when pane narrow [F]
- Multi-file review view without opening each file [D 2.0]
- Prompt UI: files/dirs as inline pills; fewer @ menu items because agent self-gathers context [D 2.0]
- Images: pasted/read, generated images inline in chat [D]
- Sandboxed terminal, run modes (allowlist/ask/sandbox) [D: 2.0, run-modes page not fetched]
- Voice mode, remote control of local agents from phone (Oct 2026) [D changelog]
- Collapsed "Explored N files, M searches" groups [K, not verified from docs]

### Windsurf Cascade (docs moved to docs.devin.ai)
- Code/Chat modes [D]
- Revert arrow on hover of each prompt + table of contents of conversation; named checkpoints; reverts irreversible [D: docs.devin.ai/desktop/cascade/cascade]
- Queued messages: Enter queues while working, Enter on empty box sends now, delete queued items [D]
- Todo list in conversation; background planning agent [D]
- @-mention previous conversations (summaries/checkpoints pulled, not full text) [D]
- Auto-Continue after 40 tool-call limit [D]
- Parallel Cascades with dropdown switcher; worktree isolation to avoid edit races (Wave 13) [D, R: devin.ai/blog/windsurf-wave-13]
- Context window indicator, Hooks (Wave 13) [R: testingcatalog]
- Per-tool-call "Auto-fix" toggle on lint step [D]
- Bottom "N files edited" bar with Accept all / Reject all / per-file [K, not verified]

### GitHub Copilot agent mode (VS Code)
- Pending edits model: edit saved -> pending; Keep/Undo per hunk or file, Keep all/Undo all; auto-accept delay; reveal-next-on-resolve [D: code.visualstudio.com/docs/copilot/chat/review-code-edits]
- Chat pill with file count + diff stats -> opens multi-diff editor [D same]
- Checkpoints per request, Restore (files + later turns) and Redo; editing an earlier request reverts and resends [D same]
- Request queue: Add to Queue / Steer with Message / Stop and Send, default configurable, drag to reorder pending [D: chat-agent-mode page]
- `!cmd` runs shell inline, output + exit status in transcript [D]
- Ctrl+F searches whole conversation [D]
- Context window control in input: hover = token breakdown by category; near limit suggests compaction; "Compact Conversation" or `/compact [instructions]` [R: v1.110 notes]
- `/fork` creates independent session with history; rename sessions [R: v1.110]
- Terminal tool calls collapsible with summary header; checkmarks off by default [R: v1.110]
- TODO list widget; Ctrl+Shift+T focuses it [R: v1.103/1.110]
- Per-session pickers: Session target, Agent, Model, Permissions; `/yolo` `/autoApprove` [D, R]
- Tool-count grouping (virtual tool groups) - not UI-relevant for Kelpie.

## 3. Kelpie current state
- Transcript: flat `For` over items; kinds user/assistant/thinking/tool/permission/system/result (`chat-model.ts:7-15`). Render window 300 + "Show N earlier items" (`chat-pane.tsx:15,123,147-149`); `content-visibility:auto` on children (`styles.css:530`), no real virtualization.
- Tool row: one card per tool, status glyph, name, 1-line summary, click-to-expand; diff auto-shown while not done, 400-line cap, no per-file header actions (`chat-pane.tsx:60-93`). No grouping of consecutive read/search tools. Thinking = always-open italic block (`chat-pane.tsx:164-166`). Plan (ACP/Codex) flattened into a "thinking" item (`acp-adapter.ts:99-101`, `codex-adapter.ts:213-214`). Claude TodoWrite not special-cased (rendered as generic tool; `claude-adapter.ts:63`).
- Permission card: title + detail + option buttons; resolved -> "-> label" (`chat-pane.tsx:170-187`). No keyboard shortcuts, no diff preview inside approval for Edit.
- Running state: "Thinking... / Working..." line, Stop button, Esc interrupts (`chat-pane.tsx:207-209`, `chat-composer.tsx:147-149,209-211`). Messages sent while busy are written straight to stdin with a dimmed pending bubble (`claude-adapter.ts:289-290`, `codex-adapter.ts:392`); no queue list, no steer vs queue distinction, no reorder/cancel.
- Turn end: `result` line "status - duration - turns - $cost" (`claude-adapter.ts:274-276`, `codex-adapter.ts:361`). No "N files changed +x -y", no review.
- Composer: textarea, `/` `$` `@` popups, model/effort/mode `SettingPicker`s, conversation id, Resume, Stop, mic, Send (`chat-composer.tsx:153-218`). No context chips/pills, no image paste/drop, no draft history (Up arrow), no Shift+Tab mode cycling, no context-usage meter.
- Scroll: stick-to-bottom with 40px threshold, no "jump to latest" button (`chat-pane.tsx:125-131,145`).
- No checkpoints/restore, no in-chat search, no session fork/compact UI (Codex `Context compacted` system line exists, `codex-adapter.ts:247`).

## 4. Prioritized gap list
Event-source legend: C=Claude stream-json, X=Codex app-server, A=ACP. "Have" = already parsed today.

| # | Idea | Looks like | Why | Data / sources | Effort | Kelpie files |
|---|------|-----------|-----|----------------|--------|--------------|
| 1 | Collapse tool runs into summary groups | Consecutive Read/Grep/Glob/List/WebSearch tools -> one row "Explored 5 files, 2 searches" (spinner while running, expand to list). Terminal calls: collapsible header with command + exit status | Biggest density win; transcripts are mostly tool rows. Cursor [K], Copilot collapsible terminal [R] | Pure client-side: needs `category` on tool item (read/search/edit/exec/other). C: map by tool name. X: item types commandExecution/fileChange/webSearch (have). A: `tool_call.kind` (read/edit/search/execute) already in payload | S-M | `chat-model.ts` (add `category`), 3 adapters, `chat-pane.tsx` (group pass in `items` memo), `styles.css` |
| 2 | Turn-end summary + changed-files bar | After each turn: "4 files changed +37 -12" pill, click expands per-file list with inline diff; sticky bar above composer for the whole session (Copilot pill, Cursor review bar) | Gives closure + review entry point; cheap because diffs already collected in tool items | Derive from tool.diff (have for C Edit/Write via input, X fileChange, A diff content). X also has `turn/diff/updated` aggregate [K, verify]. Counting +/- from diff text is trivial | S-M | `chat-model.ts` (derived selector), `chat-pane.tsx` (new `TurnSummary`), `styles.css` |
| 3 | Todo / plan widget pinned | Checklist (done/active/pending) pinned at top of transcript or above composer, collapsible; updates in place | Cursor/Windsurf/Copilot all have it; gives progress at a glance for long runs | C: TodoWrite tool_use `input.todos[{content,status,activeForm}]` (have raw, not rendered). X: `turn/plan/updated` + `plan` item [K verify]. A: `plan` session update `entries` (have, flattened to text) | S-M | add `ChatItem` kind `todos` or `ChatState.todos`; 3 adapters; `chat-pane.tsx` |
| 4 | Real message queue UI (queue / steer / stop-and-send) | Pending messages shown as chips above composer, each with x to cancel, drag reorder; Enter = queue, Ctrl/Cmd+Enter = steer/send now | Copilot [D] and Cursor [D] and Windsurf [D] all converged on this; today Kelpie blind-writes to stdin so ordering/cancel is impossible | Client-side queue, flush on turn end (`result` / `turn/completed` / `stopReason`). Steer: C stream-json accepts user msg mid-turn (current behavior, CLI queues itself); X `turn/steer` [K verify]; A none -> stop+send. Cost: need to own queue instead of writing immediately | M | `chat-model.ts` (state.queue), 3 adapters `send`, `chat-composer.tsx`, `chat-pane.tsx` |
| 5 | Context usage meter + compact/new-chat suggestion | Ring or "62% - 124k/200k" next to pickers, hover = breakdown, turns amber >80% with "Compact" / "New chat" button (`/compact`) | Copilot v1.110 [R] and Windsurf Wave 13 [R] both shipped; prevents silent quality decay in long chats | C: assistant `message.usage` (input+cache_read+cache_creation) + result `modelUsage[].contextWindow` [K verify field]. X: `thread/tokenUsage/updated` [K verify]. A: `usage_update` in newer spec [K verify; may be absent per agent]. Compact action: C `/compact` supported in stream-json; X `thread/compact/start` [K] | M | adapters, `ChatState` (`usage`), `chat-composer.tsx` row, `chat-pane.tsx` |
| 6 | Per-hunk/file diff with copy + open-in-editor, Undo-file (best-effort) | Diff card with filename header, +x -y, expand all, "Revert file" via `git checkout`/reverse-apply | Cursor/Copilot keep/undo is their review core. But agents already applied edits (no pending state) so Kelpie can only do post-hoc revert, not accept gating | Data have. Revert needs daemon git op. Caveat: Cursor "Undo All deletes created files" bug [F] -> must handle new-file case, confirm before destructive | M-L | `chat-pane.tsx` ToolCard, Rust daemon command, `lib/ipc` |
| 7 | Checkpoints per user turn (agent-agnostic via git) | Hover user bubble -> "Restore files to before this turn" (+ Redo); stored as hidden git ref/stash-create snapshot taken on send | Cursor/Windsurf/Copilot all have; works for all 3 agents since Kelpie owns snapshot, no protocol needed. Does not rewind conversation (Cursor semantics) | Client+daemon: `git stash create` / `git write-tree` on temp index at send time. Native alternatives: C file checkpointing via control request [K verify], X `thread/rollback` (turns only) | L | daemon (Rust), `chat-model.ts`, `chat-pane.tsx` user bubble, docs |
| 8 | Composer context pills + image paste | `@file` becomes pill (removable); paste/drop image -> thumbnail chip | Cursor 2.0 pills [D]; images are table stakes | Pills: client only (token map). Images: C stream-json user content blocks `{type:image,source:base64}` supported; X `UserInput` type localImage/image [K verify]; A prompt `image` block if capability advertised | M (pills) / M (images) | `chat-composer.tsx`, adapters `send`, `chat-model.ts` |
| 9 | Approval upgrades | Edit approval shows diff preview inline; keyboard (Y/N/A); "always allow this tool" already via allow-session; per-session auto-approve toggle in header (maps to permission mode) | Reduces click-dance; Copilot `/yolo`, Cursor run modes | C: permission request has tool input (have `pendingInputs`) -> render diff from input. X/A: approval payload has command/diff (partly shown) | S-M | `chat-pane.tsx` perm card, adapters |
| 10 | Scroll UX: "jump to latest" pill, Ctrl+F in-chat search, collapsed thinking | Floating button when unstuck; find bar; thinking auto-collapses to "Thought for 8s" after done | Cheap polish; Copilot Ctrl+F [D] | Client only | S | `chat-pane.tsx`, `styles.css` |
| 11 | Long-chat handling: real virtualization + fork | Replace 300-item window with windowed list; "/fork" -> new chat with history; "start new chat" banner at high usage | Copilot `/fork` [R]. `content-visibility` already gives partial win | Client; fork: C `--fork-session` w/ resume [K verify], X `thread/fork` [K verify] | M-L | `chat-pane.tsx`, adapters |
| 12 | Side chat `/side` | Ephemeral tangent thread | Cursor-only; needs second agent process | - | L, skip |
| 13 | Notifications on done/needs-approval | OS notification when unfocused tab finishes or asks permission | Copilot notifies on confirm [D]; Kelpie multi-pane makes this valuable. Probably already partially exists in session status (not checked) | Have: busy->idle, permission item | S | store/notification layer (not inspected) |
| 14 | Plan mode UX: "Build" button on plan | When mode=plan and a plan item arrives: render markdown plan card with "Approve & build" (switch mode, send "go") | Cursor plan mode [D] | C: ExitPlanMode tool_use input.plan (permission request in plan mode) - render plan in approval card. X/A: plan item | S-M | `claude-adapter.ts` perm, `chat-pane.tsx` |

Not worth adopting: Cursor Projects/cloud canvases/remote iOS control (out of scope, cloud), Windsurf Memories/Workflows (agent-side), tool-count grouping, voice (Kelpie already has MicButton).

## 5. Top 8 picks (ranked)
1. **Tool-run grouping + collapsible terminal rows (#1)** - highest visual payoff per line of code; fully client-side; all three agents supply categories. Do first, it also shapes the data model (`category`) reused by #2.
2. **Turn-end changed-files summary (#2)** - closes every turn, entry point for review; data already collected; no protocol risk.
3. **Todo/plan widget (#3)** - universal across all 3 competitors; Claude TodoWrite is the dominant Claude progress signal and is currently invisible as a list.
4. **Real queue with cancel/steer (#4)** - correctness issue as well as UX: today queued text cannot be cancelled/reordered; three competitors converged on same model. M effort but contained in composer + adapters.
5. **Context meter + compact (#5)** - all CLIs degrade in long chats; competitor parity; needs per-protocol verification (see unresolved).
6. **Approval card with diff preview and keyboard shortcuts (#9)** - approvals are where the user is blocked; cheap win; data already in `pendingInputs`.
7. **Composer pills + image paste (#8)** - parity with Cursor 2.0 prompt UI; images are expected.
8. **Git-based checkpoints (#7)** - highest-value "safety net" and the only agent-agnostic one; L effort, so ranked below cheap wins, but should be scheduled because per-file Undo (#6) alone is a weak and bug-prone substitute (Cursor forum evidence).
Honorable: #10 scroll polish (S, bundle with #1), #13 notifications (verify existing).

Adoption risk: none of these depend on third-party libs; risk is protocol drift (Codex app-server method names, ACP `usage_update`) - mark as feature-detect, degrade silently (hide meter if no usage events).

## 6. Rejected or deferred reasoning
- Accept/reject gating (Cursor/Copilot "pending edits") is NOT achievable: Kelpie cannot hold agent writes. Only post-hoc revert (#6/#7). Don't promise accept/reject buttons; label "Revert".
- Windsurf-style revert is irreversible by their own docs; Kelpie's #7 should keep a Redo ref (Copilot model).

## 7. Limitations
- Official doc fetches mostly lacked UI specifics; Cursor "Explored" grouping, Windsurf accept-all bar, and all protocol field names marked [K] need verification against live CLI output (Kelpie has recorded fixtures in `app/src/chat/*.test.ts` and e2e scripts to check Claude fields; Codex/ACP schema via `codex app-server generate-ts` and ACP spec).
- Did not run the products; no screenshots. No Cursor 2.x intermediate changelog (2.1-2.6) read.
- Did not inspect notification code or Rust daemon capabilities.

## 8. Unresolved questions
1. Does Claude stream-json expose a stable per-turn context-token figure (assistant `usage` vs `modelUsage.contextWindow`) in the Claude version Kelpie spawns?
2. Codex app-server: confirm `thread/tokenUsage/updated`, `turn/plan/updated`, `turn/diff/updated`, `turn/steer`, `thread/compact/start`, `thread/fork` exist in the pinned version.
3. ACP: which agents (OMP) emit `usage_update` and image prompt capability?
4. Does Kelpie already raise OS notifications for chat sessions?
5. Is a daemon-side git snapshot acceptable for repos with large dirty trees (cost/time per send)?

Status: DONE_WITH_CONCERNS
Summary: Report written with feature inventory, Kelpie gap map (file:line), 14 ranked ideas and top-8 picks. Concern: official docs were thin on UI details and protocol field names for context/plan/steer events are from memory and flagged for verification.
