import { describe, expect, it } from "vitest";
import { describeTool, diffStats, lineDiff, unifiedFiles } from "./tool-view";
import type { ChatItem } from "./chat-model";

const tool = (name: string, input: unknown, extra: Partial<Extract<ChatItem, { kind: "tool" }>> = {}) =>
  ({ kind: "tool", id: "t", name, input, summary: "", status: "done", ...extra }) as Extract<ChatItem, { kind: "tool" }>;

describe("lineDiff", () => {
  it("keeps shared lines as context and marks only what changed", () => {
    expect(lineDiff("a\nb\nc", "a\nB\nc")).toBe(" a\n-b\n+B\n c");
    expect(lineDiff("x\ny", "z")).toBe("-x\n-y\n+z");
  });

  it("elides unchanged stretches beyond the context", () => {
    const before = Array.from({ length: 20 }, (_, i) => `l${i}`).join("\n");
    const after = before.replace("l1\n", "L1\n").replace("l18", "L18");
    expect(lineDiff(before, after, 1).split("\n")).toEqual([" l0", "-l1", "+L1", " l2", "@@", " l17", "-l18", "+L18", " l19"]);
  });

  it("stays cheap for a small change in a large text", () => {
    const big = Array.from({ length: 50_000 }, (_, i) => `line ${i}`).join("\n");
    const t0 = performance.now();
    const out = lineDiff(big, big.replace("line 25000\n", "changed\n"));
    expect(performance.now() - t0).toBeLessThan(500);
    expect(diffStats(out)).toEqual({ adds: 1, dels: 1 });
  });
});

describe("describeTool", () => {
  it("names Claude tools by what they do", () => {
    expect(describeTool(tool("Read", { file_path: "D:\\w\\app.ts", offset: 75, limit: 6 }))).toMatchObject({ category: "read", verb: "Read", target: "app.ts", detail: "lines 75-80" });
    expect(describeTool(tool("Bash", { command: "cargo test", description: "Run the tests" }))).toMatchObject({ category: "command", target: "Run the tests", command: "cargo test" });
    expect(describeTool(tool("Grep", { pattern: "foo", path: "/w/src" }))).toMatchObject({ category: "search", target: "foo", detail: "src" });
    expect(describeTool(tool("mcp__github__get_issue", {}, { summary: "#12" }))).toMatchObject({ verb: "github · get_issue", target: "#12" });
  });

  it("shows the streamed summary while the input is still arriving", () => {
    expect(describeTool(tool("Bash", undefined, { summary: "ls -la /tmp" })).target).toBe("ls -la /tmp");
  });
});

describe("unifiedFiles", () => {
  it("counts each file of a git diff", () => {
    const diff = "diff --git a/x.ts b/x.ts\n--- a/x.ts\n+++ b/x.ts\n@@\n-a\n+b\n+c\ndiff --git a/y b/y\n+new\n";
    expect(unifiedFiles(diff)).toEqual([{ path: "x.ts", adds: 2, dels: 1 }, { path: "y", adds: 1, dels: 0 }]);
  });
});
