// How a ChatController reaches the conversation it shows. The controller and the agent adapters
// know nothing of Tauri or of the phone socket: the desktop wraps the IPC (a terminal-style attach
// and raw lines to the agent's stdin, `desktop-transport.ts`), the phone wraps its socket
// (`app/phone/src/phone-state.ts`), where a chat is watched, its questions answered and messages sent as text
// (the daemon builds the turn).
import type { EnqueueResult } from "./chat-queue";

/** What became of an answer given from a transport that can tell. */
export type AnswerOutcome =
  /** Written (`ok`), or this device had already answered it (`dup`: a retry after a lost reply). */
  | { result: "ok" | "dup" }
  /** Somebody else answered first (or the question is gone): `answeredBy` and `at` say who and when, when known. */
  | { result: "stale"; answeredBy?: string; at?: number }
  /** The connection dropped with the answer in flight and it was not settled: the person decides. */
  | { result: "unknown" }
  | { result: "refused"; message: string };

export interface ChatTransport {
  /** Follows the conversation's log: resolves with the log so far, and `onLive` gets what is printed from then on.
   *  Desktop only; a phone's log is pushed in through `ChatController.applyLog`. */
  attach?(handle: number, onLive: (bytes: Uint8Array) => void): Promise<Uint8Array | null>;
  /** Where the log `attach` last resolved with ends, as an offset in everything the agent ever printed (the log itself
   *  may have been trimmed). Without it, every attach rebuilds the transcript. */
  logEnd?(): number | null;
  detach?(): void;
  /** Writes one JSON line (with its newline) to the agent's stdin. Desktop only. */
  send?(line: string): void;
  /** Answers the agent's request `requestId` with `line` (the agent's own answer message). */
  answer?(requestId: string | number, line: string): Promise<AnswerOutcome>;
  /** The name the daemon knows this device by, in a log's `answered_by` (a phone; a window is always the desktop). */
  deviceName?(): string | undefined;
  /** Tells the daemon what the conversation turned out to be (desktop only: an observer never changes a session). */
  update?(patch: { agent_session_id?: string; model?: string }): void;

  // The daemon's queue of messages (CHAT_QUEUE_API): one per chat, written to the agent when it is idle.
  /** Desktop: queues a message under `cid`; the daemon offers it back (`chat.turn_ready`) when it is its turn. */
  enqueue?(cid: string, text: string, rich: boolean, images: string[]): Promise<EnqueueResult>;
  /** Desktop: the lines the adapter built for the message offered as `qid` (`null`: it cannot build them).
   *  `lines: null` with `retry`: not now (replaying, loading), ask again; without it, this window does not have the message.
   *  With `tooLarge`: they cannot go, so the message leaves the queue with that reason.
   *  Rejects with `too_late`, `busy`, `switching` or `too_large` when the daemon does not take them. */
  writeTurn?(qid: string, lines: string[] | null, turn: boolean, retry?: boolean, tooLarge?: boolean): Promise<void>;
  /** Desktop and phone: `cancel` a waiting message, `resume` a stopped queue. The desktop also has
   *  `front` (`interrupt`: an interrupt follows that must not pause), `take` and `hold`. */
  queueOp?(op: "cancel" | "front" | "take" | "resume" | "hold", qid?: string, opts?: { interrupt?: boolean; held?: boolean }): Promise<void>;
  /** Phone: sends a message as text; the daemon builds the turn. */
  chatSend?(cid: string, text: string): Promise<SendOutcome>;
  /** Phone: stops the agent's turn (the desktop's adapter writes its own interrupt). */
  stop?(): Promise<void>;
}

/** What became of a phone's `chat_send`. */
export type SendOutcome =
  | { result: "ok"; qid: string; state: "queued" | "sent"; ahead: number }
  /** A retry of a message the daemon already has: `state` is what became of it. */
  | { result: "dup"; qid: string; state: "queued" | "sent" | "cancelled" | "dropped" }
  /** The daemon said no: `code` is why (`rate_limited`, `queue_full`, `too_long`, `exited`, `switching`, `command_needs_desktop`, …). */
  | { result: "refused"; code: string; message: string }
  /** Not connected: nothing was sent. */
  | { result: "offline" }
  /** The connection dropped with the message in flight, or the daemon restarted: it may or may not have been sent. */
  | { result: "unknown" };
