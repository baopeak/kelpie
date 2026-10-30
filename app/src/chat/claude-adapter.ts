// Claude Code `-p --input-format stream-json --output-format stream-json` adapter.
//
// Wire facts (verified against Claude Code 2.1.x):
// - `stream_event` lines carry Anthropic streaming events (message_start, content_block_*);
// - each completed content block is re-emitted as an `assistant` line with one block;
// - `user` lines carry tool results, and our own prompts when `--replay-user-messages`;
// - permission prompts arrive as `control_request{subtype:"can_use_tool"}` and are answered
//   with `control_response` (because of `--permission-prompt-tool stdio`);
// - `result` ends a turn;
// - `control_request{initialize}` answers with its commands (built-in, skills, custom, plugin, MCP prompts), models with their effort
//   levels and the permission mode; `set_model`, `set_permission_mode` and
//   `apply_flag_settings{effortLevel}` change them; `system/init` (each turn) and `system/status`
//   report the live model and mode, and `system/init` lists `terminal_slash_commands`;
// - `/clear` answers with `conversation_reset{trigger:"clear"}`, then `system/init` with the new
//   session id and a `result` of no turns;
// - slash commands are sent as plain user text and run by the CLI (`/context` answers with a
//   synthetic assistant message).
import { type Adapter, type AdapterContext, type AskQuestion, type ChatCommand, type ChatItem, putSetting, setSettingValue, slashCommand, summarize, textOf, withImageNames } from "./chat-model";
import { lineDiff, planStatus } from "./tool-view";
import { t } from "../lib/i18n";

export function claudeArgs(resumeId: string | null, extra: string[] = []): string[] {
  return [
    "claude", "-p",
    "--input-format", "stream-json",
    "--output-format", "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--replay-user-messages",
    "--permission-prompt-tool", "stdio",
    ...(resumeId ? ["--resume", resumeId] : []),
    ...extra,
  ];
}

/** Renders one Claude transcript/stream message (assistant or user) into items. */
export function applyClaudeMessage(ctx: AdapterContext, m: any, live: { msgId: string | null }) {
  // Transcript entries (history from before chat view) carry their own time.
  const at = typeof m.timestamp === "string" ? Date.parse(m.timestamp) : NaN;
  if (!Number.isNaN(at)) ctx.setClock?.(at);
  const msg = m.message ?? {};
  const content = msg.content;
  if (m.type === "user") {
    if (m.isMeta || m.isCompactSummary) return;
    if (typeof content === "string") {
      if (!content.startsWith("<")) pushUser(ctx, content, m.uuid);
      return;
    }
    for (const block of content ?? []) {
      if (block.type === "tool_result") {
        const output = textOf(block.content);
        ctx.patch(block.tool_use_id, (it: ChatItem) => {
          if (it.kind !== "tool") return;
          it.output = output;
          it.status = block.is_error ? "error" : "done";
        });
      } else if (block.type === "text" && !String(block.text).startsWith("<")) {
        pushUser(ctx, block.text, m.uuid);
      }
    }
    return;
  }
  if (m.type !== "assistant" || !Array.isArray(content)) return;
  const group = msg.id ?? m.uuid;
  // What the request sent is what the context holds now (cached or not), plus this reply.
  const u = msg.usage;
  if (u && typeof u.input_tokens === "number") {
    const used = u.input_tokens + (u.cache_creation_input_tokens ?? 0) + (u.cache_read_input_tokens ?? 0) + (u.output_tokens ?? 0);
    ctx.set("usage", { ...(ctx.state.usage ?? { size: null }), used });
  }
  for (const block of content) {
    if (block.type === "tool_use") {
      if (block.name === "TodoWrite" && Array.isArray(block.input?.todos)) {
        ctx.set("plan", { entries: block.input.todos.map((x: any) => ({ text: String(x.status === "in_progress" && x.activeForm ? x.activeForm : x.content ?? ""), status: planStatus(x.status) })) });
      }
      const prev = ctx.find(block.id);
      ctx.upsert({
        kind: "tool", id: block.id, name: block.name, input: block.input, summary: summarize(block.input),
        status: prev?.kind === "tool" ? prev.status : "running", output: prev?.kind === "tool" ? prev.output : undefined,
        diff: diffFor(block.name, block.input),
      });
    } else if (block.type === "text" || block.type === "thinking") {
      const kind = block.type === "text" ? "assistant" : "thinking";
      const text = block.type === "text" ? block.text : block.thinking;
      if (kind === "thinking" && !text) continue;
      // Finalize the streaming item of this message/kind if there is one.
      const streaming = ctx.state.items.findLast((i) => i.kind === kind && (i as any).group === group && (i as any).streaming);
      if (streaming) ctx.patch(streaming.id, (i: any) => { i.text = text; i.streaming = false; });
      else ctx.upsert({ kind, id: `${group}:${kind}:${ctx.state.items.length}`, text, group } as ChatItem);
    }
  }
  live.msgId = group;
}

