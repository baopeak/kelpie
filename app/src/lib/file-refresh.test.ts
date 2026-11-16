import { describe, expect, it } from "vitest";
import { asksFor, refreshAsk, refreshFiles, reread } from "./file-refresh";

describe("asking open files to read again", () => {
  it("reaches every file of the project, or the one named", () => {
    refreshFiles("ssh://box/srv/app");
    const all = refreshAsk();
    expect(asksFor(all, "ssh://box/srv/app", "src/a.ts")).toBe(true);
    expect(asksFor(all, "ssh://box/srv/other", "src/a.ts")).toBe(false);
    refreshFiles("ssh://box/srv/app", "src/a.ts");
    const one = refreshAsk();
    expect(one.n).toBe(all.n + 1);
    expect(asksFor(one, "ssh://box/srv/app", "src/a.ts")).toBe(true);
    expect(asksFor(one, "ssh://box/srv/app", "src/b.ts")).toBe(false);
  });
  it("asks nothing before the first refresh", () => {
    expect(asksFor({ root: "", path: null, n: 0 }, "", "a")).toBe(false);
  });
});

describe("a fresh read of an open file", () => {
  const held = { text: "one\n", truncated: false };
  it("is the same when the text is, whatever its time says", () => {
    expect(reread({ text: "one\n", truncated: false }, held, false)).toBe("same");
    expect(reread({ text: "one\n", truncated: false }, held, true)).toBe("same");
  });
  it("replaces the text when nothing is unsaved", () => {
    expect(reread({ text: "two\n", truncated: false }, held, false)).toBe("apply");
    expect(reread({ text: null, truncated: false }, held, false)).toBe("apply");
  });
  it("keeps unsaved edits and says the file changed", () => {
    expect(reread({ text: "two\n", truncated: false }, held, true)).toBe("changed");
  });
  it("treats a file that grew past the editing limit as changed", () => {
    expect(reread({ text: "one\n", truncated: true }, held, false)).toBe("apply");
  });
});
