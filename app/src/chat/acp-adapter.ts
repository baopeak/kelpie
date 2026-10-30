// Agent Client Protocol adapter (JSON-RPC 2.0 over stdio, protocol version 1) for every agent
// with an ACP mode (`ACP_ARGS` in lib/agents.ts: OMP, Gemini, OpenCode, Copilot, Goose, …).
// Sign-in: `agent` methods are run with `authenticate`; `terminal` methods (and the older
// `_meta["terminal-auth"]`) name a command to run in a terminal first. Questions the agent asks
// (`elicitation/create`, form mode) become a card in the transcript. We advertise no client fs/terminal capabilities so the agent uses its
// own tools, exactly like in its TUI. The agent lists its slash commands
// (`available_commands_update`, run by sending `/name args` as prompt text) and its settings
// (`configOptions`: mode, model, thought level) itself. `/new`, `/clear` and `/reset` start a
// new session (`session/new`) whatever the agent lists.
import { type Adapter, type AdapterContext, type ChatCommand, type ChatImage, type ChatItem, type ToolCategory, NEW_CONVERSATION_COMMANDS, logQuestionAnswers, loggedImage, putSetting, sentImages, setSettingValue, slashCommand, summarize, textOf } from "./chat-model";
import { lineDiff, planStatus } from "./tool-view";
import { t } from "../lib/i18n";
import { version as appVersion } from "../../package.json";

/** ACP tool kinds → the transcript's categories. */
const KINDS: Record<string, ToolCategory> = { read: "read", search: "search", edit: "edit", delete: "edit", move: "edit", execute: "command", fetch: "fetch" };

/** A turn's ACP stop reason in words: nothing for a normal finish (the reply is the result), and
 *  unknown reasons (newer protocol versions) as sent. */
const STOP_TEXT = (): Record<string, string> => ({
    end_turn: "",
    max_tokens: t("Stopped: response length limit"),
    max_turn_requests: t("Stopped: step limit for this turn"),
    refusal: t("The agent declined this request"),
    cancelled: t("Cancelled"),
});
const stopText = (reason: string): string => {
  const words = STOP_TEXT();
  return Object.hasOwn(words, reason) ? words[reason] : reason;
};


const INIT_ID = "kelpie-init";
const SESSION_ID = "kelpie-session";
/** `session/new` asked by `/new` or `/clear`: its answer replaces what is shown. */
const NEW_SESSION_ID = "kelpie-session-new";

/** The commands chat view runs itself, ahead of the agent's own. */
const ownCommands = (): ChatCommand[] => [
  { name: "new", description: t("Start a new conversation") },
  { name: "clear", description: t("Start a new conversation") },
];

/** `/new`, `/clear` or `/reset` with nothing after it. Run here for every agent: most do not
 *  know it in ACP mode (Gemini, OpenCode, OMP, Copilot pass it to the model as a prompt), and the
 *  ones that do (Goose, Qwen, Kimi) only empty the context of the same session. */
const isNewConversation = (text: string) => {
  const slash = slashCommand(text);
  return !!slash && NEW_CONVERSATION_COMMANDS.has(slash[0]) && !slash[1];
};

