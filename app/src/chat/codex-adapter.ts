// Codex `app-server` adapter (JSON-RPC over stdio, no "jsonrpc" field on the wire).
// Shapes follow `codex app-server generate-json-schema` (v2 protocol, codex-cli 0.163).
//
// The app-server has no slash commands: the TUI's ones map to requests here (`/compact` →
// `thread/compact/start`, `/review` → `review/start`, `/new` (or `/clear`, `/reset`) → `thread/start`, `!cmd` →
// `thread/shellCommand`); model, effort and approval mode ride on the next `turn/start`, which
// Codex keeps for later turns; `$skill` mentions add a `skill` input item.
import { type Adapter, type AdapterContext, type ChatCommand, type ChatImage, type ToolCategory, NEW_CONVERSATION_COMMANDS, pathImage, putSetting, sentImages, setSettingValue, slashCommand, summarize, textOf } from "./chat-model";
import { basename, planStatus } from "./tool-view";
import { t } from "../lib/i18n";

/** `codex app-server` accepts `-c key=value` overrides; translate the TUI's `--model X`. */
export function codexArgs(profile: string[] = []): string[] {
  const out = ["codex", "app-server"];
  for (let i = 0; i < profile.length; i++) {
    const a = profile[i];
    if ((a === "--model" || a === "-m") && profile[i + 1]) out.push("-c", `model="${profile[++i]}"`);
    else if (a.startsWith("--model=")) out.push("-c", `model="${a.slice(8)}"`);
    else if ((a === "-c" || a === "--config") && profile[i + 1]) out.push("-c", profile[++i]);
  }
  return out;
}

const INIT_ID = "kelpie-init";
const THREAD_ID = "kelpie-thread";
const LIST_ID = "kelpie-list";
const MODELS_ID = "kelpie-models";
const SKILLS_ID = "kelpie-skills";

/** The TUI's commands chat view runs itself; any other `/name` is offered in the terminal. */
const commands = (): ChatCommand[] => [
  { name: "compact", description: t("Summarize the conversation to free up context") },
  { name: "review", description: t("Review uncommitted changes, or follow the given instructions"), hint: t("[instructions]") },
  { name: "new", description: t("Start a new conversation") },
  { name: "model", description: t("Switch the model"), hint: "<model>" },
];

/** Codex TUI commands with no app-server request: offered in the terminal UI. Any other
 *  `/word` is an ordinary prompt ("/tmp is full, clean it up"). */
const TUI_ONLY = new Set([
  "approvals", "permissions", "init", "diff", "mention", "status", "mcp", "logout", "quit", "exit", "feedback", "undo",
  "resume", "fork", "rename", "skills", "theme", "vim", "keymap", "statusline", "side", "worktree", "reasoning", "fast",
  "personality", "experimental", "plan", "agent", "apps", "plugins", "goal", "ps", "debug-config",
]);

/** The TUI's approval presets, as `turn/start` overrides. */
const MODES: Record<string, { approvalPolicy: string; sandboxPolicy: { type: string } }> = {
  "read-only": { approvalPolicy: "on-request", sandboxPolicy: { type: "readOnly" } },
  auto: { approvalPolicy: "on-request", sandboxPolicy: { type: "workspaceWrite" } },
  "full-access": { approvalPolicy: "never", sandboxPolicy: { type: "dangerFullAccess" } },
};

function modeOf(sandbox: any): string | null {
  const type = sandbox?.type;
  return type === "readOnly" ? "read-only" : type === "workspaceWrite" ? "auto" : type === "dangerFullAccess" ? "full-access" : null;
}

/** A shell command Codex parsed into reads, listings and searches reads like one of those. */
function commandView(item: any, cmd: string): { category: ToolCategory; summary: string } {
  const actions: any[] = Array.isArray(item.commandActions) ? item.commandActions : [];
  if (!actions.length || actions.some((a) => a.type === "unknown")) return { category: "command", summary: cmd };
  if (actions.every((a) => a.type === "read")) return { category: "read", summary: actions.map((a) => a.name || basename(String(a.path ?? ""))).join(", ") };
  const search = actions.find((a) => a.type === "search");
  if (search) return { category: "search", summary: [search.query, search.path && basename(String(search.path))].filter(Boolean).join(" · ") || cmd };
  return { category: "read", summary: actions.map((a) => (a.type === "listFiles" ? (a.path ? basename(String(a.path)) : ".") : a.name)).join(", ") };
}

