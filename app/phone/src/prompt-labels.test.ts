import { describe, expect, it } from "vitest";
import { promptLabels } from "./prompt-labels";

const CLAUDE = [
  "Bash(rm -rf node_modules/.vite && pnpm build)",
  " Do you want to proceed?",
  " ❯ 1. Yes",
  "   2. Yes, and don't ask again for pnpm build",
  "   3. No, and tell Claude what to do differently (esc)",
  "",
];

describe("promptLabels", () => {
  it("names each key from the menu on screen, marker or not", () => {
    expect(promptLabels(CLAUDE, ["1", "3"])).toEqual({ "1": "Yes", "3": "No, and tell Claude what to do differently (esc)" });
  });

  it("reads the last menu, not a numbered list above it", () => {
    const rows = ["Plan:", "1. Read the file", "2. Edit it", "", ...CLAUDE];
    expect(promptLabels(rows, ["1", "2", "3"])["1"]).toBe("Yes");
  });

  it("keeps an option whose text wrapped onto the next line", () => {
    const rows = [" Do you want to proceed?", " ❯ 1. Yes", "   2. Yes, and don't ask again for this very long", "      command in this folder", "   3. No"];
    expect(promptLabels(rows, ["1", "3"])).toEqual({ "1": "Yes", "3": "No" });
  });

  it("gives nothing when a key is not listed, listed twice or out of order", () => {
    expect(promptLabels(CLAUDE, ["1", "4"])).toEqual({});
    expect(promptLabels([...CLAUDE.slice(0, 3), "   1. Again"], ["1"])).toEqual({});
    expect(promptLabels(CLAUDE, ["3", "1"])).toEqual({});
  });

  it("gives nothing for a prompt with no option lines (y/n)", () => {
    expect(promptLabels(["Overwrite the file? [y/N]"], ["y", "n"])).toEqual({});
    expect(promptLabels([], ["1"])).toEqual({});
  });
});
