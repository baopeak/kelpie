import { describe, expect, it } from "vitest";
import { missedPhoneStarts, type AuditEntry } from "./phone-starts";

const entry = (at: number, action: string, summary: string, device = "ph1"): AuditEntry => ({ at, device, action, summary });

// Newest first, as the daemon gives them.
const LOG: AuditEntry[] = [
  entry(500, "session_new", "Codex in Docs (session s-5)", "ph2"),
  entry(400, "input", "s-4: 3 bytes, sha256 abc"),
  entry(300, "session_new", "Claude Code in Shop (with brackets) (session s-3)"),
  entry(200, "session_new", "Claude Code in Shop (session s-2)"),
  entry(100, "session_new", "Gemini in Old"),
];

describe("phone starts the window missed while it was closed", () => {
  it("are the starts newer than what was seen, oldest first, with their session and what was started", () => {
    const { starts, newest } = missedPhoneStarts(LOG, 150, new Set());
    expect(starts).toEqual([
      { at: 200, device: "ph1", sessionId: "s-2", what: "Claude Code in Shop" },
      { at: 300, device: "ph1", sessionId: "s-3", what: "Claude Code in Shop (with brackets)" },
      { at: 500, device: "ph2", sessionId: "s-5", what: "Codex in Docs" },
    ]);
    expect(newest).toBe(500);
  });

  it("leave out what Activity already shows and what was seen, so each is shown once", () => {
    expect(missedPhoneStarts(LOG, 300, new Set()).starts.map((s) => s.sessionId)).toEqual(["s-5"]);
    expect(missedPhoneStarts(LOG, 150, new Set(["s-3", "s-5"])).starts.map((s) => s.sessionId)).toEqual(["s-2"]);
    const { starts, newest } = missedPhoneStarts(LOG, 500, new Set());
    expect(starts).toEqual([]);
    expect(newest).toBe(500);
    expect(missedPhoneStarts([], 700, new Set()).newest, "never goes back").toBe(700);
  });

  it("show none the first time, and remember the newest so that only later ones are shown", () => {
    expect(missedPhoneStarts(LOG, undefined, new Set())).toEqual({ starts: [], newest: 500 });
    expect(missedPhoneStarts([], undefined, new Set()), "nothing yet: every later one is new").toEqual({ starts: [], newest: 0 });
  });

  it("keep a line from an older daemon that names no session, in its own words", () => {
    expect(missedPhoneStarts(LOG, 50, new Set()).starts[0]).toEqual({ at: 100, device: "ph1", sessionId: undefined, what: "Gemini in Old" });
  });
});
