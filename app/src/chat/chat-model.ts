// Chat-mode state shared by all agent adapters. Adapters translate their wire protocol
// (Claude stream-json, Codex app-server, ACP) into these items; the UI only knows items.
import { createStore, produce, type SetStoreFunction } from "solid-js/store";
import { t } from "../lib/i18n";
import type { AnswerOutcome, ChatTransport } from "./transport";

/** What a tool call does, for its row and for folding runs of reads and searches. */
export type ToolCategory = "read" | "search" | "edit" | "command" | "fetch" | "agent" | "plan" | "other";

/** A question the agent asks with choices (Claude's AskUserQuestion). */
export interface AskQuestion {
  question: string;
  header?: string;
  options: { label: string; description?: string }[];
  multiSelect?: boolean;
  /** What the agent calls this answer, when that is not the question (ACP's form fields). */
  key?: string;
}

/** What a resolved card says was answered. `answers` by question; `by` names another device. */
export interface CardAnswer {
  label: string;
  answers?: Record<string, string>;
  message?: string;
  by?: string;
  at?: number;
}

export interface PlanEntry {
  text: string;
  status: "pending" | "active" | "done";
}

/** `at` / `doneAt`: when the item appeared / finished, live only (a replayed log has no times). */
export type ChatItem =
  | { kind: "user"; id: string; text: string; images?: UserImage[]; pending?: boolean; at?: number }
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
      /** What `resolved` stands for, when the answer is known. */
      answered?: CardAnswer;
      /** Observer mode: the answer is on its way, not yet settled by the daemon. */
      answering?: boolean;
      /** Observer mode: the answer may not have gone through (`unknown`) or was refused; the person can answer again. */
      answerError?: string;
      /** Observer mode: an answer was sent and its fate is not known; this is what it said. Asking again sends that same answer. */
      retry?: string;
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

/** A picture sent with a message: the agent takes `data` (base64) or reads `path`. */
export interface ChatImage {
  mime: string;
  data: string;
  path: string;
}

/** A picture of a message in the transcript. One sent from here keeps its `path` and is read when
 *  shown; one replayed from the log has the `data` the log kept, or is only a name. A path that did
 *  not come from here is the agent's word for it: see `chat-images.ts` before reading one. */
export interface UserImage {
  name: string;
  mime?: string;
  path?: string;
  data?: string;
}

const fileName = (path: string) => path.split(/[\\/]/).pop() || path;

/** A message's text where only text will do (a queue chip, a note of what was not sent), the names of its pictures under it. */
export function withImageNames(text: string, images?: ChatImage[]): string {
  if (!images?.length) return text;
  const names = images.map((i) => `[${fileName(i.path)}]`).join(" ");
  return text ? `${text}\n${names}` : names;
}

/** The pictures of a message sent from here, as the transcript keeps them: by path, without their bytes. */
export function sentImages(images?: ChatImage[]): UserImage[] | undefined {
  return images?.length ? images.map((i) => ({ name: fileName(i.path), mime: i.mime, path: i.path })) : undefined;
}

/** A picture the agent's log names by path. */
export const pathImage = (path: string): UserImage => ({ name: fileName(path), path });

/** A picture the log has no name for, the `n`th of its message: with its bytes if it kept them. */
export function loggedImage(n: number, mime?: unknown, data?: unknown): UserImage {
  const name = t("Picture {n}", { n });
  return typeof mime === "string" && typeof data === "string" && data ? { name, mime, data } : { name };
}