/** Claude's task events (`system` task_started / task_progress / task_updated /
 *  task_notification): the background shells and subagents still running. Housekeeping tasks
 *  (`skip_transcript`, `ambient`) and a subagent's own tasks (internals, as in the transcript)
 *  are not listed; one the turn waits for (`awaited`) is not background work. */
export function applyTaskEvent(ctx: AdapterContext, m: any) {
  const id = typeof m.task_id === "string" ? m.task_id : null;
  if (!id) return;
  const tasks = ctx.state.tasks;
  const i = tasks.findIndex((x) => x.id === id);
  const drop = () => i >= 0 && ctx.set("tasks", (list) => list.filter((x) => x.id !== id));
  switch (m.subtype) {
    case "task_started":
      if (m.skip_transcript || m.ambient || m.parent_task_id || m.owned_by_subagent || i >= 0) return;
      ctx.set("tasks", (list) => [...list, { id, description: String(m.description ?? ""), background: m.is_backgrounded !== false && !m.awaited }]);
      return;
    case "task_progress":
      if (i >= 0 && typeof m.summary === "string" && m.summary) ctx.set("tasks", i, "summary", m.summary);
      return;
    case "task_updated": {
      const p = m.patch ?? {};
      if (p.status && p.status !== "running" && p.status !== "pending" && p.status !== "paused") return drop();
      if (i < 0) return;
      if (typeof p.description === "string" && p.description) ctx.set("tasks", i, "description", p.description);
      if (typeof p.is_backgrounded === "boolean") ctx.set("tasks", i, "background", p.is_backgrounded);
      return;
    }
    case "task_notification":
      drop();
  }
}

