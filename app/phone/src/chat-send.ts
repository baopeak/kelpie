// Sending a message to a chat from the phone. The phone only ever sends text: the daemon queues it with the
// desktop's messages (one queue per chat, written when the agent is idle) and builds the turn itself. This keeps
// track of each message from the tap to the transcript:
//
//   sending… → queued (n ahead) → sent (its own `cid` shows up in a `kelpie_sent` line) → the transcript item
//
// and of what can go wrong on the way: `unknown` (the connection dropped with the message in flight: not sure it
// was sent; Retry sends it again under the same `cid`, so the daemon never makes a second one), a refusal (the text
// goes back into the box with the reason), and `lost` (the computer restarted: its queue is gone). The text of a
// message is never dropped without the person being told.
import type { ChatCommand } from "../../src/chat/chat-model";
import type { QueueView } from "../../src/chat/chat-queue";
import type { SendOutcome } from "../../src/chat/transport";

/** The most text the daemon takes in one message (bytes: Vietnamese and emoji are more than a byte a character). */
export const MAX_SEND_BYTES = 16 * 1024;
/** A message the daemon wrote is shown as "sent" until its line reaches the log; if it never does it is let go after this. */
export const SENT_LINGER_MS = 5000;

export type PendingState = "sending" | "queued" | "sent" | "unknown" | "lost" | "cancelled";

export interface PendingSend {
  cid: string;
  text: string;
  state: PendingState;
  /** For `cancelled`: where it was taken out of the queue. */
  cancelledOn?: "desktop" | "phone";
  qid?: string;
  /** Messages in front of it in the queue. */
  ahead?: number;
  at: number;
  /** It needs a decision but this phone could not keep a copy (storage full, or over the cap): a reload would lose it. */
  unsaved?: boolean;
}

/** Why the phone does not even try. */
export type LocalRefusal = "empty" | "too_long" | "command" | "tui";

const bytes = (text: string) => new TextEncoder().encode(text).length;

/** Whether `text` is something the phone cannot send as a message: the agent's own client runs it. Codex runs every
 *  `/command` and `!shell` in the desktop; ACP's `/new`, `/clear` and `/reset` start another conversation there; a Claude
 *  command that works only in its terminal UI cannot be run in a chat. Other slash commands are sent as text. */
export function refuseLocally(kind: string, text: string, commands: readonly ChatCommand[]): LocalRefusal | null {
  const t = text.trim();
  if (!t) return "empty";
  if (bytes(text) > MAX_SEND_BYTES) return "too_long";
  if (kind === "codex") return t.startsWith("/") || t.startsWith("!") ? "command" : null;
  const slash = /^\/([\w:.-]+)(?:\s+([\s\S]*))?$/.exec(t);
  if (!slash) return null;
  if (kind === "claude") return commands.some((c) => c.name === slash[1] && c.tuiOnly) ? "tui" : null;
  return ["new", "clear", "reset"].includes(slash[1]) && !(slash[2] ?? "").trim() ? "command" : null;
}

export type SendResult =
  /** The daemon has it (or may have: `unknown`); the box can be cleared. */
  | { accepted: true; cid: string }
  /** Nothing was sent and the draft stays as it is. */
  | { accepted: false; reason: "offline" | "busy" | LocalRefusal }
  /** The daemon would not take it: the text goes back into the box with `message`. */
  | { accepted: false; reason: "refused"; code: string; message: string; text: string };

export interface SenderDeps {
  send(cid: string, text: string): Promise<SendOutcome>;
  /** Takes a waiting message out of the queue. */
  cancel(qid: string): Promise<"ok" | "too_late" | "failed">;
  connected(): boolean;
  now(): number;
  newCid(): string;
  /** Keeps what the person has to decide about (lost, unknown, cancelled) past a reload of the page; `[]`: nothing to keep.
   *  Returns the `cid`s it could not keep. */
  save?(kept: readonly PendingSend[]): readonly string[] | void;
}

/** The states the person has to act on: the message is not on its way any more, and only its text is left. */
const KEPT: readonly PendingState[] = ["unknown", "lost", "cancelled"];

/** What a message on its way shows: its line, and which of Retry, Put back and Discard it offers. `unsaved`: say that only
 *  this page has its text, so it should be copied now. */
export function chipView(p: PendingSend) {
  const kept = KEPT.includes(p.state);
  return { state: p.state, warn: kept, retry: p.state === "unknown", putBack: p.state === "lost" || p.state === "cancelled", discard: kept, unsaved: kept && !!p.unsaved };
}

