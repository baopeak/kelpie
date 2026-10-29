// How a ChatController reaches the conversation it shows. The controller and the agent adapters
// know nothing of Tauri or of the phone socket: the desktop wraps the IPC (a terminal-style attach
// and raw lines to the agent's stdin, `desktop-transport.ts`), the phone wraps its socket
// (`app/phone/src/chat-link.ts`), where a chat can only be watched and its questions answered.

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
  detach?(): void;
  /** Writes one JSON line (with its newline) to the agent's stdin. Desktop only. */
  send?(line: string): void;
  /** Answers the agent's request `requestId` with `line` (the agent's own answer message). */
  answer?(requestId: string | number, line: string): Promise<AnswerOutcome>;
  /** Tells the daemon what the conversation turned out to be (desktop only: an observer never changes a session). */
  update?(patch: { agent_session_id?: string; model?: string }): void;
}
