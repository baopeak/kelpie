import { describe, expect, it } from "vitest";
import { inlineParts, proseBlocks } from "./prose";

describe("proseBlocks", () => {
  it("makes a run of bullet lines one list, between paragraphs", () => {
    expect(proseBlocks("Here is a draft:\n\n- one\n- two\n\nThen this.")).toEqual([
      { kind: "para", text: "Here is a draft:" },
      { kind: "list", ordered: false, items: ["one", "two"] },
      { kind: "para", text: "Then this." },
    ]);
  });

  it("keeps numbered lists apart from bullets, and line breaks inside a paragraph", () => {
    expect(proseBlocks("1. a\n2. b\n- c\nline one\nline two")).toEqual([
      { kind: "list", ordered: true, items: ["a", "b"] },
      { kind: "list", ordered: false, items: ["c"] },
      { kind: "para", text: "line one\nline two" },
    ]);
  });

  it("leaves text with no list as it is", () => {
    expect(proseBlocks("Just a reply.")).toEqual([{ kind: "para", text: "Just a reply." }]);
  });
});

describe("inlineParts", () => {
  it("finds bold and code, and leaves the rest as text", () => {
    expect(inlineParts("Draft for **0.1.21** in `CHANGELOG.md`.")).toEqual([
      { kind: "text", text: "Draft for " },
      { kind: "bold", text: "0.1.21" },
      { kind: "text", text: " in " },
      { kind: "code", text: "CHANGELOG.md" },
      { kind: "text", text: "." },
    ]);
  });

  it("leaves an unclosed mark as characters", () => {
    expect(inlineParts("a ** b ` c")).toEqual([{ kind: "text", text: "a ** b ` c" }]);
  });

  it("never turns text into markup: a tag stays text", () => {
    expect(inlineParts("<img src=x onerror=alert(1)> **<b>**")).toEqual([
      { kind: "text", text: "<img src=x onerror=alert(1)> " },
      { kind: "bold", text: "<b>" },
    ]);
  });
});