function pushUser(ctx: AdapterContext, text: string, uuid?: string) {
  // Ctrl+C used to stop the TUI during a mode switch leaves this marker; it is not a message.
  if (/^\[Request interrupted by user/.test(text.trim())) return;
  // Replace our optimistic copy instead of duplicating it.
  const pending = ctx.state.items.find((i) => i.kind === "user" && i.pending && i.text.trim() === text.trim());
  if (pending) ctx.patch(pending.id, (i: any) => (i.pending = false));
  else ctx.upsert({ kind: "user", id: uuid ?? `u-${ctx.state.items.length}`, text });
}

export function diffFor(name: string, input: any): { path: string; text: string }[] | undefined {
  if (!input) return undefined;
  if (name === "Edit" && input.file_path) {
    return [{ path: input.file_path, text: lineDiff(String(input.old_string ?? ""), String(input.new_string ?? "")) }];
  }
  if (name === "MultiEdit" && Array.isArray(input.edits)) {
    return [{ path: input.file_path, text: input.edits.map((e: any) => lineDiff(String(e.old_string ?? ""), String(e.new_string ?? ""))).join("\n@@\n") }];
  }
  if (name === "Write" && input.file_path) {
    return [{ path: input.file_path, text: String(input.content ?? "").split("\n").slice(0, 400).map((l: string) => "+" + l).join("\n") }];
  }
  return undefined;
}

/** Request id of our `initialize` (the daemon keeps its answer when it trims the replay log). */
const INIT_ID = "kelpie-init";

interface ClaudeModel { value: string; resolvedModel?: string; displayName?: string; description?: string; supportedEffortLevels?: string[]; supportsAutoMode?: boolean }

const modes = (auto: boolean, current: string | null) => [
  { value: "default", label: t("Ask before edits") },
  { value: "acceptEdits", label: t("Accept edits") },
  { value: "plan", label: t("Plan") },
  ...(auto ? [{ value: "auto", label: t("Auto") }] : []),
  // Reachable only when the session was started with it; never offered otherwise.
  ...(current === "bypassPermissions" ? [{ value: "bypassPermissions", label: t("Bypass permissions") }] : []),
];

export function createClaudeAdapter(ctx: AdapterContext): Adapter {
  const live = { msgId: null as string | null };
  let blocks = new Map<number, string>(); // stream block index → item id
  let partialJson = new Map<number, string>();
  let interruptSeq = 0;
  let requestSeq = 0;
  const pendingInputs = new Map<string, { input: unknown; suggestions?: unknown }>();
  let models: ClaudeModel[] = [];
  let model: string | null = null;
  let mode: string | null = null;
  let effort: string | null = null;
  let tuiOnly = new Set<string>();
  /** The conversation was just cleared: its empty `result` is no turn to show. */
  let cleared = false;

  const control = (request: Record<string, unknown>, id = `kelpie-req-${++requestSeq}`) =>
    ctx.write({ type: "control_request", request_id: id, request });

  /** The picker entry for a live model id ("claude-opus-5-5") or an alias ("opus"). */
  const modelEntry = (m: string | null) => models.find((x) => x.value === m) ?? models.find((x) => x.resolvedModel === m);

  function refreshSettings() {
    if (!models.length) return;
    // Until the first turn reports the live model, the session runs the default one.
    const entry = modelEntry(model ?? "default");
    putSetting(ctx, {
      id: "model", label: t("Model"), value: entry?.value ?? model,
      options: models.map((m) => ({ value: m.value, label: m.displayName ?? m.value, description: m.description })),
    });
    const levels = (entry ?? modelEntry("default"))?.supportedEffortLevels ?? [];
    if (levels.length) putSetting(ctx, { id: "effort", label: t("Effort"), value: effort, options: levels.map((l) => ({ value: l, label: l })) });
    putSetting(ctx, { id: "mode", label: t("Mode"), value: mode, options: modes(!!entry?.supportsAutoMode, mode) });
  }

  function markTuiOnly() {
    ctx.set("commands", (cmds) => cmds.map((c) => (tuiOnly.has(c.name) ? { ...c, tuiOnly: true } : c)));
  }

  return {
    reset() {
      live.msgId = null;
      blocks = new Map();
      partialJson = new Map();
      cleared = false;
    },
    start() {
      ctx.set("status", null);
      control({ subtype: "initialize" }, INIT_ID);
    },
    onMessage(m) {
      if (!m || typeof m !== "object") return;
      switch (m.type) {
        case "system":
          if (m.subtype === "init") {
            ctx.setModel(m.model ?? null);
            if (m.session_id) ctx.setConversation(m.session_id);
            model = m.model ?? model;
            mode = m.permissionMode ?? mode;
            tuiOnly = new Set(m.terminal_slash_commands ?? []);
            markTuiOnly();
            refreshSettings();
          } else if (m.subtype === "status") {
            ctx.set("status", m.status ?? null);
            if (m.permissionMode) {
              mode = m.permissionMode;
              setSettingValue(ctx, "mode", mode);
            }
          } else if (m.subtype === "api_retry") {
            ctx.upsert({ kind: "system", id: `retry-${ctx.state.items.length}`, text: t("API retry…"), tone: "info" });
          } else if (typeof m.subtype === "string" && m.subtype.startsWith("task_")) {
            applyTaskEvent(ctx, m);
          }
          return;
        case "conversation_reset":
          // A new conversation starts empty, as in the TUI (the old one stays resumable).
          if (m.trigger !== "clear") return;
          ctx.clear();
          live.msgId = null;
          blocks = new Map();
          partialJson = new Map();
          cleared = true;
          return;
        case "stream_event": {
          const e = m.event ?? {};
          if (m.parent_tool_use_id) return; // subagent internals stay folded into the tool card
          if (e.type === "message_start") {
            live.msgId = e.message?.id ?? null;
            blocks = new Map();
            ctx.set("busy", true);
          } else if (e.type === "content_block_start") {
            const b = e.content_block ?? {};
            const group = live.msgId ?? "live";
            if (b.type === "text" || b.type === "thinking") {
              const kind = b.type === "text" ? "assistant" : "thinking";
              const id = `${group}:${kind}:${e.index}`;
              blocks.set(e.index, id);
              ctx.upsert({ kind, id, text: "", streaming: true, group } as ChatItem);
            } else if (b.type === "tool_use") {
              blocks.set(e.index, b.id);
              partialJson.set(e.index, "");
              ctx.upsert({ kind: "tool", id: b.id, name: b.name, summary: "", status: "running" });
            }
          } else if (e.type === "content_block_delta") {
            const id = blocks.get(e.index);
            const d = e.delta ?? {};
            if (!id) return;
            if (d.type === "text_delta") ctx.patch(id, (i: any) => (i.text += d.text));
            else if (d.type === "thinking_delta") ctx.patch(id, (i: any) => (i.text += d.thinking));
            else if (d.type === "input_json_delta") {
              const acc = (partialJson.get(e.index) ?? "") + (d.partial_json ?? "");
              partialJson.set(e.index, acc);
              const cmd = /"(?:command|file_path|pattern|path|url)"\s*:\s*"((?:[^"\\]|\\.)*)/.exec(acc)?.[1];
              if (cmd) ctx.patch(id, (i: any) => (i.summary = cmd.replace(/\\(.)/g, "$1")));
            }
          }
          return;
        }
        case "assistant":
        case "user":
          if (m.parent_tool_use_id) return;
          applyClaudeMessage(ctx, m, live);
          if (m.type === "user" && !m.isReplay) ctx.set("busy", true);
          return;
        case "control_cancel_request":
          // The CLI withdrew the question (its turn was interrupted).
          pendingInputs.delete(m.request_id);
          ctx.patch(m.request_id, (i: any) => {
            if (i.kind === "permission" && !i.resolved) i.resolved = t("Cancelled");
          });
          return;
        case "control_request": {
          const r = m.request ?? {};
          if (r.subtype !== "can_use_tool") return;
          ctx.upsert(permissionItem(m.request_id, String(r.tool_name), r.input ?? {}));
          pendingInputs.set(m.request_id, { input: r.input, suggestions: r.permission_suggestions });
          return;
        }
        case "control_response": {
          const r = m.response ?? {};
          if (r.subtype === "error") {
            // An interrupt with nothing running (e.g. before a hand-off to the TUI) is no news.
            if (String(r.request_id).startsWith("kelpie-interrupt-")) return;
            ctx.upsert({ kind: "system", id: `ctl-${r.request_id}`, text: t("Claude: {error}", { error: String(r.error ?? "request failed") }), tone: "error" });
            return;
          }
          const res = r.response ?? {};
          if (r.request_id === INIT_ID) {
            ctx.set("commands", (res.commands ?? []).map((c: any): ChatCommand => ({ name: c.name, description: c.description, hint: c.argumentHint || undefined, tuiOnly: tuiOnly.has(c.name) })));
            models = res.models ?? [];
            mode = res.current_permission_mode ?? mode;
            refreshSettings();
          } else if (String(r.request_id).startsWith("kelpie-effort:")) {
            // The level travels in the id, so a replayed log restores it too.
            effort = String(r.request_id).split(":")[1] || null;
            setSettingValue(ctx, "effort", effort);
          } else if (res.model) {
            model = res.model;
            ctx.setModel(model);
            refreshSettings();
          } else if (res.mode) {
            mode = res.mode;
            setSettingValue(ctx, "mode", mode);
          }
          return;
        }
        case "result": {
          if (cleared) {
            cleared = false;
            ctx.set({ busy: false, status: null });
            return;
          }
          // The main model's window: the one this session runs, else the one that cost the most.
          const usage = Object.entries((m.modelUsage ?? {}) as Record<string, any>);
          const main = usage.find(([name]) => name === model) ?? usage.sort((a, b) => (b[1].costUSD ?? 0) - (a[1].costUSD ?? 0))[0];
          if (main?.[1]?.contextWindow || m.total_cost_usd != null) {
            ctx.set("usage", { used: ctx.state.usage?.used ?? 0, size: main?.[1]?.contextWindow ?? ctx.state.usage?.size ?? null, cost: m.total_cost_usd ?? ctx.state.usage?.cost });
          }
          ctx.set({ busy: false, status: null });
          ctx.upsert({
            kind: "result",
            id: `result-${m.uuid ?? ctx.state.items.length}`,
            text: [m.is_error ? t("Error: {error}", { error: String(m.result ?? (Array.isArray(m.errors) && m.errors.length ? m.errors.join("; ") : m.subtype)) }) : null, m.duration_ms ? `${(m.duration_ms / 1000).toFixed(1)}s` : null, m.num_turns ? (m.num_turns === 1 ? t("{n} turn", { n: m.num_turns }) : t("{n} turns", { n: m.num_turns })) : null, m.total_cost_usd != null ? `$${m.total_cost_usd.toFixed(3)}` : null].filter(Boolean).join(" · "),
          });
          // Close any stale streaming markers.
          for (const it of ctx.state.items) if ((it as any).streaming) ctx.patch(it.id, (i: any) => (i.streaming = false));
          return;
        }
      }
    },
    send(text, _typed, images) {
      const slash = slashCommand(text);
      if (slash && tuiOnly.has(slash[0])) {
        ctx.upsert({ kind: "system", id: `tui-${Date.now()}`, text: t("/{name} works only in the terminal UI.", { name: slash[0] }), tone: "info", tui: { command: text.trim() } });
        return;
      }
      ctx.upsert({ kind: "user", id: `local-${Date.now()}`, text: withImageNames(text, images), pending: true });
      ctx.set("busy", true);
      const pictures = (images ?? []).map((i) => ({ type: "image", source: { type: "base64", media_type: i.mime, data: i.data } }));
      ctx.write({ type: "user", message: { role: "user", content: [...pictures, ...(text.trim() ? [{ type: "text", text }] : [])] } });
    },
    // The CLI takes a message mid-turn and reads it at its next step.
    steer(text) {
      this.send(text);
      return true;
    },
    immediate(text) {
      const slash = slashCommand(text);
      return !!slash && tuiOnly.has(slash[0]);
    },
    interrupt() {
      ctx.write({ type: "control_request", request_id: `kelpie-interrupt-${++interruptSeq}`, request: { subtype: "interrupt" } });
    },
    // Claude reports its model only with a turn, and its effort never: until then, the ones the
    // conversation ran with (it was started with them as flags).
    seed(s) {
      model ??= s.model ?? null;
      effort ??= s.effort ?? null;
      refreshSettings();
    },
    setSetting(id, value) {
      if (id === "model") control({ subtype: "set_model", model: value });
      else if (id === "mode") control({ subtype: "set_permission_mode", mode: value });
      else if (id === "effort") control({ subtype: "apply_flag_settings", settings: { effortLevel: value } }, `kelpie-effort:${value}:${++requestSeq}`);
    },
    respond(id, option, reply) {
      const pending = pendingInputs.get(id);
      pendingInputs.delete(id);
      const input = (pending?.input ?? {}) as Record<string, unknown>;
      const response =
        option === "deny"
          ? { behavior: "deny", message: reply?.message?.trim() || "The user denied this action." }
          : option === "answer"
            ? { behavior: "allow", updatedInput: { ...input, answers: reply?.answers ?? {} } }
            : option === "plan-accept-edits"
              ? { behavior: "allow", updatedInput: input, updatedPermissions: [{ type: "setMode", mode: "acceptEdits", destination: "session" }] }
              : { behavior: "allow", updatedInput: input, ...(option === "allow-session" && pending?.suggestions ? { updatedPermissions: pending.suggestions } : {}) };
      ctx.write({ type: "control_response", response: { subtype: "success", request_id: id, response } });
    },
  };
}

