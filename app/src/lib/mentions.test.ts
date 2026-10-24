import { describe, expect, it, vi } from "vitest";
import { MAX_SNIPPET_LINES, MAX_TOTAL_BYTES, droppedFileMention, expandMentions, formatNotes, looksLikeMention, mentionAt, parseMentions, pathMention, readsAtPath, shellQuote, terminalMention, type MentionContext } from "./mentions";
import type { ReviewNote } from "./types";

/** A context over an in-memory project; `redact` masks anything that looks like `sk-…`. */
function fake(files: Record<string, string>, extra: Partial<MentionContext> = {}): MentionContext & { stashed: { label: string; text: string }[] } {
  const stashed: { label: string; text: string }[] = [];
  const dirs = new Set(Object.keys(files).flatMap((p) => p.split(/[\\/]/).slice(0, -1).map((_, i, a) => a.slice(0, i + 1).join("/"))));
  return {
    stashed,
    stat: async (p) => (p in files ? { dir: false } : dirs.has(p.replace(/[\\/]$/, "")) ? { dir: true } : null),
    readText: async (p) => (p in files ? { text: files[p], binary: files[p].includes("\0"), big: false } : null),
    selection: async () => null,
    diff: async () => "",
    terminal: async () => null,
    issue: async () => null,
    notes: () => [],
    redact: async (t) => t.replace(/sk-[A-Za-z0-9]{8,}/g, "[redacted]"),
    stash: async (label, text) => {
      stashed.push({ label, text });
      return `C:\\Temp\\kelpie-composer\\${label}-1.txt`;
    },
    ...extra,
  };
}

const lines = (n: number, prefix = "line") => Array.from({ length: n }, (_, i) => `${prefix} ${i + 1}`).join("\n");

describe("parseMentions", () => {
  it("finds files, keywords and prefixed names", () => {
    const text = "look at @src/app.ts and @selection, then @diff. ask @terminal:build about @issue:ABC-12 and @note";
    expect(parseMentions(text).map((m) => [m.kind, m.arg])).toEqual([
      ["path", "src/app.ts"],
      ["selection", ""],
      ["diff", ""],
      ["terminal", "build"],
      ["issue", "ABC-12"],
      ["note", ""],
    ]);
  });

  it("keeps the offsets of the mention without the punctuation after it", () => {
    const text = "see @a/b.ts, ok";
    const [m] = parseMentions(text);
    expect(text.slice(m.start, m.end)).toBe("@a/b.ts");
  });

  it("does not take an e-mail address or a bare @ for a mention", () => {
    expect(parseMentions("write to me@example.com or @ later")).toEqual([]);
  });

  it("reads quoted names with spaces and unicode, including Windows paths", () => {
    const text = 'compare @"C:\\Users\\Nguyễn Văn A\\Đồ án\\báo cáo.md" with @"src/my file.ts" and @terminal:"dev server"';
    expect(parseMentions(text).map((m) => [m.kind, m.arg])).toEqual([
      ["path", "C:\\Users\\Nguyễn Văn A\\Đồ án\\báo cáo.md"],
      ["path", "src/my file.ts"],
      ["terminal", "dev server"],
    ]);
  });

  it("treats a keyword inside a quoted name as a file called that", () => {
    expect(parseMentions('@"diff"').map((m) => [m.kind, m.arg])).toEqual([["path", "diff"]]);
  });

  it("builds mentions the parser reads back", () => {
    for (const p of ["a.ts", "my docs/a b.ts", "C:\\Users\\Jo Ann\\tệp.txt"]) {
      expect(parseMentions(pathMention(p))[0]).toMatchObject({ kind: "path", arg: p });
    }
    expect(parseMentions(terminalMention("dev server"))[0]).toMatchObject({ kind: "terminal", arg: "dev server" });
  });
});

