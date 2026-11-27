// When a crashed session is resumed by itself: only a crash, only while sub-sessions it opened run, and a few times at most.
import { describe, expect, it } from "vitest";
import { CrashResumes, DELAYS_MS, WINDOW_MS, crashed, hasLiveChildren, crashResumable, resumeKey } from "./crash-resume";
import type { SessionInfo } from "./types";

const s = (over: Partial<SessionInfo>): SessionInfo => ({ id: "p", alive: true, failed: false, exit_code: null, chat: false, ...over }) as SessionInfo;

describe("crashed", () => {
  it("is a running session that ended in an error", () => {
    expect(crashed(s({}), s({ alive: false, failed: true, exit_code: -1073740791 }))).toBe(true);
  });

  it("is not a stop asked for, a normal exit, a failed turn of a live agent, or an exit seen before", () => {
    expect(crashed(s({}), s({ alive: false, failed: false, exit_code: 0 }))).toBe(false);
    expect(crashed(s({}), s({ alive: true, failed: true }))).toBe(false);
    expect(crashed(s({ alive: false, failed: true }), s({ alive: false, failed: true }))).toBe(false);
    expect(crashed(undefined, s({ alive: false, failed: true }))).toBe(false);
  });

  it("is not the end of a move to the other view, even one that ends in an error", () => {
    expect(crashed(s({ switching: true }), s({ alive: false, failed: true, switching: true }))).toBe(false);
    expect(crashed(s({ switching: true }), s({ alive: false, failed: true }))).toBe(false);
  });
});

describe("hasLiveChildren", () => {
  it("counts only running sub-sessions of that session", () => {
    const sessions = { p: s({ id: "p" }), a: s({ id: "a", parent: "p", alive: false }), b: s({ id: "b", parent: "other" }) };
    expect(hasLiveChildren("p", sessions)).toBe(false);
    expect(hasLiveChildren("p", { ...sessions, c: s({ id: "c", parent: "p" }) })).toBe(true);
  });
});

describe("CrashResumes", () => {
  it("waits longer each time, then stops trying within the window", () => {
    const r = new CrashResumes();
    expect(DELAYS_MS.map((_, i) => r.next("conv", i * 1000))).toEqual(DELAYS_MS);
    expect(r.next("conv", 5000)).toBeNull();
    // Another conversation has its own tries; after the window they start again.
    expect(r.next("other", 5000)).toBe(DELAYS_MS[0]);
    // At 1.5 s past the window the tries at 0 and 1 s are over and the one at 2 s still counts.
    expect(r.next("conv", 1500 + WINDOW_MS)).toBe(DELAYS_MS[1]);
    expect(r.next("conv", 3 * WINDOW_MS)).toBe(DELAYS_MS[0]);
  });

  it("leaves a chat without its conversation's id to the person", () => {
    expect(crashResumable(s({ chat: true, agent_session_id: "conv-1" }))).toBe(true);
    expect(crashResumable(s({ chat: true, agent_session_id: "latest" }))).toBe(false);
    expect(crashResumable(s({ chat: true }))).toBe(false);
    // A terminal resumes in the same session, with its own id.
    expect(crashResumable(s({ chat: false }))).toBe(true);
  });

  it("counts a chat resumed in a new session as the same conversation", () => {
    expect(resumeKey(s({ id: "a", agent_session_id: "conv-1" }))).toBe(resumeKey(s({ id: "b", agent_session_id: "conv-1" })));
    expect(resumeKey(s({ id: "a", agent_session_id: "latest" }))).toBe("a");
  });
});
