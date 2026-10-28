# Codex app UI -> Kelpie chat view ideas

Date 2026-10-23. Scope: Codex desktop app (now a mode inside the ChatGPT desktop app since 2026-07-09), IDE ext, app-server protocol (codex-cli 0.163.0, schema generated locally with `codex app-server generate-ts`, no model turn run).

## 1. Sources and credibility

| Source | Weight |
|---|---|
| Local `codex app-server generate-ts` output (ServerNotification, ClientRequest, ThreadItem v2) | Highest: ground truth for data available |
| Codex changelog https://learn.chatgpt.com/docs/changelog (old: developers.openai.com/codex/changelog, 308-redirects) | Official, dated |
| Shortcuts https://learn.chatgpt.com/docs/reference/commands.md ; slash cmds .../reference/slash-commands.md | Official |
| IDE page https://learn.chatgpt.com/docs/codex/ide | Official but thin |
| app-server README https://github.com/openai/codex/blob/main/codex-rs/app-server/README.md | Official, little on UI semantics |
| Community: steer/queue https://community.openai.com/t/a-practical-codex-app-steer-workflow-splitting-a-task-into-staged-follow-ups/1377757 ; automations https://codex.danielvaughan.com/2026/04/08/codex-desktop-automations/ | Medium/low (3rd party) |

Not found: screenshots/hands-on of exact transcript cell styling. Docs pages for app features/review are index-only post-redirect. Transcript-layout claims below are inferred from protocol + changelog, flagged where so.

## 2. Codex app feature inventory (evidence)

Transcript/inspection
- Review pane: git diff of local/worktree checkout, inline comments (collapsible, inline or detached, @mentions + skills inside comments), stage/revert per hunk or file, commit/push/PR from app. Changelog 2026-03-11, 03-19, 04-09, 04-16, 02-26; community/docs summary.
- Review-tab shortcut Ctrl+Shift+G; file tree Cmd+Shift+E; Cmd+P file search; per-thread terminal tabs (04-12/04-16); Codex can read thread terminal (03-11).
- Thread side panel: Git summary, Sources, plans, artifacts (04-09, 04-16); file tabs (04-10); image/PDF/md previews (04-12); image lightbox (05-05).
- IDE ext: "Review" button + "Undo" button, "read a summary, inspect focused diff of changed lines, keep edits you want, follow up in same chat"; Git checkpoints before/after tasks (IDE page).
- Per-thread scroll position preserved (04-09). Find-in-chat Cmd+F/G.

Running work
- Mid-turn steer (02-05); follow-ups queue by default (02-08); drag-reorder of queued messages (02-17); Settings > General > Follow-up behavior = Steer|Queue (community). Queued items shown as list above composer with per-item steer/delete (Claude Code issue describing Codex, third-hand).
- Parallel approvals (02-10), improved approval prompts (03-03), "don't ask again" (04-01), auto-approval review status + risk level (04-23). Approve = Enter, Decline = Esc when request open (shortcuts page).
- Plan mode: /plan, notification for plan questions (03-18). `/goal` persistent goal with pause/resume/edit/clear controls above composer.
- Notifications: unread indicators, "Next chat needing attention" Cmd+Alt+A, Shift+Esc clear unread, notification sound (09-22), tray usage limit (04-20), "Unread chats" in command menu (06-11).

Threads / worktrees
- New-thread mode selector Local | Worktree | Cloud; branch picker w/ search (02-10, 04-20); Handoff Local<->Worktree (03-03); Codex-managed worktrees w/ retention setting (02-27, 03-03).
- Fork conversation, fork from earlier message (02-12, 03-18), `/fork` to new chat or worktree; `/side` temporary side chat (Cmd+Alt+S); pop-out window; PR status badges on task rows (02-27); search past threads (03-24); pin, rename, archive, mark unread shortcuts; Activity view (07-30); multi-window.
- Review queue / Triage inbox for automation + worktree threads: approve (stage/commit/PR), revise, reject (community).

