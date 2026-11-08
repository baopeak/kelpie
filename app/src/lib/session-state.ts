// What a session shows at a glance, and whether it needs the user. Pure, so the sidebar, the status bar, the attention list, the
// palette and the board all decide it the same way.
import { stateWithReport } from "./sub-sessions";
import type { SessionInfo } from "./types";

/** Each state has its own shape in the sidebar, so colour is never the only cue. */
export type SessionState = "waiting" | "error" | "working" | "done" | "idle" | "exited";

export function sessionState(s: SessionInfo): SessionState {
  if (!s.alive) return s.failed ? "error" : "exited";
  if (s.status === "waiting") return "waiting";
  if (s.status === "working") return "working";
  if (s.failed) return "error";
  return s.unread ? "done" : "idle";
}

/** A session's state with what it last reported to its parent: a quiet sub-session that said it needs you (or is blocked) shows as
 *  waiting, one that said it failed as an error. */
export const reportedState = (s: SessionInfo): SessionState => stateWithReport(sessionState(s), s.report?.status);

/** 1: waits on a prompt; 2: a quiet sub-session that reported it needs you or is blocked; 0: neither. */
export function needsYou(s: SessionInfo): 0 | 1 | 2 {
  if (sessionState(s) === "waiting") return 1;
  return reportedState(s) === "waiting" ? 2 : 0;
}
