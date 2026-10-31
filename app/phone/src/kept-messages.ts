// Copies of the chat messages this phone sent that the person still has to decide about (lost in a restart, in doubt,
// cancelled elsewhere): kept in the browser so a reload does not lose their text. Bounded per chat and in all. A chat the
// computer no longer lists (closed, or resumed as a new session) keeps its copies for a week, shown under "Not sent" in the
// inbox; only then are they dropped. Under `kelpie-draft:`, so forgetting or losing the pairing wipes them with the drafts.
import type { PendingSend } from "./chat-send";
import { store } from "./storage";

export const KEPT_PREFIX = "kelpie-draft:chat-pending:";
/** At most this many kept per chat, and on this phone in all: the newest are kept, the rest are flagged as not kept. */
export const KEPT_PER_CHAT = 20;
export const KEPT_TOTAL = 60;
/** How long the copies of a chat the computer no longer lists are kept after they last changed. */
export const GONE_KEEP_MS = 7 * 24 * 60 * 60 * 1000;

/** What is stored per chat. (A plain array is what the first build stored: read as items with no name and no time.) */
interface Stored {
  v: 2;
  /** The chat's name when it was last saved, to say where a message was for once the chat is gone. */
  name?: string;
  /** When it last changed (ms). */
  savedAt: number;
  items: PendingSend[];
}

/** The kept messages of a chat that the computer no longer lists. */
export interface GoneChat {
  chat: string;
  name?: string;
  items: PendingSend[];
}

const keyOf = (chat: string) => `${KEPT_PREFIX}${chat}`;

function read(key: string): Stored | null {
  const v = store.json<unknown>(key, null);
  if (Array.isArray(v)) return { v: 2, savedAt: Math.max(0, ...v.map((p) => Number((p as PendingSend)?.at) || 0)), items: v as PendingSend[] };
  if (v && typeof v === "object" && Array.isArray((v as Stored).items)) return v as Stored;
  return null;
}

/** What was kept for `chat`. */
export function loadKept(chat: string): PendingSend[] {
  return read(keyOf(chat))?.items ?? [];
}

/** Keeps `kept` (oldest first) for `chat`, as much as the caps and the browser allow; returns the `cid`s not kept. `name`: the
 *  chat's name now (the stored one is kept when it is not known any more). */
export function saveKept(chat: string, kept: readonly PendingSend[], name?: string, now = Date.now()): string[] {
  const key = keyOf(chat);
  if (!kept.length) {
    store.del(key);
    return [];
  }
  const others = store.keys(KEPT_PREFIX).filter((k) => k !== key).reduce((n, k) => n + (read(k)?.items.length ?? 0), 0);
  const room = Math.max(0, Math.min(KEPT_PER_CHAT, KEPT_TOTAL - others));
  const keep = kept.slice(Math.max(0, kept.length - room));
  const stored: Stored = { v: 2, name: name ?? read(key)?.name, savedAt: now, items: keep };
  // A copy that cannot be written is not left behind either: it could bring back a message the person since let go.
  if (!keep.length || !store.keep(key, JSON.stringify(stored))) {
    store.del(key);
    return kept.map((p) => p.cid);
  }
  return kept.slice(0, kept.length - keep.length).map((p) => p.cid);
}

/** The chats with kept messages that are not in `chats` (the computer's whole list, as it just said it: an empty one too, since
 *  this only lists and deletes nothing). */
export function goneChats(chats: readonly string[]): GoneChat[] {
  const known = new Set(chats.map(keyOf));
  const out: GoneChat[] = [];
  for (const k of store.keys(KEPT_PREFIX)) {
    if (known.has(k)) continue;
    const s = read(k);
    // Storage is the browser's: skip entries that are not well-formed rather than render them.
    const items = (s?.items ?? []).filter((p) => p && typeof p.cid === "string" && typeof p.text === "string");
    if (items.length) out.push({ chat: k.slice(KEPT_PREFIX.length), name: s?.name, items });
  }
  return out;
}

/** Drops the copies of chats not in `chats` once they have not changed for `GONE_KEEP_MS`. An empty list says nothing. A chat
 *  that is gone because it was resumed as a new session still has its messages shown, under "Not sent", until then. */
export function forgetGoneChats(chats: readonly string[], now = Date.now()) {
  if (!chats.length) return;
  const known = new Set(chats.map(keyOf));
  for (const k of store.keys(KEPT_PREFIX)) {
    if (known.has(k)) continue;
    const s = read(k);
    if (!s?.items.length || now - s.savedAt > GONE_KEEP_MS) store.del(k);
  }
}

/** Lets one kept message of `chat` go (the person discarded it from the "Not sent" list). The others keep their week. */
export function discardKept(chat: string, cid: string) {
  const s = read(keyOf(chat));
  if (!s) return;
  const items = s.items.filter((p) => p.cid !== cid);
  if (!items.length) store.del(keyOf(chat));
  else store.keep(keyOf(chat), JSON.stringify({ ...s, v: 2, items }));
}
