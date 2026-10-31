// This phone's pairing with one computer: its own Noise static key, the computer's key, its device id and the addresses it was
// told. Kept in IndexedDB (a database of its own, so clearing the screen cache keeps it); a small marker in localStorage
// lets the first paint know there is a pairing before IndexedDB answers. Private browsing may refuse storage: the pairing
// then lasts as long as the page.
import type { Endpoint } from "./noise";
import { store } from "./storage";

export interface Pairing {
  priv: Uint8Array;
  pub: Uint8Array;
  /** The computer's Noise static key. */
  dpk: Uint8Array;
  deviceId: string;
  /** This device's name, as the computer knows it. */
  name: string;
  /** The computer's name. */
  computer: string;
  /** Where the computer can be reached: direct addresses (the newest `addrs` replace the `lan` ones) and, later, relays. */
  endpoints: Endpoint[];
  addrsSeq: number;
}

const DB = "kelpie-link";
const STORE = "kv";
const KEY = "pairing";
const MARKER = "kelpie-paired";
let memory: Pairing | null = null;
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

function valid(p: unknown): p is Pairing {
  const x = p as Pairing | null;
  return !!x && x.priv instanceof Uint8Array && x.priv.length === 32 && x.pub instanceof Uint8Array && x.dpk instanceof Uint8Array && x.dpk.length === 32 && typeof x.deviceId === "string";
}

/** Whether a pairing was saved (to paint the right screen before IndexedDB answers). */
export const seemsPaired = () => store.get(MARKER) === "1";

export async function loadPairing(): Promise<Pairing | null> {
  if (memory) return memory;
  const d = await db();
  if (!d) return null;
  const got = await new Promise<unknown>((resolve) => {
    try {
      const req = d.transaction(STORE).objectStore(STORE).get(KEY);
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
    } catch {
      resolve(null);
    }
  });
  memory = valid(got) ? got : null;
  if (!memory) store.del(MARKER);
  return memory;
}

/** Keeps the pairing; false when the browser would not store it (it then lasts as long as this page). */
export async function savePairing(p: Pairing): Promise<boolean> {
  memory = p;
  store.set(MARKER, "1");
  const d = await db();
  if (!d) return false;
  return new Promise((resolve) => {
    try {
      const tx = d.transaction(STORE, "readwrite");
      tx.objectStore(STORE).put(p, KEY);
      tx.oncomplete = () => resolve(true);
      tx.onerror = () => resolve(false);
    } catch {
      resolve(false);
    }
  });
}

export async function forgetPairing(): Promise<void> {
  memory = null;
  store.del(MARKER);
  const d = await db();
  try {
    d?.transaction(STORE, "readwrite").objectStore(STORE).delete(KEY);
  } catch {
    /* nothing kept */
  }
}

/** The pairing as it is in memory now (after `loadPairing`). */
export const currentPairing = () => memory;
