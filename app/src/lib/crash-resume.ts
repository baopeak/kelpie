// A session whose agent crashed (it exited with an error nobody asked for) while sub-sessions it opened still run is resumed by itself:
// its children go on working and report to it, and their reports wait for it. A few tries with a growing pause, then it is left to the
// person, so an agent that crashes again at once (the machine short of memory) is not started over and over.
import type { SessionInfo } from "./types";

/** Pauses before each try; their count is how many tries a conversation gets within `WINDOW_MS`. */
export const DELAYS_MS = [5_000, 30_000, 120_000];
export const WINDOW_MS = 10 * 60_000;

/** What `crashed` reads of a session before an update. A copy: the store updates a session's object in place, so the object read
 *  before the update already says what the update says. */
export type ExitState = Pick<SessionInfo, "alive" | "switching">;
export const exitState = (s: SessionInfo | undefined): ExitState | undefined => s && { alive: s.alive, switching: s.switching };

/** `next` is `prev` after its process ended in an error: not a stop asked for, not a normal exit, not the end of a move to the other
 *  view (which may end in an error, and whose new session already resumes the conversation). */
export const crashed = (prev: ExitState | undefined, next: SessionInfo) => !!prev?.alive && !next.alive && !!next.failed && !next.switching && !prev.switching;

/** A chat resumes in a new session: without its conversation's id it would start another conversation, or take the folder's latest,
 *  which may be a sub-session's, and every new session would count its tries afresh. Such a chat is left to the person. (A terminal
 *  goes by the store's `resumable`.) */
export const chatResumable = (s: SessionInfo) => resumeKey(s) !== s.id;

/** A sub-session `id` opened is still running. */
export const hasLiveChildren = (id: string, sessions: Record<string, SessionInfo>) => Object.values(sessions).some((s) => s.parent === id && s.alive);

/** What a resume is counted against: the conversation, which a chat resumed in a new session keeps; else the session. */
export const resumeKey = (s: SessionInfo) => (s.agent_session_id && s.agent_session_id !== "latest" ? s.agent_session_id : s.id);

/** The tries made for each conversation, as times. */
export class CrashResumes {
  private tries = new Map<string, number[]>();

  /** The pause before the next try for `key` at `now`, counting it; null when it has had its tries within the window. */
  next(key: string, now: number): number | null {
    const recent = (this.tries.get(key) ?? []).filter((t) => now - t < WINDOW_MS);
    if (recent.length >= DELAYS_MS.length) {
      this.tries.set(key, recent);
      return null;
    }
    this.tries.set(key, [...recent, now]);
    return DELAYS_MS[recent.length];
  }
}
