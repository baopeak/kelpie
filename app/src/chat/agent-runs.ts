// The subagents of a conversation, for the agent map: built only from messages already received, so a
// replayed log rebuilds the same list, and nothing here talks to the agent.
//
// Wire facts (verified against Claude Code 2.1.314):
// - the Agent tool returns at once (`tool_use_result.status: "async_launched"`), even without
//   `run_in_background`: its row is done while the agent runs, so a run's status comes from task events;
// - `task_started` has the name, type, prompt and, when nested, `parent_task_id`; `task_type` tells a
//   subagent (`local_agent`) from a background shell;
// - `task_progress` has the running `usage`, and in `description` what the agent is doing now, not its name;
// - `task_notification` has the final `usage`, and in `summary` the agent's final answer;
// - a subagent's own calls come as messages with `parent_tool_use_id`, a nested Agent call among them,
//   ahead of its `task_started`.
// - besides `async_launched` the tool has other launch statuses (`remote_launched`, `teammate_spawned`):
//   a tool result ends a run only on a status that means an end;
// - a finished agent can be continued: a new `task_started` under its ids.
// Older streams may have no `task_type`, or the totals in `tool_use_result`: every field is read
// defensively, and a number that is missing shows nothing.
//
// The functions that take a list change it in place: subagent streams can be large, and the list is a
// store's, updated through `produce`. Nothing here copies it per message.
import { unwrap } from "solid-js/store";
import { type ChatItem, summarize } from "./chat-model";
import { claudeCategory, describeTool } from "./tool-view";

export type RunStatus = "running" | "done" | "error" | "stopped";

/** One tool call of a subagent. Its output is not kept. */
export interface RunActivity {
  id: string;
  name: string;
  target: string;
  status: "running" | "done" | "error";
}

export interface AgentRun {
  /** The Agent tool call's id, which is also its row in the transcript when it has one. */
  id: string;
  taskId?: string;
  /** Who started it: the parent's id, and its task id, whichever the stream told. */
  parent?: string;
  parentTask?: string;
  description: string;
  type?: string;
  prompt?: string;
  status: RunStatus;
  /** The status it ended with when that means nothing here, as the agent put it. */
  statusText?: string;
  at?: number;
  endedAt?: number;
  tokens?: number;
  toolUses?: number;
  durationMs?: number;
  /** What it was doing at the last report. */
  summary?: string;
  /** Its final answer. */
  result?: string;
  /** Its latest tool calls, the oldest first. */
  activity: RunActivity[];
}

export interface RunNode {
  run: AgentRun;
  children: RunNode[];
}

const ACTIVITY_CAP = 200;
const TARGET_CHARS = 120;

const text = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" && Number.isFinite(v) ? v : undefined);

export const isAgentTool = (name: unknown) => typeof name === "string" && claudeCategory(name) === "agent";

function addRun(list: AgentRun[], id: string, at?: number): AgentRun {
  list.push({ id, description: "", status: "running", activity: [], ...(at == null ? {} : { at }) });
  return list[list.length - 1];
}

function nameRun(run: AgentRun, description: unknown, type: unknown, prompt: unknown) {
  run.description = text(description) || run.description;
  run.type = text(type) || run.type;
  run.prompt = text(prompt) || run.prompt;
}

/** An Agent call: the run it starts, or the one the stream's start of the same call began. */
export function startRun(list: AgentRun[], tool: { id: string; input?: unknown }, parent?: string, at?: number): AgentRun {
  const input = (tool.input ?? {}) as Record<string, unknown>;
  const run = list.find((r) => r.id === tool.id) ?? addRun(list, tool.id, at);
  nameRun(run, input.description, input.subagent_type, input.prompt);
  if (parent) run.parent = parent;
  return run;
}

const ENDED = new Map<string, RunStatus>([
  ["completed", "done"], ["done", "done"], ["failed", "error"], ["error", "error"],
  ["stopped", "stopped"], ["killed", "stopped"], ["cancelled", "stopped"], ["canceled", "stopped"], ["interrupted", "stopped"],
]);
const STILL_GOING = new Set(["running", "pending", "paused"]);

function finish(run: AgentRun, status: RunStatus, at?: number, statusText?: string) {
  run.status = status;
  run.statusText = statusText;
  run.endedAt ??= at;
}

/** Nothing will report these done (the agent exited). They stay listed. */
export function stopRunning(list: AgentRun[], at?: number) {
  for (const run of list) if (run.status === "running") finish(run, "stopped", at);
}

function applyUsage(run: AgentRun, usage: any) {
  run.tokens = num(usage?.total_tokens) ?? run.tokens;
  run.toolUses = num(usage?.tool_uses) ?? run.toolUses;
  run.durationMs = num(usage?.duration_ms) ?? run.durationMs;
}

/** A subagent's task, not a shell's: progress has no `task_type`, but names the subagent's type. */
const isAgentTask = (m: any) => (m.task_type == null ? !!text(m.subagent_type) : m.task_type === "local_agent");