describe("mentionAt", () => {
  it("finds the mention under the caret and what is typed of it", () => {
    expect(mentionAt("read @src/ap", 12)).toEqual({ start: 5, query: "src/ap" });
    expect(mentionAt("@", 1)).toEqual({ start: 0, query: "" });
    expect(mentionAt("a @x and @y", 4)).toEqual({ start: 2, query: "x" });
  });

  it("follows an open quote, spaces included", () => {
    const text = 'see @"my docs/re';
    expect(mentionAt(text, text.length)).toEqual({ start: 4, query: "my docs/re" });
  });

  it("finds nothing after the mention is finished, or in an e-mail address", () => {
    expect(mentionAt("read @a.ts and", 14)).toBeNull();
    expect(mentionAt('see @"my docs/a.md" ', 20)).toBeNull();
    expect(mentionAt("me@example.com", 14)).toBeNull();
  });
});

describe("expandMentions for agents that read @path", () => {
  it.each(["claude", "codex", "gemini"])("%s gets @path, no content", async (kind) => {
    const ctx = fake({ "src/app.ts": "const a = 1;\n" });
    const r = await expandMentions("explain @src/app.ts please", ctx, kind);
    expect(r).toEqual({ text: "explain @src/app.ts please", inlined: false, problems: [], refused: [] });
  });

  it("keeps folders and quotes paths with spaces", async () => {
    const ctx = fake({ "my docs/read me.md": "hi", "src/a.ts": "x" });
    const r = await expandMentions('@src and @"my docs/read me.md"', ctx, "claude");
    expect(r.text).toBe('@src and @"my docs/read me.md"');
  });

  it("handles a Windows path with spaces and unicode", async () => {
    const path = "C:\\Users\\Nguyễn Văn A\\Đồ án\\báo cáo.md";
    const ctx = fake({ [path]: "nội dung" });
    expect((await expandMentions(`đọc @"${path}" nhé`, ctx, "codex")).text).toBe(`đọc @"${path}" nhé`);
    const shell = await expandMentions(`đọc @"${path}" nhé`, ctx, "omp");
    expect(shell.text).toBe(`đọc \`${path}\` nhé\n\nFile \`${path}\`:\n\`\`\`md\nnội dung\n\`\`\``);
  });

  it("still inlines selections, diffs and terminal output: they have no path", async () => {
    const ctx = fake({}, { diff: async () => "diff --git a/x b/x\n+new" });
    const r = await expandMentions("review @diff", ctx, "claude");
    expect(r.inlined).toBe(true);
    expect(r.text).toBe("review the diff below\n\nUncommitted changes (git diff):\n```diff\ndiff --git a/x b/x\n+new\n```");
  });
});

describe("expandMentions for shells and other programs", () => {
  it.each(["omp", "opencode"])("%s gets the file fenced and labelled with its path", async (kind) => {
    const ctx = fake({ "src/app.ts": "const a = 1;\nconst b = 2;\n" });
    const r = await expandMentions("explain @src/app.ts", ctx, kind);
    expect(r.inlined).toBe(true);
    expect(r.text).toBe("explain `src/app.ts`\n\nFile `src/app.ts`:\n```ts\nconst a = 1;\nconst b = 2;\n```");
  });

  it("gives a folder as a path, never a listing", async () => {
    const ctx = fake({ "src/a.ts": "x" });
    const r = await expandMentions("list @src", ctx, "omp");
    expect(r).toEqual({ text: "list src", inlined: false, problems: [], refused: [] });
  });

  it("gives a binary file as a path", async () => {
    const ctx = fake({ "logo.bin": "ab\0cd" });
    expect((await expandMentions("see @logo.bin", ctx, "omp")).text).toBe("see logo.bin");
  });

  it("quotes a path with spaces when only the path is given", async () => {
    const ctx = fake({ "my docs/a.bin": "a\0" });
    expect((await expandMentions('open @"my docs/a.bin"', ctx, "omp")).text).toBe('open "my docs/a.bin"');
  });

  it("uses a longer fence when the content has backtick fences of its own", async () => {
    const content = "text\n```js\nx\n````\n";
    const ctx = fake({ "README.md": content });
    const r = await expandMentions("@README.md", ctx, "omp");
    expect(r.text).toContain("`````md\ntext\n```js\nx\n````\n`````");
  });

  it("leaves what it cannot resolve as typed", async () => {
    const ctx = fake({ "a.ts": "x" });
    const r = await expandMentions("thanks @alice, see @missing/file.ts", ctx, "omp");
    expect(r).toEqual({ text: "thanks @alice, see @missing/file.ts", inlined: false, problems: [], refused: [] });
  });
});

