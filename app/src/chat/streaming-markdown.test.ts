import { describe, expect, it } from "vitest";
import { marked } from "marked";
import { openTable, stableEnd } from "./streaming-markdown";

describe("stableEnd", () => {
  it("cuts after the last blank line", () => {
    expect(stableEnd("one\n\ntwo\n\nthree")).toBe("one\n\ntwo\n\n".length);
    expect(stableEnd("one\n\n")).toBe("one\n\n".length);
  });

  it("does not cut before any blank line has arrived", () => {
    expect(stableEnd("")).toBe(0);
    expect(stableEnd("trailing text")).toBe(0);
    expect(stableEnd("trailing text\n")).toBe(0);
  });

  it("ignores blank lines inside a code fence", () => {
    expect(stableEnd("one\n\n```\ncode\n\nmore")).toBe("one\n\n".length);
    expect(stableEnd("one\n\n~~~\ncode\n\nmore")).toBe("one\n\n".length);
  });

  it("cuts again once the fence has closed", () => {
    const text = "```\ncode\n\nmore\n```\n\n";
    expect(stableEnd(text + "after")).toBe(text.length);
  });
});

describe("openTable", () => {
  it("is true once the delimiter row of a pipe table has arrived", () => {
    expect(openTable("| a | b |\n|---|---|")).toBe(true);
    expect(openTable("| a | b |\n| --- | --- |\n| 1 | 2")).toBe(true);
  });

  it("accepts tables without outer pipes", () => {
    expect(openTable("a | b\n--- | ---")).toBe(true);
    expect(openTable("a | b\n--- | ---\n1 | 2")).toBe(true);
  });

  it("accepts alignment colons", () => {
    expect(openTable("| a | b | c |\n|:---|:---:|---:|")).toBe(true);
    expect(openTable("a | b\n:-- | --:")).toBe(true);
  });

  it("is false for a lone pipe line", () => {
    expect(openTable("|")).toBe(false);
    expect(openTable("|\n")).toBe(false);
  });

  it("is false for a header whose delimiter has not arrived", () => {
    expect(openTable("| a | b |")).toBe(false);
    expect(openTable("| a | b |\n")).toBe(false);
    expect(openTable("| a | b |\n| 1 | 2 |")).toBe(false);
  });

  it("is false for prose containing a pipe", () => {
    expect(openTable("pipe it with a | b")).toBe(false);
    expect(openTable("pipe it with a | b\nand read the result")).toBe(false);
  });

  it("is false for a code fence, even one whose first line has a pipe", () => {
    expect(openTable("```\n| a | b |\n|---|---|")).toBe(false);
    expect(openTable("```a|b\n---")).toBe(false);
    expect(openTable("~~~a|b\n---")).toBe(false);
  });

  it("is false when the first line has no pipe", () => {
    expect(openTable("Heading\n---")).toBe(false);
  });

  it("waits until the delimiter row has a cell for every column", () => {
    expect(openTable("a | b\n-")).toBe(false);
    expect(openTable("a | b\n---")).toBe(false);
    expect(openTable("a | b\n---|")).toBe(false);
    expect(openTable("| a | b |\n|---")).toBe(false);
    expect(openTable("| a | b |\n|---|")).toBe(false);
    expect(openTable("| a | b | c |\n|---|---|")).toBe(false);
    expect(openTable("| a | b |\n|---|-")).toBe(true);
    expect(openTable("| a | b |\n|---|---|")).toBe(true);
    expect(openTable("| a |\n|---|")).toBe(true);
  });

  it("counts an escaped pipe as text, not as a cell boundary", () => {
    expect(openTable("a \\| b | c\n---|---")).toBe(true);
    expect(openTable("a \\| b | c\n---|---|---")).toBe(false);
    expect(openTable("a \\\\| b\n---|---")).toBe(true);
  });

  it("is false for a setext heading that happens to hold a pipe", () => {
    expect(openTable("| a\n---")).toBe(false);
    expect(openTable("a \\| b\n---")).toBe(false);
  });

  it("reads lines ending in CRLF", () => {
    expect(openTable("| a | b |\r\n|---|---|\r\n")).toBe(true);
    expect(openTable("| a | b |\r\n|---|---|")).toBe(true);
    expect(openTable("a | b\r\n--- | ---\r\n1 | 2")).toBe(true);
    expect(openTable("| a | b |\r\n|---\r\n")).toBe(false);
    expect(openTable("a | b\r\n---\r\n")).toBe(false);
  });

  it("agrees with the parser that renders the table", () => {
    const cases = [
      "| a | b |\n|---|---|", "a | b\n--- | ---", "a | b\n:-- | --:", "| a |\n|---|", "| a | b | \n|---|---| ", "a | b\n|---|---|",
      "a \\| b | c\n---|---", "a \\\\| b\n---|---", "| a | b |\r\n|---|---|\r\n",
      "a | b\n-", "a | b\n---", "a | b\n---|", "| a | b |\n|---", "| a | b |\n|---|", "| a | b | c |\n|---|---|", "| a\n---", "a \\| b\n---", "|\n---", "|\n|---|",
      "| a | b |", "pipe it with a | b\nand read the result", "```a|b\n---",
    ];
    for (const text of cases) expect(openTable(text), text).toBe(marked.lexer(text, { gfm: true })[0].type === "table");
  });

  it("gets through huge runs of whitespace and dashes: this runs on every delta", () => {
    const huge = (s: string) => s.repeat(80_000);
    expect(openTable(`a | b\n${huge(" ")}`)).toBe(false);
    expect(openTable(`a | b\n|${huge(" ")}x`)).toBe(false);
    expect(openTable(`a | b\n---${huge(" ")}x`)).toBe(false);
    expect(openTable(`a | b\n---${huge(" ")}|${huge(" ")}x`)).toBe(false);
    expect(openTable(`a | b\n${huge("-")}x`)).toBe(false);
    expect(openTable(`a | b\n--- | ---${huge(" ")}`)).toBe(true);
    expect(openTable(`${huge(" ")}a | b\n--- | ---`)).toBe(true);
  });
});