/** A message sent while the agent was busy, sent when its turn ends. */
export interface QueuedMessage {
  id: string;
  text: string;
  typed: boolean;
  images?: ChatImage[];
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
  send(text: string, typed?: boolean, images?: ChatImage[]): void;
  /** Delivers `text` into the running turn; false when the agent cannot take it mid-turn. */
  steer?(text: string): boolean;
  /** Text that starts no turn (a picker command, a terminal-only one): never queued. */
  immediate?(text: string, typed: boolean): boolean;
  interrupt(): void;
  respond(permissionId: string, optionId: string, reply?: PermissionReply): void;
  /** A logged answer (the daemon's `kelpie_answer` record, in this dialect) in the card's own words, or
   *  null when it is not one this card knows. Only reads: an answer replayed from the log is long gone. */
  describeAnswer?(card: Extract<ChatItem, { kind: "permission" }>, answer: any): Pick<CardAnswer, "label" | "answers" | "message"> | null;
  /** Reset internal bookkeeping before a full log replay. */
  reset(): void;
  setSetting?(id: string, value: string): void;
  /** What the conversation ran with before this front-end (model, effort), shown until the
   *  agent reports its own. */
  seed?(settings: { model?: string | null; effort?: string | null }): void;
  /** How a picked file goes into the message (default `@path`). */
  mention?(path: string): string;
  /** False when the agent is told where a picture is instead of being given it (default true). */
  takesPictures?(): boolean;
  /** Text that runs as a command rather than going to the agent as a message: no pictures go with it. */
  isCommand?(text: string, typed: boolean): boolean;
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
  /** The turn ended: cards still open can no longer be answered. Going idle says so too; this is for a turn
   *  `busy` knows nothing of (a replay has no ACP prompt, which is what sets it there). */
  expireApprovals?(): void;
  cwd: string;
  resumeId: string | null;
  replaying: () => boolean;
  /** A watcher shows no pictures: the bytes a log has for one are not kept, only its name. */
  namesOnly?: boolean;
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
  private attachGen = 0;
  private isReplaying = false;
  /** Settled approval ids, for an older daemon's log (which does not contain our answers) and for what was
   *  settled here. `fromLog`: the log says it again on a replay, so a request under that id arriving now is that
   *  replay or a new request that reuses the id, and asks until its own answer comes. `own`: the person's answer here. */
  private resolved = new Map<string, { label: string; answered?: CardAnswer; fromLog?: boolean; own?: boolean }>();
  private started = false;
  private flushing = false;
  private queued = 0;
  /** When the log or transcript being replayed says its next line came (unix ms). */
  private clock: number | null = null;
  private loadingHistory = false;
  /** Renders the agent's own transcript (history from before chat mode) ahead of the log. */
  loadHistory: ((ctx: AdapterContext) => Promise<void>) | null = null;
  readonly transport: ChatTransport;
  /** A watcher (the phone): it never starts or configures the agent, never answers on its own, never
   *  sends a queued message and never changes the session; it only shows the log and passes on the
   *  person's answers to the agent's questions. */
  readonly observer: boolean;
  /** Set while the person's own answer is being written (`respond`): the only write an observer lets
   *  through. `false` outside it; a promise once the answer was passed on. */
  private answering: Promise<AnswerOutcome> | null | false = false;
  /** What the person's answer wrote, noted by `write` while `answering` is `null`. */
  private written: { requestId: string | number; line: string } | null = null;
  /** The first answer given to each card whose fate is still unknown (it was lost, or refused): asking again
   *  sends exactly that line. The adapters forget a request once they answered it, and a second answer
   *  built from nothing would be wrong (Claude: an empty `updatedInput`); the same line is what the daemon
   *  recognises as this device's own answer, so a repeat is harmless. */
  private unsettled = new Map<string, { requestId: string | number; line: string; label: string; identity: string }>();
  /** Cards whose answer is on its way. A log loaded again rebuilds the card, which must stay locked until the
   *  reply says what became of the answer that was sent: a second tap would send another and the card would
   *  be labelled with the one that lost. */
  private inFlight = new Set<string>();
  /** Counts the times ids and positions were forgotten: a reply that was waited for across one belongs to nothing now. */
  private generation = 0;