describe("paths only", () => {
  it("replaces every snippet with a path when nothing may be inlined", async () => {
    const ctx = fake({ "a.ts": "x" }, { diff: async () => "+new", selection: async () => ({ text: "s", path: "a.ts", startLine: 1, endLine: 1 }), terminal: async () => "out" });
    const r = await expandMentions("@a.ts @diff @selection @terminal:t", ctx, "shell", { shell: "pwsh" });
    expect(r.inlined).toBe(false);
    expect(r.text).toBe("a.ts C:\\Temp\\kelpie-composer\\diff-1.txt a.ts:1 C:\\Temp\\kelpie-composer\\terminal-1.txt");
  });
});

describe("caps", () => {
  it("inlines a file of exactly 200 lines and refers to one of 201", async () => {
    const ctx = fake({ "ok.txt": lines(MAX_SNIPPET_LINES), "long.txt": lines(MAX_SNIPPET_LINES + 1) });
    const ok = await expandMentions("@ok.txt", ctx, "omp");
    expect(ok.inlined).toBe(true);
    const long = await expandMentions("@long.txt", ctx, "omp");
    expect(long).toEqual({ text: "long.txt", inlined: false, problems: [], refused: [] });
  });

  it("does not count the line break that ends a file", async () => {
    const ctx = fake({ "ok.txt": lines(MAX_SNIPPET_LINES) + "\n" });
    expect((await expandMentions("@ok.txt", ctx, "omp")).inlined).toBe(true);
  });

  it("falls back to paths once 64 KB are used, keeping the order", async () => {
    const chunk = "x".repeat(30 * 1024);
    const ctx = fake({ "a.txt": chunk, "b.txt": chunk, "c.txt": chunk, "d.txt": "tiny" });
    const r = await expandMentions("@a.txt @b.txt @c.txt @d.txt", ctx, "omp");
    // a and b fit (60 KB); c would pass 64 KB; d is small enough for what is left.
    expect(r.text.split("\n\n")[0]).toBe("`a.txt` `b.txt` c.txt `d.txt`");
    expect(r.text).toContain("File `d.txt`:");
    expect(r.text).not.toContain("File `c.txt`");
    expect(new TextEncoder().encode(r.text).length).toBeLessThan(MAX_TOTAL_BYTES + 1024);
  });

  it("counts bytes, not characters", async () => {
    const wide = "đ".repeat(MAX_TOTAL_BYTES / 2 + 1);
    const ctx = fake({ "wide.txt": wide });
    expect((await expandMentions("@wide.txt", ctx, "omp")).inlined).toBe(false);
  });

  it("refers to a long selection of a file by its lines", async () => {
    const text = lines(MAX_SNIPPET_LINES + 5);
    const ctx = fake({}, { selection: async () => ({ text, path: "src/big.ts", startLine: 10, endLine: 214 }) });
    expect((await expandMentions("fix @selection", ctx, "claude")).text).toBe("fix @src/big.ts (lines 10-214)");
    expect((await expandMentions("fix @selection", ctx, "omp")).text).toBe("fix src/big.ts:10-214");
  });

  it("saves a long selection without a file to a temporary file", async () => {
    const text = lines(MAX_SNIPPET_LINES + 5);
    const ctx = fake({}, { selection: async () => ({ text }) });
    expect((await expandMentions("look at @selection", ctx, "claude")).text).toBe("look at @C:\\Temp\\kelpie-composer\\selection-1.txt");
    expect((await expandMentions("look at @selection", ctx, "omp")).text).toBe("look at C:\\Temp\\kelpie-composer\\selection-1.txt");
    expect(ctx.stashed[0]).toEqual({ label: "selection", text });
  });

  it("saves a long diff to a temporary file", async () => {
    const diff = lines(MAX_SNIPPET_LINES + 1, "+added");
    const ctx = fake({}, { diff: async () => diff });
    const r = await expandMentions("@diff", ctx, "omp");
    expect(r).toEqual({ text: "C:\\Temp\\kelpie-composer\\diff-1.txt", inlined: false, problems: [], refused: [] });
  });
});

