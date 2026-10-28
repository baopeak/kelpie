import { describe, expect, it } from "vitest";
import { keyAnswer, nextWaiting, waitedFor, waitingSessions } from "./attention";
import type { SessionInfo } from "./types";

const s = (id: string, status: string, waiting_since?: number, alive = true, created_at = 0) =>
  ({ id, status, waiting_since, alive, created_at }) as unknown as SessionInfo;

describe("attention", () => {
  const list = [s("a", "waiting", 300), s("b", "working"), s("c", "waiting", 100), s("d", "waiting", 200, false), s("e", "waiting")];

  it("orders live waiting sessions, the longest wait first", () => {
    expect(waitingSessions(list).map((x) => x.id)).toEqual(["c", "a", "e"]);
  });

  it("cycles through them", () => {
    expect(nextWaiting(list, null)?.id).toBe("c");
    expect(nextWaiting(list, "c")?.id).toBe("a");
    expect(nextWaiting(list, "e")?.id).toBe("c");
    expect(nextWaiting(list, "b")?.id).toBe("c");
    expect(nextWaiting([s("x", "idle")], null)).toBeUndefined();
  });

  it("says how long", () => {
    expect(waitedFor(0, 12_400)).toBe("12 s");
    expect(waitedFor(0, 185_000)).toBe("3 min");
    expect(waitedFor(0, 2 * 3600_000 + 5)).toBe("2 h");
    expect(waitedFor(10, 0)).toBe("0 s");
  });

  it("adds Enter only for line prompts", () => {
    expect(keyAnswer({ keys: ["y", "n"], line: true }, "y")).toEqual({ data: "y", enter: true });
    expect(keyAnswer({ keys: ["1", "2"], line: false }, "1")).toEqual({ data: "1", enter: false });
  });
});
