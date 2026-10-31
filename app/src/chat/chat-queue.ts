// The daemon's queue of messages for a chat, as the views show it. The daemon owns the queue (one per
// chat, fed by the desktop and the phones alike); the desktop and the phone each keep a mirror of it
// that follows `chat.queue` (desktop) or `chat_queue` (phone). Nothing here talks to the agent.

/** A message waiting in the daemon's queue. `from` names the device that queued it (not the desktop's own). */
export interface QueueItem {
  qid: string;
  cid: string;
  text: string;
  /** The phone that sent it; absent for a message from the desktop. */
  from?: string;
  /** This view's own message (this phone, or the desktop on the desktop). */
  mine: boolean;
  /** Waits at the head for the desktop window that queued it (a picture only that window can build). */
  needsWindow: boolean;
}

/** A message that left the queue without being sent: `reason` is `revoked`, `exited`, `closed`, `needs_window`, `declined`, `too_large`
 *  or `cancelled` (by someone other than its sender: `cancelledBy` says whether that was the desktop or a phone). */
export interface DroppedItem {
  qid: string;
  cid: string;
  text: string;
  /** The names of its pictures. */
  images?: string[];
  reason: string;
  mine: boolean;
  cancelledBy?: "desktop" | "phone";
  cancelledByName?: string;
}

export interface QueueView {
  items: QueueItem[];
  /** A stop is holding the queue until the next send or resume. */
  paused: boolean;
  /** The chat is being handed to a terminal. */
  held: boolean;
  /** A turn the daemon wrote (or saw written) is running. */
  inFlight: boolean;
  dropped: DroppedItem[];
}

/** What the daemon says of a queue entry. */
interface RawItem {
  qid?: string;
  cid?: string;
  text?: string;
  images?: unknown;
  kind?: string;
  reason?: string;
  by?: { id?: string; name?: string } | string;
  mine?: boolean;
  needs_window?: boolean;
  /** On the desktop `{id, name}`; on a phone `desktop` or `phone`, with the name in `cancelled_by_name`. */
  cancelled_by?: { id?: string; name?: string } | string;
  cancelled_by_name?: string;
}

interface RawQueue {
  items?: RawItem[];
  dropped?: RawItem[];
  paused?: boolean;
  held?: boolean;
  in_flight?: boolean;
}

const DESKTOP = "desktop";

const names = (images: unknown): string[] => (Array.isArray(images) ? images.map(String) : []);

function name(by: RawItem["by"]): string {
  return typeof by === "string" ? by : by?.name ?? "";
}

function canceller(e: RawItem): Pick<DroppedItem, "cancelledBy" | "cancelledByName"> {
  const c = e.cancelled_by;
  if (!c) return {};
  if (typeof c === "string") return { cancelledBy: c === DESKTOP ? "desktop" : "phone", cancelledByName: e.cancelled_by_name };
  return { cancelledBy: c.id === DESKTOP ? "desktop" : "phone", cancelledByName: c.name };
}

/** `chat.queue` as the desktop gets it: `by` is `{id, name}` and the desktop's own messages have the id `desktop`. */
export function fromDaemonEvent(p: RawQueue): QueueView {
  const item = (e: RawItem) => {
    const mine = typeof e.by === "object" && e.by?.id === DESKTOP;
    return { qid: String(e.qid), cid: String(e.cid), text: String(e.text ?? ""), from: mine ? undefined : name(e.by) || undefined, mine, needsWindow: !!e.needs_window };
  };
  return {
    items: (p.items ?? []).map(item),
    paused: !!p.paused,
    held: !!p.held,
    inFlight: !!p.in_flight,
    dropped: (p.dropped ?? []).map((e) => {
      const i = item(e);
      return { qid: i.qid, cid: i.cid, text: i.text, images: names(e.images), reason: String(e.reason ?? ""), mine: i.mine, ...canceller(e) };
    }),
  };
}

/** `chat_queue` as a phone gets it: `by` is the sender's name and `mine` says it is this phone's. A message from the
 *  desktop has the kind `desktop`. */
export function fromPhoneMessage(m: RawQueue): QueueView {
  const item = (e: RawItem) => ({ qid: String(e.qid), cid: String(e.cid), text: String(e.text ?? ""), from: e.kind === "desktop" ? undefined : name(e.by) || undefined, mine: !!e.mine, needsWindow: !!e.needs_window });
  return {
    items: (m.items ?? []).map(item),
    paused: !!m.paused,
    held: !!m.held,
    inFlight: !!m.in_flight,
    dropped: (m.dropped ?? []).map((e) => ({ qid: String(e.qid), cid: String(e.cid), text: String(e.text ?? ""), images: names(e.images), reason: String(e.reason ?? ""), mine: !!e.mine, ...canceller(e) })),
  };
}

/** The part of the reply to `chat.send` the controller acts on. */
export type EnqueueResult = { qid: string; state: "queued" | "sent" | "cancelled" | "dropped"; dup?: boolean; ahead?: number } | { refused: string; message: string };
