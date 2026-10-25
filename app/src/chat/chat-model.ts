// Chat-mode state shared by all agent adapters. Adapters translate their wire protocol
// (Claude stream-json, Codex app-server, ACP) into these items; the UI only knows items.
import { createStore, produce, type SetStoreFunction } from "solid-js/store";
import { attachTerminal, b64ToBytes, daemon, sendInput } from "../lib/ipc";
import { t } from "../lib/i18n";

/** What a tool call does, for its row and for folding runs of reads and searches. */
export type ToolCategory = "read" | "search" | "edit" | "command" | "fetch" | "agent" | "plan" | "other";

/** A question the agent asks with choices (Claude's AskUserQuestion). */
export interface AskQuestion {
  question: string;
  header?: string;
  options: { label: string; description?: string }[];
  multiSelect?: boolean;
}

export interface PlanEntry {
  text: string;
  status: "pending" | "active" | "done";
}

/** `at` / `doneAt`: when the item appeared / finished, live only (a replayed log has no times). */
export type ChatItem =
  | { kind: "user"; id: string; text: string; pending?: boolean; at?: number }
  | { kind: "assistant"; id: string; text: string; streaming?: boolean; group?: string; at?: number }
  | { kind: "thinking"; id: string; text: string; streaming?: boolean; group?: string; at?: number; doneAt?: number }
  | { kind: "tool"; id: string; name: string; summary: string; input?: unknown; output?: string; status: "running" | "done" | "error"; diff?: { path: string; text: string }[]; category?: ToolCategory; at?: number }
  | {
      kind: "permission"; id: string; title: string; detail?: string; options: { id: string; label: string; tone?: "allow" | "deny" }[]; resolved?: string; at?: number;
      /** Shown in the card: the command to run, the edit to make, the plan to approve. */
      command?: string; diff?: { path: string; text: string }[]; plan?: string;
      /** Questions answered in the card instead of with options. */
      ask?: AskQuestion[];
      /** The deny option can carry a message to the agent. */
      feedback?: boolean;
    }
  /** `tui`: the agent can run `command` only in its terminal UI; offer to continue there. */
  | { kind: "system"; id: string; text: string; tone?: "error" | "info"; tui?: { command: string }; at?: number }
  /** `diff`: the turn's changes as one unified diff, when the agent reports it (Codex). */
  | { kind: "result"; id: string; text: string; diff?: string; at?: number };

/** What the user adds to an answer: a message with a denial, answers to questions. */
export interface PermissionReply {
  message?: string;
  answers?: Record<string, string>;
}

/** A message sent while the agent was busy, sent when its turn ends. */
export interface QueuedMessage {
  id: string;
  text: string;
  typed: boolean;
}

/** Work the agent runs alongside its turn (a background shell, a subagent), from its task events;
 *  listed while it runs. `background`: the turn does not wait for it. */
export interface AgentTask {
  id: string;
  description: string;
  background: boolean;
  /** The agent's one-line status for it, when it gives one. */
  summary?: string;
}

/** A command offered by the composer: `/name` (or `$name`, a Codex skill). */
export interface ChatCommand {
  name: string;
  description?: string;
  /** Argument hint shown after the name. */
  hint?: string;
  /** Works only in the agent's terminal UI. */
  tuiOnly?: boolean;
  /** Codex skill file, sent along with a `$name` mention. */
  path?: string;
}

/** A setting under the composer (model, effort, permission mode), changed with `setSetting`. */
export interface ChatSetting {
  id: string;
  label: string;
  value: string | null;
  options: { value: string; label: string; description?: string }[];
}

export interface ChatState {
  items: ChatItem[];
  commands: ChatCommand[];
  skills: ChatCommand[];
  settings: ChatSetting[];
  busy: boolean;
  model: string | null;
  /** Agent-side conversation id (Claude session id, Codex thread id, ACP session id). */
  conversationId: string | null;
  ready: boolean;
  exited: boolean;
  status: string | null;
  /** When the agent started working (live only), for the elapsed time. */
  busySince: number | null;
  queue: QueuedMessage[];
  /** Stopping the agent holds the queue until the user sends again. */
  queuePaused: boolean;
  /** The agent's current task list (Claude TodoWrite, Codex plan updates, ACP plan). */
  plan: { entries: PlanEntry[]; explanation?: string } | null;
  /** Context in use, the model's window (null when the agent does not say) and the cost. */
  usage: { used: number; size: number | null; cost?: number } | null;
  /** Tasks still running (Claude's task events); emptied when the agent exits. */
  tasks: AgentTask[];
}

