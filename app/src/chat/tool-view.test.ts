import { describe, expect, it } from "vitest";
import { describeTool, diffStats, lineDiff, stripProjectRoot, unifiedFiles } from "./tool-view";
import { type ChatItem, summarize } from "./chat-model";

const tool = (name: string, input: unknown, extra: Partial<Extract<ChatItem, { kind: "tool" }>> = {}) =>
  ({ kind: "tool", id: "t", name, input, summary: "", status: "done", ...extra }) as Extract<ChatItem, { kind: "tool" }>;

describe("a question's row", () => {
  it("names the questions; the agent's own text for the answers stays its output", () => {
    const row = tool("AskUserQuestion", { questions: [{ question: "Which database?" }, { question: "Which port?" }] }, { output: 'User has answered your questions: "Which database?"="Postgres"' });
    expect(describeTool(row)).toMatchObject({ verb: "Questions", target: "Which database? · Which port?", category: "other" });
  });

  it("falls back to the summary when the input is not there yet", () => {
    expect(describeTool(tool("AskUserQuestion", {}, { summary: "Which?" })).target).toBe("Which?");
  });
});

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
    const changed = big.replace("line 25000\n", "changed\n");
    expect(diffStats(lineDiff(big, changed))).toEqual({ adds: 1, dels: 1 });
    // The best of several runs: a garbage collection or a busy neighbour must not decide it (all runs are in the message).
    const runs = Array.from({ length: 5 }, () => {
      const t0 = performance.now();
      lineDiff(big, changed);
      return performance.now() - t0;
    }).sort((a, b) => a - b);
    expect(runs[0], `${runs.map((r) => r.toFixed(1)).join(" / ")} ms`).toBeLessThan(500);
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

  it("keeps an agent's whole command for the IN block, and its short summary on the row", () => {
    const long = `echo ${"x".repeat(295)}`;
    const view = describeTool(tool("Run", { command: long }, { category: "command", summary: summarize({ command: long }) }));
    expect(view.command).toBe(long);
    expect(view.target).toBe(summarize({ command: long }));
    expect(view.target.length).toBe(121);
  });

  it("reads the command in every form an agent sends it", () => {
    const command = (input: unknown, summary = "cut…") => describeTool(tool("Run", input, { category: "command", summary })).command;
    expect(command({ command: ["git", "status"] })).toBe("git status");
    expect(command({ cmd: "ls -la" })).toBe("ls -la");
    expect(command({ cmd: ["ls", "-la"] })).toBe("ls -la");
    expect(command("make all")).toBe("make all");
  });

  it("falls back to the summary when the input names no command", () => {
    expect(describeTool(tool("Run", { other: 1 }, { category: "command", summary: "/w/script.sh" })).command).toBe("/w/script.sh");
    expect(describeTool(tool("Run", undefined, { category: "command", summary: "ls" })).command).toBe("ls");
  });

  it("names Codex shells Run and shows what they ran", () => {
    const view = describeTool(tool("Shell", { command: "git status" }, { category: "command", summary: "git status" }));
    expect(view).toMatchObject({ verb: "Run", target: "git status", command: "git status" });
  });
});

describe("describeTool's file", () => {
  const tool = (name: string, input: unknown, extra: object = {}) => ({ kind: "tool", id: "1", name, summary: "", status: "done", input, ...extra }) as any;

  it("names the file a Read opens, at its offset", () => {
    expect(describeTool(tool("Read", { file_path: "D:\\p\\a.ts", offset: 40, limit: 5 }))).toMatchObject({ path: "D:\\p\\a.ts", line: 40 });
    const whole = describeTool(tool("Read", { file_path: "D:\\p\\a.ts" }));
    expect(whole.path).toBe("D:\\p\\a.ts");
    expect(whole.line).toBeUndefined();
    expect(describeTool(tool("Read", { file_path: "a.ts", offset: -3 })).line).toBeUndefined();
  });

  it.each([
    ["Write", { file_path: "a.ts", content: "x" }, "a.ts"],
    ["Edit", { file_path: "a.ts" }, "a.ts"],
    ["MultiEdit", { file_path: "a.ts" }, "a.ts"],
    ["NotebookEdit", { notebook_path: "n.ipynb" }, "n.ipynb"],
  ])("names the file %s changes", (name, input, path) => {
    expect(describeTool(tool(name, input)).path).toBe(path);
  });

  it("takes a generic edit's file from its first diff", () => {
    const view = describeTool(tool("Apply patch", {}, { category: "edit", diff: [{ path: "x/a.ts", text: "+1" }, { path: "b.ts", text: "+2" }] }));
    expect(view.path).toBe("x/a.ts");
  });

  it("has no file while the input is still arriving, or for other tools", () => {
    expect(describeTool(tool("Read", {})).path).toBeUndefined();
    expect(describeTool(tool("Bash", { command: "ls" })).path).toBeUndefined();
    expect(describeTool(tool("Grep", { pattern: "x", path: "src" })).path).toBeUndefined();
  });
});

