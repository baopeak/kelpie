import { describe, expect, it } from "vitest";
import { loadMissedPhoneStarts, missedPhoneStarts, PHONE_STARTS_LIMIT, PHONE_STARTS_READ, type AuditEntry, type AuditRead, type PhoneStartsIo } from "./phone-starts";

const entry = (at: number, action: string, summary: string, device = "ph1"): AuditEntry => ({ at, device, action, summary });

// Newest first, as the daemon gives them.
const LOG: AuditEntry[] = [
  entry(500, "session_new", "Codex in Docs (session s-5)", "ph2"),
  entry(400, "input", "s-4: 3 bytes, sha256 abc"),
  entry(300, "session_new", "Claude Code in Shop (with brackets) (session s-3)"),
  entry(200, "session_new", "Claude Code in Shop (session s-2)"),
  entry(100, "session_new", "Gemini in Old"),
];
const READ = { entries: LOG };

describe("phone starts the window missed while it was closed", () => {
  it("are asked for on their own, as many as the daemon gives, so that typing on a phone never pushes them out", () => {
    expect(PHONE_STARTS_READ).toEqual({ action: "session_new", limit: PHONE_STARTS_LIMIT });
    expect(PHONE_STARTS_LIMIT).toBe(2000);
  });

  it("are the starts newer than what was seen, oldest first, with their session and what was started", () => {
    const { starts, newest, cutOff } = missedPhoneStarts(READ, 150, new Set());
    expect(starts).toEqual([
      { at: 200, device: "ph1", sessionId: "s-2", what: "Claude Code in Shop" },
      { at: 300, device: "ph1", sessionId: "s-3", what: "Claude Code in Shop (with brackets)" },
      { at: 500, device: "ph2", sessionId: "s-5", what: "Codex in Docs" },
    ]);
    expect(newest).toBe(500);
    expect(cutOff).toBe(false);
  });

  it("leave out what Activity already shows and what was seen, so each is shown once", () => {
    expect(missedPhoneStarts(READ, 300, new Set()).starts.map((s) => s.sessionId)).toEqual(["s-5"]);
    expect(missedPhoneStarts(READ, 150, new Set(["s-3", "s-5"])).starts.map((s) => s.sessionId)).toEqual(["s-2"]);
    const { starts, newest } = missedPhoneStarts(READ, 500, new Set());
    expect(starts).toEqual([]);
    expect(newest).toBe(500);
    expect(missedPhoneStarts({ entries: [] }, 700, new Set()).newest, "never goes back").toBe(700);
  });

  it("show none the first time, and remember the newest so that only later ones are shown", () => {
    expect(missedPhoneStarts(READ, undefined, new Set())).toEqual({ starts: [], newest: 500, cutOff: false });
    expect(missedPhoneStarts({ entries: [] }, undefined, new Set()), "nothing yet: every later one is new").toEqual({ starts: [], newest: 0, cutOff: false });
  });

  it("keep a line from an older daemon that names no session, in its own words", () => {
    expect(missedPhoneStarts(READ, 50, new Set()).starts[0]).toEqual({ at: 100, device: "ph1", sessionId: undefined, what: "Gemini in Old" });
  });

  it("say that some may be missing when the answer begins after what was seen, once", () => {
    // Older lines were left out, and the oldest line read is newer than what was seen: starts in between may be among them.
    const cut = missedPhoneStarts({ entries: LOG, from: 100 }, 50, new Set());
    expect(cut.cutOff).toBe(true);
    expect(cut.starts.map((s) => s.at), "what was read is still shown").toEqual([100, 200, 300, 500]);
    // Remembered past it, so the next connect does not say it again.
    expect(missedPhoneStarts({ entries: LOG, from: 100 }, cut.newest, new Set()).cutOff).toBe(false);
    // The answer covers everything since what was seen: nothing missing.
    expect(missedPhoneStarts({ entries: LOG, from: 100 }, 100, new Set()).cutOff).toBe(false);
    expect(missedPhoneStarts({ entries: LOG, from: 100 }, 250, new Set()).cutOff).toBe(false);
    // Never the first time, when nothing is shown anyway, and its time is remembered.
    expect(missedPhoneStarts({ entries: [], from: 900 }, undefined, new Set())).toEqual({ starts: [], newest: 900, cutOff: false });
  });

  it("go into Activity by the time remembered when the window connected, never twice with a start heard live meanwhile", async () => {
    // The window: Activity's phone starts, and the remembered time, both moved by a live start while the log is being read.
    const shown = new Set<string>();
    let remembered = 150;
    const added: string[] = [];
    let cut = 0;
    const live = (id: string) => {
      shown.add(id);
      added.push(`live ${id}`);
      remembered = Math.max(remembered, 10_000);
    };
    const io = (read: AuditRead, during: { audit?: () => void; names?: () => void }): PhoneStartsIo => ({
      audit: async () => (during.audit?.(), read),
      names: async () => (during.names?.(), (id) => `Phone ${id}`),
      shown: () => shown,
      add: (s, phone) => added.push(`${phone}: ${s.sessionId}`),
      cutOff: () => cut++,
      remember: (newest) => (remembered = Math.max(remembered, newest)),
    });
    // A live start while the log is read: it moves the remembered time to now, and still the older ones are shown.
    await loadMissedPhoneStarts(150, io(READ, { audit: () => live("s-9") }), () => 9_000);
    expect(added).toEqual(["live s-9", "Phone ph1: s-2", "Phone ph1: s-3", "Phone ph2: s-5"]);
    expect(remembered).toBe(10_000);
    // A live start whose line is in the answer, heard while the phones' names are asked for: shown once.
    added.length = 0;
    shown.clear();
    await loadMissedPhoneStarts(250, io(READ, { names: () => live("s-5") }));
    expect(added).toEqual(["live s-5", "Phone ph1: s-3"]);
    expect(cut).toBe(0);
    // Older lines left out after what was seen: said once, beside what was read.
    added.length = 0;
    await loadMissedPhoneStarts(50, io({ entries: [LOG[0]], from: 500 }, {}));
    expect(added).toEqual([]);
    expect(cut).toBe(1);
    // The log could not be read: nothing changes.
    remembered = 1;
    await loadMissedPhoneStarts(50, { ...io(READ, {}), audit: async () => null });
    expect([added.length, cut, remembered]).toEqual([0, 1, 1]);
  });

  it("take a full answer from a daemon that does not say what it left out as cut at its oldest line", () => {
    const typing = Array.from({ length: PHONE_STARTS_LIMIT }, (_, i) => entry(10_000 - i, "input", "1 bytes"));
    const full = missedPhoneStarts({ entries: [entry(20_000, "session_new", "Codex in Docs (session s-9)"), ...typing.slice(1)] }, 500, new Set());
    expect(full.cutOff).toBe(true);
    expect(full.starts.map((s) => s.sessionId)).toEqual(["s-9"]);
    expect(missedPhoneStarts({ entries: typing.slice(1) }, 500, new Set()).cutOff, "fewer lines than asked for: all of them").toBe(false);
  });

  it("remember the time of the read, so a log rotated after it is not taken for starts missed while the window was closed", async () => {
    let remembered = 0;
    const remember = (newest: number) => (remembered = Math.max(remembered, newest));
    const quiet: PhoneStartsIo = { audit: async () => ({ entries: [] }), names: async () => () => undefined, shown: () => new Set(), add: () => {}, cutOff: () => {}, remember };
    await loadMissedPhoneStarts(100, quiet, () => 5_000);
    expect(remembered, "nothing started, and still everything up to the read was seen").toBe(5_000);
    // Later the log rotates (lots of typing while the window is open): its oldest line is newer than any start, but older than the read.
    expect(missedPhoneStarts({ entries: [], from: 4_000 }, remembered, new Set()).cutOff).toBe(false);
    expect(missedPhoneStarts({ entries: [], from: 6_000 }, remembered, new Set()).cutOff, "rotated after the read: some may be missing").toBe(true);
  });
});