export interface Adapter {
  /** Called once per process after the log replay finished (handshake / resume). */
  start(): void;
  /** One stdout line from the agent (already JSON-parsed when possible). */
  onMessage(msg: any, raw: string): void;
  /** `typed`: the user wrote it in the composer (not text Kelpie sends for them). */
  send(text: string, typed?: boolean): void;
  /** Delivers `text` into the running turn; false when the agent cannot take it mid-turn. */
  steer?(text: string): boolean;
  /** Text that starts no turn (a picker command, a terminal-only one): never queued. */
  immediate?(text: string, typed: boolean): boolean;
  interrupt(): void;
  respond(permissionId: string, optionId: string, reply?: PermissionReply): void;
  /** Reset internal bookkeeping before a full log replay. */
  reset(): void;
  setSetting?(id: string, value: string): void;
  /** What the conversation ran with before this front-end (model, effort), shown until the
   *  agent reports its own. */
  seed?(settings: { model?: string | null; effort?: string | null }): void;
  /** How a picked file goes into the message (default `@path`). */
  mention?(path: string): string;
}

export interface AdapterContext {
  state: ChatState;
  set: SetStoreFunction<ChatState>;
  /** Writes one JSON line to the agent's stdin. */
  write(obj: unknown): void;
  upsert(item: ChatItem): void;
  patch<T extends ChatItem>(id: string, fn: (item: T) => void): void;
  find(id: string): ChatItem | undefined;
  /** Empties the transcript: the agent moved to a new conversation (`/clear`, `/new`). */
  clear(): void;
  setConversation(id: string): void;
  setModel(model: string | null): void;
  /** A replayed transcript line's own time (unix ms): items added after it carry it. */
  setClock?(at: number): void;
  cwd: string;
  resumeId: string | null;
  replaying: () => boolean;
}

export function emptyChatState(conversationId: string | null = null): ChatState {
  return {
    items: [], commands: [], skills: [], settings: [], busy: false, model: null, conversationId, ready: false, exited: false, status: null,
    busySince: null, queue: [], queuePaused: false, plan: null, usage: null, tasks: [],
  };
}

/** Owns one chat session's stream: attach, line framing, replay, adapter dispatch. */
export class ChatController {
  readonly state: ChatState;
  readonly set: SetStoreFunction<ChatState>;
  private index = new Map<string, number>();
  private buffer = "";
  private decoder = new TextDecoder();
  private adapter!: Adapter;
  private handle: number | null = null;
  private attachGen = 0;
  private isReplaying = false;
  /** Answered approval ids (the agent's log does not contain our answers). */
  private resolved = new Map<string, string>();
  private started = false;
  private flushing = false;
  private queued = 0;
  /** When the log or transcript being replayed says its next line came (unix ms). */
  private clock: number | null = null;
  private loadingHistory = false;
  /** Renders the agent's own transcript (history from before chat mode) ahead of the log. */
  loadHistory: ((ctx: AdapterContext) => Promise<void>) | null = null;

  constructor(readonly sessionId: string, readonly cwd: string, readonly resumeId: string | null, makeAdapter: (ctx: AdapterContext) => Adapter) {
    const [state, set] = createStore<ChatState>(emptyChatState(resumeId));
    this.state = state;
    // Any state change may start or end a turn: note when work starts, send the next queued message.
    this.set = ((...args: unknown[]) => {
      const wasBusy = state.busy;
      (set as (...a: unknown[]) => void)(...args);
      if (state.busy !== wasBusy) set("busySince", state.busy && !this.isReplaying ? Date.now() : null);
      if (wasBusy && !state.busy) this.expireApprovals();
      this.flushQueue();
    }) as SetStoreFunction<ChatState>;
    this.adapter = makeAdapter(this.context());
  }

  private context(): AdapterContext {
    return {
      state: this.state,
      set: this.set,
      // Replays must never talk to the agent: those messages were answered long ago.
      write: (obj) => this.handle != null && !this.isReplaying && void sendInput(this.handle, JSON.stringify(obj) + "\n"),
      upsert: (item) => this.upsert(item),
      patch: (id, fn) => this.patch(id, fn as (i: ChatItem) => void),
      find: (id) => {
        const i = this.index.get(id);
        return i == null ? undefined : this.state.items[i];
      },
      clear: () => {
        this.index.clear();
        this.set({ items: [], plan: null, usage: null });
      },
      setConversation: (id) => {
        if (this.state.conversationId === id) return;
        this.set("conversationId", id);
        void daemon("session.update", { handle: this.handle, agent_session_id: id }).catch(() => {});
      },
      setModel: (model) => {
        if (this.state.model === model) return;
        this.set("model", model);
        if (model && !this.isReplaying) void daemon("session.update", { handle: this.handle, model }).catch(() => {});
      },
      setClock: (at) => (this.clock = at),
      cwd: this.cwd,
      resumeId: this.resumeId,
      replaying: () => this.isReplaying,
    };
  }

