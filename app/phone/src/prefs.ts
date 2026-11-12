// The phone's own preferences, kept in this browser per app address (a hosted app and the app from the computer each have their own).
// A stored value this build does not know reads as the default, so an older or newer bundle never breaks on what another one kept.
import { createSignal, type Accessor } from "solid-js";
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

/** How long the app may stay out of sight before coming back asks for the passkey again, in seconds. 0 is "Immediately". */
export const LOCK_GRACE_CHOICES = [0, 60, 300, 900] as const;
export type LockGrace = (typeof LOCK_GRACE_CHOICES)[number];
export const lockGrace = choice<LockGrace>("kelpie-lock-grace", LOCK_GRACE_CHOICES, 300);