/** The card for a `can_use_tool` request: questions to answer (AskUserQuestion), a plan to
 *  approve (ExitPlanMode), or a tool to allow, showing its command or edit. */
export function permissionItem(id: string, tool: string, input: any): Extract<ChatItem, { kind: "permission" }> {
  if (tool === "AskUserQuestion" && Array.isArray(input.questions)) {
    const ask: AskQuestion[] = input.questions.map((q: any) => ({
      question: String(q.question ?? ""), header: q.header, multiSelect: !!q.multiSelect,
      options: (q.options ?? []).map((o: any) => ({ label: String(o.label ?? o), description: o.description })),
    }));
    return { kind: "permission", id, title: t("Claude has a question"), ask, options: [{ id: "deny", label: t("Skip"), tone: "deny" }], feedback: true };
  }
  if (tool === "ExitPlanMode") {
    return {
      kind: "permission", id, title: t("Ready to code?"), plan: String(input.plan ?? ""), feedback: true,
      options: [
        { id: "plan-accept-edits", label: t("Yes, and accept edits"), tone: "allow" },
        { id: "allow", label: t("Yes, ask before edits"), tone: "allow" },
        { id: "deny", label: t("Keep planning"), tone: "deny" },
      ],
    };
  }
  const command = typeof input.command === "string" ? input.command : undefined;
  const diff = diffFor(tool, input);
  return {
    kind: "permission", id, title: t("Allow {tool}?", { tool }), command, diff, feedback: true,
    detail: command || diff ? (typeof input.description === "string" ? input.description : undefined) : summarize(input, 600),
    options: [
      { id: "allow", label: t("Allow"), tone: "allow" },
      { id: "allow-session", label: t("Allow for session"), tone: "allow" },
      { id: "deny", label: t("Deny"), tone: "deny" },
    ],
  };
}