  upsert(item: ChatItem) {
    if (item.kind === "permission" && this.resolved.has(item.id)) item = { ...item, resolved: this.resolved.get(item.id) };
    const i = this.index.get(item.id);
    if (i != null) {
      const prev = this.state.items[i];
      item = { ...item, at: prev.at ?? item.at } as ChatItem;
      if (item.kind === "thinking" && prev.kind === "thinking") item.doneAt = prev.doneAt;
      this.set("items", i, finished(item));
      return;
    }
    // Live items are stamped now; replayed ones (log, transcript) with the time the log carried.
    const at = this.isReplaying || this.loadingHistory ? this.clock : Date.now();
    if (item.at == null && at != null) item = { ...item, at } as ChatItem;
    this.index.set(item.id, this.state.items.length);
    this.set("items", this.state.items.length, item);
  }

  patch(id: string, fn: (item: ChatItem) => void) {
    const i = this.index.get(id);
    if (i == null) return;
    this.set("items", i, produce((it: ChatItem) => {
      fn(it);
      finished(it);
    }));
  }

  private resetState() {
    this.index.clear();
    this.buffer = "";
    this.set({ items: [], busy: false, ready: false, status: null, plan: null, usage: null, tasks: [] });
    this.adapter.reset();
  }

  private feed(bytes: Uint8Array) {
    this.buffer += this.decoder.decode(bytes, { stream: true });
    let nl: number;
    while ((nl = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, nl).trim();
      this.buffer = this.buffer.slice(nl + 1);
      if (!line) continue;
      let msg: any = null;
      try {
        msg = JSON.parse(line);
      } catch {
        /* non-JSON noise */
      }
      if (msg?.type === "kelpie_time") {
        this.clock = msg.at;
        continue;
      }
      if (msg?.type === "kelpie_exit") {
        this.set({ exited: true, busy: false, tasks: [] });
        this.upsert({ kind: "system", id: `exit-${Date.now()}`, text: t("Agent process exited (code {code}).", { code: String(msg.code) }), tone: "info" });
        // Nothing can take them now: show what was not sent rather than keep dead chips.
        if (this.state.queue.length) {
          this.upsert({ kind: "system", id: `unsent-${Date.now()}`, text: t("Not sent:\n{messages}", { messages: this.state.queue.map((q) => q.text).join("\n") }), tone: "info" });
          this.set("queue", []);
        }
        continue;
      }
      if (msg?.type === "kelpie_stderr") {
        // Agents colour their logs: the escapes would show as text.
        const text = String(msg.text).replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
        if (/error|fatal|panic/i.test(text)) this.upsert({ kind: "system", id: `err-${this.state.items.length}`, text, tone: "error" });
        continue;
      }
      try {
        this.adapter.onMessage(msg, line);
      } catch (e) {
        console.warn("chat adapter error", e, line);
      }
    }
  }

  async attach(handle: number) {
    const gen = ++this.attachGen;
    this.handle = handle;
    const early: Uint8Array[] = [];
    let applied = false;
    const reply = await attachTerminal(handle, null, (_seq, data) => {
      if (gen !== this.attachGen) return;
      if (applied) this.feed(data);
      else early.push(data);
    }).catch((e) => {
      this.upsert({ kind: "system", id: "attach-error", text: t("Could not attach: {error}", { error: String(e) }), tone: "error" });
      return null;
    });
    if (!reply || gen !== this.attachGen) return;
    this.resetState();
    this.clock = null;
    this.loadingHistory = true;
    if (this.loadHistory) await this.loadHistory(this.context()).catch(() => {});
    this.loadingHistory = false;
    if (gen !== this.attachGen) return;
    this.clock = null;
    this.isReplaying = true;
    this.feed(b64ToBytes(reply.data));
    this.isReplaying = false;
    applied = true;
    for (const chunk of early) this.feed(chunk);
    this.set("ready", true);
    if (!this.started && !this.state.exited) {
      this.started = true;
      this.adapter.start();
    }
  }

  detach() {
    this.attachGen++;
    if (this.handle != null) void daemon("session.detach", { handle: this.handle }).catch(() => {});
  }

  /** Sends now, or queues while the agent works (or while earlier queued messages wait). */
  send(text: string, typed = false) {
    if (!text.trim() || this.state.exited) return;
    // Not attached yet: a re-attach replays the log and would wipe what is sent now.
    if ((this.state.busy || this.state.queue.length || !this.state.ready) && !this.adapter.immediate?.(text, typed)) {
      this.set("queue", (q) => [...q, { id: `q-${++this.queued}`, text, typed }]);
      this.set("queuePaused", false);
      return;
    }
    this.adapter.send(text, typed);
  }

