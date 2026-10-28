import { describe, expect, it } from "vitest";
import { MAX_HISTORY, remember, searchHistory, stepHistory } from "./prompt-history";

describe("remember", () => {
  it("puts the newest first and moves a repeated prompt to the front", () => {
    let list = remember(undefined, "one");
    list = remember(list, "two");
    list = remember(list, " one ");
    expect(list).toEqual(["one", "two"]);
  });

  it("ignores blank text and does not change the list it was given", () => {
    const list = ["a"];
    expect(remember(list, "  \n")).toEqual(["a"]);
    remember(list, "b");
    expect(list).toEqual(["a"]);
  });

  it("keeps the newest hundred", () => {
    let list: string[] = [];
    for (let i = 0; i < MAX_HISTORY + 20; i++) list = remember(list, `p${i}`);
    expect(list).toHaveLength(MAX_HISTORY);
    expect(list[0]).toBe(`p${MAX_HISTORY + 19}`);
    expect(list.at(-1)).toBe("p20");
  });

  it("keeps projects apart when each has its own list", () => {
    const history: Record<string, string[]> = {};
    history.a = remember(history.a, "in a");
    history.b = remember(history.b, "in b");
    expect(history).toEqual({ a: ["in a"], b: ["in b"] });
  });
});

describe("stepHistory", () => {
  it("walks to older prompts with up and back to the draft with down", () => {
    expect(stepHistory(3, -1, 1)).toBe(0);
    expect(stepHistory(3, 0, 1)).toBe(1);
    expect(stepHistory(3, 2, 1)).toBe(2);
    expect(stepHistory(3, 1, -1)).toBe(0);
    expect(stepHistory(3, 0, -1)).toBe(-1);
    expect(stepHistory(3, -1, -1)).toBe(-1);
  });

  it("does nothing without history", () => {
    expect(stepHistory(0, -1, 1)).toBe(-1);
  });
});

describe("searchHistory", () => {
  const list = ["fix the login bug", "add tests for @src/auth.ts", "Refactor the Login form"];
  it("matches every word, in any case, newest first", () => {
    expect(searchHistory(list, "LOGIN the")).toEqual(["fix the login bug", "Refactor the Login form"]);
    expect(searchHistory(list, "auth")).toEqual(["add tests for @src/auth.ts"]);
    expect(searchHistory(list, "zzz")).toEqual([]);
  });
  it("returns everything for an empty query, up to the limit", () => {
    expect(searchHistory(list, "")).toEqual(list);
    expect(searchHistory(list, "  ", 2)).toEqual(list.slice(0, 2));
  });
});
