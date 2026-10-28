import { describe, expect, it } from "vitest";
import { folderName, groupRows, weekStart } from "./usage-report";

describe("weeks", () => {
  it("start on Monday", () => {
    // 2026-10-24 is a Saturday = day 20750; its Monday is 2026-10-19 = day 20745.
    expect(weekStart(20_750)).toBe(20_745);
    expect(weekStart(20_745)).toBe(20_745);
    expect(weekStart(20_751)).toBe(20_745);
    expect(weekStart(20_752)).toBe(20_752);
    expect(weekStart(0)).toBe(-3);
  });
});

describe("report grouping", () => {
  const rows = [
    { day: 20_750, agent: "claude", project: "D:\\w\\a", model: "opus-5-5", tokens_in: 100, tokens_out: 10, cost: 1, unpriced: 0 },
    { day: 20_750, agent: "claude", project: "D:\\w\\b", model: "opus-5-5", tokens_in: 50, tokens_out: 5, cost: 0.5, unpriced: 0 },
    { day: 20_751, agent: "omp", project: "D:\\w\\a", model: "gpt-x", tokens_in: 10, tokens_out: 1, cost: 0.25, unpriced: 0 },
    { day: 20_740, agent: "codex", project: "D:\\w\\a", model: "gpt-6.1-sol", tokens_in: 1000, tokens_out: 100, cost: 0, unpriced: 1100 },
  ];

  it("folds by day, newest first", () => {
    const g = groupRows(rows, "day");
    expect(g.map((x) => x.key)).toEqual(["20751", "20750", "20740"]);
    expect(g[1]).toMatchObject({ tokensIn: 150, tokensOut: 15, cost: 1.5, estimate: true });
  });

  it("folds weeks onto their Monday and honours the first day", () => {
    expect(groupRows(rows, "week").map((x) => x.key)).toEqual(["20745", "20738"]);
    expect(groupRows(rows, "day", 20_745).map((x) => x.key)).toEqual(["20751", "20750"]);
  });

  it("orders other dimensions by cost, keeps unpriced tokens apart, and does not call OMP an estimate", () => {
    const byAgent = groupRows(rows, "agent");
    expect(byAgent.map((x) => x.key)).toEqual(["claude", "omp", "codex"]);
    expect(byAgent.find((x) => x.key === "omp")?.estimate).toBe(false);
    expect(byAgent.find((x) => x.key === "codex")).toMatchObject({ cost: 0, unpriced: 1100, estimate: false });
    expect(groupRows(rows, "project")[0]).toMatchObject({ key: "D:\\w\\a", cost: 1.25 });
    expect(groupRows(rows, "model").length).toBe(3);
  });

  it("names a project by its folder", () => {
    expect(folderName("D:\\Works\\Kelpie")).toBe("Kelpie");
    expect(folderName("/home/u/proj/")).toBe("proj");
    expect(folderName("")).toBe("");
  });
});
