// The phone's own small settings and its device token, in this browser only. Private browsing may refuse
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
  json<T>(key: string, fallback: T): T {
    try {
      const raw = store.get(key);
      return raw ? (JSON.parse(raw) as T) : fallback;
    } catch {
      return fallback;
    }
  },
};

export const TOKEN_KEY = "kelpie-token";

/** Drafts and any earlier build's saved reply history: what was typed on this phone is not kept once the device is forgotten. */
export function wipeTypedText() {
  try {
    for (const key of Object.keys(localStorage)) if (key.startsWith("kelpie-draft:") || key === "kelpie-history") localStorage.removeItem(key);
  } catch {
    /* no storage */
  }
  for (const key of [...memory.keys()]) if (key.startsWith("kelpie-draft:") || key === "kelpie-history") memory.delete(key);
}

/** The device token. Phones paired before the rename to Kelpie keep it under the old key. */
export function loadToken(): string | null {
  const token = store.get(TOKEN_KEY);
  if (token) return token;
  const old = store.get("erka-token");
  if (old) {
    store.set(TOKEN_KEY, old);
    store.del("erka-token");
  }
  return old;
}
