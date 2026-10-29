import { describe, expect, it } from "vitest";
import { filterBranches, parseStashSubject } from "./git-basics";
import type { GitBranch } from "./ipc";

const b = (name: string, current = false): GitBranch => ({ name, current, worktree: null, time: 0 });

describe("filterBranches", () => {
  const list = [b("feature/login"), b("main", true), b("fix-Login-redirect"), b("release")];

  it("puts the current branch first and keeps the recency order", () => {
    expect(filterBranches(list, "").map((x) => x.name)).toEqual(["main", "feature/login", "fix-Login-redirect", "release"]);
  });

  it("matches any part of the name, ignoring case and outer spaces", () => {
    expect(filterBranches(list, "  LOGIN ").map((x) => x.name)).toEqual(["feature/login", "fix-Login-redirect"]);
    expect(filterBranches(list, "zzz")).toEqual([]);
  });
});

describe("parseStashSubject", () => {
  it("reads a stash made with a message", () => {
    expect(parseStashSubject("On main: half-done refactor")).toEqual({ branch: "main", message: "half-done refactor", wip: false });
  });

  it("reads a stash made without one (the commit it was made on)", () => {
    expect(parseStashSubject("WIP on feature/x: 1a2b3c4 fix: login: redirect")).toEqual({ branch: "feature/x", message: "fix: login: redirect", wip: true });
  });

  it("has no branch for a detached HEAD, and keeps unknown text as it is", () => {
    expect(parseStashSubject("On (no branch): try")).toEqual({ branch: null, message: "try", wip: false });
    expect(parseStashSubject("autostash")).toEqual({ branch: null, message: "autostash", wip: false });
  });
});