interface CodexModel { id: string; model?: string; displayName?: string; description?: string; hidden?: boolean; isDefault?: boolean; defaultReasoningEffort?: string; supportedReasoningEfforts?: { reasoningEffort: string; description?: string }[] }

/** A decision as an answer wrote it (the older approvals have their own words) → the card's option. */
const LOGGED_DECISION: Record<string, string> = { accept: "accept", approved: "accept", acceptForSession: "acceptForSession", approved_for_session: "acceptForSession", decline: "decline", denied: "decline" };

export function createCodexAdapter(ctx: AdapterContext): Adapter {
  let initialized = false;
  let threadReady = false;
  let threadId: string | null = null;
  /** The thread whose items are on screen (kept while `/new` waits for the next one). */
  let shown: string | null = null;
  let turnId: string | null = null;
  let seq = 0;
  const approvals = new Map<string, { id: unknown; method: string }>();
  /** Messages put into a running turn, by request id: queued again if Codex refuses. */
  const steered = new Map<string, string>();
  /** The running turn's changes as one diff (`turn/diff/updated`), shown with its result. */
  let turnDiff: string | null = null;
  /** Prompts typed before the thread was ready; sent once it is. */
  const queued: { text: string; typed: boolean; images?: ChatImage[] }[] = [];
  let models: CodexModel[] = [];
  /** Models a thread ran with that the catalog does not list (one set in config.toml): they
   *  stay in the picker, or a switch away could not be undone. */
  const unlisted = new Set<string>();
  let model: string | null = null;
  let effort: string | null = null;
  let mode: string | null = null;
  /** The user chose `mode` in chat view (otherwise it only shows the thread's own sandbox). */
  let modePicked = false;
  /** The thread's sandbox as Codex reported it (network access, extra writable roots). */
  let threadSandbox: any = null;
  /** Picked in chat view; sent with the next turn (Codex keeps them for the thread). */
  const overrides: Record<string, unknown> = {};

  /** A preset as `turn/start` overrides, keeping the thread's own sandbox details (network,
   *  extra roots) when the preset is the same kind of sandbox. */
  function preset(value: string) {
    const p = MODES[value];
    return { approvalPolicy: p.approvalPolicy, sandboxPolicy: threadSandbox?.type === p.sandboxPolicy.type ? { ...threadSandbox } : p.sandboxPolicy };
  }

  /** `$name` mentions of known skills travel as skill items, as the TUI sends them. */
  function userInput(text: string, images: ChatImage[] = []) {
    const skills = ctx.state.skills.filter((s) => s.path && new RegExp(`(^|\\s)\\$${s.name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?=\\s|$)`).test(text));
    return [
      ...(text.trim() ? [{ type: "text", text, text_elements: [] }] : []),
      ...images.map((i) => ({ type: "localImage", path: i.path })),
      ...skills.map((s) => ({ type: "skill", name: s.name, path: s.path })),
    ];
  }

  function startTurn(text: string, images?: ChatImage[]) {
    ctx.set("busy", true);
    const input = userInput(text, images);
    // What the user picked travels in the request id: its answer is logged, so a reloaded view
    // restores it. Only picks — never the thread's own settings, which Codex already keeps.
    const picks = encodeURIComponent(JSON.stringify({ model: overrides.model, effort: overrides.effort, mode: modePicked ? mode : undefined }));
    request(`kelpie-turn-${++seq}-${Date.now()}~${picks}`, "turn/start", { threadId, input, ...overrides });
  }

  /** Replay only: the picks a turn was started with (see `startTurn`). */
  function restorePicks(id: string) {
    const tail = id.split("~")[1];
    if (!tail) return;
    try {
      const p = JSON.parse(decodeURIComponent(tail));
      if (p.model) model = overrides.model = p.model;
      if (p.effort) effort = overrides.effort = p.effort;
      if (p.mode && MODES[p.mode]) {
        mode = p.mode;
        modePicked = true;
        Object.assign(overrides, preset(p.mode));
      }
      refreshSettings();
    } catch {
      /* an id from an older Kelpie */
    }
  }

  const request = (id: string, method: string, params: unknown) => ctx.write({ id, method, params });

  function refreshSettings() {
    const visible = models.filter((m) => !m.hidden || m.id === model || m.model === model);
    const options = [
      ...[...unlisted].filter((x) => !models.some((m) => (m.model ?? m.id) === x)).map((x) => ({ value: x, label: x })),
      ...visible.map((m) => ({ value: m.model ?? m.id, label: m.displayName ?? m.id, description: m.description })),
    ];
    if (models.length) {
      putSetting(ctx, { id: "model", label: t("Model"), value: model, options });
      const current = models.find((m) => (m.model ?? m.id) === model) ?? models.find((m) => m.isDefault);
      const levels = current?.supportedReasoningEfforts ?? [];
      if (levels.length) putSetting(ctx, { id: "effort", label: t("Effort"), value: effort ?? current?.defaultReasoningEffort ?? null, options: levels.map((l) => ({ value: l.reasoningEffort, label: l.reasoningEffort, description: l.description })) });
    }
    putSetting(ctx, {
      id: "mode", label: t("Mode"), value: mode,
      options: [
        { value: "read-only", label: t("Read only"), description: t("Reads files; asks before editing or running commands") },
        { value: "auto", label: t("Auto"), description: t("Edits and runs commands in this folder; asks for anything else") },
        { value: "full-access", label: t("Full access"), description: t("No sandbox and no approvals") },
      ],
    });
  }

  /** `/name args`, and `!command` typed in the composer (as in the TUI it runs in the user's
   *  shell, outside the sandbox — never for text Kelpie sends on the user's behalf). False when
   *  the text is an ordinary prompt, or a command that needs the thread, which is not open yet:
   *  it is then queued like a prompt. */
  function runCommand(text: string, typed: boolean): boolean {
    if (typed && text.startsWith("!") && text.length > 1) {
      if (!threadId) return false;
      request(`kelpie-shell-${++seq}`, "thread/shellCommand", { threadId, command: text.slice(1).trim() });
      return true;
    }
    const slash = slashCommand(text);
    if (!slash) return false;
    const [name, args] = slash;
    if ((name === "compact" || name === "review" || NEW_CONVERSATION_COMMANDS.has(name)) && !threadReady) return false;
    if (name === "compact") {
      ctx.set("busy", true);
      request(`kelpie-compact-${++seq}`, "thread/compact/start", { threadId });
    } else if (name === "review") {
      ctx.set("busy", true);
      request(`kelpie-review-${++seq}`, "review/start", { threadId, target: args ? { type: "custom", instructions: args } : { type: "uncommittedChanges" } });
    } else if (NEW_CONVERSATION_COMMANDS.has(name) && !args) {
      // The running turn belongs to the old thread: it would keep rendering here.
      if (turnId) {
        ctx.upsert({ kind: "system", id: `new-${++seq}`, text: t("Stop the current turn first."), tone: "info" });
        return true;
      }
      threadId = null;
      turnId = null;
      threadReady = false;
      ctx.set("conversationId", null);
      openThread(null);
    } else if (name === "model") {
      // Only a model Codex offers: a typo would make every later turn fail.
      const known = models.find((m) => m.id === args || m.model === args);
      if (known) setModel(known.model ?? known.id);
      else if (unlisted.has(args)) setModel(args);
      else ctx.upsert({ kind: "system", id: `model-${++seq}`, text: args ? t("Unknown model {model}. Pick one below the message field.", { model: args }) : t("Pick the model below the message field."), tone: "info" });
    } else if (TUI_ONLY.has(name)) {
      ctx.upsert({ kind: "system", id: `tui-${++seq}`, text: t("/{name} works only in the terminal UI.", { name }), tone: "info", tui: { command: text.trim() } });
    } else {
      return false;
    }
    return true;
  }

  function setModel(value: string) {
    model = value;
    overrides.model = value;
    // Codex keeps the last effort sent, which the new model may not offer: send its default.
    effort = models.find((m) => (m.model ?? m.id) === value)?.defaultReasoningEffort ?? null;
    if (effort) overrides.effort = effort;
    else delete overrides.effort;
    ctx.setModel(value);
    refreshSettings();
  }

  /** `latest` (switching from a TUI whose thread id we never saw) = newest thread in cwd. */
  function openThread(id: string | null) {
    if (id === "latest") request(LIST_ID, "thread/list", { cwd: ctx.cwd, limit: 1 });
    else if (id) request(THREAD_ID, "thread/resume", { threadId: id });
    else request(THREAD_ID, "thread/start", { cwd: ctx.cwd });
  }

  function renderItem(item: any, done: boolean) {
    if (!item?.id) return;
    switch (item.type) {
      case "userMessage": {
        const images = (item.content ?? []).filter((c: any) => c.type === "localImage" && typeof c.path === "string").map((c: any) => pathImage(c.path));
        ctx.upsert({ kind: "user", id: item.id, text: textOf((item.content ?? []).filter((c: any) => c.type === "text")), ...(images.length ? { images } : {}) });
        break;
      }
      case "agentMessage":
        ctx.upsert({ kind: "assistant", id: item.id, text: item.text ?? "", streaming: !done });
        break;
      case "reasoning": {
        const text = [...(item.summary ?? []), ...(item.content ?? [])].map((s: any) => (typeof s === "string" ? s : s?.text ?? "")).join("\n");
        if (text) ctx.upsert({ kind: "thinking", id: item.id, text, streaming: !done });
        break;
      }
      case "plan":
        // The plan Codex proposes in plan mode: an answer to read, not reasoning.
        ctx.upsert({ kind: "assistant", id: item.id, text: item.text ?? "", streaming: !done });
        break;
      case "commandExecution": {
        const cmd = Array.isArray(item.command) ? item.command.join(" ") : String(item.command ?? "");
        const prev = ctx.find(item.id);
        const view = commandView(item, cmd);
        ctx.upsert({
          kind: "tool", id: item.id, name: "Shell", summary: view.summary, category: view.category, input: { command: cmd },
          output: item.aggregatedOutput ?? (prev?.kind === "tool" ? prev.output : undefined),
          status: !done ? "running" : item.exitCode && item.exitCode !== 0 ? "error" : item.status === "failed" || item.status === "declined" ? "error" : "done",
        });
        break;
      }
      case "fileChange": {
        const changes: any[] = item.changes ?? [];
        ctx.upsert({
          kind: "tool", id: item.id, name: "Edit", summary: changes.map((c) => c.path).join(", "), category: "edit",
          status: !done ? "running" : item.status === "failed" || item.status === "declined" ? "error" : "done",
          diff: changes.map((c) => ({ path: c.path, text: c.diff ?? c.unified_diff ?? "" })),
        });
        break;
      }
      case "mcpToolCall":
      case "dynamicToolCall":
        ctx.upsert({
          kind: "tool", id: item.id, name: `${item.server ? item.server + "." : ""}${item.tool}`, summary: JSON.stringify(item.arguments ?? {}).slice(0, 120),
          output: item.result ? JSON.stringify(item.result).slice(0, 4000) : item.error ? String(item.error?.message ?? item.error) : undefined,
          status: !done ? "running" : item.error ? "error" : "done",
        });
        break;
      case "collabAgentToolCall": {
        // Its other tools (sendInput, wait, closeAgent) are about agents already listed.
        if (item.tool !== "spawnAgent") break;
        const prompt = typeof item.prompt === "string" ? item.prompt : "";
        ctx.upsert({ kind: "tool", id: item.id, name: "Agent", category: "agent", summary: summarize(prompt), input: { prompt }, status: !done ? "running" : item.status === "failed" ? "error" : "done" });
        break;
      }
      case "webSearch":
        ctx.upsert({ kind: "tool", id: item.id, name: "WebSearch", summary: item.query ?? "", category: "fetch", status: done ? "done" : "running" });
        break;
      case "contextCompaction":
        ctx.upsert({ kind: "system", id: item.id, text: t("Context compacted"), tone: "info" });
        break;
      case "exitedReviewMode":
        if (item.review) ctx.upsert({ kind: "assistant", id: item.id, text: String(item.review) });
        break;
    }
  }

  function renderTurns(turns: any[]) {
    for (const t of turns ?? []) for (const item of t.items ?? []) renderItem(item, true);
  }

  return {
    snapshot() {
      // `/new` forgets the thread until the daemon's answer names the next one: if the lines are not taken, it is still this one.
      const before = { threadId, turnId, threadReady, shown, conversation: ctx.state.conversationId, waiting: queued.length };
      return () => {
        threadId = before.threadId;
        turnId = before.turnId;
        threadReady = before.threadReady;
        shown = before.shown;
        queued.length = Math.min(queued.length, before.waiting);
        if (ctx.state.conversationId !== before.conversation) ctx.set("conversationId", before.conversation);
      };
    },
    reset() {
      initialized = false;
      threadReady = false;
      threadId = null;
      shown = null;
      turnId = null;
      turnDiff = null;
      approvals.clear();
      steered.clear();
    },
    start() {
      ctx.set("commands", commands());
      if (!initialized) {
        request(INIT_ID, "initialize", { clientInfo: { name: "kelpie", title: "Kelpie", version: "0.1.0" } });
      } else if (!threadReady) {
        openThread(ctx.state.conversationId);
      }
    },
    onMessage(m) {
      if (!m || typeof m !== "object") return;
      // Responses to our requests
      if ("id" in m && ("result" in m || "error" in m) && !("method" in m)) {
        if (m.error) {
          const steer = steered.get(String(m.id));
          if (steer != null) {
            // The turn ended (or takes no input) meanwhile: the message goes next instead.
            steered.delete(String(m.id));
            if (ctx.replaying()) return;
            if (ctx.requeue) ctx.requeue(steer);
            else ctx.set("queue", (q) => [{ id: `q-steer-${String(m.id)}`, text: steer, typed: false }, ...q]);
            return;
          }
          ctx.upsert({ kind: "system", id: `e-${String(m.id)}-${ctx.state.items.length}`, text: t("Codex: {error}", { error: String(m.error.message ?? JSON.stringify(m.error)) }), tone: "error" });
          if (/^kelpie-(turn|compact|review)-/.test(String(m.id))) ctx.set("busy", false);
          if ((m.id === THREAD_ID || m.id === LIST_ID) && ctx.state.conversationId && !ctx.replaying()) {
            // Resume failed (thread gone): start fresh instead.
            ctx.set("conversationId", null);
            openThread(null);
          }
          return;
        }
        if (steered.delete(String(m.id))) return;
        if (m.id === INIT_ID) {
          initialized = true;
          if (!ctx.replaying()) {
            ctx.write({ method: "initialized", params: {} });
            openThread(ctx.state.conversationId);
            request(MODELS_ID, "model/list", {});
            request(SKILLS_ID, "skills/list", { cwds: [ctx.cwd] });
          }
        } else if (m.id === MODELS_ID) {
          models = m.result?.data ?? [];
          refreshSettings();
        } else if (m.id === SKILLS_ID) {
          const skills = (m.result?.data ?? []).flatMap((e: any) => e.skills ?? []).filter((s: any) => s.enabled !== false);
          ctx.set("skills", skills.map((s: any): ChatCommand => ({ name: s.name, description: s.shortDescription ?? s.description, path: s.path })));
        } else if (m.id === LIST_ID) {
          const latest = m.result?.data?.[0] ?? m.result?.threads?.[0];
          if (!ctx.replaying()) openThread(latest?.id ?? null);
        } else if (m.id === THREAD_ID) {
          threadReady = true;
          const thread = m.result?.thread;
          threadId = thread?.id ?? null;
          // Another thread than the one shown (`/new`, live or replayed) starts empty.
          if (shown && threadId && threadId !== shown) ctx.clear();
          if (threadId) shown = threadId;
          if (threadId) ctx.setConversation(threadId);
          ctx.setModel(m.result?.model ?? null);
          if (m.result?.model) unlisted.add(m.result.model);
          model = (overrides.model as string | undefined) ?? m.result?.model ?? model;
          effort = (overrides.effort as string | undefined) ?? m.result?.reasoningEffort ?? effort;
          threadSandbox = m.result?.sandbox ?? threadSandbox;
          mode = modeOf(overrides.sandboxPolicy ?? m.result?.sandbox) ?? mode;
          refreshSettings();
          renderTurns(thread?.turns);
          if (!ctx.replaying()) {
            // Hidden: the thread echoes these with their pictures.
            for (const it of ctx.state.items) if (it.id.startsWith("queued-")) ctx.patch(it.id, (i: any) => { i.text = ""; i.images = undefined; });
            // "Busy" only covered the wait for the thread; whatever starts a turn sets it again.
            if (queued.length) ctx.set("busy", false);
            while (queued.length) {
              const { text, typed, images } = queued.shift()!;
              if (!runCommand(text, typed)) startTurn(text, images);
            }
          }
        } else if (ctx.replaying() && String(m.id).startsWith("kelpie-turn-")) {
          restorePicks(String(m.id));
        }
        return;
      }
      const p = m.params ?? {};
      // Server → client requests (approvals)
      if ("id" in m && "method" in m) {
        const id = `approval-${String(m.id)}`;
        approvals.set(id, { id: m.id, method: m.method });
        if (m.method.endsWith("requestApproval") || m.method === "execCommandApproval" || m.method === "applyPatchApproval") {
          const command = p.command ? (Array.isArray(p.command) ? p.command.join(" ") : String(p.command)) : undefined;
          const files = p.fileChanges ? Object.keys(p.fileChanges).join(", ") : null;
          // A v2 file-change approval names the item whose changes it is about.
          const item = p.itemId ? ctx.find(p.itemId) : undefined;
          const diff = item?.kind === "tool" ? item.diff : undefined;
          ctx.upsert({
            kind: "permission", id, title: m.method.includes("fileChange") || m.method === "applyPatchApproval" ? t("Apply file changes?") : t("Run command?"),
            command, diff,
            detail: [p.reason, files, command && p.cwd ? t("in {dir}", { dir: String(p.cwd) }) : null].filter(Boolean).join("\n") || (command || diff ? undefined : m.method),
            options: [
              { id: "accept", label: t("Allow"), tone: "allow" },
              { id: "acceptForSession", label: t("Allow for session"), tone: "allow" },
              { id: "decline", label: t("Decline"), tone: "deny" },
            ],
          });
        } else if (m.method === "item/tool/requestUserInput") {
          ctx.upsert({ kind: "system", id, text: t("Codex asked for input that chat mode cannot answer yet; switch to terminal."), tone: "info" });
        }
        return;
      }
      switch (m.method) {
        case "turn/started":
          turnId = p.turn?.id ?? p.turnId ?? null;
          ctx.set("busy", true);
          break;
        case "turn/completed":
          turnId = null;
          for (const it of ctx.state.items) if ((it as any).streaming) ctx.patch(it.id, (i: any) => (i.streaming = false));
          if (p.turn?.error) ctx.upsert({ kind: "system", id: `turn-err-${p.turn.id}`, text: String(p.turn.error.message ?? t("Turn failed")), tone: "error" });
          ctx.upsert({ kind: "result", id: `turn-${p.turn?.id ?? ctx.state.items.length}`, text: [p.turn?.status, p.turn?.durationMs ? `${(p.turn.durationMs / 1000).toFixed(1)}s` : null].filter(Boolean).join(" · "), diff: turnDiff ?? undefined });
          turnDiff = null;
          // Last: going idle sends the next queued message.
          ctx.set("busy", false);
          break;
        case "turn/diff/updated":
          turnDiff = typeof p.diff === "string" && p.diff.trim() ? p.diff : null;
          break;
        case "turn/plan/updated":
          ctx.set("plan", { entries: (p.plan ?? []).map((s: any) => ({ text: String(s.step ?? ""), status: planStatus(s.status) })), explanation: p.explanation ?? undefined });
          break;
        case "thread/tokenUsage/updated": {
          const u = p.tokenUsage ?? {};
          if (typeof u.last?.totalTokens === "number") ctx.set("usage", { used: u.last.totalTokens, size: u.modelContextWindow ?? null });
          break;
        }
        case "item/started":
          renderItem(p.item, false);
          break;
        case "item/completed":
          renderItem(p.item, true);
          break;
        case "item/agentMessage/delta":
          if (!ctx.find(p.itemId)) ctx.upsert({ kind: "assistant", id: p.itemId, text: "", streaming: true });
          ctx.patch(p.itemId, (i: any) => (i.text += p.delta ?? ""));
          break;
        case "item/reasoning/summaryTextDelta":
        case "item/reasoning/textDelta":
          if (!ctx.find(p.itemId)) ctx.upsert({ kind: "thinking", id: p.itemId, text: "", streaming: true });
          ctx.patch(p.itemId, (i: any) => (i.text += p.delta ?? ""));
          break;
        case "item/commandExecution/outputDelta":
          ctx.patch(p.itemId, (i: any) => {
            if (i.kind === "tool") i.output = ((i.output ?? "") + (p.delta ?? "")).slice(-20000);
          });
          break;
        case "error":
          ctx.upsert({ kind: "system", id: `err-${ctx.state.items.length}`, text: String(p.error?.message ?? p.message ?? t("Codex error")), tone: "error" });
          break;
      }
    },
    send(text, typed = false, images) {
      if (runCommand(text, typed)) return;
      if (!threadId) {
        queued.push({ text, typed, images });
        ctx.upsert({ kind: "user", id: `queued-${Date.now()}`, text, images: sentImages(images), pending: true });
        ctx.set("busy", true);
        return;
      }
      // No optimistic bubble: Codex echoes the prompt as a `userMessage` item right away.
      startTurn(text, images);
    },
    steer(text) {
      if (!threadId || !turnId) return false;
      const id = `kelpie-steer-${++seq}`;
      steered.set(id, text);
      request(id, "turn/steer", { threadId, input: userInput(text), expectedTurnId: turnId });
      return true;
    },
    immediate(text) {
      const slash = slashCommand(text);
      return !!slash && (slash[0] === "model" || TUI_ONLY.has(slash[0]));
    },
    // What `runCommand` takes: none of it starts a turn, so no picture would go anywhere.
    isCommand(text, typed) {
      if (typed && text.startsWith("!") && text.length > 1) return true;
      const slash = slashCommand(text);
      if (!slash) return false;
      const [name, args] = slash;
      return name === "compact" || name === "review" || name === "model" || TUI_ONLY.has(name) || (NEW_CONVERSATION_COMMANDS.has(name) && !args);
    },
    interrupt() {
      if (threadId && turnId) request(`kelpie-int-${++seq}`, "turn/interrupt", { threadId, turnId });
    },
    setSetting(id, value) {
      if (id === "model") return setModel(value);
      if (id === "effort") {
        effort = value;
        overrides.effort = value;
      } else if (id === "mode" && MODES[value]) {
        mode = value;
        modePicked = true;
        Object.assign(overrides, preset(value));
      }
      setSettingValue(ctx, id, value);
    },
    // The TUI's file picker inserts the bare path.
    mention: (path) => path,
    respond(id, option) {
      const a = approvals.get(id);
      if (!a) return;
      approvals.delete(id);
      const legacy = a.method === "execCommandApproval" || a.method === "applyPatchApproval";
      const decision = legacy ? { accept: "approved", acceptForSession: "approved_for_session", decline: "denied" }[option] ?? "denied" : option;
      ctx.write({ id: a.id, result: { decision } });
    },
    describeAnswer(card, answer) {
      const option = LOGGED_DECISION[String(answer.result?.decision)];
      const label = card.options.find((o) => o.id === option)?.label;
      return label ? { label } : null;
    },
  };
}