describe("stripProjectRoot", () => {
  it("removes a Windows root", () => {
    expect(stripProjectRoot("D:\\a\\b\\src\\x.ts", "D:\\a\\b")).toBe("src\\x.ts");
  });

  it("removes a POSIX root", () => {
    expect(stripProjectRoot("/home/u/p/src/x.ts", "/home/u/p")).toBe("src/x.ts");
  });

  it("accepts either slash style in the text and the root", () => {
    expect(stripProjectRoot("D:/a/b/src/x.ts", "D:\\a\\b")).toBe("src/x.ts");
    expect(stripProjectRoot("D:\\a/b\\src/x.ts", "D:/a/b")).toBe("src/x.ts");
  });

  it("ignores case when the root has a drive letter", () => {
    expect(stripProjectRoot("d:\\A\\b\\x.ts", "D:\\a\\B")).toBe("x.ts");
  });

  it("respects case when it does not", () => {
    expect(stripProjectRoot("/Home/u/p/x.ts", "/home/u/p")).toBe("/Home/u/p/x.ts");
  });

  it("leaves a path outside the root unchanged", () => {
    expect(stripProjectRoot("D:\\other\\x.ts", "D:\\a\\b")).toBe("D:\\other\\x.ts");
    expect(stripProjectRoot("/etc/hosts", "/home/u/p")).toBe("/etc/hosts");
  });

  it("leaves a sibling sharing the prefix unchanged", () => {
    expect(stripProjectRoot("D:\\a\\bc\\x.ts", "D:\\a\\b")).toBe("D:\\a\\bc\\x.ts");
    expect(stripProjectRoot("/home/u/pp/x.ts", "/home/u/p")).toBe("/home/u/pp/x.ts");
  });

  it("leaves a deeper path that merely contains the root unchanged", () => {
    expect(stripProjectRoot("/mnt/home/u/p/x.ts", "/home/u/p")).toBe("/mnt/home/u/p/x.ts");
  });

  it("leaves the bare root unchanged: nothing would be left to show", () => {
    expect(stripProjectRoot("D:\\a\\b", "D:\\a\\b")).toBe("D:\\a\\b");
  });

  it("removes every occurrence, wherever the path sits in the text", () => {
    expect(stripProjectRoot("D:\\a\\b\\x.ts · D:\\a\\b\\y.ts", "D:\\a\\b")).toBe("x.ts · y.ts");
    expect(stripProjectRoot('grep "foo" /home/u/p/src', "/home/u/p")).toBe('grep "foo" src');
  });

  it("leaves the root with nothing after its separator unchanged", () => {
    expect(stripProjectRoot("/home/u/p/", "/home/u/p")).toBe("/home/u/p/");
    expect(stripProjectRoot("D:\\a\\b\\", "D:\\a\\b")).toBe("D:\\a\\b\\");
    expect(stripProjectRoot("grep foo /home/u/p/ --x", "/home/u/p")).toBe("grep foo /home/u/p/ --x");
    expect(stripProjectRoot("grep foo /home/u/p// --x", "/home/u/p")).toBe("grep foo /home/u/p// --x");
    expect(stripProjectRoot('"/home/u/p/"', "/home/u/p")).toBe('"/home/u/p/"');
    expect(stripProjectRoot("D:\\a\\b\\ · D:\\a\\b\\y.ts", "D:\\a\\b")).toBe("D:\\a\\b\\ · y.ts");
  });

  it("leaves a root inside a URL or a remote path unchanged", () => {
    expect(stripProjectRoot("file:///home/u/p/x.ts", "/home/u/p")).toBe("file:///home/u/p/x.ts");
    expect(stripProjectRoot("host:/home/u/p/x.ts", "/home/u/p")).toBe("host:/home/u/p/x.ts");
  });

  it("leaves a root after a non-ASCII folder name unchanged", () => {
    expect(stripProjectRoot("/tmp/é/home/u/p/x", "/home/u/p")).toBe("/tmp/é/home/u/p/x");
    expect(stripProjectRoot("/tmp/e\u0301/home/u/p/x", "/home/u/p")).toBe("/tmp/e\u0301/home/u/p/x");
    expect(stripProjectRoot("/tmp/日本/home/u/p/x", "/home/u/p")).toBe("/tmp/日本/home/u/p/x");
    expect(stripProjectRoot("/tmp/٣/home/u/p/x", "/home/u/p")).toBe("/tmp/٣/home/u/p/x");
  });

  it("strips a root that is itself not ASCII", () => {
    expect(stripProjectRoot("d:\\DỰ ÁN\\x.ts", "D:\\dự án")).toBe("x.ts");
    expect(stripProjectRoot("/home/é/p/x.ts", "/home/é/p")).toBe("x.ts");
  });

  it("copes with a root ending in a separator or holding regex characters", () => {
    expect(stripProjectRoot("D:\\a\\b\\x.ts", "D:\\a\\b\\")).toBe("x.ts");
    expect(stripProjectRoot("/w/a+b (1)/x.ts", "/w/a+b (1)")).toBe("x.ts");
  });

  it("does nothing for an empty or filesystem root", () => {
    expect(stripProjectRoot("/etc/hosts", "")).toBe("/etc/hosts");
    expect(stripProjectRoot("/etc/hosts", "/")).toBe("/etc/hosts");
  });
});

describe("unifiedFiles", () => {
  it("counts each file of a git diff", () => {
    const diff = "diff --git a/x.ts b/x.ts\n--- a/x.ts\n+++ b/x.ts\n@@\n-a\n+b\n+c\ndiff --git a/y b/y\n+new\n";
    expect(unifiedFiles(diff)).toEqual([{ path: "x.ts", adds: 2, dels: 1 }, { path: "y", adds: 1, dels: 0 }]);
  });
});