/** Claude's task events, before anything else looks at them. A run whose start a trimmed log lost comes
 *  back with its progress; a notification cannot bring one back, as a shell's looks the same. */
export function applyRunTaskEvent(list: AgentRun[], m: any, at?: number) {
  const taskId = text(m.task_id);
  const toolId = text(m.tool_use_id);
  let run = list.find((r) => (taskId && r.taskId === taskId) || (toolId && r.id === toolId));
  if (!run) {
    if (m.subtype !== "task_started" && m.subtype !== "task_progress") return;
    if (!toolId || !isAgentTask(m) || m.skip_transcript || m.ambient) return;
    run = addRun(list, toolId, at);
  }
  if (taskId) run.taskId = taskId;
  switch (m.subtype) {
    case "task_started":
      nameRun(run, m.description, m.subagent_type, m.prompt);
      run.parentTask = text(m.parent_task_id) || run.parentTask;
      run.at ??= at;
      // A finished agent continued. Its old end and time no longer say anything.
      if (run.status !== "running") {
        run.status = "running";
        run.statusText = run.endedAt = run.durationMs = undefined;
        run.at = at ?? run.at;
      }
      return;
    case "task_progress":
      // After the final count, or an exit that stopped the run, a late report changes nothing.
      if (run.status !== "running") return;
      // Alone, `description` is what the agent is doing now, no name for a run its start did not reach.
      if (text(m.summary)) run.description ||= text(m.description);
      run.type ||= text(m.subagent_type) || undefined;
      run.summary = text(m.summary) || text(m.description) || run.summary;
      applyUsage(run, m.usage);
      return;
    case "task_updated": {
      const ended = ENDED.get(text(m.patch?.status));
      if (ended && run.status === "running") finish(run, ended, at);
      return;
    }
    case "task_notification": {
      // The end of the task, whatever it calls it: a status not known here stays as its text.
      const said = text(m.status);
      if (!STILL_GOING.has(said)) finish(run, !said ? "done" : ENDED.get(said) ?? "stopped", at, !said || ENDED.has(said) ? undefined : said);
      applyUsage(run, m.usage);
      run.result = text(m.summary) || run.result;
    }
  }
}

/** The result of an Agent call, `block` of the message `line`. It ends a run only when it says so: a launch
 *  does not, and neither does the plain result of a call whose task events will tell. */
export function applyRunResult(list: AgentRun[], block: any, line: any, at?: number) {
  const run = list.find((r) => r.id === block.tool_use_id);
  if (!run) return;
  // A line's `tool_use_result` is about its one result.
  const content = line?.message?.content;
  const meta = Array.isArray(content) && content.length === 1 ? line.tool_use_result : undefined;
  const info = meta && typeof meta === "object" ? meta : undefined;
  run.tokens = num(info?.totalTokens) ?? run.tokens;
  run.durationMs = num(info?.totalDurationMs) ?? run.durationMs;
  run.toolUses = num(info?.totalToolUseCount) ?? run.toolUses;
  if (run.status !== "running") return;
  if (block.is_error) return finish(run, "error", at);
  if (info?.isAsync) return;
  if (text(info?.status)) {
    const ended = ENDED.get(info.status);
    if (ended) finish(run, ended, at);
  } else if (!run.taskId || num(info?.totalTokens) != null) finish(run, "done", at);
}

/** A message of a subagent: its tool calls, and a nested Agent call as a run of its own. */
export function applySubagentMessage(list: AgentRun[], m: any, at?: number) {
  const parent = text(m.parent_tool_use_id);
  const content = m.message?.content;
  if (!parent || !Array.isArray(content)) return;
  const run = list.find((r) => r.id === parent) ?? addRun(list, parent, at);
  run.description ||= text(m.task_description);
  run.type ||= text(m.subagent_type) || undefined;
  for (const block of content) {
    if (block?.type === "tool_use" && typeof block.id === "string") {
      if (isAgentTool(block.name)) startRun(list, block, parent, at);
      addActivity(run, block);
    } else if (block?.type === "tool_result") {
      const call = run.activity.find((a) => a.id === block.tool_use_id);
      if (call) call.status = block.is_error ? "error" : "done";
      applyRunResult(list, block, m, at);
    }
  }
}

function addActivity(run: AgentRun, block: any) {
  if (run.activity.some((a) => a.id === block.id)) return;
  const name = text(block.name);
  const target = describeTool({ kind: "tool", id: block.id, name, input: block.input, summary: summarize(block.input, TARGET_CHARS), status: "running" }).target;
  run.activity.push({ id: block.id, name, target: target.length > TARGET_CHARS ? `${target.slice(0, TARGET_CHARS)}…` : target, status: "running" });
  if (run.activity.length > ACTIVITY_CAP) run.activity = run.activity.slice(-ACTIVITY_CAP);
}

