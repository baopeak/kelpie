// Who needs the user, in what order: sessions waiting on a prompt, the longest wait first.
// The daemon marks a session `waiting` from agent hooks, a bell, or a prompt it found on the
// screen (menus, yes/no — agents without hooks show nothing else), with `waiting_since`.
import { createSignal } from "solid-js";
import type { SessionInfo } from "./types";

/** Daemon API level that brings `waiting_since`, prompts in `session.read` and `session.answer`. */
export const ANSWER_API = 7;

/** Daemon API level of phone protocol v2 (state sync, id addressing, `chat.resolved`). The one place that names it; set to the level the release gives it. */
export const PHONE_V2_API = 14;

/** Live sessions waiting on the user, the longest wait first (unknown start times last). */
export function waitingSessions(sessions: SessionInfo[]): SessionInfo[] {
  return sessions
    .filter((s) => s.alive && s.status === "waiting")
    .sort((a, b) => (a.waiting_since ?? Number.MAX_SAFE_INTEGER) - (b.waiting_since ?? Number.MAX_SAFE_INTEGER) || a.created_at - b.created_at);
}

/** The waiting session after `currentId` in that order, wrapping around; the first when the
 *  current one is not waiting. */
export function nextWaiting(sessions: SessionInfo[], currentId?: string | null): SessionInfo | undefined {
  const list = waitingSessions(sessions);
  const i = list.findIndex((s) => s.id === currentId);
  return list[(i + 1) % Math.max(list.length, 1)];
}

/** "12 s", "3 min", "2 h" since `since` (wall-clock ms). */
export function waitedFor(since: number, now = Date.now()): string {
  const s = Math.max(0, Math.round((now - since) / 1000));
  if (s < 60) return `${s} s`;
  if (s < 3600) return `${Math.floor(s / 60)} min`;
  return `${Math.floor(s / 3600)} h`;
}

/** A prompt found on screen, from `session.read`. */
export interface ScreenPrompt {
  keys: string[];
  question?: string | null;
  /** Typed as a line: the key needs Enter after it. */
  line: boolean;
}

/** What a key button sends: the key, with Enter for line prompts. */
export function keyAnswer(prompt: ScreenPrompt, key: string): { data: string; enter: boolean } {
  return { data: key, enter: prompt.line };
}

/** The session the quick-answer popover is open for. */
const [answering, setAnswering] = createSignal<string | null>(null);
export { answering, setAnswering };
