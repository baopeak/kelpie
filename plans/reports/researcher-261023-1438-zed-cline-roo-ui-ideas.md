# Chat UI ideas from Zed (ACP), Cline, Roo Code, Kilo Code -> Kelpie

Researcher: zed/cline/roo/kilo. Date 2026-10-23. Read-only on Kelpie.

## 1. Source credibility / caveats
- Zed docs (zed.dev/docs/ai/agent-panel, external-agents, tool-permissions): official, high. Zed source (thread_view.rs 627KB, acp_thread.rs 25k lines): read only partial chunks via WebFetch (first 100k chars of acp_thread, chars 0-100k and 300-400k of thread_view). Icons-per-kind, plan renderer, edits bar, queue layout NOT verified in source.
- ACP spec (agentclientprotocol.com tool-calls, agent-plan, prompt-turn, schema): official, high. Schema page read only first 100k of 158k chars; UsageUpdate fields (used/size) not directly seen, Cost{amount,currency} seen.
- Cline docs (checkpoints, auto-approve, plan-and-act, auto-compact): official. Cline webview-ui source NOT read (GitHub tree/API fetches 404/401). Task-header token/cost/context-bar detail is from third-party + official context-management page; medium.
- Roo Code: **repo archived 2026-05-15, extension shut down** (github.com/RooCodeInc/Roo-Code). Use only as pattern reference (modes docs still readable). Adoption risk if anyone proposes depending on it: abandoned; forks (ZooCode) unvetted.
- Kilo Code: rebuilt on OpenCode server, GA Apr 2026 (blog.kilo.ai, promotional; medium). Checkpoint docs official (kilo.ai/docs/.../checkpoints).

## 2. Feature inventory (evidence)

### Zed Agent Panel (hosts Claude/Codex/Gemini over ACP)
- ACP tool_call kinds: read/edit/delete/move/search/execute/think/fetch/switch_mode/other; statuses pending/in_progress/completed/failed; content = content | diff{path,oldText,newText} | terminal{terminalId}; `locations[{path,line}]` (spec: protocol/tool-calls).
- Zed internal statuses add WaitingForConfirmation, Rejected, Canceled (acp_thread.rs). Distinguishes user-rejected / user-canceled from failed.
- ToolCallLayout Standalone / Embedded / **Floating** (awaiting-permission row pinned above the editor, height-capped, scrollable) (thread_view.rs).
- Terminal card: header shows elapsed (live `started_at.elapsed()`, final `ended_at-started_at`), failed styling on non-zero exit/signal, truncated-output tooltip; auto-expands on NewTerminal, collapses when moved to background (thread_view.rs).
- Read output rendered as code block: strips `cat -n` prefixes (`parse_cat_numbered_code`), line-number gutter, syntax highlight, hover copy button.
- Edit card auto-expands on new diff; "Discard Interrupted Edit" undo button on canceled partial edit.
- Thinking block: "Thinking" header + chevron, left-border body, max-h-64 with fade, auto-expanded while streaming (auto-collapse in entry_view_state).
- Generating indicator with 1s timer, turn tokens (arrow up/down), per-turn duration + "N tokens" after finish.
- Review Changes: bar with files/lines changed; multibuffer to accept/reject per hunk or all (docs agent-panel).
- Checkpoint: "Restore Checkpoint" on user messages with edits (git-store checkpoint attached to UserMessage). Third-party claims external agents lack it (unverified).
- Queue: messages sent while generating are queued, editable/removable, can be sent immediately (message_queue.rs; docs). Edit any sent user message and regenerate (rewind-capable agents only).
- Mentions `@`: files, dirs, symbols, threads, skills, diagnostics, branch diff, URLs. Token usage meter + auto-compaction + `/compact`. ACP `compaction_update` / `compaction_summary_chunk` render as ContextCompaction entries.
- Permissions: Allow once / Deny once, "Always for <tool>", "Always for <pattern>" (regex rules always_allow/always_confirm/always_deny; terminal chains split and each sub-command checked); "Select options..." toggles individual command patterns; Allow disabled until confusable-Unicode warning acknowledged (docs tool-permissions; thread_view.rs).
- External agents: model/modes/auth owned by the agent (config_options). Import threads from agent history. Subagent card (subagent_card.rs). Elicitation UI (elicitation.rs, 93KB).
- ACP session updates (14): user/agent message chunk, agent_thought_chunk, tool_call(+update), plan, available_commands_update, current_mode_update, config_option_update, session_info_update, **usage_update**, notice, compaction_update, compaction_summary_chunk (spec: prompt-turn). Plan entries {content, priority, status}, full-replace each update (spec: agent-plan).