  /** Sends the next queued message once the agent is idle — a microtask later, as this runs
   *  inside the adapter's own state change, which must finish first. */
  private flushQueue() {
    if (this.flushing || !this.state.queue.length) return;
    this.flushing = true;
    queueMicrotask(() => {
      this.flushing = false;
      const s = this.state;
      if (s.busy || !s.ready || s.exited || s.queuePaused || this.isReplaying || !s.queue.length) return;
      const [next, ...rest] = s.queue;
      this.set("queue", rest);
      this.adapter.send(next.text, next.typed);
    });
  }

  /** A turn that ended took its open approvals with it (answered before a replay — the log has
   *  no answers — or dropped by the agent): they must not look, or be answered, as waiting. */
  private expireApprovals() {
    for (const it of this.state.items) {
      if (it.kind === "permission" && !it.resolved) {
        this.resolved.set(it.id, t("No longer needed"));
        this.patch(it.id, (i) => {
          if (i.kind === "permission") i.resolved = t("No longer needed");
        });
      }
    }
  }

  unqueue(id: string) {
    this.set("queue", (q) => q.filter((m) => m.id !== id));
  }

  /** Puts a queued message into the running turn, or stops the turn so that it goes next. */
  sendNow(id: string) {
    const m = this.state.queue.find((x) => x.id === id);
    if (!m || this.state.exited || !this.state.ready) return;
    // A command (`/compact`, `!ls`) starts something of its own: it never rides in a turn.
    const command = /^[/!]/.test(m.text.trim());
    if (!this.state.busy) {
      this.unqueue(id);
      this.adapter.send(m.text, m.typed);
    } else if (!command && this.adapter.steer?.(m.text)) {
      this.unqueue(id);
    } else {
      this.set("queue", (q) => [m, ...q.filter((x) => x.id !== id)]);
      this.set("queuePaused", false);
      this.adapter.interrupt();
    }
  }

  resumeQueue() {
    this.set("queuePaused", false);
  }

  interrupt() {
    // Stopping means "wait": queued messages stay until the user sends again.
    if (this.state.queue.length) this.set("queuePaused", true);
    this.adapter.interrupt();
  }

  setSetting(id: string, value: string) {
    if (this.state.exited) return;
    this.adapter.setSetting?.(id, value);
  }

  seed(settings: { model?: string | null; effort?: string | null }) {
    this.adapter.seed?.(settings);
  }

  mention(path: string): string {
    return this.adapter.mention?.(path) ?? `@${path}`;
  }

  respond(permissionId: string, optionId: string, label: string, reply?: PermissionReply) {
    if (this.resolved.has(permissionId)) return;
    this.resolved.set(permissionId, label);
    this.adapter.respond(permissionId, optionId, reply);
    this.patch(permissionId, (i) => {
      if (i.kind === "permission") i.resolved = label;
    });
  }
}

/** A thinking item that just stopped streaming gets its end time (for "Thought for 4s"). */
function finished(item: ChatItem): ChatItem {
  if (item.kind === "thinking" && !item.streaming && item.at != null && item.doneAt == null) item.doneAt = Date.now();
  return item;
}

/** Updates (or adds) one composer setting. */
export function putSetting(ctx: AdapterContext, setting: ChatSetting) {
  const i = ctx.state.settings.findIndex((s) => s.id === setting.id);
  ctx.set("settings", i < 0 ? ctx.state.settings.length : i, setting);
}

export function setSettingValue(ctx: AdapterContext, id: string, value: string | null) {
  const i = ctx.state.settings.findIndex((s) => s.id === id);
  if (i >= 0) ctx.set("settings", i, "value", value);
}

/** Commands that start a new conversation, under the names the agents' TUIs use (Claude Code,
 *  Copilot, Qwen, Kimi say `/reset`). Adapters whose agent has no such command run them itself. */
export const NEW_CONVERSATION_COMMANDS = new Set(["new", "clear", "reset"]);

/** `/name args` → ["name", "args"], else null. */
export function slashCommand(text: string): [string, string] | null {
  const m = /^\/([\w:.-]+)(?:\s+([\s\S]*))?$/.exec(text.trim());
  return m ? [m[1], (m[2] ?? "").trim()] : null;
}

export function summarize(input: unknown, max = 120): string {
  if (input == null) return "";
  if (typeof input === "string") return input.length > max ? input.slice(0, max) + "…" : input;
  const o = input as Record<string, unknown>;
  const pick = o.command ?? o.cmd ?? o.file_path ?? o.path ?? o.pattern ?? o.url ?? o.query ?? o.description ?? o.prompt;
  const s = typeof pick === "string" ? pick : Array.isArray(pick) ? pick.join(" ") : JSON.stringify(input);
  return s.length > max ? s.slice(0, max) + "…" : s;
}

export function textOf(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.map((c: any) => (typeof c === "string" ? c : c?.text ?? (c?.type === "image" ? "[image]" : ""))).join("");
  if (content && typeof content === "object" && "text" in (content as any)) return String((content as any).text);
  return "";
}