Composer
- `@` fuzzy file (02-17), skills in `@` menu (03-19), `$` skill, `/` commands; slash cmds `/model` `/reasoning` usable mid-draft (03-18); model picker Ctrl+Shift+M, recent models Ctrl+Cmd+M; Up restores previous prompt; "Add context" menu w/ MCP submenu (02-26); non-image file paste (04-20); image inputs; floating composer v2 (03-20); voice/dictation; Composer Predictions: Tab accepts suggested follow-up as editable draft (10-09, beta).
- `/status` = chat id + context usage + rate limits; `/compact`; `/review` (uncommitted or base branch); `/init`; `/mcp`; `/fast`; `/approve` (retry auto-review denial).

## 3. What the protocol gives a UI (verified locally; ServerNotification / ClientRequest names)

Notifications: `turn/plan/updated {explanation, plan:[{step,status}]}`, `turn/diff/updated {diff: aggregated unified diff for turn}`, `thread/tokenUsage/updated {total,last,modelContextWindow}`, `account/rateLimits/updated {primary/secondary {usedPercent, windowDurationMins, resetsAt}}`, `thread/status/changed` (+ activeFlags waitingOnApproval|waitingOnUserInput), `thread/queue/changed` (threadId only), `thread/prediction/updated`, `thread/goal/updated`, `item/fileChange/outputDelta`, `item/fileChange/patchUpdated`, `item/mcpToolCall/progress`, `item/autoApprovalReview/*`, `thread/reverted`, `thread/name/updated`, `hook/started|completed`, `warning`, `model/rerouted`, `serverRequest/resolved`, `item/commandExecution/terminalInteraction`.
Requests: `turn/steer {threadId, input, expectedTurnId}`, `thread/fork {lastTurnId?}`, `thread/revert {beforeTurnId}` (history only, NOT files), `thread/name/set`, `thread/goal/*`, `thread/attachment/*`, `thread/list`, `thread/turns/list`, `thread/items/list` (pagination), `gitDiffToRemote`, `fuzzyFileSearch`, `fs/*`, `account/rateLimits/read`, `review/start {target: uncommittedChanges|baseBranch|commit|custom, delivery}`.
Items: `commandExecution` has `commandActions[]` (read|listFiles|search|unknown) = what lets Codex UI collapse "Explored 3 files, searched 2" groups; `agentMessage.phase` = commentary|partial_answer|final_answer; `userMessage.content` includes image/localImage/mention/skill; `fileChange.changes[{path,kind,diff}]`; `imageView`; `enteredReviewMode/exitedReviewMode`; `plan`; `reasoning`.
Approval params: command, cwd, commandActions, reason, `proposedExecpolicyAmendment`, `networkApprovalContext`, decisions incl. `acceptWithExecpolicyAmendment`, `cancel`; file change: reason, grantRoot.

## 4. Kelpie current state (file:line)

