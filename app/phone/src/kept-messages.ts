// Copies of the chat messages this phone sent that the person still has to decide about (lost in a restart, in doubt,
// cancelled elsewhere): kept in the browser so a reload does not lose their text. Bounded per chat and in all, and dropped
// for chats the computer no longer has. Under `kelpie-draft:`, so forgetting or losing the pairing wipes them with the drafts.
import type { PendingSend } from "./chat-send";
import { store } from "./storage";

export const KEPT_PREFIX = "kelpie-draft:chat-pending:";
/** At most this many kept per chat, and on this phone in all: the newest are kept, the rest are flagged as not kept. */
export const KEPT_PER_CHAT = 20;
export const KEPT_TOTAL = 60;

const keyOf = (chat: string) => `${KEPT_PREFIX}${chat}`;

/** What was kept for `chat`. */
export function loadKept(chat: string): PendingSend[] {
  const v = store.json<unknown>(keyOf(chat), []);
  return Array.isArray(v) ? (v as PendingSend[]) : [];
}

/** Keeps `kept` (oldest first) for `chat`, as much as the caps and the browser allow; returns the `cid`s not kept. */
export function saveKept(chat: string, kept: readonly PendingSend[]): string[] {
  const key = keyOf(chat);
  if (!kept.length) {
    store.del(key);
    return [];
  }
  const others = store.keys(KEPT_PREFIX).filter((k) => k !== key).reduce((n, k) => n + (store.json<unknown>(k, []) as unknown[]).length, 0);
  const room = Math.max(0, Math.min(KEPT_PER_CHAT, KEPT_TOTAL - others));
  const keep = kept.slice(Math.max(0, kept.length - room));
  // A copy that cannot be written is not left behind either: it could bring back a message the person since let go.
  if (!keep.length || !store.keep(key, JSON.stringify(keep))) {
    store.del(key);
    return kept.map((p) => p.cid);
  }
  return kept.slice(0, kept.length - keep.length).map((p) => p.cid);
}

/** Drops the copies kept for chats that are not in `chats` (the computer's whole list). An empty list says nothing. */
export function forgetGoneChats(chats: readonly string[]) {
  if (!chats.length) return;
  const known = new Set(chats.map(keyOf));
  for (const k of store.keys(KEPT_PREFIX)) if (!known.has(k)) store.del(k);
}
