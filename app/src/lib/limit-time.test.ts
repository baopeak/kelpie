import { describe, expect, it } from "vitest";
import { nextClockTime, resetTime } from "./limit-time";
import type { ProviderUsage } from "./types";

const NOW = Date.UTC(2026, 9, 24, 7, 0, 0);
const usage = (...windows: [string, number, number | null][]): ProviderUsage => ({ windows: windows.map(([label, used_percent, resets_at]) => ({ label, used_percent, resets_at })), error: null });

describe("resetTime", () => {
  it("is the reset of the full window", () => {
    expect(resetTime(usage(["5h", 100, NOW + 3_600_000], ["week", 40, NOW + 86_400_000]), NOW)).toBe(NOW + 3_600_000);
  });

  it("waits for every full window: the latest reset", () => {
    expect(resetTime(usage(["5h", 100, NOW + 3_600_000], ["week", 100, NOW + 86_400_000]), NOW)).toBe(NOW + 86_400_000);
  });

  it("is unknown when nothing is full, nothing is ahead or a full window does not say", () => {
    expect(resetTime(usage(["5h", 99, NOW + 1000]), NOW)).toBeNull();
    expect(resetTime(usage(["5h", 100, NOW - 1]), NOW)).toBeNull();
    expect(resetTime(usage(["5h", 100, null]), NOW)).toBeNull();
    expect(resetTime(usage(["5h", 100, NOW + 1000], ["week", 100, null]), NOW)).toBeNull();
    expect(resetTime({ windows: [], error: "not signed in" }, NOW)).toBeNull();
    expect(resetTime(null, NOW)).toBeNull();
  });
});

describe("nextClockTime", () => {
  it("is today when the time is still ahead, else tomorrow, strictly after now", () => {
    const now = new Date(2026, 9, 24, 10, 30, 0).getTime();
    expect(new Date(nextClockTime(14, 5, now))).toEqual(new Date(2026, 9, 24, 14, 5, 0));
    expect(new Date(nextClockTime(9, 0, now))).toEqual(new Date(2026, 9, 25, 9, 0, 0));
    expect(new Date(nextClockTime(10, 30, now))).toEqual(new Date(2026, 9, 25, 10, 30, 0));
  });
});
