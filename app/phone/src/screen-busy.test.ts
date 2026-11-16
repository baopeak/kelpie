import { describe, expect, it } from "vitest";
import { BUSY_RETRY_MS, busyText } from "./screen-busy";

const t = (s: string, vars?: Record<string, string | number>) => (vars ? s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : s);

describe("why the computer is too busy for its screen", () => {
  it("names the reason with the computer's numbers and what helps", () => {
    expect(busyText("memory", { memory_free: 11.4, memory_free_mb: 812 }, "x", t)).toBe("Only 11 % of the computer's memory is free (812 MB): too little to show its screen. Close some programs on the computer.");
    expect(busyText("memory", { memory_free: null, memory_free_mb: null }, "x", t), "the test hook pins the load: no numbers").toBe("The computer is short of memory: too little to show its screen. Close some programs on the computer.");
    expect(busyText("cpu", { cpu: 98.6 }, "x", t)).toBe("The computer's processor is fully busy (99 %). Wait for a build or another heavy task on it to finish.");
    expect(busyText("cpu", {}, "x", t)).toBe("The computer's processor is fully busy. Wait for a build or another heavy task on it to finish.");
    expect(busyText("lag", { lag_ms: 250 }, "x", t)).toBe("The computer is responding slowly. Wait for a build or another heavy task on it to finish.");
    expect(busyText("thermal", {}, "x", t)).toBe("The computer is too hot and slowed itself down. Let it cool down.");
    expect(busyText("load", {}, "x", t)).toBe("The computer is too busy to show its screen now.");
  });

  it("takes the reason from the numbers when the reply has none, and the computer's own words from one that sends neither", () => {
    expect(busyText(null, { reason: "thermal" }, "x", t)).toBe("The computer is too hot and slowed itself down. Let it cool down.");
    expect(busyText(undefined, undefined, "The computer is too busy to show its screen now. Try again in a moment.", t)).toBe("The computer is too busy to show its screen now. Try again in a moment.");
  });

  it("asks again a bounded number of times, never sooner than ten seconds apart", () => {
    expect(BUSY_RETRY_MS.length).toBeLessThanOrEqual(8);
    expect(Math.min(...BUSY_RETRY_MS)).toBeGreaterThanOrEqual(10_000);
    expect(BUSY_RETRY_MS.reduce((a, b) => a + b, 0)).toBeLessThanOrEqual(5 * 60_000);
  });
});