  constructor(readonly sessionId: string, readonly cwd: string, readonly resumeId: string | null, makeAdapter: (ctx: AdapterContext) => Adapter, opts: { transport?: ChatTransport; observer?: boolean } = {}) {
    this.transport = opts.transport ?? {};
    this.observer = opts.observer ?? false;
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
      write: (obj) => this.write(obj),
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
        if (!this.observer) this.transport.update?.({ agent_session_id: id });
      },
      setModel: (model) => {
        if (this.state.model === model) return;
        this.set("model", model);
        if (model && !this.isReplaying && !this.observer) this.transport.update?.({ model });
      },
      setClock: (at) => (this.clock = at),
      expireApprovals: () => this.expireApprovals(),
      cwd: this.cwd,
      resumeId: this.resumeId,
      replaying: () => this.isReplaying,
      namesOnly: this.observer,
    };
  }

  /** A line from an adapter to the agent. An observer lets one kind through: the answer the person
   *  just gave (see `respond`), passed on as the answer to that request. Everything else an adapter
   *  writes by itself (handshakes, replies to the agent's own requests, interrupts) is dropped. */
  private write(obj: unknown) {
    if (this.isReplaying) return;
    if (!this.observer) {
      this.transport.send?.(JSON.stringify(obj) + "\n");
      return;
    }
    if (this.answering !== null || !this.transport.answer) return;
    const id = answeredRequest(obj);
    if (id == null) return;
    const line = JSON.stringify(obj);
    this.written = { requestId: id, line };
    this.answering = this.transport.answer(id, line);
  }

  /** Now, or while replaying (log, transcript) the time the log carried. */
  private now() {
    return this.isReplaying || this.loadingHistory ? this.clock : Date.now();
  }

  upsert(item: ChatItem) {
    // The daemon's reader and its exit watcher race: the agent's last lines can come after its exit.
    if (this.state.exited && "streaming" in item && item.streaming) item = { ...item, streaming: false };
    if (item.kind === "permission") {
      const settled = this.resolved.get(item.id);
      if (settled?.fromLog) this.resolved.delete(item.id);
      else if (settled) item = { ...item, resolved: settled.label, answered: settled.answered };
      // A request among the agent's last lines, which can come after its exit: nothing can take an answer.
      if (this.state.exited && !item.resolved) {
        const label = t("No longer needed");
        this.resolved.set(item.id, { label, fromLog: this.isReplaying });
        item = { ...item, resolved: label };
      }
    }
    if (item.kind === "permission" && !item.resolved) {
      const saved = this.unsettled.get(item.id);
      const same = !!saved && saved.identity === cardIdentity(item);
      if (saved && !same) this.unsettled.delete(item.id);
      if (this.observer) {
        // A store merge keeps what it is not told to drop: say it all. The error about an answer belongs to
        // the request it was about; a new request under a reused id starts clean.
        const at = this.index.get(item.id);
        const before = at == null ? undefined : this.state.items[at];
        const error = same && before?.kind === "permission" ? before.answerError : undefined;
        item = { ...item, retry: same ? saved!.label : undefined, answerError: error, answering: this.inFlight.has(item.id) ? true : undefined };
      }
    }
    const i = this.index.get(item.id);
    if (i != null) {
      const prev = this.state.items[i];
      item = { ...item, at: prev.at ?? item.at } as ChatItem;
      if (item.kind === "thinking" && prev.kind === "thinking") item.doneAt = prev.doneAt;
      // A store merge keeps what it is not told to drop: a request under a reused id is not answered yet.
      if (item.kind === "permission") item = { ...item, resolved: item.resolved, answered: item.answered };
      this.set("items", i, finished(item, this.now()));
      return;
    }
    const at = this.now();
    if (item.at == null && at != null) item = { ...item, at } as ChatItem;
    this.index.set(item.id, this.state.items.length);
    this.set("items", this.state.items.length, item);
  }

  patch(id: string, fn: (item: ChatItem) => void) {
    const i = this.index.get(id);
    if (i == null) return;
    this.set("items", i, produce((it: ChatItem) => {
      fn(it);
      finished(it, this.now());
    }));
  }

  private resetState() {
    this.index.clear();
    this.buffer = "";
    this.set({ items: [], busy: false, ready: false, status: null, plan: null, usage: null, tasks: [] });
    this.adapter.reset();
  }

  private feed(bytes: Uint8Array) {
    this.feedText(this.decoder.decode(bytes, { stream: true }));
  }

  private feedText(text: string) {
    this.buffer += text;
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
      if (msg?.type === "kelpie_answer") {
        this.applyLoggedAnswer(msg);
        continue;
      }
      if (msg?.type === "kelpie_exit") {
        this.set({ exited: true, busy: false, tasks: [] });
        // Also when `busy` did not know of the turn (an ACP replay): nothing can answer these now.
        this.expireApprovals();
        // Nothing will finish what was still arriving.
        for (const it of this.state.items) if ((it as { streaming?: boolean }).streaming) this.patch(it.id, (i: any) => (i.streaming = false));
        this.upsert({ kind: "system", id: `exit-${Date.now()}`, text: t("Agent process exited (code {code}).", { code: String(msg.code) }), tone: "info" });
        // Nothing can take them now: show what was not sent rather than keep dead chips.
        if (this.state.queue.length) {
          this.upsert({ kind: "system", id: `unsent-${Date.now()}`, text: t("Not sent:\n{messages}", { messages: this.state.queue.map((q) => withImageNames(q.text, q.images)).join("\n") }), tone: "info" });
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
    if (this.observer || !this.transport.attach) return;
    const gen = ++this.attachGen;
    const early: Uint8Array[] = [];
    let applied = false;
    const snapshot = await this.transport.attach(handle, (data) => {
      if (gen !== this.attachGen) return;
      if (applied) this.feed(data);
      else early.push(data);
    }).catch((e) => {
      this.upsert({ kind: "system", id: "attach-error", text: t("Could not attach: {error}", { error: String(e) }), tone: "error" });
      return null;
    });
    if (!snapshot || gen !== this.attachGen) return;
    this.resetState();
    this.clock = null;
    this.loadingHistory = true;
    if (this.loadHistory) await this.loadHistory(this.context()).catch(() => {});
    this.loadingHistory = false;
    if (gen !== this.attachGen) return;
    this.clock = null;
    this.isReplaying = true;
    this.feed(snapshot);
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
    this.transport.detach?.();
  }

  /** Observer: the daemon's chat message `{mode, lines}`. `reset` replaces what is shown with the log;
   *  `delta` and `more` carry on a log (history catching up: applied without a reset and acted on
   *  by nobody); `live` is output as it is printed. Nothing here talks to the agent. */
  applyLog(mode: "reset" | "delta" | "more" | "live", lines: string[]) {
    if (!this.observer) return;
    const text = lines.length ? lines.join("\n") + "\n" : "";
    if (mode === "live") return this.feedText(text);
    if (mode === "reset") {
      this.resetState();
      this.clock = null;
      // The log was loaded again: what is asked now is rebuilt from it, and an answer saved for the old
      // picture must not stand in for one given to the new.
      this.unsettled.clear();
    }
    const was = this.isReplaying;
    this.isReplaying = true;
    try {
      this.feedText(text);
    } finally {
      this.isReplaying = was;
    }
    this.clock = null;
    if (!this.state.ready) this.set("ready", true);
  }

  /** Sends now, or queues while the agent works (or while earlier queued messages wait). */
  send(text: string, typed = false, images?: ChatImage[]) {
    if (this.observer) return;
    if ((!text.trim() && !images?.length) || this.state.exited) return;
    // White space beside a picture is no text: it would show as an empty bubble.
    if (!text.trim()) text = "";
    // Not attached yet: a re-attach replays the log and would wipe what is sent now.
    if ((this.state.busy || this.state.queue.length || !this.state.ready) && !this.adapter.immediate?.(text, typed)) {
      this.set("queue", (q) => [...q, { id: `q-${++this.queued}`, text, typed, images }]);
      this.set("queuePaused", false);
      return;
    }
    this.adapter.send(text, typed, images);
  }

  /** Sends the next queued message once the agent is idle — a microtask later, as this runs
   *  inside the adapter's own state change, which must finish first. */
  private flushQueue() {
    if (this.observer || this.flushing || !this.state.queue.length) return;
    this.flushing = true;
    queueMicrotask(() => {
      this.flushing = false;
      const s = this.state;
      if (s.busy || !s.ready || s.exited || s.queuePaused || this.isReplaying || !s.queue.length) return;
      const [next, ...rest] = s.queue;
      this.set("queue", rest);
      this.adapter.send(next.text, next.typed, next.images);
    });
  }

  /** A turn that ended took its open approvals with it (dropped by the agent, or answered before a replay
   *  of an older daemon's log, which has no answers): they must not look, or be answered, as waiting. */
  private expireApprovals() {
    for (const it of this.state.items) {
      if (it.kind === "permission" && !it.resolved) {
        this.resolved.set(it.id, { label: t("No longer needed"), fromLog: this.isReplaying });
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
      this.adapter.send(m.text, m.typed, m.images);
    } else if (!command && !m.images?.length && this.adapter.steer?.(m.text)) {
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

  takesPictures(): boolean {
    return this.adapter.takesPictures?.() ?? true;
  }

  /** Whether pictures go with this text; a command takes none. */
  takesPicturesWith(text: string, typed = true): boolean {
    return !this.adapter.isCommand?.(text, typed);
  }

  /** The agent's request `requestId` was answered by someone else (a phone, the first to answer wins):
   *  its card stops asking. Each adapter names a request's card after the agent's id its own way.
   *  `lost`: the daemon dropped an answer given here because another one had already counted, so a card
   *  that shows what was clicked here shows what happened instead. */
  resolvedElsewhere(requestId: string | number, lost = false, who?: { answeredBy?: string; at?: number }) {
    const key = this.findCard(requestId);
    const item = key == null ? undefined : this.state.items[this.index.get(key)!];
    if (key == null || item?.kind !== "permission" || item.answering) return;
    const label = who ? answeredBeforeText(who.answeredBy, who.at) : t("Answered on another device");
    if (item.resolved && !(lost && item.resolved !== label)) return;
    // The log already told who it was, and what they said.
    if (item.answered?.by) return;
    this.resolved.set(key, { label });
    this.patch(key, (it) => {
      if (it.kind === "permission") {
        it.resolved = label;
        it.answered = undefined;
      }
    });
  }

  /** The card of the agent's request `requestId`: each adapter names it after the agent's id its own
   *  way. If a restart reused the id for another kind of request, the one still open. */
  private findCard(requestId: string | number): string | undefined {
    const id = String(requestId);
    const card = (key: string) => {
      const item = this.state.items[this.index.get(key) ?? -1];
      return item?.kind === "permission" ? item : undefined;
    };
    const keys = [id, `approval-${id}`, `perm-${id}`, `elicit-${id}`].filter((key) => card(key));
    return keys.find((key) => !card(key)!.resolved) ?? keys[0];
  }

  /** The daemon's record of an answer that counted (`kelpie_answer`, in the log, so also on a replay): the
   *  card shows what was chosen. Nothing here goes near the adapter's `respond` or the agent. */
  private applyLoggedAnswer(msg: any) {
    const key = this.findCard(msg.request_id);
    const card = key == null ? undefined : this.state.items[this.index.get(key)!];
    if (key == null || card?.kind !== "permission") return;
    // On its way, and the reply will settle it: a phone's own answer must not read "Already answered on Pixel".
    if (card.answering || this.inFlight.has(key)) return;
    const settled = this.resolved.get(key);
    if (this.observer && settled?.own) return;
    const by: string | undefined = msg.answered_by;
    const at: number | undefined = msg.at;
    // Who "here" is: a window answers as the desktop, a phone under its own name; anyone else is another device.
    const fromHere = this.observer ? !!by && by === this.transport.deviceName?.() : !by || by === "desktop";
    // What is left of an answer whose reply was lost: its fate is known now, whoever's answer this is.
    const saved = this.unsettled.get(key);
    this.unsettled.delete(key);
    let entry: { label: string; answered: CardAnswer; fromLog: boolean };
    if (fromHere && settled?.own && settled.answered) {
      // What was clicked here, confirmed.
      entry = { label: settled.label, answered: { ...settled.answered, at }, fromLog: true };
    } else {
      const said = this.adapter.describeAnswer?.(card, msg.answer ?? {}) ?? { label: t("Answered") };
      const given = saved?.identity === cardIdentity(card) ? saved.label : said.label;
      entry = fromHere ? { label: given, answered: { ...said, label: given, at }, fromLog: true } : { label: answeredBeforeText(by, at), answered: { ...said, by: by ?? "desktop", at }, fromLog: true };
    }
    this.resolved.set(key, entry);
    this.patch(key, (it) => {
      if (it.kind === "permission") {
        it.resolved = entry.label;
        it.answered = entry.answered;
        it.answerError = undefined;
        it.retry = undefined;
      }
    });
  }

  /** An observer's answer goes to the daemon as the answer to that request; the daemon lets the first
   *  answer win, so the card settles on what it says: ours, somebody else's, or "not sure". */
  private async respondObserved(permissionId: string, optionId: string, label: string, reply?: PermissionReply) {
    this.answering = null;
    this.written = null;
    let outcome: Promise<AnswerOutcome> | null | false;
    try {
      this.adapter.respond(permissionId, optionId, reply);
    } finally {
      outcome = this.answering as Promise<AnswerOutcome> | null | false; // set by write() while the adapter ran
      this.answering = false;
    }
    const card = this.state.items[this.index.get(permissionId) ?? -1];
    if (this.written && card?.kind === "permission") {
      this.unsettled.set(permissionId, { ...(this.written as { requestId: string | number; line: string }), label, identity: cardIdentity(card) });
    }
    this.written = null;
    await this.settleAnswer(permissionId, label, outcome);
  }

  /** Observer: sends the answer already given to this card once more, byte for byte (see `unsettled`). Only
   *  this, never an ordinary answer button, may stand in for the saved line. */
  retryAnswer(permissionId: string) {
    const saved = this.unsettled.get(permissionId);
    const card = this.state.items[this.index.get(permissionId) ?? -1];
    if (!saved || card?.kind !== "permission" || card.answering || card.resolved || this.resolved.has(permissionId) || this.inFlight.has(permissionId)) return;
    // The card is no longer the request that was answered (the log was loaded again, a restart reused the id).
    if (saved.identity !== cardIdentity(card) || !this.transport.answer) {
      this.unsettled.delete(permissionId);
      this.patch(permissionId, (i) => {
        if (i.kind === "permission") i.retry = undefined;
      });
      return;
    }
    void this.settleAnswer(permissionId, saved.label, this.transport.answer(saved.requestId, saved.line));
  }

  /** The restart of a daemon, or anything else that makes ids and positions mean nothing: answers saved
   *  for retries and the memory of answered cards are dropped, so a new request that reuses an id is
   *  answered as itself. */
  forgetSentAnswers() {
    this.unsettled.clear();
    this.resolved.clear();
    this.inFlight.clear();
    this.generation++;
  }

  private async settleAnswer(permissionId: string, label: string, outcome: Promise<AnswerOutcome> | null | false) {
    const generation = this.generation;
    if (!outcome) {
      // Nothing was written: this card is not an answer to a request the daemon tracks (a sign-in card, say).
      this.patch(permissionId, (i) => {
        if (i.kind === "permission") i.answerError = t("This can only be answered on the computer.");
      });
      return;
    }
    this.inFlight.add(permissionId);
    this.patch(permissionId, (i) => {
      if (i.kind === "permission") {
        i.answering = true;
        i.answerError = undefined;
      }
    });
    const done = await outcome.catch((): AnswerOutcome => ({ result: "unknown" }));
    // Ids and positions were forgotten meanwhile (the daemon restarted): the reply is about a request that is gone.
    if (generation !== this.generation) return;
    this.inFlight.delete(permissionId);
    const settle = (text: string | null, error?: string) => {
      if (text != null) {
        this.resolved.set(permissionId, { label: text, own: done.result === "ok" || done.result === "dup" });
        this.unsettled.delete(permissionId);
      }
      this.patch(permissionId, (i) => {
        if (i.kind !== "permission") return;
        i.answering = false;
        i.answerError = error;
        i.retry = text == null ? this.unsettled.get(permissionId)?.label : undefined;
        if (text != null) i.resolved = text;
      });
    };
    if (done.result === "ok" || done.result === "dup") settle(label);
    else if (done.result === "stale") settle(answeredBeforeText(done.answeredBy, done.at));
    else settle(null, done.result === "refused" ? done.message : t("Not sure this was sent. Check the connection and answer again."));
  }

  respond(permissionId: string, optionId: string, label: string, reply?: PermissionReply) {
    if (this.resolved.has(permissionId)) return;
    if (this.observer) {
      const card = this.state.items[this.index.get(permissionId) ?? -1];
      // One answer in flight at a time, and none by a button while an earlier answer is waiting to be sent
      // again: that goes through retryAnswer, with the same line.
      if (!(card?.kind === "permission" && card.answering) && !this.inFlight.has(permissionId) && !this.unsettled.has(permissionId)) void this.respondObserved(permissionId, optionId, label, reply);
      return;
    }
    const answered: CardAnswer = { label, answers: reply?.answers, message: reply?.message };
    this.resolved.set(permissionId, { label, answered, own: true });
    this.adapter.respond(permissionId, optionId, reply);
    this.patch(permissionId, (i) => {
      if (i.kind === "permission") {
        i.resolved = label;
        i.answered = answered;
      }
    });
  }
}

/** What a permission card asks, as one string: two cards with the same id that ask different things are different requests. */
function cardIdentity(item: Extract<ChatItem, { kind: "permission" }>): string {
  return JSON.stringify([item.title, item.detail, item.command, item.plan, item.ask, item.options.map((o) => o.id)]);
}

/** The request an agent's answer message answers: Claude's `control_response` names it in
 *  `response.request_id`, the JSON-RPC dialects (Codex, ACP) in `id`. */
export function answeredRequest(obj: unknown): string | number | null {
  const o = obj as { type?: unknown; response?: { request_id?: unknown }; id?: unknown; method?: unknown } | null;
  if (!o || typeof o !== "object" || "method" in o) return null;
  const id = o.type === "control_response" ? o.response?.request_id : o.id;
  return typeof id === "string" || typeof id === "number" ? id : null;
}

/** "the desktop", or a device's name, for "Already answered on …". */
function whoAnswered(by: string | undefined): string {
  return !by || by === "desktop" ? t("the desktop") : by;
}

/** What a card says when somebody else's answer got there first. */
export function answeredBeforeText(by: string | undefined, at: number | undefined, now = Date.now()): string {
  const who = whoAnswered(by);
  if (at == null) return t("Already answered on {who}", { who });
  const s = Math.max(0, Math.round((now - at) / 1000));
  const ago = s < 60 ? t("just now") : s < 3600 ? t("{n} min ago", { n: Math.floor(s / 60) }) : t("{n} h ago", { n: Math.floor(s / 3600) });
  return t("Already answered on {who} {ago}", { who, ago });
}

/** A logged `{question: answer}` map as what a card shows, or null when there is nothing in it. */
export function logQuestionAnswers(raw: unknown): { label: string; answers: Record<string, string> } | null {
  if (!raw || typeof raw !== "object") return null;
  const answers = Object.fromEntries(Object.entries(raw).filter((e): e is [string, string] => typeof e[1] === "string"));
  const said = Object.values(answers);
  return said.length ? { label: said.join(" · "), answers } : null;
}

/** Which of a question's options an answer chose, and what is left over (what was typed as "Other"). The
 *  card joins them with ", " and a label can contain that too: the longest label that fits comes first. */
export function matchAnswer(answer: string, options: { label: string }[]): { chosen: Set<string>; other: string } {
  const labels = new Set(options.map((o) => o.label));
  const longest = Math.max(1, ...options.map((o) => o.label.split(", ").length));
  const parts = answer.split(", ");
  const chosen = new Set<string>();
  const rest: string[] = [];
  for (let i = 0; i < parts.length; ) {
    let end = Math.min(parts.length, i + longest);
    while (end > i && !labels.has(parts.slice(i, end).join(", "))) end--;
    if (end > i) {
      chosen.add(parts.slice(i, end).join(", "));
      i = end;
    } else rest.push(parts[i++]);
  }
  return { chosen, other: rest.join(", ") };
}

/** A thinking item that just stopped streaming gets its end time (for "Thought for 4s"). */
function finished(item: ChatItem, now: number | null): ChatItem {
  if (item.kind === "thinking" && !item.streaming && item.at != null && item.doneAt == null && now != null) item.doneAt = now;
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
