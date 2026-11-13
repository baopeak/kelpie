// Who needs the user, in what order: sessions waiting on a prompt, the longest wait first.
// The daemon marks a session `waiting` from agent hooks, a bell, or a prompt it found on the
// screen (menus, yes/no — agents without hooks show nothing else), with `waiting_since`.
import { createSignal } from "solid-js";
import type { SessionInfo } from "./types";
import { needsYou } from "./session-state";

/** Daemon API level that brings `waiting_since`, prompts in `session.read` and `session.answer`. */
export const ANSWER_API = 7;

/** Daemon API level of the phone protocol as it is now: state sync by session id, acknowledged frames, heartbeats, answer results, `load.status`, `chat.resolved`, sending chat messages from the phone, and kelpie-link (end-to-end encrypted pairing by key, `mobile.set_permission`, `mobile.reset`, decisions only from the verified app), and Tailscale mode (`mobile.tailscale_status`, `mobile.set_tailscale`, `endpoints` in `addrs`), and Web Push (`push_info`, `push_sub`, `push_unsub`, `push_test`, `push` in `hello`), and the app lock with its risk labels (`lock_*`, `locked`, `wait` on a frame, `chat_request_info`). The one place that names it. */
export const PHONE_API = 25;

/** Daemon API level that has Tailscale mode. Updates keep the previous daemon running, so its controls must not ask for more than this. */
export const TAILSCALE_API = 23;

/** Daemon API level that has the phones' app lock (`mobile.set_lock`, `mobile.reset_lock`, `lock` in `mobile.status`). */
export const LOCK_API = 25;

/** Daemon API level that has Telegram's choices: alerts, alerts with a link, approvals of low-risk requests (`telegram.mode`, `notify.telegram_bind`, the `notify.telegram` event). */
export const TELEGRAM_API = 26;

/** Daemon API level that has the relay (`mobile.set_relay`, `mobile.relay_recreate`, `relay` in `mobile.status`). */
export const RELAY_API = 27;

/** Daemon API level that has browser pane takeover: the app provides panes (`provider.register`), the phone's `panes` and `tk_*`, and `control` for a
 *  browser device paired on a secure origin with its app lock on (`may_control` in `mobile.status`). Below it nothing of it is offered. */
export const TAKEOVER_API = 28;

/** Daemon API level that has the computer's screen for a phone with control (`screens`, `sc_*`, binary media on the link, the app's `screen` provider). Below it the
 *  phone offers no screen and Settings shows no screen options. */
export const SCREEN_API = 29;

/** Daemon API level that has the phone app the relay hosts (`mobile.pair_start {hosted}`, `relay.hosted` and `new_store` in `mobile.status`, `page_kind` on a
 *  device, `origin_kind` on a pairing request, `mobile.enable {lan}`) and makes rooms with no registration key. Below it the hosted app is not offered. */
export const HOSTED_API = 34;

/** Daemon API level that has each phone's own alert choices (the phone's `push_prefs`, `prefs` and `events` in its hello's `push`, `alerts` on a device in
 *  `mobile.status`). Below it the phone shows the choices disabled and says the computer needs an update. */
export const ALERT_PREFS_API = 37;

/** Daemon API level that starts sessions from a phone (`mobile.set_launch`, the phone's `new_options` and `new_session`, the `mobile.session_started`
 *  event). Below it the app sends no launch catalogue and the phone shows no "+". */
export const NEW_SESSION_API = 38;

/** Daemon API level that lets agents use browser panes (`browser.call` forwarded to this window, the policy keys `browser_agents`, `browser_eval`,
 *  `browser_sites`, `browser.audit`). Below it Settings shows no Browser panes section and panes offer no sharing with agents. */
export const BROWSER_AGENT_API = 39;
/** Daemon API level whose screen sends the stream's clock (`t0`), sizes its window of frames to the link and takes the phone's `sc_sharpen`.
 *  Below it the phone shows no Sharpen and no picture age. */
export const SCREEN_FLOW_API = 40;

/** Daemon API level that may show the screen through the relay (`mobile.set_screen`, `screen` in `mobile.status`, `device.screen` in the phone's hello,
 *  `paths` in `screens`, `sc_start {via, recheck}`). Below it the app offers no such switch and the phone never asks for the screen through the relay. */
export const SCREEN_REMOTE_API = 41;

/** Daemon API level that owns the queue of messages for a chat (`chat.send`, `chat.queue`, a phone's `chat_send`). Below it the window queues and writes its own messages. */
export const CHAT_QUEUE_API = 21;

/** Whether a daemon at level `api` owns the queue. A lower level has no `chat.send`, however close the number. */
export const daemonOwnsQueue = (api: number) => api >= CHAT_QUEUE_API;

/** Live sessions that need the user, the same ones the status bar counts: those waiting on a prompt first, the longest wait first
 *  (unknown start times last), then quiet sub-sessions that reported they need you or are blocked, the oldest report first. */
export function waitingSessions(sessions: SessionInfo[]): SessionInfo[] {
  const kind = new Map<string, 1 | 2>();
  for (const s of sessions) {
    const k = needsYou(s);
    if (k) kind.set(s.id, k);
  }
  return sessions
    .filter((s) => kind.has(s.id))
    .sort((a, b) =>
      kind.get(a.id)! - kind.get(b.id)! ||
      (kind.get(a.id) === 1
        ? (a.waiting_since ?? Number.MAX_SAFE_INTEGER) - (b.waiting_since ?? Number.MAX_SAFE_INTEGER)
        : (a.report?.at ?? 0) - (b.report?.at ?? 0)) ||
      a.created_at - b.created_at);
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