- Transcript = flat list of 7 ChatItem kinds, `chat-model.ts:7-15`. No turn grouping; each turn ends with centered `result` line "completed · 12.3s" (`codex-adapter.ts:361`, `chat-pane.tsx:201-203`).
- Tool cell: one-line head + collapsed body, `chat-pane.tsx:60-93`; Codex commands all named "Shell", `commandActions` ignored (`codex-adapter.ts:216-225`); output kept as raw tail 20k chars, no ANSI; no per-command elapsed/exit code.
- Diff: per-fileChange unified diff, line-colored, first 400 lines (`chat-pane.tsx:71-84`); `turn/diff/updated`, `item/fileChange/outputDelta` ignored (`codex-adapter.ts:351-386` default drops them). No apply/undo/review.
- Plan: `plan` item and (not handled) `turn/plan/updated`; plan item rendered as italic "thinking" text (`codex-adapter.ts:213-215`). No checklist.
- Reasoning: italic block always expanded, no collapse (`chat-pane.tsx:164-166`, css `.msg-thinking` styles.css:509). `phase` ignored.
- Running state: single "Thinking…/Working…" line (`chat-pane.tsx:207-209`); no elapsed timer, no current-action label. Interrupt: Esc and Stop button (`chat-composer.tsx:147-149,209`).
- Queue/steer: none. `send()` while busy calls `startTurn` -> `turn/start` mid-turn (`codex-adapter.ts:388-398`); `turn/steer` unused. Only pre-thread queue (`:67,390-394`) shown as dim bubble.
- Approvals: inline card, fixed 3 buttons Allow/Allow for session/Decline, detail = bare command (`codex-adapter.ts:332-349`, `chat-pane.tsx:170-187`). No Enter/Esc binding, no cwd/reason/execpolicy amendment, no diff preview for file approvals, no auto-review risk. User-input requests -> "switch to terminal" (`:346-348`).
- Composer: `/ $ @` menus, model/effort/mode pickers, mic, Stop, Send (`chat-composer.tsx:153-217`). No image/file attach/paste (userMessage image content dropped: `codex-adapter.ts:203` text only), no Up-history, no context meter, no draft persistence per session, no `/plan`, `/fork`, `/status`, `/side` (all in TUI_ONLY `:39-43`).
- No token/context usage, no rate limit display, no thread rename/fork/revert, no notifications on approval or completion.
- Perf: last 300 items window + `content-visibility:auto` (`chat-pane.tsx:15,123`, styles.css:530), streaming markdown split at stable block (`:34-58`). Good already.
- Scroll: stick-to-bottom <40px, no "jump to latest" pill (`chat-pane.tsx:125-131,145`).

## 5. Prioritized gap list

Effort S<=0.5d, M=1-3d, L>3d. "Proto" = Codex protocol supplies it.

1. **Queue + steer while running** (M). Looks like: while busy, Enter queues message as chip stack above composer (reorder/delete, "Steer now" button); setting Follow-up = Queue|Steer. Why: biggest daily-flow win; today mid-turn send hits `turn/start` which is probably rejected/raced (unverified). Proto: `turn/steer {expectedTurnId}` yes; queue is client-side (only `thread/queue/changed` exists, no params) - implement queue in Kelpie, flush on `turn/completed`. Caveat: review/compact turns reject steer (OpenClaw docs). Files: `codex-adapter.ts` (send/interrupt/turn/completed), `chat-model.ts` (add `queue` to ChatState + Adapter.steer?), `chat-composer.tsx`, styles. Claude stream-json also supports mid-turn user messages, so Adapter interface method is cross-agent.

2. **Live plan checklist** (S-M). Pinned card above composer or inline: steps with pending/inProgress/completed icons, explanation text. Why: users see progress at a glance; Codex app has task sidebar with plans. Proto: `turn/plan/updated` yes (ignored today). New ChatItem `{kind:"plan", steps[]}` upserted by turnId; same renderer can serve Claude TodoWrite and ACP plan. Files: `chat-model.ts`, `codex-adapter.ts`, `chat-pane.tsx`, css.

3. **Turn-level changed-files summary + diff review** (M-L). At turn end: card "3 files +42 -7" with per-file collapsible colored diff (reuse existing diff renderer, add +/- counts, side-by-side optional later), button "Review in IDE/diff tab", "Revert". Why: the Codex/IDE "Review" + "Undo" is the signature loop. Proto: `turn/diff/updated` aggregated diff yes; revert files NO (thread/revert only edits history) -> implement via `git apply -R` of the turn diff or git checkpoint (Kelpie has a daemon; needs a git command path). Inline comments -> sent as next prompt with file:line (client-only, L). Phase A (S): counts + aggregated diff card; Phase B (M): per-file revert via `git apply -R`; Phase C (L): inline comments. Files: `codex-adapter.ts`, `chat-model.ts`, `chat-pane.tsx` (extract DiffView), maybe new `components/diff-view.tsx`, daemon git op.

4. **Better approvals** (M). Enter=approve, Esc=decline when card focused (Codex shortcut); show cwd + reason + command in code box; file-change approval shows the pending diff (`item/fileChange/outputDelta`/item.changes); extra option "Always allow this command" via `acceptWithExecpolicyAmendment` when `proposedExecpolicyAmendment` present; sticky banner / notification when approval pending off-screen. Proto: yes all. Also handle `item/tool/requestUserInput` (questions with choices) inline instead of "switch to terminal" (plan mode questions). Files: `codex-adapter.ts:332-349`, `chat-pane.tsx:170-187`, `chat-model.ts` permission options need `payload`.

