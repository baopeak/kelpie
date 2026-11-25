import { beforeEach, describe, expect, it, vi } from "vitest";

const inbox: { title: string; body: string; sessionId?: string }[] = [];
const sessions: Record<string, { id: string; name: string; project_id: null }> = { s1: { id: "s1", name: "work", project_id: null } };
const reply = { current: { timers: [] as { session: string; fire_at: number }[], lost: [] as { session: string }[] } };

vi.mock("./store", () => ({
  store: { sessions, addInbox: (i: (typeof inbox)[number]) => inbox.push(i), notify: vi.fn(), connected: () => true, daemonApi: () => 12 },
  sessionLabel: (s: { name: string }) => s.name,
}));
vi.mock("./ipc", () => ({ daemon: vi.fn(async () => reply.current), usageFetch: vi.fn() }));
vi.mock("./accounts", () => ({ sessionHome: () => undefined }));

const { loadScheduled, scheduled } = await import("./limits");

beforeEach(() => {
  inbox.length = 0;
  reply.current = { timers: [], lost: [] };
});

describe("loadScheduled", () => {
  it("replaces the list: a timer the daemon no longer has is gone, not merged back", async () => {
    reply.current = { timers: [{ session: "a", fire_at: 1 }, { session: "b", fire_at: 2 }], lost: [] };
    await loadScheduled();
    expect({ ...scheduled }).toEqual({ a: 1, b: 2 });
    reply.current = { timers: [{ session: "b", fire_at: 3 }], lost: [] };
    await loadScheduled();
    expect({ ...scheduled }).toEqual({ b: 3 });
    reply.current = { timers: [], lost: [] };
    await loadScheduled();
    expect({ ...scheduled }).toEqual({});
  });

  it("gives each timer a stopped daemon lost an inbox note", async () => {
    reply.current = { timers: [], lost: [{ session: "s1" }, { session: "gone" }] };
    await loadScheduled();
    expect(inbox.map((i) => i.title)).toEqual(["work: “continue” was not sent", "A session: “continue” was not sent"]);
    expect(inbox[0].body).toBe("Cancelled because the session daemon restarted.");
    expect(inbox[0].sessionId).toBe("s1");
  });
});

const { sessionState } = await import("./session-state");
const { isLimited } = await import("./limits");
const { continueAt, continueTime, continueTimeShort } = await import("./continue-timers");

describe("a session waiting on its timer", () => {
  const limited = (id: string, chat: boolean) =>
    ({ id, alive: true, status: "idle", failed: true, limited: true, unread: false, chat }) as unknown as import("./types").SessionInfo;

  it("is no failure while a timer will continue it, and is one again once the timer goes", async () => {
    for (const chat of [false, true]) {
      const s = limited(chat ? "c1" : "t1", chat);
      expect(isLimited(s), "terminals and chats get the banner").toBe(true);
      reply.current = { timers: [{ session: s.id, fire_at: 42 }], lost: [] };
      await loadScheduled();
      expect(continueAt(s)).toBe(42);
      expect(sessionState(s)).toBe("idle");
      reply.current = { timers: [], lost: [] };
      await loadScheduled();
      expect(continueAt(s)).toBeUndefined();
      expect(sessionState(s)).toBe("error");
    }
  });

  it("only a session the limit stopped and that still runs has a time to show", async () => {
    reply.current = { timers: [{ session: "x", fire_at: 7 }], lost: [] };
    await loadScheduled();
    expect(continueAt({ ...limited("x", false), limited: false })).toBeUndefined();
    expect(continueAt({ ...limited("x", false), alive: false })).toBeUndefined();
    expect(sessionState({ ...limited("x", false), limited: false })).toBe("error");
  });
});

describe("continueTime", () => {
  it("is the time today, and names the day when it is another day (a weekly limit)", () => {
    const now = new Date(2026, 10, 24, 10, 0).getTime();
    const later = new Date(2026, 10, 24, 15, 20).getTime();
    const friday = new Date(2026, 10, 27, 12, 0).getTime();
    expect(continueTime(later, now)).not.toMatch(/[A-Za-z]{3}/);
    expect(continueTime(later, now)).toMatch(/15:20|3:20/);
    expect(continueTime(friday, now)).toMatch(/Fri/);
  });
});

describe("continueTimeShort", () => {
  it("is the hour and minute in 24-hour digits today, and keeps the day for another day", () => {
    const now = new Date(2026, 10, 24, 10, 0).getTime();
    const later = new Date(2026, 10, 24, 15, 20).getTime();
    const friday = new Date(2026, 10, 27, 12, 5).getTime();
    const midnight = new Date(2026, 10, 27, 0, 5).getTime();
    expect(continueTimeShort(later, now)).toBe("15:20");
    expect(continueTimeShort(friday, now)).toMatch(/^Fri,? 12:05$/);
    // Midnight and noon do not read the same without AM/PM.
    expect(continueTimeShort(midnight, now)).toMatch(/^Fri,? 0?0:05$/);
    expect(continueTimeShort(later, now).length).toBeLessThan(continueTime(later, now).length);
  });
});
