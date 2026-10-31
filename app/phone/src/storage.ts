// The phone's own small settings, in this browser only (its pairing keys are in pairing-store.ts). Private browsing may refuse
// storage: everything here then quietly lives for the page's lifetime only.
const memory = new Map<string, string>();

export const store = {
  get(key: string): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return memory.get(key) ?? null;
    }
  },
  set(key: string, value: string) {
    try {
      localStorage.setItem(key, value);
    } catch {
      memory.set(key, value);
    }
  },
  del(key: string) {
    try {
      localStorage.removeItem(key);
    } catch {
      memory.delete(key);
    }
  },
  /** Like `set`, but only into the browser's storage: false when it would not take it (full, or refused), and nothing is
   *  kept for the page alone, so the caller can tell the person. */
  keep(key: string, value: string): boolean {
    try {
      localStorage.setItem(key, value);
      return true;
    } catch {
      return false;
    }
  },
  /** The keys that start with `prefix`. */
  keys(prefix: string): string[] {
    let stored: string[] = [];
    try {
      stored = Object.keys(localStorage);
    } catch {
      /* no storage */
    }
    return [...new Set([...stored, ...memory.keys()])].filter((k) => k.startsWith(prefix));
  },
  json<T>(key: string, fallback: T): T {
    try {
      const raw = store.get(key);
      return raw ? (JSON.parse(raw) as T) : fallback;
    } catch {
      return fallback;
    }
  },
};


/** Drafts and any earlier build's saved reply history: what was typed on this phone is not kept once the device is forgotten. */
export function wipeTypedText() {
  try {
    for (const key of Object.keys(localStorage)) if (key.startsWith("kelpie-draft:") || key === "kelpie-history") localStorage.removeItem(key);
  } catch {
    /* no storage */
  }
  for (const key of [...memory.keys()]) if (key.startsWith("kelpie-draft:") || key === "kelpie-history") memory.delete(key);
}

/** Whether this browser holds a device token from before end-to-end encryption (which is removed: nothing accepts it). */
export function hadOldToken(): boolean {
  const had = !!(store.get("kelpie-token") ?? store.get("erka-token"));
  store.del("kelpie-token");
  store.del("erka-token");
  return had;
}
