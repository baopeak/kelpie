import { describe, expect, it } from "vitest";
import { pasteInput, typedText } from "./typed-text";

// Same cases as `session::tests::typed_text_drops_escapes_that_could_end_a_paste` (daemon).
describe("typedText", () => {
  it("drops escapes that could end a bracketed paste", () => {
    expect(typedText("a\x1b[201~\rrm -rf ~\r")).toBe("a[201~\nrm -rf ~\n");
  });

  it("drops C0 / C1 controls but keeps tabs and line breaks", () => {
    expect(typedText("x\u009b31m\u0007y\tz\r\nw")).toBe("x31my\tz\nw");
  });

  it("leaves ordinary text alone", () => {
    expect(typedText("héllo — 日本\n\tindented")).toBe("héllo — 日本\n\tindented");
  });
});

describe("pasteInput", () => {
  it("keeps the paste markers a program asked for and cleans the text inside them", () => {
    expect(pasteInput("a\x1b[201~\nrm -rf ~\n", true)).toBe("\x1b[200~a[201~\rrm -rf ~\r\x1b[201~");
  });

  it("sends keystrokes when the program did not ask for markers", () => {
    expect(pasteInput("one\r\ntwo", false)).toBe("one\rtwo");
  });
});
