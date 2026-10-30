---
phase: 7
title: "Agent map"
status: completed
priority: P2
effort: "12h"
dependencies: [3]
---

# Phase 7: Agent map

## Overview

A dialog listing every subagent of the conversation as a tree: live status, running time, tokens. Click one for details. Modelled on Claude Code's "Agent map" ("16 agents · click an agent for details").

Today only the transcript's "Agent" rows and the "In the background" strip exist. The strip drops tasks when they end (`applyTaskEvent`). Nested agents are skipped on purpose (`parent_task_id`, `owned_by_subagent`), and subagent messages return early (`m.parent_tool_use_id`, at `case "stream_event"` and `case "assistant"/"user"`).

## Captured stream (claude 2.1.314, 2026-10-31)

Step 1 below is done. Two parallel agents, one nested, one background, on the small model; the raw log is `D:\kelpie-chat-e2e\scratch\agents-stream.jsonl` (test content only). **It changes this plan:**

- **The Agent tool's result says nothing about the agent.** Every top-level call returned at once with `tool_use_result: { isAsync, status: "async_launched", agentId, description, resolvedModel, prompt, outputFile, canReadOutputFile, canContinueAgent }`, even without `run_in_background`, so its tool row is `done` while the agent still runs. There are no `totalTokens`/`totalDurationMs` fields. A run's status therefore comes from task events, never from its tool item; `mergeRuns` must not prefer the item's status for a run it knows from task events. A nested call with `run_in_background: false` had a plain `tool_result` with no `tool_use_result`.
- `task_started`: `task_id`, `tool_use_id`, `description`, `subagent_type`, `prompt`, `is_backgrounded` (true also for agents the tool launched in parallel), `spawn_depth`, `task_type: "local_agent"`, and `parent_task_id` on a nested one. Filter on `task_type === "local_agent"`; background shells have another type.
- `task_progress`: `task_id`, `tool_use_id`, `usage: { total_tokens, tool_uses, duration_ms }`, `last_tool_name`, and a `description` that is **the current activity** ("Reading notes.txt"), not the agent's name. Keep the name from `task_started`; show this one as its latest activity.
- `task_notification`: `task_id`, `tool_use_id`, `status: "completed"`, `summary` (**the agent's final answer**), `output_file`, `usage`. This is the result shown in the details, and its usage is the final count. Other statuses (`failed`, `stopped`) were not seen: map known ones and show anything else as its text.
- `task_updated`: `task_id`, `patch`, comes just before the notification. Ignorable.
- A subagent's own tool calls arrive as `assistant` lines with `parent_tool_use_id` set to the Agent call's id. A nested Agent call appears the same way, before its `task_started`.
- Order: the Agent `tool_use`, then `task_started`, then the tool's `tool_result`, then progress. `task_started` for a nested one can come after its parent's `task_progress`. A notification comes before the parent's final tool result.

## Requirements

- Functional:
  - Entry: a chip `Agents {n}` in the chat's composer row when `n > 0`, and a palette command "Show Agent Map".
  - Root box (session label, model, context in use) on the left; connected children on the right, nested agents under their parent.
  - Each agent: status dot (running pulses, done green, error red, stopped grey), description, `3m 49s · 90.9k tokens`.
  - Order: running, then error, then done, newest first within each.
  - Times tick while running and the dialog is open.
  - Click: details with type, status, duration, tokens, tool-use count, prompt, latest summary, activity list (recent tool calls), result (rendered markdown), and "Show in chat", which closes the dialog and scrolls to its row.
  - Esc: from details back to the list, then closes; never interrupts the agent. Use `window.addEventListener("keydown", fn, true)` with `stopPropagation`, as `context-menu.tsx` does: focus can return to the textarea, whose `onKeyDown` interrupts on Esc.
  - Works for Claude fully. For Codex it lists `collabAgentToolCall` items with `tool: spawnAgent` as `agent` category tools, without tokens. ACP has no agent kind in its `ToolKind`, so it does not apply there.
- Non-functional:
  - Built only from messages already received, so a replayed log rebuilds the same map. Never sends anything to the agent.
  - Normal transcript and `state.tasks` unchanged (the strip stays as is, and its tests pass).
  - `activity` capped at 200 entries per agent. Tool outputs are not stored in the run. The result is the notification's `summary`; only a run without one falls back to its transcript item's output (never the launch notice).
  - No timer running when the dialog is closed.

## Architecture

Data already exists, as captured above: a run's name, type and prompt come from its Agent call and `task_started`, its numbers and current activity from `task_progress`, its end and answer from `task_notification`. Subagent messages arrive with `parent_tool_use_id`. An older stream (no `task_type`, or totals in `tool_use_result`) is read defensively: a missing number shows nothing.

New pure module `chat/agent-runs.ts`:

```ts
export interface AgentRun {
  id: string;            // the Agent/Task tool_use id
  taskId?: string;
  parent?: string;       // parent run id
  parentTask?: string;   // parent_task_id, for a parent known only by its task
  description: string;   // the name, from the call or task_started
  type?: string;         // subagent_type
  prompt?: string;
  status: "running" | "done" | "error" | "stopped";
  statusText?: string;   // an ended status not known here, as the agent put it
  at?: number; endedAt?: number;
  tokens?: number; toolUses?: number; durationMs?: number;
  summary?: string;      // the current activity, from task_progress
  result?: string;       // the final answer, from task_notification
  activity: { id: string; name: string; target: string; status: "running" | "done" | "error" }[];
}
// These change the list in place, inside a store's `produce`: nothing copies it per message.
export function startRun(list, tool, parent?, at?): AgentRun;
export function applyRunTaskEvent(list, m, at?): void;
export function applyRunResult(list, block, meta, at?): void;
export function applySubagentMessage(list, m, at?): void;
export function mergeRuns(runs: AgentRun[], items: ChatItem[], busy?: boolean, previous?: AgentRun[]): AgentRun[]; // adds agent-category tool items with no run; reuses `previous`'s objects
export function runTree(runs): { run: AgentRun; children: … }[]; // sorted
export function runDuration(run, now): number | undefined;
export function runResult(run, output?): string | undefined;
```

`ChatState` gains `agentRuns: AgentRun[]`, cleared by the same resets as `tasks` and on `/clear`. On agent exit, `running` becomes `stopped` (where `tasks` is emptied). The view reads `mergeRuns(state.agentRuns, state.items, state.busy && !state.exited, previous)`, so history-loaded conversations (which have no task events, and so no runs) and other agents still list rows. `mergeRuns` never changes a run the list has: its status comes from task events, not from its tool item.

Times: `AdapterContext` gets an optional `now()` (as `setClock`, so existing test contexts stand), which returns the replay clock while replaying and `Date.now()` otherwise. `at` and `endedAt` use it, or the "identical replay" test cannot hold. A replayed log has `kelpie_time` lines; history may have none, so times can be absent.

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
   - On an Agent `tool_result`: it says nothing on 2.1.314. `async_launched` leaves the run running, and a plain result of a run with task events waits for the notification. For an older stream, `totalTokens`, `totalDurationMs` and `totalToolUseCount` are taken when present, and a result ends a run that has no task events, or that says so; an error ends any run still running. A conversation's history makes no runs: `mergeRuns` lists it by its rows.
   - Only runs with a linked Agent tool: `task_type === "local_agent"` (or, with no type, a `subagent_type`) and a `tool_use_id`. Background shells and housekeeping tasks do not appear. A notification for an unknown task is ignored, as a shell's looks the same.
4. **Dialog.** `AgentMapDialog` via `Portal` from `ChatBody` (as `UndoDialog`), using `.overlay` and `.modal`. List and detail are two states of one dialog.
5. **Layout, CSS only:** two-column flex; root box vertically centred; children column with `border-left` connector and a `::before` tick for each card, using `var(--border-strong)`. Cards: `.agent-map-card` (`.agent-card` is Settings'), a `<button>`, description with ellipsis and full text in `title`. Nested: same pattern indented inside the column. The list scrolls inside the modal (`max-height: 80vh`). Under 560 px, stack the root above the list. Dots reuse `.tdot` with `.running`, `.error` rules; add a grey `.stopped`.
6. **Helpers.** `formatTokens` (`90.9k`, `1.2M`), `formatDuration` (`3m 49s`, `99m 3s`), shared with `elapsed` in `chat-pane.tsx` if the output fits. Timer: one `setInterval` while open and something is running, cleared in `onCleanup`.
7. **Details.** Fields as in Requirements. Prompt through `Clamped`, result through `Markdown` (phase 2's links work), from `runResult`. "Show in chat" hides when the item is not found.
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
- [ ] Codex session that spawns an agent lists it, without tokens, and no errors. (ACP: not applicable, it has no agent kind.)
- [ ] Transcript looks unchanged; with 50 runs the dialog scrolls smoothly and streaming is not slowed (perf gate).

## Risk Assessment

- **Task event fields change between Claude versions.** The fields are read defensively, and a missing number shows nothing. Signal: blank tokens in the real app. Response: fall back to `tool_use_result`, then to none; do not parse transcripts.
- **Log trim loses a run's start.** `applyRunTaskEvent` creates a run from an unknown agent's `task_progress`, with a neutral name (`description` alone is its activity, shown as such) until the call, `task_started` or a subagent line's `task_description` gives one.
- **Volume.** Subagent messages can be numerous. `applySubagentMessage` stores only a short target per tool call, and updates by index, not by copying the array of runs per message. Signal: a perf-gate regression during subagent-heavy streams. Response: batch updates per frame.
- **Prompt and result text** are rendered through the same sanitizer as replies; prompts as plain text.
