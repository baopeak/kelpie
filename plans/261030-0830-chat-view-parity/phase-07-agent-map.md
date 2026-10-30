---
phase: 7
title: "Agent map"
status: pending
priority: P2
effort: "12h"
dependencies: [3]
---

# Phase 7: Agent map

## Overview

A dialog listing every subagent of the conversation as a tree: live status, running time, tokens. Click one for details. Modelled on Claude Code's "Agent map" ("16 agents · click an agent for details").

Today only the transcript's "Agent" rows and the "In the background" strip exist. The strip drops tasks when they end (`applyTaskEvent`). Nested agents are skipped on purpose (`parent_task_id`, `owned_by_subagent`), and subagent messages return early (`m.parent_tool_use_id`, at `case "stream_event"` and `case "assistant"/"user"`).

## Requirements

- Functional:
  - Entry: a chip `Agents {n}` in the chat's composer row when `n > 0`, and a palette command "Show Agent Map".
  - Root box (session label, model, context in use) on the left; connected children on the right, nested agents under their parent.
  - Each agent: status dot (running pulses, done green, error red, stopped grey), description, `3m 49s · 90.9k tokens`.
  - Order: running, then error, then done, newest first within each.
  - Times tick while running and the dialog is open.
  - Click: details with type, status, duration, tokens, tool-use count, prompt, latest summary, activity list (recent tool calls), result (rendered markdown), and "Show in chat", which closes the dialog and scrolls to its row.
  - Esc: from details back to the list, then closes; never interrupts the agent. Use `window.addEventListener("keydown", fn, true)` with `stopPropagation`, as `context-menu.tsx` does: focus can return to the textarea, whose `onKeyDown` interrupts on Esc.
  - Works for Claude fully. For Codex and ACP it lists what they report as `agent` category tools, without tokens.
- Non-functional:
  - Built only from messages already received, so a replayed log rebuilds the same map. Never sends anything to the agent.
  - Normal transcript and `state.tasks` unchanged (the strip stays as is, and its tests pass).
  - `activity` capped at 200 entries per agent. Tool outputs not stored in the run; result comes from the transcript item.
  - No timer running when the dialog is closed.

## Architecture

Data already exists. The schema in `claude-adapter.test.ts` (claude 2.1.310) shows `task_started` with `tool_use_id` and `task_type`, and `task_progress` with `usage: { total_tokens, tool_uses, duration_ms }` and `summary`; `task_notification` has `status`. Subagent messages arrive with `parent_tool_use_id`.

New pure module `chat/agent-runs.ts`:

```ts
export interface AgentRun {
  id: string;            // the Agent/Task tool_use id
  taskId?: string;
  parent?: string;       // parent run id
  description: string;
  type?: string;         // subagent_type
  prompt?: string;
  status: "running" | "done" | "error" | "stopped";
  at?: number; endedAt?: number;
  tokens?: number; toolUses?: number; durationMs?: number;
  summary?: string;
  activity: { name: string; target: string; status: "running" | "done" | "error" }[];
}
export function startRun(list, tool, parent?): …;
export function applyRunTaskEvent(list, m): …;
export function applySubagentMessage(list, m): …;
export function mergeRuns(runs: AgentRun[], items: ChatItem[]): AgentRun[]; // adds agent-category tool items with no run
export function runTree(runs): { run: AgentRun; children: … }[]; // sorted
```

`ChatState` gains `agentRuns: AgentRun[]`, cleared by the same resets as `tasks` and on `/clear`. On agent exit, `running` becomes `stopped` (where `tasks` is emptied). The view reads `mergeRuns(state.agentRuns, state.items)`, so history-loaded conversations and other agents still list rows, and status of a top-level run prefers its tool item's status.

Times: `AdapterContext` gets `now()`, which returns the replay clock while replaying and `Date.now()` otherwise. `at` and `endedAt` use it, or the "identical replay" test cannot hold. A replayed log has `kelpie_time` lines; history may have none, so times can be absent.

## Related Code Files

- Create: `app/src/chat/agent-runs.ts`, `app/src/chat/agent-runs.test.ts`, `app/src/components/agent-map.tsx`
- Modify:
  - `app/src/chat/chat-model.ts` (state, resets, exit)
  - `app/src/chat/claude-adapter.ts`, `app/src/chat/claude-adapter.test.ts`
  - `app/src/components/chat-composer.tsx` (chip), `app/src/components/chat-pane.tsx` (Portal, `scrollToItem`), `app/src/lib/commands.ts` (palette command), `app/src/components/icons.tsx`, `app/src/styles.css`
  - `app/src/locales/vi/tools.json`

Check `app/phone/` builds: it uses `chat-model.ts`, and the new state field needs a default only.

## Implementation Steps

