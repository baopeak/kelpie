// What every composer shares, whichever way its text travels (the desktop's composer, the phone's
// reply box): when Enter sends and when it only confirms an input method's composition, a draft with
// its history, and the size checks for text pasted in. Pure logic, no DOM, no transport.
import { remember, stepHistory } from "./prompt-history";

/** The most text the daemon takes in one `input` from a phone (`MAX_TYPED`). */
export const MAX_SEND_CHARS = 16 * 1024;
/** A paste longer than this asks before it goes in: it is probably the wrong clipboard. */
export const LONG_PASTE_CHARS = 1500;
export const LONG_PASTE_LINES = 25;

/** The part of a keyboard event the Enter guard looks at. */
export interface KeyLike {
  key: string;
  shiftKey?: boolean;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  isComposing?: boolean;
  keyCode?: number;
}

/** Safari ends a composition *before* the keydown of the Enter that confirmed it, with `isComposing`
 *  already false: an Enter this soon after `compositionend` is still the confirmation. */
const AFTER_COMPOSITION_MS = 60;

/** Tells an Enter that sends from one that confirms an input method's composition (Telex, VNI,
 *  Japanese, Chinese, dictation). Feed it `compositionstart` / `compositionend`, ask it on `keydown`. */
export class EnterGuard {
  private composing = false;
  private endedAt = -Infinity;

  constructor(private now: () => number = () => performance.now()) {}

  compositionStart() {
    this.composing = true;
  }

  compositionEnd() {
    this.composing = false;
    this.endedAt = this.now();
  }

  /** Whether this key press sends the text: Enter alone (or with Ctrl/Cmd), never mid-composition, never Shift+Enter (a new line). */
  sends(e: KeyLike): boolean {
    if (e.key !== "Enter" || e.shiftKey || e.altKey) return false;
    // keyCode 229 is what browsers report for any key handled by an input method.
    if (this.composing || e.isComposing || e.keyCode === 229) return false;
    return this.now() - this.endedAt > AFTER_COMPOSITION_MS;
  }
}

/** Whether pasted text should be confirmed first. */
export function isLongPaste(text: string): boolean {
  return text.length > LONG_PASTE_CHARS || text.split("\n").length > LONG_PASTE_LINES;
}

/** The checks on text about to be sent: what to say when it cannot go, or that it can. */
export type SendCheck = { ok: true; text: string } | { ok: false; reason: "empty" | "too_long" };

export function checkSend(text: string): SendCheck {
  if (!text.trim()) return { ok: false, reason: "empty" };
  if (text.length > MAX_SEND_CHARS) return { ok: false, reason: "too_long" };
  return { ok: true, text };
}

/** What stays in the field after `sent` went: anything typed behind it while it was on its way. */
export function draftAfterSend(now: string, sent: string): string {
  return now.startsWith(sent) ? now.slice(sent.length).trimStart() : now;
}

/** Runs one send at a time: Enter pressed again, or the button tapped, while the first is still on its way does nothing. */
export class SendLatch {
  private busy = false;

  get sending(): boolean {
    return this.busy;
  }

  /** Runs `fn` unless one is already running; resolves with its result, or undefined when it did not run. */
  async run<T>(fn: () => Promise<T>): Promise<T | undefined> {
    if (this.busy) return undefined;
    this.busy = true;
    try {
      return await fn();
    } finally {
      this.busy = false;
    }
  }
}

/** What stays in the box once `sent` has gone: nothing if it is unchanged, what was typed after it if it only grew,
 *  and the box as it is if the person edited the sent text meanwhile (it is theirs). */
export function unsentRest(current: string, sent: string): string {
  if (current === sent) return "";
  return current.startsWith(sent) ? current.slice(sent.length).replace(/^\n/, "") : current;
}

/** Whether the last line on screen asks for a secret (a password, passphrase, PIN or token): what is typed there is
 *  not worth remembering in a history. */
export function looksLikePasswordPrompt(rows: readonly string[]): boolean {
  for (let i = rows.length - 1; i >= 0; i--) {
    const line = rows[i].trim();
    if (!line) continue;
    return /(password|passphrase|passcode|\bpin\b|secret|token|api[ _-]?key)[^:?]{0,40}[:?]\s*$/i.test(line);
  }
  return false;
}

/** A draft and the messages sent before it, newest first; ↑ / ↓ walk them, the draft in progress is kept. */
export class Composer {
  history: string[];
  private index = -1;
  private draft = "";

  constructor(history: readonly string[] = []) {
    this.history = [...history];
  }

  /** `text` was sent: it is the newest history entry and the draft is empty again. */
  sent(text: string) {
    this.history = remember(this.history, text);
    this.index = -1;
    this.draft = "";
  }

  /** The text after ↑ (older) or ↓ (newer) from `current`; `null` when there is nowhere to go. */
  step(dir: 1 | -1, current: string): string | null {
    if (!this.history.length) return null;
    if (this.index === -1) {
      if (dir === -1) return null;
      this.draft = current;
    }
    const next = stepHistory(this.history.length, this.index, dir);
    if (next === this.index) return null;
    this.index = next;
    return next === -1 ? this.draft : this.history[next];
  }

  /** Forgets everything sent (the device was forgotten). */
  clear() {
    this.history = [];
    this.index = -1;
    this.draft = "";
  }

  /** The text was edited by hand: history stepping starts over from it. */
  edited() {
    this.index = -1;
  }
}
