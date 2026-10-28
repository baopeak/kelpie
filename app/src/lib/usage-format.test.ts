import { describe, expect, it } from "vitest";
import { DANGER_AT, WARN_AT, formatCost, formatTokens, overCap, ringFraction, ringLevel } from "./usage-format";

describe("ring thresholds", () => {
  it("is amber from 70 % and red from 85 %", () => {
    expect(WARN_AT).toBe(0.7);
    expect(DANGER_AT).toBe(0.85);
    expect(ringLevel(0)).toBe("ok");
    expect(ringLevel(0.699)).toBe("ok");
    expect(ringLevel(0.7)).toBe("warn");
    expect(ringLevel(0.849)).toBe("warn");
    expect(ringLevel(0.85)).toBe("danger");
    expect(ringLevel(1)).toBe("danger");
  });

  it("has no fraction without a window or with a bad count", () => {
    expect(ringFraction(500, 1000)).toBe(0.5);
    expect(ringFraction(2000, 1000)).toBe(1);
    expect(ringFraction(500, null)).toBeNull();
    expect(ringFraction(500, 0)).toBeNull();
    expect(ringFraction(-1, 1000)).toBeNull();
    expect(ringFraction(NaN, 1000)).toBeNull();
  });
});

describe("formatting", () => {
  it("shortens token counts", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(999)).toBe("999");
    expect(formatTokens(1234)).toBe("1.2k");
    expect(formatTokens(12_345)).toBe("12k");
    expect(formatTokens(258_400)).toBe("258k");
    expect(formatTokens(1_000_000)).toBe("1M");
    expect(formatTokens(1_234_567)).toBe("1.2M");
    expect(formatTokens(11_884_468_362)).toBe("12B");
  });

  it("marks API-price estimates with ≈ and not the agent's own numbers", () => {
    expect(formatCost(0.4321, true)).toBe("$0.43");
    expect(formatCost(0.4321, false)).toBe("≈ $0.43");
    expect(formatCost(12.5, false)).toBe("≈ $12.50");
    expect(formatCost(1234.4, true)).toBe("$1,234");
    expect(formatCost(0.004, false)).toBe("≈ <$0.01");
    expect(formatCost(0.004, true)).toBe("<$0.01");
    expect(formatCost(0, true)).toBe("$0.00");
  });

  it("applies the cap only when it is set and the cost is known", () => {
    expect(overCap(5, 0)).toBe(false);
    expect(overCap(null, 5)).toBe(false);
    expect(overCap(4.99, 5)).toBe(false);
    expect(overCap(5, 5)).toBe(true);
  });
});