describe("selection, diff and notes", () => {
  it("labels a selection with its file and lines", async () => {
    const ctx = fake({}, { selection: async () => ({ text: "let x = 1;", path: "src/a.rs", startLine: 4, endLine: 4 }) });
    const r = await expandMentions("what does @selection do?", ctx, "claude");
    expect(r.text).toBe("what does the selection below do?\n\nSelection from `src/a.rs:4`:\n```rs\nlet x = 1;\n```");
  });

  it("labels a selection from a terminal without a file", async () => {
    const ctx = fake({}, { selection: async () => ({ text: "error[E0425]" }) });
    expect((await expandMentions("@selection", ctx, "omp")).text).toBe("the selection below\n\nSelection:\n```\nerror[E0425]\n```");
  });

  it("reports an empty selection and leaves the mention", async () => {
    const ctx = fake({}, { selection: async () => ({ text: "  \n" }) });
    const r = await expandMentions("fix @selection", ctx, "claude");
    expect(r.text).toBe("fix @selection");
    expect(r.problems.map((p) => p.kind)).toEqual(["selection"]);
  });

  it("reports a diff with no changes", async () => {
    const r = await expandMentions("@diff", fake({}), "claude");
    expect(r.problems.map((p) => p.kind)).toEqual(["diff"]);
  });

  it("lists review notes by file and line", async () => {
    const note = (path: string, line: number, side: "new" | "old", text: string, n: string): ReviewNote => ({ id: `${path}${line}`, path, line, side, text, note: n, at: 0 });
    const notes = [note("b.ts", 3, "new", "  y()", "rename"), note("a.ts", 20, "old", "", "why removed?"), note("a.ts", 2, "new", "x()", "add a test")];
    expect(formatNotes(notes)).toBe("- a.ts:2 `x()`: add a test\n- a.ts:20 (removed line): why removed?\n- b.ts:3 `y()`: rename");
    const r = await expandMentions("address @note", fake({}, { notes: () => notes }), "omp");
    expect(r.text).toBe(`address the review notes below\n\nReview notes on the current changes:\n\`\`\`\n${formatNotes(notes)}\n\`\`\``);
    expect((await expandMentions("@note", fake({}), "omp")).problems.map((p) => p.kind)).toEqual(["note"]);
  });
});

describe("redaction", () => {
  const secret = "sk-ABCDEFGH12345678";

  it("masks credentials in @terminal before they are inlined", async () => {
    const terminal = vi.fn(async () => `$ export KEY=${secret}\nok`);
    const ctx = fake({}, { terminal });
    const r = await expandMentions("why did @terminal:build fail", ctx, "claude");
    expect(terminal).toHaveBeenCalledWith("build");
    expect(r.text).toContain("export KEY=[redacted]");
    expect(r.text).not.toContain(secret);
    expect(r.text).toContain("Recent output of terminal `build`:");
  });

  it("masks credentials in @issue", async () => {
    const ctx = fake({}, { issue: async () => `ABC-12: rotate key ${secret}` });
    const r = await expandMentions("@issue:ABC-12", ctx, "omp");
    expect(r.text).toContain("rotate key [redacted]");
    expect(r.text).not.toContain(secret);
  });

  it("masks what is saved to a temporary file, too", async () => {
    const ctx = fake({}, { terminal: async () => lines(MAX_SNIPPET_LINES + 1, secret) });
    const r = await expandMentions("@terminal:big", ctx, "claude");
    expect(r.text).toBe("@C:\\Temp\\kelpie-composer\\terminal-1.txt");
    expect(ctx.stashed[0].text).not.toContain(secret);
    expect(ctx.stashed[0].text).toContain("[redacted] 1");
  });

  it("asks redact only for terminal and issue text", async () => {
    const redact = vi.fn(async (t: string) => t);
    const ctx = fake({ "a.ts": "x" }, { redact, selection: async () => ({ text: "s" }), diff: async () => "d", notes: () => [{ id: "1", path: "a", line: 1, side: "new", text: "", note: "n", at: 0 }] });
    await expandMentions("@a.ts @selection @diff @note", ctx, "omp");
    expect(redact).not.toHaveBeenCalled();
    await expandMentions("@terminal:x @issue:Y", { ...ctx, terminal: async () => "t", issue: async () => "i" }, "omp");
    expect(redact.mock.calls.map((c) => c[0])).toEqual(["t", "i"]);
  });

  it("masks a selection made in a terminal, not one made in an editor", async () => {
    const secret = "sk-ABCDEFGH12345678";
    const terminal = fake({}, { selection: async () => ({ text: `KEY=${secret}`, source: "terminal" }) });
    const r = await expandMentions("@selection", terminal, "omp");
    expect(r.text).toContain("KEY=[redacted]");
    expect(r.text).not.toContain(secret);
    const editor = fake({}, { selection: async () => ({ text: `KEY=${secret}`, path: "a.ts", startLine: 1 }) });
    expect((await expandMentions("@selection", editor, "omp")).text).toContain(secret);
  });

  it("reports an unknown session or issue", async () => {
    const r = await expandMentions("@terminal:nope @issue:X-1", fake({}), "claude");
    expect(r.text).toBe("@terminal:nope @issue:X-1");
    expect(r.problems.map((p) => [p.kind, p.arg])).toEqual([["terminal", "nope"], ["issue", "X-1"]]);
  });
});

