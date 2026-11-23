import { describe, expect, it } from "vitest";
import { mainViewCovered } from "./page-cover";

describe("mainViewCovered", () => {
  it("is true for every full-window page", () => {
    for (const o of ["dashboard", "settings", "board", "runs", "hosts", "cost"] as const) expect(mainViewCovered(o)).toBe(true);
  });
  it("is false with nothing open and for dialogs and menus over the main view", () => {
    for (const o of [null, "palette", "quickopen", "jump", "newsession", "worktree", "remote", "update", "answer", "finish", "review", "try"] as const) {
      expect(mainViewCovered(o)).toBe(false);
    }
  });
});
