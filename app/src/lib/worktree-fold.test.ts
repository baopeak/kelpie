import { describe, expect, it } from "vitest";
import { WORKTREE_FOLD_LIMIT, foldWorktrees, isNotableWorktree } from "./worktree-fold";

const quiet = { active: false, sessions: false, info: { dirty: false, ahead: 0 } };

describe("isNotableWorktree", () => {
  it("keeps the open worktree, one with sessions, and one with uncommitted or unpushed work", () => {
    expect(isNotableWorktree(quiet)).toBe(false);
    expect(isNotableWorktree({ ...quiet, active: true })).toBe(true);
    expect(isNotableWorktree({ ...quiet, sessions: true })).toBe(true);
    expect(isNotableWorktree({ ...quiet, info: { dirty: true, ahead: 0 } })).toBe(true);
    expect(isNotableWorktree({ ...quiet, info: { dirty: false, ahead: 2 } })).toBe(true);
  });

  it("does not count commits it is behind, or git state not read yet", () => {
    const behindOnly = { dirty: false, ahead: 0, behind: 82 };
    expect(isNotableWorktree({ ...quiet, info: behindOnly })).toBe(false);
    expect(isNotableWorktree({ active: false, sessions: false })).toBe(false);
    expect(isNotableWorktree({ active: false, sessions: false, info: null })).toBe(false);
  });
});

describe("foldWorktrees", () => {
  const names = (n: number) => Array.from({ length: n }, (_, i) => `w${i + 1}`);
  const notableOf = (...ids: string[]) => (w: string) => ids.includes(w);

  it("shows every worktree, in order, up to the limit", () => {
    const list = names(WORKTREE_FOLD_LIMIT);
    expect(foldWorktrees(list, () => false)).toEqual({ shown: list, hidden: [] });
  });

  it("above the limit shows the notable ones first and folds the rest, both in their order", () => {
    const list = names(8);
    expect(foldWorktrees(list, notableOf("w6", "w2"))).toEqual({ shown: ["w2", "w6"], hidden: ["w1", "w3", "w4", "w5", "w7", "w8"] });
  });

  it("folds from one above the limit", () => {
    expect(foldWorktrees(names(WORKTREE_FOLD_LIMIT + 1), notableOf("w3"))).toEqual({ shown: ["w3"], hidden: ["w1", "w2", "w4", "w5"] });
  });

  it("folds everything when no worktree is notable", () => {
    expect(foldWorktrees(names(6), () => false)).toEqual({ shown: [], hidden: names(6) });
  });

  it("never folds a single row (a \"1 more\" row saves nothing)", () => {
    const list = names(6);
    expect(foldWorktrees(list, notableOf("w1", "w2", "w3", "w5", "w6"))).toEqual({ shown: list, hidden: [] });
    expect(foldWorktrees(list, () => true)).toEqual({ shown: list, hidden: [] });
  });

  it("brings a folded worktree back once it becomes notable", () => {
    const list = names(7);
    expect(foldWorktrees(list, notableOf("w1")).hidden).toContain("w5");
    const after = foldWorktrees(list, notableOf("w1", "w5"));
    expect(after.shown).toEqual(["w1", "w5"]);
    expect(after.hidden).not.toContain("w5");
  });

  it("takes another limit", () => {
    expect(foldWorktrees(names(3), () => false, 2)).toEqual({ shown: [], hidden: names(3) });
  });
});
