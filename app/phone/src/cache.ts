// The last state this browser saw, kept in IndexedDB so that a page opened on a bad network (or with the
// computer off) paints the session list and the last screen at once, dimmed and marked "as of …", before
// the connection says anything. Everything here fails soft: no storage means no cache.
import type { SessionRow } from "./link";

export interface CachedList {
  at: number;
  sessions: SessionRow[];
  projects: Record<string, string>;
  device: { name: string; can_write: boolean } | null;
}

export interface CachedScreen {
  at: number;
  rows: string[];
  title: string | null;
}

const DB = "kelpie-phone";
const STORE = "kv";

let opened: Promise<IDBDatabase | null> | null = null;

function db(): Promise<IDBDatabase | null> {
  opened ??= new Promise((resolve) => {
    try {
      const req = indexedDB.open(DB, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(STORE);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
      req.onblocked = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
  return opened;
}

async function read<T>(key: string): Promise<T | null> {
  const d = await db();
  if (!d) return null;
  return new Promise((resolve) => {
    try {
      const req = d.transaction(STORE).objectStore(STORE).get(key);
      req.onsuccess = () => resolve((req.result as T | undefined) ?? null);
      req.onerror = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
}

async function write(key: string, value: unknown): Promise<void> {
  const d = await db();
  if (!d) return;
  try {
    d.transaction(STORE, "readwrite").objectStore(STORE).put(value, key);
  } catch {
    /* a full or refused store: the next page just starts without a cache */
  }
}

export const loadList = () => read<CachedList>("list");
export const loadScreen = (id: string) => read<CachedScreen>(`screen:${id}`);

/** Writes at most once every `gap` ms per key, always the newest value. */
export function throttled(gap: number, sink: (key: string, value: unknown) => void) {
  const latest = new Map<string, unknown>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  const put = (key: string, value: unknown) => {
    latest.set(key, value);
    if (timers.has(key)) return;
    timers.set(
      key,
      setTimeout(() => {
        timers.delete(key);
        sink(key, latest.get(key));
      }, gap),
    );
  };
  /** Drops writes that are waiting: they must not come back after the cache was cleared. */
  const cancel = () => {
    for (const t of timers.values()) clearTimeout(t);
    timers.clear();
    latest.clear();
  };
  return { put, cancel };
}

const writes = throttled(1500, (key, value) => void write(key, value));
const put = writes.put;

export const saveList = (list: CachedList) => put("list", list);
export const saveScreen = (id: string, screen: CachedScreen) => put(`screen:${id}`, screen);

/** Forgets everything kept (the device was unpaired: what it saw is no longer its to keep). */
export async function clearCache(): Promise<void> {
  writes.cancel();
  const d = await db();
  d?.close();
  opened = null;
  try {
    indexedDB.deleteDatabase(DB);
  } catch {
    /* nothing to delete */
  }
}