const sameRow = (a: AgentRun, b: AgentRun) => a.description === b.description && a.type === b.type && a.prompt === b.prompt && a.status === b.status && a.at === b.at;

/** The runs, then what the transcript has of agents that report no task events: another agent's
 *  `agent` category tools, or Agent calls from a conversation's history. Nothing changes a run the
 *  list has: its status comes from its task events, not from its row. `busy`: a row can still be
 *  running; with no turn going, nothing will finish one. `previous`: the last result, whose objects
 *  stand for rows that did not change, so that their cards stay and so does the list if nothing did.
 *  In a memo, it depends on the number of rows and on the rows that matched, not on every row. */
export function mergeRuns(runs: AgentRun[], items: ChatItem[], busy = false, previous: AgentRun[] = []): AgentRun[] {
  const known = new Set(runs.map((r) => r.id));
  const count = items.length;
  const raw = unwrap(items);
  const added: AgentRun[] = [];
  for (let i = 0; i < count; i++) {
    const row = raw[i];
    if (row.kind !== "tool" || (row.category ?? claudeCategory(row.name)) !== "agent" || known.has(row.id)) continue;
    const it = items[i] as typeof row;
    const input = (it.input ?? {}) as Record<string, unknown>;
    const fresh: AgentRun = {
      id: it.id, description: describeTool(it).target, type: text(input.subagent_type) || undefined, prompt: text(input.prompt) || undefined,
      status: !busy && it.status === "running" ? "stopped" : it.status, at: it.at, activity: [],
    };
    const before = previous.find((r) => r.id === fresh.id);
    added.push(before && sameRow(before, fresh) ? before : fresh);
  }
  if (!added.length) return runs;
  const merged = [...runs, ...added];
  return merged.length === previous.length && merged.every((r, i) => r === previous[i]) ? previous : merged;
}

const RANK: Record<RunStatus, number> = { running: 0, error: 1, done: 2, stopped: 2 };

/** The runs as a tree, sorted: running, then errors, then the finished, the newest first within each
 *  (arrival order stands in for missing times). A parent not known yet leaves its child at the top
 *  until it comes, and so does a chain of parents that goes round. */
export function runTree(runs: AgentRun[]): RunNode[] {
  const byId = new Map(runs.map((r) => [r.id, r]));
  const byTask = new Map(runs.flatMap((r) => (r.taskId ? [[r.taskId, r] as const] : [])));
  const parentOf = (r: AgentRun) => (r.parent && byId.get(r.parent)) || (r.parentTask && byTask.get(r.parentTask)) || undefined;
  const inCycle = (r: AgentRun) => {
    const seen = new Set<AgentRun>();
    for (let p = parentOf(r); p; p = parentOf(p)) {
      if (p === r) return true;
      if (seen.has(p)) return false;
      seen.add(p);
    }
    return false;
  };
  const arrival = new Map(runs.map((r, i) => [r.id, i]));
  const nodes = new Map(runs.map((run) => [run.id, { run, children: [] as RunNode[] }]));
  const roots: RunNode[] = [];
  for (const run of runs) {
    const parent = inCycle(run) ? undefined : parentOf(run);
    (parent ? nodes.get(parent.id)!.children : roots).push(nodes.get(run.id)!);
  }
  const sort = (list: RunNode[]) => {
    list.sort((a, b) => RANK[a.run.status] - RANK[b.run.status] || (b.run.at ?? 0) - (a.run.at ?? 0) || arrival.get(b.run.id)! - arrival.get(a.run.id)!);
    for (const node of list) sort(node.children);
  };
  sort(roots);
  return roots;
}

/** How long a run has taken, if that can be told. A running one goes by the clock, and never below what
 *  it last reported (its start may be gone from the log); one that finished goes by its own count; one
 *  that was stopped reported last before it stopped, so by the clock again. */
export function runDuration(run: AgentRun, now: number): number | undefined {
  const until = run.status === "running" ? now : run.endedAt;
  const elapsed = run.at != null && until != null ? Math.max(0, until - run.at) : undefined;
  if (run.status === "running") return elapsed == null ? run.durationMs : Math.max(elapsed, run.durationMs ?? 0);
  return run.status === "stopped" ? elapsed ?? run.durationMs : run.durationMs ?? elapsed;
}

/** What the launch of an agent returns: the tool's output, but no result. */
const LAUNCH_NOTICE = /^Async agent launched/;

/** A run's answer: its own, or else its tool item's `output`, once it finished. */
export function runResult(run: AgentRun, output?: string): string | undefined {
  if (run.status === "running") return undefined;
  return run.result || (output && !LAUNCH_NOTICE.test(output) ? output : undefined);
}

/** "90.9k", "1.2M". */
export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  return n < 999_950 ? `${(n / 1000).toFixed(1)}k` : `${(n / 1_000_000).toFixed(1)}M`;
}

/** "59s", "3m 49s", "99m 3s". */
export function formatDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return s < 60 ? `${s}s` : `${Math.floor(s / 60)}m ${s % 60}s`;
}