1. **Capture a real stream.** On the isolated daemon, run a real Claude chat that spawns two subagents, one of which spawns another if possible, with one in the background; exit cleanly. From `kelpied call` or the log, record:
   - `task_type` of a subagent (the test shows only `local_bash`),
   - whether `task_notification` has `usage` and `tool_use_id`,
   - the `tool_use_result` of a finished Agent call (`totalTokens`, `totalDurationMs`, `totalToolUseCount`, `agentId`),
   - how a nested agent's tool use looks.

   Save a trimmed fixture into `agent-runs.test.ts`, with content scrubbed. Record what differs from this plan in the phase file.
2. **`agent-runs.ts` with tests first** (see Tests).
3. **Claude adapter.**
   - In `applyClaudeMessage`'s tool_use handling and in `stream_event`'s `content_block_start`: for `Task`/`Agent`, `startRun` with description, type and prompt from `input` (stream start has no input; the completed message fills it).
   - In `applyTaskEvent`: **before** its filters, feed `applyRunTaskEvent`. Keep the existing strip logic byte for byte.
   - At the `m.parent_tool_use_id` early returns: for `assistant` and `user`, call `applySubagentMessage` and then return as before. Skip `stream_event` deltas: completed messages are enough. A nested `Agent` tool_use makes a child run.
   - On a top-level Agent `tool_result`: take tokens and duration from `tool_use_result` if the capture confirmed it; set `endedAt`.
   - Only runs with a linked Agent tool. Background shells do not appear (filter `task_type` per the capture).
4. **Dialog.** `AgentMapDialog` via `Portal` from `ChatBody` (as `UndoDialog`), using `.overlay` and `.modal`. List and detail are two states of one dialog.
5. **Layout, CSS only:** two-column flex; root box vertically centred; children column with `border-left` connector and a `::before` tick for each card, using `var(--border-strong)`. Cards: `.agent-card`, a `<button>`, description with ellipsis and full text in `title`. Nested: same pattern indented inside the column. The list scrolls inside the modal (`max-height: 80vh`). Under 560 px, stack the root above the list. Dots reuse `.tdot` with `.running`, `.error` rules; add a grey `.stopped`.
6. **Helpers.** `formatTokens` (`90.9k`, `1.2M`), `formatDuration` (`3m 49s`, `99m 3s`), shared with `elapsed` in `chat-pane.tsx` if the output fits. Timer: one `setInterval` while open and something is running, cleared in `onCleanup`.
7. **Details.** Fields as in Requirements. Prompt through `Clamped`, result through `Markdown` (phase 2's links work), from the tool item's `output`. "Show in chat" hides when the item is not found.
8. **Entry.** Chip in `composer-row`, next to the context meter, `data-testid`-free, using `.setting-btn` style; shows a pulse dot when something runs. Palette command in `commands.ts` next to `composer.open`, using `focusedSession`; notify if the focused session is not a chat. The dialog's state lives in `ChatBody`, so the command dispatches a `kelpie:chat-agent-map` window event with the session id, which `ChatBody` listens for, as it does for its other events.
9. Run checks, then the real-app script below.

## Tests

`agent-runs.test.ts`:

- Fixture from step 1 rebuilds the expected tree.
- Events out of order: `task_progress` before the tool_use; a notification for an unknown task; a duplicate `task_started`.
- Replaying the same lines from an empty list gives an identical result (attach re-feeds).
- Nested: child under parent; child arrives before parent is known; a cycle or a self-parent does not hang `runTree`.
- `activity` cap, oldest dropped.
- `mergeRuns` adds tool items without runs, and does not duplicate.
- Sort order.
- Exit: running becomes stopped.

`claude-adapter.test.ts`: subagent lines still produce no transcript items; the `state.tasks` tests unchanged; nothing written to the agent.

## Success Criteria

- [ ] Real Claude session with parallel subagents: chip shows the count, dialog shows them live, tokens and times increase, done turns green.
- [ ] A failed or stopped subagent shows red or grey.
- [ ] Nested agent appears under its parent.
- [ ] Click: details are correct; "Show in chat" lands on the row, also inside a folded run or beyond the window.
- [ ] Close and reopen the window: the map is the same (statuses, tokens), apart from what the log trim removed.
- [ ] Codex or ACP session with an agent tool lists it, without tokens, and no errors.
- [ ] Transcript looks unchanged; with 50 runs the dialog scrolls smoothly and streaming is not slowed (perf gate).

## Risk Assessment

- **Task event fields change between Claude versions.** The fields are read defensively, and a missing number shows nothing. Signal: blank tokens in the real app. Response: fall back to `tool_use_result`, then to none; do not parse transcripts.
- **Log trim loses a run's start.** `applyRunTaskEvent` creates a run from `task_progress`'s `description` when unknown.
- **Volume.** Subagent messages can be numerous. `applySubagentMessage` stores only a short target per tool call, and updates by index, not by copying the array of runs per message. Signal: a perf-gate regression during subagent-heavy streams. Response: batch updates per frame.
- **Prompt and result text** are rendered through the same sanitizer as replies; prompts as plain text.
