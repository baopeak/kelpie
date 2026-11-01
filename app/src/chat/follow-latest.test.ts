import { describe, expect, it } from "vitest";
import { followsLatest } from "./follow-latest";

describe("followsLatest", () => {
  it("keeps following when a row grows and moves the bottom away", () => {
    expect(followsLatest(true, 640, 1000, 1000)).toBe(true);
    expect(followsLatest(true, 1300, 1200, 1000)).toBe(true);
  });

  it("stops once the user moves up past the bottom", () => {
    expect(followsLatest(true, 300, 700, 1000)).toBe(false);
  });

  it("does not stop for a small move that stays near the bottom", () => {
    expect(followsLatest(true, 20, 980, 1000)).toBe(true);
  });

  it("stays stopped while the user reads, even moving down", () => {
    expect(followsLatest(false, 300, 700, 500)).toBe(false);
    expect(followsLatest(false, 300, 700, 700)).toBe(false);
  });

  it("comes back on reaching the bottom", () => {
    expect(followsLatest(false, 10, 1000, 700)).toBe(true);
  });
});
