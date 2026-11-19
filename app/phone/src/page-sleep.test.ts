import { describe, expect, it } from "vitest";
import { PageSleep, SLEPT_MS } from "./page-sleep";

describe("a page that slept without being hidden", () => {
  it("is found by a tick that comes far too late, with the time it went to sleep", () => {
    const s = new PageSleep();
    expect(s.tick(1_000), "the first tick knows nothing yet").toBeNull();
    expect(s.tick(2_000)).toBeNull();
    // A slow tick on a busy page is not a sleep.
    expect(s.tick(2_000 + SLEPT_MS - 1)).toBeNull();
    const woke = 2_000 + SLEPT_MS - 1 + 60_000;
    expect(s.tick(woke), "a minute asleep: since the last tick before it").toBe(2_000 + SLEPT_MS - 1);
    expect(s.tick(woke + 1_000), "once").toBeNull();
  });

  it("does not count the time it was hidden, or a sleep already handled", () => {
    const s = new PageSleep();
    s.tick(0);
    s.reset();
    expect(s.tick(60_000), "hidden meanwhile (its timers stop): the hiding said it all").toBeNull();
    expect(s.tick(61_000)).toBeNull();
  });
});