export function createAcpAdapter(ctx: AdapterContext): Adapter {
  let initialized = false;
  let canLoad = false;
  /** The agent takes pictures in a prompt (otherwise it is told where the file is). */
  let canImage = false;
  let sessionId: string | null = null;
  let seq = 0;
  let lastKind: "assistant" | "thinking" | "user" | null = null;
  let lastId: string | null = null;
  const requests = new Map<string, unknown>();
  /** The config option that holds the session mode (for `current_mode_update`). */
  let modeOption: string | null = null;
  /** Settings from the older `modes` / `models` blocks, set with `session/set_mode` / `set_model`. */
  const legacy = new Set<string>();
  /** The agent's sign-in methods (from `initialize`, or an `auth_required` error). */
  let authMethods: any[] = [];
  /** Open elicitations: card id → the request id and the schema properties behind each question. */
  const elicitations = new Map<string, { rid: unknown; fields: { key: string; question: string; type: string }[] }>();
  /** The turn is over, whether or not `busy` knew of it: a replay has no prompt, which is what sets it. */
  const endTurn = () => {
    ctx.expireApprovals?.();
    // Last: going idle sends the next queued message.
    ctx.set("busy", false);
  };
  /** Prompts typed before the session was ready; sent once it is. */
  const queued: { text: string; images?: ChatImage[] }[] = [];

  function prompt(text: string, images: ChatImage[] = []) {
    if (isNewConversation(text)) {
      sessionId = null;
      endStreaming();
      rpc(NEW_SESSION_ID, "session/new", { cwd: ctx.cwd, mcpServers: [] });
      return;
    }
    endStreaming();
    ctx.set("busy", true);
    const pictures = canImage ? images.map((i) => ({ type: "image", mimeType: i.mime, data: i.data })) : [];
    const words = [text, ...(canImage ? [] : images.map((i) => i.path))].filter(Boolean).join("\n");
    rpc(`kelpie-prompt-${++seq}-${Date.now()}`, "session/prompt", { sessionId, prompt: [...pictures, ...(words ? [{ type: "text", text: words }] : [])] });
  }

  const rpc = (id: string, method: string, params: unknown) => ctx.write({ jsonrpc: "2.0", id, method, params });
  const notify = (method: string, params: unknown) => ctx.write({ jsonrpc: "2.0", method, params });

  /** The last open was `session/load`: its failure falls back to a new session, never sign-in. */
  let loading = false;

  function openSession() {
    const resume = ctx.state.conversationId;
    loading = !!(resume && canLoad);
    if (resume && canLoad) rpc(SESSION_ID, "session/load", { sessionId: resume, cwd: ctx.cwd, mcpServers: [] });
    else rpc(SESSION_ID, "session/new", { cwd: ctx.cwd, mcpServers: [] });
  }

  /** The open segment is complete: ACP has no end-of-message event, so whatever comes after it
   *  (a tool call, another kind of chunk, the turn's or the load's end) says so. */
  function endStreaming() {
    if (lastKind && lastKind !== "user" && lastId) ctx.patch(lastId, (i: any) => (i.streaming = false));
    lastKind = null;
  }

  function appendChunk(kind: "assistant" | "thinking" | "user", text: string) {
    if (lastKind === kind && lastId) {
      ctx.patch(lastId, (i: any) => (i.text += text));
      return;
    }
    endStreaming();
    lastKind = kind;
    lastId = `${kind}-${++seq}`;
    ctx.upsert({ kind, id: lastId, text, streaming: kind !== "user" } as ChatItem);
  }

  /** A picture of the user message being loaded: its bytes, never a location the agent names for it. */
  function appendUserImage(c: any) {
    if (lastKind !== "user") appendChunk("user", "");
    ctx.patch(lastId!, (i: any) => {
      const images = i.images ?? [];
      i.images = [...images, loggedImage(images.length + 1, c.mimeType, ctx.namesOnly ? undefined : c.data)];
    });
  }

  function applyConfig(options: any[] | undefined) {
    for (const o of options ?? []) {
      if (o.type !== "select" || !Array.isArray(o.options)) continue;
      // Options may come grouped ({group, options: [...]}); the picker lists them flat.
      const flat = o.options.flatMap((x: any) => (Array.isArray(x.options) ? x.options : [x]));
      putSetting(ctx, { id: o.id, label: o.name ?? o.id, value: o.currentValue ?? null, options: flat.map((x: any) => ({ value: x.value, label: x.name ?? x.value, description: x.description })) });
      if (o.category === "model") ctx.setModel(o.currentValue ?? null);
      if (o.category === "mode") modeOption = o.id;
    }
  }

  /** Agents that predate config options send their modes and models in blocks of their own. */
  function applyLegacy(result: any) {
    const modes = result?.modes;
    if (modes && Array.isArray(modes.availableModes) && !ctx.state.settings.some((x) => x.id === modeOption)) {
      legacy.add("mode");
      modeOption = "mode";
      putSetting(ctx, { id: "mode", label: t("Mode"), value: modes.currentModeId ?? null, options: modes.availableModes.map((m: any) => ({ value: m.id, label: m.name ?? m.id, description: m.description })) });
    }
    const models = result?.models;
    if (models && Array.isArray(models.availableModels) && !ctx.state.settings.some((x) => x.id === "model")) {
      legacy.add("model");
      putSetting(ctx, { id: "model", label: t("Model"), value: models.currentModelId ?? null, options: models.availableModels.map((m: any) => ({ value: m.modelId, label: m.name ?? m.modelId, description: m.description })) });
      ctx.setModel(models.currentModelId ?? null);
    }
  }

  /** The agent needs a sign-in before a session opens: offer its methods. */
  function askSignIn(methods: any[]) {
    if (methods.length) authMethods = methods;
    const id = `auth-${++seq}`;
    const terminalCommand = (m: any) => {
      const meta = m?._meta?.["terminal-auth"] ?? (m?._meta?.type === "terminal" ? m._meta : null);
      if (m?.type === "terminal" || meta) {
        const args: string[] = m.args ?? meta?.args ?? [];
        return [meta?.command ?? t("the agent's command"), ...args].join(" ").trim();
      }
      return null;
    };
    ctx.upsert({
      kind: "permission", id, title: t("Sign in to continue"),
      detail: authMethods.map((m) => `${m.name ?? m.id}${m.description ? ` — ${m.description}` : ""}${terminalCommand(m) ? `\n  ${t("Run in a terminal:")} ${terminalCommand(m)}` : ""}`).join("\n"),
      options: [
        ...authMethods.filter((m) => !terminalCommand(m)).map((m) => ({ id: `method:${m.id}`, label: m.name ?? m.id, tone: "allow" as const })),
        { id: "retry", label: t("I have signed in, try again") },
      ],
    });
    requests.set(id, "auth");
  }

  function toolContent(content: any[]): { output?: string; diff?: { path: string; text: string }[] } {
    const out: string[] = [];
    const diff: { path: string; text: string }[] = [];
    for (const c of content ?? []) {
      if (c.type === "content") out.push(textOf(c.content));
      else if (c.type === "diff") diff.push({ path: c.path, text: lineDiff(String(c.oldText ?? ""), String(c.newText ?? "")) });
    }
    return { output: out.join("\n") || undefined, diff: diff.length ? diff : undefined };
  }

  function onUpdate(u: any) {
    switch (u.sessionUpdate) {
      case "agent_message_chunk":
        appendChunk("assistant", textOf(u.content));
        break;
      case "agent_thought_chunk":
        appendChunk("thinking", textOf(u.content));
        break;
      case "user_message_chunk":
        if (u.content?.type === "image") appendUserImage(u.content);
        else appendChunk("user", textOf(u.content));
        break;
      case "tool_call": {
        endStreaming();
        const c = toolContent(u.content);
        ctx.upsert({
          kind: "tool", id: u.toolCallId, name: u.title ?? u.kind ?? "tool", summary: summarize(u.locations?.[0]?.path ?? u.rawInput ?? ""), input: u.rawInput,
          category: KINDS[u.kind] ?? "other", status: u.status === "failed" ? "error" : u.status === "completed" ? "done" : "running", ...c,
        });
        break;
      }
      case "tool_call_update": {
        endStreaming();
        const c = toolContent(u.content);
        ctx.patch(u.toolCallId, (i: any) => {
          if (u.title) i.name = u.title;
          if (u.kind) i.category = KINDS[u.kind] ?? "other";
          if (u.status) i.status = u.status === "failed" ? "error" : u.status === "completed" ? "done" : "running";
          if (c.output) i.output = c.output;
          if (c.diff) i.diff = c.diff;
          if (u.rawOutput && !c.output) i.output = typeof u.rawOutput === "string" ? u.rawOutput : JSON.stringify(u.rawOutput).slice(0, 4000);
        });
        break;
      }
      case "plan":
        // Each update is the whole plan: it replaces the last one.
        ctx.set("plan", { entries: (u.entries ?? []).map((e: any) => ({ text: String(e.content ?? ""), status: planStatus(e.status) })) });
        break;
      case "usage_update":
        // The session's cost so far; an update that leaves it out (context only) keeps it.
        if (typeof u.used === "number") ctx.set("usage", { used: u.used, size: typeof u.size === "number" ? u.size : null, cost: typeof u.cost?.amount === "number" ? u.cost.amount : ctx.state.usage?.cost });
        break;
      case "current_mode_update":
        if (modeOption && u.currentModeId) setSettingValue(ctx, modeOption, u.currentModeId);
        break;
      case "available_commands_update":
        ctx.set("commands", [
          ...ownCommands(),
          // An agent's own `clear` would be listed twice: chat view runs it (see isNewConversation).
          ...(u.availableCommands ?? []).filter((c: any) => !NEW_CONVERSATION_COMMANDS.has(c.name)).map((c: any): ChatCommand => ({ name: c.name, description: c.description, hint: c.input?.hint })),
        ]);
        break;
      case "config_option_update":
        applyConfig(u.configOptions);
        break;
    }
  }

  return {
    reset() {
      initialized = false;
      canLoad = false;
      canImage = false;
      sessionId = null;
      lastKind = null;
      requests.clear();
      elicitations.clear();
    },
    start() {
      if (!ctx.state.commands.length) ctx.set("commands", ownCommands());
      // No fs / terminal: the agent uses its own tools, as in its terminal UI. Sign-in methods
      // that need a terminal are offered (`auth.terminal`, and the older `terminal-auth`).
      if (!initialized) rpc(INIT_ID, "initialize", {
        protocolVersion: 1,
        clientCapabilities: { fs: { readTextFile: false, writeTextFile: false }, terminal: false, auth: { terminal: true }, elicitation: { form: {} }, _meta: { "terminal-auth": true } },
        clientInfo: { name: "kelpie", title: "Kelpie", version: appVersion },
      });
      else if (!sessionId) openSession();
    },
    onMessage(m) {
      if (!m || typeof m !== "object") return;
      if ("id" in m && !("method" in m)) {
        if (m.error) {
          // A load that failed part way replays no more of that reply.
          if (m.id === SESSION_ID) endStreaming();
          // Not signed in: `session/new` answers `auth_required` (-32000), maybe with the methods.
          const needsAuth = (m.id === SESSION_ID || m.id === NEW_SESSION_ID) && !(m.id === SESSION_ID && loading)
            && (m.error.code === -32000 || /auth/i.test(String(m.error.message ?? "")));
          if (needsAuth && !ctx.replaying()) {
            askSignIn(m.error.data?.authMethods ?? authMethods);
            return;
          }
          ctx.upsert({ kind: "system", id: `e-${String(m.id)}`, text: t("Agent: {error}", { error: String(m.error.message ?? JSON.stringify(m.error)) }), tone: "error" });
          // Sign-in failed (not set up yet, a method this client cannot complete): offer them again.
          if (String(m.id).startsWith("kelpie-auth-") && !ctx.replaying()) {
            askSignIn(authMethods);
            return;
          }
          if (String(m.id).startsWith("kelpie-prompt-")) {
            endStreaming();
            endTurn();
          }
          // No new session: stay in the one shown.
          if (m.id === NEW_SESSION_ID) sessionId = ctx.state.conversationId;
          if (m.id === SESSION_ID && ctx.state.conversationId && !ctx.replaying()) {
            ctx.set("conversationId", null);
            openSession();
          }
          return;
        }
        if (m.id === INIT_ID) {
          const version = m.result?.protocolVersion;
          if (version !== undefined && version !== 1) {
            ctx.upsert({ kind: "system", id: "acp-version", text: t("This agent speaks ACP version {version}; Kelpie speaks version 1.", { version: String(version) }), tone: "error" });
            return;
          }
          initialized = true;
          canLoad = !!m.result?.agentCapabilities?.loadSession;
          canImage = !!m.result?.agentCapabilities?.promptCapabilities?.image;
          authMethods = Array.isArray(m.result?.authMethods) ? m.result.authMethods : [];
          if (!ctx.replaying()) openSession();
        } else if (String(m.id).startsWith("kelpie-auth-")) {
          // Signed in: open the session now.
          if (!ctx.replaying()) openSession();
        } else if (m.id === SESSION_ID || m.id === NEW_SESSION_ID) {
          // A new conversation starts empty, live or replayed (the old one stays loadable).
          if (m.id === NEW_SESSION_ID) ctx.clear();
          sessionId = m.result?.sessionId ?? ctx.state.conversationId;
          if (sessionId) ctx.setConversation(sessionId);
          applyConfig(m.result?.configOptions);
          applyLegacy(m.result);
          endStreaming();
          if (!ctx.replaying()) {
            // "Busy" only covered the wait for the session; a prompt sets it again.
            if (queued.length) ctx.set("busy", false);
            while (queued.length) {
              const next = queued.shift()!;
              prompt(next.text, next.images);
            }
          }
        } else if (Array.isArray(m.result?.configOptions)) {
          applyConfig(m.result.configOptions);
        } else if (m.result && "stopReason" in m.result) {
          endStreaming();
          ctx.upsert({ kind: "result", id: `stop-${String(m.id)}`, text: stopText(String(m.result.stopReason)) });
          endTurn();
        }
        return;
      }
      if (m.method === "session/update") return onUpdate(m.params?.update ?? {});
      if (m.method === "elicitation/create" && "id" in m) {
        const params = m.params ?? {};
        if (params.mode && params.mode !== "form") {
          // URL mode (sign-in in a browser): not handled here; tell the agent so it does not wait.
          if (!ctx.replaying()) ctx.write({ jsonrpc: "2.0", id: m.id, result: { action: "decline" } });
          ctx.upsert({ kind: "system", id: `elicit-${String(m.id)}`, text: t("The agent asked to open {url}", { url: String(params.url ?? "") }), tone: "info" });
          return;
        }
        const props = params.requestedSchema?.properties ?? {};
        const required: string[] = Array.isArray(params.requestedSchema?.required) ? params.requestedSchema.required : Object.keys(props);
        const fields = Object.entries(props)
          .filter(([key]) => required.includes(key))
          .map(([key, def]: [string, any]) => ({ key, question: String(def.title ?? def.description ?? key), type: String(def.type ?? "string") }));
        const id = `elicit-${String(m.id)}`;
        elicitations.set(id, { rid: m.id, fields });
        ctx.upsert({
          kind: "permission", id, title: String(params.message ?? t("The agent asks:")),
          ask: fields.map((f) => {
            const def = props[f.key] ?? {};
            const source = f.type === "array" ? def.items ?? {} : def;
            const choices: string[] = source.enum ?? (source.oneOf ?? source.anyOf ?? []).map((o: any) => o.const ?? o.title) ?? [];
            const options = f.type === "boolean" ? [{ label: t("Yes") }, { label: t("No") }] : choices.map((c) => ({ label: String(c) }));
            return { key: f.key, question: f.question, header: def.title && def.description ? String(def.description) : undefined, options, multiSelect: f.type === "array" };
          }),
          // Nothing to fill in: the agent only asks to go ahead.
          options: [...(fields.length ? [] : [{ id: "answer", label: t("Accept"), tone: "allow" as const }]), { id: "decline", label: t("Decline"), tone: "deny" }, { id: "cancel", label: t("Cancel") }],
        });
        return;
      }
      if (m.method === "session/request_permission" && "id" in m) {
        const id = `perm-${String(m.id)}`;
        requests.set(id, m.id);
        const tc = m.params?.toolCall ?? {};
        ctx.upsert({
          kind: "permission", id, title: tc.title ? t("Allow: {title}?", { title: String(tc.title) }) : t("Allow this action?"),
          detail: summarize(tc.rawInput ?? "", 600),
          options: (m.params?.options ?? []).map((o: any) => ({ id: o.optionId, label: o.name, tone: String(o.kind).startsWith("allow") ? "allow" : "deny" })),
        });
        return;
      }
      if ("id" in m && "method" in m) {
        // Unsupported client method: tell the agent rather than leaving it hanging.
        if (!ctx.replaying()) ctx.write({ jsonrpc: "2.0", id: m.id, error: { code: -32601, message: `Kelpie does not implement ${m.method}` } });
      }
    },
    // Until the agent says what it takes there is nothing to warn about.
    takesPictures: () => !initialized || canImage,
    isCommand: (text) => isNewConversation(text),
    send(text, _typed, images) {
      if (!isNewConversation(text)) ctx.upsert({ kind: "user", id: `local-${Date.now()}`, text, images: sentImages(images) });
      if (!sessionId) {
        queued.push({ text, images });
        ctx.set("busy", true);
        return;
      }
      prompt(text, images);
    },
    interrupt() {
      if (!sessionId) return;
      notify("session/cancel", { sessionId });
      // Settled meanwhile (answered elsewhere, expired): that request is over. One still open, also if a
      // replay showed it, is what the agent is waiting on, and this is the person's own Stop.
      const settled = (id: string) => {
        const card = ctx.find(id);
        return card?.kind === "permission" && !!card.resolved;
      };
      // ACP: after a cancel the client answers every open permission request as cancelled,
      // or the agent may never end the turn.
      for (const [id, rid] of requests) {
        if (rid === "auth") continue;
        requests.delete(id);
        if (settled(id)) continue;
        ctx.write({ jsonrpc: "2.0", id: rid, result: { outcome: { outcome: "cancelled" } } });
        ctx.patch(id, (i: any) => {
          if (i.kind === "permission" && !i.resolved) i.resolved = t("Cancelled");
        });
      }
      // Questions the agent asked would keep it waiting just the same.
      for (const [id, e] of elicitations) {
        elicitations.delete(id);
        if (settled(id)) continue;
        ctx.write({ jsonrpc: "2.0", id: e.rid, result: { action: "cancel" } });
        ctx.patch(id, (i: any) => {
          if (i.kind === "permission" && !i.resolved) i.resolved = t("Cancelled");
        });
      }
    },
    setSetting(id, value) {
      if (!sessionId) return;
      if (legacy.has(id)) {
        if (id === "mode") rpc(`kelpie-config-${++seq}`, "session/set_mode", { sessionId, modeId: value });
        else rpc(`kelpie-config-${++seq}`, "session/set_model", { sessionId, modelId: value });
        setSettingValue(ctx, id, value);
        if (id === "model") ctx.setModel(value);
        return;
      }
      rpc(`kelpie-config-${++seq}`, "session/set_config_option", { sessionId, configId: id, value });
    },
    respond(id, option, reply) {
      const elicit = elicitations.get(id);
      if (elicit) {
        elicitations.delete(id);
        if (option !== "answer") {
          ctx.write({ jsonrpc: "2.0", id: elicit.rid, result: { action: option === "decline" ? "decline" : "cancel" } });
          return;
        }
        const content: Record<string, unknown> = {};
        for (const f of elicit.fields) {
          const v = reply?.answers?.[f.question];
          if (v === undefined) continue;
          content[f.key] = f.type === "boolean" ? v === t("Yes") : f.type === "number" || f.type === "integer" ? Number(v) : f.type === "array" ? v.split(", ") : v;
        }
        ctx.write({ jsonrpc: "2.0", id: elicit.rid, result: { action: "accept", content } });
        return;
      }
      if (requests.get(id) === "auth") {
        requests.delete(id);
        if (option.startsWith("method:")) rpc(`kelpie-auth-${++seq}`, "authenticate", { methodId: option.slice(7) });
        else openSession();
        return;
      }
      const rid = requests.get(id);
      if (rid === undefined) return;
      requests.delete(id);
      ctx.write({ jsonrpc: "2.0", id: rid, result: { outcome: { outcome: "selected", optionId: option } } });
    },
    describeAnswer(card, answer) {
      const result = answer.result ?? {};
      if (result.action === "accept") {
        const said: Record<string, string> = {};
        for (const q of card.ask ?? []) {
          const v = result.content?.[q.key ?? q.question];
          if (v === undefined) continue;
          said[q.question] = v === true ? t("Yes") : v === false ? t("No") : Array.isArray(v) ? v.join(", ") : String(v);
        }
        const questions = logQuestionAnswers(said);
        if (questions) return questions;
      }
      const id = result.action === "accept" ? "answer" : result.action ?? result.outcome?.optionId;
      const label = card.options.find((o) => o.id === id)?.label;
      return label ? { label } : null;
    },
  };
}
