// Who needs the user, in what order: sessions waiting on a prompt, the longest wait first.
// The daemon marks a session `waiting` from agent hooks, a bell, or a prompt it found on the
// screen (menus, yes/no — agents without hooks show nothing else), with `waiting_since`.
import { createSignal } from "solid-js";
import type { SessionInfo } from "./types";

/** Daemon API level that brings `waiting_since`, prompts in `session.read` and `session.answer`. */
export const ANSWER_API = 7;

/** Daemon API level of the phone protocol as it is now: state sync by session id, acknowledged frames, heartbeats, answer results, `load.status`, `chat.resolved`, sending chat messages from the phone, and kelpie-link (end-to-end encrypted pairing by key, `mobile.set_permission`, `mobile.reset`, decisions only from the verified app), and Tailscale mode (`mobile.tailscale_status`, `mobile.set_tailscale`, `endpoints` in `addrs`). The one place that names it. */
export const PHONE_API = 23;

/** Daemon API level that owns the queue of messages for a chat (`chat.send`, `chat.queue`, a phone's `chat_send`). Below it the window queues and writes its own messages. */
export const CHAT_QUEUE_API = 21;

/** Whether a daemon at level `api` owns the queue. A lower level has no `chat.send`, however close the number. */
export const daemonOwnsQueue = (api: number) => api >= CHAT_QUEUE_API;

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
