import { describe, expect, it } from "vitest";
import { splitHint } from "./hover-hint";

describe("splitHint", () => {
  it("draws a trailing shortcut as keys", () => {
    expect(splitHint("Compose (Ctrl+Shift+I)")).toEqual({ label: "Compose", keys: "Ctrl+Shift+I" });
    expect(splitHint("Maximize pane (⌘⇧↩)")).toEqual({ label: "Maximize pane", keys: "⌘⇧↩" });
  });

  it("keeps a title without a shortcut whole", () => {
    expect(splitHint("Split right")).toEqual({ label: "Split right", keys: "" });
  });

  it("leaves brackets inside the text alone", () => {
    expect(splitHint("Back to terminal (TUI) now")).toEqual({ label: "Back to terminal (TUI) now", keys: "" });
    expect(splitHint("(only brackets)")).toEqual({ label: "(only brackets)", keys: "" });
  });
});