const HOSTILE = ["$(calc).txt", "a;calc;.txt", "a&calc&.txt", "a`calc`.txt", "it's.txt", "x $(id) y.txt", "run.ps1", "setup.exe"];

describe("dropped files", () => {
  it("a name with a line break is not a mention the parser reads, so it is refused and counts as one", () => {
    const name = "x $(touch pwned)\n.txt";
    expect(parseMentions(pathMention(name))).toEqual([]);
    expect(droppedFileMention(name)).toBeNull();
    expect(looksLikeMention(pathMention(name))).toBe(true);
  });

  it("a safe name comes back as the mention the composer writes", () => {
    expect(droppedFileMention("my docs/a b.md")).toBe('@"my docs/a b.md"');
    expect(droppedFileMention("a\u202eb")).toBeNull();
  });

  it("text without an @ is not mention-like", () => {
    expect(looksLikeMention("echo hi")).toBe(false);
    expect(looksLikeMention("me@example.com")).toBe(false);
    expect(looksLikeMention("echo @x")).toBe(true);
  });
});

describe("shellQuote", () => {
  it("leaves plain names bare and quotes everything else as one word", () => {
    expect(shellQuote("src/app.ts", "pwsh")).toBe("src/app.ts");
    expect(shellQuote("C:\\Users\\Jo\\báo cáo.md", "pwsh")).toBe("'C:\\Users\\Jo\\báo cáo.md'");
    expect(shellQuote("$(calc).txt", "pwsh")).toBe("'$(calc).txt'");
    expect(shellQuote("a;calc;.txt", "posix")).toBe("'a;calc;.txt'");
    expect(shellQuote("a`calc`.txt", "pwsh")).toBe("'a`calc`.txt'");
    expect(shellQuote("-rf", "posix")).toBe("./-rf");
    expect(shellQuote("-a b", "pwsh")).toBe("'./-a b'");
    expect(shellQuote("-rf", "cmd")).toBe(".\\-rf");
  });

  it("doubles a quote in PowerShell and closes and reopens it in POSIX shells", () => {
    expect(shellQuote("it's.txt", "pwsh")).toBe("'it''s.txt'");
    expect(shellQuote("it\u2019s.txt", "pwsh")).toBe("'it\u2019\u2019s.txt'");
    expect(shellQuote("it's.txt", "posix")).toBe("'it'\\''s.txt'");
  });

  it("refuses control characters, line breaks and bidi marks, and for cmd what it would expand", () => {
    for (const style of ["pwsh", "posix", "cmd"] as const) {
      for (const bad of ["a\nb.txt", "a\rb", "a\u001bb", "a\u202eb.txt", "a\u0085b"]) expect(shellQuote(bad, style)).toBeNull();
    }
    for (const bad of ["%PATH%.txt", "a&b", "a^b", "a!b", 'a"b']) expect(shellQuote(bad, "cmd")).toBeNull();
    expect(shellQuote("my docs/a.md", "cmd")).toBe('"my docs/a.md"');
  });
});