/** A message as it is kept: without what only this page knows. */
const keptCopy = (p: PendingSend): PendingSend => {
  const copy = { ...p };
  delete copy.unsaved;
  return copy;
};

/** One chat's messages on their way. Listeners hear every change. A message is never changed in place: each change is a new
 *  object, so a view that keys its rows by object (Solid's `<For>`) draws the new state. */
export class ChatSender {
  private list: PendingSend[] = [];
  private listeners = new Set<() => void>();
  private latch = false;

  /** `kept`: what an earlier page kept (see `SenderDeps.save`), shown again as it was. */
  constructor(private deps: SenderDeps, private kind: string, private commands: () => readonly ChatCommand[], kept: readonly PendingSend[] = []) {
    this.list = kept.filter((p) => p && typeof p.cid === "string" && typeof p.text === "string" && KEPT.includes(p.state)).map(keptCopy);
  }

  get pending(): readonly PendingSend[] {
    return this.list;
  }

  /** A send is on its way: another tap does nothing. */
  get sending(): boolean {
    return this.latch;
  }

  subscribe(fn: () => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** The pairing is gone: a reply that comes in late keeps nothing and tells no one. */
  dispose() {
    this.deps = { ...this.deps, save: undefined };
    this.listeners.clear();
  }

  private changed() {
    const notKept = new Set(this.deps.save?.(this.list.filter((p) => KEPT.includes(p.state)).map(keptCopy)) ?? []);
    // Flags only the messages whose "kept or not" changed: the others stay the same objects.
    this.list = this.list.map((p) => {
      const unsaved = KEPT.includes(p.state) && notKept.has(p.cid);
      return !!p.unsaved === unsaved ? p : unsaved ? { ...p, unsaved } : keptCopy(p);
    });
    for (const fn of this.listeners) fn();
  }

  private find(cid: string) {
    return this.list.find((p) => p.cid === cid);
  }

  /** The message `cid` with `patch` applied, as a new object; the patched message (or undefined when it is gone). */
  private patch(cid: string, patch: Partial<PendingSend>): PendingSend | undefined {
    let out: PendingSend | undefined;
    this.list = this.list.map((p) => (p.cid === cid ? (out = { ...p, ...patch }) : p));
    return out;
  }

  private remove(cid: string) {
    this.list = this.list.filter((p) => p.cid !== cid);
  }

  async send(text: string): Promise<SendResult> {
    if (this.latch) return { accepted: false, reason: "busy" };
    const refused = refuseLocally(this.kind, text, this.commands());
    if (refused) return { accepted: false, reason: refused };
    if (!this.deps.connected()) return { accepted: false, reason: "offline" };
    const cid = this.deps.newCid();
    this.list.push({ cid, text, state: "sending", at: this.deps.now() });
    this.changed();
    return this.deliver(cid, text);
  }

  /** Sends a message whose fate is not known again, under the same `cid`: the daemon says what became of the first try. */
  async retry(cid: string): Promise<SendResult> {
    const p = this.find(cid);
    if (!p || (p.state !== "unknown" && p.state !== "lost")) return { accepted: false, reason: "busy" };
    if (this.latch) return { accepted: false, reason: "busy" };
    if (!this.deps.connected()) return { accepted: false, reason: "offline" };
    const before = p.state;
    this.patch(cid, { state: "sending" });
    this.changed();
    return this.deliver(cid, p.text, before);
  }

  /** `before`: for a retry, what the message was; if the connection is not there it goes back to that, text kept. */
  private async deliver(cid: string, text: string, before?: PendingState): Promise<SendResult> {
    this.latch = true;
    let outcome: SendOutcome;
    try {
      outcome = await this.deps.send(cid, text);
    } finally {
      this.latch = false;
    }
    const p = this.find(cid);
    // Gone meanwhile (the computer restarted, or the person let it go): nothing to say about it.
    if (!p) return { accepted: true, cid };
    const at = this.deps.now();
    switch (outcome.result) {
      case "ok":
        this.patch(cid, { state: outcome.state, qid: outcome.qid, ahead: outcome.ahead, at });
        break;
      case "dup":
        // The first try got there: this is what became of it.
        if (outcome.state === "cancelled" || outcome.state === "dropped") {
          this.remove(cid);
          this.changed();
          return { accepted: false, reason: "refused", code: outcome.state, message: "", text };
        }
        this.patch(cid, { state: outcome.state, qid: outcome.qid, at });
        break;
      case "refused":
        this.remove(cid);
        this.changed();
        return { accepted: false, reason: "refused", code: outcome.code, message: outcome.message, text };
      case "offline":
        // A retry with no connection sent nothing: the message is as it was.
        if (before) this.patch(cid, { state: before });
        else this.remove(cid);
        this.changed();
        return { accepted: false, reason: "offline" };
      case "unknown":
        // A restart seen while the send was out already made it lost; that is the better word.
        this.patch(cid, p.state === "sending" ? { state: "unknown", at } : { at });
        break;
    }
    this.changed();
    return { accepted: true, cid };
  }

  /** The daemon's queue as it stands (also the reply to following the chat again): messages of ours that it lists are queued,
   *  and ones it no longer lists were written (their line follows in the log) unless it says it dropped them. */
  snapshot(view: QueueView) {
    const at = (cid: string) => view.items.findIndex((i) => i.cid === cid);
    let touched = false;
    for (const p of [...this.list]) {
      const i = at(p.cid);
      if (i >= 0) {
        if (p.state !== "queued" || p.ahead !== i || p.qid !== view.items[i].qid) {
          this.patch(p.cid, { state: "queued", ahead: i, qid: view.items[i].qid });
          touched = true;
        }
      } else if (view.dropped.some((d) => d.cid === p.cid)) {
        const d = view.dropped.find((x) => x.cid === p.cid)!;
        // Taken out of the queue by someone else: it says where, and keeps the text.
        this.patch(p.cid, d.reason === "cancelled" ? { state: "cancelled", cancelledOn: d.cancelledBy ?? "phone" } : { state: "lost" });
        touched = true;
      } else if (p.state === "queued") {
        this.patch(p.cid, { state: "sent", at: this.deps.now() });
        touched = true;
      }
    }
    if (touched) this.changed();
  }

  /** The log shows the message `cid` was written: it is on the transcript now. */
  sentSeen(cid: string) {
    if (!this.find(cid)) return;
    this.remove(cid);
    this.changed();
  }

  /** The log shows the message `cid` left the queue unsent (`kelpie_unsent`): cancelled by the desktop or another phone, or
   *  dropped. Also from a replayed log, so a phone that was offline when it happened, and took the queue's silence about it for
   *  "sent", learns the truth and keeps the text. A message settled already (let go, or cancelled) is left as it is. */
  unsentSeen(cid: string, reason: string, cancelledBy?: "desktop" | "phone") {
    const p = this.find(cid);
    if (!p || p.state === "cancelled" || p.state === "lost") return;
    this.patch(cid, reason === "cancelled" ? { state: "cancelled", cancelledOn: cancelledBy ?? "phone" } : { state: "lost" });
    this.changed();
  }

  /** Lets go of "sent" messages whose line never came. */
  prune() {
    const now = this.deps.now();
    const keep = this.list.filter((p) => p.state !== "sent" || now - p.at < SENT_LINGER_MS);
    if (keep.length !== this.list.length) {
      this.list = keep;
      this.changed();
    }
  }

  /** The computer restarted: its queue, and every id in it, is gone. Nothing is sent again on its own (the message may or
   *  may not have gone out before); what was waiting stays on the screen as lost, with its text. */
  epochChanged() {
    this.list = this.list.filter((p) => p.state !== "sent").map((p) => ({ ...p, state: "lost" as const }));
    this.changed();
  }

  /** This phone took the message `qid` out of the queue itself: nothing to report about it. */
  forgetQueued(qid: string) {
    const gone = this.list.filter((p) => p.qid === qid && p.state === "queued");
    if (!gone.length) return;
    this.list = this.list.filter((p) => !gone.includes(p));
    this.changed();
  }

  /** Lets a message go (the person gave up on it). */
  discard(cid: string) {
    this.remove(cid);
    this.changed();
  }

  /** The text of a message back, for the box (Edit, or "put it back"), and the message is forgotten. A message that waits in
   *  the queue is taken out of it first; it is `null` when that was too late. */
  async take(cid: string): Promise<string | null> {
    const p = this.find(cid);
    if (!p) return null;
    if (p.state === "queued" && p.qid) {
      if ((await this.deps.cancel(p.qid)) !== "ok") return null;
    }
    this.remove(cid);
    this.changed();
    return p.text;
  }
}
