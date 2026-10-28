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