describe("a shell only gets quoted paths", () => {
  const files = Object.fromEntries([...HOSTILE, "a b.txt", "inject.txt"].map((n) => [n, "content\nline"]));

  it.each(["pwsh", "posix"] as const)("%s: every hostile name is one quoted word and no content is inlined", async (style) => {
    const ctx = fake(files);
    for (const name of HOSTILE) {
      const r = await expandMentions(`type @"${name}"`, ctx, "shell", { shell: style });
      const word = shellQuote(name, style)!;
      expect(r).toEqual({ text: `type ${word}`, inlined: false, problems: [], refused: [] });
      expect(r.text).not.toContain("content");
    }
  });

  it("sends a selection, a diff or terminal output as a quoted temporary file", async () => {
    const ctx = fake({}, { diff: async () => "+x", selection: async () => ({ text: "s" }), terminal: async () => "out" });
    const r = await expandMentions("@diff @selection @terminal:t", ctx, "shell", { shell: "pwsh" });
    expect(r.text).toBe("C:\\Temp\\kelpie-composer\\diff-1.txt C:\\Temp\\kelpie-composer\\selection-1.txt C:\\Temp\\kelpie-composer\\terminal-1.txt");
    const spaced = fake({}, { diff: async () => "+x", stash: async () => "C:\\Users\\Jo Ann\\Temp\\d.txt" });
    expect((await expandMentions("@diff", spaced, "custom", { shell: "pwsh" })).text).toBe("'C:\\Users\\Jo Ann\\Temp\\d.txt'");
  });

  it("refuses a name that cannot be written safely instead of sending it", async () => {
    const ctx = fake({ "a\u001bb.txt": "x", "ok.txt": "x" });
    const r = await expandMentions('type @"a\u001bb.txt" @ok.txt', { ...ctx, stat: async () => ({ dir: false }) }, "shell", { shell: "pwsh" });
    expect(r.refused.map((x) => [x.mention.arg, x.reason])).toEqual([["a\u001bb.txt", "unsafe-name"]]);
  });

  it("treats shell and custom as shells even without the option", async () => {
    const r = await expandMentions("@inject.txt", fake({ "inject.txt": "x\ny" }), "custom");
    expect(r.inlined).toBe(false);
    expect(r.text).toBe("inject.txt");
  });
});

describe("agents' own @ forms", () => {
  it("quotes for Gemini as Gemini's own parser reads it", async () => {
    const files = { "foo(1).ts": "x", "plain.ts": "x", "my file.md": "x" };
    const ask = (windows: boolean) => expandMentions('@"foo(1).ts" @plain.ts @"my file.md"', fake(files), "gemini", { windows });
    expect((await ask(true)).text).toBe('@"foo(1).ts" @plain.ts @"my file.md"');
    expect((await ask(false)).text).toBe("@foo\\(1\\).ts @plain.ts @my\\ file.md");
  });

  it("writes names the parser would cut at in quotes when completing", () => {
    for (const p of ["foo(1).ts", "a;b", "x,y]", "dir/end."]) expect(parseMentions(pathMention(p))[0]).toMatchObject({ kind: "path", arg: p });
  });
});

describe("a session on another machine", () => {
  const big = Array.from({ length: 300 }, (_, i) => `out ${i + 1}`).join("\n");

  it("gets the last lines of a long terminal, never a local temporary file", async () => {
    const r = await expandMentions("@terminal:t", fake({}, { terminal: async () => big }), "claude", { remote: true });
    expect(r.refused).toEqual([]);
    expect(r.text).toContain("(last 200 lines):");
    expect(r.text).toContain("out 300");
    expect(r.text).not.toContain("out 100\n");
    expect(r.text).not.toContain("kelpie-composer");
  });

  it("refuses text that would have to be a local file for a shell", async () => {
    const r = await expandMentions("@diff", fake({}, { diff: async () => "+x" }), "shell", { shell: "posix", remote: true });
    expect(r.refused.map((x) => x.reason)).toEqual(["remote"]);
  });
});

describe("readsAtPath", () => {
  it("is Claude, Codex and Gemini", () => {
    expect(["claude", "codex", "gemini", "shell", "omp", "custom"].filter(readsAtPath)).toEqual(["claude", "codex", "gemini"]);
  });
});