### Cline (docs only)
- Checkpoint bookmark after every tool use with **Compare** (diff vs checkpoint) and **Restore** menu: Restore Files / Restore Task Only / Restore Files & Task; editing an earlier message offers "Restore All" (docs.cline.bot/features/checkpoints).
- Auto-approve categories: read project / read all / edit project / edit all / safe commands / all commands / browser / MCP / notifications; OS notify when approval needed or an auto-approved command runs >30s; YOLO mode (docs auto-approve).
- Plan/Act toggle with per-mode model (docs plan-and-act). Auto-compact summarizing near context limit; task header with tokens, cost, context-window bar (third party + docs); Focus Chain todo list persisted through summarization (blog v3.25).

### Roo Code (archived)
- Modes: Code/Ask/Architect/Debug/Orchestrator; dropdown left of input; `/mode` slash switch; Ctrl+. cycles modes; each mode remembers last model (docs using-modes).

### Kilo Code (new, OpenCode-based)
- Per-turn collapsible changed-file list with +/- counts, click opens diff; hover user message -> **Revert to here**; revert banner with Redo / Redo All; sending while reverted makes it permanent (docs checkpoints).
- Diff reviewer with **inline line comments sent back to chat** as structured context (path, line, code) (blog). Agent Manager w/ worktrees (belongs to other researcher's lane).

## 3. Kelpie current state
- Item model `ChatItem` kinds user/assistant/thinking/tool/permission/system/result: chat-model.ts:7-15. Tool has only status running|done|error, `name`, `summary`, `input`, `output`, `diff[{path,text}]`; no kind, locations, timing, exit code, terminal id.
- ACP adapter: tool_call/update mapped at acp-adapter.ts:81-98 (drops `kind`, `locations`, content `terminal`); `plan` flattened to a `thinking` text item with ☑/☐ at acp-adapter.ts:99-102 (replaced items stack up: new id each update, not full-replace); diff built by prefixing every old line "-" and all new "+" (acp-adapter.ts:65) — no real hunk diff, no context lines. `usage_update`, `current_mode_update`, `session_info_update`, compaction events ignored (switch ends :109). Permission option kinds collapsed to allow/deny tone (:164) — `allow_always`/`reject_always` indistinguishable. Client caps fs/terminal off (:121).
- Claude adapter: result line shows duration/turns/cost only after finish (claude-adapter.ts:271-276); no per-turn tokens/context data surfaced; TodoWrite not special-cased (grep: no match).
- Codex adapter: has reasoning/plan/commandExecution/fileChange items (codex-adapter.ts:208-231), no token-usage notification handled.
- Tool UI: chat-pane.tsx:60-93 — single-line card, ✓/✕/◌ glyph, name+summary, chevron; body = JSON input + raw output (last 20000 chars), diff lines capped at 400 and auto-open while not done. No per-kind icon, no terminal styling, no copy, no line numbers, no syntax highlight.
- Thinking: always fully expanded italic text (chat-pane.tsx:164, styles.css:509).
- Permission: inline in transcript (chat-pane.tsx:170-187); scrolls away from the pending state; no keyboard shortcut; no "always" persistence; no sticky/floating.
- Busy indicator: static "Thinking…/Working…" (chat-pane.tsx:207-209) — no elapsed time, tokens.
- Composer: send while busy is allowed? `send` in chat-composer.tsx:109-115 always calls ctrl.send; ACP just fires another prompt (acp-adapter.ts:173-181) — no visible queue, no edit/remove. No message edit/rewind/checkpoint. No attachments/images. Mode/model/effort pickers exist as `SettingPicker` (:223-266), `/` `$` `@` completions OK (:18-25, :77-85) — parity there is already good.
- Scroll: stick-to-bottom flag only (chat-pane.tsx:108,145); no "jump to latest" button, no unread marker.
- Per-turn summary of changed files: none. Result line at end is plain text (stopReason).

## 4. Prioritized gap list
Effort: S <=0.5d, M 1-3d, L >3d.

| # | Idea | What it looks like | Why | Data / events | Eff | Kelpie files |
|---|------|--------------------|-----|---------------|-----|--------------|
| 1 | Structured tool cards by kind | Icon per kind (read/edit/execute/search/fetch/think), title = verb + path (basename + line from `locations`), status incl. rejected/canceled, elapsed time; edit cards auto-expand, others collapsed; hover Copy | Biggest daily-readability win; stops "name + JSON" look | ACP tool_call.kind/locations/status; Claude tool name (Read/Edit/Bash/Grep/WebFetch); Codex item types | M | chat-model.ts (extend tool item: `tool?: kind`, `locations`, `startedAt`, `endedAt`, `exitCode`), 3 adapters, chat-pane.tsx ToolCard, styles.css |
| 2 | Live plan / todo panel | Sticky collapsible strip above composer: "Plan 3/7" with entries (pending/in-progress/done), replaced in place | Today plan is a thinking blob that duplicates each update. Cline focus-chain / Zed plan_expanded | ACP `plan` (full replace); Claude TodoWrite tool input; Codex `turn/plan/updated` / `plan` item | S-M | new `plan` field in ChatState (not an item), acp-adapter.ts:99, claude-adapter (TodoWrite), codex-adapter :213, chat-pane.tsx |
| 3 | Context/usage meter + live turn status | Composer-row chip "42% · 84k/200k · $0.31"; busy line "Working… 1m12s · 3.1k tokens"; warn colour >80%; `/compact` hint | Cline task header, Zed token callout. Users need to know when to compact/new session | ACP `usage_update` {used,size} + cost; Claude `result.usage`/`modelUsage` & stream `message_delta.usage`; Codex `thread/tokenUsage/updated` (verify name) | M | ChatState `usage`, adapters, chat-composer.tsx row, chat-pane.tsx:207 |
| 4 | Permission UX: floating/sticky + keyboard + "always" | Pending approval pinned above composer (height-capped, tool card embedded), Y/A/N shortcuts, buttons keep ACP option kinds (Allow once / Always / Reject); resolved collapses into the tool card as "Allowed" chip; OS notification if window unfocused | Zed Floating layout; Cline notifications. Approvals buried in long transcript block agent silently | ACP `request_permission.options[].kind` (allow_once/allow_always/reject_*), Claude control_request `permission_suggestions`, Codex approval decisions | M | chat-model.ts permission item (`kind` per option), chat-pane.tsx:170, adapters respond(), styles; notification via Tauri plugin (check existing) |
| 5 | Per-turn "changes" summary + real diffs | After each turn with edits: "3 files +42 -7" collapsible list; click file -> diff with hunks/context, line numbers; Keep/Revert later | Kilo/Zed Review Changes. Current diff is whole-file -/+ (acp-adapter.ts:65) which is unreadable for edits | ACP diff{oldText,newText} -> compute hunks client-side (e.g. `diff` npm lib); Claude Edit old/new_string; Codex fileChange unified diff | M | new `diff-view` component, chat-pane.tsx, acp/claude diff builders; reject/revert = L (needs git/daemon support) |
| 6 | Thinking block polish | "Thinking" header + chevron, auto-open while streaming, auto-collapse on first assistant/tool item, max-h with fade | Zed render_thinking_block. Currently long italic walls | existing thinking items | S | chat-pane.tsx:164, styles.css:509 |
| 7 | Message queue while busy (+ steer/send-now) | Chips above composer for queued prompts, edit/remove/send-now; sent automatically on turn end | Zed message_queue.rs; avoids today's silent concurrent prompt in ACP | client-side; Claude stream-json accepts mid-turn input; ACP needs wait for stopReason; Codex `turn/steer`? (verify) | M | chat-model.ts (queue in ChatState + controller), adapters send(), chat-composer.tsx |
| 8 | Checkpoints / rewind per user message | Hover user msg -> "Restore files to here" / "Edit & resend"; Kilo-style revert banner with Redo | Cline/Kilo/Zed all have it; safety net for autonomous edits | Agent-native: Claude `--resume-session-at` + file checkpointing (verify headless support); else daemon git snapshot (stash/ref per turn) | L | daemon (kelpied) snapshot RPC, chat-model.ts, chat-pane.tsx |
| 9 | Terminal embedded in execute tool | Command header, live output (streamed), exit-code badge, elapsed, truncation note; kill button | Zed terminal card | Kelpie currently declines ACP terminal cap, so agent runs its own shell and output arrives as content/rawOutput; Codex `commandExecution/outputDelta` already in adapter :378. Implementing ACP `terminal/*` = L; styling existing output = S | S (style) / L (ACP terminal) | chat-pane.tsx ToolCard, styles.css; acp-adapter.ts:121 only if L path chosen |
| 10 | Mode quick-cycle + visible mode badge | Shift+Tab / Ctrl+. cycles permission/plan mode; badge colour in composer; apply `current_mode_update` | Roo Ctrl+., Cline Plan/Act; Kelpie has mode picker but ignores `current_mode_update` (agent-initiated mode switch e.g. plan->act is invisible) | ACP `current_mode_update`; Claude `system` init permissionMode / control_response | S | acp-adapter.ts onUpdate switch, chat-composer.tsx keydown |
| 11 | Auto-approve presets | Chat-bar popover: read / edit in project / safe commands / all; per-session | Cline auto-approve categories; Zed regex rules. Mostly duplicates the permission-mode picker; real value is "safe commands" granularity | Claude `--allowedTools`/permission rules via control; Codex approvalPolicy | M | settings + adapters; consider after #4 |
| 12 | Jump-to-latest button, long-output handling | Floating "↓ New" pill when scrolled up; tool output virtualised/"Show full" | Standard; current stick logic has no affordance | none | S | chat-pane.tsx:145 |
| 13 | Resource links / richer mentions (`@` dirs, diagnostics, branch diff, URL), image paste | Chips in composer; ACP `resource_link` content blocks instead of plain "@path" text | Zed mentions breadth | ACP prompt content types | M | chat-composer.tsx, acp-adapter.ts:28 |
| 14 | Inline diff comments -> chat | Click diff line, comment, "Send all to chat" with path:line:code | Kilo unique; builds on #5 | client-only | M (after #5) | diff-view |
| 15 | Compaction display | `compaction_update` card "Compacting context..." | Zed ContextCompaction; needs client cap `session.compaction` | ACP | S (after #3) | acp-adapter initialize caps |

## 5. Top 8 picks (ranked, with reasoning)
1. **Structured tool cards by kind (#1)**: every turn is mostly tool calls; it is the single largest quality delta vs Zed and costs one data-model extension that unblocks #4, #5, #9. All three protocols already carry the info (ACP kind/locations, Claude tool names).
2. **Live plan panel (#2)**: cheap, high visibility on long tasks; fixes an actual bug-like behavior (each ACP plan update appends a new thinking item).
3. **Context/usage meter + elapsed (#3)**: Kelpie shows nothing about context pressure; cost shown only post-turn. Users running long agents need it; data already flows (ACP usage_update, Claude usage).
4. **Permission UX (#4)**: blocking approvals buried in scrollback is a real workflow failure; preserving ACP allow_always vs reject_always is nearly free. Add OS notify when unfocused.
5. **Real diff view + per-turn change summary (#5)**: current -/+ whole-file diffs mislead; compute hunks client-side. Defer revert (L) to #8.
6. **Thinking polish (#6)**: S effort, removes visual noise immediately; do alongside #1.
7. **Message queue (#7)**: ACP adapter currently fires overlapping prompts while busy; a queue is both a correctness fix and UX win.
8. **Mode sync + cycle shortcut (#10)**: S effort; agent-initiated `current_mode_update` is currently dropped, so the picker can show a stale mode. Checkpoints (#8 in table) intentionally left out of top 8 because L effort and agent/daemon-dependent; propose as phase after diff view.

Quick-win bundle order: #6 + #12 + #10 (S) -> #1 -> #2 -> #3 -> #4 -> #5 -> #7.

## 6. Adoption risk / fit notes
- Roo Code is sunset: copy ideas, do not depend. Kilo/Cline chat UIs are VS Code webviews (React); nothing to import into SolidJS. Zed is GPL/AGPL-licensed code (Rust/GPUI): read-for-design only, do not port code. (License not re-verified here; check before any copying.)
- Fit: Kelpie runs one adapter-normalized `ChatItem` list; all ideas above need either an extended item (tool) or non-item state (plan, usage, queue). Keep that split to stay DRY across 3 adapters.

## 7. Limitations / unresolved questions
- Zed source read only in partial chunks; tool-kind icons, plan summary text, edits bar and queue layout unverified in code (docs only).
- Cline/Roo/Kilo webview source not read (GitHub fetch blocked); their UI details come from docs/blogs.
- Not verified: exact ACP `usage_update` fields (used/size), whether OMP emits `usage_update`/`current_mode_update`, whether Codex app-server emits token-usage and plan-update notifications under the names assumed, whether Claude stream-json headless supports file checkpoints/rewind (`--resume-session-at`) or mid-turn steering.
- Does the user want revert/checkpoint (L, daemon work) in scope, or review-only?
- Should Kelpie implement ACP `terminal/*` client capability (agent output then streams through Kelpie, gaining kill/timing) or keep the agent's own tools for TUI parity?
- Is there an existing OS-notification path in the Tauri app for #4?
- Syntax highlighting dependency choice (shiki vs highlight.js) not evaluated; affects bundle size and perf gate.

Status: DONE_WITH_CONCERNS