5. **Context + rate-limit meter and /status** (S). Composer row: "ctx 34%" ring from `total.totalTokens` (or `last`) / `modelContextWindow`, tooltip with breakdown; hover shows 5h/weekly `usedPercent` + reset time; `/status` renders as system card; warning at 90% with "Compact" button. Proto: `thread/tokenUsage/updated`, `account/rateLimits/updated` yes. Files: `chat-model.ts` (state.usage), `codex-adapter.ts`, `chat-composer.tsx` row. Claude result events carry usage too -> common field.

6. **Running-state rework: elapsed timer + current action + grouped exploration** (M). Typing line becomes "Working 1m 12s · Running `npm test`" ; `result` line shows duration; consecutive read/search/list `commandExecution` items (via `commandActions`) collapse into one "Explored 5 files, 2 searches" row, expandable; commands show exit code + duration; ANSI stripped/rendered. Why: transcript density; Codex collapses exploration (inferred from commandActions design, not directly documented). Proto: yes (`turn/started` time local, `commandActions`, `durationMs`). Files: `codex-adapter.ts:216-225`, `chat-model.ts` (tool.kind/actions/startedAt), `chat-pane.tsx` (group in memo over items).

7. **Reasoning/commentary folding** (S). Collapse thinking by default to a one-line "Thought for 8s" / first summary line, expand on click; render `phase:"commentary"` agent messages dimmer/compact vs `final_answer` full. Streaming stays open. Files: `chat-pane.tsx:164`, `codex-adapter.ts:205-207`, css.

8. **Image + file attach / paste** (M). Paste/drop image -> thumbnail chips in composer; sent as `localImage {path}` (save clipboard to temp via Tauri) ; user bubble shows thumbs (today image content dropped `codex-adapter.ts:203`). Non-image file paste -> `mention` input item. Proto: `UserInput.localImage|image|mention` yes. Claude: image content block; ACP: image block. Files: `chat-composer.tsx`, `chat-model.ts` (send(text, attachments)), adapters, Tauri fs for temp save.

9. **Composer quality**: Up arrow restores last prompt; per-session draft persistence; `/model` `/reasoning` map to pickers mid-draft; `/plan`, `/fork`, `/status`, `/side` promoted out of TUI_ONLY; Ctrl+Shift+M open model picker. (S-M) Files: `chat-composer.tsx`, `codex-adapter.ts:30-43`. Note `/plan` toggles collaboration mode via `turn/start collaborationMode` (verify in schema before building).

10. **Fork / rename / revert-to-here per message** (M). Hover on user message: "Fork from here" (`thread/fork {lastTurnId}` -> open in new chat tab/pane), "Edit & resend" (`thread/revert beforeTurnId` + files warning). Rename thread (`thread/name/set`, `thread/name/updated`) feeds tab title. Files: `codex-adapter.ts`, `chat-pane.tsx`, tab-title store, pane creation (`mode-switch.ts`/store). Revert-history-only caveat must be explicit in UI.

11. **Attention + notifications** (S-M). Tab/pane badge when waiting on approval/input (`thread/status/changed` activeFlags) or turn done in background; OS notification + optional sound; "next session needing attention" shortcut; Shift+Esc clear. Kelpie already tracks session status for sidebar/terminal (idle/working/waiting dots) - reuse by feeding chat state into same store. Files: `codex-adapter.ts`, `store`, `sidebar.tsx`.

12. **Scroll UX**: "Jump to latest" pill when unstuck; preserve scroll per session (Codex 04-09); Find-in-chat Ctrl+F (M). Files: `chat-pane.tsx:125-145`.

13. **Worktree mode at new-chat time + Handoff** (L). Local|Worktree selector, branch picker, PR badge. Kelpie multi-agent roadmap likely owns this (docs/plans); protocol has no worktree API - Codex app does it client-side with git. Defer to roadmap.

