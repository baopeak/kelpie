// The phone's own preferences, kept in this browser per app address (a hosted app and the app from the computer each have their own).
// A stored value this build does not know reads as the default, so an older or newer bundle never breaks on what another one kept.
import { createSignal, type Accessor } from "solid-js";
import type { InboxOrder } from "./inbox-order";
import { readVia, viaKey, type ViaId } from "./screen-paths";
import { store } from "./storage";

export interface Pref<T> {
  get: Accessor<T>;
  set(value: T): void;
}

/** One preference: its signal starts from what is kept, and setting it keeps the new value. */
export function pref<T>(key: string, fallback: T, read: (raw: string) => T | undefined, write: (value: T) => string = String): Pref<T> {
  const load = (): T => {
    const raw = store.get(key);
    if (raw === null) return fallback;
    return read(raw) ?? fallback;
  };
  const [get, put] = createSignal<T>(load());
  return {
    get,
    set(value: T) {
      store.set(key, write(value));
      put(() => value);
    },
  };
}

/** A choice out of a fixed list, kept as text. */
export function choice<T extends string | number>(key: string, choices: readonly T[], fallback: T): Pref<T> {
  return pref<T>(key, fallback, (raw) => choices.find((c) => String(c) === raw));
}

/** On or off, kept as "1" or "0". */
export function flag(key: string, fallback: boolean): Pref<boolean> {
  return pref<boolean>(key, fallback, (raw) => (raw === "1" ? true : raw === "0" ? false : undefined), (on) => (on ? "1" : "0"));
}

/** How long the app may stay out of sight before coming back asks for the passkey again, in seconds. 0 is "Immediately". */
export const LOCK_GRACE_CHOICES = [0, 60, 300, 900] as const;
export type LockGrace = (typeof LOCK_GRACE_CHOICES)[number];
export const lockGrace = choice<LockGrace>("kelpie-lock-grace", LOCK_GRACE_CHOICES, 300);

/** The size of a terminal's text and of a chat's, as a step from the usual. */
export const TEXT_SIZES = ["s", "m", "l", "xl"] as const;
export type TextSize = (typeof TEXT_SIZES)[number];
export const textSize = choice<TextSize>("kelpie-text-size", TEXT_SIZES, "m");

/** Long terminal lines wrap; off, they stay one line and the screen scrolls sideways. */
export const wrapLines = flag("kelpie-wrap-lines", true);

/** Ctrl+C stops what the agent is doing: the key bar and Ctrl with "c" in the reply box ask first. */
export const confirmCtrlC = flag("kelpie-confirm-ctrl-c", true);

/** A buzz when a session starts needing the person, while the app is open (where the phone can buzz at all). */
export const vibrate = flag("kelpie-vibrate", true);

/** The inbox lists sessions by what they ask of the person, or by the workspace they run in. */
export const INBOX_VIEWS = ["status", "workspace"] as const;
export type InboxView = (typeof INBOX_VIEWS)[number];
export const inboxView = choice<InboxView>("kelpie-inbox-view", INBOX_VIEWS, "status");

/** Stopped sessions are listed; off, the inbox leaves them out and says how many. */
export const showStopped = flag("kelpie-show-stopped", true);

/** The order inside a group of the inbox (sessions that need the person always put the longest wait first). */
export const INBOX_ORDERS: readonly InboxOrder[] = ["recent", "name"];
export const order = choice<InboxOrder>("kelpie-inbox-order", INBOX_ORDERS, "recent");

/** The way the screen connected last, kept per pairing (the key holds the pairing's device id): preselected the next time. */
export const rememberedVia = (pairingId: string): ViaId | null => readVia(store.get(viaKey(pairingId)));
export const rememberVia = (pairingId: string, via: ViaId) => store.set(viaKey(pairingId), via);

/** The key bar's keys after Ctrl, which always comes first: the daemon's names for them, in the usual order. */
export const KEY_IDS = ["esc", "tab", "shift-tab", "left", "up", "down", "right", "pgup", "pgdn", "ctrl-c", "enter"] as const;
export type KeyId = (typeof KEY_IDS)[number];

export interface KeyBarSetting {
  /** Every key, in the order the bar shows them. */
  order: KeyId[];
  hidden: KeyId[];
}

export const defaultKeyBar = (): KeyBarSetting => ({ order: [...KEY_IDS], hidden: [] });

const isKey = (k: unknown): k is KeyId => typeof k === "string" && (KEY_IDS as readonly string[]).includes(k);

/** What was kept, with what this build does not know left out and a key it has that was not kept added at the end. */
export function readKeyBar(raw: string): KeyBarSetting | undefined {
  let v: unknown;
  try {
    v = JSON.parse(raw);
  } catch {
    return undefined;
  }
  const o = v as { order?: unknown; hidden?: unknown } | null;
  if (!o || typeof o !== "object" || !Array.isArray(o.order) || !Array.isArray(o.hidden)) return undefined;
  const order = [...new Set(o.order.filter(isKey))];
  for (const k of KEY_IDS) if (!order.includes(k)) order.push(k);
  return { order, hidden: [...new Set(o.hidden.filter(isKey))] };
}

export const keyBar = pref<KeyBarSetting>("kelpie-key-bar", defaultKeyBar(), readKeyBar, JSON.stringify);

/** The keys the bar shows, in order. */
export const shownKeys = (s: KeyBarSetting): KeyId[] => s.order.filter((k) => !s.hidden.includes(k));

/** `key` one place earlier (-1) or later (1); at either end it stays. */
export function moveKey(s: KeyBarSetting, key: KeyId, by: -1 | 1): KeyBarSetting {
  const order = [...s.order];
  const at = order.indexOf(key);
  const to = at + by;
  if (at < 0 || to < 0 || to >= order.length) return s;
  [order[at], order[to]] = [order[to], order[at]];
  return { ...s, order };
}

export function toggleKey(s: KeyBarSetting, key: KeyId): KeyBarSetting {
  return { ...s, hidden: s.hidden.includes(key) ? s.hidden.filter((k) => k !== key) : [...s.hidden, key] };
}

/** Puts the text size and line wrapping on the page (`<html>`), where the stylesheet reads them. Called at start and whenever either changes. */
export function applyDisplay() {
  const root = document.documentElement;
  root.dataset.textSize = textSize.get();
  root.dataset.wrap = wrapLines.get() ? "on" : "off";
}