14. Composer Predictions (Tab accepts suggested follow-up; `thread/prediction/updated`) (S once protocol shape checked; beta, low priority). `/goal` controls (`thread/goal/*`) (M, low). Automations/triage inbox (L, out of scope for chat view). Appshots/browser/computer-use (out of scope).

## 6. Top 8 picks, ranked

1. **Queue + steer** (#1): fixes a likely correctness hazard (mid-turn `turn/start`) and the most-used flow; protocol-backed; cross-agent.
2. **Turn diff summary -> review/revert** (#3 phase A+B): the core "agent UI" loop; data already arrives (`turn/diff/updated`), only revert needs git; avoids relying on Undo that protocol lacks.
3. **Approvals upgrade** (#4): safety-critical surface; Enter/Esc, context, execpolicy "always allow", user-input questions. Removes the "switch to terminal" dead end for plan-mode questions.
4. **Plan checklist** (#2): cheap, high visible value, protocol event exists and currently dropped.
5. **Context/rate-limit meter + /status** (#5): cheap, prevents surprise compaction and limit hits; CLI parity.
6. **Running state + exploration grouping** (#6): density/perception of speed on long threads; builds on existing perf work.
7. **Images/attach** (#8): real parity gap (TUI/app both accept images; Kelpie drops them silently).
8. **Reasoning fold + commentary phase** (#7): S effort, declutters transcript. (Notifications #11 is 9th; swap in if Kelpie's multi-session manager lacks attention routing.)

Shared-design note (for debate with other researchers): items 2,3,5,6 need adapter-neutral ChatItem additions (`plan`, `turnSummary`, `usage`, tool `actions/startedAt`). Add them to `chat-model.ts` once so Claude/ACP adapters can fill them; do not make Codex-only UI.

## 7. Trade-offs / adoption risk

- App-server protocol is marked experimental ("[experimental]" in `--help`), shapes change fast (guardian review payload flagged "expected to change soon"). Mitigation: Kelpie already reads fields defensively; keep new handlers tolerant, regenerate schema per Codex bump.
- Docs domain moved (developers.openai.com -> learn.chatgpt.com, 308); old links may break.
- Revert/undo is not a Codex protocol feature; the app uses git checkpoints. Building own `git apply -R` has dirty-tree conflict risk; needs "files changed since" guard.
- Codex app features (automations, Sites, Pets, browser, computer use, cloud) mostly out of scope for a chat view.

## 8. Limitations

- No hands-on of the actual Codex app (not installed; Windows build exists since 2026-03-04). Visual details (cell density, grouping labels) inferred.
- Docs feature/review pages fetched as index-only; review-pane specifics come from changelog + 3rd party summaries.
- Did not run any turn, so mid-turn `turn/start` behavior in app-server and `collaborationMode` shape unverified.
- Other products (Claude Code ext, Cursor, Zed/Cline, managers) left to peers.

## Unresolved questions

1. What does app-server do on `turn/start` while a turn is active (reject, queue, interleave)? Needs a safe live test or reading `codex-rs/app-server` source.
2. Is `/plan` a `turn/start` `collaborationMode` param? Not confirmed in schema excerpt.
3. Can the turn diff be reverted reliably in Kelpie's daemon (git available per project? non-git folders?)
4. Where does `thread/queue/changed` fit - is there a server-side queue manual-start request (README mentions "manual queue start")? Would let Kelpie avoid a client-side queue.
5. Should diff review live in the chat pane or a dedicated pane/tab (Codex uses a separate review tab, Ctrl+Shift+G)? Depends on Kelpie layout roadmap.
6. Exact `thread/prediction/updated` result shape and whether Predictions work through app-server for third-party clients.

Status: DONE_WITH_CONCERNS
Summary: Report written with feature inventory, protocol data map (verified from locally generated app-server schema), Kelpie gap with file:line, 14 ranked ideas and top 8. Concerns: no hands-on of the Codex app UI, docs feature pages were index-only, and mid-turn `turn/start` behavior is unverified.
